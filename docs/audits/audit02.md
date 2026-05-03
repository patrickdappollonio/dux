# dux-amq-setup audit02 (2026-05-03)

Scope: branch `audit01/install-chain` @ `554255d` of `SiavZ/dux-amq-setup`. Goal: production-readiness sweep covering correctness, efficiency, cybersecurity, structure, and modernity. Builds on `audit01.md`; verifies which audit01 items the install-chain branch closed and goes beyond audit01's 4-patched-files Rust scope into the full `src/` tree, full CI/CD pipeline, and an explicit threat model.

Methodology: 4 parallel orchestrator agents — Shell/install/overlay (A), Full Rust src/ (B), CI/CD + supply chain (C), Architecture + threat model + observability (D) — each with 3 parallel research sub-agents pulling current sources (RustSec, GitHub Actions advisories May 2026, OSC/ANSI CVEs, TIOCSTI status, Anthropic + OpenAI agent CVEs 2025–26, GDPR Art 32). Every finding cites a file:line that an agent actually read.

## Executive summary

- **Posture**: install-chain has closed 4 of audit01's 24 items (P0-2, P1-6, P1-7, P1-8) cleanly. **The remaining audit01 P0s are still open** (P0-1 YOLO defaults, P0-3 inverted seed, P0-4 migration TOCTOU, P0-5 path encoding). audit02 adds **9 new P0s** the prior audit missed — log injection from raw bytes into `dux.log` and the status line, blocking git calls on the UI thread, terminal-mutex panics, default-tag (not SHA) GitHub Actions, missing SBOM/signing/`cargo audit`/`cargo deny`, no GDPR-grade purge command, no auth between AMQ peers, `amq init --force` wiping queue config on re-install, and an awk strip rule that can delete user-appended CLAUDE.md content.
- **Top blockers for production**: see "P0 — must fix before production" below. Combined effort is ~3–5 person-days of code + ~1 week to land cosign/SBOM/CODEOWNERS plumbing. None require redesign of dux itself.
- **Strengths**: `c19ab4e` and `554255d` are well-executed (sha256-pinned downloads, hash-guarded `eval`, versioned markers). Dependency tree has **zero open RustSec advisories** as of 2026-05. The PTY snapshot path drops control bytes correctly — host-terminal injection via the live render is **not** exposed.
- **Recommended overhauls** (the user explicitly invited large changes): (1) decompose the `App` god-object (120 fields in `src/app/mod.rs`); (2) introduce an explicit session state machine; (3) replace handwritten `logger.rs` with `tracing` + `tracing-appender::rolling` + JSON layer; (4) ship a `dux-amq doctor` triage command before anything else (per phase-16 plan).

---

## Audit01 status

| audit01 ID | Status | Evidence |
|---|---|---|
| **P0-1** Default YOLO for Claude+Codex | OPEN | `dux-amq/wrappers/claude-amq:83-85` (opt-out via `CLAUDE_AMQ_SAFE`), `codex-amq:27` (no flag at all). Untouched by the 5 commits. |
| **P0-2** No supply-chain verification on 3 install steps | CLOSED | `c19ab4e` — `dux-amq/install.sh:28-34, 117-122, 139-146, 180-184`. Hashes are TOFU-pinned in-tree (good but not sigstore). |
| **P0-3** Inverted seed default | OPEN — file is now self-contradicting | `claude-amq:11` says "OFF by default", line 26 says "On by default". README:92 still references `CLAUDE_AMQ_NO_SEED`. |
| **P0-4** Migration TOCTOU / `rsync --delete` | OPEN | `dux-amq/scripts/finalize-claude-migration.sh:26, 28-30, 35-39`. No `flock`, no `mv -T`, no per-step re-check. |
| **P0-5** Path-encoding mismatch + prefix-glob worktree check | OPEN | `claude-amq:38-39` still uses `sed`; prefix glob at `:67`, `codex-amq:12`, `gemini-amq:14`. |
| **P1-1** TIOCSTI / AMQ wake | OPEN — confirmed worse than audit01 estimated | AMQ source at `internal/cli/wake_tiocsti_unix.go` calls `unix.Syscall(SYS_IOCTL, fd, unix.TIOCSTI, ...)` with no PTY-master fallback. Linux 6.2+ disables CONFIG_LEGACY_TIOCSTI by default; Ubuntu 24.04 ships it disabled. Wake will silently no-op. AMQ has `--inject-via &lt;executable&gt;` escape hatch our wrappers do not use. |
| **P1-2** Background `amq wake` + `set -e` | OPEN | `claude-amq:96`, `codex-amq:25`, `gemini-amq:29`: still `&` + `>/dev/null 2>&1`. |
| **P1-3** Auto-resume thundering herd | OPEN | `src/app/mod.rs:1380-1410` — sequential, unbounded fan-out. |
| **P1-4** Scrollbar math | OPEN | `src/app/render.rs:1352-1368`. |
| **P1-5** Fork drift automation | OPEN | No `.github/workflows/upstream-sync.yml`. |
| **P1-6** `grep -oP` portability | CLOSED | `aeaba87` + `c19ab4e` — preflight requires `jq`; the `grep -oP` scrape was deleted. |
| **P1-7** Versioned `>>>/<<<` markers | CLOSED with regression risk | `61ebf28` — see new finding **N-1** below. |
| **P1-8** `eval "$(amq shell-setup)"` trust | CLOSED with caveats | `554255d` — install-time hash gate at `install.sh:163-171`, runtime guard at `bashrc-additions.sh:17-32`. Caveats N-2, N-3 below. |
| **P2-1..P2-11** | Mostly OPEN; P2-7 partially fixed by `c19ab4e`. |

