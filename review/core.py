"""Game review: turns a recorded battle (the player's own stream: log + requests + choices) into a review.

    from review import core
    r = core.Reviewer().review(game)          # game: the player's stream (review/from_replay.py builds it)

Per decision of the reviewed player:
  * move turns: the one-turn search (nnsearch's recipe: top-k joint actions per side, sampled opponent builds, value-net
    leaves) with the action actually played ALWAYS among our candidates; every leaf value is a calibrated win
    probability, so the table is in win chances. Equilibrium (best play both sides) -> the position's win chance; each
    candidate's win chance against the opponent's equilibrium mix, its worst case, and how often best play picks it.
    loss = equilibrium value - the played action's value; label + accuracy from the loss; a move inside the recommended
    mix (>= 5%) is never worse than "best"/"excellent".
  * luck: (win chance at the next decision) - (the played action's expected win chance), plus the notable random events
    of the turn (crits, misses, flinches, ...) read from the log.
  * team preview: our options (top-k by the net's preview head + the one picked + one-swap / lead-swap variants) x their
    likely options (their preview head), each cell = calibrated value of the turn-1 position, averaged over a few sampled
    opponent sets (Teams_real); the pick's win chance vs the best, the best concrete swaps, and a damage-based matchup note.
Forced switches (after a faint) are listed but not rated yet.
"""
import json
import math
import os
import random
import re
import sys

import numpy as np
import torch

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, os.path.join(ROOT, "python"))
sys.path.insert(0, HERE)

import vgcsim  # noqa: E402
import nnsearch  # noqa: E402
import sh_search  # noqa: E402
import agent  # noqa: E402
import paths  # noqa: E402
import bridge  # noqa: E402
import replay  # noqa: E402
from calibrate import apply_cal  # noqa: E402
from netlib import AMP, amp, arch_of, build_net, load_weights  # noqa: E402

PASS = ("pass", 0, 0, False)
LABELS = [(1.0, "best"), (2.5, "excellent"), (5.0, "good"), (10.0, "inaccuracy"), (20.0, "mistake"), (1e9, "blunder")]
LUCK_TOKENS = {"-crit": "critical hit", "-miss": "miss", "cant": "couldn't move", "-activate": None}


def to_id(s):
    return re.sub(r"[^a-z0-9]", "", str(s).lower())


def accuracy_of(loss):
    """lichess's move accuracy from win-chance points lost (0 lost = 100)."""
    return max(0.0, min(100.0, 103.1668 * math.exp(-0.04354 * max(0.0, loss)) - 3.1669))


def label_of(loss, freq):
    # a move best play itself uses (>= 5%) is best / excellent, but only when it is within 5 points: the smoothed
    # equilibrium (smooth_equilibrium) gives some weight to clearly worse moves too
    if freq >= 0.05 and loss < 5:
        return "best" if loss < 2.5 else "excellent"
    for lim, name in LABELS:
        if loss < lim:
            return name
    return "blunder"


class Names:
    """Display names for ids (poke-env's data)."""

    def __init__(self):
        from poke_env.data import GenData
        d = GenData.from_gen(9)
        self.moves = {k: v.get("name", k) for k, v in d.moves.items()}
        self.species = {k: v.get("name", k) for k, v in d.pokedex.items()}

    def move(self, mid):
        return self.moves.get(to_id(mid), mid)

    def mon(self, sid):
        return self.species.get(to_id(sid), sid)


def load_sets(folder=os.path.join(paths.TEAMS, "Teams_real")):
    """species id -> list of set texts (export blocks) from the team pool: plausible opponent builds."""
    cache = os.path.join(HERE, "sets_cache.json")
    files = sorted(f for f in os.listdir(folder) if f.endswith(".txt"))
    if os.path.exists(cache):
        # (2026-10-11) rebuilt when the team pool changes: the Oct 8 cache predated the 5,634-team pool ('no known set
        # for Inteleon' at team preview)
        c = json.load(open(cache, encoding="utf-8"))
        if c.pop("_files", None) == len(files):
            return c
    sets = {}
    for f in files:
        if not f.endswith(".txt"):
            continue
        for blk in open(os.path.join(folder, f), encoding="utf-8").read().strip().split("\n\n"):
            blk = blk.strip()
            if not blk:
                continue
            head = blk.split("\n")[0].split(" @ ")[0].strip()
            parts = re.findall(r"\(([^)]*)\)", head)
            names = [p for p in parts if p not in ("M", "F")]
            species = names[-1] if names else re.sub(r"\s*\([MF]\)\s*$", "", head)
            sets.setdefault(to_id(species), []).append(blk)
    json.dump(dict(sets, _files=len(files)), open(cache, "w", encoding="utf-8"))
    return sets


# weight of the equilibrium in the opponent's assumed mix (the rest: the opponent-prediction head); 1 = the old review
OPP_EQ = float(os.environ.get("REVIEW_OPP_EQ", "0.5"))
# searches per decision, pooled (Reviewer.search)
SEEDS = int(os.environ.get("REVIEW_SEEDS", "8"))
MCTS_WORLDS = int(os.environ.get("REVIEW_MCTS_WORLDS", "8"))       # (2026-10-28) opponent-set worlds per MCTS run


# temperature of the smoothed equilibrium (per unit of win chance): options a few points apart still share the play
QRE_LAMBDA = float(os.environ.get("REVIEW_QRE_LAMBDA", "20"))
# weight of the search in the recommended move (AGENT_SEARCH_BETA = 10 in the agent). The agent applies it
# to RAW values (-1..1); the review's values are calibrated win chances (0..1, about half the spread): 10 x 2 = 20
BETA = float(os.environ.get("REVIEW_BETA", "20"))
# the team preview rule's weight (AGENT_PREVIEW_BETA, on raw values as in the agent)
PREVIEW_BETA = float(os.environ.get("REVIEW_PREVIEW_BETA", "10"))


def smooth_equilibrium(M, lam=None, iters=2000):
    """(2026-10-11) Logit (quantal response) equilibrium of the zero-sum matrix game M (rows ours, maximise): each side
    plays softmax(lam x its options' values). Unlike an exact equilibrium it changes smoothly with M: on near-ties the
    exact one jumped between pure strategies (game 101 turn 6: the played move 0 or 25 points worse depending on the
    seed, with the position's value 2 points apart). Damped fixed-point iteration."""
    lam = QRE_LAMBDA if lam is None else lam
    m, n = M.shape
    x, y = np.full(m, 1.0 / m), np.full(n, 1.0 / n)
    for k in range(iters):
        a = lam * (M @ y)
        b = -lam * (x @ M)
        xn = np.exp(a - a.max()); xn /= xn.sum()
        yn = np.exp(b - b.max()); yn /= yn.sum()
        step = 1.0 / (k + 2) if k < 200 else 0.005
        dx, dy = xn - x, yn - y
        x, y = x + step * dx, y + step * dy
        if max(np.abs(dx).max(), np.abs(dy).max()) < 1e-6:
            break
    return x, y


