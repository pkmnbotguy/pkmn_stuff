"""
(2026-10-18) What changed in Showdown's battle rules between two builds, as far as Champions M-C is concerned.

    node tools/sd_snapshot.js <showdown folder> gen9championsvgc2026regmc old.json     (or tools/sd_ref_snapshot.sh REF)
    python tools/sd_changes.py old.json new.json [--out report.txt] [--all] [--full]

Compares two sd_snapshot.js files: every move / ability / item / condition / species (data and handler source), the type chart,
the format's mod scripts and rules, and every simulator method. Content is limited to what M-C can field (ids found in
the team pools, Mega formes of legal stones, in-battle formes of legal species); --all lists everything. A change is
"code" when it survives normalisation (comments, whitespace, semicolons, trailing commas removed), else "cosmetic" (formatting
or lint only; counted, not shown). Each code change prints a diff of the source (capped at 60 lines unless --full).
Use after every Showdown update: each listed item is either ported to the fast engine (and, if wanted, the exact engine) or
dismissed with a reason.
"""
import argparse
import difflib
import glob
import json
import os
import re

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, ".."))


def to_id(s):
    return re.sub(r"[^a-z0-9]", "", s.lower())


def legal_ids(snap):
    sp, mv, it, ab = set(), set(), set(), set()
    for folder in ("pools/Teams_cov",):
        for f in glob.glob(os.path.join(ROOT, folder, "*.txt")):
            for block in open(f, encoding="utf-8", errors="replace").read().split("\n\n"):
                lines = [l.strip() for l in block.strip().splitlines() if l.strip()]
                if not lines:
                    continue
                head = lines[0]
                name, _, item = head.partition(" @ ")
                m = re.search(r"\(([^()]{2,})\)", name)
                name = m.group(1) if m and m.group(1) not in ("M", "F") else re.sub(r"\s*\((M|F)\)\s*", "", name)
                sp.add(to_id(name))
                if item:
                    it.add(to_id(item))
                for l in lines[1:]:
                    if l.startswith("Ability:"):
                        ab.add(to_id(l[8:]))
                    elif l.startswith("-"):
                        mv.add(to_id(l[1:].split("/")[0]))
    species = snap["species"]
    base = {to_id(species[s]["baseSpecies"]) for s in sp if s in species}
    for sid, s in species.items():
        legal_form = to_id(s["baseSpecies"]) in base and (not s["requiredItem"] or to_id(s["requiredItem"]) in it
                                                          or s.get("battleOnly"))
        if legal_form:
            sp.add(sid)
            ab.update(to_id(a) for a in s["abilities"].values())
    return {"species": sp, "moves": mv, "items": it, "abilities": ab}


def norm(src):
    s = re.sub(r"/\*.*?\*/", "", src, flags=re.S)
    s = re.sub(r"(?m)(^|[^:\"'])//[^\n]*", r"\1", s)
    s = re.sub(r"\s+", "", s)
    s = s.replace(";", "")
    s = re.sub(r",([}\])])", r"\1", s)
    # lint rewrites with the same result: `a && a.b` -> `a?.b`, `a && a[...]` -> `a?.[...]` (Oxlint, Oct 2026)
    s = re.sub(r'(?<![\w.$?])([\w$]+(?:\.[\w$]+)*)&&\1(\.|\[)', lambda m: m.group(1) + ("?." if m.group(2) == "." else "?.["), s)
    s = re.sub(r'\?\.\?\.', "?.", s)
    return s


def flat(v, prefix=""):
    """{path: text} with function sources as text; lists / dicts walked."""
    out = {}
    if isinstance(v, dict) and "__fn" in v and len(v) == 1:
        out[prefix or "."] = ("fn", v["__fn"])
    elif isinstance(v, dict):
        for k in sorted(v):
            out.update(flat(v[k], f"{prefix}.{k}" if prefix else k))
    else:
        out[prefix or "."] = ("val", json.dumps(v, sort_keys=True))
    return out


def compare(a, b):
    """-> (code changes [(path, old, new)], cosmetic count)"""
    fa, fb = flat(a), flat(b)
    code, cosmetic = [], 0
    for p in sorted(set(fa) | set(fb)):
        x, y = fa.get(p), fb.get(p)
        if x == y:
            continue
        if x and y and x[0] == "fn" and y[0] == "fn" and norm(x[1]) == norm(y[1]):
            cosmetic += 1
            continue
        code.append((p, x[1] if x else None, y[1] if y else None))
    return code, cosmetic


