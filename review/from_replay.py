"""A Showdown replay (the spectators' log) + the reviewed player's team paste -> the same input as a recorded game:
the protocol lines with that player's |request| messages and choices reconstructed, so the whole
review pipeline (replay.py -> core.py) works on it.

    game = from_replay.build(log_text, player="p1" or a name, paste=<team export>)
    game = from_replay.build(from_replay.fetch("https://replay.pokemonshowdown.com/<id>"), player=..., paste=...)

Reconstructed, per decision of the player:
  * team preview: the six from the paste; the pick = the Pokemon that appeared (leads = the first two sent out); if fewer
    than four ever appeared, the rest are filled in paste order and marked "assumed"
  * each turn: a request with the active Pokemon and the current state; the choice per slot: a switch at the start of the
    turn (before any move), else that slot's own move line (move, target, Mega); "unknown" when the log can't tell (the
    Pokemon flinched / slept / was paralyzed / fainted before moving)
  * replacements after a faint, and mid-turn switch-outs (U-turn, Eject Button...): a forced-switch request + the switch
  * the player's HP: the replay shows percentages; converted to exact HP from the paste's stats (error < 1% of max HP)
Stats: Champions formula (HP = base + points + 75, other = (base + points + 20) x nature), level 50.
"""
import json
import re
import urllib.request

NATURES = {
    "adamant": ("atk", "spa"), "bold": ("def", "atk"), "brave": ("atk", "spe"), "calm": ("spd", "atk"), "careful": ("spd", "spa"),
    "gentle": ("spd", "def"), "hasty": ("spe", "def"), "impish": ("def", "spa"), "jolly": ("spe", "spa"), "lax": ("def", "spd"),
    "lonely": ("atk", "def"), "mild": ("spa", "def"), "modest": ("spa", "atk"), "naive": ("spe", "spd"), "naughty": ("atk", "spd"),
    "quiet": ("spa", "spe"), "rash": ("spa", "spd"), "relaxed": ("def", "spe"), "sassy": ("spd", "spe"), "timid": ("spe", "atk"),
}
STAT_KEYS = {"hp": "hp", "atk": "atk", "def": "def", "spa": "spa", "spd": "spd", "spe": "spe"}


def to_id(s):
    return re.sub(r"[^a-z0-9]", "", str(s).lower())


def fetch(url):
    """The log text of a replay page (replay.pokemonshowdown.com/<id> or its .json / .log)."""
    base = url.split("?")[0].rstrip("/")
    for suffix in (".json", ""):
        u = base if base.endswith((".json", ".log")) else base + suffix
        with urllib.request.urlopen(urllib.request.Request(u, headers={"User-Agent": "vgc-review"}), timeout=30) as r:
            body = r.read().decode("utf-8")
        if u.endswith(".json") or body.lstrip().startswith("{"):
            return json.loads(body)["log"]
        if body.lstrip().startswith("|"):
            return body
    raise ValueError(f"couldn't read a replay log from {url}")


_DEX = {}


def dex():
    if not _DEX:
        from poke_env.data import GenData
        d = GenData.from_gen(9)
        _DEX["pokedex"], _DEX["moves"] = d.pokedex, d.moves
    return _DEX


def parse_paste(text):
    """[{species, name, item, ability, moves, points, nature}] from a Showdown team export."""
    mons = []
    for blk in [b.strip() for b in re.split(r"\n\s*\n", text.strip()) if b.strip()]:
        lines = [ln.strip() for ln in blk.split("\n") if ln.strip()]
        head = lines[0]
        item = head.split(" @ ", 1)[1].strip() if " @ " in head else ""
        head = head.split(" @ ")[0].strip()
        head = re.sub(r"\s*\((M|F)\)\s*$", "", head)
        m = re.match(r"^(.*)\((.+)\)$", head)
        name, species = (m.group(1).strip(), m.group(2).strip()) if m else (head, head)
        mon = {"species": species, "name": name, "item": item, "ability": "", "moves": [], "points": {}, "nature": "serious"}
        for ln in lines[1:]:
            if ln.startswith("Ability:"):
                mon["ability"] = ln.split(":", 1)[1].strip()
            elif ln.startswith(("EVs:", "SPs:")):
                for part in ln.split(":", 1)[1].split("/"):
                    v, k = part.strip().split(" ", 1)
                    mon["points"][{"hp": "hp", "atk": "atk", "def": "def", "spa": "spa", "spd": "spd", "spe": "spe"}[to_id(k)]] = int(v)
            elif ln.endswith("Nature"):
                mon["nature"] = to_id(ln.split()[0])
            elif ln.startswith("-"):
                mon["moves"].append(ln[1:].strip())
        mons.append(mon)
    return mons


