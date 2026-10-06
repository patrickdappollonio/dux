//! Where the two surfaces meet.
//!
//! `dux-tui` sees only `dux-core`; `dux-web` never sees `dux-tui`. This binary is
//! the one crate that depends on both, so it is the only place a terminal UI can
//! be handed something that serves HTTP. The seam is a `dux-core` trait
//! ([`BackgroundServeCompanion`]) that the TUI calls and this module implements
//! over `dux-web`'s [`BackgroundServer`].
//!
//! The whole implementation is bookkeeping around an `Option`. The TUI decides
//! WHEN to serve (its palette commands, its config, its quit path) and binds the
//! listeners; this decides nothing and only relays.
//!
//! The one core it holds is either a web serve or, while nothing serves the
//! web, a core that serves only the control socket. Moving between the two
//! hands the socket over: the old core finishes what it accepted, and anything
//! that arrives meanwhile waits in the socket's backlog for the new one.

use dux_core::background_serve::{
    BackgroundServeCompanion, DrainedMaintenance, PtyOwnershipEvent, ServiceOutcome, TuiOwnership,
};
use dux_core::engine::{Engine, EventReaction};
use dux_web::background::BackgroundServer;

/// Holds the background web server, or the control socket's own core, when
/// either is running.
#[derive(Default)]
pub struct WebCompanion {
    server: Option<BackgroundServer>,
}

impl WebCompanion {
    pub fn new() -> Self {
        Self::default()
    }

    /// The core, when it serves the web.
    fn serving(&self) -> Option<&BackgroundServer> {
        self.server.as_ref().filter(|server| server.serves_web())
    }

    /// The core, when it serves the web.
    fn serving_mut(&mut self) -> Option<&mut BackgroundServer> {
        self.server.as_mut().filter(|server| server.serves_web())
    }

    /// Stop the core there is, letting the control socket finish what it
    /// already accepted.
    fn hand_over(&mut self, engine: &mut Engine) {
        let Some(server) = self.server.take() else {
            return;
        };
        if let Some(err) = server.hand_over(engine) {
            dux_core::logger::warn(&format!(
                "[server] the background web server had already stopped serving before it was \
                 turned off: {err:#}"
            ));
        }
    }

    /// Drop a serve whose required listener died, so the TUI stops servicing a server
    /// that has stopped answering and says so once rather than every iteration.
    /// Returns the sentence for the user when it retired something: the status line
    /// last said "serving on ...", and nobody reads `dux.log` to learn that stopped
    /// being true.
    fn retire_if_failed(&mut self, engine: &mut Engine) -> Option<String> {
        let failed = self.serving().is_some_and(|s| s.is_failed());
        if !failed {
            return None;
        }
        let server = self.server.take()?;
        let error = server.stop();
        // The control socket outlives the web serve that failed.
        self.start_control_socket(engine);
        let detail = error
            .map(|e| format!("{e:#}"))
            .unwrap_or_else(|| "the listener stopped accepting connections".to_string());
        dux_core::logger::error(&format!(
            "[server] the background web server stopped serving: {detail}. The terminal UI and \
             every agent are unaffected; use start-background-server to serve again."
        ));
        Some(format!(
            "The web UI stopped serving in the background: {detail}. Your agents and terminals \
             are untouched and still running here. Use start-background-server to serve again."
        ))
    }
}

impl BackgroundServeCompanion for WebCompanion {
    fn on_reaction(&mut self, engine: &mut Engine, reaction: &EventReaction) {
        if let Some(server) = self.server.as_mut() {
            server.on_reaction(engine, reaction);
        }
    }

    fn note_maintenance(&mut self, maintenance: &DrainedMaintenance) {
        if maintenance.is_empty() {
            return;
        }
        if let Some(server) = self.server.as_mut() {
            server.note_maintenance(maintenance);
        }
    }

