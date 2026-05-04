# Phase 10: GDPR hard-purge — `dux session purge --hard`

> Maps to: **P0-J** (T6 in threat model). GDPR Art 17 right-to-erasure.

## Goal
Add a single command that removes ALL bytes associated with a session:
sqlite row, worktree dir, Claude/Codex/Gemini chat JSONLs under
`/data/state/<provider>/projects/<encoded>/`, AMQ inbox under
`/data/state/amq/<branch>/`, and (Phase 09 prerequisite) any `dux.log`
records tagged with that `session_id`.

Today's `dux config reset --all` removes worktrees + sqlite + log
holistically but cannot target a single session, and does NOT touch the
provider chat dirs or AMQ inboxes — so a real "delete this customer's
data" request is impossible.

## Pre-conditions
- Phase 00 baseline green.
- Phase 09 (`tracing` migration) merged — log purge needs structured
  `session_id` fields to scope correctly. If 09 is not yet done,
  implement everything except log scoping; log purge becomes a follow-up.
- Phase 03 (sanitizer) merged — purge confirmation strings sanitized.

## Files to touch
- `src/cli.rs` — add `session purge` subcommand.
- `src/storage.rs` — add `delete_session_by_id(id) -> Result<...>`.
- `src/git.rs` — already has `remove_worktree`; reuse.
- `src/purge.rs` — NEW module orchestrating cascade delete.
- `src/config.rs` — add `provider_data_dirs` config (paths to scan per
  session) with sensible defaults derived from `DUX_HOME`.
- `tests/purge_integration.rs` — NEW.

## Steps

### 10.1 — CLI surface
`src/cli.rs`:
```rust
#[derive(Subcommand)]
pub enum SessionCmd {
    /// Permanently delete a session and ALL its on-disk data.
    /// Cascades into provider chat history, AMQ inbox, worktree, sqlite,
    /// and log lines tagged with this session_id.
    Purge {
        /// Session id (uuid) or branch name.
        target: String,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
        /// Dry run — print what would be deleted, change nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Same as purge but matches *all* sessions — used for full reset.
    PurgeAll {
        #[arg(long)] yes: bool,
        #[arg(long)] dry_run: bool,
    },
}
```

### 10.2 — Cascade orchestrator
`src/purge.rs`:
```rust
pub struct PurgePlan {
    pub session_id: String,
    pub branch: String,
    pub items: Vec<PurgeItem>,
}

pub enum PurgeItem {
    SqliteRow,
    Worktree(PathBuf),
    ProviderDir { provider: &'static str, path: PathBuf },
    AmqInbox(PathBuf),
    LogScopedRedact { since: chrono::DateTime<chrono::Utc> },
}

pub fn build_plan(
    storage: &Storage,
    paths: &DuxPaths,
    config: &PurgeConfig,
    target: &str,
) -> anyhow::Result<PurgePlan> { ... }

pub fn execute(plan: &PurgePlan, dry_run: bool) -> anyhow::Result<PurgeReport> {
    // For each PurgeItem, log structured event, then act.
    // Order matters:
    //   1. Worktree (so a stuck process can be detected before bytes vanish)
    //   2. Provider chat dirs
    //   3. AMQ inbox
    //   4. Log redact (Phase 09 prerequisite)
    //   5. Sqlite row LAST (so a crash mid-purge leaves a recoverable record)
}
```

### 10.3 — Path encoding (must match Claude Code's actual encoder)
The provider chat dirs are `/data/state/claude/projects/<enc>/` where
`<enc>` is a Claude-Code-internal encoding of the worktree absolute path.
**Audit01 P0-5** flagged that our `sed`-based encoder doesn't match —
Phase 12 will fix the encoder. Until Phase 12 lands, purge **must use the
same encoder as the seeding wrapper** so we don't orphan dirs.

Pull the encoder into `src/purge_encoding.rs` (shared with the eventual
Phase 12 wrapper-side fix). Add a fixture test that the encoded path of
a known input matches an observed real on-disk dir.

### 10.4 — Confirmation prompt
```rust
fn confirm(plan: &PurgePlan) -> bool {
    eprintln!("Will permanently delete the following for session {}:", plan.session_id);
    for item in &plan.items {
        eprintln!("  - {}", item.describe());
    }
    eprint!("Type 'PURGE {}' to confirm: ", plan.branch);
    let mut input = String::new();
    std::io::stdin().read_line(&mut input).ok();
    input.trim() == format!("PURGE {}", plan.branch)
}
```
Using "PURGE <branch>" as the magic word prevents accidental yes-mashing.

