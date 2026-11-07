//! Pokemon methods (`sim/pokemon.ts`, with the Champions mod's overrides), as Battle methods.

use super::battle::*;
use super::data::*;
use super::events::Ev;
use super::state::*;
use super::val::{Mon, Val};
use crate::add;

const BOOST_TABLE: [f64; 7] = [1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0];

impl Battle {
    // ------------------------------------------------------------------
    // construction
    // ------------------------------------------------------------------
    /// Champions statModify: HP = base + SP + 75; others = (base + SP + 20) x nature
    pub fn stat_modify(&self, base: &[i32; 6], set: &SetData, stat: usize) -> i32 {
        let b = base[stat];
        let evs = set.evs[stat].clamp(0, 255);
        if stat == 0 {
            return b + evs + 75;
        }
        let mut s = (b + evs + 20) as f64;
        let n = self.d.nature(&set.nature);
        if n.plus == Some(stat) {
            s = tr(tr_bits(s * 110.0, 16) / 100.0);
        } else if n.minus == Some(stat) {
            s = tr(tr_bits(s * 90.0, 16) / 100.0);
        }
        s as i32
    }
    pub fn spread_modify(&self, base: &[i32; 6], set: &SetData) -> [i32; 6] {
        let mut out = [0; 6];
        for (k, o) in out.iter_mut().enumerate() {
            *o = self.stat_modify(base, set, k);
        }
        out
    }

    /// new Pokemon(set, side): everything but the random gender (done by the caller, in order)
    pub fn make_pokemon(&mut self, set: &SetData, side: u8, ti: u8) -> Result<Pokemon, String> {
        let d = self.d;
        let sp = d.species_id(&set.species).ok_or_else(|| format!("Unidentified species: {}", set.species))?;
        let species = &d.species[sp];
        let mut set = set.clone();
        if set.name == set.species || set.name.is_empty() {
            set.name = species.base_species.clone();
        }
        let name: String = set.name.chars().take(20).collect();
        let level = if set.level <= 0 { 100 } else { set.level.clamp(1, 9999) };
        let mut moves = vec![];
        for mname in &set.moves {
            let Some(mi) = d.move_id(mname) else { continue };
            let md = &d.moves[mi as usize];
            let pp = if md.no_pp_boosts { md.pp } else { (md.pp / 5 + 1) * 4 };
            moves.push(MoveSlot {
                id: mi,
                pp,
                maxpp: pp,
                target: md.target,
                disabled: 0,
                disabled_source: None,
                used: false,
                virtual_: false,
            });
        }
        if moves.is_empty() {
            return Err(format!("Set {name} has no moves"));
        }
        let ability = d.ability(&set.ability);
        let item = d.item(&set.item);
        let can_mega = d.eff(item).item.as_ref().and_then(|it| {
            it.mega_stone.iter().find(|(from, _)| *from == species.name).and_then(|(_, to)| d.species_id(to)).map(|x| x as u16)
        });
        let empty = d.c.empty;
        let mut p = Pokemon {
            side,
            ti,
            set: set.clone(),
            name,
            level,
            gender: "",
            base_move_slots: moves.clone(),
            move_slots: moves,
            position: ti,
            details: String::new(),
            base_species: sp as u16,
            species: sp as u16,
            species_state: EffectState { id: empty, ..Default::default() },
            status: empty,
            status_state: EffectState { id: empty, ..Default::default() },
            volatiles: vec![],
            base_stored_stats: [0; 6],
            stored_stats: [0; 6],
            boosts: [0; 7],
            base_ability: ability,
            ability,
            ability_state: EffectState { id: ability, ..Default::default() },
            item,
            item_state: EffectState { id: item, ..Default::default() },
            last_item: empty,
            used_item_this_turn: false,
            ate_berry: false,
            trapped: 0,
            maybe_trapped: false,
            maybe_disabled: false,
            maybe_locked: false,
            transformed: false,
            syrup_triggered: false,
            maxhp: 0,
            base_maxhp: 0,
            hp: 0,
            fainted: false,
            faint_queued: false,
            sub_fainted: None,
            forme_regression: false,
            types: species.types.clone(),
            added_type: NO_TYPE,
            known_type: true,
            apparent_type: species.types.iter().map(|&t| d.type_name(t).to_string()).collect::<Vec<_>>().join("/"),
            switch_flag: SwitchFlag::No,
            force_switch_flag: false,
            skip_before_switch_out: false,
            dragged_in: None,
            newly_switched: false,
            hero_message_displayed: false,
            being_called_back: false,
            last_move: None,
            last_move_used: None,
            last_move_target_loc: None,
            move_this_turn: None,
            stats_raised_this_turn: false,
            stats_lowered_this_turn: false,
            move_last_turn_result: Val::Undef,
            move_this_turn_result: Val::Undef,
            hurt_this_turn: None,
            last_damage: 0,
            attacked_by: vec![],
            times_attacked: 0,
            is_active: false,
            active_turns: 0,
            active_move_actions: 0,
            previously_switched_in: 0,
            is_started: false,
            during_move: false,
            weighthg: 1,
            speed: 0.0,
            can_mega_evo: can_mega,
            truant_turn: false,
            illusion: None,
        };
        // the ability / item states: initEffectState with a target that isn't active -> effectOrder 0
        p.ability_state.uid = self.next_uid();
        p.item_state.uid = self.next_uid();
        p.status_state.uid = self.next_uid();
        p.species_state.uid = self.next_uid();
        p.ability_state.target_set = true;
        p.item_state.target_set = true;
        Ok(p)
    }

    /// The rest of the constructor once the Pokemon is in place (clearVolatile -> setSpecies).
    pub fn finish_pokemon(&mut self, m: Mon) {
        self.pm(m).ability_state.target_mon = Some(m);
        self.pm(m).item_state.target_mon = Some(m);
        self.clear_volatile(m, true);
        let mh = self.p(m).maxhp;
        self.pm(m).hp = mh;
        self.pm(m).details = self.updated_details(m);
    }

    pub fn updated_details(&self, m: Mon) -> String {
        self.updated_details_level(m, self.p(m).level)
    }
    pub fn updated_details_level(&self, m: Mon, level: i32) -> String {
        let p = self.p(m);
        let mut name = self.d.species[p.species as usize].name.clone();
        if name == "Greninja-Bond" || name == "Rockruff-Dusk" {
            name = self.d.species[p.species as usize].base_species.clone();
        }
        let mut s = name;
        if level != 100 {
            s.push_str(&format!(", L{}", level));
        }
        if !p.gender.is_empty() {
            s.push_str(&format!(", {}", p.gender));
        }
        if p.set.shiny {
            s.push_str(", shiny");
        }
        s
    }

    // ------------------------------------------------------------------
    // positions
    // ------------------------------------------------------------------
    pub fn is_ally(&self, a: Mon, b: Option<Mon>) -> bool {
        b.map(|b| a.s == b.s).unwrap_or(false)
    }
    pub fn is_adjacent(&self, a: Mon, b: Mon) -> bool {
        if self.p(a).fainted || self.p(b).fainted {
            return false;
        }
        a != b
    }
    pub fn adjacent_allies(&self, m: Mon) -> Vec<Mon> {
        self.allies_and_self(m.s).into_iter().filter(|&a| self.is_adjacent(m, a)).collect()
    }
    pub fn adjacent_foes(&self, m: Mon) -> Vec<Mon> {
        self.foes_of(m.s)
    }
    pub fn get_loc_of(&self, src: Mon, target: Mon) -> i8 {
        let position = self.p(target).position as i8 + 1;
        if src.s == target.s { -position } else { position }
    }
    pub fn get_at_loc(&self, src: Mon, loc: i8) -> Option<Mon> {
        let s = if loc < 0 { src.s } else { 1 - src.s };
        let l = loc.unsigned_abs() as usize;
        if l == 0 || l > 2 {
            return None;
        }
        self.active_at(s as usize, l - 1)
    }
    /// getFieldPositionValue
    pub fn field_position(&self, m: Mon) -> u8 {
        m.s + 2 * self.p(m).position
    }

