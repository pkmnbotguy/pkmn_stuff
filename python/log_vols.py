"""
(2026-10-18) Volatiles, slot conditions and field timers a PLAYER can infer from the battle log, in the same Showdown-style
format bridge_vols.apply writes into a rebuilt position.

Durations follow Showdown's conditions (Champions mod where it differs): the value at the start, minus one per end of turn
('|upkeep|') it has been on the field; '-end' removes. Values a player cannot know (confusion length, binding length 5-6,
Outrage's 2-3 turns, Substitute damage) get a best guess; the engine draws confusion's length itself (0x80 | turns elapsed).
"""

from bridge_util import base_id, to_id
from logmon import log_mon

# duration at the start (Showdown); turn-dependent ones are handled in _start
START = {"taunt": 3, "encore": 3, "disable": 4, "throatchop": 2, "healblock": 5, "magnetrise": 5, "yawn": 2, "uproar": 3,
         "syrupbomb": 4, "partiallytrapped": 5, "telekinesis": 3, "embargo": 5, "laserfocus": 2}
BATON_NOCOPY = {"encore", "disable", "torment", "attract", "trapped", "saltcure", "smackdown", "minimize", "defensecurl",
                "stockpile", "syrupbomb", "imprison", "destinybond", "flashfire", "glaiverush", "yawn", "lockon", "__hist"}
FLAGS = {"flashfire", "charge", "focusenergy", "dragoncheer", "noretreat", "aquaring", "saltcure", "powertrick", "smackdown",
         "torment", "ingrain", "imprison", "curse", "attract", "leechseed", "minimize", "gastroacid", "glaiverush", "octolock"}
SINGLE_TURN = {"electrify", "roost", "glaiverush"}     # gone at the next turn (glaive rush: after the user's next move)
BIND_MOVES = {"infestation", "whirlpool", "firespin", "sandtomb", "magmastorm", "bind", "wrap", "clamp", "snaptrap",
              "thundercage"}
ABSORB_ABILITIES = {"Flash Fire", "Volt Absorb", "Water Absorb", "Earth Eater", "Dry Skin", "Lightning Rod", "Storm Drain",
                    "Motor Drive", "Sap Sipper", "Well-Baked Body", "Wind Rider", "Eelevate"}
LOCK_MOVES = {"outrage", "thrash", "petaldance", "ragingfury"}
NAME_FIX = {"perish": "perishsong"}


def pre_mega_ability(battle, ident):
    """(2026-10-19 bridge flag 15403) the ability a Transform copied from a Pokemon that had not Mega Evolved yet (the
    copier keeps it after the target Mega Evolves; a rebuilt position knows only the Mega form). Ours: the set's ability
    from the first request (pokeenv_fix); theirs: a revealed ability that isn't the Mega form's own, else the base species'
    first ability. None = unknown."""
    try:
        m = log_mon(battle, ident)
    except Exception:
        return None
    try:
        import pokeenv_fix
        a = pokeenv_fix.set_ability_of(m)
        if a:
            return to_id(a)
    except Exception:
        pass
    try:
        from poke_env.data import GenData
        dex = GenData.from_gen(9).pokedex
    except Exception:
        return None
    entry = dex.get(to_id(m.species)) or {}
    base = dex.get(to_id(entry.get("baseSpecies", m.species))) or entry
    mega_abs = set()
    for f in base.get("otherFormes") or []:
        fe = dex.get(to_id(f)) or {}
        if str(fe.get("forme", "")).startswith("Mega"):
            mega_abs |= {to_id(x) for x in (fe.get("abilities") or {}).values()}
    own = getattr(m, "_ability", None)
    if own and to_id(own) not in mega_abs:
        return to_id(own)
    first = (base.get("abilities") or {}).get("0")
    return to_id(first) if first else None


def _vid(name):
    n = str(name)
    for p in ("move: ", "ability: ", "item: "):
        if n.startswith(p):
            n = n[len(p):]
    return to_id(n)


