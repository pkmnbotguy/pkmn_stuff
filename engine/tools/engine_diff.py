"""
Where does the fast engine (vgcsim.Battle) disagree with the exact engine (vgcsim.ExactBattle,
bit-exact Showdown)? Builds random but plausible mid-battle positions from real teams, sets them
up identically in both engines, picks one pair of joint actions, and plays that turn many times
(different seeds) in each. Then compares the outcome distributions: HP, faints, statuses, boosts,
who is active, weather/terrain.

    python tools/engine_diff.py --positions 2000 --seeds 100 --workers 10
    python tools/engine_diff.py --show 17        # logs of one position, both engines

Output: share of positions where the engines disagree beyond noise, the most frequent
disagreeing features, and `engine_diff.json` with every flagged case.
"""
import re
import argparse
import glob
import json
import math
import os
import random
import sys
import time
from multiprocessing import Pool

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
DATA = os.path.join(ROOT, "data")
STATUSES = ("brn", "par", "psn", "tox", "slp", "frz")
BOOSTS = ("atk", "def", "spa", "spd", "spe")

PINCH_BERRIES = {"sitrusberry", "oranberry", "figyberry", "wikiberry", "magoberry", "aguavberry", "iapapaberry",
                 "liechiberry", "ganlonberry", "salacberry", "petayaberry", "apicotberry", "starfberry", "lansatberry",
                 "custapberry", "micleberry"}
STATUS_BERRY = {"cheriberry": ("par",), "chestoberry": ("slp",), "pechaberry": ("psn", "tox"), "rawstberry": ("brn",), "aspearberry": ("frz",)}

_G = {}


def init(require=(), truth=False):
    import vgcsim
    _G["require"] = list(require)
    _G["truth"] = bool(truth)
    _G["vg"] = vgcsim
    _G["od"] = vgcsim.Dex(os.path.join(DATA, "dex.json"))
    _G["ed"] = vgcsim.ExactDex(os.path.join(DATA, "dex.json"), os.path.join(DATA, "effects.json"))
    # ED_TEAMS=<folder under the project root> fuzzes another pool (e.g. Teams_cov, tools/coverage_pool.js)
    files = sorted(glob.glob(os.path.join(ROOT, os.environ.get("ED_TEAMS", "pools/Teams_cov"), "*.txt")))
    teams = []
    for f in files:
        sets = [b.strip() for b in open(f, encoding="utf-8").read().replace("\r\n", "\n").split("\n\n") if b.strip()]
        if len(sets) >= 4:
            teams.append(sets)
    _G["teams"] = teams
    # --require: positions where side 0's first active Pokemon has one of these moves / abilities / items
    if _G["require"]:
        def has(set_text):
            ids = {to_id(x) for x in re.split(r"[@:\n]|- ", set_text)}
            return any(q in ids for q in _G["require"])
        _G["req_teams"] = [t for t in teams if any(has(x) for x in t)]
        _G["has"] = has
    dex = json.load(open(os.path.join(DATA, "dex.json"), encoding="utf-8"))
    _G["types"] = {k: v.get("types", []) for k, v in dex["species"].items()}
    # (2026-10-17) ED_MEGA_START=p: with probability p an active Mega-stone holder starts already Mega Evolved (set_mon(mega=True));
    # a separate generator, so positions with the default 0 stay exactly as before
    _G["mega_start"] = float(os.environ.get("ED_MEGA_START", "0") or 0)
    _G["stones"] = {k: {to_id(b) for b in v["megaStone"]} for k, v in dex["items"].items() if v.get("megaStone")}
    _G["random_gender"] = {k for k, v in dex["species"].items() if not v.get("gender")}
    if os.environ.get("ED_TARGETED", "0") not in ("", "0"):
        init_targeted()


# (2026-10-17) ED_TARGETED=1: targeted fuzzing (user's protocol, from another bot maker). Every position holds at least one of each
# ROLE below, each placed on a random side (at most 4 per side: positions have 4 Pokemon a side); the other slots are drawn with
# the INVERSE of how often their parts occur in the set pool, so rare species / abilities / items / moves become common. Sets come
# from Teams_cov + Teams_real (all validated); Species Clause and Item Clause hold on each side. Dancer is not a role: its only
# holders (Oricorio) don't exist in this format.
ROLES = {
    "zoroark": lambda sp, ab: sp in ("zoroark", "zoroarkhisui"),
    "ditto": lambda sp, ab: sp == "ditto",
    "gardevoir": lambda sp, ab: sp == "gardevoir",
    "unburden": lambda sp, ab: ab == "unburden",
    "moldbreaker": lambda sp, ab: ab == "moldbreaker",
}


