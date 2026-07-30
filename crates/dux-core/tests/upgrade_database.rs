//! Upgrade survival for `sessions.sqlite3`: a database an older dux created must
//! open, and every row in it must still be there and still say the same thing.
//!
//! There is deliberately no schema version marker and no migration ledger. The
//! whole strategy is `create table if not exists` plus additive
//! `alter table ... add column` (via the internal `ensure_column`), run on EVERY
//! `SessionStore::open`. That makes these the properties worth pinning:
//!
//! - opening an old database succeeds rather than erroring on a missing column;
//! - tables added since (`session_prs`, `changes_rev`, `agent_tabs`,
//!   `app_state`) are created empty and immediately usable;
//! - columns added since arrive at their documented default, and the one-time
//!   backfills (`title`, `initial_branch`, `sort_order`) run;
//! - none of it is destructive on the second open, because `open` is what every
//!   startup AND every background project-persistence does.
//!
//! What these tests CANNOT cover is a downgrade: running an OLDER dux binary
//! against a database this build has migrated. That needs two binaries and only
//! one exists in a checkout. The relevant mitigating property IS testable and is
//! tested here: every column added is nullable or has a default, so an older
//! binary's `INSERT` (which names none of them) still satisfies the schema.

use chrono::Utc;
use dux_core::model::{AgentSession, AgentTab, ProviderKind, SessionStatus};
use dux_core::storage::{SessionStore, StoredPr};
use rusqlite::Connection;

/// `sessions.sqlite3` as an early dux created it: the two original tables, with
/// only the columns that existed then. Everything the current schema adds is
/// absent, including all four of the tables added later.
const OLD_SCHEMA: &str = r#"
create table agent_sessions (
    id text primary key,
    project_id text not null,
    provider text not null,
    source_branch text not null,
    branch_name text not null,
    worktree_path text not null,
    status text not null,
    created_at text not null,
    updated_at text not null
);
create table projects (
    id text primary key,
    path text not null unique,
    created_at text not null,
    updated_at text not null
);
"#;

/// Build an old-shaped database with two projects and three sessions, using raw
/// SQL so the rows are written exactly as the old binary would have written them
/// (naming only the columns that existed).
fn old_database() -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("sessions.sqlite3");
    let conn = Connection::open(&path).expect("open");
    conn.execute_batch(OLD_SCHEMA).expect("old schema");
    conn.execute_batch(
        r#"
        insert into projects (id, path, created_at, updated_at) values
          ('proj-widget', '/home/ada/code/widget', '2024-01-01T00:00:00Z', '2024-01-02T00:00:00Z'),
          ('proj-gadget', '/home/ada/code/gadget', '2024-01-01T00:00:00Z', '2024-01-03T00:00:00Z');
        insert into agent_sessions
          (id, project_id, provider, source_branch, branch_name, worktree_path, status, created_at, updated_at)
        values
          ('sess-1', 'proj-widget', 'claude', 'main', 'dux/lively-otter',
           '/home/ada/.config/dux/worktrees/lively-otter', 'active',
           '2024-01-01T00:00:00Z', '2024-01-05T00:00:00Z'),
          ('sess-2', 'proj-widget', 'codex', 'main', 'dux/brave-ferret',
           '/home/ada/.config/dux/worktrees/brave-ferret', 'detached',
           '2024-01-02T00:00:00Z', '2024-01-04T00:00:00Z'),
          ('sess-3', 'proj-gadget', 'opencode', 'develop', 'dux/quiet-moose',
           '/home/ada/.config/dux/worktrees/quiet-moose', 'active',
           '2024-01-03T00:00:00Z', '2024-01-06T00:00:00Z');
        "#,
    )
    .expect("seed rows");
    drop(conn);
    (tmp, path)
}

fn session<'a>(sessions: &'a [AgentSession], id: &str) -> &'a AgentSession {
    sessions
        .iter()
        .find(|s| s.id == id)
        .unwrap_or_else(|| panic!("{id} vanished from the database"))
}

