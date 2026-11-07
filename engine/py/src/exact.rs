//! Python bindings for the exact engine (`vgcsim::sim`, Showdown's mechanics turn by turn).
//! Same methods as the old `Battle` where the bot uses them:
//!
//!     dex = vgcsim.ExactDex("engines/vgcsim/data/dex.json", "engines/vgcsim/data/effects.json")
//!     b = vgcsim.ExactBattle.from_brought(dex, ours_text, theirs_text, seed=1, turn=3)
//!     b.set_active(...); b.set_mon(...); b.set_field(...); b.set_side(...); b.refresh()
//!     values = b.payoff_matrix(ours, theirs, samples=4, seed=1, ko_bonus=0.5, position=[...])

use std::collections::HashMap;

use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use ::vgcsim::sim::battle::Battle as Engine;
use ::vgcsim::sim::data::{Data, BOOST_NAMES};
use ::vgcsim::sim::search::SlotAction;
use ::vgcsim::sim::state::{parse_team, RequestState};
use ::vgcsim::sim::val::Mon;

use crate::ActionTuple;

fn from_tuple(t: &ActionTuple) -> PyResult<SlotAction> {
    Ok(match t.0.as_str() {
        "move" => SlotAction::Move { slot: t.1 as u8, target: t.2 as i8, mega: t.3 },
        "switch" => SlotAction::Switch { to: t.1 as u8 },
        "pass" => SlotAction::Pass,
        other => return Err(PyValueError::new_err(format!("unknown action kind {other}"))),
    })
}

fn weather_id(s: &str) -> PyResult<&'static str> {
    Ok(match s {
        "" | "none" => "",
        "raindance" | "rain" => "raindance",
        "sunnyday" | "sun" => "sunnyday",
        "sandstorm" | "sand" => "sandstorm",
        "snowscape" | "snow" | "hail" => "snowscape",
        "desolateland" => "desolateland",
        "primordialsea" => "primordialsea",
        "deltastream" => "deltastream",
        _ => return Err(PyValueError::new_err(format!("unknown weather {s}"))),
    })
}

fn terrain_id(s: &str) -> PyResult<&'static str> {
    Ok(match s {
        "" | "none" => "",
        "electricterrain" | "electric" => "electricterrain",
        "grassyterrain" | "grassy" => "grassyterrain",
        "psychicterrain" | "psychic" => "psychicterrain",
        "mistyterrain" | "misty" => "mistyterrain",
        _ => return Err(PyValueError::new_err(format!("unknown terrain {s}"))),
    })
}

#[pyclass(frozen)]
pub struct ExactDex {
    d: &'static Data,
}

#[pymethods]
impl ExactDex {
    #[new]
    fn new(dex_path: &str, effects_path: &str) -> PyResult<Self> {
        Ok(ExactDex { d: ::vgcsim::sim::load_data(dex_path, effects_path).map_err(PyValueError::new_err)? })
    }
}

#[pyclass]
pub struct ExactBattle {
    b: Engine,
}

impl ExactBattle {
    fn check_mon(&self, side: usize, idx: usize) -> PyResult<Mon> {
        if side > 1 || idx >= self.b.sides[side].mons.len() {
            return Err(PyValueError::new_err(format!("no Pokemon {idx} on side {side}")));
        }
        Ok(Mon { s: side as u8, i: idx as u8 })
    }
}

fn seed_words(seed: u64) -> [u16; 4] {
    // spread a small integer seed over all four words
    let x = seed.wrapping_mul(0x9E3779B97F4A7C15) ^ 0x5DEECE66D;
    [(x >> 48) as u16, (x >> 32) as u16, (x >> 16) as u16, x as u16]
}

#[pymethods]
impl ExactBattle {
    /// A new battle from two full teams (export text), at team preview.
    #[new]
    #[pyo3(signature = (dex, team_a, team_b, seed=1))]
    fn new(dex: &ExactDex, team_a: &str, team_b: &str, seed: u64) -> PyResult<Self> {
        let (a, b) = (parse_team(team_a), parse_team(team_b));
        let e = Engine::new(dex.d, seed_words(seed), [&a, &b], ["A", "B"]).map_err(PyValueError::new_err)?;
        Ok(ExactBattle { b: e })
    }

    /// A battle in progress: each side's brought Pokemon (leads first), move request, no
    /// switch-in effects. Then set HP, boosts, field... and call refresh().
    #[staticmethod]
    #[pyo3(signature = (dex, brought_a, brought_b, seed=1, turn=1))]
    fn from_brought(dex: &ExactDex, brought_a: &str, brought_b: &str, seed: u64, turn: i32) -> PyResult<Self> {
        let (a, b) = (parse_team(brought_a), parse_team(brought_b));
        if a.is_empty() || b.is_empty() {
            return Err(PyValueError::new_err("empty team"));
        }
        let e = Engine::from_brought(dex.d, [&a, &b], seed_words(seed), turn).map_err(PyValueError::new_err)?;
        Ok(ExactBattle { b: e })
    }