def _set_parts(text):
    sp, ab = set_info(text)
    first = text.split("\n")[0]
    item = to_id(first.split("@", 1)[1]) if "@" in first else ""
    moves = [to_id(l[2:]) for l in text.split("\n") if l.startswith("- ")]
    return sp, ab, item, moves


def init_targeted():
    from collections import Counter
    seen, pool = set(), []
    # ED_TARGET_REAL: the real-team folder the targeted pool reads (regress.sh pins the copy the stored runs were made with:
    # Teams_real's Ability lines changed on 2026-10-19, which changed every regenerated targeted position)
    for folder in ("pools/Teams_cov", os.environ.get("ED_TARGET_REAL", "pools/Teams_cov")):
        for f in sorted(glob.glob(os.path.join(ROOT, folder, "*.txt"))):
            for b in open(f, encoding="utf-8").read().replace("\r\n", "\n").split("\n\n"):
                b = b.strip()
                if b and b not in seen:
                    seen.add(b)
                    pool.append((b,) + _set_parts(b))
    cnt = Counter()
    for _, sp, ab, item, moves in pool:
        cnt["s:" + sp] += 1
        cnt["a:" + ab] += 1
        cnt["i:" + item] += 1
        for m in moves:
            cnt["m:" + m] += 1
    # a set's weight = the mean of 1 / (count of each of its parts): inverse of the pool's own distribution, normalised below
    weights = []
    for _, sp, ab, item, moves in pool:
        keys = ["s:" + sp, "a:" + ab, "i:" + item] + ["m:" + m for m in moves]
        weights.append(sum(1.0 / cnt[k] for k in keys) / len(keys))
    tot = sum(weights)
    _G["tpool"] = pool
    _G["tweights"] = [w / tot for w in weights]
    import itertools
    _G["tcum"] = list(itertools.accumulate(_G["tweights"]))
    _G["troles"] = {r: [j for j, x in enumerate(pool) if f(x[1], x[2])] for r, f in ROLES.items()}
    _G["targeted"] = True
    # (2026-10-17) ED_ROLES=zoroark,zoroark? : only these roles (repeats allowed, '?' = in half the positions); unset = all ROLES once
    spec = os.environ.get("ED_ROLES", "").strip()
    if spec:
        _G["troles_list"] = [x.strip() for x in spec.split(",") if x.strip()]
        assert all(x.rstrip("?") in ROLES for x in _G["troles_list"]), _G["troles_list"]


def _targeted_pick(r):
    pool, w = _G["tpool"], _G["tweights"]
    sides = [[], []]

    def fits(side, j):
        _, sp, ab, item, _m = pool[j]
        return len(sides[side]) < 4 and all(pool[x][1] != sp and (not item or pool[x][3] != item) for x in sides[side])
    roles = list(_G.get("troles_list") or ROLES)
    r.shuffle(roles)
    for role in roles:
        if role.endswith("?"):              # optional role: present in half the positions
            if r.random() < 0.5:
                continue
            role = role[:-1]
        cand = _G["troles"][role]
        for _ in range(200):
            j = r.choices(cand, weights=[w[x] for x in cand])[0]
            side = r.randrange(2)
            if not fits(side, j):
                side = 1 - side
            if fits(side, j):
                sides[side].append(j)
                break
    for side in range(2):
        while len(sides[side]) < 4:
            j = r.choices(range(len(pool)), cum_weights=_G["tcum"])[0]
            if fits(side, j):
                sides[side].append(j)
        r.shuffle(sides[side])
    return [[pool[j][0] for j in sd] for sd in sides]


def to_id(x):
    return "".join(c for c in x.lower() if c.isalnum())


def set_info(text):
    """(species id, ability id) of one exported set."""
    lines = text.split("\n")
    head = lines[0].split("@")[0].strip()
    for g in (" (M)", " (F)"):
        head = head.replace(g, "")
    if "(" in head and head.endswith(")"):
        head = head[head.rindex("(") + 1:-1]
    ab = next((l.split(":", 1)[1].strip() for l in lines if l.startswith("Ability:")), "")
    return to_id(head), to_id(ab)


STATUS_IMMUNE_TYPE = {"brn": ("Fire",), "par": ("Electric",), "psn": ("Poison", "Steel"), "tox": ("Poison", "Steel"), "frz": ("Ice",)}
STATUS_IMMUNE_ABILITY = {
    "brn": ("thermalexchange", "waterveil", "waterbubble"), "par": ("limber",), "slp": ("insomnia", "vitalspirit", "sweetveil"),
    "psn": ("immunity", "pastelveil"), "tox": ("immunity", "pastelveil"), "frz": ("magmaarmor",),
}


