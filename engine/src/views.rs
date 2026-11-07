//! What one side can know about the other: guessed / sampled views of a battle with the opponent's hidden stats
//! replaced (`guessed_view`, `sampled_view`, `guessed_view_hidden*`), what an observer sees of HP
//! (`shown_hp`, `shown_band`, `shown_mid`), which items / abilities count as revealed (`item_known`, `ability_known`), the
//! disguised-Zoroark view, and a rough estimate of a foe's unseen STAB hit (`guess_hit`). Used by the network's input
//! encoders and by the search's set worlds, so neither ever reads the opponent's true hidden information.
//!
//! `Params` holds the spread-guess settings (and a few damage-estimate constants used by `guess_hit`).

use arrayvec::ArrayVec;

use crate::battle::Battle;
use crate::damage::Status;
use crate::dex::{Dex, Nature, TypeId, ATK, DEF, NO_TYPE, SPA, SPD, SPE};
use crate::state::{Mon, NONE};

/// The opponent-spread guess and the power assumed for a foe's unseen STAB hit.
#[derive(Clone, Debug)]
pub struct Params {
    pub threat_guess_power: f64,
    pub guess_fast_speed: u16,
    pub guess_speed_nature: u16,
    pub guess_offense_sp: u16,
    pub guess_fast_speed_sp: u16,
    pub guess_fast_hp_sp: u16,
    pub guess_slow_hp_sp: u16,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            threat_guess_power: 90.0,
            guess_fast_speed: 70,
            guess_speed_nature: 100,
            guess_offense_sp: 32,
            guess_fast_speed_sp: 32,
            guess_fast_hp_sp: 2,
            guess_slow_hp_sp: 32,
        }
    }
}

fn boost_mult(stage: i8) -> f64 {
    if stage >= 0 { (2 + stage as i32) as f64 / 2.0 } else { 2.0 / (2 - stage as i32) as f64 }
}

/// The guess for an opponent's build: (stat points, nature).
fn guess_spread(p: &Params, base: &[u16; 6]) -> ([u16; 6], Nature) {
    let mut sp = [0u16; 6];
    let (off, other) = if base[ATK] >= base[SPA] { (ATK, SPA) } else { (SPA, ATK) };
    sp[off] = p.guess_offense_sp;
    let mut plus = None;
    if base[SPE] >= p.guess_fast_speed {
        sp[SPE] = p.guess_fast_speed_sp;
        sp[0] = p.guess_fast_hp_sp;
        plus = Some(if base[SPE] >= p.guess_speed_nature { SPE } else { off });
    } else {
        sp[0] = p.guess_slow_hp_sp;
        sp[DEF] = 1;
        sp[SPD] = 1;
        plus = plus.or(Some(off));
    }
    (sp, Nature { plus, minus: Some(other) })
}

/// The 16 damage values of the bridge's simplified damage formula (as floats holding integers).
fn roll_list(level: u8, power: f64, a: f64, d: f64, mult: f64) -> [f64; 16] {
    let lv = ((2.0 * level as f64 / 5.0) + 2.0).floor();
    let base = (lv * power * a / d / 50.0).floor() + 2.0;
    let mut out = [0.0; 16];
    for (k, r) in (85..=100).enumerate() {
        out[k] = (base * mult * r as f64 / 100.0).floor();
    }
    out
}

fn mean16(r: &[f64; 16]) -> f64 {
    r.iter().sum::<f64>() / 16.0
}

// ---------------------------------------------------------------------------
// A Pokemon as the bot sees it
// ---------------------------------------------------------------------------
#[derive(Clone, Debug)]
#[allow(dead_code)]
struct Unit {
    /// stats as the bot believes them (index 0 = max HP)
    stats: [f64; 6],
    types: [TypeId; 2],
    ability: u16,
    item: u16,
    level: u8,
    boosts: [i8; 8],
    status: Status,
    fainted: bool,
    frac: f64,
    moves: ArrayVec<u16, 4>,
    pp: [u8; 4],
    /// moves that count as known (the rest are stand-ins from a Belief)
    n_known: u8,
    first_turn: bool,
    protect_counter: u8,
    speed: f64,
    base: [u16; 6],
    has_mega: bool,
    charging: bool,
}

#[allow(dead_code)]
struct Ctx<'a> {
    dex: &'a Dex,
    p: &'a Params,
    sun: bool,
    rain: bool,
    trick_room: bool,
    psychic_terrain: bool,
}

impl<'a> Ctx<'a> {
    /// poke-env's damage_multiplier: product over the defender's types
    fn eff(&self, t: TypeId, u: &Unit) -> f64 {
        let mut m = 1.0;
        for &d in u.types.iter() {
            if d == NO_TYPE {
                continue;
            }
            m *= match self.dex.type_code(t, d) {
                1 => 2.0,
                2 => 0.5,
                3 => 0.0,
                _ => 1.0,
            };
        }
        m
    }
    fn cur_hp(&self, u: &Unit) -> f64 {
        u.stats[0] * u.frac
    }

