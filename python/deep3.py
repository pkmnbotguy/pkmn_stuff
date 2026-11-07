"""Selective deepening with chance nodes (2026-10-13; steps 2 + 4 of the Nessie-inspired search plan).

On top of a one-turn sequential-halving search with chance nodes (sh_search.halving_search, SH_CHANCE=1):
  1. the surviving pairs' turns are enumerated into chance leaves (Battle.chance_leaf_battles: exact outcome
     probabilities, world 0 = the best guess of the opponent's sets), each valued by the value net (static value);
  2. leaves are deepened in order of  reach^2 x entropy(static win chance),  reach = x(ours) y(theirs) P(leaf) under the
     current root equilibrium (Nessie: the KL between a leaf's static value and its depth-1 search value is roughly
     proportional to the static value's entropy): a deepened leaf's value = the equilibrium of its own one-turn chance
     search (policy top-k x opponent-head top-k); a leaf that starts with replacements = the replacement game solved;
  3. each pair's value is corrected by sum over its deepened leaves of P(leaf) x (deep - static), the root re-solved,
     reach recomputed, and the next batch deepened, until the budget is spent;
  4. endgames (each side has <= `endgame_mons` Pokemon left): every leaf is deepened (budget = all of them) with wider
     inner searches, so the root value consists mostly of turn-2 equilibria and terminal results.

    d = deepen(net, battle, r, device, hidden=..., seed=..., budget=16)
    d["M"], d["x"], d["value"], d["q"] {our candidate: value vs their equilibrium}, d["survivors"], d["deepened"]

(2026-10-21, MAPLE paper) INFORMATION-SET DEEPENING (DEEP3_INFOSET=1, default). The old version
enumerated the leaves in world 0 only (the best guess of the opponent's sets) and solved every leaf's turn-2 game knowing
that guess exactly: our follow-up was tailored to a set we can't see (strategy fusion, PIMC-style). Now:
  - the leaves are enumerated in the first DEEP3_WORLDS (4) set worlds of the search (equal weights);
  - leaves are grouped by what WE would observe after the turn (our action, their moves by name, actives, HP in eighths,
    status, boosts, faints, weather / terrain / Trick Room / Tailwind): a group = an information set;
  - a group's turn-2 game is solved as a Bayesian game (nnsearch.bayes_solve): ONE strategy for us over candidates legal
    in every member, one per member for them (they know their set and see the dice); each member's deep value = our
    shared strategy against its own reply;
  - the root correction of a pair = the mean over worlds of its leaves' (deep - static);
  - the budget counts groups (endgames: members, to bound the time).
`opp_mix` = (w, pr over the root's surviving opponent columns): the opponent mix the bot assumes also sets the deepening
priority.
DEEP3_INFOSET=0 = the old world-0 version (deepen_world0).
"""

import os
import numpy as np
import sh_search
import torch
import vgcsim
from deep2 import batch_solve, PASS
from nnsearch import bayes_solve, solve, top_joint
from netlib import amp

INFOSET = os.environ.get("DEEP3_INFOSET", "1") == "1"
N_WORLDS = int(os.environ.get("DEEP3_WORLDS", "4"))


def _net(net, F, I, device, opp=False):
    outs = []
    with torch.no_grad(), amp():
        for s in range(0, len(F), 4096):
            o = net(torch.from_numpy(F[s:s + 4096]).to(device), torch.from_numpy(I[s:s + 4096]).to(device), opp=opp)
            outs.append([t.float().cpu().numpy() for t in o])
    return [np.concatenate([o[k] for o in outs]) for k in range(len(outs[0]))] if outs else None


def _entropy(v):
    p = np.clip((np.asarray(v, np.float64) + 1) / 2, 1e-6, 1 - 1e-6)
    return -(p * np.log(p) + (1 - p) * np.log(1 - p))


def _alive(c, side):
    return sum(1 for m in c.state()["sides"][side]["mons"] if m.get("brought", True) and not m["fainted"] and m["hp"] > 0)


