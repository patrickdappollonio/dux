//! This surface in the attachment registry ([`dux_core::attachments`]).
//!
//! The terminal UI is one connection, attached to exactly the terminals it
//! drew in its last frame: a selected agent whose pane is off screen protects
//! nothing. A change asked for elsewhere is refused while it would end a pane
//! on screen here; one asked for here is exempt from this surface's own panes
//! only. Quitting ends every terminal this surface started, so the quit
//! confirmation names anyone else attached to one
//! ([`Engine::attached_elsewhere`](dux_core::engine::Engine::attached_elsewhere)).

use super::*;
use dux_core::attachments::{
    Blocker, ConnectionFacts, Surface, TERMINAL_UI_CONNECTION, Target, TargetKind,
};
use dux_core::engine::Attached;

impl App {
    /// The pane being drawn right now streams this surface's selected
    /// terminal: remember it for this frame's attachments.
    pub(super) fn note_drawn_pty(&mut self) {
        let Some(id) = self.selected_terminal_surface_id() else {
            return;
        };
        let (kind, agent) = match self.session_surface {
            SessionSurface::Agent => (
                TargetKind::Tab,
                self.selected_session().map(|s| s.id.clone()),
            ),
            SessionSurface::Terminal => (
                TargetKind::Terminal,
                self.engine
                    .companion_terminals
                    .get(&id)
                    .and_then(|terminal| match &terminal.owner {
                        TerminalOwner::Session(session_id) => Some(session_id.clone()),
                        TerminalOwner::Project(_) | TerminalOwner::Standalone => None,
                    }),
            ),
        };
        let target = Target { kind, id, agent };
        if !self.drawn_ptys.contains(&target) {
            self.drawn_ptys.push(target);
        }
    }

    /// Make this surface's attachments exactly what the frame just drawn
    /// showed; a frame that opened or closed no pane changes nothing.
    pub(super) fn publish_drawn_attachments(&mut self) {
        let pty_conn = self.pty_ownership_conn_id();
        let drawn = (std::mem::take(&mut self.drawn_ptys), pty_conn);
        if self.published_ptys.as_ref() == Some(&drawn) {
            self.drawn_ptys = drawn.0;
            return;
        }
        let attachments = &self.engine.attachments;
        attachments.register(
            TERMINAL_UI_CONNECTION,
            ConnectionFacts {
                surface: Surface::TerminalUi,
                device: Some(dux_core::background_serve::TUI_DEVICE_LABEL.to_string()),
                address: None,
                verified: true,
                events: false,
            },
            None,
        );
        attachments.set_terminal_ui(drawn.0.clone(), drawn.1);
        self.drawn_ptys = drawn.0.clone();
        self.published_ptys = Some(drawn);
    }

    /// What stopping `targets` from the running-processes list would end:
    /// an agent target is its first tab, the others the tab or terminal named.
    pub(super) fn kill_running_scope(
        &self,
        targets: &[RuntimeTargetId],
    ) -> dux_core::attachments::Scope {
        let mut scope = dux_core::attachments::Scope::default();
        for target in targets {
            scope.ptys.insert(match target {
                RuntimeTargetId::Agent(session_id) => self
                    .engine
                    .slot_tab_id_of(SessionIdRef::new(session_id))
                    .to_string(),
                RuntimeTargetId::Tab(id) | RuntimeTargetId::Terminal(id) => id.clone(),
            });
        }
        scope
    }

    /// Run the confirm of `asked`: as the override when its dialog already
    /// names who is attached, otherwise as a plain confirm the guard may refuse.
    pub(crate) fn guarded_by<R>(
        &mut self,
        asked: &PromptState,
        act: impl FnOnce(&mut Self) -> R,
    ) -> R {
        if asked.attached().is_empty() {
            act(self)
        } else {
            self.over_shown_attached(asked.attached(), act)
        }
    }

