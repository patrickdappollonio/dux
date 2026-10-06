//! SIGUSR1 for the serves that own the engine: `dux server` and the
//! start-web-server flip. Each turns the signal into the engine actor's own
//! reload arm, exactly as the "Reload config" command does, so a hand edit
//! followed by `kill -USR1` is live at once. The background serve installs nothing
//! here: the terminal UI beside it drives the engine and reloads for both
//! (see `dux_core::reload_signal`).

use dux_core::wire::WireCommand;

use crate::engine_actor::EngineHandle;

/// Run the engine's config reload on every SIGUSR1 until `stop` flips. A
/// signal that arrived before this task started (recorded by the process-wide
/// flag) is honored once at the start.
///
/// Ends with the serve, so it never keeps the engine's request channel open
/// past it.
pub(crate) async fn reload_on_signal(
    handle: EngineHandle,
    stop: tokio::sync::watch::Receiver<bool>,
) {
    let mut signals =
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1()) {
            Ok(signals) => signals,
            Err(error) => {
                dux_core::logger::error(&format!(
                    "[server] could not listen for SIGUSR1 ({error}); `kill -USR1` cannot \
                     reload this server, so reload the config from the app menu after a hand edit"
                ));
                return;
            }
        };
    if dux_core::reload_signal::take_pending() {
        reload(&handle).await;
    }
    let stopped = crate::serve_legs::wait_for_shutdown(stop);
    tokio::pin!(stopped);
    loop {
        tokio::select! {
            _ = &mut stopped => return,
            received = signals.recv() => {
                if received.is_none() {
                    return;
                }
                // The flag was set by the same delivery; clear it so the
                // terminal UI, if the flip hands back to it, does not reload
                // a second time for a signal already answered.
                let _ = dux_core::reload_signal::take_pending();
                reload(&handle).await;
            }
        }
    }
}

async fn reload(handle: &EngineHandle) {
    dux_core::logger::info("[server] SIGUSR1 received: reloading config.toml");
    if let Err(error) = handle.apply_wire(WireCommand::ReloadConfig {}).await {
        dux_core::logger::error(&format!(
            "[server] the config reload asked for by SIGUSR1 did not start: {error}"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bootstrap::bootstrap_engine;
    use crate::engine_actor::spawn_engine_thread;
    use dux_core::config::DuxPaths;

    fn temp_paths() -> (tempfile::TempDir, DuxPaths) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let paths = DuxPaths {
            root: root.clone(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            socket_path: root.join("dux.sock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).expect("worktrees dir");
        (tmp, paths)
    }

    /// The path `dux server` and the flip take: a real SIGUSR1 to this
    /// process reaches the engine actor's reload, and the task ends with the
    /// serve.
    #[tokio::test]
    async fn a_sigusr1_reloads_the_engine_and_the_task_ends_with_the_serve() {
        dux_core::reload_signal::install().expect("install the flag handler");
        let (_tmp, paths) = temp_paths();
        let engine = bootstrap_engine(&paths).expect("bootstrap");
        let (handle, _join) = spawn_engine_thread(engine);
        let guard_set = handle.live_limits().allowed_hosts();
        let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);
        let task = tokio::spawn(reload_on_signal(handle.clone(), stop_rx));
        // Let the task register its stream before the signal is sent.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        std::fs::write(
            &paths.config_path,
            "[server]\nallowed_hosts = [\"signalled.example.com\"]\n",
        )
        .expect("edit config.toml by hand");
        rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::USR1)
            .expect("signal this process");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while guard_set.snapshot() != vec!["signalled.example.com".to_string()] {
            assert!(
                std::time::Instant::now() < deadline,
                "the signal never reloaded"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        stop_tx.send(true).expect("stop");
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .expect("the task ends with the serve")
            .expect("no panic");
    }
}