#[test]
fn an_old_database_opens_and_every_session_row_survives_intact() {
    let (_tmp, path) = old_database();
    let store = SessionStore::open(&path).expect("an old database must open");

    let sessions = store.load_sessions().expect("load sessions");
    assert_eq!(sessions.len(), 3, "{sessions:#?}");

    let one = session(&sessions, "sess-1");
    assert_eq!(one.project_id, "proj-widget");
    assert_eq!(one.provider.as_str(), "claude");
    assert_eq!(one.source_branch, "main");
    assert_eq!(one.branch_name, "dux/lively-otter");
    assert_eq!(
        one.worktree_path,
        "/home/ada/.config/dux/worktrees/lively-otter"
    );
    assert_eq!(one.status, SessionStatus::Active);
    assert_eq!(one.created_at.to_rfc3339(), "2024-01-01T00:00:00+00:00");
    assert_eq!(one.updated_at.to_rfc3339(), "2024-01-05T00:00:00+00:00");

    // A non-default provider and a non-default status both come back as written.
    assert_eq!(session(&sessions, "sess-2").provider.as_str(), "codex");
    assert_eq!(session(&sessions, "sess-2").status, SessionStatus::Detached);
    assert_eq!(session(&sessions, "sess-3").provider.as_str(), "opencode");
    assert_eq!(session(&sessions, "sess-3").source_branch, "develop");
}

#[test]
fn an_old_database_opens_and_every_project_row_survives_intact() {
    let (_tmp, path) = old_database();
    let store = SessionStore::open(&path).expect("open");

    let projects = store.load_projects().expect("load projects");
    assert_eq!(projects.len(), 2, "{projects:#?}");
    let paths: Vec<&str> = projects.iter().map(|p| p.path.as_str()).collect();
    assert!(paths.contains(&"/home/ada/code/widget"), "{paths:?}");
    assert!(paths.contains(&"/home/ada/code/gadget"), "{paths:?}");

    // Columns added after these rows were written: absent means None, not an
    // error and not a wrong value.
    for project in &projects {
        assert_eq!(project.name, None);
        assert_eq!(project.default_provider, None);
        assert_eq!(project.startup_command, None);
        assert_eq!(project.auto_reopen_agents, None);
        assert!(project.env.is_empty());
    }

    // `created_at` was already stored and must not be re-stamped to now.
    let created = store.load_project_created_ats().expect("created ats");
    assert_eq!(
        created
            .get("proj-widget")
            .expect("proj-widget")
            .to_rfc3339(),
        "2024-01-01T00:00:00+00:00"
    );
}

#[test]
fn columns_added_since_arrive_at_their_documented_defaults() {
    let (_tmp, path) = old_database();
    let store = SessionStore::open(&path).expect("open");
    let sessions = store.load_sessions().expect("load");

    for s in &sessions {
        // `project_path` and `last_focused_tab` are nullable additions.
        assert_eq!(s.project_path, None, "{}", s.id);
        assert_eq!(s.last_focused_tab, None, "{}", s.id);
        // Auto-reopen INTENT defaults off (nothing was running at upgrade time),
        // while auto-reopen PERMISSION defaults on. Getting these two backwards
        // would either relaunch every old agent at once or silently disable the
        // feature for everyone who upgrades.
        assert!(!s.desired_running, "{}", s.id);
        assert!(s.auto_reopen_enabled, "{}", s.id);
        assert!(s.started_providers.is_empty(), "{}", s.id);
    }
}

#[test]
fn the_one_time_backfills_run_on_the_first_open_after_the_upgrade() {
    let (_tmp, path) = old_database();
    let store = SessionStore::open(&path).expect("open");
    let sessions = store.load_sessions().expect("load");

    for s in &sessions {
        // `initial_branch` freezes the branch lineage. The true original is lost
        // to the old schema, so the current branch is the best available answer,
        // and it must not be left empty.
        assert_eq!(s.initial_branch, s.branch_name, "{}", s.id);
        // A legacy agent's displayed name is frozen so it can never drift with a
        // later branch rename.
        assert_eq!(s.title.as_deref(), Some(s.branch_name.as_str()), "{}", s.id);
    }
}

#[test]
fn the_sort_order_backfill_preserves_the_order_the_user_last_saw() {
    // Before `sort_order` existed the list was ordered `updated_at DESC` per
    // project. The backfill has to reproduce exactly that, or every agent in the
    // sidebar moves on the first launch after the upgrade.
    let (_tmp, path) = old_database();
    let store = SessionStore::open(&path).expect("open");
    let sessions = store.load_sessions().expect("load");

    let widget: Vec<&str> = sessions
        .iter()
        .filter(|s| s.project_id == "proj-widget")
        .map(|s| s.id.as_str())
        .collect();
    // sess-1 updated 2024-01-05, sess-2 updated 2024-01-04.
    assert_eq!(widget, vec!["sess-1", "sess-2"], "{sessions:#?}");
}

