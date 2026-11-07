"""(2026-10-17) Illusion (Zoroark / Zoroark-Hisui) for poke-env battles.

Showdown's log names a disguised Zoroark after the Pokemon it imitates ("|switch|p2a: Froslass|Froslass, L50, F|100/100"),
so poke-env books everything it does (moves, boosts, item, HP, Protect count, faint) on the imitated Pokemon, and after
the reveal ("|replace|") the imitated one keeps the Zoroark's moves and stays "revealed" even if it was never brought.
Our bridge (bridge.build_engine and the log helpers that read battle._replay_data) trusted those records, so the rebuilt
positions held e.g. a Froslass the opponent didn't bring and no Zoroark (bridge audit, 2026-10-17).

install() wraps AbstractBattle.parse_message (every path goes through it: poke-env players,
review/replay.py):
- OUR side (we know the truth): a switch-in whose name doesn't fit (the named Pokemon is fainted, active in the other
  slot, not brought, or its max HP differs from the line's exact max HP while our Illusion user's matches) is our
  Illusion user: the event and every later event naming the disguise in that slot are rewritten to its real name, until
  it leaves, is revealed or faints.
- THEIR side (known only after the fact): each switch-in remembers the named Pokemon's state before it came in. At
  '|replace|' everything since that switch-in moves to the Zoroark (moves first seen then, boosts, effects, Protect
  count, an item shown then), the imitated Pokemon gets its earlier state back (or is dropped if it had never been seen),
  and the log since the switch-in is rewritten to the Zoroark's name. A disguised Zoroark that FAINTS is never revealed
  by Showdown (onFaint clears the illusion silently): when the "fainted" Pokemon later switches in, that faint is
  given to their Zoroark the same way.
While the disguise holds, the opponent's Zoroark stays hidden (that is the real information state).
"""

import re
from poke_env.battle.abstract_battle import AbstractBattle

ILLUSION_SPECIES = ("zoroark", "zoroarkhisui", "zorua", "zoruahisui")


def _tid(s):
    return re.sub(r"[^a-z0-9]", "", str(s).lower())


def _maxhp(hp_status):
    m = re.match(r"\s*\d+/(\d+)", hp_status or "")
    return int(m.group(1)) if m else None


def _state(b):
    st = b.__dict__.get("_illusion_fix")
    if st is None:
        st = b.__dict__["_illusion_fix"] = {"ours": {}, "theirs": {}, "closed": {}, "revealed": 0}
    return st


def _mv(mon):
    """The Pokemon's move dict (poke-env 0.16 keeps it in a MoveSet: .base_moves is the real, mutable dict; a plain dict()
    of the MoveSet raises, which the never-fail wrapper swallowed, so the opponent side was silently untracked)."""
    m = mon._moves
    return m.base_moves if hasattr(m, "base_moves") else m


def _snap(mon):
    if mon is None:
        return None
    return {"moves": dict(_mv(mon)), "revealed": mon._revealed, "item": mon._item, "ability": mon._ability,
            "status": mon._status, "hp": mon._current_hp, "maxhp": mon._max_hp}


def _rewrite(ev, old, new):
    """Replace the ident `old` ('p2a: Froslass') with `new` in every field of one event (also '[of] p2a: Froslass')."""
    out = list(ev)
    for i in range(2, len(out)):
        f = out[i]
        if f == old:
            out[i] = new
        elif f.endswith(" " + old):
            out[i] = f[: -len(old)] + new
    return out


def _brought_idents(b):
    req = getattr(b, "_last_request", None) or {}
    return [p.get("ident") for p in req.get("side", {}).get("pokemon", []) if p.get("ident")]


def _req_details(b, team_key):
    req = getattr(b, "_last_request", None) or {}
    for p in req.get("side", {}).get("pokemon", []):
        if p.get("ident") == team_key:
            return p.get("details")
    return None