def status_ok(text, st):
    sp, ab = set_info(text)
    if ab in ("comatose", "purifyingsalt") or ab in STATUS_IMMUNE_ABILITY.get(st, ()):
        return False
    return not any(t in STATUS_IMMUNE_TYPE.get(st, ()) for t in _G["types"].get(sp, []))


def make_position(k):
    """The same random setup calls for both engines: (texts, turn, calls, action picker seed)."""
    r = random.Random(k * 7919 + 1)
    teams = _G["teams"]
    ta, tb = r.sample(teams, 2)
    if _G.get("targeted"):
        ta, tb = _targeted_pick(r)
    if _G.get("require"):
        ta = r.choice(_G["req_teams"])
    picked = [r.sample(t, 4) for t in (ta, tb)]
    if _G.get("require"):
        holder = next(x for x in ta if _G["has"](x))
        if holder not in picked[0]:
            picked[0][0] = holder
        else:
            j = picked[0].index(holder)
            picked[0][0], picked[0][j] = picked[0][j], picked[0][0]
    # explicit genders where the species has none (each engine would otherwise draw its own at random); a separate
    # generator, so positions stay the same as before this was added
    rg = random.Random(k * 31 + 7)
    for side in range(2):
        for j, st in enumerate(picked[side]):
            first, nl, rest = st.partition("\n")
            head, at, item = first.partition("@")
            if "(M)" in head or "(F)" in head or set_info(st)[0] not in _G["random_gender"]:
                continue
            g = "(M)" if rg.random() < 0.5 else "(F)"
            first = f"{head.rstrip()} {g}" + (f" @{item}" if at else "")
            picked[side][j] = first + nl + rest
    texts = ["\n\n".join(p) for p in picked]
    turn = r.randint(1, 8)
    calls = []
    for side in range(2):
        for i in range(4):
            first = picked[side][i].split("\n")[0]
            item = first.split("@", 1)[1].strip().lower().replace(" ", "") if "@" in first else ""
            if i >= 2 and r.random() < 0.1:
                calls.append(("set_mon", (side, i), {"fainted": True}))
                continue
            kw = {}
            if r.random() < 0.5:
                kw["hp_frac"] = r.uniform(0.1, 1.0)
            if r.random() < 0.12:
                st = r.choice(("brn", "par", "psn", "tox", "slp", "brn", "par"))
                if status_ok(picked[side][i], st):
                    kw["status"] = st
            if r.random() < 0.15:
                b = {r.choice(BOOSTS): r.choice((-2, -1, 1, 2))}
                if i < 2:                       # (2026-10-16) boosts reset at switch-out: a bench Pokemon has none
                    kw["boosts"] = b
            if i < 2:
                kw["first_turn"] = r.random() < 0.3
            # a realistic position: berries that would have triggered are already eaten
            if item in PINCH_BERRIES and kw.get("hp_frac", 1.0) <= 0.52:
                kw["item"] = ""
            if item == "whiteherb" and any(v < 0 for v in kw.get("boosts", {}).values()):
                kw["item"] = ""
            if "status" in kw and (item == "lumberry" or kw["status"] in STATUS_BERRY.get(item, ())):
                kw["item"] = ""
            calls.append(("set_mon", (side, i), kw))
    if _G.get("mega_start"):
        rm = random.Random(k * 131 + 5)
        for side in range(2):
            for c in calls:
                if c[0] != "set_mon" or c[1][0] != side or c[1][1] >= 2 or c[2].get("fainted"):
                    continue
                first = picked[side][c[1][1]].split("\n")[0]
                item = to_id(first.split("@", 1)[1]) if "@" in first else ""
                if set_info(picked[side][c[1][1]])[0] in _G["stones"].get(item, ()) and rm.random() < _G["mega_start"]:
                    c[2]["mega"] = True
                    break                     # one Mega per side
    field = {}
    if r.random() < 0.25:
        field["weather"] = r.choice(("rain", "sun", "sand", "snow"))
        field["weather_turns"] = r.randint(1, 5)
    if r.random() < 0.2:
        field["terrain"] = r.choice(("electric", "grassy", "psychic", "misty"))
        field["terrain_turns"] = r.randint(1, 5)
    if r.random() < 0.1:
        field["trick_room"] = r.randint(1, 4)
    if field:
        calls.append(("set_field", (), field))
    for side in range(2):
        kw = {}
        if r.random() < 0.1:
            kw["tailwind"] = r.randint(1, 4)
        if r.random() < 0.05:
            kw["reflect"] = r.randint(1, 5)
        if r.random() < 0.05:
            kw["light_screen"] = r.randint(1, 5)
        if kw:
            calls.append(("set_side", (side,), kw))
    return texts, turn, calls, r.randrange(1 << 30)


