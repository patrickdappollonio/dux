//! One change per thing at a time.
//!
//! A change a client can ask for (a route the command line calls) is admitted
//! before it starts: the engine works out which admission keys
//! ([`InFlightKey::Agent`] and its siblings) the change wants, from its own
//! state, and the operation registry refuses it when an open record holds one
//! of them in a conflicting mode, naming that record's id. A change that is
//! followed as an operation then holds what it changes until its record
//! finishes, so the next one is refused for as long as the work really runs,
//! however long that is. A change nobody follows is checked the same way and
//! holds nothing past its own call.
//!
//! What a change wants:
//!
//! - An agent-level change (delete, stop, start): the agent, each of its tabs
//!   and each of its own terminals, and its project shared. It holds the agent.
//! - A project removal: the project, everything under it as above, and its own
//!   terminals. It holds the project.
//! - An agent create: its project shared (and, for a fork, the source agent
//!   shared), which is also what it holds, so two creates in one project never
//!   refuse each other while a project removal is refused by either. Once the
//!   create has made its agent it holds that agent too
//!   ([`Engine::record_created_session`]).
//! - A tab or terminal change: that tab or terminal, and its agent or project
//!   shared. It holds the tab or terminal.
//! - A macro or environment change: the macro list or the environment.
//!
//! The terminal UI's own gestures do not pass through here; the in-flight
//! guards each of them already has still apply to it.

use super::{Engine, InFlightKey};
use crate::ids::TabId;
use crate::model::TerminalOwner;
use crate::operations::{Hold, InTheWay};
use crate::wire::WireCommand;

/// What a change wants admitted, and what it holds once admitted.
pub struct Admission {
    pub wants: Vec<Hold>,
    pub takes: Vec<Hold>,
}

impl Engine {
    /// Admit `command` against the open operation records, taking its keys
    /// onto the record of the change being dispatched when it has one. A
    /// command that changes nothing a client follows is always admitted.
    pub(crate) fn admit_wire(&self, command: &WireCommand) -> Result<(), InTheWay> {
        match self.wire_admission(command) {
            Some(admission) => self.admit(self.operation_in_dispatch.as_deref(), &admission),
            None => Ok(()),
        }
    }

    /// Admit `admission`, holding its keys on `record` when there is one.
    pub fn admit(&self, record: Option<&str>, admission: &Admission) -> Result<(), InTheWay> {
        self.operations
            .admit(record, &admission.wants, &admission.takes)
    }

    /// What `command` wants and holds, or `None` for a command outside
    /// admission.
    pub(crate) fn wire_admission(&self, command: &WireCommand) -> Option<Admission> {
        match command {
            WireCommand::DeleteSession { session_id, .. }
            | WireCommand::DetachAgent { session_id, .. }
            | WireCommand::ReconnectSession { session_id, .. } => Some(Admission {
                wants: self.agent_wants(session_id),
                takes: vec![Hold::exclusive(InFlightKey::Agent(session_id.clone()))],
            }),
            WireCommand::RemoveProject { project_id }
            | WireCommand::DeleteProject { project_id } => Some(Admission {
                wants: self.project_wants(project_id),
                takes: vec![Hold::exclusive(InFlightKey::Project(project_id.clone()))],
            }),
            WireCommand::CreateAgent { project_id, .. }
            | WireCommand::CreateAgentFromWorktree { project_id, .. }
            | WireCommand::CreateAgentFromPr { project_id, .. } => {
                let project = vec![Hold::shared(InFlightKey::Project(project_id.clone()))];
                Some(Admission {
                    wants: project.clone(),
                    takes: project,
                })
            }
            WireCommand::ForkSession { session_id, .. } => {
                let mut holds = vec![Hold::shared(InFlightKey::Agent(session_id.clone()))];
                holds.extend(
                    self.project_of(session_id)
                        .map(|project| Hold::shared(InFlightKey::Project(project.to_string()))),
                );
                Some(Admission {
                    wants: holds.clone(),
                    takes: holds,
                })
            }
            WireCommand::CloseAgentTab { session_id, tab_id } => {
                Some(self.tab_admission(session_id, TabId::new(tab_id.clone())))
            }
            WireCommand::DeleteTerminal { terminal_id } => Some(Admission {
                wants: self.terminal_wants(terminal_id),
                takes: vec![Hold::exclusive(InFlightKey::Terminal(terminal_id.clone()))],
            }),
            WireCommand::UpdateMacros { .. }
            | WireCommand::SetMacro { .. }
            | WireCommand::RemoveMacro { .. } => Some(Self::whole(InFlightKey::MacroList)),
            WireCommand::PersistGlobalEnv { .. }
            | WireCommand::SetGlobalEnvVar { .. }
            | WireCommand::RemoveGlobalEnvVar { .. } => Some(Self::whole(InFlightKey::GlobalEnv)),
            _ => None,
        }
    }

