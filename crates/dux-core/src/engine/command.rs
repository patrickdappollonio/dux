//! The `Command` enum — the §4.5 engine-operation vocabulary. Every
//! mutation or background-spawn the Engine performs in response to a
//! TUI key or a web-UI click is named here and dispatched through
//! `Engine::apply`.

use std::path::PathBuf;

use crate::engine::events::{
    BeginDeleteSessionView, DeleteTerminalView, DispatchAgentLaunchView, DoDeleteSessionView,
    EventReaction, FinishDeleteSessionView, StatusUpdate,
};
use crate::engine::{CommandWorkerSpec, Engine, InFlightKey};
use crate::worker::{
    AgentLaunchFailedData, AgentLaunchRequest, CreateAgentRequest, ProjectPersistenceAction,
    PullTarget, WorkerEvent,
};

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
    /// another create is already in flight; otherwise marks `InFlightKey::CreateAgent`,
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

    /// Stage a single file. Synchronous git call (microseconds for the
    /// typical case). Returns `EventReaction::Nothing` on success; an `Err`
    /// propagates to the App caller which surfaces it.
    StageFile {
        worktree_path: PathBuf,
        path: String,
    },

    /// Unstage a single file. Same shape as `StageFile` — synchronous git
    /// call, `EventReaction::Nothing` on success, `Err` on failure.
    UnstageFile {
        worktree_path: PathBuf,
        path: String,
    },

    /// Run `git commit -m <message>` synchronously. Returns
    /// `EventReaction::Status(Info(success_message))` on success or
    /// `EventReaction::Status(Error("Commit failed: <e>"))` on failure. The
    /// caller pre-formats `success_message` because it depends on
    /// view-side bindings the engine cannot resolve.
    CommitChanges {
        worktree_path: PathBuf,
        message: String,
        success_message: String,
    },

    /// Spawn a `git push` worker. Returns `EventReaction::Status(Busy("Pushing
    /// to remote\u{2026}"))` immediately; completion is reported via
    /// `WorkerEvent::PushCompleted`.
    Push { worktree_path: PathBuf },

    /// Spawn a `git pull` worker for either a project's leading branch or
    /// the current session's branch. Returns
    /// `EventReaction::Status(Busy(busy_message))` if the in-flight guard
    /// accepted the request, or `EventReaction::Status(Warning(
    /// already_running_message))` if another pull is already running for
    /// the same repo path. Completion is reported via
    /// `WorkerEvent::PullCompleted`.
    Pull {
        repo_path: PathBuf,
        target: PullTarget,
        busy_message: String,
        already_running_message: String,
    },

    /// Open a filesystem path via the user's OS handler. Fire-and-forget
    /// spawn; result posts back as `WorkerEvent::OpenPathCompleted` which the
    /// existing reaction handler surfaces as a status message.
    OpenPath { path: PathBuf, target: String },

    /// Toggle a session's `auto_reopen_enabled` flag. The App caller passes
    /// the new value + branch name (computed from the cloned selected session
    /// upfront — preserves the App-side capture-before-mutate behaviour). The
    /// engine performs the upsert in-place; if the session was removed
    /// in-flight, falls back to `session_store.set_auto_reopen_enabled`
    /// (matches the original method's race-handling).
    ToggleAgentAutoReopen {
        session_id: String,
        branch_name: String,
        new_enabled: bool,
    },

    /// Remove a companion terminal from the engine (drops `PtyClient`, killing
    /// the child). Returns a typed view carrying the terminal's label so the
    /// App can format status + reconcile `active_terminal_id` (view field).
    DeleteTerminal { terminal_id: String },

    /// Persist the global `env` block to the user config file. The engine
    /// delegates to `Engine::config_saver` so the TUI can render the
    /// `[keys]` section with `RuntimeBindings` while the web layer can plug
    /// in its own persistence. Fire-and-forget: completion arrives as
    /// `WorkerEvent::GlobalEnvPersistenceCompleted`.
    PersistGlobalEnv {
        env: std::collections::BTreeMap<String, String>,
    },

    /// Reload the user config from disk, validate it, and resync project
    /// records against the session store. Fire-and-forget: completion
    /// arrives as `WorkerEvent::ConfigReloadReady`.
    ReloadConfig,

    /// Write a canonical (fully-templated) config to disk, overwriting the
    /// existing file. Used by the config-reload-failed modal to restore a
    /// known-good config. Fire-and-forget: completion arrives as
    /// `WorkerEvent::ConfigRecoverCompleted`.
    RecoverConfig,
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
                let paths = self.paths.clone();
                let config = self.config.clone();
                Ok(self.spawn_command_worker(
                    CommandWorkerSpec {
                        label: "create-agent".into(),
                        in_flight_key: Some(InFlightKey::CreateAgent),
                        busy_status: Some(StatusUpdate::busy(busy_message)),
                        already_running_status: Some(StatusUpdate::error(
                            "An agent is already being created or forked.",
                        )),
                        panic_event: Some(Box::new(|reason| {
                            WorkerEvent::CreateAgentFailed(format!(
                                "Agent-creation worker panicked: {reason}"
                            ))
                        })),
                    },
                    move |tx| {
                        crate::agent_job::run_create_agent_job(
                            *request, paths, config, tx, term_size,
                        );
                    },
                ))
            }

            Command::DispatchAgentLaunch { request } => {
                let branch_name = request.session.branch_name.clone();
                let session_id = request.session.id.clone();
                // Pre-check in-flight so the View carries the exact "already
                // launching" message regardless of the primitive's generic
                // already-running fallback.
                if self.is_in_flight(&InFlightKey::AgentLaunch(session_id.clone())) {
                    return Ok(EventReaction::DispatchAgentLaunchView(Box::new(
                        DispatchAgentLaunchView {
                            session_id,
                            launched: false,
                            status: Some(StatusUpdate::info(format!(
                                "Agent \"{}\" is already launching.",
                                branch_name,
                            ))),
                        },
                    )));
                }
                // Clone for the panic event closure before `request` is
                // consumed by the job closure. `AgentLaunchRequest` is
                // `Clone`, which keeps the panic recovery path symmetric
                // with `process_agent_launch_failed`.
                let panic_request = (*request).clone();
                let reaction = self.spawn_command_worker(
                    CommandWorkerSpec {
                        label: format!("agent-launch:{session_id}"),
                        in_flight_key: Some(InFlightKey::AgentLaunch(session_id.clone())),
                        busy_status: None, // View variant carries the user-facing status
                        already_running_status: None, // handled by the pre-check above
                        panic_event: Some(Box::new(move |reason| {
                            WorkerEvent::AgentLaunchFailed(Box::new(AgentLaunchFailedData {
                                request: panic_request,
                                message: format!("Agent-launch worker panicked: {reason}"),
                            }))
                        })),
                    },
                    move |tx| {
                        crate::agent_job::run_agent_launch_job(*request, tx);
                    },
                );
                // Wrap the primitive's return into the View variant so App
                // callers keep a single pattern-match shape.
                match reaction {
                    EventReaction::Nothing => Ok(EventReaction::DispatchAgentLaunchView(Box::new(
                        DispatchAgentLaunchView {
                            session_id,
                            launched: true,
                            status: None,
                        },
                    ))),
                    EventReaction::Status(status) => Ok(EventReaction::DispatchAgentLaunchView(
                        Box::new(DispatchAgentLaunchView {
                            session_id,
                            launched: false,
                            status: Some(status),
                        }),
                    )),
                    other => Ok(other),
                }
            }

            Command::StageFile {
                worktree_path,
                path,
            } => {
                crate::git::stage_file(&worktree_path, &path)?;
                Ok(EventReaction::Nothing)
            }

            Command::UnstageFile {
                worktree_path,
                path,
            } => {
                crate::git::unstage_file(&worktree_path, &path)?;
                Ok(EventReaction::Nothing)
            }

            Command::CommitChanges {
                worktree_path,
                message,
                success_message,
            } => match crate::git::commit(&worktree_path, &message) {
                Ok(_) => Ok(EventReaction::Status(StatusUpdate::info(success_message))),
                Err(e) => Ok(EventReaction::Status(StatusUpdate::error(format!(
                    "Commit failed: {e}"
                )))),
            },

            Command::Push { worktree_path } => Ok(self.spawn_command_worker(
                CommandWorkerSpec {
                    label: "push".into(),
                    in_flight_key: None,
                    busy_status: Some(StatusUpdate::busy("Pushing to remote\u{2026}")),
                    already_running_status: None,
                    panic_event: Some(Box::new(|reason| {
                        WorkerEvent::PushCompleted(Err(format!("Push worker panicked: {reason}")))
                    })),
                },
                move |tx| {
                    let result = crate::git::push(&worktree_path)
                        .map(|_| ())
                        .map_err(|e| e.to_string());
                    let _ = tx.send(WorkerEvent::PushCompleted(result));
                },
            )),

            Command::Pull {
                repo_path,
                target,
                busy_message,
                already_running_message,
            } => {
                let repo_key = repo_path.to_string_lossy().into_owned();
                // Clones for the panic event closure; the job closure
                // consumes the originals.
                let repo_key_for_panic = repo_key.clone();
                let target_for_panic = target.clone();
                Ok(self.spawn_command_worker(
                    CommandWorkerSpec {
                        label: format!("pull:{repo_key}"),
                        in_flight_key: Some(InFlightKey::Pull(repo_key.clone())),
                        busy_status: Some(StatusUpdate::busy(busy_message)),
                        already_running_status: Some(StatusUpdate::warning(
                            already_running_message,
                        )),
                        panic_event: Some(Box::new(move |reason| WorkerEvent::PullCompleted {
                            repo_path: repo_key_for_panic,
                            target: target_for_panic,
                            result: Err(format!("Pull worker panicked: {reason}")),
                        })),
                    },
                    move |tx| {
                        let result = match &target {
                            PullTarget::Project { leading_branch, .. } => {
                                let leading_branch = match leading_branch.clone() {
                                    Some(branch) => Ok(branch),
                                    None => crate::git::current_branch(&repo_path).map(|branch| {
                                        crate::project_browser::leading_branch_for_project(
                                            &repo_path, &branch,
                                        )
                                    }),
                                };
                                leading_branch
                                    .and_then(|branch| {
                                        crate::git::switch_branch_if_needed(&repo_path, &branch)?;
                                        if crate::git::has_tracked_changes(&repo_path)? {
                                            return Err(anyhow::anyhow!(
                                                "Refresh blocked because the source checkout has uncommitted changes."
                                            ));
                                        }
                                        crate::git::pull_branch(&repo_path, &branch)
                                    })
                                    .map(|_| crate::git::current_branch(&repo_path).ok())
                                    .map_err(|e| e.to_string())
                            }
                            PullTarget::Session => crate::git::pull_current_branch(&repo_path)
                                .map(|_| None)
                                .map_err(|e| e.to_string()),
                        };
                        let _ = tx.send(WorkerEvent::PullCompleted {
                            repo_path: repo_key,
                            target,
                            result,
                        });
                    },
                ))
            }

            Command::OpenPath { path, target } => {
                let display = path.display().to_string();
                let busy_message = format!("Opening {target}: {display}");
                let target_for_job = target.clone();
                let target_for_panic = target.clone();
                Ok(self.spawn_command_worker(
                    CommandWorkerSpec {
                        label: format!("open-path:{target}"),
                        in_flight_key: None,
                        busy_status: Some(StatusUpdate::busy(busy_message)),
                        already_running_status: None,
                        panic_event: Some(Box::new(move |reason| WorkerEvent::OpenPathCompleted {
                            target: target_for_panic,
                            result: Err(format!("OpenPath worker panicked: {reason}")),
                        })),
                    },
                    move |tx| {
                        let result =
                            crate::startup::open_path(&path).map_err(|err| format!("{err:#}"));
                        let _ = tx.send(WorkerEvent::OpenPathCompleted {
                            target: target_for_job,
                            result,
                        });
                    },
                ))
            }

            Command::ToggleAgentAutoReopen {
                session_id,
                branch_name,
                new_enabled,
            } => {
                if let Some(current) = self.sessions.iter_mut().find(|c| c.id == session_id) {
                    // Persist FIRST so a DB failure leaves in-memory state
                    // untouched and the UI continues showing the prior
                    // (still-true) value. Mirrors finish_delete_session's
                    // DB-first pattern.
                    let mut candidate = current.clone();
                    candidate.auto_reopen_enabled = new_enabled;
                    candidate.updated_at = chrono::Utc::now();
                    self.session_store.upsert_session(&candidate)?;
                    *current = candidate;
                } else {
                    self.session_store
                        .set_auto_reopen_enabled(&session_id, new_enabled)?;
                }
                Ok(EventReaction::Status(StatusUpdate::info(format!(
                    "Startup auto-reopen {} for agent \"{}\".",
                    if new_enabled { "enabled" } else { "disabled" },
                    branch_name,
                ))))
            }

            Command::DeleteTerminal { terminal_id } => {
                let label = self
                    .companion_terminals
                    .get(&terminal_id)
                    .map(|t| t.label.clone());
                // Removing from the map drops PtyClient, which kills the child.
                self.companion_terminals.remove(&terminal_id);
                Ok(EventReaction::DeleteTerminalView(Box::new(
                    DeleteTerminalView { terminal_id, label },
                )))
            }

            Command::PersistGlobalEnv { env } => {
                let mut config = self.config.clone();
                config.env = env.clone();
                self.config_saver.persist_global_env(
                    env,
                    config,
                    self.paths.config_path.clone(),
                    self.worker_tx.clone(),
                );
                Ok(EventReaction::Nothing)
            }

            Command::ReloadConfig => {
                self.config_saver
                    .reload_config(self.paths.clone(), self.worker_tx.clone());
                Ok(EventReaction::Nothing)
            }

            Command::RecoverConfig => {
                self.config_saver.recover_config(
                    self.paths.config_path.clone(),
                    self.config.clone(),
                    self.worker_tx.clone(),
                );
                Ok(EventReaction::Nothing)
            }
        }
    }
}
