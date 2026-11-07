"""(2026-10-16) Summarise an engine_diff --truth run: which tokens are over-represented where the FAST engine disagrees with
real Showdown, split by whether the exact engine also disagrees ('both') or agrees with Showdown ('fast only').
    python tools/truth_summary.py fuzz/truth_cov_0.json [--top 40] [--min 3]"""
import json
import sys
from collections import Counter

path = sys.argv[1]
top = int(sys.argv[sys.argv.index("--top") + 1]) if "--top" in sys.argv else 40
d = json.load(open(path, encoding="utf-8"))
flagged = d["flagged"]
fast_only = [r for r in flagged if not r.get("diffs_exact")]
both = [r for r in flagged if r.get("diffs_exact")]
print(f"flagged {len(flagged)}: fast wrong, exact right {len(fast_only)}; both differ from Showdown {len(both)}")


def toks(r):
    out = set()
    for line in r["names"]:
        head, _, act = line.partition(": ")
        ab_it = head.split("[", 1)[1].rstrip("]").split("/") if "[" in head else []
        out.update("ability:" + x for x in ab_it[:1] if x)
        out.update("item:" + x for x in ab_it[1:2] if x)
        out.add("switch:" + act.split("[", 1)[-1].rstrip("]") if act.startswith("switch to ") else "move:" + act.split(" ")[0])
    return out


for name, rows in (("FAST ONLY", fast_only), ("BOTH", both)):
    c = Counter()
    for r in rows:
        c.update(toks(r))
    print(f"--- {name}")
    print("  " + ", ".join(f"{t} {n}" for t, n in c.most_common(top)))
