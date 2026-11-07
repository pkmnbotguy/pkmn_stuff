//! Battle state, mirroring Showdown's objects field by field (Pokemon, Side, Field, EffectState,
//! actions, active moves), so the port can follow Showdown's code line by line.

use super::data::*;
use super::val::{Mon, Val};

pub const NONE8: u8 = u8::MAX;
pub const NONE16: u16 = u16::MAX;

/// Keys of the free-form fields that effects keep in their state (`this.effectState.xxx`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum K {
    Counter,
    Time,
    StartTime,
    Stage,
    Move,
    TrueDuration,
    TargetLoc,
    Layers,
    Hp,
    BoundDivisor,
    Started,
    Ending,
    Busted,
    Checked,
    Resisted,
    Boosts,
    Lastmove,
    Multiplier,
    Hit,
    Source2,
    Fallen,
    Gotten,
    Libero,
    TypeWas,
    Turns,
    HasDragonType,
    Stat,
    FromBooster,
    Unnerved,
    Target2,
    EndingTurn,
    MoveData,
    TargetSlot,
    Duration2,
    SwitchingIn,
    BerryWeaken,
    Pos,
    X1,
    X2,
    X3,
}

/// `this.effectState`: the data an active effect keeps. Showdown uses free-form objects; the
/// common fields are explicit here and the rest live in `vals`.
#[derive(Clone, Debug, Default)]
pub struct EffectState {
    pub id: EffId,
    /// unique id, so a handler can tell "my state was removed and re-added" (Showdown compares objects)
    pub uid: u32,
    pub effect_order: f64,
    pub duration: Option<f64>,
    /// `state.target` has been set (it decides the subOrder of field conditions)
    pub target_set: bool,
    pub target_side: Option<u8>,
    pub target_mon: Option<Mon>,
    pub source: Option<Mon>,
    pub source_slot: Option<(u8, u8)>,
    pub source_effect: Option<EffId>,
    pub is_slot_condition: bool,
    pub linked: Vec<Mon>,
    pub linked_status: Option<EffId>,
    pub vals: Vec<(K, Val)>,
}

impl EffectState {
    pub fn get(&self, k: K) -> Val {
        self.vals.iter().find(|x| x.0 == k).map(|x| x.1).unwrap_or(Val::Undef)
    }
    pub fn num(&self, k: K) -> f64 {
        self.get(k).num()
    }
    pub fn set(&mut self, k: K, v: Val) {
        if let Some(x) = self.vals.iter_mut().find(|x| x.0 == k) {
            x.1 = v;
        } else {
            self.vals.push((k, v));
        }
    }
    pub fn del(&mut self, k: K) {
        self.vals.retain(|x| x.0 != k);
    }
}

/// Where an effect's state lives (Showdown passes the state object itself).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum StateLoc {
    None,
    Status(Mon),
    Volatile(Mon, EffId),
    Ability(Mon),
    Item(Mon),
    Species(Mon),
    Slot(u8, u8, EffId),
    SideCond(u8, EffId),
    Pseudo(EffId),
    Weather,
    Terrain,
    /// a temporary `{}` state (singleEvent with a fresh object): index into Battle::temp_states
    Temp(u16),
}

/// The holder of an effect (`effectHolder`, `this.effectState.target`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Holder {
    None,
    Mon(Mon),
    Side(u8),
    Field,
    Battle,
}

/// A target / source argument of an event.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Arg {
    None,
    Mon(Mon),
    Side(u8),
    Field,
    Battle,
    Type(TypeId),
    Key(&'static str),
}
impl Arg {
    pub fn mon(self) -> Option<Mon> {
        if let Arg::Mon(m) = self { Some(m) } else { None }
    }
    pub fn from_opt(m: Option<Mon>) -> Arg {
        match m {
            Some(m) => Arg::Mon(m),
            None => Arg::None,
        }
    }
}

/// An effect argument: a dex effect, an active move, or a part of an active move (secondary / self).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EffRef {
    pub eff: EffId,
    /// index into Battle::moves when this is an active move
    pub mv: u16,
    /// 0 = the effect itself; 1..=n = secondaries[n-1]; SELF_PART = move.self
    pub part: u8,
}
pub const SELF_PART: u8 = 0xFE;
impl EffRef {
    pub const NONE: EffRef = EffRef { eff: NO_EFF, mv: NONE16, part: 0 };
    pub fn eff(e: EffId) -> EffRef {
        EffRef { eff: e, mv: NONE16, part: 0 }
    }
    pub fn is_none(&self) -> bool {
        self.eff == NO_EFF
    }
    pub fn is_move(&self) -> bool {
        self.mv != NONE16
    }
}

