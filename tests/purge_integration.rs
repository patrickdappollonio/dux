//! Integration tests for `dux::purge` — audit02 Phase 10 (P0-J).
//!
//! These tests fabricate a self-contained dux installation under a
//! tempdir (sqlite, worktree, fake provider chat dirs, fake AMQ inbox,
//! fake JSON Lines log) and exercise `build_plan` + `execute` end-to-end
//! across the four scenarios listed in the plan:
//!
//!   1. `purge_removes_all_known_categories` — happy path.
//!   2. `purge_dry_run_changes_nothing`     — every step must honor `--dry-run`.
//!   3. `purge_aborts_on_wrong_confirmation` — confirm phrase mismatch.
//!   4. `purge_with_unknown_target_returns_error_not_panic` — target not found.
//!
//! Plus three extras that proved load-bearing while writing the module:
//!
//!   5. `purge_redacts_log_records_for_session` — log scoping correctness.
//!   6. `purge_skips_missing_provider_dirs`     — non-error skip path.
//!   7. `purge_executes_in_documented_order`     — worktree before sqlite.
//!
//! Tests construct `DuxPaths` and `PurgeConfig` by hand rather than going
//! through `DuxPaths::discover()` so each test is hermetic and parallel-safe.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::Utc;
use dux::config::DuxPaths;
use dux::model::{AgentSession, ProviderKind, SessionStatus};
use dux::purge::{
    self, PurgeConfig, PurgeItem, PurgeOutcome, build_plan, confirm_with_reader, execute,
    plan_for_session,
};
use dux::purge_encoding;
use dux::storage::SessionStore;

// ---------------------------------------------------------------------------
// Test harness
// ---------------------------------------------------------------------------

struct PurgeHarness {
    tmp: tempfile::TempDir,
    paths: DuxPaths,
    config: PurgeConfig,
    storage: SessionStore,
    session: AgentSession,
    /// Path to the fake `<claude_root>/projects/<encoded>` dir we
    /// expect to be deleted.
    claude_dir: PathBuf,
    /// Path to the fake `<codex_root>/projects/<encoded>` dir we
    /// expect to be deleted.
    codex_dir: PathBuf,
    /// Path to the fake AMQ inbox we expect to be deleted.
    amq_inbox: PathBuf,
    /// Path to the live `dux.log` we expect to be redacted.
    log_path: PathBuf,
}

impl PurgeHarness {
    fn new() -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        // Create a worktrees root that lives inside `root` so the
        // session's worktree path canonicalizes within the harness.
        let worktrees_root = root.join("worktrees");
        fs::create_dir_all(&worktrees_root).expect("worktrees root");

        let paths = DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: worktrees_root.clone(),
            lock_path: root.join("dux.lock"),
            root: root.clone(),
        };

        // Build provider/AMQ roots inside the tmp so we never touch
        // the real /data/state/*.
        let claude_root = root.join("claude");
        let codex_root = root.join("codex");
        let amq_root = root.join("amq");
        let config = PurgeConfig {
            provider_data_dirs: vec![
                ("claude".to_string(), claude_root.clone()),
                ("codex".to_string(), codex_root.clone()),
            ],
            amq_root: amq_root.clone(),
        };

        // Pick a worktree path. Use a constant suffix so encoded path
        // is stable across runs.
        let worktree = worktrees_root.join("audit02-x");
        fs::create_dir_all(&worktree).expect("worktree");
        // Drop a marker file so an empty-dir-quick-skip wouldn't fool
        // the test.
        fs::write(worktree.join("HEAD"), b"ref: refs/heads/audit02-x\n").expect("HEAD");

        // Encode the worktree path the way Claude Code would, then
        // pre-create the matching provider dirs with a marker file.
        let encoded =
            purge_encoding::encode_str(&worktree.to_string_lossy()).expect("encode worktree");
        let claude_dir = claude_root.join("projects").join(&encoded);
        let codex_dir = codex_root.join("projects").join(&encoded);
        fs::create_dir_all(&claude_dir).expect("claude dir");
        fs::create_dir_all(&codex_dir).expect("codex dir");
        fs::write(claude_dir.join("history.jsonl"), b"chat\n").expect("claude history");
        fs::write(codex_dir.join("history.jsonl"), b"chat\n").expect("codex history");

        // Fake AMQ inbox under <amq_root>/agents/<branch>/inbox.
        let amq_inbox = amq_root.join("agents").join("audit02-x");
        fs::create_dir_all(amq_inbox.join("inbox")).expect("amq inbox");
        fs::write(amq_inbox.join("inbox/00001.json"), b"{}\n").expect("amq msg");

        // Fake live JSON Lines log file. Note: the production code
        // matches files named `dux.log*`, so the live file must be
        // `dux.log` (not the rotated `dux.log.YYYY-MM-DD` form).
        let log_path = root.join("dux.log");
        let mut log = fs::File::create(&log_path).expect("log create");
        // Three lines: one matching session, one different session, one
        // line missing session_id entirely. Only the first should be
        // redacted.
        writeln!(
            log,
            r#"{{"target":"dux::probe","fields":{{"session_id":"sid-target","msg":"x"}}}}"#
        )
        .unwrap();
        writeln!(
            log,
            r#"{{"target":"dux::probe","fields":{{"session_id":"sid-other","msg":"y"}}}}"#
        )
        .unwrap();
        writeln!(
            log,
            r#"{{"target":"dux::probe","fields":{{"msg":"no sid"}}}}"#
        )
        .unwrap();
        log.sync_all().unwrap();
        drop(log);

        // Open the sqlite store and insert the target session.
        let storage = SessionStore::open(&paths.sessions_db_path).expect("store");
        let now = Utc::now();
        let session = AgentSession {
            id: "sid-target".to_string(),
            project_id: "proj".to_string(),
            project_path: None,
            provider: ProviderKind::new("claude"),
            source_branch: "main".to_string(),
            branch_name: "audit02-x".to_string(),
            worktree_path: worktree.to_string_lossy().to_string(),
            title: None,
            started_providers: Vec::new(),
            status: SessionStatus::Active,
            created_at: now,
            updated_at: now,
        };
        storage.upsert_session(&session).expect("upsert");

        Self {
            tmp,
            paths,
            config,
            storage,
            session,
            claude_dir,
            codex_dir,
            amq_inbox,
            log_path,
        }
    }

    fn worktree(&self) -> PathBuf {
        PathBuf::from(&self.session.worktree_path)
    }
}