def build(kind, texts, turn, calls):
    vg = _G["vg"]
    if kind == "old":
        b = vg.Battle.from_brought(_G["od"], texts[0], texts[1], seed=1, turn=turn)
    else:
        b = vg.ExactBattle.from_brought(_G["ed"], texts[0], texts[1], seed=1, turn=turn)
    maxhp = [[m["maxhp"] for m in s["mons"]] for s in b.state()["sides"]]
    for name, args, kw in calls:
        kw = dict(kw)
        if "hp_frac" in kw:
            kw["hp"] = max(1, int(maxhp[args[0]][args[1]] * kw.pop("hp_frac")))
        getattr(b, name)(*args, **kw)
    b.refresh()
    return b


def pick_actions(old, seed):
    """One joint action per side from the fast engine's legal actions (mostly attacks)."""
    r = random.Random(seed)
    out = []
    req_slots = set()
    if _G.get("require"):
        side0 = old.state()["sides"][0]
        a0 = side0["active"][0] if side0["active"] else None
        if a0 is not None:
            mv_list = side0["mons"][a0]["moves"]
            req_slots = {k for k, mv in enumerate(mv_list) if to_id(mv[0] if isinstance(mv, (list, tuple)) else mv) in _G["require"]}
    for side in range(2):
        legal = old.legal_actions(side)
        taken, pair = set(), []
        for slot_i, opts in enumerate(legal):
            moves = [a for a in opts if a[0] == "move"]
            if side == 0 and slot_i == 0 and req_slots and r.random() < 0.8:
                use = [a for a in moves if a[1] in req_slots]
                if use:
                    moves = use
            sw = [a for a in opts if a[0] == "switch" and a[1] not in taken]
            if sw and (not moves or r.random() < 0.12):
                a = r.choice(sw)
                taken.add(a[1])
            elif moves:
                # attacks at foes; a move aimed at an ally only if nothing else, or rarely
                foe_or_none = [a for a in moves if a[2] >= 0]
                if foe_or_none and r.random() < 0.97:
                    moves = foe_or_none
                megas = [a for a in moves if a[3]]
                a = r.choice(megas if megas and r.random() < 0.6 else [a for a in moves if not a[3]] or moves)
            else:
                a = ("pass", 0, 0, False)
            pair.append(tuple(a))
        while len(pair) < 2:
            pair.append(("pass", 0, 0, False))
        # only one Mega per side
        if pair[0][3] and pair[1][3]:
            pair[1] = (pair[1][0], pair[1][1], pair[1][2], False)
        out.append(pair)
    return out


def resolve_switches(b, kind, start_turn):
    """Answer forced switches (faint replacements, U-turn...) with the lowest team index, the same
    in both engines, until the next turn starts."""
    for _ in range(12):
        if b.phase == "ended" or (b.turn != start_turn and b.phase in ("turn", "move")):
            return
        acted = False
        for side in range(2):
            if not b.needs_choice(side):
                continue
            # read the state for each side: choosing for one side can run the turn on
            st = b.state()
            sd = st["sides"][side]
            mons = sd["mons"]
            if kind == "old":
                need = sd["switch_needed"]
                active = [a for a in sd["active"] if a is not None]
            else:
                need = sd["force_switch"]
                active = [i for i, m in enumerate(mons) if m["active"]]
            bench = [i for i, m in enumerate(mons) if not m["fainted"] and i not in active]
            dead = [i for i, m in enumerate(mons) if m["fainted"]]
            pair = []
            for k in range(2):
                # (2026-10-19) the fast engine asks for Revival Blessing's pick too ("reviving"); first fainted in team order
                rv = sd["revival"][k] if kind != "old" else (sd.get("reviving") or [False, False])[k]
                if rv and need[k] and dead:
                    pair.append(("switch", dead.pop(0), 0, False))
                elif k < len(need) and need[k] and bench:
                    pair.append(("switch", bench.pop(0), 0, False))
                else:
                    pair.append(("pass", 0, 0, False))
            b.choose(side, pair[0], pair[1])
            acted = True
        if not acted:
            return