def is_mega_stone(item):
    i = to_id(item)
    return i != "eviolite" and bool(re.search(r"ite(x|y|z)?$", i))


def stats_of(mon, species=None):
    entry = dex()["pokedex"][to_id(species or mon["species"])]
    base = entry["baseStats"]
    up, down = NATURES.get(mon["nature"], (None, None))
    out = {"hp": base["hp"] + mon["points"].get("hp", 0) + 75}
    for k in ("atk", "def", "spa", "spd", "spe"):
        raw = base[k] + mon["points"].get(k, 0) + 20
        num = 110 if k == up else 90 if k == down else 100
        out[k] = raw if num == 100 else (raw * num) // 100
    return out


class Ours:
    """The reviewed player's six, as the replay goes on."""

    def __init__(self, paste, side):
        self.side = side
        self.mons = parse_paste(paste)
        for m in self.mons:
            m["stats"] = stats_of(m)
            m["maxhp"] = m["stats"]["hp"]
            m["hp"] = m["maxhp"]
            m["status"] = ""
            m["fainted"] = False
            m["item_now"] = to_id(m["item"])
            m["pp_used"] = {}
            m["ident_name"] = None                 # the name the log uses ("p1a: <name>")
            m["appeared"] = False
        self.active = [None, None]                 # index into mons per slot
        self.order = []                            # the brought four in Showdown's request order (see switch_in)
        self.mega_used = False

    def find(self, ident_name, details=None):
        for i, m in enumerate(self.mons):
            if m["ident_name"] == ident_name:
                return i
        sp = to_id(details.split(",")[0]) if details else to_id(ident_name)
        for i, m in enumerate(self.mons):           # first sighting: by species (Mega / forms: by prefix), then by name
            if m["ident_name"] is None and (to_id(m["species"]) == sp or sp.startswith(to_id(m["species"]))):
                m["ident_name"] = ident_name
                return i
        for i, m in enumerate(self.mons):
            if m["ident_name"] is None and to_id(m["name"]) == to_id(ident_name):
                m["ident_name"] = ident_name
                return i
        return None

    def exact_hp(self, i, hp_field):
        """'54/100 par' (a spectator's percentage) -> the exact '111/206 par' for our Pokemon."""
        parts = hp_field.split(" ", 1)
        hp = parts[0]
        if hp == "0" or "fnt" in hp_field:
            return hp_field
        # "54/100", or with a health-bar colour from some replays: "54/100y" (y / g / r); any denominator (e.g. 48 pixels)
        m = re.fullmatch(r"(\d+)(?:/(\d+))?[a-z]?", hp)
        if not m:
            return hp_field
        a, b = int(m.group(1)), int(m.group(2) or 100)
        mx = self.mons[i]["maxhp"]
        exact = int(a) if b == mx else max(1, round(a / b * mx))
        return f"{exact}/{mx}" + (" " + parts[1] if len(parts) > 1 else "")

    def pokemon_entry(self, i, active):
        m = self.mons[i]
        cond = "0 fnt" if m["fainted"] else f"{m['hp']}/{m['maxhp']}" + (f" {m['status']}" if m["status"] else "")
        return {"ident": f"{self.side}: {m['ident_name'] or m['name']}", "details": f"{m.get('species_now', m['species'])}, L50",
                "condition": cond, "active": active, "stats": {k: m["stats"][k] for k in ("atk", "def", "spa", "spd", "spe")},
                "moves": [to_id(x) for x in m["moves"]], "baseAbility": to_id(m["ability"]), "item": m["item_now"],
                "pokeball": "pokeball", "ability": to_id(m["ability"])}

    def switch_in(self, slot, i):
        """Showdown's request order: the incoming Pokemon swaps places with the one at the slot's position."""
        if i in self.order:
            j = self.order.index(i)
            self.order[slot], self.order[j] = self.order[j], self.order[slot]
        self.active[slot] = i

    def mega_evolve(self, i, species):
        m = self.mons[i]
        m["stats"] = stats_of(m, species)
        m["species_now"] = species
        self.mega_used = True

    def side_entry(self, name, brought):
        order = list(self.order)
        return {"name": name, "id": self.side, "pokemon": [self.pokemon_entry(i, i in self.active) for i in order]}, order


def finished(lines):
    """True if the battle log holds the game's result (|win| or |tie|)."""
    return any(ln.startswith("|win|") or ln.rstrip() == "|tie" or ln.startswith("|tie|") for ln in lines)


