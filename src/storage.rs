use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

use crate::model::{AgentSession, SessionStatus};

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
        Ok(())
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
