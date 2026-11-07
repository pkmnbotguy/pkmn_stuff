//! Using moves (`sim/battle-actions.ts` with the Champions mod's overrides): runMove, useMove,
//! the hit steps, spreadMoveHit, damage calculation, move effects, secondaries.

use super::battle::*;
use super::data::*;
use super::events::Ev;
use super::state::*;
use super::val::{combine_results, Mon, Val};
use crate::add;

/// A target slot during a move: a Pokemon, or `null` (hit a substitute) / `false` (failed).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TS {
    M(Mon),
    Null,
    False,
}
impl TS {
    pub fn mon(self) -> Option<Mon> {
        if let TS::M(m) = self { Some(m) } else { None }
    }
}

/// Which hit effect spreadMoveHit / moveHit applies: the move itself, a secondary, `self`, or `selfBoost`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum HitFx {
    Move,
    Secondary(u8),
    SelfOf,
    /// a secondary's own `self` (e.g. a self boost with a chance)
    SecondarySelf(u8),
    SelfBoost,
}

impl Battle {
    fn mref(&self, mv: u16) -> EffRef {
        EffRef { eff: self.mv(mv).eff, mv, part: 0 }
    }
    fn move_name(&self, mv: u16) -> String {
        self.move_def(mv).name.clone()
    }

