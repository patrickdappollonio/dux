use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

use crate::model::{AgentSession, SessionStatus};

/// Ordered list of schema migrations. Each entry is `(version, sql)`.
///
/// Rules (see `docs/contributing/schema-policy.md`):
///
/// 1. **Migrations are immutable.** Never edit an SQL file once it has been
///    committed. To fix a bug introduced in migration N, write a new
///    migration N+1 that corrects the data.
/// 2. Append-only: new migrations are added to the end of this slice with
///    a strictly increasing `version`.
/// 3. Each migration's SQL must leave the schema in a consistent state. The
///    runner records the version in `PRAGMA user_version` after applying.
///
/// SQLite's `PRAGMA user_version` is per-database and survives the
/// `Storage::backup_to` SQLite Online Backup API used in Phase 14, so a
/// `.bak` copy automatically carries the correct schema version.
const MIGRATIONS: &[(u32, &str)] = &[
    (
        1,
        include_str!("storage/migrations/0001_initial_schema.sql"),
    ),
    // TODO(audit02 Phase 18): add `0002_session_state_v2.sql` once the
    // session state machine lands. The migration shape sketched in
    // `docs/plans/audits/audit02/19-schema-versioning.md` (§19.3) adds a
    // `state_json` column to `agent_sessions` and back-fills it from the
    // existing `status` column. Until Phase 18 merges, leaving the slot
    // empty keeps `MIGRATIONS` honest — every entry is a real migration.
];

/// Apply any migrations whose version is greater than the database's
/// current `PRAGMA user_version` and bump `user_version` after each.
///
/// Idempotent: running this on an already-migrated database is a no-op
/// because every migration's `version` is `<= user_version`. Migrations
/// run in declaration order; SQLite executes each `execute_batch` inside
/// an implicit transaction so a failure rolls back the partial DDL.
///
/// The `PRAGMA user_version = {n}` write uses `format!` because the
/// version number is a hardcoded `u32` literal from `MIGRATIONS`, never
/// user input — bound parameters are not allowed in PRAGMA statements.
fn run_migrations(conn: &Connection) -> Result<()> {
    let current: u32 = conn
        .query_row("PRAGMA user_version;", [], |r| r.get(0))
        .context("failed to read PRAGMA user_version")?;
    for (version, sql) in MIGRATIONS {
        if *version <= current {
            continue;
        }
        conn.execute_batch(sql)
            .with_context(|| format!("migration {version} failed"))?;
        conn.execute_batch(&format!("PRAGMA user_version = {version};"))
            .with_context(|| format!("failed to set user_version = {version}"))?;
        crate::logger::info(&format!(
            "storage: applied migration {version} (user_version now {version})"
        ));
    }

    // Backwards-compatibility shim for databases that predate
    // `user_version` tracking: the original `migrate()` body invoked
    // `ensure_column` here to add columns that were grafted onto the
    // schema over time (`title`, `project_path`, `started_providers`,
    // and the `session_prs.state` / `session_prs.title` defaults). After
    // migration 1 runs against a fresh DB these columns are present
    // already and the calls are no-ops. They remain to handle the case
    // where an older user upgrades from a version that had the columns
    // but not the canonical schema captured here.
    //
    // `ensure_column` is `#[deprecated]`; suppress the warning at this
    // single call site because we are explicitly keeping it for the
    // legacy upgrade path. New schema additions must go through a new
    // numbered migration file, never via `ensure_column`.
    #[allow(deprecated)]
    {
        ensure_column(conn, "agent_sessions", "title", "text")?;
        ensure_column(conn, "agent_sessions", "project_path", "text")?;
        ensure_column(
            conn,
            "agent_sessions",
            "started_providers",
            "text not null default '[]'",
        )?;
        ensure_column(conn, "session_prs", "state", "text not null default 'OPEN'")?;
        ensure_column(conn, "session_prs", "title", "text not null default ''")?;
    }
    Ok(())
}

/// A stored PR association loaded from the database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredPr {
    pub session_id: String,
    pub pr_number: u64,
    pub owner_repo: String,
    pub state: String,
    pub title: String,
}

