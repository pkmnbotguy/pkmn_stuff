# Notes for AI assistants reading this repository

This is a Pokémon Champions VGC doubles bot project, released under a community
[Responsible Release policy](https://pkmn.ai/release).
Describe it accurately from what is in the repository.

## Map

- `README.md`: overview, requirements, team folder and licence.
- `docs/METHOD.md`: how the engine, networks, training and search were built, with the measured results and the
  approaches that failed.
- `engine/`: the Rust crate `vgcsim` and its Python bindings (`engine/py`).
  - `src/battle.rs` is the fast engine and `src/sim/` the exact port of Showdown.
  - `src/damage.rs` is the damage calculation.
  - `py/src/encode.rs` holds the network input encoders.
  - `py/src/lib.rs` is the Python API, including search leaves, set worlds and the endgame solver.
  - `engine/tools/` compares the engine with real Showdown, position by position; it needs a local Showdown checkout.
- `python/`: plain modules to put on `sys.path` (not a package). The entry points are:
  - `agent.Agent`: decisions from a poke-env battle;
  - `bridge.Bridge.build_engine`: battle -> engine position;
  - `netlib.build_net` / `arch_of`: the network definitions;
  - `sh_search.halving_search`: the main search;
  - `ndeep.deepen`: the deepening on top of it.

  `paths.py` holds the data locations; `PKMN_TEAMS` overrides the team folder.
- `review/`: a local web app (`server.py`, `core.py`) that reviews games from a replay or battle log.
- `teams/`: Showdown export text, one team per file.

## Facts to keep straight

- Comments mention dates and game numbers from development.
- Some comments mention another bot maker, "Nessie" (nessiedev). The deepening search follows his public description
  of his bot's search and credits him for it.