def _ours_switch(b, st, ev):
    """Our switch-in: is it our Illusion user under another name? Returns the (possibly rewritten) event."""
    ident, details, hp = ev[2], ev[3], ev[4] if len(ev) > 4 else ""
    slot, name = ident[:3], ident.split(": ", 1)[1]
    key = ident[:2] + ": " + name
    named = b._team.get(key)
    brought = _brought_idents(b)
    other = b._active_pokemon.get(slot[:2] + ("b" if slot[2] == "a" else "a"))
    mx = _maxhp(hp)
    cands = [(k, m) for k, m in b._team.items()
             if k != key and m._ability == "illusion" and not m.fainted and m is not other and (not brought or k in brought)]
    if not cands:
        return ev
    definite = named is None or named.fainted or (named is other) or (brought and key not in brought)
    pick = None
    if definite and len(cands) == 1:
        pick = cands[0]
    elif mx is not None and named is not None and named._max_hp and mx != named._max_hp:
        fit = [(k, m) for k, m in cands if m._max_hp == mx]
        if len(fit) == 1:
            pick = fit[0]
    if pick is None:
        return ev
    k, m = pick
    true_ident = slot + ": " + k.split(": ", 1)[1]
    st["ours"][slot] = (ident, true_ident)
    out = list(ev)
    out[2] = true_ident
    out[3] = _req_details(b, k) or details
    return out


HP_KINDS = ("-damage", "-heal", "-sethp", "switch", "drag")


def _period_events(b, period, ident, from_switch=False):
    """(moves used, boost changes, last 'HP status' string) of `ident` in the replay since the period began
    (from_switch: boosts counted from `ident`'s own last switch-in, which may be earlier)."""
    moves, boosts, hp = set(), {}, None
    rd = b._replay_data
    bstart = period["start"]
    if from_switch:
        bstart = max((i for i, e in enumerate(rd) if len(e) > 2 and e[1] in ("switch", "drag") and e[2] == ident), default=0)
    for i, ev in enumerate(rd[min(bstart, period["start"]):], start=min(bstart, period["start"])):
        if len(ev) < 4 or ev[2] != ident:
            continue
        if i < period["start"] and ev[1] not in ("-boost", "-unboost"):
            continue
        if i < bstart and ev[1] in ("-boost", "-unboost"):
            continue
        if len(ev) < 4 or ev[2] != ident:
            continue
        if ev[1] == "move":
            moves.add(_tid(ev[3]))
        elif ev[1] in ("-boost", "-unboost") and len(ev) > 4:
            try:
                boosts[ev[3]] = boosts.get(ev[3], 0) + int(ev[4]) * (1 if ev[1] == "-boost" else -1)
            except ValueError:
                pass
        elif ev[1] in HP_KINDS:
            hp = ev[4] if ev[1] in ("switch", "drag") and len(ev) > 4 else ev[3]
    return moves, boosts, hp


_BOOST_KEYS = ("accuracy", "atk", "def", "evasion", "spa", "spd", "spe")


def _full_boosts(d):
    """poke-env indexes _boosts by every stat: a partial dict makes later -boost / -unboost lines raise KeyError."""
    return {k: max(-6, min(6, int(d.get(k, 0)))) for k in _BOOST_KEYS}


