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
//! The terminal UI's gestures keep no record, so they hold nothing, but they
//! are checked against what the followed changes hold all the same: the
//! commands it sends through `Engine::apply` in [`Engine::check_command`], and
//! the gestures that call an engine method directly (stop, start, a tab's
//! create and close, a terminal's create) through [`Engine::check_admission`]
//! at the gesture. Its own in-flight and closing guards cover two of its own
//! gestures racing each other.

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
    /// Admit `command` against the open records, its keys held by the record
    /// being dispatched; a command outside admission is always admitted.
    pub(crate) fn admit_wire(&self, command: &WireCommand) -> Result<(), InTheWay> {
        match self.wire_admission(command) {
            Some(admission) => self.admit(self.operation_in_dispatch.as_deref(), &admission),
            None => Ok(()),
        }
    }

    /// Check `admission` without holding anything: refused when an open
    /// record other than the one being dispatched holds what it wants.
    pub fn check_admission(&self, admission: &Admission) -> Result<(), InTheWay> {
        self.operations
            .admit(self.operation_in_dispatch.as_deref(), &admission.wants, &[])
    }

    /// [`Self::check_admission`] for the engine commands a surface sends itself.
    pub(crate) fn check_command(&self, command: &super::Command) -> Result<(), InTheWay> {
        use super::Command;
        use super::command::ConfigSetChange;
        let admission = match command {
            Command::BeginDeleteSession { session_id, .. } => self.agent_admission(session_id),
            Command::RemoveProject { project_id, .. }
            | Command::DeleteProject { project_id, .. } => self.project_admission(project_id),
            Command::DeleteTerminal { terminal_id } => self.terminal_admission(terminal_id),
            Command::UpdateMacros { .. }
            | Command::ChangeConfigSet(
                ConfigSetChange::ReplaceMacros { .. }
                | ConfigSetChange::SetMacro { .. }
                | ConfigSetChange::RemoveMacro { .. },
            ) => Self::whole(InFlightKey::MacroList),
            Command::PersistGlobalEnv { .. }
            | Command::ChangeConfigSet(
                ConfigSetChange::ReplaceEnv { .. }
                | ConfigSetChange::SetEnvVar { .. }
                | ConfigSetChange::RemoveEnvVar { .. },
            ) => Self::whole(InFlightKey::GlobalEnv),
            _ => return Ok(()),
        };
        self.check_admission(&admission)
    }

    /// A change to a whole agent (delete, stop, start).
    pub fn agent_admission(&self, session_id: &str) -> Admission {
        Admission {
            wants: self.agent_wants(session_id),
            takes: vec![Hold::exclusive(InFlightKey::Agent(session_id.to_string()))],
        }
    }

    /// A project removal or deletion.
    pub fn project_admission(&self, project_id: &str) -> Admission {
        Admission {
            wants: self.project_wants(project_id),
            takes: vec![Hold::exclusive(InFlightKey::Project(
                project_id.to_string(),
            ))],
        }
    }

    /// A terminal's close.
    pub fn terminal_admission(&self, terminal_id: &str) -> Admission {
        Admission {
            wants: self.terminal_wants(terminal_id),
            takes: vec![Hold::exclusive(InFlightKey::Terminal(
                terminal_id.to_string(),
            ))],
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
            | WireCommand::ReconnectSession { session_id, .. } => {
                Some(self.agent_admission(session_id))
            }
            WireCommand::RemoveProject { project_id }
            | WireCommand::DeleteProject { project_id } => Some(self.project_admission(project_id)),
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
            WireCommand::CloseAgentTab { session_id, tab_id }
            | WireCommand::StopAgentTab { session_id, tab_id } => {
                Some(self.tab_admission(session_id, TabId::new(tab_id.clone())))
            }
            WireCommand::DeleteTerminal { terminal_id } => {
                Some(self.terminal_admission(terminal_id))
            }
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

#[cfg(test)]
mod tests {
    use crate::engine::Command;
    use crate::engine::test_support::{sample_project, sample_session, test_engine};
    use crate::operations::{Hold, OperationKind};

    use super::*;

    /// A terminal UI gesture keeps no record, so it holds nothing, but it is
    /// still refused, with the sentence naming the operation, while a change
    /// another surface follows holds what the gesture would change. One that
    /// would end a terminal somebody else is attached to is refused too,
    /// naming who and from where; the terminal UI's own attachments are never
    /// in its way.
    #[test]
    fn a_terminal_ui_command_is_refused_while_a_followed_operation_holds_what_it_changes() {
        use crate::attachments::{ConnectionFacts, Heard, Surface, Target, TargetKind};

        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        for id in ["s1", "s2", "s3"] {
            let session = sample_session(id, "p1", id);
            engine.session_store.upsert_session(&session).unwrap();
            engine.sessions.push(session);
        }
        engine.open_operation("op-held", OperationKind::AgentStop);
        engine
            .operations
            .admit(
                Some("op-held"),
                &[],
                &[
                    Hold::exclusive(InFlightKey::Agent("s1".to_string())),
                    Hold::exclusive(InFlightKey::MacroList),
                    Hold::exclusive(InFlightKey::GlobalEnv),
                ],
            )
            .unwrap();
        // A browser watches s2; the terminal UI draws s3.
        let tab = |agent: &str| Target {
            kind: TargetKind::Tab,
            id: format!("{agent}-slot"),
            agent: Some(agent.to_string()),
        };
        engine.attachments.register(
            "e1",
            ConnectionFacts {
                surface: Surface::Browser,
                device: Some(
                    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                     (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36"
                        .to_string(),
                ),
                address: Some("192.168.1.5".parse().unwrap()),
                verified: true,
                events: true,
            },
            Some(Heard::now()),
        );
        engine
            .attachments
            .attach("e1", tab("s2"), None, None)
            .unwrap();
        engine.attachments.register(
            crate::attachments::TERMINAL_UI_CONNECTION,
            ConnectionFacts {
                surface: Surface::TerminalUi,
                device: Some(crate::background_serve::TUI_DEVICE_LABEL.to_string()),
                address: None,
                verified: true,
                events: false,
            },
            None,
        );
        engine.attachments.set_terminal_ui(vec![tab("s3")], None);

        let held: &[&str] = &["op-held", "stopping an agent"];
        let delete = |id: &str| Command::BeginDeleteSession {
            session_id: id.to_string(),
            delete_worktree: false,
            delete_branch: None,
        };
        // (what, command, the sentence it is refused with, or `None` to go ahead)
        let commands: Vec<(&str, Command, Option<&[&str]>)> = vec![
            ("an agent delete", delete("s1"), Some(held)),
            (
                "a project removal",
                Command::RemoveProject {
                    project_id: "p1".to_string(),
                    project_name: "p1".to_string(),
                },
                Some(held),
            ),
            (
                "a project deletion",
                Command::DeleteProject {
                    project_id: "p1".to_string(),
                    project_name: "p1".to_string(),
                },
                Some(held),
            ),
            (
                "a macro save",
                Command::UpdateMacros {
                    macros: Default::default(),
                },
                Some(held),
            ),
            (
                "an environment save",
                Command::PersistGlobalEnv {
                    env: Default::default(),
                },
                Some(held),
            ),
            (
                "a delete of an agent a browser watches",
                delete("s2"),
                Some(&["Chrome on macOS", "192.168.1.5"]),
            ),
            (
                "a delete of an agent only the terminal UI draws",
                delete("s3"),
                None,
            ),
        ];
        for (name, command, refused_with) in commands {
            match (engine.apply(command), refused_with) {
                (Err(refused), Some(needles)) => {
                    let sentence = refused.to_string();
                    for needle in needles {
                        assert!(sentence.contains(needle), "{name}: {sentence}");
                    }
                }
                (Ok(_), None) => {}
                (Err(refused), None) => panic!("{name} was refused: {refused}"),
                (Ok(_), Some(_)) => panic!("{name} went ahead"),
            }
        }
        // The web's own kill of an agent's processes is refused the same way.
        let Err(refused) = engine.apply_wire(WireCommand::KillSessionPty {
            session_id: "s2".to_string(),
        }) else {
            panic!("a kill of an agent a browser watches went ahead");
        };
        assert!(refused.to_string().contains("192.168.1.5"), "{refused}");
        // So is a stop of one tab: of an agent a change holds, naming that
        // change, and of a tab a browser watches, naming who.
        let stop_tab = |agent: &str| WireCommand::StopAgentTab {
            session_id: agent.to_string(),
            tab_id: format!("{agent}-slot"),
        };
        for (agent, needle) in [("s1", "op-held"), ("s2", "192.168.1.5")] {
            let Err(refused) = engine.apply_wire(stop_tab(agent)) else {
                panic!("a stop of {agent}'s tab went ahead");
            };
            assert!(refused.to_string().contains(needle), "{refused}");
        }
        assert!(engine.sessions.iter().any(|s| s.id == "s1"));
        assert!(engine.sessions.iter().any(|s| s.id == "s2"));
        assert!(engine.projects.iter().any(|p| p.id == "p1"));
    }
}