    // ------------------------------------------------------------------
    // abilities & items
    // ------------------------------------------------------------------
    pub fn ability_id(&self, m: Mon) -> &'static str {
        &self.eff(self.p(m).ability).id
    }
    pub fn item_id(&self, m: Mon) -> &'static str {
        &self.eff(self.p(m).item).id
    }
    pub fn ignoring_ability(&self, m: Mon) -> bool {
        let p = self.p(m);
        if !p.is_active {
            return true;
        }
        let ab = self.eff(p.ability);
        let info = ab.ability.as_deref();
        if info.map(|a| a.notransform).unwrap_or(false) && p.transformed {
            return true;
        }
        if info.map(|a| a.cantsuppress).unwrap_or(false) {
            return false;
        }
        if self.has_vol(m, "gastroacid") {
            return true;
        }
        if self.has_item(m, "abilityshield") || ab.id == "neutralizinggas" {
            return false;
        }
        for (o, _) in self.get_all_active(false) {
            let op = self.p(o);
            if self.eff(op.ability).id == "neutralizinggas" && !self.has_vol(o, "gastroacid") && !op.transformed
                && !op.ability_state.get(K::Ending).truthy() && !self.has_vol(m, "commanding") {
                return true;
            }
        }
        false
    }
    pub fn ignoring_item(&self, m: Mon) -> bool {
        let p = self.p(m);
        let it = self.eff(p.item);
        if it.item.as_ref().map(|i| i.is_primal_orb).unwrap_or(false) {
            return false;
        }
        if !p.is_active {
            return true;
        }
        if self.has_vol(m, "embargo") || self.field.pseudo_weather.iter().any(|x| self.eff(x.0).id == "magicroom") {
            return true;
        }
        let ignore_klutz = it.item.as_ref().map(|i| i.ignore_klutz).unwrap_or(false);
        !ignore_klutz && self.has_ability(m, "klutz")
    }
    pub fn has_item(&self, m: Mon, id: &str) -> bool {
        if self.item_id(m) != id {
            return false;
        }
        !self.ignoring_item(m)
    }
    pub fn has_item_any(&self, m: Mon, ids: &[&str]) -> bool {
        if !ids.contains(&self.item_id(m)) {
            return false;
        }
        !self.ignoring_item(m)
    }
    pub fn has_ability(&self, m: Mon, id: &str) -> bool {
        if self.ability_id(m) != id {
            return false;
        }
        !self.ignoring_ability(m)
    }
    pub fn has_ability_any(&self, m: Mon, ids: &[&str]) -> bool {
        if !ids.contains(&self.ability_id(m)) {
            return false;
        }
        !self.ignoring_ability(m)
    }

    // ------------------------------------------------------------------
    // types
    // ------------------------------------------------------------------
    pub fn get_types(&mut self, m: Mon, exclude_added: bool) -> Vec<TypeId> {
        let p = self.p(m);
        let mut arr = [NO_TYPE; 3];
        for (k, &t) in p.types.iter().enumerate().take(3) {
            arr[k] = t;
        }
        let n = p.types.len().min(3) as u8;
        let r = self.run_event(Ev::Type, Arg::Mon(m), None, EffRef::NONE, Val::Types(arr, n));
        let mut types: Vec<TypeId> = match r {
            Val::Types(a, n) => a[..n as usize].to_vec(),
            _ => self.p(m).types.clone(),
        };
        if types.is_empty() {
            types.push(self.d.type_id("Normal"));
        }
        let added = self.p(m).added_type;
        if !exclude_added && added != NO_TYPE {
            types.push(added);
        }
        types
    }
    pub fn has_type(&mut self, m: Mon, t: &str) -> bool {
        let tid = self.d.type_id(t);
        self.get_types(m, false).contains(&tid)
    }
    pub fn has_type_id(&mut self, m: Mon, t: TypeId) -> bool {
        self.get_types(m, false).contains(&t)
    }
    /// dex.getImmunity(key, pokemon)
    pub fn dex_immunity(&mut self, key: &str, m: Mon) -> bool {
        let types = self.get_types(m, false);
        for t in types {
            if self.d.taken_code(key, t) == 3 {
                return false;
            }
        }
        true
    }

    pub fn is_grounded(&mut self, m: Mon, negate_immunity: bool) -> Option<bool> {
        if self.field.pseudo_weather.iter().any(|x| self.eff(x.0).id == "gravity") {
            return Some(true);
        }
        if self.has_vol(m, "ingrain") || self.has_vol(m, "smackdown") {
            return Some(true);
        }
        let item = if self.ignoring_item(m) { "" } else { self.item_id(m) };
        if item == "ironball" {
            return Some(true);
        }
        if !negate_immunity && self.has_type(m, "Flying") && !(self.has_type(m, "???") && self.has_vol(m, "roost")) {
            return Some(false);
        }
        if self.has_ability_any(m, &["levitate", "eelevate"]) && !self.suppressing_ability(Some(m)) {
            return None;
        }
        if self.has_vol(m, "magnetrise") || self.has_vol(m, "telekinesis") {
            return Some(false);
        }
        Some(item != "airballoon")
    }
    pub fn is_semi_invulnerable(&self, m: Mon) -> bool {
        ["fly", "bounce", "dive", "dig", "phantomforce", "shadowforce"].iter().any(|v| self.has_vol(m, v)) || self.is_sky_dropped(m)
    }
    pub fn is_sky_dropped(&self, m: Mon) -> bool {
        if self.has_vol(m, "skydrop") {
            return true;
        }
        let Some(sd) = self.d.cond("skydrop") else { return false };
        for f in self.allies_and_self_all(1 - m.s) {
            if let Some(st) = self.volatile(f, sd) {
                if st.source == Some(m) {
                    return true;
                }
            }
        }
        false
    }
    pub fn is_protected(&self, m: Mon) -> bool {
        ["protect", "detect", "maxguard", "kingsshield", "spikyshield", "banefulbunker", "obstruct", "silktrap", "burningbulwark"]
            .iter()
            .any(|v| self.has_vol(m, v))
    }

    // ------------------------------------------------------------------
    // weather
    // ------------------------------------------------------------------
    pub fn suppressing_weather(&self) -> bool {
        for s in 0..2 {
            for pos in 0..2 {
                let Some(i) = self.sides[s].active[pos] else { continue };
                let m = Mon { s: s as u8, i };
                let p = self.p(m);
                if !p.fainted && !self.ignoring_ability(m)
                    && self.eff(p.ability).ability.as_ref().map(|a| a.suppress_weather).unwrap_or(false)
                    && !p.ability_state.get(K::Ending).truthy() {
                    return true;
                }
            }
        }
        false
    }
    /// field.effectiveWeather()
    pub fn field_weather(&self) -> &'static str {
        if self.suppressing_weather() {
            return "";
        }
        &self.eff(self.field.weather).id
    }
    pub fn is_weather(&self, ids: &[&str]) -> bool {
        let w = self.field_weather();
        ids.contains(&w)
    }
    /// pokemon.effectiveWeather(sourceEffect, message)
    pub fn effective_weather(&mut self, m: Mon, source_effect: EffRef, message: bool) -> &'static str {
        let se = if source_effect.is_none() { self.effect } else { source_effect };
        let weather = self.field_weather();
        if let Some(ap) = self.active_pokemon {
            if self.has_ability(ap, "megasol") && !se.is_none() {
                let e = self.eff(se.eff);
                if e.id == "megasol" || se.is_move() || e.effect_type == EffType::Move || e.effect_type == EffType::Weather {
                    if weather != "sunnyday" && message {
                        add!(self, "-activate", m, "ability: Mega Sol");
                    }
                    return "sunnyday";
                }
            }
        }
        if matches!(weather, "sunnyday" | "raindance" | "desolateland" | "primordialsea") && self.has_item(m, "utilityumbrella") {
            return "";
        }
        weather
    }

    // ------------------------------------------------------------------
    // stats
    // ------------------------------------------------------------------
    fn full_boosts(&self, m: Mon) -> SparseBoosts {
        let mut b = SparseBoosts::default();
        for (k, &v) in self.p(m).boosts.iter().enumerate() {
            b.set(k as u8, v);
        }
        b
    }
    /// getStat(statName, unboosted, unmodified); stat: 1..=5 (atk..spe)
    pub fn get_stat(&mut self, m: Mon, stat: usize, unboosted: bool, unmodified: bool) -> f64 {
        let mut st = self.p(m).stored_stats[stat] as f64;
        if !unboosted {
            let mut boost = self.p(m).boosts[stat - 1] as i32;
            if !unmodified {
                let b = self.full_boosts(m);
                if let Val::Boosts(nb) = self.run_event(Ev::ModifyBoost, Arg::Mon(m), None, EffRef::NONE, Val::Boosts(b)) {
                    boost = nb.get(stat as u8 - 1).unwrap_or(0) as i32;
                }
            }
            let boost = boost.clamp(-6, 6);
            st = if boost >= 0 { (st * BOOST_TABLE[boost as usize]).floor() } else { (st / BOOST_TABLE[(-boost) as usize]).floor() };
        }
        if !unmodified {
            let ev = match stat {
                1 => Ev::ModifyAtk,
                2 => Ev::ModifyDef,
                3 => Ev::ModifySpA,
                4 => Ev::ModifySpD,
                _ => Ev::ModifySpe,
            };
            st = self.run_event(ev, Arg::Mon(m), None, EffRef::NONE, Val::Num(st)).num();
        }
        if stat == 5 && st > 10000.0 {
            st = 10000.0;
        }
        st
    }
    /// calculateStat(statName, boost, modifier, statUser)
    pub fn calculate_stat(&mut self, m: Mon, stat: usize, boost: i32, modifier: f64, stat_user: Option<Mon>) -> f64 {
        let mut st = self.p(m).stored_stats[stat] as f64;
        let mut b = SparseBoosts::default();
        b.set(stat as u8 - 1, boost as i8);
        let user = stat_user.unwrap_or(m);
        let mut boost = boost;
        if let Val::Boosts(nb) = self.run_event(Ev::ModifyBoost, Arg::Mon(user), None, EffRef::NONE, Val::Boosts(b)) {
            boost = nb.get(stat as u8 - 1).unwrap_or(0) as i32;
        }
        let boost = boost.clamp(-6, 6);
        st = if boost >= 0 { (st * BOOST_TABLE[boost as usize]).floor() } else { (st / BOOST_TABLE[(-boost) as usize]).floor() };
        self.modify(st, if modifier == 0.0 { 1.0 } else { modifier })
    }
    /// Champions getActionSpeed: no 13-bit truncation, Trick Room negates
    pub fn get_action_speed(&mut self, m: Mon) -> f64 {
        let mut speed = self.get_stat(m, 5, false, false);
        if self.field.pseudo_weather.iter().any(|x| x.0 == self.d.c.trickroom) {
            speed = -speed;
        }
        speed
    }
    pub fn update_speed(&mut self, m: Mon) {
        let s = self.get_action_speed(m);
        self.pm(m).speed = s;
    }
    pub fn update_all_speed(&mut self) {
        for m in self.all_active() {
            self.update_speed(m);
        }
    }
    pub fn get_weight(&mut self, m: Mon) -> f64 {
        let w = self.p(m).weighthg as f64;
        let r = self.run_event(Ev::ModifyWeight, Arg::Mon(m), None, EffRef::NONE, Val::Num(w)).num();
        r.max(1.0)
    }
    /// getBestStat(unboosted, unmodified): 1..=5
    pub fn get_best_stat(&mut self, m: Mon, unboosted: bool, unmodified: bool) -> usize {
        let mut best = 1;
        let mut best_v = 0.0;
        for s in 1..=5 {
            let v = self.get_stat(m, s, unboosted, unmodified);
            if v > best_v {
                best = s;
                best_v = self.get_stat(m, s, unboosted, unmodified);
            }
        }
        best
    }

    // ------------------------------------------------------------------
    // immunity & effectiveness
    // ------------------------------------------------------------------
    /// runEffectiveness(move)
    pub fn run_effectiveness(&mut self, m: Mon, mv: u16) -> i32 {
        let mut total = 0;
        let move_type = self.mv(mv).type_;
        for t in self.get_types(m, false) {
            let code = self.d.taken_type(move_type, t);
            let mut tm: f64 = match code {
                1 => 1.0,
                2 => -1.0,
                _ => 0.0,
            };
            let me = self.mv(mv).eff;
            let mref = EffRef { eff: me, mv, part: 0 };
            tm = self.single_event(Ev::Effectiveness, mref, StateLoc::None, Arg::Mon(m), Arg::Type(t), mref, Val::Num(tm)).num();
            total += self.run_event_full(Ev::Effectiveness, Arg::Mon(m), Arg::Type(t), mref, Val::Num(tm), false, false).num() as i32;
        }
        total
    }

    /// runImmunity(move, message): false = immune
    pub fn run_immunity_move(&mut self, m: Mon, mv: u16, message: bool) -> bool {
        let t = self.mv(mv).type_;
        match self.mv(mv).ignore_immunity {
            Some(IgnoreImmunity::All) => return true,
            Some(IgnoreImmunity::Types(bits)) if t != NO_TYPE && bits & (1 << t) != 0 => return true,
            _ => {}
        }
        self.run_immunity_type(m, t, message)
    }
    pub fn run_immunity_type(&mut self, m: Mon, t: TypeId, message: bool) -> bool {
        if t == NO_TYPE {
            return true;
        }
        let negate = !self.run_event_full(Ev::NegateImmunity, Arg::Mon(m), Arg::Type(t), EffRef::NONE, Val::Undef, false, false).truthy();
        let ground = self.d.type_id("Ground");
        let not_immune: Option<bool> = if t == ground {
            self.is_grounded(m, negate)
        } else {
            let tn = self.d.type_name(t).to_string();
            Some(negate || self.dex_immunity(&tn, m))
        };
        if not_immune == Some(true) {
            return true;
        }
        if !message {
            return false;
        }
        if not_immune.is_none() {
            if self.has_ability(m, "levitate") {
                add!(self, "-immune", m, "[from] ability: Levitate");
            } else if self.has_ability(m, "eelevate") {
                add!(self, "-immune", m, "[from] ability: Eelevate");
            } else {
                add!(self, "-immune", m);
            }
        } else {
            add!(self, "-immune", m);
        }
        false
    }
    /// runStatusImmunity(type, message)
    pub fn run_status_immunity(&mut self, m: Mon, key: &'static str, message: bool) -> bool {
        if self.p(m).fainted {
            return false;
        }
        if key.is_empty() {
            return true;
        }
        if !self.dex_immunity(key, m) {
            if message {
                add!(self, "-immune", m);
            }
            return false;
        }
        let relay = match self.d.type_id(key) {
            NO_TYPE => Val::Key(key),
            t => Val::Type(t),
        };
        let imm = self.run_event(Ev::Immunity, Arg::Mon(m), None, EffRef::NONE, relay);
        if !imm.truthy() {
            if message && imm != Val::Null {
                add!(self, "-immune", m);
            }
            return false;
        }
        true
    }

    // ------------------------------------------------------------------
    // status
    // ------------------------------------------------------------------
    pub fn try_set_status(&mut self, m: Mon, status: EffId, source: Option<Mon>, source_effect: EffRef) -> Val {
        let cur = self.p(m).status;
        let st = if cur != self.d.c.empty { cur } else { status };
        self.set_status(m, st, source, source_effect, false)
    }
    /// setStatus: returns True / False / Null like Showdown
    pub fn set_status(&mut self, m: Mon, status: EffId, source: Option<Mon>, source_effect: EffRef, ignore_immunities: bool) -> Val {
        if self.p(m).hp == 0 {
            return Val::False;
        }
        let empty = self.d.c.empty;
        if !self.p(m).is_active && status != empty {
            return Val::False;
        }
        let mut source = source;
        let mut source_effect = source_effect;
        if source.is_none() {
            source = self.event.source.mon();
        }
        if source_effect.is_none() {
            source_effect = self.effect;
        }
        let source = source.unwrap_or(m);
        let se_status = self.move_status_of(source_effect);
        if self.p(m).status == status {
            if se_status.is_some() && se_status == Some(self.p(m).status) {
                let sid = self.eff(self.p(m).status).id.clone();
                add!(self, "-fail", m, sid);
            } else if se_status.is_some() {
                add!(self, "-fail", source);
                self.attr_last_move(&["[still]"]);
            }
            return Val::False;
        }
        let sid: &'static str = &self.eff(status).id;
        if !ignore_immunities && status != empty && !(self.has_ability(source, "corrosion") && (sid == "tox" || sid == "psn")) {
            let key = if sid == "tox" { "psn" } else { sid };
            if !self.run_status_immunity(m, key, false) {
                if se_status.is_some() {
                    add!(self, "-immune", m);
                }
                return Val::False;
            }
        }
        let prev_status = self.p(m).status;
        let prev_state = self.p(m).status_state.clone();
        if status != empty {
            let r = self.run_event(Ev::SetStatus, Arg::Mon(m), Some(source), source_effect, Val::Eff(status));
            if !r.truthy() {
                return r;
            }
        }
        let mut st = self.init_state(status, Holder::Mon(m), None);
        st.source = Some(source);
        if let Some(d) = self.eff(status).duration {
            st.duration = Some(d as f64);
        }
        self.pm(m).status = status;
        self.pm(m).status_state = st;
        if self.eff(status).duration_cb {
            let dur = self.call_duration_callback(status, Arg::Mon(m), Some(source), source_effect);
            self.pm(m).status_state.duration = Some(dur);
        }
        if status != empty {
            let r = self.single_event(Ev::Start, EffRef::eff(status), StateLoc::Status(m), Arg::Mon(m), Arg::Mon(source), source_effect, Val::Undef);
            if !r.truthy() {
                self.pm(m).status = prev_status;
                self.pm(m).status_state = prev_state;
                return Val::False;
            }
            let r = self.run_event(Ev::AfterSetStatus, Arg::Mon(m), Some(source), source_effect, Val::Eff(status));
            if !r.truthy() {
                return Val::False;
            }
        }
        Val::True
    }
    /// (sourceEffect as Move)?.status
    pub fn move_status_of(&self, e: EffRef) -> Option<EffId> {
        if e.is_move() {
            if e.part == 0 {
                return self.mv(e.mv).status;
            }
            return None;
        }
        if e.is_none() {
            return None;
        }
        let ef = self.eff(e.eff);
        if let Some(st) = ef.id.strip_prefix("synchronize:") {
            return self.d.cond(st);
        }
        if ef.kind == Kind::Move && ef.move_def != u16::MAX {
            return self.d.moves[ef.move_def as usize].status;
        }
        None
    }
    pub fn cure_status(&mut self, m: Mon, silent: bool) -> bool {
        if self.p(m).hp == 0 || self.p(m).status == self.d.c.empty {
            return false;
        }
        let sid = self.eff(self.p(m).status).id.clone();
        add!(self, "-curestatus", m, sid.clone(), if silent { "[silent]" } else { "[msg]" });
        if sid == "slp" && self.remove_volatile_id(m, "nightmare") {
            add!(self, "-end", m, "Nightmare", "[silent]");
        }
        self.set_status(m, self.d.c.empty, None, EffRef::NONE, false);
        true
    }
    pub fn clear_status(&mut self, m: Mon) -> bool {
        if self.p(m).hp == 0 || self.p(m).status == self.d.c.empty {
            return false;
        }
        if self.eff(self.p(m).status).id == "slp" && self.remove_volatile_id(m, "nightmare") {
            add!(self, "-end", m, "Nightmare", "[silent]");
        }
        self.set_status(m, self.d.c.empty, None, EffRef::NONE, false);
        true
    }

    /// durationCallback(target, source, effect)
    pub fn call_duration_callback(&mut self, cond: EffId, target: Arg, source: Option<Mon>, source_effect: EffRef) -> f64 {
        let cb = self.d.cb_id("durationCallback");
        let args = HArgs {
            ev: Ev::DurationCallback,
            relay: Val::Undef,
            target,
            source: Arg::from_opt(source),
            effect: source_effect,
            state: StateLoc::None,
            holder: Holder::None,
            this_eff: cond,
            this_ref: EffRef::eff(cond),
        };
        let parent = self.effect;
        self.effect = EffRef::eff(cond);
        let r = match self.eff(cond).handler(cb).and_then(|h| h.imp) {
            Some(f) => f(self, &args),
            None => {
                let msg = format!("{} durationCallback", self.eff(cond).id);
                if !self.missing.contains(&msg) {
                    self.missing.push(msg);
                }
                Val::Undef
            }
        };
        self.effect = parent;
        r.num()
    }

    // ------------------------------------------------------------------
    // volatiles
    // ------------------------------------------------------------------
    pub fn add_volatile(&mut self, m: Mon, status: EffId, source: Option<Mon>, source_effect: EffRef, linked: Option<EffId>) -> Val {
        if self.p(m).hp == 0 && !self.eff(status).affects_fainted {
            return Val::False;
        }
        if let (Some(_), Some(src)) = (linked, source) {
            if self.p(src).hp == 0 {
                return Val::False;
            }
        }
        let mut source = source;
        let mut source_effect = source_effect;
        if source.is_none() {
            source = self.event.source.mon();
        }
        if source_effect.is_none() {
            source_effect = self.effect;
        }
        let source = source.unwrap_or(m);
        if self.has_volatile(m, status) {
            if !self.eff(status).has(self.d.cbs(Ev::Restart)[P_ON]) {
                return Val::False;
            }
            return self.single_event(Ev::Restart, EffRef::eff(status), StateLoc::Volatile(m, status), Arg::Mon(m), Arg::Mon(source), source_effect, Val::Undef);
        }
        let sid: &'static str = &self.eff(status).id;
        if !self.run_status_immunity(m, sid, false) {
            if self.move_status_of(source_effect).is_some() {
                add!(self, "-immune", m);
            }
            return Val::False;
        }
        let r = self.run_event(Ev::TryAddVolatile, Arg::Mon(m), Some(source), source_effect, Val::Eff(status));
        if !r.truthy() {
            return r;
        }
        let mut st = self.init_state(status, Holder::Mon(m), None);
        st.source = Some(source);
        st.source_slot = Some((source.s, self.p(source).position));
        if !source_effect.is_none() {
            st.source_effect = Some(source_effect.eff);
        }
        if let Some(d) = self.eff(status).duration {
            st.duration = Some(d as f64);
        }
        self.pm(m).volatiles.push((status, st));
        if self.eff(status).duration_cb {
            let dur = self.call_duration_callback(status, Arg::Mon(m), Some(source), source_effect);
            if let Some(v) = self.volatile_mut(m, status) {
                v.duration = Some(dur);
            }
        }
        let r = self.single_event(Ev::Start, EffRef::eff(status), StateLoc::Volatile(m, status), Arg::Mon(m), Arg::Mon(source), source_effect, Val::Undef);
        if !r.truthy() {
            self.pm(m).volatiles.retain(|x| x.0 != status);
            return r;
        }
        if let Some(ls) = linked {
            if !self.has_volatile(source, ls) {
                self.add_volatile(source, ls, Some(m), source_effect, None);
                if let Some(v) = self.volatile_mut(source, ls) {
                    v.linked = vec![m];
                    v.linked_status = Some(status);
                }
            } else if let Some(v) = self.volatile_mut(source, ls) {
                v.linked.push(m);
            }
            if let Some(v) = self.volatile_mut(m, status) {
                v.linked = vec![source];
                v.linked_status = Some(ls);
            }
        }
        Val::True
    }
    pub fn remove_volatile(&mut self, m: Mon, status: EffId) -> bool {
        if self.p(m).hp == 0 {
            return false;
        }
        if !self.has_volatile(m, status) {
            return false;
        }
        let (linked, linked_status) = {
            let v = self.volatile(m, status).unwrap();
            (v.linked.clone(), v.linked_status)
        };
        self.single_event(Ev::End, EffRef::eff(status), StateLoc::Volatile(m, status), Arg::Mon(m), Arg::None, EffRef::NONE, Val::Undef);
        self.pm(m).volatiles.retain(|x| x.0 != status);
        if !linked.is_empty() {
            if let Some(ls) = linked_status {
                self.remove_linked_volatiles(m, ls, &linked);
            }
        }
        true
    }
    pub fn remove_volatile_id(&mut self, m: Mon, id: &str) -> bool {
        match self.d.cond(id) {
            Some(e) => self.remove_volatile(m, e),
            None => false,
        }
    }
    pub fn remove_linked_volatiles(&mut self, m: Mon, linked_status: EffId, linked: &[Mon]) {
        for &lp in linked {
            let Some(v) = self.volatile_mut(lp, linked_status) else { continue };
            if let Some(pos) = v.linked.iter().position(|&x| x == m) {
                v.linked.remove(pos);
            }
            if v.linked.is_empty() {
                self.remove_volatile(lp, linked_status);
            }
        }
    }

    // ------------------------------------------------------------------
    // items
    // ------------------------------------------------------------------
    pub fn set_item(&mut self, m: Mon, item: EffId, source: Option<Mon>, effect: EffRef) -> bool {
        if self.p(m).hp == 0 || !self.p(m).is_active {
            return false;
        }
        let old = self.p(m).item;
        let old_state = self.p(m).item_state.clone();
        let st = self.init_state(item, Holder::Mon(m), None);
        self.pm(m).item = item;
        self.pm(m).item_state = st;
        if old != self.d.c.empty {
            // singleEvent('End', oldItem, oldItemState, this): the old state is detached
            self.single_event_detached(Ev::End, old, old_state, Arg::Mon(m));
        }
        if item != self.d.c.empty {
            self.single_event(Ev::Start, EffRef::eff(item), StateLoc::Item(m), Arg::Mon(m), Arg::from_opt(source), effect, Val::Undef);
        }
        true
    }
    /// run a singleEvent with a state that is no longer attached (a removed item / ability state)
    pub fn single_event_detached(&mut self, ev: Ev, effect: EffId, state: EffectState, target: Arg) {
        self.temp_states.push(state);
        let loc = StateLoc::Temp((self.temp_states.len() - 1) as u16);
        self.single_event(ev, EffRef::eff(effect), loc, target, Arg::None, EffRef::NONE, Val::Undef);
        self.temp_states.pop();
    }
    pub fn take_item(&mut self, m: Mon, source: Option<Mon>) -> Option<EffId> {
        let source = source.unwrap_or(m);
        let item = self.p(m).item;
        if item == self.d.c.empty {
            return None;
        }
        if self.run_event(Ev::TakeItem, Arg::Mon(m), Some(source), EffRef::NONE, Val::Eff(item)).truthy() {
            self.pm(m).item = self.d.c.empty;
            let empty = self.d.c.empty;
            Battle::clear_state(&mut self.pm(m).item_state, empty);
            // End runs with the (now cleared) item state object
            self.single_event(Ev::End, EffRef::eff(item), StateLoc::Item(m), Arg::Mon(m), Arg::None, EffRef::NONE, Val::Undef);
            self.run_event(Ev::AfterTakeItem, Arg::Mon(m), None, EffRef::NONE, Val::Eff(item));
            return Some(item);
        }
        None
    }
    pub fn use_item(&mut self, m: Mon, source: Option<Mon>, source_effect: EffRef) -> bool {
        let item = self.p(m).item;
        let is_gem = self.eff(item).item.as_ref().map(|i| i.is_gem).unwrap_or(false);
        if (self.p(m).hp == 0 && !is_gem) || !self.p(m).is_active {
            return false;
        }
        if item == self.d.c.empty {
            return false;
        }
        let se = if source_effect.is_none() { self.effect } else { source_effect };
        let source = source.or(self.event.target.mon());
        if !se.is_none() && !se.is_move() && self.eff(se.eff).effect_type == EffType::Item && item != se.eff && source == Some(m) {
            return false;
        }
        if self.run_event(Ev::UseItem, Arg::Mon(m), None, EffRef::NONE, Val::Eff(item)).truthy() {
            let iname = self.eff(item).name.clone();
            if self.item_id(m) == "redcard" {
                let src = source.map(|s| self.mon_str(s)).unwrap_or_default();
                add!(self, "-enditem", m, format!("{iname}"), format!("[of] {src}"));
            } else if is_gem {
                let mvname = self.active_move.map(|a| self.move_def(a).name.clone()).unwrap_or_default();
                add!(self, "-enditem", m, iname, "[from] gem", format!("[move] {mvname}"));
            } else {
                add!(self, "-enditem", m, iname);
            }
            if let Some(b) = self.eff(item).item.as_ref().and_then(|i| i.boosts) {
                self.boost(b, Some(m), source, EffRef::eff(item), false, false);
            }
            self.single_event(Ev::Use, EffRef::eff(item), StateLoc::Item(m), Arg::Mon(m), Arg::from_opt(source), se, Val::Undef);
            self.pm(m).last_item = item;
            self.pm(m).item = self.d.c.empty;
            let empty = self.d.c.empty;
            Battle::clear_state(&mut self.pm(m).item_state, empty);
            self.pm(m).used_item_this_turn = true;
            self.run_event(Ev::AfterUseItem, Arg::Mon(m), None, EffRef::NONE, Val::Eff(item));
            return true;
        }
        false
    }
    pub fn eat_item(&mut self, m: Mon, force: bool, source: Option<Mon>, source_effect: EffRef) -> bool {
        let item = self.p(m).item;
        if item == self.d.c.empty {
            return false;
        }
        let iid = self.item_id(m);
        if (self.p(m).hp == 0 && iid != "jabocaberry" && iid != "rowapberry") || !self.p(m).is_active {
            return false;
        }
        let se = if source_effect.is_none() { self.effect } else { source_effect };
        let source = source.or(self.event.target.mon());
        if !se.is_none() && !se.is_move() && self.eff(se.eff).effect_type == EffType::Item && item != se.eff && source == Some(m) {
            return false;
        }
        if self.run_event(Ev::UseItem, Arg::Mon(m), None, EffRef::NONE, Val::Eff(item)).truthy()
            && (force || self.run_event(Ev::TryEatItem, Arg::Mon(m), None, EffRef::NONE, Val::Eff(item)).truthy())
        {
            let iname = self.eff(item).name.clone();
            add!(self, "-enditem", m, iname, "[eat]");
            self.single_event(Ev::Eat, EffRef::eff(item), StateLoc::Item(m), Arg::Mon(m), Arg::from_opt(source), se, Val::Undef);
            self.run_event(Ev::EatItem, Arg::Mon(m), source, se, Val::Eff(item));
            self.pm(m).last_item = item;
            self.pm(m).item = self.d.c.empty;
            let empty = self.d.c.empty;
            Battle::clear_state(&mut self.pm(m).item_state, empty);
            self.pm(m).used_item_this_turn = true;
            self.pm(m).ate_berry = true;
            self.run_event(Ev::AfterUseItem, Arg::Mon(m), None, EffRef::NONE, Val::Eff(item));
            return true;
        }
        false
    }

    // ------------------------------------------------------------------
    // abilities
    // ------------------------------------------------------------------
    pub fn set_ability(&mut self, m: Mon, ability: EffId, source: Option<Mon>, source_effect: EffRef, from_forme_change: bool, is_transform: bool) -> Val {
        if self.p(m).hp == 0 {
            return Val::False;
        }
        let se = if source_effect.is_none() { self.effect } else { source_effect };
        let old = self.p(m).ability;
        if !from_forme_change {
            let cant = |e: EffId| self.eff(e).ability.as_ref().map(|a| a.cantsuppress).unwrap_or(false);
            if cant(ability) || cant(old) {
                return Val::False;
            }
        }
        if !from_forme_change && !is_transform {
            let r = self.run_event(Ev::SetAbility, Arg::Mon(m), source, se, Val::Eff(ability));
            if !r.truthy() {
                return r;
            }
        }
        self.single_event(Ev::End, EffRef::eff(old), StateLoc::Ability(m), Arg::Mon(m), Arg::from_opt(source), EffRef::NONE, Val::Undef);
        let st = self.init_state(ability, Holder::Mon(m), None);
        self.pm(m).ability = ability;
        self.pm(m).ability_state = st;
        if !se.is_none() && !from_forme_change && !is_transform {
            let sid = if se.is_none() { "" } else { self.eff(se.eff).id.as_str() };
            let aname = self.eff(ability).name.clone();
            let oname = self.eff(old).name.clone();
            let sfull = self.eff(se.eff).fullname.clone();
            match sid {
                "mummy" | "lingeringaroma" => {
                    if let Some(src) = source {
                        add!(self, "-activate", src, sfull, m, format!("[ability] {oname}"));
                    }
                }
                _ => {
                    if let Some(src) = source {
                        let ss = self.mon_str(src);
                        add!(self, "-ability", m, aname, oname, format!("[from] {sfull}"), format!("[of] {ss}"));
                    } else {
                        add!(self, "-ability", m, aname, oname, format!("[from] {sfull}"));
                    }
                }
            }
        }
        if ability != self.d.c.empty && (!is_transform || old != ability) {
            self.single_event(Ev::Start, EffRef::eff(ability), StateLoc::Ability(m), Arg::Mon(m), Arg::from_opt(source), EffRef::NONE, Val::Undef);
        }
        Val::Eff(old)
    }

    // ------------------------------------------------------------------
    // boosts
    // ------------------------------------------------------------------
    pub fn get_capped_boost(&self, m: Mon, boosts: &SparseBoosts) -> SparseBoosts {
        let mut out = SparseBoosts::default();
        for (k, v) in boosts.iter() {
            if v == 0 {
                continue;
            }
            let cur = self.p(m).boosts[k as usize] as i32;
            out.set(k, ((cur + v as i32).clamp(-6, 6) - cur) as i8);
        }
        out
    }
    pub fn boost_by(&mut self, m: Mon, boosts: &SparseBoosts) -> i32 {
        let b = self.get_capped_boost(m, boosts);
        let mut delta = 0;
        for (k, v) in b.iter() {
            delta = v as i32;
            self.pm(m).boosts[k as usize] += v;
        }
        delta
    }
    pub fn set_boost(&mut self, m: Mon, boosts: &SparseBoosts) {
        for (k, v) in boosts.iter() {
            self.pm(m).boosts[k as usize] = v;
        }
    }
    pub fn positive_boosts(&self, m: Mon) -> i32 {
        self.p(m).boosts.iter().filter(|&&b| b > 0).map(|&b| b as i32).sum()
    }

    // ------------------------------------------------------------------
    // HP
    // ------------------------------------------------------------------
    /// pokemon.damage(d): returns the damage dealt
    pub fn mon_damage(&mut self, m: Mon, d: f64, source: Option<Mon>, effect: EffRef) -> f64 {
        if self.p(m).hp == 0 || d.is_nan() || d <= 0.0 {
            return 0.0;
        }
        let mut d = if d < 1.0 { 1.0 } else { d };
        d = tr(d);
        let p = self.pm(m);
        p.hp -= d as i32;
        if p.hp <= 0 {
            d += p.hp as f64;
            self.faint(m, source, effect);
        }
        d
    }
    /// pokemon.heal(d): returns the HP healed or false (0)
    pub fn mon_heal(&mut self, m: Mon, d: f64) -> Option<i32> {
        if self.p(m).hp == 0 {
            return None;
        }
        let d = tr(d);
        if d.is_nan() || d <= 0.0 {
            return None;
        }
        let p = self.pm(m);
        if p.hp >= p.maxhp {
            return None;
        }
        let mut d = d as i32;
        p.hp += d;
        if p.hp > p.maxhp {
            d -= p.hp - p.maxhp;
            p.hp = p.maxhp;
        }
        Some(d)
    }
    pub fn sethp(&mut self, m: Mon, d: f64) -> i32 {
        if self.p(m).hp == 0 {
            return 0;
        }
        let mut d = tr(d) as i32;
        if d < 1 {
            d = 1;
        }
        let p = self.pm(m);
        d -= p.hp;
        p.hp += d;
        if p.hp > p.maxhp {
            d -= p.hp - p.maxhp;
            p.hp = p.maxhp;
        }
        d
    }
    /// pokemon.faint(): queue the faint
    pub fn faint(&mut self, m: Mon, source: Option<Mon>, effect: EffRef) -> i32 {
        let p = self.pm(m);
        if p.fainted || p.faint_queued {
            return 0;
        }
        let d = p.hp;
        p.hp = 0;
        p.switch_flag = SwitchFlag::No;
        p.faint_queued = true;
        self.faint_queue.push((m, source, effect));
        d
    }

    // ------------------------------------------------------------------
    // species
    // ------------------------------------------------------------------
    /// setSpecies (stats and types)
    pub fn set_species(&mut self, m: Mon, species: u16, source: EffRef, is_transform: bool) -> Option<u16> {
        let r = self.run_event(Ev::ModifySpecies, Arg::Mon(m), None, source, Val::Num(species as f64));
        let sp = match r {
            Val::Num(x) => x as u16,
            Val::Undef | Val::True => species,
            _ => return None,
        };
        let d = self.d;
        let def = &d.species[sp as usize];
        let stats = self.spread_modify(&def.base, &self.p(m).set.clone());
        let p = self.pm(m);
        p.species = sp;
        p.types = def.types.clone();
        p.added_type = NO_TYPE;
        p.apparent_type = d.species[species as usize].types.iter().map(|&t| d.type_name(t).to_string()).collect::<Vec<_>>().join("/");
        p.known_type = true;
        p.weighthg = def.weighthg;
        if p.maxhp == 0 {
            p.base_maxhp = stats[0];
            p.maxhp = stats[0];
            p.hp = stats[0];
        }
        if !is_transform {
            p.base_stored_stats = stats;
        }
        for k in 1..6 {
            p.stored_stats[k] = stats[k];
        }
        p.speed = p.stored_stats[5] as f64;
        Some(sp)
    }

    /// formeChange (the Champions version: Mega Evolutions are not reverted on fainting)
    pub fn forme_change(&mut self, m: Mon, species: u16, source: EffRef, is_permanent: bool, message: Option<&str>) -> bool {
        if self.set_species(m, species, source, false).is_none() {
            return false;
        }
        let d = self.d;
        let def = &d.species[species as usize];
        if is_permanent {
            self.pm(m).base_species = species;
            let det = self.updated_details(m);
            self.pm(m).details = det.clone();
            add!(self, "detailschange", m, det);
            self.update_max_hp(m);
            if source.is_none() {
                self.pm(m).forme_regression = true;
            } else if !source.is_move() && self.eff(source.eff).effect_type == EffType::Item {
                let ri = def.required_item.clone().unwrap_or_default();
                add!(self, "-mega", m, def.base_species.clone(), ri);
                self.pm(m).move_this_turn_result = Val::True;
            } else if !source.is_move() && self.eff(source.eff).effect_type == EffType::Status {
                add!(self, "-formechange", m, def.name.clone(), message.unwrap_or(""));
            }
        } else if !source.is_none() && !source.is_move() && self.eff(source.eff).effect_type == EffType::Ability {
            let an = self.eff(source.eff).name.clone();
            add!(self, "-formechange", m, def.name.clone(), message.unwrap_or(""), format!("[from] ability: {an}"));
        } else {
            add!(self, "-formechange", m, def.name.clone(), message.unwrap_or(""));
        }
        let src_id = if source.is_none() { "" } else { self.eff(source.eff).id.as_str() };
        if is_permanent && (source.is_none() || !matches!(src_id, "disguise" | "iceface")) {
            let ab_name = def.abilities.iter().find(|(k, _)| k == "0").map(|x| x.1.clone()).unwrap_or_default();
            let cant = self.eff(self.p(m).ability).ability.as_ref().map(|a| a.cantsuppress).unwrap_or(false);
            let ab = self.d.ability(&ab_name);
            if !source.is_none() || !cant {
                self.set_ability(m, ab, None, EffRef::NONE, true, false);
            }
            self.pm(m).base_ability = ab;
        }
        true
    }
    pub fn update_max_hp(&mut self, m: Mon) {
        let def = &self.d.species[self.p(m).species as usize];
        let nb = self.stat_modify(&def.base, &self.p(m).set.clone(), 0);
        let p = self.pm(m);
        if nb == p.base_maxhp {
            return;
        }
        p.base_maxhp = nb;
        let new_max = nb;
        p.hp = if p.hp <= 0 { 0 } else { (new_max - (p.maxhp - p.hp)).max(1) };
        p.maxhp = new_max;
        if self.p(m).hp > 0 {
            add!(self, "-heal", m, Health(m), "[silent]");
        }
    }

    /// transformInto(pokemon, effect)
    pub fn transform_into(&mut self, m: Mon, t: Mon, effect: EffRef) -> bool {
        if self.p(t).fainted || self.p(t).hp <= 0 || self.has_vol(t, "substitute") || self.p(t).transformed || self.p(m).transformed {
            return false;
        }
        let sp = self.p(t).species;
        if self.set_species(m, sp, effect, true).is_none() {
            return false;
        }
        // the slots it had keep their PP for when it switches out (Showdown: baseMoveSlots)
        let cur = self.p(m).move_slots.clone();
        self.pm(m).base_move_slots = cur;
        let types = self.get_types(t, true);
        self.set_type(m, types, true);
        let (added, w, ta) = (self.p(t).added_type, self.p(t).weighthg, self.p(t).times_attacked);
        let stats = self.p(t).stored_stats;
        let boosts = self.p(t).boosts;
        let d = self.d;
        let slots: Vec<MoveSlot> = self.p(t).move_slots.iter().map(|ms| {
            let pp = (d.moves[ms.id as usize].pp as i32).min(5);
            MoveSlot { id: ms.id, pp, maxpp: pp, target: ms.target, disabled: 0, disabled_source: None, used: false, virtual_: true }
        }).collect();
        let p = self.pm(m);
        p.transformed = true;
        p.weighthg = w;
        p.added_type = added;
        for k in 1..6 {
            p.stored_stats[k] = stats[k];
        }
        p.times_attacked = ta;
        p.move_slots = slots;
        p.boosts = boosts;
        for v in ["dragoncheer", "focusenergy", "laserfocus"] {
            self.remove_volatile_id(m, v);
        }
        for v in ["dragoncheer", "focusenergy", "laserfocus"] {
            if self.has_vol(t, v) {
                if let Some(c) = self.d.cond(v) {
                    self.add_volatile(m, c, None, EffRef::NONE, None);
                    if v == "dragoncheer" {
                        let hd = self.p(t).volatiles.iter().find(|x| x.0 == c).map(|x| x.1.get(K::HasDragonType)).unwrap_or(Val::Undef);
                        if let Some(st) = self.pm(m).volatiles.iter_mut().find(|x| x.0 == c) {
                            st.1.set(K::HasDragonType, hd);
                        }
                    }
                }
            }
        }
        if effect.is_none() {
            crate::add!(self, "-transform", m, t);
        } else {
            let full = self.eff(effect.eff).fullname.clone();
            crate::add!(self, "-transform", m, t, format!("[from] {full}"));
        }
        let ab = self.p(t).ability;
        self.set_ability(m, ab, Some(m), EffRef::NONE, true, true);
        true
    }

    /// clearVolatile (Champions version: also resets timesAttacked)
    pub fn clear_volatile(&mut self, m: Mon, include_switch_flags: bool) {
        let p = self.pm(m);
        p.boosts = [0; 7];
        // moveSlots = baseMoveSlots.slice(): a new array of the SAME slot objects, so PP and
        // disabled flags carry over (no Transform in the format: the slots are the base slots)
        if p.transformed {
            p.move_slots = p.base_move_slots.clone();
        }
        p.transformed = false;
        p.ability = p.base_ability;
        let vols: Vec<(EffId, Vec<Mon>, Option<EffId>)> = p.volatiles.iter().filter(|v| v.1.linked_status.is_some()).map(|v| (v.0, v.1.linked.clone(), v.1.linked_status)).collect();
        for (_, linked, ls) in vols {
            if let Some(ls) = ls {
                self.remove_linked_volatiles(m, ls, &linked);
            }
        }
        let p = self.pm(m);
        p.volatiles.clear();
        if include_switch_flags {
            p.switch_flag = SwitchFlag::No;
            p.force_switch_flag = false;
        }
        p.last_move = None;
        p.last_move_used = None;
        p.move_this_turn = None;
        p.move_last_turn_result = Val::Undef;
        p.move_this_turn_result = Val::Undef;
        p.last_damage = 0;
        p.attacked_by.clear();
        p.hurt_this_turn = None;
        p.newly_switched = true;
        p.being_called_back = false;
        p.times_attacked = 0;
        p.ability_state.del(K::Started);
        p.item_state.del(K::Started);
        let bs = p.base_species;
        self.set_species(m, bs, EffRef::NONE, false);
    }

    // ------------------------------------------------------------------
    // moves
    // ------------------------------------------------------------------
    pub fn has_move(&self, m: Mon, move_id: u16) -> bool {
        self.p(m).move_slots.iter().any(|s| s.id == move_id)
    }
    pub fn disable_move(&mut self, m: Mon, move_id: u16, hidden: bool, source: Option<String>) {
        let src = source.or_else(|| if self.effect.is_none() { None } else { Some(self.eff(self.effect.eff).name.clone()) });
        let d = self.d;
        for slot in self.pm(m).move_slots.iter_mut() {
            if slot.id == move_id && slot.disabled != 1 {
                slot.disabled = if hidden { 2 } else { 1 };
                slot.disabled_source = Some(src.clone().unwrap_or_else(|| d.moves[move_id as usize].name.clone()));
            }
        }
    }
    pub fn deduct_pp(&mut self, m: Mon, move_id: u16, amount: Option<i32>) -> i32 {
        let Some(slot) = self.pm(m).move_slots.iter_mut().find(|s| s.id == move_id) else { return 0 };
        slot.used = true;
        if slot.pp == 0 {
            return 0;
        }
        let mut amount = amount.filter(|&a| a != 0).unwrap_or(1);
        slot.pp -= amount;
        if slot.pp < 0 {
            amount += slot.pp;
            slot.pp = 0;
        }
        amount
    }
    pub fn move_used(&mut self, m: Mon, move_id: u16, target_loc: Option<i8>) {
        let p = self.pm(m);
        p.last_move = Some(move_id);
        p.last_move_target_loc = target_loc;
        p.move_this_turn = Some(move_id);
    }
    pub fn got_attacked(&mut self, m: Mon, move_id: u16, damage: Val, source: Mon) {
        let dn = if let Val::Num(x) = damage { x as i32 } else { 0 };
        let slot = (source.s, self.p(source).position);
        self.pm(m).attacked_by.push(Attacker { source, damage: dn, this_turn: true, move_id, slot, damage_value: damage });
    }
    pub fn get_last_attacked_by(&self, m: Mon) -> Option<&Attacker> {
        self.p(m).attacked_by.last()
    }
    pub fn try_trap(&mut self, m: Mon, hidden: bool) -> bool {
        if !self.run_status_immunity(m, "trapped", false) {
            return false;
        }
        if self.p(m).trapped != 0 && hidden {
            return true;
        }
        self.pm(m).trapped = if hidden { 2 } else { 1 };
        true
    }
    /// getLockedMove: priorityEvent('LockMove')
    pub fn get_locked_move(&mut self, m: Mon) -> Option<u16> {
        match self.priority_event(Ev::LockMove, Arg::Mon(m), None, EffRef::NONE, Val::Undef) {
            Val::MoveId(x) => Some(x),
            Val::Eff(e) => {
                let ef = self.eff(e);
                if ef.move_def != u16::MAX { Some(ef.move_def) } else { None }
            }
            _ => None,
        }
    }
}

