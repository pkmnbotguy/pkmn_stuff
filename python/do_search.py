"""(2026-10-30, Nessie-style search, test 1) DOUBLE-ORACLE root search: the candidate set is NOT bounded by the policy.

Nessie (Smogon post): "the search begins with a double oracle, seeded from a guess (produced by the net) of the most likely
move of each side ... we branch on important chance events ... solve the matrix game". Our halving search instead takes the
policy's top k0 / the opponent head's top K_THEIRS and only ever ADDS one round of their best replies; over 4,916 arena turns
the opponent's real joint action was outside the final game on 20% of turns (the biggest collapse source).

Here:
    seed:   each side's DO_SEED_K most likely legal joint actions (the heads; 3 instead of Nessie's 1 to save rounds),
            evaluated in every world (chance leaves when sh_search.CHANCE is on)
    round:  solve the game on what has been evaluated; SCREEN every legal joint action not yet in the game against the
            other side's current support (cheap: DO_SCREEN_WORLDS sampled rolls, never chance leaves); the DO_ADD best
            responses that beat the game value by DO_MARGIN join the game and get FULL evaluations (fresh worlds: the
            screen picked them for being good, winner's curse) against every candidate of the other side
    stop:   no side has a beating response (converged), DO_MAX_ROUNDS, or the deadline
    final:  solved per set world (bayes_solve: they know their own set) when the worlds are several, else pooled
Result dict = halving_search's keys (ndeep.deepen / sh_search.choose work on it) + do_rounds / do_added_ours / do_added_theirs
/ do_converged / do_screened.
"""

import os
import time
import numpy as np
import sh_search
from nnsearch import bayes_solve, solve, top_joint

SEED_K = int(os.environ.get("DO_SEED_K", "3"))
SCREEN_WORLDS = int(os.environ.get("DO_SCREEN_WORLDS", "4"))
ADD = int(os.environ.get("DO_ADD", "3"))
MARGIN = float(os.environ.get("DO_MARGIN", "0.02"))   # CPU smoke: 0.01 with 2 screen rolls never converged (noise-picked replies)
MAX_ROUNDS = int(os.environ.get("DO_MAX_ROUNDS", "8"))
SUPPORT_MIN = float(os.environ.get("DO_SUPPORT_MIN", "0.02"))     # a mix's support = candidates above this probability
SUPPORT_MAX = int(os.environ.get("DO_SUPPORT_MAX", "6"))         # ... at most this many (the biggest)
BAYES_K = float(os.environ.get("SH_BAYES_K", "2"))


def _support(x, idx):
    order = np.argsort(-x)
    sup = [idx[i] for i in order if x[i] > SUPPORT_MIN][:SUPPORT_MAX]
    if len(sup) < min(3, len(idx)):
        sup = [idx[i] for i in order[:min(3, len(idx))]]
    w = np.array([x[idx.index(a)] for a in sup], np.float64)
    return sup, w / w.sum()


