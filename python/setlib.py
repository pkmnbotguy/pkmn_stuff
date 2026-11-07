"""Real opponent sets from the team pool (Teams_real), for filling in what hasn't been revealed about an opponent's Pokemon
(closed team sheets): instead of generic filler moves and a guessed spread, the tournament set that best matches what WAS
revealed (moves seen, item / ability if known).

    s = setlib.best_set("garchomp", known_moves={"earthquake"}, item=None, ability=None)
    s -> {"moves": [...], "item": "choicescarf", "ability": "roughskin", "points": {"hp": 2, ...}, "nature": "Jolly"} or None

The library (species id -> sets) is built from the team files once and cached in set_library.json (rebuilt when the pool
changes: the cache stores the number of team files).
"""

import paths
import functools
import glob
import json
import os
import re
import numpy as np
from collections import Counter

HERE = os.path.dirname(os.path.abspath(__file__))
CACHE = os.path.join(paths.CACHE, "set_library.json")
_LIB = None


_NON_ID = re.compile(r"[^a-z0-9]")


@functools.lru_cache(maxsize=65536)
def _to_id_str(s):
    return _NON_ID.sub("", s.lower())


def to_id(s):
    return _to_id_str(str(s))


def parse_block(blk):
    lines = [ln.strip() for ln in blk.strip().split("\n") if ln.strip()]
    if not lines:
        return None
    head = lines[0]
    item = head.split(" @ ", 1)[1].strip() if " @ " in head else ""
    head = re.sub(r"\s*\((M|F)\)\s*$", "", head.split(" @ ")[0].strip())
    m = re.match(r"^.*\((.+)\)$", head)
    species = m.group(1).strip() if m else head
    out = {"species": to_id(species), "item": to_id(item), "ability": "", "moves": [], "points": {}, "nature": "Serious"}
    for ln in lines[1:]:
        if ln.startswith("Ability:"):
            out["ability"] = to_id(ln.split(":", 1)[1])
        elif ln.startswith(("EVs:", "SPs:")):
            for part in ln.split(":", 1)[1].split("/"):
                try:
                    v, k = part.strip().split(" ", 1)
                    out["points"][{"hp": "hp", "atk": "atk", "def": "def", "spa": "spa", "spd": "spd", "spe": "spe"}[to_id(k)]] = int(v)
                except (ValueError, KeyError):
                    pass
        elif ln.endswith("Nature"):
            out["nature"] = ln.split()[0]
        elif ln.startswith("-"):
            mv = to_id(ln[1:])
            if mv:
                out["moves"].append(mv)
    return out


def library(folder=None):
    global _LIB
    if _LIB is not None:
        return _LIB
    folder = folder or os.path.join(paths.TEAMS, "Teams_real")
    files = sorted(glob.glob(os.path.join(folder, "*.txt")))
    cache = CACHE
    if os.path.basename(os.path.normpath(folder)) != "Teams_real":
        cache = CACHE.replace(".json", "_" + os.path.basename(os.path.normpath(folder)) + ".json")
    if os.path.exists(cache):
        try:
            c = json.load(open(cache, encoding="utf-8"))
            if c.get("n_files") == len(files):
                _LIB = c["sets"]
                return _LIB
        except (OSError, ValueError):
            pass
    lib = {}
    for f in files:
        for blk in open(f, encoding="utf-8").read().strip().split("\n\n"):
            s = parse_block(blk)
            if s and s["moves"]:
                lib.setdefault(s["species"], []).append(s)
    try:
        json.dump({"n_files": len(files), "sets": lib}, open(cache, "w", encoding="utf-8"))
    except OSError:
        pass
    _LIB = lib
    return lib


@functools.lru_cache(maxsize=4096)
def is_mega_stone(item):
    """Mega Stones end in 'ite' (plus the X / Y / Z forms), except a few ordinary items that happen to."""
    it = to_id(item)
    return bool(re.search(r"ite[xyz]?$", it)) and it not in ("eviolite", "whiteherb_", "meteorite")


