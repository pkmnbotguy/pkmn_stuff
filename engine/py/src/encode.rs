//! Network encodings: a battle position (or a team preview) turned into the networks' input features from one side's
//! point of view (encode_battle, encode_battles, encode_preview), plus the action numbering the networks and the search
//! share.
//!
//! Float features obs_f[OBS_F] (per-Pokemon blocks, field, damage / uncertainty / bench / roster / flag / belief blocks:
//! see obs_layout, ext_layout, v3_layout, flag_layout), id features obs_i[OBS_I] (species / ability / item / moves, for
//! embeddings), legal-action mask [2, N_ACT].
//!
//! Action index per active slot (N_ACT = 38):
//!   move slot m (0-3), target code t (0 none, 1 foe a, 2 foe b, 3 ally side), mega g: (m*4 + t)*2 + g
//!   32 + j: switch to brought Pokemon j (0-3);  36: Struggle;  37: pass
//!
//! Team preview: 12 Pokemon (own team then the opponent's, MON_F floats + MON_I ids each, opponent stats hidden);
//! options = preview_options() (15 fours x 6 lead pairs).

use ::vgcsim::state::BelSet;
use std::sync::Arc;

use pyo3::prelude::*;
use rayon::prelude::*;

use ::vgcsim::battle::{build_team, Action, Battle as Engine, Phase};
use ::vgcsim::damage::{Status, Terrain, Weather};
use ::vgcsim::dex::Dex as EngineDex;
use ::vgcsim::views::{self, Params};
use ::vgcsim::state::{parse_team, Mon, Rng, NONE};

pub const N_ACT: usize = 38;
const MON_F: usize = 80;
const FIELD_F: usize = 40;
/// damage features (see `encode_damage`), after the field block
const DMG_F: usize = 96;
/// history features per Pokemon (see `encode_hist`), after the damage block
pub const HIST_F: usize = 20;
const HIST0: usize = 8 * MON_F + FIELD_F + DMG_F; // 776
/// (2026-10-22) extended damage block (see `encode_ext`): EXT_F floats per Pokemon after the uncertainty block
pub const EXT0: usize = HIST0 + 8 * HIST_F + UNC_F; // 1004
/// (2026-11-03, v3 13-token net) the six Pokemon of each team that the four rows don't show (see `encode_v3`)
pub const V3_0: usize = EXT0 + 8 * EXT_F; // 1548
pub const V3_F: usize = MON_F + MON_I; // 87: Pokemon features + its 7 ids as floats
/// (2026-11-03) 13-token flags: FLAG_F per Pokemon row, rows 0-7 = the obs rows, 8-15 = the v3 rows (see `encode_flags`)
pub const FLAG0: usize = V3_0 + 8 * V3_F; // 2244
pub const FLAG_F: usize = 4;
pub const OBS_FLAG: usize = FLAG0 + 16 * FLAG_F; // 2308
/// (2026-11-05, v3 external belief input) BEL_F floats per opponent row, 10 rows in the 13-token candidate order (obs rows
/// 4-7, then v3 rows 2-7): [0] belief present, [1] exact, [2..8) believed ability, item, 4 moves (dex index + 1 as floats,
/// 0 = none), [8..14) believed stats / 300, [14..16) spare. Rows the view doesn't show (unseen brought foes) stay zero.
pub const BEL0: usize = OBS_FLAG;
pub const BEL_F: usize = 16;
pub const OBS_F: usize = BEL0 + 10 * BEL_F; // 2468
const MON_I: usize = 7;
pub const OBS_I: usize = 8 * MON_I; // 56
pub const N_PREVIEW: usize = 90;
pub const PV_F: usize = 12 * MON_F; // 960
pub const PV_I: usize = 12 * MON_I; // 84

/// Team preview options: (the 4 brought, the 2 leads among them), in a fixed order.
fn preview_table() -> &'static [([u8; 4], [u8; 2]); N_PREVIEW] {
    static T: std::sync::OnceLock<[([u8; 4], [u8; 2]); N_PREVIEW]> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let mut out = [([0u8; 4], [0u8; 2]); N_PREVIEW];
        let mut k = 0;
        for a in 0..6u8 {
            for b in a + 1..6 {
                for c in b + 1..6 {
                    for d in c + 1..6 {
                        let four = [a, b, c, d];
                        for x in 0..4 {
                            for y in x + 1..4 {
                                out[k] = (four, [four[x], four[y]]);
                                k += 1;
                            }
                        }
                    }
                }
            }
        }
        out
    })
}

/// `user` = the acting Pokemon's own slot (0 / 1): a move aimed at the user itself (Acupressure / Milk Drink) gets target
/// code 0 like other self moves; code 3 = its ally (2026-10-18)
pub(crate) fn act_index(a: Action, user: usize) -> usize {
    match a {
        Action::Move { slot, target, mega } if slot < 4 => {
            let t = if target > 0 { target as usize } else if target == -(user as i8 + 1) { 0 } else if target < 0 { 3 } else { 0 };
            (slot as usize * 4 + t) * 2 + mega as usize
        }
        Action::Move { .. } => 36,
        Action::Switch { to } => 32 + (to as usize).min(3),
        Action::Pass => 37,
    }
}

/// (2026-11-05, v3 review) what an OPEN-sheet observer believes about `m` (the sheet shows the set): live, the bridge
/// always knows the set there and guesses the points, so a belief that isn't exact becomes kind 3 (set known) with the
/// guess's stats, else the most common library set's stats for the species, else none (stats 0)
fn open_bel(m: &Mon, bs: BelSet) -> BelSet {
    if bs.kind >= 2 {
        return bs;
    }
    let stats = if bs.kind == 1 {
        bs.stats
    } else {
        unc_lib().and_then(|l| {
            l.by_species.get(&m.species).or_else(|| l.by_species.get(&m.base_species))
                .and_then(|v| v.iter().max_by_key(|x| x.1).map(|x| x.0.stats))
        }).unwrap_or([0; 6])
    };
    BelSet { kind: 3, ability: m.ability_id, item: m.item_id, moves: m.moves, stats }
}

/// (2026-11-05, v3 review) side `op`'s team member of base species `base` as on the sheet (else the brought one)
fn sheet_mon(dex: &EngineDex, b: &Engine, op: usize, base: &str) -> Option<Mon> {
    let is = |m: &Mon| dex.species[m.base_species as usize].base_species == base;
    b.sheet[op].as_ref().and_then(|v| v.iter().find(|m| is(m)).copied())
        .or_else(|| b.sides[op].mons.iter().find(|m| is(m)).copied())
}

/// (2026-11-05, v3 review) side `op`'s belief about the team member of base species `base` (the brought one, else the
/// team sheet): used for imitated (Zoroark) and unseen preview rows
fn bel_of(dex: &EngineDex, b: &Engine, op: usize, base: &str) -> BelSet {
    let is = |m: &Mon| dex.species[m.base_species as usize].base_species == base;
    if let Some(m) = b.sides[op].mons.iter().find(|m| is(m)) {
        return m.bel;
    }
    b.sheet[op].as_ref().and_then(|v| v.iter().find(|m| is(m)).map(|m| m.bel))
        .or_else(|| b.teams[op].iter().find(|m| is(m)).map(|m| m.bel))
        .unwrap_or_default()
}

/// (2026-11-05, v3 review) the base species side `op`'s opponent has seen for certain: members marked seen (or whose
/// faint was seen through a disguise) plus whatever the observer's opponent rows show now (a disguised Zoroark shows
/// the member it imitates, so that member counts as seen and isn't listed as "unseen")
fn seen_bases<'a>(dex: &'a EngineDex, b: &Engine, op: usize, shown: &[&'a str]) -> Vec<&'a str> {
    let mut v: Vec<&str> = b.sides[op].mons.iter().filter(|m| m.rev & ::vgcsim::state::rev::SEEN != 0 || m.ghost_faint)
        .map(|m| dex.species[m.base_species as usize].base_species.as_str()).collect();
    for s in shown.iter().filter(|s| !s.is_empty()) {
        if !v.contains(s) {
            v.push(s);
        }
    }
    v
}

/// (2026-11-05, v3 review) gated open-sheet candidates: every team-sheet member whose base species hasn't been seen,
/// AS ON THE SHEET (full HP, no status, all items: damage / status / a fainted disguised Zoroark can't show), with
/// whether it was brought; sorted so the order leaks nothing. Without a sheet (rebuilt positions): the old way.
fn open_cands<'a>(dex: &EngineDex, b: &'a Engine, op: usize, shown: &[&str]) -> Vec<(&'a Mon, bool)> {
    let seen = seen_bases(dex, b, op, shown);
    let base = |m: &Mon| dex.species[m.base_species as usize].base_species.as_str();
    let mut cand: Vec<(&Mon, bool)> = if let Some(sh) = b.sheet[op].as_ref() {
        sh.iter().filter(|m| !seen.contains(&base(m)))
            .map(|m| (m, b.sides[op].mons.iter().any(|x| base(x) == base(m)))).collect()
    } else {
        let mut c: Vec<(&Mon, bool)> = b.sides[op].mons.iter()
            .filter(|m| m.rev & ::vgcsim::state::rev::SEEN == 0 && !m.ghost_faint && !seen.contains(&base(m)))
            .map(|m| (m, true)).collect();
        if let Some(rs) = &b.roster[op] {
            c.extend(rs.iter().map(|m| (m, false)));
        }
        c
    };
    cand.sort_by_key(|(m, _)| (m.species, m.item_id, m.ability_id));
    cand
}

/// Features of one Pokemon from `side`'s point of view (own: exact stats; foe: hidden).
fn encode_mon(dex: &EngineDex, b: &Engine, owner: usize, idx: usize, own: bool, hidden: bool, f: &mut [f32], ids: &mut [i32]) {
    let sd = &b.sides[owner];
    if idx >= sd.mons.len() {
        return;
    }
    let m = &sd.mons[idx];
    let slot = sd.active.iter().position(|&a| a == idx as u8);
    let gated = hidden || open_gate();
    if gated && !own {
        if let Some(t) = disguise_of(b, owner, idx) {
            encode_disguised(dex, b, owner, idx, t, slot, f, ids);
            return;
        }
    }
    if gated && !own && m.ghost_faint && m.alive() {
        // (2026-11-05) a disguised Zoroark imitating it fainted without a damaging hit: to the observer it fainted
        encode_mon_ref(dex, m, None, !sd.mega_used, own, f, ids);
        if hidden {
            hide_unrevealed(dex, m, f, ids);
        }
        f[1] = 0.0;
        f[2] = 0.0;
        return;
    }
    if gated && !own && m.rev & ::vgcsim::state::rev::SEEN == 0 {
        return;                                    // not seen yet: unknown
    }
    encode_mon_ref(dex, m, slot, !sd.mega_used, own, f, ids);
    if hidden && !own {
        hide_unrevealed(dex, m, f, ids);
    }
}

