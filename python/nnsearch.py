"""
Play-time search for the network (the mikumiku37 recipe): each turn is a simultaneous-move game.

    action = search(net, battle, side)       # battle: a vgcsim.Battle in its move phase

1. The network proposes our top-K joint actions (both slots together) and, seen from the opponent's
   side, their top-K replies.
2. For every pair, in several sampled "worlds" (world 0: the opponent's stats guessed as the bridge does;
   the others: plausible builds sampled from a few templates; each world with its own damage rolls,
   crits, accuracy and speed ties), the engine plays the turn (vgcsim Battle.search_leaves).
3. The value network scores each resulting position (finished games count as +1 / -1).
4. The KxK table (averaged over worlds) is solved as a zero-sum game (regret matching), and our move
   is drawn from the resulting mixed strategy (or its most likely action with mode="max").
Forced replacements (switch phase) use the network's best action directly.
"""

import os
import numpy as np
import torch
import vgcsim
from netlib import amp

N_ACT = 38


def net_logits(net, battle, sides, device, hidden=False):
    """The network's action logits for each side in `sides`: [len(sides), 2, 38] (numpy) and values."""
    fs, is_ = [], []
    for s in sides:
        f, i = vgcsim.encode_battle(battle, s, hidden=hidden)
        fs.append(f)
        is_.append(i)
    with torch.no_grad(), amp():
        lg, v = net(torch.tensor(np.array(fs, np.float32), device=device), torch.tensor(np.array(is_, np.int32), device=device))
    return lg.float().cpu().numpy(), v.float().cpu().numpy()


def slot_probs(logits, idx_map):
    """Softmax over one slot's legal actions: [(index, action tuple, prob)]."""
    idx = sorted(idx_map)
    z = np.array([logits[k] for k in idx], np.float64)
    p = np.exp(z - z.max())
    p /= p.sum()
    return [(k, idx_map[k], float(q)) for k, q in zip(idx, p)]


def top_joint_table(table, maps, k):
    """(2026-10-22) The k most likely legal joint actions from a [38, 38] log-probability table over (slot 1, slot 2) pairs
    (netlib.opp_joint_logprobs: the opponent joint head's marginals + interaction), same tuple layout as top_joint:
    (probability, action 1, action 2), probabilities renormalised over the legal pairs."""
    m0, m1 = maps[0] if maps else {}, maps[1] if len(maps) > 1 else {}
    tab = np.asarray(table, np.float64)
    if not m0 or not m1:          # one slot has nothing to choose: per-slot logits = the table's row / column maxima
        return top_joint(np.stack([tab.max(1), tab.max(0)]), maps, k)
    i0, i1 = sorted(m0), sorted(m1)
    sub = tab[np.ix_(i0, i1)]
    joint = []
    for a, ia in enumerate(i0):
        t0 = m0[ia]
        for c, ic in enumerate(i1):
            t1 = m1[ic]
            if t0[0] == "switch" and t1[0] == "switch" and t0[1] == t1[1]:
                continue
            if t0[0] == "move" and t1[0] == "move" and t0[3] and t1[3]:
                continue
            joint.append((sub[a, c], t0, t1))
    if not joint:
        return top_joint(np.stack([tab.max(1), tab.max(0)]), maps, k)
    z = np.array([j[0] for j in joint])
    p = np.exp(z - z.max())
    p /= p.sum()
    joint = [(float(q), t0, t1) for q, (_, t0, t1) in zip(p, joint)]
    joint.sort(key=lambda x: -x[0])
    return joint[:k]


def top_joint(logits2, maps, k):
    """The k most likely legal joint actions (no double switch to one Pokemon, no double Mega)."""
    a0, a1 = slot_probs(logits2[0], maps[0]), slot_probs(logits2[1], maps[1])
    joint = []
    for _, t0, p0 in a0:
        for _, t1, p1 in a1:
            if t0[0] == "switch" and t1[0] == "switch" and t0[1] == t1[1]:
                continue
            if t0[0] == "move" and t1[0] == "move" and t0[3] and t1[3]:
                continue
            joint.append((p0 * p1, t0, t1))
    if not joint:
        # e.g. both slots must replace a fainted Pokemon but one is left: the best single one, the other passes
        p0, t0 = max((p, t) for _, t, p in a0)
        joint = [(p0, t0, ("pass", 0, 0, False))]
    joint.sort(key=lambda x: -x[0])
    return joint[:k]


