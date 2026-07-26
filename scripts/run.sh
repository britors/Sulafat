#!/usr/bin/env bash
# Builds and launches Sulafat in the background for manual testing, mirroring the ad-hoc
# commands used at the terminal (SULAFAT_LOG, RUST_BACKTRACE, nohup + disown so it survives
# the shell, output captured to a log file instead of scrolling past).
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

log_file="${SULAFAT_RUN_LOG:-$repo_root/.run/sulafat.log}"
mkdir -p "$(dirname "$log_file")"

pkill -f "target/debug/sulafat" 2>/dev/null || true

cargo build -p sulafat-gtk

SULAFAT_LOG="${SULAFAT_LOG:-debug}" RUST_BACKTRACE=1 nohup ./target/debug/sulafat >"$log_file" 2>&1 &
disown

echo "sulafat rodando, pid=$!"
echo "log: $log_file"
