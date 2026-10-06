//! The engine's half of the attachment guard: what each destructive change
//! would cut off, and the one reservation every surface's change goes
//! through before it acts.
//!
//! Guarded changes: deleting an agent, stopping it, restarting it by force,
//! closing a tab, closing a terminal, and removing or deleting a project. The
//! ones a surface sends as a [`Command`] are reserved in `Engine::apply`
//! ([`Engine::guard_command`]); the ones the web dispatches straight to an
//! engine method are reserved in `Engine::apply_wire`
//! ([`Engine::guard_wire`]); the terminal UI's gestures that call an engine
//! method directly reserve at the gesture ([`Engine::reserve_destruction`]).
//!
//! Who is asking comes from [`Engine::dispatch_policy`]: the web sets it
//! around every request it dispatches, and `None` is the terminal UI, whose
//! own attachments are then the exempt ones.

use super::{Command, Engine};
use crate::attachments::{Attachments, Blocker, Life, Policy, Scope, TargetKind};
use crate::model::TerminalOwner;
use crate::wire::WireCommand;

/// A destructive change refused because somebody else is attached to what it
/// would end. Nothing was changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attached {
    pub blockers: Vec<Blocker>,
}

/// How every [`Attached`] sentence starts.
const ATTACHED_LEAD: &str = "Someone else is using this right now";

impl std::fmt::Display for Attached {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let who: Vec<String> = self.blockers.iter().map(Blocker::describe).collect();
        write!(
            f,
            "{ATTACHED_LEAD}: {}. dux did not go ahead, so nothing was changed. Ask them to \
             close it first, or ask again and say to go ahead over everybody attached.",
            who.join("; ")
        )
    }
}

impl std::error::Error for Attached {}

/// A reservation that ends when dropped, for a change with no operation record;
/// one tied to a record ends with the record, and dropping this leaves it.
#[must_use = "the reservation ends when this is dropped"]
pub struct Reservation {
    held: Option<(Attachments, u64)>,
}

impl Reservation {
    /// The change's work runs on under the status key `key`: keep the reservation
    /// until that key's final lands, or the spinner's ceiling passes.
    pub fn until_final(mut self, key: &str) {
        if let Some((attachments, id)) = self.held.take() {
            attachments.hand_to_key(
                id,
                key,
                std::time::Instant::now() + crate::statusline::BUSY_LIVE_CEILING,
            );
        }
    }
}

impl Drop for Reservation {
    fn drop(&mut self) {
        if let Some((attachments, id)) = self.held.take() {
            attachments.release(id);
        }
    }
}

impl Engine {
    /// What deleting, stopping or restarting agent `session_id` would end:
    /// its tabs, its own terminals, and anything attached to it.
    pub fn agent_scope(&self, session_id: &str) -> Scope {
        let mut scope = Scope::default();
        self.add_agent(&mut scope, session_id);
        scope
    }

    /// What removing project `project_id` would end: every agent in it, as
    /// [`Self::agent_scope`], and its own terminals.
    pub fn project_scope(&self, project_id: &str) -> Scope {
        let mut scope = Scope::default();
        let agents: Vec<String> = self
            .sessions
            .iter()
            .filter(|session| session.project_id() == Some(project_id))
            .map(|session| session.id.clone())
            .collect();
        for agent in agents {
            self.add_agent(&mut scope, &agent);
        }
        scope
            .ptys
            .extend(self.terminal_ids_owned_by(&TerminalOwner::Project(project_id.to_string())));
        scope
    }

    /// What starting `session_id` would end: every other agent running in its
    /// folder, which a launch stops so the two never share it.
    pub fn launch_conflict_scope(&self, session_id: &str) -> Scope {
        let mut scope = Scope::default();
        let Some(directory) = self
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .map(|session| session.directory().to_string())
        else {
            return scope;
        };
        let conflicting: Vec<String> = self
            .sessions
            .iter()
            .filter(|other| {
                other.id != session_id
                    && crate::project_browser::same_directory(other.directory(), &directory)
                    && self
                        .tab_ids_for_session(&other.id)
                        .iter()
                        .any(|tab| self.providers.contains_key(tab))
            })
            .map(|other| other.id.clone())
            .collect();
        for other in conflicting {
            self.add_agent(&mut scope, &other);
        }
        scope
    }

    /// What closing one tab or one terminal would end.
    pub fn pty_scope(pty_id: &str) -> Scope {
        Scope {
            ptys: [pty_id.to_string()].into(),
            agents: Default::default(),
        }
    }

    fn add_agent(&self, scope: &mut Scope, session_id: &str) {
        scope.agents.insert(session_id.to_string());
        scope.ptys.extend(
            self.tab_ids_for_session(session_id)
                .into_iter()
                .map(|tab| tab.as_str().to_string()),
        );
        scope
            .ptys
            .extend(self.terminal_ids_owned_by(&TerminalOwner::Session(session_id.to_string())));
    }

