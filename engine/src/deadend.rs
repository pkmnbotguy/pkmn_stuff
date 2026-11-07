//! Dead-end positions for the search (2026-10-31, arena game 117598).
//!
//! A 1v1 where one Pokemon can never damage the other again (every move it still has PP for is a harmless status
//! move, e.g. Farigiraf with only Trick Room / Helping Hand / Protect left), while the other has more than enough
//! safe damaging PP to finish it, is lost for the first one: the value net doesn't see PP races, so without this the
//! search spent the last Psychic PP into a predicted Protect at 0.99 "win chance". Search leaves use `dead_end` as a
//! terminal result (py/src/lib.rs). The rule is conservative: anything that could change the outcome (healing,
//! defensive boosts, residual damage on the attacker, contact punishers, PP drain, accuracy tricks...) means "no
//! verdict". The exact endgame solver doesn't use it. Off: env VGCSIM_DEAD_END=0 or `vgcsim.set_dead_end(False)`.

use crate::battle::{Battle, Phase};
use crate::damage::{Status, Terrain, Weather};
use crate::dex::{Category, Dex, FixedDamage, ATK, DEF, EVA, SPA, SPD};
use crate::state::*;
use std::sync::atomic::{AtomicU8, Ordering};

// 0 = not read yet, 1 = on, 2 = off
static DEAD_END: AtomicU8 = AtomicU8::new(0);

pub fn dead_end_enabled() -> bool {
    match DEAD_END.load(Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => {
            let on = std::env::var("VGCSIM_DEAD_END").map(|v| v != "0").unwrap_or(true);
            DEAD_END.store(if on { 1 } else { 2 }, Ordering::Relaxed);
            on
        }
    }
}

pub fn set_dead_end(on: bool) {
    DEAD_END.store(if on { 1 } else { 2 }, Ordering::Relaxed);
}

/// Status moves that can't hurt the foe or make it weaker / the user bulkier.
const HARMLESS: &[&str] = &[
    "trickroom", "helpinghand", "tailwind", "followme", "ragepowder", "allyswitch", "gravity", "electricterrain",
    "focusenergy", "dragoncheer", "lockon", "laserfocus", "swordsdance", "nastyplot", "dragondance", "agility",
    "rockpolish", "autotomize", "howl", "workup", "growth", "coaching", "taunt", "encore", "disable", "torment",
    "imprison", "splash", "celebrate", "safeguard", "mist", "craftyshield", "roar", "whirlwind", "tidyup",
    "meditate", "honeclaws", "sharpen", "shiftgear", "aromatherapy", "healbell", "batonpass",
    "teleport",
];
/// Status moves that only block one hit per use (worst case: each one eats one of the foe's damaging PP).
const STALL: &[&str] = &["protect", "detect", "endure", "wideguard", "quickguard", "silktrap", "obstruct", "maxguard"];

/// Damaging moves whose damage depends on conditions or the target (never counted as safe damage).
const UNRELIABLE: &[&str] = &[
    "fakeout", "firstimpression", "suckerpunch", "thunderclap", "upperhand", "belch", "lastresort", "dreameater",
    "poltergeist", "steelroller", "burnup", "doubleshock", "spitup", "counter", "mirrorcoat", "metalburst",
    "comeuppance", "bide", "futuresight", "doomdesire", "fling", "naturalgift", "snore", "shelltrap", "focuspunch",
    "highjumpkick", "jumpkick", "mindblown", "steelbeam", "chloroblast", "superpower", "hammerarm", "icehammer",
    "spinout", "armorcannon", "makeitrain", "makeitrain", "dracometeor", "overheat", "leafstorm", "psychoboost",
    "fleurcannon", "vcreate", "headlongrush", "dragonascent", "clangingscales", "hyperspacefury", "synchronoise",
    "skydrop", "rollout", "iceball", "uproar", "outrage", "thrash", "petaldance", "ragingfury", "explosion",
    "selfdestruct", "mistyexplosion", "finalgambit", "memento", "struggle",
];