def _their_reveal(b, st, slot, z, illusioned, pre, period, details, twin=None):
    """Move what happened since `period` began from `illusioned` to the Zoroark `z`; restore `illusioned`.
    `twin` = the partner slot's ident when the imitated Pokemon itself stands next to the Zoroark (both shown as the
    same Pokemon, so poke-env merged them into one object): split moves / boosts / HP by slot instead."""
    snap = period["snap"]
    old_moves = set(snap["moves"]) if snap else set()
    z_moves, z_boosts, z_hp = _period_events(b, period, period["ident"])
    if twin:
        t_moves, t_boosts, t_hp = _period_events(b, period, twin, from_switch=True)
        for mid, mv in list(_mv(illusioned).items()):
            if mid not in old_moves:
                if mid in z_moves:
                    _mv(z).setdefault(mid, mv)
                if mid not in t_moves:
                    del _mv(illusioned)[mid]
        z._boosts = _full_boosts(z_boosts)
        z._effects = dict(pre["effects"]) if pre is not None else {}
        illusioned._boosts = _full_boosts(t_boosts)
        if z_hp:
            try:
                z.set_hp_status(z_hp)
            except Exception:
                pass
        if t_hp:
            try:
                illusioned.set_hp_status(t_hp)
            except Exception:
                pass
        elif snap is not None:
            illusioned._status, illusioned._current_hp, illusioned._max_hp = snap["status"], snap["hp"], snap["maxhp"]
        if snap is not None:
            illusioned._ability = snap["ability"]
        illusioned._active = True
        pre = None
        snap_restore = False
    else:
        snap_restore = True
        for mid, mv in list(_mv(illusioned).items()):
            if mid not in old_moves:
                _mv(z).setdefault(mid, mv)
                del _mv(illusioned)[mid]
        if z_hp:
            try:
                z.set_hp_status(z_hp)                          # the Zoroark's HP = what the disguise showed
            except Exception:
                pass
    if pre is not None:
        z._boosts = _full_boosts(pre["boosts"])
        z._effects = dict(pre["effects"])
        z._protect_counter = pre["protect"]
        if snap is not None and illusioned._item != snap["item"]:
            z._item = illusioned._item
            illusioned._item = snap["item"]
    if not snap_restore:
        pass
    elif snap is None:
        # never seen before the disguise: it isn't known to be in this battle at all
        for k in [k for k, v in b._opponent_team.items() if v is illusioned]:
            del b._opponent_team[k]
    else:
        illusioned._revealed = snap["revealed"]
        illusioned._status = snap["status"]
        illusioned._current_hp = snap["hp"]
        illusioned._max_hp = snap["maxhp"]
        illusioned._ability = snap["ability"]
    old_ident = period["ident"]
    new_ident = slot + ": " + z._name if z._name else None
    if new_ident:
        rd = b._replay_data
        for i in range(period["start"], len(rd)):
            ev = rd[i]
            if len(ev) > 2 and ev[1] in ("switch", "drag") and ev[2] == old_ident:
                ev = list(ev)
                ev[2] = new_ident
                if details:
                    ev[3] = details
                rd[i] = ev
            else:
                rd[i] = _rewrite(ev, old_ident, new_ident)
    st["revealed"] += 1


def _swap_letters(ident):
    return ident[:2] + {"a": "b", "b": "a"}.get(ident[2], ident[2]) + ident[3:] if len(ident) > 3 else ident


