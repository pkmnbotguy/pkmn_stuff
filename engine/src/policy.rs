//! Legal actions, and the simple random policy used for tests and rollouts.

use crate::battle::{Action, Battle, Phase};
use crate::dex::{Dex, Target};
use crate::state::Rng;
use arrayvec::ArrayVec;

/// The legal actions of one slot.
pub type SlotActions = ArrayVec<Action, 40>;

/// Bench Pokemon (alive, not active) of a side, minus `taken`.
#[inline]
fn bench_list(b: &Battle, side: usize, taken: &[u8]) -> ArrayVec<u8, 6> {
    let sd = &b.sides[side];
    let mut out = ArrayVec::new();
    for k in 0..sd.mons.len() {
        let k8 = k as u8;
        if sd.active[0] != k8 && sd.active[1] != k8 && sd.mons[k].alive() && !taken.contains(&k8) {
            out.push(k8);
        }
    }
    out
}

/// Opposing target locations (1, 2) with an alive Pokemon.
#[inline]
fn foe_locs(b: &Battle, side: usize) -> ArrayVec<i8, 3> {
    let mut t = ArrayVec::new();
    for f in 0..2 {
        if b.active(1 - side, f).is_some() {
            t.push(f as i8 + 1);
        }
    }
    t
}

/// Every legal action for each active slot of a side (in the current phase).
/// Moves aimed at one opponent are listed once per alive foe; switches once per bench Pokemon.
/// The two slots are listed independently (don't send both to the same bench Pokemon).
pub fn legal_actions(b: &Battle, dex: &Dex, side: usize) -> [SlotActions; 2] {
    let mut out: [SlotActions; 2] = [SlotActions::new(), SlotActions::new()];
    let bench = bench_list(b, side, &[]);
    if b.phase == Phase::Switch {
        for k in 0..2 {
            if b.switch_needed[side][k] && b.reviving[side][k] {
                // Revival Blessing: any fainted Pokemon (one still in an active slot too)
                for (j, m) in b.sides[side].mons.iter().enumerate() {
                    if m.fainted {
                        out[k].push(Action::Switch { to: j as u8 });
                    }
                }
            } else if b.switch_needed[side][k] {
                for &to in &bench {
                    out[k].push(Action::Switch { to });
                }
            }
            if out[k].is_empty() {
                out[k].push(Action::Pass);
            }
        }
        return out;
    }
    for k in 0..2 {
        let Some(i) = b.active(side, k) else {
            out[k].push(Action::Pass);
            continue;
        };
        let mega_ok = b.can_mega(side, i);
        let usable = b.usable_moves(dex, side, i);
        if usable.is_empty() {
            out[k].push(Action::Move { slot: 4, target: 0, mega: false });
        }
        for &slot in &usable {
            let mv = &dex.moves[b.mon(side, i).moves[slot as usize] as usize];
            // a non-Ghost's Curse targets itself (Showdown nonGhostTarget: the request takes no target)
            let self_curse = mv.kind == crate::effects::Mv::Curse && !b.mon(side, i).has_type(dex.ty.ghost);
            let targets: ArrayVec<i8, 3> = match mv.target {
                Target::Normal if !self_curse => {
                    let mut t = foe_locs(b, side);
                    if b.active(side, 1 - k).is_some() {
                        t.push(-((1 - k) as i8 + 1));
                    }
                    if t.is_empty() {
                        t.push(1);
                    }
                    t
                }
                Target::AdjacentAlly => {
                    // Showdown wants the ally's location even when the ally has fainted (the move then fails with
                    // [notarget])
                    let mut t = ArrayVec::new();
                    t.push(-((1 - k) as i8 + 1));
                    t
                }
                Target::AdjacentAllyOrSelf => {
                    // (2026-10-18) Acupressure / Champions' Milk Drink: the user itself, or the ally while it is there
                    // (only the ally was offered: the search never self-healed with Milk Drink)
                    let mut t = ArrayVec::new();
                    t.push(-(k as i8 + 1));
                    if b.active(side, 1 - k).is_some() {
                        t.push(-((1 - k) as i8 + 1));
                    }
                    t
                }
                _ => {
                    let mut t = ArrayVec::new();
                    t.push(0i8);
                    t
                }
            };
            for t in targets {
                out[k].push(Action::Move { slot, target: t, mega: false });
                if mega_ok {
                    out[k].push(Action::Move { slot, target: t, mega: true });
                }
            }
        }
        if b.mon(side, i).charge_slot == crate::state::NONE && !b.trapped(dex, side, i) {
            for &to in &bench {
                out[k].push(Action::Switch { to });
            }
        }
    }
    out
}

