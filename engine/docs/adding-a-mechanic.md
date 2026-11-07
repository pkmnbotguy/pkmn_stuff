# Adding or fixing a mechanic

1. **Find out what Showdown does.** `python tools/sd_def.py move|ability|item|cond NAME` prints Showdown's
   definition for the format, including the Champions mod's changes. Read the handlers and their order (priority,
   `onAfterHit` vs `onDamagingHit`...): most bugs found by fuzzing were order bugs.
2. **Make a repro.** Two options:
   - a scenario in `tools/scenarios.py`: 4 sets per side, setup calls, a scripted joint choice each turn, and an
     `expect` regex that must appear in Showdown's log;
   - the flagged case from `engine_diff.py`.
3. **Implement it in `src/battle.rs`.**
   - Abilities, items and moves are enums (`Ab`, `It`, `Mv`); add a variant if needed.
   - New per-Pokémon state goes in `Mon` as plain data (it must stay `Copy`).
   - If the state should survive a switch, or be reset on one, handle it in `switch_out` / `switch_in`.
4. **Check:**
   - the scenario agrees with Showdown (`python tools/scenarios.py --only N --seeds 600`);
   - `damage_check` is still 100%;
   - a fresh `engine_diff.py --truth` range comes back clean;
   - `fast_bench` is not noticeably slower.
5. **Expose new state in the bindings.** If the Python bindings expose positions (`set_mon`, `state()`), name the
   new state there too, so positions rebuilt from a real game can carry it.
