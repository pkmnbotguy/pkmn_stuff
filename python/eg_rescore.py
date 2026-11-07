"""Endgame rescoring (2026-10-29, chessprogramming idea 4 = Leela's tablebase rescoring; a v3 option, OFF by default).

Leela replaces the game result of positions found in an endgame tablebase with the tablebase's exact result, so the value
net learns those positions without the noise of whatever happened afterwards. Our analog: tiny endgames (<= max_mons
Pokemon left per side, default 1) are solved EXACTLY with no network at all:
  - every legal joint action of both sides (Battle.top_joint with flat logits = the network's whole action space),
  - every chance outcome with its exact probability (Battle.chance_leaf_battles, min_path 0; a cell that hits the leaf
    cap or whose probabilities don't sum to 1 makes the solve give up),
  - replacements: every (our x their) option pair (Battle.replacement_children),
  - each decision point solved as a simultaneous-move game (vgcsim.rm_solve), value = x M y.
(2026-10-29) Battle.exact_value (lib.rs eg_exact) is the same algorithm in Rust (identical values on 121 positions, GIL
released); exact_value() uses it when the module has it (rust=False = this Python version, the reference).
A solve that meets an undecided line at max_depth turns, or runs past the node budget, returns None (no rescoring): only
fully decided values are used, as with a real tablebase. Values are side 0's expected result (+1 win, -1 loss, 0 tie).

    v = eg_rescore.exact_value(battle)              # float or None
    v, info = eg_rescore.exact_value(battle, info=True)

"""

import os
import time
import numpy as np
import vgcsim

ZEROS = np.zeros((2, 38), np.float32).tobytes()


class _Abort(Exception):
    pass


def n_alive(battle):
    """(side 0, side 1) Pokemon not fainted"""
    st = battle.state()
    return tuple(sum(1 for m in st["sides"][s]["mons"] if not m["fainted"]) for s in (0, 1))


def qualifies(battle, max_mons=1):
    if battle.winner is not None or battle.phase not in ("turn", "switch"):
        return False
    a, b = n_alive(battle)
    return 0 < a <= max_mons and 0 < b <= max_mons


def _game_value(M, iters):
    M = np.asarray(M, np.float64)
    if M.shape[0] == 1 and M.shape[1] == 1:
        return float(M[0, 0])
    if M.shape[0] == 1:
        return float(M[0].min())
    if M.shape[1] == 1:
        return float(M[:, 0].max())
    x, y = vgcsim.rm_solve(M.tolist(), iters)
    return float(np.asarray(x) @ M @ np.asarray(y))


