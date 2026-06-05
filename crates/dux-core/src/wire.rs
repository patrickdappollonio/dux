//! Transport-agnostic command intake. A web client sends `{command, args}` JSON;
//! it deserializes into `WireCommand`, is reconstructed into the engine's
//! `Command` (looking up domain objects by id server-side), and dispatched
//! through the same `Engine::apply` the TUI uses. The result is downsampled to a
//! wire-safe `WireCommandOutcome` (the full `EventReaction` is engine-internal
//! and view-coupled; web clients re-fetch `view_model()` for fresh state).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::engine::{
    AgentLaunchFailedOutcome, BeginDeleteSessionOutcome, Command, Engine, EventReaction,
    FinishDeleteSessionOutcome, StatusUpdate, WorktreeRemoval,
};
use crate::model::{Project, ProjectBranchStatus, ProviderKind};
use crate::statusline::StatusTone;
use crate::worker::{
    AgentLaunchKind, CreateAgentRequest, NonDefaultBranchAction, ProjectPersistenceAction,
    PullTarget,
};

/// A command as received from a generic transport (e.g. the web WebSocket).
/// `#[serde(tag = "command", content = "args")]` matches the `{ "command": "...",
/// "args": { ... } }` envelope.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "command", content = "args", rename_all = "snake_case")]
pub enum WireCommand {
    StageFile {
        session_id: String,
        path: String,
    },
    UnstageFile {
        session_id: String,
        path: String,
    },
    /// Discard a single file's working-tree changes. The wire layer derives the
    /// destructive distinction (tracked → restore from HEAD vs untracked →
    /// delete the file) SERVER-SIDE from the worktree's live git status, and
    /// rejects the command when the file is currently staged — mirroring the
    /// TUI, which only allows discarding unstaged files.
    DiscardFile {
        session_id: String,
        path: String,
    },
    CommitChanges {
        session_id: String,
        message: String,
    },
    Push {
        session_id: String,
    },
    Pull {
        session_id: String,
    },
    /// Pull (refresh) a PROJECT's source checkout from remote, mirroring the
    /// TUI's `refresh_selected_project`. Unlike [`WireCommand::Pull`] (which
    /// targets a session's worktree), this resolves the project by id and runs
    /// `Command::Pull` against the project's source checkout path with
    /// `PullTarget::Project`. The busy/already-running copy and the leading
    /// branch source match the TUI byte-for-byte.
    PullProject {
        project_id: String,
    },
    GenerateCommitMessage {
        session_id: String,
    },
    ToggleAgentAutoReopen {
        session_id: String,
        enabled: bool,
    },
    DeleteTerminal {
        terminal_id: String,
    },
    DeleteSession {
        session_id: String,
        delete_worktree: bool,
    },
    PersistGlobalEnv {
        env: BTreeMap<String, String>,
    },
    UpdateProjectProvider {
        project_id: String,
        provider: Option<String>,
    },
    UpdateProjectAutoReopen {
        project_id: String,
        auto_reopen_agents: Option<bool>,
    },
    UpdateProjectStartupCommand {
        project_id: String,
        startup_command: Option<String>,
    },
    UpdateProjectEnv {
        project_id: String,
        env: BTreeMap<String, String>,
    },
    /// Re-read `config.toml` from disk and apply it to the running engine.
    ///
    /// Modeled as an empty struct variant (not a unit variant) so it deserializes
    /// from both `{"command":"reload_config"}` and `{"command":"reload_config",
    /// "args":{}}`. The frontend's generic command envelope always carries an
    /// `args` object, and serde's `content="args"` tagging rejects a map for a
    /// true unit variant — an empty struct variant accepts both forms.
    ReloadConfig {},
    /// Overwrite `config.toml` from the current in-memory config. Empty struct
    /// variant for the same reason as [`WireCommand::ReloadConfig`].
    RecoverConfig {},
    /// Register an existing git repository on the server as a project. `name`
    /// may be empty to derive the display name from the path's basename.
    AddProject {
        path: String,
        name: String,
    },
    /// Remove a project from the workspace by id (does not touch its checkout).
    RemoveProject {
        project_id: String,
    },
    /// Create a new agent in a project. `name` may be empty to auto-generate a
    /// branch name; a non-empty name becomes the custom branch/agent name.
    CreateAgent {
        project_id: String,
        name: String,
    },
    /// Fork an existing agent session into a fresh worktree branched from the
    /// source session's branch. `name` follows the same rules as `CreateAgent`:
    /// empty auto-generates a branch name (server pet-name path), a non-empty
    /// name becomes the custom branch/agent name. Mirrors the TUI's
    /// `fork_selected_session`, which builds a `CreateAgentRequest::ForkSession`.
    ForkSession {
        session_id: String,
        name: String,
    },
    /// Rename an agent session's display title. `title` is trimmed; an empty
    /// title clears the custom name back to `None`, so the row reverts to
    /// showing the branch name. A non-empty title is validated with the same
    /// agent-name rules the TUI rename enforces. This is title-only: unlike the
    /// TUI prompt (which can also rename the git branch via a checkbox), the web
    /// rename never touches the branch — see `rename_session` for the rationale.
    RenameSession {
        session_id: String,
        title: String,
    },
    /// Reconnect (relaunch) an agent session's provider. `force == false`
    /// resumes the prior conversation when the provider supports it (the
    /// TUI's `reconnect_selected_session`); `force == true` always starts a
    /// fresh session with no resume args and first tears down any running
    /// provider (the TUI's `force_reconnect_agent`).
    ReconnectSession {
        session_id: String,
        force: bool,
    },
    /// Persist a custom display order for a project's agent sessions.
    /// `session_ids` must list exactly that project's sessions (the engine
    /// validates this strictly and errors otherwise). Drives the web UI's
    /// drag-and-drop reordering.
    ReorderSessions {
        project_id: String,
        session_ids: Vec<String>,
    },
    /// Persist a custom display order for the workspace's projects.
    /// `project_ids` must list exactly the known projects.
    ReorderProjects {
        project_ids: Vec<String>,
    },
    /// Swap which CLI a session uses, mirroring the TUI palette's
    /// `change-agent-provider`. `provider` is validated server-side against the
    /// engine's configured provider list (the same source as the ViewModel's
    /// `available_providers`) — the client's choice is never trusted.
    ///
    /// Like the TUI, this does NOT kill or relaunch a running agent: it persists
    /// the new provider for the NEXT launch and, when a provider is still
    /// running on the session's PTY, pins the previously-running one so labels
    /// stay truthful until the user reconnects. Selecting the current provider
    /// is a no-op.
    ChangeAgentProvider {
        session_id: String,
        provider: String,
    },
    /// Switch a project's SOURCE checkout back to its default branch, mirroring
    /// the TUI's `checkout-project-default-branch`.
    ///
    /// The TUI runs this in two worker hops (an inspection job, then — only for
    /// the `Known` default-branch case — a `git switch` job), chained by an
    /// engine reaction the web loop does not act on. So the web does the
    /// inspection AND the checkout SYNCHRONOUSLY here (the engine loop already
    /// shells out to git synchronously for `PullProject`/discard), reproducing
    /// the TUI's four inspection outcomes byte-for-byte:
    ///   - default branch known and differs → `git switch`, then info.
    ///   - heuristic (origin/HEAD missing, on a non-main/master branch) → error.
    ///   - already on the leading branch → info, no checkout.
    ///   - inspection failed → error.
    ///
    /// The TUI does not confirm (it is a deliberate palette/keybinding action);
    /// the web confirms in the frontend dialog before sending this, since a ⋯
    /// menu click is a lighter gesture and the checkout moves HEAD.
    CheckoutProjectDefaultBranch {
        project_id: String,
    },
}

/// A status-line update in wire-safe form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WireStatus {
    /// "info" | "busy" | "warning" | "error"
    pub tone: String,
    pub message: String,
}

impl WireStatus {
    /// Construct a wire status directly (for non-reaction sources like PTY-exit notices).
    pub fn new(tone: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            tone: tone.into(),
            message: message.into(),
        }
    }

    fn from_update(update: &StatusUpdate) -> Self {
        let tone = match update.tone {
            StatusTone::Info => "info",
            StatusTone::Busy => "busy",
            StatusTone::Warning => "warning",
            StatusTone::Error => "error",
        };
        Self {
            tone: tone.to_string(),
            message: update.message.clone(),
        }
    }
}

/// What the client learns synchronously from applying a command. Fresh domain
/// state arrives separately via `view_model()`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct WireCommandOutcome {
    pub status: Option<WireStatus>,
}

fn wire_status_from_reaction(reaction: &EventReaction) -> Option<WireStatus> {
    match reaction {
        EventReaction::Status(update) => Some(WireStatus::from_update(update)),
        EventReaction::Multi(items) => items.iter().find_map(wire_status_from_reaction),
        EventReaction::DeleteTerminalView(view) => view
            .label
            .as_ref()
            .map(|l| WireStatus::new("info", format!("Closed terminal \"{l}\"."))),
        _ => None,
    }
}

/// Map an `EventReaction` to the user-facing status events it should emit on the
/// async status stream. Unlike `wire_status_from_reaction` (single value, for a
/// command's synchronous result), this flattens `Multi` and surfaces launch
/// failures, so background completions and failures reach web clients. The
/// messages mirror the TUI's `apply_agent_launch_failed_view` wording.
pub fn wire_statuses_from_reaction(reaction: &EventReaction) -> Vec<WireStatus> {
    match reaction {
        EventReaction::Status(update) => vec![WireStatus::from_update(update)],
        EventReaction::Multi(items) => items.iter().flat_map(wire_statuses_from_reaction).collect(),
        EventReaction::AgentLaunchFailedView(outcome) => match outcome.as_ref() {
            AgentLaunchFailedOutcome::Create { message } => {
                vec![WireStatus::new("error", message.clone())]
            }
            AgentLaunchFailedOutcome::Reconnect {
                branch_name,
                message,
            } => vec![WireStatus::new(
                "error",
                format!("Reconnect failed for agent \"{branch_name}\": {message}"),
            )],
            AgentLaunchFailedOutcome::ForceReconnect {
                branch_name,
                message,
            } => vec![WireStatus::new(
                "error",
                format!("Fresh restart failed for agent \"{branch_name}\": {message}"),
            )],
            AgentLaunchFailedOutcome::StartupAutoReopen {
                branch_name,
                message,
            } => vec![WireStatus::new(
                "warning",
                format!("Couldn't auto-reopen agent \"{branch_name}\": {message}"),
            )],
            AgentLaunchFailedOutcome::ResumeFallback => vec![],
        },
        EventReaction::DeleteTerminalView(view) => view
            .label
            .as_ref()
            .map(|l| WireStatus::new("info", format!("Closed terminal \"{l}\".")))
            .into_iter()
            .collect(),
        _ => vec![],
    }
}

/// User-facing message for a completed session deletion, varying by what
/// happened to the worktree.
pub fn delete_session_status_message(
    outcome: &FinishDeleteSessionOutcome,
    removal: &WorktreeRemoval,
) -> String {
    let name = outcome
        .session
        .title
        .clone()
        .unwrap_or_else(|| outcome.session.branch_name.clone());
    match removal {
        WorktreeRemoval::Performed { .. } => {
            format!("Deleted agent \"{name}\" and removed its worktree.")
        }
        WorktreeRemoval::PreservedShared => {
            format!("Deleted agent \"{name}\". Worktree kept (shared with other agents).")
        }
        WorktreeRemoval::SkippedForSiblings => {
            format!("Deleted agent \"{name}\". Worktree kept (still used by other agents).")
        }
        WorktreeRemoval::PreservedOrphan => {
            format!("Deleted agent \"{name}\". Worktree left on disk.")
        }
    }
}

/// Classify a discard request against the worktree's LIVE git status and return
/// whether the target file is untracked. Discard is destructive (it deletes
/// untracked files and restores tracked ones from HEAD), so the tracked vs
/// untracked distinction is derived server-side from `git status` rather than
/// trusted from the client. Mirrors the TUI's guard: a file that is currently
/// STAGED cannot be discarded (the TUI tells the user to unstage it first), and
/// a file with no working-tree change has nothing to discard.
fn discard_classify(worktree_path: &std::path::Path, path: &str) -> anyhow::Result<bool> {
    let (staged, unstaged) = crate::git::changed_files(worktree_path)?;
    // Reject when the file is staged. The TUI surfaces "Unstage the file first
    // to discard changes." for the same case; mirror that wording.
    if staged.iter().any(|f| f.path == path) && !unstaged.iter().any(|f| f.path == path) {
        anyhow::bail!("Unstage the file first to discard changes.");
    }
    match unstaged.iter().find(|f| f.path == path) {
        Some(file) => Ok(file.status == "?"),
        None => anyhow::bail!("No unstaged changes to discard for \"{path}\"."),
    }
}

