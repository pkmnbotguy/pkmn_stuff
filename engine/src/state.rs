//! Battle state: Pokemon, sides, field. Plain data that copies cheaply, so a search can branch.

use crate::damage::{Status, Terrain, Weather};
use crate::dex::{to_id, Boosts, Dex, TypeId, NO_TYPE};
use crate::effects::{Ab, It};
use crate::stats::champions_stats;

pub const NONE: u8 = 255;
/// Mon.trap_src: trapped with no source (No Retreat's 'trapped' volatile)
pub const TRAP_SELF: u8 = 254;

/// Temporary effects on a Pokemon (bits).
pub mod vol {
    pub const PROTECT: u32 = 1;
    pub const FLINCH: u32 = 1 << 1;
    pub const FOLLOW_ME: u32 = 1 << 2;
    pub const RAGE_POWDER: u32 = 1 << 3;
    pub const HELPING_HAND: u32 = 1 << 4;
    pub const ROOST: u32 = 1 << 5;
    pub const ENDURE: u32 = 1 << 6;
    pub const MUST_RECHARGE: u32 = 1 << 7;
    pub const CHARGING: u32 = 1 << 8;
    pub const FLASH_FIRE: u32 = 1 << 9;
    pub const GLAIVE_RUSH: u32 = 1 << 10;
    pub const IMPRISON: u32 = 1 << 11;
    pub const UNBURDEN: u32 = 1 << 12;
    pub const MOVED: u32 = 1 << 13;
    pub const DAMAGED_THIS_TURN: u32 = 1 << 14;
    pub const SWITCHING_IN: u32 = 1 << 15;
    /// a stat went up this turn (Alluring Voice)
    pub const STATS_RAISED: u32 = 1 << 16;
    /// Libero / Protean already changed its type since it switched in
    pub const LIBERO_USED: u32 = 1 << 17;
    /// Charge (Electromorphosis): its next Electric move has double power
    pub const CHARGED: u32 = 1 << 18;
    /// seeded by Leech Seed (the seeder's slot is in `Mon::leech_slot`)
    pub const LEECH_SEED: u32 = 1 << 19;
    /// a stat went down this turn (Lash Out)
    pub const STATS_LOWERED: u32 = 1 << 20;
    /// Focus Energy: critical hit ratio +2
    pub const FOCUS_ENERGY: u32 = 1 << 21;
    /// Dragon Cheer: critical hit ratio +1 (+2 with DRAGON_CHEER2: it was a Dragon when cheered)
    pub const DRAGON_CHEER: u32 = 1 << 22;
    pub const DRAGON_CHEER2: u32 = 1 << 23;
    /// Destiny Bond: if it faints to a foe's move before it moves again, the attacker faints too
    pub const DESTINY_BOND: u32 = 1 << 24;
    /// used Minimize: Body Slam & co. never miss and deal double damage
    pub const MINIMIZE: u32 = 1 << 25;
    /// cursed by a Ghost's Curse: loses 1/4 max HP each turn
    pub const CURSE: u32 = 1 << 26;
    /// No Retreat: can't switch out (and can't use No Retreat again)
    pub const NO_RETREAT: u32 = 1 << 27;
    /// transformed (Transform / Imposter): the original is in `Mon::saved`
    pub const TRANSFORMED: u32 = 1 << 28;
    /// switched in this turn (Showdown's newlySwitched; Payback)
    pub const NEWLY_SWITCHED: u32 = 1 << 29;
}

/// More volatile flags (Mon::vol2).
pub mod vol2 {
    /// infatuated (Cute Charm): source in Mon::attract_src
    pub const ATTRACT: u32 = 1 << 0;
    /// Aqua Ring: heals 1/16 at the end of each turn
    pub const AQUA_RING: u32 = 1 << 1;
    /// Salt Cure: loses 1/16 (Water / Steel: 1/8) at the end of each turn
    pub const SALT_CURE: u32 = 1 << 2;
    /// Electrify: its move this turn is Electric
    pub const ELECTRIFY: u32 = 1 << 3;
    /// Power Trick: Attack and Defense swapped (swapped back when it leaves the field)
    pub const POWER_TRICK: u32 = 1 << 4;
    /// Smack Down / Thousand Arrows: grounded until it leaves the field
    pub const SMACK_DOWN: u32 = 1 << 5;
    /// hit by a damaging move this turn (Focus Punch loses its focus)
    pub const LOST_FOCUS: u32 = 1 << 6;
    /// Beak Blast heating up (from the start of the turn until it moves): contact burns the attacker
    pub const BEAK_BLAST: u32 = 1 << 7;
    /// Torment: can't use the move it used last
    pub const TORMENT: u32 = 1 << 8;
    /// Ingrain: heals 1/16 at the end of each turn, can't switch, grounded, can't be dragged out
    pub const INGRAIN: u32 = 1 << 9;
    /// Gastro Acid: its ability is suppressed (restored when it leaves the field)
    pub const GASTRO: u32 = 1 << 10;
    /// Power Split / Guard Split changed its stats (recomputed when it leaves the field)
    pub const SPLIT: u32 = 1 << 11;
}

