# Verification

All checks compare against Pokémon Showdown itself. You need:

- a Showdown checkout built with `node build` (set `SHOWDOWN_DIR`, default `../pokemon-showdown`);
- Node.js;
- the Python module built (`cd py && maturin develop --release`).

The engine follows Showdown commit `56bc083aa` (2026-10-17). `tools/sd_check_upstream.sh [ref]` lists what has
changed upstream since then, filtered to what this format can field.

## Damage formula

```bash
node tools/damage_oracle.js $SHOWDOWN_DIR gen9championsvgc2026regmc pools/Teams_cov 3000 data/damage_cases_sample.json 1
./target/release/damage_check data/dex.json data/damage_cases_sample.json
```

The oracle sets up random matchups in Showdown and records all 16 damage rolls. The matchups vary Megas, weather,
terrain, boosts, burn, screens, Helping Hand, spread hits and crits. Moves whose damage depends on battle history
(Counter, Metal Burst, Future Sight...) are skipped here; the battle fuzzing covers them.

Result at release: 100% exact.

## One turn vs real Showdown

```bash
python tools/engine_diff.py --truth --positions 8000 --first 100000 --workers 8
python tools/engine_diff.py --truth --show K      # both logs for flagged case K
```

How it works:

1. Each position is a realistic random mid-game state built from the team pool.
2. The fast engine and Showdown play the same joint choice from it, with 60 seeds each.
3. Their outcome distributions are compared: HP, faints, status, boosts, actives, weather, terrain.
4. A position is flagged when a difference is beyond noise (more than 4% HP or 8% probability, at 4 sigma).

The comparison is statistical, so with thousands of positions a few flags can be chance. Re-check a flagged case
with many more seeds before calling it a bug:

```bash
python tools/bigseed_check.py --k K --turns 1 --n 4000 --out case.json
```

Result at release: 0 of 8,000 positions flagged on fresh ranges of tournament teams; ~0.01-0.05% on the coverage
pool.

## Several turns, and steering

- `tools/multi_turn_truth.py`: several turns from each position. Later turns follow a fixed rule both engines can
  apply.
- `tools/coverage_truth.py`: rounds of positions chosen for what has not fired yet on the field (moves, abilities,
  items, Megas). It stops when everything has fired a few times.
- `tools/scenarios.py`: hand-built positions with scripted choices, for rare branches.

Positions come from `ED_TEAMS` (default `pools/Teams_cov`). Showdown's answers can be cached with
`TRUTH_CACHE=1` (sqlite under `fuzz/`). The cache is keyed by the request and a fingerprint of Showdown's files.

## Audits

- `tools/fast_audit.py`: moves, abilities and items in the pool that the fast engine never mentions.
- `tools/ref_audit.py`: the same for the exact engine's handlers.

## Known gaps

This is a near-exact engine built for speed.

- **Own random number generator.** Single games don't replay Showdown's games seed for seed; agreement is about
  outcome distributions.
- **Rare effects.** In the last fuzzing rounds, about 1 in 5,000-10,000 random positions was flagged over several
  turns, and every flagged case was fixed or shown to be chance. Expect rare differences in unusual interactions,
  especially chains of effects that change abilities, items or forms mid-turn.
- **Known unmodelled details:**
  - observers don't see a Zoroark disguise that faints as the imitated Pokémon fainting;
  - Revival Blessing on an active slot that just fainted re-enters at a slightly different time;
  - gender-based effects (Cute Charm, Rivalry) are approximate.
- **Data version.** Rules follow the exported data and Showdown's code at the commit above. If Showdown changes
  the format, re-export the data and re-run the checks.
- **The exact engine** (`src/sim/`) is behind the fast engine. It lacks handlers for content outside tournament
  teams and has none of the later fixes, so don't use it as a reference on the coverage pool.