/// An opposing Zoroark still disguised on the field: the Pokemon it imitates (engine-tracked knowledge only; rebuilt
/// positions carry EXPLICIT reveal bits and already show what poke-env believes)
fn disguise_of(b: &Engine, owner: usize, idx: usize) -> Option<usize> {
    let sd = &b.sides[owner];
    let m = &sd.mons[idx];
    let t = m.illusion_as as usize;
    (m.illusion && m.alive() && sd.active.contains(&(idx as u8)) && t < sd.mons.len() && t != idx
        && m.rev & ::vgcsim::state::rev::EXPLICIT == 0).then_some(t)
}

/// (2026-10-17) What an observer sees of a disguised Zoroark: the imitated Pokemon's identity (species, base stats, types,
/// Mega flags, its known ability) with the Zoroark's own HP, status, boosts, volatiles and the moves it has been seen using
fn encode_disguised(dex: &EngineDex, b: &Engine, owner: usize, idx: usize, t: usize, slot: Option<usize>, f: &mut [f32], ids: &mut [i32]) {
    let sd = &b.sides[owner];
    let (z, tm) = (&sd.mons[idx], &sd.mons[t]);
    encode_mon_ref(dex, z, slot, !sd.mega_used, false, f, ids);
    hide_unrevealed(dex, z, f, ids);
    let mut tf = [0.0f32; MON_F];
    let mut ti = [0i32; MON_I];
    encode_mon_ref(dex, tm, slot, !sd.mega_used, false, &mut tf, &mut ti);
    f[25..53].copy_from_slice(&tf[25..53]);      // base stats, types, Mega flags
    if !views::item_known(dex, tm) {
        f[52] = 0.0;                               // (2026-10-22) its stone is hidden too
    }
    ids[0] = ti[0];
    ids[1] = if views::ability_known(dex, tm) { ti[1] } else { 0 };
}

/// Blank what an observer can't know about an opponent's Pokemon (hidden-information view).
fn hide_unrevealed(dex: &EngineDex, m: &Mon, f: &mut [f32], ids: &mut [i32]) {
    for k in 0..4 {
        if m.rev & (1 << k) == 0 {
            ids[3 + k] = 0;
            f[53 + k] = 0.0;
        }
    }
    for k in 57..61 {
        f[k] = 0.0;                                // choice lock: not shown
    }
    if !views::item_known(dex, m) {
        ids[2] = 0;
        f[72] = 0.0;
        f[52] = 0.0;                               // (2026-10-22) "can still Mega" = holds a stone: hidden with the item
    }
    if !views::ability_known(dex, m) {
        ids[1] = 0;
    }
}

/// Features of one Pokemon (`slot`: its active slot, if any; `mega_left`: its side can still Mega Evolve).
fn encode_mon_ref(dex: &EngineDex, m: &Mon, slot: Option<usize>, mega_left: bool, own: bool, f: &mut [f32], ids: &mut [i32]) {
    let alive = m.alive();
    f[0] = 1.0;
    f[1] = alive as u8 as f32;
    f[2] = if m.maxhp > 0 { m.hp as f32 / m.maxhp as f32 } else { 0.0 };
    // (2026-11-03, leaks) a benched foe shows the HP / status it left with (Regenerator, Natural Cure are invisible)
    let benched_foe = !own && slot.is_none() && m.bench_seen;
    let (shp, sstatus) = if benched_foe { (m.bench_hp.max(1), m.bench_status) } else { (m.hp, m.status) };
    if !own && alive && m.maxhp > 0 {
        // (2026-10-28, leaks) an opponent's HP only as Showdown shows it (percent + colour), open sheets too: the middle of
        // that range, as rebuilt positions have it (bridge.hp_from_percent). It was the exact fraction.
        let (p, c) = views::shown_hp(shp, m.maxhp);
        f[2] = views::shown_mid(p, c, m.maxhp) as f32 / m.maxhp as f32;
    }
    f[3] = (slot.is_some() && alive) as u8 as f32;
    if let Some(k) = slot {
        f[4 + k] = 1.0;
    }
    let st = match sstatus {
        Status::Burn => 1,
        Status::Paralysis => 2,
        Status::Poison => 3,
        Status::Toxic => 4,
        Status::Sleep => 5,
        Status::Freeze => 6,
        Status::None => 0,
    };
    if st > 0 {
        f[5 + st] = 1.0; // 6..11
    }
    for k in 1..8 {
        f[11 + k] = m.boosts[k] as f32 / 6.0; // 12..18
    }
    let sp = &dex.species[m.species as usize];
    for k in 0..6 {
        if own {
            f[19 + k] = m.stats[k] as f32 / 250.0; // 19..24
        }
        f[25 + k] = sp.base[k] as f32 / 200.0; // 25..30
    }
    for &t in m.types.iter() {
        if (t as usize) < 20 {
            f[31 + t as usize] = 1.0; // 31..50
        }
    }
    f[51] = m.is_mega as u8 as f32;
    f[52] = (m.mega_species != u16::MAX && mega_left) as u8 as f32;
    for k in 0..4 {
        if k < m.n_moves as usize {
            // (2026-10-16) PP are Champions' real values now (8 for Protect...), but the nets were trained when every move had
            // 20: keep the feature = (20 - PP used) / 20, so full PP is still 1.0 and each use still -0.05
            let mp = dex.moves[m.moves[k] as usize].max_pp as f32;
            f[53 + k] = if m.pp[k] == 0 { 0.0 } else { ((20.0 - (mp - m.pp[k] as f32)) / 20.0).max(0.05) }; // 53..56 (out of PP = 0)
        }
    }
    if m.choice_lock != NONE && (m.choice_lock as usize) < 4 {
        f[57 + m.choice_lock as usize] = 1.0; // 57..60
    }
    f[61] = (m.charge_slot != NONE) as u8 as f32;
    f[62] = m.protect_streak as f32 / 3.0;
    f[63] = (m.taunt > 0) as u8 as f32;
    f[64] = (m.encore > 0) as u8 as f32;
    f[65] = (m.confusion > 0) as u8 as f32;
    f[66] = (m.sub_hp > 0) as u8 as f32;
    f[67] = (m.yawn > 0) as u8 as f32;
    f[68] = m.perish as f32 / 4.0;
    f[69] = (m.throat_chop > 0) as u8 as f32;
    f[70] = (m.active_turns.min(5)) as f32 / 5.0;
    f[71] = (m.active_move_actions == 0) as u8 as f32;
    f[72] = m.has_item() as u8 as f32;
    f[73] = (m.heal_block > 0) as u8 as f32;
    f[74] = (m.bound > 0) as u8 as f32;
    f[75] = (m.disable > 0) as u8 as f32;
    f[76] = m.stockpile as f32 / 3.0;
    f[77] = (m.times_attacked.min(6)) as f32 / 6.0;
    // (2026-11-05 review) a benched foe: the status and counter it left with (Natural Cure must not show)
    let sturns = if benched_foe { m.bench_sturns } else { m.status_turns };
    f[78] = if sstatus == Status::Sleep {
        // (2026-10-18) a rebuilt sleeper whose length isn't drawn yet (0x80 | turns elapsed): the expected turns left
        if sturns & 0x80 != 0 { [8.0 / 3.0, 5.0 / 3.0, 1.0][((sturns & 0x7f) as usize).min(2)] / 3.0 } else { sturns as f32 / 3.0 }
    } else { 0.0 };
    f[79] = if sstatus == Status::Toxic { sturns as f32 / 15.0 } else { 0.0 };
    // ids (+1 so that 0 = none)
    ids[0] = m.species as i32 + 1;
    ids[1] = m.ability_id as i32 + 1;
    ids[2] = if m.item_id == u16::MAX { 0 } else { m.item_id as i32 + 1 };
    for k in 0..4 {
        ids[3 + k] = if m.moves[k] == u16::MAX { 0 } else { m.moves[k] as i32 + 1 };
    }
}

pub(crate) fn encode(dex: &EngineDex, b: &Engine, side: usize, hidden: bool, f: &mut [f32], ids: &mut [i32]) {
    encode_map(dex, b, side, None, None, hidden, f, ids)
}

