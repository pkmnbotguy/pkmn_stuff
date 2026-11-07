"""The bridge: a poke-env battle (what a player sees) -> a vgcsim engine position for the search and the network.
Our side exact (request stats -> stat points / nature); theirs from what the battle revealed, the real-set library
(setlib.py) for the rest, spread inference (spread_infer.py / spread_track.py) for stat points, plus volatiles,
timers, history, hazards, forms, PP, choice locks rebuilt from the log (log_vols.py / bridge_vols.py)."""

import paths
import os
import time
from logmon import log_mon
import random
from pathlib import Path
from poke_env.data import GenData


try:
    import vgcsim as _vgcsim
except Exception:                                   # engine not built: the bot still works without it
    _vgcsim = None

# poke-env battle-state corrections (pokeenv_fix.py: Psych Up, Baton Pass, Skill Swap, Spite...) and Illusion handling
# (illusion_fix.py: a disguised Zoroark's actions booked on the right Pokemon). POKEENV_FIX=0 / ILLUSION_FIX=0 turn them off.
if os.environ.get("POKEENV_FIX", "1") != "0":
    import pokeenv_fix
    pokeenv_fix.install()
if os.environ.get("ILLUSION_FIX", "1") != "0":
    import illusion_fix
    illusion_fix.install()


GUESS_FAST_SPEED = 70


GUESS_SPEED_NATURE = 100


GUESS_OFFENSE_SP = 32


GUESS_FAST_SPEED_SP = 32


GUESS_FAST_HP_SP = 2


GUESS_SLOW_HP_SP = 32


STATS = ("hp", "atk", "def", "spa", "spd", "spe")


PROTECT_MOVES = {
    "protect", "detect", "spikyshield", "kingsshield", "banefulbunker",
    "silktrap", "burningbulwark", "obstruct",
}


def guess_spread(base):
    """
    Guess an opponent's stat points and alignment from its base stats.
    Fast Pokemon: max offense + max speed. Slow Pokemon: max offense + HP.
    The actual build is unknown until revealed; this is a placeholder that
    we can later replace with real usage data.
    """
    sp = dict.fromkeys(STATS, 0)
    align = dict.fromkeys(STATS, 1.0)
    off, other = ("atk", "spa") if base["atk"] >= base["spa"] else ("spa", "atk")
    sp[off] = GUESS_OFFENSE_SP
    align[other] = 0.9
    if base["spe"] >= GUESS_FAST_SPEED:
        sp["spe"] = GUESS_FAST_SPEED_SP
        sp["hp"] = GUESS_FAST_HP_SP
        if base["spe"] >= GUESS_SPEED_NATURE:
            align["spe"] = 1.1       # Jolly / Timid style
        else:
            align[off] = 1.1
    else:
        sp["hp"] = GUESS_SLOW_HP_SP
        sp["def"] = 1
        sp["spd"] = 1
        align[off] = 1.1
    return sp, align


def names(collection):
    """Names of the keys of a dict/set of enums (weather, fields, side conditions)."""
    return {getattr(k, "name", str(k)) for k in collection}


_GEN9 = None


def gen9():
    global _GEN9
    if _GEN9 is None:
        _GEN9 = GenData.from_gen(9)
    return _GEN9


def to_id(text):
    return "".join(c for c in str(text).lower() if c.isalnum())


_EXTENDERS = {"raindance": "damprock", "sunnyday": "heatrock", "sandstorm": "smoothrock", "snowscape": "icyrock",
              "snow": "icyrock", "hail": "icyrock", "electricterrain": "terrainextender", "grassyterrain": "terrainextender",
              "psychicterrain": "terrainextender", "mistyterrain": "terrainextender", "reflect": "lightclay",
              "lightscreen": "lightclay", "auroraveil": "lightclay"}


_BASE_TURNS = {"tailwind": 4, "trickroom": 5}


_PSEUDO_FIELD = ("trickroom", "gravity", "magicroom", "wonderroom")


def field_timers(battle, filled_items=None):
    """(2026-10-10) Turns left of weather / terrain / Trick Room / Tailwind / screens, counted from the battle's own log
    (battle._replay_data): duration (5, or 8 when the setter holds the extending item) minus the end-of-turn ticks
    ('|upkeep|') since it started. poke-env re-stamps the weather's start turn at every upkeep and the old estimate kept
    terrain at 5: the search simulated rain / terrain that never ran out (bridge audit: wrong in ~40% of decisions).
    Returns {key: turns left}; keys 'weather', 'terrain', 'trickroom', ('p1'|'p2', 'tailwind'|'reflect'|...).
    filled_items: species -> item id filled in for an opponent's unrevealed item (build_engine's library sets)."""
    live = {}                       # key -> [effect id, duration, setter ident, ticks]
    last_user = None

    def setter_of(ev):
        for part in ev[3:]:
            if part.startswith("[of] "):
                return part[5:].strip()
        return last_user

    def start(key, eid, ev):
        live[key] = [eid, _BASE_TURNS.get(eid, 5), setter_of(ev), 0]

    for ev in getattr(battle, "_replay_data", []):
        if len(ev) < 2:
            continue
        kind = ev[1]
        if kind == "move" and len(ev) > 2:
            last_user = ev[2]
        elif kind == "upkeep":
            for v in live.values():
                v[3] += 1
        elif kind == "-weather" and len(ev) > 2:
            eid = to_id(ev[2])
            if eid == "none":
                live.pop("weather", None)
            elif "[upkeep]" not in ev:            # '[upkeep]' = the same weather continuing
                start("weather", eid, ev)
        elif kind == "-fieldstart" and len(ev) > 2:
            # (2026-10-18) Gravity / Magic Room / Wonder Room have their own timers (they overwrote the terrain's)
            eid = to_id(ev[2].replace("move:", ""))
            start(eid if eid in _PSEUDO_FIELD else "terrain", eid, ev)
        elif kind == "-fieldend" and len(ev) > 2:
            eid = to_id(ev[2].replace("move:", ""))
            live.pop(eid if eid in _PSEUDO_FIELD else "terrain", None)
        elif kind == "-swapsideconditions":
            # (2026-10-18) Court Change: the two sides' conditions trade places, timers included
            live = {((("p2" if k[0] == "p1" else "p1"), k[1]) if isinstance(k, tuple) else k): v for k, v in live.items()}
        elif kind in ("-sidestart", "-sideend") and len(ev) > 3:
            eid = to_id(ev[3].replace("move:", ""))
            key = (ev[2][:2], eid)
            if kind == "-sidestart":
                start(key, eid, ev)
            else:
                live.pop(key, None)
    out = {}
    for key, (eid, dur, setter, ticks) in live.items():
        ext = _EXTENDERS.get(eid)
        if ext and setter:
            try:
                mon = log_mon(battle, setter)
                it = to_id(getattr(mon, "item", "") or "")
                if it == "unknownitem" and filled_items:
                    # (2026-10-15) hidden item: use the one this search world filled in from the real sets (setlib), so
                    # an opponent's Damp Rock / Heat Rock / Light Clay... counts in the worlds that sampled it
                    it = filled_items.get(mon.species, it)
                if it == ext:
                    dur = 8
            except Exception:
                pass
        if ext and ticks >= dur:
            dur = 8                     # still up after 5 ticks: the setter must hold the extending item
        out[key] = max(1, dur - ticks)
    return out


_ENGINE = {"dex": None, "fillers": None}


NATURES = {
    ("atk", "def"): "Lonely", ("atk", "spa"): "Adamant", ("atk", "spd"): "Naughty", ("atk", "spe"): "Brave",
    ("def", "atk"): "Bold", ("def", "spa"): "Impish", ("def", "spd"): "Lax", ("def", "spe"): "Relaxed",
    ("spa", "atk"): "Modest", ("spa", "def"): "Mild", ("spa", "spd"): "Rash", ("spa", "spe"): "Quiet",
    ("spd", "atk"): "Calm", ("spd", "def"): "Gentle", ("spd", "spa"): "Careful", ("spd", "spe"): "Sassy",
    ("spe", "atk"): "Timid", ("spe", "def"): "Hasty", ("spe", "spa"): "Jolly", ("spe", "spd"): "Naive",
}


ENGINE_DATA = Path(paths.DEX)


def engine_dex():
    if _ENGINE["dex"] is None:
        _ENGINE["dex"] = _vgcsim.Dex(str(ENGINE_DATA))
    return _ENGINE["dex"]


def filler_moves():
    """
    For each (type, category): a plain move of about 90 power to stand for an opponent's unseen
    attack. Scored rather than filtered, so every type gets one: close to 90 power, accurate, with
    few side effects; never moves that switch, charge, hit several times, or change the user's stats.
    """
    if _ENGINE["fillers"] is None:
        import json
        data = json.loads(ENGINE_DATA.read_text(encoding="utf-8"))["moves"]
        best = {}
        for mid, m in data.items():
            if m.get("category") not in ("Physical", "Special"):
                continue
            if m.get("target") not in ("normal", "any", "adjacentFoe"):
                continue
            flags = m.get("flags") or {}
            skip = (m.get("selfSwitch") or flags.get("charge") or m.get("multihit") or m.get("priority")
                    or m.get("ohko") or m.get("damage") or (m.get("self") or {}).get("boosts")
                    or m.get("selfdestruct") or m.get("forceSwitch") or not flags.get("protect")
                    or mid == "spectralthief")
            if skip:
                continue
            hooks = [h for h in (m.get("hooks") or []) if h != "onPrepareHit"]
            bp = m.get("basePower") or 0
            if bp < 40 or hooks:
                continue
            acc = 100 if m.get("accuracy") is True else (m.get("accuracy") or 0)
            score = abs(bp - 90) + (100 - acc) * 1.5 + 8 * len(m.get("secondaries") or [])                 + (15 if m.get("recoil") or m.get("drain") else 0) + (20 if m.get("isNonstandard") else 0)                 + (10 if (m.get("critRatio") or 1) > 1 else 0)
            key = (m["type"].upper(), m["category"].upper())
            if key not in best or score < best[key][1]:
                best[key] = (mid, score)
        _ENGINE["fillers"] = {k: v[0] for k, v in best.items()}
    return _ENGINE["fillers"]