class Tracker:
    def __init__(self, battle):
        self.b = battle
        self.vols = {}          # (role, base id) -> {vid: state}
        self.pos = {}           # 'p1a' -> (role, base id)
        self.moved = set()      # (role, base id) that acted this turn
        self.slots = {"p1": [{}, {}], "p2": [{}, {}]}
        self.side = {"p1": {}, "p2": {}}
        self.pw = {}
        self.lastitem = {}          # (2026-10-18) key -> item it used / ate last (Showdown lastItem; kept across switches)
        self.last_move = {}     # (role, base id) -> move id
        self.metro_cur = self.metro_move = None
        self.cur_user = None    # the Pokemon whose move is being resolved (sources of trapped / Heal Block...)
        self.cur_slot = None
        self.cur_move = None
        self.turn = 0
        self.lock_check = None
        self.pending = None
        self.as_check = None
        self.me = getattr(battle, "player_role", None)
        self.boosts = {}        # (role, base id) -> stat stages from the log (None = unknown: Psych Up, Spectral Thief...)
        self.weather, self.terrain = "", ""                   # at each Substitute hit (damage estimate)
        self.crit = set()                                      # Pokemon crit by the move being resolved

    def key(self, ident):
        try:
            m = log_mon(self.b, ident)
        except Exception:
            return None
        return str(ident)[:2], base_id(m.species)

    def run(self):
        for ev in getattr(self.b, "_replay_data", []):
            if len(ev) < 2:
                continue
            try:
                self.event(ev)
            except Exception:
                pass
        for kp, (n, real) in (self.__dict__.pop("ta_pend", None) or {}).items():
            if real:
                self._hist(kp)["attacked"] += n            # a move still resolving (mid-turn decision)
        return self

    SCREENS = {"reflect", "lightscreen", "auroraveil"}

    def hit_context(self, ev, kind):
        """(2026-10-19) screens per side and this turn's Helping Hand targets, for Substitute hit estimates (the screens of
        the decision's position are wrong for hits taken before a screen was set or after it ended)."""
        sc = self.__dict__.setdefault("hit_screens", {})
        hh = self.__dict__.setdefault("hit_hh", set())
        if kind in ("-sidestart", "-sideend") and len(ev) > 3:
            c = to_id(str(ev[3]).replace("move:", ""))
            if c in self.SCREENS:
                side = sc.setdefault(str(ev[2])[:2], set())
                (side.add if kind == "-sidestart" else side.discard)(c)
        elif kind == "-swapsideconditions":
            sc["p1"], sc["p2"] = sc.get("p2", set()), sc.get("p1", set())
        elif kind == "-singleturn" and len(ev) > 3 and to_id(str(ev[3]).replace("move:", "")) == "helpinghand":
            k = self.key(ev[2])
            if k:
                hh.add(k)
        elif kind == "turn":
            hh.clear()

    def ability_reset(self, ev, kind):
        """(2026-10-19) An ability change gives the Pokemon a NEW abilityState (Showdown setAbility), so Libero / Protean's
        once-per-switch-in flag is gone: Skill Swap / Wandering Spirit (both Pokemon), an '-ability' line from a move or an
        ability (Entrainment, Role Play, Worry Seed, Simple Beam, Trace, Mummy, Receiver...), Transform, Mega Evolution."""
        ks = []
        if kind == "-activate" and len(ev) > 3 and to_id(str(ev[3]).replace("move:", "").replace("ability:", "")) in ("skillswap", "wanderingspirit"):
            ks.append(self.key(ev[2]))
            ks += [self.key(str(x)[5:].strip()) for x in ev[4:] if str(x).startswith("[of]")]
        elif kind == "-ability" and len(ev) > 3 and any(str(x).startswith(("[from] move:", "[from] ability:")) for x in ev[4:]):
            ks.append(self.key(ev[2]))
        elif kind in ("-transform", "-mega") and len(ev) > 2:
            ks.append(self.key(ev[2]))
        for k in ks:
            if k and "libero" in (self.vols.get(k) or {}):
                del self.vols[k]["libero"]

    def unlink(self, src):
        """Effects that end when their source leaves the field (Showdown linked volatiles: Block / Mean Look / Jaw Lock /
        Spirit Shackle 'trapped', Attract). Octolock is NOT linked: it stays until the next residual, which ends it
        ('-end ... Octolock [silent]') when its source is gone (2026-10-19)."""
        for k, v in self.vols.items():
            for vid in ("trapped", "attract"):
                if vid in v and v[vid].get("src") == src:
                    del v[vid]

    def _v(self, k):
        return self.vols.setdefault(k, {})

    def _hist(self, k):
        return self._v(k).setdefault("__hist", {"used": [], "attacked": 0, "fallen": 0})

    def finish_lock(self):
        """A NEW Outrage-style lock only starts if the move hit something (Showdown's self volatile; Protect / immunity /
        a miss on every target = no lock)."""
        k, self.lock_check = self.lock_check, None
        lm = (self.vols.get(k) or {}).get("lockedmove") if k else None
        if lm and not lm.get("hit"):
            del self.vols[k]["lockedmove"]
        g = (self.vols.get(k) or {}).get("glaiverush") if k else None
        if g is not None and "hit" in g:
            if g.pop("hit"):
                pass
            else:
                del self.vols[k]["glaiverush"]                 # Glaive Rush's self effect needs a hit too (immune target)

    def ate_berry(self, ev, kind):
        """(2026-10-19) Showdown ateBerry (Belch), kept the whole battle: the holder eating ('-enditem|X|..|[eat]'), Bug Bite /
        Pluck ('[from] stealeat' ... '[of] X'), the target of a flung berry ('-enditem|A|..|[from] move: Fling' after A's
        move at X)."""
        ate = self.__dict__.setdefault("ateberry", set())
        abt = self.__dict__.setdefault("ab_target", {})
        if kind == "move" and len(ev) > 2:
            abt.pop(self.key(ev[2]), None)
            if len(ev) > 4 and str(ev[4])[:1] == "p" and not any(str(x) in ("[miss]", "[notarget]") for x in ev[5:]):
                abt[self.key(ev[2])] = self.key(ev[4])
        elif kind in ("-miss", "-immune", "-activate") and len(ev) > 2:
            # (2026-10-19 bridge fuzz) a flung berry that misses / is blocked is used up but nobody eats it
            if kind == "-miss":
                abt.pop(self.key(ev[2]), None)
            else:
                blocked = kind == "-immune" or to_id(str(ev[3]).replace("move:", "")) in self.METRO_BLOCKS if len(ev) > 3 else kind == "-immune"
                if blocked:
                    for u, t in list(abt.items()):
                        if t == self.key(ev[2]):
                            abt.pop(u, None)
        if kind != "-enditem" or len(ev) < 4 or "berry" not in to_id(ev[3]):
            return
        fl = [str(x) for x in ev[4:]]
        k = None
        if "[eat]" in fl:
            k = self.key(ev[2])
        elif "[from] stealeat" in fl:
            of = [x for x in fl if x.startswith("[of] ")]
            k = self.key(of[0][5:]) if of else None
        elif "[from] move: Fling" in fl:
            k = (self.__dict__.get("ab_target") or {}).get(self.key(ev[2]))
        if k:
            ate.add(k)

    def event(self, ev):
        kind = ev[1]
        # (2026-10-19) Cud Chew re-eats its berry ('-activate ... Cud Chew' then '-enditem ... [eat]'): Showdown sets neither
        # lastItem nor usedItemThisTurn for it
        prev_cud, self.cud_key = self.__dict__.get("cud_key"), None
        if kind == "-activate" and len(ev) > 3 and to_id(ev[3]) == "abilitycudchew":
            self.cud_key = self.key(ev[2])
        self.cud_eat = kind == "-enditem" and len(ev) > 2 and prev_cud is not None and self.key(ev[2]) == prev_cud
        self.ate_berry(ev, kind)
        if kind == "-ability" and len(ev) > 3 and to_id(ev[3]) == "supersweetsyrup":
            k_ = self.key(ev[2])                       # (2026-10-19 bridge flag 15161) once per battle (syrupTriggered)
            if k_:
                self.__dict__.setdefault("syrupdone", set()).add(k_)
        self.stat_ops(ev, kind)
        self.turn_flags(ev, kind)
        self.metro_ops(ev, kind)
        self.ability_reset(ev, kind)
        self.hit_context(ev, kind)
        if kind == "-mega" and len(ev) > 2:
            km = self.key(ev[2])
            if km:
                self.__dict__.setdefault("megad", set()).add(km)
        if kind in ("move", "cant", "upkeep", "turn", "switch", "drag", "replace") and self.__dict__.get("ta_pend"):
            for kp, (n, real) in self.ta_pend.items():
                if real:
                    self._hist(kp)["attacked"] += n
            self.ta_pend = {}
        if kind in ("switch", "drag", "replace") and len(ev) > 4:
            ks = self.key(ev[2])
            if ks:                                         # (2026-10-19) status from the condition field ('87/175 par')
                parts = str(ev[4]).split()
                self.__dict__.setdefault("mstatus", {})[ks] = parts[1] if len(parts) > 1 and parts[1] != "fnt" else ""
        if kind in ("switch", "drag", "replace", "-damage", "-heal", "-sethp") and len(ev) > 3 and str(ev[2])[:1] == "p":
            # (2026-10-19 bridge fuzz) HP fraction as last shown: HP-dependent hits on a Substitute (Endeavor, Super Fang,
            # Eruption, Flail...) are estimated with the HP AT the hit, not today's
            ks = self.key(ev[2])
            fld = ev[4] if kind in ("switch", "drag", "replace") and len(ev) > 4 else ev[3]
            try:
                n_, d_ = str(fld).split()[0].split("/")
                if ks:
                    self.__dict__.setdefault("mhp", {})[ks] = int(n_) / max(1, int(d_))
            except (ValueError, IndexError):
                if ks and str(fld).split()[:1] == ["0"]:
                    self.__dict__.setdefault("mhp", {})[ks] = 0.0
        if kind in ("-status", "-curestatus") and len(ev) > 3 and str(ev[2])[:1] == "p":
            ks = self.key(ev[2])
            if ks:
                self.__dict__.setdefault("mstatus", {})[ks] = to_id(ev[3]) if kind == "-status" else ""
        if kind in ("-status", "-curestatus") and len(ev) > 3:
            ks = self.key(ev[2]) if str(ev[2])[:1] == "p" else None
            if ks:
                sl = self.__dict__.setdefault("asleep", set())
                if kind == "-status" and to_id(ev[3]) == "slp":
                    sl.add(ks)
                elif kind == "-curestatus" or to_id(ev[3]) != "slp":
                    sl.discard(ks)
        if kind == "-weather" and len(ev) > 2:
            self.weather = "" if to_id(ev[2]) == "none" else to_id(ev[2])
        elif kind in ("-fieldstart", "-fieldend") and len(ev) > 2 and _vid(ev[2]).endswith("terrain"):
            self.terrain = _vid(ev[2]) if kind == "-fieldstart" else ""
        elif kind == "move":
            self.crit = set()
            if len(ev) > 3:          # (2026-10-18) Showdown battle.lastMove (Copycat): every |move| line, called ones too
                self.pw["__lastmove"] = to_id(ev[3])
        elif kind == "-crit" and len(ev) > 2:
            kc = self.key(ev[2]) if str(ev[2])[:1] == "p" else None
            if kc:
                self.crit.add(kc)
        if kind in ("move", "upkeep", "turn", "cant"):
            self.finish_lock()
            if kind != "cant":
                self.pending = None
        if kind == "turn":
            self.moved = set()
            self.turn_slot = {kk: sl[2:3] for sl, kk in self.pos.items() if kk}   # (2026-10-19) slot when choosing
            try:
                self.turn = int(ev[2])
            except Exception:
                pass
            for v in self.vols.values():
                for x in SINGLE_TURN - {"glaiverush"}:
                    v.pop(x, None)
            return
        if kind == "upkeep":
            for v in self.vols.values():
                v.pop("roost", None)
                v.pop("electrify", None)
            self.residual()
            return
        if kind == "-swapsideconditions":
            self.side["p1"], self.side["p2"] = self.side["p2"], self.side["p1"]   # Court Change (2026-10-18)
            return
        if kind in ("-sidestart", "-sideend") and len(ev) > 3:
            role, cid = str(ev[2])[:2], _vid(ev[3])
            if cid == "safeguard":
                if kind == "-sidestart":
                    self.side[role][cid] = {"duration": 5}
                else:
                    self.side[role].pop(cid, None)
            return
        if kind == "-fieldactivate" and len(ev) > 2 and _vid(ev[2]) == "fairylock":
            self.pw["fairylock"] = {"duration": 2}             # (2026-10-18) this turn's end + the next turn
            return
        if kind in ("-fieldstart", "-fieldend") and len(ev) > 2:
            pid = _vid(ev[2])
            if pid in ("gravity", "magicroom", "wonderroom"):
                if kind == "-fieldstart":
                    self.pw[pid] = {"duration": 5}
                else:
                    self.pw.pop(pid, None)
            return
        if len(ev) < 3:
            return
        ident = ev[2]
        k = self.key(ident) if isinstance(ident, str) and ident[:1] == "p" else None
        if k is None:
            return
        self._track_boost(kind, k, ev)
        if kind == "swap":
            # Ally Switch: the two Pokemon trade slots (slot conditions such as Future Sight / Wish stay with the slot)
            sd_ = str(ident)[:2]
            pa, pb = self.pos.pop(sd_ + "a", None), self.pos.pop(sd_ + "b", None)
            if pa:
                self.pos[sd_ + "b"] = pa
            if pb:
                self.pos[sd_ + "a"] = pb
            return
        if kind in ("switch", "drag", "replace"):
            slot = str(ident).split(":")[0][:3]
            old = self.pos.get(slot)
            baton = None
            if old and old != k and kind == "switch" and any("[from] Baton Pass" in str(x) for x in ev[5:]):
                # Baton Pass hands over every volatile without Showdown's noCopy flag, and the stat stages (2026-10-18)
                baton = ({x: dict(e) if isinstance(e, dict) else e for x, e in (self.vols.get(old) or {}).items()
                          if x not in BATON_NOCOPY}, self.boosts.get(old))
            if old and old != k and kind != "replace":
                ko_ = ((self.vols.get(old) or {}).get("__hist") or {}).get("attacked", 0)                     if old in self.__dict__.get("fainted_keys", set()) else 0
                self.vols.pop(old, None)
                if ko_:
                    self._hist(old)["attacked"] = ko_          # (2026-10-19) the KO hit stays with the fainted Pokemon
            for s_, kk in list(self.pos.items()):
                if kk == k:
                    del self.pos[s_]
            self.pos[slot] = k
            if kind != "replace":
                keep = None
                if k in self.__dict__.get("fainted_keys", set()):
                    # (2026-10-19) Champions clears timesAttacked when a Pokemon LEAVES (faint included); the KO hit is
                    # counted after that, so a Pokemon revived by Revival Blessing comes back with it
                    self.fainted_keys.discard(k)
                    keep = ((self.vols.get(k) or {}).get("__hist") or {}).get("attacked", 0)
                self.vols.pop(k, None)
                if keep:
                    self._hist(k)["attacked"] = keep
                if old and old != k:
                    self.unlink(old)
                if any("Shed Tail" in str(x) for x in ev[5:]) and self.cur_user:
                    self._v(k)["substitute"] = {"hp_from": self.cur_user}   # Shed Tail passes its quarter-HP Substitute
                if baton is not None:
                    self.vols[k] = baton[0]
                    self.boosts[k] = dict(baton[1]) if baton[1] is not None else None
            return
        if kind == "-formechange" and len(ev) > 3:
            try:
                from poke_env.data import GenData
                ts = GenData.from_gen(9).pokedex[to_id(ev[3])]["types"]
                self._v(k)["__types"] = {"types": list(ts), "added": ""}
            except Exception:
                pass
            return
        if kind == "faint":
            self.__dict__.setdefault("fainted_keys", set()).add(k)
            self.vols.pop(k, None)
            self.unlink(k)
            return
        if kind == "move" and len(ev) > 3:
            mid = to_id(ev[3])
            self.cur_user, self.cur_move = k, mid
            self.cur_spread = any(str(x).startswith("[spread]") for x in ev[4:])
            self.cur_slot = str(ident).split(":")[0][:3]
            self.cur_target = self.key(str(ev[4])) if len(ev) > 4 and str(ev[4]).startswith("p") and ":" in str(ev[4]) else None
            # (2026-10-18) '[from] move: Round' = Round moved up by its partner's Round: chosen, not called (PP spent)
            called = any(str(x).startswith("[from]") and str(x) != "[from] move: Round" for x in ev[5:])
            v = self._v(k)
            self.moved.add(k)
            if not called:
                self.last_move[k] = mid
                # (2026-10-18) moves used since switching in (Showdown moveSlot.used, set when PP is spent: Last Resort)
                h = self._hist(k)
                if mid not in h["used"]:
                    h["used"].append(mid)
            if "twoturnmove" in v and v["twoturnmove"].get("move") == mid:
                v.pop(mid, None)
                v.pop("twoturnmove", None)
            v.pop("glaiverush", None)
            v.pop("destinybond", None)                          # Showdown: removed before the user's next move (a
            #                                                     successful Destiny Bond re-adds it: '-singlemove')
            if mid == "noretreat":
                v["noretreat"] = {}                             # (2026-10-19) Champions: a Ghost (no trap) gets it without '-start'
            if mid == "glaiverush":
                v["glaiverush"] = {"hit": False}
                self.lock_check = k
            if mid == "minimize":
                v["minimize"] = {}                              # no -start message: the volatile is silent
            self.as_check = None
            if mid == "allyswitch":
                # (2026-10-18) Ally Switch's own stall counter (Showdown's 'allyswitch' volatile, added in onPrepareHit):
                # used again while it lasts, it works 1 time in counter (then x3, max 729, duration back to 2); a failed
                # roll ('-fail|USER' with nothing after) deletes it, a fainted partner ('-fail|USER|move: Ally Switch') not
                a = v.get("allyswitch")
                if a and a.get("duration", 0) > 0:
                    a["counter"], a["duration"] = min(a.get("counter", 3) * 3, 729), 2
                else:
                    v["allyswitch"] = {"duration": 2, "counter": 3}
                self.as_check = k
            if mid in LOCK_MOVES:
                lm = v.get("lockedmove")
                from_lock = any(str(x).replace(" ", "") == "[from]lockedmove" for x in ev[4:])
                if lm and not from_lock and not called:
                    # a fresh choice of the move: the old lock had ended silently (2-turn lock on a Pokemon already
                    # confused: no fatigue message); Showdown tags every continued lock move '[from]lockedmove'
                    del v["lockedmove"]
                    lm = None
                if lm and lm.get("move") == mid:
                    lm["n"] = lm.get("n", 1) + 1
                    lm["hit"] = False                       # a continued lock is only renewed by a hit (self volatile):
                    self.lock_check = k                     # a miss / Protect / no target ends it after this move
                    if lm["n"] == 2 and "confusion" in v:
                        # already confused: a 2-turn lock ends after this move with no fatigue message, so 2 vs 3 turns
                        # is hidden; keep the lock and let the engine draw its length (0x80 | turns elapsed)
                        lm["duration"] = 2
                    elif lm["n"] == 2:
                        # restarts (duration 2) only if trueDuration was 3; with 2 it ends after this move, with fatigue
                        # confusion ('-start ... confusion|[fatigue]' pops it below)
                        lm["duration"], lm["known"] = 2, 3
                elif not from_lock:
                    # (2026-10-19) also when a caller (Copycat) ran it: Showdown adds the lock (bridge: engine CALL_SLOT)
                    v["lockedmove"] = {"duration": 2, "move": mid, "n": 1, "k": 0, "hit": False}
                    self.lock_check = k
            self.pending = None
            still = any(str(x) == "[still]" for x in ev[4:])
            if mid == "wish":
                j = int(str(ident)[2:3] == "b")
                if "wish" not in self.slots[k[0]][j]:
                    self.slots[k[0]][j]["wish"] = {"duration": 2, "src": k}
                    self.pending = (k[0], j, "wish")
            if mid == "healingwish" and not still:
                # (2026-10-18) the user faints; the wish waits in its slot until a Pokemon that is hurt or statused switches in
                j = int(str(ident)[2:3] == "b")
                self.slots[k[0]][j]["healingwish"] = {}
                self.pending = (k[0], j, "healingwish")
            if mid == "futuresight" and len(ev) > 4 and str(ev[4])[:1] == "p":
                tslot = str(ev[4]).split(":")[0]
                if len(tslot) < 3:
                    # target fainted ('p1: Kangaskhan', no slot letter): the slot it still occupies
                    tk = self.key(str(ev[4]))
                    tslot = next((sl for sl, kk in self.pos.items() if kk == tk), tslot)
                j = int(tslot[2:3] == "b")
                if "futuremove" not in self.slots[tslot[:2]][j]:
                    self.slots[tslot[:2]][j]["futuremove"] = {"duration": 3, "src": k}
                    self.pending = (tslot[:2], j, "futuremove")
            return
        if kind in ("-damage", "-activate", "-end") and self.lock_check and self.lock_check == self.cur_user and k != self.cur_user                 and not any(str(x).startswith("[from]") for x in ev[4:]):
            if kind == "-damage" or (len(ev) > 3 and _vid(ev[3]) == "substitute"):   # (2026-10-19) a broken Substitute ('-end') too
                lm = (self.vols.get(self.lock_check) or {}).get("lockedmove")
                if lm:
                    lm["hit"] = True
                g = (self.vols.get(self.lock_check) or {}).get("glaiverush")
                if g is not None and "hit" in g:
                    g["hit"] = True
        if kind == "-transform" and len(ev) > 3:
            t = self.key(ev[3])
            if t:
                # (2026-10-19) 'pre': the target hadn't Mega Evolved yet (it may later: the copy keeps the base form)
                self._v(k)["__transform"] = {"into": t, "pre": t not in self.__dict__.get("megad", set())}
                if self._v(k)["__transform"]["pre"]:
                    pa = pre_mega_ability(self.b, ev[3])
                    if pa:
                        self._v(k)["__transform"]["pre_ability"] = pa
                # (2026-10-19) Transform copies the target's CURRENT types (Burn Up's typeless, Soak, Forest's Curse...)
                self._v(k).pop("__types", None)
                tt = (self.vols.get(t) or {}).get("__types")
                if tt is not None:
                    self._v(k)["__types"] = {**({"types": list(tt["types"])} if "types" in tt else {}), "added": tt.get("added") or ""}
                h = self._hist(k)                              # Transform: fresh move slots, the target's hit count
                h["used"] = []
                h["attacked"] = ((self.vols.get(t) or {}).get("__hist") or {}).get("attacked", 0)
            return
        if kind == "-heal" and any(str(x) == "[from] move: Healing Wish" for x in ev[4:]):
            sl = str(ev[2]).split(":")[0]
            if len(sl) >= 3 and sl[:2] in self.slots:
                self.slots[sl[:2]][int(sl[2] == "b")].pop("healingwish", None)
        if kind == "-fail" and getattr(self, "as_check", None) == k and len(ev) == 3:
            (self.vols.get(k) or {}).pop("allyswitch", None)
            self.as_check = None
            return
        if kind == "-fail" and self.pending and self.cur_user == k and not any(str(x).startswith("[from]") for x in ev[3:]):
            r, j, cid = self.pending
            self.slots[r][j].pop(cid, None)
            self.pending = None
            return
        if kind == "cant":
            self.moved.add(k)
            v = self._v(k)
            v.pop("destinybond", None)
            v.pop("lockedmove", None)
            if len(ev) > 3 and ev[3] == "recharge":
                v.pop("mustrecharge", None)
            return
        v = self._v(k)
        if kind == "-damage" and any(str(x) == "[from] confusion" for x in ev[4:]):
            v.pop("destinybond", None)                         # (2026-10-18) a confusion self-hit aborts the move: onMoveAborted
        sub_hit = (kind == "-activate" and len(ev) > 3 and "Substitute" in str(ev[3]) and any("[damage]" in str(x) for x in ev[4:]))             or (kind == "-end" and len(ev) > 3 and str(ev[3]).replace("move: ", "") == "Substitute")
        if (kind == "-damage" or sub_hit) and self.cur_user and self.cur_user != k                 and not any(str(x).startswith("[from]") for x in ev[4:]):
            # (2026-10-19) hits taken from others since switching in (Rage Fist). Showdown adds the move's hit count per
            # target after the hit loop only when the LAST hit's damage is a number: a Substitute hit becomes 'true', so a
            # move that only hit the Substitute adds nothing, a multi-hit move that broke it and then hit adds every hit
            pend = self.__dict__.setdefault("ta_pend", {})
            n, _ = pend.get(k, (0, False))
            pend[k] = (n + 1, kind == "-damage")
        if kind == "-enditem" and len(ev) > 3:
            # (2026-10-18) used / eaten by its holder (useItem / eatItem, Fling, gems): Recycle can restore it. Removed by
            # others (Knock Off, Thief, Bug Bite's stealeat, Incinerate, Corrosive Gas) it isn't the holder's lastItem.
            froms = [str(x) for x in ev[4:] if str(x).startswith("[from]")]
            if not self.cud_eat and (not froms or all(f in ("[from] move: Fling", "[from] gem") for f in froms)) and (to_id(ev[3]) != "airballoon" or "[from] move: Fling" in froms):   # (2026-10-19) a FLUNG balloon is lastItem
                self.lastitem[k] = to_id(ev[3])         # a popped Air Balloon is cleared directly (item = ''): no lastItem
        if kind == "-item" and len(ev) > 3:
            if any(str(x) in ("[from] move: Recycle", "[from] ability: Harvest") for x in ev[4:]):
                self.lastitem.pop(k, None)
            elif any(str(x) == "[from] ability: Pickup" for x in ev[4:]):
                it = to_id(ev[3])
                for kk in [kk for kk, v_ in self.lastitem.items() if v_ == it and kk != k][:1]:
                    del self.lastitem[kk]
        if kind == "-enditem" and len(ev) > 3 and any(str(x) == "[from] move: Fling" for x in ev[4:]):
            # (2026-10-18) the flung item is named only after the hit: give it to the Substitute hits of this Fling
            for vv in self.vols.values():
                for h in (vv.get("substitute") or {}).get("hits") or []:
                    if h[0] == k and h[1] == "fling" and len(h) > 11 and h[11] is None:
                        vv["substitute"]["hits"][vv["substitute"]["hits"].index(h)] = h[:11] + (to_id(ev[3]),) + h[12:]
        if kind == "-mustrecharge":
            v["mustrecharge"] = {}
        elif kind == "-prepare" and len(ev) > 3 and not any(str(x) == "[premajor]" for x in ev[4:]):
            # ('[premajor]' = Chilly Reception's priority announcement, not a charge turn)
            mid = to_id(ev[3])
            v["twoturnmove"] = {"move": mid, "duration": 2}
            tl = None
            if len(ev) > 4 and str(ev[4])[:1] == "p":
                t = str(ev[4]).split(":")[0]
                n = 1 if t[2:3] == "a" else 2
                tl = n if t[:2] != k[0] else -n
            if tl is None and k[0] == self.me:
                # our own choice this turn (the agent records it): the log doesn't show the target
                slot = str(ident).split(":")[0][2:3]
                slot = (getattr(self, "turn_slot", None) or {}).get(k, slot)   # chosen before an Ally Switch moved it
                ch = (getattr(self.b, "_bridge_choices", {}).get(self.turn) or {}).get(slot)
                if ch and ch[0] == mid and ch[1]:
                    tl = ch[1]
            v[mid] = {"targetLoc": tl} if tl is not None else {}
        elif kind == "-anim" and len(ev) > 3 and (v.get("twoturnmove") or {}).get("move") == to_id(ev[3]):
            v.pop(to_id(ev[3]), None)                              # charged and released in the same action
            v.pop("twoturnmove", None)
        elif kind == "-singleturn" and len(ev) > 3 and _vid(ev[3]) in ("roost", "electrify"):
            v[_vid(ev[3])] = {}                                    # 1-turn volatiles: gone at the residual (seen by a
            #                                                        mid-turn switch decision, e.g. after U-turn)
        elif kind == "-singlemove" and len(ev) > 3:
            if _vid(ev[3]) == "destinybond":
                v["destinybond"] = {}
        elif kind == "-endability":
            v["gastroacid"] = {}
        elif kind == "-activate" and len(ev) > 3:
            a = _vid(ev[3])
            if a == "gravity":
                # Gravity starting grounds it: Magnet Rise / Telekinesis end, Fly / Bounce are cancelled (Showdown: silent)
                for x in ("magnetrise", "telekinesis"):
                    v.pop(x, None)
                tt = v.get("twoturnmove")
                if tt and tt.get("move") in ("fly", "bounce"):
                    v.pop(tt["move"], None)
                    v.pop("twoturnmove", None)
            if a == "confusion":
                self.moved.add(k)                               # its action is under way (a self-hit uses it up)
                c = v.get("confusion")
                if c is not None:
                    c["elapsed"] = c.get("elapsed", 0) + 1
            elif a == "trapped":
                src = self.last_user
                if self.cur_move == "jawlock" and src == k and getattr(self, "cur_target", None):
                    src = self.cur_target                       # Jaw Lock: user and target each trapped BY THE OTHER
                v["trapped"] = {"src": src} if src else {}
            elif a in BIND_MOVES:
                of = next((x for x in ev[4:] if str(x).startswith("[of] ")), None)
                src = self.key(str(of)[5:]) if of else None
                if "partiallytrapped" not in v:
                    v["partiallytrapped"] = {"e": 0, **({"src": src} if src else {})}
            elif a == "substitute" and any("[damage]" in str(x) for x in ev[4:]):
                # damage to the Substitute: amount hidden -> bridge_vols estimates it with the engine's damage rolls
                sub = v.get("substitute")
                if sub is not None and self.cur_user and self.cur_move:
                    sub.setdefault("hits", []).append((self.cur_user, self.cur_move, bool(getattr(self, "cur_spread", False)),
                                                       dict(self.boosts.get(self.cur_user) or {}), dict(self.boosts.get(k) or {}),
                                                       k in self.crit, self.weather, self.terrain, k in self.moved,
                                                       "roost" in v, self.__dict__.get("mstatus", {}).get(self.cur_user),
                                                       None,   # [11] Fling's item (named after the hit)
                                                       # [12] (2026-10-19) the defender side's screens, Helping Hand on the
                                                       # attacker and the defender's status, all AT the hit
                                                       {"screens": sorted(self.__dict__.get("hit_screens", {}).get(k[0], set())),
                                                        "hh": self.cur_user in self.__dict__.get("hit_hh", set()),
                                                        "tstatus": self.__dict__.get("mstatus", {}).get(k),
                                                        "wonderroom": "wonderroom" in self.pw,
                                                        "hp": [self.__dict__.get("mhp", {}).get(self.cur_user),
                                                               self.__dict__.get("mhp", {}).get(k)]}))
        elif kind == "-start" and len(ev) > 3:
            vid = _vid(ev[3])
            if vid.startswith("fallen") and vid[6:].isdigit():
                self._hist(k)["fallen"] = int(vid[6:])          # Supreme Overlord's count (silent message)
                return
            if vid.startswith("perish") and vid[6:].isdigit():
                v["perishsong"] = {"duration": int(vid[6:])}
                return
            if vid.startswith("stockpile") and vid[9:].isdigit():
                v["stockpile"] = {"layers": int(vid[9:])}
                return
            if vid.startswith("quarkdrive") or vid.startswith("protosynthesis"):
                return
            if any(str(x) == "[upkeep]" for x in ev[4:]):
                return                                          # Uproar's per-turn restatement
            if vid == "confusion" and any(str(x) == "[fatigue]" for x in ev[4:]):
                v.pop("lockedmove", None)
            if vid == "typechange" and len(ev) > 4:
                if str(ev[4]).startswith("[from]"):
                    # Reflect Type: '-start|USER|typechange|[from] move: Reflect Type|[of] TARGET' names no types: the
                    # user takes the target's current types (and its added type)
                    of = next((x for x in ev[5:] if str(x).startswith("[of] ")), None)
                    tk = self.key(str(of)[5:]) if of else None
                    if not tk:
                        return
                    tt = (self.vols.get(tk) or {}).get("__types")
                    if tt and tt.get("types"):
                        v["__types"] = {"types": list(tt["types"]), "added": tt.get("added", "")}
                    elif tt and tt.get("added"):                # (2026-10-19) target with only an added type
                        try:
                            tm = log_mon(self.b, str(of)[5:])
                            v["__types"] = {"types": [x.name.capitalize() for x in tm.types if x is not None],
                                            "added": tt.get("added", "")}
                        except Exception:
                            pass
                    else:
                        try:
                            tm = log_mon(self.b, str(of)[5:])
                            v["__types"] = {"types": [x.name.capitalize() for x in tm.types if x is not None], "added": ""}
                        except Exception:
                            pass
                    return
                t = v.setdefault("__types", {"added": ""})
                t["types"] = [x for x in str(ev[4]).split("/") if x]
                t["added"] = ""                                 # (2026-10-19) Showdown setType clears the added type (Soak...)
                if any(str(x) in ("[from] ability: Libero", "[from] ability: Protean") for x in ev[5:]):
                    v["libero"] = {}                            # once per switch-in
                return
            if vid == "typeadd" and len(ev) > 4:
                t = v.setdefault("__types", {})
                t["added"] = str(ev[4])                         # (2026-10-19) types left out unless changed (see bridge_vols)
                return
            self._start(k, v, vid, ev)
        elif kind == "-end" and len(ev) > 3:
            vid = _vid(ev[3])
            if vid in BIND_MOVES:
                vid = "partiallytrapped"
            if vid.startswith("stockpile"):
                vid = "stockpile"
            if vid == "futuresight":
                # a Future Sight lands here: the hits that follow (a Substitute's '[damage]') come from its user
                sl = str(ident).split(":")[0]
                fm = self.slots[sl[:2]][int(sl[2:3] == "b")].get("futuremove") if len(sl) > 2 else None
                if fm and fm.get("src"):
                    self.cur_user, self.cur_move, self.cur_spread = fm["src"], "futuresight", False
            if vid == "typechange":
                v.pop("__types", None)                          # Mimicry at terrain end: back to the species' own types
            v.pop(NAME_FIX.get(vid, vid), None)
        if kind == "move":
            pass

    @property
    def last_user(self):
        return self.cur_user

    def _start(self, k, v, vid, ev):
        target_moved = k in self.moved
        if vid == "confusion":
            v["confusion"] = {"elapsed": 0}
        elif vid == "taunt":
            v["taunt"] = {"duration": 4 if target_moved else 3}
        elif vid == "encore":
            v["encore"] = {"duration": 3 if not target_moved else 4, "move": self.last_move.get(k)}
        elif vid == "disable":
            # Showdown: 5, one less if the target still moves this turn or is the Pokemon moving now (Cursed Body)
            v["disable"] = {"duration": 4 if (not target_moved or k == self.cur_user) else 5,
                            "move": to_id(ev[4]) if len(ev) > 4 else None}
        elif vid == "healblock":
            psn = any("Psychic Noise" in str(x) for x in ev[4:])
            v["healblock"] = {"duration": 2 if psn or self._last_move_name() == "psychicnoise" else 5}
        elif vid == "substitute":
            v["substitute"] = {"hp": None}
        elif vid in START:
            v[vid] = {"duration": START[vid]}
            if vid == "syrupbomb" and self.cur_user:
                v[vid]["src"] = self.cur_user                  # the Speed drops come from it (an ally too)
        elif vid in FLAGS:
            if vid == "smackdown":
                # Smack Down grounds it: Magnet Rise / Telekinesis end, Fly / Bounce cancelled
                for x in ("magnetrise", "telekinesis"):
                    v.pop(x, None)
                tt = v.get("twoturnmove")
                if tt and tt.get("move") in ("fly", "bounce"):
                    v.pop(tt["move"], None)
                    v.pop("twoturnmove", None)
            e = {}
            if vid in ("attract", "leechseed", "curse", "octolock"):
                of = next((x for x in ev[4:] if str(x).startswith("[of] ")), None)
                e["src"] = self.key(str(of)[5:]) if of else self.cur_user
                if vid == "leechseed" and self.cur_slot:
                    e["slot"] = (self.cur_slot[:2], int(self.cur_slot[2:3] == "b"))
            if vid == "dragoncheer":
                try:
                    m = log_mon(self.b, ev[2])
                    e["hasDragonType"] = any(getattr(t, "name", str(t)).upper() == "DRAGON" for t in m.types if t is not None)
                except Exception:
                    pass
            v[vid] = e

    def _last_move_name(self):
        return self.cur_move

    def residual(self):
        for k, v in self.vols.items():
            for vid in list(v):
                if vid not in v:
                    continue                                        # popped with an expiring twoturnmove (its move entry)
                e = v[vid]
                if vid in ("perishsong",):
                    continue                                        # the log restates the count every turn
                if vid == "partiallytrapped":
                    e["e"] = e.get("e", 0) + 1
                    continue
                if vid == "lockedmove":
                    if k in self.__dict__.get("asleep", set()):
                        del v[vid]                                  # (2026-10-19) lockedmove onResidual: asleep -> no lock
                        continue
                    e["k"] = e.get("k", 0) + 1
                if isinstance(e, dict) and "duration" in e:
                    e["duration"] -= 1
                    if e["duration"] <= 0:
                        del v[vid]
                        if vid == "twoturnmove":
                            v.pop(e.get("move"), None)
        for role in self.slots:
            for sc in self.slots[role]:
                for cid in list(sc):
                    if "duration" not in sc[cid]:
                        continue                                # Healing Wish waits for a Pokemon that needs it
                    sc[cid]["duration"] -= 1
                    if sc[cid]["duration"] <= 0:
                        del sc[cid]
            for cid in list(self.side[role]):
                self.side[role][cid]["duration"] -= 1
                if self.side[role][cid]["duration"] <= 0:
                    del self.side[role][cid]
        for pid in list(self.pw):
            if pid.startswith("__"):                          # __lastmove: not a timer
                continue
            self.pw[pid]["duration"] -= 1
            if self.pw[pid]["duration"] <= 0:
                del self.pw[pid]

    def _track_boost(self, kind, k, ev):
        """Stat stages from the log, so damage estimated later (a Substitute's hidden HP) uses the stages AT the hit."""
        if kind in ("switch", "drag"):
            self.boosts[k] = {}
            return
        if kind == "-copyboost" and len(ev) > 3:
            # Psych Up: the user's crit volatiles are replaced by the target's (2026-10-19: also when the user's stages are
            # already unknown, which returned early below and kept a Dragon Cheer the target didn't have)
            try:
                src, tgt = self.vols.setdefault(k, {}), self.vols.get(self.key(ev[3])) or {}
                for x in ("dragoncheer", "focusenergy"):
                    src.pop(x, None)
                for x in ("dragoncheer", "focusenergy"):
                    if x in tgt:
                        src[x] = dict(tgt[x])
            except Exception:
                pass
        b = self.boosts.setdefault(k, {})
        if b is None:
            if kind == "-copyboost" and len(ev) > 3:
                tb = self.boosts.get(self.key(ev[3]), {})
                self.boosts[k] = dict(tb) if tb is not None else None
            return
        if kind in ("-boost", "-unboost") and len(ev) > 4:
            try:
                n = int(ev[4]) * (1 if kind == "-boost" else -1)
            except ValueError:
                return
            b[ev[3]] = max(-6, min(6, b.get(ev[3], 0) + n))
        elif kind == "-setboost" and len(ev) > 4:
            try:
                b[ev[3]] = int(ev[4])
            except ValueError:
                pass
        elif kind in ("-clearboost", "-clearallboost"):
            if kind == "-clearallboost":
                for kk in self.boosts:
                    self.boosts[kk] = {}
            else:
                self.boosts[k] = {}
        elif kind == "-clearnegativeboost":
            self.boosts[k] = {st: v for st, v in b.items() if v > 0}
        elif kind == "-invertboost":
            self.boosts[k] = {st: -v for st, v in b.items()}
        elif kind in ("-copyboost", "-swapboost", "-transform"):
            self.boosts[k] = None                                  # unknown from here on (until it switches out)
            if kind == "-copyboost" and len(ev) > 3:               # Psych Up: the user copies the target's stat stages
                tb = self.boosts.get(self.key(ev[3]), {})
                self.boosts[k] = dict(tb) if tb is not None else None

    def request_locks(self):
        """Our own Outrage-style lock is visible in the request: a locked Pokemon is offered only its locked move. The log
        alone can't tell a 2-turn lock from a 3-turn one when the user was already confused (no fatigue message either way),
        so drop our tracked lock when the request offers more than one move. (2026-10-18)"""
        req = getattr(self.b, "_last_request", None) or {}
        act = req.get("active") or []
        mons = [p for p in (req.get("side") or {}).get("pokemon", []) if p.get("active")]
        for j, p in enumerate(mons[:len(act)]):
            try:
                k = (str(p["ident"])[:2], base_id(log_mon(self.b, p["ident"]).species))
            except Exception:
                continue
            lm = (self.vols.get(k) or {}).get("lockedmove")
            if lm and len((act[j] or {}).get("moves") or []) > 1:
                del self.vols[k]["lockedmove"]

    def turn_flags(self, ev, kind):
        """(2026-10-18) Showdown's statsRaisedThisTurn / statsLoweredThisTurn / hurtThisTurn / usedItemThisTurn: set by
        -boost / -unboost / -damage / -enditem, cleared at '|turn|N' for N >= 2 (the leads' switch-in effects last through
        turn 1) and for a Pokemon switching out; a Pokemon switching in starts clean."""
        tf = self.__dict__.setdefault("tflags", {})
        # (2026-10-19) HP shown on each Pokemon's last HP line, and Endure / Focus Band activations: at 1 HP they cut a hit to
        # 0 damage; Showdown still prints '-damage' but sets hurtThisTurn only for nonzero damage
        lasthp = self.__dict__.setdefault("tf_lasthp", {})
        survive = self.__dict__.setdefault("tf_survive", set())
        prev_hp = None
        hp_at = 4 if kind in ("switch", "drag", "replace") else 3
        if kind in ("switch", "drag", "replace", "-damage", "-heal", "-sethp") and len(ev) > hp_at:
            kk = self.key(ev[2])
            if kk:
                prev_hp = lasthp.get(kk)
                lasthp[kk] = str(ev[hp_at]).split(" ")[0]
        if kind == "-activate" and len(ev) > 3 and to_id(ev[3]) in ("moveendure", "endure", "itemfocusband", "focusband"):
            kk = self.key(ev[2])
            if kk:
                survive.add(kk)
        if kind == "turn" and len(ev) > 2 and str(ev[2]).isdigit() and int(ev[2]) >= 2:
            tf.clear()
        elif kind in ("switch", "drag") and len(ev) > 2:
            slot = str(ev[2]).split(":")[0][:3]
            old = self.pos.get(slot)
            if old:
                tf.pop(old, None)
            k = self.key(ev[2])
            if k:
                tf.pop(k, None)
        elif kind == "-setboost" and len(ev) > 4:
            # (2026-10-19) Belly Drum / Anger Point: Showdown's boost({atk: 12}) prints -setboost and sets
            # statsRaisedThisTurn when the stage actually rose (unknown old stage: assume it did)
            k = self.key(ev[2])
            b = self.boosts.get(k) if k else None
            try:
                rose = b is None or b.get(ev[3], 0) < int(ev[4])
            except ValueError:
                rose = False
            if k and rose:
                tf.setdefault(k, set()).add("statsraised")
        elif kind in ("-boost", "-unboost", "-damage", "-enditem") and len(ev) > 2:
            k = self.key(ev[2])
            if not k:
                return
            if kind in ("-boost", "-unboost") and len(ev) > 4 and str(ev[4]).lstrip("-").isdigit() and int(ev[4]) == 0:
                return                                         # nothing changed (already at +6 / -6)
            if kind == "-enditem" and any(str(x).startswith("[from]") and str(x) not in ("[from] gem",) and "Fling" not in str(x)
                                          for x in ev[4:]):
                return                                         # removed by others: not 'used'
            if kind == "-enditem" and self.cud_eat:
                return                                         # (2026-10-19) Cud Chew's re-eat: not 'used this turn'
            if kind == "-enditem" and len(ev) > 3 and to_id(ev[3]) == "airballoon" and not any(str(x) == "[from] move: Fling" for x in ev[4:]):
                return                                         # popped with item = '' (no useItem); flung = used (Fling's onUpdate)
            if kind == "-damage" and k == self.cur_user and not any(str(x).startswith("[from]") for x in ev[4:])                     and to_id(self.cur_move or "") in ("substitute", "bellydrum", "clangoroussoul", "shedtail", "curse", "filletaway"):
                return                                         # HP costs: Showdown directDamage, not 'hurt this turn'
            if kind == "-damage":
                held = k in survive
                survive.discard(k)
                own = k[0] == getattr(self.b, "player_role", None)
                if prev_hp is not None and prev_hp == str(ev[3]).split(" ")[0] and (own or held):
                    return                                     # 0 damage (Endure / Focus Band at 1 HP): not hurt
            if kind == "-damage" and k == self.cur_user and to_id(self.cur_move or "") == "struggle"                     and any(str(x) == "[from] recoil" for x in ev[4:]):
                return                                         # (2026-10-19) Struggle recoil is directDamage too
            tf.setdefault(k, set()).add({"-boost": "statsraised", "-unboost": "statslowered", "-damage": "hurtthisturn",
                                         "-enditem": "useditem"}[kind])

    ABSORB = {"waterabsorb", "voltabsorb", "dryskin", "eartheater", "sapsipper", "lightningrod", "stormdrain", "motordrive",
              "flashfire", "wellbakedbody", "eartheater"}
    METRO_SKIP = {"copycat", "sleeptalk", "metronome", "assist", "mirrormove", "mefirst", "naturepower"}

    METRO_BLOCKS = {"protect", "detect", "spikyshield", "kingsshield", "banefulbunker", "silktrap", "burningbulwark",
                    "maxguard", "wideguard", "quickguard", "craftyshield", "matblock", "obstruct", "telepathy"}

    @staticmethod
    def _generic_heal(mid):
        try:
            from poke_env.battle.move import Move
            return bool(Move(mid or "", gen=9).entry.get("heal"))
        except Exception:
            return False

    def _sub_could_block(self, mover, mid):
        """(2026-10-19) True when the mover's status move is one a Substitute blocks and another active Pokemon has a
        Substitute up: Showdown then prints '-fail|<mover>' + '[still]' but the move counts as a success."""
        try:
            from poke_env.battle.move import Move
            mv = Move(mid, gen=9)
            if getattr(mv.category, "name", "") != "STATUS":
                return False
            tgt = str(mv.entry.get("target", ""))
            flags = mv.entry.get("flags", {}) or {}
        except Exception:
            return False
        if tgt in ("self", "allySide", "foeSide", "all", "allyTeam", "allies") or "bypasssub" in flags or "sound" in flags:
            return False
        return any(kk and kk != mover and "substitute" in self.vols.get(kk, {}) for kk in self.pos.values())

    def metro_ops(self, ev, kind):
        """(2026-10-18) Metronome (item) streak per Pokemon, as Showdown's onTryMove counts it: the same move again after a
        turn that worked = +1, else 0; called moves and their callers don't count; a charge turn leaves it unchanged; a
        'cant' (stopped before TryMove) only makes the last turn a failure. Applied only to Metronome holders."""
        mt = self.__dict__.setdefault("metro", {})          # key -> [last move, count, last turn ok, missed, hit, prev]
        if kind in ("move",) and len(ev) > 3:
            self.metro_cur, self.metro_move = self.key(ev[2]), to_id(ev[3])
            self.metro_still = any(str(x).strip() == "[still]" for x in ev[4:])
        elif kind in ("upkeep", "turn", "switch", "drag", "cant", "faint", "-heal") and not (kind == "-heal" and len(ev) > 4):
            if kind != "-heal":
                self.metro_cur = self.metro_move = None
        if kind in ("-fieldstart", "-fieldend") and len(ev) > 2 and to_id(str(ev[2]).replace("move:", "")) == "magicroom":
            self.metro_mroom = kind == "-fieldstart"
            return
        # (2026-10-19) the item's onStart adds the volatile at switch-in / when the item arrives (Trick...), except while Magic
        # Room suppresses items: then the Pokemon has NO streak until it switches out (None = off)
        if kind in ("switch", "drag") and len(ev) > 2:
            k = self.key(ev[2])
            if k:
                mt[k] = None if getattr(self, "metro_mroom", False) else [None, 0, True, False, False, None]
            return
        if kind == "-item" and len(ev) > 3 and to_id(ev[3]) == "metronome"                 and not any(to_id(str(x)) == "fromabilityfrisk" for x in ev[4:]):   # (2026-10-19) Frisk only reveals it
            k = self.key(ev[2])
            if k:
                hold = self.__dict__.setdefault("metro_hold", {})
                if mt.get(k) is None or hold.get(k, True):      # (2026-10-19) addVolatile is a no-op while the old streak
                    mt[k] = None if getattr(self, "metro_mroom", False) else [None, 0, True, False, False, None]
                hold[k] = True                                  # still exists (item lost, no move since: Trick back and forth)
            return
        # (2026-10-19) item lost (Trick / Knock Off...): the streak stays until the holder's next move (onTryMove removes it)
        if (kind == "-item" and len(ev) > 3 and not any(to_id(str(x)) == "fromabilityfrisk" for x in ev[4:]))                 or (kind == "-enditem" and len(ev) > 3 and to_id(ev[3]) == "metronome"):
            k = self.key(ev[2])
            if k:
                self.__dict__.setdefault("metro_hold", {})[k] = False
        bz = self.__dict__.setdefault("metro_bz", {})          # (2026-10-19) mover -> [boost lines with 0 seen, real change seen]
        def settle():
            for bk, (z, nz) in bz.items():                      # only '-boost ... 0' lines (Agility at +6): result null = failed
                e = mt.get(bk)
                if e is not None and z and not nz and not e[4]:
                    e[2] = False
                if e is not None and nz:
                    e[4] = True                                 # a stat change landed: the move hit someone (String Shot)
            bz.clear()
            for e in mt.values():
                if e is None:
                    continue
                if e[3] and not e[4]:
                    e[2] = False
                e[3] = e[4] = False
        # (2026-10-19) per-target outcome of the current move: a block (Protect family, Telepathy) counts like a miss; any
        # effect line on a target (status, volatile, item, stats...) counts as a hit, like damage
        if self.metro_cur and kind.startswith("-") and len(ev) > 2 and mt.get(self.metro_cur) is not None:
            tk = self.key(ev[2])
            frm = [str(x) for x in ev[3:] if str(x).startswith("[from]")]
            absorbed = tk and tk != self.metro_cur and ((kind == "-start" and "ability: Flash Fire" in "|".join(map(str, ev[3:])))
                                                        or (kind in ("-heal", "-boost") and any(
                                                            x.replace("[from] ability: ", "") in ABSORB_ABILITIES for x in frm)))
            if absorbed:
                mt[self.metro_cur][3] = True                    # (2026-10-19) absorbed (Flash Fire, Volt Absorb...) = failed on it
            elif tk and all(x.startswith("[from] move:") for x in frm):      # the mover itself too: Charge at +6 SpD still worked
                em = mt[self.metro_cur]
                what = to_id(str(ev[3]).replace("move:", "").replace("ability:", "")) if len(ev) > 3 else ""
                if kind == "-activate" and what in self.METRO_BLOCKS and tk != self.metro_cur:
                    em[3] = True
                elif kind == "-activate" and what == "substitute" and tk != self.metro_cur:
                    em[4] = True                                # (2026-10-19) a hit the Substitute took (HIT_SUBSTITUTE) = worked
                elif kind not in ("-miss", "-fail", "-immune", "-activate", "-notarget", "-hint", "-message", "-boost", "-unboost"):
                    em[4] = True
        if kind == "move" and len(ev) > 3:
            settle()
            k = self.key(ev[2])
            if not k:
                return
            if k in mt and mt[k] is None:
                return                                          # no metronome volatile: nothing counted
            if self.__dict__.get("metro_hold", {}).get(k) is False and k in mt:
                mt[k] = None                                    # moving without the item: the volatile is removed
                return
            if getattr(self, "metro_mroom", False) and k in mt:
                mt[k] = None                                    # (2026-10-19) onTryMove: item suppressed (Magic Room) -> volatile removed
                return
            e = mt.setdefault(k, [None, 0, True, False, False, None])
            mid = to_id(ev[3])
            prep = self.__dict__.setdefault("metro_prep", {})
            chg = self.__dict__.setdefault("metro_charge", {})
            for pk in list(prep):                               # an earlier '-prepare' with no hit after it = a real charge turn
                chg[pk] = prep.pop(pk)[0]
            frm = [str(x) for x in ev[4:] if str(x).startswith("[from]")]
            locked = any("lockedmove" in x for x in frm)        # (2026-10-19) Outrage / charge release: the Pokemon's own move
            if (frm and not locked) or mid in self.METRO_SKIP:
                e[2] = True
                return
            e[5] = (e[0], e[1])
            if locked and chg.get(k) == mid:                    # release turn: twoturnmove still on in onTryMove
                e[1] = e[1] + 1 if (mid == e[0] and e[2]) or mid == e[0] else 1
            else:
                e[1] = e[1] + 1 if (mid == e[0] and e[2]) else 0
            chg.pop(k, None)
            e[0] = mid
            e[2] = not any(str(x).strip() == "[spread]" for x in ev[4:])   # '[spread] ' with no targets = affected nobody
        elif kind in ("upkeep", "turn"):
            settle()
        elif kind == "-prepare" and len(ev) > 2 and not any(str(x) == "[premajor]" for x in ev[3:]):
            k = self.key(ev[2])
            e = mt.get(k)
            if e is not None and e[5] is not None:
                self.__dict__.setdefault("metro_prep", {})[k] = (e[0], e[1])   # undone if the move hits this turn (sun / Power Herb)
                e[0], e[1] = e[5]                               # charge turn: counter unchanged
        elif kind in ("-fail", "cant") and len(ev) > 2:
            k = self.key(ev[2])
            if kind == "-fail" and self.metro_cur and len(ev) > 3 and str(ev[3]).startswith("move:") and k and k != self.metro_cur:
                return                                          # (2026-10-19) per-target fail (Corrosive Gas, no item): the move still hit
            if kind == "-fail" and self.metro_cur and k == self.metro_cur and len(ev) == 3                     and getattr(self, "metro_still", False) and self._sub_could_block(self.metro_cur, self.metro_move):
                return                                          # (2026-10-19) a Substitute took it (HIT_SUBSTITUTE = 0 counts as a hit)
            if kind == "-fail" and self.metro_cur:
                k = self.metro_cur                               # (2026-10-19) '-fail' names the TARGET: the mover's move failed
            e = mt.get(k)
            if e is not None:
                if kind == "-fail" and len(ev) > 3 and to_id(ev[3]) == "heal" and not self._generic_heal(self.metro_move):
                    return                                      # (2026-10-19) full-HP Synthesis & co. (onHit heal) = NOT_FAIL: a success;
                                                                # Roost / Recover (the move's 'heal' field) at full HP = false: a failure
                if kind == "-fail" and len(ev) > 3 and k == self.metro_cur and e[5] is not None                         and to_id(str(ev[3]).replace("move:", "")) in ("burnup", "doubleshock", "shelltrap")                         and to_id(str(ev[3]).replace("move:", "")) == to_id(self.metro_move or ""):
                    e[0], e[1] = e[5]                           # the move's own onTryMove failed before the item's ran
                e[2] = False
        elif kind == "-activate" and len(ev) > 3 and to_id(str(ev[3]).replace("move:", "")) == "powder":
            k = self.key(ev[2])
            e = mt.get(k)
            if e is not None and k == self.metro_cur and e[5] is not None:
                e[0], e[1] = e[5]                               # Powder's onTryMove (priority -1) stops it before Metronome's (-2)
                e[2] = False
        elif kind == "-miss" and len(ev) > 2:
            e = mt.get(self.key(ev[2]))
            if e is not None:
                e[3] = True
        elif kind in ("-immune", "-heal", "-boost") and len(ev) > 2 and self.metro_cur and self.key(ev[2]) != self.metro_cur                 and (kind == "-immune" or any(str(x).startswith("[from] ability:") and to_id(str(x)[15:]) in self.ABSORB
                                              for x in ev[3:])):
            e = mt.get(self.metro_cur)                           # (2026-10-19) target immune / absorbed it: no hit there
            if e is not None:
                e[3] = True
        elif kind in ("-boost", "-unboost") and len(ev) > 4 and self.metro_cur and self.metro_cur in mt                 and not any(str(x).startswith("[from]") for x in ev[5:]):
            z = bz.setdefault(self.metro_cur, [False, False])
            try:
                z[0 if int(ev[4]) == 0 else 1] = True
            except ValueError:
                pass
        elif kind == "-damage" and len(ev) > 4 and any(to_id(str(x)) == "fromconfusion" for x in ev[4:]):
            e = mt.get(self.key(ev[2]))                         # (2026-10-19) confusion self-hit = a failed turn (no 'cant')
            if e is not None:
                e[2] = False
        elif kind == "-damage" and len(ev) > 2 and self.metro_cur and not any(str(x).startswith("[from]") for x in ev[4:]):
            e = mt.get(self.metro_cur)
            if e is not None:
                e[4] = True
                pr = self.__dict__.get("metro_prep", {}).pop(self.metro_cur, None)
                if pr is not None:                              # no charge turn after all: the move counted normally
                    e[0], e[1] = pr

    def stat_ops(self, ev, kind):
        """(2026-10-18) moves that change STORED stats until switch-out (the request shows the unchanged ones): Guard Split /
        Power Split (average), Speed Swap, Power Trick (Atk <-> Def). Recorded in order; bridge_vols.apply replays them on the
        rebuilt position's stats (pw['__statops'])."""
        ops = self.pw.setdefault("__statops", [])
        if kind in ("switch", "drag") and len(ev) > 2:
            k = self.key(ev[2])
            if k:
                ops.append(("reset", k))
                if len(ev) > 3:
                    ops.append(("forme", k, to_id(str(ev[3]).split(",")[0])))
        elif kind in ("-formechange", "detailschange") and len(ev) > 3:
            # (2026-10-19 bridge fuzz) a forme change recomputes the stored stats from the new forme (Showdown setSpecies):
            # a Speed Swap / Guard Split on the changer is gone (Hangry Morpeko, Aegislash-Blade)
            k = self.key(ev[2])
            if k:
                ops.append(("reset", k))
                ops.append(("forme", k, to_id(str(ev[3]).split(",")[0])))
        elif kind == "-transform" and len(ev) > 2:
            k = self.key(ev[2])
            if k:
                ops.append(("reset", k))
                t = self.key(ev[3]) if len(ev) > 3 else None
                if t:
                    ops.append(("copy", k, t))                  # (2026-10-19) Transform copies the target's CURRENT stored stats
        elif kind == "-activate" and len(ev) > 3:
            mv = str(ev[3]).replace("move: ", "")
            of = next((str(x)[5:] for x in ev[4:] if str(x).startswith("[of] ")), None)
            pair = {"Guard Split": ("split", ("def", "spd")), "Power Split": ("split", ("atk", "spa")),
                    "Speed Swap": ("swap", ("spe",))}.get(mv)
            if pair and of:
                a, b = self.key(ev[2]), self.key(of)
                if a and b:
                    ops.append((pair[0], a, b, pair[1]))
        elif kind in ("-start", "-end") and len(ev) > 3 and _vid(ev[3]) == "powertrick":
            k = self.key(ev[2])
            if k:
                ops.append(("trick", k))

    def output(self):
        """(vols, sides, pw) in bridge_vols.apply's format; 'src' = (role, base id)"""
        try:
            self.request_locks()
        except Exception:
            pass
        vols = {"p1": {}, "p2": {}}
        for (role, nm), v in self.vols.items():
            d = {}
            for vid, e in v.items():
                e = dict(e)
                if vid == "confusion":
                    e = {"time": 0x80 | min(e.get("elapsed", 0), 0x7f)}
                elif vid == "partiallytrapped":
                    e = {"duration": 0x80 | min(e.pop("e", 0), 0x7f), **e}
                elif vid == "lockedmove":
                    kk = e.get("k", 0)
                    e = {"duration": e["duration"], "move": e["move"],
                         "trueDuration": max(1, e["known"] - kk) if "known" in e else 0x80 | kk}
                if vid == "substitute" and e.get("hp") is None:
                    e.pop("hp", None)
                    e["sub_quarter"] = True
                d[vid] = e
            vols[role][nm] = d
        for (role, nm), fl in (self.__dict__.get("tflags") or {}).items():
            for f in fl:
                vols[role].setdefault(nm, {})[f] = {}
        for (role, nm), e in (self.__dict__.get("metro") or {}).items():
            if (role, nm) in self.pos.values():               # bridge_vols applies it to Metronome holders only
                vols[role].setdefault(nm, {})["metronome"] = ({"off": True} if e is None else
                                                              {"lastMove": e[0] or "", "numConsecutive": e[1]})
        for (role, nm), it in self.lastitem.items():
            vols[role].setdefault(nm, {})["__lastitem"] = {"item": it}
        for (role, nm) in self.__dict__.get("ateberry") or ():
            vols[role].setdefault(nm, {})["__ateberry"] = {}
        for (role, nm) in self.__dict__.get("syrupdone") or ():
            vols[role].setdefault(nm, {})["__syrupdone"] = {}
        # (2026-10-19 bridge flag 17320) Illusion: our disguised Zoroark (illusion_fix knows it); the others are not disguised
        # (an opponent's disguised Zoroark is rebuilt as the Pokemon it shows)
        for slot, (ident, true_ident) in ((self.b.__dict__.get("_illusion_fix") or {}).get("ours") or {}).items():
            try:
                role = slot[:2]
                z = self.b._team.get(role + ": " + str(true_ident).split(": ", 1)[1])
                shown = self.b._team.get(role + ": " + str(ident).split(": ", 1)[1])
                if z is not None and shown is not None:
                    vols[role].setdefault(base_id(z.species), {})["__illusion"] = {"as": base_id(shown.species)}
            except Exception:
                pass
        sides = {r: {"slots": self.slots[r], "side": self.side[r], "names": []} for r in ("p1", "p2")}
        return vols, sides, dict(self.pw)


def from_log(battle):
    return Tracker(battle).run().output()
