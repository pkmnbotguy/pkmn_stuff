//! Python bindings for vgcsim.
//!
//!     import vgcsim
//!     dex = vgcsim.Dex("engines/vgcsim/data/dex.json")
//!     b = vgcsim.Battle(dex, team_text_a, team_text_b, seed=1)
//!     b.choose_team(0, [0, 1, 2, 3]); b.choose_team(1, [0, 1, 2, 3])
//!     while b.phase != "ended":
//!         for side in (0, 1):
//!             if b.needs_choice(side):
//!                 opts = b.legal_actions(side)          # one list per active slot
//!                 b.choose(side, opts[0][0], opts[1][0])
//!
//! Actions are tuples: ("move", slot 0-3 (4 = Struggle), target, mega), ("switch", index, 0, False),
//! ("pass", 0, 0, False). Targets follow Showdown: 1, 2 = opposing slots; -1, -2 = own slots; 0 = none.

use std::sync::Arc;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyBytes, PyDict, PyList};
use rayon::prelude::*;

use ::vgcsim::battle::{Action, Battle as Engine, Phase};
use ::vgcsim::damage::{Status, Terrain, Weather};
use ::vgcsim::dex::{Dex as EngineDex, BOOST_IDS, STAT_IDS};
use ::vgcsim::policy;
use ::vgcsim::state::{parse_team, vol, vol2, Mon, Rng, NONE};

mod exact;
mod encode;

pub(crate) type ActionTuple = (String, i64, i64, bool);

fn to_tuple(a: Action) -> ActionTuple {
    match a {
        Action::Move { slot, target, mega } => ("move".into(), slot as i64, target as i64, mega),
        Action::Switch { to } => ("switch".into(), to as i64, 0, false),
        Action::Pass => ("pass".into(), 0, 0, false),
    }
}

fn from_tuple(t: &ActionTuple) -> PyResult<Action> {
    Ok(match t.0.as_str() {
        "move" => Action::Move { slot: t.1 as u8, target: t.2 as i8, mega: t.3 },
        "switch" => Action::Switch { to: t.1 as u8 },
        "pass" => Action::Pass,
        other => return Err(PyValueError::new_err(format!("unknown action kind {other}"))),
    })
}

fn status_name(s: Status) -> &'static str {
    match s {
        Status::None => "",
        Status::Burn => "brn",
        Status::Paralysis => "par",
        Status::Poison => "psn",
        Status::Toxic => "tox",
        Status::Sleep => "slp",
        Status::Freeze => "frz",
    }
}

fn parse_status(s: &str) -> PyResult<Status> {
    Ok(match s {
        "" => Status::None,
        "brn" => Status::Burn,
        "par" => Status::Paralysis,
        "psn" => Status::Poison,
        "tox" => Status::Toxic,
        "slp" => Status::Sleep,
        "frz" => Status::Freeze,
        _ => return Err(PyValueError::new_err(format!("unknown status {s}"))),
    })
}

fn weather_name(w: Weather) -> &'static str {
    match w {
        Weather::None => "",
        Weather::Rain => "raindance",
        Weather::Sun => "sunnyday",
        Weather::Sand => "sandstorm",
        Weather::Snow => "snowscape",
    }
}

fn parse_weather(s: &str) -> PyResult<Weather> {
    Ok(match s {
        "" | "none" => Weather::None,
        "raindance" | "rain" => Weather::Rain,
        "sunnyday" | "sun" => Weather::Sun,
        "sandstorm" | "sand" => Weather::Sand,
        "snowscape" | "snow" | "hail" => Weather::Snow,
        _ => return Err(PyValueError::new_err(format!("unknown weather {s}"))),
    })
}

fn terrain_name(t: Terrain) -> &'static str {
    match t {
        Terrain::None => "",
        Terrain::Electric => "electricterrain",
        Terrain::Grassy => "grassyterrain",
        Terrain::Psychic => "psychicterrain",
        Terrain::Misty => "mistyterrain",
    }
}

fn parse_terrain(s: &str) -> PyResult<Terrain> {
    Ok(match s {
        "" | "none" => Terrain::None,
        "electricterrain" | "electric" => Terrain::Electric,
        "grassyterrain" | "grassy" => Terrain::Grassy,
        "psychicterrain" | "psychic" => Terrain::Psychic,
        "mistyterrain" | "misty" => Terrain::Misty,
        _ => return Err(PyValueError::new_err(format!("unknown terrain {s}"))),
    })
}

fn conv_joint(v: &[(ActionTuple, ActionTuple)]) -> PyResult<Vec<[Action; 2]>> {
    v.iter().map(|(a, b)| Ok([from_tuple(a)?, from_tuple(b)?])).collect()
}

/// the search's worlds (our side = side 0): world 0 = the opponent guessed as the bridge does, the others sampled builds
/// (2026-10-28, hidden timers) hidden = closed sheets: each world also draws the lengths of weather / terrain / screens whose
/// setter's item we can't know (Battle::hide_timers)
fn search_views(b: &Engine, dex: &EngineDex, side: usize, worlds: usize, seed: u64, keep: bool, hidden: bool) -> Vec<Engine> {
    let mut v = search_views_raw(b, dex, side, worlds, seed, keep);
    if hidden {
        if !keep {
            // (2026-10-28, leaks) the opponent's unrevealed moves / item / ability and unseen brought Pokemon redrawn per
            // world (they were the TRUE ones: the closed-sheet search knew them); rebuilt positions hold only public info
            for (w, e) in v.iter_mut().enumerate() {
                let mut r = Rng::new(seed ^ 0x5E75_1EAC ^ (w as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
                encode::hide_world_sets(dex, e, &mut r);
            }
        }
        hide_world_timers(&mut v, dex, seed);
    } else if !keep && encode::open_gate() {
        // (2026-11-03, open-sheet leak) open sheets: which sheet members fill the unseen brought slots is redrawn per world
        for (w, e) in v.iter_mut().enumerate() {
            let mut r = Rng::new(seed ^ 0x09E4_6A7E ^ (w as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
            encode::shuffle_open_world(e, &mut r);
        }
    }
    v
}

fn hide_world_timers(v: &mut [Engine], dex: &EngineDex, seed: u64) {
    for (w, e) in v.iter_mut().enumerate() {
        let mut r = Rng::new(seed ^ 0x71E2_7135 ^ (w as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        e.hide_timers(dex, 0, &mut r);
    }
}

fn search_views_raw(b: &Engine, dex: &EngineDex, side: usize, worlds: usize, seed: u64, keep: bool) -> Vec<Engine> {
    let base = b.search_copy(side == 1);
    if keep {
        // (2026-10-21) rebuilt positions: the opponent's stats are already the bridge's belief (open sheet, known sets,
        // spread inference, library fills per world); keep them, worlds differ only in their random rolls
        return (0..worlds).map(|_| base.clone()).collect();
    }
    let p = ::vgcsim::views::Params::default();
    let mut wrng = Rng::new(seed ^ 0xA5A5_5A5A);
    (0..worlds).map(|w| if w == 0 {
        ::vgcsim::views::guessed_view(&base, dex, 0, &p)
    } else {
        ::vgcsim::views::sampled_view(&base, dex, 0, &mut wrng)
    }).collect()
}


/// (2026-10-16) A batch of positions played forward by the network (Battle.search_children): observe() -> the network
/// picks an action index per side and slot -> act(); repeat. Rows are (position, side): row 2k = side 0 of position k.
#[pyclass]
struct Rollouts {
    dex: Arc<EngineDex>,
    engines: Vec<Engine>,
}


// (2026-10-18, bridge) every per-Pokemon volatile counter / flag of the fast engine, by name, in the ENGINE's own convention
// (raw field values): Battle.state() reports the nonzero ones under "vols", Battle.set_vols() writes them. Used to diff a
// rebuilt position against Showdown's serialized battle and to rebuild the volatiles a player can infer from the log.
macro_rules! vol_fields {
    ($m:ident, $f:ident) => {
        $f!($m, "confusion", confusion); $f!($m, "taunt", taunt); $f!($m, "encore", encore); $f!($m, "encore_slot", encore_slot);
        $f!($m, "throatchop", throat_chop); $f!($m, "magnetrise", magnet_rise); $f!($m, "disable", disable);
        $f!($m, "disable_slot", disable_slot); $f!($m, "perish", perish); $f!($m, "yawn", yawn); $f!($m, "substitute", sub_hp);
        $f!($m, "healblock", heal_block); $f!($m, "stockpile", stockpile); $f!($m, "syrupbomb", syrup);
        $f!($m, "syrup_src", syrup_src); $f!($m, "syrup_side", syrup_side); $f!($m, "partiallytrapped", bound);
        $f!($m, "bound_div", bound_div); $f!($m, "bound_src", bound_src); $f!($m, "bound_side", bound_side);
        $f!($m, "octolock_src", octolock_src); $f!($m, "octolock_side", octolock_side); $f!($m, "trap_src", trap_src);
        $f!($m, "trap_side", trap_side); $f!($m, "leech_slot", leech_slot); $f!($m, "leech_side", leech_side);
        $f!($m, "attract_src", attract_src); $f!($m, "attract_side", attract_side); $f!($m, "lock_slot", lock_slot);
        $f!($m, "lockedmove", lock_dur); $f!($m, "lock_true", lock_true); $f!($m, "charge_slot", charge_slot); $f!($m, "charge_target", charge_target);
        $f!($m, "uproar", uproar); $f!($m, "uproar_slot", uproar_slot); $f!($m, "lockon", lock_on);
        $f!($m, "status_turns", status_turns); $f!($m, "stall", protect_streak); $f!($m, "active_turns", active_turns);
        $f!($m, "allyswitch", ally_sw_dur); $f!($m, "allyswitch_counter", ally_sw);
        $f!($m, "usedmask", used_mask); $f!($m, "timesattacked", times_attacked); $f!($m, "fallen", fallen);
        $f!($m, "last_target", last_target);       // (2026-10-19) Showdown lastMoveTargetLoc: Instruct
    };
}
const VOL_BITS: &[(&str, u32)] = &[
    ("flashfire", vol::FLASH_FIRE), ("charge", vol::CHARGED), ("focusenergy", vol::FOCUS_ENERGY), ("dragoncheer", vol::DRAGON_CHEER),
    ("dragoncheer2", vol::DRAGON_CHEER2), ("destinybond", vol::DESTINY_BOND), ("minimize", vol::MINIMIZE), ("curse", vol::CURSE),
    ("noretreat", vol::NO_RETREAT), ("glaiverush", vol::GLAIVE_RUSH), ("mustrecharge", vol::MUST_RECHARGE), ("imprison", vol::IMPRISON),
    ("leechseed", vol::LEECH_SEED), ("twoturnmove", vol::CHARGING), ("transform", vol::TRANSFORMED), ("roost", vol::ROOST),
    ("libero", vol::LIBERO_USED),
    // (2026-10-18 bridge fuzz) this-turn flags (Showdown keeps the lead switch-ins' ones through turn 1): Lash Out, Burning
    // Jealousy, Alluring Voice, Assurance, Stomping-style checks
    ("statsraised", vol::STATS_RAISED), ("statslowered", vol::STATS_LOWERED), ("hurtthisturn", vol::DAMAGED_THIS_TURN),
];
const VOL2_BITS: &[(&str, u32)] = &[
    ("attract", vol2::ATTRACT), ("aquaring", vol2::AQUA_RING), ("saltcure", vol2::SALT_CURE), ("electrify", vol2::ELECTRIFY),
    ("powertrick", vol2::POWER_TRICK), ("smackdown", vol2::SMACK_DOWN), ("torment", vol2::TORMENT), ("ingrain", vol2::INGRAIN),
    ("gastroacid", vol2::GASTRO), ("split", vol2::SPLIT),
];
fn vols_of(m: &Mon) -> Vec<(&'static str, i64)> {
    let mut out = Vec::new();
    macro_rules! get { ($m:ident, $n:expr, $x:ident) => { let v = $m.$x as i64; if v != 0 && !(v == NONE as i64 && $n.contains('_')) { out.push(($n, v)); } }; }
    vol_fields!(m, get);
    // (2026-10-18 bridge fuzz) Metronome (item): "metronome" = consecutive count + 1 (0 = no volatile), "metronome_slot" = the
    // move slot of its last move (255 = none)
    if m.metro_on {
        out.push(("metronome", m.metro_n as i64 + 1));
        let k = (0..4).find(|&k| m.moves[k] == m.metro_move && m.metro_move != u16::MAX).map(|k| k as i64).unwrap_or(255);
        if k != 255 { out.push(("metronome_slot", k)); }
    }
    // (2026-10-19 bridge fuzz) a lock / Uproar / charge that a caller (Copycat) started: the move it runs (dex index)
    if m.call_move != u16::MAX && (m.lock_slot == ::vgcsim::battle::CALL_SLOT || m.charge_slot == ::vgcsim::battle::CALL_SLOT
        || (m.uproar > 0 && m.uproar_slot == ::vgcsim::battle::CALL_SLOT)) {
        out.push(("call_move", m.call_move as i64));
    }
    if m.used_item_turn { out.push(("useditem", 1)); }
    if m.illusion { out.push(("illusion", 1)); out.push(("illusion_as", m.illusion_as as i64)); }   // (2026-10-19) disguised
    if m.ghost_faint { out.push(("ghostfaint", 1)); }   // (2026-11-05) looks fainted to the other side (disguised Zoroark fainted)
    if m.ate_berry { out.push(("ateberry", 1)); }
    if m.syrup_done { out.push(("syrupdone", 1)); }       // (2026-10-19 bridge flag 15161) Supersweet Syrup used (once per battle)          // (2026-10-19) Belch: ate a berry this battle (kept across switches)
    for (n, b) in VOL_BITS { if m.vol & b != 0 { out.push((n, 1)); } }
    for (n, b) in VOL2_BITS { if m.vol2 & b != 0 { out.push((n, 1)); } }
    out
}
fn set_vol(m: &mut Mon, name: &str, v: i64) -> bool {
    macro_rules! put { ($m:ident, $n:expr, $x:ident) => { if name == $n { $m.$x = v as _; return true; } }; }
    vol_fields!(m, put);
    if name == "metronome" {
        m.metro_on = v != 0;
        m.metro_n = if v > 0 { (v - 1).min(255) as u8 } else { 0 };
        if v == 0 { m.metro_move = u16::MAX; }
        return true;
    }
    if name == "call_move" {
        m.call_move = if v >= 0 && v < u16::MAX as i64 { v as u16 } else { u16::MAX };
        return true;
    }
    // (2026-10-19 bridge flag 17320) Illusion disguise: from_brought turns it on for every lead Zoroark; the bridge sets the
    // real state (a revealed Zoroark is not disguised: Transform / Imposter work on it). illusion_as = party index imitated.
    if name == "illusion" {
        m.illusion = v != 0;
        if v == 0 { m.illusion_as = ::vgcsim::state::NONE; }
        return true;
    }
    if name == "illusion_as" {
        m.illusion_as = if (0..6).contains(&v) { v as u8 } else { ::vgcsim::state::NONE };
        return true;
    }
    if name == "ghostfaint" {
        m.ghost_faint = v != 0;
        return true;
    }
    if name == "syrupdone" {
        m.syrup_done = v != 0;
        return true;
    }
    if name == "ateberry" {
        m.ate_berry = v != 0;
        return true;
    }
    if name == "useditem" {
        m.used_item_turn = v != 0;                            // Pickup
        return true;
    }
    if name == "metronome_slot" {
        m.metro_move = if (0..4).contains(&v) { m.moves[v as usize] } else { u16::MAX };
        return true;
    }
    for (n, b) in VOL_BITS { if name == *n { if v != 0 { m.vol |= b; } else { m.vol &= !b; } return true; } }
    for (n, b) in VOL2_BITS { if name == *n { if v != 0 { m.vol2 |= b; } else { m.vol2 &= !b; } return true; } }
    false
}

#[pymethods]
impl Rollouts {
    fn __len__(&self) -> usize {
        self.engines.len()
    }

    /// (obs_f bytes [n*2, OBS_F], obs_i bytes [n*2, OBS_I], mask bytes [n*2, 2, N_ACT], need bytes [n*2]): every
    /// position from both sides' view (rows that don't have to choose are zero with need 0).
    #[allow(clippy::type_complexity)]
    fn observe(&self, py: Python<'_>, hidden: bool) -> (Py<PyBytes>, Py<PyBytes>, Py<PyBytes>, Py<PyBytes>) {
        let (nf, ni, na) = (encode::OBS_F, encode::OBS_I, encode::N_ACT);
        let dex = &self.dex;
        let rows: Vec<(Vec<f32>, Vec<i32>, Vec<u8>, u8)> = py.detach(|| {
            self.engines.par_iter().flat_map_iter(|e| (0..2usize).map(move |side| {
                let mut f = vec![0f32; nf];
                let mut ids = vec![0i32; ni];
                let mut m = vec![0u8; 2 * na];
                if e.phase == Phase::Ended || !e.needs_choice(side) {
                    return (f, ids, m, 0u8);
                }
                encode::encode(dex, e, side, hidden, &mut f, &mut ids);
                let legal = policy::legal_actions(e, dex, side);
                for slot in 0..2 {
                    for &a in legal[slot].iter() {
                        m[slot * na + encode::act_index(a, slot)] = 1;
                    }
                    if legal[slot].is_empty() {
                        m[slot * na + 37] = 1;
                    }
                }
                (f, ids, m, 1u8)
            })).collect()
        });
        let mut f: Vec<u8> = Vec::with_capacity(rows.len() * nf * 4);
        let mut ids: Vec<u8> = Vec::with_capacity(rows.len() * ni * 4);
        let mut m: Vec<u8> = Vec::with_capacity(rows.len() * 2 * na);
        let mut need: Vec<u8> = Vec::with_capacity(rows.len());
        for (a, b, c, d) in rows {
            for x in a {
                f.extend_from_slice(&x.to_le_bytes());
            }
            for x in b {
                ids.extend_from_slice(&x.to_le_bytes());
            }
            m.extend(c);
            need.push(d);
        }
        (PyBytes::new(py, &f).unbind(), PyBytes::new(py, &ids).unbind(), PyBytes::new(py, &m).unbind(),
         PyBytes::new(py, &need).unbind())
    }

    /// Apply one action index per (row, slot) (`acts`: n*2*2 ints, rows as in observe; illegal -> first legal) for every
    /// side that must choose, then run until the next decision.
    fn act(&mut self, py: Python<'_>, acts: Vec<i32>) -> PyResult<()> {
        if acts.len() != self.engines.len() * 4 {
            return Err(PyValueError::new_err("acts: need 4 per position"));
        }
        let dex = &self.dex;
        let engines = &mut self.engines;
        py.detach(|| {
            engines.par_iter_mut().enumerate().for_each(|(k, e)| {
                if acts[k * 4] == -2 {
                    return; // (-2 in the first slot: leave this position as it is)
                }
                for side in 0..2 {
                    if e.phase == Phase::Ended || !e.needs_choice(side) {
                        continue;
                    }
                    let legal = policy::legal_actions(e, dex, side);
                    let mut pair = [Action::Pass; 2];
                    for slot in 0..2 {
                        let want = acts[k * 4 + side * 2 + slot];
                        pair[slot] = legal[slot].iter().copied().find(|&a| encode::act_index(a, slot) as i32 == want)
                            .or_else(|| legal[slot].first().copied()).unwrap_or(Action::Pass);
                    }
                    if let (Action::Switch { to: a }, Action::Switch { to: b }) = (pair[0], pair[1]) {
                        if a == b {
                            pair[1] = legal[1].iter().copied().find(|x| *x != pair[0]).unwrap_or(Action::Pass);
                        }
                    }
                    e.choose(dex, side, pair);
                }
            });
        });
        Ok(())
    }

    /// Per position: the turn number, and +1 / -1 / 0 for a finished game (side 0 won / lost / tie), NaN while it goes on.
    fn status(&self) -> (Vec<u32>, Vec<f32>) {
        let turns = self.engines.iter().map(|e| e.turn as u32).collect();
        let term = self.engines.iter().map(|e| if e.phase == Phase::Ended {
            match e.winner { Some(0) => 1.0, Some(1) => -1.0, _ => 0.0 }
        } else { f32::NAN }).collect();
        (turns, term)
    }

    /// Side 0's view of every position (for the value net at the end): (obs_f bytes [n, OBS_F], obs_i bytes [n, OBS_I]).
    fn encode0(&self, py: Python<'_>, hidden: bool) -> (Py<PyBytes>, Py<PyBytes>) {
        let (nf, ni) = (encode::OBS_F, encode::OBS_I);
        let dex = &self.dex;
        let rows: Vec<(Vec<f32>, Vec<i32>)> = py.detach(|| self.engines.par_iter().map(|e| {
            let mut f = vec![0f32; nf];
            let mut ids = vec![0i32; ni];
            if e.phase != Phase::Ended {
                encode::encode(dex, e, 0, hidden, &mut f, &mut ids);
            }
            (f, ids)
        }).collect());
        let mut fb: Vec<u8> = Vec::with_capacity(rows.len() * nf * 4);
        let mut ib: Vec<u8> = Vec::with_capacity(rows.len() * ni * 4);
        for (a, b) in rows {
            for x in a {
                fb.extend_from_slice(&x.to_le_bytes());
            }
            for x in b {
                ib.extend_from_slice(&x.to_le_bytes());
            }
        }
        (PyBytes::new(py, &fb).unbind(), PyBytes::new(py, &ib).unbind())
    }
}

// (2026-11-02, profiling) exclusive time + calls per part of the endgame solver; only with `--features egprof`
const EGP_ENUM_CLONE: usize = 0;
const EGP_PLAY_TURN: usize = 1;
const EGP_ENUM_OTHER: usize = 2;
const EGP_KEY_ENCODE: usize = 3;
const EGP_SKEY_CANON: usize = 4;
const EGP_JOINTS: usize = 5;
const EGP_SEARCH_COPY: usize = 6;
const EGP_GAME_SOLVE: usize = 7;
const EGP_PROVEN: usize = 8;
const EGP_TABLE: usize = 9;
const EGP_CELL: usize = 10;
const EGP_ACCUM: usize = 11;
const EGP_REDUCE: usize = 12;
const EGP_TURN_DBL: usize = 13;
const EGP_TURN_LAZY: usize = 14;
const EGP_VAL_BODY: usize = 15;
const EGP_MERGE_LEAVES: usize = 16;
const EGP_ROOT: usize = 17;
const EGP_OTHER: usize = 18;
const EGP_NAMES: [&str; 19] = ["enum_clone", "play_turn", "enum_other", "key_encode", "skey_canon", "joints", "search_copy", "game_solve", "proven", "table", "cell", "accum", "reduce", "turn_dbl", "turn_lazy", "val_body", "merge_leaves", "root", "other"];
#[cfg(feature = "egprof")]
static EGP_NS: [std::sync::atomic::AtomicU64; 19] = [const { std::sync::atomic::AtomicU64::new(0) }; 19];
#[cfg(feature = "egprof")]
static EGP_CALLS: [std::sync::atomic::AtomicU64; 19] = [const { std::sync::atomic::AtomicU64::new(0) }; 19];
#[cfg(feature = "egprof")]
thread_local! {
    // per open guard: child time so far (ns)
    static EGP_STACK: std::cell::RefCell<Vec<u64>> = const { std::cell::RefCell::new(Vec::new()) };
}
#[cfg(feature = "egprof")]
struct EgpGuard { cat: usize, t0: std::time::Instant }
#[cfg(feature = "egprof")]
#[inline(always)]
fn egp(cat: usize) -> EgpGuard {
    EGP_STACK.with(|st| st.borrow_mut().push(0));
    EgpGuard { cat, t0: std::time::Instant::now() }
}
#[cfg(feature = "egprof")]
impl Drop for EgpGuard {
    fn drop(&mut self) {
        use std::sync::atomic::Ordering::Relaxed;
        let el = self.t0.elapsed().as_nanos() as u64;
        EGP_STACK.with(|st| {
            let mut st = st.borrow_mut();
            let child = st.pop().unwrap_or(0);
            EGP_NS[self.cat].fetch_add(el.saturating_sub(child), Relaxed);
            EGP_CALLS[self.cat].fetch_add(1, Relaxed);
            if let Some(p) = st.last_mut() { *p += el; }
        });
    }
}
#[cfg(not(feature = "egprof"))]
#[inline(always)]
fn egp(_cat: usize) {}

/// (2026-11-02, profiling) [(part, exclusive ns, calls)] since the last reset; empty without `--features egprof`
#[pyfunction]
#[pyo3(signature = (reset=false))]
fn eg_prof(reset: bool) -> Vec<(String, u64, u64)> {
    #[cfg(feature = "egprof")]
    {
        use std::sync::atomic::Ordering::Relaxed;
        let out = (0..EGP_NAMES.len()).map(|i| (EGP_NAMES[i].to_string(), EGP_NS[i].load(Relaxed), EGP_CALLS[i].load(Relaxed))).collect();
        if reset { for i in 0..EGP_NAMES.len() { EGP_NS[i].store(0, Relaxed); EGP_CALLS[i].store(0, Relaxed); } }
        out
    }
    #[cfg(not(feature = "egprof"))]
    { let _ = reset; let _ = EGP_NAMES; Vec::new() }
}

fn cell_seed(seed: u64, w: usize) -> u64 {
    seed.wrapping_add((w as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

/// one search cell's chance tree (see Battle.search_leaves_chance): best-first over outcome paths, the leaf positions
/// with their probabilities
fn enumerate_cell(view: &Engine, dex: &EngineDex, o: [Action; 2], t: [Action; 2], s: u64, min_path: f64,
                  max_leaves: usize, stop_repl: bool) -> Vec<(Engine, f32)> {
    enumerate_cell_tail(view, dex, o, t, s, min_path, max_leaves, stop_repl, 0.0).0
}

/// (2026-11-01, opts.tail) enumerate_cell that stops once the outcomes not generated yet weigh <= stop_mass (generation is
/// most likely first); returns the leaves and that left-over mass (0 when it ran to the end or hit max_leaves)
#[allow(clippy::too_many_arguments)]
fn enumerate_cell_tail(view: &Engine, dex: &EngineDex, o: [Action; 2], t: [Action; 2], s: u64, min_path: f64,
                       max_leaves: usize, stop_repl: bool, stop_mass: f64) -> (Vec<(Engine, f32)>, f64) {
    let _egp = egp(EGP_ENUM_OTHER);
    let mut heap: Vec<(f64, Vec<u8>)> = vec![(1.0, Vec::new())];
    let mut out = Vec::new();
    let mut rest = 0.0;
    while !heap.is_empty() && out.len() < max_leaves.max(1) {
        if stop_mass > 0.0 && !out.is_empty() {
            let m: f64 = heap.iter().map(|x| x.0).sum();
            if m <= stop_mass {
                rest = m;
                break;
            }
        }
        let bi = (0..heap.len()).max_by(|&x, &y| heap[x].0.partial_cmp(&heap[y].0).unwrap()).unwrap();
        let (_, script) = heap.swap_remove(bi);
        let mut c = { let _egp = egp(EGP_ENUM_CLONE); view.clone() };
        c.rng = Rng::new(s);
        let plen = script.len();
        c.chance = Some(Box::new(::vgcsim::battle::ChanceCtl { script, pos: 0, taken: Vec::new(), path: 1.0, min_path, stop_repl }));
        let mut r2 = Rng::new(s ^ 0x5555);
        {
            let _egp = egp(EGP_PLAY_TURN);
            policy::play_turn(&mut c, dex, o, t, &mut r2);
        }
        let ctl = c.chance.take().unwrap();
        let mut pre = 1.0f64;
        for (j, &(oc, pt)) in ctl.taken.iter().enumerate() {
            let pt = pt as f64;
            let (p_here, p_alt) = if oc == 1 { (pt, 1.0 - pt) } else { (1.0 - pt, pt) };
            if j >= plen {
                let mut sc: Vec<u8> = ctl.taken[..j].iter().map(|x| x.0).collect();
                sc.push(1 - oc);
                heap.push((pre * p_alt, sc));
            }
            pre *= p_here;
        }
        out.push((c, ctl.path as f32));
    }
    (out, rest)
}

/// (2026-11-01, opts.nodice) a cell's chance outcomes for the solver; nodice = the one "average luck" outcome (every chance
/// event takes its more likely result: likely hits land, no crits, middle or likely-KO damage rolls, repeated Protect fails),
/// weight 1. NOT a certificate of the real game: an estimate (speed test)
#[allow(clippy::too_many_arguments)]
fn eg_enum(nodice: bool, view: &Engine, dex: &EngineDex, o: [Action; 2], t: [Action; 2], s: u64, max_leaves: usize,
           stop_mass: f64) -> (Vec<(Engine, f32)>, f64) {
    if nodice && EG_LEANENUM.load(std::sync::atomic::Ordering::Relaxed) {
        // the first outcome enumerate_cell_tail would produce (empty script), without the heap of alternatives
        let _egp = egp(EGP_ENUM_OTHER);
        let mut c = { let _egp = egp(EGP_ENUM_CLONE); view.clone() };
        c.rng = Rng::new(s);
        c.chance = Some(Box::new(::vgcsim::battle::ChanceCtl { script: Vec::new(), pos: 0, taken: Vec::new(), path: 1.0, min_path: 0.0, stop_repl: false }));
        let mut r2 = Rng::new(s ^ 0x5555);
        {
            let _egp = egp(EGP_PLAY_TURN);
            policy::play_turn(&mut c, dex, o, t, &mut r2);
        }
        c.chance = None;
        return (vec![(c, 1.0)], 0.0);
    }
    if nodice {
        let (mut lv, _) = enumerate_cell_tail(view, dex, o, t, s, 0.0, 1, false, 0.0);
        if let Some(x) = lv.first_mut() {
            x.1 = 1.0;
        }
        return (lv, 0.0);
    }
    enumerate_cell_tail(view, dex, o, t, s, 0.0, max_leaves, false, stop_mass)
}

/// (2026-10-29, endgame rescoring) eg_rescore.exact_value in Rust, GIL-free: tiny endgames solved with NO network over
/// every legal joint action of both sides (uniform top_joint), every chance outcome (enumerate_cell, min_path 0) and every
/// replacement pair; lines undecided at the depth limit count -1 / +1 for a lower / upper bound, iterative deepening
/// 1..max_depth turns until upper - lower <= 2 tol (value = the midpoint). Aborts (None): node budget, a cell with
/// >= max_leaves outcomes, outcome probabilities not summing to 1, or an unexpected phase.
struct EgCtx<'a> {
    dex: &'a EngineDex,
    budget: usize,
    max_leaves: usize,
    iters: usize,
    seed: u64,
    // (2026-11-01, idea 1) tables + node counter shared by the root's worker threads (one EgShared per solve)
    sh: Arc<EgShared>,
    report: bool,
    // (2026-11-01) wall-clock stop for live solves (exact_root seconds > 0): a live solve had run 43.9 s past its 20 s deadline
    end: Option<std::time::Instant>,
    // (2026-11-01, certification widening; vgcsim.eg_solver_opts) double-oracle turn nodes, transposition table, soft leaf cap
    opts: EgOpts,
    stats: [u64; 5], // turn nodes expanded, TT hits, cells evaluated, cells possible, re-searches skipped (opts.skip)
    // (2026-11-01) chance reach of the node being evaluated (product of outcome probabilities from the root)
    reach: f64,
    // (2026-11-01, opts.thresh) window for the NEXT call (taken at entry, reset to infinite) and whether the call's
    // result was cut short by its window (valid but looser bounds)
    win: (f64, f64),
    cut: bool,
    // (2026-11-01, opts.tail) chance reach left unsearched within this root cell (missing mass counted at -1 / +1)
    tail_used: f64,
    // (2026-11-01, opts.race_ext) inside a 1v1 depth extension (no second extension on the same line)
    ext_on: bool,
    // (2026-11-01, opts.sub1) 1v1 sub-solves allowed (the root itself is bigger than 1v1)
    sub_ok: bool,
    // (opts.canon) search horizon in turns (timer cap), (opts.loop_n) strict re-pass
    hz: usize,
    strict: bool,
    // keys stored in the transposition table during the current strict pass (same-depth entries reusable)
    strict_done: std::collections::HashSet<u128>,
    // (opts.order2) per side: joint action -> history score (became a best response / support in this solve)
    hist: [std::collections::HashMap<u32, f64>; 2],
}

/// (2026-11-01, idea 1) a hash map split into 64 locked shards (keys are hashes: low bits pick the shard)
struct EgShard<V> {
    s: Vec<std::sync::Mutex<std::collections::HashMap<u128, V>>>,
}

impl<V: Clone> EgShard<V> {
    fn new() -> Self {
        EgShard { s: (0..64).map(|_| std::sync::Mutex::new(std::collections::HashMap::new())).collect() }
    }
    fn get(&self, k: u128) -> Option<V> {
        let _egp = egp(EGP_TABLE);
        self.s[(k as usize) & 63].lock().unwrap().get(&k).cloned()
    }
    fn insert(&self, k: u128, v: V) {
        let _egp = egp(EGP_TABLE);
        self.s[(k as usize) & 63].lock().unwrap().insert(k, v);
    }
}

/// (2026-11-01) double-oracle supports of a turn node from the last (shallower) iteration, plus (opts.order) a score per
/// row (our utility vs their mix, unevaluated cells at +1) and per column (unevaluated at -1): the next iteration starts
/// from the supports and tries candidate best responses best-first
#[derive(Clone)]
struct EgSup {
    so: Vec<usize>,
    st: Vec<usize>,
    rs: Vec<f64>,
    cs: Vec<f64>,
}

struct EgShared {
    tt: EgShard<(f64, f64, usize)>,
    sup: EgShard<EgSup>,
    // equivalent-cell merging: (fingerprint of the cell's chance leaves mixed with the depth left) -> bounds
    cellc: EgShard<(f64, f64)>,
    // (opts.equiv / opts.prune) kept rows, kept columns, depth left when computed (usize::MAX: equivalence only)
    red: EgShard<(Vec<usize>, Vec<usize>, usize)>,
    // (opts.hpmono) HP-free position key -> stored (HP vector, lower, upper) entries
    hpm: EgShard<Vec<EgHpEnt>>,
    // (opts.sub1) 1v1 sub-solve results: position key -> (lower, upper, depth reached)
    sub1: EgShard<(f64, f64, usize)>,
    nodes: std::sync::atomic::AtomicUsize,
    stop: std::sync::atomic::AtomicBool,
}

impl EgShared {
    fn new() -> Self {
        EgShared { tt: EgShard::new(), sup: EgShard::new(), cellc: EgShard::new(), red: EgShard::new(),
                   hpm: EgShard::new(), sub1: EgShard::new(),
                   nodes: std::sync::atomic::AtomicUsize::new(0), stop: std::sync::atomic::AtomicBool::new(false) }
    }
}

/// (opts.hpmono) bounds stored for one HP vector of an otherwise identical position
#[derive(Clone)]
struct EgHpEnt {
    hp: [u16; 12],
    l: f64,
    h: f64,
}

const EG_HP_TAG: u128 = 0x4850_4D4F_4E4F_0000_7A11_C0DE_0000_0001u128;
const EG_HP_MAX_ENT: usize = 32;

/// (opts.hpmono) true when nothing on the field reacts to HP levels in a way that could make more HP worse (pinch berries
/// and other pinch effects, HP-scaled moves, abilities that trigger on crossing an HP threshold)
fn eg_hp_ok(c: &Engine, dex: &EngineDex) -> bool {
    const ITEMS: &[&str] = &["sitrusberry", "oranberry", "figyberry", "wikiberry", "magoberry", "aguavberry", "iapapaberry",
                             "liechiberry", "ganlonberry", "salacberry", "petayaberry", "apicotberry", "lansatberry",
                             "starfberry", "micleberry", "custapberry", "enigmaberry", "berryjuice"];
    const ABS: &[&str] = &["blaze", "torrent", "overgrow", "swarm", "berserk", "angershell", "emergencyexit", "wimpout",
                           "shieldsdown", "schooling", "zenmode", "powerconstruct"];
    const MOVES: &[&str] = &["flail", "reversal", "endeavor", "painsplit", "eruption", "waterspout", "dragonenergy",
                             "crushgrip", "wringout", "hardpress", "finalgambit"];
    for side in c.sides.iter() {
        for m in side.mons.iter().filter(|m| m.alive()) {
            if m.item_id != u16::MAX {
                if let Some(it) = dex.items.get(m.item_id as usize) {
                    if ITEMS.contains(&it.id.as_str()) {
                        return false;
                    }
                }
            }
            for a in [m.ability_id, m.base_ability_id] {
                if let Some(ab) = dex.abilities.get(a as usize) {
                    if ABS.contains(&ab.id.as_str()) {
                        return false;
                    }
                }
            }
            for &mv in m.moves.iter().take(m.n_moves as usize) {
                if let Some(md) = dex.moves.get(mv as usize) {
                    if MOVES.contains(&md.id.as_str()) {
                        return false;
                    }
                }
            }
        }
    }
    true
}

/// (opts.hpmono) key of the position with every living Pokemon at full HP, and the real HP vector
fn eg_hp_key(c: &Engine, dex: &EngineDex) -> (u128, [u16; 12]) {
    let mut hv = [0u16; 12];
    let mut f = c.clone();
    for s in 0..2 {
        for (i, m) in f.sides[s].mons.iter_mut().enumerate().take(6) {
            hv[s * 6 + i] = m.hp;
            if m.alive() {
                m.hp = m.maxhp;
            }
        }
    }
    (eg_key(&f, dex) ^ EG_HP_TAG, hv)
}

/// (opts.hpmono) side 0's result can't fall when side 0's HP rises or side 1's falls: an entry with <= our HP and >= their
/// HP everywhere gives a lower bound, one with >= ours and <= theirs an upper bound
fn eg_hp_bounds(cx: &EgCtx, hk: u128, hv: &[u16; 12]) -> (f64, f64) {
    let (mut l, mut h) = (-1.0f64, 1.0f64);
    if let Some(es) = cx.sh.hpm.get(hk) {
        for e in es.iter() {
            let (mut le, mut ge) = (true, true);
            for i in 0..12 {
                let (a, b) = (e.hp[i], hv[i]);
                if i < 6 {
                    le &= a <= b;
                    ge &= a >= b;
                } else {
                    le &= a >= b;
                    ge &= a <= b;
                }
            }
            if le {
                l = l.max(e.l);
            }
            if ge {
                h = h.min(e.h);
            }
        }
    }
    if l > h {
        (h, h)
    } else {
        (l, h)
    }
}

fn eg_hp_store(cx: &EgCtx, hk: u128, hv: [u16; 12], l: f64, h: f64) {
    let mut es = cx.sh.hpm.get(hk).unwrap_or_default();
    if let Some(e) = es.iter_mut().find(|e| e.hp == hv) {
        if l.max(e.l) <= h.min(e.h) {
            e.l = l.max(e.l);
            e.h = h.min(e.h);
        } else {
            e.l = l;
            e.h = h;
        }
    } else {
        if es.len() >= EG_HP_MAX_ENT {
            // drop the widest entry
            let w = es.iter().enumerate().max_by(|a, b| (a.1.h - a.1.l).partial_cmp(&(b.1.h - b.1.l)).unwrap()).map(|x| x.0).unwrap();
            es.swap_remove(w);
        }
        es.push(EgHpEnt { hp: hv, l, h });
    }
    cx.sh.hpm.insert(hk, es);
}

/// (opts.race_ext / sub1 / hpmono) a turn leaf at the depth limit whose value depends on the position itself (extension,
/// sub-solve or HP table): it needs its full key in leaf merging and the cell cache (before 2026-11-01 race_ext + cellc
/// identified such leaves by their proven verdict only, so different positions could share a cached cell)
fn eg_limit_needs_key(cx: &EgCtx, leaf: &Engine) -> bool {
    cx.opts.hpmono || cx.opts.fix1 > 0 || ((cx.opts.race_ext > 0 || cx.opts.sub1 > 0) && eg_leaf_qualifies(leaf, 1))
}

/// (opts.sub1) a 1v1 inside a bigger endgame solved on its own (iterative deepening to sub1_depth turns, at most sub1
/// nodes), cached; returns (lower, upper, depth reached)
fn eg_sub1(cx: &mut EgCtx, c: &Engine, key: u128) -> Result<(f64, f64, usize), &'static str> {
    if let Some(r) = cx.sh.sub1.get(key) {
        return Ok(r);
    }
    let (b0, ext0, win0, reach0) = (cx.budget, cx.ext_on, cx.win, cx.reach);
    let start = cx.nodes();
    cx.budget = b0.min(start.saturating_add(cx.opts.sub1));
    cx.ext_on = true;
    let (mut l, mut h, mut reached) = (-1.0f64, 1.0f64, 0usize);
    let mut fail: Option<&'static str> = None;
    for dd in 1..=cx.opts.sub1_depth.max(1) {
        cx.win = EG_NOWIN;
        match eg_val_k(cx, c, 0, dd, Some(key)) {
            Ok((a, b)) => {
                if a.max(l) <= b.min(h) {
                    l = a.max(l);
                    h = b.min(h);
                }
                reached = dd;
                if h - l < 1e-9 {
                    break;
                }
            }
            Err(e) => {
                if !(e == "budget" && cx.nodes() <= b0) {
                    fail = Some(e);
                }
                break;
            }
        }
    }
    cx.budget = b0;
    cx.ext_on = ext0;
    cx.win = win0;
    cx.reach = reach0;
    cx.cut = false;
    if let Some(e) = fail {
        return Err(e);
    }
    cx.sh.sub1.insert(key, (l, h, reached));
    Ok((l, h, reached))
}

impl EgCtx<'_> {
    fn nodes(&self) -> usize {
        self.sh.nodes.load(std::sync::atomic::Ordering::Relaxed)
    }
}

fn eg_cellc_key(f: u128, left: usize) -> u128 {
    f ^ (left as u128 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15_F39C_C060_5CED_C835u128)
}

const EG_SWITCH_TAG: u128 = 0x5157_1C4E_D00D_F00D_A11C_E5EE_DBAD_C0DEu128;

#[derive(Clone, Copy)]
struct EgOpts {
    dbl: bool,
    tt: bool,
    soft_cap: bool,
    eps: f64,
    dead: bool,
    smab: bool,
    early: bool,
    cellc: bool,
    skip: f64,
    par: usize,
    merge: bool,
    order: bool,
    race: bool,
    equiv: bool,
    prune: bool,
    race2: bool,
    thresh: bool,
    tail: f64,
    tail_max: f64,
    persist: bool,
    race_ext: usize,
    bail: f64,
    hpmono: bool,
    sub1: usize,
    sub1_depth: usize,
    canon: bool,
    mirror: bool,
    loop_n: usize,
    relax: bool,
    fix1: usize,
    fix_min: f64,
    nodice: bool,
    fastkey: bool,
    rmfast: f64,
    order2: bool,
    leanenum: bool,
}

static EG_OPTS: std::sync::RwLock<EgOpts> = std::sync::RwLock::new(EgOpts { dbl: false, tt: false, soft_cap: false, eps: 1e-3, dead: false,
                                                                                  smab: false, early: false, cellc: false, skip: 0.0,
                                                                                  par: 1, merge: false, order: false, race: false,
                                                                                  equiv: false, prune: false, race2: false, thresh: false,
                                                                                  tail: 0.0, tail_max: 0.0, persist: false, race_ext: 0, bail: 0.0,
                                                                                  hpmono: false, sub1: 0, sub1_depth: 8,
                                                                                  canon: false, mirror: false, loop_n: 0, relax: false, fix1: 0, fix_min: 0.0,
                                                                                  nodice: false, fastkey: false, rmfast: 0.0001, order2: true, leanenum: true });
// (2026-11-02, speed items 1 / 2 / 5) read by free functions (eg_key, the solver's game solve, eg_enum)
static EG_FASTKEY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static EG_RMFAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0.0001f64.to_bits());
static EG_LEANENUM: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
const EG_FASTKEY_TAG: u128 = 0x3C3C_0000_0000_0000_0000_0000_0000_0011;
static EG_LAST_STATS: std::sync::Mutex<[u64; 5]> = std::sync::Mutex::new([0; 5]);

fn eg_ctx<'a>(dex: &'a EngineDex, budget: usize, max_leaves: usize, iters: usize, seed: u64,
              end: Option<std::time::Instant>) -> EgCtx<'a> {
    let opts = *EG_OPTS.read().unwrap();
    let sh = if opts.persist { eg_persist_shared() } else { Arc::new(EgShared::new()) };
    EgCtx { dex, budget, max_leaves, iters, seed, sh, report: true, end, opts,
            stats: [0; 5], reach: 1.0, win: EG_NOWIN, cut: false, tail_used: 0.0, ext_on: false, sub_ok: false, hz: 0, strict: false,
            strict_done: std::collections::HashSet::new(), hist: [std::collections::HashMap::new(), std::collections::HashMap::new()] }
}

/// (2026-11-01, opts.persist) one set of tables kept across solves in this process (stored bounds are valid for any root:
/// keys are full positions plus the depth left); the node counter and stop flag are reset per solve; a solve that starts
/// while another one holds the tables (parallel worlds) gets fresh tables; cleared past EG_PERSIST_MAX transposition-table
/// entries or by eg_tt_reset()
static EG_PERSIST: std::sync::Mutex<Option<Arc<EgShared>>> = std::sync::Mutex::new(None);
const EG_PERSIST_MAX: usize = 3_000_000;

fn eg_tt_len(sh: &EgShared) -> usize {
    sh.tt.s.iter().map(|m| m.lock().unwrap().len()).sum()
}

fn eg_persist_shared() -> Arc<EgShared> {
    use std::sync::atomic::Ordering::Relaxed;
    let mut g = EG_PERSIST.lock().unwrap();
    if let Some(sh) = g.as_ref() {
        if Arc::strong_count(sh) > 1 {
            return Arc::new(EgShared::new());
        }
    }
    if g.as_ref().map(|sh| eg_tt_len(sh) > EG_PERSIST_MAX).unwrap_or(true) {
        *g = Some(Arc::new(EgShared::new()));
    }
    let sh = g.as_ref().unwrap().clone();
    sh.nodes.store(0, Relaxed);
    sh.stop.store(false, Relaxed);
    sh
}

/// (2026-11-01) clear the persistent endgame tables (opts.persist); returns the transposition-table size before
#[pyfunction]
fn eg_tt_reset() -> usize {
    let mut g = EG_PERSIST.lock().unwrap();
    let n = g.as_ref().map(|sh| eg_tt_len(sh)).unwrap_or(0);
    *g = None;
    n
}

const EG_NOWIN: (f64, f64) = (f64::NEG_INFINITY, f64::INFINITY);

/// (2026-11-01) eg_solver_opts(double_oracle=False, tt=False, soft_cap=False, eps=1e-3): exact endgame solver options for
/// this process. double_oracle = turn nodes evaluate only (our support x all theirs) + (all ours x their support), adding
/// best responses until none improves the lo or hi game by more than eps (same approximate-equilibrium semantics as the
/// full matrix); tt = transposition table keyed on eg_key (both sides' encodings), stored bounds reused when computed
/// with at least as much depth left; soft_cap = a cell with >= max_leaves outcomes counts its missing mass as [-1, 1]
/// instead of aborting (also at the root); dead_end = a 1v1 that Battle::dead_end proves won counts as exact (1 / -1);
/// smab = (with double_oracle) cells are evaluated lazily: a candidate best response stops being evaluated as soon as
/// its bound (unevaluated cells counted at their worst / best) proves it can't improve the restricted game (Saffidine's
/// simultaneous-move alpha-beta idea in double-oracle form; same values); early = proven-outcome stops: a node where one
/// of our joint actions wins against every reply (or one of theirs beats every action of ours) returns +-1 at once;
/// cell_cache = cells whose chance leaves are the same positions with the same probabilities (equivalent actions: a
/// failing status move, a target that doesn't matter...) are evaluated once per depth; skip = (with tt) a turn node that a
/// shallower iteration already bounded is NOT searched again when chance reach x its old gap < skip (its old bounds are
/// still valid bounds: deeper iterations spend the work on the likely, still-open lines).
/// (2026-11-01, speed ideas 1 / 2 / 3 / 8) par = threads for the root's cells (exact_root; tables and the node budget are
/// shared); merge = chance leaves that are the same position are evaluated once (probabilities summed) and replacement
/// nodes go into the transposition table too; order = candidate best responses tried best-first (row / column scores kept
/// from the last iteration and updated within the node) and root cells already exact are not searched again; race = a
/// position where one side can never hurt the other again and the other surely KOs everything left (up to 2 Pokemon a
/// side, Battle::race_win) counts as decided.
/// (2026-11-01, speed ideas 7 / 13 / 8b / 9) equiv = joint actions whose cells are the same chance outcomes against every
/// reply are dropped (one kept); prune = iterated removal of joint actions dominated in certified bounds (a row whose upper
/// bounds are <= another row's lower bounds on every column; columns mirrored); race2 = race checks also accept Grassy
/// Terrain, damage-cutting abilities, Sturdy, Quick Claw / Air Balloon / Eviolite, paralysed attackers...; thresh = the
/// lazy double oracle passes a window to each candidate's cell (the value that already decides the candidate) and a node
/// stops as soon as its cheap bounds leave its window.
/// (2026-11-01, five ideas) tail = below the root a cell's chance outcomes come most likely first and generation stops once
/// the outcomes left weigh <= tail / chance reach, while the unsearched reach within one root cell stays <= tail_max (left
/// mass counted at -1 / +1: still exact bounds); persist = tables kept across solves (eg_tt_reset clears them); race_ext =
/// a 1v1 turn node at the depth limit is searched race_ext turns deeper (once per line); bail = a root with >= 2 Pokemon
/// on both sides gets at most `bail` seconds.
/// (2026-11-01, ideas 6 / 7) hpmono = more HP is never worse (only when no pinch berry / HP-scaled move / HP-threshold
/// ability is on the field): bounds stored for one HP vector bound every position that differs only in HP (looked up at
/// turn nodes and at the depth limit); sub1 = a 1v1 inside a bigger endgame gets its own iterative-deepening solve
/// (at most sub1 nodes, sub1_depth turns), cached and used as bounds (also at the depth limit). Returns the previous
/// options as text.
/// (2026-11-01, ideas 12+5 / 25 / 6 / 7) canon = transposition keys ignore the turn counter and network-only history and
/// cap field / side timers at the search horizon + 2 (not with proven-result checks); mirror = a side whose only Pokemon
/// stands in slot b is keyed as if in slot a; loop_n = after an unsolved depth, up to loop_n strict re-passes at the same
/// depth (no reuse of same-depth table entries or cell-cache results; bounds intersected) while the gap shrinks; relax = in
/// a cell, chance leaves that differ only in HP are bounded by the componentwise worst / best member (members whose
/// bound is narrow enough are not searched); fix1 = a turn node at the depth limit gets the transposition table's bounds
/// and (chance reach >= fix_min) one more turn of pure-strategy security bounds (max-min / min-max over cells whose
/// leaves count as decided, proven or -1 / +1).
/// (2026-11-02, speed items 1 / 2 / 4 / 5; rmfast=0.0001, order2, leanenum ON by default since run 26, user) fastkey = transposition keys from Battle::state_hash (all state fields) instead
/// of hashing two network encodings; rmfast = the solver's matrix games: a pure saddle point is returned at once, else
/// regret matching stops when the averaged strategies' exploitability gap is <= rmfast (0 = fixed iterations);
/// order2 = double-oracle best responses tried in order of a per-solve history score (joint actions that became best
/// responses / supports elsewhere in the tree first; also seeds a new node's supports); leanenum = with nodice, the one
/// kept chance outcome is played without the outcome heap (same result).
#[pyfunction]
#[pyo3(signature = (double_oracle=false, tt=false, soft_cap=false, eps=1e-3, dead_end=false, smab=false, early=false, cell_cache=false, skip=0.0,
                    par=1, merge=false, order=false, race=false, equiv=false, prune=false, race2=false, thresh=false,
                    tail=0.0, tail_max=0.0, persist=false, race_ext=0, bail=0.0, hpmono=false, sub1=0, sub1_depth=8,
                    canon=false, mirror=false, loop_n=0, relax=false, fix1=0, fix_min=0.0, nodice=false,
                    fastkey=false, rmfast=0.0001, order2=true, leanenum=true))]
