//! Turning the background web server on and off from the terminal UI, and
//! lending it the engine once per run-loop iteration.
//!
//! This is the TUI's half of the seam. The other half is a
//! [`dux_core::background_serve::BackgroundServeCompanion`] the `dux` binary
//! installs; this crate depends only on `dux-core` and never learns that a web
//! layer exists.
//!
//! The `start-web-server` flip is a DIFFERENT thing and is untouched: it swaps the
//! terminal UI out for the server. These commands keep both up at once.

use dux_core::engine::EventReaction;

use super::*;

impl App {
    /// Whether a listener is up right now.
    pub(crate) fn background_server_is_serving(&self) -> bool {
        self.companion
            .as_ref()
            .is_some_and(|companion| companion.is_serving())
    }

    /// The header chip that says a listener is up, or `None` when none is.
    ///
    /// Present whenever serving, connections or not: the chip's first job is to be
    /// the standing "you are running a listener" signal, and a chip that only
    /// appeared once somebody connected would hide exactly the state worth knowing
    /// about. The count is the second job.
    pub(crate) fn serving_chip(&self) -> Option<String> {
        let companion = self.companion.as_ref()?;
        if !companion.is_serving() {
            return None;
        }
        Some(serving_chip_label(
            serving_port_label(&companion.urls()).as_deref(),
            companion.connections(),
        ))
    }

    /// Lend the engine to the companion for ONE reaction, before this surface
    /// applies it.
    ///
    /// Pre-consume because `apply_reaction` takes the reaction by value, and
    /// per-reaction rather than per-batch so the companion sees them in the order
    /// they were drained. A no-op when nothing is serving.
    pub(crate) fn notify_companion(&mut self, reaction: &EventReaction) {
        if let Some(companion) = self.companion.as_mut()
            && companion.is_serving()
        {
            companion.on_reaction(&mut self.engine, reaction);
        }
    }

    /// Snapshot the ownership verdict source for reactions about to be drained.
    ///
    /// Taken BEFORE `notify_companion` lends anything to the web layer, because
    /// the web layer's own follow-ups REMOVE the pending-op entries the verdict is
    /// read from: ask afterwards and a browser's PR create or project add answers
    /// "the drainer owns this", and this surface runs its arm too. Empty when
    /// nothing is serving, which routes every reaction here.
    pub(crate) fn companion_routing(&self) -> CompanionRouting {
        if self.background_server_is_serving() {
            CompanionRouting::Serving(self.engine.web_followup_ops())
        } else {
            CompanionRouting::NotServing
        }
    }

    /// Hand the companion the outcomes of the maintenance sweeps this surface just
    /// ran, so browsers get the exit and close notices and the change gate they
    /// open.
    ///
    /// The sweeps have exactly one runner per process and while serving that
    /// runner is this surface, so without this lane a browser watching an agent
    /// die is told nothing at all and only notices the row vanish when the
    /// fingerprint backstop next fires.
    ///
    /// Handed over on every iteration while serving, empty sweeps included: the
    /// companion is the one that decides an empty hand-off is nothing to do (it
    /// does), and a caller-side skip would make "the drain reports its sweeps" a
    /// claim no test can pin.
    pub(crate) fn note_companion_maintenance(
        &mut self,
        maintenance: &dux_core::background_serve::DrainedMaintenance,
    ) {
        if let Some(companion) = self.companion.as_mut()
            && companion.is_serving()
        {
            companion.note_maintenance(maintenance);
        }
    }

    /// Lend the engine to the companion for its per-iteration work, and do this
    /// surface's own follow-up when the companion changed shared state.
    ///
    /// Called once per run-loop iteration. A no-op when nothing is serving.
    pub(crate) fn service_companion(&mut self) {
        let applies = self.engine.command_applies;
        // A web-owned follow-up ran during this iteration's drain, which can have
        // mutated the workspace (the inline project add writes `engine.projects`
        // from inside the fanout). This surface skipped its own arm for it, so
        // nothing here has rebuilt: fold it in and clear it whether or not
        // anything is serving, so the flag never survives into a later iteration.
        let followup_ran = std::mem::take(&mut self.companion_followup_ran);
        let outcome = match self.companion.as_mut() {
            Some(companion) if companion.is_serving() => {
                // Tell it what this surface did BEFORE it services, so a keystroke
                // here reaches a browser on the same iteration rather than waiting
                // for the fingerprint backstop.
                companion.note_engine_activity(applies);
                companion.service(&mut self.engine)
            }
            _ => return,
        };
        if outcome.mutated || followup_ran {
            self.refresh_after_companion_mutation();
        }
        // The serve retired itself (a required listener died, or its request
        // channel closed). Say so where the user is looking: the last thing the
        // status line told them was the address it was serving on.
        if let Some(message) = outcome.retirement {
            self.status.set(
                Instant::now(),
                Some(BACKGROUND_SERVER_STATUS_KEY.to_string()),
                StatusTone::Warning,
                message,
            );
        }
    }

    /// Re-derive this surface's view state after the companion changed shared
    /// state: an agent renamed, reordered or deleted from a browser, a project
    /// added, a terminal closed.
    ///
    /// Without this the sidebar keeps showing the old name indefinitely, because
    /// nothing on this surface has any reason to rebuild: the change did not come
    /// through this surface's own event stream.
    fn refresh_after_companion_mutation(&mut self) {
        self.sync_view_state_from_config();
        self.rebuild_left_items();
        if self.selected_left >= self.left_items_cache.len() {
            self.selected_left = self.left_items_cache.len().saturating_sub(1);
        }
        self.clamp_terminal_cursor();
        self.clamp_files_cursor();
        // The entity under the cursor may be gone. Leaving interactive input
        // pointed at a vanished agent swallows every escape key until the next
        // tick notices, so drop out of the pane the way an exit does.
        if let Some(active) = self.active_terminal_id.clone()
            && !self.engine.companion_terminals.contains_key(&active)
        {
            self.active_terminal_id = None;
            if self.input_target == InputTarget::Terminal {
                self.input_target = InputTarget::None;
            }
            self.fullscreen_overlay = FullscreenOverlay::None;
            self.session_surface = SessionSurface::Agent;
        }
        if self.selected_session().is_none()
            && matches!(self.input_target, InputTarget::Agent)
            && self.session_surface == SessionSurface::Agent
        {
            self.input_target = InputTarget::None;
            self.fullscreen_overlay = FullscreenOverlay::None;
            self.focus = FocusPane::Left;
        }
    }

    /// The longest this surface may block waiting for a keystroke.
    ///
    /// While the background server is on, the TUI's run loop is also the web
    /// layer's engine servicer, so this interval IS a browser's request latency.
    /// The idle 100ms poll would put a remote keystroke up to that far behind, and
    /// worst case is worse than one interval: the cap restores parity with the
    /// dedicated actor loop's own 50ms tick. Not serving, the lazy cadence stands
    /// and idle CPU stays where it was.
    pub(crate) fn max_poll_ms(&self) -> u64 {
        if self.background_server_is_serving() {
            SERVING_POLL_CAP_MS
        } else {
            u64::MAX
        }
    }

