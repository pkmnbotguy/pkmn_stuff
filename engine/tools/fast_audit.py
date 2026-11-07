"""Which moves / abilities / items used in the team pool ($ED_TEAMS) (Mega abilities counted with their stone) have Showdown hooks but are
never referenced by the fast engine (an `Mv::X` / `Ab::X` / `It::X` use outside effects.rs, or the quoted id)?"""
import glob, json, os, re
from collections import Counter
HERE = os.path.dirname(__file__)
ROOT = os.path.join(HERE, "..")
SRC = os.path.join(HERE, "..", "src")
d = json.load(open(os.path.join(HERE, "..", "data", "dex.json"), encoding="utf-8"))
eff = open(os.path.join(SRC, "effects.rs"), encoding="utf-8").read()
code = "".join(open(os.path.join(SRC, f), encoding="utf-8").read() for f in ("battle.rs", "damage.rs", "dex.rs", "state.rs"))
variant = {}
for ids, v in re.findall(r'((?:"[a-z0-9]+"\s*\|\s*)*"[a-z0-9]+")\s*=>\s*([A-Za-z0-9_]+)', eff):
    for k in re.findall(r'"([a-z0-9]+)"', ids):
        variant.setdefault(k, v)
GENERIC = {"onTakeItem", "onBasePower", "onEat", "onSourceModifyDamage"}  # type items, resist berries, Mega stones: data-driven
def used(kind, k):
    v = variant.get(k)
    pre = {"move": "Mv", "ability": "Ab", "item": "It"}[kind]
    if v and re.search(rf"\b{pre}::{v}\b", code): return True
    return f'"{k}"' in code
def to_id(x): return re.sub(r"[^a-z0-9]", "", x.lower())
use = {"move": Counter(), "ability": Counter(), "item": Counter()}
for f in glob.glob(os.path.join(ROOT, os.environ.get("ED_TEAMS", "pools/Teams_cov"), "*.txt")):
    for block in open(f, encoding="utf-8").read().replace("\r\n", "\n").split("\n\n"):
        for l in block.split("\n"):
            l = l.strip()
            if l.startswith("- "): use["move"][to_id(l[2:])] += 1
            elif l.startswith("Ability:"): use["ability"][to_id(l[8:])] += 1
            elif "@" in l and not l.startswith("-"): use["item"][to_id(l.split("@", 1)[1])] += 1
for it, n in list(use["item"].items()):
    for mega in ((d["items"].get(it) or {}).get("megaStone") or {}).values():
        for ab in (d["species"].get(to_id(mega)) or {}).get("abilities", {}).values():
            use["ability"][to_id(ab)] += n
table = {"move": d["moves"], "ability": d["abilities"], "item": d["items"]}
for kind in ("move", "ability", "item"):
    rows = []
    for k, n in use[kind].most_common():
        e = table[kind].get(k)
        if not e: continue
        hooks = [h for h in e.get("hooks", []) if not re.search(r"(Priority|Order|SubOrder)$", h)]
        if kind == "item" and set(hooks) <= GENERIC: continue
        if not hooks or used(kind, k): continue
        rows.append(f"{k} {n} [{','.join(hooks[:5])}]")
    print(f"== {kind}s with hooks the fast engine never references ({len(rows)}):")
    for r in rows: print("  " + r)
