//! The one place that decides whether whoever is asking may change a row of
//! the agents list that a device drives.
//!
//! A row is an agent with all its tabs and terminals, or one project or
//! standalone terminal ([`RowKey`]). The device driving a row may always change
//! it, and so may anybody while nobody drives it; anybody else is refused with
//! [`RowDriven`], naming the driver. A change is judged once, when a client
//! asks for it: [`Engine::apply_wire`] judges every [`WireCommand`] through
//! [`row_change`], and a surface that changes a row some other way asks
//! [`Engine::admit_row_change`] itself. What the engine does afterwards to
//! finish an admitted change is never judged again.
//!
//! Who is asking comes from [`Engine::dispatch_policy`], as it does for the
//! attachment guard; with none it is the terminal UI.

use super::Engine;
use crate::attachments::{RowDriver, TERMINAL_UI_CONNECTION};
use crate::model::TerminalOwner;
use crate::pty_owners::RowKey;
use crate::wire::WireCommand;

/// What a request would change, as far as driving goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowChange {
    /// The row of this agent, by session id.
    Agent(String),
    /// The row this tab or terminal belongs to.
    Pty(String),
    /// A new order for every terminal: the row of each agent whose own
    /// terminals would change places.
    TerminalOrder(Vec<String>),
    /// No driven row: a read, a new row, a project, the list's order, config.
    NoRow,
}

/// A change refused because another device drives the row it would change.
/// Nothing was changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowDriven {
    /// The row that is driven.
    pub row: RowKey,
    /// The driver's short label, `None` when it said nothing usable about itself.
    pub driver: Option<String>,
}

impl std::fmt::Display for RowDriven {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = match self.row {
            RowKey::Agent(_) => "agent",
            RowKey::Terminal(_) => "terminal",
        };
        let who = self.driver.as_deref().unwrap_or("another device");
        write!(
            f,
            "This {what} is being driven from {who} right now, and only the device driving \
             it can change it, so dux did not make the change. Take it over on this device \
             first, then try again."
        )
    }
}

impl std::error::Error for RowDriven {}

