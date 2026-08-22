//! The web server, serving in the background of a live terminal UI.
//!
//! ## What this is
//!
//! The third serve path, beside `dux server` (a blocking CLI entry that owns the
//! process) and the flip (which swaps the terminal UI out for a status screen).
//! Here both surfaces are up at once: the terminal UI keeps the engine and keeps
//! its run loop, and lends the engine to this type once per iteration.
//!
//! ## What it does NOT do
//!
//! It does not drain worker events, and it does not run any of the shared
//! maintenance sweeps. Those have exactly one runner per process, and while this
//! is serving that runner is the terminal UI. What is left is the genuinely
//! web-only work: resolving pending PTY subscribes, checking the spine
//! fingerprint, draining queued engine requests, and retiring timed-out statuses.
//! [`crate::engine_actor::EngineService`] holds both halves and this type calls
//! only the second.
//!
//! It also installs no signal handlers. The terminal UI's handlers own the
//! process's SIGINT/SIGTERM, and its quit path is what stops this serve.
//!
//! ## Accepted: the engine's surface kind stays `Tui`
//!
//! `Engine::surface_kind` decides how a launched agent resolves `auto`/`mirror`
//! terminal identity, and it stays `Tui` here even for an agent a browser
//! launched. The flip sets `WebHeadless` because after a flip there IS no terminal
//! to resolve against; that is the case the headless identity exists for. Here a
//! real terminal is running, and resolving against it is the better answer of the
//! two available: the alternative would give every locally-launched agent a forced
//! identity for the sake of the remote ones. A per-launch surface kind would fix
//! it properly and is not worth the reach in this phase.
//!
//! ## Accepted: the terminal UI's statuses stay on the terminal
//!
//! A status the terminal UI sets by hand (a keystroke's confirmation, a refusal)
//! goes to its own status line only. What DOES reach browsers is every status
//! carried by a drained worker event, because those flow through the seam: so the
//! final of any operation a worker completes is seen on both surfaces, while the
//! chatter of driving the terminal is not. Web-originated statuses are unaffected
//! and stay scoped per connection, decided inside `handle_request` before this
//! type is ever involved.

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
    /// be spotted without every one of the terminal UI's apply sites having to
    /// announce itself.
    last_command_applies: u64,
    /// This serve's PTY-ownership registry and the terminal UI's seat in it.
    ///
    /// The seat is taken at START rather than at the terminal UI's first
    /// keystroke, because it must exist before anything can be gated by it and
    /// because taking an id costs an atomic increment. It dies with the serve,
    /// which is what makes "the terminal UI releases everything on stop" true by
    /// construction: the registry itself is gone.
    ownership: dux_core::background_serve::TuiOwnership,
    /// The two buses this serve announces the terminal UI's ownership changes on.
    ///
    /// A `OnceLock` because `build_app` is what can fill it (both buses are born
    /// in there) and it runs inside this constructor. Empty is survivable rather
    /// than fatal: nothing is announced, browsers fall back to the fingerprint
    /// backstop and the handshake, and the terminal UI keeps working.
    publisher: Arc<std::sync::OnceLock<crate::ownership_publish::OwnershipPublisher>>,
    /// How many browser tabs are connected to this serve, kept up to date by the
    /// connection registry inside the router.
    ///
    /// An atomic rather than a question asked of the actor: the terminal UI reads
    /// it once per rendered frame, from the thread that is also servicing this
    /// serve, so it has to be a load and nothing more. It dies with the serve,
    /// which is why "not serving" reports zero without anybody deciding it.
    connections: Arc<AtomicUsize>,
}

impl BackgroundServer {
    /// Start serving `engine` on `listeners`, which the CALLER already bound.
    ///
    /// Bound by the caller on purpose: a bind failure then happens while the
    /// terminal UI is fully up and nothing has been handed over, so it is a
    /// message on a status line rather than a half-torn-down process. By the time
    /// this runs the addresses are ours.
    pub fn start(
        engine: &mut Engine,
        listeners: Vec<std::net::TcpListener>,
        urls: Vec<String>,
    ) -> Result<Self> {
        crate::warn_if_ui_not_built();
        // The terminal UI owns this terminal. A stdout console would print serve
        // lifecycle lines straight over its frame, so this one writes nowhere.
        // Not `capture` either: nothing on this path reads an activity ring (the
        // flip's status screen is the only consumer, and it is not up here).
        let console = Console::noop();

        // The terminal UI's own `App::run` already spawned the four global
        // background workers, and it is still running. Asserted rather than left
        // to the spawn helpers' individual idempotence: calling them here would be
        // a claim about the OTHER surface's lifecycle, and a future non-idempotent
        // worker would then quietly double.
        debug_assert!(
            engine
                .changed_files_poller_started
                .load(std::sync::atomic::Ordering::Relaxed),
            "the terminal UI must already have spawned the global workers; the background \
             server must not spawn them a second time"
        );

        let (handle, ends) = build_actor_channels(engine);
        let shutdown_flag = handle.shutdown_flag();
        // The terminal UI's seat in this serve's registry. Its connection id comes
        // from the same process-global counter every browser socket draws from, so
        // the two can be compared and can never collide.
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

    /// How many browser tabs are connected to this serve right now.
    ///
    /// "Connections", not "devices": one browser with two tabs open is two, and
    /// there is no honest way from here to tell that they are the same laptop.
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

    /// Stop serving and release everything, bounded. Returns the first listener
    /// failure if one was recorded.
    ///
    /// Consumes `self` because stopping IS dropping: the runtime is what reaps
    /// every task `build_app` spawned, so a serve that could be "stopped" and kept
    /// would be a serve whose pollers were still running.
    pub fn stop(self) -> Option<anyhow::Error> {
        self.core.stop(&self.shutdown_flag)
    }

    /// Do the web layer's share of ONE reaction the terminal UI drained, before
    /// the terminal UI applies it.
    pub fn on_reaction(&mut self, engine: &mut Engine, reaction: &EventReaction) {
        // ByOrigin, not RunEverything: the terminal UI drained this reaction and
        // holds its own arm for it, so the routable follow-ups run on exactly one
        // surface. See `Engine::followup_owner`.
        self.service
            .fanout_reaction(engine, reaction, FollowupRouting::ByOrigin);
        // The one follow-up the terminal UI's reload arm has never fired: telling
        // browsers that config-static state changed. Read-only here; the drainer
        // still owns adopting the config.
        self.service.announce_config_reload(engine, reaction);
        // A drained worker event can insert a provider, flip a session status, or
        // apply a project mutation, all of it spine state. Bump unconditionally;
        // the fingerprint compare stays the precise emit gate.
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
    /// browser's own claim announces on.
    ///
    /// A missing publisher is logged once per batch rather than swallowed: it
    /// means `build_app` never filled the slot, which is a wiring bug, and the
    /// visible symptom (browsers never learn the terminal UI took over) is one
    /// nobody would trace back here. Near-dead in practice, because `build_app`
    /// fills the slot synchronously before the terminal UI can reach this; it is
    /// here so a future wiring change that stops doing that says so in the log
    /// rather than going quiet.
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

    /// Open the spine-change gate when the terminal UI applied anything since the
    /// last iteration.
    ///
    /// The terminal UI applies commands straight to the engine, over channels the
    /// web layer never sees, so the request drain's own `request_mutates_spine`
    /// answers cannot notice them. Without this a browser waited for the ~2s
    /// fingerprint backstop after every action taken at the keyboard.
    /// Deliberately conservative: any apply opens the gate, and the fingerprint
    /// compare decides whether anything is actually emitted.
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
