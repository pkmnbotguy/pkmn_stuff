"""(2026-10-17) Coverage-steered accuracy fuzzing against REAL Showdown (idea from Showdown's sim/tools/exhaustive-runner.ts).

ExhaustiveRunner only checks that Showdown doesn't crash; this checks the fast engine against Showdown while steering play so that
every move / ability / item / species in the pool actually FIRES, not just sits on a team. Each round:
  1. candidate positions (engine_diff.make_position on ED_TEAMS, default Teams_cov) are scored by how much on the field is still
     unexercised; the best ones are kept;
  2. turn 1 is chosen per slot to use the least-fired move (Mega when the stone hasn't fired, switch when a bench Pokemon carries an
     unfired ability / item); later turns = multi_turn_truth's fixed rule (both engines must follow it);
  3. each position runs in the fast engine and in Showdown (tools/showdown_truth.js) for --seeds seeds, distributions compared
     (engine_diff.compare), flagged positions re-run with --confirm seeds;
  4. Showdown's logs say what fired: |move| lines, 'ability: X' / 'item: X' tags, -ability / -item / -enditem / -mega lines.
Stops when every id has fired in >= --min-fire positions, after --stall rounds without a newly covered id, or at --max-positions.
Output (--out, default fuzz/coverage_<first>.json): flagged positions, errors, per-id fired / on-field / tried counts. Ids that
never fired but were on the field are listed separately: mostly silent effects (Choice Scarf, Huge Power...) Showdown never logs.

    ED_TEAMS=Teams_cov python tools/coverage_truth.py [--first 900000] [--batch 200] [--max-positions 20000] [--turns 3]
        [--seeds 60] [--confirm 300] [--min-fire 3] [--workers 10]
Resumes from --out if it exists (counts and next position number are kept)."""
import argparse
import json
import os
import random
import re
import sys
import time
from collections import Counter
from multiprocessing import Pool

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import engine_diff as E  # noqa: E402
import multi_turn_truth as M  # noqa: E402

CATS = ("move", "ability", "item", "species")
TAG = re.compile(r"\b(ability|item): ([^|\]\n]+)")


def universe():
    """every id on some team of the pool"""
    u = {c: set() for c in CATS}
    for team in E._G["teams"]:
        for s in team:
            lines = [x.strip() for x in s.split("\n") if x.strip()]
            head = lines[0]
            if "@" in head:
                head, item = head.rsplit("@", 1)
                u["item"].add(E.to_id(item))
            name = re.sub(r"\s*\([MF]\)\s*$", "", head.strip())
            m = re.search(r"\(([^()]+)\)\s*$", name)
            if m:
                name = m.group(1)
            u["species"].add(E.to_id(name))
            for x in lines[1:]:
                if x.startswith("Ability:"):
                    u["ability"].add(E.to_id(x[8:]))
                elif x.startswith("-"):
                    u["move"].add(E.to_id(x[1:]))
    return u


def fired_from_log(log):
    """{cat: set(ids)} that showed up in Showdown's log after the position was set up"""
    out = {c: set() for c in CATS}
    start = 0
    for i, ln in enumerate(log):
        if ln.startswith("|turn|"):
            for j in range(i + 1, len(log)):
                if log[j].startswith("|t:|"):
                    start = j
                    break
            break
    for ln in log[start:]:
        p = ln.split("|")
        if len(p) < 3:
            continue
        kind = p[1]
        if kind == "move" and len(p) > 3:
            out["move"].add(E.to_id(p[3]))
        elif kind == "-ability" and len(p) > 3:
            out["ability"].add(E.to_id(p[3]))
        elif kind in ("-item", "-enditem") and len(p) > 3:
            out["item"].add(E.to_id(p[3]))
        elif kind == "-mega" and len(p) > 4:
            out["item"].add(E.to_id(p[4]))
        elif kind in ("switch", "drag", "detailschange") and len(p) > 3:
            out["species"].add(E.to_id(p[3].split(",")[0]))
        for cat, name in TAG.findall(ln):
            out[cat].add(E.to_id(name))
    return out


