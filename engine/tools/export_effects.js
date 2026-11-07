// Export everything the exact engine needs to know about effects (M4):
//  - data/effects.json: for every move, ability, item and condition of the format, its event handlers
//    (callback names, Order/Priority/SubOrder, constant values), duration, flags, effectType.
//    The Rust engine uses this to build the same handler lists Showdown builds (so speed-tie
//    shuffles consume the same random numbers) and to know which handlers it must implement.
//  - ref/effects_src.txt: the source code of every handler of the effects our teams can meet,
//    as Showdown runs it for this format (mods applied), for porting by hand.
//
//   node export_effects.js <showdown> <format> <pool.json> <out effects.json> <out src.txt>
const path = require('path');
const fs = require('fs');
const [showdown, formatId, poolFile, outJson, outSrc] = process.argv.slice(2);
const {Dex} = require(path.resolve(path.join(showdown, 'dist', 'sim')));
const dex = Dex.forFormat(formatId);
const pool = JSON.parse(fs.readFileSync(poolFile, 'utf8'));

const META = new Set(['Order', 'Priority', 'SubOrder']);
function handlerInfo(obj) {
	const out = {};
	for (const k in obj) {
		if (!/^on[A-Z]/.test(k) && k !== 'durationCallback' && k !== 'basePowerCallback' && k !== 'damageCallback' &&
			k !== 'beforeMoveCallback' && k !== 'beforeTurnCallback' && k !== 'priorityChargeCallback') continue;
		let isMeta = false;
		for (const m of META) if (k.endsWith(m) && typeof obj[k] === 'number') isMeta = true;
		if (k === 'onFractionalPriority') isMeta = false;
		if (isMeta) continue;
		const v = obj[k];
		if (v === undefined) continue;
		const h = {kind: typeof v === 'function' ? 'fn' : 'value'};
		if (h.kind === 'value') h.value = v;
		for (const m of META) if (typeof obj[k + m] === 'number') h[m.charAt(0).toLowerCase() + m.slice(1)] = obj[k + m];
		out[k] = h;
	}
	// Order/Priority/SubOrder without a handler of that name (e.g. onSwitchInPriority on an ability
	// whose onStart runs during SwitchIn): resolvePriority still reads them
	for (const k in obj) {
		if (!/^on[A-Z]/.test(k) || typeof obj[k] !== 'number' || k === 'onFractionalPriority') continue;
		for (const m of ['SubOrder', 'Order', 'Priority']) {
			if (!k.endsWith(m)) continue;
			const base = k.slice(0, -m.length);
			if (!out[base]) {
				if (!out['#' + base]) out['#' + base] = {kind: 'none'};
				out['#' + base][m.charAt(0).toLowerCase() + m.slice(1)] = obj[k];
			}
			break;
		}
	}
	return out;
}
function plainValue(v, depth = 0) {
	if (v === null || typeof v !== 'object') return typeof v === 'function' ? undefined : v;
	if (depth > 4) return undefined;
	if (Array.isArray(v)) return v.map(x => plainValue(x, depth + 1));
	const o = {};
	for (const k in v) {
		if (typeof v[k] === 'function') continue;
		const pv = plainValue(v[k], depth + 1);
		if (pv !== undefined) o[k] = pv;
	}
	return o;
}
function effectRecord(e, kind) {
	const rec = {
		id: e.id, name: e.name, fullname: e.fullname, effectType: e.effectType, num: e.num || 0, handlers: handlerInfo(e),
	};
	for (const k of ['duration', 'noCopy', 'affectsFainted', 'counterMax', 'suppressWeather', 'isBerry', 'isChoice',
		'isGem', 'ignoreKlutz', 'megaStone', 'naturalGift', 'onPlate', 'fling', 'boosts', 'isPrimalOrb', 'flags', 'rating', 'isPokeball', 'zMove', 'itemUser', 'forcedForme']) {
		if (e[k] !== undefined && typeof e[k] !== 'function') rec[k] = plainValue(e[k]);
	}
	if (kind === 'moves') {
		for (const k of ['secondary', 'self']) {
			if (e[k] && typeof e[k] === 'object') rec[k + 'Handlers'] = handlerInfo(e[k]);
		}
		if (e.secondaries) rec.secondariesHandlers = e.secondaries.map(s => handlerInfo(s));
	}
	return rec;
}

