#!/bin/bash
# (2026-10-18) What would a Showdown update change for Champions M-C?  bash tools/sd_check_upstream.sh [ref, default origin/master]
# git fetch, snapshot the local build (dist as installed) and the ref (throwaway worktree build), report via tools/sd_changes.py.
set -e
SD=${SHOWDOWN_DIR:-../pokemon-showdown}
REF=${1:-origin/master}
cd "$(dirname "$0")/.."
git -C "$SD" fetch -q origin
HERE=$(git -C "$SD" rev-parse --short HEAD); THERE=$(git -C "$SD" rev-parse --short "$REF")
mkdir -p fuzz/sd_snap
node tools/sd_snapshot.js "$SD" gen9championsvgc2026regmc fuzz/sd_snap/local_$HERE.json > /dev/null
bash tools/sd_ref_snapshot.sh "$REF" fuzz/sd_snap/$THERE.json > /dev/null
echo "local $HERE -> $REF $THERE ($(git -C "$SD" rev-list --count HEAD.."$THERE") commits, $(git -C "$SD" rev-list --count HEAD.."$THERE" -- sim data) touching sim/ or data/)"
${PYTHON:-python} tools/sd_changes.py fuzz/sd_snap/local_$HERE.json fuzz/sd_snap/$THERE.json \
  --out fuzz/sd_snap/changes_${HERE}_${THERE}.txt