fn read_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

fn count_target_records(log_text: &str, target_sid: &str) -> usize {
    log_text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter(|l| {
            let v: serde_json::Value = match serde_json::from_str(l) {
                Ok(v) => v,
                Err(_) => return false,
            };
            v.get("fields")
                .and_then(|f| f.get("session_id"))
                .and_then(|s| s.as_str())
                .map(|s| s == target_sid)
                .unwrap_or(false)
        })
        .count()
}

fn count_redacted_records(log_text: &str) -> usize {
    log_text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter(|l| {
            let v: serde_json::Value = match serde_json::from_str(l) {
                Ok(v) => v,
                Err(_) => return false,
            };
            v.get("fields")
                .and_then(|f| f.get("redacted"))
                .and_then(|b| b.as_bool())
                .unwrap_or(false)
        })
        .count()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn purge_removes_all_known_categories() {
    let h = PurgeHarness::new();
    let plan = build_plan(&h.storage, &h.paths, &h.config, &h.session.id).expect("plan");

    let report = execute(&plan, &h.storage, &h.paths, false).expect("execute");
    assert!(!report.had_errors(), "summary:\n{}", report.summary());

    // Worktree gone.
    assert!(
        !h.worktree().exists(),
        "worktree should be removed: {}",
        h.worktree().display()
    );
    // Provider dirs gone.
    assert!(
        !h.claude_dir.exists(),
        "claude dir should be removed: {}",
        h.claude_dir.display()
    );
    assert!(
        !h.codex_dir.exists(),
        "codex dir should be removed: {}",
        h.codex_dir.display()
    );
    // Provider parent (`<root>/claude/projects/`) preserved — we
    // never delete recursively above the encoded dir.
    assert!(h.claude_dir.parent().expect("parent").exists());
    // AMQ inbox gone, but the parent `agents/` dir preserved.
    assert!(!h.amq_inbox.exists());
    assert!(h.amq_inbox.parent().expect("agents parent").exists());
    // Sqlite row gone.
    let surviving = h.storage.load_sessions().expect("load");
    assert!(
        surviving.iter().all(|s| s.id != h.session.id),
        "session row should be removed"
    );
    // Log: target records redacted, others untouched.
    let log_text = read_log(&h.log_path);
    assert_eq!(
        count_target_records(&log_text, "sid-target"),
        0,
        "target session_id should not appear in log:\n{log_text}"
    );
    // Other-session record still there.
    assert_eq!(count_target_records(&log_text, "sid-other"), 1);
    // We replaced the matching record's fields with {redacted:true}.
    assert!(count_redacted_records(&log_text) >= 1);
    // The harness keeps the tempdir alive.
    let _keep = h.tmp;
}

#[test]
fn purge_dry_run_changes_nothing() {
    let h = PurgeHarness::new();
    let plan = build_plan(&h.storage, &h.paths, &h.config, &h.session.id).expect("plan");
    let log_before = read_log(&h.log_path);

    let report = execute(&plan, &h.storage, &h.paths, true).expect("execute");
    assert!(!report.had_errors());
    assert!(report.dry_run);
    assert!(
        report
            .entries
            .iter()
            .all(|(_, o)| matches!(o, PurgeOutcome::DryRun)),
        "every step must report DryRun on --dry-run; got {:?}",
        report.entries
    );

    // Nothing on disk should have moved.
    assert!(h.worktree().exists());
    assert!(h.claude_dir.exists());
    assert!(h.codex_dir.exists());
    assert!(h.amq_inbox.exists());
    let log_after = read_log(&h.log_path);
    assert_eq!(log_before, log_after, "log file must be byte-identical");
    let surviving = h.storage.load_sessions().expect("load");
    assert_eq!(surviving.len(), 1);
    let _keep = h.tmp;
}

#[test]
fn purge_aborts_on_wrong_confirmation() {
    let h = PurgeHarness::new();
    let plan = build_plan(&h.storage, &h.paths, &h.config, &h.session.id).expect("plan");

    // Wrong branch in the phrase.
    let mut wrong = std::io::Cursor::new(b"PURGE wrong\n".to_vec());
    assert!(!confirm_with_reader(&plan, &mut wrong).unwrap());

    // Different verb.
    let mut wrong = std::io::Cursor::new(b"DELETE audit02-x\n".to_vec());
    assert!(!confirm_with_reader(&plan, &mut wrong).unwrap());

    // Empty input.
    let mut empty = std::io::Cursor::new(b"\n".to_vec());
    assert!(!confirm_with_reader(&plan, &mut empty).unwrap());

    // Verify nothing was deleted (we never called `execute`).
    assert!(h.worktree().exists());
    assert!(h.claude_dir.exists());
    assert!(h.amq_inbox.exists());
    let _keep = h.tmp;
}

#[test]
fn purge_with_unknown_target_returns_error_not_panic() {
    let h = PurgeHarness::new();
    let result = build_plan(&h.storage, &h.paths, &h.config, "no-such-session");
    let err = result.expect_err("should error on unknown target");
    let msg = err.to_string();
    assert!(
        msg.contains("no session found"),
        "error message should explain: got {msg}"
    );

    // Same for an obviously-malformed input.
    let result = build_plan(&h.storage, &h.paths, &h.config, "");
    assert!(result.is_err());
    let _keep = h.tmp;
}

#[test]
fn purge_redacts_log_records_for_session() {
    let h = PurgeHarness::new();
    let plan = build_plan(&h.storage, &h.paths, &h.config, &h.session.id).expect("plan");
    let report = execute(&plan, &h.storage, &h.paths, false).expect("execute");
    assert!(!report.had_errors(), "{}", report.summary());

    let text = read_log(&h.log_path);
    // Every line should still parse as JSON.
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        let parsed: Result<serde_json::Value, _> = serde_json::from_str(line);
        assert!(parsed.is_ok(), "line broke JSON shape: {line}");
    }
    // Target session's content is gone.
    assert!(!text.contains("\"sid-target\""));
    // Other session's content survives.
    assert!(text.contains("\"sid-other\""));
    // The redacted marker is present.
    assert!(text.contains("\"redacted\":true"));
    let _keep = h.tmp;
}

