"""
The playing agent: a trained network + play-time search, deciding from a poke-env battle object.

    agent = Agent()                          # weights: AGENT_CKPT (not part of this repository)
    order = agent.choose_move(battle)        # poke-env DoubleBattle -> DoubleBattleOrder
    team = agent.teampreview(battle)         # '/team 3142'


Each decision: the battle is rebuilt in the engine (bridge.Bridge.build_engine: what a player can see, the rest from the
real-set library and spread inference), encoded exactly as in training (vgcsim.encode_battle), and then
  - no search (AGENT_SEARCH=""): the network's best legal action per slot;
  - a halving search (sh_search.py presets fast / standard / strong / xstrong): our policy's top joint actions x the
    opponent-prediction head's top replies, played out in the engine over sampled opponent-set worlds, value-net leaves,
    sequential halving, a solved simultaneous-move game;
  - "nessie" / "nessie<S>": the xstrong halving root, then best-first deepening (ndeep.py) for S seconds;
  - "mcts" / "mcts<N>": the multi-turn tree search (mcts.py) seeded by the halving search;
  - optionally deep3.py (selective two-turn deepening), exact endgame solving (AGENT_ENDGAME, AGENT_EG_LEAVES).
Replacements after a faint / mid-turn switch-outs: every option x the opponent's replacements, each valued by a search.
Team preview: the preview head's top plans x the opponent's, valued by the value net on the turn-1 positions, solved.
Random choices (mixed-strategy draws) use per-turn deterministic RNG and sampling helpers, so repeated search / encode
calls within a turn see a consistent stream.
"""

import math
import os
import time
import traceback

import numpy as np
import torch
from poke_env.player.battle_order import DoubleBattleOrder, PassBattleOrder, SingleBattleOrder

import bridge
import paths
import vgcsim
from encoding import _fnv_u, to_id

# --------------------------------------------------------------------------------------------------------------------
# settings (environment variables)
# --------------------------------------------------------------------------------------------------------------------
CKPT = os.environ.get("AGENT_CKPT", os.path.join(paths.REPO, "weights", "net.pt"))
# 1 = closed team sheets: the network sees only what the battle revealed about the opponent; 0 = the rebuilt position as is
HIDDEN = int(os.environ.get("AGENT_HIDDEN", "1"))
# search preset: "" (network only), fast / standard / strong / xstrong (sh_search.PRESETS), auto, nessie[S], mcts[N]
SEARCH = os.environ.get("AGENT_SEARCH", "fast")
# how a searched move is picked: mixed = drawn from the solved equilibrium (the default), max = the top move of the prior
# score below, prior = the same score's argmax, sample = drawn from softmax(prior score)
SEARCH_MODE = os.environ.get("AGENT_SEARCH_MODE", "mixed")
SEARCH_BETA = float(os.environ.get("AGENT_SEARCH_BETA", "10"))     # prior score = gamma log pi(a) + beta (q(a) - V)
SEARCH_GAMMA = float(os.environ.get("AGENT_SEARCH_GAMMA", "1"))
SET_WORLDS = int(os.environ.get("AGENT_SET_WORLDS", "8"))          # closed sheets: opponent sets sampled per search world
BELIEF_WORLDS = os.environ.get("AGENT_BELIEF_WORLDS", "1") == "1"   # sets weighted by the belief head (nets that have one)
SWITCH_SEARCH = os.environ.get("AGENT_SWITCH_SEARCH", "1") == "1"   # search replacements too
SWITCH_BETA = os.environ.get("AGENT_SWITCH_BETA", "mixed")         # replacements: mixed / max / a beta for the prior rule
SWITCH_BUDGET = float(os.environ.get("AGENT_SWITCH_BUDGET", "28"))  # s after which replacement cells use the value net
OPENING_PRESET = os.environ.get("AGENT_OPENING_PRESET", "")        # a different preset for the first OPENING_TURNS turns
OPENING_TURNS = int(os.environ.get("AGENT_OPENING_TURNS", "2"))
DEEP2 = os.environ.get("AGENT_DEEP2", "0") == "1"                  # two-turn search on the survivors (deep2.py)
DEEP2_DICE = int(os.environ.get("AGENT_DEEP2_DICE", "4"))
DEEP2_K = int(os.environ.get("AGENT_DEEP2_K", "4"))
DEEP2_W = int(os.environ.get("AGENT_DEEP2_W", "1"))
DEEP3 = int(os.environ.get("AGENT_DEEP3", "0"))                    # leaves deepened by deep3.py (with SH_CHANCE=1); 0 = off
# Nessie search: the halving search (AGENT_NESSIE_BASE) then ndeep.deepen for S seconds, stopped AGENT_NESSIE_END seconds
# after the decision started at the latest
NESSIE_BASE = os.environ.get("AGENT_NESSIE_BASE", "xstrong")
NESSIE_SECONDS = float(os.environ.get("AGENT_NESSIE_SECONDS", "4"))
NESSIE_END = float(os.environ.get("AGENT_NESSIE_END", "38"))
# MCTS mode: depth in turns, root candidates (ours, theirs), the halving preset that seeds the root, cover visits, time cap
MCTS_DEPTH = int(os.environ.get("AGENT_MCTS_DEPTH", "3"))
MCTS_K = tuple(int(x) for x in os.environ.get("AGENT_MCTS_K", "12,12").split(","))
MCTS_BASE = os.environ.get("AGENT_MCTS_BASE", "xstrong")
MCTS_COVER = int(os.environ.get("AGENT_MCTS_COVER", "2"))
MCTS_END = float(os.environ.get("AGENT_MCTS_END", "38"))
SWITCH_MCTS = os.environ.get("AGENT_SWITCH_MCTS", "1") != "0"
SWITCH_MCTS_DEADLINE = float(os.environ.get("AGENT_SWITCH_MCTS_DEADLINE", "34"))
SWITCH_MCTS_WORLDS = os.environ.get("AGENT_SWITCH_MCTS_WORLDS", "1") != "0"
# exact endgames. AGENT_EG_LEAVES=<node budget>: search leaves with at most EG_LEAVES_MAX_MONS Pokemon per side are solved
# by the engine (certified values replace the value net). AGENT_ENDGAME=1: when both sides have at most ENDGAME_MAX_MONS
# left, the root itself is solved exactly (eg_rescore.live_solve) and the search is skipped when it certifies.
EG_LEAVES_BUDGET = int(os.environ.get("AGENT_EG_LEAVES", "0"))
EG_LEAVES_END = float(os.environ.get("AGENT_EG_LEAVES_END", "30"))
EG_LEAVES_MAX_MONS = int(os.environ.get("AGENT_EG_LEAVES_MAX_MONS", "2"))
EG_LEAVES_MAX_TOTAL = int(os.environ.get("AGENT_EG_LEAVES_MAX_TOTAL", "0"))
ENDGAME = os.environ.get("AGENT_ENDGAME", "0") == "1"
ENDGAME_MAX_MONS = int(os.environ.get("AGENT_ENDGAME_MAX_MONS", "1"))
ENDGAME_BUDGET = int(os.environ.get("AGENT_ENDGAME_BUDGET", "300000"))
ENDGAME_DEPTH = int(os.environ.get("AGENT_ENDGAME_DEPTH", "4"))
ENDGAME_TOL = float(os.environ.get("AGENT_ENDGAME_TOL", "0.02"))
ENDGAME_END = float(os.environ.get("AGENT_ENDGAME_END", "20"))
ENDGAME_LEAVES = int(os.environ.get("AGENT_ENDGAME_LEAVES", "64"))
ENDGAME_WORLDS = int(os.environ.get("AGENT_ENDGAME_WORLDS", "1"))   # the solver assumes known sets: world 0 only
# pooled search (several independent halving searches pooled; sh_search.pooled_search)
SEARCH_RUNS = int(os.environ.get("AGENT_SEARCH_RUNS", "1"))
SEARCH_SMOOTH = float(os.environ.get("AGENT_SEARCH_SMOOTH", "0"))
SEARCH_OPPEQ = float(os.environ["AGENT_SEARCH_OPPEQ"]) if os.environ.get("AGENT_SEARCH_OPPEQ") else None
# team preview: "" = fixed '/team 1234', "net" = the preview head's best plan, "search" = the preview search
PREVIEW = os.environ.get("AGENT_PREVIEW", "search")
PREVIEW_MODE = os.environ.get("AGENT_PREVIEW_MODE", "mixed")       # mixed (equilibrium draw) / max / prior
PREVIEW_K = int(os.environ.get("AGENT_PREVIEW_K", "12"))
PREVIEW_WORLDS = int(os.environ.get("AGENT_PREVIEW_WORLDS", "8"))
PREVIEW_BETA = float(os.environ.get("AGENT_PREVIEW_BETA", "10"))
# pick guards (value scale -1..1): a candidate more than QCAP below the best searched value is never picked unless the
# policy puts QCAP_PI of its mass on it and it is within QCAP_HARD; in near-decided positions a candidate whose chance to
# lose is more than QRATIO times the best's (and at least QRATIO_GAP below it) is out; values above WIN_CAP count as equal
QCAP = float(os.environ.get("AGENT_QCAP", "0.06"))
QCAP_PI = float(os.environ.get("AGENT_QCAP_PI", "0.6"))
QCAP_HARD = float(os.environ.get("AGENT_QCAP_HARD", "0.12"))
QRATIO = float(os.environ.get("AGENT_QRATIO", "3"))
QRATIO_GAP = float(os.environ.get("AGENT_QRATIO_GAP", "0.02"))
WIN_CAP = float(os.environ.get("AGENT_WIN_CAP", "0.96"))
GREEDY = int(os.environ.get("AGENT_GREEDY", "1"))                  # network-only play: best action (1) or a draw (0)
N_ACT = 38