    fn terminal_ids_owned_by(&self, owner: &TerminalOwner) -> Vec<String> {
        self.companion_terminals
            .iter()
            .filter(|(_, terminal)| &terminal.owner == owner)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// Reserve `scope` against new attachments until the change ends: the record
    /// being dispatched finishes, or, with none, the reservation is dropped.
    ///
    /// # Errors
    ///
    /// [`Attached`] naming whoever else is attached, unless the asker forced it.
    pub fn reserve_destruction(&self, scope: Scope) -> Result<Reservation, Attached> {
        let policy = self.dispatch_policy.clone().unwrap_or_else(|| Policy {
            requester: Some(crate::attachments::TERMINAL_UI_CONNECTION.to_string()),
            force: false,
            accepted: None,
        });
        let watch = self
            .operation_in_dispatch
            .as_deref()
            .and_then(|id| self.operations.watch(id));
        let life = match &watch {
            Some(watch) => Life::Record(watch.clone()),
            None => Life::Released,
        };
        let id = self
            .attachments
            .reserve(scope, &policy, life, std::time::Instant::now())
            .map_err(|blockers| Attached { blockers })?;
        Ok(Reservation {
            held: watch.is_none().then(|| (self.attachments.clone(), id)),
        })
    }

    /// The reservation for a guarded [`Command`], or `None` for any other.
    pub(crate) fn guard_command(&self, command: &Command) -> Result<Option<Reservation>, Attached> {
        let scope = match command {
            Command::BeginDeleteSession { session_id, .. } => self.agent_scope(session_id),
            Command::RemoveProject { project_id, .. }
            | Command::DeleteProject { project_id, .. } => self.project_scope(project_id),
            Command::DeleteTerminal { terminal_id } => Self::pty_scope(terminal_id),
            Command::DispatchAgentLaunch { request } => {
                let scope = self.launch_conflict_scope(&request.session.id);
                if scope.is_empty() {
                    return Ok(None);
                }
                scope
            }
            Command::PersistProject { action, .. } => match action.as_ref() {
                crate::engine::ProjectPersistenceAction::Remove { project_id, .. } => {
                    self.project_scope(project_id)
                }
                _ => return Ok(None),
            },
            _ => return Ok(None),
        };
        self.reserve_destruction(scope).map(Some)
    }

    /// The reservation for a guarded [`WireCommand`] the web dispatches to an
    /// engine method rather than as a [`Command`], or `None` for any other.
    pub(crate) fn guard_wire(
        &self,
        command: &WireCommand,
    ) -> Result<Option<Reservation>, Attached> {
        let scope = match command {
            WireCommand::DetachAgent { session_id, .. }
            | WireCommand::KillSessionPty { session_id } => self.agent_scope(session_id),
            // A start stops any other agent in the same folder, and a forced one
            // ends this agent's own run first; both before anything launches.
            WireCommand::ReconnectSession { session_id, force } => {
                let mut scope = self.launch_conflict_scope(session_id);
                if *force {
                    scope.extend(self.agent_scope(session_id));
                }
                if scope.is_empty() {
                    return Ok(None);
                }
                scope
            }
            WireCommand::CloseAgentTab { tab_id, .. }
            | WireCommand::StopAgentTab { tab_id, .. } => Self::pty_scope(tab_id),
            _ => return Ok(None),
        };
        self.reserve_destruction(scope).map(Some)
    }

    /// Everybody but the terminal UI attached to anything in this workspace:
    /// who quitting the terminal UI would cut off.
    pub fn attached_elsewhere(&self) -> Vec<Blocker> {
        self.attachments.blockers(
            &self.workspace_scope(),
            Some(crate::attachments::TERMINAL_UI_CONNECTION),
            std::time::Instant::now(),
        )
    }

    /// The terminal UI is quitting over `accepted`: nobody can attach to anything
    /// until the process has gone.
    ///
    /// # Errors
    ///
    /// [`Attached`] with everybody attached now, when somebody else has attached since.
    pub fn reserve_quit(&self, accepted: &[Blocker]) -> Result<(), Attached> {
        self.attachments
            .reserve_accepting(
                self.workspace_scope(),
                Some(crate::attachments::TERMINAL_UI_CONNECTION),
                accepted,
                Life::Released,
                std::time::Instant::now(),
            )
            .map(|_| ())
            .map_err(|blockers| Attached { blockers })
    }

    /// `blockers` with every name a dialog shows resolved; a terminal this engine
    /// no longer has is named by its id.
    pub fn attached_entries(
        &self,
        blockers: &[Blocker],
    ) -> Vec<crate::attached_prose::AttachedEntry> {
        blockers
            .iter()
            .map(|blocker| {
                let target = &blocker.target;
                let agent = target
                    .agent
                    .as_deref()
                    .and_then(|id| self.sessions.iter().find(|session| session.id == id));
                let label = match target.kind {
                    TargetKind::Tab => agent.and_then(|session| {
                        self.tab_prose_label(
                            crate::ids::SessionIdRef::new(&session.id),
                            crate::ids::TabIdRef::new(&target.id),
                        )
                    }),
                    TargetKind::Terminal => self
                        .companion_terminals
                        .get(&target.id)
                        .map(|terminal| terminal.label.clone()),
                };
                crate::attached_prose::AttachedEntry {
                    device: blocker
                        .device
                        .as_deref()
                        .and_then(crate::device_label::short_device_label),
                    surface: blocker.surface,
                    address: blocker.address.clone(),
                    verified: blocker.verified,
                    driving: blocker.driving,
                    target_kind: target.kind,
                    target_label: label.unwrap_or_else(|| target.id.clone()),
                    agent_label: agent.map(|session| session.display_label()),
                }
            })
            .collect()
    }

    fn workspace_scope(&self) -> Scope {
        let mut scope = Scope::default();
        for session in &self.sessions {
            self.add_agent(&mut scope, &session.id);
        }
        scope.ptys.extend(self.companion_terminals.keys().cloned());
        scope
    }
}

#[cfg(test)]
mod tests {
    use crate::attachments::{ConnectionFacts, Surface, Target, TargetKind};
    use crate::engine::test_support::{sample_project, sample_session, test_engine};
    use crate::ids::TabId;
    use crate::wire::WireCommand;

