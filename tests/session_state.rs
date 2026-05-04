//! Integration tests for audit02 P1-Z (Phase 18) — explicit session
//! state machine, phase 1 of 2.
//!
//! These tests pin the legal-transition matrix on
//! [`dux::model::SessionState`] and the persistence round-trip via the
//! new `agent_sessions.state_json` column added by migration 0002.
//! Phase 2 will swap `SessionStatus` for a `state` field on
//! [`dux::model::AgentSession`] and embed PTY handles inside `Live` /
//! `Detached`; until that lands, these tests are the safety net that
//! makes sure the JSON shape and the transition rules don't drift.

use chrono::{Duration, Utc};
use dux::model::{AgentSession, PersistedSessionState, ProviderKind, SessionState, SessionStatus};
use dux::storage::SessionStore;

/// Illegal transitions must be rejected loudly. We can't `Detach` a
/// brand-new session — there is no PTY to detach from. Phase 18's
/// whole point is to fail-fast on these instead of silently no-oping.
#[test]
fn illegal_transition_rejected() {
    let now = Utc::now();
    let state = SessionState::Created { created_at: now };

    let result = state.clone().transition("detached", now);
    assert!(
        result.is_err(),
        "Created -> detached should be rejected, got {result:?}"
    );

    // Self-transitions are also rejected — they would mask genuine
    // duplicate-event bugs (e.g. two `on_spawn_succeeded` calls).
    let exited = SessionState::Exited {
        exit_code: Some(0),
        exited_at: now,
    };
    let self_result = exited.transition("exited", now);
    assert!(
        self_result.is_err(),
        "Exited -> exited self-transition should be rejected"
    );
}

/// The happy path: Created -> Spawning -> Live -> Detached -> Live ->
/// Exited. Every step should succeed and leave behind the timestamps
/// supplied to `transition`. This locks in the legal-transition graph
/// the runtime layer is going to depend on.
#[test]
fn valid_transitions_succeed() {
    let t0 = Utc::now();
    let t1 = t0 + Duration::seconds(1);
    let t2 = t0 + Duration::seconds(2);
    let t3 = t0 + Duration::seconds(3);
    let t4 = t0 + Duration::seconds(4);
    let t5 = t0 + Duration::seconds(5);

    let s = SessionState::Created { created_at: t0 };
    assert_eq!(s.name(), "created");

    let s = s.transition("spawning", t1).expect("created -> spawning");
    assert!(matches!(s, SessionState::Spawning { since } if since == t1));

    let s = s.transition("live", t2).expect("spawning -> live");
    assert!(
        matches!(s, SessionState::Live { spawned_at, last_active_at } if spawned_at == t2 && last_active_at == t2)
    );

    let s = s.transition("detached", t3).expect("live -> detached");
    assert!(matches!(s, SessionState::Detached { detached_at } if detached_at == t3));

    let s = s.transition("live", t4).expect("detached -> live");
    assert!(matches!(s, SessionState::Live { spawned_at, .. } if spawned_at == t4));

    let s = s.transition("exited", t5).expect("live -> exited");
    assert!(matches!(s, SessionState::Exited { exited_at, .. } if exited_at == t5));
}

/// `SessionState::to_json` -> `from_json` must round-trip every
/// persistable variant. `Live` is intentionally excluded because the
/// persistence layer collapses it to `Detached` — that fold is
/// covered by the next test.
#[test]
fn json_round_trip_for_persistable_variants() {
    let now = Utc::now();
    let cases = vec![
        SessionState::Created { created_at: now },
        SessionState::Spawning { since: now },
        SessionState::Detached { detached_at: now },
        SessionState::Exited {
            exit_code: Some(137),
            exited_at: now,
        },
        SessionState::Exited {
            exit_code: None,
            exited_at: now,
        },
    ];
    for state in cases {
        let json = state.to_json().expect("serialize");
        let back = SessionState::from_json(&json).expect("deserialize");
        assert_eq!(state, back, "round-trip mismatch for {state:?}");
    }
}

/// `Live` is special: it cannot survive a process restart because the
/// PTY handle is gone. Persist + reload must collapse `Live` into
/// `Detached`. This test asserts that contract directly on the
/// `From<SessionState> for PersistedSessionState` impl.
#[test]
fn live_collapses_to_detached_when_persisted() {
    let now = Utc::now();
    let later = now + Duration::seconds(10);
    let live = SessionState::Live {
        spawned_at: now,
        last_active_at: later,
    };

    let persisted: PersistedSessionState = live.into();
    match persisted {
        PersistedSessionState::Detached { detached_at } => {
            assert_eq!(
                detached_at, later,
                "Live -> Detached should preserve last_active_at as detached_at"
            );
        }
        other => panic!("expected PersistedSessionState::Detached, got {other:?}"),
    }
}

/// Migration 0002 adds the `state_json` column. After
/// `SessionStore::open` runs migrations on a fresh DB, that column
/// must exist on `agent_sessions`. The schema version must also be
/// at least 2.
#[test]
fn migration_0002_adds_state_json_column() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("phase18.sqlite3");

    let store = SessionStore::open(&path).expect("open fresh DB");
    let user_version: u32 = store
        .conn()
        .query_row("PRAGMA user_version;", [], |r| r.get(0))
        .expect("read user_version");
    assert!(
        user_version >= 2,
        "expected user_version >= 2 after Phase 18 migration, got {user_version}"
    );

    let columns: Vec<String> = store
        .conn()
        .prepare("pragma table_info(agent_sessions)")
        .expect("prepare")
        .query_map([], |row| row.get::<_, String>(1))
        .expect("query")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("collect");
    assert!(
        columns.iter().any(|c| c == "state_json"),
        "agent_sessions must have a state_json column after migration 0002; got {columns:?}"
    );
}

/// End-to-end: an `AgentSession` written through `upsert_session`
/// should populate `state_json`, and re-loading the session should
/// recover the same legacy `SessionStatus`. This exercises the
/// new write + read paths together so a future regression in either
/// direction shows up immediately.
#[test]
fn session_state_persists_round_trip_through_store() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = tmp.path().join("rt.sqlite3");
    let store = SessionStore::open(&path).expect("open");

    let now = Utc::now();
    let session = AgentSession {
        id: "rt-1".to_string(),
        project_id: "p".to_string(),
        project_path: None,
        provider: ProviderKind::new("claude"),
        source_branch: "main".to_string(),
        branch_name: "feat/phase-18".to_string(),
        worktree_path: "/tmp/rt-1".to_string(),
        title: None,
        started_providers: Vec::new(),
        status: SessionStatus::Detached,
        created_at: now,
        updated_at: now,
    };
    store.upsert_session(&session).expect("upsert");

    // Verify the column was populated with JSON we can parse back.
    let raw_json: Option<String> = store
        .conn()
        .query_row(
            "select state_json from agent_sessions where id = ?1",
            rusqlite::params!["rt-1"],
            |row| row.get(0),
        )
        .expect("select state_json");
    let raw_json = raw_json.expect("state_json should be populated by upsert");
    let parsed = SessionState::from_json(&raw_json).expect("parse JSON");
    assert!(
        matches!(parsed, SessionState::Detached { .. }),
        "expected Detached after Detached upsert, got {parsed:?}"
    );

    // Reload via the public API and confirm the legacy status agrees.
    let loaded = store.load_sessions().expect("load");
    let row = loaded.iter().find(|s| s.id == "rt-1").expect("loaded row");
    assert_eq!(row.status, SessionStatus::Detached);
}
