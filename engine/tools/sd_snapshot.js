// (2026-10-18) Snapshot of everything in a Showdown build that can change battle rules, for tools/sd_changes.py.
// Usage: node sd_snapshot.js <showdown folder | a copied dist folder> <format id> <out.json>
// Records, for the format's Dex (its mod applied): every move / ability / item / condition / species (data + the source text of
// every handler function, nested ones too), the type chart, the mod's scripts, the format and all its rules, and the source of
// every method of the simulator's classes (Battle, Pokemon, Side, Field, BattleQueue, BattleActions, PRNG, Dex...) plus the
// functions each dist/sim module exports.
const path = require('path');
const fs = require('fs');
const [root, formatId, outFile] = process.argv.slice(2);
const simDir = fs.existsSync(path.join(root, 'dist', 'sim')) ? path.join(root, 'dist', 'sim') : path.join(root, 'sim');
const {Dex} = require(path.resolve(simDir));
const dex = Dex.forFormat(formatId);

const SKIP = new Set(['desc', 'shortDesc', 'num', 'tier', 'doublesTier', 'natDexTier', 'contestType', 'exists', 'fullname',
	'effectType', 'kind', 'gen', 'zMove', 'maxMove', 'isZ', 'isMax', 'color', 'heightm', 'eggGroups', 'tags', 'cosmeticFormes',
	'formeOrder', 'evos', 'prevo', 'evoType', 'evoCondition', 'evoLevel', 'evoItem', 'evoMove', 'canHatch', 'spriteid']);

function ser(v, depth, seen) {
	if (typeof v === 'function') return {__fn: v.toString()};
	if (v === null || typeof v !== 'object') return v;
	if (seen.has(v) || depth > 6) return '[ref]';
	seen.add(v);
	let out;
	if (Array.isArray(v)) out = v.map(x => ser(x, depth + 1, seen));
	else if (v instanceof Set) out = [...v].sort();
	else {
		out = {};
		for (const k of Object.keys(v).sort()) {
			if (depth === 0 && SKIP.has(k)) continue;
			out[k] = ser(v[k], depth + 1, seen);
		}
	}
	seen.delete(v);
	return out;
}
const one = x => ser(x, 0, new Set());
const table = (all) => { const o = {}; for (const e of all) o[e.id] = one(e); return o; };

const snap = {format: formatId, sim: simDir};
snap.moves = table(dex.moves.all());
snap.abilities = table(dex.abilities.all());
snap.items = table(dex.items.all());
snap.conditions = {};
for (const id of Object.keys(dex.data.Conditions || {})) snap.conditions[id] = one(dex.conditions.get(id));
snap.species = {};
for (const s of dex.species.all()) {
	snap.species[s.id] = one({name: s.name, baseSpecies: s.baseSpecies, types: s.types, baseStats: s.baseStats, abilities: s.abilities,
		weightkg: s.weightkg, isMega: !!s.isMega, requiredItem: s.requiredItem || null, requiredItems: s.requiredItems || null,
		isNonstandard: s.isNonstandard || null, battleOnly: s.battleOnly || null, changesFrom: s.changesFrom || null});
}
snap.typechart = one(dex.data.TypeChart);
snap.scripts = one(dex.data.Scripts || {});
const fmt = dex.formats.get(formatId);
snap.format_obj = one(fmt);
snap.rules = {};
const todo = [...(fmt.ruleset || [])];
const done = new Set();
while (todo.length) {
	const r = String(todo.pop()).replace(/^[!+\-*]+/, '').split('=')[0].trim();
	const id = r.toLowerCase().replace(/[^a-z0-9]/g, '');
	if (!id || done.has(id)) continue;
	done.add(id);
	let f;
	try { f = dex.formats.get(r); } catch (e) { continue; }
	if (!f || !f.exists) continue;
	snap.rules[id] = one(f);
	for (const x of f.ruleset || []) todo.push(x);
}
try { snap.rule_table = [...dex.formats.getRuleTable(fmt).keys()].sort(); } catch (e) { snap.rule_table = String(e); }

// the simulator's code: every method / accessor of every exported class + exported plain functions, per dist/sim module
snap.code = {};
const seenClass = new Set();      // index.js re-exports the classes of the other modules: each class once
function ctorOf(src) {            // a class's constructor body (the class's own toString is the whole class)
	const i = src.search(/constructor\s*\(/);
	if (i < 0) return null;
	let j = src.indexOf('{', src.indexOf(')', i)), depth = 0;
	for (let k = j; k < src.length; k++) {
		if (src[k] === '{') depth++;
		else if (src[k] === '}' && --depth === 0) return src.slice(i, k + 1);
	}
	return null;
}
const simFiles = fs.readdirSync(simDir).filter(f => f.endsWith('.js')).sort();
simFiles.push(...simFiles.splice(simFiles.indexOf('index.js'), 1));   // re-exports last: classes keep their own module's name
for (const file of simFiles) {
	let mod;
	try { mod = require(path.resolve(simDir, file)); } catch (e) { continue; }
	for (const [name, val] of Object.entries(mod)) {
		if (typeof val !== 'function') continue;
		const key = `${file}:${name}`;
		if (!/^class[\s{]/.test(val.toString())) { snap.code[key] = val.toString(); continue; }
		if (seenClass.has(val)) continue;
		seenClass.add(val);
		for (const [holder, tag] of [[val.prototype, '#'], [val, '.']]) {
			for (const m of Object.getOwnPropertyNames(holder)) {
				if (['prototype', 'length', 'name'].includes(m)) continue;
				if (m === 'constructor' && tag === '#') {
					const c = ctorOf(val.toString());
					if (c) snap.code[`${key}${tag}constructor`] = c;
					continue;
				}
				const d = Object.getOwnPropertyDescriptor(holder, m);
				const parts = [];
				for (const k of ['value', 'get', 'set']) if (typeof d[k] === 'function') parts.push(d[k].toString());
				if (parts.length) snap.code[`${key}${tag}${m}`] = parts.join('\n');
			}
		}
	}
}
fs.writeFileSync(outFile, JSON.stringify(snap));
console.log(`snapshot: ${Object.keys(snap.moves).length} moves, ${Object.keys(snap.abilities).length} abilities, ` +
	`${Object.keys(snap.items).length} items, ${Object.keys(snap.conditions).length} conditions, ${Object.keys(snap.rules).length} rules, ` +
	`${Object.keys(snap.code).length} simulator functions -> ${outFile}`);