def outcome(b, kind):
    st = b.state()
    f = {}
    for s, sd in enumerate(st["sides"]):
        if kind == "old":
            active = set(a for a in sd["active"] if a is not None)
        else:
            active = set(i for i, m in enumerate(sd["mons"]) if m["active"])
        for i, m in enumerate(sd["mons"]):
            p = f"s{s}m{i}"
            dead = m["fainted"] or m["hp"] <= 0
            f[p + ".hp"] = 0.0 if dead else m["hp"] / m["maxhp"]
            f[p + ".faint"] = float(dead)
            stt = m["status"] if m["status"] in STATUSES and not dead else ""
            for x in STATUSES:
                f[p + "." + x] = float(stt == x)
            for x in BOOSTS:
                f[p + "." + x] = 0.0 if dead else float(m["boosts"].get(x, 0))
            f[p + ".active"] = float(i in active and not dead)
    w = st["field"]["weather"] or ""
    t = st["field"]["terrain"] or ""
    for x in ("raindance", "sunnyday", "sandstorm", "snowscape"):
        f["weather." + x] = float(w == x)
    for x in ("electricterrain", "grassyterrain", "psychicterrain", "mistyterrain"):
        f["terrain." + x] = float(t == x)
    f["ended"] = float(b.phase == "ended")
    return f


def play(base, kind, acts, seeds, seed0):
    rows = []
    for k in range(seeds):
        c = base.copy()
        c.reseed(seed0 + k * 1000003)
        t0 = c.turn
        c.choose(0, *acts[0])
        c.choose(1, *acts[1])
        resolve_switches(c, kind, t0)
        rows.append(outcome(c, kind))
    return rows


def setup_diffs(old, ex):
    """Differences right after setup (stats, HP, active): not mechanics, but the bridge."""
    so, se = old.state(), ex.state()
    d = []
    for s in range(2):
        for i, (a, b) in enumerate(zip(so["sides"][s]["mons"], se["sides"][s]["mons"])):
            ast = [a["stats"][k] for k in ("hp", "atk", "def", "spa", "spd", "spe")]
            if ast != list(b["stats"]) and ast[1:] != list(b["stats"])[1:]:
                d.append(f"s{s}m{i} stats {ast} vs {b['stats']}")
            if a["hp"] != b["hp"] or a["maxhp"] != b["maxhp"]:
                d.append(f"s{s}m{i} hp {a['hp']}/{a['maxhp']} vs {b['hp']}/{b['maxhp']}")
    return d


def compare(fo, fe, loose=False, zmin=None):
    """Features whose means differ beyond noise: list of (feature, mean_old, mean_exact).
    (2026-10-17) the two sides may have different sample sizes (same result as before when equal); loose=True = the
    pre-screen of the adaptive-seed fuzzing (coverage_truth --seeds-first): half the threshold and z > 1.5."""
    n, m = len(fo), len(fe)
    out = []
    for key in fo[0]:
        xo = [r[key] for r in fo]
        xe = [r[key] for r in fe]
        mo, me = sum(xo) / n, sum(xe) / m
        diff = abs(mo - me)
        if diff == 0:
            continue
        vo = sum((x - mo) ** 2 for x in xo) / max(1, n - 1)
        ve = sum((x - me) ** 2 for x in xe) / max(1, m - 1)
        se = math.sqrt(vo / n + ve / m) or 1e-9
        kind = key.split(".")[-1]
        thresh = 0.04 if kind == "hp" else (0.3 if kind in BOOSTS else 0.08)
        if loose:
            thresh, z = thresh * 0.5, (1.5 if zmin is None else zmin)
        else:
            z = 4
        if diff > thresh and (diff / se > z or (vo == 0 and ve == 0)):
            out.append((key, round(mo, 3), round(me, 3)))
    return out


def describe(ex, acts):
    """Readable actions: 'Species: move -> target' per active slot."""
    st = ex.state()
    out = []
    for side in range(2):
        mons = st["sides"][side]["mons"]
        active = sorted((m["position"], i) for i, m in enumerate(mons) if m["active"])
        for slot, a in enumerate(acts[side]):
            if slot >= len(active):
                continue
            m = mons[active[slot][1]]
            ab = f"{m['species']}[{m['ability']}/{m['item']}]"
            if a[0] == "move":
                mv = m["moves"][a[1]][0] if a[1] < len(m["moves"]) else "?"
                out.append(f"p{side + 1}{'ab'[slot]} {ab}: {mv}{' MEGA' if a[3] else ''} -> {a[2]}")
            elif a[0] == "switch":
                out.append(f"p{side + 1}{'ab'[slot]} {ab}: switch to {mons[a[1]]['species']}[{mons[a[1]]['ability']}]")
    return out


