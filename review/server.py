"""Game review app.

Input: a game's Showdown replay (a replay.pokemonshowdown.com link) or its battle log (a finished game), which player you
were, and your team paste. Output: a decision-by-decision review: win chance before and after each decision, the search's view of
every option, labels (best / excellent / good / inaccuracy / mistake / blunder), accuracy, a team-preview analysis.

    python review/server.py            # then open http://127.0.0.1:8765

Needs network weights (REVIEW_CKPT, default
weights/net.pt; weights are not part of this repository) and the compiled engine (engine/, see the README).
API: POST /api/review_replay {"url" or "log", "player": "p1" | "p2" | name, "paste": team} -> {"id", "url"}
     GET  /api/review/<id> -> {"status": "queued|running|done|error", "progress": [done, total], "result": {...}}
     GET  /api/reviews     -> recent reviews
Reviews are saved in review/reviews/. Listens on 127.0.0.1 only (REVIEW_HOST / REVIEW_PORT to change).
"""
import http.server
import json
import os
import queue
import re
import sys
import threading
import time
import traceback
import uuid

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
PORT = int(os.environ.get("REVIEW_PORT", "8765"))
HOST = os.environ.get("REVIEW_HOST", "127.0.0.1")
STORE = os.path.join(HERE, "reviews")
WEB = os.path.join(HERE, "web")
os.makedirs(STORE, exist_ok=True)

jobs = queue.Queue()
state = {}                    # id -> {"status", "progress", "name", "opponent", "created", "error"}
lock = threading.Lock()


def worker():
    import core
    reviewer = None
    while True:
        rid = jobs.get()
        with lock:
            state[rid]["status"] = "running"
        try:
            if reviewer is None:
                reviewer = core.Reviewer(preset=os.environ.get("REVIEW_PRESET", "standard"))
            game = json.load(open(os.path.join(STORE, f"{rid}_game.json"), encoding="utf-8"))

            def progress(done, total):
                with lock:
                    state[rid]["progress"] = [done, total]

            result = reviewer.review(game, progress=progress)
            result["created"] = state[rid]["created"]
            json.dump(result, open(os.path.join(STORE, f"{rid}.json"), "w", encoding="utf-8"))
            with lock:
                state[rid]["status"] = "done"
        except Exception as e:
            traceback.print_exc()
            with lock:
                state[rid].update(status="error", error=f"{type(e).__name__}: {e}")


# protocol lines the replay viewer doesn't need (or that only make sense to the player's own client)
VIEWER_SKIP = ("|request|", "|uhtml", "|uhtmlchange", "|t:|", "|c|", "|c:|", "|j|", "|l|", "|n|", "|raw|", "|html|", "|init|",
               "|title|", "|inactive", "|timer|", "|sentchoice|", "|error|")


def viewer_log(game):
    """The battle's protocol log for Showdown's replay player: the replay's own log, or the player's recorded stream
    without requests / chat / timer lines"""
    if game.get("log"):
        return game["log"]
    out = []
    for e in game.get("events", []):
        if not isinstance(e, str):
            continue
        for line in e.split("\n"):
            if line.startswith("|") and line != "|" and not line.startswith(VIEWER_SKIP):
                out.append(line)
    return "\n".join(out)


REPLAY_PAGE = """<!doctype html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<style>
html, body {{ margin: 0; background: transparent; overflow: hidden; }}
.wrapper.replay-wrapper {{ max-width: 640px !important; margin: 0 !important; }}
.replay-controls, .replay-controls-2 {{ padding: 6px 4px 0; }}
.battle-log {{ display: none; }}
/* normal flow instead of Showdown's absolute layout (its 460px top padding left a gap when the battle scales down) */
html body .wrapper.replay-wrapper {{ padding-top: 0 !important; display: flow-root; }}
.replay-wrapper .battle {{ position: relative !important; top: 0 !important; left: 0 !important; margin-top: 2px; }}
.replay-wrapper .replay-controls, .replay-wrapper .replay-controls-2 {{ position: static !important; width: auto !important; height: auto !important; }}
@media (max-width: 520px) {{ .replay-wrapper .replay-controls-2 {{ display: none; }} }}
</style></head><body>
<div class="wrapper replay-wrapper"><div class="battle"></div><div class="battle-log"></div><div class="replay-controls"></div><div class="replay-controls-2"></div></div>
<script type="text/plain" class="battle-log-data">{log}</script>
<script>window.REVIEW_PLAYER = {player};</script>
<script src="https://play.pokemonshowdown.com/js/replay-embed.js"></script>
<script>
// the review page (parent, same origin) reads Replays.battle.turn and calls Replays.battle.seekTurn(n)
window.addEventListener("load", function () {{
  setTimeout(function () {{
    if (window.REVIEW_PLAYER === "p2" && window.Replays && Replays.battle) Replays.battle.switchViewpoint();
    if (window.parent) window.parent.postMessage({{replayReady: true}}, "*");
  }}, 0);
}});
</script>
</body></html>"""


def status_of(rid):
    with lock:
        s = dict(state.get(rid, {}))
    path = os.path.join(STORE, f"{rid}.json")
    if (not s or s.get("status") == "done") and os.path.exists(path):
        s = dict(s, status="done", result=json.load(open(path, encoding="utf-8")))
    return s or None



