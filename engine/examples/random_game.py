"""Play one game between two random players, step by step, and print the battle log.

Run from the repository root after building the Python module:
    python examples/random_game.py
"""
import os
import random

import vgcsim

ROOT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..")
POOL = os.path.join(ROOT, "pools", "Teams_cov")

rng = random.Random(1)
dex = vgcsim.Dex(os.path.join(ROOT, "data", "dex.json"))
files = sorted(f for f in os.listdir(POOL) if f.endswith(".txt"))
team_a, team_b = (open(os.path.join(POOL, f), encoding="utf8").read() for f in rng.sample(files, 2))

battle = vgcsim.Battle(dex, team_a, team_b, seed=rng.randrange(1 << 32))
battle.enable_log()
for side in (0, 1):
    battle.choose_team(side, rng.sample(range(6), 4))   # bring 4 of 6; the first two lead

while battle.winner is None and battle.turn < 100:
    for side in (0, 1):
        if battle.needs_choice(side):
            options = battle.legal_actions(side)          # one list of action tuples per active slot
            battle.choose(side, rng.choice(options[0]), rng.choice(options[1]))

print("\n".join(battle.log()))
print("winner:", battle.winner, "after", battle.turn, "turns")