    /// Palette action: start serving the web UI in the background of this TUI.
    ///
    /// The pre-flight (Tailscale detection, then an actual `TcpListener::bind` of
    /// each address) runs on a worker thread, exactly as the flip's does and for
    /// the same reason: `tailscale ip` is a subprocess call and must not block the
    /// run loop. Binding BEFORE anything starts is what keeps a port collision to
    /// a status line with the TUI untouched.
    pub(crate) fn start_background_server(&mut self) {
        if self.companion.is_none() {
            self.set_error(
                "This build of dux cannot serve the web UI in the background. Run `dux server` \
                 for the web UI, or the start-web-server command to hand this terminal over to \
                 it."
                .to_string(),
            );
            return;
        }
        if self.background_server_is_serving() {
            let urls = self
                .companion
                .as_ref()
                .map(|companion| companion.urls())
                .unwrap_or_default();
            self.set_warning(format!(
                "The web UI is already serving in the background on {}. Use \
                 stop-background-server to stop it.",
                join_urls(&urls)
            ));
            return;
        }
        if self.background_server_preflight_pending {
            self.set_warning(
                "The background web server is already starting. Wait for it to report back."
                    .to_string(),
            );
            return;
        }
        if self.server_flip_preflight_pending || self.pending_server_flip.is_some() {
            self.set_warning(
                "dux is already handing this terminal over to the web server. Wait for that to \
                 finish, or come back to the TUI and try again."
                    .to_string(),
            );
            return;
        }

        let op = dux_core::engine::status_op(
            "Starting the web server in the background; the TUI stays right here.".to_string(),
        )
        .resolve_in_handler(|o: &BackgroundServerOutcome| match o {
            BackgroundServerOutcome::Serving { urls, warning } => {
                let where_at = join_urls(urls);
                match warning {
                    Some(warning) => dux_core::engine::Final::warning(format!(
                        "The web UI is serving in the background on {where_at}, and your agents \
                         keep running here. {warning} Use stop-background-server to stop serving."
                    )),
                    None => dux_core::engine::Final::info(format!(
                        "The web UI is serving in the background on {where_at}, and your agents \
                         keep running here. There is no login, so keep it on a network you \
                         trust. Use stop-background-server to stop serving."
                    )),
                }
            }
            BackgroundServerOutcome::Failed(message) => dux_core::engine::Final::error(format!(
                "{message} Nothing is serving and the TUI is untouched."
            )),
            BackgroundServerOutcome::Cancelled => dux_core::engine::Final::info(
                "Cancelled starting the web server in the background. Nothing is serving, the \
                 addresses were released, and config.toml is untouched. Use \
                 start-background-server to try again."
                    .to_string(),
            ),
        });
        self.apply_reaction(EventReaction::Status(op.pending_status()));
        self.pending_background_server_op = Some(op);
        self.background_server_preflight_pending = true;
        self.background_server_wanted = true;
        self.spawn_background_server_preflight();
    }

    /// Run the bind pre-flight on a worker thread, reporting back through
    /// [`WorkerEvent::BackgroundServerPreflightReady`].
    ///
    /// Shares `preflight_server_listeners` with the flip: the two modes bind the
    /// same LOCAL MODE addresses (loopback plus the machine's Tailscale address),
    /// and neither ever reads the configurable `[server] host`, so neither can
    /// open a public listener.
    pub(crate) fn spawn_background_server_preflight(&mut self) {
        let port = self.engine.config.server.port;
        let tailscale = self.engine.config.server.tailscale_mode();
        let tx = self.engine.worker_tx.clone();
        std::thread::spawn(move || {
            let (tailscale_ip, detect_warning) = if tailscale.wants_tailscale() {
                match dux_core::tailscale::detect_ip() {
                    Ok(ip) => (Some(ip), None),
                    Err(reason) => (
                        None,
                        Some(dux_core::tailscale::undetected_warning(
                            tailscale, reason, "loopback",
                        )),
                    ),
                }
            } else {
                (None, None)
            };
            let event = match preflight_server_listeners(port, tailscale_ip) {
                Ok((listeners, urls, bind_warnings)) => {
                    WorkerEvent::BackgroundServerPreflightReady {
                        result: Ok((listeners, urls)),
                        warning: super::sessions::combine_flip_warnings(
                            detect_warning,
                            bind_warnings,
                        ),
                    }
                }
                // A required (loopback) bind failed: surface the error. The
                // detection warning is moot because nothing is going to serve.
                Err(err) => WorkerEvent::BackgroundServerPreflightReady {
                    result: Err(format!("{err:#}")),
                    warning: detect_warning,
                },
            };
            let _ = tx.send(event);
        });
    }

    /// Adopt the pre-flight's listeners and hand them to the companion.
    pub(crate) fn apply_background_server_preflight(
        &mut self,
        result: Result<(Vec<std::net::TcpListener>, Vec<String>), String>,
        warning: Option<String>,
    ) {
        self.background_server_preflight_pending = false;
        // The user changed their mind while the bind was on its worker thread: a
        // stop command, or a reload that turned the setting off. Starting now
        // would serve a listener nobody wants and, worse, PERSIST
        // `serve_while_tui = true` over the value they just chose. Drop the bound
        // listeners (which releases the addresses), say so, and write nothing.
        if !self.background_server_wanted {
            drop(result);
            if let Some(op) = self.pending_background_server_op.take() {
                self.apply_reaction(
                    op.resolve(&BackgroundServerOutcome::Cancelled)
                        .into_reaction(),
                );
            }
            return;
        }
        let outcome = match result {
            Ok((listeners, urls)) => match self.companion.as_mut() {
                Some(companion) => {
                    match companion.start(&mut self.engine, listeners, urls.clone()) {
                        Ok(urls) => {
                            // Persist the choice, so a restart comes up the way the
                            // user left it. Lazy rather than eager: nothing is
                            // half-done if the write lands late, because the serve
                            // is already up and the setting only decides startup.
                            self.engine.config.server.serve_while_tui = true;
                            self.engine
                                .config_writer
                                .save_lazy(self.engine.config.clone());
                            BackgroundServerOutcome::Serving { urls, warning }
                        }
                        Err(message) => BackgroundServerOutcome::Failed(message),
                    }
                }
                // The listeners drop here, releasing the ports.
                None => BackgroundServerOutcome::Failed(
                    "This build of dux cannot serve the web UI in the background.".to_string(),
                ),
            },
            Err(message) => BackgroundServerOutcome::Failed(message),
        };
        // A start that failed leaves nothing wanted, so a later stop does not
        // report on a listener that never came up.
        self.background_server_wanted = self.background_server_is_serving();
        if let Some(op) = self.pending_background_server_op.take() {
            self.apply_reaction(op.resolve(&outcome).into_reaction());
        }
    }

