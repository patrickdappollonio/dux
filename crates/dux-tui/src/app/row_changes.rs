//! The engine's entry points that change an agent's row, each behind the
//! question of who drives that row.
//!
//! One device drives a row at a time, and a surface that does not drive it
//! must not change it by any route: a key, a click, a dialog opened earlier, a
//! drag in flight, or work a worker finished later. So the question is asked
//! here, at the call that applies the change, and nowhere else in this crate
//! may make one of these calls: `the_engines_row_changes_are_named_only_here`
//! fails when another file names one. A refused call changes nothing, says on
//! the status line who drives the row, and returns `None`.
//!
//! A row with nothing running has no driver and no card, so it is never
//! refused: starting it is how this surface comes to drive it.

use anyhow::Result;
use dux_core::branch_history::BranchAnswersWire;
use dux_core::engine::{
    BranchRename, ChangeAgentProviderOutcome, CloseTabOutcome, Command, DetachSessionOutcome,
    EventReaction, ReconnectMode, ReconnectPlan, StartupRerunClaim, TabsAfterRecreate,
    WorktreeOnDelete,
};
use dux_core::model::{AgentSession, ProviderKind};
use dux_core::status_text::StatusText;
use dux_core::wire::{WireCommand, WireCommandOutcome};
use dux_core::worker::AgentLaunchRequest;

use super::{App, DockLayout, TerminalOwnerRef};

impl App {
    /// `session_id` when this surface may change its row; `None`, with the
    /// refusal on the status line, when it may not.
    fn permitted_row<'a>(&mut self, session_id: &'a str) -> Option<&'a str> {
        (!self.refuse_row_change_of(session_id)).then_some(session_id)
    }

    pub(crate) fn row_rename_agent(
        &mut self,
        session_id: &str,
        name: &str,
        rename: BranchRename,
    ) -> Option<Result<EventReaction>> {
        let session_id = self.permitted_row(session_id)?;
        Some(self.engine.rename_agent(session_id, name, rename))
    }

    pub(crate) fn row_create_tab(
        &mut self,
        session_id: &str,
        provider: ProviderKind,
        pty_size: (u16, u16),
    ) -> Option<Result<String>> {
        let session_id = self.permitted_row(session_id)?;
        Some(self.engine.create_tab(session_id, provider, pty_size))
    }

    pub(crate) fn row_close_tab(
        &mut self,
        session_id: &str,
        tab_id: &str,
    ) -> Option<Result<CloseTabOutcome>> {
        let session_id = self.permitted_row(session_id)?;
        Some(self.engine.close_tab(session_id, tab_id))
    }

    pub(crate) fn row_stop_tab(
        &mut self,
        session_id: &str,
        tab_id: &str,
    ) -> Option<Result<WireCommandOutcome>> {
        let session_id = self.permitted_row(session_id)?;
        Some(self.engine.apply_wire(WireCommand::StopAgentTab {
            session_id: session_id.to_string(),
            tab_id: tab_id.to_string(),
        }))
    }

    pub(crate) fn row_change_tab_provider(
        &mut self,
        session_id: &str,
        tab_id: &str,
        provider: &ProviderKind,
    ) -> Option<Result<ChangeAgentProviderOutcome>> {
        let session_id = self.permitted_row(session_id)?;
        Some(
            self.engine
                .change_tab_provider(session_id, tab_id, provider),
        )
    }

    /// Flip whether `session` reopens when dux starts.
    pub(crate) fn row_toggle_auto_reopen(
        &mut self,
        session: &AgentSession,
    ) -> Option<Result<EventReaction>> {
        let session_id = self.permitted_row(&session.id)?;
        Some(self.engine.apply(Command::ToggleAgentAutoReopen {
            branch_name: session.display_label(),
            session_id: session_id.to_string(),
            new_enabled: !session.auto_reopen_enabled,
        }))
    }

    pub(crate) fn row_begin_detach(&mut self, session_id: &str) -> Option<DetachSessionOutcome> {
        let session_id = self.permitted_row(session_id)?;
        Some(self.engine.begin_detach_session(session_id))
    }

    pub(crate) fn row_begin_delete(
        &mut self,
        session_id: &str,
        worktree: WorktreeOnDelete,
        delete_branch: Option<bool>,
        branches: Option<BranchAnswersWire>,
    ) -> Option<Result<EventReaction>> {
        let session_id = self.permitted_row(session_id)?;
        Some(self.engine.apply(Command::BeginDeleteSession {
            session_id: session_id.to_string(),
            delete_worktree: worktree == WorktreeOnDelete::Remove,
            delete_branch,
            branches,
        }))
    }

    pub(crate) fn row_begin_recreate(
        &mut self,
        session_id: &str,
        tabs: TabsAfterRecreate,
    ) -> Option<Result<EventReaction>> {
        let session_id = self.permitted_row(session_id)?;
        Some(self.engine.begin_recreate_working_copy(session_id, tabs))
    }

    pub(crate) fn row_claim_startup_rerun(
        &mut self,
        session_id: &str,
        label: &str,
        worktree_path: &str,
    ) -> Option<Result<StartupRerunClaim, StatusText>> {
        let session_id = self.permitted_row(session_id)?;
        Some(
            self.engine
                .claim_startup_rerun(session_id, label, worktree_path),
        )
    }

    pub(crate) fn row_reconnect_plan(
        &mut self,
        session_id: &str,
        mode: ReconnectMode,
        pty_size: (u16, u16),
    ) -> Option<Result<ReconnectPlan>> {
        let session_id = self.permitted_row(session_id)?;
        Some(self.engine.reconnect_plan(session_id, mode, pty_size))
    }

    pub(crate) fn row_dispatch_launch(
        &mut self,
        request: AgentLaunchRequest,
    ) -> Option<Result<EventReaction>> {
        let session_id = request.session.id.clone();
        self.permitted_row(&session_id)?;
        Some(self.engine.apply(Command::DispatchAgentLaunch {
            request: Box::new(request),
        }))
    }

    pub(crate) fn row_create_terminal(
        &mut self,
        session_id: &str,
        rows: u16,
        cols: u16,
    ) -> Option<Result<(String, String)>> {
        let session_id = self.permitted_row(session_id)?;
        Some(
            self.engine
                .create_companion_terminal(session_id, rows, cols),
        )
    }

    pub(crate) fn row_reorder_tabs(
        &mut self,
        session_id: &str,
        tab_ids: Vec<String>,
    ) -> Option<Result<EventReaction>> {
        let session_id = self.permitted_row(session_id)?;
        Some(self.engine.apply(Command::ReorderAgentTabs {
            session_id: session_id.to_string(),
            tab_ids,
        }))
    }

    /// Store the order of every terminal after `session_id`'s dock reordered.
    pub(crate) fn row_reorder_dock(
        &mut self,
        session_id: &str,
        terminal_ids: Vec<String>,
    ) -> Option<Result<EventReaction>> {
        self.permitted_row(session_id)?;
        Some(
            self.engine
                .apply(Command::ReorderTerminals { terminal_ids }),
        )
    }

    /// Store the order of every terminal after the list's own rows reordered:
    /// no agent's dock changes, so no row is asked about.
    pub(crate) fn reorder_list_terminals(
        &mut self,
        terminal_ids: Vec<String>,
    ) -> Result<EventReaction> {
        self.engine
            .apply(Command::ReorderTerminals { terminal_ids })
    }

    /// Close `terminal_id`, asking about its agent's row when it has one: a
    /// project or standalone terminal is a row of its own.
    pub(crate) fn row_close_terminal(
        &mut self,
        terminal_id: &str,
    ) -> Option<Result<EventReaction>> {
        let agent = self
            .engine
            .companion_terminals
            .get(terminal_id)
            .and_then(|terminal| match terminal.owner.as_ref() {
                TerminalOwnerRef::Session(session_id) => Some(session_id.to_string()),
                TerminalOwnerRef::Project(_) | TerminalOwnerRef::Standalone => None,
            });
        if let Some(session_id) = agent {
            self.permitted_row(&session_id)?;
        }
        Some(self.engine.apply(Command::DeleteTerminal {
            terminal_id: terminal_id.to_string(),
        }))
    }

    /// Remember `layout` as how `session_id` shares the center pane with its
    /// terminals.
    pub(crate) fn row_remember_layout(
        &mut self,
        session_id: &str,
        layout: DockLayout,
    ) -> Option<()> {
        let session_id = self.permitted_row(session_id)?;
        self.dock_layouts.insert(session_id.to_string(), layout);
        Some(())
    }
}