    /// Run `act` over `shown`, the blockers the dialog named: the guard still
    /// reserves what it ends, and refuses again when anybody else is in the way.
    fn over_shown_attached<R>(&mut self, shown: &[Blocker], act: impl FnOnce(&mut Self) -> R) -> R {
        let forced = dux_core::attachments::Policy {
            requester: Some(TERMINAL_UI_CONNECTION.to_string()),
            force: true,
            accepted: Some(shown.iter().map(Blocker::key).collect()),
        };
        let previous = self.engine.dispatch_policy.replace(forced);
        let result = act(self);
        self.engine.dispatch_policy = previous;
        result
    }

    /// Reopen `asked` naming who refused it, focused on Cancel so the keystroke
    /// that confirmed cannot also go ahead over them.
    pub(crate) fn reopen_naming_attached(&mut self, mut asked: PromptState, refused: Attached) {
        asked.name_attached(refused.blockers);
        self.prompt = asked;
        self.forget_buttons_until_redrawn();
    }

    /// Keep an open dialog's attached list live, focus back on Cancel when it
    /// changes; returns whether it did. A dialog naming nobody is not followed.
    pub(crate) fn refresh_attached_dialog(&mut self) -> bool {
        if self.prompt.attached().is_empty() {
            return false;
        }
        let Some(now) = self.attached_now() else {
            return false;
        };
        let keys =
            |blockers: &[Blocker]| -> Vec<String> { blockers.iter().map(Blocker::key).collect() };
        if keys(&now) == keys(self.prompt.attached()) {
            return false;
        }
        self.prompt.name_attached(now);
        self.forget_buttons_until_redrawn();
        true
    }

    /// Everybody else attached to what the open guarded dialog would end.
    fn attached_now(&self) -> Option<Vec<Blocker>> {
        let scope = match &self.prompt {
            PromptState::ConfirmDeleteAgent { session_id, .. }
            | PromptState::ConfirmDetachAgent { session_id, .. } => {
                self.engine.agent_scope(session_id)
            }
            PromptState::ConfirmDeleteTerminal { terminal_id, .. } => {
                dux_core::engine::Engine::pty_scope(terminal_id)
            }
            PromptState::ConfirmCloseTab { tab_id, .. }
            | PromptState::ConfirmStopTab { tab_id, .. } => {
                dux_core::engine::Engine::pty_scope(tab_id)
            }
            PromptState::ConfirmDeleteProject { project_id, .. }
            | PromptState::ConfirmRemoveProject { project_id, .. } => {
                self.engine.project_scope(project_id)
            }
            PromptState::ConfirmKillRunning(confirm) => {
                self.kill_running_scope(&confirm.target_ids)
            }
            PromptState::ConfirmQuit { .. } => return Some(self.engine.attached_elsewhere()),
            _ => return None,
        };
        Some(self.engine.attachments.blockers(
            &scope,
            Some(TERMINAL_UI_CONNECTION),
            std::time::Instant::now(),
        ))
    }

    /// Drop the last frame's button rects and any press in flight, so the rest
    /// of this input batch (a double click's second click) lands on nothing.
    pub(crate) fn forget_buttons_until_redrawn(&mut self) {
        self.overlay_layout.reset();
        self.pressed_button = None;
        self.mark_frame_dirty();
    }

    pub(crate) fn settle_guarded(&mut self, asked: PromptState, outcome: Result<()>) {
        if let Err(error) = outcome {
            match attached_refusal(error) {
                Ok(refused) => self.reopen_naming_attached(asked, refused),
                Err(error) => self.set_error(format!("{error:#}")),
            }
        }
    }

    /// This surface stopped drawing anything: it is leaving the terminal (a
    /// quit, or the flip to the server).
    pub(super) fn release_drawn_attachments(&mut self) {
        self.engine.attachments.set_terminal_ui(Vec::new(), None);
        self.engine.attachments.deregister(
            TERMINAL_UI_CONNECTION,
            dux_core::attachments::Ending::Deliberate,
        );
        self.drawn_ptys.clear();
        self.published_ptys = None;
    }
}

