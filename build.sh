#!/usr/bin/env bash
# GameMode build helper: uses the toolchain binaries directly (the rustup proxy is
# broken in this environment) and a project-local target dir so builds never fight
# other sessions for the shared cargo lock.
set -euo pipefail
TC="$HOME/.rustup/toolchains/stable-x86_64-pc-windows-gnu/bin"
export PATH="$TC:$HOME/.cargo/bin-direct:$PATH"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR_OVERRIDE:-F:/study/projects/games/gamemode/target}"
cd "$(dirname "$0")"
exec "$TC/cargo.exe" "$@"
