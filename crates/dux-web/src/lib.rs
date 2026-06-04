//! Placeholder for sub-project #3 — the web layer that will expose the
//! `dux-core` engine over HTTP/WebSocket. Empty for now so the workspace
//! topology is ready: this crate depends on `dux-core` (not `dux-tui`).
//!
//! Dependency isolation is enforced by the `dep-isolation` CI job, which
//! runs `cargo tree -p dux-web` and fails if any TUI-only crate appears.

pub mod bootstrap;
pub mod engine_actor;
pub mod protocol;
pub mod server;
pub mod web_assets;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::Result;
use dux_core::config::DuxPaths;
use dux_core::engine::Engine;

use crate::engine_actor::LoopControl;

/// The no-auth bind gate now lives in `dux-core` so both server entry points
/// (the `dux server` CLI and the in-process TUI↔server flip's pre-flight in
/// dux-tui) share it without dux-tui depending on dux-web. Re-exported here so
/// `crates/dux/src/main.rs` keeps calling `dux_web::resolve_bind` unchanged.
pub use dux_core::config::resolve_server_bind as resolve_bind;

/// Boot the engine on its own thread and serve the web UI on `addr` (loopback for now).
/// Blocking entry — builds its own tokio runtime.
pub fn run_server(paths: DuxPaths, addr: SocketAddr) -> Result<()> {
    let engine = bootstrap::bootstrap_engine(&paths)?;
    let (handle, _join) = engine_actor::spawn_engine_thread(engine);
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        let app = server::router(handle.clone());
        let listener = tokio::net::TcpListener::bind(addr).await?;
        axum::serve(listener, app)
            .with_graceful_shutdown(shutdown_signal())
            .await?;
        // SIGTERM the agents (they save state for a later resume), mark their
        // sessions Detached, then exit; Drop hard-kills any straggler.
        handle.shutdown().await;
        Ok::<(), anyhow::Error>(())
    })
}

/// What the status-screen tick asks `serve_with_engine` to do after the current
/// iteration. `Continue` keeps serving; `ReturnToTui` flips back to the TUI
/// (server torn down, PTYs preserved); `QuitProcess` exits the whole process
/// (server torn down, agents SIGTERMed).
pub enum ServerTick {
    Continue,
    ReturnToTui,
    QuitProcess,
}

/// How `serve_with_engine` exited, so the binary's orchestration loop knows
/// whether to resume the TUI or quit.
pub enum ServerExit {
    ReturnToTui,
    QuitProcess,
}

/// Grace period given to agent/terminal children to flush state on a quit, the
/// same window the dedicated-thread `Shutdown` path uses.
const QUIT_PTY_GRACE: Duration = Duration::from_millis(1500);

/// Upper bound on how long the flip waits for the axum server task to finish
/// after graceful shutdown is triggered. A wedged client connection must not be
/// able to hang the flip back to the TUI, so we cap the join: the runtime is
/// dropped afterward regardless, which aborts any straggler task.
const SERVER_JOIN_TIMEOUT: Duration = Duration::from_secs(3);

/// Serve the web UI over an EXISTING engine on the CALLER's thread, returning
/// the engine when serving stops. This is the in-process TUI↔server flip's
/// entry point: the TUI hands its live `Engine` (PTYs running, owned on the main
/// thread) and a pre-bound std `TcpListener` here; this turns the caller's
/// thread INTO the engine-actor loop while axum serves on a background runtime.
///
/// `on_tick` runs once per engine-loop iteration (the binary implements it with
/// a dux-tui status screen that polls keys and redraws). Its return value drives
/// the exit:
/// - `Continue` keeps serving.
/// - `ReturnToTui` triggers graceful axum shutdown and returns `(engine,
///   ReturnToTui)` with PTYs UNTOUCHED — the TUI resumes around the same agents.
/// - `QuitProcess` (or a SIGINT/SIGTERM during serving) triggers graceful axum
///   shutdown, then SIGTERMs the children (`shutdown_ptys`) like the CLI path,
///   and returns `(engine, QuitProcess)`.
pub fn serve_with_engine(
    mut engine: Engine,
    listener: std::net::TcpListener,
    mut on_tick: impl FnMut() -> ServerTick,
) -> Result<(Engine, ServerExit)> {
    let (handle, ends) = engine_actor::build_actor_channels(&engine);
    engine_actor::spawn_global_workers(&mut engine);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;

    // The std listener travels through the flip (the TUI bound it BEFORE tearing
    // down, so there is no rebind race); tokio needs it non-blocking.
    listener.set_nonblocking(true)?;
    let tokio_listener = {
        let _guard = runtime.enter();
        tokio::net::TcpListener::from_std(listener)?
    };

    // Graceful-shutdown trigger for axum: the synchronous engine loop flips this
    // watch to `true` on exit, and the server's shutdown future awaits it.
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::watch::channel(false);
    // Set by the signal task; polled by the control closure so a SIGINT/SIGTERM
    // received while serving breaks the engine loop too (not just axum).
    let signal_quit = Arc::new(AtomicBool::new(false));

    let app = server::router(handle);
    let server_task = runtime.spawn(async move {
        axum::serve(tokio_listener, app)
            .with_graceful_shutdown(async move {
                // Wait until the loop flips the watch to `true`.
                while !*shutdown_rx.borrow_and_update() {
                    if shutdown_rx.changed().await.is_err() {
                        break;
                    }
                }
            })
            .await
    });

    // Signal task: trip the flag on SIGINT/SIGTERM so the control closure exits
    // the loop with QuitProcess on the next tick.
    let signal_flag = Arc::clone(&signal_quit);
    runtime.spawn(async move {
        shutdown_signal().await;
        signal_flag.store(true, Ordering::SeqCst);
    });

    // Run the engine loop on the CURRENT thread. The control closure decides the
    // exit reason: a tripped signal flag wins (QuitProcess), otherwise the
    // caller's tick result maps straight through.
    let mut exit = ServerExit::ReturnToTui;
    let mut engine = engine_actor::run_engine_loop(engine, ends, || {
        if signal_quit.load(Ordering::SeqCst) {
            exit = ServerExit::QuitProcess;
            return LoopControl::Exit;
        }
        match on_tick() {
            ServerTick::Continue => LoopControl::Continue,
            ServerTick::ReturnToTui => {
                exit = ServerExit::ReturnToTui;
                LoopControl::Exit
            }
            ServerTick::QuitProcess => {
                exit = ServerExit::QuitProcess;
                LoopControl::Exit
            }
        }
    });

    // Trigger graceful axum shutdown and wait (bounded) for the server task to
    // wind down. Dropping the runtime afterward aborts any straggler.
    let _ = shutdown_tx.send(true);
    runtime.block_on(async {
        let _ = tokio::time::timeout(SERVER_JOIN_TIMEOUT, server_task).await;
    });

    if matches!(exit, ServerExit::QuitProcess) {
        // Quit teardown: SIGTERM the children so CLIs can save state for a later
        // resume, mark agent sessions Detached. We own the engine here, so we
        // call `shutdown_ptys` directly (the dedicated-thread path routes the
        // equivalent through the `Shutdown` request).
        engine.shutdown_ptys(QUIT_PTY_GRACE);
    }
    // ReturnToTui intentionally leaves PTYs untouched so the resumed TUI finds
    // the same live agents.

    Ok((engine, exit))
}

