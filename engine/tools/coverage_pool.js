// (2026-10-16) Fuzz-coverage team pool: validated teams in which EVERY legal species, every ability of every species, every
// legal move (that some legal species can learn) and every legal item appears at least once, so engine_diff / multi_turn_diff
// can exercise code paths no tournament team ever uses.  Showdown's own TeamValidator decides what is legal for the format.
//
//   node tools/coverage_pool.js <showdown folder> <format id> <out folder> [seed] [--copies N]
//
// Writes <out>/cov_0000.txt ... (6 Pokemon each, species + item clauses respected) and <out>/coverage_report.json.
// --copies N: N independent passes (different random sets / spreads / natures; the report counts the union).
const fs = require('fs');
const path = require('path');

const args = process.argv.slice(2);
const copiesIdx = args.indexOf('--copies');
let copies = 1;
if (copiesIdx >= 0) { copies = parseInt(args[copiesIdx + 1], 10); args.splice(copiesIdx, 2); }
const [serverDir, formatId, outDir, seedArg] = args;
if (!serverDir || !formatId || !outDir) {
	console.error('usage: node coverage_pool.js <showdown folder> <format id> <out folder> [seed] [--copies N]');
	process.exit(2);
}
const sim = require(path.resolve(serverDir, 'dist', 'sim'));
const {Dex, TeamValidator, Teams} = sim;
const validator = TeamValidator.get(formatId);
const dex = Dex.forFormat(formatId);

let seed = (parseInt(seedArg || '1', 10) >>> 0) || 1;
function rnd() { // mulberry32
	seed = (seed + 0x6D2B79F5) >>> 0;
	let t = seed;
	t = Math.imul(t ^ (t >>> 15), t | 1);
	t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
	return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
}
const pick = arr => arr[Math.floor(rnd() * arr.length)];
function shuffle(a) { for (let i = a.length - 1; i > 0; i--) { const j = Math.floor(rnd() * (i + 1)); [a[i], a[j]] = [a[j], a[i]]; } return a; }

const NATURES = ['Hardy', 'Lonely', 'Brave', 'Adamant', 'Naughty', 'Bold', 'Docile', 'Relaxed', 'Impish', 'Lax', 'Timid', 'Hasty',
	'Serious', 'Jolly', 'Naive', 'Modest', 'Mild', 'Quiet', 'Bashful', 'Rash', 'Calm', 'Gentle', 'Sassy', 'Careful', 'Quirky'];
const STATS = ['hp', 'atk', 'def', 'spa', 'spd', 'spe'];
function randomEvs() {
	const evs = {hp: 0, atk: 0, def: 0, spa: 0, spd: 0, spe: 0};
	let left = 66;
	const order = shuffle(STATS.slice());
	for (const s of order.slice(0, 2 + Math.floor(rnd() * 3))) {
		const v = Math.min(32, Math.floor(rnd() * 33), left);
		evs[s] = v; left -= v;
	}
	for (let guard = 0; left > 0 && guard < 50; guard++) {
		const s = pick(STATS);
		const v = Math.min(32 - evs[s], left);
		evs[s] += v; left -= v;
	}
	return evs;
}

function mkset(species, ability, item, moves) {
	return {name: '', species: species.name, item: item || '', ability, moves: moves.slice(), nature: pick(NATURES),
		evs: randomEvs(), level: 50, gender: ''};
}
const clone = s => JSON.parse(JSON.stringify(s));
function problems(set) { return validator.validateSet(clone(set), {}) || []; }

// ---------- what exists ----------
const allSpecies = dex.species.all().filter(s => !s.isNonstandard && !s.battleOnly && s.exists && s.num > 0 &&
	!(s.name.includes('-Gmax') || s.name.includes('-Mega') || s.name.includes('-Primal')));
const allMoves = dex.moves.all().filter(m => !m.isNonstandard && !m.isMax && !m.isZ && m.exists && m.id !== 'struggle' &&
	m.id !== 'recharge' && !/^hiddenpower/.test(m.id) && m.num > 0);