impl Battle {
    /// In a 1v1 (each side has exactly one living Pokemon, both on the field, a turn to choose): Some(winner) when
    /// the loser can never damage the winner and the winner surely finishes it with its remaining safe damage.
    pub fn dead_end(&self, dex: &Dex) -> Option<usize> {
        if !dead_end_enabled() || self.phase != Phase::Turn {
            return None;
        }
        let mut pos = [(0usize, 0usize, 0usize); 2]; // (index, slot) per side
        for s in 0..2 {
            let side = &self.sides[s];
            if side.mons.iter().filter(|m| m.alive()).count() != 1 {
                return None;
            }
            let mut found = None;
            for slot in 0..2 {
                if let Some(i) = self.active(s, slot) {
                    found = Some((i, slot));
                }
            }
            let (i, slot) = found?;
            pos[s] = (i, slot, 0);
        }
        for x in 0..2 {
            let y = 1 - x;
            if self.cannot_hurt(dex, x, pos[x].0, y, pos[y].0) && self.surely_kills(dex, y, pos[y].0, pos[y].1, x, pos[x].0, pos[x].1) {
                return Some(y);
            }
        }
        None
    }

    fn ab_str<'a>(&self, dex: &'a Dex, m: &Mon) -> &'a str {
        if m.vol2 & vol2::GASTRO != 0 || m.ability_id == u16::MAX {
            ""
        } else {
            dex.abilities[m.ability_id as usize].id.as_str()
        }
    }
    fn item_str<'a>(&self, dex: &'a Dex, m: &Mon) -> &'a str {
        if m.item_id == u16::MAX { "" } else { dex.items[m.item_id as usize].id.as_str() }
    }

    /// X (side xs, index xi) has no way left to lower Y's HP, ever.
    fn cannot_hurt(&self, dex: &Dex, xs: usize, xi: usize, ys: usize, yi: usize) -> bool {
        let xm = self.mon(xs, xi);
        let ym = self.mon(ys, yi);
        // X can only use what it can choose: Taunt / Encore / Disable / Torment may force Struggle
        if xm.taunt > 0 || xm.encore > 0 || xm.disable > 0 || xm.vol2 & vol2::TORMENT != 0 {
            return false;
        }
        if xm.has_vol(vol::DESTINY_BOND) || xm.has_vol(vol::TRANSFORMED) || xm.perish > 0 {
            return false;
        }
        let mut usable = 0;
        for j in 0..xm.n_moves as usize {
            let id = xm.moves[j];
            if id == u16::MAX || xm.pp[j] == 0 {
                continue;
            }
            if xm.choice_lock != NONE && xm.choice_lock as usize != j {
                continue;
            }
            let mv = &dex.moves[id as usize];
            if mv.category != Category::Status {
                return false;
            }
            let s = mv.id.as_str();
            if !HARMLESS.contains(&s) && !STALL.contains(&s) {
                return false;
            }
            usable += 1;
        }
        if usable == 0 {
            return false; // Struggle
        }
        let xa = self.ab_str(dex, xm);
        if matches!(xa, "roughskin" | "ironbarbs" | "aftermath" | "baddreams" | "liquidooze" | "poisonpoint"
            | "flamebody" | "effectspore" | "perishbody" | "innardsout" | "synchronize" | "mummy" | "wanderingspirit"
            | "lingeringaroma" | "pressure_never" | "stamina" | "watercompaction" | "cursedbody" | "toxicdebris"
            | "sandspit" | "seedsower" | "trace" | "imposter" | "illusion" | "pickpocket" | "magician" | "costar")
        {
            return false;
        }
        let xit = self.item_str(dex, xm);
        if matches!(xit, "rockyhelmet" | "stickybarb" | "jabocaberry" | "rowapberry" | "keeberry" | "marangaberry"
            | "luminousmoss" | "leftovers" | "blacksludge" | "shellbell" | "eviolite_never")
        {
            return false;
        }
        // anything already hurting Y, or that could
        if matches!(ym.status, Status::Burn | Status::Poison | Status::Toxic) {
            return false;
        }
        if ym.has_vol(vol::LEECH_SEED) || ym.has_vol(vol::CURSE) || ym.vol2 & vol2::SALT_CURE != 0
            || ym.bound > 0 || ym.perish > 0 || ym.confusion > 0
        {
            return false;
        }
        let w = self.weather();
        let ya = self.ab_str(dex, ym);
        if w == Weather::Sand
            && !(ym.has_type(dex.ty.rock) || ym.has_type(dex.ty.ground) || ym.has_type(dex.ty.steel))
            && !matches!(ya, "overcoat" | "magicguard" | "sandveil" | "sandrush" | "sandforce")
        {
            return false;
        }
        if w == Weather::Sun && matches!(ya, "solarpower" | "dryskin") {
            return false;
        }
        let yit = self.item_str(dex, ym);
        if matches!(yit, "lifeorb" | "stickybarb" | "blacksludge" | "flameorb" | "toxicorb") {
            return false;
        }
        if matches!(ya, "defeatist" | "moody" | "slowstart_never") {
            return false;
        }
        for s in 0..2 {
            if self.sides[s].future.iter().any(|f| f.0 != 0) || self.sides[s].wish.iter().any(|w| w.0 != 0) {
                return false;
            }
        }
        true
    }

    /// Y (attacker) surely brings X to 0 with its remaining safe damaging PP, even if every stall move of X eats one.
    #[allow(clippy::too_many_arguments)]
    fn surely_kills(&self, dex: &Dex, ys: usize, yi: usize, yslot: usize, xs: usize, xi: usize, xslot: usize) -> bool {
        let ym = self.mon(ys, yi);
        let xm = self.mon(xs, xi);
        // X can't heal or get bulkier
        if xm.vol2 & (vol2::AQUA_RING | vol2::INGRAIN) != 0 || xm.boosts[EVA] > 0 || ym.boosts[crate::dex::ACC] < 0 {
            return false;
        }
        if self.field.terrain == Terrain::Grassy {
            return false;
        }
        let xa = self.ab_str(dex, xm);
        if matches!(xa, "raindish" | "icebody" | "poisonheal" | "dryskin" | "wonderguard" | "regenerator_never") {
            return false;
        }
        let pressure = xa == "pressure";
        let mut ehp = xm.hp as u32;
        let xit = self.item_str(dex, xm);
        if xm.has_item() && dex.items[xm.item_id as usize].is_berry {
            ehp += xm.maxhp as u32 / 3 + 10;
        }
        let _ = xit;
        // margin: Sturdy / Focus Sash / Disguise / a low roll
        let margin_hits = 2u32;
        let mut stall = 0u32;
        for j in 0..xm.n_moves as usize {
            let id = xm.moves[j];
            if id == u16::MAX || xm.pp[j] == 0 {
                continue;
            }
            let mv = &dex.moves[id as usize];
            if mv.heal.is_some() || mv.self_boosts.is_some_and(|b| b[DEF] > 0 || b[SPD] > 0 || b[EVA] > 0) {
                return false;
            }
            if STALL.contains(&mv.id.as_str()) {
                stall += xm.pp[j] as u32;
            }
        }
        // Y's safe hits: (damage per hit, hits available)
        let mut hits: arrayvec::ArrayVec<(u32, u32), 4> = arrayvec::ArrayVec::new();
        for j in 0..ym.n_moves as usize {
            let id = ym.moves[j];
            if id == u16::MAX || ym.pp[j] == 0 {
                continue;
            }
            if ym.choice_lock != NONE && ym.choice_lock as usize != j {
                continue;
            }
            let mv = &dex.moves[id as usize];
            if mv.category == Category::Status || mv.recoil.is_some() || mv.self_destruct || mv.self_destruct_if_hit
                || mv.fixed != FixedDamage::No || mv.self_boosts.is_some_and(|b| b[ATK] < 0 || b[SPA] < 0)
                || mv.first_turn_only || UNRELIABLE.contains(&mv.id.as_str())
            {
                continue;
            }
            let Some(rolls) = self.preview_rolls(dex, ys, yi, j, xs, xi) else { continue };
            let dmg = rolls.iter().copied().min().unwrap_or(0) as u32;
            if dmg == 0 {
                continue;
            }
            let uses = if pressure { (ym.pp[j] as u32 + 1) / 2 } else { ym.pp[j] as u32 };
            // misses: count only the hits it surely lands on average, rounded down
            let acc = if mv.accuracy == 0 || mv.always_hits { 100 } else { mv.accuracy as u32 };
            let n = if acc >= 100 { uses } else { uses * acc / 100 };
            if n > 0 {
                hits.push((dmg, n));
            }
        }
        let _ = (yslot, xslot);
        if hits.is_empty() {
            return false;
        }
        // worst case: X's stall moves eat the strongest hits, Y's weakest-first is irrelevant (sum)
        hits.sort_by(|a, b| b.0.cmp(&a.0));
        let mut eat = stall + margin_hits;
        let mut total: u32 = 0;
        for &(d, n) in hits.iter() {
            let gone = eat.min(n);
            eat -= gone;
            total += d * (n - gone);
        }
        total > ehp
    }
}

