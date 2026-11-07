#!/usr/bin/env bash
# Profile-guided optimisation for the fast engine (optional, ~+10% speed).
# Builds an instrumented copy in target_pgo_gen, runs the benchmarks, merges the profile into pgo/<platform>.profdata.
# build_python.sh / build_python.cmd use that profile when it exists (PGO=0 = plain build).
# Needs: rustup component add llvm-tools-preview
set -euo pipefail
cd "$(dirname "$0")/.."
case "$(uname -s)" in MINGW*|MSYS*|CYGWIN*) PLAT=windows ;; *) PLAT=linux ;; esac
RAW="$PWD/target_pgo_gen/raw"
rm -rf "$RAW"; mkdir -p "$RAW" pgo
export RUSTFLAGS="-Cprofile-generate=$RAW"
CARGO_TARGET_DIR=target_pgo_gen cargo build --release --bin fast_bench
target_pgo_gen/release/fast_bench data/dex.json data/pairs.json 20000 > /dev/null
PROFDATA="$(find "$(rustc --print sysroot)" -name 'llvm-profdata*' -type f | head -1)"
"$PROFDATA" merge -o "pgo/$PLAT.profdata" "$RAW"
echo "wrote pgo/$PLAT.profdata"
