//! Game data, loaded from the JSON that `tools/export_dex.js` writes from Showdown's Dex
//! (for one format, with that format's mod applied).

use std::collections::HashMap;

use serde_json::Value;

use crate::effects::{Ab, It, Mv};

pub type TypeId = u8;
pub const NO_TYPE: TypeId = 255;

pub const HP: usize = 0;
pub const ATK: usize = 1;
pub const DEF: usize = 2;
pub const SPA: usize = 3;
pub const SPD: usize = 4;
pub const SPE: usize = 5;
pub const STAT_IDS: [&str; 6] = ["hp", "atk", "def", "spa", "spd", "spe"];
pub const ACC: usize = 6;
pub const EVA: usize = 7;
/// Stat stages: index 1..=5 = atk, def, spa, spd, spe; 6 = accuracy, 7 = evasion (0 unused)
pub type Boosts = [i8; 8];
pub const BOOST_IDS: [&str; 8] = ["", "atk", "def", "spa", "spd", "spe", "accuracy", "evasion"];

fn parse_boosts(v: &Value) -> Option<Boosts> {
    let obj = v.as_object()?;
    let mut b = [0i8; 8];
    for (k, x) in obj {
        if let Some(i) = BOOST_IDS.iter().position(|n| *n == k) {
            if i > 0 {
                b[i] = x.as_i64().unwrap_or(0) as i8;
            }
        }
    }
    Some(b)
}

fn parse_status(v: &Value) -> Option<crate::damage::Status> {
    use crate::damage::Status::*;
    Some(match v.as_str()? {
        "brn" => Burn,
        "par" => Paralysis,
        "psn" => Poison,
        "tox" => Toxic,
        "slp" => Sleep,
        "frz" => Freeze,
        _ => return Option::None,
    })
}

fn parse_fraction(v: &Value) -> Option<(u16, u16)> {
    let a = v.as_array()?;
    Some((a.first()?.as_u64()? as u16, a.get(1)?.as_u64()? as u16))
}

/// One secondary effect of a move.
#[derive(Clone, Debug)]
pub struct Secondary {
    pub chance: u8,
    pub boosts: Option<Boosts>,
    pub status: Option<crate::damage::Status>,
    pub volatile: Option<String>,
    pub self_boosts: Option<Boosts>,
    /// the effect is custom code (e.g. Dire Claw's random status)
    pub custom: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Physical,
    Special,
    Status,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Normal,
    AllAdjacentFoes,
    AllAdjacent,
    SelfTarget,
    /// a random opponent (e.g. Outrage, Struggle)
    RandomNormal,
    AdjacentAlly,
    AdjacentAllyOrSelf,
    AllySide,
    FoeSide,
    /// the whole field (weather, terrain, Trick Room, Haze, Perish Song)
    All,
    Allies,
    Other,
}

/// Move flags that matter for damage (bits).
pub mod flag {
    pub const CONTACT: u32 = 1;
    pub const PUNCH: u32 = 2;
    pub const BITE: u32 = 4;
    pub const SLICING: u32 = 8;
    pub const PULSE: u32 = 16;
    pub const SOUND: u32 = 32;
    pub const BULLET: u32 = 64;
    pub const POWDER: u32 = 128;
    pub const WIND: u32 = 256;
    pub const DANCE: u32 = 512;
    pub const DEFROST: u32 = 1024;
    pub const BYPASSSUB: u32 = 2048;
    pub const HEAL: u32 = 4096;
    pub const REFLECTABLE: u32 = 8192;
    /// two-turn moves (Solar Beam, Electro Shot...)
    pub const CHARGE: u32 = 16384;
    /// double damage and sure hit against a Minimized target (Body Slam, Heavy Slam...)
    pub const MINIMIZE: u32 = 1 << 15;
    /// Instruct can't repeat it
    pub const FAILINSTRUCT: u32 = 1 << 16;
    /// Copycat can't copy it
    pub const FAILCOPYCAT: u32 = 1 << 17;
    /// Encore fails on it (Copycat, Sleep Talk, Transform, Struggle, Encore...)
    pub const FAILENCORE: u32 = 1 << 18;
    /// Sleep Talk can't call it
    pub const NOSLEEPTALK: u32 = 1 << 19;
}

