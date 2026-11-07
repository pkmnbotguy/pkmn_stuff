//! The turn engine: Showdown's Gen 9 doubles flow, with the Champions rule changes.
//!
//! Use: `Battle::new`, `choose_team` for both sides, then repeatedly look at `needs_choice(side)`
//! and call `choose(side, [action; 2])`. The engine runs until it needs decisions again: a new
//! turn, a mid-turn switch (U-turn, Eject Button...), or replacements after faints.

use crate::damage::{crit_blocked, damage_full, Combatant, Conditions, Status, Terrain, Weather};
use crate::dex::{flag, Boosts, Category, Dex, MoveData, Target, ACC, ATK, DEF, EVA, SPA, SPD, SPE};
use crate::effects::{Ab, It, Mv};
use crate::state::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    /// `slot`: move slot 0..=3 (4 = Struggle). `target`: Showdown location: 1, 2 = foe slots,
    /// -1, -2 = own side slots, 0 = no target needed.
    Move { slot: u8, target: i8, mega: bool },
    /// index into the side's brought Pokemon
    Switch { to: u8 },
    Pass,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    TeamPreview,
    /// both sides choose moves for a new turn
    Turn,
    /// some slots must choose a replacement (see `switch_needed`)
    Switch,
    Ended,
}

#[derive(Clone, Copy, Debug)]
enum QKind {
    Switch { to: u8 },
    Mega,
    /// orig: the Pokemon in the target slot at choice time (index, | 0x80 = foe side; NONE = none)
    Move { mslot: u8, target: i8, orig: u8 },
    /// the move of a Pokemon that fainted before acting: never runs, but Showdown keeps it in the queue, so willAct()
    /// (Protect, Wide / Quick Guard: 'does anyone still act?') still sees it (2026-10-17 targeted fuzz).
    /// mv = the chosen move's id (u16::MAX = none): Round's onTry still finds a fainted Pokemon's Round (2026-10-18)
    Dead { mv: u16 },
}

#[derive(Clone, Copy, Debug)]
struct QAction {
    order: u8,
    priority: i8,
    speed: i32,
    tie: u32,
    side: u8,
    slot: u8,
    /// the Pokemon that chose this action: if another one holds the slot by then, it is cancelled
    mon: u8,
    /// Quick Claw activated: first within its priority bracket
    quick: bool,
    /// Stall: last within its priority bracket (Showdown fractional priority -0.1)
    stall: bool,
    kind: QKind,
}

/// Counters for checking the engine against Showdown.
#[derive(Clone, Copy, Debug, Default)]
pub struct Counters {
    pub moves: u32,
    pub crits: u32,
    pub misses: u32,
    pub protected: u32,
    /// hit_target calls (with `protected`: did a move that hit nothing only meet Protect-type blocks?)
    pub hit_calls: u32,
    pub switches: u32,
    pub faints: u32,
    pub cant: u32,
}

/// Optional per-effect statistics (damage, healing, "can't move" reasons), for comparing with Showdown.
#[derive(Clone, Debug, Default)]
pub struct Diag {
    pub damage: std::collections::HashMap<String, (u32, u64)>,
    pub heal: std::collections::HashMap<String, (u32, u64)>,
    pub cant: std::collections::HashMap<String, u32>,
    pub uses: std::collections::HashMap<String, u32>,
}

/// move slot used for a move called by another (Copycat): move_of gives `called_move`
pub const CALL_SLOT: u8 = 6;

#[derive(Clone, Debug)]
pub struct Battle {
    pub sides: [Side; 2],
    pub field: Field,
    pub turn: u16,
    pub rng: Rng,
    pub phase: Phase,
    /// 0 / 1 = that side won, 2 = tie
    pub winner: Option<u8>,
    /// the full teams, for team preview
    pub teams: [Vec<Mon>; 2],
    /// (2026-11-03, v3 13-token net) each side's team members NOT brought (team order), set at team preview; None in
    /// positions built without a preview (from_brought / rebuilt: the bridge sets it with set_roster)
    pub roster: [Option<std::sync::Arc<Vec<Mon>>>; 2],
    /// (2026-11-05, v3 review) each side's whole team as it entered team preview (team order, untouched by the game:
    /// full HP, no status, items, beliefs); None without a preview. Open-sheet rows of unseen members and the beliefs of
    /// unseen / imitated members come from here (self.teams is cleared at preview).
    pub sheet: [Option<std::sync::Arc<Vec<Mon>>>; 2],
    pub switch_needed: [[bool; 2]; 2],
    /// (2026-10-19 bridge flag 16714) the slot's switch request is Revival Blessing's: the player picks a FAINTED Pokemon
    /// (Showdown slotCondition 'revivalblessing'); the engine used to revive the first one without asking
    pub reviving: [[bool; 2]; 2],
    pub counters: Counters,
    pub log: Option<Vec<String>>,
    pub diag: Option<Box<Diag>>,
    /// the side whose Pokemon fainted last (Showdown: if both sides run out together, that side wins)
    last_faint_side: u8,
    /// the last move anyone used (Showdown's battle.lastMove, for Copycat; kept across turns; the called move, not the caller)
    pub last_any_move: u16,
    /// a turn is being played (start_turn .. end of end_of_turn): Showdown's queue still holds the residual action, so a
    /// berry eaten now gives Cud Chew a counter of 2; between turns (replacements) the queue is empty: 1
    pub in_action: bool,
    /// end-of-turn residuals running: Emergency Exit is judged once afterwards against the HP at their start (Showdown)
    pub residual_hp: Option<[[u16; 6]; 2]>,
    /// the weather in force when the current move began (accuracy of every target of a spread move)
    pub acc_weather: Option<Weather>,
    /// the move just run found no live target (Showdown returns before onMoveFail: no Steel Beam / Mind Blown recoil)
    pub no_target_fail: bool,
    /// Dragon Darts in progress: Emergency Exit is judged after both darts (and not at all when they split over two foes)
    pub dart_ee_hold: bool,
    /// Dragon Darts in progress: targets whose Substitute took a dart (a split still runs their AfterMoveSecondary items)
    pub dart_sub: ArrayVec<(usize, usize), 2>,
    /// a seeking Trace resolved during simultaneous replacements: only this foe can be traced
    pub trace_only: Option<(usize, usize)>,
    /// the move's self stat changes (Superpower...) were already applied before the first DamagingHit reaction
    pub self_dropped: bool,
    /// the move just announced costs its user half its HP (Steel Beam, Mind Blown), also when the user is dragged out first
    pub recoil_pending: bool,
    /// counter behind Mon.ab_epoch
    pub ab_epoch_ctr: u32,
    /// Opportunist is copying boosts now (its own rises are not copied back)
    pub opp_flushing: bool,
    /// the move a caller (Copycat) runs through move slot CALL_SLOT
    called_move: u16,
    queue: Vec<QAction>,
    pending: [Option<[Action; 2]>; 2],
    team_pending: [Option<Vec<u8>>; 2],
    /// the switch request came in the middle of a turn: resume the queue afterwards
    midturn: bool,
    /// moves chosen this turn (for Sucker Punch)
    chosen: [[Option<Action>; 2]; 2],
    /// Parting Shot lowered nothing: no switch (Showdown deletes move.selfSwitch)
    cancel_self_switch: bool,
    /// the Pokemon whose move is running (for move results)
    cur_user: (u8, u8),
    /// the target's HP before the current hit (Emergency Exit needs the crossing)
    hp_before_hit: u16,
    /// the target's ability just before the hit: a KO'd Pokemon's Transform is undone at once by faint(), Showdown faints
    /// after the move, so its DamagingHit abilities (a Ditto copying Gooey) still fire
    ab_before_hit: Ab,
    /// Grassy Terrain was up when the end of the turn began: Showdown collects the residual handlers once, so a terrain
    /// set during the residuals (Seed Sower hit by a landing Future Sight) heals from the next turn on
    eot_grassy: bool,
    /// damage of this move's earlier hits on the target (Innards Out adds move.totalDamage; not Dragon Darts)
    innards_prev: u16,
    /// this move is boosted by a Gem
    gem_active: bool,
    /// a Future Sight hit is resolving (its user may be on the bench or fainted)
    future_now: bool,
    /// a multi-hit move already ran its secondaries after each hit (post_hit skips them)
    sec_done: bool,
    /// this hit goes through Protect (Unseen Fist / Piercing Drill): x0.25 damage
    protect_bypass: bool,
    /// the hit being reacted to was a critical hit (Anger Point)
    hit_crit: bool,
    /// Fling: the flung item's power and id (u16::MAX = none)
    fling_bp: u16,
    fling_item: u16,
    /// Fling: the flung item's kind. Showdown removes it only at the next Update (after the hit), so its damage boost
    /// (Life Orb, Expert Belt, plates...) still applies; Symbiosis' replacement comes after the hit
    fling_kind: It,
    /// Analytic, fixed once per move (Showdown computes a spread move's damage on every target before any faint, so a
    /// partner KO'd by the same Surf still 'will move'); None = check the queue at the hit
    analytic_move: Option<bool>,
    /// Red Card held by a Pokemon hit this move: (side, index) of the holder
    red_card: Option<(u8, u8)>,
    /// the move's target was redirected by Follow Me / Rage Powder (they turn off Dragon Darts' split: smartTarget = false)
    follow_redirect: bool,
    /// (user side, user, target side, target): the Pokemon in the chosen target slot when the turn's actions were made
    /// (Showdown action.originalTarget; Stalwart / Propeller Tail / Snipe Shot follow it wherever it is now)
    orig_target: Option<(u8, u8, u8, u8)>,
    /// the status being set comes from Toxic Spikes (Synchronize ignores it)
    status_from_tspikes: bool,
    /// Dragon Darts: hits per target forced to this (one dart each), and a dart that already passed its accuracy check
    forced_hits: Option<u8>,
    skip_accuracy: bool,
    /// this move hits twice through Parental Bond
    parental: bool,
    /// a Round pulled forward by another Round (side, Pokemon): double power
    round_next: Option<(u8, u8)>,
    /// the move running now is such a boosted Round
    round_boost_now: bool,
    /// the type of the move running now (after ModifyType), computed once per use
    cur_type: crate::dex::TypeId,
    /// hits of single-hit moves: post-hit effects and reactions wait until all targets are hit
    defer_hits: bool,
    /// (2026-10-15) Pokemon (bit per mon index, per side) knocked out by the current spread move: Showdown computes every
    /// target's damage before applying any, so their field abilities (Fairy Aura, Friend Guard...) still count for the rest
    spread_ko: [u8; 2],
    /// their ability before fainting (faint undoes Transform at once: a KO'd Ditto's copied Friend Guard still counts)
    spread_ko_ab: [[Ab; 8]; 2],
    /// the user of the running damaging move as it was just before it fainted mid-move (Liquid Ooze, Rocky Helmet...):
    /// Showdown computes a spread move's damage for every target before applying any, so the later targets still see its
    /// Charge / Helping Hand / boosts / Transform
    spread_user_snap: Option<Box<Mon>>,
    /// simultaneous switch-ins: Mimicry (-1) and Forecast (-2) wait for every priority-0 switch-in effect
    defer_low_prio: bool,
    /// consume_berry_in_hit: Symbiosis reaches a holder KO'd by the same hit
    symb_ko_ok: bool,
    /// Pokemon a spread move knocked out (Moxie / Eelevate boost once the move's hits are done, Showdown's faint timing)
    deferred_kos: u8,
    deferred_post: ArrayVec<(usize, usize, bool), 4>,
    deferred_dh: ArrayVec<(usize, usize, i32, u16, bool, Ab), 4>,
    /// spread move: Eject Button holders hit, resolved after AfterHit in speed order (Showdown AfterMoveSecondary handlers are speed-sorted)
    eject_defer: bool,
    /// the current move was stopped in BeforeMove (Showdown then runs MoveAborted: Charge ends on an Electric move)
    move_aborted: bool,
    /// AfterMove found no handlers (mover and its target both fainted / gone): White Herb waits (2026-10-17)
    herb_skip: bool,
    /// the last boost() had a drop stopped by an ability (Clear Body, White Smoke, Hyper Cutter...; not Mirror Armor)
    boost_blocked: bool,
    // (2026-10-17) the boost being applied was bounced by Mirror Armor (Showdown: effect = Mirror Armor, which then ignores it)
    mirror_bounce: bool,
    eject_cands: ArrayVec<(usize, usize), 4>,
    /// (2026-11-05, sim report) Red Card holders hit by one spread move: Showdown's AfterMoveSecondary runs over all targets
    /// at once, speed-sorted, so the fastest holder's card fires (not the first one hit)
    card_defer: bool,
    card_cands: ArrayVec<(usize, usize), 4>,
    /// hits that landed on the last target (Parental Bond's self drops are per hit)
    last_hits: u8,
    /// (2026-10-13) chance enumeration for the search: None = ordinary random play
    pub chance: Option<Box<ChanceCtl>>,
}

/// Chance enumeration (search cells valued by exact probabilities instead of sampled rolls). At the important random
/// events (accuracy, critical hits, secondary effects, KO-deciding damage rolls, full paralysis, confusion) the engine
/// asks `branch(p)`: outcomes listed in `script` are forced in order; past the script a new branch point is recorded
/// (outcome = the more likely one) while the path's probability is >= `min_path`; below it the ordinary random draw is
/// used (an unbiased stand-in for the rest of that subtree). Damage rolls that can't decide a KO use the middle roll.
#[derive(Clone, Debug, Default)]
pub struct ChanceCtl {
    pub script: Vec<u8>,
    pub pos: usize,
    /// every branch taken on this path: (outcome, probability of `true`)
    pub taken: Vec<(u8, f32)>,
    pub path: f64,
    pub min_path: f64,
    /// (2026-10-22) stop at end-of-turn replacements (policy::play_turn) instead of drawing them as chance: they are
    /// decisions; the caller solves them (search_leaves_chance_repl)
    pub stop_repl: bool,
}

use arrayvec::ArrayVec;

/// Targets of one move: at most three Pokemon (no heap allocation).
pub type Targets = ArrayVec<(usize, usize), 4>;
/// A short list of Pokemon (speed order, entries...).
type MonList = ArrayVec<(usize, usize), 4>;

fn one(t: (usize, usize)) -> Targets {
    let mut v = Targets::new();
    v.push(t);
    v
}

const ORDER_SWITCH: u8 = 103;
const ORDER_MEGA: u8 = 104;
const ORDER_MOVE: u8 = 200;

fn tid(dex: &Dex, name: &str) -> u8 {
    dex.type_id(name)
}

/// Build a team's Pokemon once (names resolved, stats computed); reuse with `Battle::from_mons`.
pub fn build_team(dex: &Dex, sets: &[SetData]) -> Result<Vec<Mon>, String> {
    sets.iter().map(|set| make_mon(dex, set)).collect()
}

impl Battle {
    /// chance enumeration: Some(outcome) when this event is enumerated (see ChanceCtl), None = draw it as usual
    #[inline]
    fn branch(&mut self, p: f64) -> Option<bool> {
        let c = self.chance.as_deref_mut()?;
        if p <= 0.0 {
            return Some(false);
        }
        if p >= 1.0 {
            return Some(true);
        }
        let o = if c.pos < c.script.len() {
            c.script[c.pos]
        } else if c.path >= c.min_path {
            (p >= 0.5) as u8
        } else {
            return None;
        };
        c.pos += 1;
        c.taken.push((o, p as f32));
        c.path *= if o == 1 { p } else { 1.0 - p };
        Some(o == 1)
    }
    /// `num` in `den` (enumerated or drawn)
    #[inline]
    fn chance_of(&mut self, num: u32, den: u32) -> bool {
        match self.branch(num as f64 / den as f64) {
            Some(o) => o,
            None => self.rng.chance(num, den),
        }
    }
    /// uniform index in [0, n): two-way picks enumerated, others drawn
    #[inline]
    pub fn pick(&mut self, n: usize) -> usize {
        if n == 2 {
            if let Some(o) = self.branch(0.5) {
                return o as usize;
            }
        }
        self.rng.below(n as u32) as usize
    }
    /// below(100) < pct (enumerated or drawn)
    #[inline]
    fn pct(&mut self, pct: i64) -> bool {
        match self.branch(pct as f64 / 100.0) {
            Some(o) => o,
            None => (self.rng.below(100) as i64) < pct,
        }
    }
    pub fn new(dex: &Dex, teams: [&[SetData]; 2], seed: u64) -> Result<Battle, String> {
        let a = build_team(dex, teams[0])?;
        let b = build_team(dex, teams[1])?;
        Ok(Battle::from_mons([&a, &b], seed))
    }

    /// A new battle from teams already built with `build_team` (much cheaper than parsing names
    /// again for every game).
    pub fn from_mons(teams: [&[Mon]; 2], seed: u64) -> Battle {
        let mut built: [Vec<Mon>; 2] = [teams[0].to_vec(), teams[1].to_vec()];
        let mut rng = Rng::new(seed);
        for team in built.iter_mut() {
            for m in team.iter_mut() {
                if m.gender == 3 {
                    m.gender = if rng.below(2) == 1 { 2 } else { 1 };   // this.battle.random(2) ? 'F' : 'M'
                }
            }
        }
        Battle {
            sides: [Side::default(), Side::default()],
            field: Field::default(),
            turn: 0,
            rng,
            phase: Phase::TeamPreview,
            winner: None,
            teams: built,
            roster: [None, None],
            sheet: [None, None],
            switch_needed: [[false; 2]; 2],
            reviving: [[false; 2]; 2],
            counters: Counters::default(),
            log: None,
            diag: None,
            last_faint_side: 0,
            last_any_move: u16::MAX,
            in_action: false,
            residual_hp: None,
            acc_weather: None,
            no_target_fail: false,
            dart_ee_hold: false,
            dart_sub: ArrayVec::new(),
            trace_only: None,
            self_dropped: false,
            recoil_pending: false,
            ab_epoch_ctr: 0,
            opp_flushing: false,
            called_move: u16::MAX,
            queue: Vec::new(),
            pending: [None, None],
            team_pending: [None, None],
            midturn: false,
            chosen: [[None; 2]; 2],
            cancel_self_switch: false,
            cur_user: (0, 0),
            hp_before_hit: 0,
            ab_before_hit: Ab::None,
            eot_grassy: false,
            innards_prev: 0,
            gem_active: false,
            future_now: false,
            sec_done: false,
            protect_bypass: false,
            hit_crit: false,
            fling_bp: 0,
            fling_item: u16::MAX,
            fling_kind: It::None,
            analytic_move: None,
            red_card: None,
            follow_redirect: false,
            orig_target: None,
            status_from_tspikes: false,
            forced_hits: None,
            skip_accuracy: false,
            parental: false,
            round_next: None,
            round_boost_now: false,
            cur_type: crate::dex::NO_TYPE,
            defer_hits: false,
            spread_ko: [0; 2],
            spread_ko_ab: [[Ab::None; 8]; 2],
            spread_user_snap: None,
            defer_low_prio: false,
            symb_ko_ok: false,
            deferred_kos: 0,
            deferred_post: ArrayVec::new(),
            deferred_dh: ArrayVec::new(),
            eject_defer: false,
            move_aborted: false,
            herb_skip: false,
            boost_blocked: false,
            mirror_bounce: false,
            eject_cands: ArrayVec::new(),
            card_defer: false,
            card_cands: ArrayVec::new(),
            last_hits: 0,
            chance: None,
        }
    }

    /// A battle already in progress, for search: each side's brought Pokemon (leads first),
    /// actives = the first two, phase = choose moves. No switch-in effects are run; set HP,
    /// boosts, field etc. afterwards to match the real position.
    pub fn from_brought(dex: &Dex, brought: [&[SetData]; 2], seed: u64, turn: u16) -> Result<Battle, String> {
        let mut b = Battle::new(dex, brought, seed)?;
        for s in 0..2 {
            b.sides[s].mons = b.teams[s].iter().take(6).cloned().collect();
            b.sides[s].active = [0, if b.sides[s].mons.len() > 1 { 1 } else { NONE }];
            // (the leads came out with the whole team alive: Illusion is on, as in a real battle start)
            for k in 0..2 {
                if let Some(i) = b.active(s, k) {
                    b.set_illusion(s, k, i);
                    // (the Metronome item's volatile, as if its Start had run at switch-in)
                    if b.mon(s, i).has_item() && b.mon(s, i).item == It::Metronome {
                        b.metro_start(s, i);
                    }
                }
            }
        }
        b.turn = turn.max(1);
        b.phase = Phase::Turn;
        Ok(b)
    }

    /// A copy for searching from a decision point: choices already sent are dropped, and with
    /// `swap` the two sides trade places (so a search that scores for side 0 can play as side 1).
    pub fn search_copy(&self, swap: bool) -> Battle {
        let mut b = self.clone();
        b.pending = [None, None];
        b.log = None;
        b.diag = None;
        if swap {
            b.sides.swap(0, 1);
            b.switch_needed.swap(0, 1);
            b.team_pending.swap(0, 1);
            b.chosen.swap(0, 1);
            b.teams.swap(0, 1);
            b.roster.swap(0, 1);
            b.last_faint_side = 1 - b.last_faint_side.min(1);
            b.winner = b.winner.map(|w| if w < 2 { 1 - w } else { w });
            for q in b.queue.iter_mut() {
                q.side = 1 - q.side;
            }
            // timer setters are stored by absolute side
            let flip = |v: &mut u8| if *v != 0 { let x = *v - 1; *v = 1 + (1 - x / 8) * 8 + x % 8; };
            flip(&mut b.field.weather_src);
            flip(&mut b.field.terrain_src);
            for sd in b.sides.iter_mut() {
                for v in sd.screen_src.iter_mut() {
                    flip(v);
                }
            }
        }
        b
    }

    /// (2026-10-28, hidden timers) Weather, terrain and screens last 5 turns, 8 when the setter held its rock / Terrain
    /// Extender / Light Clay. For `me` the length is unknown when the setter is an opponent whose item `me` can't know
    /// (closed sheets) and the timer hasn't outlasted 5 turns yet (that proves the item). Returns the turns already
    /// elapsed in that case, None when `me` knows the turns left.
    pub fn timer_hidden(&self, dex: &Dex, src: u8, ext: bool, turns: u8, me: usize) -> Option<u8> {
        if src == 0 || turns == 0 {
            return None;
        }
        let (ss, si) = (((src - 1) / 8) as usize, ((src - 1) % 8) as usize);
        if ss == me || ss > 1 || si >= self.sides[ss].mons.len() || crate::views::item_known(dex, &self.sides[ss].mons[si]) {
            return None;
        }
        let e = (if ext { 8u8 } else { 5 }).saturating_sub(turns);
        if e >= 5 { None } else { Some(e) }
    }

    /// the timers' (turns, src, ext, P(item) prior) in the order weather, terrain, side 0 Reflect / Light Screen / Aurora Veil,
    /// side 1 the same. Priors = share of Teams_real Pokemon able to set that timer who hold the item (2026-10-28).
    fn timers(&self) -> [(u8, u8, bool, f32); 8] {
        let pw = match self.field.weather {
            Weather::Rain => 0.059,
            Weather::Sun => 0.016,
            Weather::Sand => 0.003,
            Weather::Snow => 0.009,
            Weather::None => 0.0,
        };
        let f = &self.field;
        let mut out = [(f.weather_turns, f.weather_src, f.weather_ext, pw), (f.terrain_turns, f.terrain_src, f.terrain_ext, 0.004),
            (0, 0, false, 0.0), (0, 0, false, 0.0), (0, 0, false, 0.0), (0, 0, false, 0.0), (0, 0, false, 0.0), (0, 0, false, 0.0)];
        for s in 0..2 {
            let sd = &self.sides[s];
            for (k, t) in [sd.reflect, sd.light_screen, sd.aurora_veil].into_iter().enumerate() {
                out[2 + s * 3 + k] = (t, sd.screen_src[k], sd.screen_ext[k], 0.468);
            }
        }
        out
    }

    /// The turns left as `me` can know them (network input): exact, or the expected value 5 - e + 3 p when hidden.
    /// Order: weather, terrain, side 0 Reflect / Light Screen / Aurora Veil, side 1 the same.
    pub fn timers_seen(&self, dex: &Dex, me: usize) -> [f32; 8] {
        let mut out = [0f32; 8];
        for (k, (t, src, ext, p)) in self.timers().into_iter().enumerate() {
            out[k] = match self.timer_hidden(dex, src, ext, t, me) {
                Some(e) => (5 - e) as f32 + 3.0 * p,
                None => t as f32,
            };
        }
        out
    }

    /// (2026-10-28, hidden timers) a search world for `me`: every timer whose length `me` can't know gets a length drawn
    /// from the prior (the true item is not consulted), keeping the turns already elapsed.
    pub fn hide_timers(&mut self, dex: &Dex, me: usize, rng: &mut Rng) {
        for (k, (t, src, ext, p)) in self.timers().into_iter().enumerate() {
            if let Some(e) = self.timer_hidden(dex, src, ext, t, me) {
                let x = (rng.below(100_000) as f32) < p * 100_000.0;
                let left = (if x { 8u8 } else { 5 }) - e;
                match k {
                    0 => {
                        self.field.weather_turns = left;
                        self.field.weather_ext = x;
                    }
                    1 => {
                        self.field.terrain_turns = left;
                        self.field.terrain_ext = x;
                    }
                    _ => {
                        let (s, j) = ((k - 2) / 3, (k - 2) % 3);
                        let sd = &mut self.sides[s];
                        match j {
                            0 => sd.reflect = left,
                            1 => sd.light_screen = left,
                            _ => sd.aurora_veil = left,
                        }
                        sd.screen_ext[j] = x;
                    }
                }
            }
        }
    }

    /// After editing a position by hand: re-check the winner and whether replacements are needed.
    pub fn refresh(&mut self) {
        self.check_win();
        if self.winner.is_some() {
            return;
        }
        self.switch_needed = [[false; 2]; 2];
        self.reviving = [[false; 2]; 2];
        self.phase = Phase::Turn;
        for s in 0..2 {
            for k in 0..2 {
                let i = self.sides[s].active[k];
                if i != NONE && !self.sides[s].mons[i as usize].alive() {
                    self.request_switch(s, k);
                }
                // a position built from outside with Magic Room running (set_extra only sets the timer): every held item is put
                // aside, as magic_room_start does (2026-10-18 bridge fuzzing: Leftovers kept healing under Magic Room)
                if self.field.magic_room > 0 {
                    for m in self.sides[s].mons.iter_mut() {
                        if m.item_id != u16::MAX && m.stash_item.0 == u16::MAX {
                            m.stash_item = (m.item_id, m.item);
                            m.item_id = u16::MAX;
                            m.item = It::None;
                        }
                    }
                }
                // a position built from outside never ran its leads' switch-in effects: a Klutz holder's item is put aside here
                if i != NONE {
                    let m = &mut self.sides[s].mons[i as usize];
                    if m.ability == Ab::Klutz && m.item_id != u16::MAX && m.stash_item.0 == u16::MAX {
                        m.stash_item = (m.item_id, m.item);
                        m.item_id = u16::MAX;
                        m.item = It::None;
                    }
                }
            }
        }
    }

    fn add(&mut self, f: impl FnOnce() -> String) {
        if let Some(log) = &mut self.log {
            log.push(f());
        }
    }

    #[inline(always)]
    pub fn mon(&self, s: usize, i: usize) -> &Mon {
        &self.sides[s].mons[i]
    }
    #[inline(always)]
    fn mon_mut(&mut self, s: usize, i: usize) -> &mut Mon {
        &mut self.sides[s].mons[i]
    }
    /// The Pokemon in an active slot, if it's there and alive
    #[inline(always)]
    pub fn active(&self, s: usize, slot: usize) -> Option<usize> {
        let i = self.sides[s].active[slot];
        if i != NONE && self.sides[s].mons[i as usize].alive() { Some(i as usize) } else { None }
    }
    fn slot_of(&self, s: usize, i: usize) -> Option<usize> {
        (0..2).find(|&k| self.sides[s].active[k] == i as u8)
    }

    // ------------------------------------------------------------------
    // Requests
    // ------------------------------------------------------------------
    /// the pending switch request came in the middle of a turn (U-turn, Eject Button...)
    pub fn is_midturn(&self) -> bool {
        self.midturn
    }

    pub fn needs_choice(&self, side: usize) -> bool {
        match self.phase {
            Phase::TeamPreview => self.team_pending[side].is_none(),
            Phase::Turn => self.pending[side].is_none() && (0..2).any(|k| self.active(side, k).is_some()),
            Phase::Switch => self.pending[side].is_none() && self.switch_needed[side].iter().any(|&x| x),
            Phase::Ended => false,
        }
    }

    /// Bring 4 of the 6 (indices into the team, leads first).
    pub fn choose_team(&mut self, dex: &Dex, side: usize, order: &[u8]) {
        self.team_pending[side] = Some(order.to_vec());
        if self.team_pending.iter().all(|t| t.is_some()) {
            for s in 0..2 {
                let order = self.team_pending[s].take().unwrap();
                let np = self.teams[s].len().min(6);
                for k in 0..np {
                    self.sides[s].preview[k] = self.teams[s][k].species;
                }
                self.sides[s].n_preview = np as u8;
                self.sides[s].mons = order.iter().take(4).map(|&i| self.teams[s][i as usize].clone()).collect();
                let left: Vec<Mon> = (0..np).filter(|i| !order.iter().take(4).any(|&o| o as usize == *i))
                    .map(|i| self.teams[s][i].clone()).collect();
                self.roster[s] = Some(std::sync::Arc::new(left));
                self.sheet[s] = Some(std::sync::Arc::new(self.teams[s][..np].to_vec()));
                self.sides[s].active = [0, if self.sides[s].mons.len() > 1 { 1 } else { NONE }];
                for k in 0..2 {
                    if let Some(i) = self.active(s, k) {
                        self.set_illusion(s, k, i);
                    }
                }
            }
            // the full teams are not needed any more (and would be copied with every search branch)
            self.teams = [Vec::new(), Vec::new()];
            self.turn = 1;
            // switch-in effects for all four leads, fastest first
            let mut leads = MonList::new();
            for s in 0..2 {
                for k in 0..2 {
                    if let Some(i) = self.active(s, k) {
                        leads.push((s, i));
                    }
                }
            }
            self.sort_by_speed(dex, &mut leads);
            self.run_switch_ins(dex, &leads);
            for &(s, i) in leads.iter() {
                self.seed_check(dex, s, i);
            }
            self.update_items(dex);
            self.phase = Phase::Turn;
            self.add(|| "|turn|1".into());
        }
    }

    pub fn choose(&mut self, dex: &Dex, side: usize, actions: [Action; 2]) {
        self.pending[side] = Some(actions);
        let ready = (0..2).all(|s| !self.needs_choice(s));
        if !ready {
            return;
        }
        match self.phase {
            Phase::Turn => self.start_turn(dex),
            Phase::Switch => self.apply_switch_choices(dex),
            _ => {}
        }
    }

    // ------------------------------------------------------------------
    // Speed and order
    // ------------------------------------------------------------------
    pub fn speed(&self, dex: &Dex, s: usize, i: usize) -> u32 {
        let m = self.mon(s, i);
        let mut spe = crate::stats::boosted(m.stats[SPE] as u32, m.boosts[SPE]) as i64;
        let mut chain = 4096i64;
        let mul = |c: &mut i64, x: i64| *c = (*c * x + 2048) >> 12;
        if self.sides[s].tailwind > 0 {
            mul(&mut chain, 8192);
        }
        // (2026-10-18 targeted fuzz 3847564) a fainted Pokemon's own handlers are skipped (Showdown runEvent: effectHolder
        // fainted -> continue; only slot conditions run): the speed that orders simultaneous replacements (the switch
        // action belongs to the fainted one) ignores its Iron Ball / Scarf / ability / paralysis; Tailwind is the side's
        if !m.alive() {
            if chain != 4096 {
                spe = crate::damage::modify(spe, chain);
            }
            return spe.clamp(0, 10000) as u32;
        }
        if m.item == It::ChoiceScarf {
            mul(&mut chain, 6144);
        }
        if m.item == It::IronBall {
            mul(&mut chain, 2048);
        }
        if m.ability == Ab::Quickfeet && m.status != Status::None {
            mul(&mut chain, 6144);
        }
        let w = self.weather();
        let doubled = match m.ability {
            Ab::Chlorophyll => w == Weather::Sun,
            Ab::SwiftSwim => w == Weather::Rain,
            Ab::SandRush => w == Weather::Sand,
            Ab::SlushRush => w == Weather::Snow,
            // (2026-10-17 targeted fuzz) Showdown: doubled only while it holds nothing (!pokemon.item): a Pokemon that lost
            // its berry and then stole a Choice Scarf with Thief gets the Scarf alone; an item hidden by Magic Room / Klutz
            // is still held
            Ab::Unburden => m.has_vol(vol::UNBURDEN) && !m.has_item() && m.stash_item.0 == u16::MAX,
            Ab::SurgeSurfer => self.field.terrain == Terrain::Electric,
            _ => false,
        };
        if doubled {
            mul(&mut chain, 8192);
        }
        if chain != 4096 {
            spe = crate::damage::modify(spe, chain);
        }
        // paralysis after every other modifier (Showdown onModifySpePriority -101: finalModify, then floor(x 50 / 100))
        if m.status == Status::Paralysis && m.ability != Ab::Quickfeet {
            spe = spe * 50 / 100;
        }
        spe.clamp(0, 10000) as u32
    }

    /// Infatuate (s, i) with (ts, ti): opposite genders only; Oblivious / Aroma Veil block; Mental Herb cures it at once.
    fn try_attract(&mut self, dex: &Dex, s: usize, i: usize, ts: usize, ti: usize) -> bool {
        let (g, gs) = (self.mon(s, i).gender, self.mon(ts, ti).gender);
        let opposite = (g == 1 && gs == 2) || (g == 2 && gs == 1);
        if !opposite || self.mon(s, i).vol2 & vol2::ATTRACT != 0 || self.mon(s, i).ability == Ab::Oblivious
            || self.side_has(s, |a| a == Ab::Aromaveil)
        {
            return false;
        }
        let m = self.mon_mut(s, i);
        m.vol2 |= vol2::ATTRACT;
        m.attract_src = ti as u8;
        m.attract_side = ts as u8;
        self.add(|| format!("|-start|{s}|{i}|Attract"));
        let m = self.mon(s, i);
        if m.has_item() && m.item == It::MentalHerb {
            self.mon_mut(s, i).vol2 &= !vol2::ATTRACT;
            self.consume_item(s, i);
        }
        let _ = dex;
        true
    }

    /// A move type set by its user's state: Electrify (Electric), Raging Bull (Fighting / Fire / Water for Paldean Tauros).
    fn species_move_type(&self, dex: &Dex, s: usize, i: usize, mv: &MoveData) -> Option<crate::dex::TypeId> {
        if self.mon(s, i).vol2 & vol2::ELECTRIFY != 0 && mv.id != "struggle" {
            return Some(dex.ty.electric);
        }
        if mv.kind != crate::effects::Mv::Ragingbull {
            return None;
        }
        match dex.species[self.mon(s, i).species as usize].id.as_str() {
            "taurospaldeacombat" => Some(dex.ty.fighting),
            "taurospaldeablaze" => Some(dex.ty.fire),
            "taurospaldeaaqua" => Some(dex.ty.water),
            _ => None,
        }
    }

    /// The move's type after ModifyType (Weather Ball, Terrain Pulse, -ate abilities...).
    fn eff_type(&self, dex: &Dex, s: usize, i: usize, mv: &MoveData) -> crate::dex::TypeId {
        // (2026-10-18 speed) fast path: only these moves / abilities / Electrify can change a type in resolve_move
        let m = self.mon(s, i);
        if !matches!(mv.kind, Mv::Aurawheel | Mv::Ragingbull | Mv::WeatherBall | Mv::TerrainPulse | Mv::Struggle)
            && !matches!(m.ability, Ab::Aerilate | Ab::Pixilate | Ab::Refrigerate | Ab::Galvanize | Ab::Dragonize | Ab::LiquidVoice)
            && m.vol2 & vol2::ELECTRIFY == 0
        {
            return mv.type_;
        }
        let hangry = dex.species[self.mon(s, i).species as usize].id == "morpekohangry";
        let cond = Conditions { weather: self.weather(), terrain: self.field.terrain, hangry,
                                species_move_type: self.species_move_type(dex, s, i, mv), ..Default::default() };
        crate::damage::resolve_move(dex, mv, &self.combatant(dex, s, i), &cond).type_
    }

    fn action_speed(&self, dex: &Dex, s: usize, i: usize) -> i32 {
        let v = self.speed(dex, s, i) as i32;
        if self.field.trick_room > 0 { -v } else { v }
    }

    fn sort_by_speed(&mut self, dex: &Dex, list: &mut [(usize, usize)]) {
        let mut keyed: ArrayVec<(i32, u32, (usize, usize)), 8> = ArrayVec::new();
        for &(s, i) in list.iter() {
            let sp = self.action_speed(dex, s, i);
            let tie = self.rng.next_u32();
            keyed.push((sp, tie, (s, i)));
        }
        keyed.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        for (slot, k) in list.iter_mut().zip(keyed.iter()) {
            *slot = k.2;
        }
    }

    pub fn weather(&self) -> Weather {
        if self.field.weather == Weather::None {
            return Weather::None;
        }
        // Cloud Nine / Air Lock on the field suppress weather
        for s in 0..2 {
            for k in 0..2 {
                if let Some(i) = self.active(s, k) {
                    // (Showdown suppressingWeather: !pokemon.fainted)
                    if matches!(self.mon(s, i).ability, Ab::CloudNine | Ab::AirLock) && !self.mon(s, i).fainted {
                        return Weather::None;
                    }
                } else {
                    // a holder KO'd by the move still resolving: Showdown sets 'fainted' only after the move (faintMessages),
                    // so the rest of a spread move still sees no weather (Hyper Voice KOs Cloud Nine Altaria: the partner
                    // takes no rain boost; 2026-10-17 Zoroark fuzz 5063196)
                    let raw = self.sides[s].active[k];
                    if self.defer_hits && raw != NONE && (raw as usize) < 8 && self.spread_ko[s] & (1 << raw) != 0
                        && matches!(self.spread_ko_ab[s][raw as usize], Ab::CloudNine | Ab::AirLock)
                    {
                        return Weather::None;
                    }
                }
            }
        }
        self.field.weather
    }

    pub(crate) fn is_grounded(&self, dex: &Dex, s: usize, i: usize) -> bool {
        self.grounded_brk(dex, s, i, false)
    }

    /// A move from (ss, si) ignores (s, i)'s breakable abilities (Mold Breaker, Teravolt, Turboblaze; Showdown suppressingAbility)
    fn breaks_vs(&self, s: usize, i: usize, src: Option<(usize, usize)>) -> bool {
        src.is_some_and(|(ss, si)| (ss, si) != (s, i) && matches!(self.mon(ss, si).ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze))
    }

    /// isGrounded; brk = a Mold Breaker user's move is resolving, so Levitate / Eelevate (breakable) don't lift the target
    /// (Misty / Electric Terrain then protect it from status and confusion; 2026-10-17 targeted fuzz)
    fn grounded_brk(&self, dex: &Dex, s: usize, i: usize, brk: bool) -> bool {
        let m = self.mon(s, i);
        if m.item == It::IronBall || self.field.gravity > 0 || m.vol2 & (vol2::SMACK_DOWN | vol2::INGRAIN) != 0 {
            return true;
        }
        let flying = m.has_type(dex.ty.flying) && !m.has_vol(vol::ROOST);
        !(flying || m.magnet_rise > 0 || (!brk && matches!(m.ability, Ab::Levitate | Ab::Eelevate)) || m.item == It::AirBalloon)
    }

    /// Can't switch out (Showdown's trapping): a foe's Shadow Tag (unless it has Shadow Tag too),
    /// Arena Trap (if grounded), Magnet Pull (if Steel), or a binding move whose user is still in.
    /// Ghost types and Shed Shell holders are never trapped.
    /// (s, i) holds Showdown's 'trapped' volatile: No Retreat's own (no source) or one linked to a source that is
    /// still on the field (Mean Look, Block, Jaw Lock, Spirit Shackle...; it ends when the source leaves or faints)
    pub fn has_trapped_vol(&self, s: usize, i: usize) -> bool {
        let m = self.mon(s, i);
        m.trap_src == TRAP_SELF
            || (m.trap_src != NONE && self.sides[m.trap_side as usize].active.contains(&m.trap_src)
                && self.mon(m.trap_side as usize, m.trap_src as usize).alive())
    }

    pub fn trapped(&self, dex: &Dex, s: usize, i: usize) -> bool {
        let m = self.mon(s, i);
        // locked into Outrage & co. / Uproar: hard locked (Showdown getMoveRequestData: trapped, even Ghosts / Shed Shell)
        // (2026-10-18) recharging after Giga Impact & co.: LockMove 'recharge' = hard locked too
        if m.lock_slot != NONE || m.uproar > 0 || m.has_vol(vol::MUST_RECHARGE) {
            return true;
        }
        if m.has_type(dex.ty.ghost) || m.ability == Ab::Runaway {
            return false;
        }
        if m.item_id != u16::MAX && dex.items[m.item_id as usize].id == "shedshell" {
            return false;
        }
        if m.trap_src == TRAP_SELF || m.vol2 & vol2::INGRAIN != 0 || self.field.fairy_lock > 0 {
            return true;
        }
        let bs = m.bound_side as usize;
        if m.bound > 0 && m.bound_src != NONE && self.sides[bs].active.contains(&m.bound_src)
            && self.mon(bs, m.bound_src as usize).alive() {
            return true;
        }
        if m.octolock_src != NONE && self.sides[m.octolock_side as usize].active.contains(&m.octolock_src) {
            return true;
        }
        if m.trap_src != NONE && m.trap_src != TRAP_SELF && self.sides[m.trap_side as usize].active.contains(&m.trap_src) && self.mon(m.trap_side as usize, m.trap_src as usize).alive() {
            return true;
        }
        let own = dex.abilities.get(m.ability_id as usize).map(|a| a.id.as_str()).unwrap_or("");
        for k in 0..2 {
            let Some(f) = self.active(1 - s, k) else { continue };
            let fa = dex.abilities.get(self.mon(1 - s, f).ability_id as usize).map(|a| a.id.as_str()).unwrap_or("");
            match fa {
                "shadowtag" if own != "shadowtag" => return true,
                "arenatrap" if self.is_grounded(dex, s, i) => return true,
                "magnetpull" if m.has_type(dex.ty.steel) => return true,
                _ => {}
            }
        }
        false
    }

    /// The 16 damage rolls of (s, i)'s move in `slot` against (ts, ti), computed as the real hit would
    /// be (current stats, boosts, weather, terrain, screens, abilities, spread reduction for the move's
    /// actual targets; no critical hit, no side effects). None for status moves, type immunities,
    /// absorbing abilities and ungrounded targets of Ground moves.
    pub fn preview_rolls(&self, dex: &Dex, s: usize, i: usize, slot: usize, ts: usize, ti: usize) -> Option<[u16; 16]> {
        self.preview_rolls_opt(dex, s, i, slot, ts, ti, false)
    }

    /// `state_bp`: also apply base powers computed from the battle state (Flail / Reversal, Gyro Ball, Stored Power...).
    /// The network's damage features use preview_rolls WITHOUT it (the trained nets saw it that way); the bridge's
    /// Substitute estimate (Battle.damage_rolls) uses it. (2026-10-18)
    pub fn preview_rolls_opt(&self, dex: &Dex, s: usize, i: usize, slot: usize, ts: usize, ti: usize, state_bp: bool) -> Option<[u16; 16]> {
        self.preview_rolls_spread(dex, s, i, slot, ts, ti, state_bp, None)
    }

    /// `spread`: Some(true / false) = the hit was / wasn't spread (0.75x), None = as it would be in this position.
    #[allow(clippy::too_many_arguments)]
    pub fn preview_rolls_spread(&self, dex: &Dex, s: usize, i: usize, slot: usize, ts: usize, ti: usize, state_bp: bool,
                                spread: Option<bool>) -> Option<[u16; 16]> {
        let id = *self.mon(s, i).moves.get(slot)?;
        if id == u16::MAX {
            return None;
        }
        let mv = &dex.moves[id as usize];
        if mv.category == Category::Status {
            return None;
        }
        let t = self.eff_type(dex, s, i, mv);
        let att_ab = self.mon(s, i).ability;
        let mold = matches!(att_ab, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze);
        let tab = self.mon(ts, ti).ability;
        let absorbed = !mold && match tab {
            Ab::Voltabsorb | Ab::Lightningrod | Ab::Motordrive => t == dex.ty.electric,
            Ab::Waterabsorb | Ab::Dryskin | Ab::Stormdrain => t == dex.ty.water,
            Ab::Sapsipper => t == dex.ty.grass,
            Ab::FlashFire | Ab::Wellbakedbody => t == dex.ty.fire,
            Ab::Eartheater => t == dex.ty.ground,
            _ => false,
        };
        if absorbed || (t == dex.ty.ground && !self.is_grounded(dex, ts, ti)) {
            return None;
        }
        if ts == s && tab == Ab::Telepathy {
            return None;                            // Telepathy: the partner's attacks miss it
        }
        // priority moves into Psychic Terrain (grounded target) or a side with Armor Tail / Queenly Majesty / Dazzling fail
        if ts != s && self.move_priority(dex, s, i, mv) > 0
            && ((self.field.terrain == Terrain::Psychic && self.grounded_brk(dex, ts, ti, mold))
                || (!mold && self.side_has(ts, |a| matches!(a, Ab::Armortail | Ab::Queenlymajesty | Ab::Dazzling))))
        {
            return None;
        }
        let n_targets = match self.target_kind(dex, s, i, mv) {
            Target::AllAdjacentFoes => (0..2).filter(|&k| self.active(1 - s, k).is_some()).count(),
            Target::AllAdjacent => {
                let me = self.slot_of(s, i);
                (0..2).filter(|&k| self.active(1 - s, k).is_some()).count()
                    + me.map(|k| self.active(s, 1 - k).is_some() as usize).unwrap_or(0)
            }
            _ => 1,
        };
        let att = self.combatant(dex, s, i);
        let mut def = self.combatant(dex, ts, ti);
        if mold && !matches!(tab, Ab::ShadowShield | Ab::PrismArmor | Ab::Fullmetalbody | Ab::Multitype | Ab::Rkssystem | Ab::Comatose) {
            def.ability = Ab::None;
        }
        let mut cond = self.conditions(dex, s, i, ts, ti, spread.unwrap_or(n_targets > 1));
        let mut att = att;
        if state_bp {
            cond.bp_override = self.state_bp(dex, s, i, mv, ts, ti);
            // (2026-10-19) Fling's power comes from the held item (set only while the move runs): the bridge's Substitute
            // estimate gives the attacker the flung item first
            if mv.kind == Mv::Fling {
                let m = self.mon(s, i);
                let bp = if m.has_item() && !self.stone_locked(dex, s, i) { dex.items[m.item_id as usize].fling_bp } else { 0 };
                if bp == 0 {
                    return None;
                }
                cond.fling_bp = bp;
                att.item = m.item;
            }
        }
        crate::damage::damage_full(dex, mv, &att, &def, &cond, false, 1).map(|h| h.rolls)
    }

    /// (2026-11-05 speed pass) `preview_rolls_opt` for several move slots of ONE attacker / target pair (bit k of `mask` =
    /// slot k): the two Pokemon's combatants and the field conditions are built once for the pair instead of once per move.
    /// Same results as calling preview_rolls_spread(.., spread = None) per slot.
    #[allow(clippy::too_many_arguments)]
    pub fn preview_rolls_pair(&self, dex: &Dex, s: usize, i: usize, ts: usize, ti: usize, state_bp: bool, mask: u8) -> [Option<[u16; 16]>; 4] {
        let mut out = [None; 4];
        let att_m = self.mon(s, i);
        let att_ab = att_m.ability;
        let mold = matches!(att_ab, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze);
        let tab = self.mon(ts, ti).ability;
        if ts == s && tab == Ab::Telepathy {
            return out;                             // Telepathy: the partner's attacks miss it
        }
        let mut att: Option<Combatant> = None;
        let mut def: Option<Combatant> = None;
        let mut conds: [Option<Conditions>; 2] = [None, None];
        let mut grounded: Option<bool> = None;
        let mut nt: [Option<usize>; 2] = [None, None];   // targets of an all-adjacent-foes / all-adjacent move
        for slot in 0..4 {
            if mask & (1 << slot) == 0 {
                continue;
            }
            let id = att_m.moves[slot];
            if id == u16::MAX {
                continue;
            }
            let mv = &dex.moves[id as usize];
            if mv.category == Category::Status {
                continue;
            }
            let t = self.eff_type(dex, s, i, mv);
            let absorbed = !mold && match tab {
                Ab::Voltabsorb | Ab::Lightningrod | Ab::Motordrive => t == dex.ty.electric,
                Ab::Waterabsorb | Ab::Dryskin | Ab::Stormdrain => t == dex.ty.water,
                Ab::Sapsipper => t == dex.ty.grass,
                Ab::FlashFire | Ab::Wellbakedbody => t == dex.ty.fire,
                Ab::Eartheater => t == dex.ty.ground,
                _ => false,
            };
            if absorbed {
                continue;
            }
            if t == dex.ty.ground && !*grounded.get_or_insert_with(|| self.is_grounded(dex, ts, ti)) {
                continue;
            }
            if ts != s && self.move_priority(dex, s, i, mv) > 0
                && ((self.field.terrain == Terrain::Psychic && self.grounded_brk(dex, ts, ti, mold))
                    || (!mold && self.side_has(ts, |a| matches!(a, Ab::Armortail | Ab::Queenlymajesty | Ab::Dazzling))))
            {
                continue;
            }
            let n_targets = match self.target_kind(dex, s, i, mv) {
                Target::AllAdjacentFoes => *nt[0].get_or_insert_with(|| (0..2).filter(|&k| self.active(1 - s, k).is_some()).count()),
                Target::AllAdjacent => *nt[1].get_or_insert_with(|| {
                    let me = self.slot_of(s, i);
                    (0..2).filter(|&k| self.active(1 - s, k).is_some()).count()
                        + me.map(|k| self.active(s, 1 - k).is_some() as usize).unwrap_or(0)
                }),
                _ => 1,
            };
            let a_ = att.get_or_insert_with(|| self.combatant(dex, s, i));
            let d_ = def.get_or_insert_with(|| {
                let mut d = self.combatant(dex, ts, ti);
                if mold && !matches!(tab, Ab::ShadowShield | Ab::PrismArmor | Ab::Fullmetalbody | Ab::Multitype | Ab::Rkssystem | Ab::Comatose) {
                    d.ability = Ab::None;
                }
                d
            });
            let sp = n_targets > 1;
            let mut cond = *conds[sp as usize].get_or_insert_with(|| self.conditions(dex, s, i, ts, ti, sp));
            let mut fling_att: Option<Combatant> = None;
            if state_bp {
                cond.bp_override = self.state_bp(dex, s, i, mv, ts, ti);
                if mv.kind == Mv::Fling {
                    let bp = if att_m.has_item() && !self.stone_locked(dex, s, i) { dex.items[att_m.item_id as usize].fling_bp } else { 0 };
                    if bp == 0 {
                        continue;
                    }
                    cond.fling_bp = bp;
                    let mut a2 = a_.clone();
                    a2.item = att_m.item;
                    fling_att = Some(a2);
                }
            }
            let a_use = fling_att.as_ref().unwrap_or(a_);
            out[slot] = crate::damage::damage_full(dex, mv, a_use, d_, &cond, false, 1).map(|h| h.rolls);
        }
        out
    }

    fn move_priority(&self, dex: &Dex, s: usize, i: usize, mv: &MoveData) -> i8 {
        let m = self.mon(s, i);
        let mut p = mv.priority;
        if mv.grassy_glide && self.field.terrain == Terrain::Grassy && self.is_grounded(dex, s, i) {
            p += 1;
        }
        match m.ability {
            Ab::Prankster if mv.category == Category::Status => p += 1,
            Ab::GaleWings if m.hp == m.maxhp && mv.type_ == dex.ty.flying => p += 1,
            Ab::Triage if mv.flags & flag::HEAL != 0 => p += 3,
            _ => {}
        }
        p
    }

    /// The move a queued action will run, for its priority: a move Copycat called that continues (charge turn done, Outrage /
    /// Uproar lock) is offered as slot 0 but runs the called move (2026-10-17 targeted fuzz: Copycat -> Dig had slot 0's priority)
    /// Showdown queue.willAct(): any move or switch action still in the queue (a fainted Pokemon's move included)
    fn will_act(&self) -> bool {
        self.queue.iter().any(|q| matches!(q.kind, QKind::Move { .. } | QKind::Dead { .. } | QKind::Switch { .. }))
    }

    /// Champions Encore onStart: a target that still has to move this turn gets its queued move replaced by the encored
    /// one (unless it holds Mental Herb), aimed at a random target NOW (queue.changeAction without a targetLoc ->
    /// resolveAction's getRandomTarget); that loc is what a two-turn move stores. The mod has no OverrideAction.
    /// (2026-10-17 targeted fuzz 3741134 / 3358108: re-aiming at use time picked from the Pokemon still standing then)
    fn encore_change_action(&mut self, dex: &Dex, s: usize, i: usize, eslot: u8) {
        let Some(qi) = self.queue.iter().position(|q| q.side as usize == s && q.mon as usize == i && q.order == ORDER_MOVE
            && matches!(q.kind, QKind::Move { .. })) else { return };
        let QKind::Move { mslot, .. } = self.queue[qi].kind else { return };
        // (2026-10-18 targeted fuzz 4201015) a queued Struggle is changed too (Showdown: action.moveid 'struggle' != the encored
        // move): a Taunted Tinkaton that had to Struggle after Gigaton Hammer uses Gigaton Hammer again when Encored first
        if mslot == eslot || mslot == CALL_SLOT {
            return;
        }
        let m = self.mon(s, i);
        if m.has_item() && m.item == It::MentalHerb {
            return;
        }
        if m.charge_slot != NONE || m.lock_slot != NONE {
            return;
        }
        let em = &dex.moves[m.moves[eslot as usize] as usize];
        let target: i8 = match em.target {
            Target::Normal | Target::Other | Target::RandomNormal => {
                let foes: ArrayVec<i8, 2> = (0..2)
                    .filter(|&k| self.active(1 - s, k).is_some_and(|x| self.mon(1 - s, x).alive()))
                    .map(|k| k as i8 + 1)
                    .collect();
                match foes.len() {
                    0 => 0,
                    1 => foes[0],
                    _ => foes[self.rng.below(2) as usize],
                }
            }
            Target::AdjacentAlly => self.slot_of(s, i).map(|k| -((1 - k) as i8) - 1).unwrap_or(0),
            _ => 0,
        };
        let orig = if target > 0 {
            self.active(1 - s, (target - 1) as usize).map_or(NONE, |x| x as u8) | 0x80
        } else if target < 0 {
            self.active(s, (-target - 1) as usize).map_or(NONE, |x| x as u8)
        } else {
            NONE
        };
        self.queue[qi].kind = QKind::Move { mslot: eslot, target, orig };
        let mv = &dex.moves[self.queued_move(dex, s, i, eslot)];
        self.queue[qi].priority = self.move_priority(dex, s, i, mv);
    }

    fn queued_move(&self, dex: &Dex, s: usize, i: usize, mslot: u8) -> usize {
        let m = self.mon(s, i);
        if (m.charge_slot == CALL_SLOT || m.lock_slot == CALL_SLOT || (m.uproar > 0 && m.uproar_slot == CALL_SLOT))
            && m.call_move != u16::MAX
        {
            return m.call_move as usize;
        }
        self.move_of(dex, s, i, mslot)
    }

    fn move_of(&self, dex: &Dex, s: usize, i: usize, mslot: u8) -> usize {
        if mslot == CALL_SLOT && self.called_move != u16::MAX {
            return self.called_move as usize;
        }
        if mslot >= 4 {
            return dex.move_index("struggle").unwrap();
        }
        let m = self.mon(s, i);
        let slot = if m.encore > 0 && m.encore_slot != NONE { m.encore_slot } else { mslot };
        let id = m.moves[slot as usize];
        if slot >= m.n_moves || id == u16::MAX {
            // an empty move slot (a short guessed set, or an Encore on a move it no longer has)
            return dex.move_index("struggle").unwrap();
        }
        id as usize
    }

    // ------------------------------------------------------------------
    // Turn flow
    // ------------------------------------------------------------------
    fn start_turn(&mut self, dex: &Dex) {
        self.in_action = true;
        self.queue.clear();
        self.round_next = None;
        self.chosen = [[None; 2]; 2];
        for s in 0..2 {
            let acts = self.pending[s].take().unwrap_or([Action::Pass; 2]);
            let mut mega_taken = false;
            for k in 0..2 {
                let Some(i) = self.active(s, k) else { continue };
                let act = acts[k];
                self.chosen[s][k] = Some(act);
                match act {
                    Action::Switch { to } => {
                        let sp = self.action_speed(dex, s, i);
                        let tie = self.rng.next_u32();
                        self.queue.push(QAction { order: ORDER_SWITCH, priority: 0, speed: sp, tie, side: s as u8, slot: k as u8, mon: i as u8, quick: false, stall: false, kind: QKind::Switch { to } });
                    }
                    Action::Move { slot, target, mega } => {
                        if slot < 4 && slot >= self.mon(s, i).n_moves {
                            continue;
                        }
                        if mega && !mega_taken && self.can_mega(s, i) {
                            mega_taken = true;
                            let sp = self.action_speed(dex, s, i);
                            let tie = self.rng.next_u32();
                            self.queue.push(QAction { order: ORDER_MEGA, priority: 0, speed: sp, tie, side: s as u8, slot: k as u8, mon: i as u8, quick: false, stall: false, kind: QKind::Mega });
                        }
                        // a move Copycat called and that continues (charge turn done, Outrage / Uproar lock): offered as slot 0,
                        // but its own priority counts (2026-10-17 targeted fuzz: Copycat -> Dig queued with slot 0's priority)
                        let mv = &dex.moves[self.queued_move(dex, s, i, slot)];
                        let pr = self.move_priority(dex, s, i, mv);
                        let sp = self.action_speed(dex, s, i);
                        // FractionalPriority (Showdown): Quick Draw (30%, attacking moves) runs first; Quick Claw (20%) only if
                        // nothing fired yet - its 'priority <= 0' test reads the event's running value (0), not the move's priority
                        let (ab, status) = (self.mon(s, i).ability, mv.category == Category::Status);
                        let draw = ab == Ab::Quickdraw && !status && self.chance_of(3, 10);
                        let quick = draw || (self.mon(s, i).has_item() && self.mon(s, i).item == It::QuickClaw
                            && !(status && ab == Ab::Myceliummight) && self.chance_of(1, 5));
                        let tie = self.rng.next_u32();
                        // a charging two-turn move aims at its stored target: tracksTarget follows the Pokemon there now
                        let target = if self.mon(s, i).charge_slot != NONE { self.mon(s, i).charge_target } else { target };
                        let orig = if target > 0 {
                            self.active(1 - s, (target - 1) as usize).map_or(NONE, |x| x as u8) | 0x80
                        } else if target < 0 {
                            self.active(s, (-target - 1) as usize).map_or(NONE, |x| x as u8)
                        } else {
                            NONE
                        };
                        self.queue.push(QAction { order: ORDER_MOVE, priority: pr, speed: sp, tie, side: s as u8, slot: k as u8, mon: i as u8, quick, stall: ab == Ab::Stall && !quick, kind: QKind::Move { mslot: slot, target, orig } });
                    }
                    Action::Pass => {}
                }
            }
        }
        // Beak Blast heats up before anything moves (priorityChargeCallback)
        for k in 0..self.queue.len() {
            let q = &self.queue[k];
            if let QKind::Move { mslot, .. } = q.kind {
                let (s, i) = (q.side as usize, q.mon as usize);
                if mslot < 4 && dex.moves[self.move_of(dex, s, i, mslot)].kind == Mv::Beakblast {
                    self.sides[s].mons[i].vol2 |= vol2::BEAK_BLAST;
                }
            }
        }
        if self.chance.is_some() {
            self.enumerate_ties();
        }
        self.run_queue(dex);
    }

    /// chance enumeration: two moves tied on priority, Quick Claw and speed -> one 50/50 branch decides their order
    /// (larger tie groups keep their random draw)
    fn enumerate_ties(&mut self) {
        let n = self.queue.len();
        for a in 0..n {
            for b in a + 1..n {
                let (x, y) = (&self.queue[a], &self.queue[b]);
                if x.order != ORDER_MOVE || y.order != ORDER_MOVE || x.priority != y.priority || x.quick != y.quick || x.stall != y.stall || x.speed != y.speed {
                    continue;
                }
                let others = (0..n).filter(|&c| c != a && c != b).any(|c| {
                    let z = &self.queue[c];
                    z.order == ORDER_MOVE && z.priority == x.priority && z.quick == x.quick && z.stall == x.stall && z.speed == x.speed
                });
                if others {
                    continue;
                }
                if let Some(first) = self.branch(0.5) {
                    let (lo, hi) = (self.queue[a].tie.min(self.queue[b].tie), self.queue[a].tie.max(self.queue[b].tie));
                    self.queue[a].tie = if first { lo } else { hi };
                    self.queue[b].tie = if first { hi } else { lo };
                }
            }
        }
    }

    fn sort_queue(&mut self) {
        let before = |a: &QAction, b: &QAction| {
            a.order.cmp(&b.order).then(b.priority.cmp(&a.priority)).then(b.quick.cmp(&a.quick)).then(a.stall.cmp(&b.stall)).then(b.speed.cmp(&a.speed)).then(a.tie.cmp(&b.tie))
                == std::cmp::Ordering::Less
        };
        let q = &mut self.queue;
        for i in 1..q.len() {
            let mut j = i;
            while j > 0 && before(&q[j], &q[j - 1]) {
                q.swap(j - 1, j);
                j -= 1;
            }
        }
    }

    fn run_queue(&mut self, dex: &Dex) {
        self.sort_queue();
        while !self.queue.is_empty() {
            if self.winner.is_some() {
                return;
            }
            let a = self.queue.remove(0);
            let (s, k) = (a.side as usize, a.slot as usize);
            match a.kind {
                QKind::Switch { to } => {
                    if self.sides[s].active[k] != NONE && (to as usize) < self.sides[s].mons.len() && self.sides[s].mons[to as usize].alive() {
                        self.do_switch(dex, s, k, to as usize);
                    }
                }
                // the Pokemon that chose the action, wherever it is now (Ally Switch); none if it left
                QKind::Mega => {
                    let i = a.mon as usize;
                    if self.slot_of(s, i).is_some() && self.mon(s, i).alive() {
                        self.mega_evolve(dex, s, i);
                    }
                }
                // Showdown runAction returns at once for a fainted Pokemon's move: no Update (White Herb...), no faint
                // messages, no re-sort after it (2026-10-17: the placeholder ran update_items, coverage case 2028181)
                QKind::Dead { .. } => continue,
                QKind::Move { mslot, target, orig } => {
                    let i = a.mon as usize;
                    if self.slot_of(s, i).is_some() && self.mon(s, i).alive() {
                        self.orig_target = (orig != NONE && orig & 0x7f != NONE & 0x7f)
                            .then(|| (s as u8, i as u8, if orig & 0x80 != 0 { 1 - s as u8 } else { s as u8 }, orig & 0x7f));
                        self.run_move(dex, s, i, mslot, target);
                        self.orig_target = None;
                        // Showdown runEvent('AfterMove', mover, target): with the mover fainted (not active) the onAny
                        // handlers (White Herb) are only found when the move's target is still active
                        if !self.mon(s, i).alive() {
                            let tgt = if target > 0 {
                                self.active(1 - s, (target - 1) as usize).map(|x| (1 - s, x))
                            } else if target < 0 {
                                self.active(s, (-target - 1) as usize).map(|x| (s, x))
                            } else {
                                // a spread / random move: Showdown's runMove target = a random adjacent foe (getRandomTarget)
                                let mid = if (mslot as usize) < 4 { self.mon(s, i).moves[mslot as usize] } else { u16::MAX };
                                let foe_move = mid != u16::MAX && matches!(dex.moves[mid as usize].target,
                                    Target::AllAdjacentFoes | Target::AllAdjacent | Target::RandomNormal);
                                let foes: ArrayVec<usize, 2> = (0..2).filter_map(|k| {
                                    let x = self.sides[1 - s].active[k];
                                    (x != NONE).then_some(x as usize)
                                }).collect();
                                if foe_move && !foes.is_empty() {
                                    let f = foes[self.pick(foes.len())];
                                    Some((1 - s, f))
                                } else {
                                    None
                                }
                            };
                            self.herb_skip = !tgt.map(|(a, b)| self.mon(a, b).alive()).unwrap_or(false);
                        }
                    }
                }
            }
            self.opportunist_flush(dex);
            self.update_items(dex);
            self.herb_skip = false;
            self.check_win();
            if self.winner.is_some() {
                self.phase = Phase::Ended;
                return;
            }
            // a mid-turn switch-out whose Pokemon fainted since (Emergency Exit on the 1st hit of a 2-hit move, U-turn +
            // Rocky Helmet...) doesn't happen: it is replaced at the end of the turn (Showdown)
            if self.phase == Phase::Switch {
                for s2 in 0..2 {
                    for k in 0..2 {
                        let cur = self.sides[s2].active[k];
                        if self.switch_needed[s2][k] && cur != NONE && !self.sides[s2].mons[cur as usize].alive() {
                            self.switch_needed[s2][k] = false;
                            self.reviving[s2][k] = false;
                        }
                    }
                }
                if !self.switch_needed.iter().flatten().any(|&x| x) {
                    self.phase = Phase::Turn;
                }
            }
            if self.phase == Phase::Switch {
                // a mid-turn switch is needed; the rest of the queue waits
                self.midturn = true;
                return;
            }
            // Gen 8+: speeds are recalculated after every action
            // (and priorities: Showdown's getActionSpeed re-runs ModifyPriority, e.g. Grassy Glide
            // once Grassy Terrain is up)
            // (Showdown re-sorts only while the NEXT action is a move: a queue of switches / Mega Evolutions keeps the
            // speeds it was built with, e.g. a White Herb + Unburden Speed jump doesn't reorder a double switch)
            if matches!(self.queue.first().map(|q| &q.kind), Some(QKind::Move { .. } | QKind::Dead { .. })) {
                self.refresh_queue_speeds(dex);
                self.sort_queue();
            }
        }
        self.end_of_turn(dex);
    }

    /// Recompute every queued action's speed and move priority (Showdown does this after every action).
    fn refresh_queue_speeds(&mut self, dex: &Dex) {
        for qi in 0..self.queue.len() {
            let (s, k) = (self.queue[qi].side as usize, self.queue[qi].slot as usize);
            // the Pokemon that queued the action, wherever it is now (2026-10-15 fuzzing: after Ally Switch the slot
            // holds its partner, whose speed was used), else whoever holds the slot (as before)
            let mi = self.queue[qi].mon as usize;
            let who = if mi < self.sides[s].mons.len() && self.slot_of(s, mi).is_some() && self.mon(s, mi).alive() {
                Some(mi)
            } else {
                self.active(s, k)
            };
            if let Some(i) = who {
                let sp = self.action_speed(dex, s, i);
                self.queue[qi].speed = sp;
                if let QKind::Move { mslot, .. } = self.queue[qi].kind {
                    if self.queue[qi].order == ORDER_MOVE && i == self.queue[qi].mon as usize {
                        let mv = &dex.moves[self.queued_move(dex, s, i, mslot)];
                        let pr = self.move_priority(dex, s, i, mv);
                        self.queue[qi].priority = pr;
                    }
                }
            }
        }
    }

    /// (2026-11-05) a side has no Pokemon left: the battle is decided even before check_win runs
    fn side_out(&self) -> bool {
        self.sides[0].alive_count() == 0 || self.sides[1].alive_count() == 0
    }

    fn check_win(&mut self) {
        let alive = [self.sides[0].alive_count(), self.sides[1].alive_count()];
        if alive[0] == 0 && alive[1] == 0 {
            self.winner = Some(self.last_faint_side);
        } else if alive[0] == 0 {
            self.winner = Some(1);
        } else if alive[1] == 0 {
            self.winner = Some(0);
        }
        if self.winner.is_some() {
            self.phase = Phase::Ended;
        }
    }

    /// Revival Blessing brings fainted Pokemon `f` back at half HP; one still in an active slot (it fainted this turn) comes
    /// back in there
    fn revive(&mut self, dex: &Dex, s: usize, f: usize) {
        let half = (self.sides[s].mons[f].maxhp / 2).max(1);
        let m = &mut self.sides[s].mons[f];
        m.fainted = false;
        m.hp = half;
        m.status = Status::None;
        self.add(|| format!("|-heal|{}|{}|{}|[from] revivalblessing", s, f, half));
        if self.sides[s].active.contains(&(f as u8)) {
            self.switch_in_effects(dex, s, f);
        }
    }

    /// (2026-10-19, bridge) put a rebuilt position at Revival Blessing's request: `slot`'s user picks a fainted Pokemon
    pub fn set_reviving(&mut self, s: usize, slot: usize) -> bool {
        if self.sides[s].active[slot] == NONE || !self.sides[s].mons.iter().any(|m| m.fainted) {
            return false;
        }
        self.reviving[s][slot] = true;
        self.switch_needed[s][slot] = true;
        self.phase = Phase::Switch;
        true
    }

    /// (2026-10-19, bridge: forced behaviour check) put a rebuilt position at a MID-TURN switch-out request: `slot`'s living
    /// Pokemon leaves now (U-turn, Eject Button, Baton Pass, Shed Tail...). Its switch-out effects run when the choice is made
    /// (Regenerator, Natural Cure...), `baton` passes its stat stages / volatiles, `shed_tail` leaves a Substitute of a quarter of
    /// its max HP; afterwards the rest of the turn runs (end of turn: residuals, then replacements of fainted Pokemon). Every
    /// other pending request is cleared (fainted slots are replaced at the end of the turn). Call after refresh().
    pub fn set_switching(&mut self, s: usize, slot: usize, baton: bool, shed_tail: bool) -> bool {
        let i = self.sides[s].active[slot];
        if i == NONE || !self.sides[s].mons[i as usize].alive() || self.sides[s].bench().next().is_none() {
            return false;
        }
        if !self.midturn {
            // the first call clears the other pending requests; a second call adds another slot (Flip Turn into a partner
            // holding an Eject Button: both slots switch out at once)
            self.switch_needed = [[false; 2]; 2];
            self.reviving = [[false; 2]; 2];
            self.pending = [None, None];
        }
        if baton {
            self.set_baton(s, i as usize);
        }
        if shed_tail {
            self.sides[s].shed_sub[slot] = self.sides[s].mons[i as usize].maxhp / 4;
        }
        self.switch_needed[s][slot] = true;
        self.phase = Phase::Switch;
        self.midturn = true;
        true
    }

    /// (2026-10-22, arena game 5199) a rebuilt MID-TURN position (set_switching) has an empty action queue, so the Pokemon that
    /// had not acted yet never act and the switch-in looks safe (Gholdengo came in under Mimikyu's queued Shadow Claw). Queues
    /// `slot`'s move for the Pokemon in (s, k) as if it had been chosen this turn (no Mega, no Quick Claw / Quick Draw roll);
    /// the queue runs after the switch, like a real mid-turn switch. False if nobody / no such move is there.
    pub fn queue_move(&mut self, dex: &Dex, s: usize, k: usize, slot: u8, target: i8) -> bool {
        let Some(i) = self.active(s, k) else { return false };
        if !self.mon(s, i).alive() || (slot < 4 && slot >= self.mon(s, i).n_moves) {
            return false;
        }
        if self.queue.iter().any(|q| q.side as usize == s && q.mon as usize == i) {
            return false;
        }
        let mv = &dex.moves[self.queued_move(dex, s, i, slot)];
        let pr = self.move_priority(dex, s, i, mv);
        let sp = self.action_speed(dex, s, i);
        let tie = self.rng.next_u32();
        let target = if self.mon(s, i).charge_slot != NONE { self.mon(s, i).charge_target } else { target };
        let orig = if target > 0 {
            self.active(1 - s, (target - 1) as usize).map_or(NONE, |x| x as u8) | 0x80
        } else if target < 0 {
            self.active(s, (-target - 1) as usize).map_or(NONE, |x| x as u8)
        } else {
            NONE
        };
        let ab = self.mon(s, i).ability;
        self.chosen[s][k] = Some(Action::Move { slot, target, mega: false });
        self.queue.push(QAction { order: ORDER_MOVE, priority: pr, speed: sp, tie, side: s as u8, slot: k as u8, mon: i as u8,
                                  quick: false, stall: ab == Ab::Stall, kind: QKind::Move { mslot: slot, target, orig } });
        true
    }

    fn request_switch(&mut self, s: usize, slot: usize) {
        if self.sides[s].bench().next().is_some() {
            self.switch_needed[s][slot] = true;
            self.phase = Phase::Switch;
        }
    }

    fn apply_switch_choices(&mut self, dex: &Dex) {
        let mut entered = MonList::new();
        // (side, slot, incoming, outgoing) of each switch, for Illusion below
        let mut plans: ArrayVec<(usize, usize, usize, u8), 4> = ArrayVec::new();
        for s in 0..2 {
            let acts = self.pending[s].take().unwrap_or([Action::Pass; 2]);
            for k in 0..2 {
                if !self.switch_needed[s][k] {
                    continue;
                }
                self.switch_needed[s][k] = false;
                if std::mem::take(&mut self.reviving[s][k]) {
                    let pick = match acts[k] {
                        Action::Switch { to } if (to as usize) < self.sides[s].mons.len() && self.sides[s].mons[to as usize].fainted => Some(to as usize),
                        // Showdown's default: the first fainted Pokemon in party order
                        _ => self.sides[s].party_order().iter().map(|&j| j as usize)
                            .find(|&j| j < self.sides[s].mons.len() && self.sides[s].mons[j].fainted),
                    };
                    if let Some(f) = pick {
                        self.revive(dex, s, f);
                    }
                    continue;
                }
                // a mid-turn switch-out (Emergency Exit on the 1st hit of Dual Wingbeat, U-turn + Rocky Helmet...) whose
                // Pokemon fainted since doesn't happen: it is replaced at the end of the turn (Showdown)
                let cur = self.sides[s].active[k];
                if self.midturn && cur != NONE && !self.sides[s].mons[cur as usize].alive() {
                    continue;
                }
                let to = match acts[k] {
                    Action::Switch { to } if (to as usize) < self.sides[s].mons.len()
                        && self.sides[s].mons[to as usize].alive()
                        && !self.sides[s].active.contains(&to) => to as usize,
                    _ => match self.sides[s].bench().next() {
                        Some(b) => b,
                        None => continue,
                    },
                };
                self.switch_out(dex, s, k);
                self.sides[s].active[k] = to as u8;
                self.sides[s].party_put(k, to);
                self.set_illusion(s, k, to);
                plans.push((s, k, to, cur));
                self.mon_mut(s, to).active_turns = 0;
                self.new_ab_epoch(s, to);
                self.mon_mut(s, to).vol |= vol::NEWLY_SWITCHED;
                self.mon_mut(s, to).hit_by = 0;
                self.mon_mut(s, to).retal_slot = [NONE; 3];
                if let Some(b) = self.sides[s].baton.take() {
                    let m = self.mon_mut(s, to);
                    m.boosts = b.boosts;
                    m.vol |= b.vol;
                    m.vol2 |= b.vol2;
                    m.sub_hp = b.sub_hp;
                    m.confusion = b.confusion;
                    m.bound = b.bound;
                    m.bound_div = b.bound_div;
                    m.bound_src = b.bound_src;
                    m.bound_side = b.bound_side;
                    m.heal_block = b.heal_block;
                    m.magnet_rise = b.magnet_rise;
                    m.perish = b.perish;
                    m.taunt = b.taunt;
                    m.throat_chop = b.throat_chop;
                    m.octolock_src = b.octolock_src;
                    m.octolock_side = b.octolock_side;
                    m.leech_slot = b.leech_slot;
                    m.leech_side = b.leech_side;
                }
                // Shed Tail's substitute goes to the Pokemon chosen to come in (2026-10-15 fuzzing: this path, the
                // usual one for a mid-turn self-switch, never handed it over; only do_switch did)
                let sub = std::mem::take(&mut self.sides[s].shed_sub[k]);
                if sub > 0 {
                    self.mon_mut(s, to).sub_hp = sub;
                }
                if self.sides[s].healing_wish[k] {
                    let m = self.mon(s, to);
                    if m.hp < m.maxhp || m.status != Status::None {
                        let m = self.mon_mut(s, to);
                        m.hp = m.maxhp;
                        m.status = Status::None;
                        self.sides[s].healing_wish[k] = false;
                    }
                }
                self.counters.switches += 1;
                self.add(|| format!("|switch|p{}{}|{}", s + 1, if k == 0 { 'a' } else { 'b' }, to));
                entered.push((s, to));
            }
        }
        // (2026-10-17 targeted fuzz) Illusion picks its disguise at BeforeSwitchIn, and Showdown runs simultaneous
        // replacements in the speed order of the Pokemon being REPLACED (the switch action belongs to the fainted one;
        // Trick Room reverses it). A Zoroark coming in before its partner has left the bench can still disguise as that
        // partner. Only redone when an Illusion user comes in, so other games' random numbers stay as they were.
        if plans.len() > 1 && plans.iter().any(|p| self.mon(p.0, p.2).ability == Ab::Illusion) {
            let mut order: ArrayVec<(i32, u32, usize), 4> = ArrayVec::new();
            for (n, p) in plans.iter().enumerate() {
                let sp = if p.3 == NONE { 0 } else { self.action_speed(dex, p.0, p.3 as usize) };
                order.push((sp, self.rng.next_u32(), n));
            }
            order.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
            for (rank, &(_, _, n)) in order.iter().enumerate() {
                let (s, k, to, _) = plans[n];
                if self.mon(s, to).ability != Ab::Illusion {
                    continue;
                }
                // the side's active slots at that moment: switches later in the order haven't happened yet
                let mut act = self.sides[s].active;
                for &(_, _, m) in order.iter().skip(rank + 1) {
                    let q = plans[m];
                    if q.0 == s {
                        act[q.1] = q.3;
                    }
                }
                let pick = self.illusion_pick(s, to, k, &act);
                self.mon_mut(s, to).illusion = pick.is_some();
                self.mon_mut(s, to).illusion_as = pick.map(|j| j as u8).unwrap_or(NONE);
            }
        }
        // (2026-10-18 targeted fuzz, 4660271 / 4984422) a Trace still seeking (it came in with no traceable foe) resolves at
        // the Update after the whole batch of replacement switches (Showdown: no Update between consecutive instaswitches),
        // before the switch-in abilities: it samples among ALL traceable foes now on the field (Gardevoir traces Scrafty's
        // Intimidate or Tinkaton's Mold Breaker at random; Ditto (Imposter, untraceable) + Sneasler -> Unburden).
        // Only when such a seeker exists (other games' random numbers unchanged).
        let seekers: ArrayVec<(usize, usize), 4> = (0..2).flat_map(|s| (0..2).map(move |k| (s, k)))
            .filter_map(|(s, k)| self.active(s, k).map(|i| (s, i)))
            .filter(|&(s, i)| self.mon(s, i).trace_seek && self.mon(s, i).alive() && self.mon(s, i).ability == Ab::Trace
                && !entered.contains(&(s, i)))
            .collect();
        if !seekers.is_empty() && plans.iter().any(|p| seekers.iter().any(|q| q.0 != p.0)) {
            for &(s, i) in seekers.iter() {
                if self.mon(s, i).trace_seek && self.mon(s, i).ability == Ab::Trace {
                    self.ability_start(dex, s, i);
                }
            }
        }
        self.sort_by_speed(dex, &mut entered);
        self.run_switch_ins(dex, &entered);
        for &(s, i) in entered.iter() {
            self.seed_check(dex, s, i);
        }
        // Opportunist's onAnySwitchIn (priority -3: after the switch-in effects)
        self.opportunist_flush(dex);
        self.update_items(dex);
        self.check_win();
        if self.winner.is_some() {
            return;
        }
        self.phase = Phase::Turn;
        // a replacement that fainted on entry (Stealth Rock...) is replaced again right away (end-of-turn replacements;
        // mid-turn ones wait for the end of the turn)
        if !self.midturn {
            for s in 0..2 {
                for k in 0..2 {
                    let i = self.sides[s].active[k];
                    if i != NONE && !self.sides[s].mons[i as usize].alive() {
                        self.request_switch(s, k);
                    }
                }
            }
            if self.phase == Phase::Switch {
                return;
            }
        }
        if self.midturn {
            self.midturn = false;
            // (2026-10-15 fuzzing) the action that paused the turn (U-turn, Flip Turn, Eject Button...) returned before
            // the speed refresh; the switch-in's effects (Intimidate -> White Herb -> Unburden...) change speeds
            self.refresh_queue_speeds(dex);
            self.run_queue(dex);
        } else {
            self.next_turn();
        }
    }

    fn next_turn(&mut self) {
        for s in 0..2 {
            let act = self.sides[s].active;
            for (k, m) in self.sides[s].mons.iter_mut().enumerate() {
                m.last_move_result = m.move_result;
                m.move_result = -1;
                m.last_move_failed = m.move_failed;
                m.move_failed = false;
                m.vol &= !vol::NEWLY_SWITCHED;
                if act.contains(&(k as u8)) {
                    m.active_turns = m.active_turns.saturating_add(1);
                    // Showdown nextTurn: after the end-of-turn replacements (a replacement's Grassy Seed boost or hazard
                    // damage doesn't count as 'this turn' for Alluring Voice / Burning Jealousy / Assurance / Pickup)
                    m.vol &= !(vol::STATS_RAISED | vol::STATS_LOWERED | vol::DAMAGED_THIS_TURN);
                    m.used_item_turn = false;
                }
            }
        }
        self.turn += 1;
        self.phase = Phase::Turn;
        let t = self.turn;
        self.add(|| format!("|turn|{t}"));
    }

    // ------------------------------------------------------------------
    // Switching, Mega Evolution
    // ------------------------------------------------------------------
    fn switch_out(&mut self, dex: &Dex, s: usize, slot: usize) {
        let i = self.sides[s].active[slot];
        if i == NONE {
            return;
        }
        // its queued move / Mega Evolution is cancelled (even if it comes back this turn)
        self.queue.retain(|q| !(q.side as usize == s && q.mon == i && matches!(q.kind, QKind::Move { .. } | QKind::Mega)));
        // SwitchOut abilities (Regenerator, Natural Cure) run with the CURRENT ability, before it is restored (Worry Seed,
        // Gastro Acid: no heal) - and before Transform is undone (a Ditto copying Natural Cure is cured; 2026-10-17 targeted fuzz)
        let out_ab = self.mon(s, i as usize).ability;
        {
            let m = self.mon_mut(s, i as usize);
            m.bench_seen = true;
            m.bench_hp = m.hp;
            m.bench_status = m.status;
            m.bench_sturns = m.status_turns;
        }
        if self.sides[s].transformed.map(|sv| sv.mon == i).unwrap_or(false) {
            let sv = self.sides[s].transformed.take().unwrap();
            let sp = &dex.species[sv.species as usize];
            let m = self.mon_mut(s, i as usize);
            let stats = crate::stats::champions_stats(&sp.base, &m.points, m.nature);
            m.species = sv.species;
            m.types = sp.types;
            m.added_type = crate::dex::NO_TYPE;
            for k in 1..6 {
                m.stats[k] = stats[k];
            }
            m.weight_hg = sp.weight_hg;
            m.moves = sv.moves;
            m.pp = sv.pp;
            m.n_moves = sv.n_moves;
            m.ability_id = sv.ability_id;
            m.ability = dex.abilities[sv.ability_id as usize].kind;
            m.vol &= !vol::TRANSFORMED;
        }
        // Zero to Hero: Palafin switching out becomes Palafin-Hero for good (Showdown onSwitchOut, permanent forme)
        {
            let m = self.mon(s, i as usize);
            if m.alive() && m.ability == Ab::Zerotohero && dex.species[m.species as usize].id == "palafin" {
                self.forme_change(dex, s, i as usize, "palafinhero");
            }
        }
        // Stance Change's forme change isn't permanent: Showdown clearVolatile -> setSpecies(baseSpecies) on switch-out
        // (a dragged-out / switched Blade comes back as Shield; a Mega or Palafin-Hero stays: those are permanent)
        {
            let m = self.mon(s, i as usize);
            let sid = dex.species[m.species as usize].id.as_str();
            let revert = if m.ability == Ab::Stancechange && sid == "aegislashblade" {
                Some("aegislash")
            } else if m.ability == Ab::Forecast && !m.has_vol(vol::TRANSFORMED) && sid.starts_with("castform") && sid != "castform" {
                Some("castform")
            } else if sid == "morpekohangry" && !m.has_vol(vol::TRANSFORMED) {
                // Hangry Mode is a forme change too: back to Full Belly on switch-out (Red Card drag; 2026-10-17 Zoroark fuzz 5076190)
                Some("morpeko")
            } else {
                None
            };
            if let Some(base) = revert {
                self.forme_change(dex, s, i as usize, base);
            }
        }
        // Attract ends once its source has left the field (Showdown attract onUpdate: source not active), even if it comes back
        for side in 0..2 {
            for j in 0..self.sides[side].mons.len() {
                let m = &mut self.sides[side].mons[j];
                if m.vol2 & vol2::ATTRACT != 0 && m.attract_side as usize == s && m.attract_src as usize == i as usize {
                    m.vol2 &= !vol2::ATTRACT;
                }
            }
        }
        // an ability changed in battle (Entrainment, Skill Swap, Simple Beam, Worry Seed, Role Play...) is undone on switch-out
        {
            let m = self.mon_mut(s, i as usize);
            if m.ability_id != m.base_ability_id || m.vol2 & vol2::GASTRO != 0 {
                m.ability_id = m.base_ability_id;
                m.ability = dex.abilities[m.base_ability_id as usize].kind;
            }
        }
        if self.mon(s, i as usize).ability == Ab::Klutz && self.mon(s, i as usize).stash_item.0 != u16::MAX && self.field.magic_room == 0 {
            let m = self.mon_mut(s, i as usize);
            m.item_id = m.stash_item.0;
            m.item = m.stash_item.1;
            m.stash_item = (u16::MAX, It::None);
        }
        // Power Split / Guard Split changed the stats: they come back with the species (Showdown setSpecies -> baseStoredStats)
        if self.mon(s, i as usize).vol2 & vol2::SPLIT != 0 {
            let sid = dex.species[self.mon(s, i as usize).species as usize].id.clone();
            self.forme_change(dex, s, i as usize, &sid);
            // (2026-10-18) the species' stats are back: a Power Trick swap must not be undone on top of them
            self.mon_mut(s, i as usize).vol2 &= !vol2::POWER_TRICK;
        }
        let m = self.mon_mut(s, i as usize);
        m.syrup = 0;
        m.trace_seek = false;
        m.cud_chew = 0;
        m.lock_on = 0;
        m.ally_sw = 0;
        m.ally_sw_dur = 0;
        m.metro_on = false;
        m.berry_weaken = false;
        m.metro_move = u16::MAX;
        m.metro_n = 0;
        m.opp_pending = [0; 8];
        if m.alive() {
            let ab = out_ab;
            if ab == Ab::Regenerator {
                m.hp = (m.hp + m.maxhp / 3).min(m.maxhp); // regenerator
            }
            if ab == Ab::Naturalcure {
                m.status = Status::None;
            }
        }
        m.boosts = [0; 8];
        m.vol = 0;
        if m.vol2 & vol2::POWER_TRICK != 0 {
            m.stats.swap(1, 2);
        }
        m.vol2 = 0;
        m.yawn = 0;
        m.sub_hp = 0;
        m.bound = 0;
        m.octolock_src = NONE;
        m.trap_src = NONE;
        m.times_attacked = 0;
        m.heal_block = 0;
        m.magnet_rise = 0;
        m.stockpile = 0;
        m.types = dex.species[m.species as usize].types;
        m.added_type = crate::dex::NO_TYPE;
        m.confusion = 0;
        m.taunt = 0;
        m.encore = 0;
        m.throat_chop = 0;
        m.disable = 0;
        m.perish = 0;
        m.choice_lock = NONE;
        m.lock_slot = NONE;
        m.lock_dur = 0;
        m.lock_true = 0;
        m.uproar = 0;
        m.uproar_slot = NONE;
        m.last_move = u16::MAX; // Showdown clearVolatile: lastMove = null (Encore, Instruct, Gigaton Hammer)
        m.protect_streak = 0;
        m.active_move_actions = 0;
        m.charge_slot = NONE;
        m.leech_slot = NONE;
        m.used_mask = 0;
        m.fallen = 0;
        m.h_slot = NONE;
        m.h_streak = 0;
        m.h_protect = false;
        if m.status == Status::Toxic {
            m.status_turns = 0;
        }
    }

    /// Illusion at switch-in (Showdown onBeforeSwitchIn): disguised when a living party member stands after it in the
    /// side's order; the actives always hold positions 0 and 1, so slot a counts every other living Pokemon (the
    /// partner too) and slot b only the ones not on the field
    fn set_illusion(&mut self, s: usize, slot: usize, to: usize) {
        let act = self.sides[s].active;
        let pick = if self.mon(s, to).ability == Ab::Illusion { self.illusion_pick(s, to, slot, &act) } else { None };
        self.mon_mut(s, to).illusion = pick.is_some();
        self.mon_mut(s, to).illusion_as = pick.map(|j| j as u8).unwrap_or(NONE);
    }

    /// the Pokemon a Zoroark entering `slot` imitates: Showdown takes the last living party member after its position
    /// (slot a: any other living Pokemon, the partner too; slot b: benched ones only), in side.pokemon order
    fn illusion_pick(&self, s: usize, to: usize, slot: usize, act: &[u8; 2]) -> Option<usize> {
        let n = self.sides[s].mons.len();
        self.sides[s].party_order().iter().rev().map(|&j| j as usize).find(|&j| {
            j < n && j != to && self.sides[s].mons[j].alive() && (slot == 0 || !act.contains(&(j as u8)))
        })
    }

    fn do_switch(&mut self, dex: &Dex, s: usize, slot: usize, to: usize) {
        if self.sides[s].active.contains(&(to as u8)) {
            return;
        }
        self.switch_out(dex, s, slot);
        self.sides[s].active[slot] = to as u8;
        self.sides[s].party_put(slot, to);
        self.counters.switches += 1;
        self.add(|| format!("|switch|p{}{}|{}", s + 1, if slot == 0 { 'a' } else { 'b' }, to));
        self.mon_mut(s, to).active_turns = 0;
        self.new_ab_epoch(s, to);
        self.mon_mut(s, to).vol |= vol::NEWLY_SWITCHED;
        self.mon_mut(s, to).hit_by = 0;
        self.mon_mut(s, to).retal_slot = [NONE; 3];
        let sub = std::mem::take(&mut self.sides[s].shed_sub[slot]);
        if sub > 0 {
            self.mon_mut(s, to).sub_hp = sub;
        }
        // A waiting Healing Wish (Gen 9: it stays until a hurt / statused Pokemon enters the slot) also heals a
        // Pokemon brought in by a normal switch (2026-10-18 bridge fuzzing: only replacements were healed)
        if self.sides[s].healing_wish[slot] {
            let m = self.mon(s, to);
            if m.hp < m.maxhp || m.status != Status::None {
                let m = self.mon_mut(s, to);
                m.hp = m.maxhp;
                m.status = Status::None;
                self.sides[s].healing_wish[slot] = false;
            }
        }
        self.set_illusion(s, slot, to);
        self.switch_in_effects(dex, s, to);
    }

    /// Change species (battle forme), recomputing stats from the same points / nature; HP stays.
    fn forme_change(&mut self, dex: &Dex, s: usize, i: usize, species_id: &str) {
        let Some(sp_idx) = dex.species_index(species_id) else { return };
        let sp = &dex.species[sp_idx];
        let m = self.mon_mut(s, i);
        let stats = crate::stats::champions_stats(&sp.base, &m.points, m.nature);
        m.species = sp_idx as u16;
        m.types = sp.types;
        m.added_type = crate::dex::NO_TYPE;
        for k in 1..6 {
            m.stats[k] = stats[k];
        }
        m.weight_hg = sp.weight_hg;
    }

    /// Turn a Pokemon into its Mega form without any effects (for setting up a real position).
    pub fn set_mega_form(&mut self, dex: &Dex, s: usize, i: usize) {
        let target = self.mon(s, i).mega_species;
        if target == u16::MAX || self.mon(s, i).is_mega {
            return;
        }
        let sp = &dex.species[target as usize];
        let ability_id = dex.ability_index(&sp.ability0).unwrap_or(self.mon(s, i).ability_id as usize);
        let m = self.mon_mut(s, i);
        let stats = crate::stats::champions_stats(&sp.base, &m.points, m.nature);
        m.species = target;
        m.types = sp.types;
        m.added_type = crate::dex::NO_TYPE;
        for k in 1..6 {
            m.stats[k] = stats[k];
        }
        m.ability_id = ability_id as u16;
        m.base_ability_id = ability_id as u16;
        m.ability = dex.abilities[ability_id].kind;
        m.weight_hg = sp.weight_hg;
        m.is_mega = true;
        self.sides[s].mega_used = true;
    }

    fn mega_evolve(&mut self, dex: &Dex, s: usize, i: usize) {
        if self.sides[s].mega_used {
            return;
        }
        let target = self.mon(s, i).mega_species;
        if target == u16::MAX {
            return;
        }
        let sp = &dex.species[target as usize];
        let ability_id = dex.ability_index(&sp.ability0).unwrap_or(self.mon(s, i).ability_id as usize);
        let m = self.mon_mut(s, i);
        let stats = crate::stats::champions_stats(&sp.base, &m.points, m.nature);
        m.species = target;
        m.types = sp.types;
        m.added_type = crate::dex::NO_TYPE;
        for k in 1..6 {
            m.stats[k] = stats[k];
        }
        m.ability_id = ability_id as u16;
        m.base_ability_id = ability_id as u16;
        m.ability = dex.abilities[ability_id].kind;
        m.weight_hg = sp.weight_hg;
        m.is_mega = true;
        m.vol &= !vol::LIBERO_USED; // (2026-10-19) new abilityState (Showdown setAbility)
        self.sides[s].mega_used = true;
        self.new_ab_epoch(s, i);
        self.add(|| format!("|detailschange|p{}|{}", s + 1, sp.name));
        self.ability_start(dex, s, i);
    }

    /// (2026-10-18 targeted fuzz 4034046) Pokemon entering together: Showdown's runSwitch is ONE fieldEvent('SwitchIn')
    /// ordered by handler priority, then speed: Mimicry (onSwitchInPriority -1) and Forecast (-2) run after every priority-0
    /// effect (Imposter...), so a faster Ditto copies plain Castform before it turns Sunny
    fn run_switch_ins(&mut self, dex: &Dex, list: &MonList) {
        if list.len() < 2 {
            for &(s, i) in list.iter() {
                self.switch_in_effects_opt(dex, s, i, false);
            }
            return;
        }
        self.defer_low_prio = true;
        for &(s, i) in list.iter() {
            self.switch_in_effects_opt(dex, s, i, false);
        }
        self.defer_low_prio = false;
        for &(s, i) in list.iter() {
            if self.mon(s, i).alive() && self.mon(s, i).ability == Ab::Mimicry {
                self.mimicry(dex, s, i);
            }
        }
        for &(s, i) in list.iter() {
            if self.mon(s, i).alive() && self.mon(s, i).ability == Ab::Forecast {
                self.forecast_one(dex, s, i);
            }
        }
    }

    fn switch_in_effects(&mut self, dex: &Dex, s: usize, i: usize) {
        self.switch_in_effects_opt(dex, s, i, true);
    }

    /// `seeds`: also check terrain seeds now (false when several Pokemon come in together: their
    /// items react after all of their abilities, like Showdown's SwitchIn priorities)
    fn switch_in_effects_opt(&mut self, dex: &Dex, s: usize, i: usize, seeds: bool) {
        if !self.mon(s, i).alive() {
            return;
        }
        // a disguised Zoroark is seen as the Pokemon it imitates: it counts as seen once the disguise breaks
        if !self.mon(s, i).illusion {
            self.mon_mut(s, i).rev |= rev::SEEN;
            self.mon_mut(s, i).ghost_faint = false;
        }
        if self.mon(s, i).has_item() && self.mon(s, i).item == It::Metronome {
            self.metro_start(s, i);
        }
        let boots = self.mon(s, i).has_item() && self.mon(s, i).item == It::HeavyDutyBoots;
        if !boots && !self.magic_guard(dex, s, i) {
            if self.sides[s].stealth_rock {
                let rock = dex.ty.rock;
                let mut f = 1.0f64;
                for &t in self.mon(s, i).types.iter().chain(std::iter::once(&self.mon(s, i).added_type)) {
                    if t != crate::dex::NO_TYPE {
                        f *= match dex.type_code(rock, t) { 1 => 2.0, 2 => 0.5, 3 => 0.0, _ => 1.0 };
                    }
                }
                let amt = (self.mon(s, i).maxhp as f64 * f / 8.0).floor().max(1.0) as u16;
                if f > 0.0 {
                    self.hurt(s, i, amt, "stealthrock");
                }
            }
            let layers = self.sides[s].spikes;
            if layers > 0 && self.mon(s, i).alive() && self.is_grounded(dex, s, i) {
                let num = [0u32, 3, 4, 6][layers.min(3) as usize];
                let amt = ((num * self.mon(s, i).maxhp as u32) / 24).max(1) as u16;
                self.hurt(s, i, amt, "spikes");
            }
            if !self.mon(s, i).alive() {
                return;
            }
        }
        let layers = self.sides[s].toxic_spikes;
        if layers > 0 && self.is_grounded(dex, s, i) {
            let boots = self.mon(s, i).has_item() && self.mon(s, i).item == It::HeavyDutyBoots;
            if self.mon(s, i).has_type(dex.ty.poison) {
                self.sides[s].toxic_spikes = 0;
            } else if !boots && !self.mon(s, i).has_type(dex.ty.steel) {
                let st = if layers >= 2 { Status::Toxic } else { Status::Poison };
                let src = self.active(1 - s, 0).or(self.active(1 - s, 1)).map(|f| (1 - s, f));
                self.status_from_tspikes = true;
                self.set_status(dex, s, i, st, src);
                self.status_from_tspikes = false;
            }
        }
        if self.sides[s].sticky_web && self.mon(s, i).alive() && self.is_grounded(dex, s, i)
            && !(self.mon(s, i).has_item() && self.mon(s, i).item == It::HeavyDutyBoots)
        {
            let src = self.active(1 - s, 0).map(|f| (1 - s, f));
            self.boost(dex, s, i, &[0, 0, 0, 0, 0, -1, 0, 0], src, true);
        }
        // Imposter: transforms into the foe across from it (and that ability starts instead)
        if self.mon(s, i).ability == Ab::Imposter {
            if let Some(k) = self.slot_of(s, i) {
                if let Some(f) = self.active(1 - s, 1 - k) {
                    if self.transform_into(dex, s, i, 1 - s, f) {
                        if seeds {
                            self.seed_check(dex, s, i);
                        }
                        return;
                    }
                }
            }
        }
        self.ability_start(dex, s, i);
        if seeds {
            self.seed_check(dex, s, i);
        }
    }

    /// An ability's "on start" effect (switch-in or after Mega Evolution).
    /// Klutz only makes the holder ignore its item (Showdown ignoringItem): once the ability is gone (Wandering Spirit, Skill
    /// Swap, Gastro Acid...), the put-aside item works again unless Magic Room holds it
    fn klutz_restore(&mut self, s: usize, i: usize) {
        let mr = self.field.magic_room;
        let m = self.mon_mut(s, i);
        if m.ability != Ab::Klutz && m.stash_item.0 != u16::MAX && mr == 0 {
            m.item_id = m.stash_item.0;
            m.item = m.stash_item.1;
            m.stash_item = (u16::MAX, It::None);
        }
    }

    fn ability_start(&mut self, dex: &Dex, s: usize, i: usize) {
        self.klutz_restore(s, i);
        // a new ability on the field can start or end weather suppression (Cloud Nine / Air Lock onStart / onEnd fire
        // WeatherChange): Castform follows (2026-10-17 targeted fuzz: Drampa Mega Evolving out of Cloud Nine turned Castform Sunny)
        self.weather_supp_check(dex);
        let id = self.mon(s, i).ability;
        let item = self.mon(s, i).item_id;
        let item_id = if item == u16::MAX { "" } else { dex.items[item as usize].id.as_str() };
        match id {
            Ab::Mimicry if !self.defer_low_prio => self.mimicry(dex, s, i),
            Ab::Frisk => {
                for k in 0..2 {
                    if let Some(f) = self.active(1 - s, k) {
                        if self.mon(1 - s, f).has_item() {
                            self.mon_mut(1 - s, f).rev |= rev::ITEM;
                        }
                    }
                }
            }
            Ab::Screencleaner => {
                for side in 0..2 {
                    let sd = &mut self.sides[side];
                    sd.reflect = 0;
                    sd.light_screen = 0;
                    sd.aurora_veil = 0;
                }
            }
            Ab::Curiousmedicine => {
                if let Some(k) = self.slot_of(s, i) {
                    if let Some(a) = self.active(s, 1 - k) {
                        self.mon_mut(s, a).boosts = [0; 8];
                    }
                }
            }
            Ab::Supersweetsyrup => {
                if !self.mon(s, i).syrup_done {
                    self.mon_mut(s, i).syrup_done = true;
                    for k in 0..2 {
                        if let Some(f) = self.active(1 - s, k) {
                            if self.mon(1 - s, f).sub_hp == 0 {
                                self.boost(dex, 1 - s, f, &[0, 0, 0, 0, 0, 0, 0, -1], Some((s, i)), false);
                            }
                        }
                    }
                }
            }
            Ab::Supremeoverlord => {
                let n = self.sides[s].fainted_total.min(5);
                self.mon_mut(s, i).fallen = n;
            }
            Ab::Intimidate => {
                let foe = 1 - s;
                for k in 0..2 {
                    if let Some(f) = self.active(foe, k) {
                        if self.mon(foe, f).sub_hp > 0 {
                            continue;
                        }
                        let fid = self.mon(foe, f).ability;
                        match fid {
                            Ab::Innerfocus | Ab::Oblivious | Ab::Owntempo | Ab::Scrappy => continue,
                            Ab::Guarddog => {
                                self.boost(dex, foe, f, &[0, 1, 0, 0, 0, 0, 0, 0], Some((s, i)), false);
                                continue;
                            }
                            _ => {}
                        }
                        self.boost(dex, foe, f, &[0, -1, 0, 0, 0, 0, 0, 0], Some((s, i)), true);
                        if fid == Ab::Rattled {
                            self.boost(dex, foe, f, &[0, 0, 0, 0, 0, 1, 0, 0], None, false);
                        }
                    }
                }
            }
            // onStart: only the holder itself (a Ditto transforming into Castform must not change the real one's forme)
            Ab::Forecast if !self.defer_low_prio => self.forecast_one(dex, s, i),
            Ab::Klutz => {
                let m = self.mon_mut(s, i);
                if m.item_id != u16::MAX {
                    m.stash_item = (m.item_id, m.item);
                    m.item_id = u16::MAX;
                    m.item = It::None;
                }
            }
            Ab::Drizzle => self.set_weather(dex, Weather::Rain, if item_id == "damprock" { 8 } else { 5 }, tsrc(s, i)),
            Ab::Drought => self.set_weather(dex, Weather::Sun, if item_id == "heatrock" { 8 } else { 5 }, tsrc(s, i)),
            Ab::Sandstream => self.set_weather(dex, Weather::Sand, if item_id == "smoothrock" { 8 } else { 5 }, tsrc(s, i)),
            Ab::Snowwarning => self.set_weather(dex, Weather::Snow, if item_id == "icyrock" { 8 } else { 5 }, tsrc(s, i)),
            Ab::Grassysurge => self.set_terrain(dex, Terrain::Grassy, item_id == "terrainextender", tsrc(s, i)),
            Ab::Psychicsurge => self.set_terrain(dex, Terrain::Psychic, item_id == "terrainextender", tsrc(s, i)),
            Ab::Electricsurge => self.set_terrain(dex, Terrain::Electric, item_id == "terrainextender", tsrc(s, i)),
            Ab::Mistysurge => self.set_terrain(dex, Terrain::Misty, item_id == "terrainextender", tsrc(s, i)),
            Ab::Trace => {
                // copy a random foe's ability (not the ones Trace can't take), then start it
                let only = self.trace_only;
                let foes: Vec<usize> = (0..2).filter_map(|k| self.active(1 - s, k)).filter(|&f| {
                    if only.map_or(false, |o| o != (1 - s, f)) {
                        return false;
                    }
                    let m = self.mon(1 - s, f);
                    m.alive() && !matches!(dex.abilities[m.ability_id as usize].id.as_str(),
                        "trace" | "receiver" | "powerofalchemy" | "illusion" | "imposter" | "zenmode" | "stancechange"
                        | "disguise" | "multitype" | "rkssystem" | "schooling" | "shieldsdown" | "comatose" | "battlebond"
                        | "powerconstruct" | "flowergift" | "forecast" | "iceface" | "hungerswitch" | "gulpmissile"
                        | "neutralizinggas" | "asoneglastrier" | "asonespectrier" | "commander" | "protosynthesis"
                        | "quarkdrive" | "zerotohero" | "terashift" | "teraformzero" | "poisonpuppeteer" | "tera shell" | "terashell")
                }).collect();
                self.mon_mut(s, i).trace_seek = foes.is_empty();
                if !foes.is_empty() {
                    let f = if foes.len() > 1 { foes[self.pick(foes.len())] } else { foes[0] };
                    // (2026-10-19 bridge flag 18716) the foe's ABILITY, also when Gastro Acid suppresses it (Showdown Trace reads
                    // target.getAbility(): a Gardevoir tracing a Gastro-Acided Arbok gets a working Intimidate)
                    let aid = self.mon(1 - s, f).ability_id;
                    let ak = self.ab_for(dex, s, i, aid);
                    let m = self.mon_mut(s, i);
                    m.ability_id = aid;
                    m.ability = ak;
                    m.vol &= !(vol::UNBURDEN | vol::LIBERO_USED); // the old ability ends (Showdown End: Unburden drops its volatile; a new abilityState: Libero / Protean can fire again)
                    self.ability_start(dex, s, i);
                }
            }
            Ab::Hospitality => {
                if let Some(k) = self.slot_of(s, i) {
                    if let Some(a) = self.active(s, 1 - k) {
                        let amt = self.mon(s, a).maxhp / 4;
                        self.heal(s, a, amt, "hospitality");
                    }
                }
            }
            _ => {}
        }
    }

    fn set_weather(&mut self, dex: &Dex, w: Weather, turns: u8, src: u8) {
        if self.field.weather == w {
            return;
        }
        self.field.weather = w;
        self.field.weather_turns = turns;
        self.field.weather_src = src;
        self.field.weather_ext = turns > 5;
        self.add(|| format!("|-weather|{w:?}"));
        self.forecast_update(dex);
    }

    /// Forecast (Castform): the forme follows the weather it sees (Showdown onWeatherChange); not while transformed
    /// Libero / Protean (once per switch-in): the user becomes the type of the move it is about to use (PrepareHit)
    fn libero(&mut self, s: usize, i: usize, mv: &MoveData, mt: crate::dex::TypeId) {
        let ab = self.mon(s, i).ability;
        let m = self.mon(s, i);
        if matches!(ab, Ab::Libero | Ab::Protean) && !m.has_vol(vol::LIBERO_USED) && mv.kind != Mv::Struggle
            && mt != crate::dex::NO_TYPE && !(m.types[0] == mt && m.types[1] == crate::dex::NO_TYPE)
        {
            let m = self.mon_mut(s, i);
            m.types = [mt, crate::dex::NO_TYPE];
            m.added_type = crate::dex::NO_TYPE;
            m.vol |= vol::LIBERO_USED;
        }
    }

    fn weather_supp_check(&mut self, dex: &Dex) {
        let now = (0..2).any(|s| (0..2).any(|k| self.active(s, k).is_some_and(|i| {
            let m = self.mon(s, i);
            matches!(m.ability, Ab::CloudNine | Ab::AirLock) && !m.fainted
        })));
        if now != self.field.wx_supp {
            self.field.wx_supp = now;
            self.forecast_update(dex);
        }
    }

    fn forecast_update(&mut self, dex: &Dex) {
        for s in 0..2 {
            for k in 0..2 {
                if let Some(i) = self.active(s, k) {
                    self.forecast_one(dex, s, i);
                }
            }
        }
    }

    fn forecast_one(&mut self, dex: &Dex, s: usize, i: usize) {
        let w = self.weather();
        let m = self.mon(s, i);
        if m.ability != Ab::Forecast || !m.alive() || m.has_vol(vol::TRANSFORMED) {
            return;
        }
        let cur = dex.species[m.species as usize].id.as_str();
        if !cur.starts_with("castform") {
            return;
        }
        let umbrella = m.has_item() && dex.items[m.item_id as usize].id == "utilityumbrella";
        let want = match w {
            Weather::Sun if !umbrella => "castformsunny",
            Weather::Rain if !umbrella => "castformrainy",
            Weather::Snow => "castformsnowy",
            _ => "castform",
        };
        if cur != want {
            self.forme_change(dex, s, i, want);
        }
    }

    fn set_terrain(&mut self, dex: &Dex, t: Terrain, extended: bool, src: u8) {
        if self.field.terrain == t {
            return;
        }
        self.field.terrain = t;
        self.field.terrain_turns = if extended { 8 } else { 5 };
        self.field.terrain_src = src;
        self.field.terrain_ext = extended;
        self.add(|| format!("|-fieldstart|{t:?}"));
        self.terrain_changed(dex);
        for s in 0..2 {
            for k in 0..2 {
                if let Some(i) = self.active(s, k) {
                    self.seed_check(dex, s, i);
                }
            }
        }
    }

    /// (2026-10-15) Mimicry: every active holder takes the terrain's type (Electric / Grass / Fairy / Psychic), or its
    /// species' types without terrain (Showdown onTerrainChange; also on switch-in via ability_start)
    fn terrain_changed(&mut self, dex: &Dex) {
        for s in 0..2 {
            for k in 0..2 {
                if let Some(i) = self.active(s, k) {
                    self.mimicry(dex, s, i);
                }
            }
        }
    }

    fn mimicry(&mut self, dex: &Dex, s: usize, i: usize) {
        let m = self.mon(s, i);
        if m.ability != Ab::Mimicry || !m.alive() {
            return;
        }
        let none = crate::dex::NO_TYPE;
        let types = match self.field.terrain {
            Terrain::Electric => [dex.ty.electric, none],
            Terrain::Grassy => [dex.ty.grass, none],
            Terrain::Misty => [dex.ty.fairy, none],
            Terrain::Psychic => [dex.ty.psychic, none],
            // (2026-10-17 targeted fuzz) Showdown: pokemon.baseSpecies.types, so a Transformed Pokemon (Ditto that traced
            // Mimicry) goes back to its OWN types
            Terrain::None => {
                let sp = if m.has_vol(vol::TRANSFORMED) { m.base_species } else { m.species };
                dex.species[sp as usize].types
            }
        };
        if m.types != types {
            self.mon_mut(s, i).types = types;
            self.mon_mut(s, i).added_type = crate::dex::NO_TYPE;
            self.add(|| format!("|-start|{s}|{i}|typechange|mimicry"));
        }
    }

    /// Terrain seeds: +1 Def (Grassy/Electric) or +1 SpD (Psychic/Misty), used up.
    fn seed_check(&mut self, dex: &Dex, s: usize, i: usize) {
        let m = self.mon(s, i);
        if !m.has_item() {
            return;
        }
        let id = dex.items[m.item_id as usize].id.as_str();
        let stat = match (id, self.field.terrain) {
            ("grassyseed", Terrain::Grassy) | ("electricseed", Terrain::Electric) => DEF,
            ("psychicseed", Terrain::Psychic) | ("mistyseed", Terrain::Misty) => SPD,
            _ => return,
        };
        let mut b = [0i8; 8];
        b[stat] = 1;
        self.consume_item(s, i);
        self.boost(dex, s, i, &b, None, false);
    }

    fn consume_item(&mut self, s: usize, i: usize) {
        self.consume_item_opt(s, i, true);
    }

    /// (2026-10-18 bridge fuzz) Showdown pops an Air Balloon with `item = ''` (no useItem): lastItem / usedItemThisTurn stay
    /// as they were (Recycle can't bring it back, Pickup doesn't see it); AfterUseItem still runs (Symbiosis, Unburden)
    fn pop_balloon(&mut self, s: usize, i: usize) {
        let (li, ut) = (self.mon(s, i).last_item, self.mon(s, i).used_item_turn);
        self.consume_item(s, i);
        let m = self.mon_mut(s, i);
        m.last_item = li;
        m.used_item_turn = ut;
    }

    /// (2026-10-18 targeted fuzz 4094326) a resist berry is eaten in Showdown's ModifyDamage, BEFORE the hit lands: Symbiosis
    /// still passes the partner's item to a holder this hit KOs (the fast engine eats it after the damage)
    fn consume_berry_in_hit(&mut self, s: usize, i: usize) {
        self.symb_ko_ok = true;
        self.consume_item_opt(s, i, true);
        self.symb_ko_ok = false;
    }

    /// `symbiosis = false`: the holder is already switching out (Eject Button sets switchFlag before useItem; Showdown
    /// symbiosis onAllyAfterUseItem returns on pokemon.switchFlag; 2026-10-18 targeted fuzz 4056093)
    fn consume_item_opt(&mut self, s: usize, i: usize, symbiosis: bool) {
        let used = self.mon(s, i).item_id;
        if used != u16::MAX {
            let m = self.mon_mut(s, i);
            m.last_item = used;                               // Recycle / Harvest
            m.used_item_turn = true;                          // Pickup
            if crate::dex::item_is_berry(used) {
                m.ate_berry = true;                           // Belch
            }
        }
        self.remove_item(s, i);
        // Symbiosis: the partner passes its item on when this one uses its own up
        if let Some(k) = self.slot_of(s, i) {
            if let Some(a) = self.active(s, 1 - k) {
                let am = self.mon(s, a);
                if symbiosis && am.alive() && am.ability == Ab::Symbiosis && am.has_item() && am.mega_species == u16::MAX && !am.is_mega && (self.mon(s, i).alive() || self.symb_ko_ok) {
                    let (id, kind) = (am.item_id, am.item);
                    self.remove_item(s, a);
                    let m = self.mon_mut(s, i);
                    m.item_id = id;
                    m.item = kind;
                    m.rev |= rev::ITEM;
                }
            }
        }
    }

    /// The item is gone (no Symbiosis: knocked off, stolen, eaten by a Bug Bite...).
    fn remove_item(&mut self, s: usize, i: usize) {
        let m = self.mon_mut(s, i);
        m.rev |= rev::ITEM;
        m.item = It::None;
        m.item_id = u16::MAX;
        // (2026-10-17 targeted fuzz) only a Pokemon that HAS Unburden at that moment gets the volatile (Showdown
        // onAfterUseItem / onTakeItem): Ditto eating its Sitrus and then copying Sneasler's Unburden is not doubled
        if m.ability == Ab::Unburden && m.vol2 & vol2::GASTRO == 0 {
            m.vol |= vol::UNBURDEN;
        }
        m.choice_lock = NONE;   // choicelock removes itself once the item isn't a Choice item
    }

    /// Can this Pokemon's item be taken (Thief, Pickpocket, Magician)?
    fn item_takeable(&self, dex: &Dex, s: usize, i: usize) -> bool {
        let m = self.mon(s, i);
        m.has_item() && !self.stone_locked(dex, s, i) && dex.abilities[m.ability_id as usize].id != "stickyhold"
    }

    /// A Mega Stone can't leave a holder whose base species it belongs to (Showdown onTakeItem:
    /// !item.megaStone[source.baseSpecies.baseSpecies]): also Slowbronite on Slowbro-Galar; any other holder loses it normally
    fn stone_locked(&self, dex: &Dex, s: usize, i: usize) -> bool {
        let m = self.mon(s, i);
        if !m.has_item() {
            return false;
        }
        let stones = &dex.items[m.item_id as usize].mega;
        if stones.is_empty() {
            return false;
        }
        let sp = &dex.species[m.species as usize];
        let base = if sp.base_species.is_empty() { &sp.id } else { &sp.base_species };
        stones.iter().any(|(from, _)| from == base)
    }

    /// Move the item of (fs, fi) to (ts, ti) (which has none).
    fn steal_item(&mut self, fs: usize, fi: usize, ts: usize, ti: usize) {
        let (id, kind) = (self.mon(fs, fi).item_id, self.mon(fs, fi).item);
        self.remove_item(fs, fi);
        let m = self.mon_mut(ts, ti);
        m.item_id = id;
        m.item = kind;
        m.rev |= rev::ITEM;
        m.choice_lock = NONE;
        if kind == It::Metronome {
            self.metro_start(ts, ti);
        }
    }

    /// The Metronome item's Start: adds its volatile (a fresh count) unless it is already there
    fn metro_start(&mut self, s: usize, i: usize) {
        let m = self.mon_mut(s, i);
        if !m.metro_on {
            m.metro_on = true;
            m.metro_move = u16::MAX;
            m.metro_n = 0;
        }
    }

    /// Safeguard on (ts, ti)'s side stops a status / confusion / Yawn from (s, i) (not its own, not through Infiltrator)
    /// The Outrage lock ends (Showdown lockedmove onEnd): confused from fatigue if it ran its full 2-3 turns.
    /// (2026-10-18) a rebuilt position's Outrage lock of unknown length (lock_true = 0x80 | end-of-turns elapsed): Showdown's
    /// trueDuration is 2 or 3 at the start, one less per end of turn; drawn the first time it matters
    fn lock_true_now(&mut self, s: usize, i: usize) -> u8 {
        let lt = self.mon(s, i).lock_true;
        if lt & 0x80 != 0 {
            let t: u8 = if self.chance_of(1, 2) { 2 } else { 3 };   // (2026-10-28) a chance event (enumerated in chance mode)
            let v = t.saturating_sub(lt & 0x7f).max(1);
            self.mon_mut(s, i).lock_true = v;
            return v;
        }
        lt
    }

    fn end_lock(&mut self, dex: &Dex, s: usize, i: usize) {
        let lt = self.lock_true_now(s, i);
        let m = self.mon_mut(s, i);
        let tired = lt <= 1;
        m.lock_slot = NONE;
        m.lock_dur = 0;
        m.lock_true = 0;
        if tired && self.mon(s, i).alive() && self.mon(s, i).confusion == 0 && self.mon(s, i).ability != Ab::Owntempo
            && !self.misty_no_confusion(dex, s, i, None)
        {
            let n = 0x80u8;   // (2026-10-28, hidden counters) unknown 2-5 length: decided per check (BeforeMove)
            self.mon_mut(s, i).confusion = n; // Showdown time 2-5, decremented before each move: 1-4 confused turns
        }
    }

    /// Is anyone on the field in an uproar? (nobody can fall asleep)
    fn uproar_active(&self) -> bool {
        (0..2).any(|s| (0..2).any(|k| self.active(s, k).is_some_and(|x| self.mon(s, x).alive() && self.mon(s, x).uproar > 0)))
    }

    fn misty_no_confusion(&self, dex: &Dex, ts: usize, ti: usize, src: Option<(usize, usize)>) -> bool {
        self.field.terrain == Terrain::Misty && self.grounded_brk(dex, ts, ti, self.breaks_vs(ts, ti, src)) && !self.semi_invulnerable(dex, ts, ti)
    }

    fn safeguarded(&self, ts: usize, ti: usize, s: usize, i: usize) -> bool {
        self.sides[ts].safeguard > 0 && (ts, ti) != (s, i) && !(ts != s && self.mon(s, i).ability == Ab::Infiltrator)
    }

    /// Transform (s, i) into (ts, ti): species, types, stats but HP, boosts, moves at 5 PP, ability. Then its ability starts.
    fn new_ab_epoch(&mut self, s: usize, i: usize) {
        self.ab_epoch_ctr += 1;
        let e = self.ab_epoch_ctr;
        self.mon_mut(s, i).ab_epoch = e;
    }

    /// (2026-10-18, bridge) A rebuilt position's Transform that already happened: copy (ts, ti) without starting the copied
    /// ability (it started back then) and keep (s, i)'s current boosts (they changed since). PP of the copied moves: 5 - used.
    pub fn bridge_transform(&mut self, dex: &Dex, s: usize, i: usize, ts: usize, ti: usize, pp_used: [u8; 4], pre_mega: bool) -> bool {
        let boosts = self.mon(s, i).boosts;
        // the target may have fainted since (Imposter copying a Pokemon KO'd later): copy it as if alive
        let (hp, fainted) = (self.mon(ts, ti).hp, self.mon(ts, ti).fainted);
        if !self.mon(ts, ti).alive() {
            let t = self.mon_mut(ts, ti);
            t.hp = t.maxhp.max(1);
            t.fainted = false;
        }
        // (2026-10-19 bridge) the Transform happened before the target Mega Evolved: copy its base form (stats, types)
        let saved = *self.mon(ts, ti);
        // (2026-10-19) a rebuilt position builds a Mega directly as its Mega species (base_species = itself): use the
        // dex's base forme then
        let pre_idx = if !pre_mega {
            None
        } else if saved.is_mega && saved.base_species != u16::MAX && saved.base_species != saved.species {
            Some(saved.base_species as usize)
        } else if dex.species[saved.species as usize].is_mega {
            dex.species_index(&dex.species[saved.species as usize].base_species)
        } else {
            None
        };
        if let Some(bi) = pre_idx {
            let sp = &dex.species[bi];
            let st = crate::stats::champions_stats(&sp.base, &saved.points, saved.nature);
            let t = self.mon_mut(ts, ti);
            t.species = bi as u16;
            t.types = sp.types;
            for k in 1..6 {
                t.stats[k] = st[k];
            }
            t.weight_hg = sp.weight_hg;
            t.is_mega = false;
        }
        let ok = self.transform_core(dex, s, i, ts, ti);
        *self.mon_mut(ts, ti) = saved;
        let t = self.mon_mut(ts, ti);
        t.hp = hp;
        t.fainted = fainted;
        if !ok {
            return false;
        }
        let m = self.mon_mut(s, i);
        m.boosts = boosts;
        for k in 0..4 {
            if m.pp[k] > 0 {
                m.pp[k] = 5u8.saturating_sub(pp_used[k]);
            }
        }
        self.new_ab_epoch(s, i);
        true
    }

    fn transform_into(&mut self, dex: &Dex, s: usize, i: usize, ts: usize, ti: usize) -> bool {
        if !self.transform_core(dex, s, i, ts, ti) {
            return false;
        }
        if self.mon(s, i).vol2 & vol2::GASTRO == 0 {
            self.ability_start(dex, s, i);
        }
        self.new_ab_epoch(s, i);
        true
    }

    fn transform_core(&mut self, dex: &Dex, s: usize, i: usize, ts: usize, ti: usize) -> bool {
        let t = self.mon(ts, ti).clone();
        let u = self.mon(s, i);
        if !t.alive() || t.has_vol(vol::TRANSFORMED) || u.has_vol(vol::TRANSFORMED) || t.sub_hp > 0 || (s, i) == (ts, ti)
            || self.sides[s].transformed.is_some() || t.illusion || u.illusion
        {
            return false;
        }
        let old_moves = u.moves;
        let saved = Saved { mon: i as u8, n_moves: u.n_moves, species: u.species, ability_id: u.ability_id, moves: u.moves, pp: u.pp,
            stats: u.stats, types: u.types, weight_hg: u.weight_hg, ability: u.ability };
        self.sides[s].transformed = Some(saved);
        let m = self.mon_mut(s, i);
        m.species = t.species;
        m.types = t.types;
        m.added_type = t.added_type;
        for k in 1..6 {
            m.stats[k] = t.stats[k];
        }
        m.boosts = t.boosts;
        m.moves = t.moves;
        m.n_moves = t.n_moves;
        for k in 0..4 {
            m.pp[k] = if (k as u8) < t.n_moves { 5 } else { 0 };
        }
        m.ability = t.ability;
        m.vol &= !(vol::UNBURDEN | vol::LIBERO_USED); // the old ability ends (Showdown End: Unburden drops its volatile; a new abilityState: Libero / Protean can fire again)
        m.ability_id = t.ability_id;
        // (2026-10-17 targeted fuzz) abilities flagged notransform do nothing on a Transformed Pokemon (Showdown
        // ignoringAbility): Ditto copying Mimikyu gets no Disguise; the copied id stays (Trace / Skill Swap see it)
        if matches!(dex.abilities[t.ability_id as usize].id.as_str(), "disguise" | "embodyaspectcornerstone" | "embodyaspecthearthflame"
            | "embodyaspectteal" | "embodyaspectwellspring" | "gulpmissile" | "hungerswitch" | "iceface" | "neutralizinggas"
            | "protosynthesis" | "quarkdrive" | "terashift" | "zerotohero")
        {
            m.ability = Ab::None;
        }
        // (2026-10-18 targeted fuzz 4301233) Gastro Acid stays through Transform: the copied ability is suppressed and doesn't
        // start (a Gastro-Acided Ditto copying Gardevoir doesn't Trace Intimidate)
        let gastro = m.vol2 & vol2::GASTRO != 0;
        if gastro {
            m.ability = Ab::None;
        }
        m.weight_hg = t.weight_hg;
        m.times_attacked = t.times_attacked;
        // (2026-10-17 targeted fuzz) the copied move slots start unused (Showdown transformInto: used: false), so Last
        // Resort fails until the others have been used
        m.used_mask = 0;
        m.vol |= vol::TRANSFORMED | (t.vol & (vol::FOCUS_ENERGY | vol::DRAGON_CHEER | vol::DRAGON_CHEER2));
        // Choice lock (Choice Scarf Ditto using Transform): Showdown's lock remembers the MOVE ('transform'); the copied
        // moveset usually lacks it, so the lock is dropped at the next request (onDisableMove) and the next move used
        // locks again. Keep the lock only on a slot that still holds the locked move.
        if m.choice_lock != NONE {
            let locked = old_moves[m.choice_lock as usize];
            m.choice_lock = (0..m.n_moves.min(4)).find(|&k| m.moves[k as usize] == locked).unwrap_or(NONE);
        }
        let _ = gastro;
        true
    }

    // ------------------------------------------------------------------
    // Stat changes, status, damage, healing
    // ------------------------------------------------------------------
    /// Apply stat changes. `source`: who caused it (a foe's drop can trigger Defiant etc.).
    /// Returns whether anything changed.
    pub fn boost(&mut self, dex: &Dex, s: usize, i: usize, b: &Boosts, source: Option<(usize, usize)>, from_foe_effect: bool) -> bool {
        self.boost_blocked = false;
        if !self.mon(s, i).alive() {
            return false;
        }
        let mut ab = self.mon(s, i).ability;
        // a Mold Breaker user's move ignores the target's breakable stat abilities (Full Metal Body isn't breakable)
        if let Some((ss, si)) = source {
            if (ss, si) != (s, i) && self.cur_user == (ss as u8, si as u8)
                && matches!(self.mon(ss, si).ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze)
                && matches!(ab, Ab::Clearbody | Ab::Whitesmoke | Ab::Hypercutter | Ab::Bigpecks | Ab::Keeneye | Ab::Illuminate
                    | Ab::MindsEye | Ab::Mirrorarmor | Ab::Contrary | Ab::Simple)
            {
                ab = Ab::None;
            }
        }
        let by_foe = source.map(|(ss, _)| ss != s).unwrap_or(false) || from_foe_effect;
        // Clear Body / Hyper Cutter / Mirror Armor... block any drop not made by the Pokemon itself (an ally's too);
        // Defiant / Competitive only answer an opponent's
        let by_other = source.map(|(ss, si)| (ss, si) != (s, i)).unwrap_or(false) || from_foe_effect;
        let mut b = *b;
        if ab == Ab::Contrary {
            for x in b.iter_mut() {
                *x = -*x;
            }
        } else if ab == Ab::Simple {
            for x in b.iter_mut() {
                *x *= 2;
            }
        }
        let mut reflected: Boosts = [0; 8];
        if by_other {
            let blocks_all = matches!(ab, Ab::Clearbody | Ab::Whitesmoke | Ab::Fullmetalbody);
            // Flower Veil is breakable: a Mold Breaker user's move ignores it (an ally's too; 2026-10-18 targeted fuzz, case 4509014)
            let veil_broken = source.map_or(false, |(ss, si)| self.cur_user == (ss as u8, si as u8)
                && matches!(self.mon(ss, si).ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze));
            let flower_veil = !veil_broken && self.side_has(s, |a| a == Ab::Flowerveil) && self.mon(s, i).has_type(dex.ty.grass);
            for k in 1..8 {
                if b[k] < 0 {
                    let blocked = blocks_all
                        || flower_veil
                        || (ab == Ab::Hypercutter && k == ATK)
                        || (ab == Ab::Bigpecks && k == DEF)
                        || (matches!(ab, Ab::Keeneye | Ab::Illuminate | Ab::MindsEye) && k == ACC)
                        || (ab == Ab::Mirrorarmor && !self.mirror_bounce);
                    if blocked {
                        if ab != Ab::Mirrorarmor {
                            self.boost_blocked = true;
                        }
                        if ab == Ab::Mirrorarmor && source.is_some() {
                            reflected[k] = b[k];
                        }
                        b[k] = 0;
                    }
                }
            }
        }
        let mut changed = false;
        let mut dropped = false;
        let mut n_dropped = 0;
        let before_all = self.mon(s, i).boosts;
        {
            let m = self.mon_mut(s, i);
            for k in 1..8 {
                if b[k] == 0 {
                    continue;
                }
                let before = m.boosts[k];
                m.boosts[k] = (m.boosts[k] + b[k]).clamp(-6, 6);
                if m.boosts[k] != before {
                    changed = true;
                    if m.boosts[k] < before {
                        dropped = true;
                        n_dropped += 1;
                        m.vol |= vol::STATS_LOWERED;
                    } else {
                        m.vol |= vol::STATS_RAISED;
                    }
                }
            }
        }
        // Opportunist (a foe's onFoeAfterBoost): the rises that happened, copied later
        if changed && !self.opp_flushing {
            let after = self.mon(s, i).boosts;
            for k in 0..2 {
                if let Some(f) = self.active(1 - s, k) {
                    if self.mon(1 - s, f).ability == Ab::Opportunist && self.mon(1 - s, f).alive() {
                        for st in 1..8 {
                            let rise = (after[st] - before_all[st]).max(0);
                            let p = &mut self.sides[1 - s].mons[f].opp_pending[st];
                            *p = p.saturating_add(rise);
                        }
                    }
                }
            }
        }
        // Showdown: onAfterEachBoost, once per stat that went down
        if dropped && by_foe {
            for _ in 0..n_dropped {
                match ab {
                    Ab::Defiant => {
                        self.boost(dex, s, i, &[0, 2, 0, 0, 0, 0, 0, 0], None, false);
                    }
                    Ab::Competitive => {
                        self.boost(dex, s, i, &[0, 0, 0, 2, 0, 0, 0, 0], None, false);
                    }
                    _ => {}
                }
            }
        }
        // (White Herb is checked at the next update point, like Showdown's onUpdate)
        if reflected.iter().any(|&x| x != 0) {
            if let Some((ss, si)) = source {
                if (ss, si) != (s, i) {
                    self.mirror_bounce = true;
                    self.boost(dex, ss, si, &reflected, None, true);
                    self.mirror_bounce = false;
                }
            }
        }
        changed
    }

    /// Opportunist copies the collected rises (Showdown: after any move / switch-in / Mega Evolution, and at the end of the turn)
    fn opportunist_flush(&mut self, dex: &Dex) {
        for s in 0..2 {
            for k in 0..2 {
                let Some(i) = self.active(s, k) else { continue };
                let p = self.mon(s, i).opp_pending;
                if p.iter().any(|&x| x != 0) {
                    self.mon_mut(s, i).opp_pending = [0; 8];
                    if self.mon(s, i).alive() && self.mon(s, i).ability == Ab::Opportunist {
                        self.opp_flushing = true;
                        self.boost(dex, s, i, &p, None, false);
                        self.opp_flushing = false;
                    }
                }
            }
        }
    }

    /// Showdown's Update points: items that react to the current state (White Herb).
    fn update_items(&mut self, dex: &Dex) {
        // a Cloud Nine / Air Lock user that fainted ends the suppression (Showdown faintMessages runs its ability's End ->
        // WeatherChange); update points are where faints are processed. Only a CHANGE of suppression counts: Gen 9 Forecast
        // reacts to onStart / WeatherChange, not to updates (3353093: a Castform still Normal in rain stays Normal)
        self.weather_supp_check(dex);
        for s in 0..2 {
            for k in 0..2 {
                if let Some(i) = self.active(s, k) {
                    self.klutz_restore(s, i);
                    // (2026-10-17 targeted fuzz) losing the Illusion ability (Entrainment, Skill Swap, Worry Seed, Mummy,
                    // Gastro Acid...) ends the disguise (Showdown: End event of the old ability); Imposter can copy it then
                    if self.mon(s, i).illusion
                        && (self.mon(s, i).ability != Ab::Illusion || self.mon(s, i).vol2 & vol2::GASTRO != 0) {
                        self.mon_mut(s, i).illusion = false;
                        self.mon_mut(s, i).rev |= rev::SEEN;
                        self.add(|| format!("|-end|{s}|{i}|Illusion"));
                    }
                    if self.mon(s, i).trace_seek {
                        if self.mon(s, i).alive() && self.mon(s, i).ability == Ab::Trace {
                            self.ability_start(dex, s, i);
                        } else {
                            self.mon_mut(s, i).trace_seek = false;
                        }
                    }
                    let rf = self.mon(s, i).recv_from;
                    if rf != u8::MAX {
                        self.mon_mut(s, i).recv_from = u8::MAX;
                        let aid = self.mon(s, rf as usize).ability_id;
                        let ak = if aid != u16::MAX { self.ab_for(dex, s, i, aid) } else { Ab::None };
                        if self.mon(s, i).alive() && self.mon(s, i).ability == Ab::Receiver && aid != u16::MAX && !dex.abilities[aid as usize].no_receiver {
                            let m = self.mon_mut(s, i);
                            m.ability_id = aid;
                            m.ability = ak;
                            m.vol &= !(vol::UNBURDEN | vol::LIBERO_USED); // the old ability ends (Showdown End: Unburden drops its volatile; a new abilityState: Libero / Protean can fire again)
                            self.ability_start(dex, s, i);
                        }
                    }
                    self.ability_cure(s, i);
                    self.status_berry(dex, s, i);
                    if !self.herb_skip {
                        self.white_herb(dex, s, i);
                    }
                    self.check_sitrus(s, i);
                    let lum = self.mon(s, i).has_item() && dex.items[self.mon(s, i).item_id as usize].id == "lumberry";
                    if (self.mon(s, i).item == It::PersimBerry || lum) && self.mon(s, i).confusion > 0 && self.mon(s, i).alive() && !self.berries_blocked_for(s) {
                        self.eat_berry_now(s, i);
                    }
                    let m = self.mon(s, i);
                    if m.has_item() && m.item == It::MentalHerb && (m.taunt > 0 || m.encore > 0 || m.disable > 0 || m.heal_block > 0
                        || m.vol2 & (vol2::ATTRACT | vol2::TORMENT) != 0)
                    {
                        let m = self.mon_mut(s, i);
                        m.vol2 &= !(vol2::ATTRACT | vol2::TORMENT);
                        m.taunt = 0;
                        m.encore = 0;
                        m.encore_slot = NONE;
                        m.disable = 0;
                        m.disable_slot = NONE;
                        m.heal_block = 0;
                        self.consume_item(s, i);
                    }
                }
            }
        }
    }

    /// (2026-10-15 fuzzing) Showdown's ability onUpdate cures: an ability gained while already statused (Trace, Mega
    /// Evolution, Skill Swap, Role Play...) cures the status it prevents at the next update point.
    fn ability_cure(&mut self, s: usize, i: usize) {
        let m = self.mon(s, i);
        let cure = match m.ability {
            Ab::Insomnia | Ab::Vitalspirit => m.status == Status::Sleep,
            Ab::Limber => m.status == Status::Paralysis,
            Ab::Immunity | Ab::Pastelveil => matches!(m.status, Status::Poison | Status::Toxic),
            Ab::Waterveil | Ab::WaterBubble | Ab::Thermalexchange => m.status == Status::Burn,
            Ab::Magmaarmor => m.status == Status::Freeze,
            _ => false,
        };
        if cure {
            let m = self.mon_mut(s, i);
            m.status = Status::None;
            m.status_turns = 0;
        }
        let m = self.mon(s, i);
        if m.ability == Ab::Owntempo && m.confusion > 0 {
            self.mon_mut(s, i).confusion = 0;
        }
        let m = self.mon(s, i);
        if m.ability == Ab::Oblivious && m.taunt > 0 {
            self.mon_mut(s, i).taunt = 0;
        }
    }

    fn white_herb(&mut self, dex: &Dex, s: usize, i: usize) {
        let m = self.mon(s, i);
        if m.item == It::WhiteHerb && m.boosts.iter().any(|&x| x < 0) {
            let m = self.mon_mut(s, i);
            for x in m.boosts.iter_mut() {
                if *x < 0 {
                    *x = 0;
                }
            }
            self.consume_item(s, i);
        }
    }

    fn side_has_ability(&self, dex: &Dex, s: usize, id: &str) -> bool {
        (0..2).any(|k| self.active(s, k).map(|i| dex.abilities[self.mon(s, i).ability_id as usize].id == id).unwrap_or(false))
    }

    /// An active, alive Pokemon on side `s` has an ability matching `f`.
    #[inline]
    fn side_has(&self, s: usize, f: impl Fn(Ab) -> bool) -> bool {
        let side = &self.sides[s];
        for k in 0..2 {
            let i = side.active[k];
            if i != NONE {
                let m = &side.mons[i as usize];
                if m.alive() && f(m.ability) {
                    return true;
                }
            }
        }
        false
    }

    /// Try to give a status condition. Returns whether it worked.
    pub fn set_status(&mut self, dex: &Dex, s: usize, i: usize, st: Status, source: Option<(usize, usize)>) -> bool {
        let m = self.mon(s, i);
        if !m.alive() || m.status != Status::None || st == Status::None {
            return false;
        }
        let mut ab = m.ability;
        // a Mold Breaker user's move ignores breakable status-blocking abilities (Comatose isn't breakable)
        let brk = source.is_some_and(|(ss, si)| (ss, si) != (s, i) && self.cur_user == (ss as u8, si as u8)
            && matches!(self.mon(ss, si).ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze));
        if brk && matches!(ab, Ab::Waterveil | Ab::WaterBubble | Ab::Thermalexchange | Ab::Limber | Ab::Immunity | Ab::Pastelveil
            | Ab::Insomnia | Ab::Vitalspirit | Ab::Sweetveil | Ab::Magmaarmor | Ab::Leafguard | Ab::Flowerveil | Ab::Purifyingsalt)
        {
            ab = Ab::None;
        }
        let immune = match st {
            Status::Burn => m.has_type(dex.ty.fire) || matches!(ab, Ab::Waterveil | Ab::WaterBubble | Ab::Thermalexchange),
            Status::Paralysis => m.has_type(dex.ty.electric) || ab == Ab::Limber,
            Status::Poison | Status::Toxic => {
                let corrosion = source.map(|(ss, si)| self.mon(ss, si).ability == Ab::Corrosion).unwrap_or(false);
                ((m.has_type(dex.ty.poison) || m.has_type(dex.ty.steel)) && !corrosion) || matches!(ab, Ab::Immunity | Ab::Pastelveil)
            }
            Status::Sleep => {
                matches!(ab, Ab::Insomnia | Ab::Vitalspirit | Ab::Sweetveil)
                    || (self.field.terrain == Terrain::Electric && self.grounded_brk(dex, s, i, brk) && !self.semi_invulnerable(dex, s, i))
                    || self.uproar_active()
            }
            Status::Freeze => m.has_type(dex.ty.ice) || ab == Ab::Magmaarmor || self.weather() == Weather::Sun,
            Status::None => true,
        };
        // terrains don't protect a semi-invulnerable Pokemon (Showdown onSetStatus: Yawn's sleep lands mid-Dig; 2026-10-17)
        let misty = self.field.terrain == Terrain::Misty && self.grounded_brk(dex, s, i, brk) && !self.semi_invulnerable(dex, s, i);
        let flower_veil = !brk && self.side_has(s, |a| a == Ab::Flowerveil) && self.mon(s, i).has_type(dex.ty.grass) && source.map(|(ss, _)| ss != s).unwrap_or(true);
        let umbrella = self.mon(s, i).has_item() && dex.items[self.mon(s, i).item_id as usize].id == "utilityumbrella";
        let leaf_guard = ab == Ab::Leafguard && self.weather() == Weather::Sun && !umbrella;
        let safeguard = source.map(|(ss, si)| self.safeguarded(s, i, ss, si)).unwrap_or(false);
        // (2026-10-18 targeted fuzz 4009378) an ALLY's Sweet Veil is suppressed by the user's Mold Breaker even for the user's own
        // status (Showdown suppressingAbility: any holder but the active Pokemon itself): a Mold Breaker Tinkaton can Rest
        let brk_self = source.is_some_and(|(ss, si)| (ss, si) == (s, i) && self.cur_user == (ss as u8, si as u8)
            && matches!(self.mon(ss, si).ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze));
        let sweet_veil = !brk && st == Status::Sleep && self.side_has(s, |a| a == Ab::Sweetveil)
            && !(brk_self && self.mon(s, i).ability != Ab::Sweetveil);
        if immune || misty || flower_veil || leaf_guard || safeguard || sweet_veil || ab == Ab::Comatose || ab == Ab::Purifyingsalt {
            return false;
        }
        let turns = match st {
            // (2026-10-28, hidden counters) Champions' 2-3 turn length ([2, 3, 3]) is never drawn up front: the engine keeps
            // only the public count of turns slept (0x80 | decrements so far) and decides at each check whether it wakes,
            // from the posterior given that count (BeforeMove). So a copy of the position (search worlds, children, rollouts)
            // never inherits a hidden length. Rest's 3 turns are known and stay exact (set after this call).
            Status::Sleep => 0x80,
            Status::Freeze => 3,
            _ => 0,
        };
        let m = self.mon_mut(s, i);
        m.status = st;
        m.status_turns = turns;
        self.add(|| format!("|-status|{s}|{i}|{st:?}"));
        // Synchronize (AfterSetStatus) passes it back BEFORE a Lum Berry cures it (the berry waits for the next update)
        if let Some((ss, si)) = source {
            if (ss, si) != (s, i) && self.mon(s, i).ability == Ab::Synchronize && !self.status_from_tspikes
                && matches!(st, Status::Burn | Status::Paralysis | Status::Poison | Status::Toxic)
            {
                // source = the Synchronize holder (Showdown trySetStatus(status, target)): Safeguard on the other side blocks it
                self.set_status(dex, ss, si, st, Some((s, i)));
            }
        }
        // Lum Berry / the matching status berry cures it at once (unless Unnerve blocks berries)
        self.status_berry(dex, s, i);
        true
    }

    /// Lum Berry / the status berry matching the CURRENT status is eaten (set_status, and at update points: a berry just
    /// stolen by Thief / Covet / Pickpocket / Trick cures its new holder at once, Showdown onUpdate)
    fn status_berry(&mut self, dex: &Dex, s: usize, i: usize) -> bool {
        let st = self.mon(s, i).status;
        if st == Status::None || !self.mon(s, i).alive() {
            return false;
        }
        if self.mon(s, i).has_item() {
            let id = dex.items[self.mon(s, i).item_id as usize].id.as_str();
            let cures = id == "lumberry"
                || matches!((id, st), ("cheriberry", Status::Paralysis) | ("chestoberry", Status::Sleep) | ("pechaberry", Status::Poison)
                    | ("pechaberry", Status::Toxic) | ("rawstberry", Status::Burn) | ("aspearberry", Status::Freeze));
            let unnerved = (0..2).any(|k| self.active(1 - s, k).map(|j| matches!(self.mon(1 - s, j).ability, Ab::Unnerve | Ab::AsOne)).unwrap_or(false));
            if cures && !unnerved {
                let bid = self.mon(s, i).item_id;
                self.consume_item(s, i);
                let qe = !self.in_action;
                let m = self.mon_mut(s, i);
                m.last_berry_sitrus = false;
                // Cud Chew chews a status berry again too (2026-10-17: Cheri Berry after Nuzzle)
                if m.ability == Ab::Cudchew {
                    m.cud_chew = if qe { 1 } else { 2 };
                    m.cud_berry = bid;
                }
                m.status = Status::None;
                m.status_turns = 0;
                if self.mon(s, i).ability == Ab::Cheekpouch {
                    let amt = self.mon(s, i).maxhp / 3;
                    self.heal(s, i, amt, "cheekpouch");
                }
                return true;
            }
        }
        false
    }

    /// Damage a Pokemon (not through the damage formula: recoil, weather, status...).
    fn hurt(&mut self, s: usize, i: usize, amount: u16, tag: &str) {
        let m = self.mon_mut(s, i);
        if !m.alive() {
            return;
        }
        let amount = amount.max(1).min(m.hp);
        let before = m.hp;
        m.hp -= amount;
        let hp_now = m.hp;
        self.add(|| format!("|-damage|{s}|{i}|{hp_now}|[from] {tag}"));
        let m = self.mon_mut(s, i);
        // HP costs are Showdown's directDamage: no hurtThisTurn (Assurance, Burning Jealousy... don't see them)
        if !matches!(tag, "bellydrum" | "substitute" | "clangoroussoul" | "shedtail" | "cursecost") {
            m.vol |= vol::DAMAGED_THIS_TURN;
        }
        let half = m.maxhp / 2;
        if m.ability == Ab::EmergencyExit && m.hp > 0 && m.hp <= half && before > half && !m.has_vol(vol::SWITCHING_IN)
            && self.residual_hp.is_none()
            && !matches!(tag, "bellydrum" | "substitute" | "clangoroussoul" | "confusion" | "liquidooze")
        {
            // (2026-10-18 targeted fuzz 4082086: a confusion self-hit is not an EmergencyExit point in Showdown; it runs in
            // BeforeMove, outside every runEvent('EmergencyExit') call site)
            // (2026-10-18 coverage 7021935: Liquid Ooze damage is applied inside the hit (spreadDamage's drain), before
            // Showdown records the attacker's HP for its EmergencyExit checks, so it never counts as the crossing)
            if let Some(slot) = self.slot_of(s, i) {
                self.mon_mut(s, i).vol |= vol::SWITCHING_IN;
                self.request_switch(s, slot);
            }
        }
        let m = self.mon_mut(s, i);
        if let Some(d) = &mut self.diag {
            let e = d.damage.entry(tag.to_string()).or_default();
            e.0 += 1;
            e.1 += amount as u64;
        }
        let m = self.mon_mut(s, i);
        if m.hp == 0 {
            self.faint(s, i);
        } else if !(self.residual_hp.is_some() && self.in_action) {
            // (residual damage: Showdown eats the berry at the Update after ALL residual handlers, and not at all when the
            // game ends in them; 2026-10-18 targeted fuzz, case 4547178. Sandstorm clears in_action: its Update comes at once)
            self.check_sitrus(s, i);
        }
    }

    /// Sitrus Berry at half HP or less (Showdown checks it on every Update, so after any damage).
    fn check_sitrus(&mut self, s: usize, i: usize) {
        let m = self.mon(s, i);
        if !matches!(m.item, It::SitrusBerry | It::OranBerry) || !m.alive() || m.hp > m.maxhp / 2 || m.heal_block > 0 {
            return;
        }
        if self.berries_blocked_for(s) {
            return;
        }
        self.eat_berry_now(s, i);
    }

    /// An Unnerve / As One foe keeps this side's Pokemon from eating berries.
    fn berries_blocked_for(&self, s: usize) -> bool {
        (0..2).any(|k| self.active(1 - s, k).map(|j| matches!(self.mon(1 - s, j).ability, Ab::Unnerve | Ab::AsOne) && self.mon(1 - s, j).alive()).unwrap_or(false))
    }

    /// Eat the held berry now and do what it does (Sitrus / Oran heal, status berries, Persim, Leppa; resist berries do nothing).
    /// Cud Chew will chew it again and Harvest can grow it back; Cheek Pouch heals on top.
    fn eat_berry_now(&mut self, s: usize, i: usize) {
        let id = self.mon(s, i).item_id;
        if id == u16::MAX {
            return;
        }
        let kind = self.mon(s, i).item;
        self.consume_item(s, i);
        // Cud Chew (eaten after the turn's last action -> eaten again at this turn's end)
        let qe = !self.in_action;
        let m = self.mon_mut(s, i);
        m.last_berry_sitrus = kind == It::SitrusBerry;
        if m.ability == Ab::Cudchew {
            m.cud_chew = if qe { 1 } else { 2 };
            m.cud_berry = id;
        }
        self.berry_effect(s, i, id);
        if self.mon(s, i).ability == Ab::Cheekpouch {
            let amt = self.mon(s, i).maxhp / 3;
            self.heal(s, i, amt, "cheekpouch");
        }
    }

    /// What a berry does when it is eaten (the item is already gone). `id` is its index in Dex::items via the kind table.
    fn berry_effect(&mut self, s: usize, i: usize, id: u16) {
        let ripen = self.mon(s, i).ability == Ab::Ripen;
        let kind = crate::dex::berry_kind(id);
        if ripen {
            self.mon_mut(s, i).berry_weaken = kind == 11;
        }
        match kind {
            1 => {                                                    // Sitrus: 1/4 max HP
                let amt = self.mon(s, i).maxhp / if ripen { 2 } else { 4 };
                self.heal(s, i, amt, "sitrusberry");
            }
            2 => self.heal(s, i, if ripen { 20 } else { 10 }, "oranberry"),   // Oran: 10 HP
            3 => {                                                    // Lum: any status and confusion
                let m = self.mon_mut(s, i);
                m.status = Status::None;
                m.status_turns = 0;
                m.confusion = 0;
            }
            4 => self.cure_status_if(s, i, |st| st == Status::Paralysis),
            5 => self.cure_status_if(s, i, |st| st == Status::Sleep),
            6 => self.cure_status_if(s, i, |st| matches!(st, Status::Poison | Status::Toxic)),
            7 => self.cure_status_if(s, i, |st| st == Status::Burn),
            8 => self.cure_status_if(s, i, |st| st == Status::Freeze),
            9 => self.mon_mut(s, i).confusion = 0,                    // Persim
            10 => {                                                   // Leppa: 10 PP to the first move that is short
                let m = self.mon_mut(s, i);
                if let Some(k) = (0..m.n_moves as usize).find(|&k| m.pp[k] == 0).or_else(|| (0..m.n_moves as usize).find(|&k| m.pp[k] < 8)) {
                    m.pp[k] = m.pp[k].saturating_add(10);
                }
            }
            _ => {}
        }
    }

    /// Magic Room begins: every held item (also on the bench) stops working. The item is put aside and comes back with the room.
    fn magic_room_start(&mut self) {
        for s in 0..2 {
            for m in self.sides[s].mons.iter_mut() {
                if m.item_id != u16::MAX {
                    m.stash_item = (m.item_id, m.item);
                    m.item_id = u16::MAX;
                    m.item = It::None;
                }
                m.choice_lock = NONE;
            }
        }
    }

    fn magic_room_end(&mut self) {
        for s in 0..2 {
            for m in self.sides[s].mons.iter_mut() {
                if m.stash_item.0 != u16::MAX && m.ability != Ab::Klutz {
                    m.item_id = m.stash_item.0;
                    m.item = m.stash_item.1;
                    m.stash_item = (u16::MAX, It::None);
                }
            }
        }
    }

    fn cure_status_if(&mut self, s: usize, i: usize, f: impl Fn(Status) -> bool) {
        let m = self.mon_mut(s, i);
        if f(m.status) {
            m.status = Status::None;
            m.status_turns = 0;
        }
    }

    /// Liquid Ooze damage for a heal of `amount` to (s, i): Big Root's TryHeal only chainModifies, and runEvent applies the
    /// modifier after ALL handlers, so Liquid Ooze's onSourceTryHeal damages by the UNBOOSTED amount (2026-10-19 bridge flag
    /// 22066; scenarios bigroot_*_ooze. Corrects the 2026-10-17 note from case 5076565)
    fn ooze_amt(&self, _s: usize, _i: usize, amount: u16) -> u16 {
        amount
    }

    /// (2026-10-19 bridge flag 18716) the runtime ability for ability id `aid` given to (s, i): abilities are copied by their id
    /// (Showdown getAbility ignores suppression), and Gastro Acid on the RECEIVER keeps the new one suppressed
    fn ab_for(&self, dex: &Dex, s: usize, i: usize, aid: u16) -> Ab {
        if self.mon(s, i).vol2 & vol2::GASTRO != 0 { Ab::None } else { dex.abilities[aid as usize].kind }
    }

    fn heal(&mut self, s: usize, i: usize, amount: u16, tag: &str) {
        let amount = if matches!(tag, "drain" | "leechseed" | "ingrain" | "aquaring" | "strengthsap") && self.mon(s, i).item == It::BigRoot {
            crate::damage::modify(amount as i64, 5324) as u16
        } else {
            amount
        };
        let m = self.mon_mut(s, i);
        if !m.alive() || m.hp == m.maxhp || m.heal_block > 0 {
            return;
        }
        let before = m.hp;
        m.hp = (m.hp + amount.max(1)).min(m.maxhp);
        let gained = m.hp - before;
        let hp_now = m.hp;
        self.add(|| format!("|-heal|{s}|{i}|{hp_now}|[from] {tag}"));
        let m = self.mon_mut(s, i);
        let _ = &m;
        if let Some(d) = &mut self.diag {
            let e = d.heal.entry(tag.to_string()).or_default();
            e.0 += 1;
            e.1 += gained as u64;
        }
    }

    fn note_cant(&mut self, why: &str) {
        self.counters.cant += 1;
        let (cs, ci) = self.cur_user;
        // Showdown runMove: a move stopped in BeforeMove leaves moveThisTurnResult false (Stomping Tantrum, Temper Flare)
        self.mon_mut(cs as usize, ci as usize).move_failed = true;
        if matches!(why, "attract" | "confusion" | "disable" | "flinch" | "frz" | "gravity" | "healblock" | "imprison" | "par" | "slp" | "taunt" | "throatchop") {
            self.move_aborted = true;
        }
        self.add(|| format!("|cant|{cs}|{ci}|{why}"));
        if let Some(d) = &mut self.diag {
            *d.cant.entry(why.to_string()).or_default() += 1;
        }
    }

    fn faint(&mut self, s: usize, i: usize) {
        let m = self.mon_mut(s, i);
        if m.fainted {
            return;
        }
        // (2026-11-05) still disguised = fainted without a damaging hit: the observer sees the imitated Pokemon faint
        if m.illusion && m.rev & rev::EXPLICIT == 0 {
            let t = m.illusion_as as usize;
            if t != i && t < self.sides[s].mons.len() {
                self.sides[s].mons[t].ghost_faint = true;
            }
        }
        let m = self.mon_mut(s, i);
        m.illusion = false;
        if self.defer_hits && i < 8 {
            self.spread_ko_ab[s][i] = self.mon(s, i).ability;
            if self.cur_user == (s as u8, i as u8) && self.spread_user_snap.is_none() {
                self.spread_user_snap = Some(Box::new(self.mon(s, i).clone()));
            }
        }
        // its queued move stays as a placeholder (taken before Transform is undone: the action keeps the chosen move)
        for k in 0..self.queue.len() {
            let q = self.queue[k];
            if q.side as usize == s && q.mon as usize == i {
                if let QKind::Move { mslot, .. } = q.kind {
                    let m = self.mon(s, i);
                    let mv = if (mslot as usize) < 4 { m.moves[mslot as usize] } else { u16::MAX };
                    self.queue[k].kind = QKind::Dead { mv };
                }
            }
        }
        // (2026-10-17 targeted fuzz) fainting undoes Transform (Showdown faintMessages -> clearVolatile): a pending
        // Future Sight from a fainted Ditto hits with Ditto's own stats; Revival Blessing brings back a Ditto
        if self.sides[s].transformed.map(|sv| sv.mon as usize == i).unwrap_or(false) {
            let sv = self.sides[s].transformed.take().unwrap();
            let m = self.mon_mut(s, i);
            m.species = sv.species;
            m.types = sv.types;
            m.added_type = crate::dex::NO_TYPE;
            for k in 1..6 {
                m.stats[k] = sv.stats[k];
            }
            m.weight_hg = sv.weight_hg;
            m.moves = sv.moves;
            m.pp = sv.pp;
            m.n_moves = sv.n_moves;
            m.ability_id = sv.ability_id;
            m.ability = sv.ability;
            m.vol &= !vol::TRANSFORMED;
        }
        // its queued actions are cancelled (even if Revival Blessing brings it back this turn); a move stays as a placeholder
        self.queue.retain(|q| !(q.side as usize == s && q.mon as usize == i && matches!(q.kind, QKind::Mega)));
        if self.defer_hits && i < 8 {
            self.spread_ko[s] |= 1 << i;
        }
        let m = self.mon_mut(s, i);
        m.hp = 0;
        m.fainted = true;
        m.vol = 0;
        // fainting clears stat stages and volatile effects (relevant if Revival Blessing returns it)
        m.boosts = [0; 8];
        m.status = Status::None;
        m.confusion = 0;
        m.taunt = 0;
        m.encore = 0;
        m.encore_slot = NONE;
        m.throat_chop = 0;
        m.magnet_rise = 0;
        m.disable = 0;
        m.perish = 0;
        m.yawn = 0;
        m.sub_hp = 0;
        m.bound = 0;
        m.octolock_src = NONE;
        m.added_type = crate::dex::NO_TYPE;
        m.trap_src = NONE;
        m.opp_pending = [0; 8];
        m.times_attacked = 0;
        m.heal_block = 0;
        m.magnet_rise = 0;
        m.stockpile = 0;
        m.choice_lock = NONE;
        m.protect_streak = 0;
        m.charge_slot = NONE;
        // Receiver / Power of Alchemy on the active ally: takes this ability (at the next update point, which has the dex)
        if let Some(k) = self.slot_of(s, i) {
            if let Some(a) = self.active(s, 1 - k) {
                if a != i && self.mon(s, a).alive() && self.mon(s, a).ability == Ab::Receiver {
                    self.mon_mut(s, a).recv_from = i as u8;
                }
            }
        }
        self.sides[s].fainted_total += 1;
        self.counters.faints += 1;
        self.last_faint_side = s as u8;
        self.add(|| format!("|faint|{s}|{i}"));
    }

    /// the two-turn move (s, i) is charging, if any (its own slot, or one Copycat called)
    fn charging_move(&self, s: usize, i: usize) -> Option<usize> {
        let m = self.mon(s, i);
        let id = match m.charge_slot {
            NONE => return None,
            CALL_SLOT => m.call_move,
            k if (k as usize) < 4 => m.moves[k as usize],
            _ => return None,
        };
        (id != u16::MAX).then_some(id as usize)
    }

    fn underground_or_underwater(&self, dex: &Dex, s: usize, i: usize) -> bool {
        self.charging_move(s, i).is_some_and(|k| matches!(dex.moves[k].id.as_str(), "dig" | "dive"))
    }

    /// In the air / underground / vanished during a two-turn move (Fly, Dig, Phantom Force...).
    fn semi_invulnerable(&self, dex: &Dex, s: usize, i: usize) -> bool {
        self.charging_move(s, i).is_some_and(|k| matches!(dex.moves[k].id.as_str(),
                "fly" | "bounce" | "dig" | "dive" | "phantomforce" | "shadowforce" | "skydrop"))
    }

    fn magic_guard(&self, dex: &Dex, s: usize, i: usize) -> bool {
        self.mon(s, i).ability == Ab::Magicguard
    }

    /// Sitrus Berry and other "after damage" item checks.
    fn after_damage_items(&mut self, dex: &Dex, s: usize, i: usize) {
        let m = self.mon(s, i);
        if !m.alive() || !m.has_item() {
            return;
        }
        let id = dex.items[m.item_id as usize].id.as_str();
        let unnerved = self.side_has(1 - s, |a| matches!(a, Ab::Unnerve | Ab::AsOne));
        let _ = (id, unnerved);
        self.check_sitrus(s, i);
    }

    // ------------------------------------------------------------------
    // Moves
    // ------------------------------------------------------------------
    fn combatant(&self, dex: &Dex, s: usize, i: usize) -> Combatant {
        let m = self.mon(s, i);
        Combatant {
            berry_weaken: m.berry_weaken,
            types: if m.has_vol(vol::ROOST) && m.has_type(dex.ty.flying) {
                // Roost: loses Flying for the turn (pure Flying becomes Normal)
                let fl = dex.ty.flying;
                let rest: ArrayVec<u8, 2> = m.types.iter().copied().filter(|&t| t != fl && t != crate::dex::NO_TYPE).collect();
                match rest.as_slice() {
                    [] => [dex.ty.normal, crate::dex::NO_TYPE],
                    [a] => [*a, crate::dex::NO_TYPE],
                    _ => [rest[0], rest[1]],
                }
            } else {
                m.types
            },
            added_type: m.added_type,
            stats: m.stats,
            hp: m.hp,
            maxhp: m.maxhp,
            ability: m.ability,
            item: m.item,
            item_removable: (m.has_item() || m.stash_item.0 != u16::MAX) && !(m.mega_species != u16::MAX || m.is_mega || self.stone_locked(dex, s, i)),
            has_item: m.has_item() || m.stash_item.0 != u16::MAX,   // Magic Room / Klutz: pokemon.item is still set (Acrobatics)
            boosts: [0, m.boosts[1], m.boosts[2], m.boosts[3], m.boosts[4], m.boosts[5]],
            status: m.status,
            weight_hg: match m.ability {
                Ab::Heavymetal => m.weight_hg * 2,
                Ab::Lightmetal => (m.weight_hg / 2).max(1),
                _ => m.weight_hg,
            },
            // (2026-10-15 fuzzing: was always false, so Light Ball never worked in battles)
            is_pikachu: m.item == It::LightBall && dex.species[m.species as usize].base_species == "pikachu",
            flash_fire: m.has_vol(vol::FLASH_FIRE),
            disguised: false,
            glaive_rush: m.has_vol(vol::GLAIVE_RUSH),
            last_move_failed: m.last_move_failed,
            hurt_this_turn: m.has_vol(vol::DAMAGED_THIS_TURN),
            charged: m.has_vol(vol::CHARGED),
            metro: if m.metro_on { m.metro_n } else { 0 },
            semi: match self.charging_move(s, i).map(|k| dex.moves[k].id.as_str()) {
                Some("dig") => 1,
                Some("dive") => 2,
                Some("fly" | "bounce") => 3,
                _ => 0,
            },
            times_attacked: m.times_attacked,
            forced_grounded: self.field.gravity > 0 || m.vol2 & (vol2::SMACK_DOWN | vol2::INGRAIN) != 0,
            airborne: m.magnet_rise > 0,
            stats_lowered: m.has_vol(vol::STATS_LOWERED),
            level: m.level,
            gender: m.gender,
            retal: m.retal,
        }
    }

    fn conditions(&self, dex: &Dex, s: usize, i: usize, ts: usize, ti: usize, spread: bool) -> Conditions {
        let mut c = Conditions {
            weather: self.weather(),
            terrain: self.field.terrain,
            spread,
            helping_hand: self.mon(s, i).has_vol(vol::HELPING_HAND),
            wonder_room: self.field.wonder_room > 0,
            stakeout: self.mon(ts, ti).active_turns == 0,
            reflect: self.sides[ts].reflect > 0,
            light_screen: self.sides[ts].light_screen > 0,
            aurora_veil: self.sides[ts].aurora_veil > 0,
            fainted_on_attacker_side: self.sides[s].fainted_total,
            supreme_fallen: self.mon(s, i).fallen,
            ..Default::default()
        };
        for side in 0..2 {
            for k in 0..2 {
                let raw = self.sides[side].active[k];
                let ko_now = raw != NONE && (raw as usize) < 8 && self.spread_ko[side] & (1 << raw) != 0;
                if let Some(x) = self.active(side, k).or(if ko_now { Some(raw as usize) } else { None }) {
                    let id = if ko_now { self.spread_ko_ab[side][x] } else { self.mon(side, x).ability };
                    match id {
                        Ab::FairyAura => c.fairy_aura = true,
                        Ab::DarkAura => c.dark_aura = true,
                        _ => {}
                    }
                    // (Friend Guard is breakable: Mold Breaker & co. ignore it)
                    if side == ts && x != ti && id == Ab::FriendGuard
                        && !matches!(self.mon(s, i).ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze) {
                        c.friend_guard = true;
                    }
                    if side == s && x != i && id == Ab::SteelySpirit {
                        c.steely_spirit = true;
                    }
                    if side == s && x != i && matches!(id, Ab::Plus | Ab::Minus) {
                        c.plus_minus = true;
                    }
                    if side != ts && matches!(id, Ab::Unnerve | Ab::AsOne) {
                        c.berries_blocked = true;
                    }
                }
            }
        }
        c
    }

    /// Base power that depends on the battle state (Showdown's basePowerCallback); 0 = the move's own.
    fn state_bp(&self, dex: &Dex, s: usize, i: usize, mv: &MoveData, ts: usize, ti: usize) -> u16 {
        use crate::effects::Mv;
        let bp = mv.base_power as u16;
        match mv.kind {
            Mv::Payback => {
                let t = self.mon(ts, ti);
                let will_move = self.queue.iter().any(|q| matches!(q.kind, QKind::Move { .. }) && q.side as usize == ts && q.mon as usize == ti);
                if t.has_vol(vol::NEWLY_SWITCHED) || will_move { bp } else { bp * 2 }
            }
            Mv::Avalanche => {
                if self.mon(s, i).hit_by & (1 << (ts * 8 + ti)) != 0 { bp * 2 } else { bp }
            }
            Mv::GyroBall => {
                let (us, ts_) = (self.speed(dex, s, i) as u64, self.speed(dex, ts, ti) as u64);
                if us == 0 { 1 } else { ((25 * ts_ / us) + 1).min(150) as u16 }
            }
            Mv::ElectroBall => {
                let (us, ts_) = (self.speed(dex, s, i) as u64, self.speed(dex, ts, ti) as u64);
                let ratio = if ts_ == 0 { 0 } else { us / ts_ };
                [40, 60, 80, 120, 150][ratio.min(4) as usize]
            }
            Mv::Watershuriken => {
                let m = self.mon(s, i);
                if dex.species[m.species as usize].id == "greninjaash" && dex.abilities[m.ability_id as usize].id == "battlebond"
                    && !m.has_vol(vol::TRANSFORMED)
                {
                    bp + 5
                } else {
                    bp
                }
            }
            Mv::Spitup => self.mon(s, i).stockpile as u16 * 100,
            Mv::Reversal => {
                let m = self.mon(s, i);
                let ratio = ((m.hp as u32 * 48) / (m.maxhp as u32).max(1)).max(1);
                match ratio {
                    0..=1 => 200,
                    2..=4 => 150,
                    5..=9 => 100,
                    10..=16 => 80,
                    17..=32 => 40,
                    _ => 20,
                }
            }
            _ => 0,
        }
    }

    /// Who the move actually hits.
    fn target_kind(&self, dex: &Dex, s: usize, i: usize, mv: &MoveData) -> Target {
        if mv.kind == Mv::ExpandingForce && self.field.terrain == Terrain::Psychic && self.is_grounded(dex, s, i) {
            Target::AllAdjacentFoes
        } else {
            mv.target
        }
    }

    fn resolve_targets(&mut self, dex: &Dex, s: usize, i: usize, mv: &MoveData, target: i8) -> Targets {
        self.follow_redirect = false;
        let kind = self.target_kind(dex, s, i, mv);
        let foe = 1 - s;
        let my_slot = self.slot_of(s, i).unwrap_or(0);
        let foes: ArrayVec<usize, 2> = (0..2).filter_map(|k| self.active(foe, k)).collect();
        let ally = self.active(s, 1 - my_slot);
        match kind {
            Target::SelfTarget | Target::AllySide | Target::All | Target::FoeSide => one((s, i)),
            Target::AdjacentAlly => ally.map(|a| one((s, a))).unwrap_or_default(),
            Target::AdjacentAllyOrSelf => {
                if target < 0 && (-target - 1) as usize != my_slot {
                    ally.map(|a| one((s, a))).unwrap_or(one((s, i)))
                } else {
                    one((s, i))
                }
            }
            Target::AllAdjacentFoes => foes.iter().map(|&f| (foe, f)).collect(),
            // Showdown getMoveTargets: the ally first, then the foes (drain / Liquid Ooze / KO order)
            Target::AllAdjacent => {
                let mut v: Targets = Targets::new();
                if let Some(a) = ally {
                    v.push((s, a));
                }
                v.extend(foes.iter().map(|&f| (foe, f)));
                v
            }
            Target::Allies => {
                let mut v = one((s, i));
                if let Some(a) = ally {
                    v.push((s, a));
                }
                v
            }
            // Metal Burst / Comeuppance / Counter / Mirror Coat: whatever stands in the slot of the last foe that hit it
            // this turn (of the right category); nothing = the move fails
            _ if matches!(mv.fixed, crate::dex::FixedDamage::Retaliate(_)) => {
                let k = match mv.fixed { crate::dex::FixedDamage::Retaliate(k) => k as usize, _ => 0 };
                let slot = self.mon(s, i).retal_slot[k];
                if slot == NONE {
                    return Targets::new();
                }
                // (the Pokemon that hit it has fainted since: Showdown retargets to a random remaining foe)
                let x = match self.active(foe, slot as usize).filter(|&x| self.mon(foe, x).alive()) {
                    Some(x) => x,
                    None => {
                        let alive: ArrayVec<usize, 2> = (0..2).filter_map(|k| self.active(foe, k)).filter(|&f| self.mon(foe, f).alive()).collect();
                        // (Counter / Mirror Coat redirect to the attacker through a volatile: gone = no target; Metal Burst's
                        // onModifyTarget finds a random remaining foe)
                        if alive.is_empty() || k != 0 {
                            return Targets::new();
                        }
                        alive[self.pick(alive.len())]
                    }
                };
                // then RedirectTarget like any single-target move (Showdown getMoveTargets runs it after ModifyTarget; Follow
                // Me / Rage Powder, priority 1, also beat Counter's own redirect at -1): Comeuppance into Rage Powder
                // (2026-10-17 Zoroark fuzz 5069105). Rods can't apply (Steel / Dark / Fighting / Psychic moves).
                let att_ab = self.mon(s, i).ability;
                if !matches!(att_ab, Ab::Stalwart | Ab::Propellertail) {
                    let mut best: Option<(i32, u64, usize)> = None;
                    for kk in 0..2 {
                        let Some(r) = self.active(foe, kk) else { continue };
                        let rm = self.mon(foe, r);
                        if !rm.alive() {
                            continue;
                        }
                        let ok = rm.has_vol(vol::FOLLOW_ME) || (rm.has_vol(vol::RAGE_POWDER) && {
                            let me = self.mon(s, i);
                            !(me.has_type(dex.ty.grass) || att_ab == Ab::Overcoat || (me.has_item() && me.item == It::SafetyGoggles))
                        });
                        if ok {
                            let sp = self.action_speed(dex, foe, r);
                            let rmm = self.mon(foe, r);
                            let tie = ((u32::MAX - rmm.ab_epoch) as u64) << 32 | ((rmm.active_turns as u64) * 8 + (3 - (foe * 2 + kk)) as u64);
                            if best.map(|b| (sp, tie) > (b.0, b.1)).unwrap_or(true) {
                                best = Some((sp, tie, r));
                            }
                        }
                    }
                    if let Some(b) = best {
                        self.follow_redirect = true;
                        return one((foe, b.2));
                    }
                }
                one((foe, x))
            }
            // Outrage & co. (randomNormal): a random foe, then redirection like any single-target move (Follow Me pulls it)
            Target::Normal | Target::Other | Target::RandomNormal => {
                let target = if matches!(mv.target, Target::RandomNormal) { 0 } else { target };
                // Stalwart / Propeller Tail / Snipe Shot (tracksTarget): the Pokemon chosen at the start of the turn, if it is
                // still on the field, wherever it stands now (U-turn / Ally Switch moved it; Showdown getTarget)
                if (matches!(self.mon(s, i).ability, Ab::Stalwart | Ab::Propellertail) || mv.kind == Mv::Snipeshot)
                    && !matches!(mv.target, Target::RandomNormal)
                {
                    if let Some((us, ui, ts, ti)) = self.orig_target {
                        let (ts, ti) = (ts as usize, ti as usize);
                        if (us as usize, ui as usize) == (s, i) && self.slot_of(ts, ti).is_some() && self.mon(ts, ti).alive() && (ts, ti) != (s, i) {
                            return one((ts, ti));
                        }
                    }
                }
                // the chosen target, or the other foe if it's gone
                let mut chosen: Option<(usize, usize)> = if target > 0 {
                    self.active(foe, (target - 1) as usize).map(|x| (foe, x))
                } else if target < 0 {
                    self.active(s, (-target - 1) as usize).filter(|&x| x != i).map(|x| (s, x))
                } else {
                    None
                };
                // an ally-slot target that is now the user's own slot (Ally Switch swapped them): Showdown finds no target
                if target < 0 && (-target - 1) as usize == my_slot && mv.kind != Mv::Dragondarts && !matches!(mv.target, Target::RandomNormal) {
                    self.no_target_fail = true;
                    return Targets::new();
                }
                // an ally target that has fainted: the move fails (no retargeting onto a foe) - unless a redirector
                // takes it (Showdown keeps the fainted ally as the target, then RedirectTarget runs; 2026-10-15 fuzzing:
                // Flip Turn aimed at a fainted partner went into Rage Powder)
                // (Dragon Darts = smartTarget: a fainted target means a random foe instead, Showdown getTarget)
                let ally_fainted = chosen.is_none() && target < 0 && (-target - 1) as usize != my_slot && mv.kind != Mv::Dragondarts;
                if ally_fainted {
                    let raw = self.sides[s].active[(-target - 1) as usize];
                    if raw == NONE {
                        return Targets::new();
                    }
                    chosen = Some((s, raw as usize));
                }
                if chosen.is_none() && !foes.is_empty() {
                    chosen = Some((foe, foes[self.pick(foes.len())]));
                }
                // no living foe: Showdown's getRandomTarget still returns a (fainted) foe slot and RedirectTarget runs, so an
                // ally's Lightning Rod / Storm Drain can take the move (2026-10-16: Nuzzle after both foes fell to Discharge)
                let mut placeholder = false;
                if chosen.is_none() && foes.is_empty() && target >= 0 {
                    let raw = self.sides[foe].active[0];
                    if raw != NONE {
                        chosen = Some((foe, raw as usize));
                        placeholder = true;
                    }
                }
                // redirection: Follow Me / Rage Powder, then Lightning Rod / Storm Drain
                // Redirection (Showdown: RedirectTarget, first handler wins; handlers by priority, then holder speed):
                // Follow Me / Rage Powder pull every single-target move of their foes (even one aimed
                // at the attacker's own ally); Lightning Rod / Storm Drain pull moves of their type from
                // anyone. Follow Me / Rage Powder (priority 1) beat Lightning Rod / Storm Drain (0); the fastest wins a tie.
                if chosen.is_some() {
                    let att_ab = self.mon(s, i).ability;
                    let ignores = matches!(att_ab, Ab::Stalwart | Ab::Propellertail) || mv.kind == Mv::Snipeshot;
                    if !ignores {
                        let mt = self.cur_type;
                        // Lightning Rod / Storm Drain are breakable: Mold Breaker / Teravolt / Turboblaze ignore them
                        let mold = matches!(att_ab, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze);
                        let rod = if mold {
                            ""
                        } else if mt == dex.ty.electric {
                            "lightningrod"
                        } else if mt == dex.ty.water {
                            "stormdrain"
                        } else {
                            ""
                        };
                        let mut best: Option<(i32, u64, (usize, usize), bool)> = None;
                        for side in [foe, s] {
                            for k in 0..2 {
                                let Some(r) = self.active(side, k) else { continue };
                                if side == s && r == i {
                                    continue;
                                }
                                let rm = self.mon(side, r);
                                if !rm.alive() {
                                    continue;
                                }
                                let mut ok = false;
                                if side == foe && rm.has_vol(vol::FOLLOW_ME) {
                                    ok = true;
                                } else if side == foe && rm.has_vol(vol::RAGE_POWDER) {
                                    let me = self.mon(s, i);
                                    let immune = me.has_type(dex.ty.grass) || att_ab == Ab::Overcoat
                                        || (me.has_item() && me.item == It::SafetyGoggles);
                                    ok = !immune;
                                }
                                let follow = ok;
                                if !ok && !rod.is_empty() && dex.abilities[rm.ability_id as usize].id == rod {
                                    ok = true;
                                }
                                if ok {
                                    let sp = self.action_speed(dex, side, r);      // Trick Room reverses the handler order too
                                    // equal speed: Showdown orders RedirectTarget handlers by effectOrder (the ability that was
                                    // initialised first = on the field longest, then the starting order: side 0 before side 1, slot a before b)
                                    let rmm = self.mon(side, r);
                                    // (2026-10-17) an ability set up later (switch-in, Transform, Mega) comes after: Ditto that
                                    // transformed into a Lightning Rod user loses the tie to it (Showdown compareRedirectOrder)
                                    let tie = ((u32::MAX - rmm.ab_epoch) as u64) << 32
                                        | ((rmm.active_turns as u64) * 8 + (3 - (side * 2 + k)) as u64);
                                    // Follow Me / Rage Powder (priority 1) before Lightning Rod / Storm Drain (0), then speed
                                    if best.map(|b| (follow, sp, tie) > (b.3, b.0, b.1)).unwrap_or(true) {
                                        best = Some((sp, tie, (side, r), follow));
                                    }
                                }
                            }
                        }
                        if let Some(b) = best {
                            chosen = Some(b.2);
                            self.follow_redirect = b.3;
                            placeholder = false;
                        } else if ally_fainted && mv.kind != Mv::Futuresight {
                            return Targets::new();
                        }
                    } else if ally_fainted && mv.kind != Mv::Futuresight {
                        return Targets::new();
                    }
                    // (Future Sight at a fainted partner's slot still starts: Showdown keeps the fainted target and
                    // futuremove's onTry only needs its position; it hits whoever stands there)
                }
                // (2026-10-17 targeted fuzz) Future Sight with no living foe still starts on that slot (getRandomTarget's
                // foe.active[0]; futuremove's onTry needs only the position) and hits whoever stands there later
                if placeholder && mv.kind != Mv::Futuresight {
                    return Targets::new();
                }
                chosen.into_iter().collect()
            }
        }
    }

    fn run_move(&mut self, dex: &Dex, s: usize, i: usize, mslot: u8, target: i8) {
        let used_before = self.counters.moves;
        if mslot < 4 {
            self.mon_mut(s, i).rev |= 1 << mslot;
            // Destiny Bond ends when it moves again (a second one in a row fails)
            let mid = self.mon(s, i).moves[mslot as usize];
            let is_db = mid != u16::MAX && dex.moves[mid as usize].kind == Mv::Destinybond;
            if self.mon(s, i).has_vol(vol::DESTINY_BOND) {
                self.mon_mut(s, i).vol &= !vol::DESTINY_BOND;
                if is_db {
                    self.cur_user = (s as u8, i as u8);
                    self.counters.moves += 1;
                    self.fail();
                    return;
                }
            }
        }
        // Glaive Rush's drawback ends when it next tries to move (onBeforeMove, priority 100)
        self.mon_mut(s, i).vol &= !vol::GLAIVE_RUSH;
        self.cur_user = (s as u8, i as u8);
        self.gem_active = false;
        self.parental = false;
        self.round_boost_now = self.round_next == Some((s as u8, i as u8));
        if self.round_boost_now {
            self.round_next = None;
        }
        self.move_aborted = false;
        self.run_move_inner(dex, s, i, mslot, target);
        self.called_move = u16::MAX;
        // Charge onMoveAborted: an Electric move stopped in BeforeMove (flinch, sleep, paralysis...) ends the Charge
        if std::mem::take(&mut self.move_aborted) && mslot < 4 && self.mon(s, i).has_vol(vol::CHARGED) {
            let m = self.mon(s, i);
            let slot = if m.encore > 0 && m.encore_slot != NONE { m.encore_slot } else { mslot } as usize;
            let mid = m.moves[slot];
            if mid != u16::MAX && dex.moves[mid as usize].type_ == dex.ty.electric && dex.moves[mid as usize].id != "charge" {
                self.mon_mut(s, i).vol &= !vol::CHARGED;
            }
        }
        if self.counters.moves > used_before {
            let lm = self.mon(s, i).last_move;
            if lm != u16::MAX && dex.moves[lm as usize].kind == Mv::Beakblast {
                self.mon_mut(s, i).vol2 &= !vol2::BEAK_BLAST;
            }
        }
        // lockedmove onAfterMove: its last turn ends the lock (confused if it ran its full course)
        if self.counters.moves > used_before && self.mon(s, i).lock_slot != NONE && self.mon(s, i).lock_dur == 1 {
            self.end_lock(dex, s, i);
        }
        if self.counters.moves > used_before && self.mon(s, i).has_vol(vol::CHARGED) {
            let lm = self.mon(s, i).last_move;
            if lm != u16::MAX && dex.moves[lm as usize].type_ == dex.ty.electric && dex.moves[lm as usize].id != "charge" {
                self.mon_mut(s, i).vol &= !vol::CHARGED;
            }
        }
        // Spit Up: the Stockpile ends and takes its Defense / Sp. Def boosts with it (Showdown onAfterMove -> stockpile onEnd)
        if self.counters.moves > used_before {
            let lm = self.mon(s, i).last_move;
            if lm != u16::MAX && dex.moves[lm as usize].kind == Mv::Spitup && self.mon(s, i).stockpile > 0 {
                let l = self.mon(s, i).stockpile as i8;
                self.mon_mut(s, i).stockpile = 0;
                self.boost(dex, s, i, &[0, 0, -l, 0, -l, 0, 0, 0], Some((s, i)), false);
            }
        }
        // Steel Beam / Mind Blown: half max HP (rounded) to the user once used, hit or miss (not when no target was found)
        let no_tgt = std::mem::take(&mut self.no_target_fail);
        let recoil = std::mem::take(&mut self.recoil_pending);
        if recoil && self.mon(s, i).alive() && !no_tgt && !self.magic_guard(dex, s, i) {
            let amt = (self.mon(s, i).maxhp + 1) / 2;
            self.hurt(s, i, amt, "steelbeam");
        }
    }

    fn run_move_inner(&mut self, dex: &Dex, s: usize, i: usize, mslot: u8, target: i8) {
        self.analytic_move = None;
        // the second turn of a two-turn move: locked in, no PP spent
        let continuing = self.mon(s, i).charge_slot != NONE;
        let (mslot, target) = if continuing {
            let m = self.mon(s, i);
            (m.charge_slot, m.charge_target)
        } else {
            (mslot, target)
        };
        // the slot the player chose (Showdown lastMoveTargetLoc: kept even when Encore re-aims the move below)
        let chosen_target = target;
        // Encore replaced the chosen move: Showdown runMove (OverrideAction) aims the new move with getRandomTarget
        let target = {
            let m = self.mon(s, i);
            if !continuing && mslot < 4 && m.encore > 0 && m.encore_slot != NONE && m.encore_slot != mslot
                && m.moves[m.encore_slot as usize] != u16::MAX
            {
                let em = &dex.moves[m.moves[m.encore_slot as usize] as usize];
                match em.target {
                    Target::Normal | Target::Other | Target::RandomNormal => {
                        let foes: ArrayVec<i8, 2> = (0..2)
                            .filter(|&k| self.active(1 - s, k).is_some_and(|x| self.mon(1 - s, x).alive()))
                            .map(|k| k as i8 + 1)
                            .collect();
                        match foes.len() {
                            0 => target,
                            1 => foes[0],
                            _ => foes[self.rng.below(2) as usize],
                        }
                    }
                    Target::AdjacentAlly => self.slot_of(s, i).map(|k| -((1 - k) as i8) - 1).unwrap_or(0),
                    _ => 0,
                }
            } else {
                target
            }
        };
        self.mon_mut(s, i).charge_slot = NONE;
        // a lock into a move Copycat called (Outrage, Uproar) runs that move again
        let called_lock = {
            let m = self.mon(s, i);
            m.lock_slot == CALL_SLOT || (m.uproar > 0 && m.uproar_slot == CALL_SLOT)
        };
        let mslot = if called_lock { CALL_SLOT } else { mslot };
        let resumed_call = mslot == CALL_SLOT && (continuing || called_lock);
        if resumed_call {
            self.called_move = self.mon(s, i).call_move;
        }
        // a turn locked into Outrage & co. / Uproar spends no PP (Showdown: no deductPP for a locked move)
        let locked_turn = {
            let m = self.mon(s, i);
            mslot < 4 && ((m.lock_slot != NONE && m.lock_slot == mslot) || (m.uproar > 0 && m.uproar_slot == mslot))
        };
        if mslot < 4 && mslot >= self.mon(s, i).n_moves {
            return;
        }
        let move_idx = self.move_of(dex, s, i, mslot);
        let mv = &dex.moves[move_idx];
        // a move called by another (Copycat): Showdown's useMove, no BeforeMove checks, no PP, the user's lastMove stays the caller
        let external = mslot == CALL_SLOT && !resumed_call;
        // (2026-10-17 targeted fuzz) Showdown's runMove counts the action BEFORE BeforeMove: a turn lost to Taunt / flinch /
        // paralysis... still counts, so Fake Out is no longer offered next turn
        if !external {
            let m = self.mon_mut(s, i);
            m.active_move_actions = m.active_move_actions.saturating_add(1);
        }
        // Armor Tail & co.: decided before the move but applied after it is announced (Showdown: onFoeTryMove, after
        // runMove spent the PP; the move becomes the last move used: Copycat copies it, Choice items lock)
        let mut armor_block = false;
        'pre: {
        if external {
            // (2026-10-18) Gravity's onModifyMove also stops a CALLED gravity move (Sleep Talk -> Flying Press / High Jump Kick):
            // 'cant', useMove returns false; the caller itself still succeeded (no move_failed, no onMoveAborted)
            if self.field.gravity > 0 && matches!(mv.kind, Mv::Fly | Mv::Bounce | Mv::FlyingPress | Mv::Highjumpkick | Mv::Jumpkick | Mv::Skydrop | Mv::Magnetrise | Mv::Telekinesis | Mv::Splash | Mv::Floatyfall) {
                self.add(|| format!("|cant|{s}|{i}|gravity"));
                return;
            }
            break 'pre;
        }
        // ---- before the move
        {
            let m = self.mon(s, i);
            if m.has_vol(vol::MUST_RECHARGE) {
                self.mon_mut(s, i).vol &= !vol::MUST_RECHARGE;
                self.note_cant("recharge");
                return;
            }
        }
        // (2026-10-16) Showdown's BeforeMove order: recharge 11, sleep / freeze 10, flinch 8, Disable 7, Gravity / Heal Block /
        // Throat Chop 6, Taunt 5, Imprison 4, confusion 3, attraction 2, paralysis 1 (a sleeping Pokemon hit by Fake Out
        // still counts down its sleep)
        match self.mon(s, i).status {
            Status::Sleep => {
                let dec = if self.mon(s, i).ability == Ab::Earlybird { 2u8 } else { 1 };   // Early Bird: twice as fast
                let st = self.mon(s, i).status_turns;
                let wake = if st & 0x80 != 0 {
                    // (2026-10-28, hidden counters) unknown length L in {2, 3, 3} (decrements until waking), e decrements done
                    // without waking: wakes now iff L <= e + dec. P = 0 (e + dec <= 1), 1/3 (= 2: P(L = 2 | L > e)), 1 (>= 3)
                    let e = st & 0x7f;
                    let w = match e + dec {
                        0 | 1 => false,
                        2 => self.chance_of(1, 3),
                        _ => true,
                    };
                    if !w {
                        self.mon_mut(s, i).status_turns = 0x80 | (e + dec).min(0x7f);
                    }
                    w
                } else {
                    let m = self.mon_mut(s, i);
                    m.status_turns = m.status_turns.saturating_sub(dec);
                    m.status_turns == 0
                };
                if wake {
                    let m = self.mon_mut(s, i);
                    m.status = Status::None;
                    m.status_turns = 0;
                } else if !mv.sleep_usable {
                    self.note_cant("slp");
                    self.mon_mut(s, i).vol |= vol::MOVED;
                    return;
                }
            }
            Status::Freeze => {
                if mv.flags & flag::DEFROST == 0 {
                    let m = self.mon_mut(s, i);
                    m.status_turns = m.status_turns.saturating_sub(1);
                    let thaw = m.status_turns == 0 || self.chance_of(1, 4);
                    if thaw {
                        self.mon_mut(s, i).status = Status::None;
                    } else {
                        self.note_cant("frz");
                        self.mon_mut(s, i).vol |= vol::MOVED;
                        return;
                    }
                } else {
                    self.mon_mut(s, i).status = Status::None;
                }
            }
            _ => {}
        }
        if self.mon(s, i).has_vol(vol::FLINCH) {
            self.note_cant("flinch");
            if self.mon(s, i).ability == Ab::Steadfast {
                let mut b = Boosts::default();
                b[SPE] = 1;
                self.boost(dex, s, i, &b, None, false);
            }
            self.mon_mut(s, i).vol |= vol::MOVED;
            return;
        }
        // (2026-10-17) also the release turn of a two-turn move disabled while charging (Electro Shot; Showdown's Disable
        // onBeforeMove compares move ids only)
        if self.mon(s, i).disable > 0 && mslot < 4 && mslot == self.mon(s, i).disable_slot {
            self.note_cant("disable");
            self.mon_mut(s, i).vol |= vol::MOVED;
            return;
        }
        if self.field.gravity > 0 && matches!(mv.kind, Mv::Fly | Mv::Bounce | Mv::FlyingPress | Mv::Highjumpkick | Mv::Jumpkick | Mv::Skydrop | Mv::Magnetrise | Mv::Telekinesis | Mv::Splash | Mv::Floatyfall) {
            self.note_cant("gravity");
            self.mon_mut(s, i).vol |= vol::MOVED;
            return;
        }
        // Heal Block: moves with the heal flag (incl. draining ones: Matcha Gotcha, Drain Punch...) can't be used
        if self.mon(s, i).heal_block > 0 && mv.flags & flag::HEAL != 0 {
            self.note_cant("healblock");
            self.mon_mut(s, i).vol |= vol::MOVED;
            return;
        }
        if self.mon(s, i).throat_chop > 0 && mv.flags & flag::SOUND != 0 {
            self.note_cant("throatchop");
            self.mon_mut(s, i).vol |= vol::MOVED;
            return;
        }
        if self.mon(s, i).taunt > 0 && mv.category == Category::Status {
            self.note_cant("taunt");
            self.mon_mut(s, i).vol |= vol::MOVED;
            return;
        }
        // Imprison: a foe that used it and knows this move blocks it
        if mv.kind != Mv::Struggle {
            let mid = move_idx as u16;
            let blocked = (0..2).filter_map(|k| self.active(1 - s, k)).any(|f| {
                let fm = self.mon(1 - s, f);
                fm.has_vol(vol::IMPRISON) && fm.moves.contains(&mid)
            });
            if blocked {
                self.note_cant("imprison");
                self.mon_mut(s, i).vol |= vol::MOVED;
                return;
            }
        }
        if self.mon(s, i).confusion & 0x80 != 0 {
            // (2026-10-28, hidden counters) confusion of unknown length (0x80 | moves already attempted while confused):
            // Showdown time T uniform in 2..5, decremented before each move, ends when it reaches 0, i.e. at attempt e + 1
            // iff T = e + 1. P(T = e + 1 | T > e) = 0 (e = 0), 1/4, 1/3, 1/2, 1 (e >= 4). Only the public count is kept.
            let e = self.mon(s, i).confusion & 0x7f;
            let ends = match e {
                0 => false,
                1..=3 => self.chance_of(1, 5 - e as u32),
                _ => true,
            };
            self.mon_mut(s, i).confusion = if ends { 1 } else { 0x80 | (e + 1).min(0x7f) };   // 1 = ends at the decrement below
        }
        if self.mon(s, i).confusion > 0 {
            if self.mon(s, i).confusion & 0x80 == 0 {
                self.mon_mut(s, i).confusion -= 1;
            }
            if self.mon(s, i).confusion > 0 && self.chance_of(33, 100) {
                // hits itself: 40 power typeless physical
                let m = self.mon(s, i);
                let atk = crate::stats::boosted(m.stats[ATK] as u32, m.boosts[ATK]) as i64;
                let def = crate::stats::boosted(m.stats[DEF] as u32, m.boosts[DEF]) as i64;
                let base = ((2 * 50 / 5 + 2) * 40 * atk / def) / 50 + 2;
                let roll = 100 - self.rng.below(16) as i64;
                let d = (base * roll / 100) as u16;
                self.hurt(s, i, d, "confusion");
                self.note_cant("confusion");
                self.mon_mut(s, i).vol |= vol::MOVED;
                return;
            }
        }
        if self.mon(s, i).vol2 & vol2::ATTRACT != 0 {
            let src = self.mon(s, i).attract_src as usize;
            let ss = self.mon(s, i).attract_side as usize;
            if !self.sides[ss].active.contains(&(src as u8)) || !self.mon(ss, src).alive() {
                self.mon_mut(s, i).vol2 &= !vol2::ATTRACT;
            } else if self.chance_of(1, 2) {
                self.note_cant("attract");
                self.add(|| format!("|cant|{s}|{i}|attract"));
                self.mon_mut(s, i).vol |= vol::MOVED;
                return;
            }
        }
        if self.mon(s, i).status == Status::Paralysis && self.chance_of(1, 8) {
            // Champions: fully paralysed 1 time in 8
            self.note_cant("par");
            self.mon_mut(s, i).vol |= vol::MOVED;
            return;
        }
        // Stance Change: Blade forme to attack, Shield forme for King's Shield (Showdown: ModifyMove, so before Armor Tail /
        // Queenly Majesty block the move: a blocked Shadow Sneak still leaves it in Blade forme)
        // (a transformed Pokemon never changes forme: Showdown's onModifyMove returns for attacker.transformed)
        let transformed = self.sides[s].transformed.as_ref().map(|t| t.mon as usize == i).unwrap_or(false);
        if self.mon(s, i).ability == Ab::Stancechange && !transformed {
            let cur = dex.species[self.mon(s, i).species as usize].id.as_str();
            if cur == "aegislash" && mv.category != Category::Status {
                self.forme_change(dex, s, i, "aegislashblade");
            } else if cur == "aegislashblade" && mv.kind == Mv::Kingsshield {
                self.forme_change(dex, s, i, "aegislash");
            }
        }
        // Armor Tail and friends: a priority move aimed at their side doesn't happen
        {
            let kind = self.target_kind(dex, s, i, mv);
            let at_foes = match kind {
                Target::Normal => target > 0,
                Target::AllAdjacentFoes | Target::AllAdjacent | Target::RandomNormal => true,
                _ => false,
            };
            if at_foes && self.move_priority(dex, s, i, mv) > 0 {
                let foe = 1 - s;
                // breakable abilities: Mold Breaker (Mega Emboar...) goes through them
                let mold = matches!(self.mon(s, i).ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze);
                if !mold && self.side_has(foe, |a| matches!(a, Ab::Armortail | Ab::Queenlymajesty | Ab::Dazzling)) {
                    armor_block = true;
                }
            }
        }
        // Focus Punch: hit by a damaging move earlier this turn = it loses its focus (Showdown's beforeMoveCallback)
        if mv.kind == Mv::Focuspunch && self.mon(s, i).vol2 & vol2::LOST_FOCUS != 0 {
            self.note_cant("focuspunch");
            self.mon_mut(s, i).vol |= vol::MOVED;
            return;
        }
        // no PP left at the moment of use (Spite / Eerie Spell / Pressure took it after the choice): Showdown runMove's
        // deductPP fails -> 'cant nopp', the move counts as failed
        {
            let m = self.mon(s, i);
            let real_slot = if mslot < 4 { m.encore_slot } else { NONE };
            let slot = if m.encore > 0 && real_slot != NONE { real_slot } else { mslot };
            if (slot as usize) < 4 && m.pp[slot as usize] == 0 && !continuing && !locked_turn {
                self.note_cant("nopp");
                let m = self.mon_mut(s, i);
                m.used_mask |= 1 << slot;
                m.vol |= vol::MOVED;
                m.move_failed = true;
                return;
            }
        }
        }
        // ---- use the move
        self.counters.moves += 1;
        if let Some(d) = &mut self.diag {
            *d.uses.entry(mv.id.clone()).or_default() += 1;
        }
        self.add(|| format!("|move|{s}|{i}|{}", mv.name));
        if matches!(mv.id.as_str(), "steelbeam" | "mindblown") {
            self.recoil_pending = true;
        }
        let mut metro_prev: Option<(u16, u8)> = None;
        // its volatile goes at the first move after the item was lost / suppressed (Magic Room, Klutz)
        if self.mon(s, i).metro_on && !(self.mon(s, i).has_item() && self.mon(s, i).item == It::Metronome) {
            let m = self.mon_mut(s, i);
            m.metro_on = false;
            m.metro_move = u16::MAX;
            m.metro_n = 0;
        }
        if self.mon(s, i).metro_on && !matches!(mv.id.as_str(), "copycat" | "sleeptalk" | "metronome" | "assist" | "mirrormove" | "mefirst" | "naturepower") {
            let id = move_idx as u16;
            let m = self.mon_mut(s, i);
            metro_prev = Some((m.metro_move, m.metro_n));
            m.metro_n = if m.metro_move == id && (continuing || m.last_move_result == 1) {
                m.metro_n.saturating_add(1)
            } else if continuing {
                1
            } else {
                0
            };
            m.metro_move = id;
        }
        if mv.kind != Mv::Copycat {
            self.last_any_move = move_idx as u16;
        }
        if !external {
            let real_slot = if mslot < 4 { self.mon(s, i).encore_slot } else { NONE };
            // history target code (Mon::h_target): 1 / 2 = opposing slot a / b, 3 = its ally, 0 = itself / none / spread
            let h_tgt: i8 = if target > 0 {
                target
            } else if target < 0 && self.slot_of(s, i).is_some_and(|k| (-target - 1) as usize != k) {
                3
            } else {
                0
            };
            let m = self.mon_mut(s, i);
            let slot = if m.encore > 0 && real_slot != NONE { real_slot } else { mslot };
            // (2026-10-18 targeted fuzz 3951292) Last Resort's 'used' flag is set by deductPP (Showdown moveSlot.used), i.e.
            // only once the move gets past BeforeMove: a turn lost to sleep / flinch / paralysis doesn't count
            if (slot as usize) < 4 && !continuing && !locked_turn {
                m.used_mask |= 1 << slot;
            }
            if (slot as usize) < 4 && m.pp[slot as usize] > 0 && !continuing && !locked_turn {
                m.pp[slot as usize] -= 1;
            }
            if slot < 4 && !continuing {
                // history for the network (see Mon::h_slot)
                m.h_streak = if m.h_slot == slot { m.h_streak.saturating_add(1) } else { 1 };
                m.h_slot = slot;
                m.h_target = h_tgt;
                m.h_uses[slot as usize] = m.h_uses[slot as usize].saturating_add(1);
                m.h_protect = matches!(mv.volatile.as_deref(),
                    Some("protect" | "detect" | "spikyshield" | "kingsshield" | "banefulbunker" | "silktrap" | "burningbulwark"));
                if m.h_protect {
                    m.h_protects = m.h_protects.saturating_add(1);
                } else if mv.category == Category::Status {
                    m.h_status = m.h_status.saturating_add(1);
                }
            }
            m.last_move = move_idx as u16;
            m.last_target = target;
            m.move_result = 1;
            m.vol |= vol::MOVED;
            if matches!(m.item, It::ChoiceBand | It::ChoiceSpecs | It::ChoiceScarf) && m.choice_lock == NONE && slot < 4 {
                m.choice_lock = slot;
            }
        }
        if mslot < 4 && !continuing && !locked_turn {
            let pslot = { let m = self.mon(s, i); if m.encore > 0 && m.encore_slot != NONE { m.encore_slot } else { mslot } } as usize;
            // Pressure: one more PP for each Pressure foe the move aims at
            let kind = self.target_kind(dex, s, i, mv);
            let mut extra = 0u8;
            // (2026-10-18 targeted fuzz, 4744230) a single-target move whose chosen foe has fainted retargets to the other foe
            // (Showdown getMoveTargets), and Pressure counts the ACTUAL target: Ditto's Encore aimed at a fainted Tinkaton
            // lands on Pressure Spiritomb and costs 2 PP
            let mut aim_slot = target;
            if matches!(kind, Target::Normal | Target::Other) && (target == 1 || target == 2) {
                let k0 = (target - 1) as usize;
                let alive = |k: usize| self.active(1 - s, k).is_some_and(|f| self.mon(1 - s, f).alive());
                if !alive(k0) && alive(1 - k0) {
                    aim_slot = (1 - k0) as i8 + 1;
                }
            }
            for k in 0..2 {
                if let Some(f) = self.active(1 - s, k) {
                    let aimed = match kind {
                        Target::Normal | Target::Other => aim_slot == k as i8 + 1,
                        Target::AllAdjacentFoes | Target::AllAdjacent | Target::RandomNormal | Target::FoeSide | Target::All => true,
                        _ => false,
                    };
                    if aimed && self.mon(1 - s, f).ability == Ab::Pressure {
                        extra += 1;
                    }
                }
            }
            let m = self.mon_mut(s, i);
            m.pp[pslot] = m.pp[pslot].saturating_sub(extra);
            if m.pp[pslot] == 0 && m.item == It::LeppaBerry {
                m.pp[pslot] = 10;
                self.consume_item(s, i);
            }
        }
        // a two-turn move's own TryMove (charge) runs before Armor Tail's FoeTryMove: a charge turn is never blocked;
        // with a Power Herb the herb is used first, then the move is blocked
        if armor_block && mv.flags & flag::CHARGE != 0 && !continuing {
            let mega_sol = self.mon(s, i).ability == Ab::MegaSol;
            let w = crate::damage::effective_weather_for(&self.combatant(dex, s, i), &Conditions { weather: self.weather(), ..Default::default() }, mega_sol);
            let instant = (matches!(mv.kind, Mv::SolarBeam | Mv::SolarBlade) && w == Weather::Sun)
                || (mv.kind == Mv::Electroshot && w == Weather::Rain);
            let herb = !instant && self.mon(s, i).has_item() && self.mon(s, i).item == It::PowerHerb;
            if herb {
                self.consume_item(s, i);
            } else if !instant {
                armor_block = false;
            }
        }
        if armor_block {
            self.note_cant("armortail");
            self.mon_mut(s, i).move_result = 0;
            self.mon_mut(s, i).move_failed = true;
            return;
        }
        // Sleep Talk: a random one of its moves (not charge / noSleepTalk ones), at a random target, no PP
        if mv.kind == Mv::Sleeptalk && !external {
            let m = self.mon(s, i);
            if !(m.status == Status::Sleep || m.ability == Ab::Comatose) {
                return self.fail();
            }
            let ids: ArrayVec<u16, 4> = (0..m.n_moves.min(4) as usize).map(|k| m.moves[k])
                .filter(|&id| id != u16::MAX && dex.moves[id as usize].flags & (flag::NOSLEEPTALK | flag::CHARGE) == 0).collect();
            if ids.is_empty() {
                return self.fail();
            }
            let k = self.rng.below(ids.len() as u32) as usize;
            self.called_move = ids[k];
            self.run_move_inner(dex, s, i, CALL_SLOT, 0);
            self.called_move = u16::MAX;
            return;
        }
        // Copycat: the last move anyone used, at a random foe (Showdown getRandomTarget), redirection as usual
        if mv.kind == Mv::Copycat && !external {
            let lm = self.last_any_move;
            if lm == u16::MAX {
                // nothing used yet: Showdown's onHit returns undefined (no failure message), Copycat becomes the last move
                self.last_any_move = move_idx as u16;
                return;
            }
            if dex.moves[lm as usize].flags & flag::FAILCOPYCAT != 0 {
                self.last_any_move = move_idx as u16;
                return self.fail();
            }
            self.called_move = lm;
            self.run_move_inner(dex, s, i, CALL_SLOT, 0);
            self.called_move = u16::MAX;
            return;
        }
        // ---- two-turn moves: charge now, attack next turn (unless the weather or a Power Herb says otherwise)
        if mv.flags & flag::CHARGE != 0 && !continuing {
            // aimed at the user's own position (Ally Switch swapped the slots): Showdown's getTarget finds no target before
            // the charge turn starts (no Power Herb, no boost; only a move already charging keeps its target)
            if target < 0 && matches!(mv.target, Target::Normal | Target::Other) && self.slot_of(s, i) == Some((-target - 1) as usize) {
                self.no_target_fail = true;
                return self.fail();
            }
            if matches!(mv.kind, Mv::Electroshot | Mv::Meteorbeam) {
                self.boost(dex, s, i, &[0, 0, 0, 1, 0, 0, 0, 0], Some((s, i)), false);
            }
            if mv.kind == Mv::Skullbash {
                self.boost(dex, s, i, &[0, 0, 1, 0, 0, 0, 0, 0], Some((s, i)), false);
            }
            let mega_sol = self.mon(s, i).ability == Ab::MegaSol;
            let w = crate::damage::effective_weather_for(&self.combatant(dex, s, i), &Conditions { weather: self.weather(), ..Default::default() }, mega_sol);
            let instant = (matches!(mv.kind, Mv::SolarBeam | Mv::SolarBlade) && w == Weather::Sun)
                || (mv.kind == Mv::Electroshot && w == Weather::Rain);
            let herb = !instant && self.mon(s, i).has_item() && self.mon(s, i).item == It::PowerHerb;
            if herb {
                self.consume_item(s, i);
            }
            if !instant && !herb {
                // Protean / Libero change the type on the charge turn already (Showdown: '-prepare' then the typechange;
                // 2026-10-17 coverage fuzz: Dig)
                let mt = self.eff_type(dex, s, i, mv);
                self.libero(s, i, mv, mt);
                let m = self.mon_mut(s, i);
                // Metronome (item): a charge turn's own TryMove returns null, so the item's TryMove never runs: counter unchanged
                if let Some((pm, pn)) = metro_prev {
                    m.metro_move = pm;
                    m.metro_n = pn;
                }
                // Encore replaced the chosen move: the charging move is the Encored one (2026-10-17 targeted fuzz: Encore
                // into Dig; storing the chosen slot released the wrong move and left the user above ground)
                m.charge_slot = if mslot < 4 && m.encore > 0 && m.encore_slot != NONE { m.encore_slot } else { mslot };
                // twoturnmove stores lastMoveTargetLoc = the CHOSEN slot: when Encore landed THIS turn (OverrideAction re-aims
                // the move, the stored slot stays the chosen one; 2026-10-17 targeted fuzz: Encore turned a self-target choice
                // into Dig). An Encore already active at choice time: the out-of-Encore choice is Showdown's default, whose slot
                // is the random one picked here (case 3358108)
                m.charge_target = chosen_target;
                // a charge move called by another (Copycat): Showdown's twoturnmove stores the slot of the target it resolved
                // NOW (random target, then redirection: Rage Powder / Follow Me), not the caller's chosen slot
                if mslot == CALL_SLOT && matches!(mv.target, Target::Normal | Target::Other | Target::RandomNormal) {
                    let ts = self.resolve_targets(dex, s, i, mv, target);
                    if let Some(&(xs, xi)) = ts.first() {
                        if let Some(k) = self.slot_of(xs, xi) {
                            let code = if xs != s { k as i8 + 1 } else { -(k as i8) - 1 };
                            self.mon_mut(s, i).charge_target = code;
                        }
                    }
                }
                let m = self.mon_mut(s, i);
                if mslot == CALL_SLOT {
                    m.call_move = move_idx as u16;
                }
                return;
            }
        }
        if matches!(mv.kind, Mv::Doubleshock | Mv::Burnup) {
            let t = tid(dex, if mv.kind == Mv::Doubleshock { "Electric" } else { "Fire" });
            if !self.mon(s, i).has_type(t) {
                return self.fail();
            }
        }
        // ---- move-specific "try" checks
        if matches!(mv.kind, Mv::Fakeout | Mv::Firstimpression) && self.mon(s, i).active_move_actions > 1 {
            return self.fail();
        }
        if matches!(mv.kind, Mv::Selfdestruct | Mv::Explosion | Mv::Mistyexplosion) || mv.id == "mindblown" {
            let mold = matches!(self.mon(s, i).ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze);
            if !mold && (self.side_has(0, |a| a == Ab::Damp) || self.side_has(1, |a| a == Ab::Damp)) {
                return self.fail();
            }
        }
        if mv.kind == Mv::Noretreat && self.mon(s, i).has_vol(vol::NO_RETREAT) {
            return self.fail();
        }
        if mv.volatile.as_deref().map(|v| matches!(v, "protect" | "detect" | "spikyshield" | "kingsshield" | "banefulbunker" | "silktrap" | "burningbulwark" | "obstruct" | "endure")).unwrap_or(false) {
            // Protect: fails if nobody moves after, or by chance when used repeatedly
            let others_left = self.will_act();
            let streak = self.mon(s, i).protect_streak;
            let ok = others_left && (streak == 0 || self.chance_of(1, 3u32.pow(streak.min(6) as u32)));
            if !ok {
                self.mon_mut(s, i).protect_streak = 0;
                return self.fail();
            }
            self.mon_mut(s, i).protect_streak = streak + 1;
        } else if matches!(mv.kind, Mv::Wideguard | Mv::Quickguard) {
            // Wide / Quick Guard never fail by chance, but a successful one adds to the stall counter (onHitSide addVolatile 'stall')
            let others_left = self.will_act();
            let streak = self.mon(s, i).protect_streak;
            self.mon_mut(s, i).protect_streak = if others_left { streak + 1 } else { 0 };
            self.mon_mut(s, i).guarded = others_left;
        } else {
            self.mon_mut(s, i).protect_streak = 0;
        }
        match mv.kind {
            Mv::Bellydrum => {
                let m = self.mon(s, i);
                if m.hp <= m.maxhp / 2 || m.boosts[ATK] >= 6 {
                    return self.fail();
                }
                let amt = m.maxhp / 2;
                self.hurt(s, i, amt, "bellydrum");
                self.boost(dex, s, i, &[0, 12, 0, 0, 0, 0, 0, 0], Some((s, i)), false);
                return;
            }
            Mv::Clangoroussoul => {
                let m = self.mon(s, i);
                let cost = m.maxhp as u32 * 33 / 100;
                if (m.hp as u32) <= cost || m.maxhp == 1 || (1..6).all(|k| m.boosts[k] >= 6) {
                    return self.fail();
                }
                self.hurt(s, i, cost as u16, "clangoroussoul");
            }
            _ => {}
        }
        if mv.kind == crate::effects::Mv::Round {
            let pos = self.queue.iter().position(|q| match q.kind {
                QKind::Move { mslot, .. } => {
                    let (qs, qi) = (q.side as usize, q.mon as usize);
                    let m = &self.sides[qs].mons[qi];
                    (mslot as usize) < 4 && m.moves[mslot as usize] != u16::MAX
                        && dex.moves[m.moves[mslot as usize] as usize].kind == crate::effects::Mv::Round
                }
                // Showdown's onTry also picks a fainted Pokemon's queued Round (it never runs, nobody is doubled)
                QKind::Dead { mv } => mv != u16::MAX && dex.moves[mv as usize].kind == crate::effects::Mv::Round,
                _ => false,
            });
            if let Some(p) = pos {
                let mut q = self.queue.remove(p);
                q.order = 0; // stays first even after the queue is re-sorted
                self.round_next = Some((q.side, q.mon));
                self.queue.insert(0, q);
            }
        }
        if mv.kind == Mv::Healingwish && self.sides[s].bench().next().is_none() {
            return self.fail();
        }
        if mv.kind == Mv::Helpinghand {
            let my_slot = self.slot_of(s, i).unwrap_or(0);
            // Showdown willMove(target): the partner Pokemon's own queued move (it follows the Pokemon through Ally Switch)
            let ally = self.active(s, 1 - my_slot);
            let ally_moves = ally.map_or(false, |a| self.queue.iter().any(|q| q.side as usize == s && q.mon as usize == a && matches!(q.kind, QKind::Move { .. })));
            // (newlySwitched: a partner that switched in this turn can still be helped; Showdown onTryHit)
            let newly = ally.map_or(false, |a| self.mon(s, a).has_vol(vol::NEWLY_SWITCHED));
            if !ally_moves && !newly {
                return self.fail();
            }
        }
        // ---- field / side moves
        if mv.category == Category::Status && matches!(mv.target, Target::All | Target::AllySide | Target::FoeSide) {
            let mt = self.eff_type(dex, s, i, mv);
            self.libero(s, i, mv, mt);
            // (2026-10-17) Magic Bounce on the foe side reflects side moves too (Showdown onAllyTryHitSide: Spikes, Stealth Rock,
            // Toxic Spikes, Sticky Web land on the user's own side); breakable, not while the bouncer is semi-invulnerable
            if mv.target == Target::FoeSide && mv.flags & flag::REFLECTABLE != 0
                && !matches!(self.mon(s, i).ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze) {
                let bouncer = (0..2).filter_map(|k| self.active(1 - s, k))
                    .find(|&b| self.mon(1 - s, b).alive() && self.mon(1 - s, b).ability == Ab::Magicbounce
                        && !self.semi_invulnerable(dex, 1 - s, b));
                if let Some(b) = bouncer {
                    self.add(|| format!("|move|{}|{b}|{}|[from] ability: Magic Bounce", 1 - s, mv.name));
                    self.field_move(dex, 1 - s, b, mv);
                    return;
                }
            }
            self.field_move(dex, s, i, mv);
            return;
        }
        self.cur_type = self.eff_type(dex, s, i, mv);
        let mut targets = self.resolve_targets(dex, s, i, mv, target);
        if mv.kind == Mv::Curse {
            if !self.mon(s, i).has_type(dex.ty.ghost) {
                targets.clear();
                targets.push((s, i));
            } else if target == 0 && !external {
                // (2026-10-19 bridge flag 20459) Champions: chosen while NOT a Ghost (the queue set the move's target to 'self'),
                // a Ghost when it moves (Trick-or-Treat earlier in the turn): the move still targets the user, and Champions'
                // onHit pays half HP and curses a random foe directly (no Protect / invulnerability / already-cursed check)
                targets.clear();
                targets.push((s, i));
            } else if targets.first().map(|&(ts, _)| ts == s).unwrap_or(true) {
                let foes: ArrayVec<usize, 2> = (0..2).filter_map(|k| self.active(1 - s, k)).filter(|&f| self.mon(1 - s, f).alive()).collect();
                targets.clear();
                if !foes.is_empty() {
                    let f = foes[self.pick(foes.len())];
                    targets.push((1 - s, f));
                }
            }
        }
        if targets.is_empty() {
            self.no_target_fail = true;
            // selfdestruct 'always' (Explosion, Misty Explosion...): Showdown faints the user after TryMove, BEFORE the
            // no-target fail (2026-10-17 coverage fuzz: Misty Explosion after both foes fainted)
            if mv.self_destruct && !mv.self_destruct_if_hit && self.mon(s, i).alive() {
                let r = self.fail();
                self.faint(s, i);
                return r;
            }
            return self.fail();
        }
        self.analytic_move = Some(!self.queue.iter().any(|q| matches!(q.kind, QKind::Move { .. })));
        // Fling (onPrepareHit): only once the move has a target - a Fling with no target ([notarget]: fainted partner,
        // no foe left) keeps the item (2026-10-17 coverage fuzz)
        if mv.kind == Mv::Fling {
            self.fling_bp = 0;
            if targets.iter().any(|&(a, b)| self.mon(a, b).alive()) {
                let m = self.mon(s, i);
                let bp = if m.has_item() && !self.stone_locked(dex, s, i) { dex.items[m.item_id as usize].fling_bp } else { 0 };
                if bp == 0 {
                    return self.fail();
                }
                let fid = m.item_id;
                self.fling_kind = m.item;
                self.fling_bp = bp;
                self.fling_item = fid;
                self.consume_item(s, i);
            }
        }
        // Shell Side Arm: physical (and contact) when that would do more
        let ssa_owned: MoveData;
        let mv = if mv.kind == Mv::Shellsidearm {
            // Showdown: ModifyMove sees the CHOSEN target, before redirection (Rage Powder / Follow Me)
            let (ts, ti) = if target > 0 {
                self.active(1 - s, (target - 1) as usize).filter(|&x| self.mon(1 - s, x).alive()).map(|x| (1 - s, x)).unwrap_or(targets[0])
            } else {
                targets[0]
            };
            let st = |m: &Mon, k: usize| crate::stats::boosted(m.stats[k] as u32, m.boosts[k]).max(1) as i64;
            let (u, t) = (self.mon(s, i), self.mon(ts, ti));
            let base = 2 * u.level.max(1) as i64 / 5 + 2;
            let phys = base * 90 * st(u, ATK) / st(t, DEF) / 50;
            let spec = base * 90 * st(u, SPA) / st(t, SPD) / 50;
            if phys > spec || (phys == spec && self.rng.chance(1, 2)) {
                let mut c = mv.clone();
                c.category = Category::Physical;
                c.flags |= flag::CONTACT;
                ssa_owned = c;
                &ssa_owned
            } else {
                mv
            }
        } else {
            mv
        };
        let spread = mv.category != Category::Status && targets.len() > 1;
        {
            let m = self.mon(s, i);
            let gem = mv.category != Category::Status && self.cur_type == dex.ty.normal && m.has_item()
                && m.item == It::NormalGem;
            let parental = mv.category != Category::Status && !spread && mv.multihit.1 <= 1
                && mv.flags & flag::CHARGE == 0
                && m.ability == Ab::Parentalbond
                && !matches!(mv.kind, Mv::Fling | Mv::Selfdestruct | Mv::Explosion | Mv::Mistyexplosion | Mv::Finalgambit | Mv::Endeavor
                    | Mv::Iceball | Mv::Rollout | Mv::Uproar | Mv::Dynamaxcannon | Mv::Struggle)
                && mv.kind != crate::effects::Mv::BeatUp;
            self.gem_active = gem;
            self.parental = parental;
        }
        // Sucker Punch / Thunderclap: only if the target is about to use an attacking move
        if matches!(mv.kind, Mv::Suckerpunch | Mv::Thunderclap) {
            let (ts, ti) = targets[0];
            let ok = self.slot_of(ts, ti).map(|_tslot| {
                self.queue.iter().any(|q| {
                    q.side as usize == ts && q.mon as usize == ti && match q.kind {
                        QKind::Move { mslot, .. } => dex.moves[self.move_of(dex, ts, ti, mslot)].category != Category::Status,
                        _ => false,
                    }
                })
            }).unwrap_or(false) && !self.mon(ts, ti).has_vol(vol::MUST_RECHARGE);
            if !ok {
                return self.fail();
            }
        }
        if mv.kind == Mv::Poltergeist && !(self.mon(targets[0].0, targets[0].1).has_item() || self.mon(targets[0].0, targets[0].1).stash_item.0 != u16::MAX) {
            return self.fail();
        }
        {
            let u = self.mon(s, i);
            let bad = match mv.kind {
                Mv::Snore => u.status != Status::Sleep && u.ability != Ab::Comatose,   // only while asleep
                Mv::Belch => !u.ate_berry,                                           // (Champions: not disabled, it just fails)
                Mv::Spitup => u.stockpile == 0,
                // Rest's onTry: fails before Libero / Protean can change the user's type
                Mv::Rest => u.status == Status::Sleep || u.hp >= u.maxhp || matches!(u.ability, Ab::Insomnia | Ab::Vitalspirit | Ab::Comatose),
                Mv::Stuffcheeks => !(u.has_item() && dex.items[u.item_id as usize].is_berry),
                _ => false,
            };
            if bad {
                return self.fail();
            }
        }
        if mv.kind == Mv::Upperhand {
            let (ts, ti) = targets[0];
            let ok = self.slot_of(ts, ti).map(|_tslot| {
                self.queue.iter().any(|q| {
                    q.side as usize == ts && q.mon as usize == ti && match q.kind {
                        QKind::Move { mslot, .. } => {
                            let tm = &dex.moves[self.move_of(dex, ts, ti, mslot)];
                            tm.category != Category::Status && self.move_priority(dex, ts, ti, tm) > 0
                        }
                        _ => false,
                    }
                })
            }).unwrap_or(false);
            if !ok {
                return self.fail();
            }
        }
        if mv.kind == Mv::Lastresort {
            let m = self.mon(s, i);
            let others: u8 = (0..m.n_moves.min(4)).filter(|&k| k != mslot).fold(0, |a, k| a | (1 << k));
            if m.n_moves < 2 || others & !m.used_mask != 0 {
                return self.fail();
            }
        }
        if mv.kind == Mv::Steelroller && self.field.terrain == Terrain::None {
            return self.fail();
        }
        // Aura Wheel (onTry): only Morpeko can use it (Copycat / Instruct callers fail)
        if mv.kind == Mv::Aurawheel && dex.species[self.mon(s, i).species as usize].base_species != "morpeko" {
            return self.fail();
        }
        self.libero(s, i, mv, self.cur_type);
        let mut total_damage: u32 = 0;
        let mut hit_any = false;
        self.self_dropped = false;
        let (prot0, calls0) = (self.counters.protected, self.counters.hit_calls);
        let mut hit_list: ArrayVec<(usize, usize), 4> = ArrayVec::new();
        self.red_card = None;
        self.defer_hits = mv.category != Category::Status;
        self.spread_ko = [0; 2];
        self.spread_user_snap = None;
        self.deferred_kos = 0;
        self.deferred_post.clear();
        self.deferred_dh.clear();
        // Dragon Darts (Showdown's smartTarget): one dart at each foe; a foe that blocks, is immune or dodges
        // sends both darts at the other one
        let darts = if mv.kind == Mv::Dragondarts && targets.len() == 1 && targets[0].0 != s && !self.follow_redirect {
            let (ts, ti) = targets[0];
            self.slot_of(ts, ti).and_then(|k| self.active(ts, 1 - k)).filter(|&o| o != ti && self.mon(ts, o).alive()).map(|o| (ts, ti, o))
        } else {
            None
        };
        // Darts split over two Pokemon: Showdown's hit loop ends with damage [undefined, 2nd dart], so the move still
        // counts as hitting (Life Orb...) but its hitTargets hold only the 2nd dart's target (Magician)
        let mut darts_split = false;
        if let Some((ts, a, b)) = darts {
            self.defer_hits = false;
            self.forced_hits = Some(1);
            let dart_hp0 = [self.mon(ts, a).hp, self.mon(ts, b).hp];
            self.dart_ee_hold = true;
            self.dart_sub.clear();
            let (ha, da) = self.hit_target(dex, s, i, mv, ts, a, false);
            let (hb, db) = if self.mon(s, i).alive() { self.hit_target(dex, s, i, mv, ts, b, false) } else { (false, 0) };
            let mut extra = (false, 0u16, b);
            let again = if ha && !hb { Some(a) } else if hb && !ha { Some(b) } else { None };
            if let Some(x) = again {
                if self.mon(ts, x).alive() && self.mon(s, i).alive() {
                    self.skip_accuracy = true;
                    let (h, d) = self.hit_target(dex, s, i, mv, ts, x, false);
                    self.skip_accuracy = false;
                    extra = (h, d, x);
                }
            }
            self.forced_hits = None;
            self.dart_ee_hold = false;
            darts_split = ha && hb;
            // split darts: Showdown resets targetsCopy to both targets after the hit loop, so a target whose Substitute took
            // its dart still gets AfterMoveSecondary (Eject Button, then Red Card; 2026-10-18 targeted fuzz, case 4381348)
            let subs = std::mem::take(&mut self.dart_sub);
            if darts_split {
                for &(xs, x) in &subs {
                    if !self.mon(xs, x).alive() || self.future_now {
                        continue;
                    }
                    if self.mon(xs, x).has_item() && dex.items[self.mon(xs, x).item_id as usize].id == "ejectbutton" {
                        let flagged = self.switch_needed.iter().any(|q| q.iter().any(|&f| f));
                        if let (Some(slot), false) = (self.slot_of(xs, x), flagged) {
                            if self.sides[xs].bench().next().is_some() {
                                self.consume_item_opt(xs, x, false);
                                self.request_switch(xs, slot);
                            }
                        }
                    } else if self.mon(xs, x).has_item() && self.mon(xs, x).item == It::RedCard && self.red_card.is_none() && self.mon(s, i).alive() {
                        self.red_card = Some((xs as u8, x as u8));
                    }
                }
            }
            {
                // both darts at one foe: Showdown's check after the move, against the HP before it; split over two foes: the
                // damage array is [undefined, 2nd dart], so only the 2nd dart's target is checked (2026-10-17 targeted fuzz)
                let checks: ArrayVec<(usize, u16), 2> = if darts_split {
                    [(b, dart_hp0[1])].into_iter().collect()
                } else {
                    [(a, dart_hp0[0]), (b, dart_hp0[1])].into_iter().collect()
                };
                for (x, hp0) in checks {
                    let m = self.mon(ts, x);
                    if m.ability == Ab::EmergencyExit && m.alive() && m.hp <= m.maxhp / 2 && hp0 > m.maxhp / 2
                        && !m.has_vol(vol::SWITCHING_IN)
                    {
                        if let Some(slot) = self.slot_of(ts, x) {
                            self.mon_mut(ts, x).vol |= vol::SWITCHING_IN;
                            self.request_switch(ts, slot);
                        }
                    }
                }
            }
            for (h, d, x) in [(ha, da, a), (hb, db, b), extra] {
                if h {
                    hit_any = true;
                    if !hit_list.is_full() && !hit_list.contains(&(ts, x)) {
                        hit_list.push((ts, x));
                    }
                }
                total_damage += d as u32;
            }
            targets.clear();
        }
        // Future Sight: nothing hits now; a slot condition on the target's slot hits whoever stands there at the end of
        // the next turn (Showdown onTry -> 'futuremove'); a second one on the same slot fails
        let mut future_set = false;
        if mv.kind == Mv::Futuresight {
            if let Some(&(ts, ti)) = targets.first() {
                match self.slot_of(ts, ti) {
                    Some(k) if self.sides[ts].future[k].0 == 0 => {
                        self.sides[ts].future[k] = (3, s as u8, i as u8);
                        future_set = true;
                    }
                    _ => self.fail(),
                }
            }
            targets.clear();
        }
        // Showdown quirk: Flash Fire's TryHit sets move.accuracy = true, and TryHit runs for every target before the
        // accuracy step, so a spread Fire move that Flash Fire absorbs can't miss its other targets (Protect / Wide
        // Guard / semi-invulnerability stop the move before Flash Fire's handler runs)
        let ff_sure = targets.len() > 1 && self.cur_type == dex.ty.fire
            && !matches!(self.mon(s, i).ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze)
            && targets.iter().any(|&(ts, ti)| {
                let t = self.mon(ts, ti);
                (ts, ti) != (s, i) && t.alive() && t.ability == Ab::FlashFire
                    && !(mv.protectable && (t.has_vol(vol::PROTECT) || self.sides[ts].wide_guard))
                    && !self.semi_invulnerable(dex, ts, ti)
            });
        self.acc_weather = if targets.len() > 1 { Some(self.weather()) } else { None };
        for &(ts, ti) in &targets {
            // (Showdown damages all targets of a spread move before Rocky Helmet & co. react)
            if !self.mon(s, i).alive() && !spread {
                break;
            }
            self.skip_accuracy = ff_sure;
            let (hit, dealt) = self.hit_target(dex, s, i, mv, ts, ti, spread);
            self.skip_accuracy = false;
            if hit {
                hit_any = true;
                if !hit_list.is_full() {
                    hit_list.push((ts, ti));
                }
            }
            total_damage += dealt as u32;
        }
        self.acc_weather = None;
        // the deferred parts: move effects and secondaries for each target, then their reactions
        self.defer_hits = false;
        self.spread_ko = [0; 2];
        let posts = std::mem::take(&mut self.deferred_post);
        for &(ts, ti, hit_sub) in &posts {
            self.post_hit(dex, s, i, mv, ts, ti, hit_sub);
        }
        let dhs = std::mem::take(&mut self.deferred_dh);
        self.eject_defer = dhs.len() > 1;
        self.eject_cands.clear();
        for &(ts, ti, type_mod, hp_before, crit, ab_before) in &dhs {
            self.hp_before_hit = hp_before;
            self.ab_before_hit = ab_before;
            self.innards_prev = 0;
            self.hit_crit = crit;
            self.early_self_drops(dex, s, i, mv);
            self.on_damaging_hit(dex, s, i, mv, ts, ti, type_mod);
        }
        self.eject_defer = false;
        for _ in 0..std::mem::take(&mut self.deferred_kos) {
            self.ko_boost(dex, s, i);
        }
        self.card_defer = posts.len() > 1 && self.red_card.is_none();
        self.card_cands.clear();
        for &(ts, ti, hit_sub) in &posts {
            self.after_hit(dex, s, i, mv, ts, ti, hit_sub);
        }
        self.card_defer = false;
        let mut cc = std::mem::take(&mut self.card_cands);
        if cc.len() > 1 {
            self.sort_by_speed(dex, &mut cc);
        }
        if let Some(&(ts, ti)) = cc.iter().find(|&&(ts, ti)| self.mon(ts, ti).alive() && self.mon(ts, ti).item == It::RedCard) {
            self.red_card = Some((ts as u8, ti as u8));
        }
        // Eject Button (AfterMoveSecondary): the fastest holder hit goes first (Trick Room reverses, ties random); once one
        // Pokemon has its switchFlag the others keep their buttons
        let mut cands = std::mem::take(&mut self.eject_cands);
        if cands.len() > 1 {
            self.sort_by_speed(dex, &mut cands);
        }
        for &(ts, ti) in &cands {
            if !self.mon(ts, ti).alive() || !self.mon(ts, ti).has_item() || dex.items[self.mon(ts, ti).item_id as usize].id != "ejectbutton" {
                continue;
            }
            let flagged = self.switch_needed.iter().any(|x| x.iter().any(|&f| f));
            if let (Some(slot), false) = (self.slot_of(ts, ti), flagged) {
                if self.sides[ts].bench().next().is_some() {
                    self.consume_item_opt(ts, ti, false);
                    self.request_switch(ts, slot);
                }
            }
        }
        // ---- after the move
        if !hit_any && !future_set {
            self.mon_mut(s, i).move_result = 0;
            // every target blocked it with Protect & co. (Showdown NOT_FAIL -> moveThisTurnResult null) = not a failure
            let blocks = self.counters.protected - prot0;
            if !(blocks > 0 && blocks == self.counters.hit_calls - calls0) {
                self.mon_mut(s, i).move_failed = true;
            }
            if matches!(mv.kind, Mv::Highjumpkick | Mv::Jumpkick | Mv::Axekick | Mv::Supercellslam) && self.mon(s, i).alive() && !self.magic_guard(dex, s, i) {
                let amt = (self.mon(s, i).maxhp / 2).max(1);
                self.hurt(s, i, amt, "crash");
            }
        }
        // Outrage & co.: lock in for 2-3 turns (Showdown 'lockedmove': onStart / onRestart); Uproar: 3 turns
        if hit_any && self.mon(s, i).alive() && (mslot < 4 || mslot == CALL_SLOT) {
            if mslot == CALL_SLOT && matches!(mv.self_volatile.as_deref(), Some("lockedmove" | "uproar")) {
                let mid = self.move_of(dex, s, i, mslot) as u16;
                self.mon_mut(s, i).call_move = mid;
            }
            match mv.self_volatile.as_deref() {
                Some("lockedmove") => {
                    if self.mon(s, i).lock_slot == NONE {
                        // (2026-10-28, hidden counters) trueDuration 2-3 unknown until it decides anything (lock_true_now)
                        let m = self.mon_mut(s, i);
                        m.lock_slot = mslot;
                        m.lock_dur = 2;
                        m.lock_true = 0x80;
                    } else if self.lock_true_now(s, i) >= 2 {
                        self.mon_mut(s, i).lock_dur = 2;
                    }
                }
                Some("uproar") => {
                    let m = self.mon_mut(s, i);
                    if m.uproar == 0 {
                        m.uproar = 3;
                        m.uproar_slot = mslot;
                    }
                }
                _ => {}
            }
        }
        if hit_any && mv.self_volatile.as_deref() == Some("glaiverush") {
            self.mon_mut(s, i).vol |= vol::GLAIVE_RUSH;
        }
        if hit_any && self.mon(s, i).alive() && !self.self_dropped {
            // (2026-11-05, sim report) no self stat change once the hit decided the battle; recoil still happens
            if let Some(b) = mv.self_boosts.filter(|_| !self.side_out()) {
                let times = if self.parental { self.last_hits.clamp(1, 2) } else { 1 };
                for _ in 0..times {
                    self.boost(dex, s, i, &b, Some((s, i)), false);
                }
            }
            if mv.self_volatile.as_deref() == Some("mustrecharge") {
                self.mon_mut(s, i).vol |= vol::MUST_RECHARGE;
            }
            if mv.kind == Mv::Struggle && total_damage > 0 {
                // Showdown struggleRecoil: round(max HP / 4) as direct damage (Rock Head / Magic Guard don't stop it)
                let amt = ((self.mon(s, i).maxhp as u32 + 2) / 4).max(1) as u16;
                self.hurt(s, i, amt, "recoil");
            }
            if let Some((num, den)) = mv.recoil {
                let rock_head = self.mon(s, i).ability == Ab::Rockhead;
                if total_damage > 0 && !rock_head && !self.magic_guard(dex, s, i) {
                    let amt = ((total_damage * num as u32 + den as u32 / 2) / den as u32).max(1) as u16;
                    self.hurt(s, i, amt, "recoil");
                }
            }
        }
        if hit_any && matches!(mv.kind, Mv::Doubleshock | Mv::Burnup) {
            let t = tid(dex, if mv.kind == Mv::Doubleshock { "Electric" } else { "Fire" });
            let m = self.mon_mut(s, i);
            m.added_type = crate::dex::NO_TYPE;
            for x in m.types.iter_mut() {
                if *x == t {
                    *x = crate::dex::NO_TYPE;
                }
            }
        }
        if hit_any && matches!(mv.kind, Mv::Steelroller | Mv::Icespinner) {
            self.field.terrain = Terrain::None;
            self.field.terrain_turns = 0;
            self.terrain_changed(dex);
        }
        if mv.self_destruct && (hit_any || !mv.self_destruct_if_hit) && self.mon(s, i).alive() {
            self.faint(s, i);
        }
        // Steel Beam / Mind Blown that hit: the recoil comes in the hit loop (Showdown applyRecoilDamage), BEFORE the targets'
        // AfterMoveSecondary (Red Card: a user the recoil KOs isn't dragged; 2026-10-17)
        // (2026-10-17 targeted fuzz) a hit that dealt 0 damage (the target Endured at 1 HP) gives no recoil: applyRecoilDamage
        // runs only `if (move.totalDamage)`, and the move didn't fail (no onMoveFail recoil either)
        if hit_any && self.recoil_pending && total_damage == 0 {
            self.recoil_pending = false;
        }
        if hit_any && self.recoil_pending && self.mon(s, i).alive() && !self.no_target_fail && !self.magic_guard(dex, s, i) {
            self.recoil_pending = false;
            let amt = (self.mon(s, i).maxhp + 1) / 2;
            self.hurt(s, i, amt, "steelbeam");
        }
        // Red Card: the attacker is dragged out (Showdown: the target's AfterMoveSecondary, before the user's own
        // AfterMoveSecondarySelf effects: Life Orb and Shell Bell check forceSwitchFlag (no recoil / heal), Magician checks only
        // switchFlag and still steals, Sparkling Aria still cures; the drag itself comes after the action (2026-10-19 bridge
        // flag 21978: Magician Delphox took Hippowdon's Leftovers before the Red Card drag)
        let mut card_drag: Option<(usize, ArrayVec<usize, 6>)> = None;
        if let Some((hs, hi)) = self.red_card.take().map(|(a, b)| (a as usize, b as usize)) {
            if self.mon(s, i).alive() && self.mon(hs, hi).alive() && self.mon(hs, hi).item == It::RedCard && self.slot_of(hs, hi).is_some() {
                let bench: ArrayVec<usize, 6> = self.sides[s].bench().collect();
                if let (false, Some(slot)) = (bench.is_empty(), self.slot_of(s, i)) {
                    self.consume_item(hs, hi);   // useItem: lastItem for Pickup / Recycle, Symbiosis
                    // the card is used up even when the attacker can't be dragged out (Guard Dog, Suction Cups, Ingrain:
                    // Showdown runs DragOut after useItem; the attacker's own Mold Breaker doesn't suppress its own ability)
                    if !(matches!(self.mon(s, i).ability, Ab::Guarddog | Ab::Suctioncups) || self.mon(s, i).vol2 & vol2::INGRAIN != 0) {
                        card_drag = Some((slot, bench));
                    }
                }
            }
        }
        // Sparkling Aria's onAfterMove checks source.fainted, which Showdown sets only after the move: Life Orb recoil that KOs the user
        // still lets it cure (2026-10-18, case 4865258)
        let aria_user_ok = self.mon(s, i).alive();
        if hit_any && card_drag.is_none() && mv.category != Category::Status && self.mon(s, i).item == It::LifeOrb && self.mon(s, i).alive() && !self.magic_guard(dex, s, i) {
            let sheer = self.mon(s, i).ability == Ab::SheerForce && !mv.secondaries.is_empty();
            if !sheer {
                let amt = self.mon(s, i).maxhp / 10;
                self.hurt(s, i, amt, "lifeorb");
            }
        }
        // Shell Bell (onAfterMoveSecondarySelf, priority -1: after Life Orb): heals 1/8 of the move's total damage
        if hit_any && card_drag.is_none() && total_damage > 0 && self.mon(s, i).alive() && self.mon(s, i).item == It::ShellBell {
            let sheer = self.mon(s, i).ability == Ab::SheerForce && !mv.secondaries.is_empty();
            if !sheer {
                let amt = ((total_damage / 8) as u16).max(1);
                self.heal(s, i, amt, "shellbell");
            }
        }
        // Sparkling Aria: cures the burn of what it hit (not through Shield Dust / Covert Cloak, which block its secondary)
        if hit_any && mv.kind == Mv::Sparklingaria && aria_user_ok && self.mon(s, i).ability != Ab::SheerForce {
            for &(a, b) in hit_list.iter() {
                let t = self.mon(a, b);
                let dust = (t.ability == Ab::Shielddust && !matches!(self.mon(s, i).ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze)) || (t.has_item() && dex.items[t.item_id as usize].id == "covertcloak");
                if (a, b) != (s, i) && t.alive() && t.status == Status::Burn && (!dust || hit_list.len() > 1) {
                    self.mon_mut(a, b).status = Status::None;
                }
            }
        }
        // Fell Stinger: +3 Attack when its target fainted
        if hit_any && mv.kind == Mv::FellStinger && self.mon(s, i).alive() && hit_list.iter().any(|&(a, b)| !self.mon(a, b).alive()) {
            let mut bo = Boosts::default();
            bo[ATK] = 3;
            self.boost(dex, s, i, &bo, Some((s, i)), false);
        }
        // Magician: steals the item of the first Pokemon it hit
        if darts_split && hit_list.len() > 1 {
            hit_list.remove(0);                                     // Showdown's hitTargets after a split: the 2nd only
        }
        if hit_any && mv.category != Category::Status && mv.kind != Mv::Fling && self.mon(s, i).alive()
            && self.mon(s, i).ability == Ab::Magician && !self.mon(s, i).has_item()
        {
            // Showdown speedSorts hitTargets first (fastest first, Trick Room reverses, ties random; a target the move just
            // KO'd still counts: its item can be taken)
            let mut order: ArrayVec<(usize, usize, i64, u32), 4> = ArrayVec::new();
            for &(ts, ti) in &hit_list {
                let sp = self.speed(dex, ts, ti) as i64;
                let key = if self.field.trick_room > 0 { -sp } else { sp };
                let tie = self.rng.below(1 << 16);
                order.push((ts, ti, key, tie));
            }
            if order.len() > 1 {
                order.sort_by(|a, b| b.2.cmp(&a.2).then(a.3.cmp(&b.3)));
            }
            for &(ts, ti, _, _) in &order {
                if (ts, ti) != (s, i) && self.item_takeable(dex, ts, ti) {
                    self.steal_item(ts, ti, s, i);
                    break;
                }
            }
        }
        if let Some((slot, bench)) = card_drag {
            // (2026-10-17 targeted fuzz) Showdown only sets forceSwitchFlag at the card; the drag comes after the action, after an
            // Update: the attacker's White Herb (Close Combat drops) is used before it leaves
            self.cancel_self_switch = false;
            self.update_items(dex);
            if !self.mon(s, i).alive() || self.slot_of(s, i) != Some(slot) {
                return;
            }
            let to = bench[self.rng.below(bench.len() as u32) as usize];
            self.do_switch(dex, s, slot, to);
            return;
        }
        // U-turn, Flip Turn, Parting Shot...
        let cancelled = std::mem::take(&mut self.cancel_self_switch);
        if mv.self_switch && hit_any && !cancelled && mv.kind != Mv::Revivalblessing && self.mon(s, i).alive() {
            if mv.kind == Mv::Batonpass && self.sides[s].bench().next().is_some() {
                self.set_baton(s, i);
            }
            if let Some(slot) = self.slot_of(s, i) {
                self.request_switch(s, slot);
            }
        }
    }

    /// Baton Pass: what the user passes to its replacement
    fn set_baton(&mut self, s: usize, i: usize) {
        {
            {
                let m = self.mon(s, i);
                self.sides[s].baton = Some(crate::state::BatonVol {
                    boosts: m.boosts,
                    vol: m.vol & (vol::CHARGED | vol::LEECH_SEED | vol::FOCUS_ENERGY | vol::DRAGON_CHEER | vol::DRAGON_CHEER2 | vol::CURSE | vol::NO_RETREAT),
                    vol2: m.vol2 & (vol2::AQUA_RING | vol2::INGRAIN),
                    sub_hp: m.sub_hp,
                    confusion: m.confusion,
                    bound: m.bound,
                    bound_div: m.bound_div,
                    bound_src: m.bound_src,
                    bound_side: m.bound_side,
                    heal_block: m.heal_block,
                    magnet_rise: m.magnet_rise,
                    perish: m.perish,
                    taunt: m.taunt,
                    throat_chop: m.throat_chop,
                    octolock_src: m.octolock_src,
                    octolock_side: m.octolock_side,
                    leech_slot: m.leech_slot,
                    leech_side: m.leech_side,
                });
            }
        }
    }

    fn fail(&mut self) {
        let (s, i) = (self.cur_user.0 as usize, self.cur_user.1 as usize);
        if s < 2 && i < self.sides[s].mons.len() {
            self.mon_mut(s, i).move_result = 0;
            self.mon_mut(s, i).move_failed = true;
        }
        self.add(|| "|-fail|".into());
    }

    /// Hit one target. Returns (the move connected, damage dealt).
    fn hit_target(&mut self, dex: &Dex, s: usize, i: usize, mv: &MoveData, ts: usize, ti: usize, spread: bool) -> (bool, u16) {
        self.counters.hit_calls += 1;
        if !self.mon(ts, ti).alive() {
            return (false, 0);
        }
        let is_self = ts == s && ti == i;
        let foe_target = ts != s;
        if !is_self {
            if let Some(ck) = self.charging_move(ts, ti) {
                let cm = dex.moves[ck].id.as_str();
                let hits_anyway = match cm {
                    "fly" | "bounce" | "skydrop" => matches!(mv.kind, Mv::Gust | Mv::Twister | Mv::Skyuppercut | Mv::Thunder | Mv::Hurricane | Mv::Smackdown | Mv::Thousandarrows),
                    "dig" => matches!(mv.kind, Mv::Earthquake | Mv::Magnitude),
                    "dive" => matches!(mv.kind, Mv::Surf | Mv::Whirlpool),
                    "phantomforce" | "shadowforce" => false,
                    _ => true,
                };
                let noguard = self.mon(s, i).ability == Ab::Noguard
                    || self.mon(ts, ti).ability == Ab::Noguard;
                // Gen 8+: Toxic used by a Poison type hits even semi-invulnerable targets (battle-actions.ts)
                let poison_toxic = mv.id == "toxic" && self.mon(s, i).has_type(dex.ty.poison);
                // Lock-On / Mind Reader: the user's moves reach the locked target mid-Phantom Force / Dig / Fly too
                // (lockon onSourceInvulnerability; 2026-10-17 targeted fuzz: Lock-On then Thief into Phantom Force)
                let locked = self.mon(s, i).lock_on > 0 && ts != s && self.mon(s, i).lock_on_tgt == ti as u8;
                // Helping Hand reaches a partner that is underground / in the air too (hitStepInvulnerabilityEvent)
                if !hits_anyway && !noguard && !poison_toxic && !locked && mv.kind != Mv::Helpinghand {
                    self.counters.misses += 1;
                    self.add(|| format!("|-miss|{ts}|{ti}|semi"));
                    return (false, 0);
                }
            }
        }
        if mv.kind == Mv::Pollenpuff && ts == s && !is_self {
            // on an ally: heals half its HP instead of hitting it - unless Telepathy / Bulletproof (TryHit) stops the move first
            // Protect (TryHit priority 3) blocks it first, as any move with the protect flag aimed at an ally
            if mv.protectable && self.mon(ts, ti).has_vol(vol::PROTECT) {
                self.counters.protected += 1;
                self.add(|| format!("|-activate|{ts}|{ti}|Protect"));
                return (false, 0);
            }
            let mold = matches!(self.mon(s, i).ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze);
            if !mold && matches!(self.mon(ts, ti).ability, Ab::Telepathy | Ab::Bulletproof) {
                return (false, 0);
            }
            // the move hits the ally (Hit event) even when the heal then fails: a charging Focus Punch loses focus
            // (the user's own Heal Block stops the move earlier, in TryMove)
            if self.mon(s, i).heal_block > 0 {
                return (false, 0);
            }
            self.mon_mut(ts, ti).vol2 |= vol2::LOST_FOCUS;
            let m = self.mon(ts, ti);
            let healed = !(m.hp >= m.maxhp || m.heal_block > 0);
            if healed {
                let amt = (m.maxhp / 2).max(1);
                self.heal(ts, ti, amt, "pollenpuff");
            }
            // the ally was hit by a Special move: its AfterMoveSecondary items fire (Eject Button, then Red Card) - only when
            // the heal worked: a failed heal returns NOT_FAIL, which drops the target from the hit list (2026-10-17 coverage fuzz)
            if !healed {
            } else if self.mon(ts, ti).has_item() && dex.items[self.mon(ts, ti).item_id as usize].id == "ejectbutton" {
                let flagged = self.switch_needed.iter().any(|x| x.iter().any(|&f| f));
                if let (Some(slot), false) = (self.slot_of(ts, ti), flagged) {
                    if self.sides[ts].bench().next().is_some() {
                        self.consume_item_opt(ts, ti, false);
                        self.request_switch(ts, slot);
                    }
                }
            } else if self.mon(ts, ti).has_item() && self.mon(ts, ti).item == It::RedCard && self.red_card.is_none() {
                self.red_card = Some((ts as u8, ti as u8));
            }
            return (healed, 0);
        }
        // Psychic Terrain (TryHit priority 4) comes before Protect (3)
        // (a Mold Breaker attacker ignores the target's Levitate: grounded, so Psychic Terrain blocks its priority move;
        // Showdown isGrounded -> hasAbility; 2026-10-18 targeted fuzz, case 4620018)
        let brk_pt = matches!(self.mon(s, i).ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze);
        if foe_target && self.field.terrain == Terrain::Psychic && self.grounded_brk(dex, ts, ti, brk_pt) && self.move_priority(dex, s, i, mv) > 0 {
            return (false, 0);
        }
        // Champions: Unseen Fist / Piercing Drill send contact moves through every protection at 1/4 damage
        self.protect_bypass = false;
        let bypasser = mv.protectable && !is_self && mv.flags & flag::CONTACT != 0 && self.mon(s, i).ability != Ab::Longreach && matches!(self.mon(s, i).ability, Ab::Unseenfist | Ab::Piercingdrill);
        // the 1/4 damage applies only when something would have blocked it: Protect, Wide Guard vs a spread move,
        // Quick Guard vs a priority move (Wide Guard does nothing against a single-target hit)
        if bypasser && (self.mon(ts, ti).has_vol(vol::PROTECT)
            || (self.sides[ts].wide_guard && matches!(self.target_kind(dex, s, i, mv), Target::AllAdjacent | Target::AllAdjacentFoes))
            || (self.sides[ts].quick_guard && self.move_priority(dex, s, i, mv) > 0))
        {
            self.protect_bypass = true;
        }
        // Protect and friends
        // King's Shield / Silk Trap / Burning Bulwark don't block status moves (Showdown checkMoveBypassesProtect,
        // blockStatus = false; 2026-10-15 fuzzing: Parting Shot went through King's Shield in Showdown)
        let status_passes = mv.category == Category::Status
            && matches!(self.mon(ts, ti).protect_kind, ProtectKind::KingsShield | ProtectKind::SilkTrap | ProtectKind::BurningBulwark);
        if mv.protectable && !is_self && self.mon(ts, ti).has_vol(vol::PROTECT) && !mv.breaks_protect && !bypasser && !status_passes {
            self.counters.protected += 1;
            self.add(|| format!("|-activate|{ts}|{ti}|Protect"));
            let contact = mv.flags & flag::CONTACT != 0 && self.mon(s, i).ability != Ab::Longreach;
            if contact {
                match self.mon(ts, ti).protect_kind {
                    ProtectKind::SpikyShield => {
                        let amt = self.mon(s, i).maxhp / 8;
                        if !self.magic_guard(dex, s, i) {
                            self.hurt(s, i, amt, "spikyshield");
                        }
                    }
                    ProtectKind::KingsShield => {
                        self.boost(dex, s, i, &[0, -1, 0, 0, 0, 0, 0, 0], Some((ts, ti)), false);
                    }
                    ProtectKind::SilkTrap => {
                        self.boost(dex, s, i, &[0, 0, 0, 0, 0, -1, 0, 0], Some((ts, ti)), false);
                    }
                    ProtectKind::BanefulBunker => {
                        self.set_status(dex, s, i, Status::Poison, Some((ts, ti)));
                    }
                    ProtectKind::BurningBulwark => {
                        self.set_status(dex, s, i, Status::Burn, Some((ts, ti)));
                    }
                    _ => {}
                }
            }
            return (false, 0);
        }
        // Wide Guard protects its whole side from spread moves (the attacker's ally too); moves without the protect flag pass (Feint)
        if !is_self && !bypasser && mv.protectable && self.sides[ts].wide_guard && matches!(self.target_kind(dex, s, i, mv), Target::AllAdjacent | Target::AllAdjacentFoes) {
            self.counters.protected += 1;
            return (false, 0);
        }
        // (2026-10-18 targeted fuzz 4090313) Quick Guard also stops a priority move from the target's own partner (Showdown
        // quickguard onTryHit has no side check, unlike Psychic Terrain / Armor Tail): Glalie's Ice Shard into its partner
        if !is_self && self.sides[ts].quick_guard && !bypasser && mv.protectable && self.move_priority(dex, s, i, mv) > 0 {
            self.counters.protected += 1;
            return (false, 0);
        }
        if !is_self && foe_target {
            let pr = self.move_priority(dex, s, i, mv);
            if pr > 0 {
                if self.field.terrain == Terrain::Psychic
                    && self.grounded_brk(dex, ts, ti, matches!(self.mon(s, i).ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze)) {
                    return (false, 0);
                }
                if !matches!(self.mon(s, i).ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze)
                    && self.side_has(ts, |a| matches!(a, Ab::Armortail | Ab::Queenlymajesty | Ab::Dazzling)) {
                    return (false, 0);
                }
            }
        }
        // ability immunities ("try hit")
        if !is_self {
            let tab = self.mon(ts, ti).ability;
            let t = self.cur_type;
            let att_ab = self.mon(s, i).ability;
            let mold = matches!(att_ab, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze);
            let absorbed = !mold && match tab {
                Ab::Voltabsorb if t == dex.ty.electric => { let a = self.mon(ts, ti).maxhp / 4; self.heal(ts, ti, a, "voltabsorb"); true }
                Ab::Waterabsorb | Ab::Dryskin if t == dex.ty.water => { let a = self.mon(ts, ti).maxhp / 4; self.heal(ts, ti, a, "waterabsorb"); true }
                Ab::Lightningrod | Ab::Motordrive if t == dex.ty.electric => {
                    let stat = if tab == Ab::Lightningrod { SPA } else { SPE };
                    let mut b = [0i8; 8];
                    b[stat] = 1;
                    self.boost(dex, ts, ti, &b, None, false);
                    true
                }
                Ab::Stormdrain if t == dex.ty.water => { self.boost(dex, ts, ti, &[0, 0, 0, 1, 0, 0, 0, 0], None, false); true }
                Ab::Sapsipper if t == dex.ty.grass => { self.boost(dex, ts, ti, &[0, 1, 0, 0, 0, 0, 0, 0], None, false); true }
                Ab::FlashFire if t == dex.ty.fire => { self.mon_mut(ts, ti).vol |= vol::FLASH_FIRE; true }
                Ab::Wellbakedbody if t == dex.ty.fire => { self.boost(dex, ts, ti, &[0, 0, 2, 0, 0, 0, 0, 0], None, false); true }
                Ab::Eartheater if t == dex.ty.ground => { let a = self.mon(ts, ti).maxhp / 4; self.heal(ts, ti, a, "eartheater"); true }
                Ab::Goodasgold if mv.category == Category::Status => true,
                Ab::Sturdy if mv.fixed == crate::dex::FixedDamage::Ohko => true,
                Ab::Soundproof if mv.flags & flag::SOUND != 0 => true,
                Ab::Bulletproof if mv.flags & flag::BULLET != 0 => true,
                Ab::Overcoat if mv.flags & flag::POWDER != 0 => true,
                Ab::Telepathy if !foe_target && mv.category != Category::Status => true,
                // (also an ally's move: Showdown's onTryHit only skips the user itself)
                Ab::Magicbounce if mv.flags & flag::REFLECTABLE != 0 && (ts, ti) != (s, i) => {
                    // (2026-10-18 bridge fuzz) powder immunity is per target of the bounced move: a bounced Cotton Spore spares a
                    // Grass user but still hits its partner
                    let powder_imm = |b: &Self, xs: usize, xi: usize| mv.flags & flag::POWDER != 0 && (b.mon(xs, xi).has_type(dex.ty.grass)
                        || b.mon(xs, xi).ability == Ab::Overcoat
                        || (b.mon(xs, xi).has_item() && b.mon(xs, xi).item == It::SafetyGoggles));
                    let powder_immune = mv.target != Target::AllAdjacentFoes && powder_imm(self, s, i);
                    // the bounced move keeps the original's priority (Showdown useMoveInner copies activeMove.priority):
                    // a Prankster move bounced back is still +1, so Armor Tail / Queenly Majesty / Dazzling on the original
                    // user's side (TryMove) or Psychic Terrain under a grounded original user (TryHit) stop it
                    let prio = self.move_priority(dex, s, i, mv) > 0;
                    // (2026-10-18 targeted fuzz 4314064) not when the bouncer is the user's own partner: Psychic Terrain skips
                    // allies (target.isAlly(source)) and Armor Tail & co. are onFoeTryMove (Mega Absol bouncing Fake Tears)
                    let bounce_blocked = prio && ts != s
                        && (self.side_has(s, |a| matches!(a, Ab::Armortail | Ab::Queenlymajesty | Ab::Dazzling))
                            || (self.field.terrain == Terrain::Psychic && self.is_grounded(dex, s, i)));
                    // (Throat Chop on the bouncer stops a bounced sound move: Showdown's cant)
                    let chopped = self.mon(ts, ti).throat_chop > 0 && mv.flags & flag::SOUND != 0;
                    if mv.category == Category::Status && self.mon(s, i).alive() && !powder_immune && !bounce_blocked && !chopped {
                        // a bounced spread move (String Shot...) hits the bouncer's foes = the original user and its partner (2026-10-16
                        // Showdown log: '[spread] p2a,p2b'; the bouncer's own partner only takes the ORIGINAL move's hit)
                        let mut tl: ArrayVec<(usize, usize), 3> = ArrayVec::new();
                        tl.push((s, i));
                        if mv.target == Target::AllAdjacentFoes {
                            for (sd, mi) in [(s, i)] {
                                if let Some(k) = self.slot_of(sd, mi) {
                                    if let Some(a) = self.active(sd, 1 - k) {
                                        if self.mon(sd, a).alive() {
                                            tl.push((sd, a));
                                        }
                                    }
                                }
                            }
                        }
                        for (xs, xi) in tl {
                            if !self.mon(xs, xi).alive() || powder_imm(self, xs, xi) || !self.accuracy_check(dex, ts, ti, mv, xs, xi) {
                                continue;
                            }
                            // the bounced move meets its new target's TryHit abilities (Showdown: a full useMove from the bouncer;
                            // Eerie Impulse bounced into the user's own Lightning Rod = +1 SpA; 2026-10-17 coverage fuzz)
                            let xab = self.mon(xs, xi).ability;
                            let t = self.cur_type;
                            let soaked = match xab {
                                Ab::Voltabsorb if t == dex.ty.electric => { let a = self.mon(xs, xi).maxhp / 4; self.heal(xs, xi, a, "voltabsorb"); true }
                                Ab::Waterabsorb | Ab::Dryskin if t == dex.ty.water => { let a = self.mon(xs, xi).maxhp / 4; self.heal(xs, xi, a, "waterabsorb"); true }
                                Ab::Lightningrod if t == dex.ty.electric => { self.boost(dex, xs, xi, &[0, 0, 0, 1, 0, 0, 0, 0], None, false); true }
                                Ab::Motordrive if t == dex.ty.electric => { self.boost(dex, xs, xi, &[0, 0, 0, 0, 0, 1, 0, 0], None, false); true }
                                Ab::Stormdrain if t == dex.ty.water => { self.boost(dex, xs, xi, &[0, 0, 0, 1, 0, 0, 0, 0], None, false); true }
                                Ab::Sapsipper if t == dex.ty.grass => { self.boost(dex, xs, xi, &[0, 1, 0, 0, 0, 0, 0, 0], None, false); true }
                                Ab::Goodasgold => true,
                                Ab::Soundproof if mv.flags & flag::SOUND != 0 => true,
                                Ab::Overcoat if mv.flags & flag::POWDER != 0 => true,
                                _ => false,
                            };
                            if soaked {
                                continue;
                            }
                            self.cancel_self_switch = false;
                            self.status_effects(dex, ts, ti, mv, xs, xi);
                            let cancelled = std::mem::take(&mut self.cancel_self_switch);
                            if mv.self_switch && !cancelled && mv.kind != Mv::Revivalblessing && self.mon(ts, ti).alive() {
                                if let Some(slot) = self.slot_of(ts, ti) {
                                    self.request_switch(ts, slot);
                                }
                            }
                        }
                    }
                    true
                }
                _ => false,
            };
            if absorbed {
                return (false, 0);
            }
            if mv.flags & flag::POWDER != 0 && self.mon(ts, ti).has_type(dex.ty.grass) {
                return (false, 0);
            }
            // Prankster-boosted status moves vs Dark types (Showdown hitStepTryImmunity: after the TryHit step, so Magic
            // Bounce reflects them first)
            if foe_target && mv.category == Category::Status && self.mon(s, i).ability == Ab::Prankster
                && self.mon(ts, ti).has_type(dex.ty.dark) && self.move_priority(dex, s, i, mv) > 0
            {
                return (false, 0);
            }
        }
        // Uproar wakes every active sleeper on both sides - but only when it really gets to hit (a Soundproof or Ghost target: Showdown
        // leaves everybody asleep; checked against the real simulator)
        let uproar_immune = mv.kind == Mv::Uproar && {
            let tm = self.mon(ts, ti);
            tm.types.iter().chain(std::iter::once(&tm.added_type)).any(|&dt| dt != 255 && dex.type_code(mv.type_, dt) == 3)
        };
        if mv.kind == Mv::Uproar && !uproar_immune {     // (a Ghost target: -immune, and nobody wakes - checked in Showdown)
            for side in 0..2 {
                for k in 0..2 {
                    if let Some(x) = self.active(side, k) {
                        let m = self.mon_mut(side, x);
                        if m.status == Status::Sleep && m.alive() {
                            m.status = Status::None;
                            m.status_turns = 0;
                        }
                    }
                }
            }
        }
        // type immunity for status moves that respect it (Thunder Wave vs Ground...)
        if mv.category == Category::Status && !mv.ignore_immunity && !is_self {
            let tm = self.mon(ts, ti);
            if tm.types.iter().chain(std::iter::once(&tm.added_type)).any(|&dt| dt != 255 && dex.type_code(mv.type_, dt) == 3) {
                return (false, 0);
            }
        }
        if !is_self && matches!(mv.kind, Mv::Psychicfangs | Mv::Brickbreak | Mv::Ragingbull) {
            let tm = self.mon(ts, ti);
            // (the move's type after ModifyType: Raging Bull of a Paldean Tauros is Water / Fire / Fighting, not Normal)
            let mt = self.cur_type;
            // Scrappy / Mind's Eye: Fighting into a Ghost hits, so the screens break (2026-10-17 coverage fuzz: Sirfetch'd)
            let scrappy = matches!(self.mon(s, i).ability, Ab::Scrappy | Ab::MindsEye) && (mt == dex.ty.fighting || mt == dex.ty.normal);
            let immune = tm.types.iter().chain(std::iter::once(&tm.added_type))
                .any(|&dt| dt != 255 && dex.type_code(mt, dt) == 3 && !(scrappy && dt == dex.ty.ghost));
            if !immune {
                let sd = &mut self.sides[ts];
                sd.reflect = 0;
                sd.light_screen = 0;
                sd.aurora_veil = 0;
            }
        }
        // accuracy
        if !is_self && !self.skip_accuracy && !self.accuracy_check(dex, s, i, mv, ts, ti) {
            self.counters.misses += 1;
            self.add(|| format!("|-miss|{s}|{i}"));
            return (false, 0);
        }
        if mv.breaks_protect && !is_self {
            // Feint and friends: Protect (and the side's guards) stop working for the rest of the turn
            let immune = mv.category != Category::Status
                && self.mon(ts, ti).types.iter().chain(std::iter::once(&self.mon(ts, ti).added_type))
                    .any(|&dt| dt != 255 && dex.type_code(self.cur_type, dt) == 3);
            if !immune {
                // something broken (its Protect, or the side's Wide / Quick Guard): Showdown hitStepBreakProtect also deletes the
                // target's stall counter (gen 6+), so its next Protect can't fail (2026-10-17 targeted fuzz: Phantom Force;
                // an earlier fix kept the counter here by analogy with Future Sight, wrongly)
                let broke = self.mon(ts, ti).has_vol(vol::PROTECT) || self.sides[ts].wide_guard || self.sides[ts].quick_guard;
                self.mon_mut(ts, ti).vol &= !vol::PROTECT;
                self.sides[ts].wide_guard = false;
                self.sides[ts].quick_guard = false;
                if broke {
                    let m = self.mon_mut(ts, ti);
                    m.protect_streak = 0;
                    m.guarded = false;
                }
            }
        }
        let att_infiltrates = self.mon(s, i).ability == Ab::Infiltrator;
        let behind_sub = !is_self && self.mon(ts, ti).sub_hp > 0 && mv.flags & (flag::SOUND | flag::BYPASSSUB) == 0 && !att_infiltrates;
        if mv.category == Category::Status {
            // Substitute blocks status moves from ALLIES too (Showdown substitute onTryPrimaryHit: only the user itself,
            // bypasssub and infiltrates get through)
            if behind_sub {
                return (false, 0);
            }
            self.status_effects(dex, s, i, mv, ts, ti);
            return (true, 0);
        }
        // ---- damage, hit by hit
        let hits = self.hit_count(dex, s, i, mv);
        let mut dealt_total: u16 = 0;
        let mut connected = false;
        let mut hit_sub = false;
        let mut hits_landed: u8 = 0;
        let mut scrappy0: Option<Ab> = None;
        // single-hit moves: secondaries and reactions run after every target took its damage
        let defer = self.defer_hits && hits == 1;
        for h in 1..=hits {
            if !self.mon(ts, ti).alive() || (!self.mon(s, i).alive() && !(spread && h == 1) && !self.future_now) {
                break;
            }
            // a user put to sleep mid-move (Effect Spore) stops hitting (Showdown hitStepMoveHitLoop; 2026-10-17 targeted fuzz)
            if h > 1 && self.mon(s, i).status == Status::Sleep && !mv.sleep_usable {
                break;
            }
            if h > 1 && mv.multi_accuracy && !self.accuracy_check(dex, s, i, mv, ts, ti) {
                break;
            }
            hits_landed += 1;
            // (2026-10-18 targeted fuzz 3984129) the user fainted earlier in this spread move: damage as computed up front
            // (Charged Bellibolt KO'd by its partner's Liquid Ooze still hits the foes with Charge)
            let snap_swap = spread && h == 1 && !self.mon(s, i).alive() && self.spread_user_snap.is_some();
            if snap_swap {
                let snap = self.spread_user_snap.take().unwrap();
                let cur = std::mem::replace(&mut self.sides[s].mons[i], *snap);
                self.spread_user_snap = Some(Box::new(cur));
            }
            let mut att = self.combatant(dex, s, i);
            if mv.kind == Mv::Fling && self.fling_bp > 0 {
                att.item = self.fling_kind;
            }
            // Scrappy / Mind's Eye set the move's ignoreImmunity once (ModifyMove): losing the ability mid-move (Mummy) keeps it
            if h == 1 {
                scrappy0 = matches!(att.ability, Ab::Scrappy | Ab::MindsEye).then_some(att.ability);
            } else if let Some(a0) = scrappy0 {
                att.ability = a0;
            }
            let mut def = self.combatant(dex, ts, ti);
            {
                let aab = self.mon(s, i).ability;
                let dab = self.mon(ts, ti).ability;
                if matches!(aab, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze)
                    && !matches!(dab, Ab::ShadowShield | Ab::PrismArmor | Ab::Fullmetalbody | Ab::Multitype | Ab::Rkssystem | Ab::Comatose)
                {
                    def.ability = Ab::None;
                }
            }
            // a resist berry doesn't weaken a hit its Substitute takes (Showdown: hitSub -> return)
            if behind_sub && self.mon(ts, ti).sub_hp > 0 && matches!(def.item, It::ResistBerry(_)) {
                def.item = It::None;
            }
            let mut cond = self.conditions(dex, s, i, ts, ti, spread);
            cond.gem = self.gem_active;
            cond.protect_bypass = self.protect_bypass;
            cond.minimized = self.mon(ts, ti).has_vol(vol::MINIMIZE);
            cond.analytic = self.analytic_move.unwrap_or_else(|| !self.queue.iter().any(|q| matches!(q.kind, QKind::Move { .. })));
            cond.fling_bp = self.fling_bp;
            cond.acc_eva_boosts = self.mon(s, i).boosts[6..8].iter().filter(|&&b| b > 0).map(|&b| b as u8).sum();
            cond.round_boost = self.round_boost_now;
            cond.bp_override = self.state_bp(dex, s, i, mv, ts, ti);
            cond.species_move_type = self.species_move_type(dex, s, i, mv);
            cond.hangry = dex.species[self.mon(s, i).species as usize].id == "morpekohangry";
            cond.parental_second = self.parental && h == 2;
            if mv.kind == crate::effects::Mv::BeatUp {
                let allies = self.beat_up_allies(s, i);
                if let Some(&k) = allies.get(h as usize - 1) {
                    // Showdown: the TEAM SET's species (set.species), so a Mega Evolved ally counts with its base form
                    let sp = self.sides[s].mons[k].base_species;
                    cond.beat_up_bp = 5 + dex.species[sp as usize].base[1] / 10;
                }
            }
            let poisoned = matches!(self.mon(ts, ti).status, Status::Poison | Status::Toxic);
            // fixed-damage moves (OHKO, damageCallback, level, set amounts) return before the crit roll in Showdown getDamage: no crit, no Anger Point
            let crit = mv.fixed == crate::dex::FixedDamage::No && !crit_blocked(&def) && self.crit_roll(dex, s, i, mv, poisoned);
            if snap_swap {
                let cur = self.spread_user_snap.take().unwrap();
                let snap = std::mem::replace(&mut self.sides[s].mons[i], *cur);
                self.spread_user_snap = Some(Box::new(snap));
            }
            if mv.kind == Mv::Ficklebeam {
                cond.fickle = self.chance_of(3, 10);
            }
            let roll = if self.chance.is_some() {
                self.chance_roll(dex, mv, &att, &def, &cond, crit, h, ts, ti, behind_sub)
            } else {
                self.rng.below(16) as usize
            };
            let Some(hit) = crate::damage::damage_one(dex, mv, &att, &def, &cond, crit, h, roll) else {
                return (false, 0);
            };
            if crit {
                self.counters.crits += 1;
            }
            let dmg = hit.rolls[roll];
            // Substitute takes the hit: no contact effects, no secondaries on the target
            if behind_sub && self.mon(ts, ti).sub_hp > 0 {
                let m = self.mon_mut(ts, ti);
                let dealt = dmg.min(m.sub_hp);
                m.sub_hp -= dealt;
                hit_sub = true;
                connected = true;
                dealt_total = dealt_total.saturating_add(dealt);
                // drain from a Substitute hit (Showdown substitute onTryPrimaryHit: ceil, Liquid Ooze still applies)
                if let Some((num, den)) = mv.drain {
                    if dealt > 0 && self.mon(s, i).alive() {
                        let amt = ((dealt as u32 * num as u32 + den as u32 - 1) / den as u32) as u16;
                        if amt > 0 {
                            if self.mon(ts, ti).ability == Ab::Liquidooze {
                                { let oz = self.ooze_amt(s, i, amt); self.hurt(s, i, oz, "liquidooze"); }
                            } else {
                                self.heal(s, i, amt, "drain");
                            }
                        }
                    }
                }
                // (2026-10-18 bridge fuzz) Air Balloon onAfterSubDamage: a move hitting the Substitute pops the holder's balloon
                if self.mon(ts, ti).item == It::AirBalloon && self.mon(ts, ti).has_item() && self.mon(ts, ti).alive() {
                    self.pop_balloon(ts, ti);
                }
                continue;
            }
            // this hit reached the Pokemon (a multi-hit move that broke the Substitute: later hits count, as in Showdown)
            hit_sub = false;
            // Disguise: the first damaging hit does nothing, then it loses 1/8 of its max HP
            let mold = matches!(dex.abilities[self.mon(s, i).ability_id as usize].id.as_str(), "moldbreaker" | "teravolt" | "turboblaze");
            if !mold && dex.species[self.mon(ts, ti).species as usize].id == "mimikyu"
                && self.mon(ts, ti).ability == Ab::Disguise
            {
                connected = true;
                if defer {
                    let hp = self.mon(ts, ti).hp;
                    let ab = self.mon(ts, ti).ability;
                    self.deferred_dh.push((ts, ti, 0, hp, false, ab));
                } else {
                    self.hit_crit = false;
                    self.ab_before_hit = self.mon(ts, ti).ability;
                    self.on_damaging_hit(dex, s, i, mv, ts, ti, 0);
                }
                self.forme_change(dex, ts, ti, "mimikyubusted");
                let amt = (self.mon(ts, ti).maxhp / 8).max(1);
                self.hurt(ts, ti, amt, "disguise");
                // no damage was dealt: Steel Beam / Mind Blown recoil needs totalDamage > 0 (applyRecoilDamage), and the move didn't fail
                self.recoil_pending = false;
                continue;
            }
            self.hp_before_hit = self.mon(ts, ti).hp;
            self.ab_before_hit = self.mon(ts, ti).ability;
            self.innards_prev = if mv.kind == Mv::Dragondarts { 0 } else { dealt_total };
            let db = foe_target && self.mon(ts, ti).has_vol(vol::DESTINY_BOND) && !self.future_now;
            let dealt = self.take_hit(dex, ts, ti, dmg);
            if dealt > 0 {
                self.mon_mut(ts, ti).hit_by |= 1 << (s * 8 + i);
                if ts != s {
                    let slot = self.slot_of(s, i).map(|k| k as u8).unwrap_or(NONE);
                    let k = if mv.category == Category::Physical { 1 } else { 2 };
                    let m = self.mon_mut(ts, ti);
                    m.retal[0] = dealt;
                    m.retal_slot[0] = slot;
                    m.retal[k] = dealt;
                    m.retal_slot[k] = slot;
                }
            }
            if db && !self.mon(ts, ti).alive() && self.mon(s, i).alive() {
                self.faint(s, i);
            }
            if let Some((num, den)) = mv.drain {
                if dealt > 0 && self.mon(s, i).alive() {
                    let amt = ((dealt as f64) * num as f64 / den as f64).round() as u16;
                    if amt > 0 {
                        if self.mon(ts, ti).ability == Ab::Liquidooze {
                            { let oz = self.ooze_amt(s, i, amt); self.hurt(s, i, oz, "liquidooze"); }
                        } else {
                            self.heal(s, i, amt, "drain");
                        }
                    }
                }
            }
            if let Some(d) = &mut self.diag {
                let e = d.damage.entry(mv.id.clone()).or_default();
                e.0 += 1;
                e.1 += dealt as u64;
            }
            connected = true;
            dealt_total = dealt_total.saturating_add(dealt);
            if self.gem_active && self.mon(s, i).has_item() && self.mon(s, i).item == It::NormalGem {
                self.consume_item(s, i);
            }
            if !self.mon(ts, ti).alive() {
                if defer {
                    self.deferred_kos += 1;
                } else {
                    self.ko_boost(dex, s, i);
                }
            }
            // Ripen's berryWeaken is spent by any hit that reaches ModifyDamage (or set and spent at once by a resist berry eaten now)
            if mv.fixed == crate::dex::FixedDamage::No {
                self.mon_mut(ts, ti).berry_weaken = false;
            }
            // resist berries are eaten
            // (fixed-damage moves never reach Showdown's modifyDamage, so resist berries don't trigger)
            if let It::ResistBerry(t) = self.mon(ts, ti).item {
                if mv.fixed == crate::dex::FixedDamage::No && !cond.berries_blocked && self.cur_type != crate::dex::NO_TYPE && dex.type_names[self.cur_type as usize] == t && (hit.type_mod > 0 || t == "Normal") {
                    self.consume_berry_in_hit(ts, ti);
                    if self.mon(ts, ti).ability == Ab::Cheekpouch {
                        let amt = self.mon(ts, ti).maxhp / 3;
                        self.heal(ts, ti, amt, "cheekpouch");
                    }
                }
            }
            if hits > 1 {
                self.hit_secondaries(dex, s, i, mv, ts, ti, hit_sub);
                self.sec_done = true;
            }
            // Double Shock / Burn Up: the user's type is lost in the move's self effect, before the DamagingHit abilities (Static...)
            if matches!(mv.kind, Mv::Doubleshock | Mv::Burnup) {
                let t = tid(dex, if mv.kind == Mv::Doubleshock { "Electric" } else { "Fire" });
                let m = self.mon_mut(s, i);
                m.added_type = crate::dex::NO_TYPE;
                for x in m.types.iter_mut() {
                    if *x == t {
                        *x = crate::dex::NO_TYPE;
                    }
                }
            }
            if defer {
                self.deferred_dh.push((ts, ti, hit.type_mod, self.hp_before_hit, crit, self.ab_before_hit));
            } else {
                self.hit_crit = crit;
                self.early_self_drops(dex, s, i, mv);
                self.on_damaging_hit(dex, s, i, mv, ts, ti, hit.type_mod);
            }
        }
        if !connected {
            return (false, 0);
        }
        self.last_hits = hits_landed;
        if !is_self && hits_landed > 0 {
            let m = self.mon_mut(ts, ti);
            m.times_attacked = m.times_attacked.saturating_add(hits_landed);
            if ts != s {
                m.obs_n = m.obs_n.saturating_add(1);
                let a = self.mon_mut(s, i);
                a.obs_n = a.obs_n.saturating_add(1);
            }
        }
        if hit_sub && self.dart_ee_hold && !self.dart_sub.is_full() {
            self.dart_sub.push((ts, ti));
        }
        if defer {
            self.deferred_post.push((ts, ti, hit_sub));
        } else {
            self.post_hit(dex, s, i, mv, ts, ti, hit_sub);
            self.after_hit(dex, s, i, mv, ts, ti, hit_sub);
        }
        (true, dealt_total)
    }

    /// A move's secondary effects on one target after one hit (Showdown runs them per hit for multi-hit moves),
    /// then King's Rock.
    fn hit_secondaries(&mut self, dex: &Dex, s: usize, i: usize, mv: &MoveData, ts: usize, ti: usize, hit_sub: bool) {
        // secondary effects
        let sheer = self.mon(s, i).ability == Ab::SheerForce;
        // Shield Dust is breakable (Mold Breaker & co. ignore it); Covert Cloak is an item
        let breaker = matches!(self.mon(s, i).ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze);
        let shield = hit_sub || self.mon(ts, ti).alive()
            && ((self.mon(ts, ti).ability == Ab::Shielddust && !breaker)
                || (self.mon(ts, ti).has_item() && self.mon(ts, ti).item == It::CovertCloak));
        if !sheer {
            let serene = self.mon(s, i).ability == Ab::Serenegrace;
            for sec in &mv.secondaries {
                let chance = if serene { (sec.chance as u32 * 2).min(100) } else { sec.chance as u32 };
                if chance < 100 && !self.pct(chance as i64) {
                    continue;
                }
                if let Some(b) = sec.self_boosts {
                    self.boost(dex, s, i, &b, Some((s, i)), false);
                }
                if !self.mon(ts, ti).alive() || shield {
                    continue;
                }
                if let Some(b) = sec.boosts {
                    self.boost(dex, ts, ti, &b, Some((s, i)), false);
                }
                if let Some(st) = sec.status {
                    self.set_status(dex, ts, ti, st, Some((s, i)));
                }
                if sec.custom && mv.kind == Mv::Spiritshackle && ts != s && self.slot_of(s, i).is_some()
                    && !self.mon(ts, ti).has_type(dex.ty.ghost) && !self.has_trapped_vol(ts, ti)
                {
                    self.mon_mut(ts, ti).trap_src = i as u8;
                    self.mon_mut(ts, ti).trap_side = s as u8;
                }
                if sec.custom && mv.kind == Mv::Throatchop {
                    if self.mon(ts, ti).throat_chop == 0 {
                        // Showdown: no onRestart, a second hit doesn't refresh the 2 turns
                        self.mon_mut(ts, ti).throat_chop = 2;
                    }
                }
                if sec.custom && mv.kind == Mv::Alluringvoice && self.mon(ts, ti).has_vol(vol::STATS_RAISED) && self.mon(ts, ti).confusion == 0 {
                    let tab = self.mon(ts, ti).ability;
                    let brk = self.breaks_vs(ts, ti, Some((s, i)));
                    if (tab != Ab::Owntempo || brk) && !self.misty_no_confusion(dex, ts, ti, Some((s, i))) {
                        let n = 0x80u8;   // (2026-10-28, hidden counters) unknown 2-5 length: decided per check (BeforeMove)
                        self.mon_mut(ts, ti).confusion = n; // Showdown time 2-5, decremented before each move: 1-4 confused turns
                    }
                }
                if sec.custom && mv.kind == Mv::Burningjealousy && self.mon(ts, ti).has_vol(vol::STATS_RAISED) {
                    self.set_status(dex, ts, ti, Status::Burn, Some((s, i)));
                }
                if sec.custom && mv.kind == Mv::Triattack {
                    let st = [Status::Burn, Status::Paralysis, Status::Freeze][self.rng.below(3) as usize];
                    self.set_status(dex, ts, ti, st, Some((s, i)));
                }
                if sec.custom && mv.kind == Mv::Eeriespell {
                    let t = self.mon(ts, ti);
                    let lm = t.last_move;
                    if let Some(k) = (0..t.n_moves as usize).find(|&k| t.moves[k] == lm) {
                        let mm = self.mon_mut(ts, ti);
                        mm.pp[k] = mm.pp[k].saturating_sub(3);
                    }
                }
                if sec.custom && mv.kind == Mv::Direclaw {
                    let st = [Status::Poison, Status::Paralysis, Status::Sleep][self.rng.below(3) as usize];
                    self.set_status(dex, ts, ti, st, Some((s, i)));
                }
                match sec.volatile.as_deref() {
                    Some("flinch") => {
                        // Inner Focus is breakable: Mold Breaker & co. flinch through it
                        let inner = self.mon(ts, ti).ability == Ab::Innerfocus && !matches!(self.mon(s, i).ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze);
                        // (also on a Pokemon that already moved: an Instruct afterwards makes it flinch)
                        if !inner {
                            self.mon_mut(ts, ti).vol |= vol::FLINCH;
                        }
                    }
                    Some("confusion") => {
                        let tab = self.mon(ts, ti).ability;
                        // Own Tempo is breakable (Mold Breaker confuses through it)
                        let brk = self.breaks_vs(ts, ti, Some((s, i)));
                        if self.mon(ts, ti).confusion == 0 && (tab != Ab::Owntempo || brk) && !self.safeguarded(ts, ti, s, i)
                            && !self.misty_no_confusion(dex, ts, ti, Some((s, i))) {
                            let n = 0x80u8;   // (2026-10-28, hidden counters) unknown 2-5 length: decided per check (BeforeMove)
                            self.mon_mut(ts, ti).confusion = n; // Showdown time 2-5, decremented before each move: 1-4 confused turns
                        }
                    }
                    Some("throatchop") => {
                        if self.mon(ts, ti).throat_chop == 0 {
                        // Showdown: no onRestart, a second hit doesn't refresh the 2 turns
                        self.mon_mut(ts, ti).throat_chop = 2;
                    }
                    }
                    Some("saltcure") => {
                        self.mon_mut(ts, ti).vol2 |= vol2::SALT_CURE;
                    }
                    Some("syrupbomb") => {
                        let t = self.mon_mut(ts, ti);
                        if t.syrup == 0 {
                            t.syrup = 4;
                            t.syrup_src = i as u8;
                            t.syrup_side = s as u8;
                        }
                    }
                    Some("healblock") => {
                        if self.mon(ts, ti).heal_block == 0 && !self.side_has(ts, |a| a == Ab::Aromaveil) {
                            self.mon_mut(ts, ti).heal_block = 2;
                        }
                    }

                    _ => {}
                }
            }
        }
        // King's Rock: 10% flinch on moves without a flinch chance of their own
        let kr = (self.mon(s, i).has_item() && self.mon(s, i).item == It::KingsRock) || self.mon(s, i).ability == Ab::Stench;
        // (Sheer Force removes the move's own secondaries first, so the King's Rock flinch is added even then)
        if kr && mv.category != Category::Status && !shield && self.mon(ts, ti).alive()
            && (sheer || !mv.secondaries.iter().any(|x| x.volatile.as_deref() == Some("flinch")))
        {
            let chance = if self.mon(s, i).ability == Ab::Serenegrace { 20 } else { 10 };
            if self.pct(chance) {
                let inner = self.mon(ts, ti).ability == Ab::Innerfocus && !matches!(self.mon(s, i).ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze);
                if !inner {
                    self.mon_mut(ts, ti).vol |= vol::FLINCH;
                }
            }
        }
    }

    /// After a target was hit (Showdown's order: move effects and secondaries, then — elsewhere —
    /// the target's reactions): bound, secondaries, Knock Off, Bug Bite, Dragon Tail...
    /// (2026-10-18 targeted fuzz, 4746191) Showdown spreadMoveHit applies the move's self stat changes (selfDrops, once per
    /// move: move.selfDropped) BEFORE the targets' DamagingHit reactions: Malamar's Contrary Superpower into a Wandering Spirit
    /// Tinkaton gives +1/+1, then the abilities swap. Parental Bond keeps the per-hit drops at the end of the move.
    fn early_self_drops(&mut self, dex: &Dex, s: usize, i: usize, mv: &MoveData) {
        // (2026-11-05, sim report) no self stat change once the hit decided the battle (Showdown: the battle has ended)
        if self.self_dropped || self.parental || !self.mon(s, i).alive() || self.side_out() {
            return;
        }
        if let Some(b) = mv.self_boosts {
            self.self_dropped = true;
            self.boost(dex, s, i, &b, Some((s, i)), false);
        }
    }

    fn post_hit(&mut self, dex: &Dex, s: usize, i: usize, mv: &MoveData, ts: usize, ti: usize, hit_sub: bool) {
        let foe_target = ts != s;
        {
            // Bug Bite / Pluck: the user eats the target's berry (also when the hit KOs it: Showdown's onHit still runs)
            if matches!(mv.kind, Mv::Bugbite | Mv::Pluck) && !hit_sub && self.mon(ts, ti).has_item() && self.mon(s, i).alive() {
                let id = self.mon(ts, ti).item_id;
                // (takeItem: Sticky Hold keeps it unless the hit KOed the holder or Mold Breaker ignores the ability)
                let sticky = dex.abilities[self.mon(ts, ti).ability_id as usize].id == "stickyhold" && self.mon(ts, ti).alive()
                    && !matches!(self.mon(s, i).ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze);
                if dex.items[id as usize].is_berry && !sticky {
                    self.remove_item(ts, ti);
                    // the user eats it: its effect, Cheek Pouch, Belch
                    let m = self.mon_mut(s, i);
                    m.ate_berry = true;
                    m.last_item = id;
                    self.berry_effect(s, i, id);
                    if self.mon(s, i).ability == Ab::Cheekpouch {
                        let amt = self.mon(s, i).maxhp / 3;
                        self.heal(s, i, amt, "cheekpouch");
                    }
                }
            }
        }
        if mv.kind == Mv::Clearsmog && !hit_sub && self.mon(ts, ti).alive() {
            self.mon_mut(ts, ti).boosts = [0; 8];
        }
        // Fling: the flung berry is eaten by the target
        if mv.kind == Mv::Fling && !hit_sub && self.mon(ts, ti).alive() && self.fling_item != u16::MAX {
            // orbs / Light Ball / Poison Barb / King's Rock are pushed as SECONDARIES: Shield Dust / Covert Cloak block them
            let dust = (self.mon(ts, ti).ability == Ab::Shielddust && !matches!(self.mon(s, i).ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze))
                || (self.mon(ts, ti).has_item() && self.mon(ts, ti).item == It::CovertCloak);
            let fid = dex.items[self.fling_item as usize].id.as_str();
            let fid = if dust && matches!(fid, "flameorb" | "toxicorb" | "lightball" | "poisonbarb" | "kingsrock" | "razorfang") { "" } else { fid };
            match fid {
                _ if dex.items[self.fling_item as usize].is_berry => {
                    // the target eats the flung berry (its effect, Cheek Pouch, Belch)
                    let it = self.fling_item;
                    // Klutz / Magic Room: the berry's own Eat handler is skipped (Showdown singleEvent ignoringItem); Cheek Pouch still heals
                    if !(self.mon(ts, ti).ability == Ab::Klutz || self.field.magic_room > 0) {
                        self.berry_effect(ts, ti, it);
                    }
                    self.mon_mut(ts, ti).ate_berry = true;
                    if self.mon(ts, ti).ability == Ab::Cheekpouch {
                        let amt = self.mon(ts, ti).maxhp / 3;
                        self.heal(ts, ti, amt, "cheekpouch");
                    }
                }
                "whiteherb" => {
                    let m = self.mon_mut(ts, ti);
                    for b in m.boosts.iter_mut() {
                        if *b < 0 {
                            *b = 0;
                        }
                    }
                }
                "mentalherb" => {
                    let m = self.mon_mut(ts, ti);
                    m.vol2 &= !(vol2::ATTRACT | vol2::TORMENT);
                    m.taunt = 0;
                    m.encore = 0;
                    m.encore_slot = NONE;
                    m.disable = 0;
                    m.disable_slot = NONE;
                    m.heal_block = 0;
                }
                "flameorb" => {
                    self.set_status(dex, ts, ti, Status::Burn, Some((s, i)));
                }
                "toxicorb" => {
                    self.set_status(dex, ts, ti, Status::Toxic, Some((s, i)));
                }
                "lightball" => {
                    self.set_status(dex, ts, ti, Status::Paralysis, Some((s, i)));
                }
                "poisonbarb" => {
                    self.set_status(dex, ts, ti, Status::Poison, Some((s, i)));
                }
                "kingsrock" | "razorfang" => {
                    if !(self.mon(ts, ti).ability == Ab::Innerfocus && !matches!(self.mon(s, i).ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze)) {
                        self.mon_mut(ts, ti).vol |= vol::FLINCH;
                    }
                }
                _ => {}
            }
        }
        // Smack Down / Thousand Arrows: the target is grounded from now on
        if mv.volatile.as_deref() == Some("smackdown") && !hit_sub && self.mon(ts, ti).alive() {
            self.mon_mut(ts, ti).vol2 |= vol2::SMACK_DOWN;
            // knocked out of Fly / Bounce: the charge ends and its queued move is cancelled (Showdown smackdown onStart)
            if self.charging_move(ts, ti).is_some_and(|k| matches!(dex.moves[k].id.as_str(), "fly" | "bounce")) {
                self.mon_mut(ts, ti).charge_slot = NONE;
                self.queue.retain(|q| !(q.side as usize == ts && q.mon as usize == ti && matches!(q.kind, QKind::Move { .. })));
            }
        }
        // the move's own volatile on the target (Infestation, Whirlpool...: bound)
        if mv.volatile.as_deref() == Some("partiallytrapped") && !hit_sub && self.mon(ts, ti).alive() && self.mon(ts, ti).bound == 0 {
            // Showdown: 5 or 6 turns (Grip Claw: 8); Binding Band: 1/6 a turn instead of 1/8
            let src_item = if self.mon(s, i).has_item() { dex.items[self.mon(s, i).item_id as usize].id.as_str() } else { "" };
            // (2026-10-28, hidden counters) 5 or 6 turns, unknown: 0x80 | end-of-turns elapsed, decided per end of turn
            let n = if src_item == "gripclaw" { 8 } else { 0x80 };
            let div = if src_item == "bindingband" { 6 } else { 8 };
            let t = self.mon_mut(ts, ti);
            t.bound = n;
            t.bound_div = div;
            t.bound_src = i as u8;
            t.bound_side = s as u8;
        }
        // secondary effects (multi-hit moves ran them after each hit)
        if std::mem::take(&mut self.sec_done) {
            return;
        }
        self.hit_secondaries(dex, s, i, mv, ts, ti, hit_sub);
    }

    /// The move's own effects after a damaging hit (Showdown's AfterHit, after the target's
    /// reactions): Knock Off, Bug Bite, Dragon Tail...
    fn after_hit(&mut self, dex: &Dex, s: usize, i: usize, mv: &MoveData, ts: usize, ti: usize, hit_sub: bool) {
        let foe_target = ts != s;
        // Steel Roller / Ice Spinner clear the terrain at AfterHit, AFTER the target's DamagingHit abilities (Showdown gen 9:
        // a Poison Point / Flame Body status is still blocked by the Misty Terrain the move is about to remove)
        if matches!(mv.kind, Mv::Steelroller | Mv::Icespinner) && self.mon(s, i).alive() && self.field.terrain != Terrain::None {
            self.field.terrain = Terrain::None;
            self.field.terrain_turns = 0;
            self.terrain_changed(dex);
        }
        // the move's own effects on a damaging hit (not when a Substitute took it: Showdown nulls the target, no AfterHit)
        // Sticky Hold keeps the item from Knock Off (breakable: Mold Breaker knocks it off)
        let sticky = dex.abilities[self.mon(ts, ti).ability_id as usize].id == "stickyhold"
            && !matches!(self.mon(s, i).ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze);
        if self.mon(ts, ti).alive() && !hit_sub {
            if sticky && mv.kind == Mv::KnockOff {
                // (the item stays; Sitrus & co. still get their check below)
            } else if mv.kind == Mv::KnockOff && self.mon(ts, ti).has_item() && !(self.mon(ts, ti).mega_species != u16::MAX || self.mon(ts, ti).is_mega || self.stone_locked(dex, ts, ti)) {
                self.remove_item(ts, ti);
            } else if mv.kind == Mv::KnockOff && self.mon(ts, ti).stash_item.0 != u16::MAX && !self.mon(ts, ti).is_mega {
                // Magic Room / Klutz: takeItem doesn't check ignoringItem, the put-aside item is knocked off too
                self.mon_mut(ts, ti).stash_item = (u16::MAX, It::None);
                self.remove_item(ts, ti);
            }
            if mv.kind == crate::effects::Mv::KnockOff {
                self.after_damage_items(dex, ts, ti);
            }
        }
        // Stone Axe / Ceaseless Edge: hazards even if the user fainted to Rough Skin & co. (Showdown: no hp check)
        // (always the USER's foe side, also when the move hit the user's partner: source.side.foeSidesWithConditions(); 2026-10-17)
        if (ts, ti) != (s, i) && self.mon(s, i).ability != Ab::SheerForce {
            match mv.kind {
                Mv::Stoneaxe => self.sides[1 - s].stealth_rock = true,
                Mv::Ceaselessedge => {
                    let sd = &mut self.sides[1 - s];
                    sd.spikes = (sd.spikes + 1).min(3);
                }
                _ => {}
            }
        }
        if self.mon(s, i).alive() && foe_target {
            let sheer = self.mon(s, i).ability == Ab::SheerForce;
            match mv.kind {
                Mv::Mortalspin | Mv::Rapidspin if !sheer => {
                    let m = self.mon_mut(s, i);
                    m.vol &= !vol::LEECH_SEED;
                    m.bound = 0;
                    let sd = &mut self.sides[s];
                    sd.stealth_rock = false;
                    sd.spikes = 0;
                    sd.toxic_spikes = 0;
                    sd.sticky_web = false;
                }
                _ => {}
            }
        }
        if matches!(mv.kind, Mv::Thief | Mv::Covet) && !hit_sub && (ts, ti) != (s, i) && self.mon(s, i).alive() && !self.mon(s, i).has_item() && self.item_takeable(dex, ts, ti) {
            self.steal_item(ts, ti, s, i);
            self.seed_check(dex, s, i);       // a terrain seed that arrives by theft works at once (Showdown setItem -> Start)
        }
        // Jaw Lock: both Pokemon are trapped by each other (Ghosts are immune to 'trapped')
        if mv.kind == Mv::Jawlock && !hit_sub && foe_target && self.mon(s, i).alive() && self.mon(ts, ti).alive() {
            if !self.mon(ts, ti).has_type(dex.ty.ghost) && !self.has_trapped_vol(ts, ti) {
                self.mon_mut(ts, ti).trap_src = i as u8;
                self.mon_mut(ts, ti).trap_side = s as u8;
            }
            if !self.mon(s, i).has_type(dex.ty.ghost) && !self.has_trapped_vol(s, i) {
                self.mon_mut(s, i).trap_src = ti as u8;
                self.mon_mut(s, i).trap_side = ts as u8;
            }
        }
        // (2026-10-16) Pickpocket, Red Card and a damaging move's drag-out (Dragon Tail) also work between allies (Showdown:
        // source !== target)
        let not_self = !(ts == s && ti == i);
        // (Red Card and Pickpocket run in the same AfterMoveSecondary event, whose handlers are collected first: a Red Card
        // just stolen by Pickpocket doesn't fire; Showdown 2026-10-17)
        let card_before = self.mon(ts, ti).item == It::RedCard;
        if not_self && !hit_sub && mv.flags & flag::CONTACT != 0 && self.mon(s, i).ability != Ab::Longreach && self.mon(ts, ti).alive() && self.mon(ts, ti).ability == Ab::Pickpocket
            && !self.mon(ts, ti).has_item() && self.item_takeable(dex, s, i)
        {
            self.steal_item(s, i, ts, ti);
            self.seed_check(dex, ts, ti);
        }
        // (a holder this move drags out has its forceSwitchFlag set: Showdown's Red Card then does nothing)
        let dragging = mv.force_switch && not_self && !self.drag_blocked(s, i, ts, ti) && self.sides[ts].bench().next().is_some();
        if not_self && !hit_sub && !dragging && self.mon(ts, ti).alive() && card_before && self.mon(ts, ti).item == It::RedCard && self.red_card.is_none() {   // (a Substitute took the hit: no Red Card)
            if self.card_defer {
                self.card_cands.push((ts, ti));
            } else {
                self.red_card = Some((ts as u8, ti as u8));
            }
        }
        if self.mon(ts, ti).alive() && !hit_sub {   // (a Substitute took the hit: Dragon Tail / Circle Throw drag nothing)
            if mv.force_switch && not_self && !self.drag_blocked(s, i, ts, ti) {
                let bench: ArrayVec<usize, 6> = self.sides[ts].bench().collect();
                if !bench.is_empty() {
                    if let Some(slot) = self.slot_of(ts, ti) {
                        let to = bench[self.rng.below(bench.len() as u32) as usize];
                        self.do_switch(dex, ts, slot, to);
                    }
                }
            }
        }
    }

    /// Guard Dog can't be forced out (Roar, Dragon Tail...) - unless the attacker's Mold Breaker ignores it
    fn drag_blocked(&self, s: usize, i: usize, ts: usize, ti: usize) -> bool {
        (matches!(self.mon(ts, ti).ability, Ab::Guarddog | Ab::Suctioncups) && !matches!(self.mon(s, i).ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze))
            || self.mon(ts, ti).vol2 & vol2::INGRAIN != 0
    }

    /// Beat Up's hitters: the user and every party member that isn't fainted or statused
    /// (Showdown's side.pokemon order, `Side::party`)
    fn beat_up_allies(&self, s: usize, i: usize) -> ArrayVec<usize, 6> {
        let side = &self.sides[s];
        // (2026-10-17 targeted fuzz) the real array order: switches reshuffle the bench (Sneasler out for Tinkaton puts
        // Sneasler at Tinkaton's old position)
        let order: ArrayVec<usize, 6> = side.party_order().iter().map(|&k| k as usize).filter(|&k| k < side.mons.len()).collect();
        order.into_iter().filter(|&k| k == i || (side.mons[k].alive() && side.mons[k].status == Status::None)).collect()
    }

    fn hit_count(&mut self, dex: &Dex, s: usize, i: usize, mv: &MoveData) -> u8 {
        if let Some(n) = self.forced_hits {
            return n;
        }
        if mv.kind == crate::effects::Mv::BeatUp {
            return self.beat_up_allies(s, i).len().max(1) as u8;
        }
        if self.parental {
            return 2;
        }
        let (lo, hi) = mv.multihit;
        if hi <= 1 {
            return 1;
        }
        if lo == hi {
            return lo;
        }
        let ab = self.mon(s, i).ability;
        if ab == Ab::Skilllink {
            return hi;
        }
        let m = self.mon(s, i);
        if m.has_item() && m.item == It::LoadedDice {
            return 4 + self.rng.below(2) as u8;
        }
        // 2-5 hits: 35% / 35% / 15% / 15%
        if self.chance.is_some() {
            // 2-5 hits (7/20, 7/20, 3/20, 3/20): two enumerated binary events
            if let Some(high) = self.branch(0.3) {
                let up = self.chance_of(1, 2);
                return if high { 4 + up as u8 } else { 2 + up as u8 };
            }
        }
        [2u8, 2, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 4, 4, 4, 5, 5, 5][self.rng.below(20) as usize]
    }

    /// chance enumeration's damage roll: if some rolls KO the target and some don't, branch on "KO" (probability = share
    /// of KO rolls) and use the middle roll of that class; otherwise the middle roll (no noise from rolls that change
    /// nothing decisive)
    #[allow(clippy::too_many_arguments)]
    fn chance_roll(&mut self, dex: &Dex, mv: &MoveData, att: &Combatant, def: &Combatant, cond: &Conditions, crit: bool,
                   h: u8, ts: usize, ti: usize, behind_sub: bool) -> usize {
        let Some(hit) = damage_full(dex, mv, att, def, cond, crit, h) else { return 8 };
        let m = self.mon(ts, ti);
        let survives_full = m.hp == m.maxhp && ((m.has_item() && m.item == It::FocusSash) || m.ability == Ab::Sturdy);
        if (behind_sub && m.sub_hp > 0) || survives_full {
            return 8;
        }
        let hp = m.hp;
        let mut ko: ArrayVec<usize, 16> = ArrayVec::new();
        let mut live: ArrayVec<usize, 16> = ArrayVec::new();
        for (r, &d) in hit.rolls.iter().enumerate() {
            if d >= hp { ko.push(r) } else { live.push(r) }
        }
        if ko.is_empty() || live.is_empty() {
            return 8;
        }
        match self.branch(ko.len() as f64 / 16.0) {
            Some(true) => ko[ko.len() / 2],
            Some(false) => live[live.len() / 2],
            None => self.rng.below(16) as usize,
        }
    }

    fn crit_roll(&mut self, dex: &Dex, s: usize, i: usize, mv: &MoveData, target_poisoned: bool) -> bool {
        let m = self.mon(s, i);
        if m.ability == Ab::Merciless && target_poisoned {
            return true;                                    // onModifyCritRatio: 5
        }
        let mut ratio = mv.crit_ratio as i32;
        if m.ability == Ab::Superluck {
            ratio += 1;
        }
        if m.has_item() && matches!(dex.items[m.item_id as usize].id.as_str(), "scopelens" | "razorclaw") {
            ratio += 1;
        }
        if m.has_item() && m.item == It::Leek
            && matches!(dex.species[m.species as usize].base_species.as_str(), "Farfetch’d" | "Sirfetch’d" | "farfetchd" | "sirfetchd")
        {
            ratio += 2;
        }
        if m.has_vol(vol::FOCUS_ENERGY) {
            ratio += 2;
        }
        if m.has_vol(vol::DRAGON_CHEER) {
            ratio += if m.has_vol(vol::DRAGON_CHEER2) { 2 } else { 1 };
        }
        if mv.will_crit {
            return true;
        }
        let ratio = ratio.clamp(0, 4);
        match ratio {
            0 => false,
            1 => self.chance_of(1, 24),
            2 => self.chance_of(1, 8),
            3 => self.chance_of(1, 2),
            _ => true,
        }
    }

    fn accuracy_check(&mut self, dex: &Dex, s: usize, i: usize, mv: &MoveData, ts: usize, ti: usize) -> bool {
        let att_ab = self.mon(s, i).ability;
        let def_ab = self.mon(ts, ti).ability;
        // OHKO immunity (lower level, Sheer Cold into an Ice type) comes before every always-hit rule: Glaive Rush, No Guard,
        // Lock-On (Showdown hitStepAccuracy: '-immune [ohko]' first; 2026-10-17 targeted fuzz: Glaive Rush Baxcalibur)
        if mv.fixed == crate::dex::FixedDamage::Ohko
            && (self.mon(s, i).level < self.mon(ts, ti).level || (mv.id == "sheercold" && self.mon(ts, ti).has_type(dex.ty.ice)))
        {
            return false;
        }
        if mv.always_hits || att_ab == Ab::Noguard || def_ab == Ab::Noguard || self.mon(ts, ti).has_vol(vol::GLAIVE_RUSH)
            || (mv.id == "toxic" && self.mon(s, i).has_type(dex.ty.poison))
            || (mv.flags & flag::MINIMIZE != 0 && self.mon(ts, ti).has_vol(vol::MINIMIZE))
        {
            return true;
        }
        // (a spread move's accuracy is decided for every target before any damage: weather as the move began)
        if self.mon(s, i).lock_on > 0 && ts != s && self.mon(s, i).lock_on_tgt == ti as u8 {
            return true;
        }
        let w = self.acc_weather.unwrap_or_else(|| self.weather());
        let mut acc = mv.accuracy as f64;
        match mv.kind {
            Mv::Hurricane | Mv::Thunder | Mv::Bleakwindstorm | Mv::Wildboltstorm | Mv::Sandsearstorm => {
                if w == Weather::Rain {
                    return true;
                }
                if w == Weather::Sun && mv.kind != Mv::Sandsearstorm {
                    acc = 50.0;
                }
            }
            Mv::Blizzard if w == Weather::Snow => return true,
            _ => {}
        }
        if mv.fixed == crate::dex::FixedDamage::Ohko {
            let (ul, tl) = (self.mon(s, i).level as i64, self.mon(ts, ti).level as i64);
            // Sheer Cold (ohko 'Ice'): 20% for a non-Ice user, and Ice-type targets are immune
            let sheer = mv.id == "sheercold";
            if ul < tl || (sheer && self.mon(ts, ti).has_type(dex.ty.ice)) {
                return false;
            }
            let base = if sheer && !self.mon(s, i).has_type(dex.ty.ice) { 20 } else { 30 };
            return self.pct(base + (ul - tl));
        }
        if mv.accuracy == 0 {
            return true;
        }
        let mut m4096 = 4096i64;
        if self.field.gravity > 0 {
            m4096 = (m4096 * 6840 + 2048) >> 12;
        }
        let item = self.mon(s, i).item_id;
        if self.mon(s, i).item == It::WideLens {
            m4096 = (m4096 * 4505 + 2048) >> 12;
        }
        if self.mon(s, i).item == It::ZoomLens && !self.queue.iter().any(|q| q.side as usize == ts && q.mon as usize == ti && matches!(q.kind, QKind::Move { .. })) {
            m4096 = (m4096 * 4915 + 2048) >> 12;
        }
        if att_ab == Ab::Compoundeyes {
            m4096 = (m4096 * 5325 + 2048) >> 12;
        }
        if att_ab == Ab::Hustle && mv.category == Category::Physical {
            m4096 = (m4096 * 3277 + 2048) >> 12;
        }
        let tm = self.mon(ts, ti);
        let mold = matches!(att_ab, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze);
        if !mold {
            match def_ab {
                Ab::Snowcloak if w == Weather::Snow => m4096 = (m4096 * 3277 + 2048) >> 12,
                Ab::Sandveil if w == Weather::Sand => m4096 = (m4096 * 3277 + 2048) >> 12,
                Ab::Tangledfeet if tm.confusion > 0 => m4096 = (m4096 * 2048 + 2048) >> 12,
                _ => {}
            }
        }
        if tm.has_item() && tm.item == It::BrightPowder {
            m4096 = (m4096 * 3686 + 2048) >> 12;
        }
        // (2026-10-16) Showdown's order: ModifyAccuracy (all the modifiers above) first, then the stages, truncated.
        // Unaware: an Unaware attacker ignores the target's evasion, an Unaware target the attacker's accuracy (breakable)
        let mut acc = crate::damage::modify(acc.floor() as i64, m4096);
        let eva = if mv.ignore_evasion || matches!(att_ab, Ab::Keeneye | Ab::Illuminate | Ab::MindsEye | Ab::Unaware) { 0 } else { self.mon(ts, ti).boosts[EVA] as i64 };
        let own = if def_ab == Ab::Unaware && !mold { 0 } else { self.mon(s, i).boosts[ACC] as i64 };
        let stage = (own - eva).clamp(-6, 6);
        if stage > 0 {
            acc = acc * (3 + stage) / 3;
        } else if stage < 0 {
            acc = acc * 3 / (3 - stage);
        }
        self.pct(acc)
    }

    /// Apply a hit's damage with Focus Sash / Sturdy. Returns the HP actually lost.
    fn take_hit(&mut self, dex: &Dex, ts: usize, ti: usize, dmg: u16) -> u16 {
        let m = self.mon(ts, ti);
        let mut dmg = dmg.min(m.hp);
        if dmg == m.hp && m.has_vol(vol::ENDURE) {
            dmg = m.hp - 1;
        } else if dmg == m.hp && m.hp == m.maxhp {
            let sash = m.has_item() && m.item == It::FocusSash;
            // (2026-10-17 targeted fuzz) Sturdy is breakable: Mold Breaker / Teravolt / Turboblaze hits go through (a landing
            // Future Sight has no ModifyMove, so it doesn't)
            let (cs, ci) = (self.cur_user.0 as usize, self.cur_user.1 as usize);
            let mold = !self.future_now && (cs, ci) != (ts, ti)
                && matches!(self.mon(cs, ci).ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze);
            let m = self.mon(ts, ti);
            let sturdy = m.ability == Ab::Sturdy && !mold;
            if sash || sturdy {
                dmg = m.hp - 1;
                if sash {
                    self.consume_item(ts, ti);
                }
            }
        }
        let (hp, band) = (self.mon(ts, ti).hp, self.mon(ts, ti).item == It::FocusBand);
        if dmg == hp && band && self.chance_of(1, 10) {
            dmg = hp - 1;
        }
        let m = self.mon_mut(ts, ti);
        m.hp -= dmg;
        m.vol |= vol::DAMAGED_THIS_TURN;
        m.vol2 |= vol2::LOST_FOCUS;
        let hp_now = m.hp;
        self.add(|| format!("|-damage|{ts}|{ti}|{hp_now}"));
        let m = self.mon_mut(ts, ti);
        if m.hp == 0 {
            // (2026-11-05) a damaging KO breaks the disguise first (Showdown shows '-end Illusion' before the faint)
            if m.illusion {
                m.illusion = false;
                m.rev |= rev::SEEN;
                self.add(|| format!("|-end|{ts}|{ti}|Illusion"));
            }
            self.faint(ts, ti);
        }
        dmg
    }

    /// Reactions to being hit by a damaging move (contact abilities, items...).
    /// Moxie / Eelevate after this Pokemon's move knocked one out
    fn ko_boost(&mut self, dex: &Dex, s: usize, i: usize) {
        if !self.mon(s, i).alive() {
            return;
        }
        match self.mon(s, i).ability {
            Ab::Eelevate => {
                let st = self.mon(s, i).stats;
                let mut best = 1;
                for k in 2..6 {
                    if st[k] > st[best] {
                        best = k;
                    }
                }
                let mut b = [0i8; 8];
                b[best] = 1;
                self.boost(dex, s, i, &b, Some((s, i)), false);
            }
            Ab::Moxie => {
                self.boost(dex, s, i, &[0, 1, 0, 0, 0, 0, 0, 0], Some((s, i)), false);
            }
            _ => {}
        }
    }

    fn on_damaging_hit(&mut self, dex: &Dex, s: usize, i: usize, mv: &MoveData, ts: usize, ti: usize, type_mod: i32) {
        let contact = mv.flags & flag::CONTACT != 0 && self.mon(s, i).ability != Ab::Longreach
            && !(self.mon(s, i).item == It::PunchingGlove && mv.flags & flag::PUNCH != 0);
        let tab = if self.mon(ts, ti).alive() { self.mon(ts, ti).ability } else { self.ab_before_hit };
        // a damaging hit ends Illusion (Showdown onDamagingHit -> End)
        if self.mon(ts, ti).illusion {
            self.mon_mut(ts, ti).illusion = false;
            self.mon_mut(ts, ti).rev |= rev::SEEN;
            self.add(|| format!("|-end|{ts}|{ti}|Illusion"));
        }
        let titem = if self.mon(ts, ti).has_item() { dex.items[self.mon(ts, ti).item_id as usize].id.as_str() } else { "" };
        let target_alive = self.mon(ts, ti).alive();
        let mt = self.cur_type;
        if tab == Ab::Seedsower && self.field.terrain != Terrain::Grassy {
            let item = self.mon(ts, ti).item_id;
            let ext = item != u16::MAX && dex.items[item as usize].id == "terrainextender";
            self.set_terrain(dex, Terrain::Grassy, ext, tsrc(ts, ti));
        }
        if tab == Ab::Electromorphosis {
            self.mon_mut(ts, ti).vol |= vol::CHARGED;
        }
        if tab == Ab::Sandspit {
            self.set_weather(dex, Weather::Sand, if titem == "smoothrock" { 8 } else { 5 }, tsrc(ts, ti));
        }
        if tab == Ab::Aftermath && !target_alive && contact && self.mon(s, i).alive()
            && !(0..2).any(|sd| (0..2).any(|k| self.active(sd, k).is_some_and(|x| self.mon(sd, x).alive() && self.mon(sd, x).ability == Ab::Damp)))
        {
            let amt = (self.mon(s, i).maxhp / 4).max(1);
            self.hurt(s, i, amt, "aftermath");
        }
        if tab == Ab::Spicyspray && self.mon(s, i).alive() {
            self.set_status(dex, s, i, Status::Burn, Some((ts, ti)));
        }
        // Toxic Debris: on the attacker's side, or on the foes' side when its own ally hit it (Earthquake...)
        if tab == Ab::Toxicdebris && mv.category == Category::Physical {
            let side = if ts == s { 1 - s } else { s };
            if self.sides[side].toxic_spikes < 2 {
                self.sides[side].toxic_spikes += 1;
            }
        }
        // Beak Blast's heat (its onHit runs before the DamagingHit abilities)
        if contact && self.mon(s, i).alive() && self.mon(ts, ti).vol2 & vol2::BEAK_BLAST != 0 {
            self.set_status(dex, s, i, Status::Burn, Some((ts, ti)));
        }
        if contact && self.mon(s, i).alive() {
            let att_guard = self.magic_guard(dex, s, i);
            // (the attacker's ability when the DamagingHit handlers were collected: Poison Touch still fires after Wandering
            // Spirit / Mummy replaced it during this hit; Showdown 2026-10-17)
            let aab0 = self.mon(s, i).ability;
            match tab {
                Ab::Roughskin | Ab::Ironbarbs if !att_guard => {
                    let amt = self.mon(s, i).maxhp / 8;
                    self.hurt(s, i, amt, "roughskin");
                }
                Ab::Static if self.chance_of(3, 10) => {
                    self.set_status(dex, s, i, Status::Paralysis, Some((ts, ti)));
                }
                Ab::Cutecharm if self.chance_of(3, 10) => {
                    self.try_attract(dex, s, i, ts, ti);
                }
                Ab::Flamebody if self.chance_of(3, 10) => {
                    self.set_status(dex, s, i, Status::Burn, Some((ts, ti)));
                }
                Ab::Poisonpoint if self.chance_of(3, 10) => {
                    self.set_status(dex, s, i, Status::Poison, Some((ts, ti)));
                }
                Ab::Gooey | Ab::Tanglinghair => {
                    self.boost(dex, s, i, &[0, 0, 0, 0, 0, -1, 0, 0], Some((ts, ti)), false);
                }
                Ab::Effectspore => {
                    let a = self.mon(s, i);
                    let goggles = a.has_item() && a.item == It::SafetyGoggles;
                    if !a.has_type(dex.ty.grass) && a.ability != Ab::Overcoat && !goggles {
                        let r = self.rng.below(100);
                        let st = if r < 11 { Status::Sleep } else if r < 21 { Status::Paralysis } else if r < 30 { Status::Poison } else { Status::None };
                        if st != Status::None {
                            self.set_status(dex, s, i, st, Some((ts, ti)));
                        }
                    }
                }
                Ab::Mummy => {
                    let aab = self.mon(s, i).ability;
                    let locked = matches!(aab, Ab::Mummy | Ab::Multitype | Ab::Stancechange | Ab::Schooling | Ab::Comatose | Ab::Shieldsdown
                        | Ab::Disguise | Ab::Rkssystem | Ab::Battlebond | Ab::Powerconstruct | Ab::Iceface | Ab::Gulpmissile | Ab::Zerotohero | Ab::Commander);
                    if !locked {
                        // (2026-10-18 targeted fuzz 3993269) Showdown setAbility('mummy'): always Mummy itself, also when the
                        // holder is a Ditto that this hit KO'd (faint undid its Transform: its own Limber must not spread)
                        let mid = dex.ability_index("mummy").map(|x| x as u16).unwrap_or(self.mon(ts, ti).ability_id);
                        let mk = self.ab_for(dex, s, i, mid);    // a Gastro-Acided attacker gets a suppressed Mummy
                        let a = self.mon_mut(s, i);
                        a.ability_id = mid;
                        a.ability = mk;
                        a.vol &= !(vol::UNBURDEN | vol::LIBERO_USED); // the old ability ends (Showdown End: Unburden drops its volatile; a new abilityState: Libero / Protean can fire again)
                    }
                }
                Ab::Wanderingspirit => {   // also when the hit KOs the holder (Showdown: its new ability even starts)
                    let aab = self.mon(s, i).ability;
                    let locked = matches!(aab, Ab::Wonderguard | Ab::Multitype | Ab::Illusion | Ab::Stancechange | Ab::Disguise | Ab::Schooling
                        | Ab::Comatose | Ab::Shieldsdown | Ab::Zenmode | Ab::Rkssystem | Ab::Battlebond | Ab::Powerconstruct | Ab::Iceface
                        | Ab::Gulpmissile | Ab::Neutralizinggas | Ab::Hungerswitch | Ab::Commander | Ab::Zerotohero | Ab::AsOne);
                    if !locked {
                        let (a_id, b_id) = (self.mon(s, i).ability_id, self.mon(ts, ti).ability_id);
                        let (ak, bk) = (self.ab_for(dex, ts, ti, a_id), self.ab_for(dex, s, i, b_id));
                        let _ = tab;
                        let m = self.mon_mut(s, i);
                        m.ability_id = b_id;
                        m.ability = bk;
                        m.vol &= !(vol::UNBURDEN | vol::LIBERO_USED); // the old ability ends (Showdown End: Unburden drops its volatile; a new abilityState: Libero / Protean can fire again)
                        let t = self.mon_mut(ts, ti);
                        t.ability_id = a_id;
                        t.ability = ak;
                        t.vol &= !(vol::UNBURDEN | vol::LIBERO_USED); // the old ability ends (Showdown End: Unburden drops its volatile; a new abilityState: Libero / Protean can fire again)
                        self.ability_start(dex, ts, ti);
                        self.ability_start(dex, s, i);
                    }
                }
                _ => {}
            }
            if titem == "rockyhelmet" && self.mon(s, i).alive() && !att_guard {
                let amt = self.mon(s, i).maxhp / 6;
                self.hurt(s, i, amt, "rockyhelmet");
            }
            if aab0 == Ab::Poisontouch && target_alive && tab != Ab::Shielddust && titem != "covertcloak" && self.chance_of(3, 10) {
                self.set_status(dex, ts, ti, Status::Poison, Some((s, i)));
            }
        }
        if !target_alive && tab == Ab::Innardsout && self.mon(s, i).alive() && !self.magic_guard(dex, s, i) && self.hp_before_hit > 0 {
            let amt = self.hp_before_hit.saturating_add(self.innards_prev);
            self.hurt(s, i, amt, "innardsout");
        }
        // Cursed Body: also when the hit KOs its holder (Showdown onDamagingHit has no HP check); not if the attacker is
        // already disabled, not Struggle, not a Future Sight hit
        if tab == Ab::Cursedbody && !self.future_now && mv.kind != Mv::Struggle && self.mon(s, i).alive()
            && self.mon(s, i).disable == 0 && !self.side_has(s, |a| a == Ab::Aromaveil) && self.chance_of(3, 10)
        {
            let lm = self.mon(s, i).last_move;
            let m = self.mon(s, i);
            if let Some(slot) = (0..m.n_moves as usize).find(|&k| m.moves[k] == lm) {
                let mm = self.mon_mut(s, i);
                mm.disable = 4;
                mm.disable_slot = slot as u8;
            }
        }
        if !target_alive {
            // Air Balloon pops on the KO hit too (Showdown: the hit's DamagingHit runs before the faint): Covet / Thief find no item
            if self.mon(ts, ti).item == It::AirBalloon && self.mon(ts, ti).has_item() {
                self.pop_balloon(ts, ti);
            }
            return;
        }
        if tab == Ab::Angerpoint && self.hit_crit {
            self.boost(dex, ts, ti, &[0, 12, 0, 0, 0, 0, 0, 0], Some((ts, ti)), false);
        }
        if tab == Ab::Berserk {
            let m = self.mon(ts, ti);
            if m.hp <= m.maxhp / 2 && self.hp_before_hit > m.maxhp / 2 {
                self.boost(dex, ts, ti, &[0, 0, 0, 1, 0, 0, 0, 0], Some((ts, ti)), false);
            }
        }
        match tab {
            Ab::Stamina => {
                self.boost(dex, ts, ti, &[0, 0, 1, 0, 0, 0, 0, 0], None, false);
            }
            // breakable (the only breakable onDamagingHit ability): Mold Breaker's Fire move gives no boost (2026-10-17 targeted fuzz)
            Ab::Thermalexchange if mt == dex.ty.fire && !matches!(self.mon(s, i).ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze) => {
                self.boost(dex, ts, ti, &[0, 1, 0, 0, 0, 0, 0, 0], None, false);
            }
            Ab::Justified if mt == dex.ty.dark => {
                self.boost(dex, ts, ti, &[0, 1, 0, 0, 0, 0, 0, 0], None, false);
            }
            Ab::Rattled if [dex.ty.dark, dex.ty.bug, dex.ty.ghost].contains(&mt) => {
                self.boost(dex, ts, ti, &[0, 0, 0, 0, 0, 1, 0, 0], None, false);
            }
            Ab::Weakarmor if mv.category == Category::Physical => {
                self.boost(dex, ts, ti, &[0, 0, -1, 0, 0, 2, 0, 0], None, false);
            }
            _ => {}
        }
        if titem == "weaknesspolicy" && type_mod > 0 {
            self.consume_item(ts, ti);
            self.boost(dex, ts, ti, &[0, 2, 0, 2, 0, 0, 0, 0], None, false);
        }
        // (Knock Off / Thief / Covet take the item in onAfterHit, before the Update that eats a Sitrus Berry: the berry
        // check waits for the next update point; Showdown 2026-10-17)
        if !matches!(mv.kind, Mv::KnockOff | Mv::Thief | Mv::Covet) {
            self.after_damage_items(dex, ts, ti);
        }
        // Emergency Exit / Wimp Out: dropped to half or below this hit
        // (also after a Sheer Force-boosted move: Champions mod, Showdown 2026-10-17; it used to skip the check)
        if tab == Ab::EmergencyExit && !self.dart_ee_hold {
            let m = self.mon(ts, ti);
            if m.alive() && m.hp <= m.maxhp / 2 && self.hp_before_hit > m.maxhp / 2 && !m.has_vol(vol::SWITCHING_IN) {
                if let Some(slot) = self.slot_of(ts, ti) {
                    self.mon_mut(ts, ti).vol |= vol::SWITCHING_IN;
                    self.request_switch(ts, slot);
                }
            }
        }
        // a frozen target thaws when a Fire move hits it (Showdown frz onDamagingHit) or a thawsTarget move (Scald...)
        if self.mon(ts, ti).status == Status::Freeze && self.mon(ts, ti).alive()
            && ((mt == dex.ty.fire && mv.category != Category::Status && mv.id != "polarflare")
                || matches!(mv.id.as_str(), "scald" | "steameruption" | "scorchingsands" | "matchagotcha" | "hydrosteam"))
        {
            self.mon_mut(ts, ti).status = Status::None;
        }
        // Air Balloon pops on any damaging hit (Showdown: onDamagingHit, then AfterUseItem -> Unburden / Symbiosis)
        if titem == "airballoon" && self.mon(ts, ti).item == It::AirBalloon && target_alive {
            self.pop_balloon(ts, ti);
        }
        // (a move dragging the holder out sets its forceSwitchFlag first: Showdown's Eject Button then does nothing)
        let being_dragged = mv.force_switch && !self.drag_blocked(s, i, ts, ti);
        // Thief / Covet steal it first (onAfterHit runs before the button's onAfterMoveSecondary), like Knock Off
        let stolen = matches!(mv.kind, Mv::Thief | Mv::Covet) && (ts, ti) != (s, i) && self.mon(s, i).alive() && !self.mon(s, i).has_item()
            && self.item_takeable(dex, ts, ti);
        // (the button's AfterMoveSecondary handler is collected after DamagingHit: Klutz gained from Wandering Spirit stops it)
        if titem == "ejectbutton" && self.mon(ts, ti).has_item() && dex.items[self.mon(ts, ti).item_id as usize].id == "ejectbutton" && !being_dragged && !stolen && (ts, ti) != (s, i) && mv.kind != crate::effects::Mv::KnockOff && !self.future_now {
            // Showdown: nothing happens while any active Pokemon already has its switchFlag (a spread move: the first holder hit
            // ejects, the others keep their buttons; Emergency Exit earlier in the same hit)
            let flagged = self.switch_needed.iter().any(|x| x.iter().any(|&f| f));
            if self.eject_defer {
                self.eject_cands.push((ts, ti));
            } else if let (Some(slot), false) = (self.slot_of(ts, ti), flagged) {
                if self.sides[ts].bench().next().is_some() {
                    self.consume_item_opt(ts, ti, false);   // useItem (Pickup, Recycle, Symbiosis)
                    self.request_switch(ts, slot);
                }
            }
        }
    }

    fn status_effects(&mut self, dex: &Dex, s: usize, i: usize, mv: &MoveData, ts: usize, ti: usize) {
        let foe = ts != s;
        if let Some(mut b) = mv.boosts {
            if mv.kind == Mv::Growth
                && (self.mon(s, i).ability == Ab::MegaSol
                    || (self.weather() == Weather::Sun
                        && !(self.mon(s, i).has_item() && dex.items[self.mon(s, i).item_id as usize].id == "utilityumbrella")))
            {
                b[ATK] = 2;
                b[SPA] = 2;
            }
            let ok = self.boost(dex, ts, ti, &b, Some((s, i)), false);
            // a pure stat-drop move stopped by White Smoke / Clear Body & co. FAILS (Showdown: moveThisTurnResult false ->
            // Stomping Tantrum doubles next turn; 2026-10-17 coverage fuzz)
            if !ok && self.boost_blocked && mv.category == Category::Status && mv.status.is_none() && mv.volatile.is_none()
                && mv.heal.is_none() && mv.secondaries.is_empty()
            {
                self.fail();
            }
        }
        if let Some(st) = mv.status {
            // a status move whose only effect is the status fails when it doesn't land (already statused, immune...):
            // Showdown moveThisTurnResult false (Stomping Tantrum next turn)
            if !self.set_status(dex, ts, ti, st, Some((s, i))) && mv.category == Category::Status && mv.boosts.is_none()
                && mv.volatile.is_none() && mv.heal.is_none()
            {
                self.fail();
            }
        }
        if let Some((num, den)) = mv.heal {
            if ts == s && ti == i && self.mon(ts, ti).hp >= self.mon(ts, ti).maxhp {
                // a self-heal at full HP fails (Roost then doesn't remove the Flying type either)
                self.fail();
                return;
            }
            let amt = ((self.mon(ts, ti).maxhp as u32 * num as u32 + den as u32 / 2) / den as u32) as u16;
            self.heal(ts, ti, amt, &mv.id);
        }
        // Octolock (Ghost types are immune: 'trapped' immunity)
        if mv.volatile.as_deref() == Some("octolock") && (ts, ti) != (s, i) && !self.mon(ts, ti).has_type(dex.ty.ghost)
            && self.mon(ts, ti).octolock_src == NONE
        {
            self.mon_mut(ts, ti).octolock_src = i as u8;
            self.mon_mut(ts, ti).octolock_side = s as u8;
        }
        if let Some(v) = mv.volatile.as_deref() {
            // Aroma Veil (onAllyTryAddVolatile): also against the holder's own ally's Encore / Taunt...; breakable (Mold Breaker)
            let aroma = self.side_has(ts, |a| a == Ab::Aromaveil)
                && !matches!(self.mon(s, i).ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze);
            let guarded = self.safeguarded(ts, ti, s, i);
            // Misty Terrain (grounded targets) and Own Tempo stop confusion (Swagger, Flatter, Confuse Ray...)
            let no_conf = self.misty_no_confusion(dex, ts, ti, Some((s, i)))
                || (self.mon(ts, ti).ability == Ab::Owntempo && !self.breaks_vs(ts, ti, Some((s, i))));
            // Encore / Taunt: one more turn if the target has no move still to come this turn (Showdown queue.willMove)
            let will_move = v == "encore"
                && self.queue.iter().any(|q| q.side as usize == ts && q.mon as usize == ti && matches!(q.kind, QKind::Move { .. }));
            let mut failed = false;
            let mut do_attract = false;
            let mut encore_set: Option<u8> = None;
            let m = self.mon_mut(ts, ti);
            match v {
                "minimize" => m.vol |= vol::MINIMIZE,
                "protect" | "detect" => {
                    m.vol |= vol::PROTECT;
                    m.protect_kind = ProtectKind::Plain;
                }
                "spikyshield" => {
                    m.vol |= vol::PROTECT;
                    m.protect_kind = ProtectKind::SpikyShield;
                }
                "kingsshield" => {
                    m.vol |= vol::PROTECT;
                    m.protect_kind = ProtectKind::KingsShield;
                }
                "banefulbunker" => {
                    m.vol |= vol::PROTECT;
                    m.protect_kind = ProtectKind::BanefulBunker;
                }
                "silktrap" => {
                    m.vol |= vol::PROTECT;
                    m.protect_kind = ProtectKind::SilkTrap;
                }
                "burningbulwark" => {
                    m.vol |= vol::PROTECT;
                    m.protect_kind = ProtectKind::BurningBulwark;
                }
                "endure" => m.vol |= vol::ENDURE,
                "followme" => m.vol |= vol::FOLLOW_ME,
                "ragepowder" => m.vol |= vol::RAGE_POWDER,
                "helpinghand" => m.vol |= vol::HELPING_HAND,
                "taunt" => {
                    if m.taunt == 0 && !aroma {
                        m.taunt = if m.has_vol(vol::MOVED) { 4 } else { 3 };
                    }
                }
                "encore" => {
                    // Showdown encore onStart: fails without a last move, on failencore moves, or a move it has no PP left for
                    let lm = m.last_move;
                    let slot = if lm == u16::MAX || dex.moves[lm as usize].flags & flag::FAILENCORE != 0 {
                        None
                    } else {
                        (0..m.n_moves as usize).find(|&k| m.moves[k] == lm && m.pp[k] > 0)
                    };
                    match slot {
                        Some(slot) if m.encore == 0 && !aroma => {
                            m.encore = if will_move { 3 } else { 4 };
                            m.encore_slot = slot as u8;
                            encore_set = Some(slot as u8);
                        }
                        _ => failed = !aroma,
                    }
                }
                "confusion" => {
                    if m.confusion == 0 && !guarded && !no_conf {
                        let n = 0x80u8;   // (2026-10-28, hidden counters) unknown 2-5 length: decided per check (BeforeMove)
                        self.mon_mut(ts, ti).confusion = n; // Showdown time 2-5, decremented before each move: 1-4 confused turns
                    } else if mv.category == Category::Status && mv.boosts.is_none() {
                        // Confuse Ray into Misty Terrain / Own Tempo / an already confused target: the move fails (Stomping
                        // Tantrum); Swagger / Flatter still did their boost
                        failed = true;
                    }
                }
                "imprison" => m.vol |= vol::IMPRISON,
                // Magnet Rise: 5 turns in the air (fails if already up, Smacked Down or rooted)
                "magnetrise" => {
                    if m.magnet_rise > 0 || m.vol2 & (vol2::SMACK_DOWN | vol2::INGRAIN) != 0 {
                        failed = true;
                    } else {
                        m.magnet_rise = 5;
                    }
                }
                "attract" => do_attract = true,
                "torment" => {
                    if m.vol2 & vol2::TORMENT != 0 || aroma {
                        failed = !aroma;
                    } else {
                        m.vol2 |= vol2::TORMENT;
                    }
                }
                "ingrain" => {
                    if m.vol2 & vol2::INGRAIN != 0 {
                        failed = true;
                    } else {
                        m.vol2 |= vol2::INGRAIN;
                    }
                }
                "leechseed" => {
                    let grass = m.has_type(dex.ty.grass);
                    if grass || m.has_vol(vol::LEECH_SEED) {
                        self.fail();
                    } else {
                        let slot = self.slot_of(s, i).unwrap_or(0) as u8;
                        let m = self.mon_mut(ts, ti);
                        m.vol |= vol::LEECH_SEED;
                        m.leech_slot = slot;
                        m.leech_side = s as u8;
                    }
                }
                // Charge: the next Electric move has double power (onRestart: using it again just keeps it)
                "charge" => {
                    m.vol |= vol::CHARGED;
                }
                "focusenergy" => {
                    // (2026-10-17) Focus Energy and Dragon Cheer exclude each other (Showdown onStart of each)
                    if m.has_vol(vol::FOCUS_ENERGY) || m.has_vol(vol::DRAGON_CHEER) {
                        self.fail();
                    } else {
                        m.vol |= vol::FOCUS_ENERGY;
                    }
                }
                "dragoncheer" => {
                    if m.has_vol(vol::DRAGON_CHEER) || m.has_vol(vol::FOCUS_ENERGY) {
                        self.fail();
                    } else {
                        let dragon = m.has_type(dex.ty.dragon);
                        m.vol |= vol::DRAGON_CHEER | if dragon { vol::DRAGON_CHEER2 } else { 0 };
                    }
                }
                "destinybond" => m.vol |= vol::DESTINY_BOND,
                _ => {}
            }
            if failed {
                self.fail();
            }
            if do_attract && !self.try_attract(dex, ts, ti, s, i) {
                self.fail();
            }
            if let Some(eslot) = encore_set {
                self.encore_change_action(dex, ts, ti, eslot);
            }
        }
        if mv.self_switch && mv.kind == Mv::Partingshot {
            // Showdown: onHit boosts {atk: -1, spa: -1}; no switch if nothing changed (except Mirror Armor)
            let mut b: Boosts = [0; 8];
            b[ATK] = -1;
            b[SPA] = -1;
            let ok = self.boost(dex, ts, ti, &b, Some((s, i)), false);
            let mirror = self.mon(ts, ti).ability == Ab::Mirrorarmor;
            if !ok && !mirror {
                self.cancel_self_switch = true;
            }
        }
        if mv.force_switch && !(ts == s && ti == i) && !self.drag_blocked(s, i, ts, ti) {
            let bench: ArrayVec<usize, 6> = self.sides[ts].bench().collect();
            if !bench.is_empty() {
                if let Some(slot) = self.slot_of(ts, ti) {
                    let to = bench[self.rng.below(bench.len() as u32) as usize];
                    self.do_switch(dex, ts, slot, to);
                }
            }
        }
        match mv.kind {
            Mv::Trick | Mv::Switcheroo if !(ts == s && ti == i) => {
                let (a, b) = (self.mon(s, i).clone(), self.mon(ts, ti).clone());
                // a Mega Stone can't be taken from a holder of its base species, even one that can't use it (Slowbronite on
                // Slowbro-Galar; Showdown onTakeItem; 2026-10-17 targeted fuzz)
                let mega_locked = a.mega_species != u16::MAX || b.mega_species != u16::MAX || a.is_mega || b.is_mega
                    || self.stone_locked(dex, s, i) || self.stone_locked(dex, ts, ti);
                // (2026-10-17 targeted fuzz) Showdown onTryImmunity: !target.hasAbility('stickyhold') (hasAbility: Gastro
                // Acid suppresses it, Mold Breaker does not); nothing to swap fails too (onHit returns false)
                let sticky = dex.abilities[b.ability_id as usize].id == "stickyhold" && b.vol2 & vol2::GASTRO == 0;
                if sticky {
                    self.add(|| format!("|-immune|{ts}|{ti}"));
                    self.fail();
                } else if (!a.has_item() && a.stash_item.0 == u16::MAX && !b.has_item() && b.stash_item.0 == u16::MAX) || mega_locked {
                    self.fail();
                } else {
                    // the HELD item, also one put aside by Klutz / Magic Room (Showdown takeItem ignores ignoringItem); the
                    // received item is put aside again if its new holder ignores items (2026-10-17 targeted fuzz 3750011: a
                    // Klutz Golurk kept its put-aside Haban Berry and also got the Scarf, so a second Knock Off was boosted)
                    let held = |m: &Mon| if m.item_id != u16::MAX { (m.item_id, m.item) } else { m.stash_item };
                    let ((ai, ak), (bi, bk)) = (held(&a), held(&b));
                    let mr = self.field.magic_room > 0;
                    let give = |m: &mut Mon, id: u16, it: It| {
                        if id != u16::MAX && (m.ability == Ab::Klutz || mr) {
                            m.stash_item = (id, it);
                            m.item_id = u16::MAX;
                            m.item = It::None;
                        } else {
                            m.stash_item = (u16::MAX, It::None);
                            m.item_id = id;
                            m.item = if id == u16::MAX { It::None } else { it };
                        }
                    };
                    let m = self.mon_mut(s, i);
                    give(m, bi, bk);
                    m.choice_lock = NONE;
                    m.rev |= rev::ITEM;
                    let t = self.mon_mut(ts, ti);
                    give(t, ai, ak);
                    t.choice_lock = NONE;
                    t.rev |= rev::ITEM;
                    // (2026-10-18 targeted fuzz 4096094) Unburden's onTakeItem: a holder whose item Trick takes gets the volatile
                    // (it doubles Speed only while empty-handed: Zoroark Tricking away a Ditto-Sneasler's Bright Powder)
                    for (xs, xi, had) in [(s, i, ai != u16::MAX), (ts, ti, bi != u16::MAX)] {
                        let m = self.mon_mut(xs, xi);
                        if had && m.ability == Ab::Unburden && m.vol2 & vol2::GASTRO == 0 {
                            m.vol |= vol::UNBURDEN;
                        }
                    }
                    let (ak, bk) = (self.mon(ts, ti).item, self.mon(s, i).item);
                    // a berry received at low HP is eaten at once; a terrain seed in its terrain too
                    self.check_sitrus(s, i);
                    self.check_sitrus(ts, ti);
                    self.seed_check(dex, s, i);
                    self.seed_check(dex, ts, ti);
                    if bk == It::Metronome {
                        self.metro_start(s, i);
                    }
                    if ak == It::Metronome {
                        self.metro_start(ts, ti);
                    }
                }
            }
            Mv::Spite => {
                let t = self.mon(ts, ti);
                let lm = t.last_move;
                let slot = if lm == u16::MAX { None } else { (0..t.n_moves as usize).find(|&k| t.moves[k] == lm && t.pp[k] > 0) };
                match slot {
                    Some(k) => {
                        let t = self.mon_mut(ts, ti);
                        t.pp[k] = t.pp[k].saturating_sub(4);
                    }
                    None => self.fail(),
                }
            }
            Mv::Worryseed | Mv::Gastroacid => {
                let t = self.mon(ts, ti);
                let blocked = dex.abilities[t.ability_id as usize].cant_suppress
                    || (mv.kind == Mv::Worryseed && matches!(dex.abilities[t.ability_id as usize].kind, Ab::Truant | Ab::Insomnia))
                    || (mv.kind == Mv::Gastroacid && t.vol2 & vol2::GASTRO != 0);
                if blocked {
                    self.fail();
                } else if mv.kind == Mv::Worryseed {
                    let ins = dex.ability_index("insomnia").unwrap_or(0) as u16;
                    let ik = self.ab_for(dex, ts, ti, ins);
                    let t = self.mon_mut(ts, ti);
                    t.ability_id = ins;
                    t.ability = ik;
                    t.vol &= !(vol::UNBURDEN | vol::LIBERO_USED); // the old ability ends (Showdown End: Unburden drops its volatile; a new abilityState: Libero / Protean can fire again)
                    if t.status == Status::Sleep {
                        t.status = Status::None;
                        t.status_turns = 0;
                    }
                } else {
                    let t = self.mon_mut(ts, ti);
                    t.vol2 |= vol2::GASTRO;
                    t.ability = Ab::None;
                    t.vol &= !(vol::UNBURDEN | vol::LIBERO_USED); // the old ability ends (Showdown End: Unburden drops its volatile; a new abilityState: Libero / Protean can fire again)
                }
            }
            Mv::Powerswap | Mv::Guardswap => {
                let (a, b) = if mv.kind == Mv::Powerswap { (ATK, SPA) } else { (DEF, SPD) };
                let (sb, tb) = (self.mon(s, i).boosts, self.mon(ts, ti).boosts);
                let m = self.mon_mut(s, i);
                m.boosts[a] = tb[a];
                m.boosts[b] = tb[b];
                let t = self.mon_mut(ts, ti);
                t.boosts[a] = sb[a];
                t.boosts[b] = sb[b];
            }
            Mv::Powersplit | Mv::Guardsplit => {
                let (a, b) = if mv.kind == Mv::Powersplit { (ATK, SPA) } else { (DEF, SPD) };
                for st in [a, b] {
                    let avg = ((self.mon(s, i).stats[st] as u32 + self.mon(ts, ti).stats[st] as u32) / 2) as u16;
                    self.mon_mut(s, i).stats[st] = avg;
                    self.mon_mut(ts, ti).stats[st] = avg;
                }
                self.mon_mut(s, i).vol2 |= vol2::SPLIT;
                self.mon_mut(ts, ti).vol2 |= vol2::SPLIT;
            }
            Mv::Meanlook | Mv::Block => {
                // (also on the user's own partner: Showdown's 'trapped' volatile links to its source on either side)
                if (ts, ti) == (s, i) || self.mon(ts, ti).has_type(dex.ty.ghost) || self.has_trapped_vol(ts, ti) {
                    self.fail();
                } else {
                    self.mon_mut(ts, ti).trap_src = i as u8;
                    self.mon_mut(ts, ti).trap_side = s as u8;
                }
            }
            Mv::Reflecttype => {
                let sid = dex.species[self.mon(s, i).species as usize].id.as_str();
                let t = self.mon(ts, ti);
                let none = t.types.iter().all(|&x| x == crate::dex::NO_TYPE);
                if sid.starts_with("arceus") || sid.starts_with("silvally") || (none && t.added_type == crate::dex::NO_TYPE) {
                    self.fail();
                } else {
                    let (ty, added) = if none { ([dex.ty.normal, crate::dex::NO_TYPE], t.added_type) } else { (t.types, t.added_type) };
                    let m = self.mon_mut(s, i);
                    m.types = ty;
                    m.added_type = added;
                }
            }
            Mv::Lockon => {
                if self.mon(s, i).lock_on > 0 || ts == s {
                    self.fail();
                } else {
                    let m = self.mon_mut(s, i);
                    m.lock_on = 2;
                    m.lock_on_tgt = ti as u8;
                }
            }
            Mv::Recycle => {
                let m = self.mon(s, i);
                if m.has_item() || m.last_item == u16::MAX {
                    self.fail();
                } else {
                    let id = m.last_item;
                    let kind = dex.items[id as usize].kind;
                    let m = self.mon_mut(s, i);
                    m.item_id = id;
                    m.item = kind;
                    m.last_item = u16::MAX;
                    m.rev |= rev::ITEM;
                    m.vol &= !vol::UNBURDEN;
                    self.check_sitrus(s, i);
                    self.seed_check(dex, s, i);
                }
            }
            Mv::Stuffcheeks => {
                let mut b = [0i8; 8];
                b[DEF] = 2;
                if self.boost(dex, s, i, &b, Some((s, i)), false) {
                    self.eat_berry_now(s, i);
                } else {
                    self.fail();
                }
            }
            Mv::Tidyup => {
                let mut success = false;
                for sd in 0..2 {
                    for k in 0..2 {
                        if let Some(x) = self.active(sd, k) {
                            if self.mon(sd, x).sub_hp > 0 {
                                self.mon_mut(sd, x).sub_hp = 0;
                                success = true;
                            }
                        }
                    }
                    let side = &mut self.sides[sd];
                    if side.stealth_rock || side.spikes > 0 || side.toxic_spikes > 0 || side.sticky_web {
                        success = true;
                        side.stealth_rock = false;
                        side.spikes = 0;
                        side.toxic_spikes = 0;
                        side.sticky_web = false;
                    }
                }
                let mut b = [0i8; 8];
                b[ATK] = 1;
                b[SPE] = 1;
                let boosted = self.boost(dex, s, i, &b, Some((s, i)), false);
                if !(boosted || success) {
                    self.fail();
                }
            }
            Mv::Roost => {
                self.mon_mut(ts, ti).vol |= vol::ROOST;
            }
            Mv::Revivalblessing => {
                // (2026-10-19) the player picks a fainted Pokemon: a mid-turn request like U-turn's (Showdown: selfSwitch +
                // slotCondition 'revivalblessing'); apply_switch_choices revives the pick
                if (0..self.sides[s].mons.len()).any(|k| self.sides[s].mons[k].fainted) {
                    if let Some(slot) = self.slot_of(s, i) {
                        self.reviving[s][slot] = true;
                        self.switch_needed[s][slot] = true;
                        self.phase = Phase::Switch;
                    }
                } else {
                    self.fail();
                }
            }
            Mv::Substitute if !foe => {
                let m = self.mon(s, i);
                if m.sub_hp > 0 || m.hp <= m.maxhp / 4 || m.maxhp == 1 {
                    self.fail();
                } else {
                    let amt = m.maxhp / 4;
                    self.hurt(s, i, amt, "substitute");
                    self.mon_mut(s, i).sub_hp = amt;
                    // (a new Substitute frees it from binding moves)
                    self.mon_mut(s, i).bound = 0;
                }
            }
            Mv::Skillswap => {
                let banned = |id: Ab| matches!(id, Ab::Wonderguard | Ab::Multitype | Ab::Illusion | Ab::Stancechange | Ab::Disguise
                    | Ab::Schooling | Ab::Comatose | Ab::Shieldsdown | Ab::Zenmode | Ab::Rkssystem | Ab::Battlebond | Ab::Powerconstruct
                    | Ab::Iceface | Ab::Gulpmissile | Ab::Neutralizinggas | Ab::Hungerswitch | Ab::Commander | Ab::Zerotohero
                    | Ab::AsOne | Ab::Protosynthesis | Ab::Quarkdrive | Ab::Terashift | Ab::Teraformzero | Ab::Poisonpuppeteer);
                let (a_id, b_id) = (self.mon(s, i).ability_id, self.mon(ts, ti).ability_id);
                if banned(dex.abilities[a_id as usize].kind) || banned(dex.abilities[b_id as usize].kind) {
                    self.fail();
                } else {
                    // the real abilities swap (Gastro Acid stays with each Pokemon and keeps suppressing what it now has)
                    let (ak, bk) = (self.ab_for(dex, ts, ti, a_id), self.ab_for(dex, s, i, b_id));
                    let m = self.mon_mut(s, i);
                    m.ability_id = b_id;
                    m.ability = bk;
                    m.vol &= !(vol::UNBURDEN | vol::LIBERO_USED); // the old ability ends (Showdown End: Unburden drops its volatile; a new abilityState: Libero / Protean can fire again)
                    let t = self.mon_mut(ts, ti);
                    t.ability_id = a_id;
                    t.ability = ak;
                    t.vol &= !(vol::UNBURDEN | vol::LIBERO_USED); // the old ability ends (Showdown End: Unburden drops its volatile; a new abilityState: Libero / Protean can fire again)
                    // the target's new ability starts first, then the user's
                    self.ability_start(dex, ts, ti);
                    self.ability_start(dex, s, i);
                }
            }
            Mv::Healpulse => {
                let launcher = self.mon(s, i).ability == Ab::MegaLauncher;
                let mx = self.mon(ts, ti).maxhp as f64;
                let amt = (mx * if launcher { 0.75 } else { 0.5 }).ceil() as u16;
                self.heal(ts, ti, amt, "healpulse");
            }
            Mv::Psychup => {
                let b = self.mon(ts, ti).boosts;
                self.mon_mut(s, i).boosts = b;
                // also the crit-stage volatiles (Showdown: removed from the user, then copied; Dragon Cheer keeps the
                // target's hasDragonType; 2026-10-17 coverage fuzz)
                let crit = vol::FOCUS_ENERGY | vol::DRAGON_CHEER | vol::DRAGON_CHEER2;
                let tv = self.mon(ts, ti).vol & crit;
                let m = self.mon_mut(s, i);
                m.vol = (m.vol & !crit) | tv;
            }
            Mv::Allyswitch => {
                // onPrepareHit: the 'allyswitch' volatile works like Protect's stall counter (1 in 3, 9, ... when used again
                // while it lasts; it lasts until the end of the next turn)
                let ok = {
                    let c = self.mon(s, i).ally_sw;
                    if c == 0 {
                        self.mon_mut(s, i).ally_sw = 3;
                        true
                    } else if self.rng.chance(1, c as u32) {
                        self.mon_mut(s, i).ally_sw = (c * 3).min(729);
                        true
                    } else {
                        self.mon_mut(s, i).ally_sw = 0;
                        self.mon_mut(s, i).ally_sw_dur = 0;
                        false
                    }
                };
                if !ok {
                    self.fail();
                } else {
                    self.mon_mut(s, i).ally_sw_dur = 2;
                    let a = self.sides[s].active;
                    let me = self.slot_of(s, i).unwrap_or(0);
                    let other = a[1 - me];
                    if other == NONE || !self.mon(s, other as usize).alive() {
                        self.fail();
                    } else {
                        self.sides[s].active = [a[1], a[0]];
                        let mut o = self.sides[s].party_order();
                        o.swap(0, 1);
                        self.sides[s].party = o;
                        self.sides[s].party_set = true;
                    }
                }
            }
            Mv::Topsyturvy => {
                let m = self.mon_mut(ts, ti);
                if m.boosts.iter().skip(1).all(|&b| b == 0) {
                    self.fail();
                } else {
                    for b in m.boosts.iter_mut().skip(1) {
                        *b = -*b;
                    }
                }
            }
            Mv::Corrosivegas => {
                if self.mon(ts, ti).has_item() && self.item_takeable(dex, ts, ti) {
                    self.remove_item(ts, ti);
                } else {
                    self.fail();
                }
            }
            Mv::Defog => {
                if self.mon(ts, ti).sub_hp == 0 || self.mon(s, i).ability == Ab::Infiltrator {
                    let mut b = Boosts::default();
                    b[EVA] = -1;
                    self.boost(dex, ts, ti, &b, Some((s, i)), true);
                }
                let side = &mut self.sides[ts];
                side.reflect = 0;
                side.light_screen = 0;
                side.aurora_veil = 0;
                side.safeguard = 0;
                for x in [ts, s] {
                    let side = &mut self.sides[x];
                    side.stealth_rock = false;
                    side.spikes = 0;
                    side.toxic_spikes = 0;
                    side.sticky_web = false;
                }
                self.field.terrain = Terrain::None;
                self.field.terrain_turns = 0;
            self.terrain_changed(dex);
            }
            Mv::Swallow => {
                let layers = self.mon(s, i).stockpile;
                if layers == 0 {
                    self.fail();
                } else {
                    let mx = self.mon(s, i).maxhp as u32;
                    let amt = match layers { 1 => mx / 4, 2 => mx / 2, _ => mx } as u16;
                    self.heal(s, i, amt.max(1), "swallow");
                    self.mon_mut(s, i).stockpile = 0;
                    let l = layers as i8;
                    self.boost(dex, s, i, &[0, 0, -l, 0, -l, 0, 0, 0], Some((s, i)), false);
                }
            }
            Mv::Aquaring => {
                if self.mon(s, i).vol2 & vol2::AQUA_RING != 0 {
                    self.fail();
                } else {
                    self.mon_mut(s, i).vol2 |= vol2::AQUA_RING;
                }
            }
            Mv::Electrify => {
                let moved = self.mon(ts, ti).has_vol(vol::MOVED);
                if moved {
                    self.fail();
                } else {
                    self.mon_mut(ts, ti).vol2 |= vol2::ELECTRIFY;
                }
            }
            Mv::Powertrick => {
                let m = self.mon_mut(s, i);
                m.stats.swap(1, 2);
                m.vol2 ^= vol2::POWER_TRICK;
            }
            Mv::Stockpile => {
                if self.mon(s, i).stockpile >= 3 {
                    self.fail();
                } else {
                    self.mon_mut(s, i).stockpile += 1;
                    self.boost(dex, s, i, &[0, 0, 1, 0, 1, 0, 0, 0], Some((s, i)), false);
                }
            }
            Mv::Healingwish => {
                if let Some(slot) = self.slot_of(s, i) {
                    self.sides[s].healing_wish[slot] = true;
                }
            }
            Mv::Magicpowder => {
                let m = self.mon_mut(ts, ti);
                m.types = [dex.ty.psychic, crate::dex::NO_TYPE];
                m.added_type = crate::dex::NO_TYPE;
            }
            Mv::Quash => {
                // the target moves last this turn
                if let Some(p) = self.queue.iter().position(|q| q.side as usize == ts && q.mon as usize == ti && matches!(q.kind, QKind::Move { .. })) {
                    let mut q = self.queue.remove(p);
                    q.order = 201;
                    self.queue.push(q);
                } else {
                    self.fail();
                }
            }
            Mv::Entrainment => {
                let aid = self.mon(s, i).ability_id;
                let ak = self.ab_for(dex, ts, ti, aid);
                let tname = dex.abilities[self.mon(ts, ti).ability_id as usize].kind;
                let uname = dex.abilities[aid as usize].id.as_str();
                let locked = matches!(tname, Ab::Truant | Ab::Multitype | Ab::Stancechange | Ab::Schooling | Ab::Comatose | Ab::Shieldsdown
                    | Ab::Disguise | Ab::Rkssystem | Ab::Battlebond | Ab::Powerconstruct | Ab::Iceface | Ab::Gulpmissile | Ab::Zerotohero | Ab::Commander);
                let unshareable = matches!(uname, "trace" | "forecast" | "flowergift" | "zenmode" | "illusion" | "imposter"
                    | "powerofalchemy" | "receiver" | "neutralizinggas" | "hungerswitch" | "multitype" | "stancechange" | "disguise");
                if locked || unshareable || self.mon(ts, ti).ability_id == aid {
                    self.fail();
                } else {
                    let t = self.mon_mut(ts, ti);
                    t.ability_id = aid;
                    t.ability = ak;
                    t.vol &= !(vol::UNBURDEN | vol::LIBERO_USED); // the old ability ends (Showdown End: Unburden drops its volatile; a new abilityState: Libero / Protean can fire again)
                    self.ability_start(dex, ts, ti);
                }
            }
            Mv::Soak => {
                let water = dex.ty.water;
                let m = self.mon_mut(ts, ti);
                m.types = [water, crate::dex::NO_TYPE];
                m.added_type = crate::dex::NO_TYPE;
            }
            // (2026-10-16) a third type; fails if it already has that type
            Mv::Trickortreat | Mv::Forestscurse => {
                let t = if mv.kind == Mv::Trickortreat { dex.ty.ghost } else { dex.ty.grass };
                if self.mon(ts, ti).has_type(t) {
                    self.fail();
                } else {
                    self.mon_mut(ts, ti).added_type = t;
                }
            }
            Mv::Yawn => {
                let blocked = self.safeguarded(ts, ti, s, i) || self.side_has(ts, |a| a == Ab::Sweetveil)
                    || (self.mon(ts, ti).ability == Ab::Leafguard && self.weather() == Weather::Sun)
                    // Electric Terrain stops Yawn on grounded targets (onTryAddVolatile)
                    || (self.field.terrain == Terrain::Electric && self.grounded_brk(dex, ts, ti, self.breaks_vs(ts, ti, Some((s, i))))
                        && !self.semi_invulnerable(dex, ts, ti));
                let m = self.mon(ts, ti);
                if m.status == Status::None && m.yawn == 0 && !blocked {
                    self.mon_mut(ts, ti).yawn = 2;
                } else {
                    self.fail();
                }
            }
            Mv::Strengthsap => {
                let t = self.mon(ts, ti);
                if t.boosts[ATK] == -6 {
                    self.fail();
                } else {
                    let atk = crate::stats::boosted(t.stats[ATK] as u32, t.boosts[ATK]) as u16;
                    let ok = self.boost(dex, ts, ti, &[0, -1, 0, 0, 0, 0, 0, 0], Some((s, i)), false);
                    // Liquid Ooze (onSourceTryHeal covers strengthsap): the user takes the amount as damage instead, also at full
                    // HP (TryHeal runs before heal()'s full-HP check)
                    let healed = if self.mon(ts, ti).ability == Ab::Liquidooze && self.mon(s, i).alive() {
                        let oz = self.ooze_amt(s, i, atk.max(1));
                        self.hurt(s, i, oz, "liquidooze");
                        true
                    } else if self.mon(s, i).heal_block == 0 && self.mon(s, i).hp < self.mon(s, i).maxhp {
                        self.heal(s, i, atk.max(1), "strengthsap");
                        true
                    } else {
                        false
                    };
                    if !ok && !healed {
                        self.fail();
                    }
                }
            }
            Mv::Weatherheal => {
                let m = self.mon(s, i);
                let umbrella = m.has_item() && dex.items[m.item_id as usize].id == "utilityumbrella";
                // Mega Sol: the holder's moves see sun (Pokemon.effectiveWeather), umbrella or not
                let w = if m.ability == Ab::MegaSol { Weather::Sun } else { self.weather() };
                let umbrella = umbrella && m.ability != Ab::MegaSol;
                let factor: i64 = match w {
                    Weather::Sun if !umbrella => 2732,
                    Weather::Rain if !umbrella => 1024,
                    Weather::Sand | Weather::Snow => 1024,
                    _ => 2048,
                };
                if m.hp >= m.maxhp || m.heal_block > 0 {
                    self.fail();
                } else {
                    let amt = crate::damage::modify(m.maxhp as i64, factor).max(1) as u16;
                    self.heal(s, i, amt, "weatherheal");
                }
            }
            Mv::Painsplit if foe || !(ts == s && ti == i) => {
                let avg = ((self.mon(ts, ti).hp as u32 + self.mon(s, i).hp as u32) / 2).max(1) as u16;
                for (a, b) in [(ts, ti), (s, i)] {
                    let m = self.mon_mut(a, b);
                    m.hp = avg.min(m.maxhp);
                }
            }
            Mv::Shedtail => {
                let m = self.mon(s, i);
                let half = (m.maxhp + 1) / 2;
                if self.sides[s].bench().next().is_none() || m.sub_hp > 0 || m.hp <= half {
                    self.cancel_self_switch = true;
                    self.fail();
                } else {
                    let sub = m.maxhp / 4;
                    self.hurt(s, i, half, "shedtail");
                    if let Some(slot) = self.slot_of(s, i) {
                        self.sides[s].shed_sub[slot] = sub;
                    }
                }
            }
            Mv::Disable if !(ts == s && ti == i) => {
                let t = self.mon(ts, ti);
                let lm = t.last_move;
                let slot = if lm == u16::MAX { None } else { (0..t.n_moves as usize).find(|&k| t.moves[k] == lm) };
                let aroma = self.side_has(ts, |a| a == Ab::Aromaveil);
                match slot {
                    Some(k) if t.disable == 0 && t.pp[k] > 0 && dex.moves[lm as usize].kind != Mv::Struggle && !aroma => {
                        let will_move = self.queue.iter().any(|q| q.side as usize == ts && q.mon as usize == ti && matches!(q.kind, QKind::Move { .. }));
                        let mm = self.mon_mut(ts, ti);
                        mm.disable = if will_move { 4 } else { 5 };
                        mm.disable_slot = k as u8;
                    }
                    _ => self.fail(),
                }
            }
            Mv::Instruct => {
                let t = self.mon(ts, ti);
                let lm = t.last_move;
                let slot = if lm == u16::MAX { None } else { (0..t.n_moves as usize).find(|&k| t.moves[k] == lm) };
                let ok = match slot {
                    Some(k) => {
                        let md = &dex.moves[lm as usize];
                        t.alive() && t.pp[k] > 0 && t.charge_slot == NONE && !t.has_vol(vol::MUST_RECHARGE)
                            && md.flags & (flag::FAILINSTRUCT | flag::CHARGE) == 0 && md.self_volatile.as_deref() != Some("mustrecharge")
                    }
                    None => false,
                };
                match (ok, self.slot_of(ts, ti)) {
                    (true, Some(tslot)) => {
                        let (k, lt) = (slot.unwrap() as u8, self.mon(ts, ti).last_target);
                        self.queue.insert(0, QAction { order: ORDER_MOVE - 1, priority: 0, speed: 0, tie: 0, side: ts as u8, slot: tslot as u8,
                            mon: ti as u8, quick: false, stall: false, kind: QKind::Move { mslot: k, target: lt, orig: NONE } });
                    }
                    _ => self.fail(),
                }
            }
            Mv::Curse => {
                if !self.mon(s, i).has_type(dex.ty.ghost) {
                    if !self.boost(dex, s, i, &[0, 1, 1, 0, 0, -1, 0, 0], Some((s, i)), false) {
                        self.fail();
                    }
                } else if ts == s && ti == i {
                    // the Champions 'self' path above (onTryHit: fails if the USER is cursed)
                    if self.mon(s, i).has_vol(vol::CURSE) {
                        self.fail();
                    } else {
                        let amt = (self.mon(s, i).maxhp / 2).max(1);
                        self.hurt(s, i, amt, "cursecost");
                        let foes: ArrayVec<usize, 2> = (0..2).filter_map(|k| self.active(1 - s, k)).filter(|&f| self.mon(1 - s, f).alive()).collect();
                        if foes.is_empty() {
                            self.fail();
                        } else {
                            let f = foes[self.pick(foes.len())];
                            self.mon_mut(1 - s, f).vol |= vol::CURSE;
                        }
                    }
                } else if self.mon(ts, ti).has_vol(vol::CURSE) {
                    self.fail();
                } else {
                    let amt = (self.mon(s, i).maxhp / 2).max(1);
                    self.hurt(s, i, amt, "cursecost");
                    self.mon_mut(ts, ti).vol |= vol::CURSE;
                }
            }
            Mv::Transform if !(ts == s && ti == i) => {
                if !self.transform_into(dex, s, i, ts, ti) {
                    self.fail();
                }
            }
            Mv::Rest => {
                let m = self.mon(s, i);
                if m.status == Status::Sleep || m.hp >= m.maxhp || matches!(m.ability, Ab::Insomnia | Ab::Vitalspirit | Ab::Comatose) {
                    self.fail();
                } else {
                    let old = (m.status, m.status_turns);
                    self.mon_mut(s, i).status = Status::None;
                    if self.set_status(dex, s, i, Status::Sleep, Some((s, i))) {
                        if self.mon(s, i).status == Status::Sleep {
                            self.mon_mut(s, i).status_turns = 3;
                        }
                        let amt = self.mon(s, i).maxhp;
                        self.heal(s, i, amt, "rest");
                    } else {
                        let m = self.mon_mut(s, i);
                        m.status = old.0;
                        m.status_turns = old.1;
                        self.fail();
                    }
                }
            }
            Mv::Simplebeam => {
                let tab = dex.abilities[self.mon(ts, ti).ability_id as usize].kind;
                let locked = matches!(tab, Ab::Simple | Ab::Truant | Ab::Multitype | Ab::Stancechange | Ab::Schooling | Ab::Comatose | Ab::Shieldsdown
                    | Ab::Disguise | Ab::Rkssystem | Ab::Battlebond | Ab::Powerconstruct | Ab::Iceface | Ab::Gulpmissile | Ab::Zerotohero | Ab::Commander);
                match (locked, dex.ability_index("simple")) {
                    (false, Some(idx)) => {
                        let sk = self.ab_for(dex, ts, ti, idx as u16);
                        let t = self.mon_mut(ts, ti);
                        t.ability_id = idx as u16;
                        t.ability = sk;
                        t.vol &= !(vol::UNBURDEN | vol::LIBERO_USED); // the old ability ends (Showdown End: Unburden drops its volatile; a new abilityState: Libero / Protean can fire again)
                    }
                    _ => self.fail(),
                }
            }
            Mv::Acupressure => {
                let bo = self.mon(ts, ti).boosts;
                let ok: ArrayVec<usize, 7> = (1..8).filter(|&k| bo[k] < 6).collect();
                if ok.is_empty() {
                    self.fail();
                } else {
                    let k = ok[self.rng.below(ok.len() as u32) as usize];
                    let mut b = [0i8; 8];
                    b[k] = 2;
                    self.boost(dex, ts, ti, &b, Some((s, i)), false);
                }
            }
            Mv::Speedswap if !(ts == s && ti == i) => {
                let a = self.mon(s, i).stats[5];
                let b = self.mon(ts, ti).stats[5];
                self.mon_mut(s, i).stats[5] = b;
                self.mon_mut(ts, ti).stats[5] = a;
                // (switching out restores the species' Speed: Showdown clearVolatile -> setSpecies)
                self.mon_mut(s, i).vol2 |= vol2::SPLIT;
                self.mon_mut(ts, ti).vol2 |= vol2::SPLIT;
            }
            Mv::Wish => {
                if let Some(k) = self.slot_of(s, i) {
                    if self.sides[s].wish[k].0 > 0 {
                        self.fail();
                    } else {
                        let amt = self.mon(s, i).maxhp / 2;
                        self.sides[s].wish[k] = (2, amt);
                    }
                }
            }
            Mv::Noretreat => {
                // Champions (Showdown 2026-10-16): the volatile adds 'trapped' with no source; when the user already
                // has 'trapped' (Mean Look...) or is immune to it (Ghost), No Retreat still lands (no second use) but
                // adds no trap of its own
                self.mon_mut(s, i).vol |= vol::NO_RETREAT;
                if !self.has_trapped_vol(s, i) && !self.mon(s, i).has_type(dex.ty.ghost) {
                    let m = self.mon_mut(s, i);
                    m.trap_src = TRAP_SELF;
                    m.trap_side = s as u8;
                }
            }
            Mv::Roleplay if !(ts == s && ti == i) => {
                let tid_ = self.mon(ts, ti).ability_id;
                let tk = self.ab_for(dex, s, i, tid_);
                let banned = matches!(dex.abilities[tid_ as usize].id.as_str(), "trace" | "forecast" | "flowergift" | "zenmode" | "illusion"
                    | "imposter" | "powerofalchemy" | "receiver" | "neutralizinggas" | "hungerswitch" | "multitype" | "stancechange"
                    | "disguise" | "schooling" | "shieldsdown" | "comatose" | "rkssystem" | "battlebond" | "powerconstruct" | "iceface"
                    | "gulpmissile" | "zerotohero" | "commander" | "wonderguard" | "protosynthesis" | "quarkdrive");
                if banned || tid_ == self.mon(s, i).ability_id {
                    self.fail();
                } else {
                    let m = self.mon_mut(s, i);
                    m.ability_id = tid_;
                    m.ability = tk;
                    m.vol &= !(vol::UNBURDEN | vol::LIBERO_USED); // the old ability ends (Showdown End: Unburden drops its volatile; a new abilityState: Libero / Protean can fire again)
                    self.ability_start(dex, s, i);
                }
            }
            Mv::Afteryou => {
                // the target moves next
                if let Some(p) = self.queue.iter().position(|q| q.side as usize == ts && q.mon as usize == ti && matches!(q.kind, QKind::Move { .. })) {
                    let mut q = self.queue.remove(p);
                    q.order = ORDER_MOVE - 1;
                    self.queue.push(q);
                } else {
                    self.fail();
                }
            }
            _ => {}
        }
    }

    /// Moves that affect the field or a whole side.
    fn field_move(&mut self, dex: &Dex, s: usize, i: usize, mv: &MoveData) {
        if let Some(w) = mv.weather.as_deref() {
            let item = self.mon(s, i).item_id;
            let rock = if item == u16::MAX { "" } else { dex.items[item as usize].id.as_str() };
            let (weather, ext) = match w {
                "raindance" => (Weather::Rain, rock == "damprock"),
                "sunnyday" => (Weather::Sun, rock == "heatrock"),
                "sandstorm" => (Weather::Sand, rock == "smoothrock"),
                "snowscape" | "snow" | "hail" => (Weather::Snow, rock == "icyrock"),
                _ => return,
            };
            let chilly = mv.kind == Mv::Chillyreception;
            if self.field.weather == weather && !chilly {
                return self.fail();
            }
            self.set_weather(dex, weather, if ext { 8 } else { 5 }, tsrc(s, i));
            if chilly && self.sides[s].bench().next().is_some() {
                if let Some(slot) = self.slot_of(s, i) {
                    self.request_switch(s, slot);
                }
            }
            return;
        }
        if let Some(t) = mv.terrain.as_deref() {
            let item = self.mon(s, i).item_id;
            let ext = item != u16::MAX && dex.items[item as usize].id == "terrainextender";
            let terrain = match t {
                "grassyterrain" => Terrain::Grassy,
                "psychicterrain" => Terrain::Psychic,
                "electricterrain" => Terrain::Electric,
                "mistyterrain" => Terrain::Misty,
                _ => return,
            };
            if self.field.terrain == terrain {
                return self.fail();
            }
            self.set_terrain(dex, terrain, ext, tsrc(s, i));
            return;
        }
        if mv.pseudo_weather.as_deref() == Some("gravity") {
            if self.field.gravity > 0 {
                return self.fail();
            }
            self.field.gravity = 5;
            // (2026-10-17) Gravity's onFieldStart: a Pokemon in the air from Fly / Bounce comes down (charge ended, its queued move
            // cancelled) and Magnet Rise ends
            for side in 0..2 {
                for k in 0..2 {
                    let Some(x) = self.active(side, k) else { continue };
                    if !self.mon(side, x).alive() {
                        continue;
                    }
                    if self.charging_move(side, x).is_some_and(|c| matches!(dex.moves[c].id.as_str(), "fly" | "bounce")) {
                        self.mon_mut(side, x).charge_slot = NONE;
                        self.queue.retain(|q| !(q.side as usize == side && q.mon as usize == x && matches!(q.kind, QKind::Move { .. })));
                    }
                    self.mon_mut(side, x).magnet_rise = 0;
                }
            }
            return;
        }
        if mv.pseudo_weather.as_deref() == Some("trickroom") {
            self.field.trick_room = if self.field.trick_room > 0 { 0 } else { 5 };
            return;
        }
        if let Some(c) = mv.side_condition.as_deref() {
            let side = if mv.target == Target::FoeSide { 1 - s } else { s };
            let item = self.mon(s, i).item_id;
            let clay = item != u16::MAX && dex.items[item as usize].id == "lightclay";
            let acts_after = self.will_act();
            let sd = &mut self.sides[side];
            match c {
                "stealthrock" => {
                    if sd.stealth_rock {
                        return self.fail();
                    }
                    sd.stealth_rock = true;
                }
                "spikes" => {
                    if sd.spikes >= 3 {
                        return self.fail();
                    }
                    sd.spikes += 1;
                }
                "tailwind" => {
                    if sd.tailwind > 0 {
                        return self.fail();
                    }
                    sd.tailwind = 4;
                }
                "reflect" => {
                    if sd.reflect > 0 {
                        return self.fail();
                    }
                    sd.reflect = if clay { 8 } else { 5 };
                    sd.screen_src[0] = tsrc(s, i);
                    sd.screen_ext[0] = clay;
                }
                "lightscreen" => {
                    if sd.light_screen > 0 {
                        return self.fail();
                    }
                    sd.light_screen = if clay { 8 } else { 5 };
                    sd.screen_src[1] = tsrc(s, i);
                    sd.screen_ext[1] = clay;
                }
                "auroraveil" => {
                    if sd.aurora_veil > 0 || self.weather() != Weather::Snow {
                        return self.fail();
                    }
                    self.sides[side].aurora_veil = if clay { 8 } else { 5 };
                    self.sides[side].screen_src[2] = tsrc(s, i);
                    self.sides[side].screen_ext[2] = clay;
                }
                "wideguard" => {
                    let others_left = acts_after;
                    if !others_left {
                        return self.fail();
                    }
                    sd.wide_guard = true;
                }
                "quickguard" => {
                    let others_left = acts_after;
                    if !others_left {
                        return self.fail();
                    }
                    sd.quick_guard = true;
                }
                "safeguard" => {
                    if sd.safeguard > 0 {
                        return self.fail();
                    }
                    sd.safeguard = 5;
                }
                "toxicspikes" => {
                    if sd.toxic_spikes >= 2 {
                        return self.fail();
                    }
                    sd.toxic_spikes += 1;
                }
                "stickyweb" => {
                    if sd.sticky_web {
                        return self.fail();
                    }
                    sd.sticky_web = true;
                }
                _ => {}
            }
            return;
        }
        match mv.kind {
            Mv::Haze => {
                for side in 0..2 {
                    for k in 0..2 {
                        if let Some(x) = self.active(side, k) {
                            self.mon_mut(side, x).boosts = [0; 8];
                        }
                    }
                }
            }
            Mv::Magicroom | Mv::Wonderroom => {
                let wonder = mv.kind == Mv::Wonderroom;
                let running = if wonder { self.field.wonder_room } else { self.field.magic_room } > 0;
                if running {
                    // used again: the room ends (Showdown onFieldRestart removes it)
                    if wonder {
                        self.field.wonder_room = 0;
                    } else {
                        self.field.magic_room = 0;
                        self.magic_room_end();
                    }
                } else {
                    let turns = if dex.abilities[self.mon(s, i).ability_id as usize].id == "persistent" { 7 } else { 5 };
                    if wonder {
                        self.field.wonder_room = turns;
                    } else {
                        self.field.magic_room = turns;
                        self.magic_room_start();
                    }
                }
            }
            Mv::Fairylock => {
                if self.field.fairy_lock > 0 {
                    self.fail();
                } else {
                    self.field.fairy_lock = 2;
                }
            }
            Mv::Healbell => {
                let mut success = false;
                for k in 0..self.sides[s].mons.len() {
                    let m = self.mon(s, k);
                    if k != i && matches!(m.ability, Ab::Soundproof | Ab::Goodasgold) {
                        continue;
                    }
                    if m.status != Status::None {
                        let m = self.mon_mut(s, k);
                        m.status = Status::None;
                        m.status_turns = 0;
                        success = true;
                    }
                }
                if !success {
                    self.fail();
                }
            }
            Mv::Courtchange => {
                let [sa, sb] = &mut self.sides;
                let any = sa.tailwind + sb.tailwind + sa.reflect + sb.reflect + sa.light_screen + sb.light_screen + sa.aurora_veil
                    + sb.aurora_veil + sa.spikes + sb.spikes + sa.toxic_spikes + sb.toxic_spikes + sa.safeguard + sb.safeguard > 0
                    || sa.stealth_rock || sb.stealth_rock || sa.sticky_web || sb.sticky_web;
                if !any {
                    self.fail();
                } else {
                    std::mem::swap(&mut sa.tailwind, &mut sb.tailwind);
                    std::mem::swap(&mut sa.reflect, &mut sb.reflect);
                    std::mem::swap(&mut sa.light_screen, &mut sb.light_screen);
                    std::mem::swap(&mut sa.aurora_veil, &mut sb.aurora_veil);
                    std::mem::swap(&mut sa.screen_src, &mut sb.screen_src);
                    std::mem::swap(&mut sa.screen_ext, &mut sb.screen_ext);
                    std::mem::swap(&mut sa.spikes, &mut sb.spikes);
                    std::mem::swap(&mut sa.toxic_spikes, &mut sb.toxic_spikes);
                    std::mem::swap(&mut sa.safeguard, &mut sb.safeguard);
                    std::mem::swap(&mut sa.stealth_rock, &mut sb.stealth_rock);
                    std::mem::swap(&mut sa.sticky_web, &mut sb.sticky_web);
                }
            }
            Mv::Teatime => {
                let mut who: ArrayVec<(usize, usize), 4> = ArrayVec::new();
                for side in 0..2 {
                    for k in 0..2 {
                        if let Some(x) = self.active(side, k) {
                            let m = self.mon(side, x);
                            if m.alive() && m.has_item() && dex.items[m.item_id as usize].is_berry && !self.semi_invulnerable(dex, side, x) {
                                who.push((side, x));
                            }
                        }
                    }
                }
                if who.is_empty() {
                    self.fail();
                }
                for (side, x) in who {
                    self.eat_berry_now(side, x);
                }
            }
            Mv::Magneticflux => {
                let mut any = false;
                for k in 0..2 {
                    if let Some(x) = self.active(s, k) {
                        if self.mon(s, x).alive() && matches!(self.mon(s, x).ability, Ab::Plus | Ab::Minus) {
                            let mut b = [0i8; 8];
                            b[DEF] = 1;
                            b[SPD] = 1;
                            any = self.boost(dex, s, x, &b, Some((s, i)), false) || any;
                        }
                    }
                }
                if !any {
                    self.fail();
                }
            }
            Mv::Perishsong => {
                for side in 0..2 {
                    for k in 0..2 {
                        if let Some(x) = self.active(side, k) {
                            // Showdown onHitField: Invulnerability (semi-invulnerable = miss), then TryHit: Soundproof / Good as
                            // Gold block it for everyone but the user (both breakable: Mold Breaker & co.)
                            let other = (side, x) != (s, i);
                            let breaker = matches!(self.mon(s, i).ability, Ab::Moldbreaker | Ab::Teravolt | Ab::Turboblaze);
                            let ab = self.mon(side, x).ability;
                            let blocked = other && !breaker && matches!(ab, Ab::Soundproof | Ab::Goodasgold);
                            let gone = other && self.semi_invulnerable(dex, side, x);
                            let m = self.mon_mut(side, x);
                            if m.perish == 0 && !blocked && !gone && m.alive() {
                                m.perish = 4;
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------
    // End of turn
    // ------------------------------------------------------------------
    /// Future Sight resolves on (ts, slot k): Showdown's futuremove onEnd. The hit uses fixed move data (Psychic, Special,
    /// 120, 100% accurate, not blocked by Protect, which it also removes) and the user's current stats, wherever it is.
    fn future_hit(&mut self, dex: &Dex, ts: usize, k: usize, s: usize, i: usize) {
        if self.sides[0].alive_count() == 0 || self.sides[1].alive_count() == 0 {
            return;
        }
        let Some(ti) = self.active(ts, k) else { return };
        if !self.mon(ts, ti).alive() || (ts, ti) == (s, i) {
            return;
        }
        let Some(idx) = dex.move_index("futuresight") else { return };
        let mv = &dex.moves[idx];
        self.add(|| format!("|-end|{ts}|{ti}|Future Sight"));
        // Protect / Endure stay up (Showdown 2026-10-17, 'Future Sight doesn't bypass Endure'; it used to remove both):
        // the hit goes through Protect anyway (no protect flag), Endure now holds
        self.cur_type = dex.ty.psychic;
        let saved = (self.defer_hits, self.cur_user);
        self.defer_hits = false;
        // a user that is no longer on the field (fainted or switched out) has its ability and item ignored
        // (Showdown ignoringAbility / ignoringItem: gen >= 5 && !isActive), e.g. Levitate no longer stops the terrain boost
        let off_field = self.slot_of(s, i).is_none() || !self.mon(s, i).alive();
        let saved_ab = (self.mon(s, i).ability, self.mon(s, i).item, self.mon(s, i).item_id);
        if off_field {
            let m = self.mon_mut(s, i);
            m.ability = Ab::None;
            m.item = It::None;
            m.item_id = u16::MAX;
        }
        // per-move flags are left over from the last move of the turn: the delayed hit has none of them
        // (Showdown: Parental Bond skips futuremove, no gem, plain base power)
        self.parental = false;
        self.gem_active = false;
        self.fling_bp = 0;
        self.round_boost_now = false;
        self.future_now = true;
        self.analytic_move = None;
        self.red_card = None;
        let (hit, _) = self.hit_target(dex, s, i, mv, ts, ti, false);
        self.future_now = false;
        // Red Card has no futuremove exception (unlike Eject Button): the landing hit drags its user out if it is still active
        if let Some((hs, hi)) = self.red_card.take().map(|(a, b)| (a as usize, b as usize)) {
            if self.mon(s, i).alive() && self.mon(hs, hi).alive() && self.mon(hs, hi).item == It::RedCard && self.slot_of(hs, hi).is_some() {
                let bench: ArrayVec<usize, 6> = self.sides[s].bench().collect();
                if let (false, Some(slot)) = (bench.is_empty(), self.slot_of(s, i)) {
                    self.consume_item(hs, hi);
                    if !(matches!(self.mon(s, i).ability, Ab::Guarddog | Ab::Suctioncups) || self.mon(s, i).vol2 & vol2::INGRAIN != 0) {
                        let to = bench[self.rng.below(bench.len() as u32) as usize];
                        self.do_switch(dex, s, slot, to);
                    }
                }
            }
        }
        if off_field {
            let m = self.mon_mut(s, i);
            if m.ability == Ab::None {
                m.ability = saved_ab.0;
            }
            if m.item_id == u16::MAX {
                m.item = saved_ab.1;
                m.item_id = saved_ab.2;
            }
        }
        (self.defer_hits, self.cur_user) = saved;
        // Life Orb: only if its user is on the field (Showdown runs its AfterMoveSecondarySelf by hand, whether or not the hit
        // landed: an immune / protected target still costs 1/10)
        let _ = hit;
        if self.slot_of(s, i).is_some() && self.mon(s, i).alive() && self.mon(s, i).has_item() && self.mon(s, i).item == It::LifeOrb
            && !self.magic_guard(dex, s, i)
        {
            let amt = self.mon(s, i).maxhp / 10;
            self.hurt(s, i, amt, "lifeorb");
        }
    }

    fn residual_phase(&mut self, dex: &Dex, s: usize, i: usize, phase: u8) {
        match phase {
            0 => {
                let m = self.mon(s, i);
                let ab = m.ability;
                let umbrella = m.has_item() && dex.items[m.item_id as usize].id == "utilityumbrella";
                let w = self.weather();
                if !umbrella && ((ab == Ab::SolarPower || ab == Ab::Dryskin) && w == Weather::Sun) && !self.magic_guard(dex, s, i) {
                    let amt = m.maxhp / 8;
                    self.hurt(s, i, amt, "solarpower");
                } else if !umbrella && ab == Ab::Dryskin && w == Weather::Rain {
                    let amt = m.maxhp / 8;
                    self.heal(s, i, amt, "dryskin");
                } else if !umbrella && ab == Ab::Raindish && w == Weather::Rain {
                    let amt = m.maxhp / 16;
                    self.heal(s, i, amt, "raindish");
                } else if !umbrella && ab == Ab::Icebody && w == Weather::Snow {
                    let amt = (m.maxhp / 16).max(1);
                    self.heal(s, i, amt, "icebody");
                }
            }
            1 => {
                // Shed Skin 33%, Hydration in rain: cure the status (order 5, before poison damage at 9)
                if self.mon(s, i).status != Status::None {
                    let m = self.mon(s, i);
                    let (ab, rain) = (m.ability, self.weather() == Weather::Rain && !(m.has_item() && dex.items[m.item_id as usize].id == "utilityumbrella"));
                    let cure = (ab == Ab::Shedskin && self.rng.chance(33, 100)) || (ab == Ab::Hydration && rain);
                    if cure {
                        let m = self.mon_mut(s, i);
                        m.status = Status::None;
                        m.status_turns = 0;
                    }
                }
                // Healer (residual order 5, before poison damage at 9): 1 in 2 in Champions
                if self.mon(s, i).ability == Ab::Healer {
                    if let Some(k) = self.slot_of(s, i) {
                        if let Some(a) = self.active(s, 1 - k) {
                            if self.mon(s, a).alive() && self.mon(s, a).status != Status::None && self.rng.chance(1, 2) {
                                let m = self.mon_mut(s, a);
                                m.status = Status::None;
                                m.status_turns = 0;
                            }
                        }
                    }
                }
                if self.eot_grassy && self.field.terrain == Terrain::Grassy && self.is_grounded(dex, s, i) && !self.semi_invulnerable(dex, s, i) {
                    let amt = self.mon(s, i).maxhp / 16;
                    self.heal(s, i, amt, "grassyterrain");
                }
                let m = self.mon(s, i);
                if m.alive() && m.has_item() && m.item == It::Leftovers {
                    let amt = m.maxhp / 16;
                    self.heal(s, i, amt, "leftovers");
                }
            }
            2 => {
                if self.mon(s, i).vol2 & vol2::AQUA_RING != 0 {
                    let amt = (self.mon(s, i).maxhp / 16).max(1);
                    self.heal(s, i, amt, "aquaring");
                }
            }
            3 => {
                if self.mon(s, i).vol2 & vol2::INGRAIN != 0 {
                    let amt = (self.mon(s, i).maxhp / 16).max(1);
                    self.heal(s, i, amt, "ingrain");
                }
                if self.mon(s, i).has_vol(vol::LEECH_SEED) && !self.magic_guard(dex, s, i) {
                    let slot = self.mon(s, i).leech_slot as usize;
                    let ls = self.mon(s, i).leech_side as usize;
                    if let Some(src) = (slot < 2).then(|| self.active(ls, slot)).flatten() {
                        let hp0 = self.mon(s, i).hp;
                        let amt = (self.mon(s, i).maxhp / 8).max(1);
                        self.hurt(s, i, amt, "leechseed");
                        let dealt = amt.max(1).min(hp0); // not hp0 - hp: hurt() may eat a Sitrus Berry (u16 underflow)
                        if dealt > 0 && self.mon(ls, src).alive() && self.mon(s, i).ability == Ab::Liquidooze {
                            { let oz = self.ooze_amt(ls, src, dealt); self.hurt(ls, src, oz, "liquidooze"); }
                        } else if dealt > 0 && self.mon(ls, src).alive() && self.mon(ls, src).heal_block == 0 {
                            self.heal(ls, src, dealt, "leechseed");
                        }
                    }
                }
            }
            4 | 5 => {
                // poison / toxic at order 9, burn at 10
                let m = self.mon(s, i);
                let burn = m.status == Status::Burn;
                if (phase == 5) != burn {
                    return;
                }
                let guard = self.magic_guard(dex, s, i);
                let amt = match m.status {
                    Status::Burn => if m.ability == Ab::Heatproof { m.maxhp / 32 } else { m.maxhp / 16 },   // Heatproof: onDamage / 2 (truncated)
                    Status::Poison => m.maxhp / 8,
                    // Showdown: clampIntRange(baseMaxhp / 16, 1) * stage (floored BEFORE multiplying)
                    Status::Toxic => (m.maxhp / 16).max(1) * (m.status_turns + 1) as u16,
                    _ => 0,
                };
                if m.status == Status::Toxic {
                    self.mon_mut(s, i).status_turns = (m.status_turns + 1).min(15);
                }
                if amt > 0 && !guard {
                    let poison_heal = self.mon(s, i).ability == Ab::Poisonheal
                        && matches!(self.mon(s, i).status, Status::Poison | Status::Toxic);
                    if poison_heal {
                        let h = self.mon(s, i).maxhp / 8;
                        self.heal(s, i, h, "poisonheal");
                    } else {
                        let tag = match self.mon(s, i).status { Status::Burn => "brn", Status::Toxic => "tox", _ => "psn" };
                        self.hurt(s, i, amt, tag);
                    }
                }
            }
            _ => {
                if self.mon(s, i).has_vol(vol::CURSE) && !self.magic_guard(dex, s, i) {
                    let amt = (self.mon(s, i).maxhp / 4).max(1);
                    self.hurt(s, i, amt, "curse");
                }
            }
        }
    }

    fn end_of_turn(&mut self, dex: &Dex) {
        // (in_action stays set: Showdown eats residual-time berries at the update after all residual handlers, counter 1,
        // eaten at the next turn's end; the fast engine eats at once, before Cud Chew's own tick, so 2 gives the same turn)
        let mut start_hp = [[0u16; 6]; 2];
        for s in 0..2 {
            for (j, m) in self.sides[s].mons.iter().enumerate() {
                start_hp[s][j] = m.hp;
            }
        }
        self.residual_hp = Some(start_hp);
        self.eot_grassy = self.field.terrain == Terrain::Grassy;
        let mut order = MonList::new();
        for s in 0..2 {
            for k in 0..2 {
                if let Some(i) = self.active(s, k) {
                    order.push((s, i));
                }
            }
        }
        self.sort_by_speed(dex, &mut order);
        // weather
        let w = self.weather();
        if self.field.weather != Weather::None {
            self.field.weather_turns = self.field.weather_turns.saturating_sub(1);
            if self.field.weather_turns == 0 {
                self.field.weather = Weather::None;
                self.forecast_update(dex);
            } else if w == Weather::Sand {
                for &(s, i) in &order {
                    let m = self.mon(s, i);
                    let ab = m.ability;
                    let immune = m.has_type(dex.ty.rock) || m.has_type(dex.ty.ground) || m.has_type(dex.ty.steel)
                        || matches!(ab, Ab::SandForce | Ab::SandRush | Ab::Sandveil | Ab::Overcoat | Ab::Magicguard)
                        || (m.has_item() && m.item == It::SafetyGoggles)
                        || self.underground_or_underwater(dex, s, i);   // Dig / Dive: immune to sandstorm (Showdown onImmunity)
                    if !immune && m.alive() {
                        let amt = m.maxhp / 16;
                        // (Showdown's eachEvent('Weather') is followed by an Update: a Sitrus Berry is eaten right here, before
                        // the other residuals - so Cud Chew's counter sees an empty queue)
                        self.in_action = false;
                        self.hurt(s, i, amt, "sandstorm");
                        self.in_action = true;
                    }
                }
            }
        }
        // Future Sight (residual order 3); several landing in one turn go in the speed order of the Pokemon standing in the
        // target slots (Showdown: the slot condition's effectHolder; random ties; 2026-10-17 targeted fuzz)
        let mut lands: ArrayVec<(i32, u32, usize, usize, usize, usize), 4> = ArrayVec::new();
        for ts in 0..2 {
            for k in 0..2 {
                let (t, us, ui) = self.sides[ts].future[k];
                if t == 0 {
                    continue;
                }
                self.sides[ts].future[k].0 = t - 1;
                if t == 1 {
                    lands.push((0, 0, ts, k, us as usize, ui as usize));
                }
            }
        }
        if lands.len() > 1 {
            for x in lands.iter_mut() {
                x.0 = self.active(x.2, x.3).map_or(i32::MIN, |m| self.action_speed(dex, x.2, m));
                x.1 = self.rng.next_u32();
            }
            lands.sort_unstable_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        }
        for &(_, _, ts, k, us, ui) in lands.iter() {
            self.future_hit(dex, ts, k, us, ui);
        }
        // Wish (residual order 4: before Leftovers & co.)
        for s in 0..2 {
            for k in 0..2 {
                let (t, amt) = self.sides[s].wish[k];
                if t > 0 {
                    self.sides[s].wish[k].0 = t - 1;
                    if t == 1 {
                        if let Some(i) = self.active(s, k) {
                            if self.mon(s, i).alive() {
                                self.heal(s, i, amt, "wish");
                            }
                        }
                    }
                }
            }
        }
        // healing and status damage, one EFFECT at a time over all Pokemon in speed order (Showdown's residual order:
        // weather abilities 1, Grassy Terrain + Leftovers 5, Aqua Ring 6, Leech Seed 8, poison 9, burn 10, Curse 12,
        // Salt Cure + binding 13). Showdown ends the battle the moment a side is out: no residual effects after that.
        // (a side can only run out when something faints: recount only then)
        let faints = |b: &Self| b.sides[0].fainted_total as u32 + b.sides[1].fainted_total as u32;
        let mut over = self.sides[0].alive_count() == 0 || self.sides[1].alive_count() == 0;
        let mut seen = faints(self);
        'phases: for phase in 0..7u8 {
            for &(s, i) in &order {
                if over {
                    break 'phases;
                }
                if !self.mon(s, i).alive() {
                    continue;
                }
                self.residual_phase(dex, s, i, phase);
                if faints(self) != seen {
                    seen = faints(self);
                    over = self.sides[0].alive_count() == 0 || self.sides[1].alive_count() == 0;
                }
            }
        }
        // partial trapping (Infestation, Whirlpool...)
        for &(s, i) in &order {
            if faints(self) != seen {
                seen = faints(self);
                over = self.sides[0].alive_count() == 0 || self.sides[1].alive_count() == 0;
            }
            if over {
                break;
            }
            if self.mon(s, i).vol2 & vol2::SALT_CURE != 0 && self.mon(s, i).alive() && !self.magic_guard(dex, s, i) {
                let m = self.mon(s, i);
                let div = if m.has_type(dex.ty.water) || m.has_type(dex.ty.steel) { 8 } else { 16 };
                let amt = (m.maxhp / div).max(1);
                self.hurt(s, i, amt, "saltcure");
            }
            if self.mon(s, i).alive() && self.mon(s, i).bound > 0 {
                let src = self.mon(s, i).bound_src as usize;
                let bs = self.mon(s, i).bound_side as usize;
                let src_ok = self.sides[bs].active.contains(&(src as u8)) && self.sides[bs].mons[src].alive()
                    && self.sides[bs].mons[src].active_turns > 0;
                if !src_ok {
                    self.mon_mut(s, i).bound = 0;
                    continue;
                }
                if self.mon(s, i).bound & 0x80 != 0 {
                    // (2026-10-28, hidden counters) binding of unknown length N in {5, 6} (0x80 | end-of-turns elapsed e): it ends
                    // at this end of turn iff N = e + 1: P = 0 (e < 4), 1/2 (e = 4), 1 (e >= 5). Only the public count is kept.
                    let e = self.mon(s, i).bound & 0x7f;
                    let ends = match e {
                        0..=3 => false,
                        4 => self.chance_of(1, 2),
                        _ => true,
                    };
                    if !ends {
                        if !self.magic_guard(dex, s, i) {
                            let amt = self.mon(s, i).maxhp / self.mon(s, i).bound_div.max(1) as u16;
                            self.hurt(s, i, amt, "partiallytrapped");
                        }
                        if self.mon(s, i).bound != 0 {
                            self.mon_mut(s, i).bound = 0x80 | (e + 1).min(0x7f);
                        }
                        continue;
                    }
                    self.mon_mut(s, i).bound = 1;          // ends now: the decrement below frees it
                }
                let m = self.mon_mut(s, i);
                m.bound -= 1;
                if m.bound > 0 && !self.magic_guard(dex, s, i) {
                    let amt = self.mon(s, i).maxhp / self.mon(s, i).bound_div.max(1) as u16;
                    self.hurt(s, i, amt, "partiallytrapped");
                }
            }
        }
        // (2026-10-19) a side that ran out in the residuals ends the battle at once (Showdown): no Octolock, Yawn sleep,
        // counters or durations after that (a sandstorm KO of the last foe used to let Yawn still put a Pokemon to sleep)
        if faints(self) != seen {
            over = self.sides[0].alive_count() == 0 || self.sides[1].alive_count() == 0;
        }
        if over {
            self.in_action = false;
            self.check_win();
            if self.winner.is_some() {
                return;
            }
        }
        // Octolock (Showdown residual order 14, right after binding): -1 Def / -1 SpD each turn while its user stays in
        for &(s, i) in &order {
            let src = self.mon(s, i).octolock_src;
            if src == NONE || !self.mon(s, i).alive() {
                continue;
            }
            let os = self.mon(s, i).octolock_side as usize;
            let ok = self.sides[os].active.contains(&src) && self.sides[os].mons[src as usize].alive()
                && self.sides[os].mons[src as usize].active_turns > 0;
            if !ok {
                self.mon_mut(s, i).octolock_src = NONE;
                continue;
            }
            let mut b = Boosts::default();
            b[DEF] = -1;
            b[SPD] = -1;
            self.boost(dex, s, i, &b, Some((os, src as usize)), true);
        }
        // Yawn
        for &(s, i) in &order {
            if self.mon(s, i).alive() && self.mon(s, i).yawn > 0 {
                self.mon_mut(s, i).yawn -= 1;
                if self.mon(s, i).yawn == 0 {
                    self.set_status(dex, s, i, Status::Sleep, None);
                }
            }
        }
        // counters
        for &(s, i) in &order {
            if !self.mon(s, i).alive() {
                continue;
            }
            let ab = self.mon(s, i).ability;
            let m = self.mon_mut(s, i);
            m.taunt = m.taunt.saturating_sub(1);
            m.heal_block = m.heal_block.saturating_sub(1);
            m.throat_chop = m.throat_chop.saturating_sub(1);
            m.magnet_rise = m.magnet_rise.saturating_sub(1);
            if m.encore > 0 {
                m.encore -= 1;
                // Showdown encore onResidual: ends at once when the encored move has no PP left (Eerie Spell, Spite)
                if m.encore_slot != NONE && m.pp[m.encore_slot as usize] == 0 {
                    m.encore = 0;
                }
                if m.encore == 0 {
                    m.encore_slot = NONE;
                }
            }
            if m.disable > 0 {
                m.disable -= 1;
                if m.disable == 0 {
                    m.disable_slot = NONE;
                }
            }
            if ab == Ab::Speedboost && m.active_turns > 0 {
                self.boost(dex, s, i, &[0, 0, 0, 0, 0, 1, 0, 0], None, false);
            }
            if ab == Ab::Moody {
                // +2 to a random stat that can rise, -1 to a different random stat that can fall
                let bo = self.mon(s, i).boosts;
                let ups: ArrayVec<usize, 5> = (1..=5).filter(|&k| bo[k] < 6).collect();
                let up = if ups.is_empty() { 0 } else { ups[self.rng.below(ups.len() as u32) as usize] };
                let downs: ArrayVec<usize, 5> = (1..=5).filter(|&k| bo[k] > -6 && k != up).collect();
                let down = if downs.is_empty() { 0 } else { downs[self.rng.below(downs.len() as u32) as usize] };
                let mut b = [0i8; 8];
                if up > 0 {
                    b[up] = 2;
                }
                if down > 0 {
                    b[down] = -1;
                }
                if up > 0 || down > 0 {
                    self.boost(dex, s, i, &b, Some((s, i)), false);
                }
            }
            if ab == Ab::Pickup && !self.mon(s, i).has_item() {
                let mut c: ArrayVec<(usize, usize), 3> = ArrayVec::new();
                for sd in 0..2 {
                    for k in 0..2 {
                        if let Some(x) = self.active(sd, k) {
                            let t = self.mon(sd, x);
                            if (sd, x) != (s, i) && t.alive() && t.last_item != u16::MAX && t.used_item_turn {
                                c.push((sd, x));
                            }
                        }
                    }
                }
                if !c.is_empty() {
                    let (sd, x) = c[self.rng.below(c.len() as u32) as usize];
                    let id = self.mon(sd, x).last_item;
                    self.mon_mut(sd, x).last_item = u16::MAX;
                    let kind = dex.items[id as usize].kind;
                    let m = self.mon_mut(s, i);
                    m.item_id = id;
                    m.item = kind;
                    m.rev |= rev::ITEM;
                    m.vol &= !vol::UNBURDEN;
                }
            }
            if ab == Ab::Opportunist {
                // (residual order 29: after Speed Boost & co.; the per-Pokemon loop makes it slightly earlier for the faster one)
                let p = self.mon(s, i).opp_pending;
                if p.iter().any(|&x| x != 0) {
                    self.mon_mut(s, i).opp_pending = [0; 8];
                    self.opp_flushing = true;
                    self.boost(dex, s, i, &p, None, false);
                    self.opp_flushing = false;
                }
            }
            if ab == Ab::Harvest && self.mon(s, i).last_item != u16::MAX && crate::dex::item_is_berry(self.mon(s, i).last_item)
                && !self.mon(s, i).has_item() && (self.weather() == Weather::Sun || self.rng.chance(1, 2))
            {
                let id = self.mon(s, i).last_item;
                let kind = dex.items[id as usize].kind;
                let m = self.mon_mut(s, i);
                m.item_id = id;
                m.item = kind;
                m.last_item = u16::MAX;
                m.last_berry_sitrus = false;
                m.rev |= rev::ITEM;
                m.vol &= !vol::UNBURDEN;
                self.check_sitrus(s, i);
            }
            if ab == Ab::Cudchew && self.mon(s, i).cud_chew > 0 {
                self.mon_mut(s, i).cud_chew -= 1;
                if self.mon(s, i).cud_chew == 0 {
                    let id = self.mon(s, i).cud_berry;
                    if id != u16::MAX {
                        self.berry_effect(s, i, id);
                        if self.mon(s, i).ability == Ab::Cheekpouch {
                            let amt = self.mon(s, i).maxhp / 3;
                            self.heal(s, i, amt, "cheekpouch");
                        }
                    }
                }
            }
            if self.mon(s, i).syrup > 0 {
                let src = self.mon(s, i).syrup_src;
                let ss = self.mon(s, i).syrup_side as usize;
                // (a source that faints during this end of turn - poison... - still counts: Showdown removes the volatile only at
                // the next Update, and the residual boost has no source check; 2026-10-17 coverage fuzz)
                let was_alive = src != NONE && (src as usize) < 6 && self.residual_hp.map(|h| h[ss][src as usize] > 0).unwrap_or(false);
                let src_in = src != NONE && self.sides[ss].active.contains(&src) && (self.sides[ss].mons[src as usize].alive() || was_alive);
                let m = self.mon_mut(s, i);
                m.syrup -= 1;
                if !src_in {
                    m.syrup = 0;
                } else if m.syrup > 0 {
                    self.boost(dex, s, i, &[0, 0, 0, 0, 0, -1, 0, 0], Some((ss, src as usize)), false);
                }
            }
            if ab == Ab::Hungerswitch {
                let id = dex.species[self.mon(s, i).species as usize].id.clone();
                if id == "morpeko" {
                    self.forme_change(dex, s, i, "morpekohangry");
                } else if id == "morpekohangry" {
                    self.forme_change(dex, s, i, "morpeko");
                }
            }
            let m = self.mon_mut(s, i);
            if m.perish > 0 {
                m.perish -= 1;
                if m.perish == 0 {
                    self.faint(s, i);
                }
            }
        }
        // Opportunist copies rises that another Pokemon's Speed Boost / Moody made later in this same counters pass
        for &(s, i) in &order {
            if self.mon(s, i).ability == Ab::Opportunist && self.mon(s, i).alive() {
                let p = self.mon(s, i).opp_pending;
                if p.iter().any(|&x| x != 0) {
                    self.mon_mut(s, i).opp_pending = [0; 8];
                    self.opp_flushing = true;
                    self.boost(dex, s, i, &p, None, false);
                    self.opp_flushing = false;
                }
            }
        }
        // Uproar (residual order 28) and Outrage & co. (lockedmove, no order: last)
        for &(s, i) in &order {
            if !self.mon(s, i).alive() || self.slot_of(s, i).is_none() {
                continue;
            }
            if self.mon(s, i).uproar > 0 {
                let m = self.mon_mut(s, i);
                m.uproar -= 1;
                if m.uproar == 0 || m.throat_chop > 0 {
                    m.uproar = 0;
                    m.uproar_slot = NONE;
                }
            }
            if self.mon(s, i).lock_slot != NONE {
                self.mon_mut(s, i).lock_dur -= 1;
                if self.mon(s, i).lock_dur == 0 {
                    self.end_lock(dex, s, i);
                } else {
                    let m = self.mon_mut(s, i);
                    if m.status == Status::Sleep {
                        m.lock_slot = NONE;               // calmed by sleep: no confusion
                        m.lock_dur = 0;
                    }
                    let _ = m;
                    let lt = self.mon(s, i).lock_true;
                    self.mon_mut(s, i).lock_true = if lt & 0x80 != 0 { 0x80 | ((lt & 0x7f) + 1).min(0x7f) } else { lt.saturating_sub(1) };
                }
            }
        }
        // durations
        for s in 0..2 {
            let sd = &mut self.sides[s];
            sd.tailwind = sd.tailwind.saturating_sub(1);
            sd.reflect = sd.reflect.saturating_sub(1);
            sd.light_screen = sd.light_screen.saturating_sub(1);
            sd.aurora_veil = sd.aurora_veil.saturating_sub(1);
            sd.safeguard = sd.safeguard.saturating_sub(1);
            sd.wide_guard = false;
            sd.quick_guard = false;
        }
        self.field.trick_room = self.field.trick_room.saturating_sub(1);
        self.field.gravity = self.field.gravity.saturating_sub(1);
        self.field.wonder_room = self.field.wonder_room.saturating_sub(1);
        self.field.fairy_lock = self.field.fairy_lock.saturating_sub(1);
        if self.field.magic_room > 0 {
            self.field.magic_room -= 1;
            if self.field.magic_room == 0 {
                self.magic_room_end();
            }
        }
        for &(s, i) in &order {
            let m = self.mon_mut(s, i);
            m.lock_on = m.lock_on.saturating_sub(1);
            if m.ally_sw_dur > 0 {
                m.ally_sw_dur -= 1;
                if m.ally_sw_dur == 0 {
                    m.ally_sw = 0;
                }
            }
        }
        if self.field.terrain != Terrain::None {
            self.field.terrain_turns = self.field.terrain_turns.saturating_sub(1);
            if self.field.terrain_turns == 0 {
                self.field.terrain = Terrain::None;
                self.terrain_changed(dex);
            }
        }
        // Emergency Exit after the residuals (Showdown: once per Pokemon, HP now vs HP when the residuals began)
        if let Some(start) = self.residual_hp.take() {
            for &(s, i) in &order {
                let m = self.mon(s, i);
                let half = m.maxhp / 2;
                if m.ability == Ab::EmergencyExit && m.alive() && m.hp <= half && start[s][i] > half && !m.has_vol(vol::SWITCHING_IN) {
                    if let Some(slot) = self.slot_of(s, i) {
                        self.mon_mut(s, i).vol |= vol::SWITCHING_IN;
                        self.request_switch(s, slot);
                    }
                }
            }
        }
        // per-turn effects end
        for s in 0..2 {
            for m in self.sides[s].mons.iter_mut() {
                let protected = m.has_vol(vol::PROTECT) || m.has_vol(vol::ENDURE) || m.guarded;
                if !protected && m.has_vol(vol::MOVED) {
                    m.protect_streak = 0;
                }
                m.guarded = false;
                m.vol &= !(vol::PROTECT | vol::FLINCH | vol::FOLLOW_ME | vol::RAGE_POWDER | vol::HELPING_HAND | vol::ROOST
                    | vol::ENDURE | vol::MOVED | vol::DAMAGED_THIS_TURN | vol::SWITCHING_IN | vol::STATS_RAISED | vol::STATS_LOWERED);
                m.hit_by = 0;
                m.retal_slot = [NONE; 3];
                m.used_item_turn = false;
                m.vol2 &= !(vol2::ELECTRIFY | vol2::LOST_FOCUS | vol2::BEAK_BLAST);
            }
        }
        self.in_action = false;
        // (Showdown runAction: faintMessages / checkWin before the Update: a game that ends in the residuals eats no berry)
        self.check_win();
        if self.winner.is_some() {
            return;
        }
        self.update_items(dex);
        self.check_win();
        if self.winner.is_some() {
            return;
        }
        // replacements for fainted Pokemon
        for s in 0..2 {
            for k in 0..2 {
                let i = self.sides[s].active[k];
                if i != NONE && !self.sides[s].mons[i as usize].alive() {
                    self.request_switch(s, k);
                }
            }
        }
        if self.phase != Phase::Switch {
            self.next_turn();
        } else {
            self.midturn = false;
        }
    }

    // ------------------------------------------------------------------
    // Legal choices
    // ------------------------------------------------------------------
    /// Usable move slots for an active Pokemon (empty = must Struggle).
    pub fn usable_moves(&self, dex: &Dex, s: usize, i: usize) -> ArrayVec<u8, 4> {
        let m = self.mon(s, i);
        // (a move Copycat called continues through CALL_SLOT: offered as slot 0, the lock overrides the choice)
        let shown = |k: u8| if k == CALL_SLOT { 0 } else { k };
        if m.charge_slot != NONE {
            return [shown(m.charge_slot)].into_iter().collect();
        }
        if m.lock_slot != NONE {
            return [shown(m.lock_slot)].into_iter().collect();
        }
        if m.uproar > 0 && m.uproar_slot != NONE {
            return [shown(m.uproar_slot)].into_iter().collect();
        }
        // (2026-10-21) recharging (Hyper Beam & co.): Showdown's request offers only 'recharge' = the forced action (slot 4,
        // index 36, like poke-env); BeforeMove cancels it. Offering the four moves made the search pick e.g. Hyper Beam,
        // which poke-env couldn't map, so the whole turn fell back to the network (arena battle 4645).
        if m.has_vol(vol::MUST_RECHARGE) {
            return ArrayVec::new();
        }
        // Encore: only the encored move, and it still obeys every other restriction (Fake Out later, Taunt...: Struggle)
        let enc_only = if m.encore > 0 && m.encore_slot != NONE && m.pp[m.encore_slot as usize] > 0 { m.encore_slot } else { NONE };
        let mut imprisoned: ArrayVec<u16, 8> = ArrayVec::new();
        for k in 0..2 {
            if let Some(f) = self.active(1 - s, k) {
                let fm = self.mon(1 - s, f);
                if fm.has_vol(vol::IMPRISON) {
                    for &x in fm.moves.iter() {
                        imprisoned.push(x);
                    }
                }
            }
        }
        let mut out: ArrayVec<u8, 4> = ArrayVec::new();
        for k in 0..m.n_moves.min(4) {
            let id = m.moves[k as usize];
            if id == u16::MAX || m.pp[k as usize] == 0 || (enc_only != NONE && k != enc_only) {
                continue;
            }
            let mv = &dex.moves[id as usize];
            let choice = |x: It| matches!(x, It::ChoiceBand | It::ChoiceSpecs | It::ChoiceScarf);
            // (2026-10-18 targeted fuzz 4126820) an item put aside by Klutz / Magic Room is ignored: Showdown choicelock
            // onDisableMove returns on ignoringItem(), so the lock isn't enforced (it stays, and holds again once the item works)
            let blocked = (m.choice_lock != NONE && m.choice_lock != k && choice(m.item))
                || (m.taunt > 0 && mv.category == Category::Status)
                || (m.throat_chop > 0 && mv.flags & flag::SOUND != 0)
                || (m.heal_block > 0 && mv.flags & flag::HEAL != 0)
                || (m.disable > 0 && m.disable_slot == k)
                // Gravity disables its banned moves in the request (Showdown onDisableMove)
                || (self.field.gravity > 0 && matches!(mv.kind, Mv::Fly | Mv::Bounce | Mv::FlyingPress | Mv::Highjumpkick | Mv::Jumpkick | Mv::Skydrop
                    | Mv::Magnetrise | Mv::Telekinesis | Mv::Splash | Mv::Floatyfall))
                || (!imprisoned.is_empty() && imprisoned.contains(&id))
                || (m.item == It::AssaultVest && mv.category == Category::Status)
                || (m.active_move_actions > 0 && mv.first_turn_only)
                // Showdown cantusetwice (Gigaton Hammer, Blood Moon): not selectable right after using it
                || (matches!(mv.kind, Mv::Gigatonhammer | Mv::Bloodmoon) && m.last_move == id)
                || (m.vol2 & vol2::TORMENT != 0 && m.last_move == id && mv.kind != Mv::Struggle);
            if !blocked {
                out.push(k);
            }
        }
        out
    }

    pub fn can_mega(&self, s: usize, i: usize) -> bool {
        !self.sides[s].mega_used && self.mon(s, i).mega_species != u16::MAX && !self.move_locked(s, i)
    }

    /// Showdown getLockedMove: charging a two-turn move, Outrage-style lock, Uproar, recharge. The request then has no
    /// canMegaEvo and chooseMove returns before reading 'mega' (2026-10-19 bridge fuzz 17947: Lucario Mega Evolved mid-Dig)
    pub fn move_locked(&self, s: usize, i: usize) -> bool {
        let m = self.mon(s, i);
        m.charge_slot != NONE || m.lock_slot != NONE || m.uproar > 0 || m.has_vol(vol::MUST_RECHARGE)
    }
}

/// (2026-10-28, hidden timers) a timer's setter as stored in Field::weather_src / Side::screen_src
fn tsrc(s: usize, i: usize) -> u8 {
    1 + (s * 8 + i) as u8
}

impl crate::statehash::HB for Action {
    fn hb(&self, h: &mut crate::statehash::H) {
        match *self {
            Action::Move { slot, target, mega } => {
                h.bits(1, 1);
                h.bits(slot as u64, 1);
                h.bits(target as u8 as u64, 1);
                h.bits(mega as u64, 1);
            }
            Action::Switch { to } => {
                h.bits(2, 1);
                h.bits(to as u64, 1);
            }
            Action::Pass => h.bits(3, 1),
        }
    }
}

impl Battle {
    /// (2026-11-02, endgame solver option `fastkey`) 128-bit hash of everything that can change play from here: both
    /// sides, the field, phase / requests, pending choices and the action queue (only non-empty mid-turn). Per-action
    /// scratch fields (valid only inside one action) and the random-number state are left out. See statehash.rs.
    pub fn state_hash(&self) -> u128 {
        use crate::statehash::{H, HB};
        let mut h = H::new();
        self.sides[0].hb(&mut h);
        self.sides[1].hb(&mut h);
        self.field.hb(&mut h);
        self.turn.hb(&mut h);
        h.bits(self.phase as u64, 1);
        self.winner.hb(&mut h);
        self.switch_needed.hb(&mut h);
        self.reviving.hb(&mut h);
        self.last_faint_side.hb(&mut h);
        self.last_any_move.hb(&mut h);
        self.in_action.hb(&mut h);
        self.called_move.hb(&mut h);
        self.pending.hb(&mut h);
        self.midturn.hb(&mut h);
        self.chosen.hb(&mut h);
        self.residual_hp.hb(&mut h);
        h.bits(self.queue.len() as u64, 2);
        if !self.queue.is_empty() {
            for b in format!("{:?}", self.queue).bytes() {
                h.bits(b as u64, 1);
            }
        }
        h.finish()
    }
}
