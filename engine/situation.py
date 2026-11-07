"""Set up a battle situation of your own and play it: in the engine, and optionally in real Showdown to compare.

A situation is a small text file (see docs/situations.md and examples/situations/):

    [p1]                      # side 1: Showdown export of the Pokemon it brought; the first two are on the field
    Garchomp @ Life Orb
    Ability: Rough Skin
    ...
    [p2]
    ...
    [setup]                   # optional: change the position before turn 1
    p1a hp 40%, status par, boost atk +2
    p2b item none
    weather rain 3
    p2 reflect 5
    [turns]                   # one line per turn: both sides' choices
    p1: Earthquake, Protect | p2: Moonblast p1a, switch 3

Then:
    python situation.py my_situation.txt                  # play it 200 times, print the outcome summary + one log
    python situation.py my_situation.txt --seeds 1000 --logs 3
    python situation.py my_situation.txt --showdown ../pokemon-showdown    # same situation in real Showdown, compare

From Python:
    import situation
    s = situation.load("my_situation.txt")
    b = s.battle()                 # a vgcsim.Battle in that position (before turn 1)
    s.play(b)                      # play the scripted turns on it
    print(situation.summary(s.run(seeds=500)))
"""
import argparse
import os
import random
import re
import sys

ROOT = os.path.dirname(os.path.abspath(__file__))
DATA = os.path.join(ROOT, "data")

STATUSES = {"brn": "brn", "burn": "brn", "par": "par", "paralysis": "par", "paralyzed": "par", "psn": "psn", "poison": "psn",
            "tox": "tox", "toxic": "tox", "slp": "slp", "sleep": "slp", "asleep": "slp", "frz": "frz", "freeze": "frz",
            "frozen": "frz", "none": "", "healthy": ""}
BOOST_NAMES = {"atk": "atk", "attack": "atk", "def": "def", "defense": "def", "spa": "spa", "spatk": "spa", "spd": "spd",
               "spdef": "spd", "spe": "spe", "speed": "spe", "accuracy": "accuracy", "acc": "accuracy", "evasion": "evasion",
               "eva": "evasion"}
WEATHERS = {"rain": "rain", "raindance": "rain", "sun": "sun", "sunnyday": "sun", "sand": "sand", "sandstorm": "sand",
            "snow": "snow", "snowscape": "snow", "hail": "snow", "none": "none"}
TERRAINS = {"electric": "electric", "grassy": "grassy", "psychic": "psychic", "misty": "misty", "none": "none"}
SIDE_WORDS = {"tailwind", "reflect", "lightscreen", "auroraveil", "safeguard", "stealthrock", "rocks", "sr", "spikes",
              "toxicspikes", "stickyweb", "megaused", "nomega"}


def to_id(x):
    return re.sub(r"[^a-z0-9]", "", str(x).lower())


class SituationError(ValueError):
    pass


def _dex():
    import vgcsim
    if "dex" not in _G:
        _G["dex"] = vgcsim.Dex(os.path.join(DATA, "dex.json"))
    return _G["dex"]


_G = {}


# ---------------------------------------------------------------------------------------------------------- file format

def _sections(text):
    out, cur = {}, None
    for raw in text.replace("\r\n", "\n").split("\n"):
        line = raw.split("#", 1)[0].rstrip() if not raw.lstrip().startswith("- ") else raw.rstrip()
        m = re.fullmatch(r"\s*\[(\w+)\]\s*", line)
        if m:
            cur = m.group(1).lower()
            if cur not in ("p1", "p2", "setup", "turns"):
                raise SituationError(f"unknown section [{cur}] (use [p1], [p2], [setup], [turns])")
            out.setdefault(cur, [])
            continue
        if cur is None:
            if line.strip():
                raise SituationError(f"text before the first section: {line.strip()!r}")
            continue
        out[cur].append(line)
    for need in ("p1", "p2"):
        if need not in out:
            raise SituationError(f"missing section [{need}]")
    return out


def _sets(lines, side):
    blocks = [b.strip() for b in "\n".join(lines).split("\n\n") if b.strip()]
    if not 2 <= len(blocks) <= 6:
        raise SituationError(f"[p{side + 1}] needs 2 to 6 Pokemon (blank line between sets), found {len(blocks)}")
    # a line that isn't part of a Showdown set would be ignored silently by the team reader: say so instead
    ok = re.compile(r"^(- |Ability:|Level:|EVs:|IVs:|Tera Type:|Shiny:|Gender:|Happiness:|Hidden Power:|Gigantamax:|Dynamax Level:|"
                    r"\w+ Nature\s*$)")
    for blk in blocks:
        for ln in blk.split("\n")[1:]:
            if ln.strip() and not ok.match(ln.strip()):
                raise SituationError(f"[p{side + 1}] {blk.split(chr(10))[0].strip()}: {ln.strip()!r} isn't a line of a Showdown "
                                     "set (a missing [setup] / [turns] header?)")
    return blocks


