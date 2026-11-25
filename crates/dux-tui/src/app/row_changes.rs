//! The engine's entry points that change an agent's row, each behind the
//! question of who drives that row.
//!
//! One device drives a row at a time, and a surface that does not drive it
//! must not change it by any route: a key, a click, a dialog opened earlier, a
//! drag in flight, or work a worker finished later. So the question is asked
//! here, at the call that applies the change, and nowhere else in this crate
//! may make one of these calls: `the_engines_row_changes_are_named_only_here`
//! fails when another file names one of the engine's row-changing methods or
//! `Command` or `WireCommand` variants, renames or imports those variants, or
//! writes this window's layout or front-terminal memory, outside the
//! exceptions it lists with a reason. It reads this crate's source, so a call
//! made through another crate or a macro is beyond it. A refused call changes
//! nothing, says on the status line who drives the row, and returns `None`.
//!
//! A row with nothing running has no driver and no card, so it is never
//! refused: starting it is how this surface comes to drive it.

use std::path::{Path, PathBuf};

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
use dux_core::worker::{AgentLaunchRequest, PullTarget};
use dux_core::worktree_ops::WorktreeOpGuard;

use super::{App, DockLayout};

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
        delete_folder: Option<bool>,
    ) -> Option<Result<EventReaction>> {
        let session_id = self.permitted_row(session_id)?;
        Some(self.engine.apply(Command::BeginDeleteSession {
            session_id: session_id.to_string(),
            delete_worktree: worktree == WorktreeOnDelete::Remove,
            delete_branch,
            branches,
            delete_folder,
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
            .and_then(super::row_card::docking_agent);
        if let Some(session_id) = agent {
            self.permitted_row(&session_id)?;
        }
        Some(self.engine.apply(Command::DeleteTerminal {
            terminal_id: terminal_id.to_string(),
        }))
    }

    pub(crate) fn row_stage_file(
        &mut self,
        session_id: &str,
        worktree_path: PathBuf,
        path: String,
    ) -> Option<Result<EventReaction>> {
        self.permitted_row(session_id)?;
        Some(self.engine.apply(Command::StageFile {
            worktree_path,
            path,
        }))
    }

    pub(crate) fn row_unstage_file(
        &mut self,
        session_id: &str,
        worktree_path: PathBuf,
        path: String,
    ) -> Option<Result<EventReaction>> {
        self.permitted_row(session_id)?;
        Some(self.engine.apply(Command::UnstageFile {
            worktree_path,
            path,
        }))
    }

    pub(crate) fn row_discard_file(
        &mut self,
        session_id: &str,
        worktree_path: PathBuf,
        path: String,
    ) -> Option<Result<EventReaction>> {
        self.permitted_row(session_id)?;
        Some(self.engine.apply(Command::DiscardFileLive {
            worktree_path,
            path,
        }))
    }

    /// Hold `worktree` for a whole-folder stage, unstage or delete that runs on
    /// this crate's own worker rather than through the engine.
    pub(crate) fn row_hold_for_git_change(
        &mut self,
        session_id: &str,
        worktree: &Path,
        what: &str,
    ) -> Option<Result<WorktreeOpGuard>> {
        self.permitted_row(session_id)?;
        Some(self.engine.hold_for_git_change(worktree, what))
    }

    pub(crate) fn row_commit(
        &mut self,
        session_id: &str,
        worktree_path: PathBuf,
        message: String,
        success_message: String,
    ) -> Option<Result<EventReaction>> {
        self.permitted_row(session_id)?;
        Some(self.engine.apply(Command::CommitChanges {
            worktree_path,
            message,
            success_message,
        }))
    }

    pub(crate) fn row_push(
        &mut self,
        session_id: &str,
        worktree_path: PathBuf,
    ) -> Option<Result<EventReaction>> {
        self.permitted_row(session_id)?;
        Some(self.engine.apply(Command::Push { worktree_path }))
    }

    pub(crate) fn row_pull(
        &mut self,
        session_id: &str,
        repo_path: PathBuf,
        busy_message: StatusText,
        already_running_message: StatusText,
    ) -> Option<Result<EventReaction>> {
        self.permitted_row(session_id)?;
        Some(self.engine.apply(Command::Pull {
            repo_path,
            target: PullTarget::Session,
            busy_message,
            already_running_message,
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
    /// The engine's methods that change an agent's row.
    const ENGINE_CALLS: &[&str] = &[
        "rename_agent",
        "create_tab",
        "close_tab",
        "change_tab_provider",
        "change_agent_provider",
        "begin_detach_session",
        "begin_recreate_working_copy",
        "claim_startup_rerun",
        "reconnect_plan",
        "create_companion_terminal",
        "kill_tab_runtime",
        "begin_close_companion_terminal",
        "hold_for_git_change",
    ];

    /// The variants of the engine's `Command` that change an agent's row.
    const COMMANDS: &[&str] = &[
        "BeginDeleteSession",
        "FinishDeleteSession",
        "DispatchAgentLaunch",
        "ToggleAgentAutoReopen",
        "DeleteTerminal",
        "ReorderTerminals",
        "ReorderAgentTabs",
        "StageFile",
        "UnstageFile",
        "DiscardFile",
        "DiscardFileLive",
        "CommitChanges",
        "Push",
        "Pull",
    ];

    /// The variants of `WireCommand`, the engine's other entry point, that
    /// change an agent's row.
    const WIRE_COMMANDS: &[&str] = &[
        "StopAgentTab",
        "CloseAgentTab",
        "ChangeAgentProvider",
        "ChangeAgentTabProvider",
        "DeleteSession",
        "DetachAgent",
        "RecreateWorkingCopy",
        "RenameSession",
        "ReconnectSession",
        "RerunStartupCommand",
        "DeleteTerminal",
        "KillSessionPty",
        "ReorderAgentTabs",
        "ReorderTerminals",
        "ToggleAgentAutoReopen",
        "StageFile",
        "UnstageFile",
        "DiscardFile",
        "CommitChanges",
        "Push",
        "Pull",
    ];

    /// This window's own memory of an agent's row: its layout and the
    /// terminal in front of its dock.
    const ROW_STATE: &[&str] = &["dock_layouts", "dock_active"];

    /// The methods that only read a map.
    const READS: &[&str] = &[
        "get",
        "contains_key",
        "iter",
        "keys",
        "values",
        "len",
        "is_empty",
    ];

    /// What may stay outside this file, each for its reason: (file, finding).
    const OUTSIDE: &[(&str, &str, &str)] = &[
        (
            "sessions.rs",
            "names kill_tab_runtime",
            "the kill-running list, deliberately not gated by the card",
        ),
        (
            "sessions.rs",
            "names begin_close_companion_terminal",
            "the kill-running list, deliberately not gated by the card",
        ),
        (
            "sessions.rs",
            "names Command::FinishDeleteSession",
            "finishes a delete `row_begin_delete` admitted; refusing it would strand a \
             half-deleted agent",
        ),
        (
            "sessions.rs",
            "names Command::Pull",
            "pulls a project, deliberately not gated by the card",
        ),
        (
            "dock.rs",
            "writes dock_active",
            "remembers the terminal shown in front, which is viewing, like selecting a tab",
        ),
        (
            "mod.rs",
            "writes dock_active",
            "remembers the terminal shown in front, which is viewing, like selecting a tab",
        ),
        (
            "sessions.rs",
            "writes dock_active",
            "remembers the terminal a new-terminal command opened, after asking who drives",
        ),
    ];

    /// A file's code outside its test module, without comment lines, each run
    /// of whitespace one space.
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
                code.push(' ');
                code.push_str(line);
            }
        }
        code.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    fn is_ident(c: char) -> bool {
        c.is_ascii_alphanumeric() || c == '_'
    }

    /// Every identifier in `code` with the text before and after it.
    fn identifiers(code: &str) -> Vec<(&str, &str, &str)> {
        let mut found = Vec::new();
        let mut start = None;
        for (at, c) in code.char_indices().chain([(code.len(), ' ')]) {
            match (is_ident(c), start) {
                (true, None) => start = Some(at),
                (false, Some(begun)) => {
                    found.push((&code[..begun], &code[begun..at], &code[at..]));
                    start = None;
                }
                _ => {}
            }
        }
        found
    }

    /// The row changes `code` makes or could hide, in words.
    fn row_changes_in(code: &str) -> Vec<String> {
        let mut found = Vec::new();
        for (before, word, after) in identifiers(code) {
            let (before, after) = (before.trim_end(), after.trim_start());
            if ENGINE_CALLS.contains(&word) {
                found.push(format!("names {word}"));
            }
            let variants = match word {
                "Command" => COMMANDS,
                "WireCommand" => WIRE_COMMANDS,
                _ => &[],
            };
            if !variants.is_empty() {
                let variant = after
                    .strip_prefix("::")
                    .map(|rest| rest.split(|c| !is_ident(c)).next().unwrap_or_default());
                if variant.is_some_and(|variant| variants.contains(&variant)) {
                    found.push(format!("names {word}::{}", variant.unwrap_or_default()));
                }
                // A variant imported bare or an enum under another name would
                // slip past the spelling above.
                let path = before
                    .trim_end_matches(|c: char| is_ident(c) || c == ':')
                    .trim_end();
                let aliased = after.starts_with("as ")
                    || after.starts_with(';')
                        && path.ends_with('=')
                        && !path.ends_with("==")
                        && !path.ends_with("!=");
                if after.starts_with("::{") || after.starts_with("::*") || aliased {
                    found.push(format!("renames or imports the variants of {word}"));
                }
            }
            if ROW_STATE.contains(&word) {
                let method = after
                    .strip_prefix('.')
                    .map(|rest| rest.split(|c| !is_ident(c)).next().unwrap_or_default());
                let assigned = after.starts_with('=') && !after.starts_with("==");
                let lent = before
                    .trim_end_matches(|c: char| is_ident(c) || c == '.')
                    .trim_end()
                    .ends_with("&mut");
                if method.is_some_and(|method| !READS.contains(&method)) || assigned || lent {
                    found.push(format!("writes {word}"));
                }
            }
        }
        found.sort();
        found.dedup();
        found
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
    /// does not drive it: outside this file, the crate's code names none of
    /// the engine's row-changing methods, none of the `Command` or
    /// `WireCommand` variants that change a row, and writes neither of this
    /// window's row maps, except the entries `OUTSIDE` gives a reason for. So
    /// a dialog, gesture or worker added later cannot reach one without
    /// asking who drives. It reads source text: a call made through another
    /// crate or a macro is beyond it.
    #[test]
    fn the_engines_row_changes_are_named_only_here() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rust_files(&root, &mut files);
        assert!(files.len() > 20, "the crate's sources were found");
        let mut strays = Vec::new();
        let mut used = std::collections::HashSet::new();
        for file in files {
            // Modules compiled only for tests build their own fixtures.
            let name = file
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            if matches!(name, "row_changes.rs" | "test_support.rs") {
                continue;
            }
            let code = code_outside_tests(&std::fs::read_to_string(&file).expect("a source file"));
            for finding in row_changes_in(&code) {
                match OUTSIDE
                    .iter()
                    .position(|(file, allowed, _)| *file == name && *allowed == finding)
                {
                    Some(entry) => {
                        used.insert(entry);
                    }
                    None => strays.push(format!("{} {finding}", file.display())),
                }
            }
        }
        assert!(strays.is_empty(), "{strays:#?}");
        let stale: Vec<_> = (0..OUTSIDE.len())
            .filter(|entry| !used.contains(entry))
            .map(|entry| OUTSIDE[entry])
            .collect();
        assert!(stale.is_empty(), "allowed but no longer found: {stale:#?}");
    }

    /// The spellings that would hide a change from a search for its name are
    /// found too.
    #[test]
    fn the_guard_sees_qualified_aliased_and_lent_spellings() {
        let cases = [
            (
                "return dux_core::wire::WireCommand::DeleteSession {",
                "names WireCommand::DeleteSession",
            ),
            (
                "Engine::rename_agent(&mut self.engine,",
                "names rename_agent",
            ),
            (
                "use dux_core::engine::Command as C;",
                "renames or imports the variants of Command",
            ),
            (
                "use WireCommand::{StopAgentTab};",
                "renames or imports the variants of WireCommand",
            ),
            (
                "use WireCommand::*;",
                "renames or imports the variants of WireCommand",
            ),
            (
                "type W = dux_core::wire::WireCommand;",
                "renames or imports the variants of WireCommand",
            ),
            ("self.dock_layouts.entry(id)", "writes dock_layouts"),
            ("self.dock_active .remove(&id)", "writes dock_active"),
            ("let map = &mut self.dock_layouts;", "writes dock_layouts"),
            ("app.dock_layouts = HashMap::new();", "writes dock_layouts"),
        ];
        for (code, finding) in cases {
            assert!(
                row_changes_in(code).contains(&finding.to_string()),
                "{code}: {:?}",
                row_changes_in(code)
            );
        }
        let reads = [
            "self.dock_layouts.get(&id)",
            "std::process::Command::new(\"git\")",
            "let target: WireCommand = next;",
            "self.dock_layouts == other",
        ];
        for code in reads {
            assert!(
                row_changes_in(code).is_empty(),
                "{code}: {:?}",
                row_changes_in(code)
            );
        }
    }
}
