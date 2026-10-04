#!/usr/bin/env bash
set -euo pipefail

command -v bun >/dev/null || { echo "Install Bun before running setup." >&2; exit 1; }
command -v cargo >/dev/null || { echo "Install stable Rust before running setup." >&2; exit 1; }
command -v rustc >/dev/null || { echo "Install stable Rust before running setup." >&2; exit 1; }
if [[ "$(uname -s)" == "Darwin" ]]; then
  command -v xcode-select >/dev/null || { echo "Install Xcode Command Line Tools." >&2; exit 1; }
  xcode-select -p >/dev/null || exit 1
fi

cargo fetch --locked
bun install --frozen-lockfile
