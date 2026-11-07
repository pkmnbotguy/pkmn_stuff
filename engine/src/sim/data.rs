//! Game data for the exact engine: species, moves, items, abilities, conditions and every
//! effect's event handlers, loaded from `data/dex.json` (tools/export_dex.js) and
//! `data/effects.json` (tools/export_effects.js), both exported from Showdown for the format.

use std::collections::HashMap;

use serde_json::Value;

use super::events::{Ev, EV_NAMES};
use super::val::Val;

pub type TypeId = u8;
pub const NO_TYPE: TypeId = 255;
pub type EffId = u16;
pub const NO_EFF: EffId = u16::MAX;
pub type CbId = u16;
pub const NO_CB: CbId = u16::MAX;

/// Boost ids in Showdown's object order: atk, def, spa, spd, spe, accuracy, evasion.
pub const BOOST_NAMES: [&str; 7] = ["atk", "def", "spa", "spd", "spe", "accuracy", "evasion"];
pub const B_ATK: u8 = 0;
pub const B_DEF: u8 = 1;
pub const B_SPA: u8 = 2;
pub const B_SPD: u8 = 3;
pub const B_SPE: u8 = 4;
pub const B_ACC: u8 = 5;
pub const B_EVA: u8 = 6;
/// stat ids: hp, atk, def, spa, spd, spe
pub const STAT_NAMES: [&str; 6] = ["hp", "atk", "def", "spa", "spd", "spe"];

/// A sparse boosts table that keeps insertion order (Showdown iterates `for (k in boosts)`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SparseBoosts {
    pub n: u8,
    pub e: [(u8, i8); 7],
}
impl SparseBoosts {
    pub fn get(&self, stat: u8) -> Option<i8> {
        self.e[..self.n as usize].iter().find(|x| x.0 == stat).map(|x| x.1)
    }
    pub fn set(&mut self, stat: u8, v: i8) {
        for x in self.e[..self.n as usize].iter_mut() {
            if x.0 == stat {
                x.1 = v;
                return;
            }
        }
        self.e[self.n as usize] = (stat, v);
        self.n += 1;
    }
    pub fn remove(&mut self, stat: u8) {
        if let Some(p) = self.e[..self.n as usize].iter().position(|x| x.0 == stat) {
            for k in p..self.n as usize - 1 {
                self.e[k] = self.e[k + 1];
            }
            self.n -= 1;
        }
    }
    pub fn iter(&self) -> impl Iterator<Item = (u8, i8)> + '_ {
        self.e[..self.n as usize].iter().copied()
    }
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
    pub fn from_json(v: &Value) -> Option<SparseBoosts> {
        let o = v.as_object()?;
        let mut b = SparseBoosts::default();
        for (k, x) in o {
            if let Some(i) = BOOST_NAMES.iter().position(|n| n == k) {
                b.set(i as u8, x.as_i64().unwrap_or(0) as i8);
            }
        }
        Some(b)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Move,
    Ability,
    Item,
    Condition,
}

/// Showdown's effectType
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EffType {
    Move,
    Ability,
    Item,
    Condition,
    Status,
    Weather,
    Terrain,
    Other,
}

#[derive(Clone, Debug)]
pub enum HandlerKind {
    /// a function: implemented in Rust (`imp`), or missing (None)
    Fn,
    /// no handler, only its Order/Priority/SubOrder (read for duration-only Residual handlers)
    MetaOnly,
    /// a constant handler value, e.g. `onInvulnerability: false`, `onLockMove: 'recharge'`
    Value(Val),
}

/// The Rust implementation of one effect handler.
pub type HandlerFn = fn(&mut super::battle::Battle, &super::battle::HArgs) -> Val;

#[derive(Clone, Debug)]
pub struct HandlerMeta {
    pub cb: CbId,
    /// `on...Order` (None = false)
    pub order: Option<f64>,
    pub priority: f64,
    pub sub_order: f64,
    pub kind: HandlerKind,
    pub imp: Option<HandlerFn>,
}

#[derive(Clone, Debug, Default)]
pub struct ItemInfo {
    pub is_berry: bool,
    pub is_choice: bool,
    pub is_gem: bool,
    pub ignore_klutz: bool,
    /// base species name -> mega species name
    pub mega_stone: Vec<(String, String)>,
    pub fling_bp: u16,
    pub natural_gift: Option<(u16, TypeId)>,
    pub boosts: Option<SparseBoosts>,
    pub is_primal_orb: bool,
    pub z_move: bool,
}

#[derive(Clone, Debug, Default)]
pub struct AbilityInfo {
    pub breakable: bool,
    pub cantsuppress: bool,
    pub failroleplay: bool,
    pub failskillswap: bool,
    pub noreceiver: bool,
    pub notrace: bool,
    pub notransform: bool,
    pub suppress_weather: bool,
}

#[derive(Clone, Debug)]
pub struct Effect {
    pub kind: Kind,
    pub id: String,
    pub name: String,
    pub fullname: String,
    pub effect_type: EffType,
    pub num: i32,
    /// sorted by callback id
    pub handlers: Vec<HandlerMeta>,
    pub duration: Option<i32>,
    pub duration_cb: bool,
    pub no_copy: bool,
    pub affects_fainted: bool,
    pub counter_max: i32,
    pub item: Option<Box<ItemInfo>>,
    pub ability: Option<Box<AbilityInfo>>,
    /// for moves: index into `Data::moves`
    pub move_def: u16,
}