REPL_TURN_MAX = int(os.environ.get("DEEP3_REPL_TURN", "32"))
# (2026-10-22) chance leaves stop at end-of-turn replacements (solved by _replacement_values) instead of a coin flip
def _repl_kw(b):
    return {"repl": True} if sh_search.REPL and hasattr(b, "search_leaves_chance_repl") else {}
REPL_STATS = {"turn": 0, "net": 0}          # positions after replacements: one-turn searched / value net (for DEEP3_LOG)


def _replacement_values(net, nodes, device, hidden, deep=None):
    """value of positions that start with replacements: every option of both sides x (the value net, or with `deep` =
    (seed, k_in, min_path, max_leaves) a one-turn chance search of the position after the replacements, like turn
    leaves get; at most DEEP3_REPL_TURN positions per call, the rest the value net), solved.
    (2026-10-22, arena game 5113: KO leaves were valued by the raw net after the replacement, -0.08 for a lone Mega
    Golisopod vs our two that the next turn's search valued -0.73: the most decisive leaves got the least lookahead.)"""
    F, I, spans, Ms = [], [], [], []
    TN, tidx = [], []
    for c in nodes:
        z = [np.zeros(38), np.zeros(38)]
        oo = top_joint(z, c.legal_index_map(0), 64) if c.needs_choice(0) else [(1.0, PASS, PASS)]
        tt = top_joint(z, c.legal_index_map(1), 64) if c.needs_choice(1) else [(1.0, PASS, PASS)]
        M = np.full((len(oo), len(tt)), np.nan)
        idx = []
        for a, (_, o0, o1) in enumerate(oo):
            for b, (_, t0, t1) in enumerate(tt):
                cc = c.copy()
                if cc.needs_choice(0):
                    cc.choose(0, o0, o1)
                if cc.needs_choice(1):
                    cc.choose(1, t0, t1)
                if cc.phase == "ended":
                    M[a, b] = 1.0 if cc.winner == 0 else (-1.0 if cc.winner == 1 else 0.0)
                elif (deep is not None and len(TN) < REPL_TURN_MAX and cc.phase == "turn" and cc.needs_choice(0)
                      and cc.needs_choice(1)):
                    TN.append(cc)
                    tidx.append((len(Ms), a, b))
                else:
                    f, ii = vgcsim.encode_battle(cc, 0, hidden=hidden)
                    F.append(f)
                    I.append(ii)
                    idx.append((a, b))
        Ms.append(M)
        spans.append(idx)
    REPL_STATS["turn"] += len(TN)
    REPL_STATS["net"] += len(F)
    if TN:
        tv, _ = _turn_values(net, TN, device, hidden, *deep)
        for (m_, a, b), v_ in zip(tidx, tv):
            Ms[m_][a, b] = float(v_[2]) if isinstance(v_, (tuple, list)) else float(v_)
    if F:
        v = _net(net, np.asarray(F, np.float32).reshape(len(F), -1), np.asarray(I, np.int32).reshape(len(I), -1), device)[1]
        k = 0
        for M, idx in zip(Ms, spans):
            for (a, b) in idx:
                M[a, b] = v[k]
                k += 1
    return batch_solve(Ms)