SHOWDOWN_DIR = os.environ.get("SHOWDOWN_DIR", os.path.join(ROOT, "..", "pokemon-showdown"))
FORMAT_ID = "gen9championsvgc2026regmc"


def _truth_proc():
    """One Node process per worker: tools/showdown_truth.js (real Showdown plays the same positions)."""
    import subprocess
    if "proc" not in _G:
        _G["proc"] = subprocess.Popen(["node", os.path.join(os.path.dirname(os.path.abspath(__file__)), "showdown_truth.js"),
                                       SHOWDOWN_DIR, FORMAT_ID], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
                                      encoding="utf-8", bufsize=1)
    return _G["proc"]


_CACHE = {}


def _harness_hash():
    """fingerprint of what Showdown's answers depend on besides the request: the harness script and Showdown's compiled
    simulator / data (sizes + modification times of dist/sim and dist/data files; a rebuild or update changes them)"""
    if "h" not in _CACHE:
        import hashlib
        h = hashlib.sha1(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "showdown_truth.js"), "rb").read())
        # (2026-10-18) the speed patch is part of the harness too (it must never change answers; this keeps the cache honest)
        h.update(open(os.path.join(os.path.dirname(os.path.abspath(__file__)), "sd_fast.js"), "rb").read())
        for sub in ("dist/sim", "dist/data", "dist/data/mods/champions"):
            d = os.path.join(SHOWDOWN_DIR, sub)
            for f in sorted(glob.glob(os.path.join(d, "*.js"))):
                st = os.stat(f)
                h.update(f"{os.path.basename(f)}:{st.st_size}:{int(st.st_mtime)};".encode())
        h.update(FORMAT_ID.encode())
        _CACHE["h"] = h.hexdigest()
    return _CACHE["h"]


def _cache_db():
    if "db" not in _CACHE:
        import sqlite3
        path = os.environ.get("TRUTH_CACHE_FILE") or os.path.join(ROOT, "fuzz", "truth_cache.sqlite")
        db = sqlite3.connect(path, timeout=120)
        db.execute("PRAGMA journal_mode=WAL")
        db.execute("CREATE TABLE IF NOT EXISTS truth (key TEXT PRIMARY KEY, resp BLOB)")
        db.commit()
        _CACHE["db"] = db
    return _CACHE["db"]


def _db_retry(f):
    """run f(db); several processes share the cache file: retry when it is locked, give up quietly (None) after ~1 min
    (a failed read = ask Showdown, a failed write = not cached)"""
    import sqlite3
    for k in range(30):
        try:
            return f(_cache_db())
        except sqlite3.OperationalError:
            time.sleep(0.2 + 0.1 * k)
    return None


def truth_request(req):
    """(2026-10-17) Send one request to showdown_truth.js and return its JSON answer. With TRUTH_CACHE=1 the answer comes
    from / goes to fuzz/truth_cache.sqlite (TRUTH_CACHE_FILE): key = the whole request (minus 'id') + _harness_hash(), so a
    changed harness or Showdown build never reuses old answers. Errors are never cached. TRUTH_CACHE=0 (default) = always ask."""
    use = os.environ.get("TRUTH_CACHE", "0") not in ("", "0")
    key = None
    if use:
        import hashlib
        import zlib
        body = json.dumps({k: v for k, v in req.items() if k != "id"}, sort_keys=True)
        key = hashlib.sha1((_harness_hash() + body).encode()).hexdigest()
        row = _db_retry(lambda db: db.execute("SELECT resp FROM truth WHERE key = ?", (key,)).fetchone())
        if row:
            _CACHE["hits"] = _CACHE.get("hits", 0) + 1
            return json.loads(zlib.decompress(row[0]))
    if os.environ.get("TRUTH_DUMP"):                # (2026-10-18) record requests for benchmarking showdown_truth.js
        with open(os.environ["TRUTH_DUMP"], "a", encoding="utf-8") as fd:
            fd.write(json.dumps(req) + chr(10))
    p = _truth_proc()
    p.stdin.write(json.dumps(req) + chr(10))
    p.stdin.flush()
    line = p.stdout.readline()
    if not line:
        raise RuntimeError("showdown_truth.js died")
    resp = json.loads(line)
    if use and not resp.get("error"):
        import zlib
        blob = zlib.compress(line.encode(), 6)

        def put(db):
            db.execute("INSERT OR REPLACE INTO truth (key, resp) VALUES (?, ?)", (key, blob))
            db.commit()
        _db_retry(put)
        _CACHE["misses"] = _CACHE.get("misses", 0) + 1
    return resp