impl Effect {
    #[inline]
    pub fn handler(&self, cb: CbId) -> Option<&HandlerMeta> {
        if cb == NO_CB || self.handlers.is_empty() {
            return None;
        }
        self.handlers.binary_search_by_key(&cb, |h| h.cb).ok().map(|i| &self.handlers[i])
    }
    pub fn has(&self, cb: CbId) -> bool {
        self.handler(cb).map(|h| !matches!(h.kind, HandlerKind::MetaOnly)).unwrap_or(false)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoveTarget {
    Normal,
    Any,
    AdjacentFoe,
    AdjacentAlly,
    AdjacentAllyOrSelf,
    AllAdjacent,
    AllAdjacentFoes,
    SelfTarget,
    RandomNormal,
    AllySide,
    AllyTeam,
    FoeSide,
    All,
    Allies,
    Scripted,
    /// a move with no target at all (the nonexistent 'recharge' move)
    Undefined,
}
impl MoveTarget {
    pub fn parse(s: &str) -> MoveTarget {
        use MoveTarget::*;
        match s {
            "normal" => Normal,
            "any" => Any,
            "adjacentFoe" => AdjacentFoe,
            "adjacentAlly" => AdjacentAlly,
            "adjacentAllyOrSelf" => AdjacentAllyOrSelf,
            "allAdjacent" => AllAdjacent,
            "allAdjacentFoes" => AllAdjacentFoes,
            "self" => SelfTarget,
            "randomNormal" => RandomNormal,
            "allySide" => AllySide,
            "allyTeam" => AllyTeam,
            "foeSide" => FoeSide,
            "all" => All,
            "allies" => Allies,
            _ => Scripted,
        }
    }
    pub fn name(self) -> &'static str {
        use MoveTarget::*;
        match self {
            Normal => "normal",
            Any => "any",
            AdjacentFoe => "adjacentFoe",
            AdjacentAlly => "adjacentAlly",
            AdjacentAllyOrSelf => "adjacentAllyOrSelf",
            AllAdjacent => "allAdjacent",
            AllAdjacentFoes => "allAdjacentFoes",
            SelfTarget => "self",
            RandomNormal => "randomNormal",
            AllySide => "allySide",
            AllyTeam => "allyTeam",
            FoeSide => "foeSide",
            All => "all",
            Allies => "allies",
            Scripted => "scripted",
            Undefined => "",
        }
    }
    /// targets a player can choose (CHOOSABLE_TARGETS)
    pub fn choosable(self) -> bool {
        matches!(self, MoveTarget::Normal | MoveTarget::Any | MoveTarget::AdjacentAlly | MoveTarget::AdjacentAllyOrSelf | MoveTarget::AdjacentFoe)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Physical,
    Special,
    Status,
}

/// Move flags (bit per Showdown flag name).
pub mod mflag {
    pub const NAMES: [&str; 40] = [
        "allyanim", "bite", "bullet", "bypasssub", "cantusetwice", "charge", "contact", "dance", "defrost", "distance",
        "failcopycat", "failencore", "failinstruct", "failmefirst", "failmimic", "futuremove", "gravity", "heal",
        "metronome", "mirror", "mustpressure", "noassist", "nonsky", "noparentalbond", "nosketch", "nosleeptalk",
        "pledgecombo", "powder", "protect", "pulse", "punch", "recharge", "reflectable", "slicing", "snatch", "sound",
        "wind", "kick", "minimize", "nopower",
    ];
    pub fn bit(name: &str) -> u64 {
        NAMES.iter().position(|n| *n == name).map(|p| 1u64 << p).unwrap_or(0)
    }
    pub const BITE: u64 = 1 << 1;
    pub const BULLET: u64 = 1 << 2;
    pub const BYPASSSUB: u64 = 1 << 3;
    pub const CANTUSETWICE: u64 = 1 << 4;
    pub const CHARGE: u64 = 1 << 5;
    pub const CONTACT: u64 = 1 << 6;
    pub const DANCE: u64 = 1 << 7;
    pub const DEFROST: u64 = 1 << 8;
    pub const FAILENCORE: u64 = 1 << 11;
    pub const FAILINSTRUCT: u64 = 1 << 12;
    pub const FUTUREMOVE: u64 = 1 << 15;
    pub const HEAL: u64 = 1 << 17;
    pub const MIRROR: u64 = 1 << 19;
    pub const MUSTPRESSURE: u64 = 1 << 20;
    pub const NOSLEEPTALK: u64 = 1 << 25;
    pub const POWDER: u64 = 1 << 27;
    pub const PROTECT: u64 = 1 << 28;
    pub const PULSE: u64 = 1 << 29;
    pub const PUNCH: u64 = 1 << 30;
    pub const RECHARGE: u64 = 1 << 31;
    pub const REFLECTABLE: u64 = 1 << 32;
    pub const SLICING: u64 = 1 << 33;
    pub const SNATCH: u64 = 1 << 34;
    pub const SOUND: u64 = 1 << 35;
    pub const WIND: u64 = 1 << 36;
}

/// A hit effect (a move's secondary, or its `self` effect): the parts of a move that a
/// secondary can carry.
#[derive(Clone, Debug, Default)]
pub struct HitEffectData {
    /// None = always
    pub chance: Option<i32>,
    pub boosts: Option<SparseBoosts>,
    pub status: Option<EffId>,
    pub volatile_status: Option<EffId>,
    pub side_condition: Option<EffId>,
    pub self_: Option<Box<HitEffectData>>,
    /// for Shell Side Arm-style custom secondaries: the effect has an onHit function
    pub has_on_hit: bool,
    /// Ability source for e.g. a secondary created by an ability (Poison Touch). Unused for data.
    pub dustproof: bool,
    pub kingsrock: bool,
}

impl HitEffectData {
    fn parse(d: &Data, v: &Value) -> HitEffectData {
        HitEffectData {
            chance: v["chance"].as_f64().map(|x| x as i32),
            boosts: SparseBoosts::from_json(&v["boosts"]),
            status: v["status"].as_str().and_then(|s| d.cond(s)),
            volatile_status: v["volatileStatus"].as_str().and_then(|s| d.cond(&to_id(s))),
            side_condition: v["sideCondition"].as_str().and_then(|s| d.cond(&to_id(s))),
            self_: if v["self"].is_object() { Some(Box::new(HitEffectData::parse(d, &v["self"]))) } else { None },
            has_on_hit: false,
            dustproof: v["dustproof"].as_bool().unwrap_or(false),
            kingsrock: v["kingsrock"].as_bool().unwrap_or(false),
        }
    }
}

/// `true` (bypass all) or per-type immunity bypass
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IgnoreImmunity {
    No,
    All,
    /// bit per type id
    Types(u32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FixedDmg {
    None,
    Level,
    Amount(i32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelfSwitch {
    No,
    Yes,
    CopyVolatile,
    ShedTail,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SelfDestruct {
    No,
    Always,
    IfHit,
}

#[derive(Clone, Debug)]
pub struct MoveDef {
    pub id: String,
    pub name: String,
    pub eff: EffId,
    pub num: i32,
    pub type_: TypeId,
    pub category: Category,
    pub base_power: f64,
    /// None = `true` (never misses)
    pub accuracy: Option<f64>,
    pub priority: f64,
    pub target: MoveTarget,
    pub flags: u64,
    pub crit_ratio: i32,
    pub will_crit: Option<bool>,
    pub pp: i32,
    pub no_pp_boosts: bool,
    pub secondaries: Option<Vec<HitEffectData>>,
    pub self_: Option<HitEffectData>,
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
    pub ignore_immunity: IgnoreImmunity,
    pub ignore_ability: bool,
    pub ignore_defensive: bool,
    pub ignore_offensive: bool,
    pub ignore_negative_offensive: bool,
    pub ignore_positive_defensive: bool,
    pub ignore_evasion: bool,
    pub ignore_accuracy: bool,
    pub multihit: Option<(u8, u8)>,
    pub multiaccuracy: bool,
    pub ohko: bool,
    pub damage: FixedDmg,
    pub override_offensive_stat: Option<u8>,
    pub override_offensive_target: bool,
    pub override_defensive_stat: Option<u8>,
    pub override_defensive_source: bool,
    pub sleep_usable: bool,
    pub smart_target: bool,
    pub tracks_target: bool,
    pub steals_boosts: bool,
    pub struggle_recoil: bool,
    pub mind_blown_recoil: bool,
    pub chloroblast_recoil: bool,
    pub has_crash_damage: bool,
    pub thaws_target: bool,
    pub force_stab: bool,
    pub stalling_move: bool,
    pub calls_move: bool,
    pub has_sheer_force_boost: bool,
    pub base_move_type: TypeId,
}

#[derive(Clone, Debug)]
pub struct SpeciesDef {
    pub id: String,
    pub name: String,
    pub base_species: String,
    pub forme: String,
    pub num: i32,
    pub types: Vec<TypeId>,
    pub base: [i32; 6],
    /// ability slots '0', '1', 'H', 'S' (names)
    pub abilities: Vec<(String, String)>,
    pub weighthg: i32,
    pub is_mega: bool,
    pub required_item: Option<String>,
    /// 'M', 'F', 'N' or '' (random)
    pub gender: String,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Nature {
    pub plus: Option<usize>,
    pub minus: Option<usize>,
}

pub fn to_id(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect()
}

pub struct Data {
    pub type_names: Vec<String>,
    type_index: HashMap<String, TypeId>,
    /// keys of damageTaken that are not types (status / weather immunities: brn, par, sandstorm, powder...)
    pub imm_keys: Vec<String>,
    /// taken[defending type][key]: damageTaken code (0 normal, 1 weak, 2 resist, 3 immune).
    /// key index: type ids first, then imm_keys
    pub taken: Vec<Vec<u8>>,
    pub species: Vec<SpeciesDef>,
    species_index: HashMap<String, usize>,
    pub moves: Vec<MoveDef>,
    move_index: HashMap<String, usize>,
    pub natures: HashMap<String, Nature>,
    pub effects: Vec<Effect>,
    move_eff: HashMap<String, EffId>,
    ability_eff: HashMap<String, EffId>,
    item_eff: HashMap<String, EffId>,
    cond_eff: HashMap<String, EffId>,
    pub cb_names: Vec<String>,
    cb_index: HashMap<String, CbId>,
    /// per event: callback ids of on{E}, onAlly{E}, onFoe{E}, onAny{E}, onSource{E}, onSide{E}, onField{E}
    pub ev_cbs: Vec<[CbId; 7]>,
    /// conditions that are used a lot
    pub c: CommonConds,
    /// handlers that the data has but Rust doesn't implement: "kind id callback"
    pub missing: Vec<String>,
}

/// Effect ids looked up once.
#[derive(Clone, Debug, Default)]
pub struct CommonConds {
    pub brn: EffId,
    pub par: EffId,
    pub slp: EffId,
    pub frz: EffId,
    pub psn: EffId,
    pub tox: EffId,
    pub confusion: EffId,
    pub flinch: EffId,
    pub trapped: EffId,
    pub partiallytrapped: EffId,
    pub lockedmove: EffId,
    pub twoturnmove: EffId,
    pub choicelock: EffId,
    pub mustrecharge: EffId,
    pub stall: EffId,
    pub protect: EffId,
    pub substitute: EffId,
    pub trickroom: EffId,
    pub tailwind: EffId,
    pub struggle_move: u16,
    pub recharge_move: u16,
    pub recoil: EffId,
    pub drain: EffId,
    pub empty: EffId,
}

pub const PREFIXES: [&str; 7] = ["", "Ally", "Foe", "Any", "Source", "Side", "Field"];
pub const P_ON: usize = 0;
pub const P_ALLY: usize = 1;
pub const P_FOE: usize = 2;
pub const P_ANY: usize = 3;
pub const P_SOURCE: usize = 4;
pub const P_SIDE: usize = 5;
pub const P_FIELD: usize = 6;

impl Data {
    pub fn load(dex_path: &str, effects_path: &str) -> Result<Data, String> {
        let read = |p: &str| -> Result<Value, String> {
            let text = std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?;
            serde_json::from_str(&text).map_err(|e| format!("{p}: {e}"))
        };
        let dv = read(dex_path)?;
        let ev = read(effects_path)?;

        // ---- types
        let types = dv["types"].as_object().ok_or("no types")?;
        let mut type_names: Vec<String> = types.keys().cloned().collect();
        type_names.sort();
        let type_index: HashMap<String, TypeId> =
            type_names.iter().enumerate().map(|(i, n)| (n.clone(), i as TypeId)).collect();
        let mut imm_keys: Vec<String> = vec![];
        for (_, taken) in types {
            for k in taken.as_object().unwrap().keys() {
                if !type_index.contains_key(k) && !imm_keys.contains(k) {
                    imm_keys.push(k.clone());
                }
            }
        }
        let nkeys = type_names.len() + imm_keys.len();
        let mut taken = vec![vec![0u8; nkeys]; type_names.len()];
        for (def_name, t) in types {
            let d = type_index[def_name] as usize;
            for (k, code) in t.as_object().unwrap() {
                let ki = match type_index.get(k) {
                    Some(&a) => a as usize,
                    None => type_names.len() + imm_keys.iter().position(|x| x == k).unwrap(),
                };
                taken[d][ki] = code.as_u64().unwrap_or(0) as u8;
            }
        }

        let mut data = Data {
            type_names,
            type_index,
            imm_keys,
            taken,
            species: vec![],
            species_index: HashMap::new(),
            moves: vec![],
            move_index: HashMap::new(),
            natures: HashMap::new(),
            effects: vec![],
            move_eff: HashMap::new(),
            ability_eff: HashMap::new(),
            item_eff: HashMap::new(),
            cond_eff: HashMap::new(),
            cb_names: vec![],
            cb_index: HashMap::new(),
            ev_cbs: vec![],
            c: CommonConds::default(),
            missing: vec![],
        };

        // ---- species
        for (id, s) in dv["species"].as_object().unwrap() {
            let mut base = [0i32; 6];
            for (k, val) in s["baseStats"].as_object().unwrap() {
                if let Some(i) = STAT_NAMES.iter().position(|x| x == k) {
                    base[i] = val.as_i64().unwrap_or(0) as i32;
                }
            }
            let types: Vec<TypeId> = s["types"].as_array().unwrap().iter().map(|t| data.type_id(t.as_str().unwrap())).collect();
            let abilities = s["abilities"].as_object().map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect()).unwrap_or_default();
            data.species_index.insert(id.clone(), data.species.len());
            data.species.push(SpeciesDef {
                id: id.clone(),
                name: s["name"].as_str().unwrap_or("").to_string(),
                base_species: s["baseSpecies"].as_str().unwrap_or("").to_string(),
                forme: s["forme"].as_str().unwrap_or("").to_string(),
                num: s["num"].as_i64().unwrap_or(0) as i32,
                types,
                base,
                abilities,
                weighthg: (s["weightkg"].as_f64().unwrap_or(0.0) * 10.0).round() as i32,
                is_mega: s["isMega"].as_bool().unwrap_or(false),
                required_item: s["requiredItem"].as_str().map(String::from),
                gender: s["gender"].as_str().unwrap_or("").to_string(),
            });
        }
        for (id, n) in dv["natures"].as_object().unwrap() {
            data.natures.insert(id.clone(), Nature {
                plus: n["plus"].as_str().and_then(|s| STAT_NAMES.iter().position(|x| *x == s)),
                minus: n["minus"].as_str().and_then(|s| STAT_NAMES.iter().position(|x| *x == s)),
            });
        }

        // ---- effects: register all ids first (conditions are referenced by moves)
        let kinds = [("moves", Kind::Move), ("abilities", Kind::Ability), ("items", Kind::Item), ("conditions", Kind::Condition)];
        for (key, kind) in kinds {
            for (id, e) in ev[key].as_object().unwrap() {
                let eid = data.effects.len() as EffId;
                let effect_type = match e["effectType"].as_str().unwrap_or("") {
                    "Move" => EffType::Move,
                    "Ability" => EffType::Ability,
                    "Item" => EffType::Item,
                    "Condition" => EffType::Condition,
                    "Status" => EffType::Status,
                    "Weather" => EffType::Weather,
                    "Terrain" => EffType::Terrain,
                    _ => EffType::Other,
                };
                let flags = &e["flags"];
                data.effects.push(Effect {
                    kind,
                    id: id.clone(),
                    name: e["name"].as_str().unwrap_or("").to_string(),
                    fullname: e["fullname"].as_str().unwrap_or(e["name"].as_str().unwrap_or("")).to_string(),
                    effect_type,
                    num: e["num"].as_i64().unwrap_or(0) as i32,
                    handlers: vec![],
                    duration: e["duration"].as_i64().map(|x| x as i32),
                    duration_cb: e["handlers"]["durationCallback"].is_object(),
                    no_copy: e["noCopy"].as_bool().unwrap_or(false),
                    affects_fainted: e["affectsFainted"].as_bool().unwrap_or(false),
                    counter_max: e["counterMax"].as_i64().unwrap_or(0) as i32,
                    item: if kind == Kind::Item {
                        Some(Box::new(ItemInfo {
                            is_berry: e["isBerry"].as_bool().unwrap_or(false),
                            is_choice: e["isChoice"].as_bool().unwrap_or(false),
                            is_gem: e["isGem"].as_bool().unwrap_or(false),
                            ignore_klutz: e["ignoreKlutz"].as_bool().unwrap_or(false),
                            mega_stone: e["megaStone"].as_object().map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect()).unwrap_or_default(),
                            fling_bp: e["fling"]["basePower"].as_u64().unwrap_or(0) as u16,
                            natural_gift: None,
                            boosts: SparseBoosts::from_json(&e["boosts"]),
                            is_primal_orb: e["isPrimalOrb"].as_bool().unwrap_or(false),
                            z_move: !e["zMove"].is_null() && e["zMove"].as_bool() != Some(false),
                        }))
                    } else {
                        None
                    },
                    ability: if kind == Kind::Ability {
                        Some(Box::new(AbilityInfo {
                            breakable: flags["breakable"].is_number(),
                            cantsuppress: flags["cantsuppress"].is_number(),
                            failroleplay: flags["failroleplay"].is_number(),
                            failskillswap: flags["failskillswap"].is_number(),
                            noreceiver: flags["noreceiver"].is_number(),
                            notrace: flags["notrace"].is_number(),
                            notransform: flags["notransform"].is_number(),
                            suppress_weather: e["suppressWeather"].as_bool().unwrap_or(false),
                        }))
                    } else {
                        None
                    },
                    move_def: u16::MAX,
                });
                match kind {
                    Kind::Move => data.move_eff.insert(id.clone(), eid),
                    Kind::Ability => data.ability_eff.insert(id.clone(), eid),
                    Kind::Item => data.item_eff.insert(id.clone(), eid),
                    Kind::Condition => data.cond_eff.insert(id.clone(), eid),
                };
            }
        }
        // the empty condition ('' id: no status / no weather...)
        let empty = data.effects.len() as EffId;
        data.effects.push(Effect {
            kind: Kind::Condition, id: String::new(), name: String::new(), fullname: String::new(),
            effect_type: EffType::Condition, num: 0, handlers: vec![], duration: None, duration_cb: false,
            no_copy: false, affects_fainted: false, counter_max: 0, item: None, ability: None, move_def: u16::MAX,
        });
        // pseudo-effects: "recoil" / "drain" (dex.conditions.getByID), the confusion self-hit
        // ({id: 'confused', effectType: 'Move'}) and Struggle's recoil ({id: 'strugglerecoil'})
        for (id, name, et) in [("recoil", "Recoil", EffType::Condition), ("drain", "Drain", EffType::Condition),
            ("confused", "", EffType::Move), ("strugglerecoil", "", EffType::Other), ("fnt", "fnt", EffType::Other),
            // species used as an effect (Disguise's chip damage): fullname "pokemon: <name>"
            ("species:mimikyubusted", "Mimikyu-Busted", EffType::Other),
            ("species:mimikyubustedtotem", "Mimikyu-Busted-Totem", EffType::Other),
            // Synchronize's sourceEffect {status, id: 'synchronize'}: no name, a status
            ("synchronize:brn", "", EffType::Other), ("synchronize:par", "", EffType::Other),
            ("synchronize:psn", "", EffType::Other), ("synchronize:tox", "", EffType::Other)] {
            let eid = data.effects.len() as EffId;
            data.effects.push(Effect {
                kind: Kind::Condition, id: id.to_string(), name: name.to_string(),
                fullname: if id.starts_with("species:") { format!("pokemon: {name}") } else { name.to_string() },
                effect_type: et, num: 0, handlers: vec![], duration: None, duration_cb: false,
                no_copy: false, affects_fainted: false, counter_max: 0, item: None, ability: None, move_def: u16::MAX,
            });
            data.cond_eff.insert(id.to_string(), eid);
        }
        data.c.empty = empty;

        // ---- handlers
        let mut all: Vec<(EffId, String, Value)> = vec![];
        for (key, kind) in kinds {
            for (id, e) in ev[key].as_object().unwrap() {
                let eid = data.eff_of(kind, id).unwrap();
                for (cbname, h) in e["handlers"].as_object().unwrap() {
                    all.push((eid, cbname.trim_start_matches('#').to_string(), h.clone()));
                }
            }
        }
        // intern callback names: everything in the data plus every prefixed event name
        let mut names: Vec<String> = all.iter().map(|x| x.1.clone()).collect();
        for ev_name in EV_NAMES {
            for p in PREFIXES {
                names.push(format!("on{p}{ev_name}"));
            }
        }
        names.sort();
        names.dedup();
        for n in names {
            data.cb_index.insert(n.clone(), data.cb_names.len() as CbId);
            data.cb_names.push(n);
        }
        for ev_name in EV_NAMES {
            let mut ids = [NO_CB; 7];
            for (k, p) in PREFIXES.iter().enumerate() {
                ids[k] = data.cb_index[&format!("on{p}{ev_name}")];
            }
            data.ev_cbs.push(ids);
        }
        for (eid, cbname, h) in all {
            let kind = match h["kind"].as_str() {
                Some("value") => HandlerKind::Value(Val::from_json(&h["value"])),
                Some("none") => HandlerKind::MetaOnly,
                _ => HandlerKind::Fn,
            };
            let e = &mut data.effects[eid as usize];
            e.handlers.push(HandlerMeta {
                cb: data.cb_index[&cbname],
                order: h["order"].as_f64(),
                priority: h["priority"].as_f64().unwrap_or(0.0),
                sub_order: h["subOrder"].as_f64().unwrap_or(0.0),
                kind,
                imp: None,
            });
        }
        if let Some(eid) = data.eff_of(Kind::Move, "fling") {
            if let Some(&cb) = data.cb_index.get("onHit") {
                data.effects[eid as usize].handlers.push(HandlerMeta { cb, order: None, priority: 0.0, sub_order: 0.0, kind: HandlerKind::Fn, imp: None });
            }
        }
        for e in &mut data.effects {
            e.handlers.sort_by_key(|h| h.cb);
        }

        // ---- moves (after conditions exist)
        let moves_json = dv["moves"].as_object().unwrap().clone();
        for (id, m) in &moves_json {
            let Some(eff) = data.eff_of(Kind::Move, id) else { continue };
            let def = data.parse_move(id, m, eff);
            let mi = data.moves.len() as u16;
            data.move_index.insert(id.clone(), mi as usize);
            data.effects[eff as usize].move_def = mi;
            data.moves.push(def);
        }
        // 'recharge' is not a real move (dex.moves.get('recharge') does not exist): Showdown still
        // resolves an action for it while a Pokemon must recharge
        {
            let eid = data.effects.len() as EffId;
            data.effects.push(Effect {
                kind: Kind::Move, id: "recharge".into(), name: "recharge".into(), fullname: "move: recharge".into(),
                effect_type: EffType::Move, num: 0, handlers: vec![], duration: None, duration_cb: false,
                no_copy: false, affects_fainted: false, counter_max: 0, item: None, ability: None, move_def: data.moves.len() as u16,
            });
            let mut def = data.moves[data.move_index["struggle"]].clone();
            def.id = "recharge".into();
            def.name = "recharge".into();
            def.eff = eid;
            def.target = MoveTarget::Undefined;
            def.priority = 0.0;
            def.flags = 0;
            def.base_power = 0.0;
            def.category = Category::Status;
            def.struggle_recoil = false;
            def.self_ = None;
            def.secondaries = None;
            data.move_index.insert("recharge".into(), data.moves.len());
            data.moves.push(def);
        }
        // dex.conditions.get(id) for an id with no condition returns an empty condition named id
        // (charge moves add such a volatile; Steel Beam / High Jump Kick damage uses one): give
        // every move id without a condition such an empty effect
        for mi in 0..data.moves.len() {
            let id = data.moves[mi].id.clone();
            if data.cond_eff.contains_key(&id) {
                continue;
            }
            let eid = data.effects.len() as EffId;
            data.effects.push(Effect {
                kind: Kind::Condition, id: id.clone(), name: id.clone(), fullname: id.clone(),
                effect_type: EffType::Condition, num: 0, handlers: vec![], duration: None, duration_cb: false,
                no_copy: false, affects_fainted: false, counter_max: 0, item: None, ability: None, move_def: u16::MAX,
            });
            data.cond_eff.insert(id, eid);
        }
        // secondaries with an onHit function (secondary.onHit, self.onHit)
        for (id, e) in ev["moves"].as_object().unwrap() {
            if let Some(&mi) = data.move_index.get(id) {
                if let Some(hs) = e["secondariesHandlers"].as_array() {
                    for (k, h) in hs.iter().enumerate() {
                        if h["onHit"].is_object() {
                            if let Some(secs) = &mut data.moves[mi].secondaries {
                                if let Some(s) = secs.get_mut(k) {
                                    s.has_on_hit = true;
                                }
                            }
                        }
                    }
                }
                if e["selfHandlers"]["onHit"].is_object() {
                    if let Some(s) = &mut data.moves[mi].self_ {
                        s.has_on_hit = true;
                    }
                }
            }
        }

        let c = |d: &Data, id: &str| d.cond(id).unwrap_or(NO_EFF);
        data.c = CommonConds {
            brn: c(&data, "brn"),
            par: c(&data, "par"),
            slp: c(&data, "slp"),
            frz: c(&data, "frz"),
            psn: c(&data, "psn"),
            tox: c(&data, "tox"),
            confusion: c(&data, "confusion"),
            flinch: c(&data, "flinch"),
            trapped: c(&data, "trapped"),
            partiallytrapped: c(&data, "partiallytrapped"),
            lockedmove: c(&data, "lockedmove"),
            twoturnmove: c(&data, "twoturnmove"),
            choicelock: c(&data, "choicelock"),
            mustrecharge: c(&data, "mustrecharge"),
            stall: c(&data, "stall"),
            protect: c(&data, "protect"),
            substitute: c(&data, "substitute"),
            trickroom: c(&data, "trickroom"),
            tailwind: c(&data, "tailwind"),
            struggle_move: data.move_index.get("struggle").copied().unwrap_or(0) as u16,
            recharge_move: data.move_index.get("recharge").copied().unwrap_or(0) as u16,
            recoil: c(&data, "recoil"),
            drain: c(&data, "drain"),
            empty,
        };
        Ok(data)
    }

    fn parse_move(&self, id: &str, m: &Value, eff: EffId) -> MoveDef {
        let frac = |v: &Value| -> Option<(f64, f64)> {
            let a = v.as_array()?;
            Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?))
        };
        let mut flags = 0u64;
        if let Some(f) = m["flags"].as_object() {
            for k in f.keys() {
                flags |= mflag::bit(k);
            }
        }
        let stat = |v: &Value| v.as_str().and_then(|s| STAT_NAMES.iter().position(|x| *x == s)).map(|x| x as u8);
        let ignore_immunity = match &m["ignoreImmunity"] {
            Value::Bool(true) => IgnoreImmunity::All,
            Value::Object(o) => {
                let mut bits = 0u32;
                for (k, v) in o {
                    if v.as_bool() == Some(true) {
                        if let Some(&t) = self.type_index.get(k) {
                            bits |= 1 << t;
                        }
                    }
                }
                IgnoreImmunity::Types(bits)
            }
            _ => IgnoreImmunity::No,
        };
        let cond = |v: &Value| v.as_str().and_then(|s| self.cond(&to_id(s)));
        MoveDef {
            id: id.to_string(),
            name: m["name"].as_str().unwrap_or("").to_string(),
            eff,
            num: 0,
            type_: self.type_id(m["type"].as_str().unwrap_or("")),
            category: match m["category"].as_str() {
                Some("Physical") => Category::Physical,
                Some("Special") => Category::Special,
                _ => Category::Status,
            },
            base_power: m["basePower"].as_f64().unwrap_or(0.0),
            accuracy: if m["accuracy"].as_bool() == Some(true) { None } else { Some(m["accuracy"].as_f64().unwrap_or(0.0)) },
            priority: m["priority"].as_f64().unwrap_or(0.0),
            target: MoveTarget::parse(m["target"].as_str().unwrap_or("")),
            flags,
            crit_ratio: m["critRatio"].as_i64().unwrap_or(1) as i32,
            will_crit: m["willCrit"].as_bool(),
            pp: m["pp"].as_i64().unwrap_or(0) as i32,
            no_pp_boosts: m["noPPBoosts"].as_bool().unwrap_or(false),
            secondaries: m["secondaries"].as_array().map(|a| a.iter().map(|x| HitEffectData::parse(self, x)).collect()),
            self_: if m["self"].is_object() { Some(HitEffectData::parse(self, &m["self"])) } else { None },
            self_boost: SparseBoosts::from_json(&m["selfBoost"]["boosts"]),
            boosts: SparseBoosts::from_json(&m["boosts"]),
            status: cond(&m["status"]),
            volatile_status: cond(&m["volatileStatus"]),
            side_condition: cond(&m["sideCondition"]),
            slot_condition: cond(&m["slotCondition"]),
            weather: cond(&m["weather"]),
            terrain: cond(&m["terrain"]),
            pseudo_weather: cond(&m["pseudoWeather"]),
            heal: frac(&m["heal"]),
            drain: frac(&m["drain"]),
            recoil: frac(&m["recoil"]),
            self_switch: match &m["selfSwitch"] {
                Value::Bool(true) => SelfSwitch::Yes,
                Value::String(s) if s == "copyvolatile" => SelfSwitch::CopyVolatile,
                Value::String(s) if s == "shedtail" => SelfSwitch::ShedTail,
                Value::String(_) => SelfSwitch::Yes,
                _ => SelfSwitch::No,
            },
            force_switch: m["forceSwitch"].as_bool().unwrap_or(false),
            selfdestruct: match m["selfdestruct"].as_str() {
                Some("always") => SelfDestruct::Always,
                Some("ifHit") => SelfDestruct::IfHit,
                _ => SelfDestruct::No,
            },
            breaks_protect: m["breaksProtect"].as_bool().unwrap_or(false),
            ignore_immunity,
            ignore_ability: m["ignoreAbility"].as_bool().unwrap_or(false),
            ignore_defensive: m["ignoreDefensive"].as_bool().unwrap_or(false),
            ignore_offensive: m["ignoreOffensive"].as_bool().unwrap_or(false),
            ignore_negative_offensive: m["ignoreNegativeOffensive"].as_bool().unwrap_or(false),
            ignore_positive_defensive: m["ignorePositiveDefensive"].as_bool().unwrap_or(false),
            ignore_evasion: m["ignoreEvasion"].as_bool().unwrap_or(false),
            ignore_accuracy: m["ignoreAccuracy"].as_bool().unwrap_or(false),
            multihit: match &m["multihit"] {
                Value::Number(x) => Some((x.as_u64().unwrap() as u8, x.as_u64().unwrap() as u8)),
                Value::Array(a) => Some((a[0].as_u64().unwrap() as u8, a[1].as_u64().unwrap() as u8)),
                _ => None,
            },
            multiaccuracy: m["multiaccuracy"].as_bool().unwrap_or(false),
            ohko: m["ohko"].as_bool().unwrap_or(false) || m["ohko"].is_string(),
            damage: if m["damage"].as_str() == Some("level") {
                FixedDmg::Level
            } else if let Some(n) = m["damage"].as_i64() {
                FixedDmg::Amount(n as i32)
            } else {
                FixedDmg::None
            },
            override_offensive_stat: stat(&m["overrideOffensiveStat"]),
            override_offensive_target: m["overrideOffensivePokemon"].as_str() == Some("target"),
            override_defensive_stat: stat(&m["overrideDefensiveStat"]),
            override_defensive_source: m["overrideDefensivePokemon"].as_str() == Some("source"),
            sleep_usable: m["sleepUsable"].as_bool().unwrap_or(false),
            smart_target: m["smartTarget"].as_bool().unwrap_or(false),
            tracks_target: m["tracksTarget"].as_bool().unwrap_or(false),
            steals_boosts: m["stealsBoosts"].as_bool().unwrap_or(false),
            struggle_recoil: m["struggleRecoil"].as_bool().unwrap_or(false),
            mind_blown_recoil: m["mindBlownRecoil"].as_bool().unwrap_or(false),
            chloroblast_recoil: m["chloroblastRecoil"].as_bool().unwrap_or(false),
            has_crash_damage: m["hasCrashDamage"].as_bool().unwrap_or(false),
            thaws_target: m["thawsTarget"].as_bool().unwrap_or(false),
            force_stab: m["forceSTAB"].as_bool().unwrap_or(false),
            stalling_move: m["stallingMove"].as_bool().unwrap_or(false),
            calls_move: m["callsMove"].as_bool().unwrap_or(false),
            has_sheer_force_boost: m["hasSheerForceBoost"].as_bool().unwrap_or(false),
            base_move_type: self.type_id(m["baseMoveType"].as_str().unwrap_or("")),
        }
    }

    pub fn type_id(&self, name: &str) -> TypeId {
        *self.type_index.get(name).unwrap_or(&NO_TYPE)
    }
    pub fn type_name(&self, t: TypeId) -> &str {
        if t == NO_TYPE { "???" } else { &self.type_names[t as usize] }
    }
    pub fn species_id(&self, id: &str) -> Option<usize> {
        self.species_index.get(&to_id(id)).copied()
    }
    pub fn move_id(&self, id: &str) -> Option<u16> {
        self.move_index.get(&to_id(id)).map(|&x| x as u16)
    }
    pub fn eff_of(&self, kind: Kind, id: &str) -> Option<EffId> {
        match kind {
            Kind::Move => self.move_eff.get(id),
            Kind::Ability => self.ability_eff.get(id),
            Kind::Item => self.item_eff.get(id),
            Kind::Condition => self.cond_eff.get(id),
        }
        .copied()
    }
    pub fn ability(&self, id: &str) -> EffId {
        if id.is_empty() { self.c.empty } else { self.ability_eff.get(&to_id(id)).copied().unwrap_or(self.c.empty) }
    }
    pub fn item(&self, id: &str) -> EffId {
        if id.is_empty() { self.c.empty } else { self.item_eff.get(&to_id(id)).copied().unwrap_or(self.c.empty) }
    }
    /// dex.conditions.getByID
    pub fn cond(&self, id: &str) -> Option<EffId> {
        self.cond_eff.get(id).copied()
    }
    pub fn nature(&self, id: &str) -> Nature {
        self.natures.get(&to_id(id)).copied().unwrap_or_default()
    }
    #[inline]
    pub fn eff(&self, e: EffId) -> &Effect {
        &self.effects[e as usize]
    }
    #[inline]
    pub fn cbs(&self, ev: Ev) -> &[CbId; 7] {
        &self.ev_cbs[ev as usize]
    }
    pub fn cb_name(&self, cb: CbId) -> &str {
        &self.cb_names[cb as usize]
    }
    pub fn cb_id(&self, name: &str) -> CbId {
        self.cb_index.get(name).copied().unwrap_or(NO_CB)
    }

    /// damageTaken code of `key` (a type name or a status/weather id) against one defending type
    pub fn taken_code(&self, key: &str, def: TypeId) -> u8 {
        if def == NO_TYPE {
            return 0;
        }
        let ki = match self.type_index.get(key) {
            Some(&t) => t as usize,
            None => match self.imm_keys.iter().position(|x| x == key) {
                Some(p) => self.type_names.len() + p,
                None => return 0,
            },
        };
        self.taken[def as usize][ki]
    }
    pub fn taken_type(&self, att: TypeId, def: TypeId) -> u8 {
        if att == NO_TYPE || def == NO_TYPE {
            return 0;
        }
        self.taken[def as usize][att as usize]
    }

    /// Attach the Rust implementations; list the handlers that have none.
    pub fn bind_handlers(&mut self, lookup: fn(Kind, &str, &str) -> Option<HandlerFn>) {
        self.missing.clear();
        for ei in 0..self.effects.len() {
            for hi in 0..self.effects[ei].handlers.len() {
                let (kind, id) = (self.effects[ei].kind, self.effects[ei].id.clone());
                let cb = self.effects[ei].handlers[hi].cb;
                let cbname = self.cb_names[cb as usize].clone();
                let imp = lookup(kind, &id, &cbname);
                if imp.is_none() && matches!(self.effects[ei].handlers[hi].kind, HandlerKind::Fn) {
                    self.missing.push(format!("{kind:?} {id} {cbname}"));
                }
                self.effects[ei].handlers[hi].imp = imp;
            }
        }
    }
}
