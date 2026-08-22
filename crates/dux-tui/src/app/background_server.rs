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

    /// Whether the companion owns the follow-up for `reaction`, so this surface
    /// must skip its own arm for it.
    ///
    /// Gated on actually serving, not merely on the engine's answer. The engine's
    /// web pending-op maps can still hold entries after a flip has come back (the
    /// serve is over, the ops are not), and skipping then would leave a reaction
    /// nobody handles at all.
    pub(crate) fn companion_owns_followup(&self, reaction: &EventReaction) -> bool {
        self.background_server_is_serving()
            && matches!(
                self.engine.followup_owner(reaction),
                dux_core::engine::FollowupOwner::Web
            )
    }

    /// Lend the engine to the companion for its per-iteration work, and do this
    /// surface's own follow-up when the companion changed shared state.
    ///
    /// Called once per run-loop iteration. A no-op when nothing is serving.
    pub(crate) fn service_companion(&mut self) {
        let applies = self.engine.command_applies;
        let mutated = match self.companion.as_mut() {
            Some(companion) if companion.is_serving() => {
                // Tell it what this surface did BEFORE it services, so a keystroke
                // here reaches a browser on the same iteration rather than waiting
                // for the fingerprint backstop.
                companion.note_engine_activity(applies);
                companion.service(&mut self.engine).mutated
            }
            _ => return,
        };
        if mutated {
            self.refresh_after_companion_mutation();
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
            "Starting the web server in the background — the TUI stays right here.".to_string(),
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
        });
        self.apply_reaction(EventReaction::Status(op.pending_status()));
        self.pending_background_server_op = Some(op);
        self.background_server_preflight_pending = true;
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
        if let Some(op) = self.pending_background_server_op.take() {
            self.apply_reaction(op.resolve(&outcome).into_reaction());
        }
    }

    /// Palette action: stop the background web server, leaving every agent
    /// running.
    pub(crate) fn stop_background_server(&mut self) {
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
        let serving = self.background_server_is_serving();
        match (serving, wanted) {
            (false, true) => self.start_background_server(),
            (true, false) => self.stop_background_server(),
            // Already where the config asks for. Nothing to say: a reload that
            // did not change this should not report on it.
            (true, true) | (false, false) => {}
        }
    }
}

/// The poll-interval cap while the background server is on, in milliseconds.
///
/// A web request waits for the TUI's next loop iteration, so this interval is the
/// browser's added latency. 33ms is the cadence the TUI already polls at while a
/// row animates, so it is a proven-comfortable floor rather than a new number,
/// and it is under the dedicated actor loop's own 50ms tick.
pub(crate) const SERVING_POLL_CAP_MS: u64 = 33;

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
mod tests {
    use std::sync::{Arc, Mutex};

    use dux_core::background_serve::{BackgroundServeCompanion, ServiceOutcome};
    use dux_core::engine::EventReaction;

    use super::super::test_support::{default_bindings, test_app};
    use super::*;

    /// What a [`FakeCompanion`] saw and what it was told to do, shared with the
    /// test because the App owns the companion behind a trait object.
    #[derive(Default)]
    struct Recorded {
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
    }

    /// A companion that records instead of serving. Serving is a real socket and a
    /// real runtime; none of that is what these tests are about.
    struct FakeCompanion {
        serving: bool,
        recorded: Arc<Mutex<Recorded>>,
    }

    impl FakeCompanion {
        fn serving() -> (Box<Self>, Arc<Mutex<Recorded>>) {
            let recorded = Arc::new(Mutex::new(Recorded::default()));
            (
                Box::new(Self {
                    serving: true,
                    recorded: Arc::clone(&recorded),
                }),
                recorded,
            )
        }
    }

    impl BackgroundServeCompanion for FakeCompanion {
        fn on_reaction(&mut self, _engine: &mut Engine, reaction: &EventReaction) {
            self.recorded
                .lock()
                .expect("not poisoned")
                .reactions
                .push(reaction_kind(reaction).to_string());
        }

        fn service(&mut self, engine: &mut Engine) -> ServiceOutcome {
            let mut recorded = self.recorded.lock().expect("not poisoned");
            recorded.serviced += 1;
            if let Some(id) = recorded.remove_session.take() {
                engine.sessions.retain(|s| s.id != id);
            }
            ServiceOutcome {
                mutated: recorded.mutated_next,
                stopped: false,
            }
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

    fn pr_reaction(status_op_id: Option<String>) -> EventReaction {
        EventReaction::OpenNewAgentPromptForPr {
            pr: Box::new(dux_core::worker::ResolvedPullRequest {
                project: crate::model::Project {
                    id: "p1".to_string(),
                    name: "p1".to_string(),
                    path: "/tmp/p1".to_string(),
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
                },
                host: "github.com".to_string(),
                owner_repo: "octocat/Hello-World".to_string(),
                number: 42,
                title: "Fix bug".to_string(),
                state: "OPEN".to_string(),
                head_ref_name: "feature/pr-42".to_string(),
                custom_name: None,
            }),
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
        app.companion = Some(Box::new(FakeCompanion {
            serving: false,
            recorded: Arc::clone(&recorded),
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
}
