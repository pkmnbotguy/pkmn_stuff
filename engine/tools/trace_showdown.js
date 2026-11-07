// Records every random number Showdown draws in random-vs-random battles, with where it came from.
// Used to make vgcsim draw the SAME random numbers in the SAME order (M4: identical battles).
//
//   node trace_showdown.js <showdown> <format> stats <pairs.json> <count> [seed]
//       tally of call sites (which code asked for random numbers, and how often)
//   node trace_showdown.js <showdown> <format> record <pairs.json> <first> <count> <out.json>
//       full record per battle: seed, teams, every choice, every random call, state at every request
const path = require('path');
const fs = require('fs');
const [showdown, formatId, mode, ...rest] = process.argv.slice(2);
const sim = require(path.resolve(path.join(showdown, 'dist', 'sim')));
const {Battle, Teams} = sim;
const {Pokemon} = require(path.resolve(path.join(showdown, 'dist', 'sim', 'pokemon')));

function makeRng(seed) {
	let s = (seed >>> 0) || 1;
	return n => {
		s ^= s << 13; s >>>= 0; s ^= s >>> 17; s ^= s << 5; s >>>= 0;
		return Math.floor((s / 4294967296) * n);
	};
}

// ---- instrumentation -------------------------------------------------------------------------
// A stack of what the simulator is doing (events, sorts), so each random call can be labelled.
let ctx = [];
let calls = null; // array when recording
function label(battle) {
	const top = ctx.length ? ctx[ctx.length - 1] : '';
	const ev = battle.event ? battle.event.id : '';
	const eff = battle.effect ? battle.effect.id : '';
	const st = new Error().stack.split('\n').slice(3, 9).map(l => {
		const m = l.match(/at (?:new )?([\w.$<>]+)/);
		return m ? m[1].replace(/^Object\./, '').replace(/^(Battle|BattleActions|BattleQueue|Pokemon|Side|Field|PRNG)\./, '') : '?';
	});
	return {top, ev, eff, st};
}
function wrap(proto, name, makeTag) {
	const orig = proto[name];
	proto[name] = function (...args) {
		ctx.push(makeTag.call(this, ...args));
		try { return orig.apply(this, args); } finally { ctx.pop(); }
	};
}
wrap(Battle.prototype, 'runEvent', function (id) { return 'run:' + id; });
wrap(Battle.prototype, 'fieldEvent', function (id) { return 'field:' + id; });
wrap(Battle.prototype, 'eachEvent', function (id) { return 'each:' + id; });
wrap(Battle.prototype, 'singleEvent', function (id, eff) { return 'single:' + id + ':' + (eff && eff.id); });

function instrument(battle) {
	const prng = battle.prng;
	const orig = prng.random.bind(prng);
	prng.random = function (from, to) {
		const r = orig(from, to);
		if (calls) {
			const l = label(battle);
			calls.push({from: from === undefined ? null : from, to: to === undefined ? null : to, r, L: battle.log.length, ...l});
		}
		return r;
	};
}

// ---- the random policy (same as showdown_random_battles.js, plus random team order) ------------
function choose(battle, side, rnd) {
	const req = side.activeRequest;
	if (!req || req.wait) return null;
	if (req.teamPreview) {
		const idx = [0, 1, 2, 3, 4, 5];
		for (let i = 5; i > 0; i--) { const j = rnd(i + 1); [idx[i], idx[j]] = [idx[j], idx[i]]; }
		return 'team ' + idx.slice(0, 4).map(i => i + 1).join('');
	}
	const foeSide = side.foe;
	const pokemon = req.side.pokemon;
	const taken = [];
	const benchFree = () => pokemon.map((p, i) => i).filter(i => i >= 2 && !pokemon[i].condition.endsWith(' fnt') && !taken.includes(i));
	const parts = [];
	if (req.forceSwitch) {
		for (let k = 0; k < req.forceSwitch.length; k++) {
			if (!req.forceSwitch[k]) { parts.push('pass'); continue; }
			const bench = benchFree();
			if (!bench.length) { parts.push('pass'); continue; }
			const to = bench[rnd(bench.length)];
			taken.push(to);
			parts.push(`switch ${to + 1}`);
		}
		return parts.join(', ');
	}
	let megaDone = false;
	for (let k = 0; k < req.active.length; k++) {
		const act = req.active[k];
		const mon = side.active[k];
		if (!mon || mon.fainted || pokemon[k].condition.endsWith(' fnt')) { parts.push('pass'); continue; }
		const bench = benchFree();
		if (bench.length && !act.trapped && rnd(10) === 0) {
			const to = bench[rnd(bench.length)];
			taken.push(to);
			parts.push(`switch ${to + 1}`);
			continue;
		}
		const usable = act.moves.map((m, j) => ({m, j})).filter(x => !x.m.disabled && (x.m.pp === undefined || x.m.pp > 0));
		const mega = !megaDone && act.canMegaEvo;
		if (mega) megaDone = true;
		if (!usable.length) { parts.push('move 1' + (mega ? ' mega' : '')); continue; }
		const pick = usable[rnd(usable.length)];
		let target = '';
		if (['normal', 'any', 'adjacentFoe'].includes(pick.m.target)) {
			const foes = [0, 1].filter(f => foeSide.active[f] && !foeSide.active[f].fainted).map(f => f + 1);
			target = ' ' + (foes.length ? foes[rnd(foes.length)] : 1);
		} else if (['adjacentAlly', 'adjacentAllyOrSelf'].includes(pick.m.target)) {
			const ally = side.active[1 - k];
			target = ' ' + (ally && !ally.fainted ? -((1 - k) + 1) : -(k + 1));
		}
		parts.push(`move ${pick.j + 1}${target}${mega ? ' mega' : ''}`);
	}
	return parts.join(', ');
}

