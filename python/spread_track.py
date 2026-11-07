"""Live opponent spread / item / ability inference from a real battle log (2026-10-19; spread_infer.Belief per opponent).

At every decision `Tracker.update(bot, battle)`:
  1. reads the log events since the previous decision,
  2. turns the CLEAN ones into observations, using the position the bot rebuilt at that previous decision
     (bridge.build_engine) as their context,
  3. rebuilds the context for the next turn.

Clean = nothing earlier in that turn changed what damage / speed depend on (stat stages, weather, terrain, screens, items,
abilities, statuses, switches, Mega Evolution, Helping Hand...: tracked per Pokemon / side / field, see MON_DIRTY), and nothing during the hit itself that the
damage rolls don't cover (crit, multi-hit, Focus Sash / Sturdy / Disguise, resist berries, Substitute...). Everything else
is skipped, so nothing false is 'proven'. Our hits give their bulk (their HP shown as a percentage), their hits on us give
their attack (exact damage), move order in one priority bracket gives their Speed.
"""

import re
import spread_infer as si
from logmon import log_mon

# events that change a damage / speed context for the rest of the turn, by what they touch
GLOBAL_DIRTY = {"-fieldstart", "-fieldend", "-swapsideconditions", "-clearallboost"}      # + '-weather' unless [upkeep]
SIDE_DIRTY = {"-sidestart", "-sideend", "-cureteam"}                                       # the side named in ev[2]
MON_DIRTY = {"-boost", "-unboost", "-setboost", "-clearboost", "-clearnegativeboost", "-clearpositiveboost",
             "-swapboost", "-copyboost", "-invertboost", "-item", "-enditem", "-ability", "-endability", "switch", "drag",
             "replace", "detailschange", "-formechange", "-mega", "-status", "-curestatus", "-start", "-end", "-transform",
             "-singlemove", "-prepare", "-burst", "-primal", "-terastallize", "-sethp", "-zpower", "-zbroken"}
# the second Pokemon named (ev[3]) is touched too
MON_DIRTY_OF = {"-swapboost", "-copyboost", "-transform"}
# -singleturn / -activate effects that change nothing damage or speed depend on (blocks, redirection)
HARMLESS_EFFECTS = {"protect", "detect", "kingsshield", "spikyshield", "banefulbunker", "silktrap", "burningbulwark",
                    "obstruct", "maxguard", "followme", "ragepowder", "spotlight", "endure", "wideguard", "quickguard",
                    "craftyshield", "matblock"}
# hidden partner abilities that change damage and that the rebuilt position may lack (unrevealed)
GUARD_DEFENDER_PARTNER = {"friendguard"}
GUARD_ATTACKER_PARTNER = {"battery", "powerspot", "steelyspirit", "flowergift"}
GUARD_ANY = {"fairyaura", "darkaura", "aurabreak"}
# events inside one action that make its hits unusable (the damage isn't the plain roll)
BAD_IN_ACTION = {"-crit", "-hitcount", "-enditem", "-activate", "-item", "-ability", "-endability", "-sethp",
                 "-start", "-end", "-transform", "-block", "-fail", "-notarget", "-formechange", "detailschange"}
PRIORITY_ABILITIES = {"prankster", "galewings", "triage", "quickdraw", "stall", "myceliummight"}
ORDER_ITEMS = {"quickclaw", "custapberry", "laggingtail", "fullincense"}


def to_id(s):
    return re.sub(r"[^a-z0-9]", "", str(s).lower())


def parse_shown_hp(s):
    """'54/100y' (Champions' shared HP) -> spread_infer.display code; '0 fnt' -> 0; None if not a percentage."""
    s = str(s).split()[0] if s else ""
    if s.startswith("0"):
        return 0
    m = re.match(r"^(\d+)/100([gyr]?)$", s)
    if not m:
        return None
    pct, c = int(m.group(1)), m.group(2)
    col = 0
    if pct == 20:
        col = 1 if c == "y" else 2
    elif pct == 50:
        col = 3 if c == "g" else 1
    return pct * 4 + col


