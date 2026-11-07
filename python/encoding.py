"""Feature-layout constants and helpers around the engine's input encoders (game memory frames, set library,
process-wide encoder switches, team folder loading)."""

import paths
import glob
import os
import re
import numpy as np
import vgcsim


HERE = os.path.dirname(os.path.abspath(__file__))


DATA = paths.ENGINE_DATA


_NON_ID = re.compile(r"[^a-z0-9]")


def to_id(s):
    return _NON_ID.sub("", str(s or "").lower())


def _fnv_u(s, h=0xe50eb7106b29188f):
    """Deterministic 64-bit string mixer -> float in (0, 1)."""
    for c in s:
        h = ((h ^ ord(c)) * 0x100000001b3) & 0xffffffffffffffff
    h = ((h ^ (h >> 30)) * 0xbf58476d1ce4e5b9) & 0xffffffffffffffff
    h = ((h ^ (h >> 27)) * 0x94d049bb133111eb) & 0xffffffffffffffff
    h = h ^ (h >> 31)
    return ((h >> 11) + 0.5) / 9007199254740992.0


def _species_of(block):
    """species id of a Showdown export block ('Nick (Species) (F) @ Item' / 'Species @ Item')"""
    head = block.strip().split("\n")[0].split("@")[0].strip()
    head = re.sub(r"\((M|F)\)\s*$", "", head).strip()
    m = re.search(r"\(([^()]+)\)\s*$", head)
    name = m.group(1) if m else head
    return to_id(name)


def _canon_block(block):
    # fixed hash order normalizes raw pokepastes without alphabetical lead/move-0 bias
    lines = [ln.strip() for ln in block.strip().split("\n") if ln.strip()]
    if not lines:
        return ""
    other, moves = [], []
    for ln in lines:
        if ln.startswith("-"):
            moves.append(ln)
        else:
            other.append(ln)
    moves.sort(key=lambda ln: (_fnv_u("move|" + to_id(ln[1:])), to_id(ln[1:])))
    return "\n".join(other + moves)


def canonical_team(text):
    """Canonical Pokemon and move-slot ordering so identical teams produce identical feature tensors and preview indices."""
    blocks = [_canon_block(b) for b in re.split(r"\n\s*\n", text.strip()) if b.strip()]
    blocks = [b for b in blocks if b]
    blocks.sort(key=lambda b: (_fnv_u("species|" + _species_of(b)), _species_of(b)))
    return "\n\n".join(blocks) + "\n"


def load_teams(folder="Teams_real", names=False):
    """The teams in a folder under teams/, canonicalized. names=True: (folder/file name, text) pairs instead of texts."""
    out = []
    for f in sorted(glob.glob(os.path.join(paths.TEAMS, folder, "*.txt"))):
        t = open(f, encoding="utf-8").read().strip()
        if t:
            out.append((f"{folder}/{os.path.basename(f)}", canonical_team(t)))
    return out if names else [t for _, t in out]


_UNC_SET = False


def apply_open_gate(on=None):
    """(2026-11-03) Open-sheet leak fix (vgcsim.set_open_gate): open-sheet views hide opponent Pokemon not seen yet (the
    rest of the sheet goes to the v3 rows, sorted) and open-sheet search worlds shuffle which sheet Pokemon were brought.
    From `on` or the env VGCSIM_OPEN_GATE ("1" = on), so worker processes follow the trainer. Old nets were trained with it off."""
    on = os.environ.get("VGCSIM_OPEN_GATE", "0") == "1" if on is None else bool(on)
    if hasattr(vgcsim, "set_open_gate"):
        vgcsim.set_open_gate(on)
    elif on:
        raise SystemExit("open gate: the vgcsim module has no set_open_gate (rebuild it)")
    return on


def ensure_unc_library(team_dir="Teams_real"):
    """Sets the process-wide real-set library behind the uncertainty-aware damage features (once). Nets with the unc
    input call it when built."""
    global _UNC_SET
    if _UNC_SET or not hasattr(vgcsim, "set_unc_library"):
        return
    n = vgcsim.set_unc_library(vgcsim.Dex(os.path.join(DATA, "dex.json")), load_teams(team_dir))
    _UNC_SET = True
    print(f"uncertainty-aware damage features: set library of {n} species ({team_dir})", flush=True)


MEM0, MEM_K, MEM_F = 2468, 16, 78


MEM_W = MEM_K * MEM_F


_HIST0, _HIST_F = 776, 20


def mem_frame(f):
    """compact frames [n, MEM_F] from base observations [n, >= 640]"""
    n = f.shape[0]
    out = np.zeros((n, MEM_F), np.float32)
    out[:, 0] = 1.0
    m = f[:, :640].reshape(n, 8, 80)
    h = f[:, _HIST0: _HIST0 + 8 * _HIST_F].reshape(n, 8, _HIST_F)
    per = np.concatenate([m[:, :, 1:4], h[:, :, 1:5], h[:, :, 9:11]], -1)          # [n, 8, 9]
    out[:, 1:73] = per.reshape(n, 72)
    g = f[:, 640:680]
    out[:, 73] = g[:, 0:5].argmax(1) / 4.0
    out[:, 74] = g[:, 6:11].argmax(1) / 4.0
    out[:, 75] = g[:, 12] > 0
    out[:, 76] = g[:, 15] > 0
    out[:, 77] = g[:, 24] > 0
    return out
