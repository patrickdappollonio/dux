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
//! A request is judged for every row it would change, not only the one it
//! names. [`RowChange`] says what a request is, and [`Engine::rows_changed`]
//! is the one place that turns that into rows: removing a project is every
//! agent and terminal in it, a start is also every agent it would stop. A
//! write to the disk is judged by where it lands as well as by the row it was
//! sent through: [`Engine::admit_write`] answers the row and hands back the
//! folder of every agent another device drives ([`DrivenFolders`]), which the
//! writer checks its destinations against off the engine thread.
//!
//! Who is asking comes from [`Engine::dispatch_policy`], as it does for the
//! attachment guard; with none it is the terminal UI.

use std::path::{Path, PathBuf};

use super::Engine;
use crate::attachments::{Asker, RowDriver};
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
    /// Starting this agent: its row, and the row of every agent running in
    /// its folder, which a start stops.
    StartAgent(String),
    /// Starting this tab: as [`Self::StartAgent`] for the tab's agent.
    StartPty(String),
    /// Opening a terminal socket on this tab, which starts a tab that is not
    /// running: [`Self::StartPty`] then, and nothing while it runs or is
    /// already starting.
    OpenPty(String),
    /// Removing this project: the row of every agent in it and of each of its
    /// own terminals.
    Project(String),
    /// Git in this project's own checkout: the row of every agent whose
    /// folder is that checkout.
    ProjectCheckout(String),
    /// No driven row: a read, a new row, the list's order, config.
    NoRow,
}

/// How a refused request reaches the driven row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reach {
    /// The request names the row.
    Named,
    /// The request names something else and would change this row too; the
    /// row's label.
    Also(String),
    /// The request would write inside this agent's folder; the agent's label.
    Inside(String),
}

/// A change refused because another device drives a row it would change.
/// Nothing was changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RowDriven {
    /// The row that is driven.
    pub row: RowKey,
    /// The driver's short label, `None` when it said nothing usable about itself.
    pub driver: Option<String>,
    /// How the request reaches that row.
    pub reach: Reach,
}

impl std::fmt::Display for RowDriven {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let what = match self.row {
            RowKey::Agent(_) => "agent",
            RowKey::Terminal(_) => "terminal",
        };
        let who = self.driver.as_deref().unwrap_or("another device");
        match &self.reach {
            Reach::Named => write!(
                f,
                "This {what} is being driven from {who} right now, and only the device \
                 driving it can change it, so dux did not make the change. Take it over on \
                 this device first, then try again."
            ),
            Reach::Also(label) => write!(
                f,
                "This would also change the {what} \"{label}\", which is being driven from \
                 {who} right now, and only the device driving it can change it, so dux did \
                 not make the change. Take that {what} over on this device first, then try \
                 again."
            ),
            Reach::Inside(label) => write!(
                f,
                "This would write inside the folder of the agent \"{label}\", which is being \
                 driven from {who} right now, and only the device driving it can change its \
                 files through dux, so dux did not make the change. Take that agent over on \
                 this device first, then try again."
            ),
        }
    }
}

impl std::error::Error for RowDriven {}

/// The folder of every agent another device drives, as one asker sees them:
/// what a write to the disk is checked against ([`Engine::admit_write`]).
#[derive(Clone, Debug, Default)]
pub struct DrivenFolders(Vec<(PathBuf, RowDriven)>);

impl DrivenFolders {
    /// Whether no agent is driven by another device, so no write can land in
    /// one's folder and nothing needs checking.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The refusal for a write to `path` when it is one of these folders or
    /// anywhere inside one, under every spelling of both
    /// ([`crate::worktree_ops::folder_contains`]), so a symbolic link on the way
    /// changes nothing. Reads the disk: call it off the engine thread and off
    /// the async runtime.
    pub fn refusal_inside(&self, path: &Path) -> Option<&RowDriven> {
        self.0
            .iter()
            .find(|(folder, _)| crate::worktree_ops::folder_contains(folder, path))
            .map(|(_, refusal)| refusal)
    }

