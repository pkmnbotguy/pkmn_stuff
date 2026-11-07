"""Where the repository keeps its data: the engine's dex / effects data, the team pools and the Python caches."""
import os

PY = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(PY)
ENGINE_DATA = os.path.join(REPO, "engine", "data")       # dex.json / effects.json (exported from Showdown)
DEX = os.path.join(ENGINE_DATA, "dex.json")
TEAMS = os.environ.get("PKMN_TEAMS", os.path.join(REPO, "teams"))   # one folder per pool (Teams_real)
CACHE = os.path.join(PY, "data")                          # set_library.json, spread_prior.json
