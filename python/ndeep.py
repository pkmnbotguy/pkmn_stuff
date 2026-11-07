"""Nessie-style recursive best-first deepening (2026-10-28, from the bot makers' Discord: nessiedev's search).

On top of a one-turn halving search result `r` (sh_search.halving_search), grow a tree of positions until a time budget is
spent, always expanding the open position with the highest  reach x entropy(static win chance)  anywhere in the tree
(Nessie: the gap between a position's static value and its searched value grows with the value's entropy; reach = the
probability the line is played under the current equilibria x the chance outcomes along it). Unlike deep3 (one level,
fixed budget of 16 groups), lines keep getting deeper wherever they matter.

  root    : the halving search's surviving (our x their) cells; their turns enumerated into exact chance leaves
            (Battle.chance_leaf_battles) in up to NDEEP_WORLDS set worlds (equal weights); the root game = the halving
            matrix (Bayesian over its worlds when it has M_worlds) + per cell sum P(leaf) (deep - static), like deep3
  turn    : expanding a position = our policy top-k x the opponent head's top-k (k by depth: NDEEP_K "4,3,2"), every
            cell's turn enumerated into chance leaves, each leaf a new open position valued by the value net; the
            position's value = the equilibrium of its matrix of leaf values
  switch  : positions that start with replacements are expanded at once (every option pair -> the position after it)
  backup  : after each batch every expanded position's matrix is rebuilt from its children and re-solved, deepest first
  ended   : terminal values +1 / -1 / 0

Below the root each position is solved as a plain matrix game in its own set world (strategy fusion, as deep3's world-0
version and as Nessie: the root's worlds still keep our root choice from being tailored to one guess).

    d = ndeep.deepen(net, worlds, r, device, hidden=True, seed=1, seconds=4.0)
    d: same keys as a halving result for the pick (ours, x over r["ours"], q, value) + tree stats.
"""

import os
import time
import numpy as np
import torch
import vgcsim
import sh_search
from deep2 import PASS
from nnsearch import bayes_solve, solve, top_joint
from netlib import amp

N_WORLDS = int(os.environ.get("NDEEP_WORLDS", "4"))
K_BY_DEPTH = [int(x) for x in os.environ.get("NDEEP_K", "4,3,2").split(",")]
# (2026-10-28) the opponent's candidates below the root (default = NDEEP_K): more replies than ours counters the
# optimism of solving each node against only the head's few likeliest replies
K_THEIRS_BY_DEPTH = [int(x) for x in os.environ.get("NDEEP_K_THEIRS", os.environ.get("NDEEP_K", "4,3,2")).split(",")]
BATCH = int(os.environ.get("NDEEP_BATCH", "8"))
MIN_PATH = float(os.environ.get("NDEEP_MIN_PATH", "0.1"))
MAX_LEAVES = int(os.environ.get("NDEEP_MAX_LEAVES", "12"))
# (2026-10-28 speed pass 3) transposition cache: ~60% of the rows sent to the net were exact duplicates (the same encoded
# position reached from several cells / lines: identical chance outcomes, Protect turns, misses); a position whose encoded
# row was already evaluated in this search reuses its outputs (identical input = identical output). NDEEP_DEDUP=0 = off.
DEDUP = os.environ.get("NDEEP_DEDUP", "1") == "1"
SWITCH_FIRST = os.environ.get("NDEEP_SWITCH_FIRST", "1") == "1"
SWITCH_VEC = os.environ.get("NDEEP_SWITCH_VEC", "1") == "1"
REACH_POW = float(os.environ.get("NDEEP_REACH_POW", "1"))       # Nessie: reach x entropy; deep3 used reach^2
LAMBDA = float(os.environ.get("NDEEP_LAMBDA", "1"))           # (2026-10-29) share of the deepening correction the root trusts: 1 = Nessie, 0 = xstrong
SW_CAP = int(os.environ.get("NDEEP_SWITCH_CAP", "16"))
MAX_NODES = int(os.environ.get("NDEEP_MAX_NODES", "60000"))
# (2026-10-28) worlds fixed down the tree (NDEEP_KEEP_WORLDS=0 = the first version: every enumeration redraws them)
KEEP_WORLDS = os.environ.get("NDEEP_KEEP_WORLDS", "1") == "1"

# (2026-10-28 profile) seconds per part of the last deepen() call: net, encode, leaves (engine chance enumeration), cands
# (legal maps + top_joint), switch (replacement expansion), backup, reach (reach + priority), other
TIMES = {}


def _tick(key, t0):
    t = time.perf_counter()
    TIMES[key] = TIMES.get(key, 0.0) + t - t0
    return t


