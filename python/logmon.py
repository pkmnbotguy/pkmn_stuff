"""Pokemon lookup for reading a battle's log AFTER the fact (2026-10-18).

poke-env's get_pokemon('p2b: Malamar') checks who is active in slot b NOW and, if that is an Illusion user, returns it
(its Zoroark handling for live messages). Code that re-reads old log lines (log_vols, bridge.log_history /
status_progress / field_timers...) then books a past Malamar move on today's Zoroark. This lookup only strips the
position (illusion_fix already rewrote our disguised Zoroark's lines) and never creates Pokemon."""


def log_mon(battle, ident):
    ident = str(ident)
    if len(ident) > 3 and ident[3] != " ":
        ident = ident[:2] + ident[3:]
    m = battle._team.get(ident) or battle._opponent_team.get(ident)
    if m is not None:
        return m
    return battle.get_pokemon(ident)