impl Engine {
    /// Reconstruct and dispatch a wire command, returning a wire-safe outcome.
    pub fn apply_wire(&mut self, command: WireCommand) -> anyhow::Result<WireCommandOutcome> {
        // Rename and Reconnect need `&mut self` and don't map cleanly onto a
        // single `Command`, so they're handled directly here rather than via
        // `wire_to_command`/`apply`.
        match command {
            WireCommand::RenameSession { session_id, title } => {
                let status = self.rename_session(&session_id, &title)?;
                return Ok(WireCommandOutcome {
                    status: Some(status),
                });
            }
            WireCommand::ReconnectSession { session_id, force } => {
                let status = self.reconnect_session(&session_id, force)?;
                return Ok(WireCommandOutcome { status });
            }
            WireCommand::CheckoutProjectDefaultBranch { project_id } => {
                let status = self.checkout_project_default_branch(&project_id)?;
                return Ok(WireCommandOutcome {
                    status: Some(status),
                });
            }
            WireCommand::ChangeAgentProvider {
                session_id,
                provider,
            } => {
                let status = self.change_agent_provider_wire(&session_id, &provider)?;
                return Ok(WireCommandOutcome {
                    status: Some(status),
                });
            }
            _ => {}
        }
        let core = self.wire_to_command(command)?;
        let reaction = self.apply(core)?;
        let mut status = wire_status_from_reaction(&reaction);
        if status.is_none() {
            status = self.drive_delete_followup(&reaction).into_iter().next();
        }
        Ok(WireCommandOutcome { status })
    }

    /// Rename an agent session's display title, mirroring the title half of the
    /// TUI's `apply_rename_session`. The custom `title` is trimmed; a non-empty
    /// title is validated with the same `is_valid_agent_name` backstop the TUI
    /// enforces and stored as `Some(title)`; an empty title clears it back to
    /// `None` so the row reverts to the branch name.
    ///
    /// Deliberate deviation from the TUI prompt: that prompt also renames the
    /// git branch by default (a `rename_branch` checkbox) and rejects an empty
    /// name outright. The web rename is title-only — it never touches the git
    /// branch — so clearing the title is the only way to revert to the branch
    /// name, which is why an empty title clears rather than errors here.
    fn rename_session(&mut self, session_id: &str, title: &str) -> anyhow::Result<WireStatus> {
        let trimmed = title.trim();
        let new_title = if trimmed.is_empty() {
            None
        } else {
            if !crate::git::is_valid_agent_name(trimmed) {
                anyhow::bail!(
                    "Invalid agent name \"{trimmed}\". Use only letters, digits, dashes, \
                     underscores and slashes; it must start with a letter or digit, must \
                     not contain \"//\", and must not end with \"/\"."
                );
            }
            Some(trimmed.to_string())
        };
        let session = self
            .sessions
            .iter_mut()
            .find(|s| s.id == session_id)
            .ok_or_else(|| anyhow::anyhow!("unknown session: {session_id}"))?;
        session.title = new_title.clone();
        session.updated_at = chrono::Utc::now();
        // Re-borrow immutably to persist (the mutable borrow above has ended).
        if let Some(session) = self.sessions.iter().find(|s| s.id == session_id) {
            self.session_store.upsert_session(session)?;
        }
        let message = match &new_title {
            Some(name) => format!("Renamed agent to \"{name}\"."),
            None => {
                let branch = self
                    .sessions
                    .iter()
                    .find(|s| s.id == session_id)
                    .map(|s| s.branch_name.clone())
                    .unwrap_or_default();
                format!("Cleared the custom name. Agent shows its branch name \"{branch}\" again.")
            }
        };
        Ok(WireStatus::new("info", message))
    }

    /// Swap which provider a session uses, mirroring the TUI's
    /// `apply_change_agent_provider`. The engine half (persist + pin) lives in
    /// [`Engine::change_agent_provider`]; this wire wrapper validates the
    /// provider against the configured provider list (the ViewModel's
    /// `available_providers` source — never trusting the client), handles the
    /// no-op "already uses this provider" case, and formats the status message.
    ///
    /// Deliberate substitution: the TUI's messages reference the rebindable
    /// `reconnect-agent` keybinding label ("press {key} to relaunch"); the web
    /// has no keybindings, so it points the user at the agent's Reconnect action
    /// instead, while keeping the rest of the wording byte-identical.
    fn change_agent_provider_wire(
        &mut self,
        session_id: &str,
        provider: &str,
    ) -> anyhow::Result<WireStatus> {
        let session = self
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .ok_or_else(|| anyhow::anyhow!("unknown session: {session_id}"))?;
        let label = session
            .title
            .clone()
            .unwrap_or_else(|| session.branch_name.clone());
        let current = session.provider.clone();

        // Validate against the configured provider list — the same source the
        // ViewModel's `available_providers` is built from — so a forged or
        // stale provider name from the client is rejected with actionable copy.
        if !self.config.providers.commands.contains_key(provider) {
            anyhow::bail!(
                "Provider \"{provider}\" is not configured. Pick one of the configured providers."
            );
        }
        let provider = ProviderKind::new(provider);

        // No-op when the session already uses the chosen provider (mirrors the
        // TUI's `is_current` short-circuit, which knows the display label).
        if provider == current {
            return Ok(WireStatus::new(
                "info",
                format!(
                    "Agent \"{}\" already uses {}. Pick another provider to swap.",
                    label,
                    provider.as_str(),
                ),
            ));
        }

        let outcome = self.change_agent_provider(session_id, provider.clone())?;

        if outcome.running {
            Ok(WireStatus::new(
                "warning",
                format!(
                    "Worktree \"{}\" is set to {}, but the {} agent is still running. Exit it and reconnect the agent to relaunch with {}.",
                    label,
                    provider.as_str(),
                    outcome.previous.as_str(),
                    provider.as_str(),
                ),
            ))
        } else {
            let resume_note = if outcome.resume_available {
                " dux will resume its prior session on this worktree."
            } else {
                " This provider hasn't run on this worktree yet, so it'll start a fresh session."
            };
            Ok(WireStatus::new(
                "info",
                format!(
                    "Worktree \"{}\" will use {} next launch. Reconnect the agent to start it.{}",
                    label,
                    provider.as_str(),
                    resume_note,
                ),
            ))
        }
    }

    /// Reconnect (relaunch) an agent session's provider. Mirrors the TUI's
    /// `reconnect_selected_session` (`force == false`) and `force_reconnect_agent`
    /// (`force == true`):
    ///
    /// - Both require the session to exist and its worktree to still be present.
    /// - Normal reconnect REFUSES while a provider is already connected
    ///   (matching the TUI's "already connected" early-return); force reconnect
    ///   first tears down any running provider + pins + activity + resume
    ///   candidate, then starts fresh with no resume args.
    /// - Normal reconnect resumes the prior conversation when the provider
    ///   supports it (`should_resume_session`); force never resumes.
    ///
    /// Deliberate substitution: the TUI sources the PTY size from view state
    /// (`last_pty_size`); the web has no such state, so it uses the same default
    /// `(24, 80)` the subscribe-launch path already uses (`launch_agent`). The
    /// focused TerminalPane re-attaches via the existing subscribe machinery
    /// once the new provider comes up.
    ///
    /// Returns the busy status to surface synchronously (matching the TUI's
    /// `set_busy` after a successful dispatch), or the launch view's status when
    /// the dispatch was refused (e.g. a launch already in flight).
    fn reconnect_session(
        &mut self,
        session_id: &str,
        force: bool,
    ) -> anyhow::Result<Option<WireStatus>> {
        let session = self
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown session: {session_id}"))?;

        // Check order mirrors the TUI exactly: normal reconnect tests
        // "already connected" first (`reconnect_selected_session`), then the
        // worktree; force reconnect has no connected-check (it kills the
        // provider) and only guards the worktree (`force_reconnect_agent`).
        if !force && self.providers.contains_key(&session.id) {
            return Ok(Some(WireStatus::new(
                "info",
                format!("Agent \"{}\" is already connected.", session.branch_name),
            )));
        }

        if !std::path::Path::new(&session.worktree_path).exists() {
            anyhow::bail!(
                "Worktree for agent \"{}\" no longer exists. Delete and re-create the agent.",
                session.branch_name
            );
        }

        if force {
            // Kill the existing provider and clear all resume state so the
            // relaunch starts genuinely fresh (mirrors `force_reconnect_agent`).
            self.providers.remove(&session.id);
            self.running_provider_pins.remove(&session.id);
            self.pty_activity.remove(&session.id);
            self.resume_fallback_candidates.remove(&session.id);
        }

        // Detach any other session holding the same worktree's live PTY. The
        // engine method marks it Detached and clears provider/pin/candidate;
        // mirror the TUI App wrapper by also clearing its `pty_activity` entry.
        let detached_label = self
            .detach_conflicting_worktree_session(&session.worktree_path, &session.id)
            .map(|detached| {
                self.pty_activity.remove(&detached.id);
                detached.label
            });

        let use_resume = if force {
            false
        } else {
            self.should_resume_session(&session)
        };
        let proj_name = self.project_name_for_session(&session);
        let mut msg = if use_resume {
            format!(
                "Resumed {} agent \"{}\" in project \"{}\".",
                session.provider.as_str(),
                session.branch_name,
                proj_name
            )
        } else {
            format!(
                "Started fresh {} session for agent \"{}\" in project \"{}\". Use /sessions inside the agent to restore a prior conversation.",
                session.provider.as_str(),
                session.branch_name,
                proj_name
            )
        };
        if let Some(detached) = &detached_label {
            msg.push_str(&format!(
                " Agent \"{}\" was detached to avoid worktree conflicts.",
                detached,
            ));
        }
        if let Some(project) = self.projects.iter().find(|p| p.id == session.project_id)
            && project.default_provider != session.provider
        {
            let provider_label = if self.project_uses_explicit_default_provider(&project.id) {
                "current project provider"
            } else {
                "current global default provider"
            };
            msg.push_str(&format!(
                " Note: this agent uses {}. Your {provider_label} is {}.",
                session.provider.as_str(),
                project.default_provider.as_str(),
            ));
        }

        let branch_name = session.branch_name.clone();
        let kind = if force {
            AgentLaunchKind::ForceReconnect {
                status_message: msg,
            }
        } else {
            AgentLaunchKind::Reconnect {
                status_message: msg,
            }
        };
        // The TUI sources `last_pty_size` from view state; the web has none, so
        // reuse the subscribe-launch default `(24, 80)` (rows, cols).
        let request = self.build_agent_launch_request(session, use_resume, (24, 80), kind);
        let reaction = self.apply(Command::DispatchAgentLaunch {
            request: Box::new(request),
        })?;
        // `DispatchAgentLaunch` returns a `DispatchAgentLaunchView`, which
        // `wire_status_from_reaction` doesn't surface. Mirror the TUI: on a
        // successful dispatch (`launched`) show the busy status; otherwise
        // surface the view's own status (e.g. "already launching").
        let busy = if force {
            format!("Starting fresh agent \"{branch_name}\"...")
        } else {
            format!("Launching agent \"{branch_name}\"...")
        };
        match reaction {
            EventReaction::DispatchAgentLaunchView(view) => {
                if view.launched {
                    Ok(Some(WireStatus::new("busy", busy)))
                } else {
                    Ok(view.status.as_ref().map(WireStatus::from_update))
                }
            }
            other => Ok(wire_status_from_reaction(&other)),
        }
    }

