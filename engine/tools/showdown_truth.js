// (2026-10-16) Showdown itself as the third engine for engine_diff: plays one turn of the SAME random mid-battle position in
// the real simulator, many seeds, and answers with the same outcome features engine_diff compares.
//
//   node showdown_truth.js <showdown folder> <format id>        (then JSON lines on stdin, JSON lines on stdout)
//
// request : {id, texts: [team text of side 0 (4 sets), team text of side 1], turn, calls: [[name, args, kw], ...], choices: ["...", "..."],
//            seeds, seed0}
// response: {id, rows: [{feature: value, ...} per seed], illegal: n}
//
// Positions are built like vgcsim's Battle.from_brought: four Pokemon per side, the first two lead, NO switch-in effects
// (abilities' onStart never runs), then engine_diff's set_mon / set_field / set_side calls are applied by hand.
const path = require('path');
const readline = require('readline');
const sim = require(path.resolve(process.argv[2], 'dist', 'sim'));
const SD_FAST_N = require('./sd_fast.js')(process.argv[2]);   // (2026-10-18) cached callback names (see sd_fast.js)
const {Battle, Teams} = sim;
// (2026-10-17) build the position ONCE per request and copy it per seed (State.deserializeBattle: ~0.37 ms vs ~1.07 ms
// to build), with a fresh PRNG per seed. Seeds whose log isn't asked for run with logging switched off (Showdown never
// reads its log for game logic: attrLastMove / retargetLastMove return when no move line was written).
const {State} = require(path.resolve(process.argv[2], 'dist', 'sim', 'state'));
const {PRNG} = require(path.resolve(process.argv[2], 'dist', 'sim', 'prng'));
const NOLOG = () => {};
function quietLog(battle) { battle.add = NOLOG; battle.addMove = NOLOG; battle.addSplit = NOLOG; battle.lastMoveLine = -1; }
const formatId = process.argv[3];

const STATUSES = ['brn', 'par', 'psn', 'tox', 'slp', 'frz'];
const BOOSTS = ['atk', 'def', 'spa', 'spd', 'spe'];
const WEATHER = {rain: 'raindance', sun: 'sunnyday', sand: 'sandstorm', snow: 'snowscape'};
const TERRAIN = {electric: 'electricterrain', grassy: 'grassyterrain', psychic: 'psychicterrain', misty: 'mistyterrain'};

// (2026-10-16) seeds were linear in n: Showdown's Gen 5 LCG then gave CORRELATED streams across games (a 59/41 random
// target split over 300 'independent' games). Now each 16-bit part is a hashed (murmur3 finalizer) value.
function mix32(h) {
	h = Math.imul(h ^ (h >>> 16), 0x85ebca6b);
	h = Math.imul(h ^ (h >>> 13), 0xc2b2ae35);
	return (h ^ (h >>> 16)) >>> 0;
}
function seedFor(n) {
	n = n >>> 0;
	const a = mix32(n * 2 + 0x9e3779b9), b = mix32(a ^ 0x7f4a7c15);
	return [a & 0xffff, a >>> 16, b & 0xffff, b >>> 16];
}

function makeTeam(texts) {
	const sets = Teams.import(texts);
	sets.forEach((s, i) => { s.name = 'm' + i; });
	return Teams.pack(sets);
}

function mon(battle, side, idx) { return battle.sides[side].pokemon.find(p => p.name === 'm' + idx); }