def _turn_values(net, nodes, device, hidden, seed, k_in, min_path, max_leaves):
    """value of positions where both sides choose a turn: a one-turn chance search of their own, solved"""
    F, I = [], []
    for c in nodes:
        f, ii = vgcsim.encode_battle(c, 0, hidden=hidden)
        F.append(f)
        I.append(ii)
    L, _, OL = _net(net, np.asarray(F, np.float32).reshape(len(F), -1), np.asarray(I, np.int32).reshape(len(I), -1),
                    device, opp=True)
    LF, LI, T, W, CELL, shapes, G = [], [], [], [], [], [], []
    off = 0
    for n_, c in enumerate(nodes):
        oo = top_joint(L[n_], c.legal_index_map(0), k_in) if c.needs_choice(0) else [(1.0, PASS, PASS)]
        tt = top_joint(OL[n_], c.legal_index_map(1), k_in) if c.needs_choice(1) else [(1.0, PASS, PASS)]
        f, ii, term, wt, cell, grp = sh_search.chance_rows(c, 0, [x[1:] for x in oo], [x[1:] for x in tt], 1,
                                                           seed + 104729 * (n_ + 1), hidden, min_path, max_leaves)
        term = np.array(term, np.float64)
        G.append(grp if grp is not None else (np.full(len(term), -1, np.int64),) * 3)
        LF.append(np.frombuffer(f, np.float32).reshape(len(term), -1))
        LI.append(np.frombuffer(ii, np.int32).reshape(len(term), -1))
        T.append(term)
        W.append(np.array(wt, np.float64))
        CELL.append(np.array(cell, np.int64) + off)
        shapes.append((len(oo), len(tt), off))
        off += len(oo) * len(tt)
    allT, allW, allC = np.concatenate(T), np.concatenate(W), np.concatenate(CELL)
    v = allT.copy()
    live = np.flatnonzero(np.isnan(allT))
    if len(live):
        v[live] = _net(net, np.concatenate(LF)[live], np.concatenate(LI)[live], device)[1]
    v, allW = sh_search.solve_groups(v, allW, allC, tuple(np.concatenate([g[k] for g in G]) for k in range(3)))
    num = np.bincount(allC, weights=allW * v, minlength=off)
    den = np.bincount(allC, weights=allW, minlength=off)
    cellv = num / np.maximum(den, 1e-12)
    Ms = [cellv[o:o + a * b].reshape(a, b) for (a, b, o) in shapes]
    return batch_solve(Ms), sum(len(t) for t in T)


def _hp8(m):
    return 0 if m["fainted"] or m["hp"] <= 0 else max(1, int(round(8 * m["hp"] / max(m["maxhp"], 1))))


def observation(c, root_state, j_tuple, side=0):
    """what `side` observes after the turn that led to leaf position c: their moves by NAME (slot indices differ between
    set worlds), and the visible state (actives, HP in eighths, status, boosts, faints, field)"""
    st = c.state()
    th = root_state["sides"][1 - side]
    used = []
    for k, t in enumerate(j_tuple):
        if t[0] == "move":
            a = th["active"][k] if k < len(th["active"]) else None
            mv = th["mons"][a]["moves"] if a is not None and 0 <= a < len(th["mons"]) else []
            used.append(("m", mv[t[1]][0] if t[1] < len(mv) else "?", t[2], bool(t[3])))
        elif t[0] == "switch":
            used.append(("s", th["mons"][t[1]]["species"] if t[1] < len(th["mons"]) else "?"))
        else:
            used.append(("p",))
    sides = []
    for s in (side, 1 - side):
        sd = st["sides"][s]
        act = []
        for k in range(2):
            a = sd["active"][k] if k < len(sd["active"]) else None
            m = sd["mons"][a] if a is not None and 0 <= a < len(sd["mons"]) else None
            act.append(None if m is None else (m["species"], _hp8(m), m["status"],
                                                tuple(m["boosts"][x] for x in ("atk", "def", "spa", "spd", "spe"))))
        alive = sum(1 for m in sd["mons"] if m.get("brought", True) and not m["fainted"] and m["hp"] > 0)
        sides.append((tuple(act), alive, sd["tailwind"] > 0, tuple(sd.get("switch_needed") or ())))
    f = st["field"]
    return (st["phase"], tuple(used), tuple(sides), f["weather"], f["terrain"], f["trick_room"] > 0)


