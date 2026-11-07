"""(2026-10-17) Hand-built scenarios for code the fuzzers never run (tools/code_coverage.py report), each checked against REAL Showdown.

A scenario = both sides' four Pokemon (first two lead), setup calls (engine_diff's set_mon / set_field / set_side), and a scripted
joint choice for EVERY turn. Played --seeds times in the fast engine and in Showdown (tools/showdown_truth.js, req.script for turns
after the first); the outcome distributions are compared (engine_diff.compare), and Showdown's logs must match the scenario's
`expect` regex in some seeds, or the scenario didn't test what it says.

Actions per slot: "Move Name" (no target / spread / self), "Move Name>1" / ">2" (foe slot a / b), ">-1" / ">-2" (own slot a / b),
"Move Name*" (+ Mega Evolution), "switch N" (team position 1-4, as on turn 1: avoid switches after the first switch), "pass".

    python tools/scenarios.py [--seeds 200] [--only NAME,...] [--show NAME]
"""
import argparse
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
if os.environ.get("VGCSIM_MODULE_DIR"):          # test a rebuilt module without installing it (regress.sh sets it)
    sys.path.insert(0, os.path.abspath(os.environ["VGCSIM_MODULE_DIR"]))
import engine_diff as E  # noqa: E402
import multi_turn_truth as M  # noqa: E402


def mon(species, item, ability, moves, evs="32 HP / 2 Atk / 32 Def", nature="Hardy"):
    head = species + (f" @ {item}" if item else "")
    return "\n".join([head, f"Ability: {ability}", "Level: 50", f"EVs: {evs}", f"{nature} Nature"] + [f"- {m}" for m in moves])


# passive fillers (set up, never attack), distinct species
SNORLAX = mon("Snorlax", "Leftovers", "Thick Fat", ["Curse", "Body Slam", "Protect", "Rest"])
CRESSELIA = mon("Cresselia", "Mental Herb", "Levitate", ["Calm Mind", "Moonblast", "Protect", "Moonlight"])
AGGRON = mon("Aggron", "Shuca Berry", "Sturdy", ["Iron Defense", "Heavy Slam", "Protect", "Sleep Talk"])
CORVIKNIGHT = mon("Corviknight", "Rocky Helmet", "Pressure", ["Bulk Up", "Brave Bird", "Protect", "Roost"])
SKARMORY = mon("Skarmory", "Rocky Helmet", "Sturdy", ["Iron Defense", "Brave Bird", "Protect", "Roost"])
GARGANACL = mon("Garganacl", "Leftovers", "Purifying Salt", ["Iron Defense", "Salt Cure", "Protect", "Recover"])
BENCH = [AGGRON, CORVIKNIGHT]
BENCH_B = [SKARMORY, GARGANACL]


def S(name, why, a, b, script, calls=(), expect=None, turn=3, ill_ok=False):
    # ill_ok: the fast engine refuses a scripted choice that Showdown accepts and turns into Struggle (Champions Fake Out)
    return {"name": name, "why": why, "a": a, "b": b, "script": script, "calls": list(calls), "expect": expect, "turn": turn, "ill_ok": ill_ok}