#[allow(clippy::too_many_arguments)]
fn eg_solver_opts(double_oracle: bool, tt: bool, soft_cap: bool, eps: f64, dead_end: bool, smab: bool, early: bool,
                  cell_cache: bool, skip: f64, par: usize, merge: bool, order: bool, race: bool, equiv: bool, prune: bool,
                  race2: bool, thresh: bool, tail: f64, tail_max: f64, persist: bool, race_ext: usize, bail: f64,
                  hpmono: bool, sub1: usize, sub1_depth: usize, canon: bool, mirror: bool, loop_n: usize, relax: bool,
                  fix1: usize, fix_min: f64, nodice: bool, fastkey: bool, rmfast: f64, order2: bool, leanenum: bool) -> String {
    let mut g = EG_OPTS.write().unwrap();
    let old = format!("dbl={} tt={} soft={} eps={} dead={} smab={} early={} cellc={} skip={} par={} merge={} order={} race={} equiv={} prune={} race2={} thresh={} tail={} tail_max={} persist={} race_ext={} bail={} hpmono={} sub1={} sub1_depth={} canon={} mirror={} loop_n={} relax={} fix1={} fix_min={} nodice={} fastkey={} rmfast={} order2={} leanenum={}",
                      g.dbl, g.tt, g.soft_cap, g.eps, g.dead, g.smab, g.early, g.cellc, g.skip, g.par, g.merge, g.order, g.race,
                      g.equiv, g.prune, g.race2, g.thresh, g.tail, g.tail_max, g.persist, g.race_ext, g.bail, g.hpmono, g.sub1, g.sub1_depth,
                      g.canon, g.mirror, g.loop_n, g.relax, g.fix1, g.fix_min, g.nodice, g.fastkey, g.rmfast, g.order2, g.leanenum);
    *g = EgOpts { dbl: double_oracle, tt, soft_cap, eps, dead: dead_end, smab, early, cellc: cell_cache, skip, par, merge, order, race,
                  equiv, prune, race2, thresh, tail, tail_max, persist, race_ext, bail, hpmono, sub1, sub1_depth,
                  canon, mirror, loop_n, relax, fix1, fix_min, nodice, fastkey, rmfast, order2, leanenum };
    use std::sync::atomic::Ordering::Relaxed;
    EG_FASTKEY.store(fastkey, Relaxed);
    EG_RMFAST.store(rmfast.max(0.0).to_bits(), Relaxed);
    EG_LEANENUM.store(leanenum, Relaxed);
    old
}

/// (2026-11-01) the last finished solve's counters in this process: (turn nodes expanded, TT hits, cells evaluated,
/// cells a full-width solver would evaluate at those nodes)
#[pyfunction]
fn eg_solver_stats() -> (u64, u64, u64, u64, u64) {
    let g = EG_LAST_STATS.lock().unwrap();
    (g[0], g[1], g[2], g[3], g[4])
}

impl Drop for EgCtx<'_> {
    fn drop(&mut self) {
        if !self.report {
            return;
        }
        if let Ok(mut g) = EG_LAST_STATS.lock() {
            *g = self.stats;
        }
    }
}

fn eg_spend(cx: &mut EgCtx, n: usize) -> Result<(), &'static str> {
    use std::sync::atomic::Ordering::Relaxed;
    let t = cx.sh.nodes.fetch_add(n, Relaxed) + n;
    if t > cx.budget { return Err("budget"); }
    if let Some(e) = cx.end { if std::time::Instant::now() > e { return Err("time"); } }
    if cx.sh.stop.load(Relaxed) { return Err("stop"); }
    Ok(())
}

/// (2026-11-01, opts.race / opts.dead) a proven result without search: +-1, or None
fn eg_proven(cx: &EgCtx, c: &Engine) -> Option<f64> {
    let _egp = egp(EGP_PROVEN);
    let w = if cx.opts.race || cx.opts.race2 { c.race_win_opt(cx.dex, 2, cx.opts.race2).or_else(|| c.dead_end(cx.dex)) } else if cx.opts.dead { c.dead_end(cx.dex) } else { None };
    w.map(|w| if w == 0 { 1.0 } else { -1.0 })
}

/// (opts.canon) search horizon of a solve in turns (timers longer than this + 2 can't run out inside the search)
fn eg_hz(o: &EgOpts, max_depth: usize) -> usize {
    max_depth.max(1) + o.race_ext + if o.sub1 > 0 { o.sub1_depth } else { 0 } + if o.fix1 > 0 { 1 } else { 0 }
}

/// (opts.mirror) slot-mirror keys only where nothing remembers slot numbers beyond what eg_skey remaps
fn eg_mirror_ok(c: &Engine, dex: &EngineDex) -> bool {
    if c.phase != Phase::Turn {
        return false;
    }
    for side in c.sides.iter() {
        if side.baton.is_some() {
            return false;
        }
        for m in side.mons.iter() {
            if m.illusion || m.charge_slot != NONE || m.leech_slot != NONE || m.retal_slot.iter().any(|&x| x != NONE) {
                return false;
            }
            for &mv in m.moves.iter().take(m.n_moves as usize) {
                if let Some(md) = dex.moves.get(mv as usize) {
                    if md.kind == ::vgcsim::effects::Mv::Instruct || md.kind == ::vgcsim::effects::Mv::BeatUp {
                        return false;
                    }
                }
            }
        }
    }
    true
}

/// (opts.mirror) per side: its only living active Pokemon stands in slot b
fn eg_mirror_sides(c: &Engine) -> [bool; 2] {
    let mut out = [false; 2];
    for (s, o) in out.iter_mut().enumerate() {
        let sd = &c.sides[s];
        let alive = |a: u8| a != NONE && sd.mons.get(a as usize).map(|m| m.alive()).unwrap_or(false);
        *o = alive(sd.active[1]) && !alive(sd.active[0]);
    }
    out
}