_NET = {"net": None, "path": None}
_PV = {"dex": None, "options": None}
_AUTO = None
_DEX_IDS = None
_T0 = [0.0]                                         # start of the current decision


def _dex_ids():
    """name id -> the network's id (dex.json order + 1, 0 = none) per kind"""
    global _DEX_IDS
    if _DEX_IDS is None:
        import json
        d = json.load(open(paths.DEX, encoding="utf-8"))
        _DEX_IDS = {k: {x: i + 1 for i, x in enumerate(d[k])} for k in ("species", "abilities", "items", "moves")}
    return _DEX_IDS


def _preset(name):
    global _AUTO
    if name != "auto":
        return name
    if _AUTO is None:
        import sh_search
        _AUTO = sh_search.AutoPreset(float(os.environ.get("AGENT_TIME", "3")))
    return _AUTO.pick()


def _halving_preset(name):
    """the halving preset for searches the tree modes don't cover (replacement cells)"""
    if name.startswith("nessie"):
        return NESSIE_BASE
    return (MCTS_BASE if MCTS_BASE != "none" else "fast") if name.startswith("mcts") else _preset(name)


def _turn_state(battle):
    """Per-turn state string for deterministic search and sampling."""
    return bridge.turn_state_key(battle)


def _seed(battle=None, salt=0):
    """search seed for this decision, from the per-turn state string"""
    if battle is None:
        return int(np.random.randint(1 << 30))
    u = _fnv_u(f"{_turn_state(battle)}|seed:{salt}")
    return int(u * ((1 << 30) - 1)) + 1


# --------------------------------------------------------------------------------------------------------------------
# picking
# --------------------------------------------------------------------------------------------------------------------
def wcap(q):
    return min(q, WIN_CAP) if WIN_CAP > 0 else q


def cap_q(scores, q, pi=None):
    """scores with the candidates the guards rule out masked (-inf); see QCAP / QRATIO"""
    s = np.asarray(scores, np.float64).copy()
    if q is None or QCAP <= 0 or len(s) < 2:
        return s
    qa = np.asarray(q, np.float64)
    if len(qa) != len(s) or not np.all(np.isfinite(qa)):
        return s
    ok = qa >= qa.max() - QCAP
    if pi is not None and len(pi) == len(s):
        pa = np.asarray(pi, np.float64)
        ok |= (pa >= QCAP_PI) & ((QCAP_HARD < 0) | (qa >= qa.max() - QCAP_HARD))
    if QRATIO > 0:
        b = qa.max()
        lose, win = (1 - qa) / 2, (1 + qa) / 2
        bad = (qa <= b - QRATIO_GAP) & ((lose > QRATIO * max((1 - b) / 2, 1e-4)) | (win * QRATIO < (1 + b) / 2))
        ok &= ~bad
    return np.where(ok, s, -np.inf)


def prior_scores(r, beta=SEARCH_BETA):
    V = r["value"]
    return [SEARCH_GAMMA * math.log(max(o[0], 1e-12)) + beta * (wcap(r["q"].get(k, V)) - wcap(V)) for k, o in enumerate(r["ours"])]


def _slot_key(eb, slot, tup, side=0):
    st = eb.state()["sides"][side]
    mons, act = st["mons"], st["active"]
    a = act[slot] if slot < len(act) else None
    ok = a is not None and 0 <= a < len(mons)
    if tup[0] == "move":
        mv = mons[a]["moves"][tup[1]][0] if ok and tup[1] < len(mons[a]["moves"]) else str(tup[1])
        return f"slot{slot}:move:{to_id(mv)}"
    if tup[0] == "switch":
        sp = mons[tup[1]]["species"] if 0 <= tup[1] < len(mons) else str(tup[1])
        return f"slot{slot}:switch:{to_id(sp)}"
    return f"slot{slot}:{tup[0]}"


def _gumbel_pick(state, key_probs):
    best, best_k = -math.inf, None
    for k, p in key_probs.items():
        if p and p > 0 and math.isfinite(p):
            s = math.log(_fnv_u(f"{state}|{k}")) / p
            if s > best:
                best, best_k = s, k
    return best_k


def _keyed_draw(state, keys, probs):
    """index drawn from probs: the key's marginal first, then a tiebreak among the indices sharing that key"""
    w = np.clip(np.asarray(probs, np.float64), 0.0, None)
    if len(w) == 0:
        return 0
    if w.sum() <= 0 or not np.isfinite(w.sum()):
        return int(np.argmax(probs))
    w = w / w.sum()
    pk = {}
    for k, p in zip(keys, w):
        pk[k] = pk.get(k, 0.0) + float(p)
    win = _gumbel_pick(state, pk)
    idx = [i for i, k in enumerate(keys) if k == win and w[i] > 0] or list(range(len(keys)))
    if len(idx) == 1:
        return idx[0]
    wt = sum(float(w[i]) for i in idx) or 1.0
    win_t = _gumbel_pick(state, {f"tgt:{i}": float(w[i]) / wt for i in idx})
    return int(win_t.split(":")[1]) if win_t else idx[0]


