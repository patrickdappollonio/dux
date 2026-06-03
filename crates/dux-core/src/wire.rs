//! Transport-agnostic command intake. A web client sends `{command, args}` JSON;
//! it deserializes into `WireCommand`, is reconstructed into the engine's
//! `Command` (looking up domain objects by id server-side), and dispatched
//! through the same `Engine::apply` the TUI uses. The result is downsampled to a
//! wire-safe `WireCommandOutcome` (the full `EventReaction` is engine-internal
//! and view-coupled; web clients re-fetch `view_model()` for fresh state).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::engine::{
    AgentLaunchFailedOutcome, BeginDeleteSessionOutcome, Command, Engine, EventReaction,
    FinishDeleteSessionOutcome, StatusUpdate, WorktreeRemoval,
};
use crate::statusline::StatusTone;

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
        })
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
}
