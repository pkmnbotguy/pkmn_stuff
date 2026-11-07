"""Replays a player's recorded battle stream (rebuilt from a game's replay or log by from_replay.py) through
poke-env, and yields every decision point with the rebuilt engine position.

    for d in decisions(game): ...   # d: {"kind": "preview" | "turn" | "switch", "battle", "bot", "eb", "options", "choice", ...}

The engine position comes from the agent's encode (bridge.build_engine: our exact sets from Showdown's request, the
opponent from what was revealed + guesses), so it is the same bridge the bots use in live play.
"""
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, os.path.join(ROOT, "python"))

import logging  # noqa: E402
import agent  # noqa: E402
from poke_env.battle import DoubleBattle  # noqa: E402

def read_sheet(battle, m):
    """an open team sheet ('|showteam|pX|packed'), as a client reads it: the opponent's sets go into
    opponent_team (build_engine fills their unseen Pokemon from it) and battle._ots_sheet is set"""
    try:
        from poke_env.teambuilder import Teambuilder
        role = m[2]
        if role == battle.player_role:
            return
        sheet = Teambuilder.parse_packed_team("|".join(m[3:]))
        read = []
        for pm in battle.teampreview_opponent_team:
            hits = [t for t in sheet if t.nickname is not None and pm.identifies_as(t.nickname)]
            if not hits:
                continue
            try:
                mon = battle.get_pokemon(f"{role}: {hits[0].nickname}", details=pm._last_details)
                mon._update_from_teambuilder(hits[0])
                read.append(mon)
            except Exception:
                pass
        battle._ots_sheet = len(read) >= 4
    except Exception:
        pass


IGNORE = {"expire", "uhtmlchange", "tempnotify", "tempnotifyoff", "split", "uhtml", "html", "raw", "c", "j", "l",
          "chat", "inactive", "inactiveoff", "timer", "error", "bigerror", "seed", ""}


class ReviewBot(agent.Agent):
    """the agent's position encoding, keeping the engine battle it builds (encode() doesn't return it). The opponent's
    unrevealed moves / item / ability come from real tournament sets (setlib.py): the review's search simulates them."""
    LIBRARY_FILL = True

    def build_engine(self, battle):
        r = super().build_engine(battle)
        self.last_engine = r
        return r


def make_bot(name, fmt):
    bot = ReviewBot()
    bot.username, bot.battle_format = name, fmt
    bot.logger = logging.getLogger("review")
    bot.logger.setLevel(logging.ERROR)
    return bot


def to_id(s):
    return re.sub(r"[^a-z0-9]", "", str(s).lower())


def decisions(game):
    """Every decision of the reviewed player, in order, with the position as it was when they chose."""
    fmt, name = game.get("format", "gen9championsvgc2026regmc"), game["name"]
    # (2026-10-20) the room name has the real one (random-team ladders)
    m_ = re.match(r"battle-(gen9[a-z0-9]+)-\d+", game.get("room", "") or "")
    if m_ and m_.group(1).startswith("gen9championsvgc"):
        fmt = m_.group(1)
    bot = make_bot(name, fmt)
    # (2026-10-21) the room's battle number is kept
    tag = f"{game['room']}-review" if m_ else f"battle-{fmt}-review"
    battle = DoubleBattle(battle_tag=tag, username=name, logger=bot.logger, gen=9)
    open_sheets = any(isinstance(e, str) and e.startswith("|showteam|") for e in game["events"])
    pending = None                       # the decision waiting for its choice
    seg = []                             # public log lines since the last decision (what happened in between)
    for ev in game["events"]:
        if isinstance(ev, dict):
            if pending is not None and "choice" in ev:
                pending["choice"] = ev["choice"]
                pending["log_before"] = seg
                seg = []
                yield pending
                pending = None
            continue
        split = ev.split("|")
        kind = split[1] if len(split) > 1 else ""
        if kind not in IGNORE and kind != "request":
            seg.append(ev)
        if kind == "request":
            payload = "|".join(split[2:])
            if not payload:
                continue
            req = json.loads(payload)
            battle.parse_request(req)
            if req.get("wait"):
                continue
            if req.get("teamPreview"):
                pending = {"kind": "preview", "battle": battle, "bot": bot, "request": req, "open_sheets": open_sheets}
            else:
                pending = {"kind": "switch" if req.get("forceSwitch") else "turn", "battle": battle, "bot": bot,
                           "request": req, "turn": battle.turn, "open_sheets": open_sheets}
            continue
        if kind in IGNORE:
            continue
        if kind in ("win", "tie"):
            yield {"kind": "end", "battle": battle, "winner": split[2] if kind == "win" and len(split) > 2 else None,
                   "log_before": seg}
            return
        if kind == "showteam":
            read_sheet(battle, split)            # (2026-10-22) poke-env's own handler raises on these: sheet never read
            continue
        try:
            battle.parse_message(split)
        except Exception:
            pass