impl Battle {
    /// pokemon.setType(types, enforce)
    pub fn set_type(&mut self, m: Mon, types: Vec<TypeId>, enforce: bool) -> bool {
        if !enforce {
            let num = self.d.species[self.p(m).species as usize].num;
            if num == 493 || num == 773 {
                return false;
            }
        }
        let d = self.d;
        let p = self.pm(m);
        p.apparent_type = types.iter().map(|&t| d.type_name(t).to_string()).collect::<Vec<_>>().join("/");
        p.types = types;
        p.added_type = NO_TYPE;
        p.known_type = true;
        true
    }

    /// Battle.swapPosition
    pub fn swap_position(&mut self, m: Mon, new_pos: u8, attrs: &str) -> bool {
        let s = m.s as usize;
        let target = self.active_at(s, new_pos as usize);
        if new_pos != 1 && target.map(|t| self.p(t).fainted).unwrap_or(true) {
            return false;
        }
        add!(self, "swap", m, new_pos as i32, attrs);
        let old_pos = self.p(m).position;
        let side = &mut self.sides[s];
        side.pokemon[old_pos as usize] = target.map(|t| t.i).unwrap_or(side.pokemon[old_pos as usize]);
        side.pokemon[new_pos as usize] = m.i;
        side.active[old_pos as usize] = target.map(|t| t.i);
        side.active[new_pos as usize] = Some(m.i);
        if let Some(t) = target {
            self.pm(t).position = old_pos;
        }
        self.pm(m).position = new_pos;
        if let Some(t) = target {
            self.run_event(Ev::Swap, Arg::Mon(t), Some(m), EffRef::NONE, Val::Undef);
        }
        self.run_event(Ev::Swap, Arg::Mon(m), target, EffRef::NONE, Val::Undef);
        true
    }