#[derive(Clone, Debug)]
pub struct MoveSlot {
    pub id: u16,
    pub pp: i32,
    pub maxpp: i32,
    pub target: MoveTarget,
    /// 0 = no, 1 = yes, 2 = 'hidden'
    pub disabled: u8,
    pub disabled_source: Option<String>,
    pub used: bool,
    pub virtual_: bool,
}

#[derive(Clone, Debug)]
pub struct Attacker {
    pub source: Mon,
    pub damage: i32,
    pub this_turn: bool,
    pub move_id: u16,
    pub slot: (u8, u8),
    pub damage_value: Val,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchFlag {
    No,
    Yes,
    /// the move that asked for the switch (U-turn, Volt Switch...)
    Move(u16),
}
impl SwitchFlag {
    pub fn is_set(self) -> bool {
        self != SwitchFlag::No
    }
}

/// A team member as written in a Showdown export.
#[derive(Clone, Debug, Default)]
pub struct SetData {
    pub name: String,
    pub species: String,
    pub item: String,
    pub ability: String,
    pub nature: String,
    pub gender: String,
    pub evs: [i32; 6],
    pub level: i32,
    pub moves: Vec<String>,
    pub shiny: bool,
}

/// Parse a team in Showdown's export format.
pub fn parse_team(text: &str) -> Vec<SetData> {
    let mut out = vec![];
    for block in text.replace("\r\n", "\n").split("\n\n") {
        let lines: Vec<&str> = block.lines().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
        if lines.is_empty() {
            continue;
        }
        let mut set = SetData { level: 100, ..Default::default() };
        let mut head = lines[0].to_string();
        if let Some(p) = head.find(" @ ") {
            set.item = head[p + 3..].trim().to_string();
            head = head[..p].trim().to_string();
        }
        // gender
        for g in ["(M)", "(F)"] {
            if head.ends_with(g) {
                set.gender = g[1..2].to_string();
                head = head[..head.len() - 3].trim().to_string();
            }
        }
        // "Nickname (Species)"
        if head.ends_with(')') {
            if let Some(p) = head.rfind('(') {
                set.species = head[p + 1..head.len() - 1].trim().to_string();
                set.name = head[..p].trim().to_string();
            }
        }
        if set.species.is_empty() {
            set.species = head.clone();
            set.name = String::new();
        }
        for l in &lines[1..] {
            if let Some(rest) = l.strip_prefix("Ability:") {
                set.ability = rest.trim().to_string();
            } else if let Some(rest) = l.strip_prefix("Level:") {
                set.level = rest.trim().parse().unwrap_or(100);
            } else if let Some(rest) = l.strip_prefix("EVs:").or_else(|| l.strip_prefix("SPs:")) {
                for part in rest.split('/') {
                    let mut it = part.split_whitespace();
                    let (Some(n), Some(st)) = (it.next(), it.next()) else { continue };
                    let n: i32 = n.parse().unwrap_or(0);
                    let idx = match st.to_ascii_lowercase().as_str() {
                        "hp" => 0,
                        "atk" => 1,
                        "def" => 2,
                        "spa" => 3,
                        "spd" => 4,
                        "spe" => 5,
                        _ => continue,
                    };
                    set.evs[idx] = n;
                }
            } else if let Some(rest) = l.strip_suffix(" Nature") {
                set.nature = rest.trim().to_string();
            } else if let Some(rest) = l.strip_prefix("- ") {
                set.moves.push(rest.trim().to_string());
            } else if l.starts_with("Shiny: Yes") {
                set.shiny = true;
            }
        }
        out.push(set);
    }
    out
}

#[derive(Clone, Debug)]
pub struct Pokemon {
    pub side: u8,
    /// index in the side's team (stable)
    pub ti: u8,
    pub set: SetData,
    pub name: String,
    pub level: i32,
    /// 'M', 'F' or "" (genderless)
    pub gender: &'static str,
    pub base_move_slots: Vec<MoveSlot>,
    pub move_slots: Vec<MoveSlot>,
    pub position: u8,
    pub details: String,
    pub base_species: u16,
    pub species: u16,
    pub species_state: EffectState,
    pub status: EffId,
    pub status_state: EffectState,
    pub volatiles: Vec<(EffId, EffectState)>,
    pub base_stored_stats: [i32; 6],
    pub stored_stats: [i32; 6],
    /// atk, def, spa, spd, spe, accuracy, evasion
    pub boosts: [i8; 7],
    pub base_ability: EffId,
    pub ability: EffId,
    pub ability_state: EffectState,
    pub item: EffId,
    pub item_state: EffectState,
    pub last_item: EffId,
    pub used_item_this_turn: bool,
    pub ate_berry: bool,
    /// 0 = false, 1 = true, 2 = 'hidden'
    pub trapped: u8,
    pub maybe_trapped: bool,
    pub maybe_disabled: bool,
    pub maybe_locked: bool,
    pub transformed: bool,
    /// Supersweet Syrup already triggered this battle (Showdown's pokemon.syrupTriggered)
    pub syrup_triggered: bool,
    pub maxhp: i32,
    pub base_maxhp: i32,
    pub hp: i32,
    pub fainted: bool,
    pub faint_queued: bool,
    pub sub_fainted: Option<bool>,
    pub forme_regression: bool,
    pub types: Vec<TypeId>,
    pub added_type: TypeId,
    pub known_type: bool,
    pub apparent_type: String,
    pub switch_flag: SwitchFlag,
    pub force_switch_flag: bool,
    pub skip_before_switch_out: bool,
    pub dragged_in: Option<i32>,
    pub newly_switched: bool,
    /// Zero to Hero's switch-in message was shown (pokemon.heroMessageDisplayed)
    pub hero_message_displayed: bool,
    pub being_called_back: bool,
    pub last_move: Option<u16>,
    pub last_move_used: Option<u16>,
    pub last_move_target_loc: Option<i8>,
    pub move_this_turn: Option<u16>,
    pub stats_raised_this_turn: bool,
    pub stats_lowered_this_turn: bool,
    /// Undef / Null / False / True
    pub move_last_turn_result: Val,
    pub move_this_turn_result: Val,
    pub hurt_this_turn: Option<i32>,
    pub last_damage: i32,
    pub attacked_by: Vec<Attacker>,
    pub times_attacked: i32,
    pub is_active: bool,
    pub active_turns: i32,
    pub active_move_actions: i32,
    pub previously_switched_in: i32,
    pub is_started: bool,
    pub during_move: bool,
    pub weighthg: i32,
    pub speed: f64,
    /// species index of the Mega form this Pokemon can become (None = can't, or already used)
    pub can_mega_evo: Option<u16>,
    pub truant_turn: bool,
    /// Illusion: the Pokemon this one is disguised as
    pub illusion: Option<Mon>,
}

#[derive(Clone, Debug, Default)]
pub struct Choice {
    pub cant_undo: bool,
    pub error: String,
    pub actions: Vec<ChosenAction>,
    pub forced_switches_left: i32,
    pub forced_passes_left: i32,
    pub switch_ins: Vec<u8>,
    pub mega: bool,
}

#[derive(Clone, Debug)]
pub struct ChosenAction {
    pub choice: ActionKind,
    pub pokemon: Option<Mon>,
    pub target_loc: i8,
    pub move_id: u16,
    pub move_slot: Option<u8>,
    pub mega: bool,
    pub target: Option<Mon>,
    pub index: u8,
    pub priority: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestState {
    None,
    TeamPreview,
    Move,
    Switch,
}

#[derive(Clone, Debug)]
pub struct Side {
    pub n: u8,
    pub name: String,
    pub mons: Vec<Pokemon>,
    /// Showdown's `side.pokemon`: team indices in current order
    pub pokemon: Vec<u8>,
    /// Showdown's `side.active`: team indices (may be fainted), None before the first switch-in
    pub active: [Option<u8>; 2],
    pub pokemon_left: i32,
    pub fainted_last_turn: Option<u8>,
    pub fainted_this_turn: Option<u8>,
    pub total_fainted: i32,
    pub side_conditions: Vec<(EffId, EffectState)>,
    pub slot_conditions: [Vec<(EffId, EffectState)>; 2],
    /// what this side was asked for (Showdown's activeRequest; `wait` = None)
    pub request: RequestState,
    pub force_switch: [bool; 2],
    pub choice: Choice,
}

#[derive(Clone, Debug, Default)]
pub struct Field {
    pub weather: EffId,
    pub weather_state: EffectState,
    pub terrain: EffId,
    pub terrain_state: EffectState,
    pub pseudo_weather: Vec<(EffId, EffectState)>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionKind {
    Move,
    BeforeTurnMove,
    PriorityChargeMove,
    Switch,
    InstaSwitch,
    RevivalBlessing,
    Team,
    Start,
    Residual,
    Pass,
    BeforeTurn,
    MegaEvo,
    RunSwitch,
    Event,
    Shift,
}

#[derive(Clone, Debug)]
pub struct Action {
    pub choice: ActionKind,
    pub order: f64,
    pub priority: f64,
    pub fractional_priority: f64,
    pub speed: f64,
    pub pokemon: Option<Mon>,
    pub target_loc: i8,
    pub original_target: Option<Mon>,
    pub move_id: u16,
    /// active move (index into Battle::moves)
    pub mv: u16,
    /// mega: 0 = no, 1 = yes, 2 = done
    pub mega: u8,
    pub target: Option<Mon>,
    pub source_effect: Option<EffId>,
    pub index: u8,
    pub event: Option<super::events::Ev>,
    pub side: u8,
}
impl Action {
    pub fn new(choice: ActionKind) -> Action {
        Action {
            choice,
            order: 0.0,
            priority: 0.0,
            fractional_priority: 0.0,
            speed: 0.0,
            pokemon: None,
            target_loc: 0,
            original_target: None,
            move_id: NONE16,
            mv: NONE16,
            mega: 0,
            target: None,
            source_effect: None,
            index: 0,
            event: None,
            side: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MoveHitData {
    pub crit: bool,
    pub type_mod: i32,
    /// None = false; Some(NO_EFF) = true; Some(eff) = an effect (Unseen Fist)
    pub bypass_protect: Option<EffId>,
    pub used: bool,
}

/// Showdown's ActiveMove: a mutable copy of a move for one use.
#[derive(Clone, Debug)]
pub struct ActiveMove {
    pub def: u16,
    pub eff: EffId,
    pub type_: TypeId,
    pub category: Category,
    pub base_power: f64,
    pub accuracy: Option<f64>,
    pub priority: f64,
    pub target: MoveTarget,
    pub flags: u64,
    pub crit_ratio: i32,
    pub will_crit: Option<bool>,
    pub crit_modifier: Option<f64>,
    /// Future Sight's delayed hit: Showdown builds it from plain move data (`new Move(moveData)`), without the
    /// move's own handlers
    pub future_hit: bool,
    pub secondaries: Option<Vec<HitEffectData>>,
    pub self_: Option<HitEffectData>,
    pub self_dropped: bool,
    pub self_boost: Option<SparseBoosts>,
    pub boosts: Option<SparseBoosts>,
    pub status: Option<EffId>,
    pub volatile_status: Option<EffId>,
    pub side_condition: Option<EffId>,
    pub slot_condition: Option<EffId>,
    pub weather: Option<EffId>,
    pub terrain: Option<EffId>,
    pub pseudo_weather: Option<EffId>,
    pub heal: Option<(f64, f64)>,
    pub drain: Option<(f64, f64)>,
    pub recoil: Option<(f64, f64)>,
    pub self_switch: SelfSwitch,
    pub force_switch: bool,
    pub selfdestruct: SelfDestruct,
    pub breaks_protect: bool,
    /// None = undefined
    pub ignore_immunity: Option<IgnoreImmunity>,
    pub ignore_ability: bool,
    pub ignore_defensive: bool,
    pub ignore_offensive: bool,
    pub ignore_negative_offensive: bool,
    pub ignore_positive_defensive: bool,
    pub ignore_evasion: bool,
    pub ignore_accuracy: bool,
    pub always_hit: bool,
    pub multihit: Option<(u8, u8)>,
    pub multihit_type: u8,
    pub multiaccuracy: bool,
    pub ohko: bool,
    pub damage: FixedDmg,
    pub override_offensive_stat: Option<u8>,
    pub override_offensive_target: bool,
    pub override_defensive_stat: Option<u8>,
    pub override_defensive_source: bool,
    pub sleep_usable: bool,
    /// None = undefined (typeof smartTarget !== 'boolean')
    pub smart_target: Option<bool>,
    pub tracks_target: bool,
    pub steals_boosts: bool,
    pub struggle_recoil: bool,
    pub mind_blown_recoil: bool,
    pub has_crash_damage: bool,
    pub thaws_target: bool,
    pub force_stab: bool,
    pub stalling_move: bool,
    pub spread_hit: bool,
    pub has_sheer_force: bool,
    pub infiltrates: bool,
    pub has_bounced: bool,
    pub prankster_boosted: bool,
    pub is_external: bool,
    pub source_effect: Option<EffId>,
    pub hit: u8,
    pub last_hit: bool,
    pub total_damage: f64,
    pub hit_targets: Vec<Mon>,
    /// per field slot: [side][position]
    pub move_hit_data: [[MoveHitData; 2]; 2],
    /// Pixilate & co: `move.typeChangerBoosted`
    pub type_changer_boosted: Option<EffId>,
    pub stab: Option<f64>,
    pub has_life_orb_boost: bool,
    pub ruined_atk: Option<Mon>,
    pub ruined_def: Option<Mon>,
    pub ruined_spa: Option<Mon>,
    pub ruined_spd: Option<Mon>,
    pub allies_hit: bool,
    pub is_z_or_max_powered: bool,
    pub base_move: u16,
    /// Beat Up: the allies that still have to hit
    pub allies: Vec<Mon>,
    /// Fairy Aura & co: `move.auraBooster`
    pub aura_booster: Option<Mon>,
}
