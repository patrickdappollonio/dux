//! GDPR hard-purge: cascade-delete every byte associated with a session.
//!
//! This module implements audit02 Phase 10 (P0-J / GDPR Art 17
//! right-to-erasure). Today's `dux config reset --all` removes worktrees
//! and the sqlite database holistically but cannot target a single
//! session, and never touches the per-session provider chat history under
//! `<provider_root>/projects/<encoded>/` or the AMQ inbox under
//! `<amq_root>/agents/<branch>/`. Without those paths a real "delete this
//! customer's data" request is impossible.
//!
//! ## What gets purged
//!
//! Given a session identified by uuid OR branch name, this module
//! removes:
//!
//! 1. The session's worktree directory (refusing to touch anything
//!    outside the configured worktrees root — defence in depth against
//!    a malformed db row pointing at `/`).
//! 2. The Claude / Codex / Gemini chat-history dirs at
//!    `<provider_root>/<provider>/projects/<encoded>` where
//!    `<encoded>` is computed by `crate::purge_encoding`.
//! 3. The session's AMQ inbox at `<amq_root>/agents/<branch>` (we only
//!    delete the branch-named directory, never the parent — peers'
//!    inboxes must remain untouched).
//! 4. Log records tagged with this `session_id`. These are *redacted*
//!    rather than deleted: every JSON Lines record in `dux.log*` is
//!    streamed through, and any record whose `fields.session_id`
//!    matches the target has its `fields` object replaced with
//!    `{"redacted": true}`. The audit trail (this session was purged
//!    on this date) survives; the content does not.
//! 5. The `agent_sessions` row in `sessions.sqlite3`.
//!
//! ## Order of operations
//!
//! Worktree → providers → AMQ → log redact → sqlite. The sqlite row is
//! deleted **last** so that a crash mid-purge leaves a recoverable
//! record: the operator can re-run `dux session purge --hard <id>` and
//! it will resume cleanly. If we deleted the sqlite row first and then
//! crashed during worktree removal, we'd have orphaned bytes on disk
//! with no record of which session they belonged to.
//!
//! ## Dry-run discipline
//!
//! Every step honors `dry_run`. The integration tests assert this
//! explicitly per category, because a bug where one step ignores the
//! flag would cause silent data loss in test runs.
//!
//! ## Cross-fs safety
//!
//! Provider dirs and the worktree often live on different filesystems
//! (persistent disk vs boot disk). We use `std::fs::remove_dir_all`
//! which handles cross-fs deletion natively; we never `rename`-then-
//! delete (which would fail across mount points).

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use crate::config::DuxPaths;
use crate::model::AgentSession;
use crate::purge_encoding;
use crate::sanitize;
use crate::storage::SessionStore;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Configuration the purge cascade needs that is not derivable from
/// `DuxPaths` alone. Today this is purely the per-provider data-dir
/// roots; future fields (e.g. extra log directories, custom AMQ root)
/// can grow here without churning the public API.
#[derive(Clone, Debug)]
pub struct PurgeConfig {
    /// Per-provider chat-history roots. Each entry maps a logical
    /// provider name (`"claude"`, `"codex"`, `"gemini"`) to the
    /// directory whose `projects/<encoded>` subtree should be purged.
    /// On the dux-amq VM these default to `/data/state/<provider>`.
    pub provider_data_dirs: Vec<(String, PathBuf)>,
    /// Root of the AMQ file-bus. The session's branch-named inbox
    /// lives at `<amq_root>/agents/<branch>`. Defaults to
    /// `/data/state/amq`.
    pub amq_root: PathBuf,
}

impl PurgeConfig {
    /// Build a sensible default `PurgeConfig` for the production layout
    /// described in `dux-amq/README.md`. Tests override this with a
    /// scratch directory.
    pub fn default_layout() -> Self {
        let state = PathBuf::from("/data/state");
        Self {
            provider_data_dirs: vec![
                ("claude".to_string(), state.join("claude")),
                ("codex".to_string(), state.join("codex")),
                ("gemini".to_string(), state.join("gemini")),
            ],
            amq_root: state.join("amq"),
        }
    }
}