fn eg_cap_u8(x: &mut u8, cap: u8) {
    if *x > cap {
        *x = cap;
    }
}

/// (opts.canon / opts.mirror) transposition key: eg_key of a canonical copy (canon: turn counter and network-only history
/// cleared, timers capped at the horizon + 2 unless proven-result checks run; mirror: a side whose only Pokemon stands in
/// slot b is moved to slot a, slot references remapped), tagged with the horizon (tables can outlive a solve: persist)
fn eg_skey_m(cx: &EgCtx, c: &Engine, mirror: bool) -> u128 {
    let _egp = egp(EGP_SKEY_CANON);
    let dex = cx.dex;
    let sw = if mirror && eg_mirror_ok(c, dex) { eg_mirror_sides(c) } else { [false; 2] };
    if !cx.opts.canon && !sw[0] && !sw[1] {
        return eg_key(c, dex);
    }
    let mut f = c.clone();
    if cx.opts.canon {
        f.turn = 0;
        for sd in f.sides.iter_mut() {
            for m in sd.mons.iter_mut() {
                m.h_slot = NONE;
                m.h_target = 0;
                m.h_streak = 0;
                m.h_protect = false;
                m.h_uses = [0; 4];
                m.h_protects = 0;
                m.h_status = 0;
            }
        }
        if cx.hz > 0 && !(cx.opts.dead || cx.opts.race || cx.opts.race2) {
            let cap = (cx.hz + 2).min(250) as u8;
            let fl = &mut f.field;
            for x in [&mut fl.weather_turns, &mut fl.terrain_turns, &mut fl.trick_room, &mut fl.gravity, &mut fl.magic_room,
                      &mut fl.wonder_room, &mut fl.fairy_lock] {
                eg_cap_u8(x, cap);
            }
            for sd in f.sides.iter_mut() {
                for x in [&mut sd.tailwind, &mut sd.reflect, &mut sd.light_screen, &mut sd.aurora_veil, &mut sd.safeguard] {
                    eg_cap_u8(x, cap);
                }
                for w in sd.wish.iter_mut() {
                    eg_cap_u8(&mut w.0, cap);
                }
                for fu in sd.future.iter_mut() {
                    eg_cap_u8(&mut fu.0, cap);
                }
            }
        }
    }
    let canon = cx.opts.canon;
    for s in 0..2 {
        if !sw[s] {
            continue;
        }
        {
            let sd = &mut f.sides[s];
            sd.active.swap(0, 1);
            sd.healing_wish.swap(0, 1);
            sd.shed_sub.swap(0, 1);
            sd.wish.swap(0, 1);
            sd.future.swap(0, 1);
            sd.party.swap(0, 1);
            for m in sd.mons.iter_mut() {
                m.last_target = match m.last_target { -1 => -2, -2 => -1, x => x };
            }
        }
        for m in f.sides[1 - s].mons.iter_mut() {
            m.last_target = match m.last_target { 1 => 2, 2 => 1, x => x };
            if !canon {
                m.h_target = match m.h_target { 1 => 2, 2 => 1, x => x };
            }
        }
    }
    eg_key(&f, dex) ^ (0x5A5Au128 << 112) ^ ((cx.hz as u128) << 8) ^ (((sw[0] as u128) | ((sw[1] as u128) << 1)) << 4)
}

fn eg_skey(cx: &EgCtx, c: &Engine) -> u128 {
    if !cx.opts.canon && !cx.opts.mirror {
        return eg_key(c, cx.dex);
    }
    eg_skey_m(cx, c, cx.opts.mirror)
}

/// (opts.mirror) key for caches that store action indices (double-oracle supports, reductions): no slot mirroring
fn eg_akey(cx: &EgCtx, c: &Engine, skey: u128) -> u128 {
    if cx.opts.mirror && eg_mirror_ok(c, cx.dex) && eg_mirror_sides(c).iter().any(|&x| x) {
        eg_skey_m(cx, c, false)
    } else {
        skey
    }
}

/// (opts.fix1) one turn of pure-strategy security bounds at a depth-limit node: each cell's chance leaves count as their
/// winner's value, a proven result, or -1 / +1; lower = max over our actions of the min over theirs, upper mirrored
fn eg_fix1(cx: &mut EgCtx, c: &Engine) -> Result<(f64, f64), &'static str> {
    let dex = cx.dex;
    let ours = eg_joints(c, dex, 0);
    let theirs = eg_joints(c, dex, 1);
    let (n, k) = (ours.len(), theirs.len());
    if n * k > cx.opts.fix1 * 256 || n == 0 || k == 0 {
        return Ok((-1.0, 1.0));
    }
    let view = { let _egp = egp(EGP_SEARCH_COPY); c.search_copy(false) };
    let s = cell_seed(cx.seed, 0);
    let mut lo = vec![0.0; n * k];
    let mut hi = vec![0.0; n * k];
    for (i, o) in ours.iter().enumerate() {
        for (j, t) in theirs.iter().enumerate() {
            let lv = eg_enum(cx.opts.nodice, &view, dex, *o, *t, s, cx.max_leaves, 0.0).0;
            eg_spend(cx, lv.len().max(1))?;
            let capped = lv.len() >= cx.max_leaves;
            let tot: f64 = lv.iter().map(|x| x.1 as f64).sum();
            if tot > 1.0 + 1e-3 || (!capped && (tot - 1.0).abs() > 1e-3) || tot <= 0.0 {
                return Ok((-1.0, 1.0));
            }
            let (scale, extra) = if capped { (1.0, (1.0 - tot).max(0.0)) } else { (1.0 / tot, 0.0) };
            let (mut l, mut h) = (-extra, extra);
            for (leaf, p) in lv.iter() {
                let q = *p as f64 * scale;
                let (x, y) = if let Some(w) = leaf.winner {
                    let v = if w == 0 { 1.0 } else if w == 1 { -1.0 } else { 0.0 };
                    (v, v)
                } else if leaf.phase == Phase::Turn {
                    eg_proven(cx, leaf).map(|v| (v, v)).unwrap_or((-1.0, 1.0))
                } else {
                    (-1.0, 1.0)
                };
                l += q * x;
                h += q * y;
            }
            lo[i * k + j] = l;
            hi[i * k + j] = h;
        }
    }
    let low = (0..n).map(|i| (0..k).map(|j| lo[i * k + j]).fold(f64::INFINITY, f64::min)).fold(f64::NEG_INFINITY, f64::max);
    let up = (0..k).map(|j| (0..n).map(|i| hi[i * k + j]).fold(f64::NEG_INFINITY, f64::max)).fold(f64::INFINITY, f64::min);
    Ok((low.max(-1.0), up.min(1.0)))
}

/// one leaf of a cell evaluated with no window (opts.relax pre-pass)
fn eg_leaf_eval(cx: &mut EgCtx, lv: &[(Engine, f32)], it: (usize, f64, Option<u128>), d: usize, depth: usize, r0: f64)
                -> Result<(f64, f64), &'static str> {
    cx.reach = r0 * it.1;
    cx.win = EG_NOWIN;
    cx.cut = false;
    let res = eg_val_k(cx, &lv[it.0].0, d + 1, depth, it.2);
    cx.reach = r0;
    cx.win = EG_NOWIN;
    cx.cut = false;
    res
}

/// (2026-11-01, opts.merge) identity of a chance leaf for merging: winner, or the position's eg_key (a replacement node
/// tagged), or for a turn at the depth limit (worth (-1, 1) unless proven) only the proven verdict
fn eg_leaf_id(cx: &EgCtx, leaf: &Engine, at_limit: bool) -> (u128, Option<u128>) {
    if let Some(w) = leaf.winner {
        return (0x7777_0000_0000_0000_0000_0000_0000_0001u128 + w as u128, None);
    }
    if leaf.phase == Phase::Turn {
        if at_limit && !eg_limit_needs_key(cx, leaf) {
            let v = eg_proven(cx, leaf).map(|v| if v > 0.0 { 1u128 } else { 2 }).unwrap_or(0);
            return (0x7777_0000_0000_0000_0000_0000_0000_0100u128 + v, None);
        }
        let k = eg_skey(cx, leaf);
        return (k, Some(k));
    }
    (eg_skey(cx, leaf) ^ EG_SWITCH_TAG, None)
}

/// every legal joint action of `side` in top_joint's order (uniform logits: no double switch to one Pokemon, no double
/// Mega; empty = slot 0's first action + pass)
fn eg_joints(b: &Engine, dex: &EngineDex, side: usize) -> Vec<[Action; 2]> {
    let _egp = egp(EGP_JOINTS);
    let la = policy::legal_actions(b, dex, side);
    let mut slots: Vec<Vec<Action>> = Vec::with_capacity(2);
    for user in 0..2 {
        let mut by_idx: [Option<Action>; 38] = [None; 38];
        for &a in la[user].iter() {
            by_idx[encode::act_index(a, user).min(37)] = Some(a);
        }
        let mut z: Vec<Action> = by_idx.iter().flatten().copied().collect();
        if z.is_empty() {
            z.push(Action::Pass);
        }
        slots.push(z);
    }
    let mut out = Vec::new();
    for &a0 in slots[0].iter() {
        for &a1 in slots[1].iter() {
            match (a0, a1) {
                (Action::Switch { to: x }, Action::Switch { to: y }) if x == y => continue,
                (Action::Move { mega: true, .. }, Action::Move { mega: true, .. }) => continue,
                _ => {}
            }
            out.push([a0, a1]);
        }
    }
    if out.is_empty() {
        out.push([slots[0][0], Action::Pass]);
    }
    out
}

fn eg_game_value(m: &[f64], n: usize, k: usize, iters: usize) -> f64 {
    if n == 1 && k == 1 {
        return m[0];
    }
    if n == 1 {
        return m.iter().copied().fold(f64::INFINITY, f64::min);
    }
    if k == 1 {
        return m.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    }
    let (x, y) = eg_rm(m, n, k, iters);
    let mut v = 0.0;
    for i in 0..n {
        for j in 0..k {
            v += x[i] * m[i * k + j] * y[j];
        }
    }
    v
}

fn eg_pair(cx: &EgCtx, lo: &[f64], hi: &[f64], n: usize, k: usize) -> (f64, f64) {
    (eg_game_value(lo, n, k, cx.iters), eg_game_value(hi, n, k, cx.iters))
}

/// (opts.merge) chance leaves grouped by identity: (representative leaf index, summed probability, its eg_key for turns)
fn eg_merge_leaves(cx: &EgCtx, lv: &[(Engine, f32)], at_limit: bool) -> (Vec<(usize, f64, Option<u128>)>, Vec<u128>) {
    let _egp = egp(EGP_MERGE_LEAVES);
    let mut out: Vec<(usize, f64, Option<u128>)> = Vec::with_capacity(lv.len());
    let mut ids: Vec<u128> = Vec::with_capacity(lv.len());
    let mut at: std::collections::HashMap<u128, usize> = std::collections::HashMap::new();
    for (i, (leaf, p)) in lv.iter().enumerate() {
        let (id, key) = eg_leaf_id(cx, leaf, at_limit);
        match at.get(&id) {
            Some(&g) => out[g].1 += *p as f64,
            None => {
                at.insert(id, out.len());
                out.push((i, *p as f64, key));
                ids.push(id);
            }
        }
    }
    (out, ids)
}

/// one (our joint x their joint) cell of a turn node: expectation over its chance outcomes of the children's bounds
fn eg_cell(cx: &mut EgCtx, view: &Engine, o: [Action; 2], t: [Action; 2], d: usize, depth: usize) -> Result<(f64, f64), &'static str> {
    let _egp = egp(EGP_CELL);
    let win = std::mem::replace(&mut cx.win, EG_NOWIN);
    cx.cut = false;
    let dex = cx.dex;
    cx.stats[2] += 1;
    let s = cell_seed(cx.seed, 0);
    // (opts.tail) stop generating outcomes once the rest is too unlikely to matter (counted at -1 / +1)
    let stop_mass = if cx.opts.tail > 0.0 && cx.reach > 0.0 {
        (cx.opts.tail.min((cx.opts.tail_max - cx.tail_used).max(0.0)) / cx.reach).min(0.25)
    } else {
        0.0
    };
    let (lv, rest) = eg_enum(cx.opts.nodice, view, dex, o, t, s, cx.max_leaves, stop_mass);
    let tailed = rest > 0.0;
    if tailed {
        cx.tail_used += cx.reach * rest;
    }
    let full = lv.len() >= cx.max_leaves;
    if full && !cx.opts.soft_cap {
        return Err("leaf cap");
    }
    let capped = full || tailed;
    let tot: f64 = lv.iter().map(|x| x.1 as f64).sum();
    if tot > 1.0 + 1e-3 || (!capped && (tot - 1.0).abs() > 1e-3) {
        return Err("probabilities");
    }
    eg_spend(cx, lv.len())?;
    if cx.opts.merge {
        return eg_cell_merged(cx, &lv, capped, tot, d, depth, win);
    }
    // (2026-11-01) equivalent cells: same leaf positions with the same probabilities = same bounds at this depth
    let keys: Vec<Option<u128>> = if cx.opts.cellc {
        // a Turn leaf at the depth limit is worth (-1, 1) whatever it is: no key needed (eg_key = two full encodes)
        let at_limit = d + 1 >= depth;
        lv.iter().map(|(leaf, _)| if leaf.winner.is_none() && leaf.phase == Phase::Turn && (!at_limit || eg_limit_needs_key(cx, leaf)) { Some(eg_skey(cx, leaf)) } else { None }).collect()
    } else {
        Vec::new()
    };
    let fp = if cx.opts.cellc {
        let mut items: Vec<(u128, u32)> = lv.iter().zip(keys.iter()).map(|((leaf, p), k)| {
            let kk = match k {
                Some(x) => *x,
                // decided / replacement leaves: winner + phase + both sides' HP (replacement children are rebuilt from it)
                None => {
                    // a limit leaf's value is (-1, 1) unless opts.dead proves it: the dead-end verdict joins the key
                    let dw = if (cx.opts.dead || cx.opts.race || cx.opts.race2) && leaf.winner.is_none() && leaf.phase == Phase::Turn {
                        eg_proven(cx, leaf).map(|v| if v > 0.0 { 1u128 } else { 2 }).unwrap_or(0)
                    } else {
                        0
                    };
                    let mut h = 0x9E37_79B9_7F4A_7C15u128 ^ ((leaf.winner.map(|w| w as u128 + 1).unwrap_or(0)) << 1) ^ (dw << 5);
                    h = h.wrapping_mul(0x0000_0100_0000_01B3).wrapping_add(leaf.phase as u128);
                    if leaf.winner.is_none() && leaf.phase != Phase::Turn {
                        h ^= eg_skey(cx, leaf).rotate_left(7);
                    }
                    h
                }
            };
            (kk, p.to_bits())
        }).collect();
        items.sort_unstable();
        let mut h = (capped as u128).wrapping_add(0x6C62_272E_07BB_0142_62B8_2175_6295_C58Du128);
        for (k, p) in items {
            h = (h ^ k).wrapping_mul(0x0000_0000_0100_0000_0000_0000_0000_013Bu128) ^ p as u128;
        }
        Some(h)
    } else {
        None
    };
    if let Some(f) = fp.filter(|_| !cx.strict) {
        if let Some(r) = cx.sh.cellc.get(eg_cellc_key(f, depth - d)) {
            return Ok(r);
        }
    }
    let items: Vec<(usize, f64, Option<u128>)> = lv.iter().enumerate().map(|(i, (_, p))| (i, *p as f64, keys.get(i).copied().flatten())).collect();
    let (r, cut) = eg_accum(cx, lv.as_slice(), &items, capped, tot, d, depth, win)?;
    if let Some(f) = fp.filter(|_| !cut) {
        cx.sh.cellc.insert(eg_cellc_key(f, depth - d), r);
    }
    cx.cut = cut;
    Ok(r)
}

/// expectation of a cell over its leaves `items` (leaf index, probability, eg_key): normalised by tot, or with the
/// missing mass of a capped cell at -1 / +1. (opts.thresh) with a window (a, b): each leaf gets the window that decides
/// the cell, and the cell stops (cut = true; bounds with the rest at -1 / +1) once its upper bound is <= a or its lower
/// bound >= b
#[allow(clippy::too_many_arguments)]
fn eg_accum(cx: &mut EgCtx, lv: &[(Engine, f32)], items: &[(usize, f64, Option<u128>)], capped: bool, tot: f64, d: usize,
            depth: usize, win: (f64, f64)) -> Result<((f64, f64), bool), &'static str> {
    let _egp = egp(EGP_ACCUM);
    let (scale, extra) = if capped { (1.0, (1.0 - tot).max(0.0)) } else { (1.0 / tot, 0.0) };
    let (a, b) = win;
    let use_win = a > f64::NEG_INFINITY || b < f64::INFINITY;
    let (mut l, mut h) = (-extra, extra);
    let mut rem: f64 = if use_win { items.iter().map(|x| x.1 * scale).sum() } else { 0.0 };
    let r0 = cx.reach;
    // (opts.relax) leaves differing only in HP: the componentwise worst / best member bound the rest
    let mut pre: Vec<Option<(f64, f64)>> = Vec::new();
    if cx.opts.relax && !use_win && d + 1 < depth && items.len() >= 3 {
        pre = vec![None; items.len()];
        let dex = cx.dex;
        let mut grp: std::collections::HashMap<u128, Vec<(usize, [u16; 12])>> = std::collections::HashMap::new();
        for (ii, it) in items.iter().enumerate() {
            let lf = &lv[it.0].0;
            if lf.winner.is_none() && lf.phase == Phase::Turn && eg_hp_ok(lf, dex) {
                let (hk, hv) = eg_hp_key(lf, dex);
                grp.entry(hk).or_default().push((ii, hv));
            }
        }
        let dom = |a: &[u16; 12], b: &[u16; 12]| (0..6).all(|x| a[x] <= b[x]) && (6..12).all(|x| a[x] >= b[x]);
        for g in grp.into_values() {
            if g.len() < 3 {
                continue;
            }
            let w = g.iter().find(|a| g.iter().all(|b| dom(&a.1, &b.1))).map(|x| x.0);
            let bs = g.iter().find(|a| g.iter().all(|b| dom(&b.1, &a.1))).map(|x| x.0);
            let (Some(w), Some(bs)) = (w, bs) else { continue };
            if w == bs {
                continue;
            }
            let wr = eg_leaf_eval(cx, lv, items[w], d, depth, r0)?;
            let br = eg_leaf_eval(cx, lv, items[bs], d, depth, r0)?;
            pre[w] = Some(wr);
            pre[bs] = Some(br);
            let (lo, hi) = (wr.0, br.1);
            if lo > hi + 1e-9 {
                continue;
            }
            for &(ii, _) in g.iter() {
                if ii == w || ii == bs {
                    continue;
                }
                let width = (hi - lo).max(0.0);
                if width < 1e-9 || (cx.opts.skip > 0.0 && r0 * items[ii].1 * width < cx.opts.skip) {
                    pre[ii] = Some((lo, hi.max(lo)));
                    cx.stats[4] += 1;
                }
            }
        }
    }
    for (ii, &(i, p, key)) in items.iter().enumerate() {
        let q = p * scale;
        if let Some((x, y)) = pre.get(ii).copied().flatten() {
            l += q * x;
            h += q * y;
            continue;
        }
        let mut lw = EG_NOWIN;
        if use_win {
            rem -= q;
            let rem0 = rem.max(0.0);
            if h + q + rem0 <= a || l - q - rem0 >= b {
                return Ok(((l - q - rem0, h + q + rem0), true));
            }
            if q > 1e-12 {
                lw = ((a - h - rem0) / q, (b - l + rem0) / q);
            }
        }
        cx.reach = r0 * p;
        cx.win = lw;
        cx.cut = false;
        let res = eg_val_k(cx, &lv[i].0, d + 1, depth, key);
        cx.reach = r0;
        cx.win = EG_NOWIN;
        let leaf_cut = std::mem::replace(&mut cx.cut, false);
        let (x, y) = res?;
        l += q * x;
        h += q * y;
        if leaf_cut {
            let rem0 = rem.max(0.0);
            return Ok(((l - rem0, h + rem0), true));
        }
    }
    Ok(((l, h), false))
}

/// (2026-11-01, opts.merge) eg_cell over merged leaves: each distinct position evaluated once; the cell-cache fingerprint
/// is the sorted (identity, probability) list of the merged groups
fn eg_cell_merged(cx: &mut EgCtx, lv: &[(Engine, f32)], capped: bool, tot: f64, d: usize, depth: usize,
                  win: (f64, f64)) -> Result<(f64, f64), &'static str> {
    let at_limit = d + 1 >= depth;
    let (groups, ids) = eg_merge_leaves(cx, lv, at_limit);
    let fp = if cx.opts.cellc {
        let mut items: Vec<(u128, u64)> = ids.iter().zip(groups.iter()).map(|(id, g)| (*id, g.1.to_bits())).collect();
        items.sort_unstable();
        let mut h = (capped as u128).wrapping_add(0x6C62_272E_07BB_0142_62B8_2175_6295_C58Du128);
        for (k, p) in items {
            h = (h ^ k).wrapping_mul(0x0000_0000_0100_0000_0000_0000_0000_013Bu128) ^ p as u128;
        }
        Some(eg_cellc_key(h, depth - d))
    } else {
        None
    };
    if let Some(f) = fp.filter(|_| !cx.strict) {
        if let Some(r) = cx.sh.cellc.get(f) {
            return Ok(r);
        }
    }
    let (r, cut) = eg_accum(cx, lv, &groups, capped, tot, d, depth, win)?;
    if let Some(f) = fp.filter(|_| !cut) {
        cx.sh.cellc.insert(f, r);
    }
    cx.cut = cut;
    Ok(r)
}

/// (2026-11-01, opts.equiv / opts.prune) joint actions to keep at a turn node: every cell's chance leaves are enumerated
/// (no search), cells with the same leaf positions and probabilities share a fingerprint, and each cell gets bounds from
/// what is already known (winners, transposition-table bounds, proven results; anything else -1 / +1). equiv: rows with
/// the same fingerprints on every column are one action (first kept), columns mirrored. prune: iterated weak dominance in
/// those certified bounds (a row whose upper bounds are <= another live row's lower bounds on every live column; columns
/// mirrored): it keeps the game's value. None = not reduced (too big).
fn eg_reduce(cx: &mut EgCtx, view: &Engine, ours: &[[Action; 2]], theirs: &[[Action; 2]], key: Option<u128>, left: usize)
             -> Result<Option<(Vec<usize>, Vec<usize>)>, &'static str> {
    let _egp = egp(EGP_REDUCE);
    let (n, k) = (ours.len(), theirs.len());
    if n * k > 4096 {
        return Ok(None);
    }
    let ck = key.map(|kk| kk ^ 0x0EED_0000_5EED_0000_0EED_0000_5EED_0007u128);
    if let Some(e) = ck.and_then(|c| cx.sh.red.get(c)) {
        if !cx.opts.prune || e.2 >= left {
            return Ok(Some((e.0, e.1)));
        }
    }
    let dex = cx.dex;
    let s = cell_seed(cx.seed, 0);
    let mut fp = vec![0u128; n * k];
    let mut lb = vec![-1.0f64; n * k];
    let mut ub = vec![1.0f64; n * k];
    for i in 0..n {
        for j in 0..k {
            let c = i * k + j;
            let lv = eg_enum(cx.opts.nodice, view, dex, ours[i], theirs[j], s, cx.max_leaves, 0.0).0;
            eg_spend(cx, lv.len().max(1))?;
            let tot: f64 = lv.iter().map(|x| x.1 as f64).sum();
            if lv.len() >= cx.max_leaves || (tot - 1.0).abs() > 1e-3 {
                fp[c] = 0xC0FF_EE00_0000_0000_0000_0000_0000_0000u128 ^ ((c as u128) << 1) ^ 1;
                continue;
            }
            let (groups, ids) = eg_merge_leaves(cx, &lv, false);
            let mut it: Vec<(u128, u64)> = ids.iter().zip(groups.iter()).map(|(id, g)| (*id, (g.1 / tot * 1e9).round() as u64)).collect();
            it.sort_unstable();
            let mut hh = 0x6C62_272E_07BB_0142_62B8_2175_6295_C58Du128;
            for (kk, p) in it {
                hh = (hh ^ kk).wrapping_mul(0x0000_0000_0100_0000_0000_0000_0000_013Bu128) ^ p as u128;
            }
            fp[c] = hh;
            if !cx.opts.prune {
                continue;
            }
            let (mut l, mut h) = (0.0, 0.0);
            for (g, id) in groups.iter().zip(ids.iter()) {
                let leaf = &lv[g.0].0;
                let p = g.1 / tot;
                let (a, b) = if let Some(w) = leaf.winner {
                    let v = if w == 0 { 1.0 } else if w == 1 { -1.0 } else { 0.0 };
                    (v, v)
                } else if leaf.phase == Phase::Turn {
                    match g.2.filter(|_| cx.opts.tt).and_then(|kk| cx.sh.tt.get(kk)) {
                        Some((x, y, _)) => (x, y),
                        None => eg_proven(cx, leaf).map(|v| (v, v)).unwrap_or((-1.0, 1.0)),
                    }
                } else if cx.opts.merge && cx.opts.tt {
                    cx.sh.tt.get(*id).map(|(x, y, _)| (x, y)).unwrap_or((-1.0, 1.0))
                } else {
                    (-1.0, 1.0)
                };
                l += p * a;
                h += p * b;
            }
            lb[c] = l;
            ub[c] = h;
        }
    }
    let mut ra = vec![true; n];
    let mut ca = vec![true; k];
    if cx.opts.equiv {
        let mut seen: std::collections::HashSet<Vec<u128>> = std::collections::HashSet::new();
        for i in 0..n {
            if !seen.insert(fp[i * k..i * k + k].to_vec()) {
                ra[i] = false;
            }
        }
        let mut seen: std::collections::HashSet<Vec<u128>> = std::collections::HashSet::new();
        for j in 0..k {
            let col: Vec<u128> = (0..n).filter(|&i| ra[i]).map(|i| fp[i * k + j]).collect();
            if !seen.insert(col) {
                ca[j] = false;
            }
        }
    }
    if cx.opts.prune && (n * n * k + k * k * n) as f64 <= 2e7 {
        let tol = 1e-12;
        loop {
            let mut changed = false;
            for i in 0..n {
                if !ra[i] { continue; }
                for i2 in 0..n {
                    if i2 == i || !ra[i2] { continue; }
                    if (0..k).all(|j| !ca[j] || ub[i * k + j] <= lb[i2 * k + j] + tol) {
                        ra[i] = false;
                        changed = true;
                        break;
                    }
                }
            }
            for j in 0..k {
                if !ca[j] { continue; }
                for j2 in 0..k {
                    if j2 == j || !ca[j2] { continue; }
                    if (0..n).all(|i| !ra[i] || lb[i * k + j] + tol >= ub[i * k + j2]) {
                        ca[j] = false;
                        changed = true;
                        break;
                    }
                }
            }
            if !changed { break; }
        }
    }
    let rows: Vec<usize> = (0..n).filter(|&i| ra[i]).collect();
    let cols: Vec<usize> = (0..k).filter(|&j| ca[j]).collect();
    if let Some(c) = ck {
        cx.sh.red.insert(c, (rows.clone(), cols.clone(), if cx.opts.prune { left } else { usize::MAX }));
    }
    Ok(Some((rows, cols)))
}

/// (opts.thresh) the value c of the next cell that makes a candidate row's bound u + w c + (r - w) <= v + e
fn eg_thr_row(u: f64, r: f64, w: f64, v: f64) -> f64 {
    if w <= 1e-12 {
        return if u + r <= v { f64::INFINITY } else { f64::NEG_INFINITY };
    }
    (v - u - (r - w)) / w
}

/// (opts.thresh) the value c of the next cell that makes a candidate column's bound u + w c - (r - w) >= v
fn eg_thr_col(u: f64, r: f64, w: f64, v: f64) -> f64 {
    if w <= 1e-12 {
        return if u - r >= v { f64::NEG_INFINITY } else { f64::INFINITY };
    }
    (v - u + (r - w)) / w
}


/// x' M y with x, y from rm_core on the sub-game rows `ri` x cols `cj` of the full n x k matrix m
fn eg_sub(m: &[f64], k: usize, ri: &[usize], cj: &[usize], iters: usize) -> (Vec<f64>, Vec<f64>, f64) {
    let (a, b) = (ri.len(), cj.len());
    let mut sub = Vec::with_capacity(a * b);
    for &i in ri {
        for &j in cj {
            sub.push(m[i * k + j]);
        }
    }
    if a == 1 && b == 1 {
        return (vec![1.0], vec![1.0], sub[0]);
    }
    let (x, y) = eg_rm(&sub, a, b, iters);
    let mut v = 0.0;
    for p in 0..a {
        for q in 0..b {
            v += x[p] * sub[p * b + q] * y[q];
        }
    }
    (x, y, v)
}

