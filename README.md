# pkmn_stuff

A research project on playing **Pokémon Champions VGC doubles** (Showdown format `gen9championsvgc2026regmc`,
"[Champions] VGC 2026 Reg M-C": level 50, bring 4 of 6, stat points instead of EVs, Mega Evolution, no Tera) with a
neural network and play-time search.

Certain aspects of this project have been redacted before release to preserve competitive integrity in accordance with the community's [Responsible Release policy](https://pkmn.ai/release).

## Contents

| Path | What it is |
|---|---|
| [docs/METHOD.md](docs/METHOD.md) | The write-up: engine, features, networks, training, search, opponent sets, results and negative results. |
| `engine/` | `vgcsim`, a Rust battle engine with Python bindings: a fast engine fuzzed against real Showdown, an exact port of Showdown's event system used as an oracle, the damage calculator, network encodings, search leaves, an endgame solver. Its own README / docs. |
| `python/` | The library: network definitions (`netlib.py`), the poke-env -> engine bridge (`bridge.py` + log trackers), opponent-set inference (`setlib.py`, `spread_infer.py`, `spread_track.py`), the searches (`sh_search.py`, `ndeep.py`, `deep3.py`, `mcts.py`, `do_search.py`, `endgame.py`, `eg_rescore.py`), and `agent.py`, which turns a poke-env battle object into an order. |
| `review/` | A local web app that reviews a game from its Showdown replay or battle log: win chance per decision, every option's value, labels, accuracy, a team-preview analysis. |
| `teams/` | Public tournament / community teams (Showdown export text, one team per file). See below. |

## Requirements

- Rust (stable) and Python 3.10+ (developed on 3.14), with `numpy`, `torch` and `poke-env` (0.16 era).
- Build the engine into your environment: `engine/build_python.sh` (or `build_python.cmd` on Windows; uses maturin).
- The agent and the review app need a network checkpoint, given as `AGENT_CKPT` / `REVIEW_CKPT`. The network classes
  in `python/netlib.py` define the formats.

## Using the pieces

```python
import sys; sys.path.insert(0, "python")
import vgcsim, paths
dex = vgcsim.Dex(paths.DEX)
b = vgcsim.Battle(dex, open("teams/Teams_real/<file>.txt").read(), open("teams/Teams_real/<file2>.txt").read(), 1)
```

- The engine on its own: see `engine/README.md` (random playouts, scripted situations, damage rolls, logs).
- The agent: `agent.Agent().choose_move(battle)` with a poke-env `DoubleBattle` you maintain yourself; it returns a
  poke-env order. Settings are environment variables (`AGENT_*`, listed at the top of `agent.py`).
- The review app: `python review/server.py`, open http://127.0.0.1:8765, paste a replay link (or log), say which player
  you were, paste your team. Only finished games can be reviewed.

## Teams

`teams/Teams_real` holds 9,775 public teams from the VGC Pastes repository (tabs M-A, M-B, M-C) and Limitless
tournament team lists, checked against Showdown's M-C rules. Where the source gave no spreads, they were
filled from the spreads that real sets of the same Pokemon use. `engine/pools/Teams_cov` is a synthetic coverage pool
(every legal species, ability, move and item), used for engine fuzzing.

## Licence

Code: MIT No Attribution. Our generated data (tables in `python/data`, `engine/data`): CC0.
The teams in `Teams_real` were published by their authors on public sites; they are included
as collected, for research. See [LICENSE](LICENSE).
