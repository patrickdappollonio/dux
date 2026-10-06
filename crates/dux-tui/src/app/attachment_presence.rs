//! This surface in the attachment registry ([`dux_core::attachments`]).
//!
//! The terminal UI is one connection, and it is attached to exactly the
//! terminals it drew in its last frame, focused or not: an agent that is
//! selected but whose pane is not on screen (a diff fills the center, or the
//! server log covers everything) protects nothing. A change somebody else asks
//! for (the command line, a browser) is refused while it would end a pane on
//! screen here; a change asked for here is exempt from this surface's own
//! panes and from nobody else's.
//!
//! Quitting the terminal UI ends every terminal it started, so the quit
//! confirmation asks whenever another device is attached to one, and says who
//! ([`Engine::attached_elsewhere`](dux_core::engine::Engine::attached_elsewhere)).

use super::*;
use dux_core::attachments::{ConnectionFacts, Surface, TERMINAL_UI_CONNECTION, Target, TargetKind};

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
    /// showed. Cheap when nothing changed, which is every frame but the ones
    /// that open or close a pane.
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

    /// This surface stopped drawing anything: it is leaving the terminal (a
    /// quit, or the flip to the server).
    pub(super) fn release_drawn_attachments(&mut self) {
        self.engine.attachments.deregister(TERMINAL_UI_CONNECTION);
        self.drawn_ptys.clear();
        self.published_ptys = None;
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
}