/// (2026-11-01) double-oracle turn node: cells evaluated only for (our support x all theirs) and (all ours x their
/// support); the restricted lo / hi games are solved and best responses added until neither side can improve either
/// game by more than eps against the restricted equilibria. Value semantics = the full-matrix solver's (approximate
/// equilibrium values of the lo and hi games).
fn eg_turn_dbl(cx: &mut EgCtx, view: &Engine, ours: &[[Action; 2]], theirs: &[[Action; 2]], d: usize, depth: usize,
               key: Option<u128>) -> Result<(f64, f64), &'static str> {
    let _egp = egp(EGP_TURN_DBL);
    let (n, k) = (ours.len(), theirs.len());
    let mut lo = vec![f64::NAN; n * k];
    let mut hi = vec![f64::NAN; n * k];
    // start from the supports a shallower iteration ended with at this position (same legal lists: same position)
    let (mut so, mut st): (Vec<usize>, Vec<usize>) = match key.and_then(|kk| cx.sh.sup.get(kk)) {
        Some(e) if e.so.iter().all(|&i| i < n) && e.st.iter().all(|&j| j < k) && !e.so.is_empty() && !e.st.is_empty() => (e.so, e.st),
        _ => (vec![0], vec![0]),
    };
    let eps = cx.opts.eps;
    let iters = cx.iters;
    let mut todo: Vec<(usize, usize)> = Vec::new();
    for &i in &so {
        todo.extend((0..k).map(|j| (i, j)));
    }
    for &j in &st {
        todo.extend((0..n).map(|i| (i, j)));
    }
    loop {
        for &(i, j) in &todo {
            if lo[i * k + j].is_nan() {
                eg_spend(cx, 1)?;
                let (l, h) = eg_cell(cx, view, ours[i], theirs[j], d, depth)?;
                lo[i * k + j] = l;
                hi[i * k + j] = h;
            }
        }
        todo.clear();
        let (xl, yl, vl) = eg_sub(&lo, k, &so, &st, iters);
        let (xh, yh, vh) = eg_sub(&hi, k, &so, &st, iters);
        let mut add_r: Vec<usize> = Vec::new();
        let mut add_c: Vec<usize> = Vec::new();
        for (m, x, y, v) in [(&lo, &xl, &yl, vl), (&hi, &xh, &yh, vh)] {
            // our best response against y (every row is evaluated on the support columns)
            let (mut bi, mut bv) = (usize::MAX, v + eps);
            for i in 0..n {
                let u: f64 = st.iter().zip(y.iter()).map(|(&j, &q)| q * m[i * k + j]).sum();
                if u > bv { bv = u; bi = i; }
            }
            if bi != usize::MAX && !so.contains(&bi) && !add_r.contains(&bi) { add_r.push(bi); }
            // their best response against x
            let (mut bj, mut bw) = (usize::MAX, v - eps);
            for j in 0..k {
                let u: f64 = so.iter().zip(x.iter()).map(|(&i, &p)| p * m[i * k + j]).sum();
                if u < bw { bw = u; bj = j; }
            }
            if bj != usize::MAX && !st.contains(&bj) && !add_c.contains(&bj) { add_c.push(bj); }
        }
        if add_r.is_empty() && add_c.is_empty() {
            if let Some(kk) = key {
                cx.sh.sup.insert(kk, EgSup { so, st, rs: Vec::new(), cs: Vec::new() });
            }
            return Ok((vl, vh));
        }
        for &i in &add_r {
            so.push(i);
            todo.extend((0..k).map(|j| (i, j)));
        }
        for &j in &add_c {
            st.push(j);
            todo.extend((0..n).map(|i| (i, j)));
        }
    }
}

/// (2026-11-01, opts.smab) lazy double oracle: only the support block is evaluated in full; a candidate best response is
/// evaluated cell by cell (largest opponent weight first) and dropped as soon as its bound, with every unevaluated cell
/// at its most favourable value (+1 for our rows, -1 for their columns), proves it can't beat both restricted games by
/// eps (nor the best candidate found so far). Same fixed point as eg_turn_dbl. With opts.early, a restricted game already
/// worth +1 (-1) tries to certify one support row (column) against every reply and returns at once.
#[allow(clippy::too_many_arguments)]
/// (opts.order2) key of a joint action for the history table
fn eg_jkey(a: &[Action; 2]) -> u32 {
    let one = |x: Action| -> u32 {
        match x {
            Action::Move { slot, target, mega } => 0x1000 | ((slot as u32) << 8) | (((target as i32 + 4) as u32 & 15) << 4) | mega as u32,
            Action::Switch { to } => 0x2000 | to as u32,
            Action::Pass => 0x3000,
        }
    };
    (one(a[0]) << 16) | one(a[1])
}

fn eg_turn_lazy(cx: &mut EgCtx, view: &Engine, ours: &[[Action; 2]], theirs: &[[Action; 2]], d: usize, depth: usize,
                key: Option<u128>, win: (f64, f64)) -> Result<(f64, f64), &'static str> {
    let _egp = egp(EGP_TURN_LAZY);
    let (n, k) = (ours.len(), theirs.len());
    let mut lo = vec![f64::NAN; n * k];
    let mut hi = vec![f64::NAN; n * k];
    // (opts.thresh) cells cut short by their window: lo / hi hold valid but looser bounds; re-evaluated in full when needed
    let thresh = cx.opts.thresh;
    let mut cutm = vec![false; n * k];
    let node_win = thresh && (win.0 > -1.0 || win.1 < 1.0);
    let order = cx.opts.order;
    // (opts.order) candidate scores: rows high = promising for us, columns low = promising for them
    let (mut rs, mut cs) = (vec![0.0f64; n], vec![0.0f64; k]);
    let (mut so, mut st): (Vec<usize>, Vec<usize>) = match key.and_then(|kk| cx.sh.sup.get(kk)) {
        Some(e) if e.so.iter().all(|&i| i < n) && e.st.iter().all(|&j| j < k) && !e.so.is_empty() && !e.st.is_empty() => {
            if e.rs.len() == n && e.cs.len() == k {
                rs = e.rs;
                cs = e.cs;
            }
            (e.so, e.st)
        }
        _ => {
            if cx.opts.order2 {
                // (opts.order2) start from the joint actions with the best history in this solve
                let pick = |side: usize, acts: &[[Action; 2]], cx: &EgCtx| -> usize {
                    let mut b = (0usize, 0.0f64);
                    for (i, a) in acts.iter().enumerate() {
                        let h = cx.hist[side].get(&eg_jkey(a)).copied().unwrap_or(0.0);
                        if h > b.1 { b = (i, h); }
                    }
                    b.0
                };
                (vec![pick(0, ours, cx)], vec![pick(1, theirs, cx)])
            } else {
                (vec![0], vec![0])
            }
        }
    };
    let eps = cx.opts.eps;
    let iters = cx.iters;
    let order2 = cx.opts.order2;
    let hbump = 1.0 + depth.saturating_sub(d) as f64;
    macro_rules! cell {
        ($i:expr, $j:expr) => {{
            let (ii, jj) = ($i, $j);
            if lo[ii * k + jj].is_nan() || cutm[ii * k + jj] {
                eg_spend(cx, 1)?;
                cx.win = EG_NOWIN;
                let (l, h) = eg_cell(cx, view, ours[ii], theirs[jj], d, depth)?;
                cx.cut = false;
                lo[ii * k + jj] = l;
                hi[ii * k + jj] = h;
                cutm[ii * k + jj] = false;
            }
        }};
    }
    // a candidate's cell with a window (unevaluated only)
    macro_rules! cellw {
        ($i:expr, $j:expr, $a:expr, $b:expr) => {{
            let (ii, jj) = ($i, $j);
            if lo[ii * k + jj].is_nan() {
                eg_spend(cx, 1)?;
                cx.win = ($a, $b);
                let (l, h) = eg_cell(cx, view, ours[ii], theirs[jj], d, depth)?;
                cx.win = EG_NOWIN;
                let c = std::mem::replace(&mut cx.cut, false);
                lo[ii * k + jj] = l;
                hi[ii * k + jj] = h;
                cutm[ii * k + jj] = c;
            }
        }};
    }
    // cheap node bounds from the evaluated cells (the restricted mixes against every reply, unevaluated cells at -1 / +1)
    macro_rules! node_cut {
        ($xl:expr, $yh:expr) => {{
            if node_win {
                let mut l0 = f64::INFINITY;
                for j in 0..k {
                    let mut u = 0.0;
                    for (p, &i) in so.iter().enumerate() {
                        let v = lo[i * k + j];
                        u += $xl[p] * if v.is_nan() { -1.0 } else { v };
                    }
                    l0 = l0.min(u);
                }
                let mut u0 = f64::NEG_INFINITY;
                for i in 0..n {
                    let mut u = 0.0;
                    for (q, &j) in st.iter().enumerate() {
                        let v = hi[i * k + j];
                        u += $yh[q] * if v.is_nan() { 1.0 } else { v };
                    }
                    u0 = u0.max(u);
                }
                if u0 <= win.0 || l0 >= win.1 {
                    cx.cut = true;
                    return Ok((l0.min(u0), u0.max(l0)));
                }
            }
        }};
    }
    loop {
        for &i in &so {
            for &j in &st {
                cell!(i, j);
            }
        }
        let (xl, yl, vl) = eg_sub(&lo, k, &so, &st, iters);
        let (xh, yh, vh) = eg_sub(&hi, k, &so, &st, iters);
        node_cut!(xl, yh);
        if cx.opts.early {
            // proven win: a support row whose lo is +1 against EVERY reply (stop at the first reply that isn't)
            if vl >= 1.0 - 1e-12 {
                for &i in &so {
                    let mut ok = true;
                    for j in 0..k {
                        cell!(i, j);
                        if lo[i * k + j] < 1.0 - 1e-12 { ok = false; break; }
                    }
                    if ok { cx.cut = false; return Ok((1.0, 1.0)); }
                }
            }
            if vh <= -1.0 + 1e-12 {
                for &j in &st {
                    let mut ok = true;
                    for i in 0..n {
                        cell!(i, j);
                        if hi[i * k + j] > -1.0 + 1e-12 { ok = false; break; }
                    }
                    if ok { cx.cut = false; return Ok((-1.0, -1.0)); }
                }
            }
        }
        // our best responses: rows outside the support, columns of st by weight
        let mut ord_c: Vec<usize> = (0..st.len()).collect();
        ord_c.sort_by(|&a, &b| (yl[b].max(yh[b])).partial_cmp(&yl[a].max(yh[a])).unwrap());
        let (mut best_r, mut bu) = (usize::MAX, f64::NEG_INFINITY);
        let mut rows: Vec<usize> = (0..n).filter(|i| !so.contains(i)).collect();
        if order {
            rows.sort_by(|&a, &b| rs[b].partial_cmp(&rs[a]).unwrap_or(std::cmp::Ordering::Equal));
        } else if order2 {
            let hs: Vec<f64> = rows.iter().map(|&i| cx.hist[0].get(&eg_jkey(&ours[i])).copied().unwrap_or(0.0)).collect();
            let mut idx: Vec<usize> = (0..rows.len()).collect();
            idx.sort_by(|&a, &b| hs[b].partial_cmp(&hs[a]).unwrap_or(std::cmp::Ordering::Equal));
            rows = idx.into_iter().map(|p| rows[p]).collect();
        }
        for i in rows {
            // bounds: unevaluated cells counted at +1
            let (mut ul, mut uh) = (0.0, 0.0);
            let (mut rl, mut rh) = (1.0, 1.0); // remaining weight
            let mut pruned = false;
            for &p in &ord_c {
                let j = st[p];
                if thresh {
                    let a1 = eg_thr_row(ul, rl, yl[p], vl + eps).min(eg_thr_row(uh, rh, yh[p], vh + eps));
                    let a2 = if bu > f64::NEG_INFINITY {
                        eg_thr_row(ul, rl, yl[p], vl + eps + bu).min(eg_thr_row(uh, rh, yh[p], vh + eps + bu))
                    } else {
                        f64::NEG_INFINITY
                    };
                    cellw!(i, j, a1.max(a2), f64::INFINITY);
                } else {
                    cell!(i, j);
                }
                // a cut cell: only its upper bound is known
                let cl = if cutm[i * k + j] { hi[i * k + j] } else { lo[i * k + j] };
                ul += yl[p] * cl;
                uh += yh[p] * hi[i * k + j];
                rl -= yl[p];
                rh -= yh[p];
                let bl = ul + rl.max(0.0);
                let bh = uh + rh.max(0.0);
                let gl = bl - (vl + eps);
                let gh = bh - (vh + eps);
                if gl <= 0.0 && gh <= 0.0 { pruned = true; rs[i] = 0.5 * (bl + bh); break; }
                if gl.max(gh) <= bu { pruned = true; rs[i] = 0.5 * (bl + bh); break; }
            }
            if pruned { continue; }
            rs[i] = 0.5 * (ul + uh);
            let g = (ul - (vl + eps)).max(uh - (vh + eps));
            if g > 0.0 && g > bu { bu = g; best_r = i; }
        }
        node_cut!(xl, yh);
        // their best responses: columns outside the support, rows of so by weight; unevaluated cells at -1
        let mut ord_r: Vec<usize> = (0..so.len()).collect();
        ord_r.sort_by(|&a, &b| (xl[b].max(xh[b])).partial_cmp(&xl[a].max(xh[a])).unwrap());
        let (mut best_c, mut bw) = (usize::MAX, f64::NEG_INFINITY);
        let mut cols: Vec<usize> = (0..k).filter(|j| !st.contains(j)).collect();
        if order {
            cols.sort_by(|&a, &b| cs[a].partial_cmp(&cs[b]).unwrap_or(std::cmp::Ordering::Equal));
        } else if order2 {
            let hs: Vec<f64> = cols.iter().map(|&j| cx.hist[1].get(&eg_jkey(&theirs[j])).copied().unwrap_or(0.0)).collect();
            let mut idx: Vec<usize> = (0..cols.len()).collect();
            idx.sort_by(|&a, &b| hs[b].partial_cmp(&hs[a]).unwrap_or(std::cmp::Ordering::Equal));
            cols = idx.into_iter().map(|p| cols[p]).collect();
        }
        for j in cols {
            let (mut ul, mut uh) = (0.0, 0.0);
            let (mut rl, mut rh) = (1.0, 1.0);
            let mut pruned = false;
            for &p in &ord_r {
                let i = so[p];
                if thresh {
                    let b1 = eg_thr_col(ul, rl, xl[p], vl - eps).max(eg_thr_col(uh, rh, xh[p], vh - eps));
                    let b2 = if bw > f64::NEG_INFINITY {
                        eg_thr_col(ul, rl, xl[p], vl - eps - bw).max(eg_thr_col(uh, rh, xh[p], vh - eps - bw))
                    } else {
                        f64::INFINITY
                    };
                    cellw!(i, j, f64::NEG_INFINITY, b1.min(b2));
                } else {
                    cell!(i, j);
                }
                // a cut cell: only its lower bound is known
                let ch = if cutm[i * k + j] { lo[i * k + j] } else { hi[i * k + j] };
                ul += xl[p] * lo[i * k + j];
                uh += xh[p] * ch;
                rl -= xl[p];
                rh -= xh[p];
                let bl = ul - rl.max(0.0);
                let bh = uh - rh.max(0.0);
                let gl = (vl - eps) - bl;
                let gh = (vh - eps) - bh;
                if gl <= 0.0 && gh <= 0.0 { pruned = true; cs[j] = 0.5 * (bl + bh); break; }
                if gl.max(gh) <= bw { pruned = true; cs[j] = 0.5 * (bl + bh); break; }
            }
            if pruned { continue; }
            cs[j] = 0.5 * (ul + uh);
            let g = ((vl - eps) - ul).max((vh - eps) - uh);
            if g > 0.0 && g > bw { bw = g; best_c = j; }
        }
        if best_r == usize::MAX && best_c == usize::MAX {
            if order2 {
                // final supports, by weight
                for (p, &i) in so.iter().enumerate() {
                    *cx.hist[0].entry(eg_jkey(&ours[i])).or_insert(0.0) += hbump * xl[p].max(xh[p]);
                }
                for (q, &j) in st.iter().enumerate() {
                    *cx.hist[1].entry(eg_jkey(&theirs[j])).or_insert(0.0) += hbump * yl[q].max(yh[q]);
                }
            }
            if let Some(kk) = key {
                if order {
                    // support rows / columns: their value in the restricted games (kept first next time)
                    for (p, &i) in so.iter().enumerate() {
                        rs[i] = 2.0 + xl[p].max(xh[p]);
                    }
                    for (q, &j) in st.iter().enumerate() {
                        cs[j] = -2.0 - yl[q].max(yh[q]);
                    }
                }
                cx.sh.sup.insert(kk, EgSup { so, st, rs, cs });
            }
            cx.cut = false;
            return Ok((vl, vh));
        }
        if order2 {
            if best_r != usize::MAX { *cx.hist[0].entry(eg_jkey(&ours[best_r])).or_insert(0.0) += hbump; }
            if best_c != usize::MAX { *cx.hist[1].entry(eg_jkey(&theirs[best_c])).or_insert(0.0) += hbump; }
        }
        if best_r != usize::MAX { so.push(best_r); }
        if best_c != usize::MAX { st.push(best_c); }
    }
}

fn eg_val(cx: &mut EgCtx, c: &Engine, d: usize, depth: usize) -> Result<(f64, f64), &'static str> {
    eg_val_k(cx, c, d, depth, None)
}

/// eg_val with the position's eg_key when the caller already computed it
fn eg_val_k(cx: &mut EgCtx, c: &Engine, d: usize, depth: usize, known_key: Option<u128>) -> Result<(f64, f64), &'static str> {
    // (opts.thresh) the caller's window applies to this node only; cx.cut tells the caller whether it was cut short
    let win = std::mem::replace(&mut cx.win, EG_NOWIN);
    cx.cut = false;
    let r = eg_val_body(cx, c, d, depth, known_key, win);
    if r.is_err() {
        cx.cut = false;
    }
    r
}

fn eg_val_body(cx: &mut EgCtx, c: &Engine, d: usize, depth: usize, known_key: Option<u128>, win: (f64, f64))
               -> Result<(f64, f64), &'static str> {
    let _egp = egp(EGP_VAL_BODY);
    if let Some(w) = c.winner {
        let v = if w == 0 { 1.0 } else if w == 1 { -1.0 } else { 0.0 };
        return Ok((v, v));
    }
    let dex = cx.dex;
    match c.phase {
        Phase::Switch => {
            // (opts.merge) replacement nodes in the transposition table too (their value depends on the depth left)
            let skey = if cx.opts.merge && cx.opts.tt { Some(eg_skey(cx, c) ^ EG_SWITCH_TAG) } else { None };
            let left = depth.saturating_sub(d);
            if let Some(kk) = skey {
                if let Some((l, h, dl)) = cx.sh.tt.get(kk) {
                    if dl > left || h - l < 1e-9 || (dl == left && (!cx.strict || cx.strict_done.contains(&kk))) {
                        cx.stats[1] += 1;
                        return Ok((l, h));
                    }
                }
            }
            let mut jo = policy::replacement_joints(c, dex, 0);
            let mut jt = policy::replacement_joints(c, dex, 1);
            jo.truncate(64);
            jt.truncate(64);
            let (na, nb) = (jo.len().max(1), jt.len().max(1));
            if jo.is_empty() { jo.push([Action::Pass, Action::Pass]); }
            if jt.is_empty() { jt.push([Action::Pass, Action::Pass]); }
            eg_spend(cx, na * nb)?;
            let (mut lo, mut hi) = (Vec::with_capacity(na * nb), Vec::with_capacity(na * nb));
            for a in &jo {
                for bb in &jt {
                    let mut ch = c.clone();
                    if ch.needs_choice(0) { ch.choose(dex, 0, *a); }
                    if ch.needs_choice(1) { ch.choose(dex, 1, *bb); }
                    let (l, h) = eg_val(cx, &ch, d, depth)?;
                    lo.push(l);
                    hi.push(h);
                }
            }
            let r = eg_pair(cx, &lo, &hi, na, nb);
            if let Some(kk) = skey {
                cx.sh.tt.insert(kk, (r.0, r.1, left));
                if cx.strict {
                    cx.strict_done.insert(kk);
                }
            }
            Ok(r)
        }
        Phase::Turn => {
            if let Some(v) = eg_proven(cx, c) {
                return Ok((v, v));
            }
            // (opts.sub1) a 1v1 inside a bigger endgame: its own sub-solve, cached
            let mut sub: Option<(f64, f64)> = None;
            if cx.opts.sub1 > 0 && cx.sub_ok && !cx.ext_on && d > 0 && eg_leaf_qualifies(c, 1) {
                let kk = known_key.unwrap_or_else(|| eg_skey(cx, c));
                let (l, h, reached) = eg_sub1(cx, c, kk)?;
                if h - l < 1e-9 || d >= depth || reached >= depth - d {
                    return Ok((l, h));
                }
                sub = Some((l, h));
            }
            // (opts.hpmono) bounds from the same position at other HP levels
            let hpk = if cx.opts.hpmono && eg_hp_ok(c, dex) { Some(eg_hp_key(c, dex)) } else { None };
            let hpb = hpk.as_ref().map(|(hk, hv)| eg_hp_bounds(cx, *hk, hv));
            if let Some((l, h)) = hpb {
                if h - l < 1e-9 {
                    cx.stats[1] += 1;
                    return Ok((l, h));
                }
            }
            if d >= depth {
                // (opts.race_ext) a 1v1 at the depth limit is searched race_ext turns deeper, once per line
                if cx.opts.race_ext > 0 && !cx.ext_on && eg_leaf_qualifies(c, 1) {
                    cx.ext_on = true;
                    cx.win = win;
                    let r = eg_val_k(cx, c, d, depth + cx.opts.race_ext, known_key);
                    cx.ext_on = false;
                    return r;
                }
                // (opts.fix1) the table's bounds for this position, then one turn of security bounds
                if cx.opts.fix1 > 0 {
                    let mut r = hpb.unwrap_or((-1.0, 1.0));
                    let kk = known_key.unwrap_or_else(|| eg_skey(cx, c));
                    let old = if cx.opts.tt { cx.sh.tt.get(kk) } else { None };
                    if let Some((l, h, _)) = old {
                        if r.0.max(l) <= r.1.min(h) {
                            r = (r.0.max(l), r.1.min(h));
                        }
                    }
                    if r.1 - r.0 > 1e-9 && cx.reach >= cx.opts.fix_min {
                        let f = eg_fix1(cx, c)?;
                        if r.0.max(f.0) <= r.1.min(f.1) {
                            r = (r.0.max(f.0), r.1.min(f.1));
                        }
                        if cx.opts.tt && old.map(|o| o.0 < r.0 - 1e-12 || o.1 > r.1 + 1e-12).unwrap_or(true) {
                            cx.sh.tt.insert(kk, (r.0, r.1, old.map(|o| o.2).unwrap_or(0)));
                        }
                    }
                    return Ok(r);
                }
                return Ok(hpb.unwrap_or((-1.0, 1.0)));
            }
            let left = depth - d;
            let key = if cx.opts.tt || cx.opts.dbl { Some(known_key.unwrap_or_else(|| eg_skey(cx, c))) } else { None };
            let mut prior: Option<(f64, f64)> = None;
            if let Some(kk) = key.filter(|_| cx.opts.tt) {
                if let Some((l, h, dl)) = cx.sh.tt.get(kk) {
                    if dl > left || h - l < 1e-9 || (dl == left && (!cx.strict || cx.strict_done.contains(&kk))) {
                        cx.stats[1] += 1;
                        return Ok((l, h));
                    }
                    if cx.opts.skip > 0.0 && cx.reach * (h - l) < cx.opts.skip {
                        cx.stats[4] += 1;
                        return Ok((l, h));
                    }
                    prior = Some((l, h));
                }
            }
            for (l, h) in [sub, hpb].into_iter().flatten() {
                prior = Some(match prior {
                    Some((a, b)) if a.max(l) <= b.min(h) => (a.max(l), b.min(h)),
                    Some(p) => p,
                    None => (l, h),
                });
            }
            if let Some((l, h)) = prior {
                if h - l < 1e-9 {
                    return Ok((l, h));
                }
            }
            let ours = eg_joints(c, dex, 0);
            let theirs = eg_joints(c, dex, 1);
            let (n, k) = (ours.len(), theirs.len());
            cx.stats[0] += 1;
            cx.stats[3] += (n * k) as u64;
            let view = { let _egp = egp(EGP_SEARCH_COPY); c.search_copy(false) };
            // (opts.mirror) caches of action indices (supports, reductions) use the key without slot mirroring
            let tkey = key;
            let key = key.map(|kk| eg_akey(cx, c, kk));
            // (opts.equiv / opts.prune) equivalent and dominated joint actions dropped
            // only at the root turn: a full leaf enumeration per node undoes the lazy double oracle below it (2026-11-01)
            let (ours, theirs) = if (cx.opts.equiv || cx.opts.prune) && d == 0 && n * k > 4 {
                match eg_reduce(cx, &view, &ours, &theirs, key, left)? {
                    Some((ri, cj)) => (ri.iter().map(|&i| ours[i]).collect::<Vec<_>>(), cj.iter().map(|&j| theirs[j]).collect::<Vec<_>>()),
                    None => (ours, theirs),
                }
            } else {
                (ours, theirs)
            };
            let (n, k) = (ours.len(), theirs.len());
            let mut cut = false;
            let r = if cx.opts.dbl && cx.opts.smab && n * k > 4 {
                let r = eg_turn_lazy(cx, &view, &ours, &theirs, d, depth, key, if cx.opts.thresh { win } else { EG_NOWIN })?;
                cut = std::mem::replace(&mut cx.cut, false);
                r
            } else if cx.opts.dbl && n * k > 4 {
                eg_turn_dbl(cx, &view, &ours, &theirs, d, depth, key)?
            } else {
                eg_spend(cx, n * k)?;
                let (mut lo, mut hi) = (Vec::with_capacity(n * k), Vec::with_capacity(n * k));
                let mut won: Option<f64> = None;
                'rows: for o in &ours {
                    let mut row_win = true;
                    for t in &theirs {
                        let (l, h) = eg_cell(cx, &view, *o, *t, d, depth)?;
                        row_win &= l >= 1.0 - 1e-12;
                        lo.push(l);
                        hi.push(h);
                    }
                    // (opts.early) one action of ours wins against every reply: the node is won
                    if cx.opts.early && row_win {
                        won = Some(1.0);
                        break 'rows;
                    }
                }
                match won {
                    Some(v) => (v, v),
                    None => eg_pair(cx, &lo, &hi, n, k),
                }
            };
            // a shallower search's bounds are bounds too: keep the tighter of both (when they overlap)
            let r = match prior {
                Some((l, h)) if r.0.max(l) <= r.1.min(h) => (r.0.max(l), r.1.min(h)),
                _ => r,
            };
            if let Some(kk) = tkey.filter(|_| cx.opts.tt && !cut) {
                cx.sh.tt.insert(kk, (r.0, r.1, left));
                if cx.strict {
                    cx.strict_done.insert(kk);
                }
            }
            if let Some((hk, hv)) = hpk.filter(|_| !cut) {
                eg_hp_store(cx, hk, hv, r.0, r.1);
            }
            cx.cut = cut;
            Ok(r)
        }
        _ => Err("phase"),
    }
}

/// (value or None, nodes, why, gap, depth)
#[allow(clippy::too_many_arguments)]
fn eg_exact(b: &Engine, dex: &EngineDex, budget: usize, max_depth: usize, max_leaves: usize, iters: usize, seed: u64,
            tol: f64) -> (Option<f64>, usize, &'static str, f64, usize) {
    eg_exact_with(b, dex, budget, max_depth, max_leaves, iters, seed, tol, None)
}

/// eg_exact with the solver's "average luck" mode forced on / off for this solve only (fresh tables: never mixed with the
/// persistent ones of exact solves)
#[allow(clippy::too_many_arguments)]
fn eg_exact_with(b: &Engine, dex: &EngineDex, budget: usize, max_depth: usize, max_leaves: usize, iters: usize, seed: u64,
                 tol: f64, nodice: Option<bool>) -> (Option<f64>, usize, &'static str, f64, usize) {
    let _egp = egp(EGP_ROOT);
    let mut cx = eg_ctx(dex, budget, max_leaves, iters, seed, None);
    if let Some(nd) = nodice {
        if nd != cx.opts.nodice {
            cx.opts.nodice = nd;
            cx.opts.persist = false;
            cx.sh = Arc::new(EgShared::new());
        }
    }
    cx.sub_ok = !eg_leaf_qualifies(b, 1);
    cx.hz = eg_hz(&cx.opts, max_depth);
    let (mut gap, mut dep) = (2.0, 0);
    for depth in 1..=max_depth.max(1) {
        dep = depth;
        let mut last = f64::INFINITY;
        let mut pass = 0usize;
        loop {
            // (opts.loop_n) strict re-passes at the same depth
            cx.strict = pass > 0;
            cx.strict_done.clear();
            cx.tail_used = 0.0;
            match eg_val(&mut cx, b, 0, depth) {
                Ok((lo, hi)) => {
                    gap = hi - lo;
                    if gap <= 2.0 * tol {
                        return (Some(0.5 * (lo + hi)), cx.nodes(), "solved", gap, dep);
                    }
                }
                Err(e) => return (None, cx.nodes(), e, gap, dep),
            }
            if pass >= cx.opts.loop_n || (pass > 0 && last - gap <= 1e-6) {
                break;
            }
            last = gap;
            pass += 1;
        }
        cx.strict = false;
    }
    (None, cx.nodes(), "depth", gap, dep)
}