    /// Palette action: stop the background web server, leaving every agent
    /// running.
    pub(crate) fn stop_background_server(&mut self) {
        // A start whose bind pre-flight is still on its worker thread is not
        // serving yet, so it cannot be stopped: it is CANCELLED. Clearing the
        // wanted flag is what the pre-flight's apply consults when it lands, and
        // config is deliberately left alone, because the start never got as far as
        // writing it.
        if !self.background_server_is_serving() && self.background_server_preflight_pending {
            self.background_server_wanted = false;
            self.set_info(
                "Cancelling the background web server start that is still binding its addresses. \
                 Nothing will serve and config.toml is untouched."
                    .to_string(),
            );
            return;
        }
        if !self.background_server_is_serving() {
            self.set_warning(
                "The web UI is not serving in the background right now. Use \
                 start-background-server to start it."
                    .to_string(),
            );
            return;
        }
        self.stop_background_server_quietly();
        // Persist the choice so a restart does not bring back a listener the user
        // just turned off.
        self.engine.config.server.serve_while_tui = false;
        self.engine
            .config_writer
            .save_lazy(self.engine.config.clone());
        self.set_info(
            "Stopped serving the web UI. Your agents and terminals are untouched and still \
             running here; browsers that were connected will report the connection closed. Use \
             start-background-server to serve again."
                .to_string(),
        );
    }

    /// Stop serving without saying anything and without touching config. The
    /// shared half of the palette command, the config-reload transition and the
    /// quit path.
    pub(crate) fn stop_background_server_quietly(&mut self) {
        // Runtime intent, not the saved setting: quitting stops the listener
        // without deciding anything about next time, and that distinction lives in
        // config, which this deliberately does not touch.
        self.background_server_wanted = false;
        // Let go of every pty this surface is driving FIRST, while the serve's
        // buses are still up to carry the news. After the stop there is nothing
        // left to announce on, and every browser watching one of those ptys would
        // keep showing a take-over card naming a terminal that stopped serving.
        //
        // This is also the flip's release and the quit's release: both leave the
        // run loop, and both exits call through here.
        self.release_owned_ptys();
        if let Some(companion) = self.companion.as_mut()
            && companion.is_serving()
        {
            companion.stop(&mut self.engine);
        }
    }

    /// Honor an `[server] serve_while_tui` change that arrived through a config
    /// reload, in both directions.
    ///
    /// The setting is the startup default AND a live switch, so a reload that
    /// flips it has to act rather than wait for the next start: a user who edits
    /// the file to turn the listener off has asked for the listener to go away.
    pub(crate) fn apply_serve_while_tui_setting(&mut self, wanted: bool) {
        // A start still in its bind pre-flight counts as on: otherwise a reload
        // that turns the setting off looks like it has nothing to do, and the
        // pre-flight lands afterwards and starts serving anyway.
        let on = self.background_server_is_serving() || self.background_server_preflight_pending;
        match (on, wanted) {
            (false, true) => self.start_background_server(),
            (true, false) => self.stop_background_server(),
            // Already where the config asks for. Nothing to say: a reload that
            // did not change this should not report on it.
            (true, true) | (false, false) => {}
        }
    }
}

/// The verdict source for one drained batch's origin routing.
///
/// A snapshot rather than a live read, because the web layer's follow-ups consume
/// the pending-op entries the verdict depends on and they run first. See
/// [`dux_core::engine::WebFollowupOps`].
pub(crate) enum CompanionRouting {
    /// Nothing is serving, so every reaction belongs to this surface. Also the
    /// answer when the engine's web pending-op maps still hold entries from a flip
    /// that came back: the serve is over, the ops are not, and skipping then would
    /// leave a reaction nobody handles at all.
    NotServing,
    /// Serving: route against the ids that were in flight when the batch was
    /// drained.
    Serving(dux_core::engine::WebFollowupOps),
}

impl CompanionRouting {
    /// Whether the companion owns the follow-up for `reaction`, so this surface
    /// must skip its own arm for it.
    pub(crate) fn companion_owns(&self, reaction: &EventReaction) -> bool {
        match self {
            Self::NotServing => false,
            Self::Serving(ops) => {
                matches!(ops.owner_of(reaction), dux_core::engine::FollowupOwner::Web)
            }
        }
    }
}

/// The status key the background server's own lifecycle messages ride on, so a
/// self-retirement replaces whatever the last one said instead of queueing behind
/// it.
pub(crate) const BACKGROUND_SERVER_STATUS_KEY: &str = "background-server";

/// The poll-interval cap while the background server is on, in milliseconds.
///
/// A web request waits for the TUI's next loop iteration, so this interval is the
/// browser's added latency. 33ms is the cadence the TUI already polls at while a
/// row animates, so it is a proven-comfortable floor rather than a new number,
/// and it is under the dedicated actor loop's own 50ms tick.
pub(crate) const SERVING_POLL_CAP_MS: u64 = 33;

/// The port a serve is reachable on, rendered for the header chip as ":8080".
///
/// The port rather than a whole address, because every leg of a serve is on the
/// same one and a header crumb has no room for two addresses. Read back from the
/// URLs the serve reported rather than from config, so an ephemeral port is
/// reported as what was actually bound. `None` when no URL carries a port, which
/// leaves the chip saying "serving" and nothing it cannot back up.
fn serving_port_label(urls: &[String]) -> Option<String> {
    // Every leg of a serve is on the same port, so the first URL that carries a
    // readable one answers for all of them. Searching rather than taking the first
    // URL only: if one ever arrives without a port, the answer is the port the
    // others agree on, not silence.
    urls.iter().find_map(|url| {
        let port = url.rsplit_once(':').map(|(_, port)| port)?;
        let port = port.trim_end_matches('/');
        if port.is_empty() || !port.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        Some(format!(":{port}"))
    })
}

/// The serving chip's text: where it is serving, and how many browsers are on it.
///
/// "Connected", not "devices": one browser with two tabs open counts twice, and
/// nothing here can honestly tell that they are the same laptop. Zero says only
/// that a listener exists, because "0 connected" is a number nobody needs and the
/// chip is already carrying that fact by being there at all.
fn serving_chip_label(port: Option<&str>, connections: usize) -> String {
    let where_at = match port {
        Some(port) => format!("serving {port}"),
        None => "serving".to_string(),
    };
    if connections == 0 {
        format!("● {where_at}")
    } else {
        format!("● {where_at} · {connections} connected")
    }
}