---

## P0 — must fix before production

### P0-A — Default-on YOLO permissions (audit01 P0-1, unchanged)
- `dux-amq/wrappers/claude-amq:83-85`, `dux-amq/wrappers/codex-amq:27`.
- Anthropic/Claude Code 2025–26 CVEs (CVE-2025-59536, CVE-2026-21852, CVE-2026-25723, CVE-2026-33068, CVE-2026-35020/35021/35022) all targeted credential exfil through prompt-injected paths. `--dangerously-skip-permissions` + `--dangerously-bypass-approvals-and-sandbox` on by default makes every dux pane the worst-case configuration.
- Fix: invert the defaults. Two-line change.
```bash
# claude-amq:83-85
EXTRA=()
[[ "${CLAUDE_AMQ_YOLO:-${CLAUDE_YOLO:-}}" == "1" ]] && EXTRA+=(--dangerously-skip-permissions)
# codex-amq:27 — same pattern, add CODEX_AMQ_YOLO
```

### P0-B — Log injection: `logger.rs` writes raw attacker-controlled bytes
- `src/logger.rs:84-92`. Producers feed unfiltered git stderr (`String::from_utf8_lossy(&output.stderr)` across `src/git.rs`), GitHub PR titles via `gh pr view`, `/proc/<pid>/comm` from `pty.rs:521-525`, and arbitrary user-supplied paths.
- A hostile branch name, PR title, or process name with embedded ANSI/OSC/DCS bytes lands verbatim in `dux.log`. Subsequent `tail dux.log` rewrites the operator's terminal title (OSC 0/2), drops a covering OSC 8 hyperlink, or paste-injects via OSC 52. Same class as Rails CVE-2025-55193.
- Fix: shared `sanitize_for_terminal(s: &str) -> String` that strips `[\x00-\x08\x0b-\x1f\x7f\x1b]`. Route every `logger::*` call through it. ~15 lines.

### P0-C — Status-line/UI strings carry unsanitized stderr
- `src/git.rs` — every `Err(anyhow!(... String::from_utf8_lossy(&output.stderr) ...))` (lines 47-51, 108-111, 130-132, 153-155, 226-231, 277-279, 298-301, 343-344, 383-388, 619-621, 633-635, 656-658, 679-681, 693-695, 708-710, 778-780). Consumed by `set_error/set_info` in `src/app/{workers,sessions,input}.rs`.
- Same sanitizer as P0-B; one source of truth.

### P0-D — Blocking git calls on the main UI thread (CLAUDE.md tenet violation)
- `src/app/sessions.rs:50, 66, 71`; `src/app/mod.rs:1974, 2363, 2389`; `src/app/input.rs:950, 994`; `src/app/sessions.rs:577-582`. Each is a `Command::new("git")` shelling out, blocking the event loop. CLAUDE.md is explicit: even `git symbolic-ref` must use a worker.
- Worst case: `load_projects` at startup loops over every configured project and synchronously runs `is_git_repo` + `current_branch`. N projects = N×fork+wait before the first frame.
- Fix: route through `worker_tx` like `push`/`pull` already do. Add `WorkerEvent::ProjectMetaReady`.

### P0-E — Terminal mutex `.expect()` panics abort the whole TUI
- `src/pty.rs:277, 289, 295` — three `.expect("terminal mutex poisoned")` on the render path. Mutex poisoning leaves the process dead with the lockfile flock'd and the terminal in raw mode.
- Mitigations elsewhere already handle poison gracefully (`pty.rs:413, 460-464` use `if let Ok`). Align the three above.
- Fix: `.lock().ok()` + sentinel snapshot.