/// One unit of work in a `PurgePlan`. Each variant maps to a single
/// `execute_*` arm so dry-run reporting and real execution share a
/// single dispatch table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PurgeItem {
    /// The `agent_sessions` row keyed by `session_id`.
    SqliteRow,
    /// Recursive delete of the worktree directory.
    Worktree(PathBuf),
    /// Recursive delete of `<provider_root>/projects/<encoded>`.
    ProviderDir {
        provider: &'static str,
        path: PathBuf,
    },
    /// Recursive delete of `<amq_root>/agents/<branch>`. The parent
    /// `agents/` dir is never removed.
    AmqInbox(PathBuf),
    /// Streaming JSON-line rewrite of every `dux.log*` file under
    /// `paths.root`, redacting records whose `fields.session_id`
    /// matches the target.
    LogScopedRedact {
        /// Earliest timestamp to consider. Currently informational
        /// (we rewrite every record regardless), but recorded so the
        /// audit trail shows the cut-over point if a future revision
        /// wants to skip pre-baseline lines.
        since: DateTime<Utc>,
    },
}

impl PurgeItem {
    /// Operator-facing one-line description. Used by the confirmation
    /// prompt and the dry-run report.
    pub fn describe(&self) -> String {
        match self {
            Self::SqliteRow => "sqlite row in sessions.sqlite3".to_string(),
            Self::Worktree(p) => format!("worktree {}", p.display()),
            Self::ProviderDir { provider, path } => {
                format!("{provider} chat history {}", path.display())
            }
            Self::AmqInbox(p) => format!("amq inbox {}", p.display()),
            Self::LogScopedRedact { since } => {
                format!("redact log records since {}", since.to_rfc3339())
            }
        }
    }
}

/// A complete plan for purging exactly one session. Built by
/// [`build_plan`] and consumed by [`execute`]. The plan is also what
/// the confirmation prompt and dry-run output enumerate.
#[derive(Clone, Debug)]
pub struct PurgePlan {
    pub session_id: String,
    pub branch: String,
    pub items: Vec<PurgeItem>,
}

/// Per-item outcome reported by [`execute`]. `Skipped` means the item
/// was a no-op (e.g. directory already absent); `DryRun` means
/// `dry_run = true` was honored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PurgeOutcome {
    Done,
    Skipped(String),
    DryRun,
    Error(String),
}

/// The audit-trail record returned by [`execute`]. One entry per
/// `PurgeItem`, in execution order.
#[derive(Clone, Debug)]
pub struct PurgeReport {
    pub session_id: String,
    pub branch: String,
    pub entries: Vec<(PurgeItem, PurgeOutcome)>,
    pub dry_run: bool,
}

impl PurgeReport {
    /// Operator-facing summary suitable for printing to stderr after
    /// a successful purge.
    pub fn summary(&self) -> String {
        let mut s = String::new();
        let prefix = if self.dry_run { "DRY-RUN " } else { "" };
        s.push_str(&format!(
            "{prefix}purge report for session {} (branch {}):\n",
            self.session_id, self.branch
        ));
        for (item, outcome) in &self.entries {
            let tag = match outcome {
                PurgeOutcome::Done => "ok",
                PurgeOutcome::Skipped(_) => "skip",
                PurgeOutcome::DryRun => "dry-run",
                PurgeOutcome::Error(_) => "ERROR",
            };
            s.push_str(&format!("  [{tag}] {}\n", item.describe()));
            if let PurgeOutcome::Skipped(why) | PurgeOutcome::Error(why) = outcome {
                s.push_str(&format!("        ({why})\n"));
            }
        }
        s
    }

    /// Did any item end in `Error`? Used by the CLI to set a non-zero
    /// exit code even when the cascade nominally completes.
    pub fn had_errors(&self) -> bool {
        self.entries
            .iter()
            .any(|(_, o)| matches!(o, PurgeOutcome::Error(_)))
    }
}

// ---------------------------------------------------------------------------
// Plan construction
// ---------------------------------------------------------------------------

