"""Mid-turn switch-ins see the rest of the turn (2026-10-22, arena game 5199).

A mid-turn switch request (U-turn, Parting Shot, Flip Turn, Eject Button, Emergency Exit...) is searched from a rebuilt
position whose action queue is empty (bridge_util.forced_setup -> set_switching), so the Pokemon that had not acted yet
never act there: every switch-in looked safe. In 5199 our Parting Shot brought Gholdengo into the slot an unmoved Mimikyu
was aiming Shadow Claw at; it was KO'd (crit) at once.

Here the moves still to come are put back (Battle.queue_move): our unmoved partner's move from this turn's choice, the
opponent's unmoved Pokemon's moves from the opponent mix this turn's search assumed (the agent stores both in
battle._turn_plan, by species + move ID). Each switch option's search value gets a correction from the value net:
E[value after the rest of the turn] - value without it, over SEEDS common seeds x the TOPK likeliest opponent actions
(positions that need a replacement after a KO: deep3's replacement solve). AGENT_MIDTURN=0 turns it off."""

import os
import numpy as np

ON = os.environ.get("AGENT_MIDTURN", "1") == "1"
SEEDS = int(os.environ.get("MIDTURN_SEEDS", "8"))
TOPK = int(os.environ.get("MIDTURN_TOPK", "3"))


def _ev(e):
    return e.split("|") if isinstance(e, str) else list(e)


def unmoved(battle):
    """{(engine side, slot)} of Pokemon on the field now that have not acted this turn (no move / cant line, did not
    switch in this turn), from the log; engine side 0 = us"""
    role = battle.player_role or "p1"
    log = [_ev(e) for e in (getattr(battle, "_replay_data", None) or [])]
    start = 0
    for n, ev in enumerate(log):
        if len(ev) > 1 and ev[1] == "turn":
            start = n + 1
    occ = {}                                 # position 'p1a' -> name
    acted = set()                            # (side 'p1', name)
    fake_out = None                          # target of a Fake Out this turn: flinches if it is hit before acting
    for n, ev in enumerate(log):
        if len(ev) < 3:
            continue
        kind = ev[1]
        ident = str(ev[2])
        pos = ident[:3]
        name = ident.split(":", 1)[1].strip() if ":" in ident else ""
        if kind in ("switch", "drag", "replace"):
            occ[pos] = name
            if n >= start:
                acted.add((pos[:2], name))
        elif kind == "swap" and len(pos) == 3:
            other = pos[:2] + ("b" if pos[2] == "a" else "a")
            occ[pos], occ[other] = occ.get(other), occ.get(pos)
        elif n >= start and kind in ("move", "cant"):
            rest = "|".join(str(x) for x in ev[3:])
            if kind == "move" and ("Dancer" in rest or "move: Instruct" in rest):
                continue
            acted.add((pos[:2], name))
            fake_out = None
            if kind == "move" and len(ev) > 4 and str(ev[3]) == "Fake Out" and ":" in str(ev[4]):
                fake_out = (str(ev[4])[:2], str(ev[4]).split(":", 1)[1].strip())
        elif n >= start and kind == "-damage" and fake_out is not None and len(ev) > 3 and "[from]" not in "|".join(ev[3:]):
            if (pos[:2], name) == fake_out:
                acted.add(fake_out)          # flinched (Inner Focus / Covert Cloak ignored)
            fake_out = None
    opp = "p2" if role == "p1" else "p1"
    out = set()
    for side, r in ((0, role), (1, opp)):
        for k, letter in enumerate("ab"):
            nm = occ.get(r + letter)
            if nm and (r, nm) not in acted:
                out.add((side, k))
    return out


def _species(m):
    return str(m.get("set_species") or m.get("species") or "")


def _match(named, st, side, k):
    """the planned (move slot, target) for the Pokemon now in (side, k): named = per slot (species, move id, target)"""
    act = st["sides"][side]["active"]
    a = act[k] if k < len(act) else None
    if a is None or a < 0:
        return None
    mon = st["sides"][side]["mons"][a]
    if mon.get("fainted") or mon.get("hp", 1) <= 0:
        return None
    for ent in named:
        if ent is None or ent[0] not in (_species(mon), str(mon.get("species"))):
            continue
        ids = [mv[0] for mv in mon["moves"]]
        if ent[1] in ids:
            return ids.index(ent[1]), int(ent[2])
    return None


def _norm(x):
    return "".join(ch for ch in str(x).lower() if ch.isalnum())


def _did_moves(battle):
    """{normalised name: set of move ids} the opponent's Pokemon used this turn (own moves, called ones skipped)"""
    role = battle.player_role or "p1"
    opp = "p2" if role == "p1" else "p1"
    log = [_ev(e) for e in (getattr(battle, "_replay_data", None) or [])]
    start = 0
    for n, ev in enumerate(log):
        if len(ev) > 1 and ev[1] == "turn":
            start = n + 1
    did = {}
    for ev in log[start:]:
        if len(ev) > 3 and ev[1] == "move" and str(ev[2]).startswith(opp) and ":" in str(ev[2]):
            rest = "|".join(str(x) for x in ev[4:])
            if "[from]" in rest and "lockedmove" not in rest and "move: Round" not in rest:
                continue
            did.setdefault(_norm(str(ev[2]).split(":", 1)[1]), set()).add(_norm(ev[3]))
    return did


