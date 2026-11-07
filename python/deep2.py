"""Two-turn search (2026-10-10, user: test whether looking a second turn ahead helps; open team sheets first).

On top of a one-turn sequential-halving search (sh_search.halving_search): the surviving pairs (our candidates x their
candidates) are played out for real in `dice` random seeds (common across pairs), and every resulting position is valued
by a small one-turn search of its own (policy top-k x opponent-head top-k, the value net after the second turn, solved
for its equilibrium) instead of by the value net directly. Positions that start with replacements (a faint at the end of
turn 1) are valued as the replacement game: every replacement option of both sides x the value net, solved.
The top-level game on the survivors is then solved with these deeper values.

    d = deepen(net, battle, r, device, dice=4, k_in=4, w_in=1, seed=...)
    d["M"] [len(survivors), len(their survivors)], d["x"], d["y"], d["value"], d["q"] {our candidate: value vs y}

Open sheets: `battle` is one position. Closed sheets: a list of positions with sampled opponent sets (as the agent's search
worlds); the dice rolls cycle through them. Each child's own search then treats that world's set as known (slightly
optimistic, as the one-turn search's worlds already are).
"""

import numpy as np
import torch
import vgcsim
from nnsearch import top_joint
from netlib import amp

PASS = ("pass", 0, 0, False)


def batch_solve(Ms, iters=400):
    """Equilibrium values of many small zero-sum games at once (regret matching+, linear averaging). Ms: list of
    [n_i, m_i] arrays; padded rows are hopeless for the row player, padded columns for the column player."""
    if not Ms:
        return np.zeros(0)
    import nnsearch
    if nnsearch._RUST_SOLVE:
        # (2026-10-18 speed) each game solved on its own in Rust (the padding never changed a solution)
        out = []
        for M, (x, y) in zip(Ms, vgcsim.rm_solve_batch([np.asarray(M, np.float64).tolist() for M in Ms], iters)):
            out.append(float(np.asarray(x) @ np.asarray(M, np.float64) @ np.asarray(y)))
        return np.array(out)
    n = max(m.shape[0] for m in Ms)
    m_ = max(m.shape[1] for m in Ms)
    B = len(Ms)
    A = np.full((B, n, m_), 0.0)
    rmask = np.zeros((B, n), bool)
    cmask = np.zeros((B, m_), bool)
    for b, M in enumerate(Ms):
        A[b, :M.shape[0], :M.shape[1]] = M
        rmask[b, :M.shape[0]] = True
        cmask[b, :M.shape[1]] = True
    # padded entries: row padding = -10 everywhere, column padding = +10 everywhere (never chosen)
    A = np.where(rmask[:, :, None], A, -10.0)
    A = np.where(cmask[:, None, :] | ~rmask[:, :, None], A, 10.0)
    rx, ry = np.zeros((B, n)), np.zeros((B, m_))
    sx, sy = np.zeros((B, n)), np.zeros((B, m_))
    ux0 = rmask / rmask.sum(1, keepdims=True)
    uy0 = cmask / cmask.sum(1, keepdims=True)
    for t in range(1, iters + 1):
        s = rx.sum(1, keepdims=True)
        x = np.where(s > 0, rx / np.maximum(s, 1e-30), ux0)
        s = ry.sum(1, keepdims=True)
        y = np.where(s > 0, ry / np.maximum(s, 1e-30), uy0)
        ux = np.einsum("bij,bj->bi", A, y)
        uy = -np.einsum("bi,bij->bj", x, A)
        rx = np.maximum(rx + ux - (x * ux).sum(1, keepdims=True), 0.0) * rmask
        ry = np.maximum(ry + uy - (y * uy).sum(1, keepdims=True), 0.0) * cmask
        sx += t * x
        sy += t * y
    x = sx / sx.sum(1, keepdims=True)
    y = sy / sy.sum(1, keepdims=True)
    return np.einsum("bi,bij,bj->b", x, A, y)


def _net_values(net, F, I, device):
    out = []
    with torch.no_grad(), amp():
        for s in range(0, len(F), 4096):
            out.append(net(torch.from_numpy(F[s:s + 4096]).to(device), torch.from_numpy(I[s:s + 4096]).to(device))[1]
                       .float().cpu().numpy())
    return np.concatenate(out).astype(np.float64) if out else np.zeros(0)


def _play(eb, o, t, seed):
    """Turn 1 from eb with our joint action o and theirs t (mid-turn switches: random, as the engine's search does)."""
    c = eb.copy()
    c.reseed(seed)
    turn = c.turn
    c.choose(0, *o)
    c.choose(1, *t)
    g = 0
    while c.phase == "switch" and c.turn == turn and g < 8:
        g += 1
        for s in (0, 1):
            if c.needs_choice(s):
                c.choose(s, *c.random_actions(s, seed ^ (0x5555 + 17 * g + s)))
    return c


