"""Small helpers for rebuilt positions: id normalisation, putting fainted Pokemon back for a replacement search, mid-turn switch kinds."""


_base_cache = {}


def to_id(s):
    return "".join(c for c in str(s).lower() if c.isalnum())


def base_id(name):
    k = to_id(name)
    if k not in _base_cache:
        import bridge
        e = bridge.gen9().pokedex.get(k) or {}
        _base_cache[k] = to_id(e.get("baseSpecies") or e.get("name") or name)
    return _base_cache[k]


def put_back_fainted(e, side):
    """(2026-10-19) the rebuilt position leaves a fainted Pokemon's slot empty; when that side still has a Pokemon to send in,
    put a fainted one back in the slot so the engine asks for the replacement (Showdown asks both sides at the same time).
    Returns True when something was put back (call refresh() after)."""
    sd = e.state()["sides"][side]
    mons, act = sd["mons"], sd["active"]
    dead = [j for j, m in enumerate(mons) if (m["fainted"] or m["hp"] <= 0) and j not in act]
    bench = [j for j, m in enumerate(mons) if not (m["fainted"] or m["hp"] <= 0) and j not in act]
    done = False
    for k in range(len(act)):
        if act[k] is None and dead and bench:
            e.set_active(side, k, dead.pop(0))
            bench.pop(0)
            done = True
    return done


def midturn_kinds(battle):
    """(2026-10-19) per our slot, how a living Pokemon is leaving mid-turn: 'baton' (Baton Pass), 'shedtail' (Shed Tail) or ''
    (U-turn, Eject Button...), from the last move this turn by the Pokemon in that slot"""
    return midturn_kinds_log(getattr(battle, "_replay_data", None) or [], battle.player_role or "p1")


def midturn_kinds_log(log, role):
    """midturn_kinds from a log (lists of fields, or '|'-separated strings)"""
    out = ["", ""]
    for ev in reversed(log):
        if isinstance(ev, str):
            ev = ev.split("|")
        if len(ev) > 1 and ev[1] == "turn":
            break
        if len(ev) > 3 and ev[1] == "move" and str(ev[2]).startswith(role):
            k = "ab".find(str(ev[2])[2:3])
            if k >= 0 and not out[k]:
                mv = to_id(ev[3])
                out[k] = "baton" if mv == "batonpass" else "shedtail" if mv == "shedtail" else "-"
    return [x if x != "-" else "" for x in out]


def forced_setup(eb, fs, kinds=("", "")):
    """(2026-10-19) the position the bot's replacement search (agent._search_switch) plays a forced decision from. A fainted
    Pokemon is put back in its empty slot (both sides: Showdown asks both to replace at the same time). A Pokemon leaving
    mid-turn (U-turn, Eject Button, Baton Pass, Shed Tail...) gets the engine's own mid-turn switch request (set_switching:
    switch-out effects run, Baton Pass / Shed Tail hand over, then the end of the turn). Returns (engine, midturn) or None."""
    st = eb.state()
    e = eb.copy()
    mons = st["sides"][0]["mons"]
    spare = [j for j, m in enumerate(mons) if (m["fainted"] or m["hp"] <= 0) and j not in st["sides"][0]["active"]]
    leaving = []
    for k in (0, 1):
        if k < len(fs) and fs[k]:
            idx = st["sides"][0]["active"][k]
            if idx is None:
                if not spare:
                    return None
                e.set_active(0, k, spare.pop(0))
            elif not (mons[idx]["fainted"] or mons[idx]["hp"] <= 0):
                leaving.append(k)
    put_back_fainted(e, 0)
    put_back_fainted(e, 1)
    e.refresh()
    if leaving:
        if not hasattr(e, "set_switching"):
            return None
        # (2026-10-19) several slots can leave at once (Flip Turn into a partner holding an Eject Button)
        if not all(e.set_switching(0, k, baton=kinds[k] == "baton", shed_tail=kinds[k] == "shedtail") for k in leaving):
            return None
    if not e.needs_choice(0):
        return None
    return e, bool(leaving)