function applyCall(battle, name, args, kw) {
	if (name === 'set_mon') {
		const [s, i] = args;
		const p = mon(battle, s, i);
		if (kw.fainted) {
			p.hp = 0; p.fainted = true; p.status = ''; p.isActive = false;
			battle.sides[s].pokemonLeft--;
			battle.sides[s].totalFainted++;
			return;
		}
		if (kw.hp_frac !== undefined) p.hp = Math.max(1, Math.floor(p.maxhp * kw.hp_frac));
		if (kw.hp !== undefined) p.hp = Math.max(1, Math.min(p.maxhp, kw.hp));      // (2026-11-05, situation.py) absolute HP
		if (kw.status) {
			// by hand: setStatus refuses bench Pokemon. The condition's own onStart still runs (sleep length, toxic stage)
			p.status = kw.status;
			p.statusState = {id: kw.status, target: p};
			battle.singleEvent('Start', battle.dex.conditions.get(kw.status), p.statusState, p, null, null);
			// vgcsim's set_mon puts a sleeper at 2 turns
			if (kw.status === 'slp') { p.statusState.startTime = 2; p.statusState.time = 2; }
		}
		if (kw.boosts) for (const [k, v] of Object.entries(kw.boosts)) p.boosts[k] = v;
		if (kw.first_turn !== undefined) { p.activeMoveActions = kw.first_turn ? 0 : 2; }   // (vgcsim's first_turn is Fake Out's flag only; activeTurns stays 1 for everyone)
		if (kw.item === '') { p.item = ''; p.itemState = {id: '', target: p}; }
		// (2026-11-05, situation.py) a different held item, silently (no onStart: as vgcsim's set_mon)
		else if (kw.item) { const it = battle.dex.items.get(kw.item); if (!it.exists) throw new Error('unknown item ' + kw.item); p.item = it.id; p.itemState = {id: it.id, target: p}; }
		if (kw.pp_used) kw.pp_used.forEach((u, k) => { if (p.moveSlots[k]) p.moveSlots[k].pp = Math.max(0, p.moveSlots[k].maxpp - u); });
		if (kw.protect_streak !== undefined && kw.protect_streak > 0) { p.addVolatile('stall'); p.volatiles['stall'].counter = Math.pow(3, kw.protect_streak); }
		// (2026-10-17) already Mega Evolved (vgcsim set_mega_form / the exact engine's set_mon(mega)): the form, its ability and
		// stats, the side's Mega used up; the new ability's Start does NOT run (no switch-in effects in these positions)
		if (kw.mega && p.canMegaEvo) {
			const origSingle = battle.singleEvent.bind(battle);
			battle.singleEvent = function (eventid, ...rest) {
				if (eventid === 'Start') return undefined;
				return origSingle(eventid, ...rest);
			};
			try { p.formeChange(p.canMegaEvo, p.getItem(), true); } finally { battle.singleEvent = origSingle; }
			for (const ally of p.side.pokemon) ally.canMegaEvo = false;
			if (['unnerve', 'asoneglastrier', 'asonespectrier'].includes(p.ability)) p.abilityState.unnerved = true;
		}
	} else if (name === 'set_field') {
		const f = battle.field;
		const src = battle.sides[0].active[0];
		// no item reacts to the new field (a Grassy Seed would fire now; vgcsim's set_field fires nothing)
		const stash = [];
		for (const sd of battle.sides) for (const p of sd.pokemon) { stash.push([p, p.item, p.itemState]); p.item = ''; p.itemState = {id: '', target: p}; }
		// ... and no ability reacts to the new weather / terrain either (Mimicry, Forecast...)
		const origEach = battle.eachEvent.bind(battle);
		battle.eachEvent = function (eventid, ...rest) {
			if (eventid === 'WeatherChange' || eventid === 'TerrainChange') return undefined;
			return origEach(eventid, ...rest);
		};
		try {
		if (kw.weather) { f.setWeather(WEATHER[kw.weather], src); f.weatherState.duration = kw.weather_turns; }
		if (kw.terrain) { f.setTerrain(TERRAIN[kw.terrain], src); f.terrainState.duration = kw.terrain_turns; }
		if (kw.trick_room) { f.addPseudoWeather('trickroom', src); f.pseudoWeather.trickroom.duration = kw.trick_room; }
		} finally {
			battle.eachEvent = origEach;
			for (const [p, it, st] of stash) { p.item = it; p.itemState = st; }
		}
	} else if (name === 'set_side') {
		const side = battle.sides[args[0]];
		const src = side.active[0];
		for (const [k, id] of [['tailwind', 'tailwind'], ['reflect', 'reflect'], ['light_screen', 'lightscreen'], ['aurora_veil', 'auroraveil']]) {
			if (kw[k]) { side.addSideCondition(id, src); side.sideConditions[id].duration = kw[k]; }
		}
		// (2026-11-05, situation.py) entry hazards and the Mega flag
		if (kw.stealth_rock) side.addSideCondition('stealthrock', src);
		if (kw.sticky_web) side.addSideCondition('stickyweb', src);
		for (const [k, id] of [['spikes', 'spikes'], ['toxic_spikes', 'toxicspikes']]) {
			for (let n = 0; n < (kw[k] || 0); n++) side.addSideCondition(id, src);
		}
		if (kw.mega_used) for (const q of side.pokemon) q.canMegaEvo = false;
	} else if (name === 'set_extra') {
		// (2026-11-05, situation.py) Safeguard (on side args[0]) and Gravity / Magic Room / Wonder Room / Fairy Lock
		const side = battle.sides[args[0]];
		const src = side.active[0];
		if (kw.safeguard) { side.addSideCondition('safeguard', src); side.sideConditions['safeguard'].duration = kw.safeguard; }
		for (const [k, id] of [['gravity', 'gravity'], ['magic_room', 'magicroom'], ['wonder_room', 'wonderroom'], ['fairy_lock', 'fairylock']]) {
			if (kw[k]) { battle.field.addPseudoWeather(id, src); battle.field.pseudoWeather[id].duration = kw[k]; }
		}
	} else if (name === 'set_ability') {
		// (2026-11-05, situation.py) the current ability, silently (no start effects: as vgcsim's set_ability)
		const p = mon(battle, args[0], args[1]);
		const ab = battle.dex.abilities.get(kw.ability);
		if (!ab.exists) throw new Error('unknown ability ' + kw.ability);
		p.ability = ab.id; p.abilityState = {id: ab.id, target: p};
	} else {
		throw new Error('unknown call ' + name);
	}
}

