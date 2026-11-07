"""(2026-10-27) Simultaneous-move tree search over several turns (SM-MCTS, open loop).

The halving search (sh_search.py) solves ONE turn: every leaf is the position after the turn, valued by the value net.
This search grows a tree over several turns and spends its simulations where the game is decided.

Tree: a node = a decision point reached by a path of joint actions (ours, theirs) and what was OBSERVED after each turn
(phase, who must choose, the actives' species and HP in eighths, faints, weather / terrain / Trick Room; see `obs_key`).
Open loop: every simulation starts from a fresh copy of the root position (one of the worlds) with fresh random numbers,
so damage rolls, crits, accuracy, speed ties and secondary effects are SAMPLED, not enumerated; positions that look the
same to the players share a node.

At a node both sides have candidate lists from the network (ours: the policy's top joint actions; theirs: the opponent
head / joint table from our view, as sh_search uses). Each node keeps the mean value S/C of every (our, their) pair.
Selection: solve the node's matrix game (regret matching, Rust) on the current means (unvisited pairs = the node's
value-net estimate), then sample each side's action from (1 - eps) x equilibrium + eps x network prior, eps falling
with the node's visits (eps = min(EPS_MAX, EPS_C / sqrt(1 + n))). A new node is valued by the value net (batched: B
simulations per network call); a finished game counts +1 / -1 / 0. Values are always from OUR side's view.

Result: same dict layout as sh_search.halving_search (ours, x, value, q, survivors...), so sh_search.choose() works.

    r = search(net, battle, side, device, hidden=False, seed=1, sims=800, batch=64, depth=3, k_root=(12, 12), k_deep=None)

(2026-10-27) Pass `battle.search_worlds(side, W, seed)` (our side = side 0 in
every copy, opponent stats guessed / sampled like the halving search's worlds) with side 0; r["logits"] = the root policy
logits.
"""

import math
import os
import time
import numpy as np
import torch
import vgcsim
from nnsearch import bayes_solve, solve, top_joint
from sh_search import their_joint, opp_table, K_THEIRS, ensure_world_sets
from netlib import amp, has_opp_joint

