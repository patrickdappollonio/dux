//! Test-only helpers shared by the REST route modules' `#[cfg(test)]` suites:
//! a minimal headless engine handle plus a plain router builder. With no
//! password in the test config, the auth layer lets every route through.
//! Mirrors the private `test_engine_handle` in `server.rs`, lifted here so every
//! route module can boot the same engine without duplicating the recipe.

use std::net::SocketAddr;
use std::path::Path;

use axum::Router;
use dux_core::test_scratch::ScratchDir;

use crate::engine_actor::EngineHandle;
use crate::server;

/// [`crate::bootstrap::bootstrap_engine`] for tests: the same boot, with every
/// provider pointed at a harmless stand-in under its stock name and terminals at
/// a plain `sh`, so no test that creates or launches an agent can exec the
/// developer's real agent CLI, and none depends on the developer's `$SHELL`. Every
/// test in this crate boots through here rather than the production function.
///
/// The `gh` probe the engine loop starts is pointed at a program that does not
/// exist, so it settles at once on "not installed" and never runs the
/// developer's real `gh`. A real `gh` answers on its own schedule (it may ask
/// GitHub over the network), and the moment it does it flips availability and
/// fires `config.changed`, an event no test asked for that arrives whenever it
/// likes. A test that needs `gh` points the probe at a stand-in of its own.
pub(crate) fn bootstrap_test_engine(
    paths: &dux_core::config::DuxPaths,
) -> anyhow::Result<dux_core::engine::Engine> {
    let mut engine = crate::bootstrap::bootstrap_engine(paths)?;
    dux_core::test_provider::defuse_config(&mut engine.config);
    engine.gh_probe.program = paths.root.join("gh-is-not-installed").into_os_string();
    Ok(engine)
}

/// Boot a minimal headless engine handle rooted at `tmp`. The handle just needs
/// to exist; routing-only tests never drive a real agent through it.
pub(crate) fn test_engine_handle(tmp: &Path) -> EngineHandle {
    let paths = dux_core::config::DuxPaths {
        root: tmp.to_path_buf(),
        config_path: tmp.join("config.toml"),
        sessions_db_path: tmp.join("sessions.sqlite3"),
        worktrees_root: tmp.join("worktrees"),
        lock_path: tmp.join("dux.lock"),
    };
    std::fs::create_dir_all(&paths.worktrees_root).unwrap();
    let engine = crate::test_support::bootstrap_test_engine(&paths).unwrap();
    let (handle, _join) = crate::engine_actor::spawn_engine_thread(engine);
    handle
}

/// A fresh scratch dir + an engine-backed router. Returns the `ScratchDir` so the
/// caller keeps it alive for the test's duration.
pub(crate) fn router_no_auth() -> (ScratchDir, Router) {
    let tmp = ScratchDir::new();
    let router = server::router(test_engine_handle(tmp.path()));
    (tmp, router)
}

/// Bind a real loopback server on an ephemeral port and serve the plain router on
/// a background task. Returns the bound `SocketAddr` so an integration test can
/// issue real HTTP/WebSocket requests against it. The `ScratchDir` is kept alive by
/// the returned guard; drop it to clean up the engine's on-disk state.
#[allow(dead_code)]
pub(crate) async fn boot_plain_test_server() -> (ScratchDir, SocketAddr) {
    let tmp = ScratchDir::new();
    let app = server::router(test_engine_handle(tmp.path()));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            app.into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    (tmp, addr)
}

/// A managed agent of project `p1`, for a test that puts it in an engine by
/// hand before the actor starts. Its worktree path does not exist.
pub(crate) fn sample_agent(id: &str) -> dux_core::model::AgentSession {
    let now = chrono::Utc::now();
    dux_core::model::AgentSession {
        id: id.to_string(),
        slot_tab_id: format!("{id}-slot"),
        provider: dux_core::model::ProviderKind::new("claude"),
        title: Some(format!("{id}-title")),
        started_providers: Vec::new(),
        desired_running: false,
        auto_reopen_enabled: false,
        status: dux_core::model::SessionStatus::Detached,
        created_at: now,
        updated_at: now,
        last_focused_tab: None,
        workspace: dux_core::model::AgentWorkspace::Managed(dux_core::model::ManagedWorkspace {
            project_id: "p1".to_string(),
            project_path: None,
            source_branch: "main".to_string(),
            branch_name: "feat".to_string(),
            initial_branch: "feat".to_string(),
            branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
            worktree_path: format!("/tmp/{id}-worktree"),
        }),
    }
}

/// [`bootstrap_test_engine`] rooted at `tmp`, before an actor owns it, so a
/// test can put state in it by hand first.
pub(crate) fn unstarted_test_engine(tmp: &Path) -> dux_core::engine::Engine {
    let paths = dux_core::config::DuxPaths {
        root: tmp.to_path_buf(),
        config_path: tmp.join("config.toml"),
        sessions_db_path: tmp.join("sessions.sqlite3"),
        worktrees_root: tmp.join("worktrees"),
        lock_path: tmp.join("dux.lock"),
    };
    std::fs::create_dir_all(&paths.worktrees_root).unwrap();
    bootstrap_test_engine(&paths).unwrap()
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_test_boot_cannot_launch_a_real_agent_cli() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = dux_core::config::DuxPaths {
            root: tmp.path().to_path_buf(),
            config_path: tmp.path().join("config.toml"),
            sessions_db_path: tmp.path().join("sessions.sqlite3"),
            worktrees_root: tmp.path().join("worktrees"),
            lock_path: tmp.path().join("dux.lock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).unwrap();
        let engine = super::bootstrap_test_engine(&paths).unwrap();
        dux_core::test_provider::assert_fixture_config_is_harmless(&engine.config);
    }
}
