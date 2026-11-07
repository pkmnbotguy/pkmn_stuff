//! The Battle object and Showdown's event system (`sim/battle.ts`): runEvent, singleEvent,
//! eachEvent, fieldEvent, handler collection and speedSort, ported line by line so that the
//! same handlers run in the same order and speed ties consume the same random numbers.

use super::data::*;
use super::events::Ev;
use super::prng::Prng;
use super::state::*;
use super::val::{Mon, Val};

/// Showdown's `trunc`: `num >>> 0`
#[inline]
pub fn tr(x: f64) -> f64 {
    if x.is_nan() {
        return 0.0;
    }
    let t = x.trunc();
    if (0.0..4294967296.0).contains(&t) {
        t
    } else {
        (t as i64).rem_euclid(1i64 << 32) as f64
    }
}
/// `trunc(num, bits)`
#[inline]
pub fn tr_bits(x: f64, bits: u32) -> f64 {
    tr(x) % (2f64.powi(bits as i32))
}
/// Utils.clampIntRange
#[inline]
pub fn clamp_int(x: f64, min: Option<f64>, max: Option<f64>) -> f64 {
    let mut n = if x.is_nan() { 0.0 } else { x.floor() };
    if let Some(m) = min {
        if n < m {
            n = m;
        }
    }
    if let Some(m) = max {
        if n > m {
            n = m;
        }
    }
    n
}

/// What `handler.end` does when a Residual duration runs out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EndKind {
    None,
    Noop,
    ClearStatus(Mon),
    RemoveVolatile(Mon, EffId),
    ClearAbility(Mon),
    ClearItem(Mon),
    RemoveSlot(u8, Mon, EffId),
    RemoveSide(u8, EffId),
    RemovePseudo(EffId),
    ClearWeather,
    ClearTerrain,
}

#[derive(Clone, Copy, Debug)]
pub struct Handler {
    pub effect: EffId,
    /// the callback that was looked up (None = only collected for its duration)
    pub cb: Option<CbId>,
    pub state: StateLoc,
    pub state_uid: u32,
    pub holder: Holder,
    pub end: EndKind,
    pub order: Option<f64>,
    pub priority: f64,
    pub speed: f64,
    pub sub_order: f64,
    pub effect_order: f64,
    /// array events: which target this handler belongs to
    pub index: Option<u8>,
    pub target: Option<Mon>,
    /// the sourceEffect's own handler added by runEvent's onEffect
    pub on_effect: bool,
}