def on_field(st):
    """{cat: set(ids)} of the Pokemon active at the start (what the position exposes)"""
    out = {c: set() for c in CATS}
    for sd in st["sides"]:
        for i in sd["active"]:
            if i is None:
                continue
            m = sd["mons"][i]
            if m["fainted"]:
                continue
            out["species"].add(E.to_id(m["species"]))
            out["ability"].add(E.to_id(m["ability"]))
            if m["item"]:
                out["item"].add(E.to_id(m["item"]))
            for mv in m["moves"]:
                out["move"].add(E.to_id(mv[0]))
    return out


class Counts:
    def __init__(self, uni, data=None):
        self.uni = uni
        self.fired = {c: Counter() for c in CATS}
        self.field = {c: Counter() for c in CATS}
        self.tried = {c: Counter() for c in CATS}
        if data:
            for key in ("fired", "field", "tried"):
                for c in CATS:
                    getattr(self, key)[c].update(data[key][c])

    def need(self, cat, i, min_fire):
        """how much an id still needs exercising (0 = done)"""
        if i not in self.uni[cat] or self.fired[cat][i] >= min_fire:
            return 0.0
        # ids tried many times without firing lose priority (silent effects, conditions that rarely hold)
        return 1.0 / (1 + self.fired[cat][i] + 0.3 * self.tried[cat][i])

    def missing(self, min_fire):
        return {c: sorted(i for i in self.uni[c] if self.fired[c][i] < min_fire) for c in CATS}

    def dump(self):
        return {key: {c: dict(getattr(self, key)[c]) for c in CATS} for key in ("fired", "field", "tried")}


def mon_need(cnt, m, min_fire):
    v = cnt.need("ability", E.to_id(m["ability"]), min_fire) + cnt.need("species", E.to_id(m["species"]), min_fire)
    if m["item"]:
        v += cnt.need("item", E.to_id(m["item"]), min_fire)
    return v + sum(cnt.need("move", E.to_id(mv[0]), min_fire) for mv in m["moves"])


def score_position(cnt, st, min_fire):
    v = 0.0
    for sd in st["sides"]:
        act = [i for i in sd["active"] if i is not None]
        for i, m in enumerate(sd["mons"]):
            if not m["fainted"]:
                v += mon_need(cnt, m, min_fire) * (1.0 if i in act else 0.3)
    return v


def steer_actions(cnt, b, seed, min_fire):
    """per slot: the option whose move / Mega stone / incoming Pokemon is least exercised (random tie-break)"""
    r = random.Random(seed)
    st = b.state()
    out = []
    for side in range(2):
        sd = st["sides"][side]
        legal = b.legal_actions(side)
        taken, pair = set(), []
        for slot_i, opts in enumerate(legal):
            act_i = sd["active"][slot_i] if slot_i < len(sd["active"]) else None
            mon = sd["mons"][act_i] if act_i is not None else None
            best, best_v = None, -1.0
            for a in opts:
                if a[0] == "move" and mon is not None:
                    v = cnt.need("move", E.to_id(mon["moves"][a[1]][0]), min_fire) if a[1] < len(mon["moves"]) else 0.0
                    if a[3] and mon["item"]:
                        v += cnt.need("item", E.to_id(mon["item"]), min_fire)
                    if a[2] < 0:
                        v *= 0.5   # aimed at the ally: allowed, but attacks at foes first
                elif a[0] == "switch":
                    if a[1] in taken:
                        continue
                    m = sd["mons"][a[1]]
                    # below an unused move (1.0): switch only when the active Pokemon's moves are mostly exercised
                    v = 0.4 * max(cnt.need("ability", E.to_id(m["ability"]), min_fire),
                                  cnt.need("item", E.to_id(m["item"]), min_fire) if m["item"] else 0.0,
                                  cnt.need("species", E.to_id(m["species"]), min_fire))
                else:
                    v = 0.0
                v += 0.05 * r.random()
                if v > best_v:
                    best, best_v = tuple(a), v
            if best is None:
                best = ("pass", 0, 0, False)
            if best[0] == "switch":
                taken.add(best[1])
            pair.append(best)
        while len(pair) < 2:
            pair.append(("pass", 0, 0, False))
        if pair[0][3] and pair[1][3]:
            pair[1] = (pair[1][0], pair[1][1], pair[1][2], False)
        out.append(pair)
    return out