def double_oracle_search(net, battle, side, device, hidden=False, seed=1, worlds=8, seed_k=None, screen_worlds=None,
                         add=None, margin=None, max_rounds=None, deadline=None, bayes_worlds=True, transform=None):
    t0 = time.time()
    seed_k = SEED_K if seed_k is None else int(seed_k)
    screen_worlds = SCREEN_WORLDS if screen_worlds is None else int(screen_worlds)
    add = ADD if add is None else int(add)
    margin = MARGIN if margin is None else float(margin)
    max_rounds = MAX_ROUNDS if max_rounds is None else int(max_rounds)
    b0 = battle[0] if isinstance(battle, (list, tuple)) else battle
    if sh_search.LEAF_CACHE and sh_search._CACHE["d"] is None:
        sh_search._CACHE["d"], sh_search._CACHE["hits"] = {}, 0
        try:
            out = double_oracle_search(net, battle, side, device, hidden, seed, worlds, seed_k, screen_worlds, add, margin,
                                       max_rounds, deadline, bayes_worlds, transform)
            out["cache_rows"], out["cache_hits"] = len(sh_search._CACHE["d"]), sh_search._CACHE["hits"]
            return out
        finally:
            sh_search._CACHE["d"] = None
    l0, ol, v0, oj = sh_search.root_full(net, b0, side, device, hidden)
    maps = b0.legal_index_map(side)
    ours = top_joint(l0, maps, 10 ** 6)                                  # ALL our legal joints, policy-ordered
    theirs = sh_search.their_joint(ol, oj, sh_search.their_maps(b0, side, hidden), 10 ** 6)   # all of theirs
    no, nt = len(ours), len(theirs)
    S, C = np.zeros((no, nt)), np.zeros((no, nt))
    B = len(battle) if isinstance(battle, (list, tuple)) else 1
    bw = bool(bayes_worlds) and B > 1
    Sw = np.zeros((B, no, nt)) if bw else None
    Cw = np.zeros((B, no, nt)) if bw else None
    stats = {"leaves": 0, "screened": 0}

    def ev(rows, cols, w, s):
        """full evaluation of rows x cols in w worlds (chance leaves when sh_search.CHANCE), added to the sums"""
        if not rows or not cols or w <= 0:
            return
        sub = sh_search.leaves(net, battle, side, [ours[a] for a in rows], [theirs[b] for b in cols], w, s, hidden,
                               device, transform)
        stats["leaves"] += sub.size
        S[np.ix_(rows, cols)] += sub.sum(-1)
        C[np.ix_(rows, cols)] += w
        if bw:
            start = s % B
            ids = np.repeat(np.arange(B), [w // B + (1 if (j - start) % B < w % B else 0) for j in range(B)])
            for j in np.unique(ids):
                m = ids == j
                Sw[j][np.ix_(rows, cols)] += sub[..., m].sum(-1)
                Cw[j][np.ix_(rows, cols)] += int(m.sum())

    def screen(rows, cols, s):
        """cheap sampled-roll values [rows, cols] (never chance leaves, not added to the sums)"""
        was = sh_search.CHANCE
        sh_search.CHANCE = False
        try:
            sub = sh_search.leaves(net, battle, side, [ours[a] for a in rows], [theirs[b] for b in cols], screen_worlds,
                                   s, hidden, device, transform)
        finally:
            sh_search.CHANCE = was
        stats["screened"] += sub.size
        return sub.mean(-1)

    def game(act_o, act_t):
        M = S[np.ix_(act_o, act_t)] / np.maximum(C[np.ix_(act_o, act_t)], 1e-12)
        Mw = None
        if bw:
            Sa, Ca = Sw[np.ix_(range(B), act_o, act_t)], Cw[np.ix_(range(B), act_o, act_t)]
            Mw = (Sa + BAYES_K * M[None]) / (Ca + BAYES_K)
            x, Yw, V, rows = bayes_solve(Mw)
            y = Yw.mean(0)
            return M, x, y, float(V), np.asarray(rows), Mw, Yw
        x, y = solve(M)
        return M, x, y, float(x @ M @ y), M @ y, None, None

    act_o = list(range(min(seed_k, no)))
    act_t = list(range(min(seed_k, nt)))
    ev(act_o, act_t, worlds, seed)
    M, x, y, V, rows, Mw, Yw = game(act_o, act_t)
    rounds, n_add_o, n_add_t, converged = 0, 0, 0, False
    while rounds < max_rounds:
        if deadline is not None and time.time() > deadline:
            break
        rounds += 1
        sup_o, wo = _support(np.asarray(x), act_o)
        sup_t, wt = _support(np.asarray(y), act_t)
        new_t = [b for b in range(nt) if b not in act_t]
        new_o = [a for a in range(no) if a not in act_o]
        best_t, best_o = [], []
        if new_t:
            scr = screen(sup_o, new_t, seed + 104729 * rounds)             # [sup_o, new_t]: value for us
            v_t = wo @ scr
            best_t = [new_t[j] for j in np.argsort(v_t)[:add] if v_t[j] < V - margin]
        if new_o:
            scr = screen(new_o, sup_t, seed + 15485863 * rounds)           # [new_o, sup_t]
            v_o = scr @ wt
            best_o = [new_o[j] for j in np.argsort(-v_o)[:add] if v_o[j] > V + margin]
        if not best_t and not best_o:
            converged = True
            break
        # fresh full evaluations: new columns against every row (old and new), new rows against the old columns
        act_t_new = act_t + best_t
        ev(act_o + best_o, best_t, worlds, seed + 32452843 * rounds)
        ev(best_o, act_t, worlds, seed + 49979687 * rounds)
        act_o, act_t = act_o + best_o, act_t_new
        n_add_o += len(best_o)
        n_add_t += len(best_t)
        M, x, y, V, rows, Mw, Yw = game(act_o, act_t)
    q = {a: float(rows[j]) for j, a in enumerate(act_o)}
    xfull = np.zeros(no)
    xfull[act_o] = x
    with np.errstate(invalid="ignore", divide="ignore"):
        Mall = np.where(C > 0, S / np.maximum(C, 1), np.nan)
    return {"ours": ours, "theirs": theirs, "x": xfull, "survivors": act_o, "value": V, "leaves": stats["leaves"], "q": q,
            "logits": l0, "root_value": v0, "y_survivors": (act_t, np.asarray(y)), "M_all": Mall, "forced": None,
            "C_all": C.copy(), "M_worlds": Mw, "y_worlds": Yw, "opp_logits": ol, "protected_theirs": [],
            "opp_added": n_add_t, "opp_rechecked": 0, "ours_rechecked": n_add_o,
            "opp_table": (sh_search.opp_table(ol, oj) if oj is not None else None),
            "do_rounds": rounds, "do_added_ours": n_add_o, "do_added_theirs": n_add_t, "do_converged": converged,
            "do_screened": stats["screened"], "do_n_ours": no, "do_n_theirs": nt, "seconds": time.time() - t0}
