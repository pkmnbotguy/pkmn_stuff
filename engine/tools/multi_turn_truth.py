"""(2026-10-16) Several turns: the fast engine against REAL Showdown (tools/showdown_truth.js), from the same random mid-battle
positions engine_diff builds.  Turn 1 = engine_diff's joint action; later turns = a fixed rule that both sides of the comparison
follow (repeat the move chosen on turn 1, else rotate through attacking moves, never status moves unless Choice-locked; target the first
living foe slot; lowest-index replacements).  Catches what one turn cannot: timers (weather, terrain, Tailwind, Trick Room, screens,
Taunt / Encore / Disable / Perish / Yawn / Wish / Future Sight), residual effects, lock-ins, switching.

    python tools/multi_turn_truth.py REQUIRE N FIRST [--turns 3] [--seeds 60] [--workers 10] [--confirm 300] [--out F.json]
REQUIRE: ids side 0's first active must have (or "" for none); ED_TEAMS=Teams_cov selects the coverage pool."""
import argparse
import json
import os
import sys
import time
from multiprocessing import Pool

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import engine_diff as E  # noqa: E402

MV = {}
FOE_T = {"normal", "any", "adjacentFoe"}
ALLY_T = {"adjacentAlly", "adjacentAllyOrSelf"}
FIRST_ONLY = {"fakeout", "firstimpression", "matblock", "gigatonhammer", "bloodmoon"}


def init(require, truth=True):
    E.init(require, truth)
    MV.update(json.load(open(os.path.join(E.DATA, "dex.json"), encoding="utf-8"))["moves"])


def slots(sd):
    out = {}
    for p, i in enumerate(sd["active"]):
        if i is not None and not sd["mons"][i]["fainted"]:
            out[p] = i
    return out


def det_old(b, t, mem):
    """the rule, for the fast engine (legal_actions says what is allowed)"""
    st = b.state()
    out = []
    for side in range(2):
        mine = slots(st["sides"][side])
        foes = slots(st["sides"][1 - side])
        la = b.legal_actions(side)
        pair = []
        for p in range(2):
            if p not in mine:
                pair.append(("pass", 0, 0, False))
                continue
            mon = st["sides"][side]["mons"][mine[p]]
            moves = mon["moves"]
            n = len(moves)

            def tgt(mid):
                tg = MV.get(mid, {}).get("target", "normal")
                if mid == "curse" and "Ghost" not in (mon.get("types") or []):
                    tg = "self"                          # (2026-10-28) Showdown's request: nonGhostTarget (the rule fell to Calm Mind)
                if tg in ALLY_T:                      # the partner, or ourselves when it is gone (= legal_actions)
                    return -(2 - p) if (1 - p) in mine else -(p + 1)
                return (1 if 0 in foes else 2) if tg in FOE_T else 0
            c = []
            ids = [x[0] for x in moves]
            if (side, mine[p]) in mem and mem[(side, mine[p])] in ids:
                idx = ids.index(mem[(side, mine[p])])
                mid = moves[idx][0]
                if mid not in FIRST_ONLY and (MV.get(mid, {}).get("category") != "Status" or str(mon["item"] or mon.get("stash_item", "")).startswith("choice")):
                    c.append(("move", idx, tgt(mid), False))
            for j in range(n):
                idx = (t + p + j) % n
                mid, pp = moves[idx][0], moves[idx][1]
                tg = MV.get(mid, {}).get("target", "normal")
                if pp <= 0 or tg in ALLY_T or mid in FIRST_ONLY or MV.get(mid, {}).get("category") == "Status":
                    continue
                c.append(("move", idx, tgt(mid), False))
            for idx in range(n):
                mid = moves[idx][0]
                tg = MV.get(mid, {}).get("target", "normal")
                if moves[idx][1] > 0 and tg not in ALLY_T and mid not in FIRST_ONLY:
                    c.append(("move", idx, tgt(mid), False))
            for idx in range(n):
                c.append(("move", idx, tgt(moves[idx][0]), False))
            c = list(dict.fromkeys(c))
            ok = [x for x in c if x in la[p]]
            if not ok:
                ok = [x for x in la[p] if x[0] == "move" and not x[3]] or [x for x in la[p] if not x[3]] or [("pass", 0, 0, False)]
            pair.append(ok[0])
            if pair[p][0] == "move" and pair[p][1] < len(moves) and (side, mine[p]) not in mem:
                mem[(side, mine[p])] = moves[pair[p][1]][0]
        out.append(pair)
    return out