def _net(net, F, I, device):
    """(policy logits [n, 2, 38], value [n], opponent logits [n, 2, 38]); one device->host transfer per call"""
    L, V, O = [], [], []
    with torch.no_grad(), amp():
        for s in range(0, len(F), 2048):
            o = net(torch.from_numpy(F[s:s + 2048]).to(device), torch.from_numpy(I[s:s + 2048]).to(device), opp=True)
            n = o[0].shape[0]
            cat = torch.cat([o[0].reshape(n, -1), o[1].reshape(n, 1), o[2].reshape(n, -1)], 1).float().cpu().numpy()
            L.append(cat[:, :76].reshape(n, 2, 38))
            V.append(cat[:, 76])
            O.append(cat[:, 77:].reshape(n, 2, 38))
    if len(L) == 1:
        return L[0], V[0].astype(np.float64), O[0]
    return np.concatenate(L), np.concatenate(V).astype(np.float64), np.concatenate(O)


def _entropy(v):
    p = np.clip((np.asarray(v, np.float64) + 1) / 2, 1e-6, 1 - 1e-6)
    return -(p * np.log(p) + (1 - p) * np.log(1 - p))


_RUST_FLAT = hasattr(vgcsim, "rm_solve_flat") and os.environ.get("VGCSIM_PY_SOLVE", "0") != "1"


def _solve_many(Ms):
    """[(x, y, value)] for many small games"""
    if not Ms:
        return []
    import nnsearch
    if getattr(nnsearch, "_RUST_SOLVE", False):
        out = []
        for M, (x, y) in zip(Ms, vgcsim.rm_solve_batch([np.asarray(M, np.float64).tolist() for M in Ms], 400)):
            x, y = np.asarray(x, np.float64), np.asarray(y, np.float64)
            out.append((x, y, float(x @ np.asarray(M, np.float64) @ y)))
        return out
    res = []
    for M in Ms:
        x, y = solve(np.asarray(M, np.float64), 400)
        res.append((x, y, float(x @ M @ y)))
    return res


def _solve_flat(data, shapes):
    """(xs, ys) concatenated over the games (float64 arrays): the same solver as _solve_many, one call for all of them"""
    shapes = [(int(n), int(m)) for n, m in shapes]
    if _RUST_FLAT:
        xs, ys = vgcsim.rm_solve_flat(np.ascontiguousarray(data, np.float64).tobytes(), shapes, 400)
        return np.frombuffer(xs, np.float64), np.frombuffer(ys, np.float64)
    xs, ys, o = [], [], 0
    for n, m in shapes:
        x, y, _ = _solve_many([data[o:o + n * m].reshape(n, m)])[0]
        xs.append(x)
        ys.append(y)
        o += n * m
    return np.concatenate(xs), np.concatenate(ys)


# (2026-10-28 speed pass) one batched, parallel encode call (vgcsim.encode_battles) instead of one call per position
# (the per-position binding returned Python lists of 1,548 floats: 36% of the deepening time)
_ENC_BATCH = hasattr(vgcsim, "encode_battles") and os.environ.get("NDEEP_ENC_BATCH", "1") == "1"
# (2026-10-28 speed pass 2) replacement positions expanded by the engine in one call (Battle.replacement_children)
_REPL_CH = hasattr(vgcsim.Battle, "replacement_children") and os.environ.get("NDEEP_REPL_CH", "1") == "1"
# (2026-10-28, speed pass 3c) kinds of many positions in one call and batched replacement expansion (Rust)
_KINDS = hasattr(vgcsim, "battle_kinds") and os.environ.get("NDEEP_KINDS", "1") == "1"
_REPL_MANY = _REPL_CH and hasattr(vgcsim, "replacement_children_many") and os.environ.get("NDEEP_REPL_MANY", "1") == "1"
_TOPJ = hasattr(vgcsim.Battle, "top_joint") and os.environ.get("NDEEP_RUST_TOPJ", "1") == "1"   # candidates in Rust
# (2026-10-28, speed pass 3e) the round's chance leaves from one engine call, tree arrays filled with numpy
_LEAF_MANY = _TOPJ and hasattr(vgcsim, "chance_leaf_battles_many") and os.environ.get("NDEEP_LEAF_MANY", "1") == "1"
_REACH_RUST = hasattr(vgcsim, "tree_reach") and os.environ.get("NDEEP_REACH_RUST", "1") == "1"   # reach in one Rust pass
# (2026-11-01) exact endgame leaves: 1v1 turn positions solved by the engine (vgcsim.eg_leaves_config) instead of the net;
# certified ones become closed nodes with the exact value. Off unless EG_LEAVES (env NDEEP_EG_LEAVES=1 or set by the agent).
EG_LEAVES = hasattr(vgcsim, "eg_leaf_values") and os.environ.get("NDEEP_EG_LEAVES", "0") == "1"
# (2026-11-01) uncertified exact leaves still give proven bounds: the net value is clamped into them (NDEEP_EG_CLAMP=1)
EG_CLAMP = EG_LEAVES and hasattr(vgcsim, "eg_leaf_bounds") and os.environ.get("NDEEP_EG_CLAMP", "0") == "1"
_KIND_VAL = np.array([0.0, 0.0, 1.0, -1.0, 0.0])
_KIND_MAP = np.array([0, 1, 2, 2, 2], np.int8)          # Rust kind byte -> KIND_TURN / KIND_SWITCH / KIND_END

KIND_TURN, KIND_SWITCH, KIND_END = 0, 1, 2


