"""
(2026-10-18, bridge fuzz) Fixes for poke-env 0.16 battle-state bugs that make the bot's rebuilt position wrong.
install() wraps AbstractBattle.parse_message (installed by bridge.py before illusion_fix, so illusion_fix's renames
happen first). POKEENV_FIX=0 = off.

1. '|-copyboost|USER|TARGET|[from] move: Psych Up': Showdown means the USER copies the TARGET's stat stages; poke-env
   copied the user's stages onto the target (the target's stages were overwritten, the user's never changed).
2. '|switch|...|[from] Baton Pass': poke-env clears boosts at switch-out, so the stages Baton Pass hands over were lost.
3. Abilities gained mid-battle (Skill Swap with hidden abilities, Role Play, Entrainment, Worry Seed, Simple Beam, Doodle,
   Mummy, Lingering Aroma, Wandering Spirit, Power of Alchemy, Receiver): when the Pokemon's own ability was still unknown,
   poke-env stored the gained one as its PERMANENT ability (a Skill-Swapped Speed Boost stayed after switching out). They
   now go to temporary_ability, which poke-env clears at switch-out.
4. Regenerator: Showdown prints the heal ('|-heal|...|[from] ability: Regenerator|[silent]') and poke-env healed another
   third at switch_out (an opponent's Slowbro at 51% came back as 100%). switch_out keeps the HP the log gave.
6. Switch orders: poke-env sends '/choose switch NAME' and Showdown resolves NAME against each party member's name OR CURRENT
   species in party order, so an active Ditto transformed into a Clawitzer captured 'switch Clawitzer' ("can't switch to an
   active Pokemon", re-asked every turn). Orders now name the party slot from the latest request ('switch 3').
5. Spite / Eerie Spell ('|-activate|MON|move: Spite|MOVE|N'): poke-env ignored them, so an opponent's move Spited to 0 PP
   still had PP.
7. '|t:|' turn timestamps: Player.MESSAGES_TO_IGNORE dropped them before parse_message, so _replay_data and saved replays
   had no timestamps; moved to AbstractBattle.MESSAGES_TO_IGNORE (recorded in _replay_data, otherwise ignored).
"""

from poke_env.battle.abstract_battle import AbstractBattle
from poke_env.battle.pokemon import Pokemon

_GAINED_FROM = ("[from] move:", "[from] ability: Mummy", "[from] ability: Lingering Aroma", "[from] ability: Wandering Spirit",
                "[from] ability: Power of Alchemy", "[from] ability: Receiver")


_BLOCKER_ABILITIES = ("ability: Armor Tail", "ability: Queenly Majesty", "ability: Dazzling")
_GAINED_IDS = set()     # Pokemon (by id: poke-env's Pokemon has __slots__) holding a silently Skill-Swapped ability


def _gained(battle):
    """ids of this battle's Pokemon whose current ability was gained since they switched in"""
    g = getattr(battle, "_pokeenv_ab_gained", None)
    if g is None:
        g = set()
        battle._pokeenv_ab_gained = g
    return g


def _tid(s):
    return "".join(c for c in str(s).lower() if c.isalnum())


# (2026-10-18) Champions move-target changes (data/mods/champions): Milk Drink targets "adjacentAllyOrSelf"; poke-env's gen 9
# data says "self", so its orders had no target and Showdown refused them ("Milk Drink needs a target")
CHAMPIONS_TARGETS = {"milkdrink": "adjacentAllyOrSelf"}


def _patch_targets():
    try:
        from poke_env.data import GenData
        moves = GenData.from_gen(9).moves
        for mid, tg in CHAMPIONS_TARGETS.items():
            if mid in moves:
                moves[mid]["target"] = tg
    except Exception:
        pass


_MON_BATTLE = {}   # id(our Pokemon) -> its battle (set at every request; finished battles pruned; battles have no __weakref__)
_SET_ABILITY = {}  # (2026-10-19) id(our Pokemon) -> its SET's ability (the first request's; a Mega's later requests show the Mega's)


def set_ability_of(mon):
    """(2026-10-19 bridge flag 15403) our Pokemon's set ability as first seen in a request, or None"""
    return _SET_ABILITY.get(id(mon))