def deepen(net, eb, r, device, dice=4, k_in=4, w_in=1, seed=1, hidden=False):
    act_o = list(r["survivors"])
    act_t = list(r["y_survivors"][0])
    ours = [r["ours"][a][1:] for a in act_o]
    theirs = [r["theirs"][b][1:] for b in act_t]
    no, nt = len(ours), len(theirs)
    vals = np.zeros((no, nt, dice))
    # closed sheets: eb is a list of positions (one per sampled opponent set, [0] = the best guess); dice roll d plays in
    # set world (start + d) mod len, the same for every pair (common random numbers)
    ebs = list(eb) if isinstance(eb, (list, tuple)) else [eb]
    start = seed % len(ebs)
    turn_nodes, sw_nodes = [], []           # (i, j, d, child)
    for i, o in enumerate(ours):
        for j, t in enumerate(theirs):
            for d in range(dice):
                c = _play(ebs[(start + d) % len(ebs)], o, t, seed + 7919 * d)
                ph = c.phase
                if ph == "ended":
                    w = c.winner
                    vals[i, j, d] = 1.0 if w == 0 else (-1.0 if w == 1 else 0.0)
                elif ph == "switch":
                    sw_nodes.append((i, j, d, c))
                else:
                    turn_nodes.append((i, j, d, c))
    # replacement nodes: every option of both sides -> value net, solved
    if sw_nodes:
        F, I, spans, Ms, terms = [], [], [], [], []
        for (i, j, d, c) in sw_nodes:
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
                    else:
                        f, ii = vgcsim.encode_battle(cc, 0, hidden=hidden)
                        F.append(f)
                        I.append(ii)
                        idx.append((a, b))
            Ms.append(M)
            spans.append(idx)
        v = _net_values(net, np.asarray(F, np.float32).reshape(len(F), -1), np.asarray(I, np.int32).reshape(len(I), -1),
                        device) if F else np.zeros(0)
        k = 0
        for M, idx in zip(Ms, spans):
            for (a, b) in idx:
                M[a, b] = v[k]
                k += 1
        sv = batch_solve(Ms)
        for (i, j, d, _), x in zip(sw_nodes, sv):
            vals[i, j, d] = x
    # turn nodes: a one-turn search of their own
    if turn_nodes:
        F, I = [], []
        for (_, _, _, c) in turn_nodes:
            f, ii = vgcsim.encode_battle(c, 0, hidden=hidden)
            F.append(f)
            I.append(ii)
        F = np.asarray(F, np.float32).reshape(len(F), -1)
        I = np.asarray(I, np.int32).reshape(len(I), -1)
        L, OL = [], []
        with torch.no_grad(), amp():
            for s in range(0, len(F), 4096):
                l0, _, ol = net(torch.from_numpy(F[s:s + 4096]).to(device), torch.from_numpy(I[s:s + 4096]).to(device), opp=True)
                L.append(l0.float().cpu().numpy())
                OL.append(ol.float().cpu().numpy())
        L, OL = np.concatenate(L), np.concatenate(OL)
        LF, LI, shapes, terms = [], [], [], []
        for n_, (_, _, _, c) in enumerate(turn_nodes):
            oo = top_joint(L[n_], c.legal_index_map(0), k_in)
            tt = top_joint(OL[n_], c.legal_index_map(1), k_in)
            f, ii, term = c.search_leaves(0, [x[1:] for x in oo], [x[1:] for x in tt], worlds=w_in,
                                          seed=seed + 104729 * (n_ + 1), hidden=hidden)
            term = np.array(term, np.float64)
            LF.append(np.asarray(f, np.float32).reshape(len(term), -1))
            LI.append(np.asarray(ii, np.int32).reshape(len(term), -1))
            shapes.append((len(oo), len(tt)))
            terms.append(term)
        allF, allI = np.concatenate(LF), np.concatenate(LI)
        allT = np.concatenate(terms)
        live = np.flatnonzero(np.isnan(allT))
        v = allT.copy()
        if len(live):
            v[live] = _net_values(net, allF[live], allI[live], device)
        Ms, k = [], 0
        for (a, b), term in zip(shapes, terms):
            n = a * b * w_in
            Ms.append(v[k:k + n].reshape(a, b, w_in).mean(-1))
            k += n
        sv = batch_solve(Ms)
        for (i, j, d, _), x in zip(turn_nodes, sv):
            vals[i, j, d] = x
    M = vals.mean(-1)
    x, y = solve_small(M)
    rows = M @ y
    return {"M": M, "x": x, "y": y, "value": float(x @ M @ y), "q": {a: float(rows[n]) for n, a in enumerate(act_o)},
            "survivors": act_o, "nodes": len(turn_nodes) + len(sw_nodes)}


def solve_small(M):
    from nnsearch import solve
    return solve(M)


def choose(r, d, mode="prior", beta=10.0):
    """Our candidate (index into r["ours"]) from the deep values: prior = argmax over the survivors of
    log pi(a) + beta (q(a) - V); max = the deep equilibrium's most likely action."""
    if mode == "max":
        return d["survivors"][int(np.argmax(d["x"]))]
    V = d["value"]
    best, bs = None, -1e18
    for a in d["survivors"]:
        s = np.log(max(r["ours"][a][0], 1e-12)) + beta * (d["q"][a] - V)
        if s > bs:
            best, bs = a, s
    return best