#[derive(Clone, Debug)]
pub struct Species {
    pub id: String,
    pub name: String,
    pub base_species: String,
    pub types: [TypeId; 2],
    pub base: [u16; 6],
    pub ability0: String,
    pub weight_hg: u32,
    pub is_mega: bool,
    /// number of possible abilities (1 = the ability is public knowledge)
    pub n_abilities: u8,
    /// 0 genderless, 1 always male, 2 always female, 3 either (chosen at random when the battle starts)
    pub gender: u8,
}

#[derive(Clone, Debug)]
pub struct MoveData {
    pub id: String,
    /// battle PP: Champions calculatePP = noPPBoosts ? pp : (pp / 5 + 1) * 4
    pub max_pp: u8,
    pub name: String,
    pub kind: Mv,
    pub type_: TypeId,
    pub category: Category,
    pub base_power: u16,
    pub accuracy: u8, // 0 = never misses
    pub priority: i8,
    pub target: Target,
    pub flags: u32,
    pub has_secondaries: bool,
    pub has_sheer_force_boost: bool,
    pub multihit: (u8, u8),
    /// Body Press: attack with Defense
    pub offensive_stat: Option<usize>,
    /// Psyshock: hit the target's Defense
    pub defensive_stat: Option<usize>,
    /// Foul Play: use the target's Attack
    pub uses_target_attack: bool,
    /// Darkest Lariat, Sacred Sword: ignore the target's defensive boosts
    pub ignore_defensive: bool,
    /// Chip Away, Darkest Lariat, Sacred Sword, Menacing Moonraze...: accuracy ignores the target's evasion stages
    pub ignore_evasion: bool,
    pub fixed: FixedDamage,
    // ---- effects
    pub always_hits: bool,
    pub crit_ratio: u8,
    pub will_crit: bool,
    /// Fake Out / First Impression style: only usable on the first turn out
    pub first_turn_only: bool,
    /// Grassy Glide: priority depends on the terrain
    pub grassy_glide: bool,
    pub secondaries: Vec<Secondary>,
    pub self_boosts: Option<Boosts>,
    /// status moves: stat changes for the target (or the user, for self-targeting moves)
    pub boosts: Option<Boosts>,
    pub status: Option<crate::damage::Status>,
    pub volatile: Option<String>,
    pub self_volatile: Option<String>,
    pub side_condition: Option<String>,
    pub weather: Option<String>,
    pub terrain: Option<String>,
    pub pseudo_weather: Option<String>,
    pub drain: Option<(u16, u16)>,
    pub recoil: Option<(u16, u16)>,
    pub heal: Option<(u16, u16)>,
    pub self_switch: bool,
    pub force_switch: bool,
    pub self_destruct: bool,
    /// selfdestruct: "ifHit" (Memento, Final Gambit): only if the move hit
    pub self_destruct_if_hit: bool,
    pub breaks_protect: bool,
    pub ignore_immunity: bool,
    /// can be blocked by Protect
    pub protectable: bool,
    pub sleep_usable: bool,
    pub multi_accuracy: bool,
    pub hooks: Vec<String>,
}

/// Moves that don't use the damage formula.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixedDamage {
    No,
    /// Seismic Toss, Night Shade: the user's level
    Level,
    Amount(u16),
    /// Super Fang, Ruination: half the target's current HP
    HalfHp,
    /// Final Gambit: the user's HP
    UserHp,
    /// Fissure etc.: the target's max HP
    Ohko,
    /// Endeavor: target HP minus user HP (fails unless the user has less)
    Endeavor,
    /// Metal Burst / Comeuppance (0: last foe hit this turn x1.5), Counter (1: physical x2), Mirror Coat (2: special x2)
    Retaliate(u8),
}

#[derive(Clone, Debug)]
pub struct ItemData {
    pub id: String,
    pub name: String,
    pub kind: It,
    pub is_berry: bool,
    /// For Mega Stones: (base species id, mega species id) pairs.
    pub mega: Vec<(String, String)>,
    /// Fling's base power with this item (0 = can't be flung)
    pub fling_bp: u16,
}