    fn copy(&self) -> Self {
        ExactBattle { b: self.b.clone() }
    }

    fn reseed(&mut self, seed: u64) {
        self.b.prng = ::vgcsim::sim::prng::Prng::from_u64(seed);
    }

    #[getter]
    fn phase(&self) -> &'static str {
        if self.b.ended {
            return "ended";
        }
        match self.b.request_state {
            RequestState::TeamPreview => "team",
            RequestState::Switch => "switch",
            RequestState::Move => "move",
            RequestState::None => "none",
        }
    }

    #[getter]
    fn winner(&self) -> Option<u8> {
        self.b.winner.flatten()
    }

    #[getter]
    fn turn(&self) -> i32 {
        self.b.turn
    }

    fn needs_choice(&mut self, side: usize) -> bool {
        self.b.sides[side.min(1)].request != RequestState::None && !self.b.is_choice_done_pub(side.min(1))
    }

    /// A raw Showdown choice ("move 1 2, switch 3"); returns False if it was rejected.
    fn choose_raw(&mut self, side: usize, choice: &str) -> bool {
        self.b.choose(side.min(1), choice)
    }

    /// Choose with action tuples; an illegal choice becomes Showdown's default.
    fn choose(&mut self, side: usize, a0: ActionTuple, a1: ActionTuple) -> PyResult<()> {
        let acts = [from_tuple(&a0)?, from_tuple(&a1)?];
        self.b.choose_actions(side.min(1), acts);
        Ok(())
    }

    /// The choice string that `choose` would send.
    fn choice_string(&mut self, side: usize, a0: ActionTuple, a1: ActionTuple) -> PyResult<String> {
        let acts = [from_tuple(&a0)?, from_tuple(&a1)?];
        Ok(self.b.choice_string(side.min(1), acts))
    }

    fn playouts(&self, py: Python<'_>, n: u32, seed: u64, max_turns: i32) -> (u32, u32, u32, u32) {
        let b = &self.b;
        py.detach(|| b.playouts(n, seed, max_turns))
    }

    /// values[i][j] for side 0 after one turn of (ours[i], theirs[j]), averaged over `samples`
    /// seeds (the same seeds for every pair).
    #[pyo3(signature = (ours, theirs, samples=4, seed=1, ko_bonus=0.5, position=None, self_weight=1.0))]
    fn payoff_matrix(
        &self, py: Python<'_>, ours: Vec<(ActionTuple, ActionTuple)>, theirs: Vec<(ActionTuple, ActionTuple)>,
        samples: u32, seed: u64, ko_bonus: f64, position: Option<Vec<f64>>, self_weight: f64,
    ) -> PyResult<Vec<Vec<f64>>> {
        let conv = |v: &Vec<(ActionTuple, ActionTuple)>| -> PyResult<Vec<[SlotAction; 2]>> {
            v.iter().map(|(a, b)| Ok([from_tuple(a)?, from_tuple(b)?])).collect()
        };
        let (o, t) = (conv(&ours)?, conv(&theirs)?);
        let pos = position.unwrap_or_default();
        let b = &self.b;
        Ok(py.detach(|| b.payoff_matrix(&o, &t, samples, seed, ko_bonus, &pos, self_weight)))
    }

    fn state<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let b = &self.b;
        let d = b.d;
        let out = PyDict::new(py);
        out.set_item("turn", b.turn)?;
        out.set_item("phase", self.phase())?;
        out.set_item("winner", self.winner())?;
        let f = PyDict::new(py);
        f.set_item("weather", &d.eff(b.field.weather).id)?;
        f.set_item("weather_turns", b.field.weather_state.duration)?;
        f.set_item("terrain", &d.eff(b.field.terrain).id)?;
        f.set_item("terrain_turns", b.field.terrain_state.duration)?;
        let pw: Vec<(String, Option<f64>)> = b.field.pseudo_weather.iter().map(|(e, st)| (d.eff(*e).id.clone(), st.duration)).collect();
        f.set_item("pseudo_weather", pw)?;
        out.set_item("field", f)?;
        let sides = PyList::empty(py);
        for s in 0..2 {
            let sd = &b.sides[s];
            let sdict = PyDict::new(py);
            sdict.set_item("active", sd.active.to_vec())?;
            sdict.set_item("pokemon_left", sd.pokemon_left)?;
            sdict.set_item("force_switch", sd.force_switch.to_vec())?;
            sdict.set_item("revival", vec![b.has_revival(s, 0), b.has_revival(s, 1)])?;
            let sc: Vec<(String, Option<f64>)> = sd.side_conditions.iter().map(|(e, st)| (d.eff(*e).id.clone(), st.duration)).collect();
            sdict.set_item("side_conditions", sc)?;
            let mons = PyList::empty(py);
            for p in &sd.mons {
                let md = PyDict::new(py);
                md.set_item("species", &d.species[p.species as usize].name)?;
                md.set_item("hp", p.hp)?;
                md.set_item("maxhp", p.maxhp)?;
                md.set_item("status", &d.eff(p.status).id)?;
                md.set_item("fainted", p.fainted)?;
                md.set_item("active", p.is_active)?;
                md.set_item("position", p.position)?;
                md.set_item("ability", &d.eff(p.ability).id)?;
                md.set_item("item", &d.eff(p.item).id)?;
                md.set_item("stats", p.stored_stats.to_vec())?;
                let boosts = PyDict::new(py);
                for (k, n) in BOOST_NAMES.iter().enumerate() {
                    boosts.set_item(*n, p.boosts[k])?;
                }
                md.set_item("boosts", boosts)?;
                let moves: Vec<(String, i32)> = p.move_slots.iter().map(|m| (d.moves[m.id as usize].id.clone(), m.pp)).collect();
                md.set_item("moves", moves)?;
                md.set_item("can_mega", p.can_mega_evo.is_some())?;
                let vols: Vec<String> = p.volatiles.iter().map(|(e, _)| d.eff(*e).id.clone()).collect();
                md.set_item("volatiles", vols)?;
                mons.append(md)?;
            }
            sdict.set_item("mons", mons)?;
            sides.append(sdict)?;
        }
        out.set_item("sides", sides)?;
        Ok(out)
    }

    #[pyo3(signature = (side, idx, hp=None, status=None, boosts=None, item=None, fainted=None, mega=None, first_turn=None, protect_streak=None, stats=None))]
    #[allow(clippy::too_many_arguments)]
    fn set_mon(
        &mut self, side: usize, idx: usize, hp: Option<i32>, status: Option<&str>, boosts: Option<HashMap<String, i8>>,
        item: Option<&str>, fainted: Option<bool>, mega: Option<bool>, first_turn: Option<bool>, protect_streak: Option<u8>,
        stats: Option<HashMap<String, i32>>,
    ) -> PyResult<()> {
        let m = self.check_mon(side, idx)?;
        if let Some(st) = stats {
            let mut arr = [0i32; 6];
            for (k, v) in st {
                if let Some(pos) = ["hp", "atk", "def", "spa", "spd", "spe"].iter().position(|n| *n == k) {
                    arr[pos] = v;
                }
            }
            self.b.set_stats(m, arr);
        }
        let bo = boosts.map(|map| {
            let mut arr = [0i8; 7];
            for (k, v) in map {
                if let Some(pos) = BOOST_NAMES.iter().position(|n| *n == k) {
                    arr[pos] = v.clamp(-6, 6);
                }
            }
            arr
        });
        if let Some(st) = status {
            if !matches!(st, "" | "brn" | "par" | "psn" | "tox" | "slp" | "frz") {
                return Err(PyValueError::new_err(format!("unknown status {st}")));
            }
        }
        let fainted = fainted.or(if hp == Some(0) { Some(true) } else { None });
        self.b.set_mon_state(m, hp, status, bo, item, fainted, mega, first_turn, protect_streak);
        Ok(())
    }

    fn set_active(&mut self, side: usize, slot: usize, idx: Option<usize>) -> PyResult<()> {
        if let Some(i) = idx {
            self.check_mon(side, i)?;
        }
        self.b.set_active(side.min(1), slot.min(1), idx);
        Ok(())
    }

    #[pyo3(signature = (weather=None, weather_turns=None, terrain=None, terrain_turns=None, trick_room=None))]
    fn set_field(&mut self, weather: Option<&str>, weather_turns: Option<i32>, terrain: Option<&str>, terrain_turns: Option<i32>, trick_room: Option<i32>) -> PyResult<()> {
        let w = match weather {
            Some(x) => Some((weather_id(x)?, weather_turns.unwrap_or(5))),
            None => None,
        };
        let t = match terrain {
            Some(x) => Some((terrain_id(x)?, terrain_turns.unwrap_or(5))),
            None => None,
        };
        self.b.set_field_state(w, t, trick_room);
        Ok(())
    }

    #[pyo3(signature = (side, tailwind=None, reflect=None, light_screen=None, aurora_veil=None, mega_used=None))]
    fn set_side(&mut self, side: usize, tailwind: Option<i32>, reflect: Option<i32>, light_screen: Option<i32>, aurora_veil: Option<i32>, mega_used: Option<bool>) {
        let s = side.min(1);
        for (id, v) in [("tailwind", tailwind), ("reflect", reflect), ("lightscreen", light_screen), ("auroraveil", aurora_veil)] {
            if let Some(turns) = v {
                self.b.set_side_condition(s, id, turns);
            }
        }
        if let Some(x) = mega_used {
            self.b.set_mega_used(s, x);
        }
    }

    fn refresh(&mut self) {
        self.b.refresh();
    }

    fn enable_log(&mut self) {
        self.b.log = Some(vec![]);
    }

    fn log(&self) -> Vec<String> {
        self.b.log.clone().unwrap_or_default()
    }
}