    fn guess_hit_fraction(&self, foe: &Unit, target: &Unit) -> f64 {
        let cur = self.cur_hp(target);
        if cur <= 0.0 {
            return 0.0;
        }
        let off = if foe.base[ATK] >= foe.base[SPA] { ATK } else { SPA };
        let dk = if off == ATK { DEF } else { SPD };
        let a = foe.stats[off] * boost_mult(foe.boosts[off]);
        let d = target.stats[dk] * boost_mult(target.boosts[dk]);
        let mut eff = f64::NEG_INFINITY;
        for &t in foe.types.iter() {
            if t != NO_TYPE {
                eff = eff.max(self.eff(t, target));
            }
        }
        if eff == f64::NEG_INFINITY {
            eff = 1.0;
        }
        let r = roll_list(foe.level, self.p.threat_guess_power, a, d, 1.5 * eff);
        mean16(&r) / cur
    }

}

/// A Pokemon outside a battle (team preview) or in one: exact stats if `own`, else the bridge's guess.
fn unit_from_mon(dex: &Dex, p: &Params, m: &Mon, own: bool) -> Unit {
    let base = dex.species[m.species as usize].base;
    let stats = if own {
        let mut s = [0.0; 6];
        for k in 0..6 {
            s[k] = m.stats[k] as f64;
        }
        s[0] = m.maxhp as f64;
        s
    } else {
        guessed_stats(p, &base)
    };
    let moves: ArrayVec<u16, 4> = (0..m.n_moves.min(4) as usize).map(|k| m.moves[k]).filter(|&x| x != u16::MAX).collect();
    let n = moves.len() as u8;
    let frac = if m.maxhp > 0 { m.hp as f64 / m.maxhp as f64 } else { 0.0 };
    Unit {
        stats,
        types: m.types,
        ability: m.ability_id,
        item: m.item_id,
        level: if m.level > 0 { m.level } else { 50 },
        boosts: m.boosts,
        status: m.status,
        fainted: m.fainted,
        frac,
        moves,
        pp: m.pp,
        n_known: n,
        first_turn: m.active_move_actions == 0,
        protect_counter: m.protect_streak,
        speed: 0.0,
        base,
        has_mega: m.mega_species != u16::MAX,
        charging: m.charge_slot != NONE,
    }
}

fn guessed_stats(p: &Params, base: &[u16; 6]) -> [f64; 6] {
    let (sp, nat) = guess_spread(p, base);
    let st = crate::stats::champions_stats(base, &sp, nat);
    let mut out = [0.0; 6];
    for k in 0..6 {
        out[k] = st[k] as f64;
    }
    out
}

/// (2026-10-28, leaks) What an opponent sees of a Pokemon's HP (Champions / Showdown getHealth): floor(100 hp / maxhp), at
/// least 1 while alive, and the colour at 50 (above half or not) and 20 (above a fifth or not). (percent, colour: -1 none)
pub fn shown_hp(hp: u16, maxhp: u16) -> (u16, i8) {
    if hp == 0 || maxhp == 0 {
        return (0, -1);
    }
    if hp >= maxhp {
        return (100, -1);
    }
    let p = ((100 * hp as u32) / maxhp as u32).max(1) as u16;
    let c = match p {
        50 => (hp as u32 * 2 > maxhp as u32) as i8,
        20 => (hp as u32 * 5 > maxhp as u32) as i8,
        _ => -1,
    };
    (p, c)
}

/// The HP values (lo, hi) of a Pokemon with `maxhp` that show as (p, colour) (= bridge.hp_from_percent's range)
pub fn shown_band(p: u16, c: i8, maxhp: u16) -> (u16, u16) {
    let m = maxhp as u32;
    if p == 0 || m == 0 {
        return (0, 0);
    }
    if p >= 100 {
        return (maxhp, maxhp);
    }
    let p = p as u32;
    let mut lo = if p <= 1 { 1 } else { (p * m + 99) / 100 };
    let mut hi = (((p + 1) * m + 99) / 100).saturating_sub(1).min(m.saturating_sub(1));
    match (p, c) {
        (50, 1) => lo = lo.max(m / 2 + 1),
        (50, 0) => hi = hi.min(m / 2),
        (20, 1) => lo = lo.max(m / 5 + 1),
        (20, 0) => hi = hi.min(m / 5),
        _ => {}
    }
    let lo = lo.clamp(1, m);
    (lo as u16, hi.max(lo).min(m) as u16)
}