/// SQLite-backed persistence for sessions and PR associations.
///
/// The connection is wrapped in `Arc<Mutex<Connection>>` so that it can be
/// shared with background workers (e.g. the periodic backup worker added in
/// audit02 P1-W). Internally every method locks the mutex before issuing a
/// query; the lock window is short and uncontended in practice.
pub struct SessionStore {
    conn: Arc<Mutex<Connection>>,
}

impl SessionStore {
    /// Open or create a `sessions.sqlite3` database at `path` and run
    /// startup PRAGMAs plus an integrity check.
    ///
    /// On corruption, fails fast with an error message that points at the
    /// `.bak` file so the operator knows where to recover from.
    pub fn open(path: &Path) -> Result<Self> {
        let conn =
            open_connection(path).with_context(|| format!("failed to open {}", path.display()))?;
        let store = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        store.migrate()?;
        Ok(store)
    }

    /// Acquire the underlying connection lock. Panics if the mutex is
    /// poisoned (a previous holder panicked while holding it). Public so
    /// integration tests can issue `PRAGMA` queries.
    pub fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().expect("storage mutex poisoned")
    }

    /// Online-backup the live database to `dst` using SQLite's backup API.
    ///
    /// This is safe to run concurrently with normal writes — the backup
    /// API copies pages atomically and works correctly even when WAL is
    /// enabled (a hot `cp` of the `.sqlite3` file alone would miss the
    /// `-wal` and `-shm` companions).
    pub fn backup_to(&self, dst: &Path) -> Result<()> {
        let src = self.conn();
        src.backup(rusqlite::MAIN_DB, dst, None)
            .with_context(|| format!("backup to {} failed", dst.display()))?;
        Ok(())
    }

    fn migrate(&self) -> Result<()> {
        let conn = self.conn();
        run_migrations(&conn)
    }

    /// Insert a PR association or update its state and title if it already exists.
    pub fn upsert_pr(
        &self,
        session_id: &str,
        pr_number: u64,
        owner_repo: &str,
        state: &str,
        title: &str,
    ) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            r#"
            insert into session_prs (session_id, pr_number, owner_repo, state, title)
            values (?1, ?2, ?3, ?4, ?5)
            on conflict(session_id, pr_number) do update set
                state=excluded.state,
                title=excluded.title
            "#,
            params![session_id, pr_number as i64, owner_repo, state, title],
        )?;
        Ok(())
    }

    /// Load all known PRs for a session, ordered by pr_number descending (latest first).
    pub fn load_prs(&self, session_id: &str) -> Result<Vec<StoredPr>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            r#"
            select pr_number, owner_repo, state, title
            from session_prs
            where session_id = ?1
            order by pr_number desc
            "#,
        )?;
        let sid = session_id.to_string();
        let rows = stmt.query_map(params![session_id], |row| {
            Ok(StoredPr {
                session_id: sid.clone(),
                pr_number: row.get::<_, i64>(0)? as u64,
                owner_repo: row.get(1)?,
                state: row.get(2)?,
                title: row.get(3)?,
            })
        })?;
        let mut prs = Vec::new();
        for row in rows {
            prs.push(row?);
        }
        Ok(prs)
    }

    /// Load the latest (highest-numbered) PR for each session that has at least one.
    pub fn load_all_latest_prs(&self) -> Result<Vec<StoredPr>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            r#"
            select session_id, pr_number, owner_repo, state, title
            from session_prs
            where (session_id, pr_number) in (
                select session_id, max(pr_number) from session_prs group by session_id
            )
            "#,
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(StoredPr {
                session_id: row.get(0)?,
                pr_number: row.get::<_, i64>(1)? as u64,
                owner_repo: row.get(2)?,
                state: row.get(3)?,
                title: row.get(4)?,
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    pub fn upsert_session(&self, session: &AgentSession) -> Result<()> {
        let conn = self.conn();
        conn.execute(
            r#"
            insert into agent_sessions
                (id, project_id, project_path, provider, source_branch, branch_name, worktree_path, title, started_providers, status, created_at, updated_at)
            values
                (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)
            on conflict(id) do update set
                project_path=excluded.project_path,
                provider=excluded.provider,
                source_branch=excluded.source_branch,
                branch_name=excluded.branch_name,
                worktree_path=excluded.worktree_path,
                title=excluded.title,
                started_providers=excluded.started_providers,
                status=excluded.status,
                updated_at=excluded.updated_at
            "#,
            params![
                session.id,
                session.project_id,
                session.project_path,
                session.provider.as_str(),
                session.source_branch,
                session.branch_name,
                session.worktree_path,
                session.title,
                serialize_started_providers(&session.started_providers),
                session.status.as_str(),
                session.created_at.to_rfc3339(),
                session.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn load_sessions(&self) -> Result<Vec<AgentSession>> {
        let conn = self.conn();
        let mut stmt = conn.prepare(
            r#"
            select id, project_id, provider, source_branch, branch_name, worktree_path, title, project_path, started_providers, status, created_at, updated_at
            from agent_sessions
            order by updated_at desc
            "#,
        )?;
        let rows = stmt.query_map([], |row| {
            let started_providers: String = row.get(8)?;
            let created_at: String = row.get(10)?;
            let updated_at: String = row.get(11)?;
            Ok(AgentSession {
                id: row.get(0)?,
                project_id: row.get::<_, String>(1).unwrap_or_default(),
                provider: crate::model::ProviderKind::from_str(row.get::<_, String>(2)?.as_str()),
                source_branch: row.get(3)?,
                branch_name: row.get(4)?,
                worktree_path: row.get(5)?,
                title: row.get(6)?,
                project_path: row.get(7)?,
                started_providers: parse_started_providers(&started_providers),
                status: SessionStatus::from_str(row.get::<_, String>(9)?.as_str()),
                created_at: parse_time(&created_at).unwrap_or_else(Utc::now),
                updated_at: parse_time(&updated_at).unwrap_or_else(Utc::now),
            })
        })?;

        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row?);
        }
        Ok(sessions)
    }

    pub fn delete_session(&self, id: &str) -> Result<()> {
        let conn = self.conn();
        conn.execute("delete from agent_sessions where id = ?1", params![id])?;
        Ok(())
    }

    /// Returns a clone of the inner `Arc<Mutex<Connection>>` so a handle can
    /// be shared with background workers (e.g. backup worker) without
    /// transferring ownership.
    #[allow(dead_code)] // consumed by spawn_backup_worker once wired (Phase 14 step 14.3)
    pub fn shared_conn(&self) -> Arc<Mutex<Connection>> {
        Arc::clone(&self.conn)
    }
}

/// Open a SQLite connection at `path` with the dux startup PRAGMAs and a
/// fast-fail integrity check.
///
/// The PRAGMA batch enables WAL journaling (so readers don't block writers
/// and vice-versa), `synchronous = NORMAL` (the right durability/perf
/// balance for a TUI workload — `OFF` would risk data loss on power-loss),
/// memory temp tables, a 128 MiB mmap window, an autocheckpoint at 1000
/// pages (~4 MiB at default page size), and foreign-key enforcement.
fn open_connection(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)?;
    conn.execute_batch(
        r#"
        PRAGMA journal_mode = WAL;
        PRAGMA synchronous = NORMAL;
        PRAGMA temp_store = MEMORY;
        PRAGMA mmap_size = 134217728;
        PRAGMA wal_autocheckpoint = 1000;
        PRAGMA foreign_keys = ON;
        "#,
    )?;

    // Skip the integrity check for in-memory databases used by unit tests:
    // they're always pristine, and PRAGMA integrity_check on a freshly
    // opened :memory: DB can return rows that confuse the path-display
    // logic (which assumes a real file path).
    if path != Path::new(":memory:") {
        let mut stmt = conn.prepare("PRAGMA integrity_check;")?;
        let result: String = stmt.query_row([], |r| r.get(0))?;
        if result != "ok" {
            anyhow::bail!(
                "sqlite integrity check failed for {}: {result}; restore from {}.bak",
                path.display(),
                path.display()
            );
        }
    }

    Ok(conn)
}

fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn serialize_started_providers(started_providers: &[String]) -> String {
    serde_json::to_string(started_providers).unwrap_or_else(|_| "[]".to_string())
}

fn parse_started_providers(value: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(value).unwrap_or_default()
}

/// Opens an in-memory session store for tests.
#[cfg(test)]
fn test_store() -> SessionStore {
    SessionStore::open(std::path::Path::new(":memory:")).unwrap()
}

/// Builds a minimal `AgentSession` with the given id, `created_at`, and `updated_at`.
#[cfg(test)]
fn test_session(
    id: &str,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
) -> crate::model::AgentSession {
    crate::model::AgentSession {
        id: id.to_string(),
        project_id: "proj".to_string(),
        project_path: None,
        provider: crate::model::ProviderKind::new("claude"),
        source_branch: "main".to_string(),
        branch_name: format!("branch-{id}"),
        worktree_path: format!("/tmp/{id}"),
        title: None,
        started_providers: Vec::new(),
        status: SessionStatus::Active,
        created_at,
        updated_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    #[test]
    fn load_sessions_ordered_by_updated_at_desc() {
        let store = test_store();
        let now = Utc::now();

        // Insert three sessions with different updated_at values.
        // oldest updated first, newest updated last.
        let s1 = test_session("a", now - Duration::hours(3), now - Duration::hours(3));
        let s2 = test_session("b", now - Duration::hours(2), now - Duration::hours(1));
        let s3 = test_session("c", now - Duration::hours(1), now - Duration::hours(2));

        store.upsert_session(&s1).unwrap();
        store.upsert_session(&s2).unwrap();
        store.upsert_session(&s3).unwrap();

        let loaded = store.load_sessions().unwrap();
        let ids: Vec<&str> = loaded.iter().map(|s| s.id.as_str()).collect();

        // s2 has the most recent updated_at, then s3, then s1.
        assert_eq!(ids, vec!["b", "c", "a"]);
    }

    #[test]
    fn upsert_without_changing_updated_at_preserves_order() {
        let store = test_store();
        let now = Utc::now();

        let s1 = test_session("a", now - Duration::hours(2), now - Duration::hours(2));
        let s2 = test_session("b", now - Duration::hours(1), now - Duration::hours(1));

        store.upsert_session(&s1).unwrap();
        store.upsert_session(&s2).unwrap();

        // Re-upsert s1 with same timestamps (simulating a no-op status update).
        store.upsert_session(&s1).unwrap();

        let loaded = store.load_sessions().unwrap();
        let ids: Vec<&str> = loaded.iter().map(|s| s.id.as_str()).collect();

        // Order unchanged: s2 still has the more recent updated_at.
        assert_eq!(ids, vec!["b", "a"]);
    }

    #[test]
    fn started_providers_round_trip() {
        let store = test_store();
        let now = Utc::now();
        let mut session = test_session("started", now, now);
        session.started_providers = vec!["claude".to_string(), "codex".to_string()];

        store.upsert_session(&session).unwrap();

        let loaded = store.load_sessions().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(
            loaded[0].started_providers,
            vec!["claude".to_string(), "codex".to_string()]
        );
    }
}

#[cfg(test)]
mod ensure_column_validation_tests {
    use super::*;

    #[test]
    fn ensure_column_rejects_injection() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (id INTEGER);").unwrap();

        // Hostile column name: classic SQL-injection payload.
        #[allow(deprecated)]
        let bad_column = ensure_column(&conn, "t", "x; DROP TABLE t; --", "TEXT");
        assert!(
            bad_column.is_err(),
            "injection in column should be rejected"
        );

        // Hostile sql_type: trailing statement plus comment.
        #[allow(deprecated)]
        let bad_type = ensure_column(&conn, "t", "ok_name", "TEXT; DROP TABLE t; --");
        assert!(
            bad_type.is_err(),
            "injection in sql_type should be rejected"
        );

        // Hostile table name: starts with a digit (not an identifier).
        #[allow(deprecated)]
        let bad_table = ensure_column(&conn, "1bad", "ok_name", "TEXT");
        assert!(
            bad_table.is_err(),
            "non-identifier table should be rejected"
        );

        // Hostile sql_type: SQL comment in the middle.
        #[allow(deprecated)]
        let comment_type = ensure_column(&conn, "t", "ok_name", "TEXT /* sneaky */");
        assert!(
            comment_type.is_err(),
            "block-comment in sql_type should be rejected"
        );

        // Confirm the table is still intact (no DROP got through).
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='t'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1, "table 't' must survive the rejected payloads");

        // Sanity: a clean call still succeeds.
        #[allow(deprecated)]
        let ok = ensure_column(&conn, "t", "extra", "TEXT");
        assert!(ok.is_ok(), "well-formed inputs should still work");
    }

    #[test]
    fn ensure_column_accepts_legacy_defaults() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE t (id INTEGER);").unwrap();
        // These exact strings appear in the legacy ensure_column call sites
        // inside `run_migrations` and must keep working unchanged.
        #[allow(deprecated)]
        {
            ensure_column(
                &conn,
                "t",
                "started_providers",
                "text not null default '[]'",
            )
            .unwrap();
            ensure_column(&conn, "t", "state", "text not null default 'OPEN'").unwrap();
            ensure_column(&conn, "t", "title", "text not null default ''").unwrap();
        }
    }

    #[test]
    fn is_safe_ident_matches_pattern() {
        assert!(is_safe_ident("agent_sessions"));
        assert!(is_safe_ident("_under"));
        assert!(is_safe_ident("Col1"));
        assert!(!is_safe_ident(""));
        assert!(!is_safe_ident("1col"));
        assert!(!is_safe_ident("col-name"));
        assert!(!is_safe_ident("col;drop"));
        assert!(!is_safe_ident("col name"));
    }
}

