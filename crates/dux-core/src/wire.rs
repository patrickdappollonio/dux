//! Transport-agnostic command intake. A web client sends `{command, args}` JSON;
//! it deserializes into `WireCommand`, is reconstructed into the engine's
//! `Command` (looking up domain objects by id server-side), and dispatched
//! through the same `Engine::apply` the TUI uses. The result is downsampled to a
//! wire-safe `WireCommandOutcome` (the full `EventReaction` is engine-internal
//! and view-coupled; web clients re-fetch `view_model()` for fresh state).

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::engine::{Command, Engine, EventReaction, StatusUpdate};
use crate::statusline::StatusTone;

/// A command as received from a generic transport (e.g. the web WebSocket).
/// `#[serde(tag = "command", content = "args")]` matches the `{ "command": "...",
/// "args": { ... } }` envelope.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "command", content = "args", rename_all = "snake_case")]
pub enum WireCommand {
    StageFile { session_id: String, path: String },
    UnstageFile { session_id: String, path: String },
    CommitChanges { session_id: String, message: String },
    Push { session_id: String },
    ToggleAgentAutoReopen { session_id: String, enabled: bool },
}

/// A status-line update in wire-safe form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct WireStatus {
    /// "info" | "busy" | "warning" | "error"
    pub tone: String,
    pub message: String,
}

impl WireStatus {
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
        _ => None,
    }
}

impl Engine {
    /// Reconstruct and dispatch a wire command, returning a wire-safe outcome.
    pub fn apply_wire(&mut self, command: WireCommand) -> anyhow::Result<WireCommandOutcome> {
        let core = self.wire_to_command(command)?;
        let reaction = self.apply(core)?;
        Ok(WireCommandOutcome {
            status: wire_status_from_reaction(&reaction),
        })
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
    use crate::engine::test_support::{sample_session, test_engine};
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