def _turn_values_infoset(net, groups, device, hidden, seed, k_in, min_path, max_leaves):
    """groups: lists of (position, weight) = information sets; returns per group the list of member values (our ONE
    strategy over the group, theirs per member: a Bayesian game), and the number of leaves evaluated"""
    nodes = [c for g in groups for (c, _) in g]
    F, I = [], []
    for c in nodes:
        f, ii = vgcsim.encode_battle(c, 0, hidden=hidden)
        F.append(f)
        I.append(ii)
    L, _, OL = _net(net, np.asarray(F, np.float32).reshape(len(F), -1), np.asarray(I, np.int32).reshape(len(I), -1),
                    device, opp=True)
    jobs = []                    # per group: (ours, [(theirs, cell offset, node)], weights)
    LF, LI, T, W, CELL, G = [], [], [], [], [], []
    off, n_ = 0, 0
    for g in groups:
        idx = list(range(n_, n_ + len(g)))
        n_ += len(g)
        cs = [nodes[k] for k in idx]
        rep = idx[int(np.argmax([w for _, w in g]))]
        if cs[0].needs_choice(0):
            legal = [[set(m.values()) for m in c.legal_index_map(0)] for c in cs]
            pool = top_joint(L[rep], nodes[rep].legal_index_map(0), k_in)
            for k in idx:                        # every member's own top choice is a candidate too
                pool += top_joint(L[k], nodes[k].legal_index_map(0), 1)
            ours, seen = [], set()
            for o in pool:
                key = (tuple(o[1]), tuple(o[2]))
                if key in seen:
                    continue
                if all(tuple(o[1 + s_]) in lg[s_] for lg in legal for s_ in range(min(2, len(lg)))):
                    ours.append(o)
                    seen.add(key)
            ours = ours[:k_in + 2]
            if not ours:                         # nothing legal everywhere: solve the members on their own
                ours = None
        else:
            ours = [(1.0, PASS, PASS)]
        members = [[k] for k in idx] if ours is None else [idx]
        for mem_idx in members:
            ours_m = ours if ours is not None else top_joint(L[mem_idx[0]], nodes[mem_idx[0]].legal_index_map(0), k_in)
            per = []
            for k in mem_idx:
                c = nodes[k]
                if c.needs_choice(1):
                    tt = top_joint(OL[k], c.legal_index_map(1), k_in)
                else:
                    tt = [(1.0, PASS, PASS)]
                f, ii, term, wt, cell, grp = sh_search.chance_rows(c, 0, [x[1:] for x in ours_m], [x[1:] for x in tt], 1,
                                                                   seed + 104729 * (k + 1), hidden, min_path, max_leaves)
                term = np.array(term, np.float64)
                G.append(grp if grp is not None else (np.full(len(term), -1, np.int64),) * 3)
                LF.append(np.frombuffer(f, np.float32).reshape(len(term), -1))
                LI.append(np.frombuffer(ii, np.int32).reshape(len(term), -1))
                T.append(term)
                W.append(np.array(wt, np.float64))
                CELL.append(np.array(cell, np.int64) + off)
                per.append((tt, off, k))
                off += len(ours_m) * len(tt)
            jobs.append((ours_m, per, [g[k - idx[0]][1] for k in mem_idx]))
    allT, allW, allC = np.concatenate(T), np.concatenate(W), np.concatenate(CELL)
    v = allT.copy()
    live = np.flatnonzero(np.isnan(allT))
    if len(live):
        v[live] = _net(net, np.concatenate(LF)[live], np.concatenate(LI)[live], device)[1]
    v, allW = sh_search.solve_groups(v, allW, allC, tuple(np.concatenate([g[k] for g in G]) for k in range(3)))
    num = np.bincount(allC, weights=allW * v, minlength=off)
    den = np.bincount(allC, weights=allW, minlength=off)
    cellv = num / np.maximum(den, 1e-12)
    vals = {}
    for ours_m, per, wts in jobs:
        Ms = []
        for tt, o, k in per:
            Ms.append(cellv[o:o + len(ours_m) * len(tt)].reshape(len(ours_m), len(tt)))
        if len(Ms) == 1:
            x, y = solve(Ms[0])
            vals[per[0][2]] = float(x @ Ms[0] @ y)
            continue
        m = max(M.shape[1] for M in Ms)
        Mp = np.stack([np.hstack([M, np.repeat(M[:, :1], m - M.shape[1], axis=1)]) if M.shape[1] < m else M for M in Ms])
        x, Y, _, _ = bayes_solve(Mp, p=np.asarray(wts, np.float64) + 1e-12)
        for kk, (_, _, k) in enumerate(per):
            vals[k] = float(x @ Mp[kk] @ Y[kk])
    out, n_ = [], 0
    for g in groups:
        out.append([vals[k] for k in range(n_, n_ + len(g))])
        n_ += len(g)
    return out, sum(len(t) for t in T)