def infer_points(mon, base=None, nature=None):
    """Our own Pokemon: recover stat points and nature from its exact stats (Champions formula).
    Solves for the nature and all points together: a raised stat can also look like a neutral one
    with more points, so prefer the consistent combination that uses the full 66 points.
    base: the base stats of the form the request's stats belong to (2026-10-18: poke-env keeps a temporary form's base
    stats after it ended: an Aegislash back in Shield forme was solved against Blade's and got 135 max HP instead of 165)."""
    own_base = base is not None
    base = base or mon.base_stats
    stats = mon.stats or {}
    own = base if own_base else (getattr(mon, "_base_stats", None) or base)
    # (2026-10-18) transformed (Ditto): poke-env's base_stats / the request's stats are the COPIED ones, but HP stays its own:
    # HP points from its own base HP (gave Ditto 123 max HP instead of 136); the other stats are copied anyway
    hp_pts = max(0, min(32, (mon.max_hp or 0) - own["hp"] - 75))
    if getattr(mon, "transformed", False):
        return {"hp": hp_pts, **{k: 0 for k in ("atk", "def", "spa", "spd", "spe")}}, "Serious"
    keys = ("atk", "def", "spa", "spd", "spe")

    def points_for(st, num):
        real = stats.get(st)
        if not real:
            return 0
        for p in range(33):
            raw = base[st] + p + 20
            value = raw if num == 100 else ((raw * num) & 0xFFFF) // 100
            if value == real:
                return p
        return None

    best = None
    # (2026-10-20) nature= (known from a team sheet): only that nature (two natures can give the same stats)
    want = next(((pl, mi) for (pl, mi), nm in NATURES.items() if to_id(nm) == to_id(nature)), None) if nature else None
    if nature and want is None and to_id(nature) in {to_id(n) for n in ("Hardy", "Docile", "Serious", "Bashful", "Quirky")}:
        want = (None, None)
    for plus in (None,) + keys:
        for minus in (None,) + keys:
            if (plus is None) != (minus is None) or (plus is not None and plus == minus):
                continue
            if want is not None and (plus, minus) != want:
                continue
            pts = {"hp": hp_pts}
            ok = True
            for st in keys:
                num = 110 if st == plus else 90 if st == minus else 100
                p = points_for(st, num)
                if p is None:
                    ok = False
                    break
                pts[st] = p
            if not ok:
                continue
            total = sum(pts.values())
            if total > 66:
                continue
            key = (total == 66, total)
            if best is None or key > best[0]:
                best = (key, pts, NATURES.get((plus, minus), "Serious"))
    if best is None:
        return {"hp": hp_pts, **{k: 0 for k in keys}}, "Serious"     # keep the HP points (exact from max HP)
    return best[1], best[2]


_ENGINE_SPECIES = None


BELIEF_EPS = float(os.environ.get("AGENT_BELIEF_EPS", "0.2"))


SPREAD_TO_NET = os.environ.get("SPREAD_TO_NET", "1") == "1"      # inferred opponent stats into the network input (live play setting)


def engine_species(sid):
    """The engine's data has no purely cosmetic forms (Furfrou trims, Vivillon patterns, Alcremie...): those play like the
    base species, so an unknown form becomes its base species."""
    global _ENGINE_SPECIES
    if _ENGINE_SPECIES is None:
        import json as _json
        import os
        path = paths.DEX
        try:
            _ENGINE_SPECIES = set(_json.load(open(path, encoding="utf-8"))["species"])
        except (OSError, KeyError, ValueError):
            _ENGINE_SPECIES = set()
    if not _ENGINE_SPECIES or to_id(sid) in _ENGINE_SPECIES:
        return sid
    entry = gen9().pokedex.get(to_id(sid)) or {}
    base = to_id(entry.get("baseSpecies") or "")
    return base if base in _ENGINE_SPECIES else sid


def base_species_id(name):
    from bridge_util import base_id
    return base_id(name)


_FAIL_MARKS = ("-fail", "-miss", "-immune", "-notarget")


_ABSORB = ("Flash Fire", "Volt Absorb", "Water Absorb", "Earth Eater", "Dry Skin", "Lightning Rod", "Storm Drain",
           "Motor Drive", "Sap Sipper", "Well-Baked Body", "Wind Rider", "Eelevate")


def absorbed(ev):
    """(2026-10-19) a log line where the target's absorbing ability took the move (Showdown: TryHit returns null =
    the move failed on it): '-start|X|ability: Flash Fire', '-heal / -boost ... [from] ability: Volt Absorb' etc."""
    if len(ev) < 4 or ev[1] not in ("-start", "-heal", "-boost"):
        return False
    txt = "|".join(ev[3:])
    if ev[1] == "-start":
        return "ability: Flash Fire" in txt
    return any(f"[from] ability: {a}" in txt for a in _ABSORB)


_GUARDS = ("Protect", "Detect", "Spiky Shield", "King's Shield", "Baneful Bunker", "Silk Trap", "Burning Bulwark",
           "Wide Guard", "Quick Guard", "Max Guard", "Obstruct")


_SINGLE_TARGET = ("normal", "any", "adjacentfoe", "adjacentally", "adjacentallyorself", "randomnormal")


def _last_timestamp(battle):
    for ev in reversed(getattr(battle, "_replay_data", None) or []):
        if len(ev) > 2 and ev[1] == "t:":
            return str(ev[2])
    return str(int(time.time()))


def _active_key(mons):
    from encoding import to_id as enc_id
    out = []
    for m in (list(mons or []) + [None, None])[:2]:
        out.append(enc_id(m.species) if m is not None and not getattr(m, "fainted", False) else "-")
    return "+".join(out)


def turn_state_key(battle):
    """Per-turn state string for deterministic search and sampling."""
    if battle is None:
        return f"turn=0|t={int(time.time())}|p1=-+-|p2=-+-"
    return (f"turn={battle.turn}|t={_last_timestamp(battle)}|p1={_active_key(battle.active_pokemon)}"
            f"|p2={_active_key(battle.opponent_active_pokemon)}")


def moved_since_switch(battle):
    """(2026-10-11) {id(poke-env mon): it has taken a move action since it last switched in} from the battle log (Showdown's
    activeMoveActions > 0: a move, or a turn it couldn't move). poke-env's first_turn (= active turns == 1) only updates at
    the next '|turn|', so at a replacement between turns it described the PREVIOUS turn: a foe that had just attacked looked
    fresh (Fake Out possible) and one that had just switched in looked used; the search's value moved ~20 points."""
    out = {}
    for ev in getattr(battle, "_replay_data", []):
        if len(ev) < 3 or ev[1] not in ("switch", "drag", "move", "cant"):   # (2026-10-19) "replace" = Illusion broken, same stint
            continue
        if ev[1] == "move" and any(str(x).startswith("[from]") for x in ev[5:]):
            continue                                       # a move called by another move: the same action
        try:
            m = log_mon(battle, ev[2])
        except Exception:
            continue
        out[id(m)] = ev[1] in ("move", "cant")
    return out


_TT = None


def _TWO_TURN_IDS():
    """ids of two-turn (charge) moves: Solar Beam, Electro Shot, Fly, Dig, Phantom Force..."""
    global _TT
    if _TT is None:
        try:
            _TT = {k for k, v in gen9().moves.items() if "charge" in (v.get("flags") or {})}
        except Exception:
            _TT = set()
    return _TT


def switched_this_turn(battle):
    """(2026-10-19 forced behaviour check) {id(poke-env mon)} that switched in (switch / drag) after the last '|turn|' line:
    Showdown's activeTurns is 0 for them until the end of the turn (no Speed Boost / Moody at this end of turn, Stakeout
    doubles against them). A rebuilt position mid-turn (U-turn, Eject Button, replacement decisions) needs it."""
    out = set()
    for ev in getattr(battle, "_replay_data", []):
        if len(ev) < 3:
            continue
        if ev[1] == "turn":
            out = set()
        elif ev[1] in ("switch", "drag"):
            try:
                out.add(id(log_mon(battle, ev[2])))
            except Exception:
                pass
    return out