# (2026-10-18 speed) the solvers run in Rust (vgcsim.rm_solve / rm_bayes_solve: same algorithm and update order, ~1000x
# faster); VGCSIM_PY_SOLVE=1 or an old module without them = the Python versions below
_RUST_SOLVE = os.environ.get("VGCSIM_PY_SOLVE", "0") != "1" and hasattr(vgcsim, "rm_solve")


def solve(M, iters=1000):
    """Mixed strategies (x for the row player, maximising; y for the column player) of the zero-sum
    game M, by regret matching+ with linear averaging."""
    if _RUST_SOLVE:
        x, y = vgcsim.rm_solve(np.asarray(M, np.float64).tolist(), iters)
        return np.array(x), np.array(y)
    return solve_py(M, iters)


def solve_py(M, iters=1000):
    M = np.asarray(M, np.float64)
    n, m = M.shape
    rx, ry = np.zeros(n), np.zeros(m)
    sx, sy = np.zeros(n), np.zeros(m)
    for t in range(1, iters + 1):
        x = rx / rx.sum() if rx.sum() > 0 else np.full(n, 1.0 / n)
        y = ry / ry.sum() if ry.sum() > 0 else np.full(m, 1.0 / m)
        ux = M @ y
        uy = -(x @ M)
        rx = np.maximum(rx + ux - x @ ux, 0.0)
        ry = np.maximum(ry + uy - y @ uy, 0.0)
        sx += t * x
        sy += t * y
    return sx / sx.sum(), sy / sy.sum()


def bayes_solve(Ms, p=None, iters=1000):
    """(2026-10-13) Bayesian zero-sum game over sampled worlds (opponent sets): one strategy x for us (we don't know the
    world), one strategy y_w per world for them (they know their own set). Ms: [W, n, m] payoffs for us, p: world weights.
    Regret matching+ with linear averaging. Returns x, Y [W, m], value, rows (our candidates' values against the Y's)."""
    Ms = np.asarray(Ms, np.float64)
    W, n, m = Ms.shape
    p = np.full(W, 1.0 / W) if p is None else np.asarray(p, np.float64) / np.sum(p)
    if _RUST_SOLVE:
        x, Y = vgcsim.rm_bayes_solve(Ms.tolist(), p.tolist(), iters)
        x, Y = np.array(x), np.array(Y)
        rows = np.einsum("w,wij,wj->i", p, Ms, Y)
        return x, Y, float(x @ rows), rows
    rx, ry = np.zeros(n), np.zeros((W, m))
    sx, sy = np.zeros(n), np.zeros((W, m))
    for t in range(1, iters + 1):
        x = rx / rx.sum() if rx.sum() > 0 else np.full(n, 1.0 / n)
        s = ry.sum(1, keepdims=True)
        Y = np.where(s > 0, ry / np.maximum(s, 1e-30), 1.0 / m)
        ux = np.einsum("w,wij,wj->i", p, Ms, Y)
        uy = -np.einsum("i,wij->wj", x, Ms)
        rx = np.maximum(rx + ux - x @ ux, 0.0)
        ry = np.maximum(ry + uy - (Y * uy).sum(1, keepdims=True), 0.0)
        sx += t * x
        sy += t * Y
    x = sx / sx.sum()
    Y = sy / sy.sum(1, keepdims=True)
    rows = np.einsum("w,wij,wj->i", p, Ms, Y)
    return x, Y, float(x @ rows), rows


