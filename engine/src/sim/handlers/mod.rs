//! Effect handlers: the Rust versions of the functions in Showdown's data files (moves,
//! abilities, items, conditions), as the format runs them. `lookup` maps (kind, id, callback
//! name) to the implementation; data.rs attaches them to the handler lists from effects.json.

use super::battle::{Battle, HArgs};
use super::data::{HandlerFn, Kind};
use super::events::Ev;
use super::state::*;
use super::val::{Mon, Val};

pub mod abilities;
pub mod conditions;
pub mod items;
pub mod moves;

pub fn lookup(kind: Kind, id: &str, cb: &str) -> Option<HandlerFn> {
    match kind {
        Kind::Move => moves::lookup(id, cb),
        Kind::Ability => abilities::lookup(id, cb),
        Kind::Item => items::lookup(id, cb),
        Kind::Condition => conditions::lookup(id, cb),
    }
}

/// Call a move's named callback (basePowerCallback, damageCallback...).
fn call_move_cb(b: &mut Battle, mv: u16, cbname: &str, target: Arg, source: Arg) -> Val {
    let eff = b.mv(mv).eff;
    let cb = b.d.cb_id(cbname);
    let imp = b.eff(eff).handler(cb).and_then(|h| h.imp);
    let mref = EffRef { eff, mv, part: 0 };
    let args = HArgs {
        ev: Ev::BasePowerCallback,
        relay: Val::Undef,
        target,
        source,
        effect: mref,
        state: StateLoc::None,
        holder: Holder::None,
        this_eff: eff,
        this_ref: mref,
    };
    match imp {
        Some(f) => f(b, &args),
        None => {
            let msg = format!("{} {}", b.eff(eff).id, cbname);
            if !b.missing.contains(&msg) {
                b.missing.push(msg);
            }
            Val::Undef
        }
    }
}

/// basePowerCallback(pokemon, target, move): target = attacker, source = defender in HArgs
pub fn base_power_callback(b: &mut Battle, source: Mon, target: Mon, mv: u16) -> Val {
    call_move_cb(b, mv, "basePowerCallback", Arg::Mon(source), Arg::Mon(target))
}
/// damageCallback(pokemon, target)
pub fn damage_callback(b: &mut Battle, source: Mon, target: Mon, mv: u16) -> Val {
    call_move_cb(b, mv, "damageCallback", Arg::Mon(source), Arg::Mon(target))
}
/// beforeMoveCallback(pokemon, target, move): true = the move is cancelled
pub fn before_move_callback(b: &mut Battle, m: Mon, target: Option<Mon>, mv: u16) -> bool {
    call_move_cb(b, mv, "beforeMoveCallback", Arg::Mon(m), Arg::from_opt(target)).truthy()
}
/// beforeTurnCallback(pokemon, target)
pub fn before_turn_callback(b: &mut Battle, m: Mon, target: Mon, mv: u16) {
    call_move_cb(b, mv, "beforeTurnCallback", Arg::Mon(m), Arg::Mon(target));
}
/// priorityChargeCallback(pokemon)
pub fn priority_charge_callback(b: &mut Battle, m: Mon, mv: u16) {
    call_move_cb(b, mv, "priorityChargeCallback", Arg::Mon(m), Arg::None);
}

// ----------------------------------------------------------------------
// helpers shared by the handler files
// ----------------------------------------------------------------------
use super::data::{mflag, EffType, MoveTarget, NO_EFF};

pub fn tgt(a: &HArgs) -> Mon {
    a.target.mon().expect("handler target")
}
pub fn src(a: &HArgs) -> Option<Mon> {
    a.source.mon()
}
/// the active move passed as the handler's effect argument
pub fn amv(a: &HArgs) -> Option<u16> {
    if a.effect.is_move() { Some(a.effect.mv) } else { None }
}
pub fn holder(a: &HArgs) -> Mon {
    match a.holder {
        Holder::Mon(m) => m,
        _ => tgt(a),
    }
}

