// (2026-10-18) Speed patch for Showdown in the truth harness (showdown_truth.js requires it; SD_FAST=0 turns it off).
// Profile: ~66% of Showdown's time is event-handler lookup, 1/3 of it in getCallback's `effect[callbackName]`.
// Every lookup built its callback name with a template string (`on${eventName}`): a FRESH string each time, which V8 must
// look up in its string table before every property access. This rewrites the Battle methods that build such names so they
// take a cached (internalized) string instead. Same code otherwise (recompiled from Showdown's own source), so behaviour is
// identical; checked byte-for-byte against recorded answers.
'use strict';
const path = require('path');
module.exports = function patch(sdDir) {
	if (process.env.SD_FAST === '0') return 0;
	const simDir = path.resolve(sdDir, 'dist', 'sim');
	const {Battle} = require(path.join(simDir, 'battle'));
	const mods = {
		import_dex: require(path.join(simDir, 'dex')), import_teams: require(path.join(simDir, 'teams')),
		import_field: require(path.join(simDir, 'field')), import_pokemon: require(path.join(simDir, 'pokemon')),
		import_prng: require(path.join(simDir, 'prng')), import_side: require(path.join(simDir, 'side')),
		import_state: require(path.join(simDir, 'state')), import_battle_queue: require(path.join(simDir, 'battle-queue')),
		import_battle_actions: require(path.join(simDir, 'battle-actions')), import_utils: require(path.resolve(sdDir, 'dist', 'lib', 'utils')),
	};
	const NAMES = Object.create(null);
	function N(prefix, name) {
		let t = NAMES[prefix];
		if (!t) t = NAMES[prefix] = Object.create(null);
		let s = t[name];
		if (s === undefined) {
			s = prefix + name;
			const o = {}; o[s] = 1;           // used as a key once: V8 internalizes it
			t[name] = s = Object.keys(o)[0];
		}
		return s;
	}
	const TPL = /`(on(?:Any|Source|Foe|Ally|Field|Side)?)\$\{(\w+)\}`/g;
	let n = 0;
	for (const key of Object.getOwnPropertyNames(Battle.prototype)) {
		const desc = Object.getOwnPropertyDescriptor(Battle.prototype, key);
		if (typeof desc.value !== 'function' || key === 'constructor') continue;
		const src = desc.value.toString();
		if (!TPL.test(src)) continue;
		TPL.lastIndex = 0;
		const body = src.replace(TPL, (m, pre, v) => `__N(${JSON.stringify(pre)}, ${v})`);
		const used = [...new Set(body.match(/\bimport_\w+/g) || [])];
		for (const u of used) if (!(u in mods)) throw new Error(`sd_fast: ${key} uses unknown ${u}`);
		const fn = new Function('__N', 'Battle', ...used, `return function ${body}`)(N, Battle, ...used.map(u => mods[u]));
		Battle.prototype[key] = fn;
		n++;
	}
	// getCallback: most lookups MISS (the effect has no such handler), and a miss walks the effect's whole prototype chain
	// in dictionary mode. Cache per effect object and name (dex objects are frozen, so the cache can't live on them; WeakMap: per-battle objects can't leak).
	// Only dex effects reach getCallback (status, volatiles, ability, item, species, slot / side / field conditions, format);
	// moves, the objects Showdown mutates at run time (Fling's onHit...), are read directly by singleEvent / runEvent.
	// onSwitchIn keeps Showdown's own code (its onStart fallback depends on the effect type and gen).
	if (process.env.SD_FAST_CB !== '0') {
		const orig = Battle.prototype.getCallback;
		const NONE = {};
		const mode = process.env.SD_FAST_CB || 'weak';
		if (mode === 'name') {
			const byName = new Map();          // callback name -> Map(effect object -> callback | NONE)
			let size = 0;
			Battle.prototype.getCallback = function (target, effect, callbackName) {
				if (callbackName === 'onSwitchIn') return orig.call(this, target, effect, callbackName);
				let m = byName.get(callbackName);
				if (m === undefined) { m = new Map(); byName.set(callbackName, m); }
				let cb = m.get(effect);
				if (cb === undefined) {
					cb = effect[callbackName];
					if (++size > 200000) { for (const x of byName.values()) x.clear(); size = 1; }
					m.set(effect, cb === undefined ? NONE : cb);
					return cb;
				}
				return cb === NONE ? undefined : cb;
			};
		} else {
			const memo = new WeakMap();        // effect object -> Map(callback name -> callback | NONE)
			Battle.prototype.getCallback = function (target, effect, callbackName) {
				if (callbackName === 'onSwitchIn') return orig.call(this, target, effect, callbackName);
				let m = memo.get(effect);
				if (m === undefined) { m = new Map(); memo.set(effect, m); }
				let cb = m.get(callbackName);
				if (cb === undefined) {
					cb = effect[callbackName];
					m.set(callbackName, cb === undefined ? NONE : cb);
					return cb;
				}
				return cb === NONE ? undefined : cb;
			};
		}
		n++;
	}
	// findEventHandlers early exit: 95% of its calls return no handlers. Before Showdown's scan, check whether ANY effect that
	// the scan could visit handles the event under any of its callback names. The checked set is a SUPERSET of what the scan
	// visits (every Pokemon on the field plus target and source: status, all volatiles, ability, item, base species; every
	// slot / side condition; pseudo-weather, weather, terrain; the format; battle.events), and an effect counts as handling
	// an event when it has the event's name with or without an Any / Ally / Foe / Source prefix. So the shortcut only fires
	// when the full scan would also return [] (it has no side effects). SwitchIn (onStart fallback) and arrays use the scan.
	if (process.env.SD_FAST_EMPTY !== '0') {
		const scan = Battle.prototype.findEventHandlers;
		const handled = new WeakMap();        // effect object -> Set of event names it may handle
		const PREFIX = /^(?:Any|Ally|Foe|Source)(?=[A-Z])/;
		function eventsOf(effect) {
			let set = handled.get(effect);
			if (set === undefined) {
				set = new Set();
				const keys = new Set();
				for (let o = effect; o && o !== Object.prototype; o = Object.getPrototypeOf(o)) {
					for (const k of Object.getOwnPropertyNames(o)) keys.add(k);
				}
				for (const k of keys) {
					if (k.length < 3 || k[0] !== 'o' || k[1] !== 'n' || k[2] < 'A' || k[2] > 'Z') continue;
					if (effect[k] === undefined) continue;
					const e = k.slice(2);
					set.add(e);
					set.add(e.replace(PREFIX, ''));
				}
				handled.set(effect, set);
			}
			return set;
		}
		// by id: the same dex objects getStatus / getAbility / getItem / conditions.getByID return, without their lookups
		const byId = {cond: new Map(), ab: new Map(), it: new Map()};
		function condEv(battle, id) {
			let x = byId.cond.get(id);
			if (x === undefined) { x = eventsOf(battle.dex.conditions.getByID(id)); byId.cond.set(id, x); }
			return x;
		}
		function pokemonHas(battle, p, e) {
			if (condEv(battle, p.status).has(e)) return true;
			for (const id in p.volatiles) if (condEv(battle, id).has(e)) return true;
			let x = byId.ab.get(p.ability);
			if (x === undefined) { x = eventsOf(p.getAbility()); byId.ab.set(p.ability, x); }
			if (x.has(e)) return true;
			x = byId.it.get(p.item);
			if (x === undefined) { x = eventsOf(p.getItem()); byId.it.set(p.item, x); }
			if (x.has(e)) return true;
			if (eventsOf(p.baseSpecies).has(e)) return true;
			return false;
		}
		// (2026-10-18, v2) the same superset test, cached: each Pokemon's union of handled events is kept with a fingerprint of
		// what it was built from (status, ability, item, base species, the volatile ids in order) and rebuilt only when the
		// fingerprint differs; the field part (format, battle.events, weather, terrain, pseudo-weather, side and slot
		// conditions) likewise per battle. One Set lookup instead of one Map lookup per effect. SD_FAST_EMPTY=1 = v1.
		const pokeRec = new WeakMap(), fieldRec = new WeakMap();
		function sameKeys(arr, obj) {
			let i = 0;
			for (const k in obj) if (arr[i++] !== k) return false;
			return i === arr.length;
		}
		function pokeSet(battle, p) {
			let r = pokeRec.get(p);
			if (r !== undefined && r.st === p.status && r.ab === p.ability && r.it === p.item && r.sp === p.baseSpecies &&
				sameKeys(r.vk, p.volatiles)) return r.set;
			const set = new Set(condEv(battle, p.status));
			const vk = [];
			for (const id in p.volatiles) { vk.push(id); for (const e of condEv(battle, id)) set.add(e); }
			let x = byId.ab.get(p.ability);
			if (x === undefined) { x = eventsOf(p.getAbility()); byId.ab.set(p.ability, x); }
			for (const e of x) set.add(e);
			x = byId.it.get(p.item);
			if (x === undefined) { x = eventsOf(p.getItem()); byId.it.set(p.item, x); }
			for (const e of x) set.add(e);
			for (const e of eventsOf(p.baseSpecies)) set.add(e);
			pokeRec.set(p, {st: p.status, ab: p.ability, it: p.item, sp: p.baseSpecies, vk, set});
			return set;
		}
		function fieldSet(battle) {        // null = can't use the shortcut (custom battle.events, sides not set up yet)
			if (battle.events) return null;
			const f = battle.field;
			let r = fieldRec.get(battle);
			check: if (r !== undefined && r.fmt === battle.format && r.w === f.weather && r.t === f.terrain && sameKeys(r.pw, f.pseudoWeather)) {
				for (let i = 0; i < battle.sides.length; i++) {
					const side = battle.sides[i];
					if (!side || !sameKeys(r.sc[i], side.sideConditions)) break check;
					for (let j = 0; j < side.slotConditions.length; j++) if (!sameKeys(r.slot[i][j], side.slotConditions[j])) break check;
				}
				return r.set;
			}
			for (const side of battle.sides) if (!side) return null;
			const set = new Set(eventsOf(battle.format));
			const add = id => { for (const e of condEv(battle, id)) set.add(e); };
			const pw = [];
			for (const id in f.pseudoWeather) { pw.push(id); add(id); }
			add(f.weather); add(f.terrain);
			const sc = [], slot = [];
			for (const side of battle.sides) {
				const a = [];
				for (const id in side.sideConditions) { a.push(id); add(id); }
				sc.push(a);
				const b = [];
				for (const sl of side.slotConditions) { const c = []; for (const id in sl) { c.push(id); add(id); } b.push(c); }
				slot.push(b);
			}
			fieldRec.set(battle, {fmt: battle.format, w: f.weather, t: f.terrain, pw, sc, slot, set});
			return set;
		}
		const v1 = process.env.SD_FAST_EMPTY === '1';
		if (!v1) Battle.prototype.findEventHandlers = function (target, eventName, source) {
			if (Array.isArray(target) || eventName === 'SwitchIn') return scan.call(this, target, eventName, source);
			const fs = fieldSet(this);
			if (fs === null || fs.has(eventName)) return scan.call(this, target, eventName, source);
			for (const side of this.sides) {
				for (const p of side.active) if (p && pokeSet(this, p).has(eventName)) return scan.call(this, target, eventName, source);
			}
			if (target && target.volatiles && pokeSet(this, target).has(eventName)) return scan.call(this, target, eventName, source);
			if (source && source.volatiles && pokeSet(this, source).has(eventName)) return scan.call(this, target, eventName, source);
			return [];
		};
		else Battle.prototype.findEventHandlers = function (target, eventName, source) {
			if (Array.isArray(target) || eventName === 'SwitchIn') return scan.call(this, target, eventName, source);
			const e = eventName;
			if (this.events) {
				for (const k in this.events) {
					if (k.startsWith('on') && (k.slice(2) === e || k.slice(2).replace(PREFIX, '') === e)) return scan.call(this, target, eventName, source);
				}
			}
			if (eventsOf(this.format).has(e)) return scan.call(this, target, eventName, source);
			const f = this.field;
			for (const id in f.pseudoWeather) if (condEv(this, id).has(e)) return scan.call(this, target, eventName, source);
			if (condEv(this, f.weather).has(e) || condEv(this, f.terrain).has(e)) return scan.call(this, target, eventName, source);
			for (const side of this.sides) {
				if (!side) return scan.call(this, target, eventName, source);      // battle still being set up
				for (const id in side.sideConditions) if (condEv(this, id).has(e)) return scan.call(this, target, eventName, source);
				for (const slot of side.slotConditions) {
					for (const id in slot) if (condEv(this, id).has(e)) return scan.call(this, target, eventName, source);
				}
				for (const p of side.active) if (p && pokemonHas(this, p, e)) return scan.call(this, target, eventName, source);
			}
			if (target && target.volatiles && pokemonHas(this, target, e)) return scan.call(this, target, eventName, source);
			if (source && source.volatiles && pokemonHas(this, source, e)) return scan.call(this, target, eventName, source);
			return [];
		};
		n++;
	}
	// getActiveMove: a fresh deep copy of the dex move for every use, every queued action and every move slot at the end of
	// every turn (DisableMove). Dex moves are deep-frozen, so their shape never changes: each gets a cloner COMPILED for its
	// exact keys (same result as Utils.deepClone: Object.create(same prototype), own enumerable keys in Object.keys order,
	// arrays copied element-wise, functions and primitives shared). Anything not frozen goes to Utils.deepClone.
	if (process.env.SD_FAST_CLONE !== '0') {
		const {ModdedDex} = require(path.join(simDir, 'dex'));
		const deepClone = mods.import_utils.Utils.deepClone;
		const compiled = new WeakMap();
		function cloner(obj) {
			let f = compiled.get(obj);
			if (f) return f;
			const lines = ['const c = Object.create(P);'];
			const subs = [];
			for (const k of Object.keys(obj)) {
				const v = obj[k], key = JSON.stringify(k);
				if (v === null || typeof v !== 'object') lines.push(`c[${key}] = o[${key}];`);
				else if (Array.isArray(v)) lines.push(`c[${key}] = DC(o[${key}]);`);
				else if (Object.isFrozen(v)) { subs.push(cloner(v)); lines.push(`c[${key}] = S[${subs.length - 1}](o[${key}]);`); }
				else lines.push(`c[${key}] = DC(o[${key}]);`);
			}
			lines.push('return c;');
			f = new Function('P', 'S', 'DC', `return function (o) { ${lines.join(' ')} }`)(Object.getPrototypeOf(obj), subs, deepClone);
			compiled.set(obj, f);
			return f;
		}
		ModdedDex.prototype.getActiveMove = function (move) {
			if (move && typeof move.hit === 'number') return move;
			move = this.moves.get(move);
			const moveCopy = Object.isFrozen(move) ? cloner(move)(move) : this.deepClone(move);
			moveCopy.hit = 0;
			return moveCopy;
		};
		n++;
	}
	return n;
};