def exact_value(battle, budget=20000, max_depth=4, max_leaves=64, iters=2000, seed=1, tol=0.02, info=False, rust=True):
    """side 0's expected result, certified within +-tol, or None (see the module docstring).
    Lines still undecided at the depth limit count as -1 for a LOWER and +1 for an UPPER bound (a game's value is monotone
    in its payoffs, so both bounds hold); iterative deepening 1..max_depth turns until upper - lower <= 2 tol, then the
    midpoint. Protect-vs-Protect lines never end, so a strict "every line decided" rule would solve almost nothing."""
    t0 = time.time()
    if rust and hasattr(battle, "exact_value"):   # (2026-10-29) the same algorithm in Rust (lib.rs eg_exact), GIL released
        v, n, why, gap, dep = battle.exact_value(budget, max_depth, max_leaves, iters, seed, tol)
        if info:
            return v, {"nodes": n, "secs": time.time() - t0, "why": why, "gap": gap, "depth": dep}
        return v
    nodes = [0]

    def spend(n):
        nodes[0] += n
        if nodes[0] > budget:
            raise _Abort("budget")

    def pair(Ms, iters):
        lo = _game_value([[c[0] for c in row] for row in Ms], iters)
        hi = _game_value([[c[1] for c in row] for row in Ms], iters)
        return lo, hi

    def val(c, d, depth):
        w = c.winner
        if w is not None:
            v = 1.0 if w == 0 else (-1.0 if w == 1 else 0.0)
            return v, v
        ph = c.phase
        if ph == "switch":
            na, nb, ch = c.replacement_children(64)
            spend(len(ch))
            return pair([[val(ch[a * nb + b], d, depth) for b in range(nb)] for a in range(na)], iters)
        if ph != "turn":
            raise _Abort("phase " + ph)
        if d >= depth:
            return -1.0, 1.0
        ours = [(a0, a1) for _, a0, a1 in c.top_joint(0, ZEROS, 1000)]
        theirs = [(a0, a1) for _, a0, a1 in c.top_joint(1, ZEROS, 1000)]
        cells = [(i, j, 0) for i in range(len(ours)) for j in range(len(theirs))]
        spend(len(cells))
        leaves = c.chance_leaf_battles(0, ours, theirs, cells, 1, seed, 0.0, max_leaves, False, False)
        M = [[None] * len(theirs) for _ in ours]
        for (i, j, _), lv in zip(cells, leaves):
            if len(lv) >= max_leaves:
                raise _Abort("leaf cap")
            tot = sum(p for _, p in lv)
            if abs(tot - 1.0) > 1e-3:
                raise _Abort(f"probabilities sum to {tot:.4f}")
            spend(len(lv))
            lo = hi = 0.0
            for leaf, p in lv:
                l, h = val(leaf, d + 1, depth)
                lo += p * l
                hi += p * h
            M[i][j] = (lo / tot, hi / tot)
        return pair(M, iters)

    v, why, gap, dep = None, "depth", 2.0, 0
    try:
        b = battle.copy()
        try:
            b.set_keep_foe_stats(True)      # leaves = this exact position, never guessed opponent stats
        except Exception:
            pass
        for dep in range(1, max_depth + 1):
            lo, hi = val(b, 0, dep)
            gap = hi - lo
            if gap <= 2 * tol:
                v, why = 0.5 * (lo + hi), "solved"
                break
    except _Abort as e:
        why = str(e)
    if info:
        return v, {"nodes": nodes[0], "secs": time.time() - t0, "why": why, "gap": gap, "depth": dep}
    return v


_ROOT_SECONDS = None
_OPTS_DONE = False


def apply_opts(spec=None):
    """(2026-11-01) solver options (vgcsim.eg_solver_opts) from a comma list like run 18's
    "dbl,tt,soft,dead,smab,early,cellc,persist,tail=0.0005,tail_max=0.005,race_ext=2,bail=2,skip=0.001";
    default = env AGENT_EG_OPTS (empty = the module's defaults: all off except rmfast=0.0001, order2, leanenum,
    which are on since 2026-11-02). Process-wide; returns the old options as text,
    or None when nothing was set / the module is too old (unknown keyword)."""
    spec = os.environ.get("AGENT_EG_OPTS", "") if spec is None else spec
    if not spec or spec.strip().lower() in ("0", "off", "none") or not hasattr(vgcsim, "eg_solver_opts"):
        return None
    flags = {"dbl": "double_oracle", "tt": "tt", "soft": "soft_cap", "dead": "dead_end", "smab": "smab", "early": "early",
             "cellc": "cell_cache", "merge": "merge", "order": "order", "race": "race", "equiv": "equiv", "prune": "prune",
             "race2": "race2", "thresh": "thresh", "persist": "persist", "hpmono": "hpmono", "canon": "canon",
             "mirror": "mirror", "relax": "relax", "nodice": "nodice", "fastkey": "fastkey", "order2": "order2",
             "leanenum": "leanenum"}
    kw = {}
    for x in spec.split(","):
        x = x.strip()
        if not x:
            continue
        if "=" in x:
            k, v = x.split("=", 1)
            kw[k] = int(float(v)) if k in ("race_ext", "par", "sub1", "sub1_depth", "loop_n", "fix1") else float(v)
        else:
            kw[flags[x]] = True
    try:
        return vgcsim.eg_solver_opts(**kw)
    except TypeError as e:
        print("eg_rescore: solver options not applied (old module?):", e, flush=True)
        return None
def ensure_opts():
    """(2026-11-01) AGENT_EG_OPTS applied once per process (live solves and exact search leaves)."""
    global _OPTS_DONE
    if not _OPTS_DONE:
        _OPTS_DONE = True
        old = apply_opts()
        if old is not None:
            print("eg_rescore: solver options", os.environ.get("AGENT_EG_OPTS"), flush=True)


LAST_INFO = []   # (2026-11-01) per world (why, gap, depth, nodes) of the last live_solve