def status_progress(battle):
    """(2026-10-18, bridge check) {id(poke-env mon): (status, attempts, rest, toxic_stage)} from the battle log, for the
    status counters a rebuilt position can't see: sleep / freeze = BeforeMove attempts since the status started ('|cant|X|slp'
    or 'frz', or a move used while asleep: Sleep Talk, Snore); rest = the sleep came from Rest (exactly 3 turns, not 2-3);
    toxic_stage = end-of-turn ticks ('|upkeep|') while badly poisoned AND on the field since it last switched in (Showdown
    resets the stage on switch-in). Sleep counts survive switching out (gen 9)."""
    out, pos = {}, {}
    just_cant = set()       # '|cant|X|slp' then a sleep-usable move (Sleep Talk, Snore) = ONE attempt (Showdown logs both)
    for ev in getattr(battle, "_replay_data", []):
        if len(ev) < 2:
            continue
        kind = ev[1]
        if kind == "turn":
            just_cant = set()
            continue
        if kind == "upkeep":
            for mid in pos.values():
                st = out.get(mid)
                if st and st[0] == "tox":
                    out[mid] = (st[0], st[1], st[2], st[3] + 1)
            continue
        if kind == "swap" and len(ev) > 2:
            # Ally Switch: the two Pokemon trade slots (2026-10-18; a stale slot map let the next switch-in wipe the partner)
            sd_ = str(ev[2])[:2]
            pa, pb = pos.pop(sd_ + "a", None), pos.pop(sd_ + "b", None)
            if pa is not None:
                pos[sd_ + "b"] = pa
            if pb is not None:
                pos[sd_ + "a"] = pb
            continue
        if len(ev) < 3 or kind not in ("switch", "drag", "replace", "faint", "-status", "-curestatus", "cant", "move"):
            continue
        try:
            m = log_mon(battle, ev[2])
        except Exception:
            continue
        mid = id(m)
        if kind in ("switch", "drag", "replace"):
            slot = str(ev[2]).split(":")[0][:3]
            old = pos.get(slot)
            if kind != "replace" and old is not None and old != mid and (out.get(old) or ("",))[0] == "tox":
                # the Pokemon leaving the slot: the engine resets its toxic stage on switch-out (Showdown on its next
                # switch-in), so a benched one must hold 0 (2026-10-18 bridge fuzz: 3/16 instead of 1/16 on its return)
                o = out[old]
                out[old] = (o[0], o[1], o[2], 0)
            pos = {k: v for k, v in pos.items() if v != mid}
            pos[slot] = mid
            st = out.get(mid)
            if st and st[0] == "tox":
                out[mid] = ("tox", 0, False, 0)
        elif kind == "faint":
            pos = {k: v for k, v in pos.items() if v != mid}
            out.pop(mid, None)
        elif kind == "-status" and len(ev) > 3:
            out[mid] = (ev[3], 0, ev[3] == "slp" and any("Rest" in str(x) for x in ev[4:]), 0)
        elif kind == "-curestatus":
            out.pop(mid, None)
        elif kind == "cant" and len(ev) > 3 and ev[3] in ("slp", "frz"):
            st = out.get(mid)
            if st and st[0] == ev[3]:
                out[mid] = (st[0], st[1] + 1, st[2], st[3])
                just_cant.add(mid)
        elif kind == "move":
            st = out.get(mid)
            if mid in just_cant:
                just_cant.discard(mid)
            elif st and st[0] == "slp" and not any(str(x).startswith("[from]") for x in ev[5:]):
                out[mid] = (st[0], st[1] + 1, st[2], st[3])   # Sleep Talk / Snore while asleep (a wake shows -curestatus first)
    return out


def hp_colours(battle):
    """(2026-10-19) {id(poke-env mon): colour letter of its latest HP line}. Champions writes '50/100g' / '50/100y' and
    '20/100y' / '20/100r' (Showdown getHealth): the letter tells on which side of 1/2 or 1/5 the true HP lies."""
    out = {}
    for ev in getattr(battle, "_replay_data", []):
        if len(ev) < 4 or ev[1] not in ("switch", "drag", "replace", "-damage", "-heal", "-sethp"):
            continue
        hp = ev[4] if ev[1] in ("switch", "drag", "replace") and len(ev) > 4 else ev[3]
        try:
            m = log_mon(battle, ev[2])
        except Exception:
            continue
        if m is None:
            continue
        num = str(hp).split(" ")[0]
        out[id(m)] = num[-1] if num and num[-1] in "gyr" else ""
    return out