### 10.5 — Log scoping (Phase 09 dep)
Once `tracing` JSON logs include `session_id` fields, redact-by-id is:
```rust
pub fn redact_session_logs(log_path: &Path, session_id: &str) -> Result<usize> {
    // Stream-rewrite: read log_path, for each JSON line, if it has
    // session_id == target, replace `fields` with `{"redacted": true}`.
    // Atomic rename when done.
}
```
Pure redact (not delete) preserves audit trail of *which* sessions were
purged when, while erasing content. Some compliance regimes require the
audit record; check with operator before choosing redact vs delete.

### 10.6 — Tests
`tests/purge_integration.rs`:
```rust
#[test]
fn purge_removes_all_known_categories() {
    let tmp = tempfile::tempdir().unwrap();
    // Create fixtures: sqlite row, worktree, fake claude project dir,
    // fake amq inbox, fake log line.
    // ...
    let plan = build_plan(...).unwrap();
    let report = execute(&plan, false).unwrap();
    assert!(plan.items.iter().all(|item| match item {
        PurgeItem::SqliteRow => storage.find(...).is_none(),
        PurgeItem::Worktree(p) => !p.exists(),
        PurgeItem::ProviderDir{path, ..} => !path.exists(),
        PurgeItem::AmqInbox(p) => !p.exists(),
        PurgeItem::LogScopedRedact{..} => log_has_no_session_records(),
    }));
}

#[test]
fn purge_dry_run_changes_nothing() { ... }
#[test]
fn purge_aborts_on_wrong_confirmation() { ... }
#[test]
fn purge_with_unknown_target_returns_error_not_panic() { ... }
```

### 10.7 — Document in README
Under a "Data lifecycle" section:
```
Right-to-erasure: `dux session purge --hard <branch-or-id>` permanently
deletes every on-disk artifact associated with a session, including
chat history under /data/state/{claude,codex,gemini}/projects/, the AMQ
inbox, the worktree, and (when structured logging is enabled) all log
records tagged with that session_id. Run with --dry-run first.
```

## Validation
- `cargo test --test purge_integration` green.
- Manual on test VM: create a session, send messages, run purge with
  `--dry-run` (verify list of paths), then for real (verify all gone).
- `cargo clippy --all-targets -- -D warnings` green.

## Acceptance criteria
- [x] `dux session purge --hard <target>` exists and is documented (`src/cli.rs::run_session_purge`, README "Data lifecycle").
- [x] Cascades to: sqlite, worktree, provider dirs (claude/codex/gemini),
      AMQ inbox, log redact.
- [x] `--dry-run` prints plan and exits without changes.
- [x] Confirmation requires typing "PURGE <branch>" verbatim.
- [x] Order of operations: worktree → providers → amq → log → sqlite (verified in `src/purge.rs::plan_for_session`).
- [x] 4 integration tests pass (`tests/purge_integration.rs`).
- [x] README "Data lifecycle" section.
- [x] PR: `feat(privacy): hard-purge command for full session erasure (P0-J)` — landed via PR #2.

## Known pitfalls
- The Claude Code on-disk encoder is **not** documented; we're
  reverse-engineering. Phase 12's encoder fix and Phase 10's purge
  encoder MUST share the same module to avoid drift. If Phase 12
  hasn't shipped, lock the encoder behind a feature flag and document
  the limitation.
- Provider dirs may live on a different filesystem than the worktree
  (persistent disk vs boot disk). Use `std::fs::remove_dir_all` which
  handles the cross-fs case; not `rename`-then-delete.
- AMQ may have queued messages to the purged peer from siblings. Send
  one final "purge" notification BEFORE deleting the inbox so other
  panes see the agent is gone (don't surprise them).
- `--dry-run` must be honored by every step. A bug where one step
  ignores it could cause silent data loss in test runs.
- Don't try to delete /data/state/<provider>/* recursively — only the
  *specific* `projects/<encoded>/` dir. Wider deletes catch other panes'
  data.

## References
- audit02 P0-J, T6.
- GDPR Art 17 right-to-erasure.
- audit01 P0-5 (Claude Code path encoder; Phase 12 here).
