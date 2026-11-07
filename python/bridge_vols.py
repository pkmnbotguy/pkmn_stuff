"""
(2026-10-18) Volatiles / slot / side / field timers for a rebuilt position, in the fast engine's own fields (Battle.set_vols,
set_extra). Showdown-style values (the names and numbers of Showdown's effect states) are converted by `apply`; they come
from the battle log (bridge.volatiles_from_log, what a player can see).

Showdown-style dict per Pokemon: {vid: {"duration": d, "move": id, "hp": n, "layers": n, "time": n, "trueDuration": n,
"src": (role, pokemon index), "slot": (role, position 0/1), "targetLoc": n}}.
"""

from bridge_util import base_id
import re as _re


def to_id(x):
    return _re.sub(r"[^a-z0-9]", "", str(x).lower())
import os
DEBUG = bool(os.environ.get("BRIDGE_DEBUG"))
TRAP_SELF = 254
NONE = 255
FLAGS = ("libero", "transform", "flashfire", "charge", "focusenergy", "destinybond", "minimize", "curse", "noretreat", "glaiverush", "mustrecharge",
         "imprison", "aquaring", "saltcure", "electrify", "powertrick", "smackdown", "torment", "ingrain", "gastroacid", "roost",
         "statsraised", "statslowered", "hurtthisturn", "useditem")
TIMED = {"taunt": "taunt", "throatchop": "throatchop", "magnetrise": "magnetrise", "yawn": "yawn", "healblock": "healblock",
         "perishsong": "perish"}


ORDER_MOVES = {"payback": True, "fishiousrend": False, "boltbeak": False}


SUB_RANGE = {}   # (2026-10-19) (engine side, base id) -> (HP with every hit at its max roll, HP with every hit at its min roll)