/// Which Protect-like move is up (they differ on contact).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ProtectKind {
    #[default]
    Plain,
    SpikyShield,
    KingsShield,
    BanefulBunker,
    SilkTrap,
    BurningBulwark,
    Obstruct,
}

/// What Transform overwrites, restored when the Pokemon leaves the field.
#[derive(Clone, Copy, Debug)]
pub struct Saved {
    /// which Pokemon (index on its side)
    pub mon: u8,
    pub n_moves: u8,
    pub species: u16,
    pub ability_id: u16,
    pub moves: [u16; 4],
    pub pp: [u8; 4],
    /// (2026-10-17) the rest of the original state, for undoing Transform at a faint (no dex there)
    pub stats: [u16; 6],
    pub types: [TypeId; 2],
    pub weight_hg: u32,
    pub ability: Ab,
}

/// (2026-11-05, v3 external belief input) What the OTHER side believes about this Pokemon's set, fed to its network as
/// input (encode.rs encode_bel): kind 0 = no belief, 1 = a guess (e.g. a library set), 2 = exact (an open team sheet, a
/// known random team); ability / item / moves are dex indices (u16::MAX = none), stats the believed stats.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BelSet {
    pub kind: u8,
    pub ability: u16,
    pub item: u16,
    pub moves: [u16; 4],
    pub stats: [u16; 6],
}