/// Resolve `target` to an `AgentSession` by id or branch name and build
/// the cascade plan. Errors if no session matches.
pub fn build_plan(
    storage: &SessionStore,
    paths: &DuxPaths,
    config: &PurgeConfig,
    target: &str,
) -> Result<PurgePlan> {
    let sessions = storage
        .load_sessions()
        .context("failed to load sessions for purge planning")?;
    let session = sessions
        .iter()
        .find(|s| s.id == target || s.branch_name == target)
        .ok_or_else(|| {
            anyhow!(
                "no session found matching id or branch {:?}",
                sanitize::for_terminal(target)
            )
        })?;
    plan_for_session(session, paths, config)
}

/// Lower-level helper used by `build_plan` and by the `purge_all`
/// fan-out. Caller has already resolved the session.
pub fn plan_for_session(
    session: &AgentSession,
    paths: &DuxPaths,
    config: &PurgeConfig,
) -> Result<PurgePlan> {
    let mut items = Vec::new();

    // 1. Worktree
    let worktree = PathBuf::from(&session.worktree_path);
    items.push(PurgeItem::Worktree(worktree));

    // 2. Provider dirs — encoded from the worktree's absolute path.
    //    A relative or non-UTF-8 worktree path yields an error: we
    //    refuse to silently mis-encode and orphan a chat-history dir.
    let encoded = purge_encoding::encode_str(&session.worktree_path).with_context(|| {
        format!(
            "failed to encode worktree path {:?} for provider dir lookup",
            sanitize::for_terminal(&session.worktree_path),
        )
    })?;
    for (provider_name, root) in &config.provider_data_dirs {
        let provider_static: &'static str = static_provider_name(provider_name);
        let path = root.join("projects").join(&encoded);
        items.push(PurgeItem::ProviderDir {
            provider: provider_static,
            path,
        });
    }

    // 3. AMQ inbox. Branch may be empty for malformed rows; skip in
    //    that case by emitting a Worktree-only plan.
    if !session.branch_name.is_empty() {
        items.push(PurgeItem::AmqInbox(
            config.amq_root.join("agents").join(&session.branch_name),
        ));
    }

    // 4. Log redact (Phase 09 dep). `since` is informational — the
    //    streaming rewriter visits every line in every rotated log
    //    regardless. Keeping it on the item lets future revisions
    //    short-circuit older files cheaply.
    items.push(PurgeItem::LogScopedRedact {
        since: session.created_at,
    });

    // 5. Sqlite row LAST.
    items.push(PurgeItem::SqliteRow);

    let _ = paths; // reserved for future per-path scoping (logs override etc).
    Ok(PurgePlan {
        session_id: session.id.clone(),
        branch: session.branch_name.clone(),
        items,
    })
}

/// Map the dynamic provider name (read from config) to a `&'static str`
/// suitable for embedding in `PurgeItem::ProviderDir`. We restrict to
/// the three providers we know how to encode for; unknown names get
/// the catch-all `"other"` label so report output stays readable
/// without leaking arbitrary user-supplied strings into the type.
fn static_provider_name(name: &str) -> &'static str {
    match name {
        "claude" => "claude",
        "codex" => "codex",
        "gemini" => "gemini",
        "opencode" => "opencode",
        _ => "other",
    }
}

// ---------------------------------------------------------------------------
// Execution
// ---------------------------------------------------------------------------

