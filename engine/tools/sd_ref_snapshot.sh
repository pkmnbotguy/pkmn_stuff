#!/bin/bash
# (2026-10-18) Snapshot (tools/sd_snapshot.js) of ANY Showdown git ref without touching the working copy:
#   bash tools/sd_ref_snapshot.sh <ref> [out.json]        e.g.  origin/master, HEAD, before_update_oct17
# Makes a throwaway git worktree, links the main copy's node_modules into it, builds it (node build), snapshots, removes it.
# Then:  python tools/sd_changes.py fuzz/sd_snap/<current>.json <out.json> --out report.txt
set -e
SD=${SHOWDOWN_DIR:-../pokemon-showdown}
REF=${1:?ref}
cd "$(dirname "$0")/.."
SHA=$(git -C "$SD" rev-parse --short "$REF")
OUT=${2:-fuzz/sd_snap/$SHA.json}
mkdir -p "$(dirname "$OUT")"
WT=$(cygpath -m "${TMPDIR:-/tmp}")/sd_wt_$SHA
git -C "$SD" worktree remove --force "$WT" 2>/dev/null || true
git -C "$SD" worktree add --detach "$WT" "$SHA" > /dev/null
powershell -NoProfile -Command "New-Item -ItemType Junction -Path '$(cygpath -w "$WT/node_modules")' -Target '$(cygpath -w "$SD/node_modules")'" > /dev/null
( cd "$WT" && node build > build.log 2>&1 ) || { echo "build failed: $WT/build.log"; exit 1; }
node tools/sd_snapshot.js "$WT" gen9championsvgc2026regmc "$OUT"
powershell -NoProfile -Command "(Get-Item '$(cygpath -w "$WT/node_modules")').Delete()"
git -C "$SD" worktree remove --force "$WT"
echo "snapshot of $REF ($SHA): $OUT"