def _consistent(named, did):
    """(2026-10-22, arena game 5855) a joint action of the assumed mix fits this turn's log unless one of its Pokemon
    already used a DIFFERENT move this turn (the unmoved partner's move is then drawn from the matching joints only)"""
    for ent in named or ():
        if ent is None:
            continue
        sp = _norm(ent[0])
        for nm, mids in did.items():
            if nm and (sp == nm or sp.startswith(nm) or nm.startswith(sp)) and _norm(ent[1]) not in mids:
                return False
    return True


def plan_of(battle, e):
    """(our queued moves [(slot, mslot, target)], their candidates [(p, [(slot, mslot, target)])]) or None"""
    plan = getattr(battle, "_turn_plan", None)
    if not ON or not plan or plan.get("turn") != battle.turn or not hasattr(e, "queue_move"):
        return None
    un = unmoved(battle)
    if not un:
        return None
    st = e.state()
    ours = []
    for k in (0, 1):
        if (0, k) in un:
            m = _match(plan["ours"], st, 0, k)
            if m:
                ours.append((k,) + m)
    agg = {}
    did = _did_moves(battle)
    rows = [(p, named) for p, named in plan["theirs"] if _consistent(named, did)]
    if not rows or sum(p for p, _ in rows) <= 0:      # nothing in the assumed mix fits what they did: unconditioned
        rows = plan["theirs"]
    for p, named in rows:
        key = tuple((k,) + m for k in (0, 1) if (1, k) in un for m in [_match(named, st, 1, k)] if m)
        agg[key] = agg.get(key, 0.0) + float(p)
    cands = sorted(agg.items(), key=lambda kv: -kv[1])[:TOPK]
    tot = sum(p for _, p in cands)
    if tot <= 0:
        cands, tot = [((), 1.0)], 1.0
    cands = [(p / tot, list(key)) for key, p in cands]
    if not ours and all(not key for _, key in cands):
        return None
    return ours, cands


def delta(net, e, ours, theirs, plan, device, hidden, seed):
    """[len(ours), len(theirs)] corrections (value with the rest of the turn - without), from the value net"""
    import deep3
    import vgcsim
    our_q, cands = plan
    need1 = e.needs_choice(1)
    nodes, refs = [], []                     # refs: (a, b, candidate index; -1 = without the rest of the turn)
    for a, (_, o0, o1) in enumerate(ours):
        for b, (_, t0, t1) in enumerate(theirs):
            for ci in [-1] + list(range(len(cands))):
                for s in range(SEEDS):
                    c = e.copy()
                    c.reseed(int(seed) + 7919 * s + 1)
                    if ci >= 0:
                        for (k, ms, tg) in our_q:
                            c.queue_move(0, k, ("move", ms, tg, False))
                        for (k, ms, tg) in cands[ci][1]:
                            c.queue_move(1, k, ("move", ms, tg, False))
                    c.choose(0, tuple(o0), tuple(o1))
                    if need1:
                        c.choose(1, tuple(t0), tuple(t1))
                    nodes.append(c)
                    refs.append((a, b, ci))
    vals = np.zeros(len(nodes))
    F, I, live, sw = [], [], [], []
    for n, c in enumerate(nodes):
        if c.phase == "ended":
            vals[n] = 1.0 if c.winner == 0 else (-1.0 if c.winner == 1 else 0.0)
        elif c.phase == "turn" and c.needs_choice(0) and c.needs_choice(1):
            f, ii = vgcsim.encode_battle(c, 0, hidden=hidden)
            F.append(f)
            I.append(ii)
            live.append(n)
        else:
            sw.append(n)
    if live:
        vals[live] = deep3._net(net, np.asarray(F, np.float32).reshape(len(F), -1),
                                np.asarray(I, np.int32).reshape(len(I), -1), device)[1]
    if sw:
        vals[sw] = deep3._replacement_values(net, [nodes[n] for n in sw], device, hidden)
    D = np.zeros((len(ours), len(theirs)))
    base = np.zeros_like(D)
    for (a, b, ci), v in zip(refs, vals):
        if ci < 0:
            base[a, b] += v / SEEDS
        else:
            D[a, b] += cands[ci][0] * v / SEEDS
    return D - base


def store_plan(battle, r, c, b0):
    """this turn's chosen joint action and the opponent mix the search assumed, by species + move ID"""
    st = b0.state()

    def named(side, tup):
        out = []
        for k, t in enumerate(tup):
            act = st["sides"][side]["active"]
            a = act[k] if k < len(act) else None
            if t[0] == "move" and a is not None and a >= 0 and t[1] < len(st["sides"][side]["mons"][a]["moves"]):
                mon = st["sides"][side]["mons"][a]
                out.append((_species(mon), mon["moves"][t[1]][0], int(t[2])))
            else:
                out.append(None)
        return out

    import sh_search
    act_t = r["y_survivors"][0]
    mix = np.asarray(sh_search.base_y(r), np.float64)   # (2026-10-31) deepened equilibrium
    theirs = [(float(mix[j]), named(1, r["theirs"][bi][1:3])) for j, bi in enumerate(act_t) if mix[j] > 1e-4]
    battle._turn_plan = {"turn": battle.turn, "ours": named(0, r["ours"][c][1:3]), "theirs": theirs}