def deepen(net, battle, r, device, hidden=False, seed=1, budget=16, batch=8, k_in=4, min_path=0.1, max_leaves=16,
           endgame_mons=2, endgame_budget=200, endgame_k=6, side=0, opp_mix=None):
    """see the module docstring; `r` = a halving_search result for `side` on `battle` (a position or a list of them)"""
    if INFOSET and side == 0:
        return deepen_infoset(net, battle, r, device, hidden=hidden, seed=seed, budget=budget, batch=batch, k_in=k_in,
                              min_path=min_path, max_leaves=max_leaves, endgame_mons=endgame_mons,
                              endgame_budget=endgame_budget, endgame_k=endgame_k, opp_mix=opp_mix)
    return deepen_world0(net, battle, r, device, hidden=hidden, seed=seed, budget=budget, batch=batch, k_in=k_in,
                         min_path=min_path, max_leaves=max_leaves, endgame_mons=endgame_mons,
                         endgame_budget=endgame_budget, endgame_k=endgame_k, side=side)


def _endgame_solver(net, b0, r, device, hidden, seed, side):
    """(2026-10-13) the endgame solver: search until the game is decided or the node budget runs out (endgame.py)"""
    import endgame as eg
    d = eg.solve(net, b0, r, device, hidden=hidden, seed=seed, budget=int(os.environ.get("DEEP3_SOLVER_BUDGET", "20000")),
                 side=side)
    if os.environ.get("DEEP3_LOG"):
        with open(os.environ["DEEP3_LOG"], "a") as fh:
            fh.write(f"SOLVER {d['seconds']:.2f}s nodes {d['nodes']}, depth {d['depth']}, open {d['open']}, solved {d['solved']}, "
                     f"value {d['value']:+.3f} (root {float(r['value']):+.3f})\n")
    return d


