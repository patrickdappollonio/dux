use std::collections::BTreeMap;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

use crate::config::ProjectConfig;
use crate::model::{AgentSession, AgentTab, ProviderKind, SessionStatus};

/// A stored PR association loaded from the database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredPr {
    pub session_id: String,
    pub pr_number: u64,
    pub host: String,
    pub owner_repo: String,
    pub state: String,
    pub title: String,
    pub url: String,
}

/// The `app_state` key holding the last dux version whose first-load screen was
/// shown. One key, one meaning; see [`SessionStore::last_seen_version`].
const LAST_SEEN_VERSION_KEY: &str = "last_seen_version";

pub struct SessionStore {
    conn: Connection,
}

/// SQLite names its journal sidecars by APPENDING to the database's file name,
/// so `sessions.sqlite3` gets `sessions.sqlite3-wal`. That is a suffix on the
/// whole name, not a new extension, which is why this appends to the OS string
/// rather than going through `set_extension`.
fn sidecar_path(db: &std::path::Path, suffix: &str) -> std::path::PathBuf {
    let mut name = db.as_os_str().to_os_string();
    name.push(suffix);
    std::path::PathBuf::from(name)
}

impl SessionStore {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let conn =
            Connection::open(path).with_context(|| format!("failed to open {}", path.display()))?;
        // The engine keeps one connection open for the lifetime of the process
        // while background workers open their own to the same file. WAL lets a
        // writer and readers proceed without blocking each other, and a busy
        // timeout turns the rare writer/writer overlap into a short wait-and-retry
        // instead of an immediate `SQLITE_BUSY` failure (the default timeout is 0).
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        // `journal_mode` returns the resulting mode as a row, so use a statement
        // that tolerates it (a `:memory:` DB stays in "memory" mode — a harmless
        // no-op). `execute_batch` ignores the returned row.
        conn.execute_batch("PRAGMA journal_mode=WAL;")?;
        // The database mirrors the same per-project `env` map that made
        // `config.toml` 0600, so it gets the same mode. SQLite creates the file
        // (and, after the WAL pragma, its `-wal`/`-shm` sidecars) itself at the
        // umask default and offers no API to choose their mode, so the only
        // thing dux can do here is tighten afterwards. The sidecars can also be
        // recreated at any later point, which is why the owner-only CONFIG
        // DIRECTORY, not this, is what actually closes the gap; see
        // `crate::file_modes`. Tightening runs on every open so a database left
        // 0644 by an older installation is corrected. A failure is not fatal:
        // an unwritable mode must not stop dux from reading its own sessions.
        for path in [
            path.to_path_buf(),
            sidecar_path(path, "-wal"),
            sidecar_path(path, "-shm"),
        ] {
            crate::file_modes::restrict_to_owner_best_effort(&path, "session database");
        }
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<()> {
        self.conn.execute_batch(
            r#"
            create table if not exists agent_sessions (
                id text primary key,
                project_id text not null,
                provider text not null,
                source_branch text not null,
                branch_name text not null,
                initial_branch text not null default '',
                worktree_path text not null,
                title text,
                project_path text,
                status text not null,
                created_at text not null,
                updated_at text not null
            );
            "#,
        )?;
        self.conn.execute_batch(
            r#"
            create table if not exists projects (
                id text primary key,
                path text not null unique,
                name text,
                default_provider text,
                leading_branch text,
                auto_reopen_agents integer,
                startup_command text,
                env text not null default '{}',
                sort_order integer not null default 0,
                created_at text not null,
                updated_at text not null
            );
            "#,
        )?;
        ensure_column(&self.conn, "projects", "name", "text")?;
        ensure_column(&self.conn, "projects", "default_provider", "text")?;
        ensure_column(&self.conn, "projects", "leading_branch", "text")?;
        ensure_column(&self.conn, "projects", "auto_reopen_agents", "integer")?;
        ensure_column(&self.conn, "projects", "startup_command", "text")?;
        ensure_column(&self.conn, "projects", "env", "text not null default '{}'")?;
        ensure_column(
            &self.conn,
            "projects",
            "sort_order",
            "integer not null default 0",
        )?;
        ensure_column(
            &self.conn,
            "projects",
            "created_at",
            "text not null default ''",
        )?;
        ensure_column(
            &self.conn,
            "projects",
            "updated_at",
            "text not null default ''",
        )?;
        ensure_column(&self.conn, "agent_sessions", "title", "text")?;
        // The immutable branch an agent was created on. Additive column with a
        // '' default so old rows and inserts by an older binary still succeed.
        //
        // The ALTER runs in AUTOCOMMIT (on `&self.conn`), NOT inside the backfill
        // transaction below, so the duplicate-column tolerance in `ensure_column`
        // works: two connections opening at first-boot-after-upgrade can race the
        // ALTER, and the loser sees SQLite's "duplicate column name" error (which
        // `is_duplicate_column_error` swallows as `Ok(false)`). Wrapping the ALTER
        // in a transaction instead would make the loser raise SQLITE_BUSY_SNAPSHOT,
        // which that classifier does NOT match — hard-failing `open()`.
        let initial_branch_added = ensure_column(
            &self.conn,
            "agent_sessions",
            "initial_branch",
            "text not null default ''",
        )?;
        // Only the backfill UPDATEs run in a transaction so a crash mid-backfill
        // rolls them back and the step is retried cleanly on the next boot (the
        // idempotent/ungated portion below self-heals a partially-applied run).
        // Capture the one-time freeze count and log it ONLY after the commit
        // succeeds, so the success line can never claim a migration that a commit
        // failure actually rolled back.
        let frozen_titles = {
            let tx = self.conn.unchecked_transaction()?;
            // IDEMPOTENT, UNGATED backfill: freeze the birth branch for any row
            // that still lacks one. The WHERE clause is self-limiting (new rows
            // always record a genuine `initial_branch` at creation, so they are
            // never empty), so running it on every `migrate()` is a no-op once
            // healed — but it self-heals rows stranded by a crash mid-migration
            // or a downgrade→re-upgrade window. The true original may already be
            // lost to prior drift, so freeze the current branch as the recorded
            // initial (best available).
            tx.execute(
                "update agent_sessions set initial_branch = branch_name \
                 where initial_branch = '' or initial_branch is null",
                [],
            )?;
            // ONE-TIME backfill, gated on the FIRST appearance of the
            // `initial_branch` column so it runs exactly once — mirroring the
            // gated `sort_order` backfill below. `migrate()` runs on every
            // `open()`, which happens on every startup AND every background
            // project-persistence / config-reload; an unconditional `title`
            // backfill would re-freeze the intentionally-NULL `title` of every
            // auto-named agent on each open, silently pinning it so the display
            // can no longer track the branch. `title IS NULL` is a legitimate
            // ongoing state for auto-named agents, so this must never re-run.
            //
            // Migration asymmetry (intentional): legacy pet-named agents present
            // at the moment of the one-time upgrade get their current name frozen
            // into `title` here, so their display can never drift with the
            // branch again. Agents auto-named AFTER the upgrade keep `title` NULL
            // and their display continues to track `branch_name` (drift shown via
            // `initial_branch`). A future reader seeing this split should know it
            // is the deliberate freeze tradeoff, not an oversight.
            let frozen = if initial_branch_added {
                Some(tx.execute(
                    "update agent_sessions set title = branch_name where title is null",
                    [],
                )?)
            } else {
                None
            };
            tx.commit()?;
            frozen
        };
        if let Some(frozen) = frozen_titles {
            crate::logger::info(&format!(
                "one-time migration: froze title for {frozen} legacy session(s)"
            ));
        }
        ensure_column(&self.conn, "agent_sessions", "project_path", "text")?;
        ensure_column(
            &self.conn,
            "agent_sessions",
            "started_providers",
            "text not null default '[]'",
        )?;
        ensure_column(
            &self.conn,
            "agent_sessions",
            "desired_running",
            "integer not null default 0",
        )?;
        ensure_column(
            &self.conn,
            "agent_sessions",
            "auto_reopen_enabled",
            "integer not null default 1",
        )?;
        // Persisted display order for agent sessions. The ALTER runs in AUTOCOMMIT
        // (same duplicate-column-tolerance rationale as `initial_branch` above);
        // the backfill numbers positions GLOBALLY (agents are one flat list) from
        // the legacy `updated_at DESC` order so the visible order is preserved
        // exactly across the upgrade, and it runs inside its own transaction (see
        // `backfill_session_sort_order`).
        //
        // Retryable: run the backfill when the column was just added, OR when a
        // prior crash stranded the table in the gap between the (autocommitted)
        // ALTER and the backfill — detected by `session_sort_order_needs_backfill`
        // as "some project has 2+ sessions all still at the default 0". Gating on
        // `ensure_column` alone (the previous behavior) left a crash-stranded
        // table pinned at sort_order=0 forever, because the next boot sees the
        // column present and skips the backfill permanently.
        let sort_order_added = ensure_column(
            &self.conn,
            "agent_sessions",
            "sort_order",
            "integer not null default 0",
        )?;
        if sort_order_added || self.session_sort_order_needs_backfill()? {
            self.backfill_session_sort_order()?;
        }
        // Per-agent remembered focused tab (derived runtime/UI state, never config).
        // Additive, NULL default = "no memory" = falls back to the session-slot tab.
        // Deliberately OMITTED from `upsert_session`'s SET/INSERT lists (same rationale
        // as `sort_order`): a dedicated setter owns it so status/config churn can't reset it.
        ensure_column(&self.conn, "agent_sessions", "last_focused_tab", "text")?;
        self.conn.execute_batch(
            r#"
            create table if not exists session_prs (
                session_id text not null,
                pr_number integer not null,
                host text not null default 'github.com',
                owner_repo text not null,
                state text not null default 'OPEN',
                primary key (session_id, pr_number),
                foreign key (session_id) references agent_sessions(id) on delete cascade
            );
            "#,
        )?;
        ensure_column(
            &self.conn,
            "session_prs",
            "host",
            "text not null default 'github.com'",
        )?;
        ensure_column(
            &self.conn,
            "session_prs",
            "state",
            "text not null default 'OPEN'",
        )?;
        ensure_column(
            &self.conn,
            "session_prs",
            "title",
            "text not null default ''",
        )?;
        ensure_column(&self.conn, "session_prs", "url", "text not null default ''")?;
        // Per-session monotonic changed-files revision counter (server mode).
        // Separate from the session record so it is purely housekeeping: a single
        // chokepoint that hands out a strictly-increasing `rev` per session,
        // persisted so it survives restarts (never resets to a lower value). The
        // row is removed when the session is deleted (see `delete_session`).
        self.conn.execute_batch(
            r#"
            create table if not exists changes_rev (
                session_id text primary key,
                rev integer not null
            );
            "#,
        )?;
        // extra tabs (secondary provider tabs). Additive and backward
        // compatible: existing databases start with zero rows and behave exactly
        // as before. The session-slot tab has no row here — it is derived from the
        // `agent_sessions` row. Rows are removed when the owning session (or its
        // project) is deleted (see `delete_session`/`remove_project_records`).
        self.conn.execute_batch(
            r#"
            create table if not exists agent_tabs (
                id text primary key,
                session_id text not null,
                provider text not null,
                sort_order integer not null default 0,
                created_at text not null
            );
            create index if not exists idx_agent_tabs_session on agent_tabs(session_id);
            "#,
        )?;
        // Small key/value bag for whole-app derived state that belongs to no
        // session and no project, and that must be SHARED by the TUI and the
        // web (see `last_seen_version`: dismissing the what's-new screen in one
        // surface dismisses it in the other). Additive and backward compatible:
        // existing databases start with zero rows. Keep it deliberately small —
        // per-entity state belongs in its own purpose-built table, exactly like
        // `changes_rev`.
        self.conn.execute_batch(
            r#"
            create table if not exists app_state (
                key text primary key,
                value text not null
            );
            "#,
        )?;
        Ok(())
    }