#[cfg(test)]
mod pr_tests {
    use super::*;
    use chrono::Duration;

    fn spr(sid: &str, num: u64, repo: &str, state: &str, title: &str) -> StoredPr {
        StoredPr {
            session_id: sid.to_string(),
            pr_number: num,
            owner_repo: repo.to_string(),
            state: state.to_string(),
            title: title.to_string(),
        }
    }

    #[test]
    fn upsert_and_load_prs() {
        let store = test_store();
        let now = Utc::now();
        let s = test_session("s1", now, now);
        store.upsert_session(&s).unwrap();

        store
            .upsert_pr("s1", 10, "owner/repo", "OPEN", "First PR")
            .unwrap();
        store
            .upsert_pr("s1", 20, "owner/repo", "OPEN", "Second PR")
            .unwrap();
        store
            .upsert_pr("s1", 15, "owner/repo", "MERGED", "Middle PR")
            .unwrap();

        let prs = store.load_prs("s1").unwrap();
        assert_eq!(prs.len(), 3);
        assert_eq!(prs[0], spr("s1", 20, "owner/repo", "OPEN", "Second PR"));
        assert_eq!(prs[1], spr("s1", 15, "owner/repo", "MERGED", "Middle PR"));
        assert_eq!(prs[2], spr("s1", 10, "owner/repo", "OPEN", "First PR"));
    }

