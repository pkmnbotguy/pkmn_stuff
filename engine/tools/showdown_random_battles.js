// Random-vs-random battles in Showdown with the same policy as vgcsim's random_battles, to compare
// statistics. Also writes the shared list of team pairs.
//
//   node showdown_random_battles.js <showdown> <format> make-pairs <teams dir> <count> <seed> <pairs.json>
//   node showdown_random_battles.js <showdown> <format> run <pairs.json> <seed>
const path = require('path');
const fs = require('fs');
const [showdown, formatId, mode, ...rest] = process.argv.slice(2);
const {Battle, Teams} = require(path.resolve(path.join(showdown, 'dist', 'sim')));

function makeRng(seed) {
	let s = (seed >>> 0) || 1;
	return n => {
		s ^= s << 13; s >>>= 0; s ^= s >>> 17; s ^= s << 5; s >>>= 0;
		return Math.floor((s / 4294967296) * n);
	};
}

if (mode === 'make-pairs') {
	const [teamsDir, countArg, seedArg, outFile] = rest;
	const rnd = makeRng(Number(seedArg));
	const files = fs.readdirSync(teamsDir).filter(f => f.endsWith('.txt')).sort();
	const texts = files.map(f => fs.readFileSync(path.join(teamsDir, f), 'utf8'));
	const pairs = [];
	for (let i = 0; i < Number(countArg); i++) {
		pairs.push({a: texts[rnd(texts.length)], b: texts[rnd(texts.length)]});
	}
	fs.writeFileSync(outFile, JSON.stringify(pairs));
	console.log(`${pairs.length} pairs written`);
	process.exit(0);
}

const [pairsFile, seedArg, diagFile] = rest;
const diag = {damage: {}, heal: {}, cant: {}, uses: {}};
const bump = (tab, k, amt) => { const e = tab[k] || (tab[k] = [0, 0]); e[0]++; e[1] += amt; };
const toId = x => String(x || '').toLowerCase().replace(/[^a-z0-9]/g, '');
const pairs = JSON.parse(fs.readFileSync(pairsFile, 'utf8'));
const rnd = makeRng(Number(seedArg) ^ 0xABCDEF);

function choose(battle, side) {
	const req = side.activeRequest;
	if (!req || req.wait) return null;
	if (req.teamPreview) return 'team 1234';
	const foeSide = side.foe;
	const pokemon = req.side.pokemon;
	const nActive = 2;
	const taken = [];
	const benchFree = () => pokemon.map((p, i) => i).filter(i => i >= nActive && !pokemon[i].condition.endsWith(' fnt') && !taken.includes(i));
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

let turns = 0, p1Wins = 0, ties = 0, unfinished = 0;
const c = {moves: 0, crits: 0, misses: 0, protected: 0, switches: 0, faints: 0, cant: 0};
const start = Date.now();
pairs.forEach((p, n) => {
	let battle;
	try {
		battle = new Battle({formatid: formatId, seed: [n + 1, 2, 3, 4]});
		battle.setPlayer('p1', {name: 'A', team: Teams.pack(Teams.import(p.a))});
		battle.setPlayer('p2', {name: 'B', team: Teams.pack(Teams.import(p.b))});
		if (diagFile) {
			const spread = battle.spreadDamage.bind(battle);
			battle.spreadDamage = function (damage, targets, source, effect, instafaint) {
				const list = Array.isArray(targets) ? targets : [targets];
				const before = list.map(t => (t && t.hp !== undefined ? t.hp : null));
				const eff = effect || this.effect;
				const res = spread(damage, targets, source, effect, instafaint);
				list.forEach((t, k) => {
					if (t && before[k] !== null && t.hp < before[k]) bump(diag.damage, toId(eff && (eff.id || eff)), before[k] - t.hp);
				});
				return res;
			};
			const heal = battle.heal.bind(battle);
			battle.heal = function (amount, target, source, effect) {
				const tgt = target || this.event?.target;
				const before = tgt ? tgt.hp : null;
				const eff = effect || this.effect;
				const res = heal(amount, target, source, effect);
				if (tgt && before !== null && tgt.hp > before) bump(diag.heal, toId(eff && (eff.id || eff)), tgt.hp - before);
				return res;
			};
		}
	} catch (e) {
		unfinished++;
		return;
	}
	let guard = 0;
	while (!battle.ended && guard < 2000 && battle.turn <= 300) {
		guard++;
		let any = false;
		for (const side of battle.sides) {
			const ch = choose(battle, side);
			if (ch !== null && !side.isChoiceDone()) {
				any = true;
				if (!battle.choose(side.id, ch)) battle.choose(side.id, 'default');
			}
		}
		if (!any) break;
	}
	if (!battle.ended) { unfinished++; }
	else if (battle.winner === 'A') p1Wins++;
	else if (!battle.winner) ties++;
	turns += battle.turn;
	let started = false;
	for (const line of battle.log) {
		if (line.startsWith('|turn|')) started = true;
		if (line.startsWith('|move|')) c.moves++;
		else if (line.startsWith('|-crit|')) c.crits++;
		else if (line.startsWith('|-miss|')) c.misses++;
		else if (line.startsWith('|-activate|') && /move: (Protect|Wide Guard|Quick Guard)/.test(line)) c.protected++;
		else if ((line.startsWith('|switch|') || line.startsWith('|drag|')) && started) c.switches++;
		else if (line.startsWith('|faint|')) c.faints++;
		else if (line.startsWith('|cant|')) { c.cant++; const why = toId(line.split('|')[3]); diag.cant[why] = (diag.cant[why] || 0) + 1; }
		if (line.startsWith('|move|')) { const id = toId(line.split('|')[3]); diag.uses[id] = (diag.uses[id] || 0) + 1; }
	}
});
const n = pairs.length;
if (diagFile) fs.writeFileSync(diagFile, JSON.stringify(diag));
const secs = (Date.now() - start) / 1000;
const per = x => +(x / n).toFixed(3);
console.log(JSON.stringify({
	engine: 'showdown', battles: n, turns: per(turns), p1_win: +(p1Wins / n).toFixed(4), ties, unfinished,
	moves: per(c.moves), crits: per(c.crits), misses: per(c.misses), protected: per(c.protected),
	switches: per(c.switches), faints: per(c.faints), cant: per(c.cant), seconds: secs, battles_per_s: Math.round(n / secs),
}));