/// What `command` would change. Exhaustive, so a new command does not build
/// until it is given an answer here.
pub fn row_change(command: &WireCommand) -> RowChange {
    match command {
        // Git writes in the agent's working copy.
        WireCommand::StageFile { session_id, .. }
        | WireCommand::UnstageFile { session_id, .. }
        | WireCommand::DiscardFile { session_id, .. }
        | WireCommand::CommitChanges { session_id, .. }
        | WireCommand::Push { session_id }
        | WireCommand::Pull { session_id }
        // The agent's own settings and lifecycle.
        | WireCommand::ToggleAgentAutoReopen { session_id, .. }
        | WireCommand::RerunStartupCommand { session_id }
        | WireCommand::DeleteSession { session_id, .. }
        | WireCommand::KillSessionPty { session_id }
        | WireCommand::DetachAgent { session_id, .. }
        | WireCommand::RecreateWorkingCopy { session_id, .. }
        | WireCommand::RenameSession { session_id, .. }
        | WireCommand::ReconnectSession { session_id, .. }
        | WireCommand::ChangeAgentProvider { session_id, .. }
        // Its tabs.
        | WireCommand::ReorderAgentTabs { session_id, .. }
        | WireCommand::CloseAgentTab { session_id, .. }
        | WireCommand::StopAgentTab { session_id, .. }
        | WireCommand::ChangeAgentTabProvider { session_id, .. } => {
            RowChange::Agent(session_id.clone())
        }
        WireCommand::DeleteTerminal { terminal_id } => RowChange::Pty(terminal_id.clone()),
        WireCommand::ReorderTerminals { terminal_ids } => {
            RowChange::TerminalOrder(terminal_ids.clone())
        }

        // A fork reads the agent and makes a new row.
        WireCommand::ForkSession { .. }
        // New rows, which nobody drives yet.
        | WireCommand::CreateAgent { .. }
        | WireCommand::CreateStandaloneAgent { .. }
        | WireCommand::CreateAgentFromWorktree { .. }
        | WireCommand::CreateAgentFromPr { .. }
        // Projects: their settings, their own git, adding them. Removing one
        // ends rows, which the attachment guard judges against everybody attached.
        | WireCommand::PullProject { .. }
        | WireCommand::UpdateProjectProvider { .. }
        | WireCommand::UpdateProjectAutoReopen { .. }
        | WireCommand::UpdateProjectStartupCommand { .. }
        | WireCommand::UpdateProjectEnv { .. }
        | WireCommand::CheckoutProjectDefaultBranch { .. }
        | WireCommand::ChangeProjectBaseBranch { .. }
        | WireCommand::AddProject { .. }
        | WireCommand::AddProjectCheckoutDefault { .. }
        | WireCommand::AddProjectCreateInitialCommit { .. }
        | WireCommand::AddProjectInitRepo { .. }
        | WireCommand::CloneProject { .. }
        | WireCommand::RemoveProject { .. }
        | WireCommand::DeleteProject { .. }
        // Pull-request bookkeeping.
        | WireCommand::AttachPullRequest { .. }
        | WireCommand::DetachPullRequest { .. }
        | WireCommand::HidePullRequest { .. }
        | WireCommand::ShowPullRequest { .. }
        | WireCommand::SetPullRequestSort { .. }
        | WireCommand::MovePullRequest { .. }
        | WireCommand::ReorderPullRequests { .. }
        // Where rows sit in the list.
        | WireCommand::ReorderAgents { .. }
        | WireCommand::ReorderProjects { .. }
        | WireCommand::PlaceTerminalInList { .. }
        // What a device looks at: the tab it has on top, the changes it watches.
        | WireCommand::SetLastFocusedTab { .. }
        | WireCommand::WatchChangedFiles { .. }
        // Config and preferences.
        | WireCommand::PersistGlobalEnv { .. }
        | WireCommand::SetGlobalEnvVar { .. }
        | WireCommand::RemoveGlobalEnvVar { .. }
        | WireCommand::UpdateMacros { .. }
        | WireCommand::SetMacro { .. }
        | WireCommand::RemoveMacro { .. }
        | WireCommand::ReloadConfig {}
        | WireCommand::RecoverConfig {}
        | WireCommand::SetChangesPaneVisible { .. }
        | WireCommand::SetInstanceIdentity { .. }
        | WireCommand::ToggleRandomizedPetNameDefault {}
        | WireCommand::TogglePrBannerPosition {}
        | WireCommand::SetAgentSort { .. }
        | WireCommand::ToggleGithubIntegration {}
        | WireCommand::RecheckGithub {}
        | WireCommand::ToggleCopyOnSelect {}
        | WireCommand::ToggleAlwaysShowTabStrip {}
        | WireCommand::ToggleTabReachesAgent {}
        | WireCommand::SetTailscaleMode { .. }
        | WireCommand::SetSettings(_) => RowChange::NoRow,
    }
}

impl Engine {
    /// Whether whoever is asking may make `change`: yes when no row it touches
    /// is driven, when the asker drives it, or when the asker said to go ahead
    /// over everybody.
    ///
    /// # Errors
    ///
    /// [`RowDriven`] naming the device that drives a row the change touches.
    pub fn admit_row_change(&self, change: &RowChange) -> Result<(), RowDriven> {
        let (asker, forced) = match &self.dispatch_policy {
            // Going ahead over named blockers accepts those attachments only.
            Some(policy) => (
                policy.asker.as_deref(),
                policy.force && policy.accepted.is_none(),
            ),
            None => (Some(TERMINAL_UI_CONNECTION), false),
        };
        if forced {
            return Ok(());
        }
        for row in self.rows_changed(change) {
            if let RowDriver::Another { device } = self.attachments.row_driver(&row, asker) {
                return Err(RowDriven {
                    row,
                    driver: device
                        .as_deref()
                        .and_then(crate::device_label::short_device_label),
                });
            }
        }
        Ok(())
    }

    /// [`Self::admit_row_change`] for a wire command.
    pub(crate) fn admit_wire_row_change(&self, command: &WireCommand) -> Result<(), RowDriven> {
        self.admit_row_change(&row_change(command))
    }

    fn rows_changed(&self, change: &RowChange) -> Vec<RowKey> {
        match change {
            RowChange::Agent(session_id) => vec![RowKey::Agent(session_id.clone())],
            RowChange::Pty(pty_id) => vec![self.pty_row(pty_id)],
            RowChange::TerminalOrder(order) => self.agents_reordered_by(order),
            RowChange::NoRow => Vec::new(),
        }
    }