// (2026-10-18 speed) Copy: a clone is a plain memory copy (search clones the whole Battle per simulated turn)
#[derive(Clone, Copy, Debug)]
pub struct Mon {
    pub species: u16,
    /// species before Mega Evolution
    pub base_species: u16,
    /// species it becomes when it Mega Evolves (u16::MAX = can't)
    pub mega_species: u16,
    pub types: [TypeId; 2],
    /// (2026-10-16) a third type added by Trick-or-Treat (Ghost) / Forest's Curse (Grass); NO_TYPE = none. Cleared by any
    /// type or species change and when it leaves the field (Showdown: setType / setSpecies reset addedType)
    pub added_type: TypeId,
    pub stats: [u16; 6],
    pub points: [u16; 6],
    pub nature: crate::dex::Nature,
    pub hp: u16,
    pub maxhp: u16,
    pub ability: Ab,
    pub ability_id: u16,
    /// the ability it has when it comes in (Entrainment, Skill Swap, Trace... are undone on switch-out; Mega Evolution is permanent)
    pub base_ability_id: u16,
    pub item: It,
    /// u16::MAX = no item
    pub item_id: u16,
    pub moves: [u16; 4],
    pub pp: [u8; 4],
    pub n_moves: u8,
    pub boosts: Boosts,
    pub status: Status,
    /// sleep / freeze turns left, or the toxic counter
    pub status_turns: u8,
    /// (2026-11-03, leaks) HP / status when it last left the field: what the opponent saw. A benched foe is encoded with
    /// these, so Regenerator / Natural Cure on switch-out don't show (or reveal the ability) before it comes back
    pub bench_seen: bool,
    pub bench_hp: u16,
    pub bench_status: Status,
    /// (2026-11-05) status counter when it last left the field (sleep turns / toxic stage: f[78] / f[79] of a benched foe)
    pub bench_sturns: u8,
    pub vol: u32,
    pub protect_kind: ProtectKind,
    /// consecutive successful Protects (for the 1/3^n chance)
    pub protect_streak: u8,
    /// a successful Wide / Quick Guard this turn (keeps the stall counter at the end of the turn)
    pub guarded: bool,
    pub confusion: u8,
    pub taunt: u8,
    pub encore: u8,
    pub encore_slot: u8,
    pub throat_chop: u8,
    /// Magnet Rise turns left (ungrounded)
    pub magnet_rise: u8,
    pub disable: u8,
    pub disable_slot: u8,
    pub perish: u8,
    /// Choice item lock: move slot (NONE = not locked)
    pub choice_lock: u8,
    /// Outrage / Thrash / Petal Dance / Raging Fury lock (Showdown 'lockedmove'): move slot (NONE = not locked),
    /// Showdown's duration counter and its hidden trueDuration (2-3 turns; confusion when it runs out)
    pub lock_slot: u8,
    pub lock_dur: u8,
    pub lock_true: u8,
    /// Uproar: turns left (0 = none) and the move slot it locks into
    pub uproar: u8,
    pub uproar_slot: u8,
    /// the move a caller (Copycat) started that continues next turn (charge / lock / Uproar slot = battle::CALL_SLOT)
    pub call_move: u16,
    pub last_move: u16,
    /// moves used since switching in (Fake Out works only on the first)
    pub active_move_actions: u8,
    pub weight_hg: u32,
    pub fainted: bool,
    pub is_mega: bool,
    pub sash_ready: bool,
    /// charging a two-turn move: its slot (NONE = not charging) and target
    pub charge_slot: u8,
    pub charge_target: i8,
    /// result of its move this turn / last turn: -1 = didn't use one, 0 = failed, 1 = worked
    /// (Stomping Tantrum, Temper Flare)
    pub move_result: i8,
    pub last_move_result: i8,
    /// (2026-10-15) Showdown's moveThisTurnResult === false: like move_result 0, except a move whose every target blocked it
    /// with Protect / Wide Guard / Quick Guard (NOT_FAIL) did NOT fail. Stomping Tantrum / Temper Flare use this; the
    /// network's history input keeps move_result (blocked = 0, as in training)
    pub move_failed: bool,
    pub last_move_failed: bool,
    /// (2026-10-16) the last item it used up was a Sitrus Berry (Showdown's lastItem; Harvest), kept across switches
    pub last_berry_sitrus: bool,
    /// ate a berry at some point (Belch); the last item it used up (Recycle; u16::MAX = none)
    pub ate_berry: bool,
    /// (2026-10-17) Illusion is disguising it (Showdown pokemon.illusion: set at switch-in when a living party member stands
    /// 'to its right', ended by a damaging hit / fainting); Transform and Imposter fail against (and from) it
    pub illusion: bool,
    /// whom the disguise shows (Showdown pokemon.illusion: the last living party member after it); NONE = none
    pub illusion_as: u8,
    /// (2026-11-05) to the OTHER side it looks fainted: a disguised Zoroark imitating it fainted without a damaging hit
    /// (Showdown never reveals that Zoroark); cleared when it really appears (switch-in while seen)
    pub ghost_faint: bool,
    /// (2026-11-05) the other side's belief about its set (network input only; the game never reads it)
    pub bel: BelSet,
    /// Cud Chew: the berry it will chew again
    pub cud_berry: u16,
    /// Receiver / Power of Alchemy: the fainted ally (index) whose ability it takes at the next update
    pub recv_from: u8,
    /// used an item up this turn (Pickup)
    pub used_item_turn: bool,
    /// Metronome (item): the move it used last and how many times in a row
    /// (2026-10-17) the Metronome item's volatile is on it (Showdown: added by the item's Start at switch-in / when it receives
    /// the item, removed at its next move once the item is gone or suppressed; it does NOT come back when Magic Room ends)
    pub metro_on: bool,
    /// Ripen ate a resist berry outside a hit (Fling, Bug Bite, Cud Chew): its next damaging hit taken is halved (Showdown berryWeaken)
    pub berry_weaken: bool,
    pub metro_move: u16,
    pub metro_n: u8,
    pub last_item: u16,
    /// Lock-On: turns left (2 when used) and who it locked onto (index on the foe side)
    pub lock_on: u8,
    /// Ally Switch's own stall counter (Gen 9: 3, 9, 27...; 0 = no volatile) and the volatile's turns left
    pub ally_sw: u16,
    pub ally_sw_dur: u8,
    pub lock_on_tgt: u8,
    /// Magic Room: the item it holds while the room hides it
    pub stash_item: (u16, It),
    /// Opportunist: foes' stat rises waiting to be copied (after the current action / at the end of the turn)
    pub opp_pending: [i8; 8],
    /// Cud Chew: end-of-turn ticks until it eats its last Sitrus Berry again (0 = nothing pending; reset on switch-out)
    pub cud_chew: u8,
    /// Yawn: turns until it falls asleep (0 = not drowsy)
    pub yawn: u8,
    /// Substitute's HP (0 = no Substitute)
    pub sub_hp: u16,
    /// turns on the field (Showdown's activeTurns: 0 on the turn it switched in)
    pub active_turns: u8,
    /// (2026-10-17) when its ability state was last initialised (switch-in, Transform, Mega): Showdown's abilityState.effectOrder,
    /// which breaks speed ties between Lightning Rod / Storm Drain holders (lower = older = first); 0 = from the starting position
    pub ab_epoch: u32,
    /// (2026-10-17) Trace found nothing to copy yet: it keeps looking at every update point (Showdown effectState.seek)
    pub trace_seek: bool,
    /// bound by Infestation / Whirlpool...: turns left (0 = free)
    pub bound: u8,
    /// residual damage divisor of the binding: 8, or 6 if the binder held a Binding Band when it started
    pub bound_div: u8,
    /// Octolock: index of the foe that locked it (NONE = not locked)
    pub octolock_src: u8,
    /// (2026-10-17) side of the Octolock user (an ally can Octolock its partner)
    pub octolock_side: u8,
    /// (2026-10-16) Spirit Shackle ('trapped' linked to the user): index of the foe that trapped it (NONE = free);
    /// ends when it leaves the field or that foe does
    pub trap_src: u8,
    /// side of trap_src (Mean Look / Block can come from the partner)
    pub trap_side: u8,
    /// who bound it (index on side `bound_side`: usually the other side, an ally's Infestation = its own)
    pub bound_src: u8,
    pub bound_side: u8,
    /// hits taken from other Pokemon's moves since it switched in (Rage Fist)
    pub times_attacked: u8,
    /// (2026-10-20) damaging hits between it and the other side this battle (taken or dealt; never reset): how much an
    /// observer could have inferred about its stats (spread inference); feeds the stat estimate in hidden views
    pub obs_n: u8,
    /// (2026-10-20) an observer's estimate of its stats (HP, Atk, Def, SpA, SpD, Spe; [0] == 0 = none), set from outside
    /// (spread inference in rebuilt positions: Battle.set_stat_belief): hidden / guessed views use it instead of the guess
    pub stat_belief: [u16; 6],
    /// attackers that dealt damage to it this turn, bit side*8+index (Avalanche)
    pub hit_by: u16,
    /// Metal Burst / Counter / Mirror Coat: damage of the last foe hit this turn (any, physical, special) and that foe's slot
    pub retal: [u16; 3],
    pub retal_slot: [u8; 3],
    /// 0 genderless, 1 male, 2 female (3 = not chosen yet: Battle::from_mons draws it)
    pub gender: u8,
    /// more volatile flags (vol2::...)
    pub vol2: u32,
    /// who it is infatuated with (index on the other side)
    pub attract_src: u8,
    /// (2026-10-17) side of the Pokemon that caused the infatuation (an ally can Attract its partner)
    pub attract_side: u8,
    /// Heal Block turns left (Psychic Noise)
    pub heal_block: u8,
    /// Stockpile layers (0-3)
    pub stockpile: u8,
    pub level: u8,
    /// Leech Seed: the opposing slot (0 / 1) that heals from it
    pub leech_slot: u8,
    /// the seeder's side (Leech Seed on an ally heals its own side's Pokemon in that slot)
    pub leech_side: u8,
    /// move slots used since it came in (bits; Last Resort)
    pub used_mask: u8,
    /// Supreme Overlord: fainted allies counted when the ability started (0-5)
    pub fallen: u8,
    /// the target of its last move (Instruct repeats it)
    pub last_target: i8,
    /// Syrup Bomb: turns left (0 = none) and who threw it (index on the other side)
    pub syrup: u8,
    pub syrup_src: u8,
    /// side of syrup_src (Syrup Bomb can hit the thrower's partner)
    pub syrup_side: u8,
    /// Supersweet Syrup already used this battle
    pub syrup_done: bool,
    /// revealed publicly (bits): 0-3 = move slot used, 4 = item shown (consumed, removed, swapped),
    /// 5 = has been on the field. For the hidden-information view (see `rev` module).
    pub rev: u8,
    /// history (network inputs only, never read by the mechanics; all public information):
    /// move slot it used last since switching in (NONE = none yet) and its target (1 / 2 = opposing slot a / b, 3 = ally,
    /// 0 = itself / none / spread),
    /// how many times in a row it used that slot, whether it was a Protect-type move,
    /// and per battle: uses of each move slot, Protect-type moves, other status moves
    pub h_slot: u8,
    pub h_target: i8,
    pub h_streak: u8,
    pub h_protect: bool,
    pub h_uses: [u8; 4],
    pub h_protects: u8,
    pub h_status: u8,
}

