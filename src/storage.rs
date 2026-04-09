use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

use crate::model::{AgentSession, SessionStatus};

/// A stored PR association loaded from the database.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoredPr {
    pub session_id: String,
    pub pr_number: u64,
    pub owner_repo: String,
    pub state: String,
    pub title: String,
}

pub struct SessionStore {
    conn: Connection,
}

impl SessionStore {
    pub fn open(path: &std::path::Path) -> Result<Self> {
        let conn =
            Connection::open(path).with_context(|| format!("failed to open {}", path.display()))?;
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
        ensure_column(&self.conn, "agent_sessions", "title", "text")?;
        ensure_column(&self.conn, "agent_sessions", "project_path", "text")?;
        self.conn.execute_batch(
            r#"
            create table if not exists session_prs (
                session_id text not null,
                pr_number integer not null,
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
            "state",
            "text not null default 'OPEN'",
        )?;
        ensure_column(
            &self.conn,
            "session_prs",
            "title",
            "text not null default ''",
        )?;
        Ok(())
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
        self.conn.execute(
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
        let mut stmt = self.conn.prepare(
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
        let mut stmt = self.conn.prepare(
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
        self.conn.execute(
            r#"
            insert into agent_sessions
                (id, project_id, project_path, provider, source_branch, branch_name, worktree_path, title, status, created_at, updated_at)
            values
                (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)
            on conflict(id) do update set
                project_path=excluded.project_path,
                provider=excluded.provider,
                source_branch=excluded.source_branch,
                branch_name=excluded.branch_name,
                worktree_path=excluded.worktree_path,
                title=excluded.title,
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
                session.status.as_str(),
                session.created_at.to_rfc3339(),
                session.updated_at.to_rfc3339(),
            ],
        )?;
        Ok(())
    }

    pub fn load_sessions(&self) -> Result<Vec<AgentSession>> {
        let mut stmt = self.conn.prepare(
            r#"
            select id, project_id, provider, source_branch, branch_name, worktree_path, title, project_path, status, created_at, updated_at
            from agent_sessions
            order by updated_at desc
            "#,
        )?;
        let rows = stmt.query_map([], |row| {
            let created_at: String = row.get(9)?;
            let updated_at: String = row.get(10)?;
            Ok(AgentSession {
                id: row.get(0)?,
                project_id: row.get::<_, String>(1).unwrap_or_default(),
                provider: crate::model::ProviderKind::from_str(row.get::<_, String>(2)?.as_str()),
                source_branch: row.get(3)?,
                branch_name: row.get(4)?,
                worktree_path: row.get(5)?,
                title: row.get(6)?,
                project_path: row.get(7)?,
                status: SessionStatus::from_str(row.get::<_, String>(8)?.as_str()),
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
        self.conn
            .execute("delete from agent_sessions where id = ?1", params![id])?;
        Ok(())
    }
}

fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
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

fn ensure_column(conn: &Connection, table: &str, column: &str, sql_type: &str) -> Result<()> {
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
