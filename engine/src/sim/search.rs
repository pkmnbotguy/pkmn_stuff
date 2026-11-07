//! Using the exact engine for search: build a position in the middle of a battle (what the bot
//! knows), set its details, and score (our joint action, their joint action) pairs by playing one
//! turn from it.

use super::battle::Battle;
use super::data::*;
use super::policy::XorShift;
use super::prng::Prng;
use super::state::*;
use super::val::{Mon, Val};

/// An action for one active slot: ("move", slot, target, mega), ("switch", team index), ("pass").
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SlotAction {
    Move { slot: u8, target: i8, mega: bool },
    Switch { to: u8 },
    Pass,
}

impl Battle {
    /// A battle already in progress: each side's brought Pokemon (leads first), the first two
    /// active, no switch-in effects. Set HP, status, field... afterwards, then `refresh()`.
    pub fn from_brought(d: &'static Data, brought: [&[SetData]; 2], seed: [u16; 4], turn: i32) -> Result<Battle, String> {
        let mut b = Battle::new_opts(d, seed, brought, ["A", "B"], false, false)?;
        b.queue.clear();
        b.request_state = RequestState::None;
        for s in 0..2u8 {
            let n = b.sides[s as usize].mons.len();
            b.sides[s as usize].pokemon = (0..n as u8).collect();
            b.sides[s as usize].pokemon_left = n as i32;
            b.sides[s as usize].request = RequestState::None;
            for k in 0..n.min(2) {
                b.activate(Mon { s, i: k as u8 }, k as u8);
            }
        }
        b.turn = turn.max(1);
        b.mid_turn = false;
        b.started = true;
        b.update_all_speed();
        b.make_request(RequestState::Move);
        Ok(b)
    }

    /// put a Pokemon into an active slot without any switch-in effects
    fn activate(&mut self, m: Mon, pos: u8) {
        let s = m.s as usize;
        // keep side.pokemon positions consistent: swap m into `pos`
        let cur = self.sides[s].pokemon.iter().position(|&i| i == m.i).unwrap();
        let other = self.sides[s].pokemon[pos as usize];
        self.sides[s].pokemon.swap(cur, pos as usize);
        if other != m.i {
            self.pm(Mon { s: m.s, i: other }).position = cur as u8;
        }
        self.pm(m).position = pos;
        self.sides[s].active[pos as usize] = Some(m.i);
        let p = self.pm(m);
        p.is_active = true;
        p.is_started = true;
        p.active_turns = 1;
        p.active_move_actions = 1;
        p.newly_switched = false;
        let ab = self.p(m).ability;
        let mut st = self.init_state(ab, Holder::Mon(m), None);
        Self::prime_ability_state(self.d, ab, &mut st);
        self.pm(m).ability_state = st;
        let it = self.p(m).item;
        let st = self.init_state(it, Holder::Mon(m), None);
        self.pm(m).item_state = st;
    }

    /// An ability that has been on the field since before this position: the state its onStart
    /// would have left (Unnerve's flag).
    fn prime_ability_state(d: &Data, ab: EffId, st: &mut EffectState) {
        if matches!(d.eff(ab).id.as_str(), "unnerve" | "asoneglastrier" | "asonespectrier") {
            st.set(K::Unnerved, Val::True);
        }
    }

    /// Who is in an active slot (None = nobody / fainted).
    pub fn set_active(&mut self, s: usize, slot: usize, idx: Option<usize>) {
        if let Some(old) = self.active_at(s, slot) {
            let p = self.pm(old);
            p.is_active = false;
            p.is_started = false;
        }
        match idx {
            Some(i) if i < self.sides[s].mons.len() => {
                let m = Mon { s: s as u8, i: i as u8 };
                if self.p(m).is_active && self.p(m).position as usize != slot {
                    let other = self.p(m).position as usize;
                    self.sides[s].active[other] = None;
                }
                self.activate(m, slot as u8);
            }
            _ => self.sides[s].active[slot] = None,
        }
    }