#[cfg(test)]
mod tests {
    /// What changes an agent's row: the engine's entry points and this
    /// window's own layout memory, as their calls read with whitespace removed.
    const ROW_CHANGES: &[&str] = &[
        ".rename_agent(",
        ".create_tab(",
        ".close_tab(",
        ".change_tab_provider(",
        ".change_agent_provider(",
        ".begin_detach_session(",
        ".begin_recreate_working_copy(",
        ".claim_startup_rerun(",
        ".reconnect_plan(",
        ".create_companion_terminal(",
        "Command::BeginDeleteSession",
        "Command::DeleteTerminal",
        "Command::ToggleAgentAutoReopen",
        "Command::ReorderAgentTabs",
        "Command::ReorderTerminals",
        "Command::DispatchAgentLaunch",
        "WireCommand::StopAgentTab",
        "dock_layouts.insert(",
    ];

    /// A file's code outside its test module, without comment lines or whitespace.
    fn code_outside_tests(source: &str) -> String {
        let mut code = String::new();
        let mut lines = source.lines().peekable();
        while let Some(line) = lines.next() {
            let opens_tests = line.trim() == "#[cfg(test)]"
                && lines
                    .peek()
                    .is_some_and(|next| next.trim_end().ends_with("mod tests {"));
            if opens_tests {
                break;
            }
            if !line.trim_start().starts_with("//") {
                code.extend(line.chars().filter(|c| !c.is_whitespace()));
            }
        }
        code
    }

    fn rust_files(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("the source directory") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                rust_files(&path, found);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                found.push(path);
            }
        }
    }

    /// The structure that keeps a row from being changed by a surface that
    /// does not drive it: everything that changes an agent's row is called
    /// from this file alone, behind the question of who drives the row, so a
    /// dialog, gesture or worker added later cannot reach one without asking.
    /// A new call belongs behind a `row_*` method here, never on this list.
    #[test]
    fn the_engines_row_changes_are_named_only_here() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rust_files(&root, &mut files);
        assert!(files.len() > 20, "the crate's sources were found");
        let mut strays = Vec::new();
        for file in files {
            // Modules compiled only for tests build their own fixtures.
            let name = file.file_name().and_then(|name| name.to_str());
            if matches!(name, Some("row_changes.rs" | "test_support.rs")) {
                continue;
            }
            let code = code_outside_tests(&std::fs::read_to_string(&file).expect("a source file"));
            for change in ROW_CHANGES {
                if code.contains(change) {
                    strays.push(format!("{} names {change}", file.display()));
                }
            }
        }
        assert!(strays.is_empty(), "{strays:#?}");
    }
}