def _joint_draw(battle, eb, cands, probs):
    """Draw a joint action index from probs via slot-0 marginal then slot-1 conditional Gumbel-max."""
    w = np.clip(np.asarray(probs, np.float64), 0.0, None)
    if len(w) == 0:
        return 0
    if w.sum() <= 0 or not np.isfinite(w.sum()):
        return int(np.argmax(probs))
    w = w / w.sum()
    state = _turn_state(battle)
    k0s = [_slot_key(eb, 0, c[0]) for c in cands]
    k1s = [_slot_key(eb, 1, c[1]) for c in cands]
    p0 = {}
    for k0, p in zip(k0s, w):
        p0[k0] = p0.get(k0, 0.0) + float(p)
    win0 = _gumbel_pick(state, p0)
    idx0 = [i for i, k0 in enumerate(k0s) if k0 == win0 and w[i] > 0] or list(range(len(cands)))
    w0 = sum(float(w[i]) for i in idx0) or 1.0
    p1 = {}
    for i in idx0:
        p1[k1s[i]] = p1.get(k1s[i], 0.0) + float(w[i]) / w0
    win1 = _gumbel_pick(state, p1)
    idx1 = [i for i in idx0 if k1s[i] == win1 and w[i] > 0] or idx0
    if len(idx1) == 1:
        return idx1[0]
    w1 = sum(float(w[i]) for i in idx1) or 1.0
    pt = {f"tgt:{i}": float(w[i]) / w1 for i in idx1}
    win_t = _gumbel_pick(state, pt)
    return int(win_t.split(":")[1]) if win_t else idx1[0]


def _order_key(slot, order):
    """Canonical key of a poke-env order for turn-local sampling."""
    o = getattr(order, "order", None)
    if hasattr(o, "id") and not hasattr(o, "species"):
        return f"slot{slot}:move:{to_id(o.id)}"
    if hasattr(o, "species"):
        return f"slot{slot}:switch:{to_id(o.species)}"
    return f"slot{slot}:pass"


def _preview_draw(battle, opts, ours_sp, probs):
    """index into opts ((four, leads) pairs) drawn from probs: the lead pair's marginal first, then the back pair"""
    w = np.clip(np.asarray(probs, np.float64), 0.0, None)
    if len(w) == 0:
        return 0
    if w.sum() <= 0 or not np.isfinite(w.sum()):
        return int(np.argmax(probs))
    w = w / w.sum()
    state = _turn_state(battle)
    lead_keys = ["lead:" + "+".join(sorted(ours_sp[j] for j in leads)) for four, leads in opts]
    back_keys = ["back:" + "+".join(sorted(ours_sp[j] for j in four if j not in leads)) for four, leads in opts]
    p_lead = {}
    for lk, p in zip(lead_keys, w):
        p_lead[lk] = p_lead.get(lk, 0.0) + float(p)
    win_lead = _gumbel_pick(state, p_lead)
    idx_l = [j for j, lk in enumerate(lead_keys) if lk == win_lead and w[j] > 0] or list(range(len(opts)))
    wl = sum(float(w[j]) for j in idx_l) or 1.0
    win_b = _gumbel_pick(state, {f"{back_keys[j]}:{j}": float(w[j]) / wl for j in idx_l})
    return int(win_b.split(":")[-1]) if win_b else idx_l[0]


def _soft(sc):
    s = np.asarray(sc, np.float64)
    return np.where(np.isfinite(s), np.exp(s - s[np.isfinite(s)].max()), 0.0)


def pick_from_search(battle, r, eb, mode=None):
    """our joint action (index into r['ours']) from a search result"""
    import sh_search
    mode = mode or SEARCH_MODE
    cands = [o[1:] for o in r["ours"]]
    qs = [r["q"].get(k, r["value"]) for k in range(len(r["ours"]))]
    pi = [o[0] for o in r["ours"]]
    if mode == "q":
        return sh_search.choose_q(r)
    if mode == "mixed":
        x = np.asarray(r["x"], np.float64)
        if len(x) == len(cands):
            x_ok = np.where(np.isfinite(cap_q(x, qs, pi)), x, 0.0)
            if x_ok.sum() > 0:
                x = x_ok
        return _joint_draw(battle, eb, cands, x)
    sc = cap_q(prior_scores(r), qs, pi)
    sc = np.array([x if k in r["q"] else -np.inf for k, x in enumerate(sc)], np.float64)   # never pick an unvalued candidate
    if not np.isfinite(np.max(sc)):
        return sh_search.choose(r, "prior", SEARCH_BETA)
    if mode == "sample":
        return _joint_draw(battle, eb, cands, _soft(sc))
    return int(np.argmax(sc))                                        # prior / max


def endgame_r(res, net, worlds, hidden):
    """eg_rescore.live_solve's result -> a search-result dict (the shape halving_search returns)"""
    import nnsearch
    wl = list(worlds) if isinstance(worlds, (list, tuple)) else [worlds]
    ours, theirs0, x, Y, V, rows = res["ours"], res["theirs0"], res["x"], res["Y"], res["value"], res["rows"]
    dev = next(net.parameters()).device
    lg, _ = nnsearch.net_logits(net, wl[0], [0], dev, hidden=hidden)
    pmap = {(tuple(a0), tuple(a1)): p for p, a0, a1 in wl[0].top_joint(0, np.ascontiguousarray(lg[0], np.float32).tobytes(), 1000)}
    pri = [float(pmap.get(o, 1e-6)) for o in ours]
    m0 = len(theirs0)
    y0 = np.asarray(Y[0][:m0], np.float64)
    y0 = y0 / y0.sum() if y0.sum() > 0 else np.full(m0, 1.0 / m0)
    q = {k: float(rows[k]) for k in range(len(ours))}
    return {"ours": [(pri[k], o[0], o[1]) for k, o in enumerate(ours)], "x": np.asarray(x), "q": q, "value": float(V),
            "survivors": [k for k in range(len(ours)) if x[k] > 1e-3] or list(range(len(ours))),
            "theirs": [(0.0, t[0], t[1]) for t in theirs0], "y_survivors": (list(range(m0)), y0),
            "endgame": {"worlds": len(wl), "info": res["info"]}}


# --------------------------------------------------------------------------------------------------------------------
# the network
# --------------------------------------------------------------------------------------------------------------------
# game memory (nets with memory: the last 16 decisions of this game as compact frames after the base columns). Each battle
# keeps its frames (battle._v3_mem, newest first); MemPad appends them to every base-width call: "root" (frames before this
# decision) for root positions, "leaf" (this decision's frame + those) for search positions one decision ahead.
_MEMCTX = {"root": None, "leaf": None, "mode": "leaf"}


class MemPad:
    def __init__(self, net, base_w):
        self._n, self._w = net, base_w

    def __call__(self, obs_f, obs_i, *a, **kw):
        blk = _MEMCTX[_MEMCTX["mode"]]
        if blk is not None and obs_f.shape[1] == self._w:
            obs_f = torch.cat([obs_f, blk.to(obs_f.device, obs_f.dtype).expand(obs_f.shape[0], -1)], 1)
        return self._n(obs_f, obs_i, *a, **kw)

    def __getattr__(self, k):
        return getattr(self._n, k)


