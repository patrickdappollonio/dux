//! The `Command` enum, the engine-operation vocabulary. Every mutation or
//! background spawn the Engine performs is named here and dispatched through
//! `Engine::apply`.

use std::path::PathBuf;

use crate::git::FileTracking;

use crate::engine::events::{
    BeginDeleteSessionView, DeleteTerminalView, DispatchAgentLaunchView, EventReaction,
    FinishDeleteSessionView, ProjectPersistenceOutcome, ProjectPersistenceView, StatusUpdate,
    WorktreeRemoval,
};
use crate::engine::{CommandWorkerSpec, Engine, InFlightKey};
use crate::model::Project;
use crate::text::count_of;
use crate::worker::{
    AgentLaunchFailedData, AgentLaunchRequest, CreateAgentRequest, ProjectPersistenceAction,
    PullOutcome, PullTarget, WorkerEvent,
};

/// What the Engine should do. Variants carry their payload: the caller computes
/// the context and supplies it, the Engine does the domain work and returns an
/// `EventReaction` describing any view follow-up.
#[derive(Debug)]
pub enum Command {
    /// Complete a deletion that's already past its git step: the async
    /// `WorktreeRemoveCompleted` callback.
    FinishDeleteSession {
        /// The agent being deleted.
        session_id: String,
        /// What happened to its worktree, for the status the delete reports.
        removal: WorktreeRemoval,
        /// Whether the finish reports a status; `false` when the caller keys
        /// its own.
        update_status: bool,
    },
    /// Modal entrypoint: branches between async git-removal worker and
    /// inline finish.
    BeginDeleteSession {
        /// The agent to delete.
        session_id: String,
        /// Whether its worktree goes too.
        delete_worktree: bool,
        /// The delete dialog's "also delete the branch" answer. See
        /// [`crate::model::BranchProvenance::resolve_branch_deletion`].
        delete_branch: Option<bool>,
    },
    /// Persist a project mutation. `Add` is handled inline: the handler writes
    /// SQLite and config.toml in `apply` and returns
    /// `EventReaction::ProjectPersistenceOutcome(Added)`, or an error-toned
    /// `EventReaction::Status` once the add has rolled back. Every other action
    /// goes through the background worker, whose
    /// `WorkerEvent::ProjectPersistenceCompleted` surfaces as
    /// `EventReaction::ProjectPersistenceOutcome` on a later drain.
    ///
    /// Boxed to keep the enum size within the clippy `large_enum_variant`
    /// threshold.
    ///
    /// `status_op_id` correlates a [`crate::engine::HandlerStatusOp`] whose final
    /// is decided in the completion handler, because the post-worker config
    /// write is fallible and produces a third outcome the worker never sees. It
    /// rides from the dispatch site through the worker and back on
    /// `ProjectPersistenceOutcome`. `None` for callers that drive no
    /// handler-resolved status.
    PersistProject {
        /// The change to save.
        action: Box<ProjectPersistenceAction>,
        /// The op whose final the completion handler resolves, if any.
        status_op_id: Option<String>,
    },

    /// Clone `url` into `path` as a new project on the remote's default branch,
    /// then start an agent named `agent_name` there (a random name when it is
    /// blank; `random_name` is the New agent dialog's checkbox). Returns the clone's busy, or an error
    /// status saying which check refused it; see [`crate::clone_project`].
    /// With `pull_request` the agent is created from that pull request, the
    /// address is built by the engine and `url` is ignored. With
    /// `add_existing` too, `path` is a folder that already holds a checkout of
    /// the repository, which becomes the project instead of a clone.
    CloneProject {
        /// The address to clone from.
        url: String,
        /// The folder to clone into, or to add with `add_existing`.
        path: String,
        /// The agent's name, or `None` for a random one.
        agent_name: Option<String>,
        /// Whether the New agent dialog asked for a random name.
        random_name: bool,
        /// The pull request the agent is created from, if any.
        pull_request: Option<crate::pr_reference::PullRequestCloneTarget>,
        /// Add the existing checkout at `path` instead of cloning.
        add_existing: bool,
        /// With `pull_request`: the agreement to a review copy given before
        /// the clone or the add.
        review_copy: crate::clone_project::UpfrontReviewCopy,
    },

    /// Remove a project and cascade-delete its agents' records and runtime,
    /// keeping their worktrees on disk. Tolerates a ghost project id that exists
    /// only through orphaned sessions: those are cleared and the project-record
    /// delete is a no-op. The record and config removal run synchronously on
    /// the engine thread, before the status is returned.
    RemoveProject {
        /// The project to remove.
        project_id: String,
        /// Its display name, for the status.
        project_name: String,
    },

    /// Delete a project and cascade-delete its agents' records and runtime,
    /// removing their worktrees from disk. The destructive counterpart to
    /// `RemoveProject`, which keeps them. Each session goes through the shared
    /// `begin_delete_session` pipeline with `delete_worktree == true`, so the kill,
    /// worktree removal and record cleanup stay single-source. Guards the whole
    /// project up front against an in-flight worktree removal or a launching
    /// tab, so a partial delete cannot report success while stranding a session.
    DeleteProject {
        /// The project to delete.
        project_id: String,
        /// Its display name, for the status.
        project_name: String,
    },

    /// Spawn the create-agent worker. Returns an error status if another create
    /// is already in flight; otherwise marks `InFlightKey::CreateAgent`, spawns
    /// the worker and returns the busy status. `term_size` comes from the caller
    /// because `crossterm::terminal::size()` is binary-only.
    ///
    /// Boxed to keep the enum size within the clippy `large_enum_variant`
    /// threshold.
    DispatchCreateAgentRequest {
        /// What to create.
        request: Box<CreateAgentRequest>,
        /// The busy shown while the create runs.
        busy_message: crate::status_text::StatusText,
        /// The caller's terminal size, as columns and rows.
        term_size: (u16, u16),
    },

    /// Spawn the agent-launch worker. Returns a typed view carrying
    /// `launched: bool` so callers can do their per-site post-action; an
    /// already-in-flight launch carries `launched: false` and an info status.
    ///
    /// Boxed to keep the enum size within the clippy `large_enum_variant`
    /// threshold.
    DispatchAgentLaunch {
        /// The launch to run.
        request: Box<AgentLaunchRequest>,
    },

    /// Stage a single file. A synchronous git call on the calling thread.
    /// Returns `EventReaction::Nothing` on success; an `Err` propagates to the
    /// App caller which surfaces it.
    StageFile {
        /// The worktree the file is in.
        worktree_path: PathBuf,
        /// The file, relative to the worktree.
        path: String,
    },

    /// Unstage a single file. Same shape as `StageFile`: synchronous git
    /// call, `EventReaction::Nothing` on success, `Err` on failure.
    UnstageFile {
        /// The worktree the file is in.
        worktree_path: PathBuf,
        /// The file, relative to the worktree.
        path: String,
    },

    /// Discard a single unstaged file's changes, on a worker, with a keyed
    /// busy and a final; `is_untracked` selects `git checkout -- <path>` or
    /// `rm`. Destructive: it permanently throws away working-tree changes, or
    /// deletes an untracked file outright. A refusal the engine can give at
    /// once (the folder being removed, something living at the path) is an
    /// `Err`.
    DiscardFile {
        /// The worktree the file is in.
        worktree_path: PathBuf,
        /// The file, relative to the worktree.
        path: String,
        /// Whether the file is untracked, so the discard deletes it.
        is_untracked: bool,
        /// What the row the user confirmed was: the discard is refused when
        /// the path is no longer that (a file a folder has replaced, a folder
        /// that became a repository), rather than deleting something else.
        confirmed: crate::git::ConfirmedEntry,
    },

    /// [`Command::DiscardFile`] for a file, classified as tracked or untracked
    /// against LIVE git status on the worker, right before the discard,
    /// rather than by the caller: the surface's thread never runs git for it.
    DiscardFileLive {
        /// The worktree the file is in.
        worktree_path: PathBuf,
        /// The file, relative to the worktree.
        path: String,
    },

    /// Commit the staged changes with `message`, on a worker: a keyed busy
    /// now, then the final. The worker first asks the shared preflight
    /// against live git status, so an empty message or nothing staged is the
    /// final's refusal ([`COMMIT_EMPTY_MESSAGE`], [`COMMIT_NOTHING_STAGED`])
    /// and nothing is committed. The caller pre-formats `success_message`
    /// because it depends on view-side bindings the engine cannot resolve.
    CommitChanges {
        /// The worktree to commit in.
        worktree_path: PathBuf,
        /// The commit message.
        message: String,
        /// The line reported when the commit lands.
        success_message: String,
    },

    /// Spawn a `git push` worker. Returns `EventReaction::Status(Busy("Pushing
    /// to remote\u{2026}"))` immediately; the worker's keyed final replaces it
    /// when the push ends.
    Push {
        /// The worktree whose branch is pushed.
        worktree_path: PathBuf,
    },

    /// Spawn a `git pull` worker for either a project's leading branch or the
    /// current session's branch. Returns the busy status when the in-flight
    /// guard accepts, or a warning when another pull is already running for the
    /// same repo path. Completion arrives as `WorkerEvent::PullCompleted`.
    Pull {
        /// The repository to pull.
        repo_path: PathBuf,
        /// What the pull is of.
        target: PullTarget,
        /// The busy shown while the pull runs.
        busy_message: crate::status_text::StatusText,
        /// The refusal while a pull of the same worktree runs. A SESSION pull's
        /// only: a project pull shares the project folder's lock with "Check
        /// out default branch" and "Change base branch", and its refusal is
        /// [`crate::engine::project_folder_busy_message`], naming whichever of
        /// the three is running.
        already_running_message: crate::status_text::StatusText,
    },

    /// Open a filesystem path via the user's OS handler, off the engine
    /// thread. Returns a keyed busy; the worker's keyed final replaces it with
    /// the outcome.
    OpenPath {
        /// The path to open.
        path: PathBuf,
        /// What the path is, for the status line ("session worktree", say).
        target: String,
    },

    /// Toggle a session's `auto_reopen_enabled` flag. The caller passes the new
    /// value and the branch name, captured before the mutation. The engine
    /// upserts in place, falling back to `session_store.set_auto_reopen_enabled`
    /// when the session was removed in flight.
    ToggleAgentAutoReopen {
        /// The agent changed.
        session_id: String,
        /// The agent's name as the caller showed it, for the status.
        branch_name: String,
        /// Whether the agent now reopens at startup.
        new_enabled: bool,
    },

    /// Remove a companion terminal from the engine: its child is sent SIGTERM
    /// and moved to the terminating set, which force-kills it after the grace.
    /// Returns a typed view carrying the terminal's label so the App can format
    /// status + reconcile `active_terminal_id` (view field).
    DeleteTerminal {
        /// The terminal to close.
        terminal_id: String,
    },

    /// Persist the global `env` block to the user config file. The engine
    /// eager-saves through `Engine::config_writer` and reports the result
    /// synchronously, rolling the in-memory env back if the write fails.
    PersistGlobalEnv {
        /// The new global environment.
        env: std::collections::BTreeMap<String, String>,
    },

    /// Reload the user config from disk, validate it, and resync project
    /// records against the session store. Opens a reload barrier, quiescing the
    /// config writer and deferring config-mutating commands, and starts the
    /// surface's reload worker; `WorkerEvent::ConfigReloadReady` closes it.
    ReloadConfig,

    /// Write a canonical, fully-templated config to disk, overwriting the
    /// existing file, to restore a known-good config after a failed reload.
    /// Renders through `Engine::surface` and writes synchronously through
    /// `config_write::replace_config_file`.
    RecoverConfig,

    /// Persist a global custom order for every agent. `session_ids` must be
    /// exactly the full set of all session ids, with no omissions, extras or
    /// duplicates, or the engine returns an actionable error and touches
    /// nothing. On success it writes one global
    /// `sort_order` permutation and re-sorts `self.sessions` to match. Silent:
    /// the refreshed view is the feedback.
    ReorderAgents {
        /// Every agent, in the new order.
        session_ids: Vec<String>,
    },

    /// Persist a custom display order for the workspace's projects.
    /// `project_ids` must be exactly the full set of known project ids, under
    /// the same strict validation as [`Command::ReorderAgents`]. On success it
    /// writes the order to storage and reorders `self.projects` to match.
    ReorderProjects {
        /// Every project, in the new order.
        project_ids: Vec<String>,
    },

    /// Persist a global runtime order for every companion terminal, whatever its
    /// owner. `terminal_ids` must be exactly the full set of current terminal
    /// ids; each terminal's runtime `sort_order` is stamped to match, with no
    /// storage write, because terminals are runtime-only. Silent success: the
    /// refreshed view is the feedback.
    ReorderTerminals {
        /// Every terminal, in the new order.
        terminal_ids: Vec<String>,
    },

    /// Wholesale-replace the `[macros]` config and persist it. Adopts `macros`
    /// into the running `config.macros` immediately, so the `ViewModel` refreshes
    /// without a reload, then eager-saves through `Engine::config_writer` and
    /// reports the result synchronously. Keep and report: a failed write leaves
    /// the new macros active for the session.
    ///
    /// Last write wins. The replacement is the editor's whole set, seeded from a
    /// pre-edit snapshot, so a save clobbers any hand-edit to the `[macros]`
    /// block made on disk in between. Acceptable under the single-operator
    /// model; a multi-writer setup would need read-modify-merge.
    UpdateMacros {
        /// The whole new macro list.
        macros: crate::config::MacrosConfig,
    },

    /// A change to the macro list or the global environment, kept as asked so a
    /// change a reload deferred runs against the config as it is when it runs.
    ChangeConfigSet(ConfigSetChange),

    /// Point the changed-files watch at a session's worktree, or clear it with
    /// `None`. Keeps git off the engine actor thread: resolving the session,
    /// setting `watched_worktree` and `watched_session_id` and emptying the
    /// lists are synchronous, then a one-shot worker computes the staged and
    /// unstaged lists and its `ChangedFilesReady` populates the pane on a later
    /// drain. Returns `EventReaction::Nothing`, because the refreshed `ViewModel`
    /// broadcast is the feedback rather than a toast on every selection.
    WatchChangedFiles {
        /// The agent whose worktree is watched, or `None` to stop watching.
        session_id: Option<String>,
    },
}

/// The commit's refusal when its message is empty.
pub const COMMIT_EMPTY_MESSAGE: &str = "Enter a commit message first.";

/// The commit's refusal when nothing is staged.
pub const COMMIT_NOTHING_STAGED: &str = "No staged changes to commit.";