def parse_exact_hp(s):
    s = str(s).split()[0] if s else ""
    if s.startswith("0"):
        return 0
    m = re.match(r"^(\d+)/(\d+)$", s)
    return int(m.group(1)) if m else None


class Tracker:
    def __init__(self):
        self.beliefs = {}           # id(their poke-env Pokemon) -> Belief
        self.mons = {}              # id -> poke-env Pokemon
        self.seen = 0               # events already read
        self.ctx = None             # context built at the previous decision
        self.stats = {"our_hit": 0, "their_hit": 0, "order": 0, "skipped_actions": 0, "dirty_turns": 0,
                      "contradictions": 0}

    # ------------------------------------------------------------ public
    def update(self, bot, battle):
        evs = list(getattr(battle, "_replay_data", []) or [])
        new, self.seen = evs[self.seen:], len(evs)
        if self.ctx is not None and new:
            try:
                self._process(battle, new)
            except Exception as e:                          # never break the bot's turn
                self.stats["errors"] = self.stats.get("errors", 0) + 1
                self.stats["last_error"] = repr(e)
        try:
            self.ctx = self._context(bot, battle)
        except Exception as e:
            self.ctx = None
            self.stats["errors"] = self.stats.get("errors", 0) + 1
            self.stats["last_error"] = repr(e)

    # ------------------------------------------------------------ context
    def _context(self, bot, battle):
        eb, index, our_slots, their_index, theirs, *_ = bot.build_engine(battle)
        st = eb.state()
        role = battle.player_role
        ctx = {"eb": eb, "st": st, "ours": {id(m): k for m, k in ((m, index.get(id(m))) for m in battle.team.values())
                                            if k is not None},
               "theirs": dict(their_index), "role": role, "turn": battle.turn,
               "tr": bool(st["field"].get("trick_room")), "shown0": {}}
        for f in theirs:
            self.mons[id(f)] = f
            pct = getattr(f, "current_hp", None)
            # their HP as shown (percent); the colour at 20 / 50 isn't kept by poke-env: unknown there
            ctx["shown0"][id(f)] = (0 if f.fainted else (int(pct) * 4 if pct not in (None, 20, 50) else None))
            k = their_index[id(f)]
            em = st["sides"][1]["mons"][k]
            if id(f) not in self.beliefs:
                try:
                    self.beliefs[id(f)] = si.Belief(self._team_species(f, em))
                except Exception:
                    continue
            self._restrict(self.beliefs[id(f)], f, em)
        return ctx

    @staticmethod
    def _team_species(f, em):
        sp = to_id(em.get("set_species") or em["species"])
        dj = si.dexjson()["species"]
        d = dj.get(sp)
        if d and d.get("isMega"):
            sp = to_id(d["baseSpecies"])
        return sp

    @staticmethod
    def _restrict(b, f, em):
        """Facts the battle revealed (Mega Evolution, a shown item / ability) narrow the hypotheses."""
        b.__dict__.setdefault("notes", [])
        tag = (em["species"], em.get("is_mega"), getattr(f, "item", None), getattr(f, "ability", None))
        if not b.notes or b.notes[-1] != tag:
            b.notes.append(tag)
        if em.get("is_mega"):
            # Mega Evolved: the item is proven to be its stone; its ability now is the form's (the base form's ability
            # hypotheses stay as they are: observations in Mega form are evaluated with the Mega's ability)
            stone = si.mega_stone(em["species"])
            if stone in b.items:
                _only(b, item=stone)
            return
        it = getattr(f, "item", None)
        if it and it not in ("unknown_item",) and it in b.items:
            _only(b, item=it)
        ab = getattr(f, "ability", None)
        if ab and to_id(ab) in b.abilities and len(b.abilities) > 1:
            _only(b, ability=to_id(ab))

    # ------------------------------------------------------------ the turn's events
    def _process(self, battle, evs):
        """Split the turn after the context position into actions; record, before each action, what has stopped matching
        the context so far: everything (weather, terrain...), a side (screens, Tailwind), single Pokemon (stages, items...)."""
        dg, dside, dmon = False, set(), set()
        actions = []            # [(move event, [events of that action]), (global, sides, mons) dirty before it]
        cur = None

        def mon(x):
            m = log_mon(battle, x) if x else None
            if m is not None:
                dmon.add(id(m))

        for ev in evs:
            if len(ev) < 2:
                continue
            kind = ev[1]
            if kind == "turn":
                break                                    # only the turn that followed the context position
            if kind == "move":
                cur = [ev, []]
                actions.append((cur, (dg, frozenset(dside), frozenset(dmon))))
                continue
            if cur is not None:
                cur[1].append(ev)
            rest = "|".join(str(x) for x in ev[2:])
            if kind in GLOBAL_DIRTY or (kind == "-weather" and "[upkeep]" not in rest):
                dg = True
            elif kind in SIDE_DIRTY and len(ev) > 2:
                dside.add(str(ev[2])[:2])
            elif kind in MON_DIRTY and len(ev) > 2:
                mon(ev[2])
                if kind in MON_DIRTY_OF and len(ev) > 3 and re.match(r"^p\d[ab]?: ", str(ev[3])):
                    mon(ev[3])
            elif kind == "swap" and len(ev) > 2:                  # Ally Switch: both Pokemon of the side
                side_ours = str(ev[2])[:2] == battle.player_role
                for m in (battle.active_pokemon if side_ours else battle.opponent_active_pokemon):
                    if m is not None:
                        dmon.add(id(m))
                mon(ev[2])
            elif kind in ("-singleturn", "-activate") and len(ev) > 3:
                eff = to_id(re.sub(r"^(move|ability|item):\s*", "", str(ev[3])))
                if eff not in HARMLESS_EFFECTS:
                    mon(ev[2])
            self.stats.setdefault("kinds", {})
            self.stats["kinds"][kind] = self.stats["kinds"].get(kind, 0) + 1
        if dg or dside or dmon:
            self.stats["dirty_turns"] += 1
        self._use(battle, actions)

    def _risky(self, j, guard):
        """Could their engine mon j hold an unrevealed ability from `guard` that the rebuilt position doesn't give it?"""
        ctx = self.ctx
        em = ctx["st"]["sides"][1]["mons"][j]
        if em.get("fainted") or em.get("hp", 1) <= 0:
            return False
        if em.get("is_mega"):
            return False                                  # the Mega's ability is known and in the position
        inv = {v: p for p, v in ctx["theirs"].items()}
        bp = self.beliefs.get(inv.get(j))
        poss = set(bp.abilities) if bp is not None else {em.get("ability")}
        return bool(poss & guard) and not (len(poss) == 1 and em.get("ability") in poss)

    def _partner_risk(self, k, guard):
        """Could the active partner of their engine mon k hold an unrevealed ability from `guard`?"""
        act = self.ctx["st"]["sides"][1].get("active") or []
        return any(j is not None and j != k and self._risky(j, guard) for j in act)

    def _use(self, battle, actions):
        ctx, role = self.ctx, self.ctx["role"]
        eb, st = ctx["eb"], ctx["st"]
        foe = "p2" if role == "p1" else "p1"
        aura = any(j is not None and self._risky(j, GUARD_ANY) for j in (st["sides"][1].get("active") or []))
        # HP before each hit: replay the turn's HP lines in order
        shown = dict(ctx["shown0"])
        exact = {pid: st["sides"][0]["mons"][k]["hp"] for pid, k in ctx["ours"].items()}
        order_seen = []
        for (cur, (dg, dside, dmon)) in actions:
            mv, evs = cur
            src = mv[2] if len(mv) > 2 else ""
            attacker = log_mon(battle, src)
            if attacker is None:
                continue
            ours = src[:2] == role
            mid = to_id(mv[3]) if len(mv) > 3 else ""
            flags = "|".join(str(x) for x in mv[4:])
            called = "[from]" in flags and "lockedmove" not in flags.replace(" ", "")
            bad = any(e[1] in BAD_IN_ACTION for e in evs) or called or "[still]" in flags or "[zeffect]" in flags or aura
            spread = "[spread]" in flags
            hits = [e for e in evs if e[1] == "-damage" and len(e) > 3 and not any("[from]" in str(x) for x in e[4:])]
            pid_a = id(attacker)
            clean_a = not dg and pid_a not in dmon
            if not called and clean_a:
                if not ours and pid_a in ctx["theirs"] and foe not in dside:
                    order_seen.append((pid_a, mid, False, dg, dside, dmon))
                elif ours and pid_a in ctx["ours"] and role not in dside:
                    order_seen.append((pid_a, mid, True, dg, dside, dmon))
            for e in evs:                               # HP bookkeeping (before -> after for every line)
                if e[1] in ("-damage", "-heal") and len(e) > 3:
                    t = log_mon(battle, e[2])
                    if t is None:
                        continue
                    if e[2][:2] == role:
                        new = parse_exact_hp(e[3])
                        before = exact.get(id(t))
                        if e in hits and not bad and clean_a and id(t) not in dmon and role not in dside and not ours \
                                and before is not None and new is not None and id(t) in ctx["ours"] \
                                and pid_a in ctx["theirs"]:
                            if self._partner_risk(ctx["theirs"][pid_a], GUARD_ATTACKER_PARTNER):
                                self.stats["partner_skips"] = self.stats.get("partner_skips", 0) + 1
                            else:
                                self._their_hit(pid_a, mid, id(t), before - new, before, spread)
                        exact[id(t)] = new
                    else:
                        new = parse_shown_hp(e[3])
                        before = shown.get(id(t))
                        if e in hits and not bad and clean_a and id(t) not in dmon and foe not in dside and ours \
                                and before is not None and new is not None and id(t) in ctx["theirs"] \
                                and pid_a in ctx["ours"]:
                            if self._partner_risk(ctx["theirs"][id(t)], GUARD_DEFENDER_PARTNER):
                                self.stats["partner_skips"] = self.stats.get("partner_skips", 0) + 1
                            else:
                                self._our_hit(pid_a, mid, id(t), before, new, spread)
                        shown[id(t)] = new
            if bad and (hits or ours):
                self.stats["skipped_actions"] += 1
        self._orders(battle, order_seen)

    def _note(self, b, ok, em, mid):
        if ok:
            return
        self.stats["contradictions"] += 1
        f = self.ctx["st"]["field"]
        b.contradictions[-1] = list(b.contradictions[-1]) + [{
            "turn": self.ctx["turn"], "form": em["species"], "engine_item": em["item"], "engine_ability": em["ability"],
            "boosts": {k: v for k, v in em["boosts"].items() if v}, "status": em["status"], "mid": mid,
            "weather": f.get("weather"), "terrain": f.get("terrain")}]

    # ------------------------------------------------------------ observations
    def _slot_of(self, side, k, mid):
        for j, (m_, _pp) in enumerate(self.ctx["st"]["sides"][side]["mons"][k]["moves"]):
            if m_ == mid:
                return j
        return None

    def _our_hit(self, att_pid, mid, tgt_pid, before, after, spread):
        ctx = self.ctx
        a, t = ctx["ours"][att_pid], ctx["theirs"][tgt_pid]
        slot = self._slot_of(0, a, mid)
        b = self.beliefs.get(tgt_pid)
        if slot is None or b is None:
            return
        em = ctx["st"]["sides"][1]["mons"][t]
        ok = b.our_hit(ctx["eb"], (0, a), slot, (1, t), before, after, spread, form=em["species"],
                       fixed_ability=em["ability"] if em.get("is_mega") else None)
        self.stats["our_hit"] += 1
        self._note(b, ok, em, mid)

    def _their_hit(self, att_pid, mid, tgt_pid, damage, hp_before, spread):
        ctx = self.ctx
        a, t = ctx["theirs"][att_pid], ctx["ours"][tgt_pid]
        slot = self._slot_of(1, a, mid)
        b = self.beliefs.get(att_pid)
        if slot is None or b is None:
            return                                    # the move isn't in the rebuilt set (unrevealed / filler)
        em = ctx["st"]["sides"][1]["mons"][a]
        ok = b.their_hit(ctx["eb"], (1, a), slot, (0, t), damage, hp_before, spread, form=em["species"],
                         fixed_ability=em["ability"] if em.get("is_mega") else None)
        if ok is None:
            return
        self.stats["their_hit"] += 1
        self._note(b, ok, em, mid)

    def _orders(self, battle, seq):
        """Pairs (their mover before / after our mover) inside one priority bracket, before anything dirty."""
        ctx = self.ctx
        dj = si.dexjson()["moves"]
        st = ctx["st"]
        for i, (p1, m1, o1, *_d1) in enumerate(seq):
            for (p2, m2, o2, dg2, ds2, dm2) in seq[i + 1:]:
                if o1 == o2:
                    continue
                if dg2 or p1 in dm2 or p2 in dm2 or ds2:          # something changed before the later mover acted
                    continue
                if dj.get(m1, {}).get("priority", 0) != dj.get(m2, {}).get("priority", 0):
                    continue
                their_pid, our_pid, they_first = (p1, p2, True) if not o1 else (p2, p1, False)
                their_mid, our_mid = (m1, m2) if not o1 else (m2, m1)
                k_t, k_o = ctx["theirs"][their_pid], ctx["ours"][our_pid]
                tm, om = st["sides"][1]["mons"][k_t], st["sides"][0]["mons"][k_o]
                b = self.beliefs.get(their_pid)
                if b is None:
                    continue
                # the real priority may differ from the move's listed one: skip (no false Speed proof)
                their_abs = {tm["ability"]} if tm.get("is_mega") else set(b.abilities)
                if (_may_shift(our_mid, {om.get("ability")}, dj) or _may_shift(their_mid, their_abs, dj)
                        or om.get("item") in ORDER_ITEMS):
                    continue
                other = ctx["eb"].speed(0, k_o)
                ok = b.order(ctx["eb"], (1, k_t), other, they_first, ctx["tr"], form=tm["species"],
                             fixed_ability=tm["ability"] if tm.get("is_mega") else None)
                self.stats["order"] += 1
                self._note(b, ok, tm, m1 if not o1 else m2)


def _may_shift(mid, abilities, dj):
    """Could this move's priority (or the user's place in its bracket) differ from the move data's priority?"""
    md = dj.get(mid, {})
    if "onModifyPriority" in (md.get("hooks") or []):           # Grassy Glide in Grassy Terrain
        return True
    for ab in abilities:
        if ab in ("quickdraw", "stall", "myceliummight"):
            return True
        if ab == "prankster" and md.get("category") == "Status":
            return True
        if ab == "galewings" and md.get("type") == "Flying":
            return True
        if ab == "triage" and (md.get("flags") or {}).get("heal"):
            return True
    return False


def _only(b, item=None, ability=None):
    """Narrow a Belief to a revealed item / ability (keeps the likelihood history)."""
    if item is not None and item in b.items:
        if item in b.special:
            b.blocks = {k: v for k, v in b.blocks.items() if k[0] == item}
            b.special = {item}
        else:
            b.blocks = {k: v for k, v in b.blocks.items() if k[0] is None}
            b.special = set()
        b.items = [item]
    if ability is not None and ability in b.abilities:
        b.blocks = {k: v for k, v in b.blocks.items() if k[1] == ability}
        b.abilities = [ability]