def truth_rows(texts, turn, calls, choices, seeds, seed0, log=0):
    req = {"id": 0, "texts": list(texts), "turn": turn,
           "calls": [[n, list(a), kw] for n, a, kw in calls], "choices": choices, "seeds": seeds, "seed0": seed0, "log": log}
    resp = truth_request(req)
    if resp.get("error"):
        raise RuntimeError("showdown_truth: " + resp["error"])
    if log:
        return resp["rows"], resp.get("illegal", 0), resp.get("logs", [])
    return resp["rows"], resp.get("illegal", 0)


def run_one(k, seeds):
    try:
        texts, turn, calls, aseed = make_position(k)
        old = build("old", texts, turn, calls)
        ex = build("exact", texts, turn, calls)
        sd = setup_diffs(old, ex)
        acts = pick_actions(old, aseed)
        t = time.perf_counter()
        fo = play(old, "old", acts, seeds, 11)
        t_old = time.perf_counter() - t
        t = time.perf_counter()
        fe = play(ex, "exact", acts, seeds, 11)
        t_ex = time.perf_counter() - t
        if _G.get("truth"):
            choices = [ex.copy().choice_string(0, *acts[0]), ex.copy().choice_string(1, *acts[1])]
            ft, ill = truth_rows(texts, turn, calls, choices, seeds, 11)
            return {"k": k, "setup": sd, "acts": acts, "names": describe(ex, acts), "diffs": compare(fo, ft),
                    "diffs_exact": compare(fe, ft), "diffs_fast_exact": compare(fo, fe), "illegal": ill,
                    "t_old": t_old, "t_ex": t_ex}
        diffs = compare(fo, fe)
        return {"k": k, "setup": sd, "acts": acts, "names": describe(ex, acts), "diffs": diffs, "t_old": t_old, "t_ex": t_ex}
    except BaseException as e:  # noqa: BLE001 (Rust panics are BaseException)
        return {"k": k, "error": f"{type(e).__name__}: {e}"}


def _run(args):
    return run_one(*args)