impl Engine {
    /// One discard of a changes-pane row, on a worker. `untracked` is the
    /// caller's classification, or `None` to classify the row against live
    /// git status on the worker, right before acting.
    fn discard(
        &mut self,
        worktree_path: PathBuf,
        path: String,
        untracked: Option<bool>,
        confirmed: crate::git::ConfirmedEntry,
    ) -> anyhow::Result<EventReaction> {
        let _engine_thread = crate::engine::destructive_guard::engine_thread();
        let hold = self.hold_for_git_change(&worktree_path, "discard the change")?;
        let target = worktree_path.join(&path);
        // Classified on the worker, so the message is decided there too.
        let classify = {
            let worktree_path = worktree_path.clone();
            let path = path.clone();
            move || -> Result<FileTracking, String> {
                let untracked = match untracked {
                    Some(untracked) => untracked,
                    None => crate::git::discard_classify(&worktree_path, &path)
                        .map_err(|err| format!("{err:#}"))?,
                };
                Ok(if untracked {
                    FileTracking::Untracked
                } else {
                    FileTracking::Tracked
                })
            }
        };
        // A folder, a nested repository or an untracked link is removed
        // whole, so the discard follows the one destructive protocol: claim
        // it here (in memory, never waiting: a claim already held refuses it
        // at once), ask the occupancy question under the claim, and clear it
        // and delete on a worker. The engine thread never opens the session
        // database, reads the process table, or runs git for it.
        // A link at the path is removed whether it is untracked or stands
        // where a tracked file was (the restore replaces it).
        let destructive = !matches!(confirmed, crate::git::ConfirmedEntry::File)
            || crate::engine::is_symlink(&target);
        if !destructive {
            // A file restored or deleted as a file: nothing to clear.
            let op = crate::engine::status_op(crate::status_text![
                "Discarding the changes to ",
                q(path.clone()),
                "\u{2026}"
            ])
            .on_success(|message: &crate::status_text::StatusText| {
                crate::engine::Final::info(message.clone())
            })
            .on_failure(|error: &String| {
                crate::engine::Final::error(format!("Discard failed: {error}"))
            });
            return Ok(self.spawn_changes_op(op, move || {
                let _hold = hold;
                let tracking = classify()?;
                crate::git::discard_confirmed(
                    &worktree_path,
                    &path,
                    tracking,
                    Some(confirmed),
                    || {
                        Err(crate::destructive::Refused(format!(
                            "dux did not delete {}: it was confirmed as a file",
                            crate::home_path::shorten_home(&target)
                        )))
                    },
                )
                .map_err(|err| format!("{err:#}"))?;
                Ok(discard_message(&path, tracking, confirmed)(0))
            }));
        }
        let claim = self
            .worktree_ops()
            .claim_for_destructive_as(&target, std::time::Duration::ZERO, "a changes-pane delete")
            .map_err(|reason| {
                anyhow::Error::new(crate::destructive::Refused(format!(
                    "dux did not delete {}: {reason}.",
                    crate::home_path::shorten_home(&target)
                )))
            })?;
        let check = self.destructive_check(&target);
        if let Some(refused) = check.refused_now("delete") {
            return Err(anyhow::Error::new(refused));
        }
        let op = crate::engine::status_op(crate::status_text!["Deleting ", q(path), "\u{2026}"])
            .on_success(|message: &crate::status_text::StatusText| {
                crate::engine::Final::info(message.clone())
            })
            .on_failure(|error: &String| crate::engine::Final::error(error.clone()));
        Ok(self.spawn_changes_op(op, move || {
            let _hold = hold;
            let tracking = classify()?;
            // The discard refuses anything that is no longer what was
            // confirmed, so a success is the kind the user confirmed; the
            // number of files is what actually went.
            let went = crate::git::discard_confirmed(
                &worktree_path,
                &path,
                tracking,
                Some(confirmed),
                || check.clear(&[&claim], "delete"),
            )
            .map_err(|err| format!("{err:#}"))?;
            Ok(discard_message(&path, tracking, confirmed)(went))
        }))
    }
}

/// What a discard of `path` says once it is done, given how many files went.
fn discard_message(
    path: &str,
    tracking: FileTracking,
    confirmed: crate::git::ConfirmedEntry,
) -> impl Fn(usize) -> crate::status_text::StatusText + Send + 'static {
    let path = path.to_string();
    move |went| match confirmed {
        crate::git::ConfirmedEntry::Repository => crate::status_text![
            "Deleted ",
            q(format!("{path}/")),
            ", a repository of its own, with its history."
        ],
        crate::git::ConfirmedEntry::Folder { .. } => crate::status_text![
            "Deleted the untracked files in ",
            q(format!("{path}/")),
            format!(
                " ({}); files the repository ignores and repositories of their own inside it \
                 are kept.",
                crate::model::count_words(went, "file", "files")
            )
        ],
        crate::git::ConfirmedEntry::File if tracking == FileTracking::Untracked => {
            crate::status_text!["Deleted untracked file ", q(path.clone()), "."]
        }
        crate::git::ConfirmedEntry::File => crate::status_text![
            "Discarded unstaged changes to ",
            q(path.clone()),
            ". Staged changes, if any, are kept."
        ],
    }
}

/// What a client asked of the macro list or the global environment.
#[derive(Clone, Debug, PartialEq)]
pub enum ConfigSetChange {
    /// Replace the whole list, refused with [`crate::wire::StaleSet::Macros`] when
    /// it has moved since the [`crate::wire::macros_version`] in `read_at`.
    ReplaceMacros {
        /// The whole new list.
        macros: crate::config::MacrosConfig,
        /// The list version the client read, or `None` to replace whatever is
        /// there.
        read_at: Option<String>,
    },
    /// Add one macro, or replace it in place; every other macro stays as it is.
    SetMacro {
        /// The macro's name.
        name: String,
        /// What it types and where it is offered.
        entry: crate::config::MacroEntry,
    },
    /// Remove one macro; an unknown name is an error.
    RemoveMacro {
        /// The macro to remove.
        name: String,
    },
    /// Replace the whole table, with the same `read_at` rule.
    ReplaceEnv {
        /// The whole new table.
        env: std::collections::BTreeMap<String, String>,
        /// The table version the client read, or `None` to replace whatever is
        /// there.
        read_at: Option<String>,
    },
    /// Set one variable; every other stays as it is.
    SetEnvVar {
        /// The variable's name.
        name: String,
        /// Its value.
        value: String,
    },
    /// Remove one variable; an unknown name is an error.
    RemoveEnvVar {
        /// The variable to remove.
        name: String,
    },
}

/// The whole set a [`ConfigSetChange`] resolves to, saved by the handler of the
/// whole-set command it stands for.
enum ConfigSetSave {
    /// Saved as [`Command::UpdateMacros`] saves it.
    Macros(crate::config::MacrosConfig),
    /// Saved as [`Command::PersistGlobalEnv`] saves it.
    Env(std::collections::BTreeMap<String, String>),
}

impl Engine {
    /// Single dispatch point for every engine-affecting operation, whether the
    /// `Command` came from a key event or off the wire. Returns an
    /// `EventReaction` the caller routes through its view-applier.
    ///
    /// # Errors
    ///
    /// Returns an error when the command is refused, because an open change
    /// holds what it would change or somebody else is attached to what it would
    /// end, or when carrying it out fails; the error is the sentence the
    /// surfaces show.
    pub fn apply(&mut self, command: Command) -> anyhow::Result<EventReaction> {
        // Count every command taken, deferred ones included: the counter's
        // reader only asks whether anything happened this iteration. Counted at
        // the top so an early return cannot skip it.
        self.command_applies = self.command_applies.wrapping_add(1);
        // Refused while a change another surface follows holds what this one
        // would change. Checked only: a command has no record to hold with.
        self.check_command(&command)?;
        // Refused while somebody else is attached to what it would end, and held
        // against new attachments until the change ends.
        let reservation = self.guard_command(&command)?;
        let status_key = match &command {
            Command::PersistProject {
                status_op_id: Some(key),
                ..
            } => Some(key.clone()),
            // A launch reports back under its tab, whatever surface started it.
            Command::DispatchAgentLaunch { request } => Some(
                crate::operations::launch_binding_key(request.tab_id.as_str()),
            ),
            _ => None,
        };
        let applied = self.apply_admitted(command);
        if let Some(reservation) = reservation {
            let key = status_key.or_else(|| {
                applied
                    .as_ref()
                    .ok()
                    .and_then(crate::wire::wire_status_from_reaction)
                    .filter(|status| {
                        crate::statusline::StatusTone::from_wire(&status.tone)
                            == crate::statusline::StatusTone::Busy
                    })
                    .and_then(|status| status.key)
            });
            if let Some(key) = key {
                reservation.until_final(&key);
            }
        }
        applied
    }