// ---- state snapshot (compared with vgcsim's at every request) --------------------------------
const toId = x => String(x || '').toLowerCase().replace(/[^a-z0-9]/g, '');
function snapshot(battle, origOrder) {
	const sides = battle.sides.map((side, s) => {
		const mons = origOrder[s].map(p => {
			const vol = Object.keys(p.volatiles).sort();
			return {
				species: p.species.id, hp: p.hp, maxhp: p.maxhp, status: p.status || '', fainted: !!p.fainted,
				boosts: {...p.boosts}, item: p.item || '', ability: p.ability, active: p.isActive ? p.position : -1,
				vol, statusTurns: p.statusState && (p.statusState.time ?? p.statusState.stage ?? null),
			};
		});
		const sc = {};
		for (const [k, v] of Object.entries(side.sideConditions)) sc[k] = v.duration ?? v.layers ?? 1;
		return {mons, sc};
	});
	const pw = {};
	for (const [k, v] of Object.entries(battle.field.pseudoWeather)) pw[k] = v.duration ?? 1;
	return {
		turn: battle.turn, sides,
		weather: battle.field.weather || '', weatherDur: battle.field.weatherState.duration ?? null,
		terrain: battle.field.terrain || '', terrainDur: battle.field.terrainState.duration ?? null, pw,
		seed: battle.prng.getSeed(),
	};
}

function seedFor(n) {
	// Gen 5 LCG seed (four 16-bit words); vgcsim implements the same generator
	return [(n * 7919 + 13) & 0xffff, (n * 104729 + 7) & 0xffff, (n * 31 + 4242) & 0xffff, (n * 523 + 99) & 0xffff];
}

function runBattle(p, n, record) {
	const battle = new Battle({formatid: formatId, seed: seedFor(n)});
	instrument(battle);
	calls = [];
	battle.setPlayer('p1', {name: 'A', team: Teams.pack(Teams.import(p.a))});
	battle.setPlayer('p2', {name: 'B', team: Teams.pack(Teams.import(p.b))});
	const origOrder = battle.sides.map(s => s.pokemon.slice());
	const rnd = makeRng(n * 2654435761 + 12345);
	const steps = [];
	let guard = 0;
	while (!battle.ended && guard < 2000 && battle.turn <= 300) {
		guard++;
		let any = false;
		for (const side of battle.sides) {
			// state and draw count right before this side's choice (the other side's choice may
			// have just finished a turn)
			const snap = record ? snapshot(battle, origOrder) : null;
			const ncalls = calls.length;
			const ch = choose(battle, side, rnd);
			if (ch !== null && !side.isChoiceDone()) {
				any = true;
				// switch targets as the ORIGINAL team index (Showdown reorders side.pokemon on switches)
				const canon = ch.replace(/switch (\d)/g, (_, d) => 'switch @' + origOrder[side.n].indexOf(side.pokemon[Number(d) - 1]));
				const ok = battle.choose(side.id, ch);
				if (record) steps.push({side: side.n, choice: ch, canon, ok: !!ok, calls: ncalls, snap});
				if (!ok) battle.choose(side.id, 'default');
				if (record && !ok) steps[steps.length - 1].applied = 'default';
			}
		}
		if (!any) break;
	}
	const result = {n, seed: seedFor(n), ended: battle.ended, winner: battle.winner, turns: battle.turn, steps,
		final: record ? snapshot(battle, origOrder) : null, calls, log: record ? battle.log : null};
	calls = null;
	return result;
}