    /// Set what is known about a Pokemon. `boosts`: atk, def, spa, spd, spe, accuracy, evasion.
    #[allow(clippy::too_many_arguments)]
    pub fn set_mon_state(&mut self, m: Mon, hp: Option<i32>, status: Option<&str>, boosts: Option<[i8; 7]>, item: Option<&str>,
        fainted: Option<bool>, mega: Option<bool>, first_turn: Option<bool>, protect_streak: Option<u8>) {
        if fainted == Some(true) {
            if !self.p(m).fainted {
                let s = m.s as usize;
                self.sides[s].pokemon_left = (self.sides[s].pokemon_left - 1).max(0);
                self.sides[s].total_fainted += 1;     // Last Respects / Supreme Overlord, as in a real game
            }
            let p = self.pm(m);
            p.hp = 0;
            p.fainted = true;
            p.is_active = false;
            return;
        }
        if mega == Some(true) {
            if let Some(sp) = self.p(m).can_mega_evo {
                self.set_species(m, sp, EffRef::NONE, false);
                self.pm(m).base_species = sp;
                let ab_name = self.d.species[sp as usize].abilities.iter().find(|(k, _)| k == "0").map(|x| x.1.clone()).unwrap_or_default();
                let ab = self.d.ability(&ab_name);
                let p = self.pm(m);
                p.ability = ab;
                p.base_ability = ab;
                if p.is_active {
                    let mut st = self.init_state(ab, Holder::Mon(m), None);
                    Self::prime_ability_state(self.d, ab, &mut st);
                    self.pm(m).ability_state = st;
                }
                let s = m.s as usize;
                for k in 0..self.sides[s].mons.len() {
                    self.sides[s].mons[k].can_mega_evo = None;
                }
            }
        }
        if let Some(h) = hp {
            let mx = self.p(m).maxhp;
            self.pm(m).hp = h.clamp(1, mx);
        }
        if let Some(st) = status {
            let id = match st {
                "" => self.d.c.empty,
                x => self.d.cond(x).unwrap_or(self.d.c.empty),
            };
            let mut state = self.init_state(id, Holder::Mon(m), None);
            if id == self.d.c.slp {
                state.set(K::StartTime, Val::Num(2.0));
                state.set(K::Time, Val::Num(2.0));
            } else if id == self.d.c.frz {
                state.set(K::StartTime, Val::Num(3.0));
                state.set(K::Time, Val::Num(3.0));
            } else if id == self.d.c.tox {
                state.set(K::Stage, Val::Num(0.0));
            }
            let p = self.pm(m);
            p.status = id;
            p.status_state = state;
        }
        if let Some(bo) = boosts {
            self.pm(m).boosts = bo;
        }
        if let Some(it) = item {
            let e = self.d.item(it);
            self.pm(m).item = e;
            let st = self.init_state(e, Holder::Mon(m), None);
            self.pm(m).item_state = st;
        }
        if let Some(ft) = first_turn {
            self.pm(m).active_move_actions = if ft { 0 } else { 1 };
        }
        if let Some(n) = protect_streak {
            let stall = self.d.c.stall;
            self.pm(m).volatiles.retain(|x| x.0 != stall);
            if n > 0 {
                let mut st = self.init_state(stall, Holder::Mon(m), None);
                st.duration = Some(2.0);
                st.set(K::Counter, Val::Num(3f64.powi(n as i32).min(729.0)));
                self.pm(m).volatiles.push((stall, st));
            }
        }
    }

    pub fn set_stats(&mut self, m: Mon, stats: [i32; 6]) {
        let p = self.pm(m);
        if stats[0] > 0 {
            let lost = p.maxhp - p.hp;
            p.maxhp = stats[0];
            p.base_maxhp = stats[0];
            p.hp = (stats[0] - lost).max(if p.hp > 0 { 1 } else { 0 });
        }
        for k in 1..6 {
            if stats[k] > 0 {
                p.stored_stats[k] = stats[k];
                p.base_stored_stats[k] = stats[k];
            }
        }
    }

    /// Field: weather / terrain ids ("raindance", "grassyterrain"...; "" = none), turns left.
    pub fn set_field_state(&mut self, weather: Option<(&str, i32)>, terrain: Option<(&str, i32)>, trick_room: Option<i32>) {
        if let Some((w, turns)) = weather {
            let id = if w.is_empty() { self.d.c.empty } else { self.d.cond(w).unwrap_or(self.d.c.empty) };
            let mut st = self.init_state(id, Holder::None, None);
            if id != self.d.c.empty {
                st.duration = Some(turns.max(1) as f64);
            }
            self.field.weather = id;
            self.field.weather_state = st;
        }
        if let Some((t, turns)) = terrain {
            let id = if t.is_empty() { self.d.c.empty } else { self.d.cond(t).unwrap_or(self.d.c.empty) };
            let mut st = self.init_state(id, Holder::None, None);
            if id != self.d.c.empty {
                st.duration = Some(turns.max(1) as f64);
            }
            self.field.terrain = id;
            self.field.terrain_state = st;
        }
        if let Some(turns) = trick_room {
            let tr = self.d.c.trickroom;
            self.field.pseudo_weather.retain(|x| x.0 != tr);
            if turns > 0 {
                let mut st = self.init_state(tr, Holder::None, None);
                st.duration = Some(turns as f64);
                self.field.pseudo_weather.push((tr, st));
            }
        }
        self.update_all_speed();
    }

