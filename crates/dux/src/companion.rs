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

use dux_core::background_serve::{
    BackgroundServeCompanion, DrainedMaintenance, PtyOwnershipEvent, ServiceOutcome, TuiOwnership,
};
use dux_core::engine::{Engine, EventReaction};
use dux_web::background::BackgroundServer;

/// Holds the background web server, if one is running.
#[derive(Default)]
pub struct WebCompanion {
    server: Option<BackgroundServer>,
}

impl WebCompanion {
    pub fn new() -> Self {
        Self::default()
    }

    /// Drop a serve whose required listener died, so the TUI stops servicing a
    /// server that has stopped answering and says so once rather than every
    /// iteration.
    ///
    /// Returns the sentence for the user when it retired something. The log line
    /// on its own was not enough: the last thing the status line said was
    /// "serving on ...", and nobody reads `dux.log` to find out that stopped
    /// being true.
    fn retire_if_failed(&mut self) -> Option<String> {
        let failed = self.server.as_ref().is_some_and(|s| s.is_failed());
        if !failed {
            return None;
        }
        let server = self.server.take()?;
        let error = server.stop();
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
        outcome.retirement = self.retire_if_failed();
        outcome
    }

    fn note_engine_activity(&mut self, command_applies: u64) {
        if let Some(server) = self.server.as_mut() {
            server.note_engine_activity(command_applies);
        }
    }

    fn is_serving(&self) -> bool {
        self.server.is_some()
    }

    fn urls(&self) -> Vec<String> {
        self.server
            .as_ref()
            .map(|s| s.urls().to_vec())
            .unwrap_or_default()
    }

    fn connections(&self) -> usize {
        // No serve, no connections: the count is structurally zero rather than
        // remembered from last time.
        self.server.as_ref().map_or(0, |s| s.connections())
    }

    fn start(
        &mut self,
        engine: &mut Engine,
        listeners: Vec<std::net::TcpListener>,
        urls: Vec<String>,
    ) -> Result<Vec<String>, String> {
        if self.server.is_some() {
            return Err("The web UI is already serving in the background.".to_string());
        }
        // The listeners are already bound, so a failure here is a runtime or
        // adoption problem rather than a busy port; either way nothing has been
        // taken away from the terminal UI, and dropping `listeners` with the error
        // releases the addresses again.
        match BackgroundServer::start(engine, listeners, urls) {
            Ok(server) => {
                let urls = server.urls().to_vec();
                self.server = Some(server);
                Ok(urls)
            }
            Err(err) => Err(format!("Could not start the web server: {err:#}")),
        }
    }

    fn ownership(&self) -> Option<TuiOwnership> {
        self.server.as_ref().map(|server| server.ownership())
    }

    fn publish_ownership_events(&mut self, events: &[PtyOwnershipEvent]) {
        if let Some(server) = self.server.as_mut() {
            server.publish_ownership_events(events);
        }
    }

    fn stop(&mut self, _engine: &mut Engine) {
        let Some(server) = self.server.take() else {
            return;
        };
        // Stopping trips the PTY forwarders' teardown flag before it waits on
        // anything, then reaps the legs and the runtime under bounded timeouts.
        if let Some(err) = server.stop() {
            dux_core::logger::warn(&format!(
                "[server] the background web server had already stopped serving before it was \
                 turned off: {err:#}"
            ));
        }
    }
}
