"""Endgame solver (2026-10-13; Nessie-style): when each side has <= 2 Pokemon left, search the simultaneous-move game
turn after turn until the game is DECIDED or the node budget runs out, with exact chance outcomes, and back up
equilibrium values. The value net is used only for lines still open when the budget ends.

Tree (built breadth-first, one level = one decision point):
  root      the search's surviving candidates (ours x theirs), each cell's turn enumerated into chance outcomes
            (Battle.chance_leaf_battles, world 0 = the best guess of the opponent's sets)
  turn node both sides choose: policy top-k (ours) x opponent-head top-k (theirs), each pair's turn enumerated into
            chance outcomes (exact probabilities, cut at min_path / max_leaves)
  switch    replacements: every legal option of the side(s) that must choose (deterministic children)
  terminal  +1 / -1 / 0
Every new node is evaluated by the net once (value = fallback, logits = candidates). Nodes are expanded level by level,
most important first (reach = product of the root equilibrium / policy weights and outcome probabilities along the path)
while the budget allows. Backup from the deepest level: each expanded node's payoff matrix -> equilibrium value.
"solved" = no open line was left (exact within the candidate lists and the outcome cut).

    d = solve(net, battle, r, device, hidden=..., seed=..., budget=20000)   # same dict as deep3.deepen (+ "solved", "depth")
"""

import os
import time
import numpy as np
import vgcsim
from deep2 import batch_solve, PASS
from deep3 import _net
from nnsearch import solve as solve_game, top_joint


def _ks(depth):
    """candidates per side at a turn node of this depth (1 = the turn after the root)"""
    return 4 if depth <= 1 else (3 if depth == 2 else 2)


def _terminal(c):
    return 1.0 if c.winner == 0 else (-1.0 if c.winner == 1 else 0.0)