    /// A side condition with `turns` left (0 = remove).
    pub fn set_side_condition(&mut self, s: usize, id: &str, turns: i32) {
        let Some(e) = self.d.cond(id) else { return };
        self.sides[s].side_conditions.retain(|x| x.0 != e);
        if turns > 0 {
            let mut st = self.init_state(e, Holder::Side(s as u8), None);
            st.duration = Some(turns as f64);
            st.source = self.active_at(s, 0);
            self.sides[s].side_conditions.push((e, st));
        }
    }
    pub fn set_mega_used(&mut self, s: usize, used: bool) {
        if used {
            for k in 0..self.sides[s].mons.len() {
                self.sides[s].mons[k].can_mega_evo = None;
            }
        }
    }

    /// After editing a position: check the winner, update speeds, and ask for moves (or for
    /// replacements where an active slot is empty).
    pub fn refresh(&mut self) {
        self.update_all_speed();
        if self.check_win(None) {
            return;
        }
        let mut need = false;
        for s in 0..2 {
            for pos in 0..2 {
                let empty = match self.active_at(s, pos) {
                    None => true,
                    Some(m) => self.p(m).fainted,
                };
                if empty && self.can_switch(s) > 0 {
                    // a slot to refill: mark it like a fainted active
                    if let Some(m) = self.active_at(s, pos) {
                        self.pm(m).switch_flag = SwitchFlag::Yes;
                        need = true;
                    }
                }
            }
        }
        self.make_request(if need { RequestState::Switch } else { RequestState::Move });
    }

    /// The choice string for one side from slot actions; targets are fixed up so the choice is
    /// legal where possible (a spread move gets no target, a single-target move a default one).
    pub fn choice_string(&mut self, s: usize, acts: [SlotAction; 2]) -> String {
        let mut parts = vec![];
        let mut mega_done = false;
        for (k, act) in acts.iter().enumerate() {
            let Some(m) = self.active_at(s, k) else {
                parts.push("pass".to_string());
                continue;
            };
            if self.p(m).fainted && !matches!(act, SlotAction::Switch { .. }) {
                parts.push("pass".to_string());
                continue;
            }
            match *act {
                SlotAction::Pass => parts.push("pass".to_string()),
                SlotAction::Switch { to } => {
                    let pos = self.sides[s].pokemon.iter().position(|&i| i == to).unwrap_or(0);
                    parts.push(format!("switch {}", pos + 1));
                }
                SlotAction::Move { slot, target, mega } => {
                    let req = self.slot_request(m);
                    if req.locked {
                        parts.push("move 1".to_string());
                        continue;
                    }
                    let mid = self.p(m).move_slots.get(slot as usize).map(|x| x.id);
                    let idx = mid.and_then(|id| req.moves.iter().position(|r| r.id == id && !r.disabled))
                        .or_else(|| req.moves.iter().position(|r| !r.disabled))
                        .unwrap_or(0);
                    let tt = req.moves.get(idx).map(|r| r.target).unwrap_or(MoveTarget::Normal);
                    let mut t = String::new();
                    if tt.choosable() {
                        let mut tl = target;
                        if tl == 0 || !self.valid_target_loc(tl, m, tt) {
                            tl = match tt {
                                MoveTarget::AdjacentAlly | MoveTarget::AdjacentAllyOrSelf => {
                                    if self.active_at(s, 1 - k).map(|x| !self.p(x).fainted).unwrap_or(false) { -((1 - k) as i8 + 1) } else { -(k as i8 + 1) }
                                }
                                _ => {
                                    let f = (0..2).find(|&f| self.active_at(1 - s, f).map(|x| !self.p(x).fainted).unwrap_or(false)).unwrap_or(0);
                                    f as i8 + 1
                                }
                            };
                        }
                        t = format!(" {tl}");
                    }
                    let use_mega = mega && req.can_mega && !mega_done;
                    if use_mega {
                        mega_done = true;
                    }
                    parts.push(format!("move {}{}{}", idx + 1, t, if use_mega { " mega" } else { "" }));
                }
            }
        }
        parts.join(", ")
    }

