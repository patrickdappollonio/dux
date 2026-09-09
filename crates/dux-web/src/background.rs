//! The web server, serving in the background of a live terminal UI, which keeps
//! the engine and lends it to this type once per run-loop iteration.
//!
//! This drains no worker events and runs no shared maintenance sweep: those have
//! one runner per process and while this serves that runner is the terminal UI.
//! It calls only the web-only half of [`crate::engine_actor::EngineService`]:
//! pending PTY subscribes, the spine fingerprint, queued engine requests, timed
//! out statuses. It installs no signal handlers either; the terminal UI's own
//! handlers own SIGINT/SIGTERM and its quit path stops this serve.
//!
//! `Engine::surface_kind` stays `Tui` here even for a browser-launched agent, so
//! `auto`/`mirror` terminal identity resolves against the running terminal.
//!
//! A status the terminal UI sets by hand stays on its own status line; a status
//! carried by a drained worker event crosses the seam and reaches browsers too.
//! Web-originated statuses stay scoped per connection, decided in `handle_request`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};

use anyhow::Result;
use dux_core::background_serve::ServiceOutcome;
use dux_core::engine::{Engine, EventReaction};

use crate::console::Console;
use crate::engine_actor::{EngineService, FollowupRouting, ShutdownEcho, build_actor_channels};
use crate::{ServeCore, SignalPolicy};

/// A live background serve: the serve core plus the per-iteration servicing
/// state, held together so a stop drops both at once.
pub struct BackgroundServer {
    core: ServeCore,
    service: EngineService,
    /// Shared with every PTY forwarder. Tripped before the runtime is torn down,
    /// or a parked forwarder would keep the teardown waiting.
    shutdown_flag: Arc<AtomicBool>,
    urls: Vec<String>,
    /// The engine's total apply count as of the last iteration, so a change can
    /// be spotted without every terminal UI apply site announcing itself.
    last_command_applies: u64,
    /// This serve's PTY-ownership registry and the terminal UI's seat in it, taken
    /// at start. Both die with the serve, which is what releases everything on stop.
    ownership: dux_core::background_serve::TuiOwnership,
    /// The two buses this serve announces the terminal UI's ownership changes on,
    /// filled by `build_app`. Empty is survivable: nothing is announced and
    /// browsers fall back to the fingerprint backstop and the handshake.
    publisher: Arc<std::sync::OnceLock<crate::ownership_publish::OwnershipPublisher>>,
    /// How many browser tabs are connected, kept up to date by the router's
    /// connection registry. An atomic because the terminal UI reads it once per
    /// rendered frame from the thread that also services this serve.
    connections: Arc<AtomicUsize>,
}

impl BackgroundServer {
    /// Start serving `engine` on `listeners`, which the caller already bound: a
    /// bind failure is then a status line message, not a half-torn-down process.
    pub fn start(
        engine: &mut Engine,
        listeners: Vec<std::net::TcpListener>,
        urls: Vec<String>,
    ) -> Result<Self> {
        crate::warn_if_ui_not_built();
        // Writes nowhere: the terminal UI owns this terminal, and nothing on this
        // path reads a captured activity ring.
        let console = Console::noop();

        // The terminal UI's `App::run` already spawned the global background
        // workers and is still running; spawning them here would double them.
        debug_assert!(
            engine
                .changed_files_poller_started
                .load(std::sync::atomic::Ordering::Relaxed),
            "the terminal UI must already have spawned the global workers; the background \
             server must not spawn them a second time"
        );

        let (handle, ends) = build_actor_channels(engine);
        let shutdown_flag = handle.shutdown_flag();
        // The connection id comes from the same process-global counter every
        // browser socket draws from, so the two compare and cannot collide.
        let owners = handle.pty_input_owners();
        let ownership = dux_core::background_serve::TuiOwnership {
            conn_id: owners.next_conn_id(),
            owners,
        };
        let publisher = Arc::new(std::sync::OnceLock::new());
        let connections = Arc::new(AtomicUsize::new(0));
        let service = EngineService::new(engine, ends, ShutdownEcho::Silent);
        let core = ServeCore::start(
            handle,
            listeners,
            &engine.config,
            console,
            // The access log would print to a console that writes nowhere, and it
            // is never wanted over a terminal UI's frame regardless.
            false,
            SignalPolicy::Inherited,
            crate::BackgroundHooks {
                ownership_publisher: Some(Arc::clone(&publisher)),
                connections_gauge: Some(Arc::clone(&connections)),
            },
        )?;
        Ok(Self {
            core,
            service,
            shutdown_flag,
            urls,
            last_command_applies: engine.command_applies,
            ownership,
            publisher,
            connections,
        })
    }