    // ------------------------------------------------------------------
    // runMove / useMove
    // ------------------------------------------------------------------
    pub fn run_move(&mut self, mv: u16, m: Mon, target_loc: i8, source_effect: Option<EffId>, external: bool, original_target: Option<Mon>) {
        self.pm(m).active_move_actions += 1;
        let mut target = self.get_target(m, mv, target_loc, original_target);
        let mut base = mv;
        let priority = self.mv(base).priority;
        let prankster = self.mv(base).prankster_boosted;
        if self.move_def(base).id != "struggle" && !external {
            let r = self.run_event(Ev::OverrideAction, Arg::Mon(m), target, self.mref(base), Val::Undef);
            let changed = match r {
                Val::MoveId(x) => Some(x),
                Val::Eff(e) if self.eff(e).move_def != u16::MAX => Some(self.eff(e).move_def),
                _ => None,
            };
            if let Some(nm) = changed {
                base = self.new_active_move(nm);
                self.mvm(base).priority = priority;
                if prankster {
                    self.mvm(base).prankster_boosted = true;
                }
                target = self.get_random_target(m, base);
            }
        }
        let mv = base;
        self.mvm(mv).is_external = external;
        self.set_active_move(Some(mv), Some(m), target);
        let will_try = self.run_event(Ev::BeforeMove, Arg::Mon(m), target, self.mref(mv), Val::Undef);
        if !will_try.truthy() {
            self.run_event(Ev::MoveAborted, Arg::Mon(m), target, self.mref(mv), Val::Undef);
            self.clear_active_move(true);
            self.pm(m).move_this_turn_result = will_try;
            return;
        }
        let mid = self.mv(mv).def;
        if self.mv(mv).flags & mflag::CANTUSETWICE != 0 && self.p(m).last_move == Some(mid) {
            if let Some(c) = self.d.cond(&self.d.moves[mid as usize].id) {
                self.add_volatile(m, c, None, EffRef::NONE, None);
            }
        }
        if self.eff(self.mv(mv).eff).has(self.d.cb_id("beforeMoveCallback")) {
            if super::handlers::before_move_callback(self, m, target, mv) {
                self.clear_active_move(true);
                self.pm(m).move_this_turn_result = Val::False;
                return;
            }
        }
        self.pm(m).last_damage = 0;
        let mut source_effect = source_effect;
        if !external {
            let locked = self.get_locked_move(m);
            if locked.is_none() {
                let base_def = self.mv(base).def;
                if self.deduct_pp(m, base_def, None) == 0 && self.move_def(mv).id != "struggle" {
                    let n = self.move_name(mv);
                    add!(self, "cant", m, "nopp", n);
                    self.clear_active_move(true);
                    self.pm(m).move_this_turn_result = Val::False;
                    return;
                }
            } else {
                source_effect = Some(self.d.c.lockedmove);
            }
            self.move_used(m, mid, Some(target_loc));
        }
        let no_lock = external && !self.has_volatile(m, self.d.c.lockedmove);
        let did = self.use_move(base, m, target, source_effect);
        self.last_successful_move_this_turn = if did.truthy() { self.active_move.map(|a| self.mv(a).def) } else { None };
        let mv = self.active_move.unwrap_or(mv);
        self.single_event(Ev::AfterMove, self.mref(mv), StateLoc::None, Arg::Mon(m), Arg::from_opt(target), self.mref(mv), Val::Undef);
        self.run_event(Ev::AfterMove, Arg::Mon(m), target, self.mref(mv), Val::Undef);
        if self.mv(mv).flags & mflag::CANTUSETWICE != 0 {
            if let Some(c) = self.d.cond(&self.move_def(mv).id.clone()) {
                if self.remove_volatile(m, c) {
                    let n = self.move_name(mv);
                    add!(self, "-hint", format!("Some effects can force a Pokemon to use {n} again in a row."));
                }
            }
        }
        // Dancer
        if self.mv(mv).flags & mflag::DANCE != 0 && did.truthy() && !self.mv(mv).is_external {
            let mut dancers: Vec<Mon> = vec![];
            for o in self.all_active() {
                if o == m {
                    continue;
                }
                if self.has_ability(o, "dancer") && !self.is_semi_invulnerable(o) {
                    dancers.push(o);
                }
            }
            let mut keyed: Vec<(Mon, i32, f64)> = dancers.iter().map(|&d| (d, self.p(d).stored_stats[5], self.p(d).ability_state.effect_order)).collect();
            keyed.sort_by(|a, b| {
                let r = -((b.1 - a.1) as f64);
                let r = if r != 0.0 { r } else { b.2 - a.2 };
                r.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal)
            });
            let first_target = self.active_target;
            for (dancer, _, _) in keyed {
                if self.faint_messages(false, false, true) {
                    break;
                }
                if self.p(dancer).fainted {
                    continue;
                }
                add!(self, "-activate", dancer, "ability: Dancer");
                let dt = match first_target {
                    Some(t) if t.s != dancer.s && m.s == dancer.s => t,
                    _ => m,
                };
                let loc = self.get_loc_of(dancer, dt);
                let nm = self.new_active_move(self.mv(mv).def);
                let dancer_eff = self.d.ability("dancer");
                self.run_move(nm, dancer, loc, Some(dancer_eff), true, None);
            }
        }
        if no_lock && self.has_volatile(m, self.d.c.lockedmove) {
            let lm = self.d.c.lockedmove;
            self.pm(m).volatiles.retain(|x| x.0 != lm);
        }
        self.faint_messages(false, false, true);
        self.check_win(None);
    }

    pub fn use_move(&mut self, mv: u16, m: Mon, target: Option<Mon>, source_effect: Option<EffId>) -> Val {
        self.pm(m).move_this_turn_result = Val::Undef;
        let old = self.p(m).move_this_turn_result;
        let r = self.use_move_inner(mv, m, target, source_effect);
        if old == self.p(m).move_this_turn_result {
            self.pm(m).move_this_turn_result = r;
        }
        r
    }

    /// useMove(move, pokemon) without a target (undefined): the target is chosen at random after ModifyTarget
    pub fn use_move_random(&mut self, mv: u16, m: Mon, source_effect: Option<EffId>) -> Val {
        self.use_move_undef_target = true;
        self.use_move(mv, m, None, source_effect)
    }

    fn use_move_inner(&mut self, mv: u16, m: Mon, target: Option<Mon>, source_effect: Option<EffId>) -> Val {
        let undef_target = std::mem::replace(&mut self.use_move_undef_target, false);
        let mut target = target;
        let mut source_effect = source_effect;
        if source_effect.is_none() && !self.effect.is_none() && !self.eff(self.effect.eff).id.is_empty() {
            source_effect = Some(self.effect.eff);
        }
        if let Some(se) = source_effect {
            if matches!(self.eff(se).id.as_str(), "instruct" | "custapberry") {
                source_effect = None;
            }
        }
        let mid = self.mv(mv).def;
        self.pm(m).last_move_used = Some(mid);
        if let Some(am) = self.active_move {
            let pr = self.mv(am).priority;
            let pb = self.mv(am).prankster_boosted;
            self.mvm(mv).priority = pr;
            if !self.mv(mv).has_bounced {
                self.mvm(mv).prankster_boosted = pb;
            }
        }
        let base_target = self.mv(mv).target;
        // ModifyTarget (fast exit)
        let r = self.run_event_full(Ev::ModifyTarget, Arg::Mon(m), Arg::from_opt(target), self.mref(mv), match target {
            Some(t) => Val::Mon(t),
            None => Val::Null,
        }, true, false);
        match r {
            Val::Mon(t) => target = Some(t),
            Val::Null => target = None,
            _ => {}
        }
        // `if (target === undefined) target = this.battle.getRandomTarget(pokemon, move)` (Sleep Talk's random move...)
        if target.is_none() && undef_target {
            target = self.get_random_target(m, mv);
        }
        let mt = self.mv(mv).target;
        if mt == MoveTarget::SelfTarget || mt == MoveTarget::Allies {
            target = Some(m);
        }
        if let Some(se) = source_effect {
            self.mvm(mv).source_effect = Some(se);
            self.mvm(mv).ignore_ability = false;
        }
        let mut move_result = Val::False;
        self.set_active_move(Some(mv), Some(m), target);
        let mr = self.mref(mv);
        self.single_event(Ev::ModifyType, mr, StateLoc::None, Arg::Mon(m), Arg::from_opt(target), mr, Val::Move(mv));
        self.single_event(Ev::ModifyMove, mr, StateLoc::None, Arg::Mon(m), Arg::from_opt(target), mr, Val::Move(mv));
        if base_target != self.mv(mv).target {
            target = self.get_random_target(m, mv);
        }
        self.run_event(Ev::ModifyType, Arg::Mon(m), target, mr, Val::Move(mv));
        self.run_event(Ev::ModifyMove, Arg::Mon(m), target, mr, Val::Move(mv));
        if base_target != self.mv(mv).target {
            target = self.get_random_target(m, mv);
        }
        if self.p(m).fainted {
            return Val::False;
        }
        let mut attrs = String::new();
        let movename = self.move_name(mv);
        if let Some(se) = source_effect {
            attrs.push_str(&format!("|[from] {}", self.eff(se).fullname));
        }
        if self.log.is_some() {
            let ts = match target {
                Some(t) => self.mon_str(t),
                None => "null".to_string(),
            };
            let ms = self.mon_str(m);
            self.add_move(vec!["move".into(), ms, movename, format!("{ts}{attrs}")]);
        }
        let Some(tgt) = target else {
            self.attr_last_move(&["[notarget]"]);
            add!(self, "-fail", m);
            return Val::False;
        };
        let (targets, pressure_targets) = self.get_move_targets(m, mv, tgt);
        let target = if let Some(&t) = targets.last() { t } else { tgt };
        let caller_pp = source_effect.map(|se| self.eff(se).kind == Kind::Move).unwrap_or(false);
        if source_effect.is_none() || caller_pp {
            let mut extra = 0.0;
            for src in pressure_targets {
                let r = self.run_event(Ev::DeductPP, Arg::Mon(src), Some(m), mr, Val::Undef);
                if r != Val::True {
                    extra += if r.truthy() { r.num() } else { 0.0 };
                }
            }
            if extra > 0.0 {
                self.deduct_pp(m, mid, Some(extra as i32));
            }
        }
        let mut try_move = self.single_event(Ev::TryMove, mr, StateLoc::None, Arg::Mon(m), Arg::Mon(target), mr, Val::Undef);
        if try_move.truthy() {
            try_move = self.run_event(Ev::TryMove, Arg::Mon(m), Some(target), mr, Val::Undef);
        }
        if !try_move.truthy() {
            return try_move;
        }
        self.single_event(Ev::UseMoveMessage, mr, StateLoc::None, Arg::Mon(m), Arg::Mon(target), mr, Val::Undef);
        if self.mv(mv).ignore_immunity.is_none() {
            let st = self.mv(mv).category == Category::Status;
            self.mvm(mv).ignore_immunity = Some(if st { IgnoreImmunity::All } else { IgnoreImmunity::No });
        }
        if self.mv(mv).selfdestruct == SelfDestruct::Always {
            self.faint(m, Some(m), mr);
        }
        let mt = self.mv(mv).target;
        if matches!(mt, MoveTarget::All | MoveTarget::FoeSide | MoveTarget::AllySide | MoveTarget::AllyTeam) {
            let dmg = self.try_move_hit(&targets, m, mv);
            if dmg == Val::Empty {
                self.pm(m).move_this_turn_result = Val::Null;
            }
            if dmg.truthy() || matches!(dmg, Val::Num(x) if x == 0.0) || dmg == Val::Undef {
                move_result = Val::True;
            }
        } else {
            if targets.is_empty() {
                self.attr_last_move(&["[notarget]"]);
                add!(self, "-fail", m);
                return Val::False;
            }
            move_result = Val::from_bool(self.try_spread_move_hit(targets.clone(), m, mv, false));
        }
        if self.mv(mv).self_boost.is_some() && move_result.truthy() {
            self.move_hit(&[TS::M(m)], m, mv, HitFx::SelfBoost, false, true);
        }
        if self.p(m).hp == 0 {
            self.faint(m, Some(m), mr);
        }
        if !move_result.truthy() {
            let ohp = self.p(m).hp;
            self.single_event(Ev::MoveFail, mr, StateLoc::None, Arg::Mon(target), Arg::Mon(m), mr, Val::Undef);
            if m != target && self.mv(mv).category != Category::Status {
                let p = self.p(m);
                if p.hp as f64 <= p.maxhp as f64 / 2.0 && ohp as f64 > p.maxhp as f64 / 2.0 {
                    self.run_event(Ev::EmergencyExit, Arg::Mon(m), Some(m), EffRef::NONE, Val::Undef);
                }
            }
            return Val::False;
        }
        let sheer = self.mv(mv).has_sheer_force && self.has_ability(m, "sheerforce");
        if !sheer && self.mv(mv).flags & mflag::FUTUREMOVE == 0 {
            let ohp = self.p(m).hp;
            self.single_event(Ev::AfterMoveSecondarySelf, mr, StateLoc::None, Arg::Mon(m), Arg::Mon(target), mr, Val::Undef);
            self.run_event(Ev::AfterMoveSecondarySelf, Arg::Mon(m), Some(target), mr, Val::Undef);
            if m != target && self.mv(mv).category != Category::Status {
                let p = self.p(m);
                if p.hp as f64 <= p.maxhp as f64 / 2.0 && ohp as f64 > p.maxhp as f64 / 2.0 {
                    self.run_event(Ev::EmergencyExit, Arg::Mon(m), Some(m), EffRef::NONE, Val::Undef);
                }
            }
        }
        Val::True
    }

    /// pokemon.getMoveTargets(move, target): (targets, pressureTargets)
    pub fn get_move_targets(&mut self, m: Mon, mv: u16, target: Mon) -> (Vec<Mon>, Vec<Mon>) {
        let mut targets: Vec<Mon> = vec![];
        let mt = self.mv(mv).target;
        match mt {
            MoveTarget::All | MoveTarget::FoeSide | MoveTarget::AllySide | MoveTarget::AllyTeam => {
                if !matches!(mt, MoveTarget::FoeSide) {
                    targets.extend(self.allies_and_self(m.s));
                }
                if !matches!(mt, MoveTarget::AllySide | MoveTarget::AllyTeam) {
                    targets.extend(self.allies_and_self_all(1 - m.s));
                }
                if !targets.is_empty() && !targets.contains(&target) {
                    let last = *targets.last().unwrap();
                    self.retarget_last_move(last);
                }
            }
            MoveTarget::AllAdjacent | MoveTarget::AllAdjacentFoes => {
                if mt == MoveTarget::AllAdjacent {
                    targets.extend(self.adjacent_allies(m));
                }
                targets.extend(self.adjacent_foes(m));
                if !targets.is_empty() && !targets.contains(&target) {
                    let last = *targets.last().unwrap();
                    self.retarget_last_move(last);
                }
            }
            MoveTarget::Allies => {
                targets = self.allies_and_self(m.s);
            }
            _ => {
                let selected = target;
                let mut target = target;
                if self.p(target).fainted && target.s != m.s {
                    match self.get_random_target(m, mv) {
                        Some(t) => target = t,
                        None => return (vec![], vec![]),
                    }
                }
                if !self.mv(mv).tracks_target {
                    let r = self.priority_event(Ev::RedirectTarget, Arg::Mon(m), Some(m), self.mref(mv), Val::Mon(target));
                    if let Val::Mon(t) = r {
                        target = t;
                    }
                }
                if self.mv(mv).smart_target == Some(true) {
                    targets = self.get_smart_targets(m, target, mv);
                    target = targets[0];
                } else {
                    targets.push(target);
                }
                if self.p(target).fainted && self.mv(mv).flags & mflag::FUTUREMOVE == 0 {
                    return (vec![], vec![]);
                }
                if selected != target {
                    self.retarget_last_move(target);
                }
            }
        }
        let mut pressure = targets.clone();
        if mt == MoveTarget::FoeSide {
            pressure = vec![];
        }
        if self.mv(mv).flags & mflag::MUSTPRESSURE != 0 {
            pressure = self.foes_of(m.s);
        }
        (targets, pressure)
    }

    fn get_smart_targets(&mut self, m: Mon, target: Mon, mv: u16) -> Vec<Mon> {
        let t2 = self.adjacent_allies(target).first().copied();
        match t2 {
            Some(t2) if t2 != m && self.p(t2).hp > 0 => {
                if self.p(target).hp == 0 {
                    self.mvm(mv).smart_target = Some(false);
                    return vec![t2];
                }
                vec![target, t2]
            }
            _ => {
                self.mvm(mv).smart_target = Some(false);
                vec![target]
            }
        }
    }

    // ------------------------------------------------------------------
    // the hit steps
    // ------------------------------------------------------------------
    pub fn try_spread_move_hit(&mut self, targets: Vec<Mon>, m: Mon, mv: u16, not_active: bool) -> bool {
        let mut targets = targets;
        if targets.len() > 1 && self.mv(mv).smart_target != Some(true) {
            self.mvm(mv).spread_hit = true;
        }
        if not_active {
            self.set_active_move(Some(mv), Some(m), targets.first().copied());
        }
        let mr = self.mref(mv);
        let t0 = targets.first().copied();
        let mut hit = self.single_event(Ev::Try, mr, StateLoc::None, Arg::Mon(m), Arg::from_opt(t0), mr, Val::Undef);
        if hit.truthy() {
            hit = self.single_event(Ev::PrepareHit, mr, StateLoc::None, Arg::from_opt(t0), Arg::Mon(m), mr, Val::Undef);
        }
        if hit.truthy() {
            hit = self.run_event(Ev::PrepareHit, Arg::Mon(m), t0, mr, Val::Undef);
        }
        if !hit.truthy() {
            if hit == Val::False {
                add!(self, "-fail", m);
                self.attr_last_move(&["[still]"]);
            }
            return hit == Val::Empty;
        }
        let mut at_least_one_failure = false;
        for step in 0..8 {
            let res: Option<Vec<Val>> = match step {
                0 => Some(self.hit_step_invulnerability(&targets, m, mv)),
                1 => Some(self.hit_step_try_hit(&targets, m, mv)),
                2 => Some(self.hit_step_type_immunity(&targets, m, mv)),
                3 => Some(self.hit_step_try_immunity(&targets, m, mv)),
                4 => Some(self.hit_step_accuracy(&targets, m, mv)),
                5 => {
                    self.hit_step_break_protect(&targets, m, mv);
                    None
                }
                6 => {
                    self.hit_step_steal_boosts(&targets, m, mv);
                    None
                }
                _ => Some(self.hit_step_move_hit_loop(&targets, m, mv)),
            };
            let Some(r) = res else { continue };
            let mut nt = vec![];
            for (i, t) in targets.iter().enumerate() {
                let v = r.get(i).copied().unwrap_or(Val::Undef);
                if v.truthy() || matches!(v, Val::Num(x) if x == 0.0) {
                    nt.push(*t);
                }
            }
            targets = nt;
            at_least_one_failure = at_least_one_failure || r.iter().any(|v| *v == Val::False);
            if self.mv(mv).smart_target == Some(true) && at_least_one_failure {
                self.mvm(mv).smart_target = Some(false);
            }
            if targets.is_empty() {
                break;
            }
        }
        self.mvm(mv).hit_targets = targets.clone();
        let move_result = !targets.is_empty();
        if !move_result && !at_least_one_failure {
            self.pm(m).move_this_turn_result = Val::Null;
        }
        if self.mv(mv).spread_hit && self.log.is_some() {
            let slots: Vec<String> = targets.iter().map(|&t| self.slot_str(t)).collect();
            let a = format!("[spread] {}", slots.join(","));
            self.attr_last_move(&[&a]);
        }
        move_result
    }

    fn hit_step_invulnerability(&mut self, targets: &[Mon], m: Mon, mv: u16) -> Vec<Val> {
        if self.move_def(mv).id == "helpinghand" {
            return vec![Val::True; targets.len()];
        }
        let mut res = vec![];
        let mr = self.mref(mv);
        for &t in targets {
            let r = if self.has_vol(t, "commanding") {
                Val::False
            } else if self.move_def(mv).id == "toxic" && self.has_type(m, "Poison") {
                Val::True
            } else {
                self.run_event(Ev::Invulnerability, Arg::Mon(t), Some(m), mr, Val::Undef)
            };
            if r == Val::False {
                if self.mv(mv).smart_target == Some(true) {
                    self.mvm(mv).smart_target = Some(false);
                } else {
                    if !self.mv(mv).spread_hit {
                        self.attr_last_move(&["[miss]"]);
                    }
                    add!(self, "-miss", m, t);
                }
            }
            res.push(r);
        }
        res
    }

    fn hit_step_try_hit(&mut self, targets: &[Mon], m: Mon, mv: u16) -> Vec<Val> {
        let mr = self.mref(mv);
        let mut r = self.run_event_multi(Ev::TryHit, targets, Some(m), mr, None);
        if !r.iter().any(|v| *v == Val::True) && r.iter().any(|v| *v == Val::False) {
            add!(self, "-fail", m);
            self.attr_last_move(&["[still]"]);
        }
        for v in r.iter_mut() {
            if *v != Val::Empty {
                if !v.truthy() {
                    *v = Val::False;
                }
            }
        }
        r
    }

    fn hit_step_type_immunity(&mut self, targets: &[Mon], _m: Mon, mv: u16) -> Vec<Val> {
        if self.mv(mv).ignore_immunity.is_none() {
            let st = self.mv(mv).category == Category::Status;
            self.mvm(mv).ignore_immunity = Some(if st { IgnoreImmunity::All } else { IgnoreImmunity::No });
        }
        let msg = self.mv(mv).smart_target != Some(true);
        targets.iter().map(|&t| Val::from_bool(self.run_immunity_move(t, mv, msg))).collect()
    }

    fn hit_step_try_immunity(&mut self, targets: &[Mon], m: Mon, mv: u16) -> Vec<Val> {
        let mut res = vec![];
        let mr = self.mref(mv);
        for &t in targets {
            if self.mv(mv).flags & mflag::POWDER != 0 && t != m && !self.dex_immunity("powder", t) {
                add!(self, "-immune", t);
                res.push(Val::False);
            } else if !self.single_event(Ev::TryImmunity, mr, StateLoc::None, Arg::Mon(t), Arg::Mon(m), mr, Val::Undef).truthy() {
                add!(self, "-immune", t);
                res.push(Val::False);
            } else if self.mv(mv).prankster_boosted && self.has_ability(m, "prankster") && t.s != m.s && !self.dex_immunity("prankster", t) {
                let status = self.mv(mv).status;
                let hint = match status {
                    Some(st) => {
                        let sid = self.eff(st).id.clone();
                        self.dex_immunity(&sid, t)
                    }
                    None => true,
                };
                if hint {
                    self.hint("Since gen 7, Dark is immune to Prankster moves.", false);
                }
                add!(self, "-immune", t);
                res.push(Val::False);
            } else {
                res.push(Val::True);
            }
        }
        res
    }

    fn hit_step_accuracy(&mut self, targets: &[Mon], m: Mon, mv: u16) -> Vec<Val> {
        let mut res = vec![];
        let mr = self.mref(mv);
        for &t in targets {
            self.active_target = Some(t);
            let mut accuracy: Option<f64> = self.mv(mv).accuracy;
            let mut immune = false;
            if self.mv(mv).ohko {
                if !self.is_semi_invulnerable(t) {
                    let mut acc = 30.0;
                    // move.ohko === 'Ice' (Sheer Cold, the only one): 20 for a non-Ice user, Ice targets immune
                    let ice = self.move_def(mv).id == "sheercold";
                    if ice && !self.has_type(m, "Ice") {
                        acc = 20.0;
                    }
                    if self.p(m).level >= self.p(t).level && !(ice && self.has_type(t, "Ice")) {
                        acc += (self.p(m).level - self.p(t).level) as f64;
                    } else {
                        add!(self, "-immune", t, "[ohko]");
                        immune = true;
                    }
                    accuracy = Some(acc);
                }
            } else {
                let r = self.run_event(Ev::ModifyAccuracy, Arg::Mon(t), Some(m), mr, acc_val(accuracy));
                accuracy = val_acc(r);
                if let Some(mut acc) = accuracy {
                    let mut boost = 0i32;
                    if !self.mv(mv).ignore_accuracy {
                        let b = self.full_boosts_of(m);
                        if let Val::Boosts(nb) = self.run_event(Ev::ModifyBoost, Arg::Mon(m), None, EffRef::NONE, Val::Boosts(b)) {
                            boost = (nb.get(B_ACC).unwrap_or(0) as i32).clamp(-6, 6);
                        }
                    }
                    if !self.mv(mv).ignore_evasion {
                        let b = self.full_boosts_of(t);
                        if let Val::Boosts(nb) = self.run_event(Ev::ModifyBoost, Arg::Mon(t), None, EffRef::NONE, Val::Boosts(b)) {
                            boost = (boost - nb.get(B_EVA).unwrap_or(0) as i32).clamp(-6, 6);
                        }
                    }
                    if boost > 0 {
                        acc = tr(acc * (3 + boost) as f64 / 3.0);
                    } else if boost < 0 {
                        acc = tr(acc * 3.0 / (3 - boost) as f64);
                    }
                    accuracy = Some(acc);
                }
            }
            if immune {
                res.push(Val::False);
                continue;
            }
            let always = self.mv(mv).always_hit
                || (self.move_def(mv).id == "toxic" && self.has_type(m, "Poison"))
                || (self.mv(mv).target == MoveTarget::SelfTarget && self.mv(mv).category == Category::Status && !self.is_semi_invulnerable(t));
            if always {
                accuracy = None;
            } else {
                let r = self.run_event(Ev::Accuracy, Arg::Mon(t), Some(m), mr, acc_val(accuracy));
                accuracy = val_acc(r);
            }
            if let Some(acc) = accuracy {
                if !((self.random(100) as f64) < acc) {
                    if self.mv(mv).smart_target == Some(true) {
                        self.mvm(mv).smart_target = Some(false);
                    } else {
                        if !self.mv(mv).spread_hit {
                            self.attr_last_move(&["[miss]"]);
                        }
                        add!(self, "-miss", m, t);
                    }
                    if !self.mv(mv).ohko && self.has_item(m, "blunderpolicy") && self.use_item(m, None, EffRef::NONE) {
                        let mut b = SparseBoosts::default();
                        b.set(B_SPE, 2);
                        self.boost(b, Some(m), None, EffRef::NONE, false, false);
                    }
                    res.push(Val::False);
                    continue;
                }
            }
            res.push(Val::True);
        }
        res
    }

    pub fn full_boosts_of(&self, m: Mon) -> SparseBoosts {
        let mut b = SparseBoosts::default();
        for (k, &v) in self.p(m).boosts.iter().enumerate() {
            b.set(k as u8, v);
        }
        b
    }

    fn hit_step_break_protect(&mut self, targets: &[Mon], _m: Mon, mv: u16) {
        if !self.mv(mv).breaks_protect {
            return;
        }
        for &t in targets {
            let mut broke = false;
            for id in ["banefulbunker", "burningbulwark", "kingsshield", "obstruct", "protect", "silktrap", "spikyshield"] {
                if self.remove_volatile_id(t, id) {
                    broke = true;
                }
            }
            for id in ["craftyshield", "matblock", "quickguard", "wideguard"] {
                if let Some(c) = self.d.cond(id) {
                    if self.remove_side_condition(t.s, c) {
                        broke = true;
                    }
                }
            }
            if broke {
                if self.move_def(mv).id == "feint" {
                    add!(self, "-activate", t, "move: Feint");
                } else {
                    let n = self.move_name(mv);
                    add!(self, "-activate", t, format!("move: {n}"), "[broken]");
                }
                let stall = self.d.c.stall;
                self.pm(t).volatiles.retain(|x| x.0 != stall);
            }
        }
    }

    fn hit_step_steal_boosts(&mut self, targets: &[Mon], m: Mon, mv: u16) {
        let Some(&t) = targets.first() else { return };
        if !self.mv(mv).steals_boosts {
            return;
        }
        let mut boosts = SparseBoosts::default();
        let mut stolen = false;
        for k in 0..7u8 {
            let st = self.p(t).boosts[k as usize];
            if st > 0 {
                boosts.set(k, st);
                stolen = true;
            }
        }
        if stolen {
            self.attr_last_move(&["[still]"]);
            let n = self.move_name(mv);
            add!(self, "-clearpositiveboost", t, m, format!("move: {n}"));
            self.boost(boosts, Some(m), Some(m), EffRef::NONE, false, false);
            let mut zero = boosts;
            for k in 0..zero.n as usize {
                zero.e[k].1 = 0;
            }
            self.set_boost(t, &zero);
        }
    }

    /// Champions' hitStepMoveHitLoop
    fn hit_step_move_hit_loop(&mut self, targets: &[Mon], m: Mon, mv: u16) -> Vec<Val> {
        let mut damage: Vec<Val> = vec![Val::Num(0.0); targets.len()];
        self.mvm(mv).total_damage = 0.0;
        self.pm(m).last_damage = 0;
        let mut target_hits: u32 = 1;
        if let Some((lo, hi)) = self.mv(mv).multihit {
            if lo != hi {
                if lo == 2 && hi == 5 {
                    let table = [2u32, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 4, 4, 4, 5, 5, 5];
                    let i = self.prng.sample_index(table.len());
                    target_hits = table[i];
                    if target_hits < 4 && self.has_item(m, "loadeddice") {
                        target_hits = 5 - self.random(2);
                    }
                } else {
                    target_hits = self.random_range(lo as u32, hi as u32 + 1);
                }
            } else {
                target_hits = lo as u32;
            }
        }
        if target_hits == 10 && self.has_item(m, "loadeddice") {
            target_hits -= self.random(7);
        }
        let mut null_damage = true;
        let mut move_damage: Vec<Val> = vec![];
        let sleep_usable = self.mv(mv).sleep_usable
            || self.mv(mv).source_effect.map(|se| {
                let e = self.eff(se);
                e.kind == Kind::Move && self.d.moves[e.move_def as usize].sleep_usable
            }).unwrap_or(false);
        let mut targets_copy: Vec<TS> = targets.iter().map(|&t| TS::M(t)).collect();
        let mut hit: u32 = 1;
        let slp = self.d.c.slp;
        while hit <= target_hits {
            if damage.iter().any(|v| *v == Val::False) {
                break;
            }
            if hit > 1 && self.p(m).status == slp && !sleep_usable {
                break;
            }
            if targets.iter().all(|&t| self.p(t).hp == 0) {
                break;
            }
            self.mvm(mv).hit = hit as u8;
            self.mvm(mv).last_hit = hit == target_hits;
            let smart = self.mv(mv).smart_target == Some(true);
            if smart && targets.len() > 1 {
                targets_copy = vec![TS::M(targets[hit as usize - 1])];
                // JS: damage = [damage[hit - 1]] reads past the end on later hits (-> undefined)
                damage = vec![damage.get(hit as usize - 1).cloned().unwrap_or(Val::Undef)];
            } else {
                targets_copy = targets.iter().map(|&t| TS::M(t)).collect();
            }
            let target = targets_copy[0].mon();
            if let Some(t) = target {
                if self.mv(mv).smart_target.is_some() {
                    if hit > 1 {
                        if self.log.is_some() {
                            let ms = self.mon_str(m);
                            let ts = self.mon_str(t);
                            let n = self.move_name(mv);
                            self.add_move(vec!["-anim".into(), ms, n, ts]);
                        }
                    } else {
                        self.retarget_last_move(t);
                    }
                }
            }
            if let Some(t) = target {
                if self.mv(mv).multiaccuracy && hit > 1 {
                    let mut accuracy: Option<f64> = self.mv(mv).accuracy;
                    let table = [1.0, 4.0 / 3.0, 5.0 / 3.0, 2.0, 7.0 / 3.0, 8.0 / 3.0, 3.0];
                    if let Some(mut acc) = accuracy {
                        if !self.mv(mv).ignore_accuracy {
                            let b = self.full_boosts_of(m);
                            if let Val::Boosts(nb) = self.run_event(Ev::ModifyBoost, Arg::Mon(m), None, EffRef::NONE, Val::Boosts(b)) {
                                let boost = (nb.get(B_ACC).unwrap_or(0) as i32).clamp(-6, 6);
                                if boost > 0 { acc *= table[boost as usize]; } else { acc /= table[(-boost) as usize]; }
                            }
                        }
                        if !self.mv(mv).ignore_evasion {
                            let b = self.full_boosts_of(t);
                            if let Val::Boosts(nb) = self.run_event(Ev::ModifyBoost, Arg::Mon(t), None, EffRef::NONE, Val::Boosts(b)) {
                                let boost = (nb.get(B_EVA).unwrap_or(0) as i32).clamp(-6, 6);
                                if boost > 0 { acc /= table[boost as usize]; } else if boost < 0 { acc *= table[(-boost) as usize]; }
                            }
                        }
                        accuracy = Some(acc);
                    }
                    let mr = self.mref(mv);
                    accuracy = val_acc(self.run_event(Ev::ModifyAccuracy, Arg::Mon(t), Some(m), mr, acc_val(accuracy)));
                    if !self.mv(mv).always_hit {
                        accuracy = val_acc(self.run_event(Ev::Accuracy, Arg::Mon(t), Some(m), mr, acc_val(accuracy)));
                        if let Some(acc) = accuracy {
                            // randomChance(accuracy, 100): accuracy can be fractional here
                            if !((self.random(100) as f64) < acc) {
                                break;
                            }
                        }
                    }
                }
            }
            let (dmg_this, tc) = self.spread_move_hit(targets_copy.clone(), m, mv, HitFx::Move, false, false);
            targets_copy = tc;
            if self.mv(mv).smart_target == Some(true) {
                move_damage.extend(dmg_this);
            } else {
                move_damage = dmg_this;
            }
            if !move_damage.iter().any(|v| *v != Val::False) {
                break;
            }
            null_damage = false;
            let smart = self.mv(mv).smart_target == Some(true);
            for (i, md) in move_damage.clone().iter().enumerate() {
                if smart && i != hit as usize - 1 {
                    continue;
                }
                let v = if *md == Val::True || !md.truthy() { 0.0 } else { md.num() };
                // JS `damage[i] = ...` grows the array (holes = undefined): after a Dragon Darts split, damage is
                // [undefined, 2nd dart] and the move counts as hitting (Life Orb...) unless the 2nd dart failed
                if i >= damage.len() {
                    damage.resize(i + 1, Val::Undef);
                }
                damage[i] = Val::Num(v);
                self.mvm(mv).total_damage += v;
            }
            self.each_event(Ev::Update, EffRef::NONE);
            if self.p(m).hp == 0 && targets.len() == 1 {
                hit += 1;
                break;
            }
            hit += 1;
        }
        if hit == 1 {
            return vec![Val::False; damage.len()];
        }
        if null_damage {
            for v in damage.iter_mut() {
                *v = Val::False;
            }
        }
        self.faint_messages(false, false, self.p(m).hp == 0);
        if self.mv(mv).multihit.is_some() && self.mv(mv).smart_target.is_none() && !(self.mv(mv).hit == 1 && self.mv(mv).multihit_type == 1) {
            add!(self, "-hitcount", targets[0], (hit - 1) as i32);
        }
        if self.mv(mv).total_damage != 0.0 {
            let td = self.mv(mv).total_damage;
            self.apply_recoil_damage(td, mv, m);
        }
        if self.mv(mv).smart_target == Some(true) {
            targets_copy = targets.iter().map(|&t| TS::M(t)).collect();
        }
        let mid = self.mv(mv).def;
        for (i, t) in targets_copy.iter().enumerate() {
            if let TS::M(t) = *t {
                if m != t {
                    let md = move_damage.get(i).copied().unwrap_or(Val::Undef);
                    self.got_attacked(t, mid, md, m);
                    if md.is_num() {
                        let add_n = if self.mv(mv).smart_target == Some(true) { 1 } else { hit as i32 - 1 };
                        self.pm(t).times_attacked += add_n;
                    }
                }
            }
        }
        if self.mv(mv).ohko && self.p(targets[0]).hp == 0 {
            add!(self, "-ohko");
        }
        if !damage.iter().any(|v| v.truthy() || matches!(v, Val::Num(x) if *x == 0.0)) {
            return damage;
        }
        self.each_event(Ev::Update, EffRef::NONE);
        let tc: Vec<Mon> = targets_copy.iter().filter_map(|t| t.mon()).collect();
        self.after_move_secondary_event(&tc, m, mv);
        let sheer = self.mv(mv).has_sheer_force && self.has_ability(m, "sheerforce");
        if !sheer {
            for (i, d) in damage.clone().iter().enumerate() {
                let cur = if targets.len() == 1 { Val::Num(self.mv(mv).total_damage) } else { *d };
                if let Val::Num(c) = cur {
                    let t = targets[i];
                    if self.p(t).hp > 0 {
                        let before = self.p(t).hurt_this_turn.unwrap_or(0) as f64 + c;
                        let p = self.p(t);
                        if p.hp as f64 <= p.maxhp as f64 / 2.0 && before > p.maxhp as f64 / 2.0 {
                            self.run_event(Ev::EmergencyExit, Arg::Mon(t), Some(m), EffRef::NONE, Val::Undef);
                        }
                    }
                }
            }
        }
        damage
    }

    /// Champions: AfterMoveSecondary runs even with Sheer Force
    fn after_move_secondary_event(&mut self, targets: &[Mon], m: Mon, mv: u16) {
        let mr = self.mref(mv);
        let t0 = targets.first().copied();
        self.single_event(Ev::AfterMoveSecondary, mr, StateLoc::None, Arg::from_opt(t0), Arg::Mon(m), mr, Val::Undef);
        self.run_event_multi(Ev::AfterMoveSecondary, targets, Some(m), mr, None);
    }

    /// tryMoveHit: moves that target a side or the field
    fn try_move_hit(&mut self, targets: &[Mon], m: Mon, mv: u16) -> Val {
        let target = targets.first().copied();
        self.set_active_move(Some(mv), Some(m), target);
        let mr = self.mref(mv);
        let mut hit = self.single_event(Ev::Try, mr, StateLoc::None, Arg::Mon(m), Arg::from_opt(target), mr, Val::Undef);
        if hit.truthy() {
            hit = self.single_event(Ev::PrepareHit, mr, StateLoc::None, Arg::from_opt(target), Arg::Mon(m), mr, Val::Undef);
        }
        if hit.truthy() {
            hit = self.run_event(Ev::PrepareHit, Arg::Mon(m), target, mr, Val::Undef);
        }
        if !hit.truthy() {
            if hit == Val::False {
                add!(self, "-fail", m);
                self.attr_last_move(&["[still]"]);
            }
            return Val::False;
        }
        let r = if self.mv(mv).target == MoveTarget::All {
            self.run_event(Ev::TryHitField, Arg::from_opt(target), Some(m), mr, Val::Undef)
        } else {
            self.run_event(Ev::TryHitSide, Arg::from_opt(target), Some(m), mr, Val::Undef)
        };
        if !r.truthy() {
            if r == Val::False {
                add!(self, "-fail", m);
                self.attr_last_move(&["[still]"]);
            }
            return Val::False;
        }
        let ts = match target {
            Some(t) => vec![TS::M(t)],
            None => vec![TS::Null],
        };
        self.move_hit(&ts, m, mv, HitFx::Move, false, false)
    }

    /// moveHit: spreadMoveHit for one target, returning `undefined` for true
    pub fn move_hit(&mut self, targets: &[TS], m: Mon, mv: u16, fx: HitFx, is_secondary: bool, is_self: bool) -> Val {
        let (d, _) = self.spread_move_hit(targets.to_vec(), m, mv, fx, is_secondary, is_self);
        let r = d.first().copied().unwrap_or(Val::Undef);
        if r == Val::True { Val::Undef } else { r }
    }

    fn fx_ref(&self, mv: u16, fx: HitFx) -> EffRef {
        let part = match fx {
            HitFx::Move => 0,
            HitFx::Secondary(k) => k + 1,
            HitFx::SelfOf => SELF_PART,
            HitFx::SecondarySelf(_) => 0xFD,
            HitFx::SelfBoost => 0xFC,
        };
        EffRef { eff: self.mv(mv).eff, mv, part }
    }

    /// the hit effect's data (secondary / self / selfBoost): HitEffectData; None for the move itself
    fn fx_data(&self, mv: u16, fx: HitFx) -> Option<HitEffectData> {
        let a = self.mv(mv);
        match fx {
            HitFx::Move => None,
            HitFx::Secondary(k) => a.secondaries.as_ref().and_then(|s| s.get(k as usize).cloned()),
            HitFx::SelfOf => a.self_.clone(),
            HitFx::SecondarySelf(k) => a.secondaries.as_ref().and_then(|s| s.get(k as usize)).and_then(|s| s.self_.as_deref().cloned()),
            HitFx::SelfBoost => Some(HitEffectData { boosts: a.self_boost, ..Default::default() }),
        }
    }

    /// Champions' spreadMoveHit
    pub fn spread_move_hit(&mut self, targets: Vec<TS>, m: Mon, mv: u16, fx: HitFx, is_secondary: bool, is_self: bool) -> (Vec<Val>, Vec<TS>) {
        let mut targets = targets;
        let target = targets.first().copied().unwrap_or(TS::Null);
        let mut damage: Vec<Val> = vec![Val::True; targets.len()];
        let mr = self.mref(mv);
        let fxr = self.fx_ref(mv, fx);
        let mt = self.mv(mv).target;
        let mut hit_result = Val::True;
        if mt == MoveTarget::All && !is_self {
            hit_result = self.single_event(Ev::TryHitField, fxr, StateLoc::None, Arg::from_opt(target.mon()), Arg::Mon(m), mr, Val::Undef);
        } else if matches!(mt, MoveTarget::FoeSide | MoveTarget::AllySide | MoveTarget::AllyTeam) && !is_self {
            hit_result = self.single_event(Ev::TryHitSide, fxr, StateLoc::None, Arg::from_opt(target.mon()), Arg::Mon(m), mr, Val::Undef);
        } else if let TS::M(t) = target {
            hit_result = self.single_event(Ev::TryHit, fxr, StateLoc::None, Arg::Mon(t), Arg::Mon(m), mr, Val::Undef);
        }
        if !hit_result.truthy() {
            if hit_result == Val::False {
                add!(self, "-fail", m);
                self.attr_last_move(&["[still]"]);
            }
            return (vec![Val::False], targets);
        }
        // 0. substitute
        if !is_secondary && !is_self && !matches!(mt, MoveTarget::All | MoveTarget::AllyTeam | MoveTarget::AllySide | MoveTarget::FoeSide) {
            for (i, t) in targets.iter().enumerate() {
                if let TS::M(t) = *t {
                    damage[i] = self.run_event(Ev::TryPrimaryHit, Arg::Mon(t), Some(m), fxr, Val::Undef);
                }
            }
        }
        let fx_self = self.fx_data(mv, fx).map(|d| d.self_.is_some()).unwrap_or(self.mv(mv).self_.is_some());
        for i in 0..targets.len() {
            if damage[i] == Val::Num(0.0) {
                damage[i] = Val::True;
                targets[i] = TS::Null;
            }
            if matches!(targets[i], TS::M(_)) && is_secondary && !fx_self {
                damage[i] = Val::True;
            }
            if !damage[i].truthy() {
                targets[i] = TS::False;
            }
        }
        // 1. getDamage
        damage = self.get_spread_damage(damage, &targets, m, mv, fx, is_secondary, is_self);
        for i in 0..targets.len() {
            if damage[i] == Val::False {
                targets[i] = TS::False;
            }
        }
        // 2. spreadDamage
        let tmons: Vec<Option<Mon>> = targets.iter().map(|t| t.mon()).collect();
        damage = self.spread_damage(&mut damage.clone(), &tmons, Some(m), mr, false);
        for i in 0..targets.len() {
            if damage[i] == Val::False {
                targets[i] = TS::False;
            }
        }
        // 3. onHit
        damage = self.run_move_effects(damage, &targets, m, mv, fx, is_secondary, is_self);
        for i in 0..targets.len() {
            if !damage[i].truthy() && !matches!(damage[i], Val::Num(x) if x == 0.0) {
                targets[i] = TS::False;
            }
        }
        let active_target = self.active_target;
        // 4. self drops
        let has_self = match fx {
            HitFx::Move => self.mv(mv).self_.is_some(),
            _ => self.fx_data(mv, fx).map(|d| d.self_.is_some()).unwrap_or(false),
        };
        if has_self && !self.mv(mv).self_dropped {
            self.self_drops(&targets, m, mv, fx, is_secondary);
        }
        // 5. secondaries
        if fx == HitFx::Move && self.mv(mv).secondaries.is_some() {
            self.secondaries(&targets, m, mv, is_self);
        }
        self.active_target = active_target;
        // 6. force switch
        if fx == HitFx::Move && self.mv(mv).force_switch {
            damage = self.force_switch(damage, &targets, m, mv);
        }
        for i in 0..targets.len() {
            if !damage[i].truthy() && !matches!(damage[i], Val::Num(x) if x == 0.0) {
                targets[i] = TS::False;
            }
        }
        let mut damaged_targets: Vec<Mon> = vec![];
        let mut damaged_damage: Vec<Val> = vec![];
        for (i, t) in targets.iter().enumerate() {
            if let (Val::Num(_), TS::M(t)) = (damage[i], *t) {
                damaged_targets.push(t);
                damaged_damage.push(damage[i]);
            }
        }
        let ohp = self.p(m).hp;
        if !damaged_damage.is_empty() && !is_secondary && !is_self {
            self.run_event_multi(Ev::DamagingHit, &damaged_targets, Some(m), mr, Some(damaged_damage));
            if fx == HitFx::Move && self.eff(self.mv(mv).eff).has(self.d.cbs(Ev::AfterHit)[P_ON]) {
                for &t in &damaged_targets {
                    self.single_event(Ev::AfterHit, fxr, StateLoc::None, Arg::Mon(t), Arg::Mon(m), mr, Val::Undef);
                }
            }
            let p = self.p(m);
            if p.hp > 0 && p.hp as f64 <= p.maxhp as f64 / 2.0 && ohp as f64 > p.maxhp as f64 / 2.0 {
                self.run_event(Ev::EmergencyExit, Arg::Mon(m), None, EffRef::NONE, Val::Undef);
            }
        }
        (damage, targets)
    }

    #[allow(clippy::too_many_arguments)]
    fn get_spread_damage(&mut self, damage: Vec<Val>, targets: &[TS], m: Mon, mv: u16, fx: HitFx, is_secondary: bool, is_self: bool) -> Vec<Val> {
        let mut damage = damage;
        for (i, t) in targets.iter().enumerate() {
            let TS::M(t) = *t else { continue };
            self.active_target = Some(t);
            damage[i] = Val::Undef;
            // getDamage(source, target, moveData): for secondaries/self the hit effect has no damage
            let cur = if fx == HitFx::Move { self.get_damage(m, t, mv, false) } else { Val::Undef };
            if cur == Val::False || cur == Val::Null {
                if damage[i] == Val::False && !is_secondary && !is_self {
                    add!(self, "-fail", m);
                    self.attr_last_move(&["[still]"]);
                }
                damage[i] = Val::False;
                continue;
            }
            damage[i] = cur;
        }
        damage
    }

    #[allow(clippy::too_many_arguments)]
    fn run_move_effects(&mut self, damage: Vec<Val>, targets: &[TS], m: Mon, mv: u16, fx: HitFx, is_secondary: bool, is_self: bool) -> Vec<Val> {
        let mut damage = damage;
        let mut did_anything = damage.iter().copied().reduce(combine_results).unwrap_or(Val::Undef);
        let mr = self.mref(mv);
        let fxr = self.fx_ref(mv, fx);
        // the hit effect's fields
        let (boosts, heal, status, volatile, side_cond, slot_cond, weather, terrain, pseudo, force_switch, self_destruct, self_switch, has_self) = match fx {
            HitFx::Move => {
                let a = self.mv(mv);
                (a.boosts, a.heal, a.status, a.volatile_status, a.side_condition, a.slot_condition, a.weather, a.terrain, a.pseudo_weather, a.force_switch, a.selfdestruct, a.self_switch, a.self_.is_some())
            }
            _ => {
                let d = self.fx_data(mv, fx).unwrap_or_default();
                (d.boosts, None, d.status, d.volatile_status, d.side_condition, None, None, None, None, false, SelfDestruct::No, SelfSwitch::No, d.self_.is_some())
            }
        };
        let fx_has_on_hit = match fx {
            HitFx::Move => self.eff(self.mv(mv).eff).has(self.d.cbs(Ev::Hit)[P_ON]),
            _ => self.fx_data(mv, fx).map(|d| d.has_on_hit).unwrap_or(false),
        };
        let mt = self.mv(mv).target;
        for (i, t) in targets.iter().enumerate() {
            if *t == TS::False {
                continue;
            }
            let mut did_something = Val::Undef;
            if let TS::M(t) = *t {
                if let Some(b) = boosts {
                    if !self.p(t).fainted {
                        let r = self.boost(b, Some(t), Some(m), mr, is_secondary, is_self);
                        did_something = combine_results(did_something, r);
                    }
                }
                if let Some((hn, hd)) = heal {
                    if !self.p(t).fainted {
                        if self.p(t).hp >= self.p(t).maxhp {
                            add!(self, "-fail", t, "heal");
                            self.attr_last_move(&["[still]"]);
                            damage[i] = combine_results(damage[i], Val::False);
                            did_anything = combine_results(did_anything, Val::Null);
                            continue;
                        }
                        let amount = self.p(t).base_maxhp as f64 * hn / hd;
                        let d = self.heal(amount.round(), Some(t), Some(m), mr);
                        if !d.truthy() && !matches!(d, Val::Num(x) if x == 0.0) {
                            if d != Val::Null {
                                add!(self, "-fail", m);
                                self.attr_last_move(&["[still]"]);
                            }
                            damage[i] = combine_results(damage[i], Val::False);
                            did_anything = combine_results(did_anything, Val::Null);
                            continue;
                        }
                        did_something = Val::True;
                    }
                }
                if let Some(st) = status {
                    let r = self.try_set_status(t, st, Some(m), mr);
                    if !r.truthy() && self.mv(mv).status.is_some() {
                        damage[i] = combine_results(damage[i], Val::False);
                        did_anything = combine_results(did_anything, Val::Null);
                        continue;
                    }
                    did_something = combine_results(did_something, r);
                }
                if let Some(v) = volatile {
                    let r = self.add_volatile(t, v, Some(m), mr, None);
                    did_something = combine_results(did_something, r);
                }
                if let Some(sc) = side_cond {
                    let r = self.add_side_condition(t.s, sc, Some(m), mr);
                    did_something = combine_results(did_something, r);
                }
                if let Some(sc) = slot_cond {
                    let pos = self.p(t).position;
                    let r = self.add_slot_condition(t.s, pos, sc, Some(m), mr);
                    did_something = combine_results(did_something, r);
                }
                if let Some(w) = weather {
                    let r = self.set_weather(w, Some(m), mr);
                    did_something = combine_results(did_something, r);
                }
                if let Some(tr_) = terrain {
                    let r = self.set_terrain(tr_, Some(m), mr);
                    did_something = combine_results(did_something, r);
                }
                if let Some(pw) = pseudo {
                    let r = self.add_pseudo_weather(pw, Some(m), mr);
                    did_something = combine_results(did_something, r);
                }
                if force_switch {
                    let r = Val::from_bool(self.can_switch(t.s as usize) > 0);
                    did_something = combine_results(did_something, r);
                }
                if mt == MoveTarget::All && !is_self {
                    if fx_has_on_hit && self.eff(self.mv(mv).eff).has(self.d.cb_id("onHitField")) {
                        let r = self.single_event(Ev::HitField, fxr, StateLoc::None, Arg::Mon(t), Arg::Mon(m), mr, Val::Undef);
                        did_something = combine_results(did_something, r);
                    } else if fx == HitFx::Move && self.eff(self.mv(mv).eff).has(self.d.cb_id("onHitField")) {
                        let r = self.single_event(Ev::HitField, fxr, StateLoc::None, Arg::Mon(t), Arg::Mon(m), mr, Val::Undef);
                        did_something = combine_results(did_something, r);
                    }
                } else if matches!(mt, MoveTarget::FoeSide | MoveTarget::AllySide) && !is_self {
                    if fx == HitFx::Move && self.eff(self.mv(mv).eff).has(self.d.cb_id("onHitSide")) {
                        let r = self.single_event(Ev::HitSide, fxr, StateLoc::None, Arg::Side(t.s), Arg::Mon(m), mr, Val::Undef);
                        did_something = combine_results(did_something, r);
                    }
                } else {
                    if fx_has_on_hit {
                        let r = self.single_event(Ev::Hit, fxr, StateLoc::None, Arg::Mon(t), Arg::Mon(m), mr, Val::Undef);
                        did_something = combine_results(did_something, r);
                    }
                    if !is_self && !is_secondary {
                        self.run_event(Ev::Hit, Arg::Mon(t), Some(m), mr, Val::Undef);
                    }
                }
            }
            if self_destruct == SelfDestruct::IfHit && damage[i] != Val::False {
                self.faint(m, Some(m), mr);
            }
            if self_switch != SelfSwitch::No {
                if self.can_switch(m.s as usize) > 0 && !self.has_vol(m, "commanded") {
                    did_something = Val::True;
                } else {
                    did_something = combine_results(did_something, Val::False);
                }
            }
            if did_something == Val::Undef {
                did_something = Val::True;
            }
            damage[i] = combine_results(damage[i], if did_something == Val::Null { Val::False } else { did_something });
            did_anything = combine_results(did_anything, did_something);
        }
        if !did_anything.truthy() && !matches!(did_anything, Val::Num(x) if x == 0.0) && !has_self && self_destruct == SelfDestruct::No {
            if !is_self && !is_secondary && did_anything == Val::False {
                add!(self, "-fail", m);
                self.attr_last_move(&["[still]"]);
            }
        } else if self.mv(mv).self_switch != SelfSwitch::No && self.p(m).hp > 0 && !self.has_vol(m, "commanded") {
            let mid = self.mv(mv).def;
            self.pm(m).switch_flag = SwitchFlag::Move(mid);
        }
        damage
    }

    fn self_drops(&mut self, targets: &[TS], m: Mon, mv: u16, fx: HitFx, is_secondary: bool) {
        let self_fx = match fx {
            HitFx::Move => HitFx::SelfOf,
            HitFx::Secondary(k) => HitFx::SecondarySelf(k),
            other => other,
        };
        let self_data = self.fx_data(mv, self_fx);
        for t in targets {
            if *t == TS::False {
                continue;
            }
            if self.mv(mv).self_dropped {
                continue;
            }
            let has_boosts = self_data.as_ref().map(|d| d.boosts.is_some()).unwrap_or(false);
            if !is_secondary && has_boosts {
                let roll = self.random(100) as i32;
                let chance = self_data.as_ref().and_then(|d| d.chance);
                if chance.is_none() || roll < chance.unwrap() {
                    self.move_hit(&[TS::M(m)], m, mv, self_fx, is_secondary, true);
                }
                if self.mv(mv).multihit.is_none() {
                    self.mvm(mv).self_dropped = true;
                }
            } else {
                self.move_hit(&[TS::M(m)], m, mv, self_fx, is_secondary, true);
            }
        }
    }

    fn secondaries(&mut self, targets: &[TS], m: Mon, mv: u16, is_self: bool) {
        let Some(secs) = self.mv(mv).secondaries.clone() else { return };
        let _ = secs;
        let mr = self.mref(mv);
        for t in targets {
            let TS::M(t) = *t else {
                if *t == TS::Null {
                    // null targets (substitute) still roll in Showdown? `if (target === false) continue`
                    // -> null passes: runEvent('ModifySecondaries', null...) then rolls
                    self.secondaries_for(None, m, mv, mr, is_self);
                }
                continue;
            };
            self.secondaries_for(Some(t), m, mv, mr, is_self);
        }
    }

    fn secondaries_for(&mut self, t: Option<Mon>, m: Mon, mv: u16, mr: EffRef, is_self: bool) {
        // ModifySecondaries: handlers return a filtered list (Shield Dust, Covert Cloak)
        let n = self.mv(mv).secondaries.as_ref().map(|s| s.len()).unwrap_or(0);
        let r = self.run_event(Ev::ModifySecondaries, Arg::from_opt(t), Some(m), mr, Val::Num(((1u32 << n) - 1) as f64));
        let mask = match r {
            Val::Num(x) => x as u32,
            _ => (1u32 << n) - 1,
        };
        for k in 0..n {
            if mask & (1 << k) == 0 {
                continue;
            }
            let roll = self.random(100) as i32;
            let sec = self.mv(mv).secondaries.as_ref().unwrap()[k].clone();
            let overflow = sec.boosts.is_some() || sec.self_.is_some();
            let ok = match sec.chance {
                None => true,
                Some(c) => roll < if overflow { c % 256 } else { c },
            };
            if ok {
                let ts = match t {
                    Some(t) => vec![TS::M(t)],
                    None => vec![TS::Null],
                };
                self.move_hit(&ts, m, mv, HitFx::Secondary(k as u8), true, is_self);
            }
        }
    }

    fn force_switch(&mut self, damage: Vec<Val>, targets: &[TS], m: Mon, mv: u16) -> Vec<Val> {
        let mut damage = damage;
        let mr = self.mref(mv);
        for (i, t) in targets.iter().enumerate() {
            let TS::M(t) = *t else { continue };
            if self.p(t).hp > 0 && self.p(m).hp > 0 && self.can_switch(t.s as usize) > 0 {
                let r = self.run_event(Ev::DragOut, Arg::Mon(t), Some(m), mr, Val::Undef);
                if r.truthy() {
                    self.pm(t).force_switch_flag = true;
                } else if r == Val::False && self.mv(mv).category == Category::Status {
                    add!(self, "-fail", m);
                    self.attr_last_move(&["[still]"]);
                    damage[i] = Val::False;
                }
            }
        }
        damage
    }

    pub fn apply_recoil_damage_pub(&mut self, dealt: f64, mv: u16, m: Mon) {
        self.apply_recoil_damage(dealt, mv, m)
    }
    fn apply_recoil_damage(&mut self, dealt: f64, mv: u16, m: Mon) {
        let a = self.mv(mv);
        let recoil = if a.struggle_recoil {
            clamp_int((self.p(m).base_maxhp as f64 / 4.0).round(), Some(1.0), None)
        } else if a.mind_blown_recoil {
            (self.p(m).maxhp as f64 / 2.0).round()
        } else if let Some((rn, rd)) = a.recoil {
            clamp_int((dealt * rn / rd).round(), Some(1.0), None)
        } else {
            return;
        };
        let before = self.p(m).hp;
        if self.mv(mv).struggle_recoil {
            let e = self.d.cond("strugglerecoil").unwrap_or(self.d.c.recoil);
            self.direct_damage(recoil, Some(m), Some(m), EffRef::eff(e));
        } else if self.mv(mv).mind_blown_recoil {
            let e = self.d.cond(&self.move_def(mv).id.clone()).unwrap_or(self.d.c.recoil);
            self.damage(recoil, Some(m), Some(m), EffRef::eff(e), false);
        } else {
            let e = self.d.c.recoil;
            self.damage(recoil, Some(m), Some(m), EffRef::eff(e), false);
        }
        let p = self.p(m);
        if p.hp as f64 <= p.maxhp as f64 / 2.0 && before as f64 > p.maxhp as f64 / 2.0 {
            self.run_event(Ev::EmergencyExit, Arg::Mon(m), Some(m), EffRef::NONE, Val::Undef);
        }
    }

    // ------------------------------------------------------------------
    // damage
    // ------------------------------------------------------------------
    /// getDamage(source, target, move)
    pub fn get_damage(&mut self, source: Mon, target: Mon, mv: u16, suppress: bool) -> Val {
        if !self.run_immunity_move(target, mv, !suppress) {
            return Val::False;
        }
        if self.mv(mv).ohko {
            return Val::Num(self.p(target).maxhp as f64);
        }
        let mr = self.mref(mv);
        if self.eff(self.mv(mv).eff).has(self.d.cb_id("damageCallback")) {
            return super::handlers::damage_callback(self, source, target, mv);
        }
        match self.mv(mv).damage {
            FixedDmg::Level => return Val::Num(self.p(source).level as f64),
            FixedDmg::Amount(n) => return Val::Num(n as f64),
            FixedDmg::None => {}
        }
        let category = self.mv(mv).category;
        let mut base_power = self.mv(mv).base_power;
        if self.eff(self.mv(mv).eff).has(self.d.cb_id("basePowerCallback")) {
            let r = super::handlers::base_power_callback(self, source, target, mv);
            match r {
                Val::Num(x) => base_power = x,
                Val::False => return Val::False,
                Val::Null => return Val::Null,
                _ => base_power = r.num(),
            }
        }
        if base_power == 0.0 {
            return Val::Undef;
        }
        base_power = clamp_int(base_power, Some(1.0), None);
        let crit_ratio = self.run_event(Ev::ModifyCritRatio, Arg::Mon(source), Some(target), mr, Val::Num(self.mv(mv).crit_ratio as f64)).num();
        let crit_ratio = clamp_int(crit_ratio, Some(0.0), Some(4.0)) as usize;
        let crit_mult = [0u32, 24, 8, 2, 1];
        let tslot = (target.s as usize, self.p(target).position as usize);
        let mut crit = self.mv(mv).will_crit.unwrap_or(false);
        if self.mv(mv).will_crit.is_none() && crit_ratio > 0 {
            crit = self.random_chance(1, crit_mult[crit_ratio]);
        }
        self.mvm(mv).move_hit_data[tslot.0][tslot.1].crit = crit;
        if crit {
            crit = self.run_event(Ev::CriticalHit, Arg::Mon(target), None, mr, Val::Undef).truthy();
            self.mvm(mv).move_hit_data[tslot.0][tslot.1].crit = crit;
        }
        base_power = self.run_event_full(Ev::BasePower, Arg::Mon(source), Arg::Mon(target), mr, Val::Num(base_power), true, false).num();
        if base_power == 0.0 {
            return Val::Num(0.0);
        }
        base_power = clamp_int(base_power, Some(1.0), None);
        let level = self.p(source).level as f64;
        let attacker = if self.mv(mv).override_offensive_target { target } else { source };
        let defender = if self.mv(mv).override_defensive_source { source } else { target };
        let is_physical = category == Category::Physical;
        let attack_stat = self.mv(mv).override_offensive_stat.map(|x| x as usize).unwrap_or(if is_physical { 1 } else { 3 });
        let defense_stat = self.mv(mv).override_defensive_stat.map(|x| x as usize).unwrap_or(if is_physical { 2 } else { 4 });
        let mut atk_boosts = self.p(attacker).boosts[attack_stat - 1] as i32;
        let mut def_boosts = self.p(defender).boosts[defense_stat - 1] as i32;
        let mut ign_neg_off = self.mv(mv).ignore_negative_offensive;
        let mut ign_pos_def = self.mv(mv).ignore_positive_defensive;
        if crit {
            ign_neg_off = true;
            ign_pos_def = true;
        }
        let ign_off = self.mv(mv).ignore_offensive || (ign_neg_off && atk_boosts < 0);
        let ign_def = self.mv(mv).ignore_defensive || (ign_pos_def && def_boosts > 0);
        if ign_off {
            atk_boosts = 0;
        }
        if ign_def {
            def_boosts = 0;
        }
        let mut attack = self.calculate_stat(attacker, attack_stat, atk_boosts, 1.0, Some(source));
        let mut defense = self.calculate_stat(defender, defense_stat, def_boosts, 1.0, Some(target));
        let attack_stat2 = if category == Category::Physical { 1 } else { 3 };
        let ev_atk = if attack_stat2 == 1 { Ev::ModifyAtk } else { Ev::ModifySpA };
        let ev_def = match defense_stat {
            2 => Ev::ModifyDef,
            4 => Ev::ModifySpD,
            1 => Ev::ModifyAtk,
            3 => Ev::ModifySpA,
            _ => Ev::ModifySpe,
        };
        attack = self.run_event(ev_atk, Arg::Mon(source), Some(target), mr, Val::Num(attack)).num();
        defense = self.run_event(ev_def, Arg::Mon(target), Some(source), mr, Val::Num(defense)).num();
        let base_damage = tr(tr(tr(tr(2.0 * level / 5.0 + 2.0) * base_power * attack) / defense) / 50.0);
        self.modify_damage(base_damage, source, target, mv, suppress)
    }

    /// Champions' modifyDamage
    fn modify_damage(&mut self, base: f64, m: Mon, target: Mon, mv: u16, suppress: bool) -> Val {
        let mr = self.mref(mv);
        let mtype = self.mv(mv).type_;
        let mut d = base + 2.0;
        if self.mv(mv).spread_hit {
            d = self.modify(d, 0.75);
        } else if self.mv(mv).multihit_type == 1 && self.mv(mv).hit > 1 {
            d = self.modify(d, 0.25);
        }
        d = self.run_event_full(Ev::WeatherModifyDamage, Arg::Mon(m), Arg::Mon(target), mr, Val::Num(d), false, true).num();
        let tslot = (target.s as usize, self.p(target).position as usize);
        let is_crit = self.mv(mv).move_hit_data[tslot.0][tslot.1].crit;
        if is_crit {
            d = tr(d * self.mv(mv).crit_modifier.unwrap_or(1.5));
        }
        d = self.randomizer(d);
        if mtype != NO_TYPE {
            let mut stab = 1.0;
            let is_stab = self.mv(mv).force_stab || self.has_type_id(m, mtype) || {
                let t = self.get_types(m, false);
                t.contains(&mtype)
            };
            if is_stab {
                stab = 1.5;
            }
            stab = self.run_event(Ev::ModifySTAB, Arg::Mon(m), Some(target), mr, Val::Num(stab)).num();
            d = self.modify(d, stab);
        }
        let mut type_mod = self.run_effectiveness(target, mv);
        type_mod = type_mod.clamp(-6, 6);
        self.mvm(mv).move_hit_data[tslot.0][tslot.1].type_mod = type_mod;
        if type_mod > 0 {
            if !suppress {
                add!(self, "-supereffective", target, type_mod.min(2));
            }
            for _ in 0..type_mod {
                d *= 2.0;
            }
        }
        if type_mod < 0 {
            if !suppress {
                add!(self, "-resisted", target, (-type_mod).min(2));
            }
            for _ in 0..(-type_mod) {
                d = tr(d / 2.0);
            }
        }
        if is_crit && !suppress {
            add!(self, "-crit", target);
        }
        if self.p(m).status == self.d.c.brn && self.mv(mv).category == Category::Physical && !self.has_ability(m, "guts") {
            if self.move_def(mv).id != "facade" {
                d = self.modify(d, 0.5);
            }
        }
        d = self.run_event(Ev::ModifyDamage, Arg::Mon(m), Some(target), mr, Val::Num(d)).num();
        let bp = self.mv(mv).move_hit_data[tslot.0][tslot.1].bypass_protect;
        if let Some(bpe) = bp {
            d = self.modify(d, 0.25);
            if bpe != NO_EFF && self.eff(bpe).effect_type == EffType::Ability {
                let n = self.eff(bpe).name.clone();
                add!(self, "-ability", m, n);
            }
            add!(self, "-zbroken", target);
        }
        if d == 0.0 {
            return Val::Num(1.0);
        }
        Val::Num(tr_bits(d, 16))
    }

    /// getConfusionDamage(pokemon, basePower)
    pub fn get_confusion_damage(&mut self, m: Mon, base_power: f64) -> f64 {
        let ab = self.p(m).boosts[0] as i32;
        let db = self.p(m).boosts[1] as i32;
        let attack = self.calculate_stat(m, 1, ab, 1.0, None);
        let defense = self.calculate_stat(m, 2, db, 1.0, None);
        let level = self.p(m).level as f64;
        let base = tr(tr(tr(tr(2.0 * level / 5.0 + 2.0) * base_power * attack) / defense) / 50.0) + 2.0;
        let d = tr_bits(base, 16);
        let d = self.randomizer(d);
        d.max(1.0)
    }
}

fn acc_val(a: Option<f64>) -> Val {
    match a {
        Some(x) => Val::Num(x),
        None => Val::True,
    }
}
fn val_acc(v: Val) -> Option<f64> {
    match v {
        Val::Num(x) => Some(x),
        Val::True => None,
        // false accuracy (never hits) is not used by any effect in the format
        _ => Some(0.0),
    }
}