/// Bits of `Mon::rev`.
pub mod rev {
    pub const ITEM: u8 = 1 << 4;
    pub const SEEN: u8 = 1 << 5;
    /// ability known (only meaningful with EXPLICIT)
    pub const ABILITY: u8 = 1 << 6;
    /// the bits were set from outside (a real battle's knowledge): item / ability known = their bits only
    pub const EXPLICIT: u8 = 1 << 7;
}

impl Mon {
    #[inline(always)]
    pub fn alive(&self) -> bool {
        !self.fainted && self.hp > 0
    }
    #[inline(always)]
    pub fn has_vol(&self, v: u32) -> bool {
        self.vol & v != 0
    }
    #[inline(always)]
    pub fn has_type(&self, t: TypeId) -> bool {
        t != NO_TYPE && (self.types[0] == t || self.types[1] == t || self.added_type == t)
    }
    pub fn has_item(&self) -> bool {
        self.item_id != u16::MAX
    }
}

/// What Baton Pass hands over (Showdown copyVolatileFrom: boosts + every volatile without noCopy)
#[derive(Clone, Copy, Debug, Default)]
pub struct BatonVol {
    pub boosts: Boosts,
    pub vol: u32,
    pub vol2: u32,
    pub sub_hp: u16,
    pub confusion: u8,
    pub bound: u8,
    pub bound_div: u8,
    pub bound_src: u8,
    pub bound_side: u8,
    pub heal_block: u8,
    pub magnet_rise: u8,
    pub perish: u8,
    pub taunt: u8,
    pub throat_chop: u8,
    pub octolock_src: u8,
    pub octolock_side: u8,
    pub leech_slot: u8,
    pub leech_side: u8,
}