def _set_info(block):
    first = block.split("\n")[0]
    name = first.split("@")[0].strip()
    name = re.sub(r"\s*\((M|F)\)\s*$", "", name)
    m = re.match(r"^(.*?)\s*\(([^()]+)\)$", name)           # "Nickname (Species)"
    nick, species = (m.group(1), m.group(2)) if m else (name, name)
    moves = [ln[2:].strip() for ln in block.split("\n") if ln.startswith("- ")]
    return {"nick": nick.strip(), "species": species.strip(), "moves": moves}


class Situation:
    """A parsed situation file: two sides' Pokemon, setup steps, scripted turns."""

    def __init__(self, text, name="situation"):
        sec = _sections(text)
        self.name = name
        self.sets = [_sets(sec["p1"], 0), _sets(sec["p2"], 1)]
        self.info = [[_set_info(b) for b in side] for side in self.sets]
        self.turn = 1
        self.calls = []                                         # (method, args, kwargs) on vgcsim.Battle, in order
        self.notes = []                                         # setup the Showdown comparison can't mirror
        for n, line in enumerate(sec.get("setup", []), 1):
            if line.strip():
                try:
                    self._setup_line(line.strip())
                except SituationError as e:
                    raise SituationError(f"[setup] line {n} ({line.strip()!r}): {e}") from None
        self.turns = [ln.strip() for ln in sec.get("turns", []) if ln.strip()]
        for n, line in enumerate(self.turns, 1):         # the format of every turn line is checked now, not mid-game
            try:
                turn = self.parse_turn(line[7:] if line.lower().startswith("replace") else line)
                if not line.lower().startswith("replace"):
                    for side in range(2):
                        if side not in turn:
                            raise SituationError(f"no choices for p{side + 1}")
            except SituationError as e:
                raise SituationError(f"[turns] line {n} ({line!r}): {e}") from None

    # -- who is who --------------------------------------------------------------------------------------------------
    def find(self, side, word):
        """a Pokemon of `side` by team position (1-6), species or nickname -> its index in the team"""
        w = to_id(word)
        if w.isdigit():
            k = int(w) - 1
            if 0 <= k < len(self.sets[side]):
                return k
            raise SituationError(f"p{side + 1} has no Pokemon number {w} (it has {len(self.sets[side])})")
        hits = [k for k, i in enumerate(self.info[side]) if w in (to_id(i["nick"]), to_id(i["species"]))]
        if not hits:
            hits = [k for k, i in enumerate(self.info[side]) if to_id(i["species"]).startswith(w)]
        if len(hits) != 1:
            names = ", ".join(i["species"] for i in self.info[side])
            raise SituationError(f"p{side + 1} has {'no' if not hits else 'more than one'} Pokemon called {word!r} ({names})")
        return hits[0]

    def who(self, ref):
        """'p1a' / 'p1b' (the field at the start), 'p2:3' / 'p2 3' (team position), 'p1 Garchomp' -> (side, index)"""
        m = re.fullmatch(r"p([12])([ab])", ref)
        if m:
            return int(m.group(1)) - 1, "ab".index(m.group(2))
        m = re.fullmatch(r"p([12])[:\s]+(.+)", ref)
        if m:
            side = int(m.group(1)) - 1
            return side, self.find(side, m.group(2).strip())
        raise SituationError(f"don't know which Pokemon {ref!r} is (use p1a, p1b, p2a, p2b, p1:3 or 'p2 Garchomp')")

    # -- setup -------------------------------------------------------------------------------------------------------
    def _setup_line(self, line):
        words = line.split()
        head = words[0].lower()
        if head == "turn":
            self.turn = int(words[1])
            return
        if head in ("weather", "terrain"):
            table = WEATHERS if head == "weather" else TERRAINS
            what = table.get(to_id(words[1]) if len(words) > 1 else "")
            if what is None:
                raise SituationError(f"unknown {head} (one of {', '.join(sorted(set(table.values())))})")
            turns = int(words[2]) if len(words) > 2 else 5
            self.calls.append(("set_field", (), {head: what, head + "_turns": turns}))
            return
        if head in ("trickroom", "trick", "tr"):
            self.calls.append(("set_field", (), {"trick_room": int(words[-1]) if words[-1].isdigit() else 5}))
            return
        if head in ("gravity", "magicroom", "wonderroom", "fairylock"):
            key = {"gravity": "gravity", "magicroom": "magic_room", "wonderroom": "wonder_room", "fairylock": "fairy_lock"}[head]
            self.calls.append(("set_extra", (0,), {key: int(words[1]) if len(words) > 1 else 5}))
            return
        m = re.fullmatch(r"p([12])", head)
        if m and len(words) > 1 and to_id(words[1]) not in SIDE_WORDS:
            m = None                                            # "p1 Garchomp hp 50%": a Pokemon, not a side condition
        if m:                                                   # side conditions: "p2 reflect 5, stealthrock"
            side = int(m.group(1)) - 1
            for part in " ".join(words[1:]).split(","):
                self._side_part(side, part.split())
            return
        # a Pokemon: "p1a hp 50%, status par" / "p2 Garchomp item none" / "p1:3 fainted"
        m = re.match(r"^(p[12][ab]|p[12]:\S+|p[12]\s+\S+)\s*(.*)$", line)
        if not m:
            raise SituationError("expected weather / terrain / trickroom / gravity / turn, a side (p1 ...) or a Pokemon (p1a ...)")
        side, idx = self.who(m.group(1).strip())
        rest = m.group(2).strip()
        if not rest:
            raise SituationError("nothing to set")
        for part in rest.split(","):
            if part.strip():
                self._mon_part(side, idx, part.split())

    def _side_part(self, side, w):
        if not w:
            return
        key = to_id(w[0])
        n = int(w[1]) if len(w) > 1 and w[1].isdigit() else None
        table = {"tailwind": ("set_side", "tailwind", 4), "reflect": ("set_side", "reflect", 5),
                 "lightscreen": ("set_side", "light_screen", 5), "auroraveil": ("set_side", "aurora_veil", 5),
                 "safeguard": ("set_extra", "safeguard", 5)}
        if key in table:
            meth, kw, default = table[key]
            self.calls.append((meth, (side,), {kw: n if n is not None else default}))
        elif key in ("stealthrock", "rocks", "sr"):
            self.calls.append(("set_side", (side,), {"stealth_rock": True}))
        elif key == "spikes":
            self.calls.append(("set_side", (side,), {"spikes": n or 1}))
        elif key == "toxicspikes":
            self.calls.append(("set_side", (side,), {"toxic_spikes": n or 1}))
        elif key == "stickyweb":
            self.calls.append(("set_side", (side,), {"sticky_web": True}))
        elif key in ("megaused", "nomega"):
            self.calls.append(("set_side", (side,), {"mega_used": True}))
            self.notes.append(f"p{side + 1} megaused")
        else:
            raise SituationError(f"unknown side condition {w[0]!r} (tailwind, reflect, lightscreen, auroraveil, safeguard, "
                                 "stealthrock, spikes N, toxicspikes N, stickyweb, megaused)")

    def _mon_part(self, side, idx, w):
        key = to_id(w[0])
        args = w[1:]
        at = (side, idx)
        if key == "hp":
            if not args:
                raise SituationError("hp needs a value: hp 50% or hp 123")
            v = args[0]
            if v.endswith("%"):
                self.calls.append(("set_mon", at, {"hp_frac": float(v[:-1]) / 100}))
            else:
                self.calls.append(("set_mon", at, {"hp": int(v)}))
        elif key == "status":
            st = STATUSES.get(to_id(args[0]) if args else "")
            if st is None:
                raise SituationError("status: brn, par, psn, tox, slp, frz or none")
            kw = {"status": st}
            if st == "slp" and len(args) > 1:               # "status slp 3" = sleeps 3 more turns
                kw["status_turns"] = int(args[1])
                self.notes.append(f"p{side + 1} {self.info[side][idx]['species']}: sleep turns")
            self.calls.append(("set_mon", at, kw))
        elif key in ("boost", "boosts"):
            b, k = {}, 0
            while k < len(args):
                m = re.fullmatch(r"([a-zA-Z]+)([+-]\d+)?", args[k])
                if not m or to_id(m.group(1)) not in BOOST_NAMES:
                    raise SituationError(f"boost: like 'boost atk +2 spe -1', got {args[k]!r}")
                stat = BOOST_NAMES[to_id(m.group(1))]
                if m.group(2) is None:
                    k += 1
                    if k >= len(args):
                        raise SituationError(f"boost {stat} needs a number")
                    val = int(args[k])
                else:
                    val = int(m.group(2))
                b[stat] = max(-6, min(6, val))
                k += 1
            self.calls.append(("set_mon", at, {"boosts": b}))
        elif key == "item":
            it = " ".join(args)
            self.calls.append(("set_mon", at, {"item": "" if to_id(it) in ("", "none", "noitem") else it}))
        elif key == "ability":
            self.calls.append(("set_ability", at, {"ability": " ".join(args)}))
        elif key in ("fainted", "faint", "ko"):
            self.calls.append(("set_mon", at, {"fainted": True}))
        elif key == "mega":
            self.calls.append(("set_mon", at, {"mega": True}))
        elif key in ("moved", "notfirstturn"):            # has already moved since switching in (no Fake Out)
            self.calls.append(("set_mon", at, {"first_turn": False}))
        elif key in ("pp", "ppused"):
            # "pp Protect 2" = 2 PP of Protect used
            mv, n = " ".join(args[:-1]), int(args[-1])
            moves = [to_id(x) for x in self.info[side][idx]["moves"]]
            if to_id(mv) not in moves:
                raise SituationError(f"{self.info[side][idx]['species']} has no move {mv!r}")
            used = [0] * len(moves)
            used[moves.index(to_id(mv))] = n
            self.calls.append(("set_mon", at, {"pp_used": used}))
            self.notes.append(f"p{side + 1} {self.info[side][idx]['species']}: pp")
        elif key in ("choicelock", "locked"):
            self.calls.append(("set_mon", at, {"choice_lock": " ".join(args)}))
            self.notes.append(f"p{side + 1} {self.info[side][idx]['species']}: choice lock")
        elif key == "protect":                             # "protect 1" = Protect used last turn (the next one may fail)
            self.calls.append(("set_mon", at, {"protect_streak": int(args[0]) if args else 1}))
        elif key == "types":
            self.calls.append(("set_types", at, {"types": [x.capitalize() for x in args]}))
            self.notes.append(f"p{side + 1} {self.info[side][idx]['species']}: types")
        elif key in ("vol", "volatile"):                   # raw engine volatile: "vol taunt 3" (see Battle.state()['...']['vols'])
            if len(args) != 2:
                raise SituationError("vol needs a name and a number: vol taunt 3")
            self.calls.append(("set_vols", at, {"vols": {args[0].lower(): int(args[1])}}))
            self.notes.append(f"p{side + 1} {self.info[side][idx]['species']}: vol {args[0]}")
        else:
            raise SituationError(f"unknown setting {w[0]!r} (hp, status, boost, item, ability, fainted, mega, moved, pp, "
                                 "choicelock, protect, types, vol)")

    # -- building ----------------------------------------------------------------------------------------------------
    def battle(self, seed=1):
        """a vgcsim.Battle in this situation, before the first scripted turn"""
        import vgcsim
        texts = ["\n\n".join(s) for s in self.sets]
        try:
            b = vgcsim.Battle.from_brought(_dex(), texts[0], texts[1], seed=seed, turn=self.turn)
        except ValueError as e:
            raise SituationError(f"the engine couldn't read a team: {e}") from None
        st = b.state()
        for meth, args, kw in self.calls:
            kw = dict(kw)
            if "hp_frac" in kw:
                kw["hp"] = max(1, int(st["sides"][args[0]]["mons"][args[1]]["maxhp"] * kw.pop("hp_frac")))
            if meth == "set_ability":
                b.set_ability(*args, kw["ability"])
            elif meth == "set_types":
                b.set_types(*args, kw["types"])
            elif meth == "set_vols":
                b.set_vols(*args, kw["vols"])
            else:
                getattr(b, meth)(*args, **kw)
        b.refresh()
        return b

    # -- turns -------------------------------------------------------------------------------------------------------
    def parse_turn(self, line):
        """'p1: X, Y | p2: Z, W' -> {side: [text slot a, text slot b]}"""
        out = {}
        for part in line.split("|"):
            m = re.fullmatch(r"\s*p([12])\s*:\s*(.*?)\s*", part)
            if not m:
                raise SituationError(f"turn line part {part.strip()!r}: write it as 'p1: move, move | p2: move, move'")
            acts = [a.strip() for a in m.group(2).split(",")]
            if len(acts) == 1:
                acts.append("")
            if len(acts) != 2:
                raise SituationError(f"p{m.group(1)}: two actions (one per slot), separated by a comma")
            out[int(m.group(1)) - 1] = acts
        return out

    def action(self, b, side, slot, text, rng=None):
        """one slot's action text -> the engine's action tuple (raises with the legal options when it doesn't fit)"""
        st = b.state()
        sd = st["sides"][side]
        legal = b.legal_actions(side)
        opts = legal[slot] if slot < len(legal) else []
        idx = sd["active"][slot]
        t = text.strip()
        low = t.lower()
        if not opts or idx is None or sd["mons"][idx]["fainted"]:
            if low not in ("", "pass", "random", "auto"):
                raise SituationError(f"p{side + 1} slot {'ab'[slot]} has nobody who can act, but the turn says {t!r}")
            return ("pass", 0, 0, False)
        if low == "":
            raise SituationError(f"p{side + 1} slot {'ab'[slot]}: no action given (write one, or 'random')")
        if low in ("random", "auto"):
            return (rng or random).choice(opts)
        if low == "pass":
            a = ("pass", 0, 0, False)
        elif low.startswith("switch"):
            to = self.find(side, t[6:].strip())
            a = ("switch", to, 0, False)
        else:
            mega = False
            words = t.split()
            if words[-1].lower() == "mega":
                mega, words = True, words[:-1]
            target = None
            if words and re.fullmatch(r"p[12][ab]|-?[12]", words[-1].lower()):
                tw = words[-1].lower()
                words = words[:-1]
                if tw.startswith("p"):
                    tside, tslot = int(tw[1]) - 1, "ab".index(tw[2])
                    target = (tslot + 1) if tside != side else -(tslot + 1)
                else:
                    target = int(tw)
            name = " ".join(words)
            moves = [mv for mv, _pp in sd["mons"][idx]["moves"]]
            if name.isdigit():
                k = int(name) - 1
            elif to_id(name) in moves:
                k = moves.index(to_id(name))
            else:
                raise SituationError(f"p{side + 1} {sd['mons'][idx]['species']} has no move {name!r} (moves: {', '.join(moves)})")
            same = [o for o in opts if o[0] == "move" and o[1] == k and o[3] == mega]
            if target is None:
                if len({o[2] for o in same}) > 1:
                    raise SituationError(f"p{side + 1} {sd['mons'][idx]['species']}: {name} needs a target "
                                         f"({', '.join(self._target_names(side, o[2]) for o in same)})")
                a = same[0] if same else ("move", k, 0, mega)
            else:
                a = ("move", k, target, mega)
        if a not in opts:
            raise SituationError(f"p{side + 1} {sd['mons'][idx]['species']}: {t!r} isn't legal now. Legal: "
                                 + "; ".join(self.describe(b, side, slot, o) for o in opts))
        return a

    def _target_names(self, side, tg):
        if tg == 0:
            return "no target"
        if tg > 0:
            return f"p{2 - side}{'ab'[tg - 1]}"
        return f"p{side + 1}{'ab'[-tg - 1]}"

    def play(self, b, rng=None, on_turn=None):
        """play the scripted turns on battle `b` (forced replacements: first healthy bench Pokemon in team order, or a
        'replace' line). Returns the list of joint actions used."""
        used = []
        lines = list(self.turns)
        k = 0
        while k < len(lines) and b.phase != "ended":
            line = lines[k]
            k += 1
            if line.lower().startswith("replace"):
                raise SituationError(f"turn line {k}: a 'replace' line only fits right after a turn in which a Pokemon fainted")
            turn = self.parse_turn(line)
            acts = []
            for side in range(2):
                if side not in turn:
                    raise SituationError(f"turn line {k}: no choices for p{side + 1}")
                acts.append([self.action(b, side, slot, turn[side][slot], rng) for slot in range(2)])
            t0 = b.turn
            b.choose(0, *acts[0])
            b.choose(1, *acts[1])
            # replacements: a following 'replace p1: Garchomp | p2: ...' line, else first healthy bench Pokemon
            repl = None
            if k < len(lines) and lines[k].lower().startswith("replace"):
                repl = self.parse_turn(lines[k][7:].strip())
                k += 1
            self._resolve(b, t0, repl)
            used.append(acts)
            if on_turn:
                on_turn(b)
        return used

    def _resolve(self, b, start_turn, repl=None):
        for _ in range(12):
            if b.phase == "ended" or (b.turn != start_turn and b.phase == "turn"):
                return
            acted = False
            for side in range(2):
                if b.phase == "ended" or not b.needs_choice(side):
                    continue
                sd = b.state()["sides"][side]
                need = sd["switch_needed"]
                active = [a for a in sd["active"] if a is not None]
                bench = [i for i, m in enumerate(sd["mons"]) if not m["fainted"] and i not in active]
                dead = [i for i, m in enumerate(sd["mons"]) if m["fainted"]]
                rv = sd.get("reviving") or [False, False]
                pair = []
                for slot in range(2):
                    want = repl.get(side, ["", ""])[slot].strip() if repl else ""
                    want = re.sub(r"^switch\s+", "", want, flags=re.I)
                    if slot < len(need) and need[slot]:
                        pool = dead if rv[slot] else bench
                        if want and want.lower() != "pass":
                            j = self.find(side, want)
                            if j not in pool:
                                raise SituationError(f"replace: p{side + 1} can't bring in {want!r} now")
                            pool.remove(j)
                            pair.append(("switch", j, 0, False))
                        elif pool:
                            pair.append(("switch", pool.pop(0), 0, False))
                        else:
                            pair.append(("pass", 0, 0, False))
                    else:
                        pair.append(("pass", 0, 0, False))
                b.choose(side, pair[0], pair[1])
                acted = True
            if not acted:
                return

    # -- many games --------------------------------------------------------------------------------------------------
    def run(self, seeds=200, seed0=11):
        """play the situation `seeds` times; returns a list of end states (dicts, see outcome())"""
        base = self.battle()
        rows = []
        for k in range(seeds):
            c = base.copy()
            c.reseed(seed0 + k * 1000003)
            done = []
            stop = None
            try:
                self.play(c, rng=random.Random(seed0 + k), on_turn=lambda b: done.append(1))
            except SituationError as e:      # the dice changed the board so a later scripted turn no longer fits
                if not done:                  # the first turn is the same in every game: the script itself is wrong
                    raise SituationError(f"[turns] line 1: {e}") from None
                stop = (len(done) + 1, str(e))
            r = outcome(c)
            r["stopped"] = stop
            rows.append(r)
        return rows

    def log(self, seed=11):
        """the battle log of one game (Pokemon named; '-damage' / '-heal' lines show the HP LEFT)"""
        b = self.battle()
        b.reseed(seed)
        b.enable_log()
        done = []
        note = None
        try:
            self.play(b, rng=random.Random(seed), on_turn=lambda x: done.append(1))
        except SituationError as e:
            if not done:
                raise SituationError(f"[turns] line 1: {e}") from None
            note = f"(the script stopped fitting on turn line {len(done) + 1}: {e})"
        names = [[m["set_species"] for m in sd["mons"]] for sd in b.state()["sides"]]

        def name(m):
            s_, i_ = int(m.group(1)), int(m.group(2))
            return f"|p{s_ + 1} {names[s_][i_] if i_ < len(names[s_]) else i_}|"
        def switch(m):
            s_, i_ = int(m.group(1)) - 1, int(m.group(3))
            return f"|switch|p{s_ + 1}{m.group(2)}|{names[s_][i_] if i_ < len(names[s_]) else i_}"
        out = []
        for ln in b.log():
            if ln.startswith("|switch|"):
                out.append(re.sub(r"^\|switch\|p([12])([ab])\|(\d+)", switch, ln))
            else:
                out.append(re.sub(r"\|([01])\|(\d)\|", name, ln if ln.endswith("|") else ln + "|", count=2).rstrip("|"))
        if note:
            out.append(note)
        return out

    def describe(self, b, side, slot, a):
        """an action tuple in the file's own words"""
        sd = b.state()["sides"][side]
        if a[0] == "pass":
            return "pass"
        if a[0] == "switch":
            return f"switch {sd['mons'][a[1]]['species']}"
        idx = sd["active"][slot]
        moves = sd["mons"][idx]["moves"] if idx is not None else []
        s = moves[a[1]][0] if a[1] < len(moves) else f"move {a[1] + 1}"
        if a[2]:
            s += " " + self._target_names(side, a[2])
        if a[3]:
            s += " mega"
        return s


