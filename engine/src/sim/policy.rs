//! Requests as a player sees them, and a random player (the same policy as
//! tools/trace_showdown.js), for tests, benchmarks and rollouts on the exact engine.

use super::battle::Battle;
use super::data::MoveTarget;
use super::state::*;
use super::val::Mon;

/// xorshift32 with the same output as trace_showdown.js's makeRng
#[derive(Clone, Debug)]
pub struct XorShift(pub u32);
impl XorShift {
    pub fn new(seed: u32) -> XorShift {
        XorShift(if seed == 0 { 1 } else { seed })
    }
    pub fn below(&mut self, n: usize) -> usize {
        let mut s = self.0;
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        self.0 = s;
        ((s as f64 / 4294967296.0) * n as f64).floor() as usize
    }
}

/// One usable move as listed in a move request.
#[derive(Clone, Debug)]
pub struct ReqMove {
    pub id: u16,
    pub target: MoveTarget,
    pub disabled: bool,
}

/// The move request of one active slot.
#[derive(Clone, Debug)]
pub struct SlotRequest {
    pub moves: Vec<ReqMove>,
    pub trapped: bool,
    pub can_mega: bool,
    /// a locked move (two-turn move, recharge...): the only choice
    pub locked: bool,
}

impl Battle {
    /// What a player sees for one active slot in a move request.
    pub fn slot_request(&mut self, m: Mon) -> SlotRequest {
        let locked = self.get_locked_move(m);
        if let Some(l) = locked {
            return SlotRequest {
                moves: vec![ReqMove { id: l, target: MoveTarget::Undefined, disabled: false }],
                trapped: true,
                can_mega: false,
                locked: true,
            };
        }
        let list = self.get_moves_list(m, None);
        let mut moves: Vec<ReqMove> = vec![];
        if list.is_empty() {
            moves.push(ReqMove { id: self.d.c.struggle_move, target: MoveTarget::RandomNormal, disabled: false });
        } else {
            for (id, disabled) in list {
                let mut t = self.p(m).move_slots.iter().find(|s| s.id == id).map(|s| s.target).unwrap_or(MoveTarget::Normal);
                if self.d.moves[id as usize].id == "curse" && !self.has_type(m, "Ghost") {
                    t = MoveTarget::SelfTarget;
                }
                moves.push(ReqMove { id, target: t, disabled });
            }
        }
        SlotRequest { moves, trapped: self.p(m).trapped != 0, can_mega: self.p(m).can_mega_evo.is_some(), locked: false }
    }

    /// A random legal choice string for side `s` (None = nothing to choose), with the same
    /// policy and random stream as tools/trace_showdown.js.
    pub fn random_choice(&mut self, s: usize, rng: &mut XorShift) -> Option<String> {
        let req = self.sides[s].request;
        match req {
            RequestState::None => None,
            RequestState::TeamPreview => {
                let mut idx = [0usize, 1, 2, 3, 4, 5];
                for i in (1..6).rev() {
                    let j = rng.below(i + 1);
                    idx.swap(i, j);
                }
                Some(format!("team {}", idx[..4].iter().map(|i| (i + 1).to_string()).collect::<String>()))
            }
            RequestState::Switch => {
                let mut taken: Vec<usize> = vec![];
                let mut parts = vec![];
                for k in 0..2 {
                    if !self.sides[s].force_switch[k] {
                        parts.push("pass".to_string());
                        continue;
                    }
                    let bench = self.bench_free(s, &taken);
                    if bench.is_empty() {
                        parts.push("pass".to_string());
                        continue;
                    }
                    let to = bench[rng.below(bench.len())];
                    taken.push(to);
                    parts.push(format!("switch {}", to + 1));
                }
                Some(parts.join(", "))
            }
            RequestState::Move => {
                let mut taken: Vec<usize> = vec![];
                let mut parts = vec![];
                let mut mega_done = false;
                for k in 0..2 {
                    let Some(m) = self.active_at(s, k) else {
                        parts.push("pass".to_string());
                        continue;
                    };
                    if self.p(m).fainted {
                        parts.push("pass".to_string());
                        continue;
                    }
                    let req = self.slot_request(m);
                    let bench = self.bench_free(s, &taken);
                    if !bench.is_empty() && !req.trapped && rng.below(10) == 0 {
                        let to = bench[rng.below(bench.len())];
                        taken.push(to);
                        parts.push(format!("switch {}", to + 1));
                        continue;
                    }
                    let usable: Vec<(usize, &ReqMove)> = req.moves.iter().enumerate().filter(|(_, x)| !x.disabled).collect();
                    let mega = !mega_done && req.can_mega && !req.locked;
                    if mega {
                        mega_done = true;
                    }
                    if usable.is_empty() {
                        parts.push(format!("move 1{}", if mega { " mega" } else { "" }));
                        continue;
                    }
                    let (j, pick) = usable[rng.below(usable.len())];
                    let mut target = String::new();
                    match pick.target {
                        MoveTarget::Normal | MoveTarget::Any | MoveTarget::AdjacentFoe => {
                            let foes: Vec<usize> = (0..2).filter(|&f| self.active_at(1 - s, f).map(|x| !self.p(x).fainted).unwrap_or(false)).map(|f| f + 1).collect();
                            let t = if foes.is_empty() { 1 } else { foes[rng.below(foes.len())] };
                            target = format!(" {t}");
                        }
                        MoveTarget::AdjacentAlly | MoveTarget::AdjacentAllyOrSelf => {
                            let ally_ok = self.active_at(s, 1 - k).map(|x| !self.p(x).fainted).unwrap_or(false);
                            let t: i32 = if ally_ok { -((1 - k) as i32 + 1) } else { -(k as i32 + 1) };
                            target = format!(" {t}");
                        }
                        _ => {}
                    }
                    parts.push(format!("move {}{}{}", j + 1, target, if mega { " mega" } else { "" }));
                }
                Some(parts.join(", "))
            }
        }
    }

    /// positions (in side.pokemon) of bench Pokemon that can come in
    fn bench_free(&self, s: usize, taken: &[usize]) -> Vec<usize> {
        let side = &self.sides[s];
        (2..side.pokemon.len()).filter(|&i| !self.p(Mon { s: s as u8, i: side.pokemon[i] }).fainted && !taken.contains(&i)).collect()
    }
}