function build(req, seed) {
	const battle = new Battle({formatid: formatId, seed: seedFor(seed)});
	// no switch-in effects while the leads come out
	let quiet = true;
	const origField = battle.fieldEvent.bind(battle);
	battle.fieldEvent = function (eventid, ...rest) {
		if (quiet && eventid === 'SwitchIn') return undefined;
		return origField(eventid, ...rest);
	};
	battle.setPlayer('p1', {name: 'A', team: makeTeam(req.texts[0])});
	battle.setPlayer('p2', {name: 'B', team: makeTeam(req.texts[1])});
	battle.choose('p1', 'team 1234');
	battle.choose('p2', 'team 1234');
	quiet = false;
	for (const side of battle.sides) for (const p of side.active) {
		if (!p) continue;
		p.isStarted = true; p.activeTurns = 1;
		// onStart never ran: Unnerve / As One would not be 'unnerved' yet (vgcsim's from_brought primes it)
		if (['unnerve', 'asoneglastrier', 'asonespectrier'].includes(p.ability)) p.abilityState.unnerved = true;
	}
	battle.turn = req.turn;
	for (const [name, args, kw] of req.calls) applyCall(battle, name, args, kw);
	// the Metronome item's counter is a volatile added by the item's onStart at switch-in (which never ran here)
	quiet = true;
	for (const side of battle.sides) for (const p of side.active) {
		if (p && !p.fainted && p.item === 'metronome' && !p.volatiles['metronome']) p.addVolatile('metronome');
	}
	quiet = false;
	return battle;
}

function teamIndex(p) { return Number(p.name.slice(1)); }

function resolve(battle, startTurn) {
	for (let n = 0; n < 12; n++) {
		if (battle.ended) return;
		let pending = false;
		for (const side of battle.sides) {
			const rq = side.activeRequest;
			if (!rq || rq.wait || !rq.forceSwitch) continue;
			pending = true;
			const mons = side.pokemon;
			const bench = mons.filter(p => !p.fainted && !p.isActive).sort((a, b) => teamIndex(a) - teamIndex(b));
			const dead = mons.filter(p => p.fainted).sort((a, b) => teamIndex(a) - teamIndex(b));
			const parts = [];
			for (let k = 0; k < rq.forceSwitch.length; k++) {
				if (!rq.forceSwitch[k]) { parts.push('pass'); continue; }
				const reviving = side.slotConditions[k] && side.slotConditions[k].revivalblessing;
				const pool = reviving ? dead : bench;
				if (!pool.length) { parts.push('pass'); continue; }
				const p = pool.shift();
				if (reviving) { /* dead list only */ } else { /* bench list only */ }
				parts.push('switch ' + (mons.indexOf(p) + 1));
			}
			if (!battle.choose(side.id, parts.join(', '))) battle.choose(side.id, 'default');
		}
		if (!pending) return;
	}
}