def two_sided_solve(Ms, pt, iters=1000):
    """(2026-11-03, our types) Zero-sum game where BOTH sides have hidden types: Ms [T, W, n, m] payoffs for us, T = our
    types (which two we left at home; type 0 = the true one), W = their set worlds (uniform). We pick x_t knowing our type,
    they pick y_w knowing their world. pt: our type weights. Regret matching+ with linear averaging.
    Returns X [T, n], Y [W, m], value, rows of type 0 (our candidates' values against the Y's)."""
    Ms = np.asarray(Ms, np.float64)
    T, W, n, m = Ms.shape
    pt = np.asarray(pt, np.float64) / np.sum(pt)
    pw = np.full(W, 1.0 / W)
    rx, ry = np.zeros((T, n)), np.zeros((W, m))
    sx, sy = np.zeros((T, n)), np.zeros((W, m))
    for it in range(1, iters + 1):
        a = rx.sum(1, keepdims=True)
        X = np.where(a > 0, rx / np.maximum(a, 1e-30), 1.0 / n)
        b = ry.sum(1, keepdims=True)
        Y = np.where(b > 0, ry / np.maximum(b, 1e-30), 1.0 / m)
        ux = np.einsum("w,twij,wj->ti", pw, Ms, Y)                     # per type (conditional on t)
        uy = -np.einsum("t,ti,twij->wj", pt, X, Ms)                    # per world (conditional on w)
        rx = np.maximum(rx + ux - (X * ux).sum(1, keepdims=True), 0.0)
        ry = np.maximum(ry + uy - (Y * uy).sum(1, keepdims=True), 0.0)
        sx += it * X
        sy += it * Y
    X = sx / sx.sum(1, keepdims=True)
    Y = sy / sy.sum(1, keepdims=True)
    rows = np.einsum("w,wij,wj->i", pw, Ms[0], Y)
    V = float(np.einsum("t,ti,w,twij,wj->", pt, X, pw, Ms, Y))
    return X, Y, V, rows


def greedy(net, battle, side, device, hidden=False):
    """The network's most likely legal joint action."""
    lg, _ = net_logits(net, battle, [side], device, hidden)
    return top_joint(lg[0], battle.legal_index_map(side), 1)[0][1:]


def search(net, battle, side, device, worlds=16, k=8, hidden=False, seed=1, mode="mixed", rng=None, info=None, opp="self"):
    """Our joint action (two action tuples) chosen by one-turn search. `opp`: where the opponent's top-k
    replies come from: "self" = our network run from their side (needs their information: fine with open
    team sheets), "head" = the opponent-prediction head from OUR view (honest with closed team sheets)."""
    if battle.phase != "turn" or not battle.needs_choice(1 - side):
        return greedy(net, battle, side, device, hidden)
    if opp == "head":
        f, i = vgcsim.encode_battle(battle, side, hidden=hidden)
        with torch.no_grad(), amp():
            l0, _, ol = net(torch.tensor(np.array([f], np.float32), device=device),
                            torch.tensor(np.array([i], np.int32), device=device), opp=True)
        lg = np.stack([l0[0].float().cpu().numpy(), ol[0].float().cpu().numpy()])
    else:
        lg, _ = net_logits(net, battle, [side, 1 - side], device, hidden)
    ours = top_joint(lg[0], battle.legal_index_map(side), k)
    if len(ours) == 1:
        return ours[0][1:]
    import sh_search
    theirs = top_joint(lg[1], sh_search.their_maps(battle, side, hidden), k)   # (2026-10-28, leaks)
    if hasattr(battle, "search_leaves_bytes"):     # (2026-10-28, speed pass 3c) features as bytes, not lists
        f, i, term = battle.search_leaves_bytes(side, [o[1:] for o in ours], [t[1:] for t in theirs], worlds=worlds, seed=seed, hidden=hidden)
        term = np.array(term, np.float32)
        vals = term.copy()
        live = np.flatnonzero(np.isnan(term))
        F = np.frombuffer(f, np.float32).reshape(len(term), -1)[live]
        I = np.frombuffer(i, np.int32).reshape(len(term), -1)[live]
    else:
        f, i, term = battle.search_leaves(side, [o[1:] for o in ours], [t[1:] for t in theirs], worlds=worlds, seed=seed, hidden=hidden)
        term = np.array(term, np.float32)
        vals = term.copy()
        live = np.flatnonzero(np.isnan(term))
        F = np.asarray(f, np.float32).reshape(len(term), -1)[live]
        I = np.asarray(i, np.int32).reshape(len(term), -1)[live]
    if len(live):
        out = []
        with torch.no_grad(), amp():
            for s in range(0, len(live), 4096):
                out.append(net(torch.from_numpy(F[s:s + 4096]).to(device), torch.from_numpy(I[s:s + 4096]).to(device))[1].float().cpu().numpy())
        vals[live] = np.concatenate(out)
    M = vals.reshape(len(ours), len(theirs), worlds).mean(-1)
    x, y = solve(M.astype(np.float64))
    if mode == "max":
        c = int(np.argmax(x))
    else:
        rng = rng or np.random.default_rng()
        c = int(rng.choice(len(x), p=x / x.sum()))
    if info is not None:
        info.update({"M": M, "x": x, "y": y, "ours": ours, "theirs": theirs, "value": float(x @ M @ y)})
    return ours[c][1:]