    /// [`Self::apply`] once the command is admitted and guarded.
    #[expect(
        clippy::too_many_lines,
        reason = "one short arm per Command variant, each handing the variant's fields to the code that carries it out"
    )]
    fn apply_admitted(&mut self, command: Command) -> anyhow::Result<EventReaction> {
        // While a config reload barrier is open, hold any config-mutating
        // command until the reload lands, so it re-applies against the fresh
        // config instead of racing it.
        if self.reloading && Self::is_config_mutating(&command) {
            self.deferred_commands.push(command);
            self.deferred_policies.push(self.dispatch_policy.clone());
            self.deferred_operations
                .push(self.operation_in_dispatch.clone());
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
            Command::BeginDeleteSession {
                session_id,
                delete_worktree,
                delete_branch,
            } => {
                let worktree = if delete_worktree {
                    crate::engine::WorktreeOnDelete::Remove
                } else {
                    crate::engine::WorktreeOnDelete::Keep
                };
                let outcome = self.begin_delete_session(&session_id, worktree, delete_branch);
                Ok(EventReaction::BeginDeleteSessionView(Box::new(
                    BeginDeleteSessionView {
                        session_id,
                        outcome,
                    },
                )))
            }
            Command::PersistProject {
                action,
                status_op_id,
            } => self.apply_project_persistence(*action, status_op_id),

            Command::CloneProject {
                url,
                path,
                agent_name,
                random_name,
                pull_request,
                add_existing,
                review_copy,
            } => {
                let request = crate::clone_project::CloneRequest {
                    url,
                    path,
                    agent_name,
                    random_name,
                    pull_request,
                    add_existing,
                    review_copy,
                };
                Ok(EventReaction::Status(
                    match self
                        .begin_clone_project(&request, super::clone::CloneAskedBy::OtherSurface)
                    {
                        Ok(busy) => busy,
                        Err(refused) => StatusUpdate::error(refused),
                    },
                ))
            }

            Command::RemoveProject {
                project_id,
                project_name,
            } => self.remove_project_keep_worktrees(&project_id, &project_name),

            Command::DeleteProject {
                project_id,
                project_name,
            } => Ok(self.delete_project_with_worktrees(&project_id, &project_name)),

            Command::DispatchCreateAgentRequest {
                request,
                busy_message,
                term_size,
            } => Ok(self.apply_dispatch_create_agent(request, busy_message, term_size)),

            Command::DispatchAgentLaunch { request } => {
                Ok(self.apply_dispatch_agent_launch(request))
            }

            Command::StageFile {
                worktree_path,
                path,
            } => self.apply_stage_file(&worktree_path, &path),

            Command::UnstageFile {
                worktree_path,
                path,
            } => self.apply_unstage_file(&worktree_path, &path),

            Command::DiscardFileLive {
                worktree_path,
                path,
            } => self.discard(worktree_path, path, None, crate::git::ConfirmedEntry::File),

            Command::DiscardFile {
                worktree_path,
                path,
                is_untracked,
                confirmed,
            } => self.discard(worktree_path, path, Some(is_untracked), confirmed),

            Command::CommitChanges {
                worktree_path,
                message,
                success_message,
            } => Ok(self.apply_commit_changes(worktree_path, message, success_message)),

            Command::Push { worktree_path } => Ok(self.apply_push(&worktree_path)),

            Command::Pull {
                repo_path,
                target,
                busy_message,
                already_running_message,
            } => Ok(self.dispatch_pull(repo_path, target, busy_message, already_running_message)),

            Command::OpenPath { path, target } => Ok(self.apply_open_path(path, &target)),

            Command::ToggleAgentAutoReopen {
                session_id,
                branch_name,
                new_enabled,
            } => {
                let reopen = if new_enabled {
                    crate::model::AutoReopen::Enabled
                } else {
                    crate::model::AutoReopen::Disabled
                };
                self.apply_toggle_agent_auto_reopen(&session_id, &branch_name, reopen)
            }

            Command::DeleteTerminal { terminal_id } => Ok(self.apply_delete_terminal(terminal_id)),

            Command::PersistGlobalEnv { env } => Ok(self.apply_persist_global_env(env)),

            Command::ReloadConfig => Ok(self.apply_reload_config()),

            Command::RecoverConfig => Ok(self.apply_recover_config()),

            Command::ReorderAgents { session_ids } => self
                .reorder_agents(&session_ids)
                .map(|()| EventReaction::Nothing),

            Command::ReorderProjects { project_ids } => self
                .reorder_projects(&project_ids)
                .map(|()| EventReaction::Nothing),

            Command::ReorderTerminals { terminal_ids } => self
                .reorder_terminals(&terminal_ids)
                .map(|()| EventReaction::Nothing),

            Command::ChangeConfigSet(change) => self.apply_config_set_change(change),

            Command::UpdateMacros { macros } => Ok(self.apply_update_macros(macros)),

            Command::WatchChangedFiles { session_id } => {
                // Resolving and setting the watch runs no git, so it is cheap on
                // the actor thread; the changed files are computed off-thread.
                // `ChangedFilesReady` path-checks the worktree, so a watch that
                // moved meanwhile drops the stale result.
                if let Some(worktree) = self.set_watched_session(session_id.as_deref()) {
                    self.spawn_changed_files_refresh(worktree);
                }
                Ok(EventReaction::Nothing)
            }
        }
    }

    /// [`Command::ToggleAgentAutoReopen`]: persist the agent's auto-reopen
    /// choice first, so a failed write leaves memory showing the still-true
    /// value, then mirror it in memory.
    fn apply_toggle_agent_auto_reopen(
        &mut self,
        session_id: &str,
        branch_name: &str,
        reopen: crate::model::AutoReopen,
    ) -> anyhow::Result<EventReaction> {
        let enabled = reopen == crate::model::AutoReopen::Enabled;
        if let Some(current) = self.sessions.iter_mut().find(|c| c.id == session_id) {
            // Persist first so a DB failure leaves in-memory state
            // untouched and the UI keeps showing the still-true value.
            let mut candidate = current.clone();
            candidate.auto_reopen_enabled = enabled;
            candidate.updated_at = chrono::Utc::now();
            self.session_store.upsert_session(&candidate)?;
            *current = candidate;
        } else {
            self.session_store
                .set_auto_reopen_enabled(session_id, reopen)?;
        }
        Ok(EventReaction::Status(StatusUpdate::info(
            crate::status_text![
                format!(
                    "Startup auto-reopen {} for agent ",
                    if enabled { "enabled" } else { "disabled" }
                ),
                q(branch_name),
                "."
            ],
        )))
    }

    /// [`Command::StageFile`]: stage one file, holding the worktree while git
    /// runs.
    fn apply_stage_file(
        &self,
        worktree_path: &std::path::Path,
        path: &str,
    ) -> anyhow::Result<EventReaction> {
        let _hold = self.hold_for_git_change(worktree_path, "stage the file")?;
        crate::git::stage_file(worktree_path, path)?;
        Ok(EventReaction::Nothing)
    }

    /// [`Command::UnstageFile`]: unstage one file, holding the worktree while
    /// git runs.
    fn apply_unstage_file(
        &self,
        worktree_path: &std::path::Path,
        path: &str,
    ) -> anyhow::Result<EventReaction> {
        let _hold = self.hold_for_git_change(worktree_path, "unstage the file")?;
        crate::git::unstage_file(worktree_path, path)?;
        Ok(EventReaction::Nothing)
    }

    /// [`Command::DeleteTerminal`]: close a terminal gracefully. It is sent
    /// SIGTERM and moved to the terminating set, so the reaper force-kills it
    /// after the grace rather than it being dropped here for an immediate
    /// SIGKILL; the terminal leaves the UI now and the child winds down behind
    /// it.
    fn apply_delete_terminal(&mut self, terminal_id: String) -> EventReaction {
        let label = self.begin_close_companion_terminal(&terminal_id);
        EventReaction::DeleteTerminalView(Box::new(DeleteTerminalView { terminal_id, label }))
    }

    /// Why a create may not start, before anything of it is minted: another
    /// create is running, or its project is being deleted.
    fn create_agent_refusal(&self, request: &CreateAgentRequest) -> Option<StatusUpdate> {
        // Pre-check the in-flight guard before minting the op, so a
        // rejected dispatch cannot leak an unresolved op into
        // `pending_create_ops`. `spawn_command_worker` applies the same
        // guard only after consuming the spec, by which point the op
        // would already exist.
        if self.is_in_flight(&InFlightKey::CreateAgent) {
            return Some(StatusUpdate::error(
                crate::operations::CREATE_IN_FLIGHT_REFUSAL,
            ));
        }
        // A project being deleted takes no new agents: the deletion
        // has already decided which agents it removes.
        let project_id = request.project_id()?;
        if !self.project_is_being_deleted(project_id) {
            return None;
        }
        let name = self
            .projects
            .iter()
            .find(|project| project.id == project_id)
            .map_or_else(|| project_id.to_string(), |project| project.name.clone());
        Some(StatusUpdate::error(crate::status_text![
            "Project ",
            q(name),
            " is being deleted, so dux did not create an agent in it."
        ]))
    }

    /// A pull request create tells a branch an agent holds from one anything
    /// else holds, so it is handed the project's agents as they are now.
    fn fill_pull_request_agents(&self, request: &mut CreateAgentRequest) {
        if let crate::worker::CreateAgentRequest::PullRequest {
            project, agents, ..
        } = request
        {
            *agents = self
                .sessions
                .iter()
                .filter(|session| session.project_id() == Some(project.id.as_str()))
                .filter_map(|session| {
                    Some(crate::worker::AgentAtWorktree {
                        name: session.display_label(),
                        worktree_path: PathBuf::from(session.managed_worktree()?),
                    })
                })
                .collect();
        }
    }

    /// [`Command::DispatchCreateAgentRequest`]: mint the create op and start the
    /// create worker, unless a create is already running.
    fn apply_dispatch_create_agent(
        &mut self,
        mut request: Box<CreateAgentRequest>,
        busy_message: crate::status_text::StatusText,
        term_size: (u16, u16),
    ) -> EventReaction {
        if let Some(refusal) = self.create_agent_refusal(&request) {
            return EventReaction::Status(refusal);
        }
        // An agent created on a folder that already exists (an
        // existing worktree it adopts, or the user's own folder for a
        // standalone agent) holds it from now until its launch lands,
        // so a removal of that folder waits for it (or keeps it), and a
        // folder already being removed is refused rather than adopted:
        // dux never removes a folder a standalone agent runs in. A
        // create that makes a NEW worktree holds its path from inside
        // the job, the moment the path is known and before
        // `git worktree add` runs.
        let existing_worktree = match request.as_ref() {
            crate::worker::CreateAgentRequest::ExistingManagedWorktree {
                worktree_path, ..
            } => Some(worktree_path.clone()),
            crate::worker::CreateAgentRequest::Standalone { folder, .. } => Some(folder.clone()),
            _ => None,
        };
        // A pull request create tells a branch an agent holds from one
        // anything else holds, so it needs the project's agents as they
        // are now. Every pull request create passes through here.
        self.fill_pull_request_agents(&mut request);
        // Mint the shared create-agent `HandlerStatusOp`: its opaque id
        // correlates the dispatch busy, every progress re-emit and the
        // final the launch-ready and launch-failed handlers resolve from
        // a `CreateLaunchOutcome`, so both surfaces word it identically.
        // A clone's first agent: a create that fails says what the
        // clone already did, which is on disk and in the workspace.
        let cloned = match request.as_ref() {
            crate::worker::CreateAgentRequest::NewProject { cloned, .. }
            | crate::worker::CreateAgentRequest::PullRequest { cloned, .. } => cloned.clone(),
            _ => None,
        };
        let op = crate::engine::status_op(busy_message)
            .resolve_in_handler(move |o: &crate::engine::CreateLaunchOutcome| {
                create_launch_final(o, cloned.as_ref())
            })
            // Stamp the originating connection so the busy, every progress
            // re-emit, and the eventual final reach only it (the followups
            // resolve in later ticks, after `current_origin` was reset).
            .with_scope(self.current_origin.clone());
        let op_id = op.id().to_string();
        // Checked and held in one step, under the registry's lock: a
        // folder being removed, deleted or moved refuses the create,
        // out loud, and nothing of it has started yet.
        if let Some(path) = &existing_worktree
            && let Err(refused) = self.worktree_ops().hold_as(
                crate::worktree_ops::HoldOwner::CreateOp(op_id.clone()),
                path,
                crate::worktree_ops::WorktreeOpKind::CreateAgent,
            )
        {
            return EventReaction::Status(StatusUpdate::error(
                refused.sentence("create an agent there"),
            ));
        }
        // Surface this create's op id to a synchronous `apply_wire`
        // caller, which reads `WireCommandOutcome.created_op_id` to
        // correlate its own new session through `created_session_for_op`.
        // Cleared at the top of every `apply_wire`, so it cannot leak
        // into an unrelated command's outcome.
        self.last_created_op_id = Some(op_id.clone());
        let op_id_for_job = op_id.clone();
        let op_id_panic = op_id.clone();
        let pending = op.pending_status();
        self.pending_create_ops.insert(op_id.clone(), op);
        self.note_create_started(&op_id, request.project_id());
        let paths = self.paths.clone();
        let config = self.config.clone();
        let identity = self.resolved_identity();
        let registry = crate::agent_job::CreateJobRegistries {
            processes: self.process_registry.clone(),
            worktrees: self.worktree_ops().clone(),
            moves: self.checkout_move_guard(),
        };
        let reaction = self.spawn_command_worker(
            CommandWorkerSpec {
                label: "create-agent".into(),
                in_flight_key: Some(InFlightKey::CreateAgent),
                busy_status: Some(pending),
                already_running_status: Some(StatusUpdate::error(
                    crate::operations::CREATE_IN_FLIGHT_REFUSAL,
                )),
                panic_event: Some(Box::new(move |reason| WorkerEvent::CreateAgentFailed {
                    status_op_id: op_id_panic,
                    attempted_name: None,
                    message: format!("Agent-creation worker panicked: {reason}").into(),
                })),
            },
            move |tx| {
                crate::agent_job::run_create_agent_job(
                    *request,
                    paths,
                    &config,
                    &tx,
                    term_size,
                    op_id_for_job,
                    identity,
                    registry,
                );
            },
        );
        // A worker that never started cleared the in-flight key; the
        // create's own bookkeeping goes with it.
        if self.is_in_flight(&InFlightKey::CreateAgent) {
            self.operations.set_create_guard(&op_id);
        } else {
            self.note_create_finished(&op_id);
        }
        reaction
    }

    /// Why a tab's launch may not start, logged where dux.log should show it:
    /// the agent is being deleted, its folder is being removed, a working-copy
    /// recreate is restarting the tab, or the tab is already launching or
    /// running.
    fn launch_refusal(&self, request: &AgentLaunchRequest) -> Option<StatusUpdate> {
        let branch_name = request.session.display_label();
        let session_id = request.session.id.as_str();
        let tab_id = &request.tab_id;
        // Guard the shared launch chokepoint against a session whose
        // worktree is mid-removal. Every launch path funnels through
        // this command, so checking once here covers them all, the
        // residual window included where a transient
        // `finish_delete_session` DB failure leaves the record in
        // `closing_sessions` past the synchronous delete.
        if self.closing_sessions.contains(session_id) {
            // Log the refusal so it shows up in dux.log even when a
            // caller ignores the returned view's `launched` flag, as
            // dux-web's `launch_agent` does.
            crate::logger::warn(&format!(
                "refused to launch tab \"{tab_id}\" for agent \"{branch_name}\": session \"{session_id}\" is being deleted"
            ));
            return Some(StatusUpdate::error(crate::status_text![
                "Agent ",
                q(branch_name),
                " is being deleted and cannot be launched."
            ]));
        }
        // Nothing new may start in a folder a removal has claimed,
        // whoever's it is: another agent sharing it, a standalone agent
        // in it. The removal's last look relies on this.
        if let Some(refused) = self
            .worktree_ops()
            .removal_refusal(request.session.directory())
        {
            let refusal = refused.sentence("start this agent there");
            crate::logger::warn(&format!(
                "refused to launch tab \"{tab_id}\" for agent \"{branch_name}\": its folder is being removed"
            ));
            return Some(StatusUpdate::error(refusal));
        }
        // A tab a working-copy recreate is about to start again is the
        // restart's to start: its old process is still shutting down,
        // and a launch now would race the restart for the conversation.
        if let Some(refusal) = self.restart_launch_refusal(tab_id) {
            crate::logger::info(&format!(
                "refused to launch tab \"{tab_id}\" for agent \"{branch_name}\": a working-copy recreate is restarting it"
            ));
            return Some(StatusUpdate::info(refusal));
        }
        // Pre-check in-flight so the View carries the exact "already
        // launching" message regardless of the primitive's generic
        // already-running fallback.
        if self.is_in_flight(&InFlightKey::AgentLaunch(tab_id.clone())) {
            return Some(StatusUpdate::info(crate::status_text![
                "Agent ",
                q(branch_name),
                " is already launching."
            ]));
        }
        // A live process already holds this tab, and launching over it
        // would replace the entry in `providers`, whose displaced client
        // SIGKILLs the child it names on drop. Every legitimate relaunch
        // path tears the runtime down before it dispatches, so reaching
        // here with a live provider means the launch is redundant.
        //
        // `PtyClient::is_live` is the right question because it excludes
        // both a child at end of input and one already reaped: each is
        // waiting to be pruned, and refusing for either would refuse the
        // relaunch of a tab that just died.
        if let Some(client) = self.providers.get(tab_id)
            && client.is_live()
        {
            let pid = client
                .child_process_id()
                .map_or_else(|| "unknown".to_string(), |pid| pid.to_string());
            crate::logger::warn(&format!(
                "refused to launch tab \"{tab_id}\" for agent \"{branch_name}\": \
                 process {pid} is already running in it"
            ));
            return Some(StatusUpdate::info(crate::status_text![
                "This tab of agent ",
                q(branch_name),
                " is already running. Stop it before starting it again."
            ]));
        }
        None
    }

    /// [`Command::DispatchAgentLaunch`]: start one tab's launch worker, refusing
    /// an agent that is being deleted.
    fn apply_dispatch_agent_launch(&mut self, request: Box<AgentLaunchRequest>) -> EventReaction {
        let session_id = request.session.id.clone();
        // The in-flight launch lock is keyed by tab id, one per tab, so
        // two tabs of one session can launch concurrently. The View
        // keeps `session_id` for UI correlation.
        let tab_id = request.tab_id.clone();
        // The View is a surface payload, so it carries the id as a plain
        // string; the engine-side decisions above keep the typed form.
        let tab_id_view = tab_id.as_str().to_string();
        if let Some(status) = self.launch_refusal(&request) {
            return EventReaction::DispatchAgentLaunchView(Box::new(DispatchAgentLaunchView {
                session_id,
                tab_id: tab_id_view,
                launched: false,
                status: Some(status),
            }));
        }
        // The same provider of the same agent still shutting down (a
        // tab closed or detached a moment ago, a restart): the new CLI
        // waits for it rather than sharing the worktree and the
        // conversation with it. Read from what each terminating entry
        // recorded when it started, never from the tab table.
        let predecessors = self.terminating_predecessors(&session_id, &request.provider);
        // An environment that cannot be resolved fails the launch the way a
        // spawn that cannot start does, so the tab keeps its card with the
        // reason rather than starting with no variables.
        let env_refusal = self.launch_env_refusal(&request.session);
        // Clone for the panic event closure before the job closure
        // consumes `request`, so panic recovery can take the same path
        // as `process_agent_launch_failed`.
        let panic_request = (*request).clone();
        let gate = self.spawn_gate();
        let reaction = self.spawn_command_worker(
            CommandWorkerSpec {
                label: format!("agent-launch:{tab_id}"),
                in_flight_key: Some(InFlightKey::AgentLaunch(tab_id.clone())),
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
                if let Some(message) = env_refusal {
                    let _ = tx.send(WorkerEvent::AgentLaunchFailed(Box::new(
                        AgentLaunchFailedData {
                            request: *request,
                            message,
                        },
                    )));
                    return;
                }
                if let Err(message) = wait_for_predecessors(&predecessors, &request, &tx) {
                    let _ = tx.send(WorkerEvent::AgentLaunchFailed(Box::new(
                        AgentLaunchFailedData {
                            request: *request,
                            message,
                        },
                    )));
                    return;
                }
                crate::agent_job::run_agent_launch_job(*request, &tx, &gate);
            },
        );
        // Wrap the primitive's return into the View variant so App
        // callers keep a single pattern-match shape.
        match reaction {
            EventReaction::Nothing => {
                // A launch is on its way, so the previous run's verdict
                // stops counting. Clearing it at the one chokepoint
                // every launch funnels through is what gets an explicit
                // start past the diagnosis card with no surface asking.
                // A refusal leaves the verdict standing: nothing ran.
                self.clear_tab_run_failure(&tab_id);
                EventReaction::DispatchAgentLaunchView(Box::new(DispatchAgentLaunchView {
                    session_id,
                    tab_id: tab_id_view.clone(),
                    launched: true,
                    status: None,
                }))
            }
            EventReaction::Status(status) => {
                EventReaction::DispatchAgentLaunchView(Box::new(DispatchAgentLaunchView {
                    session_id,
                    tab_id: tab_id_view,
                    launched: false,
                    status: Some(status),
                }))
            }
            other => other,
        }
    }

    /// [`Command::CommitChanges`]: commit the staged changes off the engine
    /// thread, holding the worktree for the commit's duration.
    fn apply_commit_changes(
        &mut self,
        worktree_path: PathBuf,
        message: String,
        success_message: String,
    ) -> EventReaction {
        // Held for the commit's own duration, so a removal of this
        // worktree that is already waiting sees it, and refused once a
        // removal has begun.
        let hold = match self
            .worktree_ops()
            .hold(&worktree_path, crate::worktree_ops::WorktreeOpKind::Commit)
        {
            Ok(hold) => hold,
            Err(refused) => {
                return EventReaction::Status(StatusUpdate::error(
                    refused.sentence("commit in it"),
                ));
            }
        };
        let op = crate::engine::status_op("Committing the staged changes\u{2026}")
            .on_success(move |(): &()| crate::engine::Final::info(success_message.clone()))
            // Commit failures leave the index and typed message intact, so
            // recovery stays inside the UI and does not require a sticky
            // status.
            .on_failure(|error: &String| crate::engine::Final::error(error.clone()));
        self.spawn_changes_op(op, move || {
            let _hold = hold;
            match crate::git::commit_preflight(&worktree_path, &message) {
                crate::git::CommitPreflight::EmptyMessage => {
                    return Err(COMMIT_EMPTY_MESSAGE.to_string());
                }
                crate::git::CommitPreflight::NothingStaged => {
                    return Err(COMMIT_NOTHING_STAGED.to_string());
                }
                crate::git::CommitPreflight::Ready => {}
            }
            crate::git::commit(&worktree_path, &message)
                .map(|_| ())
                .map_err(|e| format!("Commit failed: {e}"))
        })
    }

    /// [`Command::Push`]: push the worktree's branch off the engine thread.
    fn apply_push(&mut self, worktree_path: &std::path::Path) -> EventReaction {
        let op = crate::engine::status_op("Pushing to remote\u{2026}")
            .on_success(|(): &()| {
                crate::engine::Final::info(
                    "Pushed to remote successfully. Your changes are now available to collaborators.",
                )
            })
            .on_failure(|e: &String| {
                crate::engine::Final::error(format!("Push to remote failed: {e}"))
            });
        // The push holds its worktree until git is done with it, so a
        // removal that begins meanwhile waits for it, and a worktree
        // already being removed takes no push.
        let hold = match self
            .worktree_ops()
            .hold(worktree_path, crate::worktree_ops::WorktreeOpKind::Push)
        {
            Ok(hold) => hold,
            Err(refused) => {
                return EventReaction::Status(StatusUpdate::error(
                    refused.sentence("push from it"),
                ));
            }
        };
        let wt = worktree_path.to_path_buf();
        let timeout = self.config.git.network_timeout();
        self.spawn_status_op(op, move || {
            let _hold = hold;
            crate::git::push(&wt, timeout)
                .map(|_| ())
                .map_err(|e| format!("{e:#}"))
        })
    }

    /// [`Command::OpenPath`]: open a path with the system opener off the engine
    /// thread.
    fn apply_open_path(&mut self, path: PathBuf, target: &str) -> EventReaction {
        let display = path.display().to_string();
        let t_ok = target.to_string();
        let t_err = target.to_string();
        let op = crate::engine::status_op(crate::status_text![
            format!("Opening {}: ", target),
            n(display)
        ])
        .on_success(move |(): &()| crate::engine::Final::info(format!("Opened {t_ok}.")))
        .on_failure(move |e: &String| {
            crate::engine::Final::error(format!("Could not open {t_err}: {e}"))
        });
        self.spawn_status_op(op, move || {
            crate::startup::open_path(&path).map_err(|err| format!("{err:#}"))
        })
    }

    /// [`Command::PersistGlobalEnv`]: save the global environment, restoring the
    /// previous one when the write fails.
    fn apply_persist_global_env(
        &mut self,
        env: std::collections::BTreeMap<String, String>,
    ) -> EventReaction {
        // Eager save through the queue, with rollback: swap the new env
        // in, persist synchronously, and restore the previous env if the
        // write fails so in-memory state never diverges from disk.
        let previous = std::mem::replace(&mut self.config.env, env);
        if let Err(e) = self.config_writer.save_eager(self.config.clone()) {
            self.config.env = previous;
            return EventReaction::Status(StatusUpdate::error(format!(
                "Couldn't save global environment variables: {e}"
            )));
        }
        let msg = if self.config.env.is_empty() {
            "Global environment variables cleared.".to_string()
        } else {
            saved_global_env_message(self.config.env.len())
        };
        EventReaction::Status(StatusUpdate::info(msg))
    }

    /// [`Command::ReloadConfig`]: start a config reload, or queue one behind the
    /// reload already running.
    fn apply_reload_config(&mut self) -> EventReaction {
        // Reject a reentrant reload: a second one would drop the live
        // `reload_guard`, resuming the writer mid-reload, and spawn a
        // worker whose completion closes a barrier it never opened.
        // A reload asked for DURING one (a second `dux config set`'s
        // SIGUSR1, a second press) may be about a file that changed
        // after the running reload read it, so it is never dropped:
        // it joins the deferred commands and runs as soon as the
        // current reload closes its barrier. Any number of them
        // collapse into one follow-up, which reads the latest file.
        self.reload_origin.reload_asked(if self.reloading {
            crate::engine::reload_origin::ReloadSlot::AfterTheRunningOne
        } else {
            crate::engine::reload_origin::ReloadSlot::Now
        });
        if self.reloading {
            if !self
                .deferred_commands
                .iter()
                .any(|c| matches!(c, Command::ReloadConfig))
            {
                self.deferred_commands.push(Command::ReloadConfig);
                self.deferred_operations.push(None);
                self.deferred_policies.push(None);
            }
            // A client waiting on this one follows the reload that
            // reads the file after the one running.
            if let Some(id) = &self.operation_in_dispatch {
                self.operations
                    .await_reload(id, crate::engine::ReloadSlot::AfterTheRunningOne);
            }
            return EventReaction::Status(StatusUpdate::info(
                "A config reload is already running; another runs right after it, so \
                 the latest config.toml is the one that applies.",
            ));
        }
        // Open the reload barrier: quiesce the writer so no queued save
        // races the reload, mark `reloading` so config-mutating commands
        // defer, and start the surface's reload worker. The barrier
        // closes when `ConfigReloadReady` lands.
        //
        // An unacknowledged pause means no barrier: a still-running
        // writer could clobber the reload's config write, so abort and
        // let the caller retry.
        let guard = self.config_writer.quiesce();
        if !guard.is_acknowledged() {
            // The reload that was to start never will, so the clients
            // waiting on it are told so.
            self.operations.fail_reload(
                "Config writer is busy; please retry.",
                std::time::Instant::now(),
            );
            return EventReaction::Status(StatusUpdate::error(
                "Config writer is busy; please retry.",
            ));
        }
        self.reloading = true;
        self.reload_guard = Some(guard);
        if let Some(id) = &self.operation_in_dispatch {
            self.operations
                .await_reload(id, crate::engine::ReloadSlot::Now);
        }
        self.surface
            .reload(self.paths.clone(), self.worker_tx.clone());
        EventReaction::Nothing
    }

    /// [`Command::RecoverConfig`]: overwrite a corrupt config file with the
    /// running config, keeping the file's own `[server.auth]` when it reads.
    fn apply_recover_config(&mut self) -> EventReaction {
        // Reject recovery while a reload barrier is open: the reload
        // holds the writer quiesce, and a second quiesce would resume
        // the writer mid-reload when its guard dropped.
        if self.reloading {
            return EventReaction::Status(StatusUpdate::info(
                "A config reload is in progress; try recovering again in a moment.",
            ));
        }
        // Overwrite a corrupt on-disk config with a fresh render of the
        // in-memory config. Quiesce the writer for the duration of the
        // direct write so a queued save can't clobber the recovery.
        let guard = self.config_writer.quiesce();
        if !guard.is_acknowledged() {
            // The writer never confirmed the pause (timeout or dead). Performing
            // the direct write would race a still-running writer. Abort and let
            // the caller retry once the writer is responsive.
            return EventReaction::Status(StatusUpdate::error(
                "Config writer is busy; please retry.",
            ));
        }
        // The running config may be older than the file's [server.auth]
        // (a `dux config set`, a ban since dux started), so the file's
        // own section is kept whenever it reads. Only when it cannot be
        // read is the running one written back, and the status says so.
        // Read inside the config write lock, so a `dux config set` that
        // lands meanwhile is part of what is kept.
        let running = self.config.clone();
        let surface = &self.surface;
        let outcome =
            crate::config_write::replace_config_file(&self.paths.config_path, |current| {
                let mut recovered = running.clone();
                // A missing file holds no newer password than the
                // running one: nothing to explain.
                let auth_note = match current.map(crate::config::auth_section_of) {
                    None => None,
                    // The file has no password but the running dux has
                    // one: recovering must not quietly turn the login
                    // off, so the running password stays, beside the
                    // file's other auth settings, and the status says so.
                    Some(Ok(mut auth))
                        if !auth.has_password() && running.server.auth.has_password() =>
                    {
                        auth.password_hash
                            .clone_from(&running.server.auth.password_hash);
                        recovered.server.auth = auth;
                        Some(Some(recover_kept_password_note()))
                    }
                    Some(Ok(auth)) => {
                        recovered.server.auth = auth;
                        Some(None)
                    }
                    Some(Err(problem)) => Some(Some(recover_unreadable_auth_note(
                        &recovered.server.auth,
                        problem.reason(),
                    ))),
                };
                Ok((surface.recover_render(&recovered), auth_note))
            });
        match outcome {
            Ok(auth_note) => EventReaction::Status(match auth_note {
                None => {
                    StatusUpdate::info("Restored the last working configuration to config.toml.")
                }
                Some(None) => StatusUpdate::info(
                    "Restored the last working configuration to config.toml. Its \
                     [server.auth] was kept as the file had it.",
                ),
                Some(Some(note)) => StatusUpdate::warning(crate::status_text![
                    "Restored the last working configuration to config.toml. ",
                    note
                ]),
            }),
            Err(e) => EventReaction::Status(StatusUpdate::error(format!(
                "Couldn't restore the last working configuration: {e:#}"
            ))),
        }
    }

    /// [`Command::UpdateMacros`]: put the new macros live and save them, keeping
    /// them even when the write fails.
    fn apply_update_macros(&mut self, macros: crate::config::MacrosConfig) -> EventReaction {
        // Keep and report, never roll back: the new macros go live so
        // the ViewModel reflects them, and a failed write is reported as
        // a possibly stale file rather than reverting the user's change.
        self.config.macros = macros;
        let count = self.config.macros.entries.len();
        if let Err(e) = self.config_writer.save_eager(self.config.clone()) {
            return EventReaction::Status(StatusUpdate::error(format!(
                "Macros updated this session, but saving to config failed: {e}"
            )));
        }
        let msg = if count == 0 {
            "All macros removed. The macro list is now empty.".to_string()
        } else {
            saved_macros_message(count)
        };
        EventReaction::Status(StatusUpdate::info(msg))
    }

    /// Resolve a [`ConfigSetChange`] against the config as it is now and save the
    /// result through the whole-set command, answering about the one entry.
    fn apply_config_set_change(
        &mut self,
        change: ConfigSetChange,
    ) -> anyhow::Result<EventReaction> {
        use crate::wire::{StaleSet, global_env_version, macros_version};
        let (save, done) = match change {
            ConfigSetChange::ReplaceMacros { macros, read_at } => {
                if read_at.is_some_and(|read| read != macros_version(&self.config.macros)) {
                    return Err(StaleSet::Macros.into());
                }
                (ConfigSetSave::Macros(macros), None)
            }
            ConfigSetChange::ReplaceEnv { env, read_at } => {
                if read_at.is_some_and(|read| read != global_env_version(&self.config.env)) {
                    return Err(StaleSet::GlobalEnv.into());
                }
                (ConfigSetSave::Env(env), None)
            }
            ConfigSetChange::SetMacro { name, entry } => {
                let mut macros = self.config.macros.clone();
                match macros.entries.get_mut(&name) {
                    Some(existing) => *existing = entry,
                    None => {
                        macros.entries.insert(name.clone(), entry);
                    }
                }
                (
                    ConfigSetSave::Macros(macros),
                    Some(crate::status_text!["Saved macro ", q(name), "."]),
                )
            }
            ConfigSetChange::RemoveMacro { name } => {
                let mut macros = self.config.macros.clone();
                if macros.entries.shift_remove(&name).is_none() {
                    anyhow::bail!("unknown macro \"{name}\"");
                }
                (
                    ConfigSetSave::Macros(macros),
                    Some(crate::status_text!["Removed macro ", q(name), "."]),
                )
            }
            ConfigSetChange::SetEnvVar { name, value } => {
                let mut env = self.config.env.clone();
                env.insert(name.clone(), value);
                (
                    ConfigSetSave::Env(env),
                    Some(crate::status_text![
                        "Saved global environment variable ",
                        q(name),
                        ". New agents and terminals receive it unless a project overrides it."
                    ]),
                )
            }
            ConfigSetChange::RemoveEnvVar { name } => {
                let mut env = self.config.env.clone();
                if env.remove(&name).is_none() {
                    anyhow::bail!("{}", crate::config_resources::unknown_env_var(&name));
                }
                // A name that is no variable name may be a token pasted
                // where a name goes, so the status does not repeat it.
                let done = if crate::config::is_valid_env_name(&name) {
                    crate::status_text!["Removed global environment variable ", q(name), "."]
                } else {
                    crate::status_text![
                        "Removed a global environment variable whose name is not a variable \
                         name, so it is not shown."
                    ]
                };
                (ConfigSetSave::Env(env), Some(done))
            }
        };
        // The whole-set save's own handler, called directly: the change was
        // admitted once already, as itself.
        let reaction = match save {
            ConfigSetSave::Macros(macros) => self.apply_update_macros(macros),
            ConfigSetSave::Env(env) => self.apply_persist_global_env(env),
        };
        Ok(match (done, reaction) {
            (Some(done), EventReaction::Status(status))
                if status.tone == crate::statusline::StatusTone::Info =>
            {
                EventReaction::Status(StatusUpdate::info(done))
            }
            (_, reaction) => reaction,
        })
    }

    fn dispatch_pull(
        &mut self,
        repo_path: PathBuf,
        target: PullTarget,
        busy_message: crate::status_text::StatusText,
        already_running_message: crate::status_text::StatusText,
    ) -> EventReaction {
        // The pull's check of what git would remove runs on its worker.
        let _engine_thread = crate::engine::destructive_guard::engine_thread();
        let repo_key = repo_path.to_string_lossy().into_owned();
        // A project pull switches the folder to its base first, so it shares
        // the folder's lock with the other two operations that switch it, and
        // a refusal names whichever is running (`already_running_message` is a
        // session pull's). A session pull keeps its own per-worktree key.
        let in_flight_key = match &target {
            PullTarget::Project { project_name, .. } => {
                if let Some(running) = self.project_folder_action(&repo_key) {
                    return EventReaction::Status(StatusUpdate::warning(
                        crate::engine::project_folder_busy_message(project_name, running),
                    ));
                }
                // Taken (and released on a failed spawn) by the worker
                // primitive, which is why this is not `begin_project_folder_action`.
                InFlightKey::ProjectFolder {
                    path: repo_key.clone(),
                    action: crate::engine::ProjectFolderAction::Pull,
                }
            }
            PullTarget::Session => InFlightKey::Pull(repo_key.clone()),
        };
        // A session pull holds its worktree until its completion lands, so a
        // removal that begins meanwhile waits for it; a worktree already being
        // removed takes no pull. A pull already running is left to the worker
        // primitive's own "already running" answer.
        if matches!(target, PullTarget::Session)
            && !self.is_in_flight(&in_flight_key)
            && let Err(refused) = self.hold_path_for_in_flight(
                &in_flight_key,
                &repo_path,
                crate::worktree_ops::WorktreeOpKind::Pull,
            )
        {
            return EventReaction::Status(StatusUpdate::warning(refused.sentence("pull into it")));
        }
        let repo_key_for_panic = repo_key.clone();
        let target_for_panic = target.clone();
        let op = match &target {
            PullTarget::Project { project_name, .. } => {
                project_refresh_status_op(busy_message, project_name)
            }
            PullTarget::Session => crate::engine::status_op(busy_message)
                .on_success(|_: &PullOutcome| {
                    crate::engine::Final::info(
                        "Pulled latest changes from remote successfully. Local branch is up to date.",
                    )
                })
                .on_failure(|error: &String| {
                    crate::engine::Final::error(format!("Pull from remote failed: {error}"))
                }),
        };
        let pending = op.pending_status();
        let panic_key = op.key().to_string();
        let origin = self.current_origin.clone();
        let origin_for_panic = origin.clone();
        let guard = self.checkout_move_guard();
        let timeout = self.config.git.network_timeout();
        self.spawn_command_worker(
            CommandWorkerSpec {
                label: format!("pull:{repo_key}"),
                in_flight_key: Some(in_flight_key),
                busy_status: Some(pending),
                already_running_status: Some(StatusUpdate::warning(already_running_message)),
                panic_event: Some(Box::new(move |reason| WorkerEvent::PullCompleted {
                    repo_path: repo_key_for_panic,
                    target: target_for_panic,
                    result: Err(format!("Pull worker panicked: {reason}")),
                    status: crate::engine::ResolvedFinal::error(
                        panic_key,
                        format!("Pull worker panicked: {reason}"),
                    )
                    .with_scope(origin_for_panic.clone()),
                })),
            },
            move |tx| {
                let result = match &target {
                    PullTarget::Project { leading_branch, .. } => {
                        run_project_refresh(&repo_path, leading_branch.clone(), &guard, timeout)
                    }
                    PullTarget::Session => {
                        crate::git::pull_current_branch(&repo_path, &guard, timeout)
                            .map(|()| PullOutcome::Pulled {
                                current_branch: None,
                            })
                            .map_err(|error| format!("{error:#}"))
                    }
                };
                let status = op.resolve(&result).with_scope(origin.clone());
                let _ = tx.send(WorkerEvent::PullCompleted {
                    repo_path: repo_key,
                    target,
                    result,
                    status,
                });
            },
        )
    }

    fn remove_project_keep_worktrees(
        &mut self,
        project_id: &str,
        project_name: &str,
    ) -> anyhow::Result<EventReaction> {
        if self.project_has_pending_deletion(project_id) {
            return Ok(EventReaction::Status(StatusUpdate::error(
                crate::status_text![
                    "An agent in ",
                    q(project_name),
                    " is still being removed. Try again in a moment."
                ],
            )));
        }
        // Removing the project removes its agents' records, so an agent still
        // coming up in it would come up with no record behind it.
        if self.project_has_launching_tab(project_id) {
            return Ok(EventReaction::Status(StatusUpdate::error(
                crate::status_text![
                    "Cannot remove project ",
                    q(project_name),
                    " while an agent tab is still launching. Wait a moment, then try again."
                ],
            )));
        }
        if self.project_has_create_in_flight(project_id) {
            return Ok(EventReaction::Status(StatusUpdate::error(
                crate::status_text![
                    "Cannot remove project ",
                    q(project_name),
                    " while an agent is being created in it. \
                 Wait for the create to finish, then try again."
                ],
            )));
        }
        let was_real = self.projects.iter().any(|project| project.id == project_id);
        let removed = self.session_store.remove_project_records(project_id)?;
        // What became of each worktree, decided here: kept on disk.
        let kept: Vec<crate::operations::OperationPart> = self
            .sessions
            .iter()
            .filter(|session| removed.contains(&session.id))
            .filter_map(|session| session.workspace.as_managed())
            .map(|managed| crate::operations::OperationPart {
                part: crate::operations::PartKind::Worktree,
                subject: managed.worktree_path.clone(),
                outcome: crate::operations::PartOutcome::Kept,
                reason: None,
            })
            .collect();
        self.note_operation(
            None,
            &crate::operations::OperationNotes {
                parts: kept,
                ..Default::default()
            },
        );
        for session_id in &removed {
            self.finish_delete_session_memory(session_id);
        }
        self.remove_project_from_runtime(project_id);
        let detail = crate::engine::removal::removed_agents_detail(removed.len());
        if was_real && let Err(error) = self.persist_projects_to_config() {
            return Ok(EventReaction::Status(StatusUpdate::error(
                crate::status_text![
                    "Removed ",
                    q(project_name),
                    format!(
                        "{} from dux, but updating config.toml failed: \
                 {}. The project may reappear on restart. Check the file is writable.",
                        detail, error
                    )
                ],
            )));
        }
        Ok(EventReaction::Status(StatusUpdate::info(
            crate::status_text![
                "Removed project ",
                q(project_name),
                format!("{}. Worktrees were kept on disk.", detail)
            ],
        )))
    }

    fn project_has_pending_deletion(&self, project_id: &str) -> bool {
        self.sessions.iter().any(|session| {
            session.project_id() == Some(project_id) && self.pending_deletions.contains(&session.id)
        })
    }

    pub(crate) fn project_has_launching_tab(&self, project_id: &str) -> bool {
        self.sessions.iter().any(|session| {
            session.project_id() == Some(project_id)
                && self
                    .tab_ids_for_session(&session.id)
                    .iter()
                    .any(|tab_id| self.is_in_flight(&InFlightKey::AgentLaunch(tab_id.clone())))
        })
    }

    pub(crate) fn remove_project_from_runtime(&mut self, project_id: &str) {
        self.begin_close_project_terminals(project_id);
        self.projects.retain(|project| project.id != project_id);
    }

    fn apply_project_persistence(
        &mut self,
        action: ProjectPersistenceAction,
        status_op_id: Option<String>,
    ) -> anyhow::Result<EventReaction> {
        match action {
            ProjectPersistenceAction::Add {
                project,
                status_message,
            } => self.add_project_inline(project, status_message),
            action => {
                self.spawn_project_persistence(action, status_op_id);
                Ok(EventReaction::Nothing)
            }
        }
    }

    fn add_project_inline(
        &mut self,
        project: Project,
        status_message: crate::status_text::StatusText,
    ) -> anyhow::Result<EventReaction> {
        if let Some((project_id, project_name)) = self
            .projects
            .iter()
            .find(|existing| existing.path == project.path)
            .map(|existing| (existing.id.clone(), existing.name.clone()))
        {
            let message = crate::status_text![
                "Project at this path is already in the workspace as ",
                q(project_name),
                "; nothing new was added."
            ];
            return Ok(project_added_reaction(project, project_id, message));
        }

        // Held while the project is written: a removal of a folder around
        // the repository waits for the add and then sees the project, and an
        // add into a folder already being removed is refused.
        let _hold = match self.worktree_ops().hold(
            std::path::Path::new(&project.path),
            crate::worktree_ops::WorktreeOpKind::AddProject,
        ) {
            Ok(hold) => hold,
            Err(refused) => {
                return Ok(EventReaction::Status(StatusUpdate::error(
                    refused.sentence("add a project there"),
                )));
            }
        };
        self.session_store
            .upsert_project(&crate::config::ProjectConfig {
                id: project.id.clone(),
                path: project.path.clone(),
                name: Some(project.name.clone()),
                default_provider: project
                    .explicit_default_provider
                    .as_ref()
                    .map(|provider| provider.as_str().to_string()),
                leading_branch: project.leading_branch.clone(),
                auto_reopen_agents: project.auto_reopen_agents,
                startup_command: project.startup_command.clone(),
                env: project.env.clone(),
            })?;
        let project_id = project.id.clone();
        let previous_config_projects = self.config.projects.clone();
        self.projects.push(project.clone());
        match self.persist_projects_to_config() {
            Ok(()) => Ok(project_added_reaction(project, project_id, status_message)),
            Err(error) => {
                Ok(self.rollback_project_add(&project_id, previous_config_projects, &error))
            }
        }
    }

    fn rollback_project_add(
        &mut self,
        project_id: &str,
        previous_config_projects: Vec<crate::config::ProjectConfig>,
        config_error: &anyhow::Error,
    ) -> EventReaction {
        self.projects.retain(|project| project.id != project_id);
        self.config.projects = previous_config_projects;
        if let Err(database_error) = self.session_store.delete_project(project_id) {
            return EventReaction::Status(
                StatusUpdate::error(format!(
                    "Project add failed and couldn't be cleaned up, it may reappear on restart. \
                     Config error: {config_error:#}. DB cleanup error: {database_error:#}"
                ))
                .sticky(),
            );
        }
        EventReaction::Status(StatusUpdate::error(format!(
            "Project add failed and was rolled back; config.toml could not be updated: \
             {config_error:#}"
        )))
    }

    /// Validate and apply a new global agent order. `session_ids` must be the
    /// complete set of all sessions and the whole Vec is re-sorted to match,
    /// because agents are one list and a dragged agent can land anywhere. The
    /// store is written first, then the in-memory Vec.
    fn reorder_agents(&mut self, session_ids: &[String]) -> anyhow::Result<()> {
        let current: Vec<String> = self.sessions.iter().map(|s| s.id.clone()).collect();
        validate_reorder(&current, session_ids, "agent")?;

        self.session_store.set_global_session_order(session_ids)?;

        let new_pos: std::collections::HashMap<&str, usize> = session_ids
            .iter()
            .enumerate()
            .map(|(i, id)| (id.as_str(), i))
            .collect();
        reorder_in_place(&mut self.sessions, |s| new_pos.get(s.id.as_str()).copied());
        Ok(())
    }

    /// Validate and apply a new global order for every companion terminal.
    /// `terminal_ids` must be exactly the full set of current terminal ids; each
    /// terminal's runtime `sort_order` is stamped to its index. Like
    /// [`reorder_agents`] but with no storage call, because terminals are
    /// runtime-only, so the order resets to creation order on restart. A pure
    /// permutation of `sort_order`: reordering is not activity, so it leaves
    /// `pty_activity` and `updated_at` alone.
    fn reorder_terminals(&mut self, terminal_ids: &[String]) -> anyhow::Result<()> {
        let current: Vec<String> = self.companion_terminals.keys().cloned().collect();
        validate_reorder(&current, terminal_ids, "terminal")?;

        for (index, id) in terminal_ids.iter().enumerate() {
            if let Some(terminal) = self.companion_terminals.get_mut(id) {
                terminal.sort_order =
                    u64::try_from(index).expect("a terminal list index fits in u64");
            }
        }
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

    /// Persist the current in-memory session order to storage, per project, so a
    /// sort action's chosen order survives a reload and matches every surface by
    /// construction. Does not re-sort the Vec, which is already in the desired
    /// order; it only writes each project's ordered id list.
    ///
    /// # Errors
    ///
    /// Returns an error when the session database cannot store the order.
    pub fn persist_session_order(&self) -> anyhow::Result<()> {
        // Agents are one global list, so the in-memory Vec order is persisted
        // as a single global `sort_order` permutation rather than per project.
        let ids: Vec<String> = self.sessions.iter().map(|s| s.id.clone()).collect();
        self.session_store.set_global_session_order(&ids)?;
        Ok(())
    }
}

