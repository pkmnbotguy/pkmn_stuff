// Ground truth for vgcsim's damage calculator: random real matchups computed by Showdown itself.
// For each case: two real teams, a random attacker and defender on the field (sometimes Mega
// Evolved), random conditions, one of the attacker's real moves, and all 16 damage rolls.
//
// Usage: node damage_oracle.js <showdown folder> <format id> <teams folder> <count> <out.json> [seed]
const path = require('path');
const fs = require('fs');
const [showdown, formatId, teamsDir, countArg, outFile, seedArg] = process.argv.slice(2);
const {Battle, Teams, toID} = require(path.resolve(path.join(showdown, 'dist', 'sim')));

let seed = Number(seedArg || 1) >>> 0 || 1;
function rnd() { // xorshift32
	seed ^= seed << 13; seed >>>= 0; seed ^= seed >>> 17; seed ^= seed << 5; seed >>>= 0;
	return seed / 4294967296;
}
const pick = arr => arr[Math.floor(rnd() * arr.length)];

const teams = fs.readdirSync(teamsDir).filter(f => f.endsWith('.txt'))
	.map(f => Teams.import(fs.readFileSync(path.join(teamsDir, f), 'utf8')));

function dump(p) {
	return {
		species: p.species.id, baseSpecies: toID(p.set.species), mega: !!p.species.isMega,
		set: {evs: p.set.evs, nature: toID(p.set.nature || 'serious'), level: p.level},
		stats: p.storedStats, maxhp: p.maxhp, hp: p.hp, types: p.getTypes(), ability: p.ability, item: p.item,
		boosts: {...p.boosts}, status: p.status || '', statsLowered: !!p.statsLoweredThisTurn,  // Lash Out (start-of-battle Intimidate)
	};
}

