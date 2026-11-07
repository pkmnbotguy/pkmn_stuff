//! Gen 9 damage, ported from Showdown's `getDamage` / `modifyDamage` with the same integer
//! rounding. Modifiers go through "events" exactly like Showdown's `runEvent`: handlers run in
//! priority order; each either chains a 4096-based factor (applied once at the end, rounded), or
//! changes the value directly on the spot.

use crate::dex::{Category, Dex, FixedDamage, MoveData, Target, TypeId, ATK, DEF, NO_TYPE, SPA, SPD, flag};
use crate::effects::{Ab, It, Mv};
use crate::stats::boosted;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Weather {
    #[default]
    None,
    Rain,
    Sun,
    Sand,
    Snow,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Terrain {
    #[default]
    None,
    Electric,
    Grassy,
    Psychic,
    Misty,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    None,
    Burn,
    Paralysis,
    Poison,
    Toxic,
    Sleep,
    Freeze,
}

/// What the damage formula needs to know about one Pokemon.
#[derive(Clone, Debug)]
pub struct Combatant {
    pub types: [TypeId; 2],
    /// Trick-or-Treat / Forest's Curse (NO_TYPE = none)
    pub added_type: TypeId,
    /// Ripen's berryWeaken (a resist berry eaten outside a hit): this hit is halved
    pub berry_weaken: bool,
    /// hp, atk, def, spa, spd, spe (unboosted)
    pub stats: [u16; 6],
    pub hp: u16,
    pub maxhp: u16,
    pub ability: Ab,
    pub item: It,
    /// The held item can be knocked off (false for none, or a Mega Stone on its own Pokemon)
    pub item_removable: bool,
    pub has_item: bool,
    /// atk, def, spa, spd, spe stages (index 1..=5 like the stats; index 0 unused)
    pub boosts: [i8; 6],
    pub status: Status,
    pub weight_hg: u32,
    pub is_pikachu: bool,
    pub flash_fire: bool,
    /// Mimikyu with its Disguise up: every hit counts as neutral
    pub disguised: bool,
    /// used Glaive Rush and hasn't moved since: takes double damage
    pub glaive_rush: bool,
    /// its move last turn failed (Stomping Tantrum, Temper Flare)
    pub last_move_failed: bool,
    /// lost HP this turn (Assurance)
    pub hurt_this_turn: bool,
    /// Charge: Electric moves have double power
    pub charged: bool,
    /// Metronome (item): consecutive uses of the same move so far
    pub metro: u8,
    /// in the middle of Dig (1), Dive (2) or Fly / Bounce (3): some moves hit it for double
    pub semi: u8,
    /// times hit by other Pokemon since switching in (Rage Fist)
    pub times_attacked: u8,
    /// Gravity: grounded whatever its type / ability
    pub forced_grounded: bool,
    /// Magnet Rise: in the air
    pub airborne: bool,
    /// one of its stats went down this turn (Lash Out)
    pub stats_lowered: bool,
    pub level: u8,
    /// 0 genderless, 1 male, 2 female
    pub gender: u8,
    /// damage taken this turn from foes: last hit of any kind, last physical, last special (Metal Burst, Counter, Mirror Coat)
    pub retal: [u16; 3],
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Conditions {
    pub weather: Weather,
    pub terrain: Terrain,
    pub spread: bool,
    pub helping_hand: bool,
    /// Round moved up by an earlier Round this turn: double power
    pub round_boost: bool,
    pub reflect: bool,
    pub light_screen: bool,
    pub aurora_veil: bool,
    /// The defender's partner has Friend Guard
    pub friend_guard: bool,
    /// Stakeout: the defender switched in this turn
    pub stakeout: bool,
    /// Wonder Room: the defender's Defense and Sp. Def values are swapped (the boosts stay with the name)
    pub wonder_room: bool,
    /// Someone on the field has Fairy Aura / Dark Aura (other than the attacker, who is checked directly)
    pub fairy_aura: bool,
    pub dark_aura: bool,
    /// The attacker's partner has Steely Spirit
    pub steely_spirit: bool,
    /// Plus / Minus with a Plus / Minus partner: Sp. Atk x1.5
    pub plus_minus: bool,
    /// The defender's foes include an Unnerve / As One user: it can't eat berries
    pub berries_blocked: bool,
    /// Pokemon fainted so far on the attacker's side (Last Respects)
    pub fainted_on_attacker_side: u8,
    /// Supreme Overlord: fallen allies counted when the ability started
    pub supreme_fallen: u8,
    /// Champions: a contact move through Protect (Unseen Fist / Piercing Drill) deals 1/4
    pub protect_bypass: bool,
    /// Fickle Beam's 30% double power came up
    pub fickle: bool,
    /// Analytic: the attacker moves last this turn
    pub analytic: bool,
    /// the defender used Minimize
    pub minimized: bool,
    /// the attacker is Morpeko-Hangry (Aura Wheel turns Dark)
    pub hangry: bool,
    /// a type the move takes from its user's species (Raging Bull of Paldean Tauros)
    pub species_move_type: Option<TypeId>,
    /// Fling's power (the flung item)
    pub fling_bp: u16,
    /// the attacker's positive accuracy + evasion stages (Stored Power / Power Trip count them; Combatant.boosts has none)
    pub acc_eva_boosts: u8,
    /// a base power computed from the battle state (Payback, Gyro Ball, Reversal...); 0 = none
    pub bp_override: u16,
    /// Beat Up: this hit's base power (5 + the party member's base Attack / 10)
    pub beat_up_bp: u16,
    /// a Gem boosts this move (x5325/4096 base power)
    pub gem: bool,
    /// Parental Bond's second hit (x0.25 damage)
    pub parental_second: bool,
}

/// The move after Showdown's ModifyType / ModifyMove steps.
#[derive(Clone, Copy, Debug)]
pub struct ResolvedMove {
    pub type_: TypeId,
    pub category: Category,
    pub base_power: u32,
    pub flags: u32,
    pub type_changed_by_ability: bool,
    pub sheer_force: bool,
    pub ignore_immunity_normal_fighting: bool,
    pub target: Target,
    /// Infiltrator: screens don't apply
    pub infiltrates: bool,
}

// ---------------------------------------------------------------------------
// Showdown's arithmetic
// ---------------------------------------------------------------------------

/// `battle.modify(value, mod/4096)`
#[inline]
pub fn modify(value: i64, m4096: i64) -> i64 {
    (value * m4096 + 2048 - 1) / 4096
}

/// `battle.chainModify`: combine two 4096-based factors
#[inline]
fn chain(prev: i64, next: i64) -> i64 {
    (prev * next + 2048) >> 12
}

/// One event: (priority, kind). Handlers run highest priority first.
#[derive(Clone, Copy)]
enum H {
    Chain(i64),
    Direct(i64),
}

/// The handlers of one event, on the stack (no allocation): (priority, handler).
struct Event {
    handlers: [(i32, H); 16],
    n: usize,
}

impl Event {
    fn new() -> Event {
        Event { handlers: [(0, H::Chain(4096)); 16], n: 0 }
    }
    fn chain(&mut self, priority: i32, m: i64) {
        if self.n < 16 {
            self.handlers[self.n] = (priority, H::Chain(m));
            self.n += 1;
        }
    }
    fn direct(&mut self, priority: i32, m: i64) {
        if self.n < 16 {
            self.handlers[self.n] = (priority, H::Direct(m));
            self.n += 1;
        }
    }
    /// stable insertion sort by priority, highest first (equal priorities keep insertion order)
    fn sorted(&mut self) {
        let h = &mut self.handlers[..self.n];
        for i in 1..h.len() {
            let mut j = i;
            while j > 0 && h[j - 1].0 < h[j].0 {
                h.swap(j - 1, j);
                j -= 1;
            }
        }
    }
    fn run(mut self, mut value: i64) -> i64 {
        self.sorted();
        let mut m = 4096i64;
        for &(_, h) in &self.handlers[..self.n] {
            match h {
                H::Chain(x) => m = chain(m, x),
                H::Direct(x) => value = modify(value, x),
            }
        }
        if m != 4096 { modify(value, m) } else { value }
    }
    /// The chained factor alone (all handlers are chains): apply with `modify` later.
    fn chained_factor(mut self) -> i64 {
        self.sorted();
        let mut m = 4096i64;
        for &(_, h) in &self.handlers[..self.n] {
            if let H::Chain(x) = h {
                m = chain(m, x);
            }
        }
        m
    }
}

const X1_5: i64 = 6144;
const X0_5: i64 = 2048;
const X2: i64 = 8192;
const X0_75: i64 = 3072;

// ---------------------------------------------------------------------------

fn has_type(c: &Combatant, t: TypeId) -> bool {
    t != NO_TYPE && (c.types[0] == t || c.types[1] == t || c.added_type == t)
}

pub fn effective_weather(c: &Combatant, cond: &Conditions) -> Weather {
    effective_weather_for(c, cond, false)
}

/// `mega_sol_attack`: the Pokemon using the move has Mega Sol, so everyone sees sun for it.
pub fn effective_weather_for(c: &Combatant, cond: &Conditions, mega_sol_attack: bool) -> Weather {
    if mega_sol_attack {
        return Weather::Sun;
    }
    if c.item == It::UtilityUmbrella && matches!(cond.weather, Weather::Rain | Weather::Sun) {
        return Weather::None;
    }
    cond.weather
}

pub fn is_grounded(dex: &Dex, c: &Combatant) -> bool {
    if c.item == It::IronBall || c.forced_grounded {
        return true;
    }
    !(c.airborne || has_type(c, dex.ty.flying) || matches!(c.ability, Ab::Levitate | Ab::Eelevate) || c.item == It::AirBalloon)
}

fn type_named(dex: &Dex, t: TypeId, name: &str) -> bool {
    t != NO_TYPE && dex.type_names[t as usize] == name
}

/// Showdown's ModifyType / ModifyMove steps for one move used by `att`.
pub fn resolve_move(dex: &Dex, mv: &MoveData, att: &Combatant, cond: &Conditions) -> ResolvedMove {
    let mut r = ResolvedMove {
        type_: mv.type_,
        category: mv.category,
        base_power: mv.base_power as u32,
        flags: mv.flags,
        type_changed_by_ability: false,
        sheer_force: false,
        ignore_immunity_normal_fighting: false,
        target: mv.target,
        infiltrates: att.ability == Ab::Infiltrator,
    };
    let grounded = is_grounded(dex, att);
    // the move's own ModifyType / ModifyMove
    match mv.kind {
        Mv::Aurawheel if cond.hangry => r.type_ = dex.ty.dark,
        Mv::Ragingbull if cond.species_move_type.is_some() => r.type_ = cond.species_move_type.unwrap(),
        Mv::WeatherBall => {
            let w = effective_weather_for(att, cond, att.ability == Ab::MegaSol);
            let t = match w {
                Weather::Sun => Some("Fire"),
                Weather::Rain => Some("Water"),
                Weather::Sand => Some("Rock"),
                Weather::Snow => Some("Ice"),
                Weather::None => None,
            };
            if let Some(t) = t {
                r.type_ = dex.type_id(t);
                r.base_power *= 2;
            }
        }
        Mv::TerrainPulse => {
            if grounded {
                let t = match cond.terrain {
                    Terrain::Electric => Some("Electric"),
                    Terrain::Grassy => Some("Grass"),
                    Terrain::Misty => Some("Fairy"),
                    Terrain::Psychic => Some("Psychic"),
                    Terrain::None => None,
                };
                if let Some(t) = t {
                    r.type_ = dex.type_id(t);
                    r.base_power *= 2;
                }
            }
        }
        Mv::ExpandingForce => {
            if cond.terrain == Terrain::Psychic && grounded {
                r.target = Target::AllAdjacentFoes;
            }
        }
        _ => {}
    }
    // abilities' ModifyType
    let normal = dex.ty.normal;
    let ate = match att.ability {
        Ab::Aerilate => Some("Flying"),
        Ab::Pixilate => Some("Fairy"),
        Ab::Refrigerate => Some("Ice"),
        Ab::Galvanize => Some("Electric"),
        Ab::Dragonize => Some("Dragon"),
        _ => None,
    };
    if let Some(t) = ate {
        // (2026-10-18 speed: the name checks only when an -ate ability is in play)
        let skip_ate = matches!(mv.kind, Mv::WeatherBall | Mv::TerrainPulse)
            || matches!(mv.id.as_str(), "judgment" | "multiattack" | "naturalgift" | "revelationdance" | "technoblast");
        if r.type_ == normal && !skip_ate {
            r.type_ = dex.type_id(t);
            r.type_changed_by_ability = true;
        }
    }
    if att.ability == Ab::LiquidVoice && mv.flags & flag::SOUND != 0 {
        r.type_ = dex.ty.water;
    }
    // abilities' / items' ModifyMove
    if att.ability == Ab::SheerForce && mv.has_secondaries && !mv.has_sheer_force_boost {
        r.sheer_force = true;
    }
    if matches!(att.ability, Ab::Scrappy | Ab::MindsEye) {
        r.ignore_immunity_normal_fighting = true;
    }
    if (att.item == It::PunchingGlove && r.flags & flag::PUNCH != 0) || att.ability == Ab::Longreach {
        r.flags &= !flag::CONTACT;
    }
    // Electrify (onModifyType priority -2): after every other type change
    if let Some(t) = cond.species_move_type {
        r.type_ = t;
    }
    // Struggle (its onModifyMove, after every ModifyType): typeless = no STAB, neutral against every type
    if mv.kind == Mv::Struggle {
        r.type_ = NO_TYPE;
    }
    r
}

/// Type effectiveness as Showdown's typeMod (+1 per super effective, -1 per resisted),
/// or None if the defender is immune.
pub fn effectiveness(dex: &Dex, mv: &MoveData, r: &ResolvedMove, def: &Combatant) -> Option<i32> {
    let ground = dex.ty.ground;
    // runImmunity: type immunity (Scrappy lets Normal/Fighting hit Ghost), and ungrounded vs Ground
    if r.type_ == ground && !is_grounded(dex, def) {
        return None;
    }
    // Iron Ball's onEffectiveness returns 0 for EVERY type of a Flying holder (Showdown: Steel/Flying takes neutral Ground damage)
    if r.type_ == ground && def.item == It::IronBall && !def.forced_grounded && has_type(def, dex.ty.flying) {
        return Some(0);
    }
    let mut total = 0;
    for &dt in def.types.iter().chain(std::iter::once(&def.added_type)) {
        if dt == NO_TYPE {
            continue;
        }
        let code = dex.type_code(r.type_, dt);
        if code == 3 {
            let ignored = r.ignore_immunity_normal_fighting
                && (type_named(dex, r.type_, "Normal") || type_named(dex, r.type_, "Fighting"));
            let iron_ball_flying = r.type_ == ground && (def.item == It::IronBall || def.forced_grounded);
            if !ignored && !iron_ball_flying {
                return None;
            }
            continue;
        }
        if mv.kind == Mv::FreezeDry && type_named(dex, dt, "Water") {
            total += 1;
            continue;
        }
        if mv.kind == Mv::FlyingPress {
            // onEffectiveness: typeMod + the Flying type's effectiveness against this type
            total += match dex.type_code(dex.ty.flying, dt) {
                1 => 1,
                2 => -1,
                _ => 0,
            };
        }
        total += match code {
            1 => 1,
            2 => -1,
            _ => 0,
        };
    }
    Some(total)
}

fn base_power(dex: &Dex, mv: &MoveData, r: &ResolvedMove, att: &Combatant, def: &Combatant, cond: &Conditions) -> i64 {
    let weight_bp = |w: u32| -> i64 {
        match w {
            w if w >= 2000 => 120,
            w if w >= 1000 => 100,
            w if w >= 500 => 80,
            w if w >= 250 => 60,
            w if w >= 100 => 40,
            _ => 20,
        }
    };
    let bp = match mv.kind {
        _ if cond.bp_override > 0 => cond.bp_override as i64,
        Mv::LowKick | Mv::GrassKnot => weight_bp(def.weight_hg),
        Mv::HeavySlam | Mv::HeatCrash => {
            let (a, d) = (att.weight_hg, def.weight_hg);
            if a >= d * 5 { 120 } else if a >= d * 4 { 100 } else if a >= d * 3 { 80 } else if a >= d * 2 { 60 } else { 40 }
        }
        Mv::Eruption | Mv::WaterSpout => {
            // Showdown: basePower * hp / maxhp in floating point, then floored by clampIntRange
            ((r.base_power as f64) * (att.hp as f64) / (att.maxhp as f64)).floor() as i64
        }
        Mv::Acrobatics => {
            if att.has_item { r.base_power as i64 } else { r.base_power as i64 * 2 }
        }
        Mv::TripleAxel => 20,
        Mv::PowerTrip => {
            r.base_power as i64 + 20 * (att.boosts.iter().skip(1).filter(|&&b| b > 0).map(|&b| b as i64).sum::<i64>() + cond.acc_eva_boosts as i64)
        }
        Mv::Hex => {
            if def.status != Status::None { r.base_power as i64 * 2 } else { r.base_power as i64 }
        }
        Mv::LastRespects => 50 + 50 * cond.fainted_on_attacker_side as i64,
        Mv::Hardpress => {
            let x = def.hp as i64 * 4096 / (def.maxhp as i64).max(1);
            ((100 * (100 * x) + 2048 - 1) / 4096 / 100).max(1)
        }
        Mv::Fling => cond.fling_bp as i64,
        Mv::StompingTantrum => {
            if att.last_move_failed { r.base_power as i64 * 2 } else { r.base_power as i64 }
        }
        Mv::RageFist => (50 + 50 * att.times_attacked as i64).min(350),
        Mv::Round => if cond.round_boost { r.base_power as i64 * 2 } else { r.base_power as i64 },
        Mv::Assurance => {
            if def.hurt_this_turn { r.base_power as i64 * 2 } else { r.base_power as i64 }
        }
        Mv::RisingVoltage => {
            if cond.terrain == Terrain::Electric && is_grounded(dex, def) {
                r.base_power as i64 * 2
            } else {
                r.base_power as i64
            }
        }
        _ => r.base_power as i64,
    };
    bp.max(1)
}

/// All 16 damage rolls (highest first, like Showdown's random(16) = 0..15), or None if immune.
/// No critical hits.
pub fn damage_rolls(dex: &Dex, mv: &MoveData, att: &Combatant, def: &Combatant, cond: &Conditions) -> Option<[u16; 16]> {
    damage_full(dex, mv, att, def, cond, false, 1).map(|h| h.rolls)
}

/// The result of one hit: all 16 rolls and the type effectiveness (+1 per super effective step).
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub rolls: [u16; 16],
    pub type_mod: i32,
}

/// Shell Armor / Battle Armor and an intact Disguise stop critical hits.
pub fn crit_blocked(def: &Combatant) -> bool {
    def.ability == Ab::ShellArmor || def.disguised
}

/// One hit's damage. `crit`: a critical hit. `hit`: which hit of a multi-hit move (1-based).
pub fn damage_full(dex: &Dex, mv: &MoveData, att: &Combatant, def: &Combatant, cond: &Conditions, crit: bool, hit: u8) -> Option<Hit> {
    damage_impl(dex, mv, att, def, cond, crit, hit, None)
}

/// Like `damage_full`, but only roll `roll` (0 = highest, 15 = lowest) is computed (the others are 0).
pub fn damage_one(dex: &Dex, mv: &MoveData, att: &Combatant, def: &Combatant, cond: &Conditions, crit: bool, hit: u8, roll: usize) -> Option<Hit> {
    damage_impl(dex, mv, att, def, cond, crit, hit, Some(roll))
}

#[allow(clippy::too_many_arguments)]
fn damage_impl(dex: &Dex, mv: &MoveData, att: &Combatant, def: &Combatant, cond: &Conditions, crit: bool, hit: u8, only: Option<usize>) -> Option<Hit> {
    if mv.category == Category::Status {
        return None;
    }
    let r = resolve_move(dex, mv, att, cond);
    let mut type_mod = effectiveness(dex, mv, &r, def)?;
    if def.disguised {
        type_mod = 0;
    }
    // moves that skip the formula
    let fixed = match mv.fixed {
        FixedDamage::No => None,
        FixedDamage::Level => Some(50),
        FixedDamage::Amount(n) => Some(n),
        FixedDamage::HalfHp => Some((def.hp / 2).max(1)),
        FixedDamage::UserHp => Some(att.hp),
        FixedDamage::Ohko => Some(def.maxhp),
        FixedDamage::Endeavor if att.hp < def.hp => Some(def.hp - att.hp),
        FixedDamage::Endeavor => return None,
        FixedDamage::Retaliate(0) => Some(((att.retal[0] as u32 * 3 / 2) as u16).max(1)),
        FixedDamage::Retaliate(k) => Some(att.retal[k as usize].saturating_mul(2).max(1)),
    };
    if let Some(d) = fixed {
        return Some(Hit { rolls: [d; 16], type_mod });
    }
    let physical = r.category == Category::Physical;
    let mtype = r.type_;
    let is = |name: &str| type_named(dex, mtype, name);
    let att_grounded = is_grounded(dex, att);
    let def_grounded = is_grounded(dex, def);
    let mega_sol = att.ability == Ab::MegaSol;
    let att_weather = effective_weather_for(att, cond, mega_sol);
    let def_weather = effective_weather_for(def, cond, mega_sol);

    // ---- base power event
    let bp0 = if mv.kind == Mv::TripleAxel {
        20 * hit as i64
    } else if mv.kind == Mv::BeatUp {
        cond.beat_up_bp.max(1) as i64
    } else {
        base_power(dex, mv, &r, att, def, cond)
    };
    let mut ev = Event::new();
    if att.ability == Ab::Rivalry && att.gender != 0 && def.gender != 0 && att.gender < 3 && def.gender < 3 {
        ev.chain(24, if att.gender == def.gender { 5120 } else { 3072 });
    }
    if att.ability == Ab::Technician {
        // Technician looks at the base power after the modifiers chained so far (it runs first
        // at priority 30, so that is just the base power)
        if bp0 <= 60 {
            ev.chain(30, X1_5);
        }
    }
    if r.type_changed_by_ability {
        ev.chain(23, 4915);
    }
    if att.ability == Ab::IronFist && r.flags & flag::PUNCH != 0 {
        ev.chain(23, 4915);
    }
    if att.item == It::PunchingGlove && r.flags & flag::PUNCH != 0 {
        ev.chain(23, 4506);
    }
    if cond.steely_spirit && (mtype == dex.ty.steel) {
        ev.chain(22, X1_5);
    }
    if att.ability == Ab::SteelySpirit && (mtype == dex.ty.steel) {
        ev.chain(22, X1_5);
    }
    if att.ability == Ab::ToughClaws && r.flags & flag::CONTACT != 0 {
        ev.chain(21, 5325);
    }
    if att.ability == Ab::SheerForce && r.sheer_force {
        ev.chain(21, 5325);
    }
    if att.ability == Ab::SandForce && cond.weather == Weather::Sand && ((mtype == dex.ty.rock) || (mtype == dex.ty.ground) || (mtype == dex.ty.steel)) {
        ev.chain(21, 5325);
    }
    let fairy_aura = cond.fairy_aura || att.ability == Ab::FairyAura;
    let dark_aura = cond.dark_aura || att.ability == Ab::DarkAura;
    if (fairy_aura && (mtype == dex.ty.fairy)) || (dark_aura && (mtype == dex.ty.dark)) {
        ev.chain(20, 5448);
    }
    // Dry Skin: Fire moves get x1.25 base power (onSourceBasePower, priority 17; 2026-10-15 fuzzing)
    if def.ability == Ab::Dryskin && (mtype == dex.ty.fire) {
        ev.chain(17, 5120);
    }
    if att.ability == Ab::Sharpness && r.flags & flag::SLICING != 0 {
        ev.chain(19, X1_5);
    }
    if att.ability == Ab::MegaLauncher && r.flags & flag::PULSE != 0 {
        ev.chain(19, X1_5);
    }
    if att.ability == Ab::StrongJaw && r.flags & flag::BITE != 0 {
        ev.chain(19, X1_5);
    }
    match att.item {
        It::MuscleBand if physical => ev.chain(16, 4505),
        It::WiseGlasses if !physical => ev.chain(16, 4505),
        It::TypeBoost(t) if is(t) => ev.chain(15, 4915),
        _ => {}
    }
    if cond.helping_hand {
        ev.chain(10, X1_5);
    }
    if att.charged && (mtype == dex.ty.electric) {
        ev.chain(9, X2);
    }
    if cond.gem {
        ev.chain(0, 5325);
    }
    if att.ability == Ab::PunkRock && r.flags & flag::SOUND != 0 {
        ev.chain(7, 5325);
    }
    match cond.terrain {
        Terrain::Electric if (mtype == dex.ty.electric) && att_grounded => ev.chain(6, 5325),
        Terrain::Psychic if (mtype == dex.ty.psychic) && att_grounded => ev.chain(6, 5325),
        Terrain::Grassy => {
            if matches!(mv.id.as_str(), "earthquake" | "bulldoze" | "magnitude") && def_grounded && def.semi == 0 {
                ev.chain(6, X0_5);
            } else if (mtype == dex.ty.grass) && att_grounded {
                ev.chain(6, 5325);
            }
        }
        Terrain::Misty if (mtype == dex.ty.dragon) && def_grounded => ev.chain(6, X0_5),
        _ => {}
    }
    // the move's own BasePower handler (default priority 0)
    match mv.kind {
        Mv::KnockOff if def.item_removable => ev.chain(0, X1_5),
        Mv::ExpandingForce if cond.terrain == Terrain::Psychic && att_grounded => ev.chain(0, X1_5),
        Mv::Mistyexplosion if cond.terrain == Terrain::Misty && att_grounded => ev.chain(0, X1_5),
        Mv::SolarBeam | Mv::SolarBlade if matches!(att_weather, Weather::Rain | Weather::Sand | Weather::Snow) => {
            ev.chain(0, X0_5)
        }
        Mv::Facade if !matches!(att.status, Status::None | Status::Sleep) => ev.chain(0, X2),
        Mv::Lashout if att.stats_lowered => ev.chain(0, X2),
        Mv::Barbbarrage if matches!(def.status, Status::Poison | Status::Toxic) => ev.chain(0, X2),
        Mv::Venoshock if matches!(def.status, Status::Poison | Status::Toxic) => ev.chain(0, X2),
        Mv::GravApple if att.forced_grounded => ev.chain(0, X1_5),
        Mv::Ficklebeam if cond.fickle => ev.chain(0, X2),
        _ => {}
    }
    if att.ability == Ab::Reckless && (mv.recoil.is_some() || matches!(mv.kind, Mv::Highjumpkick | Mv::Jumpkick | Mv::Axekick | Mv::Supercellslam)) {
        ev.chain(23, 4915);
    }
    if att.ability == Ab::Analytic && cond.analytic {
        ev.chain(21, 5325);
    }
    if att.ability == Ab::Supremeoverlord {
        const POW: [i64; 6] = [4096, 4506, 4915, 5325, 5734, 6144];
        let n = (cond.supreme_fallen as usize).min(5);
        if n > 0 {
            ev.chain(21, POW[n]);
        }
    }
    let bp = ev.run(bp0).max(1);

    // ---- attack stat
    let (a_stat, d_stat) = if physical { (ATK, DEF) } else { (SPA, SPD) };
    let a_stat = mv.offensive_stat.unwrap_or(a_stat);
    let d_stat = mv.defensive_stat.unwrap_or(d_stat);
    let attacker_for_stat = if mv.uses_target_attack { def } else { att };
    // Unaware on the defender ignores the attacker's offensive boosts
    // Wonder Room: Body Press-style moves keep the Def VALUE but use the SpD boosts (Showdown swaps
    // overrideOffensiveStat, then calculateStat swaps the stored stat back)
    let a_boost_stat = if cond.wonder_room && mv.offensive_stat.is_some() {
        if a_stat == DEF { SPD } else if a_stat == SPD { DEF } else { a_stat }
    } else { a_stat };
    let mut atk_boost = if def.ability == Ab::Unaware { 0 } else { attacker_for_stat.boosts[a_boost_stat] };
    if crit && atk_boost < 0 {
        atk_boost = 0;
    }
    let mut attack = boosted(attacker_for_stat.stats[a_stat] as u32, atk_boost) as i64;
    let mut ev = Event::new();
    let pinch = att.hp as u32 * 3 <= att.maxhp as u32;
    match att.ability {
        Ab::HugePower | Ab::PurePower if physical => ev.chain(5, X2),
        Ab::Stakeout if cond.stakeout => ev.chain(5, X2),
        Ab::Guts if physical && att.status != Status::None => ev.chain(5, X1_5),
        Ab::Hustle if physical => ev.direct(5, X1_5),
        Ab::Overgrow if pinch && (mtype == dex.ty.grass) => ev.chain(5, X1_5),
        Ab::Blaze if pinch && (mtype == dex.ty.fire) => ev.chain(5, X1_5),
        Ab::Torrent if pinch && (mtype == dex.ty.water) => ev.chain(5, X1_5),
        Ab::Swarm if pinch && (mtype == dex.ty.bug) => ev.chain(5, X1_5),
        Ab::FireMane if (mtype == dex.ty.fire) => ev.chain(5, X1_5),
        Ab::SolarPower if !physical && att_weather == Weather::Sun => ev.chain(5, X1_5),
        Ab::FlashFire if att.flash_fire && (mtype == dex.ty.fire) => ev.chain(5, X1_5),
        Ab::WaterBubble if (mtype == dex.ty.water) => ev.chain(0, X2),
        Ab::Plus | Ab::Minus if !physical && cond.plus_minus => ev.chain(5, X1_5),
        _ => {}
    }
    match att.item {
        It::ChoiceBand if physical => ev.chain(1, X1_5),
        It::ChoiceSpecs if !physical => ev.chain(1, X1_5),
        It::LightBall if att.is_pikachu => ev.chain(1, X2),
        _ => {}
    }
    match def.ability {
        Ab::ThickFat if (mtype == dex.ty.ice) || (mtype == dex.ty.fire) => ev.chain(if physical { 6 } else { 5 }, X0_5),
        Ab::Purifyingsalt if (mtype == dex.ty.ghost) => ev.chain(if physical { 6 } else { 5 }, X0_5),
        Ab::Heatproof if (mtype == dex.ty.fire) => ev.chain(if physical { 6 } else { 5 }, X0_5),
        Ab::WaterBubble if (mtype == dex.ty.fire) => ev.chain(5, X0_5),
        _ => {}
    }
    attack = ev.run(attack);

    // ---- defense stat
    let mut def_boost = if mv.ignore_defensive || att.ability == Ab::Unaware { 0 } else { def.boosts[d_stat] };
    if crit && def_boost > 0 {
        def_boost = 0;
    }
    let d_val = if cond.wonder_room { if d_stat == DEF { SPD } else if d_stat == SPD { DEF } else { d_stat } } else { d_stat };
    let mut defense = boosted(def.stats[d_val] as u32, def_boost) as i64;
    let mut ev = Event::new();
    if d_stat == DEF {
        if def_weather == Weather::Snow && has_type(def, dex.ty.ice) {
            ev.direct(10, X1_5);
        }
        if def.ability == Ab::FurCoat {
            ev.chain(6, X2);
        }
        if def.ability == Ab::GrassPelt && cond.terrain == Terrain::Grassy {
            ev.chain(6, X1_5);
        }
        if def.ability == Ab::Marvelscale && def.status != Status::None {
            ev.chain(6, X1_5);
        }
    } else {
        if def_weather == Weather::Sand && has_type(def, dex.ty.rock) {
            ev.direct(10, X1_5);
        }
        if def.item == It::AssaultVest {
            ev.chain(1, X1_5);
        }
    }
    defense = ev.run(defense);

    // ---- base damage
    let level = if att.level == 0 { 50 } else { att.level as i64 };
    let base = ((2 * level / 5 + 2) * bp * attack / defense) / 50;
    let mut dmg = base + 2;
    if cond.spread {
        dmg = modify(dmg, X0_75);
    } else if cond.parental_second {
        dmg = modify(dmg, 1024);
    }
    // weather (uses the defender's effective weather)
    let mut ev = Event::new();
    match def_weather {
        Weather::Rain if (mtype == dex.ty.water) => ev.chain(0, X1_5),
        Weather::Rain if (mtype == dex.ty.fire) => ev.chain(0, X0_5),
        Weather::Sun if (mtype == dex.ty.fire) => ev.chain(0, X1_5),
        Weather::Sun if (mtype == dex.ty.water) => ev.chain(0, X0_5),
        _ => {}
    }
    dmg = ev.run(dmg);
    if crit {
        dmg = dmg * 3 / 2;
    }

    // ---- per roll
    let stab_mod: i64 = {
        let stab = has_type(att, mtype);
        if att.ability == Ab::Adaptability && stab {
            X2
        } else if stab {
            X1_5
        } else {
            4096
        }
    };
    // ModifyDamage: the attacker's onModifyDamage, the defender's onSourceModifyDamage, and field
    // effects' onAnyModifyDamage (all chained: the same factor for every roll)
    let final_mod = {
        let mut ev = Event::new();
        if def.glaive_rush {
            ev.chain(0, X2);
        }
        if cond.minimized && r.flags & flag::MINIMIZE != 0 {
            ev.chain(0, X2);
        }
        let screen = (physical && cond.reflect) || (!physical && cond.light_screen);
        if !crit && !r.infiltrates && (screen || cond.aurora_veil) {
            ev.chain(0, 2732);
        }
        if crit && att.ability == Ab::Sniper {
            ev.chain(0, X1_5);
        }
        match att.item {
            It::LifeOrb => ev.chain(0, 5324),
            It::Metronome => ev.chain(0, [4096, 4915, 5734, 6553, 7372, 8192][att.metro.min(5) as usize]),
            It::ExpertBelt if type_mod > 0 => ev.chain(0, 4915),
            _ => {}
        }
        match (def.semi, mv.kind) {
            (1, Mv::Earthquake | Mv::Magnitude) | (2, Mv::Surf | Mv::Whirlpool) | (3, Mv::Gust | Mv::Twister) => ev.chain(0, X2),
            _ => {}
        }
        match def.ability {
            Ab::Multiscale | Ab::ShadowShield if def.hp >= def.maxhp => ev.chain(0, X0_5),
            Ab::SolidRock | Ab::Filter | Ab::PrismArmor if type_mod > 0 => ev.chain(0, X0_75),
            Ab::AuraGuard if r.flags & flag::CONTACT != 0 => ev.chain(0, X0_5),
            Ab::PunkRock if r.flags & flag::SOUND != 0 => ev.chain(0, X0_5),
            Ab::IceScales if !physical => ev.chain(0, X0_5),
            Ab::Fluffy => {
                let fire = (mtype == dex.ty.fire);
                let contact = r.flags & flag::CONTACT != 0;
                if fire && !contact {
                    ev.chain(0, X2);
                } else if contact && !fire {
                    ev.chain(0, X0_5);
                }
            }
            _ => {}
        }
        if let It::ResistBerry(t) = def.item {
            if !cond.berries_blocked && is(t) && (type_mod > 0 || t == "Normal") {
                ev.chain(0, if def.ability == Ab::Ripen { 1024 } else { X0_5 });   // Ripen: the berry is twice as strong
            } else if def.berry_weaken && def.ability == Ab::Ripen {
                ev.chain(-1, X0_5);
            }
        } else if def.berry_weaken && def.ability == Ab::Ripen {
            ev.chain(-1, X0_5);
        }
        if cond.friend_guard {
            ev.chain(0, X0_75);
        }
        ev.chained_factor()
    };
    let burned = physical && att.status == Status::Burn && att.ability != Ab::Guts && mv.kind != Mv::Facade;   // Facade ignores the burn (gen 6+)
    let mut out = [0u16; 16];
    for (i, slot) in out.iter_mut().enumerate() {
        if let Some(o) = only {
            if o != i {
                continue;
            }
        }
        let mut d = dmg * (100 - i as i64) / 100;
        if stab_mod != 4096 {
            d = modify(d, stab_mod);
        }
        if type_mod > 0 {
            for _ in 0..type_mod {
                d *= 2;
            }
        } else {
            for _ in 0..(-type_mod) {
                d /= 2;
            }
        }
        if burned {
            d = modify(d, X0_5);
        }
        if final_mod != 4096 {
            d = modify(d, final_mod);
        }
        if cond.protect_bypass {
            d = modify(d, 1024);
        }
        let d = (d & 0xFFFF) as u16;
        *slot = if d == 0 { 1 } else { d };
    }
    Some(Hit { rolls: out, type_mod })
}