    /// Switch a project's source checkout back to its default branch, mirroring
    /// the TUI's `checkout_selected_project_default_branch` (sessions.rs).
    ///
    /// This kicks off the TUI's two-worker chain rather than doing the work
    /// inline: `git switch` rewrites the working tree (seconds on large repos,
    /// blocks indefinitely on a held `index.lock`), and the web engine loop
    /// thread also drives every ViewModel push and PTY route, so a synchronous
    /// checkout would freeze every connected browser. The CLAUDE.md workers
    /// tenet forbids that.
    ///
    /// Worker 1 (`run_checkout_project_default_branch_inspection_job`) inspects
    /// the branch off-thread and posts `CheckoutProjectDefaultBranchInspected`.
    /// `process_worker_event` turns that into either a `Status` (heuristic /
    /// already-leading / inspection-error — surfaced by the actor's existing
    /// `wire_statuses_from_reaction` drain) or a
    /// `DispatchProjectDefaultBranchCheckout` reaction for the Known case, which
    /// `drive_checkout_followup` picks up to spawn worker 2 (the actual switch).
    ///
    /// Returns the busy status to surface synchronously (matching the TUI's
    /// `set_busy`). `Err` is reserved for the cheap project lookup / missing
    /// path guards (mirroring `PullProject`), so every real outcome comes back
    /// later as an async status.
    fn checkout_project_default_branch(&mut self, project_id: &str) -> anyhow::Result<WireStatus> {
        // Mirror the TUI's `checkout_selected_project_default_branch` guards:
        // resolve the project and refuse when its checkout path is missing.
        let project = self
            .projects
            .iter()
            .find(|p| p.id == project_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("unknown project: {project_id}"))?;
        if project.path_missing {
            anyhow::bail!(
                "Cannot check out default branch: path not found for \"{}\"",
                project.name
            );
        }

        // Spawn worker 1 exactly as the TUI does, with the same busy message.
        let busy = format!(
            "Checking the default branch for project \"{}\"...",
            project.name
        );
        let worker_tx = self.worker_tx.clone();
        std::thread::spawn(move || {
            crate::project_browser::run_checkout_project_default_branch_inspection_job(
                project, worker_tx,
            );
        });
        Ok(WireStatus::new("busy", busy))
    }

    /// Drive a checkout-related reaction to completion, returning user-facing
    /// statuses. Called from the web engine actor's worker-event drain alongside
    /// `drive_delete_followup`: when worker 1's inspection produces a
    /// `DispatchProjectDefaultBranchCheckout` (the Known default-branch case),
    /// spawn worker 2 (`run_add_project_checkout_job`) to run `git switch`
    /// off-thread, mirroring the TUI's `dispatch_non_default_branch_checkout`.
    /// Worker 2's completion posts `NonDefaultBranchCheckoutCompleted`, whose
    /// `process_worker_event` arm returns the success/failure `Status` that the
    /// actor's existing `wire_statuses_from_reaction` drain broadcasts. Other
    /// reactions return `[]`.
    pub fn drive_checkout_followup(&mut self, reaction: &EventReaction) -> Vec<WireStatus> {
        match reaction {
            EventReaction::DispatchProjectDefaultBranchCheckout {
                project,
                default_branch,
            } => {
                let action = NonDefaultBranchAction::CheckoutProjectDefault {
                    project: project.clone(),
                };
                let target_branch = default_branch.clone();
                let worker_tx = self.worker_tx.clone();
                std::thread::spawn(move || {
                    crate::project_browser::run_add_project_checkout_job(
                        action,
                        target_branch,
                        worker_tx,
                    );
                });
                vec![]
            }
            _ => vec![],
        }
    }

    /// Drive a delete-related reaction to completion, returning user-facing
    /// statuses. Used by `apply_wire` (synchronous Begin/Inline) and by the web
    /// engine actor's worker-event drain (async worktree-removal completion), so
    /// deletions finish without a view layer. Non-delete reactions return `[]`.
    pub fn drive_delete_followup(&mut self, reaction: &EventReaction) -> Vec<WireStatus> {
        match reaction {
            EventReaction::BeginDeleteSessionView(view) => match &view.outcome {
                BeginDeleteSessionOutcome::AlreadyInFlight => vec![WireStatus::new(
                    "error",
                    "Deletion already in progress for this agent. Wait for it to finish.",
                )],
                BeginDeleteSessionOutcome::NotFound => vec![],
                BeginDeleteSessionOutcome::AsyncStarted { busy_message } => {
                    vec![WireStatus::new("busy", busy_message.clone())]
                }
                BeginDeleteSessionOutcome::Inline { removal } => {
                    let removal = *removal;
                    self.finish_delete_and_status(&view.session_id, removal)
                }
            },
            EventReaction::WorktreeRemoveSucceeded {
                session_id,
                branch_already_deleted,
                ..
            } => {
                if self.sessions.iter().any(|s| s.id == *session_id) {
                    self.finish_delete_and_status(
                        session_id,
                        WorktreeRemoval::Performed {
                            branch_already_deleted: *branch_already_deleted,
                        },
                    )
                } else {
                    vec![]
                }
            }
            EventReaction::WorktreeRemoveFailed { message, .. } => {
                vec![WireStatus::new(
                    "error",
                    format!("Worktree delete failed: {message}"),
                )]
            }
            _ => vec![],
        }
    }

    fn finish_delete_and_status(
        &mut self,
        session_id: &str,
        removal: WorktreeRemoval,
    ) -> Vec<WireStatus> {
        match self.apply(Command::FinishDeleteSession {
            session_id: session_id.to_string(),
            removal,
            update_status: true,
        }) {
            Ok(EventReaction::FinishDeleteSessionView(view)) => vec![WireStatus::new(
                "info",
                delete_session_status_message(&view.outcome, &view.removal),
            )],
            Ok(_) => vec![],
            Err(e) => vec![WireStatus::new(
                "error",
                format!("Session cleanup failed: {e:#}"),
            )],
        }
    }