/// `ours`: which of our team's Pokemon to show, in order (default: the first four)
#[allow(clippy::too_many_arguments)]
fn encode_map(dex: &EngineDex, b: &Engine, side: usize, ours: Option<&[usize]>, foes: Option<&[usize]>, hidden: bool, f: &mut [f32], ids: &mut [i32]) {
    let mut tm_ = EncT::new();
    f.fill(0.0);
    ids.fill(0);
    for (k, (owner, own)) in [(side, true), (1 - side, false)].into_iter().enumerate() {
        for pos in 0..4 {
            let map = if k == 0 { ours } else { foes };
            let idx = match map {
                Some(m) => match m.get(pos) {
                    Some(&x) => x,
                    None => continue,
                },
                None => pos,
            };
            let slot = k * 4 + pos;
            encode_mon(dex, b, owner, idx, own, hidden, &mut f[slot * MON_F..(slot + 1) * MON_F], &mut ids[slot * MON_I..(slot + 1) * MON_I]);
        }
    }
    tm_.lap(0);
    let shown_ids: [i32; 4] = std::array::from_fn(|k| if f[(4 + k) * MON_F] > 0.5 { ids[(4 + k) * MON_I] } else { 0 });
    let mut rbel = [BelSet::default(); 6];
    encode_v3(dex, b, side, hidden, &shown_ids, &mut f[V3_0..V3_0 + 8 * V3_F], &mut rbel);
    encode_flags(dex, b, side, ours, hidden, f, ids);
    encode_bel(dex, b, side, foes, &shown_ids, &rbel, hidden, &mut f[BEL0..BEL0 + 10 * BEL_F]);
    tm_.lap(1);
    let g = &mut f[8 * MON_F..];
    let w = match b.field.weather {
        Weather::None => 0,
        Weather::Rain => 1,
        Weather::Sun => 2,
        Weather::Sand => 3,
        Weather::Snow => 4,
    };
    g[w] = 1.0;
    // (2026-10-28, hidden timers) turns left as this player can know them (closed sheets: an opponent's unseen rock /
    // Terrain Extender / Light Clay -> the expected value; Battle::timers_seen)
    let tv = if hidden { b.timers_seen(dex, side) } else {
        [b.field.weather_turns as f32, b.field.terrain_turns as f32, b.sides[0].reflect as f32, b.sides[0].light_screen as f32,
         b.sides[0].aurora_veil as f32, b.sides[1].reflect as f32, b.sides[1].light_screen as f32, b.sides[1].aurora_veil as f32]
    };
    g[5] = tv[0] / 8.0;
    let t = match b.field.terrain {
        Terrain::None => 0,
        Terrain::Electric => 1,
        Terrain::Grassy => 2,
        Terrain::Psychic => 3,
        Terrain::Misty => 4,
    };
    g[6 + t] = 1.0;
    g[11] = tv[1] / 8.0;
    g[12] = b.field.trick_room as f32 / 5.0;
    g[13] = b.field.gravity as f32 / 5.0;
    g[14] = (b.turn.min(30)) as f32 / 30.0;
    for (k, s) in [side, 1 - side].into_iter().enumerate() {
        let sd = &b.sides[s];
        let o = 15 + k * 9;
        g[o] = sd.tailwind as f32 / 4.0;
        g[o + 1] = tv[2 + s * 3] / 8.0;
        g[o + 2] = tv[3 + s * 3] / 8.0;
        g[o + 3] = tv[4 + s * 3] / 8.0;
        g[o + 4] = sd.mega_used as u8 as f32;
        g[o + 5] = sd.toxic_spikes as f32 / 2.0;
        g[o + 6] = sd.fainted_total as f32 / 4.0;
        g[o + 7] = sd.healing_wish.iter().any(|&x| x) as u8 as f32;
        g[o + 8] = b.switch_needed[s].iter().any(|&x| x) as u8 as f32;
    }
    g[33] = (b.phase == Phase::Switch) as u8 as f32;
    g[34] = b.switch_needed[side][0] as u8 as f32;
    g[35] = b.switch_needed[side][1] as u8 as f32;
    g[36] = hidden as u8 as f32;
    tm_.lap(2);
    // one guessed view (opponent stats guessed / believed) for the main damage block and the extended block
    let view = {
        let p = Params::default();
        if hidden { views::guessed_view_hidden(b, dex, side, &p) } else { open_view(b, dex, side, &p) }
    };
    tm_.lap(3);
    encode_damage(dex, b, side, hidden, &view, &mut f[8 * MON_F + FIELD_F..8 * MON_F + FIELD_F + DMG_F]);
    tm_.lap(4);
    for (k, owner) in [side, 1 - side].into_iter().enumerate() {
        for pos in 0..4 {
            let map = if k == 0 { ours } else { foes };
            let idx = match map {
                Some(m) => match m.get(pos) {
                    Some(&x) => x,
                    None => continue,
                },
                None => pos,
            };
            let slot = k * 4 + pos;
            if (hidden || open_gate()) && k == 1 && idx < b.sides[owner].mons.len() && b.sides[owner].mons[idx].rev & ::vgcsim::state::rev::SEEN == 0
                && disguise_of(b, owner, idx).is_none() {
                continue;                          // not seen yet: unknown (as in encode_mon; a disguised one's moves were seen)
            }
            encode_hist(b, owner, idx, &mut f[HIST0 + slot * HIST_F..HIST0 + (slot + 1) * HIST_F]);
        }
    }
    tm_.lap(5);
    if let Some(lib) = unc_lib() {
        encode_unc(dex, b, side, hidden, &lib, &mut f[UNC0..UNC0 + UNC_F]);
    }
    tm_.lap(6);
    encode_ext(dex, b, side, hidden, ours, foes, &view, &mut f[EXT0..EXT0 + 8 * EXT_F]);
    tm_.lap(7);
}

// ---------------------------------------------------------------------------------------------------------------------
// (2026-10-22) Extended damage block: EXT_F floats per Pokemon token (our four, then theirs; the history block's token
// order), appended after the uncertainty block. For a Pokemon X, "O_j" = the OTHER side's active Pokemon in slot j.
//   [0,34)  bench block, only for a living BENCHED Pokemon (the actives' arithmetic is the main damage block): X's move m
//           vs O_j at (m * 2 + j) * 2 + {0 damage as a fraction of O_j's HP left (cap 2) x accuracy, 1 KO chance x
//           accuracy}; O_j's move m on X at 16 + (j * 4 + m) * 2 + {0, 1}; 32 + j: speed order X vs O_j (+1 X moves first,
//           -1 O_j first, 0 tie; Trick Room counted). Switch evaluation used to see 2 numbers per foe (best in / out).
//   [34,68) the same 34 for X's MEGA form (stats = X's stats + the base-stat difference, the Mega's ability and types),
//           when X's side can still Mega Evolve and X holds its stone; actives too: the Mega decision's arithmetic.
// Closed sheets: an unseen opponent Pokemon is all zeros, an opponent's moves count only when revealed, and its Mega
// block exists only when its stone is known. Zeros at team preview and after the game.
pub const EXT_F: usize = 68;

fn ext_block(dex: &EngineDex, v: &Engine, s: usize, i: usize, opp: &[Option<usize>; 2], hid: Option<usize>, out: &mut [f32]) {
    let other = 1 - s;
    let tr = v.field.trick_room > 0;
    for j in 0..2 {
        let Some(f) = opp[j] else { continue };
        let ma = if hid != Some(s) { 0x0f } else { (v.mon(s, i).rev & 0x0f) as u8 };
        let mb = if hid != Some(other) { 0x0f } else { (v.mon(other, f).rev & 0x0f) as u8 };
        let fa = dmg_feats(dex, v, s, i, other, f, ma, true);
        let fb = dmg_feats(dex, v, other, f, s, i, mb, true);
        for m in 0..4 {
            if ma & (1 << m) != 0 {
                out[(m * 2 + j) * 2] = fa[m].0;
                out[(m * 2 + j) * 2 + 1] = fa[m].1;
            }
            if mb & (1 << m) != 0 {
                out[16 + (j * 4 + m) * 2] = fb[m].0;
                out[16 + (j * 4 + m) * 2 + 1] = fb[m].1;
            }
        }
        let (sm, sf) = (v.speed(dex, s, i), v.speed(dex, other, f));
        out[32 + j] = if sm == sf { 0.0 } else if (sm > sf) != tr { 1.0 } else { -1.0 };
    }
}

/// The view with Pokemon (s, i) in its Mega form: `set_mega_form` recomputes the stats from the stat points, so a guessed /
/// believed stat keeps its offset (new = old + (Mega from points - base from points)).
fn mega_view(dex: &EngineDex, v: &Engine, s: usize, i: usize) -> Engine {
    let mut vm = v.clone();
    let m = v.mon(s, i);
    let old = m.stats;
    let base_pts = ::vgcsim::stats::champions_stats(&dex.species[m.species as usize].base, &m.points, m.nature);
    vm.set_mega_form(dex, s, i);
    let mm = &mut vm.sides[s].mons[i];
    for k in 1..6 {
        mm.stats[k] = (old[k] as i32 + mm.stats[k] as i32 - base_pts[k] as i32).max(1) as u16;
    }
    vm
}

fn encode_ext(dex: &EngineDex, b: &Engine, side: usize, hidden: bool, ours: Option<&[usize]>, foes: Option<&[usize]>, v: &Engine, out: &mut [f32]) {
    out.fill(0.0);
    if b.phase == Phase::TeamPreview || b.phase == Phase::Ended {
        return;
    }
    let hid = if hidden { Some(1 - side) } else { None };
    let gated = hidden || open_gate();
    for (k, owner) in [side, 1 - side].into_iter().enumerate() {
        let other = 1 - owner;
        let opp: [Option<usize>; 2] = [v.active(other, 0), v.active(other, 1)];
        let map = if k == 0 { ours } else { foes };
        for pos in 0..4 {
            let idx = match map {
                Some(m) => match m.get(pos) {
                    Some(&x) => x,
                    None => continue,
                },
                None => pos,
            };
            if idx >= v.sides[owner].mons.len() || !v.mon(owner, idx).alive() {
                continue;
            }
            if gated && k == 1 && ((b.sides[owner].mons[idx].rev & ::vgcsim::state::rev::SEEN == 0 && disguise_of(b, owner, idx).is_none())
                || (b.sides[owner].mons[idx].ghost_faint)) {
                continue;                          // not seen yet: unknown (as the history block); a ghost faint looks fainted
            }
            let o = (k * 4 + pos) * EXT_F;
            if !v.sides[owner].active.contains(&(idx as u8)) {
                ext_block(dex, v, owner, idx, &opp, hid, &mut out[o..o + 34]);
            }
            let m = v.mon(owner, idx);
            let stone_known = k == 0 || !hidden || views::item_known(dex, &b.sides[owner].mons[idx]);
            if stone_known && !v.sides[owner].mega_used && m.mega_species != u16::MAX && !m.is_mega {
                let vm = mega_view(dex, v, owner, idx);
                ext_block(dex, &vm, owner, idx, &opp, hid, &mut out[o + 34..o + 68]);
            }
        }
    }
}

