# dux-amq overlay

Setup scripts that wire **dux** (the worktree TUI from `patrickdappollonio/dux`) together with **AMQ** (file-based agent-to-agent messaging from `avivsinai/agent-message-queue`) on a Linux VM with a persistent disk.

This directory does **not** modify dux source. It sits alongside the dux Rust source in this fork so I can keep both pieces under one fork while still pulling upstream.

## What you get

- **Worktree-per-agent UI** (dux) for parallel Claude/Codex/Gemini sessions
- **File-based message bus** (AMQ) so agents on the same VM can `send`/`list`/`read` between each other
- **Automatic identity**: each dux pane's AMQ handle is its git branch name, lowercased + sanitized
- **Spot-VM survival**: dux config + sessions, AMQ queue, and Claude session JSONLs all live on a persistent disk (default `/data/state/`)
- **Past-chat resume** in fresh worktrees via `--continue --fork-session` (bypasses deferred-tool blocks)
- **YOLO is opt-in** (audit02 P0-A): `CLAUDE_AMQ_YOLO=1` / `CODEX_AMQ_YOLO=1` enable the per-pane `--dangerously-*` flag. See [Permission model](#permission-model).

## Layout

```
dux-amq/
├── install.sh                         # one-shot installer (idempotent)
├── wrappers/
│   ├── claude-amq                     # wraps `claude` with AMQ co-op + history seed
│   ├── codex-amq                      # wraps `codex` with AMQ co-op
│   └── gemini-amq                     # wraps `gemini` with AMQ co-op
├── scripts/
│   └── finalize-claude-migration.sh   # moves ~/.claude + ~/.agents onto /data
├── config/
│   ├── bashrc-additions.sh            # env vars + amq shell-setup eval
│   ├── claude-md-additions.md         # global CLAUDE.md fragment teaching AMQ usage
│   └── dux-config-changes.toml        # dux config diff to apply post-first-launch
└── vscode/
    └── settings-additions.json        # VSCode Remote-SSH terminal Ctrl-G fix
```

## Quickstart

Prerequisites:
- Linux VM with a persistent disk mounted at `/data` (tested on GCE)
- `claude` CLI on PATH (Anthropic Claude Code)
- `git`, `curl`, `tar`, `rsync`, `npx`
- `sudo` access (only for the persistent-disk migration step)

Install:
```bash
git clone https://github.com/SiavZ/dux-amq-setup.git
cd dux-amq-setup/dux-amq
./install.sh
exec bash -l
```

Optional one-time migration of an existing `~/.claude` onto `/data` (run **after** closing every running `claude` process):
```bash
/data/state/scripts/finalize-claude-migration.sh
```

Launch:
```bash
dux
```

YOLO mode for that session (legacy `CLAUDE_YOLO=1` still works for both panes):
```bash
CLAUDE_AMQ_YOLO=1 CODEX_AMQ_YOLO=1 dux
```

## Permission model

YOLO is **opt-in** as of audit02 phase 01. The wrappers default-deny on tool
execution; you must explicitly export an env var per pane to bypass prompts.
The Anthropic 2025–26 CVE wave (CVE-2025-59536, CVE-2026-21852,
CVE-2026-25723, CVE-2026-33068, CVE-2026-35020/35021/35022) all exploited
credential exfil through prompt-injected paths — default-deny is the single
biggest mitigation.

| Pane     | Env var to enable YOLO              | What it does                                         |
|----------|-------------------------------------|------------------------------------------------------|
| claude   | `CLAUDE_AMQ_YOLO=1`                 | passes `--dangerously-skip-permissions`              |
| codex    | `CODEX_AMQ_YOLO=1`                  | passes `--dangerously-bypass-approvals-and-sandbox`  |
| (legacy) | `CLAUDE_YOLO=1`                     | enables BOTH for backwards compat                    |

When YOLO is active, the wrapper prints a one-line stderr banner so it's
visible in the dux pane header. If you previously exported the deprecated
`CLAUDE_AMQ_SAFE=1` opt-out, the wrapper now prints a transitional warning;
the variable is otherwise ignored — unset it from your shell rc.

## Session seeding

Cloning the parent worktree's Claude session JSONLs into a fresh worktree
is **opt-in** (audit02 phase 01). Set `CLAUDE_AMQ_SEED_FROM_PARENT=1` to
enable.

Trade-offs to weigh before turning it on:

- **Disk amplification**: rsync clones the parent's full Claude history.
  ~100 MB per worktree on heavy repos; multiplies by N worktrees.
- **Token billing**: a long inherited history pushes new sessions toward
  the 1M-context tier earlier than a clean start would.
- **Cross-worktree info leak**: the parent's transcripts may carry secrets
  or PII from a different feature; seeding makes them readable from the
  new pane.

If you enable seeding, pair it with `resume_args = ["--resume"]` in dux
config so the picker actually shows the seeded chats. Avoid combining with
`--continue`: the latest parent session may carry a deferred-tool marker
that `--continue` refuses.

## Architecture sketch

```
┌─────────────────────────── dux (TUI on persistent disk) ───────────────────────────┐
│                                                                                    │
│  ┌──────────── pane 1: alice ───────────┐    ┌──────────── pane 2: bob ─────────┐ │
│  │   claude-amq (wrapper)               │    │   claude-amq (wrapper)           │ │
│  │     ↳ amq coop exec --me alice ─────────────┐                                │ │
│  │       ↳ claude --continue --fork-session    │   ↳ same                       │ │
│  └──────────────────────────────────────┘   ┌─┘                                  │ │
│                                             │                                    │ │
└───────────────── /data/state/amq (file-based queue) ───────────────────────────────┘
                          │
              alice's mailbox  ←→  bob's mailbox  (Maildir-style)
```

- dux creates a git worktree per pane; each pane gets its own CWD and Claude session storage.
- The `claude-amq` wrapper sets `AM_ME = <branch>`, ensures `--no-init`, and uses the shared `AMQ_GLOBAL_ROOT` queue.
- `--continue --fork-session` lets a worktree pick up the parent repo's most-recent chat as context, forking off cleanly so deferred-tool markers don't block resume.
- All inter-pane communication is `amq send <peer> "..."` from the agent — no MCP, no daemon, just files on disk.

## Kernel compatibility (TIOCSTI)

`amq wake` injects messages into agent panes via `ioctl(TIOCSTI)` by default. Linux 6.2 (Nov 2022) made `CONFIG_LEGACY_TIOCSTI` default-off, and Ubuntu 24.04 LTS / Debian 12+ ship the option built out entirely. On those kernels, `--inject-mode raw` silently fails — every wake notification is dropped before reaching the agent.

`install.sh` detects the kernel state by reading `/proc/sys/dev/tty/legacy_tiocsti`:

| Procfs value | Meaning                                | Sentinel `$STATE_ROOT/dux/.tiocsti-state` |
|--------------|----------------------------------------|-------------------------------------------|
| `1`          | TIOCSTI compiled in AND enabled        | absent (raw mode)                         |
| `0`          | Compiled in but disabled at runtime    | present (via mode)                        |
| (file absent)| Compiled out — no runtime toggle helps | present (via mode)                        |

When the sentinel is present, the wrappers switch `amq wake` to `--inject-via "$LOCAL_BIN/dux-amq-inject-bridge"`. The bridge:

1. Validates the envelope (HMAC + freshness + replay) via `amq-receive-verify`.
2. On success, delivers the body either by `tmux send-keys` (when `$TMUX` is set) or by writing to `~/.local/share/dux-amq/inject-queue/<ts>.msg` for later pickup.

Override the auto-detected mode per-pane:

```bash
DUX_AMQ_INJECT_MODE=raw    # force TIOCSTI even on locked-down kernels
DUX_AMQ_INJECT_MODE=via    # force bridge mode (e.g. for testing)
DUX_TMUX_TARGET=<pane>     # specific tmux target for the bridge (default: current pane)
```

Inspect at runtime: `cat $STATE_ROOT/dux/.tiocsti-state` (absent → raw mode active). Wake stderr lands in `~/.local/share/dux-amq/wake-<me>.log` — verify-drop reasons are visible there.

A native upstream fix (HMAC envelope + stdin piping inside AMQ itself) is tracked in `docs/plans/audits/audit02/artifacts/13-upstream-issue.txt`.

## Trade-offs

- **No native dux hook** for worktree-create lifecycle, so seeding past-chat history (when enabled) is done in the wrapper (one-shot, on first launch).
- **Seeded worktrees get their own snapshot** of past sessions on first launch (~100 MB for a heavy repo). They diverge afterward — by design. See [Session seeding](#session-seeding) for the disk/billing/leak trade-offs.
- **Identity collisions are possible** if two worktrees normalize to the same handle. Pick distinct branch names.
- **Compaction risk** (when seeding is enabled): on repos with a heavy session history, `--fork-session` inherits all of it, which can push fresh sessions toward 1M-context billing tier earlier. If that bites, leave `CLAUDE_AMQ_SEED_FROM_PARENT` unset (the default) or revert `resume_args` to `["--continue"]`.

## Diagnostics

`dux-amq doctor` (audit02 phase 20) prints a self-check dump that
operators can attach to a support thread. Same output is reachable
via `dux doctor`, which adds a Rust-side `sessions.sqlite3` integrity
section on top of the bash-script bundle.

```bash
dux-amq doctor              # human-readable
dux-amq doctor --json       # machine-parseable; pipe to `jq`
dux-amq doctor --anonymize  # redact $HOME, branch names, agent IDs
```

It reports: AMQ binary integrity (sha256 vs the pin in
`bashrc-additions.sh`), kernel `dev.tty.legacy_tiocsti` value,
`sessions.sqlite3` `PRAGMA integrity_check`, encryption posture of
`$STATE_ROOT`, AMQ queue depth and oldest-message age, the
`~/.claude` symlink target, free disk space, and the currently-running
dux PID/uptime/RSS. Each external call is wrapped in `timeout 5`, so
the tool never hangs even when the underlying piece is broken.

The HMAC envelope (audit02 phase 08) lives at the path pointed to by
`AMQ_SECRET_PATH` (default `$HOME/.local/share/dux-amq/amq-secret`,
mode 0600). To rotate, `rm` the file and restart every pane —
rotation invalidates every in-flight signed envelope by design.
`dux-amq/scripts/amq-secret-init.sh` regenerates it idempotently on
the next install.

## Production setup

The default deployment relies on the cloud provider's at-rest
encryption (GCE PD, EBS, Azure Disk). That covers physical-disk
theft but **not** a compromised cloud IAM principal who can attach
the persistent disk to another VM and read agent transcripts /
queues in plaintext.

For stronger isolation, see the operator playbook at
[`docs/operations/encryption-at-rest.md`](../docs/operations/encryption-at-rest.md).
It covers two paths:

- **gocryptfs** — file-level FUSE encryption, no reformat, ~5% IO
  overhead. Recommended for single-user spot VMs. An opt-in helper
  is shipped at [`scripts/install-gocryptfs.sh`](scripts/install-gocryptfs.sh)
  and is **not** invoked by `install.sh`.
- **LUKS** — block-level, requires reformatting the persistent
  disk. Recommended for long-lived shared hosts.

Either path is layered on top of, not in place of, the cloud
default. See also `SECURITY.md` for the broader threat model.

## License

The wrappers and scripts in this directory are MIT-licensed (matching the dux license in the parent repo).