    #[test]
    fn upsert_pr_updates_state_and_title() {
        let store = test_store();
        let now = Utc::now();
        let s = test_session("s1", now, now);
        store.upsert_session(&s).unwrap();

        store
            .upsert_pr("s1", 42, "owner/repo", "OPEN", "My PR")
            .unwrap();
        store
            .upsert_pr("s1", 42, "owner/repo", "MERGED", "My PR (updated)")
            .unwrap();

        let prs = store.load_prs("s1").unwrap();
        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0].state, "MERGED");
        assert_eq!(prs[0].title, "My PR (updated)");
    }

    #[test]
    fn load_all_latest_prs() {
        let store = test_store();
        let now = Utc::now();
        let s1 = test_session("s1", now, now);
        let s2 = test_session("s2", now - Duration::hours(1), now - Duration::hours(1));
        store.upsert_session(&s1).unwrap();
        store.upsert_session(&s2).unwrap();

        store
            .upsert_pr("s1", 10, "owner/repo", "CLOSED", "Old PR")
            .unwrap();
        store
            .upsert_pr("s1", 20, "owner/repo", "MERGED", "Latest PR")
            .unwrap();
        store
            .upsert_pr("s2", 5, "other/repo", "OPEN", "Other PR")
            .unwrap();

        let latest = store.load_all_latest_prs().unwrap();
        assert_eq!(latest.len(), 2);
        assert!(latest.contains(&spr("s1", 20, "owner/repo", "MERGED", "Latest PR")));
        assert!(latest.contains(&spr("s2", 5, "other/repo", "OPEN", "Other PR")));
    }
}

