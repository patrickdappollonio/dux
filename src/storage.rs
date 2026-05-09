use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

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
        ensure_column(
            &self.conn,
            "agent_sessions",
            "started_providers",
            "text not null default '[]'",
        )?;
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
        Ok(())
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
        self.conn.execute(
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
        let mut stmt = self.conn.prepare(
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
