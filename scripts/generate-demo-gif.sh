#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if ! command -v vhs >/dev/null 2>&1; then
  echo "vhs is required to render docs/assets/pump-demo.gif" >&2
  echo "Install it from https://github.com/charmbracelet/vhs, then rerun make demo." >&2
  exit 1
fi

if ! command -v bat >/dev/null 2>&1; then
  echo "bat is required for syntax-highlighted demo rendering" >&2
  echo "Install it from https://github.com/sharkdp/bat, then rerun make demo." >&2
  exit 1
fi

cargo build --release --locked
export PATH="$repo_root/target/release:$PATH"

mkdir -p docs/assets
vhs docs/demo/pump.tape
