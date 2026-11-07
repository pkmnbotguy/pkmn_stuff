"""Side-by-side per-battle averages from the two engines' diagnostics, biggest differences first."""
import json
import sys

a = json.load(open(sys.argv[1]))  # showdown
b = json.load(open(sys.argv[2]))  # vgcsim
n = float(sys.argv[3])
top = int(sys.argv[4]) if len(sys.argv) > 4 else 25
# damage per use, for moves (removes differences in how often a move gets picked)
print("== damage per use (moves used at least 200 times in each)")
rows = []
for k in set(a["damage"]) & set(b["damage"]):
    ua, ub = a["uses"].get(k, 0), b["uses"].get(k, 0)
    if ua >= 200 and ub >= 200:
        pa, pb = a["damage"][k][1] / ua, b["damage"][k][1] / ub
        rows.append((abs(pb - pa) / max(pa, 1e-9), k, pa, pb, ua, ub))
for rel, k, pa, pb, ua, ub in sorted(rows, reverse=True)[:top]:
    print(f"  {k:<22} {pa:8.1f} {pb:8.1f}   {(pb - pa) / pa * 100:+6.0f}%   (uses {ua}/{ub})")
for kind in ("damage", "heal", "cant", "uses"):
    keys = set(a.get(kind, {})) | set(b.get(kind, {}))
    def val(d, k):
        v = d.get(kind, {}).get(k, 0)
        return (v[1] if isinstance(v, list) else v) / n
    rows = sorted(keys, key=lambda k: -abs(val(a, k) - val(b, k)))
    print(f"== {kind} per battle (showdown vs vgcsim), biggest differences first")
    for k in rows[:top]:
        sa, sb = val(a, k), val(b, k)
        rel = (sb - sa) / sa * 100 if sa else float("inf")
        print(f"  {k:<22} {sa:9.2f} {sb:9.2f}   {rel:+6.0f}%")
