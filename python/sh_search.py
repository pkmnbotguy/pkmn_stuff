"""Sequential-halving search for the one-turn simultaneous-move game (after Gumbel MuZero's root search, adapted to two
players choosing at once). Instead of giving every (our action, their action) pair the same number of sampled worlds
(nnsearch.search: top-8 x top-8, 16 worlds each = 1,024 leaves), it starts wide and cheap and spends the rest of the
budget on the contenders:

    round 1: k0 x k0 candidates (by the policy / opponent-prediction head), w1 worlds each
    after each round: solve the game on what has been seen so far; keep the better half of OUR candidates (by value against
             their equilibrium) and the better half of THEIRS (by value against ours), then evaluate the survivors in new
             worlds; averages use every world a pair has been evaluated in
    final: the equilibrium of the survivors

    r = halving_search(net, battle, side, device, hidden=..., seed=..., k0=12, worlds=(2, 4, 8))
    r["x"], r["ours"], r["value"], r["leaves"]
"""

import os
import numpy as np
import torch
import vgcsim
from nnsearch import bayes_solve, solve, top_joint, top_joint_table
from netlib import amp, has_opp_joint, opp_joint_logprobs

N_ACT = 38


def root(net, battle, side, device, hidden):
    """Our logits, the opponent-prediction head's logits (their numbering) and the value, from our view."""
    return root_full(net, battle, side, device, hidden)[:3]


def root_full(net, battle, side, device, hidden):
    """root() plus the opponent joint head's interaction [38, 38] (None for a net without one; netlib.TNet.add_opp_joint)"""
    f, i = vgcsim.encode_battle(battle, side, hidden=hidden)
    oj = None
    with torch.no_grad(), amp():
        tf, ti = torch.tensor(np.array([f], np.float32), device=device), torch.tensor(np.array([i], np.int32), device=device)
        if has_opp_joint(net):
            l0, v, ol, ojt = net(tf, ti, opp=True, opp_joint=True)
            oj = ojt[0].float().cpu().numpy()
        else:
            l0, v, ol = net(tf, ti, opp=True)
    return l0[0].float().cpu().numpy(), ol[0].float().cpu().numpy(), float(v[0]), oj


def ensure_world_sets():
    """(2026-10-28, leaks) closed-sheet worlds of a TRUE position draw the opponent's unknown moves / item / ability / unseen
    Pokemon from the real-set library (encoding.ensure_unc_library); without it the engine keeps the true sets (a leak)"""
    import encoding
    encoding.ensure_unc_library()


def their_maps(b, side, hidden):
    """(2026-10-28, leaks) the opponent's legal actions for their candidates: in a closed-sheet search of a TRUE position
    (not a rebuilt one) as we can know them (Battle.legal_index_map(public=True): no Choice lock / Mega verdict from an
    unknown item, every target form for unrevealed move slots); the true list revealed their hidden item and moves"""
    if hidden and not getattr(b, "keep_foe_stats", True):
        ensure_world_sets()
        try:
            return b.legal_index_map(1 - side, public=True)
        except TypeError:                     # an engine module from before 2026-10-28
            pass
    return b.legal_index_map(1 - side)


def their_joint(ol, oj, maps, k):
    """(2026-10-22) the opponent's k most likely legal joint actions: from the joint table (marginals + interaction) when
    the net has an opponent joint head, else the product of the marginals (top_joint)"""
    if oj is None:
        return top_joint(ol, maps, k)
    return top_joint_table(opp_table(ol, oj), maps, k)


def opp_table(ol, oj):
    """[38, 38] log-probability table of the opponent's pairs (impossible pairs excluded) from numpy marginal logits + interaction"""
    return opp_joint_logprobs(torch.as_tensor(np.asarray(ol, np.float32))[None], torch.as_tensor(np.asarray(oj, np.float32))[None])[0].numpy()


# (2026-10-13) chance nodes: each cell's important random events enumerated with their probabilities
# (Battle.search_leaves_chance) instead of one sampled roll per world. SH_CHANCE=1 turns it on.
CHANCE = os.environ.get("SH_CHANCE", "0") == "1"
MIN_PATH = float(os.environ.get("SH_MIN_PATH", "0.05"))
MAX_LEAVES = int(os.environ.get("SH_MAX_LEAVES", "32"))


# (2026-10-22) end-of-turn replacements inside chance leaves are SOLVED (both sides choose who comes in; the value net scores
# each pair, the small game is solved) instead of drawn 50/50 as chance: the coin flip under-valued positions where we had
# to replace (+0.032 mean, 12% of cases off by > 0.1) and over-valued those where they had to (-0.047, 16%), i.e. the
# search favoured lines where they lose a Pokemon (measured on 205 arena positions). SH_REPL=0 = the old coin flip.
REPL = os.environ.get("SH_REPL", "1") == "1"


def chance_rows(battle, side, ours, theirs, worlds, seed, hidden, min_path, max_leaves):
    """search_leaves_chance(_repl) on joint tuples without the prior: (f, i, term, wt, cell, groups); groups = (group id,
    our option, their option) per row (group -1 = ordinary leaf) or None with the old behaviour / an old module"""
    if REPL and hasattr(battle, "search_leaves_chance_repl"):
        f, i, term, wt, cell, grp, ga, gb = battle.search_leaves_chance_repl(
            side, ours, theirs, worlds=worlds, seed=seed, hidden=hidden, min_path=min_path, max_leaves=max_leaves)
        u8 = lambda x: (np.frombuffer(x, np.uint8) if isinstance(x, (bytes, bytearray)) else np.asarray(x)).astype(np.int64)
        return f, i, term, wt, cell, (np.asarray(grp, np.int64), u8(ga), u8(gb))
    f, i, term, wt, cell = battle.search_leaves_chance(side, ours, theirs, worlds=worlds, seed=seed, hidden=hidden,
                                                       min_path=min_path, max_leaves=max_leaves)
    return f, i, term, wt, cell, None


def solve_groups(vals, wt, cell, groups):
    """each replacement group's rows -> the solved value of its (our replacement x their replacement) game, and the
    parent's weight shared out over the rows (so the group counts once)"""
    vals = np.asarray(vals, np.float64)
    wt = np.asarray(wt, np.float64)
    if groups is None:
        return vals, wt
    grp, ga, gb = groups
    idx = np.flatnonzero(grp >= 0)
    if not len(idx):
        return vals, wt
    key = np.asarray(cell, np.int64)[idx] * 65536 + grp[idx]
    _, inv = np.unique(key, return_inverse=True)
    members = [[] for _ in range(int(inv.max()) + 1)]
    for r, g in zip(idx, inv):
        members[g].append(r)
    Ms = []
    for mem in members:
        M = np.full((int(ga[mem].max()) + 1, int(gb[mem].max()) + 1), np.nan)
        M[ga[mem], gb[mem]] = vals[mem]
        if np.isnan(M).any():
            M = np.where(np.isnan(M), np.nanmean(M), M)
        Ms.append(M)
    import deep2
    sol = deep2.batch_solve(Ms)
    vals, wt = vals.copy(), wt.copy()
    for mem, v in zip(members, sol):
        vals[mem] = float(v)
        wt[mem] = wt[mem] / len(mem)
    return vals, wt