#[derive(Debug, Default)]
pub struct Side {
    /// the four Pokemon brought, in team-preview order
    pub mons: arrayvec::ArrayVec<Mon, 6>,
    /// index into `mons` for each active slot (NONE = empty)
    pub active: [u8; 2],
    pub tailwind: u8,
    pub reflect: u8,
    pub light_screen: u8,
    pub aurora_veil: u8,
    /// (2026-10-28, hidden timers) Reflect / Light Screen / Aurora Veil: setter (as Field::weather_src) and Light Clay (8 turns)
    pub screen_src: [u8; 3],
    pub screen_ext: [bool; 3],
    pub wide_guard: bool,
    pub quick_guard: bool,
    pub mega_used: bool,
    pub fainted_total: u8,
    /// Toxic Spikes layers (0-2)
    pub toxic_spikes: u8,
    /// Baton Pass: stat stages and copyable volatiles waiting for the next Pokemon to switch in
    pub baton: Option<BatonVol>,
    /// Healing Wish waiting in an active slot for a hurt Pokemon to switch in
    pub healing_wish: [bool; 2],
    /// entry hazards on this side
    pub stealth_rock: bool,
    pub spikes: u8,
    /// Shed Tail: a substitute (its HP) waiting for the Pokemon that comes into this slot
    pub shed_sub: [u16; 2],
    /// Safeguard turns left
    pub safeguard: u8,
    pub sticky_web: bool,
    /// Wish per active slot: (end-of-turns until it heals, amount)
    pub wish: [(u8, u16); 2],
    /// Future Sight aimed at this side's slot: (end-of-turns until it hits, user's side, user's index); 0 = none
    pub future: [(u8, u8, u8); 2],
    /// a transformed Pokemon of this side and what it was (one at a time; Transform is rare)
    pub transformed: Option<Saved>,
    /// (2026-10-17) Showdown's side.pokemon array (mon index at each position; positions 0 / 1 = the active slots): a
    /// switch-in swaps the incoming Pokemon with the slot's position, Ally Switch swaps 0 and 1. Beat Up hits in this
    /// order. Identity until `party_set`.
    pub party: [u8; 6],
    pub party_set: bool,
    /// (2026-10-28, leaks) the six species shown at team preview (team order; n_preview = 0: unknown, e.g. rebuilt
    /// positions): closed-sheet search worlds draw from them which Pokemon the opponent's unseen brought ones are
    pub preview: [u16; 6],
    pub n_preview: u8,
}