class Tree:
    """(2026-10-28 speed pass 2) every per-node number lives in a numpy array grown by doubling: value, static value, its
    entropy, open flag, parent, level (root leaves 0, a child = its parent's + 1), depth, kind, the edge weight
    w = x[a] y[b] p of the cell the node sits in (refreshed when its parent is re-solved), the node's cell index and chance
    weight within its parent, and, for expanded nodes, the children's id range (children are added contiguously) with the
    matrix shape. Nodes are added per batch (add_many), the policy / opponent logits are rows of two arrays, and the
    backup solves every dirty node of a level in ONE Rust call (rm_solve_flat) with the matrices built by one bincount.
    Reach = one vectorized step per level."""

    def __init__(self, net, device, hidden, seed):
        self.net, self.device, self.hidden, self.seed = net, device, hidden, seed
        self.bat = []
        self.n = 0
        cap = 1024
        self.val = np.zeros(cap)
        self.static = np.zeros(cap)
        self.ent = np.zeros(cap)
        self.n_exact = 0
        self.open = np.zeros(cap, bool)      # open turn position (not expanded yet)
        self.par = np.full(cap, -1, np.int64)
        self.w = np.zeros(cap)
        self.lvl = np.zeros(cap, np.int64)
        self.depth = np.zeros(cap, np.int64)
        self.kind = np.zeros(cap, np.int8)
        self.cell = np.zeros(cap, np.int64)   # cell index a * nb + b within the parent's matrix
        self.p = np.zeros(cap)                # chance weight within that cell (normalised)
        self.c0 = np.full(cap, -1, np.int64)  # expanded: children [c0, c1)
        self.c1 = np.full(cap, -1, np.int64)
        self.na = np.zeros(cap, np.int64)
        self.nb = np.zeros(cap, np.int64)
        self.Lm = np.zeros((cap, 2, 38), np.float32)
        self.Om = np.zeros((cap, 2, 38), np.float32)
        self.lev = []          # level -> node ids (level 0 = root leaves)
        self.lev_arr = []      # level -> the same ids as an array (refreshed when the level grew)
        self.reach = np.zeros(0)
        self.n_net = 0
        self.n_dup = 0         # rows served from the transposition cache
        self.seen = {}         # encoded row digest -> node id holding its outputs
        self.max_depth = 0

    def _grow(self, need):
        cap = len(self.val)
        while cap < need:
            cap *= 2
        for name in ("val", "static", "ent", "open", "par", "w", "lvl", "depth", "kind", "cell", "p", "c0", "c1", "na", "nb",
                     "Lm", "Om"):
            a = getattr(self, name)
            b = np.empty((cap,) + a.shape[1:], a.dtype)
            b[:len(a)] = a
            b[len(a):] = -1 if name in ("par", "c0", "c1") else 0
            setattr(self, name, b)

    def add_many(self, battles, depth, parent, cells=None, ps=None, kinds=None):
        """append positions as the children of `parent` (-1 = root leaves); returns their ids as an array.
        `kinds`: the positions' Rust kind bytes (battle_kinds) when the caller has them."""
        m = len(battles)
        k0 = self.n
        if k0 + m > len(self.val):
            self._grow(k0 + m)
        ks = np.arange(k0, k0 + m)
        self.n += m
        self.bat.extend(battles)
        if kinds is None and _KINDS:
            kinds = np.frombuffer(vgcsim.battle_kinds(battles), np.uint8)
        if kinds is not None:
            kinds = np.asarray(kinds, np.uint8)
            kind = _KIND_MAP[kinds]
            v = _KIND_VAL[kinds]
        else:
            kind = np.empty(m, np.int8)
            v = np.zeros(m)
            for n_, c in enumerate(battles):
                ph = c.phase
                if ph == "ended":
                    kind[n_] = KIND_END
                    wn = c.winner
                    v[n_] = 1.0 if wn == 0 else (-1.0 if wn == 1 else 0.0)
                else:
                    kind[n_] = KIND_SWITCH if ph == "switch" else KIND_TURN
        self.kind[ks] = kind
        self.val[ks] = v
        self.static[ks] = v
        self.ent[ks] = 0.0
        self.depth[ks] = depth
        self.par[ks] = parent
        lv = 0 if parent < 0 else int(self.lvl[parent]) + 1
        self.lvl[ks] = lv
        while len(self.lev) <= lv:
            self.lev.append([])
            self.lev_arr.append(np.zeros(0, np.int64))
        self.lev[lv].extend(range(k0, k0 + m))
        self.open[ks] = kind == KIND_TURN
        if cells is not None:
            self.cell[ks] = cells
            self.p[ks] = ps
        self.max_depth = max(self.max_depth, depth)
        return ks

    def _dedup(self, turn, F, I, fb=None, ib=None):
        """serve rows already evaluated in this search from the node that holds them; returns the rows still to run"""
        n = len(turn)
        # 128-bit key per row (Rust when available, else two random linear hashes mod 2**64 in numpy)
        key = sh_search.row_keys(F, I, fb, ib)
        _, first, inv = np.unique(key, return_index=True, return_inverse=True)
        inv = inv.ravel()
        run, copy_from, copy_to = [], [], []
        for u, i in enumerate(first):                     # one representative per distinct row in this call
            d = key[i].tobytes()
            src = self.seen.get(d)
            if src is None:
                self.seen[d] = int(turn[i])
                run.append(i)
            else:
                copy_from.append(src)
                copy_to.append(i)
        run = np.asarray(run, np.int64)
        # rows of this call that are duplicates of a representative (same call) or of an earlier node
        rep = first[inv]                                   # each row's representative row
        dup = np.nonzero(rep != np.arange(n))[0]
        if len(copy_to):
            cf, ct = np.asarray(copy_from), turn[np.asarray(copy_to)]
            self.static[ct] = self.val[ct] = self.static[cf]
            self.ent[ct] = self.ent[cf]
            self.Lm[ct] = self.Lm[cf]
            self.Om[ct] = self.Om[cf]
        self.n_dup += len(copy_to) + len(dup)
        if len(dup):
            self._pending = (turn[dup], turn[rep[dup]])   # copied after the net call (their representative may run now)
        else:
            self._pending = None
        return turn[run], F[run], I[run]

    def _flush_dup(self):
        if getattr(self, "_pending", None) is not None:
            ct, cf = self._pending
            self.static[ct] = self.val[ct] = self.static[cf]
            self.ent[ct] = self.ent[cf]
            self.Lm[ct] = self.Lm[cf]
            self.Om[ct] = self.Om[cf]
            self._pending = None

    def evaluate(self, ids):
        """value net + policy on new turn positions; switch positions are expanded at once (their children evaluated).
        (2026-10-28, speed pass 3) switch positions are expanded BEFORE the net call, so their children (and their
        children's, for a switch after a switch) join the same call instead of making an extra small one
        (NDEEP_SWITCH_FIRST=0 = the old order: net call first, then the switches' children in a second call)."""
        ids = np.asarray(ids, np.int64)
        while len(ids):
            kinds = self.kind[ids]
            turn = ids[kinds == KIND_TURN]
            sw = ids[kinds == KIND_SWITCH]
            if SWITCH_FIRST:
                t0 = time.perf_counter()
                parts = [turn]
                while len(sw):
                    if _REPL_MANY:
                        kids = self.expand_switches(sw)
                    else:
                        new = [self.expand_switch(int(k)) for k in sw]
                        kids = np.concatenate(new) if new else np.zeros(0, np.int64)
                    parts.append(kids[self.kind[kids] == KIND_TURN])
                    sw = kids[self.kind[kids] == KIND_SWITCH]
                turn = np.concatenate(parts)
                _tick("switch", t0)
            clamp = None
            if EG_LEAVES and len(turn):
                t0 = time.perf_counter()
                if EG_CLAMP:
                    eb3 = np.frombuffer(vgcsim.eg_leaf_bounds([self.bat[k] for k in turn]), np.float32).reshape(-1, 3)
                    ev = eb3[:, 0]
                    cl = np.isnan(ev) & ((eb3[:, 1] > -1.0) | (eb3[:, 2] < 1.0))
                    clamp = (turn[cl], eb3[cl, 1], eb3[cl, 2]) if cl.any() else None
                else:
                    ev = np.frombuffer(vgcsim.eg_leaf_values([self.bat[k] for k in turn]), np.float32)
                    clamp = None
                ok = ~np.isnan(ev)
                if ok.any():
                    ks = turn[ok]
                    self.static[ks] = self.val[ks] = ev[ok]
                    self.ent[ks] = 0.0
                    self.kind[ks] = KIND_END
                    self.open[ks] = False
                    self.n_exact += int(ok.sum())
                    turn = turn[~ok]
                _tick("eg", t0)
            if len(turn):
                t0 = time.perf_counter()
                fb = ib = None
                if _ENC_BATCH:
                    fb, ib = vgcsim.encode_battles([self.bat[k] for k in turn], 0, hidden=bool(self.hidden))
                    F = np.frombuffer(fb, np.float32).reshape(len(turn), -1)
                    I = np.frombuffer(ib, np.int32).reshape(len(turn), -1)
                else:
                    F, I = [], []
                    for k in turn:
                        f, ii = vgcsim.encode_battle(self.bat[k], 0, hidden=self.hidden)
                        F.append(f)
                        I.append(ii)
                    F = np.asarray(F, np.float32).reshape(len(F), -1)
                    I = np.asarray(I, np.int32).reshape(len(I), -1)
                t0 = _tick("encode", t0)
                if DEDUP:
                    turn, F, I = self._dedup(turn, F, I, fb, ib)
                    t0 = _tick("dedup", t0)
                if len(turn):
                    L, V, O = _net(self.net, F, I, self.device)
                    t0 = _tick("net", t0)
                    self.n_net += len(turn)
                    self.static[turn] = self.val[turn] = V
                    self.ent[turn] = _entropy(V)
                    self.Lm[turn] = L
                    self.Om[turn] = O
                if DEDUP:
                    self._flush_dup()
                if EG_LEAVES and clamp is not None:
                    ck, clo, chi = clamp
                    v0 = self.val[ck]
                    v1 = np.clip(v0, clo, chi)
                    self.n_clamped = getattr(self, "n_clamped", 0) + int((v1 != v0).sum())
                    self.static[ck] = self.val[ck] = v1
                    self.ent[ck] = _entropy(v1)
            if SWITCH_FIRST:
                break
            t0 = time.perf_counter()
            new = [self.expand_switch(int(k)) for k in sw]
            ids = np.concatenate(new) if new else np.zeros(0, np.int64)
            _tick("switch", t0)

    def expand_switch(self, k):
        c = self.bat[k]
        if _REPL_CH:
            na, nb, kids = c.replacement_children(SW_CAP)
        else:
            z = [np.zeros(38), np.zeros(38)]
            oo = top_joint(z, c.legal_index_map(0), SW_CAP) if c.needs_choice(0) else [(1.0, PASS, PASS)]
            tt = top_joint(z, c.legal_index_map(1), SW_CAP) if c.needs_choice(1) else [(1.0, PASS, PASS)]
            kids = []
            for (_, o0, o1) in oo:
                for (_, t0, t1) in tt:
                    cc = c.copy()
                    if cc.needs_choice(0):
                        cc.choose(0, o0, o1)
                    if cc.needs_choice(1):
                        cc.choose(1, t0, t1)
                    kids.append(cc)
            na, nb = len(oo), len(tt)
        ks = self.add_many(kids, int(self.depth[k]), k, np.arange(na * nb), np.ones(na * nb))   # no turn of depth used
        self.set_cells(k, na, nb, ks)
        return ks[self.kind[ks] != KIND_END]

    def expand_switches(self, sw):
        """(2026-10-28, speed pass 3c) expand_switch for many replacement positions: one Rust call builds every child
        (same children, same order as expand_switch); returns the non-terminal children's ids.
        (speed pass 3d) all parents' children are appended in ONE vectorized step (NDEEP_SWITCH_VEC=0 = one add_many per
        parent: ~10k replacement positions per decision made that loop ~5% of the time); ids / order identical."""
        sw = np.asarray(sw, np.int64)
        counts, kinds, kids = vgcsim.replacement_children_many([self.bat[int(k)] for k in sw], SW_CAP)
        counts = np.frombuffer(counts, np.uint8)
        kinds = np.frombuffer(kinds, np.uint8)
        if not SWITCH_VEC:
            out = []
            pos = 0
            for n_, k in enumerate(sw):
                k = int(k)
                na, nb = int(counts[2 * n_]), int(counts[2 * n_ + 1])
                m = na * nb
                ks = self.add_many(kids[pos:pos + m], int(self.depth[k]), k, np.arange(m), np.ones(m), kinds=kinds[pos:pos + m])
                pos += m
                self.set_cells(k, na, nb, ks)
                out.append(ks[self.kind[ks] != KIND_END])
            return np.concatenate(out) if out else np.zeros(0, np.int64)
        na = counts[0::2].astype(np.int64)
        nb = counts[1::2].astype(np.int64)
        ms = na * nb
        M = int(ms.sum())
        if M == 0:
            return np.zeros(0, np.int64)
        starts = np.concatenate([[0], np.cumsum(ms)[:-1]])
        k0 = self.n
        if k0 + M > len(self.val):
            self._grow(k0 + M)
        ks = np.arange(k0, k0 + M)
        self.n += M
        self.bat.extend(kids)
        par = np.repeat(sw, ms)
        kind = _KIND_MAP[kinds]
        v = _KIND_VAL[kinds]
        self.kind[ks] = kind
        self.val[ks] = v
        self.static[ks] = v
        self.ent[ks] = 0.0
        self.depth[ks] = self.depth[par]
        self.par[ks] = par
        lv = self.lvl[par] + 1
        self.lvl[ks] = lv
        for l_ in np.unique(lv):
            while len(self.lev) <= l_:
                self.lev.append([])
                self.lev_arr.append(np.zeros(0, np.int64))
            self.lev[l_].extend(ks[lv == l_].tolist())
        self.open[ks] = kind == KIND_TURN
        self.cell[ks] = ks - np.repeat(k0 + starts, ms)
        self.p[ks] = 1.0
        self.max_depth = max(self.max_depth, int(self.depth[par].max()))
        self.c0[sw] = k0 + starts
        self.c1[sw] = k0 + starts + ms
        self.na[sw] = na
        self.nb[sw] = nb
        self.open[sw] = False
        self.ent[sw] = 0.0
        return ks[kind != KIND_END]

    def set_cells(self, k, na, nb, ks):
        self.c0[k], self.c1[k] = int(ks[0]), int(ks[-1]) + 1
        self.na[k], self.nb[k] = na, nb
        self.open[k] = False
        self.ent[k] = 0.0

    def expand_turns(self, ks, round_seed):
        """expand open turn positions: candidates x chance leaves; returns the new open positions"""
        if _LEAF_MANY and len(ks) > 0:
            return self.expand_turns_many(ks, round_seed)
        new = []
        for n_, k in enumerate(ks):
            t0 = time.perf_counter()
            c = self.bat[k]
            kk = K_BY_DEPTH[min(int(self.depth[k]), len(K_BY_DEPTH) - 1)]
            kt = K_THEIRS_BY_DEPTH[min(int(self.depth[k]), len(K_THEIRS_BY_DEPTH) - 1)]
            if _TOPJ:
                oo = c.top_joint(0, self.Lm[k].tobytes(), kk) if c.needs_choice(0) else [(1.0, PASS, PASS)]
                tt = c.top_joint(1, self.Om[k].tobytes(), kt) if c.needs_choice(1) else [(1.0, PASS, PASS)]
            else:
                oo = top_joint(self.Lm[k], c.legal_index_map(0), kk) if c.needs_choice(0) else [(1.0, PASS, PASS)]
                tt = top_joint(self.Om[k], c.legal_index_map(1), kt) if c.needs_choice(1) else [(1.0, PASS, PASS)]
            cl = [(a, b, 0) for a in range(len(oo)) for b in range(len(tt))]
            t0 = _tick("cands", t0)
            sets = c.chance_leaf_battles(0, [o[1:] for o in oo], [t[1:] for t in tt], cl, worlds=1,
                                         seed=round_seed + 104729 * (n_ + 1), min_path=MIN_PATH, max_leaves=MAX_LEAVES,
                                         hidden=bool(self.hidden) and not KEEP_WORLDS, **_repl_kw(c))
            t0 = _tick("leaves", t0)
            kids, cells, ps = [], [], []
            for c_, ls in enumerate(sets):
                tot = sum(p for _, p in ls) or 1.0
                for cc, p in ls:
                    kids.append(cc)
                    cells.append(c_)
                    ps.append(p / tot)
            ids = self.add_many(kids, int(self.depth[k]) + 1, k, np.asarray(cells, np.int64), np.asarray(ps))
            self.set_cells(k, len(oo), len(tt), ids)
            new.append(ids[self.kind[ids] != KIND_END])
            _tick("tree", t0)
        return np.concatenate(new) if new else np.zeros(0, np.int64)

    def expand_turns_many(self, ks, round_seed):
        """expand_turns with ONE engine call for the whole round (same candidates, cells and seeds as the per-node path;
        the leaves come back flat in position / cell order) and the tree arrays filled with numpy"""
        t0 = time.perf_counter()
        bats, oo_l, tt_l, seeds = [], [], [], []
        for n_, k in enumerate(ks):
            c = self.bat[k]
            kk = K_BY_DEPTH[min(int(self.depth[k]), len(K_BY_DEPTH) - 1)]
            kt = K_THEIRS_BY_DEPTH[min(int(self.depth[k]), len(K_THEIRS_BY_DEPTH) - 1)]
            oo = c.top_joint(0, self.Lm[k].tobytes(), kk) if c.needs_choice(0) else [(1.0, PASS, PASS)]
            tt = c.top_joint(1, self.Om[k].tobytes(), kt) if c.needs_choice(1) else [(1.0, PASS, PASS)]
            bats.append(c)
            oo_l.append([o[1:] for o in oo])
            tt_l.append([t[1:] for t in tt])
            seeds.append(round_seed + 104729 * (n_ + 1))
        t0 = _tick("cands", t0)
        cb, pb, kids, kb = vgcsim.chance_leaf_battles_many(bats, 0, oo_l, tt_l, seeds, MIN_PATH, MAX_LEAVES,
                                                           bool(sh_search.REPL and hasattr(bats[0], "search_leaves_chance_repl")),
                                                           bool(self.hidden) and not KEEP_WORLDS)
        t0 = _tick("leaves", t0)
        na = np.array([len(o) for o in oo_l], np.int64)
        nb = np.array([len(t) for t in tt_l], np.int64)
        ncell = na * nb
        counts = np.frombuffer(cb, np.uint16).astype(np.int64)         # leaves per cell, cells in position order
        ps = np.frombuffer(pb, np.float32).astype(np.float64)
        kinds = np.frombuffer(kb, np.uint8)
        M = len(kids)
        cell_of_leaf = np.repeat(np.arange(len(counts)), counts)       # global cell index per leaf
        node_of_cell = np.repeat(np.arange(len(ks)), ncell)
        cell_start = np.repeat(np.cumsum(ncell) - ncell, ncell)         # each cell's node's first global cell
        tot = np.bincount(cell_of_leaf, weights=ps, minlength=len(counts))
        tot[tot == 0] = 1.0
        ps = ps / tot[cell_of_leaf]
        nl = np.bincount(node_of_cell, weights=counts, minlength=len(ks)).astype(np.int64)   # leaves per node
        k0 = self.n
        if k0 + M > len(self.val):
            self._grow(k0 + M)
        ksn = np.arange(k0, k0 + M)
        self.n += M
        self.bat.extend(kids)
        parn = np.asarray(ks, np.int64)
        par = np.repeat(parn, nl)
        kind = _KIND_MAP[kinds]
        self.kind[ksn] = kind
        self.val[ksn] = _KIND_VAL[kinds]
        self.static[ksn] = _KIND_VAL[kinds]
        self.ent[ksn] = 0.0
        self.depth[ksn] = self.depth[par] + 1
        self.par[ksn] = par
        lv = self.lvl[par] + 1
        self.lvl[ksn] = lv
        for l_ in np.unique(lv):
            l_ = int(l_)
            while len(self.lev) <= l_:
                self.lev.append([])
                self.lev_arr.append(np.zeros(0, np.int64))
            self.lev[l_].extend(ksn[lv == l_].tolist())
        self.open[ksn] = kind == KIND_TURN
        self.cell[ksn] = cell_of_leaf - cell_start[cell_of_leaf]
        self.p[ksn] = ps
        if M:
            self.max_depth = max(self.max_depth, int(self.depth[ksn].max()))
        starts = k0 + np.cumsum(nl) - nl
        self.c0[parn] = starts
        self.c1[parn] = starts + nl
        self.na[parn] = na
        self.nb[parn] = nb
        self.open[parn] = False
        self.ent[parn] = 0.0
        _tick("tree", t0)
        return ksn[kind != KIND_END]

    def backup(self, changed):
        """re-solve the expanded positions on the paths from `changed` up to the root leaves, one level at a time from the
        deepest (a node's children are one level below it, so a level's nodes are independent): every level's matrices
        from one bincount, one Rust solve call; refreshes the edge weights of the re-solved positions' children"""
        t0 = time.perf_counter()
        dirty = set()
        par = self.par
        c0 = self.c0
        for k in changed:
            k = int(k)
            while k >= 0 and k not in dirty:
                if c0[k] >= 0:
                    dirty.add(k)
                k = int(par[k])
        if dirty:
            D = np.fromiter(dirty, np.int64, len(dirty))
            lv = self.lvl[D]
            for l_ in np.unique(lv)[::-1]:
                self._backup_level(D[lv == l_])
        _tick("backup", t0)

    def _backup_level(self, D):
        na, nb = self.na[D], self.nb[D]
        nc = na * nb
        coff = np.cumsum(nc) - nc                      # each node's first cell in the flat matrix buffer
        lens = self.c1[D] - self.c0[D]
        tot = int(lens.sum())
        rep = np.repeat(np.arange(len(D)), lens)       # child -> its node's position in D
        child = np.arange(tot) - np.repeat(np.cumsum(lens) - lens, lens) + np.repeat(self.c0[D], lens)
        g = coff[rep] + self.cell[child]
        ncell = int(nc.sum())
        pv = self.p[child]
        M = np.bincount(g, weights=pv * self.val[child], minlength=ncell)
        cnt = np.bincount(g, minlength=ncell)
        node_of_cell = np.repeat(np.arange(len(D)), nc)
        if (cnt == 0).any():
            empty = cnt == 0
            M[empty] = self.static[D][node_of_cell[empty]]
        xs, ys = _solve_flat(M, zip(na, nb))
        xoff = np.cumsum(na) - na
        yoff = np.cumsum(nb) - nb
        cidx = np.arange(ncell) - coff[node_of_cell]
        nbc = nb[node_of_cell]
        a = cidx // nbc
        b = cidx - a * nbc
        xy = xs[xoff[node_of_cell] + a] * ys[yoff[node_of_cell] + b]
        self.val[D] = np.bincount(node_of_cell, weights=M * xy, minlength=len(D))
        self.w[child] = xy[g] * pv

    def propagate_reach(self, root_ids, root_reach):
        """reach of every position: root leaves get root_reach, a child its parent's reach x its edge weight (one
        vectorized step per level: a node's level is its parent's + 1; or one Rust pass in id order, same products)"""
        if _REACH_RUST:
            n = self.n
            rb = vgcsim.tree_reach(n, self.par[:n].tobytes(), self.w[:n].tobytes(), np.ascontiguousarray(root_ids, np.int64).tobytes(),
                                   np.ascontiguousarray(root_reach, np.float64).tobytes())
            self.reach = np.frombuffer(rb, np.float64).copy()
            return self.reach
        reach = np.zeros(self.n)
        reach[root_ids] = root_reach
        for lv in range(1, len(self.lev)):
            if len(self.lev_arr[lv]) != len(self.lev[lv]):
                self.lev_arr[lv] = np.asarray(self.lev[lv], np.int64)
            ids = self.lev_arr[lv]
            reach[ids] = reach[self.par[ids]] * self.w[ids]
        self.reach = reach
        return reach