def _slot_of(mon):
    """1-based party slot of our Pokemon in the latest request, or None"""
    b = _MON_BATTLE.get(id(mon))
    if b is None:
        return None
    try:
        side = (b._last_request or {}).get("side", {}).get("pokemon", [])
        for i, e in enumerate(side):
            if b.get_pokemon(e["ident"]) is mon:
                return i + 1
    except Exception:
        return None
    return None


def _patch_switch_orders():
    from poke_env.player.battle_order import SingleBattleOrder
    orig = SingleBattleOrder.message

    def message(self):
        if isinstance(self.order, Pokemon):
            k = _slot_of(self.order)
            if k is not None:
                return f"/choose switch {k}"
        return orig.fget(self)

    SingleBattleOrder.message = property(message)
    from poke_env.battle.battle import Battle
    from poke_env.battle.double_battle import DoubleBattle

    def wrap(cls):
        orig_req = cls.parse_request

        def parse_request(self, request, *a, **k):
            r = orig_req(self, request, *a, **k)
            try:
                if len(_MON_BATTLE) > 4000:
                    for key in [key for key, b in _MON_BATTLE.items() if b.finished]:
                        del _MON_BATTLE[key]
                        _SET_ABILITY.pop(key, None)
                for mon in self.team.values():
                    _MON_BATTLE[id(mon)] = self
                for e in (request or {}).get("side", {}).get("pokemon", []):
                    mon = self.get_pokemon(e["ident"])
                    ab = e.get("baseAbility") or e.get("ability")
                    if mon is not None and ab and id(mon) not in _SET_ABILITY:
                        _SET_ABILITY[id(mon)] = str(ab)
            except Exception:
                pass
            return r

        cls.parse_request = parse_request

    for cls in {AbstractBattle, Battle, DoubleBattle}:
        if "parse_request" in cls.__dict__:
            wrap(cls)


def _patch_teambuilder():
    """Normalize raw Showdown pastes into encoding.canonical_team order so poke-env slot and move indices match the encoder's canonical order."""
    try:
        from poke_env.teambuilder.teambuilder import Teambuilder
        from encoding import _fnv_u, to_id
        orig = Teambuilder.parse_showdown_team

        def parse_showdown_team(team):
            mons = orig(team)
            for m in mons:
                m.moves.sort(key=lambda mv: (_fnv_u("move|" + to_id(mv)), to_id(mv)))
            mons.sort(key=lambda m: (_fnv_u("species|" + to_id(m.species or m.nickname)), to_id(m.species or m.nickname)))
            return mons

        Teambuilder.parse_showdown_team = staticmethod(parse_showdown_team)
    except Exception:
        pass