const pairs = mode === 'inputlogs' ? [] : JSON.parse(fs.readFileSync(rest[0], 'utf8'));
if (mode === 'inputlogs') {
	// node trace_showdown.js <showdown> <format> inputlogs <recorded.jsonl> <first> <count> <out.json>
	// replays battles recorded by sim_driver.js (VGCSIM_RECORD) with the same instrumentation
	const [file, first, count, outFile] = rest;
	const lines = fs.readFileSync(file, 'utf8').split(String.fromCharCode(10)).filter(l => l.trim());
	const out = [];
	for (let k = Number(first); k < Math.min(lines.length, Number(first) + Number(count)); k++) {
		const inputLog = JSON.parse(lines[k]).inputLog;
		let spec = null;
		const players = {};
		const choices = [];
		for (const l of inputLog) {
			if (l.startsWith('>start ')) spec = JSON.parse(l.slice(7));
			else if (l.startsWith('>player ')) {
				const sp = l.indexOf(' ', 8);
				players[l.slice(8, sp)] = JSON.parse(l.slice(sp + 1));
			} else if (/^>p[12] /.test(l)) choices.push({side: Number(l[2]) - 1, choice: l.slice(4)});
		}
		const seed = String(spec.seed).split(',').map(Number);
		const battle = new Battle({formatid: spec.formatid, seed});
		instrument(battle);
		calls = [];
		battle.setPlayer('p1', players.p1);
		battle.setPlayer('p2', players.p2);
		const origOrder = battle.sides.map(s => s.pokemon.slice());
		const steps = [];
		for (const c of choices) {
			const snap = snapshot(battle, origOrder);
			const ncalls = calls.length;
			const ok = battle.choose(battle.sides[c.side].id, c.choice);
			steps.push({side: c.side, choice: c.choice, ok: !!ok, calls: ncalls, snap});
		}
		const exp = t => Teams.export(Teams.unpack(t));
		out.push({n: k, seed, ended: battle.ended, winner: battle.winner, turns: battle.turn, steps,
			final: snapshot(battle, origOrder), calls, log: battle.log, a: exp(players.p1.team), b: exp(players.p2.team),
			names: [players.p1.name, players.p2.name]});
		calls = null;
	}
	fs.writeFileSync(outFile, JSON.stringify(out));
	console.log(`${out.length} recorded battles replayed -> ${outFile}`);
} else if (mode === 'stats') {
	const count = Number(rest[1]);
	const tally = {};
	let total = 0;
	const t0 = Date.now();
	for (let n = 0; n < count; n++) {
		const res = runBattle(pairs[n % pairs.length], n, false);
		for (const c of res.calls) {
			const sortCtx = c.st[0] === 'speedSort' || c.st[1] === 'speedSort' ? 'SORT ' + c.top : '';
			const key = (sortCtx || c.st.slice(0, 3).join('<')) + ' | ev=' + c.ev + ' eff=' + c.eff + ' range=' +
				(c.to === null ? c.from : `${c.from}-${c.to}`).toString().replace(/\d+-\d+/, 'a-b');
			tally[key] = (tally[key] || 0) + 1;
			total++;
		}
	}
	const rows = Object.entries(tally).sort((a, b) => b[1] - a[1]);
	console.log(`${count} battles, ${total} random calls (${(total / count).toFixed(1)} per battle), ${((Date.now() - t0) / 1000).toFixed(1)} s`);
	for (const [k, v] of rows) console.log(String(v).padStart(8), k);
} else if (mode === 'record') {
	const [, first, count, outFile] = rest;
	const out = [];
	for (let n = Number(first); n < Number(first) + Number(count); n++) {
		const p = pairs[n % pairs.length];
		const res = runBattle(p, n, true);
		res.a = p.a; res.b = p.b;
		out.push(res);
	}
	fs.writeFileSync(outFile, JSON.stringify(out));
	console.log(`${out.length} battles recorded -> ${outFile}`);
}