def solve(net, battle, r, device, hidden=False, seed=1, budget=20000, min_path=0.05, max_leaves=12, inner_min_path=0.1,
          inner_max_leaves=8, max_depth=12, side=0, prune=None):
    t_start = time.time()
    prune = float(os.environ.get("SOLVER_PRUNE", "0.98")) if prune is None else prune   # lines the net calls decided
    b0 = battle[0] if isinstance(battle, (list, tuple)) else battle
    act_o = list(r["survivors"])
    act_t = list(r["y_survivors"][0])
    ours = [r["ours"][a] for a in act_o]
    theirs = [r["theirs"][b] for b in act_t]
    Mall = np.asarray(r["M_all"], np.float64)
    M0 = np.nan_to_num(Mall[np.ix_(act_o, act_t)], nan=float(r["value"]))
    x0, y0 = solve_game(M0)
    no, nt = len(ours), len(theirs)

    # node arrays
    bat, kind, val, reach, depth = [], [], [], [], []      # kind: 0 open (net value), 1 terminal, 2 expanded
    logits, ologits = [], []
    children = []                                           # per expanded node: (shape, [(a, b, child, P)])

    def add(c, rch, d):
        bat.append(c)
        reach.append(rch)
        depth.append(d)
        logits.append(None)
        ologits.append(None)
        children.append(None)
        if c.phase == "ended":
            kind.append(1)
            val.append(_terminal(c))
        else:
            kind.append(0)
            val.append(np.nan)
        return len(bat) - 1

    def evaluate(ids):
        ids = [i for i in ids if kind[i] == 0]
        if not ids:
            return
        F, I = [], []
        for i in ids:
            f, ii = vgcsim.encode_battle(bat[i], 0, hidden=hidden)
            F.append(f)
            I.append(ii)
        L, V, OL = _net(net, np.asarray(F, np.float32).reshape(len(F), -1), np.asarray(I, np.int32).reshape(len(I), -1),
                        device, opp=True)
        for n_, i in enumerate(ids):
            val[i] = float(V[n_])
            logits[i] = L[n_]
            ologits[i] = OL[n_]

    # root
    cells = [(i, j, 0) for i in range(no) for j in range(nt)]
    lsets = b0.chance_leaf_battles(side, [o[1:] for o in ours], [t[1:] for t in theirs], cells, worlds=1, seed=seed,
                                   min_path=min_path, max_leaves=max_leaves, hidden=bool(hidden))
    root_kids = []
    for (i, j, _), ls in zip(cells, lsets):
        tot = sum(w for _, w in ls) or 1.0
        for c, w in ls:
            p = w / tot
            root_kids.append((i, j, add(c, x0[i] * y0[j] * p, 1), p))
    frontier = list(range(len(bat)))
    evaluate(frontier)
    frontier = [i for i in frontier if kind[i] == 0]
    d_reached = 1
    out_of_budget = False

    while frontier and not out_of_budget:
        frontier.sort(key=lambda i: -reach[i])
        new = []
        for i in frontier:
            if len(bat) >= budget:
                out_of_budget = True
                break
            c, d = bat[i], depth[i]
            if d >= max_depth or abs(val[i]) >= prune:
                continue
            if c.phase == "switch":
                z = [np.zeros(38), np.zeros(38)]
                oo = top_joint(z, c.legal_index_map(0), 64) if c.needs_choice(0) else [(1.0, PASS, PASS)]
                tt = top_joint(z, c.legal_index_map(1), 64) if c.needs_choice(1) else [(1.0, PASS, PASS)]
                kids = []
                for a, (_, o0, o1) in enumerate(oo):
                    for bb, (_, t0, t1) in enumerate(tt):
                        cc = c.copy()
                        if cc.needs_choice(0):
                            cc.choose(0, o0, o1)
                        if cc.needs_choice(1):
                            cc.choose(1, t0, t1)
                        k = add(cc, reach[i] / (len(oo) * len(tt)), d)     # replacements don't use up a turn
                        kids.append((a, bb, k, 1.0))
                        new.append(k)
            else:
                k_ = _ks(d)
                oo = top_joint(logits[i], c.legal_index_map(0), k_) if c.needs_choice(0) else [(1.0, PASS, PASS)]
                tt = top_joint(ologits[i], c.legal_index_map(1), k_) if c.needs_choice(1) else [(1.0, PASS, PASS)]
                po = np.array([p for p, _, _ in oo]) + 1e-9
                pt = np.array([p for p, _, _ in tt]) + 1e-9
                po, pt = po / po.sum(), pt / pt.sum()
                cl = [(a, bb, 0) for a in range(len(oo)) for bb in range(len(tt))]
                ls_ = c.chance_leaf_battles(0, [o[1:] for o in oo], [t[1:] for t in tt], cl, worlds=1,
                                            seed=seed + 7919 * (i + 1), min_path=inner_min_path,
                                            max_leaves=inner_max_leaves)
                kids = []
                for (a, bb, _), ls in zip(cl, ls_):
                    tot = sum(w for _, w in ls) or 1.0
                    for cc, w in ls:
                        p = w / tot
                        k = add(cc, reach[i] * po[a] * pt[bb] * p, d + 1)
                        kids.append((a, bb, k, p))
                        new.append(k)
            kind[i] = 2
            children[i] = ((len(oo), len(tt)), kids)
        evaluate(new)
        frontier = [k for k in new if kind[k] == 0]
        if new:
            d_reached = max(d_reached, max(depth[k] for k in new))

    # backup, deepest expanded nodes first (children always have larger ids than their parent)
    exp = [i for i in range(len(bat)) if kind[i] == 2]
    exp.sort(key=lambda i: -i)
    # group into batches whose children are all final: process by descending id in chunks of independent nodes
    done = np.array([kind[i] != 2 for i in range(len(bat))])
    pending = exp
    while pending:
        ready, rest = [], []
        for i in pending:
            if all(done[k] for _, _, k, _ in children[i][1]):
                ready.append(i)
            else:
                rest.append(i)
        Ms = []
        for i in ready:
            (n_, m_), kids = children[i]
            M = np.zeros((n_, m_))
            for a, bb, k, p in kids:
                M[a, bb] += p * val[k]
            Ms.append(M)
        for i, v in zip(ready, batch_solve(Ms)):
            val[i] = float(v)
            done[i] = True
        pending = rest
    M = np.zeros((no, nt))
    for i, j, k, p in root_kids:
        M[i, j] += p * val[k]
    x, y = solve_game(M)
    rows = M @ y
    n_open = sum(1 for i in range(len(bat)) if kind[i] == 0)
    return {"M": M, "x": x, "y": y, "value": float(x @ M @ y), "q": {a: float(rows[n]) for n, a in enumerate(act_o)},
            "survivors": act_o, "deepened": sum(1 for i in range(len(bat)) if kind[i] == 2), "leaves": len(root_kids),
            "evals": len(bat), "endgame": True, "solved": n_open == 0, "open": n_open, "nodes": len(bat),
            "depth": d_reached, "seconds": time.time() - t_start}