// ---- the multi-turn policy (mirrors multi_turn_truth.py's fast-engine side exactly) ---------------------------------------
const FOE_T = new Set(['normal', 'any', 'adjacentFoe']);
const ALLY_T = new Set(['adjacentAlly', 'adjacentAllyOrSelf']);
const FIRST_ONLY = new Set(['fakeout', 'firstimpression', 'matblock', 'gigatonhammer', 'bloodmoon']);

function chooseMulti(battle, side, t, mem) {
	const rq = side.activeRequest;
	if (!rq || rq.wait || rq.forceSwitch || !rq.active) return null;
	const foe = side.foe;
	const foeAt = [0, 1].map(k => !!(foe.active[k] && !foe.active[k].fainted && foe.active[k].hp > 0));
	const parts = [];
	for (let p = 0; p < 2; p++) {
		const mon = side.active[p];
		const ra = rq.active[p];
		if (!mon || mon.fainted || mon.hp <= 0 || !ra) { parts.push('pass'); continue; }
		const moves = ra.moves.map(m => ({id: m.id, pp: m.pp === undefined ? 99 : m.pp, disabled: !!m.disabled || !!(mon.moveSlots.find(x => x.id === m.id) || {}).disabled,   // Imprison disables 'hidden' (not in the request)
			
			cat: battle.dex.moves.get(m.id).category, tg: m.target || battle.dex.moves.get(m.id).target}));
		const n = moves.length;
		if (process.env.MT_DEBUG) console.error("req", side.id, p, JSON.stringify(ra.moves.map(m => [m.id, m.pp, m.disabled])));
		// ally moves need a target in doubles (Showdown rejects 'move N' for adjacentAlly): the partner, or ourselves
		// when it is gone; same rule as the fast engine's legal_actions
		const allyUp = !!(side.active[1 - p] && !side.active[1 - p].fainted && side.active[1 - p].hp > 0);
		const tgt = m => (FOE_T.has(m.tg) ? (foeAt[0] ? 1 : 2) : ALLY_T.has(m.tg) ? (allyUp ? -(2 - p) : -(p + 1)) : 0);
		const cands = [];
		const memId = mem[side.id + mon.name];
		if (memId !== undefined) {
			const j = moves.findIndex(m => m.id === memId);
			if (j >= 0 && !FIRST_ONLY.has(memId) && (moves[j].cat !== 'Status' || String(mon.item).startsWith('choice'))) cands.push(j);
		}
		for (let j = 0; j < n; j++) {
			const idx = (t + p + j) % n;
			const m = moves[idx];
			if (m.pp <= 0 || ALLY_T.has(m.tg) || FIRST_ONLY.has(m.id) || m.cat === 'Status') continue;
			cands.push(idx);
		}
		for (let idx = 0; idx < n; idx++) {
			const m = moves[idx];
			if (m.pp > 0 && !ALLY_T.has(m.tg) && !FIRST_ONLY.has(m.id)) cands.push(idx);
		}
		for (let idx = 0; idx < n; idx++) cands.push(idx);
		let pick = cands.find(idx => !moves[idx].disabled && moves[idx].pp > 0);
		// nothing usable = Struggle: remember nothing (the fast side's rule picks Struggle, which isn't a move slot)
		const usable = pick !== undefined;
		if (pick === undefined) pick = moves.findIndex(m => !m.disabled);
		if (pick < 0) pick = 0;
		if (usable && mem[side.id + mon.name] === undefined) mem[side.id + mon.name] = moves[pick].id;
		const locked = ra.moves.length === 1 && ra.moves[0].pp === undefined;   // charge turn / Outrage lock: no target
		const tg = locked ? 0 : tgt(moves[pick]);
		parts.push('move ' + (pick + 1) + (tg ? ' ' + tg : ''));
	}
	return parts.join(', ');
}