def load(path):
    with open(path, encoding="utf-8") as f:
        return Situation(f.read(), name=os.path.basename(path))


# -------------------------------------------------------------------------------------------------------- outcomes

def outcome(b):
    st = b.state()
    o = {"winner": st["winner"], "turn": st["turn"], "weather": st["field"]["weather"], "terrain": st["field"]["terrain"],
         "mons": []}
    for s, sd in enumerate(st["sides"]):
        act = {a for a in sd["active"] if a is not None}
        for i, m in enumerate(sd["mons"]):
            dead = m["fainted"] or m["hp"] <= 0
            o["mons"].append({"side": s, "idx": i, "species": m["set_species"], "hp": 0.0 if dead else m["hp"] / m["maxhp"],
                              "fainted": dead, "status": "" if dead else m["status"], "active": i in act and not dead,
                              "boosts": {k: v for k, v in m["boosts"].items() if v and not dead}, "item": m["item"]})
    return o


def summary(rows):
    n = len(rows)
    out = [f"{n} games"]
    stopped = [r["stopped"] for r in rows if r.get("stopped")]
    if stopped:
        out.append(f"  the script stopped fitting in {len(stopped)} games (they end there; e.g. turn line {stopped[0][0]}: "
                   f"{stopped[0][1]})")
    wins = [sum(1 for r in rows if r["winner"] == w) for w in (0, 1)]
    if any(r["winner"] is not None for r in rows):
        out.append(f"  ended: p1 won {wins[0]}, p2 won {wins[1]}, still going {sum(1 for r in rows if r['winner'] is None)}")
    out.append(f"  {'Pokemon':<26}{'HP left (mean)':>15}{'fainted':>9}{'on field':>10}   status / stat stages / item")
    for k, m0 in enumerate(rows[0]["mons"]):
        ms = [r["mons"][k] for r in rows]
        hp = sum(m["hp"] for m in ms) / n
        fa = sum(m["fainted"] for m in ms) / n
        on = sum(m["active"] for m in ms) / n
        extra = []
        for stt in ("brn", "par", "psn", "tox", "slp", "frz"):
            c = sum(m["status"] == stt for m in ms)
            if c:
                extra.append(f"{stt} {100 * c / n:.0f}%")
        boosts = {}
        for m in ms:
            for b, v in m["boosts"].items():
                boosts.setdefault(b, []).append(v)
        for b, vs in boosts.items():
            extra.append(f"{b} {sum(vs) / n:+.2f}")
        items = {}
        for m in ms:
            items[m["item"]] = items.get(m["item"], 0) + 1
        if len(items) > 1 or "" in items:
            extra.append("item " + ", ".join(f"{it or 'none'} {100 * c / n:.0f}%" for it, c in
                                             sorted(items.items(), key=lambda x: -x[1])))
        label = f"p{m0['side'] + 1} {m0['species']}"
        out.append(f"  {label:<26}{100 * hp:>14.1f}%{100 * fa:>8.0f}%{100 * on:>9.0f}%   {'; '.join(extra)}")
    ws = {}
    for r in rows:
        key = (r["weather"] or "-") + " / " + (r["terrain"] or "-")
        ws[key] = ws.get(key, 0) + 1
    out.append("  weather / terrain at the end: " + ", ".join(f"{k} {100 * v / n:.0f}%" for k, v in sorted(ws.items(), key=lambda x: -x[1])))
    return "\n".join(out)