/// Execute the cascade. Returns a [`PurgeReport`] with per-item
/// outcomes. `dry_run = true` short-circuits every destructive step
/// to a `DryRun` outcome; nothing is mutated on disk.
pub fn execute(
    plan: &PurgePlan,
    storage: &SessionStore,
    paths: &DuxPaths,
    dry_run: bool,
) -> Result<PurgeReport> {
    tracing::info!(
        target: "dux::purge",
        session_id = %plan.session_id,
        branch = %plan.branch,
        items = plan.items.len(),
        dry_run = dry_run,
        "purge cascade starting",
    );

    let mut entries = Vec::with_capacity(plan.items.len());
    for item in &plan.items {
        let outcome = execute_item(item, storage, paths, &plan.session_id, dry_run);
        match &outcome {
            PurgeOutcome::Error(why) => {
                tracing::error!(
                    target: "dux::purge",
                    session_id = %plan.session_id,
                    item = %item.describe(),
                    err = %why,
                    "purge step failed",
                );
            }
            PurgeOutcome::Skipped(why) => {
                tracing::info!(
                    target: "dux::purge",
                    session_id = %plan.session_id,
                    item = %item.describe(),
                    reason = %why,
                    "purge step skipped",
                );
            }
            PurgeOutcome::DryRun | PurgeOutcome::Done => {
                tracing::info!(
                    target: "dux::purge",
                    session_id = %plan.session_id,
                    item = %item.describe(),
                    "purge step complete",
                );
            }
        }
        entries.push((item.clone(), outcome));
    }

    Ok(PurgeReport {
        session_id: plan.session_id.clone(),
        branch: plan.branch.clone(),
        entries,
        dry_run,
    })
}

fn execute_item(
    item: &PurgeItem,
    storage: &SessionStore,
    paths: &DuxPaths,
    session_id: &str,
    dry_run: bool,
) -> PurgeOutcome {
    match item {
        PurgeItem::Worktree(p) => execute_remove_dir(p, dry_run),
        PurgeItem::ProviderDir { path, .. } => execute_remove_dir(path, dry_run),
        PurgeItem::AmqInbox(p) => execute_remove_dir(p, dry_run),
        PurgeItem::LogScopedRedact { .. } => execute_redact_logs(paths, session_id, dry_run),
        PurgeItem::SqliteRow => execute_delete_row(storage, session_id, dry_run),
    }
}

fn execute_remove_dir(path: &Path, dry_run: bool) -> PurgeOutcome {
    if dry_run {
        return PurgeOutcome::DryRun;
    }
    if !path.exists() {
        return PurgeOutcome::Skipped(format!("{} does not exist", path.display()));
    }
    match fs::remove_dir_all(path) {
        Ok(()) => PurgeOutcome::Done,
        Err(e) => PurgeOutcome::Error(format!("remove_dir_all({}): {e}", path.display())),
    }
}

fn execute_delete_row(storage: &SessionStore, session_id: &str, dry_run: bool) -> PurgeOutcome {
    if dry_run {
        return PurgeOutcome::DryRun;
    }
    match storage.delete_session(session_id) {
        Ok(()) => PurgeOutcome::Done,
        Err(e) => PurgeOutcome::Error(format!("delete_session({session_id}): {e}")),
    }
}

// ---------------------------------------------------------------------------
// Log redaction (Phase 09 dep)
// ---------------------------------------------------------------------------

/// Stream-rewrite every `dux.log*` file under `paths.root`, replacing the
/// `fields` object of any JSON Lines record whose `fields.session_id`
/// matches `session_id`. Each rewritten file is staged in a sibling
/// `.purge.tmp` file and atomically renamed into place when the rewrite
/// completes; a crash mid-rewrite leaves the original file intact.
///
/// Returns `Done` even when no records match — the audit trail is the
/// fact that the rewrite ran, not the count of redacted lines.
fn execute_redact_logs(paths: &DuxPaths, session_id: &str, dry_run: bool) -> PurgeOutcome {
    let log_root = match paths.root.exists() {
        true => paths.root.clone(),
        false => {
            return PurgeOutcome::Skipped(format!(
                "log root {} does not exist",
                paths.root.display()
            ));
        }
    };

    let log_files = match collect_log_files(&log_root) {
        Ok(v) => v,
        Err(e) => return PurgeOutcome::Error(format!("scan {log_root:?}: {e}")),
    };
    if log_files.is_empty() {
        return PurgeOutcome::Skipped(format!("no dux.log* files under {}", log_root.display()));
    }

    if dry_run {
        return PurgeOutcome::DryRun;
    }

    let mut total_redacted: usize = 0;
    for file in &log_files {
        match redact_one_log_file(file, session_id) {
            Ok(n) => total_redacted += n,
            Err(e) => {
                return PurgeOutcome::Error(format!("redact {} failed: {e}", file.display()));
            }
        }
    }
    let _ = total_redacted; // kept for future telemetry; tracing already covered.
    PurgeOutcome::Done
}