    /// The agents whose own terminals `order` would put in a different order
    /// than they are in now.
    fn agents_reordered_by(&self, order: &[String]) -> Vec<RowKey> {
        let mut now: Vec<(&String, &crate::model::CompanionTerminal)> =
            self.companion_terminals.iter().collect();
        now.sort_by(|a, b| a.1.sort_order.cmp(&b.1.sort_order).then(a.0.cmp(b.0)));
        let agent_of = |id: &String| match self.companion_terminals.get(id).map(|t| &t.owner) {
            Some(TerminalOwner::Session(session_id)) => Some(session_id.clone()),
            Some(TerminalOwner::Project(_) | TerminalOwner::Standalone) | None => None,
        };
        let mut agents: Vec<String> = now.iter().filter_map(|(id, _)| agent_of(id)).collect();
        agents.sort();
        agents.dedup();
        agents
            .into_iter()
            .filter(|agent| {
                let of_agent = |id: &&String| agent_of(id).as_ref() == Some(agent);
                let before: Vec<&String> = now.iter().map(|(id, _)| *id).filter(of_agent).collect();
                let after: Vec<&String> = order.iter().filter(of_agent).collect();
                before != after
            })
            .map(RowKey::Agent)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::attachments::{ConnectionFacts, Policy, Surface, Target, TargetKind};
    use crate::engine::test_support::{sample_project, sample_session, test_engine};
    use crate::pty_owners::{PtySizeOwners, ResizeIntent};

    const FIREFOX: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0";

    /// An engine with agent `s1`, one terminal of its own and a standalone
    /// terminal, serving with an ownership record nobody holds yet.
    fn serving_engine() -> (
        Engine,
        crate::test_scratch::ScratchDir,
        tempfile::TempDir,
        Arc<PtySizeOwners>,
        String,
        String,
    ) {
        let (mut engine, tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree");
        engine.projects.push(sample_project("p1", "/repo"));
        let mut session = sample_session("s1", "p1", "feat/login");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = worktree.path().to_string_lossy().to_string();
        engine
            .session_store
            .upsert_session(&session)
            .expect("save the agent");
        engine.sessions.push(session);
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];
        let (companion, _) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("the agent's terminal");
        let (standalone, _) = engine
            .create_standalone_terminal(24, 80)
            .expect("a standalone terminal");
        let owners = Arc::new(PtySizeOwners::default());
        engine.attachments.use_owners(Arc::clone(&owners));
        owners.join_row("s1-slot", RowKey::Agent("s1".to_string()));
        owners.join_row(&companion, RowKey::Agent("s1".to_string()));
        (engine, tmp, worktree, owners, companion, standalone)
    }

    /// A browser tab `events` with the agent's first tab open on seat `seat`.
    fn open_the_agent(engine: &Engine, events: &str, seat: u64) {
        engine.attachments.register(
            events,
            ConnectionFacts {
                surface: Surface::Browser,
                device: Some(FIREFOX.to_string()),
                address: Some("192.168.1.5".parse().unwrap()),
                verified: false,
                events: true,
                browser: None,
            },
            None,
        );
        engine
            .attachments
            .attach(
                events,
                Target {
                    kind: TargetKind::Tab,
                    id: "s1-slot".to_string(),
                    agent: Some("s1".to_string()),
                },
                None,
                Some(seat),
            )
            .expect("attach");
    }

    fn asked_by(
        engine: &mut Engine,
        asker: Option<&str>,
        change: &RowChange,
    ) -> Result<(), String> {
        engine.dispatch_policy = Some(Policy {
            asker: asker.map(str::to_string),
            ..Policy::default()
        });
        let answer = engine.admit_row_change(change).map_err(|e| e.to_string());
        engine.dispatch_policy = None;
        answer
    }

    /// The whole rule in one walk: anybody while nobody drives, then only the
    /// driver, through a take-over and the driver's page closing.
    #[test]
    fn a_driven_row_is_changed_only_by_its_driver() {
        let (mut engine, _tmp, _worktree, owners, companion, standalone) = serving_engine();
        let agent = RowChange::Agent("s1".to_string());
        let (first, second) = (owners.next_conn_id(), owners.next_conn_id());
        open_the_agent(&engine, "e1", first);
        open_the_agent(&engine, "e2", second);

        for asker in [Some("e1"), Some("e2"), None] {
            asked_by(&mut engine, asker, &agent).expect("nobody drives the row yet");
        }

        owners.may_write("s1-slot", first, Some(FIREFOX), || {});
        asked_by(&mut engine, Some("e1"), &agent).expect("the driver");
        for asker in [Some("e2"), None, Some("no-such-connection")] {
            let refused = asked_by(&mut engine, asker, &agent).expect_err("not the driver");
            assert!(refused.contains("Firefox on Linux"), "{refused}");
            assert!(refused.contains("did not make the change"), "{refused}");
        }
        // The agent's terminal is the agent's row; a standalone terminal is its own.
        asked_by(&mut engine, Some("e2"), &RowChange::Pty(companion)).expect_err("the agent's row");
        asked_by(&mut engine, Some("e2"), &RowChange::Pty(standalone)).expect("a row of its own");
        asked_by(&mut engine, Some("e2"), &RowChange::NoRow).expect("no row");
        // With no policy the asker is the terminal UI, which does not drive it.
        engine
            .admit_row_change(&agent)
            .expect_err("the terminal UI is not the driver");

        let taken = owners.claim_for_resize(
            "s1-slot",
            second,
            ResizeIntent::TakeOver {
                expected_owner: None,
            },
            Some(FIREFOX),
            |_| {},
        );
        assert!(taken.apply);
        asked_by(&mut engine, Some("e2"), &agent).expect("the driver right after taking over");
        asked_by(&mut engine, Some("e1"), &agent).expect_err("the device that lost the row");

        owners.release("s1-slot", second);
        asked_by(&mut engine, Some("e1"), &agent).expect("nobody drives once the driver has gone");
    }

    /// Going ahead over everybody gets past a driver; going ahead over named
    /// attachments does not.
    #[test]
    fn only_a_blanket_go_ahead_overrides_the_driver() {
        let (mut engine, _tmp, _worktree, owners, _, _) = serving_engine();
        let agent = RowChange::Agent("s1".to_string());
        let seat = owners.next_conn_id();
        open_the_agent(&engine, "e1", seat);
        owners.may_write("s1-slot", seat, Some(FIREFOX), || {});

        engine.dispatch_policy = Some(Policy {
            force: true,
            ..Policy::default()
        });
        engine
            .admit_row_change(&agent)
            .expect("forced over everybody");
        engine.dispatch_policy = Some(Policy {
            force: true,
            accepted: Some(std::collections::BTreeSet::new()),
            ..Policy::default()
        });
        engine
            .admit_row_change(&agent)
            .expect_err("named blockers are not the driver's consent");
    }

    /// A refused wire command changes nothing, and its error is the refusal a
    /// surface can recognize.
    #[test]
    fn a_refused_wire_command_changes_nothing() {
        let (mut engine, _tmp, _worktree, owners, _, _) = serving_engine();
        let seat = owners.next_conn_id();
        open_the_agent(&engine, "e1", seat);
        owners.may_write("s1-slot", seat, Some(FIREFOX), || {});
        let before = engine.sessions[0].auto_reopen_enabled;
        let toggle = || WireCommand::ToggleAgentAutoReopen {
            session_id: "s1".to_string(),
            enabled: !before,
        };

        engine.dispatch_policy = Some(Policy::default());
        let refused = engine.apply_wire(toggle()).expect_err("refused");
        assert!(refused.downcast_ref::<RowDriven>().is_some(), "{refused}");
        assert_eq!(engine.sessions[0].auto_reopen_enabled, before);

        engine.dispatch_policy = Some(Policy {
            asker: Some("e1".to_string()),
            ..Policy::default()
        });
        engine
            .apply_wire(toggle())
            .expect("the driver's own change");
        assert_eq!(engine.sessions[0].auto_reopen_enabled, !before);
    }

    /// A new terminal order touches an agent's row only when that agent's own
    /// terminals change places.
    #[test]
    fn a_terminal_reorder_is_judged_by_the_agents_whose_terminals_move() {
        let (mut engine, _tmp, _worktree, _owners, first, standalone) = serving_engine();
        let (second, _) = engine
            .create_companion_terminal("s1", 24, 80)
            .expect("a second terminal");

        let kept = vec![standalone.clone(), first.clone(), second.clone()];
        assert_eq!(engine.agents_reordered_by(&kept), Vec::new());
        let swapped = vec![second, first, standalone];
        assert_eq!(
            engine.agents_reordered_by(&swapped),
            vec![RowKey::Agent("s1".to_string())]
        );
    }
}