# ------------------------------------------------------------------------------------------------ Showdown comparison

def showdown_compare(sit, seeds, showdown_dir, show_log=False):
    """play the same situation in real Showdown (tools/showdown_truth.js) and compare the end states. Needs Node.js and a
    built Pokemon Showdown checkout (node build). Returns a dict: diffs (feature, engine mean, Showdown mean),
    off_script_engine / illegal_showdown, log (Showdown's log of game 1 or None), turns."""
    os.environ["SHOWDOWN_DIR"] = os.path.abspath(showdown_dir)
    sys.path.insert(0, os.path.join(ROOT, "tools"))
    import engine_diff as E
    import multi_turn_truth as M
    E.init((), True)
    texts = ["\n\n".join(s) for s in sit.sets]
    # the harness mirrors set_mon / set_field / set_side / set_extra / set_ability; settings it can't mirror are left out
    # (listed in sit.notes, printed by the CLI)
    calls = []
    for m, a, kw in sit.calls:
        if m not in ("set_mon", "set_field", "set_side", "set_extra", "set_ability"):
            continue
        kw = {k: v for k, v in kw.items() if k not in ("choice_lock", "status_turns")}
        if kw:
            calls.append((m, list(a), kw))
    # Showdown gets the choices as fixed text, so 'random' actions are drawn once (by one engine game) and then used in
    # every game of both engines; the turns are played as long as that game lasted
    base = sit.battle()
    probe = base.copy()
    probe.reseed(11)
    joint = sit.play(probe, rng=random.Random(11))
    sd_turns, fast_turns = [], []
    for t in joint:
        fast_turns.append(t)
        parts = []
        for side in range(2):
            parts.append(", ".join(_sd_choice(a) for a in t[side]))
        sd_turns.append(parts)
    # the engine side: the same fixed choices; one that is illegal in a game (the dice changed the board) is replaced by
    # Showdown's 'default' (first move of each Pokemon), as the harness does in Showdown; forced replacements in both
    # engines = first healthy bench Pokemon in team order
    rows_fast, off_fast = [], 0
    for k in range(seeds):
        c = base.copy()
        c.reseed(11 + k * 1000003)
        off = False
        for acts in fast_turns:
            if c.phase == "ended":
                break
            use = []
            for side in range(2):
                la = c.legal_actions(side)
                pair = []
                for slot in range(2):
                    opts = la[slot] if slot < len(la) else []
                    a = tuple(acts[side][slot])
                    if not opts:
                        a = ("pass", 0, 0, False)
                    elif a not in opts:
                        off = True
                        a = next((x for x in opts if x[0] == "move" and not x[3]), opts[0])
                    pair.append(a)
                use.append(pair)
            t0 = c.turn
            c.choose(0, *use[0])
            c.choose(1, *use[1])
            M.resolve(c, t0)
        off_fast += off
        rows_fast.append(M.finish(E.outcome(c, "old"), c.phase == "ended"))
    req = {"id": 0, "texts": texts, "turn": sit.turn, "calls": [[n, list(a), kw] for n, a, kw in calls],
           "choices": sd_turns[0], "script": sd_turns[1:], "seeds": seeds, "seed0": 11, "turns": len(sd_turns),
           "log": 1 if show_log else 0}
    resp = E.truth_request(req)
    if resp.get("error"):
        raise SituationError("Showdown: " + resp["error"])
    rows_sd = [M.finish(r, r.get("ended", 0) == 1) for r in resp["rows"]]
    diffs = E.compare(rows_fast, rows_sd)
    log = None
    if show_log and resp.get("logs"):
        log = [l for l in resp["logs"][0] if not l.startswith(("|t:", "|split", "|uhtml"))]
    return {"diffs": diffs, "illegal_showdown": resp.get("illegal", 0), "off_script_engine": off_fast, "log": log,
            "turns": len(sd_turns)}


