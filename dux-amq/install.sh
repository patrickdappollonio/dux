#!/usr/bin/env bash
# install.sh — set up dux + AMQ on a Linux VM with a persistent disk at /data.
# Idempotent: re-run at will. Won't move files on the boot disk if /data is
# missing — bails early.
set -euo pipefail

STATE_ROOT="${STATE_ROOT:-/data/state}"
LOCAL_BIN="${HOME}/.local/bin"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

say()  { printf '\033[1;34m→\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!\033[0m %s\n' "$*" >&2; }
ok()   { printf '\033[1;32m✓\033[0m %s\n' "$*"; }

# 1. preflight ---------------------------------------------------------------
[[ -d /data ]] || { warn "/data not mounted — set up a persistent disk first."; exit 1; }
mkdir -p "$STATE_ROOT"/{claude,agents,codex,gemini,dux,amq,worktrees,scripts} "$LOCAL_BIN"
ok "state dirs ready under $STATE_ROOT"

# 2. dux ---------------------------------------------------------------------
if ! command -v dux >/dev/null 2>&1; then
  say "installing dux"
  TAG=$(curl -fsSL https://api.github.com/repos/patrickdappollonio/dux/releases/latest | grep -oP '"tag_name":\s*"\K[^"]+')
  TMP=$(mktemp -d); cd "$TMP"
  curl -fsSL -o dux.tar.gz "https://github.com/patrickdappollonio/dux/releases/download/${TAG}/dux-linux-amd64.tar.gz"
  tar -xzf dux.tar.gz; install -m 0755 dux "$LOCAL_BIN/dux"
  cd - >/dev/null; rm -rf "$TMP"
fi
ok "dux: $(dux --help 2>&1 | head -1 || echo installed)"

# 3. AMQ ---------------------------------------------------------------------
if ! command -v amq >/dev/null 2>&1; then
  say "installing amq"
  curl -fsSL https://raw.githubusercontent.com/avivsinai/agent-message-queue/main/scripts/install.sh | bash
fi
amq init --root "$STATE_ROOT/amq" --agents claude,codex,gemini --force >/dev/null
chmod 700 "$STATE_ROOT/amq"
ok "amq queue at $STATE_ROOT/amq"

# 4. AMQ skills (gives Claude/etc. native knowledge of amq) ------------------
if command -v npx >/dev/null 2>&1; then
  npx -y skills add avivsinai/agent-message-queue -g -y >/dev/null 2>&1 || \
    warn "npx skills add failed; install manually if needed"
fi

# 5. install wrappers --------------------------------------------------------
say "installing wrappers to $LOCAL_BIN"
install -m 0755 "$HERE/wrappers/claude-amq"  "$LOCAL_BIN/claude-amq"
install -m 0755 "$HERE/wrappers/codex-amq"   "$LOCAL_BIN/codex-amq"
install -m 0755 "$HERE/wrappers/gemini-amq"  "$LOCAL_BIN/gemini-amq"
install -m 0755 "$HERE/scripts/finalize-claude-migration.sh" "$STATE_ROOT/scripts/finalize-claude-migration.sh"

# 6. dux config --------------------------------------------------------------
DUX_HOME="$STATE_ROOT/dux" dux config regenerate --yes >/dev/null
say "patching $STATE_ROOT/dux/config.toml"
sed -i \
  -e 's|^prompt_for_name = false$|prompt_for_name = true|' \
  -e 's|^command = "claude"$|command = "claude-amq"|' \
  -e 's|^command = "codex"$|command = "codex-amq"|' \
  -e 's|^command = "gemini"$|command = "gemini-amq"|' \
  -e 's|^resume_args = \["--continue"\]$|resume_args = ["--continue", "--fork-session"]|' \
  "$STATE_ROOT/dux/config.toml"

# 7. shell rc ----------------------------------------------------------------
if ! grep -q '=== dux + AMQ ===' "$HOME/.bashrc" 2>/dev/null; then
  say "appending env stanza to ~/.bashrc"
  cat "$HERE/config/bashrc-additions.sh" >> "$HOME/.bashrc"
fi

# 8. global CLAUDE.md --------------------------------------------------------
if ! grep -q 'Multi-agent environment (AMQ + dux)' "$HOME/.claude/CLAUDE.md" 2>/dev/null; then
  say "appending AMQ section to ~/.claude/CLAUDE.md"
  printf '\n\n' >> "$HOME/.claude/CLAUDE.md"
  cat "$HERE/config/claude-md-additions.md" >> "$HOME/.claude/CLAUDE.md"
fi

# 9. VSCode Remote-SSH machine settings (best-effort) ------------------------
# Free Ctrl-G in the integrated terminal so dux's `exit_interactive` works.
# Workbench-level settings like commandsToSkipShell are typically resolved
# on the LOCAL machine, so this VM-side write may or may not propagate. We
# do it anyway because it's harmless when ineffective and helpful otherwise.
# The User-settings copy-paste printed below is the authoritative fix.
configure_vscode_remote() {
  local f="$HOME/.vscode-server/data/Machine/settings.json"
  [[ -d "$(dirname "$f")" ]] || return 0
  if ! command -v jq >/dev/null 2>&1; then
    warn "jq not installed; skipping VM-side VSCode settings merge"
    return 0
  fi
  local entries='["-workbench.action.gotoLine","-workbench.action.terminal.goToRecentDirectory"]'
  if [[ ! -f "$f" ]]; then
    printf '%s\n' "{
  \"terminal.integrated.commandsToSkipShell\": $entries
}" > "$f"
    ok "wrote $f"
    return 0
  fi
  if jq --argjson new "$entries" '
    .["terminal.integrated.commandsToSkipShell"] = (
      ((.["terminal.integrated.commandsToSkipShell"] // []) + $new) | unique
    )
  ' "$f" > "$f.tmp" && mv "$f.tmp" "$f"; then
    ok "merged Ctrl-G passthrough into $f"
  else
    warn "could not merge $f"
  fi
}
configure_vscode_remote

ok "install complete"
echo
echo "Next steps:"
echo "  1. exec bash -l                  # pick up new env"
echo "  2. (optional) $STATE_ROOT/scripts/finalize-claude-migration.sh"
echo "     # ONLY after closing every running 'claude' process"
echo "  3. dux                            # launch"
echo
echo "─── VSCode Remote-SSH (Windows / macOS local) ───"
echo "If Ctrl-G still opens VSCode's 'Go to Recent Directory' picker after"
echo "restarting dux, the workbench setting must live on your LOCAL machine."
echo "Open VSCode → Cmd/Ctrl+Shift+P → 'Preferences: Open User Settings (JSON)'"
echo "and merge into the existing terminal.integrated.commandsToSkipShell"
echo "array (or add the key if absent):"
echo
cat <<'JSON'
    "terminal.integrated.commandsToSkipShell": [
      "-workbench.action.gotoLine",
      "-workbench.action.terminal.goToRecentDirectory"
    ]
JSON
echo
echo "Both entries are needed: the first frees Ctrl-G in editors, the"
echo "second frees Ctrl-G inside the integrated terminal (which is the"
echo "one that bites in dux)."