/// What a handler receives (Showdown: the handler's arguments plus this.effect / this.effectState).
#[derive(Clone, Copy, Debug)]
pub struct HArgs {
    pub ev: Ev,
    pub relay: Val,
    pub target: Arg,
    pub source: Arg,
    pub effect: EffRef,
    pub state: StateLoc,
    pub holder: Holder,
    pub this_eff: EffId,
    /// this.effect, with the active move index when the running effect is an active move
    pub this_ref: EffRef,
}
impl HArgs {
    pub fn target_mon(&self) -> Mon {
        self.target.mon().expect("handler target is not a Pokemon")
    }
    pub fn source_mon(&self) -> Option<Mon> {
        self.source.mon()
    }
    pub fn holder_mon(&self) -> Mon {
        match self.holder {
            Holder::Mon(m) => m,
            _ => self.target_mon(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct EventCtx {
    pub id: Option<Ev>,
    pub target: Arg,
    pub source: Arg,
    pub effect: EffRef,
    pub modifier: f64,
}
impl EventCtx {
    pub const EMPTY: EventCtx = EventCtx { id: None, target: Arg::None, source: Arg::None, effect: EffRef::NONE, modifier: 1.0 };
}

pub trait Prio {
    fn order(&self) -> Option<f64>;
    fn priority(&self) -> f64;
    fn speed(&self) -> f64;
    fn sub_order(&self) -> f64;
    fn effect_order(&self) -> f64;
}
impl Prio for Handler {
    fn order(&self) -> Option<f64> { self.order }
    fn priority(&self) -> f64 { self.priority }
    fn speed(&self) -> f64 { self.speed }
    fn sub_order(&self) -> f64 { self.sub_order }
    fn effect_order(&self) -> f64 { self.effect_order }
}
impl Prio for Action {
    fn order(&self) -> Option<f64> { if self.order == 0.0 { None } else { Some(self.order) } }
    fn priority(&self) -> f64 { self.priority }
    fn speed(&self) -> f64 { self.speed }
    fn sub_order(&self) -> f64 { 0.0 }
    fn effect_order(&self) -> f64 { 0.0 }
}

/// Battle.comparePriority: negative = a goes first
pub fn compare_priority<T: Prio>(a: &T, b: &T) -> f64 {
    let ao = a.order().filter(|x| *x != 0.0).unwrap_or(4294967296.0);
    let bo = b.order().filter(|x| *x != 0.0).unwrap_or(4294967296.0);
    let d = -(bo - ao);
    if d != 0.0 && !d.is_nan() {
        return d;
    }
    let d = b.priority() - a.priority();
    if d != 0.0 {
        return d;
    }
    let d = b.speed() - a.speed();
    if d != 0.0 {
        return d;
    }
    let d = -(b.sub_order() - a.sub_order());
    if d != 0.0 {
        return d;
    }
    let d = -(b.effect_order() - a.effect_order());
    if d != 0.0 {
        return d;
    }
    0.0
}

pub struct Battle {
    pub d: &'static Data,
    pub sides: [Side; 2],
    pub field: Field,
    pub prng: Prng,
    pub queue: Vec<Action>,
    pub faint_queue: Vec<(Mon, Option<Mon>, EffRef)>,
    pub request_state: RequestState,
    pub turn: i32,
    pub mid_turn: bool,
    pub started: bool,
    pub ended: bool,
    /// Some(Some(side)) = that side won, Some(None) = tie
    pub winner: Option<Option<u8>>,
    pub effect: EffRef,
    pub effect_state: StateLoc,
    pub event: EventCtx,
    pub event_depth: u32,
    pub active_move: Option<u16>,
    pub active_pokemon: Option<Mon>,
    pub active_target: Option<Mon>,
    pub last_move: Option<u16>,
    /// set by use_move_random for the next use_move_inner: the target is undefined (not null)
    pub use_move_undef_target: bool,
    pub last_successful_move_this_turn: Option<u16>,
    pub effect_order: f64,
    pub speed_order: Vec<u8>,
    /// active moves of this turn
    pub moves: Vec<ActiveMove>,
    pub temp_states: Vec<EffectState>,
    pub log: Option<Vec<String>>,
    pub last_move_line: i64,
    pub uid: u32,
    /// unimplemented handlers that were reached (should stay empty)
    pub missing: Vec<String>,
    pub hints: Vec<String>,
}

impl Clone for Battle {
    fn clone(&self) -> Battle {
        Battle {
            d: self.d,
            sides: self.sides.clone(),
            field: self.field.clone(),
            prng: self.prng.clone(),
            queue: self.queue.clone(),
            faint_queue: self.faint_queue.clone(),
            request_state: self.request_state,
            turn: self.turn,
            mid_turn: self.mid_turn,
            started: self.started,
            ended: self.ended,
            winner: self.winner,
            effect: self.effect,
            effect_state: self.effect_state,
            event: self.event,
            event_depth: self.event_depth,
            active_move: self.active_move,
            active_pokemon: self.active_pokemon,
            active_target: self.active_target,
            last_move: self.last_move,
            use_move_undef_target: self.use_move_undef_target,
            last_successful_move_this_turn: self.last_successful_move_this_turn,
            effect_order: self.effect_order,
            speed_order: self.speed_order.clone(),
            moves: self.moves.clone(),
            temp_states: self.temp_states.clone(),
            log: self.log.clone(),
            last_move_line: self.last_move_line,
            uid: self.uid,
            missing: vec![],
            hints: self.hints.clone(),
        }
    }
}

/// Things that can be written into the battle log.
pub trait LogArg {
    fn to_log(&self, b: &Battle) -> String;
}
impl LogArg for &str {
    fn to_log(&self, _: &Battle) -> String { self.to_string() }
}
impl LogArg for String {
    fn to_log(&self, _: &Battle) -> String { self.clone() }
}
impl LogArg for i32 {
    fn to_log(&self, _: &Battle) -> String { self.to_string() }
}
impl LogArg for f64 {
    fn to_log(&self, _: &Battle) -> String { js_num(*self) }
}
impl LogArg for Mon {
    fn to_log(&self, b: &Battle) -> String { b.mon_str(*self) }
}
/// `pokemon.getHealth` (the exact-HP form that battle.log keeps)
pub struct Health(pub Mon);
impl LogArg for Health {
    fn to_log(&self, b: &Battle) -> String { b.health(self.0) }
}
/// `pokemon.getFullDetails`
pub struct FullDetails(pub Mon);
impl LogArg for FullDetails {
    fn to_log(&self, b: &Battle) -> String {
        let p = b.p(self.0);
        let details = match p.illusion {
            Some(il) => b.updated_details_level(il, p.level),
            None => p.details.clone(),
        };
        format!("{}|{}", details, b.health(self.0))
    }
}

pub fn js_num(x: f64) -> String {
    if x == x.trunc() && x.abs() < 1e15 { format!("{}", x as i64) } else { format!("{x}") }
}

#[macro_export]
macro_rules! add {
    ($b:expr, $($p:expr),* $(,)?) => {
        if $b.log.is_some() {
            let parts: Vec<String> = vec![$( $crate::sim::battle::LogArg::to_log(&$p, &*$b) ),*];
            $b.push_log(parts);
        }
    };
}

impl Battle {
    // ------------------------------------------------------------------
    // basic access
    // ------------------------------------------------------------------
    #[inline]
    pub fn p(&self, m: Mon) -> &Pokemon {
        &self.sides[m.s as usize].mons[m.i as usize]
    }
    #[inline]
    pub fn pm(&mut self, m: Mon) -> &mut Pokemon {
        &mut self.sides[m.s as usize].mons[m.i as usize]
    }
    #[inline]
    pub fn eff(&self, e: EffId) -> &'static Effect {
        &self.d.effects[e as usize]
    }
    #[inline]
    pub fn mv(&self, r: u16) -> &ActiveMove {
        &self.moves[r as usize]
    }
    #[inline]
    pub fn mvm(&mut self, r: u16) -> &mut ActiveMove {
        &mut self.moves[r as usize]
    }
    pub fn move_def(&self, r: u16) -> &'static MoveDef {
        &self.d.moves[self.moves[r as usize].def as usize]
    }
    pub fn move_id_str(&self, r: u16) -> &'static str {
        &self.d.moves[self.moves[r as usize].def as usize].id
    }
    pub fn next_uid(&mut self) -> u32 {
        self.uid += 1;
        self.uid
    }
    /// Pokemon in side.active[pos]
    pub fn active_at(&self, s: usize, pos: usize) -> Option<Mon> {
        self.sides[s].active[pos].map(|i| Mon { s: s as u8, i })
    }

    // ------------------------------------------------------------------
    // log
    // ------------------------------------------------------------------
    pub fn push_log(&mut self, parts: Vec<String>) {
        if let Some(log) = &mut self.log {
            log.push(format!("|{}", parts.join("|")));
            self.prng.log_len = log.len() as u32;
        }
    }
    pub fn mon_str(&self, m: Mon) -> String {
        let p = self.p(m);
        let name = match p.illusion {
            Some(il) => &self.p(il).name,
            None => &p.name,
        };
        if p.is_active {
            format!("p{}{}: {}", m.s + 1, if p.position == 0 { 'a' } else { 'b' }, name)
        } else {
            format!("p{}: {}", m.s + 1, name)
        }
    }
    pub fn health(&self, m: Mon) -> String {
        let p = self.p(m);
        if p.hp == 0 {
            return "0 fnt".into();
        }
        let mut s = format!("{}/{}", p.hp, p.maxhp);
        if p.status != self.d.c.empty {
            s.push(' ');
            s.push_str(&self.eff(p.status).id);
        }
        s
    }
    pub fn slot_str(&self, m: Mon) -> String {
        let p = self.p(m);
        format!("p{}{}", m.s + 1, if p.position == 0 { 'a' } else { 'b' })
    }
    /// addMove: remembers the line so attrLastMove can amend it
    pub fn add_move(&mut self, parts: Vec<String>) {
        if let Some(log) = &mut self.log {
            self.last_move_line = log.len() as i64;
            log.push(format!("|{}", parts.join("|")));
            self.prng.log_len = log.len() as u32;
        }
    }
    pub fn attr_last_move(&mut self, args: &[&str]) {
        if self.last_move_line < 0 {
            return;
        }
        let Some(log) = &mut self.log else { return };
        let i = self.last_move_line as usize;
        if i >= log.len() {
            return;
        }
        if log[i].starts_with("|-anim|") {
            if args.contains(&"[still]") {
                log.remove(i);
                self.last_move_line = -1;
                return;
            }
        } else if args.contains(&"[still]") {
            let mut parts: Vec<String> = log[i].split('|').map(String::from).collect();
            if parts.len() > 4 {
                parts[4] = String::new();
            }
            log[i] = parts.join("|");
        }
        log[i].push('|');
        log[i].push_str(&args.join("|"));
    }
    pub fn retarget_last_move(&mut self, m: Mon) {
        if self.last_move_line < 0 || self.log.is_none() {
            return;
        }
        let s = self.mon_str(m);
        let log = self.log.as_mut().unwrap();
        let i = self.last_move_line as usize;
        let mut parts: Vec<String> = log[i].split('|').map(String::from).collect();
        if parts.len() > 4 {
            parts[4] = s;
        }
        log[i] = parts.join("|");
    }
    pub fn hint(&mut self, h: &str, once: bool) {
        if self.hints.iter().any(|x| x == h) {
            return;
        }
        add!(self, "-hint", h);
        if once {
            self.hints.push(h.to_string());
        }
    }

    // ------------------------------------------------------------------
    // random numbers
    // ------------------------------------------------------------------
    pub fn random(&mut self, n: u32) -> u32 {
        self.prng.random(n)
    }
    pub fn random_range(&mut self, m: u32, n: u32) -> u32 {
        self.prng.random_range(m, n)
    }
    pub fn random_chance(&mut self, num: u32, den: u32) -> bool {
        self.prng.chance(num, den)
    }

    /// Battle.speedSort: selection sort, speed ties shuffled with the PRNG
    pub fn speed_sort<T: Prio>(&mut self, list: &mut [T]) {
        self.speed_sort_by(list, compare_priority::<T>);
    }
    pub fn speed_sort_by<T>(&mut self, list: &mut [T], cmp: fn(&T, &T) -> f64) {
        if list.len() < 2 {
            return;
        }
        let mut sorted = 0;
        let mut next: Vec<usize> = Vec::with_capacity(list.len());
        while sorted + 1 < list.len() {
            next.clear();
            next.push(sorted);
            for i in sorted + 1..list.len() {
                let delta = cmp(&list[next[0]], &list[i]);
                if delta < 0.0 {
                    continue;
                }
                if delta > 0.0 {
                    next.clear();
                    next.push(i);
                } else {
                    next.push(i);
                }
            }
            for k in 0..next.len() {
                let index = next[k];
                if index != sorted + k {
                    list.swap(sorted + k, index);
                }
            }
            if next.len() > 1 {
                let end = sorted + next.len();
                self.prng.shuffle(list, sorted, end);
            }
            sorted += next.len();
        }
    }

    // ------------------------------------------------------------------
    // effect states
    // ------------------------------------------------------------------
    /// initEffectState: effectOrder is counted only for states with an id and a target that
    /// is not an inactive Pokemon
    pub fn init_state(&mut self, id: EffId, target: Holder, explicit_order: Option<f64>) -> EffectState {
        let mut st = EffectState { id, uid: self.next_uid(), ..Default::default() };
        let has_id = id != NO_EFF && id != self.d.c.empty;
        match target {
            Holder::Mon(m) => {
                st.target_set = true;
                st.target_mon = Some(m);
            }
            Holder::Side(s) => {
                st.target_set = true;
                st.target_side = Some(s);
            }
            Holder::Field => st.target_set = true,
            _ => {}
        }
        if let Some(o) = explicit_order {
            st.effect_order = o;
        } else if has_id && target != Holder::None && !matches!(target, Holder::Mon(m) if !self.p(m).is_active) {
            st.effect_order = self.effect_order;
            self.effect_order += 1.0;
        } else {
            st.effect_order = 0.0;
        }
        st
    }
    /// clearEffectState
    pub fn clear_state(st: &mut EffectState, empty: EffId) {
        st.id = empty;
        st.effect_order = 0.0;
        st.duration = None;
        st.source = None;
        st.source_slot = None;
        st.source_effect = None;
        st.is_slot_condition = false;
        st.linked.clear();
        st.linked_status = None;
        st.vals.clear();
    }

    pub fn state(&self, loc: StateLoc) -> Option<&EffectState> {
        match loc {
            StateLoc::None => None,
            StateLoc::Status(m) => Some(&self.p(m).status_state),
            StateLoc::Volatile(m, e) => self.p(m).volatiles.iter().find(|x| x.0 == e).map(|x| &x.1),
            StateLoc::Ability(m) => Some(&self.p(m).ability_state),
            StateLoc::Item(m) => Some(&self.p(m).item_state),
            StateLoc::Species(m) => Some(&self.p(m).species_state),
            StateLoc::Slot(s, pos, e) => self.sides[s as usize].slot_conditions[pos as usize].iter().find(|x| x.0 == e).map(|x| &x.1),
            StateLoc::SideCond(s, e) => self.sides[s as usize].side_conditions.iter().find(|x| x.0 == e).map(|x| &x.1),
            StateLoc::Pseudo(e) => self.field.pseudo_weather.iter().find(|x| x.0 == e).map(|x| &x.1),
            StateLoc::Weather => Some(&self.field.weather_state),
            StateLoc::Terrain => Some(&self.field.terrain_state),
            StateLoc::Temp(i) => self.temp_states.get(i as usize),
        }
    }
    pub fn state_mut(&mut self, loc: StateLoc) -> Option<&mut EffectState> {
        match loc {
            StateLoc::None => None,
            StateLoc::Status(m) => Some(&mut self.pm(m).status_state),
            StateLoc::Volatile(m, e) => self.pm(m).volatiles.iter_mut().find(|x| x.0 == e).map(|x| &mut x.1),
            StateLoc::Ability(m) => Some(&mut self.pm(m).ability_state),
            StateLoc::Item(m) => Some(&mut self.pm(m).item_state),
            StateLoc::Species(m) => Some(&mut self.pm(m).species_state),
            StateLoc::Slot(s, pos, e) => self.sides[s as usize].slot_conditions[pos as usize].iter_mut().find(|x| x.0 == e).map(|x| &mut x.1),
            StateLoc::SideCond(s, e) => self.sides[s as usize].side_conditions.iter_mut().find(|x| x.0 == e).map(|x| &mut x.1),
            StateLoc::Pseudo(e) => self.field.pseudo_weather.iter_mut().find(|x| x.0 == e).map(|x| &mut x.1),
            StateLoc::Weather => Some(&mut self.field.weather_state),
            StateLoc::Terrain => Some(&mut self.field.terrain_state),
            StateLoc::Temp(i) => self.temp_states.get_mut(i as usize),
        }
    }
    /// read a field of a handler's state
    pub fn sget(&self, loc: StateLoc, k: K) -> Val {
        self.state(loc).map(|s| s.get(k)).unwrap_or(Val::Undef)
    }
    pub fn sset(&mut self, loc: StateLoc, k: K, v: Val) {
        if let Some(s) = self.state_mut(loc) {
            s.set(k, v);
        }
    }
    pub fn volatile(&self, m: Mon, e: EffId) -> Option<&EffectState> {
        self.p(m).volatiles.iter().find(|x| x.0 == e).map(|x| &x.1)
    }
    pub fn volatile_mut(&mut self, m: Mon, e: EffId) -> Option<&mut EffectState> {
        self.pm(m).volatiles.iter_mut().find(|x| x.0 == e).map(|x| &mut x.1)
    }
    pub fn has_volatile(&self, m: Mon, e: EffId) -> bool {
        e != NO_EFF && self.p(m).volatiles.iter().any(|x| x.0 == e)
    }
    pub fn has_vol(&self, m: Mon, id: &str) -> bool {
        match self.d.cond(id) {
            Some(e) => self.has_volatile(m, e),
            None => false,
        }
    }
    pub fn cond(&self, id: &str) -> EffId {
        self.d.cond(id).unwrap_or_else(|| panic!("unknown condition {id}"))
    }

    // ------------------------------------------------------------------
    // handler collection (findEventHandlers & co)
    // ------------------------------------------------------------------
    fn get_callback(&self, holder_is_mon: bool, effect: EffId, cb: CbId, cb_switch_in: bool) -> Option<CbId> {
        let e = self.eff(effect);
        if e.has(cb) {
            return Some(cb);
        }
        // abilities and items run their onStart during the SwitchIn event (gen 5+)
        if holder_is_mon && cb_switch_in && matches!(e.kind, Kind::Ability | Kind::Item) {
            let any = self.d.cbs(Ev::SwitchIn)[P_ANY];
            if !e.has(any) {
                let start = self.d.cbs(Ev::Start)[P_ON];
                if e.has(start) {
                    return Some(start);
                }
            }
        }
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn resolve_priority(&self, effect: EffId, cb: Option<CbId>, cbname_cb: CbId, state: StateLoc, holder: Holder, end: EndKind, state_uid: u32) -> Handler {
        let e = self.eff(effect);
        // Order/Priority/SubOrder come from the callback name that was asked for (callbackName)
        let meta = e.handler(cbname_cb);
        let mut h = Handler {
            effect,
            cb,
            state,
            state_uid,
            holder,
            end,
            order: meta.and_then(|m| m.order).filter(|o| *o != 0.0),
            priority: meta.map(|m| m.priority).unwrap_or(0.0),
            speed: 0.0,
            sub_order: meta.map(|m| m.sub_order).unwrap_or(0.0),
            effect_order: 0.0,
            index: None,
            target: None,
            on_effect: false,
        };
        // when onSwitchIn falls back to onStart, the meta (priority etc.) is that of onSwitchIn (absent)
        if h.sub_order == 0.0 {
            h.sub_order = match e.effect_type {
                EffType::Condition => {
                    let st = self.state(state);
                    let target_side = st.map(|s| s.target_set && s.target_side.is_some()).unwrap_or(false);
                    let target_field = st.map(|s| s.target_set && matches!(state, StateLoc::Pseudo(_) | StateLoc::Weather | StateLoc::Terrain)).unwrap_or(false);
                    if target_side {
                        if st.map(|s| s.is_slot_condition).unwrap_or(false) { 3.0 } else { 4.0 }
                    } else if target_field {
                        5.0
                    } else {
                        2.0
                    }
                }
                EffType::Weather => 5.0,
                EffType::Ability => match e.id.as_str() {
                    "poisontouch" | "perishbody" => 6.0,
                    "stall" => 9.0,
                    _ => 7.0,
                },
                EffType::Item => 8.0,
                _ => 0.0,
            };
        }
        let cbname = self.d.cb_name(cbname_cb);
        if cbname.ends_with("SwitchIn") || cbname.ends_with("RedirectTarget") {
            h.effect_order = self.state(state).map(|s| s.effect_order).unwrap_or(0.0);
        }
        if let Holder::Mon(m) = holder {
            h.speed = self.p(m).speed;
            if e.id == "magicbounce" && cbname == "onAllyTryHitSide" {
                // getStat(spe, unboosted, unmodified) = the stored stat
                h.speed = self.p(m).stored_stats[5] as f64;
            }
            if cbname.ends_with("SwitchIn") {
                let fpv = m.s + 2 * self.p(m).position;
                let idx = self.speed_order.iter().position(|&x| x == fpv).map(|x| x as f64).unwrap_or(-1.0);
                h.speed -= idx / 4.0;
            }
        }
        h
    }

    /// findPokemonEventHandlers
    pub fn find_pokemon_handlers(&self, m: Mon, cb: CbId, get_duration: bool, out: &mut Vec<Handler>) {
        let switch_in = self.d.cb_name(cb) == "onSwitchIn";
        let p = self.p(m);
        let hold = Holder::Mon(m);
        // status
        let status = p.status;
        let c = self.get_callback(true, status, cb, switch_in);
        if c.is_some() || (get_duration && p.status_state.duration.map(|x| x != 0.0).unwrap_or(false)) {
            out.push(self.resolve_priority(status, c, cb, StateLoc::Status(m), hold, EndKind::ClearStatus(m), p.status_state.uid));
        }
        for (vid, vst) in &p.volatiles {
            let c = self.get_callback(true, *vid, cb, switch_in);
            if c.is_some() || (get_duration && vst.duration.map(|x| x != 0.0).unwrap_or(false)) {
                out.push(self.resolve_priority(*vid, c, cb, StateLoc::Volatile(m, *vid), hold, EndKind::RemoveVolatile(m, *vid), vst.uid));
            }
        }
        let ab = p.ability;
        let c = self.get_callback(true, ab, cb, switch_in);
        if c.is_some() || (get_duration && p.ability_state.duration.map(|x| x != 0.0).unwrap_or(false)) {
            out.push(self.resolve_priority(ab, c, cb, StateLoc::Ability(m), hold, EndKind::ClearAbility(m), p.ability_state.uid));
        }
        let it = p.item;
        let c = self.get_callback(true, it, cb, switch_in);
        if c.is_some() || (get_duration && p.item_state.duration.map(|x| x != 0.0).unwrap_or(false)) {
            out.push(self.resolve_priority(it, c, cb, StateLoc::Item(m), hold, EndKind::ClearItem(m), p.item_state.uid));
        }
        // species handlers: none of the format's species have any
        let side = &self.sides[m.s as usize];
        let pos = p.position as usize;
        if pos < 2 {
            for (cid, cst) in &side.slot_conditions[pos] {
                let c = self.get_callback(true, *cid, cb, switch_in);
                if c.is_some() || (get_duration && cst.duration.map(|x| x != 0.0).unwrap_or(false)) {
                    out.push(self.resolve_priority(*cid, c, cb, StateLoc::Slot(m.s, pos as u8, *cid), hold, EndKind::RemoveSlot(m.s, m, *cid), cst.uid));
                }
            }
        }
    }

    /// findFieldEventHandlers
    pub fn find_field_handlers(&self, cb: CbId, get_duration: bool, custom: Option<Mon>, out: &mut Vec<Handler>) {
        let hold = match custom {
            Some(m) => Holder::Mon(m),
            None => Holder::Field,
        };
        for (pid, pst) in &self.field.pseudo_weather {
            let c = self.get_callback(false, *pid, cb, false);
            if c.is_some() || (get_duration && pst.duration.map(|x| x != 0.0).unwrap_or(false)) {
                let end = if custom.is_some() { EndKind::None } else { EndKind::RemovePseudo(*pid) };
                out.push(self.resolve_priority(*pid, c, cb, StateLoc::Pseudo(*pid), hold, end, pst.uid));
            }
        }
        let w = self.field.weather;
        let c = self.get_callback(false, w, cb, false);
        if c.is_some() || (get_duration && self.field.weather_state.duration.map(|x| x != 0.0).unwrap_or(false)) {
            let end = if custom.is_some() { EndKind::None } else { EndKind::ClearWeather };
            out.push(self.resolve_priority(w, c, cb, StateLoc::Weather, hold, end, self.field.weather_state.uid));
        }
        let t = self.field.terrain;
        let c = self.get_callback(false, t, cb, false);
        if c.is_some() || (get_duration && self.field.terrain_state.duration.map(|x| x != 0.0).unwrap_or(false)) {
            let end = if custom.is_some() { EndKind::None } else { EndKind::ClearTerrain };
            out.push(self.resolve_priority(t, c, cb, StateLoc::Terrain, hold, end, self.field.terrain_state.uid));
        }
    }

    /// findSideEventHandlers
    pub fn find_side_handlers(&self, s: u8, cb: CbId, get_duration: bool, custom: Option<Mon>, out: &mut Vec<Handler>) {
        let hold = match custom {
            Some(m) => Holder::Mon(m),
            None => Holder::Side(s),
        };
        for (cid, cst) in &self.sides[s as usize].side_conditions {
            let c = self.get_callback(false, *cid, cb, false);
            if c.is_some() || (get_duration && cst.duration.map(|x| x != 0.0).unwrap_or(false)) {
                let end = if custom.is_some() { EndKind::None } else { EndKind::RemoveSide(s, *cid) };
                out.push(self.resolve_priority(*cid, c, cb, StateLoc::SideCond(s, *cid), hold, end, cst.uid));
            }
        }
    }

    /// side.allies(): active, non-null, hp > 0 (in position order)
    pub fn allies_and_self(&self, s: u8) -> Vec<Mon> {
        let side = &self.sides[s as usize];
        side.active.iter().flatten().map(|&i| Mon { s, i }).filter(|&m| self.p(m).hp > 0).collect()
    }
    pub fn allies_and_self_all(&self, s: u8) -> Vec<Mon> {
        self.sides[s as usize].active.iter().flatten().map(|&i| Mon { s, i }).collect()
    }
    pub fn foes_of(&self, s: u8) -> Vec<Mon> {
        self.allies_and_self(1 - s)
    }
    pub fn allies(&self, m: Mon) -> Vec<Mon> {
        self.allies_and_self(m.s).into_iter().filter(|&x| x != m).collect()
    }

    /// findEventHandlers for one target
    pub fn find_event_handlers(&self, target: Arg, ev: Ev, source: Option<Mon>, out: &mut Vec<Handler>) {
        let cbs = *self.d.cbs(ev);
        let should_bubble_down = matches!(target, Arg::Side(_));
        let prefixed = ev.prefixed();
        let mut tgt = target;
        if let Arg::Mon(m) = target {
            let src_active = source.map(|s| self.p(s).is_active).unwrap_or(false);
            if self.p(m).is_active || src_active {
                self.find_pokemon_handlers(m, cbs[P_ON], false, out);
                if prefixed {
                    for a in self.allies_and_self(m.s) {
                        self.find_pokemon_handlers(a, cbs[P_ALLY], false, out);
                        self.find_pokemon_handlers(a, cbs[P_ANY], false, out);
                    }
                    for f in self.foes_of(m.s) {
                        self.find_pokemon_handlers(f, cbs[P_FOE], false, out);
                        self.find_pokemon_handlers(f, cbs[P_ANY], false, out);
                    }
                }
                tgt = Arg::Side(m.s);
            }
        }
        if let Some(src) = source {
            if prefixed {
                self.find_pokemon_handlers(src, cbs[P_SOURCE], false, out);
            }
        }
        if let Arg::Side(ts) = tgt {
            for s in 0..2u8 {
                if should_bubble_down {
                    for pos in 0..2 {
                        let Some(i) = self.sides[s as usize].active[pos] else { continue };
                        let a = Mon { s, i };
                        if s == ts {
                            self.find_pokemon_handlers(a, cbs[P_ON], false, out);
                        } else if prefixed {
                            self.find_pokemon_handlers(a, cbs[P_FOE], false, out);
                        }
                        if prefixed {
                            self.find_pokemon_handlers(a, cbs[P_ANY], false, out);
                        }
                    }
                }
                if s == ts {
                    self.find_side_handlers(s, cbs[P_ON], false, None, out);
                } else if prefixed {
                    self.find_side_handlers(s, cbs[P_FOE], false, None, out);
                }
                if prefixed {
                    self.find_side_handlers(s, cbs[P_ANY], false, None, out);
                }
            }
        }
        self.find_field_handlers(cbs[P_ON], false, None, out);
        // no format / battle handlers in this format
    }

    // ------------------------------------------------------------------
    // running handlers
    // ------------------------------------------------------------------
    /// Call one handler. `cb` is the callback that was found.
    fn call_handler(&mut self, effect: EffId, cb: CbId, args: &HArgs) -> Val {
        let meta = self.eff(effect).handler(cb);
        match meta {
            Some(m) => match &m.kind {
                HandlerKind::Value(v) => self.handler_value(effect, cb, *v),
                HandlerKind::MetaOnly => Val::Undef,
                HandlerKind::Fn => match m.imp {
                    Some(f) => f(self, args),
                    None => {
                        let msg = format!("{} {} {}", self.eff(effect).id, self.d.cb_name(cb), args.ev.name());
                        if !self.missing.contains(&msg) {
                            self.missing.push(msg);
                        }
                        Val::Undef
                    }
                },
            },
            None => Val::Undef,
        }
    }
    fn handler_value(&self, effect: EffId, cb: CbId, v: Val) -> Val {
        if let Val::Key("string") = v {
            // string handler values that get dispatched: onLockMove = a move id ('recharge' for mustrecharge, else the
            // condition's own move: uproar, rollout, iceball, bide)
            let _ = cb;
            let id = &self.eff(effect).id;
            if id == "mustrecharge" {
                return Val::MoveId(self.d.c.recharge_move);
            }
            if let Some(m) = self.d.move_id(id) {
                return Val::MoveId(m);
            }
            return Val::MoveId(self.d.c.recharge_move);
        }
        v
    }

    /// Showdown's `suppressingAbility(target)`
    pub fn suppressing_ability(&self, target: Option<Mon>) -> bool {
        let Some(ap) = self.active_pokemon else { return false };
        let Some(am) = self.active_move else { return false };
        self.p(ap).is_active
            && (Some(ap) != target)
            && self.mv(am).ignore_ability
            && !target.map(|t| self.has_item(t, "abilityshield")).unwrap_or(false)
    }

    /// runEvent(eventid, target, source, effect, relayVar)
    pub fn run_event(&mut self, ev: Ev, target: Arg, source: Option<Mon>, effect: EffRef, relay: Val) -> Val {
        self.run_event_full(ev, target, Arg::from_opt(source), effect, relay, false, false)
    }
    pub fn priority_event(&mut self, ev: Ev, target: Arg, source: Option<Mon>, effect: EffRef, relay: Val) -> Val {
        self.run_event_full(ev, target, Arg::from_opt(source), effect, relay, false, true)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn run_event_full(&mut self, ev: Ev, target: Arg, source: Arg, effect: EffRef, relay: Val, on_effect: bool, fast_exit: bool) -> Val {
        let target = if target == Arg::None { Arg::Battle } else { target };
        let effect_source = source.mon();
        let mut handlers: Vec<Handler> = Vec::new();
        self.find_event_handlers(target, ev, effect_source, &mut handlers);
        if on_effect && !effect.is_none() {
            let cb = self.d.cbs(ev)[P_ON];
            if self.eff(effect.eff).has(cb) {
                let holder = match target {
                    Arg::Mon(m) => Holder::Mon(m),
                    Arg::Side(s) => Holder::Side(s),
                    _ => Holder::Battle,
                };
                let mut h = self.resolve_priority(effect.eff, Some(cb), cb, StateLoc::None, holder, EndKind::None, 0);
                h.on_effect = true;
                handlers.insert(0, h);
            }
        }
        self.sort_handlers(ev, &mut handlers, fast_exit);
        self.run_handlers(ev, target, source, effect, relay, handlers, fast_exit)
    }

    fn sort_handlers(&mut self, ev: Ev, handlers: &mut [Handler], fast_exit: bool) {
        if matches!(ev, Ev::Invulnerability | Ev::TryHit | Ev::DamagingHit | Ev::EntryHazard) {
            handlers.sort_by(|a, b| {
                let d = compare_left_to_right(a, b);
                d.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal)
            });
        } else if fast_exit {
            let d = self;
            let keyed: Vec<f64> = handlers.iter().map(|h| match h.holder {
                Holder::Mon(m) => d.p(m).ability_state.effect_order,
                _ => f64::NAN,
            }).collect();
            let mut idx: Vec<usize> = (0..handlers.len()).collect();
            idx.sort_by(|&a, &b| {
                let (ha, hb) = (&handlers[a], &handlers[b]);
                let mut r = hb.priority - ha.priority;
                if r == 0.0 {
                    r = hb.speed - ha.speed;
                }
                if r == 0.0 && !keyed[a].is_nan() && !keyed[b].is_nan() {
                    r = -(keyed[b] - keyed[a]);
                }
                r.partial_cmp(&0.0).unwrap_or(std::cmp::Ordering::Equal)
            });
            let copy: Vec<Handler> = idx.iter().map(|&i| handlers[i]).collect();
            handlers.copy_from_slice(&copy);
        } else {
            self.speed_sort(handlers);
        }
    }

    /// the handler loop of runEvent (single target)
    #[allow(clippy::too_many_arguments)]
    fn run_handlers(&mut self, ev: Ev, target: Arg, source: Arg, effect: EffRef, relay: Val, handlers: Vec<Handler>, fast_exit: bool) -> Val {
        let mut relay_var = relay;
        let has_relay = !matches!(relay, Val::Undef | Val::Null);
        if !has_relay {
            relay_var = Val::True;
        }
        let parent_event = self.event;
        self.event = EventCtx { id: Some(ev), target, source, effect, modifier: 1.0 };
        self.event_depth += 1;
        for h in handlers {
            if !self.handler_applies(ev, &h, effect) {
                continue;
            }
            let ret = match h.cb {
                Some(cb) => {
                    let parent_effect = self.effect;
                    let parent_state = self.effect_state;
                    self.effect = EffRef::eff(h.effect);
                    self.effect_state = h.state;
                    self.mark_target(h.state, h.holder);
                    let args = HArgs {
                        ev,
                        relay: if has_relay { relay_var } else { Val::Undef },
                        target,
                        source,
                        effect,
                        state: h.state,
                        holder: h.holder,
                        this_eff: h.effect,
                        this_ref: if h.on_effect { effect } else { EffRef::eff(h.effect) },
                    };
                    let r = self.call_handler(h.effect, cb, &args);
                    self.effect = parent_effect;
                    self.effect_state = parent_state;
                    r
                }
                None => Val::Undef,
            };
            if !ret.is_undef() {
                relay_var = ret;
                if !relay_var.truthy() || fast_exit {
                    break;
                }
            }
        }
        self.event_depth -= 1;
        if let Val::Num(x) = relay_var {
            if x == x.floor().abs() {
                relay_var = Val::Num(self.modify(x, self.event.modifier));
            }
        }
        self.event = parent_event;
        relay_var
    }

    /// runEvent with an array of targets (TryHit, DamagingHit, AfterMoveSecondary...)
    pub fn run_event_multi(&mut self, ev: Ev, targets: &[Mon], source: Option<Mon>, effect: EffRef, relays: Option<Vec<Val>>) -> Vec<Val> {
        let mut handlers: Vec<Handler> = Vec::new();
        for (i, &t) in targets.iter().enumerate() {
            let start = handlers.len();
            self.find_event_handlers(Arg::Mon(t), ev, source, &mut handlers);
            for h in &mut handlers[start..] {
                h.target = Some(t);
                h.index = Some(i as u8);
            }
        }
        self.sort_handlers(ev, &mut handlers, false);
        let has_relay = relays.is_some();
        let mut target_relay: Vec<Val> = relays.unwrap_or_else(|| vec![Val::True; targets.len()]);
        let parent_event = self.event;
        self.event = EventCtx {
            id: Some(ev),
            target: targets.first().map(|&t| Arg::Mon(t)).unwrap_or(Arg::None),
            source: Arg::from_opt(source),
            effect,
            modifier: 1.0,
        };
        self.event_depth += 1;
        for h in handlers {
            let idx = h.index.unwrap() as usize;
            let tv = target_relay[idx];
            if !tv.truthy() && !(matches!(tv, Val::Num(x) if x == 0.0) && ev == Ev::DamagingHit) {
                continue;
            }
            let tgt = h.target.unwrap();
            self.event.target = Arg::Mon(tgt);
            if !self.handler_applies(ev, &h, effect) {
                continue;
            }
            let ret = match h.cb {
                Some(cb) => {
                    let parent_effect = self.effect;
                    let parent_state = self.effect_state;
                    self.effect = EffRef::eff(h.effect);
                    self.effect_state = h.state;
                    self.mark_target(h.state, h.holder);
                    let args = HArgs {
                        ev,
                        relay: if has_relay { tv } else { Val::Undef },
                        target: Arg::Mon(tgt),
                        source: Arg::from_opt(source),
                        effect,
                        state: h.state,
                        holder: h.holder,
                        this_eff: h.effect,
                        this_ref: if h.on_effect { effect } else { EffRef::eff(h.effect) },
                    };
                    let r = self.call_handler(h.effect, cb, &args);
                    self.effect = parent_effect;
                    self.effect_state = parent_state;
                    r
                }
                None => Val::Undef,
            };
            if !ret.is_undef() {
                if !ret.truthy() {
                    target_relay[idx] = ret;
                    if target_relay.iter().all(|v| !v.truthy()) {
                        break;
                    }
                } else {
                    // relayVar = returnVal; for array events the per-target value is only stored on failure
                    let _ = ret;
                }
            }
        }
        self.event_depth -= 1;
        self.event = parent_event;
        target_relay
    }

    /// the skip conditions inside runEvent's loop
    fn handler_applies(&self, ev: Ev, h: &Handler, _effect: EffRef) -> bool {
        let e = self.eff(h.effect);
        if e.effect_type == EffType::Status {
            if let Holder::Mon(m) = h.holder {
                if self.p(m).status != h.effect {
                    return false;
                }
            }
        }
        if e.effect_type == EffType::Ability {
            if let Holder::Mon(m) = h.holder {
                if e.ability.as_ref().map(|a| a.breakable).unwrap_or(false) && self.suppressing_ability(Some(m)) {
                    return false;
                }
            }
        }
        if !matches!(ev, Ev::Start | Ev::SwitchIn | Ev::TakeItem) && e.effect_type == EffType::Item {
            if let Holder::Mon(m) = h.holder {
                if self.ignoring_item(m) {
                    return false;
                }
            }
        } else if ev != Ev::End && e.effect_type == EffType::Ability {
            if let Holder::Mon(m) = h.holder {
                if self.ignoring_ability(m) {
                    return false;
                }
            }
        }
        if (e.effect_type == EffType::Weather || ev == Ev::Weather) && ev != Ev::Residual && ev != Ev::End && self.suppressing_weather() {
            return false;
        }
        true
    }

    /// `this.effectState.target = effectHolder` (runEvent does this before calling a function handler)
    fn mark_target(&mut self, loc: StateLoc, holder: Holder) {
        if let Some(st) = self.state_mut(loc) {
            st.target_set = true;
            match holder {
                Holder::Mon(m) => st.target_mon = Some(m),
                Holder::Side(s) => st.target_side = Some(s),
                _ => {}
            }
        }
    }

    /// singleEvent(eventid, effect, state, target, source, sourceEffect, relayVar)
    #[allow(clippy::too_many_arguments)]
    pub fn single_event(&mut self, ev: Ev, effect: EffRef, state: StateLoc, target: Arg, source: Arg, source_effect: EffRef, relay: Val) -> Val {
        self.single_event_cb(ev, effect, state, target, source, source_effect, relay, None)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn single_event_cb(&mut self, ev: Ev, effect: EffRef, state: StateLoc, target: Arg, source: Arg, source_effect: EffRef, relay: Val, custom_cb: Option<CbId>) -> Val {
        if effect.is_none() {
            return if relay.is_undef() { Val::True } else { relay };
        }
        let has_relay = !relay.is_undef();
        let relay_var = if has_relay { relay } else { Val::True };
        let e = self.eff(effect.eff);
        if e.effect_type == EffType::Status {
            if let Arg::Mon(m) = target {
                if self.p(m).status != effect.eff {
                    return relay_var;
                }
            }
        }
        if ev == Ev::SwitchIn && e.effect_type == EffType::Ability && e.ability.as_ref().map(|a| a.breakable).unwrap_or(false) {
            if let Arg::Mon(m) = target {
                if self.suppressing_ability(Some(m)) {
                    return relay_var;
                }
            }
        }
        if !matches!(ev, Ev::Start | Ev::TakeItem | Ev::SetAbility) && e.effect_type == EffType::Item {
            if let Arg::Mon(m) = target {
                if self.ignoring_item(m) {
                    return relay_var;
                }
            }
        }
        if ev != Ev::End && e.effect_type == EffType::Ability {
            if let Arg::Mon(m) = target {
                if self.ignoring_ability(m) {
                    return relay_var;
                }
            }
        }
        if e.effect_type == EffType::Weather && !matches!(ev, Ev::FieldStart | Ev::FieldResidual | Ev::FieldEnd) && self.suppressing_weather() {
            return relay_var;
        }
        // the callback: a custom one (fieldEvent), a part of a move (secondary/self), or effect[`on${eventid}`]
        let cb = custom_cb.unwrap_or(self.d.cbs(ev)[P_ON]);
        let part_fn: Option<HandlerFn> = if effect.part != 0 && effect.mv != NONE16 {
            let id = self.move_id_str(effect.mv);
            let name = if effect.part == SELF_PART { format!("self.{}", self.d.cb_name(cb)) } else { format!("secondary.{}", self.d.cb_name(cb)) };
            super::handlers::lookup(Kind::Move, id, &name)
        } else {
            None
        };
        if effect.part != 0 && part_fn.is_none() {
            return relay_var;
        }
        if part_fn.is_none() && !self.eff(effect.eff).has(cb) {
            return relay_var;
        }
        let parent_effect = self.effect;
        let parent_state = self.effect_state;
        let parent_event = self.event;
        // a temp state for `{}` / null
        let mut temp_pushed = false;
        let state = if state == StateLoc::None {
            let st = self.init_state(NO_EFF, Holder::None, None);
            self.temp_states.push(st);
            temp_pushed = true;
            StateLoc::Temp((self.temp_states.len() - 1) as u16)
        } else {
            state
        };
        self.effect = effect;
        self.effect_state = state;
        self.event = EventCtx { id: Some(ev), target, source, effect: source_effect, modifier: 1.0 };
        self.event_depth += 1;
        let holder = match target {
            Arg::Mon(m) => Holder::Mon(m),
            Arg::Side(s) => Holder::Side(s),
            Arg::Field => Holder::Field,
            _ => Holder::Battle,
        };
        let args = HArgs {
            ev,
            relay: if has_relay { relay_var } else { Val::Undef },
            target,
            source,
            effect: source_effect,
            state,
            holder,
            this_eff: effect.eff,
            this_ref: effect,
        };
        let ret = match part_fn {
            Some(f) => f(self, &args),
            None => self.call_handler(effect.eff, cb, &args),
        };
        self.event_depth -= 1;
        self.effect = parent_effect;
        self.effect_state = parent_state;
        self.event = parent_event;
        if temp_pushed {
            self.temp_states.pop();
        }
        if ret.is_undef() { relay_var } else { ret }
    }

    /// eachEvent: the event on every active Pokemon in speed order
    pub fn each_event(&mut self, ev: Ev, effect: EffRef) {
        let mut actives = self.get_all_active(false);
        let effect = if effect.is_none() && !self.effect.is_none() { self.effect } else { effect };
        self.speed_sort_by(&mut actives, |a: &(Mon, f64), b: &(Mon, f64)| b.1 - a.1);
        for (m, _) in actives {
            self.run_event(ev, Arg::Mon(m), None, effect, Val::Undef);
        }
        if ev == Ev::Weather {
            self.each_event(Ev::Update, EffRef::NONE);
        }
    }

    /// getAllActive with each Pokemon's stored speed (for eachEvent sorting)
    pub fn get_all_active(&self, include_fainted: bool) -> Vec<(Mon, f64)> {
        let mut out = vec![];
        for s in 0..2u8 {
            for pos in 0..2 {
                if let Some(i) = self.sides[s as usize].active[pos] {
                    let m = Mon { s, i };
                    if include_fainted || !self.p(m).fainted {
                        out.push((m, self.p(m).speed));
                    }
                }
            }
        }
        out
    }
    pub fn all_active(&self) -> Vec<Mon> {
        self.get_all_active(false).into_iter().map(|x| x.0).collect()
    }

    /// fieldEvent: Residual / SwitchIn
    pub fn field_event(&mut self, ev: Ev, targets: Option<&[Mon]>) {
        let cbs = *self.d.cbs(ev);
        let get_duration = ev == Ev::Residual;
        let mut handlers: Vec<Handler> = vec![];
        self.find_field_handlers(cbs[P_FIELD], get_duration, None, &mut handlers);
        for s in 0..2u8 {
            self.find_side_handlers(s, cbs[P_SIDE], get_duration, None, &mut handlers);
            for pos in 0..2 {
                let Some(i) = self.sides[s as usize].active[pos] else { continue };
                let a = Mon { s, i };
                if ev == Ev::SwitchIn {
                    self.find_pokemon_handlers(a, cbs[P_ANY], false, &mut handlers);
                }
                if let Some(t) = targets {
                    if !t.contains(&a) {
                        continue;
                    }
                }
                self.find_pokemon_handlers(a, cbs[P_ON], get_duration, &mut handlers);
                self.find_side_handlers(s, cbs[P_ON], false, Some(a), &mut handlers);
                self.find_field_handlers(cbs[P_ON], false, Some(a), &mut handlers);
            }
        }
        self.speed_sort(&mut handlers);
        let mut k = 0;
        while k < handlers.len() {
            let h = handlers[k];
            k += 1;
            if let Holder::Mon(m) = h.holder {
                if self.p(m).fainted {
                    let is_slot = self.state(h.state).map(|s| s.is_slot_condition).unwrap_or(false);
                    if !is_slot {
                        continue;
                    }
                }
            }
            if ev == Ev::Residual && h.end != EndKind::None {
                let dur = self.state(h.state).and_then(|s| s.duration);
                if let Some(d) = dur {
                    if d != 0.0 && self.state_uid_ok(h.state, h.state_uid) {
                        let nd = d - 1.0;
                        if let Some(st) = self.state_mut(h.state) {
                            st.duration = Some(nd);
                        }
                        if nd == 0.0 {
                            self.run_end(h.end, h.effect);
                            if self.ended {
                                return;
                            }
                            continue;
                        }
                    }
                }
            }
            // the effect may have been removed by a prior handler
            if !self.state_uid_ok(h.state, h.state_uid) {
                continue;
            }
            let handler_ev = match h.holder {
                Holder::Side(_) => Ev::SideResidual,
                Holder::Field => Ev::FieldResidual,
                _ => ev,
            };
            let handler_ev = if ev == Ev::SwitchIn {
                match h.holder {
                    Holder::Side(_) | Holder::Field => ev,
                    _ => ev,
                }
            } else {
                handler_ev
            };
            if let Some(cb) = h.cb {
                let target = match h.holder {
                    Holder::Mon(m) => Arg::Mon(m),
                    Holder::Side(s) => Arg::Side(s),
                    Holder::Field => Arg::Field,
                    _ => Arg::Battle,
                };
                self.single_event_cb(handler_ev, EffRef::eff(h.effect), h.state, target, Arg::None, EffRef::NONE, Val::Undef, Some(cb));
            }
            self.faint_messages(false, false, true);
            if self.ended {
                return;
            }
        }
    }

    /// does the state the handler was collected from still exist (same object)?
    fn state_uid_ok(&self, loc: StateLoc, uid: u32) -> bool {
        match loc {
            StateLoc::None | StateLoc::Temp(_) => true,
            StateLoc::Status(_) | StateLoc::Ability(_) | StateLoc::Item(_) | StateLoc::Weather | StateLoc::Terrain | StateLoc::Species(_) => {
                self.state(loc).map(|s| s.uid == uid).unwrap_or(false)
            }
            _ => self.state(loc).map(|s| s.uid == uid).unwrap_or(false),
        }
    }

    fn run_end(&mut self, end: EndKind, effect: EffId) {
        match end {
            EndKind::None | EndKind::Noop => {}
            EndKind::ClearStatus(m) => {
                self.clear_status(m);
            }
            EndKind::RemoveVolatile(m, e) => {
                self.remove_volatile(m, e);
            }
            EndKind::ClearAbility(m) => {
                self.set_ability(m, self.d.c.empty, None, EffRef::NONE, false, false);
            }
            EndKind::ClearItem(m) => {
                self.set_item(m, self.d.c.empty, None, EffRef::NONE);
            }
            EndKind::RemoveSlot(s, m, e) => {
                let pos = self.p(m).position;
                self.remove_slot_condition(s, pos, e);
            }
            EndKind::RemoveSide(s, e) => {
                self.remove_side_condition(s, e);
            }
            EndKind::RemovePseudo(e) => {
                self.remove_pseudo_weather(e);
            }
            EndKind::ClearWeather => {
                self.clear_weather();
            }
            EndKind::ClearTerrain => {
                self.clear_terrain();
            }
        }
        let _ = effect;
    }

    // ------------------------------------------------------------------
    // number helpers
    // ------------------------------------------------------------------
    /// Battle.modify(value, numerator, denominator)
    pub fn modify(&self, value: f64, modifier: f64) -> f64 {
        let m = tr(modifier * 4096.0);
        tr((tr(value * m) + 2048.0 - 1.0) / 4096.0)
    }
    pub fn modify_frac(&self, value: f64, num: f64, den: f64) -> f64 {
        let m = tr(num * 4096.0 / den);
        tr((tr(value * m) + 2048.0 - 1.0) / 4096.0)
    }
    /// chainModify(numerator, denominator)
    pub fn chain_modify(&mut self, num: f64, den: f64) {
        let prev = tr(self.event.modifier * 4096.0);
        let next = tr(num * 4096.0 / den);
        self.event.modifier = (((prev * next + 2048.0) as i64) >> 12) as f64 / 4096.0;
    }
    pub fn final_modify(&mut self, v: f64) -> f64 {
        let r = self.modify(v, self.event.modifier);
        self.event.modifier = 1.0;
        r
    }
    pub fn randomizer(&mut self, base: f64) -> f64 {
        let r = self.random(16) as f64;
        tr(tr(base * (100.0 - r)) / 100.0)
    }
}

/// Battle.compareLeftToRightOrder
fn compare_left_to_right(a: &Handler, b: &Handler) -> f64 {
    let ao = a.order.unwrap_or(4294967296.0);
    let bo = b.order.unwrap_or(4294967296.0);
    let d = -(bo - ao);
    if d != 0.0 {
        return d;
    }
    let d = b.priority - a.priority;
    if d != 0.0 {
        return d;
    }
    // index: handlers of array events carry the target index
    let ai = a.index.map(|x| x as f64).unwrap_or(0.0);
    let bi = b.index.map(|x| x as f64).unwrap_or(0.0);
    -(bi - ai)
}