def _sd_choice(a):
    if a[0] == "pass":
        return "pass"
    if a[0] == "switch":
        return f"switch m{a[1]}"                        # Showdown matches the harness' nickname m<team index>
    return f"move {a[1] + 1}" + (f" {a[2]}" if a[2] else "") + (" mega" if a[3] else "")


# ------------------------------------------------------------------------------------------------------------- CLI

def main():
    ap = argparse.ArgumentParser(description="Set up a battle situation and play it (engine; optionally real Showdown too).")
    ap.add_argument("file", help="situation file (see docs/situations.md)")
    ap.add_argument("--seeds", type=int, default=200, help="how many games to play (default 200)")
    ap.add_argument("--logs", type=int, default=1, help="print the battle log of this many games (default 1)")
    ap.add_argument("--state", action="store_true", help="print the starting position's full state and stop")
    ap.add_argument("--showdown", metavar="DIR", help="also play it in real Showdown (path to a built pokemon-showdown checkout)")
    a = ap.parse_args()
    try:
        sit = load(a.file)
        b = sit.battle()
        if a.state:
            import json
            print(json.dumps(b.state(), indent=1, default=str))
            return
        print(_position(sit, b))
        if not sit.turns:
            print("\n(no [turns] section: nothing to play)")
            return
        for k in range(a.logs):
            print(f"\n---- game {k + 1} ----")
            print("\n".join(sit.log(seed=11 + k * 1000003)))
        print()
        print(summary(sit.run(seeds=a.seeds)))
        if a.showdown:
            if sit.notes:
                print("\nnote: not mirrored in Showdown (the comparison may differ because of these): " + ", ".join(sit.notes))
            if any(t.lower().startswith("replace") for t in sit.turns):
                print("\nnote: 'replace' lines aren't mirrored in the comparison (both engines bring in the first healthy bench Pokemon)")
            r = showdown_compare(sit, a.seeds, a.showdown, show_log=a.logs > 0)
            if r["log"]:
                print("\n---- Showdown, game 1 ----")
                print("\n".join(r["log"]))
            print(f"\nShowdown comparison ({a.seeds} games each, {r['turns']} turns):",
                  "SAME (no difference beyond chance)" if not r["diffs"] else "DIFFERENT")
            if r["diffs"]:
                print(f"    {'end-state feature':<22}{'engine':>9}{'Showdown':>10}   (sNmK = side N+1, team position K+1)")
                for d in r["diffs"][:20]:
                    print(f"    {d[0]:<22}{d[1]:>9.3f}{d[2]:>10.3f}")
            if r["off_script_engine"] or r["illegal_showdown"]:
                print(f"    went off script (a scripted choice was illegal in that game, so the default move was played): "
                      f"engine {r['off_script_engine']} games, Showdown {r['illegal_showdown']} choices")
    except SituationError as e:
        print(f"error: {e}", file=sys.stderr)
        sys.exit(2)