#[derive(Clone, Debug)]
pub struct AbilityData {
    pub id: String,
    pub name: String,
    pub kind: Ab,
    /// Showdown flag cantsuppress (Gastro Acid and Worry Seed fail on it)
    pub cant_suppress: bool,
    /// flags.noreceiver: Receiver / Power of Alchemy can't take it
    pub no_receiver: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Nature {
    pub plus: Option<usize>,
    pub minus: Option<usize>,
}

/// The type ids, looked up once (the hot paths compare ids, not names).
#[derive(Clone, Copy, Debug, Default)]
pub struct TypeIds {
    pub normal: TypeId,
    pub fire: TypeId,
    pub water: TypeId,
    pub electric: TypeId,
    pub grass: TypeId,
    pub ice: TypeId,
    pub fighting: TypeId,
    pub poison: TypeId,
    pub ground: TypeId,
    pub flying: TypeId,
    pub psychic: TypeId,
    pub bug: TypeId,
    pub rock: TypeId,
    pub ghost: TypeId,
    pub dragon: TypeId,
    pub dark: TypeId,
    pub steel: TypeId,
    pub fairy: TypeId,
}

pub struct Dex {
    pub ty: TypeIds,
    pub type_names: Vec<String>,
    type_index: HashMap<String, TypeId>,
    /// chart[attacking][defending]: Showdown's damageTaken codes (0 normal, 1 weak, 2 resist, 3 immune)
    pub chart: Vec<Vec<u8>>,
    pub species: Vec<Species>,
    species_index: HashMap<String, usize>,
    pub moves: Vec<MoveData>,
    move_index: HashMap<String, usize>,
    pub items: Vec<ItemData>,
    item_index: HashMap<String, usize>,
    pub abilities: Vec<AbilityData>,
    ability_index: HashMap<String, usize>,
    natures: HashMap<String, Nature>,
}

pub fn to_id(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_lowercase()).collect()
}

fn stat_index(s: &str) -> Option<usize> {
    STAT_IDS.iter().position(|x| *x == s)
}

static BERRIES: std::sync::OnceLock<Vec<bool>> = std::sync::OnceLock::new();

static BERRY_KINDS: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();

/// What a berry does when eaten, by index into Dex::items: 1 Sitrus, 2 Oran, 3 Lum, 4 Cheri (par), 5 Chesto (slp),
/// 6 Pecha (psn), 7 Rawst (brn), 8 Aspear (frz), 9 Persim (confusion), 10 Leppa; 0 = nothing (resist berries...)
pub fn berry_kind(id: u16) -> u8 {
    BERRY_KINDS.get().and_then(|v| v.get(id as usize)).copied().unwrap_or(0)
}

/// Is item `id` (an index into Dex::items of the loaded dex) a berry?  (consume_item has no Dex at hand)
pub fn item_is_berry(id: u16) -> bool {
    BERRIES.get().and_then(|v| v.get(id as usize)).copied().unwrap_or(false)
}

impl Dex {
    pub fn load(path: &str) -> Result<Dex, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let v: Value = serde_json::from_str(&text).map_err(|e| format!("{path}: {e}"))?;

        // types and chart
        let types = v["types"].as_object().ok_or("no types")?;
        let mut type_names: Vec<String> = types.keys().cloned().collect();
        type_names.sort();
        let type_index: HashMap<String, TypeId> =
            type_names.iter().enumerate().map(|(i, n)| (n.clone(), i as TypeId)).collect();
        let n = type_names.len();
        let mut chart = vec![vec![0u8; n]; n];
        for (def_name, taken) in types {
            let d = type_index[def_name] as usize;
            for (att_name, code) in taken.as_object().unwrap() {
                if let Some(&a) = type_index.get(att_name) {
                    chart[a as usize][d] = code.as_u64().unwrap_or(0) as u8;
                }
            }
        }
        let ty = |name: &str| -> TypeId { *type_index.get(name).unwrap_or(&NO_TYPE) };

        let mut species = Vec::new();
        let mut species_index = HashMap::new();
        for (id, s) in v["species"].as_object().unwrap() {
            let t = s["types"].as_array().unwrap();
            let mut base = [0u16; 6];
            for (k, val) in s["baseStats"].as_object().unwrap() {
                if let Some(i) = stat_index(k) {
                    base[i] = val.as_u64().unwrap_or(0) as u16;
                }
            }
            species_index.insert(id.clone(), species.len());
            species.push(Species {
                id: id.clone(),
                name: s["name"].as_str().unwrap_or("").to_string(),
                base_species: to_id(s["baseSpecies"].as_str().unwrap_or("")),
                types: [
                    ty(t[0].as_str().unwrap()),
                    t.get(1).map(|x| ty(x.as_str().unwrap())).unwrap_or(NO_TYPE),
                ],
                base,
                ability0: to_id(s["abilities"]["0"].as_str().unwrap_or("")),
                n_abilities: s["abilities"].as_object().map(|o| o.len()).unwrap_or(1).max(1) as u8,
                weight_hg: (s["weightkg"].as_f64().unwrap_or(0.0) * 10.0).round() as u32,
                is_mega: s["isMega"].as_bool().unwrap_or(false),
                gender: match s["gender"].as_str().unwrap_or("") { "M" => 1, "F" => 2, "N" => 0, _ => 3 },
            });
        }

