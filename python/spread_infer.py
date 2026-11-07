"""Opponent stat-point / item / ability inference from damage and move order (2026-10-19, user).

Per opponent Pokemon a `Belief` over (stat points, nature, item, ability), with two strictly separate outputs:
  * PROVEN (`b.proven()`): every hypothesis that can still produce what we saw in THIS game. A hypothesis is dropped
    only when the engine's own damage rolls / speed say it cannot have produced an observation; nothing is dropped for
    being unusual. Assumption: the items / abilities considered are every legal one for the species (Teams_cov), and the
    observation's context (boosts, weather, screens, partner effects...) is what the position we were given says.
    An observation that NO hypothesis can explain is not applied: it goes to `b.contradictions` (something we don't
    model, e.g. a hidden partner effect) instead of wiping out the proven set.
  * GUESS (`b.summary()`, `b.sample()`): inside the proven set, hypotheses weighted by real tournament spreads of the
    species (spread_prior.json, built from Teams_real teams whose spreads came from the original pastes, validation teams
    left out) plus a small 'anything legal' share, times the likelihood of the observations. Search worlds should be drawn
    with `sample()`: it never returns a hypothesis outside the proven set.

Observations (each takes a vgcsim position = the context of that action, with the opponent Pokemon in it):
  * `our_hit(bt, att, slot, dfn, before, after, spread)`: our move hit them; `before` / `after` = their displayed HP
    (Champions shows floor(100 hp / maxhp), min 1, plus a colour at 20 / 50: `display()` codes it) -> HP x Def or
    HP x SpD (the product, not the split), defensive item / ability. Fixed damage (Seismic Toss, Night Shade, Super Fang,
    Final Gambit, Endeavor) pins down HP itself.
  * `their_hit(bt, att, slot, dfn, damage, hp_before)`: their move hit us, exact damage -> Atk or SpA, attacking items /
    abilities.
  * `order(bt, them, other_speed, they_first, trick_room)`: who moved first in the same priority bracket -> Speed bounds
    (Choice Scarf, Iron Ball, weather / terrain speed abilities, paralysis, Tailwind via the engine's speed()).
Crits are not modelled: skip critical hits (nothing is proven from them).

Champions stats (level 50, perfect IVs): HP = base + SP + 75, others = floor((base + SP + 20) x nat / 100), SP 0..32,
66 in total.

    python spread_infer.py build-prior          # spread_prior.json
    python spread_infer.py test --trials 200    # synthetic test on validation teams (truth known)
"""

import paths
import argparse
import collections
import glob
import json
import math
import os
import random
import re
import numpy as np

HERE = os.path.dirname(os.path.abspath(__file__))
STATS = ("hp", "atk", "def", "spa", "spd", "spe")
SP = np.arange(33)
NATS = (90, 100, 110)
BUDGET = 66
PRIOR_FILE = os.path.join(paths.CACHE, "spread_prior.json")
NEG = -np.inf
# items / abilities that change the move ORDER outside the speed stat: no speed constraint from the order
ORDER_ITEMS = {"laggingtail", "fullincense"}
ORDER_ABILITIES = {"stall", "myceliummight", "quickdraw"}
HALF_HP_MOVES = {"superfang", "ruination", "naturesmadness"}
_DEX = None


def dexjson():
    global _DEX
    if _DEX is None:
        _DEX = json.load(open(paths.DEX, encoding="utf-8"))
    return _DEX


def to_id(s):
    return re.sub(r"[^a-z0-9]", "", str(s).lower())


def hp_stat(base, sp):
    return base + sp + 75


def other_stat(base, sp, nat):
    return (base + sp + 20) * nat // 100