const allItems = dex.items.all().filter(i => !i.isNonstandard && i.exists && !i.isPokeball && i.num > 0 || (i.megaStone && !i.isNonstandard));

// legal team species = ones for which SOME simple set validates
const legalSpecies = [];
const learn = new Map();         // species id -> [move ids]
let t0 = Date.now();
for (const sp of allSpecies) {
	const abil = Object.values(sp.abilities)[0];
	const ok = [];
	for (const m of allMoves) {
		const pr = problems({name: '', species: sp.name, item: '', ability: abil, moves: [m.id], nature: 'Hardy', evs: {hp: 0, atk: 0, def: 0, spa: 0, spd: 0, spe: 0}, level: 50});
		if (!pr.length) ok.push(m.id);
	}
	if (ok.length) { legalSpecies.push(sp); learn.set(sp.id, ok); }
}
console.error(`legal species ${legalSpecies.length}/${allSpecies.length} (${((Date.now() - t0) / 1000).toFixed(1)} s)`);

const learnable = new Set();
for (const v of learn.values()) for (const m of v) learnable.add(m);
console.error(`moves learnable by a legal species: ${learnable.size}/${allMoves.length}`);

function itemsFor(sp) {          // items this species may hold (stones are species-specific)
	return allItems.filter(it => {
		if (it.megaStone) return Object.keys(it.megaStone).includes(sp.baseSpecies) || Object.keys(it.megaStone).includes(sp.name);
		if (it.itemUser) return it.itemUser.includes(sp.name);
		return true;
	});
}

// ---------- build sets ----------
const sets = [];
const coveredMoves = new Set(), coveredItems = new Set(), coveredPairs = new Set();
function register(set, sp) {
	for (const m of set.moves) coveredMoves.add(m);
	if (set.item) coveredItems.add(toID(set.item));
	coveredPairs.add(sp.id + '|' + toID(set.ability));
	sets.push({set, sp});
}
const toID = s => String(s).toLowerCase().replace(/[^a-z0-9]/g, '');

function chooseMoves(sp, preferUncovered) {
	const pool = learn.get(sp.id);
	const unc = shuffle(pool.filter(m => !coveredMoves.has(m)));
	const chosen = [];
	for (const m of unc) { if (chosen.length < 4 && preferUncovered) chosen.push(m); }
	const rest = shuffle(pool.filter(m => !chosen.includes(m)));
	for (const m of rest) { if (chosen.length < 4) chosen.push(m); }
	return chosen;
}
function chooseItem(sp) {
	const cand = itemsFor(sp);
	const unc = cand.filter(i => !coveredItems.has(i.id));
	const it = unc.length ? pick(unc) : pick(cand);
	return it.name;
}
function tryBuild(sp, ability, forceItem, forceMoves) {
	for (let attempt = 0; attempt < 12; attempt++) {
		const moves = forceMoves ? forceMoves.slice(0, 4) : chooseMoves(sp, attempt < 8);
		if (forceMoves && moves.length < 4) {
			for (const m of shuffle(learn.get(sp.id).slice())) { if (moves.length < 4 && !moves.includes(m)) moves.push(m); }
		}
		const item = forceItem !== undefined ? forceItem : (attempt < 8 ? chooseItem(sp) : '');
		const set = mkset(sp, ability, item, moves);
		const pr = problems(set);
		if (!pr.length) return set;
		if (attempt === 11) { // fall back to no item
			set.item = '';
			if (!problems(set).length) return set;
		}
	}
	return null;
}