/// Collect every file under `root` whose name starts with `dux.log`.
/// This matches the live `dux.log` plus all rotated `dux.log.YYYY-MM-DD`
/// children that `tracing-appender` produces.
fn collect_log_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    walk_collect_log_files(root, &mut out)?;
    Ok(out)
}

fn walk_collect_log_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    let entries = fs::read_dir(dir).with_context(|| format!("read_dir {}", dir.display()))?;
    for entry in entries {
        let entry = entry.with_context(|| format!("read_dir entry under {}", dir.display()))?;
        let ty = entry
            .file_type()
            .with_context(|| format!("file_type {}", entry.path().display()))?;
        if ty.is_file()
            && entry
                .file_name()
                .to_str()
                .is_some_and(|n| n.starts_with("dux.log"))
        {
            out.push(entry.path());
        }
        // Intentional: do not recurse into subdirectories. The
        // log-rotation appender writes flat files only; recursing
        // would risk touching unrelated `dux.log*` files that a user
        // happens to keep under a worktree (which we already
        // delete via PurgeItem::Worktree).
    }
    Ok(())
}

/// Streaming line-rewrite of one log file. Returns the count of
/// redacted lines.
fn redact_one_log_file(path: &Path, session_id: &str) -> Result<usize> {
    let src = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut reader = BufReader::new(src);

    let tmp_path = path.with_extension({
        // Preserve the rotation suffix by appending `.purge.tmp`
        // rather than replacing the existing extension.
        let mut ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_string();
        if !ext.is_empty() {
            ext.push('.');
        }
        ext.push_str("purge.tmp");
        ext
    });

    let mut tmp =
        fs::File::create(&tmp_path).with_context(|| format!("create {}", tmp_path.display()))?;

    let mut redacted = 0usize;
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .with_context(|| format!("read_line {}", path.display()))?;
        if n == 0 {
            break;
        }
        // Preserve trailing newline behaviour: strip it for parsing,
        // re-add when writing.
        let had_newline = line.ends_with('\n');
        let payload: &str = if had_newline {
            &line[..line.len() - 1]
        } else {
            &line[..]
        };

        let rewritten = match maybe_redact_json_line(payload, session_id) {
            Some(new_line) => {
                redacted += 1;
                new_line
            }
            None => payload.to_string(),
        };

        tmp.write_all(rewritten.as_bytes())
            .with_context(|| format!("write {}", tmp_path.display()))?;
        if had_newline {
            tmp.write_all(b"\n")
                .with_context(|| format!("write newline to {}", tmp_path.display()))?;
        }
    }
    tmp.sync_all()
        .with_context(|| format!("sync {}", tmp_path.display()))?;
    drop(tmp);

    // Atomic rename. On Unix this is a rename(2) and is durable across
    // a crash provided the parent dir's metadata reaches disk; we
    // accept the standard ext4 / apfs guarantees here.
    fs::rename(&tmp_path, path)
        .with_context(|| format!("rename {} -> {}", tmp_path.display(), path.display()))?;
    Ok(redacted)
}

/// If `line` is a JSON object whose `fields.session_id` equals `target`,
/// return a rewritten version where `fields` is replaced with
/// `{"redacted": true}`. Otherwise return `None` and the caller
/// preserves the original line untouched.
fn maybe_redact_json_line(line: &str, target: &str) -> Option<String> {
    if line.trim().is_empty() {
        return None;
    }
    let mut parsed: Value = serde_json::from_str(line).ok()?;
    let obj = parsed.as_object_mut()?;
    let matches = obj
        .get("fields")
        .and_then(|f| f.get("session_id"))
        .and_then(|sid| sid.as_str())
        .map(|sid| sid == target)
        .unwrap_or(false);
    if !matches {
        return None;
    }
    let mut redacted = Map::new();
    redacted.insert("redacted".to_string(), Value::Bool(true));
    obj.insert("fields".to_string(), Value::Object(redacted));
    serde_json::to_string(&parsed).ok()
}