#[test]
fn tables_added_since_are_created_empty_and_immediately_usable() {
    let (_tmp, path) = old_database();
    let store = SessionStore::open(&path).expect("open");

    // `app_state`: no row means "never seen a what's-new screen", which is what
    // makes the v0.7.0 screen show up for someone upgrading from v0.6.0.
    assert_eq!(store.last_seen_version().expect("read"), None);
    store.set_last_seen_version("v0.7.0").expect("write");
    assert_eq!(
        store.last_seen_version().expect("read back"),
        Some("v0.7.0".to_string())
    );

    // `agent_tabs`: zero rows, so every old agent comes up with just its
    // session-slot tab.
    assert!(store.load_agent_tabs().expect("load tabs").is_empty());
    store
        .insert_agent_tab(&AgentTab {
            id: "tab-x".to_string(),
            session_id: "sess-1".to_string(),
            provider: ProviderKind::new("codex"),
            sort_order: 0,
            created_at: Utc::now(),
        })
        .expect("insert tab");
    let tabs = store.load_agent_tabs().expect("load tabs");
    assert_eq!(tabs.len(), 1);
    assert_eq!(tabs[0].session_id, "sess-1");

    // `changes_rev`: a fresh counter that starts handing out revisions.
    assert_eq!(store.next_changes_rev("sess-1").expect("rev"), 1);
    assert_eq!(store.next_changes_rev("sess-1").expect("rev"), 2);

    // `session_prs`: empty, and writable.
    assert!(store.load_prs("sess-1").expect("load prs").is_empty());
    store
        .upsert_pr(&StoredPr {
            session_id: "sess-1".to_string(),
            pr_number: 42,
            host: "github.com".to_string(),
            owner_repo: "ada/widget".to_string(),
            state: "OPEN".to_string(),
            title: "Make it faster".to_string(),
            url: "https://github.com/ada/widget/pull/42".to_string(),
        })
        .expect("upsert pr");
    let prs = store.load_prs("sess-1").expect("load prs");
    assert_eq!(prs.len(), 1);
    assert_eq!(prs[0].pr_number, 42);
    assert_eq!(prs[0].title, "Make it faster");
}

#[test]
fn old_state_survives_a_write_a_close_and_a_reopen() {
    let (_tmp, path) = old_database();

    // First run after the upgrade: migrate, then do a normal amount of work.
    let migrated = {
        let store = SessionStore::open(&path).expect("open");
        let mut sessions = store.load_sessions().expect("load");
        let mine = session(&sessions, "sess-2").clone();
        // A status change, the commonest write there is.
        store
            .upsert_session(&AgentSession {
                status: SessionStatus::Active,
                ..mine
            })
            .expect("upsert");
        store.set_last_seen_version("v0.7.0").expect("app state");
        store
            .set_desired_running("sess-1", true)
            .expect("desired running");
        store
            .set_last_focused_tab("sess-1", Some("tab-x"))
            .expect("focused tab");
        sessions.sort_by(|a, b| a.id.cmp(&b.id));
        sessions
    };

    // Second run: everything old is still there, and the new writes stuck.
    let store = SessionStore::open(&path).expect("reopen");
    let mut sessions = store.load_sessions().expect("reload");
    sessions.sort_by(|a, b| a.id.cmp(&b.id));
    assert_eq!(sessions.len(), 3);

    // The untouched rows are byte-for-byte what the first open produced: the
    // second `migrate()` must not re-backfill anything.
    for (before, after) in migrated.iter().zip(sessions.iter()) {
        if before.id == "sess-2" || before.id == "sess-1" {
            continue; // deliberately written above
        }
        // Field-by-field: `AgentSession` has no `PartialEq`, and naming the
        // fields is the point anyway (a new field added without a thought about
        // reopen stability shows up here as a compile error, not a silent pass).
        assert_eq!(before.id, after.id);
        assert_eq!(before.project_id, after.project_id);
        assert_eq!(before.provider.as_str(), after.provider.as_str());
        assert_eq!(before.source_branch, after.source_branch);
        assert_eq!(before.branch_name, after.branch_name);
        assert_eq!(before.initial_branch, after.initial_branch);
        assert_eq!(before.worktree_path, after.worktree_path);
        assert_eq!(before.title, after.title);
        assert_eq!(before.project_path, after.project_path);
        assert_eq!(before.started_providers, after.started_providers);
        assert_eq!(before.desired_running, after.desired_running);
        assert_eq!(before.auto_reopen_enabled, after.auto_reopen_enabled);
        assert_eq!(before.status, after.status);
        assert_eq!(before.created_at, after.created_at);
        assert_eq!(before.updated_at, after.updated_at);
        assert_eq!(before.last_focused_tab, after.last_focused_tab);
    }

    assert_eq!(session(&sessions, "sess-2").status, SessionStatus::Active);
    assert!(session(&sessions, "sess-1").desired_running);
    assert_eq!(
        session(&sessions, "sess-1").last_focused_tab.as_deref(),
        Some("tab-x")
    );
    assert_eq!(
        store.last_seen_version().expect("read"),
        Some("v0.7.0".to_string())
    );
    // The original rows kept their identity through both opens.
    assert_eq!(session(&sessions, "sess-3").branch_name, "dux/quiet-moose");
    assert_eq!(store.load_projects().expect("projects").len(), 2);
}

