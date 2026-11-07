// (2026-10-18) Benchmark / regression check for the truth harness's Showdown (showdown_truth.js + sd_fast.js).
//   node tools/sd_bench.js <showdown folder> <format> <requests.jsonl>... [--limit N] [--out answers.jsonl] [--cmp answers.jsonl]
// Requests come from TRUTH_DUMP=<file> (engine_diff.truth_request). --out stores the answers, --cmp checks them against stored
// ones (log timestamps |t:| removed): any difference = the speed patch changed Showdown's behaviour. SD_FAST=0 = unpatched.
const fs = require('fs');
const args = process.argv.slice(4);
const opt = (name, dflt) => { const i = args.indexOf(name); if (i < 0) return dflt; const v = args[i + 1]; args.splice(i, 2); return v; };
const limit = +opt('--limit', 1e9), out = opt('--out', null), cmp = opt('--cmp', null);
const truth = require('./showdown_truth.js');
let reqs = [];
for (const f of args) for (const l of fs.readFileSync(f, 'utf8').split('\n')) {
	if (!l.trim()) continue;
	try { reqs.push(JSON.parse(l)); } catch (e) {}      // parallel dumps can interleave lines: those are skipped
}
reqs = reqs.slice(0, limit);
const norm = r => JSON.stringify({rows: r.rows, illegal: r.illegal, error: r.error, illegal_script: r.illegal_script,
	logs: (r.logs || []).map(lg => lg.filter(x => !x.startsWith('|t:|')))});
const ref = cmp ? fs.readFileSync(cmp, 'utf8').split('\n').filter(Boolean) : null;
const answers = [];
let bad = 0, seeds = 0;
const t0 = process.hrtime.bigint();
for (let i = 0; i < reqs.length; i++) {
	const a = norm(truth.handle(reqs[i]));
	seeds += reqs[i].seeds || 0;
	if (out) answers.push(a);
	if (ref && ref[i] !== a) { if (bad < 5) console.log(`DIFFERENT answer: request ${i}`); bad++; }
}
const s = Number(process.hrtime.bigint() - t0) / 1e9;
if (out) fs.writeFileSync(out, answers.join('\n') + '\n');
console.log(`${reqs.length} requests, ${seeds} seeds: ${s.toFixed(2)} s (${(s / seeds * 1000).toFixed(2)} ms per seed)` +
	(ref ? `; ${bad ? bad + ' DIFFERENT' : 'all identical'} vs ${cmp}` : ''));