def _repl_kw(b):
    return {"repl": True} if sh_search.REPL and hasattr(b, "search_leaves_chance_repl") else {}


def deepen(net, battle, r, device, hidden=False, seed=1, seconds=4.0, deadline=None, max_nodes=None, batch=None):
    t0 = time.time()
    TIMES.clear()
    end = min(deadline if deadline is not None else 1e18, t0 + seconds)
    max_nodes = max_nodes or MAX_NODES
    bt = int(batch or BATCH)                   # positions expanded per round (NDEEP_BATCH unless given)
    blist = list(battle) if isinstance(battle, (list, tuple)) else [battle]
    wl = blist[:max(1, N_WORLDS)]
    if KEEP_WORLDS:
        # each world is already a guess of the opponent (search_worlds): keep its stats and sets fixed down the whole tree
        # (enumerating with hidden=True redrew the unrevealed moves / items and re-guessed the stats at EVERY node)
        wl = [b.copy() for b in wl]
        for b in wl:
            b.set_keep_foe_stats(True)
    pw = 1.0 / len(wl)
    act_o = list(r["survivors"])
    act_t = list(r["y_survivors"][0])
    ours = [r["ours"][a] for a in act_o]
    theirs = [r["theirs"][b] for b in act_t]
    no, nt = len(ours), len(theirs)
    M0 = np.asarray(r["M_all"], np.float64)[np.ix_(act_o, act_t)]
    if np.isnan(M0).any():
        M0 = np.nan_to_num(M0, nan=float(r["value"]))
    Mw0 = r.get("M_worlds")
    use_bw = Mw0 is not None and np.shape(Mw0)[1:] == M0.shape

    def solve_root(corr):
        if use_bw:
            x_, Y_, v_, rows_ = bayes_solve(np.asarray(Mw0) + corr[None])
            return np.asarray(x_), np.asarray(Y_).mean(0), float(v_), np.asarray(rows_)
        M = M0 + corr
        x_, y_ = solve(M)
        return np.asarray(x_), np.asarray(y_), float(x_ @ M @ y_), M @ y_

    T = Tree(net, device, hidden, seed)
    leaves = []                       # root leaves: (i, j, node, P within cell and world)
    cells = [(i, j, 0) for i in range(no) for j in range(nt)]
    tl = time.perf_counter()
    for w, bw in enumerate(wl):
        sets = bw.chance_leaf_battles(0, [o[1:] for o in ours], [t[1:] for t in theirs], cells, worlds=1,
                                      seed=seed + 7919 * w, min_path=MIN_PATH, max_leaves=MAX_LEAVES,
                                      hidden=bool(hidden) and not KEEP_WORLDS, **_repl_kw(bw))
        kids, meta = [], []
        for (i, j, _), ls in zip(cells, sets):
            tot = sum(p for _, p in ls) or 1.0
            for c, p in ls:
                kids.append(c)
                meta.append((i, j, pw * p / tot))
        ids = T.add_many(kids, 1, -1)
        leaves += [(i, j, int(k), p) for (i, j, p), k in zip(meta, ids)]
    _tick("leaves", tl)
    first = [n for (_, _, n, _) in leaves if T.kind[n] != KIND_END]
    T.evaluate(first)
    T.backup(np.nonzero(T.c0[:T.n] >= 0)[0])
    li = np.array([l_[0] for l_ in leaves], np.int64)
    lj = np.array([l_[1] for l_ in leaves], np.int64)
    ln = np.array([l_[2] for l_ in leaves], np.int64)
    lp = np.array([l_[3] for l_ in leaves], np.float64)
    root_static = T.val[ln].copy()                             # the root matrix's own leaves: static = first value
    lcell = li * nt + lj
    corr = np.zeros((no, nt))

    def root_corr():
        corr[:] = np.bincount(lcell, weights=LAMBDA * lp * (T.val[ln] - root_static), minlength=no * nt).reshape(no, nt)
        return corr

    x, y, val, rows = solve_root(root_corr())
    expanded, rounds = 0, 0
    while time.time() < end and T.n < max_nodes:
        tr = time.perf_counter()
        reach = T.propagate_reach(ln, x[li] * y[lj] * lp)
        n_ = T.n
        rp = reach if REACH_POW == 1 else reach ** REACH_POW
        pri = np.where(T.open[:n_], rp * T.ent[:n_], -1.0)
        top = np.argpartition(-pri, bt)[:bt] if n_ > bt else np.arange(n_)
        top = top[np.lexsort((top, -pri[top]))]                 # highest first, ties by id
        order = [int(k) for k in top if pri[k] > 0]
        _tick("reach", tr)
        if not order:
            break
        n_before = T.n
        new = T.expand_turns(order, seed + 31 * rounds + 13)
        T.evaluate(new)
        T.backup(order + [int(k) for k in n_before + np.nonzero(T.c0[n_before:T.n] >= 0)[0]])
        expanded += len(order)
        rounds += 1
        x, y, val, rows = solve_root(root_corr())
    q = {a: float(rows[n]) for n, a in enumerate(act_o)}
    xfull = np.zeros(len(r["ours"]))
    for n, a in enumerate(act_o):
        xfull[a] = x[n]
    depths = T.depth[:T.n][T.c0[:T.n] >= 0]
    out = {"ours": r["ours"], "theirs": r["theirs"], "x": xfull, "x_surv": x, "y": y, "value": val, "q": q,
            "survivors": act_o, "M": M0 + corr, "nodes": T.n, "expanded": expanded, "rounds": rounds,
            "net_evals": T.n_net, "dup_rows": T.n_dup, "exact_leaves": T.n_exact, "max_depth": int(depths.max()) if len(depths) else 0, "root_value": float(r["value"]),
            "seconds": time.time() - t0, "times": dict(TIMES)}
    return out