class Reviewer:
    def __init__(self, ckpt=os.environ.get("REVIEW_CKPT") or os.path.join(ROOT, "weights", "net.pt"),
                 cal=os.environ.get("REVIEW_CAL") or os.path.join(HERE, "calibration.json"), worlds=16, k=8,
                 preview_worlds=16, preview_k=12, device=None, preset="standard"):
        gpu = torch.cuda.is_available() and os.environ.get("VGC_DEVICE", "") != "cpu"      # VGC_DEVICE=cpu: no GPU
        self.dev = torch.device(device or ("cuda" if gpu else "cpu"))
        AMP["on"] = self.dev.type == "cuda"
        ck = torch.load(ckpt, map_location=self.dev, weights_only=False)
        sd = ck["net"] if isinstance(ck, dict) and "net" in ck else ck
        vocab = {"species": sd["e_species.weight"].shape[0], "ability": sd["e_ability.weight"].shape[0],
                 "item": sd["e_item.weight"].shape[0], "move": sd["e_move.weight"].shape[0]}
        self.net = build_net(vocab, agent.N_ACT, arch_of(sd)).to(self.dev).eval()
        load_weights(self.net, sd)
        import fastinfer                          # (2026-10-18) small calls replayed from CUDA graphs: identical outputs
        self.net = fastinfer.wrap(self.net)
        self.cal = json.load(open(cal))
        self.dex = vgcsim.Dex(paths.DEX)
        self.worlds, self.k, self.pw, self.pk = worlds, k, preview_worlds, preview_k
        self.preset = preset                      # sh_search.PRESETS: fast / standard / strong
        self.mcts_k = None                        # (2026-10-28) MCTS root candidates (ours, theirs); None = (12, 12)
        self.cell_mode = False                    # True while scoring replacement cells (no root cover needed)
        self.names = Names()
        self.sets = None
        self.options = [(list(f), list(l)) for f, l in vgcsim.preview_options()]

    # ------------------------------------------------------------------ helpers
    def p_battle(self, v):
        return apply_cal(self.cal["battle"], np.asarray(v, np.float64))

    def values(self, F, I):
        out = []
        with torch.no_grad(), amp():
            for s in range(0, len(F), 4096):
                out.append(self.net(torch.from_numpy(F[s:s + 4096]).to(self.dev), torch.from_numpy(I[s:s + 4096]).to(self.dev))[1]
                           .float().cpu().numpy())
        return np.concatenate(out) if out else np.zeros(0, np.float32)

    def describe(self, st, side, tup):
        """Human text for one slot's action tuple, from `side`'s point of view in engine state `st`."""
        kind, a, t, mega = tup
        if kind == "pass":
            return "(nothing)"
        mons = st["sides"][side]["mons"]
        if kind == "switch":
            return f"switch to {self.names.mon(mons[a]['species'])}" if a < len(mons) else "switch"
        return None   # moves need the acting slot: see describe_joint

    def describe_joint(self, st, side, joint, mark=False):
        if mark:
            return self.describe_joint_marked(st, side, joint)
        out = []
        sd = st["sides"][side]
        foe = st["sides"][1 - side]
        for slot, tup in enumerate(joint):
            kind, a, t, mega = tup
            act_idx = sd["active"][slot] if slot < len(sd["active"]) else None
            user = self.names.mon(sd["mons"][act_idx]["species"]) if act_idx is not None and act_idx < len(sd["mons"]) else f"slot {slot + 1}"
            if kind == "pass":
                continue
            if kind == "switch":
                out.append(f"{user}: {self.describe(st, side, tup)}")
                continue
            mv = sd["mons"][act_idx]["moves"][a][0] if act_idx is not None and a < len(sd["mons"][act_idx]["moves"]) else "?"
            txt = f"{user}: {'Mega Evolve + ' if mega else ''}{self.names.move(mv)}"
            if t in (1, 2) and t - 1 < len(foe["active"]):
                fi = foe["active"][t - 1]
                if fi < len(foe["mons"]):
                    txt += f" -> {self.names.mon(foe['mons'][fi]['species'])}"
            elif t in (-1, -2):
                ai = sd["active"][-t - 1] if -t - 1 < len(sd["active"]) else None
                if ai is not None and ai < len(sd["mons"]) and ai != act_idx:
                    txt += f" -> ally {self.names.mon(sd['mons'][ai]['species'])}"
            out.append(txt)
        return "; ".join(out) if out else "(nothing)"

    def describe_joint_marked(self, st, side, joint):
        """describe_joint with Pokemon / move names wrapped in [[ ]] (explanation text)."""
        names = self.names
        m_mon, m_move = names.mon, names.move
        try:
            names.mon = lambda sid: f"[[{m_mon(sid)}]]"
            names.move = lambda mid: f"[[{m_move(mid)}]]"
            return self.describe_joint(st, side, joint)
        finally:
            names.mon, names.move = m_mon, m_move

    # ------------------------------------------------------------------ the player's choice -> action indices
    def played_indices(self, choice, req, options, battle):
        """Each slot's action index (harness numbering) of the choice string the player sent, or None (pass)."""
        toks = [t.strip() for t in choice.split(",")]
        out = []
        for k in range(2):
            tok = toks[k] if k < len(toks) else "pass"
            out.append(self.match_token(tok, k, req, options[k], battle))
        return out

    def match_token(self, tok, k, req, opts, battle):
        w = tok.split()
        if not w or w[0] in ("pass", "default"):
            return None
        if w[0] == "switch" and len(w) > 1:
            arg = " ".join(w[1:])
            if arg.isdigit():
                pk = req["side"]["pokemon"][int(arg) - 1]
                target = to_id(pk["details"].split(",")[0])
                ident = to_id(pk["ident"].split(": ", 1)[-1])
            else:
                target = ident = to_id(arg)
            for a, o in opts.items():
                p = getattr(o, "order", None)
                if a >= 32 and p is not None and (to_id(getattr(p, "species", "")) in (target, ident)
                                                   or to_id(getattr(p, "name", "")) in (target, ident)):
                    return a
            return None
        if w[0] == "move" and len(w) > 1:
            mega = any(x in ("mega", "megax", "megay") for x in w[2:])
            nums = [x for x in w[2:] if re.fullmatch(r"-?\d+", x)]
            tgt = int(nums[0]) if nums else None
            mv = w[1]
            if mv.isdigit():
                act = req.get("active", [])
                mv = act[k]["moves"][int(mv) - 1]["id"] if k < len(act) and int(mv) - 1 < len(act[k]["moves"]) else mv
            mv = to_id(mv)
            cands = [(a, o) for a, o in opts.items() if a < 32 and to_id(getattr(getattr(o, "order", None), "id", "")) == mv
                     and bool(getattr(o, "mega", False)) == mega]
            for a, o in cands:
                if tgt is None or getattr(o, "move_target", 0) == tgt:
                    if tgt is None and getattr(o, "move_target", 0) not in (0, None, 1, 2) and any(
                            getattr(o2, "move_target", 0) in (1, 2) for _, o2 in cands):
                        continue                  # no target known (a replay where the move failed): prefer a foe
                    return a
            return cands[0][0] if cands else (36 if 36 in opts else None)
        return None

    # ------------------------------------------------------------------ one move turn
    def analyze_turn(self, eb, played, hidden, seed):
        """The sequential-halving search (sh_search, preset self.preset) with the played move forced in and never
        eliminated; every leaf a calibrated win chance. played=None: only the position's value."""
        maps = eb.legal_index_map(0)
        pt = None
        if played is not None:
            pt = tuple(maps[s].get(played[s], PASS) if played[s] is not None else PASS for s in (0, 1))
        if pt is None and self.cell_mode and self.preset.startswith("mcts"):
            # (2026-10-28) a replacement cell needs only the position's value: each MCTS run's own root equilibrium value
            # (without the root cover not every pair has a visit, so the pooled every-pair matrix can't be solved)
            return {"win": float(np.mean([self.mcts_run(eb, None, hidden, seed + 104729 * k)["value"]
                                          for k in range(max(1, SEEDS))]))}
        r = self.search(eb, pt, hidden, seed)
        self.realistic(r)
        # (2026-10-11, user) the position's win chance = the value of the move the bot itself would play (recommend), not
        # the best of ~60 noisy estimates: that maximum ran high (winner's curse), so a replacement scored 8% was followed
        # by a next turn 'worth' 23% with nothing random in between
        rec = self.recommend(r)
        V = float(r["q"][rec])
        if pt is None:
            return {"win": V}
        ours, theirs, x, q, Mall = r["ours"], r["theirs"], r["x"], r["q"], r["M_all"]
        pos = r["forced"]
        st = eb.state()
        worst = {c: float(np.nanmin(Mall[c])) for c in q}
        # by win chance against the realistic opponent ('why' covers the top 3 + the played move)
        cands = sorted(q, key=lambda c: -q[c])
        table = [{"action": self.describe_joint(st, 0, ours[c][1:]), "win": float(q[c]), "worst": worst[c],
                  "freq": float(x[c]), "policy": float(ours[c][0]), "played": c == pos, "recommended": c == rec} for c in cands]
        act_t, y = r["y_survivors"]
        opp = [{"action": self.describe_joint(st, 1, theirs[act_t[j]][1:]), "freq": float(y[j])}
               for j in np.argsort(-y) if y[j] >= 0.03]
        loss_raw = max(0.0, 100 * (V - q[pos]))              # measured against the recommended move
        noise = self.loss_noise(r, rec, pos)
        # (2026-10-11, user) only the part of the loss beyond the search's own noise counts (2 standard errors of the gap
        # between the best move and the played one, from the spread over the pooled runs); the bot's own pick costs nothing
        loss = 0.0 if pos == rec else max(0.0, loss_raw - 2 * noise)
        forced = len(ours) == 1
        why = []
        if not forced:
            try:
                why = self.explain(eb, r, cands, pos)
            except Exception as e:             # (an explanation never breaks the review)
                why = [{"action": "", "lines": [f"(no explanation: {type(e).__name__}: {e})"], "rank": 0}]
        sims = {}
        if not forced:
            try:                                # (2026-10-21) every option's likely outcome, for the page's option boards
                sims = self.option_sims(eb, r, cands)
            except Exception as e:
                sims = {"error": f"{type(e).__name__}: {e}"}
        return {"win": V, "played_win": float(q[pos]), "loss": loss, "label": "forced" if forced else label_of(loss, float(x[pos])),
                "accuracy": None if forced else accuracy_of(loss), "played": self.describe_joint(st, 0, ours[pos][1:]), "table": table,
                "opponent": opp, "raw_value": float(r["root_value"]), "leaves": int(r["leaves"]), "why": why, "sims": sims,
                "recommended": self.describe_joint(st, 0, ours[rec][1:]), "loss_raw": loss_raw, "noise": 2 * noise}

    def recommend(self, r):
        """The recommended move: argmax of log policy + BETA x (win chance - position value), on the review's pooled
        values."""
        V = r["value"]
        return max(r["q"], key=lambda c: np.log(max(float(r["ours"][c][0]), 1e-12)) + BETA * (r["q"][c] - V))

    def loss_noise(self, r, best, pos):
        """Standard error (win-chance points) of the gap between move `best` (the recommended one) and the played one, from the spread of
        that gap over the pooled runs (each run's values against the same realistic opponent mix). 0 with one run."""
        Mr = r.get("M_runs")
        if Mr is None or len(Mr) < 2:
            return 0.0
        act_t, y = r["y_survivors"]
        if best == pos:
            return 0.0
        d = []
        for Mk in Mr:
            a, b = Mk[best, act_t], Mk[pos, act_t]
            if np.all(np.isfinite(a)) and np.all(np.isfinite(b)):
                d.append(float((a - b) @ y))
        return 100 * float(np.std(d, ddof=1) / math.sqrt(len(d))) if len(d) >= 2 else 0.0

    def search(self, eb, pt, hidden, seed, seeds=None):
        """(2026-10-11) The halving search REVIEW_SEEDS times with different seeds, pooled: one run's value of a move swung by
        20-40 points in positions with unknown opponent sets (game 101 turn 6: 'blunder' with one seed, 'best' with the
        others). Every pair's estimate = the mean over the runs that evaluated it; the equilibrium is re-solved over all runs'
        survivors. Same result format as sh_search.halving_search."""
        if self.preset.startswith("mcts"):
            rs = [self.mcts_run(eb, pt, hidden, seed + 104729 * k) for k in range(max(1, seeds or SEEDS))]
        else:
            rs = [sh_search.halving_search(self.net, eb, 0, self.dev, hidden, seed=seed + 104729 * k, transform=self.p_battle,
                                           force=pt, **sh_search.PRESETS[self.preset]) for k in range(max(1, seeds or SEEDS))]
        r0 = rs[0]
        key = lambda r: [(o[1], o[2]) for o in r["ours"]] + [(o[1], o[2]) for o in r["theirs"]]
        if any(key(r) != key(r0) for r in rs[1:]):
            return r0                                       # (candidate lists differ: can't pool; never seen)
        with np.errstate(invalid="ignore"):
            Mall = np.nanmean(np.stack([r["M_all"] for r in rs]), axis=0)
        # the equilibrium over ALL candidates (every run evaluates every pair in its first round): each run's own shortlist of
        # their replies differs, and solving over a shortlist flipped the equilibrium between runs (the played move's value
        # 20% or 65% depending on which replies survived); 'survivors' (the best-move pool) = any run's survivors + played
        cols = [j for j in range(len(r0["theirs"])) if np.all(np.isfinite(Mall[:, j]))]
        rows = [c for c in range(len(r0["ours"])) if np.all(np.isfinite(Mall[c, cols]))]
        if not cols or not rows:
            # (2026-10-28) small searches (the search-depth setting's medium / light) leave some root pairs unvisited, so no
            # reply is evaluated against every option: fill each gap with its row's mean (our option's average result), else
            # the column's, else the overall mean, and keep the pairs that were visited at all
            with np.errstate(invalid="ignore"):
                rm, cm, am = np.nanmean(Mall, axis=1), np.nanmean(Mall, axis=0), np.nanmean(Mall)
            Mall = np.where(np.isfinite(Mall), Mall, np.where(np.isfinite(rm)[:, None], rm[:, None],
                                                              np.where(np.isfinite(cm)[None, :], cm[None, :], am)))
            cols = [j for j in range(len(r0["theirs"])) if np.all(np.isfinite(Mall[:, j]))]
            rows = [c for c in range(len(r0["ours"])) if np.all(np.isfinite(Mall[c, cols]))]
        M = Mall[np.ix_(rows, cols)]
        x, y = smooth_equilibrium(M)
        xfull = np.zeros(len(r0["ours"]))
        xfull[rows] = x
        pool = sorted(set().union(*[r["survivors"] for r in rs]) | ({r0["forced"]} if r0.get("forced") is not None else set()))
        q = {c: float(Mall[c, cols] @ y) for c in rows}
        return dict(r0, M_all=Mall, M_runs=[r["M_all"] for r in rs], x=xfull, survivors=[c for c in pool if c in q], value=float(x @ M @ y), q=q,
                    y_survivors=(cols, y),
                    leaves=sum(r["leaves"] for r in rs), root_value=float(np.mean([r["root_value"] for r in rs])))

    def mcts_run(self, eb, pt, hidden, seed):
        """(2026-10-28) preset "mcts" / "mcts<N>": the multi-turn tree search (mcts.py, N simulations, default 800, 3 turns
        deep) in REVIEW_MCTS_WORLDS opponent-set worlds (eb.search_worlds: our side = side 0, opponent stats guessed /
        sampled), the played move forced in, every root (our, their) pair visited at least once, leaves calibrated"""
        import mcts
        sims = int(self.preset[4:] or 800)
        worlds = eb.search_worlds(0, MCTS_WORLDS, seed) if hasattr(eb, "search_worlds") else eb
        k = self.mcts_k or (12, sh_search.K_THEIRS or 12)
        # (2026-10-28, search-depth setting) every root pair is visited once only where the options' values are shown (a rated
        # turn); a replacement cell needs only the position's value, so it runs its plain simulation count
        cover = not self.cell_mode
        n_pairs = (k[0] + 1) * k[1]                                     # root candidates (+ the forced one) x replies
        return mcts.search(self.net, worlds, 0, self.dev, hidden, seed=seed, sims=max(sims, n_pairs + 64) if cover else sims,
                           depth=3, k_root=k, force=pt, transform=self.p_battle, root_cover=int(cover))

    def realistic(self, r):
        """(2026-10-11, user) Grade against a REALISTIC opponent instead of a perfect one: the equilibrium of the search often
        collapses to one 'correct' reply at ~100%, and real players (our own net too) don't play like that. Their mix =
        OPP_EQ x equilibrium + (1 - OPP_EQ) x the opponent-prediction head (renormalised over the surviving replies). Every
        candidate's value is re-scored against that mix; the position's value = the best well-evaluated candidate (the
        search's survivors + the played move). Rewrites r["value"], r["q"], r["y_survivors"] (originals kept as *_eq)."""
        act_t, y = r["y_survivors"]
        ph = np.array([max(float(r["theirs"][j][0]), 0.0) for j in act_t], np.float64)
        ph = ph / ph.sum() if ph.sum() > 0 else np.full(len(act_t), 1.0 / len(act_t))
        y_r = OPP_EQ * np.asarray(y, np.float64) + (1 - OPP_EQ) * ph
        Mt = r["M_all"][:, act_t]
        q_r = {c: float(Mt[c] @ y_r) for c in range(len(r["ours"])) if np.all(np.isfinite(Mt[c]))}
        pool = [c for c in list(r["survivors"]) + ([r["forced"]] if r.get("forced") is not None else []) if c in q_r]
        r["value_eq"], r["q_eq"], r["y_eq"] = r["value"], r["q"], r["y_survivors"]
        if pool:
            r["value"] = max(q_r[c] for c in pool)
            r["q"] = q_r
            r["y_survivors"] = (act_t, y_r)
        return r

    # ------------------------------------------------------------------ "why": plain-language reasons for the top options
    STATUS_TXT = {"slp": "falls asleep", "par": "is paralyzed", "brn": "is burned", "psn": "is poisoned", "tox": "is badly poisoned",
                  "frz": "is frozen"}

    def simulate(self, eb, ours_t, theirs_t, n=32, seed=0):
        """Plays this turn n times (our joint action vs theirs) in the engine and summarises what happens to every Pokemon:
        {(side, idx): {"faint": p, "lost": mean share of max HP lost, "status": {code: p}}} + field changes."""
        st0 = eb.state()
        acc, field = {}, {"tw0": 0.0, "tw1": 0.0, "tr": 0.0}
        for k in range(n):
            b = eb.copy()
            b.reseed(seed * 7919 + k + 1)
            b.enable_log()
            try:
                b.choose(0, ours_t[0], ours_t[1])
                b.choose(1, theirs_t[0], theirs_t[1])
            except Exception:
                return None
            st = b.state()
            ev = {"flinch": set(), "blocked": set(), "miss": set()}
            for line in b.log():
                f_ = line.split("|")
                if line.startswith("|cant|") and len(f_) > 4 and f_[4] == "flinch":
                    ev["flinch"].add((int(f_[2]), int(f_[3])))
                elif line.startswith("|-activate|") and len(f_) > 4 and f_[4] == "Protect":
                    ev["blocked"].add((int(f_[2]), int(f_[3])))
                elif line.startswith("|-miss|"):
                    ev["miss"].add((int(f_[2]), int(f_[3])))
            for s in (0, 1):
                for i, (m0, m1) in enumerate(zip(st0["sides"][s]["mons"], st["sides"][s]["mons"])):
                    if m0["fainted"]:
                        continue
                    a = acc.setdefault((s, i), {"faint": 0.0, "lost": 0.0, "status": {}, "flinch": 0.0, "blocked": 0.0, "miss": 0.0})
                    for key_ in ("flinch", "blocked", "miss"):
                        a[key_] += ((s, i) in ev[key_]) / n
                    a["faint"] += (m1["fainted"] or m1["hp"] <= 0) / n
                    a["lost"] += max(0, m0["hp"] - m1["hp"]) / max(1, m0["maxhp"]) / n
                    if m1["status"] and m1["status"] != m0["status"] and not m1["fainted"]:
                        a["status"][m1["status"]] = a["status"].get(m1["status"], 0.0) + 1 / n
            for s in (0, 1):
                if st["sides"][s]["tailwind"] and not st0["sides"][s]["tailwind"]:
                    field[f"tw{s}"] += 1 / n
            if st["field"]["trick_room"] and not st0["field"]["trick_room"]:
                field["tr"] += 1 / n
        return {"mons": acc, "field": field}

    def effects_text(self, st0, sim, only=None):
        """The notable things in a simulated turn, as short phrases (KOs, big damage, status, Tailwind / Trick Room)."""
        out = []
        name = lambda s, i: ("your " if s == 0 else "their ") + "[[" + self.names.mon(st0["sides"][s]["mons"][i]["species"]) + "]]"
        items = sorted(sim["mons"].items(), key=lambda kv: (kv[0][0] == 0, -kv[1]["faint"], -kv[1]["lost"]))   # theirs first
        for (s, i), a in items:
            if only is not None and (s, i) not in only:
                continue
            if a["faint"] >= 0.2:
                often = "" if a["faint"] > 0.95 else f" ({100 * a['faint']:.0f}% of the time)"
                out.append(f"{name(s, i)} faints{often}")
            elif a["lost"] >= 0.2:
                out.append(f"{name(s, i)} loses ~{100 * a['lost']:.0f}% HP")
            if a["blocked"] >= 0.4:
                out.append(f"{name(s, i)}'s Protect blocks an attack" if s == 0 else f"{name(s, i)} protects itself from your attack")
            if a["flinch"] >= 0.25:
                out.append(f"{name(s, i)} flinches and can't move" + ("" if a["flinch"] > 0.95 else f" ({100 * a['flinch']:.0f}%)"))
            if a["miss"] >= 0.25:
                out.append(f"{name(s, i)} misses ({100 * a['miss']:.0f}%)")
            for code, p in a["status"].items():
                if p >= 0.25 and code in self.STATUS_TXT:
                    out.append(f"{name(s, i)} {self.STATUS_TXT[code]} ({100 * p:.0f}%)")
        f = sim["field"]
        if f["tw0"] >= 0.5:
            out.append("you get Tailwind")
        if f["tw1"] >= 0.5:
            out.append("they get Tailwind")
        if f["tr"] >= 0.5:
            out.append("Trick Room goes up")
        return out

    def diff_text(self, st0, a, b):
        """What happens differently between two simulated turns (a = this option, b = the other one), split into what is
        better for us with this option and what is worse."""
        good, bad = [], []
        name = lambda s, i: ("your " if s == 0 else "their ") + "[[" + self.names.mon(st0["sides"][s]["mons"][i]["species"]) + "]]"
        for key in sorted(set(a["mons"]) | set(b["mons"]), key=lambda k: (k[0] == 0, k[1])):
            x, y = a["mons"].get(key), b["mons"].get(key)
            if x is None or y is None:
                continue
            s, i = key
            sign = 1 if s == 1 else -1                 # their Pokemon fainting / losing HP is good for us
            if abs(x["faint"] - y["faint"]) >= 0.2:
                txt = f"{name(s, i)} faints {100 * x['faint']:.0f}% of the time (vs {100 * y['faint']:.0f}%)"
                (good if sign * (x["faint"] - y["faint"]) > 0 else bad).append(txt)
            elif abs(x["lost"] - y["lost"]) >= 0.2 and max(x["faint"], y["faint"]) < 0.8:
                txt = f"{name(s, i)} loses ~{100 * x['lost']:.0f}% HP (vs ~{100 * y['lost']:.0f}%)"
                (good if sign * (x["lost"] - y["lost"]) > 0 else bad).append(txt)
            if abs(x["flinch"] - y["flinch"]) >= 0.25:
                txt = f"{name(s, i)} flinches {100 * x['flinch']:.0f}% of the time (vs {100 * y['flinch']:.0f}%)"
                (good if sign * (x["flinch"] - y["flinch"]) > 0 else bad).append(txt)
            if s == 0 and abs(x["blocked"] - y["blocked"]) >= 0.4:
                txt = f"{name(s, i)}'s Protect blocks an attack" if x["blocked"] > y["blocked"] else f"{name(s, i)} isn't protected"
                (good if x["blocked"] > y["blocked"] else bad).append(txt)
        return good, bad

    def joint_struct(self, st, side, joint):
        """(2026-10-21) A joint action as data for the review page's boards: one entry per slot with the acting
        Pokemon's species, the move (or switch target), the target as a slot code (foe-a / foe-b / ally / self / spread-foes /
        spread-all / none) and the Mega flag. Slot codes are the ENGINE's (their a / b as they stand)."""
        out = []
        sd = st["sides"][side]
        foe = st["sides"][1 - side]
        if not hasattr(self, "_move_targets"):
            try:
                dj = json.load(open(paths.DEX, encoding="utf-8"))
                self._move_targets = {k: v.get("target", "normal") for k, v in dj["moves"].items()}
            except Exception:
                self._move_targets = {}
        for slot, tup in enumerate(joint):
            kind, a, t, mega = tup
            act_idx = sd["active"][slot] if slot < len(sd["active"]) else None
            user = self.names.mon(sd["mons"][act_idx]["species"]) if act_idx is not None and act_idx < len(sd["mons"]) else None
            e = {"slot": "ab"[slot] if slot < 2 else "?", "user": user, "kind": kind}
            if kind == "pass":
                e["kind"] = "pass"
            elif kind == "switch":
                e["to"] = self.names.mon(sd["mons"][a]["species"]) if a < len(sd["mons"]) else "?"
            else:
                mv = sd["mons"][act_idx]["moves"][a][0] if act_idx is not None and a < len(sd["mons"][act_idx]["moves"]) else ""
                e["move"] = self.names.move(mv) if mv else "?"
                e["mega"] = bool(mega)
                if t in (1, 2):
                    e["target"] = "foe-" + "ab"[t - 1]
                    fi = foe["active"][t - 1] if t - 1 < len(foe["active"]) else None
                    e["target_name"] = self.names.mon(foe["mons"][fi]["species"]) if fi is not None and fi < len(foe["mons"]) else None
                elif t in (-1, -2):
                    e["target"] = "self" if -t - 1 == slot else "ally"
                else:
                    tk = self._move_targets.get(mv, "normal")
                    e["target"] = {"allAdjacentFoes": "spread-foes", "allAdjacent": "spread-all", "foeSide": "spread-foes",
                                   "adjacentAlly": "ally", "adjacentAllyOrSelf": "self"}.get(tk, "self")
            out.append(e)
        return out

    def option_sims(self, eb, r, cands, n=16):
        """(2026-10-21) For every option in the table: what happens against their most likely replies (the same 'likely'
        set as the explanation; at least the top reply), as data: {option text: [{reply, acts, freq, win, mons: [...],
        field}]}, plus 'acts' = the option's own structure. Used by the review page to draw a board per option."""
        ours, theirs, Mall = r["ours"], r["theirs"], r["M_all"]
        act_t, y = r["y_survivors"]
        st0 = eb.state()
        replies = [(act_t[j], float(y[j])) for j in np.argsort(-y)]
        likely = [(t, p) for t, p in replies if p >= 0.12][:2] or replies[:1]
        out = {}
        for c in cands:
            key = self.describe_joint(st0, 0, ours[c][1:])
            rows = []
            for t, p in likely:
                s_ = self.simulate(eb, ours[c][1:], theirs[t][1:], n=n, seed=31 * c + t)
                if s_ is None:
                    continue
                v = Mall[c, t]
                mons = []
                for (sd_, i), a in s_["mons"].items():
                    m0 = st0["sides"][sd_]["mons"][i]
                    mons.append({"side": sd_, "species": self.names.mon(m0["species"]), "lost": round(float(a["lost"]), 3),
                                 "faint": round(float(a["faint"]), 3), "blocked": round(float(a["blocked"]), 3),
                                 "flinch": round(float(a["flinch"]), 3), "miss": round(float(a["miss"]), 3),
                                 "status": {k: round(float(v_), 3) for k, v_ in a["status"].items()}})
                rows.append({"reply": self.describe_joint(st0, 1, theirs[t][1:]), "acts": self.joint_struct(st0, 1, theirs[t][1:]),
                             "freq": p, "win": None if np.isnan(v) else float(v), "mons": mons,
                             "field": {k: round(float(v_), 3) for k, v_ in s_["field"].items()}})
            out[key] = {"acts": self.joint_struct(st0, 0, ours[c][1:]), "replies": rows}
        return out

    def explain(self, eb, r, table_order, pos, top=3):
        """Reasons for the top options (and the played one): what each does against their likely replies, and where the
        difference to the next-best option (or to the best one) comes from. Built from the engine replaying the turn."""
        ours, theirs, Mall = r["ours"], r["theirs"], r["M_all"]
        act_t, y = r["y_survivors"]
        st0 = eb.state()
        replies = [(act_t[j], float(y[j])) for j in np.argsort(-y)]
        likely = [(t, p) for t, p in replies if p >= 0.12][:2] or replies[:1]
        pick = table_order[:top] + ([pos] if pos is not None and pos not in table_order[:top] else [])
        cache = {}

        def sim(c, t):
            if (c, t) not in cache:
                cache[(c, t)] = self.simulate(eb, ours[c][1:], theirs[t][1:], seed=31 * c + t)
            return cache[(c, t)]

        def mval(c, t):
            v = Mall[c, t]
            return None if np.isnan(v) else float(v)

        out = []
        best = table_order[0]
        for rank, c in enumerate(pick):
            lines = []
            for t, p in likely:
                s_ = sim(c, t)
                if s_ is None:
                    continue
                eff = self.effects_text(st0, s_)
                v = mval(c, t)
                lines.append(f"If they play {self.describe_joint(st0, 1, theirs[t][1:], mark=True)} ({100 * p:.0f}% likely): "
                             + ("; ".join(eff) if eff else "nothing big happens to either side")
                             + (f". Chance to win after: {100 * v:.0f}%" if v is not None else "") + ".")
            # the comparison: the best option vs the next one, or this option vs the best
            other = (table_order[1] if len(table_order) > 1 else None) if c == best else best
            if other is not None and abs(float(r["q"][c]) - float(r["q"][other])) < 0.01:
                lines.append(f"About as good as {'the next-best option' if c == best else 'the best option'} "
                             f"({self.describe_joint(st0, 0, ours[other][1:], mark=True)}): within 1% chance to win.")
            elif other is not None:
                gaps = []
                for t, p in replies:
                    a_, b_ = mval(c, t), mval(other, t)
                    if a_ is not None and b_ is not None and p >= 0.03:
                        gaps.append((p * (a_ - b_), t, p, a_, b_))
                if gaps:
                    g = max(gaps, key=lambda z: z[0]) if c == best else min(gaps, key=lambda z: z[0])
                    _, t, p, a_, b_ = g
                    if abs(a_ - b_) >= 0.03:
                        sa, sb = sim(c, t), sim(other, t)
                        good, bad = self.diff_text(st0, sa, sb) if sa is not None and sb is not None else ([], [])
                        who = "the next-best option" if c == best else "the best option"
                        lines.append(f"Compared with {who} ({self.describe_joint(st0, 0, ours[other][1:], mark=True)}): the biggest "
                                     f"difference is when they play {self.describe_joint(st0, 1, theirs[t][1:], mark=True)} ({100 * p:.0f}% "
                                     f"likely): {100 * a_:.0f}% to win with this vs {100 * b_:.0f}%."
                                     + (" Better here: " + "; ".join(good) + "." if good else "")
                                     + (" Worse here: " + "; ".join(bad) + "." if bad else "")
                                     + ("" if good or bad else " The difference is in the position afterwards (HP, speed, "
                                        "who is on the field), not in KOs or big damage this turn."))
            out.append({"action": self.describe_joint(st0, 0, ours[c][1:]), "win": float(r["q"][c]), "rank": rank + 1,
                        "played": c == pos, "best": c == best, "lines": lines})
        return out

    # ------------------------------------------------------------------ forced switches (faint replacements, U-turn...)
    SELF_SWITCH = {"uturn": "U-turn", "voltswitch": "Volt Switch", "flipturn": "Flip Turn", "partingshot": "Parting Shot",
                   "teleport": "Teleport", "batonpass": "Baton Pass", "shedtail": "Shed Tail", "chillyreception": "Chilly Reception"}

    def switch_reason(self, lines, us, slots_fainted):
        """Why this replacement is being chosen, from the log since the last decision."""
        if all(slots_fainted):
            return "replacement after a faint"
        for ln in reversed(lines):
            p = ln.split("|")
            if len(p) < 3 or not p[2].startswith(us):
                continue
            if p[1] == "move" and len(p) > 3 and to_id(p[3]) in self.SELF_SWITCH:
                return f"switch out from {self.SELF_SWITCH[to_id(p[3])]}"
            if p[1] == "-enditem" and len(p) > 3 and to_id(p[3]) in ("ejectbutton", "ejectpack"):
                return f"switch out from {p[3]}"
            if p[1] == "-activate" and "Emergency Exit" in ln:
                return "switch out from Emergency Exit"
        return "switch out" if not any(slots_fainted) else "replacement after a faint"

    def analyze_switch(self, d, eb, played, hidden, seed, us):
        """Every Pokemon you could send in (and the opponent's simultaneous replacements), each scored by the win chance of
        the resulting position with best play from there (the same search as move turns). A mid-turn switch-out (U-turn,
        Eject Button...) is played as if the turn ended after it: the attacks still to come this turn are unknown."""
        fs = d["request"].get("forceSwitch") or [False, False]
        st = eb.state()
        e = eb.copy()
        restore, fainted_slots = [], []
        mons = st["sides"][0]["mons"]
        spare = [j for j, m in enumerate(mons) if (m["fainted"] or m["hp"] <= 0) and j not in st["sides"][0]["active"]]
        # (2026-10-19) Revival Blessing's request (Showdown marks the user 'reviving'): pick a FAINTED Pokemon to bring back
        revive = any(p.get("reviving") for p in (d["request"].get("side") or {}).get("pokemon", []))
        if revive:
            e.refresh()
            if not hasattr(e, "set_reviving") or not any(k < len(fs) and fs[k] and e.set_reviving(0, k) for k in (0, 1)):
                return {"note": "Revival Blessing pick (couldn't set up the choice)"}
        for k in (0, 1):
            if k < len(fs) and fs[k] and not revive:
                idx = st["sides"][0]["active"][k]
                if idx is None:
                    # the rebuilt position leaves a fainted Pokemon's slot empty: put a fainted one back in it, so the
                    # engine asks for the replacement as in the real battle
                    if not spare:
                        return {"note": "replacement (couldn't set up the choice)"}
                    idx = spare.pop(0)
                    e.set_active(0, k, idx)
                mon = mons[idx]
                fainted_slots.append(bool(mon["fainted"] or mon["hp"] <= 0))
        midturn = False
        if not revive:
            # (2026-10-19 forced behaviour check) the same position as the agent's replacement search
            # (bridge_util.forced_setup): fainted Pokemon put back on both sides (both replace at the same time); a mid-turn
            # switch-out is the engine's own (Regenerator, Baton Pass, Shed Tail used to be lost), then the end of the turn
            import bridge_util
            setup = bridge_util.forced_setup(eb, fs, bridge_util.midturn_kinds_log(d.get("log_before", []), us))
            if setup is None:
                return {"note": f"{self.switch_reason(d.get('log_before', []), us, fainted_slots)} (couldn't set up the choice)"}
            e, midturn = setup
        reason = "Revival Blessing pick" if revive else self.switch_reason(d.get("log_before", []), us, fainted_slots)
        if not e.needs_choice(0):
            return {"note": f"{reason} (couldn't set up the choice)"}
        maps = e.legal_index_map(0)
        f, i = vgcsim.encode_battle(e, 0, hidden=hidden)
        with torch.no_grad(), amp():
            l0 = self.net(torch.tensor(np.array([f], np.float32), device=self.dev),
                          torch.tensor(np.array([i], np.int32), device=self.dev))[0]
        ours = nnsearch.top_joint(l0[0].float().cpu().numpy(), maps, 64)
        pos = None
        if played is not None:                                      # (None: only the position's value)
            pt = tuple(maps[s].get(played[s], PASS) if played[s] is not None else PASS for s in (0, 1))
            pos = next((c for c, o in enumerate(ours) if (o[1], o[2]) == pt), None)
            if pos is None:
                ours.append((0.0, pt[0], pt[1]))
                pos = len(ours) - 1
        theirs = [(1.0, PASS, PASS)]
        if e.needs_choice(1):                                       # they replace at the same time
            maps1 = e.legal_index_map(1)
            theirs = nnsearch.top_joint(np.zeros((2, 38), np.float32), maps1, 64)
        M = np.zeros((len(ours), len(theirs)))
        for a, (_, o0, o1) in enumerate(ours):
            for b, (_, t0, t1) in enumerate(theirs):
                e2 = e.copy()
                e2.choose(0, tuple(o0), tuple(o1))
                if e.needs_choice(1):
                    e2.choose(1, tuple(t0), tuple(t1))
                for idx, hp in restore:
                    e2.set_mon(0, idx, hp=hp, fainted=False)
                e2.refresh()
                s2 = e2.state()
                if s2["phase"] == "ended":
                    M[a, b] = 1.0 if s2["winner"] == 0 else 0.0 if s2["winner"] == 1 else 0.5
                elif s2["phase"] == "turn" and e2.needs_choice(0) and e2.needs_choice(1):
                    self.cell_mode = True
                    try:
                        M[a, b] = self.analyze_turn(e2, None, hidden, seed + 31 * a + b)["win"]
                    finally:
                        self.cell_mode = False
                else:                                               # e.g. another replacement pending: the value net
                    ff, ii = vgcsim.encode_battle(e2, 0, hidden=hidden)
                    M[a, b] = float(self.p_battle(self.values(np.array([ff], np.float32), np.array([ii], np.int32)))[0])
        x, y = nnsearch.solve(M)
        rows, worst = M @ y, M.min(1)
        V = float(x @ M @ y)
        if pos is None:
            return {"win": V}
        loss = max(0.0, 100 * (V - rows[pos]))
        sent = lambda o: "; ".join(f"send in {self.names.mon(st['sides'][0]['mons'][t[1]]['species'])}" for t in o[1:]
                                   if t[0] == "switch" and t[1] < len(st["sides"][0]["mons"])) or "(nothing)"
        cands = sorted(range(len(ours)), key=lambda c: (-round(float(x[c]), 3), -rows[c]))
        table = [{"action": sent(ours[c]), "win": float(rows[c]), "worst": float(worst[c]), "freq": float(x[c]),
                  "policy": float(ours[c][0]), "played": c == pos} for c in cands]
        forced = len(ours) == 1                                     # only one Pokemon left to send in: not a decision
        out = {"win": V, "played_win": float(rows[pos]), "loss": loss, "label": "forced" if forced else label_of(loss, float(x[pos])),
               "accuracy": None if forced else accuracy_of(loss), "played": sent(ours[pos]), "table": table, "reason": reason}
        if e.needs_choice(1):
            out["opponent"] = [{"action": "they send in " + ", ".join(self.names.mon(st["sides"][1]["mons"][t[1]]["species"])
                                for t in theirs[j][1:] if t[0] == "switch" and t[1] < len(st["sides"][1]["mons"])), "freq": float(y[j])}
                               for j in np.argsort(-y) if y[j] >= 0.03]
        if midturn:
            out["note"] = "Mid-turn switch: the attacks still to come this turn are unknown, so it is scored as if the turn went straight to its end."
        return out

    # ------------------------------------------------------------------ team preview
    def their_species(self, events, foe):
        return [e.split("|")[3].split(",")[0] for e in events if isinstance(e, str) and e.startswith(f"|poke|{foe}|")]

    def our_team_text(self, battle, req):
        """Export text of our six from the team preview request (exact stats -> stat points + nature)."""
        blocks = []
        for p in req["side"]["pokemon"]:
            mon = battle.team.get(p["ident"])
            pts, nature = bridge.infer_points(mon)
            species = p["details"].split(",")[0]
            evs = " / ".join(f"{pts[s]} {lab}" for s, lab in (("hp", "HP"), ("atk", "Atk"), ("def", "Def"), ("spa", "SpA"),
                                                               ("spd", "SpD"), ("spe", "Spe")) if pts.get(s))
            lines = [f"{species} @ {p.get('item') or ''}".rstrip(" @"), f"Ability: {p.get('baseAbility') or p.get('ability')}",
                     "Level: 50"] + ([f"EVs: {evs}"] if evs else []) + [f"{nature} Nature"] + [f"- {m}" for m in p["moves"]]
            blocks.append("\n".join(lines))
        return "\n\n".join(blocks) + "\n"

    def their_team_text(self, species, rng):
        if self.sets is None:
            self.sets = load_sets()
        blocks = []
        for sp in species:
            sid = to_id(sp)
            pool = self.sets.get(sid) or next((v for k_, v in self.sets.items() if k_.startswith(sid.rstrip("*"))), None)
            blocks.append(rng.choice(pool) if pool else self.fallback_set(sp, rng))
        return "\n\n".join(blocks) + "\n"

    def fallback_set(self, sp, rng):
        """(2026-10-11) A legal species with no set in the team pool: a cosmetic / size form borrows its base species' set
        (header renamed to the form), else a generic set: first ability, Protect + ~90-power attacks of its own types
        (bridge.filler_moves) in its better attacking category, points in that stat and Speed."""
        import bridge
        dex = getattr(self, "_species_json", None)
        if dex is None:
            dex = self._species_json = json.load(open(paths.DEX,
                                                      encoding="utf-8"))["species"]
        d = dex.get(to_id(sp))
        if d is None:
            raise KeyError(f"unknown species {sp}")
        base = to_id(d.get("baseSpecies") or "")
        if base and base != to_id(sp) and self.sets.get(base):
            blk = rng.choice(self.sets[base]).split("\n")
            item = blk[0].split(" @ ", 1)[1] if " @ " in blk[0] else ""
            return "\n".join([d["name"] + (f" @ {item}" if item else "")] + blk[1:])
        bs = d["baseStats"]
        cat = "PHYSICAL" if bs["atk"] >= bs["spa"] else "SPECIAL"
        fill = bridge.filler_moves()
        moves = [fill[(t.upper(), cat)] for t in d["types"] if (t.upper(), cat) in fill]
        if len(moves) < 2 and ("NORMAL", cat) in fill:
            moves.append(fill[("NORMAL", cat)])
        names = [self.names.move(m) for m in dict.fromkeys(moves)][:3] + ["Protect"]
        stat = "Atk" if cat == "PHYSICAL" else "SpA"
        return (f"{d['name']}\nAbility: {d['abilities'].get('0', '')}\nLevel: 50\nEVs: 2 HP / 32 {stat} / 32 Spe\n"
                f"{'Jolly' if cat == 'PHYSICAL' else 'Timid'} Nature\n" + "\n".join(f"- {n}" for n in names))

    def preview_probs(self, b, side, hidden):
        pf, pi, pm = vgcsim.encode_preview(b, side, hidden=hidden)
        with torch.no_grad(), amp():
            lg, v = self.net.preview(torch.tensor([pf], device=self.dev), torch.tensor([pi], device=self.dev))
        lg = lg[0].float().cpu().numpy()
        lg[np.frombuffer(bytes(pm), np.uint8) == 0] = -1e9   # (2026-10-22) pm is bytes
        p = np.exp(lg - lg.max())
        return p / p.sum(), float(v[0])

    def order_of(self, opt):
        four, leads = self.options[opt]
        return list(leads) + [m for m in four if m not in leads]

    def option_of(self, order):
        four, leads = sorted(order[:4]), sorted(order[:2])
        return next((k for k, (f, l) in enumerate(self.options) if sorted(f) == four and sorted(l) == leads), None)

    def analyze_preview(self, d, events, player):
        battle, req = d["battle"], d["request"]
        hidden = not d["open_sheets"]
        foe = "p2" if player == "p1" else "p1"
        their = self.their_species(events, foe)
        ours_txt = self.our_team_text(battle, req)
        picked = [int(c) - 1 for c in d["choice"].split()[-1]] if d["choice"].split()[-1].isdigit() else []
        mine = self.option_of(picked) if len(picked) >= 4 else None
        rng = random.Random(7)
        worlds = []
        for w in range(self.pw):
            worlds.append(vgcsim.Battle(self.dex, ours_txt, self.their_team_text(their, rng), seed=11 + w))
            if hasattr(worlds[-1], "set_keep_foe_stats"):
                worlds[-1].set_keep_foe_stats(True)        # (2026-10-21) their sets' stats are our belief: keep them
        p_ours, pv_ours = self.preview_probs(worlds[0], 0, hidden)
        p_theirs, _ = self.preview_probs(worlds[0], 1, hidden)
        rows = list(np.argsort(-p_ours)[: self.pk])
        if mine is not None:
            f0, l0 = self.options[mine]
            for k_, (f, l) in enumerate(self.options):         # lead changes and one-member swaps of the pick
                if (sorted(f) == sorted(f0)) or (len(set(f) & set(f0)) == 3 and set(l) <= set(f0) | set(f) and
                                                  len(set(l) & set(l0)) >= 1 and set(l) - set(l0) <= set(f) - set(f0)):
                    rows.append(k_)
            rows.append(mine)
        rows = list(dict.fromkeys(int(r) for r in rows))
        cols = [int(c) for c in np.argsort(-p_theirs)[: self.pk]]
        F, I = [], []
        for b in worlds:
            for r in rows:
                for c in cols:
                    b2 = b.copy()
                    b2.choose_team(0, self.order_of(r))
                    b2.choose_team(1, self.order_of(c))
                    f, i = vgcsim.encode_battle(b2, 0, hidden=hidden)
                    F.append(f)
                    I.append(i)
        raw = self.values(np.asarray(F, np.float32), np.asarray(I, np.int32))
        Mw = self.p_battle(raw).reshape(self.pw, len(rows), len(cols))      # per sampled opponent set, calibrated
        M = Mw.mean(0)
        Mraw = raw.reshape(self.pw, len(rows), len(cols)).mean(0)
        top = [r for r in range(len(rows)) if rows[r] in set(int(x) for x in np.argsort(-p_ours)[: self.pk])]
        # the reference pick: head log-prob + PREVIEW_BETA x value advantage on the RAW value scale
        xr, yr = nnsearch.solve(Mraw[top])
        qr = Mraw[top] @ yr
        Vr = float(xr @ qr)
        best = top[int(np.argmax([math.log(max(float(p_ours[rows[r]]), 1e-12)) + PREVIEW_BETA * (qr[j] - Vr)
                                  for j, r in enumerate(top)]))]
        x, y = nnsearch.solve(M[top])
        win_rows = M @ y

        def gap_noise(a, b):
            """2 standard errors of win_rows[a] - win_rows[b] over the sampled opponent sets (win-chance points)"""
            d = (Mw[:, a, :] - Mw[:, b, :]) @ y
            return 200 * float(np.std(d, ddof=1) / math.sqrt(len(d))) if len(d) > 1 else 0.0
        species = [p["details"].split(",")[0] for p in req["side"]["pokemon"]]
        name = lambda r: {"bring": [self.names.mon(species[m]) for m in self.options[rows[r]][0]],
                          "lead": [self.names.mon(species[m]) for m in self.options[rows[r]][1]]}
        out = {"win_best": float(win_rows[best]), "best": name(best), "their_likely": [], "swaps": [],
               "matchup_value": float(apply_cal(self.cal["preview"], pv_ours))}
        # (user, 2026-10-11) always 3 other good teams for the opponent, never the one they actually picked: their side of
        # the equilibrium first, then the teams that do best against our mix (lowest win chance for us)
        tp = self.their_pick(events, foe)
        vs_us = x @ M[top]
        for j in sorted(range(len(cols)), key=lambda j: (-round(float(y[j]), 3), float(vs_us[j]))):
            fo, lo = self.options[cols[j]]
            bring = [self.names.mon(their[m]) for m in fo if m < len(their)]
            lead = [self.names.mon(their[m]) for m in lo if m < len(their)]
            if tp and tp["bring"] and set(tp["lead"]) == set(lead) and set(tp["bring"]) <= set(bring):
                continue                                   # (what they brought; unseen back Pokemon fit any team)
            out["their_likely"].append({"freq": float(y[j]), "bring": bring, "lead": lead})
            if len(out["their_likely"]) == 3:
                break
        if mine is not None:
            mi = rows.index(mine)
            raw_loss = max(0.0, 100 * (win_rows[best] - win_rows[mi]))
            # only the part beyond the noise of the sampled opponent sets counts; the bot's own pick costs nothing
            loss = 0.0 if mi == best else max(0.0, raw_loss - gap_noise(best, mi))
            out.update({"win_picked": float(win_rows[mi]), "picked": name(mi), "loss": loss, "loss_raw": raw_loss})
            f0, l0 = self.options[mine]
            # (2026-10-11, user: trust the bot) the only 'better' team is the bot's own pick, and only when yours cost points
            # beyond the noise; otherwise: the 3 other good picks in the bot's own order. The raw swap gains (up to +23% on
            # a pick within noise of the bot's) came from the estimates the bot's rule deliberately discounts.
            out["swaps"] = []
            out["match"] = "exact" if mi == best else ("close" if loss <= 0.5 else "worse")
            if loss > 0.5:
                out["better_team"] = [self.names.mon(species[m]) for m in self.order_of(rows[best])]
            qr_all = Mraw @ yr
            score = lambda r: math.log(max(float(p_ours[rows[r]]), 1e-12)) + PREVIEW_BETA * (qr_all[r] - Vr)
            if loss > 0.5:
                # (user, 2026-10-11) up to 3 better teams: the bot's pick, then the next ones in the bot's order that ALSO
                # beat yours beyond the noise
                bt = [best]
                for r in sorted(range(len(rows)), key=lambda r: -score(r)):
                    if len(bt) == 3:
                        break
                    if r not in bt and rows[r] != mine and 100 * (win_rows[r] - win_rows[mi]) - gap_noise(r, mi) > 0.5:
                        bt.append(r)
                out["better_teams"] = [[self.names.mon(species[m]) for m in self.order_of(rows[r])] for r in bt]
            else:
                nxt = []
                for r in sorted(range(len(rows)), key=lambda r: -score(r)):
                    if rows[r] == mine:
                        continue
                    f, l = self.options[rows[r]]
                    if sorted(f) == sorted(f0):
                        txt = f"same four, lead {' + '.join(self.names.mon(species[m]) for m in l)}"
                    elif len(set(f) & set(f0)) == 3:
                        gone, new = (set(f0) - set(f)).pop(), (set(f) - set(f0)).pop()
                        txt = f"bring {self.names.mon(species[new])} instead of {self.names.mon(species[gone])}" +                               (f" (lead {' + '.join(self.names.mon(species[m]) for m in l)})" if sorted(l) != sorted(l0) else "")
                    else:
                        txt = (f"bring {', '.join(self.names.mon(species[m]) for m in f)} "
                               f"(lead {' + '.join(self.names.mon(species[m]) for m in l)})")
                    nxt.append({"change": txt, "win": float(win_rows[r]), "diff": 100 * float(win_rows[r] - win_rows[mi]),
                                "team": [self.names.mon(species[m]) for m in self.order_of(rows[r])]})   # leads first
                    if len(nxt) == 3:
                        break
                out["next_best"] = nxt
            out["notes"] = self.matchup_notes(worlds[0], mine, cols[int(np.argmax(y))], species, their)
            out["threats"] = list(self.last_threats)
        out["their_pick"] = self.their_pick(events, foe)
        return out

    def their_pick(self, events, foe):
        """What the opponent actually brought, from the game log: their two leads and every Pokemon they sent out (a
        Pokemon brought but never sent out can't be seen)."""
        lead, seen = [], []
        for e in events:
            if not isinstance(e, str):
                continue
            for line in e.split("\n"):
                parts = line.split("|")
                if len(parts) > 3 and parts[1] in ("switch", "drag", "replace") and parts[2].startswith(foe):
                    sp = self.names.mon(parts[3].split(",")[0])
                    base = re.sub(r"-Mega(-[XYZ])?$", "", sp)
                    if base not in seen:
                        seen.append(base)
                if line == "|turn|1" and not lead:
                    lead = list(seen[:2])                      # sent out before turn 1 = the leads
        return {"lead": lead or seen[:2], "bring": seen[:4], "seen": len(seen[:4])}

    def matchup_notes(self, b, mine, theirs_opt, species, their):
        """Damage-based matchup summary for the picked four vs their most likely four (world 0's sets)."""
        b2 = b.copy()
        b2.choose_team(0, self.order_of(mine))
        b2.choose_team(1, self.order_of(theirs_opt))
        st = b2.state()
        notes = []

        def best_hit(sa, ia, sd, id_):
            mon, tgt = st["sides"][sa]["mons"][ia], st["sides"][sd]["mons"][id_]
            best = (0.0, None, 0.0)
            for slot, (mv, _) in enumerate(mon["moves"]):
                r = b2.damage_rolls(sa, ia, slot, sd, id_)
                if r:
                    frac = sum(r) / len(r) / max(1, tgt["maxhp"])
                    ko = sum(1 for x in r if x >= tgt["hp"]) / len(r)
                    if frac > best[0]:
                        best = (frac, mv, ko)
            return best

        ns, nt = len(st["sides"][0]["mons"]), len(st["sides"][1]["mons"])
        for j in range(nt):
            threats = []
            for i in range(ns):
                frac, mv, ko = best_hit(1, j, 0, i)
                if ko >= 0.5:
                    threats.append(f"{self.names.mon(st['sides'][0]['mons'][i]['species'])} ({self.names.move(mv)}, {100 * ko:.0f}% KO)")
            if len(threats) >= 2:
                notes.append(f"Their {self.names.mon(st['sides'][1]['mons'][j]['species'])} can KO {len(threats)} of your four in one hit: "
                             + ", ".join(threats))
        for j in range(nt):
            answers = [i for i in range(ns) if best_hit(0, i, 1, j)[0] >= 0.5]
            if not answers:
                notes.append(f"None of your four hits their {self.names.mon(st['sides'][1]['mons'][j]['species'])} for half its HP")
        # always: their two most dangerous Pokemon against your four (mean of their best hit on each of yours)
        danger = []
        for j in range(nt):
            hits = [best_hit(1, j, 0, i) for i in range(ns)]
            mean = sum(h[0] for h in hits) / max(1, ns)
            worst = max(range(ns), key=lambda i: hits[i][0])
            danger.append((mean, j, worst, hits[worst]))
        # (2026-10-11, user) the page lists just these two, under 'Threats'
        self.last_threats = [
            f"{self.names.mon(st['sides'][1]['mons'][j]['species'])}: {self.names.move(mv)} does {100 * frac:.0f}% to your "
            f"{self.names.mon(st['sides'][0]['mons'][i]['species'])}, and {100 * mean:.0f}% per hit on average to your entire team"
            for mean, j, i, (frac, mv, ko) in sorted(danger, reverse=True)[:2]]
        return notes + [f"Biggest threat: {t}" for t in self.last_threats]

    def outcome_text(self, lines, us):
        """The turn's public log as short readable lines."""
        out = []
        side = lambda who: "your" if who.startswith(us) else "their"
        mon = lambda who: who.split(": ", 1)[-1]
        for ln in lines:
            p = ln.split("|")
            if len(p) < 3:
                continue
            k = p[1]
            if k == "move" and len(p) > 3:
                tgt = f" on {side(p[4])} {mon(p[4])}" if (len(p) > 4 and p[4] and ":" in p[4] and "[spread]" not in ln
                                                           and p[4] != p[2]) else ""
                out.append(f"{side(p[2]).capitalize()} {mon(p[2])} used {p[3]}{tgt}")
            elif k == "switch" and len(p) > 3:
                out.append(f"{side(p[2]).capitalize()} {p[3].split(',')[0]} came in")
            elif k == "drag" and len(p) > 3:       # Roar, Whirlwind, Dragon Tail, Red Card...: random, not a choice
                out.append(f"{side(p[2]).capitalize()} {p[3].split(',')[0]} was dragged in (forced, random: not a decision)")
            elif k == "faint":
                out.append(f"{side(p[2]).capitalize()} {mon(p[2])} fainted")
            elif k == "-damage" and len(p) > 3 and "[from]" not in ln:
                hp = p[3].split(" ")[0]
                if hp == "0":
                    left = "0%"
                elif "/" in hp:
                    a_, b_ = hp.split("/")
                    left = f"{100 * int(a_) / max(1, int(b_)):.0f}%" if a_.isdigit() and b_.isdigit() else hp
                else:
                    left = hp
                out.append(f"   {side(p[2])} {mon(p[2])}: {left} HP left")
            elif k == "-mega":
                out.append(f"{side(p[2]).capitalize()} {mon(p[2])} Mega Evolved")
            elif k == "-crit":
                out.append(f"   critical hit on {side(p[2])} {mon(p[2])}!")
            elif k == "-miss":
                out.append(f"   {side(p[2])} {mon(p[2])} missed")
            elif k == "-supereffective":
                out.append("   super effective")
            elif k == "-immune":
                out.append(f"   {side(p[2])} {mon(p[2])} was immune")
            elif k == "cant" and len(p) > 3:
                out.append(f"   {side(p[2])} {mon(p[2])} couldn't move ({p[3]})")
        return out

    # ------------------------------------------------------------------ luck events
    def luck_events(self, lines, us):
        ev = []
        for ln in lines:
            p = ln.split("|")
            if len(p) < 3:
                continue
            kind, who = p[1], p[2]
            mine = who.startswith(us)
            mon = who.split(": ", 1)[-1] if ": " in who else who
            if kind == "-crit":
                ev.append(("bad" if mine else "good", f"critical hit on {'your' if mine else 'their'} {mon}"))
            elif kind == "-miss":
                ev.append(("bad" if mine else "good", f"{'your' if mine else 'their'} {mon} missed"))
            elif kind == "cant" and len(p) > 3 and p[3] in ("flinch", "par", "slp", "frz"):
                why = {"flinch": "flinched", "par": "was fully paralyzed", "slp": "was asleep", "frz": "was frozen"}[p[3]]
                ev.append(("bad" if mine else "good", f"{'your' if mine else 'their'} {mon} {why}"))
            elif kind == "-status" and len(p) > 3 and "[from]" not in ln:
                pass
        return [{"good": g == "good", "text": t} for g, t in ev]

    # ------------------------------------------------------------------ the whole game
    def review(self, game, progress=None):
        if not any(isinstance(e, str) and (e.startswith("|win|") or e.rstrip() == "|tie" or e.startswith("|tie|"))
                   for e in game.get("events", [])):
            raise ValueError("only finished games can be reviewed (no |win| or |tie| in the game)")
        player = game.get("player", "p1")
        out = {"player": player, "name": game["name"], "preview": None, "turns": [], "result": None,
               "source": game.get("source", "log"), "assumed_brought": game.get("assumed_brought", [])}
        # (2026-10-11, user) a title to tell games apart: the game's own (test games: "Game 403"), else "vs <opponent>"
        foe = next((e.split("|")[3] for e in game["events"] if isinstance(e, str) and e.startswith("|player|")
                    and e.split("|")[2] != player and len(e.split("|")) > 3 and e.split("|")[3]), "")
        out["opponent_name"] = foe
        out["title"] = game.get("title") or (f"vs {foe}" if foe else game["name"])
        agent.HIDDEN = 0 if any(isinstance(e, str) and e.startswith("|showteam|") for e in game["events"]) else 1
        prev = None             # the last RATED turn (its swing runs until the next rated decision)
        last = None             # the last decision of any kind (what happened right after it)
        n_turns = sum(1 for e in game["events"] if isinstance(e, str) and e.startswith("|turn|"))
        done = 0
        # analyse each decision WHILE the replay stands there: the battle object keeps changing as the replay goes on
        for d in replay.decisions(game):
            if last is not None:
                seg = d.get("log_before", [])
                last["luck_events"] = self.luck_events(seg, player)
                last["outcome_text"] = self.outcome_text(seg, player)
            if d["kind"] == "preview":
                try:
                    out["preview"] = self.analyze_preview(d, game["events"], player)
                except Exception as e:                                  # e.g. a species with no known set
                    out["preview"] = {"error": f"team preview not analysed: {e}"}
                continue
            if d["kind"] == "end":
                out["result"] = "win" if d["winner"] == game["name"] else ("loss" if d["winner"] else "tie")
                end_v = 1.0 if out["result"] == "win" else 0.0 if out["result"] == "loss" else 0.5
                if prev is not None and prev.get("played_win") is not None:
                    prev["luck"] = 100 * (end_v - prev["played_win"])
                break
            bot, battle = d["bot"], d["battle"]
            if "unknown" in [t.strip() for t in d["choice"].split(",")]:
                # a replay that can't show this choice (flinched / asleep / fainted before moving): listed, not rated
                entry = {"turn": d.get("turn"), "kind": d["kind"], "choice": d["choice"],
                         "note": "couldn't tell from the replay what you chose this turn (a Pokemon flinched, slept or fainted "
                                 "before moving)"}
                out["turns"].append(entry)
                last = entry
                continue
            bot._memo = {}
            try:
                _, _, _, options = bot.encode(battle)
                eb = bot.last_engine[0]
                played = self.played_indices(d["choice"], d["request"], options, battle)
                entry = {"turn": d.get("turn"), "kind": d["kind"], "choice": d["choice"]}
                if d["kind"] == "turn":
                    entry.update(self.analyze_turn(eb, played, bool(agent.HIDDEN), seed=1000 + 7 * (d.get("turn") or 0)))
                    done += 1
                    if progress:
                        progress(done, n_turns)
                else:
                    entry.update(self.analyze_switch(d, eb, played, bool(agent.HIDDEN), 5000 + 7 * (d.get("turn") or 0), player))
            except Exception as e:
                entry = {"turn": d.get("turn"), "kind": d["kind"], "choice": d["choice"], "error": f"{type(e).__name__}: {e}"}
            finally:
                bot._memo = None
            # luck of the previous rated turn: win chance now vs what its move was expected to give
            if prev is not None and prev.get("played_win") is not None and entry.get("win") is not None:
                prev["luck"] = 100 * (entry["win"] - prev["played_win"])
            out["turns"].append(entry)
            last = entry
            if entry.get("win") is not None:
                prev = entry
        rated = [t for t in out["turns"] if t.get("accuracy") is not None]
        out["accuracy"] = float(np.mean([t["accuracy"] for t in rated])) if rated else None
        out["counts"] = {lab: sum(1 for t in rated if t["label"] == lab) for _, lab in LABELS}
        out["graph"] = [{"turn": t["turn"], "win": t["win"]} for t in out["turns"] if t.get("win") is not None]
        return out