/// one root cell of eg_root: expectation over its (merged) chance leaves
fn eg_root_cell(cx: &mut EgCtx, lv: &[(Engine, f32)], g: &[(usize, f64, Option<u128>)], depth: usize, max_leaves: usize)
                -> Result<(f64, f64), &'static str> {
    let tot: f64 = lv.iter().map(|x| x.1 as f64).sum();
    eg_spend(cx, lv.len())?;
    cx.tail_used = 0.0;
    let (mut l, mut h) = (0.0, 0.0);
    for &(i, p, key) in g {
        cx.reach = p;
        let (a, b) = eg_val_k(cx, &lv[i].0, 1, depth, key)?;
        l += p * a;
        h += p * b;
    }
    Ok(if lv.len() >= max_leaves {
        let miss = (1.0 - tot).max(0.0);
        (l - miss, h + miss)
    } else {
        (l / tot, h / tot)
    })
}

/// (2026-10-30) the root of eg_exact as a game: per (our joint x their joint) cell the certified lower / upper bounds of
/// side 0's result, at the first depth whose root value gap <= 2 tol. Returns (solved, why, gap, depth, nodes, ours,
/// theirs, lo, hi); lo / hi row-major [ours x theirs]. Root must be a turn (a replacement root returns why "phase").
#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn eg_root(b: &Engine, dex: &EngineDex, budget: usize, max_depth: usize, max_leaves: usize, iters: usize, seed: u64,
           tol: f64, seconds: f64) -> (bool, &'static str, f64, usize, usize, Vec<[Action; 2]>, Vec<[Action; 2]>, Vec<f64>, Vec<f64>) {
    let _egp = egp(EGP_ROOT);
    let mut end = if seconds > 0.0 { Some(std::time::Instant::now() + std::time::Duration::from_secs_f64(seconds)) } else { None };
    // (opts.bail) >= 2 Pokemon on both sides: a short time slice (these are almost never certified)
    let bail = EG_OPTS.read().unwrap().bail;
    if bail > 0.0 && (0..2).all(|si| b.sides[si].mons.iter().filter(|m| m.alive()).count() >= 2) {
        let e2 = std::time::Instant::now() + std::time::Duration::from_secs_f64(bail);
        end = Some(end.map_or(e2, |e| e.min(e2)));
    }
    let mut cx = eg_ctx(dex, budget, max_leaves, iters, seed, end);
    cx.sub_ok = !eg_leaf_qualifies(b, 1);
    cx.hz = eg_hz(&cx.opts, max_depth);
    let ours = eg_joints(b, dex, 0);
    let theirs = eg_joints(b, dex, 1);
    let (n, k) = (ours.len(), theirs.len());
    if b.winner.is_some() || !matches!(b.phase, Phase::Turn) {
        return (false, "phase", 2.0, 0, 0, ours, theirs, Vec::new(), Vec::new());
    }
    let view = { let _egp = egp(EGP_SEARCH_COPY); b.search_copy(false) };
    let s = cell_seed(seed, 0);
    // chance leaves of each root cell, enumerated once (they don't depend on the depth)
    let mut cells: Vec<Vec<(Engine, f32)>> = Vec::with_capacity(n * k);
    for o in &ours {
        for t in &theirs {
            let lv = eg_enum(cx.opts.nodice, &view, dex, *o, *t, s, max_leaves, 0.0).0;
            let capped = lv.len() >= max_leaves;
            if capped && !cx.opts.soft_cap {
                return (false, "leaf cap", 2.0, 0, cx.nodes(), ours, theirs, Vec::new(), Vec::new());
            }
            let tot: f64 = lv.iter().map(|x| x.1 as f64).sum();
            if tot > 1.0 + 1e-3 || (!capped && (tot - 1.0).abs() > 1e-3) {
                return (false, "probabilities", 2.0, 0, cx.nodes(), ours, theirs, Vec::new(), Vec::new());
            }
            cells.push(lv);
        }
    }
    // (opts.merge) each root cell's chance leaves deduplicated once (full keys: valid at every depth)
    let groups: Vec<Vec<(usize, f64, Option<u128>)>> = cells.iter().map(|lv| {
        if cx.opts.merge {
            eg_merge_leaves(&cx, lv, false).0
        } else {
            lv.iter().enumerate().map(|(i, (_, p))| (i, *p as f64, None)).collect()
        }
    }).collect();
    // (opts.equiv, 2026-11-01) root cells with the same merged chance leaves (same keys and probabilities) have the same
    // value: only the first is solved, the others copy it. Below the root, equiv / prune are only used at d == 0 (eg_exact):
    // their full per-node leaf enumeration undid the lazy double oracle (every position timed out).
    let mut same: Vec<usize> = (0..n * k).collect();
    if cx.opts.equiv {
        let mut first: std::collections::HashMap<Vec<(u128, u64)>, usize> = std::collections::HashMap::new();
        for c in 0..n * k {
            if cells[c].len() >= max_leaves {
                continue;
            }
            let (gm, ids) = eg_merge_leaves(&cx, &cells[c], false);
            let mut fp: Vec<(u128, u64)> = gm.iter().zip(ids.iter()).map(|(g, id)| (*id, (g.1 * 1e9).round() as u64)).collect();
            fp.sort_unstable();
            same[c] = *first.entry(fp).or_insert(c);
        }
    }
    let (mut gap, mut dep) = (2.0, 0);
    let (mut lo, mut hi) = (vec![-1.0; n * k], vec![1.0; n * k]);
    for depth in 1..=max_depth.max(1) {
      dep = depth;
      let mut last_gap = f64::INFINITY;
      let mut pass = 0usize;
      loop {
        // (opts.loop_n) strict re-passes at the same depth (bounds intersected)
        cx.strict = pass > 0;
        cx.strict_done.clear();
        if let Err(e) = eg_spend(&mut cx, n * k) {
            return (false, e, gap, dep, cx.nodes(), ours, theirs, lo, hi);
        }
        // (opts.order) cells already exact at a shallower depth stay exact
        let todo: Vec<usize> = (0..n * k).filter(|&c| same[c] == c && !(cx.opts.order && depth > 1 && hi[c] - lo[c] < 1e-9) && !(pass > 0 && hi[c] - lo[c] < 1e-9)).collect();
        let par = cx.opts.par.max(1).min(todo.len().max(1));
        let mut res: Vec<Option<(f64, f64)>> = vec![None; n * k];
        let mut err: Option<&'static str> = None;
        if par <= 1 {
            for &c in &todo {
                match eg_root_cell(&mut cx, &cells[c], &groups[c], depth, max_leaves) {
                    Ok(r) => res[c] = Some(r),
                    Err(e) => {
                        err = Some(e);
                        break;
                    }
                }
            }
        } else {
            // (idea 1) worker threads take cells from a shared index; tables, node budget and stop flag are shared
            use std::sync::atomic::Ordering::Relaxed;
            let next = std::sync::atomic::AtomicUsize::new(0);
            let out = std::sync::Mutex::new((vec![None::<(f64, f64)>; n * k], None::<&'static str>, [0u64; 5]));
            let sh = cx.sh.clone();
            let opts = cx.opts;
            let sub_ok = cx.sub_ok;
            let (hz, strict) = (cx.hz, cx.strict);
            let (todo_r, cells_r, groups_r, next_r, out_r, sh_r) = (&todo, &cells, &groups, &next, &out, &sh);
            std::thread::scope(|sc| {
                for _ in 0..par {
                    sc.spawn(move || {
                        let mut w = EgCtx { dex, budget, max_leaves, iters, seed, sh: sh_r.clone(), report: false, end, opts,
                                            stats: [0; 5], reach: 1.0, win: EG_NOWIN, cut: false, tail_used: 0.0, ext_on: false, sub_ok, hz, strict,
                                            strict_done: std::collections::HashSet::new(),
                                            hist: [std::collections::HashMap::new(), std::collections::HashMap::new()] };
                        loop {
                            let t = next_r.fetch_add(1, Relaxed);
                            if t >= todo_r.len() {
                                break;
                            }
                            let c = todo_r[t];
                            let r = eg_root_cell(&mut w, &cells_r[c], &groups_r[c], depth, max_leaves);
                            let mut g = out_r.lock().unwrap();
                            match r {
                                Ok(v) => g.0[c] = Some(v),
                                Err(e) => {
                                    if e != "stop" && g.1.is_none() {
                                        g.1 = Some(e);
                                    }
                                    w.sh.stop.store(true, Relaxed);
                                    break;
                                }
                            }
                        }
                        let mut g = out_r.lock().unwrap();
                        for q in 0..5 {
                            g.2[q] += w.stats[q];
                        }
                    });
                }
            });
            let (r, e, st) = out.into_inner().unwrap();
            res = r;
            err = e;
            if err.is_none() && sh.stop.load(Relaxed) {
                err = Some("stop");
            }
            for q in 0..5 {
                cx.stats[q] += st[q];
            }
        }
        if let Some(e) = err {
            return (false, e, gap, dep, cx.nodes(), ours, theirs, lo, hi);
        }
        for c in 0..n * k {
            if let Some((a, bb)) = res[same[c]] {
                if pass > 0 && a.max(lo[c]) <= bb.min(hi[c]) {
                    lo[c] = a.max(lo[c]);
                    hi[c] = bb.min(hi[c]);
                } else {
                    lo[c] = a;
                    hi[c] = bb;
                }
            }
        }
        let (vl, vh) = eg_pair(&cx, &lo, &hi, n, k);
        gap = vh - vl;
        if gap <= 2.0 * tol {
            return (true, "solved", gap, dep, cx.nodes(), ours, theirs, lo, hi);
        }
        if pass >= cx.opts.loop_n || (pass > 0 && last_gap - gap <= 1e-6) {
            break;
        }
        last_gap = gap;
        pass += 1;
      }
      cx.strict = false;
    }
    (false, "depth", gap, dep, cx.nodes(), ours, theirs, lo, hi)
}

#[pyclass(frozen)]
struct Dex {
    inner: Arc<EngineDex>,
}

#[pymethods]
impl Dex {
    #[new]
    fn new(path: &str) -> PyResult<Self> {
        Ok(Dex { inner: Arc::new(EngineDex::load(path).map_err(PyValueError::new_err)?) })
    }
}

#[pyclass]
struct Battle {
    dex: Arc<EngineDex>,
    b: Engine,
    /// (2026-10-21) search leaves keep the opponent's stats instead of replacing them with guessed / template spreads
    /// (set by the bridge: bridge.build_engine, team-preview worlds). Off = every world redraws the opponent's stats.
    keep_stats: bool,
}

impl Battle {
    fn check_mon(&self, side: usize, idx: usize) -> PyResult<()> {
        if side > 1 || idx >= self.b.sides[side].mons.len() {
            return Err(PyValueError::new_err(format!("no Pokemon {idx} on side {side}")));
        }
        Ok(())
    }
}

#[pymethods]
impl Battle {
    /// A new battle from two full teams (Showdown export text), starting at team preview.
    #[new]
    #[pyo3(signature = (dex, team_a, team_b, seed=1))]
    fn new(dex: &Dex, team_a: &str, team_b: &str, seed: u64) -> PyResult<Self> {
        let (a, b) = (parse_team(team_a), parse_team(team_b));
        let engine = Engine::new(&dex.inner, [&a, &b], seed).map_err(PyValueError::new_err)?;
        Ok(Battle { dex: dex.inner.clone(), b: engine, keep_stats: false })
    }

    /// A battle in progress: each side's brought Pokemon (export text, leads first).
    /// Starts at the move phase with no switch-in effects; then set HP, boosts, field...
    #[staticmethod]
    #[pyo3(signature = (dex, brought_a, brought_b, seed=1, turn=1))]
    fn from_brought(dex: &Dex, brought_a: &str, brought_b: &str, seed: u64, turn: u16) -> PyResult<Self> {
        let (a, b) = (parse_team(brought_a), parse_team(brought_b));
        let engine = Engine::from_brought(&dex.inner, [&a, &b], seed, turn).map_err(PyValueError::new_err)?;
        Ok(Battle { dex: dex.inner.clone(), b: engine, keep_stats: false })
    }

    /// (2026-11-03, v3 13-token net) The side's team members NOT brought (export text, team order; empty = none known):
    /// the v3 roster block shows ours exactly and, with open sheets, theirs. Positions made by team preview have it.
    fn set_roster(&mut self, side: usize, team: &str) -> PyResult<()> {
        let side = side.min(1);
        let t = parse_team(team);
        if t.is_empty() {
            self.b.roster[side] = None;
            return Ok(());
        }
        let mons = ::vgcsim::battle::build_team(&self.dex, &t).map_err(PyValueError::new_err)?;
        self.b.roster[side] = Some(Arc::new(mons));
        Ok(())
    }

    /// (2026-11-03) The six species the side showed at team preview (names or ids, team order): closed-sheet views
    /// list the ones not seen yet in the v3 roster block. Unknown names raise.
    fn set_preview(&mut self, side: usize, species: Vec<String>) -> PyResult<()> {
        let side = side.min(1);
        let sd = &mut self.b.sides[side];
        sd.preview = [0; 6];
        let mut n = 0;
        for name in species.iter().take(6) {
            let id: String = name.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
            let k = self.dex.species.iter().position(|sp| sp.id == id)
                .ok_or_else(|| PyValueError::new_err(format!("unknown species {name}")))?;
            sd.preview[n] = k as u16;
            n += 1;
        }
        sd.n_preview = n as u8;
        Ok(())
    }

    fn copy(&self) -> Self {
        Battle { dex: self.dex.clone(), b: self.b.clone(), keep_stats: self.keep_stats }
    }

    /// (2026-11-03, our types) Copies of this position in which side `side`'s unrevealed brought Pokemon (alive, benched,
    /// never on the field) are swapped with its roster (the two left at home), up to `k` distinct alternatives (each
    /// different from the true assignment). Returns (copies, n_types): n_types = how many assignments the other side can't
    /// tell apart (binomial(pool, slots)); 1 = nothing hidden (no copies). Needs set_roster(side, ...).
    #[pyo3(signature = (side, k=2, seed=1))]
    fn type_variants(&self, side: usize, k: usize, seed: u64) -> (Vec<Battle>, u64) {
        let side = side.min(1);
        let (n, pool) = encode::hidden_slots(&self.b, side);
        if n == 0 || pool <= n {
            return (vec![], 1);
        }
        let mut nt: u64 = 1;
        for i in 0..n as u64 {
            nt = nt * (pool as u64 - i) / (i + 1);
        }
        let key = |e: &Engine| -> Vec<u16> {
            let mut v: Vec<u16> = e.sides[side].mons.iter().filter(|m| m.alive()).map(|m| m.species).collect();
            v.sort_unstable();
            v
        };
        let mut seen = vec![key(&self.b)];
        let mut out = vec![];
        let mut r = Rng::new(seed ^ 0x0A7E_7195);
        for _ in 0..(8 * k + 8) {
            if out.len() >= k || seen.len() as u64 >= nt {
                break;
            }
            let mut e = self.b.clone();
            encode::shuffle_side_world(&mut e, &mut r, side);
            let kk = key(&e);
            if seen.contains(&kk) {
                continue;
            }
            seen.push(kk);
            out.push(Battle { dex: self.dex.clone(), b: e, keep_stats: self.keep_stats });
        }
        (out, nt)
    }

    /// (2026-10-27) The worlds the one-turn search plays in, as positions: OUR side is SIDE 0 in
    /// every copy (search_copy swaps when side = 1); world 0 = the opponent's stats guessed as the bridge does, the others
    /// sampled builds (views::sampled_view: stats only); with keep_foe_stats, plain copies.
    #[pyo3(signature = (side, worlds=4, seed=1, hidden=false))]
    fn search_worlds(&self, side: usize, worlds: usize, seed: u64, hidden: bool) -> Vec<Battle> {
        search_views(&self.b, &self.dex, side.min(1), worlds.max(1), seed, self.keep_stats, hidden)
            .into_iter()
            .map(|e| Battle { dex: self.dex.clone(), b: e, keep_stats: self.keep_stats })
            .collect()
    }

    /// (2026-10-21) see `keep_stats`: True for positions rebuilt from a real battle (their stats are the bridge's belief).
    fn set_keep_foe_stats(&mut self, keep: bool) {
        self.keep_stats = keep;
    }

    #[getter]
    fn keep_foe_stats(&self) -> bool {
        self.keep_stats
    }

    /// Reseed the battle's random generator (e.g. before each rollout from a copy).
    fn reseed(&mut self, seed: u64) {
        self.b.rng = Rng::new(seed);
    }

    #[getter]
    fn phase(&self) -> &'static str {
        match self.b.phase {
            Phase::TeamPreview => "preview",
            Phase::Turn => "turn",
            Phase::Switch => "switch",
            Phase::Ended => "ended",
        }
    }

    #[getter]
    fn winner(&self) -> Option<u8> {
        self.b.winner
    }

    #[getter]
    fn turn(&self) -> u16 {
        self.b.turn
    }

    fn choose_team(&mut self, side: usize, order: Vec<u8>) {
        self.b.choose_team(&self.dex, side, &order);
    }

    fn needs_choice(&self, side: usize) -> bool {
        self.b.needs_choice(side)
    }

    /// For each active slot, the legal actions (moves at each valid target, with and without Mega,
    /// and switches). Don't send both slots to the same bench Pokemon.
    fn legal_actions(&self, side: usize) -> Vec<Vec<ActionTuple>> {
        policy::legal_actions(&self.b, &self.dex, side).iter().map(|v| v.iter().map(|&a| to_tuple(a)).collect()).collect()
    }

    fn choose(&mut self, side: usize, a0: ActionTuple, a1: ActionTuple) -> PyResult<()> {
        let acts = [from_tuple(&a0)?, from_tuple(&a1)?];
        self.b.choose(&self.dex, side, acts);
        Ok(())
    }

    /// (2026-10-28, ndeep speed) a replacement request expanded in one call: every (our replacement x their replacement)
    /// pair (policy::replacement_joints, at most `cap` per side), returns (na, nb, children in a*nb+b order). A side that
    /// needs no choice has one option (pass). Replaces a Python loop of copy + choose + top_joint with uniform logits.
    #[pyo3(signature = (cap=16))]
    fn replacement_children(&self, cap: usize) -> (usize, usize, Vec<Battle>) {
        let dex = &self.dex;
        let mut jo = policy::replacement_joints(&self.b, dex, 0);
        let mut jt = policy::replacement_joints(&self.b, dex, 1);
        jo.truncate(cap.max(1));
        jt.truncate(cap.max(1));
        let mut out = Vec::with_capacity(jo.len() * jt.len());
        for a in &jo {
            for bb in &jt {
                let mut c = self.b.clone();
                if c.needs_choice(0) {
                    c.choose(dex, 0, *a);
                }
                if c.needs_choice(1) {
                    c.choose(dex, 1, *bb);
                }
                out.push(Battle { dex: self.dex.clone(), b: c, keep_stats: self.keep_stats });
            }
        }
        (jo.len(), jt.len(), out)
    }

    /// (2026-10-29, endgame rescoring) exact value of this position for side 0 (eg_exact; GIL released, so Python
    /// threads solve in parallel): (value or None, nodes, why, gap, depth turns). Uses the position as it is (no guessed
    /// views).
    /// (2026-10-30) eg_root: (solved, why, gap, depth, nodes, ours [(a0, a1)], theirs [(a0, a1)], lo, hi) for a live
    /// endgame decision (agent AGENT_ENDGAME); lo / hi = flat [len(ours) x len(theirs)] bounds of side 0's result
    #[pyo3(signature = (budget=20000, max_depth=4, max_leaves=64, iters=2000, seed=1, tol=0.02, seconds=0.0))]
    #[allow(clippy::type_complexity, clippy::too_many_arguments)]
    fn exact_root(&self, py: Python<'_>, budget: usize, max_depth: usize, max_leaves: usize, iters: usize, seed: u64,
                  tol: f64, seconds: f64) -> (bool, String, f64, usize, usize, Vec<(ActionTuple, ActionTuple)>, Vec<(ActionTuple, ActionTuple)>,
                                Vec<f64>, Vec<f64>) {
        let (b, dex) = (&self.b, &self.dex);
        let (ok, why, gap, dep, n, o, t, lo, hi) = py.detach(|| eg_root(b, dex, budget, max_depth, max_leaves, iters, seed, tol, seconds));
        let cv = |v: Vec<[Action; 2]>| v.into_iter().map(|j| (to_tuple(j[0]), to_tuple(j[1]))).collect::<Vec<_>>();
        (ok, why.to_string(), gap, dep, n, cv(o), cv(t), lo, hi)
    }

    #[pyo3(signature = (budget=20000, max_depth=4, max_leaves=64, iters=2000, seed=1, tol=0.02))]
    #[allow(clippy::too_many_arguments)]
    fn exact_value(&self, py: Python<'_>, budget: usize, max_depth: usize, max_leaves: usize, iters: usize, seed: u64,
                   tol: f64) -> (Option<f64>, usize, String, f64, usize) {
        let (b, dex) = (&self.b, &self.dex);
        let (v, n, why, gap, dep) = py.detach(|| eg_exact(b, dex, budget, max_depth, max_leaves, iters, seed, tol));
        (v, n, why.to_string(), gap, dep)
    }

    /// (2026-11-05, v3 external belief input) What `side`'s OPPONENT believes about `side`'s Pokemon `idx` (the brought
    /// four; with team=True the team member at that team-preview position): kind "exact" / "guess" / "none", ability,
    /// item, moves (names or ids), stats [hp, atk, def, spa, spd, spe]. A v3 network reads it as input; the game never does.
    #[pyo3(signature = (side, idx, kind, ability=None, item=None, moves=None, stats=None, team=false))]
    #[allow(clippy::too_many_arguments)]
    fn set_belief(&mut self, side: usize, idx: usize, kind: &str, ability: Option<&str>, item: Option<&str>, moves: Option<Vec<String>>,
                  stats: Option<Vec<u16>>, team: bool) -> PyResult<()> {
        let dex = self.dex.clone();
        let k = match kind {
            "exact" => 2,
            "set" => 3,
            "guess" => 1,
            "none" | "" => 0,
            _ => return Err(PyValueError::new_err("kind: exact (set + stats) / set (set known, stats guessed) / guess / none")),
        };
        let mut bs = ::vgcsim::state::BelSet { kind: k, ability: u16::MAX, item: u16::MAX, moves: [u16::MAX; 4], stats: [0; 6] };
        if let Some(a) = ability {
            bs.ability = dex.abilities.iter().position(|x| x.id == ::vgcsim::dex::to_id(a)).map(|x| x as u16).unwrap_or(u16::MAX);
        }
        if let Some(it) = item {
            bs.item = dex.items.iter().position(|x| x.id == ::vgcsim::dex::to_id(it)).map(|x| x as u16).unwrap_or(u16::MAX);
        }
        if let Some(mv) = moves {
            for (j, m) in mv.iter().take(4).enumerate() {
                bs.moves[j] = dex.moves.iter().position(|x| x.id == ::vgcsim::dex::to_id(m)).map(|x| x as u16).unwrap_or(u16::MAX);
            }
        }
        if let Some(st) = stats {
            for (j, v) in st.iter().take(6).enumerate() {
                bs.stats[j] = *v;
            }
        }
        if team {
            let t = self.b.teams.get_mut(side).and_then(|t| t.get_mut(idx)).ok_or_else(|| PyValueError::new_err("no such team member"))?;
            t.bel = bs;
        } else {
            self.check_mon(side, idx)?;
            self.b.sides[side].mons[idx].bel = bs;
        }
        Ok(())
    }

    /// The random policy's choice for a side (same policy as the Showdown comparison).
    fn random_actions(&mut self, side: usize, seed: u64) -> Vec<ActionTuple> {
        let mut rng = Rng::new(seed);
        policy::random_actions(&self.b, &self.dex, side, &mut rng).iter().map(|&a| to_tuple(a)).collect()
    }

    /// Play to the end with the random policy; returns the winner (0, 1, 2 = tie) or None.
    #[pyo3(signature = (seed, max_turns=100))]
    fn random_playout(&mut self, seed: u64, max_turns: u16) -> Option<u8> {
        let mut rng = Rng::new(seed);
        policy::random_playout(&mut self.b, &self.dex, &mut rng, max_turns)
    }

    /// `n` random playouts from copies of this position: (side 0 wins, side 1 wins, ties, unfinished).
    #[pyo3(signature = (n, seed, max_turns=100))]
    fn playouts(&self, py: Python<'_>, n: u32, seed: u64, max_turns: u16) -> (u32, u32, u32, u32) {
        let (b, dex) = (&self.b, &self.dex);
        py.detach(|| {
            let mut res = (0, 0, 0, 0);
            let mut rng = Rng::new(seed);
            for _ in 0..n {
                let mut c = b.clone();
                c.rng = Rng::new(rng.next_u32() as u64 | ((rng.next_u32() as u64) << 32));
                match policy::random_playout(&mut c, dex, &mut rng, max_turns) {
                    Some(0) => res.0 += 1,
                    Some(1) => res.1 += 1,
                    Some(_) => res.2 += 1,
                    None => res.3 += 1,
                }
            }
            res
        })
    }

    /// Values (for side 0) of each (our joint action, their joint action) after one turn, averaged
    /// over `samples` seeds: values[i][j]. Actions are pairs of action tuples (slot 0, slot 1).
    /// `position`: extra value per side-0 Pokemon index if it ends the turn active and alive.
    #[pyo3(signature = (ours, theirs, samples=4, seed=1, ko_bonus=0.5, position=None, self_weight=1.0))]
    fn payoff_matrix(
        &self, py: Python<'_>, ours: Vec<(ActionTuple, ActionTuple)>, theirs: Vec<(ActionTuple, ActionTuple)>,
        samples: u32, seed: u64, ko_bonus: f64, position: Option<Vec<f64>>, self_weight: f64,
    ) -> PyResult<Vec<Vec<f64>>> {
        let conv = |v: &Vec<(ActionTuple, ActionTuple)>| -> PyResult<Vec<[Action; 2]>> {
            v.iter().map(|(a, b)| Ok([from_tuple(a)?, from_tuple(b)?])).collect()
        };
        let (o, t) = (conv(&ours)?, conv(&theirs)?);
        let pos = position.unwrap_or_default();
        let (b, dex) = (&self.b, &self.dex);
        Ok(py.detach(|| policy::payoff_matrix(b, dex, &o, &t, samples, seed, ko_bonus, &pos, self_weight)))
    }

    /// The 16 damage rolls of move `slot` of Pokemon `idx` on `side` against Pokemon `target_idx` on `target_side`, in
    /// the current position, without side effects (the engine's preview_rolls); None for status moves, immunities,
    /// absorbing abilities, empty slots or out-of-range indices. For the game review's matchup notes.
    /// `state_bp` (default true): base powers computed from the battle state (Flail, Gyro Ball, Payback...).
    /// `spread`: True / False = the hit was / was not a spread hit (0.75x); None = as in this position.
    #[pyo3(signature = (side, idx, slot, target_side, target_idx, state_bp=true, spread=None))]
    fn damage_rolls(&self, side: usize, idx: usize, slot: usize, target_side: usize, target_idx: usize, state_bp: bool,
                    spread: Option<bool>) -> Option<Vec<u16>> {
        let (s, ts) = (side.min(1), target_side.min(1));
        if idx >= self.b.sides[s].mons.len() || target_idx >= self.b.sides[ts].mons.len() {
            return None;
        }
        self.b.preview_rolls_spread(&self.dex, s, idx, slot, ts, target_idx, state_bp, spread).map(|r| r.to_vec())
    }

    /// (2026-10-19) The engine's speed of Pokemon `idx` on `side` in the current position (stat, boosts, Tailwind, paralysis,
    /// Choice Scarf / Iron Ball, weather abilities...; Trick Room NOT applied: it only reverses the order). For inferring an
    /// opponent's Speed from move order (spread_infer.py).
    fn speed(&self, side: usize, idx: usize) -> PyResult<u32> {
        self.check_mon(side, idx)?;
        Ok(self.b.speed(&self.dex, side.min(1), idx))
    }

    /// For each active slot of `side`: {action index (the network's numbering): action tuple}
    /// for every legal action (pass only = {37: pass}).
    /// (2026-10-28, leaks) public=True: the actions as the OTHER side can know them (encode::public_legal: no Choice lock
    /// or Mega verdict from an unknown item, every target form for unrevealed move slots) -- for the opponent's candidates
    /// in a closed-sheet search of a true position.
    #[pyo3(signature = (side, public=false))]
    fn legal_index_map(&self, side: usize, public: bool) -> Vec<std::collections::HashMap<usize, ActionTuple>> {
        let la = if public { encode::public_legal(&self.dex, &self.b, side.min(1)) } else { policy::legal_actions(&self.b, &self.dex, side.min(1)) };
        la.iter().enumerate().map(|(user, slot)| {
            let mut m: std::collections::HashMap<usize, ActionTuple> = slot.iter().map(|&a| (encode::act_index(a, user), to_tuple(a))).collect();
            if m.is_empty() {
                m.insert(37, to_tuple(Action::Pass));
            }
            m
        }).collect()
    }

    /// One-turn search leaves: for every (our joint action, their joint action, world), play the turn
    /// from `side`'s point of view and return the resulting positions encoded for the network (as the
    /// harness does, from `side`'s view), in the order [ours][theirs][world]: (obs_f flat, obs_i flat,
    /// terminal) where terminal is +1 / -1 / 0 for a finished game (win / loss / tie for `side`) and NaN
    /// otherwise. World 0 = the opponent's stats guessed as the bridge does, the others = sampled plausible
    /// builds (`views::sampled_view`); each world has its own random rolls.
    #[pyo3(signature = (side, ours, theirs, worlds=16, seed=1, hidden=false))]
    #[allow(clippy::type_complexity)]
    fn search_leaves(&self, py: Python<'_>, side: usize, ours: Vec<(ActionTuple, ActionTuple)>, theirs: Vec<(ActionTuple, ActionTuple)>,
                     worlds: usize, seed: u64, hidden: bool) -> PyResult<(Vec<f32>, Vec<i32>, Vec<f32>)> {
        let conv = |v: &Vec<(ActionTuple, ActionTuple)>| -> PyResult<Vec<[Action; 2]>> {
            v.iter().map(|(a, b)| Ok([from_tuple(a)?, from_tuple(b)?])).collect()
        };
        let (o, t) = (conv(&ours)?, conv(&theirs)?);
        let worlds = worlds.max(1);
        let (b, dex) = (&self.b, &self.dex);
        let side = side.min(1);
        Ok(py.detach(|| {
            // our side becomes side 0 of the search positions (2026-10-28: the shared search_views, same worlds as before)
            let views = search_views(b, dex, side, worlds, seed, self.keep_stats, hidden);
            let n = o.len() * t.len() * worlds;
            let (nf, ni) = (encode::OBS_F, encode::OBS_I);
            let out: Vec<(Vec<f32>, Vec<i32>, f32)> = (0..n).into_par_iter().map(|k| {
                let w = k % worlds;
                let ti = (k / worlds) % t.len();
                let oi = k / (worlds * t.len());
                let mut c = views[w].clone();
                let s = seed.wrapping_add((w as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
                c.rng = Rng::new(s);
                let mut r2 = Rng::new(s ^ 0x5555);
                policy::play_turn(&mut c, dex, o[oi], t[ti], &mut r2);
                let mut f = vec![0f32; nf];
                let mut ids = vec![0i32; ni];
                let term = leaf_term(&c, dex);
                if !term.is_nan() {
                    return (f, ids, term);
                }
                encode::encode(dex, &c, 0, hidden, &mut f, &mut ids);
                (f, ids, f32::NAN)
            }).collect();
            let mut f = Vec::with_capacity(n * nf);
            let mut ids = Vec::with_capacity(n * ni);
            let mut term = Vec::with_capacity(n);
            for (a, bb, c) in out {
                f.extend(a);
                ids.extend(bb);
                term.push(c);
            }
            (f, ids, term)
        }))
    }


    /// (2026-10-28, speed pass 3c) nnsearch.top_joint in Rust: the k most likely legal joint actions of `side` from per-slot
    /// logits (`logits` = 2 x 38 float32, little-endian bytes), same rules and order as the Python version (softmax over
    /// each slot's legal indices, product, no double switch to one Pokemon, no double Mega, stable sort by probability;
    /// empty = the best single action of slot 0 + pass). `public` = the public legal map (hidden Choice lock / Mega).
    #[pyo3(signature = (side, logits, k, public=false))]
    fn top_joint(&self, side: usize, logits: &[u8], k: usize, public: bool) -> PyResult<Vec<(f64, ActionTuple, ActionTuple)>> {
        if logits.len() != 2 * 38 * 4 {
            return Err(PyValueError::new_err("top_joint: logits must be 2 x 38 float32"));
        }
        let side = side.min(1);
        let la = if public { encode::public_legal(&self.dex, &self.b, side) } else { policy::legal_actions(&self.b, &self.dex, side) };
        let mut slots: Vec<Vec<(Action, f64)>> = Vec::with_capacity(2);
        for user in 0..2 {
            // index -> action, the LAST action with that index wins (as the Python dict did), then by index
            let mut by_idx: [Option<Action>; 38] = [None; 38];
            for &a in la[user].iter() {
                by_idx[encode::act_index(a, user).min(37)] = Some(a);
            }
            let mut any = false;
            for o in by_idx.iter() {
                if o.is_some() { any = true; break; }
            }
            if !any {
                by_idx[37] = Some(Action::Pass);
            }
            let mut z: Vec<(Action, f64)> = Vec::new();
            let mut zmax = f64::NEG_INFINITY;
            for (idx, o) in by_idx.iter().enumerate() {
                if let Some(a) = o {
                    let off = (user * 38 + idx) * 4;
                    let l = f32::from_le_bytes([logits[off], logits[off + 1], logits[off + 2], logits[off + 3]]) as f64;
                    if l > zmax { zmax = l; }
                    z.push((*a, l));
                }
            }
            let mut tot = 0.0;
            for e in z.iter_mut() {
                e.1 = (e.1 - zmax).exp();
                tot += e.1;
            }
            for e in z.iter_mut() {
                e.1 /= tot;
            }
            slots.push(z);
        }
        let mut joint: Vec<(f64, Action, Action)> = Vec::new();
        for &(a0, p0) in slots[0].iter() {
            for &(a1, p1) in slots[1].iter() {
                match (a0, a1) {
                    (Action::Switch { to: x }, Action::Switch { to: y }) if x == y => continue,
                    (Action::Move { mega: true, .. }, Action::Move { mega: true, .. }) => continue,
                    _ => {}
                }
                joint.push((p0 * p1, a0, a1));
            }
        }
        if joint.is_empty() {
            let mut best = slots[0][0];
            for &e in slots[0].iter() {
                if e.1 > best.1 { best = e; }
            }
            joint.push((best.1, best.0, Action::Pass));
        }
        joint.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        joint.truncate(k.max(1));
        Ok(joint.into_iter().map(|(p, a, b)| (p, to_tuple(a), to_tuple(b))).collect())
    }

    /// (2026-10-28, speed pass 3c) search_leaves with the features returned as little-endian BYTES (np.frombuffer) instead of
    /// Python lists: the root halving search crossed ~66M floats per decision through list conversion.
    fn search_leaves_bytes(&self, py: Python<'_>, side: usize, ours: Vec<(ActionTuple, ActionTuple)>, theirs: Vec<(ActionTuple, ActionTuple)>,
                           worlds: usize, seed: u64, hidden: bool) -> PyResult<(Py<PyBytes>, Py<PyBytes>, Vec<f32>)> {
        let conv = |v: &Vec<(ActionTuple, ActionTuple)>| -> PyResult<Vec<[Action; 2]>> {
            v.iter().map(|(a, b)| Ok([from_tuple(a)?, from_tuple(b)?])).collect()
        };
        let (o, t) = (conv(&ours)?, conv(&theirs)?);
        let worlds = worlds.max(1);
        let (b, dex) = (&self.b, &self.dex);
        let side = side.min(1);
        let (fb, ib, term) = py.detach(|| {
            let views = search_views(b, dex, side, worlds, seed, self.keep_stats, hidden);
            let n = o.len() * t.len() * worlds;
            let (nf, ni) = (encode::OBS_F, encode::OBS_I);
            let mut fb = vec![0u8; n * nf * 4];
            let mut ib = vec![0u8; n * ni * 4];
            let mut term = vec![f32::NAN; n];
            fb.par_chunks_mut(nf * 4).zip(ib.par_chunks_mut(ni * 4)).zip(term.par_iter_mut()).enumerate()
                .for_each(|(k, ((fo, io), tm))| {
                    let w = k % worlds;
                    let ti = (k / worlds) % t.len();
                    let oi = k / (worlds * t.len());
                    let mut c = views[w].clone();
                    let s = seed.wrapping_add((w as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
                    c.rng = Rng::new(s);
                    let mut r2 = Rng::new(s ^ 0x5555);
                    policy::play_turn(&mut c, dex, o[oi], t[ti], &mut r2);
                    let term = leaf_term(&c, dex);
                    if !term.is_nan() {
                        *tm = term;
                        return;
                    }
                    let mut f = vec![0f32; nf];
                    let mut ids = vec![0i32; ni];
                    encode::encode(dex, &c, 0, hidden, &mut f, &mut ids);
                    for (d, x) in fo.chunks_exact_mut(4).zip(f.iter()) {
                        d.copy_from_slice(&x.to_le_bytes());
                    }
                    for (d, x) in io.chunks_exact_mut(4).zip(ids.iter()) {
                        d.copy_from_slice(&x.to_le_bytes());
                    }
                });
            (fb, ib, term)
        });
        Ok((PyBytes::new(py, &fb).unbind(), PyBytes::new(py, &ib).unbind(), term))
    }

    /// (2026-10-16, Ataraxos-style deeper search) The positions after one turn of every (ours, theirs, world) cell, as
    /// search_leaves plays them (our side = side 0), kept as a Rollouts batch the network can play further. Order
    /// [ours][theirs][world].
    #[pyo3(signature = (side, ours, theirs, worlds=4, seed=1, hidden=false))]
    fn search_children(&self, py: Python<'_>, side: usize, ours: Vec<(ActionTuple, ActionTuple)>,
                       theirs: Vec<(ActionTuple, ActionTuple)>, worlds: usize, seed: u64, hidden: bool) -> PyResult<Rollouts> {
        let (o, t) = (conv_joint(&ours)?, conv_joint(&theirs)?);
        let worlds = worlds.max(1);
        let (b, dex) = (&self.b, &self.dex);
        let side = side.min(1);
        let engines = py.detach(|| {
            let views = search_views(b, dex, side, worlds, seed, self.keep_stats, hidden);
            let n = o.len() * t.len() * worlds;
            (0..n).into_par_iter().map(|k| {
                let w = k % worlds;
                let ti = (k / worlds) % t.len();
                let oi = k / (worlds * t.len());
                let mut c = views[w].clone();
                let s = cell_seed(seed, w);
                c.rng = Rng::new(s);
                let mut r2 = Rng::new(s ^ 0x5555);
                policy::play_turn(&mut c, dex, o[oi], t[ti], &mut r2);
                c
            }).collect::<Vec<Engine>>()
        });
        Ok(Rollouts { dex: self.dex.clone(), engines })
    }

    /// (2026-10-13) Like search_leaves, but each (ours, theirs, world) cell's important random events (accuracy,
    /// critical hits, secondary effects, KO-deciding damage rolls, full paralysis, confusion, speed ties, random targets,
    /// multi-hit counts, mid-turn replacements) are ENUMERATED with their probabilities instead of drawn once
    /// (battle.rs ChanceCtl): outcome paths are explored most likely first while their probability is >= min_path
    /// (deeper events are drawn as usual), at most max_leaves per cell. Returns (obs_f flat, obs_i flat, terminal,
    /// weight, cell) per leaf; cell = (oi * len(theirs) + ti) * worlds + w; a cell's weights sum to the probability mass
    /// explored (1 unless max_leaves cut it short: normalise per cell).
    #[pyo3(signature = (side, ours, theirs, worlds=4, seed=1, hidden=false, min_path=0.05, max_leaves=32))]
    #[allow(clippy::type_complexity, clippy::too_many_arguments)]
    fn search_leaves_chance(&self, py: Python<'_>, side: usize, ours: Vec<(ActionTuple, ActionTuple)>,
                            theirs: Vec<(ActionTuple, ActionTuple)>, worlds: usize, seed: u64, hidden: bool,
                            min_path: f64, max_leaves: usize)
                            -> PyResult<(Py<PyBytes>, Py<PyBytes>, Vec<f32>, Vec<f32>, Vec<u32>)> {
        let (o, t) = (conv_joint(&ours)?, conv_joint(&theirs)?);
        let worlds = worlds.max(1);
        let (b, dex) = (&self.b, &self.dex);
        let side = side.min(1);
        let (f, ids, term, wt, cell) = py.detach(|| {
            let views = search_views(b, dex, side, worlds, seed, self.keep_stats, hidden);
            let n = o.len() * t.len() * worlds;
            let (nf, ni) = (encode::OBS_F, encode::OBS_I);
            let cells: Vec<Vec<(Vec<f32>, Vec<i32>, f32, f32)>> = (0..n).into_par_iter().map(|k| {
                let w = k % worlds;
                let ti = (k / worlds) % t.len();
                let oi = k / (worlds * t.len());
                enumerate_cell(&views[w], dex, o[oi], t[ti], cell_seed(seed, w), min_path, max_leaves, false).into_iter().map(|(c, wt)| {
                    let mut f = vec![0f32; nf];
                    let mut ids = vec![0i32; ni];
                    let term = leaf_term(&c, dex);
                    if !term.is_nan() {
                        return (f, ids, term, wt);
                    }
                    encode::encode(dex, &c, 0, hidden, &mut f, &mut ids);
                    (f, ids, f32::NAN, wt)
                }).collect()
            }).collect();
            let total: usize = cells.iter().map(|c| c.len()).sum();
            let mut f = Vec::with_capacity(total * nf);
            let mut ids = Vec::with_capacity(total * ni);
            let (mut term, mut wt, mut cell) = (Vec::with_capacity(total), Vec::with_capacity(total), Vec::with_capacity(total));
            for (k, c) in cells.into_iter().enumerate() {
                for (a, bb, tm, w) in c {
                    f.extend(a);
                    ids.extend(bb);
                    term.push(tm);
                    wt.push(w);
                    cell.push(k as u32);
                }
            }
            let fb: Vec<u8> = f.iter().flat_map(|x| x.to_le_bytes()).collect();
            let ib: Vec<u8> = ids.iter().flat_map(|x| x.to_le_bytes()).collect();
            (fb, ib, term, wt, cell)
        });
        // obs as raw little-endian bytes (np.frombuffer: no per-number Python objects)
        Ok((PyBytes::new(py, &f).unbind(), PyBytes::new(py, &ids).unbind(), term, wt, cell))
    }

    /// (2026-10-22) search_leaves_chance with end-of-turn replacements SOLVED instead of drawn as chance: a leaf that stops at
    /// a replacement request is expanded into every (our replacement x their replacement) pair (policy::replacement_joints),
    /// each its own row with the parent's weight. Returns (obs_f, obs_i, terminal, weight, cell, group, ga, gb): group = -1
    /// for an ordinary leaf, else an id unique within its cell; ga / gb = our / their option index in that group's game.
    /// The caller values the rows, solves each group's game and counts its value once with the parent's weight.
    #[pyo3(signature = (side, ours, theirs, worlds=4, seed=1, hidden=false, min_path=0.05, max_leaves=32))]
    #[allow(clippy::type_complexity, clippy::too_many_arguments)]
    fn search_leaves_chance_repl(&self, py: Python<'_>, side: usize, ours: Vec<(ActionTuple, ActionTuple)>,
                                 theirs: Vec<(ActionTuple, ActionTuple)>, worlds: usize, seed: u64, hidden: bool,
                                 min_path: f64, max_leaves: usize)
                                 -> PyResult<(Py<PyBytes>, Py<PyBytes>, Vec<f32>, Vec<f32>, Vec<u32>, Vec<i32>, Vec<u8>, Vec<u8>)> {
        let (o, t) = (conv_joint(&ours)?, conv_joint(&theirs)?);
        let worlds = worlds.max(1);
        let (b, dex) = (&self.b, &self.dex);
        let side = side.min(1);
        type Row = (Vec<f32>, Vec<i32>, f32, f32, i32, u8, u8);
        let (f, ids, term, wt, cell, grp, ga, gb) = py.detach(|| {
            let views = search_views(b, dex, side, worlds, seed, self.keep_stats, hidden);
            let n = o.len() * t.len() * worlds;
            let (nf, ni) = (encode::OBS_F, encode::OBS_I);
            let enc = |c: &Engine, wt: f32, g: i32, a: u8, bb: u8| -> Row {
                let mut f = vec![0f32; nf];
                let mut ids = vec![0i32; ni];
                let term = leaf_term(c, dex);
                if !term.is_nan() {
                    return (f, ids, term, wt, g, a, bb);
                }
                encode::encode(dex, c, 0, hidden, &mut f, &mut ids);
                (f, ids, f32::NAN, wt, g, a, bb)
            };
            let cells: Vec<Vec<Row>> = (0..n).into_par_iter().map(|k| {
                let w = k % worlds;
                let ti = (k / worlds) % t.len();
                let oi = k / (worlds * t.len());
                let mut rows = Vec::new();
                let mut g = 0i32;
                for (c, wt) in enumerate_cell(&views[w], dex, o[oi], t[ti], cell_seed(seed, w), min_path, max_leaves, true) {
                    if c.phase == Phase::Switch && c.winner.is_none() {
                        let jo = policy::replacement_joints(&c, dex, 0);
                        let jt = policy::replacement_joints(&c, dex, 1);
                        for (ai, a) in jo.iter().enumerate() {
                            for (bi, bb) in jt.iter().enumerate() {
                                let mut c2 = c.clone();
                                if c2.needs_choice(0) {
                                    c2.choose(dex, 0, *a);
                                }
                                if c2.needs_choice(1) {
                                    c2.choose(dex, 1, *bb);
                                }
                                rows.push(enc(&c2, wt, g, ai.min(255) as u8, bi.min(255) as u8));
                            }
                        }
                        g += 1;
                    } else {
                        rows.push(enc(&c, wt, -1, 0, 0));
                    }
                }
                rows
            }).collect();
            let total: usize = cells.iter().map(|c| c.len()).sum();
            let mut f = Vec::with_capacity(total * nf);
            let mut ids = Vec::with_capacity(total * ni);
            let (mut term, mut wt, mut cell) = (Vec::with_capacity(total), Vec::with_capacity(total), Vec::with_capacity(total));
            let (mut grp, mut ga, mut gb) = (Vec::with_capacity(total), Vec::with_capacity(total), Vec::with_capacity(total));
            for (k, c) in cells.into_iter().enumerate() {
                for (a, bb, tm, w, g, x, y) in c {
                    f.extend(a);
                    ids.extend(bb);
                    term.push(tm);
                    wt.push(w);
                    cell.push(k as u32);
                    grp.push(g);
                    ga.push(x);
                    gb.push(y);
                }
            }
            let fb: Vec<u8> = f.iter().flat_map(|x| x.to_le_bytes()).collect();
            let ib: Vec<u8> = ids.iter().flat_map(|x| x.to_le_bytes()).collect();
            (fb, ib, term, wt, cell, grp, ga, gb)
        });
        Ok((PyBytes::new(py, &f).unbind(), PyBytes::new(py, &ids).unbind(), term, wt, cell, grp, ga, gb))
    }

    /// (2026-10-13) The leaf POSITIONS of chosen cells of search_leaves_chance (same seed / worlds / cut-offs = the same
    /// leaves in the same order), for deepening the search there: per requested (oi, ti, w) a list of (Battle, weight).
    /// The positions are from the search's view: `side` is side 0 in them.
    /// (2026-10-22) repl=True: leaves stop at end-of-turn replacements (phase 'switch'), the caller solves them.
    #[pyo3(signature = (side, ours, theirs, cells, worlds=4, seed=1, min_path=0.05, max_leaves=32, repl=false, hidden=false))]
    #[allow(clippy::type_complexity, clippy::too_many_arguments)]
    fn chance_leaf_battles(&self, py: Python<'_>, side: usize, ours: Vec<(ActionTuple, ActionTuple)>,
                           theirs: Vec<(ActionTuple, ActionTuple)>, cells: Vec<(usize, usize, usize)>, worlds: usize,
                           seed: u64, min_path: f64, max_leaves: usize, repl: bool, hidden: bool) -> PyResult<Vec<Vec<(Battle, f32)>>> {
        let (o, t) = (conv_joint(&ours)?, conv_joint(&theirs)?);
        let worlds = worlds.max(1);
        let (b, dex) = (&self.b, &self.dex);
        let side = side.min(1);
        for &(oi, ti, w) in &cells {
            if oi >= o.len() || ti >= t.len() || w >= worlds {
                return Err(PyValueError::new_err("cell out of range"));
            }
        }
        let out: Vec<Vec<(Engine, f32)>> = py.detach(|| {
            let views = search_views(b, dex, side, worlds, seed, self.keep_stats, hidden);
            cells.par_iter().map(|&(oi, ti, w)| enumerate_cell(&views[w], dex, o[oi], t[ti], cell_seed(seed, w), min_path, max_leaves, repl))
                .collect()
        });
        Ok(out.into_iter().map(|v| v.into_iter().map(|(e, wt)| (Battle { dex: self.dex.clone(), b: e, keep_stats: self.keep_stats }, wt)).collect()).collect())
    }

    /// The whole state as plain Python data.
    fn state<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let dex = &self.dex;
        let d = PyDict::new(py);
        d.set_item("turn", self.b.turn)?;
        d.set_item("phase", self.phase())?;
        d.set_item("winner", self.b.winner)?;
        let f = PyDict::new(py);
        f.set_item("weather", weather_name(self.b.field.weather))?;
        f.set_item("weather_turns", self.b.field.weather_turns)?;
        f.set_item("terrain", terrain_name(self.b.field.terrain))?;
        f.set_item("terrain_turns", self.b.field.terrain_turns)?;
        f.set_item("trick_room", self.b.field.trick_room)?;
        f.set_item("gravity", self.b.field.gravity)?;
        f.set_item("magic_room", self.b.field.magic_room)?;
        f.set_item("wonder_room", self.b.field.wonder_room)?;
        f.set_item("fairy_lock", self.b.field.fairy_lock)?;
        let lm = self.b.last_any_move;
        f.set_item("last_move", if (lm as usize) < dex.moves.len() { Some(dex.moves[lm as usize].id.as_str()) } else { None })?;
        d.set_item("field", f)?;
        let sides = PyList::empty(py);
        for s in 0..2 {
            let sd = &self.b.sides[s];
            let sdict = PyDict::new(py);
            let active: Vec<Option<u8>> = sd.active.iter().map(|&x| if x == NONE { None } else { Some(x) }).collect();
            sdict.set_item("active", active)?;
            sdict.set_item("tailwind", sd.tailwind)?;
            sdict.set_item("reflect", sd.reflect)?;
            sdict.set_item("light_screen", sd.light_screen)?;
            sdict.set_item("aurora_veil", sd.aurora_veil)?;
            sdict.set_item("mega_used", sd.mega_used)?;
            sdict.set_item("switch_needed", self.b.switch_needed[s].to_vec())?;
            sdict.set_item("reviving", self.b.reviving[s].to_vec())?;
            // (2026-10-18) entry hazards (for rebuilding a position as the Showdown bridge sees it)
            sdict.set_item("stealth_rock", sd.stealth_rock)?;
            sdict.set_item("spikes", sd.spikes)?;
            sdict.set_item("toxic_spikes", sd.toxic_spikes)?;
            sdict.set_item("sticky_web", sd.sticky_web)?;
            sdict.set_item("safeguard", sd.safeguard)?;
            sdict.set_item("wish", sd.wish.iter().map(|w| (w.0, w.1)).collect::<Vec<_>>())?;
            sdict.set_item("future", sd.future.iter().map(|w| (w.0, w.1, w.2)).collect::<Vec<_>>())?;
            sdict.set_item("healing_wish", sd.healing_wish.to_vec())?;
            sdict.set_item("shed_sub", sd.shed_sub.to_vec())?;
            sdict.set_item("fainted_total", sd.fainted_total)?;
            let mons = PyList::empty(py);
            for (mi, m) in sd.mons.iter().enumerate() {
                let md = PyDict::new(py);
                md.set_item("species", &dex.species[m.species as usize].name)?;
                // (2026-10-18, bridge) the team set's species, also while Transformed (species = the copied one)
                let set_sp = match &sd.transformed { Some(t) if t.mon as usize == mi => t.species, _ => m.species };
                md.set_item("set_species", &dex.species[set_sp as usize].name)?;
                md.set_item("hp", m.hp)?;
                md.set_item("maxhp", m.maxhp)?;
                md.set_item("status", status_name(m.status))?;
                md.set_item("fainted", m.fainted)?;
                md.set_item("ability", &dex.abilities[m.ability_id as usize].id)?;
                md.set_item("base_ability", &dex.abilities[m.base_ability_id as usize].id)?;
                md.set_item("item", if m.item_id == u16::MAX { "" } else { dex.items[m.item_id as usize].id.as_str() })?;
                // Magic Room / Klutz: the item the Pokemon still holds while it is hidden ("" = none)
                md.set_item("stash_item", if m.stash_item.0 == u16::MAX { "" } else { dex.items[m.stash_item.0 as usize].id.as_str() })?;
                // (2026-10-18 bridge fuzz) the item it used / ate last (Showdown lastItem: Recycle, Harvest, Pickup)
                md.set_item("last_item", if (m.last_item as usize) < dex.items.len() { dex.items[m.last_item as usize].id.as_str() } else { "" })?;
                let stats = PyDict::new(py);
                for (k, n) in STAT_IDS.iter().enumerate() {
                    stats.set_item(n, m.stats[k])?;
                }
                md.set_item("stats", stats)?;
                let boosts = PyDict::new(py);
                for (k, n) in BOOST_IDS.iter().enumerate().skip(1) {
                    boosts.set_item(n, m.boosts[k])?;
                }
                md.set_item("boosts", boosts)?;
                let moves: Vec<(String, u8)> = (0..m.n_moves as usize).map(|k| (dex.moves[m.moves[k] as usize].id.clone(), m.pp[k])).collect();
                md.set_item("moves", moves)?;
                md.set_item("choice_lock", if m.choice_lock != NONE && (m.choice_lock as usize) < 4 && m.moves[m.choice_lock as usize] != u16::MAX {
                    dex.moves[m.moves[m.choice_lock as usize] as usize].id.as_str() } else { "" })?;
                md.set_item("is_mega", m.is_mega)?;
                md.set_item("can_mega", m.mega_species != u16::MAX && !sd.mega_used)?;
                // (2026-10-18) public knowledge + the volatiles the Showdown bridge rebuilds:
                // revealed bits (0-3 move slots used, 4 item shown, 5 seen, 6 ability, 7 explicit), item / ability known
                // to an observer (views::item_known / ability_known, as in the hidden network view), Protect streak,
                // moved since switching in, Unburden active, the history inputs
                md.set_item("rev", m.rev)?;
                md.set_item("item_known", ::vgcsim::views::item_known(dex, m))?;
                md.set_item("ability_known", ::vgcsim::views::ability_known(dex, m))?;
                md.set_item("protect_streak", m.protect_streak)?;
                md.set_item("moved", m.active_move_actions > 0)?;
                md.set_item("unburden", m.vol & vol::UNBURDEN != 0)?;
                let h = PyDict::new(py);
                h.set_item("slot", if m.h_slot == NONE { -1 } else { m.h_slot as i32 })?;
                h.set_item("target", m.h_target as i32)?;
                h.set_item("streak", m.h_streak as i32)?;
                h.set_item("protect", m.h_protect as i32)?;
                h.set_item("result", m.last_move_result as i32)?;
                for k in 0..4 {
                    h.set_item(format!("uses{k}"), m.h_uses[k] as i32)?;
                }
                h.set_item("protects", m.h_protects as i32)?;
                h.set_item("status", m.h_status as i32)?;
                md.set_item("history", h)?;
                let vd = PyDict::new(py);
                for (n, v) in vols_of(m) {
                    vd.set_item(n, v)?;
                }
                md.set_item("vols", vd)?;
                let tn = |t: u8| if t == ::vgcsim::dex::NO_TYPE { "???".to_string() } else { dex.type_names[t as usize].clone() };
                md.set_item("types", m.types.iter().filter(|&&t| t != ::vgcsim::dex::NO_TYPE).map(|&t| tn(t)).collect::<Vec<_>>())?;
                md.set_item("gender", match m.gender { 1 => "M", 2 => "F", _ => "" })?;
                md.set_item("added_type", if m.added_type == ::vgcsim::dex::NO_TYPE { String::new() } else { tn(m.added_type) })?;
                mons.append(md)?;
            }
            sdict.set_item("mons", mons)?;
            sides.append(sdict)?;
        }
        d.set_item("sides", sides)?;
        Ok(d)
    }

    /// Change one Pokemon (for matching a real position). `boosts`: dict like {"atk": -1}.
    /// `revealed`: what the opponent knows about this Pokemon (hidden-information features): bits 0-3 move
    /// slots, 4 item, 5 seen, 6 ability (set from a real battle: these bits are then taken as they are).
    #[pyo3(signature = (side, idx, hp=None, status=None, boosts=None, item=None, fainted=None, mega=None, first_turn=None, protect_streak=None, stats=None, revealed=None, history=None, unburden=None, choice_lock=None, pp_used=None, status_turns=None, sleep_elapsed=None))]
    #[allow(clippy::too_many_arguments)]
    fn set_mon(
        &mut self, side: usize, idx: usize, hp: Option<u16>, status: Option<&str>, boosts: Option<std::collections::HashMap<String, i8>>,
        item: Option<&str>, fainted: Option<bool>, mega: Option<bool>, first_turn: Option<bool>, protect_streak: Option<u8>,
        stats: Option<std::collections::HashMap<String, u16>>, revealed: Option<u8>,
        history: Option<std::collections::HashMap<String, i32>>, unburden: Option<bool>, choice_lock: Option<&str>,
        pp_used: Option<Vec<u8>>, status_turns: Option<u8>, sleep_elapsed: Option<u8>,
    ) -> PyResult<()> {
        self.check_mon(side, idx)?;
        if let Some(used) = &pp_used {
            // (2026-10-17) PP spent per move slot (a rebuilt position started every move at full PP)
            let dex = self.dex.clone();
            let m = &mut self.b.sides[side].mons[idx];
            for (k, u) in used.iter().enumerate().take(m.n_moves as usize) {
                let max = dex.moves[m.moves[k] as usize].max_pp;
                m.pp[k] = max.saturating_sub(*u);
            }
        }
        if let Some(u) = unburden {
            // (2026-10-14) Unburden active: the item was lost since this Pokemon last switched in (a rebuilt position
            // can't know it from item="" alone)
            let m = &mut self.b.sides[side].mons[idx];
            if u { m.vol |= vol::UNBURDEN; } else { m.vol &= !vol::UNBURDEN; }
        }
        if let Some(h) = &history {
            // the network's history inputs (see Mon::h_slot), from a real battle's log: slot (-1 = none since it came
            // in), target (1 / 2 = opposing slot a / b, 3 = ally, 0 = itself / none / spread), streak, protect (0/1), result (last turn: -1 none, 0 failed, 1 worked),
            // uses0..uses3, protects, status
            let m = &mut self.b.sides[side].mons[idx];
            let g = |k: &str| h.get(k).copied();
            if let Some(v) = g("slot") {
                m.h_slot = if (0..4).contains(&v) { v as u8 } else { NONE };
                // (2026-10-18 bridge fuzz) the same chosen move is Showdown's lastMove: Torment, Gigaton Hammer, Blood Moon
                m.last_move = if (0..4).contains(&v) { m.moves[v as usize] } else { u16::MAX };
            }
            if let Some(v) = g("target") { m.h_target = v as i8; }
            // (2026-10-19 bridge flag 19724) Showdown lastMoveTargetLoc (engine encoding: + foe slot + 1, - own slot + 1, 0 none):
            // Instruct re-aims the last move there (an instructed High Horsepower went at a foe instead of the user's ally)
            if let Some(v) = g("loc") { m.last_target = v.clamp(-2, 2) as i8; }
            if let Some(v) = g("streak") { m.h_streak = v.clamp(0, 255) as u8; }
            if let Some(v) = g("protect") { m.h_protect = v != 0; }
            if let Some(v) = g("result") { m.last_move_result = v.clamp(-1, 1) as i8; m.last_move_failed = v == 0; }
            // (2026-10-18) a move stopped in BeforeMove: failed for Stomping Tantrum, but no result for the network
            if let Some(v) = g("failed") { m.last_move_failed = v != 0; }
            for k in 0..4 {
                if let Some(v) = g(&format!("uses{k}")) { m.h_uses[k] = v.clamp(0, 255) as u8; }
            }
            if let Some(v) = g("protects") { m.h_protects = v.clamp(0, 255) as u8; }
            if let Some(v) = g("status") { m.h_status = v.clamp(0, 255) as u8; }
        }
        if mega == Some(true) {
            self.b.set_mega_form(&self.dex, side, idx);
        }
        let dex = self.dex.clone();
        let was_fainted = self.b.sides[side].mons[idx].fainted;
        let m = &mut self.b.sides[side].mons[idx];
        // exact stats (e.g. from the battle request, or a guess): "hp" sets max HP
        if let Some(st) = stats {
            for (k, v) in st {
                if let Some(pos) = STAT_IDS.iter().position(|n| *n == k) {
                    m.stats[pos] = v;
                    if pos == 0 {
                        m.maxhp = v;
                        m.hp = m.hp.min(v);
                    }
                }
            }
        }
        if let Some(h) = hp {
            m.hp = h.min(m.maxhp);
            if m.hp == 0 {
                m.fainted = true;
            }
        }
        if let Some(st) = status {
            m.status = parse_status(st)?;
            // (2026-10-18, bridge check) the status's own counter: sleep = turns left, freeze = Champions' 3-turn cap
            // left, toxic = residual stages so far. A rebuilt position used to reset them (sleep always 2 left, freeze 0 =
            // thawed at the first move, toxic back to 1/16). `status_turns` = the exact value (Rest's 3-turn sleep,
            // freeze, toxic); `sleep_elapsed` = sleep turns already counted down of an UNKNOWN 2-3 turn sleep: the engine
            // draws the length at the next BeforeMove, given that it lasted that long (0x80 | elapsed until then).
            m.status_turns = match m.status {
                Status::Sleep => 2,
                Status::Freeze => 3,
                _ => 0,
            };
        }
        if let Some(t) = status_turns {
            m.status_turns = t;
        } else if let Some(e) = sleep_elapsed {
            if m.status == Status::Sleep {
                m.status_turns = 0x80 | e.min(0x7f);
            }
        }
        if let Some(b) = boosts {
            for (k, v) in b {
                if let Some(pos) = BOOST_IDS.iter().position(|n| *n == k) {
                    if pos > 0 {
                        m.boosts[pos] = v.clamp(-6, 6);
                    }
                }
            }
        }
        if let Some(it) = item {
            if it.is_empty() {
                m.item_id = u16::MAX;
                m.item = ::vgcsim::effects::It::None;
            } else {
                let i = dex.item_index(it).ok_or_else(|| PyValueError::new_err(format!("unknown item {it}")))?;
                m.item_id = i as u16;
                m.item = dex.items[i].kind;
            }
        }
        if let Some(f) = fainted {
            m.fainted = f;
            if f {
                m.hp = 0;
            }
        }
        if let Some(ft) = first_turn {
            // Fake Out works only before the Pokemon's first move since switching in
            m.active_move_actions = if ft { 0 } else { 1 };
        }
        if let Some(p) = protect_streak {
            m.protect_streak = p;
        }
        if let Some(r) = revealed {
            m.rev = (r & 0x7f) | ::vgcsim::state::rev::EXPLICIT;
        }
        if let Some(cl) = choice_lock {
            // (2026-10-14) Choice item lock from a real battle's log: the move this Pokemon is locked into ("" = none).
            // A rebuilt position can't know it otherwise (the net reads it, and the search must not give a locked
            // Pokemon other moves). Only a Choice item holder is locked (in a
            // sampled world the opponent may hold something else); a move it doesn't have leaves it unlocked.
            m.choice_lock = NONE;
            let choice = matches!(m.item, ::vgcsim::effects::It::ChoiceBand | ::vgcsim::effects::It::ChoiceSpecs | ::vgcsim::effects::It::ChoiceScarf);
            if !cl.is_empty() && choice {
                if let Some(id) = dex.move_index(cl) {
                    if let Some(k) = m.moves.iter().position(|&x| x != u16::MAX && x as usize == id) {
                        m.choice_lock = k as u8;
                    }
                }
            }
        }
        // a Pokemon marked fainted counts for Last Respects / Supreme Overlord (the side's faint counter), as in a real game
        if !was_fainted && self.b.sides[side].mons[idx].fainted {
            self.b.sides[side].fainted_total = self.b.sides[side].fainted_total.saturating_add(1);
        }
        Ok(())
    }

    /// Put Pokemon `idx` in active slot `slot` (None = empty slot).
    /// (2026-10-18) Current types (type changes: Burn Up / Double Shock, Soak, Reflect Type, Protean...; [] or ["???"] =
    /// typeless) and the added third type (Forest's Curse / Trick-or-Treat; "" = none).
    #[pyo3(signature = (side, idx, types, added=None))]
    fn set_types(&mut self, side: usize, idx: usize, types: Vec<String>, added: Option<String>) -> PyResult<()> {
        self.check_mon(side, idx)?;
        let dex = self.dex.clone();
        let id = |n: &str| -> PyResult<u8> {
            if n == "???" || n.is_empty() { return Ok(::vgcsim::dex::NO_TYPE); }
            dex.type_names.iter().position(|x| x.eq_ignore_ascii_case(n)).map(|k| k as u8)
                .ok_or_else(|| PyValueError::new_err(format!("unknown type {n}")))
        };
        let mut t = [::vgcsim::dex::NO_TYPE; 2];
        for (k, n) in types.iter().take(2).enumerate() {
            t[k] = id(n)?;
        }
        if t[0] == ::vgcsim::dex::NO_TYPE && t[1] != ::vgcsim::dex::NO_TYPE {
            t.swap(0, 1);
        }
        let m = &mut self.b.sides[side].mons[idx];
        m.types = t;
        if let Some(a) = added {
            m.added_type = id(&a)?;
        }
        Ok(())
    }

    /// (2026-10-18, bridge) (side, idx) Transformed into (tside, tidx) earlier in the battle: species, types, stats, moves,
    /// ability (not started again), weight; current boosts kept; pp_used = PP spent on the copied moves (of 5).
    /// pre_mega: the target Mega Evolved after the Transform (copy its base form).
    #[pyo3(signature = (side, idx, tside, tidx, pp_used=None, pre_mega=false))]
    fn transform(&mut self, side: usize, idx: usize, tside: usize, tidx: usize, pp_used: Option<Vec<u8>>, pre_mega: bool) -> PyResult<bool> {
        self.check_mon(side, idx)?;
        self.check_mon(tside, tidx)?;
        let mut pu = [0u8; 4];
        for (k, x) in pp_used.unwrap_or_default().into_iter().take(4).enumerate() {
            pu[k] = x;
        }
        let dex = self.dex.clone();
        Ok(self.b.bridge_transform(&dex, side, idx, tside, tidx, pu, pre_mega))
    }

    /// (2026-10-18) Set volatile counters / flags by name (see vol_fields!, VOL_BITS; engine convention), e.g.
    /// {"taunt": 2, "leechseed": 1, "leech_slot": 0, "leech_side": 1}. Unknown names raise.
    fn set_vols(&mut self, side: usize, idx: usize, vols: std::collections::HashMap<String, i64>) -> PyResult<()> {
        self.check_mon(side, idx)?;
        let m = &mut self.b.sides[side].mons[idx];
        let gastro_before = m.vol2 & ::vgcsim::state::vol2::GASTRO != 0;
        for (k, v) in vols {
            if !set_vol(m, &k, v) {
                return Err(PyValueError::new_err(format!("unknown volatile {k}")));
            }
        }
        // (2026-10-19 bridge flag 21812) Gastro Acid suppresses the ability (the engine's move sets ability = None, keeping the
        // id): the flag alone left a Gastro-Acided Falinks' Defiant working
        let gastro = m.vol2 & ::vgcsim::state::vol2::GASTRO != 0;
        if gastro {
            m.ability = ::vgcsim::effects::Ab::None;
        } else if gastro_before {
            m.ability = self.dex.abilities[m.ability_id as usize].kind;
        }
        Ok(())
    }

    /// (2026-10-18) Side / field timers the bridge can rebuild: safeguard turns, wish (turns, hp) per slot, future
    /// attack (turns, source mon, move-ish payload as stored) per slot, gravity / magic room / wonder room / fairy lock.
    /// healing_wish: per slot, a Healing Wish waiting for a Pokemon that needs it (2026-10-18).
    #[pyo3(signature = (side, safeguard=None, wish=None, future=None, gravity=None, magic_room=None, wonder_room=None, healing_wish=None, fairy_lock=None))]
    #[allow(clippy::too_many_arguments)]
    fn set_extra(&mut self, side: usize, safeguard: Option<u8>, wish: Option<Vec<(u8, u16)>>, future: Option<Vec<(u8, u8, u8)>>,
                 gravity: Option<u8>, magic_room: Option<u8>, wonder_room: Option<u8>, healing_wish: Option<Vec<bool>>,
                 fairy_lock: Option<u8>) -> PyResult<()> {
        let sd = &mut self.b.sides[side];
        if let Some(h) = healing_wish { for (k, x) in h.into_iter().take(2).enumerate() { sd.healing_wish[k] = x; } }
        if let Some(v) = safeguard { sd.safeguard = v; }
        if let Some(w) = wish { for (k, x) in w.into_iter().take(2).enumerate() { sd.wish[k] = x; } }
        if let Some(w) = future { for (k, x) in w.into_iter().take(2).enumerate() { sd.future[k] = x; } }
        if let Some(v) = gravity { self.b.field.gravity = v; }
        if let Some(v) = magic_room { self.b.field.magic_room = v; }
        if let Some(v) = wonder_room { self.b.field.wonder_room = v; }
        if let Some(v) = fairy_lock { self.b.field.fairy_lock = v; }      // (2026-10-18) nobody can switch while > 0
        Ok(())
    }

    /// (2026-10-20) an observer's estimate of this Pokemon's stats [HP, Atk, Def, SpA, SpD, Spe] (spread inference) for the
    /// network's damage features (guessed views use it instead of the generic guess); None = no estimate.
    #[pyo3(signature = (side, idx, stats=None))]
    fn set_stat_belief(&mut self, side: usize, idx: usize, stats: Option<[u16; 6]>) -> PyResult<()> {
        self.check_mon(side, idx)?;
        self.b.sides[side].mons[idx].stat_belief = stats.unwrap_or([0; 6]);
        Ok(())
    }

    /// (2026-10-18 bridge fuzz) the item this Pokemon used / ate last (Showdown lastItem: what Recycle restores); None / "" = none.
    #[pyo3(signature = (side, idx, item=None))]
    fn set_last_item(&mut self, side: usize, idx: usize, item: Option<&str>) -> PyResult<()> {
        self.check_mon(side, idx)?;
        let id = match item {
            Some(it) if !it.is_empty() => self.dex.item_index(it)
                .ok_or_else(|| pyo3::exceptions::PyValueError::new_err(format!("unknown item {it}")))? as u16,
            _ => u16::MAX,
        };
        self.b.sides[side].mons[idx].last_item = id;
        Ok(())
    }

    /// (2026-10-18 bridge fuzz) the CURRENT ability, changed by a move earlier in the stint (Entrainment, Skill Swap, Role Play,
    /// Worry Seed, Simple Beam, Doodle...): the team set keeps the base ability, which comes back at switch-out. Silent (no
    /// start effects: they already happened).
    fn set_ability(&mut self, side: usize, idx: usize, ability: &str) -> PyResult<()> {
        self.check_mon(side, idx)?;
        let id = self.dex.ability_index(ability)
            .ok_or_else(|| pyo3::exceptions::PyValueError::new_err(format!("unknown ability {ability}")))?;
        let kind = self.dex.abilities[id].kind;
        let m = &mut self.b.sides[side].mons[idx];
        m.ability_id = id as u16;
        // under Gastro Acid the (new) ability stays suppressed (2026-10-19)
        m.ability = if m.vol2 & ::vgcsim::state::vol2::GASTRO != 0 { ::vgcsim::effects::Ab::None } else { kind };
        Ok(())
    }

    /// (2026-10-19 bridge fuzz) the dex index of a move id (None = unknown): for set_vols' "call_move".
    fn move_index(&self, move_id: &str) -> Option<usize> {
        self.dex.move_index(move_id)
    }

    /// (2026-10-18 bridge fuzz) the last move anyone used (Showdown's battle.lastMove: what Copycat copies); None / "" = none.
    #[pyo3(signature = (move_id=None))]
    fn set_last_move(&mut self, move_id: Option<&str>) -> PyResult<()> {
        self.b.last_any_move = match move_id {
            Some(id) if !id.is_empty() => self.dex.move_index(id)
                .ok_or_else(|| pyo3::exceptions::PyValueError::new_err(format!("unknown move {id}")))? as u16,
            _ => u16::MAX,
        };
        Ok(())
    }

    /// (2026-10-19) Revival Blessing's request: `slot`'s Pokemon used it and now picks a fainted Pokemon (legal_actions lists
    /// them as switches). False if that slot is empty or nothing has fainted. Call after refresh() (which clears requests).
    fn set_reviving(&mut self, side: usize, slot: usize) -> PyResult<bool> {
        if side > 1 || slot > 1 {
            return Err(PyValueError::new_err("side / slot out of range"));
        }
        Ok(self.b.set_reviving(side, slot))
    }

    /// (2026-10-19) a mid-turn switch-out request for `slot`'s living Pokemon (U-turn, Eject Button...; see Battle::set_switching).
    /// Call after refresh(). False if there is nobody to switch in.
    #[pyo3(signature = (side, slot, baton=false, shed_tail=false))]
    fn set_switching(&mut self, side: usize, slot: usize, baton: bool, shed_tail: bool) -> PyResult<bool> {
        if side > 1 || slot > 1 {
            return Err(PyValueError::new_err("side / slot out of range"));
        }
        Ok(self.b.set_switching(side, slot, baton, shed_tail))
    }

    /// (2026-10-22) after set_switching: a move still to come this turn for the Pokemon in (side, slot), ("move", mslot,
    /// target, mega) as in choose(); it runs after the switch-in. False if refused (nobody there, no such move, already queued).
    fn queue_move(&mut self, side: usize, slot: usize, action: ActionTuple) -> PyResult<bool> {
        if side > 1 || slot > 1 {
            return Err(PyValueError::new_err("side / slot out of range"));
        }
        match from_tuple(&action)? {
            Action::Move { slot: ms, target, .. } => Ok(self.b.queue_move(&self.dex, side, slot, ms, target)),
            _ => Ok(false),
        }
    }

    fn set_active(&mut self, side: usize, slot: usize, idx: Option<usize>) -> PyResult<()> {
        if let Some(i) = idx {
            self.check_mon(side, i)?;
        }
        self.b.sides[side].active[slot.min(1)] = idx.map(|i| i as u8).unwrap_or(NONE);
        if let Some(i) = idx {
            self.b.sides[side].party_put(slot.min(1), i);
        }
        Ok(())
    }

    #[pyo3(signature = (weather=None, weather_turns=None, terrain=None, terrain_turns=None, trick_room=None))]
    fn set_field(&mut self, weather: Option<&str>, weather_turns: Option<u8>, terrain: Option<&str>, terrain_turns: Option<u8>, trick_room: Option<u8>) -> PyResult<()> {
        let f = &mut self.b.field;
        if let Some(w) = weather {
            f.weather = parse_weather(w)?;
            f.weather_turns = weather_turns.unwrap_or(5);
            f.weather_src = 0;
        }
        if let Some(t) = terrain {
            f.terrain = parse_terrain(t)?;
            f.terrain_turns = terrain_turns.unwrap_or(5);
            f.terrain_src = 0;
        }
        if let Some(tr) = trick_room {
            f.trick_room = tr;
        }
        Ok(())
    }

    /// (2026-10-11) also the entry hazards on that side: rebuilt positions had none (a Pokemon switched into Toxic Spikes
    /// stayed healthy in the search).
    #[pyo3(signature = (side, tailwind=None, reflect=None, light_screen=None, aurora_veil=None, mega_used=None,
                        stealth_rock=None, spikes=None, toxic_spikes=None, sticky_web=None))]
    fn set_side(&mut self, side: usize, tailwind: Option<u8>, reflect: Option<u8>, light_screen: Option<u8>, aurora_veil: Option<u8>, mega_used: Option<bool>,
                stealth_rock: Option<bool>, spikes: Option<u8>, toxic_spikes: Option<u8>, sticky_web: Option<bool>) {
        let sd = &mut self.b.sides[side.min(1)];
        if let Some(x) = stealth_rock {
            sd.stealth_rock = x;
        }
        if let Some(x) = spikes {
            sd.spikes = x.min(3);
        }
        if let Some(x) = toxic_spikes {
            sd.toxic_spikes = x.min(2);
        }
        if let Some(x) = sticky_web {
            sd.sticky_web = x;
        }
        if let Some(x) = tailwind {
            sd.tailwind = x;
        }
        if let Some(x) = reflect {
            sd.reflect = x;
            sd.screen_src[0] = 0;
        }
        if let Some(x) = light_screen {
            sd.light_screen = x;
            sd.screen_src[1] = 0;
        }
        if let Some(x) = aurora_veil {
            sd.aurora_veil = x;
            sd.screen_src[2] = 0;
        }
        if let Some(x) = mega_used {
            sd.mega_used = x;
        }
    }

    /// After editing: re-check the winner and pending replacements.
    fn refresh(&mut self) {
        self.b.refresh();
    }

    /// Turn on the readable battle log (see `log()`).
    fn enable_log(&mut self) {
        self.b.log = Some(vec![]);
    }

    fn log(&self) -> Vec<String> {
        self.b.log.clone().unwrap_or_default()
    }
}

/// (2026-10-18 speed) Regret matching+ with linear averaging on a zero-sum game M [n][m] (row player maximises): the same
/// algorithm and update order as nnsearch.solve (Python), ~1000 iterations in microseconds instead of ~16 ms.
/// (2026-11-02, opts.rmfast) the endgame solver's matrix games: rm_core, or (rmfast > 0) a pure saddle point at once, else
/// regret matching stopped once the averaged strategies' exploitability gap is <= rmfast (checked every 16 iterations)
fn eg_rm(mm: &[f64], n: usize, m: usize, iters: usize) -> (Vec<f64>, Vec<f64>) {
    let tol = f64::from_bits(EG_RMFAST.load(std::sync::atomic::Ordering::Relaxed));
    if tol <= 0.0 {
        return rm_core(mm, n, m, iters);
    }
    let _egp = egp(EGP_GAME_SOLVE);
    let (mut bi, mut lo) = (0usize, f64::NEG_INFINITY);
    for i in 0..n {
        let r = mm[i * m..(i + 1) * m].iter().copied().fold(f64::INFINITY, f64::min);
        if r > lo { lo = r; bi = i; }
    }
    let (mut bj, mut hi) = (0usize, f64::INFINITY);
    for j in 0..m {
        let mut c = f64::NEG_INFINITY;
        for i in 0..n { c = c.max(mm[i * m + j]); }
        if c < hi { hi = c; bj = j; }
    }
    if hi - lo <= 1e-12 {
        let mut x = vec![0.0; n];
        let mut y = vec![0.0; m];
        x[bi] = 1.0;
        y[bj] = 1.0;
        return (x, y);
    }
    let (mut rx, mut ry) = (vec![0.0f64; n], vec![0.0f64; m]);
    let (mut sx, mut sy) = (vec![0.0f64; n], vec![0.0f64; m]);
    let (mut x, mut y) = (vec![0.0f64; n], vec![0.0f64; m]);
    let (mut ux, mut uy) = (vec![0.0f64; n], vec![0.0f64; m]);
    let (mut ax, mut ay) = (vec![0.0f64; n], vec![0.0f64; m]);
    for t in 1..=iters {
        let s: f64 = rx.iter().sum();
        for i in 0..n { x[i] = if s > 0.0 { rx[i] / s } else { 1.0 / n as f64 }; }
        let s: f64 = ry.iter().sum();
        for j in 0..m { y[j] = if s > 0.0 { ry[j] / s } else { 1.0 / m as f64 }; }
        for i in 0..n {
            let row = &mm[i * m..(i + 1) * m];
            ux[i] = row.iter().zip(&y).map(|(a, b)| a * b).sum();
        }
        for j in 0..m { uy[j] = 0.0; }
        for i in 0..n {
            let row = &mm[i * m..(i + 1) * m];
            for j in 0..m { uy[j] -= x[i] * row[j]; }
        }
        let vx: f64 = x.iter().zip(&ux).map(|(a, b)| a * b).sum();
        let vy: f64 = y.iter().zip(&uy).map(|(a, b)| a * b).sum();
        for i in 0..n { rx[i] = (rx[i] + ux[i] - vx).max(0.0); sx[i] += t as f64 * x[i]; }
        for j in 0..m { ry[j] = (ry[j] + uy[j] - vy).max(0.0); sy[j] += t as f64 * y[j]; }
        if t % 16 == 0 && t < iters {
            let a: f64 = sx.iter().sum();
            let b: f64 = sy.iter().sum();
            for i in 0..n { ax[i] = sx[i] / a; }
            for j in 0..m { ay[j] = sy[j] / b; }
            let mut br = f64::NEG_INFINITY;
            for i in 0..n {
                let u: f64 = mm[i * m..(i + 1) * m].iter().zip(&ay).map(|(p, q)| p * q).sum();
                br = br.max(u);
            }
            let mut bc = f64::INFINITY;
            for j in 0..m {
                let mut u = 0.0;
                for i in 0..n { u += ax[i] * mm[i * m + j]; }
                bc = bc.min(u);
            }
            if br - bc <= tol {
                return (ax, ay);
            }
        }
    }
    let a: f64 = sx.iter().sum();
    let b: f64 = sy.iter().sum();
    (sx.iter().map(|v| v / a).collect(), sy.iter().map(|v| v / b).collect())
}

fn rm_core(mm: &[f64], n: usize, m: usize, iters: usize) -> (Vec<f64>, Vec<f64>) {
    let _egp = egp(EGP_GAME_SOLVE);
    let (mut rx, mut ry) = (vec![0.0f64; n], vec![0.0f64; m]);
    let (mut sx, mut sy) = (vec![0.0f64; n], vec![0.0f64; m]);
    let (mut x, mut y) = (vec![0.0f64; n], vec![0.0f64; m]);
    let (mut ux, mut uy) = (vec![0.0f64; n], vec![0.0f64; m]);
    for t in 1..=iters {
        let s: f64 = rx.iter().sum();
        for i in 0..n { x[i] = if s > 0.0 { rx[i] / s } else { 1.0 / n as f64 }; }
        let s: f64 = ry.iter().sum();
        for j in 0..m { y[j] = if s > 0.0 { ry[j] / s } else { 1.0 / m as f64 }; }
        for i in 0..n {
            let row = &mm[i * m..(i + 1) * m];
            ux[i] = row.iter().zip(&y).map(|(a, b)| a * b).sum();
        }
        for j in 0..m { uy[j] = 0.0; }
        for i in 0..n {
            let row = &mm[i * m..(i + 1) * m];
            for j in 0..m { uy[j] -= x[i] * row[j]; }
        }
        let vx: f64 = x.iter().zip(&ux).map(|(a, b)| a * b).sum();
        let vy: f64 = y.iter().zip(&uy).map(|(a, b)| a * b).sum();
        for i in 0..n { rx[i] = (rx[i] + ux[i] - vx).max(0.0); sx[i] += t as f64 * x[i]; }
        for j in 0..m { ry[j] = (ry[j] + uy[j] - vy).max(0.0); sy[j] += t as f64 * y[j]; }
    }
    let a: f64 = sx.iter().sum();
    let b: f64 = sy.iter().sum();
    (sx.iter().map(|v| v / a).collect(), sy.iter().map(|v| v / b).collect())
}

fn flat(mat: &[Vec<f64>]) -> PyResult<(Vec<f64>, usize, usize)> {
    let n = mat.len();
    let m = mat.first().map(|r| r.len()).unwrap_or(0);
    if n == 0 || m == 0 || mat.iter().any(|r| r.len() != m) {
        return Err(PyValueError::new_err("payoff matrix must be non-empty and rectangular"));
    }
    Ok((mat.iter().flatten().copied().collect(), n, m))
}

/// rm_solve(M, iters=1000) -> (x, y): see rm_core.
#[pyfunction]
#[pyo3(signature = (mat, iters=1000))]
fn rm_solve(py: Python<'_>, mat: Vec<Vec<f64>>, iters: usize) -> PyResult<(Vec<f64>, Vec<f64>)> {
    let (mm, n, m) = flat(&mat)?;
    Ok(py.detach(|| rm_core(&mm, n, m, iters)))
}

/// rm_solve_batch([M...], iters) -> [(x, y)...] (deep2.batch_solve's games, each solved on its own: padding never mattered)
#[pyfunction]
#[pyo3(signature = (mats, iters=400))]
fn rm_solve_batch(py: Python<'_>, mats: Vec<Vec<Vec<f64>>>, iters: usize) -> PyResult<Vec<(Vec<f64>, Vec<f64>)>> {
    let fl: Vec<(Vec<f64>, usize, usize)> = mats.iter().map(|m| flat(m)).collect::<PyResult<_>>()?;
    Ok(py.detach(|| fl.par_iter().map(|(mm, n, m)| rm_core(mm, *n, *m, iters)).collect()))
}

/// (2026-10-28, ndeep speed) rm_solve_flat(data, shapes, iters) -> (xs, ys): many small games from ONE buffer: `data` = the
/// matrices' float64 values in order (row-major, each n x m as in `shapes`), returns the strategies concatenated as float64
/// bytes (x of game 0, x of game 1, ... | y of game 0, ...). Same solver as rm_solve_batch without a nested list per matrix.
#[pyfunction]
#[pyo3(signature = (data, shapes, iters=400))]
fn rm_solve_flat(py: Python<'_>, data: &[u8], shapes: Vec<(usize, usize)>, iters: usize) -> PyResult<(Py<PyBytes>, Py<PyBytes>)> {
    let vals: Vec<f64> = data.chunks_exact(8).map(|c| f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]])).collect();
    let need: usize = shapes.iter().map(|(n, m)| n * m).sum();
    if need != vals.len() || shapes.iter().any(|(n, m)| *n == 0 || *m == 0) {
        return Err(PyValueError::new_err("rm_solve_flat: data length doesn't match the shapes"));
    }
    let mut offs = Vec::with_capacity(shapes.len());
    let mut o = 0;
    for (n, m) in &shapes {
        offs.push(o);
        o += n * m;
    }
    let res: Vec<(Vec<f64>, Vec<f64>)> = py.detach(|| {
        shapes.par_iter().zip(offs.par_iter()).map(|((n, m), off)| rm_core(&vals[*off..*off + n * m], *n, *m, iters)).collect()
    });
    let xs: Vec<u8> = res.iter().flat_map(|(x, _)| x.iter().flat_map(|v| v.to_le_bytes())).collect();
    let ys: Vec<u8> = res.iter().flat_map(|(_, y)| y.iter().flat_map(|v| v.to_le_bytes())).collect();
    Ok((PyBytes::new(py, &xs).unbind(), PyBytes::new(py, &ys).unbind()))
}

