"""(2026-10-16) Print Showdown's definition of moves / abilities / items (champions-mod overrides included), comments and
display-only fields stripped.   python tools/sd_def.py move torment spite   |   python tools/sd_def.py ability shedskin   |  item metronome"""
import re
import sys

import os
SD = os.path.join(os.environ.get("SHOWDOWN_DIR", "../pokemon-showdown"), "data")
kind = sys.argv[1]
file = {"move": "moves.ts", "ability": "abilities.ts", "item": "items.ts", "cond": "conditions.ts"}[kind]
for name in sys.argv[2:]:
    for base in (SD, SD + "/mods/champions"):
        try:
            t = open(f"{base}/{file}", encoding="utf-8").read()
        except OSError:
            continue
        i = t.find("\n\t" + name + ": {")
        if i < 0:
            continue
        j = t.find("\n\t},", i)
        blk = t[i:j + 4]
        blk = "\n".join(l for l in blk.split("\n") if not re.match(r"\s*(num|contestType|isNonstandard|pp|priority|zMove|noPPBoosts|rating|spritenum|gen|fling)\b", l))
        print(("[champions mod] " if "champions" in base else "") + blk)
