//! The `Command` enum — the §4.5 engine-operation vocabulary. Every
//! mutation or background-spawn the Engine performs in response to a
//! TUI key or a web-UI click is named here and dispatched through
//! `Engine::apply`.

use crate::engine::Engine;
use crate::engine::events::{
    BeginDeleteSessionView, DispatchAgentLaunchView, DoDeleteSessionView, EventReaction,
    FinishDeleteSessionView, StatusUpdate,
};
use crate::worker::{AgentLaunchRequest, CreateAgentRequest, ProjectPersistenceAction};

/// What the Engine should do. Variants are payload-carrying — the caller
/// computes the context (selected session id, prompt state, etc.) and
/// supplies it. The Engine performs the domain work and returns an
/// `EventReaction` describing any view follow-up.
pub enum Command {
    /// Complete a deletion that's already past its git step. Used by both
    /// the synchronous `do_delete_session` path (after `git::remove_worktree`)
    /// and the async `WorktreeRemoveCompleted` callback.
    FinishDeleteSession {
        session_id: String,
        delete_worktree: bool,
        remove_outcome: Option<bool>,
        update_status: bool,
    },
    /// Synchronous deletion: lookup → optional `git::remove_worktree` → full
    /// finish cascade. Used by `delete_selected_project`'s cascade.
    DoDeleteSession {
        session_id: String,
        delete_worktree: bool,
    },
    /// Modal entrypoint: branches between async git-removal worker and
    /// inline finish.
    BeginDeleteSession {
        session_id: String,
        delete_worktree: bool,
    },
    /// Persist a project mutation via the background worker. Fire-and-
    /// forget — the worker posts `WorkerEvent::ProjectPersistenceCompleted`
    /// back, which surfaces as `EventReaction::ProjectPersistenceOutcome`
    /// in the next `drain_events` pass.
    ///
    /// Boxed to keep the enum size within the clippy `large_enum_variant`
    /// threshold (`ProjectPersistenceAction` is 248 bytes unboxed).
    PersistProject(Box<ProjectPersistenceAction>),

    /// Spawn the create-agent worker. Returns `EventReaction::Status(Error)` if
    /// another create is already in flight; otherwise sets `create_agent_in_flight`,
    /// spawns the worker, and returns `EventReaction::Status(Busy(busy_message))`.
    /// `term_size` is supplied by the caller because `crossterm::terminal::size()`
    /// is binary-only.
    ///
    /// Boxed to keep the enum size within the clippy `large_enum_variant`
    /// threshold (`CreateAgentRequest` contains a full `Project` + fields).
    DispatchCreateAgentRequest {
        request: Box<CreateAgentRequest>,
        busy_message: String,
        term_size: (u16, u16),
    },

    /// Spawn the agent-launch worker (Reconnect / ForceReconnect / ResumeFallback
    /// / StartupAutoReopen / Create-finalize). Returns a typed view carrying
    /// `launched: bool` so App callers can do their per-site post-action.
    /// When already-in-flight, the view carries `launched: false` + a
    /// Status::info ("Agent X is already launching.").
    ///
    /// Boxed to keep the enum size within the clippy `large_enum_variant`
    /// threshold (`AgentLaunchRequest` carries `AgentSession` + env vector).
    DispatchAgentLaunch { request: Box<AgentLaunchRequest> },
}

impl Engine {
    /// Single dispatch point for every engine-affecting operation. The
    /// TUI's input layer calls this with a `Command` translated from key
    /// events; the web layer (sub-project #3) will call it with `Command`s
    /// deserialized from WebSocket messages. Returns an `EventReaction`
    /// the caller routes through its view-applier.
    pub fn apply(&mut self, command: Command) -> anyhow::Result<EventReaction> {
        match command {
            Command::FinishDeleteSession {
                session_id,
                delete_worktree,
                remove_outcome,
                update_status,
            } => {
                let Some(outcome) = self.finish_delete_session(&session_id)? else {
                    return Ok(EventReaction::Nothing);
                };
                Ok(EventReaction::FinishDeleteSessionView(Box::new(
                    FinishDeleteSessionView {
                        session_id,
                        outcome,
                        delete_worktree,
                        remove_outcome,
                        update_status,
                    },
                )))
            }
            Command::DoDeleteSession {
                session_id,
                delete_worktree,
            } => {
                let Some(outcome) = self.do_delete_session(&session_id, delete_worktree)? else {
                    return Ok(EventReaction::Nothing);
                };
                Ok(EventReaction::DoDeleteSessionView(Box::new(
                    DoDeleteSessionView {
                        session_id,
                        outcome,
                        delete_worktree,
                    },
                )))
            }
            Command::BeginDeleteSession {
                session_id,
                delete_worktree,
            } => {
                let outcome = self.begin_delete_session(&session_id, delete_worktree);
                Ok(EventReaction::BeginDeleteSessionView(Box::new(
                    BeginDeleteSessionView {
                        session_id,
                        outcome,
                        delete_worktree,
                    },
                )))
            }
            Command::PersistProject(action) => {
                self.spawn_project_persistence(*action);
                Ok(EventReaction::Nothing)
            }

            Command::DispatchCreateAgentRequest {
                request,
                busy_message,
                term_size,
            } => {
                if self.create_agent_in_flight {
                    return Ok(EventReaction::Status(StatusUpdate::error(
                        "An agent is already being created or forked.",
                    )));
                }
                self.create_agent_in_flight = true;
                let paths = self.paths.clone();
                let config = self.config.clone();
                let worker_tx = self.worker_tx.clone();
                std::thread::spawn(move || {
                    crate::agent_job::run_create_agent_job(
                        *request, paths, config, worker_tx, term_size,
                    );
                });
                Ok(EventReaction::Status(StatusUpdate::busy(busy_message)))
            }

            Command::DispatchAgentLaunch { request } => {
                let branch_name = request.session.branch_name.clone();
                let session_id = request.session.id.clone();
                if !self.agent_launches_in_flight.insert(session_id) {
                    return Ok(EventReaction::DispatchAgentLaunchView(Box::new(
                        DispatchAgentLaunchView {
                            launched: false,
                            status: Some(StatusUpdate::info(format!(
                                "Agent \"{}\" is already launching.",
                                branch_name,
                            ))),
                        },
                    )));
                }
                let tx = self.worker_tx.clone();
                std::thread::spawn(move || {
                    crate::agent_job::run_agent_launch_job(*request, tx);
                });
                Ok(EventReaction::DispatchAgentLaunchView(Box::new(
                    DispatchAgentLaunchView {
                        launched: true,
                        status: None,
                    },
                )))
            }
        }
    }
}
