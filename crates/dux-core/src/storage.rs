use std::collections::BTreeMap;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

use crate::config::ProjectConfig;
use crate::model::{AgentSession, SessionStatus};

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

pub struct SessionStore {
    conn: Connection,
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
        // Persisted per-project display order for agent sessions. When the
        // column is added on an existing database, backfill positions per
        // project from the legacy `updated_at DESC` order so the visible order
        // is preserved exactly across the upgrade.
        if ensure_column(
            &self.conn,
            "agent_sessions",
            "sort_order",
            "integer not null default 0",
        )? {
            self.backfill_session_sort_order()?;
        }
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
        Ok(())
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
                updated_at=?12
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
                (id, project_id, project_path, provider, source_branch, branch_name, worktree_path, title, started_providers, desired_running, auto_reopen_enabled, status, sort_order, created_at, updated_at)
            values
                (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
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

    /// One-time backfill run when the `sort_order` column is first added to an
    /// existing `agent_sessions` table. Numbers each project's sessions
    /// `0,1,2,…` following the legacy `updated_at DESC` order so the visible
    /// order is preserved exactly after the upgrade.
    fn backfill_session_sort_order(&self) -> Result<()> {
        let mut stmt = self.conn.prepare(
            "select id, project_id from agent_sessions order by project_id, updated_at desc",
        )?;
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        let tx = self.conn.unchecked_transaction()?;
        {
            let mut update =
                tx.prepare("update agent_sessions set sort_order = ?1 where id = ?2")?;
            let mut position = 0i64;
            let mut current_project: Option<String> = None;
            for (id, project_id) in rows {
                if current_project.as_deref() != Some(project_id.as_str()) {
                    position = 0;
                    current_project = Some(project_id);
                }
                update.execute(params![position, id])?;
                position += 1;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn load_sessions(&self) -> Result<Vec<AgentSession>> {
        let mut stmt = self.conn.prepare(
            r#"
            select id, project_id, provider, source_branch, branch_name, worktree_path, title, project_path, started_providers, desired_running, auto_reopen_enabled, status, created_at, updated_at
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
            })
        })?;

        let mut sessions = Vec::new();
        for row in rows {
            sessions.push(row?);
        }
        Ok(sessions)
    }

    pub fn delete_session(&self, id: &str) -> Result<()> {
        // Clear the session's PR associations first. `session_prs` declares an
        // ON DELETE CASCADE FK to `agent_sessions`, but the connection never
        // enables `PRAGMA foreign_keys`, so that cascade does not fire — delete
        // the rows explicitly to avoid leaking orphaned PR records.
        self.conn
            .execute("delete from session_prs where session_id = ?1", params![id])?;
        // Drop the per-session changed-files revision counter too, so a deleted
        // session leaves no housekeeping rows behind.
        self.conn
            .execute("delete from changes_rev where session_id = ?1", params![id])?;
        self.conn
            .execute("delete from agent_sessions where id = ?1", params![id])?;
        Ok(())
    }

    pub fn set_desired_running(&self, id: &str, desired_running: bool) -> Result<()> {
        self.conn.execute(
            "update agent_sessions set desired_running = ?2, updated_at = ?3 where id = ?1",
            params![id, desired_running, Utc::now().to_rfc3339()],
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
        worktree_path: format!("/tmp/{id}"),
        title: None,
        started_providers: Vec::new(),
        desired_running: false,
        auto_reopen_enabled: true,
        status: SessionStatus::Active,
        created_at,
        updated_at,
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

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
    conn.execute(
        &format!("alter table {table} add column {column} {sql_type}"),
        [],
    )?;
    Ok(true)
}