#[test]
fn purge_skips_missing_provider_dirs() {
    let h = PurgeHarness::new();
    // Pre-delete the codex dir so the cascade has to skip it cleanly.
    fs::remove_dir_all(&h.codex_dir).expect("pre-delete");
    let plan = build_plan(&h.storage, &h.paths, &h.config, &h.session.id).expect("plan");
    let report = execute(&plan, &h.storage, &h.paths, false).expect("execute");
    assert!(!report.had_errors(), "{}", report.summary());

    // The codex outcome should be Skipped, not Error.
    let codex_outcome = report
        .entries
        .iter()
        .find(|(item, _)| {
            matches!(
                item,
                PurgeItem::ProviderDir {
                    provider: "codex",
                    ..
                }
            )
        })
        .map(|(_, o)| o.clone())
        .expect("codex entry");
    assert!(
        matches!(codex_outcome, PurgeOutcome::Skipped(_)),
        "expected codex to be Skipped, got {codex_outcome:?}"
    );
    let _keep = h.tmp;
}

#[test]
fn purge_executes_in_documented_order() {
    let h = PurgeHarness::new();
    let plan = plan_for_session(&h.session, &h.paths, &h.config).expect("plan");

    // Order: Worktree, ProviderDir(claude), ProviderDir(codex),
    // AmqInbox, LogScopedRedact, SqliteRow.
    let kinds: Vec<&str> = plan
        .items
        .iter()
        .map(|i| match i {
            PurgeItem::Worktree(_) => "worktree",
            PurgeItem::ProviderDir { .. } => "provider",
            PurgeItem::AmqInbox(_) => "amq",
            PurgeItem::LogScopedRedact { .. } => "log",
            PurgeItem::SqliteRow => "sqlite",
        })
        .collect();
    assert_eq!(
        kinds,
        vec!["worktree", "provider", "provider", "amq", "log", "sqlite"],
        "purge order must match the contract: worktree → providers → amq → log → sqlite"
    );

    // And `purge` is the module path so dead-code-warning doesn't fire.
    let _module_used: fn(&Path, &str) = |_, _| ();
    let _ = std::convert::identity::<&dyn std::fmt::Debug>(&purge::PurgeOutcome::Done);

    let _keep = h.tmp;
}

#[test]
fn purge_target_resolvable_by_branch_name() {
    let h = PurgeHarness::new();
    // Use the branch name rather than the session id.
    let plan = build_plan(&h.storage, &h.paths, &h.config, "audit02-x").expect("plan by branch");
    assert_eq!(plan.session_id, h.session.id);
    assert_eq!(plan.branch, "audit02-x");
    let _keep = h.tmp;
}