for (let c = 0; c < copies; c++) {
	for (const sp of shuffle(legalSpecies.slice())) {
		for (const ab of Array.from(new Set(Object.values(sp.abilities)))) {
			if (c > 0 && coveredPairs.has(sp.id + '|' + toID(ab)) && rnd() < 0.5) continue;
			const set = tryBuild(sp, ab);
			if (set) register(set, sp);
		}
	}
}
// moves nobody has yet
for (const m of shuffle(Array.from(learnable).filter(m => !coveredMoves.has(m)))) {
	if (coveredMoves.has(m)) continue;
	const holders = legalSpecies.filter(sp => learn.get(sp.id).includes(m));
	if (!holders.length) continue;
	const sp = pick(holders);
	const set = tryBuild(sp, pick(Object.values(sp.abilities)), undefined, [m, ...chooseMoves(sp, true).filter(x => x !== m)]);
	if (set) register(set, sp);
}
// items nobody holds yet
for (const it of allItems) {
	if (coveredItems.has(it.id)) continue;
	const holders = legalSpecies.filter(sp => itemsFor(sp).includes(it));
	for (const sp of shuffle(holders.slice()).slice(0, 8)) {
		const set = tryBuild(sp, pick(Object.values(sp.abilities)), it.name);
		if (set && toID(set.item) === it.id) { register(set, sp); break; }
	}
}
console.error(`${sets.length} sets; moves ${coveredMoves.size}/${learnable.size}, items ${coveredItems.size}/${allItems.length}, species-ability pairs ${coveredPairs.size}`);

// ---------- teams: 6 sets, distinct species and items ----------
fs.mkdirSync(outDir, {recursive: true});
const queue = shuffle(sets.slice());
const teams = [];
let leftovers = [];
while (queue.length) {
	const team = [];
	const species = new Set(), items = new Set();
	const skipped = [];
	while (queue.length && team.length < 6) {
		const e = queue.shift();
		const sid = e.sp.baseSpecies, iid = toID(e.set.item);
		if (species.has(sid) || (iid && items.has(iid))) { skipped.push(e); continue; }
		species.add(sid); if (iid) items.add(iid); team.push(e);
	}
	queue.unshift(...skipped);
	if (team.length < 6) { leftovers = leftovers.concat(team); break; }
	teams.push(team);
}
// leftover sets: pad with random other sets from the pool
for (let k = 0; k < leftovers.length; k += 6) {
	const team = leftovers.slice(k, k + 6);
	const species = new Set(team.map(e => e.sp.baseSpecies)), items = new Set(team.map(e => toID(e.set.item)).filter(Boolean));
	for (const e of shuffle(sets.slice())) {
		if (team.length >= 6) break;
		const sid = e.sp.baseSpecies, iid = toID(e.set.item);
		if (species.has(sid) || (iid && items.has(iid))) continue;
		species.add(sid); if (iid) items.add(iid); team.push(e);
	}
	if (team.length === 6) teams.push(team);
}
let written = 0, rejected = 0;
const bad = [];
teams.forEach((team, k) => {
	const arr = team.map(e => { const c = clone(e.set); c.moves = c.moves.map(m => dex.moves.get(m).name); return c; });
	const pr = validator.validateTeam(clone(arr));
	if (pr && pr.length) { rejected++; bad.push(pr.slice(0, 3)); return; }
	fs.writeFileSync(path.join(outDir, `cov_${String(written).padStart(4, '0')}.txt`), Teams.export(arr).replace(/\n/g, '\r\n').replace(/\r\n$/, '\r\n'));
	written++;
});
// ---------- report (only the sets in written teams count) ----------
const report = {teams: written, rejected, legal_species: legalSpecies.length, moves_learnable: learnable.size,
	moves_covered: coveredMoves.size, items_total: allItems.length, items_covered: coveredItems.size,
	species_ability_pairs: coveredPairs.size,
	moves_missing: Array.from(learnable).filter(m => !coveredMoves.has(m)),
	items_missing: allItems.filter(i => !coveredItems.has(i.id)).map(i => i.id),
	species_missing: legalSpecies.filter(sp => !Array.from(coveredPairs).some(p => p.startsWith(sp.id + '|'))).map(s => s.id),
	sample_problems: bad.slice(0, 5)};
fs.writeFileSync(path.join(outDir, 'coverage_report.json'), JSON.stringify(report, null, 1));
console.log(JSON.stringify(report));