def live_solve(worlds, budget=300000, max_depth=4, tol=0.02, max_mons=1, deadline=None, iters=2000, threads=8, max_leaves=64):
    """(2026-10-30) live endgame decision (agent AGENT_ENDGAME): each search world's turn solved at the root
    (Battle.exact_root), then one Bayesian game over the worlds (one strategy for side 0, one per world for side 1).
    Returns (None, why) or (dict, "solved"): ours [(a0, a1)] (the same in every world), theirs0 (world 0's replies), x,
    Y [W, m] (worlds' reply lists padded with copies of their first reply), value, rows (our joints' values), info."""
    import nnsearch
    wl = list(worlds) if isinstance(worlds, (list, tuple)) else [worlds]
    if not wl or not hasattr(wl[0], "exact_root"):
        return None, "no exact_root"
    if wl[0].phase != "turn" or not qualifies(wl[0], max_mons):
        return None, "not an endgame"
    ours, theirs0, Ms, info, theirs_w = None, None, [], [], []
    # worlds solved on threads (exact_root releases the GIL); results used in world order
    # (2026-11-01) never wait past the deadline: a live solve ran 43.9 s with a 20 s deadline (the old code waited for every
    # world, then checked the time), leaving the normal search no time. Unfinished solves keep running on their threads
    # (the Rust call can't be interrupted) but nobody waits for them.
    ensure_opts()
    from concurrent.futures import ThreadPoolExecutor, wait
    ex = ThreadPoolExecutor(max(1, min(threads, len(wl))))
    secs = 0.0 if deadline is None else max(0.01, deadline - time.time())
    global _ROOT_SECONDS
    if _ROOT_SECONDS is None:                    # newer modules stop on their own at the deadline (seconds=)
        try:
            wl[0].exact_root(1, 1, 64, 1, 1, tol, seconds=0.01)
            _ROOT_SECONDS = True
        except TypeError:
            _ROOT_SECONDS = False
    kw = {"seconds": secs} if _ROOT_SECONDS else {}
    futs = [ex.submit(w.exact_root, budget, max_depth, max_leaves, iters, i + 1, tol, **kw) for i, w in enumerate(wl)]
    # (2026-11-01) EG_GRACE s of grace past the deadline: the Rust solve stops on its own at `seconds` and returns its last
    # gap / depth (kept in LAST_INFO for diagnostics); without the grace the Python wait always won the race
    grace = float(os.environ.get("EG_GRACE", "0.5"))
    done, pending = wait(futs, timeout=None if deadline is None else max(0.0, deadline - time.time() + grace))
    ex.shutdown(wait=False, cancel_futures=True)
    global LAST_INFO
    LAST_INFO = [tuple(f.result()[1:5]) if f in done else ("pending",) for f in futs]
    if pending:
        return None, "out of time"
    sols = [f.result() for f in futs]
    for i, sol in enumerate(sols):
        ok, why, gap, dep, nodes, o, th, lo, hi = sol
        info.append((why, round(float(gap), 4), dep, nodes))
        if not ok:
            return None, f"world {i}: {why}"
        o = [(tuple(a), tuple(b)) for a, b in o]
        if ours is None:
            ours, theirs0 = o, [(tuple(a), tuple(b)) for a, b in th]
        elif o != ours:
            return None, f"our joint actions differ in world {i}"
        theirs_w.append([(tuple(a), tuple(b)) for a, b in th])
        Ms.append(0.5 * (np.array(lo) + np.array(hi)).reshape(len(o), len(th)))
    m = max(M.shape[1] for M in Ms)
    Ms = [np.concatenate([M, np.repeat(M[:, :1], m - M.shape[1], 1)], 1) if M.shape[1] < m else M for M in Ms]
    x, Y, V, rows = nnsearch.bayes_solve(np.stack(Ms), iters=iters)
    return {"ours": ours, "theirs0": theirs0, "x": np.asarray(x), "Y": np.asarray(Y), "value": float(V),
            "rows": np.asarray(rows), "info": info, "Ms": [np.asarray(M) for M in Ms], "theirs_w": theirs_w}, "solved"
    # (2026-10-30 late) Ms (padded per-world cell midpoints) + theirs_w (each world's own reply list), for callers