/// rm_bayes_solve(Ms [W][n][m], p [W], iters) -> (x [n], Y [W][m]): nnsearch.bayes_solve (one x for us, one y per world).
#[pyfunction]
#[pyo3(signature = (mats, p, iters=1000))]
fn rm_bayes_solve(py: Python<'_>, mats: Vec<Vec<Vec<f64>>>, p: Vec<f64>, iters: usize) -> PyResult<(Vec<f64>, Vec<Vec<f64>>)> {
    let w = mats.len();
    if w == 0 || p.len() != w {
        return Err(PyValueError::new_err("need one weight per world"));
    }
    let fl: Vec<(Vec<f64>, usize, usize)> = mats.iter().map(|m| flat(m)).collect::<PyResult<_>>()?;
    let (n, m) = (fl[0].1, fl[0].2);
    if fl.iter().any(|f| f.1 != n || f.2 != m) {
        return Err(PyValueError::new_err("all worlds need the same shape"));
    }
    let ps: f64 = p.iter().sum();
    let p: Vec<f64> = p.iter().map(|v| v / ps).collect();
    Ok(py.detach(|| {
        let (mut rx, mut sx, mut x, mut ux) = (vec![0.0f64; n], vec![0.0f64; n], vec![0.0f64; n], vec![0.0f64; n]);
        let mut ry = vec![vec![0.0f64; m]; w];
        let mut sy = vec![vec![0.0f64; m]; w];
        let mut yy = vec![vec![0.0f64; m]; w];
        let mut uy = vec![0.0f64; m];
        for t in 1..=iters {
            let s: f64 = rx.iter().sum();
            for i in 0..n { x[i] = if s > 0.0 { rx[i] / s } else { 1.0 / n as f64 }; }
            for k in 0..w {
                let s: f64 = ry[k].iter().sum();
                for j in 0..m { yy[k][j] = if s > 0.0 { ry[k][j] / s.max(1e-30) } else { 1.0 / m as f64 }; }
            }
            for i in 0..n {
                let mut acc = 0.0;
                for k in 0..w {
                    let row = &fl[k].0[i * m..(i + 1) * m];
                    acc += p[k] * row.iter().zip(&yy[k]).map(|(a, b)| a * b).sum::<f64>();
                }
                ux[i] = acc;
            }
            let vx: f64 = x.iter().zip(&ux).map(|(a, b)| a * b).sum();
            for k in 0..w {
                for j in 0..m { uy[j] = 0.0; }
                for i in 0..n {
                    let row = &fl[k].0[i * m..(i + 1) * m];
                    for j in 0..m { uy[j] -= x[i] * row[j]; }
                }
                let vy: f64 = yy[k].iter().zip(&uy).map(|(a, b)| a * b).sum();
                for j in 0..m { ry[k][j] = (ry[k][j] + uy[j] - vy).max(0.0); sy[k][j] += t as f64 * yy[k][j]; }
            }
            for i in 0..n { rx[i] = (rx[i] + ux[i] - vx).max(0.0); sx[i] += t as f64 * x[i]; }
        }
        let a: f64 = sx.iter().sum();
        let xs: Vec<f64> = sx.iter().map(|v| v / a).collect();
        let ys: Vec<Vec<f64>> = sy.iter().map(|r| { let b: f64 = r.iter().sum(); r.iter().map(|v| v / b).collect() }).collect();
        (xs, ys)
    }))
}