def show(path, old, new, full):
    lines = [f"    {path}:"]
    if old is None or new is None:
        lines.append(f"      {'added' if old is None else 'removed'}: {(new or old)[:300]}")
        return lines
    if "\n" not in old and "\n" not in new and len(old) + len(new) < 400:
        lines.append(f"      - {old}\n      + {new}")
        return lines
    d = list(difflib.unified_diff(old.splitlines(), new.splitlines(), lineterm="", n=2))[2:]
    if not full and len(d) > 60:
        d = d[:60] + [f"... ({len(d) - 60} more lines; --full)"]
    lines += ["      " + l for l in d]
    return lines


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("old")
    ap.add_argument("new")
    ap.add_argument("--out")
    ap.add_argument("--all", action="store_true", help="also content M-C can't field")
    ap.add_argument("--full", action="store_true", help="uncapped diffs")
    a = ap.parse_args()
    A, B = json.load(open(a.old, encoding="utf-8")), json.load(open(a.new, encoding="utf-8"))
    legal = legal_ids(B)
    rep, summary = [], []
    for sec in ("moves", "abilities", "items", "species", "conditions", "typechart", "scripts", "format_obj", "rules", "code"):
        x, y = A.get(sec, {}), B.get(sec, {})
        if sec in ("typechart", "scripts", "format_obj"):
            x, y = {sec: x}, {sec: y}
        n_code = n_cos = n_skip = 0
        block = []
        if sec == "code":       # renamed / replaced functions in one class: show as a diff (faintMessages -> resolveFaints)
            gone = [k for k in x if k not in y]
            for k in [k for k in y if k not in x]:
                cls = k.rsplit("#", 1)[0] if "#" in k else k.rsplit(".", 1)[0]
                best = max(((difflib.SequenceMatcher(None, x[g], y[k]).ratio(), g) for g in gone
                            if g.startswith(cls)), default=(0, None))
                if best[0] > 0.5:
                    g = best[1]
                    gone.remove(g)
                    block.append(f"  {g} -> {k}: RENAMED / REPLACED (similarity {best[0]:.2f})")
                    block += show(".", x[g], y[k], a.full)
                    x = {kk: vv for kk, vv in x.items() if kk != g}
                    y = {kk: vv for kk, vv in y.items() if kk != k}
                    n_code += 1
        for k in sorted(set(x) | set(y)):
            if sec in legal and not a.all and k not in legal[sec]:
                if x.get(k) != y.get(k):
                    n_skip += 1
                continue
            if k not in x or k not in y:
                block.append(f"  {k}: {'ADDED' if k not in x else 'REMOVED'}")
                body = y.get(k) if k not in x else None
                if sec == "code" and isinstance(body, str):
                    lines = body.splitlines()
                    if not a.full and len(lines) > 60:
                        lines = lines[:60] + [f"... ({len(lines) - 60} more lines; --full)"]
                    block += ["      + " + l for l in lines]
                n_code += 1
                continue
            if sec == "code":
                ch = [] if x[k] == y[k] else ([] if norm(x[k]) == norm(y[k]) else [(".", x[k], y[k])])
                cos = int(x[k] != y[k] and not ch)
            else:
                ch, cos = compare(x[k], y[k])
            n_cos += cos
            if ch:
                n_code += 1
                block.append(f"  {k}:")
                for p, o, n in ch:
                    block += show(p, o, n, a.full)
        summary.append(f"{sec:<11} code changes {n_code:>4}   cosmetic-only parts {n_cos:>4}"
                       + (f"   (not in M-C, skipped: {n_skip})" if n_skip else ""))
        if block:
            rep += [f"== {sec}"] + block
    text = "\n".join([f"Showdown changes: {a.old} -> {a.new}", *summary, "", *rep])
    if a.out:
        open(a.out, "w", encoding="utf-8").write(text + "\n")
    print("\n".join(summary))
    print(f"\n{sum(1 for l in rep if l.startswith('  ') and not l.startswith('    '))} changed entries"
          + (f"; full report: {a.out}" if a.out else ""))
    if not a.out:
        print("\n".join(rep))


if __name__ == "__main__":
    main()
