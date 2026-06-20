//! The `Command` enum — the §4.5 engine-operation vocabulary. Every
//! mutation or background-spawn the Engine performs in response to a
//! TUI key or a web-UI click is named here and dispatched through
//! `Engine::apply`.

use std::path::PathBuf;

use crate::engine::events::{
    BeginDeleteSessionView, DeleteTerminalView, DispatchAgentLaunchView, DoDeleteSessionView,
    EventReaction, FinishDeleteSessionView, ProjectPersistenceOutcome, ProjectPersistenceView,
    StatusUpdate, WorktreeRemoval,
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
        removal: WorktreeRemoval,
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
    /// Persist a project mutation. The `Add` action is handled INLINE
    /// (synchronously): the handler writes SQLite and config.toml in `apply` and
    /// returns `EventReaction::ProjectPersistenceOutcome(Added)` on success or an
    /// error-toned `EventReaction::Status` (the add rolled back) on failure — both
    /// immediately, no worker. All OTHER actions go through the background worker:
    /// fire-and-forget, the worker posts `WorkerEvent::ProjectPersistenceCompleted`
    /// back, which surfaces as `EventReaction::ProjectPersistenceOutcome` in the
    /// next `drain_events` pass.
    ///
    /// Boxed to keep the enum size within the clippy `large_enum_variant`
    /// threshold (`ProjectPersistenceAction` is 248 bytes unboxed).
    PersistProject(Box<ProjectPersistenceAction>),

    /// Remove a project AND cascade-delete its agents' records + runtime,
    /// KEEPING their worktrees on disk. Tolerates a "ghost" project id that
    /// exists only via orphaned sessions (no project record): the orphaned
    /// sessions are cleared and the project-record delete is a harmless no-op.
    /// The project-record + config removal run via the persistence worker.
    RemoveProject {
        project_id: String,
        project_name: String,
    },

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

    /// Discard a single unstaged file's changes. Synchronous git call (`git
    /// checkout -- <path>` for tracked files, `rm` for untracked ones — the
    /// `is_untracked` flag selects which). Destructive: it permanently throws
    /// away working-tree changes (or deletes the file outright when untracked).
    /// Returns `EventReaction::Status(Info(...))` with an actionable message on
    /// success; an `Err` propagates to the caller on failure.
    DiscardFile {
        worktree_path: PathBuf,
        path: String,
        is_untracked: bool,
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
    /// eager-saves through `Engine::config_writer` (the shared off-thread
    /// queue) and reports the result synchronously, rolling the in-memory env
    /// back if the write fails.
    PersistGlobalEnv {
        env: std::collections::BTreeMap<String, String>,
    },

    /// Reload the user config from disk, validate it, and resync project
    /// records against the session store. Opens a reload barrier (quiesces the
    /// config writer + defers config-mutating commands) and kicks off the
    /// surface's reload worker; completion arrives as
    /// `WorkerEvent::ConfigReloadReady`, which closes the barrier.
    ReloadConfig,

    /// Write a canonical (fully-templated) config to disk, overwriting the
    /// existing file. Used by the config-reload-failed modal to restore a
    /// known-good config. Renders via `Engine::surface` and writes synchronously
    /// through `config_write::write_config_secure`, returning the result status.
    RecoverConfig,

    /// Generate an AI commit message for a session by running the session's
    /// provider in one-shot mode over the staged diff. Mirrors the TUI's
    /// `trigger_ai_commit_message`: builds `default_commit_prompt() + diff`,
    /// spawns a thread to call `run_oneshot`, and posts the result back via
    /// `WorkerEvent::CommitMessageGenerated` / `CommitMessageFailed`. Returns
    /// `EventReaction::Status(Busy(...))` immediately; an error status if the
    /// session is unknown or there is nothing staged to summarize.
    GenerateCommitMessage { session_id: String },

    /// Persist a custom display order for the agent sessions within a single
    /// project. `session_ids` must be EXACTLY the full set of that project's
    /// sessions — no missing ids, no extras, no duplicates, all belonging to
    /// `project_id` — otherwise the engine returns an actionable error and
    /// touches nothing. On success it writes the order to storage and reorders
    /// the matching rows of `self.sessions` in place, leaving other projects'
    /// rows in their existing relative positions. Returns
    /// `EventReaction::Nothing` (silent success; the refreshed view is the
    /// feedback), since reorders are high-frequency during a drag.
    ReorderSessions {
        project_id: String,
        session_ids: Vec<String>,
    },

    /// Persist a custom display order for the workspace's projects.
    /// `project_ids` must be EXACTLY the full set of known project ids (same
    /// strict validation as [`Command::ReorderSessions`]). On success it writes
    /// the order to storage and reorders `self.projects` to match. Returns
    /// `EventReaction::Nothing`.
    ReorderProjects { project_ids: Vec<String> },

    /// Run a configured text macro against a live PTY target. `target_id` names
    /// either an agent session (surface `Agent`) or a companion terminal
    /// (surface `Terminal`); the engine resolves which via the same
    /// providers-then-terminals lookup the web actor's `pty_for` uses. Resolves
    /// the macro by name (unknown → error Status), surface-checks it against the
    /// target (mismatch → error Status), transforms the text via
    /// `dux_core::macros::macro_payload_bytes`, and writes it to the target's
    /// PTY. Returns `EventReaction::Status(Info("Sent macro \"<name>\"."))` on
    /// success — the TUI's exact wording.
    RunMacro { target_id: String, name: String },

    /// Wholesale-replace the `[macros]` config and persist it. Adopts `macros`
    /// into the running `config.macros` immediately (so the ViewModel's `macros`
    /// refreshes without a manual reload) and eager-saves through
    /// `Engine::config_writer`, reporting the result synchronously. Keep-and-
    /// report: a failed write leaves the new macros active for the session.
    ///
    /// Last-write-wins: the replacement is the editor's whole set, seeded from a
    /// pre-edit snapshot of `[macros]`. A Save therefore clobbers any concurrent
    /// hand-edit to the `[macros]` block made on disk between snapshot and save —
    /// identical to the `PersistGlobalEnv` precedent. Acceptable for the
    /// single-operator model; a multi-writer setup would need read-modify-merge.
    UpdateMacros { macros: crate::config::MacrosConfig },

    /// Point the changed-files watch at a session's worktree (or clear it with
    /// `None`). Mirrors the engine half of the TUI's `reload_changed_files`, but
    /// keeps git OFF the engine actor thread: resolves the session, sets
    /// `watched_worktree` + `watched_session_id`, and empties the lists
    /// synchronously (cheap), then spawns a one-shot worker to compute the
    /// staged/unstaged lists off-thread. The worker's `ChangedFilesReady` event
    /// populates the pane a few ticks later via the normal drain. Returns
    /// `EventReaction::Nothing` — the refreshed ViewModel broadcast is the
    /// feedback (no status toast on every selection). The web sends this when a
    /// browser selects a session so the global poller knows which worktree the
    /// client is viewing.
    WatchChangedFiles { session_id: Option<String> },
}

impl Engine {
    /// Single dispatch point for every engine-affecting operation. The
    /// TUI's input layer calls this with a `Command` translated from key
    /// events; the web layer (sub-project #3) will call it with `Command`s
    /// deserialized from WebSocket messages. Returns an `EventReaction`
    /// the caller routes through its view-applier.
    pub fn apply(&mut self, command: Command) -> anyhow::Result<EventReaction> {
        // While a config reload barrier is open, hold any config-mutating
        // command until the reload lands so it re-applies against the
        // freshly-reloaded config instead of racing it (see
        // `Engine::is_config_mutating` and the `ConfigReloadReady` handler).
        if self.reloading && Self::is_config_mutating(&command) {
            self.deferred_commands.push(command);
            return Ok(EventReaction::Nothing);
        }
        match command {
            Command::FinishDeleteSession {
                session_id,
                removal,
                update_status,
            } => {
                let Some(outcome) = self.finish_delete_session(&session_id)? else {
                    return Ok(EventReaction::Nothing);
                };
                Ok(EventReaction::FinishDeleteSessionView(Box::new(
                    FinishDeleteSessionView {
                        session_id,
                        outcome,
                        removal,
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
                    },
                )))
            }
            Command::PersistProject(action) => {
                if let ProjectPersistenceAction::Add {
                    project,
                    status_message,
                } = *action
                {
                    // Insert the project into SQLite inline so we can roll back
                    // the row if the subsequent config write fails.
                    self.session_store
                        .upsert_project(&crate::config::ProjectConfig {
                            id: project.id.clone(),
                            path: project.path.clone(),
                            name: Some(project.name.clone()),
                            default_provider: project
                                .explicit_default_provider
                                .as_ref()
                                .map(|p| p.as_str().to_string()),
                            leading_branch: project.leading_branch.clone(),
                            auto_reopen_agents: project.auto_reopen_agents,
                            startup_command: project.startup_command.clone(),
                            env: project.env.clone(),
                        })?;
                    let id = project.id.clone();
                    // Add to in-memory list before the config write.
                    self.projects.push(project.clone());
                    match self.persist_projects_to_config() {
                        Ok(()) => Ok(EventReaction::ProjectPersistenceOutcome(Box::new(
                            ProjectPersistenceOutcome {
                                action: ProjectPersistenceAction::Add {
                                    project,
                                    status_message: status_message.clone(),
                                },
                                view: ProjectPersistenceView::Added {
                                    project_id: id,
                                    status_message,
                                },
                            },
                        ))),
                        Err(e) => {
                            // Config write failed — roll back the in-memory project
                            // and the SQLite row so the state stays consistent.
                            self.projects.retain(|p| p.id != id);
                            if let Err(db_err) = self.session_store.delete_project(&id) {
                                return Ok(EventReaction::Status(StatusUpdate::error(format!(
                                    "Project add failed and couldn't be cleaned up — it may \
                                     reappear on restart. Config error: {e:#}. DB cleanup error: \
                                     {db_err:#}"
                                ))));
                            }
                            Ok(EventReaction::Status(StatusUpdate::error(format!(
                                "Project add failed and was rolled back — config.toml could \
                                 not be updated: {e:#}"
                            ))))
                        }
                    }
                } else {
                    self.spawn_project_persistence(*action);
                    Ok(EventReaction::Nothing)
                }
            }

            Command::RemoveProject {
                project_id,
                project_name,
            } => {
                // Refuse while one of the project's agents has an in-flight async
                // worktree removal: proceeding could race `git::remove_worktree`
                // and delete a worktree we promised to keep.
                if self
                    .sessions
                    .iter()
                    .any(|s| s.project_id == project_id && self.pending_deletions.contains(&s.id))
                {
                    return Ok(EventReaction::Status(StatusUpdate::error(format!(
                        "An agent in \"{project_name}\" is still being removed — try again in a moment."
                    ))));
                }
                // Was this a real, config-backed project (vs. a ghost id that only
                // exists as orphaned session rows)? A ghost was never written to
                // config, so there is nothing to rewrite for it below.
                let was_real = self.projects.iter().any(|p| p.id == project_id);
                // Delete the project row, every session record, and their PR rows in
                // a SINGLE transaction, so a mid-way failure cannot leave the project
                // half-removed (e.g. agents gone but the project row surviving to
                // reappear on restart). The rows are gone before any in-memory state
                // changes, and on error nothing is mutated. Tolerates a ghost id.
                let removed = self.session_store.remove_project_records(&project_id)?;
                // The DB rows are gone; finish_delete_session now only runs the
                // (infallible) in-memory/runtime teardown per session — its own
                // delete_session is a no-op on the already-removed row. Dropping
                // each provider SIGKILLs the PTY process group; worktrees are
                // deliberately left on disk.
                for id in &removed {
                    let _ = self.finish_delete_session(id);
                }
                // Remove the project from memory synchronously so a concurrent
                // CreateAgent cannot attach a new session to a project mid-removal.
                self.projects.retain(|p| p.id != project_id);
                let detail = match removed.len() {
                    0 => String::new(),
                    1 => " and its agent".to_string(),
                    n => format!(" and its {n} agents"),
                };
                // Keep portable config in sync with the now-removed project. Only a
                // real project needs this (a ghost was never in config). The DB delete
                // already committed, so a config-write failure is reported but does not
                // undo the removal — it warns that the project may reappear on restart.
                if was_real && let Err(e) = self.persist_projects_to_config() {
                    return Ok(EventReaction::Status(StatusUpdate::error(format!(
                        "Removed \"{project_name}\"{detail} from dux, but updating config.toml \
                         failed: {e}. The project may reappear on restart — check the file is \
                         writable."
                    ))));
                }
                Ok(EventReaction::Status(StatusUpdate::info(format!(
                    "Removed project \"{project_name}\"{detail}. Worktrees were kept on disk."
                ))))
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

            Command::DiscardFile {
                worktree_path,
                path,
                is_untracked,
            } => {
                crate::git::discard_file(&worktree_path, &path, is_untracked)?;
                let message = if is_untracked {
                    format!("Deleted untracked file \"{path}\".")
                } else {
                    format!(
                        "Discarded unstaged changes to \"{path}\" — staged changes, if any, are kept."
                    )
                };
                Ok(EventReaction::Status(StatusUpdate::info(message)))
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
                let label = format!("open-path:{target}");
                let target_for_panic = target.clone();
                Ok(self.spawn_command_worker(
                    CommandWorkerSpec {
                        label,
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
                        let _ = tx.send(WorkerEvent::OpenPathCompleted { target, result });
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
                // Eager save through the queue, with rollback: swap the new env
                // in, persist synchronously, and restore the previous env if the
                // write fails so in-memory state never diverges from disk.
                let previous = std::mem::replace(&mut self.config.env, env);
                if let Err(e) = self.config_writer.save_eager(self.config.clone()) {
                    self.config.env = previous;
                    return Ok(EventReaction::Status(StatusUpdate::error(format!(
                        "Couldn't save global environment variables: {e}"
                    ))));
                }
                let msg = if self.config.env.is_empty() {
                    "Global environment variables cleared.".to_string()
                } else {
                    format!(
                        "Saved {} global environment variable(s). New agents and terminals will receive them unless a project overrides the same key.",
                        self.config.env.len()
                    )
                };
                Ok(EventReaction::Status(StatusUpdate::info(msg)))
            }

            Command::ReloadConfig => {
                // Reject a reentrant reload: a second reload while one is in
                // flight would drop the live `reload_guard` (resuming the writer
                // mid-reload) and spawn a second worker whose completion would
                // close a barrier that is no longer the one it opened. Refuse
                // instead — the in-flight reload will land on its own.
                if self.reloading {
                    return Ok(EventReaction::Status(StatusUpdate::info(
                        "A config reload is already in progress.",
                    )));
                }
                // Open the reload barrier: quiesce the writer (so no queued save
                // races the reload), mark `reloading` (so config-mutating
                // commands defer), and kick off the surface's reload worker. The
                // barrier closes when `ConfigReloadReady` lands.
                self.reloading = true;
                self.reload_guard = Some(self.config_writer.quiesce());
                self.surface
                    .reload(self.paths.clone(), self.worker_tx.clone());
                Ok(EventReaction::Nothing)
            }

            Command::RecoverConfig => {
                // Reject recovery while a reload barrier is open: a reload already
                // holds the writer quiesce, and recovery taking its OWN quiesce
                // would, on its guard drop, resume the writer while the reload is
                // still mid-flight. Refuse and let the reload finish first.
                if self.reloading {
                    return Ok(EventReaction::Status(StatusUpdate::info(
                        "A config reload is in progress; try recovering again in a moment.",
                    )));
                }
                // Overwrite a corrupt on-disk config with a fresh render of the
                // in-memory config. Quiesce the writer for the duration of the
                // direct write so a queued save can't clobber the recovery.
                let _guard = self.config_writer.quiesce();
                let body = self.surface.recover_render(&self.config);
                match crate::config_write::write_config_secure(&self.paths.config_path, &body) {
                    Ok(()) => Ok(EventReaction::Status(StatusUpdate::info(
                        "Restored the last working configuration to config.toml.",
                    ))),
                    Err(e) => Ok(EventReaction::Status(StatusUpdate::error(format!(
                        "Couldn't restore the last working configuration: {e:#}"
                    )))),
                }
            }

            Command::GenerateCommitMessage { session_id } => {
                let Some(session) = self.sessions.iter().find(|s| s.id == session_id).cloned()
                else {
                    return Ok(EventReaction::Status(StatusUpdate::error(
                        "Unknown session.",
                    )));
                };
                let worktree = PathBuf::from(&session.worktree_path);
                let prompt_prefix = self.config.default_commit_prompt();
                let cfg = crate::config::provider_config(&self.config, &session.provider);
                let prov = crate::provider::create_provider(session.provider.as_str(), cfg);
                let tx = self.worker_tx.clone();
                // Read the staged diff AND run the provider off the engine thread.
                // `git diff --staged` can be slow on a large staged set, and on the
                // web server every request runs on the single engine thread —
                // reading it inline would stall every other session. The empty-diff
                // and read-error cases are reported through the same failure event
                // the provider path uses, so the UI surfaces them the same way.
                std::thread::spawn(move || {
                    let diff_text = match crate::git::staged_diff_text(&worktree) {
                        Ok(d) if d.trim().is_empty() => {
                            let _ = tx.send(crate::worker::WorkerEvent::CommitMessageFailed {
                                session_id,
                                error: "No staged changes to summarize. Stage files first."
                                    .to_string(),
                            });
                            return;
                        }
                        Ok(d) => d,
                        Err(e) => {
                            let _ = tx.send(crate::worker::WorkerEvent::CommitMessageFailed {
                                session_id,
                                error: format!("Failed to read the staged diff: {e}"),
                            });
                            return;
                        }
                    };
                    let prompt = format!("{prompt_prefix}\n\n{diff_text}");
                    match prov.run_oneshot(&prompt, &worktree) {
                        Ok(message) => {
                            let _ = tx.send(crate::worker::WorkerEvent::CommitMessageGenerated {
                                session_id,
                                message,
                            });
                        }
                        Err(e) => {
                            let _ = tx.send(crate::worker::WorkerEvent::CommitMessageFailed {
                                session_id,
                                error: e.to_string(),
                            });
                        }
                    }
                });
                Ok(EventReaction::Status(StatusUpdate::busy(
                    "Generating an AI commit message from the staged diff\u{2026}",
                )))
            }

            Command::ReorderSessions {
                project_id,
                session_ids,
            } => {
                self.reorder_sessions(&project_id, &session_ids)?;
                Ok(EventReaction::Nothing)
            }

            Command::ReorderProjects { project_ids } => {
                self.reorder_projects(&project_ids)?;
                Ok(EventReaction::Nothing)
            }

            Command::RunMacro { target_id, name } => self.run_macro(&target_id, &name),

            Command::UpdateMacros { macros } => {
                // Keep-and-report (no rollback): adopt the new macros into the
                // running config immediately so the ViewModel reflects them, then
                // eager-save through the queue. If the write fails the macros stay
                // active for this session — we only report that the on-disk file
                // may be stale, rather than reverting a change the user made.
                self.config.macros = macros;
                let count = self.config.macros.entries.len();
                if let Err(e) = self.config_writer.save_eager(self.config.clone()) {
                    return Ok(EventReaction::Status(StatusUpdate::error(format!(
                        "Macros updated this session, but saving to config failed: {e}"
                    ))));
                }
                let msg = if count == 0 {
                    "All macros removed. The macro list is now empty.".to_string()
                } else {
                    format!("Saved {count} macro(s).")
                };
                Ok(EventReaction::Status(StatusUpdate::info(msg)))
            }

            Command::WatchChangedFiles { session_id } => {
                // Cheap on the actor thread: resolve + set the watch (no git),
                // then compute changed files OFF-thread in a one-shot worker. The
                // `ChangedFilesReady` event lands via the normal drain (it
                // path-checks the worktree, so a moved watch drops a stale
                // result) → ViewModel update → broadcast. On clear (`None`),
                // `set_watched_session` already emptied the lists synchronously,
                // so the ViewModel reflects the cleared pane immediately.
                if let Some(worktree) = self.set_watched_session(session_id.as_deref()) {
                    self.spawn_changed_files_refresh(worktree);
                }
                Ok(EventReaction::Nothing)
            }
        }
    }

    /// Run a configured text macro against a live PTY target. Mirrors the TUI's
    /// macro bar: resolve the macro by name, gate it by the target's surface,
    /// translate newlines via the shared core transform, and write to the PTY.
    /// See [`Command::RunMacro`] for the full contract.
    fn run_macro(&mut self, target_id: &str, name: &str) -> anyhow::Result<EventReaction> {
        // Resolve the target's surface: an agent provider is `Agent`, a companion
        // terminal is `Terminal`. Unknown id → error.
        let surface = if self.providers.contains_key(target_id) {
            crate::model::SessionSurface::Agent
        } else if self.companion_terminals.contains_key(target_id) {
            crate::model::SessionSurface::Terminal
        } else {
            return Ok(EventReaction::Status(StatusUpdate::error(format!(
                "No live agent or terminal for target \"{target_id}\". Reconnect it and try again."
            ))));
        };

        // Resolve the macro entry by name. Unknown → error naming it.
        let Some(entry) = self.config.macros.entries.get(name) else {
            return Ok(EventReaction::Status(StatusUpdate::error(format!(
                "Macro \"{name}\" does not exist."
            ))));
        };

        // Surface gate: refuse a macro whose surface doesn't match the target.
        // The TUI bar simply omits mismatches; the wire returns an explicit error.
        if !crate::macros::macro_matches_surface(entry.surface, surface) {
            let target_kind = match surface {
                crate::model::SessionSurface::Agent => "agent",
                crate::model::SessionSurface::Terminal => "terminal",
            };
            return Ok(EventReaction::Status(StatusUpdate::error(format!(
                "Macro \"{name}\" is not available on {target_kind} targets ({}).",
                entry.surface.label()
            ))));
        }

        let payload = crate::macros::macro_payload_bytes(&entry.text);
        // Unified PTY lookup: providers first, then companion terminals — the
        // same order the web actor's `pty_for` uses.
        let client = self
            .providers
            .get(target_id)
            .or_else(|| self.companion_terminals.get(target_id).map(|t| &t.client));
        if let Some(client) = client {
            client.write_bytes(&payload)?;
        }
        Ok(EventReaction::Status(StatusUpdate::info(format!(
            "Sent macro \"{name}\"."
        ))))
    }

    /// Validate and apply a new per-project session order. See
    /// [`Command::ReorderSessions`] for the strict-set contract. On success the
    /// store is written first (DB-first, matching the rest of the engine), then
    /// `self.sessions` is re-sorted so the project's rows follow `session_ids`
    /// while every other project's rows keep their existing relative order.
    fn reorder_sessions(&mut self, project_id: &str, session_ids: &[String]) -> anyhow::Result<()> {
        let current: Vec<String> = self
            .sessions
            .iter()
            .filter(|s| s.project_id == project_id)
            .map(|s| s.id.clone())
            .collect();
        validate_reorder(&current, session_ids, "agent")?;

        self.session_store
            .reorder_sessions(project_id, session_ids)?;

        // Build a position lookup for this project's ids, then stably re-sort
        // the whole Vec. Rows outside this project sort by their existing index
        // (kept stable); rows inside it sort by their new position. Because the
        // sort is stable and out-of-project keys preserve the original index,
        // cross-project relative order is untouched.
        let new_pos: std::collections::HashMap<&str, usize> = session_ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();
        reorder_in_place(&mut self.sessions, |s| {
            if s.project_id == project_id {
                new_pos.get(s.id.as_str()).copied()
            } else {
                None
            }
        });
        Ok(())
    }

    /// Validate and apply a new project order. See [`Command::ReorderProjects`].
    fn reorder_projects(&mut self, project_ids: &[String]) -> anyhow::Result<()> {
        let current: Vec<String> = self.projects.iter().map(|p| p.id.clone()).collect();
        validate_reorder(&current, project_ids, "project")?;

        self.session_store.reorder_projects(project_ids)?;

        let new_pos: std::collections::HashMap<&str, usize> = project_ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();
        reorder_in_place(&mut self.projects, |p| new_pos.get(p.id.as_str()).copied());
        Ok(())
    }

    /// Persist the current in-memory session order to storage, per project. The
    /// TUI calls this after its sort actions mutate `self.sessions` so the
    /// chosen order survives a reload and matches the web UI by construction.
    /// Does NOT re-sort the Vec (it is already in the desired order); it only
    /// writes each project's ordered id list. Errors propagate to the caller.
    pub fn persist_session_order(&self) -> anyhow::Result<()> {
        use std::collections::BTreeMap;
        // Preserve the Vec's project encounter order so the writes are
        // deterministic; the per-project id lists follow the Vec order exactly.
        let mut per_project: BTreeMap<&str, Vec<String>> = BTreeMap::new();
        for session in &self.sessions {
            per_project
                .entry(session.project_id.as_str())
                .or_default()
                .push(session.id.clone());
        }
        for (project_id, ids) in per_project {
            self.session_store.reorder_sessions(project_id, &ids)?;
        }
        Ok(())
    }
}

/// Strict reorder validation: `requested` must be a permutation of `current`
/// (same elements, no missing, no extras, no duplicates). `noun` names the
/// entity for the error message (e.g. "agent", "project").
fn validate_reorder(current: &[String], requested: &[String], noun: &str) -> anyhow::Result<()> {
    use std::collections::HashSet;
    let current_set: HashSet<&str> = current.iter().map(String::as_str).collect();
    let requested_set: HashSet<&str> = requested.iter().map(String::as_str).collect();

    if requested.len() != requested_set.len() {
        anyhow::bail!("Cannot reorder {noun}s: the new order contains duplicate ids.");
    }
    if requested_set != current_set {
        anyhow::bail!(
            "Cannot reorder {noun}s: the new order must list exactly the current {noun}s (expected {} ids, got {}).",
            current.len(),
            requested.len(),
        );
    }
    Ok(())
}

/// Re-sort `items` so that elements for which `position` returns `Some(p)` are
/// ordered by `p` among themselves, while elements returning `None` keep their
/// original relative order. Note that absolute interleaving between the two
/// groups MAY change (positioned items can compact toward the front of the
/// range they sort into); what is guaranteed is each group's internal relative
/// order. That is sufficient here because both surfaces group sessions by
/// project before display, so only per-project relative order is observable.
fn reorder_in_place<T>(items: &mut Vec<T>, position: impl Fn(&T) -> Option<usize>) {
    // Build the desired index order: a stable sort of the original indices by
    // (key, original_index), where the key is the new position for positioned
    // items and the original index for the rest. Positioned items can interleave
    // with None items, but because we then write the reordered elements back
    // into the SAME slot sequence, the relative order of None items is preserved
    // and positioned items land in ascending-position order.
    let mut indices: Vec<usize> = (0..items.len()).collect();
    indices.sort_by(|&a, &b| {
        let ka = position(&items[a]).unwrap_or(a);
        let kb = position(&items[b]).unwrap_or(b);
        ka.cmp(&kb).then(a.cmp(&b))
    });

    // Move out every element, then re-collect in the sorted index order. Works
    // for non-Clone `T` (Project/AgentSession are not trivially copyable here).
    let mut taken: Vec<Option<T>> = items.drain(..).map(Some).collect();
    *items = indices
        .into_iter()
        .map(|i| taken[i].take().expect("each index visited once"))
        .collect();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{sample_project, sample_session, test_engine};
    use crate::statusline::StatusTone;

    fn session_ids_for(engine: &Engine, project_id: &str) -> Vec<String> {
        engine
            .sessions
            .iter()
            .filter(|s| s.project_id == project_id)
            .map(|s| s.id.clone())
            .collect()
    }

    #[test]
    fn reorder_sessions_happy_path_reorders_vec_and_persists() {
        let (mut engine, _tmp) = test_engine();
        for id in ["a", "b", "c"] {
            let session = sample_session(id, "p1", id);
            engine.session_store.upsert_session(&session).unwrap();
            engine.sessions.push(session);
        }

        let reaction = engine
            .apply(Command::ReorderSessions {
                project_id: "p1".to_string(),
                session_ids: vec!["c".into(), "a".into(), "b".into()],
            })
            .expect("apply");
        assert!(matches!(reaction, EventReaction::Nothing));

        // In-memory Vec follows the new order.
        assert_eq!(session_ids_for(&engine, "p1"), vec!["c", "a", "b"]);
        // Persisted: reload from the store reflects the same order.
        let reloaded: Vec<String> = engine
            .session_store
            .load_sessions()
            .unwrap()
            .into_iter()
            .filter(|s| s.project_id == "p1")
            .map(|s| s.id)
            .collect();
        assert_eq!(reloaded, vec!["c", "a", "b"]);
    }

    #[test]
    fn reorder_sessions_keeps_other_projects_relative_order() {
        let (mut engine, _tmp) = test_engine();
        // Interleave two projects in the Vec: p1-a, p2-x, p1-b, p2-y.
        for (id, proj) in [("a", "p1"), ("x", "p2"), ("b", "p1"), ("y", "p2")] {
            let session = sample_session(id, proj, id);
            engine.session_store.upsert_session(&session).unwrap();
            engine.sessions.push(session);
        }

        engine
            .apply(Command::ReorderSessions {
                project_id: "p1".to_string(),
                session_ids: vec!["b".into(), "a".into()],
            })
            .expect("apply");

        // p1 reordered to b, a.
        assert_eq!(session_ids_for(&engine, "p1"), vec!["b", "a"]);
        // p2's relative order (x before y) is untouched.
        assert_eq!(session_ids_for(&engine, "p2"), vec!["x", "y"]);
    }

    #[test]
    fn reorder_sessions_rejects_missing_id() {
        let (mut engine, _tmp) = test_engine();
        for id in ["a", "b"] {
            engine.sessions.push(sample_session(id, "p1", id));
        }
        let err = engine
            .apply(Command::ReorderSessions {
                project_id: "p1".to_string(),
                session_ids: vec!["a".into()], // missing "b"
            })
            .map(|_| ())
            .expect_err("missing id must error");
        assert!(
            err.to_string().contains("exactly the current"),
            "err: {err}"
        );
    }

    #[test]
    fn reorder_sessions_rejects_extra_id() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("a", "p1", "a"));
        let err = engine
            .apply(Command::ReorderSessions {
                project_id: "p1".to_string(),
                session_ids: vec!["a".into(), "ghost".into()],
            })
            .map(|_| ())
            .expect_err("extra id must error");
        assert!(
            err.to_string().contains("exactly the current"),
            "err: {err}"
        );
    }

    #[test]
    fn reorder_sessions_rejects_duplicate_id() {
        let (mut engine, _tmp) = test_engine();
        for id in ["a", "b"] {
            engine.sessions.push(sample_session(id, "p1", id));
        }
        let err = engine
            .apply(Command::ReorderSessions {
                project_id: "p1".to_string(),
                session_ids: vec!["a".into(), "a".into()],
            })
            .map(|_| ())
            .expect_err("duplicate id must error");
        assert!(err.to_string().contains("duplicate"), "err: {err}");
    }

    #[test]
    fn reorder_sessions_rejects_foreign_id() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("a", "p1", "a"));
        engine.sessions.push(sample_session("b", "p2", "b"));
        // "b" belongs to p2, not p1; reordering p1 with it is rejected.
        let err = engine
            .apply(Command::ReorderSessions {
                project_id: "p1".to_string(),
                session_ids: vec!["a".into(), "b".into()],
            })
            .map(|_| ())
            .expect_err("foreign id must error");
        assert!(
            err.to_string().contains("exactly the current"),
            "err: {err}"
        );
    }

    #[test]
    fn reorder_projects_happy_path_reorders_and_persists() {
        let (mut engine, _tmp) = test_engine();
        for id in ["a", "b", "c"] {
            engine
                .projects
                .push(sample_project(id, &format!("/repo/{id}")));
            engine
                .session_store
                .upsert_project(&crate::config::ProjectConfig {
                    id: id.to_string(),
                    path: format!("/repo/{id}"),
                    name: Some(id.to_string()),
                    default_provider: None,
                    leading_branch: Some("main".to_string()),
                    auto_reopen_agents: None,
                    startup_command: None,
                    env: Default::default(),
                })
                .unwrap();
        }

        let reaction = engine
            .apply(Command::ReorderProjects {
                project_ids: vec!["c".into(), "a".into(), "b".into()],
            })
            .expect("apply");
        assert!(matches!(reaction, EventReaction::Nothing));

        let ids: Vec<String> = engine.projects.iter().map(|p| p.id.clone()).collect();
        assert_eq!(ids, vec!["c", "a", "b"]);
        let reloaded: Vec<String> = engine
            .session_store
            .load_projects()
            .unwrap()
            .into_iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(reloaded, vec!["c", "a", "b"]);
    }

    #[test]
    fn reorder_projects_rejects_wrong_set() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("a", "/repo/a"));
        engine.projects.push(sample_project("b", "/repo/b"));

        // Missing one.
        let err = engine
            .apply(Command::ReorderProjects {
                project_ids: vec!["a".into()],
            })
            .map(|_| ())
            .expect_err("missing id must error");
        assert!(
            err.to_string().contains("exactly the current"),
            "err: {err}"
        );

        // Duplicate.
        let err = engine
            .apply(Command::ReorderProjects {
                project_ids: vec!["a".into(), "a".into()],
            })
            .map(|_| ())
            .expect_err("duplicate id must error");
        assert!(err.to_string().contains("duplicate"), "err: {err}");
    }

    #[test]
    fn persist_session_order_writes_current_vec_order() {
        let (mut engine, _tmp) = test_engine();
        for id in ["a", "b", "c"] {
            let session = sample_session(id, "p1", id);
            engine.session_store.upsert_session(&session).unwrap();
            engine.sessions.push(session);
        }
        // Manually reorder the in-memory Vec (as a TUI sort action would), then
        // persist. A reload must reflect the same order.
        engine.sessions.reverse(); // c, b, a
        engine.persist_session_order().expect("persist");

        let reloaded: Vec<String> = engine
            .session_store
            .load_sessions()
            .unwrap()
            .into_iter()
            .filter(|s| s.project_id == "p1")
            .map(|s| s.id)
            .collect();
        assert_eq!(reloaded, vec!["c", "b", "a"]);
    }

    /// Init a temp git repo with a committed `a.txt` so discard tests can both
    /// restore a tracked file and create an untracked one.
    fn discard_test_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let run = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .status()
                .expect("spawn git")
                .success();
            assert!(ok, "git {args:?} failed");
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(dir.path().join("a.txt"), "original\n").expect("write file");
        run(&["add", "a.txt"]);
        run(&["commit", "-q", "-m", "init"]);
        dir
    }

    #[test]
    fn discard_file_restores_tracked_file_from_head() {
        let repo = discard_test_repo();
        let file = repo.path().join("a.txt");
        std::fs::write(&file, "modified\n").expect("modify");
        let (mut engine, _tmp) = test_engine();

        let reaction = engine
            .apply(Command::DiscardFile {
                worktree_path: repo.path().to_path_buf(),
                path: "a.txt".to_string(),
                is_untracked: false,
            })
            .expect("apply");

        match reaction {
            EventReaction::Status(update) => {
                assert_eq!(update.tone, StatusTone::Info);
                assert!(
                    update
                        .message
                        .contains("Discarded unstaged changes to \"a.txt\"")
                );
                assert!(update.message.contains("staged changes, if any, are kept"));
            }
            _ => panic!("expected Info status reaction"),
        }
        // The working copy is back to the committed content.
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "original\n");
    }

    #[test]
    fn discard_file_deletes_untracked_file() {
        let repo = discard_test_repo();
        let untracked = repo.path().join("new.txt");
        std::fs::write(&untracked, "scratch\n").expect("write untracked");
        let (mut engine, _tmp) = test_engine();

        let reaction = engine
            .apply(Command::DiscardFile {
                worktree_path: repo.path().to_path_buf(),
                path: "new.txt".to_string(),
                is_untracked: true,
            })
            .expect("apply");

        match reaction {
            EventReaction::Status(update) => {
                assert_eq!(update.tone, StatusTone::Info);
                assert!(
                    update
                        .message
                        .contains("Deleted untracked file \"new.txt\"")
                );
            }
            _ => panic!("expected Info status reaction"),
        }
        assert!(!untracked.exists(), "untracked file should be deleted");
    }

    // ── Command::RunMacro ───────────────────────────────────────────────────

    use crate::config::{MacroEntry, MacroSurface};
    use crate::pty::PtyClient;

    /// Spawn a `cat -v` PTY in raw mode (control bytes render as caret notation:
    /// ESC → `^[`, CR → `^M`), the same fixture the TUI macro-bar PTY test uses,
    /// so a RunMacro write can be observed in the terminal snapshot.
    fn spawn_cat_v_pty() -> PtyClient {
        let client = PtyClient::spawn(
            "sh",
            &["-c".to_string(), "stty raw -echo; exec cat -v".to_string()],
            std::path::Path::new("."),
            5,
            80,
            100,
        )
        .expect("spawn pty");
        std::thread::sleep(std::time::Duration::from_millis(250));
        client
    }

    fn rendered_snapshot(client: &PtyClient) -> String {
        client
            .snapshot()
            .cells
            .iter()
            .map(|cell| cell.symbol.as_str())
            .collect()
    }

    fn insert_macro(engine: &mut Engine, name: &str, text: &str, surface: MacroSurface) {
        engine.config.macros.entries.insert(
            name.to_string(),
            MacroEntry {
                text: text.to_string(),
                surface,
            },
        );
    }

    #[test]
    fn run_macro_writes_to_agent_provider_pty() {
        let (mut engine, _tmp) = test_engine();
        engine
            .providers
            .insert("sess-1".to_string(), spawn_cat_v_pty());
        insert_macro(&mut engine, "greet", "first\nsecond", MacroSurface::Agent);

        let reaction = engine
            .apply(Command::RunMacro {
                target_id: "sess-1".to_string(),
                name: "greet".to_string(),
            })
            .expect("apply");
        match reaction {
            EventReaction::Status(update) => {
                assert_eq!(update.tone, StatusTone::Info);
                assert_eq!(update.message, "Sent macro \"greet\".");
            }
            _ => panic!("expected Info status reaction"),
        }

        std::thread::sleep(std::time::Duration::from_millis(300));
        let rendered = rendered_snapshot(engine.providers.get("sess-1").unwrap());
        assert!(
            rendered.contains("first") && rendered.contains("second"),
            "both halves should be visible; got: {rendered:?}"
        );
        // Newline translated to ESC+CR (Alt+Enter) by the shared core transform.
        assert!(
            rendered.contains("^[") && rendered.contains("^M"),
            "newline should become ESC+CR; got: {rendered:?}"
        );
    }

    #[test]
    fn run_macro_writes_to_companion_terminal_pty() {
        use crate::model::CompanionTerminal;
        let (mut engine, _tmp) = test_engine();
        engine.companion_terminals.insert(
            "term-1".to_string(),
            CompanionTerminal {
                session_id: "sess-1".to_string(),
                label: "term".to_string(),
                foreground_cmd: None,
                client: spawn_cat_v_pty(),
            },
        );
        insert_macro(&mut engine, "ls", "ls -la", MacroSurface::Terminal);

        let reaction = engine
            .apply(Command::RunMacro {
                target_id: "term-1".to_string(),
                name: "ls".to_string(),
            })
            .expect("apply");
        match reaction {
            EventReaction::Status(update) => {
                assert_eq!(update.tone, StatusTone::Info);
                assert_eq!(update.message, "Sent macro \"ls\".");
            }
            _ => panic!("expected Info status reaction"),
        }

        std::thread::sleep(std::time::Duration::from_millis(300));
        let rendered = rendered_snapshot(&engine.companion_terminals.get("term-1").unwrap().client);
        assert!(
            rendered.contains("ls -la"),
            "macro text should reach the terminal PTY; got: {rendered:?}"
        );
    }

    #[test]
    fn run_macro_unknown_name_errors() {
        let (mut engine, _tmp) = test_engine();
        engine
            .providers
            .insert("sess-1".to_string(), spawn_cat_v_pty());

        let reaction = engine
            .apply(Command::RunMacro {
                target_id: "sess-1".to_string(),
                name: "nope".to_string(),
            })
            .expect("apply");
        match reaction {
            EventReaction::Status(update) => {
                assert_eq!(update.tone, StatusTone::Error);
                assert!(
                    update.message.contains("Macro \"nope\" does not exist"),
                    "got: {}",
                    update.message
                );
            }
            _ => panic!("expected Error status reaction"),
        }
    }

    #[test]
    fn run_macro_wrong_surface_errors() {
        let (mut engine, _tmp) = test_engine();
        // Agent target, but the macro is terminal-only.
        engine
            .providers
            .insert("sess-1".to_string(), spawn_cat_v_pty());
        insert_macro(&mut engine, "ls", "ls -la", MacroSurface::Terminal);

        let reaction = engine
            .apply(Command::RunMacro {
                target_id: "sess-1".to_string(),
                name: "ls".to_string(),
            })
            .expect("apply");
        match reaction {
            EventReaction::Status(update) => {
                assert_eq!(update.tone, StatusTone::Error);
                assert!(
                    update.message.contains("not available on agent targets"),
                    "got: {}",
                    update.message
                );
            }
            _ => panic!("expected Error status reaction"),
        }
    }

    #[test]
    fn run_macro_unknown_target_errors() {
        let (mut engine, _tmp) = test_engine();
        insert_macro(&mut engine, "greet", "hi", MacroSurface::Both);

        let reaction = engine
            .apply(Command::RunMacro {
                target_id: "ghost".to_string(),
                name: "greet".to_string(),
            })
            .expect("apply");
        match reaction {
            EventReaction::Status(update) => {
                assert_eq!(update.tone, StatusTone::Error);
                assert!(
                    update.message.contains("No live agent or terminal"),
                    "got: {}",
                    update.message
                );
            }
            _ => panic!("expected Error status reaction"),
        }
    }

    // ── Command::UpdateMacros ───────────────────────────────────────────────

    #[test]
    fn update_macros_adopts_into_engine_config_and_writes_through_queue() {
        let (mut engine, _tmp) = test_engine();
        let mut macros = crate::config::MacrosConfig::default();
        macros.entries.insert(
            "greet".to_string(),
            MacroEntry {
                text: "hi".to_string(),
                surface: MacroSurface::Both,
            },
        );

        let reaction = engine
            .apply(Command::UpdateMacros {
                macros: macros.clone(),
            })
            .expect("apply");
        // Eager save reports synchronously.
        match reaction {
            EventReaction::Status(update) => {
                assert_eq!(update.tone, StatusTone::Info);
                assert!(
                    update.message.contains("Saved 1 macro"),
                    "got: {}",
                    update.message
                );
            }
            _ => panic!("expected Info status reaction"),
        }
        // The engine adopted the new macros immediately so the ViewModel refreshes.
        assert!(engine.config.macros.entries.contains_key("greet"));
        // The eager save lands on disk through the queue.
        engine.config_writer.flush();
        let written = std::fs::read_to_string(&engine.paths.config_path).expect("read back");
        assert!(written.contains("greet"), "macro persisted: {written}");
    }

    #[test]
    fn update_macros_keeps_macros_when_write_fails() {
        // Keep-and-report: a failed write means the on-disk file can't be updated,
        // but the new macros stay active for the session AND the existing config
        // file is left intact (the atomic temp-file-then-rename primitive never
        // truncates or partially-writes the real file on failure — F14).
        let (mut engine, _tmp) = test_engine();

        // Seed a known-good, parseable config on disk so we can prove a failed
        // write does not corrupt or truncate it.
        let original = "[defaults]\nprovider = \"claude\"\n\n[env]\nSEED = \"keep-me\"\n";
        std::fs::write(&engine.paths.config_path, original).expect("seed config");

        engine.config_writer = crate::config_queue::ConfigWriteQueue::with_dead_writer(
            engine.paths.config_path.clone(),
        );
        let mut macros = crate::config::MacrosConfig::default();
        macros.entries.insert(
            "greet".to_string(),
            MacroEntry {
                text: "hi".to_string(),
                surface: MacroSurface::Both,
            },
        );

        let reaction = engine
            .apply(Command::UpdateMacros { macros })
            .expect("apply");
        match reaction {
            EventReaction::Status(update) => {
                assert_eq!(update.tone, StatusTone::Error);
                assert!(
                    update.message.contains("Macros updated this session"),
                    "got: {}",
                    update.message
                );
            }
            _ => panic!("expected Error status reaction"),
        }
        // Despite the failed write, the macros are still active in memory.
        assert!(engine.config.macros.entries.contains_key("greet"));

        // The on-disk file is byte-for-byte the original — not truncated, emptied,
        // or partially overwritten — and still parses as valid config.
        let on_disk = std::fs::read_to_string(&engine.paths.config_path).expect("read back");
        assert_eq!(
            on_disk, original,
            "a failed write must leave config.toml untouched"
        );
        let parsed: crate::config::Config = toml::from_str(&on_disk).expect("config still valid");
        assert_eq!(parsed.env.get("SEED").map(String::as_str), Some("keep-me"));
        assert!(
            !parsed.macros.entries.contains_key("greet"),
            "the failed write must not have leaked the new macro to disk"
        );
    }

    // ── Engine::set_watched_session + spawn_changed_files_refresh +
    //    Command::WatchChangedFiles ─────────────────────────────────────────

    /// A temp git repo with one STAGED change (`staged.txt` added to the index)
    /// and one UNSTAGED change (`unstaged.txt` is a new, untracked file) so a
    /// `changed_files` read returns a non-empty entry in each list.
    fn watch_test_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let run = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .status()
                .expect("spawn git")
                .success();
            assert!(ok, "git {args:?} failed");
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        std::fs::write(dir.path().join("base.txt"), "base\n").expect("write base");
        run(&["add", "base.txt"]);
        run(&["commit", "-q", "-m", "init"]);
        // Staged: a new file added to the index.
        std::fs::write(dir.path().join("staged.txt"), "staged\n").expect("write staged");
        run(&["add", "staged.txt"]);
        // Unstaged: a new untracked file (never added).
        std::fs::write(dir.path().join("unstaged.txt"), "unstaged\n").expect("write unstaged");
        dir
    }

    fn session_in_repo(id: &str, repo: &std::path::Path) -> crate::model::AgentSession {
        let mut session = sample_session(id, "p1", id);
        session.worktree_path = repo.to_string_lossy().into_owned();
        session
    }

    fn sample_changed_file(path: &str) -> crate::model::ChangedFile {
        crate::model::ChangedFile {
            status: "M".to_string(),
            path: path.to_string(),
            additions: 1,
            deletions: 0,
            binary: false,
        }
    }

    /// Block on the engine's worker channel for the next `ChangedFilesReady`
    /// event (the one-shot refresh worker sends exactly one), then feed it back
    /// through `process_worker_event` so the engine adopts the computed lists —
    /// exactly what the actor loop / TUI drain does. Panics if no such event
    /// arrives within a few seconds.
    fn drain_changed_files_refresh(engine: &mut Engine) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let event = engine
                .worker_rx
                .recv_timeout(remaining)
                .expect("ChangedFilesReady within deadline");
            if matches!(event, WorkerEvent::ChangedFilesReady { .. }) {
                engine.process_worker_event(event);
                return;
            }
        }
    }

    #[test]
    fn set_watched_session_records_id_clears_lists_and_returns_path() {
        let repo = watch_test_repo();
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(session_in_repo("s1", repo.path()));
        // Seed stale lists so we can prove the cheap set EMPTIES them (the
        // watched_session_id invariant: never show the previous watch's files).
        engine.staged_files = vec![sample_changed_file("stale.txt")];
        engine.unstaged_files = vec![sample_changed_file("stale.txt")];

        let path = engine.set_watched_session(Some("s1"));

        // Cheap: no git ran, the lists are EMPTY, and the worktree is returned.
        assert_eq!(path.as_deref(), Some(repo.path()));
        assert_eq!(engine.watched_session_id.as_deref(), Some("s1"));
        assert!(engine.staged_files.is_empty());
        assert!(engine.unstaged_files.is_empty());
        // The poller's shared watched-worktree handle now points at the repo.
        assert_eq!(
            engine.watched_worktree.lock().unwrap().as_deref(),
            Some(repo.path())
        );
    }

    #[test]
    fn set_watched_session_none_clears_everything_and_returns_none() {
        let repo = watch_test_repo();
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(session_in_repo("s1", repo.path()));
        engine.staged_files = vec![sample_changed_file("a.txt")];

        let path = engine.set_watched_session(None);

        assert!(path.is_none());
        assert!(engine.watched_session_id.is_none());
        assert!(engine.staged_files.is_empty());
        assert!(engine.unstaged_files.is_empty());
        assert!(engine.watched_worktree.lock().unwrap().is_none());
    }

    #[test]
    fn set_watched_session_unknown_id_clears_and_returns_none() {
        let repo = watch_test_repo();
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(session_in_repo("s1", repo.path()));
        let _ = engine.set_watched_session(Some("s1"));
        engine.staged_files = vec![sample_changed_file("a.txt")];

        // A stale/unknown id must clear (so it never shows another session's files).
        let path = engine.set_watched_session(Some("ghost"));

        assert!(path.is_none());
        assert!(engine.watched_session_id.is_none());
        assert!(engine.staged_files.is_empty());
        assert!(engine.unstaged_files.is_empty());
        assert!(engine.watched_worktree.lock().unwrap().is_none());
    }

    #[test]
    fn spawn_changed_files_refresh_emits_ready_with_computed_lists() {
        let repo = watch_test_repo();
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(session_in_repo("s1", repo.path()));
        // Arm the watch (cheap) so the ChangedFilesReady drain accepts the event.
        let path = engine.set_watched_session(Some("s1")).expect("path");

        engine.spawn_changed_files_refresh(path);

        // The one-shot worker posts ChangedFilesReady tagged with the worktree.
        let event = engine
            .worker_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("ChangedFilesReady");
        match event {
            WorkerEvent::ChangedFilesReady {
                staged,
                unstaged,
                worktree,
            } => {
                assert_eq!(worktree.as_path(), repo.path());
                assert!(staged.iter().any(|f| f.path == "staged.txt"), "{staged:?}");
                assert!(
                    unstaged.iter().any(|f| f.path == "unstaged.txt"),
                    "{unstaged:?}"
                );
            }
            _ => panic!("expected ChangedFilesReady"),
        }
    }

    #[test]
    fn watch_command_arms_watch_and_queues_refresh_worker() {
        let repo = watch_test_repo();
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(session_in_repo("s1", repo.path()));

        let reaction = engine
            .apply(Command::WatchChangedFiles {
                session_id: Some("s1".to_string()),
            })
            .expect("apply");
        // The actor thread did only the cheap set + spawn: Nothing, watch armed,
        // lists EMPTY (population is now async via the worker, not inline).
        assert!(matches!(reaction, EventReaction::Nothing));
        assert_eq!(engine.watched_session_id.as_deref(), Some("s1"));
        assert!(engine.staged_files.is_empty());
        assert!(engine.unstaged_files.is_empty());

        // The off-thread worker computed and queued the lists; draining it (as
        // the actor loop would) populates the engine.
        drain_changed_files_refresh(&mut engine);
        assert!(engine.staged_files.iter().any(|f| f.path == "staged.txt"));
        assert!(
            engine
                .unstaged_files
                .iter()
                .any(|f| f.path == "unstaged.txt")
        );
    }

    #[test]
    fn view_model_changed_files_empty_before_watch_populated_after_drain() {
        let repo = watch_test_repo();
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(session_in_repo("s1", repo.path()));

        // Before the watch the ViewModel carries empty lists and no watched id —
        // exactly what the web saw (the pane showed nothing).
        let before = engine.view_model();
        assert!(before.changed_files.staged.is_empty());
        assert!(before.changed_files.unstaged.is_empty());
        assert!(before.changed_files.watched_session_id.is_none());

        // The command arms the watch (id visible immediately) but population is
        // now async: the ViewModel shows the watched id with EMPTY lists until
        // the worker event drains.
        engine
            .apply(Command::WatchChangedFiles {
                session_id: Some("s1".to_string()),
            })
            .expect("apply");
        let armed = engine.view_model();
        assert_eq!(
            armed.changed_files.watched_session_id.as_deref(),
            Some("s1")
        );
        assert!(armed.changed_files.staged.is_empty());
        assert!(armed.changed_files.unstaged.is_empty());

        // After the worker lands the lists are populated and still tagged.
        drain_changed_files_refresh(&mut engine);
        let after = engine.view_model();
        assert_eq!(
            after.changed_files.watched_session_id.as_deref(),
            Some("s1")
        );
        assert!(
            after
                .changed_files
                .staged
                .iter()
                .any(|f| f.path == "staged.txt")
        );
        assert!(
            after
                .changed_files
                .unstaged
                .iter()
                .any(|f| f.path == "unstaged.txt")
        );
    }
}