    /// Read one [`app_state`](Self::set_app_state) value.
    pub fn app_state(&self, key: &str) -> Result<Option<String>> {
        let mut stmt = self
            .conn
            .prepare("select value from app_state where key = ?1")?;
        let mut rows = stmt.query(params![key])?;
        Ok(match rows.next()? {
            Some(row) => Some(row.get(0)?),
            None => None,
        })
    }

    /// Write one app-wide value, replacing any previous value for the key.
    pub fn set_app_state(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "insert into app_state(key, value) values(?1, ?2) \
             on conflict(key) do update set value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// The dux version whose first-load screen the user last saw, or `None` when
    /// dux has never shown one (the very first launch).
    ///
    /// Derived UI state, so it lives here and never in portable config. Shared
    /// by both surfaces on purpose: see [`crate::first_load`].
    pub fn last_seen_version(&self) -> Result<Option<String>> {
        self.app_state(LAST_SEEN_VERSION_KEY)
    }

    /// Record `version` as seen, so its what's-new screen does not reappear.
    pub fn set_last_seen_version(&self, version: &str) -> Result<()> {
        self.set_app_state(LAST_SEEN_VERSION_KEY, version)
    }

    /// Insert a new extra tab row.
    pub fn insert_agent_tab(&self, tab: &AgentTab) -> Result<()> {
        self.conn.execute(
            "insert into agent_tabs (id, session_id, provider, sort_order, created_at) \
             values (?1, ?2, ?3, ?4, ?5)",
            params![
                tab.id,
                tab.session_id,
                tab.provider.as_str(),
                tab.sort_order,
                tab.created_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    /// Remove a single extra tab row (closing an extra tab).
    pub fn delete_agent_tab(&self, tab_id: &str) -> Result<()> {
        let affected = self
            .conn
            .execute("delete from agent_tabs where id = ?1", params![tab_id])?;
        if affected == 0 {
            crate::logger::warn(&format!(
                "delete_agent_tab affected no rows for {tab_id} — the in-memory tab map and \
                 SQLite may have diverged",
            ));
        }
        Ok(())
    }

    /// Load every extra tab, ordered so a session's tabs come out in a stable
    /// creation order. A cheap orphan sweep first drops any rows whose owning
    /// session no longer exists — belt-and-suspenders for tabs an older binary
    /// (which predates this table) could have left behind when deleting a session.
    pub fn load_agent_tabs(&self) -> Result<Vec<AgentTab>> {
        self.conn.execute(
            "delete from agent_tabs where session_id not in (select id from agent_sessions)",
            [],
        )?;
        let mut stmt = self.conn.prepare(
            "select id, session_id, provider, sort_order, created_at \
             from agent_tabs order by session_id, sort_order, created_at",
        )?;
        let rows = stmt.query_map([], |row| {
            let created_at: String = row.get(4)?;
            Ok(AgentTab {
                id: row.get(0)?,
                session_id: row.get(1)?,
                provider: ProviderKind::from_str(row.get::<_, String>(2)?.as_str()),
                sort_order: row.get(3)?,
                created_at: parse_time(&created_at).unwrap_or_else(Utc::now),
            })
        })?;
        let mut tabs = Vec::new();
        for row in rows {
            tabs.push(row?);
        }
        Ok(tabs)
    }

    /// Retarget an extra tab's provider (effective on its next launch).
    pub fn update_agent_tab_provider(&self, tab_id: &str, provider: &str) -> Result<()> {
        let affected = self.conn.execute(
            "update agent_tabs set provider = ?2 where id = ?1",
            params![tab_id, provider],
        )?;
        if affected == 0 {
            crate::logger::warn(&format!(
                "update_agent_tab_provider affected no rows for {tab_id} — the in-memory tab map \
                 and SQLite may have diverged",
            ));
        }
        Ok(())
    }

    /// The largest `sort_order` among a session's extra tabs, if any — used to
    /// append a new tab after the existing ones.
    pub fn max_tab_sort_order(&self, session_id: &str) -> Result<Option<i64>> {
        let value: Option<i64> = self.conn.query_row(
            "select max(sort_order) from agent_tabs where session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )?;
        Ok(value)
    }

    /// Number of extra tabs for one session (excludes the session-slot tab, which has
    /// no row). The per-agent cap counts Main as tab 1, so the create path checks
    /// `count_agent_tabs(session_id) + 1 >= max_per_agent`.
    pub fn count_agent_tabs(&self, session_id: &str) -> Result<i64> {
        let count: i64 = self.conn.query_row(
            "select count(*) from agent_tabs where session_id = ?1",
            params![session_id],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Atomically bump and return the next changed-files revision for `session_id`.
    ///
    /// First call for a session returns `1`; each subsequent call returns the
    /// previous value plus one. Implemented as a single upsert with `RETURNING`
    /// (supported by the bundled SQLite in `rusqlite`) so it is the one chokepoint
    /// that guarantees a strictly-increasing, persisted `rev` per session — the
    /// ordering/dedup token web clients apply to changed-files GETs and events.
    pub fn next_changes_rev(&self, session_id: &str) -> rusqlite::Result<u64> {
        let rev: i64 = self.conn.query_row(
            "insert into changes_rev(session_id, rev) values(?1, 1) \
             on conflict(session_id) do update set rev = rev + 1 returning rev",
            params![session_id],
            |row| row.get(0),
        )?;
        Ok(rev as u64)
    }

    /// Insert or update a project. If a project with the same path already
    /// exists under a different id, keep the existing id so sessions remain
    /// attached and refresh the editable metadata.
    pub fn upsert_project(&self, project: &ProjectConfig) -> Result<()> {
        let sort_order = self.next_project_sort_order()?;
        self.upsert_project_at(project, sort_order)
    }

    pub fn upsert_project_at(&self, project: &ProjectConfig, sort_order: i64) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        let updated = self.conn.execute(
            r#"
            update projects
            set path = ?2,
                name = ?3,
                default_provider = ?4,
                leading_branch = ?5,
                auto_reopen_agents = ?6,
                startup_command = ?7,
                env = ?8,
                sort_order = ?9,
                updated_at = ?10
            where id = ?1
            "#,
            params![
                project.id,
                project.path,
                project.name,
                project.default_provider,
                project.leading_branch,
                project.auto_reopen_agents,
                project.startup_command,
                serialize_project_env(&project.env),
                sort_order,
                now,
            ],
        )?;
        if updated > 0 {
            return Ok(());
        }

        self.conn.execute(
            r#"
            insert into projects
                (id, path, name, default_provider, leading_branch, auto_reopen_agents, startup_command, env, sort_order, created_at, updated_at)
            values
                (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?10)
            on conflict(path) do update set
                name=excluded.name,
                default_provider=excluded.default_provider,
                leading_branch=excluded.leading_branch,
                auto_reopen_agents=excluded.auto_reopen_agents,
                startup_command=excluded.startup_command,
                env=excluded.env,
                sort_order=excluded.sort_order,
                updated_at=excluded.updated_at
            "#,
            params![
                project.id,
                project.path,
                project.name,
                project.default_provider,
                project.leading_branch,
                project.auto_reopen_agents,
                project.startup_command,
                serialize_project_env(&project.env),
                sort_order,
                now,
            ],
        )?;
        Ok(())
    }

    fn next_project_sort_order(&self) -> Result<i64> {
        self.conn
            .query_row(
                "select coalesce(max(sort_order) + 1, 0) from projects",
                [],
                |row| row.get(0),
            )
            .context("failed to compute next project sort order")
    }

    pub fn load_projects(&self) -> Result<Vec<ProjectConfig>> {
        let mut stmt = self.conn.prepare(
            r#"
            select id, path, name, default_provider, leading_branch, auto_reopen_agents, startup_command, env
            from projects
            order by sort_order, name collate nocase, path collate nocase
            "#,
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ProjectConfig {
                id: row.get(0)?,
                path: row.get(1)?,
                name: row.get(2)?,
                default_provider: row.get(3)?,
                leading_branch: row.get(4)?,
                auto_reopen_agents: row.get(5)?,
                startup_command: row.get(6)?,
                env: deserialize_project_env(row.get::<_, String>(7)?.as_str()),
            })
        })?;

        let mut projects = Vec::new();
        for row in rows {
            projects.push(row?);
        }
        Ok(projects)
    }

    /// Map of project id -> `created_at` timestamp from the `projects` table.
    /// Kept separate from [`SessionStore::load_projects`] because `created_at` is
    /// persisted/runtime state, not portable `ProjectConfig`: surfacing it does
    /// not pollute the config representation that gets written back to disk.
    pub fn load_project_created_ats(
        &self,
    ) -> Result<std::collections::HashMap<String, DateTime<Utc>>> {
        let mut stmt = self.conn.prepare("select id, created_at from projects")?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let created_at: String = row.get(1)?;
            Ok((id, created_at))
        })?;

        let mut map = std::collections::HashMap::new();
        for row in rows {
            let (id, created_at) = row?;
            if let Some(parsed) = parse_time(&created_at) {
                map.insert(id, parsed);
            }
        }
        Ok(map)
    }

    pub fn update_project_default_provider(
        &self,
        project_id: &str,
        default_provider: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            r#"
            update projects
            set default_provider = ?2,
                updated_at = ?3
            where id = ?1
            "#,
            params![project_id, default_provider, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn update_project_auto_reopen(
        &self,
        project_id: &str,
        auto_reopen_agents: Option<bool>,
    ) -> Result<()> {
        self.conn.execute(
            r#"
            update projects
            set auto_reopen_agents = ?2,
                updated_at = ?3
            where id = ?1
            "#,
            params![project_id, auto_reopen_agents, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn update_project_startup_command(
        &self,
        project_id: &str,
        startup_command: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            r#"
            update projects
            set startup_command = ?2,
                updated_at = ?3
            where id = ?1
            "#,
            params![project_id, startup_command, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn update_project_env(
        &self,
        project_id: &str,
        env: &BTreeMap<String, String>,
    ) -> Result<()> {
        self.conn.execute(
            r#"
            update projects
            set env = ?2,
                updated_at = ?3
            where id = ?1
            "#,
            params![
                project_id,
                serialize_project_env(env),
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    pub fn delete_project(&self, id: &str) -> Result<()> {
        self.conn
            .execute("delete from projects where id = ?1", params![id])?;
        Ok(())
    }

    /// Remove a project and every record that belongs to it — each session's PR
    /// rows, the session rows, and the `projects` row — in a single transaction,
    /// returning the deleted session ids. Atomic: a failure leaves all rows
    /// intact, so a removal can never half-delete a project (e.g. agents gone but
    /// the project row surviving to reappear on restart). Deleting a project row
    /// that does not exist (a ghost id) is a harmless no-op within the same
    /// transaction.
    pub fn remove_project_records(&self, project_id: &str) -> Result<Vec<String>> {
        let tx = self.conn.unchecked_transaction()?;
        let ids: Vec<String> = {
            let mut stmt = tx.prepare("select id from agent_sessions where project_id = ?1")?;
            let rows = stmt.query_map(params![project_id], |row| row.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<String>>>()?
        };
        tx.execute(
            "delete from session_prs where session_id in \
             (select id from agent_sessions where project_id = ?1)",
            params![project_id],
        )?;
        // Drop the per-session changed-files rev counters BEFORE the sessions
        // themselves (the subquery resolves the ids while the rows still exist),
        // so a project removal cannot leave orphaned `changes_rev` rows behind.
        tx.execute(
            "delete from changes_rev where session_id in \
             (select id from agent_sessions where project_id = ?1)",
            params![project_id],
        )?;
        // Drop the sessions' extra tabs BEFORE the sessions themselves (the
        // subquery resolves the ids while the parent rows still exist), so a
        // project removal cannot leave orphaned `agent_tabs` rows behind.
        tx.execute(
            "delete from agent_tabs where session_id in \
             (select id from agent_sessions where project_id = ?1)",
            params![project_id],
        )?;
        tx.execute(
            "delete from agent_sessions where project_id = ?1",
            params![project_id],
        )?;
        tx.execute("delete from projects where id = ?1", params![project_id])?;
        tx.commit()?;
        Ok(ids)
    }

    /// Insert a PR association or update its state and title if it already exists.
    pub fn upsert_pr(&self, pr: &StoredPr) -> Result<()> {
        self.conn.execute(
            r#"
            insert into session_prs (session_id, pr_number, host, owner_repo, state, title, url)
            values (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            on conflict(session_id, pr_number) do update set
                host=excluded.host,
                owner_repo=excluded.owner_repo,
                state=excluded.state,
                title=excluded.title,
                url=excluded.url
            "#,
            params![
                pr.session_id,
                pr.pr_number as i64,
                pr.host,
                pr.owner_repo,
                pr.state,
                pr.title,
                pr.url
            ],
        )?;
        Ok(())
    }

    /// Load all known PRs for a session, ordered by pr_number descending (latest first).
    pub fn load_prs(&self, session_id: &str) -> Result<Vec<StoredPr>> {
        let mut stmt = self.conn.prepare(
            r#"
            select pr_number, host, owner_repo, state, title, url
            from session_prs
            where session_id = ?1
            order by pr_number desc
            "#,
        )?;
        let sid = session_id.to_string();
        let rows = stmt.query_map(params![session_id], |row| {
            let pr_number = row.get::<_, i64>(0)? as u64;
            let host: String = row.get(1)?;
            let owner_repo: String = row.get(2)?;
            Ok(StoredPr {
                session_id: sid.clone(),
                pr_number,
                host: host.clone(),
                owner_repo: owner_repo.clone(),
                state: row.get(3)?,
                title: row.get(4)?,
                url: normalize_pr_url(row.get(5)?, &host, &owner_repo, pr_number),
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
        let mut stmt = self.conn.prepare(
            r#"
            select session_id, pr_number, host, owner_repo, state, title, url
            from session_prs
            where (session_id, pr_number) in (
                select session_id, max(pr_number) from session_prs group by session_id
            )
            "#,
        )?;
        let rows = stmt.query_map([], |row| {
            let pr_number = row.get::<_, i64>(1)? as u64;
            let host: String = row.get(2)?;
            let owner_repo: String = row.get(3)?;
            Ok(StoredPr {
                session_id: row.get(0)?,
                pr_number,
                host: host.clone(),
                owner_repo: owner_repo.clone(),
                state: row.get(4)?,
                title: row.get(5)?,
                url: normalize_pr_url(row.get(6)?, &host, &owner_repo, pr_number),
            })
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    pub fn upsert_session(&self, session: &AgentSession) -> Result<()> {
        // UPDATE first: existing sessions are re-upserted constantly (status
        // changes, provider starts), and that hot path must not pay the
        // min(sort_order) placement query below. The SET list deliberately
        // omits `sort_order` so re-upserting an existing session never
        // disturbs the user's chosen order.
        let updated = self.conn.execute(
            r#"
            update agent_sessions set
                project_path=?2,
                provider=?3,
                source_branch=?4,
                branch_name=?5,
                worktree_path=?6,
                title=?7,
                started_providers=?8,
                desired_running=?9,
                auto_reopen_enabled=?10,
                status=?11,
                updated_at=?12,
                initial_branch=?13
            where id = ?1
            "#,
            params![
                session.id,
                session.project_path,
                session.provider.as_str(),
                session.source_branch,
                session.branch_name,
                session.worktree_path,
                session.title,
                serialize_started_providers(&session.started_providers),
                session.desired_running,
                session.auto_reopen_enabled,
                session.status.as_str(),
                session.updated_at.to_rfc3339(),
                session.initial_branch,
            ],
        )?;
        if updated > 0 {
            return Ok(());
        }
        // A brand-new session lands at the TOP of its project's order: one
        // position above the current minimum (negative values are fine —
        // positions are relative, only their ordering matters). The engine is
        // single-threaded over this connection, so the UPDATE-miss → INSERT
        // sequence cannot race.
        let new_sort_order = self
            .min_session_sort_order(&session.project_id)?
            .unwrap_or(1)
            - 1;
        self.conn.execute(
            r#"
            insert into agent_sessions
                (id, project_id, project_path, provider, source_branch, branch_name, worktree_path, title, started_providers, desired_running, auto_reopen_enabled, status, sort_order, created_at, updated_at, initial_branch)
            values
                (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
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
                session.desired_running,
                session.auto_reopen_enabled,
                session.status.as_str(),
                new_sort_order,
                session.created_at.to_rfc3339(),
                session.updated_at.to_rfc3339(),
                session.initial_branch,
            ],
        )?;
        Ok(())
    }

    /// The smallest `sort_order` currently assigned to any session in
    /// `project_id`, or `None` when the project has no sessions yet. Used to
    /// place a new session one position above the current top.
    pub fn min_session_sort_order(&self, project_id: &str) -> Result<Option<i64>> {
        self.conn
            .query_row(
                "select min(sort_order) from agent_sessions where project_id = ?1",
                params![project_id],
                |row| row.get::<_, Option<i64>>(0),
            )
            .context("failed to compute min session sort order")
    }

    /// Assign positions `0..n` to exactly `ordered_ids`, in that order, scoped
    /// to `project_id`. Runs in a single transaction. The storage layer is
    /// intentionally "dumb": it does not validate that `ordered_ids` is the
    /// complete set of the project's sessions — that strict validation lives in
    /// `Engine::apply`. `updated_at` is deliberately NOT touched, because doing
    /// so would corrupt the "sort by most recently updated" semantics.
    pub fn reorder_sessions(&self, project_id: &str, ordered_ids: &[String]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare(
                "update agent_sessions set sort_order = ?1 where id = ?2 and project_id = ?3",
            )?;
            for (position, id) in ordered_ids.iter().enumerate() {
                stmt.execute(params![position as i64, id, project_id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Assign a GLOBAL `sort_order` of `0..n` to `ordered_ids`, in that order,
    /// across every session regardless of project. This is the flat-model ordering:
    /// agents are one independent list, so a drag persists a single global
    /// permutation (not a per-project one). Not project-scoped, so it can move an
    /// agent anywhere in the list. Like [`reorder_sessions`] it is "dumb": strict
    /// validation that `ordered_ids` is the complete session set lives in the
    /// engine. `updated_at` is deliberately untouched (preserves recency sorting).
    pub fn set_global_session_order(&self, ordered_ids: &[String]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare("update agent_sessions set sort_order = ?1 where id = ?2")?;
            for (position, id) in ordered_ids.iter().enumerate() {
                stmt.execute(params![position as i64, id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Assign positions `0..n` to exactly `ordered_ids`, in that order, over the
    /// `projects.sort_order` column. Single transaction. Like
    /// [`reorder_sessions`], validation that `ordered_ids` is the complete set
    /// of known projects lives in `Engine::apply`, not here.
    pub fn reorder_projects(&self, ordered_ids: &[String]) -> Result<()> {
        let tx = self.conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare("update projects set sort_order = ?1 where id = ?2")?;
            for (position, id) in ordered_ids.iter().enumerate() {
                stmt.execute(params![position as i64, id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// True when some project has more than one session and ALL of that
    /// project's sessions are still at `sort_order = 0` — the fingerprint of a
    /// `sort_order` backfill that never ran (stranded by a crash between the
    /// autocommitted ALTER and the backfill). Used to make the one-time
    /// backfill retryable.
    ///
    /// This is deliberately narrower than "every row is 0": a project with a
    /// single session legitimately sits at `sort_order = 0` (position 0), so
    /// that state must NOT trigger a re-run on every open. Two-or-more sessions
    /// in one project all pinned at 0 is impossible in steady state — inserts
    /// land at `min-1` (negative) and reorders assign distinct `0..n` — so it
    /// only ever indicates a stranded half-migration. `count(nullif(sort_order,
    /// 0))` counts only rows whose value is neither 0 nor NULL.
    fn session_sort_order_needs_backfill(&self) -> Result<bool> {
        // The flat model orders agents by a GLOBAL `sort_order`, so it must be a
        // total order (globally distinct). Re-run the backfill whenever two or more
        // sessions share a value: that means either a fresh (all-zero) table or a
        // legacy PER-PROJECT numbering (each project restarted at 0) that must be
        // globalized. Once globally distinct, this returns false and never re-runs.
        let has_duplicates: bool = self.conn.query_row(
            "select exists( \
                 select 1 from agent_sessions \
                 group by sort_order \
                 having count(*) > 1 \
             )",
            [],
            |row| row.get(0),
        )?;
        Ok(has_duplicates)
    }

    /// One-time backfill run when the `sort_order` column is first added to an
    /// existing `agent_sessions` table. Numbers ALL sessions `0,1,2,…` in one
    /// global sequence (agents are a single flat list) following the legacy
    /// `updated_at DESC` order, so the visible order is preserved exactly after
    /// the upgrade.
    fn backfill_session_sort_order(&self) -> Result<()> {
        // Assign a GLOBAL 0..n order (flat model: agents are one independent list).
        // Order by the CURRENT effective order (`sort_order asc, updated_at desc` —
        // the same order `load_sessions` uses) so this is non-destructive: a legacy
        // per-project arrangement is frozen into a sensible global sequence rather
        // than reshuffled, and a fresh (all-zero) table falls back to most-recent
        // first. Runs once; afterwards the values are globally distinct.
        let ids: Vec<String> = self
            .conn
            .prepare("select id from agent_sessions order by sort_order asc, updated_at desc")?
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let tx = self.conn.unchecked_transaction()?;
        {
            let mut update =
                tx.prepare("update agent_sessions set sort_order = ?1 where id = ?2")?;
            for (position, id) in ids.iter().enumerate() {
                update.execute(params![position as i64, id])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn load_sessions(&self) -> Result<Vec<AgentSession>> {
        let mut stmt = self.conn.prepare(
            r#"
            select id, project_id, provider, source_branch, branch_name, worktree_path, title, project_path, started_providers, desired_running, auto_reopen_enabled, status, created_at, updated_at, initial_branch, last_focused_tab
            from agent_sessions
            order by sort_order asc, updated_at desc
            "#,
        )?;
        let rows = stmt.query_map([], |row| {
            let started_providers: String = row.get(8)?;
            let created_at: String = row.get(12)?;
            let updated_at: String = row.get(13)?;
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
                desired_running: row.get(9)?,
                auto_reopen_enabled: row.get(10)?,
                status: SessionStatus::from_str(row.get::<_, String>(11)?.as_str()),
                created_at: parse_time(&created_at).unwrap_or_else(Utc::now),
                updated_at: parse_time(&updated_at).unwrap_or_else(Utc::now),
                initial_branch: row.get(14)?,
                last_focused_tab: row.get(15)?,
            })
        })?;

        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row?);
        }
        Ok(sessions)
    }

    pub fn delete_session(&self, id: &str) -> Result<()> {
        // Delete the session and all of its dependent rows atomically. These
        // tables declare ON DELETE CASCADE FKs to `agent_sessions`, but the
        // connection never enables `PRAGMA foreign_keys`, so those cascades do
        // not fire — delete the rows explicitly. Wrapped in a transaction so a
        // mid-sequence failure leaves either all of the session's rows or none,
        // never a half-deleted session (e.g. tabs gone but the session surviving).
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("delete from session_prs where session_id = ?1", params![id])?;
        // Drop the per-session changed-files revision counter too, so a deleted
        // session leaves no housekeeping rows behind.
        tx.execute("delete from changes_rev where session_id = ?1", params![id])?;
        // Drop the session's extra tabs (the session-slot tab has no row).
        tx.execute("delete from agent_tabs where session_id = ?1", params![id])?;
        tx.execute("delete from agent_sessions where id = ?1", params![id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn set_desired_running(&self, id: &str, desired_running: bool) -> Result<()> {
        self.conn.execute(
            "update agent_sessions set desired_running = ?2, updated_at = ?3 where id = ?1",
            params![id, desired_running, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    /// Persist the remembered last-focused tab for a session. `None` clears the
    /// memory (resolves to the session-slot tab). Deliberately its own tiny
    /// setter, mirroring [`Self::set_auto_reopen_enabled`], rather than folded
    /// into `upsert_session` — see the field doc comment on
    /// [`crate::model::AgentSession::last_focused_tab`] for why. `updated_at` is
    /// intentionally NOT touched: a focus change is not a content change, and
    /// touching it would perturb "sort by most recently updated" ordering.
    pub fn set_last_focused_tab(&self, id: &str, tab_id: Option<&str>) -> Result<()> {
        self.conn.execute(
            "update agent_sessions set last_focused_tab = ?2 where id = ?1",
            params![id, tab_id],
        )?;
        Ok(())
    }

    pub fn set_auto_reopen_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        self.conn.execute(
            "update agent_sessions set auto_reopen_enabled = ?2, updated_at = ?3 where id = ?1",
            params![id, enabled, Utc::now().to_rfc3339()],
        )?;
        Ok(())
    }

    /// Test-only fault injection: drops the `agent_sessions` table so the
    /// next session-write call (upsert/delete/set_*) returns an error.
    /// Used to verify DB-first failure semantics in the engine.
    #[cfg(test)]
    pub(crate) fn break_sessions_table_for_test(&self) -> Result<()> {
        self.conn
            .execute_batch("drop table if exists agent_sessions;")?;
        Ok(())
    }
}

fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

fn serialize_project_env(env: &BTreeMap<String, String>) -> String {
    serde_json::to_string(env).unwrap_or_else(|_| "{}".to_string())
}

fn deserialize_project_env(value: &str) -> BTreeMap<String, String> {
    serde_json::from_str::<BTreeMap<String, String>>(value).unwrap_or_default()
}

fn serialize_started_providers(started_providers: &[String]) -> String {
    serde_json::to_string(started_providers).unwrap_or_else(|_| "[]".to_string())
}

fn parse_started_providers(value: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(value).unwrap_or_default()
}

pub fn fallback_pr_url(host: &str, owner_repo: &str, pr_number: u64) -> String {
    let host = if host.trim().is_empty() {
        "github.com"
    } else {
        host
    };
    format!("https://{host}/{owner_repo}/pull/{pr_number}")
}

fn normalize_pr_url(url: String, host: &str, owner_repo: &str, pr_number: u64) -> String {
    if url.trim().is_empty() {
        fallback_pr_url(host, owner_repo, pr_number)
    } else {
        url
    }
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
        initial_branch: format!("branch-{id}"),
        worktree_path: format!("/tmp/{id}"),
        title: None,
        started_providers: Vec::new(),
        desired_running: false,
        auto_reopen_enabled: true,
        status: SessionStatus::Active,
        created_at,
        updated_at,
        last_focused_tab: None,
    }
}

/// Like [`test_session`] but lets the caller pick the project id, for tests
/// that exercise per-project ordering across multiple projects.
#[cfg(test)]
fn test_session_in(
    id: &str,
    project_id: &str,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
) -> crate::model::AgentSession {
    crate::model::AgentSession {
        project_id: project_id.to_string(),
        ..test_session(id, created_at, updated_at)
    }
}

/// Builds an extra-tab row owned by `session_id`.
#[cfg(test)]
fn test_tab(id: &str, session_id: &str, sort_order: i64) -> crate::model::AgentTab {
    crate::model::AgentTab {
        id: id.to_string(),
        session_id: session_id.to_string(),
        provider: crate::model::ProviderKind::new("codex"),
        sort_order,
        created_at: Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    /// The database mirrors the same per-project `env` map that made
    /// `config.toml` `0600`, and SQLite's `-wal`/`-shm` sidecars carry the same
    /// content.
    ///
    /// Be precise about what this proves, because the name it used to carry
    /// promised more than it delivered. On a FIRST open the sidecars do not yet
    /// exist when the tightening loop runs, so the loop cannot be what makes
    /// them owner-only: SQLite creates them afterwards and they INHERIT the
    /// database file's mode. Removing `-wal` and `-shm` from that loop left the
    /// old test green, which is why it is named for inheritance now. The loop
    /// entries are covered by the reopen test below, which is where they are
    /// actually load-bearing.
    ///
    /// The `if let Ok(meta)` this used to wrap each check in is gone: a sidecar
    /// that was not there passed silently, so the test could prove nothing at
    /// all and still succeed.
    #[test]
    fn sidecars_created_after_the_open_inherit_the_databases_owner_only_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("sessions.sqlite3");
        let store = SessionStore::open(&db).unwrap();
        // Force a WAL write so the sidecars definitely exist.
        store
            .conn
            .execute_batch("create table if not exists probe (x);")
            .unwrap();

        for path in [
            db.clone(),
            sidecar_path(&db, "-wal"),
            sidecar_path(&db, "-shm"),
        ] {
            let meta = std::fs::metadata(&path)
                .unwrap_or_else(|e| panic!("{} must exist to be checked: {e}", path.display()));
            let mode = meta.permissions().mode() & 0o777;
            assert_eq!(
                mode & 0o077,
                0,
                "{} should not be group/world readable, got {mode:o}",
                path.display()
            );
        }
    }

    /// This is what the `-wal`/`-shm` entries in the tightening loop are FOR: a
    /// sidecar that already exists at a loose mode when dux opens the database,
    /// as an older installation would have left it. The sidecars only exist
    /// while a connection is open (SQLite removes them when the last one
    /// closes), so the first store is held open across the second open.
    #[test]
    fn open_tightens_sidecars_left_world_readable_by_an_older_install() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("sessions.sqlite3");
        let holder = SessionStore::open(&db).unwrap();
        holder
            .conn
            .execute_batch("create table if not exists probe (x);")
            .unwrap();

        let wal = sidecar_path(&db, "-wal");
        let shm = sidecar_path(&db, "-shm");
        for path in [&wal, &shm] {
            assert!(path.exists(), "{} must exist for this test", path.display());
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644)).unwrap();
        }

        let _second = SessionStore::open(&db).unwrap();

        for path in [&wal, &shm] {
            let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
            assert_eq!(
                mode,
                0o600,
                "{} should have been tightened on open, got {mode:o}",
                path.display()
            );
        }
    }

    #[test]
    fn open_tightens_a_database_left_world_readable_by_an_older_install() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("sessions.sqlite3");
        drop(SessionStore::open(&db).unwrap());
        std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o644)).unwrap();

        let _store = SessionStore::open(&db).unwrap();
        let mode = std::fs::metadata(&db).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {mode:o}");
    }

    #[test]
    fn agent_tabs_table_is_idempotent_and_empty_on_fresh_db() {
        let store = test_store();
        // migrate() ran in open(); a second migrate is a no-op.
        store.migrate().unwrap();
        assert!(store.load_agent_tabs().unwrap().is_empty());
    }

    #[test]
    fn agent_tab_crud_round_trips() {
        let store = test_store();
        let now = Utc::now();
        store.upsert_session(&test_session("s1", now, now)).unwrap();
        store.insert_agent_tab(&test_tab("t1", "s1", 1)).unwrap();
        store.insert_agent_tab(&test_tab("t2", "s1", 2)).unwrap();

        let loaded = store.load_agent_tabs().unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].id, "t1");
        assert_eq!(loaded[0].provider.as_str(), "codex");
        assert_eq!(store.count_agent_tabs("s1").unwrap(), 2);
        assert_eq!(store.max_tab_sort_order("s1").unwrap(), Some(2));
        assert_eq!(store.max_tab_sort_order("nope").unwrap(), None);

        store.update_agent_tab_provider("t1", "claude").unwrap();
        assert_eq!(
            store.load_agent_tabs().unwrap()[0].provider.as_str(),
            "claude"
        );

        store.delete_agent_tab("t1").unwrap();
        assert_eq!(store.count_agent_tabs("s1").unwrap(), 1);
    }

    #[test]
    fn delete_session_removes_its_agent_tabs_rows() {
        let store = test_store();
        let now = Utc::now();
        store.upsert_session(&test_session("s1", now, now)).unwrap();
        store.upsert_session(&test_session("s2", now, now)).unwrap();
        store.insert_agent_tab(&test_tab("t1", "s1", 1)).unwrap();
        store.insert_agent_tab(&test_tab("t2", "s2", 1)).unwrap();

        store.delete_session("s1").unwrap();
        assert_eq!(store.count_agent_tabs("s1").unwrap(), 0);
        // A sibling session's tabs are untouched.
        assert_eq!(store.count_agent_tabs("s2").unwrap(), 1);
    }

    #[test]
    fn remove_project_records_removes_all_its_sessions_tabs() {
        let store = test_store();
        let now = Utc::now();
        store
            .upsert_session(&test_session_in("s1", "projA", now, now))
            .unwrap();
        store
            .upsert_session(&test_session_in("s2", "projB", now, now))
            .unwrap();
        store.insert_agent_tab(&test_tab("t1", "s1", 1)).unwrap();
        store.insert_agent_tab(&test_tab("t2", "s2", 1)).unwrap();

        store.remove_project_records("projA").unwrap();
        assert_eq!(store.count_agent_tabs("s1").unwrap(), 0);
        assert_eq!(store.count_agent_tabs("s2").unwrap(), 1);
    }

    #[test]
    fn load_agent_tabs_sweeps_orphans_with_no_session() {
        let store = test_store();
        let now = Utc::now();
        store.upsert_session(&test_session("s1", now, now)).unwrap();
        store.insert_agent_tab(&test_tab("t1", "s1", 1)).unwrap();
        // A row whose session was removed by an older binary that didn't cascade.
        store
            .insert_agent_tab(&test_tab("orphan", "gone", 1))
            .unwrap();

        let loaded = store.load_agent_tabs().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "t1");
    }

    #[test]
    fn last_seen_version_round_trips_through_a_real_database_file() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("sessions.sqlite3");

        // A brand new database has never seen a version: that is what makes the
        // very first launch show the welcome screen.
        {
            let store = SessionStore::open(&db).unwrap();
            assert_eq!(store.last_seen_version().unwrap(), None);
            store.set_last_seen_version("v0.6.0").unwrap();
            assert_eq!(
                store.last_seen_version().unwrap(),
                Some("v0.6.0".to_string())
            );
            // Setting it again replaces rather than duplicating (upsert on the key).
            store.set_last_seen_version("v0.7.0").unwrap();
            assert_eq!(
                store.last_seen_version().unwrap(),
                Some("v0.7.0".to_string())
            );
        }

        // Reopening the SAME file keeps the value: dismissing on one surface is
        // remembered by the other, and across restarts.
        {
            let store = SessionStore::open(&db).unwrap();
            assert_eq!(
                store.last_seen_version().unwrap(),
                Some("v0.7.0".to_string())
            );
        }
    }

    #[test]
    fn app_state_is_a_generic_key_value_table() {
        let store = test_store();
        assert_eq!(store.app_state("nope").unwrap(), None);
        store.set_app_state("k", "v").unwrap();
        store.set_app_state("other", "w").unwrap();
        assert_eq!(store.app_state("k").unwrap(), Some("v".to_string()));
        assert_eq!(store.app_state("other").unwrap(), Some("w".to_string()));
        // Keys are independent and values are replaced in place.
        store.set_app_state("k", "v2").unwrap();
        assert_eq!(store.app_state("k").unwrap(), Some("v2".to_string()));
        assert_eq!(store.app_state("other").unwrap(), Some("w".to_string()));
    }

    #[test]
    fn migrate_is_idempotent_for_app_state() {
        // `migrate()` runs on every open; a second open must not fail or wipe
        // the row (there is no migration-versioning table in this project).
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("sessions.sqlite3");
        SessionStore::open(&db)
            .unwrap()
            .set_app_state("k", "v")
            .unwrap();
        let store = SessionStore::open(&db).unwrap();
        assert_eq!(store.app_state("k").unwrap(), Some("v".to_string()));
    }

    fn stored_pr(session_id: &str, pr_number: u64) -> StoredPr {
        StoredPr {
            session_id: session_id.to_string(),
            pr_number,
            host: "github.com".to_string(),
            owner_repo: "o/r".to_string(),
            state: "OPEN".to_string(),
            title: "t".to_string(),
            url: "u".to_string(),
        }
    }

    #[test]
    fn next_changes_rev_increments_and_persists_across_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("sessions.sqlite3");

        // First run: the counter starts at 1 and strictly increases per session,
        // independently per session id.
        {
            let store = SessionStore::open(&db).unwrap();
            assert_eq!(store.next_changes_rev("s1").unwrap(), 1);
            assert_eq!(store.next_changes_rev("s1").unwrap(), 2);
            assert_eq!(store.next_changes_rev("s1").unwrap(), 3);
            // A different session has its own independent counter.
            assert_eq!(store.next_changes_rev("s2").unwrap(), 1);
        }

        // Reopen the SAME database file: the counter continues from its last
        // value rather than resetting (persisted, monotonic across restarts).
        {
            let store = SessionStore::open(&db).unwrap();
            assert_eq!(store.next_changes_rev("s1").unwrap(), 4);
            assert_eq!(store.next_changes_rev("s2").unwrap(), 2);
        }
    }

    #[test]
    fn delete_session_removes_its_changes_rev_row() {
        let store = test_store();
        let now = Utc::now();
        store.upsert_session(&test_session("s1", now, now)).unwrap();
        assert_eq!(store.next_changes_rev("s1").unwrap(), 1);
        assert_eq!(store.next_changes_rev("s1").unwrap(), 2);

        store.delete_session("s1").unwrap();

        // The counter row was dropped, so a fresh session reusing the id starts
        // back at 1 rather than continuing the deleted session's sequence.
        assert_eq!(store.next_changes_rev("s1").unwrap(), 1);
    }

    #[test]
    fn delete_session_also_removes_its_pr_rows() {
        let store = test_store();
        let now = Utc::now();
        store.upsert_session(&test_session("s1", now, now)).unwrap();
        store.upsert_pr(&stored_pr("s1", 7)).unwrap();
        assert_eq!(store.load_all_latest_prs().unwrap().len(), 1);

        store.delete_session("s1").unwrap();

        assert!(store.load_sessions().unwrap().is_empty());
        // The ON DELETE CASCADE FK is unenforced (PRAGMA foreign_keys is off), so
        // the explicit session_prs delete is what keeps the PR row from leaking.
        assert!(store.load_all_latest_prs().unwrap().is_empty());
    }

    #[test]
    fn remove_project_records_clears_project_sessions_and_prs_atomically() {
        let store = test_store();
        let now = Utc::now();
        let p1 = ProjectConfig {
            id: "p1".to_string(),
            path: "/tmp/p1".to_string(),
            name: Some("p1".to_string()),
            default_provider: None,
            leading_branch: None,
            auto_reopen_agents: None,
            startup_command: None,
            env: BTreeMap::new(),
        };
        let p2 = ProjectConfig {
            id: "p2".to_string(),
            path: "/tmp/p2".to_string(),
            name: Some("p2".to_string()),
            default_provider: None,
            leading_branch: None,
            auto_reopen_agents: None,
            startup_command: None,
            env: BTreeMap::new(),
        };
        store.upsert_project(&p1).unwrap();
        store.upsert_project(&p2).unwrap();
        store
            .upsert_session(&test_session_in("a", "p1", now, now))
            .unwrap();
        store
            .upsert_session(&test_session_in("b", "p1", now, now))
            .unwrap();
        store
            .upsert_session(&test_session_in("c", "p2", now, now))
            .unwrap();
        store.upsert_pr(&stored_pr("a", 1)).unwrap();
        // Advance a changed-files rev for one of p1's sessions so there is a
        // `changes_rev` row to prove the bulk removal drops it too.
        assert_eq!(store.next_changes_rev("a").unwrap(), 1);
        assert_eq!(store.next_changes_rev("a").unwrap(), 2);

        let removed = store.remove_project_records("p1").unwrap();

        assert_eq!(removed.len(), 2);
        assert!(removed.contains(&"a".to_string()));
        assert!(removed.contains(&"b".to_string()));
        // Only p2's session survives; p1's sessions AND their PR rows are gone.
        let remaining: Vec<String> = store
            .load_sessions()
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(remaining, vec!["c".to_string()]);
        assert!(store.load_all_latest_prs().unwrap().is_empty());
        // The project row itself is deleted in the same transaction — only p2
        // remains, so a removal cannot leave a row that reappears on restart.
        let project_ids: Vec<String> = store
            .load_projects()
            .unwrap()
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(project_ids, vec!["p2".to_string()]);
        // The deleted session's changes_rev row is gone: a fresh session reusing
        // the id starts back at 1 rather than continuing the deleted sequence.
        assert_eq!(store.next_changes_rev("a").unwrap(), 1);
    }

    #[test]
    fn new_sessions_land_at_top_of_their_project() {
        let store = test_store();
        let now = Utc::now();

        // Insert three sessions into the same project. Each new insert takes the
        // top slot (sort_order = current min - 1), so the load order is the
        // reverse of the insertion order regardless of updated_at.
        let s1 = test_session("a", now - Duration::hours(3), now - Duration::hours(3));
        let s2 = test_session("b", now - Duration::hours(2), now - Duration::hours(1));
        let s3 = test_session("c", now - Duration::hours(1), now - Duration::hours(2));

        store.upsert_session(&s1).unwrap();
        store.upsert_session(&s2).unwrap();
        store.upsert_session(&s3).unwrap();

        let loaded = store.load_sessions().unwrap();
        let ids: Vec<&str> = loaded.iter().map(|s| s.id.as_str()).collect();

        // Most recently inserted (c) is at the top, then b, then a.
        assert_eq!(ids, vec!["c", "b", "a"]);
    }

    #[test]
    fn upsert_existing_session_preserves_sort_order() {
        let store = test_store();
        let now = Utc::now();

        let s1 = test_session("a", now - Duration::hours(2), now - Duration::hours(2));
        let s2 = test_session("b", now - Duration::hours(1), now - Duration::hours(1));

        store.upsert_session(&s1).unwrap();
        store.upsert_session(&s2).unwrap();

        // After two inserts the order is b (top), a. Re-upserting an existing
        // session must NOT touch its sort_order (the on-conflict set omits it).
        store.upsert_session(&s1).unwrap();

        let loaded = store.load_sessions().unwrap();
        let ids: Vec<&str> = loaded.iter().map(|s| s.id.as_str()).collect();

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
    #[test]
    fn projects_round_trip_all_project_fields() {
        let store = test_store();
        let project = ProjectConfig {
            id: "project-1".to_string(),
            path: "$CODE/dux".to_string(),
            name: Some("dux".to_string()),
            default_provider: Some("codex".to_string()),
            leading_branch: Some("main".to_string()),
            auto_reopen_agents: Some(false),
            startup_command: Some("npm install".to_string()),
            env: BTreeMap::from([("EDITOR".to_string(), "true".to_string())]),
        };

        store.upsert_project(&project).unwrap();

        let loaded = store.load_projects().unwrap();
        assert_eq!(loaded, vec![project]);
    }

    #[test]
    fn project_path_conflict_keeps_existing_id() {
        let store = test_store();
        store
            .upsert_project(&ProjectConfig {
                id: "stable-id".to_string(),
                path: "/repo".to_string(),
                name: Some("old".to_string()),
                default_provider: None,
                leading_branch: Some("main".to_string()),
                auto_reopen_agents: None,
                startup_command: None,
                env: Default::default(),
            })
            .unwrap();

        store
            .upsert_project(&ProjectConfig {
                id: "new-id".to_string(),
                path: "/repo".to_string(),
                name: Some("new".to_string()),
                default_provider: Some("claude".to_string()),
                leading_branch: Some("trunk".to_string()),
                auto_reopen_agents: Some(false),
                startup_command: Some("echo setup".to_string()),
                env: BTreeMap::from([("API_KEY".to_string(), "${FOO_API_KEY}".to_string())]),
            })
            .unwrap();

        let loaded = store.load_projects().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "stable-id");
        assert_eq!(loaded[0].name.as_deref(), Some("new"));
        assert_eq!(loaded[0].default_provider.as_deref(), Some("claude"));
        assert_eq!(loaded[0].leading_branch.as_deref(), Some("trunk"));
        assert_eq!(loaded[0].auto_reopen_agents, Some(false));
        assert_eq!(loaded[0].startup_command.as_deref(), Some("echo setup"));
        assert_eq!(
            loaded[0].env.get("API_KEY").map(String::as_str),
            Some("${FOO_API_KEY}")
        );
    }

    #[test]
    fn auto_reopen_fields_round_trip() {
        let store = test_store();
        let now = Utc::now();
        let mut session = test_session("auto", now, now);
        session.desired_running = true;
        session.auto_reopen_enabled = false;

        store.upsert_session(&session).unwrap();

        let loaded = store.load_sessions().unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].desired_running);
        assert!(!loaded[0].auto_reopen_enabled);

        store.set_auto_reopen_enabled("auto", true).unwrap();
        store.set_desired_running("auto", false).unwrap();
        let loaded = store.load_sessions().unwrap();
        assert!(!loaded[0].desired_running);
        assert!(loaded[0].auto_reopen_enabled);
    }

    #[test]
    fn last_focused_tab_is_null_on_a_fresh_row_and_round_trips_through_the_setter() {
        let store = test_store();
        let now = Utc::now();
        store.upsert_session(&test_session("s1", now, now)).unwrap();

        // Fresh row: NULL, i.e. "no memory recorded".
        let loaded = store.load_sessions().unwrap();
        assert_eq!(loaded[0].last_focused_tab, None);

        store.set_last_focused_tab("s1", Some("t1")).unwrap();
        let loaded = store.load_sessions().unwrap();
        assert_eq!(loaded[0].last_focused_tab.as_deref(), Some("t1"));

        // Setting None clears it back to NULL.
        store.set_last_focused_tab("s1", None).unwrap();
        let loaded = store.load_sessions().unwrap();
        assert_eq!(loaded[0].last_focused_tab, None);
    }

    #[test]
    fn last_focused_tab_survives_upsert_session_status_churn() {
        // Locks in the "omit from SET/INSERT lists" decision: re-upserting an
        // existing session (simulating a status change or config-reload churn)
        // must never clobber a previously remembered focused tab, exactly like
        // `sort_order`.
        let store = test_store();
        let now = Utc::now();
        let session = test_session("s1", now, now);
        store.upsert_session(&session).unwrap();
        store.set_last_focused_tab("s1", Some("t1")).unwrap();

        // Re-upsert with an unrelated field changed, like a status transition.
        let mut churned = session;
        churned.status = SessionStatus::Detached;
        churned.updated_at = Utc::now();
        store.upsert_session(&churned).unwrap();

        let loaded = store.load_sessions().unwrap();
        assert_eq!(loaded[0].last_focused_tab.as_deref(), Some("t1"));
    }

    #[test]
    fn last_focused_tab_column_migration_is_idempotent() {
        let store = test_store();
        // migrate() ran in open(); a second migrate is a no-op and the column
        // stays present and nullable.
        store.migrate().unwrap();
        store
            .upsert_session(&test_session("s1", Utc::now(), Utc::now()))
            .unwrap();
        let loaded = store.load_sessions().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].last_focused_tab, None);
    }

    #[test]
    fn auto_reopen_fields_migrate_from_old_database() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            create table agent_sessions (
                id text primary key,
                project_id text not null,
                provider text not null,
                source_branch text not null,
                branch_name text not null,
                worktree_path text not null,
                title text,
                project_path text,
                status text not null,
                created_at text not null,
                updated_at text not null
            );
            insert into agent_sessions (
                id, project_id, provider, source_branch, branch_name,
                worktree_path, title, project_path, status, created_at, updated_at
            ) values (
                'old', 'proj', 'claude', 'main', 'agent', '/tmp/agent',
                null, null, 'detached', '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z'
            );
            "#,
        )
        .unwrap();

        let store = SessionStore { conn };
        store.migrate().unwrap();

        let loaded = store.load_sessions().unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(!loaded[0].desired_running);
        assert!(loaded[0].auto_reopen_enabled);
    }

    #[test]
    fn initial_branch_round_trips_through_storage() {
        let store = test_store();
        let now = Utc::now();
        let mut s = test_session("id1", now, now);
        s.branch_name = "renamed".into();
        s.initial_branch = "born-on".into();
        store.upsert_session(&s).unwrap();

        let loaded = store.load_sessions().unwrap();
        let got = loaded.iter().find(|s| s.id == "id1").expect("stored id1");
        assert_eq!(got.initial_branch, "born-on");
        assert_eq!(got.branch_name, "renamed");
    }

    #[test]
    fn migration_backfills_null_titles_from_branch_name() {
        // A legacy row inserted the old way (title NULL) gets its title frozen to
        // the current branch on open, so the displayed name can never drift.
        let store = legacy_store_with_sessions(&[("feat-x", "p1", "2026-01-01T00:00:00Z")]);
        let loaded = store.load_sessions().unwrap();
        let s = loaded.iter().find(|s| s.id == "feat-x").expect("row");
        // legacy_store_with_sessions sets branch_name == id.
        assert_eq!(s.title.as_deref(), Some("feat-x"));
    }

    #[test]
    fn migration_backfills_initial_branch_from_branch_name() {
        // A legacy row predating the initial_branch column has it backfilled to
        // the current branch (the best available birth branch).
        let store = legacy_store_with_sessions(&[("feat-x", "p1", "2026-01-01T00:00:00Z")]);
        let loaded = store.load_sessions().unwrap();
        let s = loaded.iter().find(|s| s.id == "feat-x").expect("row");
        assert_eq!(s.initial_branch, "feat-x");
    }

    #[test]
    fn second_migrate_does_not_freeze_a_null_title_inserted_after_upgrade() {
        // Regression: the title/initial_branch backfills must run EXACTLY ONCE
        // (when the initial_branch column is first added), not on every open().
        // A store built by legacy_store_with_sessions has already migrated once,
        // so the initial_branch column now exists. Insert a fresh auto-named
        // agent (title NULL — intentionally, so its display tracks the branch),
        // then migrate() again (simulating a later startup / config reload). The
        // second migration must NOT re-run the backfill and freeze the NULL title.
        let store = legacy_store_with_sessions(&[("feat-x", "p1", "2026-01-01T00:00:00Z")]);
        let mut fresh = test_session("auto-named", Utc::now(), Utc::now());
        fresh.project_id = "p1".into();
        fresh.branch_name = "pet-name".into();
        fresh.title = None;
        store.upsert_session(&fresh).unwrap();

        // A second open()/migrate() must be a no-op for the backfills.
        store.migrate().unwrap();

        let loaded = store.load_sessions().unwrap();
        let s = loaded.iter().find(|s| s.id == "auto-named").expect("row");
        assert_eq!(
            s.title, None,
            "a NULL title inserted after the one-time upgrade must not be frozen"
        );
    }

    #[test]
    fn migrate_self_heals_a_stranded_empty_initial_branch() {
        // A row left with initial_branch='' (e.g. stranded by a crash between the
        // ALTER and the backfill, or a downgrade→re-upgrade window) must be
        // self-healed by the idempotent, ungated backfill on the next migrate().
        let store = legacy_store_with_sessions(&[("feat-x", "p1", "2026-01-01T00:00:00Z")]);
        // Force the stranded state directly, bypassing normal inserts.
        store
            .conn
            .execute(
                "update agent_sessions set initial_branch = '' where id = 'feat-x'",
                [],
            )
            .unwrap();

        store.migrate().unwrap();

        let loaded = store.load_sessions().unwrap();
        let s = loaded.iter().find(|s| s.id == "feat-x").expect("row");
        assert_eq!(
            s.initial_branch, "feat-x",
            "an empty initial_branch must be self-healed to branch_name on migrate()"
        );
    }

    #[test]
    fn ensure_column_returns_false_when_column_already_exists() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("create table t (id text primary key, extra text);")
            .unwrap();
        assert!(
            !ensure_column(&conn, "t", "extra", "text").unwrap(),
            "an existing column must report Ok(false)"
        );
        // And adding a genuinely new column reports Ok(true).
        assert!(ensure_column(&conn, "t", "brand_new", "text").unwrap());
    }

    #[test]
    fn duplicate_column_error_is_classified() {
        // Exercise the concurrent-add tolerance path: a raw ALTER on an existing
        // column raises SQLite's "duplicate column name" error, which
        // ensure_column swallows as Ok(false).
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("create table t (id text primary key, extra text);")
            .unwrap();
        let err = conn
            .execute("alter table t add column extra text", [])
            .unwrap_err();
        assert!(
            is_duplicate_column_error(&err),
            "expected a duplicate-column classification, got: {err}"
        );
    }

    #[test]
    fn ensure_column_tolerates_column_added_by_another_connection() {
        // Cross-connection duplicate-column tolerance (the real-world race the
        // autocommit ALTER guards against): once one connection has committed
        // the column, a SECOND connection's ensure_column must report Ok(false)
        // and not error. This is the on-disk two-connection variant of the
        // concurrent first-boot add.
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("t.sqlite3");
        let conn1 = Connection::open(&db).unwrap();
        conn1
            .execute_batch("create table t (id text primary key);")
            .unwrap();
        assert!(
            ensure_column(&conn1, "t", "c", "text").unwrap(),
            "first add"
        );

        let conn2 = Connection::open(&db).unwrap();
        assert!(
            !ensure_column(&conn2, "t", "c", "text").unwrap(),
            "a second connection must tolerate the already-present column as Ok(false)"
        );
    }

    #[test]
    fn reopening_same_db_file_remigrates_cleanly() {
        // F3 regression: with the initial_branch ALTER moved to autocommit,
        // re-opening the same on-disk DB re-runs migrate() and every
        // ensure_column hits already-present without hard-failing open().
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("sessions.sqlite3");
        {
            let _s = SessionStore::open(&db).unwrap();
        }
        // A second open() must succeed (no SQLITE_BUSY_SNAPSHOT / duplicate-column
        // hard failure on the re-run migration).
        let _s = SessionStore::open(&db).unwrap();
    }

    #[test]
    fn migrate_reheals_stranded_all_zero_sort_order() {
        // F4 regression: a crash between the (autocommitted) sort_order ALTER and
        // its backfill strands every row at 0. The previous gating (only when
        // ensure_column just added the column) skipped the backfill forever on
        // the next boot. Now migrate() detects the stranded fingerprint (a
        // project with 2+ sessions all at 0) and re-runs the backfill.
        let store = legacy_store_with_sessions(&[
            ("p1-old", "p1", "2026-01-01T00:00:00Z"),
            ("p1-new", "p1", "2026-03-01T00:00:00Z"),
        ]);
        // Simulate the stranded half-migration: column present, all rows at 0.
        store
            .conn
            .execute("update agent_sessions set sort_order = 0", [])
            .unwrap();
        assert!(
            store.session_sort_order_needs_backfill().unwrap(),
            "two same-project sessions both at 0 must read as stranded"
        );

        store.migrate().unwrap();

        // The stored value changed (proving the backfill re-ran, not just the
        // load-time all-zero fallback): older session sorts to position 1.
        let so_old: i64 = store
            .conn
            .query_row(
                "select sort_order from agent_sessions where id = 'p1-old'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        let so_new: i64 = store
            .conn
            .query_row(
                "select sort_order from agent_sessions where id = 'p1-new'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!((so_new, so_old), (0, 1), "updated_at DESC → new=0, old=1");
        // And once healed, the fingerprint no longer trips (no destructive re-run).
        assert!(!store.session_sort_order_needs_backfill().unwrap());
    }

    #[test]
    fn duplicate_sort_order_triggers_global_backfill_then_settles() {
        // The flat model needs a GLOBAL total order. Two sessions in different
        // projects both at sort_order 0 (legacy per-project numbering) is NOT a
        // valid global order, so the backfill must run to give them distinct
        // positions — then never re-run once globalized.
        let store = test_store();
        let now = Utc::now();
        store
            .upsert_session(&test_session_in("solo-a", "pA", now, now))
            .unwrap();
        store
            .upsert_session(&test_session_in("solo-b", "pB", now, now))
            .unwrap();
        store
            .conn
            .execute("update agent_sessions set sort_order = 0", [])
            .unwrap();
        assert!(
            store.session_sort_order_needs_backfill().unwrap(),
            "a shared sort_order is not a global total order"
        );

        store.backfill_session_sort_order().unwrap();
        assert!(
            !store.session_sort_order_needs_backfill().unwrap(),
            "a globalized order is distinct and never re-runs"
        );
        let distinct: i64 = store
            .conn
            .query_row(
                "select count(distinct sort_order) from agent_sessions",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(distinct, 2, "every session gets a distinct global position");
    }

    #[test]
    fn migration_never_overwrites_an_existing_non_null_title() {
        // A legacy row that already carries a user-authored title must keep it
        // through the one-time backfill (the `title IS NULL` guard protects it).
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            create table agent_sessions (
                id text primary key,
                project_id text not null,
                provider text not null,
                source_branch text not null,
                branch_name text not null,
                worktree_path text not null,
                title text,
                project_path text,
                status text not null,
                created_at text not null,
                updated_at text not null
            );
            "#,
        )
        .unwrap();
        conn.execute(
            r#"
            insert into agent_sessions (
                id, project_id, provider, source_branch, branch_name,
                worktree_path, title, project_path, status, created_at, updated_at
            ) values ('id1', 'p1', 'claude', 'main', 'feat-x', '/tmp/x',
                      'My Named Agent', null, 'detached',
                      '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z')
            "#,
            [],
        )
        .unwrap();
        let store = SessionStore { conn };
        store.migrate().unwrap();

        let loaded = store.load_sessions().unwrap();
        let s = loaded.iter().find(|s| s.id == "id1").expect("row");
        assert_eq!(
            s.title.as_deref(),
            Some("My Named Agent"),
            "an already-set title must never be overwritten by the backfill"
        );
    }

    /// Builds a legacy `agent_sessions` table (no `sort_order` column) and seeds
    /// it with rows so the migration's backfill has something to number.
    fn legacy_store_with_sessions(rows: &[(&str, &str, &str)]) -> SessionStore {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            create table agent_sessions (
                id text primary key,
                project_id text not null,
                provider text not null,
                source_branch text not null,
                branch_name text not null,
                worktree_path text not null,
                title text,
                project_path text,
                status text not null,
                created_at text not null,
                updated_at text not null
            );
            "#,
        )
        .unwrap();
        for (id, project_id, updated_at) in rows {
            conn.execute(
                r#"
                insert into agent_sessions (
                    id, project_id, provider, source_branch, branch_name,
                    worktree_path, title, project_path, status, created_at, updated_at
                ) values (?1, ?2, 'claude', 'main', ?1, '/tmp/x', null, null, 'detached', ?3, ?3)
                "#,
                params![id, project_id, updated_at],
            )
            .unwrap();
        }
        let store = SessionStore { conn };
        store.migrate().unwrap();
        store
    }

    #[test]
    fn migration_backfill_preserves_updated_at_desc_order_per_project() {
        // Two projects, interleaved updated_at values. After backfill, each
        // project's sessions must be numbered 0..n following updated_at DESC,
        // and load_sessions (sort_order asc, updated_at desc) must reflect that.
        let store = legacy_store_with_sessions(&[
            ("p1-old", "p1", "2026-01-01T00:00:00Z"),
            ("p1-new", "p1", "2026-03-01T00:00:00Z"),
            ("p1-mid", "p1", "2026-02-01T00:00:00Z"),
            ("p2-new", "p2", "2026-05-01T00:00:00Z"),
            ("p2-old", "p2", "2026-04-01T00:00:00Z"),
        ]);

        let loaded = store.load_sessions().unwrap();
        let ordered: Vec<(&str, &str)> = loaded
            .iter()
            .map(|s| (s.project_id.as_str(), s.id.as_str()))
            .collect();

        // Group the loaded ids by project and assert each project's internal
        // order is updated_at DESC. (Cross-project interleaving in the global
        // Vec is not meaningful — the UI groups by project.)
        let p1: Vec<&str> = ordered
            .iter()
            .filter(|(p, _)| *p == "p1")
            .map(|(_, id)| *id)
            .collect();
        let p2: Vec<&str> = ordered
            .iter()
            .filter(|(p, _)| *p == "p2")
            .map(|(_, id)| *id)
            .collect();
        assert_eq!(p1, vec!["p1-new", "p1-mid", "p1-old"]);
        assert_eq!(p2, vec!["p2-new", "p2-old"]);
    }

    #[test]
    fn reorder_sessions_assigns_zero_to_n_positions() {
        let store = test_store();
        let now = Utc::now();
        store
            .upsert_session(&test_session_in("a", "proj", now, now))
            .unwrap();
        store
            .upsert_session(&test_session_in("b", "proj", now, now))
            .unwrap();
        store
            .upsert_session(&test_session_in("c", "proj", now, now))
            .unwrap();

        // Reorder to a, b, c (explicitly) and confirm load order matches.
        store
            .reorder_sessions("proj", &["a".into(), "b".into(), "c".into()])
            .unwrap();
        let ids: Vec<String> = store
            .load_sessions()
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, vec!["a", "b", "c"]);

        // Reorder again to a different order; positions are reassigned 0..n.
        store
            .reorder_sessions("proj", &["c".into(), "a".into(), "b".into()])
            .unwrap();
        let ids: Vec<String> = store
            .load_sessions()
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, vec!["c", "a", "b"]);
    }

    #[test]
    fn reorder_sessions_does_not_touch_updated_at() {
        let store = test_store();
        let original = Utc::now() - chrono::Duration::hours(5);
        store
            .upsert_session(&test_session_in("a", "proj", original, original))
            .unwrap();
        store
            .upsert_session(&test_session_in("b", "proj", original, original))
            .unwrap();

        store
            .reorder_sessions("proj", &["a".into(), "b".into()])
            .unwrap();

        let loaded = store.load_sessions().unwrap();
        for session in loaded {
            assert_eq!(
                session.updated_at.timestamp(),
                original.timestamp(),
                "reorder must not bump updated_at for {}",
                session.id
            );
        }
    }

    #[test]
    fn reorder_sessions_is_scoped_to_project() {
        let store = test_store();
        let now = Utc::now();
        store
            .upsert_session(&test_session_in("a", "p1", now, now))
            .unwrap();
        store
            .upsert_session(&test_session_in("b", "p2", now, now))
            .unwrap();

        // Passing a foreign id in the wrong project is a silent no-op at the
        // storage layer (the WHERE project_id guard matches nothing). Engine
        // validation is what rejects such input; storage stays dumb.
        store.reorder_sessions("p1", &["b".into()]).unwrap();
        // b's sort_order in p2 is unchanged: it still loads.
        let p2_ids: Vec<String> = store
            .load_sessions()
            .unwrap()
            .into_iter()
            .filter(|s| s.project_id == "p2")
            .map(|s| s.id)
            .collect();
        assert_eq!(p2_ids, vec!["b"]);
    }

    #[test]
    fn reorder_projects_assigns_zero_to_n_positions() {
        let store = test_store();
        let mk = |id: &str| ProjectConfig {
            id: id.to_string(),
            path: format!("/repo/{id}"),
            name: Some(id.to_string()),
            default_provider: None,
            leading_branch: Some("main".to_string()),
            auto_reopen_agents: None,
            startup_command: None,
            env: Default::default(),
        };
        store.upsert_project(&mk("a")).unwrap();
        store.upsert_project(&mk("b")).unwrap();
        store.upsert_project(&mk("c")).unwrap();

        store
            .reorder_projects(&["c".into(), "a".into(), "b".into()])
            .unwrap();
        let ids: Vec<String> = store
            .load_projects()
            .unwrap()
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(ids, vec!["c", "a", "b"]);
    }

    #[test]
    fn load_sessions_tie_break_falls_back_to_updated_at_desc() {
        // Two sessions sharing the same sort_order tie-break by updated_at DESC.
        let store = test_store();
        let now = Utc::now();
        store
            .upsert_session(&test_session_in(
                "older",
                "proj",
                now,
                now - Duration::hours(2),
            ))
            .unwrap();
        store
            .upsert_session(&test_session_in(
                "newer",
                "proj",
                now,
                now - Duration::hours(1),
            ))
            .unwrap();
        // Force both to the same sort_order so only the tie-break differs.
        store
            .conn
            .execute("update agent_sessions set sort_order = 0", [])
            .unwrap();

        let ids: Vec<String> = store
            .load_sessions()
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, vec!["newer", "older"]);
    }

    #[test]
    fn min_session_sort_order_reports_top_position() {
        let store = test_store();
        let now = Utc::now();
        assert_eq!(store.min_session_sort_order("proj").unwrap(), None);
        store
            .upsert_session(&test_session_in("a", "proj", now, now))
            .unwrap(); // sort_order 0
        store
            .upsert_session(&test_session_in("b", "proj", now, now))
            .unwrap(); // sort_order -1
        assert_eq!(store.min_session_sort_order("proj").unwrap(), Some(-1));
    }

    #[test]
    fn half_upgrade_all_zero_sort_orders_fall_back_to_legacy_order() {
        // Simulates the crash window where the sort_order column was added but
        // the one-time backfill never ran (on the next start `ensure_column`
        // reports "already present", so the backfill is permanently skipped):
        // every row ties at 0, and load_sessions must fall back to the legacy
        // updated_at DESC order. New sessions must still land on top at -1,
        // and the first explicit reorder self-heals positions to 0..n.
        let store = test_store();
        let now = Utc::now();
        store
            .upsert_session(&test_session_in(
                "old",
                "proj",
                now - Duration::minutes(10),
                now - Duration::minutes(10),
            ))
            .unwrap();
        store
            .upsert_session(&test_session_in("new", "proj", now, now))
            .unwrap();
        // Flatten every position to 0 — the half-upgraded state.
        store
            .conn
            .execute("update agent_sessions set sort_order = 0", [])
            .unwrap();

        let ids: Vec<String> = store
            .load_sessions()
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, ["new", "old"]); // updated_at DESC tie-break

        store
            .upsert_session(&test_session_in("fresh", "proj", now, now))
            .unwrap();
        let first = store.load_sessions().unwrap().remove(0);
        assert_eq!(first.id, "fresh"); // -1 sorts above the zeros
    }
}

#[cfg(test)]
mod pr_tests {
    use super::*;
    use chrono::Duration;

    fn spr(sid: &str, num: u64, host: &str, repo: &str, state: &str, title: &str) -> StoredPr {
        StoredPr {
            session_id: sid.to_string(),
            pr_number: num,
            host: host.to_string(),
            owner_repo: repo.to_string(),
            state: state.to_string(),
            title: title.to_string(),
            url: fallback_pr_url(host, repo, num),
        }
    }

    #[test]
    fn upsert_and_load_prs() {
        let store = test_store();
        let now = Utc::now();
        let s = test_session("s1", now, now);
        store.upsert_session(&s).unwrap();

        store
            .upsert_pr(&spr(
                "s1",
                10,
                "github.com",
                "owner/repo",
                "OPEN",
                "First PR",
            ))
            .unwrap();
        store
            .upsert_pr(&spr(
                "s1",
                20,
                "github.com",
                "owner/repo",
                "OPEN",
                "Second PR",
            ))
            .unwrap();
        store
            .upsert_pr(&spr(
                "s1",
                15,
                "github.com",
                "owner/repo",
                "MERGED",
                "Middle PR",
            ))
            .unwrap();

        let prs = store.load_prs("s1").unwrap();
        assert_eq!(prs.len(), 3);
        assert_eq!(
            prs[0],
            spr("s1", 20, "github.com", "owner/repo", "OPEN", "Second PR")
        );
        assert_eq!(
            prs[1],
            spr("s1", 15, "github.com", "owner/repo", "MERGED", "Middle PR")
        );
        assert_eq!(
            prs[2],
            spr("s1", 10, "github.com", "owner/repo", "OPEN", "First PR")
        );
    }

    #[test]
    fn upsert_pr_updates_state_and_title() {
        let store = test_store();
        let now = Utc::now();
        let s = test_session("s1", now, now);
        store.upsert_session(&s).unwrap();

        store
            .upsert_pr(&spr("s1", 42, "github.com", "owner/repo", "OPEN", "My PR"))
            .unwrap();
        store
            .upsert_pr(&StoredPr {
                url: "https://github.com/owner/repo/pull/42".to_string(),
                ..spr(
                    "s1",
                    42,
                    "github.example.com",
                    "owner/repo",
                    "MERGED",
                    "My PR (updated)",
                )
            })
            .unwrap();

        let prs = store.load_prs("s1").unwrap();
        assert_eq!(prs.len(), 1);
        assert_eq!(prs[0].host, "github.example.com");
        assert_eq!(prs[0].state, "MERGED");
        assert_eq!(prs[0].title, "My PR (updated)");
        assert_eq!(prs[0].url, "https://github.com/owner/repo/pull/42");
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
            .upsert_pr(&spr(
                "s1",
                10,
                "github.com",
                "owner/repo",
                "CLOSED",
                "Old PR",
            ))
            .unwrap();
        store
            .upsert_pr(&spr(
                "s1",
                20,
                "github.com",
                "owner/repo",
                "MERGED",
                "Latest PR",
            ))
            .unwrap();
        store
            .upsert_pr(&spr(
                "s2",
                5,
                "github.com",
                "other/repo",
                "OPEN",
                "Other PR",
            ))
            .unwrap();

        let latest = store.load_all_latest_prs().unwrap();
        assert_eq!(latest.len(), 2);
        assert!(latest.contains(&spr(
            "s1",
            20,
            "github.com",
            "owner/repo",
            "MERGED",
            "Latest PR"
        )));
        assert!(latest.contains(&spr(
            "s2",
            5,
            "github.com",
            "other/repo",
            "OPEN",
            "Other PR"
        )));
    }
}

/// Adds `column` to `table` if it is missing. Returns `true` when the column
/// was just added by this call, `false` when it already existed. Callers that
/// need a one-time backfill of a newly-added column branch on the return value.
fn ensure_column(conn: &Connection, table: &str, column: &str, sql_type: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("pragma table_info({table})"))?;
    let existing = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if existing.iter().any(|name| name == column) {
        return Ok(false);
    }
    match conn.execute(
        &format!("alter table {table} add column {column} {sql_type}"),
        [],
    ) {
        Ok(_) => Ok(true),
        // Tolerate a concurrent add: two connections opening at first-boot-after
        // -upgrade can both pass the pragma check above and race on the ALTER.
        // The loser sees SQLite's "duplicate column name" error — the column is
        // present, so treat it as already-existing (Ok(false)) instead of
        // hard-failing open().
        Err(e) if is_duplicate_column_error(&e) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// True when `err` is SQLite's "duplicate column name" error, raised when an
/// `alter table ... add column` targets a column that already exists (e.g. a
/// concurrent connection added it first).
fn is_duplicate_column_error(err: &rusqlite::Error) -> bool {
    err.to_string().to_lowercase().contains("duplicate column")
}