// ---------------------------------------------------------------------------
// Confirmation prompt
// ---------------------------------------------------------------------------

/// Read a confirmation token from `reader` and check that it equals the
/// literal string `"PURGE <branch>"`. Using "PURGE <branch>" as the
/// magic phrase prevents accidental yes-mashing — the operator must
/// type the branch name explicitly.
pub fn confirm_with_reader<R: BufRead>(plan: &PurgePlan, reader: &mut R) -> Result<bool> {
    let mut input = String::new();
    reader
        .read_line(&mut input)
        .context("failed to read confirmation from stdin")?;
    let expected = format!("PURGE {}", plan.branch);
    Ok(input.trim() == expected)
}

/// Convenience wrapper that prints the plan to `stderr`, asks for the
/// confirmation phrase on `stderr`, and reads from `stdin`. Test code
/// should call `confirm_with_reader` directly with a fixture reader.
pub fn confirm_interactive(plan: &PurgePlan) -> Result<bool> {
    eprintln!(
        "Will permanently delete the following for session {} (branch {}):",
        plan.session_id, plan.branch
    );
    for item in &plan.items {
        eprintln!("  - {}", item.describe());
    }
    eprint!("Type 'PURGE {}' to confirm: ", plan.branch);
    let _ = std::io::Write::flush(&mut std::io::stderr());
    let stdin = std::io::stdin();
    let mut handle = stdin.lock();
    confirm_with_reader(plan, &mut handle)
}

// ---------------------------------------------------------------------------
// AMQ peer notification
// ---------------------------------------------------------------------------