function outcome(battle) {
	const f = {};
	battle.sides.forEach((side, s) => {
		for (const p of side.pokemon) {
			const i = teamIndex(p);
			const pre = `s${s}m${i}`;
			const dead = p.fainted || p.hp <= 0;
			f[pre + '.hp'] = dead ? 0 : p.hp / p.maxhp;
			f[pre + '.faint'] = dead ? 1 : 0;
			const st = !dead && STATUSES.includes(p.status) ? p.status : '';
			for (const x of STATUSES) f[pre + '.' + x] = st === x ? 1 : 0;
			for (const x of BOOSTS) f[pre + '.' + x] = dead ? 0 : (p.boosts[x] || 0);
			f[pre + '.active'] = p.isActive && !dead ? 1 : 0;
		}
	});
	const w = battle.field.weather || '';
	const t = battle.field.terrain || '';
	for (const x of ['raindance', 'sunnyday', 'sandstorm', 'snowscape']) f['weather.' + x] = w === x ? 1 : 0;
	for (const x of ['electricterrain', 'grassyterrain', 'psychicterrain', 'mistyterrain']) f['terrain.' + x] = t === x ? 1 : 0;
	f['ended'] = battle.ended ? 1 : 0;
	return f;
}

function handle(req) {
	const rows = [];
	const logs = [];
	let illegal = 0;
	const illegalScript = [];
	let error = null;
	let base = null;
	for (let k = 0; k < req.seeds; k++) {
		try {
			const seed = req.seed0 + k * 1000003;
			let battle;
			if (process.env.TRUTH_REBUILD) battle = build(req, seed);   // the old way (checks)
			else if (base === null) { battle = build(req, seed); base = JSON.stringify(State.serializeBattle(battle)); }   // a string: the snapshot holds LIVE references (its log array is battle.log)
			else { battle = State.deserializeBattle(base); battle.prng = new PRNG(seedFor(seed)); battle.prngSeed = battle.prng.getSeed(); }
			if (!(req.log && k < req.log)) quietLog(battle);
			const t0 = battle.turn;
			const mem = {};
			if (req.turns > 1) {
				// the move each Pokemon chose on turn 0 is the one it keeps repeating (when it may)
				battle.sides.forEach((sd, si) => {
					const rq = sd.activeRequest;
					const parts = String(req.choices[si]).split(',').map(x => x.trim());
					parts.forEach((part, p) => {
						const mm = part.match(/^move (\d+)/);
						if (mm && rq && rq.active && rq.active[p] && sd.active[p]) mem[sd.id + sd.active[p].name] = rq.active[p].moves[Number(mm[1]) - 1].id;
					});
				});
			}
			const ok1 = battle.choose('p1', req.choices[0]);
			const ok2 = battle.choose('p2', req.choices[1]);
			if (!ok1 || !ok2) { illegal++; }
			resolve(battle, t0);
			for (let t = 1; t < (req.turns || 1) && !battle.ended; t++) {
				// (2026-10-17) req.script[t - 1] = [p1 choice, p2 choice] replaces the rule on that turn (tools/scenarios.py)
				const scripted = req.script && req.script[t - 1];
				const c = scripted ? scripted.slice() : battle.sides.map(sd => chooseMulti(battle, sd, t, mem));
				if (process.env.MT_DEBUG) console.error("t", t, JSON.stringify(mem), JSON.stringify(c), JSON.stringify(req.choices));
				if (c[0] === null || c[1] === null) break;
				const tt = battle.turn;
				['p1', 'p2'].forEach((id, si) => {
					if (battle.choose(id, c[si])) return;
					// a locked move (charge turn, Outrage...) takes no target
					const bare = c[si].split(',').map(x => x.trim().replace(/^(move \d+) \d+/, '$1')).join(', ');
					if (!battle.choose(id, bare)) { illegal++; if (scripted) illegalScript.push([t, si]); battle.choose(id, 'default'); }
				});
				resolve(battle, tt);
			}
			rows.push(outcome(battle));
			if (req.log && k < req.log) logs.push(battle.log.slice());
		} catch (e) {
			error = String(e && e.stack || e).slice(0, 400);
			break;
		}
	}
	return {id: req.id, rows, illegal, error, logs, illegal_script: illegalScript};
}

if (require.main !== module) {
	module.exports = {build, handle, seedFor, outcome};
} else {
	const rl = readline.createInterface({input: process.stdin});
	rl.on('line', line => {
		if (!line.trim()) return;
		let req;
		try { req = JSON.parse(line); } catch (e) { process.stdout.write(JSON.stringify({error: 'bad json'}) + '\n'); return; }
		process.stdout.write(JSON.stringify(handle(req)) + '\n');
	});
}