def build(log_text, player, paste, fmt="gen9championsvgc2026regmc"):
    lines = [ln for ln in log_text.split("\n") if ln.startswith("|")]
    if not finished(lines):
        raise ValueError("only finished games can be reviewed (no |win| or |tie| in this log)")
    players = {}
    for ln in lines:
        p = ln.split("|")
        if len(p) > 3 and p[1] == "player" and p[3]:
            players[p[2]] = p[3]
    side = player if player in ("p1", "p2") else next((s for s, n in players.items() if to_id(n) == to_id(player)), None)
    if side is None:
        raise ValueError(f"player {player!r} not in this replay ({players})")
    name = players.get(side, side)
    ours = Ours(paste, side)
    mdex = dex()["moves"]
    # the brought four: those that appear (leads first), then paste order to fill
    appear = []
    for ln in lines:
        p = ln.split("|")
        if len(p) > 3 and p[1] in ("switch", "drag", "replace") and p[2].startswith(side):
            i = ours.find(p[2].split(": ", 1)[1], p[3])
            if i is not None and i not in appear:
                appear.append(i)
    for m in ours.mons:                            # (find() assigned names: reset for the real pass)
        m["ident_name"] = None
    assumed = [i for i in range(len(ours.mons)) if i not in appear][: max(0, 4 - len(appear))]
    brought = (appear + assumed)[:4]
    ours.order = list(brought)                     # after team preview: the chosen order, leads first
    events, rq = [], [0]

    def request(obj):
        rq[0] += 1
        obj["rqid"] = rq[0]
        events.append("|request|" + json.dumps(obj))

    def move_request(force=None):
        sd, order = ours.side_entry(name, brought)
        act = []
        for k in (0, 1):
            i = ours.active[k]
            if i is None or ours.mons[i]["fainted"]:
                continue
            m = ours.mons[i]
            entry = {"moves": [{"move": mdex.get(to_id(x), {}).get("name", x), "id": to_id(x),
                                   "pp": max(0, mdex.get(to_id(x), {}).get("pp", 16) - m["pp_used"].get(to_id(x), 0)),
                                   "maxpp": mdex.get(to_id(x), {}).get("pp", 16), "target": mdex.get(to_id(x), {}).get("target", "normal"),
                                   "disabled": False} for x in m["moves"]]}
            if not ours.mega_used and is_mega_stone(m["item_now"]) and "species_now" not in m:
                entry["canMegaEvo"] = True
            act.append(entry)
        obj = {"side": sd}
        if force is not None:
            obj["forceSwitch"] = force
        else:
            obj["active"] = act
        request(obj)
        return order

    def slot_of(ident):
        return 0 if ident[2] == "a" else 1

    # the turn's lines (for reading the choices): index of each "|turn|" line
    turn_at = [k for k, ln in enumerate(lines) if ln.startswith("|turn|")]
    k = 0
    pending_force = None
    while k < len(lines):
        ln = lines[k]
        p = ln.split("|")
        kind = p[1] if len(p) > 1 else ""
        # our HP / status bookkeeping (before the line goes out, so the next request sees the new state)
        if kind in ("switch", "drag", "replace", "detailschange") and len(p) > 3 and p[2].startswith(side):
            nm = p[2].split(": ", 1)[1]
            i = ours.find(nm, p[3])
            if i is not None:
                if kind != "detailschange":
                    if pending_force is not None and kind == "switch":
                        # a forced replacement / switch-out: the request goes out before the switch line
                        slots = [False, False]
                        slots[slot_of(p[2])] = True
                        order = move_request(force=slots)
                        toks = ["pass", "pass"]
                        toks[slot_of(p[2])] = f"switch {order.index(i) + 1}"
                        events_line_choice = ", ".join(toks)
                        pending_force = None
                    else:
                        events_line_choice = None
                    ours.switch_in(slot_of(p[2]), i)
                    ours.mons[i]["appeared"] = True
                    if len(p) > 4:
                        p[4] = ours.exact_hp(i, p[4])
                        ln = "|".join(p)
                        hp = p[4].split(" ")[0]
                        if "/" in hp:
                            ours.mons[i]["hp"] = int(hp.split("/")[0])
                    if events_line_choice:
                        events.append({"choice": events_line_choice})
                if kind == "detailschange" and "-Mega" in p[3]:
                    ours.mega_evolve(i, p[3].split(",")[0])
        if kind in ("-damage", "-heal", "-sethp") and len(p) > 3 and p[2].startswith(side):
            i = ours.find(p[2].split(": ", 1)[1])
            if i is not None:
                p[3] = ours.exact_hp(i, p[3])
                ln = "|".join(p)
                hp = p[3].split(" ")[0]
                if hp.startswith("0"):
                    ours.mons[i]["hp"] = 0
                elif "/" in hp:
                    ours.mons[i]["hp"] = int(hp.split("/")[0])
        if kind == "swap" and len(p) > 2 and p[2].startswith(side):
            # Ally Switch: our two active Pokemon trade slots (and places in Showdown's request order)
            ours.active[0], ours.active[1] = ours.active[1], ours.active[0]
            ours.order[0], ours.order[1] = ours.order[1], ours.order[0]
        if kind == "faint" and p[2].startswith(side):
            i = ours.find(p[2].split(": ", 1)[1])
            if i is not None:
                ours.mons[i]["fainted"] = True
                ours.mons[i]["hp"] = 0
        if kind == "-status" and p[2].startswith(side):
            i = ours.find(p[2].split(": ", 1)[1])
            if i is not None:
                ours.mons[i]["status"] = p[3]
        if kind == "-curestatus" and p[2].startswith(side):
            i = ours.find(p[2].split(": ", 1)[1])
            if i is not None:
                ours.mons[i]["status"] = ""
        if kind == "-enditem" and p[2].startswith(side):
            i = ours.find(p[2].split(": ", 1)[1])
            if i is not None:
                ours.mons[i]["item_now"] = ""
        if kind == "move" and p[2].startswith(side) and "[from]" not in ln:
            i = ours.find(p[2].split(": ", 1)[1])
            if i is not None:
                mid = to_id(p[3])
                ours.mons[i]["pp_used"][mid] = ours.mons[i]["pp_used"].get(mid, 0) + 1
            if len(p) > 3 and to_id(p[3]) in ("uturn", "voltswitch", "flipturn", "partingshot", "teleport", "batonpass", "shedtail",
                                              "chillyreception"):
                pending_force = "selfswitch"
        if kind == "-enditem" and p[2].startswith(side) and to_id(p[3] if len(p) > 3 else "") in ("ejectbutton", "ejectpack"):
            pending_force = "item"
        if kind == "-activate" and p[2].startswith(side) and "Emergency Exit" in ln:
            pending_force = "ability"
        if kind == "upkeep" and any(ours.active[s] is not None and ours.mons[ours.active[s]]["fainted"] for s in (0, 1)):
            bench = [i for i in brought if i not in ours.active and not ours.mons[i]["fainted"]]
            if bench:
                pending_force = "faint"
        events.append(ln)
        if kind == "teampreview":
            sd = {"name": name, "id": side, "pokemon": [ours.pokemon_entry(i, False) for i in range(len(ours.mons))]}
            for e in sd["pokemon"]:
                e["ident"] = f"{side}: {ours.mons[sd['pokemon'].index(e)]['name']}"
            request({"teamPreview": True, "maxChosenTeamSize": 4, "side": sd})
            events.append({"choice": "team " + "".join(str(i + 1) for i in brought), "assumed": [ours.mons[i]["species"] for i in assumed]})
        if kind == "turn":
            order = move_request()
            # this turn's lines: up to the next "|turn|" / the end
            nxt = next((t for t in turn_at if t > k), len(lines))
            seg = lines[k + 1: nxt]
            toks = []
            for s in (0, 1):
                i = ours.active[s]
                if i is None or ours.mons[i]["fainted"]:
                    toks.append("pass")
                    continue
                ident_prefix = f"{side}{'ab'[s]}: "
                tok = "unknown"
                for x in seg:
                    q = x.split("|")
                    if len(q) < 3:
                        continue
                    if q[1] == "switch" and q[2].startswith(ident_prefix):
                        j = ours.find(q[2].split(": ", 1)[1], q[3])
                        tok = f"switch {order.index(j) + 1}" if j in order else "unknown"
                        break
                    if q[1] == "move" and q[2].startswith(ident_prefix) and "[from]" not in x:
                        mid = to_id(q[3])
                        tgt = ""
                        if len(q) > 4 and q[4] and ": " in q[4] and "[spread]" not in x and q[4] != q[2]:
                            ts = q[4][:2]
                            tslot = 1 if q[4][2] == "a" else 2
                            tgt = f" {tslot}" if ts != side else f" -{tslot}"
                        mega = any(y.startswith(f"|-mega|{ident_prefix}") or (y.startswith(f"|detailschange|{ident_prefix}") and "-Mega" in y)
                                   for y in seg[: seg.index(x)])
                        tok = f"move {mid}{tgt}{' mega' if mega else ''}"
                        break
                    if q[1] in ("cant", "faint") and q[2].startswith(ident_prefix):
                        tok = "unknown"
                        break
                toks.append(tok)
            events.append({"choice": ", ".join(toks)})
        k += 1
    return {"format": fmt, "player": side, "name": name, "events": events, "source": "replay",
            "assumed_brought": [ours.mons[i]["species"] for i in assumed]}
