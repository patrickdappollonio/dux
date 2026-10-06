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
    /// `None` for a core that serves only the control socket, which serves no
    /// terminal and so gives the terminal UI no seat.
    ownership: Option<dux_core::background_serve::TuiOwnership>,
    /// The two buses this serve announces the terminal UI's ownership changes on,
    /// filled by `build_app`. Empty is survivable: nothing is announced and
    /// browsers fall back to the fingerprint backstop and the handshake.
    publisher: Arc<std::sync::OnceLock<crate::ownership_publish::OwnershipPublisher>>,
    /// How many browser tabs are connected, kept up to date by the router's
    /// connection registry. An atomic because the terminal UI reads it once per
    /// rendered frame from the thread that also services this serve.
    connections: Arc<AtomicUsize>,
    /// The file this serve's console writes, which a reload of `[server]
    /// log_path` does not move for as long as this serve runs.
    log_path: Option<std::path::PathBuf>,
}

/// How long a hand-over waits for the control socket's accepted requests to be
/// answered before the core stops anyway.
const HAND_OVER_BOUND: std::time::Duration = std::time::Duration::from_secs(3);

/// Choose between the live address list and the one captured at start.
///
/// `None` is the registry saying it could not answer; an empty list is it saying
/// there is nothing to answer with, and only the first is worth papering over.
fn urls_to_show(live: Option<Vec<String>>, captured: &[String]) -> Vec<String> {
    live.unwrap_or_else(|| captured.to_vec())
}

impl BackgroundServer {
    /// Start serving `engine` on `listeners`, which the caller already bound: a
    /// bind failure is then a status line message, not a half-torn-down process.
    ///
    /// With `claim_before_serving`, every running pty nobody drives is claimed
    /// for the terminal UI before any listener accepts a connection, and the
    /// claims are announced once the serve is up. See
    /// [`dux_core::background_serve::BackgroundServeCompanion::start`].
    pub fn start(
        engine: &mut Engine,
        listeners: Vec<std::net::TcpListener>,
        urls: Vec<String>,
        claim_before_serving: bool,
        startup: dux_core::serve_log::StartupNotes,
    ) -> Result<Self> {
        crate::warn_if_ui_not_built();
        // Prints nothing over the terminal UI's frame, but keeps `server.log`:
        // the lines `dux server` prints, written to the file, access lines
        // included when `[server] access_log` is on.
        let server_log = crate::open_server_log(&engine.config, &engine.paths);
        let log_path = server_log.as_ref().map(|log| log.path().to_path_buf());
        let console = match server_log {
            Some(log) => Console::server_log_only(log),
            None => Console::noop(),
        };
        // What `dux server` opens its log with: the warnings raised before
        // binding, then the banner with the version and what actually bound.
        for warning in &startup.warnings {
            console.warn(warning);
        }
        let legs: Vec<(std::net::SocketAddr, bool)> = listeners
            .iter()
            .filter_map(|listener| listener.local_addr().ok())
            .map(|addr| (addr, addr.ip().is_loopback()))
            .collect();
        console.banner(&crate::serve_banner(
            dux_core::display_version(),
            &legs,
            &startup.bind_warnings,
            engine.config.server.tailscale_mode(),
            startup.tailscale_detected,
            &engine.config.server.auth,
        ));

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
        // Seeded BEFORE any leg is spawned below: nothing can be connected yet,
        // so no browser tab that is already reconnecting can win a plain-attach
        // claim first, and every handshake reads the terminal UI as the owner.
        // Announced once the serve is up and its publisher exists.
        let seeded_claims = if claim_before_serving {
            ownership.claim_every_running_pty(engine)
        } else {
            Vec::new()
        };
        let publisher = Arc::new(std::sync::OnceLock::new());
        let connections = Arc::new(AtomicUsize::new(0));
        let service = EngineService::new(engine, ends, ShutdownEcho::Silent);
        let core = ServeCore::start(
            handle,
            listeners,
            crate::control_socket::listener_of(engine),
            &engine.config,
            console,
            // Gated by the console too: with no `server.log` to write, the console
            // records nothing and a request line costs nothing.
            engine.config.server.access_log,
            SignalPolicy::Inherited,
            crate::BackgroundHooks {
                ownership_publisher: Some(Arc::clone(&publisher)),
                connections_gauge: Some(Arc::clone(&connections)),
            },
        )?;
        let mut server = Self {
            core,
            service,
            shutdown_flag,
            urls,
            last_command_applies: engine.command_applies,
            ownership: Some(ownership),
            publisher,
            connections,
            log_path,
        };
        server.publish_ownership_events(&seeded_claims);
        Ok(server)
    }