def sub_after_hits(eb, st, src_of, hp, hits, s, k):
    """Substitute HP after the hits it took (the log hides the amounts): median damage roll of each hit in the CURRENT
    position with the stat stages AT the hit (tracked from the log; today's when unknown); the Substitute survived, so
    >= 1."""
    lo = hi = hp
    for hit in hits:
        att, mid = hit[0], hit[1]
        a = src_of(att)
        if not a:
            continue
        mv = [x for x, _pp in st["sides"][a[0]]["mons"][a[1]]["moves"]]
        if mid not in mv:
            continue
        try:
            b = eb
            if len(hit) > 4:
                b = eb.copy()
            if len(hit) > 4 and hit[3] is not None and hit[4] is not None:
                # the stat stages at the hit (from the log), not today's (2026-10-18: Close Combat's drops after the hit)
                crit = len(hit) > 5 and bool(hit[5])
                for who, ((sd_, ix), bo) in enumerate((((a[0], a[1]), hit[3]), ((s, k), hit[4]))):
                    cur = st["sides"][sd_]["mons"][ix].get("boosts") or {}
                    nb = {n: int(bo.get(n, 0)) for n in cur}
                    if crit:   # (2026-10-19) a crit ignores the attacker's drops and the defender's boosts
                        nb = {n: (max(0, x) if who == 0 else min(0, x)) if n in ("atk", "def", "spa", "spd") else x
                              for n, x in nb.items()}
                    b.set_mon(sd_, ix, boosts=nb)
            if len(hit) > 10 and hit[10] is not None:
                try:   # (2026-10-19) the attacker's status AT the hit (Facade, Guts; burn halves physical hits)
                    if hit[10] != (st["sides"][a[0]]["mons"][a[1]].get("status") or ""):
                        b.set_mon(a[0], a[1], status=hit[10] or "")
                except Exception:
                    pass
            if len(hit) > 11 and hit[11]:
                try:
                    b.set_mon(a[0], a[1], item=hit[11])         # Fling: the item it threw (gone from the position now)
                except Exception:
                    pass
            if len(hit) > 9:
                try:
                    b.set_vols(s, k, {"roost": 1 if hit[9] else 0})   # Roost's lost Flying type AT the hit
                except Exception:
                    pass
            if len(hit) > 7:
                try:
                    b.set_field(weather=hit[6] or "none", terrain=hit[7] or "none")   # the field at the hit
                except Exception:
                    pass
            ctx = hit[12] if len(hit) > 12 and isinstance(hit[12], dict) else None
            if ctx is not None:
                try:   # (2026-10-19) the defender side's screens AT the hit
                    on = set(ctx.get("screens") or []) if not (len(hit) > 5 and hit[5]) else set()   # a crit ignores screens
                    if b is eb:
                        b = eb.copy()
                    b.set_side(s, reflect=5 if "reflect" in on else 0, light_screen=5 if "lightscreen" in on else 0,
                               aurora_veil=5 if "auroraveil" in on else 0)
                    if ctx.get("wonderroom") is not None:      # Wonder Room AT the hit swaps Def / SpD
                        b.set_extra(s, wonder_room=5 if ctx["wonderroom"] else 0)
                    if ctx.get("tstatus") is not None and ctx["tstatus"] != (st["sides"][s]["mons"][k].get("status") or ""):
                        b.set_mon(s, k, status=ctx["tstatus"] or "")      # the defender's status AT the hit (Hex...)
                    for (sd_, ix), fr in zip(((a[0], a[1]), (s, k)), ctx.get("hp") or []):
                        # (2026-10-19) HP AT the hit (Endeavor, Super Fang, Eruption, Flail...: today's HP gave other damage)
                        if fr is not None and fr > 0:
                            mh = st["sides"][sd_]["mons"][ix]["maxhp"]
                            b.set_mon(sd_, ix, hp=max(1, min(mh, round(fr * mh))))
                except Exception:
                    pass
            order = mid in ORDER_MOVES and len(hit) > 8
            try:                                               # spread AT the hit (the log's '[spread]' tag)
                rolls = b.damage_rolls(a[0], a[1], mv.index(mid), s, k, state_bp=not order, spread=bool(hit[2]))
            except TypeError:                                  # older engine build
                rolls = b.damage_rolls(a[0], a[1], mv.index(mid), s, k, state_bp=not order)
            if order and rolls and ORDER_MOVES[mid] == bool(hit[8]):
                rolls = [r * 2 for r in rolls]                 # doubled by the move order AT the hit (from the log)
        except Exception:
            rolls = None
        if not rolls:
            continue
        ds = []
        for d in (sorted(rolls)[len(rolls) // 2], max(rolls), min(rolls)):
            if len(hit) > 12 and isinstance(hit[12], dict) and hit[12].get("hh"):
                d = d * 3 // 2                                 # Helping Hand AT the hit (x6144/4096 base power)
            if len(hit) > 5 and hit[5]:
                d = d * 3 // 2                                 # critical hit (stage rules ignored)
            ds.append(max(1, d))
        hp = max(1, hp - ds[0])
        lo = max(1, lo - ds[1])
        hi = max(1, hi - ds[2])
    try:
        m = st["sides"][s]["mons"][k]
        SUB_RANGE[(s, base_id(m.get("set_species", m["species"])))] = (lo, hi)
    except Exception:
        pass
    return hp


def apply(eb, me_role, vols, sides, pw):
    """Write Showdown-style values into the engine (side 0 = me_role). sides[role]["names"] = base ids in Showdown's
    side.pokemon order, for 'src' references."""
    st = eb.state()
    roles = [me_role, "p2" if me_role == "p1" else "p1"]
    eng_side = {roles[0]: 0, roles[1]: 1}
    eidx = [{base_id(m.get("set_species", m["species"])): k for k, m in enumerate(sd["mons"])} for sd in st["sides"]]

    def src_of(ref):
        if not ref:
            return None
        role, k = ref
        if isinstance(k, str):                                # (role, base id): from the log tracker
            return (eng_side[role], eidx[eng_side[role]][k]) if role in eng_side and k in eidx[eng_side[role]] else None
        nm = (sides.get(role) or {}).get("names", [])
        if role not in eng_side or k >= len(nm) or nm[k] not in eidx[eng_side[role]]:
            return None
        return eng_side[role], eidx[eng_side[role]][nm[k]]

    # (2026-10-19 bridge flag 17320) a rebuilt position starts every lead Zoroark disguised (from_brought); only the
    # '__illusion' entries below keep a disguise, every other one is off (Transform / Imposter into a revealed Zoroark work)
    for s_, sd_ in enumerate(st["sides"]):
        for k_, m_ in enumerate(sd_["mons"]):
            if (m_.get("vols") or {}).get("illusion"):
                try:
                    eb.set_vols(s_, k_, {"illusion": 0})
                except ValueError:                            # older engine build: no 'illusion' name
                    pass
    # first pass: Transform (it replaces moves / types, which the other entries refer to)
    done_tf = False
    for role in roles:
        for nm, v in (vols.get(role) or {}).items():
            if "__transform" in v and nm in eidx[eng_side[role]]:
                t = src_of(v["__transform"].get("into"))
                ok = False
                if t:
                    try:
                        ok = eb.transform(eng_side[role], eidx[eng_side[role]][nm], t[0], t[1],
                                          pre_mega=bool(v["__transform"].get("pre")))
                    except TypeError:                         # older engine build
                        ok = eb.transform(eng_side[role], eidx[eng_side[role]][nm], t[0], t[1])
                    # (2026-10-19 bridge flag 15403) copied before the target Mega Evolved: its ability then, not the Mega's
                    pa = v["__transform"].get("pre_ability")
                    if (ok and pa and v["__transform"].get("pre") and hasattr(eb, "set_ability")
                            and eb.state()["sides"][t[0]]["mons"][t[1]].get("is_mega")):
                        try:
                            eb.set_ability(eng_side[role], eidx[eng_side[role]][nm], pa)
                        except ValueError:
                            pass
                done_tf |= bool(ok)
                if DEBUG and not ok:
                    print(f"[bridge_vols] {role} {nm}: transform into {v['__transform'].get('into')} failed", flush=True)
    if done_tf:
        st = eb.state()
    for role in roles:
        s = eng_side[role]
        for nm, v in (vols.get(role) or {}).items():
            k = eidx[s].get(nm)
            if k is None:
                continue
            m = st["sides"][s]["mons"][k]
            slot_of = {mid: j for j, (mid, _pp) in enumerate(m["moves"])}
            out = {}

            def call_move(mid, out=out):
                # (2026-10-19 bridge fuzz) a lock / Uproar / charge a caller (Copycat) started runs a move outside the
                # moveset: engine slot CALL_SLOT (6) + the move's dex index. False when the engine can't hold it.
                ix = eb.move_index(to_id(mid)) if hasattr(eb, "move_index") else None
                if ix is None:
                    return False
                out["call_move"] = ix
                return True
            for vid, e in v.items():
                d = e.get("duration", 0)
                if vid == "__gender":
                    pass
                elif vid == "__transform":
                    pass                                      # done in the first pass
                elif vid == "__types":
                    # (2026-10-19) no "types" = only an added type (Trick-or-Treat / Forest's Curse): the engine's own types stay
                    ts = [t for t in (e["types"] if "types" in e else m.get("types", [])) if t != "???"]
                    if sorted(ts) != sorted(m.get("types", [])) or (e.get("added") or "") != m.get("added_type", ""):
                        try:
                            eb.set_types(s, k, ts, e.get("added") or "")
                        except Exception as ex:                # never let one bad entry crash a decision
                            if DEBUG:
                                print(f"[bridge_vols] {role} {nm}: set_types {ts} failed: {ex}", flush=True)
                elif vid == "__hist":
                    mask = 0
                    for mid in e.get("used") or []:
                        if mid in slot_of:
                            mask |= 1 << slot_of[mid]
                    if k not in (st["sides"][s].get("active") or []):
                        mask = 0                                   # engine clears it on switch-OUT, Showdown on switch-in
                    out["usedmask"] = mask
                    out["timesattacked"] = min(255, int(e.get("attacked", 0)))
                    out["fallen"] = int(e.get("fallen", 0))
                elif vid == "__activeturns":
                    out["active_turns"] = min(255, int(e.get("n", 0)))
                elif vid == "__ateberry":
                    out["ateberry"] = 1
                elif vid == "__syrupdone":
                    out["syrupdone"] = 1
                elif vid == "__illusion":
                    j_ = eidx[s].get(e.get("as"))
                    if j_ is not None and j_ != k:
                        out["illusion"] = 1
                        out["illusion_as"] = j_
                elif vid == "__lastloc":
                    out["last_target"] = max(-2, min(2, int(e.get("loc", 0))))
                elif vid == "__lastitem":
                    if (m.get("last_item") or "") != (e.get("item") or "") and hasattr(eb, "set_last_item"):
                        try:
                            eb.set_last_item(s, k, e.get("item") or None)
                        except ValueError:
                            pass
                elif vid == "__status":
                    if m["status"] == e["status"]:
                        out["status_turns"] = e["stage"] if e["status"] == "tox" else e["time"]
                        if e["status"] == "tox" and k not in (st["sides"][s].get("active") or []):
                            out["status_turns"] = 0                # engine resets the stage on switch-OUT, Showdown on switch-in
                elif vid in TIMED:
                    out[TIMED[vid]] = d
                elif vid in FLAGS:
                    out[vid] = 1
                elif vid == "dragoncheer":
                    out["dragoncheer"] = 1
                    out["dragoncheer2"] = 1 if e.get("hasDragonType") else 0
                elif vid == "confusion":
                    out["confusion"] = e.get("time", 0)
                elif vid == "encore" and e.get("move") in slot_of:
                    out["encore"], out["encore_slot"] = d, slot_of[e["move"]]
                elif vid == "disable" and e.get("move") in slot_of:
                    out["disable"], out["disable_slot"] = d, slot_of[e["move"]]
                elif vid == "metronome" and "metronome" in (m.get("item"), m.get("stash_item")) and e.get("off"):
                    out["metronome"], out["metronome_slot"] = 0, 255                 # (2026-10-19) no volatile (Magic Room)
                elif vid == "metronome" and "metronome" in (m.get("item"), m.get("stash_item")):   # stashed under Magic Room
                    out["metronome"] = int(e.get("numConsecutive", 0)) + 1          # (2026-10-18) Metronome item streak
                    out["metronome_slot"] = slot_of.get(to_id(str(e.get("lastMove") or "")), 255)
                elif vid == "uproar":
                    out["uproar"], out["uproar_slot"] = d, slot_of.get("uproar", 6)
                    if "uproar" not in slot_of:
                        call_move("uproar")                # (2026-10-19) started by Copycat: not in the moveset
                elif vid == "substitute":
                    hf = src_of(e.get("hp_from"))                  # Shed Tail: the user's quarter
                    out["substitute"] = e["hp"] if "hp" in e else (st["sides"][hf[0]]["mons"][hf[1]]["maxhp"] // 4 if hf
                                                                      else m["maxhp"] // 4)
                    if "hp" not in e:
                        out["substitute"] = sub_after_hits(eb, st, src_of, out["substitute"], e.get("hits") or [], s, k)
                elif vid == "allyswitch":
                    # (2026-10-18) Ally Switch's own stall counter (gen 9): 1 in counter to work again while it lasts
                    out["allyswitch"], out["allyswitch_counter"] = d, e.get("counter", 3)
                elif vid == "stockpile":
                    out["stockpile"] = e.get("layers", 0)
                elif vid == "lockedmove" and e.get("move") and (e["move"] in slot_of or call_move(e["move"])):
                    out["lock_slot"], out["lockedmove"] = slot_of.get(e["move"], 6), d
                    out["lock_true"] = e.get("trueDuration", d)
                elif vid == "twoturnmove" and e.get("move") and e.get("move") in v and (e["move"] in slot_of
                                                                                      or call_move(e["move"])):
                    # (Showdown keeps 'twoturnmove' until the residual after the release; the move's own volatile, e.g.
                    # 'solarbeam', is what says it is still charging)
                    out["twoturnmove"], out["charge_slot"] = 1, slot_of.get(e["move"], 6)
                    tl = (v.get(e["move"]) or {}).get("targetLoc")
                    if tl is not None:
                        out["charge_target"] = tl
                elif vid == "partiallytrapped":
                    out["partiallytrapped"], out["bound_div"] = d, 8
                    src = src_of(e.get("src"))
                    if src:
                        out["bound_side"], out["bound_src"] = src
                elif vid == "attract":
                    out["attract"] = 1
                    src = src_of(e.get("src"))
                    if src:
                        out["attract_side"], out["attract_src"] = src
                elif vid == "trapped":
                    src = src_of(e.get("src"))
                    if src and src == (s, k):
                        out["trap_src"] = TRAP_SELF
                    elif src:
                        out["trap_side"], out["trap_src"] = src
                elif vid == "octolock":
                    src = src_of(e.get("src"))
                    if src:
                        out["octolock_side"], out["octolock_src"] = src
                elif vid == "syrupbomb":
                    out["syrupbomb"] = d
                    src = src_of(e.get("src"))
                    if src:
                        out["syrup_side"], out["syrup_src"] = src
                elif vid == "leechseed" and e.get("slot"):
                    out["leechseed"] = 1
                    out["leech_side"], out["leech_slot"] = eng_side[e["slot"][0]], e["slot"][1]
            if out:
                for _ in range(len(out)):
                    try:
                        eb.set_vols(s, k, out)
                        break
                    except ValueError as e_:                  # older engine build without a newer name: keep the rest
                        bad = str(e_).rsplit(" ", 1)[-1]
                        if bad not in out:
                            raise
                        del out[bad]
                        if not out:
                            break
            if DEBUG:
                for vid, e in v.items():
                    if e.get("src") and not src_of(e["src"]):
                        print(f"[bridge_vols] {role} {nm} {vid}: source {e['src']} not found", flush=True)
        if DEBUG:
            for j, sc in enumerate((sides.get(role) or {}).get("slots") or []):
                if "futuremove" in sc and not src_of(sc["futuremove"].get("src")):
                    print(f"[bridge_vols] {role} slot {j} futuremove: source {sc['futuremove'].get('src')} not found "
                          f"(have {sorted(eidx[0])} / {sorted(eidx[1])})", flush=True)
        sd_ = sides.get(role) or {}
        extra = {}
        wish, fut = [(0, 0), (0, 0)], [(0, 0, 0), (0, 0, 0)]
        hw = [False, False]
        for j, sc in enumerate((sd_.get("slots") or [])[:2]):
            if "healingwish" in sc:
                hw[j] = True
            if "wish" in sc:
                hp = sc["wish"].get("hp")
                if not hp:                                    # the log doesn't show it: half the wisher's max HP
                    w = src_of(sc["wish"].get("src"))
                    hp = st["sides"][w[0]]["mons"][w[1]]["maxhp"] // 2 if w else 0
                wish[j] = (sc["wish"].get("duration", 0), int(hp))
            if "futuremove" in sc:
                src = src_of(sc["futuremove"].get("src"))
                if src:
                    fut[j] = (sc["futuremove"].get("duration", 0), src[0], src[1])
        if any(w[0] for w in wish):
            extra["wish"] = wish
        if any(f[0] for f in fut):
            extra["future"] = fut
        if any(hw):
            extra["healing_wish"] = hw
        if "safeguard" in (sd_.get("side") or {}):
            extra["safeguard"] = sd_["side"]["safeguard"].get("duration", 0)
        if s == 0:
            for pid, key in (("gravity", "gravity"), ("magicroom", "magic_room"), ("wonderroom", "wonder_room"),
                             ("fairylock", "fairy_lock")):
                if pid in pw:
                    extra[key] = pw[pid].get("duration", 0)
        if s == 0 and "__lastmove" in pw and hasattr(eb, "set_last_move"):
            try:
                eb.set_last_move(pw["__lastmove"] or None)
            except ValueError:
                pass
        if extra:
            try:
                eb.set_extra(s, **extra)
            except TypeError:                                  # an engine build without healing_wish / fairy_lock
                extra.pop("healing_wish", None)
                extra.pop("fairy_lock", None)
                eb.set_extra(s, **extra)
    apply_stats(eb, roles, eng_side, eidx, pw)
    if hasattr(eb, "set_ability"):
        st = eb.state()
        for role in roles:
            s = eng_side[role]
            for nm, v in (vols.get(role) or {}).items():
                k = eidx[s].get(nm)
                a = (v.get("__ability") or {}).get("ability")
                if k is not None and a and st["sides"][s]["mons"][k].get("ability") != a:
                    try:
                        eb.set_ability(s, k, a)
                    except ValueError:
                        pass


_STATS = ("atk", "def", "spa", "spd", "spe")


def _forme_stats(stats, cur_sp, new_sp):
    """(2026-10-19) stats of the same Pokemon (same stat points and nature) in another forme: Champions stat = (base + points
    + 20) x nature, solved per stat from the current forme's value"""
    try:
        import bridge
        dx = bridge.gen9().pokedex
        b0, b1 = dx[cur_sp]["baseStats"], dx[new_sp]["baseStats"]
    except Exception:
        return dict(stats)
    out = {}
    for n, v in stats.items():
        if n not in b0 or n not in b1 or b0[n] == b1[n]:
            out[n] = v
            continue
        out[n] = v
        for nat in (100, 110, 90):
            hit = [sp for sp in range(33) if (b0[n] + sp + 20) * nat // 100 == v]
            if hit:
                out[n] = (b1[n] + hit[0] + 20) * nat // 100
                break
    return out


def apply_stats(eb, roles, eng_side, eidx, pw):
    """(2026-10-18) stored stats of the active Pokemon: Showdown's truth (pw['__stored']) or the log's Guard Split / Power
    Split / Speed Swap / Power Trick replayed on the rebuilt stats (pw['__statops']). A changed Pokemon gets the 'split'
    volatile: the engine restores its species' stats when it switches out, as Showdown does."""
    st = eb.state()
    sides = st["sides"]
    act = {(s, k) for s, sd in enumerate(sides) for k in (sd.get("active") or []) if k is not None}
    base = lambda s, k: {n: sides[s]["mons"][k]["stats"][n] for n in _STATS}
    def where(key):
        role, nm = key
        if role not in eng_side or nm not in eidx[eng_side[role]]:
            return None
        return eng_side[role], eidx[eng_side[role]][nm]
    final = {}
    if pw.get("__stored"):
        for key, stats in pw["__stored"].items():
            w = where(key)
            if w is not None and w in act:
                final[w] = {n: stats[n] for n in _STATS if n in stats}
    elif pw.get("__statops"):
        cur, forme = {}, {}

        def start(w):
            # (2026-10-19) the Pokemon's stats in the forme it had AT THE TIME of the operation (Guard Split with a Shield
            # forme Aegislash that is Blade now)
            st_ = base(*w)
            f_ = forme.get(w)
            sp_ = to_id(sides[w[0]]["mons"][w[1]].get("species", ""))
            return _forme_stats(st_, sp_, f_) if f_ and sp_ and f_ != sp_ else st_
        for op in pw["__statops"]:
            if op[0] == "forme":
                w = where(op[1])
                if w is not None:
                    forme[w] = op[2]
                continue
            if op[0] == "reset":
                w = where(op[1])
                if w is not None:
                    cur.pop(w, None)
            elif op[0] == "copy":                           # (2026-10-19) Transform: the target's stats as replayed so far
                a, b = where(op[1]), where(op[2])
                if a is not None and b is not None and a != b and b in cur:
                    cur[a] = {n: v for n, v in cur[b].items()}
            elif op[0] == "trick":
                w = where(op[1])
                if w is not None:
                    c = cur.setdefault(w, start(w))
                    c["atk"], c["def"] = c["def"], c["atk"]
            else:
                a, b = where(op[1]), where(op[2])
                if a is None or b is None or a == b:
                    continue
                ca, cb = cur.setdefault(a, start(a)), cur.setdefault(b, start(b))
                for n in op[3]:
                    if op[0] == "split":
                        ca[n] = cb[n] = (ca[n] + cb[n]) // 2
                    else:
                        ca[n], cb[n] = cb[n], ca[n]
        final = {w: c for w, c in cur.items() if w in act}
    for (s, k), stats in final.items():
        if stats == base(s, k):
            continue
        eb.set_mon(s, k, stats=stats)
        try:
            eb.set_vols(s, k, {"split": 1})
        except (ValueError, TypeError):
            pass                                              # an engine build without the 'split' name