def tried_ids(st, acts):
    """ids the chosen turn-1 actions try to use"""
    out = {c: set() for c in CATS}
    for side in range(2):
        sd = st["sides"][side]
        for slot_i, a in enumerate(acts[side]):
            i = sd["active"][slot_i] if slot_i < len(sd["active"]) else None
            if a[0] == "move" and i is not None:
                m = sd["mons"][i]
                if a[1] < len(m["moves"]):
                    out["move"].add(E.to_id(m["moves"][a[1]][0]))
                if a[3] and m["item"]:
                    out["item"].add(E.to_id(m["item"]))
            elif a[0] == "switch":
                m = sd["mons"][a[1]]
                out["species"].add(E.to_id(m["species"]))
                out["ability"].add(E.to_id(m["ability"]))
                if m["item"]:
                    out["item"].add(E.to_id(m["item"]))
    return out


def init(truth=True):
    M.init((), truth)


# (2026-10-18) staged seeds [(seeds so far, loose-screen z), ...]; set from --seed-stages in main / the pool initializer
STAGES = [(15, 1.5), (30, 2.0), (45, 2.8)]


def parse_stages(spec):
    """'15:1.5,30:2.0,45:2.8' -> [(15, 1.5), ...]; '' or 'off' = the old single --seeds-first screen"""
    if not spec or spec == "off":
        return []
    return [(int(a), float(b)) for a, b in (x.split(":") for x in spec.split(","))]

# False = don't ask Showdown for its logs (rechecks: no coverage counting; smaller truth-cache entries)
LOG_SEEDS = True