/// The random policy shared with tools/showdown_random_battles.js: Mega Evolve when possible,
/// switch 10% of the time, otherwise a random usable move at a random opponent.
pub fn random_actions(b: &Battle, dex: &Dex, side: usize, rng: &mut Rng) -> [Action; 2] {
    let mut out = [Action::Pass; 2];
    let mut taken: ArrayVec<u8, 2> = ArrayVec::new();
    if b.phase == Phase::Switch {
        for k in 0..2 {
            if b.switch_needed[side][k] && b.reviving[side][k] {
                let dead: ArrayVec<u8, 6> = (0..b.sides[side].mons.len()).filter(|&j| b.sides[side].mons[j].fainted).map(|j| j as u8).collect();
                if !dead.is_empty() {
                    out[k] = Action::Switch { to: dead[rng.below(dead.len() as u32) as usize] };
                }
            } else if b.switch_needed[side][k] {
                let bench = bench_list(b, side, &taken);
                if !bench.is_empty() {
                    let to = bench[rng.below(bench.len() as u32) as usize];
                    taken.push(to);
                    out[k] = Action::Switch { to };
                }
            }
        }
        return out;
    }
    let mut mega_done = false;
    for k in 0..2 {
        let Some(i) = b.active(side, k) else { continue };
        let bench = bench_list(b, side, &taken);
        if !bench.is_empty() && b.mon(side, i).charge_slot == crate::state::NONE && !b.trapped(dex, side, i) && rng.below(10) == 0 {
            let to = bench[rng.below(bench.len() as u32) as usize];
            taken.push(to);
            out[k] = Action::Switch { to };
            continue;
        }
        let usable = b.usable_moves(dex, side, i);
        let mega = !mega_done && b.can_mega(side, i);
        if mega {
            mega_done = true;
        }
        if usable.is_empty() {
            out[k] = Action::Move { slot: 4, target: 0, mega };
            continue;
        }
        let slot = usable[rng.below(usable.len() as u32) as usize];
        let mv = &dex.moves[b.mon(side, i).moves[slot as usize] as usize];
        let target = match mv.target {
            Target::Normal => {
                let foes = foe_locs(b, side);
                if foes.is_empty() { 1 } else { foes[rng.below(foes.len() as u32) as usize] }
            }
            Target::AdjacentAlly => -((1 - k) as i8 + 1),
            Target::AdjacentAllyOrSelf => {
                if b.active(side, 1 - k).is_some() && rng.below(2) == 0 { -((1 - k) as i8 + 1) } else { -(k as i8 + 1) }
            }
            _ => 0,
        };
        out[k] = Action::Move { slot, target, mega };
    }
    out
}

/// Play the battle to the end (or `max_turns` more turns) with the random policy for both sides.
/// Returns the winner (0, 1, 2 = tie) or None if unfinished.
pub fn random_playout(b: &mut Battle, dex: &Dex, rng: &mut Rng, max_turns: u16) -> Option<u8> {
    let stop = b.turn.saturating_add(max_turns);
    let mut guard = 0;
    while b.phase != Phase::Ended && b.turn < stop && guard < 5000 {
        guard += 1;
        let mut any = false;
        for side in 0..2 {
            if b.needs_choice(side) {
                any = true;
                let acts = random_actions(b, dex, side, rng);
                b.choose(dex, side, acts);
            }
        }
        if !any {
            break;
        }
    }
    b.winner
}

/// How good a position is for side 0: for every Pokemon, alive ? (hp fraction + ko_bonus) : 0,
/// side 0 minus side 1, plus `position[i]` for side 0's Pokemon that are active and alive.
pub fn evaluate(b: &Battle, ko_bonus: f64, position: &[f64]) -> f64 {
    evaluate_weighted(b, ko_bonus, position, 1.0)
}

/// `self_weight`: how much side 0's own HP and Pokemon count, relative to the opponent's.
pub fn evaluate_weighted(b: &Battle, ko_bonus: f64, position: &[f64], self_weight: f64) -> f64 {
    let mut v = 0.0;
    for s in 0..2 {
        let sign = if s == 0 { self_weight } else { -1.0 };
        for (i, m) in b.sides[s].mons.iter().enumerate() {
            if m.alive() {
                v += sign * (m.hp as f64 / m.maxhp as f64 + ko_bonus);
                if s == 0 && b.sides[0].active.contains(&(i as u8)) {
                    v += position.get(i).copied().unwrap_or(0.0);
                }
            }
        }
    }
    v
}