/// (2026-10-28, speed pass 3c) 128-bit keys of encoded rows for the search caches: `f` = n x nf float32 rows and
/// `i` = n x ni int32 rows as little-endian bytes (as encode_battles / search_leaves_bytes return them, or numpy
/// .tobytes()); returns n x 16 bytes (np.frombuffer(..., dtype=np.dtype((np.void, 16)))). Identical input bits =
/// identical key; two independent 64-bit multiply-xor-rotate hashes over the row's 32-bit words.
#[pyfunction]
fn row_keys(py: Python<'_>, f: &[u8], i: &[u8], n: usize) -> PyResult<Py<PyBytes>> {
    if n == 0 {
        return Ok(PyBytes::new(py, &[]).unbind());
    }
    if f.len() % n != 0 || i.len() % n != 0 || f.len() % 4 != 0 || i.len() % 4 != 0 {
        return Err(PyValueError::new_err("row_keys: byte lengths must be multiples of 4 * n"));
    }
    let (lf, li) = (f.len() / n, i.len() / n);
    let out = py.detach(|| {
        let mut out = vec![0u8; n * 16];
        out.par_chunks_mut(16).enumerate().for_each(|(r, o)| {
            let mut h1: u64 = 0x9E37_79B9_7F4A_7C15;
            let mut h2: u64 = 0xD1B5_4A32_D192_ED03;
            let mut step = |w: u32| {
                let x = w as u64;
                h1 = (h1 ^ x).wrapping_mul(0xFF51_AFD7_ED55_8CCD).rotate_left(29) ^ (x.wrapping_mul(0x9E37_79B9_7F4A_7C15));
                h2 = (h2.rotate_left(23) ^ x).wrapping_mul(0xC4CE_B9FE_1A85_EC53).wrapping_add(x.rotate_left(17));
            };
            for c in f[r * lf..(r + 1) * lf].chunks_exact(4) {
                step(u32::from_le_bytes([c[0], c[1], c[2], c[3]]));
            }
            for c in i[r * li..(r + 1) * li].chunks_exact(4) {
                step(u32::from_le_bytes([c[0], c[1], c[2], c[3]]) ^ 0xA5A5_A5A5);
            }
            // finalisers (mix the high bits down)
            h1 ^= h1 >> 33; h1 = h1.wrapping_mul(0xFF51_AFD7_ED55_8CCD); h1 ^= h1 >> 29;
            h2 ^= h2 >> 31; h2 = h2.wrapping_mul(0xC4CE_B9FE_1A85_EC53); h2 ^= h2 >> 27;
            o[..8].copy_from_slice(&h1.to_le_bytes());
            o[8..].copy_from_slice(&h2.to_le_bytes());
        });
        out
    });
    Ok(PyBytes::new(py, &out).unbind())
}