def deepen_infoset(net, battle, r, device, hidden=False, seed=1, budget=16, batch=8, k_in=4, min_path=0.1, max_leaves=16,
                   endgame_mons=2, endgame_budget=200, endgame_k=6, opp_mix=None):
    """the information-set version (module docstring); same result format as deepen_world0"""
    import time as _time
    t_start = _time.time()
    side = 0
    blist = list(battle) if isinstance(battle, (list, tuple)) else [battle]
    b0 = blist[0]
    wl = blist[:max(1, N_WORLDS)]
    pw = 1.0 / len(wl)
    act_o = list(r["survivors"])
    act_t = list(r["y_survivors"][0])
    ours = [r["ours"][a] for a in act_o]
    theirs = [r["theirs"][b] for b in act_t]
    Mall = np.asarray(r["M_all"], np.float64)
    M0 = Mall[np.ix_(act_o, act_t)]
    if np.isnan(M0).any():
        M0 = np.nan_to_num(M0, nan=float(r["value"]))
    no, nt = len(ours), len(theirs)
    st0 = b0.state()
    endgame = all(sum(1 for m in st0["sides"][s]["mons"] if not m["fainted"] and m["hp"] > 0 and m.get("brought", True))
                  <= endgame_mons for s in (0, 1))
    if endgame and int(os.environ.get("DEEP3_SOLVER", "0")):
        return _endgame_solver(net, b0, r, device, hidden, seed, side)
    if endgame:
        budget, k_in, min_path, max_leaves = endgame_budget, endgame_k, 0.05, 32
    cells = [(i, j, 0) for i in range(no) for j in range(nt)]
    leaves = []                             # (i, j, world, position, P within the cell and world, world's root state)
    for w, bw in enumerate(wl):
        stw = bw.state()
        leaf_sets = bw.chance_leaf_battles(side, [o[1:] for o in ours], [t[1:] for t in theirs], cells, worlds=1,
                                           seed=seed + 7919 * w, min_path=min_path, max_leaves=max_leaves, hidden=bool(hidden), **_repl_kw(bw))
        for (i, j, _), ls in zip(cells, leaf_sets):
            tot = sum(p for _, p in ls) or 1.0
            for c, p in ls:
                leaves.append((i, j, w, c, p / tot, stw))
    static = np.zeros(len(leaves))
    F, I, live, swl = [], [], [], []
    for n_, (i, j, w, c, p, _) in enumerate(leaves):
        if c.phase == "ended":
            static[n_] = 1.0 if c.winner == 0 else (-1.0 if c.winner == 1 else 0.0)
        elif c.phase == "switch":
            swl.append(n_)                  # (2026-10-22) end-of-turn replacements: solved, not encoded mid-request
        else:
            f, ii = vgcsim.encode_battle(c, 0, hidden=hidden)
            F.append(f)
            I.append(ii)
            live.append(n_)
    if live:
        static[live] = _net(net, np.asarray(F, np.float32).reshape(len(F), -1),
                            np.asarray(I, np.int32).reshape(len(I), -1), device)[1]
    if swl:
        static[swl] = _replacement_values(net, [leaves[k][3] for k in swl], device, hidden)
    # information sets: same our action, same observation (their move names, visible state)
    gkey, groups = {}, []
    for n_, (i, j, w, c, p, stw) in enumerate(leaves):
        if c.phase == "ended":
            continue
        key = (i, observation(c, stw, theirs[j][1:], side))
        g = gkey.get(key)
        if g is None:
            g = gkey[key] = len(groups)
            groups.append([])
        groups[g].append(n_)
    deep = static.copy()
    gdone = np.zeros(len(groups), bool)
    leaf_w = np.array([pw * p for (_, _, _, _, p, _) in leaves])
    corr = np.zeros((no, nt))
    M = M0.copy()
    Mw0 = r.get("M_worlds")
    y_mix = None
    if callable(opp_mix):                        # (2026-10-22) y -> the assumed mix
        y_mix = opp_mix
    elif opp_mix is not None:
        w_, pr_ = opp_mix
        if pr_ is not None and len(pr_) == nt and w_ > 0:
            y_mix = (float(w_), np.asarray(pr_, np.float64))

    def solve_root(M, corr):
        if Mw0 is not None and np.shape(Mw0)[1:] == M.shape:
            x_, Y_, v_, rows_ = bayes_solve(np.asarray(Mw0) + corr[None])
            return x_, Y_.mean(0), v_, rows_
        x_, y_ = solve(M)
        return x_, y_, float(x_ @ M @ y_), M @ y_

    x, y, val, rows = solve_root(M, corr)
    n_deep, n_members, n_evals = 0, 0, len(live)
    gstatic = np.array([np.average(static[g], weights=leaf_w[g] + 1e-12) for g in groups]) if groups else np.zeros(0)
    gphase = [leaves[g[0]][3].phase for g in groups]
    gi = [np.array([leaves[k][0] for k in g]) for g in groups]
    gj = [np.array([leaves[k][1] for k in g]) for g in groups]
    gw = [leaf_w[g] for g in groups]
    cap = budget if not endgame else 10 ** 9
    member_cap = 10 ** 9 if not endgame else budget
    while n_deep < cap and n_members < member_cap and len(groups) and not gdone.all():
        yy = (np.asarray(y) if y_mix is None else np.asarray(y_mix(np.asarray(y, np.float64))) if callable(y_mix)
              else (1 - y_mix[0]) * np.asarray(y) + y_mix[0] * y_mix[1])
        xx = np.asarray(x)
        reach = np.array([float((xx[a] * yy[b] * w).sum()) for a, b, w in zip(gi, gj, gw)])
        pri = reach ** 2 * _entropy(gstatic)
        pri[gdone] = -1.0
        order = [g for g in np.argsort(-pri) if pri[g] > 0][:min(batch, cap - n_deep)]
        if not order:
            break
        turn_g = [g for g in order if gphase[g] == "turn"]
        sw_g = [g for g in order if gphase[g] == "switch"]
        if sw_g:
            ks = [k for g in sw_g for k in groups[g]]
            for k, v in zip(ks, _replacement_values(net, [leaves[k][3] for k in ks], device, hidden,
                                                         deep=(seed + 31 * n_deep + 7, k_in, min_path, max_leaves))):
                deep[k] = v
        if turn_g:
            gl = [[(leaves[k][3], leaf_w[k]) for k in groups[g]] for g in turn_g]
            vals, ne = _turn_values_infoset(net, gl, device, hidden, seed + 31 * n_deep, k_in, min_path, max_leaves)
            n_evals += ne
            for g, vs in zip(turn_g, vals):
                for k, v in zip(groups[g], vs):
                    deep[k] = v
        for g in order:
            gdone[g] = True
            n_members += len(groups[g])
        n_deep += len(order)
        corr[:] = 0.0
        for n_, (i, j, w, c, p, _) in enumerate(leaves):
            corr[i, j] += pw * p * (deep[n_] - static[n_])
        M = M0 + corr
        x, y, val, rows = solve_root(M, corr)
    if os.environ.get("DEEP3_LOG"):
        sizes = [len(g) for g in groups]
        with open(os.environ["DEEP3_LOG"], "a") as fh:
            fh.write(f"infoset: {len(wl)} worlds, {len(leaves)} leaves, {len(groups)} groups (mean size "
                     f"{np.mean(sizes) if sizes else 0:.2f}, max {max(sizes) if sizes else 0}, multi-world "
                     f"{sum(1 for g in groups if len({leaves[k][2] for k in g}) > 1)}), deepened {n_deep} groups / "
                     f"{n_members} leaves, endgame {endgame}, value {val:+.3f} (root {float(r['value']):+.3f}), "
                     f"{_time.time() - t_start:.2f}s, after replacements {REPL_STATS['turn']} searched / "
                     f"{REPL_STATS['net']} net, group phases "
                     f"{ {p_: gphase.count(p_) for p_ in set(gphase)} }, switch-phase reach "
                     f"{sum(float(leaf_w[k]) for g, p_ in zip(groups, gphase) if p_ == 'switch' for k in g):.3f}\n")
        REPL_STATS["turn"] = REPL_STATS["net"] = 0
    return {"M": M, "x": x, "y": y, "value": val, "q": {a: float(rows[n]) for n, a in enumerate(act_o)},
            "survivors": act_o, "deepened": n_deep, "leaves": len(leaves), "evals": n_evals, "endgame": endgame,
            "groups": len(groups), "members_deepened": n_members}