def game_species(rid):
    """every species name the game's log shows (team preview |poke| lines, forms from |switch| / |detailschange|)"""
    out = []
    try:
        g = json.load(open(os.path.join(STORE, f"{rid}_game.json"), encoding="utf-8"))
        for e in g.get("events") or []:
            if not isinstance(e, str):
                continue
            q = e.split("|")
            if len(q) > 3 and q[1] in ("poke", "switch", "drag", "replace", "detailschange"):
                name = q[3].split(",")[0].strip()
                if name and name not in out:
                    out.append(name)
    except Exception:
        pass
    return out




class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, fmt, *args):
        pass

    def send_bytes(self, body, ctype, code=200):
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def send_json(self, obj, code=200):
        self.send_bytes(json.dumps(obj).encode("utf-8"), "application/json", code)

    def load_game(self, rid):
        path = os.path.join(STORE, f"{rid}_game.json")
        return json.load(open(path, encoding="utf-8")) if os.path.exists(path) else None

    def do_POST(self):
        if self.path != "/api/review_replay":
            return self.send_json({"error": "not found"}, 404)
        try:
            n = int(self.headers.get("Content-Length", "0"))
            body = json.loads(self.rfile.read(n).decode("utf-8"))
            import from_replay
            log = (body.get("log") or "").strip() or from_replay.fetch(body["url"])
            paste = (body.get("paste") or "").strip()
            if not paste:
                raise ValueError("paste the team you played")
            if not from_replay.finished(log.split("\n")):
                raise ValueError("only finished games can be reviewed")
            game = from_replay.build(log, body["player"], paste)
            game["log"] = log                  # the original log, for the replay viewer next to the review
        except Exception as e:
            return self.send_json({"error": f"bad game data: {type(e).__name__}: {e}"}, 400)
        rid = uuid.uuid4().hex[:12]
        json.dump(game, open(os.path.join(STORE, f"{rid}_game.json"), "w", encoding="utf-8"))
        foe = next((e.split("|")[3] for e in game["events"] if isinstance(e, str) and e.startswith("|player|")
                    and e.split("|")[2] != game.get("player", "p1") and len(e.split("|")) > 3), "")
        with lock:
            state[rid] = {"status": "queued", "progress": [0, 0], "name": game["name"], "opponent": foe,
                          "created": time.strftime("%Y-%m-%d %H:%M")}
        jobs.put(rid)
        self.send_json({"id": rid, "url": f"/review/{rid}"})

    def do_GET(self):
        p = self.path.split("?")[0]
        if p in ("/", "/index.html"):
            return self.send_bytes(open(os.path.join(WEB, "index.html"), "rb").read(), "text/html; charset=utf-8")
        if re.fullmatch(r"/review/[0-9a-f]{12}", p):
            return self.send_bytes(open(os.path.join(WEB, "review.html"), "rb").read(), "text/html; charset=utf-8")
        m = re.fullmatch(r"/replay/([0-9a-f]{12})", p)
        if m:
            game = self.load_game(m.group(1))
            if game is None:
                return self.send_json({"error": "no such review"}, 404)
            log = viewer_log(game).replace("</", "<\\/")          # replay-embed.js turns \/ back into /
            page = REPLAY_PAGE.format(log=log, player=json.dumps(game.get("player", "p1")))
            return self.send_bytes(page.encode("utf-8"), "text/html; charset=utf-8")
        m = re.fullmatch(r"/api/game/([0-9a-f]{12})", p)
        if m:
            game = self.load_game(m.group(1))
            if game is None:
                return self.send_json({"error": "no such review"}, 404)
            return self.send_json({"player": game.get("player", "p1"), "log": viewer_log(game)})
        m = re.fullmatch(r"/api/review/([0-9a-f]{12})", p)
        if m:
            s = status_of(m.group(1))
            if not s:
                return self.send_json({"error": "no such review"}, 404)
            if s.get("status") == "done" and isinstance(s.get("result"), dict) and "species" not in s["result"]:
                s["result"]["species"] = game_species(m.group(1))
            return self.send_json(s)
        if p == "/api/reviews":
            items = []
            files = sorted(os.listdir(STORE), key=lambda f: -os.path.getmtime(os.path.join(STORE, f)))
            for f in files:
                mm = re.fullmatch(r"([0-9a-f]{12})_game\.json", f)
                if not mm:
                    continue
                rid = mm.group(1)
                s = status_of(rid) or {"status": "unknown"}
                r = s.get("result") or {}
                items.append({"id": rid, "status": s.get("status"), "name": s.get("name") or r.get("name"),
                              "title": r.get("title") or (f"vs {s['opponent']}" if s.get("opponent") else None),
                              "result": r.get("result"), "accuracy": r.get("accuracy"),
                              "created": s.get("created") or r.get("created")})
            return self.send_json(items[:50])
        return self.send_json({"error": "not found"}, 404)


def main():
    threading.Thread(target=worker, daemon=True).start()
    srv = http.server.ThreadingHTTPServer((HOST, PORT), Handler)
    print(f"review app: http://{HOST}:{PORT}", flush=True)
    srv.serve_forever()


if __name__ == "__main__":
    main()