/// A refusal from a guarded change, when that is what `error` is.
pub(crate) fn attached_refusal(error: anyhow::Error) -> Result<Attached, anyhow::Error> {
    error.downcast::<Attached>()
}

impl PromptState {
    /// Who the guard named when it refused this dialog's confirm: nobody until
    /// it has, and nobody for a dialog the guard does not cover.
    pub(crate) fn attached(&self) -> &[Blocker] {
        match self {
            PromptState::ConfirmDeleteAgent { attached, .. }
            | PromptState::ConfirmDeleteTerminal { attached, .. }
            | PromptState::ConfirmCloseTab { attached, .. }
            | PromptState::ConfirmStopTab { attached, .. }
            | PromptState::ConfirmDetachAgent { attached, .. }
            | PromptState::ConfirmDeleteProject { attached, .. }
            | PromptState::ConfirmRemoveProject { attached, .. }
            | PromptState::ConfirmQuit { attached, .. } => attached,
            PromptState::ConfirmKillRunning(confirm) => &confirm.attached,
            _ => &[],
        }
    }

    /// Name `blockers` in this dialog and put focus back on Cancel.
    pub(crate) fn name_attached(&mut self, blockers: Vec<Blocker>) {
        match self {
            PromptState::ConfirmDeleteAgent {
                attached, focus, ..
            } => {
                *attached = blockers;
                *focus = DeleteAgentFocus::Cancel;
            }
            PromptState::ConfirmKillRunning(confirm) => {
                confirm.attached = blockers;
                confirm.focus = ConfirmFocus::Cancel;
            }
            PromptState::ConfirmDeleteTerminal {
                attached, focus, ..
            }
            | PromptState::ConfirmCloseTab {
                attached, focus, ..
            }
            | PromptState::ConfirmStopTab {
                attached, focus, ..
            }
            | PromptState::ConfirmDetachAgent {
                attached, focus, ..
            }
            | PromptState::ConfirmDeleteProject {
                attached, focus, ..
            }
            | PromptState::ConfirmRemoveProject {
                attached, focus, ..
            }
            | PromptState::ConfirmQuit {
                attached, focus, ..
            } => {
                *attached = blockers;
                *focus = ConfirmFocus::Cancel;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::{default_bindings, test_app};
    use dux_core::attachments::Policy;

    fn render(app: &mut App) {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).expect("terminal");
        terminal.draw(|frame| app.render(frame)).expect("render");
    }

    /// A dialog naming who is attached keeps its list live: somebody new
    /// joins it (with focus back on Cancel, since the override would now cut
    /// off someone the person has not seen yet), and once everybody has gone
    /// the confirm is the plain one again.
    #[test]
    fn a_dialog_naming_who_is_attached_follows_them_while_it_is_open() {
        let mut app = test_app(default_bindings());
        let session_id = app.engine.sessions[0].id.clone();
        let slot_tab = app.engine.sessions[0].slot_tab_id().to_string();
        let first = crate::app::test_support::watch_from_a_browser(&app, &slot_tab, &session_id);
        app.confirm_delete_selected_session()
            .expect("open the delete dialog");
        app.resolve_confirm_delete_agent(true);
        assert_eq!(app.prompt.attached().len(), 1);
        if let PromptState::ConfirmDeleteAgent { focus, .. } = &mut app.prompt {
            *focus = DeleteAgentFocus::Delete;
        }
        assert!(!app.refresh_attached_dialog(), "nothing moved");

        app.engine.attachments.register(
            "second-browser",
            ConnectionFacts {
                surface: Surface::Browser,
                device: Some("Safari".to_string()),
                address: Some("10.0.0.8".parse().unwrap()),
                verified: false,
                events: true,
            },
            None,
        );
        let second = app
            .engine
            .attachments
            .attach(
                "second-browser",
                Target {
                    kind: TargetKind::Tab,
                    id: slot_tab.clone(),
                    agent: Some(session_id.clone()),
                },
                None,
                None,
            )
            .unwrap();
        assert!(app.refresh_attached_dialog());
        let PromptState::ConfirmDeleteAgent {
            attached, focus, ..
        } = &app.prompt
        else {
            panic!("the dialog stays open");
        };
        assert_eq!(attached.len(), 2);
        assert_eq!(*focus, DeleteAgentFocus::Cancel);

        crate::app::test_support::stop_watching(&app, first);
        crate::app::test_support::stop_watching(&app, second);
        assert!(app.refresh_attached_dialog());
        assert!(app.prompt.attached().is_empty(), "{:?}", app.prompt);
    }

    /// A delete the command line asks for is refused while this surface draws
    /// the agent's pane, naming this surface; once the pane is not drawn (a
    /// diff fills the center), the same agent, still selected, protects
    /// nothing.
    #[test]
    fn a_drawn_pane_blocks_a_command_line_delete_and_a_selected_undrawn_one_does_not() {
        let mut app = test_app(default_bindings());
        let session_id = app.engine.sessions[0].id.clone();
        let worktree = std::path::PathBuf::from(
            app.engine.sessions[0]
                .managed_worktree()
                .expect("managed test session"),
        );
        let slot_tab = app.engine.sessions[0].slot_tab_id().to_string();
        let args = vec!["-c".to_string(), "sleep 30".to_string()];
        let client =
            PtyClient::spawn("/bin/sh", &args, &worktree, 24, 80, 1_000).expect("spawn pty");
        app.engine
            .providers
            .insert(TabId::new(slot_tab.clone()), client);
        assert_eq!(
            app.selected_session().map(|s| s.id.clone()),
            Some(session_id.clone())
        );

        render(&mut app);
        app.engine.dispatch_policy = Some(Policy::default());
        let refused = match app
            .engine
            .reserve_destruction(app.engine.agent_scope(&session_id))
        {
            Ok(_) => panic!("the drawn pane did not block the delete"),
            Err(refused) => refused,
        };
        assert_eq!(refused.blockers.len(), 1);
        assert_eq!(refused.blockers[0].surface, Surface::TerminalUi);
        assert_eq!(refused.blockers[0].target.id, slot_tab);

        app.center_mode = CenterMode::Diff {
            lines: std::sync::Arc::new(Vec::new()),
            scroll: 0,
            gutter_width: 0,
            worktree_path: String::new(),
            rel_path: "a.rs".to_string(),
        };
        render(&mut app);
        assert_eq!(
            app.selected_session().map(|s| s.id.clone()),
            Some(session_id.clone())
        );
        assert!(
            app.engine
                .reserve_destruction(app.engine.agent_scope(&session_id))
                .is_ok()
        );
    }

    fn spawn_sleeper() -> PtyClient {
        PtyClient::spawn(
            "/bin/sh",
            &["-c".to_string(), "sleep 30".to_string()],
            std::path::Path::new("."),
            24,
            80,
            1_000,
        )
        .expect("spawn pty")
    }

    /// The ids of what this surface is attached to.
    fn attached_here(app: &App) -> Vec<String> {
        app.engine
            .attachments
            .connections(std::time::Instant::now())
            .into_iter()
            .filter(|connection| connection.surface == Surface::TerminalUi)
            .flat_map(|connection| connection.attachments)
            .map(|attachment| attachment.target.id)
            .collect()
    }

    /// A ready view a browser's launch of `session` produced.
    fn browser_ready(
        session: AgentSession,
        view: dux_core::engine::AgentLaunchReadyView,
    ) -> dux_core::engine::AgentLaunchReadyOutcome {
        dux_core::engine::AgentLaunchReadyOutcome {
            tab_id: session.slot_tab_id().to_string(),
            session,
            pty_size: (24, 80),
            detached_session_id: None,
            wants_fullscreen: false,
            status_quiet: dux_core::statusline::QuietSurfaces::LOUD,
            view,
        }
    }

    /// An agent a browser creates or starts lands on the browser, not here:
    /// this surface keeps drawing the agent it was on, so deleting the
    /// browser's agent from anywhere is not refused in this surface's name,
    /// while the pane this surface really draws still is. The create goes
    /// through the worker event the engine really processes, which puts the
    /// new agent at the head of the session list before anything here runs.
    #[test]
    fn an_agent_a_browser_launches_is_not_drawn_or_guarded_here() {
        use dux_core::engine::AgentLaunchReadyView;

        enum Launch {
            Create,
            Ready(AgentLaunchReadyView),
        }
        let launches = [
            Launch::Create,
            Launch::Ready(AgentLaunchReadyView::Reconnect {
                status_message: "Launched agent.".to_string().into(),
            }),
            Launch::Ready(AgentLaunchReadyView::ResumeFallback {
                session_id: "from-the-browser".to_string(),
                status_message: "Started fresh.".to_string().into(),
            }),
        ];
        for launch in launches {
            let mut app = test_app(default_bindings());
            let (companion, _recorded) =
                crate::app::background_server::tests::FakeCompanion::serving();
            app.companion = Some(companion);
            let shown = app.engine.sessions[0].id.clone();
            app.engine
                .mark_session_status(&shown, crate::model::SessionStatus::Active);
            app.engine
                .providers
                .insert(TabId::new("session-1-slot".to_string()), spawn_sleeper());
            let folder = tempfile::tempdir().expect("tempdir");
            let mut browsers = app.engine.sessions[0].clone();
            browsers.id = "from-the-browser".to_string();
            browsers.slot_tab_id = "from-the-browser-slot".to_string();
            if let dux_core::model::AgentWorkspace::Managed(managed) = &mut browsers.workspace {
                managed.worktree_path = folder.path().to_string_lossy().to_string();
            }
            if matches!(launch, Launch::Ready(_)) {
                browsers.status = crate::model::SessionStatus::Detached;
                app.engine.sessions.push(browsers.clone());
            }
            app.rebuild_left_items();
            render(&mut app);

            match launch {
                Launch::Create => {
                    let request = app.agent_launch_request(
                        browsers,
                        false,
                        dux_core::worker::AgentLaunchKind::Create {
                            status_message: "Created agent.".to_string().into(),
                            status_warns: false,
                            status_notes: None,
                            pull_request_pin: None,
                            repo_path: app.engine.projects[0].path.clone(),
                            owns_worktree: false,
                            startup_result: None,
                            status_op_id: String::new(),
                        },
                    );
                    app.engine
                        .worker_tx
                        .send(WorkerEvent::AgentLaunchReady(Box::new(
                            crate::app::AgentLaunchReadyData {
                                request,
                                client: spawn_sleeper(),
                                spawn_ticket: None,
                            },
                        )))
                        .expect("send the launch");
                    app.drain_events();
                    assert_eq!(
                        app.engine.sessions[0].id, "from-the-browser",
                        "the engine puts a new agent at the head of the list"
                    );
                }
                Launch::Ready(view) => {
                    app.engine.providers.insert(
                        TabId::new("from-the-browser-slot".to_string()),
                        spawn_sleeper(),
                    );
                    app.engine.mark_session_status(
                        "from-the-browser",
                        crate::model::SessionStatus::Active,
                    );
                    app.apply_agent_launch_ready_view(browser_ready(browsers, view));
                }
            }
            render(&mut app);

            assert_eq!(
                app.selected_session().map(|s| s.id.as_str()),
                Some("session-1"),
                "a browser's launch must not move this surface's selection"
            );
            assert_eq!(attached_here(&app), vec!["session-1-slot".to_string()]);
            app.engine.dispatch_policy = Some(Policy::default());
            assert!(
                app.engine
                    .reserve_destruction(app.engine.agent_scope("from-the-browser"))
                    .is_ok(),
                "deleting the browser's agent is not refused for this surface"
            );
            let refused = match app
                .engine
                .reserve_destruction(app.engine.agent_scope("session-1"))
            {
                Ok(_) => panic!("the pane drawn here did not block its delete"),
                Err(refused) => refused,
            };
            assert_eq!(refused.blockers.len(), 1);
            assert_eq!(refused.blockers[0].surface, Surface::TerminalUi);
            assert_eq!(refused.blockers[0].target.id, "session-1-slot");
        }
    }

    /// A browser resuming the agent this surface's cursor still remembers
    /// leaves the terminal this surface shows where it is: the fallback launch
    /// is the browser's, so nothing here lands on it.
    #[test]
    fn a_browsers_resume_fallback_leaves_the_terminal_shown_here() {
        use dux_core::engine::AgentLaunchReadyView;

        let mut app = test_app(default_bindings());
        app.engine.config.terminal.command = "cat".to_string();
        app.engine.config.terminal.args = vec![];
        app.rebuild_left_items();
        app.reselect_left_session("session-1");
        let (terminal, _) = app
            .engine
            .create_standalone_terminal(24, 80)
            .expect("standalone terminal");
        app.left_section = LeftSection::Terminals;
        app.selected_terminal_index = 0;
        app.open_terminal_from_terminal_list()
            .expect("open the terminal");
        app.fullscreen_overlay = FullscreenOverlay::Terminal;
        render(&mut app);
        assert_eq!(attached_here(&app), vec![terminal.clone()]);

        let resumed = app.engine.sessions[0].clone();
        app.engine
            .providers
            .insert(TabId::new("session-1-slot".to_string()), spawn_sleeper());
        app.engine
            .mark_session_status("session-1", crate::model::SessionStatus::Active);
        app.apply_agent_launch_ready_view(browser_ready(
            resumed,
            AgentLaunchReadyView::ResumeFallback {
                session_id: "session-1".to_string(),
                status_message: "Started fresh.".to_string().into(),
            },
        ));
        render(&mut app);

        assert_eq!(app.session_surface, SessionSurface::Terminal);
        assert_eq!(app.active_terminal_id.as_deref(), Some(terminal.as_str()));
        assert_eq!(app.fullscreen_overlay, FullscreenOverlay::Terminal);
        assert_eq!(attached_here(&app), vec![terminal]);
    }

    /// A cursor on no agent stays on no agent when a browser starts the agent
    /// whose dormant tail it was resting on: the row it was on is gone, and the
    /// agent that took its place is not one it was on.
    #[test]
    fn a_cursor_on_the_inactive_toggle_does_not_land_on_an_agent_a_browser_starts() {
        use dux_core::engine::AgentLaunchReadyView;

        let mut app = test_app(default_bindings());
        app.rebuild_left_items();
        let toggle = app
            .left_items()
            .iter()
            .position(|item| matches!(item, LeftItem::InactiveToggle))
            .expect("a dormant agent sits under the Inactive toggle");
        app.selected_left = toggle;
        render(&mut app);
        assert!(attached_here(&app).is_empty());

        let started = app.engine.sessions[0].clone();
        app.engine
            .providers
            .insert(TabId::new("session-1-slot".to_string()), spawn_sleeper());
        app.engine
            .mark_session_status("session-1", crate::model::SessionStatus::Active);
        app.apply_agent_launch_ready_view(browser_ready(
            started,
            AgentLaunchReadyView::Reconnect {
                status_message: "Launched agent.".to_string().into(),
            },
        ));
        render(&mut app);

        assert_eq!(app.selected_session().map(|s| s.id.as_str()), None);
        assert!(attached_here(&app).is_empty(), "{:?}", attached_here(&app));
    }
}
