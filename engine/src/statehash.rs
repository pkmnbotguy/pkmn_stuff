//! (2026-11-02, endgame solver option `fastkey`) a direct 128-bit hash of the engine state, field by field, instead of
//! hashing two network encodings. Knowledge-only fields (Mon.rev, obs_n, stat_belief) and the random-number state are
//! left out; everything else that can change play is included. The Mon / Side / Field impls are generated from
//! state.rs (every field listed): a new state field must be added here too, or positions differing only in it share keys.

use crate::damage::{Status, Terrain, Weather};
use crate::effects::{Ab, It};
use crate::state::*;

/// two multiplicative lanes over 64-bit words; bytes are packed into words before mixing
pub struct H {
    acc: u64,
    n: u32,
    h1: u64,
    h2: u64,
}

impl H {
    pub fn new() -> Self {
        H { acc: 0, n: 0, h1: 0xcbf2_9ce4_8422_2325, h2: 0x8422_2325_cbf2_9ce4 }
    }
    #[inline]
    fn mix(&mut self, x: u64) {
        self.h1 = (self.h1 ^ x).wrapping_mul(0x0000_0100_0000_01B3).rotate_left(29);
        self.h2 = (self.h2 ^ x.rotate_left(17)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
    #[inline]
    pub fn bits(&mut self, x: u64, nbytes: u32) {
        if self.n + nbytes > 8 {
            let a = self.acc;
            self.mix(a);
            self.acc = 0;
            self.n = 0;
        }
        self.acc |= x << (8 * self.n);
        self.n += nbytes;
    }
    pub fn finish(mut self) -> u128 {
        let a = self.acc;
        self.mix(a ^ 0xA5A5_0000_0000_0000);
        self.mix(self.h1 ^ self.h2.rotate_left(31));
        ((self.h1 as u128) << 64) | self.h2 as u128
    }
}

impl Default for H {
    fn default() -> Self {
        H::new()
    }
}

pub trait HB {
    fn hb(&self, h: &mut H);
}

macro_rules! hb_int {
    ($($t:ty => $n:expr),*) => { $(impl HB for $t { #[inline] fn hb(&self, h: &mut H) { h.bits(*self as u64 & (u64::MAX >> (64 - 8 * $n)), $n); } })* };
}
hb_int!(u8 => 1, i8 => 1, u16 => 2, i16 => 2, u32 => 4, i32 => 4, u64 => 8, usize => 8);

impl HB for bool {
    #[inline]
    fn hb(&self, h: &mut H) {
        h.bits(*self as u64, 1);
    }
}

macro_rules! hb_enum {
    ($($t:ty),*) => { $(impl HB for $t { #[inline] fn hb(&self, h: &mut H) { h.bits(*self as u64 & 0xffff, 2); } })* };
}
hb_enum!(Ab, Status, Weather, Terrain, ProtectKind);

impl std::hash::Hasher for H {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.bits(b as u64, 1);
        }
    }
    fn write_u64(&mut self, x: u64) {
        self.bits(x, 8);
    }
    fn write_isize(&mut self, x: isize) {
        self.bits(x as u64, 8);
    }
    fn finish(&self) -> u64 {
        0
    }
}

/// It carries a type name for type-boost items / resist berries: discriminant + that name
impl HB for It {
    fn hb(&self, h: &mut H) {
        std::hash::Hash::hash(&std::mem::discriminant(self), h);
        if let It::TypeBoost(t) | It::ResistBerry(t) = self {
            std::hash::Hasher::write(h, t.as_bytes());
        }
    }
}

impl<T: HB, const N: usize> HB for [T; N] {
    #[inline]
    fn hb(&self, h: &mut H) {
        for x in self.iter() {
            x.hb(h);
        }
    }
}

impl<T: HB> HB for Option<T> {
    #[inline]
    fn hb(&self, h: &mut H) {
        match self {
            None => h.bits(0, 1),
            Some(x) => {
                h.bits(1, 1);
                x.hb(h);
            }
        }
    }
}

impl<A: HB, B: HB> HB for (A, B) {
    #[inline]
    fn hb(&self, h: &mut H) {
        self.0.hb(h);
        self.1.hb(h);
    }
}

impl<A: HB, B: HB, C: HB> HB for (A, B, C) {
    #[inline]
    fn hb(&self, h: &mut H) {
        self.0.hb(h);
        self.1.hb(h);
        self.2.hb(h);
    }
}

impl<T: HB, const N: usize> HB for arrayvec::ArrayVec<T, N> {
    #[inline]
    fn hb(&self, h: &mut H) {
        h.bits(self.len() as u64, 1);
        for x in self.iter() {
            x.hb(h);
        }
    }
}

impl HB for crate::dex::Nature {
    fn hb(&self, h: &mut H) {
        self.plus.hb(h);
        self.minus.hb(h);
    }
}

impl HB for Saved {
    fn hb(&self, h: &mut H) {
        self.mon.hb(h);
        self.n_moves.hb(h);
        self.species.hb(h);
        self.ability_id.hb(h);
        self.moves.hb(h);
        self.pp.hb(h);
        self.stats.hb(h);
        self.types.hb(h);
        self.weight_hg.hb(h);
        self.ability.hb(h);
    }
}

impl HB for Mon {
    fn hb(&self, h: &mut H) {
        self.species.hb(h);
        self.base_species.hb(h);
        self.mega_species.hb(h);
        self.types.hb(h);
        self.added_type.hb(h);
        self.stats.hb(h);
        self.points.hb(h);
        self.nature.hb(h);
        self.hp.hb(h);
        self.maxhp.hb(h);
        self.ability.hb(h);
        self.ability_id.hb(h);
        self.base_ability_id.hb(h);
        self.item.hb(h);
        self.item_id.hb(h);
        self.moves.hb(h);
        self.pp.hb(h);
        self.n_moves.hb(h);
        self.boosts.hb(h);
        self.status.hb(h);
        self.status_turns.hb(h);
        self.vol.hb(h);
        self.protect_kind.hb(h);
        self.protect_streak.hb(h);
        self.guarded.hb(h);
        self.confusion.hb(h);
        self.taunt.hb(h);
        self.encore.hb(h);
        self.encore_slot.hb(h);
        self.throat_chop.hb(h);
        self.magnet_rise.hb(h);
        self.disable.hb(h);
        self.disable_slot.hb(h);
        self.perish.hb(h);
        self.choice_lock.hb(h);
        self.lock_slot.hb(h);
        self.lock_dur.hb(h);
        self.lock_true.hb(h);
        self.uproar.hb(h);
        self.uproar_slot.hb(h);
        self.call_move.hb(h);
        self.last_move.hb(h);
        self.active_move_actions.hb(h);
        self.weight_hg.hb(h);
        self.fainted.hb(h);
        self.is_mega.hb(h);
        self.sash_ready.hb(h);
        self.charge_slot.hb(h);
        self.charge_target.hb(h);
        self.move_result.hb(h);
        self.last_move_result.hb(h);
        self.move_failed.hb(h);
        self.last_move_failed.hb(h);
        self.last_berry_sitrus.hb(h);
        self.ate_berry.hb(h);
        self.illusion.hb(h);
        self.illusion_as.hb(h);
        self.cud_berry.hb(h);
        self.recv_from.hb(h);
        self.used_item_turn.hb(h);
        self.metro_on.hb(h);
        self.berry_weaken.hb(h);
        self.metro_move.hb(h);
        self.metro_n.hb(h);
        self.last_item.hb(h);
        self.lock_on.hb(h);
        self.ally_sw.hb(h);
        self.ally_sw_dur.hb(h);
        self.lock_on_tgt.hb(h);
        self.stash_item.hb(h);
        self.opp_pending.hb(h);
        self.cud_chew.hb(h);
        self.yawn.hb(h);
        self.sub_hp.hb(h);
        self.active_turns.hb(h);
        self.ab_epoch.hb(h);
        self.trace_seek.hb(h);
        self.bound.hb(h);
        self.bound_div.hb(h);
        self.octolock_src.hb(h);
        self.octolock_side.hb(h);
        self.trap_src.hb(h);
        self.trap_side.hb(h);
        self.bound_src.hb(h);
        self.bound_side.hb(h);
        self.times_attacked.hb(h);
        self.hit_by.hb(h);
        self.retal.hb(h);
        self.retal_slot.hb(h);
        self.gender.hb(h);
        self.vol2.hb(h);
        self.attract_src.hb(h);
        self.attract_side.hb(h);
        self.heal_block.hb(h);
        self.stockpile.hb(h);
        self.level.hb(h);
        self.leech_slot.hb(h);
        self.leech_side.hb(h);
        self.used_mask.hb(h);
        self.fallen.hb(h);
        self.last_target.hb(h);
        self.syrup.hb(h);
        self.syrup_src.hb(h);
        self.syrup_side.hb(h);
        self.syrup_done.hb(h);
        self.h_slot.hb(h);
        self.h_target.hb(h);
        self.h_streak.hb(h);
        self.h_protect.hb(h);
        self.h_uses.hb(h);
        self.h_protects.hb(h);
        self.h_status.hb(h);
    }
}

impl HB for BatonVol {
    fn hb(&self, h: &mut H) {
        self.boosts.hb(h);
        self.vol.hb(h);
        self.vol2.hb(h);
        self.sub_hp.hb(h);
        self.confusion.hb(h);
        self.bound.hb(h);
        self.bound_div.hb(h);
        self.bound_src.hb(h);
        self.bound_side.hb(h);
        self.heal_block.hb(h);
        self.magnet_rise.hb(h);
        self.perish.hb(h);
        self.taunt.hb(h);
        self.throat_chop.hb(h);
        self.octolock_src.hb(h);
        self.octolock_side.hb(h);
        self.leech_slot.hb(h);
        self.leech_side.hb(h);
    }
}

impl HB for Side {
    fn hb(&self, h: &mut H) {
        self.mons.hb(h);
        self.active.hb(h);
        self.tailwind.hb(h);
        self.reflect.hb(h);
        self.light_screen.hb(h);
        self.aurora_veil.hb(h);
        self.screen_src.hb(h);
        self.screen_ext.hb(h);
        self.wide_guard.hb(h);
        self.quick_guard.hb(h);
        self.mega_used.hb(h);
        self.fainted_total.hb(h);
        self.toxic_spikes.hb(h);
        self.baton.hb(h);
        self.healing_wish.hb(h);
        self.stealth_rock.hb(h);
        self.spikes.hb(h);
        self.shed_sub.hb(h);
        self.safeguard.hb(h);
        self.sticky_web.hb(h);
        self.wish.hb(h);
        self.future.hb(h);
        self.transformed.hb(h);
        self.party.hb(h);
        self.party_set.hb(h);
        self.preview.hb(h);
        self.n_preview.hb(h);
    }
}

impl HB for Field {
    fn hb(&self, h: &mut H) {
        self.weather.hb(h);
        self.weather_turns.hb(h);
        self.terrain.hb(h);
        self.terrain_turns.hb(h);
        self.trick_room.hb(h);
        self.gravity.hb(h);
        self.magic_room.hb(h);
        self.wonder_room.hb(h);
        self.fairy_lock.hb(h);
        self.wx_supp.hb(h);
        self.weather_src.hb(h);
        self.weather_ext.hb(h);
        self.terrain_src.hb(h);
        self.terrain_ext.hb(h);
    }
}