    /// Starting an agent stops any other agent running in the same folder, so
    /// the start is refused, naming who, while somebody else watches that
    /// other agent, forced or not; the other agent keeps running.
    #[test]
    fn a_start_that_would_stop_a_watched_agent_in_the_same_folder_is_refused() {
        let (mut engine, tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        let shared = tmp.path().join("shared-folder");
        std::fs::create_dir_all(&shared).unwrap();
        for id in ["s1", "s2"] {
            let mut session = sample_session(id, "p1", id);
            session
                .workspace
                .as_managed_mut()
                .expect("managed test session")
                .worktree_path = shared.to_string_lossy().to_string();
            engine.session_store.upsert_session(&session).unwrap();
            engine.sessions.push(session);
        }
        let args = vec!["-c".to_string(), "sleep 30".to_string()];
        let running = crate::pty::PtyClient::spawn("/bin/sh", &args, &shared, 24, 80, 100)
            .expect("spawn the running agent");
        engine.providers.insert(TabId::new("s2-slot"), running);
        engine.attachments.register(
            "e1",
            ConnectionFacts {
                surface: Surface::Browser,
                device: Some("Firefox".to_string()),
                address: Some("192.168.1.5".parse().unwrap()),
                verified: false,
                events: true,
            },
            None,
        );
        engine
            .attachments
            .attach(
                "e1",
                Target {
                    kind: TargetKind::Tab,
                    id: "s2-slot".to_string(),
                    agent: Some("s2".to_string()),
                },
                None,
                None,
            )
            .unwrap();

        for force in [false, true] {
            let refused = match engine.apply_wire(WireCommand::ReconnectSession {
                session_id: "s1".to_string(),
                force,
            }) {
                Ok(_) => panic!("the start went ahead (force: {force})"),
                Err(refused) => refused.to_string(),
            };
            assert!(refused.contains("192.168.1.5"), "{refused}");
        }
        assert!(engine.providers.contains_key(&TabId::new("s2-slot")));
    }

    /// A blocker as a dialog names it: the device by its short label, the tab
    /// by the label its strip shows and the agent by its name; a terminal this
    /// engine no longer has is named by its id rather than dropped.
    #[test]
    fn a_blocker_is_named_by_its_device_tab_and_agent_labels() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        engine
            .sessions
            .push(sample_session("s1", "p1", "feat/login"));
        let blocker = |kind, id: &str, agent: Option<&str>| crate::attachments::Blocker {
            surface: Surface::Browser,
            device: Some(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                 (KHTML, like Gecko) Chrome/124.0 Safari/537.36"
                    .to_string(),
            ),
            address: Some("100.64.0.2".to_string()),
            verified: true,
            driving: false,
            target: Target {
                kind,
                id: id.to_string(),
                agent: agent.map(str::to_string),
            },
        };

        let entries = engine.attached_entries(&[
            blocker(TargetKind::Tab, "s1-slot", Some("s1")),
            blocker(TargetKind::Terminal, "gone-terminal", None),
        ]);

        assert_eq!(entries[0].device.as_deref(), Some("Chrome on macOS"));
        assert_eq!(entries[0].target_label, "Claude");
        assert_eq!(entries[0].agent_label.as_deref(), Some("s1-title"));
        assert_eq!(entries[1].target_label, "gone-terminal");
        assert_eq!(entries[1].agent_label, None);
    }
}