    fn wire_to_command(&self, command: WireCommand) -> anyhow::Result<Command> {
        Ok(match command {
            WireCommand::StageFile { session_id, path } => Command::StageFile {
                worktree_path: self.session_worktree(&session_id)?,
                path,
            },
            WireCommand::UnstageFile { session_id, path } => Command::UnstageFile {
                worktree_path: self.session_worktree(&session_id)?,
                path,
            },
            WireCommand::DiscardFile { session_id, path } => {
                let worktree_path = self.session_worktree(&session_id)?;
                let is_untracked = discard_classify(&worktree_path, &path)?;
                Command::DiscardFile {
                    worktree_path,
                    path,
                    is_untracked,
                }
            }
            WireCommand::CommitChanges {
                session_id,
                message,
            } => Command::CommitChanges {
                worktree_path: self.session_worktree(&session_id)?,
                message,
                success_message: "Changes committed successfully.".to_string(),
            },
            WireCommand::Push { session_id } => Command::Push {
                worktree_path: self.session_worktree(&session_id)?,
            },
            WireCommand::Pull { session_id } => Command::Pull {
                repo_path: self.session_worktree(&session_id)?,
                target: PullTarget::Session,
                busy_message: "Pulling latest changes from remote\u{2026}".to_string(),
                already_running_message:
                    "Pull already in progress for this worktree. Wait for the current pull to finish."
                        .to_string(),
            },
            WireCommand::PullProject { project_id } => {
                // Mirror the TUI's `refresh_selected_project`: resolve the
                // project, refuse when its checkout path is missing, then build
                // `Command::Pull` with `PullTarget::Project` from the project's
                // SOURCE checkout path (not a worktree) and the persisted leading
                // branch — byte-for-byte the same payload and message strings.
                let project = self
                    .projects
                    .iter()
                    .find(|p| p.id == project_id)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("unknown project: {project_id}"))?;
                if project.path_missing {
                    anyhow::bail!("Cannot refresh: path not found for \"{}\"", project.name);
                }
                Command::Pull {
                    repo_path: PathBuf::from(&project.path),
                    target: PullTarget::Project {
                        project_id: project.id,
                        project_name: project.name.clone(),
                        leading_branch: project.leading_branch.clone(),
                    },
                    busy_message: format!(
                        "Refreshing project \"{}\" from remote\u{2026}",
                        project.name
                    ),
                    already_running_message: format!(
                        "Project refresh already in progress for \"{}\". Wait for the current pull to finish.",
                        project.name,
                    ),
                }
            }
            WireCommand::GenerateCommitMessage { session_id } => {
                Command::GenerateCommitMessage { session_id }
            }
            WireCommand::ToggleAgentAutoReopen {
                session_id,
                enabled,
            } => {
                let branch_name = self
                    .sessions
                    .iter()
                    .find(|s| s.id == session_id)
                    .ok_or_else(|| anyhow::anyhow!("unknown session: {session_id}"))?
                    .branch_name
                    .clone();
                Command::ToggleAgentAutoReopen {
                    session_id,
                    branch_name,
                    new_enabled: enabled,
                }
            }
            WireCommand::DeleteTerminal { terminal_id } => Command::DeleteTerminal { terminal_id },
            WireCommand::DeleteSession {
                session_id,
                delete_worktree,
            } => Command::BeginDeleteSession {
                session_id,
                delete_worktree,
            },
            WireCommand::PersistGlobalEnv { env } => Command::PersistGlobalEnv { env },
            WireCommand::UpdateProjectProvider {
                project_id,
                provider,
            } => {
                let project_name = self.project_name(&project_id)?;
                Command::PersistProject(Box::new(
                    ProjectPersistenceAction::UpdateDefaultProvider {
                        project_id,
                        project_name,
                        provider: provider.map(ProviderKind::new),
                        global_default: self.config.default_provider(),
                    },
                ))
            }
            WireCommand::UpdateProjectAutoReopen {
                project_id,
                auto_reopen_agents,
            } => {
                let project_name = self.project_name(&project_id)?;
                Command::PersistProject(Box::new(ProjectPersistenceAction::UpdateAutoReopen {
                    project_id,
                    project_name,
                    auto_reopen_agents,
                }))
            }
            WireCommand::UpdateProjectStartupCommand {
                project_id,
                startup_command,
            } => {
                let project_name = self.project_name(&project_id)?;
                Command::PersistProject(Box::new(
                    ProjectPersistenceAction::UpdateStartupCommand {
                        project_id,
                        project_name,
                        startup_command,
                    },
                ))
            }
            WireCommand::UpdateProjectEnv { project_id, env } => {
                let project_name = self.project_name(&project_id)?;
                Command::PersistProject(Box::new(ProjectPersistenceAction::UpdateEnv {
                    project_id,
                    project_name,
                    env,
                }))
            }
            WireCommand::ReloadConfig {} => Command::ReloadConfig,
            WireCommand::RecoverConfig {} => Command::RecoverConfig,
            WireCommand::AddProject { path, name } => {
                let validated = self
                    .validate_project_add_path(&path)
                    .map_err(|e| anyhow::anyhow!(e))?;
                let branch = crate::git::current_branch(&validated)?;
                let leading_branch =
                    crate::project_browser::leading_branch_for_project(&validated, &branch);
                let path_str = validated.to_string_lossy().to_string();
                let display_name = if name.trim().is_empty() {
                    validated
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("project")
                        .to_string()
                } else {
                    name.trim().to_string()
                };
                let project = Project {
                    id: uuid::Uuid::new_v4().to_string(),
                    name: display_name.clone(),
                    path: path_str,
                    explicit_default_provider: None,
                    default_provider: self.config.default_provider(),
                    leading_branch: Some(leading_branch),
                    auto_reopen_agents: None,
                    startup_command: None,
                    env: std::collections::BTreeMap::new(),
                    current_branch: branch,
                    branch_status: ProjectBranchStatus::Unknown,
                    path_missing: false,
                };
                let status_message =
                    format!("Added project \"{display_name}\" to the workspace.");
                Command::PersistProject(Box::new(ProjectPersistenceAction::Add {
                    project,
                    status_message,
                }))
            }
            WireCommand::RemoveProject { project_id } => {
                let project_name = self.project_name(&project_id)?;
                // Mirror the TUI guard: refuse to remove a project that still has
                // agents, otherwise their sessions would be orphaned (pointing at a
                // vanished project id). The user must delete the agents first.
                if self.sessions.iter().any(|s| s.project_id == project_id) {
                    anyhow::bail!(
                        "Delete this project's agents before removing \"{project_name}\"."
                    );
                }
                Command::PersistProject(Box::new(ProjectPersistenceAction::Remove {
                    project_name,
                    project_id,
                }))
            }
            WireCommand::CreateAgent { project_id, name } => {
                let project = self
                    .projects
                    .iter()
                    .find(|p| p.id == project_id)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("unknown project: {project_id}"))?;
                let trimmed = name.trim();
                let custom_name = if trimmed.is_empty() {
                    None
                } else {
                    // Backstop: the TUI prompt filters keystrokes so an invalid
                    // name can't be typed, but a raw wire client (e.g. the web
                    // UI before its own sanitizer, or a scripted client) can send
                    // anything. Reject names git would refuse as a ref before
                    // dispatching the create worker.
                    if !crate::git::is_valid_agent_name(trimmed) {
                        anyhow::bail!(
                            "Invalid agent name \"{trimmed}\". Use only letters, digits, dashes, \
                             underscores and slashes; it must start with a letter or digit, must \
                             not contain \"//\", and must not end with \"/\"."
                        );
                    }
                    Some(trimmed.to_string())
                };
                let request = CreateAgentRequest::NewProject {
                    project,
                    custom_name,
                    use_existing_branch: false,
                    pull_before_create: self.config.defaults.pull_before_creating_agent_by_default,
                };
                Command::DispatchCreateAgentRequest {
                    request: Box::new(request),
                    busy_message: "Creating a new agent\u{2026}".to_string(),
                    term_size: (80, 24),
                }
            }
            WireCommand::ForkSession { session_id, name } => {
                let source_session = self
                    .sessions
                    .iter()
                    .find(|s| s.id == session_id)
                    .cloned()
                    .ok_or_else(|| anyhow::anyhow!("unknown session: {session_id}"))?;
                let project = self
                    .projects
                    .iter()
                    .find(|p| p.id == source_session.project_id)
                    .cloned()
                    .ok_or_else(|| {
                        anyhow::anyhow!("unknown project: {}", source_session.project_id)
                    })?;
                // `source_label` mirrors the TUI's `session_label`: the custom
                // title if set, otherwise the branch name.
                let source_label = source_session
                    .title
                    .clone()
                    .unwrap_or_else(|| source_session.branch_name.clone());
                let trimmed = name.trim();
                // Unlike CreateAgent, a fork REQUIRES a name: the create-agent
                // worker rejects a `None` custom_name with "Forking an agent
                // requires choosing a name first.", and the TUI's name prompt
                // refuses an empty name for every request kind. Mirror the
                // prompt's rejection here so the failure is immediate and clear
                // rather than surfacing later from the worker.
                if trimmed.is_empty() {
                    anyhow::bail!("Agent name cannot be empty.");
                }
                // Same backstop as CreateAgent: reject names git would refuse as
                // a ref before dispatching the worker.
                if !crate::git::is_valid_agent_name(trimmed) {
                    anyhow::bail!(
                        "Invalid agent name \"{trimmed}\". Use only letters, digits, dashes, \
                         underscores and slashes; it must start with a letter or digit, must \
                         not contain \"//\", and must not end with \"/\"."
                    );
                }
                let name = trimmed.to_string();
                // Mirror the TUI's fork busy copy (input.rs NameNewAgent confirm).
                let busy_message = format!(
                    "Forking agent \"{source_label}\" as \"{name}\" by cloning its current \
                     worktree contents into a fresh session...",
                );
                let request = CreateAgentRequest::ForkSession {
                    project,
                    source_session: Box::new(source_session),
                    source_label,
                    custom_name: Some(name),
                };
                Command::DispatchCreateAgentRequest {
                    request: Box::new(request),
                    busy_message,
                    term_size: (80, 24),
                }
            }
            // Rename, Reconnect, CheckoutProjectDefaultBranch, and
            // ChangeAgentProvider are NOT reconstructible into a single
            // `Command` — all need `&mut self` (rename persists in place;
            // reconnect tears down provider state and surfaces a launch view's
            // status synchronously; checkout inspects and switches branches
            // synchronously; change-provider persists + pins in place).
            // `apply_wire` intercepts them before this immutable mapping;
            // reaching here means that interception broke.
            WireCommand::RenameSession { .. }
            | WireCommand::ReconnectSession { .. }
            | WireCommand::CheckoutProjectDefaultBranch { .. }
            | WireCommand::ChangeAgentProvider { .. } => {
                unreachable!(
                    "rename/reconnect/checkout-default-branch/change-provider are handled in apply_wire before wire_to_command"
                )
            }
            WireCommand::ReorderSessions {
                project_id,
                session_ids,
            } => Command::ReorderSessions {
                project_id,
                session_ids,
            },
            WireCommand::ReorderProjects { project_ids } => {
                Command::ReorderProjects { project_ids }
            }
        })
    }

    fn project_name(&self, project_id: &str) -> anyhow::Result<String> {
        self.projects
            .iter()
            .find(|p| p.id == project_id)
            .map(|p| p.name.clone())
            .ok_or_else(|| anyhow::anyhow!("unknown project: {project_id}"))
    }

    fn session_worktree(&self, session_id: &str) -> anyhow::Result<PathBuf> {
        let session = self
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .ok_or_else(|| anyhow::anyhow!("unknown session: {session_id}"))?;
        Ok(PathBuf::from(&session.worktree_path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::{sample_project, sample_session, test_engine};
    use crate::engine::{DeleteTerminalView, InFlightKey};
    use crate::model::AgentSession;
    use std::path::Path;

    #[test]
    fn wire_command_deserializes_from_json_envelope() {
        let json = r#"{"command":"stage_file","args":{"session_id":"s1","path":"a.txt"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::StageFile {
                session_id: "s1".to_string(),
                path: "a.txt".to_string()
            }
        );
    }

    #[test]
    fn wire_delete_terminal_deserializes() {
        let json = r#"{"command":"delete_terminal","args":{"terminal_id":"term-1"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::DeleteTerminal {
                terminal_id: "term-1".to_string()
            }
        );
    }

    #[test]
    fn wire_statuses_reports_closed_terminal() {
        let r = EventReaction::DeleteTerminalView(Box::new(DeleteTerminalView {
            terminal_id: "term-1".to_string(),
            label: Some("Terminal 1".to_string()),
        }));
        let s = wire_statuses_from_reaction(&r);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].tone, "info");
        assert!(s[0].message.contains("Closed terminal \"Terminal 1\""));
    }

    #[test]
    fn wire_to_command_resolves_worktree_from_session() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let cmd = engine
            .wire_to_command(WireCommand::StageFile {
                session_id: "s1".to_string(),
                path: "a.txt".to_string(),
            })
            .expect("reconstruct");
        match cmd {
            Command::StageFile {
                worktree_path,
                path,
            } => {
                assert_eq!(worktree_path, Path::new("/tmp/s1-worktree"));
                assert_eq!(path, "a.txt");
            }
            _ => panic!("expected Command::StageFile variant"),
        }
    }

    #[test]
    fn wire_discard_file_deserializes() {
        let json = r#"{"command":"discard_file","args":{"session_id":"s1","path":"a.txt"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::DiscardFile {
                session_id: "s1".to_string(),
                path: "a.txt".to_string()
            }
        );
    }

    /// Build a session whose worktree points at a real temp git repo so the
    /// discard classifier can run `git status` against it.
    fn session_in_repo(id: &str, repo: &Path) -> AgentSession {
        let mut s = sample_session(id, "p1", "feat");
        s.worktree_path = repo.to_string_lossy().into_owned();
        s
    }

    #[test]
    fn wire_to_command_discard_derives_untracked_for_untracked_file() {
        // init_repo leaves a.txt untracked (never committed).
        let repo = init_repo();
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(session_in_repo("s1", repo.path()));
        let cmd = engine
            .wire_to_command(WireCommand::DiscardFile {
                session_id: "s1".to_string(),
                path: "a.txt".to_string(),
            })
            .expect("reconstruct");
        match cmd {
            Command::DiscardFile {
                worktree_path,
                path,
                is_untracked,
            } => {
                assert_eq!(worktree_path, repo.path());
                assert_eq!(path, "a.txt");
                assert!(is_untracked, "untracked file must be classified untracked");
            }
            _ => panic!("expected Command::DiscardFile variant"),
        }
    }

    #[test]
    fn wire_to_command_discard_derives_tracked_for_modified_file() {
        // init_repo_with_commit commits a.txt; modify it so it has an unstaged
        // (tracked) change.
        let repo = init_repo_with_commit();
        std::fs::write(repo.path().join("a.txt"), "changed\n").expect("modify file");
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(session_in_repo("s1", repo.path()));
        let cmd = engine
            .wire_to_command(WireCommand::DiscardFile {
                session_id: "s1".to_string(),
                path: "a.txt".to_string(),
            })
            .expect("reconstruct");
        match cmd {
            Command::DiscardFile { is_untracked, .. } => {
                assert!(!is_untracked, "modified tracked file must not be untracked");
            }
            _ => panic!("expected Command::DiscardFile variant"),
        }
    }

    #[test]
    fn wire_to_command_discard_rejects_staged_file() {
        // Commit a.txt, modify it, then STAGE the modification: it is now purely
        // staged with no remaining working-tree change. The TUI blocks this.
        let repo = init_repo_with_commit();
        std::fs::write(repo.path().join("a.txt"), "staged change\n").expect("modify file");
        let ok = std::process::Command::new("git")
            .args(["add", "a.txt"])
            .current_dir(repo.path())
            .status()
            .expect("spawn git")
            .success();
        assert!(ok, "git add failed");
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(session_in_repo("s1", repo.path()));
        let err = engine
            .wire_to_command(WireCommand::DiscardFile {
                session_id: "s1".to_string(),
                path: "a.txt".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        // Mirrors the TUI's "Unstage the file first to discard changes." copy.
        assert!(
            err.to_string()
                .contains("Unstage the file first to discard changes."),
            "got: {err}"
        );
    }

    #[test]
    fn wire_to_command_discard_rejects_unchanged_file() {
        // a.txt is committed and clean — nothing to discard.
        let repo = init_repo_with_commit();
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(session_in_repo("s1", repo.path()));
        let err = engine
            .wire_to_command(WireCommand::DiscardFile {
                session_id: "s1".to_string(),
                path: "a.txt".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        assert!(
            err.to_string().contains("No unstaged changes to discard"),
            "got: {err}"
        );
    }

    #[test]
    fn wire_pull_deserializes() {
        let json = r#"{"command":"pull","args":{"session_id":"s1"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::Pull {
                session_id: "s1".to_string()
            }
        );
    }

    #[test]
    fn wire_to_command_pull_resolves_worktree() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let cmd = engine
            .wire_to_command(WireCommand::Pull {
                session_id: "s1".to_string(),
            })
            .expect("reconstruct");
        match cmd {
            Command::Pull {
                repo_path,
                target: PullTarget::Session,
                ..
            } => {
                assert_eq!(repo_path, Path::new("/tmp/s1-worktree"));
            }
            _ => panic!("expected Command::Pull variant with Session target"),
        }
    }

    #[test]
    fn wire_pull_project_deserializes() {
        let json = r#"{"command":"pull_project","args":{"project_id":"p1"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::PullProject {
                project_id: "p1".to_string()
            }
        );
    }

    #[test]
    fn wire_to_command_pull_project_mirrors_tui_refresh() {
        // sample_project("p1", "/repo") has leading_branch Some("main") and
        // name "p1-name"; the constructed Pull must target the project's source
        // checkout path with PullTarget::Project and the TUI's exact messages.
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        let cmd = engine
            .wire_to_command(WireCommand::PullProject {
                project_id: "p1".to_string(),
            })
            .expect("reconstruct");
        match cmd {
            Command::Pull {
                repo_path,
                target,
                busy_message,
                already_running_message,
            } => {
                assert_eq!(repo_path, Path::new("/repo"));
                match target {
                    PullTarget::Project {
                        project_id,
                        project_name,
                        leading_branch,
                    } => {
                        assert_eq!(project_id, "p1");
                        assert_eq!(project_name, "p1-name");
                        assert_eq!(leading_branch.as_deref(), Some("main"));
                    }
                    PullTarget::Session => panic!("expected PullTarget::Project"),
                }
                assert_eq!(
                    busy_message,
                    "Refreshing project \"p1-name\" from remote\u{2026}"
                );
                assert_eq!(
                    already_running_message,
                    "Project refresh already in progress for \"p1-name\". Wait for the current pull to finish."
                );
            }
            _ => panic!("expected Command::Pull variant with Project target"),
        }
    }

    #[test]
    fn wire_to_command_pull_project_unknown_project_errors() {
        let (engine, _tmp) = test_engine();
        let err = engine
            .wire_to_command(WireCommand::PullProject {
                project_id: "ghost".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("unknown project"), "err: {err}");
    }

    #[test]
    fn wire_to_command_pull_project_path_missing_errors() {
        let (mut engine, _tmp) = test_engine();
        let mut project = sample_project("p1", "/repo");
        project.path_missing = true;
        engine.projects.push(project);
        let err = engine
            .wire_to_command(WireCommand::PullProject {
                project_id: "p1".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        // Mirrors the TUI's `refresh_selected_project` path_missing warning.
        assert!(
            err.to_string()
                .contains("Cannot refresh: path not found for \"p1-name\""),
            "err: {err}"
        );
    }

    #[test]
    fn apply_wire_pull_project_blocks_repeat_while_running() {
        // First dispatch spawns the pull worker (busy arrives on the async
        // status stream, so the synchronous outcome is empty); the second
        // dispatch hits the in-flight guard and surfaces the TUI's
        // already-running warning synchronously.
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));

        let first = engine
            .apply_wire(WireCommand::PullProject {
                project_id: "p1".to_string(),
            })
            .expect("first project pull");
        assert!(
            first.status.is_none(),
            "first pull busy is async, not a synchronous outcome: {:?}",
            first.status
        );
        assert!(engine.is_in_flight(&InFlightKey::Pull("/repo".to_string())));

        let second = engine
            .apply_wire(WireCommand::PullProject {
                project_id: "p1".to_string(),
            })
            .expect("repeat project pull should not error");
        let status = second.status.expect("in-flight warning status");
        assert_eq!(status.tone, "warning");
        assert!(
            status.message.contains("already in progress"),
            "unexpected message: {}",
            status.message
        );
    }

    #[test]
    fn wire_to_command_unknown_session_errors() {
        let (engine, _tmp) = test_engine();
        let result = engine.wire_to_command(WireCommand::Push {
            session_id: "ghost".to_string(),
        });
        let err = result.map(|_| ()).unwrap_err();
        assert!(err.to_string().contains("unknown session"), "err: {err}");
    }

    #[test]
    fn apply_wire_unknown_session_errors() {
        let (mut engine, _tmp) = test_engine();
        let res = engine.apply_wire(WireCommand::Push {
            session_id: "ghost".to_string(),
        });
        assert!(res.is_err());
    }

    fn init_repo() -> tempfile::TempDir {
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
        std::fs::write(dir.path().join("a.txt"), "hello\n").expect("write file");
        dir
    }

    #[test]
    fn wire_delete_session_deserializes() {
        let json =
            r#"{"command":"delete_session","args":{"session_id":"s1","delete_worktree":true}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::DeleteSession {
                session_id: "s1".to_string(),
                delete_worktree: true,
            }
        );
    }

    #[test]
    fn wire_persist_global_env_deserializes() {
        let json = r#"{"command":"persist_global_env","args":{"env":{"FOO":"bar"}}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        match cmd {
            WireCommand::PersistGlobalEnv { env } => {
                assert_eq!(env.get("FOO").map(String::as_str), Some("bar"));
            }
            _ => panic!("expected WireCommand::PersistGlobalEnv variant"),
        }
    }

    #[test]
    fn wire_update_project_provider_deserializes() {
        let json = r#"{"command":"update_project_provider","args":{"project_id":"p1","provider":"codex"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::UpdateProjectProvider {
                project_id: "p1".to_string(),
                provider: Some("codex".to_string()),
            }
        );
    }

    #[test]
    fn wire_update_project_auto_reopen_deserializes() {
        let json = r#"{"command":"update_project_auto_reopen","args":{"project_id":"p1","auto_reopen_agents":true}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::UpdateProjectAutoReopen {
                project_id: "p1".to_string(),
                auto_reopen_agents: Some(true),
            }
        );
    }

    #[test]
    fn wire_update_project_startup_command_deserializes() {
        let json = r#"{"command":"update_project_startup_command","args":{"project_id":"p1","startup_command":"echo hi"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::UpdateProjectStartupCommand {
                project_id: "p1".to_string(),
                startup_command: Some("echo hi".to_string()),
            }
        );
    }

    #[test]
    fn wire_update_project_env_deserializes() {
        let json =
            r#"{"command":"update_project_env","args":{"project_id":"p1","env":{"FOO":"bar"}}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        match cmd {
            WireCommand::UpdateProjectEnv { project_id, env } => {
                assert_eq!(project_id, "p1");
                assert_eq!(env.get("FOO").map(String::as_str), Some("bar"));
            }
            _ => panic!("expected WireCommand::UpdateProjectEnv variant"),
        }
    }

    #[test]
    fn wire_reload_config_deserializes_with_empty_args_object() {
        // The frontend sends `args: {}` through the generic command envelope, and
        // the server always re-includes the `args` key when reconstructing the
        // envelope. The empty struct variant deserializes from that map form. (A
        // true unit variant would reject the `args:{}` map; the empty struct
        // variant requires the `args` key, which is exactly what the wire carries.)
        let json = r#"{"command":"reload_config","args":{}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(cmd, WireCommand::ReloadConfig {});
    }

    #[test]
    fn wire_recover_config_deserializes_with_empty_args_object() {
        let json = r#"{"command":"recover_config","args":{}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(cmd, WireCommand::RecoverConfig {});
    }

    #[test]
    fn wire_checkout_project_default_branch_deserializes() {
        let json = r#"{"command":"checkout_project_default_branch","args":{"project_id":"p1"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::CheckoutProjectDefaultBranch {
                project_id: "p1".to_string(),
            }
        );
    }

    // Initialize a repo on `default_branch` with one commit, then create and
    // check out `feature` so HEAD sits off the default. Returns the temp dir.
    fn init_repo_on_feature_branch(default_branch: &str) -> tempfile::TempDir {
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
        run(&["init", "-q", "-b", default_branch]);
        run(&["config", "user.email", "t@example.com"]);
        run(&["config", "user.name", "t"]);
        run(&["commit", "--allow-empty", "-q", "-m", "init"]);
        run(&["switch", "-q", "-c", "feature"]);
        dir
    }

    fn current_git_branch(repo: &std::path::Path) -> String {
        let out = std::process::Command::new("git")
            .args([
                "-C",
                repo.to_string_lossy().as_ref(),
                "symbolic-ref",
                "--short",
                "HEAD",
            ])
            .output()
            .expect("spawn git");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    // Drive the checkout-default-branch worker chain the same way the web engine
    // actor's drain does (engine_actor.rs): pull the inspection event (worker 1)
    // off `worker_rx`, run `process_worker_event`, then feed the reaction through
    // BOTH `wire_statuses_from_reaction` (the Status outcomes) and
    // `drive_checkout_followup` (the Known case, which spawns worker 2). Returns
    // every status the actor would broadcast across both phases, in order.
    fn drive_checkout_chain(engine: &mut Engine) -> Vec<WireStatus> {
        let mut statuses = Vec::new();
        // Phase 1: the inspection worker. The Known case spawns worker 2; the
        // other cases produce their final status here.
        let event = engine
            .worker_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("inspection worker event");
        let reaction = engine.process_worker_event(event);
        statuses.extend(wire_statuses_from_reaction(&reaction));
        let spawned_switch = matches!(
            reaction,
            EventReaction::DispatchProjectDefaultBranchCheckout { .. }
        );
        statuses.extend(engine.drive_checkout_followup(&reaction));
        if spawned_switch {
            // Phase 2: the switch worker's completion carries the success/failure
            // status, surfaced by the actor's wire_statuses_from_reaction drain.
            let event = engine
                .worker_rx
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("switch worker event");
            let reaction = engine.process_worker_event(event);
            statuses.extend(wire_statuses_from_reaction(&reaction));
        }
        statuses
    }

    #[test]
    fn apply_wire_checkout_project_default_branch_switches_from_feature() {
        // A project whose persisted leading branch ("trunk") differs from HEAD
        // ("feature") gets switched back, and the in-memory state updates to
        // Leading — mirroring the TUI's Known-default-branch outcome. apply_wire
        // now returns Busy and spawns the inspection worker; the switch happens
        // off-thread, driven through the two-worker chain like the web actor.
        let repo = init_repo_on_feature_branch("trunk");
        let (mut engine, _tmp) = test_engine();
        let mut project = sample_project("p1", repo.path().to_string_lossy().as_ref());
        project.leading_branch = Some("trunk".to_string());
        project.current_branch = "feature".to_string();
        project.branch_status = ProjectBranchStatus::NotLeading;
        engine.projects.push(project);

        let outcome = engine
            .apply_wire(WireCommand::CheckoutProjectDefaultBranch {
                project_id: "p1".to_string(),
            })
            .expect("checkout");
        let busy = outcome.status.expect("busy status");
        assert_eq!(busy.tone, "busy");
        assert!(
            busy.message
                .contains("Checking the default branch for project \"p1-name\""),
            "unexpected busy message: {}",
            busy.message
        );

        let statuses = drive_checkout_chain(&mut engine);
        let status = statuses.last().expect("final status");
        assert_eq!(status.tone, "info");
        assert!(
            status
                .message
                .contains("Checked out \"trunk\" for project \"p1-name\""),
            "unexpected message: {}",
            status.message
        );
        // HEAD actually moved in the temp repo.
        assert_eq!(current_git_branch(repo.path()), "trunk");
        let updated = &engine.projects[0];
        assert_eq!(updated.current_branch, "trunk");
        assert_eq!(updated.branch_status, ProjectBranchStatus::Leading);
    }

    #[test]
    fn apply_wire_checkout_project_default_branch_switch_failure_reports_error() {
        // Worker 2 fails: the persisted leading branch ("ghost") does not exist,
        // so `git switch` errors. The inspection (which trusts the persisted
        // leading branch) yields Known, worker 2 runs the switch and fails, and
        // the completion arm surfaces the TUI's exact failure wording. HEAD stays
        // on feature.
        let repo = init_repo_on_feature_branch("trunk");
        let (mut engine, _tmp) = test_engine();
        let mut project = sample_project("p1", repo.path().to_string_lossy().as_ref());
        project.leading_branch = Some("ghost".to_string());
        project.current_branch = "feature".to_string();
        project.branch_status = ProjectBranchStatus::NotLeading;
        engine.projects.push(project);

        let outcome = engine
            .apply_wire(WireCommand::CheckoutProjectDefaultBranch {
                project_id: "p1".to_string(),
            })
            .expect("checkout");
        assert_eq!(outcome.status.expect("busy status").tone, "busy");

        let statuses = drive_checkout_chain(&mut engine);
        let status = statuses.last().expect("final status");
        assert_eq!(status.tone, "error");
        assert!(
            status.message.contains("Couldn't check out \"ghost\" in")
                && status
                    .message
                    .contains("resolve in your terminal and retry"),
            "unexpected message: {}",
            status.message
        );
        // The failed switch leaves HEAD where it was.
        assert_eq!(current_git_branch(repo.path()), "feature");
    }

    #[test]
    fn apply_wire_checkout_project_default_branch_inspection_error_reports_error() {
        // Worker 1 fails: the project's path points at a directory that is not a
        // git repo, so `git::current_branch` errors. (The `path_missing` guard is
        // a stored flag, not a live filesystem check, so a non-repo path with
        // `path_missing == false` reaches the inspection worker.) The inspection
        // arm surfaces the "Couldn't inspect the default branch" wording and no
        // switch is spawned.
        let not_a_repo = tempfile::tempdir().expect("tempdir");
        let (mut engine, _tmp) = test_engine();
        let mut project = sample_project("p1", not_a_repo.path().to_string_lossy().as_ref());
        project.leading_branch = None;
        project.current_branch = "feature".to_string();
        project.path_missing = false;
        engine.projects.push(project);

        let outcome = engine
            .apply_wire(WireCommand::CheckoutProjectDefaultBranch {
                project_id: "p1".to_string(),
            })
            .expect("checkout");
        assert_eq!(outcome.status.expect("busy status").tone, "busy");

        let statuses = drive_checkout_chain(&mut engine);
        let status = statuses.last().expect("final status");
        assert_eq!(status.tone, "error");
        assert!(
            status
                .message
                .contains("Couldn't inspect the default branch for project \"p1-name\""),
            "unexpected message: {}",
            status.message
        );
    }

    #[test]
    fn apply_wire_checkout_project_default_branch_already_on_leading_is_noop() {
        // HEAD already equals the persisted leading branch: no switch, an info
        // status, and the branch is left untouched (the TUI's None outcome).
        let repo = init_repo_on_feature_branch("trunk");
        let (mut engine, _tmp) = test_engine();
        let mut project = sample_project("p1", repo.path().to_string_lossy().as_ref());
        // Pin the leading branch to the branch HEAD is on so it is already leading.
        project.leading_branch = Some("feature".to_string());
        project.current_branch = "feature".to_string();
        engine.projects.push(project);

        let outcome = engine
            .apply_wire(WireCommand::CheckoutProjectDefaultBranch {
                project_id: "p1".to_string(),
            })
            .expect("checkout");
        assert_eq!(outcome.status.expect("busy status").tone, "busy");

        let statuses = drive_checkout_chain(&mut engine);
        let status = statuses.last().expect("final status");
        assert_eq!(status.tone, "info");
        assert!(
            status
                .message
                .contains("already on the leading branch \"feature\""),
            "unexpected message: {}",
            status.message
        );
        assert_eq!(current_git_branch(repo.path()), "feature");
        assert_eq!(
            engine.projects[0].branch_status,
            ProjectBranchStatus::Leading
        );
    }

    #[test]
    fn apply_wire_checkout_project_default_branch_heuristic_errors() {
        // No origin/HEAD and HEAD is neither main nor master, so the default
        // branch can't be determined: the TUI refuses with an error status
        // rather than guessing. No leading_branch is persisted here so the
        // heuristic path runs.
        let repo = init_repo_on_feature_branch("trunk");
        let (mut engine, _tmp) = test_engine();
        let mut project = sample_project("p1", repo.path().to_string_lossy().as_ref());
        project.leading_branch = None;
        project.current_branch = "feature".to_string();
        engine.projects.push(project);

        let outcome = engine
            .apply_wire(WireCommand::CheckoutProjectDefaultBranch {
                project_id: "p1".to_string(),
            })
            .expect("checkout");
        assert_eq!(outcome.status.expect("busy status").tone, "busy");

        let statuses = drive_checkout_chain(&mut engine);
        let status = statuses.last().expect("final status");
        assert_eq!(status.tone, "error");
        assert!(
            status
                .message
                .contains("Can't determine the default branch"),
            "unexpected message: {}",
            status.message
        );
        // Heuristic refusal leaves HEAD where it was.
        assert_eq!(current_git_branch(repo.path()), "feature");
    }

    #[test]
    fn apply_wire_checkout_project_default_branch_unknown_project_errors() {
        let (mut engine, _tmp) = test_engine();
        let err = engine
            .apply_wire(WireCommand::CheckoutProjectDefaultBranch {
                project_id: "ghost".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("unknown project"), "err: {err}");
    }

    #[test]
    fn apply_wire_checkout_project_default_branch_path_missing_errors() {
        let (mut engine, _tmp) = test_engine();
        let mut project = sample_project("p1", "/repo");
        project.path_missing = true;
        engine.projects.push(project);
        let err = engine
            .apply_wire(WireCommand::CheckoutProjectDefaultBranch {
                project_id: "p1".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("Cannot check out default branch: path not found for \"p1-name\""),
            "err: {err}"
        );
    }

    #[test]
    fn wire_to_command_reload_config_maps_to_command() {
        let (engine, _tmp) = test_engine();
        let cmd = engine
            .wire_to_command(WireCommand::ReloadConfig {})
            .expect("reconstruct");
        assert!(matches!(cmd, Command::ReloadConfig));
    }

    #[test]
    fn wire_to_command_recover_config_maps_to_command() {
        let (engine, _tmp) = test_engine();
        let cmd = engine
            .wire_to_command(WireCommand::RecoverConfig {})
            .expect("reconstruct");
        assert!(matches!(cmd, Command::RecoverConfig));
    }

    #[test]
    fn wire_to_command_update_project_startup_command_builds_persist_action() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        let cmd = engine
            .wire_to_command(WireCommand::UpdateProjectStartupCommand {
                project_id: "p1".to_string(),
                startup_command: Some("echo hi".to_string()),
            })
            .expect("reconstruct");
        match cmd {
            Command::PersistProject(action) => match *action {
                ProjectPersistenceAction::UpdateStartupCommand {
                    project_id,
                    project_name,
                    startup_command,
                } => {
                    assert_eq!(project_id, "p1");
                    assert_eq!(project_name, "p1-name");
                    assert_eq!(startup_command.as_deref(), Some("echo hi"));
                }
                other => panic!("expected UpdateStartupCommand, got {other:?}"),
            },
            _ => panic!("expected Command::PersistProject variant"),
        }
    }

    #[test]
    fn wire_to_command_update_project_unknown_project_errors() {
        let (engine, _tmp) = test_engine();
        let result = engine.wire_to_command(WireCommand::UpdateProjectStartupCommand {
            project_id: "ghost".to_string(),
            startup_command: None,
        });
        let err = result.map(|_| ()).unwrap_err();
        assert!(err.to_string().contains("unknown project"), "err: {err}");
    }

    #[test]
    fn apply_wire_delete_session_inline_removes_session() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let session = sample_session("s1", "p1", "feat");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        // The web delete path must also drop the activity stamp — this is the
        // engine-side cleanup; no TUI App caller exists on this path to do it.
        engine
            .pty_activity
            .insert("s1".to_string(), std::time::Instant::now());

        let outcome = engine
            .apply_wire(WireCommand::DeleteSession {
                session_id: "s1".to_string(),
                delete_worktree: false,
            })
            .expect("apply_wire");
        let status = outcome.status.expect("status");
        assert!(
            status.message.contains("Deleted agent"),
            "unexpected status: {}",
            status.message
        );
        assert!(!engine.sessions.iter().any(|s| s.id == "s1"));
        assert!(
            !engine.pty_activity.contains_key("s1"),
            "deleting a session over the wire must clear its activity stamp"
        );
    }

    #[test]
    fn wire_statuses_passes_through_status() {
        let r = EventReaction::Status(StatusUpdate::error("boom"));
        let s = wire_statuses_from_reaction(&r);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].tone, "error");
        assert_eq!(s[0].message, "boom");
    }

    #[test]
    fn wire_statuses_formats_launch_failure() {
        let r =
            EventReaction::AgentLaunchFailedView(Box::new(AgentLaunchFailedOutcome::Reconnect {
                branch_name: "feat".to_string(),
                message: "nope".to_string(),
            }));
        let s = wire_statuses_from_reaction(&r);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].tone, "error");
        assert!(
            s[0].message
                .contains("Reconnect failed for agent \"feat\": nope")
        );
    }

    #[test]
    fn wire_statuses_resume_fallback_is_silent() {
        let r = EventReaction::AgentLaunchFailedView(Box::new(
            AgentLaunchFailedOutcome::ResumeFallback,
        ));
        assert!(wire_statuses_from_reaction(&r).is_empty());
    }

    #[test]
    fn wire_statuses_flattens_multi() {
        let r = EventReaction::Multi(vec![
            EventReaction::Status(StatusUpdate::info("a")),
            EventReaction::Nothing,
            EventReaction::Status(StatusUpdate::busy("b")),
        ]);
        assert_eq!(wire_statuses_from_reaction(&r).len(), 2);
    }

    #[test]
    fn apply_wire_stage_file_stages_in_real_repo() {
        let repo = init_repo();
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat");
        session.worktree_path = repo.path().to_string_lossy().into_owned();
        engine.sessions.push(session);

        let outcome = engine
            .apply_wire(WireCommand::StageFile {
                session_id: "s1".to_string(),
                path: "a.txt".to_string(),
            })
            .expect("apply_wire");
        // StageFile dispatches to EventReaction::Nothing -> no status.
        assert!(outcome.status.is_none());

        let staged = std::process::Command::new("git")
            .args(["diff", "--cached", "--name-only"])
            .current_dir(repo.path())
            .output()
            .expect("git diff");
        let names = String::from_utf8_lossy(&staged.stdout);
        assert!(names.contains("a.txt"), "staged names: {names}");
    }

    #[test]
    fn wire_generate_commit_message_deserializes() {
        let json = r#"{"command":"generate_commit_message","args":{"session_id":"s1"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::GenerateCommitMessage {
                session_id: "s1".to_string()
            }
        );
    }

    #[test]
    fn wire_to_command_generate_commit_message_maps_to_command() {
        let (engine, _tmp) = test_engine();
        let cmd = engine
            .wire_to_command(WireCommand::GenerateCommitMessage {
                session_id: "s1".to_string(),
            })
            .expect("reconstruct");
        match cmd {
            Command::GenerateCommitMessage { session_id } => {
                assert_eq!(session_id, "s1");
            }
            _ => panic!("expected Command::GenerateCommitMessage variant"),
        }
    }

    /// Stage a file in `repo` so `git diff --cached` has content.
    fn stage_file(repo: &std::path::Path) {
        let ok = std::process::Command::new("git")
            .args(["add", "a.txt"])
            .current_dir(repo)
            .status()
            .expect("spawn git add")
            .success();
        assert!(ok, "git add failed");
    }

    #[test]
    fn apply_generate_commit_message_returns_busy_with_staged_diff() {
        let repo = init_repo();
        stage_file(repo.path());
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat");
        session.worktree_path = repo.path().to_string_lossy().into_owned();
        engine.sessions.push(session);

        let reaction = engine
            .apply(Command::GenerateCommitMessage {
                session_id: "s1".to_string(),
            })
            .expect("apply");
        match reaction {
            EventReaction::Status(update) => {
                assert_eq!(update.tone, StatusTone::Busy);
                assert!(
                    update.message.contains("Generating an AI commit message"),
                    "unexpected message: {}",
                    update.message
                );
            }
            _ => panic!("expected Busy Status reaction"),
        }
    }

    #[test]
    fn apply_generate_commit_message_errors_with_nothing_staged() {
        // init_repo writes a.txt but does NOT stage it, so the cached diff is empty.
        let repo = init_repo();
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat");
        session.worktree_path = repo.path().to_string_lossy().into_owned();
        engine.sessions.push(session);

        let reaction = engine
            .apply(Command::GenerateCommitMessage {
                session_id: "s1".to_string(),
            })
            .expect("apply");
        match reaction {
            EventReaction::Status(update) => {
                assert_eq!(update.tone, StatusTone::Error);
                assert!(
                    update.message.contains("No staged changes"),
                    "unexpected message: {}",
                    update.message
                );
            }
            _ => panic!("expected Error Status reaction"),
        }
    }

    #[test]
    fn apply_generate_commit_message_unknown_session_errors() {
        let (mut engine, _tmp) = test_engine();
        let reaction = engine
            .apply(Command::GenerateCommitMessage {
                session_id: "ghost".to_string(),
            })
            .expect("apply");
        match reaction {
            EventReaction::Status(update) => {
                assert_eq!(update.tone, StatusTone::Error);
                assert!(update.message.contains("Unknown session"));
            }
            _ => panic!("expected Error Status reaction"),
        }
    }

    /// Like `init_repo`, but also stages and commits the file so HEAD exists
    /// and `current_branch` resolves a normal (born) branch.
    fn init_repo_with_commit() -> tempfile::TempDir {
        let dir = init_repo();
        let run = |args: &[&str]| {
            let ok = std::process::Command::new("git")
                .args(args)
                .current_dir(dir.path())
                .status()
                .expect("spawn git")
                .success();
            assert!(ok, "git {args:?} failed");
        };
        run(&["add", "a.txt"]);
        run(&["commit", "-q", "-m", "init"]);
        dir
    }

    #[test]
    fn wire_add_project_deserializes() {
        let json = r#"{"command":"add_project","args":{"path":"/repo","name":"My Project"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::AddProject {
                path: "/repo".to_string(),
                name: "My Project".to_string(),
            }
        );
    }

    #[test]
    fn wire_remove_project_deserializes() {
        let json = r#"{"command":"remove_project","args":{"project_id":"p1"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::RemoveProject {
                project_id: "p1".to_string(),
            }
        );
    }

    #[test]
    fn wire_to_command_add_project_builds_persist_add() {
        let repo = init_repo_with_commit();
        let (engine, _tmp) = test_engine();
        let cmd = engine
            .wire_to_command(WireCommand::AddProject {
                path: repo.path().to_string_lossy().into_owned(),
                name: String::new(),
            })
            .expect("reconstruct");
        // canonicalize so the assertion survives symlinked temp dirs (e.g. /tmp -> /private/tmp).
        let expected_path = repo.path().canonicalize().unwrap();
        let expected_name = expected_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap()
            .to_string();
        match cmd {
            Command::PersistProject(action) => match *action {
                ProjectPersistenceAction::Add { project, .. } => {
                    assert_eq!(PathBuf::from(&project.path), expected_path);
                    assert_eq!(project.name, expected_name);
                }
                other => panic!("expected Add, got {other:?}"),
            },
            _ => panic!("expected Command::PersistProject variant"),
        }
    }

    #[test]
    fn wire_to_command_add_project_rejects_non_repo() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (engine, _tmp) = test_engine();
        let result = engine.wire_to_command(WireCommand::AddProject {
            path: dir.path().to_string_lossy().into_owned(),
            name: String::new(),
        });
        let err = result.map(|_| ()).unwrap_err();
        assert!(
            err.to_string().contains("not a git repository"),
            "err: {err}"
        );
    }

    #[test]
    fn wire_to_command_remove_project() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        let cmd = engine
            .wire_to_command(WireCommand::RemoveProject {
                project_id: "p1".to_string(),
            })
            .expect("reconstruct");
        match cmd {
            Command::PersistProject(action) => match *action {
                ProjectPersistenceAction::Remove {
                    project_id,
                    project_name,
                } => {
                    assert_eq!(project_id, "p1");
                    assert_eq!(project_name, "p1-name");
                }
                other => panic!("expected Remove, got {other:?}"),
            },
            _ => panic!("expected Command::PersistProject variant"),
        }
    }

    #[test]
    fn wire_to_command_remove_project_with_sessions_errors() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let err = engine
            .wire_to_command(WireCommand::RemoveProject {
                project_id: "p1".to_string(),
            })
            .map(|_| ())
            .expect_err("removing a project with agents must be refused");
        assert!(
            err.to_string().contains("Delete this project's agents"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn wire_create_agent_deserializes() {
        let json = r#"{"command":"create_agent","args":{"project_id":"p1","name":"feature-x"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::CreateAgent {
                project_id: "p1".to_string(),
                name: "feature-x".to_string(),
            }
        );
    }

    #[test]
    fn wire_to_command_create_agent_builds_request() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));

        // Non-empty name -> Some(custom_name), use_existing_branch == false.
        let cmd = engine
            .wire_to_command(WireCommand::CreateAgent {
                project_id: "p1".to_string(),
                name: "feature-x".to_string(),
            })
            .expect("reconstruct");
        match cmd {
            Command::DispatchCreateAgentRequest { request, .. } => match *request {
                CreateAgentRequest::NewProject {
                    project,
                    custom_name,
                    use_existing_branch,
                    ..
                } => {
                    assert_eq!(project.id, "p1");
                    assert_eq!(custom_name.as_deref(), Some("feature-x"));
                    assert!(!use_existing_branch);
                }
                other => panic!("expected NewProject, got {other:?}"),
            },
            _ => panic!("expected Command::DispatchCreateAgentRequest variant"),
        }

        // Empty name -> custom_name == None.
        let cmd = engine
            .wire_to_command(WireCommand::CreateAgent {
                project_id: "p1".to_string(),
                name: String::new(),
            })
            .expect("reconstruct");
        match cmd {
            Command::DispatchCreateAgentRequest { request, .. } => match *request {
                CreateAgentRequest::NewProject { custom_name, .. } => {
                    assert_eq!(custom_name, None);
                }
                other => panic!("expected NewProject, got {other:?}"),
            },
            _ => panic!("expected Command::DispatchCreateAgentRequest variant"),
        }
    }

    #[test]
    fn wire_to_command_create_agent_unknown_project_errors() {
        let (engine, _tmp) = test_engine();
        let result = engine.wire_to_command(WireCommand::CreateAgent {
            project_id: "ghost".to_string(),
            name: "feature-x".to_string(),
        });
        let err = result.map(|_| ()).unwrap_err();
        assert!(err.to_string().contains("unknown project"), "err: {err}");
    }

    #[test]
    fn wire_to_command_create_agent_rejects_invalid_names() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));

        // Raw wire clients can send anything; each of these must be rejected with
        // an actionable message naming the rules.
        for bad in [
            "has space", // spaces aren't valid (the FE converts; raw wire doesn't)
            "-leading",  // can't start with a dash
            "a//b",      // no consecutive slashes
            "trailing/", // can't end with a slash
            "/leading",  // can't start with a slash
            "naïve",     // non-ASCII dropped
            "emoji😀",   // non-ASCII dropped
            "with.dot",  // '.' isn't whitelisted
        ] {
            let result = engine.wire_to_command(WireCommand::CreateAgent {
                project_id: "p1".to_string(),
                name: bad.to_string(),
            });
            let err = result.map(|_| ()).unwrap_err();
            let msg = err.to_string();
            assert!(
                msg.contains("Invalid agent name"),
                "expected rejection for {bad:?}, got: {msg}"
            );
        }
    }

    #[test]
    fn wire_to_command_create_agent_accepts_valid_names() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));

        for good in ["feature-x", "feat/sub-thing", "a_b-c", "AbC123"] {
            let cmd = engine
                .wire_to_command(WireCommand::CreateAgent {
                    project_id: "p1".to_string(),
                    name: good.to_string(),
                })
                .unwrap_or_else(|e| panic!("expected {good:?} to be accepted, got: {e}"));
            match cmd {
                Command::DispatchCreateAgentRequest { request, .. } => match *request {
                    CreateAgentRequest::NewProject { custom_name, .. } => {
                        assert_eq!(custom_name.as_deref(), Some(good));
                    }
                    other => panic!("expected NewProject, got {other:?}"),
                },
                _ => panic!("expected Command::DispatchCreateAgentRequest variant"),
            }
        }
    }

    #[test]
    fn wire_to_command_create_agent_empty_after_trim_is_none() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));

        // Whitespace-only names trim to empty -> None (server auto-generates),
        // which is valid and must NOT trip the backstop.
        let cmd = engine
            .wire_to_command(WireCommand::CreateAgent {
                project_id: "p1".to_string(),
                name: "   ".to_string(),
            })
            .expect("whitespace-only name should reconstruct");
        match cmd {
            Command::DispatchCreateAgentRequest { request, .. } => match *request {
                CreateAgentRequest::NewProject { custom_name, .. } => {
                    assert_eq!(custom_name, None);
                }
                other => panic!("expected NewProject, got {other:?}"),
            },
            _ => panic!("expected Command::DispatchCreateAgentRequest variant"),
        }
    }

    #[test]
    fn wire_reorder_sessions_deserializes() {
        let json =
            r#"{"command":"reorder_sessions","args":{"project_id":"p1","session_ids":["b","a"]}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::ReorderSessions {
                project_id: "p1".to_string(),
                session_ids: vec!["b".to_string(), "a".to_string()],
            }
        );
    }

    #[test]
    fn wire_reorder_projects_deserializes() {
        let json = r#"{"command":"reorder_projects","args":{"project_ids":["c","a","b"]}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::ReorderProjects {
                project_ids: vec!["c".to_string(), "a".to_string(), "b".to_string()],
            }
        );
    }

    #[test]
    fn wire_to_command_reorder_sessions_maps_to_command() {
        let (engine, _tmp) = test_engine();
        let cmd = engine
            .wire_to_command(WireCommand::ReorderSessions {
                project_id: "p1".to_string(),
                session_ids: vec!["b".to_string(), "a".to_string()],
            })
            .expect("reconstruct");
        match cmd {
            Command::ReorderSessions {
                project_id,
                session_ids,
            } => {
                assert_eq!(project_id, "p1");
                assert_eq!(session_ids, vec!["b".to_string(), "a".to_string()]);
            }
            _ => panic!("expected Command::ReorderSessions variant"),
        }
    }

    #[test]
    fn wire_to_command_reorder_projects_maps_to_command() {
        let (engine, _tmp) = test_engine();
        let cmd = engine
            .wire_to_command(WireCommand::ReorderProjects {
                project_ids: vec!["c".to_string(), "a".to_string()],
            })
            .expect("reconstruct");
        match cmd {
            Command::ReorderProjects { project_ids } => {
                assert_eq!(project_ids, vec!["c".to_string(), "a".to_string()]);
            }
            _ => panic!("expected Command::ReorderProjects variant"),
        }
    }

    #[test]
    fn apply_wire_reorder_sessions_reorders_and_is_silent() {
        let (mut engine, _tmp) = test_engine();
        for id in ["a", "b"] {
            let session = sample_session(id, "p1", id);
            engine.session_store.upsert_session(&session).unwrap();
            engine.sessions.push(session);
        }
        let outcome = engine
            .apply_wire(WireCommand::ReorderSessions {
                project_id: "p1".to_string(),
                session_ids: vec!["b".to_string(), "a".to_string()],
            })
            .expect("apply_wire");
        // Silent success: no status surfaced.
        assert!(outcome.status.is_none());
        let ids: Vec<String> = engine.sessions.iter().map(|s| s.id.clone()).collect();
        assert_eq!(ids, vec!["b".to_string(), "a".to_string()]);
    }

    #[test]
    fn apply_wire_reorder_sessions_invalid_set_errors() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("a", "p1", "a"));
        engine.sessions.push(sample_session("b", "p1", "b"));
        let res = engine.apply_wire(WireCommand::ReorderSessions {
            project_id: "p1".to_string(),
            session_ids: vec!["a".to_string()],
        });
        assert!(res.is_err());
    }

    #[test]
    fn drive_delete_followup_finishes_on_worktree_removed() {
        // The async deletion path: BeginDeleteSession spawned a git-removal
        // worker and did NOT remove the session yet. When the worker reports
        // success, drive_delete_followup must run FinishDeleteSession to
        // completion and report the "removed its worktree" status. This covers
        // the async glue without needing a real git worktree.
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/tmp/p1"));
        let session = sample_session("s1", "p1", "feat");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        let reaction = EventReaction::WorktreeRemoveSucceeded {
            session_id: "s1".to_string(),
            branch_already_deleted: false,
            our_busy_message: None,
        };
        let statuses = engine.drive_delete_followup(&reaction);

        assert!(
            !engine.sessions.iter().any(|s| s.id == "s1"),
            "session should be removed after worktree removal"
        );
        assert_eq!(statuses.len(), 1, "expected one status: {statuses:?}");
        assert_eq!(statuses[0].tone, "info");
        assert!(
            statuses[0].message.contains("Deleted agent")
                && statuses[0].message.contains("removed its worktree"),
            "unexpected status: {}",
            statuses[0].message
        );
    }

    // ---- G2: fork ----------------------------------------------------------

    #[test]
    fn wire_fork_session_deserializes() {
        let json = r#"{"command":"fork_session","args":{"session_id":"s1","name":"feature-y"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::ForkSession {
                session_id: "s1".to_string(),
                name: "feature-y".to_string(),
            }
        );
    }

    #[test]
    fn wire_to_command_fork_session_builds_request() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        // `sample_session` sets title = Some("s1-title"); source_label must use it.
        engine.sessions.push(sample_session("s1", "p1", "feat"));

        let cmd = engine
            .wire_to_command(WireCommand::ForkSession {
                session_id: "s1".to_string(),
                name: "feature-y".to_string(),
            })
            .expect("reconstruct");
        match cmd {
            Command::DispatchCreateAgentRequest {
                request,
                busy_message,
                ..
            } => match *request {
                CreateAgentRequest::ForkSession {
                    project,
                    source_session,
                    source_label,
                    custom_name,
                } => {
                    assert_eq!(project.id, "p1");
                    assert_eq!(source_session.id, "s1");
                    // session_label precedence: title-or-branch.
                    assert_eq!(source_label, "s1-title");
                    assert_eq!(custom_name.as_deref(), Some("feature-y"));
                    // Busy copy mirrors the TUI fork message.
                    assert!(
                        busy_message.contains("Forking agent \"s1-title\" as \"feature-y\""),
                        "unexpected busy message: {busy_message}"
                    );
                }
                other => panic!("expected ForkSession, got {other:?}"),
            },
            _ => panic!("expected Command::DispatchCreateAgentRequest variant"),
        }
    }

    #[test]
    fn wire_to_command_fork_session_label_falls_back_to_branch() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        let mut session = sample_session("s1", "p1", "feat");
        session.title = None; // no custom title -> source_label is the branch.
        engine.sessions.push(session);

        let cmd = engine
            .wire_to_command(WireCommand::ForkSession {
                session_id: "s1".to_string(),
                name: "feature-y".to_string(),
            })
            .expect("reconstruct");
        match cmd {
            Command::DispatchCreateAgentRequest { request, .. } => match *request {
                CreateAgentRequest::ForkSession { source_label, .. } => {
                    assert_eq!(source_label, "feat");
                }
                other => panic!("expected ForkSession, got {other:?}"),
            },
            _ => panic!("expected Command::DispatchCreateAgentRequest variant"),
        }
    }

    #[test]
    fn wire_to_command_fork_session_unknown_session_errors() {
        let (engine, _tmp) = test_engine();
        let err = engine
            .wire_to_command(WireCommand::ForkSession {
                session_id: "ghost".to_string(),
                name: "feature-y".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("unknown session"), "err: {err}");
    }

    #[test]
    fn wire_to_command_fork_session_empty_name_errors() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        // Unlike CreateAgent, a fork requires a name (the worker would otherwise
        // reject it). Mirror the TUI prompt's "cannot be empty" rejection.
        for blank in ["", "   "] {
            let err = engine
                .wire_to_command(WireCommand::ForkSession {
                    session_id: "s1".to_string(),
                    name: blank.to_string(),
                })
                .map(|_| ())
                .unwrap_err();
            assert!(
                err.to_string().contains("cannot be empty"),
                "blank {blank:?} err: {err}"
            );
        }
    }

    #[test]
    fn wire_to_command_fork_session_rejects_invalid_name() {
        let (mut engine, _tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        for bad in ["-leading", "a//b", "trailing/", "with.dot"] {
            let err = engine
                .wire_to_command(WireCommand::ForkSession {
                    session_id: "s1".to_string(),
                    name: bad.to_string(),
                })
                .map(|_| ())
                .unwrap_err();
            assert!(
                err.to_string().contains("Invalid agent name"),
                "bad {bad:?} err: {err}"
            );
        }
    }

    // ---- G2: rename --------------------------------------------------------

    #[test]
    fn wire_rename_session_deserializes() {
        let json = r#"{"command":"rename_session","args":{"session_id":"s1","title":"My agent"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::RenameSession {
                session_id: "s1".to_string(),
                title: "My agent".to_string(),
            }
        );
    }

    #[test]
    fn apply_wire_rename_session_sets_title_and_persists() {
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat");
        session.title = None;
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        let outcome = engine
            .apply_wire(WireCommand::RenameSession {
                session_id: "s1".to_string(),
                title: "  My-agent  ".to_string(),
            })
            .expect("apply_wire");
        // Trimmed and stored in memory.
        assert_eq!(
            engine.sessions[0].title.as_deref(),
            Some("My-agent"),
            "title should be trimmed and set"
        );
        let status = outcome.status.expect("rename surfaces a status");
        assert_eq!(status.tone, "info");
        assert!(
            status.message.contains("My-agent"),
            "msg: {}",
            status.message
        );

        // Persisted: a fresh load from the same SQLite file sees the new title.
        let reloaded = engine
            .session_store
            .load_sessions()
            .expect("reload sessions");
        let s = reloaded.iter().find(|s| s.id == "s1").expect("session row");
        assert_eq!(s.title.as_deref(), Some("My-agent"));
    }

    #[test]
    fn apply_wire_rename_session_empty_clears_title() {
        let (mut engine, _tmp) = test_engine();
        let mut session = sample_session("s1", "p1", "feat");
        session.title = Some("old-name".to_string());
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        let outcome = engine
            .apply_wire(WireCommand::RenameSession {
                session_id: "s1".to_string(),
                title: "   ".to_string(),
            })
            .expect("apply_wire");
        // Cleared back to None -> row shows the branch name again.
        assert_eq!(engine.sessions[0].title, None);
        let status = outcome.status.expect("clear surfaces a status");
        assert!(
            status.message.contains("feat"),
            "clear message names the branch: {}",
            status.message
        );

        let reloaded = engine
            .session_store
            .load_sessions()
            .expect("reload sessions");
        let s = reloaded.iter().find(|s| s.id == "s1").expect("session row");
        assert_eq!(s.title, None, "cleared title should persist");
    }

    #[test]
    fn apply_wire_rename_session_rejects_invalid_title() {
        let (mut engine, _tmp) = test_engine();
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let err = engine
            .apply_wire(WireCommand::RenameSession {
                session_id: "s1".to_string(),
                title: "bad name!".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("Invalid agent name"), "err: {err}");
    }

    #[test]
    fn apply_wire_rename_session_unknown_errors() {
        let (mut engine, _tmp) = test_engine();
        let err = engine
            .apply_wire(WireCommand::RenameSession {
                session_id: "ghost".to_string(),
                title: "x".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("unknown session"), "err: {err}");
    }

    // ---- G2: reconnect -----------------------------------------------------

    #[test]
    fn wire_reconnect_session_deserializes() {
        let json = r#"{"command":"reconnect_session","args":{"session_id":"s1","force":true}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::ReconnectSession {
                session_id: "s1".to_string(),
                force: true,
            }
        );
    }

    #[test]
    fn apply_wire_reconnect_session_unknown_errors() {
        let (mut engine, _tmp) = test_engine();
        let err = engine
            .apply_wire(WireCommand::ReconnectSession {
                session_id: "ghost".to_string(),
                force: false,
            })
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("unknown session"), "err: {err}");
    }

    #[test]
    fn apply_wire_reconnect_session_missing_worktree_errors() {
        let (mut engine, _tmp) = test_engine();
        // sample_session points worktree_path at a path that does not exist.
        engine.sessions.push(sample_session("s1", "p1", "feat"));
        let err = engine
            .apply_wire(WireCommand::ReconnectSession {
                session_id: "s1".to_string(),
                force: false,
            })
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("no longer exists"), "err: {err}");
    }

    #[test]
    fn apply_wire_reconnect_session_dispatches_launch() {
        let (mut engine, tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        let mut session = sample_session("s1", "p1", "feat");
        // Point the worktree at a real, existing directory so the
        // worktree-exists pre-check passes.
        let wt = tmp.path().join("wt-s1");
        std::fs::create_dir_all(&wt).unwrap();
        session.worktree_path = wt.to_string_lossy().to_string();
        engine.sessions.push(session);

        let outcome = engine
            .apply_wire(WireCommand::ReconnectSession {
                session_id: "s1".to_string(),
                force: false,
            })
            .expect("apply_wire");
        // The launch was dispatched: the in-flight guard is set and a busy
        // status was surfaced.
        assert!(
            engine.is_in_flight(&crate::engine::InFlightKey::AgentLaunch("s1".to_string())),
            "reconnect should mark the launch in-flight"
        );
        let status = outcome.status.expect("reconnect surfaces a busy status");
        assert_eq!(status.tone, "busy");
        assert!(
            status.message.contains("Launching agent \"feat\""),
            "msg: {}",
            status.message
        );
    }

    #[test]
    fn apply_wire_reconnect_session_force_uses_fresh_busy_copy() {
        let (mut engine, tmp) = test_engine();
        engine.projects.push(sample_project("p1", "/repo"));
        let mut session = sample_session("s1", "p1", "feat");
        let wt = tmp.path().join("wt-s1");
        std::fs::create_dir_all(&wt).unwrap();
        session.worktree_path = wt.to_string_lossy().to_string();
        engine.sessions.push(session);

        let outcome = engine
            .apply_wire(WireCommand::ReconnectSession {
                session_id: "s1".to_string(),
                force: true,
            })
            .expect("apply_wire");
        let status = outcome
            .status
            .expect("force reconnect surfaces a busy status");
        assert_eq!(status.tone, "busy");
        // Force reconnect uses the "Starting fresh agent" copy, distinct from
        // the normal reconnect's "Launching agent".
        assert!(
            status.message.contains("Starting fresh agent \"feat\""),
            "msg: {}",
            status.message
        );
    }

    // ---- L3: change agent provider -----------------------------------------

    #[test]
    fn wire_change_agent_provider_deserializes() {
        let json =
            r#"{"command":"change_agent_provider","args":{"session_id":"s1","provider":"codex"}}"#;
        let cmd: WireCommand = serde_json::from_str(json).expect("deserialize");
        assert_eq!(
            cmd,
            WireCommand::ChangeAgentProvider {
                session_id: "s1".to_string(),
                provider: "codex".to_string(),
            }
        );
    }

    #[test]
    fn apply_wire_change_agent_provider_swaps_and_persists_when_stopped() {
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s1", "p1", "feat"); // provider "claude"
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        let outcome = engine
            .apply_wire(WireCommand::ChangeAgentProvider {
                session_id: "s1".to_string(),
                provider: "codex".to_string(),
            })
            .expect("apply_wire");
        assert_eq!(engine.sessions[0].provider.as_str(), "codex");
        let status = outcome.status.expect("swap surfaces a status");
        assert_eq!(status.tone, "info");
        assert!(
            status.message.contains("will use codex next launch"),
            "msg: {}",
            status.message
        );
        // Not-running + provider never launched here → "fresh session" note.
        assert!(
            status.message.contains("start a fresh session"),
            "msg: {}",
            status.message
        );

        // Persisted: a fresh load from the same SQLite file sees the new provider.
        let reloaded = engine.session_store.load_sessions().expect("reload");
        let s = reloaded.iter().find(|s| s.id == "s1").expect("row");
        assert_eq!(s.provider.as_str(), "codex");
    }

    #[test]
    fn apply_wire_change_agent_provider_same_provider_is_noop() {
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s1", "p1", "feat"); // provider "claude"
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        let outcome = engine
            .apply_wire(WireCommand::ChangeAgentProvider {
                session_id: "s1".to_string(),
                provider: "claude".to_string(),
            })
            .expect("apply_wire");
        let status = outcome.status.expect("no-op still surfaces a status");
        assert_eq!(status.tone, "info");
        assert!(
            status.message.contains("already uses claude"),
            "msg: {}",
            status.message
        );
        // Still claude; no spurious updated_at-driven persistence concerns here.
        assert_eq!(engine.sessions[0].provider.as_str(), "claude");
    }

    #[test]
    fn apply_wire_change_agent_provider_unknown_session_errors() {
        let (mut engine, _tmp) = test_engine();
        let err = engine
            .apply_wire(WireCommand::ChangeAgentProvider {
                session_id: "ghost".to_string(),
                provider: "codex".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("unknown session"), "err: {err}");
    }

    #[test]
    fn apply_wire_change_agent_provider_rejects_unconfigured_provider() {
        let (mut engine, _tmp) = test_engine();
        let session = sample_session("s1", "p1", "feat");
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        // "frobnicate" is not in the configured provider list — the server must
        // reject it rather than trusting the client.
        let err = engine
            .apply_wire(WireCommand::ChangeAgentProvider {
                session_id: "s1".to_string(),
                provider: "frobnicate".to_string(),
            })
            .map(|_| ())
            .unwrap_err();
        assert!(err.to_string().contains("not configured"), "err: {err}");
        // The session's provider is unchanged after a rejected swap.
        assert_eq!(engine.sessions[0].provider.as_str(), "claude");
    }

    #[test]
    fn apply_wire_change_agent_provider_warns_when_running() {
        let (mut engine, _tmp) = test_engine();
        let worktree = tempfile::tempdir().expect("worktree dir");
        engine.projects.push(sample_project(
            "p1",
            worktree.path().to_string_lossy().as_ref(),
        ));
        let mut session = sample_session("s1", "p1", "feat"); // provider "claude"
        session.worktree_path = worktree.path().to_string_lossy().to_string();
        engine.session_store.upsert_session(&session).unwrap();
        engine.sessions.push(session);

        // Spawn a real `cat` PTY so the session counts as running.
        let client = crate::pty::PtyClient::spawn_with_env(
            "cat",
            &[],
            worktree.path(),
            24,
            80,
            engine.config.ui.agent_scrollback_lines,
            &[],
        )
        .expect("spawn cat provider");
        engine.providers.insert("s1".to_string(), client);

        let outcome = engine
            .apply_wire(WireCommand::ChangeAgentProvider {
                session_id: "s1".to_string(),
                provider: "codex".to_string(),
            })
            .expect("apply_wire");
        let status = outcome.status.expect("running swap surfaces a status");
        assert_eq!(status.tone, "warning");
        assert!(
            status.message.contains("still running") && status.message.contains("codex"),
            "msg: {}",
            status.message
        );
        // Provider is persisted to the new one; the pin keeps the old one for labels.
        assert_eq!(engine.sessions[0].provider.as_str(), "codex");
        assert_eq!(
            engine.running_provider_pins.get("s1").map(|p| p.as_str()),
            Some("claude")
        );

        // Clean up so the PTY doesn't outlive the test.
        engine.providers.remove("s1");
    }
}