fn project_added_reaction(
    project: Project,
    project_id: String,
    status_message: crate::status_text::StatusText,
) -> EventReaction {
    EventReaction::ProjectPersistenceOutcome(Box::new(ProjectPersistenceOutcome {
        action: ProjectPersistenceAction::Add {
            project,
            status_message: status_message.clone(),
        },
        view: ProjectPersistenceView::Added {
            project_id,
            status_message,
        },
        status_op_id: None,
    }))
}

/// The status line confirming a global environment save, counting the
/// variables and matching the pronoun that refers back to them.
fn saved_global_env_message(count: usize) -> String {
    let them = if count == 1 { "it" } else { "them" };
    format!(
        "Saved {}. New agents and terminals will receive {them} unless a project overrides \
         the same key.",
        count_of(count, "global environment variable")
    )
}

/// The status line confirming a macro save, counting the macros written.
fn saved_macros_message(count: usize) -> String {
    format!("Saved {}.", count_of(count, "macro"))
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

/// Re-sort `items` so elements for which `position` returns `Some(p)` are
/// ordered by `p` among themselves, while elements returning `None` keep their
/// original relative order. Only each group's internal relative order is
/// guaranteed: interleaving between the two groups may change, because
/// positioned items can compact toward the front of the range they sort into.
fn reorder_in_place<T>(items: &mut Vec<T>, position: impl Fn(&T) -> Option<usize>) {
    // A stable sort of the original indices by (key, original_index), where the
    // key is the new position for positioned items and the original index for
    // the rest. The reordered elements are written back into the same slot
    // sequence, which is what preserves the relative order of the None items.
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

/// The tri-state status op for a `PullTarget::Project` refresh. A failure is a
/// warning rather than an error: the refresh is best-effort and the project
/// keeps working from local branch state.
fn project_refresh_status_op(
    busy_message: crate::status_text::StatusText,
    project_name: &str,
) -> crate::engine::StatusOp<crate::worker::PullOutcome, String> {
    let pn_ok = project_name.to_string();
    let pn_err = project_name.to_string();
    crate::engine::status_op(busy_message)
        .on_success(move |outcome: &crate::worker::PullOutcome| match outcome {
            crate::worker::PullOutcome::Pulled { .. } => {
                crate::engine::Final::info(crate::status_text![
                    "Refreshed project ",
                    q(pn_ok),
                    ". Local branch is up to date with remote."
                ])
            }
            crate::worker::PullOutcome::NoOrigin { .. } => {
                crate::engine::Final::info(crate::status_text![
                    "Project ",
                    q(pn_ok),
                    " has no origin remote; nothing to pull. Local branch state refreshed."
                ])
            }
        })
        .on_failure(move |e: &String| {
            crate::engine::Final::warning(crate::status_text![
                "Could not refresh ",
                q(pn_err),
                format!(
                    " from origin: {}. Continuing from the local branch state.",
                    e
                )
            ])
        })
}

/// Body of the `PullTarget::Project` refresh worker. A dirty checkout never
/// gates the refresh: git itself refuses a fast-forward only when it would clobber a
/// locally edited file, which surfaces through the error path.
fn run_project_refresh(
    repo_path: &std::path::Path,
    leading_branch: Option<String>,
    guard: &crate::checkout_move::CheckoutMoveGuard,
    timeout: Option<std::time::Duration>,
) -> Result<crate::worker::PullOutcome, String> {
    use crate::worker::PullOutcome;
    let leading_branch = match leading_branch {
        Some(branch) => branch,
        None => crate::git::current_branch_opt(repo_path)
            .map(|opt_branch| {
                crate::project_browser::leading_branch_for_project(repo_path, opt_branch.as_deref())
            })
            .map_err(|e| format!("{e:#}"))?,
    };
    crate::git::switch_branch_if_needed(repo_path, &leading_branch, guard)
        .map_err(|e| format!("{e:#}"))?;
    if !crate::git::has_origin_remote(repo_path).map_err(|e| format!("{e:#}"))? {
        // Nothing to pull, and that is fine; still re-read the current branch
        // so the sidebar stays fresh.
        return Ok(PullOutcome::NoOrigin {
            current_branch: crate::git::current_branch(repo_path).ok(),
        });
    }
    crate::git::pull_branch(repo_path, &leading_branch, guard, timeout)
        .map_err(|e| format!("{e:#}"))?;
    Ok(PullOutcome::Pulled {
        current_branch: crate::git::current_branch(repo_path).ok(),
    })
}

/// A terminating PTY a launch has to outlive: the gone signal and the moment
/// the reaper force-kills it.
pub(crate) struct LaunchPredecessor {
    gone: super::PtyGone,
    deadline: std::time::Instant,
}

impl Engine {
    /// The terminating PTYs of `session_id` that run `provider`, which a new
    /// launch of that provider must wait for.
    pub(crate) fn terminating_predecessors(
        &self,
        session_id: &str,
        provider: &crate::model::ProviderKind,
    ) -> Vec<LaunchPredecessor> {
        self.terminating_ptys
            .iter()
            .filter(|entry| entry.kind == super::PrunedPtyKind::Agent)
            .filter(|entry| {
                entry.owner.as_ref().is_some_and(|owner| {
                    owner.session_id == session_id && owner.provider.as_ref() == Some(provider)
                })
            })
            .map(|entry| LaunchPredecessor {
                gone: entry.gone.signal(),
                deadline: entry.deadline,
            })
            .collect()
    }
}

/// Hold a launch until every previous run of its provider in the same agent
/// has gone, saying so while it waits.
///
/// Bounded: each predecessor is force-killed at its own deadline (the close
/// grace) by the reaper, and the wait allows that kill
/// [`crate::process_sessions::KILL_SETTLE`] more to land. A predecessor still
/// there after that refuses this launch, because two CLIs of one provider in
/// one worktree is the thing being prevented.
fn wait_for_predecessors(
    predecessors: &[LaunchPredecessor],
    request: &crate::worker::AgentLaunchRequest,
    tx: &std::sync::mpsc::Sender<WorkerEvent>,
) -> Result<(), String> {
    if predecessors.is_empty() {
        return Ok(());
    }
    let provider = request.provider.as_str();
    let label = request.session.display_label();
    let _ = tx.send(WorkerEvent::PollerStatus(StatusUpdate::info(
        crate::status_text![
            "Waiting for the previous ",
            q(provider),
            " of agent ",
            q(label.clone()),
            " to finish before starting it again\u{2026}"
        ],
    )));
    for predecessor in predecessors {
        let bound = predecessor
            .deadline
            .saturating_duration_since(std::time::Instant::now())
            + crate::process_sessions::KILL_SETTLE;
        if !predecessor.gone.wait(bound) {
            return Err(format!(
                "the previous {provider} of agent \"{label}\" was still shutting down after \
                 dux force-closed it, so this one was not started rather than run two of them \
                 in one worktree. Try again in a moment."
            ));
        }
    }
    Ok(())
}

/// The final a create-agent op ends on, worded once for both surfaces.
/// `cloned` is set for a clone's first agent, so a create that fails also says
/// what the clone already did, which is on disk and in the workspace.
pub(super) fn create_launch_final(
    outcome: &crate::engine::CreateLaunchOutcome,
    cloned: Option<&crate::clone_project::ClonedFor>,
) -> crate::engine::Final {
    use crate::engine::{CreateLaunchOutcome, Final};
    // `attempted` is the name the create had settled on when it failed, which
    // a review copy of a pull request makes different from the name asked for.
    let after_clone =
        |message: crate::status_text::StatusText, attempted: Option<&str>| match cloned {
            Some(cloned) => cloned.agent_missing_named(attempted, message),
            None => message,
        };
    match outcome {
        CreateLaunchOutcome::Committed {
            status_message,
            quiet_on,
            warns,
        } => if *warns {
            Final::warning(status_message.clone())
        } else {
            Final::info(status_message.clone())
        }
        .quiet_on(*quiet_on),
        CreateLaunchOutcome::StartupFailed {
            branch_name,
            error,
            notes,
        } => {
            // Sticky: provisioning stopped part-way, so the
            // worktree is in an unknown state and the
            // message sends the user to the startup command
            // logs, an action outside the toast. The
            // failure leads; what the create's notes said
            // is still true and follows it.
            let failure = if let Some(cloned) = cloned {
                cloned.startup_failed(branch_name, error)
            } else {
                crate::status_text![
                    "Startup command failed for agent ",
                    q(branch_name),
                    format!(
                        ": {}. \
             Open the startup command logs for details.",
                        error
                    )
                ]
            };
            Final::error(match notes {
                Some(notes) => {
                    crate::status_text![failure, " ", notes.clone()]
                }
                None => failure,
            })
            .sticky()
        }
        CreateLaunchOutcome::PersistFailed {
            error,
            attempted_name,
            left_folder,
            left_branch,
        } => {
            // Sticky: the worktree exists but its session row
            // does not, so dux forgets it on restart and the
            // user is left the directory to clean up.
            let failed = format!("Could not save the new agent: {error}.");
            let message = match (left_folder, left_branch) {
                (Some(folder), Some(branch)) => crate::status_text![
                    format!("{failed} Its working copy "),
                    n(folder),
                    " and branch ",
                    q(branch),
                    " are left on disk."
                ],
                (Some(folder), None) => crate::status_text![
                    format!("{failed} Its working copy "),
                    n(folder),
                    " is left on disk."
                ],
                (None, Some(branch)) => crate::status_text![
                    format!("{failed} Its branch "),
                    q(branch),
                    " is left behind."
                ],
                (None, None) => failed.into(),
            };
            Final::error(after_clone(message, attempted_name.as_deref())).sticky()
        }
        CreateLaunchOutcome::Failed {
            message,
            attempted_name,
        } => Final::error(after_clone(message.clone(), attempted_name.as_deref())),
    }
}

/// What Recover config adds when the file's `[server.auth]` had no password
/// but the running dux has one: the running password stayed.
fn recover_kept_password_note() -> crate::status_text::StatusText {
    crate::status_text![
        "The file's [server.auth] had no password but the running dux has one, so the running \
         password was kept. To remove it, run ",
        q("dux config set server.auth.password_hash \"\""),
        "."
    ]
}

/// What Recover config adds when the file's `[server.auth]` could not be read,
/// so the running settings were written back.
fn recover_unreadable_auth_note(
    running: &crate::config_auth::ServerAuthConfig,
    reason: &str,
) -> crate::status_text::StatusText {
    let password = if running.has_password() {
        "a password is set"
    } else {
        "no password is set"
    };
    crate::status_text![
        format!(
            "The file's [server.auth] could not be read, so the running dux's web password \
             settings were written back ({password}); check them with "
        ),
        q("dux config get server.auth"),
        format!(". What could not be read: {reason}")
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The commands a Recover config note names reach the web as chips and the
    /// status line in quotes, never between backticks.
    #[test]
    fn the_recover_notes_name_their_commands_as_chips() {
        for (note, command) in [
            (
                recover_kept_password_note(),
                "dux config set server.auth.password_hash \"\"",
            ),
            (
                recover_unreadable_auth_note(
                    &crate::config_auth::ServerAuthConfig {
                        password_hash: "set".to_string(),
                        ..crate::config_auth::ServerAuthConfig::default()
                    },
                    "not TOML",
                ),
                "dux config get server.auth",
            ),
        ] {
            assert!(!note.contains('`'), "{note}");
            assert!(note.contains(&format!("\"{command}\"")), "{note}");
            assert!(
                note.segments().expect("built from parts").contains(
                    &crate::prose::ProseSegment::Name {
                        name: command.to_string(),
                        quoted: true,
                    }
                ),
                "{note:?}"
            );
        }
    }
    use crate::engine::test_support::{sample_project, sample_session, test_engine};
    use crate::ids::TabId;
    use crate::statusline::StatusTone;

    /// Feed a status update into a controller the way a surface's reaction
    /// handler does.
    fn show(status: &mut crate::statusline::KeyedStatusController, update: &StatusUpdate) {
        status.set(
            std::time::Instant::now(),
            update.key.clone(),
            update.tone,
            update.message.clone(),
        );
    }

    #[test]
    fn push_and_pull_keep_their_spinners_while_their_workers_run() {
        // Neither of these operations has an op registry of its own, so the
        // first cut of liveness (which enumerated the registries) still called
        // both of them timed out twenty seconds in, whatever the network was
        // doing. Registration happens in the two spawn primitives instead, so
        // both are covered without either dispatch site knowing about it.
        let (mut engine, tmp) = test_engine();
        let mut status = crate::statusline::KeyedStatusController::emitting_finals()
            .with_live_keys(engine.live_status_keys.clone());
        let t0 = std::time::Instant::now();

        // Pull goes through `spawn_command_worker`; its busy rides the worker
        // channel, posted ahead of anything that worker can send. It is
        // dispatched first, while the channel is still empty: the push worker
        // fails within milliseconds on this non-repository directory and posts
        // its completion onto the same channel, so with push first that
        // completion can land ahead of the pull's busy.
        engine
            .apply(Command::Pull {
                repo_path: tmp.path().to_path_buf(),
                target: PullTarget::Session,
                busy_message: "Pulling latest changes\u{2026}".to_string().into(),
                already_running_message: "A pull is already running.".to_string().into(),
            })
            .expect("apply succeeds");
        let Ok(WorkerEvent::CommandWorkerStarted(pull_busy)) = engine.worker_rx.try_recv() else {
            panic!("expected the pull busy ahead of anything the worker sends")
        };
        let pull_key = pull_busy.key.clone().expect("the pull busy is keyed");
        show(&mut status, &pull_busy);

        // Push goes through `spawn_status_op`; its busy rides the reaction.
        let reaction = engine
            .apply(Command::Push {
                worktree_path: tmp.path().to_path_buf(),
            })
            .expect("apply succeeds");
        let EventReaction::Status(push_busy) = reaction else {
            panic!("expected a pending busy from the push dispatch")
        };
        let push_key = push_busy.key.clone().expect("the push busy is keyed");
        assert_eq!(push_busy.tone, StatusTone::Busy);
        show(&mut status, &push_busy);

        assert!(engine.status_op_is_live(&push_key));
        assert!(engine.status_op_is_live(&pull_key));

        let changes = c_tick(&mut status, t0);
        assert!(
            changes.upgraded.is_empty(),
            "neither may be called timed out while its worker runs, got {:?}",
            changes.upgraded
        );
        assert_eq!(
            changes.refreshed.len(),
            2,
            "both spinners are re-sent instead, got {:?}",
            changes.refreshed
        );

        // Each is retired by its own final, and only its own.
        show(
            &mut status,
            &StatusUpdate::info("Pushed.").with_key(push_key.clone()),
        );
        assert!(!engine.status_op_is_live(&push_key));
        assert!(
            engine.status_op_is_live(&pull_key),
            "one final must not retire the other operation"
        );
        show(
            &mut status,
            &StatusUpdate::error("Pull failed.").with_key(pull_key.clone()),
        );
        assert!(!engine.status_op_is_live(&pull_key));
        assert!(
            engine.live_status_keys.is_empty(),
            "and nothing is left registered: {:?}",
            engine.live_status_keys
        );
    }

    fn c_tick(
        status: &mut crate::statusline::KeyedStatusController,
        t0: std::time::Instant,
    ) -> crate::statusline::StatusTickChanges {
        status.tick(
            t0 + crate::statusline::BUSY_TIMEOUT + std::time::Duration::from_secs(1),
            crate::statusline::BUSY_TIMEOUT,
        )
    }

    #[test]
    fn a_worker_that_cannot_be_spawned_takes_its_spinner_and_its_op_with_it() {
        // The one path that emits a busy and can then never produce a final.
        // Abandoning the operation is what stops the spinner outliving the
        // process: the keyed error replaces it on screen, and dropping the
        // registration stops liveness heartbeating it in the meantime.
        let (mut engine, _tmp) = test_engine();
        let op = crate::engine::status_op("Creating a new agent\u{2026}").resolve_in_handler(
            |_: &crate::engine::CreateLaunchOutcome| crate::engine::Final::clear(),
        );
        let op_id = op.id().to_string();
        let _ = engine.begin_status_op(&op);
        engine.pending_create_ops.insert(op_id.clone(), op);
        assert!(engine.status_op_is_live(&op_id));

        engine.abandon_status_op(&op_id);

        assert!(
            !engine.status_op_is_live(&op_id),
            "a spinner nothing will ever answer must not be heartbeated"
        );
        assert!(
            engine.pending_create_ops.is_empty(),
            "and the op it would have resolved is dropped with it"
        );
    }

    /// The counter behind "did this surface change anything on this iteration?".
    /// It has to move for EVERY apply, including one the reload barrier defers,
    /// because the caller is asking about activity, not about success.
    #[test]
    fn every_applied_command_moves_the_apply_counter() {
        let (mut engine, _tmp) = test_engine();
        let before = engine.command_applies;
        let _ = engine.apply(Command::RecoverConfig);
        assert_eq!(
            engine.command_applies,
            before.wrapping_add(1),
            "an ordinary apply must be counted"
        );

        engine.reloading = true;
        let deferred_before = engine.command_applies;
        let _ = engine.apply(Command::ReloadConfig);
        assert_eq!(
            engine.command_applies,
            deferred_before.wrapping_add(1),
            "a deferred apply is still a caller asking for something"
        );
    }

    #[test]
    fn config_mutation_is_deferred_unchanged_while_reload_barrier_is_open() {
        let (mut engine, _tmp) = test_engine();
        engine.reloading = true;
        let original_macros = engine.config.macros.clone();
        let mut deferred_macros = original_macros.clone();
        deferred_macros.entries.insert(
            "deferred".to_string(),
            crate::config::MacroEntry {
                text: "echo deferred".to_string(),
                surface: crate::config::MacroSurface::Both,
            },
        );

        let reaction = engine
            .apply(Command::UpdateMacros {
                macros: deferred_macros,
            })
            .expect("defer config mutation");

        assert!(matches!(reaction, EventReaction::Nothing));
        assert_eq!(engine.config.macros, original_macros);
        assert_eq!(engine.deferred_commands.len(), 1);
        assert!(matches!(
            engine.deferred_commands.first(),
            Some(Command::UpdateMacros { .. })
        ));
    }

    #[test]
    fn duplicate_project_add_reports_the_registered_project_without_adding_a_second_row() {
        let (mut engine, tmp) = test_engine();
        let path = tmp.path().join("shared-project");
        let registered = sample_project("registered", path.to_string_lossy().as_ref());
        engine.projects.push(registered.clone());

        let mut duplicate = sample_project("duplicate", path.to_string_lossy().as_ref());
        duplicate.name = "Losing request".to_string();
        let reaction = engine
            .apply(Command::PersistProject {
                action: Box::new(ProjectPersistenceAction::Add {
                    project: duplicate,
                    status_message: "Added losing request".to_string().into(),
                }),
                status_op_id: Some("ignored-for-inline-add".to_string()),
            })
            .expect("deduplicate project add");

        let EventReaction::ProjectPersistenceOutcome(outcome) = reaction else {
            panic!("expected an inline project persistence outcome");
        };
        assert!(matches!(
            outcome.view,
            ProjectPersistenceView::Added {
                ref project_id,
                ref status_message,
            } if project_id == "registered"
                && status_message.contains(&registered.name)
        ));
        assert_eq!(outcome.status_op_id, None);
        assert_eq!(engine.projects.len(), 1);
        assert_eq!(engine.projects[0].id, "registered");
    }

    #[test]
    fn remove_project_closes_project_terminals_but_not_other_projects() {
        let (mut engine, _tmp) = test_engine();
        let repo1 = tempfile::tempdir().expect("p1 dir");
        let repo2 = tempfile::tempdir().expect("p2 dir");
        engine.projects.push(sample_project(
            "p1",
            repo1.path().to_string_lossy().as_ref(),
        ));
        engine.projects.push(sample_project(
            "p2",
            repo2.path().to_string_lossy().as_ref(),
        ));
        engine.config.terminal.command = "cat".to_string();
        engine.config.terminal.args = vec![];
        let (t1, _) = engine
            .create_project_terminal("p1", 24, 80)
            .expect("terminal on p1");
        let (t2, _) = engine
            .create_project_terminal("p2", 24, 80)
            .expect("terminal on p2");

        engine
            .apply(Command::RemoveProject {
                project_id: "p1".to_string(),
                project_name: "p1".to_string(),
            })
            .expect("remove project");

        assert!(
            !engine.companion_terminals.contains_key(&t1),
            "removing a project must close its project terminals"
        );
        assert!(
            engine.terminating_ptys.iter().any(|t| t.id == t1),
            "the closed terminal is reaped gracefully via the terminating set"
        );
        assert!(
            engine.companion_terminals.contains_key(&t2),
            "another project's terminal must be untouched"
        );
        // Pin the surviving project by name: "p1 is absent" alone is also true
        // of a remove that wiped every project.
        assert_eq!(
            engine
                .projects
                .iter()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            vec!["p2"],
            "only the removed project is gone",
        );
    }

    #[test]
    fn removing_a_project_is_refused_while_one_of_its_agents_launches_or_is_being_created() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let session = sample_session("s1", "p1", "feat");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);
        let try_remove = |engine: &mut Engine| {
            engine.apply(Command::RemoveProject {
                project_id: "p1".to_string(),
                project_name: "p1".to_string(),
            })
        };
        let remove = |engine: &mut Engine| try_remove(engine).expect("the command answers");
        let still_there = |engine: &Engine| {
            engine.projects.iter().any(|p| p.id == "p1")
                && engine.sessions.iter().any(|s| s.id == "s1")
        };

        engine.mark_in_flight(InFlightKey::AgentLaunch(TabId::new("s1-slot")));
        let reaction = remove(&mut engine);
        let EventReaction::Status(status) = reaction else {
            panic!("a refusal status")
        };
        assert_eq!(status.tone, StatusTone::Error);
        assert!(status.message.contains("launching"), "{}", status.message);
        assert!(still_there(&engine));
        engine.clear_in_flight(&InFlightKey::AgentLaunch(TabId::new("s1-slot")));

        engine.note_create_started("op-c", Some("p1"));
        let reaction = remove(&mut engine);
        let EventReaction::Status(status) = reaction else {
            panic!("a refusal status")
        };
        assert_eq!(status.tone, StatusTone::Error);
        assert!(
            status.message.contains("being created"),
            "{}",
            status.message
        );
        assert!(still_there(&engine));

        // With a followed create behind it, the refusal names that operation.
        let policy = crate::operations::OperationPolicy {
            unknown_after: std::time::Duration::from_secs(60),
            retention: std::time::Duration::from_secs(60),
        };
        engine.operations.open(
            "op-c",
            crate::operations::OperationKind::AgentCreate,
            policy,
            std::time::Instant::now(),
        );
        engine
            .operations
            .admit(
                Some("op-c"),
                &[],
                &[crate::operations::Hold::shared(InFlightKey::Project(
                    "p1".to_string(),
                ))],
            )
            .unwrap();
        let Err(refused) = try_remove(&mut engine) else {
            panic!("refused")
        };
        assert!(refused.to_string().contains("op-c"), "{refused}");
        engine.note_create_finished("op-c");
        engine.operations.finish(
            "op-c",
            StatusTone::Info,
            "",
            None,
            std::time::Instant::now(),
        );

        // And a followed start of the launching tab.
        engine.mark_in_flight(InFlightKey::AgentLaunch(TabId::new("s1-slot")));
        engine.operations.open(
            "op-start",
            crate::operations::OperationKind::AgentStart,
            policy,
            std::time::Instant::now(),
        );
        engine
            .operations
            .admit(
                Some("op-start"),
                &[],
                &[crate::operations::Hold::exclusive(InFlightKey::Agent(
                    "s1".to_string(),
                ))],
            )
            .unwrap();
        let Err(refused) = try_remove(&mut engine) else {
            panic!("refused")
        };
        assert!(refused.to_string().contains("op-start"), "{refused}");
        assert!(still_there(&engine));
    }

    #[test]
    fn delete_project_removes_its_sessions_worktrees_and_project() {
        // The worktree-deleting project delete (distinct from RemoveProject, which
        // keeps worktrees) drops the project row and memory at once and sends each
        // session through the deferred, worker-run removal a single delete takes.
        // A non-existent worktree path makes the removal take its already-gone
        // branch without a real git checkout (the removal itself is covered in
        // `engine::removal`).
        let (mut engine, tmp) = test_engine();
        let proj = tmp.path().join("proj");
        std::fs::create_dir_all(&proj).unwrap();
        engine
            .projects
            .push(sample_project("p1", proj.to_str().unwrap()));
        let mut session = sample_session("s1", "p1", "feat/x");
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = tmp.path().join("wt-gone").to_str().unwrap().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        let reaction = engine
            .apply(Command::DeleteProject {
                project_id: "p1".to_string(),
                project_name: "p1".to_string(),
            })
            .expect("delete project");
        let EventReaction::Multi(started) = reaction else {
            panic!("expected the cascade's busy");
        };
        let Some(EventReaction::Status(busy)) = started.first() else {
            panic!("the busy leads");
        };
        assert_eq!(busy.tone, StatusTone::Busy);
        assert!(
            engine.projects.iter().all(|p| p.id != "p1"),
            "project must be gone from memory"
        );
        assert!(
            engine.sessions.iter().all(|s| s.id != "s1"),
            "session must be gone from memory"
        );

        let event = engine
            .worker_rx
            .recv_timeout(std::time::Duration::from_secs(20))
            .expect("the removal reports");
        let EventReaction::Status(status) = engine.process_worker_event(event) else {
            panic!("expected the cascade's final");
        };
        assert_eq!(status.key, busy.key, "the final replaces the busy");
        assert_eq!(status.tone, StatusTone::Info, "expected a success status");
        assert!(
            status.message.contains("its agent"),
            "message should mention the removed agent: {}",
            status.message
        );
    }

    #[test]
    fn delete_project_goes_ahead_past_an_agent_delete_already_in_progress() {
        // That agent's own removal is already running and reports on its own;
        // the project delete neither refuses nor runs it a second time.
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let session = sample_session("s1", "p1", "feat/x");
        engine.sessions.push(session);
        engine.pending_deletions.insert("s1".to_string());

        let reaction = engine
            .apply(Command::DeleteProject {
                project_id: "p1".to_string(),
                project_name: "p1".to_string(),
            })
            .expect("apply returns Ok");
        let tones: Vec<StatusTone> = match &reaction {
            EventReaction::Multi(all) => all
                .iter()
                .filter_map(|r| match r {
                    EventReaction::Status(status) => Some(status.tone),
                    _ => None,
                })
                .collect(),
            _ => panic!("expected the cascade's statuses"),
        };
        assert_eq!(tones, vec![StatusTone::Busy, StatusTone::Info]);
        assert!(engine.projects.iter().all(|p| p.id != "p1"));
        assert!(engine.sessions.iter().all(|s| s.id != "s1"));
        assert!(
            engine.pending_deletions.contains("s1"),
            "the running removal is left to finish"
        );
    }

    #[test]
    fn delete_project_refuses_while_a_tab_is_launching() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let session = sample_session("s1", "p1", "feat/x");
        engine.sessions.push(session);
        // Mark the session-slot tab's launch in flight, under the id
        // `AgentSession::slot_tab_id` resolves to for this fixture.
        engine.mark_in_flight(InFlightKey::AgentLaunch(TabId::new("s1-slot")));

        let reaction = engine
            .apply(Command::DeleteProject {
                project_id: "p1".to_string(),
                project_name: "p1".to_string(),
            })
            .expect("apply returns Ok");

        match reaction {
            EventReaction::Status(status) => {
                assert_eq!(status.tone, StatusTone::Error);
            }
            _ => panic!("expected an error Status"),
        }
        assert!(
            engine.projects.iter().any(|p| p.id == "p1"),
            "the project must be untouched when the launch guard fires"
        );
    }

    #[test]
    fn dispatch_agent_launch_refuses_a_closing_session() {
        // The shared launch chokepoint must refuse a launch into
        // a session that is mid-deletion (`closing_sessions`), covering every
        // launch path (reconnect, resume-fallback, web-dormant-relaunch,
        // session-slot reconnect) that funnels through DispatchAgentLaunch.
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s1", "p1", "feat/x");
        engine.sessions.push(session.clone());
        engine.closing_sessions.insert("s1".to_string());

        let request = engine.build_agent_launch_request(
            session,
            crate::model::ConversationStart::Fresh,
            (24, 80),
            crate::worker::AgentLaunchKind::Reconnect {
                status_message: crate::status_text::StatusText::default(),
            },
        );
        let reaction = engine
            .apply(Command::DispatchAgentLaunch {
                request: Box::new(request),
            })
            .expect("apply");

        match reaction {
            EventReaction::DispatchAgentLaunchView(view) => {
                assert!(!view.launched, "a closing session must not launch");
                let status = view.status.expect("a refusal status");
                assert!(
                    status.message.contains("being deleted"),
                    "msg: {}",
                    status.message
                );
            }
            _ => panic!("expected DispatchAgentLaunchView"),
        }
        // No in-flight launch key was set (the guard returned before dispatch).
        assert!(!engine.is_in_flight(&InFlightKey::AgentLaunch(TabId::new("s1"))));
    }

    /// A launch whose environment cannot be resolved fails the way a launch
    /// that cannot spawn does: nothing runs, and the tab keeps a recorded
    /// failure with the refusal as its reason.
    #[test]
    fn dispatch_agent_launch_fails_on_an_environment_it_cannot_resolve() {
        let (mut engine, _tmp) = test_engine();
        let mut project = sample_project("p1", "/tmp/p1");
        project.env.insert("BROKEN".to_string(), "x$".to_string());
        engine.projects.push(project);
        let session = sample_session("s1", "p1", "feat/x");
        engine.sessions.push(session.clone());

        let request = engine.build_agent_launch_request(
            session.clone(),
            crate::model::ConversationStart::Fresh,
            (24, 80),
            crate::worker::AgentLaunchKind::Reconnect {
                status_message: crate::status_text::StatusText::default(),
            },
        );
        engine
            .apply(Command::DispatchAgentLaunch {
                request: Box::new(request),
            })
            .expect("apply");
        let event = engine
            .worker_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the launch answers");
        let reaction = engine.process_worker_event(event);

        let message = match reaction {
            EventReaction::AgentLaunchFailedView(outcome) => match *outcome {
                crate::engine::AgentLaunchFailedOutcome::Reconnect { message, .. } => message,
                other => panic!("expected a reconnect failure, got {other:?}"),
            },
            other => panic!("expected a failed launch, got {other:?}"),
        };
        assert!(
            message.starts_with(
                "dux did not start agent \"s1-title\": environment variable BROKEN could not be \
                 resolved, because its value has invalid expansion syntax. Fix or remove it in \
                 the environment variables of project \"p1-name\", then try again."
            ),
            "{message}"
        );
        assert!(engine.tab_last_run_failed(session.slot_tab_id().as_str()));
        assert!(engine.providers.is_empty(), "nothing was started");
    }

    /// A launch over a tab that already has a LIVE process is refused before
    /// anything is spawned. Letting it through would replace the entry in
    /// `providers`, and the displaced client's drop SIGKILLs the child it names,
    /// so a working agent would be killed by a redundant launch with nothing
    /// said anywhere.
    #[test]
    fn dispatch_agent_launch_refuses_a_tab_that_is_already_running() {
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        let session = sample_session("s1", "p1", "feat/x");
        engine.sessions.push(session.clone());
        // `cat` sits waiting on its input, so this provider is genuinely live.
        let live =
            crate::pty::PtyClient::spawn_with_env("cat", &[], worktree.path(), 24, 80, 100, &[])
                .expect("spawn cat");
        let live_pid = live.child_process_id();
        engine
            .providers
            .insert(session.slot_tab_id().to_owned(), live);

        let request = engine.build_agent_launch_request(
            session.clone(),
            crate::model::ConversationStart::Fresh,
            (24, 80),
            crate::worker::AgentLaunchKind::Reconnect {
                status_message: crate::status_text::StatusText::default(),
            },
        );
        let reaction = engine
            .apply(Command::DispatchAgentLaunch {
                request: Box::new(request),
            })
            .expect("apply");

        match reaction {
            EventReaction::DispatchAgentLaunchView(view) => {
                assert!(!view.launched, "a tab with a live process must not launch");
                let status = view.status.expect("a refusal status");
                assert!(
                    status.message.contains("already running"),
                    "msg: {}",
                    status.message
                );
            }
            _ => panic!("expected DispatchAgentLaunchView"),
        }
        assert_eq!(
            engine
                .providers
                .get(session.slot_tab_id())
                .and_then(crate::pty::PtyClient::child_process_id),
            live_pid,
            "the running child must still be the one in the tab"
        );
    }

    /// A dispatched launch clears the tab's recorded failure, and a REFUSED one
    /// leaves it standing. This one chokepoint is why an explicit start, a
    /// reconnect and a create all get past the diagnosis card without any of
    /// them asking for it, and why a refusal (which ran nothing) does not.
    #[test]
    fn dispatch_agent_launch_clears_a_recorded_failure_only_when_it_launches() {
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s1", "p1", "feat/x");
        engine.sessions.push(session.clone());
        engine.mark_tab_run_failed(
            session.slot_tab_id(),
            crate::tab_verdict::TabRunEnding::ExitedWithStatus { status: 1 },
            Vec::new(),
        );

        let request = engine.build_agent_launch_request(
            session.clone(),
            crate::model::ConversationStart::Fresh,
            (24, 80),
            crate::worker::AgentLaunchKind::Reconnect {
                status_message: crate::status_text::StatusText::default(),
            },
        );
        let reaction = engine
            .apply(Command::DispatchAgentLaunch {
                request: Box::new(request),
            })
            .expect("apply");
        assert!(
            matches!(reaction, EventReaction::DispatchAgentLaunchView(ref v) if v.launched),
            "the launch must actually dispatch for this test to say anything"
        );
        assert!(!engine.tab_last_run_failed(session.slot_tab_id().as_str()));

        // Now the refusal side: a closing session runs nothing, so the verdict
        // it would have replaced must survive.
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s2", "p1", "feat/y");
        engine.sessions.push(session.clone());
        engine.closing_sessions.insert("s2".to_string());
        engine.mark_tab_run_failed(
            session.slot_tab_id(),
            crate::tab_verdict::TabRunEnding::ExitedWithStatus { status: 1 },
            Vec::new(),
        );
        let request = engine.build_agent_launch_request(
            session.clone(),
            crate::model::ConversationStart::Fresh,
            (24, 80),
            crate::worker::AgentLaunchKind::Reconnect {
                status_message: crate::status_text::StatusText::default(),
            },
        );
        let _ = engine
            .apply(Command::DispatchAgentLaunch {
                request: Box::new(request),
            })
            .expect("apply");
        assert!(
            engine.tab_last_run_failed(session.slot_tab_id().as_str()),
            "a refused dispatch launched nothing, so it clears nothing"
        );
    }

    #[test]
    fn reorder_agents_moves_across_projects_globally_and_persists() {
        let (mut engine, _tmp) = test_engine();
        // Two projects interleaved: p1-a, p1-b, p2-x, p2-y (Vec order).
        for (id, proj) in [("a", "p1"), ("b", "p1"), ("x", "p2"), ("y", "p2")] {
            let session = sample_session(id, proj, id);
            engine.session_store.upsert_session(&session).unwrap();
            engine.sessions.push(session);
        }

        // Global reorder: interleave across project boundaries, with x (p2) between the
        // two p1 agents. Impossible under the old project-grouped model; the whole
        // point of the flat model.
        engine
            .apply(Command::ReorderAgents {
                session_ids: vec!["a".into(), "x".into(), "b".into(), "y".into()],
            })
            .expect("apply");

        let vec_order: Vec<String> = engine.sessions.iter().map(|s| s.id.clone()).collect();
        assert_eq!(vec_order, vec!["a", "x", "b", "y"]);

        // Persisted: reload reflects the same global order (load_sessions orders by
        // the now-global sort_order).
        let reloaded: Vec<String> = engine
            .session_store
            .load_sessions()
            .unwrap()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(reloaded, vec!["a", "x", "b", "y"]);
    }

    #[test]
    fn reorder_agents_rejects_incomplete_set() {
        let (mut engine, _tmp) = test_engine();
        for (id, proj) in [("a", "p1"), ("b", "p2")] {
            let session = sample_session(id, proj, id);
            engine.session_store.upsert_session(&session).unwrap();
            engine.sessions.push(session);
        }
        // Only one of the two sessions, not the complete set.
        let err = engine
            .apply(Command::ReorderAgents {
                session_ids: vec!["a".into()],
            })
            .map(|_| ())
            .expect_err("incomplete set must error");
        assert!(
            err.to_string().contains("exactly the current"),
            "err: {err}"
        );
    }

    #[test]
    fn reorder_agents_rejects_extra_id() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("a", "p1", "a"));
        let err = engine
            .apply(Command::ReorderAgents {
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
    fn reorder_agents_rejects_duplicate_id() {
        let (mut engine, _tmp) = test_engine();
        for id in ["a", "b"] {
            engine.sessions.push(sample_session(id, "p1", id));
        }
        let err = engine
            .apply(Command::ReorderAgents {
                session_ids: vec!["a".into(), "a".into()],
            })
            .map(|_| ())
            .expect_err("duplicate id must error");
        assert!(err.to_string().contains("duplicate"), "err: {err}");
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
                    env: std::collections::BTreeMap::default(),
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
            .filter(|s| s.project_id().expect("managed test session") == "p1")
            .map(|s| s.id)
            .collect();
        assert_eq!(reloaded, vec!["c", "b", "a"]);
    }

    /// Init a temp git repo with a committed `a.txt` so discard tests can both
    /// restore a tracked file and create an untracked one.
    fn discard_test_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let run = |args: &[&str]| {
            let ok = crate::git::test_support::git_command()
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
                confirmed: crate::git::ConfirmedEntry::File,
            })
            .expect("apply");
        let reaction = finish_discard(&mut engine, reaction);

        match reaction {
            EventReaction::Status(update) => {
                assert_eq!(update.tone, StatusTone::Info);
                assert!(
                    update
                        .message
                        .contains("Discarded unstaged changes to \"a.txt\"")
                );
                assert!(update.message.contains("Staged changes, if any, are kept"));
            }
            _ => panic!("expected Info status reaction"),
        }
        // The working copy is back to the committed content.
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "original\n");
    }

    /// A file row that a repository has since replaced is refused through
    /// the command and nothing is deleted: the confirmation named a file.
    #[test]
    fn discarding_a_repository_through_the_command_is_refused() {
        let repo = discard_test_repo();
        let nested = repo.path().join("clone");
        std::fs::create_dir_all(&nested).expect("folder");
        let status = crate::test_git::fixture_git()
            .args(["init", "-q"])
            .current_dir(&nested)
            .status()
            .expect("git init");
        assert!(status.success());
        std::fs::write(nested.join("own.txt"), "own\n").expect("write");
        let (mut engine, _tmp) = test_engine();

        let reaction = engine
            .apply(Command::DiscardFile {
                worktree_path: repo.path().to_path_buf(),
                path: "clone".to_string(),
                is_untracked: true,
                confirmed: crate::git::ConfirmedEntry::File,
            })
            .expect("a keyed busy");

        let refused = finish_discard(&mut engine, reaction);
        assert!(
            matches!(refused, EventReaction::Status(ref s) if s.tone == StatusTone::Error),
            "refused"
        );
        assert!(nested.join(".git").exists() && nested.join("own.txt").exists());
    }

    /// A destructive discard answers with a busy status and finishes on a
    /// worker: drive it to its final.
    fn finish_discard(engine: &mut Engine, reaction: EventReaction) -> EventReaction {
        let EventReaction::Status(busy) = &reaction else {
            return reaction;
        };
        if busy.tone != crate::statusline::StatusTone::Busy {
            return reaction;
        }
        loop {
            let event = engine
                .worker_rx
                .recv_timeout(std::time::Duration::from_secs(30))
                .expect("the delete reports");
            let done = matches!(event, crate::worker::WorkerEvent::StatusOpCompleted { .. });
            let processed = engine.process_worker_event(event);
            if done {
                return processed;
            }
        }
    }

    /// A folded folder is discarded whole, and the message says it was a
    /// folder: "deleted untracked file" about thirty thousand files is a lie
    /// about how much just went.
    #[test]
    fn discarding_an_untracked_folder_says_it_deleted_a_folder() {
        let repo = discard_test_repo();
        let folder = repo.path().join("node_modules/pkg");
        std::fs::create_dir_all(&folder).expect("folder");
        std::fs::write(folder.join("index.js"), "x\n").expect("write");
        let (mut engine, _tmp) = test_engine();

        let reaction = engine
            .apply(Command::DiscardFile {
                worktree_path: repo.path().to_path_buf(),
                path: "node_modules".to_string(),
                is_untracked: true,
                // Confirmed at three; one is there now, and one is what went.
                confirmed: crate::git::ConfirmedEntry::Folder { files: 3 },
            })
            .expect("apply");
        let reaction = finish_discard(&mut engine, reaction);

        match reaction {
            EventReaction::Status(update) => assert!(
                update.message.contains(
                    "Deleted the untracked files in \"node_modules/\" (1 file); files the repository ignores and repositories of their own inside it are kept."
                ),
                "{}",
                update.message
            ),
            _ => panic!("expected Info status reaction"),
        }
        assert!(!repo.path().join("node_modules").exists());
    }

    /// An empty `.git` directory does not make a folder a repository to git,
    /// so the message must not claim a history went with it.
    #[test]
    fn a_folder_that_only_looks_like_a_repository_is_reported_as_a_folder() {
        let repo = discard_test_repo();
        let folder = repo.path().join("lookalike");
        std::fs::create_dir_all(folder.join(".git")).expect("empty .git");
        std::fs::write(folder.join("a.js"), "a\n").expect("write");
        let (mut engine, _tmp) = test_engine();

        let reaction = engine
            .apply(Command::DiscardFile {
                worktree_path: repo.path().to_path_buf(),
                path: "lookalike".to_string(),
                is_untracked: true,
                confirmed: crate::git::ConfirmedEntry::Folder { files: 1 },
            })
            .expect("apply");
        let reaction = finish_discard(&mut engine, reaction);

        match reaction {
            EventReaction::Status(update) => {
                assert!(
                    update.message.contains("Deleted the untracked files in"),
                    "{}",
                    update.message
                );
                assert!(!update.message.contains("history"), "{}", update.message);
            }
            _ => panic!("expected Info status reaction"),
        }
    }

    /// A commit runs on a worker: a keyed busy at once, the preflight and
    /// `git commit` off the engine thread, and the final.
    #[test]
    fn a_commit_runs_on_a_worker_and_refuses_nothing_staged_in_its_final() {
        let repo = discard_test_repo();
        let (mut engine, _tmp) = test_engine();
        let commit = |engine: &mut Engine| {
            let reaction = engine
                .apply(Command::CommitChanges {
                    worktree_path: repo.path().to_path_buf(),
                    message: "a real message".to_string(),
                    success_message: "Committed.".to_string(),
                })
                .expect("apply");
            assert!(
                matches!(&reaction, EventReaction::Status(s) if s.tone == StatusTone::Busy
                    && s.key.is_some()),
                "a keyed busy"
            );
            match finish_discard(engine, reaction) {
                EventReaction::Status(update) => update,
                _ => panic!("expected the commit's final"),
            }
        };

        let refused = commit(&mut engine);
        assert_eq!(refused.tone, StatusTone::Error);
        assert_eq!(refused.message, COMMIT_NOTHING_STAGED);

        std::fs::write(repo.path().join("b.txt"), "b\n").expect("write");
        let added = crate::git::test_support::git_command()
            .args(["add", "b.txt"])
            .current_dir(repo.path())
            .status()
            .expect("git add");
        assert!(added.success());
        let landed = commit(&mut engine);
        assert_eq!(landed.tone, StatusTone::Info, "{}", landed.message);
        assert_eq!(landed.message, "Committed.");
    }

    /// A file's discard, classified live, runs entirely on a worker: the
    /// engine answers at once with a keyed busy, the final follows, and then
    /// the surfaces are told to read the changed files again.
    #[test]
    fn a_live_file_discard_runs_on_a_worker_then_reloads_the_changes() {
        let repo = discard_test_repo();
        let file = repo.path().join("a.txt");
        std::fs::write(&file, "edited\n").expect("edit");
        let (mut engine, _tmp) = test_engine();

        let reaction = engine
            .apply(Command::DiscardFileLive {
                worktree_path: repo.path().to_path_buf(),
                path: "a.txt".to_string(),
            })
            .expect("apply");
        let EventReaction::Status(busy) = &reaction else {
            panic!("expected a busy status");
        };
        assert_eq!(busy.tone, StatusTone::Busy);
        assert!(busy.key.is_some(), "keyed, so its final replaces it");

        let finished = finish_discard(&mut engine, reaction);
        assert!(
            matches!(&finished, EventReaction::Status(s) if s.tone == StatusTone::Info
                && s.message.contains("Discarded unstaged changes to \"a.txt\"")),
            "the discard's final"
        );
        let next = engine
            .worker_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the reload follows");
        assert!(matches!(
            engine.process_worker_event(next),
            EventReaction::ReloadChangedFiles
        ));
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
                confirmed: crate::git::ConfirmedEntry::File,
            })
            .expect("apply");
        let reaction = finish_discard(&mut engine, reaction);

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

    // ── Command::ReorderTerminals ───────────────────────────────────────────

    use crate::config::{MacroEntry, MacroSurface};
    use crate::pty::PtyClient;

    /// Spawn a `cat -v` PTY in raw mode, a live client for a terminal entry.
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

    /// Register a companion terminal directly in the map for reorder tests, with a
    /// deliberately scrambled `sort_order` so a passing test proves the reorder
    /// (not the insertion order) set the field.
    fn insert_terminal(engine: &mut Engine, id: &str, sort_order: u64) {
        use crate::model::{CompanionTerminal, TerminalOwner};
        engine.companion_terminals.insert(
            id.to_string(),
            CompanionTerminal {
                owner: TerminalOwner::Session("s1".to_string()),
                label: id.to_string(),
                foreground_cmd: None,
                client: spawn_cat_v_pty(),
                sort_order,
                created_at: chrono::Utc::now(),
            },
        );
    }

    #[test]
    fn reorder_terminals_stamps_sort_order_to_full_set_order() {
        let (mut engine, _tmp) = test_engine();
        // Three terminals whose stored sort_order does not match the requested one.
        insert_terminal(&mut engine, "term-1", 99);
        insert_terminal(&mut engine, "term-2", 5);
        insert_terminal(&mut engine, "term-3", 42);

        engine
            .apply(Command::ReorderTerminals {
                terminal_ids: vec!["term-3".into(), "term-1".into(), "term-2".into()],
            })
            .expect("apply");

        // Each terminal's sort_order is now its index in the requested order.
        assert_eq!(engine.companion_terminals["term-3"].sort_order, 0);
        assert_eq!(engine.companion_terminals["term-1"].sort_order, 1);
        assert_eq!(engine.companion_terminals["term-2"].sort_order, 2);
    }

    #[test]
    fn reorder_terminals_rejects_incomplete_set() {
        let (mut engine, _tmp) = test_engine();
        insert_terminal(&mut engine, "term-1", 0);
        insert_terminal(&mut engine, "term-2", 1);

        // Only one of the two terminals, not the complete set.
        let err = engine
            .apply(Command::ReorderTerminals {
                terminal_ids: vec!["term-1".into()],
            })
            .map(|_| ())
            .expect_err("incomplete set must error");
        assert!(
            err.to_string().contains("exactly the current"),
            "err: {err}"
        );
        // The rejected reorder left the stored order untouched.
        assert_eq!(engine.companion_terminals["term-1"].sort_order, 0);
        assert_eq!(engine.companion_terminals["term-2"].sort_order, 1);
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
        // truncates or partially-writes the real file on failure).
        let (mut engine, _tmp) = test_engine();

        // Seed a known-good, parseable config on disk so we can prove a failed
        // write does not corrupt or truncate it.
        let original = "[defaults]\nprovider = \"claude\"\n\n[env]\nSEED = \"keep-me\"\n";
        std::fs::write(&engine.paths.config_path, original).expect("seed config");

        engine.config_writer =
            crate::config_queue::ConfigWriteQueue::with_dead_writer(&engine.paths.config_path);
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

        // The on-disk file is byte-for-byte the original (not truncated, emptied,
        // or partially overwritten) and still parses as valid config.
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
            let ok = crate::git::test_support::git_command()
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
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .worktree_path = repo.to_string_lossy().into_owned();
        session
    }

    fn sample_changed_file(path: &str) -> crate::model::ChangedFile {
        crate::model::ChangedFile {
            status: "M".to_string(),
            path: path.to_string(),
            additions: 1,
            deletions: 0,
            binary: false,
            diff_excluded: false,
            renamed_from: None,
            kind: crate::model::ChangedFileKind::File,
        }
    }

    /// Block on the engine's worker channel for the next `ChangedFilesReady`
    /// event (the one-shot refresh worker sends exactly one), then feed it back
    /// through `process_worker_event` so the engine adopts the computed lists,
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
        let event = crate::engine::test_support::recv_changed_files_ready(&mut engine);
        match event {
            WorkerEvent::ChangedFilesReady { outcome, worktree } => {
                let (staged, unstaged) = outcome.expect("git read the worktree");
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
    fn changed_files_empty_before_watch_populated_after_drain() {
        let repo = watch_test_repo();
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(session_in_repo("s1", repo.path()));

        // Before the watch the engine carries empty lists and no watched id,
        // exactly what the web saw (the pane showed nothing).
        assert!(engine.staged_files.is_empty());
        assert!(engine.unstaged_files.is_empty());
        assert!(engine.watched_session_id.is_none());

        // The command arms the watch (id visible immediately) but population is
        // now async: the watched id is set with EMPTY lists until the worker
        // event drains.
        engine
            .apply(Command::WatchChangedFiles {
                session_id: Some("s1".to_string()),
            })
            .expect("apply");
        assert_eq!(engine.watched_session_id.as_deref(), Some("s1"));
        assert!(engine.staged_files.is_empty());
        assert!(engine.unstaged_files.is_empty());

        // After the worker lands the lists are populated and still tagged.
        drain_changed_files_refresh(&mut engine);
        assert_eq!(engine.watched_session_id.as_deref(), Some("s1"));
        assert!(engine.staged_files.iter().any(|f| f.path == "staged.txt"));
        assert!(
            engine
                .unstaged_files
                .iter()
                .any(|f| f.path == "unstaged.txt")
        );
    }

    // ── run_project_refresh / project_refresh_status_op ──────────────────

    fn refresh_test_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let run = |args: &[&str]| {
            let out = crate::git::test_support::git_command()
                .args(args)
                .current_dir(dir.path())
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        };
        run(&["init", "-b", "main"]);
        run(&["config", "user.name", "test"]);
        run(&["config", "user.email", "t@t"]);
        std::fs::write(dir.path().join("tracked.txt"), "base\n").unwrap();
        run(&["add", "-A"]);
        run(&["commit", "-m", "base"]);
        dir
    }

    fn git_ok(cwd: &std::path::Path, args: &[&str]) {
        let out = crate::git::test_support::git_command()
            .args(args)
            .current_dir(cwd)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Kills the old blanket dirty gate: a dirty tracked file no longer blocks
    /// the refresh, and git fast-forwards when nothing conflicts.
    #[test]
    fn project_refresh_proceeds_with_dirty_checkout() {
        let repo = refresh_test_repo();
        let bare = tempfile::tempdir().unwrap();
        git_ok(bare.path(), &["init", "--bare", "-b", "main"]);
        git_ok(
            repo.path(),
            &["remote", "add", "origin", bare.path().to_str().unwrap()],
        );
        git_ok(repo.path(), &["push", "origin", "main"]);
        // The dirt that used to abort the refresh.
        std::fs::write(repo.path().join("tracked.txt"), "dirty\n").unwrap();

        let outcome = run_project_refresh(
            repo.path(),
            Some("main".to_string()),
            &crate::checkout_move::CheckoutMoveGuard::default(),
            Some(std::time::Duration::from_secs(60)),
        )
        .expect("a dirty checkout must not block the refresh");
        match outcome {
            PullOutcome::Pulled { current_branch } => {
                assert_eq!(current_branch.as_deref(), Some("main"));
            }
            other @ PullOutcome::NoOrigin { .. } => panic!("expected Pulled, got {other:?}"),
        }
    }

    /// A local-only repo is "nothing to pull", not a failure, and the current
    /// branch is still re-read so the sidebar stays fresh.
    #[test]
    fn project_refresh_without_origin_is_nothing_to_pull_info() {
        let repo = refresh_test_repo();

        let outcome = run_project_refresh(
            repo.path(),
            Some("main".to_string()),
            &crate::checkout_move::CheckoutMoveGuard::default(),
            Some(std::time::Duration::from_secs(60)),
        )
        .expect("no origin must not be an error");
        match &outcome {
            PullOutcome::NoOrigin { current_branch } => {
                assert_eq!(current_branch.as_deref(), Some("main"));
            }
            other @ PullOutcome::Pulled { .. } => panic!("expected NoOrigin, got {other:?}"),
        }

        // The op maps the no-origin outcome to an INFO final.
        let op = project_refresh_status_op("Refreshing...".to_string().into(), "demo");
        let resolved = op.resolve(&Ok(outcome));
        match resolved.outcome {
            crate::engine::Final::Message { tone, text, .. } => {
                assert_eq!(tone, StatusTone::Info);
                assert!(text.contains("no origin remote"), "got: {text}");
            }
            other @ crate::engine::Final::Clear => {
                panic!("expected a message final, got {other:?}")
            }
        }
    }

    /// Catches both the old error tone and over-softening to info: a failed
    /// refresh resolves as a WARNING that says the project continues from the
    /// local branch state.
    #[test]
    fn project_refresh_pull_failure_resolves_warning_not_error() {
        let repo = refresh_test_repo();
        git_ok(
            repo.path(),
            &["remote", "add", "origin", "/nonexistent/dux-test-origin"],
        );

        let result = run_project_refresh(
            repo.path(),
            Some("main".to_string()),
            &crate::checkout_move::CheckoutMoveGuard::default(),
            Some(std::time::Duration::from_secs(60)),
        );
        assert!(result.is_err(), "the pull must fail");

        let op = project_refresh_status_op("Refreshing...".to_string().into(), "demo");
        let resolved = op.resolve(&result);
        match resolved.outcome {
            crate::engine::Final::Message { tone, text, .. } => {
                assert_eq!(tone, StatusTone::Warning, "got: {text}");
                assert!(
                    text.contains("Continuing from the local branch state"),
                    "got: {text}"
                );
            }
            other @ crate::engine::Final::Clear => {
                panic!("expected a message final, got {other:?}")
            }
        }
    }

    #[test]
    fn saved_env_vars_message_counts_the_variables() {
        assert_eq!(
            saved_global_env_message(1),
            "Saved 1 global environment variable. New agents and terminals will receive it \
             unless a project overrides the same key."
        );
        assert_eq!(
            saved_global_env_message(3),
            "Saved 3 global environment variables. New agents and terminals will receive them \
             unless a project overrides the same key."
        );
    }

    #[test]
    fn saved_macros_message_counts_the_macros() {
        assert_eq!(saved_macros_message(1), "Saved 1 macro.");
        assert_eq!(saved_macros_message(3), "Saved 3 macros.");
    }
}
