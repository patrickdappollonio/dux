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
use crate::model::ProviderKind;
use crate::statusline::StatusTone;
use crate::worker::{ProjectPersistenceAction, PullTarget};

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

impl Engine {
    /// Reconstruct and dispatch a wire command, returning a wire-safe outcome.
    pub fn apply_wire(&mut self, command: WireCommand) -> anyhow::Result<WireCommandOutcome> {
        let core = self.wire_to_command(command)?;
        let reaction = self.apply(core)?;
        let mut status = wire_status_from_reaction(&reaction);
        if status.is_none() {
            status = self.drive_delete_followup(&reaction).into_iter().next();
        }
        Ok(WireCommandOutcome { status })
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
    use crate::engine::DeleteTerminalView;
    use crate::engine::test_support::{sample_project, sample_session, test_engine};
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
}
