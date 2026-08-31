use std::collections::BTreeMap;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

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
                branch_provenance text not null default 'created',
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
        // Where the agent's branch came from, deciding whether a delete may
        // force-delete it. Additive column, same autocommit ALTER rationale as
        // `initial_branch` above.
        //
        // The default is 'created' for existing rows on purpose: it preserves
        // exactly today's behavior for every agent that predates the column.
        // Defaulting to a kept variant instead would silently stop branch
        // cleanup for every existing agent, resurrecting the "create foo,
        // delete foo, recreate foo -> branch already exists" leak, and the true
        // provenance of an old row is unknowable anyway. No backfill beyond the
        // default is needed or possible.
        ensure_column(
            &self.conn,
            "agent_sessions",
            "branch_provenance",
            "text not null default 'created'",
        )?;
        // WHICH SHAPE THIS ROW IS: a managed working copy dux owns, or a
        // folder the user already had. Additive column, same autocommit ALTER
        // rationale as `initial_branch` above.
        //
        // The default is 'managed' for existing rows because every row that
        // predates this column IS one: there was no other kind of agent. No
        // backfill beyond the default is needed or possible.
        //
        // This column is read FIRST, before any git column is believed,
        // because a standalone row stores empty text under `project_id`,
        // `branch_name`, `source_branch`, `initial_branch` and
        // `worktree_path` (they are NOT NULL, or predate this feature). Read
        // in the other order those empties become facts: a branch named "", a
        // worktree path of "" that a delete path would try to remove.
        ensure_column(
            &self.conn,
            "agent_sessions",
            "workspace_kind",
            "text not null default 'managed'",
        )?;
        // The folder a standalone agent runs in. NULL for a managed row, which
        // is the honest spelling: there is no folder, as opposed to an empty
        // one.
        ensure_column(&self.conn, "agent_sessions", "folder_path", "text")?;
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
            //
            // GATED ON THE KIND COLUMN. Note what the gate does and does not
            // buy: a standalone row has an empty `initial_branch` AND an empty
            // `branch_name` permanently by design, so the assignment itself
            // would be '' to '', a no-op. The gate is here so the statement can
            // never START mattering for folder rows: it says out loud that this
            // heal is about branch identity, which they have none of, and it
            // keeps a future change to either side (a default branch name, a
            // non-empty placeholder) from quietly writing one onto them.
            tx.execute(
                "update agent_sessions set initial_branch = branch_name \
                 where workspace_kind = 'managed' \
                   and (initial_branch = '' or initial_branch is null)",
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
                //
                // Gated on the kind column for the same reason as the backfill
                // above: freezing a standalone row's empty branch name into its
                // title would leave the row with no label at all. A standalone
                // agent always has a title anyway (creation enforces one), so
                // this arm has nothing to do for one.
                Some(tx.execute(
                    "update agent_sessions set title = branch_name \
                     where workspace_kind = 'managed' and title is null",
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
        // Which `agent_tabs` row currently occupies this agent's session slot:
        // its first tab, the one the user cannot close. Every tab is a row, so
        // this is a pointer rather than a synthesized identity, and moving it is
        // what promoting a sibling tab into the slot will mean.
        //
        // NULL means one thing only, and only for as long as `migrate()` is
        // running: "this session predates the pointer and has not been migrated
        // yet". `backfill_slot_tabs` below closes that window on every open, and
        // `heal_slot_tab_pointers` closes the other one (a pointer naming a row
        // that is gone). After `migrate()` returns, a session with no usable
        // pointer is a bug, not a state the read path tolerates.
        //
        // The table is created further down (`create table if not exists
        // agent_tabs`), and both passes below run after it, because they write
        // rows into it.
        ensure_column(&self.conn, "agent_sessions", "slot_tab_id", "text")?;
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
        // A manually attached ("pinned") pull request, one row per session,
        // mirroring `session_prs`'s columns. The FK is declared for parity with
        // `session_prs`, but the connection never enables `PRAGMA foreign_keys`,
        // so the cascade does not fire; `delete_session` and
        // `remove_project_records` delete these rows explicitly. The cached
        // state/title/url make a restart render the pin instantly, before the
        // first sync cycle refreshes them. Derived runtime state, so it lives
        // here and never in portable config.
        self.conn.execute_batch(
            r#"
            create table if not exists session_pr_overrides (
                session_id text primary key
                    references agent_sessions(id) on delete cascade,
                host       text not null,
                owner_repo text not null,
                pr_number  integer not null,
                state      text not null default 'OPEN',
                title      text not null default '',
                url        text not null default ''
            );
            "#,
        )?;
        // A session whose pull-request autodetection the user switched off by
        // detaching. One row per session, presence is the whole meaning, so the
        // table has a single column. Durable on purpose: a detach is a user
        // decision and a restart must not quietly resume detection. Like
        // `session_pr_overrides` the FK is declared for parity only (the
        // connection never enables `PRAGMA foreign_keys`), so `delete_session`
        // and `remove_project_records` delete these rows explicitly. Derived
        // runtime state, so it lives here and never in portable config.
        self.conn.execute_batch(
            r#"
            create table if not exists session_pr_suppressions (
                session_id text primary key
                    references agent_sessions(id) on delete cascade
            );
            "#,
        )?;
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
        // One row per provider tab, the agent's FIRST tab included: the
        // session-slot tab is a row like any other, named by
        // `agent_sessions.slot_tab_id`. Rows are removed when the owning session
        // (or its project) is deleted (see
        // `delete_session`/`remove_project_records`).
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
        // The slot-tab passes run last: they write `agent_tabs` rows, so the
        // table has to exist, and a failure in any of them aborts the open. A
        // workspace whose first tabs are unaddressable is worse than a startup
        // that says why it stopped.
        self.sweep_orphan_agent_tabs()?;
        self.backfill_slot_tabs()?;
        self.heal_slot_tab_pointers()?;
        Ok(())
    }

    /// Drop any `agent_tabs` row whose owning session is gone.
    ///
    /// Belt-and-suspenders for rows an older binary (which predates the cascade,
    /// and the table) could have left behind when deleting a session. It runs
    /// here rather than in a reader so it happens exactly once per open, ahead of
    /// the two repair passes below, which both reason about "the session's tabs".
    fn sweep_orphan_agent_tabs(&self) -> Result<()> {
        self.conn
            .execute(
                "delete from agent_tabs where session_id not in (select id from agent_sessions)",
                [],
            )
            .context("failed to sweep tab rows whose agent no longer exists")?;
        Ok(())
    }

    /// Give every session that predates the slot pointer a real first-tab row,
    /// in one transaction.
    ///
    /// A pre-pointer session's first tab was synthesized from the session record
    /// and had no row, so the row is MINTED here rather than adopted from the
    /// session's existing tabs: adopting one would silently turn tab 2 into tab 1
    /// and lose a tab.
    ///
    /// The minted row is placed one below the session's current minimum
    /// `sort_order`. That arithmetic is not what puts the first tab at the front
    /// of the strip today, because both surfaces render the slot tab first by
    /// following the pointer and then the extras by `(sort_order, created_at)`.
    /// It is there so the stamp keeps telling the truth if the slot is ever
    /// PROMOTED to another tab: at that point the row's own position is what
    /// orders it, and a first tab stamped above its successors would jump.
    ///
    /// Idempotent: a session with a pointer is not touched, so a second run
    /// changes nothing.
    fn backfill_slot_tabs(&self) -> Result<()> {
        let pending: Vec<(String, String, String)> = {
            let mut stmt = self.conn.prepare(
                "select id, provider, created_at from agent_sessions \
                 where slot_tab_id is null or trim(slot_tab_id) = ''",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get::<_, String>(2)?))
            })?;
            let mut pending = Vec::new();
            for row in rows {
                pending.push(row?);
            }
            pending
        };
        if pending.is_empty() {
            return Ok(());
        }
        let count = pending.len();
        let tx = self
            .conn
            .unchecked_transaction()
            .context("failed to start the slot tab migration")?;
        for (session_id, provider, created_at) in pending {
            let tab_id = uuid::Uuid::new_v4().to_string();
            // One below whatever the session's existing tabs start at, so the
            // minted first tab leads the strip and every extra tab keeps the
            // position it had.
            let sort_order: i64 = tx.query_row(
                "select coalesce(min(sort_order), 1) - 1 from agent_tabs where session_id = ?1",
                params![session_id],
                |row| row.get(0),
            )?;
            tx.execute(
                "insert into agent_tabs (id, session_id, provider, sort_order, created_at) \
                 values (?1, ?2, ?3, ?4, ?5)",
                params![tab_id, session_id, provider, sort_order, created_at],
            )
            .with_context(|| format!("failed to mint the slot tab row for session {session_id}"))?;
            tx.execute(
                "update agent_sessions set slot_tab_id = ?2 where id = ?1",
                params![session_id, tab_id],
            )
            .with_context(|| format!("failed to record the slot tab for session {session_id}"))?;
        }
        tx.commit()
            .context("failed to commit the slot tab migration")?;
        crate::logger::info(&format!(
            "one-time migration: gave {count} session(s) a stored first tab"
        ));
        Ok(())
    }

    /// Repair a slot pointer that names no tab of its own session, out loud.
    ///
    /// "No tab of its own session" covers both a row that is gone and a row that
    /// belongs to some other agent: a pointer across sessions would otherwise
    /// resolve, and the slot of one agent would be a tab living in another's
    /// strip.
    ///
    /// Distinct from [`Self::backfill_slot_tabs`] on purpose. An EMPTY pointer
    /// (NULL or blank, spelled the same way in both passes) means "not migrated
    /// yet" and its first tab has to be minted; a DANGLING pointer
    /// means the row it named is gone, and the honest repair is to hand the slot
    /// to the session's oldest surviving tab (which the user is already looking
    /// at) rather than mint a tab nothing has ever run in. Only a session with no
    /// tabs left at all falls back to minting one.
    fn heal_slot_tab_pointers(&self) -> Result<()> {
        let dangling: Vec<(String, String, String, String)> = {
            let mut stmt = self.conn.prepare(
                "select s.id, s.slot_tab_id, s.provider, s.created_at from agent_sessions s \
                 where s.slot_tab_id is not null and trim(s.slot_tab_id) <> '' \
                   and not exists (select 1 from agent_tabs t \
                                   where t.id = s.slot_tab_id and t.session_id = s.id)",
            )?;
            let rows = stmt.query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })?;
            let mut dangling = Vec::new();
            for row in rows {
                dangling.push(row?);
            }
            dangling
        };
        if dangling.is_empty() {
            return Ok(());
        }
        let tx = self
            .conn
            .unchecked_transaction()
            .context("failed to start the slot tab repair")?;
        for (session_id, stale, provider, created_at) in dangling {
            let oldest: Option<(String, String)> = tx
                .query_row(
                    "select id, provider from agent_tabs where session_id = ?1 \
                     order by sort_order, created_at limit 1",
                    params![session_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let (tab_id, how) = match oldest {
                Some((id, adopted_provider)) => {
                    // `agent_sessions.provider` mirrors the SLOT tab's provider
                    // and is what a launch reads, so the mirror has to move with
                    // the slot. Leaving it behind would relaunch the vanished
                    // tab's provider in the adopted tab's PTY.
                    tx.execute(
                        "update agent_sessions set provider = ?2 where id = ?1",
                        params![session_id, adopted_provider],
                    )
                    .with_context(|| {
                        format!(
                            "failed to move the provider mirror to the adopted slot tab \
                             for session {session_id}"
                        )
                    })?;
                    (id, "its oldest surviving tab")
                }
                None => {
                    let tab_id = uuid::Uuid::new_v4().to_string();
                    tx.execute(
                        "insert into agent_tabs (id, session_id, provider, sort_order, created_at) \
                         values (?1, ?2, ?3, 0, ?4)",
                        params![tab_id, session_id, provider, created_at],
                    )
                    .with_context(|| {
                        format!("failed to mint a replacement slot tab for session {session_id}")
                    })?;
                    (tab_id, "a freshly minted tab")
                }
            };
            tx.execute(
                "update agent_sessions set slot_tab_id = ?2 where id = ?1",
                params![session_id, tab_id],
            )
            .with_context(|| format!("failed to repair the slot tab for session {session_id}"))?;
            crate::logger::warn(&format!(
                "agent {session_id} pointed at a first tab ({stale}) that no longer exists; \
                 gave the slot to {how} ({tab_id})"
            ));
        }
        tx.commit()
            .context("failed to commit the slot tab repair")?;
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

    /// Load every tab, the slot tab included, ordered so a session's tabs come
    /// out in a stable creation order. A plain reader: orphan rows are swept once
    /// per open by [`Self::sweep_orphan_agent_tabs`], not on every read.
    pub fn load_agent_tabs(&self) -> Result<Vec<AgentTab>> {
        let mut stmt = self.conn.prepare(
            "select id, session_id, provider, sort_order, created_at \
             from agent_tabs order by session_id, sort_order, created_at",
        )?;
        let rows = stmt.query_map([], read_agent_tab)?;
        let mut tabs = Vec::new();
        for row in rows {
            tabs.push(row?);
        }
        Ok(tabs)
    }

    /// Every tab EXCEPT the one currently occupying its session's slot.
    ///
    /// The engine's in-memory `agent_tabs` map holds the extras; the slot tab is
    /// reached through the session record's pointer, which is also the mirror of
    /// its provider (see [`Self::set_slot_provider`]). Sessions and tabs are
    /// joined, so an orphan row cannot come back out even if
    /// [`Self::sweep_orphan_agent_tabs`] has not run.
    pub fn load_extra_agent_tabs(&self) -> Result<Vec<AgentTab>> {
        let mut stmt = self.conn.prepare(
            "select t.id, t.session_id, t.provider, t.sort_order, t.created_at \
             from agent_tabs t join agent_sessions s on s.id = t.session_id \
             where s.slot_tab_id is null or t.id <> s.slot_tab_id \
             order by t.session_id, t.sort_order, t.created_at",
        )?;
        let rows = stmt.query_map([], read_agent_tab)?;
        let mut tabs = Vec::new();
        for row in rows {
            tabs.push(row?);
        }
        Ok(tabs)
    }

    /// Retarget the provider of the tab occupying a session's slot, and the
    /// session's own `provider` column with it.
    ///
    /// `agent_sessions.provider` is a MIRROR of the slot tab's provider, kept
    /// because every read path in both surfaces asks the session for it. This is
    /// the one place a RETARGET writes either value, so a retarget cannot leave
    /// them disagreeing. It is not the only writer of the mirror: `upsert_session`
    /// stores whatever the in-memory session carries, and the migration's adopt
    /// repair moves the mirror when the slot moves.
    pub fn set_slot_provider(
        &self,
        session_id: &str,
        provider: &str,
        updated_at: DateTime<Utc>,
    ) -> Result<()> {
        let tx = self
            .conn
            .unchecked_transaction()
            .context("failed to start retargeting the agent's provider")?;
        tx.execute(
            "update agent_tabs set provider = ?2 where id = \
             (select slot_tab_id from agent_sessions where id = ?1)",
            params![session_id, provider],
        )?;
        tx.execute(
            "update agent_sessions set provider = ?2, updated_at = ?3 where id = ?1",
            params![session_id, provider, updated_at.to_rfc3339()],
        )?;
        tx.commit()
            .context("failed to commit the agent's new provider")?;
        Ok(())
    }

    /// Move a session's slot to one of its other tabs and delete the tab that
    /// was in the slot, in ONE transaction.
    ///
    /// This is what closing an agent's first tab does when it has siblings: the
    /// pointer names the successor, the session's `provider` mirror follows the
    /// promoted tab's provider (the mirror's rule is "whatever the slot tab
    /// runs"), and the departing tab's row goes away. All three or none: a
    /// pointer that moved without the old row being deleted leaves a tab in the
    /// strip whose PTY is being torn down, and a deletion without the pointer
    /// move leaves the agent naming a row that no longer exists.
    ///
    /// The promoted row keeps its `sort_order`. Renumbering it to 0 would buy
    /// nothing (both surfaces render the slot tab first from the pointer, not
    /// from the ordering) and would cost the identity this whole design is
    /// built on: the promoted tab changes role, not shape.
    ///
    /// Refuses when `new_slot_tab_id` is not a tab of `session_id` (including
    /// when it does not exist at all), because a pointer at a foreign or absent
    /// row is the one state no later read can recover from.
    ///
    /// The focus memory is normalized in the same statement: the slot tab is
    /// represented there as ABSENCE (see
    /// [`crate::model::AgentSession::last_focused_tab`]), so a memory naming
    /// either the promoted tab or the departing one becomes NULL. Doing it here
    /// rather than in a follow-up write is what keeps a restart from reading
    /// back a memory this promotion invalidated.
    pub fn promote_tab_to_slot(
        &self,
        session_id: &str,
        new_slot_tab_id: &str,
        old_slot_tab_id: &str,
        provider: &str,
        updated_at: DateTime<Utc>,
    ) -> Result<()> {
        if new_slot_tab_id == old_slot_tab_id {
            anyhow::bail!(
                "tab {new_slot_tab_id} cannot be promoted over itself: it is already the slot tab \
                 of agent {session_id}"
            );
        }
        let tx = self
            .conn
            .unchecked_transaction()
            .context("failed to start promoting a tab into the agent's slot")?;
        let owner: Option<String> = tx
            .query_row(
                "select session_id from agent_tabs where id = ?1",
                params![new_slot_tab_id],
                |row| row.get(0),
            )
            .optional()
            .context("failed to read the promoted tab's owning agent")?;
        match owner.as_deref() {
            Some(owner) if owner == session_id => {}
            Some(other) => anyhow::bail!(
                "tab {new_slot_tab_id} belongs to agent {other}, not {session_id}, so it cannot \
                 take its slot"
            ),
            None => anyhow::bail!("unknown tab {new_slot_tab_id}: it cannot take a slot"),
        }
        let sessions_updated = tx
            .execute(
                "update agent_sessions set slot_tab_id = ?2, provider = ?3, updated_at = ?4, \
                 last_focused_tab = case when last_focused_tab in (?2, ?5) then null \
                 else last_focused_tab end \
                 where id = ?1",
                params![
                    session_id,
                    new_slot_tab_id,
                    provider,
                    updated_at.to_rfc3339(),
                    old_slot_tab_id,
                ],
            )
            .context("failed to point the agent at its new slot tab")?;
        if sessions_updated == 0 {
            anyhow::bail!("unknown agent {session_id}: its slot cannot be moved");
        }
        let removed = tx
            .execute(
                "delete from agent_tabs where id = ?1 and session_id = ?2",
                params![old_slot_tab_id, session_id],
            )
            .context("failed to delete the tab that was in the agent's slot")?;
        if removed == 0 {
            crate::logger::warn(&format!(
                "promote_tab_to_slot found no row for the outgoing slot tab {old_slot_tab_id} of \
                 agent {session_id} — the in-memory tab map and SQLite may have diverged",
            ));
        }
        tx.commit()
            .context("failed to commit the agent's new slot tab")?;
        Ok(())
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

    /// How many tabs one session has, the slot tab included: every tab is a
    /// row, so this is the number the per-agent cap is compared against directly.
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
    /// EVERY statement below scopes on `workspace_kind = 'managed'` as well as
    /// the project id, and that is not belt and braces. A standalone agent
    /// stores EMPTY TEXT under `project_id` (the column is NOT NULL), so a
    /// project whose id is the empty string would otherwise sweep up every
    /// standalone agent the user has. The kind column, not the project id, is
    /// what says who owns a row.
    pub fn remove_project_records(&self, project_id: &str) -> Result<Vec<String>> {
        let tx = self.conn.unchecked_transaction()?;
        let ids: Vec<String> = {
            let mut stmt = tx.prepare("select id from agent_sessions where project_id = ?1 and workspace_kind = 'managed'")?;
            let rows = stmt.query_map(params![project_id], |row| row.get::<_, String>(0))?;
            rows.collect::<rusqlite::Result<Vec<String>>>()?
        };
        tx.execute(
            "delete from session_prs where session_id in \
             (select id from agent_sessions where project_id = ?1 and workspace_kind = 'managed')",
            params![project_id],
        )?;
        tx.execute(
            "delete from session_pr_overrides where session_id in \
             (select id from agent_sessions where project_id = ?1 and workspace_kind = 'managed')",
            params![project_id],
        )?;
        tx.execute(
            "delete from session_pr_suppressions where session_id in \
             (select id from agent_sessions where project_id = ?1 and workspace_kind = 'managed')",
            params![project_id],
        )?;
        // Drop the per-session changed-files rev counters BEFORE the sessions
        // themselves (the subquery resolves the ids while the rows still exist),
        // so a project removal cannot leave orphaned `changes_rev` rows behind.
        tx.execute(
            "delete from changes_rev where session_id in \
             (select id from agent_sessions where project_id = ?1 and workspace_kind = 'managed')",
            params![project_id],
        )?;
        // Drop the sessions' extra tabs BEFORE the sessions themselves (the
        // subquery resolves the ids while the parent rows still exist), so a
        // project removal cannot leave orphaned `agent_tabs` rows behind.
        tx.execute(
            "delete from agent_tabs where session_id in \
             (select id from agent_sessions where project_id = ?1 and workspace_kind = 'managed')",
            params![project_id],
        )?;
        tx.execute(
            "delete from agent_sessions where project_id = ?1 and workspace_kind = 'managed'",
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

    /// Insert or replace a session's manually attached (pinned) pull request.
    /// One row per session: attaching again replaces the previous pin.
    pub fn upsert_pr_override(&self, pr: &StoredPr) -> Result<()> {
        self.conn.execute(
            r#"
            insert into session_pr_overrides
                (session_id, host, owner_repo, pr_number, state, title, url)
            values (?1, ?2, ?3, ?4, ?5, ?6, ?7)
            on conflict(session_id) do update set
                host=excluded.host,
                owner_repo=excluded.owner_repo,
                pr_number=excluded.pr_number,
                state=excluded.state,
                title=excluded.title,
                url=excluded.url
            "#,
            params![
                pr.session_id,
                pr.host,
                pr.owner_repo,
                pr.pr_number as i64,
                pr.state,
                pr.title,
                pr.url
            ],
        )?;
        Ok(())
    }

    /// Load every session's pinned pull request (at most one per session).
    pub fn load_pr_overrides(&self) -> Result<Vec<StoredPr>> {
        let mut stmt = self.conn.prepare(
            r#"
            select session_id, pr_number, host, owner_repo, state, title, url
            from session_pr_overrides
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

    /// Remove a session's pinned pull request, if any (a no-op otherwise).
    pub fn delete_pr_override(&self, session_id: &str) -> Result<()> {
        self.conn.execute(
            "delete from session_pr_overrides where session_id = ?1",
            params![session_id],
        )?;
        Ok(())
    }

    /// Record that a session's pull-request autodetection is suppressed (the
    /// user detached). Idempotent: suppressing an already-suppressed session
    /// changes nothing.
    pub fn set_pr_suppressed(&self, session_id: &str) -> Result<()> {
        self.conn.execute(
            "insert into session_pr_suppressions (session_id) values (?1) \
             on conflict(session_id) do nothing",
            params![session_id],
        )?;
        Ok(())
    }

    /// Clear a session's suppression so autodetection runs again (a no-op when
    /// the session was not suppressed).
    pub fn delete_pr_suppression(&self, session_id: &str) -> Result<()> {
        self.conn.execute(
            "delete from session_pr_suppressions where session_id = ?1",
            params![session_id],
        )?;
        Ok(())
    }

    /// Every session id whose pull-request autodetection is suppressed. Loaded
    /// once at boot into the engine's in-memory mirror.
    pub fn load_pr_suppressions(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("select session_id from session_pr_suppressions")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Persist a brand-new agent: its session row, its first tab's
    /// `agent_tabs` row, and the pointer between them, in ONE transaction.
    ///
    /// Every tab is a row, so a session and its first tab are created together
    /// or not at all: a session row with no slot tab names a PTY address nothing
    /// resolves, and a tab row with no session is an orphan the next load sweeps
    /// away. The slot row sorts at 0, below the 1-based `sort_order` every extra
    /// tab is appended at, so the first tab leads the strip.
    ///
    /// Existing sessions keep going through [`Self::upsert_session`], which is
    /// the hot path status churn takes and which never touches `agent_tabs`.
    pub fn create_session(&self, session: &AgentSession) -> Result<()> {
        let tx = self
            .conn
            .unchecked_transaction()
            .context("failed to start creating the agent")?;
        tx.execute(
            "insert into agent_tabs (id, session_id, provider, sort_order, created_at) \
             values (?1, ?2, ?3, 0, ?4)",
            params![
                session.slot_tab_id,
                session.id,
                session.provider.as_str(),
                session.created_at.to_rfc3339(),
            ],
        )
        .with_context(|| format!("failed to write the first tab of agent {}", session.id))?;
        Self::upsert_session_in(&tx, session)?;
        tx.commit().context("failed to commit the new agent")?;
        Ok(())
    }

    pub fn upsert_session(&self, session: &AgentSession) -> Result<()> {
        Self::upsert_session_in(&self.conn, session)
    }

    /// The body of [`Self::upsert_session`], parameterized over the connection so
    /// [`Self::create_session`] can run it inside its transaction.
    fn upsert_session_in(conn: &Connection, session: &AgentSession) -> Result<()> {
        // Flatten the workspace into the row's columns ONCE, here, so no SQL
        // below reaches into the enum. A folder row writes empty text into the
        // git columns (`project_id` is NOT NULL, and the rest predate the
        // workspace split); `workspace_kind` is what tells the read path not to
        // believe them. See the column's migration comment.
        let managed = session.workspace.as_managed();
        let workspace_kind = session.workspace.kind().as_str();
        let folder_path = session.workspace.folder_path();
        let project_id = managed.map(|m| m.project_id.as_str()).unwrap_or_default();
        let project_path = managed.and_then(|m| m.project_path.as_deref());
        let source_branch = managed
            .map(|m| m.source_branch.as_str())
            .unwrap_or_default();
        let branch_name = managed.map(|m| m.branch_name.as_str()).unwrap_or_default();
        let initial_branch = managed
            .map(|m| m.initial_branch.as_str())
            .unwrap_or_default();
        let worktree_path = managed
            .map(|m| m.worktree_path.as_str())
            .unwrap_or_default();
        let branch_provenance = managed
            .map(|m| m.branch_provenance.as_str())
            // Never read back: the read path decides on `workspace_kind`
            // first, and a folder row has no provenance to parse. Written as
            // the safe word anyway, so a row inspected by hand cannot suggest
            // dux may delete a branch here.
            .unwrap_or("unknown");
        // UPDATE first: existing sessions are re-upserted constantly (status
        // changes, provider starts), and that hot path must not pay the
        // min(sort_order) placement query below. The SET list deliberately
        // omits `sort_order` so re-upserting an existing session never
        // disturbs the user's chosen order.
        //
        // It omits `branch_provenance` for a stronger reason: provenance is
        // decided once, at creation, and an UPDATE that could rewrite it is an
        // UPDATE that could turn a user's pre-existing `develop` into a branch
        // dux believes it owns and force-deletes. INSERT-but-not-SET, following
        // `sort_order`. Do NOT copy `initial_branch`'s treatment below: that
        // one IS in the SET list (its immutability is engine discipline), and
        // adding `branch_provenance` beside it would break this guarantee.
        //
        // `slot_tab_id` joins them, for the same shape of reason. The pointer is
        // identity: it is written once by `create_session` and afterwards only by
        // the migration's repair passes, which are the only code that knows
        // whether a session still needs its first tab MINTED or has a live tab to
        // ADOPT. A hot-path UPDATE cannot know that, and one that writes the
        // pointer back turns the first answer into the second: the read path
        // hands a pre-pointer row the session's own id as a stand-in, and
        // re-upserting it would store that stand-in as a real pointer, so the
        // next open sees a dangling id and adopts tab 2 instead of minting tab 1.
        // INSERT-but-not-SET, following `sort_order` and `branch_provenance`.
        let updated = conn.execute(
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
                initial_branch=?13,
                workspace_kind=?14,
                folder_path=?15
            where id = ?1
            "#,
            params![
                session.id,
                project_path,
                session.provider.as_str(),
                source_branch,
                branch_name,
                worktree_path,
                session.title,
                serialize_started_providers(&session.started_providers),
                session.desired_running,
                session.auto_reopen_enabled,
                session.status.as_str(),
                session.updated_at.to_rfc3339(),
                initial_branch,
                workspace_kind,
                folder_path,
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
        // A standalone agent belongs to no project, so "the top of its
        // project's order" is not a question with an answer for it. Its row
        // stores an empty project id, and taking the minimum over that group
        // would only put it above the other standalone agents, landing it in the
        // middle of a flat list ordered globally. The minimum over EVERY row is
        // the honest reading of "the top" for an agent whose group is the whole
        // list.
        let new_sort_order = if workspace_kind == "folder" {
            min_session_sort_order_overall_in(conn)?.unwrap_or(1) - 1
        } else {
            min_session_sort_order_in(conn, project_id)?.unwrap_or(1) - 1
        };
        conn.execute(
            r#"
            insert into agent_sessions
                (id, project_id, project_path, provider, source_branch, branch_name, worktree_path, title, started_providers, desired_running, auto_reopen_enabled, status, sort_order, created_at, updated_at, initial_branch, branch_provenance, workspace_kind, folder_path, slot_tab_id)
            values
                (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20)
            "#,
            params![
                session.id,
                project_id,
                project_path,
                session.provider.as_str(),
                source_branch,
                branch_name,
                worktree_path,
                session.title,
                serialize_started_providers(&session.started_providers),
                session.desired_running,
                session.auto_reopen_enabled,
                session.status.as_str(),
                new_sort_order,
                session.created_at.to_rfc3339(),
                session.updated_at.to_rfc3339(),
                initial_branch,
                branch_provenance,
                workspace_kind,
                folder_path,
                session.slot_tab_id,
            ],
        )?;
        Ok(())
    }

    /// The smallest `sort_order` assigned to ANY session, or `None` when there
    /// are none. The placement rule for a standalone agent, which has no project
    /// whose top it could be placed at.
    pub fn min_session_sort_order_overall(&self) -> Result<Option<i64>> {
        min_session_sort_order_overall_in(&self.conn)
    }

    /// The smallest `sort_order` currently assigned to any session in
    /// `project_id`, or `None` when the project has no sessions yet. Used to
    /// place a new session one position above the current top.
    pub fn min_session_sort_order(&self, project_id: &str) -> Result<Option<i64>> {
        min_session_sort_order_in(&self.conn, project_id)
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
            select id, project_id, provider, source_branch, branch_name, worktree_path, title, project_path, started_providers, desired_running, auto_reopen_enabled, status, created_at, updated_at, initial_branch, last_focused_tab, branch_provenance, workspace_kind, folder_path, slot_tab_id
            from agent_sessions
            order by sort_order asc, updated_at desc
            "#,
        )?;
        let rows = stmt.query_map([], |row| {
            let started_providers: String = row.get(8)?;
            let created_at: String = row.get(12)?;
            let updated_at: String = row.get(13)?;
            // THE KIND COLUMN IS READ FIRST, before any git column is
            // believed. A folder row stores empty text in all of them, and
            // reading them in the other order would turn those empties into
            // facts about a branch, a project and a worktree that do not exist.
            let kind = crate::model::AgentWorkspaceKind::from_str(
                row.get::<_, String>(17).unwrap_or_default().as_str(),
            );
            let workspace = match kind {
                crate::model::AgentWorkspaceKind::Managed => {
                    // A managed row's working copy is where its provider is
                    // spawned, where git runs and what deletion may remove, so
                    // an empty one is not an agent dux can load either. The
                    // strictness mirrors the folder arm below, and the
                    // population is the same one: `from_str` reads an unknown
                    // workspace kind as MANAGED, so a row written by a newer dux
                    // (or edited by hand) whose real shape has no worktree lands
                    // here with every git column empty. Admitted, it would
                    // enrol in branch sync with an empty branch and render as a
                    // nameless row. Skipped, loudly, instead.
                    let worktree_path: String = row.get(5)?;
                    if worktree_path.trim().is_empty() {
                        let id: String = row.get(0)?;
                        crate::logger::error(&format!(
                            "skipping session {id}: it is recorded as running in a working copy \
                             dux manages but the row names no worktree, so there is no directory \
                             to run it in"
                        ));
                        return Ok(None);
                    }
                    crate::model::AgentWorkspace::Managed(crate::model::ManagedWorkspace {
                        project_id: row.get::<_, String>(1).unwrap_or_default(),
                        project_path: row.get(7)?,
                        source_branch: row.get(3)?,
                        branch_name: row.get(4)?,
                        initial_branch: row.get(14)?,
                        branch_provenance: crate::model::BranchProvenance::from_str(
                            row.get::<_, String>(16)?.as_str(),
                        ),
                        worktree_path,
                    })
                }
                crate::model::AgentWorkspaceKind::Folder => {
                    // A folder row's whole identity is its path, so a NULL or
                    // empty one is not an agent dux can load. It is UNREACHABLE
                    // from anything dux writes; the population it exists for is
                    // the same one the kind column itself exists for, a row from
                    // a newer dux or a hand-edited database. The row is skipped,
                    // loudly, rather than admitted as an agent whose directory
                    // is "": that empty string is what a PTY would be spawned
                    // in, and what `Path::new("").exists()` would be asked
                    // about.
                    let folder_path = row
                        .get::<_, Option<String>>(18)?
                        .filter(|path| !path.trim().is_empty());
                    let Some(folder_path) = folder_path else {
                        let id: String = row.get(0)?;
                        crate::logger::error(&format!(
                            "skipping session {id}: it is recorded as running in a folder \
                             but the row names no folder, so there is no directory to run it in"
                        ));
                        return Ok(None);
                    };
                    crate::model::AgentWorkspace::Folder(crate::model::FolderWorkspace {
                        folder_path,
                    })
                }
            };
            Ok(Some(AgentSession {
                id: row.get(0)?,
                // `migrate()` has already run by the time anything reads, so a
                // usable pointer is guaranteed. The fallback is defence for a
                // row written by a build that is not this one; it restores the
                // pre-pivot identity rather than inventing an unaddressable id.
                slot_tab_id: row
                    .get::<_, Option<String>>(19)?
                    .filter(|id| !id.trim().is_empty())
                    .unwrap_or_else(|| row.get::<_, String>(0).unwrap_or_default()),
                provider: crate::model::ProviderKind::from_str(row.get::<_, String>(2)?.as_str()),
                workspace,
                title: row.get(6)?,
                started_providers: parse_started_providers(&started_providers),
                desired_running: row.get(9)?,
                auto_reopen_enabled: row.get(10)?,
                status: SessionStatus::from_str(row.get::<_, String>(11)?.as_str()),
                created_at: parse_time(&created_at).unwrap_or_else(Utc::now),
                updated_at: parse_time(&updated_at).unwrap_or_else(Utc::now),
                last_focused_tab: row.get(15)?,
            }))
        })?;

        let mut sessions = Vec::new();
        for row in rows {
            // `None` is a row this loader deliberately refused; the reason was
            // logged where it was decided.
            if let Some(session) = row? {
                sessions.push(session);
            }
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
        tx.execute(
            "delete from session_pr_overrides where session_id = ?1",
            params![id],
        )?;
        tx.execute(
            "delete from session_pr_suppressions where session_id = ?1",
            params![id],
        )?;
        // Drop the per-session changed-files revision counter too, so a deleted
        // session leaves no housekeeping rows behind.
        tx.execute("delete from changes_rev where session_id = ?1", params![id])?;
        // Drop every tab the session owns, its slot tab included.
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
        slot_tab_id: format!("{id}-slot"),
        provider: crate::model::ProviderKind::new("claude"),
        title: None,
        started_providers: Vec::new(),
        desired_running: false,
        auto_reopen_enabled: true,
        status: SessionStatus::Active,
        created_at,
        updated_at,
        last_focused_tab: None,
        workspace: crate::model::AgentWorkspace::Managed(crate::model::ManagedWorkspace {
            project_id: "proj".to_string(),
            project_path: None,
            source_branch: "main".to_string(),
            branch_name: format!("branch-{id}"),
            initial_branch: format!("branch-{id}"),
            branch_provenance: crate::model::BranchProvenance::CreatedByDux,
            worktree_path: format!("/tmp/{id}"),
        }),
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
    let mut session = test_session(id, created_at, updated_at);
    session
        .workspace
        .as_managed_mut()
        .expect("test_session builds a managed agent")
        .project_id = project_id.to_string();
    session
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
    use crate::model::{AgentWorkspace, FolderWorkspace};
    use chrono::Duration;

    fn standalone_session(id: &str, folder: &str) -> AgentSession {
        let now = Utc::now();
        AgentSession {
            id: id.to_string(),
            slot_tab_id: format!("{id}-slot"),
            provider: crate::model::ProviderKind::new("claude"),
            workspace: AgentWorkspace::Folder(FolderWorkspace {
                folder_path: folder.to_string(),
            }),
            title: Some(format!("{id} title")),
            started_providers: Vec::new(),
            desired_running: true,
            auto_reopen_enabled: true,
            status: SessionStatus::Active,
            created_at: now,
            updated_at: now,
            last_focused_tab: None,
        }
    }

    fn sample_project_row(id: &str, path: &str) -> ProjectConfig {
        ProjectConfig {
            id: id.to_string(),
            path: path.to_string(),
            name: Some(id.to_string()),
            default_provider: None,
            leading_branch: None,
            auto_reopen_agents: None,
            startup_command: None,
            env: Default::default(),
        }
    }

    fn temp_store() -> (tempfile::TempDir, SessionStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = SessionStore::open(&dir.path().join("sessions.sqlite3")).unwrap();
        (dir, store)
    }

    /// The row's git columns hold empty text for a standalone agent (the
    /// schema's `project_id` is NOT NULL and the rest predate this feature), so
    /// the load path must decide the SHAPE off the kind column BEFORE any git
    /// field is believed. If it did not, the empties would come back as facts:
    /// a branch named "", a project id that matches nothing, and a worktree
    /// path of "" that some delete path would try to remove.
    #[test]
    fn a_standalone_row_round_trips_as_a_folder_and_never_as_empty_git_fields() {
        let (_dir, store) = temp_store();
        store
            .upsert_session(&standalone_session("sa1", "/home/someone/notes"))
            .unwrap();

        let loaded = store.load_sessions().unwrap();
        let session = loaded.iter().find(|s| s.id == "sa1").expect("row");
        assert_eq!(session.folder_path(), Some("/home/someone/notes"));
        assert_eq!(session.branch_name(), None);
        assert_eq!(session.project_id(), None);
        assert_eq!(session.managed_worktree(), None);
        assert_eq!(session.branch_provenance(), None);
        assert_eq!(session.directory(), "/home/someone/notes");
    }

    /// The git COLUMNS of a standalone row are empty on disk, read straight from
    /// the database rather than through the accessors.
    ///
    /// The accessors answer `None` for a folder workspace unconditionally, so
    /// asserting through them would pass even if the writer had put a real
    /// branch name in the row. This reads the raw values, because "the row holds
    /// no branch identity" is the claim.
    #[test]
    fn a_standalone_rows_git_columns_are_empty_on_disk() {
        let (dir, store) = temp_store();
        store
            .upsert_session(&standalone_session("sa1", "/home/someone/notes"))
            .unwrap();
        drop(store);

        let conn = Connection::open(dir.path().join("sessions.sqlite3")).unwrap();
        let (kind, project_id, branch, initial, source, worktree, folder): (
            String,
            String,
            String,
            Option<String>,
            String,
            String,
            Option<String>,
        ) = conn
            .query_row(
                "select workspace_kind, project_id, branch_name, initial_branch, \
                 source_branch, worktree_path, folder_path from agent_sessions where id = 'sa1'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(kind, "folder");
        assert_eq!(folder.as_deref(), Some("/home/someone/notes"));
        assert_eq!(project_id, "");
        assert_eq!(branch, "");
        assert_eq!(initial.unwrap_or_default(), "");
        assert_eq!(source, "");
        assert_eq!(worktree, "");
    }

    /// A folder row that names no folder is not an agent dux can load, so it is
    /// skipped rather than admitted with a directory of "" that a PTY would be
    /// spawned in. Only reachable from a row a newer dux or a person wrote.
    #[test]
    fn a_folder_row_with_no_folder_is_refused_rather_than_loaded_empty() {
        let (_dir, store) = temp_store();
        store
            .upsert_session(&standalone_session("sa1", "/home/someone/notes"))
            .unwrap();
        store
            .conn
            .execute(
                "update agent_sessions set folder_path = null where id = 'sa1'",
                [],
            )
            .unwrap();

        let loaded = store.load_sessions().unwrap();
        assert!(
            loaded.iter().all(|s| s.id != "sa1"),
            "a row with no directory to run in must not load as an agent"
        );

        // An empty string is the same fact spelled differently.
        store
            .conn
            .execute(
                "update agent_sessions set folder_path = '' where id = 'sa1'",
                [],
            )
            .unwrap();
        assert!(store.load_sessions().unwrap().iter().all(|s| s.id != "sa1"));
    }

    /// The same strictness from the other side: a MANAGED row that names no
    /// worktree is skipped too.
    ///
    /// It is reachable the same way the folder arm's refusal is, and by one more
    /// door: an unknown `workspace_kind` reads as managed on purpose, so a row
    /// whose real shape this build has never heard of arrives here with every
    /// git column empty. Loaded, it would enrol in branch sync with an empty
    /// branch and render as a nameless row pointing at "".
    #[test]
    fn a_managed_row_with_no_worktree_is_refused_rather_than_loaded_empty() {
        let (_dir, store) = temp_store();
        let now = Utc::now();
        store
            .upsert_session(&test_session_in("m1", "p1", now, now))
            .unwrap();
        store
            .conn
            .execute(
                "update agent_sessions set worktree_path = '' where id = 'm1'",
                [],
            )
            .unwrap();
        assert!(
            store.load_sessions().unwrap().iter().all(|s| s.id != "m1"),
            "a managed row with no worktree must not load as an agent"
        );

        // And through the other door: a kind this build cannot classify reads as
        // managed, and such a row has no worktree either.
        store
            .conn
            .execute(
                "update agent_sessions set workspace_kind = 'something-newer' where id = 'm1'",
                [],
            )
            .unwrap();
        assert!(store.load_sessions().unwrap().iter().all(|s| s.id != "m1"));
    }

    /// A new standalone agent lands at the TOP of the list.
    ///
    /// Its row stores an empty project id, so taking the minimum over "its
    /// project" would only place it above the other standalone agents, which in
    /// a flat, globally ordered list means somewhere in the middle.
    #[test]
    fn a_new_standalone_agent_lands_above_every_other_agent() {
        let (_dir, store) = temp_store();
        let now = Utc::now();
        store
            .upsert_session(&test_session_in("a", "p1", now, now))
            .unwrap();
        store
            .upsert_session(&test_session_in("b", "p2", now, now))
            .unwrap();
        let top_before = store.min_session_sort_order_overall().unwrap().unwrap();

        store
            .upsert_session(&standalone_session("sa1", "/home/someone/notes"))
            .unwrap();
        let placed: i64 = store
            .conn
            .query_row(
                "select sort_order from agent_sessions where id = 'sa1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            placed < top_before,
            "a new standalone agent goes above every other agent, got {placed} against {top_before}"
        );
    }

    #[test]
    fn an_update_of_a_standalone_row_keeps_it_a_folder() {
        let (_dir, store) = temp_store();
        let mut session = standalone_session("sa1", "/home/someone/notes");
        store.upsert_session(&session).unwrap();
        session.title = Some("renamed".to_string());
        store.upsert_session(&session).unwrap();

        let loaded = store.load_sessions().unwrap();
        let session = loaded.iter().find(|s| s.id == "sa1").expect("row");
        assert_eq!(session.title.as_deref(), Some("renamed"));
        assert_eq!(session.folder_path(), Some("/home/someone/notes"));
        assert_eq!(session.branch_name(), None);
    }

    /// The self-healing backfill freezes an empty `initial_branch` to
    /// `branch_name`. What this pins is the OUTCOME for a standalone row: it
    /// comes back through `migrate()` with no branch identity invented for it.
    ///
    /// It deliberately does not claim to prove the kind gate is load-bearing,
    /// because it is not: a folder row has `branch_name` empty too, so the
    /// assignment would be '' to '' with or without the gate. The gate is there
    /// so a future change to either side (a default branch name, a non-empty
    /// placeholder) cannot start writing a branch onto an agent that has none,
    /// and this test is what would notice if one did.
    #[test]
    fn the_initial_branch_healing_never_touches_a_standalone_row() {
        let (_dir, store) = temp_store();
        store
            .upsert_session(&standalone_session("sa1", "/home/someone/notes"))
            .unwrap();
        store.migrate().unwrap();

        let loaded = store.load_sessions().unwrap();
        let session = loaded.iter().find(|s| s.id == "sa1").expect("row");
        assert_eq!(session.branch_name(), None);
        assert_eq!(session.initial_branch(), None);
    }

    /// The one-time title freeze writes `title = branch_name` for NULL titles,
    /// and it must skip a folder row: freezing an empty branch name into a title
    /// would leave the row with no label at all.
    ///
    /// Built on a store where the freeze REALLY RUNS. The arm is gated on
    /// `initial_branch` being added by this very migration, which `temp_store`
    /// (already migrated) never triggers, so a test written against that fixture
    /// would pass with the gate deleted. Here the table is created with the kind
    /// and folder columns but WITHOUT `initial_branch`, so migrating adds it and
    /// the one-time freeze fires with a standalone row present.
    #[test]
    fn the_title_freeze_never_gives_a_standalone_row_an_empty_name() {
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
                updated_at text not null,
                workspace_kind text not null default 'managed',
                folder_path text
            );
            "#,
        )
        .unwrap();
        // A folder row and a managed row side by side, both with a NULL title.
        conn.execute(
            "insert into agent_sessions (id, project_id, provider, source_branch, \
             branch_name, worktree_path, title, project_path, status, created_at, \
             updated_at, workspace_kind, folder_path) values \
             ('sa1', '', 'claude', '', '', '', null, null, 'detached', '2026-01-01T00:00:00Z', \
             '2026-01-01T00:00:00Z', 'folder', '/home/someone/notes')",
            [],
        )
        .unwrap();
        conn.execute(
            "insert into agent_sessions (id, project_id, provider, source_branch, \
             branch_name, worktree_path, title, project_path, status, created_at, \
             updated_at, workspace_kind, folder_path) values \
             ('s1', 'p1', 'claude', 'main', 'feat', '/tmp/wt', null, null, 'detached', \
             '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 'managed', null)",
            [],
        )
        .unwrap();
        let store = SessionStore { conn };
        store.migrate().unwrap();

        // Proof the freeze really ran in this fixture: the managed row's NULL
        // title was frozen to its branch name. Without this the assertions below
        // would hold for a migration that did nothing at all.
        let loaded = store.load_sessions().unwrap();
        let managed = loaded.iter().find(|s| s.id == "s1").expect("managed row");
        assert_eq!(managed.title.as_deref(), Some("feat"));

        let session = loaded.iter().find(|s| s.id == "sa1").expect("row");
        assert_eq!(
            session.title, None,
            "a frozen empty branch name would leave the row with no label at all"
        );
        assert!(!session.display_label().is_empty());
    }

    /// A standalone row stores empty text under `project_id` (the column is NOT
    /// NULL). `remove_project_records` scopes by a project-id subquery, so an
    /// empty id can never match a real project's cascade. Pinned anyway,
    /// because this is the difference between removing one project and
    /// mass-deleting every standalone agent the user has.
    #[test]
    fn removing_a_project_never_cascades_into_standalone_agents() {
        let (_dir, store) = temp_store();
        store
            .upsert_project(&sample_project_row("p1", "/tmp/p1"))
            .unwrap();
        store
            .upsert_session(&standalone_session("sa1", "/home/someone/notes"))
            .unwrap();

        store.remove_project_records("p1").unwrap();

        let loaded = store.load_sessions().unwrap();
        assert!(
            loaded.iter().any(|s| s.id == "sa1"),
            "a standalone agent belongs to no project and must survive every project removal"
        );
    }

    /// And the pathological spelling of the same thing: a project whose id is
    /// literally the empty string must not sweep up the standalone rows whose
    /// stored project id is also empty.
    #[test]
    fn even_a_project_with_an_empty_id_cannot_cascade_into_standalone_agents() {
        let (_dir, store) = temp_store();
        store
            .upsert_session(&standalone_session("sa1", "/home/someone/notes"))
            .unwrap();

        store.remove_project_records("").unwrap();

        let loaded = store.load_sessions().unwrap();
        assert!(
            loaded.iter().any(|s| s.id == "sa1"),
            "the kind column, not the project id, is what says who owns a row"
        );
    }

    /// The database mirrors the same per-project `env` map that made
    /// `config.toml` `0600`, and SQLite's `-wal`/`-shm` sidecars carry the same
    /// content.
    ///
    /// On a FIRST open the sidecars do not yet exist when the tightening loop
    /// runs, so the loop cannot be what makes them owner-only: SQLite creates
    /// them afterwards and they INHERIT the database file's mode. The loop
    /// entries are load-bearing in the reopen test below, not here. Every
    /// metadata read is unwrapped: a sidecar that is not there must fail the
    /// test, not pass silently.
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
    fn migrate_sweeps_orphan_tabs_with_no_session() {
        let store = test_store();
        let now = Utc::now();
        let mut session = test_session("s1", now, now);
        session.slot_tab_id = "slot-1".to_string();
        store.create_session(&session).unwrap();
        store.insert_agent_tab(&test_tab("t1", "s1", 1)).unwrap();
        // A row whose session was removed by an older binary that didn't cascade.
        store
            .insert_agent_tab(&test_tab("orphan", "gone", 1))
            .unwrap();

        store.migrate().unwrap();

        let loaded: Vec<String> = store
            .load_agent_tabs()
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(loaded, vec!["slot-1".to_string(), "t1".to_string()]);
    }

    #[test]
    fn load_agent_tabs_is_a_plain_reader_and_sweeps_nothing() {
        let store = test_store();
        store
            .insert_agent_tab(&test_tab("orphan", "gone", 1))
            .unwrap();

        let loaded = store.load_agent_tabs().unwrap();

        assert_eq!(
            loaded.len(),
            1,
            "the sweep belongs to migrate(), so a read must not delete rows"
        );
    }

    /// A session row shaped the way this branch's dev databases are: a real
    /// session with no `slot_tab_id` pointer and no first-tab row, because the
    /// slot tab was synthesized from the session record rather than stored.
    fn pre_pivot_session(store: &SessionStore, id: &str, provider: &str) {
        let now = Utc::now();
        let mut session = test_session(id, now, now);
        session.provider = crate::model::ProviderKind::new(provider);
        store.upsert_session(&session).unwrap();
        store
            .conn
            .execute(
                "update agent_sessions set slot_tab_id = null where id = ?1",
                params![id],
            )
            .unwrap();
    }

    fn slot_pointer(store: &SessionStore, id: &str) -> Option<String> {
        store
            .conn
            .query_row(
                "select slot_tab_id from agent_sessions where id = ?1",
                params![id],
                |row| row.get::<_, Option<String>>(0),
            )
            .unwrap()
    }

    #[test]
    fn create_session_writes_the_session_its_slot_tab_row_and_the_pointer_at_once() {
        let store = test_store();
        let now = Utc::now();
        let mut session = test_session("s1", now, now);
        session.slot_tab_id = "slot-1".to_string();
        session.provider = crate::model::ProviderKind::new("codex");
        store.create_session(&session).unwrap();

        let loaded = store.load_sessions().unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].slot_tab_id, "slot-1");
        let tabs = store.load_agent_tabs().unwrap();
        assert_eq!(tabs.len(), 1, "the first tab is a row like any other");
        assert_eq!(tabs[0].id, "slot-1");
        assert_eq!(tabs[0].session_id, "s1");
        assert_eq!(tabs[0].provider.as_str(), "codex");
    }

    #[test]
    fn create_session_writes_nothing_at_all_when_the_slot_tab_row_cannot_be_written() {
        let store = test_store();
        let now = Utc::now();
        // Somebody else already holds this tab id, so the slot row's INSERT
        // fails. The session row must not survive on its own: a session with no
        // slot tab is exactly the half-created state the transaction exists to
        // prevent.
        store
            .upsert_session(&test_session("other", now, now))
            .unwrap();
        store
            .insert_agent_tab(&test_tab("slot-1", "other", 1))
            .unwrap();

        let mut session = test_session("s1", now, now);
        session.slot_tab_id = "slot-1".to_string();
        assert!(store.create_session(&session).is_err());
        assert!(
            store.load_sessions().unwrap().iter().all(|s| s.id != "s1"),
            "the session row must roll back with the slot tab row"
        );
    }

    #[test]
    fn create_session_places_the_slot_row_before_every_tab_added_later() {
        let store = test_store();
        let now = Utc::now();
        let mut session = test_session("s1", now, now);
        session.slot_tab_id = "slot-1".to_string();
        store.create_session(&session).unwrap();
        let next = store.max_tab_sort_order("s1").unwrap().unwrap_or(0) + 1;
        store.insert_agent_tab(&test_tab("t2", "s1", next)).unwrap();

        let ids: Vec<String> = store
            .load_agent_tabs()
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, vec!["slot-1".to_string(), "t2".to_string()]);
    }

    #[test]
    fn migration_mints_a_slot_tab_row_for_a_session_that_predates_the_pointer() {
        let store = test_store();
        pre_pivot_session(&store, "s1", "codex");
        assert_eq!(slot_pointer(&store, "s1"), None);

        store.migrate().unwrap();

        let pointer = slot_pointer(&store, "s1").expect("the migration sets the pointer");
        assert_ne!(
            pointer, "s1",
            "the slot tab gets a generated id, not the session id"
        );
        let tabs = store.load_agent_tabs().unwrap();
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].id, pointer);
        assert_eq!(
            tabs[0].provider.as_str(),
            "codex",
            "the minted row carries the provider the session was running"
        );
    }

    #[test]
    fn migration_keeps_every_existing_extra_tab_after_the_minted_first_row() {
        let store = test_store();
        pre_pivot_session(&store, "s1", "claude");
        store.insert_agent_tab(&test_tab("t2", "s1", 1)).unwrap();
        store.insert_agent_tab(&test_tab("t3", "s1", 2)).unwrap();

        store.migrate().unwrap();

        let pointer = slot_pointer(&store, "s1").unwrap();
        let ids: Vec<String> = store
            .load_agent_tabs()
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(ids, vec![pointer, "t2".to_string(), "t3".to_string()]);
    }

    #[test]
    fn migration_is_a_no_op_on_a_second_run() {
        let store = test_store();
        pre_pivot_session(&store, "s1", "claude");
        store.migrate().unwrap();
        let pointer = slot_pointer(&store, "s1").unwrap();
        let before = store.load_agent_tabs().unwrap();

        store.migrate().unwrap();

        assert_eq!(slot_pointer(&store, "s1").unwrap(), pointer);
        let after = store.load_agent_tabs().unwrap();
        assert_eq!(after.len(), before.len());
        assert_eq!(after[0].id, before[0].id);
    }

    #[test]
    fn a_failed_slot_tab_migration_aborts_the_open_rather_than_half_migrating() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.sqlite3");
        {
            let store = SessionStore::open(&path).unwrap();
            pre_pivot_session(&store, "s1", "claude");
            // A corrupted `agent_tabs` the minting INSERT cannot satisfy. The
            // table already exists, so `create table if not exists` leaves it
            // alone and the migration's INSERT is the thing that fails.
            store
                .conn
                .execute_batch(
                    "drop table agent_tabs; \
                     create table agent_tabs ( \
                        id text primary key, \
                        session_id text not null, \
                        provider text not null, \
                        sort_order integer not null default 0, \
                        created_at text not null, \
                        unsatisfiable text not null \
                     );",
                )
                .unwrap();
        }

        let err = match SessionStore::open(&path) {
            Ok(_) => panic!("a failed migration must stop startup"),
            Err(err) => err,
        };
        assert!(
            format!("{err:#}").contains("slot tab"),
            "the failure must name what could not be migrated, got: {err:#}"
        );
        let conn = Connection::open(&path).unwrap();
        let pointer: Option<String> = conn
            .query_row(
                "select slot_tab_id from agent_sessions where id = 's1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(pointer, None, "a failed migration leaves nothing behind");
    }

    #[test]
    fn migrate_heals_a_pointer_that_names_no_row_by_adopting_the_oldest_tab() {
        let store = test_store();
        let now = Utc::now();
        let mut session = test_session("s1", now, now);
        session.slot_tab_id = "slot-1".to_string();
        session.provider = crate::model::ProviderKind::new("claude");
        store.create_session(&session).unwrap();
        // `test_tab` builds codex tabs, so the adopted tab's provider differs
        // from the vanished slot tab's.
        store.insert_agent_tab(&test_tab("t2", "s1", 1)).unwrap();
        store.insert_agent_tab(&test_tab("t3", "s1", 2)).unwrap();
        // The slot row vanished without the pointer moving with it.
        store.delete_agent_tab("slot-1").unwrap();

        store.migrate().unwrap();

        assert_eq!(
            slot_pointer(&store, "s1").as_deref(),
            Some("t2"),
            "the oldest surviving tab takes the slot"
        );
        assert_eq!(
            store.load_sessions().unwrap()[0].provider.as_str(),
            "codex",
            "the session's provider mirror must follow the slot it now points at"
        );
    }

    #[test]
    fn migrate_treats_a_blank_pointer_the_same_as_a_missing_one_and_mints() {
        let store = test_store();
        let now = Utc::now();
        let mut session = test_session("s1", now, now);
        session.slot_tab_id = "slot-1".to_string();
        store.create_session(&session).unwrap();
        store.insert_agent_tab(&test_tab("t2", "s1", 1)).unwrap();
        // A hand-edited row, or one written by a build that spelled "no pointer
        // yet" as the empty string.
        store
            .conn
            .execute(
                "update agent_sessions set slot_tab_id = '' where id = 's1'",
                [],
            )
            .unwrap();

        store.migrate().unwrap();

        let pointer = slot_pointer(&store, "s1").expect("healed");
        assert!(
            pointer != "t2" && pointer != "slot-1",
            "a blank pointer means unmigrated, so a fresh first tab is minted \
             rather than tab 2 being adopted, got {pointer}"
        );
        assert_eq!(store.count_agent_tabs("s1").unwrap(), 3);
    }

    #[test]
    fn migrate_treats_a_pointer_at_another_sessions_tab_as_dangling() {
        let store = test_store();
        let now = Utc::now();
        let mut first = test_session("s1", now, now);
        first.slot_tab_id = "slot-1".to_string();
        store.create_session(&first).unwrap();
        let mut second = test_session("s2", now, now);
        second.slot_tab_id = "slot-2".to_string();
        store.create_session(&second).unwrap();
        // s1's slot now names a tab that lives in s2's strip.
        store
            .conn
            .execute(
                "update agent_sessions set slot_tab_id = 'slot-2' where id = 's1'",
                [],
            )
            .unwrap();

        store.migrate().unwrap();

        assert_eq!(
            slot_pointer(&store, "s1").as_deref(),
            Some("slot-1"),
            "a pointer into another agent's tabs is dangling, so s1 adopts its \
             own oldest tab"
        );
        assert_eq!(
            slot_pointer(&store, "s2").as_deref(),
            Some("slot-2"),
            "the other agent is untouched"
        );
    }

    #[test]
    fn upserting_a_pre_pivot_session_still_leaves_its_first_tab_to_be_minted() {
        let store = test_store();
        pre_pivot_session(&store, "s1", "claude");
        store.insert_agent_tab(&test_tab("t2", "s1", 1)).unwrap();
        store.insert_agent_tab(&test_tab("t3", "s1", 2)).unwrap();

        // The commonest write there is: load the agent, then re-upsert it on a
        // status change. The loaded record carries the read path's stand-in
        // pointer; storing it would turn "not migrated" into "dangling".
        let loaded = store.load_sessions().unwrap();
        assert_eq!(loaded.len(), 1);
        store.upsert_session(&loaded[0]).unwrap();

        store.migrate().unwrap();

        assert_eq!(
            store.count_agent_tabs("s1").unwrap(),
            3,
            "the agent's first tab must be minted, leaving its three tabs intact"
        );
        let pointer = slot_pointer(&store, "s1").expect("migrated");
        assert!(
            pointer != "t2" && pointer != "t3" && pointer != "s1",
            "the minted tab takes the slot, not an existing tab, got {pointer}"
        );
    }

    #[test]
    fn migrate_heals_a_dangling_pointer_with_no_tabs_left_by_minting_one() {
        let store = test_store();
        let now = Utc::now();
        let mut session = test_session("s1", now, now);
        session.slot_tab_id = "slot-1".to_string();
        session.provider = crate::model::ProviderKind::new("codex");
        store.create_session(&session).unwrap();
        store.delete_agent_tab("slot-1").unwrap();

        store.migrate().unwrap();

        let pointer = slot_pointer(&store, "s1").expect("healed");
        assert_ne!(pointer, "slot-1");
        let tabs = store.load_agent_tabs().unwrap();
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].id, pointer);
        assert_eq!(tabs[0].provider.as_str(), "codex");
    }

    #[test]
    fn deleting_a_session_removes_its_slot_tab_row_with_the_rest() {
        let store = test_store();
        let now = Utc::now();
        let mut session = test_session("s1", now, now);
        session.slot_tab_id = "slot-1".to_string();
        store.create_session(&session).unwrap();
        store.insert_agent_tab(&test_tab("t2", "s1", 1)).unwrap();

        store.delete_session("s1").unwrap();

        assert_eq!(store.count_agent_tabs("s1").unwrap(), 0);
        assert!(store.load_agent_tabs().unwrap().is_empty());
    }

    #[test]
    fn set_slot_provider_moves_the_row_and_the_session_mirror_together() {
        let store = test_store();
        let now = Utc::now();
        let mut session = test_session("s1", now, now);
        session.slot_tab_id = "slot-1".to_string();
        session.provider = crate::model::ProviderKind::new("claude");
        store.create_session(&session).unwrap();

        store.set_slot_provider("s1", "codex", Utc::now()).unwrap();

        assert_eq!(store.load_sessions().unwrap()[0].provider.as_str(), "codex");
        assert_eq!(
            store.load_agent_tabs().unwrap()[0].provider.as_str(),
            "codex"
        );
    }

    /// The shape every promotion test starts from: an agent whose slot tab is
    /// `slot-1` and which has one extra tab, `t2`.
    fn session_with_one_extra_tab(store: &SessionStore) {
        let now = Utc::now();
        let mut session = test_session("s1", now, now);
        session.slot_tab_id = "slot-1".to_string();
        session.provider = crate::model::ProviderKind::new("claude");
        store.create_session(&session).unwrap();
        let mut extra = test_tab("t2", "s1", 1);
        extra.provider = crate::model::ProviderKind::new("codex");
        store.insert_agent_tab(&extra).unwrap();
    }

    #[test]
    fn promote_tab_to_slot_moves_the_pointer_the_mirror_and_the_old_row_at_once() {
        let store = test_store();
        session_with_one_extra_tab(&store);

        store
            .promote_tab_to_slot("s1", "t2", "slot-1", "codex", Utc::now())
            .unwrap();

        let session = store.load_sessions().unwrap().remove(0);
        assert_eq!(session.slot_tab_id, "t2", "the pointer names the successor");
        assert_eq!(
            session.provider.as_str(),
            "codex",
            "the session's provider mirrors the promoted tab's"
        );
        let tabs = store.load_agent_tabs().unwrap();
        assert_eq!(
            tabs.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            vec!["t2"],
            "the closed slot tab's row is gone and the promoted row survives"
        );
    }

    #[test]
    fn promote_tab_to_slot_restores_the_same_shape_on_reload() {
        // The invariant a restart depends on: after a promotion the store reads
        // back with the promoted tab in the slot and NOT among the extras, and
        // the cap counts each surviving tab exactly once.
        let store = test_store();
        session_with_one_extra_tab(&store);
        store.insert_agent_tab(&test_tab("t3", "s1", 2)).unwrap();

        store
            .promote_tab_to_slot("s1", "t2", "slot-1", "codex", Utc::now())
            .unwrap();

        assert_eq!(store.load_sessions().unwrap()[0].slot_tab_id, "t2");
        let extras: Vec<String> = store
            .load_extra_agent_tabs()
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(
            extras,
            vec!["t3".to_string()],
            "the promoted tab is the slot, so it is not an extra as well"
        );
        assert_eq!(store.count_agent_tabs("s1").unwrap(), 2);
    }

    #[test]
    fn promote_tab_to_slot_forgets_a_focus_memory_the_promotion_invalidated() {
        // The slot tab is represented in the focus memory as absence, so a
        // memory naming the promoted tab must not survive the promotion; one
        // naming the departing tab must not survive its deletion either.
        for remembered in ["t2", "slot-1"] {
            let store = test_store();
            session_with_one_extra_tab(&store);
            store.insert_agent_tab(&test_tab("t3", "s1", 2)).unwrap();
            store.set_last_focused_tab("s1", Some(remembered)).unwrap();

            store
                .promote_tab_to_slot("s1", "t2", "slot-1", "codex", Utc::now())
                .unwrap();

            assert_eq!(
                store.load_sessions().unwrap()[0].last_focused_tab,
                None,
                "a memory of {remembered} cannot outlive this promotion"
            );
        }

        // A memory of an untouched sibling is left exactly as it was.
        let store = test_store();
        session_with_one_extra_tab(&store);
        store.insert_agent_tab(&test_tab("t3", "s1", 2)).unwrap();
        store.set_last_focused_tab("s1", Some("t3")).unwrap();
        store
            .promote_tab_to_slot("s1", "t2", "slot-1", "codex", Utc::now())
            .unwrap();
        assert_eq!(
            store.load_sessions().unwrap()[0].last_focused_tab,
            Some("t3".to_string())
        );
    }

    #[test]
    fn promote_tab_to_slot_refuses_a_tab_belonging_to_another_agent() {
        let store = test_store();
        session_with_one_extra_tab(&store);
        let now = Utc::now();
        let mut other = test_session("s2", now, now);
        other.slot_tab_id = "s2-slot".to_string();
        store.create_session(&other).unwrap();
        store
            .insert_agent_tab(&test_tab("foreign", "s2", 1))
            .unwrap();

        assert!(
            store
                .promote_tab_to_slot("s1", "foreign", "slot-1", "codex", Utc::now())
                .is_err()
        );

        let session = store.load_sessions().unwrap();
        let s1 = session.iter().find(|s| s.id == "s1").unwrap();
        assert_eq!(s1.slot_tab_id, "slot-1", "the pointer must not have moved");
        assert_eq!(s1.provider.as_str(), "claude", "nor the mirror");
        assert_eq!(
            store.count_agent_tabs("s1").unwrap(),
            2,
            "nor may the old slot row have been deleted"
        );
    }

    #[test]
    fn promote_tab_to_slot_refuses_a_tab_that_does_not_exist() {
        let store = test_store();
        session_with_one_extra_tab(&store);

        assert!(
            store
                .promote_tab_to_slot("s1", "ghost", "slot-1", "codex", Utc::now())
                .is_err()
        );
        assert_eq!(store.load_sessions().unwrap()[0].slot_tab_id, "slot-1");
        assert_eq!(store.count_agent_tabs("s1").unwrap(), 2);
    }

    #[test]
    fn promote_tab_to_slot_rolls_the_pointer_back_when_the_old_row_cannot_be_deleted() {
        // Atomicity under storage failure: the pointer move and the old row's
        // deletion are one transaction, so a failure on the second half must not
        // leave the agent pointing at a tab whose predecessor is still a row.
        // A trigger injects the failure deterministically, which is the only way
        // to fail a DELETE that would otherwise always succeed.
        let store = test_store();
        session_with_one_extra_tab(&store);
        store
            .conn
            .execute_batch(
                "create trigger refuse_tab_delete before delete on agent_tabs \
                 begin select raise(abort, 'no deletes today'); end",
            )
            .unwrap();

        assert!(
            store
                .promote_tab_to_slot("s1", "t2", "slot-1", "codex", Utc::now())
                .is_err()
        );

        let session = store.load_sessions().unwrap().remove(0);
        assert_eq!(session.slot_tab_id, "slot-1");
        assert_eq!(session.provider.as_str(), "claude");
        assert_eq!(store.count_agent_tabs("s1").unwrap(), 2);
    }

    #[test]
    fn count_agent_tabs_counts_the_slot_row_too_and_extras_omit_it() {
        let store = test_store();
        let now = Utc::now();
        let mut session = test_session("s1", now, now);
        session.slot_tab_id = "slot-1".to_string();
        store.create_session(&session).unwrap();
        store.insert_agent_tab(&test_tab("t2", "s1", 1)).unwrap();

        assert_eq!(store.count_agent_tabs("s1").unwrap(), 2);
        let extras: Vec<String> = store
            .load_extra_agent_tabs()
            .unwrap()
            .into_iter()
            .map(|t| t.id)
            .collect();
        assert_eq!(extras, vec!["t2".to_string()]);
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
    fn pr_override_round_trips_and_survives_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("sessions.sqlite3");
        let now = Utc::now();
        {
            let store = SessionStore::open(&db).unwrap();
            store.upsert_session(&test_session("s1", now, now)).unwrap();
            store.upsert_pr_override(&stored_pr("s1", 9)).unwrap();
            let loaded = store.load_pr_overrides().unwrap();
            assert_eq!(loaded, vec![stored_pr("s1", 9)]);
            // One row per session: a second attach REPLACES the first rather
            // than accumulating (the primary key is the session id alone).
            store.upsert_pr_override(&stored_pr("s1", 12)).unwrap();
            let loaded = store.load_pr_overrides().unwrap();
            assert_eq!(loaded, vec![stored_pr("s1", 12)]);
        }
        // The cached state/title/url are what make a restart render the pin
        // instantly, so the row must survive a reopen intact.
        let store = SessionStore::open(&db).unwrap();
        assert_eq!(
            store.load_pr_overrides().unwrap(),
            vec![stored_pr("s1", 12)]
        );
        store.delete_pr_override("s1").unwrap();
        assert!(store.load_pr_overrides().unwrap().is_empty());
        // Deleting an absent override is a harmless no-op.
        store.delete_pr_override("s1").unwrap();
    }

    /// A detach must outlive the process: dux restarting is not the user
    /// changing their mind, so the suppression row round-trips a reopen.
    #[test]
    fn pr_suppression_round_trips_and_survives_reopen() {
        let tmp = tempfile::tempdir().unwrap();
        let db = tmp.path().join("sessions.sqlite3");
        let now = Utc::now();
        {
            let store = SessionStore::open(&db).unwrap();
            store.upsert_session(&test_session("s1", now, now)).unwrap();
            store.upsert_session(&test_session("s2", now, now)).unwrap();
            assert!(store.load_pr_suppressions().unwrap().is_empty());
            store.set_pr_suppressed("s1").unwrap();
            // Suppressing twice is the same single row: presence is the whole
            // meaning, so the write is idempotent.
            store.set_pr_suppressed("s1").unwrap();
            assert_eq!(
                store.load_pr_suppressions().unwrap(),
                vec!["s1".to_string()]
            );
        }
        let store = SessionStore::open(&db).unwrap();
        assert_eq!(
            store.load_pr_suppressions().unwrap(),
            vec!["s1".to_string()]
        );
        store.delete_pr_suppression("s1").unwrap();
        assert!(store.load_pr_suppressions().unwrap().is_empty());
        // Clearing an absent suppression is a harmless no-op.
        store.delete_pr_suppression("s1").unwrap();
    }

    #[test]
    fn delete_session_also_removes_its_pr_suppression_row() {
        let store = test_store();
        let now = Utc::now();
        store.upsert_session(&test_session("s1", now, now)).unwrap();
        store.set_pr_suppressed("s1").unwrap();

        store.delete_session("s1").unwrap();

        // The declared FK cascade never fires (PRAGMA foreign_keys is off), so
        // the explicit delete is what keeps a later session reusing the id from
        // inheriting a detach it never asked for.
        assert!(store.load_pr_suppressions().unwrap().is_empty());
    }

    #[test]
    fn delete_session_also_removes_its_pr_override_row() {
        let store = test_store();
        let now = Utc::now();
        store.upsert_session(&test_session("s1", now, now)).unwrap();
        store.upsert_pr_override(&stored_pr("s1", 7)).unwrap();

        store.delete_session("s1").unwrap();

        // The declared FK cascade never fires (PRAGMA foreign_keys is off), so
        // the explicit delete is what keeps the override row from leaking to a
        // later session that reuses the id.
        assert!(store.load_pr_overrides().unwrap().is_empty());
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
        // A pinned PR for one of p1's sessions and one for p2's, so the removal
        // is proven to drop exactly its own project's override rows.
        store.upsert_pr_override(&stored_pr("a", 1)).unwrap();
        store.upsert_pr_override(&stored_pr("c", 3)).unwrap();
        // One suppressed session per project, so the removal is proven to drop
        // exactly its own project's suppression rows.
        store.set_pr_suppressed("b").unwrap();
        store.set_pr_suppressed("c").unwrap();
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
        // p1's override row went with its sessions; p2's survives untouched.
        assert_eq!(store.load_pr_overrides().unwrap(), vec![stored_pr("c", 3)]);
        // Same for the suppression rows: p1's went, p2's stayed.
        assert_eq!(store.load_pr_suppressions().unwrap(), vec!["c".to_string()]);
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
        s.workspace
            .as_managed_mut()
            .expect("managed test session")
            .branch_name = "renamed".into();
        s.workspace
            .as_managed_mut()
            .expect("managed test session")
            .initial_branch = "born-on".into();
        store.upsert_session(&s).unwrap();

        let loaded = store.load_sessions().unwrap();
        let got = loaded.iter().find(|s| s.id == "id1").expect("stored id1");
        assert_eq!(
            got.initial_branch().expect("managed test session"),
            "born-on"
        );
        assert_eq!(got.branch_name().expect("managed test session"), "renamed");
    }

    #[test]
    fn branch_provenance_round_trips_through_storage() {
        let store = test_store();
        let now = Utc::now();
        let cases = [
            ("created", crate::model::BranchProvenance::CreatedByDux),
            ("attached", crate::model::BranchProvenance::AttachedExisting),
            ("adopted", crate::model::BranchProvenance::Adopted),
        ];
        for (id, provenance) in cases {
            let mut s = test_session(id, now, now);
            s.workspace
                .as_managed_mut()
                .expect("managed test session")
                .branch_provenance = provenance;
            store.upsert_session(&s).unwrap();
        }

        let loaded = store.load_sessions().unwrap();
        for (id, provenance) in cases {
            let got = loaded.iter().find(|s| s.id == id).expect("stored row");
            assert_eq!(
                got.branch_provenance().expect("managed test session"),
                provenance
            );
        }
    }

    #[test]
    fn branch_provenance_survives_a_re_upsert_of_an_existing_session() {
        // Provenance is decided once, at creation. An UPDATE that could rewrite
        // it is an UPDATE that could turn the user's pre-existing branch into
        // one dux believes it owns and force-deletes on the next delete, so the
        // column is deliberately absent from `upsert_session`'s SET list.
        let store = test_store();
        let now = Utc::now();
        let mut s = test_session("id1", now, now);
        s.workspace
            .as_managed_mut()
            .expect("managed test session")
            .branch_provenance = crate::model::BranchProvenance::AttachedExisting;
        store.upsert_session(&s).unwrap();

        // A later status-churn upsert claiming the branch is dux's must not stick.
        s.workspace
            .as_managed_mut()
            .expect("managed test session")
            .branch_provenance = crate::model::BranchProvenance::CreatedByDux;
        s.status = SessionStatus::Detached;
        store.upsert_session(&s).unwrap();

        let loaded = store.load_sessions().unwrap();
        let got = loaded.iter().find(|s| s.id == "id1").expect("stored id1");
        assert_eq!(
            got.branch_provenance().expect("managed test session"),
            crate::model::BranchProvenance::AttachedExisting,
            "re-upserting a session must never rewrite its recorded provenance"
        );
        assert_eq!(
            got.status,
            SessionStatus::Detached,
            "the rest still updates"
        );
    }

    #[test]
    fn legacy_rows_arrive_as_created_by_dux() {
        // The migration default preserves exactly today's behavior for agents
        // that predate the column: their branches are still cleaned up.
        let store = legacy_store_with_sessions(&[("feat-x", "p1", "2026-01-01T00:00:00Z")]);
        let loaded = store.load_sessions().unwrap();
        let s = loaded.iter().find(|s| s.id == "feat-x").expect("row");
        assert_eq!(
            s.branch_provenance().expect("managed test session"),
            crate::model::BranchProvenance::CreatedByDux
        );
    }

    #[test]
    fn adding_the_branch_provenance_column_twice_is_a_no_op() {
        let store = test_store();
        // `migrate()` runs on every open(); the second ALTER must be tolerated.
        store.migrate().unwrap();
        assert!(
            !ensure_column(
                &store.conn,
                "agent_sessions",
                "branch_provenance",
                "text not null default 'created'"
            )
            .unwrap(),
            "the column already exists, so ensure_column must report no change"
        );
    }

    #[test]
    fn an_unknown_provenance_value_is_not_treated_as_created() {
        // A future binary may write a variant this one has never heard of.
        // Guessing "dux made it" would force-delete a branch on that guess.
        let store = test_store();
        let now = Utc::now();
        store
            .upsert_session(&test_session("id1", now, now))
            .unwrap();
        store
            .conn
            .execute(
                "update agent_sessions set branch_provenance = 'borrowed-from-the-future'",
                [],
            )
            .unwrap();

        let loaded = store.load_sessions().unwrap();
        assert!(
            !loaded[0].workspace.dux_may_delete_branch(),
            "an unrecognized provenance must keep the branch"
        );
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
        assert_eq!(s.initial_branch().expect("managed test session"), "feat-x");
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
        fresh
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .project_id = "p1".into();
        fresh
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .branch_name = "pet-name".into();
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
            s.initial_branch().expect("managed test session"),
            "feat-x",
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
        // With the initial_branch ALTER moved to autocommit,
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
        // A crash between the (autocommitted) sort_order ALTER and
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
            .map(|s| (s.project_id().expect("managed test session"), s.id.as_str()))
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
            .filter(|s| s.project_id().expect("managed test session") == "p2")
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
/// Read one `agent_tabs` row. Shared by every tab query so the slot tab and an
/// extra tab can never be decoded differently.
fn read_agent_tab(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentTab> {
    let created_at: String = row.get(4)?;
    Ok(AgentTab {
        id: row.get(0)?,
        session_id: row.get(1)?,
        provider: ProviderKind::from_str(row.get::<_, String>(2)?.as_str()),
        sort_order: row.get(3)?,
        created_at: parse_time(&created_at).unwrap_or_else(Utc::now),
    })
}

/// See [`SessionStore::min_session_sort_order_overall`]. Parameterized over the
/// connection so the insert path can run inside `create_session`'s transaction.
fn min_session_sort_order_overall_in(conn: &Connection) -> Result<Option<i64>> {
    conn.query_row("select min(sort_order) from agent_sessions", [], |row| {
        row.get::<_, Option<i64>>(0)
    })
    .context("failed to compute the overall min session sort order")
}

/// See [`SessionStore::min_session_sort_order`]. Parameterized over the
/// connection for the same reason as its sibling above.
fn min_session_sort_order_in(conn: &Connection, project_id: &str) -> Result<Option<i64>> {
    conn.query_row(
        "select min(sort_order) from agent_sessions where project_id = ?1",
        params![project_id],
        |row| row.get::<_, Option<i64>>(0),
    )
    .context("failed to compute min session sort order")
}

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