    /// Choose for one side; an illegal choice falls back to Showdown's default.
    pub fn choose_actions(&mut self, s: usize, acts: [SlotAction; 2]) {
        if self.sides[s].request == RequestState::None {
            return;
        }
        let ch = self.choice_string(s, acts);
        if !self.choose(s, &ch) {
            self.choose(s, "default");
        }
    }

    /// Run until the next turn starts (or the battle ends), answering any switch request with
    /// the default choice.
    pub fn finish_turn(&mut self, start_turn: i32) {
        let mut guard = 0;
        while !self.ended && self.turn == start_turn && guard < 16 {
            guard += 1;
            let mut any = false;
            for s in 0..2 {
                if self.sides[s].request != RequestState::None && !self.is_choice_done_pub(s) {
                    any = true;
                    self.choose(s, "default");
                }
            }
            if !any {
                break;
            }
        }
    }

    /// side 0's view: for every Pokemon, alive ? (hp fraction + ko_bonus) : 0, ours (x self_weight)
    /// minus theirs, plus position[i] for our active Pokemon.
    pub fn evaluate(&self, ko_bonus: f64, position: &[f64], self_weight: f64) -> f64 {
        let mut v = 0.0;
        for s in 0..2 {
            let sign = if s == 0 { self_weight } else { -1.0 };
            for (i, p) in self.sides[s].mons.iter().enumerate() {
                if p.hp > 0 && !p.fainted {
                    v += sign * (p.hp as f64 / p.maxhp as f64 + ko_bonus);
                    if s == 0 && p.is_active {
                        v += position.get(i).copied().unwrap_or(0.0);
                    }
                }
            }
        }
        v
    }

    /// values[our index][their index], averaged over `samples` seeds (common random numbers:
    /// every pair uses the same seeds).
    #[allow(clippy::too_many_arguments)]
    pub fn payoff_matrix(&self, ours: &[[SlotAction; 2]], theirs: &[[SlotAction; 2]], samples: u32, seed: u64,
        ko_bonus: f64, position: &[f64], self_weight: f64) -> Vec<Vec<f64>> {
        let mut out = vec![vec![0.0; theirs.len()]; ours.len()];
        for (oi, o) in ours.iter().enumerate() {
            for (ti, t) in theirs.iter().enumerate() {
                let mut total = 0.0;
                for k in 0..samples.max(1) {
                    let mut b = self.clone();
                    b.log = None;
                    b.prng = Prng::from_u64(seed.wrapping_add((k as u64).wrapping_mul(0x9E3779B97F4A7C15)));
                    let start = b.turn;
                    b.choose_actions(0, *o);
                    b.choose_actions(1, *t);
                    b.finish_turn(start);
                    total += b.evaluate(ko_bonus, position, self_weight);
                }
                out[oi][ti] = total / samples.max(1) as f64;
            }
        }
        out
    }

    /// Random playouts to the end (or max_turns more turns): (side 0 wins, side 1 wins, ties, unfinished).
    pub fn playouts(&self, n: u32, seed: u64, max_turns: i32) -> (u32, u32, u32, u32) {
        let mut res = (0, 0, 0, 0);
        for k in 0..n {
            let mut b = self.clone();
            b.log = None;
            b.prng = Prng::from_u64(seed.wrapping_add((k as u64).wrapping_mul(0x9E3779B97F4A7C15)));
            let mut rng = XorShift::new((seed as u32) ^ k.wrapping_mul(2654435761) ^ 0x1234);
            let stop = b.turn + max_turns;
            let mut guard = 0;
            while !b.ended && b.turn < stop && guard < 2000 {
                guard += 1;
                let mut any = false;
                for s in 0..2 {
                    if let Some(ch) = b.random_choice(s, &mut rng) {
                        if b.is_choice_done_pub(s) {
                            continue;
                        }
                        any = true;
                        if !b.choose(s, &ch) {
                            b.choose(s, "default");
                        }
                    }
                }
                if !any {
                    break;
                }
            }
            match b.winner {
                Some(Some(0)) => res.0 += 1,
                Some(Some(_)) => res.1 += 1,
                Some(None) => res.2 += 1,
                None => res.3 += 1,
            }
        }
        res
    }
}

