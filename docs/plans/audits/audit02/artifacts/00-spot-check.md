# Audit02 P0 Spot-Check

Re-verification of audit02 P0 findings against current HEAD.

- Baseline rev: `554255df5d92a2200dc0b619413221bdd4fe50c7`
- Branch: `audit02/00-preflight` (forked from `audit01/install-chain`)
- Audit was originally captured at `554255d` — HEAD matches the audit's reference commit, so line numbers should align.

## Results

| ID    | Confirmed | File:line                                | Snippet |
|-------|-----------|------------------------------------------|---------|
| P0-A  | Y         | `dux-amq/wrappers/claude-amq:79-85`      | `# Every interactive claude pane gets --dangerously-skip-permissions by / # default. Set CLAUDE_AMQ_SAFE=1 to opt out ... EXTRA+=(--dangerously-skip-permissions)` — opt-out via `CLAUDE_AMQ_SAFE` |
| P0-A  | Y         | `dux-amq/wrappers/codex-amq:27`          | `exec amq coop exec ... codex -- --dangerously-bypass-approvals-and-sandbox "$@"` — unconditional bypass |
| P0-B  | Y         | `src/logger.rs:77-94`                    | `fn log(level: LogLevel, message: &str) { ... let line = format!("{} {:<5} {}\n", ...); ... file.write_all(line.as_bytes()); ... }` — no sanitization |
| P0-D  | Y         | `src/app/sessions.rs:50,66,71`           | `git::is_git_repo(&path)`, `git::current_branch(&path)?`, `git::remote_default_branch(&path)` — sync git on UI thread |
| P0-D  | Y         | `src/app/mod.rs:1974,2363,2389`          | `git::changed_files(&p)`, `git::is_git_repo(&p)`, `git::current_branch(&path)` — sync git on UI thread |
| P0-E  | Y         | `src/pty.rs:277,289,295`                 | 3 hits of `let terminal = self.terminal.lock().expect("terminal mutex poisoned");` |
| P0-F  | Y         | `dux-amq/install.sh:148`                 | `amq init --root "$STATE_ROOT/amq" --agents claude,codex,gemini --force >/dev/null` — unconditional `--force` |
| P0-G  | Y         | `dux-amq/install.sh:83-90`               | `awk '/^<!-- >>> dux-amq v[^ ]+ >>> -->$/ {s=1; next} ... /^## Multi-agent environment .../ {s=1; next} !s'` — md branch sets `s=1` on legacy heading and never resets |
| P0-H  | Y         | `.github/workflows/release.yml:32,38,42` | `actions/checkout@v4`, `dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache@v2` — tag/floating-ref pinned |
| P0-H  | Y         | `.github/workflows/pr.yml:15,17,28,30,34,43,45,47` | `actions/checkout@v4`, `dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache@v2` — tag/floating-ref pinned |
| P0-H  | Y         | `.github/workflows/test.yml:10,12,14`    | `actions/checkout@v4`, `dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache@v2` — tag/floating-ref pinned |
| P0-J  | Y         | `src/cli.rs:86,464`                      | `reset_agent_data(paths)?;` and `fn reset_agent_data(paths: &DuxPaths) -> Result<()>` — only `reset`, no `purge` cascading delete |
| P0-K  | Partial   | `dux-amq/wrappers/claude-amq:96`, `dux-amq/wrappers/codex-amq:25` | `amq wake --me "$ME" --root "$ROOT" --inject-mode raw </dev/tty ...` — 2 hits with `raw`. `gemini-amq:29` uses `--inject-mode auto` (not `raw`); audit said "3 hits". Finding still applies (no envelope auth on any of the three wrappers) but the `raw`-mode count is 2, not 3. |

## Notes

- All 9 P0 findings remain valid against `554255d`. None were silently fixed in the meantime.
- P0-K wording ("3 hits, no envelope auth") is slightly imprecise: gemini-amq uses `--inject-mode auto`, not `raw`. The underlying threat (no sender authentication on AMQ wake-driven auto-injection) still applies to all three wrappers, but Phase 08 should treat gemini's `auto` mode separately if its semantics differ from `raw`.
- HEAD matches the audit's reference commit `554255d`, so line numbers in the audit are exact (no shift to compensate for via `grep -n`).
