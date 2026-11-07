# How it was built

A summary of the project, roughly in the order things were built, with the numbers that decided each step. Win rates
are head-to-head results on held-out teams unless stated otherwise; "+x" means points of win rate.

## 1. Format

Pokémon Champions VGC, Regulation M-C, as implemented by Pokémon Showdown (`gen9championsvgc2026regmc`): doubles,
level 50, six registered and four brought, one Mega Evolution per battle, no Terastallization. Stat points replace EVs
(66 in total, at most 32 per stat, perfect IVs, plus a nature). The `champions` mod changes many base powers, PP
(at most 20), paralysis (1/8 full-para), sleep (2-3 turns) and freeze (thaws within 3 turns).

## 2. Engine (`engine/`)

Training needs hundreds of millions of games, and play-time search needs millions of simulated turns per minute, so the
first large piece of work was a purpose-built engine in Rust.

- **Damage**: a port of Showdown's `getDamage` / `modifyDamage` with the same integer rounding and modifier order.
  Checked against Showdown on randomly generated real matchups (weather, terrain, boosts, screens, spread, Helping Hand,
  Megas): 100% exact on every case set.
- **Two engines.** An exact port of Showdown's event system and PRNG (`src/sim`) matched Showdown on every one of
  8,400 tested battles, but costs ~127 µs per simulated turn. The fast engine (`src/battle.rs`) is hand-written for this
  format's content and runs at ~5 µs per turn (25-30k random battles per second per thread); the exact port is kept as
  an oracle only. Speed was chosen over the last bit of exactness throughout.
- **Fuzzing against Showdown itself.** Positions (real teams, a coverage pool with every legal species / ability /
  move / item, and targeted pools full of awkward mechanics: Illusion, Imposter, Trace, Mold Breaker, Unburden...)
  are played for one to five turns in both the fast engine and real Showdown (`tools/showdown_truth.js`) over many
  seeds; outcome distributions are compared. Several hundred mechanics fixes came out of this; on fresh position
  ranges the flagged share fell to ~0.01-0.05%. Coverage-steered runs pick positions to exercise what hasn't fired yet.
- **Hidden information inside the engine.** Hidden counters (sleep, confusion, binding, lock moves) are stored as
  public counts and drawn from their posterior at each check; weather / screen lengths set by an opponent whose item is
  unknown are drawn from the prior in search worlds; the encoder shows only what a player could know.

## 3. From a battle to the engine (`python/bridge.py`, `log_vols.py`, `bridge_vols.py`)

