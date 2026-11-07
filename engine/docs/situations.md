# Setting up your own situations

`situation.py` lets you write a battle position in a small text file, play it in the engine many times, read the logs,
and (optionally) play the same position in real Pokemon Showdown to see whether the two agree. It is meant for
debugging: "what does the engine do when X happens?", and "does it do the same as Showdown?".

```
python situation.py examples/situations/rain_spread.txt
python situation.py examples/situations/rain_spread.txt --seeds 1000 --logs 3
python situation.py examples/situations/rain_spread.txt --showdown ../pokemon-showdown
python situation.py my_situation.txt --state        # print the starting position's full state and stop
```

It prints the starting position, the log of one game (`--logs N` for more), and a summary over all games: HP left,
how often each Pokemon fainted, statuses, stat stages, items, weather / terrain at the end.

## The file

Four sections. `#` starts a comment.

```
[p1]
<Showdown export of side 1's Pokemon: 2 to 6 sets, blank line between them>

[p2]
<side 2, the same way>

[setup]
<optional: change the position before the first turn>

[turns]
<one line per turn>
```

### [p1] and [p2]

Paste the Pokemon each side **brought** (Showdown's export format, Level 50, Champions stat points as `EVs:`). The first
two are on the field (slot a, then slot b); the rest are on the bench in that order. No team preview and no switch-in
effects run: the leads are simply standing there (so Intimidate, Drizzle etc. don't fire at the start; use `[setup]` for
the weather, stat stages and so on that you want).

A line that isn't part of a Showdown set is an error (it usually means a missing `[setup]` or `[turns]` header).

### Naming a Pokemon

| Write | Means |
|---|---|
| `p1a`, `p1b`, `p2a`, `p2b` | the Pokemon in that slot at the start |
| `p1:3`, `p2 4` | team position (1 to 6) |
| `p1 Garchomp`, `p2 Sylvie` | species or nickname |

### [setup]

One setting per line, or several for the same Pokemon / side separated by commas.

**A Pokemon** (`p1a ...`, `p2 Garchomp ...`):

| Setting | Example | Notes |
|---|---|---|
| `hp` | `hp 50%`, `hp 123` | percent of max HP, or exact HP |
| `status` | `status par`, `status slp 3` | brn, par, psn, tox, slp, frz, none; for sleep an optional number of turns left (default 2) |
| `boost` | `boost atk +2 spe -1` | atk, def, spa, spd, spe, accuracy, evasion (-6 to +6) |
| `item` | `item Sitrus Berry`, `item none` | a different held item |
| `ability` | `ability Pressure` | the CURRENT ability (as after Skill Swap / Entrainment); the set's own comes back on switch-out |
| `fainted` | `fainted` | |
| `mega` | `mega` | already Mega Evolved (holds its stone); the side's Mega is used up |
| `moved` | `moved` | has already moved since switching in (no Fake Out / First Impression) |
| `protect` | `protect 1` | Protect used on the previous turn(s): the next one may fail |
| `pp` | `pp Protect 3` | PP used of that move |
| `choicelock` | `choicelock Earthquake` | locked by its Choice item |
| `types` | `types Water Ground` | current types |
| `vol` | `vol taunt 3` | any engine volatile by name (see `Battle.state()[...]["vols"]` and `set_vols` in `py/src/lib.rs`) |

**A side** (`p1 ...`, `p2 ...`): `tailwind 3`, `reflect 5`, `lightscreen 5`, `auroraveil 5`, `safeguard 5`,
`stealthrock`, `spikes 2`, `toxicspikes 1`, `stickyweb`, `megaused`. The number is the turns left (defaults: Tailwind
4, others 5).

**The field:** `weather rain 3` (rain, sun, sand, snow), `terrain grassy 5` (electric, grassy, psychic, misty),
`trickroom 3`, `gravity 5`, `magicroom 5`, `wonderroom 5`, `fairylock 2`, and `turn 4` (the turn number the position
starts at).

### [turns]

```
p1: Earthquake, Protect | p2: Moonblast p1a, switch Incineroar
```

Two actions per side, slot a then slot b:

| Action | Example |
|---|---|
| a move, by name or number (1-4) | `Earthquake`, `2` |
| a move with a target | `Moonblast p1a`, `Pollen Puff p2b` (your partner), or Showdown's numbers `1`, `2`, `-1`, `-2` |
| Mega Evolve and move | `Rock Slide mega` |
| switch | `switch Incineroar`, `switch 3` (team position) |
| nothing (an empty slot) | `pass` |
| a random legal action | `random` |

A single-target move needs a target only when there is a choice; spread and self moves need none. If an action isn't
legal, the error lists every legal action of that Pokemon.

**Replacements after a faint** happen automatically: the first healthy bench Pokemon in team order. To choose, put a
line right after that turn:

```
replace p1: Basculegion
```

**When the dice change the board** (a Pokemon faints in some games but not others), a later scripted turn may no longer
fit (the Pokemon you scripted isn't there). Those games stop at that point; the summary says how many and why. The first
turn is the same in every game, so a choice that doesn't fit there is reported as an error.

## Comparing with Showdown

`--showdown <path to a pokemon-showdown checkout>` plays the same situation in real Showdown (needs Node.js, and the
checkout must be built: `node build` in it). The position is built there the same way (`tools/showdown_truth.js`), both
engines play the same choices for the same number of games, and the end states are compared: HP, faints, statuses, stat
stages, who is on the field, weather, terrain. A difference larger than chance is listed as

```
    end-state feature        engine  Showdown
    s1m0.hp                   0.412     0.538     (side 2, team position 1: mean HP fraction)
```

Two things to know:

* Showdown gets the choices as fixed text. So `random` actions are drawn once (by one engine game) and used in every
  game of both engines, and `replace` lines aren't used (both bring in the first healthy bench Pokemon).
* When a scripted choice is illegal in some game, both engines play Showdown's default (the first move) there; the
  output says how often this happened.
* A few settings have no Showdown counterpart in the harness (`types`, `vol`, `choicelock`, sleep turns): they are listed
  and left out of the Showdown side, so a difference may come from them.

With 200 games (the default) a difference that shows up once can be chance; rerun with `--seeds 1000` before reporting it.
If it stays, please open an issue with the situation file: that is everything we need to reproduce it.

## From Python

```python
import situation

s = situation.load("examples/situations/rain_spread.txt")
b = s.battle()                    # a vgcsim.Battle in that position, before the first turn
print(b.legal_actions(0))         # anything from the Battle API works on it
s.play(b)                         # play the scripted turns
print("\n".join(s.log(seed=1)))   # one game's log, Pokemon named
print(situation.summary(s.run(seeds=500)))

# or the text directly
s = situation.Situation(open("my.txt").read())
```

Everything `situation.py` does goes through the plain `vgcsim.Battle` API (`Battle.from_brought`, `set_mon`,
`set_field`, `set_side`, `set_extra`, `set_ability`, `set_types`, `set_vols`, `refresh`, `legal_actions`, `choose`,
`state`, `log`), so you can also build positions in your own code the same way.