/// The main damage block's arithmetic, UNCHANGED for the nets trained on it (no state-dependent base powers: see
/// `dmg_feat`, which the extended block uses). (mean roll / HP left capped at 2, KO share) x accuracy;
/// multi-hit moves by their expected hits. (0, 0) for status moves, immunities and empty slots.
/// (2026-11-05 speed pass) dmg_feat / dmg_feat_plain for the move slots in `mask` of one attacker / target pair (one
/// combatant + conditions build for the pair: Battle::preview_rolls_pair); slots not in the mask stay (0, 0)
#[allow(clippy::too_many_arguments)]
fn dmg_feats(dex: &EngineDex, v: &Engine, s: usize, i: usize, ts: usize, ti: usize, mask: u8, state_bp: bool) -> [(f32, f32); 4] {
    let mut out = [(0.0f32, 0.0f32); 4];
    let hp = v.mon(ts, ti).hp as f32;
    if hp <= 0.0 || mask == 0 {
        return out;
    }
    let rolls = v.preview_rolls_pair(dex, s, i, ts, ti, state_bp, mask);
    let att = v.mon(s, i);
    for m in 0..4 {
        let Some(r) = rolls[m] else { continue };
        let mv = &dex.moves[att.moves[m] as usize];
        let hits = match mv.multihit {
            (_, hi) if hi <= 1 => 1.0,
            (lo, hi) if lo == hi => lo as f32,
            (2, 5) => 3.1,
            (lo, hi) => (lo as f32 + hi as f32) / 2.0,
        };
        let acc = if mv.always_hits || mv.accuracy == 0 { 1.0 } else { mv.accuracy as f32 / 100.0 };
        let avg = r.iter().map(|&x| x as f32).sum::<f32>() / 16.0 * hits;
        let ko = r.iter().filter(|&&x| x as f32 * hits >= hp).count() as f32 / 16.0;
        out[m] = ((avg / hp).min(2.0) * acc, ko * acc);
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------------
// (2026-11-03) v3 roster block, 8 rows of V3_F = MON_F features + the MON_I ids (as floats, obs_i stays 56 wide):
//   rows 0-1  our two team members NOT brought (exact sets; no slot)
//   rows 2-7  their team members the four opponent rows don't show, packed:
//             open sheets: their two not brought (full sets, as an open-sheet foe row);
//             closed sheets: every preview species whose base species no shown opponent row has (dex only: present,
//             alive, full HP, base stats, types, species id); with no preview known (rebuilt, no set_preview): nothing.
// Old nets read fixed offsets below V3_0 and never see it.
/// (2026-11-05) one belief row (see BEL0)
fn bel_row(bs: &BelSet, o: &mut [f32]) {
    if bs.kind == 0 {
        return;
    }
    o[0] = 1.0;
    // (2026-11-05, v3 review) [1] the set (moves / item / ability) is known, [14] its stats too
    o[1] = (bs.kind >= 2) as u8 as f32;
    o[14] = (bs.kind == 2) as u8 as f32;
    let id = |x: u16| if x == u16::MAX { 0.0 } else { x as f32 + 1.0 };
    o[2] = id(bs.ability);
    o[3] = id(bs.item);
    for k in 0..4 {
        o[4 + k] = id(bs.moves[k]);
    }
    for k in 0..6 {
        o[8 + k] = bs.stats[k] as f32 / 300.0;
    }
}

/// (2026-11-05, v3 external belief input) The other side's beliefs about the opponent rows this view shows: obs rows 4-7
/// (only rows the view shows; a disguised Zoroark gets the belief about the team member it imitates) and v3 rows 2-7
/// (`rbel`, in encode_v3's order).
fn encode_bel(dex: &EngineDex, b: &Engine, side: usize, foes: Option<&[usize]>, shown_ids: &[i32; 4], rbel: &[BelSet; 6], hidden: bool,
              out: &mut [f32]) {
    let op = 1 - side;
    for pos in 0..4 {
        if shown_ids[pos] <= 0 {
            continue;
        }
        let idx = foes.and_then(|m| m.get(pos).copied()).unwrap_or(pos);
        let Some(m) = b.sides[op].mons.get(idx) else { continue };
        let shown_base = dex.species[(shown_ids[pos] - 1) as usize].base_species.as_str();
        let own = dex.species[m.base_species as usize].base_species == shown_base;
        let mut bs = if own { m.bel } else { bel_of(dex, b, op, shown_base) };
        if !hidden {
            let mm = if own { Some(*m) } else { sheet_mon(dex, b, op, shown_base) };
            bs = mm.map(|x| open_bel(&x, bs)).unwrap_or(bs);
        }
        bel_row(&bs, &mut out[pos * BEL_F..(pos + 1) * BEL_F]);
    }
    for r in 0..6 {
        bel_row(&rbel[r], &mut out[(4 + r) * BEL_F..(5 + r) * BEL_F]);
    }
}

fn encode_v3(dex: &EngineDex, b: &Engine, side: usize, hidden: bool, shown_ids: &[i32; 4], out: &mut [f32], rbel: &mut [BelSet; 6]) {
    let row = |out: &mut [f32], r: usize, m: &Mon, own: bool| {
        let mut ff = [0.0f32; MON_F];
        let mut ii = [0i32; MON_I];
        encode_mon_ref(dex, m, None, false, own, &mut ff, &mut ii);
        let o = &mut out[r * V3_F..(r + 1) * V3_F];
        o[..MON_F].copy_from_slice(&ff);
        for k in 0..MON_I {
            o[MON_F + k] = ii[k] as f32;
        }
    };
    if let Some(r) = &b.roster[side] {
        for (k, m) in r.iter().take(2).enumerate() {
            row(out, k, m, true);
        }
    }
    let op = 1 - side;
    let mut r = 2;
    // (2026-11-05, v3 review) at most 6 opponent rows in all (a disguise can show one species twice)
    let r_end = 2 + 6 - shown_ids.iter().filter(|&&x| x > 0).count();
    let mut shown: [&str; 4] = [""; 4];
    for k in 0..4 {
        if shown_ids[k] > 0 {
            shown[k] = dex.species[(shown_ids[k] - 1) as usize].base_species.as_str();
        }
    }
    if !hidden {
        if !open_gate() {
            if let Some(rs) = &b.roster[op] {
                for m in rs.iter().take(2) {
                    row(out, r, m, false);
                    rbel[r - 2] = open_bel(m, m.bel);
                    r += 1;
                }
            }
            return;
        }
        // (2026-11-03, open-sheet leak) gated open sheets: every sheet member not seen yet (unseen brought ones + the two
        // left at home), full sets, sorted by species id so the rows don't tell which were brought (open_cands)
        for (m, _) in open_cands(dex, b, op, &shown).into_iter() {
            if r >= r_end {
                break;
            }
            row(out, r, m, false);
            rbel[r - 2] = open_bel(m, m.bel);
            r += 1;
        }
        return;
    }
    let sd = &b.sides[op];
    for k in 0..(sd.n_preview as usize).min(6) {
        let spi = sd.preview[k] as usize;
        if spi >= dex.species.len() || r >= r_end {
            continue;
        }
        let sp = &dex.species[spi];
        if shown.contains(&sp.base_species.as_str()) {
            continue;
        }
        let o = &mut out[r * V3_F..(r + 1) * V3_F];
        o[0] = 1.0;
        o[1] = 1.0;
        o[2] = 1.0;
        for j in 0..6 {
            o[25 + j] = sp.base[j] as f32 / 200.0;
        }
        for &t in sp.types.iter() {
            if (t as usize) < 20 {
                o[31 + t as usize] = 1.0;
            }
        }
        o[MON_F] = spi as f32 + 1.0;
        rbel[r - 2] = bel_of(dex, b, op, sp.base_species.as_str());
        r += 1;
    }
}

/// (2026-11-03) 13-token flags, public information only, FLAG_F = 4 per row (rows: obs 0-7, then v3 0-7):
///   0 ours: the opponent has seen it (rev SEEN); theirs: shown as seen
///   1 theirs: not seen yet and could still be among the four brought (fewer than four of theirs seen)
///   2 known to be at home: our unbrought; theirs not seen once four of theirs have been seen
///   3 theirs: another of their rows shows the same base species (a Zoroark disguise copying a team member)
fn encode_flags(dex: &EngineDex, b: &Engine, side: usize, ours: Option<&[usize]>, hidden: bool, f: &mut [f32], ids: &[i32]) {
    use ::vgcsim::state::rev;
    let op = 1 - side;
    // (2026-11-05, v3 review) distinct base species seen for certain, the ones the opponent rows show now included (a
    // member imitated by a disguised Zoroark counts as seen, as for a real observer)
    let mut shown_now: [&str; 4] = [""; 4];
    for k in 0..4 {
        let r = 4 + k;
        let id = ids[r * MON_I] as i64;
        if f[r * MON_F] > 0.5 && id >= 1 && ((id - 1) as usize) < dex.species.len() {
            shown_now[k] = dex.species[(id - 1) as usize].base_species.as_str();
        }
    }
    let seen_n = seen_bases(dex, b, op, &shown_now).len();
    let gated = hidden || open_gate();
    for pos in 0..4 {
        if f[pos * MON_F] > 0.5 {
            let idx = ours.and_then(|m| m.get(pos).copied()).unwrap_or(pos);
            if idx < b.sides[side].mons.len() && b.sides[side].mons[idx].rev & rev::SEEN != 0 {
                f[FLAG0 + pos * FLAG_F] = 1.0;
            }
        }
        let r = 4 + pos;
        if f[r * MON_F] > 0.5 {
            let seen = if gated { true } else { pos < b.sides[op].mons.len() && b.sides[op].mons[pos].rev & rev::SEEN != 0 };
            f[FLAG0 + r * FLAG_F] = seen as u8 as f32;
        }
    }
    for k in 0..2 {
        if f[V3_0 + k * V3_F] > 0.5 {
            f[FLAG0 + (8 + k) * FLAG_F + 2] = 1.0;
        }
    }
    for k in 2..8 {
        if f[V3_0 + k * V3_F] > 0.5 {
            let o = FLAG0 + (8 + k) * FLAG_F;
            if !gated || seen_n >= 4 { f[o + 2] = 1.0 } else { f[o + 1] = 1.0 }
        }
    }
    // duplicate base species among their rows (obs rows 4-7 from ids, v3 rows 2-7 from the floats)
    let name = |id: i64| -> Option<&str> {
        if id >= 1 && ((id - 1) as usize) < dex.species.len() { Some(dex.species[(id - 1) as usize].base_species.as_str()) } else { None }
    };
    let mut rows: [(usize, Option<&str>); 10] = [(0, None); 10];
    for k in 0..4 {
        let r = 4 + k;
        rows[k] = (r, if f[r * MON_F] > 0.5 { name(ids[r * MON_I] as i64) } else { None });
    }
    for k in 2..8 {
        let o = V3_0 + k * V3_F;
        rows[2 + k] = (8 + k, if f[o] > 0.5 { name(f[o + MON_F].round() as i64) } else { None });
    }
    for a in 0..10 {
        if let (r, Some(n)) = rows[a] {
            if (0..10).any(|c| c != a && rows[c].1 == Some(n)) {
                f[FLAG0 + r * FLAG_F + 3] = 1.0;
            }
        }
    }
}

/// (FLAG0, FLAG_F): the 13-token flag block's layout for Python
#[pyfunction]
pub fn flag_layout() -> (usize, usize) {
    (FLAG0, FLAG_F)
}

/// (V3_0, V3_F): the v3 roster block's layout for Python
#[pyfunction]
pub fn v3_layout() -> (usize, usize) {
    (V3_0, V3_F)
}

/// (EXT0, EXT_F): the extended damage block's layout for Python
#[pyfunction]
pub fn ext_layout() -> (usize, usize) {
    (EXT0, EXT_F)
}


/// History of one Pokemon (public information: what it was seen doing), appended after the damage block:
///   0 it has used a move since switching in; 1..5 that move's slot (one-hot);
///   5 / 6 its target was the opposing slot a / b, 7 its ally, 8 itself / no target / spread (Mon::h_target);
///   9 its move last turn failed or hit nothing, 10 it worked (active Pokemon only); 11 that move was a Protect-type move;
///   12 times in a row it has used that slot (cap 3, / 3);
///   13..17 uses of each move slot this battle (cap 4, / 4); 17 Protect-type moves this battle, 18 other status moves
///   (cap 4, / 4); 19 unused (0)
fn encode_hist(b: &Engine, owner: usize, idx: usize, f: &mut [f32]) {
    let sd = &b.sides[owner];
    if idx >= sd.mons.len() {
        return;
    }
    let m = &sd.mons[idx];
    let slot = sd.active.iter().position(|&a| a == idx as u8);
    if m.h_slot != NONE && (m.h_slot as usize) < 4 {
        f[0] = 1.0;
        f[1 + m.h_slot as usize] = 1.0;
        match m.h_target {
            t @ (1 | 2) => f[4 + t as usize] = 1.0,
            3 => f[7] = 1.0,
            _ => f[8] = 1.0,
        }
        f[11] = m.h_protect as u8 as f32;
        f[12] = m.h_streak.min(3) as f32 / 3.0;
    }
    if slot.is_some() && m.alive() {
        match m.last_move_result {
            0 => f[9] = 1.0,
            1 => f[10] = 1.0,
            _ => {}
        }
    }
    for k in 0..4 {
        f[13 + k] = m.h_uses[k].min(4) as f32 / 4.0;
    }
    f[17] = m.h_protects.min(4) as f32 / 4.0;
    f[18] = m.h_status.min(4) as f32 / 4.0;
}

/// Damage features from `side`'s point of view, computed with the engine's exact damage formula on
/// the battle as `side` can know it (the opponent's stats guessed):
///   [0, 32)  ours: active slot k, move m, foe slot j -> (damage / its HP left, capped at 2; KO chance), x accuracy
///   [32, 64) theirs: foe slot j, move m, our slot k -> the same against us
///   [64, 68) speed order (k, j): +1 we move first at equal priority (Trick Room counted), -1 they do, 0 tie
///   [68, 72) our bench Pokemon b (up to 2, team order) x foe j: the most damage j can do to it
///   [72, 76) our bench Pokemon b x foe j: the most damage it can do to j
///   [76, 92) ours on our own partner: active slot k, move m (moves that hit everyone: Earthquake, Surf...)
///            -> (damage / partner's HP left, KO chance); 0 for moves that don't hit the partner
///   [92, 96) hidden information: foe j x our slot k: the estimated unseen STAB hit (the bridge's guess),
///            when j has moves not revealed yet. With `hidden`, the foes' unrevealed moves count as 0,
///            and their unrevealed items / abilities are not used (`guessed_view_hidden`).
fn encode_damage(dex: &EngineDex, b: &Engine, side: usize, hidden: bool, v: &Engine, out: &mut [f32]) {
    out.fill(0.0);
    if b.phase == Phase::TeamPreview || b.phase == Phase::Ended {
        return;
    }
    let (me, op) = (side, 1 - side);
    let st = dmg_state();
    let feats = |s: usize, i: usize, ts: usize, ti: usize, mask: u8| dmg_feats(dex, v, s, i, ts, ti, mask, st);
    let ours: [Option<usize>; 2] = [v.active(me, 0), v.active(me, 1)];
    let theirs: [Option<usize>; 2] = [v.active(op, 0), v.active(op, 1)];
    for k in 0..2 {
        let Some(i) = ours[k] else { continue };
        for j in 0..2 {
            let Some(f) = theirs[j] else { continue };
            let fo = feats(me, i, op, f, 0x0f);
            let fmask = if hidden { (v.mon(op, f).rev & 0x0f) as u8 } else { 0x0f };
            let ft = feats(op, f, me, i, fmask);
            for m in 0..4 {
                let (x, y) = fo[m];
                out[((k * 4 + m) * 2 + j) * 2] = x;
                out[((k * 4 + m) * 2 + j) * 2 + 1] = y;
                if fmask & (1 << m) != 0 {
                    let (x, y) = ft[m];
                    out[32 + ((j * 4 + m) * 2 + k) * 2] = x;
                    out[32 + ((j * 4 + m) * 2 + k) * 2 + 1] = y;
                }
            }
            let (sm, sf) = (v.speed(dex, me, i), v.speed(dex, op, f));
            let tr = v.field.trick_room > 0;
            out[64 + k * 2 + j] = if sm == sf { 0.0 } else if (sm > sf) != tr { 1.0 } else { -1.0 };
            if hidden && v.mon(op, f).rev & 0x0f != 0x0f {
                out[92 + j * 2 + k] = (views::guess_hit(dex, &Params::default(), v, op, f, me, i) as f32).min(2.0);
            }
        }
    }
    for k in 0..2 {
        let (Some(i), Some(a)) = (ours[k], ours[1 - k]) else { continue };
        let mut am = 0u8;
        for m in 0..4 {
            let id = v.mon(me, i).moves[m];
            if id != u16::MAX && dex.moves[id as usize].target == ::vgcsim::dex::Target::AllAdjacent {
                am |= 1 << m;
            }
        }
        let fp = feats(me, i, me, a, am);
        for m in 0..4 {
            if am & (1 << m) != 0 {
                out[76 + (k * 4 + m) * 2] = fp[m].0;
                out[76 + (k * 4 + m) * 2 + 1] = fp[m].1;
            }
        }
    }
    let bench: Vec<usize> = v.sides[me].bench().take(2).collect();
    for (bi, &c) in bench.iter().enumerate() {
        for j in 0..2 {
            let Some(f) = theirs[j] else { continue };
            let mut inc: f32 = 0.0;
            let mut outg: f32 = 0.0;
            let fmask = if hidden { (v.mon(op, f).rev & 0x0f) as u8 } else { 0x0f };
            let fi = feats(op, f, me, c, fmask);
            let fo = feats(me, c, op, f, 0x0f);
            for m in 0..4 {
                if fmask & (1 << m) != 0 {
                    inc = inc.max(fi[m].0);
                }
                outg = outg.max(fo[m].0);
            }
            out[68 + bi * 2 + j] = inc;
            out[72 + bi * 2 + j] = outg;
        }
    }
}

/// The team preview options, index -> (the 4 brought, the 2 leads), team indices 0-5.
#[pyfunction]
pub fn preview_options() -> Vec<(Vec<u8>, Vec<u8>)> {
    preview_table().iter().map(|(f, l)| (f.to_vec(), l.to_vec())).collect()
}

/// Features of a position (an old-API `vgcsim.Battle`, e.g. rebuilt from a live battle) from
/// `side`'s point of view: (obs_f, obs_i).
/// `ours`: the team indices of our (up to 4) brought Pokemon, in the order to show them.
#[pyfunction]
#[pyo3(signature = (battle, side, ours=None, foes=None, hidden=false))]
pub fn encode_battle(battle: &crate::Battle, side: usize, ours: Option<Vec<usize>>, foes: Option<Vec<usize>>, hidden: bool) -> (Vec<f32>, Vec<i32>) {
    let mut f = vec![0f32; OBS_F];
    let mut ids = vec![0i32; OBS_I];
    encode_map(&battle.dex, &battle.b, side.min(1), ours.as_deref(), foes.as_deref(), hidden, &mut f, &mut ids);
    (f, ids)
}

/// (2026-10-28) encode_battles([battle...], side, hidden) -> (obs_f bytes, obs_i bytes): `encode_battle` (ours / foes None)
/// for many positions in one call, in parallel without the GIL, as little-endian bytes (np.frombuffer(f, np.float32)
/// .reshape(n, -1)). ndeep's evaluate spent ~36% of its time in per-position calls returning lists of 1,548 floats.
#[pyfunction]
#[pyo3(signature = (battles, side, hidden=false))]
pub fn encode_battles(py: Python<'_>, battles: Vec<PyRef<'_, crate::Battle>>, side: usize, hidden: bool)
                      -> (Py<pyo3::types::PyBytes>, Py<pyo3::types::PyBytes>) {
    let side = side.min(1);
    let refs: Vec<(&EngineDex, &Engine)> = battles.iter().map(|b| (&*b.dex, &b.b)).collect();
    let (fb, ib) = py.detach(|| {
        let mut fb = vec![0u8; refs.len() * OBS_F * 4];
        let mut ib = vec![0u8; refs.len() * OBS_I * 4];
        fb.par_chunks_mut(OBS_F * 4).zip(ib.par_chunks_mut(OBS_I * 4)).zip(refs.par_iter())
            .for_each(|((fo, io), (dex, b))| {
                let mut f = vec![0f32; OBS_F];
                let mut ids = vec![0i32; OBS_I];
                encode_map(dex, b, side, None, None, hidden, &mut f, &mut ids);
                for (d, x) in fo.chunks_exact_mut(4).zip(f.iter()) {
                    d.copy_from_slice(&x.to_le_bytes());
                }
                for (d, x) in io.chunks_exact_mut(4).zip(ids.iter()) {
                    d.copy_from_slice(&x.to_le_bytes());
                }
            });
        (fb, ib)
    });
    (pyo3::types::PyBytes::new(py, &fb).unbind(), pyo3::types::PyBytes::new(py, &ib).unbind())
}

/// Team preview features of a position at team preview (an old-API `vgcsim.Battle` made from two full teams, before
/// choose_team) from `side`'s point of view: (pf, pi, pmask).
/// `hidden`: closed team sheets (the opponent's six as species only). For the game review's team preview analysis.
#[pyfunction]
#[pyo3(signature = (battle, side, hidden=false))]
pub fn encode_preview(battle: &crate::Battle, side: usize, hidden: bool) -> (Vec<f32>, Vec<i32>, Vec<u8>) {
    let side = side.min(1);
    let dex = &battle.dex;
    let b = &battle.b;
    let mut pfr = vec![0f32; PV_F];
    let mut pir = vec![0i32; PV_I];
    for (part, owner) in [(0usize, side), (1, 1 - side)] {
        for (k, m) in b.teams[owner].iter().take(6).enumerate() {
            let slot = part * 6 + k;
            encode_mon_ref(dex, m, None, true, part == 0, &mut pfr[slot * MON_F..(slot + 1) * MON_F],
                           &mut pir[slot * MON_I..(slot + 1) * MON_I]);
            if part == 1 && hidden {
                let (fr, ir) = (&mut pfr[slot * MON_F..(slot + 1) * MON_F], &mut pir[slot * MON_I..(slot + 1) * MON_I]);
                for k in 1..7 {
                    ir[k] = 0;
                }
                for k in 53..57 {
                    fr[k] = 0.0;
                }
                fr[72] = 0.0;
                fr[52] = 0.0;                      // (2026-10-22) stone holder = hidden item
            }
        }
    }
    let n_own = b.teams[side].len().min(6) as u8;
    let pm: Vec<u8> = preview_table().iter().map(|(four, _)| four.iter().all(|&x| x < n_own) as u8).collect();
    (pfr, pir, pm)
}

// ---------------------------------------------------------------------------------------------------------------------
// (2026-10-20) Uncertainty-aware damage features: the opponent's active Pokemon evaluated over up to UNC_K sets from a
// real-set library that match what this row knows (revealed moves / item / ability; open sheets: everything but the
// spread), instead of one guessed spread with revealed moves only. Block UNC0..UNC0+UNC_F, zeros unless a library is set
// (vgcsim.set_unc_library). Damage uses state-dependent base powers (Flail, Stored Power...). Layout:
//   [0,16)  their threat on our slot k from foe slot j, (j * 2 + k) * 4: mean / max over sets of the best move's damage
//           (fraction of HP left, capped 2), mean / max KO chance of that move
//   [16,64) our move m of slot k vs foe j, 16 + ((k * 4 + m) * 2 + j) * 3: mean damage, mean KO, min KO over sets
//   [64,68) 64 + k * 2 + j: share of sets our slot k moves first (Trick Room counted; ties 0.5)

pub const UNC_F: usize = 68;
pub const UNC0: usize = HIST0 + 8 * HIST_F; // 936
const UNC_K: usize = 4;
// (2026-10-22) open sheets: the set is known, only the spread is uncertain -> fewer draws, and a spread drawn twice is
// evaluated once (halves the block's cost on open rows; hidden rows are unchanged)
const UNC_K_OPEN: usize = 2;

/// Distinct sets per species (key: the set's species), with how often each appears in the teams given.
pub struct UncLib {
    by_species: std::collections::HashMap<u16, Vec<(Mon, u32)>>,
}

impl UncLib {
    pub fn from_teams(teams: &[Vec<Mon>]) -> UncLib {
        let mut by_species: std::collections::HashMap<u16, Vec<(Mon, u32)>> = std::collections::HashMap::new();
        for t in teams {
            for m in t {
                let list = by_species.entry(m.species).or_default();
                let mut mv = m.moves;
                mv.sort_unstable();
                let same = |c: &Mon| {
                    let mut cm = c.moves;
                    cm.sort_unstable();
                    cm == mv && c.item_id == m.item_id && c.ability_id == m.ability_id && c.points == m.points
                        && c.nature.plus == m.nature.plus && c.nature.minus == m.nature.minus
                };
                match list.iter_mut().find(|(c, _)| same(c)) {
                    Some(e) => e.1 += 1,
                    None => list.push((*m, 1)),
                }
            }
        }
        UncLib { by_species }
    }

    pub fn n_species(&self) -> usize {
        self.by_species.len()
    }
}

static UNC_LIB: std::sync::RwLock<Option<Arc<UncLib>>> = std::sync::RwLock::new(None);
/// (2026-11-03, open-sheet leak) 0 off, 1 on, 2 = not read yet (env VGCSIM_OPEN_GATE)
static OPEN_GATE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(2);

/// Open sheets show every opponent's set, but not which four they brought: with the gate on, open-sheet encodings show an
/// opposing Pokemon only once it was seen (as closed sheets do), the v3 rows list the rest of the sheet sorted by species,
/// and open-sheet search worlds redraw which sheet members fill the unseen brought slots. Before (and with it off) open
/// rows and searches knew the brought four from turn 1.
pub fn open_gate() -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    let v = OPEN_GATE.load(Relaxed);
    if v != 2 {
        return v == 1;
    }
    let on = std::env::var("VGCSIM_OPEN_GATE").map(|s| s.trim() == "1").unwrap_or(false);
    OPEN_GATE.store(on as u8, Relaxed);
    on
}

/// (2026-11-05, v3) 0 off, 1 on, 2 = not read yet (env VGCSIM_DMG_STATE)
static DMG_STATE: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(2);

/// (2026-11-05, v3) The main damage block (encode_damage) uses `dmg_feat` (state-dependent base powers: Flail, Payback, Fling,
/// Acrobatics...) instead of `dmg_feat_plain`. Off for v1 / v2 (their inputs), on for v3.
pub fn dmg_state() -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    let v = DMG_STATE.load(Relaxed);
    if v != 2 {
        return v == 1;
    }
    let on = std::env::var("VGCSIM_DMG_STATE").map(|s| s.trim() == "1").unwrap_or(false);
    DMG_STATE.store(on as u8, Relaxed);
    on
}