/// Play one turn from `base` with the given choices for both sides, resolving any mid-turn
/// switch with the random policy, and stop at the start of the next decision.
/// chance enumeration (search): a mid-turn replacement between two bench Pokemon is an enumerated 50/50 branch
fn chance_switches(b: &mut Battle, side: usize, rng: &mut Rng) -> [Action; 2] {
    let mut out = [Action::Pass; 2];
    let mut taken: ArrayVec<u8, 2> = ArrayVec::new();
    for k in 0..2 {
        // (Revival Blessing's pick is a decision, not chance: pass = Showdown's default, the first fainted in party order)
        if b.switch_needed[side][k] && !b.reviving[side][k] {
            let bench = bench_list(b, side, &taken);
            if !bench.is_empty() {
                let to = if bench.len() == 2 { bench[b.pick(2)] } else { bench[rng.below(bench.len() as u32) as usize] };
                taken.push(to);
                out[k] = Action::Switch { to };
            }
        }
    }
    out
}

/// (2026-10-22) every joint replacement choice of `side` at a switch request: each needing slot gets a different bench
/// Pokemon while there are enough, the rest pass ([Pass, Pass] when the side has nothing to choose)
pub fn replacement_joints(b: &Battle, dex: &Dex, side: usize) -> Vec<[Action; 2]> {
    if !b.needs_choice(side) {
        return vec![[Action::Pass; 2]];
    }
    let la = legal_actions(b, dex, side);
    let need = (0..2).filter(|&k| b.switch_needed[side][k]).count();
    let want = need.min(bench_list(b, side, &[]).len());
    let mut opts: [Vec<Action>; 2] = [la[0].to_vec(), la[1].to_vec()];
    for k in 0..2 {
        if b.switch_needed[side][k] && !opts[k].contains(&Action::Pass) {
            opts[k].push(Action::Pass);
        }
    }
    let mut out = Vec::new();
    for &a0 in &opts[0] {
        for &a1 in &opts[1] {
            let n_sw = [a0, a1].iter().filter(|a| matches!(a, Action::Switch { .. })).count();
            if n_sw != want {
                continue;
            }
            if let (Action::Switch { to: x }, Action::Switch { to: y }) = (a0, a1) {
                if x == y {
                    continue;
                }
            }
            out.push([a0, a1]);
        }
    }
    if out.is_empty() {
        out.push([la[0][0], la[1][0]]);
    }
    out
}

pub fn play_turn(b: &mut Battle, dex: &Dex, ours: [Action; 2], theirs: [Action; 2], rng: &mut Rng) {
    let start_turn = b.turn;
    b.choose(dex, 0, ours);
    b.choose(dex, 1, theirs);
    // mid-turn switches (U-turn, Eject Button...) and anything else before the turn ends
    let mut guard = 0;
    while b.phase == Phase::Switch && b.turn == start_turn && guard < 8 {
        guard += 1;
        if b.chance.as_ref().is_some_and(|c| c.stop_repl) && !b.is_midturn()
            && !(0..2).any(|s| b.reviving[s][0] || b.reviving[s][1]) {
            break;                       // end-of-turn replacements: left to the caller (replacement_joints)
        }
        for side in 0..2 {
            if b.needs_choice(side) {
                let acts = if b.chance.is_some() { chance_switches(b, side, rng) } else { random_actions(b, dex, side, rng) };
                b.choose(dex, side, acts);
            }
        }
    }
}

/// The value (for side 0) of each pair (our joint action, their joint action), averaged over
/// `samples` random seeds. Returns values[our index][their index].
pub fn payoff_matrix(
    base: &Battle, dex: &Dex, ours: &[[Action; 2]], theirs: &[[Action; 2]], samples: u32, seed: u64,
    ko_bonus: f64, position: &[f64], self_weight: f64,
) -> Vec<Vec<f64>> {
    let mut rng = Rng::new(seed);
    let mut out = vec![vec![0.0; theirs.len()]; ours.len()];
    for (oi, o) in ours.iter().enumerate() {
        for (ti, t) in theirs.iter().enumerate() {
            let mut total = 0.0;
            for k in 0..samples {
                let mut b = base.clone();
                // common random numbers: every pair uses the same seeds, so comparisons between our
                // actions are not decided by luck
                b.rng = Rng::new(seed.wrapping_add((k as u64).wrapping_mul(0x9E3779B97F4A7C15)));
                rng = Rng::new(seed ^ 0x5555 ^ k as u64);
                play_turn(&mut b, dex, *o, *t, &mut rng);
                total += evaluate_weighted(&b, ko_bonus, position, self_weight);
            }
            out[oi][ti] = total / samples.max(1) as f64;
        }
    }
    out
}