/// The middle of that range: the HP a rebuilt position gives an opponent (bridge.hp_from_percent)
pub fn shown_mid(p: u16, c: i8, maxhp: u16) -> u16 {
    let (lo, hi) = shown_band(p, c, maxhp);
    ((lo as u32 + hi as u32 + 1) / 2) as u16
}

/// The battle as `me` can know it: the opponent's stats replaced by the believed stats (`stat_belief`, set on rebuilt
/// positions from spread inference) or else the generic guess (HP scaled to keep the same fraction). For features that
/// must not leak the opponent's hidden stats.
pub fn guessed_view(b: &Battle, dex: &Dex, me: usize, p: &Params) -> Battle {
    let mut v = b.search_copy(false);
    for m in v.sides[1 - me].mons.iter_mut() {
        let base = dex.species[m.species as usize].base;
        let (sp, nat) = guess_spread(p, &base);
        let mut st = crate::stats::champions_stats(&base, &sp, nat);
        if m.stat_belief[0] > 0 {
            st = m.stat_belief;
        }
        let (sp, nat) = if m.stat_belief[0] > 0 { (m.points, m.nature) } else { (sp, nat) };
        // (2026-10-28, leaks) the HP an observer can know: the middle of the range the shown percentage allows
        let (hp_p, hp_c) = shown_hp(m.hp, m.maxhp);
        m.points = sp;
        m.nature = nat;
        m.stats = st;
        m.maxhp = st[0];
        if m.alive() {
            m.hp = shown_mid(hp_p, hp_c, st[0]).max(1).min(st[0]);
        }
    }
    v
}

/// A plausible build for each opposing Pokemon (for search worlds): one of a few common stat-point
/// templates (fast attacker, bulky attacker, defensive, mixed, fast support) with a fitting nature;
/// HP fractions kept. The real stats are never used.
pub fn sampled_view(b: &Battle, dex: &Dex, me: usize, rng: &mut crate::state::Rng) -> Battle {
    let mut v = b.search_copy(false);
    for m in v.sides[1 - me].mons.iter_mut() {
        let base = dex.species[m.species as usize].base;
        let off = if base[ATK] >= base[SPA] { ATK } else { SPA };
        let other = if off == ATK { SPA } else { ATK };
        let mut sp = [0u16; 6];
        let mut plus = Some(off);
        match rng.below(5) {
            0 => { sp[off] = 32; sp[SPE] = 32; sp[0] = 2; if base[SPE] >= 90 { plus = Some(SPE); } }
            1 => { sp[off] = 32; sp[0] = 32; sp[DEF] = 1; sp[SPD] = 1; }
            2 => { sp[0] = 32; sp[DEF] = 16; sp[SPD] = 18; plus = Some(if base[DEF] >= base[SPD] { SPD } else { DEF }); }
            3 => { sp[off] = 32; sp[0] = 16; sp[SPE] = 18; }
            _ => { sp[0] = 32; sp[SPE] = 32; sp[DEF] = 2; plus = Some(SPE); }
        }
        let nat = Nature { plus, minus: Some(other) };
        let st = crate::stats::champions_stats(&base, &sp, nat);
        // (2026-10-28, leaks) HP drawn among the values the shown percentage allows (it was the exact fraction)
        let (hp_p, hp_c) = shown_hp(m.hp, m.maxhp);
        m.points = sp;
        m.nature = nat;
        m.stats = st;
        m.maxhp = st[0];
        if m.alive() {
            let (lo, hi) = shown_band(hp_p, hp_c, st[0]);
            m.hp = (lo + rng.below((hi - lo) as u32 + 1) as u16).max(1).min(st[0]);
        }
    }
    v
}

/// What `me` can know about the opponent's item and ability in a hidden-information game: shown ones,
/// those announced by their effects, or a species with one possible ability.
pub fn item_known(dex: &Dex, m: &Mon) -> bool {
    if m.rev & crate::state::rev::EXPLICIT != 0 {
        return m.rev & crate::state::rev::ITEM != 0;
    }
    if m.rev & crate::state::rev::ITEM != 0 || m.is_mega {
        return true;
    }
    if m.item_id == u16::MAX {
        return m.rev & crate::state::rev::SEEN != 0 && m.active_turns >= 1;   // no item: noticed after a while
    }
    let id = dex.items[m.item_id as usize].id.as_str();
    let seen = m.rev & crate::state::rev::SEEN != 0;
    match id {
        "airballoon" => seen,
        "leftovers" | "blacksludge" | "rockyhelmet" | "shellbell" => seen && m.active_turns >= 1,
        "lifeorb" => m.rev & 0x0f != 0,
        _ => false,
    }
}

