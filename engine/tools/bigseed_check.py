"""(2026-10-17) Settle a 'chance' flag: N fresh seeds in BOTH engines for one flagged position, every feature's z-score.

    ED_TEAMS=<pool> python tools/bigseed_check.py --out F.json --k K [--turns 5] [--n 20000] [--procs 4]
Showdown plays seeds disjoint from the fuzz / confirm / cache sets (seed0 offset); the fast engine plays n seeds of its own.
Prints the largest |z| features. A real bias of d points shows up as z ~ d / se; with n = 20000 per side se ~ 0.4-0.5 points."""
import argparse
import json
import math
import multiprocessing as mp
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
if os.environ.get("VGCSIM_MODULE_DIR"):
    sys.path.insert(0, os.path.abspath(os.environ["VGCSIM_MODULE_DIR"]))
import engine_diff as E  # noqa: E402
import multi_turn_truth as M  # noqa: E402

SEED0 = 11 + 7_000_000 * 1000003     # far from every seed set used so far


def sd_chunk(job):
    k, acts, turns, n, start, req = job
    M.init(req, True)
    texts, turn, calls, _ = E.make_position(k)
    ex = E.build("exact", texts, turn, calls)
    acts = [[tuple(a) for a in side] for side in acts]
    choices = [ex.copy().choice_string(0, *acts[0]), ex.copy().choice_string(1, *acts[1])]
    req = {"id": 0, "texts": list(texts), "turn": turn, "calls": [[nm, list(a), kw] for nm, a, kw in calls], "choices": choices,
           "seeds": n, "seed0": SEED0 + start * 1000003, "turns": turns, "log": 0}
    resp = E.truth_request(req)
    if resp.get("error"):
        raise RuntimeError(resp["error"])
    return [M.finish(r, r.get("ended", 0) == 1) for r in resp["rows"]], resp.get("illegal", 0)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", required=True)
    ap.add_argument("--k", type=int, required=True)
    ap.add_argument("--turns", type=int, default=5)
    ap.add_argument("--n", type=int, default=20000)
    ap.add_argument("--procs", type=int, default=4)
    ap.add_argument("--require", default="", help="(2026-11-05) the --require ids the case was made with (multi_turn_truth)")
    a = ap.parse_args()
    r = next(x for x in json.load(open(a.out))["flagged"] if x["k"] == a.k)
    per = a.n // a.procs
    req = tuple(x for x in a.require.split(",") if x)
    jobs = [(a.k, r["acts"], a.turns, per, j * per, req) for j in range(a.procs)]
    with mp.Pool(a.procs) as pool:
        res = pool.map_async(sd_chunk, jobs)
        M.init(req, True)
        texts, turn, calls, _ = E.make_position(a.k)
        old = E.build("old", texts, turn, calls)
        fo = M.play_fast(old, [[tuple(x) for x in s] for s in r["acts"]], a.turns, per * a.procs)
        out = res.get()
    ft = [row for rows, _ in out for row in rows]
    illegal = sum(il for _, il in out)
    n, m = len(fo), len(ft)
    stats = []
    for key in fo[0]:
        xo = [x[key] for x in fo]
        xe = [x[key] for x in ft]
        mo, me = sum(xo) / n, sum(xe) / m
        vo = sum((x - mo) ** 2 for x in xo) / max(1, n - 1)
        ve = sum((x - me) ** 2 for x in xe) / max(1, m - 1)
        se = math.sqrt(vo / n + ve / m)
        z = (mo - me) / se if se > 0 else (0.0 if mo == me else float("inf"))
        stats.append((abs(z), key, mo, me, se))
    stats.sort(reverse=True)
    print(f"case {a.k}: fast {n} seeds, Showdown {m} seeds, illegal Showdown choices {illegal}, {len(stats)} features")
    print(f"  original flag: {r['diffs']}")
    for az, key, mo, me, se in stats[:8]:
        print(f"  {key:16s} fast {mo:.4f}  Showdown {me:.4f}  diff {100 * (mo - me):+.2f} pts  se {100 * se:.2f}  |z| {az:.2f}")
    # Bonferroni over the features: |z| > 4 (two-sided p ~ 6e-5 each) is the bug line; < 3 everywhere = no detectable bias
    print("  VERDICT:", "DIFFERENT" if stats and stats[0][0] > 4 else ("borderline" if stats and stats[0][0] > 3 else "no bias detected"))


if __name__ == "__main__":
    main()