def resolve(b, start_turn):
    for _ in range(12):
        if b.phase == "ended" or (b.turn != start_turn and b.phase in ("turn", "move")):
            return
        acted = False
        for side in range(2):
            if b.phase == "ended" or (b.turn != start_turn and b.phase in ("turn", "move")):
                return
            if not b.needs_choice(side):
                continue
            sd = b.state()["sides"][side]
            mons = sd["mons"]
            need = sd["switch_needed"]
            active = [a for a in sd["active"] if a is not None]
            bench = [i for i, m in enumerate(mons) if not m["fainted"] and i not in active]
            dead = [i for i, m in enumerate(mons) if m["fainted"]]
            rv = sd.get("reviving") or [False, False]
            pair = []
            for k in range(2):
                if k < len(need) and need[k] and rv[k] and dead:      # (2026-10-19) Revival Blessing: first fainted, team order
                    pair.append(("switch", dead.pop(0), 0, False))
                elif k < len(need) and need[k] and bench:
                    pair.append(("switch", bench.pop(0), 0, False))
                else:
                    pair.append(("pass", 0, 0, False))
            b.choose(side, pair[0], pair[1])
            acted = True
        if not acted:
            return


def play_fast(base, acts, turns, seeds):
    rows = []
    for k in range(seeds):
        c = base.copy()
        c.reseed(11 + k * 1000003)
        mem = {}
        st0 = c.state()
        for side in range(2):
            sl = slots(st0["sides"][side])
            for p, a in enumerate(acts[side]):
                if a[0] == "move" and p in sl:
                    mem[(side, sl[p])] = st0["sides"][side]["mons"][sl[p]]["moves"][a[1]][0]
        for t in range(turns):
            if c.phase == "ended":
                break
            a = acts if t == 0 else det_old(c, t, mem)
            t0 = c.turn
            c.choose(0, *a[0])
            c.choose(1, *a[1])
            resolve(c, t0)
        rows.append(finish(E.outcome(c, "old"), c.phase == "ended"))
    return rows


def finish(o, ended):
    """a finished game's stat stages and field timers can't matter (Showdown stops ticking the moment a side is out)"""
    if ended:
        o = {k: (0.0 if k.split(".")[-1] in ("atk", "def", "spa", "spd", "spe") or k.startswith(("terrain.", "weather.")) else v)
             for k, v in o.items()}
    return o


def run_one(args):
    k, turns, seeds = args
    try:
        texts, turn, calls, aseed = E.make_position(k)
        old = E.build("old", texts, turn, calls)
        ex = E.build("exact", texts, turn, calls)
        acts = E.pick_actions(old, aseed)
        fo = play_fast(old, acts, turns, seeds)
        choices = [ex.copy().choice_string(0, *acts[0]), ex.copy().choice_string(1, *acts[1])]
        req = {"id": 0, "texts": list(texts), "turn": turn, "calls": [[n, list(a), kw] for n, a, kw in calls], "choices": choices,
               "seeds": seeds, "seed0": 11, "turns": turns}
        resp = E.truth_request(req)
        if resp.get("error"):
            raise RuntimeError("showdown_truth: " + resp["error"])
        ft = [finish(r, r.get("ended", 0) == 1) for r in resp["rows"]]
        return {"k": k, "names": E.describe(ex, acts), "diffs": E.compare(fo, ft), "acts": acts}
    except BaseException as e:  # noqa: BLE001
        return {"k": k, "error": f"{type(e).__name__}: {e}"}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("require")
    ap.add_argument("n", type=int)
    ap.add_argument("first", type=int)
    ap.add_argument("--turns", type=int, default=3)
    ap.add_argument("--seeds", type=int, default=60)
    ap.add_argument("--workers", type=int, default=10)
    ap.add_argument("--confirm", type=int, default=0)
    ap.add_argument("--out", default=None)
    a = ap.parse_args()
    req = tuple(x for x in a.require.split(",") if x)
    jobs = [(k, a.turns, a.seeds) for k in range(a.first, a.first + a.n)]
    t0 = time.time()
    with Pool(a.workers, initializer=init, initargs=(req,)) as pool:
        res = list(pool.imap_unordered(run_one, jobs, chunksize=4))
        if a.confirm:
            again = [(r["k"], a.turns, a.confirm) for r in res if "error" not in r and r["diffs"]]
            redo = {r["k"]: r for r in pool.imap_unordered(run_one, again, chunksize=1)}
            res = [redo.get(r["k"], r) for r in res]
    res.sort(key=lambda r: r["k"])
    errs = [r for r in res if "error" in r]
    flagged = [r for r in res if "error" not in r and r["diffs"]]
    print(f"positions {len(res) - len(errs)}  errors {len(errs)}  flagged {len(flagged)} ({100 * len(flagged) / max(1, len(res)):.2f}%)  "
          f"{time.time() - t0:.0f}s")
    for r in errs[:5]:
        print("error", r["k"], r["error"][:300])
    for r in flagged:
        print(r["k"], [x for x in r["names"]], r["diffs"][:3])
    if a.out:
        json.dump({"flagged": flagged, "errors": errs}, open(a.out, "w"), indent=1)


if __name__ == "__main__":
    main()