    /// [`Self::refusal_inside`], and also when `path` is a folder one of these
    /// is inside: for a delete or a move, which takes everything under it.
    pub fn refusal_touching(&self, path: &Path) -> Option<&RowDriven> {
        self.refusal_inside(path).or_else(|| {
            self.0
                .iter()
                .find(|(folder, _)| crate::worktree_ops::folder_contains(path, folder))
                .map(|(_, refusal)| refusal)
        })
    }
}

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
        | WireCommand::RecreateWorkingCopy {
            session_id,
            restart_tabs: false,
        }
        | WireCommand::RenameSession { session_id, .. }
        | WireCommand::ChangeAgentProvider { session_id, .. }
        // Its tabs.
        | WireCommand::ReorderAgentTabs { session_id, .. }
        | WireCommand::CloseAgentTab { session_id, .. }
        | WireCommand::StopAgentTab { session_id, .. }
        | WireCommand::ChangeAgentTabProvider { session_id, .. } => {
            RowChange::Agent(session_id.clone())
        }
        // Each of these starts the agent, or tabs of it.
        WireCommand::ReconnectSession { session_id, .. }
        | WireCommand::RecreateWorkingCopy {
            session_id,
            restart_tabs: true,
        } => RowChange::StartAgent(session_id.clone()),
        WireCommand::DeleteTerminal { terminal_id } => RowChange::Pty(terminal_id.clone()),
        WireCommand::ReorderTerminals { terminal_ids } => {
            RowChange::TerminalOrder(terminal_ids.clone())
        }
        // Ends every agent and terminal of the project.
        WireCommand::RemoveProject { project_id } | WireCommand::DeleteProject { project_id } => {
            RowChange::Project(project_id.clone())
        }
        // Git in the project's own checkout, which an agent may have as its folder.
        WireCommand::PullProject { project_id }
        | WireCommand::CheckoutProjectDefaultBranch { project_id }
        | WireCommand::ChangeProjectBaseBranch { project_id, .. } => {
            RowChange::ProjectCheckout(project_id.clone())
        }

        // A fork reads the agent and makes a new row.
        WireCommand::ForkSession { .. }
        // New rows, which nobody drives yet. A new agent never shares a folder
        // with another: creation refuses a folder an agent already has.
        | WireCommand::CreateAgent { .. }
        | WireCommand::CreateStandaloneAgent { .. }
        | WireCommand::CreateAgentFromWorktree { .. }
        | WireCommand::CreateAgentFromPr { .. }
        // Projects: their settings, and adding them.
        | WireCommand::UpdateProjectProvider { .. }
        | WireCommand::UpdateProjectAutoReopen { .. }
        | WireCommand::UpdateProjectStartupCommand { .. }
        | WireCommand::UpdateProjectEnv { .. }
        | WireCommand::AddProject { .. }
        | WireCommand::AddProjectCheckoutDefault { .. }
        | WireCommand::AddProjectCreateInitialCommit { .. }
        | WireCommand::AddProjectInitRepo { .. }
        | WireCommand::CloneProject { .. }
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
    /// Who the question is asked as, and whether they said to go ahead over
    /// everybody. Going ahead over named blockers accepts those attachments
    /// only, never a driver.
    fn row_asker(&self) -> (Asker, bool) {
        match &self.dispatch_policy {
            Some(policy) => (
                policy.asker.clone(),
                policy.force && policy.accepted.is_none(),
            ),
            None => (Asker::TerminalUi, false),
        }
    }

    /// The refusal when another device drives `row`, as `asker` sees it.
    fn driven_by_another(&self, row: &RowKey, asker: &Asker, reach: Reach) -> Option<RowDriven> {
        match self.attachments.row_driver(row, asker) {
            RowDriver::Another { device } => Some(RowDriven {
                row: row.clone(),
                driver: device
                    .as_deref()
                    .and_then(crate::device_label::short_device_label),
                reach,
            }),
            RowDriver::Nobody | RowDriver::Asker => None,
        }
    }

    /// Whether whoever is asking may make `change`: yes when no row it touches
    /// is driven, when the asker drives each one that is, or when the asker
    /// said to go ahead over everybody.
    ///
    /// # Errors
    ///
    /// [`RowDriven`] naming the device that drives a row the change touches.
    pub fn admit_row_change(&self, change: &RowChange) -> Result<(), RowDriven> {
        let (asker, forced) = self.row_asker();
        if forced {
            return Ok(());
        }
        for (row, reach) in self.rows_changed(change) {
            if let Some(driven) = self.driven_by_another(&row, &asker, reach) {
                return Err(driven);
            }
        }
        Ok(())
    }

    /// [`Self::admit_row_change`] for a write to the disk sent through
    /// `through`, answering what the writer still has to check: the folder of
    /// every agent another device drives, which no destination may be inside
    /// ([`DrivenFolders::refusal_inside`]). Empty when the asker said to go
    /// ahead over everybody.
    ///
    /// # Errors
    ///
    /// [`RowDriven`] when another device drives a row `through` touches.
    pub fn admit_write(&self, through: &RowChange) -> Result<DrivenFolders, RowDriven> {
        self.admit_row_change(through)?;
        let (asker, forced) = self.row_asker();
        if forced {
            return Ok(DrivenFolders::default());
        }
        Ok(DrivenFolders(
            self.sessions
                .iter()
                .filter_map(|session| {
                    let row = RowKey::Agent(session.id.clone());
                    let reach = Reach::Inside(session.display_label());
                    let driven = self.driven_by_another(&row, &asker, reach)?;
                    Some((PathBuf::from(session.directory()), driven))
                })
                .collect(),
        ))
    }

    /// [`Self::admit_row_change`] for a wire command.
    pub(crate) fn admit_wire_row_change(&self, command: &WireCommand) -> Result<(), RowDriven> {
        self.admit_row_change(&row_change(command))
    }

    /// [`Self::admit_row_change`] for a command a surface sends itself: a
    /// project removal, which the terminal UI sends with no row gating it.
    /// Any other command is gated at its gesture or finishes an admitted change.
    pub(crate) fn admit_command_row_change(
        &self,
        command: &super::Command,
    ) -> Result<(), RowDriven> {
        match command {
            super::Command::RemoveProject { project_id, .. }
            | super::Command::DeleteProject { project_id, .. } => {
                self.admit_row_change(&RowChange::Project(project_id.clone()))
            }
            _ => Ok(()),
        }
    }

    /// Every row `change` would change, each with how the request reaches it.
    /// The one place a request's effect on rows it does not name is worked out.
    fn rows_changed(&self, change: &RowChange) -> Vec<(RowKey, Reach)> {
        let named = |row: RowKey| vec![(row, Reach::Named)];
        match change {
            RowChange::Agent(session_id) => named(RowKey::Agent(session_id.clone())),
            RowChange::Pty(pty_id) => named(self.pty_row(pty_id)),
            RowChange::TerminalOrder(order) => self
                .agents_reordered_by(order)
                .into_iter()
                .map(|row| (row, Reach::Named))
                .collect(),
            RowChange::StartAgent(session_id) => {
                let mut rows = named(RowKey::Agent(session_id.clone()));
                rows.extend(self.agents_a_start_would_stop(session_id));
                rows
            }
            RowChange::StartPty(pty_id) => {
                let row = self.pty_row(pty_id);
                let stopped = match &row {
                    RowKey::Agent(session_id) => self.agents_a_start_would_stop(session_id),
                    RowKey::Terminal(_) => Vec::new(),
                };
                let mut rows = named(row);
                rows.extend(stopped);
                rows
            }
            RowChange::OpenPty(pty_id) => {
                let tab = crate::ids::TabIdRef::new(pty_id);
                let starts = !self.providers.contains_key(tab)
                    && !self.is_in_flight(&super::InFlightKey::AgentLaunch(tab.to_owned()));
                if starts {
                    self.rows_changed(&RowChange::StartPty(pty_id.clone()))
                } else {
                    Vec::new()
                }
            }
            RowChange::Project(project_id) => {
                let scope = self.project_scope(project_id);
                let agents = scope.agents.iter().map(|session_id| {
                    (
                        RowKey::Agent(session_id.clone()),
                        Reach::Also(self.agent_label(session_id)),
                    )
                });
                let terminals = self
                    .companion_terminals
                    .iter()
                    .filter(|(_, terminal)| {
                        terminal.owner == TerminalOwner::Project(project_id.clone())
                    })
                    .map(|(id, terminal)| {
                        (
                            RowKey::Terminal(id.clone()),
                            Reach::Also(terminal.label.clone()),
                        )
                    });
                agents.chain(terminals).collect()
            }
            RowChange::ProjectCheckout(project_id) => {
                let Some(checkout) = self
                    .projects
                    .iter()
                    .find(|project| &project.id == project_id)
                    .map(|project| project.path.clone())
                else {
                    return Vec::new();
                };
                self.sessions
                    .iter()
                    .filter(|session| {
                        crate::project_browser::same_directory(session.directory(), &checkout)
                    })
                    .map(|session| {
                        (
                            RowKey::Agent(session.id.clone()),
                            Reach::Also(session.display_label()),
                        )
                    })
                    .collect()
            }
            RowChange::NoRow => Vec::new(),
        }
    }

    /// The agents a start of `session_id` would stop, the ones
    /// [`Engine::launch_conflict_scope`] names.
    fn agents_a_start_would_stop(&self, session_id: &str) -> Vec<(RowKey, Reach)> {
        self.launch_conflict_scope(session_id)
            .agents
            .iter()
            .map(|other| {
                (
                    RowKey::Agent(other.clone()),
                    Reach::Also(self.agent_label(other)),
                )
            })
            .collect()
    }

    fn agent_label(&self, session_id: &str) -> String {
        self.session_by_id(session_id).map_or_else(
            || session_id.to_string(),
            crate::model::AgentSession::display_label,
        )
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
    use crate::attachments::Policy;
    use crate::engine::test_support::{sample_project, sample_session, test_engine};
    use crate::pty_owners::{PageId, PtySizeOwners, ResizeIntent};

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

    fn page(raw: &str) -> PageId {
        PageId::parse(&raw.repeat(32)).expect("a page id")
    }

    /// `change` asked as a browser request naming `pages`.
    fn asked_by(engine: &mut Engine, pages: &[PageId], change: &RowChange) -> Result<(), String> {
        engine.dispatch_policy = Some(Policy {
            asker: if pages.is_empty() {
                Asker::Nobody
            } else {
                Asker::Pages(pages.to_vec())
            },
            ..Policy::default()
        });
        let answer = engine.admit_row_change(change).map_err(|e| e.to_string());
        engine.dispatch_policy = None;
        answer
    }

    /// The whole rule in one walk: anybody while nobody drives, then only the
    /// page whose seat holds the row, through a take-over and the driver's
    /// sockets closing.
    #[test]
    fn a_driven_row_is_changed_only_by_its_driver() {
        let (mut engine, _tmp, _worktree, owners, companion, standalone) = serving_engine();
        let agent = RowChange::Agent("s1".to_string());
        let first = owners.take_seat(Some(&page("a")), "s1-slot");
        let second = owners.take_seat(Some(&page("b")), "s1-slot");

        for pages in [vec![page("a")], vec![page("b")], Vec::new()] {
            asked_by(&mut engine, &pages, &agent).expect("nobody drives the row yet");
        }

        owners.may_write("s1-slot", first.id(), Some(FIREFOX), || {});
        asked_by(&mut engine, &[page("a")], &agent).expect("the driving page");
        asked_by(&mut engine, &[page("c"), page("a")], &agent)
            .expect("an editor tab the driving page opened");
        for pages in [vec![page("b")], vec![page("c")], Vec::new()] {
            let refused = asked_by(&mut engine, &pages, &agent).expect_err("not the driver");
            assert!(refused.contains("Firefox on Linux"), "{refused}");
            assert!(refused.contains("did not make the change"), "{refused}");
        }
        // The agent's terminal is the agent's row; a standalone terminal is its own.
        asked_by(&mut engine, &[page("b")], &RowChange::Pty(companion))
            .expect_err("the agent's row");
        asked_by(&mut engine, &[page("b")], &RowChange::Pty(standalone)).expect("a row of its own");
        asked_by(&mut engine, &[page("b")], &RowChange::NoRow).expect("no row");
        // With no policy the asker is the terminal UI, which does not drive it.
        engine
            .admit_row_change(&agent)
            .expect_err("the terminal UI is not the driver");

        let taken = owners.claim_for_resize(
            "s1-slot",
            second.id(),
            ResizeIntent::TakeOver {
                expected_owner: None,
            },
            Some(FIREFOX),
            |_| {},
        );
        assert!(taken.apply);
        asked_by(&mut engine, &[page("b")], &agent).expect("the driver right after taking over");
        asked_by(&mut engine, &[page("a")], &agent).expect_err("the page that lost the row");

        owners.leave_seat(&second);
        asked_by(&mut engine, &[page("a")], &agent)
            .expect("nobody drives once the driver's sockets have gone");
    }

    /// A request is judged for every row it would change: removing a project
    /// for each agent and terminal in it, the project's own git for an agent
    /// whose folder is that checkout, and a start for the agent it would stop.
    #[test]
    fn a_request_is_judged_for_rows_it_does_not_name() {
        let (mut engine, _tmp, worktree, owners, _, _) = serving_engine();
        let seat = owners.take_seat(Some(&page("a")), "s1-slot");
        owners.may_write("s1-slot", seat.id(), Some(FIREFOX), || {});
        let folder = worktree.path().to_string_lossy().to_string();

        // A second agent in the same folder, not running, and the first running.
        let mut second = sample_session("s2", "p1", "feat/signup");
        second
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = folder.clone();
        engine.sessions.push(second);
        let args = vec!["-c".to_string(), "sleep 30".to_string()];
        let running = crate::pty::PtyClient::spawn("/bin/sh", &args, worktree.path(), 24, 80, 100)
            .expect("spawn the running agent");
        engine
            .providers
            .insert(crate::ids::TabId::new("s1-slot"), running);
        engine.projects[0].path = folder;

        for (what, change) in [
            ("removing the project", RowChange::Project("p1".to_string())),
            (
                "git in the checkout the agent has as its folder",
                RowChange::ProjectCheckout("p1".to_string()),
            ),
            (
                "starting an agent in the same folder",
                RowChange::StartAgent("s2".to_string()),
            ),
            (
                "starting that agent's tab",
                RowChange::StartPty("s2-slot".to_string()),
            ),
            (
                "opening a socket on that agent's stopped tab",
                RowChange::OpenPty("s2-slot".to_string()),
            ),
        ] {
            let refused = asked_by(&mut engine, &[], &change).expect_err(what);
            assert!(refused.contains("Firefox on Linux"), "{what}: {refused}");
            assert!(
                refused.contains("s1-title"),
                "{what} names the agent: {refused}"
            );
            asked_by(&mut engine, &[page("a")], &change).expect(what);
        }
        // A socket on a tab that is running starts nothing, so anybody opens it.
        asked_by(&mut engine, &[], &RowChange::OpenPty("s1-slot".to_string()))
            .expect("watching a running tab");
        // A project with no driven agent in it, and one the agent is not in.
        asked_by(&mut engine, &[], &RowChange::Project("p2".to_string())).expect("another project");
        engine.projects[0].path = "/repo".to_string();
        asked_by(
            &mut engine,
            &[],
            &RowChange::ProjectCheckout("p1".to_string()),
        )
        .expect("a checkout no agent has as its folder");
    }

    /// The terminal UI removes a project by a command of its own, which is
    /// judged like anybody's: refused while a browser drives an agent in it,
    /// and let through while the terminal UI's own seat does.
    #[test]
    fn the_terminal_uis_project_removal_is_judged_for_the_agents_it_ends() {
        let (mut engine, _tmp, _worktree, owners, _, _) = serving_engine();
        let seat = owners.take_seat(Some(&page("a")), "s1-slot");
        owners.may_write("s1-slot", seat.id(), Some(FIREFOX), || {});
        let remove = || crate::engine::Command::RemoveProject {
            project_id: "p1".to_string(),
            project_name: "p1".to_string(),
        };

        let refused = engine
            .apply(remove())
            .expect_err("a browser drives its agent");
        assert!(refused.downcast_ref::<RowDriven>().is_some(), "{refused}");
        assert!(engine.sessions.iter().any(|session| session.id == "s1"));

        let terminal_ui = owners.seat_terminal_ui();
        let taken = owners.claim_for_resize(
            "s1-slot",
            terminal_ui,
            ResizeIntent::TakeOver {
                expected_owner: None,
            },
            Some(crate::background_serve::TUI_DEVICE_LABEL),
            |_| {},
        );
        assert!(taken.apply);
        engine
            .apply(remove())
            .expect("the terminal UI drives it now");
        assert!(engine.sessions.iter().all(|session| session.id != "s1"));
    }

    /// A write is checked against the folder of every agent another device
    /// drives, through a symbolic link too, and against none for the driver.
    #[test]
    fn a_write_is_judged_by_the_driven_folder_it_lands_in() {
        let (mut engine, _tmp, worktree, owners, _, standalone) = serving_engine();
        let seat = owners.take_seat(Some(&page("a")), "s1-slot");
        owners.may_write("s1-slot", seat.id(), Some(FIREFOX), || {});
        let through = RowChange::Pty(standalone);
        let mut folders = |pages: &[PageId], force: bool| {
            engine.dispatch_policy = Some(Policy {
                asker: Asker::Pages(pages.to_vec()),
                force,
                ..Policy::default()
            });
            let folders = engine
                .admit_write(&through)
                .expect("nobody drives the terminal");
            engine.dispatch_policy = None;
            folders
        };

        let watcher = folders(&[page("b")], false);
        let inside = worktree.path().join("src/new.rs");
        let refused = watcher.refusal_inside(&inside).expect("inside the folder");
        assert!(refused.to_string().contains("s1-title"), "{refused}");
        assert!(
            refused.to_string().contains("Firefox on Linux"),
            "{refused}"
        );
        let outside = tempfile::tempdir().expect("another folder");
        assert!(watcher.refusal_inside(outside.path()).is_none());
        let link = outside.path().join("link");
        std::os::unix::fs::symlink(worktree.path(), &link).expect("a link to the folder");
        assert!(watcher.refusal_inside(&link.join("new.rs")).is_some());
        // Deleting or moving a folder takes an agent's folder inside it along.
        let parent = worktree.path().parent().expect("a parent");
        assert!(watcher.refusal_inside(parent).is_none());
        assert!(watcher.refusal_touching(parent).is_some());

        assert!(folders(&[page("a")], false).is_empty(), "the driver");
        assert!(folders(&[page("b")], true).is_empty(), "a blanket go-ahead");
    }

    /// The terminal UI drives a row while its seat holds it, whatever panes it
    /// drew last: it is let through with nothing attached, and refused once a
    /// browser has taken the row.
    #[test]
    fn the_terminal_ui_is_judged_by_its_seat() {
        let (engine, _tmp, _worktree, owners, _, _) = serving_engine();
        let agent = RowChange::Agent("s1".to_string());
        let seat = crate::background_serve::TuiOwnership::seat(Arc::clone(&owners));
        owners.may_write(
            "s1-slot",
            seat.conn_id,
            Some(crate::background_serve::TUI_DEVICE_LABEL),
            || {},
        );
        engine
            .admit_row_change(&agent)
            .expect("the terminal UI drives the row and has drawn none of it");

        let browser = owners.next_conn_id();
        let taken = owners.claim_for_resize(
            "s1-slot",
            browser,
            ResizeIntent::TakeOver {
                expected_owner: None,
            },
            Some(FIREFOX),
            |_| {},
        );
        assert!(taken.apply);
        let refused = engine
            .admit_row_change(&agent)
            .expect_err("a browser took the row");
        assert!(
            refused.to_string().contains("Firefox on Linux"),
            "{refused}"
        );
    }

    /// Going ahead over everybody gets past a driver; going ahead over named
    /// attachments does not.
    #[test]
    fn only_a_blanket_go_ahead_overrides_the_driver() {
        let (mut engine, _tmp, _worktree, owners, _, _) = serving_engine();
        let agent = RowChange::Agent("s1".to_string());
        let seat = owners.next_conn_id();
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
        let seat = owners.take_seat(Some(&page("a")), "s1-slot");
        owners.may_write("s1-slot", seat.id(), Some(FIREFOX), || {});
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
            asker: Asker::Pages(vec![page("a")]),
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