def show(k, seeds):
    if "teams" not in _G:
        init()
    texts, turn, calls, aseed = make_position(k)
    print(texts[0], "\n---\n", texts[1], "\nturn", turn, "\n", calls)
    old = build("old", texts, turn, calls)
    ex = build("exact", texts, turn, calls)
    print("setup diffs:", setup_diffs(old, ex))
    acts = pick_actions(old, aseed)
    print("actions:", acts)
    print("exact choice strings:", ex.copy().choice_string(0, *acts[0]), "|", ex.copy().choice_string(1, *acts[1]))
    for kind, b in (("old", old), ("exact", ex)):
        for k2 in range(seeds):
            c = b.copy()
            c.reseed(11 + k2 * 1000003)
            c.enable_log()
            t0 = c.turn
            c.choose(0, *acts[0])
            c.choose(1, *acts[1])
            resolve_switches(c, kind, t0)
            print(f"===== {kind} seed {k2}")
            print("\n".join(x for x in c.log() if x.strip()))
    if _G.get("truth"):
        choices = [ex.copy().choice_string(0, *acts[0]), ex.copy().choice_string(1, *acts[1])]
        _, _, logs = truth_rows(texts, turn, calls, choices, seeds, 11, log=seeds)
        for k2, lg in enumerate(logs):
            print(f"===== SHOWDOWN seed {k2}")
            print("\n".join(x for x in lg if x.strip() and not x.startswith(("|t:", "|j", "|gametype", "|player", "|teamsize", "|gen", "|tier", "|rule", "|split", "|poke", "|start", "|clearpoke"))))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--positions", type=int, default=1000)
    ap.add_argument("--first", type=int, default=0)
    ap.add_argument("--seeds", type=int, default=100)
    ap.add_argument("--workers", type=int, default=10)
    ap.add_argument("--out", default=os.path.join(DATA, "engine_diff.json"))
    ap.add_argument("--show", type=int, default=None, help="print both engines' logs for one position")
    ap.add_argument("--show-seeds", type=int, default=3)
    ap.add_argument("--require", default="", help="comma-separated ids: side 0's first active Pokemon has one of them")
    ap.add_argument("--confirm", type=int, default=0, help="re-play every flagged position with this many seeds and keep only those still flagged (removes noise)")
    ap.add_argument("--exact-out", default="", help="with --truth: write the positions where the EXACT engine differs from Showdown here")
    ap.add_argument("--truth", action="store_true", help="also play every position in REAL Showdown (tools/showdown_truth.js); the flagged list is then fast-vs-Showdown, with the exact engine vs Showdown beside it")
    a = ap.parse_args()
    req = tuple(x for x in a.require.split(",") if x)
    if a.show is not None:
        init(req, a.truth)
        show(a.show, a.show_seeds)
        return
    jobs = [(k, a.seeds) for k in range(a.first, a.first + a.positions)]
    res = []
    t0 = time.time()
    with Pool(a.workers, initializer=init, initargs=(req, a.truth)) as pool:
        for i, r in enumerate(pool.imap_unordered(_run, jobs, chunksize=4)):
            res.append(r)
            if (i + 1) % 50 == 0 or i + 1 == len(jobs):
                el = time.time() - t0
                print(f"\rprogress: {i + 1}/{len(jobs)} positions, {el:.0f}s, ETA {el / (i + 1) * (len(jobs) - i - 1):.0f}s ",
                      end="", flush=True)
    print()
    if a.confirm:
        again = [(r["k"], a.confirm) for r in res if "error" not in r and r["diffs"]]
        print(f"confirming {len(again)} flagged positions with {a.confirm} seeds", flush=True)
        with Pool(a.workers, initializer=init, initargs=(req, a.truth)) as pool:
            redo = {r["k"]: r for r in pool.imap_unordered(_run, again, chunksize=1)}
        res = [redo.get(r["k"], r) if r["k"] in redo else r for r in res]
    res.sort(key=lambda r: r["k"])
    errs = [r for r in res if "error" in r]
    ok = [r for r in res if "error" not in r]
    flagged = [r for r in ok if r["diffs"]]
    setup = [r for r in ok if r["setup"]]
    print(f"positions: {len(ok)}  errors: {len(errs)}  setup mismatches: {len(setup)}")
    print(f"positions where the outcome distribution differs: {len(flagged)} ({100 * len(flagged) / max(1, len(ok)):.1f}%)")
    if a.truth:   # (2026-10-18) the exact engine vs real Showdown on the same positions and seeds
        fx = [r for r in ok if r.get("diffs_exact")]
        print(f"exact engine vs Showdown differs: {len(fx)} ({100 * len(fx) / max(1, len(ok)):.2f}%)")
        if a.exact_out:
            json.dump(fx, open(a.exact_out, "w"), indent=1)
    to = sum(r["t_old"] for r in ok)
    te = sum(r["t_ex"] for r in ok)
    print(f"time per played turn (incl. Python): fast {1e6 * to / (len(ok) * a.seeds):.0f} us, exact {1e6 * te / (len(ok) * a.seeds):.0f} us")
    from collections import Counter
    kinds = Counter()
    for r in flagged:
        for key in set(d[0].split(".", 1)[-1] for d in r["diffs"]):
            kinds[key] += 1
    print("disagreeing feature kinds (positions):", ", ".join(f"{k} {v}" for k, v in kinds.most_common()))
    for r in errs[:5]:
        print("error", r["k"], r["error"])
    for r in setup[:5]:
        print("setup", r["k"], r["setup"][:3])
    # which moves / abilities / items show up in disagreeing positions more than their share
    used, bad = Counter(), Counter()
    for r in ok:
        toks = set()
        for line in r["names"]:
            head, _, act = line.partition(": ")
            ab_it = head.split("[", 1)[1].rstrip("]").split("/") if "[" in head else []
            toks.update("ability:" + x for x in ab_it[:1] if x)
            toks.update("item:" + x for x in ab_it[1:2] if x)
            if act.startswith("switch to "):
                toks.add("switch-in ability:" + act.split("[", 1)[-1].rstrip("]"))
            else:
                toks.add("move:" + act.split(" ")[0])
        for t in toks:
            used[t] += 1
            if r["diffs"]:
                bad[t] += 1
    base = len(flagged) / max(1, len(ok))
    rank = sorted(used, key=lambda t: -(bad[t] - base * used[t]))
    print("most over-represented in disagreements (token: flagged/used):")
    print("  " + ", ".join(f"{t} {bad[t]}/{used[t]}" for t in rank[:30]))
    json.dump({"flagged": flagged, "errors": errs, "setup": setup}, open(a.out, "w"), indent=1)
    print("details:", a.out)


if __name__ == "__main__":
    sys.setrecursionlimit(10000)
    main()
