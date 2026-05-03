-- 0001_initial_schema.sql
--
-- Captures the canonical dux schema as of audit02 P1-Y (Phase 19). This
-- migration is the first entry in the `MIGRATIONS` table in
-- `src/storage.rs` and therefore runs on every fresh database. It is
-- IMMUTABLE: never edit this file. If a column or table needs to change,
-- write a new migration (`0002_*.sql`, `0003_*.sql`, ...).
--
-- Every CREATE statement uses `IF NOT EXISTS` so that rerunning this
-- migration on a database that already has the canonical schema (e.g. a
-- DB created before `PRAGMA user_version` was wired up) is a no-op. The
-- companion `ensure_column` shims in `src/storage.rs` are deprecated and
-- only retained so that legacy databases — which lacked the
-- `started_providers`, `state`, and `title` columns added historically
-- via ALTER TABLE — keep working. New schema additions must come via a
-- numbered migration in this directory.

create table if not exists agent_sessions (
    id text primary key,
    project_id text not null,
    provider text not null,
    source_branch text not null,
    branch_name text not null,
    worktree_path text not null,
    title text,
    project_path text,
    started_providers text not null default '[]',
    status text not null,
    created_at text not null,
    updated_at text not null
);

create table if not exists session_prs (
    session_id text not null,
    pr_number integer not null,
    owner_repo text not null,
    state text not null default 'OPEN',
    title text not null default '',
    primary key (session_id, pr_number),
    foreign key (session_id) references agent_sessions(id) on delete cascade
);