### P0-F — `amq init --force` on every re-install wipes queue config
- `dux-amq/install.sh:148`. `amq init --root "$STATE_ROOT/amq" --agents claude,codex,gemini --force` runs unconditionally. Any agent registered out-of-band by the user (additional panes, `bob`/`alice` from the README architecture sketch) is silently removed. Breaks the "idempotent — re-run at will" contract printed at `:3`.
- Fix: gate behind `[[ ! -f "$STATE_ROOT/amq/.amqrc" ]]` (or whatever AMQ's init marker is); on re-install, log `ok "amq queue exists, skipping init"`.

### P0-G — `strip_block` for legacy CLAUDE.md never resets `s=0` → silently deletes user content
- `dux-amq/install.sh:84-90`. The legacy migration rule sets `s=1` on `## Multi-agent environment (AMQ + dux)` and never re-disables it. Anything appended below that heading by the user is permanently lost on first re-install.
- Fix: pair the strip rule with a closing match (next blank-line + `^# `/`^## ` heading), OR snapshot+diff before overwrite, OR write an explicit `^---$` end-of-block sentinel during the legacy era and only strip up to it.

### P0-H — GitHub Actions pinned by tag, not SHA (tj-actions / trivy-action class)
- `release.yml:32` (`actions/checkout@v4`), `:38` (`dtolnay/rust-toolchain@stable`), `release.yml:42`, `pr.yml:34`, `test.yml:14` (`Swatinem/rust-cache@v2`).
- `overlay-ci.yml:21` already pins by SHA — propagate that pattern. Tooling: `pinact` or `frizbee` autogenerates SHA pins from a manifest.
- Risk: cache poisoning + arbitrary-code execution if any tag is force-pushed. Same incident class as tj-actions/changed-files (Mar 2025) and trivy-action (Mar 2026).

### P0-I — No SBOM, no signing, no `cargo audit`/`cargo deny` in CI
- `release.yml:56-62` ships binaries with no provenance, no `.sha256` (the `dux-amq/install.sh:29` pin is hand-derived per release), no SLSA attestation, no cosign signature.
- `pr.yml`, `test.yml` run only fmt/clippy/cargo-test. Zero `cargo audit`, zero `cargo deny`, zero `shellcheck` on `install.sh` (top-level), zero macOS in matrix despite shipping macOS binaries.
- Fix bundle (cost ≈ 1 day):
  1. Add `cargo install --locked cargo-audit cargo-deny`, run `cargo audit --deny warnings` and `cargo deny check` in PR CI.
  2. Add `actions/attest-build-provenance@<sha>` to `release.yml` (free, OIDC-keyless, SLSA L3-eligible).
  3. Add `cargo install --locked cargo-auditable` and replace `cargo build --release` with `cargo auditable build --release`. Embeds the dep tree in the binary so `cargo audit bin`/trivy/grype work post-release.
  4. Add `cargo cyclonedx --format json` → upload `sbom.cdx.json`.
  5. `sha256sum *.tar.gz > SHA256SUMS` upload — closes the loop with `dux-amq/install.sh:29`.
  6. Add `rust-toolchain.toml` so `dtolnay/rust-toolchain@<sha>` is reproducible.
  7. Add `dependabot.yml` for `cargo` and `github-actions`.
  8. Add `cargo deny.toml` with `[advisories] yanked = "deny"`, `[bans] multiple-versions = "warn"`, `[licenses]` allowlist, `[sources] unknown-registry = "deny"`.

### P0-J — No "purge this session and all its bytes" command (GDPR Art 17 gap)
- `src/cli.rs:464` (`reset_agent_data`) removes worktrees + sqlite + log. It does **not** touch `~/.claude/projects/<encoded>/*.jsonl` (now under `/data/state/claude/`), `/data/state/{codex,gemini}/`, or per-pane AMQ inboxes.
- Right-to-erasure today is impossible — every prompt/response with potential PII survives any "delete agent" action.
- Fix: `dux session purge --hard <id>` that cascades to claude/codex/gemini JSONLs + `/data/state/amq/<branch>/` + sqlite + worktree + dux.log lines tagged with that session_id (requires structured logging — see overhaul §1).

### P0-K — AMQ has no auth between panes; spoofing `--me <other>` lets any pane impersonate any other
- `dux-amq/wrappers/claude-amq:96`, `codex-amq:25`, `gemini-amq:29` — `amq wake … --inject-mode raw` auto-submits whatever lands in the inbox. Combined with no sender-verification in the AMQ protocol, any pane can `amq send --me alice "rm -rf $HOME"` and the receiver auto-types it.
- Threat: one compromised pane (via P0-A YOLO + prompt injection) trivially compromises every other pane.
- Fix: HMAC-signed messages with a per-VM secret in `$STATE_ROOT/amq/secret`, mode 0600. Verify on receive; drop unsigned messages. Coordinate with AMQ upstream — file an issue. Defense in depth: `--inject-mode raw` should be `--inject-mode confirm` for messages from senders not on an allowlist.

---

## P1 — should fix soon

| # | File:line | Issue | Fix |
|---|---|---|---|
| **P1-A** | `dux-amq/install.sh:165-168` | `command -v amq` returns earliest-on-PATH; if user has `~/go/bin/amq` ahead of `~/.local/bin`, the hash check pins the wrong binary | Verify `$LOCAL_BIN/amq` directly when the install branch ran; second hash-check inside the install branch's `{}` block before mv |
| **P1-B** | `claude-amq:96`, `codex-amq:25`, `gemini-amq:29` | Background `amq wake &` orphaned by `exec`; dies on TTY hangup; no PID file | `setsid amq wake … & disown`; write PID to `$STATE_ROOT/amq/wake-$ME.pid` |
| **P1-C** | `dux-amq/install.sh:103-108` | Preflight bails on first missing tool; users hit each one separately | Accumulate a `missing=()` array, fail once with full list |
| **P1-D** | `claude-amq:50-56` | `rsync … 2>/dev/null \|\| true` discards seed errors; reports misleading count | Capture exit; if non-zero, log "partial seed (N copied, errors at $LOG)" |
| **P1-E** | `bashrc-additions.sh:17-32` | Hash guard fails open if `binary.sha256` record is missing — attacker who deletes the record disables the guard | Refuse to no-op when `$AMQ_BIN` exists but record doesn't; install should `chmod 0444` and own-by-root if possible |
| **P1-F** | `claude-amq:75`, `codex-amq:19` | Identity normalization (`sed` `[^a-z0-9_-]` → `-`) silently collides `feat/foo` and `feat-foo` to the same handle | Detect collision in wrapper, bail with explicit error |
| **P1-G** | `src/pty.rs:509-513` | PtyClient::Drop kills child but doesn't join the spawned reader thread → fd + memory leak per session create/delete | Store JoinHandle, join in Drop |
| **P1-H** | `src/pty.rs:496` | `unsafe BorrowedFd::borrow_raw` SAFETY relies on `master: Box<dyn MasterPty>` field marked `#[allow(dead_code)]` — future cleanup could break it | Add `// keeps the fd alive — do not remove` doc-comment + debug_assert tcgetpgrp parent matches child |
| **P1-I** | `src/git.rs:206-225, 261-273, 765-783, 144-159, 165-200` | `git -C path branch <user-name>` etc. lack `--` separator — defense-in-depth gap if branch names ever bypass `is_valid_agent_name` | Insert `"--"` before all user-controlled positional args |
| **P1-J** | `src/provider.rs:49-66` | If `read_to_string` fails, tempfile in `/tmp` containing prompt body (= staged diff = source code) is never deleted | Use `tempfile::NamedTempFile` with mode 0600 + defer-style unlink |
| **P1-K** | `src/storage.rs:431-443` (`ensure_column`) | `format!`-built DDL — currently safe (literal callers) but fragile pattern | Allowlist `[A-Za-z_][A-Za-z0-9_]*` for table/column/sql_type; bail on mismatch |
| **P1-L** | `src/config.rs:2775, 2778, 2783, 2786, 2812, 2814` | Rust 2024 made `set_var/remove_var` unsafe due to thread races; tests run in parallel by default → flaky env-var tests | `serial_test` crate or per-test `Mutex<()>` |
| **P1-M** | `src/git.rs:786` (`petname::petname(...).expect`) | Aborts UI on any future petname change | Fall back to `agent-{uuid}` |
| **P1-N** | `release.yml:42`, `pr.yml:34`, `test.yml:14` | `Swatinem/rust-cache@v2` cache-poisoning surface from PR forks | Pin SHA + `save-if: ${{ github.event_name == 'push' && github.ref == 'refs/heads/main' }}` |
| **P1-O** | `release.yml:7` | Workflow-level `permissions: contents: write` inherited by every job | Move to job-level; explicit `id-token: write` only on signing job |
| **P1-P** | `release.yml:46-47` | `cargo install cargo-edit --locked` runs unpinned every release | `cargo install cargo-edit@<ver> --locked` or `taiki-e/install-action@<sha>` |
| **P1-Q** | `release.yml:49-53` | `cargo set-version` mutates Cargo.toml/lock during build → release binary doesn't match committed lockfile | Bump version on `main` *before* tagging; capture modified Cargo.lock as release asset |
| **P1-R** | `pr.yml`, `test.yml` (whole files) | No `permissions:` block — fork PRs get default scope | Add explicit `permissions: { contents: read }` |
| **P1-S** | `pr.yml`, `test.yml` matrix | `runs-on: ubuntu-latest` only; release.yml ships macOS but CI never tests it | Add `matrix: { os: [ubuntu-latest, macos-latest] }` |
| **P1-T** | `overlay-ci.yml:29` | shellcheck doesn't lint top-level `install.sh` or `bashrc-additions.sh` (the file with the runtime `eval`) | Add both to the shellcheck command |
| **P1-U** | `src/app/mod.rs:1380-1410` | `auto_resume_all_sessions` sequential unbounded → 50 sessions = 50 simultaneous PTY+TLS handshakes → API rate-limit + OOM on spot reboot | Bounded semaphore, default cap 4, `auto_resume_concurrency` config field; skip sessions with worktree mtime > N days |
| **P1-V** | `src/app/mod.rs:54-201` | `App` struct has 120 `pub(crate)` fields — peer-mutable god object | Decompose into `UiState` / `RuntimeState` / `GitState` / `RemoteState` sub-structs (see overhaul §1) |
| **P1-W** | `src/storage.rs:22` | sqlite opens with default rollback journal, no `PRAGMA journal_mode=WAL`, no `integrity_check` on open. Spot-VM preempt mid-write → operator sees a session row attached to project `""` (`storage.rs:209`) | `PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA integrity_check;` on open. Periodic `.backup` to `sessions.sqlite3.bak` |
| **P1-X** | `src/logger.rs` (whole) | Manual `Mutex<File>` writer; no rotation; unstructured strings; no correlation IDs | Replace with `tracing` + `tracing-subscriber` JSON layer + `tracing-appender::rolling::daily`. Unblocks P0-J (per-session purge needs structured fields) |
| **P1-Y** | `src/cli.rs`, `src/config.rs`, `src/storage.rs` | No schema versioning. `ensure_column` adds nullable columns ad-hoc forever. `Config` has no `schema_version` field | `PRAGMA user_version` + `migrations` table; `Config { schema_version: u32, … }` with explicit migration step. Document backwards-compat policy |
| **P1-Z** | `src/model.rs:62` (`SessionStatus`) | No state machine. Illegal states representable: `Exited` session with running PTY in `providers: HashMap`; `Active` session whose `child.try_wait()` returned `Some(_)` | `enum SessionState { Created, Spawning, Live { pty }, Detached { last_seen }, Exited { code } }` — all transitions through one function |
| **P1-AA** | (all wrappers) | No PTY count cap, no per-pane memory cap. 100 panes × ~1 MB grid + ~100 MB chat process = OOM in <1 min | `[limits]` config: `max_panes`, `max_companion_terminals`, `max_total_scrollback_mb`. Refuse new agents at 95% disk |
| **P1-BB** | `dux-amq/install.sh:40, :167-169` | AMQ binary sha is a Go build artifact (not reproducible); upstream re-publishing under same tag fails closed but no upgrade path; no cosign verify | Document a rotation policy; commit upstream-published `checksums.txt` line alongside our pin so any rotation is auditable |
| **P1-CC** | top-level `install.sh` | Top-level dux installer ships in `release.yml:77` with **zero verification surface** — users curl-pipe-bash | Host signed copy alongside release; document `cosign verify-blob` one-liner |

---

## P2 — improvements / hygiene

- **P2-1**: `dux-amq/install.sh:84-90` legacy strip rule for `.bashrc` block uses `v[^ ]+` regex — works today but brittle once a release pipeline rewrites the line.
- **P2-2**: `bashrc-additions.sh:17-32` race during install — fresh shell mid-install may see new binary but old recorded hash → red banner self-corrects on next shell. Document.
- **P2-3**: `claude-amq:38-39` uses `echo "$PWD" | sed`. With `set -e` and unusual paths, `echo` may interpret `-n`/`-e` flags. Use `printf '%s'`. (audit01 P2-10 still open.)
- **P2-4**: `dux-amq/install.sh:116, :136` two `trap 'rm -rf "$TMP"' EXIT` calls — second replaces first. Currently fine because each branch tears down. Use named tmp vars or stack traps.
- **P2-5**: `release.yml:13` matrix — `macos-latest` for both x86 and arm64; arm64 cross-compiles on Intel runner. Pin `macos-13` (Intel) + `macos-14`/latest (ARM).
- **P2-6**: `release.yml` no top-level `concurrency:` block — parallel release tags race on `gh release edit`. Add `concurrency: { group: release-${{ github.event.release.tag_name }} }`.
- **P2-7**: `release.yml` no reproducible-tar flags. Add `tar --sort=name --owner=0 --group=0 --numeric-owner --mtime='@0' -czf …`.
- **P2-8**: `release.yml:90-92` parses README for `## Install` section; silently empties release notes if the heading moves. Add `[[ -n "$install_section" ]] || exit 1`.
- **P2-9**: `pr.yml`, `overlay-ci.yml` — `on: push` runs on every branch; doubles CI minutes. Scope `push` to `branches: [main]`, leave `pull_request:` open.
- **P2-10**: `src/storage.rs:243-244, 247-249` — `serde_json::to_string(...).unwrap_or_else(|_| "[]".to_string())` and matching parse — silently maps DB corruption to "no providers ever started". WARN log.
- **P2-11**: `src/git.rs` uses `to_string_lossy()` everywhere for `-C path` — non-UTF-8 worktree paths get `\u{FFFD}`'d. Pass `OsStr` directly to `Command::arg`.
- **P2-12**: `src/provider.rs:23-32` tempfile name uses `SystemTime::duration_since(UNIX_EPOCH).unwrap_or(0)` — clock-skew collision risk. Use `Uuid::new_v4()` (already a dep).
- **P2-13**: `src/clipboard.rs:36, 77` — `Builder::new().spawn().expect()` panics if `RLIMIT_NPROC` hit. Degrade gracefully to OSC52-only.
- **P2-14**: `src/config.rs::expand_path` documented as "rejects `..` components" but `${X}` where `X` itself contains `..` is accepted. Add a post-expansion canonicalisation check.
- **P2-15**: AMQ message TTL — old messages accumulate forever in the maildir-style queue. Upstream-side fix; file an issue.
- **P2-16**: `dux config reset` doesn't validate symlink targets on each launch — symlink swap of `~/.claude` is a real attack on shared `/data/state`.
- **P2-17**: build profile has `debug = 0` — strips line numbers from panics. For prod consider `debug = "line-tables-only"` (~5% size, big diagnostics win).
- **P2-18**: `src/app/input.rs` is 10,506 LOC — 4× the 200-line CLAUDE.md submodule guideline. Extract palette, prompt-state-machine, mouse subsystems.
- **P2-19**: `tests/scrollbar_render.rs` is a placeholder file. Either populate or delete.
- **P2-20**: `signal-hook` 0.3.18 + 0.4.4 both in tree — duplicate-dep code-bloat (not a CVE).
- **P2-21**: README missing `SECURITY.md`, `CODEOWNERS` (verified absent in `.github/`).
- **P2-22**: No `dependabot.yml` / `renovate.json` (verified absent).

---

## Architecture overhauls (the big-ticket items)

The user invited large overhauls. The four highest-leverage:

### 1. Decompose `App` (P1-V) — large, blocking on every other UI change
`src/app/mod.rs:54-201` declares 120 `pub(crate)` fields on a single struct. Submodules (`input.rs`, `render.rs`, `sessions.rs`, `workers.rs`) split *methods* by concern but every method reaches into `&mut self` for any of those 120 fields. That's a peer-mutable god object — open-source contributors must mentally hold the whole field list before touching anything.

Proposed structure:
```rust
struct App {
    ui: UiState,        // panes, focus, scrollback offsets, prompt stack, palette, mouse
    runtime: RuntimeState, // PTY map, providers, lockfile, signals
    git: GitState,      // projects, worktrees, change-file caches
    remote: RemoteState, // gh state, AMQ identity, network workers
    config: Config,
    theme: Theme,
}
```
Each submodule's `impl App` block becomes `impl GitState` / `impl UiState` etc. with thin `App` shims that delegate. No semantic change; pure refactor. Effort: 2–3 days. Unblocks every other architectural improvement (state machine, schema versioning, doctor tool).

### 2. Explicit session state machine (P1-Z) — medium
Today: `enum SessionStatus = Active | Detached | Exited` + `providers: HashMap<String, PtyClient>` + `child.try_wait()` exit signal — three sources of truth that drift. Fix with one typestate: `enum SessionState { Created, Spawning, Live { pty }, Detached { last_seen }, Exited { code } }`. Forces all transitions through one function. Effort: 1 day.

### 3. Replace `logger.rs` with `tracing` + JSON + rotation (P1-X) — small
Stops P0-B (sanitization at the source via `tracing` field-formatters), unblocks per-session purge (P0-J needs structured fields), gives you log rotation and OpenTelemetry export for free. Effort: half a day.

### 4. Ship `dux-amq doctor` per phase-16 plan — small/medium
Without it, every other production-readiness improvement is invisible to operators. Sections (per audit01 P2-11 + audit02): version table, binary integrity (sha256 vs `binary.sha256`), persistent-disk free space + top-3 worktree sizes, AMQ queue depth + oldest-message age, `~/.claude` symlink target, `sysctl dev.tty.legacy_tiocsti`, sqlite `PRAGMA integrity_check`, currently-running dux PID + uptime + RSS, `--anonymize` flag for safe sharing. Effort: 1 day in bash + a Rust subcommand.

---

## Threat model (STRIDE, abridged)

| # | Threat | STRIDE | Asset | Mitigation present | Sev |
|---|---|---|---|---|---|
| T1 | Malicious repo executes via default `--dangerously-skip-permissions` | T,E | Host shell, API tokens | None — opt-out only | P0 |
| T2 | Compromised AMQ peer poisons sibling via spoofed `--me` | S,T | Other panes | Filesystem perms (700) | P0 |
| T3 | Tampered `amq` binary `eval`'d every shell | T,E | TCB | sha256 guard | P1 (P1-E) |
| T4 | Spot-VM preemption mid-sqlite write | T,D | sessions.sqlite3 | Default rollback journal | P1 (P1-W) |
| T5 | Plaintext API tokens + chat history on persistent disk | I | API tokens, PII | GCE PD encryption (Google KMS) | P1 |
| T6 | Right-to-erasure: no full-purge command | I,N | Chat JSONLs | Partial — `dux config reset` skips `~/.claude/projects/` | P0 (P0-J) |
| T7 | Wrapper identity collision (`feat/foo` ≡ `feat-foo`) | S | AMQ identity | None — README warns | P1 (P1-F) |
| T8 | Log injection via PTY content into `dux.log` | T | Operator trust | None | P0 (P0-B) |
| T9 | Resource exhaustion: no PTY/memory caps | D | Host RAM/disk | Per-pane scrollback only | P1 (P1-AA) |
| T10 | DoS via AMQ flood (10k tiny mailbox files exhaust inodes) | D | Filesystem | None | P1 |
| T11 | Symlink swap of `~/.claude` → attacker dir | T,E | API tokens | None | P2 |
| T12 | Auto-resume thundering herd | D | Host CPU | None | P1 (P1-U) |

Trust boundary: **single-user, single-Linux-account model**. All agents share `$HOME`, perms, env. There is no in-VM isolation between panes — one compromised pane = one compromised user account. This is the central trust assumption and must be documented in `SECURITY.md`.

---

## Test coverage gaps (ranked by risk)

| Area | Coverage | Risk |
|---|---|---|
| `src/app/workers.rs` (1,441 LOC) | **0 unit tests** — `run_pr_sync`, `check_pr_for_entry`, `parse_pr_json_value`, `run_create_agent_job`, `run_add_project_checkout_job` all uncovered | High |
| `src/logger.rs` | **0 tests** — and P0-B says it's broken | High |
| `src/storage.rs::ensure_column` | **0 tests** — P1-K relies on this | Medium |
| `src/app/render.rs` (6,276 LOC) | 41 tests | Medium |
| `src/app/input.rs` (10,506 LOC) | 196 tests but mostly modal-state assertions; **zero PTY-byte injection tests** | High |
| `src/clipboard.rs::clipboard_worker` (real arboard path) | only `from_fn` mock | Medium |
| `src/git.rs::parse_status_porcelain_z`, `parse_github_owner_repo` | **0 direct tests** | Medium |
| `tests/scrollbar_render.rs` | placeholder, zero tests | Low |
| `dux-amq/wrappers/*` path-encoder regression (audit01 P0-5) | 0 fixture tests | High (security) |

---

## Quick-win action items (ranked by ROI)

1. **Pin every action by SHA across all 4 workflows** (30 min) — closes P0-H. Use `pinact`.
2. **Add `permissions: contents: read` to `pr.yml`/`test.yml`** (2 lines).
3. **Add `cargo audit` + `cargo deny` PR-CI job** (20 min + starter `deny.toml`).
4. **Flip YOLO defaults in `claude-amq` and `codex-amq`** (5 min) — P0-A.
5. **Write `sanitize_for_terminal` and route `logger::*` + `set_error/set_info` through it** (1–2 hours) — P0-B + P0-C.
6. **Gate `amq init --force` on existing `.amqrc` marker** (3 lines) — P0-F.
7. **Fix `strip_block` legacy rule with end-of-block sentinel** (10 min) — P0-G.
8. **Replace 3 `pty.rs` `.expect()` panics with poison-tolerant code** (10 min) — P0-E.
9. **Add `actions/attest-build-provenance` + `cargo auditable build` + `SHA256SUMS` upload to `release.yml`** (1 hour) — half of P0-I.
10. **Add `dependabot.yml`** (10 min).
11. **Move startup `git` calls to a worker** (~half day) — P0-D.
12. **Implement `dux session purge --hard <id>`** (1 day) — P0-J.
13. **Land `dux-amq doctor`** (1–2 days) — overhaul §4.
14. **Replace `logger.rs` with `tracing`** (half day) — overhaul §3, prerequisite for #12.

---

## Out of scope / accepted risks

- **AMQ binary internals** beyond the wake transport (P1-1 confirmed). Full audit of avivsinai/agent-message-queue is a separate engagement.
- **Dynamic e2e on a kernel with `dev.tty.legacy_tiocsti=0`** — confirmed via static source read; recommend a smoke test.
- **Server-side GitHub configuration** (branch protection, tag protection, default workflow permissions, allowed actions list) — needs follow-up on the GitHub UI; document expected settings in `SECURITY.md`.
- **Multi-tenant use** — explicitly single-user-on-a-VM. Multi-user requires redesign.
- **AMQ protocol-level auth** (P0-K HMAC fix) — needs upstream coordination; document interim mitigation (process-isolation, less-privileged file modes).

---

## Sources consulted (web research, May 2026)

- RustSec Advisory Database — `https://rustsec.org/advisories/`
- GitHub Actions Secure Use Reference — `https://docs.github.com/en/actions/reference/security/secure-use`
- StepSecurity: Pinning GitHub Actions — `https://www.stepsecurity.io/blog/pinning-github-actions-for-enhanced-security-a-complete-guide`
- GitHub Changelog: Actions policy supports SHA pinning (Aug 2025) — `https://github.blog/changelog/2025-08-15-github-actions-policy-now-supports-blocking-and-sha-pinning-actions/`
- GitHub Artifact Attestations — `https://docs.github.com/en/actions/security-for-github-actions/using-artifact-attestations`
- Sigstore "A Safer curl|bash" — `https://blog.sigstore.dev/a-safer-curl-bash-7698c8125063/`
- cargo-auditable — `https://github.com/rust-secure-code/cargo-auditable`
- Check Point: RCE & API Token Exfiltration in Claude Code (CVE-2025-59536) — `https://research.checkpoint.com/2026/rce-and-api-token-exfiltration-through-claude-code-project-files-cve-2025-59536/`
- VentureBeat: Six Exploits Broke AI Coding Agents — `https://venturebeat.com/security/six-exploits-broke-ai-coding-agents-iam-never-saw-them`
- Phoenix Security: Three CVEs in Claude Code CLI — `https://phoenix.security/claude-code-leak-to-vulnerability-three-cves-in-claude-code-cli-and-the-chain-that-connects-them/`
- Anthropic Engineering: Multi-agent research system — `https://www.anthropic.com/engineering/multi-agent-research-system`
- TIOCSTI deprecation status — Ubuntu LP #2046192, `cateee.net/lkddb/web-lkddb/LEGACY_TIOCSTI.html`
- Atomic dir-swap patterns — BashFAQ/045, `axialcorps.wordpress.com/2013/07/03/atomically-replacing-files-and-directories/`
- ANSI escape security — `dgl.cx/2023/09/ansi-terminal-security`, `packetlabs.net/posts/weaponizing-ansi-escape-sequences/`
- Rails ActiveRecord ANSI log injection (CVE-2025-55193) — `rubysec.com/advisories/CVE-2025-55193/`
- Rust 2024 edition — `blog.rust-lang.org/2025/02/20/Rust-1.85.0/`
- tracing + OpenTelemetry — `oneuptime.com/blog/post/2026-01-07-rust-tracing-structured-logs/view`
- GCE CSEK + LUKS — `cloud.google.com/compute/docs/disks/customer-supplied-encryption`, `github.com/salrashid123/gcp_luks_csek_disks`
- AMQ source: `internal/cli/wake_tiocsti_unix.go` in `github.com/avivsinai/agent-message-queue`

---

**Total findings**: 11 P0, 29 P1, 22 P2 = 62 ranked items. Of audit01's 24 items, 4 are CLOSED, 20 OPEN. The path to production is clear and concrete: the quick-win list above eliminates ~70% of the risk surface in roughly one engineering week. The architectural overhauls (god-object decomp, state machine, `tracing`, doctor tool) should follow in a second week and unblock every subsequent feature.