A player sees a protocol log, not a state. The bridge rebuilds an engine position from a poke-env battle: our side
exactly (stat points and nature solved from the request's stats), the opponent from what was revealed, with unrevealed
moves / items / abilities filled from a library of real sets consistent with what was seen (`setlib.py`) and stat
points from **spread inference** (`spread_infer.py`: exact likelihoods over stat points x nature from observed damage,
HP bars as displayed, and move order; the "proven" set is kept apart from the guess). Volatiles, timers, history, hazards,
forms, PP, choice locks, Illusion and many more are tracked from the log. This was checked by attaching Showdown's true
state to every decision of test games and diffing the rebuilt position field by field, and by playing one random joint
action from both and comparing outcomes; the remaining differences are hidden information.

## 4. Networks (`python/netlib.py`)

Inputs per position (from one side's view): per-Pokemon features (HP, status, boosts, stats or guessed stats, types,
volatiles, history of moves used), field, and computed blocks: damage of every active move against every target
(expected fraction, KO chance, accuracy), speed order, bench switch-in damage, Mega-form damage, an
uncertainty block over plausible opponent sets, and, for the newest nets, the not-brought roster, belief inputs and a
memory of earlier turns. Ids (species, ability, item, moves) go through embeddings.

- **v1 ("B")**: transformer, 17 tokens (8 Pokemon + 8 active moves + field), width 256, 8 layers, ~7M parameters.
- **v2**: 448 wide, 8 layers, ~24M parameters with all heads; joint (pair) action head, opponent-action head,
  belief head over unrevealed sets, future-HP / faint heads, team-preview head with a pairwise term.
- **v3**: 13 tokens (our six, their six, field), width 256, 16 layers, a one-layer decoder that produces the active
  moves' vectors, attention memory over the last 16 turns, an opponent-conditioned policy, an HL-Gauss value head and
  ~30 auxiliary targets (final fate per Pokemon, final margin, pairwise future damage, move order, 1v1 matchup results
  from an endgame solver, extended beliefs...) sharing a fixed 30% of the loss.

Actions per active slot: move x target x Mega, switches, Struggle, pass (38 indices); the policy is per slot plus a
learned interaction term over the pair.

## 5. Training

1. **Imitation** of a hand-written heuristic bot (one-turn search over a damage model) on ~1M engine games, then
   **PPO self-play** against the current net, a pool of past nets and a hall of fame, with a KL anchor to the imitation
   net and to a moving average of the net's own weights. Reward: win / loss only.
2. **Distillation** of each generation into the next architecture, then RL again (v1 -> v2 -> v3).
3. **Teams**: drawn from the real pool, with generated teams (mutants, species-uniform draws so every legal species and
   Mega is played often) early and a shift toward strong teams late; validation teams never trained on.
4. **Learning rate**: plateaus repeatedly turned out to be the learning rate; automatic plateau checks (the net vs itself
   from two hours earlier) cut it when the win rate stopped rising. v2 used coupled damping of learning rate and
   regularisation.
5. **Engine-side choices that mattered**: open-team-sheet games must not show which four were brought (an early leak);
   a team-order shuffle; stat-estimate noise matching what inference achieves live.

Measured along the way (head to head, held-out teams):

| Step | Result |
|---|---|
| Imitation copy vs the heuristic bot | 37-39% |
| First RL run, best checkpoint vs the heuristic bot | ~50% without search |
| v1 final vs the heuristic bot (no search) | ~80% |
| Lowering the learning rate at a plateau | +5 to +9 within hours, twice |
| Search on top of the network (true state, engine) | +4.7 open sheets, +6.3 closed |

## 6. Search at play time (`python/sh_search.py`, `ndeep.py`, ...)

Each turn is a simultaneous-move game. The default search:

1. our policy's top joint actions x the opponent-prediction head's top joint replies (up to 40), each pair played out
   in the engine in several **set worlds** (sampled opponent builds weighted by the belief head), value-net leaves;
2. **sequential halving** over worlds: weak rows and columns are dropped as evidence accumulates; replies dropped
   early are re-checked against the final mix, and a double-oracle round adds the opponent's best reply among all legal
   joint actions;
3. the game is solved (regret matching; optionally Bayesian over set worlds) and the move is drawn from our equilibrium.

On top: best-first deepening of the most uncertain lines (`ndeep.py`, reach x entropy, after nessiedev's description
of his bot's search), exact chance outcomes, and an endgame solver for <= 2 Pokemon a side (`endgame.py`,
`eg_rescore.py` + engine bindings).

| Search comparison (same net both sides, closed sheets, paired seats) | Result |
|---|---|
| Halving search vs no search | +4 |
| + 4 s of deepening vs the halving search alone | +2.7 (P(better) 94%) |
| 8 s vs 2 s of deepening | no difference (49.1%) |
| Multi-turn MCTS vs the halving search | 44.6% |
| Exact 1v1 leaves | +0.6 |
| Opponent set worlds = the TRUE sets vs inferred (ceiling for better beliefs) | +5.8 |

## 7. Things that did not work

- Bigger search budgets beyond ~2 s; wider roots; MCTS; deeper rollouts at search leaves.
- Training the value net on search values ("search distillation"); a smaller student evaluated with more search.
- Fine-tuning parameters of the heuristic bot: gains on a few teams never transferred to a large team pool
  (the effective sample size is the number of distinct teams, not games).
- Single-neuron / attention-head edits, weight averaging, extrapolation along the training direction.

## 8. Game review (`review/`)

From a replay or battle log the review rebuilds every decision from the player's side, runs the search with the played action
forced into the candidates, converts values to win probabilities with a calibration fitted on self-play results, and
labels each decision by how much win chance it cost against the recommended option. Team preview gets its own analysis
(the pick vs the best plans, the opponent's likely leads).
