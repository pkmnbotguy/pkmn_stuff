//! Battle flow (`sim/battle.ts`, `sim/battle-queue.ts`, `sim/side.ts`, `sim/field.ts` and the
//! switch parts of `sim/battle-actions.ts`): setup, team preview, requests and choices, the
//! action queue, the turn loop, switching, fainting, damage / heal / boost, field effects.

use super::battle::*;
use super::data::*;
use super::events::Ev;
use super::prng::Prng;
use super::state::*;
use super::val::{Mon, Val};
use crate::add;

impl Battle {
    /// new Battle({formatid, seed}) + setPlayer p1 + setPlayer p2 (which starts the battle:
    /// team preview request).
    pub fn new(d: &'static Data, seed: [u16; 4], teams: [&[SetData]; 2], names: [&str; 2]) -> Result<Battle, String> {
        Battle::new_opts(d, seed, teams, names, false, false)
    }

    /// `trace`: record every random number drawn; `log`: keep the battle log
    pub fn new_opts(d: &'static Data, seed: [u16; 4], teams: [&[SetData]; 2], names: [&str; 2], trace: bool, log: bool) -> Result<Battle, String> {
        let empty = d.c.empty;
        let mut b = Battle {
            d,
            sides: [Battle::empty_side(0, names[0], empty), Battle::empty_side(1, names[1], empty)],
            field: Field {
                weather: empty,
                weather_state: EffectState { id: empty, ..Default::default() },
                terrain: empty,
                terrain_state: EffectState { id: empty, ..Default::default() },
                pseudo_weather: vec![],
            },
            prng: Prng::from_words(seed),
            queue: vec![],
            faint_queue: vec![],
            request_state: RequestState::None,
            turn: 0,
            mid_turn: false,
            started: false,
            ended: false,
            winner: None,
            effect: EffRef::NONE,
            effect_state: StateLoc::None,
            event: EventCtx::EMPTY,
            event_depth: 0,
            active_move: None,
            active_pokemon: None,
            active_target: None,
            last_move: None,
            use_move_undef_target: false,
            last_successful_move_this_turn: None,
            effect_order: 0.0,
            speed_order: vec![0, 1, 2, 3],
            moves: vec![],
            temp_states: vec![],
            log: None,
            last_move_line: -1,
            uid: 0,
            missing: vec![],
            hints: vec![],
        };
        b.field.weather_state.uid = b.next_uid();
        b.field.terrain_state.uid = b.next_uid();
        if trace {
            b.prng.trace = Some(vec![]);
        }
        if log {
            b.log = Some(vec![]);
        }
        for s in 0..2u8 {
            b.add_side_team(s, teams[s as usize])?;
        }
        b.start();
        Ok(b)
    }

    pub fn enable_log(&mut self) {
        self.log = Some(vec![]);
    }

    fn empty_side(n: u8, name: &str, empty: EffId) -> Side {
        let _ = empty;
        Side {
            n,
            name: name.to_string(),
            mons: vec![],
            pokemon: vec![],
            active: [None, None],
            pokemon_left: 0,
            fainted_last_turn: None,
            fainted_this_turn: None,
            total_fainted: 0,
            side_conditions: vec![],
            slot_conditions: [vec![], vec![]],
            request: RequestState::None,
            force_switch: [false; 2],
            choice: Choice::default(),
        }
    }

