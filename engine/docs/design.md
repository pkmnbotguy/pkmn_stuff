# Design

## Two engines

| | Fast engine (`src/battle.rs`, `vgcsim::battle`) | Exact engine (`src/sim/`, `vgcsim::sim`) |
|---|---|---|
| Purpose | Everything that needs speed: simulation, playouts, bots | Reference / oracle |
| How it is written | Direct Rust: enums for abilities / items / moves, hand-written effect code, plain-old-data state | A port of Showdown's simulator: event system, handler priorities, action queue, Showdown's PRNG |
| Speed | ~2 µs per simulated turn | ~23x slower per turn |
| Checked against | Real Showdown (the fuzzers in `tools/`) | Showdown, bit-exact on 8,400 battles at the time of the port (commit `9fb3a5b99`) |
| Kept up to date | Yes: every fix found by fuzzing went here | No: only tournament-team content was ported, and later Showdown changes were not |

Use the fast engine. The exact engine is useful when you need Showdown's exact random stream, or as a second
opinion for tournament-team content. For anything else, the real Showdown (through `tools/showdown_truth.js`) is
the reference.

## Battle state

- `state.rs` holds `Mon`, `Side` and `Field`, plus the team parser (Showdown export format; `EVs:` = stat points).
- `Mon` and `Side` are plain old data and `Copy`, so a battle copy is a bitwise copy (~4.7 KB, ~100-150 ns) and
  positions can be copied freely. A compile-time check enforces this: never put a `Vec`, `String` or `Box` inside
  them. Short lists use fixed-size arrays (`ArrayVec`).
- Effect lengths that players can't see (sleep, confusion, binding, Outrage lock) are stored as public counts
  (turns elapsed). Each check draws from the posterior given that count, so copies of a position don't carry
  hidden dice.

## Turn model

Requests work like Showdown's:

1. `Battle::new`;
2. `choose_team` for both sides;
3. then a loop of `needs_choice(side)` / `choose(side, [Action; 2])`.

The action queue lives in the battle state. Mid-turn switches (U-turn, Eject Button, Emergency Exit...) pause the
turn, and it resumes after the switch choice.

Order of a turn: switches, then Mega Evolutions, then moves by priority and speed (Trick Room reverses). Speed ties
are random. Speeds are recomputed after every action, as in Gen 8+.

## Data

`data/dex.json` and `data/effects.json` are exported from Showdown's dex for the format (`Dex.forFormat`), so the
Champions mod applies. Examples: changed base powers, PP, paralysis 1/8, sleep 2-3 turns, freeze at most 3 turns.
Names are resolved to ids and enums at load time, so nothing in the hot path compares strings.

## Chance mode

Besides normal sampling, the fast engine can enumerate the outcomes of a turn with their exact probabilities:
crits, accuracy, secondary effects, the damage-roll split at a KO, speed ties and replacements. It is a Rust-side
feature (`Battle::chance`, a `ChanceCtl` in `src/battle.rs`): set a script of outcomes, play the turn,
read back the branches taken and their probabilities. The Python bindings don't expose it.

## Performance notes

- Turn resolution allocates almost nothing; `fast_bench` prints heap allocations per turn (~1).
- Profile-guided optimisation: `tools/pgo.sh` writes a profile to `pgo/`. The build scripts use it when it is
  present (~10-20% faster).
