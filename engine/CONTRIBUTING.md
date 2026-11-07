# Contributing

Bug reports with a reproducible position are the most useful contribution. The engine aims
to match Pokémon Showdown on outcome distributions, so a bug is a difference from Showdown.

## Reporting a difference from Showdown

1. If you can, find it with the tools: `python tools/engine_diff.py --truth --positions 2000 --first N`
   prints flagged positions; `--show K --truth` prints both engines' logs for case K.
2. Give the two teams, the position (turn, HP, field) and the choices, plus what Showdown does
   and what vgcsim does. A Showdown replay link or log is ideal.
3. Before calling it a bug, re-run the case with more seeds (`tools/bigseed_check.py`):
   with 60 seeds per position, one flagged position in a few thousand is expected by chance.

## Making a change

- Read [docs/adding-a-mechanic.md](docs/adding-a-mechanic.md).
- Keep the fast engine fast: run `fast_bench` before and after, and avoid heap allocations
  in turn resolution (the bench prints allocations per turn; ~1 is normal).
- `damage_check` must stay at 100% exact, and the cases you fixed plus a fresh fuzzing range
  should come back clean.
- Keep `Mon` and `Side` plain-old-data (`Copy`, no `Vec` / `String` / `Box` inside): battle
  copies are a bitwise copy and a compile-time check enforces it.

By contributing you agree that your contribution is released under the repository's license (MIT-0).