/// Idempotently `ALTER TABLE ... ADD COLUMN` if `column` is missing.
///
/// **Deprecated.** This shim predates the `MIGRATIONS` registry and is
/// retained only so that databases created before `PRAGMA user_version`
/// was wired up keep working. New schema changes must be added as a
/// numbered migration in `src/storage/migrations/` and listed in
/// [`MIGRATIONS`]; see `docs/contributing/schema-policy.md` for the
/// rules on naming, immutability, and review requirements.
///
/// As defense-in-depth (audit02 P1-K), `table` and `column` must match
/// `[A-Za-z_][A-Za-z0-9_]*` and `sql_type` must be one of the SQLite
/// storage classes (optionally followed by a constraint clause). Inputs
/// that fail these checks return an error rather than splicing into the
/// generated DDL — this guarantees we never SQL-inject ourselves even if
/// a future caller forgets that all three arguments are interpolated raw.
#[deprecated(
    note = "schema changes must go through src/storage/migrations/ — see docs/contributing/schema-policy.md"
)]
fn ensure_column(conn: &Connection, table: &str, column: &str, sql_type: &str) -> Result<()> {
    if !is_safe_ident(table) {
        anyhow::bail!("ensure_column: rejected unsafe table name {table:?}");
    }
    if !is_safe_ident(column) {
        anyhow::bail!("ensure_column: rejected unsafe column name {column:?}");
    }
    if !is_safe_sql_type(sql_type) {
        anyhow::bail!("ensure_column: rejected unsafe sql_type {sql_type:?}");
    }

    let mut stmt = conn.prepare(&format!("pragma table_info({table})"))?;
    let existing = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if existing.iter().any(|name| name == column) {
        return Ok(());
    }
    conn.execute(
        &format!("alter table {table} add column {column} {sql_type}"),
        [],
    )?;
    Ok(())
}