    /// A change to one tab of `session_id`: the tab, and its agent shared.
    pub fn tab_admission(&self, session_id: &str, tab_id: TabId) -> Admission {
        Admission {
            wants: vec![
                Hold::exclusive(InFlightKey::Tab(tab_id.clone())),
                Hold::shared(InFlightKey::Agent(session_id.to_string())),
            ],
            takes: vec![Hold::exclusive(InFlightKey::Tab(tab_id))],
        }
    }

    /// A change that opens something new inside `owner`: the owner shared,
    /// and nothing held, because what it makes does not exist yet.
    pub fn inside_admission(&self, owner: &TerminalOwner) -> Admission {
        Admission {
            wants: self.owner_wants(owner),
            takes: Vec::new(),
        }
    }

    fn whole(key: InFlightKey) -> Admission {
        let hold = vec![Hold::exclusive(key)];
        Admission {
            wants: hold.clone(),
            takes: hold,
        }
    }

    fn project_of(&self, session_id: &str) -> Option<&str> {
        self.sessions
            .iter()
            .find(|session| session.id == session_id)
            .and_then(|session| session.project_id())
    }

    fn agent_wants(&self, session_id: &str) -> Vec<Hold> {
        let mut wants = self.agent_subtree(session_id);
        wants.extend(
            self.project_of(session_id)
                .map(|project| Hold::shared(InFlightKey::Project(project.to_string()))),
        );
        wants
    }

    /// The agent, each of its tabs and each of its own terminals, exclusive.
    fn agent_subtree(&self, session_id: &str) -> Vec<Hold> {
        let mut wants = vec![Hold::exclusive(InFlightKey::Agent(session_id.to_string()))];
        wants.extend(
            self.tab_ids_for_session(session_id)
                .into_iter()
                .map(|tab| Hold::exclusive(InFlightKey::Tab(tab))),
        );
        wants.extend(self.terminals_owned_by(&TerminalOwner::Session(session_id.to_string())));
        wants
    }

    fn project_wants(&self, project_id: &str) -> Vec<Hold> {
        let mut wants = vec![Hold::exclusive(InFlightKey::Project(
            project_id.to_string(),
        ))];
        for session in &self.sessions {
            if session.project_id() == Some(project_id) {
                wants.extend(self.agent_subtree(&session.id));
            }
        }
        wants.extend(self.terminals_owned_by(&TerminalOwner::Project(project_id.to_string())));
        wants
    }

    fn terminal_wants(&self, terminal_id: &str) -> Vec<Hold> {
        let mut wants = vec![Hold::exclusive(InFlightKey::Terminal(
            terminal_id.to_string(),
        ))];
        if let Some(terminal) = self.companion_terminals.get(terminal_id) {
            wants.extend(self.owner_wants(&terminal.owner));
        }
        wants
    }

    fn owner_wants(&self, owner: &TerminalOwner) -> Vec<Hold> {
        match owner {
            TerminalOwner::Session(session_id) => {
                let mut wants = vec![Hold::shared(InFlightKey::Agent(session_id.clone()))];
                wants.extend(
                    self.project_of(session_id)
                        .map(|project| Hold::shared(InFlightKey::Project(project.to_string()))),
                );
                wants
            }
            TerminalOwner::Project(project_id) => {
                vec![Hold::shared(InFlightKey::Project(project_id.clone()))]
            }
            TerminalOwner::Standalone => Vec::new(),
        }
    }

    fn terminals_owned_by(&self, owner: &TerminalOwner) -> Vec<Hold> {
        self.companion_terminals
            .iter()
            .filter(|(_, terminal)| &terminal.owner == owner)
            .map(|(id, _)| Hold::exclusive(InFlightKey::Terminal(id.clone())))
            .collect()
    }
}