// (2026-10-18 speed) Side is plain data (Mon, Saved, BatonVol are Copy; ArrayVec<Mon, 6> = an inline array + a length):
// clone it as one memory copy. The derived Clone pushed the six Mons one by one (the search clones the Battle per simulated
// turn). KEEP Side free of heap-owning fields (Vec, String, Box...): a bitwise copy of one would double-free.
impl Clone for Side {
    #[inline]
    fn clone(&self) -> Self {
        // SAFETY: no field owns heap memory or has a meaningful Drop (Mon: Copy, checked below)
        unsafe { std::ptr::read(self) }
    }
}
const _: () = {
    const fn copy<T: Copy>() {}
    copy::<Mon>();
    copy::<Saved>();
    copy::<BatonVol>();
};

impl Side {
    pub fn party_order(&self) -> [u8; 6] {
        if self.party_set { self.party } else { [0, 1, 2, 3, 4, 5] }
    }
    /// the Pokemon `mon` takes array position `pos` (Showdown switchIn / swapPosition)
    pub fn party_put(&mut self, pos: usize, mon: usize) {
        let mut o = self.party_order();
        if let Some(q) = o.iter().position(|&x| x as usize == mon) {
            o.swap(pos, q);
        }
        self.party = o;
        self.party_set = true;
    }
    pub fn active_mon(&self, slot: usize) -> Option<&Mon> {
        let i = self.active[slot];
        if i == NONE { None } else { Some(&self.mons[i as usize]) }
    }
    pub fn bench(&self) -> impl Iterator<Item = usize> + '_ {
        (0..self.mons.len()).filter(move |&i| !self.active.contains(&(i as u8)) && self.mons[i].alive())
    }
    pub fn alive_count(&self) -> usize {
        self.mons.iter().filter(|m| m.alive()).count()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Field {
    pub weather: Weather,
    pub weather_turns: u8,
    pub terrain: Terrain,
    pub terrain_turns: u8,
    pub trick_room: u8,
    /// Gravity turns left
    pub gravity: u8,
    /// Magic Room (held items do nothing), Wonder Room (Defense and Sp. Def swapped), Fairy Lock (nobody can switch): turns left
    pub magic_room: u8,
    pub wonder_room: u8,
    pub fairy_lock: u8,
    /// Cloud Nine / Air Lock were suppressing weather at the last check (their onStart / onEnd fire WeatherChange: Forecast)
    pub wx_supp: bool,
    /// (2026-10-28, hidden timers) who set the weather / terrain (0 = unknown / set from outside, else 1 + side * 8 + index)
    /// and whether its item made it 8 turns (rock / Terrain Extender): an observer who can't see that item doesn't know
    /// the turns left (Battle::timer_hidden, hide_timers)
    pub weather_src: u8,
    pub weather_ext: bool,
    pub terrain_src: u8,
    pub terrain_ext: bool,
}

/// A small fast random generator (xoshiro128++), deterministic per seed.
#[derive(Clone, Copy, Debug)]
pub struct Rng {
    s: [u32; 4],
}

impl Rng {
    pub fn new(seed: u64) -> Rng {
        let mut z = seed.wrapping_add(0x9E3779B97F4A7C15);
        let mut next = || {
            z = z.wrapping_add(0x9E3779B97F4A7C15);
            let mut x = z;
            x = (x ^ (x >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94D049BB133111EB);
            (x ^ (x >> 31)) as u32
        };
        Rng { s: [next(), next(), next(), next() | 1] }
    }
    pub fn next_u32(&mut self) -> u32 {
        let result = self.s[0].wrapping_add(self.s[3]).rotate_left(7).wrapping_add(self.s[0]);
        let t = self.s[1] << 9;
        self.s[2] ^= self.s[0];
        self.s[3] ^= self.s[1];
        self.s[1] ^= self.s[2];
        self.s[0] ^= self.s[3];
        self.s[2] ^= t;
        self.s[3] = self.s[3].rotate_left(11);
        result
    }
    /// uniform in [0, n)
    pub fn below(&mut self, n: u32) -> u32 {
        ((self.next_u32() as u64 * n as u64) >> 32) as u32
    }
    /// true with probability num/den
    pub fn chance(&mut self, num: u32, den: u32) -> bool {
        self.below(den) < num
    }
}

/// A team member as written in a Showdown export.
#[derive(Clone, Debug, Default)]
pub struct SetData {
    pub species: String,
    pub item: String,
    pub ability: String,
    pub nature: String,
    /// "Level: N" (50 if missing)
    pub level: u8,
    pub points: [u16; 6],
    pub moves: Vec<String>,
    /// 0 = not given, 1 = (M), 2 = (F)
    pub gender: u8,
}

/// Parse a Showdown export ("Species @ Item / Ability: / EVs: / Nature / - moves").
pub fn parse_team(text: &str) -> Vec<SetData> {
    let mut out = Vec::new();
    for block in text.replace("\r\n", "\n").split("\n\n") {
        let lines: Vec<&str> = block.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
        if lines.is_empty() {
            continue;
        }
        let mut set = SetData::default();
        set.level = 50;
        let (head, item) = match lines[0].split_once('@') {
            Some((h, i)) => (h.trim(), i.trim()),
            None => (lines[0], ""),
        };
        set.item = item.to_string();
        set.gender = if head.trim_end().ends_with("(M)") { 1 } else if head.trim_end().ends_with("(F)") { 2 } else { 0 };
        let mut name = head.trim_end_matches("(M)").trim_end_matches("(F)").trim().to_string();
        if let (Some(a), Some(b)) = (name.rfind('('), name.rfind(')')) {
            if a < b {
                name = name[a + 1..b].to_string();
            }
        }
        set.species = name;
        for l in &lines[1..] {
            if let Some(m) = l.strip_prefix("- ") {
                set.moves.push(m.trim().to_string());
            } else if let Some(v) = l.strip_prefix("Level:") {
                set.level = v.trim().parse().unwrap_or(50);
            } else if let Some(a) = l.strip_prefix("Ability:") {
                set.ability = a.trim().to_string();
            } else if let Some(n) = l.strip_suffix(" Nature") {
                set.nature = n.trim().to_string();
            } else if let Some(e) = l.strip_prefix("EVs:").or_else(|| l.strip_prefix("SPs:")) {
                for part in e.split('/') {
                    let mut it = part.split_whitespace();
                    if let (Some(n), Some(stat)) = (it.next(), it.next()) {
                        let i = match stat.to_ascii_lowercase().as_str() {
                            "hp" => 0,
                            "atk" => 1,
                            "def" => 2,
                            "spa" => 3,
                            "spd" => 4,
                            "spe" => 5,
                            _ => continue,
                        };
                        set.points[i] = n.parse().unwrap_or(0);
                    }
                }
            }
        }
        out.push(set);
    }
    out
}

/// Build a battle-ready Pokemon from a set. Unknown names are an error.
pub fn make_mon(dex: &Dex, set: &SetData) -> Result<Mon, String> {
    let si = dex.species_index(&set.species).ok_or(format!("unknown species {}", set.species))?;
    let sp = &dex.species[si];
    let nature = dex.nature(&set.nature);
    let stats = champions_stats(&sp.base, &set.points, nature);
    let ability_id = dex.ability_index(&set.ability).ok_or(format!("unknown ability {}", set.ability))?;
    let (item, item_id) = if set.item.is_empty() {
        (It::None, u16::MAX)
    } else {
        let i = dex.item_index(&set.item).ok_or(format!("unknown item {}", set.item))?;
        (dex.items[i].kind, i as u16)
    };
    let mut mega_species = u16::MAX;
    if item_id != u16::MAX {
        for (from, to) in &dex.items[item_id as usize].mega {
            if *from == sp.id {
                if let Some(m) = dex.species_index(to) {
                    mega_species = m as u16;
                }
            }
        }
    }
    // (2026-10-19 bridge fuzz) a set built directly as a Mega (rebuilt positions): its TEAM species is the base forme
    // (Beat Up's power, stone locks) and it is already Mega Evolved
    let built_mega = sp.is_mega;
    let base_si = if built_mega { dex.species_index(&sp.base_species).unwrap_or(si) } else { si };
    let mut moves = [u16::MAX; 4];
    let mut pp = [0u8; 4];
    let mut n = 0;
    for m in set.moves.iter().take(4) {
        let mi = dex.move_index(m).ok_or(format!("unknown move {m}"))?;
        moves[n] = mi as u16;
        pp[n] = dex.moves[mi].max_pp; // Champions: (pp / 5 + 1) * 4 (8 for Protect, 12 for 10-PP moves, 20 max)
        n += 1;
    }
    Ok(Mon {
        species: si as u16,
        base_species: base_si as u16,
        mega_species,
        types: sp.types,
        added_type: NO_TYPE,
        stats,
        points: set.points,
        nature,
        hp: stats[0],
        maxhp: stats[0],
        ability: dex.abilities[ability_id].kind,
        ability_id: ability_id as u16,
        base_ability_id: ability_id as u16,
        item,
        item_id,
        moves,
        pp,
        n_moves: n as u8,
        boosts: [0; 8],
        status: Status::None,
        status_turns: 0,
        bench_seen: false,
        bench_hp: 0,
        bench_status: Status::None,
        bench_sturns: 0,
        vol: 0,
        protect_kind: ProtectKind::Plain,
        protect_streak: 0,
        guarded: false,
        confusion: 0,
        taunt: 0,
        encore: 0,
        encore_slot: NONE,
        throat_chop: 0,
        magnet_rise: 0,
        disable: 0,
        disable_slot: NONE,
        perish: 0,
        choice_lock: NONE,
        lock_slot: NONE,
        lock_dur: 0,
        lock_true: 0,
        uproar: 0,
        uproar_slot: NONE,
        call_move: u16::MAX,
        last_move: u16::MAX,
        active_move_actions: 0,
        weight_hg: sp.weight_hg,
        fainted: false,
        is_mega: built_mega,
        sash_ready: true,
        charge_slot: NONE,
        charge_target: 0,
        move_result: -1,
        last_move_result: -1,
        move_failed: false,
        last_move_failed: false,
        last_berry_sitrus: false,
        ate_berry: false,
        illusion: false,
        illusion_as: NONE,
        ghost_faint: false,
        bel: BelSet::default(),
        cud_berry: u16::MAX,
        recv_from: u8::MAX,
        used_item_turn: false,
        metro_on: false,
        berry_weaken: false,
        metro_move: u16::MAX,
        metro_n: 0,
        last_item: u16::MAX,
        lock_on: 0,
        ally_sw: 0,
        ally_sw_dur: 0,
        lock_on_tgt: NONE,
        stash_item: (u16::MAX, It::None),
        opp_pending: [0; 8],
        cud_chew: 0,
        yawn: 0,
        sub_hp: 0,
        active_turns: 1,
        ab_epoch: 0,
        trace_seek: false,
        bound: 0,
        bound_div: 8,
        octolock_src: NONE,
        octolock_side: 0,
        trap_src: NONE,
        trap_side: 0,
        bound_src: 0,
        bound_side: 0,
        times_attacked: 0,
        obs_n: 0,
        stat_belief: [0; 6],
        hit_by: 0,
        retal: [0; 3],
        retal_slot: [NONE; 3],
        gender: if set.gender != 0 { set.gender } else { sp.gender },
        vol2: 0,
        attract_src: 0,
        attract_side: 0,
        heal_block: 0,
        stockpile: 0,
        level: if set.level == 0 { 50 } else { set.level },
        leech_slot: NONE,
        leech_side: 0,
        used_mask: 0,
        fallen: 0,
        last_target: 0,
        syrup: 0,
        syrup_src: NONE,
        syrup_side: 0,
        syrup_done: false,
        rev: 0,
        h_slot: NONE,
        h_target: 0,
        h_streak: 0,
        h_protect: false,
        h_uses: [0; 4],
        h_protects: 0,
        h_status: 0,
    })
}

pub fn species_id(dex: &Dex, mon: &Mon) -> String {
    to_id(&dex.species[mon.species as usize].name)
}
