"""Which hooks of the moves / abilities / items used in the team pool ($ED_TEAMS) (Showdown code) have NO handler in the
reference engine (src/sim/handlers/*.rs lookup tables)? Weighted by how many team sets use them.
A missing handler makes the reference engine silently skip that effect, so engine_diff blames the fast engine."""
import glob, json, os, re
from collections import Counter
HERE = os.path.dirname(__file__)
ROOT = os.path.join(HERE, "..")
d = json.load(open(os.path.join(HERE, "..", "data", "dex.json"), encoding="utf-8"))
have = set()
for f in glob.glob(os.path.join(HERE, "..", "src", "sim", "handlers", "*.rs")):
    have |= set(re.findall(r'\("([a-z0-9]+)", "([A-Za-z.]+)"\)', open(f, encoding="utf-8").read()))
# hooks the reference engine handles generically or that only print / affect things we never model
IGNORE = {"onSideResidual", "onSideEnd", "onEnd", "onDisableMove", "onPrepareHit"}
def to_id(x): return re.sub(r"[^a-z0-9]", "", x.lower())
use = {"move": Counter(), "ability": Counter(), "item": Counter()}
for f in glob.glob(os.path.join(ROOT, os.environ.get("ED_TEAMS", "pools/Teams_cov"), "*.txt")):
    for block in open(f, encoding="utf-8").read().replace("\r\n", "\n").split("\n\n"):
        for l in block.split("\n"):
            l = l.strip()
            if l.startswith("- "): use["move"][to_id(l[2:])] += 1
            elif l.startswith("Ability:"): use["ability"][to_id(l[8:])] += 1
            elif "@" in l and not l.startswith("-"): use["item"][to_id(l.split("@", 1)[1])] += 1
# abilities gained by Mega Evolution count with the stone's uses
for it, n in list(use["item"].items()):
    for mega in ((d["items"].get(it) or {}).get("megaStone") or {}).values():
        sp = d["species"].get(to_id(mega))
        for ab in (sp or {}).get("abilities", {}).values():
            use["ability"][to_id(ab)] += n
table = {"move": d["moves"], "ability": d["abilities"], "item": d["items"]}
for kind in ("move", "ability", "item"):
    rows = []
    for k, n in use[kind].most_common():
        e = table[kind].get(k)
        if not e: continue
        miss = []
        for h in e.get("hooks", []):
            hh = h[len("condition."):] if h.startswith("condition.") else h
            if hh.split(".")[-1].endswith("Priority") or hh.split(".")[-1].endswith("Order") or hh.split(".")[-1].endswith("SubOrder"):
                continue
            if hh in IGNORE or (k, hh) in have or (k, h) in have:
                continue
            miss.append(hh)
        if miss:
            rows.append(f"{k} {n} [{','.join(miss)}]")
    print(f"== {kind}s with hooks missing from the reference engine ({len(rows)}):")
    for r in rows: print("  " + r)