// ---------------------------------------------------------------------------------------------------------------
// (2026-11-01, endgame solver speed idea 8) race_win: dead_end generalised to up to `max_mons` Pokemon per side.

/// X moves that can't hurt Y but can shrink Y's damage later (lock or remove Y's moves).
const RACE_BLOCKERS: &[&str] = &["encore", "disable", "torment", "imprison"];
/// Y moves whose damage can drop or depends on the target / turn order (never counted in a race proof).
const RACE_UNSAFE: &[&str] = &[
    "triplekick", "tripleaxel", "populationbomb", "hex", "venoshock", "infernalparade", "barbbarrage", "wakeupslap",
    "smellingsalts", "crushgrip", "hardpress", "wringout", "knockoff", "boltbeak", "fishiousrend", "gyroball",
    "electroball", "heavyslam", "heatcrash", "lowkick", "grassknot", "payback", "avalanche", "revenge", "assurance",
    "facade", "acrobatics", "storedpower", "powertrip", "punishment", "eruption", "waterspout", "dragonenergy",
];

/// At least how many of n tries with hit chance acc% land, except with probability <= 0.001.
fn sure_hits(n: u32, acc: u32) -> u32 {
    if acc >= 100 || n == 0 {
        return n;
    }
    let p = acc as f64 / 100.0;
    let mut pk = (1.0 - p).powi(n as i32); // P(X = k), starting at k = 0
    let mut cdf = 0.0; // P(X < k)
    for k in 0..=n {
        if cdf + pk > 1e-3 {
            return k;
        }
        cdf += pk;
        if k < n {
            pk *= (n - k) as f64 / (k + 1) as f64 * p / (1.0 - p);
        }
    }
    n
}

