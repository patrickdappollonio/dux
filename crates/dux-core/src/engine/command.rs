//! The `Command` enum — the §4.5 engine-operation vocabulary. Every
//! mutation or background-spawn the Engine performs in response to a
//! TUI key or a web-UI click is named here and dispatched through
//! `Engine::apply`.

use std::path::PathBuf;

use crate::engine::events::{
    BeginDeleteSessionView, DeleteTerminalView, DispatchAgentLaunchView, DoDeleteSessionView,
    EventReaction, FinishDeleteSessionView, StatusUpdate, WorktreeRemoval,
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
                        "Discarded changes to \"{path}\". File restored to last committed state."
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

            Command::GenerateCommitMessage { session_id } => {
                let Some(session) = self.sessions.iter().find(|s| s.id == session_id).cloned()
                else {
                    return Ok(EventReaction::Status(StatusUpdate::error(
                        "Unknown session.",
                    )));
                };
                let worktree = PathBuf::from(&session.worktree_path);
                let diff_text = match crate::git::staged_diff_text(&worktree) {
                    Ok(d) if d.trim().is_empty() => {
                        return Ok(EventReaction::Status(StatusUpdate::error(
                            "No staged changes to summarize. Stage files first.",
                        )));
                    }
                    Ok(d) => d,
                    Err(e) => {
                        return Ok(EventReaction::Status(StatusUpdate::error(format!(
                            "Failed to read the staged diff: {e}"
                        ))));
                    }
                };
                let prompt = format!("{}\n\n{}", self.config.default_commit_prompt(), diff_text);
                let cfg = crate::config::provider_config(&self.config, &session.provider);
                let prov = crate::provider::create_provider(session.provider.as_str(), cfg);
                let tx = self.worker_tx.clone();
                std::thread::spawn(move || match prov.run_oneshot(&prompt, &worktree) {
                    Ok(msg) => {
                        let _ = tx.send(crate::worker::WorkerEvent::CommitMessageGenerated(msg));
                    }
                    Err(e) => {
                        let _ = tx.send(crate::worker::WorkerEvent::CommitMessageFailed(
                            e.to_string(),
                        ));
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
        }
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
                assert!(update.message.contains("Discarded changes to \"a.txt\""));
                assert!(update.message.contains("restored to last committed state"));
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
}