impl Battle {
    /// effectType of an effect argument
    pub fn etype(&self, e: EffRef) -> EffType {
        // a move's secondary / self effect is a plain object: no effectType, id or name
        if e.is_none() || e.part != 0 {
            return EffType::Other;
        }
        if e.is_move() {
            return EffType::Move;
        }
        self.eff(e.eff).effect_type
    }
    pub fn eid(&self, e: EffRef) -> &'static str {
        if e.is_none() || e.part != 0 {
            return "";
        }
        let id: &'static str = &self.d.effects[e.eff as usize].id;
        if id.starts_with("synchronize:") { "synchronize" } else { id }
    }
    pub fn ename(&self, e: EffRef) -> &'static str {
        if e.is_none() || e.part != 0 { "" } else { &self.d.effects[e.eff as usize].name }
    }
    /// checkMoveBypassesProtect(move, attacker, defender): true = the move goes through
    pub fn check_move_bypasses_protect(&mut self, mv: u16, attacker: Mon, defender: Mon) -> bool {
        self.check_move_bypasses_protect_bs(mv, attacker, defender, true)
    }
    pub fn check_move_bypasses_protect_bs(&mut self, mv: u16, attacker: Mon, defender: Mon, block_status: bool) -> bool {
        let m = self.mv(mv);
        if (m.category != super::data::Category::Status || block_status) && m.flags & mflag::PROTECT != 0 {
            let r = self.run_event(Ev::HitProtect, Arg::Mon(attacker), Some(defender), EffRef { eff: m.eff, mv, part: 0 }, Val::Undef);
            if r.truthy() {
                return false;
            }
        }
        true
    }
    /// checkMoveMakesContact(move, attacker, defender, announcePads)
    pub fn check_move_makes_contact(&mut self, mv: u16, attacker: Mon, defender: Mon, announce: bool) -> bool {
        if self.mv(mv).flags & mflag::CONTACT != 0 && self.has_item(attacker, "protectivepads") {
            if announce {
                let full = self.eff(self.effect.eff).fullname.clone();
                crate::add!(self, "-activate", defender, full);
                crate::add!(self, "-activate", attacker, "item: Protective Pads");
            }
            return false;
        }
        self.mv(mv).flags & mflag::CONTACT != 0
    }
    pub fn valid_target(&self, target: Mon, source: Mon, t: MoveTarget) -> bool {
        self.valid_target_loc(self.get_loc_of(source, target), source, t)
    }
    /// target.getMoveHitData(move)
    pub fn hit_data(&self, mv: u16, target: Mon) -> super::state::MoveHitData {
        let pos = self.p(target).position as usize;
        self.mv(mv).move_hit_data[target.s as usize][pos.min(1)]
    }
    pub fn hit_data_mut(&mut self, mv: u16, target: Mon) -> &mut super::state::MoveHitData {
        let pos = self.p(target).position as usize;
        &mut self.mvm(mv).move_hit_data[target.s as usize][pos.min(1)]
    }
    /// delete pokemon.volatiles[id] (no End event)
    pub fn delete_volatile(&mut self, m: Mon, id: &str) {
        if let Some(e) = self.d.cond(id) {
            self.pm(m).volatiles.retain(|x| x.0 != e);
        }
    }
    pub fn state_duration(&self, loc: StateLoc) -> Option<f64> {
        self.state(loc).and_then(|s| s.duration)
    }
    pub fn set_state_duration(&mut self, loc: StateLoc, d: Option<f64>) {
        if let Some(s) = self.state_mut(loc) {
            s.duration = d;
        }
    }
    pub fn state_source(&self, loc: StateLoc) -> Option<Mon> {
        self.state(loc).and_then(|s| s.source)
    }
    pub fn boosts1(stat: u8, v: i8) -> super::data::SparseBoosts {
        let mut b = super::data::SparseBoosts::default();
        b.set(stat, v);
        b
    }
    pub fn no_eff() -> u16 {
        NO_EFF
    }
}