#[test]
fn a_migrated_title_is_not_re_frozen_when_a_later_agent_leaves_it_null() {
    // `title IS NULL` is the ongoing state of an auto-named agent, so the
    // one-time title freeze must never run again. If it did, every agent created
    // after the upgrade would have its name pinned on the next config reload and
    // would stop tracking its branch.
    let (_tmp, path) = old_database();
    let store = SessionStore::open(&path).expect("first open");
    let now = Utc::now();
    store
        .upsert_session(&AgentSession {
            id: "sess-new".to_string(),
            project_id: "proj-widget".to_string(),
            project_path: None,
            provider: ProviderKind::new("claude"),
            source_branch: "main".to_string(),
            branch_name: "dux/post-upgrade".to_string(),
            initial_branch: "dux/post-upgrade".to_string(),
            worktree_path: "/tmp/post-upgrade".to_string(),
            title: None,
            started_providers: Vec::new(),
            desired_running: false,
            auto_reopen_enabled: true,
            status: SessionStatus::Active,
            created_at: now,
            updated_at: now,
            last_focused_tab: None,
        })
        .expect("insert a post-upgrade agent");
    drop(store);

    let store = SessionStore::open(&path).expect("second open");
    let sessions = store.load_sessions().expect("load");
    assert_eq!(
        session(&sessions, "sess-new").title,
        None,
        "the one-time title freeze re-ran and pinned an auto-named agent"
    );
    // ...while the legacy agents keep the title the freeze gave them.
    assert_eq!(
        session(&sessions, "sess-1").title.as_deref(),
        Some("dux/lively-otter")
    );
}