    /// Serve only the control socket `engine`'s lock holds: the plain terminal
    /// UI's core, so a command-line client can reach a dux that serves no web
    /// UI. `None` when dux runs without the socket. It claims no terminal,
    /// counts no connection and serves nothing a browser could open.
    pub fn start_control_only(engine: &mut Engine) -> Result<Option<Self>> {
        let Some(control) = crate::control_socket::listener_of(engine) else {
            return Ok(None);
        };
        let (handle, ends) = build_actor_channels(engine);
        let shutdown_flag = handle.shutdown_flag();
        let service = EngineService::new(engine, ends, ShutdownEcho::Silent);
        let core = ServeCore::start_control_only(handle, control, &engine.config)?;
        Ok(Some(Self {
            core,
            service,
            shutdown_flag,
            urls: Vec::new(),
            last_command_applies: engine.command_applies,
            ownership: None,
            publisher: Arc::new(std::sync::OnceLock::new()),
            connections: Arc::new(AtomicUsize::new(0)),
            log_path: None,
        }))
    }

    /// Whether this serves the web UI, rather than only the control socket.
    pub fn serves_web(&self) -> bool {
        self.ownership.is_some()
    }

    /// Stop serving, first letting the control socket finish the requests it
    /// already accepted, servicing `engine` meanwhile (bounded). A connection
    /// that arrives from then on waits in the socket's backlog for the next
    /// core, so moving between cores loses no command-line request.
    pub fn hand_over(mut self, engine: &mut Engine) -> Option<anyhow::Error> {
        self.core.stop_control_socket();
        let deadline = std::time::Instant::now() + HAND_OVER_BOUND;
        while !self.core.control_socket_drained() && std::time::Instant::now() < deadline {
            self.service(engine);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        self.stop()
    }

    /// The log file this serve opened, `None` when it could not open one.
    pub fn server_log_path(&self) -> Option<std::path::PathBuf> {
        self.log_path.clone()
    }

    /// How many browser tabs are connected to this serve right now. Connections,
    /// not devices: two tabs of one browser count as two.
    pub fn connections(&self) -> usize {
        self.connections.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// The addresses this serve is reachable on right now.
    ///
    /// Read from the live leg registry, not from the list the caller bound and
    /// handed over: the Tailscale leg comes and goes underneath a running serve,
    /// and the terminal UI shows these addresses for as long as it serves. The
    /// captured list is the fallback for the one case where the registry COULD
    /// NOT BE READ, and for that case only: a serve whose legs have all gone is
    /// reachable nowhere, and answering it with the addresses it bound at start
    /// puts a dead address on screen.
    pub fn urls(&self) -> Vec<String> {
        urls_to_show(self.core.live_urls(), &self.urls)
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

    /// This serve's live Tailscale-mode handle, for a test that moves its leg.
    #[cfg(test)]
    pub(crate) fn tailscale_mode_control(&self) -> crate::serve_legs::TailscaleModeControl {
        self.core.tailscale_mode()
    }

    /// The terminal UI's seat in this serve's PTY-ownership registry, or
    /// `None` for a core that serves only the control socket.
    pub fn ownership(&self) -> Option<dux_core::background_serve::TuiOwnership> {
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
    pub fn note_config_applied(&mut self, config: &dux_core::config::Config) {
        self.service.note_config_applied(config);
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
            socket_path: tmp.path().join("dux.sock"),
        };
        std::fs::create_dir_all(&paths.worktrees_root).expect("worktrees dir");
        let engine = crate::test_support::bootstrap_test_engine(&paths).expect("engine");
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
        get(addr, "/healthz")
    }

    /// The status line of a bare GET of `path`.
    fn get(addr: std::net::SocketAddr, path: &str) -> Result<String, String> {
        use std::io::{Read, Write};

        let timeout = std::time::Duration::from_secs(3);
        let mut stream = std::net::TcpStream::connect_timeout(&addr, timeout)
            .map_err(|e| format!("connect failed: {e}"))?;
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|e| e.to_string())?;
        write!(
            stream,
            "GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
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

    /// Send `GET <uri>` over the control socket at `path` on its own thread,
    /// so it can be sent before anything serves the socket.
    fn request_over_socket(
        path: &std::path::Path,
        uri: &str,
    ) -> std::thread::JoinHandle<Result<String, String>> {
        use std::io::{Read, Write};
        let path = path.to_path_buf();
        let uri = uri.to_string();
        std::thread::spawn(move || {
            let mut stream = std::os::unix::net::UnixStream::connect(&path)
                .map_err(|e| format!("connect failed: {e}"))?;
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(15)))
                .map_err(|e| e.to_string())?;
            write!(
                stream,
                "GET {uri} HTTP/1.1\r\nHost: dux\r\nConnection: close\r\n\r\n"
            )
            .map_err(|e| format!("write failed: {e}"))?;
            let mut response = String::new();
            stream
                .read_to_string(&mut response)
                .map_err(|e| format!("read failed: {e}"))?;
            Ok(response)
        })
    }

    /// Service `server` the way the terminal UI's loop does until `request`
    /// has its answer.
    fn serviced_until_answered(
        server: &mut BackgroundServer,
        engine: &mut dux_core::engine::Engine,
        request: std::thread::JoinHandle<Result<String, String>>,
    ) -> String {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while !request.is_finished() && std::time::Instant::now() < deadline {
            server.service(engine);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        request
            .join()
            .expect("the request thread")
            .expect("the request was answered")
    }

    fn socket_inode(path: &std::path::Path) -> u64 {
        use std::os::unix::fs::MetadataExt;
        std::fs::symlink_metadata(path).expect("the socket").ino()
    }

    /// The address list the terminal UI shows has to follow the legs, not the
    /// snapshot the flip handed over: the Tailscale leg comes and goes under a
    /// running serve, and a remembered list keeps naming an address that stopped
    /// answering (or misses one that started).
    #[test]
    fn the_address_list_is_read_from_the_live_legs() {
        let (mut engine, _tmp) = engine_in_tempdir();
        let (listener, addr) = loopback_listener();
        let server = BackgroundServer::start(
            &mut engine,
            vec![listener],
            vec!["http://stale.example:1".to_string()],
            false,
            dux_core::serve_log::StartupNotes::default(),
        )
        .expect("the serve starts");

        assert_eq!(
            server.urls(),
            vec![format!("http://{addr}")],
            "the live leg wins over the list start was handed"
        );

        server.stop();
    }

    /// A start/stop/start cycle: the second serve is a whole new app, and the
    /// first one's listener is genuinely gone rather than still accepting.
    ///
    /// The runtime is the reaper for everything `build_app` spawns, so this is
    /// also what stops a toggle cycle from leaving a second changed-files poller
    /// or a second event-bus forwarder running beside the first.
    ///
    /// The control socket rides through it: the plain terminal UI's core serves
    /// it alone, each serve takes it over, and a request sent while nothing
    /// serves it is answered by whichever core comes next, on the same socket.
    #[test]
    fn a_toggle_cycle_stops_serving_and_starts_a_fresh_app() {
        let (mut engine, tmp) = engine_in_tempdir();
        // Not a test about Tailscale: on any other mode every request waits on
        // the first Funnel check, which would consult this machine's real CLI.
        engine.config.server.tailscale = "no".to_string();
        // The config folder is owner-only, and the control socket goes nowhere else.
        std::fs::set_permissions(
            &engine.paths.root,
            <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
        )
        .unwrap();
        let socket = engine.paths.root.join("dux.sock");
        assert_eq!(
            dux_core::control_socket::open(&mut engine.single_instance_lock, &socket),
            None
        );
        let bound = socket_inode(&socket);

        let mut plain = BackgroundServer::start_control_only(&mut engine)
            .expect("the socket core starts")
            .expect("dux holds a control socket");
        assert!(!plain.serves_web());
        assert!(plain.ownership().is_none(), "the socket core claims no pty");
        let answer = serviced_until_answered(
            &mut plain,
            &mut engine,
            request_over_socket(&socket, "/api/v1/workspace"),
        );
        assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
        assert!(answer.contains("\"projects\""), "{answer}");
        assert!(plain.hand_over(&mut engine).is_none());
        let waiting = request_over_socket(&socket, "/api/v1/workspace");
        engine.config.server.access_log = true;

        let (listener, first_addr) = loopback_listener();
        let mut server = BackgroundServer::start(
            &mut engine,
            vec![listener],
            vec![format!("http://{first_addr}")],
            false,
            dux_core::serve_log::StartupNotes {
                warnings: vec!["Tailscale not detected (test).".to_string()],
                ..Default::default()
            },
        )
        .expect("the first serve starts");
        let answer = serviced_until_answered(&mut server, &mut engine, waiting);
        assert!(
            answer.starts_with("HTTP/1.1 200"),
            "a request sent between cores is answered by the next: {answer}"
        );
        assert_eq!(socket_inode(&socket), bound, "the same bound socket");
        let status = healthz(first_addr).expect("the first serve answers");
        assert!(
            status.contains("200"),
            "a started background server must answer on its own address, got {status:?}"
        );
        get(first_addr, "/api/v1/build").expect("the first serve answers a page request");
        assert_eq!(
            server.server_log_path(),
            Some(tmp.path().join("server.log")),
            "the serve says which file it opened"
        );
        assert!(
            server.hand_over(&mut engine).is_none(),
            "a clean stop records no failure"
        );
        assert!(
            healthz(first_addr).is_err(),
            "the stopped serve must not still be accepting on {first_addr}"
        );
        let waiting = request_over_socket(&socket, "/api/v1/workspace");
        let mut plain = BackgroundServer::start_control_only(&mut engine)
            .expect("the socket core starts again")
            .expect("dux still holds its control socket");
        let answer = serviced_until_answered(&mut plain, &mut engine, waiting);
        assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
        assert_eq!(socket_inode(&socket), bound, "the same bound socket");
        assert!(plain.hand_over(&mut engine).is_none());

        // Toggling back on builds a fresh app rather than reviving the old one.
        engine.config.server.access_log = false;
        let (listener, second_addr) = loopback_listener();
        let server = BackgroundServer::start(
            &mut engine,
            vec![listener],
            vec![format!("http://{second_addr}")],
            false,
            dux_core::serve_log::StartupNotes::default(),
        )
        .expect("the second serve starts");
        let status = healthz(second_addr).expect("the second serve answers");
        assert!(
            status.contains("200"),
            "toggling back on must serve again, got {status:?}"
        );
        get(second_addr, "/api/v1/build").expect("the second serve answers a page request");
        assert!(server.stop().is_none());

        // Serving in the background prints nothing over the terminal UI, but the
        // file keeps the request lines of the serve that had `access_log` on, and
        // none of the one that had it off, and never the health probe's.
        let log = std::fs::read_to_string(tmp.path().join("server.log")).expect("server.log");
        let page_requests: Vec<&str> = log
            .lines()
            .filter(|line| {
                let (stamp, rest) = line.split_once(' ').expect("a dated line");
                chrono::DateTime::parse_from_rfc3339(stamp).expect("an RFC 3339 date");
                rest.len() > 9 && rest[9..].starts_with("GET /api/v1/build 200 ")
            })
            .collect();
        assert_eq!(page_requests.len(), 1, "{log}");
        assert!(!log.contains("/healthz"), "{log}");
        // Starting writes what `dux server` opens its log with: the warnings
        // raised before binding, then the banner with the version and addresses.
        let lines: Vec<&str> = log
            .lines()
            .map(|line| line.split_once(' ').expect("a dated line").1)
            .collect();
        assert!(
            lines
                .iter()
                .any(|l| l.len() > 9 && l[9..] == *"warn Tailscale not detected (test)."),
            "{log}"
        );
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("dux ") && l.ends_with("  plain HTTP")),
            "{log}"
        );
        assert!(
            lines.contains(&format!("  -> Local (loopback): http://{first_addr}").as_str()),
            "{log}"
        );
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
        let server = BackgroundServer::start(
            &mut engine,
            vec![listener],
            vec![format!("http://{addr}")],
            false,
            dux_core::serve_log::StartupNotes::default(),
        )
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
        let server = BackgroundServer::start(
            &mut engine,
            vec![listener],
            vec![format!("http://{addr}")],
            false,
            dux_core::serve_log::StartupNotes::default(),
        )
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
        let server = BackgroundServer::start(
            &mut engine,
            vec![listener],
            vec![format!("http://{addr}")],
            false,
            dux_core::serve_log::StartupNotes::default(),
        )
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
        let server = BackgroundServer::start(
            &mut engine,
            vec![listener],
            vec![format!("http://{addr}")],
            false,
            dux_core::serve_log::StartupNotes::default(),
        )
        .expect("first serve");
        let first = crate::pty_owners::PtySizeOwners::default().next_conn_id();
        server.stop();

        let (listener, addr) = loopback_listener();
        let server = BackgroundServer::start(
            &mut engine,
            vec![listener],
            vec![format!("http://{addr}")],
            false,
            dux_core::serve_log::StartupNotes::default(),
        )
        .expect("second serve");
        let second = crate::pty_owners::PtySizeOwners::default().next_conn_id();
        server.stop();

        assert!(
            second > first,
            "a second cycle must issue fresh ids ({first} then {second})"
        );
    }

    /// An empty live list is an answer: every leg has gone, so the serve really
    /// is reachable nowhere and the addresses it bound at start are a dead link
    /// on screen.
    #[test]
    fn no_legs_left_shows_no_addresses_rather_than_the_ones_bound_at_start() {
        let captured = vec!["http://127.0.0.1:8080".to_string()];
        assert!(super::urls_to_show(Some(Vec::new()), &captured).is_empty());
        assert_eq!(
            super::urls_to_show(Some(vec!["http://100.64.0.5:8080".to_string()]), &captured),
            vec!["http://100.64.0.5:8080".to_string()],
            "a live list is the answer whenever there is one"
        );
    }

    /// And the fallback is for the registry that could not answer at all, which
    /// is the one case where a header would otherwise go blank for no reason.
    #[test]
    fn an_unreadable_registry_falls_back_to_the_captured_list() {
        let captured = vec!["http://127.0.0.1:8080".to_string()];
        assert_eq!(super::urls_to_show(None, &captured), captured);
    }

    /// The registry itself tells those two apart: a poisoned lock answers
    /// `None`, an empty registry answers an empty list.
    #[test]
    fn a_poisoned_leg_registry_answers_that_it_could_not_read() {
        let shutdown = crate::serve_legs::ServeShutdown::for_watched(false);
        assert_eq!(
            shutdown.leg_addrs(),
            Some(Vec::new()),
            "an empty registry knows it is empty"
        );

        let poisoner = shutdown.clone();
        let _ = std::thread::spawn(move || {
            poisoner.poison_legs_for_test();
        })
        .join();
        assert_eq!(shutdown.leg_addrs(), None);
    }
}