impl Battle {
    /// Some(winner) when each side has 1..=max_mons living Pokemon, every living Pokemon of the loser can never
    /// lower any living foe's HP again (dead_end's rules), and the winner's Pokemon on the field surely KO all of
    /// the loser's living Pokemon with their remaining safe damage: minimum rolls, minimum over every target and over
    /// the position with weather / terrain / Gravity gone and the attackers' items removed, misses at the 0.1% tail,
    /// each stall PP of the loser eating one turn of hits, overkill (one max hit per Pokemon) and a margin of hits.
    /// Anything that could change that (healing, bulk boosts, Substitute, a Mega still to come, switch-in abilities,
    /// status or locks on the attackers...) means no verdict. Used by the exact endgame solver with opts.race.
    pub fn race_win(&self, dex: &Dex, max_mons: usize) -> Option<usize> {
        self.race_win_opt(dex, max_mons, false)
    }

    /// (2026-11-01, endgame solver idea 8b) race_win with `wide` = fewer refusals where the damage rolls already cover
    /// the effect: static damage-reducing abilities / items of the loser (Multiscale, Fluffy, Filter, Thick Fat, Eviolite...),
    /// Air Balloon / Quick Claw, Sturdy at full HP (one more max hit), Grassy Terrain (its remaining healing added to the
    /// loser's HP), Safeguard / Mist / Tailwind / Trick Room on the loser's side, paralysed attackers (hits x 3/4).
    pub fn race_win_opt(&self, dex: &Dex, max_mons: usize, wide: bool) -> Option<usize> {
        if !dead_end_enabled() || self.phase != Phase::Turn || self.winner.is_some() {
            return None;
        }
        let mut alive: [arrayvec::ArrayVec<usize, 6>; 2] = [arrayvec::ArrayVec::new(), arrayvec::ArrayVec::new()];
        for s in 0..2 {
            for (i, m) in self.sides[s].mons.iter().enumerate() {
                if m.alive() {
                    alive[s].push(i);
                }
            }
            if alive[s].is_empty() || alive[s].len() > max_mons {
                return None;
            }
        }
        'side: for x in 0..2 {
            let y = 1 - x;
            for &xi in &alive[x] {
                for &yi in &alive[y] {
                    if !self.cannot_hurt(dex, x, xi, y, yi) {
                        continue 'side;
                    }
                }
            }
            if self.race_kills(dex, y, x, &alive[y], &alive[x], wide) {
                return Some(y);
            }
        }
        None
    }

    fn race_kills(&self, dex: &Dex, y: usize, x: usize, ya: &[usize], xa: &[usize], wide: bool) -> bool {
        let grassy = self.field.terrain == Terrain::Grassy;
        if grassy && !wide {
            return false;
        }
        // Y's attackers = its Pokemon on the field (X can't hurt or force it, so it never has to switch)
        let mut atk: arrayvec::ArrayVec<usize, 2> = arrayvec::ArrayVec::new();
        for slot in 0..2 {
            if let Some(i) = self.active(y, slot) {
                if self.sides[y].mons[i].alive() {
                    atk.push(i);
                }
            }
        }
        if atk.is_empty() {
            return false;
        }
        let y_bench = ya.len() > atk.len();
        let mut stall = 0u32;
        let mut ehp_sum = 0u64;
        let mut pressure = false;
        let mut sturdy = 0u64;
        for &xi in xa {
            let xm = &self.sides[x].mons[xi];
            if xm.vol2 & (vol2::AQUA_RING | vol2::INGRAIN) != 0 || xm.boosts[EVA] > 0 || xm.sub_hp > 0 {
                return false;
            }
            let ab = self.ab_str(dex, xm);
            let wide_ok = wide && matches!(ab, "multiscale" | "shadowshield" | "fluffy" | "furcoat" | "icescales" | "filter"
                | "solidrock" | "prismarmor" | "thickfat" | "heatproof" | "waterbubble" | "purifyingsalt" | "punkrock" | "sturdy");
            if !wide_ok && matches!(ab, "raindish" | "icebody" | "poisonheal" | "dryskin" | "wonderguard" | "regenerator"
                | "intimidate" | "friendguard" | "drizzle" | "drought" | "sandstream" | "snowwarning" | "electricsurge"
                | "psychicsurge" | "grassysurge" | "mistysurge" | "neutralizinggas" | "orichalcumpulse" | "hadronengine"
                | "supersweetsyrup" | "static" | "cutecharm" | "gulpmissile" | "mirrorarmor" | "opportunist"
                | "flowergift" | "snowcloak" | "sandveil" | "multiscale" | "shadowshield" | "iceface" | "disguise"
                | "fluffy" | "furcoat" | "icescales" | "punkrock" | "stamina" | "weakarmor" | "justified" | "rattled"
                | "zerotohero" | "commander" | "imposter" | "illusion" | "screencleaner" | "cloudnine" | "airlock"
                | "forecast" | "unaware" | "hospitality" | "filter" | "solidrock" | "prismarmor" | "thickfat"
                | "heatproof" | "waterbubble" | "purifyingsalt" | "levitate" | "eelevate" | "sapsipper" | "voltabsorb"
                | "waterabsorb" | "flashfire" | "lightningrod" | "stormdrain" | "motordrive" | "eartheater"
                | "wellbakedbody" | "windrider" | "bulletproof" | "soundproof" | "telepathy" | "sturdy"
                | "marvelscale" | "grasspelt" | "tintedlens_never")
            {
                return false;
            }
            if ab == "pressure" {
                pressure = true;
            }
            let it = self.item_str(dex, xm);
            if matches!(it, "brightpowder" | "laxincense" | "focusband") {
                return false;
            }
            if !wide && matches!(it, "quickclaw" | "airballoon" | "eviolite") {
                return false;
            }
            if xm.has_item() && !dex.items[xm.item_id as usize].mega.is_empty() && !self.sides[x].mega_used {
                return false;
            }
            for j in 0..xm.n_moves as usize {
                let id = xm.moves[j];
                if id == u16::MAX || xm.pp[j] == 0 {
                    continue;
                }
                let mv = &dex.moves[id as usize];
                let s = mv.id.as_str();
                if mv.heal.is_some() || mv.self_boosts.is_some_and(|b| b[DEF] > 0 || b[SPD] > 0 || b[EVA] > 0)
                    || RACE_BLOCKERS.contains(&s) || ((s == "roar" || s == "whirlwind") && y_bench)
                    || matches!(s, "allyswitch" | "followme" | "ragepowder" | "craftyshield"
                        | "batonpass" | "teleport" | "electricterrain" | "gravity")
                    || (!wide && matches!(s, "safeguard" | "mist" | "tailwind" | "trickroom"))
                {
                    return false;
                }
                if STALL.contains(&s) {
                    stall += xm.pp[j] as u32;
                }
            }
            let mut ehp = xm.hp as u64;
            if xm.has_item() && dex.items[xm.item_id as usize].is_berry {
                ehp += xm.maxhp as u64 / 2 + 10;
            }
            if wide && grassy && self.is_grounded(dex, x, xi) {
                ehp += self.field.terrain_turns as u64 * (xm.maxhp as u64 / 16 + 1);
            }
            if wide && ab == "sturdy" && xm.hp == xm.maxhp {
                sturdy += 1;
            }
            ehp_sum += ehp;
        }
        // the position with weather / terrain / Gravity gone and Y's attackers' items removed (both are checked)
        let mut clean = self.clone();
        clean.field.weather = Weather::None;
        clean.field.terrain = Terrain::None;
        clean.field.gravity = 0;
        for &i in &atk {
            clean.sides[y].mons[i].item_id = u16::MAX;
        }
        let n_atk = atk.len() as u32;
        let spread_hits_two = xa.len() > 1;
        let mut hits: arrayvec::ArrayVec<(u32, u32), 8> = arrayvec::ArrayVec::new();
        let mut dmax = 0u32;
        for &yi in &atk {
            let ym = &self.sides[y].mons[yi];
            let par = wide && ym.status == Status::Paralysis;
            if !(matches!(ym.status, Status::None) || par) || ym.encore > 0 || ym.disable > 0 || ym.taunt > 0
                || ym.vol2 & vol2::TORMENT != 0 || ym.vol2 & vol2::ATTRACT != 0 || ym.confusion > 0
                || ym.boosts[crate::dex::ACC] < 0
            {
                return false;
            }
            if matches!(self.ab_str(dex, ym), "truant" | "hustle" | "slowstart" | "defeatist" | "moody") {
                return false;
            }
            if matches!(self.item_str(dex, ym), "lifeorb" | "stickybarb" | "blacksludge" | "flameorb" | "toxicorb") {
                return false;
            }
            for j in 0..ym.n_moves as usize {
                let id = ym.moves[j];
                if id == u16::MAX || ym.pp[j] == 0 {
                    continue;
                }
                if ym.choice_lock != NONE && ym.choice_lock as usize != j {
                    continue;
                }
                let mv = &dex.moves[id as usize];
                let s = mv.id.as_str();
                if mv.category == Category::Status || mv.recoil.is_some() || mv.self_destruct || mv.self_destruct_if_hit
                    || mv.fixed != FixedDamage::No || mv.self_boosts.is_some_and(|b| b[ATK] < 0 || b[SPA] < 0)
                    || mv.first_turn_only || UNRELIABLE.contains(&s) || RACE_UNSAFE.contains(&s)
                    || mv.flags & crate::dex::flag::CHARGE != 0 || mv.self_volatile.as_deref() == Some("mustrecharge")
                    || mv.multihit.0 != mv.multihit.1 || (mv.target == crate::dex::Target::AllAdjacent && n_atk > 1)
                {
                    continue;
                }
                let spread = matches!(mv.target, crate::dex::Target::AllAdjacentFoes | crate::dex::Target::AllAdjacent);
                let sp = if spread && spread_hits_two { Some(true) } else if spread { Some(false) } else { None };
                let mut dmg = u32::MAX;
                for b in [self, &clean] {
                    for &xi in xa {
                        match b.preview_rolls_spread(dex, y, yi, j, x, xi, false, sp) {
                            Some(rolls) => dmg = dmg.min(rolls.iter().copied().min().unwrap_or(0) as u32),
                            None => dmg = 0,
                        }
                    }
                }
                if dmg == 0 || dmg == u32::MAX {
                    continue;
                }
                let uses = if pressure { (ym.pp[j] as u32 + 1) / 2 } else { ym.pp[j] as u32 };
                let mut acc = if mv.accuracy == 0 || mv.always_hits { 100 } else { mv.accuracy as u32 };
                if par {
                    acc = acc * 3 / 4;
                }
                let n = sure_hits(uses, acc);
                if n > 0 {
                    dmax = dmax.max(dmg);
                    hits.push((dmg, n));
                }
            }
        }
        if hits.is_empty() {
            return false;
        }
        // worst case: X's stall PP eat the strongest hits (each stall turn blocks every attacker's hit), plus a margin
        hits.sort_by(|a, b| b.0.cmp(&a.0));
        let n_x = xa.len() as u32;
        let margin = if n_x > 1 { 3 * n_x } else { 2 };
        let mut eat = stall * n_atk + margin;
        let mut total: u64 = 0;
        for &(d, n) in hits.iter() {
            let gone = eat.min(n);
            eat -= gone;
            total += d as u64 * (n - gone) as u64;
        }
        total > ehp_sum + (n_x as u64 + sturdy) * dmax as u64
    }
}