        let mut moves = Vec::new();
        let mut move_index = HashMap::new();
        for (id, m) in v["moves"].as_object().unwrap() {
            let category = match m["category"].as_str() {
                Some("Physical") => Category::Physical,
                Some("Special") => Category::Special,
                _ => Category::Status,
            };
            let target = match m["target"].as_str() {
                Some("normal") | Some("any") | Some("adjacentFoe") => Target::Normal,
                Some("randomNormal") => Target::RandomNormal,
                Some("allAdjacentFoes") => Target::AllAdjacentFoes,
                Some("allAdjacent") => Target::AllAdjacent,
                Some("self") => Target::SelfTarget,
                Some("adjacentAlly") => Target::AdjacentAlly,
                Some("adjacentAllyOrSelf") => Target::AdjacentAllyOrSelf,
                Some("allySide") | Some("allyTeam") => Target::AllySide,
                Some("foeSide") => Target::FoeSide,
                Some("all") => Target::All,
                Some("allies") => Target::Allies,
                _ => Target::Other,
            };
            let mut flags = 0;
            if let Some(f) = m["flags"].as_object() {
                for (k, bit) in [
                    ("contact", flag::CONTACT),
                    ("punch", flag::PUNCH),
                    ("bite", flag::BITE),
                    ("slicing", flag::SLICING),
                    ("pulse", flag::PULSE),
                    ("sound", flag::SOUND),
                    ("bullet", flag::BULLET),
                    ("powder", flag::POWDER),
                    ("wind", flag::WIND),
                    ("dance", flag::DANCE),
                    ("defrost", flag::DEFROST),
                    ("bypasssub", flag::BYPASSSUB),
                    ("heal", flag::HEAL),
                    ("reflectable", flag::REFLECTABLE),
                    ("charge", flag::CHARGE),
                    ("minimize", flag::MINIMIZE),
                    ("failinstruct", flag::FAILINSTRUCT),
                    ("failcopycat", flag::FAILCOPYCAT),
                    ("failencore", flag::FAILENCORE),
                    ("nosleeptalk", flag::NOSLEEPTALK),
                ] {
                    if f.get(k).is_some() {
                        flags |= bit;
                    }
                }
            }
            let multihit = match &m["multihit"] {
                Value::Number(x) => (x.as_u64().unwrap() as u8, x.as_u64().unwrap() as u8),
                Value::Array(a) => (a[0].as_u64().unwrap() as u8, a[1].as_u64().unwrap() as u8),
                _ => (1, 1),
            };
            let hooks: Vec<&str> = m["hooks"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
            let fixed = if m["ohko"].as_bool().unwrap_or(false) || m["ohko"].is_string() {
                FixedDamage::Ohko
            } else if m["damage"].as_str() == Some("level") {
                FixedDamage::Level
            } else if let Some(n) = m["damage"].as_u64() {
                FixedDamage::Amount(n as u16)
            } else if hooks.contains(&"damageCallback") && matches!(id.as_str(), "superfang" | "ruination" | "naturesmadness") {
                FixedDamage::HalfHp
            } else if matches!(id.as_str(), "metalburst" | "comeuppance") {
                FixedDamage::Retaliate(0)
            } else if id == "counter" {
                FixedDamage::Retaliate(1)
            } else if id == "mirrorcoat" {
                FixedDamage::Retaliate(2)
            } else if id == "endeavor" {
                FixedDamage::Endeavor
            } else if id == "finalgambit" {
                FixedDamage::UserHp
            } else {
                FixedDamage::No
            };
            move_index.insert(id.clone(), moves.len());
            let base_pp = m["pp"].as_u64().unwrap_or(20) as u32;
            let max_pp = if m["noPPBoosts"].as_bool().unwrap_or(false) { base_pp } else { (base_pp / 5 + 1) * 4 }.clamp(1, 64) as u8;
            moves.push(MoveData {
                id: id.clone(),
                max_pp,
                name: m["name"].as_str().unwrap_or("").to_string(),
                kind: Mv::from_id(id),
                type_: ty(m["type"].as_str().unwrap_or("")),
                category,
                base_power: m["basePower"].as_u64().unwrap_or(0) as u16,
                accuracy: m["accuracy"].as_u64().unwrap_or(0) as u8,
                priority: m["priority"].as_i64().unwrap_or(0) as i8,
                target,
                flags,
                has_secondaries: m["secondaries"].as_array().map(|a| !a.is_empty()).unwrap_or(false),
                has_sheer_force_boost: m["hasSheerForceBoost"].as_bool().unwrap_or(false),
                multihit,
                offensive_stat: m["overrideOffensiveStat"].as_str().and_then(stat_index),
                defensive_stat: m["overrideDefensiveStat"].as_str().and_then(stat_index),
                uses_target_attack: m["overrideOffensivePokemon"].as_str() == Some("target"),
                ignore_defensive: m["ignoreDefensive"].as_bool().unwrap_or(false),
                ignore_evasion: m["ignoreEvasion"].as_bool().unwrap_or(false),
                fixed,
                always_hits: m["accuracy"].as_bool() == Some(true),
                crit_ratio: m["critRatio"].as_u64().unwrap_or(1) as u8,
                will_crit: m["willCrit"].as_bool().unwrap_or(false),
                first_turn_only: id == "fakeout" || id == "firstimpression",
                grassy_glide: id == "grassyglide",
                secondaries: m["secondaries"].as_array().map(|a| {
                    a.iter().map(|x| Secondary {
                        chance: x["chance"].as_u64().unwrap_or(100) as u8,
                        boosts: parse_boosts(&x["boosts"]),
                        status: parse_status(&x["status"]),
                        volatile: x["volatileStatus"].as_str().map(String::from),
                        self_boosts: parse_boosts(&x["self"]["boosts"]),
                        custom: hooks.iter().any(|h| h.starts_with("secondary.")),
                    }).collect()
                }).unwrap_or_default(),
                self_boosts: parse_boosts(&m["self"]["boosts"]).or_else(|| parse_boosts(&m["selfBoost"]["boosts"])),
                boosts: parse_boosts(&m["boosts"]),
                status: parse_status(&m["status"]),
                volatile: m["volatileStatus"].as_str().map(String::from),
                self_volatile: m["self"]["volatileStatus"].as_str().map(String::from),
                side_condition: m["sideCondition"].as_str().map(to_id),
                weather: m["weather"].as_str().map(to_id),
                terrain: m["terrain"].as_str().map(to_id),
                pseudo_weather: m["pseudoWeather"].as_str().map(to_id),
                drain: parse_fraction(&m["drain"]),
                recoil: parse_fraction(&m["recoil"]),
                heal: parse_fraction(&m["heal"]),
                self_switch: m["selfSwitch"].as_bool().unwrap_or(false) || m["selfSwitch"].is_string(),
                force_switch: m["forceSwitch"].as_bool().unwrap_or(false),
                self_destruct: m["selfdestruct"].is_string() || m["selfdestruct"].as_bool().unwrap_or(false),
                self_destruct_if_hit: m["selfdestruct"].as_str() == Some("ifHit"),
                breaks_protect: m["breaksProtect"].as_bool().unwrap_or(false),
                ignore_immunity: m["ignoreImmunity"].as_bool().unwrap_or(false),
                protectable: m["flags"]["protect"].is_number(),
                sleep_usable: m["sleepUsable"].as_bool().unwrap_or(false),
                multi_accuracy: m["multiaccuracy"].as_bool().unwrap_or(false),
                hooks: hooks.iter().map(|h| h.to_string()).collect(),
            });
        }

        let mut items = Vec::new();
        let mut item_index = HashMap::new();
        for (id, it) in v["items"].as_object().unwrap() {
            let mut mega = Vec::new();
            if let Some(ms) = it["megaStone"].as_object() {
                for (from, to) in ms {
                    mega.push((to_id(from), to_id(to.as_str().unwrap_or(""))));
                }
            }
            item_index.insert(id.clone(), items.len());
            items.push(ItemData {
                id: id.clone(),
                name: it["name"].as_str().unwrap_or("").to_string(),
                kind: It::from_id(id),
                is_berry: it["isBerry"].as_bool().unwrap_or(false),
                mega,
                fling_bp: it["fling"]["basePower"].as_u64().unwrap_or(0) as u16,
            });
        }

        let mut abilities = Vec::new();
        let mut ability_index = HashMap::new();
        for (id, a) in v["abilities"].as_object().unwrap() {
            ability_index.insert(id.clone(), abilities.len());
            abilities.push(AbilityData {
                id: id.clone(),
                name: a["name"].as_str().unwrap_or("").to_string(),
                kind: Ab::from_id(id),
                cant_suppress: a["flags"]["cantsuppress"].as_i64().unwrap_or(0) != 0,
                no_receiver: a["flags"]["noreceiver"].as_i64().unwrap_or(0) != 0 || id == "noability",
            });
        }

        let mut natures = HashMap::new();
        for (id, nat) in v["natures"].as_object().unwrap() {
            natures.insert(
                id.clone(),
                Nature {
                    plus: nat["plus"].as_str().and_then(stat_index),
                    minus: nat["minus"].as_str().and_then(stat_index),
                },
            );
        }

        let ty = TypeIds {
            normal: ty("Normal"),
            fire: ty("Fire"),
            water: ty("Water"),
            electric: ty("Electric"),
            grass: ty("Grass"),
            ice: ty("Ice"),
            fighting: ty("Fighting"),
            poison: ty("Poison"),
            ground: ty("Ground"),
            flying: ty("Flying"),
            psychic: ty("Psychic"),
            bug: ty("Bug"),
            rock: ty("Rock"),
            ghost: ty("Ghost"),
            dragon: ty("Dragon"),
            dark: ty("Dark"),
            steel: ty("Steel"),
            fairy: ty("Fairy"),
        };
        let _ = BERRIES.set(items.iter().map(|x| x.is_berry).collect());
        let _ = BERRY_KINDS.set(items.iter().map(|x| match x.id.as_str() {
            "sitrusberry" => 1, "oranberry" => 2, "lumberry" => 3, "cheriberry" => 4, "chestoberry" => 5, "pechaberry" => 6,
            "rawstberry" => 7, "aspearberry" => 8, "persimberry" => 9, "leppaberry" => 10,
            _ if matches!(x.kind, It::ResistBerry(_)) => 11, _ => 0,
        }).collect());
        Ok(Dex {
            ty,
            type_names,
            type_index,
            chart,
            species,
            species_index,
            moves,
            move_index,
            items,
            item_index,
            abilities,
            ability_index,
            natures,
        })
    }

