# Changelog

## 0.2.0 (2026-11-06)

- Released as part of pkmn_stuff: adds the bindings the agent and review app use (network encodings, search leaves and
  set worlds, endgame solver, state hashing, belief / stat views).

## 0.1.2 (2026-11-05)

- `situation.py` + [docs/situations.md](docs/situations.md): set up a battle situation of your own in a small text file
  (both sides' Pokemon, HP / status / stat stages / items / abilities / Mega, weather, terrain, Trick Room, screens,
  hazards..., then scripted turns with moves, targets, switches and replacements), play it many times in the engine
  (named logs + an outcome summary), and with `--showdown` play the same situation in real Showdown and compare.
  Examples in `examples/situations/`.
- `tools/showdown_truth.js` mirrors more setup: exact HP, any held item, PP used, Protect streak, current ability, Aurora
  Veil, entry hazards, Safeguard, Gravity, Magic Room, Wonder Room, Fairy Lock, Mega already used.

## 0.1.1 (2026-11-05)

Fixes from a report by another bot maker (thanks!):

- No self stat change after a knockout that ends the battle (Clanging Scales, Scale Shot, Close Combat...). Recoil still happens.
- Red Card: when one spread move hits several holders, the fastest holder's card fires (Showdown runs AfterMoveSecondary
  over all targets speed-sorted), not the first one hit.
- `tools/scenarios.py`: new scenarios for these, plus Fake Out past turn one with one move / a Choice lock (the engine follows
  Champions, which turns it into Struggle; older Showdown pins let it fail); `ill_ok` for scripted choices the engine refuses
  but Showdown converts.
- `tools/bigseed_check.py`: `--require` (the ids a multi-turn case was made with).

## 0.1.0 (2026-11)

First public release.

- Fast engine for Champions VGC Reg M-C doubles (`vgcsim::battle`), damage formula exact vs Showdown.
- Exact reference engine (`vgcsim::sim`), a port of Showdown's simulator, used as an oracle.
- Python bindings (`vgcsim`): battles, legal actions, position setters, playouts, damage rolls, and the
  exact reference engine (`ExactDex`, `ExactBattle`).
- Coverage team pool (418 teams covering every legal species, ability, move and item).
- Verification tools against a local Showdown checkout.