_IDX = {}


def _index(species_id):
    """(2026-10-18 speed) per species: each set with its move set, item, ability, Mega-Stone flag and signature, computed once
    (best_set was ~40% of a searched decision's time on closed sheets)."""
    ix = _IDX.get(species_id)
    if ix is None:
        sets = library().get(species_id) or []
        ix = [(s, frozenset(s["moves"]), s["item"], s["ability"], is_mega_stone(s["item"]),
               (tuple(sorted(s["moves"])), s["item"], s["ability"])) for s in sets]
        _IDX[species_id] = ix
    return ix


def best_set(species, known_moves=(), item=None, ability=None, rng=None, mega_stone=None, weight=None, eps=0.2,
             exclude_items=None):
    """The library set of `species` most consistent with what is known (revealed moves weigh most, then a known item, then a
    known ability); among equally consistent sets the most common one. None if the species has no sets.
    With `rng` (numpy Generator): a random set among the fully consistent ones instead (None if there is none).
    With `rng` and `weight` (2026-10-20, a function set -> score, e.g. from the network's belief head): DISTINCT consistent
    sets drawn in proportion to their score with probability 1 - eps, else by library frequency as before (the belief
    already holds the prior, so the score is not multiplied by how often a set occurs).
    `exclude_items` (2026-10-30): items this Pokemon showed it doesn't hold; sets with them are
    skipped while any other consistent set remains (never used when the item is known)."""
    sid = to_id(species)
    ix = _index(sid)
    if not ix:
        return None
    known = frozenset(to_id(m) for m in known_moves)
    item, ability = (to_id(item) if item else None), (to_id(ability) if ability else None)
    exc = frozenset(exclude_items) if exclude_items and not item else frozenset()
    if len(_OK) + len(_BEST) > 200000:      # (bounded: a long ladder session meets many combinations)
        _OK.clear()
        _BEST.clear()
    if rng is not None:
        # a SAMPLE from the sets consistent with everything known (every revealed move, the known item / ability), so
        # common sets come up more often: one guess per search world instead of one fixed guess for all of them.
        # `mega_stone` (True / False): only sets whose item is / isn't a Mega Stone (keeps Mega actions legal everywhere).
        # The consistent sets depend only on the inputs: cached (the draw itself still uses rng, so results are unchanged)
        key = (sid, known, item, ability, mega_stone, exc)
        ok = _OK.get(key)
        if ok is None:
            ok = [e for e in ix if known <= e[1] and (not item or e[2] == item)
                  and (not ability or e[3] == ability)
                  and (mega_stone is None or e[4] == mega_stone)]
            if exc:
                ok = [e for e in ok if e[2] not in exc] or ok
            ok = [e[0] for e in ok]
            _OK[key] = ok
        if ok and weight is not None and rng.random() >= eps:
            sigs = {}
            for s in ok:
                sigs.setdefault((tuple(sorted(s["moves"])), s["item"], s["ability"]), s)
            uniq = list(sigs.values())
            w = np.array([max(0.0, float(weight(s))) for s in uniq])
            if w.sum() > 0:
                return uniq[int(rng.choice(len(uniq), p=w / w.sum()))]
        if ok:
            return ok[int(rng.integers(len(ok)))]
        return None
    key = (sid, known, item, ability, exc)
    hit = _BEST.get(key)
    if hit is not None:
        return hit
    if exc:
        ix = [e for e in ix if e[2] not in exc] or ix
    hit = _best_uncached(ix, known, item, ability)
    _BEST[key] = hit
    return hit


_OK, _BEST = {}, {}


def _best_uncached(ix, known, item, ability):
    best, top = [], None
    for e in ix:
        sc = 3 * len(known & e[1]) + (2 if item and e[2] == item else 0) + (1 if ability and e[3] == ability else 0)
        if top is None or sc > top:
            top, best = sc, [e]
        elif sc == top:
            best.append(e)
    want = Counter(e[5] for e in best).most_common(1)[0][0]
    return next(e[0] for e in best if e[5] == want)