def hp_from_percent(frac, maxhp, colour=""):
    """(2026-10-18 bridge fuzz) An opponent's HP from the percentage Showdown shows: floor(100 * hp / maxhp), at least 1
    (checked on 1,366 log lines), so the true HP lies in a range; take its middle. round(frac * maxhp) took the range's
    bottom (biased low by ~half a percent: 26 instead of 27 of 185 decided a KO)."""
    p = int(round(frac * 100))
    if p >= 100:
        return maxhp
    if p <= 0:
        return 1
    lo = 1 if p == 1 else -(-p * maxhp // 100)                 # smallest hp with floor(100 hp / maxhp) == p
    hi = min(maxhp - 1, -(-(p + 1) * maxhp // 100) - 1)          # largest
    # (2026-10-19) the colour letter at 50 / 20 (Champions getHealth): g = hp*2 > maxhp, y = not; at 20 y = hp*5 > maxhp, r = not
    if p == 50 and colour == "g":
        lo = max(lo, maxhp // 2 + 1)
    elif p == 50 and colour == "y":
        hi = min(hi, maxhp // 2)
    elif p == 20 and colour == "y":
        lo = max(lo, maxhp // 5 + 1)
    elif p == 20 and colour == "r":
        hi = min(hi, maxhp // 5)
    hi = max(lo, hi)
    return (lo + hi + 1) // 2


def lost_item_since_switch(battle):
    """(2026-10-14) {id(poke-env mon): it lost its item since it last switched in} from the battle log ('-enditem': eaten,
    used, knocked off, popped; '-item' = it got one again). Unburden's speed boost lives on a volatile that a rebuilt
    position can't infer from 'no item' alone (it ends on switching out)."""
    out = {}
    for ev in getattr(battle, "_replay_data", []):
        if len(ev) < 3 or ev[1] not in ("switch", "drag", "-enditem", "-item"):   # (2026-10-19) not "replace": same stint
            continue
        try:
            m = log_mon(battle, ev[2])
        except Exception:
            continue
        out[id(m)] = ev[1] == "-enditem"
    return out


def choice_lock_moves(battle):
    """(2026-10-14) {id(poke-env mon): the move it is locked into if it holds a Choice item} from the battle log: the first
    move it used since it last switched in (not one called by another move, '[from]'), reset when it switches out or its
    item changes ('-enditem' / '-item': Trick, Knock Off...). The engine applies it only to a Choice item holder."""
    out = {}
    for ev in getattr(battle, "_replay_data", []):
        if len(ev) < 3 or ev[1] not in ("switch", "drag", "-enditem", "-item", "move"):   # (2026-10-19) not "replace" (Illusion broken): the lock stays
            continue
        try:
            m = log_mon(battle, ev[2])
        except Exception:
            continue
        if ev[1] == "-item" and any(str(x) in ("[from] ability: Frisk", "[identify]") for x in ev[4:]):
            continue                                         # (2026-10-18) Frisk only reveals the item: the lock stays
        if ev[1] != "move":
            out[id(m)] = None
        elif len(ev) > 3 and not any(str(x).startswith("[from]") for x in ev[4:]) and out.get(id(m)) is None:
            mid = "".join(c for c in ev[3].lower() if c.isalnum())
            if mid not in ("struggle", "recharge"):
                out[id(m)] = mid
    return out


def log_history(battle):
    """(2026-10-10) The network's history inputs (engine Mon::h_*, encode.rs encode_hist), rebuilt from the battle log:
    {id(poke-env mon): {"move": last move id since it switched in (None), "target": 1 / 2 opposing slot a / b, 3 ally,
    0 itself / none / spread, "streak": uses of that move in a row, "protect": it was a Protect-type move,
    "result": its move in the previous turn -1 none / 0 failed or hit nothing / 1 worked, "uses": {move id: uses this
    battle}, "protects": n, "status": n}}. Moves called by other moves ('[from]') and the second turn of a charge move
    don't count (as in the engine)."""
    from poke_env.battle import Move
    h, pos, charging, start_slot = {}, {}, {}, {}
    turn = 0
    cur = None                             # [mon key, turn, failed?, hit something?] of the move being resolved

    def mon_of(ident):
        try:
            return log_mon(battle, ident)
        except Exception:
            return None

    def rec(m):
        return h.setdefault(id(m), {"move": None, "target": 0, "streak": 0, "protect": False, "res": None,
                                    "uses": {}, "protects": 0, "status": 0})

    def slot_of_target(t):
        # (2026-10-19 bridge fuzz) '[notarget]' lines name the target without a slot ('p1: Persian'): the slot it stood in
        t = str(t)
        if len(t) > 3 and t[2] in "ab":
            return t[:3]
        m_ = mon_of(t)
        for s_, x_ in pos.items():
            if x_ is m_ and m_ is not None and s_[:2] == t[:2]:
                return s_
        return ""

    def close():
        nonlocal cur
        if cur is not None:
            r = rec(cur[0])
            # failed for Stomping Tantrum: nothing hit and not ONLY Protect & co. (Showdown NOT_FAIL; engine battle.rs, 2026-10-19)
            r["res"] = (cur[1], 0 if (cur[2] and not cur[3]) else 1, int(cur[5] and not cur[3]))
            cur = None

    for ev in getattr(battle, "_replay_data", []):
        if len(ev) < 2:
            continue
        kind = ev[1]
        if kind in ("move", "turn", "upkeep", "switch", "drag", "cant", "faint"):
            if kind != "faint":
                close()
        if kind == "turn" and len(ev) > 2:
            try:
                turn = int(ev[2])
            except ValueError:
                pass
            # (2026-10-19) slots when the turn's choices were made (Ally Switch moves a Pokemon before it acts)
            start_slot = {id(x_): s_ for s_, x_ in pos.items() if x_ is not None}
        elif kind == "cant" and len(ev) > 3:
            # (2026-10-18) a move stopped in BeforeMove (flinch, paralysis, sleep, Taunt...) counts as FAILED for Stomping
            # Tantrum (Showdown moveThisTurnResult = false); only 'recharge' returns null. The network's result stays -1.
            m = mon_of(ev[2])
            if m is not None and ev[3] != "recharge":
                rec(m)["cantf"] = turn
            if m is not None:
                charging.pop(id(m), None)          # (2026-10-19) stopped on the release turn (paralysis...): the charge is lost
        elif kind == "swap" and len(ev) > 2:
            # Ally Switch: the two Pokemon trade slots (2026-10-18; a stale slot map let the next switch-in wipe the partner)
            sd_ = str(ev[2])[:2]
            pa, pb = pos.pop(sd_ + "a", None), pos.pop(sd_ + "b", None)
            if pa is not None:
                pos[sd_ + "b"] = pa
            if pb is not None:
                pos[sd_ + "a"] = pb
        elif kind in ("switch", "drag") and len(ev) > 2:
            slot = ev[2][:3]
            old = pos.get(slot)
            m = mon_of(ev[2])
            for x in (old, m):
                if x is not None:
                    r = rec(x)
                    r["move"], r["streak"], r["protect"], r["target"], r["loc"] = None, 0, False, 0, 0
                    charging.pop(id(x), None)
            pos[slot] = m
        elif kind == "move" and len(ev) > 3:
            m = mon_of(ev[2])
            if m is None:
                continue
            # ('[from] move: Round' = moved up by the partner's Round: its own chosen move; 2026-10-18)
            flags = "|".join(x for x in ev[5:] if str(x) != "[from] move: Round") if len(ev) > 5 else ""
            mid = to_id(ev[3])
            cur = [m, turn, False, False, None, False]   # [4]: the history record to complete from '-anim'; [5]: a real fail
            if "[from]" in flags or mid in ("struggle", "recharge"):
                # (2026-10-19) a charge move's release logged '[from] lockedmove' ends the charge (else the NEXT charge of
                # the same move looked like a second turn and was skipped: Encore then copied a stale last move)
                if "lockedmove" in flags.replace(" ", "") and charging.get(id(m)) == mid:
                    charging.pop(id(m), None)
                # (2026-10-19) the release of a two-turn move CALLED by another (Copycat -> Sky Attack) is a real use in
                # Showdown (moveUsed with the charge's stored target): lastMoveTargetLoc moves there, the caller's is gone
                r_ = h.get(id(m))
                # (2026-10-19 bridge fuzz) only a two-turn move's release: a LOCK move's later turns (Uproar, Outrage called
                # by Copycat) keep the stored target (Showdown's locked choice reuses lastMoveTargetLoc)
                if ("lockedmove" in flags.replace(" ", "") and r_ is not None and r_.get("move") not in (None, mid)
                        and mid in _TWO_TURN_IDS()
                        and len(ev) > 4 and len(str(ev[4])) > 3 and str(ev[4])[2:3] in ("a", "b")):
                    t_, s_ = str(ev[4])[:3], str(ev[2])[:3]
                    n_ = 1 if t_[2] == "a" else 2
                    r_["loc"] = n_ if t_[:2] != s_[:2] else -n_
                continue
            if charging.get(id(m)) == mid:
                charging.pop(id(m), None)          # the charge move's second turn
                continue
            r = rec(m)
            try:
                mv = Move(mid, gen=9)
                target_kind, cat = mv.target.name if hasattr(mv.target, "name") else str(mv.target), mv.category.name
            except Exception:
                target_kind, cat = "normal", "PHYSICAL"
            src, tgt = ev[2][:3], (slot_of_target(ev[4]) if len(ev) > 4 and ev[4][:2] in ("p1", "p2") else "")
            if not tgt or "[spread]" in flags or not to_id(target_kind) in _SINGLE_TARGET:
                code = 0
            elif tgt[:2] != src[:2]:
                code = 1 if tgt[2] == "a" else 2
            else:
                code = 0 if tgt == src else 3
            r["streak"] = r["streak"] + 1 if r["move"] == mid else 1
            r["move"], r["target"] = mid, code
            # (2026-10-19 bridge flag 19724) Showdown's lastMoveTargetLoc (what Instruct re-aims at) in the engine's encoding:
            # + foe slot + 1, - own slot + 1 (the user's ally), 0 none / spread / self
            r["loc"] = 0 if code == 0 else ((1 if tgt[2] == "a" else 2) if code in (1, 2) else -(1 if tgt[2] == "a" else 2))
            if tgt and tgt == src and "[spread]" not in flags and to_id(target_kind) in _SINGLE_TARGET:
                # (2026-10-19) a single-target move aimed at the user itself (Acupressure, non-Ghost Curse: Champions makes it
                # 'self' after the queue drew the user) stores the user's own location
                r["loc"] = -(1 if src[2] == "a" else 2)
            elif to_id(target_kind) in ("self", "all", "allyside", "allyteam", "adjacentallyorself") and not (
                    tgt and tgt != src and to_id(target_kind) == "adjacentallyorself"):
                # (2026-10-19 bridge fuzz) no target to choose: Showdown's queue fills in getRandomTarget = the user itself
                # (Copycat, Swords Dance, Tailwind...), i.e. the user's own location; a lock move it calls keeps it.
                # The queue fills it in when the turn starts: the slot held then (Ally Switch may have moved the user since)
                s0_ = start_slot.get(id(m), src)
                r["loc"] = -(1 if s0_[2] == "a" else 2)
            elif not tgt and mid == "curse":
                # (2026-10-19) a failed non-Ghost Curse ('Curse||[still]'): no target chosen, Showdown's queue drew the user
                s0_ = start_slot.get(id(m), src)
                r["loc"] = -(1 if s0_[2] == "a" else 2)
            # (2026-10-19 bridge fuzz) Showdown keeps the CHOSEN target (before retargeting / redirection): ours is recorded
            ch = (getattr(battle, "_bridge_choices", {}).get(turn) or {}).get(start_slot.get(id(m), src)[2:3])
            if (src[:2] == getattr(battle, "player_role", None) and ch and ch[0] == mid and ch[1] in (1, 2, -1, -2)
                    and to_id(target_kind) in _SINGLE_TARGET):
                r["loc"] = ch[1]
            if not tgt and to_id(target_kind) in _SINGLE_TARGET:
                cur[4] = (r, src)          # the target shows on the '-anim' line (moves that charge and fire at once)
            r["protect"] = mid in PROTECT_MOVES
            r["uses"][mid] = r["uses"].get(mid, 0) + 1
            if r["protect"]:
                r["protects"] += 1
            elif cat == "STATUS":
                r["status"] += 1
        elif kind == "-prepare" and len(ev) > 3 and not any(str(x) == "[premajor]" for x in ev[4:]):
            # (2026-10-19) '[premajor]' = Chilly Reception's message BEFORE its move line, not a charge turn
            m = mon_of(ev[2])
            if m is not None:
                charging[id(m)] = to_id(ev[3])
        elif kind == "-anim" and len(ev) > 3:
            # the move went off this turn (Electro Shot in rain, Solar Beam in sun...): no second turn to skip
            m = mon_of(ev[2])
            if m is not None and charging.get(id(m)) == to_id(ev[3]):
                charging.pop(id(m), None)
            if cur is not None and cur[4] is not None and len(ev) > 4 and ev[4][:2] in ("p1", "p2")                     and slot_of_target(ev[4]):
                r, src = cur[4]
                # (2026-10-19 bridge fuzz) '[notarget]' names a fainted target without a slot ('p1: Staraptor')
                tgt = slot_of_target(ev[4])
                r["target"] = (1 if tgt[2] == "a" else 2) if tgt[:2] != src[:2] else (0 if tgt == src else 3)
                r["loc"] = 0 if r["target"] == 0 else ((1 if tgt[2] == "a" else 2) * (1 if r["target"] in (1, 2) else -1))
                cur[4] = None
        elif cur is not None:
            if kind in _FAIL_MARKS:
                cur[2] = cur[5] = True
            elif absorbed(ev) and mon_of(ev[2]) is not cur[0]:
                # (2026-10-19 bridge fuzz) an absorbing ability (Flash Fire, Volt Absorb, Lightning Rod...) makes the hit
                # fail on that target, as '-immune' does: Stomping Tantrum next turn is doubled
                cur[2] = cur[5] = True
            elif kind == "-activate" and "ability: Telepathy" in "|".join(ev[3:]) and mon_of(ev[2]) is not cur[0]:
                # (2026-10-19 bridge flag 14287) Telepathy's TryHit returns null: a real fail (Metronome resets, Stomping
                # Tantrum doubles), unlike Protect's NOT_FAIL
                cur[2] = cur[5] = True
            elif kind == "-activate" and any(g in "|".join(ev[2:]) for g in _GUARDS):
                cur[2] = True
            elif kind == "-damage" and len(ev) > 2 and "[from]" not in "|".join(ev[3:]) and mon_of(ev[2]) is not cur[0]:
                cur[3] = True
    close()
    for r in h.values():
        res = r.pop("res")
        # the previous turn's result, as the engine keeps it (last_move_result): during turn T, the result of turn T - 1
        r["result"] = res[1] if res is not None and res[0] == turn - 1 else -1
        cantf = r.pop("cantf", None)
        r["failed"] = int((res is not None and res[0] == turn - 1 and res[2] == 1) or cantf == turn - 1)
    return h


_HAS_HIST = None


def HAS_HISTORY():
    """The vgcsim module takes history inputs (built from the 2026-10-10 sources: set_mon(history=...))."""
    global _HAS_HIST
    if _HAS_HIST is None:
        try:
            _HAS_HIST = "history" in (_vgcsim.Battle.set_mon.__text_signature__ or "")
        except Exception:
            _HAS_HIST = False
    return _HAS_HIST


def history_args(rec, slots):
    """set_mon(history=...) for one Pokemon from log_history's record and its engine move slots {move id: slot}."""
    if rec is None:
        return {"slot": -1, "target": 0, "streak": 0, "protect": 0, "result": -1, "protects": 0, "status": 0,
                "uses0": 0, "uses1": 0, "uses2": 0, "uses3": 0}
    out = {"slot": slots.get(rec["move"], -1) if rec["move"] else -1, "target": rec["target"], "streak": rec["streak"],
           "protect": int(rec["protect"]), "result": rec["result"], "protects": rec["protects"], "status": rec["status"],
           "failed": rec.get("failed", int(rec["result"] == 0)), "loc": rec.get("loc", 0)}
    for k in range(4):
        out[f"uses{k}"] = 0
    for mid, n in rec["uses"].items():
        k = slots.get(mid)
        if k is not None and k < 4:
            out[f"uses{k}"] = n
    return out


def post_transform_abilities(battle):
    """(2026-10-19 bridge flag 20445) {id(poke-env mon): ability id} for Pokemon whose ability changed AFTER their last
    Transform in the current stint (Entrainment, Worry Seed, Simple Beam, Role Play, Doodle, Skill Swap with a foe, Mummy,
    Trace): the bridge's Transform copies the target's ability, which overwrote these (a Transformed Ditto given Sweet Veil
    by Entrainment kept the copied Strong Jaw)."""
    out, tf = {}, set()

    def mon_id(ident):
        try:
            return id(log_mon(battle, ident))
        except Exception:
            return None
    for ev in getattr(battle, "_replay_data", []):
        if len(ev) < 3:
            continue
        kind = ev[1]
        if kind == "-transform":
            k = mon_id(ev[2])
            tf.add(k)
            out.pop(k, None)
        elif kind in ("switch", "drag", "faint"):          # (2026-10-19) not "replace": an Illusion break keeps the stint
            k = mon_id(ev[2])
            tf.discard(k)
            out.pop(k, None)
        elif kind == "-ability" and len(ev) > 3:
            txt = "|".join(ev[4:])
            if "[from] move:" in txt or "[from] ability: Trace" in txt:
                k = mon_id(ev[2])
                if k in tf:
                    out[k] = to_id(ev[3])
        elif kind == "-activate" and len(ev) > 5 and ev[3] in ("move: Skill Swap", "Skill Swap"):
            src, of = mon_id(ev[2]), None
            for x in ev[6:]:
                if x.startswith("[of] "):
                    of = mon_id(x[5:])
            if src in tf:
                out[src] = to_id(ev[4])
            if of in tf:
                out[of] = to_id(ev[5])
        elif kind == "-activate" and len(ev) > 3 and ev[3] == "ability: Mummy":
            for x in ev[4:]:
                if x.startswith("[of] "):
                    k = mon_id(x[5:])
                    if k in tf:
                        out[k] = "mummy"
    return out


def log_forms(battle):
    """(2026-10-10) {id(poke-env mon): species name} from the log's '|detailschange|' lines (Mega Evolution): exact
    forms, also those poke-env's data lacks or confuses (Garchomp-Mega-Z was rebuilt as Garchomp-Mega)."""
    out, temp, slot_mon, base = {}, {}, {}, {}
    for ev in getattr(battle, "_replay_data", []):
        if len(ev) < 3:
            continue
        kind = ev[1]
        if len(ev) > 3 and kind == "detailschange":
            try:
                out[id(log_mon(battle, ev[2]))] = ev[3].split(",")[0].strip()
            except Exception:
                pass
        elif len(ev) > 3 and kind == "-formechange" and to_id(ev[3]).startswith(_TEMP_FORM_BASES):
            # (2026-10-18) temporary forms for the current stint (Hunger Switch, Stance Change, Gulp Missile): a Hangry
            # Morpeko was rebuilt Full Belly (Electric Aura Wheel, super effective instead of Dark)
            try:
                k_ = id(log_mon(battle, ev[2]))
                temp[k_] = ev[3].split(",")[0].strip()
                base[k_] = temp[k_].split("-")[0]
            except Exception:
                pass
        elif kind in ("switch", "drag", "faint"):
            sl = str(ev[2]).split(":")[0][:3]
            try:
                new = id(log_mon(battle, ev[2]))
            except Exception:
                new = None
            if kind == "faint":
                temp.pop(new, None)
            else:
                old = slot_mon.get(sl)
                if old is not None and old != new:
                    temp.pop(old, None)                 # the form ends when it leaves the field
                temp.pop(new, None)
                slot_mon[sl] = new
        elif kind == "swap":
            sd_ = str(ev[2])[:2]
            a_, b_ = slot_mon.get(sd_ + "a"), slot_mon.get(sd_ + "b")
            slot_mon[sd_ + "a"], slot_mon[sd_ + "b"] = b_, a_
    # (2026-10-18 bridge fuzz) a temporary form that ENDED (switch-out / faint) is the base form again: poke-env keeps the
    # '-formechange' species (an Aegislash back in Shield forme was rebuilt as Aegislash-Blade: Flamethrower 117 vs 52)
    for k_, b_ in base.items():
        out.setdefault(k_, b_)
    out.update(temp)
    return out


_TEMP_FORM_BASES = ("morpeko", "aegislash", "cramorant")


def set_current_abilities(eb, ours, theirs, index, their_index):
    """(2026-10-18 bridge fuzz) abilities changed by a move this stint (Entrainment, Skill Swap, Role Play, Worry Seed, Simple
    Beam, Trace, Mummy...): the team text holds the set's ability (it comes back at switch-out), the current one is set on
    top. A Mega Salamence given Sweet Veil by Entrainment was rebuilt with Aerilate. Transformed Pokemon: the bridge's
    Transform sets theirs. Old engine builds without set_ability: skipped."""
    if not hasattr(eb, "set_ability"):
        return
    st = eb.state()
    for side, mons, idx in ((0, ours, index), (1, theirs, their_index)):
        for m in mons:
            if getattr(m, "transformed", False) or id(m) not in idx:
                continue
            cur = getattr(m, "_temporary_ability", None)
            k = idx[id(m)]
            if not cur or to_id(cur) == st["sides"][side]["mons"][k].get("ability"):
                continue
            try:
                eb.set_ability(side, k, to_id(cur))
            except ValueError:
                pass


def engine_form(mon, forms, ability):
    """(species, ability) for the engine: the logged form if there is one; a Mega's ability is the Mega form's own
    (poke-env reports the base ability, or wrong Mega abilities: Golisopod-Mega had Emergency Exit, not Tough Claws)."""
    sp = forms.get(id(mon)) or current_species(mon)
    engine_species(sp)                                  # loads the engine's species list
    entry = _ENGINE_DEX_SPECIES().get(to_id(sp))
    if entry and str(entry.get("forme", "")).startswith("Mega"):
        ability = to_id(entry.get("abilities", {}).get("0", "")) or ability
    return (engine_species(entry["name"]) if entry else engine_species(sp)), ability


_ENGINE_DEX = None


def _ENGINE_DEX_SPECIES():
    global _ENGINE_DEX
    if _ENGINE_DEX is None:
        import json as _json
        import os
        path = paths.DEX
        try:
            _ENGINE_DEX = _json.load(open(path, encoding="utf-8"))["species"]
        except (OSError, KeyError, ValueError):
            _ENGINE_DEX = {}
    return _ENGINE_DEX


def current_species(mon):
    """poke-env keeps the old species name after Mega Evolution (only stats and types change):
    find the form that matches the current base stats. Cosmetic forms -> the base species (engine_species)."""
    return engine_species(_current_species(mon))


def _current_species(mon):
    dex = gen9().pokedex
    entry = dex.get(mon.species)
    if entry is None or entry.get("baseStats") == mon.base_stats:
        return mon.species
    base = to_id(entry.get("baseSpecies") or entry.get("name", ""))
    for sid, e in dex.items():
        if to_id(e.get("baseSpecies") or "") == base and e.get("baseStats") == mon.base_stats:
            return sid
    return mon.species


def is_mega_form(mon):
    """Has this Pokemon Mega Evolved? (By the form's data: a name check would also match Meganium.)"""
    entry = gen9().pokedex.get(current_species(mon), {})
    return str(entry.get("forme", "")).startswith("Mega")


def mega_used_in_log(battle, pid):
    """(2026-10-21, arena battle 4618) has player `pid` ('p1' / 'p2') Mega Evolved in this battle, by the log's '|-mega|'
    lines. is_mega_form alone missed it after an open team sheet reset the opponent's species to the base form: the
    rebuilt position let their Floette Mega Evolve after their Salamence already had."""
    for ev in getattr(battle, "_replay_data", []):
        if len(ev) > 2 and ev[1] == "-mega" and str(ev[2]).startswith(pid):
            return True
    return False


def mon_export(species, item, ability, moves, points, nature, gender=None):
    # (2026-10-18) gender from the battle (poke-env MALE / FEMALE); without it the engine drew one at random (Attract,
    # Rivalry, Cute Charm went wrong)
    g = str(getattr(gender, "name", gender or "")).upper()
    head = species + (" (M)" if g == "MALE" else " (F)" if g == "FEMALE" else "") + (f" @ {item}" if item else "")
    evs = " / ".join(f"{points[k]} {lab}" for k, lab in (("hp", "HP"), ("atk", "Atk"), ("def", "Def"),
                                                         ("spa", "SpA"), ("spd", "SpD"), ("spe", "Spe")) if points.get(k))
    lines = [head, f"Ability: {ability}", "Level: 50"]
    if evs:
        lines.append(f"EVs: {evs}")
    lines.append(f"{nature} Nature")
    lines += [f"- {m}" for m in moves]
    return "\n".join(lines)


def _bridge_err(where, e):
    """A swallowed bridge error: re-raised with BRIDGE_DEBUG, recorded in $BRIDGE_ERRLOG."""
    if os.environ.get("BRIDGE_DEBUG"):
        raise e
    if os.environ.get("BRIDGE_ERRLOG"):
        import traceback
        try:
            with open(os.environ["BRIDGE_ERRLOG"], "a", encoding="utf-8") as fh:
                fh.write(f"== {where}\n{traceback.format_exc()}\n")
        except OSError:
            pass


SPREAD_INFER = os.environ.get("SPREAD_INFER", "1") != "0"


def spread_update(bot, battle):
    """Update the battle's spread tracker with the log events since the last decision (once per decision)."""
    if not SPREAD_INFER or getattr(battle, "_spread_busy", False):
        return
    try:
        evs = getattr(battle, "_replay_data", None)
        if not evs:
            return
        tr = getattr(battle, "_spread_tracker", None)
        if tr is None:
            import spread_track
            tr = battle._spread_tracker = spread_track.Tracker()
        if tr.seen == len(evs) and tr.ctx is not None:
            return
        battle._spread_busy = True
        rng_keep = getattr(bot, "_set_rng", None)
        bot._set_rng = None                     # the tracker's own rebuild must not use up the search worlds' draws
        try:
            tr.update(bot, battle)
        finally:
            bot._set_rng = rng_keep
            battle._spread_busy = False
    except Exception as e:
        _bridge_err("spread_update", e)


def spread_choice(bot, battle, f, item, ability, item_known, ability_known, nature_known):
    """(points, nature, item, ability) for opponent f in this rebuild from its spread belief, or None (nothing observed
    yet / inference off). item / ability = what the rebuild would use otherwise: kept when still possible, required when
    revealed (item_known / ability_known); a sampled item keeps the same Mega-Stone-or-not as `item`."""
    if not SPREAD_INFER or getattr(battle, "_spread_busy", False):
        return None
    tr = getattr(battle, "_spread_tracker", None)
    b = tr.beliefs.get(id(f)) if tr is not None else None
    if b is None or b.n_obs == 0:
        return None
    try:
        import setlib
        it = to_id(item) if item else None
        ab = to_id(ability) if ability else None
        rng = getattr(bot, "_set_rng", None)
        stone = setlib.is_mega_stone(it) if it else False
        allowed = None if item_known else {x for x in b.items if setlib.is_mega_stone(x) == stone}
        if item_known and it not in b.items:
            return None                          # revealed item outside the belief's list: leave the rebuild alone
        out = b.draw(rng, item=it, ability=ab, items=allowed)
        if out is None:
            return None
        pts, nat, it2, ab2 = out
        if item_known:
            it2 = it
        elif setlib.is_mega_stone(it2) != stone:
            it2 = it                              # never change Mega-or-not (the search's candidates assume it)
        if ability_known or not ab2:
            ab2 = ab or ab2
        if nature_known:
            nat = nature_known
        tr.stats["applied"] = tr.stats.get("applied", 0) + 1
        return dict(pts), nat, it2 or "", ab2 or ability
    except Exception as e:
        _bridge_err("spread_choice", e)
        return None

class Bridge:
    """Rebuilds engine positions from poke-env battles. LIBRARY_FILL: unrevealed opponent moves / items / abilities
    from the real-set library (the search simulates with them) instead of generic filler moves."""
    LIBRARY_FILL = True

    def __init__(self):
        self._memo = None
        self._set_rng = None

    def memo(self, key, compute):
        """Remember a value for the rest of this decision (the state can't change within one)."""
        memo = getattr(self, "_memo", None)
        if memo is None:
            return compute()
        if key not in memo:
            memo[key] = compute()
        return memo[key]

    def brought(self, battle):
        """Our Pokemon in this battle. poke-env keeps all 6 of the team in battle.team (including the
        2 left at home); Showdown's latest request lists exactly the brought ones."""
        req = getattr(battle, "_last_request", None) or {}
        mons = []
        for p in req.get("side", {}).get("pokemon", []):
            m = battle.team.get(p.get("ident"))
            if m is not None and all(m is not x for x in mons):
                mons.append(m)
        return mons if mons else list(battle.team.values())

    def _engine_teams(self, battle):
        self._inferred = set()        # their_texts indices whose stats came from spread inference (build_engine)
        """(2026-10-19, split out of build_engine) both sides' brought Pokemon as engine team
        texts (our exact sets; theirs from what was revealed + library fill), with the slot maps."""
        spread_update(self, battle)
        dex_gen = gen9().pokedex
        ours = self.brought(battle)
        theirs_active = [f for f in battle.opponent_active_pokemon if f is not None]
        theirs = theirs_active + [f for f in battle.opponent_team.values() if all(f is not a for a in theirs_active)]
        # (2026-10-12) open team sheets: the sheet puts all SIX of their Pokemon (full sets) in opponent_team; only the
        # ones seen in battle are known to be brought, the rest fill up from the sheet (below), not from team preview
        ots = bool(getattr(battle, "_ots_sheet", False))
        sheet_rest = []
        if ots:
            seen_t = [f for f in theirs if f.revealed or f.active or f.fainted]
            sheet_rest = [f for f in theirs if all(f is not s for s in seen_t)]
            theirs = seen_t
        unseen = set()
        if getattr(self, "LIBRARY_FILL", False) and len(theirs) < 4:
            # (2026-10-10) the opponent brought four, but poke-env lists only the ones seen so far: without the rest the
            # search's opponent lost as soon as its SEEN Pokemon fainted (and could never switch to the others). Add the
            # missing ones from team preview (species not seen yet), a different random choice per search world
            # (self._set_rng), else a fixed choice per battle; the agent marks them unrevealed (blank to the network).
            seen = [to_id(f.species) for f in theirs]
            cands = sorted((p for p in (sheet_rest if ots else (getattr(battle, "teampreview_opponent_team", None) or []))
                            if not any(to_id(p.species).startswith(x) or x.startswith(to_id(p.species)) for x in seen)),
                           key=lambda p: p.species)
            need = min(4 - len(theirs), len(cands))
            if need > 0:
                rng = getattr(self, "_set_rng", None)
                spw = getattr(self, "_species_weight", None)   # (2026-10-20) agent: the belief head's species guess
                if rng is not None and spw is not None:
                    w = [spw(p.species) + 1e-9 for p in cands]
                    pick = [cands[i] for i in rng.choice(len(cands), need, replace=False, p=[x / sum(w) for x in w])]
                elif rng is not None:
                    pick = [cands[i] for i in rng.choice(len(cands), need, replace=False)]
                else:
                    pick = random.Random(battle.battle_tag).sample(cands, need)
                theirs = theirs + pick
                unseen = {id(p) for p in pick}
        def clean_item(it):
            return "" if not it or it == "unknown_item" else it
        def ability_of(m):
            # (2026-10-18) the SET's ability: one changed by a move this stint (Entrainment, Skill Swap, Trace...) is in
            # poke-env's temporary_ability and is set on top after the build (set_current_abilities)
            base = getattr(m, "_ability", None)
            entry = dex_gen.get(m.species, {})
            if getattr(m, "transformed", False):
                # (2026-10-19 bridge fuzz) a transformed Pokemon's OWN ability ('-transform ... [from] ability: Imposter'
                # stores it in _ability); the copied one is temporary. Falling back to the dex's first ability rebuilt an
                # Imposter Ditto as Limber: it didn't re-copy (and Intimidate) when it came back in.
                own = {to_id(a) for a in entry.get("abilities", {}).values()}
                if base and to_id(base) in own:
                    return base
                base = None
            if base:
                return base
            if m.ability and getattr(m, "_temporary_ability", None) is None:
                return m.ability
            return to_id(entry.get("abilities", {}).get("0", "")) or "noability"
        forms = log_forms(battle)
        texts, our_slots = [], []
        for m in ours:
            moves = [mid for mid in m.moves if mid not in ("struggle", "recharge")][:4]
            sp, ab = engine_form(m, forms, ability_of(m))
            fb = None
            if not getattr(m, "transformed", False):
                # (2026-10-18 bridge fuzz) always solve against the engine forme's own base stats: poke-env can keep a
                # temporary forme's base stats even when its species name is back to the base forme (Aegislash: 135 max HP)
                fb = (dex_gen.get(to_id(sp)) or {}).get("baseStats")
            pts, nature = infer_points(m, fb)
            texts.append(mon_export(sp, clean_item(m.item), ab, moves, pts, nature, getattr(m, "gender", None)))
            our_slots.append({mid: k for k, mid in enumerate(moves)})
        # (2026-11-03, our types) our two left at home, for Battle.set_roster: the search's "our hidden back" game
        self._our_home_texts = []
        try:
            for m in battle.team.values():
                if any(m is o for o in ours):
                    continue
                moves = [mid for mid in m.moves if mid not in ("struggle", "recharge")][:4]
                sp, ab = engine_form(m, {}, ability_of(m))
                pts, nature = infer_points(m, (dex_gen.get(to_id(sp)) or {}).get("baseStats"))
                self._our_home_texts.append(mon_export(sp, clean_item(m.item), ab, moves, pts, nature,
                                                       getattr(m, "gender", None)))
        except Exception as e:
            _bridge_err("home_texts", e)
            self._our_home_texts = []
        fillers = filler_moves()
        their_texts, their_slots, their_fill, their_protect = [], [], [], []
        # (2026-11-06, v3 belief input) per opponent Pokemon: where its set came from -> Battle.set_belief kind
        # (known pair set = exact; open / public sheet = set; library or pool fill = guess; generic fillers = none)
        self._bel_kind = []
        filled_items = {}                # species -> item filled in for an unrevealed one (library sets)
        for f in theirs:
            moves = [mid for mid in f.moves if mid not in ("struggle", "recharge")][:4]
            lib = None
            if getattr(self, "LIBRARY_FILL", False):
                # what hasn't been revealed comes from the best-matching real tournament set (setlib.py) instead of the
                # generic fillers / guessed spread: known moves stay first (the agent's revealed bits rely on that)
                import setlib
                known_item = None if f.item == "unknown_item" else (f.item or None)
                f_ability = ability_of(f) if getattr(f, "_ability", None) else None
                exc = set()
                lib = setlib.best_set(f.species, moves, known_item, f_ability, exclude_items=exc)
                rng = getattr(self, "_set_rng", None)
                if rng is not None and lib is not None:
                    # one search world: a set sampled among the consistent ones, with the same Mega-Stone-or-not as the
                    # best guess (the search's opponent candidates come from one position: Mega actions must stay legal)
                    sw = getattr(self, "_set_weight", None)      # (2026-10-20) agent: belief head score of a set
                    lib = setlib.best_set(f.species, moves, known_item, f_ability, rng=rng,
                                          mega_stone=setlib.is_mega_stone(known_item or lib["item"]),
                                          weight=(lambda s_, f_=f: sw(f_, s_)) if sw is not None else None,
                                          eps=BELIEF_EPS, exclude_items=exc) or lib
            if lib is not None:
                moves = moves + [mid for mid in lib["moves"] if mid not in moves][: 4 - len(moves)]
                item = (known_item or lib["item"]) if f.item == "unknown_item" else clean_item(f.item)
                if f.item == "unknown_item":
                    filled_items[f.species] = to_id(item)     # field_timers: a sampled Damp Rock etc. extends the weather
                pts = {k: lib["points"].get(k, 0) for k in ("hp", "atk", "def", "spa", "spd", "spe")}
                sp, ab = engine_form(f, forms, (ability_of(f) if getattr(f, "_ability", None) else None)
                                     or (f_ability if getattr(self, "LIBRARY_FILL", False) else None)
                                     or lib["ability"] or ability_of(f))
                # open team sheets also give the nature (not the stat points)
                nat = (getattr(f, "_nature", None) or "").capitalize() if ots else ""
                nature = nat or lib["nature"]
                inf = spread_choice(self, battle, f, item, ab, f.item != "unknown_item",
                                    bool(getattr(f, "_ability", None)), nat or None)
                if inf is not None:
                    pts, nature, item, ab = inf
                    self._inferred.add(len(their_texts))
                their_texts.append(mon_export(sp, item, ab, moves, pts, nature, getattr(f, "gender", None)))
                self._bel_kind.append("set" if ots else "guess")
                their_slots.append({mid: k for k, mid in enumerate(moves)})
                their_fill.append({})
                their_protect.append(next((k for k, mid in enumerate(moves) if mid in PROTECT_MOVES), None))
                continue
            if len(moves) < 4 and not any(mid in PROTECT_MOVES for mid in moves):
                moves.append("protect")
            sp, align = guess_spread(f.base_stats)
            off = "atk" if f.base_stats["atk"] >= f.base_stats["spa"] else "spa"
            cat = "PHYSICAL" if off == "atk" else "SPECIAL"
            fill = {}
            for t in f.types:
                if t is None or len(moves) >= 4:
                    continue
                other = "SPECIAL" if cat == "PHYSICAL" else "PHYSICAL"
                mid = fillers.get((t.name, cat)) or fillers.get((t.name, other))
                if mid and mid not in moves:
                    fill[t.name] = len(moves)
                    moves.append(mid)
            plus = next((k for k, v in align.items() if v > 1), None)
            minus = next((k for k, v in align.items() if v < 1), None)
            nature = NATURES.get((plus, minus), "Serious")
            fsp, fab = engine_form(f, forms, ability_of(f))
            fitem = clean_item(f.item)
            inf = spread_choice(self, battle, f, fitem or None, fab, f.item != "unknown_item", True, None)
            if inf is not None:
                sp, nature, fitem, fab = inf
                self._inferred.add(len(their_texts))
            their_texts.append(mon_export(fsp, fitem, fab, moves, sp, nature, getattr(f, "gender", None)))
            self._bel_kind.append("set" if ots else "none")
            their_slots.append({mid: k for k, mid in enumerate(moves)})
            their_fill.append(fill)
            their_protect.append(next((k for k, mid in enumerate(moves) if mid in PROTECT_MOVES), None))
        # (2026-11-06, v3) open sheet: the sheet Pokemon not placed among the four -> Battle.set_roster(1, ...), so v3's
        # open-gated view lists every sheet Pokemon not seen yet (rows 2-7), as in training
        self._their_roster_texts = []
        if ots:
            try:
                import setlib
                for p_ in sheet_rest:
                    if any(p_ is t_ for t_ in theirs):
                        continue
                    mv = [mid for mid in p_.moves if mid not in ("struggle", "recharge")][:4]
                    it = clean_item(p_.item) if p_.item not in (None, "unknown_item") else ""
                    ab_ = ability_of(p_)
                    lb = setlib.best_set(p_.species, mv, it or None, ab_ or None)
                    pts_ = {k: (lb["points"].get(k, 0) if lb else 0) for k in ("hp", "atk", "def", "spa", "spd", "spe")}
                    nat_ = (getattr(p_, "_nature", None) or "").capitalize() or (lb["nature"] if lb else "Serious")
                    self._their_roster_texts.append(mon_export(engine_species(p_.species), it, ab_, mv, pts_, nat_,
                                                               getattr(p_, "gender", None)))
            except Exception as e:
                _bridge_err("their_roster", e)
                self._their_roster_texts = []
        return (ours, theirs, unseen, texts, our_slots, their_texts, their_slots, their_fill, their_protect,
                filled_items)

    def build_engine(self, battle):
        """
        The current position as a vgcsim battle: our side = 0, theirs = 1. Returns
        (engine battle, our engine index by id(pokemon), our move slots, their move slots by foe slot,
        their filler slots by foe slot, their Protect slot by foe slot).
        """
        try:
            import bridge_vols as _bv
            _bv.SUB_RANGE.clear()                 # (2026-10-19) Substitute HP ranges belong to this rebuild only
        except Exception:
            pass
        (ours, theirs, unseen, texts, our_slots, their_texts, their_slots, their_fill, their_protect,
         filled_items) = self._engine_teams(battle)
        from encoding import _fnv_u
        eb = _vgcsim.Battle.from_brought(engine_dex(), "\n\n".join(texts), "\n\n".join(their_texts),
                                         seed=int(_fnv_u(turn_state_key(battle)) * ((1 << 30) - 1)) + 1,
                                         turn=max(1, battle.turn))
        # (2026-10-21, arena battle 4618) the search must keep the opponent stats built here (open sheet, spread
        # inference, library fills): without this the engine replaced them with guessed / template spreads in every search
        # world, and a Timid Mega Salamence + Modest Floette that KO'd both our Pokemon were searched as ones that couldn't
        if hasattr(eb, "set_keep_foe_stats"):
            eb.set_keep_foe_stats(True)
        if hasattr(eb, "set_roster") and len(getattr(self, "_our_home_texts", [])) == 2:
            try:
                eb.set_roster(0, "\n\n".join(self._our_home_texts))
            except Exception as e:
                _bridge_err("set_roster", e)
        if hasattr(eb, "set_belief"):
            # (2026-11-06, v3 external belief input) what we believe about each opponent Pokemon = the set this build used
            try:
                smons = eb.state()["sides"][1]["mons"]
                for k_, kind_ in enumerate(getattr(self, "_bel_kind", [])[:len(smons)]):
                    if kind_ == "none":
                        continue
                    m_ = smons[k_]
                    st_ = m_["stats"]
                    eb.set_belief(1, k_, kind_, ability=m_.get("base_ability") or m_.get("ability") or None,
                                  item=m_.get("item") or None, moves=[mv_ for mv_, _ in m_["moves"]],
                                  stats=[int(st_[x]) for x in ("hp", "atk", "def", "spa", "spd", "spe")])
            except Exception as e:
                _bridge_err("set_belief", e)
        if hasattr(eb, "set_roster") and getattr(self, "_their_roster_texts", None):
            try:
                eb.set_roster(1, "\n\n".join(self._their_roster_texts))
            except Exception as e:
                _bridge_err("set_roster_theirs", e)
        if hasattr(eb, "set_preview"):
            # (2026-11-06, v3 net) the opponent's six from team preview (or their open sheet): v3's closed view lists the
            # ones not seen yet as roster candidates (rows 2-7); without this those rows were empty
            try:
                pv = list(getattr(battle, "teampreview_opponent_team", None) or [])
                if bool(getattr(battle, "_ots_sheet", False)) or not pv:
                    pv = list(battle.opponent_team.values())
                pv_names = [engine_species(p.species) for p in pv][:6]
                if pv_names:
                    eb.set_preview(1, pv_names)
            except Exception as e:
                _bridge_err("set_preview", e)
        index = {id(m): k for k, m in enumerate(ours)}
        their_index = {id(f): k for k, f in enumerate(theirs)}
        set_current_abilities(eb, ours, theirs, index, their_index)
        # positions
        eng_maxhp = [m["maxhp"] for m in eb.state()["sides"][1]["mons"]]
        moved = moved_since_switch(battle) if getattr(battle, "_replay_data", None) else None
        sprog = status_progress(battle) if getattr(battle, "_replay_data", None) else {}
        eng_abil = [[mm.get("ability") for mm in sd["mons"]] for sd in eb.state()["sides"]]
        try:
            hpcol = hp_colours(battle)
        except Exception:
            hpcol = {}
        for side, mons, active in ((0, ours, battle.active_pokemon), (1, theirs, battle.opponent_active_pokemon)):
            idx_map = index if side == 0 else their_index
            for k in range(2):
                m = active[k] if k < len(active) else None
                eb.set_active(side, k, idx_map.get(id(m)) if m is not None and not m.fainted else None)
            for k, m in enumerate(mons):
                if side == 1 and id(m) in unseen:
                    continue                                  # not seen yet: full HP, no status (the engine's defaults)
                if m.fainted:
                    eb.set_mon(side, k, fainted=True)
                    continue
                # theirs: the HP FRACTION times the engine's own max HP (the set it was built with, e.g. a library
                # spread); an absolute HP from the guessed spread made full-HP foes ~85% (bridge audit, 2026-10-10)
                hp = m.current_hp if side == 0 else hp_from_percent(m.current_hp_fraction, eng_maxhp[k], hpcol.get(id(m), ""))
                status = m.status.name.lower() if m.status is not None else ""
                if status == "fnt":
                    status = ""
                boosts = {k2: v for k2, v in m.boosts.items() if v}
                ft = (not moved.get(id(m), False)) if moved is not None else bool(getattr(m, "first_turn", False))
                extra = {}
                sp_ = sprog.get(id(m))
                if sp_ is not None and sp_[0] == status:
                    unit = 2 if to_id(eng_abil[side][k] or "") == "earlybird" else 1
                    if status == "slp":
                        if sp_[2]:
                            extra["status_turns"] = max(1, 3 - unit * sp_[1])
                        else:
                            extra["sleep_elapsed"] = unit * sp_[1]
                    elif status == "frz":
                        extra["status_turns"] = max(1, 3 - sp_[1])
                    elif status == "tox":
                        extra["status_turns"] = min(15, sp_[3])
                kw = dict(hp=hp, status=status, boosts=boosts, first_turn=ft or not getattr(m, "active", False),
                          protect_streak=min(getattr(m, "protect_counter", 0) or 0, 6))
                try:
                    eb.set_mon(side, k, **kw, **extra)
                except TypeError:                             # an engine build without status_turns / sleep_elapsed
                    eb.set_mon(side, k, **kw)
        # (2026-10-19 forced behaviour check) Pokemon that switched in during the current turn: activeTurns 0 (Speed Boost)
        try:
            fresh = switched_this_turn(battle) if getattr(battle, "_replay_data", None) else set()
            for side, mons in ((0, ours), (1, theirs)):
                for k, m in enumerate(mons):
                    if id(m) in fresh and getattr(m, "active", False) and not m.fainted:
                        eb.set_vols(side, k, {"active_turns": 0})
        except Exception:
            pass
        # (2026-10-17) PP spent so far (poke-env counts uses, Pressure included), per engine move slot; the rebuilt
        # position used to start every move at full PP. Old engine builds lack pp_used: skipped.
        try:
            est = eb.state()["sides"]
            # (2026-10-18 bridge fuzzer) our ACTIVE Pokemon's PP is exact in Showdown's request; poke-env's count of uses
            # misses Pressure and charges moves called by Sleep Talk / Copycat
            req = getattr(battle, "_last_request", None) or {}
            req_pp = {}
            req_act = [p for p in (req.get("side") or {}).get("pokemon", []) if p.get("active")]
            for j, p in enumerate(req_act[:len(req.get("active") or [])]):
                mon = battle.team.get(p.get("ident"))
                if mon is None:
                    continue
                for e in (req["active"][j] or {}).get("moves") or []:
                    if "pp" in e and "maxpp" in e:
                        req_pp.setdefault(id(mon), {})[to_id(e.get("id") or e.get("move"))] = int(e["maxpp"]) - int(e["pp"])
            for side, mons in ((0, ours), (1, theirs)):
                for k, m in enumerate(mons):
                    if m.fainted or k >= len(est[side]["mons"]):
                        continue
                    pe = {to_id(mid): mv for mid, mv in m.moves.items()}
                    rp = req_pp.get(id(m), {}) if side == 0 else {}
                    used = []
                    for mid, _pp in est[side]["mons"][k]["moves"]:
                        if to_id(mid) in rp:
                            used.append(max(0, rp[to_id(mid)]))
                            continue
                        mv = pe.get(to_id(mid))
                        try:
                            used.append(max(0, int(mv.max_pp) - int(mv.current_pp)) if mv is not None else 0)
                        except Exception:
                            used.append(0)
                    if any(used):
                        eb.set_mon(side, k, pp_used=used)
        except TypeError:
            pass
        # (2026-10-14) Unburden: the item was lost since it last came in (the speed doubling is a volatile in the engine)
        if getattr(battle, "_replay_data", None):
            lost = lost_item_since_switch(battle)
            try:
                for side, mons in ((0, ours), (1, theirs)):
                    for k, m in enumerate(mons):
                        if lost.get(id(m)) and not m.item:     # the engine boosts only an Unburden holder
                            eb.set_mon(side, k, unburden=True)
            except TypeError:
                pass                                       # older vgcsim module without set_mon(unburden=)
            # (2026-10-14) Choice lock: the net reads it and the search must not give a locked Pokemon other moves
            locks = choice_lock_moves(battle)
            try:
                for side, mons in ((0, ours), (1, theirs)):
                    for k, m in enumerate(mons):
                        if getattr(m, "active", False) and locks.get(id(m)):
                            eb.set_mon(side, k, choice_lock=locks[id(m)])
            except TypeError:
                pass                                       # older vgcsim module without set_mon(choice_lock=)
        # (2026-10-10) history inputs for nets that read them (what each Pokemon was seen doing), from the battle log
        if getattr(battle, "_replay_data", None) and HAS_HISTORY():
            hist = log_history(battle)
            for side, mons, slots in ((0, ours, our_slots), (1, theirs, their_slots)):
                for k, m in enumerate(mons):
                    if side == 1 and id(m) in unseen:
                        continue
                    eb.set_mon(side, k, history=history_args(hist.get(id(m)), slots[k]))
        # field and sides
        turn = battle.turn
        w = names(battle.weather)
        weather = next((x for x, n in (("rain", "RAINDANCE"), ("sun", "SUNNYDAY"), ("sand", "SANDSTORM"),
                                       ("snow", "SNOWSCAPE"), ("snow", "SNOW"), ("snow", "HAIL")) if n in w), "")
        def left(table, key, total):
            for k, start in table.items():
                if getattr(k, "name", str(k)) == key:
                    return max(1, total - (turn - (start or turn)))
            return 0
        weather_start = next(iter(battle.weather.values()), turn) if battle.weather else turn
        terrain = next((x for x, n in (("electric", "ELECTRIC_TERRAIN"), ("grassy", "GRASSY_TERRAIN"),
                                       ("psychic", "PSYCHIC_TERRAIN"), ("misty", "MISTY_TERRAIN")) if n in names(battle.fields)), "")
        terrain_left = max([left(battle.fields, n, 5) for n in ("ELECTRIC_TERRAIN", "GRASSY_TERRAIN", "PSYCHIC_TERRAIN", "MISTY_TERRAIN")] + [0])
        timers = field_timers(battle, filled_items) if getattr(battle, "_replay_data", None) else None
        if timers is not None:
            # turns left counted from the log (field_timers); the poke-env estimates below were wrong ~40% of the time
            eb.set_field(weather=weather, weather_turns=timers.get("weather", 5) if weather else 0,
                         terrain=terrain, terrain_turns=timers.get("terrain", 5) if terrain else 0,
                         trick_room=timers.get("trickroom", 5) if "TRICK_ROOM" in names(battle.fields) else 0)
        else:
            eb.set_field(weather=weather, weather_turns=max(1, 5 - (turn - (weather_start or turn))) if weather else 0,
                         terrain=terrain, terrain_turns=terrain_left or 0, trick_room=left(battle.fields, "TRICK_ROOM", 5))
        me = getattr(battle, "player_role", None) or "p1"
        for side, conds, mons in ((0, battle.side_conditions, ours), (1, battle.opponent_side_conditions, theirs)):
            if timers is not None:
                pid = me if side == 0 else ("p2" if me == "p1" else "p1")
                tl = {k: (timers.get((pid, cid), d) if n in names(conds) else 0) for k, cid, n, d in (
                    ("tailwind", "tailwind", "TAILWIND", 4), ("reflect", "reflect", "REFLECT", 5),
                    ("light_screen", "lightscreen", "LIGHT_SCREEN", 5), ("aurora_veil", "auroraveil", "AURORA_VEIL", 5))}
            else:
                tl = dict(tailwind=left(conds, "TAILWIND", 4), reflect=left(conds, "REFLECT", 5),
                          light_screen=left(conds, "LIGHT_SCREEN", 5), aurora_veil=left(conds, "AURORA_VEIL", 5))
            pid_ = me if side == 0 else ("p2" if me == "p1" else "p1")
            eb.set_side(side, mega_used=any(is_mega_form(m) for m in mons) or mega_used_in_log(battle, pid_), **tl)
            # (2026-10-11) entry hazards: poke-env counts layers for Spikes / Toxic Spikes; rebuilt positions had none (a
            # replacement into Toxic Spikes stayed healthy in the search). Old engine builds lack these arguments: skipped.
            hz = {n: v for n, v in ((str(getattr(c, "name", c)).upper(), v) for c, v in conds.items())}
            if any(k in hz for k in ("STEALTH_ROCK", "SPIKES", "TOXIC_SPIKES", "STICKY_WEB")):
                try:
                    eb.set_side(side, stealth_rock="STEALTH_ROCK" in hz, spikes=int(hz.get("SPIKES", 0) or 0),
                                toxic_spikes=int(hz.get("TOXIC_SPIKES", 0) or 0), sticky_web="STICKY_WEB" in hz)
                except TypeError:
                    pass
        # (2026-10-18) volatiles (Taunt, Encore, confusion, Substitute, charging moves, Leech Seed...), slot conditions (Wish,
        # Future Sight) and Safeguard / Gravity / Magic Room: from the log (volatiles_from_log: what a player can see)
        if hasattr(eb, "set_vols"):
            import bridge_vols
            if getattr(battle, "_replay_data", None) and os.environ.get("BRIDGE_LOG_VOLS", "1") == "1":
                import log_vols
                try:
                    bridge_vols.apply(eb, battle.player_role, *log_vols.from_log(battle))
                except Exception:
                    if os.environ.get("BRIDGE_DEBUG"):
                        raise
                    if os.environ.get("BRIDGE_ERRLOG"):    # (2026-10-19) record swallowed tracker / apply errors
                        import traceback
                        try:
                            with open(os.environ["BRIDGE_ERRLOG"], "a", encoding="utf-8") as fh:
                                fh.write(f"== {battle.battle_tag} turn {battle.turn}\n{traceback.format_exc()}\n")
                        except OSError:
                            pass
        if hasattr(eb, "set_ability") and getattr(battle, "_replay_data", None):
            # (2026-10-19) abilities changed after a Transform (the Transform above copied the target's)
            for k_, ab_ in post_transform_abilities(battle).items():
                for side_, idx_ in ((0, index), (1, their_index)):
                    if k_ in idx_ and ab_:
                        try:
                            eb.set_ability(side_, idx_[k_], ab_)
                        except ValueError:
                            pass
        eb.refresh()
        if SPREAD_TO_NET and self._inferred and hasattr(eb, "set_stat_belief"):
            # (2026-10-20) the inferred stats also feed the network's damage features (for nets trained with
            # the stat-emulation setting); otherwise those features use the generic stat guess as in training before
            st_ = eb.state()["sides"][1]["mons"]
            for k in self._inferred:
                if k < len(st_):
                    s_ = st_[k]["stats"]
                    eb.set_stat_belief(1, k, [st_[k]["maxhp"], s_["atk"], s_["def"], s_["spa"], s_["spd"], s_["spe"]])
        return eb, index, our_slots, their_index, theirs, their_slots, their_fill, their_protect
