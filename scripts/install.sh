#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

cargo install --path .

cat <<'EOF'
Installed `dux` with Cargo.

Run:
  dux

On first launch, dux writes a fully materialized default config to:
  ~/.config/dux/config.toml
EOF