def _root_mode(fn):
    def w(*a, **kw):
        _MEMCTX["mode"] = "root"
        try:
            return fn(*a, **kw)
        finally:
            _MEMCTX["mode"] = "leaf"
    w._memwrapped = True
    return w


def _install_root_mode():
    import sh_search
    if not getattr(sh_search.root_full, "_memwrapped", False):
        sh_search.root_full = _root_mode(sh_search.root_full)


def _mem_block(frames):
    from encoding import MEM_K, MEM_F
    out = np.zeros((MEM_K, MEM_F), np.float32)
    for j, fr in enumerate(frames[:MEM_K]):
        out[j] = fr
    return torch.from_numpy(out.reshape(1, -1))


def load_net(path=None):
    path = path or CKPT
    if _NET["net"] is not None and _NET["path"] == path:
        return _NET["net"]
    from netlib import AMP, arch_of, build_net, load_weights
    torch.set_num_threads(1)
    ck = torch.load(path, map_location="cpu", weights_only=False)
    sd = ck["net"] if isinstance(ck, dict) and "net" in ck else ck
    vocab = {"species": sd["e_species.weight"].shape[0], "ability": sd["e_ability.weight"].shape[0],
             "item": sd["e_item.weight"].shape[0], "move": sd["e_move.weight"].shape[0]}
    arch = arch_of(sd)
    net = build_net(vocab, N_ACT, arch)               # kind, input size and width read from the weights
    if arch.get("tokens") == 13:
        # v3-era nets were trained with the open-sheet gate (unseen brought foes hidden even with open sheets) and
        # state-dependent base powers in the damage features (AGENT_V3_FLAGS=0 leaves the engine's settings alone)
        if os.environ.get("AGENT_V3_FLAGS", "1") == "1":
            vgcsim.set_open_gate(True)
            vgcsim.set_dmg_state(True)
    if "tok_unc_foe.weight" in sd or "bel_in.weight" in sd:
        import encoding                              # the uncertainty block / library-guess beliefs need the set library
        encoding.ensure_unc_library()
    load_weights(net, sd)
    net.eval()
    if torch.cuda.is_available() and "cpu" not in (os.environ.get("AGENT_DEVICE", ""), os.environ.get("VGC_DEVICE", "")):
        net = net.cuda()
        AMP["on"] = True                             # bf16 autocast only on GPUs with native bf16
        import fastinfer                             # small calls replayed from CUDA graphs: identical outputs
        net = fastinfer.wrap(net)
    if arch.get("memory"):
        from encoding import MEM0
        net = MemPad(net, MEM0)
        _install_root_mode()
    _NET["net"], _NET["path"] = net, path
    return net


# --------------------------------------------------------------------------------------------------------------------
# poke-env helpers
# --------------------------------------------------------------------------------------------------------------------
def target_code(t, user=None):
    """Showdown target -> the action encoding's target code (a move aimed at the user itself = 0, like the engine)"""
    if t is None or t == 0:
        return 0
    if user is not None and t == -(user + 1):
        return 0
    return int(t) if t > 0 else 3


def mark_revealed(eb, theirs, their_index):
    """what we know about each opponent: moves seen (build_engine puts them first), item, ability, seen on the field"""
    for f in theirs:
        if id(f) not in their_index:
            continue
        n_known = len([m for m in f.moves if m not in ("struggle", "recharge")][:4])
        bits = (1 << n_known) - 1
        item = getattr(f, "item", None)
        if item not in (None, "unknown_item"):
            bits |= 1 << 4
        if f.revealed or f.active or f.fainted:
            bits |= 1 << 5
        if f.ability:
            bits |= 1 << 6
        eb.set_mon(1, their_index[id(f)], revealed=bits)


def _fix_locked_targets(battle, order):
    """a Pokemon locked into Outrage & co. gets a request move entry without 'target'; Showdown still wants one"""
    try:
        act = (getattr(battle, "_last_request", None) or {}).get("active") or []
        foes = [k + 1 for k, f in enumerate(battle.opponent_active_pokemon) if f is not None and not f.fainted] or [1]
        for slot, o in enumerate((getattr(order, "first_order", None), getattr(order, "second_order", None))):
            mv = getattr(o, "order", None)
            if mv is None or not hasattr(mv, "id") or getattr(o, "move_target", 0) or slot >= len(act):
                continue
            ent = next((m for m in act[slot].get("moves", []) if m.get("id") == mv.id), None)
            if ent is not None and "target" not in ent and mv.id not in ("recharge", "struggle"):
                o.move_target = foes[0]
            elif ent is not None and ent.get("target") in ("normal", "any", "adjacentFoe"):
                o.move_target = foes[0]
    except Exception:
        pass


def _request_target(battle, slot, move_id):
    try:
        act = (getattr(battle, "_last_request", None) or {}).get("active") or []
        ent = next((m for m in act[slot].get("moves", []) if m.get("id") == move_id), None)
        return ent.get("target") if ent else None
    except Exception:
        return None


def _record_choice(battle, order):
    """our chosen move + target per slot for this turn (battle._bridge_choices): the log never shows the target of a
    charging two-turn move, which the rebuilt position needs (log_vols.py)"""
    try:
        ch = {}
        for slot, o in zip("ab", (getattr(order, "first_order", None), getattr(order, "second_order", None))):
            mv = getattr(o, "order", None)
            if mv is not None and hasattr(mv, "id"):
                ch[slot] = (mv.id, getattr(o, "move_target", 0))
        if not hasattr(battle, "_bridge_choices"):
            battle._bridge_choices = {}
        battle._bridge_choices.setdefault(battle.turn, {}).update(ch)
    except Exception:
        pass


def _to_orders(options, eb_maps, cand, fs=None):
    """an engine joint action -> poke-env orders via the legal-action index; None when engine and poke-env disagree"""
    inv = [{v: k for k, v in mp.items()} for mp in eb_maps]
    orders = []
    for k, tup in enumerate(cand):
        if tup[0] == "pass" or (fs is not None and not (k < len(fs) and fs[k])):
            orders.append(None)
            continue
        a = inv[k].get(tuple(tup))
        if (a is None or a not in options[k]) and len(options[k]) == 1:
            a = next(iter(options[k]))          # one legal option (recharge, Struggle, a lock): play it
        if a is None or a not in options[k]:
            return None
        orders.append(options[k][a])
    if orders[0] is None and orders[1] is None:
        return None
    return DoubleBattleOrder(orders[0] or PassBattleOrder(), orders[1] or PassBattleOrder())