/// (2026-11-05) Process-wide switch for dmg_state; returns the old setting. Default = env VGCSIM_DMG_STATE=1, else off.
#[pyfunction]
pub fn set_dmg_state(on: bool) -> bool {
    let old = dmg_state();
    DMG_STATE.store(on as u8, std::sync::atomic::Ordering::Relaxed);
    old
}

// (2026-11-05, speed pass) encoder section timers: VGCSIM_ENC_PROF=1 turns them on, vgcsim.enc_prof() reads (and resets)
// the nanoseconds per section: mons, v3+flags+bel, field, view, damage, hist, unc, ext, legal, engine step
static ENC_T: [std::sync::atomic::AtomicU64; 10] = [const { std::sync::atomic::AtomicU64::new(0) }; 10];
static ENC_ON: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(2);
#[inline]
fn enc_on() -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    let v = ENC_ON.load(Relaxed);
    if v != 2 { return v == 1; }
    let on = std::env::var("VGCSIM_ENC_PROF").map(|s| s.trim() == "1").unwrap_or(false);
    ENC_ON.store(on as u8, Relaxed);
    on
}
struct EncT { on: bool, t: std::time::Instant }
impl EncT {
    #[inline]
    fn new() -> Self { let on = enc_on(); EncT { on, t: std::time::Instant::now() } }
    #[inline]
    fn lap(&mut self, k: usize) {
        if self.on {
            let n = std::time::Instant::now();
            ENC_T[k].fetch_add((n - self.t).as_nanos() as u64, std::sync::atomic::Ordering::Relaxed);
            self.t = n;
        }
    }
}
#[pyfunction]
pub fn enc_prof() -> Vec<u64> {
    ENC_T.iter().map(|x| x.swap(0, std::sync::atomic::Ordering::Relaxed)).collect()
}

