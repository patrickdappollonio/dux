#!/usr/bin/env bash
# install.sh — set up dux + AMQ on a Linux VM with a persistent disk at /data.
# Idempotent: re-run at will. Won't move files on the boot disk if /data is
# missing — bails early.
#
# Supply-chain pins (audit01 / P0-2). Update these together when bumping a
# dependency; recompute hashes against fresh downloads (see Validation section
# in docs/plans/audits/audit01/01-supply-chain-hardening.md).
#
#   dux        v0.4.0
#     tarball: dux-linux-amd64.tar.gz
#     sha256:  a1c449989e9c4dd53b260d75d29d0d5d6832b3852cf5327f3725b5e7bb881102
#
#   amq        v0.34.0   (commit 6a9417d40cc8b9d9f71e9fbb1e39c872d0763b54)
#     tarball: amq_0.34.0_linux_amd64.tar.gz
#     sha256:  cba940987d00a3d072f395c7ec7a648e47d652f1ff503abf46da538595510d7a
#
#   skills     1.5.3 (npm)
#     skills-rev (avivsinai/agent-message-queue commit pinned for `skills add`)
#                6a9417d40cc8b9d9f71e9fbb1e39c872d0763b54
set -euo pipefail

STATE_ROOT="${STATE_ROOT:-/data/state}"
LOCAL_BIN="${HOME}/.local/bin"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Pinned versions + sha256 (overrideable for testing only; CI must use defaults).
DUX_TAG="${DUX_TAG:-v0.4.0}"
DUX_SHA256="${DUX_SHA256:-a1c449989e9c4dd53b260d75d29d0d5d6832b3852cf5327f3725b5e7bb881102}"
AMQ_TAG="${AMQ_TAG:-v0.34.0}"
AMQ_VERSION="${AMQ_VERSION:-0.34.0}"
AMQ_SHA256="${AMQ_SHA256:-cba940987d00a3d072f395c7ec7a648e47d652f1ff503abf46da538595510d7a}"
SKILLS_PIN="${SKILLS_PIN:-1.5.3}"
SKILLS_REV="${SKILLS_REV:-6a9417d40cc8b9d9f71e9fbb1e39c872d0763b54}"

# Expected sha256 of the extracted amq binary (audit01 P1-8). Cross-checked
# against the file inside amq_${AMQ_VERSION}_linux_amd64.tar.gz at install
# time so a tampered-with binary already in $PATH is rejected before being
# pinned at $STATE_ROOT/amq-bin/amq.
AMQ_BINARY_SHA256="${AMQ_BINARY_SHA256:-eb78901f3dd13534884923e02ad9c6852be1b0a4c7f452fe52b8bcd795e3556b}"

# AUDIT01-VERSION — overlay version; gates idempotent config-block rewrites
# (Phase 12). Phase 15's release pipeline rewrites this line on tag.
DUX_AMQ_VERSION="${DUX_AMQ_VERSION:-0.1.0}"

say()  { printf '\033[1;34m→\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!\033[0m %s\n' "$*" >&2; }
ok()   { printf '\033[1;32m✓\033[0m %s\n' "$*"; }

# Audit02 Phase 13 (audit01 P1-1): detect kernel-side TIOCSTI support.
#
# AMQ v0.34.0's `--inject-mode raw` uses `unix.Syscall(SYS_IOCTL, fd,
# unix.TIOCSTI, ...)` with no PTY-master fallback. Linux 6.2 (Nov 2022)
# made `CONFIG_LEGACY_TIOCSTI` default-off, and Ubuntu 24.04 LTS /
# Debian 12+ ship the option built out entirely (the sysctl key is
# absent — it isn't merely set to 0). Without TIOCSTI, every wake
# notification is silently dropped before reaching the agent's TTY.
#
# Returns:
#   0 — sysctl reports `1` (kernel built with the option AND it's on)
#   1 — sysctl reports `0` (compiled in but disabled at runtime; the
#       operator can lift it with `sudo sysctl -w dev.tty.legacy_tiocsti=1`)
#   2 — sysctl key absent / file not found (kernel built without the
#       option; runtime toggle won't help)
#
# Reads `/proc/sys/dev/tty/legacy_tiocsti` directly rather than shelling
# out to `sysctl`. The `sysctl` binary lives in `/sbin` on Debian and
# isn't on a non-root user's PATH; the procfs file IS readable to all
# users (mode 0644 on stock kernels). One less external dependency.
tiocsti_status() {
  # `TIOCSTI_PROC_PATH` is overridable for tests; production callers
  # leave it unset and we fall back to the canonical procfs file.
  local proc_path="${TIOCSTI_PROC_PATH:-/proc/sys/dev/tty/legacy_tiocsti}"
  if [[ ! -e "$proc_path" ]]; then
    return 2
  fi
  local val
  val=$(<"$proc_path") 2>/dev/null || return 2
  case "$val" in
    1) return 0 ;;
    0) return 1 ;;
    *) return 2 ;;
  esac
}