/// Resolves when the process receives SIGINT (Ctrl-C) or SIGTERM, the standard
/// signals an operator or supervisor sends to stop the server.
async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut sig) => {
                sig.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[cfg(test)]
mod tests {
    use dux_core::engine::Command;

    /// Light smoke test that the public dux-core API can be invoked from
    /// dux-web without TUI imports. Real architectural enforcement of the
    /// "no TUI deps" rule lives in the `dep-isolation` CI job.
    #[test]
    fn dux_core_command_is_constructible() {
        let cmd = Command::OpenPath {
            path: std::path::PathBuf::from("/tmp/dux-web-smoke"),
            target: "session worktree".to_string(),
        };
        // Exercise pattern-matching so the variant fields are actually
        // referenced — a dead-code construction wouldn't catch API drift.
        match cmd {
            Command::OpenPath { path, target } => {
                assert_eq!(target, "session worktree");
                assert_eq!(path.display().to_string(), "/tmp/dux-web-smoke");
            }
            _ => unreachable!("constructed an OpenPath variant"),
        }
    }
}

#[cfg(test)]
mod config_saver_tests {
    use dux_core::config::{Config, DuxPaths};
    use dux_core::engine::ConfigSaver;
    use dux_core::worker::WorkerEvent;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::mpsc::{self, Sender};

    /// Web-layer placeholder: today no-op. Sub-project #3 will replace this
    /// with whatever persistence semantics the web layer actually needs.
    struct WebConfigSaver;

    impl ConfigSaver for WebConfigSaver {
        fn persist_global_env(
            &self,
            env: BTreeMap<String, String>,
            _config: Config,
            _config_path: PathBuf,
            worker_tx: Sender<WorkerEvent>,
        ) {
            let _ = worker_tx.send(WorkerEvent::GlobalEnvPersistenceCompleted {
                env,
                result: Ok(()),
            });
        }

        fn reload_config(&self, _paths: DuxPaths, worker_tx: Sender<WorkerEvent>) {
            let _ = worker_tx.send(WorkerEvent::ConfigReloadReady(Box::new(Ok(
                Config::default(),
            ))));
        }

        fn recover_config(
            &self,
            _config_path: PathBuf,
            _config: Config,
            worker_tx: Sender<WorkerEvent>,
        ) {
            let _ = worker_tx.send(WorkerEvent::ConfigRecoverCompleted(Ok(())));
        }
    }

    /// Proves the web layer can implement `ConfigSaver` against `dux-core`
    /// alone (no TUI deps). This is what unblocks sub-project #3 from
    /// reusing the engine's config-persistence command dispatch.
    #[test]
    fn web_can_implement_config_saver() {
        let (tx, rx) = mpsc::channel();
        let saver: Box<dyn ConfigSaver> = Box::new(WebConfigSaver);
        let mut env = BTreeMap::new();
        env.insert("X".into(), "1".into());
        saver.persist_global_env(
            env,
            Config::default(),
            PathBuf::from("/tmp/dux-web-test"),
            tx,
        );
        let event = rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .expect("event");
        assert!(matches!(
            event,
            WorkerEvent::GlobalEnvPersistenceCompleted { result: Ok(()), .. }
        ));
    }
}