def _pre(b, ev):
    """Rewrite one event before poke-env sees it; returns (event, post-step or None)."""
    if len(ev) < 3 or not b.player_role:
        return ev, None
    st = _state(b)
    kind, ident = ev[1], ev[2]
    side = ident[:2]
    slot = ident[:3] if len(ident) > 3 and ident[2] in "abc" else None
    if kind == "swap" and slot:
        # Ally Switch: the two slots of that side trade places (open disguises / periods move with them)
        tab = st["ours"] if side == b.player_role else st["theirs"]
        moved = {}
        for s, v in list(tab.items()):
            if s[:2] == side:
                del tab[s]
                if isinstance(v, tuple):
                    moved[_swap_letters(s)] = (_swap_letters(v[0]), _swap_letters(v[1]))
                else:
                    v = dict(v, ident=_swap_letters(v["ident"]))
                    moved[_swap_letters(s)] = v
        # the swap event itself names the Pokemon BEFORE the swap
        if side == b.player_role:
            for s, (old, new) in list(moved.items()):
                ev = _rewrite(ev, _swap_letters(old), _swap_letters(new))
        tab.update(moved)
        return ev, None
    if side == b.player_role:
        for s, (old, new) in list(st["ours"].items()):
            if kind in ("switch", "drag") and slot == s:
                del st["ours"][s]
                continue
            ev = _rewrite(ev, old, new)
        if kind in ("switch", "drag") and slot:
            ev = _ours_switch(b, st, ev)
        if kind in ("replace", "faint") and slot in st["ours"]:
            del st["ours"][slot]
        return ev, None
    # (2026-10-19) an opponent's event that names OUR disguised Pokemon (a move's target, '[of] p1a: Tauros' after Speed
    # Swap / Trick...): we know who it really is
    for s, (old, new) in list(st["ours"].items()):
        ev = _rewrite(ev, old, new)
    # (2026-10-19) their slot deduced to hold the disguised Zoroark (see below): name it so until it leaves
    ded = st.setdefault("deduced", {})
    for s, (old, new) in list(ded.items()):
        if kind in ("switch", "drag", "replace") and slot == s:
            del ded[s]
            continue
        ev = _rewrite(ev, old, new)
        if kind == "faint" and slot == s:
            del ded[s]
    if kind in ("switch", "drag", "replace") and slot and len(ev) > 3:
        _rename_guess(b, st, side, ident.split(": ", 1)[1], ev[3])
    # opponent events: the disguise holds until revealed (that is the real information state)
    if kind in ("switch", "drag") and slot:
        name = ident.split(": ", 1)[1]
        key = side + ": " + name
        named = b._opponent_team.get(key)
        # (2026-10-19) X switches in while the other slot shows X: one of them is their Zoroark (species clause). Showdown's
        # Illusion (onBeforeSwitchIn, after the positions are swapped) imitates the LAST living party member at a later
        # position than the Zoroark's own: entering slot b (position 1) it never imitates the slot-a Pokemon, entering slot
        # a it imitates the slot-b one only when no living Pokemon is left on the bench. In those cases the one already on
        # the field is the Zoroark: reveal that slot now (its moves / boosts were being booked on the real X).
        other = slot[:2] + ("b" if slot[2] == "a" else "a")
        op = st["theirs"].get(other)
        fainted = sum(1 for e in b._replay_data if len(e) > 2 and e[1] == "faint" and str(e[2])[:2] == side)
        sure = slot[2] == "b" or (4 - 2 - fainted) > 0
        if (sure and op is not None and op["key"] == key and named is not None and not named.fainted
                and b._opponent_active_pokemon.get(other) is named):
            _deduce(b, st, other, named, op)
        period = st["theirs"].pop(slot, None)
        if period is not None:
            st["closed"][period["key"]] = period
        if named is not None and named.fainted:
            # it 'fainted' earlier yet comes in now: that faint was their disguised Zoroark's
            late = st["closed"].pop(key, None)
            z = _their_illusionist(b, side)
            if late is not None and z is not None and z is not named:
                _their_reveal(b, st, late["ident"][:3], z, named, {"boosts": {}, "effects": {}, "protect": 0}, late, None)
                z._current_hp = 0
                z._status = _fnt()
                z._active = False
                z._revealed = True
        st["theirs"][slot] = {"start": len(b._replay_data), "key": key, "ident": ident, "snap": _snap(named)}
        return ev, None
    if kind == "move" and slot and len(ev) > 3 and slot in st["theirs"] and not any(str(x).startswith("[from]") for x in ev[4:]):
        # (2026-10-19) a move the shown species can't learn (but their Illusion user can): the disguise is that Zoroark's
        op = st["theirs"][slot]
        named = b._opponent_active_pokemon.get(slot)
        if named is not None and _unlearnable(b, st, side, named, _tid(ev[3]), op):
            z = _deduce(b, st, slot, named, op)
            if z is not None:
                ev = _rewrite(ev, op["ident"], slot + ": " + z._name)
        return ev, None
    if kind == "replace" and slot:
        illusioned = b._opponent_active_pokemon.get(slot)
        period = st["theirs"].pop(slot, None)
        if illusioned is None or period is None:
            return ev, None
        pre = {"boosts": dict(illusioned._boosts), "effects": dict(illusioned._effects),
               "protect": illusioned._protect_counter}
        details = ev[3] if len(ev) > 3 else None

        def post():
            z = b._opponent_active_pokemon.get(slot)
            if z is not None and z is not illusioned:
                other = slot[:2] + ("b" if slot[2] == "a" else "a")
                twin = (other + ": " + period["ident"].split(": ", 1)[1])                     if b._opponent_active_pokemon.get(other) is illusioned else None   # (the merged object may show the
                #                                                     Zoroark's KO: not a reason to doubt the twin)
                _their_reveal(b, st, slot, z, illusioned, pre, period, details, twin)
                z._revealed = True
        return ev, post
    if kind == "faint" and slot and slot in st["theirs"]:
        def post():
            p = st["theirs"].pop(slot, None)
            if p is not None:
                st["closed"][p["key"]] = p
        return ev, post
    return ev, None


def install():
    if getattr(AbstractBattle, "_illusion_fix_installed", False):
        return
    orig = AbstractBattle.parse_message

    def parse_message(self, split_message):
        try:
            ev, post = _pre(self, split_message)
        except Exception:
            ev, post = split_message, None        # never break battle parsing over this
        r = orig(self, ev)
        if post is not None:
            try:
                post()
            except Exception:
                pass
        return r

    AbstractBattle.parse_message = parse_message
    AbstractBattle._illusion_fix_installed = True


def _fnt():
    from poke_env.battle.status import Status
    return Status.FNT


