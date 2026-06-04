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

use anyhow::{Result, anyhow, bail};
use dux_core::config::DuxPaths;

/// Resolve the address `dux server` should bind to, enforcing the no-auth safety
/// gate. CLI values take precedence over config values.
///
/// The web UI ships with NO authentication yet, so binding to anything other than
/// loopback is refused unless the operator explicitly opts in (via the
/// `--insecure-allow-remote` CLI flag or `insecure_allow_remote = true` under
/// `[server]` in config.toml).
pub fn resolve_bind(
    cfg_bind: &str,
    cfg_insecure_allow_remote: bool,
    cli_bind: Option<&str>,
    cli_insecure_allow_remote: bool,
) -> Result<SocketAddr> {
    let raw = cli_bind.unwrap_or(cfg_bind);
    let addr: SocketAddr = raw.parse().map_err(|_| {
        anyhow!(
            "invalid bind address \"{raw}\": expected IP:port, \
             e.g. 127.0.0.1:8080 or 0.0.0.0:8080"
        )
    })?;

    let allow_remote = cli_insecure_allow_remote || cfg_insecure_allow_remote;
    if !addr.ip().is_loopback() && !allow_remote {
        bail!(
            "refusing to bind {addr}: the dux web UI has no authentication yet, \
             so anyone who can reach this address can control your agents and worktrees. \
             To proceed deliberately, re-run with --insecure-allow-remote, \
             or set insecure_allow_remote = true under [server] in config.toml."
        );
    }

    Ok(addr)
}

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
mod resolve_bind_tests {
    use super::resolve_bind;

    #[test]
    fn default_loopback_passes_without_opt_in() {
        let addr = resolve_bind("127.0.0.1:8080", false, None, false).expect("loopback ok");
        assert_eq!(addr.to_string(), "127.0.0.1:8080");
    }

    #[test]
    fn cli_bind_overrides_config_bind() {
        let addr =
            resolve_bind("127.0.0.1:8080", false, Some("127.0.0.1:9999"), false).expect("cli ok");
        assert_eq!(addr.to_string(), "127.0.0.1:9999");
    }

    #[test]
    fn invalid_value_error_mentions_the_value() {
        let err = resolve_bind("not-an-addr", false, None, false)
            .expect_err("invalid address should error");
        let msg = err.to_string();
        assert!(
            msg.contains("not-an-addr"),
            "error should name the value: {msg}"
        );
        assert!(
            msg.contains("IP:port"),
            "error should explain the shape: {msg}"
        );
    }

    #[test]
    fn non_loopback_without_opt_in_errors() {
        let err = resolve_bind("0.0.0.0:8080", false, None, false)
            .expect_err("non-loopback without opt-in should error");
        let msg = err.to_string();
        assert!(
            msg.contains("--insecure-allow-remote"),
            "error should point to the CLI flag: {msg}"
        );
        assert!(
            msg.contains("authentication"),
            "error should explain why it refused: {msg}"
        );
    }

    #[test]
    fn non_loopback_with_cli_opt_in_passes() {
        let addr = resolve_bind("0.0.0.0:8080", false, None, true).expect("cli opt-in ok");
        assert_eq!(addr.to_string(), "0.0.0.0:8080");
    }

    #[test]
    fn non_loopback_with_config_opt_in_passes() {
        let addr = resolve_bind("0.0.0.0:8080", true, None, false).expect("config opt-in ok");
        assert_eq!(addr.to_string(), "0.0.0.0:8080");
    }

    #[test]
    fn loopback_ipv6_passes_without_opt_in() {
        let addr = resolve_bind("[::1]:8080", false, None, false).expect("ipv6 loopback ok");
        assert!(addr.ip().is_loopback());
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