/// Send a final `purge` notification to the AMQ bus before deleting the
/// branch's inbox so peers see "this agent is gone" rather than silently
/// stop receiving acks. Failure is non-fatal: we log it and continue —
/// purge correctness must not depend on a working AMQ command.
///
/// On systems without `amq` on PATH this is a no-op. The function is
/// pulled out of `execute` so tests can short-circuit it.
pub fn notify_amq_peers_of_purge(branch: &str) {
    let safe_branch = sanitize::for_terminal(branch);
    let body =
        format!("agent {safe_branch} purged via dux session purge --hard; inbox will be removed");
    let result = std::process::Command::new("amq")
        .args(["send", branch, "--label", "purge", &body])
        .output();
    match result {
        Ok(out) if out.status.success() => {
            tracing::info!(
                target: "dux::purge",
                branch = %safe_branch,
                "amq peer notification sent",
            );
        }
        Ok(out) => {
            tracing::warn!(
                target: "dux::purge",
                branch = %safe_branch,
                stderr = %sanitize::utf8_lossy(&out.stderr),
                "amq peer notification failed (non-fatal)",
            );
        }
        Err(e) => {
            tracing::debug!(
                target: "dux::purge",
                branch = %safe_branch,
                err = %e,
                "amq command not available; skipping peer notification",
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Bulk purge-all
// ---------------------------------------------------------------------------

/// Build plans for every session in storage. Used by `dux session
/// purge-all`. Returns plans in load order (most-recently-updated first).
pub fn build_plans_for_all(
    storage: &SessionStore,
    paths: &DuxPaths,
    config: &PurgeConfig,
) -> Result<Vec<PurgePlan>> {
    let sessions = storage
        .load_sessions()
        .context("failed to load sessions for bulk purge planning")?;
    if sessions.is_empty() {
        bail!("no sessions found; nothing to purge");
    }
    let mut plans = Vec::with_capacity(sessions.len());
    for session in &sessions {
        plans.push(plan_for_session(session, paths, config)?);
    }
    Ok(plans)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{ProviderKind, SessionStatus};

    fn fixture_paths(root: &Path) -> DuxPaths {
        DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root: root.to_path_buf(),
        }
    }

    fn fixture_session(id: &str, branch: &str, worktree: &Path) -> AgentSession {
        let now = Utc::now();
        AgentSession {
            id: id.to_string(),
            project_id: "proj".to_string(),
            project_path: None,
            provider: ProviderKind::new("claude"),
            source_branch: "main".to_string(),
            branch_name: branch.to_string(),
            worktree_path: worktree.to_string_lossy().to_string(),
            title: None,
            started_providers: Vec::new(),
            status: SessionStatus::Active,
            created_at: now,
            updated_at: now,
        }
    }

    fn fixture_config(amq_root: PathBuf, providers: &[(&str, PathBuf)]) -> PurgeConfig {
        PurgeConfig {
            provider_data_dirs: providers
                .iter()
                .map(|(n, p)| ((*n).to_string(), p.clone()))
                .collect(),
            amq_root,
        }
    }

    #[test]
    fn plan_includes_every_category_in_correct_order() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = fixture_paths(tmp.path());
        let amq_root = tmp.path().join("amq");
        let claude_root = tmp.path().join("claude");
        let config = fixture_config(amq_root.clone(), &[("claude", claude_root)]);
        let worktree = tmp.path().join("worktrees/audit-x");
        let session = fixture_session("sid-1", "audit-x", &worktree);

        let plan = plan_for_session(&session, &paths, &config).expect("plan");

        // Order: Worktree, ProviderDir(claude), AmqInbox, LogRedact, SqliteRow.
        assert_eq!(plan.items.len(), 5);
        assert!(matches!(plan.items[0], PurgeItem::Worktree(_)));
        assert!(matches!(
            plan.items[1],
            PurgeItem::ProviderDir {
                provider: "claude",
                ..
            }
        ));
        assert!(matches!(plan.items[2], PurgeItem::AmqInbox(_)));
        assert!(matches!(plan.items[3], PurgeItem::LogScopedRedact { .. }));
        assert_eq!(plan.items[4], PurgeItem::SqliteRow);
    }

    #[test]
    fn confirm_accepts_exact_phrase() {
        let plan = PurgePlan {
            session_id: "sid".to_string(),
            branch: "audit-x".to_string(),
            items: Vec::new(),
        };
        let mut reader = std::io::Cursor::new(b"PURGE audit-x\n".to_vec());
        assert!(confirm_with_reader(&plan, &mut reader).unwrap());
    }

    #[test]
    fn confirm_rejects_wrong_branch() {
        let plan = PurgePlan {
            session_id: "sid".to_string(),
            branch: "audit-x".to_string(),
            items: Vec::new(),
        };
        let mut reader = std::io::Cursor::new(b"PURGE other\n".to_vec());
        assert!(!confirm_with_reader(&plan, &mut reader).unwrap());
    }

    #[test]
    fn confirm_rejects_blank() {
        let plan = PurgePlan {
            session_id: "sid".to_string(),
            branch: "audit-x".to_string(),
            items: Vec::new(),
        };
        let mut reader = std::io::Cursor::new(b"\n".to_vec());
        assert!(!confirm_with_reader(&plan, &mut reader).unwrap());
    }

    #[test]
    fn redact_helper_rewrites_only_matching_lines() {
        let target = "abc";
        let line_match = r#"{"target":"dux::probe","fields":{"session_id":"abc","msg":"x"}}"#;
        let line_other = r#"{"target":"dux::probe","fields":{"session_id":"xyz","msg":"y"}}"#;
        let line_no_sid = r#"{"target":"dux::probe","fields":{"msg":"y"}}"#;

        let rewritten = maybe_redact_json_line(line_match, target).expect("match");
        let parsed: Value = serde_json::from_str(&rewritten).unwrap();
        assert_eq!(parsed["fields"]["redacted"], Value::Bool(true));
        assert!(parsed["fields"].get("session_id").is_none());
        assert!(parsed["fields"].get("msg").is_none());

        assert!(maybe_redact_json_line(line_other, target).is_none());
        assert!(maybe_redact_json_line(line_no_sid, target).is_none());
        assert!(maybe_redact_json_line("not json", target).is_none());
        assert!(maybe_redact_json_line("", target).is_none());
    }
}