    /// Battle.skillSwap(source, target)
    pub fn skill_swap(&mut self, source: Mon, target: Mon) -> Val {
        if self.p(source).fainted || self.p(target).fainted {
            return Val::False;
        }
        let sa = self.p(source).ability;
        let ta = self.p(target).ability;
        let fs = |e: EffId| self.eff(e).ability.as_ref().map(|a| a.failskillswap).unwrap_or(false);
        if fs(sa) || fs(ta) {
            return Val::False;
        }
        let ss = self.d.cond("skillswap").map(EffRef::eff).unwrap_or(EffRef::NONE);
        let r = self.run_event(Ev::SetAbility, Arg::Mon(target), Some(source), ss, Val::Eff(sa));
        if !r.truthy() {
            return r;
        }
        let r = self.run_event(Ev::SetAbility, Arg::Mon(source), Some(source), ss, Val::Eff(ta));
        if !r.truthy() {
            return r;
        }
        if source.s == target.s {
            let ts = self.mon_str(target);
            add!(self, "-activate", source, "Skill Swap", "", "", format!("[of] {ts}"));
        } else {
            let (tn, sn) = (self.eff(ta).name.clone(), self.eff(sa).name.clone());
            let ts = self.mon_str(target);
            add!(self, "-activate", source, "Skill Swap", tn, sn, format!("[of] {ts}"));
        }
        self.single_event(Ev::End, EffRef::eff(sa), StateLoc::Ability(source), Arg::Mon(source), Arg::None, EffRef::NONE, Val::Undef);
        self.single_event(Ev::End, EffRef::eff(ta), StateLoc::Ability(target), Arg::Mon(target), Arg::None, EffRef::NONE, Val::Undef);
        self.pm(source).ability = ta;
        self.pm(target).ability = sa;
        let st = self.init_state(ta, Holder::Mon(source), None);
        self.pm(source).ability_state = st;
        let st = self.init_state(sa, Holder::Mon(target), None);
        self.pm(target).ability_state = st;
        self.single_event(Ev::Start, EffRef::eff(sa), StateLoc::Ability(target), Arg::Mon(target), Arg::None, EffRef::NONE, Val::Undef);
        self.single_event(Ev::Start, EffRef::eff(ta), StateLoc::Ability(source), Arg::Mon(source), Arg::None, EffRef::NONE, Val::Undef);
        Val::Undef
    }
}