#[test]
fn every_column_added_since_is_nullable_or_defaulted_so_an_older_binary_can_still_insert() {
    // The downgrade property that IS testable in one checkout. An older binary's
    // INSERT names only the columns it knows about, so every column added after
    // it shipped must be satisfiable without being named. A `not null` column
    // with no default would make that INSERT fail, which is how an upgrade
    // becomes a one-way door.
    let (_tmp, path) = old_database();
    let store = SessionStore::open(&path).expect("open");
    drop(store);

    // The columns the old binary named in its own INSERTs (i.e. [`OLD_SCHEMA`]).
    // Only the columns OUTSIDE this set are the ones the property is about: an
    // original `not null` column with no default is fine, because the old binary
    // always supplied it.
    let original: &[(&str, &[&str])] = &[
        (
            "agent_sessions",
            &[
                "id",
                "project_id",
                "provider",
                "source_branch",
                "branch_name",
                "worktree_path",
                "status",
                "created_at",
                "updated_at",
            ],
        ),
        ("projects", &["id", "path", "created_at", "updated_at"]),
    ];

    let conn = Connection::open(&path).expect("reopen raw");
    for (table, original_columns) in original {
        let mut stmt = conn
            .prepare(&format!("pragma table_info({table})"))
            .expect("table_info");
        let rows = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(1)?,         // name
                    row.get::<_, i64>(3)? != 0,       // notnull
                    row.get::<_, Option<String>>(4)?, // dflt_value
                ))
            })
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("rows");
        let mut added = 0usize;
        for (name, not_null, default) in rows {
            if original_columns.contains(&name.as_str()) {
                continue;
            }
            added += 1;
            assert!(
                !not_null || default.is_some(),
                "{table}.{name} was added after {table} shipped and is NOT NULL with \
                 no default, so an older binary's INSERT that does not name it would fail"
            );
        }
        assert!(
            added > 0,
            "{table} gained no columns, so this test is checking nothing"
        );
    }

    // And prove it, rather than only asserting the schema shape: replay the exact
    // INSERT the old binary issued against the migrated table.
    conn.execute_batch(
        "insert into agent_sessions
           (id, project_id, provider, source_branch, branch_name, worktree_path, status, created_at, updated_at)
         values ('sess-legacy-insert', 'proj-widget', 'claude', 'main', 'dux/old-writer',
                 '/tmp/old-writer', 'active', '2024-02-01T00:00:00Z', '2024-02-01T00:00:00Z');
         insert into projects (id, path, created_at, updated_at)
         values ('proj-legacy-insert', '/home/ada/code/legacy',
                 '2024-02-01T00:00:00Z', '2024-02-01T00:00:00Z');",
    )
    .expect("an older binary's INSERT must still satisfy the migrated schema");
    drop(conn);

    // ...and the current binary reads what the old writer wrote.
    let store = SessionStore::open(&path).expect("reopen through the store");
    let sessions = store.load_sessions().expect("load");
    assert_eq!(
        session(&sessions, "sess-legacy-insert").branch_name,
        "dux/old-writer"
    );
    // The ungated `initial_branch` self-heal covers the row the old writer left
    // empty, so nothing is stranded without a lineage.
    assert_eq!(
        session(&sessions, "sess-legacy-insert").initial_branch,
        "dux/old-writer"
    );
}

#[test]
fn opening_a_database_this_build_created_is_a_no_op_the_second_time() {
    // The baseline the old-database tests are compared against: a fresh database
    // must also be stable across opens, or "nothing changed" would prove nothing.
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("sessions.sqlite3");
    let now = Utc::now();
    {
        let store = SessionStore::open(&path).expect("create");
        store
            .upsert_project(&dux_core::config::ProjectConfig {
                id: "p1".to_string(),
                path: "/home/ada/code/fresh".to_string(),
                name: Some("fresh".to_string()),
                default_provider: Some("codex".to_string()),
                leading_branch: None,
                auto_reopen_agents: Some(true),
                startup_command: Some("just setup".to_string()),
                env: std::collections::BTreeMap::from([("TOKEN".to_string(), "abc".to_string())]),
            })
            .expect("upsert project");
        store
            .upsert_session(&AgentSession {
                id: "s1".to_string(),
                project_id: "p1".to_string(),
                project_path: Some("/home/ada/code/fresh".to_string()),
                provider: ProviderKind::new("codex"),
                source_branch: "main".to_string(),
                branch_name: "dux/fresh".to_string(),
                initial_branch: "dux/fresh".to_string(),
                worktree_path: "/tmp/fresh".to_string(),
                title: Some("fresh".to_string()),
                started_providers: vec!["codex".to_string()],
                desired_running: true,
                auto_reopen_enabled: false,
                status: SessionStatus::Active,
                created_at: now,
                updated_at: now,
                last_focused_tab: None,
            })
            .expect("upsert session");
    }

    let store = SessionStore::open(&path).expect("reopen");
    let sessions = store.load_sessions().expect("load");
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].title.as_deref(), Some("fresh"));
    assert!(sessions[0].desired_running);
    assert!(!sessions[0].auto_reopen_enabled);
    assert_eq!(sessions[0].started_providers, vec!["codex".to_string()]);
    assert_eq!(sessions[0].provider.as_str(), "codex");

    let projects = store.load_projects().expect("projects");
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].startup_command.as_deref(), Some("just setup"));
    assert_eq!(
        projects[0].env.get("TOKEN").map(String::as_str),
        Some("abc")
    );
}