# Verify a downloaded artifact's sha256 against an expected value. Bails out
# on mismatch — the calling install branch must not proceed.
verify_sha256() {
  local file="$1" expected="$2" label="$3" actual
  actual=$(sha256sum "$file" | awk '{print $1}')
  if [[ "$actual" != "$expected" ]]; then
    warn "$label sha256 mismatch: got $actual, expected $expected"
    exit 1
  fi
  ok "$label sha256 verified ($actual)"
}

# Audit01 P1-7: strip any prior dux-amq versioned block from a config file so
# the next append always lands a clean current-version block. Also migrates
# the legacy unversioned `=== dux + AMQ ===`/`Multi-agent environment (AMQ +
# dux)` blocks. `kind` selects the marker style:
#   sh  → `# >>> dux-amq vN.M.K >>>` … `# <<< dux-amq vN.M.K <<<`
#   md  → `<!-- >>> dux-amq vN.M.K >>> -->` … `<!-- <<< dux-amq vN.M.K <<< -->`
strip_block() {
  local file="$1" kind="${2:-sh}"
  [[ -f "$file" ]] || return 0
  local tmp; tmp=$(mktemp "${file}.dux-amq.XXXXXX")
  case "$kind" in
    sh)
      awk '
        /^# >>> dux-amq v[^ ]+ >>>$/ {s=1; next}
        /^# <<< dux-amq v[^ ]+ <<<$/ {s=0; next}
        # Legacy (audit01 pre-Phase-12) markers — migrate by stripping.
        /^# === dux \+ AMQ ===$/        {s=1; next}
        /^# === end dux \+ AMQ ===$/    {s=0; next}
        !s
      ' "$file" > "$tmp"
      ;;
    md)
      # Audit02 P0-G: the legacy CLAUDE.md branch used to set s=1 on the
      # heading and never reset it — anything appended after the AMQ
      # stanza (a user's own `## Notes` etc.) was deleted to EOF. Fix:
      # also reset s=0 when awk hits the *next* `## ` sibling heading.
      awk '
        /^<!-- >>> dux-amq v[^ ]+ >>> -->$/ {s=1; next}
        /^<!-- <<< dux-amq v[^ ]+ <<< -->$/ {s=0; next}
        # Legacy: explicit end sentinel (added by Phase 02). Only present
        # in installs from versions that wrote it.
        /^<!-- end dux-amq legacy -->$/ {s=0; next}
        # Legacy fallback: heading through next "## " sibling (NOT EOF).
        /^## Multi-agent environment \(AMQ \+ dux\)$/ {s=1; next}
        s && /^## /                                   {s=0}
        !s
      ' "$file" > "$tmp"
      ;;
    *) warn "strip_block: unknown kind: $kind"; rm -f "$tmp"; return 1 ;;
  esac
  mv "$tmp" "$file"
}

# 1. preflight ---------------------------------------------------------------
[[ -d /data ]] || { warn "/data not mounted — set up a persistent disk first."; exit 1; }
# Audit01 P1-6 / Audit02 P1-C: hard-fail on missing tools, but collect ALL
# misses in one pass so the operator can `apt-get install` the full list in
# one shot instead of fix → re-run → next-error → repeat.
#
# `realpath` (GNU coreutils, audit02 Phase 12) — required by the wrappers'
#   is_dux_worktree helper for canonicalised path-segment containment
#   (audit01 P0-5). The `--` arg-separator is GNU-only; stock BSD realpath
#   on macOS will fail at wrapper time even if the binary exists.
# `openssl` (audit02 Phase 08) — required by amq-secret-init.sh /
#   amq-send-signed / amq-receive-verify for HMAC envelope signing.
# `jq` was a soft dep at the VSCode-settings step; required so we can
#   drop the non-portable `grep -oP` PCRE scrape entirely.
missing=()
for _tool in curl jq sha256sum tar install git rsync awk sed realpath openssl; do
  command -v "$_tool" >/dev/null 2>&1 || missing+=("$_tool")