    /// How many browser tabs are connected to this serve right now. Connections,
    /// not devices: two tabs of one browser count as two.
    pub fn connections(&self) -> usize {
        self.connections.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The addresses this serve is reachable on.
    pub fn urls(&self) -> &[String] {
        &self.urls
    }

    /// Whether a required leg's accept loop died, so the caller can stop serving
    /// and say so rather than leaving a dead server on screen.
    pub fn is_failed(&self) -> bool {
        self.core.is_failed()
    }

    /// Whether this serve installed the process's SIGINT/SIGTERM handlers. Always
    /// false: the terminal UI's handlers own them.
    pub fn installed_signal_handlers(&self) -> bool {
        self.core.installed_signal_handlers()
    }

    /// Ask this serve to change `[server] tailscale`, reporting through the
    /// terminal UI's worker event lane. Never blocks: the caller is the run loop
    /// that also services this serve, so waiting would stop both surfaces.
    pub fn set_tailscale_mode(
        &self,
        mode: dux_core::config::TailscaleMode,
        worker_tx: std::sync::mpsc::Sender<dux_core::worker::WorkerEvent>,
    ) {
        self.core
            .tailscale_mode()
            .set_mode_detached(mode, move |outcome| {
                let _ = worker_tx
                    .send(dux_core::worker::WorkerEvent::TailscaleModeApplied { mode, outcome });
            });
    }

    /// Stop serving and release everything, bounded. Returns the first listener
    /// failure if one was recorded. Consumes `self` because dropping the runtime
    /// is what reaps the tasks `build_app` spawned.
    pub fn stop(self) -> Option<anyhow::Error> {
        self.core.stop(&self.shutdown_flag)
    }

    /// Do the web layer's share of ONE reaction the terminal UI drained, before
    /// the terminal UI applies it.
    pub fn on_reaction(&mut self, engine: &mut Engine, reaction: &EventReaction) {
        // ByOrigin, not RunEverything: the terminal UI drained this reaction and
        // holds its own arm, so routable follow-ups run on exactly one surface.
        self.service
            .fanout_reaction(engine, reaction, FollowupRouting::ByOrigin);
        // Read-only: the drainer still owns adopting the config.
        self.service.announce_config_reload(engine, reaction);
        // Unconditional; the fingerprint compare is the precise emit gate.
        self.service.note_mutation();
    }

    /// Emit the browser-facing half of the maintenance sweeps the terminal UI ran
    /// this iteration: the exit and close notices, and the change gate they open.
    /// Nothing is swept here, because the drainer already swept it.
    pub fn note_maintenance(
        &mut self,
        maintenance: &dux_core::background_serve::DrainedMaintenance,
    ) {
        self.service.note_drained_maintenance(maintenance);
    }

    /// Do the web-only per-iteration work.
    pub fn service(&mut self, engine: &mut Engine) -> ServiceOutcome {
        self.service.service_engine_once(engine)
    }

    /// The terminal UI's seat in this serve's PTY-ownership registry.
    pub fn ownership(&self) -> dux_core::background_serve::TuiOwnership {
        self.ownership.clone()
    }

    /// Announce ownership facts the terminal UI produced, on the same two buses a
    /// browser's own claim announces on. A publisher `build_app` never filled is
    /// logged once per batch rather than swallowed.
    pub fn publish_ownership_events(
        &mut self,
        events: &[dux_core::background_serve::PtyOwnershipEvent],
    ) {
        if events.is_empty() {
            return;
        }
        match self.publisher.get() {
            Some(publisher) => publisher.publish(events),
            None => dux_core::logger::warn(
                "[server] the background web server has no ownership publisher, so browsers were \
                 not told that the terminal UI took over a terminal. They will notice at their \
                 next reconnect.",
            ),
        }
    }

    /// Adopt the `[server]` section the terminal UI just swapped in, so the limits
    /// the routes read per request stop answering on the old config.
    pub fn note_config_applied(&mut self, server: &dux_core::config::ServerConfig) {
        self.service.note_config_applied(server);
    }

    /// Open the spine-change gate when the terminal UI applied anything since the
    /// last iteration: it applies over channels `request_mutates_spine` never sees,
    /// so without this a browser waits for the fingerprint backstop.
    pub fn note_engine_activity(&mut self, command_applies: u64) {
        if command_applies != self.last_command_applies {
            self.last_command_applies = command_applies;
            self.service.note_mutation();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::BackgroundServer;

    /// An engine on a fresh temp root, with the global-worker flags in the state
    /// `App::run` would have left them in.
    fn engine_in_tempdir() -> (dux_core::engine::Engine, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let paths = dux_core::config::DuxPaths {
            root: tmp.path().to_path_buf(),
            config_path: tmp.path().join("config.toml"),
            sessions_db_path: tmp.path().join("sessions.sqlite3"),
            worktrees_root: tmp.path().join("worktrees"),
            lock_path: tmp.path().join("dux.lock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).expect("worktrees dir");
        let engine = crate::bootstrap::bootstrap_engine(&paths).expect("engine");
        // The terminal UI spawned the four global workers before it started
        // serving. Mark the two observable ones as already up, so a test can tell
        // "the background server left them alone" from "nothing ever started".
        engine
            .changed_files_poller_started
            .store(true, Ordering::Relaxed);
        (engine, tmp)
    }

    fn loopback_listener() -> (std::net::TcpListener, std::net::SocketAddr) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral loopback bind");
        let addr = listener.local_addr().expect("bound address");
        (listener, addr)
    }

    /// Ask the server for its health endpoint over a real socket, so "is it
    /// serving?" is measured rather than inferred from a struct still existing.
    ///
    /// Hand-rolled over a `TcpStream` rather than through an HTTP client, because
    /// the two the workspace has are an async one and one this crate does not
    /// depend on, and a bare GET with no body is not worth either.
    fn healthz(addr: std::net::SocketAddr) -> Result<String, String> {
        use std::io::{Read, Write};

        let timeout = std::time::Duration::from_secs(3);
        let mut stream = std::net::TcpStream::connect_timeout(&addr, timeout)
            .map_err(|e| format!("connect failed: {e}"))?;
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|e| e.to_string())?;
        write!(
            stream,
            "GET /healthz HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
        )
        .map_err(|e| format!("write failed: {e}"))?;
        let mut response = String::new();
        stream
            .read_to_string(&mut response)
            .map_err(|e| format!("read failed: {e}"))?;
        response
            .lines()
            .next()
            .map(|line| line.to_string())
            .ok_or_else(|| "the server closed without answering".to_string())
    }

    /// A start/stop/start cycle: the second serve is a whole new app, and the
    /// first one's listener is genuinely gone rather than still accepting.
    ///
    /// The runtime is the reaper for everything `build_app` spawns, so this is
    /// also what stops a toggle cycle from leaving a second changed-files poller
    /// or a second event-bus forwarder running beside the first.
    #[test]
    fn a_toggle_cycle_stops_serving_and_starts_a_fresh_app() {
        let (mut engine, _tmp) = engine_in_tempdir();

        let (listener, first_addr) = loopback_listener();
        let server = BackgroundServer::start(
            &mut engine,
            vec![listener],
            vec![format!("http://{first_addr}")],
        )
        .expect("the first serve starts");
        let status = healthz(first_addr).expect("the first serve answers");
        assert!(
            status.contains("200"),
            "a started background server must answer on its own address, got {status:?}"
        );
        assert!(server.stop().is_none(), "a clean stop records no failure");
        assert!(
            healthz(first_addr).is_err(),
            "the stopped serve must not still be accepting on {first_addr}"
        );

        // Toggling back on builds a fresh app rather than reviving the old one.
        let (listener, second_addr) = loopback_listener();
        let server = BackgroundServer::start(
            &mut engine,
            vec![listener],
            vec![format!("http://{second_addr}")],
        )
        .expect("the second serve starts");
        let status = healthz(second_addr).expect("the second serve answers");
        assert!(
            status.contains("200"),
            "toggling back on must serve again, got {status:?}"
        );
        assert!(server.stop().is_none());
    }

    /// The teardown flag must be set before anything waits on the runtime.
    ///
    /// The PTY forwarders park inside a blocking `recv_timeout` on channels the
    /// engine still owns, and `spawn_blocking` tasks cannot be aborted. Without
    /// the flag the teardown blocks on tasks that will never notice, and the
    /// caller here is a terminal UI in the middle of a keystroke.
    #[test]
    fn stopping_trips_the_teardown_flag_before_dropping_the_runtime() {
        let (mut engine, _tmp) = engine_in_tempdir();
        let (listener, addr) = loopback_listener();
        let server =
            BackgroundServer::start(&mut engine, vec![listener], vec![format!("http://{addr}")])
                .expect("serve starts");
        let flag = std::sync::Arc::clone(&server.shutdown_flag);
        assert!(
            !flag.load(Ordering::SeqCst),
            "the flag starts clear while serving"
        );
        server.stop();
        assert!(
            flag.load(Ordering::SeqCst),
            "stopping must trip the teardown flag, or a parked forwarder wedges the drop"
        );
    }

    /// Two sets of handlers for one signal is a race over who tears down what, so
    /// the background serve installs none: the terminal UI's own handlers own the
    /// process and its quit is what stops the serve.
    #[test]
    fn the_background_serve_installs_no_signal_handlers() {
        let (mut engine, _tmp) = engine_in_tempdir();
        let (listener, addr) = loopback_listener();
        let server =
            BackgroundServer::start(&mut engine, vec![listener], vec![format!("http://{addr}")])
                .expect("serve starts");
        assert!(
            !server.installed_signal_handlers(),
            "the background serve must leave SIGINT/SIGTERM to the terminal UI"
        );
        server.stop();
    }

    /// Starting must not re-run the global background workers. `App::run` already
    /// spawned them and is still running; a second spawn here would be this serve
    /// making a claim about the other surface's lifecycle.
    #[test]
    fn starting_does_not_respawn_the_global_workers() {
        let (mut engine, _tmp) = engine_in_tempdir();
        // Left deliberately clear: if `start` called `spawn_global_workers`, this
        // is the flag that would flip.
        engine
            .branch_sync_worker_started
            .store(false, Ordering::Relaxed);
        let (listener, addr) = loopback_listener();
        let server =
            BackgroundServer::start(&mut engine, vec![listener], vec![format!("http://{addr}")])
                .expect("serve starts");
        assert!(
            !engine.branch_sync_worker_started.load(Ordering::Relaxed),
            "the background serve must not spawn the global workers a second time"
        );
        server.stop();
    }

    /// Connection ids keep climbing across a toggle cycle. The registries are per
    /// serve, so a per-registry counter would hand cycle two the ids cycle one
    /// used, and the ghost self-succession rule compares raw ids.
    #[test]
    fn conn_ids_stay_disjoint_across_a_toggle_cycle() {
        let (mut engine, _tmp) = engine_in_tempdir();

        let (listener, addr) = loopback_listener();
        let server =
            BackgroundServer::start(&mut engine, vec![listener], vec![format!("http://{addr}")])
                .expect("first serve");
        let first = crate::pty_owners::PtySizeOwners::default().next_conn_id();
        server.stop();

        let (listener, addr) = loopback_listener();
        let server =
            BackgroundServer::start(&mut engine, vec![listener], vec![format!("http://{addr}")])
                .expect("second serve");
        let second = crate::pty_owners::PtySizeOwners::default().next_conn_id();
        server.stop();

        assert!(
            second > first,
            "a second cycle must issue fresh ids ({first} then {second})"
        );
    }
}