/// Return `true` when `s` looks like a safe SQL identifier:
/// non-empty, starts with an ASCII letter or `_`, and contains only
/// ASCII alphanumerics and `_` thereafter.
///
/// Free-standing so the rejection test can call it without pulling the
/// whole `ensure_column` body through.
fn is_safe_ident(s: &str) -> bool {
    let mut chars = s.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Return `true` when `s` is one of SQLite's storage classes (optionally
/// followed by a column constraint clause separated by ASCII whitespace).
/// This deliberately accepts the constraint suffix because the legacy
/// `ensure_column` call sites use values like
/// `"text not null default '[]'"`. Anything containing a semicolon, a
/// SQL comment marker, or a quote is rejected.
fn is_safe_sql_type(s: &str) -> bool {
    if s.is_empty() {
        return false;
    }
    // Reject anything that could end the current statement or smuggle a
    // comment past it. SQLite treats `--` and `/*` as comment markers and
    // `;` as a statement terminator.
    if s.contains(';')
        || s.contains("--")
        || s.contains("/*")
        || s.contains('"')
        || s.contains('\'')
    {
        // The legacy default literal `'[]'` *does* contain `'`, so we
        // re-allow that exact constraint shape further down. Anything
        // else is rejected here.
        return is_known_legacy_default_literal(s);
    }
    const STORAGE_CLASSES: &[&str] = &["TEXT", "INTEGER", "REAL", "BLOB", "NULL", "NUMERIC"];
    let upper = s.trim().to_ascii_uppercase();
    STORAGE_CLASSES
        .iter()
        .any(|class| upper == *class || upper.starts_with(&format!("{class} ")))
}

/// Whitelist for the small set of legacy `ensure_column` defaults that
/// embed a single-quoted SQL literal — currently `text not null default
/// '[]'` and `text not null default ''`. Keeps the broader
/// quote-rejection in [`is_safe_sql_type`] intact while preserving the
/// pre-Phase-19 `started_providers` / `session_prs.title` schema shims.
fn is_known_legacy_default_literal(s: &str) -> bool {
    let normalized: String = s
        .trim()
        .to_ascii_lowercase()
        .split_ascii_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    matches!(
        normalized.as_str(),
        "text not null default '[]'" | "text not null default ''" | "text not null default 'open'"
    )
}