/// (2026-11-03) Process-wide open-sheet gate (see open_gate); returns the old setting. Default = env VGCSIM_OPEN_GATE=1, else off.
#[pyfunction]
pub fn set_open_gate(on: bool) -> bool {
    let old = open_gate();
    OPEN_GATE.store(on as u8, std::sync::atomic::Ordering::Relaxed);
    old
}

/// (2026-11-03) Open-sheet search world: the opponent (side 1) unseen brought Pokemon swapped at random with the sheet
/// members left at home (roster), so the search doesn't know which four were brought. Without a roster: unchanged.
pub fn shuffle_open_world(e: &mut Engine, rng: &mut Rng) {
    shuffle_side_world(e, rng, 1);
}

/// (2026-11-03, our types) number of unrevealed brought slots of `op` (alive, benched, never on the field) and the size of
/// the pool they are drawn from from the other side's view (those slots + the side's roster); 0 / 0 without a roster
pub fn hidden_slots(e: &Engine, op: usize) -> (usize, usize) {
    use ::vgcsim::state::rev;
    let Some(rs) = e.roster[op].as_ref() else { return (0, 0) };
    let n = (0..e.sides[op].mons.len())
        .filter(|&i| e.sides[op].mons[i].rev & rev::SEEN == 0 && e.sides[op].mons[i].alive()
            && !e.sides[op].active.contains(&(i as u8)) && e.sides[op].transformed.map(|t| t.mon as usize) != Some(i))
        .count();
    (n, n + rs.len())
}

/// the unrevealed brought slots of side `op` redrawn from those slots + its roster (what the other side can't tell apart)
pub fn shuffle_side_world(e: &mut Engine, rng: &mut Rng, op: usize) {
    use ::vgcsim::state::rev;
    let Some(rs) = e.roster[op].clone() else { return };
    let slots: Vec<usize> = (0..e.sides[op].mons.len())
        .filter(|&i| e.sides[op].mons[i].rev & rev::SEEN == 0 && e.sides[op].mons[i].alive()
            && !e.sides[op].active.contains(&(i as u8)) && e.sides[op].transformed.map(|t| t.mon as usize) != Some(i))
        .collect();
    if slots.is_empty() || rs.is_empty() {
        return;
    }
    let mut pool: Vec<Mon> = slots.iter().map(|&i| e.sides[op].mons[i]).collect();
    pool.extend(rs.iter().copied());
    for &i in slots.iter() {
        let k = rng.below(pool.len() as u32) as usize;
        e.sides[op].mons[i] = pool.swap_remove(k);
    }
    e.roster[op] = Some(Arc::new(pool));
}
pub fn set_unc_lib(lib: Option<Arc<UncLib>>) {
    *UNC_LIB.write().unwrap() = lib;
}

fn unc_lib() -> Option<Arc<UncLib>> {
    UNC_LIB.read().unwrap().clone()
}

fn mix64(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9E3779B97F4A7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D049BB133111EB);
    x ^ (x >> 31)
}

