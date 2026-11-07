// Export Showdown's data for one format (with that format's mod applied) to JSON for vgcsim.
// Usage: node export_dex.js <showdown folder> <format id> <out.json>
const path = require('path');
const fs = require('fs');
const [showdown, formatId, outFile] = process.argv.slice(2);
const {Dex} = require(path.resolve(path.join(showdown, 'dist', 'sim')));
const dex = Dex.forFormat(formatId);

// plain data only; functions are listed by name in "hooks" so we know what needs custom code
function plain(obj, depth = 0) {
	if (obj === null || typeof obj !== 'object') return obj;
	if (Array.isArray(obj)) return obj.map(x => plain(x, depth + 1));
	const out = {};
	for (const k of Object.keys(obj)) {
		const v = obj[k];
		if (typeof v === 'function') continue;
		if (depth === 0 && ['desc', 'shortDesc', 'gen', 'effectType', 'exists', 'fullname', 'kind',
			'isZ', 'zMove', 'maxMove', 'contestType', 'num', 'tier', 'doublesTier', 'natDexTier'].includes(k)) continue;
		out[k] = plain(v, depth + 1);
	}
	return out;
}
function hooks(obj) {
	const out = [];
	for (const k in obj) if (typeof obj[k] === 'function') out.push(k);
	for (const k of ['condition', 'secondary', 'self']) {
		if (obj[k] && typeof obj[k] === 'object') for (const h in obj[k]) if (typeof obj[k][h] === 'function') out.push(`${k}.${h}`);
	}
	return out.sort();
}

const species = {};
for (const s of dex.species.all()) {
	if (!s.exists || s.isNonstandard === 'CAP' || s.isNonstandard === 'Custom') continue;
	species[s.id] = {
		name: s.name, baseSpecies: s.baseSpecies, forme: s.forme, types: s.types, baseStats: s.baseStats,
		abilities: s.abilities, weightkg: s.weightkg, isMega: !!s.isMega, requiredItem: s.requiredItem || null,
		requiredItems: s.requiredItems || null, nonstandard: s.isNonstandard || null,
		gender: s.gender || '', num: s.num, forme: s.forme, battleOnly: s.battleOnly || null, otherFormes: s.otherFormes || null,
	};
}
const moves = {};
for (const m of dex.moves.all()) {
	if (!m.exists || m.isZ || m.isMax) continue;
	moves[m.id] = {...plain(m), hooks: hooks(m)};
}
const items = {};
for (const it of dex.items.all()) {
	if (!it.exists) continue;
	items[it.id] = {...plain(it), hooks: hooks(it)};
}
const abilities = {};
for (const a of dex.abilities.all()) {
	if (!a.exists) continue;
	abilities[a.id] = {...plain(a), hooks: hooks(a)};
}
const types = {};
for (const t of dex.types.all()) types[t.name] = t.damageTaken;
const natures = {};
for (const n of dex.natures.all()) natures[n.id] = {name: n.name, plus: n.plus || null, minus: n.minus || null};

fs.writeFileSync(outFile, JSON.stringify({format: formatId, species, moves, items, abilities, types, natures}));
console.log(`species ${Object.keys(species).length}, moves ${Object.keys(moves).length}, items ${Object.keys(items).length}, ` +
	`abilities ${Object.keys(abilities).length}, types ${Object.keys(types).length}`);