SCENARIOS = [
    S("brick_break_into_ghost", "does Brick Break break Reflect when it hits an immune Ghost? (TryHit runs before type immunity)",
      [mon("Machamp", "Leftovers", "No Guard", ["Brick Break", "Bulk Up", "Protect", "Knock Off"]), CRESSELIA] + BENCH,
      [mon("Gengar", "Leftovers", "Cursed Body", ["Protect", "Shadow Ball", "Will-O-Wisp", "Taunt"]), SNORLAX] + BENCH_B,
      [["Brick Break>1", "Calm Mind"], ["Taunt>2", "Curse"]], calls=[("set_side", (1,), {"reflect": 4})], expect=r"\|-immune\|p2a"),
    S("rest_full_hp", "Rest fails at full HP (battle.rs status_effects Rest)",
      [SNORLAX, CRESSELIA] + BENCH, [mon("Gholdengo", "Leftovers", "Good as Gold", ["Nasty Plot", "Make It Rain", "Protect", "Recover"]),
                                     mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Rest", "Calm Mind"], ["Nasty Plot", "Slack Off"]], expect=r"\|-fail\|p1a: m0"),
    S("rest_insomnia", "Rest fails with Insomnia (half HP)",
      [mon("Banette", "Leftovers", "Insomnia", ["Rest", "Shadow Claw", "Protect", "Sleep Talk"]), CRESSELIA] + BENCH,
      [mon("Gholdengo", "Leftovers", "Good as Gold", ["Nasty Plot", "Make It Rain", "Protect", "Recover"]), SNORLAX] + BENCH_B,
      [["Rest", "Calm Mind"], ["Nasty Plot", "Curse"]], calls=[("set_mon", (0, 0), {"hp_frac": 0.5})], expect=r"\|-fail\|p1a: m0"),
    S("rest_vital_spirit", "Rest fails with Vital Spirit (half HP)",
      [mon("Annihilape", "Leftovers", "Vital Spirit", ["Rest", "Drain Punch", "Protect", "Bulk Up"]), CRESSELIA] + BENCH,
      [mon("Gholdengo", "Leftovers", "Good as Gold", ["Nasty Plot", "Make It Rain", "Protect", "Recover"]), SNORLAX] + BENCH_B,
      [["Rest", "Calm Mind"], ["Nasty Plot", "Curse"]], calls=[("set_mon", (0, 0), {"hp_frac": 0.5})], expect=r"\|-fail\|p1a: m0"),
    S("sleeptalk_calls_rest", "asleep: Sleep Talk may call Rest, which fails while asleep",
      [mon("Snorlax", "Leftovers", "Thick Fat", ["Sleep Talk", "Rest", "Curse", "Body Slam"]), CRESSELIA] + BENCH,
      [mon("Gholdengo", "Leftovers", "Good as Gold", ["Nasty Plot", "Make It Rain", "Protect", "Recover"]),
       mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Sleep Talk", "Calm Mind"], ["Nasty Plot", "Slack Off"]],
      calls=[("set_mon", (0, 0), {"hp_frac": 0.6, "status": "slp"})], expect=r"\|move\|p1a: m0\|Rest"),
    S("sleeptalk_gravity", "asleep under Gravity: Sleep Talk calling Flying Press / High Jump Kick fails (Gravity's onModifyMove)",
      [mon("Hawlucha", "Leftovers", "Mold Breaker", ["Sleep Talk", "Flying Press", "High Jump Kick", "Protect"], "32 HP / 32 Atk / 2 Def",
           "Brave"),
       mon("Alakazam", "Focus Sash", "Inner Focus", ["Gravity", "Psychic", "Protect", "Calm Mind"], "32 SpA / 32 Spe / 2 HP", "Timid")] + BENCH,
      [mon("Gholdengo", "Leftovers", "Good as Gold", ["Nasty Plot", "Make It Rain", "Protect", "Recover"]), SNORLAX] + BENCH_B,
      [["Sleep Talk", "Gravity"], ["Nasty Plot", "Curse"]],
      calls=[("set_mon", (0, 0), {"status": "slp"})], expect=r"\|cant\|p1a: m0\|move: Gravity"),
    S("milkdrink_self", "Champions Milk Drink (adjacentAllyOrSelf) aimed at the user itself",
      [mon("Gogoat", "Leftovers", "Sap Sipper", ["Milk Drink", "Horn Leech", "Protect", "Bulk Up"]), CRESSELIA] + BENCH,
      [mon("Gholdengo", "Leftovers", "Good as Gold", ["Nasty Plot", "Make It Rain", "Protect", "Recover"]), SNORLAX] + BENCH_B,
      [["Milk Drink>-1", "Calm Mind"], ["Nasty Plot", "Curse"]],
      calls=[("set_mon", (0, 0), {"hp_frac": 0.4}), ("set_mon", (0, 1), {"hp_frac": 0.4})], expect=r"\|-heal\|p1a: m0"),
    S("milkdrink_ally", "Champions Milk Drink aimed at the partner",
      [mon("Gogoat", "Leftovers", "Sap Sipper", ["Milk Drink", "Horn Leech", "Protect", "Bulk Up"]), CRESSELIA] + BENCH,
      [mon("Gholdengo", "Leftovers", "Good as Gold", ["Nasty Plot", "Make It Rain", "Protect", "Recover"]), SNORLAX] + BENCH_B,
      [["Milk Drink>-2", "Calm Mind"], ["Nasty Plot", "Curse"]],
      calls=[("set_mon", (0, 0), {"hp_frac": 0.4}), ("set_mon", (0, 1), {"hp_frac": 0.4})], expect=r"\|-heal\|p1b: m1"),
    S("sleeptalk_nothing_callable", "asleep: Sleep Talk with only charge moves fails",
      [mon("Chandelure", "Leftovers", "Flash Fire", ["Sleep Talk", "Solar Beam", "Meteor Beam", "Sky Attack"]), CRESSELIA] + BENCH,
      [mon("Gholdengo", "Leftovers", "Good as Gold", ["Nasty Plot", "Make It Rain", "Protect", "Recover"]), SNORLAX] + BENCH_B,
      [["Sleep Talk", "Calm Mind"], ["Nasty Plot", "Curse"]],
      calls=[("set_mon", (0, 0), {"status": "slp"})], expect=r"\|-fail\|p1a: m0"),
    S("aura_wheel_copycat", "Copycat copies Morpeko's Aura Wheel: fails for a non-Morpeko",
      [mon("Morpeko", "Sitrus Berry", "Hunger Switch", ["Aura Wheel", "Protect", "Parting Shot", "Fake Out"], "32 Atk / 32 Spe / 2 HP",
           "Jolly"),
       mon("Wigglytuff", "Leftovers", "Competitive", ["Copycat", "Protect", "Dazzling Gleam", "Sing"], "32 HP / 32 Spe / 2 Def",
           "Timid")] + BENCH,       # faster than both foes: the last move used before Copycat is Morpeko's
      [SNORLAX, mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Aura Wheel>1", "Copycat"], ["Curse", "Slack Off"]], expect=r"\|move\|p1b: m1\|Aura Wheel"),
    S("fling_light_ball", "Fling + Light Ball paralyses the target",
      [mon("Grimmsnarl", "Light Ball", "Prankster", ["Fling", "Spirit Break", "Reflect", "Thunder Wave"], "32 HP / 32 Atk / 2 Def"),
       CRESSELIA] + BENCH,
      [SNORLAX, mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Fling>1", "Calm Mind"], ["Curse", "Slack Off"]], expect=r"\|-status\|p2a: m0\|par"),
    S("cheek_pouch_bug_bite", "Cheek Pouch user eats the target's Sitrus with Bug Bite",
      [mon("Diggersby", "Silk Scarf", "Cheek Pouch", ["Bug Bite", "Earthquake", "Protect", "Swords Dance"], "32 HP / 32 Atk / 2 Def"),
       CRESSELIA] + BENCH,
      [mon("Gholdengo", "Sitrus Berry", "Good as Gold", ["Nasty Plot", "Make It Rain", "Protect", "Recover"]), SNORLAX] + BENCH_B,
      [["Bug Bite>1", "Calm Mind"], ["Nasty Plot", "Curse"]], calls=[("set_mon", (0, 0), {"hp_frac": 0.4})],
      expect=r"Cheek Pouch"),
    S("cheek_pouch_residual", "Cheek Pouch: Sitrus eaten after poison damage at the end of the turn",
      [mon("Dedenne", "Sitrus Berry", "Cheek Pouch", ["Nuzzle", "Protect", "Super Fang", "Eerie Impulse"]), CRESSELIA] + BENCH,
      [mon("Gholdengo", "Leftovers", "Good as Gold", ["Nasty Plot", "Make It Rain", "Protect", "Recover"]), SNORLAX] + BENCH_B,
      [["Protect", "Calm Mind"], ["Nasty Plot", "Curse"]], calls=[("set_mon", (0, 0), {"hp_frac": 0.55, "status": "psn"})],
      expect=r"Cheek Pouch"),
    S("ripen_resist_berry", "Ripen doubles a resist berry (Yache vs Ice Beam)",
      [mon("Appletun", "Yache Berry", "Ripen", ["Apple Acid", "Protect", "Recover", "Draco Meteor"]), CRESSELIA] + BENCH,
      [mon("Glalie", "Never-Melt Ice", "Inner Focus", ["Ice Beam", "Protect", "Freeze-Dry", "Ice Shard"], "32 SpA / 32 Spe / 2 HP",
           "Modest"), SNORLAX] + BENCH_B,
      [["Recover", "Calm Mind"], ["Ice Beam>1", "Curse"]], expect=r"Yache Berry"),
    S("sand_stream_smooth_rock", "Sand Stream + Smooth Rock = 8 turns of sand (switched in on turn 1, 6 turns played)",
      [SKARMORY, CRESSELIA, mon("Tyranitar", "Smooth Rock", "Sand Stream", ["Rock Slide", "Protect", "Crunch", "Dragon Dance"]),
       AGGRON],
      [mon("Garganacl", "Leftovers", "Purifying Salt", ["Iron Defense", "Salt Cure", "Protect", "Recover"]),
       mon("Excadrill", "Leftovers", "Sand Rush", ["Swords Dance", "Iron Head", "Protect", "Earthquake"]), CORVIKNIGHT,
       mon("Steelix", "Leftovers", "Sturdy", ["Iron Defense", "Heavy Slam", "Protect", "Curse"])],
      [["switch 3", "Calm Mind"], ["Iron Defense", "Swords Dance"]] + [["Dragon Dance", "Calm Mind"], ["Recover", "Protect"]] * 0
      + [["Dragon Dance", "Moonlight"], ["Recover", "Swords Dance"]] * 5, expect=r"\|-weather\|Sandstorm"),
    S("sand_spit_smooth_rock", "Sand Spit + Smooth Rock = 8 turns of sand (6 turns played)",
      [mon("Sandaconda", "Smooth Rock", "Sand Spit", ["Coil", "Protect", "Rest", "High Horsepower"]), CRESSELIA] + BENCH,
      [mon("Garganacl", "Leftovers", "Purifying Salt", ["Iron Defense", "Salt Cure", "Protect", "Recover"]),
       mon("Excadrill", "Leftovers", "Sand Rush", ["Swords Dance", "Iron Head", "Protect", "Earthquake"])] + [CORVIKNIGHT, SKARMORY],
      [["Coil", "Calm Mind"], ["Salt Cure>1", "Swords Dance"]] + [["Coil", "Moonlight"], ["Recover", "Swords Dance"]] * 5,
      expect=r"\|-weather\|Sandstorm"),
    S("shed_tail", "Shed Tail passes its Substitute to the incoming Pokemon (mid-turn switch)",
      [mon("Orthworm", "Leftovers", "Earth Eater", ["Shed Tail", "Iron Head", "Protect", "Coil"]), CRESSELIA, SNORLAX, AGGRON],
      [mon("Garchomp", "Life Orb", "Rough Skin", ["Dragon Claw", "Protect", "Earthquake", "Swords Dance"], "32 Atk / 32 Spe / 2 HP",
           "Jolly"), mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Shed Tail", "Calm Mind"], ["Swords Dance", "Slack Off"], ["Curse", "Calm Mind"], ["Dragon Claw>1", "Slack Off"]],
      expect=r"Substitute"),
    S("baton_pass_leech_seed", "Leech Seed passed by Baton Pass keeps healing the seeder (not the seeded side)",
      [mon("Espeon", "Leftovers", "Synchronize", ["Baton Pass", "Calm Mind", "Protect", "Psychic"]), CRESSELIA, SNORLAX, AGGRON],
      [mon("Venusaur", "Leftovers", "Chlorophyll", ["Leech Seed", "Protect", "Giga Drain", "Sludge Bomb"]),
       mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Calm Mind", "Calm Mind"], ["Leech Seed>1", "Slack Off"], ["Baton Pass", "Calm Mind"], ["Protect", "Slack Off"],
       ["Curse", "Calm Mind"], ["Protect", "Slack Off"], ["Curse", "Calm Mind"], ["Protect", "Slack Off"]],
      calls=[("set_mon", (1, 0), {"hp_frac": 0.5}), ("set_mon", (0, 2), {"hp_frac": 0.6})],
      expect=r"leechseed|Leech Seed"),
    S("liquid_ooze_vs_substitute", "a draining hit on a Liquid Ooze user's Substitute hurts the attacker",
      [mon("Snorlax", "Leftovers", "Thick Fat", ["Drain Punch", "Curse", "Protect", "Rest"], "32 HP / 32 Atk / 2 Def", "Brave"),
       CRESSELIA] + BENCH,
      [mon("Swalot", "Leftovers", "Liquid Ooze", ["Substitute", "Protect", "Sludge Bomb", "Yawn"], "32 HP / 32 Spe / 2 Def"),
       mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Drain Punch>1", "Calm Mind"], ["Substitute", "Slack Off"]], expect=r"Liquid Ooze"),
    S("bigroot_drain_ooze", "Big Root holder's drain hit into Liquid Ooze: ooze damage boosted or not?",
      [mon("Snorlax", "Big Root", "Thick Fat", ["Drain Punch", "Curse", "Protect", "Rest"], "32 HP / 32 Atk / 2 Def", "Brave"),
       CRESSELIA] + BENCH,
      [mon("Swalot", "Leftovers", "Liquid Ooze", ["Protect", "Sludge Bomb", "Yawn", "Stockpile"], "32 HP / 32 Def / 2 SpD"),
       mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Drain Punch>1", "Calm Mind"], ["Stockpile", "Slack Off"]], expect=r"Liquid Ooze"),
    S("bigroot_leechseed_ooze", "Big Root holder's Leech Seed on a Liquid Ooze user",
      [mon("Venusaur", "Big Root", "Chlorophyll", ["Leech Seed", "Protect", "Giga Drain", "Sludge Bomb"], "32 HP / 32 Def / 2 SpD"),
       CRESSELIA] + BENCH,
      [mon("Swalot", "Leftovers", "Liquid Ooze", ["Protect", "Sludge Bomb", "Yawn", "Stockpile"], "32 HP / 32 Def / 2 SpD"),
       mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Leech Seed>1", "Calm Mind"], ["Stockpile", "Slack Off"], ["Protect", "Calm Mind"], ["Stockpile", "Slack Off"]],
      expect=r"Liquid Ooze"),
    S("bigroot_strengthsap_ooze", "Big Root holder's Strength Sap on a Liquid Ooze user",
      [mon("Venusaur", "Big Root", "Chlorophyll", ["Strength Sap", "Protect", "Giga Drain", "Sludge Bomb"], "32 HP / 32 Def / 2 SpD"),
       CRESSELIA] + BENCH,
      [mon("Swalot", "Leftovers", "Liquid Ooze", ["Protect", "Sludge Bomb", "Yawn", "Stockpile"], "32 HP / 32 Atk / 2 SpD"),
       mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Strength Sap>1", "Calm Mind"], ["Stockpile", "Slack Off"]], expect=r"Liquid Ooze"),
    S("magician_before_red_card", "Magician spread move into a Red Card holder + a Leftovers holder: steals before the drag?",
      [mon("Delphox", "", "Magician", ["Heat Wave", "Protect", "Psychic", "Agility"], "32 SpA / 32 Spe / 2 HP", "Timid"),
       CRESSELIA] + BENCH,
      [mon("Hippowdon", "Red Card", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"]),
       mon("Snorlax", "Leftovers", "Thick Fat", ["Curse", "Protect", "Rest", "Body Slam"], "32 HP / 32 Def / 2 SpD")] + BENCH_B,
      [["Heat Wave", "Calm Mind"], ["Slack Off", "Curse"]], expect=r"Magician"),
    S("curse_after_trick_or_treat", "Curse chosen as a non-Ghost, Ghost by Trick-or-Treat when it moves: whom does it curse (foe mid-Bounce)?",
      [mon("Gourgeist-Small", "Leftovers", "Frisk", ["Trick-or-Treat", "Protect", "Shadow Ball", "Will-O-Wisp"], "32 Spe / 32 HP / 2 Def", "Timid"),
       mon("Sharpedo", "Psychic Seed", "Rough Skin", ["Bounce", "Crunch", "Protect", "Focus Energy"], "32 Atk / 32 Spe / 2 HP", "Jolly")] + BENCH,
      [mon("Meganium", "Leftovers", "Overgrow", ["Curse", "Protect", "Leaf Storm", "Helping Hand"], "32 HP / 32 Def / 2 SpD", "Relaxed"),
       mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Trick-or-Treat>1", "Bounce>1"], ["Curse", "Slack Off"]], expect=r"Curse"),
    S("trace_gastro_acided_foe", "Trace into a foe whose Intimidate is suppressed by Gastro Acid: copied and working?",
      [mon("Arbok", "Leftovers", "Intimidate", ["Glare", "Protect", "Sludge Bomb", "Coil"]), CRESSELIA] + BENCH,
      [mon("Swalot", "Leftovers", "Liquid Ooze", ["Gastro Acid", "Protect", "Sludge Bomb", "Yawn"], "32 HP / 32 Spe / 2 Def", "Timid"),
       mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"]),
       mon("Gardevoir", "Leftovers", "Trace", ["Psychic", "Protect", "Moonblast", "Calm Mind"]), SKARMORY],
      [["Coil", "Calm Mind"], ["Gastro Acid>1", "Slack Off"], ["Coil", "Calm Mind"], ["Protect", "switch 3"]], expect=r"Trace"),
    S("skill_swap_gastro_acided", "Skill Swap with a Gastro-Acided Intimidate user: what does each side end up with?",
      [mon("Arbok", "Leftovers", "Intimidate", ["Glare", "Protect", "Sludge Bomb", "Coil"]),
       mon("Hatterene", "Leftovers", "Magic Bounce", ["Skill Swap", "Protect", "Psychic", "Calm Mind"])] + BENCH,
      [mon("Swalot", "Leftovers", "Liquid Ooze", ["Gastro Acid", "Protect", "Sludge Bomb", "Yawn"], "32 HP / 32 Spe / 2 Def", "Timid"),
       mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Coil", "Calm Mind"], ["Gastro Acid>1", "Slack Off"], ["Coil", "Skill Swap>-1"], ["Protect", "Earthquake"],
       ["switch 3", "Psychic>1"], ["Protect", "Slack Off"]], expect=r"Skill Swap"),
    S("choice_lock_magic_room", "Choice-locked under Magic Room: may it pick another move on turn 2?",
      [mon("Alakazam", "Choice Scarf", "Magic Guard", ["Psychic", "Shadow Ball", "Focus Blast", "Dazzling Gleam"],
           "32 SpA / 32 Spe / 2 HP", "Timid"),
       mon("Meowstic", "Leftovers", "Prankster", ["Magic Room", "Protect", "Psychic", "Light Screen"])] + BENCH,
      [SNORLAX, mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Psychic>2", "Magic Room"], ["Curse", "Slack Off"], ["Shadow Ball>1", "Light Screen"], ["Curse", "Slack Off"]],
      expect=r"Magic Room"),
    S("moody_all_raised", "Moody when no stat can go up (all +6): only a drop",
      [mon("Glalie", "Leftovers", "Moody", ["Protect", "Ice Beam", "Substitute", "Freeze-Dry"]), CRESSELIA] + BENCH,
      [SNORLAX, mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Protect", "Calm Mind"], ["Curse", "Slack Off"]],
      calls=[("set_mon", (0, 0), {"boosts": {"atk": 6, "def": 6, "spa": 6, "spd": 6, "spe": 6}})], expect=r"Moody"),
    S("moody_all_lowered", "Moody when no stat can go down (all -6): only a raise",
      [mon("Glalie", "Leftovers", "Moody", ["Protect", "Ice Beam", "Substitute", "Freeze-Dry"]), CRESSELIA] + BENCH,
      [SNORLAX, mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Protect", "Calm Mind"], ["Curse", "Slack Off"]],
      calls=[("set_mon", (0, 0), {"boosts": {"atk": -6, "def": -6, "spa": -6, "spd": -6, "spe": -6}})], expect=r"Moody"),
    S("sparkling_aria_shield_dust", "Sparkling Aria on two burned foes, one with Shield Dust: whose burn is cured?",
      [mon("Primarina", "Mystic Water", "Torrent", ["Sparkling Aria", "Protect", "Moonblast", "Calm Mind"], "32 SpA / 32 HP / 2 Def",
           "Modest"), CRESSELIA] + BENCH,
      [mon("Vivillon", "Leftovers", "Shield Dust", ["Quiver Dance", "Protect", "Hurricane", "Sleep Powder"], "32 HP / 32 SpD / 2 Def"),
       SNORLAX] + BENCH_B,
      [["Sparkling Aria", "Calm Mind"], ["Quiver Dance", "Curse"]],
      calls=[("set_mon", (1, 0), {"status": "brn"}), ("set_mon", (1, 1), {"status": "brn"})], expect=r"Sparkling Aria"),
    # Showdown 2026-10-17 update (Champions): No Retreat's trap is a plain 'trapped' volatile; Sheer Force no longer
    # stops Emergency Exit; Future Sight no longer removes Endure
    S("noretreat_after_meanlook", "No Retreat while Mean Looked: no own trap, so it may switch once the Mean Look user leaves",
      [mon("Falinks", "Leftovers", "Defiant", ["No Retreat", "Iron Head", "Protect", "Close Combat"], "32 HP / 32 Atk / 2 Def",
           "Adamant"), CRESSELIA] + BENCH,
      [mon("Umbreon", "Leftovers", "Inner Focus", ["Mean Look", "Foul Play", "Protect", "Wish"]), mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Iron Head>2", "Calm Mind"], ["Mean Look>1", "Slack Off"], ["No Retreat", "Calm Mind"], ["Protect", "Slack Off"],
       ["No Retreat", "Calm Mind"], ["switch 3", "Slack Off"], ["switch 3", "Calm Mind"], ["Protect", "Slack Off"]],
      expect=r"No Retreat"),
    S("noretreat_traps", "No Retreat alone traps its user (switch refused), and can't be used twice",
      [mon("Falinks", "Leftovers", "Defiant", ["No Retreat", "Iron Head", "Protect", "Close Combat"], "32 HP / 32 Atk / 2 Def",
           "Adamant"), CRESSELIA] + BENCH,
      [SNORLAX, mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["No Retreat", "Calm Mind"], ["Curse", "Slack Off"], ["No Retreat", "Calm Mind"], ["Curse", "Slack Off"],
       ["switch 3", "Calm Mind"], ["Curse", "Slack Off"]],
      expect=r"No Retreat"),
    S("emergency_exit_sheer_force", "a Sheer Force-boosted hit drops Golisopod below half: Emergency Exit now triggers",
      [mon("Golisopod", "Leftovers", "Emergency Exit", ["First Impression", "Liquidation", "Protect", "Aqua Jet"]), CRESSELIA] + BENCH,
      [mon("Mawile", "Leftovers", "Sheer Force", ["Iron Head", "Play Rough", "Protect", "Swords Dance"], "32 Atk / 32 HP / 2 Def",
           "Adamant"), mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Liquidation>2", "Calm Mind"], ["Play Rough>1", "Slack Off"]],
      calls=[("set_mon", (0, 0), {"hp_frac": 0.53})], expect=r"Emergency Exit"),
    S("endure_vs_future_sight", "Endure on the turn Future Sight lands: it now holds",
      [mon("Snorlax", "Leftovers", "Thick Fat", ["Endure", "Body Slam", "Protect", "Rest"]), CRESSELIA] + BENCH,
      [mon("Reuniclus", "Leftovers", "Magic Guard", ["Future Sight", "Calm Mind", "Protect", "Recover"], "32 SpA / 32 HP / 2 Def",
           "Modest"), mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Body Slam>2", "Calm Mind"], ["Future Sight>1", "Slack Off"], ["Body Slam>2", "Calm Mind"], ["Calm Mind", "Slack Off"],
       ["Endure", "Calm Mind"], ["Calm Mind", "Slack Off"]],
      calls=[("set_mon", (0, 0), {"hp_frac": 0.08})], expect=r"Future Sight"),
    # coverage gaps (2026-10-18): legal paths the fuzzers never ran
    S("magnetic_flux_plus_minus", "Magnetic Flux boosts the Plus user and its Minus partner",
      [mon("Ampharos", "Leftovers", "Plus", ["Magnetic Flux", "Thunderbolt", "Protect", "Dazzling Gleam"]),
       mon("Manectric", "Leftovers", "Minus", ["Protect", "Thunderbolt", "Snarl", "Volt Switch"])] + BENCH,
      [SNORLAX, mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Magnetic Flux", "Protect"], ["Curse", "Slack Off"]], expect=r"\|-boost\|p1b"),
    S("magic_bounce_into_volt_absorb", "Thunder Wave from a Volt Absorb Jolteon bounced by Magic Bounce: absorbed by Jolteon",
      [mon("Jolteon", "Leftovers", "Volt Absorb", ["Thunder Wave", "Thunderbolt", "Protect", "Calm Mind"]), CRESSELIA] + BENCH,
      [mon("Espeon", "Leftovers", "Magic Bounce", ["Calm Mind", "Psychic", "Protect", "Dazzling Gleam"]), SNORLAX] + BENCH_B,
      [["Thunder Wave>1", "Calm Mind"], ["Calm Mind", "Curse"]], calls=[("set_mon", (0, 0), {"hp_frac": 0.6})],
      expect=r"Magic Bounce"),
    S("snowscape_icy_rock", "Snowscape with Icy Rock lasts 8 turns",
      [mon("Glaceon", "Icy Rock", "Snow Cloak", ["Snowscape", "Ice Beam", "Protect", "Calm Mind"]), CRESSELIA] + BENCH,
      [SNORLAX, mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Snowscape", "Calm Mind"], ["Curse", "Slack Off"]] + [["Calm Mind", "Calm Mind"], ["Curse", "Slack Off"]] * 8,
      expect=r"\|-weather\|none"),
    S("long_reach_rocky_helmet", "Long Reach: a contact move into Rocky Helmet / Rough Skin costs nothing",
      [mon("Decidueye", "Leftovers", "Long Reach", ["Leaf Blade", "Brave Bird", "Protect", "Swords Dance"], "32 Atk / 32 HP / 2 Def",
           "Adamant"), CRESSELIA] + BENCH,
      [mon("Corviknight", "Rocky Helmet", "Pressure", ["Bulk Up", "Body Press", "Protect", "Roost"]),
       mon("Garchomp", "Leftovers", "Rough Skin", ["Swords Dance", "Earthquake", "Protect", "Dragon Claw"])] + BENCH_B,
      [["Brave Bird>1", "Calm Mind"], ["Bulk Up", "Swords Dance"], ["Leaf Blade>2", "Calm Mind"], ["Bulk Up", "Swords Dance"]],
      expect=r"Brave Bird"),
    S("magic_bounce_soak_water_absorb", "Soak from a Water Absorb Araquanid bounced by Magic Bounce: Araquanid heals instead",
      [mon("Araquanid", "Leftovers", "Water Absorb", ["Soak", "Liquidation", "Protect", "Wide Guard"]), CRESSELIA] + BENCH,
      [mon("Espeon", "Leftovers", "Magic Bounce", ["Calm Mind", "Psychic", "Protect", "Dazzling Gleam"]), SNORLAX] + BENCH_B,
      [["Soak>1", "Calm Mind"], ["Calm Mind", "Curse"]], calls=[("set_mon", (0, 0), {"hp_frac": 0.6})],
      expect=r"Magic Bounce"),
    S("magic_bounce_cotton_spore", "Cotton Spore (spread) bounced by Magic Bounce hits the user's side: Grass Whimsicott immune, Weavile -2",
      [mon("Whimsicott", "Leftovers", "Chlorophyll", ["Cotton Spore", "Moonblast", "Protect", "Tailwind"]),
       mon("Weavile", "Leftovers", "Pressure", ["Calm Mind", "Ice Beam", "Protect", "Reflect"])] + BENCH,
      [mon("Hatterene", "Leftovers", "Magic Bounce", ["Calm Mind", "Psychic", "Protect", "Dazzling Gleam"]),
       mon("Roserade", "Leftovers", "Natural Cure", ["Calm Mind", "Giga Drain", "Protect", "Sludge Bomb"])] + BENCH_B,
      [["Cotton Spore", "Calm Mind"], ["Calm Mind", "Calm Mind"]], expect=r"Magic Bounce"),
    S("magic_bounce_good_as_gold", "Confuse Ray from Gholdengo bounced by Magic Bounce: Good as Gold blocks it",
      [mon("Gholdengo", "Leftovers", "Good as Gold", ["Confuse Ray", "Make It Rain", "Protect", "Nasty Plot"]), CRESSELIA] + BENCH,
      [mon("Espeon", "Leftovers", "Magic Bounce", ["Calm Mind", "Psychic", "Protect", "Dazzling Gleam"]), SNORLAX] + BENCH_B,
      [["Confuse Ray>1", "Calm Mind"], ["Calm Mind", "Curse"]], expect=r"Magic Bounce"),
    S("reflect_type_typeless_added", "Reflect Type on a Burn Up'd (typeless) target with Forest's Curse Grass: user becomes Normal + Grass",
      [mon("Gengar", "Leftovers", "Cursed Body", ["Reflect Type", "Shadow Ball", "Protect", "Sludge Bomb"]),
       mon("Trevenant", "Leftovers", "Natural Cure", ["Forest's Curse", "Wood Hammer", "Protect", "Trick Room"])] + BENCH,
      [mon("Arcanine", "Leftovers", "Intimidate", ["Burn Up", "Extreme Speed", "Protect", "Will-O-Wisp"]), SNORLAX] + BENCH_B,
      [["Sludge Bomb>2", "Protect"], ["Burn Up>1", "Curse"], ["Sludge Bomb>2", "Forest's Curse>1"], ["Will-O-Wisp>2", "Curse"],
       ["Reflect Type>1", "Wood Hammer>2"], ["Extreme Speed>2", "Curse"], ["Sludge Bomb>2", "Wood Hammer>2"],
       ["Extreme Speed>1", "Body Slam>1"]],
      expect=r"p1a: m0\|typechange\|Normal\|\[silent\]"),
    S("pollen_puff_ally_protect", "Pollen Puff on a Protecting ally: blocked, no heal",
      [mon("Vivillon", "Leftovers", "Friend Guard", ["Pollen Puff", "Protect", "Hurricane", "Quiver Dance"]), CRESSELIA] + BENCH,
      [SNORLAX, mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Pollen Puff>-2", "Protect"], ["Curse", "Slack Off"]], calls=[("set_mon", (0, 1), {"hp_frac": 0.5})],
      expect=r"\|-activate\|p1b: m1\|move: Protect"),
    S("pollen_puff_ally_telepathy", "Pollen Puff on a Telepathy ally: the move fails, no heal",
      [mon("Vivillon", "Leftovers", "Friend Guard", ["Pollen Puff", "Protect", "Hurricane", "Quiver Dance"]),
       mon("Gardevoir", "Leftovers", "Telepathy", ["Calm Mind", "Moonblast", "Protect", "Psychic"])] + BENCH,
      [SNORLAX, mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Pollen Puff>-2", "Calm Mind"], ["Curse", "Slack Off"]], calls=[("set_mon", (0, 1), {"hp_frac": 0.5})],
      expect=r"Pollen Puff"),
    S("pollen_puff_heal_blocked_user", "Pollen Puff on an ally by a user under Psychic Noise's Heal Block",
      [mon("Vivillon", "Leftovers", "Friend Guard", ["Pollen Puff", "Protect", "Hurricane", "Quiver Dance"]), CRESSELIA] + BENCH,
      [mon("Gardevoir", "Leftovers", "Synchronize", ["Psychic Noise", "Moonblast", "Protect", "Calm Mind"], "32 SpA / 32 Spe / 2 HP",
           "Timid"), SNORLAX] + BENCH_B,
      [["Protect", "Calm Mind"], ["Psychic Noise>1", "Curse"], ["Pollen Puff>-2", "Calm Mind"], ["Calm Mind", "Curse"]],
      calls=[("set_mon", (0, 1), {"hp_frac": 0.5})], expect=r"Pollen Puff"),
    S("pollen_puff_ally_eject_button", "Pollen Puff heals an Eject Button ally: the button fires",
      [mon("Vivillon", "Leftovers", "Friend Guard", ["Pollen Puff", "Protect", "Hurricane", "Quiver Dance"]),
       mon("Cresselia", "Eject Button", "Levitate", ["Calm Mind", "Moonblast", "Protect", "Moonlight"])] + BENCH,
      [SNORLAX, mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Pollen Puff>-2", "Calm Mind"], ["Curse", "Slack Off"]], calls=[("set_mon", (0, 1), {"hp_frac": 0.5})],
      expect=r"Eject Button"),
    S("red_card_rough_skin_ko", "a contact move into a Rough Skin Red Card holder: Rough Skin KOs the attacker, no drag",
      [mon("Lycanroc-Midnight", "Leftovers", "No Guard", ["Throat Chop", "Rock Slide", "Protect", "Swords Dance"], "32 Atk / 32 HP / 2 Def",
           "Adamant"), CRESSELIA] + BENCH,
      [mon("Garchomp", "Red Card", "Rough Skin", ["Swords Dance", "Earthquake", "Protect", "Dragon Claw"]), SNORLAX] + BENCH_B,
      [["Throat Chop>1", "Calm Mind"], ["Swords Dance", "Curse"]], calls=[("set_mon", (0, 0), {"hp_frac": 0.05})],
      expect=r"Rough Skin"),
    S("ally_switch_charge_own_slot", "Meteor Beam aimed at the partner's slot, partner Ally Switches first: the charge move fails",
      [mon("Ampharos", "Leftovers", "Static", ["Meteor Beam", "Thunderbolt", "Protect", "Dazzling Gleam"]),
       mon("Musharna", "Leftovers", "Telepathy", ["Ally Switch", "Psychic", "Protect", "Calm Mind"])] + BENCH,
      [SNORLAX, mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Meteor Beam>-2", "Ally Switch"], ["Curse", "Slack Off"]], expect=r"Ally Switch"),
    S("balloon_pop_recycle", "a popped Air Balloon isn't the holder's lastItem: Recycle fails next turn",
      [mon("Snorlax", "Air Balloon", "Thick Fat", ["Recycle", "Body Slam", "Protect", "Curse"], "32 HP / 32 Def / 2 SpD", "Relaxed"),
       CRESSELIA] + BENCH,
      [mon("Gholdengo", "Leftovers", "Good as Gold", ["Nasty Plot", "Make It Rain", "Protect", "Recover"]),
       mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Curse", "Calm Mind"], ["Make It Rain", "Slack Off"], ["Recycle", "Calm Mind"], ["Nasty Plot", "Slack Off"],
       ["Curse", "Calm Mind"], ["Nasty Plot", "Earthquake"]],
      expect=r"\|-enditem\|p1a: m0\|Air Balloon"),
    S("balloon_behind_substitute", "a hit on the Substitute pops the holder's Air Balloon (onAfterSubDamage)",
      [mon("Snorlax", "Air Balloon", "Thick Fat", ["Substitute", "Body Slam", "Protect", "Curse"], "32 HP / 32 Def / 2 SpD", "Relaxed"),
       CRESSELIA] + BENCH,
      [mon("Gholdengo", "Leftovers", "Good as Gold", ["Nasty Plot", "Make It Rain", "Protect", "Recover"]),
       mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Substitute", "Calm Mind"], ["Nasty Plot", "Slack Off"], ["Curse", "Calm Mind"], ["Make It Rain", "Slack Off"],
       ["Curse", "Calm Mind"], ["Nasty Plot", "Earthquake"]],
      expect=r"\|-enditem\|p1a: m0\|Air Balloon"),
    S("libero_swapped_back", "Libero fires again after Skill Swap takes it away and gives it back (new abilityState): Fire, not Poison, vs Psychic",
      [mon("Cinderace", "Leftovers", "Libero", ["Gunk Shot", "Pyro Ball", "Bulk Up", "Protect"]), CRESSELIA] + BENCH,
      [mon("Banette", "Leftovers", "Frisk", ["Skill Swap", "Protect", "Shadow Claw", "Taunt"]),
       mon("Slowbro", "Leftovers", "Oblivious", ["Psychic", "Protect", "Slack Off", "Calm Mind"])] + BENCH_B,
      [["Gunk Shot>2", "Calm Mind"], ["Protect", "Calm Mind"], ["Bulk Up", "Calm Mind"], ["Skill Swap>1", "Psychic>2"],
       ["Bulk Up", "Calm Mind"], ["Skill Swap>1", "Psychic>2"], ["Pyro Ball>2", "Calm Mind"], ["Protect", "Protect"],
       ["Bulk Up", "Calm Mind"], ["Protect", "Psychic>1"]],
      expect=r"\|-start\|p1a: m0\|typechange\|Fire"),
    S("throat_chop_twice", "Throat Chop on an already-chopped target (no refresh)",
      [mon("Umbreon", "Leftovers", "Inner Focus", ["Throat Chop", "Foul Play", "Protect", "Wish"]), CRESSELIA] + BENCH,
      [mon("Primarina", "Leftovers", "Torrent", ["Hyper Voice", "Moonblast", "Protect", "Calm Mind"]), SNORLAX] + BENCH_B,
      [["Throat Chop>1", "Calm Mind"], ["Calm Mind", "Curse"], ["Throat Chop>1", "Calm Mind"], ["Calm Mind", "Curse"],
       ["Protect", "Calm Mind"], ["Hyper Voice", "Curse"], ["Protect", "Calm Mind"], ["Hyper Voice", "Curse"]],
      expect=r"Throat Chop"),
    S("sand_ko_ends_before_yawn", "sandstorm KOs the last Pokemon: the battle ends before a pending Yawn puts anyone to sleep",
      [mon("Snorlax", "Shed Shell", "Thick Fat", ["Protect", "Body Slam", "Curse", "Rest"]), CRESSELIA] + BENCH,
      [mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"]),
       mon("Cresselia", "Leftovers", "Levitate", ["Calm Mind", "Moonblast", "Protect", "Moonlight"])] + BENCH_B,
      [["Protect", "pass"], ["Yawn>-2", "Calm Mind"], ["Curse", "pass"], ["Slack Off", "Calm Mind"]],
      calls=[("set_field", (), {"weather": "sand", "weather_turns": 5}), ("set_mon", (0, 0), {"hp_frac": 0.09})] + [("set_mon", (0, k), {"fainted": True}) for k in (1, 2, 3)],
      expect=r"\|win\|"),
    S("stealth_rock_switch_in", "Stealth Rock damage on switch-ins (Charizard 4x: 50%, Skarmory neutral)",
      [mon("Garchomp", "Leftovers", "Rough Skin", ["Stealth Rock", "Earthquake", "Protect", "Dragon Claw"]), CRESSELIA] + BENCH,
      [SNORLAX, mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"]),
       mon("Charizard", "Leftovers", "Blaze", ["Protect", "Heat Wave", "Air Slash", "Roost"]), SKARMORY],
      [["Stealth Rock", "Calm Mind"], ["Curse", "Protect"], ["Protect", "Calm Mind"], ["switch 4", "switch 3"],
       ["Protect", "Calm Mind"], ["Iron Defense", "Roost"]],
      expect=r"Stealth Rock"),
    # (2026-11-05) from another bot maker's report on just_a_simulator (their Showdown pin is older than ours)
    S("fakeout_onemove_turn2", "a Pokemon whose only move is Fake Out, past its first turn",
      [mon("Incineroar", "Sitrus Berry", "Intimidate", ["Fake Out"]), CRESSELIA] + BENCH,
      [SNORLAX, mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Fake Out>1", "Calm Mind"], ["Curse", "Slack Off"], ["Fake Out>1", "Calm Mind"], ["Curse", "Slack Off"]],
      expect=r"\|move\|p1a: m0\|(Fake Out|Struggle)", ill_ok=True),
    S("fakeout_scarf_locked_turn2", "Choice Scarf locked into Fake Out, past its first turn",
      [mon("Incineroar", "Choice Scarf", "Intimidate", ["Fake Out", "Flare Blitz", "Knock Off", "Parting Shot"]), CRESSELIA] + BENCH,
      [SNORLAX, mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"])] + BENCH_B,
      [["Fake Out>1", "Calm Mind"], ["Curse", "Slack Off"], ["Fake Out>1", "Calm Mind"], ["Curse", "Slack Off"]],
      expect=r"\|move\|p1a: m0\|(Fake Out|Struggle)", ill_ok=True),
    S("redcard_spread_fastest", "a spread move hits two Red Card holders (the user's partner, slow; a foe, fast): the FASTER card fires",
      [mon("Heliolisk", "Leftovers", "Dry Skin", ["Brutal Swing", "Thunderbolt", "Protect", "Volt Switch"], "32 Atk / 32 Spe / 2 HP", "Jolly"),
       mon("Snorlax", "Red Card", "Thick Fat", ["Curse", "Body Slam", "Protect", "Rest"], "32 HP / 32 Def / 2 SpD", "Relaxed")] + BENCH,
      [mon("Jolteon", "Red Card", "Volt Absorb", ["Protect", "Thunderbolt", "Calm Mind", "Shadow Ball"], "32 HP / 32 Spe / 2 SpD", "Timid"),
       CRESSELIA] + BENCH_B,
      [["Brutal Swing", "Curse"], ["Calm Mind", "Calm Mind"], ["Protect", "Body Slam>1"], ["Calm Mind", "Calm Mind"]],
      expect=r"\|-enditem\|p2a: m0\|Red Card"),
    S("clanging_scales_battle_end", "Clanging Scales KOs the last two foes: the battle ends, no Defense drop",
      [mon("Kommo-o", "Leftovers", "Bulletproof", ["Clanging Scales", "Protect", "Close Combat", "Iron Defense"], "32 SpA / 32 Spe / 2 HP",
           "Modest"), CRESSELIA] + BENCH,
      [mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"]), SNORLAX] + BENCH_B,
      [["Clanging Scales", "Calm Mind"], ["Slack Off", "Curse"]],
      calls=[("set_mon", (1, 0), {"hp_frac": 0.05}), ("set_mon", (1, 1), {"hp_frac": 0.05})] + [("set_mon", (1, k), {"fainted": True}) for k in (2, 3)],
      expect=r"\|win\|"),
    S("clanging_scales_ko_one", "Clanging Scales KOs one foe, the battle goes on: one Defense drop",
      [mon("Kommo-o", "Leftovers", "Bulletproof", ["Clanging Scales", "Protect", "Close Combat", "Iron Defense"], "32 SpA / 32 Spe / 2 HP",
           "Modest"), CRESSELIA] + BENCH,
      [mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"]), SNORLAX] + BENCH_B,
      [["Clanging Scales", "Calm Mind"], ["Slack Off", "Curse"], ["Protect", "Calm Mind"], ["Slack Off", "Curse"]],
      calls=[("set_mon", (1, 0), {"hp_frac": 0.05})], expect=r"\|-unboost\|p1a: m0\|def"),
    S("scale_shot_battle_end", "Scale Shot KOs the last foe: the battle ends, no Defense drop / Speed boost",
      [mon("Dragalge", "Life Orb", "Poison Touch", ["Scale Shot", "Protect", "Surf", "Haze"], "32 Atk / 32 Spe / 2 HP", "Jolly"),
       CRESSELIA] + BENCH,
      [mon("Hippowdon", "Leftovers", "Sand Stream", ["Slack Off", "Earthquake", "Protect", "Yawn"]), SNORLAX] + BENCH_B,
      [["Scale Shot>1", "Calm Mind"], ["Slack Off", "pass"]],
      calls=[("set_mon", (1, 0), {"hp_frac": 0.05})] + [("set_mon", (1, k), {"fainted": True}) for k in (1, 2, 3)],
      expect=r"\|win\|"),
]


def parse_action(text, set_text, side, team=()):
    """one slot's action -> (fast-engine tuple, Showdown choice part)"""
    t = text.strip()
    if t == "pass":
        return ("pass", 0, 0, False), "pass"
    if t.startswith("switch "):
        n = int(t.split()[1])
        return ("switch", n - 1, 0, False), f"switch {n}"
    mega = t.endswith("*")
    t = t.rstrip("*")
    name, _, tg = t.partition(">")
    k = None
    for st in [set_text] + list(team):        # the slot's Pokemon first; a forced replacement's moves come from the team
        ids = [E.to_id(ln[2:].strip()) for ln in st.split("\n") if ln.startswith("- ")]
        if E.to_id(name) in ids:
            k = ids.index(E.to_id(name))
            break
    if k is None:
        raise ValueError(f"no Pokemon on side {side} has {name}")
    target = int(tg) if tg else 0
    sd = f"move {k + 1}" + (f" {target}" if target else "") + (" mega" if mega else "")
    return ("move", k, target, mega), sd


def joint(sc, turn_actions, slots_sets):
    out_fast, out_sd = [], []
    for side in range(2):
        f, s = [], []
        for slot in range(2):
            a, b = parse_action(turn_actions[side][slot], slots_sets[side][slot], side, (sc["a"], sc["b"])[side])
            f.append(a)
            s.append(b)
        out_fast.append(f)
        out_sd.append(", ".join(s))
    return out_fast, out_sd


def play_fast(base, fast_turns, seeds):
    rows, illegal = [], []
    for k in range(seeds):
        c = base.copy()
        c.reseed(11 + k * 1000003)
        for t, acts in enumerate(fast_turns):
            if c.phase == "ended":
                break
            la = [c.legal_actions(0), c.legal_actions(1)]
            use = []
            for side in range(2):
                pair = []
                for slot in range(2):
                    a = tuple(acts[side][slot])
                    opts = la[side][slot] if slot < len(la[side]) else []
                    if opts and a not in opts and a[0] == "move" and a[2] == 0:
                        # no target given: the engine's own encoding for that move (Curse: random foe, spread moves...)
                        same = [x for x in opts if x[0] == "move" and x[1] == a[1] and x[3] == a[3]]
                        if same:
                            a = next((x for x in same if x[2] == 1), same[0])
                    if opts and a not in opts:
                        illegal.append((t, side, slot))
                        a = next((x for x in opts if x[0] == "move" and not x[3]), opts[0])   # Showdown 'default': first move
                    elif not opts:
                        a = ("pass", 0, 0, False)
                    pair.append(a)
                use.append(pair)
            t0 = c.turn
            c.choose(0, *use[0])
            c.choose(1, *use[1])
            M.resolve(c, t0)
        rows.append(M.finish(E.outcome(c, "old"), c.phase == "ended"))
    return rows, illegal


def run(sc, seeds, show=False):
    texts = ["\n\n".join(sc["a"]), "\n\n".join(sc["b"])]
    old = E.build("old", texts, sc["turn"], sc["calls"])
    sets_by_slot = [list(sc["a"][:2]), list(sc["b"][:2])]
    fast_turns, sd_turns = [], []
    for k in range(0, len(sc["script"]), 2):
        acts = [sc["script"][k], sc["script"][k + 1]]
        f, s = joint(sc, acts, sets_by_slot)
        fast_turns.append(f)
        sd_turns.append(s)
        for side in range(2):                         # a switch puts that Pokemon's moveset in the slot for later turns
            for slot in range(2):
                if acts[side][slot].startswith("switch "):
                    sets_by_slot[side][slot] = (sc["a"], sc["b"])[side][int(acts[side][slot].split()[1]) - 1]
    fo, ill_fast = play_fast(old, fast_turns, seeds)
    req = {"id": 0, "texts": texts, "turn": sc["turn"], "calls": [[n, list(a), kw] for n, a, kw in sc["calls"]],
           "choices": sd_turns[0], "script": sd_turns[1:], "seeds": seeds, "seed0": 11, "turns": len(sd_turns), "log": seeds}
    resp = E.truth_request(req)
    if resp.get("error"):
        return {"name": sc["name"], "error": resp["error"]}
    ft = [M.finish(r, r.get("ended", 0) == 1) for r in resp["rows"]]
    logs = resp.get("logs", [])
    hit = sum(1 for lg in logs if sc["expect"] and re.search(sc["expect"], "\n".join(lg))) if sc["expect"] else None
    out = {"name": sc["name"], "diffs": E.compare(fo, ft), "expect_seeds": hit, "seeds": seeds,
           "illegal_fast": sorted(set(ill_fast)), "illegal_showdown": sorted({tuple(x) for x in resp.get("illegal_script", [])}),
           "illegal_t1": resp.get("illegal", 0) - len(resp.get("illegal_script", []))}
    if show:
        print("\n".join(l for l in logs[0] if not l.startswith(("|t:", "|split", "|uhtml"))))
        c = old.copy()
        c.enable_log()
        c.reseed(11)
        for acts in fast_turns:
            if c.phase == "ended":
                break
            t0 = c.turn
            c.choose(0, *acts[0])
            c.choose(1, *acts[1])
            M.resolve(c, t0)
        print("---- fast engine ----")
        print("\n".join(c.log()))
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--seeds", type=int, default=200)
    ap.add_argument("--only", default="")
    ap.add_argument("--show", default="")
    a = ap.parse_args()
    M.init((), True)
    only = {x for x in a.only.split(",") if x}
    if a.show:
        only = {a.show}
    bad = 0
    for sc in SCENARIOS:
        if only and sc["name"] not in only:
            continue
        r = run(sc, a.seeds, show=bool(a.show))
        if "error" in r:
            print(f"ERROR  {sc['name']}: {r['error'][:300]}")
            bad += 1
            continue
        ok_expect = r["expect_seeds"] is None or r["expect_seeds"] > 0
        legal_same = r["illegal_fast"] == [tuple(x) + (0,) for x in []] or True
        ill = ""
        if r["illegal_fast"] or r["illegal_showdown"] or r["illegal_t1"]:
            ill = f"  illegal: fast {r['illegal_fast']} showdown {r['illegal_showdown']} turn-1 {r['illegal_t1']}"
        status = "DIFF" if r["diffs"] else ("NOT EXERCISED" if not ok_expect else "ok")
        # an illegal choice is fine when both engines refuse the same one (e.g. a switch while trapped)
        same_ill = {(x[0], x[1]) for x in r["illegal_fast"]} == {tuple(x[:2]) for x in r["illegal_showdown"]} and not r["illegal_t1"]
        if status != "ok" or (ill and not same_ill and not sc.get("ill_ok")):
            bad += 1
        print(f"{status:<14}{sc['name']:<30} expect in {r['expect_seeds']}/{r['seeds']} seeds{ill}")
        for d in r["diffs"][:6]:
            print(f"      {d}")
    print(f"{bad} scenario(s) need a look")


if __name__ == "__main__":
    main()