    fn service(&mut self, engine: &mut Engine) -> ServiceOutcome {
        let mut outcome = match self.server.as_mut() {
            Some(server) => server.service(engine),
            None => ServiceOutcome::default(),
        };
        if outcome.stopped && self.server.as_ref().is_some_and(|s| !s.serves_web()) {
            // Restarting a socket core whose loop was asked to stop could ask
            // again forever, so it stays down and says so once.
            dux_core::logger::error(
                "[server] the control socket's core stopped serving: its request channel \
                 closed. Command-line clients cannot reach this dux until it restarts.",
            );
            if let Some(server) = self.server.take() {
                server.stop();
            }
            return outcome;
        }
        if outcome.stopped && self.server.is_some() {
            // The serve's request channel closed, or something asked its loop to
            // stop. Nothing routine does that while a serve is up, so retiring it
            // is both the safe answer and the informative one: servicing a stopped
            // server every iteration forever would be a silent lie about what the
            // status line said.
            dux_core::logger::warn(
                "[server] the background web server's request channel closed, so it has stopped \
                 serving. The terminal UI and every agent are unaffected; use \
                 start-background-server to serve again.",
            );
            self.stop(engine);
            outcome.retirement = Some(
                "The web UI stopped serving in the background: its request channel closed. Your \
                 agents and terminals are untouched and still running here. Use \
                 start-background-server to serve again."
                    .to_string(),
            );
            return outcome;
        }
        // Checked after servicing rather than before, so the iteration that
        // noticed the death still drained whatever was queued.
        outcome.retirement = self.retire_if_failed(engine);
        outcome
    }

    fn note_config_applied(&mut self, config: &dux_core::config::Config) {
        if let Some(server_handle) = self.server.as_mut() {
            server_handle.note_config_applied(config);
        }
    }

    fn note_engine_activity(&mut self, command_applies: u64) {
        if let Some(server) = self.server.as_mut() {
            server.note_engine_activity(command_applies);
        }
    }

    fn set_tailscale_mode(&mut self, engine: &Engine, mode: dux_core::config::TailscaleMode) {
        if let Some(server) = self.serving() {
            server.set_tailscale_mode(mode, engine.worker_tx.clone());
        }
    }

    fn is_serving(&self) -> bool {
        self.serving().is_some()
    }

    fn has_core(&self) -> bool {
        self.server.is_some()
    }

    fn start_control_socket(&mut self, engine: &mut Engine) {
        if self.server.is_some() {
            return;
        }
        match BackgroundServer::start_control_only(engine) {
            Ok(core) => self.server = core,
            Err(err) => dux_core::logger::error(&format!(
                "[server] could not serve the control socket: {err:#}. Command-line clients \
                 cannot reach this dux until it restarts."
            )),
        }
    }

    fn release(&mut self, engine: &mut Engine) {
        self.hand_over(engine);
    }

    fn urls(&self) -> Vec<String> {
        self.serving().map(|s| s.urls()).unwrap_or_default()
    }

    fn connections(&self) -> usize {
        // No serve, no connections: the count is structurally zero rather than
        // remembered from last time.
        self.serving().map_or(0, |s| s.connections())
    }

    fn start(
        &mut self,
        engine: &mut Engine,
        listeners: Vec<std::net::TcpListener>,
        urls: Vec<String>,
        claim_before_serving: bool,
    ) -> Result<Vec<String>, String> {
        if self.is_serving() {
            return Err("The web UI is already serving in the background.".to_string());
        }
        // The control socket moves to the new serve.
        self.hand_over(engine);
        // The listeners are already bound, so a failure here is a runtime or
        // adoption problem rather than a busy port; either way nothing has been
        // taken away from the terminal UI, and dropping `listeners` with the error
        // releases the addresses again.
        match BackgroundServer::start(engine, listeners, urls, claim_before_serving) {
            Ok(server) => {
                let urls = server.urls();
                self.server = Some(server);
                Ok(urls)
            }
            Err(err) => {
                self.start_control_socket(engine);
                Err(format!("Could not start the web server: {err:#}"))
            }
        }
    }

    fn ownership(&self) -> Option<TuiOwnership> {
        self.server.as_ref().and_then(|server| server.ownership())
    }

    fn publish_ownership_events(&mut self, events: &[PtyOwnershipEvent]) {
        if let Some(server) = self.serving_mut() {
            server.publish_ownership_events(events);
        }
    }

    fn stop(&mut self, engine: &mut Engine) {
        if !self.is_serving() {
            return;
        }
        // Stopping trips the PTY forwarders' teardown flag before it waits on
        // anything, then reaps the legs and the runtime under bounded timeouts.
        self.hand_over(engine);
        self.start_control_socket(engine);
    }
}