const out = {format: formatId, moves: {}, abilities: {}, items: {}, conditions: {}};
for (const m of dex.moves.all()) if (m.exists && !m.isZ && !m.isMax) out.moves[m.id] = effectRecord(m, 'moves');
for (const a of dex.abilities.all()) if (a.exists) out.abilities[a.id] = effectRecord(a, 'abilities');
for (const it of dex.items.all()) if (it.exists) out.items[it.id] = effectRecord(it, 'items');
// conditions, exactly as dex.conditions.getByID resolves them
const condIds = new Set(Object.keys(dex.data.Conditions));
for (const t of ['Moves', 'Abilities', 'Items']) {
	for (const id in dex.data[t]) if (dex.data[t][id].condition) condIds.add(id);
}
for (const id of [...condIds].sort()) {
	const c = dex.conditions.getByID(id);
	if (!c.exists) continue;
	out.conditions[id] = effectRecord(c, 'conditions');
}
fs.writeFileSync(outJson, JSON.stringify(out));
console.log(`moves ${Object.keys(out.moves).length}, abilities ${Object.keys(out.abilities).length}, ` +
	`items ${Object.keys(out.items).length}, conditions ${Object.keys(out.conditions).length}`);

// ---- source code of the pool's effects --------------------------------------------------------
function srcOf(obj, label) {
	let s = '';
	for (const k in obj) {
		const v = obj[k];
		if (typeof v === 'function') s += `  ${k}: ${v.toString()}\n`;
	}
	for (const k of ['secondary', 'self']) {
		if (obj[k] && typeof obj[k] === 'object') for (const h in obj[k]) {
			if (typeof obj[k][h] === 'function') s += `  ${k}.${h}: ${obj[k][h].toString()}\n`;
		}
	}
	return s;
}
const lines = [];
const seenCond = new Set();
function dumpCondition(id, why) {
	if (seenCond.has(id)) return;
	const c = dex.conditions.getByID(id);
	if (!c.exists) return;
	seenCond.add(id);
	lines.push(`### condition ${id} (${c.effectType}) [${why}]  data: ${JSON.stringify(plainValue({
		duration: c.duration, noCopy: c.noCopy, affectsFainted: c.affectsFainted, counterMax: c.counterMax,
	}))}`);
	lines.push(srcOf(c));
}
for (const [kind, table] of [['moves', dex.moves], ['abilities', dex.abilities], ['items', dex.items]]) {
	for (const id of Object.keys(pool[kind]).sort()) {
		const e = table.get(id);
		if (!e.exists) { lines.push(`### ${kind} ${id}: DOES NOT EXIST`); continue; }
		const data = plainValue(e);
		for (const k of ['desc', 'shortDesc', 'num', 'gen', 'exists', 'fullname', 'effectType', 'kind', 'contestType',
			'zMove', 'maxMove', 'isNonstandard', 'spritenum', 'name', 'id', 'rating', 'sourceEffect', 'noCopy',
			'affectsFainted', 'isZ', 'isMax', 'baseMoveType', 'zMoveBoost', 'condition']) delete data[k];
		lines.push(`### ${kind} ${id} (used ${pool[kind][id]}x)  data: ${JSON.stringify(data)}`);
		lines.push(srcOf(e));
		if (dex.data[kind[0].toUpperCase() + kind.slice(1)][id]?.condition) dumpCondition(id, `${kind} ${id}`);
	}
}
for (const id of Object.keys(dex.data.Conditions).sort()) dumpCondition(id, 'conditions table');
fs.writeFileSync(outSrc, lines.join('\n'));
console.log(`source of ${lines.length / 2 | 0} effects -> ${outSrc}`);