EPS_C = float(os.environ.get("MCTS_EPS_C", "1.0"))
EPS_MAX = float(os.environ.get("MCTS_EPS_MAX", "0.5"))
# (2026-10-28, leaks) worlds built when search() gets one TRUE position (Battle.search_worlds)
WORLDS = int(os.environ.get("MCTS_WORLDS", "8"))
# (2026-10-28, root fix after the arena losses) search(..., prior=r) with r = a halving search's result on the same worlds:
# the root's candidates are its survivors + the best of the rest by its q (at least ROOT_ROWS rows) x its final opponent
# columns (expanded + rechecked replies included), and every root pair starts with PRIOR_K pseudo-visits at the halving
# search's one-turn value (FILL_K at the row's q where that pair was never evaluated). Before, the root held the policy's top
# 12 x the opponent head's top list with no anchor: a few noisy samples per pair decided the pick.
ROOT_ROWS = int(os.environ.get("MCTS_ROOT_ROWS", "6"))
PRIOR_K = float(os.environ.get("MCTS_PRIOR_K", "8"))
FILL_K = float(os.environ.get("MCTS_FILL_K", "1"))
# (2026-10-28, fixes 1-4 after a code review) 1: the opponent's candidates below the root (was 6: deeper nodes gave them far
# fewer options than the root = values too good for us, part of the measured +0.1-0.3 gap); 2: replies the halving search
# dropped come back at the end when they beat our final mix (rounds, replies per round, margin, extra simulations as a share
# of sims); 3: prior weight per pair = min(halving evaluations, PRIOR_K) (MCTS_PRIOR_COUNTS=0 = flat PRIOR_K); 4: cover
# passes on common worlds / dice (MCTS_COVER_CRN=0 = old)
K_DEEP = tuple(int(x) for x in os.environ.get("MCTS_K_DEEP", "4,6").split(","))   # (2026-10-28 ~9:40, user) was 6,12
EXPAND_ROUNDS = int(os.environ.get("MCTS_EXPAND_ROUNDS", "2"))
EXPAND_ADD = int(os.environ.get("MCTS_EXPAND_ADD", "3"))
EXPAND_MARGIN = float(os.environ.get("MCTS_EXPAND_MARGIN", "0.01"))
EXPAND_SIMS = float(os.environ.get("MCTS_EXPAND_SIMS", "0.1"))
PRIOR_COUNTS = os.environ.get("MCTS_PRIOR_COUNTS", "1") != "0"
COVER_CRN = os.environ.get("MCTS_COVER_CRN", "1") != "0"
# (2026-10-28, items 5-6 of the review) 5: a node's candidates are the union over up to NODE_WORLDS worlds whose legal sets
# differ (the first world to reach a node used to decide both sides' candidates for every world); without a prior the root's
# candidates come from all worlds' net outputs (ROOT_UNION); 6a: a replacement root doesn't use up a turn of depth (REPL_DEPTH);
# 6c: a prior root explores with the halving search's mix (+ ROOT_MIX_FLOOR per candidate) instead of the raw policy
# (ROOT_PRIOR_MIX); 6d: obs_key also tells boosts, screens, Tailwind and Trick Room turns apart (OBS_FINE)
NODE_WORLDS = int(os.environ.get("MCTS_NODE_WORLDS", "3"))
ROOT_UNION = os.environ.get("MCTS_ROOT_UNION", "1") != "0"
REPL_DEPTH = os.environ.get("MCTS_REPL_DEPTH", "1") != "0"
ROOT_PRIOR_MIX = os.environ.get("MCTS_ROOT_PRIOR_MIX", "1") != "0"
ROOT_MIX_FLOOR = float(os.environ.get("MCTS_ROOT_MIX_FLOOR", "0.02"))
OBS_FINE = os.environ.get("MCTS_OBS_FINE", "1") != "0"
# (2026-10-28, review 2) 1a: node keys. "coarse" (default) = phase, who must choose, the actives' species and the number of
# Pokemon left per side: HP / status / boosts / timers in the key had split the tree so finely that nodes 2-3 turns down got
# ~1 visit each (both sides then just sampled their priors). "fine" = the items-5-6 key, "old" = the original one
# (MCTS_OBS_FINE=0 without MCTS_OBS also gives "old").
OBS_MODE = os.environ.get("MCTS_OBS") or ("coarse" if OBS_FINE else "old")
# 1b: below the root the opponent's candidates and prior come from the network run from THEIR side in that world (their own
# policy), not from our opponent-prediction head (measured flatter: joint entropy 3.0 vs 2.45 closed, P(their top) 0.16 vs 0.35)
OPP_SELF = os.environ.get("MCTS_OPP_SELF", "1") != "0"
# 5: time limit per search in seconds (0 = none); search(..., deadline=) = an absolute time.time() limit (the bot's)
SECONDS = float(os.environ.get("MCTS_SECONDS", "0"))
# (2026-10-28, review 2 follow-up) progressive widening below the root: a node plays only its WIDEN_BASE + WIDEN_C x n^WIDEN_ALPHA
# most likely candidates per side (n = its visits; 2 at first, 3 after 4 visits, 4 after 16, 6 after 64...), so its few
# visits go to a few pairs instead of being spread over ~70-150 (MCTS_WIDEN=1 turns it on; off by default)
WIDEN = os.environ.get("MCTS_WIDEN", "0") != "0"
# (2026-10-28, user: "of course the opponent would know their own moves") the opponent KNOWS its own set: every node is solved
# as a Bayesian game over the set worlds (one strategy for us, who can't tell the worlds apart; one per world for them,
# nnsearch.bayes_solve) and their action in a simulation is drawn from THEIR strategy for that simulation's world. Before, one
# opponent strategy served all 8 worlds, as if they didn't know which Pokemon / moves / items they had. Per-world means are
# shrunk toward the node's pooled mean (BAYES_K pseudo-visits), since each world gets only 1/W of a node's visits.
# BAYES_ITERS = solver iterations below the root (the root, and nodes with > 200 visits, use 1000). MCTS_BAYES=0 = old.
BAYES = os.environ.get("MCTS_BAYES", "1") != "0"
BAYES_K = float(os.environ.get("MCTS_BAYES_K", "2"))
BAYES_ITERS = int(os.environ.get("MCTS_BAYES_ITERS", "300"))
# (2026-10-28 afternoon, after the head to head lost 44.6% to xstrong; user: "give the deeper branches more looks")
# FOCUS: with a halving prior, the root holds only the CONTESTED decision: our rows = the prior's survivors with equilibrium
# weight >= FOCUS_MINX or value within FOCUS_QGAP of the best (2..FOCUS_ROWS rows), their columns = the prior's FOCUS_COLS most
# likely replies (the others stay in the expansion pool). 3000 simulations then go to ~15 root pairs (~200 each) instead of
# ~100+. When the prior is already sure (top weight >= FOCUS_SKIP, or one contender) the tree is skipped: the prior's result
# is returned (r["skipped"]). MCTS_FOCUS=1 turns it on (off by default).
FOCUS = os.environ.get("MCTS_FOCUS", "0") != "0"
FOCUS_ROWS = int(os.environ.get("MCTS_FOCUS_ROWS", "3"))
FOCUS_COLS = int(os.environ.get("MCTS_FOCUS_COLS", "5"))
FOCUS_MINX = float(os.environ.get("MCTS_FOCUS_MINX", "0.05"))
FOCUS_QGAP = float(os.environ.get("MCTS_FOCUS_QGAP", "0.03"))
FOCUS_SKIP = float(os.environ.get("MCTS_FOCUS_SKIP", "0.9"))
# TRUST_K: a node's value as seen by its parent = (TRUST_K x its own value-net estimate + n x its current equilibrium value)
# / (TRUST_K + n), n = its visits. A branch with a few noisy looks barely moves its parent; only many looks override the
# net. Replaces the raw sampled return in the backup (each node above gets its child's blended estimate). 0 = off (old).
TRUST_K = float(os.environ.get("MCTS_TRUST_K", "0"))
WIDEN_BASE = float(os.environ.get("MCTS_WIDEN_BASE", "2"))
WIDEN_C = float(os.environ.get("MCTS_WIDEN_C", "0.5"))
WIDEN_ALPHA = float(os.environ.get("MCTS_WIDEN_ALPHA", "0.5"))
PASS = ("pass", 0, 0, False)


def _hp8(m):
    return 0 if m["fainted"] or m["hp"] <= 0 else max(1, int(round(8 * m["hp"] / max(m["maxhp"], 1))))


def obs_key(c):
    """what both players see after a turn (coarse): enough to tell decision points apart, coarse enough to share nodes"""
    st = c.state()
    if OBS_MODE == "coarse":
        sides = []
        for s in (0, 1):
            sd = st["sides"][s]
            sp = []
            for k in range(2):
                a = sd["active"][k] if k < len(sd["active"]) else None
                m = sd["mons"][a] if a is not None and 0 <= a < len(sd["mons"]) else None
                sp.append(None if m is None or m["fainted"] or m["hp"] <= 0 else m["species"])
            alive = sum(1 for m in sd["mons"] if m.get("brought", True) and not m["fainted"] and m["hp"] > 0)
            sides.append((tuple(sp), alive))
        return (st["phase"], c.needs_choice(0), c.needs_choice(1), tuple(sides))
    fine = OBS_MODE == "fine"
    sides = []
    for s in (0, 1):
        sd = st["sides"][s]
        act = []
        for k in range(2):
            a = sd["active"][k] if k < len(sd["active"]) else None
            m = sd["mons"][a] if a is not None and 0 <= a < len(sd["mons"]) else None
            if m is None:
                act.append(None)
            elif fine:
                bo = m.get("boosts") or {}
                cl = lambda v: max(-2, min(2, int(v)))
                act.append((m["species"], _hp8(m), m["status"], cl(bo.get("atk", 0)), cl(bo.get("spa", 0)),
                            cl(bo.get("spe", 0)), int(np.sign(bo.get("def", 0) + bo.get("spd", 0)))))
            else:
                act.append((m["species"], _hp8(m), m["status"]))
        alive = sum(1 for m in sd["mons"] if m.get("brought", True) and not m["fainted"] and m["hp"] > 0)
        if fine:
            sides.append((tuple(act), alive, int(sd.get("tailwind", 0)), sd.get("reflect", 0) > 0,
                          sd.get("light_screen", 0) > 0, sd.get("aurora_veil", 0) > 0))
        else:
            sides.append((tuple(act), alive))
    f = st["field"]
    tr = int(f["trick_room"]) if fine else f["trick_room"] > 0
    return (st["phase"], c.needs_choice(0), c.needs_choice(1), tuple(sides), f["weather"], f["terrain"], tr)


