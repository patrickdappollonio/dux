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
use crate::attachments::{Attachments, Blocker, Life, Policy, Scope, Surface, TargetKind};
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
        let who: Vec<String> = self.blockers.iter().map(describe).collect();
        write!(
            f,
            "{ATTACHED_LEAD}: {}. dux did not go ahead, so nothing was changed. Ask them to \
             close it first, or ask again and say to go ahead over everybody attached.",
            who.join("; ")
        )
    }
}

impl std::error::Error for Attached {}

/// One blocker in words: who, from where, doing what, to which tab or
/// terminal.
fn describe(blocker: &Blocker) -> String {
    let who = blocker
        .device
        .as_deref()
        .and_then(crate::device_label::short_device_label)
        .unwrap_or_else(|| match blocker.surface {
            Surface::Browser => "a browser".to_string(),
            Surface::TerminalUi => crate::background_serve::TUI_DEVICE_LABEL.to_string(),
        });
    let place = match &blocker.address {
        Some(address) if blocker.verified => format!(" at {address}"),
        Some(address) => format!(" at {address} (unverified)"),
        None => String::new(),
    };
    let doing = if blocker.driving {
        "typing in"
    } else {
        "watching"
    };
    let what = match blocker.target.kind {
        TargetKind::Tab => "tab",
        TargetKind::Terminal => "terminal",
    };
    format!("{who}{place}, {doing} {what} {}", blocker.target.id)
}

/// A reservation that lasts until the end of the call holding it, for a
/// change with no operation record to outlive it. One tied to a record ends
/// with the record instead, and dropping this does nothing to it.
#[must_use = "the reservation ends when this is dropped"]
pub struct Reservation {
    held: Option<(Attachments, u64)>,
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

    /// Refuse a change to `scope` with whoever is attached to it, other than
    /// the asking connection, unless the asker forced it; otherwise refuse
    /// every new attachment to it until the change ends: the operation record
    /// being dispatched finishes, or, with none, the returned reservation is
    /// dropped.
    pub fn reserve_destruction(&self, scope: Scope) -> Result<Reservation, Attached> {
        let policy = self.dispatch_policy.clone().unwrap_or_else(|| Policy {
            requester: Some(crate::attachments::TERMINAL_UI_CONNECTION.to_string()),
            force: false,
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
            _ => return Ok(None),
        };
        self.reserve_destruction(scope).map(Some)
    }

    /// The reservation for a guarded [`WireCommand`] the web dispatches to an
    /// engine method rather than as a [`Command`] (those are guarded in
    /// `Engine::apply`), or `None` for any other.
    pub(crate) fn guard_wire(
        &self,
        command: &WireCommand,
    ) -> Result<Option<Reservation>, Attached> {
        let scope = match command {
            WireCommand::DetachAgent { session_id, .. }
            | WireCommand::ReconnectSession {
                session_id,
                force: true,
            } => self.agent_scope(session_id),
            WireCommand::CloseAgentTab { tab_id, .. } => Self::pty_scope(tab_id),
            _ => return Ok(None),
        };
        self.reserve_destruction(scope).map(Some)
    }

    /// Everybody but the terminal UI attached to anything in this workspace:
    /// who quitting the terminal UI would cut off.
    pub fn attached_elsewhere(&self) -> Vec<Blocker> {
        let mut scope = Scope::default();
        for session in &self.sessions {
            self.add_agent(&mut scope, &session.id);
        }
        scope.ptys.extend(self.companion_terminals.keys().cloned());
        self.attachments.blockers(
            &scope,
            Some(crate::attachments::TERMINAL_UI_CONNECTION),
            std::time::Instant::now(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attachments::{ConnectionFacts, Heard, Target};
    use crate::engine::test_support::{sample_project, sample_session, test_engine};

    fn watch_from_a_browser(engine: &Engine, pty: &str, agent: &str) {
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
            .attach(
                "e1",
                Target {
                    kind: TargetKind::Tab,
                    id: pty.to_string(),
                    agent: Some(agent.to_string()),
                },
                None,
                None,
            )
            .unwrap();
    }

    /// A delete asked for by a client with no connection here (the command
    /// line) is refused while a browser watches the agent, with a sentence
    /// naming the browser and where it is; the agent stays.
    #[test]
    fn a_delete_from_a_client_with_no_connection_is_refused_while_a_browser_watches() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let session = sample_session("s1", "p1", "feat");
        engine.session_store.upsert_session(&session).unwrap();
        let slot = session.slot_tab_id().to_string();
        engine.sessions.push(session);
        watch_from_a_browser(&engine, &slot, "s1");
        engine.dispatch_policy = Some(Policy::default());

        let Err(refused) = engine.apply(Command::BeginDeleteSession {
            session_id: "s1".to_string(),
            delete_worktree: false,
            delete_branch: None,
        }) else {
            panic!("the delete went ahead");
        };
        let attached = refused
            .downcast_ref::<Attached>()
            .expect("an attachment refusal");
        assert_eq!(attached.blockers.len(), 1);
        let sentence = refused.to_string();
        assert!(sentence.contains("Chrome on macOS"), "{sentence}");
        assert!(sentence.contains("192.168.1.5"), "{sentence}");
        assert!(engine.sessions.iter().any(|s| s.id == "s1"));
    }

    /// The terminal UI asking (no dispatch policy) is exempt from its own
    /// attachments and from nobody else's.
    #[test]
    fn the_terminal_ui_is_exempt_only_from_its_own_attachments() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        for id in ["s1", "s2"] {
            let session = sample_session(id, "p1", id);
            engine.session_store.upsert_session(&session).unwrap();
            engine.sessions.push(session);
        }
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
        engine.attachments.set_terminal_ui(
            vec![Target {
                kind: TargetKind::Tab,
                id: "s1-slot".to_string(),
                agent: Some("s1".to_string()),
            }],
            None,
        );
        watch_from_a_browser(&engine, "s2-slot", "s2");

        assert!(engine.reserve_destruction(engine.agent_scope("s1")).is_ok());
        assert!(
            engine
                .reserve_destruction(engine.agent_scope("s2"))
                .is_err()
        );
    }
}