def _position(sit, b):
    st = b.state()
    lines = [f"{sit.name}: turn {st['turn']}"]
    f = st["field"]
    fl = []
    if f["weather"]:
        fl.append(f"{f['weather']} ({f['weather_turns']} turns)")
    if f["terrain"]:
        fl.append(f"{f['terrain']} ({f['terrain_turns']} turns)")
    if f["trick_room"]:
        fl.append(f"Trick Room ({f['trick_room']} turns)")
    for k in ("gravity", "magic_room", "wonder_room", "fairy_lock"):
        if f[k]:
            fl.append(f"{k} ({f[k]} turns)")
    lines.append("  field: " + (", ".join(fl) or "nothing"))
    for s, sd in enumerate(st["sides"]):
        conds = [f"{k} {sd[k]}" for k in ("tailwind", "reflect", "light_screen", "aurora_veil", "safeguard") if sd[k]]
        conds += [k for k in ("stealth_rock", "sticky_web") if sd[k]]
        conds += [f"{k} {sd[k]}" for k in ("spikes", "toxic_spikes") if sd[k]]
        lines.append(f"  p{s + 1}" + (": " + ", ".join(conds) if conds else ""))
        for i, m in enumerate(sd["mons"]):
            where = f"p{s + 1}{'ab'[sd['active'].index(i)]}" if i in sd["active"] else f"bench {i + 1}"
            bits = [f"{m['hp']}/{m['maxhp']} HP"]
            if m["fainted"]:
                bits = ["fainted"]
            if m["status"]:
                bits.append(m["status"])
            bits += [f"{k} {v:+d}" for k, v in m["boosts"].items() if v]
            bits.append(f"item {m['item'] or 'none'}")
            bits.append(f"ability {m['ability']}")
            lines.append(f"    {where:<8} {m['species']:<22} " + ", ".join(bits))
    return "\n".join(lines)


if __name__ == "__main__":
    main()