_HW = {}


def hash_weights(m):
    """(2026-10-28, speed pass 3) odd uint64 multipliers for the 128-bit row keys (two linear hashes mod 2**64)"""
    w = _HW.get(m)
    if w is None:
        g = np.random.default_rng(0x5EED)
        w = _HW[m] = g.integers(1, 2 ** 63, size=(2, m), dtype=np.uint64) * np.uint64(2) + np.uint64(1)
    return w


_RUST_KEYS = hasattr(vgcsim, "row_keys") and os.environ.get("SH_RUST_KEYS", "1") == "1"


def row_keys(F, I, fb=None, ib=None):
    """one 16-byte key per encoded row (the bits of obs_f + obs_i): identical input = identical network output.
    (2026-10-28, speed pass 3c) hashed in Rust (vgcsim.row_keys, parallel) when the module has it; `fb` / `ib` = the rows'
    little-endian bytes when the caller already holds them (encode_battles / search_leaves_bytes output: no copy)."""
    n = len(F)
    if _RUST_KEYS:
        if fb is None:
            fb = np.ascontiguousarray(F, np.float32).tobytes()
        if ib is None:
            ib = np.ascontiguousarray(I, np.int32).tobytes()
        return np.frombuffer(vgcsim.row_keys(fb, ib, n), np.dtype((np.void, 16)))
    raw = np.concatenate([np.ascontiguousarray(F, np.float32).view(np.uint32),
                          np.ascontiguousarray(I, np.int32).view(np.uint32)], 1).astype(np.uint64)
    W = hash_weights(raw.shape[1])
    return np.ascontiguousarray(np.stack([raw @ W[0], raw @ W[1]], 1)).view(np.dtype((np.void, 16))).ravel()


# (2026-10-28, speed pass 3) leaf-value cache for ONE search: a probe found 27% of the rows the halving search sends to
# the value net are exact duplicates (the same position after the turn reached from several cells / rounds / worlds).
# halving_search opens a cache (a dict: row key -> value) and closes it at the end; net_values serves rows from it.
# SH_LEAF_CACHE=0 = off. The dict is per call, so two searches never share values.
LEAF_CACHE = os.environ.get("SH_LEAF_CACHE", "1") == "1"
_CACHE = {"d": None, "hits": 0}


def _net_values_raw(net, F, I, device):
    out = []
    with torch.no_grad(), amp():
        for s in range(0, len(F), 4096):
            out.append(net(torch.from_numpy(F[s:s + 4096]).to(device), torch.from_numpy(I[s:s + 4096]).to(device))[1]
                       .float().cpu().numpy())
    return np.concatenate(out).astype(np.float64)


def net_values(net, F, I, device):
    cache = _CACHE["d"]
    if cache is None or len(F) == 0:
        return _net_values_raw(net, F, I, device)
    keys = row_keys(F, I)
    out = np.empty(len(F), np.float64)
    need = {}
    for r in range(len(F)):
        k = keys[r].tobytes()
        v = cache.get(k)
        if v is None:
            need.setdefault(k, []).append(r)
        else:
            out[r] = v
    _CACHE["hits"] += len(F) - sum(len(rs) for rs in need.values())
    if need:
        first = np.array([rs[0] for rs in need.values()], np.int64)
        v = _net_values_raw(net, F[first], I[first], device)
        for (k, rs), val in zip(need.items(), v):
            cache[k] = val
            out[rs] = val
    return out


def leaves_chance(net, battle, side, ours, theirs, worlds, seed, hidden, device, transform=None,
                  min_path=None, max_leaves=None):
    """Like leaves(), each cell = probability-weighted mean over its enumerated chance outcomes."""
    f, i, term, wt, cell, groups = chance_rows(
        battle, side, [o[1:] for o in ours], [t[1:] for t in theirs], worlds, seed, hidden,
        MIN_PATH if min_path is None else min_path, MAX_LEAVES if max_leaves is None else max_leaves)
    term = np.array(term, np.float64)
    wt = np.array(wt, np.float64)
    cell = np.array(cell, np.int64)
    vals = term.copy() if transform is None else np.where(np.isnan(term), 0.5, np.clip((term + 1.0) / 2.0, 0.0, 1.0))
    live = np.flatnonzero(np.isnan(term))
    if len(live):
        F = np.frombuffer(f, np.float32).reshape(len(term), -1)[live]
        I = np.frombuffer(i, np.int32).reshape(len(term), -1)[live]
        v = net_values(net, F, I, device)
        vals[live] = transform(v) if transform is not None else v
    vals, wt = solve_groups(vals, wt, cell, groups)
    n = len(ours) * len(theirs) * worlds
    num = np.bincount(cell, weights=wt * vals, minlength=n)
    den = np.bincount(cell, weights=wt, minlength=n)
    return (num / np.maximum(den, 1e-12)).reshape(len(ours), len(theirs), worlds)


# (2026-10-16, Ataraxos idea 2) deeper leaves: after the searched turn the network plays BOTH sides for this many more
# turns (actions sampled from its policy), then the value net. 0 = off (value net right after the turn).
ROLLOUT_TURNS = int(os.environ.get("SH_ROLLOUT_TURNS", "0"))


def leaves_rollout(net, battle, side, ours, theirs, worlds, seed, hidden, device, transform=None, turns=None):
    turns = ROLLOUT_TURNS if turns is None else turns
    R = battle.search_children(side, [o[1:] for o in ours], [t[1:] for t in theirs], worlds=worlds, seed=seed, hidden=bool(hidden))
    n = len(R)
    t0, _ = R.status()
    limit = np.array(t0, np.int64) + turns
    g = torch.Generator(device=device)
    g.manual_seed(int(seed) & 0x7FFFFFFF)
    for _ in range(4 * turns + 4):
        tn, term = R.status()
        go = np.isnan(np.array(term, np.float64)) & (np.array(tn, np.int64) < limit)
        if not go.any():
            break
        f, i, m, need = R.observe(hidden)
        F = np.frombuffer(f, np.float32).reshape(2 * n, -1)
        I = np.frombuffer(i, np.int32).reshape(2 * n, -1)
        M = np.frombuffer(m, np.uint8).reshape(2 * n, 2, N_ACT)
        rows = np.flatnonzero((np.frombuffer(need, np.uint8) > 0) & np.repeat(go, 2))
        acts = np.zeros((n, 2, 2), np.int32)
        if len(rows):
            out = []
            with torch.no_grad(), amp():
                for s0 in range(0, len(rows), 4096):
                    r = rows[s0:s0 + 4096]
                    lg = net(torch.from_numpy(F[r]).to(device), torch.from_numpy(I[r]).to(device))[0].float()
                    lg = lg.masked_fill(torch.from_numpy(M[r] == 0).to(device), -1e9)
                    gum = -torch.log(-torch.log(torch.rand(lg.shape, generator=g, device=device).clamp_(1e-9, 1 - 1e-9)))
                    out.append((lg + gum).argmax(-1).cpu().numpy())
            a = np.concatenate(out)
            acts.reshape(2 * n, 2)[rows] = a
        acts[~go, 0, 0] = -2
        R.act(acts.reshape(-1).tolist())
    _, term = R.status()
    term = np.array(term, np.float64)
    vals = term.copy() if transform is None else np.where(np.isnan(term), 0.5, np.clip((term + 1.0) / 2.0, 0.0, 1.0))
    live = np.flatnonzero(np.isnan(term))
    if len(live):
        f, i = R.encode0(hidden)
        F = np.frombuffer(f, np.float32).reshape(n, -1)[live]
        I = np.frombuffer(i, np.int32).reshape(n, -1)[live]
        v = net_values(net, F, I, device)
        vals[live] = transform(v) if transform is not None else v
    return vals.reshape(len(ours), len(theirs), worlds)