done
unset _tool
if (( ${#missing[@]} > 0 )); then
  warn "missing required tools: ${missing[*]}"
  warn "  Debian/Ubuntu: apt-get install -y ${missing[*]}"
  warn "  macOS:         brew install ${missing[*]}"
  exit 1
fi
mkdir -p "$STATE_ROOT"/{claude,agents,codex,gemini,dux,amq,worktrees,scripts} "$LOCAL_BIN"
ok "state dirs ready under $STATE_ROOT"

# Audit02 Phase 13: TIOCSTI kernel-state detection. Write a sentinel
# file under $STATE_ROOT/dux/.tiocsti-state when the kernel doesn't
# support `--inject-mode raw`. Each wrapper reads this sentinel at
# startup and switches `amq wake` to bridge mode (see Phase 13 plan).
#
# The sentinel content is informational ("tiocsti_disabled"); only its
# *presence* is consulted at runtime. A separate flag file rather than
# a config-toml entry keeps this concern out of dux's config schema —
# this is purely about the message-bus TTY transport, not a user
# preference.
TIOCSTI_FLAG="$STATE_ROOT/dux/.tiocsti-state"
if tiocsti_status; then
  # Kernel supports it AND it's enabled — clear any stale sentinel from
  # a previous install on a different (locked-down) kernel.
  rm -f "$TIOCSTI_FLAG"
  ok "kernel: dev.tty.legacy_tiocsti=1 — amq wake will use TIOCSTI"
else
  case $? in
    1)
      warn "kernel: dev.tty.legacy_tiocsti=0 — amq wake injection will not work."
      warn "  Either run 'sudo sysctl -w dev.tty.legacy_tiocsti=1' (if your kernel"
      warn "  supports it) or rely on the inject bridge (default since Phase 13)."
      ;;
    2)
      warn "kernel: TIOCSTI not present (CONFIG_LEGACY_TIOCSTI=n)."
      warn "  Switching to --inject-via bridge mode for amq wake; runtime sysctl"
      warn "  toggle won't help — the option is compiled out of this kernel."
      ;;
  esac
  printf 'tiocsti_disabled\n' > "$TIOCSTI_FLAG"
  ok "wrote $TIOCSTI_FLAG (wrappers will use --inject-via bridge)"
fi

# 2. dux ---------------------------------------------------------------------
if ! command -v dux >/dev/null 2>&1; then
  say "installing dux $DUX_TAG"
  TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
  curl -fsSL -o "$TMP/dux.tar.gz" \
    "https://github.com/patrickdappollonio/dux/releases/download/${DUX_TAG}/dux-linux-amd64.tar.gz"
  verify_sha256 "$TMP/dux.tar.gz" "$DUX_SHA256" "dux ${DUX_TAG}"
  tar -xzf "$TMP/dux.tar.gz" -C "$TMP"
  install -m 0755 "$TMP/dux" "$LOCAL_BIN/dux"
  rm -rf "$TMP"; trap - EXIT
fi
ok "dux: $(dux --help 2>&1 | head -1 || echo installed)"

# 3. AMQ ---------------------------------------------------------------------
# Bypass the upstream `curl … | bash` install script entirely: download the
# pinned release tarball, verify sha256, install the binary directly. The
# upstream installer's behavior (paths, side effects) is then irrelevant to
# our trust boundary. Install log is teed to $STATE_ROOT/amq/install.log so
# stderr is never silenced.
if ! command -v amq >/dev/null 2>&1; then
  say "installing amq $AMQ_TAG"
  AMQ_LOG="$STATE_ROOT/amq/install.log"
  : > "$AMQ_LOG"
  TMP=$(mktemp -d); trap 'rm -rf "$TMP"' EXIT
  {
    echo "[$(date -u +%FT%TZ)] downloading amq ${AMQ_TAG}"
    curl -fsSL -o "$TMP/amq.tar.gz" \
      "https://github.com/avivsinai/agent-message-queue/releases/download/${AMQ_TAG}/amq_${AMQ_VERSION}_linux_amd64.tar.gz"
    verify_sha256 "$TMP/amq.tar.gz" "$AMQ_SHA256" "amq ${AMQ_TAG}"
    tar -xzf "$TMP/amq.tar.gz" -C "$TMP"
    install -m 0755 "$TMP/amq" "$LOCAL_BIN/amq"
    echo "[$(date -u +%FT%TZ)] amq installed to $LOCAL_BIN/amq"
  } 2>&1 | tee -a "$AMQ_LOG"
  rm -rf "$TMP"; trap - EXIT
fi
# Audit02 P0-F: don't wipe queue config on re-install. AMQ writes its
# state under $STATE_ROOT/amq; the presence of `meta/config.json` (the
# file `amq init --force` overwrites — confirmed via `amq init --help`
# against pinned v0.34.0) tells us init has already run. Probing a fresh
# `amq init` shows the layout is `meta/config.json`, `agents/<handle>/`,
# `threads/` — *not* a top-level `agents.json` as earlier audit notes
# assumed.
AMQ_INIT_MARKER="$STATE_ROOT/amq/meta/config.json"
if [[ ! -f "$AMQ_INIT_MARKER" ]]; then
  amq init --root "$STATE_ROOT/amq" --agents claude,codex,gemini --force >/dev/null
  ok "amq queue initialized at $STATE_ROOT/amq"
else
  ok "amq queue already initialized at $STATE_ROOT/amq (skipping init)"
fi
chmod 700 "$STATE_ROOT/amq"

# Audit01 P1-8: pin amq at a controlled absolute path under $STATE_ROOT and
# record its sha256, so the bashrc guard (in bashrc-additions.sh) can refuse
# to source `amq shell-setup` if the binary on disk no longer matches.
# Without this guard, every interactive shell start would `eval` whatever the
# `amq` binary in PATH chose to print — a much larger trust radius than the
# install-time pin we just verified above.
#
# Before pinning, verify the binary about to be copied matches AMQ_BINARY_SHA256
# (cross-checked against the extracted tarball). This catches the case where
# the user already has a tampered `amq` in PATH from an earlier untrusted
# install and the Phase 01 tarball-download branch was skipped.
AMQ_BIN_DIR="$STATE_ROOT/amq-bin"
AMQ_BIN_PINNED="$AMQ_BIN_DIR/amq"
mkdir -p "$AMQ_BIN_DIR"

# Audit02 P1-A: don't trust `command -v amq` here — PATH order is
# unpredictable (a user's own ~/.local/bin/amq from a prior run can
# shadow $LOCAL_BIN, or vice versa). If the install branch above ran,
# $LOCAL_BIN/amq is the binary we just verified against $AMQ_SHA256
# (tarball hash). If the branch was skipped (binary already present),
# we still hash-check whatever's on PATH before pinning.
AMQ_FRESH_INSTALL_BIN="$LOCAL_BIN/amq"
if [[ -x "$AMQ_FRESH_INSTALL_BIN" ]]; then
  AMQ_BIN_SOURCE="$AMQ_FRESH_INSTALL_BIN"
else
  AMQ_BIN_SOURCE="$(command -v amq || true)"
  [[ -n "$AMQ_BIN_SOURCE" ]] || { warn "amq not found after install"; exit 1; }
fi
verify_sha256 "$AMQ_BIN_SOURCE" "$AMQ_BINARY_SHA256" "amq binary at $AMQ_BIN_SOURCE"
install -m 0755 "$AMQ_BIN_SOURCE" "$AMQ_BIN_PINNED"
# Audit02 P1-E prep: harden binary.sha256 to read-only (0444). Re-runs
# of install.sh need to overwrite this file, but on a writable parent
# dir the redirect succeeds; on read-only mounts we restore u+w first.
chmod u+w "$STATE_ROOT/amq/binary.sha256" 2>/dev/null || true
sha256sum "$AMQ_BIN_PINNED" > "$STATE_ROOT/amq/binary.sha256"
chmod 0444 "$STATE_ROOT/amq/binary.sha256"
ok "amq binary pinned at $AMQ_BIN_PINNED ($(awk '{print $1}' "$STATE_ROOT/amq/binary.sha256"))"

# 4. AMQ skills (gives Claude/etc. native knowledge of amq) ------------------
# Pin the npm package version, pin the skills-source git ref, block postinstall
# scripts (--ignore-scripts), and tee the full output to a log. Failure is
# non-fatal — the AMQ binary alone is enough to operate.
if command -v npx >/dev/null 2>&1; then
  SKILLS_LOG="$STATE_ROOT/amq/skills-install.log"
  : > "$SKILLS_LOG"
  npx --yes --ignore-scripts "skills@${SKILLS_PIN}" add \
    "avivsinai/agent-message-queue#${SKILLS_REV}" -g -y \
    2>&1 | tee -a "$SKILLS_LOG" || \
    warn "npx skills add failed; see $SKILLS_LOG"
fi

# 5. install wrappers --------------------------------------------------------
say "installing wrappers to $LOCAL_BIN"
install -m 0755 "$HERE/wrappers/claude-amq"  "$LOCAL_BIN/claude-amq"
install -m 0755 "$HERE/wrappers/codex-amq"   "$LOCAL_BIN/codex-amq"
install -m 0755 "$HERE/wrappers/gemini-amq"  "$LOCAL_BIN/gemini-amq"
# The encoder is the single source of truth for Claude Code's on-disk
# project-dir naming (audit01 P0-5, audit02 Phase 12). It must be on
# $PATH so claude-amq's seed step (and Phase 10's purge job) can find it.
install -m 0755 "$HERE/scripts/encode-claude-project-dir" "$LOCAL_BIN/encode-claude-project-dir"
install -m 0755 "$HERE/scripts/finalize-claude-migration.sh" "$STATE_ROOT/scripts/finalize-claude-migration.sh"

# Audit02 P0-K (T2): HMAC envelope tooling. Install the signing helper
# and the verifier alongside the wrappers — both must be on $PATH so:
#   * scripts and skills can call `amq-send-signed` instead of `amq send`
#   * `amq wake --inject-via amq-receive-verify` (in claude-amq /
#     codex-amq / gemini-amq) can find the verifier without an absolute
#     path. The wrappers do not export PATH explicitly; `amq` resolves
#     `--inject-via` against the calling process's $PATH.
install -m 0755 "$HERE/scripts/amq-secret-init.sh" "$LOCAL_BIN/amq-secret-init.sh"
install -m 0755 "$HERE/scripts/amq-send-signed"    "$LOCAL_BIN/amq-send-signed"
install -m 0755 "$HERE/scripts/amq-receive-verify" "$LOCAL_BIN/amq-receive-verify"

# Audit02 Phase 13: TIOCSTI fallback bridge. Wrappers point
# `amq wake --inject-via "$LOCAL_BIN/dux-amq-inject-bridge"` when
# `.tiocsti-state` is present. The bridge runs verify internally, then
# either sends keys to the current tmux pane or queues to disk. Always
# install it so the runtime mode (via DUX_AMQ_INJECT_MODE=via) works
# even on TIOCSTI-OK kernels for forced-fallback testing.
install -m 0755 "$HERE/scripts/dux-amq-inject-bridge" "$LOCAL_BIN/dux-amq-inject-bridge"

# Generate the per-VM HMAC secret (idempotent; preserves an existing
# secret so signed messages already in flight stay verifiable). Run
# *after* the AMQ binary is in place so a failure here implicates only
# the new auth layer, not the queue itself.
"$HERE/scripts/amq-secret-init.sh"

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
# Audit01 P1-7: delete-then-rewrite (the pyenv/sdkman pattern). On every
# install we strip any prior `# >>> dux-amq vN.M.K >>>` block AND the legacy
# unversioned `# === dux + AMQ ===` block, then append the current version.
# That way version bumps actually propagate instead of being no-ops.
say "rewriting ~/.bashrc dux-amq stanza (v$DUX_AMQ_VERSION)"
touch "$HOME/.bashrc"
strip_block "$HOME/.bashrc" sh
sed "s|REPLACE_AT_INSTALL|$DUX_AMQ_VERSION|g" "$HERE/config/bashrc-additions.sh" >> "$HOME/.bashrc"

# 8. global CLAUDE.md --------------------------------------------------------
mkdir -p "$HOME/.claude"
touch "$HOME/.claude/CLAUDE.md"
say "rewriting ~/.claude/CLAUDE.md dux-amq stanza (v$DUX_AMQ_VERSION)"
# Audit02 P0-G: snapshot-then-diff guardrail. Before rewriting CLAUDE.md
# (a user-owned doc), copy the original to $STATE_ROOT/dux/claude-md.<ts>.bak
# so an operator can recover if `strip_block` ever does the wrong thing.
if [[ -s "$HOME/.claude/CLAUDE.md" ]]; then
  install -m 0644 "$HOME/.claude/CLAUDE.md" \
    "$STATE_ROOT/dux/claude-md.$(date +%s).bak"
fi
strip_block "$HOME/.claude/CLAUDE.md" md
{
  printf '\n<!-- >>> dux-amq v%s >>> -->\n\n' "$DUX_AMQ_VERSION"
  cat "$HERE/config/claude-md-additions.md"
  printf '\n<!-- <<< dux-amq v%s <<< -->\n' "$DUX_AMQ_VERSION"
} >> "$HOME/.claude/CLAUDE.md"

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