const out = [];
let attempts = 0;
let skipped = 0;
const count = Number(countArg);
while (out.length < count && attempts < count * 20) {
	attempts++;
	try {
	const setsA = [...pick(teams)], setsB = [...pick(teams)];
	const ia = Math.floor(rnd() * setsA.length), ib = Math.floor(rnd() * setsB.length);
	[setsA[0], setsA[ia]] = [setsA[ia], setsA[0]];
	[setsB[0], setsB[ib]] = [setsB[ib], setsB[0]];
	let battle;
	try {
		battle = new Battle({formatid: formatId, seed: [1, 2, 3, Math.floor(rnd() * 65535)]});
		battle.setPlayer('p1', {name: 'A', team: Teams.pack(setsA)});
		battle.setPlayer('p2', {name: 'B', team: Teams.pack(setsB)});
		battle.makeChoices('team 1234', 'team 1234');
	} catch (e) {
		continue;
	}
	const attacker = battle.p1.active[0], defender = battle.p2.active[0];
	if (!attacker || !defender) continue;
	if (attacker.canMegaEvo && rnd() < 0.7) battle.actions.runMegaEvo(attacker);
	if (defender.canMegaEvo && rnd() < 0.5) battle.actions.runMegaEvo(defender);

	// random conditions
	const weather = pick(['', '', 'raindance', 'sunnyday', 'sandstorm', 'snowscape']);
	if (weather && rnd() < 0.5) battle.field.setWeather(weather, attacker);
	const terrain = pick(['', '', 'electricterrain', 'grassyterrain', 'psychicterrain', 'mistyterrain']);
	if (terrain && rnd() < 0.5) battle.field.setTerrain(terrain, attacker);
	for (const s of ['atk', 'spa']) if (rnd() < 0.25) attacker.boosts[s] = pick([-2, -1, 1, 2, 6]);
	for (const s of ['def', 'spd']) if (rnd() < 0.25) defender.boosts[s] = pick([-2, -1, 1, 2]);
	if (rnd() < 0.1 && !attacker.status) attacker.status = 'brn';
	const screens = [];
	for (const sc of ['reflect', 'lightscreen', 'auroraveil']) {
		if (rnd() < 0.08) { battle.p2.addSideCondition(sc, defender); screens.push(sc); }
	}
	let helpingHand = false;
	if (rnd() < 0.1) { attacker.addVolatile('helpinghand'); helpingHand = true; }

	const damaging = attacker.moveSlots.map(s => battle.dex.moves.get(s.id))
		.filter(m => m.category !== 'Status');
	if (!damaging.length) continue;
	const dexMove = pick(damaging);
	const move = battle.dex.getActiveMove(dexMove.id);
	const spreadTarget = ['allAdjacentFoes', 'allAdjacent'].includes(move.target);
	if (spreadTarget && rnd() < 0.8) move.spreadHit = true;
	const crit = rnd() < 0.15;
	move.willCrit = crit;
	move.hit = 1;  // multi-hit moves: the first hit
	if (move.id === 'finalgambit') continue;  // its damage callback makes the user faint
	battle.activeMove = move;
	battle.activePokemon = attacker;
	battle.activeTarget = defender;
	battle.singleEvent('ModifyType', move, null, attacker, defender, move, move);
	battle.singleEvent('ModifyMove', move, null, attacker, defender, move, move);
	battle.runEvent('ModifyType', attacker, defender, move, move);
	battle.runEvent('ModifyMove', attacker, defender, move, move);

	// Battle-state context. The oracle's battle has an empty queue and no damage history, so moves whose power or damage
	// comes from what happened earlier are recorded with the value Showdown computed, for the checker to supply
	// (the engine computes these from its own battle state; the battle fuzzing checks that part against Showdown).
	if (['counter', 'mirrorcoat', 'metalburst', 'comeuppance'].includes(move.id)) continue;  // damage = damage taken this turn
	if (['spitup', 'futuresight', 'doomdesire'].includes(move.id)) continue;  // needs Stockpile / lands turns later
	if (attacker.getAbility().id === 'rivalry') continue;  // depends on both Pokemon's genders
	const stateBp = ['avalanche', 'electroball', 'expandingforce', 'flail', 'gyroball', 'payback', 'reversal', 'watershuriken'];
	const bpCallback = stateBp.includes(move.id) && move.basePowerCallback ?
		move.basePowerCallback.call(battle, attacker, defender, move) : null;
	// Fling: its power is set in PrepareHit (not run here) from the held item
	let flingBp = 0;
	if (move.id === 'fling') {
		const it = attacker.getItem();
		if (!it.fling || attacker.ignoringItem(true)) continue;  // the move fails in battle
		flingBp = move.basePower = it.fling.basePower;
	}
	const realChance = battle.randomChance;
	battle.randomChance = () => false;  // Fickle Beam's 30% double: off in every roll (the checker assumes off)

	const rolls = [];
	const realRandom = battle.random;
	// snapshot before any roll: a resist berry is eaten during the first damage calculation
	const attackerDump = dump(attacker), defenderDump = dump(defender);
	const defenderItem = defender.item, attackerItem = attacker.item;
	let immune = false;
	try {
		for (let r = 0; r < 16; r++) {
			battle.random = (m, n) => (m === 16 && n === undefined ? r : realRandom.call(battle, m, n));
			defender.item = defenderItem;
			attacker.item = attackerItem;
			const d = battle.actions.getDamage(attacker, defender, move, true);
			if (d === false || d === undefined || d === null) { immune = true; break; }
			rolls.push(d);
		}
	} catch (e) {
		continue;
	} finally {
		battle.random = realRandom;
		battle.randomChance = realChance;
	}
	out.push({
		attacker: attackerDump, defender: defenderDump,
		attackerAlly: battle.p1.active[1] && !battle.p1.active[1].fainted ? battle.p1.active[1].ability : '',
		defenderAlly: battle.p2.active[1] && !battle.p2.active[1].fainted ? battle.p2.active[1].ability : '',
		move: move.id, moveType: move.type, category: move.category, basePower: move.basePower,
		spread: !!move.spreadHit,
		weather: battle.field.effectiveWeather() || '', terrain: battle.field.terrain || '',
		screens, helpingHand, crit, immune, rolls: immune ? [] : rolls,
		bpCallback, flingBp, analytic: true,  // Analytic: nobody left to move in the empty queue
	});
	} catch (e) {
		skipped++;
		if (skipped <= 3) console.error('skipped case: ' + e.message);
	}
}
fs.writeFileSync(outFile, JSON.stringify(out));
console.log(`${out.length} cases written (${attempts} attempts, ${skipped} skipped by errors)`);