def install():
    if getattr(AbstractBattle, "_pokeenv_fix_installed", False):
        return
    _patch_targets()
    _patch_switch_orders()
    _patch_teambuilder()
    orig = AbstractBattle.parse_message

    def parse_message(self, split_message):
        ev = split_message
        try:
            kind = ev[1] if len(ev) > 1 else ""
            if kind == "move" and any(str(x) == "[from] move: Round" for x in ev[4:]):
                # (2026-10-19 bridge fuzz parse errors) Round moved up by its partner's Round is the Pokemon's OWN chosen move;
                # poke-env takes the tag for a called move and looks Round up in its moveset (KeyError if not seen yet)
                n = len(self._replay_data)
                r = orig(self, [x for x in ev if str(x) != "[from] move: Round"])
                if len(self._replay_data) > n:
                    self._replay_data[-1] = split_message[:]      # the trackers read the original line
                return r
            if (kind == "cant" and len(ev) > 4 and str(ev[3]) in _BLOCKER_ABILITIES
                    and any(str(x).startswith("[of] ") for x in ev[4:])):
                # (2026-10-19 bridge fuzz) Armor Tail / Queenly Majesty / Dazzling: '|cant|HOLDER|ability: X|MOVE|[of] MOVER'
                # names the holder first, but the MOVER is the one stopped; every log reader takes ev[2] as the Pokemon that
                # couldn't move (moved flag, failed turn, Disable length...). The stored line puts the mover first.
                n = len(self._replay_data)
                r = orig(self, split_message)
                mover = next(str(x)[5:] for x in ev[4:] if str(x).startswith("[of] "))
                rest = [x for x in ev[4:] if not str(x).startswith("[of] ")]
                if len(self._replay_data) > n:
                    self._replay_data[-1] = [ev[0], "cant", mover, ev[3]] + rest + ["[of] " + str(ev[2])]
                return r
            if kind == "-copyboost" and len(ev) > 3:
                user, target = self.get_pokemon(ev[2]), self.get_pokemon(ev[3])
                user._boosts = dict(target._boosts.items())
                self._replay_data.append(split_message[:])   # poke-env's handler (skipped) records every line
                return None
            if kind == "-ability" and len(ev) > 3:
                mon = self.get_pokemon(ev[2])
                gset = _gained(self)
                gained = id(mon) in gset or any(str(x).startswith(_GAINED_FROM) for x in ev[4:])
                if gained and mon._ability is None and str(ev[3]) != "Neutralizing Gas":
                    mon._temporary_ability = _tid(ev[3])
                    gset.add(id(mon))
                    self._replay_data.append(split_message[:])
                    return None
            if kind == "-activate" and len(ev) > 5 and str(ev[3]) in ("move: Spite", "move: Eerie Spell"):
                mon = self.get_pokemon(ev[2])
                mv = mon.moves.get(_tid(ev[4]))
                if mv is not None:
                    try:
                        mv._current_pp = max(0, mv._current_pp - int(ev[5]))
                    except ValueError:
                        pass
                return orig(self, split_message)
            if kind == "-activate" and len(ev) > 3 and str(ev[3]).replace("move: ", "") == "Skill Swap":
                r = orig(self, split_message)
                for x in [ev[2]] + [str(y)[5:] for y in ev[4:] if str(y).startswith("[of] ")]:
                    try:
                        pm = self.get_pokemon(x)
                        _gained(self).add(id(pm))
                        _GAINED_IDS.add(id(pm))
                    except Exception:
                        pass
                return r
            if kind in ("switch", "drag") and len(ev) > 2:
                slot = str(ev[2]).split(":")[0][:3]
                act = self._active_pokemon if slot[:2] == self._player_role else self._opponent_active_pokemon
                if act.get(slot) is not None:
                    _gained(self).discard(id(act[slot]))          # switch-out ends a gained ability
                    _GAINED_IDS.discard(id(act[slot]))
            passed = None
            if kind == "switch" and len(ev) > 2 and any("[from] Baton Pass" in str(x) for x in ev[5:]):
                slot = str(ev[2]).split(":")[0][:3]
                act = self._active_pokemon if slot[:2] == self._player_role else self._opponent_active_pokemon
                old = act.get(slot)
                if old is not None:
                    passed = dict(old._boosts.items())
        except Exception:
            return orig(self, split_message)
        r = orig(self, split_message)
        if passed is not None:
            try:
                self.get_pokemon(ev[2])._boosts = passed
            except Exception:
                pass
        return r

    AbstractBattle.parse_message = parse_message
    orig_out = Pokemon.switch_out

    def switch_out(self, *a, **k):
        hp = self._current_hp
        r = orig_out(self, *a, **k)
        self._current_hp = hp                     # the Regenerator heal comes from the log's own '-heal' line
        return r

    Pokemon.switch_out = switch_out

    # (2026-10-18) any ability revealed while a Skill-Swapped one is held (e.g. '|-heal|...|[from] ability: Regenerator' at
    # switch-out after allies swapped silently) belongs to the stint, not the Pokemon: poke-env's other handlers (heal /
    # damage / '[from] ability:') set it as PERMANENT when the own ability was unknown (Tsareena kept Regenerator)
    prop = Pokemon.ability

    def _set_ability(self, ability):
        if id(self) in _GAINED_IDS and self._ability is None:
            from poke_env.data.normalize import to_id_str
            self._temporary_ability = to_id_str(ability)
            return
        prop.fset(self, ability)

    Pokemon.ability = property(prop.fget, _set_ability, prop.fdel, prop.__doc__)
    try:
        from poke_env.player.player import Player
        Player.MESSAGES_TO_IGNORE = set(Player.MESSAGES_TO_IGNORE) - {"t:"}
        AbstractBattle.MESSAGES_TO_IGNORE = set(AbstractBattle.MESSAGES_TO_IGNORE) | {"t:"}
    except Exception:
        pass
    AbstractBattle._pokeenv_fix_installed = True
