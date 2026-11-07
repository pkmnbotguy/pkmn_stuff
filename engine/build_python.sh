#!/usr/bin/env bash
# Build the vgcsim Python module and install it into the ACTIVE virtual environment (Linux / macOS / Git Bash).
# Uses pgo/<platform>.profdata when present (bash tools/pgo.sh makes it; PGO=0 = plain build).
set -euo pipefail
[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"
if [ -z "${VIRTUAL_ENV:-}" ]; then echo "activate a virtual environment first" >&2; exit 1; fi
VGC="$(cd "$(dirname "$0")" && pwd)"
case "$(uname -s)" in MINGW*|MSYS*|CYGWIN*) PLAT=windows ;; *) PLAT=linux ;; esac
if [ "${PGO:-1}" != 0 ] && [ -f "$VGC/pgo/$PLAT.profdata" ]; then
    export RUSTFLAGS="${RUSTFLAGS:-} -Cprofile-use=$VGC/pgo/$PLAT.profdata -Cllvm-args=-no-pgo-warn-mismatch"
fi
cd "$VGC/py"
pip install -q "maturin>=1.5"
maturin develop --release