_LEAF_BYTES = hasattr(vgcsim.Battle, "search_leaves_bytes") and os.environ.get("SH_LEAF_BYTES", "1") == "1"


def leaves(net, battle, side, ours, theirs, worlds, seed, hidden, device, transform=None):
    """Leaf values [len(ours), len(theirs), worlds]: the value net (or +1 / -1 / 0 at a finished game) after the turn.
    `transform`: applied to the value net's outputs (e.g. calibration to win probabilities; terminals become 1 / 0 / 0.5)."""
    if isinstance(battle, (list, tuple)):
        # several positions (closed sheets: the opponent's unrevealed sets sampled per position): the worlds are dealt out
        # over them in turn, starting at a seed-dependent one, so every round sees different sets
        B = len(battle)
        start = seed % B
        per = [worlds // B + (1 if (j - start) % B < worlds % B else 0) for j in range(B)]
        parts = [leaves(net, battle[j], side, ours, theirs, per[j], seed + 104729 * j, hidden, device, transform)
                 for j in range(B) if per[j] > 0]
        return np.concatenate(parts, axis=-1)
    if ROLLOUT_TURNS > 0:
        return leaves_rollout(net, battle, side, ours, theirs, worlds, seed, hidden, device, transform)
    if CHANCE:
        return leaves_chance(net, battle, side, ours, theirs, worlds, seed, hidden, device, transform)
    if _LEAF_BYTES:
        # (2026-10-28, speed pass 3c) the leaf features as bytes: the list path crossed ~1.5k floats per leaf through Python
        f, i, term = battle.search_leaves_bytes(side, [o[1:] for o in ours], [t[1:] for t in theirs], worlds=worlds, seed=seed, hidden=hidden)
    else:
        f, i, term = battle.search_leaves(side, [o[1:] for o in ours], [t[1:] for t in theirs], worlds=worlds, seed=seed, hidden=hidden)
    term = np.array(term, np.float64)
    if transform is None:
        vals = term.copy()
    else:
        vals = np.where(np.isnan(term), 0.5, np.clip((term + 1.0) / 2.0, 0.0, 1.0))
    live = np.flatnonzero(np.isnan(term))
    if len(live):
        if _LEAF_BYTES:
            F = np.frombuffer(f, np.float32).reshape(len(term), -1)[live]
            I = np.frombuffer(i, np.int32).reshape(len(term), -1)[live]
        else:
            F = np.asarray(f, np.float32).reshape(len(term), -1)[live]
            I = np.asarray(i, np.int32).reshape(len(term), -1)[live]
        v = net_values(net, F, I, device)
        vals[live] = transform(v) if transform is not None else v
    return vals.reshape(len(ours), len(theirs), worlds)


def full_search(net, battle, side, device, hidden=False, seed=1, k=8, worlds=16, transform=None):
    """The plain search (as nnsearch.search with opp="head"): every pair, every world."""
    l0, ol, v0 = root(net, battle, side, device, hidden)
    ours = top_joint(l0, battle.legal_index_map(side), k)
    theirs = top_joint(ol, their_maps(battle, side, hidden), k)
    M = leaves(net, battle, side, ours, theirs, worlds, seed, hidden, device, transform).mean(-1)
    x, y = solve(M)
    return {"ours": ours, "theirs": theirs, "M": M, "x": x, "y": y, "value": float(x @ M @ y),
            "leaves": len(ours) * len(theirs) * worlds, "logits": l0, "root_value": v0}


PROTECT_IDS = {"protect", "wideguard", "quickguard"}
# (2026-10-13) Protect discount: a joint action with Protect is worth this much less (win chance) in the worlds where the
# opponent's joint action also protects (double Protect = both sides wasted the turn). 0 = off (the ladder default).
PROTECT_DISCOUNT = float(os.environ.get("SH_PROTECT_DISCOUNT", "0"))


def protects(battle, side, joint):
    """True when the joint action (one action tuple per slot) uses a Protect-type move."""
    sd = battle.state()["sides"][side]
    for slot, t in enumerate(joint):
        if t[0] != "move" or slot >= len(sd["active"]) or sd["active"][slot] is None:
            continue
        moves = sd["mons"][sd["active"][slot]]["moves"]
        if t[1] < len(moves) and moves[t[1]][0] in PROTECT_IDS:
            return True
    return False


# (2026-10-13) opponent expansion (double-oracle style, after a forum question): rounds of "their best reply to our mix
# among all their legal joint actions"; SH_OPP_EXPAND = rounds (0 = off), _ADD = replies added per round, _MARGIN = how much
# better for them (win-chance units, value scale -1..1) a reply must screen to be added.
# (2026-10-13) SH_BAYES=1: solve the survivors as a Bayesian game (one strategy for us, one per sampled world for them: they
# know their own set) instead of solving the matrix averaged over worlds (which pretends they don't).
BAYES = os.environ.get("SH_BAYES", "0") == "1"
OPP_EXPAND = int(os.environ.get("SH_OPP_EXPAND", "0"))
OPP_EXPAND_ADD = int(os.environ.get("SH_OPP_EXPAND_ADD", "2"))
OPP_EXPAND_MARGIN = float(os.environ.get("SH_OPP_EXPAND_MARGIN", "0.02"))
# (2026-10-21, arena battle 4618) SH_RECHECK=1: a reply of theirs that the halving rounds dropped is re-checked against our
# FINAL mix and comes back (fresh worlds) if it beats their equilibrium. Without it, a reply dropped in round 1 (when our
# side still looked like "switch + Protect") was never compared with the move we ended up picking: Tailwind + Dire Claw
# was valued 0.65 while their dropped Hyper Voice + Moonblast KO'd both of our Pokemon. The expansion step below only
# looks at replies that were never candidates, so it couldn't catch this.
RECHECK = os.environ.get("SH_RECHECK", "1") == "1"
RESTALE = os.environ.get("SH_RESTALE", "1") == "1"     # (2026-10-22) eliminated candidates re-valued vs the final mix
RECHECK_ADD = int(os.environ.get("SH_RECHECK_ADD", "4"))
# (2026-10-22, arena game 5619) OUR side too: with few options the halving ended with ONE candidate of ours (Protect), so the
# final opponent equilibrium only answered Protect (100% on a pointless ally Sucker Punch) and the eliminated options, re-valued
# against that mix (RESTALE), looked far better than the survivor (Heat Wave +0.39 vs -0.33): the pick played them on values
# the opponent was never asked to answer. Now (a) halving keeps at least MIN_OURS of ours, (b) an eliminated option of ours that
# beats the game value against the final mix comes back (fresh worlds against every final reply) and the game is re-solved, as
# their dropped replies already did. SH_RECHECK_OURS=0 / SH_MIN_OURS=1 = the old behaviour.
RECHECK_OURS = os.environ.get("SH_RECHECK_OURS", "1") == "1"
MIN_OURS = int(os.environ.get("SH_MIN_OURS", "2"))
# (2026-10-15, recall audit: the opponent's real reply was outside their top 12 in 25-38% of ladder turns) SH_K_THEIRS = their
# starting candidates (0 = k0), SH_MIN_THEIRS = never halve them below this (0 = the min_theirs argument)
# (2026-10-21, arena loss review) SH_KEEP_SWITCH=1: the opponent's most likely switch-out per slot is a protected candidate
# (added if the head's top K_THEIRS has none, never halved away). Arena bots switch when threatened ~1.5x more often than the
# head expects and 9 of the 20 biggest value swings in a day's reviewed losses were switch-ins we never searched: with the
# switch columns cut at the first halving our values assumed the hit lands on the Pokemon that is leaving.
KEEP_SWITCH = os.environ.get("SH_KEEP_SWITCH", "1") == "1"
K_THEIRS = int(os.environ.get("SH_K_THEIRS", "20"))     # default 20 since 2026-10-15 (test D: 66.2% vs 64.8%; user: keep it)
MIN_THEIRS = int(os.environ.get("SH_MIN_THEIRS", "10"))  # default 10 since 2026-10-15
# (2026-10-28, arena loss review: "the opponent knows its own team"). Over 4,916 xstrong arena turns their real joint action
# was outside the final game on 20% of turns; the largest uncovered class was a switch to a Pokemon we had not seen (288
# turns, 35 value collapses), and switch-ins were in the game only 67% of the time (moves 95%). SH_KEEP_SWITCH_ALL=1: a
# protected switch candidate for EVERY bench Pokemon in each slot (KEEP_SWITCH protects one per slot). SH_BAYES_WORLDS=1:
# the final game is solved per SET WORLD (bayes_solve: one strategy for us, one per world for them: they know which Pokemon
# they brought and their sets, so "switch to the unseen slot" gets weight exactly in the worlds where it is good); dice
# stay averaged within a world, unlike SH_BAYES, whose "worlds" were single evaluations (they knew the rolls; lost on
# 2026-10-13). SH_BAYES_K: pseudo-evaluations shrinking a world's cell toward the pooled estimate (as MCTS_BAYES_K).
# Both can also be given per call (halving_search(keep_switch_all=, bayes_worlds=, min_theirs_force=)) for A/B tests.
KEEP_SWITCH_ALL = os.environ.get("SH_KEEP_SWITCH_ALL", "0") == "1"
BAYES_WORLDS = os.environ.get("SH_BAYES_WORLDS", "0") == "1"
BAYES_K = float(os.environ.get("SH_BAYES_K", "2"))
# (2026-11-03, our types) the opponent doesn't know which two of ours stayed home: SH_OUR_TYPES = how many alternative
# backs (our unrevealed brought Pokemon swapped with the two left at home, Battle.type_variants) the final game is also
# played in; the solve is two-sided Bayesian (x per our type, y per their set world) and we play the true type's x.
# Needs our roster on the position (Battle.set_roster(side, ...)); without it nothing changes. 0 = off.
OUR_TYPES = int(os.environ.get("SH_OUR_TYPES", "0"))


def deep_game(r):
    """(2026-10-31) the root game r's pick is based on: (M [survivors x surviving replies], equilibrium y, our x over the
    survivors, deep). Nessie's deepened matrix (ndeep: r["M"] / r["y"] / r["x_surv"], merged into r) when present, else the
    halving search's one-turn M_all. Before this, the opponent model / situation memory re-valued Nessie results on M_all and
    so threw the deepening away on every turn."""
    act_t = list(r["y_survivors"][0])
    act_o = list(r["survivors"])
    M = r.get("M")
    if M is not None and r.get("y") is not None and np.shape(M) == (len(act_o), len(act_t)) and len(r["y"]) == len(act_t):
        x = r.get("x_surv")
        x = np.asarray(x if x is not None else np.asarray(r["x"])[act_o], np.float64)
        return np.asarray(M, np.float64), np.asarray(r["y"], np.float64), x, True
    Ma = np.asarray(r["M_all"], np.float64)[np.ix_(act_o, act_t)]
    return Ma, np.asarray(r["y_survivors"][1], np.float64), np.asarray(r["x"], np.float64)[act_o], False


def base_y(r):
    """the opponent's equilibrium over r["y_survivors"]'s columns in the game r was solved on (deepened when present)"""
    if r.get("M") is None and "M_all" not in r:     # endgame / book results: no matrix, their mix as solved (old behaviour)
        return np.asarray(r["y_survivors"][1], np.float64)
    return deep_game(r)[1]


def revalue_q(r, mix):
    """(2026-10-22) r's candidate values against an opponent mix over r["y_survivors"]'s columns: survivors and every
    eliminated candidate whose estimates cover >= 90% of the mix (M_all rows, renormalised); the rest are shifted by the
    change of the game value. Returns (q dict, V).
    (2026-10-31) survivors use the game the pick is based on (deep_game: Nessie's deepened matrix when present); eliminated
    candidates use one-turn M_all rows only when the survivors do too (else they'd mix one-turn and deepened values)."""
    act_t = r["y_survivors"][0]
    act_o = r["survivors"]
    mix = np.asarray(mix, np.float64)
    Ma = np.asarray(r["M_all"])[:, act_t]
    Md, _, xs, deep = deep_game(r)
    if not deep:
        Md = np.nan_to_num(Md, nan=float(r["value"]))
    q_new = Md @ mix
    V_new = float(xs @ q_new) if xs.sum() > 0 else float(r["value"])
    q = {}
    for c, v in r["q"].items():
        row = Ma[c]
        ok = np.isfinite(row)
        if c not in act_o and RESTALE and not deep and ok.any() and mix[ok].sum() >= 0.9 * mix.sum():
            q[c] = float(row[ok] @ mix[ok] / mix[ok].sum())
        else:
            q[c] = v - r["value"] + V_new
    for j, a in enumerate(act_o):
        q[a] = float(q_new[j])
    return q, V_new


OUR_SWITCH_BONUS = float(os.environ.get("SH_SWITCH_BONUS", "0"))   # (2026-11-03) MW switch test: added to our switch logits (32-35)
OUR_NO_MEGA = set()     # (2026-10-31) slots of side 0 that may not Mega Evolve (set per decision by agent.encode)


def halving_search(net, battle, side, device, hidden=False, seed=1, k0=12, worlds=(2, 4, 8), transform=None,
                   force=None, min_theirs=6, opp_adjust=None, force_theirs=None, keep_switch_all=None, bayes_worlds=None,
                   min_theirs_force=None, min_ours=None, k_theirs=None, our_types=None):
    """Sequential halving over both sides' candidates (see the module docstring). worlds[r] = new worlds per surviving pair
    in round r; both candidate lists are halved (rounded up) after every round but the last. `force`: a joint action (two
    action tuples) that is added to our candidates if missing and never eliminated (the game review's played move: its
    value then gets every world). `min_theirs`: the opponent's candidates are never cut below this many (halving them down
    to 2 judged our moves against too narrow a set of replies: the win chances / costs came out biased).
    `min_ours` / `k_theirs` (2026-10-28, wider-root test): per-call overrides of SH_MIN_OURS / SH_K_THEIRS.
    `force_theirs` (2026-10-21, situation memory): opponent joint actions (two action tuples each) added to their
    candidates if missing and never eliminated, so they get every world."""
    b0 = battle[0] if isinstance(battle, (list, tuple)) else battle      # (a list: positions per opponent-set world)
    if LEAF_CACHE and _CACHE["d"] is None:
        _CACHE["d"], _CACHE["hits"] = {}, 0
        try:
            out = halving_search(net, battle, side, device, hidden, seed, k0, worlds, transform, force, min_theirs,
                                 opp_adjust, force_theirs, keep_switch_all, bayes_worlds, min_theirs_force, min_ours, k_theirs,
                                 our_types)
            out["cache_rows"], out["cache_hits"] = len(_CACHE["d"]), _CACHE["hits"]
            return out
        finally:
            _CACHE["d"] = None
    l0, ol, v0, oj = root_full(net, b0, side, device, hidden)
    if opp_adjust is not None:          # (2026-10-20) per-opponent model : their candidates from it
        ol = opp_adjust(ol, b0, side)
    maps = b0.legal_index_map(side)
    if side == 0 and OUR_SWITCH_BONUS:
        l0 = np.array(l0, copy=True)
        l0[:, 32:36] += OUR_SWITCH_BONUS
    if side == 0 and OUR_NO_MEGA:       # (2026-10-31) the agent's MEGA_ONLY rule: no Mega for these slots of ours
        maps = [{i: t for i, t in m.items() if not (t[0] == "move" and t[3] and k in OUR_NO_MEGA)} for k, m in enumerate(maps)]
    ours = top_joint(l0, maps, k0)
    keep = set()
    if force is not None:
        pos = next((c for c, o in enumerate(ours) if (o[1], o[2]) == tuple(force)), None)
        if pos is None:
            ours.append((0.0, force[0], force[1]))
            pos = len(ours) - 1
        keep.add(pos)
    theirs = their_joint(ol, oj, their_maps(b0, side, hidden), (K_THEIRS or k0) if k_theirs is None else int(k_theirs))   # (2026-10-22) joint-aware with an opp joint head
    min_theirs = (MIN_THEIRS or min_theirs) if min_theirs_force is None else int(min_theirs_force)
    ksa = KEEP_SWITCH_ALL if keep_switch_all is None else bool(keep_switch_all)
    prot_t = set()                                 # (2026-10-21) their protected candidates: one switch-out per slot
    if KEEP_SWITCH or ksa:
        maps_t = their_maps(b0, side, hidden)
        import nnsearch
        for k in range(min(2, len(maps_t))):
            sw = {i: a for i, a in maps_t[k].items() if a[0] == "switch"}
            if not sw:
                continue
            # (2026-10-28) SH_KEEP_SWITCH_ALL: one protected switch per bench Pokemon (party index), else any one switch
            targets = sorted({a[1] for a in sw.values()}) if ksa else [None]
            full = other = None
            for tgt in targets:
                j = next((j for j, t in enumerate(theirs)
                          if t[1 + k][0] == "switch" and (tgt is None or t[1 + k][1] == tgt)), None)
                if j is None:
                    # their most likely such switch in slot k with the other slot's most likely action (true probabilities)
                    if full is None:
                        full = nnsearch.slot_probs(ol[k], maps_t[k])
                        o = 1 - k
                        other = (nnsearch.slot_probs(ol[o], maps_t[o]) if o < len(maps_t) and maps_t[o]
                                 else [(0, ("pass", 0, 0, False), 1.0)])
                    p_sw, i_sw, a_sw = max(((pp, i, a) for i, a, pp in full
                                            if a[0] == "switch" and (tgt is None or a[1] == tgt)), key=lambda t: t[0])
                    oth = [t for t in other if not (t[1][0] == "switch" and t[1][1] == a_sw[1])] or other
                    i_o, a_o, p_o = max(oth, key=lambda t: t[2])
                    theirs.append((p_sw * p_o, a_sw, a_o) if k == 0 else (p_sw * p_o, a_o, a_sw))
                    j = len(theirs) - 1
                prot_t.add(j)
    for fj in force_theirs or ():                  # (2026-10-21) joint actions the caller forces in
        fj = (tuple(fj[0]), tuple(fj[1]))
        j = next((j for j, t in enumerate(theirs) if (tuple(t[1]), tuple(t[2])) == fj), None)
        if j is None:
            theirs.append((0.0, fj[0], fj[1]))
            j = len(theirs) - 1
        prot_t.add(j)
    PEN = None
    if PROTECT_DISCOUNT:
        pr_o = np.array([protects(b0, side, o[1:]) for o in ours], np.float64)
        pr_t = np.array([protects(b0, 1 - side, t[1:]) for t in theirs], np.float64)
        PEN = PROTECT_DISCOUNT * np.outer(pr_o, pr_t)                 # our Protect x their Protect, per pair
    S = np.zeros((len(ours), len(theirs)))
    C = np.zeros((len(ours), len(theirs)))
    # (2026-10-28) SH_BAYES_WORLDS: the same sums per SET WORLD (the positions of `battle`) for the final per-world solve.
    # leaves() deals a call's evaluations over the positions in turn (start = seed % B; world j gets per[j] of them, in order).
    B = len(battle) if isinstance(battle, (list, tuple)) else 1
    bw = (BAYES_WORLDS if bayes_worlds is None else bool(bayes_worlds)) and B > 1 and PEN is None
    Sw = np.zeros((B, len(ours), len(theirs))) if bw else None
    Cw = np.zeros((B, len(ours), len(theirs))) if bw else None

    def acc(rows, cols, sub, s, replace=False):
        """add (or replace) the per-world sums of one leaves() call; `s` = the seed that call was given"""
        if not bw:
            return
        w = sub.shape[2]
        start = s % B
        ids = np.repeat(np.arange(B), [w // B + (1 if (j - start) % B < w % B else 0) for j in range(B)])
        if replace:
            Sw[np.ix_(range(B), rows, cols)] = 0.0
            Cw[np.ix_(range(B), rows, cols)] = 0.0
        for j in np.unique(ids):
            m = ids == j
            Sw[j][np.ix_(rows, cols)] += sub[..., m].sum(-1)
            Cw[j][np.ix_(rows, cols)] += int(m.sum())
    act_o, act_t = list(range(len(ours))), list(range(len(theirs)))
    q_last = {}                                    # our candidate -> its latest value against their equilibrium
    n_leaves = 0
    hist = []                                      # per round: (our candidates, theirs, values [o, t, worlds])
    for r, w in enumerate(worlds):
        sub = leaves(net, battle, side, [ours[a] for a in act_o], [theirs[b] for b in act_t], w, seed + 7919 * r, hidden,
                     device, transform)
        hist.append((list(act_o), list(act_t), sub))
        n_leaves += sub.size
        S[np.ix_(act_o, act_t)] += sub.sum(-1)
        C[np.ix_(act_o, act_t)] += w
        acc(act_o, act_t, sub, seed + 7919 * r)
        M = S[np.ix_(act_o, act_t)] / C[np.ix_(act_o, act_t)]
        if PEN is not None:
            M = M - PEN[np.ix_(act_o, act_t)]
        x, y = solve(M)
        rows, cols = M @ y, x @ M
        for j, a in enumerate(act_o):
            q_last[a] = float(rows[j])
        if r < len(worlds) - 1:
            keep_o = min(len(act_o), max(MIN_OURS if min_ours is None else int(min_ours), -(-len(act_o) // 2)))
            keep_t = min(len(act_t), max(min_theirs, -(-len(act_t) // 2)))
            best_o = [act_o[j] for j in np.argsort(-rows)[:keep_o]]
            act_o = best_o + [a for a in act_o if a in keep and a not in best_o]
            kept_t = [act_t[j] for j in np.argsort(cols)[:keep_t]]      # their best = lowest value for us
            act_t = kept_t + [a for a in act_t if a in prot_t and a not in kept_t]
    M = S[np.ix_(act_o, act_t)] / C[np.ix_(act_o, act_t)]
    if PEN is not None:
        M = M - PEN[np.ix_(act_o, act_t)]
    x, y = solve(M)
    V = float(x @ M @ y)
    n_back, rechecked, n_back_o = 0, set(), 0
    for it in range(3 if RECHECK else 0):
        added = False
        cols_y = list(act_t)                               # the columns the current y covers (act_t may grow below)
        dropped = [b for b in range(len(theirs)) if b not in act_t and np.all(C[act_o, b] > 0)]
        back = []
        if dropped:
            Md = S[np.ix_(act_o, dropped)] / C[np.ix_(act_o, dropped)]
            if PEN is not None:
                Md = Md - PEN[np.ix_(act_o, dropped)]
            v_d = x @ Md                                       # each dropped reply's value for us against our final mix
            back = [dropped[j] for j in np.argsort(v_d)[:RECHECK_ADD] if v_d[j] < V - OPP_EXPAND_MARGIN]
        if back:
            # fresh worlds only (the old estimate picked them for being bad for us: winner's curse)
            extra = int(sum(worlds))
            sub = leaves(net, battle, side, [ours[a] for a in act_o], [theirs[b] for b in back], extra,
                         seed + 32452843 * (it + 1), hidden, device, transform)
            n_leaves += sub.size
            S[np.ix_(act_o, back)] = sub.sum(-1)
            C[np.ix_(act_o, back)] = extra
            acc(act_o, back, sub, seed + 32452843 * (it + 1), replace=True)
            act_t = act_t + back
            rechecked.update(back)
            n_back += len(back)
            added = True
        if RECHECK_OURS:
            # (2026-10-22) our eliminated options against their final mix, over the columns they were evaluated on
            yv = np.asarray(y, np.float64)
            cand_o, v_o = [], []
            for a in range(len(ours)):
                if a in act_o:
                    continue
                ok = C[a, cols_y] > 0
                if ok.any() and yv[ok].sum() >= 0.9 * yv.sum():
                    row = S[a, cols_y][ok] / C[a, cols_y][ok]
                    if PEN is not None:
                        row = row - PEN[a, cols_y][ok]
                    cand_o.append(a)
                    v_o.append(float(row @ yv[ok] / yv[ok].sum()))
            back_o = [cand_o[j] for j in np.argsort(-np.asarray(v_o))[:RECHECK_ADD]
                      if v_o[j] > V + OPP_EXPAND_MARGIN] if cand_o else []
            if back_o:
                extra = int(sum(worlds))                      # fresh worlds against every final reply (winner's curse)
                sub = leaves(net, battle, side, [ours[a] for a in back_o], [theirs[b] for b in act_t], extra,
                             seed + 49979687 * (it + 1), hidden, device, transform)
                n_leaves += sub.size
                S[np.ix_(back_o, act_t)] = sub.sum(-1)
                C[np.ix_(back_o, act_t)] = extra
                acc(back_o, act_t, sub, seed + 49979687 * (it + 1), replace=True)
                act_o = act_o + back_o
                n_back_o += len(back_o)
                added = True
        if not added:
            break
        M = S[np.ix_(act_o, act_t)] / C[np.ix_(act_o, act_t)]
        if PEN is not None:
            M = M - PEN[np.ix_(act_o, act_t)]
        x, y = solve(M)
        V = float(x @ M @ y)
    n_exp = 0
    for it in range(OPP_EXPAND):
        # (2026-10-13) double-oracle-style step: their best reply to our current mix among ALL their legal joint actions
        # (one cheap world each); a reply outside the prediction head's top k0 that beats their equilibrium joins the game
        have = {(t[1], t[2]) for t in theirs}
        new = [t for t in their_joint(ol, oj, their_maps(b0, side, hidden), 10 ** 6) if (t[1], t[2]) not in have]
        if not new or PEN is not None:
            break
        scr = leaves(net, battle, side, [ours[a] for a in act_o], new, 1, seed + 104729 * (it + 1), hidden, device,
                     transform)[..., 0]
        n_leaves += scr.size
        v_new = x @ scr                                       # each new reply's value for us against our mix
        best = [j for j in np.argsort(v_new)[:OPP_EXPAND_ADD] if v_new[j] < V - OPP_EXPAND_MARGIN]
        if not best:
            break
        k = len(theirs)
        theirs = theirs + [new[j] for j in best]
        S = np.hstack([S, np.zeros((len(ours), len(best)))])
        C = np.hstack([C, np.zeros((len(ours), len(best)))])
        if bw:
            Sw = np.concatenate([Sw, np.zeros((B, len(ours), len(best)))], axis=2)
            Cw = np.concatenate([Cw, np.zeros((B, len(ours), len(best)))], axis=2)
        # the screening world is NOT counted (it was picked for being bad for us: winner's curse); fresh worlds instead
        extra = int(sum(worlds))                              # the new columns get as many worlds as the survivors
        if extra > 0:
            sub = leaves(net, battle, side, [ours[a] for a in act_o], [theirs[j] for j in range(k, len(theirs))], extra,
                         seed + 15485863 * (it + 1), hidden, device, transform)
            n_leaves += sub.size
            S[np.ix_(act_o, list(range(k, len(theirs))))] += sub.sum(-1)
            C[np.ix_(act_o, list(range(k, len(theirs))))] += extra
            acc(act_o, list(range(k, len(theirs))), sub, seed + 15485863 * (it + 1))
        act_t = act_t + list(range(k, len(theirs)))
        n_exp += len(best)
        M = S[np.ix_(act_o, act_t)] / C[np.ix_(act_o, act_t)]
        x, y = solve(M)
        V = float(x @ M @ y)
    rows_final = M @ y
    Mw = None
    Yw = None
    n_types, types_used = 1, 0
    ot = OUR_TYPES if our_types is None else int(our_types)
    if ot > 0 and PEN is None:
        # (2026-11-03) our hidden back: the final game in alternative types of ours too (fresh worlds, as many per type as
        # the survivors had), solved two-sided; their y can't depend on which two of ours are at home
        bl = list(battle) if isinstance(battle, (list, tuple)) else [battle]
        var = [b.type_variants(side, ot, seed + 611953) for b in bl]
        n_types = int(var[0][1])
        k_alt = min(len(v[0]) for v in var)
        if k_alt > 0:
            Mtrue = None
            if bw:
                Mtrue = (Sw[np.ix_(range(B), act_o, act_t)] + BAYES_K * M[None]) / (Cw[np.ix_(range(B), act_o, act_t)] + BAYES_K)
            else:
                Mtrue = np.repeat(M[None], B, 0)
            per = max(1, -(-int(sum(worlds)) // B))
            Mts = [Mtrue]
            for t in range(k_alt):
                vl = [v[0][t] for v in var]
                s_t = seed + 86028121 * (t + 1)
                sub = leaves(net, vl if len(vl) > 1 else vl[0], side, [ours[a] for a in act_o], [theirs[b] for b in act_t],
                             per * B, s_t, hidden, device, transform)
                n_leaves += sub.size
                ids = np.repeat(np.arange(B), sub.shape[2] // B)
                Mt = np.stack([sub[..., ids == j].mean(-1) if np.any(ids == j) else sub.mean(-1) for j in range(B)])
                Mts.append(Mt)
            pt = np.array([1.0 / n_types] + [(1.0 - 1.0 / n_types) / k_alt] * k_alt)
            import nnsearch
            X, Yw, V, rows_final = nnsearch.two_sided_solve(np.stack(Mts), pt)
            x = X[0]
            y = Yw.mean(0)
            Mw = Mtrue if bw else None
            if not bw:
                Yw = None
            types_used = k_alt
            if os.environ.get("SH_OUR_TYPES_LOG"):
                print(f"our types: {n_types} possible, {k_alt} searched", flush=True)
            bw = False                   # solved above
    if bw:
        # (2026-10-28) the opponent knows its own set: one strategy for us (we can't tell the set worlds apart), one per
        # set world for them; each world's matrix = that world's own evaluations, shrunk towards the pooled one by BAYES_K
        # pseudo-evaluations (a pair that a world never evaluated keeps the pooled value)
        Sa = Sw[np.ix_(range(B), act_o, act_t)]
        Ca = Cw[np.ix_(range(B), act_o, act_t)]
        Mw = (Sa + BAYES_K * M[None]) / (Ca + BAYES_K)
        x, Yw, V, rows_final = bayes_solve(Mw)
        y = Yw.mean(0)
    elif BAYES and PEN is None:
        # every (round, world) evaluation of the survivors is one world of the Bayesian game; columns a round didn't
        # evaluate (opponent-expansion replies) keep their average there
        Mw = []
        for ao, at, sub in hist:
            # (2026-10-22) our re-checked options (not in this round's rows) keep their fresh average
            ri = [(i, ao.index(a)) for i, a in enumerate(act_o) if a in ao]
            for k in range(sub.shape[2]):
                Mk = M.copy()
                for j, b in enumerate(act_t):
                    if b in at and b not in rechecked:      # re-checked replies: fresh average only
                        for i, ia in ri:
                            Mk[i, j] = sub[ia, at.index(b), k]
                Mw.append(Mk)
        Mw = np.asarray(Mw)
        x, Yw, V, rows_final = bayes_solve(Mw)
        y = Yw.mean(0)
    for j, a in enumerate(act_o):
        q_last[a] = float(rows_final[j])
    if RESTALE:
        # (2026-10-22, arena game 5065) a candidate cut in an early round kept its value against THAT round's opponent mix,
        # which still held replies cut later for being bad for them: inflated, not comparable with the survivors'. Re-valued
        # against the final mix y over the columns it was evaluated on (all of the final ones it shared rounds with).
        yv = np.asarray(y, np.float64)
        for a in range(len(ours)):
            if a in act_o or a not in q_last:
                continue
            ok = C[a, act_t] > 0
            if yv[ok].sum() >= 0.9 * yv.sum() and ok.any():
                q_last[a] = float((S[a, act_t][ok] / C[a, act_t][ok]) @ yv[ok] / yv[ok].sum())
    xfull = np.zeros(len(ours))
    xfull[act_o] = x
    with np.errstate(invalid="ignore", divide="ignore"):
        Mall = np.where(C > 0, S / np.maximum(C, 1), np.nan)            # every pair's estimate (NaN: never evaluated)
    out = {"ours": ours, "theirs": theirs, "x": xfull, "survivors": act_o, "value": V, "leaves": n_leaves, "q": q_last,
           "logits": l0, "root_value": v0, "y_survivors": (act_t, y), "M_all": Mall, "forced": min(keep) if keep else None,
           "C_all": C.copy(),   # (2026-10-28) world evaluations per pair (MCTS uses them as its root prior's weight)
           "opp_added": n_exp, "opp_rechecked": n_back, "ours_rechecked": n_back_o, "M_worlds": Mw, "opp_logits": ol,
           "y_worlds": Yw, "protected_theirs": sorted(prot_t), "our_types": types_used, "n_types": n_types,
           "opp_table": (opp_table(ol, oj) if oj is not None else None)}
    return out


def choose(r, mode="mixed", beta=10.0, rng=None, gamma=1.0):
    """Our joint action (index into r["ours"]) from a halving_search result.
    mixed / max: the survivors' equilibrium (drawn from / its most likely). prior: the policy's preference corrected by the
    search, argmax of log pi(a) + beta (q(a) - V) over the candidates (q = V where never evaluated: the prior alone). With
    a strong policy and few noisy worlds per pair, mixed / max follow lucky estimates; prior moves away from the policy
    only by as much as the search's evidence says."""
    x = np.asarray(r["x"], np.float64)
    if mode == "sample":
        # (2026-10-16, Ataraxos idea 1) DRAW from the policy pulled toward the search: p ~ pi(a)^gamma exp(beta (q(a) - V)).
        # Ataraxos: pi_search ~ [exp(q) magnet^a pi^b]^(1/(a+b)); with a uniform magnet (constant over the candidates) that is
        # beta = 1/(a+b), gamma = b/(a+b) (their a 0.002, b 0.02: beta ~45, gamma ~0.91 for q in [-1, 1])
        V = r["value"]
        z = np.array([gamma * np.log(max(o[0], 1e-12)) + beta * (r["q"].get(c, V) - V) for c, o in enumerate(r["ours"])])
        p = np.exp(z - z.max())
        rng = rng if rng is not None else np.random.default_rng()
        return int(rng.choice(len(p), p=p / p.sum()))
    if mode == "prior":
        V = r["value"]
        score = [np.log(max(o[0], 1e-12)) + beta * (r["q"].get(c, V) - V) for c, o in enumerate(r["ours"])]
        return int(np.argmax(score))
    if mode == "max" or x.sum() <= 0:
        return int(np.argmax(x))
    rng = rng if rng is not None else np.random.default_rng()
    return int(rng.choice(len(x), p=x / x.sum()))


# play / review presets (the game review and the open-source bot): cost in value-net evaluations per decision
PRESETS = {
    "fast": dict(k0=12, worlds=(1, 2, 4)),          # ~300 leaves: laptops without a GPU
    # (2026-10-28) a light preset (--k0 8, worlds 1 2 4)
    "light": dict(k0=8, worlds=(1, 2, 4)),
    "standard": dict(k0=16, worlds=(1, 2, 4, 8)),   # ~600 leaves
    # (2026-10-10, closed sheets) every round sees all 8 sampled opponent sets: 'fast' pruned on ONE sampled set
    "wide": dict(k0=12, worlds=(8, 8, 8)),
    "strong": dict(k0=16, worlds=(2, 4, 8, 16)),    # ~1,200 leaves
    # (2026-10-10) more candidates and twice the worlds of 'strong' (~3,000+ leaves): openings, strong GPUs
    "xstrong": dict(k0=20, worlds=(4, 8, 16, 32)),
    # (2026-10-22) twice xstrong's worlds per round (same candidates): the "longer turns" test
    "xxstrong": dict(k0=20, worlds=(8, 16, 32, 64)),
}


# (2026-10-20) "auto": the biggest preset that fits a time budget per decision on THIS machine (CPU laptop or GPU), from
# the measured time of each preset (moving average); starts at "fast", steps up while the next one is expected to fit
# (unmeasured: 2.2x the current one), steps down when the current one runs over.
AUTO_ORDER = ["fast", "standard", "strong", "xstrong"]


class AutoPreset:
    def __init__(self, budget=3.0):
        self.budget, self.t, self.cur = budget, {}, "fast"

    def pick(self):
        return self.cur

    def record(self, preset, secs):
        self.t[preset] = secs if preset not in self.t else 0.7 * self.t[preset] + 0.3 * secs
        i = AUTO_ORDER.index(self.cur)
        if self.t.get(self.cur, 0) > self.budget and i > 0:
            self.cur = AUTO_ORDER[i - 1]
        elif i + 1 < len(AUTO_ORDER) and self.cur in self.t:
            nxt = AUTO_ORDER[i + 1]
            if self.t.get(nxt, 2.2 * self.t[self.cur]) <= 0.8 * self.budget:
                self.cur = nxt

# ---------------------------------------------------------------------------------------------------------------------
# (2026-10-11) pooled search: the game review's findings applied to play. One halving search's value of a move swung by
# 20-40 points on close positions (which of their replies survived its pruning flipped the equilibrium); runs with
# different seeds pooled, the equilibrium solved over ALL candidates (every run evaluates every pair in its first round),
# optionally smoothed (logit equilibrium, smooth = lambda per unit of value) and against a realistic opponent
# (opp_eq x equilibrium + (1 - opp_eq) x the opponent-prediction head). Same result format as halving_search.

def smooth_equilibrium(M, lam, iters=2000):
    m, n = M.shape
    x, y = np.full(m, 1.0 / m), np.full(n, 1.0 / n)
    for k in range(iters):
        a = lam * (M @ y)
        b = -lam * (x @ M)
        xn = np.exp(a - a.max())
        xn /= xn.sum()
        yn = np.exp(b - b.max())
        yn /= yn.sum()
        step = 1.0 / (k + 2) if k < 200 else 0.005
        dx, dy = xn - x, yn - y
        x, y = x + step * dx, y + step * dy
        if max(np.abs(dx).max(), np.abs(dy).max()) < 1e-6:
            break
    return x, y


def pooled_search(net, battle, side, device, hidden=False, seed=1, runs=4, smooth=0.0, opp_eq=None, **preset):
    rs = [halving_search(net, battle, side, device, hidden, seed=seed + 104729 * k, **preset) for k in range(max(1, runs))]
    r0 = rs[0]
    key = lambda r: [(o[1], o[2]) for o in r["ours"]] + [(o[1], o[2]) for o in r["theirs"]]
    if any(key(r) != key(r0) for r in rs[1:]):
        return r0
    with np.errstate(invalid="ignore"):
        Mall = np.nanmean(np.stack([r["M_all"] for r in rs]), axis=0)
    cols = [j for j in range(len(r0["theirs"])) if np.all(np.isfinite(Mall[:, j]))]
    rows = [c for c in range(len(r0["ours"])) if np.all(np.isfinite(Mall[c, cols]))]
    M = Mall[np.ix_(rows, cols)]
    x, y = smooth_equilibrium(M, smooth) if smooth and smooth > 0 else solve(M)
    xfull = np.zeros(len(r0["ours"]))
    xfull[rows] = x
    pool = sorted(set().union(*[r["survivors"] for r in rs]))
    V = float(x @ M @ y)
    if opp_eq is not None:
        ph = np.array([max(float(r0["theirs"][j][0]), 0.0) for j in cols], np.float64)
        ph = ph / ph.sum() if ph.sum() > 0 else np.full(len(cols), 1.0 / len(cols))
        y = opp_eq * np.asarray(y, np.float64) + (1 - opp_eq) * ph
    q = {c: float(Mall[c, cols] @ y) for c in rows}
    if opp_eq is not None:
        V = max(q[c] for c in pool if c in q) if any(c in q for c in pool) else V
    return dict(r0, M_all=Mall, x=xfull, survivors=[c for c in pool if c in q], value=V, q=q, y_survivors=(cols, y),
                leaves=sum(r["leaves"] for r in rs))


def choose_q(r):
    """the candidate with the highest value q (against the opponent mix the search assumed): the review's 'best move'"""
    pool = r["survivors"] or list(r["q"])
    return max(pool, key=lambda c: r["q"][c])
