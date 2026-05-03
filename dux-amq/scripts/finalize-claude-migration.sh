#!/usr/bin/env bash
# Finalize migration of ~/.claude and ~/.agents onto persistent disk /data.
# Run this AFTER you've closed every running `claude` process on this VM.
# Idempotent: safe to re-run.

set -euo pipefail

migrate_dir() {
  local src="$1" dst="$2"
  local bak
  bak="${src}.bak.$(date +%Y%m%d-%H%M%S)"

  if [[ -L "$src" ]]; then
    echo "✓ $src is already a symlink → $(readlink "$src"). Skipping."
    return 0
  fi
  if [[ ! -e "$src" ]]; then
    echo "ℹ $src does not exist. Creating fresh symlink → $dst."
    mkdir -p "$dst"
    ln -s "$dst" "$src"
    return 0
  fi

  echo "→ Final delta rsync $src/ → $dst/"
  mkdir -p "$dst"
  rsync -aH --delete "$src/" "$dst/"
  echo "→ Backing up original $src → $bak"
  mv "$src" "$bak"
  echo "→ Creating symlink $src → $dst"
  ln -s "$dst" "$src"
  echo "  Backup retained at $bak — delete after verifying:"
  echo "    rm -rf $bak"
}

if pgrep -u "$USER" -fa '(^|/)claude( |$)' >/dev/null; then
  echo "ERROR: a 'claude' process is still running. Exit all Claude Code sessions first." >&2
  pgrep -u "$USER" -fa '(^|/)claude( |$)' >&2
  exit 1
fi

migrate_dir "$HOME/.claude"  "/data/state/claude"
migrate_dir "$HOME/.agents"  "/data/state/agents"

# The skills CLI creates RELATIVE symlinks under ~/.claude/skills/ pointing to
# ~/.agents/skills/ via "../../.agents/...". After migration, ~/.claude is a
# symlink to /data/state/claude, so the relative path resolves to
# /data/state/.agents/... — which doesn't exist. Solve once with a sibling
# symlink so present and future skill installs Just Work.
if [[ ! -e /data/state/.agents ]]; then
  echo "→ Creating /data/state/.agents → /data/state/agents (relative-path bridge for skills)"
  ln -s /data/state/agents /data/state/.agents
fi

echo
echo "✓ Done. ~/.claude and ~/.agents now live on /data/state."
