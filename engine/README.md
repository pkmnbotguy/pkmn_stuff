# vgcsim (engine)

This is the engine of this repository: the same code as the public `just_a_simulator` release, plus the
bindings the agent and the review app need (network encodings in `py/src/encode.rs`, search leaves / worlds, the
endgame solver, state hashing, belief views). Build it with `build_python.sh` / `build_python.cmd` (maturin develop
into the active virtual environment).


A fast battle engine for **Pokémon Champions VGC doubles** (Showdown format
`gen9championsvgc2026regmc`, "[Champions] VGC 2026 Reg M-C"), written in Rust
with Python bindings. Its job is to play very many games very fast while
matching Pokémon Showdown closely enough that what a bot learns or computes
inside it still holds on Showdown.

- **Fast:** about 25,000-30,000 random-vs-random battles per second on ONE
  desktop CPU thread with tournament teams. It scales close to linearly with
  threads. A full copy of a battle takes ~100-150 ns, so copying a position and
  playing one turn costs a few µs.
- **Checked against Showdown, not just written from it:** the damage formula
  is exact. Turn outcomes are fuzzed against the real Showdown simulator on
  random positions (see [Verification](#verification)).
- **Champions rules:** stat points (66 total, 32 per stat) instead of EVs,
  level 50, bring 4 of 6, Mega Evolution (incl. the Champions / Z-A Megas),
  no Tera, and the Champions mod's changed moves, PP and status rules.
- **Two engines in one crate:**
  - `vgcsim::battle`, the **fast engine**, what you use;
  - `vgcsim::sim`, a slower **exact port** of Showdown's event system and PRNG, kept as a
    reference / oracle (see [docs/design.md](docs/design.md)).

## Contents

| Path | What it is |
|---|---|
| `src/` | The engine crate: `dex` (data loading), `damage`, `stats`, `state` (battle state, team parser), `battle` (fast engine), `policy` (legal actions, random policy, playouts), `sim/` (exact reference engine). |
| `src/bin/` | `fast_bench`, `damage_check`, `random_battles`, `turn_bench`, `sim_bench`, `replay_check`. |
| `py/` | Python bindings (PyO3 + maturin), module name `vgcsim`. |
| `data/` | `dex.json` / `effects.json` exported from Showdown for the format; `pairs.json` (2,000 team pairs for benchmarks); `damage_cases_sample.json` (3,000 damage cases computed by Showdown). |
| `pools/Teams_cov/` | 418 generated, validated teams that together field every legal species, species-ability pair, learnable move and item. See [pools/README.md](pools/README.md). |
| `tools/` | Verification against a local Showdown checkout (fuzzers, damage oracle, data exporters, audits). |
| `examples/` | A small Python example. |

## Quick start (Rust)

Needs a stable Rust toolchain (edition 2024, Rust 1.85+).

```bash
cargo build --release

# speed on your machine: 1 thread, then all threads
./target/release/fast_bench data/dex.json data/pairs.json 20000

# damage formula vs Showdown's own numbers
./target/release/damage_check data/dex.json data/damage_cases_sample.json
```

`damage_check` should end with `3000 cases: 2989 exact, 0 wrong (100.00% exact)`
(11 cases with Imposter Ditto are skipped).

## Quick start (Python)

Needs Python 3.10+ and a Rust toolchain. In a virtual environment:

```bash
pip install maturin
cd py && maturin develop --release
```

or run `build_python.sh` / `build_python.cmd`. Then:

```python
import vgcsim

dex = vgcsim.Dex("data/dex.json")
team_a = open("pools/Teams_cov/<some team>.txt").read()   # Showdown export format
team_b = open("pools/Teams_cov/<another>.txt").read()

b = vgcsim.Battle(dex, team_a, team_b, seed=1)
b.choose_team(0, [0, 1, 2, 3])        # bring 4 (first two lead)
b.choose_team(1, [0, 1, 2, 3])
print(b.random_playout(seed=7))       # winner: 0 / 1, 2 = tie, None = turn limit
print(b.winner, b.turn)               # properties
```

[examples/random_game.py](examples/random_game.py) plays a full game step by step
(`needs_choice`, `legal_actions`, `choose`, `log`). The bindings also expose:

- `copy()` and `state()`;
- position setters (`set_mon`, `set_field`, `set_side`, ...);
- `playouts(n, seed)`, which runs many playouts in Rust with the GIL released;
- `damage_rolls`;
- `ExactDex` / `ExactBattle`, the exact reference engine with the same calls.

Actions are tuples:
- `("move", slot, target, mega)`: target `1`/`2` = foe a/b, `-1`/`-2` = own side;
- `("switch", index, 0, False)`;
- `("pass", 0, 0, False)`.

## Setting up your own situations

To test a specific position, write it as a small text file (both sides' Pokemon, the setup, the turns to play) and run
it with `situation.py`:

```
[p1]
<Showdown export; the first two are on the field>
[p2]
...
[setup]
p1a hp 40%, status par, boost atk +2
p2 reflect 5, stealthrock
weather rain 3
[turns]
p1: Earthquake, Protect | p2: Moonblast p1a, switch Incineroar
```

```
python situation.py examples/situations/rain_spread.txt               # engine: logs + outcome summary
python situation.py my.txt --showdown ../pokemon-showdown              # also play it in real Showdown and compare
```

The format, every setting, and the Python API are in [docs/situations.md](docs/situations.md). A situation file is also
the easiest way to report a bug: it is everything needed to reproduce it.

## Verification

What "matches Showdown" means here, and how to check it yourself. Details are in
[docs/verification.md](docs/verification.md).

| Check | Result (at release) |
|---|---|
| Damage formula vs Showdown (`damage_check`, random real matchups incl. weather, terrain, boosts, screens, spread, Helping Hand, Megas, abilities and items) | 100% of 16-roll lists exact on 30,000 tournament-team cases and on the 3,000 coverage-pool cases shipped here |
| One turn from a random mid-game position, fast engine vs **real Showdown** (`tools/engine_diff.py --truth`, 60 seeds per position, outcome distributions compared) | 0 of 8,000 positions flagged on fresh tournament-team ranges; ~0.01-0.05% on the coverage pool |
| Several turns vs real Showdown (`tools/multi_turn_truth.py`, `tools/coverage_truth.py`, which steers positions towards effects that haven't fired yet) | flag rates fell from 8.7% to ~0.02-0.05% over the fuzzing rounds; every flagged case was fixed or shown to be chance |
| Hand-built scenarios for rare branches (`tools/scenarios.py`) | all agree with Showdown |

The checks need a local [Pokémon Showdown](https://github.com/smogon/pokemon-showdown)
checkout built with `node build` (set `SHOWDOWN_DIR`, default `../pokemon-showdown`).
The fast engine follows Showdown commit `56bc083aa` (2026-10-17). `tools/sd_check_upstream.sh`
lists what changed upstream since then, filtered to what this format can field.

**Not exact on purpose:** this is a near-exact engine built for speed. It uses its
own random number generator, so single games don't replay Showdown's games
seed-for-seed; agreement is checked on outcome distributions. The rare
differences left are listed in [docs/verification.md](docs/verification.md#known-gaps).

## Data and teams

`data/dex.json` and `data/effects.json` are exported from Showdown's dex for this
format (`tools/export_dex.js`, `tools/export_effects.js`), so the Champions mod's
changes are applied. To target another Showdown version, re-export them and
re-run the checks.

The engine reads teams in Showdown's export format. In that format, `EVs:` values
are read as Champions **stat points**.

## License

Code: [MIT No Attribution](LICENSE) (MIT-0). Data files and documentation: CC0.
Do what you like with it. As a non-binding request, please keep things built on it free.

Contains data derived from Pokémon Showdown (MIT); see [NOTICE](NOTICE).
Pokémon and related names are trademarks of Nintendo, Creatures Inc. and
GAME FREAK inc. This project is not affiliated with or endorsed by them or by
Pokémon Showdown.