def deepen_world0(net, battle, r, device, hidden=False, seed=1, budget=16, batch=8, k_in=4, min_path=0.1, max_leaves=16,
                  endgame_mons=2, endgame_budget=200, endgame_k=6, side=0):
    """see the module docstring; `r` = a halving_search result for `side` on `battle` (a position or a list of them)"""
    b0 = battle[0] if isinstance(battle, (list, tuple)) else battle
    act_o = list(r["survivors"])
    act_t = list(r["y_survivors"][0])
    ours = [r["ours"][a] for a in act_o]
    theirs = [r["theirs"][b] for b in act_t]
    Mall = np.asarray(r["M_all"], np.float64)
    M0 = Mall[np.ix_(act_o, act_t)]
    if np.isnan(M0).any():
        M0 = np.nan_to_num(M0, nan=float(r["value"]))
    no, nt = len(ours), len(theirs)
    st = b0.state()
    endgame = all(sum(1 for m in st["sides"][s]["mons"] if not m["fainted"] and m["hp"] > 0 and m.get("brought", True))
                  <= endgame_mons for s in (0, 1))
    if endgame and int(os.environ.get("DEEP3_SOLVER", "0")):
        # (2026-10-13) the endgame solver: search until the game is decided or the node budget runs out (endgame.py)
        import endgame as eg
        d = eg.solve(net, b0, r, device, hidden=hidden, seed=seed, budget=int(os.environ.get("DEEP3_SOLVER_BUDGET", "20000")),
                     side=side)
        if os.environ.get("DEEP3_LOG"):
            with open(os.environ["DEEP3_LOG"], "a") as fh:
                fh.write(f"SOLVER {d['seconds']:.2f}s nodes {d['nodes']}, depth {d['depth']}, open {d['open']}, solved {d['solved']}, "
                         f"value {d['value']:+.3f} (root {float(r['value']):+.3f})\n")
        return d
    if endgame:
        budget, k_in, min_path, max_leaves = endgame_budget, endgame_k, 0.05, 32
    cells = [(i, j, 0) for i in range(no) for j in range(nt)]
    leaf_sets = b0.chance_leaf_battles(side, [o[1:] for o in ours], [t[1:] for t in theirs], cells, worlds=1,
                                       seed=seed, min_path=min_path, max_leaves=max_leaves, hidden=bool(hidden), **_repl_kw(b0))
    leaves = []                             # (i, j, battle, P)
    for (i, j, _), ls in zip(cells, leaf_sets):
        tot = sum(w for _, w in ls) or 1.0
        for c, w in ls:
            leaves.append((i, j, c, w / tot))
    # static values (from the search's view: side 0 = us)
    static = np.zeros(len(leaves))
    F, I, live, swl = [], [], [], []
    for n_, (i, j, c, w) in enumerate(leaves):
        if c.phase == "ended":
            static[n_] = 1.0 if c.winner == 0 else (-1.0 if c.winner == 1 else 0.0)
        elif c.phase == "switch":
            swl.append(n_)
        else:
            f, ii = vgcsim.encode_battle(c, 0, hidden=hidden)
            F.append(f)
            I.append(ii)
            live.append(n_)
    if live:
        static[live] = _net(net, np.asarray(F, np.float32).reshape(len(F), -1),
                            np.asarray(I, np.int32).reshape(len(I), -1), device)[1]
    if swl:
        static[swl] = _replacement_values(net, [leaves[k][2] for k in swl], device, hidden)
    deep = static.copy()
    done = np.array([c.phase == "ended" for (_, _, c, _) in leaves])
    corr = np.zeros((no, nt))
    M = M0.copy()
    Mw0 = r.get("M_worlds")

    def solve_root(M, corr):
        """(x, y, value, rows): the Bayesian game over the search's worlds when it has them, else the averaged game"""
        if Mw0 is not None and np.shape(Mw0)[1:] == M.shape:
            x_, Y_, v_, rows_ = bayes_solve(np.asarray(Mw0) + corr[None])
            return x_, Y_.mean(0), v_, rows_
        x_, y_ = solve(M)
        return x_, y_, float(x_ @ M @ y_), M @ y_

    x, y, val, rows = solve_root(M, corr)
    n_deep, n_evals = 0, len(live)
    while n_deep < budget and not done.all():
        reach = np.array([x[i] * y[j] * w for (i, j, _, w) in leaves])
        pri = reach ** 2 * _entropy(static)
        pri[done] = -1.0
        order = [k for k in np.argsort(-pri) if pri[k] > 0][:min(batch, budget - n_deep)]
        if not order:
            break
        turn_nodes = [k for k in order if leaves[k][2].phase == "turn"]
        sw_nodes = [k for k in order if leaves[k][2].phase == "switch"]
        if sw_nodes:
            vals = _replacement_values(net, [leaves[k][2] for k in sw_nodes], device, hidden,
                                        deep=(seed + 31 * n_deep + 7, k_in, min_path, max_leaves))
            for k, v in zip(sw_nodes, vals):
                deep[k] = v
        if turn_nodes:
            vals, ne = _turn_values(net, [leaves[k][2] for k in turn_nodes], device, hidden, seed + 31 * n_deep, k_in,
                                    min_path, max_leaves)
            n_evals += ne
            for k, v in zip(turn_nodes, vals):
                deep[k] = v
        for k in order:
            done[k] = True
        n_deep += len(order)
        corr[:] = 0.0
        for n_, (i, j, _, w) in enumerate(leaves):
            corr[i, j] += w * (deep[n_] - static[n_])
        M = M0 + corr
        x, y, val, rows = solve_root(M, corr)
    if os.environ.get("DEEP3_LOG"):                 # one line per call (checks that a bot really runs the deepening)
        with open(os.environ["DEEP3_LOG"], "a") as fh:
            fh.write(f"deepened {n_deep} of {len(leaves)} leaves, endgame {endgame}, value {val:+.3f} "
                     f"(root {float(r['value']):+.3f})\n")
    return {"M": M, "x": x, "y": y, "value": val, "q": {a: float(rows[n]) for n, a in enumerate(act_o)},
            "survivors": act_o, "deepened": n_deep, "leaves": len(leaves), "evals": n_evals, "endgame": endgame}