pub fn ability_known(dex: &Dex, m: &Mon) -> bool {
    if m.rev & crate::state::rev::EXPLICIT != 0 {
        return m.rev & crate::state::rev::ABILITY != 0;
    }
    if dex.species[m.species as usize].n_abilities <= 1 {
        return true;
    }
    if m.rev & crate::state::rev::SEEN == 0 {
        return false;
    }
    matches!(dex.abilities.get(m.ability_id as usize).map(|a| a.id.as_str()).unwrap_or(""),
        "intimidate" | "drizzle" | "drought" | "sandstream" | "snowwarning" | "electricsurge" | "grassysurge"
        | "psychicsurge" | "mistysurge" | "pressure" | "moldbreaker" | "teravolt" | "turboblaze" | "unnerve"
        | "asone" | "asoneglastrier" | "asonespectrier" | "trace" | "frisk" | "airlock" | "cloudnine"
        | "anticipation" | "forewarn" | "download" | "screencleaner" | "intrepidsword" | "dauntlessshield"
        | "supremeoverlord" | "orichalcumpulse" | "hadronengine" | "neutralizinggas" | "comatose" | "darkaura"
        | "fairyaura" | "aurabreak" | "vesselofruin" | "swordofruin" | "tabletsofruin" | "beadsofruin"
        | "costar" | "hospitality" | "commander" | "zerotohero" | "megasol" | "dragonize" | "shadowtag" | "arenatrap")
}

/// `guessed_view` for a hidden-information game: also the opponent's unrevealed items removed and
/// unrevealed abilities replaced by the species' first ability (what an observer would assume).
pub fn guessed_view_hidden(b: &Battle, dex: &Dex, me: usize, p: &Params) -> Battle {
    let mut v = guessed_view(b, dex, me, p);
    for m in v.sides[1 - me].mons.iter_mut() {
        if !item_known(dex, m) {
            m.item_id = u16::MAX;
            m.item = crate::effects::It::None;
        }
        if !ability_known(dex, m) {
            if let Some(a) = dex.ability_index(&dex.species[m.species as usize].ability0) {
                m.ability_id = a as u16;
                m.ability = dex.abilities[a].kind;
            }
        }
    }
    disguise_view(&mut v, me, false);
    v
}

/// A disguised Zoroark (engine-tracked knowledge only: rebuilt positions carry EXPLICIT bits) looks like the Pokemon it
/// imitates: that one's species, types, guessed stats and assumed ability; its own HP fraction (2026-10-17). `copy_set`
/// (open sheets, 2026-11-05 review): also the imitated one's moves and item, which an open sheet shows for it.
pub fn disguise_view(v: &mut Battle, me: usize, copy_set: bool) {
    let op = 1 - me;
    for z in 0..v.sides[op].mons.len() {
        let zm = &v.sides[op].mons[z];
        let t = zm.illusion_as as usize;
        if !zm.illusion || t >= v.sides[op].mons.len() || t == z || zm.rev & crate::state::rev::EXPLICIT != 0 {
            continue;
        }
        let tm = v.sides[op].mons[t].clone();
        let m = &mut v.sides[op].mons[z];
        let frac = if m.maxhp > 0 { m.hp as f64 / m.maxhp as f64 } else { 0.0 };
        m.species = tm.species;
        m.types = tm.types;
        m.added_type = tm.added_type;
        m.points = tm.points;
        m.nature = tm.nature;
        m.stats = tm.stats;
        m.maxhp = tm.maxhp;
        m.weight_hg = tm.weight_hg;
        m.ability_id = tm.ability_id;
        m.ability = tm.ability;
        if copy_set {
            m.moves = tm.moves;
            m.n_moves = tm.n_moves;
            m.item = tm.item;
            m.item_id = tm.item_id;
        }
        if m.alive() {
            m.hp = (round_half_even(frac * m.maxhp as f64) as u16).max(1).min(m.maxhp);
        }
    }
}

/// the bridge's estimate of a foe's unseen STAB attack on a target: fraction of the target's HP left
/// (`v` should be a guessed view).
pub fn guess_hit(dex: &Dex, p: &Params, v: &Battle, fs: usize, fi: usize, ts: usize, ti: usize) -> f64 {
    let ctx = Ctx { dex, p, sun: false, rain: false, trick_room: false, psychic_terrain: false };
    let a = unit_from_mon(dex, p, &v.sides[fs].mons[fi], true);
    let d = unit_from_mon(dex, p, &v.sides[ts].mons[ti], true);
    ctx.guess_hit_fraction(&a, &d)
}

/// Python's round(): halves go to the even neighbour.
fn round_half_even(x: f64) -> f64 {
    let r = x.round();
    if (x - x.trunc()).abs() == 0.5 && r % 2.0 != 0.0 { r - x.signum() } else { r }
}