/// Render a serve's addresses for a status line. Comma-separated, because there
/// are at most two of them (loopback and the Tailscale leg).
fn join_urls(urls: &[String]) -> String {
    if urls.is_empty() {
        // Should not happen (a serve without an address is not a serve), but a
        // status line saying nothing is worse than one saying it does not know.
        return "an address dux could not read back".to_string();
    }
    urls.join(", ")
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::{Arc, Mutex};

    use dux_core::background_serve::{
        BackgroundServeCompanion, DrainedMaintenance, PtyOwnershipEvent, ServiceOutcome,
        TuiOwnership,
    };
    use dux_core::engine::EventReaction;

    use super::super::test_support::{default_bindings, test_app};
    use super::*;

    /// What a [`FakeCompanion`] saw and what it was told to do, shared with the
    /// test because the App owns the companion behind a trait object.
    #[derive(Default)]
    pub(crate) struct Recorded {
        /// One entry per reaction the seam handed over, by variant name.
        reactions: Vec<String>,
        /// The apply counts the seam reported.
        activity: Vec<u64>,
        /// What the next `service` call answers.
        mutated_next: bool,
        /// How many times `service` was called.
        serviced: usize,
        /// A session to remove from the engine the next time `service` runs, so a
        /// test can stand in for a browser deleting an agent.
        remove_session: Option<String>,
        /// Turn `ui.show_changes_pane` off in the engine's config the next time
        /// `service` runs, standing in for a browser saving that preference.
        hide_changes_pane: bool,
        /// What the next `service` call reports as a self-retirement, standing in
        /// for a required listener dying.
        retirement_next: Option<String>,
        /// One entry per maintenance hand-off the seam made.
        maintenance: Vec<DrainedMaintenance>,
        /// Stand in for the REAL fanout's side effects: its `drive_*` follow-ups
        /// take their pending op out of the engine's web map to resolve it, and
        /// the add-project one performs the add inline while it is in there. Both
        /// happen BEFORE this surface applies the reaction, which is exactly the
        /// window the ownership snapshot exists to survive.
        fanout_consumes_ops: bool,
        /// Every ownership fact the seam published, in order, standing in for the
        /// `pty.owner` and grid broadcasts a real serve would have emitted.
        pub(crate) published: Vec<PtyOwnershipEvent>,
        /// How many browser tabs the serve says are connected, standing in for the
        /// connection registry's own count.
        pub(crate) connections: usize,
    }

    /// A companion that records instead of serving. Serving is a real socket and a
    /// real runtime; none of that is what these tests are about.
    pub(crate) struct FakeCompanion {
        serving: bool,
        recorded: Arc<Mutex<Recorded>>,
        /// A REAL ownership registry, because the gate's whole job is to obey one
        /// and a fake verdict would test nothing. Shared with the test so it can
        /// stand in for a browser connection claiming a pty.
        ownership: TuiOwnership,
    }

    impl FakeCompanion {
        pub(crate) fn serving() -> (Box<Self>, Arc<Mutex<Recorded>>) {
            let (companion, recorded, _ownership) = Self::serving_with_ownership();
            (companion, recorded)
        }

        /// The same companion, handing back its ownership seat so a test can act
        /// as the other device in the registry.
        pub(crate) fn serving_with_ownership() -> (Box<Self>, Arc<Mutex<Recorded>>, TuiOwnership) {
            let recorded = Arc::new(Mutex::new(Recorded::default()));
            let owners = Arc::new(dux_core::pty_owners::PtySizeOwners::default());
            let ownership = TuiOwnership {
                conn_id: owners.next_conn_id(),
                owners,
            };
            (
                Box::new(Self {
                    serving: true,
                    recorded: Arc::clone(&recorded),
                    ownership: ownership.clone(),
                }),
                recorded,
                ownership,
            )
        }
    }

    impl BackgroundServeCompanion for FakeCompanion {
        fn on_reaction(&mut self, engine: &mut Engine, reaction: &EventReaction) {
            let mut recorded = self.recorded.lock().expect("not poisoned");
            recorded.reactions.push(reaction_kind(reaction).to_string());
            if !recorded.fanout_consumes_ops {
                return;
            }
            // What the real `drive_pr_lookup_followup` / `finish_web_project_add`
            // do: resolve their keyed op, which means REMOVING it from the map the
            // routing reads, and (for the add) write the project inline.
            engine.pending_web_pr_lookup_ops.clear();
            engine.pending_web_checkout_ops.clear();
            engine.pending_web_add_project_ops.clear();
            if matches!(
                reaction,
                EventReaction::AddProjectAfterBranchCheckout { .. }
                    | EventReaction::AddProjectAfterInitialCommit { .. }
            ) {
                engine.projects.push(sample_project(
                    "added-by-a-browser",
                    "/tmp/added-by-a-browser",
                ));
            }
        }

        fn service(&mut self, engine: &mut Engine) -> ServiceOutcome {
            let mut recorded = self.recorded.lock().expect("not poisoned");
            recorded.serviced += 1;
            if let Some(id) = recorded.remove_session.take() {
                engine.sessions.retain(|s| s.id != id);
            }
            if std::mem::take(&mut recorded.hide_changes_pane) {
                engine.config.ui.show_changes_pane = false;
            }
            ServiceOutcome {
                mutated: recorded.mutated_next,
                stopped: false,
                retirement: recorded.retirement_next.take(),
            }
        }

        fn note_maintenance(&mut self, maintenance: &DrainedMaintenance) {
            self.recorded
                .lock()
                .expect("not poisoned")
                .maintenance
                .push(maintenance.clone());
        }

        fn note_engine_activity(&mut self, command_applies: u64) {
            self.recorded
                .lock()
                .expect("not poisoned")
                .activity
                .push(command_applies);
        }

        fn is_serving(&self) -> bool {
            self.serving
        }

        fn urls(&self) -> Vec<String> {
            vec!["http://127.0.0.1:8080".to_string()]
        }

        fn connections(&self) -> usize {
            if !self.serving {
                return 0;
            }
            self.recorded.lock().expect("not poisoned").connections
        }

        fn start(
            &mut self,
            _engine: &mut Engine,
            _listeners: Vec<std::net::TcpListener>,
            _urls: Vec<String>,
        ) -> Result<Vec<String>, String> {
            self.serving = true;
            Ok(self.urls())
        }

        fn stop(&mut self, _engine: &mut Engine) {
            self.serving = false;
        }

        fn ownership(&self) -> Option<TuiOwnership> {
            self.serving.then(|| self.ownership.clone())
        }

        fn publish_ownership_events(&mut self, events: &[PtyOwnershipEvent]) {
            self.recorded
                .lock()
                .expect("not poisoned")
                .published
                .extend_from_slice(events);
        }
    }

    /// The reaction variants these tests care about, named. `EventReaction` has no
    /// public discriminant accessor, and a local matcher is clearer than one
    /// anyway: it says which variants the test is watching for.
    fn reaction_kind(reaction: &EventReaction) -> &'static str {
        match reaction {
            EventReaction::ApplyReloadedConfig(_) => "ApplyReloadedConfig",
            EventReaction::OpenNewAgentPromptForPr { .. } => "OpenNewAgentPromptForPr",
            EventReaction::Nothing => "Nothing",
            _ => "other",
        }
    }

    fn sample_project(id: &str, path: &str) -> crate::model::Project {
        crate::model::Project {
            id: id.to_string(),
            name: id.to_string(),
            path: path.to_string(),
            explicit_default_provider: None,
            default_provider: crate::model::ProviderKind::new("claude"),
            leading_branch: Some("main".to_string()),
            auto_reopen_agents: None,
            startup_command: None,
            env: Default::default(),
            current_branch: "main".to_string(),
            branch_status: crate::model::ProjectBranchStatus::Leading,
            path_missing: false,
            created_at: None,
        }
    }

    fn resolved_pr() -> dux_core::worker::ResolvedPullRequest {
        dux_core::worker::ResolvedPullRequest {
            project: sample_project("p1", "/tmp/p1"),
            host: "github.com".to_string(),
            owner_repo: "octocat/Hello-World".to_string(),
            number: 42,
            title: "Fix bug".to_string(),
            state: "OPEN".to_string(),
            head_ref_name: "feature/pr-42".to_string(),
            custom_name: None,
        }
    }

    fn pr_reaction(status_op_id: Option<String>) -> EventReaction {
        EventReaction::OpenNewAgentPromptForPr {
            pr: Box::new(resolved_pr()),
            status_op_id,
        }
    }

    fn a_web_pr_lookup_op(app: &mut App) -> String {
        let op = dux_core::engine::status_op("Resolving PR…".to_string()).resolve_in_handler(
            |_o: &dux_core::engine::WebPrLookupOutcome| dux_core::engine::Final::info("done"),
        );
        let id = op.id().to_string();
        app.engine.pending_web_pr_lookup_ops.insert(id.clone(), op);
        id
    }

    /// A PR lookup a BROWSER started must not pop a name prompt in the terminal.
    ///
    /// The browser already sent the name and the web layer dispatches the create
    /// straight away, so a prompt here is a second create waiting on a keystroke
    /// nobody meant to give. This was one of the three concrete double-runs the
    /// origin routing exists for.
    #[test]
    fn a_web_originated_pr_lookup_opens_no_terminal_prompt() {
        let mut app = test_app(default_bindings());
        let (companion, _recorded) = FakeCompanion::serving();
        app.companion = Some(companion);
        let id = a_web_pr_lookup_op(&mut app);

        app.apply_reaction(pr_reaction(Some(id)));

        assert!(
            matches!(app.prompt, PromptState::None),
            "a browser's PR create must not open the terminal's name prompt"
        );
    }

    /// The same reaction from the TERMINAL's own lookup still opens its prompt.
    /// The routing is about origin, not about the mode being on.
    #[test]
    fn a_tui_originated_pr_lookup_still_opens_its_prompt() {
        let mut app = test_app(default_bindings());
        let (companion, _recorded) = FakeCompanion::serving();
        app.companion = Some(companion);

        app.apply_reaction(pr_reaction(Some("a-tui-op-id".to_string())));

        assert!(
            matches!(app.prompt, PromptState::NameNewAgent { .. }),
            "the terminal's own PR lookup must still prompt for a name, got {:?}",
            std::mem::discriminant(&app.prompt)
        );
    }

    /// With NOTHING serving, the routing must not fire at all, even though the
    /// engine's web pending-op maps can still hold entries (a flip that came back
    /// leaves them behind). Skipping then would leave the reaction unhandled by
    /// anyone.
    #[test]
    fn with_no_companion_serving_every_reaction_is_the_terminals() {
        let mut app = test_app(default_bindings());
        let id = a_web_pr_lookup_op(&mut app);

        app.apply_reaction(pr_reaction(Some(id)));

        assert!(
            matches!(app.prompt, PromptState::NameNewAgent { .. }),
            "with no web layer running, the terminal owns every reaction"
        );
    }

    /// The seam is wired into the DRAIN, not just callable. Proven with a real
    /// worker event on the real channel, because "the helper works" and "the
    /// helper is called" are different claims and only the second one matters.
    #[test]
    fn draining_a_worker_event_lends_it_to_the_seam_before_applying_it() {
        let mut app = test_app(default_bindings());
        let (companion, recorded) = FakeCompanion::serving();
        app.companion = Some(companion);
        app.engine
            .worker_tx
            .send(dux_core::worker::WorkerEvent::ResourceStatsReady(
                Vec::new(),
                false,
            ))
            .expect("the engine's worker channel is open");

        app.drain_events();

        let reactions = recorded.lock().expect("not poisoned").reactions.clone();
        assert!(
            !reactions.is_empty(),
            "the drain must hand every reaction to the seam; it handed over nothing"
        );
    }

    /// A config reload has to reach the web layer, because the terminal's own
    /// reload arm has never had a reason to announce one and browsers need to
    /// refetch. The seam is what carries it, and it must be handed the reaction
    /// BEFORE `apply_reaction` consumes it.
    #[test]
    fn the_seam_sees_a_config_reload_before_the_terminal_applies_it() {
        let mut app = test_app(default_bindings());
        let (companion, recorded) = FakeCompanion::serving();
        app.companion = Some(companion);

        let reaction = EventReaction::ApplyReloadedConfig(Box::new(app.engine.config.clone()));
        app.notify_companion(&reaction);
        app.apply_reaction(reaction);

        assert_eq!(
            recorded.lock().expect("not poisoned").reactions,
            vec!["ApplyReloadedConfig".to_string()],
            "the reload must reach the seam, or a browser never learns config changed"
        );
    }

    /// A rename made in a browser has to rebuild this surface's sidebar. Nothing
    /// on this surface has any other reason to: the change did not arrive through
    /// its own event stream.
    #[test]
    fn a_mutating_service_iteration_rebuilds_the_sidebar() {
        let mut app = test_app(default_bindings());
        let (companion, recorded) = FakeCompanion::serving();
        app.companion = Some(companion);
        let session_id = app
            .engine
            .sessions
            .first()
            .map(|s| s.id.clone())
            .expect("the test app has a session");
        app.rebuild_left_items();
        let before = app.left_items_cache.len();
        app.selected_left = before.saturating_sub(1);
        {
            let mut recorded = recorded.lock().expect("not poisoned");
            recorded.mutated_next = true;
            recorded.remove_session = Some(session_id.clone());
        }

        app.service_companion();

        assert!(
            !app.engine.sessions.iter().any(|s| s.id == session_id),
            "the fake stands in for a browser deleting the agent"
        );
        assert!(
            app.left_items_cache.len() < before,
            "the sidebar must rebuild without the deleted agent ({before} rows before, {} after)",
            app.left_items_cache.len()
        );
        assert!(
            app.selected_left < app.left_items_cache.len().max(1),
            "the cursor must be clamped back inside the shorter list"
        );
        assert_eq!(
            recorded.lock().expect("not poisoned").serviced,
            1,
            "the seam is serviced once per iteration"
        );
    }

    /// Under one engine, a browser's Preferences save writes `engine.config`
    /// directly. Anything this surface caches off that config would stay stale
    /// until a manual reload, so the seam re-seeds the fields the web can write.
    #[test]
    fn a_web_written_preference_re_seeds_this_surfaces_cached_view_state() {
        let mut app = test_app(default_bindings());
        let (companion, recorded) = FakeCompanion::serving();
        app.companion = Some(companion);
        app.engine.config.ui.show_changes_pane = true;
        app.right_hidden = false;
        app.focus = FocusPane::Files;
        {
            let mut recorded = recorded.lock().expect("not poisoned");
            recorded.mutated_next = true;
            recorded.hide_changes_pane = true;
        }

        app.service_companion();

        assert!(
            app.right_hidden,
            "the changes pane follows the preference the browser saved"
        );
        assert_ne!(
            app.focus,
            FocusPane::Files,
            "focus must leave a pane that is no longer on screen"
        );
    }

    /// The seam is told what this surface applied, so a keystroke here opens the
    /// web layer's change gate on the same iteration instead of waiting for its
    /// slow fingerprint backstop.
    #[test]
    fn servicing_reports_this_surfaces_apply_count() {
        let mut app = test_app(default_bindings());
        let (companion, recorded) = FakeCompanion::serving();
        app.companion = Some(companion);

        app.service_companion();
        let _ = app.engine.apply(dux_core::engine::Command::RecoverConfig);
        app.service_companion();

        let activity = recorded.lock().expect("not poisoned").activity.clone();
        assert_eq!(activity.len(), 2, "reported once per iteration");
        assert!(
            activity[1] > activity[0],
            "an apply between iterations must be visible to the seam: {activity:?}"
        );
    }

    /// Nothing serving means nothing is lent the engine, and no cost is paid.
    #[test]
    fn a_companion_that_is_not_serving_is_never_lent_the_engine() {
        let mut app = test_app(default_bindings());
        let recorded = Arc::new(Mutex::new(Recorded::default()));
        let owners = Arc::new(dux_core::pty_owners::PtySizeOwners::default());
        app.companion = Some(Box::new(FakeCompanion {
            serving: false,
            recorded: Arc::clone(&recorded),
            ownership: TuiOwnership {
                conn_id: owners.next_conn_id(),
                owners,
            },
        }));

        app.notify_companion(&EventReaction::Nothing);
        app.service_companion();

        let recorded = recorded.lock().expect("not poisoned");
        assert!(recorded.reactions.is_empty());
        assert_eq!(recorded.serviced, 0);
        assert!(recorded.activity.is_empty());
    }

    /// The poll interval IS a browser's request latency while serving, so it is
    /// capped. Off, the lazy cadence stands and idle CPU is unchanged.
    #[test]
    fn the_poll_interval_is_capped_only_while_serving() {
        let mut app = test_app(default_bindings());
        assert_eq!(
            app.max_poll_ms(),
            u64::MAX,
            "not serving, nothing constrains the poll"
        );

        let (companion, _recorded) = FakeCompanion::serving();
        app.companion = Some(companion);
        assert_eq!(app.max_poll_ms(), SERVING_POLL_CAP_MS);
        const {
            // At least as tight as the animation cadence the TUI already polls
            // at, and under the dedicated actor loop's own 50ms tick.
            assert!(SERVING_POLL_CAP_MS <= 33);
        }
        // The two branches that actually wait: the structured-event poll's idle
        // interval and the raw-stdin poll's fixed one. Both take the minimum.
        assert_eq!(100u64.min(app.max_poll_ms()), SERVING_POLL_CAP_MS);
        assert_eq!(33u64.min(app.max_poll_ms()), SERVING_POLL_CAP_MS);
    }

    /// The flip and the background server bind the same addresses, so flipping
    /// while serving would fail on dux's own port. Refuse with the reason and both
    /// ways out instead.
    #[test]
    fn flipping_while_serving_in_the_background_is_refused_with_a_reason() {
        let mut app = test_app(default_bindings());
        let (companion, _recorded) = FakeCompanion::serving();
        app.companion = Some(companion);

        app.start_web_server();

        let message = app
            .status
            .most_recent_tui()
            .map(|(_, message)| message)
            .unwrap_or_default();
        assert!(
            message.contains("already serving in the background"),
            "the refusal must say why: {message}"
        );
        assert!(
            message.contains("stop-background-server"),
            "and how to get the flip instead: {message}"
        );
        assert!(
            !app.server_flip_preflight_pending,
            "no pre-flight may be dispatched"
        );
    }

    /// The flip refusal covers a background start that is still in its bind
    /// pre-flight, too: that worker holds the ports, so the flip would collide
    /// with dux itself and say something less useful about it.
    #[test]
    fn flipping_while_a_background_start_is_in_flight_is_refused_too() {
        let mut app = test_app(default_bindings());
        let (companion, _recorded) = FakeCompanion::serving();
        app.companion = Some(companion);
        app.stop_background_server_quietly();
        app.background_server_preflight_pending = true;

        app.start_web_server();

        let message = app
            .status
            .most_recent_tui()
            .map(|(_, message)| message)
            .unwrap_or_default();
        assert!(
            message.contains("already starting the web server in the background"),
            "the refusal must name the start that is in flight: {message}"
        );
        assert!(
            !app.server_flip_preflight_pending,
            "no competing pre-flight may be dispatched"
        );
    }

    /// Asking to start while already serving says where it is rather than
    /// spawning a second pre-flight that would collide with the live listener.
    #[test]
    fn starting_while_already_serving_says_where_it_is() {
        let mut app = test_app(default_bindings());
        let (companion, _recorded) = FakeCompanion::serving();
        app.companion = Some(companion);

        app.start_background_server();

        let message = app
            .status
            .most_recent_tui()
            .map(|(_, message)| message)
            .unwrap_or_default();
        assert!(
            message.contains("127.0.0.1:8080"),
            "the message must name the address: {message}"
        );
        assert!(
            !app.background_server_preflight_pending,
            "no second pre-flight may be dispatched"
        );
    }

    /// Stopping turns the listener off, saves the choice, and says the agents are
    /// untouched, because "stopped the server" reads like "stopped my work".
    #[test]
    fn stopping_saves_the_choice_and_says_the_agents_are_untouched() {
        let mut app = test_app(default_bindings());
        let (companion, _recorded) = FakeCompanion::serving();
        app.companion = Some(companion);
        app.engine.config.server.serve_while_tui = true;

        app.stop_background_server();

        assert!(!app.background_server_is_serving());
        assert!(
            !app.engine.config.server.serve_while_tui,
            "the choice is saved, or a restart brings the listener back"
        );
        let message = app
            .status
            .most_recent_tui()
            .map(|(_, message)| message)
            .unwrap_or_default();
        assert!(
            message.contains("agents"),
            "the message must say the agents are unaffected: {message}"
        );
    }

    /// Leaving the terminal UI stops the serve, and it goes through the same
    /// quiet stop the palette command and the config transition use.
    ///
    /// This matters for a reason that is not visible at the call site: stopping
    /// trips the PTY forwarders' teardown flag BEFORE anything waits on the
    /// serve's runtime, and the forwarders are parked on channels the engine still
    /// owns. An implicit drop instead of this call would block on tasks that never
    /// notice. The `App::run` teardown that calls this is not itself covered by a
    /// test, because driving the run loop needs a real terminal; the two halves
    /// either side of it are.
    #[test]
    fn a_quiet_stop_turns_the_listener_off_without_touching_config_or_status() {
        let mut app = test_app(default_bindings());
        let (companion, _recorded) = FakeCompanion::serving();
        app.companion = Some(companion);
        app.engine.config.server.serve_while_tui = true;

        app.stop_background_server_quietly();

        assert!(!app.background_server_is_serving(), "the listener is off");
        assert!(
            app.engine.config.server.serve_while_tui,
            "quitting is not a decision to stop serving next time"
        );
        assert!(
            app.status.most_recent_tui().is_none(),
            "nothing to announce on a surface that is going away"
        );
    }

    /// A reload that flipped the setting acts on it, in both directions. Waiting
    /// for the next start would mean a user who edited the file to turn the
    /// listener off still has one.
    #[test]
    fn a_reloaded_setting_turns_the_listener_off_and_on() {
        let mut app = test_app(default_bindings());
        let (companion, _recorded) = FakeCompanion::serving();
        app.companion = Some(companion);

        app.apply_serve_while_tui_setting(false);
        assert!(
            !app.background_server_is_serving(),
            "a reload that turned it off must stop the listener"
        );

        // And back on: the fake's `start` needs no listeners, so this exercises
        // the transition rather than a real bind.
        app.apply_serve_while_tui_setting(true);
        assert!(
            app.background_server_preflight_pending,
            "a reload that turned it on must dispatch the bind pre-flight"
        );
    }

    /// THE REAL SHAPE OF THE DOUBLE-RUN. A browser's PR create must not pop a name
    /// prompt here even though the web fanout, which runs FIRST, has already taken
    /// its pending op out of the map the routing reads.
    ///
    /// The earlier routing test passed while the bug was live, because its fake
    /// companion did nothing. Here the fake removes the entry exactly as
    /// `drive_pr_lookup_followup` does, and the whole thing goes through the real
    /// `drain_events` on a real worker event, so "the verdict is taken before the
    /// fanout" is what is being asserted rather than "the helper works".
    #[test]
    fn a_web_pr_create_pops_no_prompt_even_after_the_fanout_consumed_its_op() {
        let mut app = test_app(default_bindings());
        let (companion, recorded) = FakeCompanion::serving();
        app.companion = Some(companion);
        recorded.lock().expect("not poisoned").fanout_consumes_ops = true;
        let id = a_web_pr_lookup_op(&mut app);
        app.engine
            .worker_tx
            .send(dux_core::worker::WorkerEvent::PullRequestResolved {
                result: Ok(resolved_pr()),
                purpose: dux_core::worker::PrLookupPurpose::CreateAgent,
                status_op_id: Some(id.clone()),
            })
            .expect("the engine's worker channel is open");

        app.drain_events();

        assert!(
            app.engine.pending_web_pr_lookup_ops.is_empty(),
            "the fake stands in for the fanout, which resolves and removes the op"
        );
        assert!(
            matches!(app.prompt, PromptState::None),
            "a browser's PR create must not open the terminal's name prompt; the verdict has \
             to be taken before the fanout can empty the map it is read from"
        );
    }

    /// A web-owned follow-up mutated the workspace during the fanout, so this
    /// surface has to rebuild even though it ran no arm of its own.
    ///
    /// The inline project add writes `engine.projects` from inside the fanout, and
    /// the change never travels through this surface's own event stream. While the
    /// duplicate arm existed it happened to rebuild the sidebar as a side effect of
    /// the bug; with the duplicate gone something has to say so deliberately.
    #[test]
    fn a_web_owned_followup_that_mutated_the_workspace_refreshes_the_sidebar() {
        let mut app = test_app(default_bindings());
        let (companion, recorded) = FakeCompanion::serving();
        app.companion = Some(companion);
        {
            let mut recorded = recorded.lock().expect("not poisoned");
            recorded.fanout_consumes_ops = true;
            // The service iteration itself reports NOTHING: the mutation happened
            // during the drain, which is the case this test is about.
            recorded.mutated_next = false;
        }
        let op = dux_core::engine::status_op("Adding…".to_string()).resolve_in_handler(
            |_o: &dux_core::engine::WebAddProjectOutcome| dux_core::engine::Final::info("done"),
        );
        let id = op.id().to_string();
        app.engine
            .pending_web_add_project_ops
            .insert(id.clone(), op);
        app.rebuild_left_items();
        // The observable for "the post-mutation refresh ran": it rebuilds the
        // sidebar and then repairs the cursor. Park the cursor out of range so a
        // refresh that did not happen leaves it there.
        app.selected_left = 99;

        let reaction = EventReaction::AddProjectAfterBranchCheckout {
            path: "/tmp/added-by-a-browser".to_string(),
            name: "added-by-a-browser".to_string(),
            target_branch: "main".to_string(),
            leading_branch: "main".to_string(),
            status_op_id: Some(id),
        };
        let routing = app.companion_routing();
        app.notify_companion(&reaction);
        app.apply_routed_reaction(reaction, &routing);
        app.service_companion();

        assert!(
            app.engine
                .projects
                .iter()
                .any(|p| p.id == "added-by-a-browser"),
            "the fake stands in for the web's inline add"
        );
        assert!(
            app.selected_left < app.left_items_cache.len().max(1),
            "a web-owned follow-up that mutated the workspace must trigger this surface's \
             post-mutation refresh; the cursor is still at {}",
            app.selected_left
        );
    }

    /// An agent that exits while this surface is the sweeper still reaches the
    /// companion, which is the only way a browser learns about it.
    ///
    /// The maintenance sweeps have one runner per process; while serving, that is
    /// this surface, so the web layer's own prune loop (the sole emitter of "Agent
    /// exited." and of the change gate a prune opens) never runs.
    #[test]
    fn the_maintenance_this_surface_swept_is_handed_to_the_companion() {
        let mut app = test_app(default_bindings());
        let (companion, recorded) = FakeCompanion::serving();
        app.companion = Some(companion);

        // The real drain, wired end to end. Staging an actual exit here would mean
        // a real child process; what has to be pinned is that the drain REPORTS
        // its sweeps at all, because the failure mode is a lane nobody calls.
        app.drain_events();

        assert_eq!(
            recorded.lock().expect("not poisoned").maintenance.len(),
            1,
            "the drain must hand its sweep results to the companion once per pass"
        );

        // And the payload is carried rather than flattened on the way through.
        app.note_companion_maintenance(&DrainedMaintenance {
            pruned: Vec::new(),
            foregrounds_changed: true,
        });
        let recorded = recorded.lock().expect("not poisoned");
        assert!(
            recorded.maintenance[1].foregrounds_changed,
            "a foreground change must cross the seam, or a browser waits for the backstop"
        );
    }

    /// Stopping while the bind pre-flight is still on its worker thread CANCELS
    /// the start, and writes nothing.
    ///
    /// The old guard only asked whether a listener was up, so a stop in this window
    /// was a no-op warning; the pre-flight then landed, started serving anyway, and
    /// persisted `serve_while_tui = true` over the answer the user had just given.
    #[test]
    fn stopping_during_the_pre_flight_cancels_it_and_persists_nothing() {
        let mut app = test_app(default_bindings());
        let (companion, _recorded) = FakeCompanion::serving();
        app.companion = Some(companion);
        app.stop_background_server_quietly();
        app.engine.config.server.serve_while_tui = false;
        app.start_background_server();
        assert!(app.background_server_preflight_pending);

        app.stop_background_server();

        // The pre-flight's own listeners land afterwards; they must be dropped.
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral bind");
        app.apply_background_server_preflight(
            Ok((vec![listener], vec!["http://127.0.0.1:0".to_string()])),
            None,
        );

        assert!(
            !app.background_server_is_serving(),
            "a cancelled start must not serve"
        );
        assert!(
            !app.engine.config.server.serve_while_tui,
            "a cancelled start must not rewrite the setting the user just turned off"
        );
        let message = app
            .status
            .most_recent_tui()
            .map(|(_, message)| message)
            .unwrap_or_default();
        assert!(
            message.contains("Cancelled"),
            "the keyed busy must resolve to an honest final: {message}"
        );
    }

    /// The same, arriving as a config reload rather than the palette command: a
    /// user who edits the file to turn the listener off while a start is in flight
    /// has still asked for no listener.
    #[test]
    fn a_reload_that_turns_it_off_during_the_pre_flight_cancels_it_too() {
        let mut app = test_app(default_bindings());
        let (companion, _recorded) = FakeCompanion::serving();
        app.companion = Some(companion);
        app.stop_background_server_quietly();
        app.engine.config.server.serve_while_tui = false;
        app.start_background_server();
        assert!(app.background_server_preflight_pending);

        app.apply_serve_while_tui_setting(false);

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("ephemeral bind");
        app.apply_background_server_preflight(
            Ok((vec![listener], vec!["http://127.0.0.1:0".to_string()])),
            None,
        );

        assert!(
            !app.background_server_is_serving(),
            "the reload asked for no listener, so the landing pre-flight must not start one"
        );
        assert!(
            !app.engine.config.server.serve_while_tui,
            "and it must not write the setting back on"
        );
    }

    /// The chip is the standing "there is a listener" signal first and a counter
    /// second, so it is there with nobody connected and grows a count when
    /// somebody is.
    #[test]
    fn the_serving_chip_says_where_it_serves_and_who_is_on_it() {
        let mut app = test_app(default_bindings());
        assert_eq!(
            app.serving_chip(),
            None,
            "nothing serving, nothing to say about a listener"
        );

        let (companion, recorded) = FakeCompanion::serving();
        app.companion = Some(companion);
        assert_eq!(
            app.serving_chip().as_deref(),
            Some("● serving :8080"),
            "serving with nobody on it still has to say a listener exists"
        );

        recorded.lock().expect("not poisoned").connections = 1;
        assert_eq!(
            app.serving_chip().as_deref(),
            Some("● serving :8080 · 1 connected")
        );
        recorded.lock().expect("not poisoned").connections = 3;
        assert_eq!(
            app.serving_chip().as_deref(),
            Some("● serving :8080 · 3 connected")
        );

        app.stop_background_server_quietly();
        assert_eq!(
            app.serving_chip(),
            None,
            "a stopped serve leaves no chip behind, whatever the last count was"
        );
    }

    /// The port comes back from the addresses the serve actually bound, so an
    /// ephemeral port is reported as what it became. Anything the parse cannot
    /// vouch for leaves the chip saying only that it is serving.
    #[test]
    fn the_chips_port_is_read_back_from_the_served_addresses() {
        assert_eq!(
            serving_port_label(&["http://127.0.0.1:41337".to_string()]).as_deref(),
            Some(":41337")
        );
        assert_eq!(
            serving_port_label(&["http://[fd7a:115c:a1e0::1]:8080".to_string()]).as_deref(),
            Some(":8080"),
            "a bracketed IPv6 literal must not confuse the port off the end"
        );
        assert_eq!(
            serving_port_label(&["http://127.0.0.1:8080/".to_string()]).as_deref(),
            Some(":8080"),
            "a trailing slash is not part of the port"
        );
        assert_eq!(serving_port_label(&[]), None, "no address, no port");
        assert_eq!(
            serving_port_label(&["http://dux.example".to_string()]),
            None,
            "an address with no port must not report the host as one"
        );
        assert_eq!(
            serving_chip_label(None, 2),
            "● serving · 2 connected",
            "an unreadable port drops the address, never the count"
        );
    }

    /// A serve that retired itself says so on the status line, not only in the log.
    ///
    /// The last thing the user was told is the address it was serving on. A
    /// required leg dying while that sentence is still on screen makes it a lie,
    /// and `dux.log` is not where anybody looks to find that out.
    #[test]
    fn a_self_retired_serve_warns_on_the_status_line() {
        let mut app = test_app(default_bindings());
        let (companion, recorded) = FakeCompanion::serving();
        app.companion = Some(companion);
        recorded.lock().expect("not poisoned").retirement_next = Some(
            "The web UI stopped serving in the background: the listener stopped accepting \
             connections. Use start-background-server to serve again."
                .to_string(),
        );

        app.service_companion();

        let (tone, message) = app
            .status
            .most_recent_tui()
            .expect("a retirement must reach the status line");
        assert_eq!(tone, StatusTone::Warning, "serving stopped is a warning");
        assert!(
            message.contains("stopped serving"),
            "the message must say what happened: {message}"
        );
        assert!(
            message.contains("start-background-server"),
            "and name the way back: {message}"
        );
    }
}