/// (2026-10-28, speed pass 3c) kind of many positions in one call: 0 = turn, 1 = switch (replacement), 2 = ended,
/// side 0 won, 3 = ended, side 1 won, 4 = ended, tie / undecided (other phases count as turn). One byte per battle.
/// (2026-10-31) a search leaf's result from side 0's view: +1 / -1 / 0 for a finished game or a dead-end 1v1
/// (vgcsim::deadend), NaN while it goes on.
fn leaf_term(c: &Engine, dex: &EngineDex) -> f32 {
    if c.phase == Phase::Ended {
        return match c.winner { Some(0) => 1.0, Some(1) => -1.0, _ => 0.0 };
    }
    match c.dead_end(dex) { Some(0) => 1.0, Some(_) => -1.0, None => eg_leaf(c, dex) }
}

/// kind byte as battle_kinds (0 turn, 1 switch, 2 / 3 side 0 / 1 won, 4 tie), dead-end 1v1s counted as won.
fn leaf_kind(c: &Engine, dex: &EngineDex) -> u8 {
    match c.phase {
        Phase::Ended => match c.winner { Some(0) => 2, Some(1) => 3, _ => 4 },
        Phase::Switch => 1,
        _ => match c.dead_end(dex) { Some(0) => 2, Some(_) => 3, None => 0 },
    }
}

/// (2026-10-31) turn the dead-end rule for search leaves on / off for this process (default on; env VGCSIM_DEAD_END=0).
#[pyfunction]
fn set_dead_end(on: bool) {
    ::vgcsim::deadend::set_dead_end(on);
}

/// Battle.dead_end as a module function: the side that surely wins this 1v1, or None.
#[pyfunction]
fn dead_end_winner(b: PyRef<'_, Battle>) -> Option<usize> {
    b.b.dead_end(&b.dex)
}

#[pyfunction]
fn battle_kinds(py: Python<'_>, battles: Vec<PyRef<'_, Battle>>) -> Py<PyBytes> {
    let out: Vec<u8> = battles.iter().map(|b| leaf_kind(&b.b, &b.dex)).collect();
    PyBytes::new(py, &out).unbind()
}

/// (2026-10-28, speed pass 3e) Battle.chance_leaf_battles for many positions in one call (world 0 only, every our x their
/// pair of each position, one seed per position): returns (counts bytes = leaves per cell as u16, in position order then
/// cell order a * nb + b; p bytes = each leaf's weight as f32; leaves flat in that order; kinds bytes as battle_kinds).
/// Enumerations of all positions run in one parallel pass; the leaves go straight into the Python list.
#[pyfunction]
fn chance_leaf_battles_many(py: Python<'_>, battles: Vec<PyRef<'_, Battle>>, side: usize,
                            ours: Vec<Vec<(ActionTuple, ActionTuple)>>, theirs: Vec<Vec<(ActionTuple, ActionTuple)>>,
                            seeds: Vec<u64>, min_path: f64, max_leaves: usize, repl: bool, hidden: bool)
                            -> PyResult<(Py<PyBytes>, Py<PyBytes>, Py<PyList>, Py<PyBytes>)> {
    if ours.len() != battles.len() || theirs.len() != battles.len() || seeds.len() != battles.len() {
        return Err(PyValueError::new_err("chance_leaf_battles_many: one ours / theirs / seed per battle"));
    }
    let side = side.min(1);
    let mut os: Vec<Vec<[Action; 2]>> = Vec::with_capacity(battles.len());
    let mut ts: Vec<Vec<[Action; 2]>> = Vec::with_capacity(battles.len());
    let mut jobs: Vec<(usize, usize, usize)> = Vec::new();
    for (n, (o, t)) in ours.iter().zip(theirs.iter()).enumerate() {
        let (o, t) = (conv_joint(o)?, conv_joint(t)?);
        for oi in 0..o.len() {
            for ti in 0..t.len() {
                jobs.push((n, oi, ti));
            }
        }
        os.push(o);
        ts.push(t);
    }
    let refs: Vec<(&Engine, &EngineDex, bool)> = battles.iter().map(|pb| (&pb.b, &*pb.dex, pb.keep_stats)).collect();
    let out: Vec<Vec<(Engine, f32)>> = py.detach(|| {
        let views: Vec<Engine> = refs.iter().zip(seeds.iter())
            .map(|(&(b, dex, ks), &s)| search_views(b, dex, side, 1, s, ks, hidden).swap_remove(0)).collect();
        jobs.par_iter().map(|&(n, oi, ti)| enumerate_cell(&views[n], refs[n].1, os[n][oi], ts[n][ti], cell_seed(seeds[n], 0),
                                                           min_path, max_leaves, repl)).collect()
    });
    let kids = PyList::empty(py);
    let mut counts: Vec<u8> = Vec::with_capacity(jobs.len() * 2);
    let mut ps: Vec<u8> = Vec::new();
    let mut kinds: Vec<u8> = Vec::new();
    for (v, &(n, _, _)) in out.into_iter().zip(jobs.iter()) {
        counts.extend_from_slice(&(v.len() as u16).to_le_bytes());
        let pb = &battles[n];
        for (e, w) in v {
            ps.extend_from_slice(&w.to_le_bytes());
            kinds.push(leaf_kind(&e, &pb.dex));
            kids.append(Py::new(py, Battle { dex: pb.dex.clone(), b: e, keep_stats: pb.keep_stats })?)?;
        }
    }
    Ok((PyBytes::new(py, &counts).unbind(), PyBytes::new(py, &ps).unbind(), kids.unbind(), PyBytes::new(py, &kinds).unbind()))
}

/// (2026-10-28, speed pass 3e) reach of every tree position in one pass: `par` (int64 per node, -1 = a root leaf), `w` (f64 per
/// node, the edge weight below its parent), root leaves' reach from (`root_ids` int64, `root_reach` f64). Node ids come after
/// their parents', so reach[k] = reach[par[k]] * w[k] in id order gives exactly the level-wise products.
#[pyfunction]
fn tree_reach(py: Python<'_>, n: usize, par: &[u8], w: &[u8], root_ids: &[u8], root_reach: &[u8]) -> PyResult<Py<PyBytes>> {
    if par.len() < n * 8 || w.len() < n * 8 || root_ids.len() != root_reach.len() {
        return Err(PyValueError::new_err("tree_reach: buffer sizes"));
    }
    let pv: Vec<i64> = par[..n * 8].chunks_exact(8).map(|c| i64::from_le_bytes(c.try_into().unwrap())).collect();
    let wv: Vec<f64> = w[..n * 8].chunks_exact(8).map(|c| f64::from_le_bytes(c.try_into().unwrap())).collect();
    let mut reach = vec![0.0f64; n];
    for (c, r) in root_ids.chunks_exact(8).zip(root_reach.chunks_exact(8)) {
        let k = i64::from_le_bytes(c.try_into().unwrap());
        if k >= 0 && (k as usize) < n {
            reach[k as usize] = f64::from_le_bytes(r.try_into().unwrap());
        }
    }
    for k in 0..n {
        let p = pv[k];
        if p >= 0 {
            let p = p as usize;
            if p >= k {
                return Err(PyValueError::new_err("tree_reach: a parent id after its child"));
            }
            reach[k] = reach[p] * wv[k];
        }
    }
    let bytes: Vec<u8> = reach.iter().flat_map(|v| v.to_le_bytes()).collect();
    Ok(PyBytes::new(py, &bytes).unbind())
}

/// (2026-10-28, speed pass 3c) Battle.replacement_children for many replacement positions in one call: returns
/// (counts bytes = [na_0, nb_0, na_1, nb_1, ...] as u8, kinds bytes as battle_kinds, children flat in input order).
#[pyfunction]
fn replacement_children_many(py: Python<'_>, battles: Vec<PyRef<'_, Battle>>, cap: usize) -> PyResult<(Py<PyBytes>, Py<PyBytes>, Py<PyList>)> {
    // Children go straight into the Python list as they are made: a batch of a few hundred positions makes ~1,000
    // children of 4.4 KB each, and collecting them in large fresh Vecs first (page faults, two extra moves) cost more
    // than the expansions themselves (batched 11 us vs 5 us per position). No GIL release: ~2 us of work per child.
    let kids = PyList::empty(py);
    let mut counts = Vec::with_capacity(battles.len() * 2);
    let mut kinds = Vec::new();
    for pb in battles.iter() {
        let dex = &pb.dex;
        let b = &pb.b;
        let mut jo = policy::replacement_joints(b, dex, 0);
        let mut jt = policy::replacement_joints(b, dex, 1);
        jo.truncate(cap.max(1));
        jt.truncate(cap.max(1));
        counts.push(jo.len() as u8);
        counts.push(jt.len() as u8);
        for a in &jo {
            for bb in &jt {
                let mut c = b.clone();
                if c.needs_choice(0) {
                    c.choose(dex, 0, *a);
                }
                if c.needs_choice(1) {
                    c.choose(dex, 1, *bb);
                }
                kinds.push(leaf_kind(&c, dex));
                kids.append(Py::new(py, Battle { dex: dex.clone(), b: c, keep_stats: pb.keep_stats })?)?;
            }
        }
    }
    Ok((PyBytes::new(py, &counts).unbind(), PyBytes::new(py, &kinds).unbind(), kids.unbind()))
}

#[pymodule]
fn vgcsim(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<Dex>()?;
    m.add_class::<Battle>()?;
    m.add_class::<exact::ExactDex>()?;
    m.add_class::<exact::ExactBattle>()?;
    m.add_class::<Rollouts>()?;
    m.add_function(wrap_pyfunction!(encode::encode_battle, m)?)?;
    m.add_function(wrap_pyfunction!(encode::encode_battles, m)?)?;
    m.add_function(wrap_pyfunction!(encode::set_unc_library, m)?)?;
    m.add_function(wrap_pyfunction!(encode::set_open_gate, m)?)?;
    m.add_function(wrap_pyfunction!(encode::enc_prof, m)?)?;
    m.add_function(wrap_pyfunction!(encode::set_dmg_state, m)?)?;
    m.add_function(wrap_pyfunction!(encode::obs_layout, m)?)?;
    m.add_function(wrap_pyfunction!(encode::ext_layout, m)?)?;
    m.add_function(wrap_pyfunction!(encode::v3_layout, m)?)?;
    m.add_function(wrap_pyfunction!(encode::flag_layout, m)?)?;
    m.add_function(wrap_pyfunction!(rm_solve, m)?)?;
    m.add_function(wrap_pyfunction!(rm_solve_batch, m)?)?;
    m.add_function(wrap_pyfunction!(rm_solve_flat, m)?)?;
    m.add_function(wrap_pyfunction!(row_keys, m)?)?;
    m.add_function(wrap_pyfunction!(battle_kinds, m)?)?;
    m.add_function(wrap_pyfunction!(set_dead_end, m)?)?;
    m.add_function(wrap_pyfunction!(eg_leaves_config, m)?)?;
    m.add_function(wrap_pyfunction!(eg_solver_opts, m)?)?;
    m.add_function(wrap_pyfunction!(eg_solver_stats, m)?)?;
    m.add_function(wrap_pyfunction!(eg_prof, m)?)?;
    m.add_function(wrap_pyfunction!(eg_tt_reset, m)?)?;
    m.add_function(wrap_pyfunction!(eg_leaf_bounds, m)?)?;
    m.add_function(wrap_pyfunction!(eg_leaves_stats, m)?)?;
    m.add_function(wrap_pyfunction!(eg_leaf_values, m)?)?;
    m.add_function(wrap_pyfunction!(dead_end_winner, m)?)?;
    m.add_function(wrap_pyfunction!(replacement_children_many, m)?)?;
    m.add_function(wrap_pyfunction!(chance_leaf_battles_many, m)?)?;
    m.add_function(wrap_pyfunction!(tree_reach, m)?)?;
    m.add_function(wrap_pyfunction!(rm_bayes_solve, m)?)?;
    m.add_function(wrap_pyfunction!(encode::preview_options, m)?)?;
    m.add_function(wrap_pyfunction!(encode::encode_preview, m)?)?;
    Ok(())
}

// ---------------------------------------------------------------------------------------------------------------------
// (2026-11-01, user: "build the endgame leaves first") exact values at 1v1 search leaves. A search leaf where each side has
// at most `max_mons` Pokemon left and both must choose a turn gets a small eg_exact solve (no network, exact chance
// outcomes); when it certifies (gap <= 2 tol) its value replaces the value net's. Results (solved or not) are cached per
// position for the process; solves stop after the per-decision deadline set by `eg_leaves_config(seconds=)`.
// Off by default (budget 0). Python: vgcsim.eg_leaves_config(...), vgcsim.eg_leaves_stats(), vgcsim.eg_leaf_values(...).
struct EgLeafCfg {
    budget: usize,
    depth: usize,
    max_leaves: usize,
    tol: f64,
    max_mons: usize,
    // (2026-11-02) at most this many Pokemon left on both sides together (0 = no cap; 3 = 1v1, 2v1, 1v2)
    max_total: usize,
    end: Option<std::time::Instant>,
}

static EG_LEAF: std::sync::RwLock<EgLeafCfg> =
    std::sync::RwLock::new(EgLeafCfg { budget: 0, depth: 4, max_leaves: 64, tol: 0.02, max_mons: 1, max_total: 0, end: None });
/// (value or NaN, proven lower bound, proven upper bound) per position (2026-11-01: bounds kept for clamping net values)
static EG_CACHE: std::sync::Mutex<Option<std::collections::HashMap<u128, (f32, f32, f32)>>> = std::sync::Mutex::new(None);
static EG_STATS: [std::sync::atomic::AtomicU64; 5] = [const { std::sync::atomic::AtomicU64::new(0) }; 5]; // asked, cache hits, solved, failed, skipped (late)
const EG_CACHE_MAX: usize = 400_000;

fn eg_key(c: &Engine, dex: &EngineDex) -> u128 {
    let _egp = egp(EGP_KEY_ENCODE);
    if EG_FASTKEY.load(std::sync::atomic::Ordering::Relaxed) {
        return c.state_hash() ^ EG_FASTKEY_TAG;
    }
    // both sides' open views: side 1's own view carries its exact stats, which the side-0 view hides
    let (nf, ni) = (encode::OBS_F, encode::OBS_I);
    let (mut h1, mut h2) = (0xcbf2_9ce4_8422_2325u64, 0x8422_2325_cbf2_9ce4u64);
    let mut f = vec![0f32; nf];
    let mut ids = vec![0i32; ni];
    for s in 0..2 {
        f.iter_mut().for_each(|x| *x = 0.0);
        ids.iter_mut().for_each(|x| *x = 0);
        encode::encode(dex, c, s, false, &mut f, &mut ids);
        for x in f.iter().map(|x| x.to_bits()).chain(ids.iter().map(|x| *x as u32)) {
            h1 = (h1 ^ x as u64).wrapping_mul(0x0000_0100_0000_01B3);
            h2 = (h2 ^ (x as u64).rotate_left(17)).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
    }
    ((h1 as u128) << 64) | h2 as u128
}

fn eg_leaf_qualifies(c: &Engine, max_mons: usize) -> bool {
    if c.phase != Phase::Turn || c.winner.is_some() || !(c.needs_choice(0) && c.needs_choice(1)) {
        return false;
    }
    (0..2).all(|s| {
        let n = c.sides[s].mons.iter().filter(|m| m.alive()).count();
        n > 0 && n <= max_mons
    })
}

/// exact value of a search leaf from side 0's view, NaN when off / not a small endgame / not certified / past the deadline
fn eg_leaf(c: &Engine, dex: &EngineDex) -> f32 {
    eg_leaf_b(c, dex).0
}

/// (2026-11-01) eg_leaf plus the proven bounds of the deepest finished iteration ((-1, 1) when nothing finished / not asked)
fn eg_leaf_b(c: &Engine, dex: &EngineDex) -> (f32, f32, f32) {
    const NONE: (f32, f32, f32) = (f32::NAN, -1.0, 1.0);
    use std::sync::atomic::Ordering::Relaxed;
    let (budget, depth, max_leaves, tol, max_mons, max_total, end) = {
        let g = EG_LEAF.read().unwrap();
        (g.budget, g.depth, g.max_leaves, g.tol, g.max_mons, g.max_total, g.end)
    };
    if budget == 0 || !eg_leaf_qualifies(c, max_mons) {
        return NONE;
    }
    if max_total > 0 && c.sides.iter().map(|sd| sd.mons.iter().filter(|m| m.alive()).count()).sum::<usize>() > max_total {
        return NONE;
    }
    EG_STATS[0].fetch_add(1, Relaxed);
    let key = eg_key(c, dex);
    if let Some(v) = EG_CACHE.lock().unwrap().as_ref().and_then(|m| m.get(&key).copied()) {
        EG_STATS[1].fetch_add(1, Relaxed);
        return v;
    }
    let left = match end {
        Some(e) => {
            let now = std::time::Instant::now();
            if now >= e {
                EG_STATS[4].fetch_add(1, Relaxed);
                return NONE;
            }
            Some(e)
        }
        None => None,
    };
    let mut cx = eg_ctx(dex, budget, max_leaves, 300, 1, left);
    let mut v = f32::NAN;
    let (mut blo, mut bhi) = (-1.0f32, 1.0f32);
    let mut timed_out = false;
    for d in 1..=depth.max(1) {
        match eg_val(&mut cx, c, 0, d) {
            Ok((lo, hi)) => {
                blo = blo.max(lo as f32);
                bhi = bhi.min(hi as f32);
                if hi - lo <= 2.0 * tol {
                    v = (0.5 * (lo + hi)) as f32;
                    break;
                }
            }
            Err(e) => {
                timed_out = e == "time";
                break;
            }
        }
    }
    EG_STATS[if v.is_nan() { 3 } else { 2 }].fetch_add(1, Relaxed);
    if !timed_out {
        // a deadline stop says nothing about the position: don't cache it as unsolvable
        let mut g = EG_CACHE.lock().unwrap();
        let m = g.get_or_insert_with(std::collections::HashMap::new);
        if m.len() >= EG_CACHE_MAX {
            m.clear();
        }
        m.insert(key, (v, blo, bhi));
    }
    (v, blo, bhi)
}

/// eg_leaves_config(budget=0 (off), depth=4, max_leaves=64, tol=0.02, max_mons=1, seconds=0 (no deadline), clear=False):
/// set the exact-leaf solver for this process; `seconds` > 0 = solves started after now + seconds are skipped (call it at
/// the start of each decision). Returns nothing.
#[pyfunction]
#[pyo3(signature = (budget=0, depth=4, max_leaves=64, tol=0.02, max_mons=1, seconds=0.0, clear=false, max_total=0))]
fn eg_leaves_config(budget: usize, depth: usize, max_leaves: usize, tol: f64, max_mons: usize, seconds: f64, clear: bool,
                    max_total: usize) {
    let mut g = EG_LEAF.write().unwrap();
    *g = EgLeafCfg {
        budget, depth, max_leaves, tol, max_mons, max_total,
        end: if seconds > 0.0 { Some(std::time::Instant::now() + std::time::Duration::from_secs_f64(seconds)) } else { None },
    };
    if clear {
        *EG_CACHE.lock().unwrap() = None;
    }
}

/// (asked, cache hits, solved, failed, skipped late) since the last reset; reset=True zeroes them
#[pyfunction]
#[pyo3(signature = (reset=false))]
fn eg_leaves_stats(reset: bool) -> (u64, u64, u64, u64, u64) {
    use std::sync::atomic::Ordering::Relaxed;
    let r = (EG_STATS[0].load(Relaxed), EG_STATS[1].load(Relaxed), EG_STATS[2].load(Relaxed), EG_STATS[3].load(Relaxed),
             EG_STATS[4].load(Relaxed));
    if reset {
        for s in EG_STATS.iter() {
            s.store(0, Relaxed);
        }
    }
    r
}

/// (2026-11-01) eg_leaf_bounds([battle...]) -> bytes of float32 [n, 3] = (exact value or NaN, proven lower, proven upper
/// bound) of side 0's result; for uncertified positions the bounds of the deepest finished iteration (to clamp net values).
#[pyfunction]
fn eg_leaf_bounds(py: Python<'_>, battles: Vec<PyRef<'_, Battle>>) -> Py<PyBytes> {
    let refs: Vec<(&Engine, &EngineDex)> = battles.iter().map(|b| (&b.b, &*b.dex)).collect();
    let out: Vec<(f32, f32, f32)> = py.detach(|| refs.par_iter().map(|(b, d)| eg_leaf_b(b, d)).collect());
    let bytes: Vec<u8> = out.iter().flat_map(|x| [x.0, x.1, x.2]).flat_map(|x| x.to_le_bytes()).collect();
    PyBytes::new(py, &bytes).unbind()
}

/// eg_leaf_values([battle...]) -> bytes of float32 (np.frombuffer): eg_leaf for many positions in parallel (ndeep's
/// tree positions); NaN = use the network.
#[pyfunction]
fn eg_leaf_values(py: Python<'_>, battles: Vec<PyRef<'_, Battle>>) -> Py<PyBytes> {
    let refs: Vec<(&Engine, &EngineDex)> = battles.iter().map(|b| (&b.b, &*b.dex)).collect();
    let out: Vec<f32> = py.detach(|| refs.par_iter().map(|(b, d)| eg_leaf(b, d)).collect());
    let bytes: Vec<u8> = out.iter().flat_map(|x| x.to_le_bytes()).collect();
    PyBytes::new(py, &bytes).unbind()
}