    pub fn type_id(&self, name: &str) -> TypeId {
        *self.type_index.get(name).unwrap_or(&NO_TYPE)
    }
    pub fn species(&self, id: &str) -> Option<&Species> {
        self.species_index.get(&to_id(id)).map(|&i| &self.species[i])
    }
    pub fn species_index(&self, id: &str) -> Option<usize> {
        self.species_index.get(&to_id(id)).copied()
    }
    pub fn move_index(&self, id: &str) -> Option<usize> {
        self.move_index.get(&to_id(id)).copied()
    }
    pub fn item_index(&self, id: &str) -> Option<usize> {
        self.item_index.get(&to_id(id)).copied()
    }
    pub fn ability_index(&self, id: &str) -> Option<usize> {
        self.ability_index.get(&to_id(id)).copied()
    }
    pub fn move_by_id(&self, id: &str) -> Option<&MoveData> {
        self.move_index.get(&to_id(id)).map(|&i| &self.moves[i])
    }
    pub fn item(&self, id: &str) -> Option<&ItemData> {
        self.item_index.get(&to_id(id)).map(|&i| &self.items[i])
    }
    pub fn ability(&self, id: &str) -> Option<&AbilityData> {
        self.ability_index.get(&to_id(id)).map(|&i| &self.abilities[i])
    }
    pub fn nature(&self, id: &str) -> Nature {
        self.natures.get(&to_id(id)).copied().unwrap_or_default()
    }

    /// Showdown's damageTaken code for an attacking type against one defending type.
    pub fn type_code(&self, attacking: TypeId, defending: TypeId) -> u8 {
        if attacking == NO_TYPE || defending == NO_TYPE {
            return 0;
        }
        self.chart[attacking as usize][defending as usize]
    }
}