/// Up to UNC_K sets (drawn by frequency, deterministic per position) consistent with what is known of `m` (the opponent's
/// Pokemon as this row's view holds it); the filters relax (item / ability first) when nothing matches. Empty = no data.
fn unc_candidates(dex: &EngineDex, lib: &UncLib, m: &Mon, hidden: bool, ik: bool, ak: bool, seed: u64) -> Vec<Mon> {
    let mut out = Vec::with_capacity(UNC_K);
    let list = lib.by_species.get(&m.species).or_else(|| lib.by_species.get(&m.base_species));
    let Some(list) = list else { return out };
    let mut known: Vec<u16> = Vec::with_capacity(4);
    for s in 0..4 {
        if m.moves[s] != u16::MAX && (!hidden || m.rev & (1 << s) != 0) {
            known.push(m.moves[s]);
        }
    }
    // (2026-11-05 review) ik / ak = known flags of the TRUE Pokemon: the view copy's unknown item is already removed,
    // which item_known read as "no item" from its second turn on
    let _ = dex;
    let item_known = ik;
    let ab_known = ak;
    let moves_ok = |c: &Mon| known.iter().all(|k| c.moves.contains(k));
    let item_ok = |c: &Mon| !item_known || m.item_id == u16::MAX || c.item_id == m.item_id;
    let ab_ok = |c: &Mon| !ab_known || m.ability_id != m.base_ability_id || c.ability_id == m.ability_id;
    let levels: [&dyn Fn(&Mon) -> bool; 3] = [&|c| moves_ok(c) && item_ok(c) && ab_ok(c), &|c| moves_ok(c) && item_ok(c), &|c| moves_ok(c)];
    // (2026-11-05 speed pass) one filtered pass with running totals, then a binary search per draw: the same draws as
    // walking the filtered list for each draw (first set whose running total exceeds x)
    let mut cum: Vec<(u32, u64)> = Vec::with_capacity(list.len().min(256));
    for f in levels {
        cum.clear();
        let mut total: u64 = 0;
        for (k, (c, n)) in list.iter().enumerate() {
            if *n > 0 && f(c) {
                total += *n as u64;
                cum.push((k as u32, total));
            }
        }
        if total == 0 {
            continue;
        }
        let mut h = seed;
        let k = if hidden { UNC_K } else { UNC_K_OPEN };
        for _ in 0..k {
            h = mix64(h);
            let x = h % total;
            let pos = cum.partition_point(|&(_, t)| t <= x);
            let c = &list[cum[pos].0 as usize].0;
            // open sheets: a set with the same spread as one already drawn adds nothing (moves / item / ability
            // are known there, so only points and nature matter)
            if hidden || !out.iter().any(|o: &Mon| o.points == c.points && o.nature == c.nature) {
                out.push(*c);
            }
        }
        break;
    }
    out
}

/// `m` (the view's copy of an opposing Pokemon) with set `c`: its moves, unknown item / ability, stat points and nature
/// (stats for its CURRENT species, HP fraction kept); what is known stays.
fn apply_cand(dex: &EngineDex, m: &mut Mon, c: &Mon, hidden: bool, ik: bool, ak: bool) {
    m.moves = c.moves;
    m.n_moves = c.n_moves;
    if hidden && !ik {
        m.item = c.item;
        m.item_id = c.item_id;
    }
    if hidden && !ak && !m.is_mega {
        m.ability = c.ability;
        m.ability_id = c.ability_id;
    }
    let base = dex.species[m.species as usize].base;
    let st = ::vgcsim::stats::champions_stats(&base, &c.points, c.nature);
    let frac = if m.maxhp > 0 { m.hp as f64 / m.maxhp as f64 } else { 0.0 };
    m.points = c.points;
    m.nature = c.nature;
    m.stats = st;
    m.maxhp = st[0];
    if m.alive() {
        m.hp = ((frac * st[0] as f64).round() as u16).max(1).min(st[0]);
    }
}

/// (2026-10-28, leaks) the Mega form `species` reaches with the stone `item_id` (u16::MAX = none)
fn mega_with(dex: &EngineDex, species: u16, item_id: u16) -> u16 {
    if item_id == u16::MAX {
        return u16::MAX;
    }
    let sid = dex.species[species as usize].id.as_str();
    for (from, to) in &dex.items[item_id as usize].mega {
        if from == sid {
            if let Some(m) = dex.species_index(to) {
                return m as u16;
            }
        }
    }
    u16::MAX
}

/// (2026-10-28, leaks) some Mega form `species` could reach with a stone (u16::MAX = none)
fn any_mega(dex: &EngineDex, species: u16) -> u16 {
    let sid = dex.species[species as usize].id.as_str();
    for it in dex.items.iter() {
        for (from, to) in &it.mega {
            if from == sid {
                if let Some(m) = dex.species_index(to) {
                    return m as u16;
                }
            }
        }
    }
    u16::MAX
}

/// one set from the library for `m` (an opposing Pokemon as it stands), drawn by frequency among those that contain its
/// revealed moves (always) and match its known item / ability and the item clause (relaxed in that order); None = none
fn draw_world_set(dex: &EngineDex, lib: &UncLib, m: &Mon, species: u16, used_items: &[u16], rng: &mut Rng) -> Option<Mon> {
    let list = lib.by_species.get(&species).or_else(|| lib.by_species.get(&m.base_species))?;
    let mut known: Vec<u16> = Vec::with_capacity(4);
    for k in 0..4 {
        if m.moves[k] != u16::MAX && m.rev & (1 << k) != 0 {
            known.push(m.moves[k]);
        }
    }
    let item_known = views::item_known(dex, m);
    let ab_known = views::ability_known(dex, m);
    let moves_ok = |c: &Mon| known.iter().all(|k| c.moves.contains(k));
    let item_ok = |c: &Mon| !item_known || c.item_id == m.item_id;
    let clause_ok = |c: &Mon| item_known || c.item_id == u16::MAX || !used_items.contains(&c.item_id);
    let ab_ok = |c: &Mon| !ab_known || c.ability_id == m.ability_id;
    let levels: [&dyn Fn(&Mon) -> bool; 4] = [
        &|c| moves_ok(c) && item_ok(c) && ab_ok(c) && clause_ok(c),
        &|c| moves_ok(c) && item_ok(c) && clause_ok(c),
        &|c| moves_ok(c) && clause_ok(c),
        &|c| moves_ok(c),
    ];
    for f in levels {
        let total: u64 = list.iter().filter(|(c, _)| f(c)).map(|&(_, n)| n as u64).sum();
        if total == 0 {
            continue;
        }
        let mut x = rng.below(total.min(u32::MAX as u64) as u32) as u64;
        for (c, n) in list.iter().filter(|(c, _)| f(c)) {
            if x < *n as u64 {
                return Some(*c);
            }
            x -= *n as u64;
        }
    }
    None
}

/// (2026-10-28, leaks) A closed-sheet search world (our side = side 0): what we can't know about the opponent (side 1) is
/// redrawn from the real-set library (vgcsim.set_unc_library) instead of kept true. Seen Pokemon: unrevealed move slots
/// (revealed moves keep their slots), an unknown item (Mega stone -> its Mega form; a Choice lock goes with a non-Choice
/// item) and an unknown ability; unseen brought Pokemon: which of the team-preview species they are, and their whole set.
/// Stats are drawn by the view (sampled_view / guessed_view), HP within the shown percentage. Not redrawn: a disguised
/// Zoroark (rare), a transformed Pokemon, species with no library sets. Without a library: unchanged.
pub fn hide_world_sets(dex: &EngineDex, e: &mut Engine, rng: &mut Rng) {
    use ::vgcsim::state::{rev, NONE};
    let Some(lib) = unc_lib() else { return };
    let op = 1;
    let n = e.sides[op].mons.len();
    let tf = e.sides[op].transformed.map(|t| t.mon as usize);
    let mut used: Vec<u16> = Vec::with_capacity(6);
    for i in 0..n {
        let m = &e.sides[op].mons[i];
        if (m.rev & rev::SEEN != 0 || !m.alive()) && views::item_known(dex, m) && m.item_id != u16::MAX {
            used.push(m.item_id);
        }
    }
    // seen Pokemon
    for i in 0..n {
        let m = e.sides[op].mons[i];
        if m.rev & rev::SEEN == 0 || m.illusion || tf == Some(i) {
            continue;
        }
        let item_known = views::item_known(dex, &m);
        let ab_known = views::ability_known(dex, &m);
        let hidden_slots = (0..m.n_moves.min(4) as usize).any(|k| m.moves[k] != u16::MAX && m.rev & (1 << k) == 0);
        if item_known && ab_known && !hidden_slots {
            continue;
        }
        let Some(c) = draw_world_set(dex, &lib, &m, m.species, &used, rng) else { continue };
        let mm = &mut e.sides[op].mons[i];
        let revealed: Vec<u16> = (0..4).filter(|&k| mm.moves[k] != u16::MAX && mm.rev & (1 << k) != 0).map(|k| mm.moves[k]).collect();
        let mut extra = c.moves.iter().copied().filter(|x| *x != u16::MAX && !revealed.contains(x));
        for k in 0..mm.n_moves.min(4) as usize {
            if mm.moves[k] != u16::MAX && mm.rev & (1 << k) == 0 {
                if let Some(mv) = extra.next() {
                    mm.moves[k] = mv;
                    mm.pp[k] = dex.moves[mv as usize].max_pp;
                }
            }
        }
        if !item_known {
            if mm.stash_item.0 != u16::MAX {
                mm.stash_item = (c.item_id, c.item);    // Magic Room / Klutz: the (unknown) item is set aside
            } else {
                mm.item = c.item;
                mm.item_id = c.item_id;
            }
            if c.item_id != u16::MAX {
                used.push(c.item_id);
            }
            if !mm.is_mega {
                mm.mega_species = mega_with(dex, mm.species, c.item_id);
            }
            if !matches!(mm.item, ::vgcsim::effects::It::ChoiceBand | ::vgcsim::effects::It::ChoiceSpecs | ::vgcsim::effects::It::ChoiceScarf) {
                mm.choice_lock = NONE;
            }
        }
        if !ab_known && !mm.is_mega && mm.ability_id == mm.base_ability_id && c.ability_id != u16::MAX {
            mm.ability = c.ability;
            mm.ability_id = c.ability_id;
            mm.base_ability_id = c.ability_id;
        }
    }
    // unseen brought Pokemon: which of the preview species, and their sets
    let np = e.sides[op].n_preview as usize;
    if np == 0 {
        return;
    }
    let seen: Vec<u16> = e.sides[op].mons.iter().filter(|m| m.rev & rev::SEEN != 0).map(|m| m.base_species).collect();
    let mut pool: Vec<u16> = e.sides[op].preview[..np].iter().copied()
        .filter(|sp| !seen.contains(sp))
        .collect();
    for i in 0..n {
        let m = e.sides[op].mons[i];
        if m.rev & rev::SEEN != 0 || !m.alive() || e.sides[op].active.contains(&(i as u8)) || pool.is_empty() {
            continue;
        }
        let k = rng.below(pool.len() as u32) as usize;
        let sp = pool.swap_remove(k);
        let blank = Mon { rev: 0, ..m };
        let Some(mut c) = draw_world_set(dex, &lib, &blank, sp, &used, rng) else { continue };
        if c.item_id != u16::MAX {
            used.push(c.item_id);
        }
        c.rev = 0;
        e.sides[op].mons[i] = c;
    }
}