    /// new Side(...): one Pokemon per set, in order (each may draw a random gender)
    fn add_side_team(&mut self, s: u8, team: &[SetData]) -> Result<(), String> {
        for (ti, set) in team.iter().enumerate() {
            let mut p = self.make_pokemon(set, s, ti as u8)?;
            // gender: set gender, else species gender, else random
            let sp_gender = self.d.species[p.species as usize].gender.clone();
            let g: &'static str = match set.gender.as_str() {
                "M" => "M",
                "F" => "F",
                "N" => "N",
                _ => match sp_gender.as_str() {
                    "M" => "M",
                    "F" => "F",
                    "N" => "N",
                    _ => {
                        if self.prng.random(2) == 0 { "M" } else { "F" }
                    }
                },
            };
            p.gender = if g == "N" { "" } else { g };
            self.sides[s as usize].mons.push(p);
            self.sides[s as usize].pokemon.push(ti as u8);
            self.sides[s as usize].pokemon_left += 1;
            self.finish_pokemon(Mon { s, i: ti as u8 });
        }
        Ok(())
    }

    /// Battle.start(): runPickTeam (team preview), then the 'start' action waits in the queue
    fn start(&mut self) {
        self.started = true;
        // Team Preview rule's onTeamPreview
        add!(self, "clearpoke");
        for s in 0..2 {
            for k in 0..self.sides[s].pokemon.len() {
                let m = Mon { s: s as u8, i: self.sides[s].pokemon[k] };
                let det = self.p(m).details.replace(", shiny", "");
                add!(self, "poke", format!("p{}", s + 1), det, "");
            }
        }
        self.make_request(RequestState::TeamPreview);
        self.queue_add_choice(Action::new(ActionKind::Start));
        self.mid_turn = true;
    }

    // ------------------------------------------------------------------
    // requests
    // ------------------------------------------------------------------
    pub fn make_request(&mut self, typ: RequestState) {
        self.request_state = typ;
        for s in 0..2 {
            self.clear_choice(s);
        }
        if typ == RequestState::TeamPreview {
            add!(self, "teampreview", "4");
        }
        for s in 0..2usize {
            self.sides[s].request = RequestState::None;
            self.sides[s].force_switch = [false; 2];
            match typ {
                RequestState::Switch => {
                    if self.sides[s].pokemon_left == 0 {
                        continue;
                    }
                    let mut table = [false; 2];
                    for pos in 0..2 {
                        if let Some(m) = self.active_at(s, pos) {
                            table[pos] = self.p(m).switch_flag.is_set();
                        }
                    }
                    if table.iter().any(|&x| x) {
                        self.sides[s].request = RequestState::Switch;
                        self.sides[s].force_switch = table;
                    }
                }
                RequestState::TeamPreview => self.sides[s].request = RequestState::TeamPreview,
                RequestState::Move => {
                    if self.sides[s].pokemon_left == 0 {
                        continue;
                    }
                    for pos in 0..2 {
                        if let Some(m) = self.active_at(s, pos) {
                            self.get_move_request_data(m);
                        }
                    }
                    self.sides[s].request = RequestState::Move;
                }
                RequestState::None => {}
            }
        }
    }

    pub fn clear_request(&mut self) {
        self.request_state = RequestState::None;
        for s in 0..2 {
            self.sides[s].request = RequestState::None;
            self.clear_choice(s);
        }
    }

    fn clear_choice(&mut self, s: usize) {
        let mut forced_switches = 0;
        let mut forced_passes = 0;
        if self.request_state == RequestState::Switch {
            let can_out = (0..2).filter(|&pos| self.active_at(s, pos).map(|m| self.p(m).switch_flag.is_set()).unwrap_or(false)).count() as i32;
            let can_in = self.sides[s].pokemon[2.min(self.sides[s].pokemon.len())..]
                .iter()
                .filter(|&&i| !self.p(Mon { s: s as u8, i }).fainted)
                .count() as i32;
            forced_switches = can_out.min(can_in);
            forced_passes = can_out - forced_switches;
        }
        self.sides[s].choice = Choice { forced_switches_left: forced_switches, forced_passes_left: forced_passes, ..Default::default() };
    }

    /// pokemon.getMoves(lockedMove, restrictData): usable move ids (empty = must Struggle)
    pub fn get_moves_list(&self, m: Mon, locked: Option<u16>) -> Vec<(u16, bool)> {
        if let Some(l) = locked {
            return vec![(l, false)];
        }
        let mut out = vec![];
        let mut has_valid = false;
        for slot in &self.p(m).move_slots {
            let mut disabled = slot.disabled != 0;
            if slot.pp <= 0 {
                disabled = true;
            }
            if slot.disabled == 2 {
                // 'hidden': restrictData decides; for choosing, hidden-disabled moves count as disabled
                disabled = true;
            }
            if !disabled {
                has_valid = true;
            }
            out.push((slot.id, disabled));
        }
        if has_valid { out } else { vec![] }
    }

    /// getMoveRequestData: the parts that change state (trapped flags)
    pub fn get_move_request_data(&mut self, m: Mon) {
        let locked = self.get_locked_move(m);
        let hard_locked = locked.is_some();
        if hard_locked {
            self.pm(m).trapped = 1;
        }
        let is_last_active = self.is_last_active(m);
        if hard_locked || !is_last_active {
            let p = self.pm(m);
            p.maybe_disabled = false;
            p.maybe_locked = false;
            p.maybe_trapped = false;
        } else {
            let p = self.pm(m);
            p.maybe_locked = p.maybe_locked || p.maybe_disabled;
        }
    }
    pub fn is_last_active(&self, m: Mon) -> bool {
        let p = self.p(m);
        if !p.is_active {
            return false;
        }
        for pos in (p.position as usize + 1)..2 {
            if let Some(o) = self.active_at(m.s as usize, pos) {
                if !self.p(o).fainted {
                    return false;
                }
            }
        }
        true
    }

    // ------------------------------------------------------------------
    // choices (side.ts)
    // ------------------------------------------------------------------
    /// Battle.choose(sideid, input). Returns false on an invalid choice.
    pub fn choose(&mut self, s: usize, input: &str) -> bool {
        if !self.side_choose(s, input) {
            return false;
        }
        if !self.is_choice_done(s) {
            return false;
        }
        if self.all_choices_done() {
            self.commit_choices();
        }
        true
    }

    pub fn all_choices_done(&mut self) -> bool {
        (0..2).filter(|&s| self.is_choice_done(s)).count() >= 2
    }

    /// side.isChoiceDone() (for drivers)
    pub fn is_choice_done_pub(&mut self, s: usize) -> bool {
        self.sides[s].choice.actions.len() >= 2 && self.is_choice_done(s)
    }

    pub fn is_choice_done(&mut self, s: usize) -> bool {
        if self.sides[s].request == RequestState::None {
            return true;
        }
        if self.sides[s].choice.forced_switches_left > 0 {
            return false;
        }
        if self.sides[s].request == RequestState::TeamPreview {
            return self.sides[s].choice.actions.len() >= 4.min(self.sides[s].pokemon.len());
        }
        self.get_choice_index(s, false);
        self.sides[s].choice.actions.len() >= 2
    }

    fn get_choice_index(&mut self, s: usize, is_pass: bool) -> usize {
        let mut index = self.sides[s].choice.actions.len();
        if !is_pass {
            match self.sides[s].request {
                RequestState::Move => {
                    while index < 2 {
                        let Some(m) = self.active_at(s, index) else { break };
                        if !(self.p(m).fainted || self.has_vol(m, "commanding")) {
                            break;
                        }
                        self.choose_pass(s);
                        index += 1;
                    }
                }
                RequestState::Switch => {
                    while index < 2 {
                        let flag = self.active_at(s, index).map(|m| self.p(m).switch_flag.is_set()).unwrap_or(false);
                        if flag {
                            break;
                        }
                        self.choose_pass(s);
                        index += 1;
                    }
                }
                _ => {}
            }
        }
        index
    }

    fn choose_pass(&mut self, s: usize) -> bool {
        let index = self.get_choice_index(s, true);
        if index >= 2 {
            return false;
        }
        let m = self.active_at(s, index);
        match self.sides[s].request {
            RequestState::Switch => {
                if let Some(m) = m {
                    if self.p(m).switch_flag.is_set() {
                        if self.sides[s].choice.forced_passes_left == 0 {
                            return self.choice_error(s, "Can't pass: You need to switch in a Pokémon");
                        }
                        self.sides[s].choice.forced_passes_left -= 1;
                    }
                }
            }
            RequestState::Move => {
                if let Some(m) = m {
                    if !self.p(m).fainted && !self.has_vol(m, "commanding") {
                        return self.choice_error(s, "Can't pass: Your Pokémon must make a move (or switch)");
                    }
                }
            }
            _ => return self.choice_error(s, "Can't pass: Not a move or switch request"),
        }
        self.sides[s].choice.actions.push(ChosenAction {
            choice: ActionKind::Pass,
            pokemon: None,
            target_loc: 0,
            move_id: NONE16,
            move_slot: None,
            mega: false,
            target: None,
            index: 0,
            priority: 0.0,
        });
        true
    }

    fn choice_error(&mut self, s: usize, msg: &str) -> bool {
        self.sides[s].choice.error = msg.to_string();
        false
    }

    fn side_choose(&mut self, s: usize, input: &str) -> bool {
        if self.sides[s].request == RequestState::None {
            return self.choice_error(s, "Can't do anything: It's not your turn");
        }
        self.clear_choice(s);
        let parts: Vec<String> = if input.starts_with("team ") { vec![input.to_string()] } else { input.split(',').map(|x| x.trim().to_string()).collect() };
        if parts.len() > 2 {
            return self.choice_error(s, "Can't make choices: too many");
        }
        for part in parts {
            let (typ, data) = match part.find(' ') {
                Some(p) => (part[..p].to_string(), part[p + 1..].trim().to_string()),
                None => (part.clone(), String::new()),
            };
            let ok = match typ.as_str() {
                "move" => {
                    let mut data = data.clone();
                    let mut target_loc: i8 = 0;
                    let mut mega = false;
                    loop {
                        let bytes = data.as_bytes();
                        let n = bytes.len();
                        if n >= 2 && bytes[n - 1].is_ascii_digit() && (bytes[n - 1] - b'0') <= 3 && bytes[n - 1] != b'0' {
                            let pre = &data[..n - 1];
                            if pre.ends_with(" -") || pre.ends_with(" +") {
                                target_loc = data[n - 2..].parse().unwrap_or(0);
                                data = data[..n - 2].trim().to_string();
                                continue;
                            } else if pre.ends_with(' ') {
                                target_loc = data[n - 1..].parse().unwrap_or(0);
                                data = data[..n - 1].trim().to_string();
                                continue;
                            }
                        }
                        if data.ends_with(" mega") {
                            mega = true;
                            data = data[..data.len() - 5].to_string();
                            continue;
                        }
                        break;
                    }
                    self.choose_move(s, &data, target_loc, mega)
                }
                "switch" => self.choose_switch(s, &data),
                "team" => self.choose_team(s, &data),
                "pass" | "skip" => self.choose_pass(s),
                "default" | "auto" => self.auto_choose(s),
                _ => self.choice_error(s, "Unrecognized choice"),
            };
            if !ok {
                return false;
            }
        }
        self.sides[s].choice.error.is_empty()
    }

    fn auto_choose(&mut self, s: usize) -> bool {
        match self.sides[s].request {
            RequestState::TeamPreview => {
                if !self.is_choice_done(s) {
                    self.choose_team(s, "");
                }
            }
            RequestState::Switch => {
                let mut i = 0;
                while !self.is_choice_done(s) {
                    if !self.choose_switch(s, "") {
                        return false;
                    }
                    i += 1;
                    if i > 10 {
                        return false;
                    }
                }
            }
            RequestState::Move => {
                let mut i = 0;
                while !self.is_choice_done(s) {
                    if !self.choose_move(s, "", 0, false) {
                        return false;
                    }
                    i += 1;
                    if i > 10 {
                        return false;
                    }
                }
            }
            RequestState::None => {}
        }
        true
    }

    fn choose_team(&mut self, s: usize, data: &str) -> bool {
        if self.sides[s].request != RequestState::TeamPreview {
            return self.choice_error(s, "not team preview");
        }
        let n = self.sides[s].pokemon.len();
        let mut positions: Vec<usize> = if data.is_empty() {
            (0..n).collect()
        } else if data.contains(',') {
            data.split(',').filter_map(|x| x.trim().parse::<usize>().ok()).map(|x| x - 1).collect()
        } else {
            data.chars().filter_map(|c| c.to_digit(10)).map(|x| x as usize - 1).collect()
        };
        let picked = 4.min(n);
        positions.truncate(picked);
        if positions.len() < picked {
            for i in 0..picked {
                if !positions.contains(&i) {
                    positions.push(i);
                }
                if positions.len() >= picked {
                    break;
                }
            }
        }
        for (index, &pos) in positions.iter().enumerate() {
            if pos >= n {
                return self.choice_error(s, "no Pokemon in that slot");
            }
            self.sides[s].choice.switch_ins.push(pos as u8);
            let ti = self.sides[s].pokemon[pos];
            self.sides[s].choice.actions.push(ChosenAction {
                choice: ActionKind::Team,
                pokemon: Some(Mon { s: s as u8, i: ti }),
                target_loc: 0,
                move_id: NONE16,
                move_slot: None,
                mega: false,
                target: None,
                index: index as u8,
                priority: -(index as f64),
            });
        }
        true
    }

    fn choose_switch(&mut self, s: usize, slot_text: &str) -> bool {
        let rs = self.sides[s].request;
        if rs != RequestState::Move && rs != RequestState::Switch {
            return self.choice_error(s, "Can't switch now");
        }
        let index = self.get_choice_index(s, false);
        if index >= 2 {
            return self.choice_error(s, "Can't switch: too many choices");
        }
        let Some(pm) = self.active_at(s, index) else { return self.choice_error(s, "no active Pokemon") };
        let reviving = self.has_revival(s, self.p(pm).position);
        let slot: usize = if slot_text.is_empty() {
            if rs != RequestState::Switch {
                return self.choice_error(s, "Can't switch: You need to select a Pokémon to switch in");
            }
            if reviving {
                let mut slot = 0;
                while !self.p(Mon { s: s as u8, i: self.sides[s].pokemon[slot] }).fainted {
                    slot += 1;
                }
                slot
            } else {
                if self.sides[s].choice.forced_switches_left == 0 {
                    return self.choose_pass(s);
                }
                let mut slot = 2;
                while self.sides[s].choice.switch_ins.contains(&(slot as u8)) || self.p(Mon { s: s as u8, i: self.sides[s].pokemon[slot] }).fainted {
                    slot += 1;
                }
                slot
            }
        } else {
            match slot_text.parse::<usize>() {
                Ok(x) if x >= 1 => x - 1,
                _ => return self.choice_error(s, "bad switch slot"),
            }
        };
        if slot >= self.sides[s].pokemon.len() {
            return self.choice_error(s, "no Pokemon in that slot");
        } else if slot < 2 && !reviving {
            return self.choice_error(s, "Can't switch to an active Pokémon");
        } else if self.sides[s].choice.switch_ins.contains(&(slot as u8)) {
            return self.choice_error(s, "can only switch in once");
        }
        let target = Mon { s: s as u8, i: self.sides[s].pokemon[slot] };
        if reviving {
            if !self.p(target).fainted {
                return self.choice_error(s, "Can't switch: You have to pass to a fainted Pokémon");
            }
            let left = self.sides[s].choice.forced_switches_left;
            self.sides[s].choice.forced_switches_left = (left - 1).max(0);
            self.pm(pm).switch_flag = SwitchFlag::No;
            self.sides[s].choice.actions.push(ChosenAction {
                choice: ActionKind::RevivalBlessing,
                pokemon: Some(pm),
                target_loc: 0,
                move_id: NONE16,
                move_slot: None,
                mega: false,
                target: Some(target),
                index: 0,
                priority: 0.0,
            });
            return true;
        }
        if self.p(target).fainted {
            return self.choice_error(s, "Can't switch to a fainted Pokémon");
        }
        if rs == RequestState::Move {
            if self.p(pm).trapped != 0 {
                return self.choice_error(s, "Can't switch: The active Pokémon is trapped");
            }
        } else {
            if self.sides[s].choice.forced_switches_left == 0 {
                return self.choice_error(s, "switched too many");
            }
            self.sides[s].choice.forced_switches_left -= 1;
        }
        self.sides[s].choice.switch_ins.push(slot as u8);
        self.sides[s].choice.actions.push(ChosenAction {
            choice: if rs == RequestState::Switch { ActionKind::InstaSwitch } else { ActionKind::Switch },
            pokemon: Some(pm),
            target_loc: 0,
            move_id: NONE16,
            move_slot: None,
            mega: false,
            target: Some(target),
            index: 0,
            priority: 0.0,
        });
        true
    }

    fn choose_move(&mut self, s: usize, move_text: &str, target_loc: i8, mega: bool) -> bool {
        if self.sides[s].request != RequestState::Move {
            return self.choice_error(s, "Can't move now");
        }
        let index = self.get_choice_index(s, false);
        if index >= 2 {
            return self.choice_error(s, "Can't move: more choices than Pokemon");
        }
        let auto = move_text.is_empty();
        let pm = self.active_at(s, index).unwrap();
        // request data: the move list the player sees
        let locked = self.get_locked_move(pm);
        // a locked move's request entry has no target (so no target can be chosen)
        let mut req_moves: Vec<(u16, bool, MoveTarget)> = match locked {
            Some(l) => vec![(l, false, MoveTarget::Undefined)],
            None => self.get_moves_list(pm, None).into_iter().map(|(id, dis)| (id, dis, self.slot_target(pm, id))).collect(),
        };
        if req_moves.is_empty() {
            let st = self.d.c.struggle_move;
            req_moves = vec![(st, false, MoveTarget::RandomNormal)];
        }

        let (mut move_slot, mut moveid, mut target_type) = (0usize, NONE16, MoveTarget::Normal);
        if auto || move_text.chars().all(|c| c.is_ascii_digit()) {
            let mi = if auto { 0 } else { move_text.parse::<usize>().unwrap_or(0).saturating_sub(1) };
            if mi >= req_moves.len() {
                return self.choice_error(s, "doesn't have that move");
            }
            move_slot = mi;
            moveid = req_moves[mi].0;
            target_type = req_moves[mi].2;
        } else {
            let want = to_id(move_text);
            for (k, mvd) in req_moves.iter().enumerate() {
                if self.d.moves[mvd.0 as usize].id == want {
                    move_slot = k;
                    moveid = mvd.0;
                    // `move.target || 'normal'`: a locked move chosen by name counts as 'normal'
                    target_type = if mvd.2 == MoveTarget::Undefined { MoveTarget::Normal } else { mvd.2 };
                    break;
                }
            }
            if moveid == NONE16 {
                return self.choice_error(s, "doesn't have a move matching that");
            }
        }
        let moves = self.get_moves_list(pm, None);
        if auto {
            for (k, mvd) in req_moves.iter().enumerate() {
                if mvd.1 {
                    continue;
                }
                if k < moves.len() && mvd.0 == moves[k].0 && moves[k].1 {
                    continue;
                }
                moveid = mvd.0;
                move_slot = k;
                target_type = mvd.2;
                break;
            }
        }
        let mut target_loc = target_loc;
        if auto {
            target_loc = 0;
        } else if target_type.choosable() {
            if target_loc == 0 {
                return self.choice_error(s, "needs a target");
            }
            if !self.valid_target_loc(target_loc, pm, target_type) {
                return self.choice_error(s, "Invalid target");
            }
        } else if target_loc != 0 {
            return self.choice_error(s, "You can't choose a target for that move");
        }
        let locked2 = locked.or(None);
        if let Some(l) = locked2 {
            let mut tl = self.p(pm).last_move_target_loc.unwrap_or(0);
            let lid = self.d.moves[l as usize].id.clone();
            if let Some(c) = self.d.cond(&lid) {
                if let Some(v) = self.volatile(pm, c) {
                    if let Val::Num(x) = v.get(K::TargetLoc) {
                        tl = x as i8;
                    }
                }
            }
            self.sides[s].choice.actions.push(ChosenAction {
                choice: ActionKind::Move,
                pokemon: Some(pm),
                target_loc: tl,
                move_id: l,
                move_slot: None,
                mega: false,
                target: None,
                index: 0,
                priority: 0.0,
            });
            return true;
        } else if moves.is_empty() {
            self.sides[s].choice.actions.push(ChosenAction {
                choice: ActionKind::Move,
                pokemon: Some(pm),
                target_loc: 0,
                move_id: self.d.c.struggle_move,
                move_slot: None,
                mega: false,
                target: None,
                index: 0,
                priority: 0.0,
            });
            return true;
        } else {
            let enabled = moves.iter().any(|(id, dis)| *id == moveid && !dis);
            if !enabled {
                return self.choice_error(s, "move is disabled");
            }
        }
        if mega && self.p(pm).can_mega_evo.is_none() {
            return self.choice_error(s, "can't mega evolve");
        }
        if mega && self.sides[s].choice.mega {
            return self.choice_error(s, "You can only mega-evolve once per battle");
        }
        self.sides[s].choice.actions.push(ChosenAction {
            choice: ActionKind::Move,
            pokemon: Some(pm),
            target_loc,
            move_id: moveid,
            move_slot: Some(move_slot as u8),
            mega,
            target: None,
            index: 0,
            priority: 0.0,
        });
        if mega {
            self.sides[s].choice.mega = true;
        }
        true
    }

    /// the move target shown in the request (Curse is 'self' for non-Ghosts)
    fn slot_target(&mut self, m: Mon, id: u16) -> MoveTarget {
        let md = &self.d.moves[id as usize];
        if md.id == "curse" && !self.has_type(m, "Ghost") {
            return MoveTarget::SelfTarget;
        }
        if md.id == "pollenpuff" && self.has_vol(m, "healblock") {
            return MoveTarget::AdjacentFoe;
        }
        self.p(m).move_slots.iter().find(|s| s.id == id).map(|s| s.target).unwrap_or(md.target)
    }

    /// Battle.commitChoices
    pub fn commit_choices(&mut self) {
        self.update_all_speed();
        let old_queue = std::mem::take(&mut self.queue);
        for s in 0..2 {
            let actions = self.sides[s].choice.actions.clone();
            for a in actions {
                self.queue_add_chosen(a);
            }
        }
        self.clear_request();
        let mut q = std::mem::take(&mut self.queue);
        self.speed_sort(&mut q);
        q.extend(old_queue);
        self.queue = q;
        self.request_state = RequestState::None;
        for s in 0..2 {
            self.sides[s].request = RequestState::None;
        }
        self.turn_loop();
    }

    fn queue_add_chosen(&mut self, c: ChosenAction) {
        let mut a = Action::new(c.choice);
        a.pokemon = c.pokemon;
        a.target_loc = c.target_loc;
        a.move_id = c.move_id;
        a.mega = c.mega as u8;
        a.target = c.target;
        a.index = c.index;
        a.priority = c.priority;
        a.side = c.pokemon.map(|m| m.s).unwrap_or(0);
        self.queue_add_choice(a);
    }

    // ------------------------------------------------------------------
    // the queue (battle-queue.ts)
    // ------------------------------------------------------------------
    pub fn queue_add_choice(&mut self, a: Action) {
        let resolved = self.resolve_action(a, false);
        self.queue.extend(resolved);
    }

    /// dex.getActiveMove(id)
    pub fn new_active_move(&mut self, move_id: u16) -> u16 {
        let md = &self.d.moves[move_id as usize];
        let am = ActiveMove {
            def: move_id,
            eff: md.eff,
            type_: md.type_,
            category: md.category,
            base_power: md.base_power,
            accuracy: md.accuracy,
            priority: md.priority,
            target: md.target,
            flags: md.flags,
            crit_ratio: md.crit_ratio,
            will_crit: md.will_crit,
            crit_modifier: None,
            future_hit: false,
            secondaries: md.secondaries.clone(),
            self_: md.self_.clone(),
            self_dropped: false,
            self_boost: md.self_boost,
            boosts: md.boosts,
            status: md.status,
            volatile_status: md.volatile_status,
            side_condition: md.side_condition,
            slot_condition: md.slot_condition,
            weather: md.weather,
            terrain: md.terrain,
            pseudo_weather: md.pseudo_weather,
            heal: md.heal,
            drain: md.drain,
            recoil: md.recoil,
            self_switch: md.self_switch,
            force_switch: md.force_switch,
            selfdestruct: md.selfdestruct,
            breaks_protect: md.breaks_protect,
            ignore_immunity: Some(md.ignore_immunity),
            ignore_ability: md.ignore_ability,
            ignore_defensive: md.ignore_defensive,
            ignore_offensive: md.ignore_offensive,
            ignore_negative_offensive: md.ignore_negative_offensive,
            ignore_positive_defensive: md.ignore_positive_defensive,
            ignore_evasion: md.ignore_evasion,
            ignore_accuracy: md.ignore_accuracy,
            always_hit: false,
            multihit: md.multihit,
            multihit_type: 0,
            multiaccuracy: md.multiaccuracy,
            ohko: md.ohko,
            damage: md.damage,
            override_offensive_stat: md.override_offensive_stat,
            override_offensive_target: md.override_offensive_target,
            override_defensive_stat: md.override_defensive_stat,
            override_defensive_source: md.override_defensive_source,
            sleep_usable: md.sleep_usable,
            smart_target: if md.smart_target { Some(true) } else { None },
            tracks_target: md.tracks_target,
            steals_boosts: md.steals_boosts,
            struggle_recoil: md.struggle_recoil,
            mind_blown_recoil: md.mind_blown_recoil || md.chloroblast_recoil,
            has_crash_damage: md.has_crash_damage,
            thaws_target: md.thaws_target,
            force_stab: md.force_stab,
            stalling_move: md.stalling_move,
            spread_hit: false,
            has_sheer_force: false,
            infiltrates: false,
            has_bounced: false,
            prankster_boosted: false,
            is_external: false,
            source_effect: None,
            hit: 0,
            last_hit: false,
            total_damage: 0.0,
            hit_targets: vec![],
            move_hit_data: Default::default(),
            type_changer_boosted: None,
            stab: None,
            has_life_orb_boost: false,
            ruined_atk: None,
            ruined_def: None,
            ruined_spa: None,
            ruined_spd: None,
            allies_hit: false,
            is_z_or_max_powered: false,
            base_move: move_id,
            allies: vec![],
            aura_booster: None,
        };
        self.moves.push(am);
        (self.moves.len() - 1) as u16
    }

    /// BattleQueue.resolveAction
    pub fn resolve_action(&mut self, mut a: Action, mid_turn: bool) -> Vec<Action> {
        if a.choice == ActionKind::Pass {
            return vec![];
        }
        if a.order == 0.0 {
            a.order = match a.choice {
                ActionKind::Team => 1.0,
                ActionKind::Start => 2.0,
                ActionKind::InstaSwitch => 3.0,
                ActionKind::BeforeTurn => 4.0,
                ActionKind::BeforeTurnMove => 5.0,
                ActionKind::RevivalBlessing => 6.0,
                ActionKind::RunSwitch => 101.0,
                ActionKind::Switch => 103.0,
                ActionKind::MegaEvo => 104.0,
                ActionKind::PriorityChargeMove => 107.0,
                ActionKind::Shift => 200.0,
                ActionKind::Residual => 300.0,
                _ => 200.0,
            };
        }
        if a.mv == NONE16 && a.move_id != NONE16 {
            a.mv = self.new_active_move(a.move_id);
        }
        let mut pre: Vec<Action> = vec![];
        if !mid_turn {
            if a.choice == ActionKind::Move {
                let m = a.pokemon.unwrap();
                let md = &self.d.moves[a.move_id as usize];
                if self.eff(md.eff).has(self.d.cb_id("beforeTurnCallback")) {
                    let mut bt = Action::new(ActionKind::BeforeTurnMove);
                    bt.pokemon = Some(m);
                    bt.move_id = a.move_id;
                    bt.mv = a.mv;
                    bt.target_loc = a.target_loc;
                    pre.extend(self.resolve_action(bt, false));
                }
                if a.mega == 1 && !self.is_sky_dropped(m) {
                    let mut me = Action::new(ActionKind::MegaEvo);
                    me.pokemon = Some(m);
                    let r = self.resolve_action(me, false);
                    // unshift: the mega action goes before everything resolved so far
                    let mut v = r;
                    v.extend(pre);
                    pre = v;
                }
                if self.eff(md.eff).has(self.d.cb_id("priorityChargeCallback")) {
                    let mut pc = Action::new(ActionKind::PriorityChargeMove);
                    pc.pokemon = Some(m);
                    pc.move_id = a.move_id;
                    pc.mv = a.mv;
                    let r = self.resolve_action(pc, false);
                    let mut v = r;
                    v.extend(pre);
                    pre = v;
                }
                let mref = EffRef { eff: self.mv(a.mv).eff, mv: a.mv, part: 0 };
                a.fractional_priority = self.run_event(Ev::FractionalPriority, Arg::Mon(m), None, mref, Val::Num(0.0)).num();
            } else if matches!(a.choice, ActionKind::Switch | ActionKind::InstaSwitch) {
                let m = a.pokemon.unwrap();
                if let SwitchFlag::Move(mid) = self.p(m).switch_flag {
                    a.source_effect = Some(self.d.moves[mid as usize].eff);
                }
                self.pm(m).switch_flag = SwitchFlag::No;
            }
        }
        if a.mv != NONE16 {
            let m = a.pokemon.unwrap();
            if self.d.moves[a.move_id as usize].id == "curse" && !self.has_type(m, "Ghost") {
                self.mvm(a.mv).target = MoveTarget::SelfTarget;
            }
            if a.target_loc == 0 {
                if let Some(t) = self.get_random_target(m, a.mv) {
                    a.target_loc = self.get_loc_of(m, t);
                }
            }
            a.original_target = self.get_at_loc(m, a.target_loc);
        }
        self.action_speed(&mut a);
        pre.push(a);
        pre
    }

    /// Battle.getActionSpeed
    pub fn action_speed(&mut self, a: &mut Action) {
        if matches!(a.choice, ActionKind::Move | ActionKind::BeforeTurnMove | ActionKind::PriorityChargeMove) && a.mv != NONE16 {
            let m = a.pokemon.unwrap();
            let base_prio = self.d.moves[self.mv(a.mv).def as usize].priority;
            let target = self.get_target(m, a.mv, a.target_loc, None);
            let mref = EffRef { eff: self.mv(a.mv).eff, mv: a.mv, part: 0 };
            let mut priority = self.single_event(Ev::ModifyPriority, mref, StateLoc::None, Arg::Mon(m), Arg::from_opt(target), EffRef::NONE, Val::Num(base_prio)).num();
            priority = self.run_event(Ev::ModifyPriority, Arg::Mon(m), target, mref, Val::Num(priority)).num();
            a.priority = priority + a.fractional_priority;
            self.mvm(a.mv).priority = priority;
        }
        match a.pokemon {
            None => a.speed = 1.0,
            Some(m) => a.speed = self.get_action_speed_of(m),
        }
    }
    fn get_action_speed_of(&mut self, m: Mon) -> f64 {
        self.get_action_speed(m)
    }

    /// BattleQueue.insertChoice: insert where it belongs without re-sorting
    pub fn insert_choice(&mut self, a: Action, mid_turn: bool) {
        if let Some(m) = a.pokemon {
            self.update_speed(m);
        }
        let actions = self.resolve_action(a, mid_turn);
        if actions.is_empty() {
            return;
        }
        let mut first: Option<usize> = None;
        let mut last: Option<usize> = None;
        for (i, cur) in self.queue.iter().enumerate() {
            let c = compare_priority(&actions[0], cur);
            if c <= 0.0 && first.is_none() {
                first = Some(i);
            }
            if c < 0.0 {
                last = Some(i);
                break;
            }
        }
        match first {
            None => self.queue.extend(actions),
            Some(f) => {
                let l = last.unwrap_or(self.queue.len());
                let index = if f == l { f } else { self.random_range(f as u32, l as u32 + 1) as usize };
                for (k, act) in actions.into_iter().enumerate() {
                    self.queue.insert(index + k, act);
                }
            }
        }
    }
    pub fn cancel_action(&mut self, m: Mon) -> bool {
        let n = self.queue.len();
        self.queue.retain(|a| a.pokemon != Some(m));
        self.queue.len() != n
    }
    pub fn cancel_move(&mut self, m: Mon) -> bool {
        if let Some(i) = self.queue.iter().position(|a| a.choice == ActionKind::Move && a.pokemon == Some(m)) {
            self.queue.remove(i);
            return true;
        }
        false
    }
    pub fn will_move(&self, m: Mon) -> Option<usize> {
        if self.p(m).fainted {
            return None;
        }
        self.queue.iter().position(|a| a.choice == ActionKind::Move && a.pokemon == Some(m))
    }
    pub fn will_switch(&self, m: Mon) -> Option<usize> {
        self.queue.iter().position(|a| matches!(a.choice, ActionKind::Switch | ActionKind::InstaSwitch) && a.pokemon == Some(m))
    }
    pub fn will_act(&self) -> Option<usize> {
        self.queue.iter().position(|a| matches!(a.choice, ActionKind::Move | ActionKind::Switch | ActionKind::InstaSwitch | ActionKind::Shift))
    }
    /// queue.prioritizeAction
    pub fn prioritize_action(&mut self, idx: usize, source_effect: Option<EffId>) {
        let mut a = self.queue.remove(idx);
        a.source_effect = source_effect;
        a.order = 3.0;
        self.queue.insert(0, a);
    }

    // ------------------------------------------------------------------
    // targets
    // ------------------------------------------------------------------
    pub fn valid_target_loc(&self, target_loc: i8, source: Mon, target_type: MoveTarget) -> bool {
        if target_loc == 0 {
            return true;
        }
        let num_slots = 2i8;
        let source_loc = self.get_loc_of(source, source);
        if target_loc.abs() > num_slots {
            return false;
        }
        let is_self = source_loc == target_loc;
        let is_foe = target_loc > 0;
        let across = -(num_slots + 1 - target_loc);
        let is_adjacent = if target_loc > 0 { (across - source_loc).abs() <= 1 } else { (target_loc - source_loc).abs() == 1 };
        match target_type {
            MoveTarget::RandomNormal | MoveTarget::Scripted | MoveTarget::Normal => is_adjacent,
            MoveTarget::AdjacentAlly => is_adjacent && !is_foe,
            MoveTarget::AdjacentAllyOrSelf => (is_adjacent && !is_foe) || is_self,
            MoveTarget::AdjacentFoe => is_adjacent && is_foe,
            MoveTarget::Any => !is_self,
            _ => false,
        }
    }

    /// Battle.getTarget(pokemon, move, targetLoc, originalTarget): `dex.moves.get(move)` returns
    /// the active move itself, so its current (possibly modified) properties are used
    pub fn get_target(&mut self, m: Mon, mv: u16, target_loc: i8, original: Option<Mon>) -> Option<Mon> {
        let (mut tracks, smart, mtarget, flags) = {
            let a = self.mv(mv);
            (a.tracks_target, a.smart_target == Some(true), a.target, a.flags)
        };
        if self.has_ability_any(m, &["stalwart", "propellertail"]) {
            tracks = true;
        }
        if tracks {
            if let Some(o) = original {
                if self.p(o).is_active {
                    return Some(o);
                }
            }
        }
        if smart {
            let cur = self.get_at_loc(m, target_loc);
            return match cur {
                Some(c) if !self.p(c).fainted => Some(c),
                _ => self.get_random_target_t(m, mtarget),
            };
        }
        let self_loc = self.get_loc_of(m, m);
        if matches!(mtarget, MoveTarget::AdjacentAlly | MoveTarget::Any | MoveTarget::Normal) && target_loc == self_loc
            && !self.has_vol(m, "twoturnmove") && !self.has_vol(m, "iceball") && !self.has_vol(m, "rollout") {
            return if flags & mflag::FUTUREMOVE != 0 { Some(m) } else { None };
        }
        if mtarget != MoveTarget::RandomNormal && self.valid_target_loc(target_loc, m, mtarget) {
            let t = self.get_at_loc(m, target_loc);
            if let Some(t) = t {
                if self.p(t).fainted {
                    if t.s == m.s {
                        if mtarget == MoveTarget::AdjacentAllyOrSelf {
                            return Some(m);
                        }
                        return Some(t);
                    }
                } else {
                    return Some(t);
                }
            }
        }
        self.get_random_target_t(m, mtarget)
    }

    /// Battle.getRandomTarget(pokemon, move) (the active move's current target type)
    pub fn get_random_target(&mut self, m: Mon, mv: u16) -> Option<Mon> {
        let t = self.mv(mv).target;
        self.get_random_target_t(m, t)
    }
    pub fn get_random_target_t(&mut self, m: Mon, t: MoveTarget) -> Option<Mon> {
        match t {
            MoveTarget::SelfTarget | MoveTarget::All | MoveTarget::AllySide | MoveTarget::AllyTeam | MoveTarget::AdjacentAllyOrSelf => Some(m),
            MoveTarget::AdjacentAlly => {
                let aa = self.adjacent_allies(m);
                if aa.is_empty() {
                    None
                } else {
                    let i = self.prng.sample_index(aa.len());
                    Some(aa[i])
                }
            }
            _ => {
                // side.randomFoe() || side.foe.active[0]
                let foes = self.foes_of(m.s);
                if !foes.is_empty() {
                    let i = self.prng.sample_index(foes.len());
                    return Some(foes[i]);
                }
                self.active_at(1 - m.s as usize, 0)
            }
        }
    }

    // ------------------------------------------------------------------
    // turn loop
    // ------------------------------------------------------------------
    pub fn turn_loop(&mut self) {
        add!(self, "");
        if self.request_state != RequestState::None {
            self.request_state = RequestState::None;
        }
        if !self.mid_turn {
            self.insert_choice(Action::new(ActionKind::BeforeTurn), false);
            self.queue_add_choice(Action::new(ActionKind::Residual));
            self.mid_turn = true;
        }
        while !self.queue.is_empty() {
            let a = self.queue.remove(0);
            self.run_action(a);
            if self.request_state != RequestState::None || self.ended {
                return;
            }
        }
        self.end_turn();
        self.mid_turn = false;
        self.queue.clear();
    }

    pub fn run_action(&mut self, a: Action) -> bool {
        let original_hp = a.pokemon.map(|m| self.p(m).hp);
        let mut residual_pokemon: Vec<(Mon, i32)> = vec![];
        match a.choice {
            ActionKind::Start => {
                for s in 0..2 {
                    if self.sides[s].pokemon_left > 0 {
                        self.sides[s].pokemon_left = self.sides[s].pokemon.len() as i32;
                    }
                    let n = self.sides[s].pokemon.len() as i32;
                    add!(self, "teamsize", format!("p{}", s + 1), n);
                }
                add!(self, "start");
                // species BattleStart handlers: none in this format
                for s in 0..2 {
                    for pos in 0..2 {
                        let ti = self.sides[s].pokemon[pos];
                        self.switch_in(Mon { s: s as u8, i: ti }, pos as u8, None, false);
                    }
                }
                self.mid_turn = true;
            }
            ActionKind::Move => {
                let m = a.pokemon.unwrap();
                if !self.p(m).is_active || self.p(m).fainted {
                    return false;
                }
                self.run_move(a.mv, m, a.target_loc, a.source_effect, false, a.original_target);
            }
            ActionKind::MegaEvo => {
                self.run_mega_evo(a.pokemon.unwrap());
            }
            ActionKind::BeforeTurnMove => {
                let m = a.pokemon.unwrap();
                if !self.p(m).is_active || self.p(m).fainted {
                    return false;
                }
                let Some(t) = self.get_target(m, a.mv, a.target_loc, None) else { return false };
                super::handlers::before_turn_callback(self, m, t, a.mv);
            }
            ActionKind::PriorityChargeMove => {
                let m = a.pokemon.unwrap();
                if !self.p(m).is_active || self.p(m).fainted {
                    return false;
                }
                super::handlers::priority_charge_callback(self, m, a.mv);
            }
            ActionKind::Event => {
                if let (Some(ev), Some(m)) = (a.event, a.pokemon) {
                    self.run_event(ev, Arg::Mon(m), None, EffRef::NONE, Val::Undef);
                }
            }
            ActionKind::Team => {
                let m = a.pokemon.unwrap();
                let s = m.s as usize;
                if a.index == 0 {
                    self.sides[s].pokemon.clear();
                }
                self.sides[s].pokemon.push(m.i);
                self.pm(m).position = a.index;
                return false;
            }
            ActionKind::Pass => return false,
            ActionKind::InstaSwitch | ActionKind::Switch => {
                let m = a.pokemon.unwrap();
                if a.choice == ActionKind::Switch && self.p(m).status != self.d.c.empty {
                    // CheckShow (Natural Cure): no handler in the Champions version
                }
                let pos = self.p(m).position;
                let r = self.switch_in(a.target.unwrap(), pos, a.source_effect, false);
                if r == SwitchResult::PursuitFaint {
                    self.hint("A Pokemon can't switch between when it runs out of HP and when it faints", false);
                }
            }
            ActionKind::RunSwitch => {
                self.run_switch(a.pokemon.unwrap());
            }
            ActionKind::Shift => {}
            ActionKind::BeforeTurn => {
                self.each_event(Ev::BeforeTurn, EffRef::NONE);
            }
            ActionKind::Residual => {
                add!(self, "");
                self.clear_active_move(true);
                self.update_all_speed();
                residual_pokemon = self.all_active().into_iter().map(|m| (m, self.p(m).hp)).collect();
                self.field_event(Ev::Residual, None);
                if !self.ended {
                    add!(self, "upkeep");
                }
            }
            ActionKind::RevivalBlessing => {
                let m = a.pokemon.unwrap();
                let t = a.target.unwrap();
                let s = m.s as usize;
                self.sides[s].pokemon_left += 1;
                if (self.p(t).position as usize) < 2 {
                    let mut ins = Action::new(ActionKind::InstaSwitch);
                    ins.pokemon = Some(t);
                    ins.target = Some(t);
                    self.queue_add_choice(ins);
                }
                let empty = self.d.c.empty;
                let p = self.pm(t);
                p.fainted = false;
                p.faint_queued = false;
                p.sub_fainted = Some(false);
                p.status = empty;
                p.hp = 1;
                let half = self.p(t).maxhp as f64 / 2.0;
                self.sethp(t, half);
                add!(self, "-heal", t, Health(t), "[from] move: Revival Blessing");
                let pos = self.p(m).position;
                let rb = self.cond("revivalblessing");
                self.remove_slot_condition(m.s, pos, rb);
            }
        }

        // phazing (Roar, Whirlwind, Dragon Tail...)
        for s in 0..2 {
            for pos in 0..2 {
                let Some(m) = self.active_at(s, pos) else { continue };
                if self.p(m).force_switch_flag {
                    if self.p(m).hp > 0 {
                        self.drag_in(s as u8, pos as u8);
                    }
                    self.pm(m).force_switch_flag = false;
                }
            }
        }
        self.clear_active_move(false);
        self.faint_messages(false, false, true);
        if self.ended {
            return true;
        }
        if self.queue.is_empty() {
            self.check_fainted();
        } else if self.queue[0].choice == ActionKind::InstaSwitch {
            return false;
        }
        if a.choice != ActionKind::Start {
            self.each_event(Ev::Update, EffRef::NONE);
            for (m, ohp) in residual_pokemon {
                let maxhp = self.p(m).maxhp;
                let hp = self.p(m).hp;
                if hp > 0 && hp as f64 <= maxhp as f64 / 2.0 && ohp as f64 > maxhp as f64 / 2.0 {
                    self.run_event(Ev::EmergencyExit, Arg::Mon(m), None, EffRef::NONE, Val::Undef);
                }
            }
        }
        if a.choice == ActionKind::RunSwitch {
            let m = a.pokemon.unwrap();
            let p = self.p(m);
            if p.hp > 0 && p.hp as f64 <= p.maxhp as f64 / 2.0 && original_hp.unwrap_or(0) as f64 > p.maxhp as f64 / 2.0 {
                self.run_event(Ev::EmergencyExit, Arg::Mon(m), None, EffRef::NONE, Val::Undef);
            }
        }
        let mut switches = [false; 2];
        for s in 0..2 {
            switches[s] = (0..2).any(|pos| self.active_at(s, pos).map(|m| self.p(m).switch_flag.is_set()).unwrap_or(false));
        }
        let rb_move = self.d.move_id("revivalblessing");
        for s in 0..2 {
            if switches[s] && self.can_switch(s) == 0 {
                let mut revive_switch = false;
                for pos in 0..2 {
                    if let Some(m) = self.active_at(s, pos) {
                        let ppos = self.p(m).position;
                        if self.has_revival(s, ppos) {
                            revive_switch = true;
                            continue;
                        }
                        self.pm(m).switch_flag = SwitchFlag::No;
                    }
                }
                if !revive_switch {
                    switches[s] = false;
                }
            } else if switches[s] {
                for pos in 0..2 {
                    let Some(m) = self.active_at(s, pos) else { continue };
                    let p = self.p(m);
                    let is_rb = rb_move.map(|r| p.switch_flag == SwitchFlag::Move(r)).unwrap_or(false);
                    if p.hp > 0 && p.switch_flag.is_set() && !is_rb && !p.skip_before_switch_out {
                        self.run_event(Ev::BeforeSwitchOut, Arg::Mon(m), None, EffRef::NONE, Val::Undef);
                        self.pm(m).skip_before_switch_out = true;
                        self.faint_messages(false, false, true);
                        if self.ended {
                            return true;
                        }
                        if self.p(m).fainted {
                            switches[s] = (0..2).any(|q| self.active_at(s, q).map(|x| self.p(x).switch_flag.is_set()).unwrap_or(false));
                        }
                    }
                }
            }
        }
        for s in 0..2 {
            if switches[s] {
                self.make_request(RequestState::Switch);
                return true;
            }
        }
        // gen 8+: speeds are updated dynamically
        if let Some(next) = self.queue.first() {
            if next.choice == ActionKind::Move {
                self.update_all_speed();
                let mut q = std::mem::take(&mut self.queue);
                for qa in q.iter_mut() {
                    if qa.pokemon.is_some() {
                        self.action_speed(qa);
                    }
                }
                self.speed_sort(&mut q);
                self.queue = q;
            }
        }
        false
    }

    pub fn clear_active_move(&mut self, failed: bool) {
        if let Some(am) = self.active_move {
            if !failed {
                self.last_move = Some(am);
            }
            self.active_move = None;
            self.active_pokemon = None;
            self.active_target = None;
        }
    }
    pub fn set_active_move(&mut self, mv: Option<u16>, pokemon: Option<Mon>, target: Option<Mon>) {
        self.active_move = mv;
        self.active_pokemon = pokemon;
        self.active_target = target.or(pokemon);
    }

    pub fn check_fainted(&mut self) {
        for s in 0..2 {
            for pos in 0..2 {
                let Some(m) = self.active_at(s, pos) else { continue };
                if self.p(m).fainted {
                    self.pm(m).status = self.d.cond("fnt").unwrap();
                    self.pm(m).switch_flag = SwitchFlag::Yes;
                }
            }
        }
    }

    /// side.slotConditions[pos]['revivalblessing']
    pub fn has_revival(&self, s: usize, pos: u8) -> bool {
        let Some(rb) = self.d.cond("revivalblessing") else { return false };
        (pos as usize) < 2 && self.sides[s].slot_conditions[pos as usize].iter().any(|x| x.0 == rb)
    }

    pub fn can_switch(&self, s: usize) -> usize {
        self.possible_switches(s).len()
    }
    fn possible_switches(&self, s: usize) -> Vec<Mon> {
        if self.sides[s].pokemon_left == 0 {
            return vec![];
        }
        let side = &self.sides[s];
        side.pokemon.iter().skip(2).map(|&i| Mon { s: s as u8, i }).filter(|&m| !self.p(m).fainted).collect()
    }
    pub fn get_random_switchable(&mut self, s: usize) -> Option<Mon> {
        let c = self.possible_switches(s);
        if c.is_empty() {
            return None;
        }
        let i = self.prng.sample_index(c.len());
        Some(c[i])
    }

    /// Battle.endTurn
    pub fn end_turn(&mut self) {
        self.turn += 1;
        self.last_successful_move_this_turn = None;
        self.moves_gc();
        for s in 0..2 {
            for pos in 0..2 {
                let Some(m) = self.active_at(s, pos) else { continue };
                {
                    let turn = self.turn;
                    let p = self.pm(m);
                    p.move_this_turn = None;
                    p.newly_switched = false;
                    p.move_last_turn_result = p.move_this_turn_result;
                    p.move_this_turn_result = Val::Undef;
                    if turn != 1 {
                        p.used_item_this_turn = false;
                        p.stats_raised_this_turn = false;
                        p.stats_lowered_this_turn = false;
                        p.hurt_this_turn = None;
                    }
                    p.maybe_disabled = false;
                    p.maybe_locked = false;
                    for slot in p.move_slots.iter_mut() {
                        slot.disabled = 0;
                        slot.disabled_source = None;
                    }
                }
                self.run_event(Ev::DisableMove, Arg::Mon(m), None, EffRef::NONE, Val::Undef);
                let ids: Vec<u16> = self.p(m).move_slots.iter().map(|x| x.id).collect();
                for id in ids {
                    let mv = self.new_active_move(id);
                    let mref = EffRef { eff: self.mv(mv).eff, mv, part: 0 };
                    self.single_event(Ev::DisableMove, mref, StateLoc::None, Arg::Mon(m), Arg::None, EffRef::NONE, Val::Undef);
                    if self.mv(mv).flags & mflag::CANTUSETWICE != 0 && self.p(m).last_move == Some(id) {
                        self.disable_move(m, id, false, None);
                    }
                }
                if !self.p(m).attacked_by.is_empty() {
                    self.pm(m).known_type = true;
                }
                // attackedBy bookkeeping
                let actives: Vec<Mon> = self.all_active();
                let p = self.pm(m);
                let mut k = p.attacked_by.len();
                while k > 0 {
                    k -= 1;
                    let src = p.attacked_by[k].source;
                    if actives.contains(&src) {
                        p.attacked_by[k].this_turn = false;
                    } else {
                        p.attacked_by.remove(k);
                    }
                }
                // gen 7+: real types are visible (of the Pokemon it appears to be)
                let seen = self.p(m).illusion.unwrap_or(m);
                let types = self.get_types(seen, true);
                let real = types.iter().map(|&t| self.d.type_name(t).to_string()).collect::<Vec<_>>().join("/");
                if real != self.p(seen).apparent_type {
                    add!(self, "-start", m, "typechange", real.clone(), "[silent]");
                    self.pm(seen).apparent_type = real;
                    let at = self.p(m).added_type;
                    if at != NO_TYPE {
                        let tn = self.d.type_name(at).to_string();
                        add!(self, "-start", m, "typeadd", tn, "[silent]");
                    }
                }
                self.pm(m).trapped = 0;
                self.pm(m).maybe_trapped = false;
                self.run_event(Ev::TrapPokemon, Arg::Mon(m), None, EffRef::NONE, Val::Undef);
                if !self.p(m).known_type || self.dex_immunity("trapped", m) {
                    self.run_event(Ev::MaybeTrapPokemon, Arg::Mon(m), None, EffRef::NONE, Val::Undef);
                }
                // foes' possible trapping abilities
                let foes = self.foes_of(m.s as u8);
                for f in foes {
                    let sp = &self.d.species[self.p(f).species as usize];
                    for (_, aname) in sp.abilities.clone() {
                        let ab = self.d.ability(&aname);
                        // (Showdown compares the ability NAME with the current ability ID: never equal)
                        if self.p(m).known_type && !self.dex_immunity("trapped", m) {
                            continue;
                        }
                        self.single_event(Ev::FoeMaybeTrapPokemon, EffRef::eff(ab), StateLoc::None, Arg::Mon(m), Arg::Mon(f), EffRef::NONE, Val::Undef);
                    }
                }
                if self.p(m).fainted {
                    continue;
                }
                self.pm(m).active_turns += 1;
            }
            self.sides[s].fainted_last_turn = self.sides[s].fainted_this_turn;
            self.sides[s].fainted_this_turn = None;
        }
        let t = self.turn;
        add!(self, "turn", t);
        self.make_request(RequestState::Move);
    }

    /// drop active moves nobody refers to any more (between turns)
    fn moves_gc(&mut self) {
        if self.queue.is_empty() {
            // battle.lastMove survives the turn in Showdown (Copycat copies the previous turn's last move): keep that one
            let keep = self.last_move.map(|k| self.moves[k as usize].clone());
            self.moves.clear();
            self.active_move = None;
            self.last_move = keep.map(|am| {
                self.moves.push(am);
                (self.moves.len() - 1) as u16
            });
        }
    }

    // ------------------------------------------------------------------
    // fainting & winning
    // ------------------------------------------------------------------
    pub fn faint_messages(&mut self, last_first: bool, force_check: bool, check_win: bool) -> bool {
        if self.ended {
            return false;
        }
        let length = self.faint_queue.len();
        if length == 0 {
            if force_check && self.check_win(None) {
                return true;
            }
            return false;
        }
        if last_first {
            let last = self.faint_queue.pop().unwrap();
            self.faint_queue.insert(0, last);
        }
        let mut check_win = check_win;
        let mut faint_data: Option<(Mon, Option<Mon>, EffRef)> = None;
        while !self.faint_queue.is_empty() {
            let left = self.faint_queue.len();
            let fd = self.faint_queue.remove(0);
            faint_data = Some(fd);
            let (m, src, eff) = fd;
            if !self.p(m).fainted && self.run_event(Ev::BeforeFaint, Arg::Mon(m), src, eff, Val::Undef).truthy() {
                add!(self, "faint", m);
                let s = m.s as usize;
                if self.sides[s].pokemon_left > 0 {
                    self.sides[s].pokemon_left -= 1;
                }
                if self.sides[s].total_fainted < 100 {
                    self.sides[s].total_fainted += 1;
                }
                self.run_event(Ev::Faint, Arg::Mon(m), src, eff, Val::Undef);
                let ab = self.p(m).ability;
                self.single_event(Ev::End, EffRef::eff(ab), StateLoc::Ability(m), Arg::Mon(m), Arg::None, EffRef::NONE, Val::Undef);
                let it = self.p(m).item;
                self.single_event(Ev::End, EffRef::eff(it), StateLoc::Item(m), Arg::Mon(m), Arg::None, EffRef::NONE, Val::Undef);
                self.clear_volatile(m, false);
                let p = self.pm(m);
                p.fainted = true;
                p.illusion = None;
                p.is_active = false;
                p.is_started = false;
                self.sides[s].fainted_this_turn = Some(m.i);
                if self.faint_queue.len() >= left {
                    check_win = true;
                }
            }
        }
        if check_win && self.check_win(faint_data) {
            return true;
        }
        if let Some((m, src, eff)) = faint_data {
            if length > 0 {
                self.run_event(Ev::AfterFaint, Arg::Mon(m), src, eff, Val::Num(length as f64));
            }
        }
        false
    }

    pub fn check_win(&mut self, faint_data: Option<(Mon, Option<Mon>, EffRef)>) -> bool {
        if self.sides.iter().all(|s| s.pokemon_left == 0) {
            let w = faint_data.map(|fd| fd.0.s);
            self.win(w);
            return true;
        }
        for s in 0..2 {
            if self.sides[1 - s].pokemon_left == 0 {
                self.win(Some(s as u8));
                return true;
            }
        }
        false
    }

    pub fn win(&mut self, side: Option<u8>) -> bool {
        if self.ended {
            return false;
        }
        self.winner = Some(side);
        add!(self, "");
        match side {
            Some(s) => {
                let n = self.sides[s as usize].name.clone();
                add!(self, "win", n);
            }
            None => add!(self, "tie"),
        }
        self.ended = true;
        self.request_state = RequestState::None;
        for s in 0..2 {
            self.sides[s].request = RequestState::None;
        }
        true
    }

    // ------------------------------------------------------------------
    // switching (battle-actions.ts)
    // ------------------------------------------------------------------
    pub fn switch_in(&mut self, m: Mon, pos: u8, source_effect: Option<EffId>, is_drag: bool) -> SwitchResult {
        if self.p(m).is_active {
            self.hint("A switch failed because the Pokémon trying to switch in is already in.", false);
            return SwitchResult::Failed;
        }
        let s = m.s as usize;
        let old = self.active_at(s, pos as usize);
        if let Some(o) = old {
            if self.p(o).hp > 0 {
                self.pm(o).being_called_back = true;
                let mut copy_flag = SelfSwitch::No;
                if let Some(se) = source_effect {
                    let e = self.eff(se);
                    if e.kind == Kind::Move {
                        let ss = self.d.moves[e.move_def as usize].self_switch;
                        if matches!(ss, SelfSwitch::CopyVolatile | SelfSwitch::ShedTail) {
                            copy_flag = ss;
                        }
                    }
                }
                if !self.p(o).skip_before_switch_out && !is_drag {
                    self.run_event(Ev::BeforeSwitchOut, Arg::Mon(o), None, EffRef::NONE, Val::Undef);
                    self.each_event(Ev::Update, EffRef::NONE);
                }
                self.pm(o).skip_before_switch_out = false;
                if !self.run_event(Ev::SwitchOut, Arg::Mon(o), None, EffRef::NONE, Val::Undef).truthy() {
                    return SwitchResult::Failed;
                }
                if self.p(o).hp == 0 {
                    return SwitchResult::PursuitFaint;
                }
                let ab = self.p(o).ability;
                self.single_event(Ev::End, EffRef::eff(ab), StateLoc::Ability(o), Arg::Mon(o), Arg::None, EffRef::NONE, Val::Undef);
                let it = self.p(o).item;
                self.single_event(Ev::End, EffRef::eff(it), StateLoc::Item(o), Arg::Mon(o), Arg::None, EffRef::NONE, Val::Undef);
                self.cancel_action(o);
                if copy_flag != SelfSwitch::No {
                    self.copy_volatile_from(m, o, copy_flag);
                }
                self.clear_volatile(o, true);
            }
            let op = self.pm(o);
            op.is_active = false;
            op.is_started = false;
            op.used_item_this_turn = false;
            op.stats_raised_this_turn = false;
            op.stats_lowered_this_turn = false;
            let new_pos = self.p(m).position;
            self.pm(o).position = new_pos;
            if self.p(o).fainted {
                self.pm(o).status = self.d.c.empty;
            }
            self.pm(m).position = pos;
            let side = &mut self.sides[s];
            side.pokemon[pos as usize] = m.i;
            side.pokemon[new_pos as usize] = o.i;
        }
        let p = self.pm(m);
        p.is_active = true;
        p.active_turns = 0;
        p.active_move_actions = 0;
        for slot in p.move_slots.iter_mut() {
            slot.used = false;
        }
        self.sides[s].active[pos as usize] = Some(m.i);
        let ab = self.p(m).ability;
        let st = self.init_state(ab, Holder::Mon(m), None);
        self.pm(m).ability_state = st;
        let it = self.p(m).item;
        let st = self.init_state(it, Holder::Mon(m), None);
        self.pm(m).item_state = st;
        self.run_event(Ev::BeforeSwitchIn, Arg::Mon(m), None, EffRef::NONE, Val::Undef);
        let kind = if is_drag { "drag" } else { "switch" };
        match source_effect {
            Some(se) => {
                let name = self.eff(se).name.clone();
                add!(self, kind, m, FullDetails(m), format!("[from] {name}"));
            }
            None => add!(self, kind, m, FullDetails(m)),
        }
        self.pm(m).previously_switched_in += 1;
        if is_drag {
            self.run_switch(m);
        } else {
            let mut a = Action::new(ActionKind::RunSwitch);
            a.pokemon = Some(m);
            self.insert_choice(a, false);
        }
        SwitchResult::Ok
    }

    pub fn drag_in(&mut self, s: u8, pos: u8) -> bool {
        let Some(m) = self.get_random_switchable(s as usize) else { return false };
        if self.p(m).is_active {
            return false;
        }
        let Some(old) = self.active_at(s as usize, pos as usize) else { return false };
        if self.p(old).hp == 0 {
            return false;
        }
        if !self.run_event(Ev::DragOut, Arg::Mon(old), None, EffRef::NONE, Val::Undef).truthy() {
            return false;
        }
        self.switch_in(m, pos, None, true) == SwitchResult::Ok
    }

    pub fn run_switch(&mut self, m: Mon) {
        let mut switchers = vec![m];
        while self.queue.first().map(|a| a.choice == ActionKind::RunSwitch).unwrap_or(false) {
            let a = self.queue.remove(0);
            switchers.push(a.pokemon.unwrap());
        }
        let mut all = self.get_all_active(true);
        self.speed_sort_by(&mut all, |a: &(Mon, f64), b: &(Mon, f64)| b.1 - a.1);
        self.speed_order = all.iter().map(|x| self.field_position(x.0)).collect();
        self.field_event(Ev::SwitchIn, Some(&switchers));
        for p in switchers {
            if self.p(p).hp == 0 {
                continue;
            }
            self.pm(p).is_started = true;
            self.pm(p).dragged_in = None;
        }
    }

    pub fn run_mega_evo(&mut self, m: Mon) -> bool {
        let Some(species) = self.p(m).can_mega_evo else { return false };
        let item = self.p(m).item;
        self.forme_change(m, species, EffRef::eff(item), true, None);
        let s = m.s as usize;
        for k in 0..self.sides[s].mons.len() {
            self.sides[s].mons[k].can_mega_evo = None;
        }
        self.run_event(Ev::AfterMega, Arg::Mon(m), None, EffRef::NONE, Val::Undef);
        true
    }

    /// copyVolatileFrom (Baton Pass / Shed Tail)
    pub fn copy_volatile_from(&mut self, m: Mon, from: Mon, cause: SelfSwitch) {
        self.clear_volatile(m, true);
        if cause != SelfSwitch::ShedTail {
            self.pm(m).boosts = self.p(from).boosts;
        }
        let vols = self.p(from).volatiles.clone();
        for (id, st) in vols {
            if cause == SelfSwitch::ShedTail && self.eff(id).id != "substitute" {
                continue;
            }
            if self.eff(id).no_copy {
                continue;
            }
            let mut ns = st.clone();
            let o = self.init_state(id, Holder::Mon(m), None);
            ns.uid = o.uid;
            ns.effect_order = o.effect_order;
            ns.target_mon = Some(m);
            self.pm(m).volatiles.push((id, ns));
        }
        self.clear_volatile(from, true);
        let ids: Vec<EffId> = self.p(m).volatiles.iter().map(|x| x.0).collect();
        for id in ids {
            self.single_event(Ev::Copy, EffRef::eff(id), StateLoc::Volatile(m, id), Arg::Mon(m), Arg::None, EffRef::NONE, Val::Undef);
        }
    }

    // ------------------------------------------------------------------
    // HP, boosts
    // ------------------------------------------------------------------
    /// Battle.spreadDamage
    pub fn spread_damage(&mut self, damage: &mut [Val], targets: &[Option<Mon>], source: Option<Mon>, effect: EffRef, instafaint: bool) -> Vec<Val> {
        let mut ret: Vec<Val> = vec![Val::Undef; damage.len()];
        let eff_id = if effect.is_none() { self.d.c.empty } else { effect.eff };
        let is_move_effect = effect.is_move() || self.eff(eff_id).effect_type == EffType::Move;
        for i in 0..damage.len() {
            let target = targets[i];
            let cur = damage[i];
            let mut td = cur;
            if !(td.truthy() || matches!(td, Val::Num(x) if x == 0.0)) {
                ret[i] = td;
                continue;
            }
            let Some(t) = target else {
                ret[i] = Val::Num(0.0);
                continue;
            };
            if self.p(t).hp == 0 {
                ret[i] = Val::Num(0.0);
                continue;
            }
            if !self.p(t).is_active {
                ret[i] = Val::False;
                continue;
            }
            if td.num() != 0.0 {
                td = Val::Num(clamp_int(td.num(), Some(1.0), None));
            }
            let eid: &str = &self.eff(eff_id).id;
            if eid != "strugglerecoil" {
                if self.eff(eff_id).effect_type == EffType::Weather && !self.run_status_immunity(t, &self.eff(eff_id).id, false) {
                    ret[i] = Val::Num(0.0);
                    continue;
                }
                td = self.run_event_full(Ev::Damage, Arg::Mon(t), Arg::from_opt(source), effect, td, true, false);
                if !(td.truthy() || matches!(td, Val::Num(x) if x == 0.0)) {
                    ret[i] = if cur == Val::True { Val::Undef } else { td };
                    continue;
                }
            }
            if td.num() != 0.0 {
                td = Val::Num(clamp_int(td.num(), Some(1.0), None));
            }
            let dealt = self.mon_damage(t, td.num(), source, effect);
            ret[i] = Val::Num(dealt);
            if dealt != 0.0 {
                self.pm(t).hurt_this_turn = Some(self.p(t).hp);
            }
            if let Some(src) = source {
                if is_move_effect {
                    self.pm(src).last_damage = dealt as i32;
                }
            }
            let full = self.eff(eff_id).fullname.clone();
            let name = if full == "tox" { "psn".to_string() } else { full };
            match self.eff(eff_id).id.as_str() {
                "partiallytrapped" => {
                    let se = self.volatile(t, self.d.c.partiallytrapped).and_then(|v| v.source_effect).map(|e| self.eff(e).fullname.clone()).unwrap_or_default();
                    add!(self, "-damage", t, Health(t), format!("[from] {se}"), "[partiallytrapped]");
                }
                "powder" => add!(self, "-damage", t, Health(t), "[silent]"),
                "confused" => add!(self, "-damage", t, Health(t), "[from] confusion"),
                _ => {
                    if is_move_effect || name.is_empty() {
                        add!(self, "-damage", t, Health(t));
                    } else if source.is_some() && (source != Some(t) || self.eff(eff_id).effect_type == EffType::Ability) {
                        let ss = self.mon_str(source.unwrap());
                        add!(self, "-damage", t, Health(t), format!("[from] {name}"), format!("[of] {ss}"));
                    } else {
                        add!(self, "-damage", t, Health(t), format!("[from] {name}"));
                    }
                }
            }
            if dealt != 0.0 && is_move_effect {
                let drain = if effect.is_move() { self.mv(effect.mv).drain } else { None };
                if let (Some((a, b)), Some(src)) = (drain, source) {
                    let amount = (dealt * a / b).round();
                    let drain_eff = self.d.c.drain;
                    self.heal(amount, Some(src), Some(t), EffRef::eff(drain_eff));
                }
            }
        }
        if instafaint {
            for i in 0..damage.len() {
                if !ret[i].truthy() {
                    continue;
                }
                let Some(t) = targets[i] else { continue };
                if self.p(t).hp <= 0 {
                    self.faint_messages(true, false, true);
                }
            }
        }
        ret
    }

    /// Battle.damage
    pub fn damage(&mut self, amount: f64, target: Option<Mon>, source: Option<Mon>, effect: EffRef, instafaint: bool) -> Val {
        let target = target.or(self.event.target.mon());
        let source = source.or(self.event.source.mon());
        let effect = if effect.is_none() { self.effect } else { effect };
        let mut d = [Val::Num(amount)];
        self.spread_damage(&mut d, &[target], source, effect, instafaint)[0]
    }

    /// Battle.directDamage
    pub fn direct_damage(&mut self, amount: f64, target: Option<Mon>, source: Option<Mon>, effect: EffRef) -> f64 {
        let target = target.or(self.event.target.mon());
        let source = source.or(self.event.source.mon());
        let effect = if effect.is_none() { self.effect } else { effect };
        let Some(t) = target else { return 0.0 };
        if self.p(t).hp == 0 || amount == 0.0 {
            return 0.0;
        }
        let amount = clamp_int(amount, Some(1.0), None);
        let dealt = self.mon_damage(t, amount, source, effect);
        let eid = if effect.is_none() { "" } else { self.eff(effect.eff).id.as_str() };
        match eid {
            "strugglerecoil" => add!(self, "-damage", t, Health(t), "[from] recoil"),
            "confusion" => add!(self, "-damage", t, Health(t), "[from] confusion"),
            _ => add!(self, "-damage", t, Health(t)),
        }
        if self.p(t).fainted {
            // battle.faint(target): already queued by mon_damage
        }
        dealt
    }

    /// Battle.heal
    pub fn heal(&mut self, amount: f64, target: Option<Mon>, source: Option<Mon>, effect: EffRef) -> Val {
        let target = target.or(self.event.target.mon());
        let source = source.or(self.event.source.mon());
        let effect = if effect.is_none() { self.effect } else { effect };
        let mut amount = amount;
        if amount != 0.0 && amount <= 1.0 {
            amount = 1.0;
        }
        amount = tr(amount);
        let r = self.run_event(Ev::TryHeal, Arg::from_opt(target), source, effect, Val::Num(amount));
        if !r.truthy() {
            return r;
        }
        let amount = r.num();
        let Some(t) = target else { return Val::False };
        if self.p(t).hp == 0 || !self.p(t).is_active || self.p(t).hp >= self.p(t).maxhp {
            return Val::False;
        }
        let Some(fin) = self.mon_heal(t, amount) else { return Val::False };
        let eid = if effect.is_none() { "" } else { self.eff(effect.eff).id.as_str() };
        match eid {
            "leechseed" | "rest" => add!(self, "-heal", t, Health(t), "[silent]"),
            "drain" => {
                let ss = source.map(|s| self.mon_str(s)).unwrap_or_default();
                add!(self, "-heal", t, Health(t), "[from] drain", format!("[of] {ss}"));
            }
            "wish" => {}
            "" => {}
            _ => {
                let full = self.eff(effect.eff).fullname.clone();
                if effect.is_move() || self.eff(effect.eff).effect_type == EffType::Move {
                    add!(self, "-heal", t, Health(t));
                } else if source.is_some() && source != Some(t) {
                    let ss = self.mon_str(source.unwrap());
                    add!(self, "-heal", t, Health(t), format!("[from] {full}"), format!("[of] {ss}"));
                } else {
                    add!(self, "-heal", t, Health(t), format!("[from] {full}"));
                }
            }
        }
        self.run_event(Ev::Heal, Arg::Mon(t), source, effect, Val::Num(fin as f64));
        Val::Num(fin as f64)
    }

    /// Battle.boost; returns null(None)/true/false/0 like Showdown: Val
    pub fn boost(&mut self, boost: SparseBoosts, target: Option<Mon>, source: Option<Mon>, effect: EffRef, is_secondary: bool, is_self: bool) -> Val {
        let target = target.or(self.event.target.mon());
        let source = source.or(self.event.source.mon());
        let effect = if effect.is_none() { self.effect } else { effect };
        let Some(t) = target else { return Val::Num(0.0) };
        if self.p(t).hp == 0 {
            return Val::Num(0.0);
        }
        if !self.p(t).is_active {
            return Val::False;
        }
        if self.sides[1 - t.s as usize].pokemon_left == 0 {
            return Val::False;
        }
        let b = match self.run_event(Ev::ChangeBoost, Arg::Mon(t), source, effect, Val::Boosts(boost)) {
            Val::Boosts(b) => b,
            _ => boost,
        };
        let b = self.get_capped_boost(t, &b);
        let b = match self.run_event(Ev::TryBoost, Arg::Mon(t), source, effect, Val::Boosts(b)) {
            Val::Boosts(b) => b,
            _ => b,
        };
        let mut success = Val::Null;
        let mut boosted = is_secondary;
        let eff_type = if effect.is_move() { EffType::Move } else if effect.is_none() { EffType::Other } else { self.eff(effect.eff).effect_type };
        let eid: &str = if effect.is_none() { "" } else { &self.eff(effect.eff).id };
        for (stat, amount) in b.iter() {
            let mut one = SparseBoosts::default();
            one.set(stat, amount);
            let mut boost_by = self.boost_by(t, &one);
            let mut msg = "-boost";
            if amount < 0 || self.p(t).boosts[stat as usize] == -6 {
                msg = "-unboost";
                boost_by = -boost_by;
            }
            let sname = BOOST_NAMES[stat as usize];
            if boost_by != 0 {
                success = Val::True;
                match eid {
                    "bellydrum" | "angerpoint" => {
                        let full = self.eff(effect.eff).fullname.clone();
                        let atk = self.p(t).boosts[0] as i32;
                        add!(self, "-setboost", t, "atk", atk, format!("[from] {full}"));
                    }
                    _ => {
                        if !effect.is_none() {
                            if eff_type == EffType::Move {
                                add!(self, msg, t, sname, boost_by);
                            } else if eff_type == EffType::Item {
                                let n = self.eff(effect.eff).name.clone();
                                add!(self, msg, t, sname, boost_by, format!("[from] item: {n}"));
                            } else {
                                if eff_type == EffType::Ability && !boosted {
                                    let n = self.eff(effect.eff).name.clone();
                                    add!(self, "-ability", t, n, "boost");
                                    boosted = true;
                                }
                                add!(self, msg, t, sname, boost_by);
                            }
                        }
                    }
                }
                self.run_event(Ev::AfterEachBoost, Arg::Mon(t), source, effect, Val::Boosts(one));
            } else if eff_type == EffType::Ability {
                if is_secondary || is_self {
                    add!(self, msg, t, sname, boost_by);
                }
            } else if !is_secondary && !is_self {
                add!(self, msg, t, sname, boost_by);
            }
        }
        self.run_event(Ev::AfterBoost, Arg::Mon(t), source, effect, Val::Boosts(b));
        if success == Val::True {
            if b.iter().any(|x| x.1 > 0) {
                self.pm(t).stats_raised_this_turn = true;
            }
            if b.iter().any(|x| x.1 < 0) {
                self.pm(t).stats_lowered_this_turn = true;
            }
        }
        success
    }

    // ------------------------------------------------------------------
    // side & slot conditions (side.ts)
    // ------------------------------------------------------------------
    pub fn add_side_condition(&mut self, s: u8, status: EffId, source: Option<Mon>, source_effect: EffRef) -> Val {
        let source = source.or(self.event.target.mon()).or_else(|| self.active_at(s as usize, 0));
        let Some(source) = source else { return Val::False };
        if self.sides[s as usize].side_conditions.iter().any(|x| x.0 == status) {
            if !self.eff(status).has(self.d.cbs(Ev::SideRestart)[P_ON]) {
                return Val::False;
            }
            return self.single_event(Ev::SideRestart, EffRef::eff(status), StateLoc::SideCond(s, status), Arg::Side(s), Arg::Mon(source), source_effect, Val::Undef);
        }
        let mut st = self.init_state(status, Holder::Side(s), None);
        st.source = Some(source);
        st.source_slot = Some((source.s, self.p(source).position));
        st.duration = self.eff(status).duration.map(|x| x as f64);
        self.sides[s as usize].side_conditions.push((status, st));
        if self.eff(status).duration_cb {
            let a0 = self.active_at(s as usize, 0);
            let dur = self.call_duration_callback(status, Arg::from_opt(a0), Some(source), source_effect);
            if let Some(x) = self.state_mut(StateLoc::SideCond(s, status)) {
                x.duration = Some(dur);
            }
        }
        if !self.single_event(Ev::SideStart, EffRef::eff(status), StateLoc::SideCond(s, status), Arg::Side(s), Arg::Mon(source), source_effect, Val::Undef).truthy() {
            self.sides[s as usize].side_conditions.retain(|x| x.0 != status);
            return Val::False;
        }
        self.run_event(Ev::SideConditionStart, Arg::Side(s), Some(source), EffRef::eff(status), Val::Undef);
        Val::True
    }
    pub fn remove_side_condition(&mut self, s: u8, status: EffId) -> bool {
        if !self.sides[s as usize].side_conditions.iter().any(|x| x.0 == status) {
            return false;
        }
        self.single_event(Ev::SideEnd, EffRef::eff(status), StateLoc::SideCond(s, status), Arg::Side(s), Arg::None, EffRef::NONE, Val::Undef);
        self.sides[s as usize].side_conditions.retain(|x| x.0 != status);
        true
    }
    pub fn has_side_condition(&self, s: u8, id: &str) -> bool {
        match self.d.cond(id) {
            Some(e) => self.sides[s as usize].side_conditions.iter().any(|x| x.0 == e),
            None => false,
        }
    }
    pub fn add_slot_condition(&mut self, s: u8, pos: u8, status: EffId, source: Option<Mon>, source_effect: EffRef) -> Val {
        let source = source.or(self.event.target.mon());
        let Some(source) = source else { return Val::False };
        if self.sides[s as usize].slot_conditions[pos as usize].iter().any(|x| x.0 == status) {
            if !self.eff(status).has(self.d.cbs(Ev::Restart)[P_ON]) {
                return Val::False;
            }
            let tgt = self.active_at(s as usize, pos as usize);
            return self.single_event(Ev::Restart, EffRef::eff(status), StateLoc::Slot(s, pos, status), Arg::from_opt(tgt), Arg::Mon(source), source_effect, Val::Undef);
        }
        let mut st = self.init_state(status, Holder::Side(s), None);
        st.source = Some(source);
        st.source_slot = Some((source.s, self.p(source).position));
        st.is_slot_condition = true;
        st.duration = self.eff(status).duration.map(|x| x as f64);
        self.sides[s as usize].slot_conditions[pos as usize].push((status, st));
        if self.eff(status).duration_cb {
            let a0 = self.active_at(s as usize, 0);
            let dur = self.call_duration_callback(status, Arg::from_opt(a0), Some(source), source_effect);
            if let Some(x) = self.state_mut(StateLoc::Slot(s, pos, status)) {
                x.duration = Some(dur);
            }
        }
        let tgt = self.active_at(s as usize, pos as usize);
        if !self.single_event(Ev::Start, EffRef::eff(status), StateLoc::Slot(s, pos, status), Arg::from_opt(tgt), Arg::Mon(source), source_effect, Val::Undef).truthy() {
            self.sides[s as usize].slot_conditions[pos as usize].retain(|x| x.0 != status);
            return Val::False;
        }
        Val::True
    }
    pub fn remove_slot_condition(&mut self, s: u8, pos: u8, status: EffId) -> bool {
        if !self.sides[s as usize].slot_conditions[pos as usize].iter().any(|x| x.0 == status) {
            return false;
        }
        let tgt = self.active_at(s as usize, pos as usize);
        self.single_event(Ev::End, EffRef::eff(status), StateLoc::Slot(s, pos, status), Arg::from_opt(tgt), Arg::None, EffRef::NONE, Val::Undef);
        self.sides[s as usize].slot_conditions[pos as usize].retain(|x| x.0 != status);
        true
    }

    // ------------------------------------------------------------------
    // field (field.ts)
    // ------------------------------------------------------------------
    pub fn set_weather(&mut self, status: EffId, source: Option<Mon>, source_effect: EffRef) -> Val {
        let se = if source_effect.is_none() { self.effect } else { source_effect };
        let source = source.or(self.event.target.mon());
        let se_type = if se.is_move() { EffType::Move } else if se.is_none() { EffType::Other } else { self.eff(se.eff).effect_type };
        if self.field.weather == status {
            if se_type == EffType::Ability {
                return Val::False;
            } else {
                return Val::False;
            }
        }
        if let Some(src) = source {
            let r = self.run_event(Ev::SetWeather, Arg::Mon(src), Some(src), EffRef::eff(status), Val::Undef);
            if !r.truthy() {
                if r == Val::False {
                    let wname = self.eff(self.field.weather).id.clone();
                    if self.move_weather_of(se).is_some() {
                        let sfull = self.eff(se.eff).fullname.clone();
                        add!(self, "-fail", src, sfull, format!("[from] {wname}"));
                    } else if se_type == EffType::Ability {
                        let sfull = self.eff(se.eff).fullname.clone();
                        add!(self, "-ability", src, sfull, format!("[from] {wname}"), "[fail]");
                    }
                }
                return Val::Null;
            }
        }
        let prev = self.field.weather;
        let prev_state = self.field.weather_state.clone();
        let mut st = self.init_state(status, Holder::None, None);
        if let Some(src) = source {
            st.source = Some(src);
            st.source_slot = Some((src.s, self.p(src).position));
        }
        st.duration = self.eff(status).duration.map(|x| x as f64);
        self.field.weather = status;
        self.field.weather_state = st;
        if self.eff(status).duration_cb {
            let dur = self.call_duration_callback(status, Arg::from_opt(source), source, se);
            self.field.weather_state.duration = Some(dur);
        }
        if !self.single_event(Ev::FieldStart, EffRef::eff(status), StateLoc::Weather, Arg::Field, Arg::from_opt(source), se, Val::Undef).truthy() {
            self.field.weather = prev;
            self.field.weather_state = prev_state;
            return Val::False;
        }
        self.each_event(Ev::WeatherChange, se);
        Val::True
    }
    fn move_weather_of(&self, e: EffRef) -> Option<EffId> {
        if e.is_move() {
            return self.mv(e.mv).weather;
        }
        None
    }
    pub fn clear_weather(&mut self) -> bool {
        if self.field.weather == self.d.c.empty {
            return false;
        }
        let prev = self.field.weather;
        self.single_event(Ev::FieldEnd, EffRef::eff(prev), StateLoc::Weather, Arg::Field, Arg::None, EffRef::NONE, Val::Undef);
        self.field.weather = self.d.c.empty;
        let empty = self.d.c.empty;
        Battle::clear_state(&mut self.field.weather_state, empty);
        self.each_event(Ev::WeatherChange, EffRef::NONE);
        true
    }
    pub fn set_terrain(&mut self, status: EffId, source: Option<Mon>, source_effect: EffRef) -> Val {
        let se = if source_effect.is_none() { self.effect } else { source_effect };
        let source = source.or(self.event.target.mon());
        let Some(src) = source else { return Val::False };
        if self.field.terrain == status {
            return Val::False;
        }
        let prev = self.field.terrain;
        let prev_state = self.field.terrain_state.clone();
        let mut st = self.init_state(status, Holder::None, None);
        st.source = Some(src);
        st.source_slot = Some((src.s, self.p(src).position));
        st.duration = self.eff(status).duration.map(|x| x as f64);
        self.field.terrain = status;
        self.field.terrain_state = st;
        if self.eff(status).duration_cb {
            let dur = self.call_duration_callback(status, Arg::Mon(src), Some(src), se);
            self.field.terrain_state.duration = Some(dur);
        }
        if !self.single_event(Ev::FieldStart, EffRef::eff(status), StateLoc::Terrain, Arg::Field, Arg::Mon(src), se, Val::Undef).truthy() {
            self.field.terrain = prev;
            self.field.terrain_state = prev_state;
            return Val::False;
        }
        self.each_event(Ev::TerrainChange, se);
        Val::True
    }
    pub fn clear_terrain(&mut self) -> bool {
        if self.field.terrain == self.d.c.empty {
            return false;
        }
        let prev = self.field.terrain;
        self.single_event(Ev::FieldEnd, EffRef::eff(prev), StateLoc::Terrain, Arg::Field, Arg::None, EffRef::NONE, Val::Undef);
        self.field.terrain = self.d.c.empty;
        let empty = self.d.c.empty;
        Battle::clear_state(&mut self.field.terrain_state, empty);
        self.each_event(Ev::TerrainChange, EffRef::NONE);
        true
    }
    /// field.effectiveTerrain(target)
    pub fn effective_terrain(&mut self, target: Option<Mon>) -> &'static str {
        let t = target.map(Arg::Mon).unwrap_or(self.event.target);
        if self.run_event(Ev::TryTerrain, t, None, EffRef::NONE, Val::Undef).truthy() {
            &self.eff(self.field.terrain).id
        } else {
            ""
        }
    }
    pub fn is_terrain(&mut self, id: &str, target: Option<Mon>) -> bool {
        if self.field.terrain == self.d.c.empty {
            // the event still runs in Showdown; with no terrain no handler can matter
            return false;
        }
        self.effective_terrain(target) == id
    }
    pub fn add_pseudo_weather(&mut self, status: EffId, source: Option<Mon>, source_effect: EffRef) -> Val {
        let source = source.or(self.event.target.mon());
        if self.field.pseudo_weather.iter().any(|x| x.0 == status) {
            if !self.eff(status).has(self.d.cbs(Ev::FieldRestart)[P_ON]) {
                return Val::False;
            }
            return self.single_event(Ev::FieldRestart, EffRef::eff(status), StateLoc::Pseudo(status), Arg::Field, Arg::from_opt(source), source_effect, Val::Undef);
        }
        let mut st = self.init_state(status, Holder::None, None);
        st.source = source;
        st.source_slot = source.map(|s| (s.s, self.p(s).position));
        st.duration = self.eff(status).duration.map(|x| x as f64);
        self.field.pseudo_weather.push((status, st));
        if self.eff(status).duration_cb {
            let dur = self.call_duration_callback(status, Arg::from_opt(source), source, source_effect);
            if let Some(x) = self.state_mut(StateLoc::Pseudo(status)) {
                x.duration = Some(dur);
            }
        }
        if !self.single_event(Ev::FieldStart, EffRef::eff(status), StateLoc::Pseudo(status), Arg::Field, Arg::from_opt(source), source_effect, Val::Undef).truthy() {
            self.field.pseudo_weather.retain(|x| x.0 != status);
            return Val::False;
        }
        self.run_event(Ev::PseudoWeatherChange, Arg::from_opt(source), source, EffRef::eff(status), Val::Undef);
        Val::True
    }
    pub fn remove_pseudo_weather(&mut self, status: EffId) -> bool {
        if !self.field.pseudo_weather.iter().any(|x| x.0 == status) {
            return false;
        }
        self.single_event(Ev::FieldEnd, EffRef::eff(status), StateLoc::Pseudo(status), Arg::Field, Arg::None, EffRef::NONE, Val::Undef);
        self.field.pseudo_weather.retain(|x| x.0 != status);
        true
    }
    pub fn has_pseudo(&self, id: &str) -> bool {
        self.field.pseudo_weather.iter().any(|x| self.eff(x.0).id == id)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchResult {
    Ok,
    Failed,
    PursuitFaint,
}