def run_one(args):
    # (2026-10-17) optional 5th element = adaptive seeds: Showdown first plays only that many seeds; the rest (up to `seeds`,
    # the same seed numbers as a full run) only when the loose pre-screen finds anything. 0 = all seeds at once.
    k, acts, turns, seeds = args[:4]
    first = args[4] if len(args) > 4 else 0
    stage_spec = args[5] if len(args) > 5 else None
    try:
        texts, turn, calls, _ = E.make_position(k)
        old = E.build("old", texts, turn, calls)
        ex = E.build("exact", texts, turn, calls)
        acts = [[tuple(a) for a in side] for side in acts]
        fo = M.play_fast(old, acts, turns, seeds)
        choices = [ex.copy().choice_string(0, *acts[0]), ex.copy().choice_string(1, *acts[1])]
        req = {"id": 0, "texts": list(texts), "turn": turn, "calls": [[n, list(a), kw] for n, a, kw in calls], "choices": choices,
               "seeds": seeds, "seed0": 11, "turns": turns, "log": seeds if LOG_SEEDS else 0}
        escalated = False
        # (2026-10-18) staged seeds: Showdown plays the seeds in chunks (same seed numbers as one full run: game i = seed0 +
        # i * 1000003) and stops at the first stage whose loose screen (half threshold, z > the stage's value) finds nothing.
        # Default 15:1.5,30:2.0,45:2.8 (chosen on 2,400 positions: 29.6 Showdown seeds per position instead of 44.9 for the
        # old single 15-seed screen, same 99.4% recall of borderline injected differences). --seeds-first N = old rule.
        st = parse_stages(stage_spec) if stage_spec is not None else STAGES
        stages = st if (first and st) else ([(first, 1.5)] if 0 < first < seeds else [])
        stages = [(n, z) for n, z in stages if 0 < n < seeds]
        if stages:
            req["seeds"], req["log"] = stages[0][0], (stages[0][0] if LOG_SEEDS else 0)
        resp = E.truth_request(req)
        if resp.get("error"):
            raise RuntimeError("showdown_truth: " + resp["error"])
        ft = [M.finish(r, r.get("ended", 0) == 1) for r in resp["rows"]]
        passed = not stages
        for j, (n, z) in enumerate(stages):
            if not E.compare(fo, ft, loose=True, zmin=z):
                break
            escalated = True
            upto = stages[j + 1][0] if j + 1 < len(stages) else seeds
            more = dict(req, seeds=upto - n, seed0=req["seed0"] + n * 1000003, log=0)
            r2 = E.truth_request(more)
            if r2.get("error"):
                raise RuntimeError("showdown_truth: " + r2["error"])
            ft += [M.finish(r, r.get("ended", 0) == 1) for r in r2["rows"]]
            resp["illegal"] = resp.get("illegal", 0) + r2.get("illegal", 0)
            if upto == seeds:
                passed = True
        if os.environ.get("COV_STUDY"):      # (2026-10-18) dump both engines' outcome rows (escalation-rule study)
            with open(os.path.join(os.environ["COV_STUDY"], f"{k}.json"), "w") as fd:
                json.dump({"fo": fo, "ft": ft}, fd)
        if not passed:
            diffs = []
        else:
            diffs = E.compare(fo, ft)
        fired = {c: set() for c in CATS}
        fired["species"] |= on_field(old.state())["species"]     # species coverage = on the field during a compared turn
        for log in resp.get("logs", []):
            for c, ids in fired_from_log(log).items():
                fired[c] |= ids
        return {"k": k, "names": E.describe(ex, acts), "diffs": diffs, "acts": acts, "escalated": escalated, "sd_seeds": len(ft),
                "illegal": resp.get("illegal", 0), "fired": {c: sorted(v) for c, v in fired.items()}}
    except BaseException as e:  # noqa: BLE001
        return {"k": k, "error": f"{type(e).__name__}: {e}"}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--first", type=int, default=900000, help="first candidate position number")
    ap.add_argument("--batch", type=int, default=200, help="positions run per round")
    ap.add_argument("--candidates", type=int, default=5, help="candidates scored per position kept")
    ap.add_argument("--max-positions", type=int, default=20000)
    ap.add_argument("--turns", type=int, default=3)
    ap.add_argument("--seeds", type=int, default=60)
    ap.add_argument("--confirm", type=int, default=300)
    ap.add_argument("--seed-stages", default="15:1.5,30:2.0,45:2.8", help="staged seeds 'n:z,...' (each stage: seeds so "
                    "far and the loose screen's z); 'off' = the old rule (--seeds-first, then all --seeds)")
    ap.add_argument("--seeds-first", type=int, default=15, help="adaptive seeds: Showdown plays this many first, the rest of "
                    "--seeds only when a loose pre-screen finds a difference (0 = always all --seeds)")
    ap.add_argument("--min-fire", type=int, default=3, help="positions an id must fire in")
    ap.add_argument("--stall", type=int, default=5, help="stop after this many rounds without a newly covered id")
    ap.add_argument("--workers", type=int, default=10)
    ap.add_argument("--out", default=None)
    a = ap.parse_args()
    out = a.out or os.path.join(E.ROOT, "fuzz", f"coverage_{a.first}.json")
    os.makedirs(os.path.dirname(os.path.abspath(out)), exist_ok=True)
    M.init((), False)                     # main process: fast engine only (scoring, action choice)
    uni = universe()
    prev = json.load(open(out)) if os.path.exists(out) else None
    cnt = Counts(uni, prev["counts"] if prev else None)
    flagged = prev["flagged"] if prev else []
    errors = prev["errors"] if prev else []
    nxt = prev["next_k"] if prev else a.first
    done = prev["positions"] if prev else 0
    illegal = prev.get("illegal", 0) if prev else 0
    print(f"pool {os.environ.get('ED_TEAMS', 'pools/Teams_cov')}: " + ", ".join(f"{len(uni[c])} {c}s" for c in CATS) +
          (f"; resumed at {done} positions" if prev else ""), flush=True)
    t0 = time.time()
    run_done = 0
    run_esc = 0
    run_sd = 0
    stall = 0
    total = sum(len(uni[c]) for c in CATS)

    def n_done():
        return sum(sum(1 for i in uni[c] if cnt.fired[c][i] >= a.min_fire) for c in CATS)

    with Pool(a.workers, initializer=init) as pool:
        while done < a.max_positions:
            before = n_done()
            cands = []
            for k in range(nxt, nxt + a.batch * a.candidates):
                texts, turn, calls, aseed = E.make_position(k)
                try:
                    b = E.build("old", texts, turn, calls)
                except BaseException:  # noqa: BLE001
                    continue
                cands.append((score_position(cnt, b.state(), a.min_fire), k, b, aseed))
            nxt += a.batch * a.candidates
            cands.sort(key=lambda x: -x[0])
            jobs = []
            for _, k, b, aseed in cands[:min(a.batch, a.max_positions - done)]:
                acts = steer_actions(cnt, b, aseed, a.min_fire)
                st = b.state()
                for c, ids in on_field(st).items():
                    cnt.field[c].update(ids)
                for c, ids in tried_ids(st, acts).items():
                    cnt.tried[c].update(ids)
                jobs.append((k, acts, a.turns, a.seeds, a.seeds_first, a.seed_stages))
            res = list(pool.imap_unordered(run_one, jobs, chunksize=2))
            run_esc += sum(1 for r in res if r.get("escalated"))
            run_sd += sum(r.get("sd_seeds", 0) for r in res)
            if a.confirm:
                again = [(r["k"], r["acts"], a.turns, a.confirm) for r in res if "error" not in r and r["diffs"]]
                redo = {r["k"]: r for r in pool.imap_unordered(run_one, again, chunksize=1)}
                res = [redo.get(r["k"], r) for r in res]
            for r in res:
                if "error" in r:
                    errors.append(r)
                    continue
                illegal += r.get("illegal", 0)
                for c in CATS:
                    cnt.fired[c].update(r["fired"][c])
                if r["diffs"]:
                    flagged.append({k: r[k] for k in ("k", "names", "diffs", "acts")})
            done += len(res)
            run_done += len(res)
            after = n_done()
            stall = stall + 1 if after == before else 0
            miss = cnt.missing(a.min_fire)
            rate = run_done / max(1e-9, time.time() - t0)
            print(f"[{time.strftime('%H:%M')}] positions {done}  covered {after}/{total} ids (fired in >= {a.min_fire} positions)  "
                  + "  ".join(f"{c} {len(uni[c]) - len(miss[c])}/{len(uni[c])}" for c in CATS)
                  + f"  flagged {len(flagged)}  errors {len(errors)}  {rate:.1f} positions/s"
                  + (f"  escalated {100 * run_esc / max(1, run_done):.1f}%  Showdown seeds/position {run_sd / max(1, run_done):.1f}"
                     if a.seeds_first else ""), flush=True)
            json.dump({"positions": done, "next_k": nxt, "illegal": illegal, "counts": cnt.dump(),
                       "flagged": flagged, "errors": errors[-200:]}, open(out, "w"), indent=1)
            if after == total:
                print("every id fired: done")
                break
            if stall >= a.stall:
                print(f"no newly covered id in {a.stall} rounds: stopping")
                break
    miss = cnt.missing(a.min_fire)
    print(f"\n{done} positions, {len(flagged)} flagged ({100 * len(flagged) / max(1, done):.2f}%), {len(errors)} errors, "
          f"{illegal} illegal Showdown choices, {time.time() - t0:.0f}s")
    for c in CATS:
        silent = [i for i in miss[c] if cnt.field[c][i] > 0]
        never = [i for i in miss[c] if cnt.field[c][i] == 0]
        if silent:
            print(f"{c}s under {a.min_fire} firings but on the field (silent or condition rarely met), {len(silent)}: "
                  + ", ".join(f"{i}({cnt.fired[c][i]}/{cnt.field[c][i]})" for i in silent))
        if never:
            print(f"{c}s never on the field, {len(never)}: " + ", ".join(never))
    for r in flagged[-20:]:
        print("flagged", r["k"], r["names"], r["diffs"][:3])
    for r in errors[:5]:
        print("error", r["k"], r["error"][:300])
    print(f"written: {out}")


if __name__ == "__main__":
    main()