/// (2026-10-28, leaks) The opponent's (`side`) legal actions as the other side can know them: an unknown item neither
/// Choice-locks it nor rules out a Mega stone (Mega allowed for a species that has one while its side can still Mega),
/// and every unrevealed move slot is offered with every target form (none / either foe / the partner), whatever the true
/// move is or whether Taunt, Imprison, Heal Block or Gravity would block it. Locked Pokemon (charging, Outrage, Uproar,
/// recharge, Encore: all public) keep their one option.
pub fn public_legal(dex: &EngineDex, b: &Engine, side: usize) -> [::vgcsim::policy::SlotActions; 2] {
    use ::vgcsim::battle::Action;
    use ::vgcsim::state::{vol, NONE};
    let mut pv = b.clone();
    for i in 0..pv.sides[side].mons.len() {
        if !views::item_known(dex, &pv.sides[side].mons[i]) {
            let sp = pv.sides[side].mons[i].species;
            let mm = any_mega(dex, sp);
            let m = &mut pv.sides[side].mons[i];
            m.choice_lock = NONE;
            if !m.is_mega {
                m.mega_species = mm;
            }
        }
    }
    let mut la = ::vgcsim::policy::legal_actions(&pv, dex, side);
    if pv.phase != Phase::Turn {
        return la;
    }
    for k in 0..2 {
        let Some(i) = pv.active(side, k) else { continue };
        let m = pv.mon(side, i);
        if m.charge_slot != NONE || m.lock_slot != NONE || m.uproar > 0 || m.has_vol(vol::MUST_RECHARGE) || m.encore > 0 {
            continue;
        }
        let hs: Vec<u8> = (0..m.n_moves.min(4)).filter(|&s| m.moves[s as usize] != u16::MAX && m.rev & (1 << s) == 0).collect();
        if hs.is_empty() {
            continue;
        }
        let mega_ok = pv.can_mega(side, i);
        la[k].retain(|a| !matches!(a, Action::Move { slot, .. } if hs.contains(slot) || *slot == 4));
        let mut tg: Vec<i8> = vec![0];
        for f in 0..2 {
            if pv.active(1 - side, f).is_some() {
                tg.push(f as i8 + 1);
            }
        }
        tg.push(-((1 - k) as i8 + 1));            // the partner's location (ally-only moves want it even after it fainted)
        for &sl in &hs {
            for &t in &tg {
                if la[k].len() + 2 <= la[k].capacity() {
                    la[k].push(Action::Move { slot: sl, target: t, mega: false });
                    if mega_ok {
                        la[k].push(Action::Move { slot: sl, target: t, mega: true });
                    }
                }
            }
        }
    }
    la
}

/// (2026-11-05 review) the open-sheet guessed view: with the gate on, a disguised Zoroark looks like (and has the open
/// set of) the Pokemon it imitates, as in closed views; the damage / unc / ext blocks used to see the real Zoroark
fn open_view(b: &Engine, dex: &EngineDex, side: usize, p: &Params) -> Engine {
    let mut v = views::guessed_view(b, dex, side, p);
    if open_gate() {
        views::disguise_view(&mut v, side, true);
    }
    v
}

fn encode_unc(dex: &EngineDex, b: &Engine, side: usize, hidden: bool, lib: &UncLib, out: &mut [f32]) {
    out.fill(0.0);
    if b.phase == Phase::TeamPreview || b.phase == Phase::Ended {
        return;
    }
    let p = Params::default();
    let v = if hidden { views::guessed_view_hidden(b, dex, side, &p) } else { open_view(b, dex, side, &p) };
    let (me, op) = (side, 1 - side);
    let ours: [Option<usize>; 2] = [v.active(me, 0), v.active(me, 1)];
    let theirs: [Option<usize>; 2] = [v.active(op, 0), v.active(op, 1)];
    let tr = v.field.trick_room > 0;
    for j in 0..2 {
        let Some(f) = theirs[j] else { continue };
        let m0 = *v.mon(op, f);
        let seed = ((b.turn as u64) << 32) ^ ((side as u64) << 24) ^ ((f as u64) << 16) ^ m0.species as u64;
        let tm = b.mon(op, f);
        let disg = tm.illusion && m0.species != tm.species;
        let ik = !hidden || if disg { views::item_known(dex, &m0) } else { views::item_known(dex, tm) };
        let ak = !hidden || if disg { views::ability_known(dex, &m0) } else { views::ability_known(dex, tm) };
        let cands = unc_candidates(dex, lib, &m0, hidden, ik, ak, seed);
        let mut w = v.clone();
        let n = cands.len().max(1);
        let mut thr = [[0.0f32; 4]; 2];        // per our slot: sum dmg, max dmg, sum ko, max ko
        let mut our = [[[0.0f32, 0.0, 1.0]; 4]; 2]; // per our slot, move: sum dmg, sum ko, min ko
        let mut first = [0.0f32; 2];
        // (2026-11-05 speed pass) candidates are drawn with replacement: identical applied sets give identical numbers,
        // so each distinct one is computed once and its sums weighted by how often it was drawn (max / min unchanged)
        let mut buf = [(m0, 0.0f32); UNC_K];
        let mut na = 0usize;
        if cands.is_empty() {
            buf[0].1 = 1.0;
            na = 1;
        } else {
            for c in cands.iter().take(UNC_K) {
                let mut mm = m0;
                apply_cand(dex, &mut mm, c, hidden, ik, ak);
                if let Some(x) = buf[..na].iter_mut().find(|(a, _)| a.moves == mm.moves && a.item_id == mm.item_id
                        && a.ability_id == mm.ability_id && a.points == mm.points && a.nature == mm.nature
                        && a.n_moves == mm.n_moves && a.stats == mm.stats && a.hp == mm.hp) {
                    x.1 += 1.0;
                } else {
                    buf[na] = (mm, 1.0);
                    na += 1;
                }
            }
        }
        // (2026-11-05 speed pass) our hits on them and the speed order don't depend on their moves: candidates that
        // differ only in moves reuse the first such candidate's numbers
        let mut defn = [[[(0.0f32, 0.0f32); 4]; 2]; UNC_K];
        let mut fsto = [[0.0f32; 2]; UNC_K];
        for (ci, &(mm, wt)) in buf[..na].iter().enumerate() {
            let same = buf[..ci].iter().position(|(a, _)| a.item_id == mm.item_id && a.item == mm.item
                && a.ability_id == mm.ability_id && a.ability == mm.ability && a.stats == mm.stats
                && a.hp == mm.hp && a.maxhp == mm.maxhp);
            let fallback = cands.is_empty();
            if !fallback {
                w.sides[op].mons[f] = mm;
            }
            for k in 0..2 {
                let Some(i) = ours[k] else { continue };
                let mut best = (0.0f32, 0.0f32);
                // no library data: revealed moves only (as the old block)
                let tmask = if fallback && hidden { (m0.rev & 0x0f) as u8 } else { 0x0f };
                let tf = dmg_feats(dex, &w, op, f, me, i, tmask, true);
                for mv in 0..4 {
                    if tmask & (1 << mv) == 0 {
                        continue;
                    }
                    let x = tf[mv];
                    if x.0 > best.0 {
                        best = x;
                    }
                }
                thr[k][0] += wt * best.0;
                thr[k][1] = thr[k][1].max(best.0);
                thr[k][2] += wt * best.1;
                thr[k][3] = thr[k][3].max(best.1);
                let uf = match same {
                    Some(s0) => defn[s0][k],
                    None => dmg_feats(dex, &w, me, i, op, f, 0x0f, true),
                };
                defn[ci][k] = uf;
                for mv in 0..4 {
                    let x = uf[mv];
                    our[k][mv][0] += wt * x.0;
                    our[k][mv][1] += wt * x.1;
                    our[k][mv][2] = our[k][mv][2].min(x.1);
                }
                let fk = match same {
                    Some(s0) => fsto[s0][k],
                    None => {
                        let (sm, sf) = (w.speed(dex, me, i), w.speed(dex, op, f));
                        if sm == sf { 0.5 } else if (sm > sf) != tr { 1.0 } else { 0.0 }
                    }
                };
                fsto[ci][k] = fk;
                first[k] += wt * fk;
            }
        }
        let nf = n as f32;
        for k in 0..2 {
            if ours[k].is_none() {
                continue;
            }
            let o = (j * 2 + k) * 4;
            out[o] = thr[k][0] / nf;
            out[o + 1] = thr[k][1];
            out[o + 2] = thr[k][2] / nf;
            out[o + 3] = thr[k][3];
            for mv in 0..4 {
                let o = 16 + ((k * 4 + mv) * 2 + j) * 3;
                out[o] = our[k][mv][0] / nf;
                out[o + 1] = our[k][mv][1] / nf;
                out[o + 2] = our[k][mv][2];
            }
            out[64 + k * 2 + j] = first[k] / nf;
        }
    }
}

/// (2026-10-20) Sets (or with None clears) the real-set library behind the uncertainty-aware damage features (obs_f
/// columns UNC0..UNC0+UNC_F, zeros without one): every Pokemon of `teams` (Showdown export texts). Process-wide: every
/// encoding (encode_battle, search leaves) uses it. Returns the number of species in it.
#[pyfunction]
#[pyo3(signature = (dex, teams=None))]
pub fn set_unc_library(dex: &crate::Dex, teams: Option<Vec<String>>) -> usize {
    let Some(teams) = teams else {
        set_unc_lib(None);
        return 0;
    };
    let d = dex.inner.clone();
    let built: Vec<Vec<Mon>> = teams.iter().filter_map(|t| build_team(&d, &parse_team(t)).ok()).collect();
    let lib = UncLib::from_teams(&built);
    let n = lib.n_species();
    set_unc_lib(Some(Arc::new(lib)));
    n
}

/// (OBS_F, UNC0, UNC_F): feature layout constants for Python
#[pyfunction]
pub fn obs_layout() -> (usize, usize, usize) {
    (OBS_F, UNC0, UNC_F)
}
