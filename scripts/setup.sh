#!/usr/bin/env bash
set -euo pipefail

command -v bun >/dev/null || { echo "Install Bun before running setup." >&2; exit 1; }
command -v cargo >/dev/null || { echo "Install stable Rust before running setup." >&2; exit 1; }
command -v rustc >/dev/null || { echo "Install stable Rust before running setup." >&2; exit 1; }
[[ "$(uname -s)" == "Darwin" ]] || { echo "GoggleLab desktop setup requires macOS." >&2; exit 1; }
command -v xcode-select >/dev/null || { echo "Install Xcode Command Line Tools before running setup." >&2; exit 1; }
xcode-select -p >/dev/null || { echo "Install Xcode Command Line Tools before running setup." >&2; exit 1; }

bun install --frozen-lockfile