def _sig(ls0, ls1):
    """a world's legal sets at a node (item 5: worlds whose candidates can differ)"""
    fz = lambda ls: None if ls is None else tuple(frozenset(x) for x in ls)
    return (fz(ls0), fz(ls1))


class Node:
    __slots__ = ("ours", "theirs", "po", "pt", "S", "C", "S0", "C0", "v0", "n", "kids", "x", "y", "n_solved", "turns", "pw", "act", "Sw", "Cw", "Y",
                 "sigs", "pa_o", "pa_t")

    def __init__(self, ours, theirs, v0, turns):
        self.ours, self.theirs = ours, theirs                      # lists of (p, a0, a1)
        po = np.array([o[0] for o in ours], np.float64) + 1e-9
        pt = np.array([t[0] for t in theirs], np.float64) + 1e-9
        self.po, self.pt = po / po.sum(), pt / pt.sum()
        self.S = np.zeros((len(ours), len(theirs)))
        self.C = np.zeros((len(ours), len(theirs)))
        self.S0 = self.C0 = 0.0                                    # pseudo-visits (root prior); C counts real simulations
        self.v0 = float(v0)
        self.n = 0
        self.kids = {}
        self.x, self.y, self.n_solved = self.po, self.pt, -1
        self.turns = turns                                         # turns played from the root to here
        self.pw = False                                            # progressive widening (nodes below the root)
        self.act = None                                            # active candidates (ours, theirs) at the last solve
        self.Sw = self.Cw = None                                   # (Bayesian) per-world sums / counts [W, ours, theirs]
        self.Y = None                                              # their strategy per world [W, theirs]
        self.sigs = None                                           # (item 5) legal-set signatures of the worlds merged in
        self.pa_o = np.array([o[0] for o in ours], np.float64)     # summed priors over those worlds
        self.pa_t = np.array([t[0] for t in theirs], np.float64)

    def bayes_init(self, W):
        if BAYES and W > 1:
            self.Sw = np.zeros((W, len(self.ours), len(self.theirs)))
            self.Cw = np.zeros((W, len(self.ours), len(self.theirs)))

    def matrix_w(self):
        """per-world means, shrunk toward the pooled matrix (which holds the root prior and level correction)"""
        Mp = self.matrix()
        return (self.Sw + BAYES_K * Mp[None]) / (self.Cw + BAYES_K)

    def merge(self, o_new, t_new):
        """(item 5) add another world's candidates: new rows / columns start unvisited; priors = mean over the worlds"""
        def grow(lst, pa, new):
            idx = {(tuple(e[1]), tuple(e[2])): i for i, e in enumerate(lst)}
            add = np.zeros(len(lst), np.float64)
            lst = list(lst)
            for e in new:
                k = (tuple(e[1]), tuple(e[2]))
                if k in idx:
                    add[idx[k]] += e[0]
                else:
                    idx[k] = len(lst)
                    lst.append(e)
                    add = np.append(add, e[0])
            pa = np.concatenate([pa, np.zeros(len(lst) - len(pa))]) + add
            return lst, pa
        n_o, n_t = len(self.ours), len(self.theirs)
        self.ours, self.pa_o = grow(self.ours, self.pa_o, o_new)
        self.theirs, self.pa_t = grow(self.theirs, self.pa_t, t_new)
        do, dt = len(self.ours) - n_o, len(self.theirs) - n_t
        if do or dt:
            self.S = np.pad(self.S, ((0, do), (0, dt)))
            self.C = np.pad(self.C, ((0, do), (0, dt)))
            if self.Sw is not None:
                self.Sw = np.pad(self.Sw, ((0, 0), (0, do), (0, dt)))
                self.Cw = np.pad(self.Cw, ((0, 0), (0, do), (0, dt)))
        self.po = (self.pa_o + 1e-9) / (self.pa_o + 1e-9).sum()
        self.pt = (self.pa_t + 1e-9) / (self.pa_t + 1e-9).sum()
        self.n_solved = -1
        if self.n == 0:
            self.x, self.y = self.po, self.pt

    def matrix(self):
        S0 = self.S0
        if isinstance(self.C0, np.ndarray):
            # (2026-10-28) the prior = one-turn values; the tree's multi-turn means sit at another level (measured +0.1 to
            # +0.3). Shift the prior by the visit-weighted mean gap on the pairs simulated so far, else lightly visited rows
            # stay at the lower level and the most simulated row wins by construction.
            vis = (self.C > 0) & (self.C0 > 0)
            if vis.any():
                P = S0 / np.maximum(self.C0, 1e-9)
                off = float((self.S[vis] - self.C[vis] * P[vis]).sum() / self.C[vis].sum())
                S0 = self.C0 * (P + off)
        C = self.C + self.C0
        return np.where(C > 0, (self.S + S0) / np.maximum(C, 1e-9), self.v0)

    def width(self):
        if not self.pw:
            return len(self.ours), len(self.theirs)
        m = int(math.ceil(WIDEN_BASE + WIDEN_C * self.n ** WIDEN_ALPHA))
        return min(len(self.ours), m), min(len(self.theirs), m)

    def _solve(self, mo, mt):
        """x (ours) and their strategy: y (one for all worlds) or Y [W, mt] (Bayesian), over the first mo / mt candidates"""
        if mo == 1 and mt == 1:
            x, y, Y = np.ones(1), np.ones(1), None
            if self.Sw is not None:
                Y = np.ones((self.Sw.shape[0], 1))
        elif self.Sw is not None:
            x, Y, _, _ = bayes_solve(self.matrix_w()[:, :mo, :mt], iters=1000 if self.n > 200 else BAYES_ITERS)
            Y = np.maximum(np.asarray(Y, np.float64), 0)
            Y /= np.maximum(Y.sum(1, keepdims=True), 1e-12)
            y = Y.mean(0)
        else:
            x, y = solve(self.matrix()[:mo, :mt])
            Y = None
        x = np.maximum(np.asarray(x, np.float64), 0)
        y = np.maximum(np.asarray(y, np.float64), 0)
        x /= max(x.sum(), 1e-12)
        y /= max(y.sum(), 1e-12)
        no, nt = len(self.ours) - mo, len(self.theirs) - mt
        self.x = np.concatenate([x, np.zeros(no)])
        self.y = np.concatenate([y, np.zeros(nt)])
        self.Y = None if Y is None else np.hstack([Y, np.zeros((Y.shape[0], nt))])

    def mixes(self, w=0):
        """sampling mixes for a simulation in world w: ours (shared by all worlds), theirs (their strategy for world w)"""
        if len(self.ours) == 1 and len(self.theirs) == 1:
            return self.po, self.pt
        mo, mt = self.width()
        if self.n_solved < 0 or self.act != (mo, mt) or self.n >= self.n_solved + max(1, self.n_solved // 8):
            self._solve(mo, mt)
            self.n_solved, self.act = self.n, (mo, mt)
        po, pt = self.po, self.pt
        if self.pw:
            po = np.concatenate([po[:mo], np.zeros(len(self.ours) - mo)])
            pt = np.concatenate([pt[:mt], np.zeros(len(self.theirs) - mt)])
            po, pt = po / po.sum(), pt / pt.sum()
        y = self.y if self.Y is None or self.n == 0 else self.Y[w % self.Y.shape[0]]
        eps = min(EPS_MAX, EPS_C / math.sqrt(1 + self.n))
        return (1 - eps) * self.x + eps * po, (1 - eps) * y + eps * pt

def _net_eval(net, cs, side, device, hidden):
    """value-net + policy + opponent head (+ opponent joint table) for a list of positions, from `side`'s view"""
    F, I = [], []
    for c in cs:
        f, ii = vgcsim.encode_battle(c, side, hidden=hidden)
        F.append(f)
        I.append(ii)
    F = np.asarray(F, np.float32).reshape(len(F), -1)
    I = np.asarray(I, np.int32).reshape(len(I), -1)
    with torch.no_grad(), amp():
        tf, ti = torch.from_numpy(F).to(device), torch.from_numpy(I).to(device)
        if has_opp_joint(net):
            l0, v, ol, oj = net(tf, ti, opp=True, opp_joint=True)
            oj = oj.float().cpu().numpy()
        else:
            l0, v, ol = net(tf, ti, opp=True)
            oj = None
    return l0.float().cpu().numpy(), v.float().cpu().numpy().reshape(-1), ol.float().cpu().numpy(), oj


def _cands(c, side, l, ol, oj, k_o, k_t):
    ours = top_joint(l, c.legal_index_map(side), k_o) if c.needs_choice(side) else [(1.0, PASS, PASS)]
    theirs = (their_joint(ol, oj, c.legal_index_map(1 - side), k_t) if c.needs_choice(1 - side)
              else [(1.0, PASS, PASS)])
    return ours or [(1.0, PASS, PASS)], theirs or [(1.0, PASS, PASS)]


def _their_self(c, side, l1, k_t):
    """(1b) the opponent's candidates from their own policy (logits from their side's view of c)"""
    if not c.needs_choice(1 - side):
        return [(1.0, PASS, PASS)]
    return top_joint(l1, c.legal_index_map(1 - side), k_t) or [(1.0, PASS, PASS)]


def _legal(c, side, acts):
    lg = c.legal_actions(side)
    return all(tuple(acts[k]) in lg[k] for k in range(min(2, len(lg))))


def _legal_sets(c, side):
    return [set(tuple(a) for a in x) for x in c.legal_actions(side)]


def _focus_rows(r):
    q, surv = r["q"], [int(a) for a in r["survivors"]]
    x = np.asarray(r["x"], np.float64)
    best = max(q.get(a, -9) for a in surv)
    order = sorted(surv, key=lambda a: -float(x[a]) if a < len(x) else 0.0)
    keep = [a for a in order if (a < len(x) and x[a] >= FOCUS_MINX) or q.get(a, -9) >= best - FOCUS_QGAP][:FOCUS_ROWS]
    for a in order + sorted((a for a in q if a not in surv), key=lambda a: -q[a]):
        if len(keep) >= 2:
            break
        if a not in keep:
            keep.append(a)
    return keep


def focus_skip(r):
    """(FOCUS) True when the halving prior is already sure: one contender, or its top equilibrium weight >= FOCUS_SKIP"""
    x = np.asarray(r["x"], np.float64)
    surv = [int(a) for a in r["survivors"]]
    return len(surv) <= 1 or float(max(x[a] for a in surv if a < len(x))) >= FOCUS_SKIP


def _sel_cols(r):
    """the prior's opponent columns (FOCUS: its FOCUS_COLS most likely) and their weights in the prior's mix"""
    cols = [int(b) for b in r["y_survivors"][0]]
    y = np.asarray(r["y_survivors"][1], np.float64)[:len(cols)]
    if FOCUS and len(cols) > FOCUS_COLS:
        idx = list(np.argsort(-y)[:FOCUS_COLS])
        return [cols[i] for i in idx], y[idx]
    return cols, y


def _sel_rows(r, rows=None):
    if FOCUS:
        return _focus_rows(r)
    rows = ROOT_ROWS if rows is None else rows
    q, surv = r["q"], [int(a) for a in r["survivors"]]
    rest = sorted((a for a in q if a not in surv), key=lambda a: -q[a])
    return surv + rest[:max(0, rows - len(surv))]


def prior_root(r, rows=None, k_eval=None, k_fill=None):
    """root candidates + pseudo-visits from a halving search's result r (see ROOT_ROWS / PRIOR_K above).
    (2026-10-28, fix 3) a pair's weight = min(the halving search's world evaluations of it (r["C_all"]), PRIOR_K): replies it
    measured in few worlds no longer count as much as the survivors' pairs. Also returns the halving search's OTHER evaluated
    replies (columns, prior values, weights) for the final expansion step (fix 2)."""
    rows = ROOT_ROWS if rows is None else rows
    k_eval = PRIOR_K if k_eval is None else k_eval
    k_fill = FILL_K if k_fill is None else k_fill
    q, V = r["q"], float(r["value"])
    sel = _sel_rows(r, rows)
    cols = _sel_cols(r)[0]
    Mall = np.asarray(r["M_all"], np.float64)
    Call = np.asarray(r["C_all"], np.float64) if (r.get("C_all") is not None and PRIOR_COUNTS) else None
    fill_row = np.array([q.get(a, V) for a in sel], np.float64)[:, None]

    def block(cs):
        P = Mall[np.ix_(sel, cs)]
        if Call is not None:
            K = np.where(np.isnan(P), k_fill, np.minimum(np.maximum(Call[np.ix_(sel, cs)], 1.0), k_eval))
        else:
            K = np.where(np.isnan(P), k_fill, k_eval)
        return np.where(np.isnan(P), np.repeat(fill_row, len(cs), axis=1), P), K
    P, K = block(cols)
    others = [b for b in range(Mall.shape[1]) if b not in cols and not np.all(np.isnan(Mall[sel, b]))]
    extra = None
    if others:
        Po, Ko = block(others)
        extra = ([r["theirs"][b] for b in others], Po, Ko)
    return [r["ours"][a] for a in sel], [r["theirs"][b] for b in cols], P, K, extra


def _pick(rng, p, ok=None):
    if ok is not None:
        p = p * ok
        if p.sum() <= 0:
            return None
    return int(rng.choice(len(p), p=p / p.sum()))


def _add_cols(node, new, P, K):
    """append opponent candidates to a node (root expansion): no simulations yet, prior pseudo-visits K at values P"""
    n = len(new)
    node.theirs = node.theirs + list(new)
    node.pa_t = np.concatenate([node.pa_t, [t[0] for t in new]])
    if ROOT_PRIOR_MIX and isinstance(node.C0, np.ndarray):         # (6c) the root explores with the prior's mix + a floor
        pt = np.concatenate([node.pt, np.full(n, ROOT_MIX_FLOOR)])
    else:
        pt = node.pa_t + 1e-9
    node.pt = pt / pt.sum()
    node.S = np.hstack([node.S, np.zeros((node.S.shape[0], n))])
    node.C = np.hstack([node.C, np.zeros((node.C.shape[0], n))])
    if node.Sw is not None:
        node.Sw = np.pad(node.Sw, ((0, 0), (0, 0), (0, n)))
        node.Cw = np.pad(node.Cw, ((0, 0), (0, 0), (0, n)))
    if isinstance(node.C0, np.ndarray):
        node.S0 = np.hstack([node.S0, K * P])
        node.C0 = np.hstack([node.C0, K])
    node.n_solved = -1


def _offset(node):
    """the root's level correction (see Node.matrix): visit-weighted gap between the tree's means and the prior"""
    if not isinstance(node.C0, np.ndarray):
        return 0.0
    vis = (node.C > 0) & (node.C0 > 0)
    if not vis.any():
        return 0.0
    P = node.S0 / np.maximum(node.C0, 1e-9)
    return float((node.S[vis] - node.C[vis] * P[vis]).sum() / node.C[vis].sum())


def search(net, battle, side, device, hidden=False, seed=1, sims=800, batch=64, depth=3, k_root=(12, 12), k_deep=None,
           max_turns=None, force=None, transform=None, root_cover=0, prior=None, deadline=None):
    """see the module docstring. `battle`: a position or a list of positions (worlds; simulation i uses world i % W).
    `depth`: turns searched below the root (a node more than `depth` turns from the root is a leaf: value net).
    (2026-10-28, game review) `force` = our joint action (a0, a1) added to the root candidates (r["forced"] = its index);
    `transform` = applied to the value net's outputs (calibration to win chances; finished games become 1 / 0 / 0.5);
    `root_cover` = the first simulations visit every root (our, their) pair this many times, so every candidate has a value
    against every reply (r["M_all"]: root means, NaN where a pair was never visited).
    (2026-10-28) `prior` = a halving search's result on the same worlds and side (see prior_root); then r["M_all"] / r["q"]
    include the prior's pseudo-visits and r["visits"] counts real simulations only.
    (2026-10-28, fixes 1-4) k_deep default = K_DEEP (6 ours, 12 theirs; was 6 / 6); with a prior, replies the halving search
    evaluated but dropped come back at the end when they beat our final mix (EXPAND_*); the root's cover passes use one world
    and one dice seed per pass for every pair (COVER_CRN).
    (2026-10-28, review 2) `deadline` = absolute time.time() limit (also MCTS_SECONDS from the start): checked before every
    batch of simulations and expansion round; the search returns what it has (r["timed_out"])."""
    t_start = time.time()
    if SECONDS > 0:
        deadline = min(deadline or 1e18, t_start + SECONDS)
    k_deep = K_DEEP if k_deep is None else k_deep
    worlds = list(battle) if isinstance(battle, (list, tuple)) else [battle]
    if len(worlds) == 1 and not getattr(worlds[0], "keep_foe_stats", True) and hasattr(worlds[0], "search_worlds"):
        # (2026-10-28, leaks) a TRUE position: search worlds as the halving search builds them
        # (opponent stats guessed / sampled, HP within the shown percentage; closed sheets also redraw what is unrevealed),
        # our side = side 0. Before, MCTS searched the true position itself: exact stats, HP, items, hidden counters.
        if hidden:
            ensure_world_sets()
        worlds = worlds[0].search_worlds(side, WORLDS, int(seed) & 0x7FFFFFFF, hidden=bool(hidden))
        side = 0
    b0 = worlds[0]
    W = len(worlds)
    k_t0 = K_THEIRS or k_root[1]
    union = prior is None and ROOT_UNION and W > 1
    l, v, ol, oj = _net_eval(net, worlds if union else [b0], side, device, hidden)
    extra_cols = None
    if prior is not None and FOCUS and force is None and focus_skip(prior):
        return dict(prior, skipped=True, secs=round(time.time() - t_start, 3))
    if prior is not None:
        assert transform is None, "prior values are raw value-net values"
        ours, theirs, P0, K0, extra_cols = prior_root(prior)
    elif union:
        # (item 5) the root's candidates from every world's net outputs: mean joint probability over the worlds, top k
        acc_o, acc_t = {}, {}
        for w in range(W):
            o_, t_ = _cands(worlds[w], side, l[w], ol[w], None if oj is None else oj[w], k_root[0], k_t0)
            for acc, lst in ((acc_o, o_), (acc_t, t_)):
                for e in lst:
                    k = (tuple(e[1]), tuple(e[2]))
                    acc[k] = (acc[k][0] + e[0] / W, e[1], e[2]) if k in acc else (e[0] / W, e[1], e[2])
        ours = sorted(acc_o.values(), key=lambda e: -e[0])[:k_root[0]]
        theirs = sorted(acc_t.values(), key=lambda e: -e[0])[:k_t0]
        v = np.array([float(np.mean(v))])
    else:
        ours, theirs = _cands(b0, side, l[0], ol[0], None if oj is None else oj[0], k_root[0], k_t0)
    forced = None
    if force is not None:
        ft = tuple(tuple(a) for a in force)
        for i, o in enumerate(ours):
            if (tuple(o[1]), tuple(o[2])) == ft:
                forced = i
        if forced is None and (not b0.needs_choice(side) or _legal(b0, side, ft)):
            ours = ours + [(1e-6, ft[0], ft[1])]
            forced = len(ours) - 1
    tf = transform
    v0r = float(tf(np.asarray([v[0]]))[0]) if tf is not None else v[0]
    # (6a) a replacement root: entering the next turn after it doesn't count as a searched turn
    root = Node(ours, theirs, v0r, -1 if (REPL_DEPTH and b0.phase == "switch") else 0)
    root.bayes_init(W)
    npad = 0
    if prior is not None:
        npad = len(ours) - P0.shape[0]                            # a forced action of ours added after the prior's rows
        if npad:
            P0 = np.vstack([P0, np.full((npad, P0.shape[1]), v0r)])
            K0 = np.vstack([K0, np.zeros((npad, K0.shape[1]))])
        root.S0, root.C0 = K0 * P0, K0
        if ROOT_PRIOR_MIX:
            # (6c) explore with the halving search's equilibrium (+ a floor), not the network's raw policy
            px = np.asarray(prior["x"], np.float64)
            po = np.array([float(px[a]) if a < len(px) else 0.0 for a in _sel_rows(prior)] + [0.0] * npad) + ROOT_MIX_FLOOR
            pt = _sel_cols(prior)[1][:len(theirs)] + ROOT_MIX_FLOOR
            if len(pt) == len(theirs):
                root.po, root.pt = po / po.sum(), pt / pt.sum()
                root.x, root.y = root.po, root.pt
    l_root = l[0]
    rng = np.random.default_rng(seed)
    st = {"leaf": 0, "term": 0, "depth": 0, "illegal": 0, "sim_id": 0, "merged": 0, "timed_out": False, "done": 0,
          "opp_self": 0}

    def make_cover(pairs):
        # (fix 4) pass k of the cover plays every pair in world k % W with the same dice seed (common random numbers, as the
        # halving search compares its candidates); before, a pair's world and dice followed the simulation counter
        return [(i, j, k) for k in range(root_cover) for (i, j) in pairs]

    def run(n_sims, cover):
        cover = list(reversed(cover))
        done = 0
        while done < n_sims or cover:
            if deadline is not None and time.time() > deadline:
                st["timed_out"] = True
                return
            nb = min(batch, max(n_sims - done, len(cover)))
            pend = []                           # (path, position, key, turns, world) for new nodes to value
            backs = []                          # (path, value, world) finished now
            for _ in range(nb):
                cv = cover.pop() if cover else None
                wd = (cv[2] if cv is not None and COVER_CRN else st["sim_id"]) % W     # this simulation's world
                if cv is not None and COVER_CRN:
                    c = worlds[cv[2] % W].copy()
                    c.reseed(int(seed * 1000003 + 7907 * (cv[2] + 1) + 104729) & 0x7FFFFFFF)
                else:
                    c = worlds[st["sim_id"] % W].copy()
                    c.reseed(int(seed * 1000003 + st["sim_id"]) & 0x7FFFFFFF)
                st["sim_id"] += 1
                node, path = root, []
                while True:
                    xo, yt = node.mixes(wd)
                    # open loop: a candidate may be illegal in THIS sample (Encore, PP, a different world's sets...)
                    ls0 = _legal_sets(c, side) if c.needs_choice(side) else None
                    ls1 = _legal_sets(c, 1 - side) if c.needs_choice(1 - side) else None
                    if node.sigs is not None and len(node.sigs) < NODE_WORLDS and W > 1:
                        sg = _sig(ls0, ls1)
                        if sg not in node.sigs:
                            # (item 5) a world with other legal sets reaches this node: value the position here and merge
                            # this world's candidates into the node (no deeper simulation this time)
                            node.sigs.add(sg)
                            pend.append((path, c, ("merge", node), node.turns, wd))
                            st["merged"] += 1
                            break
                    ok_o = np.array([1.0 if ls0 is None or all(tuple(o[1 + s]) in ls0[s] for s in range(min(2, len(ls0))))
                                     else 0.0 for o in node.ours])
                    ok_t = np.array([1.0 if ls1 is None or all(tuple(t[1 + s]) in ls1[s] for s in range(min(2, len(ls1))))
                                     else 0.0 for t in node.theirs])
                    a = b = None
                    if node is root and cv is not None:
                        a, b = cv[0], cv[1]
                        cv = None
                        if ok_o[a] <= 0 or ok_t[b] <= 0:            # illegal in this sample: an ordinary simulation
                            a = b = None
                    if a is None:
                        a = _pick(rng, xo, ok_o)
                        b = _pick(rng, yt, ok_t)
                        if node.pw:                              # every active candidate illegal here: the wider list
                            a = _pick(rng, node.po, ok_o) if a is None else a
                            b = _pick(rng, node.pt, ok_t) if b is None else b
                    if a is None or b is None:
                        st["illegal"] += 1
                        pend.append((path, c, None, 0, wd))      # nothing legal here in this sample: value the position itself
                        break
                    if c.needs_choice(side):
                        c.choose(side, tuple(node.ours[a][1]), tuple(node.ours[a][2]))
                    if c.needs_choice(1 - side):
                        c.choose(1 - side, tuple(node.theirs[b][1]), tuple(node.theirs[b][2]))
                    path.append((node, a, b))
                    if c.phase == "ended":
                        w = c.winner
                        tv = 1.0 if w == side else (-1.0 if w is not None and w >= 0 else 0.0)
                        backs.append((path, (tv + 1) / 2 if tf is not None else tv, wd))
                        st["term"] += 1
                        break
                    turns = node.turns + (1 if c.phase == "turn" else 0)
                    key = (a, b, obs_key(c))
                    kid = node.kids.get(key)
                    if kid is None or turns > depth or (max_turns and c.turn > max_turns):
                        pend.append((path, c, key if (kid is None and turns <= depth) else None, turns, wd))
                        break
                    node = kid
                    st["depth"] = max(st["depth"], len(path))
            if pend:
                cs = [p[1] for p in pend]
                l_, v_, ol_, oj_ = _net_eval(net, cs, side, device, hidden)
                th = {}                                     # (1b) their own policy where a node is made or merged
                if OPP_SELF:
                    ni = [i for i, p in enumerate(pend) if p[2] is not None and (p[2][0] == "merge" or p[0])]
                    if ni:
                        l1 = _net_eval(net, [cs[i] for i in ni], 1 - side, device, hidden)[0]
                        th = {i: l1[j] for j, i in enumerate(ni)}
                        st["opp_self"] += len(ni)
                if tf is not None:
                    v_ = np.asarray(tf(np.asarray(v_, np.float64)), np.float64)
                st["leaf"] += len(cs)
                for i, p in enumerate(pend):
                    path, c, key = p[0], p[1], p[2]
                    if key is not None and key[0] == "merge":
                        o_, t_ = _cands(c, side, l_[i], ol_[i], None if oj_ is None else oj_[i], *k_deep)
                        if i in th:
                            t_ = _their_self(c, side, th[i], k_deep[1])
                        key[1].merge(o_, t_)
                    elif key is not None and path:
                        parent = path[-1][0]
                        if key not in parent.kids:
                            o_, t_ = _cands(c, side, l_[i], ol_[i], None if oj_ is None else oj_[i], *k_deep)
                            if i in th:
                                t_ = _their_self(c, side, th[i], k_deep[1])
                            kid = Node(o_, t_, v_[i], p[3])
                            kid.bayes_init(W)
                            kid.pw = WIDEN
                            if W > 1:
                                kid.sigs = {_sig(_legal_sets(c, side) if c.needs_choice(side) else None,
                                                 _legal_sets(c, 1 - side) if c.needs_choice(1 - side) else None)}
                            parent.kids[key] = kid
                    backs.append((path, float(v_[i]), p[4]))
            for path, val, wd_ in backs:
                for node, a, b in reversed(path):           # deepest first (TRUST: the parent gets this node's estimate)
                    node.S[a, b] += val
                    if node.Sw is not None:
                        node.Sw[wd_, a, b] += val
                        node.Cw[wd_, a, b] += 1
                    node.C[a, b] += 1
                    node.n += 1
                    if TRUST_K > 0:
                        val = trusted_value(node)
            done += nb
            st["done"] += nb

    run(sims, make_cover([(i, j) for i in range(len(ours)) for j in range(len(theirs))]))
    n_added = 0
    if extra_cols is not None and EXPAND_ROUNDS > 0:
        # (fix 2) the halving search dropped these replies against ITS mix; against our final mix one of them can be their
        # best answer. Judge each by its prior values (level-corrected), add the best ones (EXPAND_ADD per round) that beat
        # the current equilibrium for them by EXPAND_MARGIN, simulate them, re-solve.
        pool_t, pool_P, pool_K = extra_cols
        if npad:
            pool_P = np.vstack([pool_P, np.full((npad, pool_P.shape[1]), v0r)])
            pool_K = np.vstack([pool_K, np.zeros((npad, pool_K.shape[1]))])
        left = list(range(len(pool_t)))
        for _ in range(EXPAND_ROUNDS):
            if not left or st["timed_out"] or (deadline is not None and time.time() > deadline):
                break
            root._solve(len(root.ours), len(root.theirs))
            x_ = root.x
            V_ = float(x_ @ (root.matrix() @ root.y)) if root.Y is None else \
                float(np.mean([x_ @ Mw_ @ Y_ for Mw_, Y_ in zip(root.matrix_w(), root.Y)]))
            vals = x_ @ (pool_P[:, left] + _offset(root))        # each dropped reply's value for us against our mix
            order = [left[j] for j in np.argsort(vals) if vals[j] < V_ - EXPAND_MARGIN][:EXPAND_ADD]
            if not order:
                break
            k = len(root.theirs)
            _add_cols(root, [pool_t[j] for j in order], pool_P[:, order], pool_K[:, order])
            left = [j for j in left if j not in order]
            n_added += len(order)
            run(int(sims * EXPAND_SIMS),
                make_cover([(i, j) for i in range(len(ours)) for j in range(k, len(root.theirs))]))
    theirs = root.theirs
    M = root.matrix()
    if root.Sw is not None:
        # (Bayesian) our values against their per-world strategies; y = their strategy averaged over the worlds (for logs,
        # midturn.py and the opponent model, which expect one mix)
        x, Yw, V, rows = bayes_solve(root.matrix_w(), iters=1000)
        x = np.maximum(np.asarray(x, np.float64), 0)
        y = np.maximum(np.asarray(Yw, np.float64), 0).mean(0)
        rows = np.asarray(rows, np.float64)
    else:
        x, y = solve(M)
        x = np.maximum(np.asarray(x, np.float64), 0)
        y = np.maximum(np.asarray(y, np.float64), 0)
        V = float(x @ M @ y)
        rows = M @ y
    q = {i: float(rows[i]) for i in range(len(ours)) if prior is not None or root.C[i].sum() > 0}
    res = {"ours": ours, "theirs": theirs, "x": x, "survivors": list(range(len(ours))), "value": V, "q": q,
           "leaves": st["leaf"], "terminal": st["term"], "root_value": root.v0, "visits": root.C.copy(),
           "y_survivors": (list(range(len(theirs))), y), "max_depth": st["depth"], "illegal": st["illegal"],
           "logits": l_root, "nodes": _count(root), "forced": forced,
           "M_all": M.copy(), "prior": prior is not None, "opp_added": n_added, "offset": _offset(root),
           "merged": st["merged"], "timed_out": st["timed_out"], "sims_done": st["done"],
           "secs": round(time.time() - t_start, 3), "opp_self": st["opp_self"]}
    # (6b) the opponent head's logits (+ joint table) for callers: the halving prior's
    # ones when there is a prior, else the root world's
    if prior is not None and prior.get("opp_logits") is not None:
        res["opp_logits"], res["opp_table"] = prior["opp_logits"], prior.get("opp_table")
    else:
        res["opp_logits"] = ol[0]
        res["opp_table"] = None if oj is None else opp_table(ol[0], oj[0])
    if os.environ.get("MCTS_LOG"):
        import json
        lv = {}
        _levels(root, 0, lv)
        with open(os.environ["MCTS_LOG"], "a") as fh:
            fh.write(json.dumps({"turn": int(b0.turn), "sims": sims, "nodes": res["nodes"], "terminal": st["term"],
                                 "max_depth": st["depth"], "sims_done": st["done"], "timed_out": st["timed_out"],
                                 "secs": res["secs"], "focus": FOCUS, "trust_k": TRUST_K, "obs": OBS_MODE, "opp_self": st["opp_self"], "widen": WIDEN, "bayes": root.Sw is not None, "turn_levels": {str(k): v for k, v in sorted(lv.items())},
                                 "root_visits_top": float(root.C.sum(1).max() / max(root.C.sum(), 1)),
                                 **({} if prior is None else {
                                     "pi": [round(float(o[0]), 4) for o in ours], "x": [round(float(v), 3) for v in x],
                                     "q": [round(float(rows[i]), 3) for i in range(len(ours))],
                                     "prior_x": [round(float(prior["x"][a]), 3) for a in _sel_rows(prior)],
                                     "prior_q": [round(float(prior["q"].get(a, prior["value"])), 3) for a in _sel_rows(prior)],
                                     "prior_V": round(float(prior["value"]), 4), "V": round(V, 4),
                                     "offset": round(_offset(root), 4), "opp_added": n_added, "merged": st["merged"],
                                     "rows_visits": [int(v) for v in root.C.sum(1)]})}) + chr(10))
    return res


def node_value(node):
    """a node's current equilibrium value (its means against both sides' current strategies)"""
    if len(node.ours) == 1 and len(node.theirs) == 1:
        return float(node.matrix()[0, 0])
    node.mixes(0)                                    # re-solves when stale
    M = node.matrix()
    if len(node.x) != M.shape[0] or len(node.y) != M.shape[1]:
        node._solve(M.shape[0], M.shape[1])
    return float(node.x @ M @ node.y)


def trusted_value(node, v_net=None, k=None):
    """(TRUST_K) the node's value for its parent: K pseudo-visits at the value net's estimate + n at the node's equilibrium"""
    k = TRUST_K if k is None else k
    v_net = node.v0 if v_net is None else v_net
    n = node.n
    if n <= 0:
        return float(v_net)
    return (k * float(v_net) + n * node_value(node)) / (k + n)


def _levels(node, d, lv):
    """nodes per number of turns played below the root, weighted by visits: {turns: [nodes, visits]}"""
    e = lv.setdefault(node.turns, [0, 0])
    e[0] += 1
    e[1] += node.n
    for k in node.kids.values():
        _levels(k, d + 1, lv)


def _count(node):
    return 1 + sum(_count(k) for k in node.kids.values())