class Agent(bridge.Bridge):
    """The player: give it poke-env battle objects, get poke-env orders back."""
    LIBRARY_FILL = True

    def __init__(self, ckpt=None):
        super().__init__()
        self.ckpt = ckpt or CKPT
        self.crash_count = 0

    # ---------------------------------------------------------------------------------------------------------------
    def _mem_begin(self, battle):
        """game-memory context for this decision; returns this decision's frame (stored after the decision)"""
        _MEMCTX["root"] = _MEMCTX["leaf"] = None
        _MEMCTX["mode"] = "leaf"
        net = load_net(self.ckpt)
        if not isinstance(net, MemPad):
            return None
        from encoding import mem_frame
        eb = self.build_engine(battle)[0]
        f, _ = vgcsim.encode_battle(eb, 0, hidden=bool(HIDDEN))
        cur = mem_frame(np.asarray(f, np.float32)[None])[0]
        past = getattr(battle, "_v3_mem", [])
        _MEMCTX["root"] = _mem_block(past)
        _MEMCTX["leaf"] = _mem_block([cur] + past)
        return cur

    def choose_move(self, battle):
        self._memo = {}
        _T0[0] = time.time()
        cur = None
        try:
            try:
                cur = self._mem_begin(battle)
            except Exception:
                traceback.print_exc()
            res = self._nn_turn(battle)
            _fix_locked_targets(battle, res)
            _record_choice(battle, res)
            return res
        except Exception:
            traceback.print_exc()
            self.crash_count += 1
            return self._fallback_order(battle)
        finally:
            self._memo = None
            if cur is not None:
                battle._v3_mem = [cur] + getattr(battle, "_v3_mem", [])[:15]
            _MEMCTX["root"] = _MEMCTX["leaf"] = None

    def _fallback_order(self, battle):
        """a legal order without the network (only after an error): first available move / switch per slot"""
        orders = []
        forced = any(battle.force_switch)
        for k in range(2):
            mv = battle.available_moves[k] if k < len(battle.available_moves) else []
            sw = [p for p in (battle.available_switches[k] if k < len(battle.available_switches) else [])
                  if not any(getattr(o, "order", None) is p for o in orders if o is not None)]
            if forced and not (k < len(battle.force_switch) and battle.force_switch[k]):
                orders.append(None)
            elif mv and not forced:
                mon = battle.active_pokemon[k]
                t = (battle.get_possible_showdown_targets(mv[0], mon) or [0])[0] if mon is not None else 0
                orders.append(SingleBattleOrder(mv[0], move_target=t))
            elif sw:
                orders.append(SingleBattleOrder(sw[0]))
            else:
                orders.append(None)
        return DoubleBattleOrder(orders[0] or PassBattleOrder(), orders[1] or PassBattleOrder())

    def teampreview(self, battle):
        if PREVIEW in ("net", "search"):
            try:
                return self.preview_choice(battle, PREVIEW)
            except Exception:
                traceback.print_exc()
                self.crash_count += 1
        return "/team 1234"

    # ---------------------------------------------------------------------------------------------------------------
    # team preview
    # ---------------------------------------------------------------------------------------------------------------
    def _preview_texts(self, battle, rng):
        """our six as export text in request order; their six with sets from the open team sheet when there is one
        (battle._ots_sheet, set by the client after reading '|showteam|'), else from the set library (rng None: the
        most common set, else a sampled one)"""
        import setlib
        texts = []
        for m in self.brought(battle):
            pts, nature = bridge.infer_points(m)
            moves = [mid for mid in m.moves if mid not in ("struggle", "recharge")][:4]
            ab = m.ability or bridge.to_id((bridge.gen9().pokedex.get(m.species, {}).get("abilities", {}) or {}).get("0", ""))
            texts.append(bridge.mon_export(bridge.current_species(m), m.item if m.item != "unknown_item" else "", ab, moves,
                                            pts, nature))
        sheet = {}
        if getattr(battle, "_ots_sheet", False):
            for f in battle.opponent_team.values():
                if f.moves:
                    sheet[bridge.to_id(f.species)] = f
        theirs = []
        for p in battle.teampreview_opponent_team:
            sp = bridge.engine_species(bridge.current_species(p))
            ab_default = bridge.to_id((bridge.gen9().pokedex.get(p.species, {}).get("abilities", {}) or {}).get("0", ""))
            f = sheet.get(bridge.to_id(p.species))
            if f is not None:
                known = [mid for mid in f.moves if mid not in ("struggle", "recharge")][:4]
                item = f.item if f.item not in (None, "", "unknown_item") else None
                ab = f.ability or None
                lib = setlib.best_set(p.species, known, item, ab)
                pts = {k: (lib or {}).get("points", {}).get(k, 0) for k in ("hp", "atk", "def", "spa", "spd", "spe")}
                nat = (getattr(f, "_nature", None) or "").capitalize() or (lib or {}).get("nature", "Serious")
                theirs.append(bridge.mon_export(sp, item or "", ab or (lib or {}).get("ability") or ab_default,
                                                 known or ["protect"], pts, nat))
                continue
            lib = (setlib.best_set(p.species, [], None, None, rng=rng) if rng is not None else None) \
                or setlib.best_set(p.species, [], None, None)
            if lib is None:
                theirs.append(bridge.mon_export(sp, "", ab_default, ["protect"], {}, "Serious"))
                continue
            pts = {k: lib["points"].get(k, 0) for k in ("hp", "atk", "def", "spa", "spd", "spe")}
            theirs.append(bridge.mon_export(sp, lib["item"], lib["ability"], list(lib["moves"])[:4], pts, lib["nature"]))
        return "\n\n".join(texts) + "\n", "\n\n".join(theirs) + "\n"

    def preview_choice(self, battle, mode):
        """'/team ...' from the network's preview head (mode net) or the preview search (mode search): our top
        PREVIEW_K plans x their top PREVIEW_K (their head from their side) in PREVIEW_WORLDS opponent-set worlds, each
        cell = the value net on the turn-1 position, averaged over worlds, solved as a zero-sum game"""
        if _PV["dex"] is None:
            _PV["dex"] = vgcsim.Dex(paths.DEX)
            _PV["options"] = [(list(f), list(l)) for f, l in vgcsim.preview_options()]
        dex, options = _PV["dex"], _PV["options"]
        net = load_net(self.ckpt)
        dev = next(net.parameters()).device
        hidden = bool(HIDDEN)
        rng = np.random.default_rng(_seed(battle))
        worlds = []
        for w in range(PREVIEW_WORLDS if mode == "search" else 1):
            ours_txt, theirs_txt = self._preview_texts(battle, rng if w > 0 else None)
            worlds.append(vgcsim.Battle(dex, ours_txt, theirs_txt, seed=11 + w))
            worlds[-1].set_keep_foe_stats(True)        # their sets' stats are our belief: keep them

        def head(b, side):
            pf, pi, pm = vgcsim.encode_preview(b, side, hidden=hidden)
            with torch.no_grad():
                lg, _ = net.preview(torch.tensor(np.array([pf], np.float32), device=dev),
                                    torch.tensor(np.array([pi], np.int32), device=dev))
            lg = lg[0].float().cpu().numpy()
            lg[np.frombuffer(bytes(pm), np.uint8) == 0] = -1e9
            p = np.exp(lg - lg.max())
            return p / p.sum()

        ours_sp = [bridge.to_id(m.species) for m in self.brought(battle)]

        p_ours = head(worlds[0], 0)
        choice = int(np.argmax(p_ours))
        if mode == "search":
            import nnsearch
            p_theirs = head(worlds[0], 1)
            rows = [int(r) for r in np.argsort(-p_ours)[:PREVIEW_K] if p_ours[r] > 0]
            cols = [int(c) for c in np.argsort(-p_theirs) if p_theirs[c] > 0][:PREVIEW_K]

            def order(o):
                return list(options[o][1]) + [m for m in options[o][0] if m not in options[o][1]]
            F, I = [], []
            for b in worlds:
                for r in rows:
                    for c in cols:
                        b2 = b.copy()
                        b2.choose_team(0, order(r))
                        b2.choose_team(1, order(c))
                        f, i = vgcsim.encode_battle(b2, 0, hidden=hidden)
                        F.append(f)
                        I.append(i)
            F = torch.tensor(np.asarray(F, np.float32), device=dev)
            I = torch.tensor(np.asarray(I, np.int32), device=dev)
            with torch.no_grad():
                v = torch.cat([net(F[k:k + 4096], I[k:k + 4096])[1] for k in range(0, len(F), 4096)]).float().cpu().numpy()
            M = v.reshape(len(worlds), len(rows), len(cols)).mean(0)
            x, y = nnsearch.solve(M)
            q = M @ y
            if PREVIEW_MODE == "mixed":
                choice = rows[_preview_draw(battle, [options[r] for r in rows], ours_sp, x)]
            elif PREVIEW_MODE == "prior":
                V = float(np.asarray(x) @ q)
                choice = rows[int(np.argmax([math.log(max(p_ours[r], 1e-12)) + PREVIEW_BETA * (q[j] - V)
                                             for j, r in enumerate(rows)]))]
            else:
                choice = rows[int(np.argmax(q))]
        four, leads = options[choice]
        seq = list(leads) + [m for m in four if m not in leads]
        return "/team " + "".join(str(i + 1) for i in seq)

    # ---------------------------------------------------------------------------------------------------------------
    # move turns
    # ---------------------------------------------------------------------------------------------------------------
    def _nn_turn(self, battle):
        net = load_net(self.ckpt)
        obs_f, obs_i, mask, options = self.encode(battle)
        if SEARCH and not any(battle.force_switch):
            order = self._search_turn(battle, net, options)
            if order is not None:
                return order
        if SEARCH and SWITCH_SEARCH and any(battle.force_switch):
            try:
                order = self._search_switch(battle, net, options)
            except Exception:
                traceback.print_exc()
                order = None
            if order is not None:
                return order
        dev = next(net.parameters()).device
        _MEMCTX["mode"] = "root"
        try:
            with torch.no_grad():
                logits, _ = net(torch.from_numpy(np.asarray(obs_f, np.float32))[None].to(dev),
                                torch.from_numpy(np.asarray(obs_i, np.int32))[None].to(dev))[:2]
        finally:
            _MEMCTX["mode"] = "leaf"
        return self._pick(battle, logits.float().cpu()[0].numpy(), mask, options)

    def _set_worlds(self, battle, net, eb, extra_setup=None):
        """closed team sheets: the rebuilt position (world 0 = the best guess) + SET_WORLDS - 1 more, each with the
        opponent's unrevealed moves / item / ability drawn from the library among the sets consistent with what was
        revealed (weighted by the belief head when the net has one); extra_setup(position) -> position or None"""
        worlds = [eb]
        self._set_rng = np.random.default_rng(_seed(battle, 1))
        bel = self._belief_weights(net) if BELIEF_WORLDS else None
        if bel is not None:
            self._set_weight, self._species_weight = bel
        try:
            for _ in range(SET_WORLDS - 1):
                e2, _, _, their_index, theirs = self.build_engine(battle)[:5]
                mark_revealed(e2, theirs, their_index)
                if extra_setup is not None:
                    e2 = extra_setup(e2)
                    if e2 is None:
                        continue
                worlds.append(e2)
        finally:
            self._set_rng = self._set_weight = self._species_weight = None
        return worlds

    def _endgame(self, battle, net, worlds):
        """AGENT_ENDGAME: (search-result dict, our pick) from the exact endgame solve, or None (normal search)"""
        try:
            import eg_rescore
            if ENDGAME_WORLDS > 0 and isinstance(worlds, (list, tuple)):
                worlds = worlds[:ENDGAME_WORLDS]
            res, why = eg_rescore.live_solve(worlds, ENDGAME_BUDGET, ENDGAME_DEPTH, ENDGAME_TOL, ENDGAME_MAX_MONS,
                                             deadline=_T0[0] + ENDGAME_END, max_leaves=ENDGAME_LEAVES)
            if res is None:
                return None
            r = endgame_r(res, net, worlds, bool(HIDDEN))
            b0 = worlds[0] if isinstance(worlds, (list, tuple)) else worlds
            return r, pick_from_search(battle, r, b0)
        except Exception:
            traceback.print_exc()
            return None

    def _search_turn(self, battle, net, options):
        """search from the rebuilt position (self._eb, set by encode); the pick mapped back to poke-env orders"""
        eb = self._eb
        worlds = self._set_worlds(battle, net, eb) if (HIDDEN and self.LIBRARY_FILL and SET_WORLDS > 1) else eb
        preset = _preset(OPENING_PRESET if OPENING_PRESET and battle.turn <= OPENING_TURNS else SEARCH)
        eg_leaves = EG_LEAVES_BUDGET > 0 and hasattr(vgcsim, "eg_leaves_config")
        if eg_leaves:
            import eg_rescore
            import ndeep
            eg_rescore.ensure_opts()
            vgcsim.eg_leaves_config(budget=EG_LEAVES_BUDGET, seconds=max(0.05, _T0[0] + EG_LEAVES_END - time.time()),
                                    max_mons=EG_LEAVES_MAX_MONS, max_total=EG_LEAVES_MAX_TOTAL)
            ndeep.EG_LEAVES = True
        try:
            return self._search_turn_inner(battle, net, options, worlds, preset)
        finally:
            if eg_leaves:
                vgcsim.eg_leaves_config(budget=0)
                ndeep.EG_LEAVES = False

    def _search_turn_inner(self, battle, net, options, worlds, preset):
        import sh_search
        eb = self._eb
        b_root = worlds[0] if isinstance(worlds, (list, tuple)) else worlds
        dev = next(net.parameters()).device
        t_search = time.time()
        r_eg = self._endgame(battle, net, worlds) if ENDGAME else None
        if r_eg is not None:
            r, c = r_eg
        else:
            if preset.startswith("nessie"):
                import ndeep
                r = sh_search.halving_search(net, worlds, 0, dev, hidden=bool(HIDDEN), seed=_seed(battle, 2),
                                             **sh_search.PRESETS[NESSIE_BASE])
                try:
                    rd = ndeep.deepen(net, worlds, r, dev, hidden=bool(HIDDEN), seed=_seed(battle, 3),
                                      seconds=float(preset[6:] or NESSIE_SECONDS), deadline=_T0[0] + NESSIE_END)
                    r = dict(r, **rd)
                except Exception:
                    traceback.print_exc()                # the halving result is kept
            elif preset.startswith("mcts"):
                import mcts
                r_h = None
                if MCTS_BASE != "none":
                    r_h = sh_search.halving_search(net, worlds, 0, dev, hidden=bool(HIDDEN), seed=_seed(battle, 2),
                                                   **sh_search.PRESETS[MCTS_BASE])
                r = mcts.search(net, worlds, 0, dev, hidden=bool(HIDDEN), seed=_seed(battle, 4), sims=int(preset[4:] or 800),
                                depth=MCTS_DEPTH, k_root=MCTS_K, prior=r_h, root_cover=MCTS_COVER if r_h is not None else 0,
                                deadline=_T0[0] + MCTS_END)
            elif SEARCH_RUNS > 1 or SEARCH_SMOOTH > 0 or SEARCH_OPPEQ is not None:
                r = sh_search.pooled_search(net, worlds, 0, dev, hidden=bool(HIDDEN), seed=_seed(battle, 2), runs=SEARCH_RUNS,
                                            smooth=SEARCH_SMOOTH, opp_eq=SEARCH_OPPEQ, **sh_search.PRESETS[preset])
            else:
                r = sh_search.halving_search(net, worlds, 0, dev, hidden=bool(HIDDEN), seed=_seed(battle, 2), **sh_search.PRESETS[preset])
            if _AUTO is not None:
                _AUTO.record(preset, time.time() - t_search)
            c = pick_from_search(battle, r, b_root)
            if DEEP2 and len(r["survivors"]) > 1:
                import deep2
                d = deep2.deepen(net, worlds, r, dev, dice=DEEP2_DICE, k_in=DEEP2_K, w_in=DEEP2_W, seed=_seed(battle, 5),
                                 hidden=bool(HIDDEN))
                c = deep2.choose(r, d, "max" if SEARCH_MODE == "max" else "prior", SEARCH_BETA)
            if DEEP3 and len(r["survivors"]) > 1:
                import deep2
                import deep3
                d = deep3.deepen(net, worlds, r, dev, hidden=bool(HIDDEN), seed=_seed(battle, 5), budget=DEEP3)
                if SEARCH_MODE in ("max", "prior"):
                    c = deep2.choose(r, d, "max", SEARCH_BETA)
                else:
                    q = np.array([d["q"][a] for a in d["survivors"]])
                    V = d["value"]
                    sc = [SEARCH_GAMMA * math.log(max(r["ours"][a][0], 1e-12)) + SEARCH_BETA * (wcap(float(q[n])) - wcap(V))
                          for n, a in enumerate(d["survivors"])]
                    sc = cap_q(sc, q, [r["ours"][a][0] for a in d["survivors"]])
                    c = d["survivors"][_joint_draw(battle, b_root, [r["ours"][a][1:] for a in d["survivors"]], _soft(sc))]
        try:                                              # for a mid-turn switch later this turn (midturn.py)
            import midturn
            midturn.store_plan(battle, r, c, b_root)
        except Exception:
            traceback.print_exc()
        return _to_orders(options, eb.legal_index_map(0), r["ours"][c][1:])

    # ---------------------------------------------------------------------------------------------------------------
    # replacements
    # ---------------------------------------------------------------------------------------------------------------
    def _search_switch(self, battle, net, options):
        """a replacement after a faint / a mid-turn switch-out (U-turn, Eject Button...): every Pokemon we could send in
        x the opponent's simultaneous replacements, each valued by a search of the resulting position (terminal = +-1),
        solved; mid-turn switch-ins also see the rest of the turn (midturn.py). None: the network's own choice"""
        import bridge_util
        import nnsearch
        import sh_search
        eb = self._eb
        dev = next(net.parameters()).device
        hidden = bool(HIDDEN)
        fs = list(battle.force_switch)
        e = eb.copy()
        midturn_now = False
        revive = bool(getattr(battle, "reviving", False))
        if revive:
            # Revival Blessing: the user's slot picks a FAINTED Pokemon (the engine asks the same way)
            e.refresh()
            if not any(k < len(fs) and fs[k] and e.set_reviving(0, k) for k in (0, 1)):
                return None
        else:
            # fainted Pokemon put back on BOTH sides (both replace at once); a mid-turn switch-out is the engine's own
            # (Regenerator, Baton Pass, Shed Tail...), followed by the end of the turn
            setup = bridge_util.forced_setup(eb, fs, bridge_util.midturn_kinds(battle))
            if setup is None:
                return None
            e, midturn_now = setup[0], bool(setup[1])
        if not e.needs_choice(0):
            return None
        maps = e.legal_index_map(0)
        f, i = vgcsim.encode_battle(e, 0, hidden=hidden)
        with torch.no_grad():
            l0 = net(torch.tensor(np.array([f], np.float32), device=dev), torch.tensor(np.array([i], np.int32), device=dev))[0]
        ours = nnsearch.top_joint(l0[0].float().cpu().numpy(), maps, 8)
        if len(ours) < 2:
            return None                                          # one option: nothing to search
        PASS = ("pass", 0, 0, False)
        theirs = [(1.0, PASS, PASS)]
        if e.needs_choice(1):                                    # they replace at the same time
            theirs = nnsearch.top_joint(np.zeros((2, 38), np.float32), e.legal_index_map(1), 8)
        M = np.zeros((len(ours), len(theirs)))
        seed = _seed(battle, 6)
        for a_, (_, o0, o1) in enumerate(ours):
            for b_, (_, t0, t1) in enumerate(theirs):
                e2 = e.copy()
                e2.choose(0, tuple(o0), tuple(o1))
                if e.needs_choice(1):
                    e2.choose(1, tuple(t0), tuple(t1))
                e2.refresh()
                s2 = e2.state()
                if s2["phase"] == "ended":
                    M[a_, b_] = 1.0 if s2["winner"] == 0 else -1.0 if s2["winner"] == 1 else 0.0
                elif (s2["phase"] == "turn" and e2.needs_choice(0) and e2.needs_choice(1)
                      and time.time() - _T0[0] < SWITCH_BUDGET):
                    r = sh_search.halving_search(net, e2, 0, dev, hidden=hidden, seed=seed + 31 * a_ + b_,
                                                 **sh_search.PRESETS[_halving_preset(SEARCH)])
                    M[a_, b_] = float(r["value"])
                else:                                            # another replacement pending (or out of time): value net
                    ff, ii = vgcsim.encode_battle(e2, 0, hidden=hidden)
                    with torch.no_grad():
                        M[a_, b_] = float(net(torch.tensor(np.array([ff], np.float32), device=dev),
                                              torch.tensor(np.array([ii], np.int32), device=dev))[1].float().cpu()[0])
        if midturn_now:
            try:
                import midturn
                plan = midturn.plan_of(battle, e)
                if plan is not None:
                    M = M + midturn.delta(net, e, ours, theirs, plan, dev, hidden, seed)
            except Exception:
                traceback.print_exc()
        x, y = nnsearch.solve(M)
        rows = M @ y
        V = float(x @ M @ y)
        if (SWITCH_MCTS and SEARCH.startswith("mcts") and not midturn_now and not revive
                and time.time() - _T0[0] < SWITCH_MCTS_DEADLINE):
            try:
                import mcts
                pr = {"ours": ours, "theirs": theirs, "M_all": M, "x": np.asarray(x, np.float64),
                      "q": {a_: float(rows[a_]) for a_ in range(len(ours))}, "value": V,
                      "survivors": list(range(len(ours))), "y_survivors": (list(range(len(theirs))), y)}
                ws = [e]
                if SWITCH_MCTS_WORLDS and HIDDEN and self.LIBRARY_FILL and SET_WORLDS > 1:
                    kinds = bridge_util.midturn_kinds(battle)

                    def setup_(e2):
                        su = bridge_util.forced_setup(e2, fs, kinds)
                        return su[0] if su is not None and su[0].needs_choice(0) else None
                    ws = [e] + self._set_worlds(battle, net, eb, setup_)[1:]
                rm = mcts.search(net, ws, 0, dev, hidden=hidden, seed=seed + 7919, sims=int(SEARCH[4:] or 800),
                                 depth=MCTS_DEPTH, prior=pr, root_cover=MCTS_COVER, deadline=_T0[0] + MCTS_END)
                if [tuple(o[1:]) for o in rm["ours"]] == [tuple(o[1:]) for o in ours]:
                    x, y = np.asarray(rm["x"], np.float64), np.asarray(rm["y_survivors"][1], np.float64)
                    rows = np.asarray(rm["M_all"], np.float64) @ y
                    V = float(rm["value"])
            except Exception:
                traceback.print_exc()
        if SWITCH_BETA == "mixed":
            c = _joint_draw(battle, e, [o[1:] for o in ours], x)
        elif SWITCH_BETA == "max":
            c = int(np.argmax(rows))
        else:
            c = int(np.argmax([math.log(max(o[0], 1e-12)) + float(SWITCH_BETA) * (rows[k] - V) for k, o in enumerate(ours)]))
        return _to_orders(options, maps, ours[c][1:], fs)

    # ---------------------------------------------------------------------------------------------------------------
    def _belief_weights(self, net):
        """search worlds from the belief head (nets that have one): (set score, species score) for the world sampling.
        A library set's score = P(item) x P(ability) x prod over its unrevealed moves of min(1, 4 P(move)) from that foe
        row's belief; unseen foes' species: the mean species belief of the blank rows"""
        m = net
        while not hasattr(m, "aux_bel") and any(hasattr(m, a) for a in ("_n", "net", "model", "_orig_mod")):
            m = next(getattr(m, a) for a in ("_n", "net", "model", "_orig_mod") if hasattr(m, a))
        enc = getattr(self, "_enc", None)
        if not hasattr(m, "aux_bel") or enc is None:
            return None
        obs_f, obs_i, rows = enc
        dev = next(m.parameters()).device
        _MEMCTX["mode"] = "root"
        try:
            with torch.no_grad():
                bl = net(torch.as_tensor(np.asarray(obs_f, np.float32))[None].to(dev),
                         torch.as_tensor(np.asarray(obs_i, np.int32))[None].to(dev), belief=True)[-1][0].float()
        finally:
            _MEMCTX["mode"] = "leaf"
        ps = [torch.softmax(x, -1).cpu().numpy() for x in torch.split(bl, m.bel_sizes, -1)]   # species, ability, item, move
        ids = _dex_ids()
        row_of = {id(f): r for r, f in enumerate(rows)}
        blank = list(range(len(rows), 4))
        sp = ps[0][blank].mean(0) if blank else None

        def set_w(f, s):
            r = row_of.get(id(f))
            if r is None:
                return 1.0
            w = 1.0
            if not f.item or f.item == "unknown_item":
                w *= ps[2][r][ids["items"].get(bridge.to_id(s["item"]), 0)]
            if getattr(f, "_ability", None) is None:
                w *= ps[1][r][ids["abilities"].get(bridge.to_id(s["ability"]), 0)]
            for mv in s["moves"]:
                if mv not in f.moves:
                    w *= min(1.0, 4 * ps[3][r][ids["moves"].get(bridge.to_id(mv), 0)])
            return w

        def species_w(name):
            return 1.0 if sp is None else float(sp[ids["species"].get(bridge.to_id(name), 0)])
        return set_w, species_w

    def encode(self, battle):
        """the network's view of this decision: features (identical to training), the legal-action mask per slot, and
        the poke-env order behind each legal action index"""
        eb, index, our_slots, their_index, theirs = self.build_engine(battle)[:5]
        self._eb = eb
        req = getattr(battle, "_last_request", None) or {}
        brought = []
        for p in req.get("side", {}).get("pokemon", []):
            mon = battle.team.get(p.get("ident"))
            if mon is not None and id(mon) in index:
                brought.append(mon)
        if not brought:
            brought = [m for m in battle.team.values()][:4]
        pos_of = {id(m): k for k, m in enumerate(brought[:4])}
        ours_idx = [index[id(m)] for m in brought[:4]]
        seen = [f for f in theirs if id(f) in their_index and (f.revealed or f.active or f.fainted)][:4]
        foes_idx = [their_index[id(f)] for f in seen]
        if HIDDEN:
            mark_revealed(eb, theirs, their_index)
        obs_f, obs_i = vgcsim.encode_battle(eb, 0, ours_idx, foes_idx, hidden=bool(HIDDEN))
        self._enc = (obs_f, obs_i, seen)
        mask = np.zeros((2, N_ACT), np.uint8)
        options = [dict(), dict()]          # action index -> SingleBattleOrder
        can_mega = list(getattr(battle, "can_mega_evolve", [False, False]))
        forced = any(battle.force_switch)
        for k in range(2):
            mon = battle.active_pokemon[k] if k < len(battle.active_pokemon) else None
            if forced and not battle.force_switch[k]:
                continue
            if not forced and mon is not None:
                slots = our_slots[index[id(mon)]] if id(mon) in index else {}
                for move in battle.available_moves[k]:
                    if move.id in slots:
                        base = slots[move.id] * 4
                        targets = battle.get_possible_showdown_targets(move, mon) or [0]
                        if move.id == "curse" and _request_target(battle, k, "curse") == "normal" and targets == [0]:
                            # a Ghost by Trick-or-Treat: the request makes Curse a foe-targeting move
                            targets = [j + 1 for j, f in enumerate(battle.opponent_active_pokemon)
                                       if f is not None and not f.fainted] or [1]
                        for t in targets:
                            for mega in ([False, True] if k < len(can_mega) and can_mega[k] else [False]):
                                a = (base + target_code(t, k)) * 2 + int(mega)
                                options[k].setdefault(a, SingleBattleOrder(move, move_target=t, mega=mega))
                    else:
                        options[k].setdefault(36, SingleBattleOrder(move))   # Struggle / Recharge / unlisted
            for p in battle.available_switches[k] if k < len(battle.available_switches) else []:
                if id(p) in pos_of:
                    options[k].setdefault(32 + pos_of[id(p)], SingleBattleOrder(p))
            for a in options[k]:
                mask[k, a] = 1
        return obs_f, obs_i, mask, options

    def _pick(self, battle, logits, mask, options):
        """network-only play: the best (GREEDY) or a drawn legal action per slot"""
        orders = [None, None]
        used_switch, used_mega = None, False
        for k in range(2):
            lg = logits[k].copy()
            lg[mask[k] == 0] = -1e9
            if used_switch is not None:                  # the second slot can't switch to the same Pokemon
                lg[used_switch] = -1e9
            if used_mega:                                # or Mega Evolve again
                lg[1::2][:16] = -1e9
            if mask[k].sum() == 0 or lg.max() <= -1e8:
                continue
            if GREEDY:
                a = int(lg.argmax())
            else:
                legal = [j for j in range(len(lg)) if lg[j] > -1e8]
                a = legal[_keyed_draw(_turn_state(battle), [_order_key(k, options[k][j]) for j in legal], _soft(lg[legal]))]
            orders[k] = options[k][a]
            if 32 <= a < 36:
                used_switch = a
            if a < 32 and a % 2 == 1:
                used_mega = True
        if orders[0] is None and orders[1] is None:
            return self._fallback_order(battle)
        return DoubleBattleOrder(orders[0] or PassBattleOrder(), orders[1] or PassBattleOrder())