def display(hp, maxhp):
    """What the opponent sees of a Pokemon's HP in Champions (Showdown getHealth), coded as percent * 4 + colour."""
    hp = np.asarray(hp)
    pct = np.where(hp > 0, np.maximum(100 * hp // maxhp, 1), 0)
    col = np.where(pct == 20, np.where(hp * 5 > maxhp, 1, 2), np.where(pct == 50, np.where(hp * 2 > maxhp, 3, 1), 0))
    return pct * 4 + col


_NATURES = None
_NATMAP = None


def _nature_map():
    global _NATMAP
    if _NATMAP is None:
        _NATMAP = dict(natures())
    return _NATMAP


def natures():
    """[(name, {stat: index into NATS})] for the 25 natures (cached: don't modify the dicts)."""
    global _NATURES
    if _NATURES is not None:
        return list(_NATURES)
    out = []
    for nid, n in dexjson()["natures"].items():
        idx = {s: 1 for s in STATS[1:]}
        if n["plus"] and n["plus"] != n["minus"]:
            idx[n["plus"]] = 2
            idx[n["minus"]] = 0
        out.append((n["name"], idx))
    _NATURES = out
    return list(out)


# ---------------------------------------------------------------- legal content + real-spread prior

_LEGAL = None


def legal():
    """species id -> {"abilities": set, "items": set} (legal pairs from the coverage pool Teams_cov), "items": all legal."""
    global _LEGAL
    if _LEGAL is None:
        import setlib
        ab = collections.defaultdict(set)
        items = set()
        for p in glob.glob(os.path.join(paths.REPO, "engine", "pools", "Teams_cov", "*.txt")):
            for blk in re.split(r"\n\s*\n", open(p, encoding="utf-8").read().strip()):
                s = setlib.parse_block(blk)
                if s:
                    ab[s["species"]].add(s["ability"])
                    if s["item"]:
                        items.add(s["item"])
        _LEGAL = {"abilities": ab, "items": items}
    return _LEGAL


def species_items(species):
    """Legal items for a species: every legal item except other species' Mega Stones."""
    dj = dexjson()
    out = {""}
    base = to_id(dj["species"].get(species, {}).get("baseSpecies", species))
    for it in legal()["items"]:
        d = dj["items"].get(it, {})
        if d.get("megaStone"):
            if not ({base, to_id(species)} & {to_id(x) for x in d["megaStone"]}):     # Floette-Eternal's stone
                continue
        out.add(it)
    return out


def mega_stone(form):
    """The Mega Stone that turns a base species into `form` (None if `form` isn't a Mega)."""
    dj = dexjson()
    name = dj["species"].get(to_id(form), {}).get("name")
    for it, d in dj["items"].items():
        if name and name in (d.get("megaStone") or {}).values():
            return it
    return None


def build_prior(path=PRIOR_FILE):
    """Real spreads per species from the team folder (teams whose every Pokemon carries the guessed template are left
    out); items / abilities from every team."""
    import setlib
    out = collections.defaultdict(collections.Counter)
    items = collections.defaultdict(collections.Counter)
    abil = collections.defaultdict(collections.Counter)
    nteams = 0
    for p in sorted(glob.glob(os.path.join(paths.TEAMS, "Teams_real", "*.txt"))):
        txt = open(p, encoding="utf-8").read()
        sets = [setlib.parse_block(b) for b in re.split(r"\n\s*\n", txt.strip())]
        sets = [s for s in sets if s]
        for s in sets:
            items[s["species"]][s["item"]] += 1
            abil[s["species"]][s["ability"]] += 1
        if _all_template(sets):
            continue
        nteams += 1
        for s in sets:
            pts = tuple(int(s["points"].get(k, 0)) for k in STATS)
            out[s["species"]][(pts, to_id(s["nature"]), s["item"], s["ability"])] += 1
    data = {"teams": nteams, "spreads": {}, "items": {k: dict(v) for k, v in items.items()},
            "abilities": {k: dict(v) for k, v in abil.items()}}
    for sp, c in out.items():
        data["spreads"][sp] = [[list(k[0]), k[1], k[2], k[3], n] for k, n in c.most_common()]
    json.dump(data, open(path, "w"), indent=0)
    return data


def _all_template(sets):
    """the guessed-spread template: every Pokemon 2 HP / 32 / 32 in its best attack + Speed (approximation:
    the file is skipped when all six have exactly 2 HP and two stats at 32)."""
    def tmpl(s):
        p = s["points"]
        return p.get("hp", 0) == 2 and sorted(p.values(), reverse=True)[:2] == [32, 32]
    return bool(sets) and all(tmpl(s) for s in sets)


_PRIOR = None


def prior_data():
    global _PRIOR
    if _PRIOR is None:
        _PRIOR = json.load(open(PRIOR_FILE)) if os.path.exists(PRIOR_FILE) else build_prior()
    return _PRIOR


_NVALID = None


def n_valid_spreads():
    """Number of point spreads with each stat 0..32 and total <= 66 (the 'anything legal' prior's normaliser)."""
    global _NVALID
    if _NVALID is None:
        v = np.ones(1)
        for _ in range(6):
            v = np.convolve(v, np.ones(33))
        _NVALID = float(v[:BUDGET + 1].sum())
    return _NVALID


# ---------------------------------------------------------------- likelihood kernels

def bulk_loglik(R_full, R_hurt, hp_base, obs_before, obs_after, kind=None, fixed=None):
    """log P(their displayed HP before / after one of our hits | HP points h, defence value index k) -> [33, K].
    R_full / R_hurt: [K, 16] rolls with the target at full / not full HP (Multiscale & co.). Starting HP unknown: uniform
    over the values that show `obs_before`. kind 'half' = floor(hp / 2) (Super Fang), 'endeavor' = hp - fixed."""
    K = R_full.shape[0]
    out = np.full((33, K), NEG)
    for h in SP:
        mx = hp_stat(hp_base, h)
        hps = np.arange(1, mx + 1)
        h0 = hps[display(hps, mx) == obs_before]
        if len(h0) == 0:
            continue
        if kind == "half":
            dmg = np.maximum(h0 // 2, 1)[None, :, None].repeat(K, 0)
        elif kind == "endeavor":
            dmg = np.maximum(h0 - fixed, 0)[None, :, None].repeat(K, 0)
        else:
            full = (h0 == mx)[None, :, None]
            dmg = np.where(full, R_full[:, None, :], R_hurt[:, None, :])
        after = np.maximum(h0[None, :, None] - dmg, 0)
        p = (display(after, mx) == obs_after).mean(axis=(1, 2))
        with np.errstate(divide="ignore"):
            out[h] = np.log(p)
    return out


def exact_loglik(R, damage, hp_before):
    """log P(exact damage we took | attack value index k) -> [K]; a KO only shows damage >= hp_before."""
    p = (R >= hp_before).mean(axis=1) if damage >= hp_before else (R == damage).mean(axis=1)
    with np.errstate(divide="ignore"):
        return np.log(p)


# ---------------------------------------------------------------- belief

class Belief:
    """What we know about ONE opponent Pokemon's points / nature / item / ability (see the module doc)."""

    def __init__(self, species, item=None, ability=None, items=None, abilities=None):
        """species: the TEAM species id (base form; points are shared by its Mega form). item / ability: known values
        (None = unknown)."""
        self.species = to_id(species)
        dj = dexjson()
        self.base_by_form = {}
        lg = legal()
        self.abilities = sorted({ability} if ability else (abilities or lg["abilities"].get(self.species)
                                or {to_id(a) for a in dj["species"][self.species]["abilities"].values()}))
        self.items = sorted({item} if item is not None else (items or species_items(self.species)))
        # (2026-10-19) a base form never plays with its Mega's ability: Showdown's validator rewrites such sets to the
        # species' first ability, so those are NOT hypotheses
        # log-likelihood blocks per hypothesis key (item, ability); item None = 'every item not listed separately'
        # (items that changed nothing in any observation so far share one history)
        self.special = set()
        self.blocks = {(None, a): self._empty() for a in self.abilities}
        self.contradictions = []
        self.n_obs = 0

    @staticmethod
    def _empty():
        return {"def": np.zeros((33, 33, 3)), "spd": np.zeros((33, 33, 3)), "atk": np.zeros((33, 3)),
                "spa": np.zeros((33, 3)), "spe": np.zeros((33, 3))}

    def keys(self):
        return list(self.blocks)

    def _base(self, form):
        if form not in self.base_by_form:
            self.base_by_form[form] = dexjson()["species"][to_id(form)]["baseStats"]
        return self.base_by_form[form]

    # ------------------------------------------------------------ hypothesis screening
    def _apply(self, delta):
        """delta: {(item or None, ability): (block name, array)} for every key of the CURRENT observation, where the
        observation's evaluator gave item None = no special effect. Applies it unless it would leave nothing proven."""
        new_items = {k[0] for k in delta if k[0] is not None} - self.special
        trial = {}
        for (it, ab), blk in self.blocks.items():
            trial[(it, ab)] = {k: v.copy() for k, v in blk.items()}
        for it in new_items:                     # an item that mattered for the first time: copy the shared history
            for ab in self.abilities:
                trial[(it, ab)] = {k: v.copy() for k, v in self.blocks[(None, ab)].items()}
        for (it, ab), blk in trial.items():
            d = delta.get((it, ab)) or delta.get((None, ab))
            if d is None:
                continue
            name, arr = d
            blk[name] = blk[name] + arr
        if not self._any_feasible(trial):
            return False
        self.blocks = trial
        self.special |= new_items
        self.n_obs += 1
        return True

    def _screen(self, evaluate, ref):
        """Run `evaluate(item, ability)` (-> comparable array) at the reference point for every hypothesis; return the
        items whose result differs from 'no item' for some ability (each needs its own likelihood)."""
        out = set()
        for ab in self.abilities:
            base = evaluate("", ab, ref)
            for it in self.items:
                if it == "":
                    continue
                r = evaluate(it, ab, ref)
                if r is None and base is None:
                    continue
                if (r is None) != (base is None) or not np.array_equal(r, base):
                    out.add(it)
        return out

    # ------------------------------------------------------------ observations
    def our_hit(self, bt, att, slot, dfn, before, after, spread=False, form=None, fixed_ability=None):
        """Our move (Pokemon att = (side, idx), move slot) hit them (dfn = (side, idx)); before / after = display codes."""
        dj = dexjson()
        st = bt.state()
        a_mon, d_mon = st["sides"][att[0]]["mons"][att[1]], st["sides"][dfn[0]]["mons"][dfn[1]]
        mid = a_mon["moves"][slot][0]
        md = dj["moves"].get(mid, {})
        form = form or d_mon["species"]
        base = self._base(form)
        key = md.get("overrideDefensiveStat") or ("def" if md.get("category") == "Physical" else "spd")
        kind, fixed = None, None
        if mid in HALF_HP_MOVES:
            kind = "half"
        elif mid == "endeavor":
            kind, fixed = "endeavor", a_mon["hp"]
        b = bt.copy()
        maxhp0 = d_mon["maxhp"]

        def rolls(item, ab, values, hurt):
            b.set_mon(dfn[0], dfn[1], item=item)
            b.set_ability(dfn[0], dfn[1], fixed_ability or ab)   # a Mega's ability is its form's
            out = []
            for v in values:
                b.set_mon(dfn[0], dfn[1], stats={key: int(v)})
                b.set_mon(dfn[0], dfn[1], hp=max(maxhp0 - 1, 1) if hurt else maxhp0)
                r = b.damage_rolls(att[0], att[1], slot, dfn[0], dfn[1], True, spread)
                if r is None:
                    return None
                if mid == "finalgambit":
                    r = [a_mon["hp"]] * 16
                out.append(r)
            return np.array(out, dtype=np.int64)

        vals = sorted({other_stat(base[key], s, n) for s in SP for n in NATS})
        vix = {v: i for i, v in enumerate(vals)}
        cell = np.array([[vix[other_stat(base[key], s, n)] for n in NATS] for s in SP])     # [33, 3] -> value index
        ref = [vals[len(vals) // 2]]
        special = self._screen(lambda it, ab, v: _cat(rolls(it, ab, v, False), rolls(it, ab, v, True)), ref)
        delta = {}
        for ab in self.abilities:
            for it in [None] + sorted(special):
                Rf, Rh = rolls(it or "", ab, vals, False), rolls(it or "", ab, vals, True)
                if Rf is None:
                    ll = np.full((33, len(vals)), NEG)           # immune under this hypothesis, but it took damage
                    if before == after:
                        ll[:] = 0.0                              # no damage shown: consistent
                else:
                    ll = bulk_loglik(Rf, Rh, base["hp"], before, after, kind, fixed)
                delta[(it, ab)] = (key, ll[:, cell])             # [33 h, 33 sp, 3 nat]
        ok = self._apply(delta)
        if not ok:
            self.contradictions.append(("our_hit", mid, before, after))
        return ok

    def their_hit(self, bt, att, slot, dfn, damage, hp_before, spread=False, form=None, fixed_ability=None):
        """Their move (att = their Pokemon) hit our Pokemon dfn for `damage` exact HP (hp_before = our HP before)."""
        dj = dexjson()
        st = bt.state()
        a_mon = st["sides"][att[0]]["mons"][att[1]]
        mid = a_mon["moves"][slot][0]
        md = dj["moves"].get(mid, {})
        if md.get("overrideOffensivePokemon") or md.get("damage") is not None or mid in HALF_HP_MOVES:
            return None                                          # no information about their attack stat
        key = md.get("overrideOffensiveStat") or ("atk" if md.get("category") == "Physical" else "spa")
        if key not in ("atk", "spa", "def"):
            return None
        if key == "def":
            return None                                          # Body Press: their Def (not modelled yet)
        form = form or a_mon["species"]
        base = self._base(form)
        b = bt.copy()

        def rolls(item, ab, values):
            b.set_mon(att[0], att[1], item=item)
            b.set_ability(att[0], att[1], fixed_ability or ab)
            out = []
            for v in values:
                b.set_mon(att[0], att[1], stats={key: int(v)})
                r = b.damage_rolls(att[0], att[1], slot, dfn[0], dfn[1], True, spread)
                if r is None:
                    return None
                out.append(r)
            return np.array(out, dtype=np.int64)

        vals = sorted({other_stat(base[key], s, n) for s in SP for n in NATS})
        vix = {v: i for i, v in enumerate(vals)}
        cell = np.array([[vix[other_stat(base[key], s, n)] for n in NATS] for s in SP])
        special = self._screen(rolls, [vals[len(vals) // 2]])
        delta = {}
        for ab in self.abilities:
            for it in [None] + sorted(special):
                R = rolls(it or "", ab, vals)
                ll = np.full(len(vals), NEG) if R is None else exact_loglik(R, damage, hp_before)
                delta[(it, ab)] = (key, ll[cell])
        ok = self._apply(delta)
        if not ok:
            self.contradictions.append(("their_hit", mid, damage))
        return ok

    def order(self, bt, them, other_speed, they_first, trick_room=False, form=None, fixed_ability=None):
        """In one priority bracket their Pokemon `them` moved before (they_first) / after a Pokemon whose speed is
        `other_speed` (ours: known exactly). Ties go either way, so the bound is weak (>= / <=)."""
        st = bt.state()
        mon = st["sides"][them[0]]["mons"][them[1]]
        form = form or mon["species"]
        base = self._base(form)
        b = bt.copy()

        def speeds(item, ab, values):
            b.set_mon(them[0], them[1], item=item)
            b.set_ability(them[0], them[1], fixed_ability or ab)
            out = []
            for v in values:
                b.set_mon(them[0], them[1], stats={"spe": int(v)})
                out.append(b.speed(them[0], them[1]))
            return np.array(out, dtype=np.int64)

        vals = sorted({other_stat(base["spe"], s, n) for s in SP for n in NATS})
        vix = {v: i for i, v in enumerate(vals)}
        cell = np.array([[vix[other_stat(base["spe"], s, n)] for n in NATS] for s in SP])
        special = self._screen(speeds, [vals[len(vals) // 2]]) | (ORDER_ITEMS & set(self.items))
        faster = they_first != bool(trick_room)
        delta = {}
        for ab in self.abilities:
            for it in [None] + sorted(special):
                if (it in ORDER_ITEMS) or ((fixed_ability or ab) in ORDER_ABILITIES):
                    ll = np.zeros(len(vals))
                else:
                    s = speeds(it or "", ab, vals)
                    ok_ = s >= other_speed if faster else s <= other_speed
                    ll = np.where(ok_, 0.0, NEG)
                delta[(it, ab)] = ("spe", ll[cell])
        ok = self._apply(delta)
        if not ok:
            self.contradictions.append(("order", other_speed, they_first, trick_room))
        return ok

    # ------------------------------------------------------------ combining the blocks
    def _combos(self, blocks=None):
        """Yield (key, nature name, nature idx, Ld [33h,33], Ls [33h,33], la, lc, le [33]) per hypothesis key x nature."""
        blocks = blocks or self.blocks
        for key, blk in blocks.items():
            for nname, ix in natures():
                yield (key, nname, ix, blk["def"][:, :, ix["def"]], blk["spd"][:, :, ix["spd"]],
                       blk["atk"][:, ix["atk"]], blk["spa"][:, ix["spa"]], blk["spe"][:, ix["spe"]])

    @staticmethod
    def _minpts(v):
        f = np.flatnonzero(np.isfinite(v))
        return int(f[0]) if len(f) else None

    def _any_feasible(self, blocks):
        for key, nname, ix, Ld, Ls, la, lc, le in self._combos(blocks):
            m_off = [self._minpts(x) for x in (la, lc, le)]
            if None in m_off:
                continue
            rest = sum(m_off)
            for h in SP:
                md, ms = self._minpts(Ld[h]), self._minpts(Ls[h])
                if md is not None and ms is not None and h + md + ms + rest <= BUDGET:
                    return True
        return False

    def proven(self, form=None):
        """What THIS game's observations prove (no prior involved). Returns {"items", "abilities", "natures": the ones
        still possible, "points": {stat: (min, max)} possible stat points, "stats": {stat: (min, max)} possible values
        for `form`, "bulk_phys" / "bulk_spec": (min, max) of HP x Def / HP x SpD, "n_obs", "contradictions"}."""
        base = self._base(form or self.species)
        items, abil, nats = set(), set(), set()
        pts = {s: [99, -1] for s in STATS}
        sv = {s: [10 ** 9, -1] for s in STATS}
        bulk = {"def": [10 ** 12, -1], "spd": [10 ** 12, -1]}
        for key, nname, ix, Ld, Ls, la, lc, le in self._combos():
            offs = {"atk": la, "spa": lc, "spe": le}
            m_off = {k: self._minpts(v) for k, v in offs.items()}
            if None in m_off.values():
                continue
            any_ok = False
            for h in SP:
                fd, fs = np.flatnonzero(np.isfinite(Ld[h])), np.flatnonzero(np.isfinite(Ls[h]))
                if not len(fd) or not len(fs):
                    continue
                slack = BUDGET - h - fd[0] - fs[0] - sum(m_off.values())
                if slack < 0:
                    continue
                any_ok = True
                hpv = hp_stat(base["hp"], h)
                pts["hp"][0], pts["hp"][1] = min(pts["hp"][0], h), max(pts["hp"][1], h)
                sv["hp"][0], sv["hp"][1] = min(sv["hp"][0], hpv), max(sv["hp"][1], hpv)
                for stat, f in (("def", fd), ("spd", fs)):
                    ok = f[f <= f[0] + slack]
                    pts[stat][0], pts[stat][1] = min(pts[stat][0], ok[0]), max(pts[stat][1], ok[-1])
                    for s_ in (ok[0], ok[-1]):
                        v = other_stat(base[stat], s_, NATS[ix[stat]])
                        sv[stat][0], sv[stat][1] = min(sv[stat][0], v), max(sv[stat][1], v)
                        bulk[stat][0], bulk[stat][1] = min(bulk[stat][0], v * hpv), max(bulk[stat][1], v * hpv)
                for stat, v_ in offs.items():
                    f = np.flatnonzero(np.isfinite(v_))
                    ok = f[f <= f[0] + slack]
                    pts[stat][0], pts[stat][1] = min(pts[stat][0], ok[0]), max(pts[stat][1], ok[-1])
                    for s_ in (ok[0], ok[-1]):
                        v = other_stat(base[stat], s_, NATS[ix[stat]])
                        sv[stat][0], sv[stat][1] = min(sv[stat][0], v), max(sv[stat][1], v)
            if any_ok:
                it, ab = key
                items |= ({it} if it is not None else (set(self.items) - self.special))
                abil.add(ab)
                nats.add(nname)
        return {"items": sorted(items), "abilities": sorted(abil), "natures": sorted(nats),
                "points": {k: tuple(v) for k, v in pts.items()}, "stats": {k: tuple(v) for k, v in sv.items()},
                "bulk_phys": tuple(bulk["def"]), "bulk_spec": tuple(bulk["spd"]), "n_obs": self.n_obs,
                "contradictions": list(self.contradictions)}

    # ------------------------------------------------------------ guess (prior x likelihood) and sampling
    def _priors(self, eps):
        pd = prior_data()
        ic = pd["items"].get(self.species, {})
        ac = pd["abilities"].get(self.species, {})
        tot_i = sum(ic.values()) + 0.5 * len(self.items)
        p_item = {it: (ic.get(it, 0) + 0.5) / tot_i for it in self.items}
        tot_a = sum(ac.get(a, 0) for a in self.abilities) + 0.5 * len(self.abilities)
        p_ab = {a: (ac.get(a, 0) + 0.5) / tot_a for a in self.abilities}
        nc = collections.Counter()
        for pts, nat, it, ab, n in pd["spreads"].get(self.species, []):
            nc[nat] += n
        names = [n for n, _ in natures()]
        tot_n = sum(nc.values()) + 0.5 * len(names)
        p_nat = {n: (nc.get(to_id(n), 0) + 0.5) / tot_n for n in names}
        lib = [(tuple(p), nat, it, ab, n) for p, nat, it, ab, n in pd["spreads"].get(self.species, [])
               if (it in self.items) and (ab in self.abilities) and sum(p) <= BUDGET and max(p) <= 32]
        return p_item, p_ab, p_nat, lib

    def _ll_of(self, pts, nat_name, item, ab):
        key = (item if item in self.special else None, ab)
        blk = self.blocks.get(key)
        if blk is None:
            return NEG
        ix = _nature_map().get(nat_name)
        if ix is None:
            return NEG
        h, a, d, c, s, e = pts
        return (blk["def"][h, d, ix["def"]] + blk["spd"][h, s, ix["spd"]] + blk["atk"][a, ix["atk"]]
                + blk["spa"][c, ix["spa"]] + blk["spe"][e, ix["spe"]])

    def posterior(self, eps=0.2):
        """Components of the guess: [(log weight, kind, payload)] with kind 'lib' (a real set) or 'uni' (anything legal,
        payload = (key, nature, ix, h, log mass over the rest))."""
        p_item, p_ab, p_nat, lib = self._priors(eps)
        comps = []
        nat_by_id = {to_id(n): n for n, _ in natures()}
        tot_lib = sum(x[4] for x in lib)
        if lib and eps < 1:
            for pts, nat, it, ab, n in lib:
                ll = self._ll_of(pts, nat_by_id.get(nat, "Serious"), it, ab)
                if np.isfinite(ll):
                    comps.append((math.log((1 - eps) * n / tot_lib) + ll, "lib", (pts, nat_by_id.get(nat, "Serious"), it, ab)))
        e_uni = eps if lib else 1.0
        logN = math.log(n_valid_spreads())
        pair_cache = {}
        for key, nname, ix, Ld, Ls, la, lc, le in self._combos():
            it, ab = key
            pi = p_item[it] if it is not None else sum(p for i_, p in p_item.items() if i_ not in self.special)
            lp = math.log(e_uni) + math.log(max(pi, 1e-300)) + math.log(p_ab[ab]) + math.log(p_nat[nname]) - logN
            g = _conv_exp([la, lc, le])
            if g is None:
                continue
            gm, gv = g
            ck = (key, ix["def"], ix["spd"])            # the Def x SpD part depends on 9 nature cases, not 25
            pc = pair_cache.get(ck)
            if pc is None:
                pc = pair_cache[ck] = _pair_conv(Ld, Ls)
            for h, lt in _uni_mass(pc, gv):
                comps.append((lp + lt + gm, "uni", (key, nname, ix, h)))
        return comps

    def sample(self, rng, n=1, eps=0.2, comps=None):
        """Draw n hypotheses (points dict, nature, item, ability) from the guess; never outside the proven set."""
        comps = comps if comps is not None else self.posterior(eps)
        if not comps:
            return []
        lw = np.array([c[0] for c in comps])
        w = np.exp(lw - lw.max())
        w /= w.sum()
        out = []
        for i in rng.choice(len(comps), size=n, p=w):
            _, kind, pl = comps[i]
            if kind == "lib":
                pts, nat, it, ab = pl
                out.append((dict(zip(STATS, pts)), nat, it, ab))
                continue
            key, nname, ix, h = pl
            blk = self.blocks[key]
            vecs = [blk["def"][h, :, ix["def"]], blk["spd"][h, :, ix["spd"]], blk["atk"][:, ix["atk"]],
                    blk["spa"][:, ix["spa"]], blk["spe"][:, ix["spe"]]]
            vals = _sample_budget([np.exp(v - np.max(v)) for v in vecs], BUDGET - h, rng)
            d, s, a, c, e = vals
            it = key[0]
            if it is None:
                p_item, _, _, _ = self._priors(eps)
                cand = [x for x in self.items if x not in self.special]
                pw = np.array([p_item[x] for x in cand])
                it = cand[rng.choice(len(cand), p=pw / pw.sum())]
            out.append(({"hp": h, "atk": a, "def": d, "spa": c, "spd": s, "spe": e}, nname, it, key[1]))
        return out

    def _comps_cached(self, eps=0.2):
        key = (self.n_obs, tuple(sorted(self.blocks, key=str)), tuple(self.items), tuple(self.abilities), eps)
        if getattr(self, "_cc_key", None) != key:
            self._cc_key, self._cc = key, self.posterior(eps)
        return self._cc

    def draw(self, rng=None, item=None, ability=None, items=None, eps=0.2):
        """One hypothesis (points dict, nature, item, ability) for a search world, never outside the proven set.
        item / ability: wanted values (kept if this game hasn't ruled them out); items: allowed item set when the item is
        free (e.g. 'Mega Stones only'). rng None = the single most likely component (world 0, deterministic)."""
        comps = self._comps_cached(eps)
        if not comps:
            return None

        def fits(c, it_ok, ab_ok):
            kind, pl = c[1], c[2]
            if kind == "lib":
                it, ab = pl[2], pl[3]
            else:
                it, ab = pl[0]
            return ab_ok(ab) and (it_ok(it) if it is not None else any(it_ok(x) for x in self.items if x not in self.special))
        tries = []
        if item is not None or ability is not None:
            tries.append((lambda x: item is None or x == item, lambda a: ability is None or a == ability))
        if items is not None:
            tries.append((lambda x: x in items, lambda a: True))
        tries.append((lambda x: True, lambda a: True))
        for it_ok, ab_ok in tries:
            sub = [c for c in comps if fits(c, it_ok, ab_ok)]
            if sub:
                break
        if rng is None:
            sub = [max(sub, key=lambda c: c[0])]
            rng = np.random.default_rng(0)
        out = self.sample(rng, 1, eps, comps=sub)
        if not out:
            return None
        pts, nat, it, ab = out[0]
        # items that never changed an observation share one likelihood history: a wanted / allowed one of them is as
        # consistent as the one drawn
        if it not in self.special:
            if item is not None and it != item and item in self.items and item not in self.special:
                it = item
            elif items is not None and it not in items:
                ok = sorted(x for x in items if x in self.items and x not in self.special)
                if ok:
                    it = ok[int(rng.integers(len(ok)))]
        return pts, nat, it, ab

    def summary(self, rng=None, n=2000, eps=0.2, form=None):
        """The guess, summarised from samples: median points per stat, P(item), P(ability), top natures."""
        rng = rng or np.random.default_rng(0)
        smp = self.sample(rng, n, eps)
        if not smp:
            return None
        base = self._base(form or self.species)
        pts = {k: float(np.median([s[0][k] for s in smp])) for k in STATS}
        it = collections.Counter(s[2] for s in smp)
        ab = collections.Counter(s[3] for s in smp)
        nat = collections.Counter(s[1] for s in smp)
        nmap = dict(natures())

        def statv(s, k):
            p = s[0][k]
            return hp_stat(base["hp"], p) if k == "hp" else other_stat(base[k], p, NATS[nmap[s[1]][k]])
        bulk_p = [statv(s, "hp") * statv(s, "def") for s in smp]
        bulk_s = [statv(s, "hp") * statv(s, "spd") for s in smp]
        return {"points": pts, "stats": {k: float(np.median([statv(s, k) for s in smp])) for k in STATS},
                "bulk_phys": float(np.median(bulk_p)), "bulk_spec": float(np.median(bulk_s)),
                "items": {k: v / n for k, v in it.most_common(6)}, "abilities": {k: v / n for k, v in ab.most_common()},
                "natures": {k: v / n for k, v in nat.most_common(4)}, "samples": smp}


def _cat(a, b):
    if a is None or b is None:
        return None
    return np.concatenate([a, b])


def _expv(v):
    m = np.max(v)
    if not np.isfinite(m):
        return None
    return m, np.exp(v - m)


_TOEP = {}


def _toep_idx(n):
    """Index [d, k] -> k - d + n - 1 into fs padded with n - 1 zeros on both sides."""
    t = _TOEP.get(n)
    if t is None:
        t = _TOEP[n] = np.arange(2 * n - 1)[None, :] - np.arange(n)[:, None] + n - 1
    return t


def _pair_conv(Ld, Ls):
    """Per HP value h with any feasible Def and SpD: conv[h, k] = sum over d + s = k of exp(Ld[h,d] - max) * exp(Ls[h,s] - max).
    Returns (hs, log maxima per h, conv) or None."""
    md, ms = np.max(Ld, axis=1), np.max(Ls, axis=1)
    ok = np.isfinite(md) & np.isfinite(ms)
    if not ok.any():
        return None
    hs = np.nonzero(ok)[0]
    fd = np.exp(Ld[hs] - md[hs, None])
    fs = np.exp(Ls[hs] - ms[hs, None])
    n = fd.shape[1]
    # gather fs into [h, d, k] (zero outside 0..n-1), then one einsum
    fsp = np.concatenate([np.zeros((len(hs), n - 1)), fs, np.zeros((len(hs), n - 1))], axis=1)
    conv = np.einsum("hd,hdk->hk", fd, fsp[:, _toep_idx(n)])
    return hs, md[hs] + ms[hs], conv


def _uni_mass(pc, gv):
    """For every HP value h: log sum over d, s, g with d + s + g <= BUDGET - h of exp(Ld[h,d] + Ls[h,s]) * gv[g]
    (the remaining stats' mass; same numbers as convolving row by row, vectorized over h). Yields (h, log mass)."""
    if pc is None:
        return []
    hs, mm, conv = pc
    G = np.concatenate([[0.0], np.cumsum(gv)])          # G[k + 1] = sum of gv[0..k]
    k = np.arange(conv.shape[1])
    idx = np.clip(BUDGET - hs[:, None] - k[None, :], -1, len(gv) - 1) + 1
    tot = (conv * G[idx]).sum(axis=1)
    pos = tot > 0
    return list(zip(hs[pos].tolist(), (mm[pos] + np.log(tot[pos])).tolist()))


def _conv_exp(vecs):
    tot_m, g = 0.0, np.ones(1)
    for v in vecs:
        e = _expv(v)
        if e is None:
            return None
        tot_m += e[0]
        g = np.convolve(g, e[1])
    return tot_m, g


def _sample_budget(vecs, budget, rng):
    """Sample x_k ~ prod vecs[k][x_k] subject to sum x_k <= budget."""
    suffix = [None] * (len(vecs) + 1)
    suffix[len(vecs)] = np.ones(1)
    for k in range(len(vecs) - 1, -1, -1):
        suffix[k] = np.convolve(vecs[k], suffix[k + 1])
    out = []
    for k, v in enumerate(vecs):
        cs = np.concatenate([[0.0], np.cumsum(suffix[k + 1])])
        w = np.array([v[x] * cs[min(budget - x, len(cs) - 2) + 1] if x <= budget else 0.0 for x in range(len(v))])
        x = int(rng.choice(len(v), p=w / w.sum()))
        out.append(x)
        budget -= x
    return out


# ---------------------------------------------------------------- synthetic test

def team_sets(path):
    return [b for b in re.split(r"\n\s*\n", open(path, encoding="utf-8").read().strip()) if b.strip()]


def run_test(a):
    import vgcsim
    dex = vgcsim.Dex(paths.DEX)
    dj = dexjson()
    files = sorted(glob.glob(os.path.join(paths.TEAMS, "Teams_real", "*.txt")))
    val = files[::15]
    rng = random.Random(a.seed)
    nrng = np.random.default_rng(a.seed)
    ks = [int(k) for k in a.ks.split(",")]
    rows = collections.defaultdict(list)
    trials = skipped = 0
    while trials < a.trials:
        O, D = team_sets(rng.choice(files)), team_sets(rng.choice(val))
        if len(O) < 4 or len(D) < 4:
            continue
        rng.shuffle(D)
        try:
            bt = vgcsim.Battle.from_brought(dex, "\n\n".join(O[:4]), "\n\n".join(D[:4]), rng.randrange(1 << 30), 1)
        except Exception:
            skipped += 1
            continue
        st = bt.state()
        tgt = st["sides"][1]["mons"][0]
        sid = to_id(tgt["set_species"])
        if tgt.get("is_mega") or sid not in dj["species"]:
            skipped += 1
            continue
        nat_true = _nature_of(D[0])
        pts_true = _points_of(D[0])
        true = dict(item=tgt["item"], ability=tgt["ability"], nature=nat_true, pts=pts_true, stats=tgt["stats"])
        bel = Belief(sid)
        if true["item"] not in bel.items or true["ability"] not in bel.abilities:
            skipped += 1
            continue
        # observation menu
        ours = []
        for i, m in enumerate(st["sides"][0]["mons"]):
            for slot, (mid, _pp) in enumerate(m["moves"]):
                md = dj["moves"].get(mid, {})
                if md.get("category") == "Status" or md.get("multihit") or mid in ("counter", "mirrorcoat", "metalburst",
                                                                                     "comeuppance", "bide"):
                    continue
                ours.append(("our", i, slot))
        theirs = []
        for slot, (mid, _pp) in enumerate(tgt["moves"]):
            md = dj["moves"].get(mid, {})
            if md.get("category") == "Status" or md.get("multihit") or md.get("damage") is not None:
                continue
            theirs += [("their", slot, j) for j in range(4)]
        speedo = [("order", j) for j in range(4)]
        menu = ours + theirs + speedo
        if not ours:
            skipped += 1
            continue
        comps0 = bel.posterior(a.eps)
        s0 = bel.summary(nrng, 600, a.eps)
        for step in range(1, max(ks) + 1):
            _observe_one(rng, menu, bt, st, tgt, bel)
            if step in ks:
                pr = bel.proven()
                sm = bel.summary(nrng, 600, a.eps)
                rows[step].append(_score(true, tgt, pr, sm, s0, bel))
        trials += 1
    _report(rows, ks, trials, skipped, a.eps)


def _observe_one(rng, menu, bt, st, tgt, bel):
    """One random observation (retried until one applies); the truth is the engine position itself."""
    for _try in range(20):
        if _observe_try(rng, menu, bt, st, tgt, bel):
            return


def _observe_try(rng, menu, bt, st, tgt, bel):
    if True:                                     # (kept the test's original indentation)
        if True:
            kind = rng.choice(menu)
            if kind[0] == "our":
                _, i, slot = kind
                R = bt.damage_rolls(0, i, slot, 1, 0, True, False)
                if R is None:
                    return False
                mx = tgt["maxhp"]
                h0 = mx if rng.random() < 0.4 else rng.randint(1, mx)
                mid = st["sides"][0]["mons"][i]["moves"][slot][0]
                if mid in HALF_HP_MOVES:
                    dmg = max(h0 // 2, 1)
                elif mid == "endeavor":
                    dmg = max(h0 - st["sides"][0]["mons"][i]["hp"], 0)
                elif mid == "finalgambit":
                    dmg = st["sides"][0]["mons"][i]["hp"]
                else:
                    if h0 < mx:
                        bh = bt.copy()
                        bh.set_mon(1, 0, hp=h0)
                        R = bh.damage_rolls(0, i, slot, 1, 0, True, False)
                    dmg = R[rng.randrange(16)]
                bel.our_hit(bt, (0, i), slot, (1, 0), int(display(h0, mx)), int(display(max(h0 - dmg, 0), mx)))
            elif kind[0] == "their":
                _, slot, j = kind
                R = bt.damage_rolls(1, 0, slot, 0, j, True, False)
                if R is None or max(R) == 0:
                    return False
                hpj = st["sides"][0]["mons"][j]["maxhp"]
                dmg = R[rng.randrange(16)]
                bel.their_hit(bt, (1, 0), slot, (0, j), int(dmg), int(hpj))
            else:
                j = kind[1]
                so = bt.speed(0, j)
                st_ = bt.speed(1, 0)
                if so == st_:
                    first = rng.random() < 0.5
                else:
                    first = st_ > so
                bel.order(bt, (1, 0), so, first, False)
            return True


def _nature_of(blk):
    m = re.search(r"^\s*(\w+) Nature", blk, re.M)
    return m.group(1) if m else "Serious"


def _points_of(blk):
    out = dict.fromkeys(STATS, 0)
    m = re.search(r"^(?:EVs|SPs):(.*)$", blk, re.M)
    if m:
        for part in m.group(1).split("/"):
            v, k = part.strip().split(" ", 1)
            out[{"hp": "hp", "atk": "atk", "def": "def", "spa": "spa", "spd": "spd", "spe": "spe"}[to_id(k)]] = int(v)
    return out


def _score(true, tgt, pr, sm, s0, bel):
    st = true["stats"]
    r = {}
    # proven: does every proven range contain the truth?
    ok = true["item"] in pr["items"] and true["ability"] in pr["abilities"] and true["nature"] in pr["natures"]
    for k in STATS:
        lo, hi = pr["points"][k]
        ok = ok and lo <= true["pts"][k] <= hi
    r["truth_proven"] = ok
    r["contra"] = len(pr["contradictions"])
    r["items_left"] = len(pr["items"])
    r["items_all"] = len(bel.items)
    for name, tv, gv, g0 in (("bulk_phys", st["hp"] * st["def"], sm["bulk_phys"], s0["bulk_phys"]),
                             ("bulk_spec", st["hp"] * st["spd"], sm["bulk_spec"], s0["bulk_spec"]),
                             ("atk", st["atk"], sm["stats"]["atk"], s0["stats"]["atk"]),
                             ("spa", st["spa"], sm["stats"]["spa"], s0["stats"]["spa"]),
                             ("spe", st["spe"], sm["stats"]["spe"], s0["stats"]["spe"])):
        r[name] = abs(math.log(gv / tv))
        r[name + "0"] = abs(math.log(g0 / tv))
    r["p_item"] = sm["items"].get(true["item"], 0.0)
    r["p_item0"] = s0["items"].get(true["item"], 0.0)
    r["item_top"] = max(sm["items"], key=sm["items"].get) == true["item"]
    r["item_top0"] = max(s0["items"], key=s0["items"].get) == true["item"]
    return r


def _report(rows, ks, trials, skipped, eps):
    print(f"{trials} validation-team defenders ({skipped} skipped), guess = real-spread prior (eps {eps}) x likelihood")
    print("obs  proven-contains-truth  contradictions  items still possible")
    for k in ks:
        r = rows[k]
        if r:
            print(f"{k:>3}  {100 * np.mean([x['truth_proven'] for x in r]):6.1f}%  {np.mean([x['contra'] for x in r]):6.2f}"
                  f"   {np.mean([x['items_left'] for x in r]):5.1f} of {np.mean([x['items_all'] for x in r]):.0f}")
    print("guess error (median of the guess vs truth, mean |log| in %; before any observation in brackets):")
    print("obs   bulk_phys       bulk_spec       atk             spa             spe          P(true item)  item top-1")
    for k in ks:
        r = rows[k]
        if not r:
            continue
        f = lambda key: 100 * np.mean([x[key] for x in r])
        cols = "  ".join(f"{f(n):5.1f} ({f(n + '0'):4.1f})" for n in ("bulk_phys", "bulk_spec", "atk", "spa", "spe"))
        print(f"{k:>3}  {cols}   {np.mean([x['p_item'] for x in r]):.2f} ({np.mean([x['p_item0'] for x in r]):.2f})"
              f"   {f('item_top'):4.0f}% ({f('item_top0'):3.0f}%)")


def main():
    p = argparse.ArgumentParser()
    p.add_argument("cmd", choices=["test", "build-prior"])
    p.add_argument("--trials", type=int, default=100)
    p.add_argument("--ks", default="2,4,8,16")
    p.add_argument("--eps", type=float, default=0.2, help="share of the 'anything legal' prior (1 = no real-spread prior)")
    p.add_argument("--seed", type=int, default=1)
    a = p.parse_args()
    if a.cmd == "build-prior":
        d = build_prior()
        print(f"spread_prior.json: {d['teams']} teams with real spreads, {len(d['spreads'])} species, "
              f"{sum(len(v) for v in d['spreads'].values())} distinct sets")
    else:
        run_test(a)


if __name__ == "__main__":
    main()