def _deduce(b, st, slot, named, op):
    """(2026-10-19) The open disguise period `op` in their `slot` is their Illusion user: reveal it (as Showdown's
    '|replace|' would) and name that slot's later events after it until it leaves. Returns the Zoroark or None."""
    side = slot[:2]
    z = _their_illusionist(b, side, create=True)
    if z is None or z is named:
        return None
    del st["theirs"][slot]
    pre = {"boosts": dict(named._boosts), "effects": dict(named._effects), "protect": named._protect_counter}
    other = slot[:2] + ("b" if slot[2] == "a" else "a")
    # the imitated Pokemon itself stands in the other slot (poke-env merged both into one object): split by slot
    twin = (other + ": " + op["ident"].split(": ", 1)[1]) if b._opponent_active_pokemon.get(other) is named else None
    _their_reveal(b, st, slot, z, named, pre, op, _z_details(b, side), twin)
    b._opponent_active_pokemon[slot] = z
    z._active, z._revealed = True, True
    named._active = twin is not None
    st.setdefault("deduced", {})[slot] = (op["ident"], slot + ": " + z._name)
    return z


_LEARN = None
_COPY_MOVES = ("transform", "mimic", "sketch")


def _learnsets():
    global _LEARN
    if _LEARN is None:
        import json
        import os
        try:
            with open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "champions_learnsets.json")) as f:
                _LEARN = {k: set(v) for k, v in json.load(f).items()}
        except Exception:
            _LEARN = {}
    return _LEARN


def _unlearnable(b, st, side, named, mid, op):
    """`named` (shown in this disguise period) used `mid`, which its species can't learn while their Illusion user can,
    and nothing in the stint copied moves (Transform / Imposter, Mimic, Sketch)."""
    L = _learnsets()
    sp = _tid(named.species)
    if not L or sp not in L or mid in L[sp] or mid in ("struggle", "recharge"):
        return False
    det = _z_details(b, side)
    zsp = _tid(det.split(",")[0]) if det else None
    if not zsp or mid not in L.get(zsp, ()):
        return False
    for e in b._replay_data[op["start"]:]:
        if len(e) > 2 and e[2] == op["ident"] and (e[1] == "-transform" or (e[1] == "move" and len(e) > 3 and _tid(e[3]) in _COPY_MOVES)):
            return False
    return True


def _z_details(b, side):
    """Their Illusion user's details from team preview ('|poke|p2|Zoroark-Hisui, L50, M|'), or None."""
    for ev in b._replay_data:
        if len(ev) > 3 and ev[1] == "poke" and ev[2] == side and _tid(ev[3].split(",")[0]) in ILLUSION_SPECIES:
            return ev[3]
    return None


def _rename_guess(b, st, side, name, details):
    """(2026-10-19) A Zoroark named by us before Showdown ever showed it (deduced while disguised) appears under its real
    nickname: rename the log lines we wrote with the guessed one (poke-env re-keys the object itself by its details)."""
    g = st.get("zguess")
    if not g or g[0] != side or _tid(str(details).split(",")[0]) not in ILLUSION_SPECIES or name == g[1]:
        return
    st["zguess"] = None
    for sl in ("a", "b", "c"):
        old, new = side + sl + ": " + g[1], side + sl + ": " + name
        rd = b._replay_data
        for i, ev in enumerate(rd):
            rd[i] = _rewrite(ev, old, new)
        for s, (o, n) in list(st.get("deduced", {}).items()):
            if n == old:
                st["deduced"][s] = (o, new)


def _their_illusionist(b, side, create=False):
    """Their Illusion user (from team preview / what has been seen), as a poke-env Pokemon in opponent_team.
    create: not seen yet -> added under Showdown's default nickname (the base species name), details from team preview."""
    for k, m in b._opponent_team.items():
        if _tid(m.species) in ILLUSION_SPECIES:
            return m
    if create:
        det = _z_details(b, side)
        if det:
            name = det.split(",")[0].split("-")[0]
            try:
                z = b.get_pokemon(side + ": " + name, details=det)
            except Exception:
                return None
            _state(b)["zguess"] = (side, name)
            return z
    for p in getattr(b, "teampreview_opponent_team", None) or []:
        if _tid(p.species) in ILLUSION_SPECIES:
            name = p.species
            try:
                return b.get_pokemon(side + ": " + name, details=name)
            except Exception:
                return None
    return None
