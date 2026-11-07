"""(2026-10-17) Re-run coverage_truth.py's flagged positions (e.g. with a fixed engine) and say which still differ from Showdown.

    VGCSIM_MODULE_DIR=target_test/module ED_TEAMS=Teams_cov python tools/coverage_recheck.py [--out F.json] [--seeds 300]
        [--turns 3] [--only K,K] [--by-turn] [--workers 5]
--by-turn: for cases that still differ, also the shorter turn counts, to find the first turn that differs.
--workers: cases run in parallel (one Showdown process each); results print as each case finishes.
Showdown's answers are cached (engine_diff.truth_request; TRUTH_CACHE=0 = always ask Showdown)."""
import argparse
import json
import os
import sys
from multiprocessing import Pool

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
if os.environ.get("VGCSIM_MODULE_DIR"):
    sys.path.insert(0, os.path.abspath(os.environ["VGCSIM_MODULE_DIR"]))
import coverage_truth as C  # noqa: E402

# Showdown's answers come from the truth cache (fuzz/truth_cache.sqlite) unless TRUTH_CACHE=0
os.environ.setdefault("TRUTH_CACHE", "1")
import engine_diff as E  # noqa: E402


def check(job):
    r, turns, seeds, by_turn = job
    C.LOG_SEEDS = False
    x = C.run_one((r["k"], r["acts"], turns, seeds))
    res = [(turns, x.get("diffs", x.get("error")))]
    if res[0][1] and by_turn:
        for t in range(1, turns):
            y = C.run_one((r["k"], r["acts"], t, seeds))
            res.insert(-1, (t, y.get("diffs", y.get("error"))))
    return r, res, os.getpid(), E._CACHE.get("hits", 0), E._CACHE.get("misses", 0)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default=os.path.join(E.ROOT, "fuzz", "coverage_900000.json"))
    ap.add_argument("--seeds", type=int, default=300)
    ap.add_argument("--turns", type=int, default=3)
    ap.add_argument("--only", default="")
    ap.add_argument("--skip", default="", help="cases cleared as chance (comma list): not rechecked")
    ap.add_argument("--by-turn", action="store_true")
    ap.add_argument("--workers", type=int, default=5)
    a = ap.parse_args()
    C.init()
    import vgcsim
    print("module:", vgcsim.__file__, flush=True)
    only = {int(x) for x in a.only.split(",") if x}
    skip = {int(x) for x in a.skip.split(",") if x}
    jobs = [(r, a.turns, a.seeds, a.by_turn) for r in json.load(open(a.out))["flagged"] if (not only or r["k"] in only) and r["k"] not in skip]
    still = 0
    cache = {}
    with Pool(max(1, min(a.workers, len(jobs) or 1)), initializer=C.init) as pool:
        for n, (r, res, pid, hits, misses) in enumerate(pool.imap_unordered(check, jobs), 1):
            cache[pid] = (hits, misses)
            bad = any(d for _, d in res)
            err = any(isinstance(d, str) for _, d in res)
            still += bad
            print(f"[{n}/{len(jobs)}] " + ("ERROR" if err else "DIFF " if bad else "ok   ") + " " + str(r["k"]), r["names"], flush=True)
            for t, d in res:
                if d:
                    print(f"      after {t} turn(s): {d if isinstance(d, str) else d[:4]}", flush=True)
    print(f"{still} still differ", flush=True)
    if os.environ.get("TRUTH_CACHE", "0") not in ("", "0"):
        print(f"truth cache: {sum(h for h, _ in cache.values())} answers reused, {sum(m for _, m in cache.values())} asked Showdown", flush=True)


if __name__ == "__main__":
    main()
