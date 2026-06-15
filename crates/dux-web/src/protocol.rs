//! WebSocket message envelopes between the browser and the server. Control messages
//! are JSON text frames; raw PTY bytes travel as binary frames (input client->server,
//! output server->client) and are not modeled here.

use serde::{Deserialize, Serialize};

use dux_core::wire::WireStatus;

/// Browser -> server (JSON text frames). PTY input is sent as binary frames instead.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Fire a wire command. `command` is the `WireCommand` tag (e.g. "stage_file") and
    /// `args` its arguments; the server reconstructs a `dux_core::wire::WireCommand`
    /// from `{ "command": command, "args": args }`.
    Command {
        command: String,
        args: serde_json::Value,
    },
    /// Start streaming a session's PTY to this connection.
    Subscribe { session_id: String },
    /// Resize the subscribed session's PTY.
    Resize {
        session_id: String,
        rows: u16,
        cols: u16,
    },
    /// Start streaming an existing companion terminal's PTY to this connection.
    SubscribeTerminal { terminal_id: String },
    /// Create a new companion terminal for a session (distinct from its agent).
    CreateTerminal { session_id: String },
    /// List subdirectories of a server-side path so the client can pick a git
    /// repo to add as a project. `None` starts at the user's `$HOME`.
    BrowseDir { path: Option<String> },
    /// Request a freshly generated two-word "pet" agent name. The reply is an
    /// `AgentName` frame. Generation is pure, so the server answers directly
    /// without touching the engine thread.
    GenerateAgentName,
    /// List a project's managed git worktrees so the client can adopt an
    /// orphaned one as a new agent (the TUI's `new-agent-from-worktree`). The
    /// reply is a `ProjectWorktrees` frame. The server resolves the project from
    /// the engine (a cheap lookup) then classifies the worktrees in
    /// `spawn_blocking` (it shells to git), following the `browse_dir` precedent.
    ListProjectWorktrees { project_id: String },
    /// Inspect a candidate project path's branch BEFORE it is added, mirroring
    /// the TUI add flow's pre-flight (`add_project` runs `current_branch` +
    /// `branch_warning_kind` and shows the `ConfirmNonDefaultBranch` prompt when
    /// the repo is on a non-default branch). The reply is a
    /// `ProjectPathInspection` frame. The path is not a registered project yet,
    /// so no engine state is needed: the server runs the bounded plumbing reads
    /// directly in `spawn_blocking` (the `browse_dir` precedent).
    InspectProjectPath { path: String },
}

/// The branch-warning classification for a candidate project path, in
/// serializable form. Mirrors `dux_core::worker::BranchWarningKind`:
/// `Known` carries the resolved default branch; `Heuristic` means dux can't
/// confidently identify the default branch. Absence (`None` on the reply)
/// means the repo is already on its default branch — no warning.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum BranchWarningView {
    Known { default_branch: String },
    Heuristic,
}

/// A single managed-worktree candidate in serializable form, derived from
/// `dux_core::project_browser::classify_project_worktrees`. Only worktrees
/// managed by dux are listed (external worktrees are the TUI's separate fork
/// flow). `adoptable` is true when the worktree has no live agent and can be
/// attached; when false, `reason` explains why (currently: it already has an
/// agent), so the client can show it disabled rather than hiding it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProjectWorktreeEntryView {
    pub worktree_path: String,
    pub branch_name: String,
    pub adoptable: bool,
    pub reason: Option<String>,
}

/// A single directory-browser entry in serializable form (the engine's
/// `BrowserEntry` is not `Serialize`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DirEntryView {
    pub path: String,
    pub label: String,
    pub is_git_repo: bool,
}

/// Server -> browser (JSON text frames). PTY bytes are sent as separate binary frames.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Full ViewModel snapshot as raw JSON (already projected by the engine).
    ViewModel { data: serde_json::Value },
    /// Result of a command: a status tone+message, or an error string.
    CommandResult {
        status: Option<WireStatus>,
        error: Option<String>,
    },
    /// A subscription was accepted (the repaint follows as the first binary frame).
    Subscribed { session_id: String },
    /// A companion terminal was created for the given session.
    TerminalCreated {
        session_id: String,
        terminal_id: String,
    },
    /// An error not tied to a specific command.
    Error { message: String },
    /// An asynchronous status/lifecycle event not tied to a specific command:
    /// a background push/pull completing, an agent launch failing, or a PTY
    /// exiting. Same tone+message shape as a command result's status.
    Status { tone: String, message: String },
    /// An AI-generated commit message produced by a one-shot provider run,
    /// pushed asynchronously after a `generate_commit_message` command. The
    /// `session_id` scopes the result to the dialog that requested it so the
    /// frontend never fills the wrong session's draft (two open dialogs or a
    /// rapid switch). Broadcast to every client; each one routes by session id.
    CommitMessage { session_id: String, message: String },
    /// The most recently generated commit message, delivered ONCE on connect so a
    /// client that reconnected after generation completed (or in the
    /// connect/subscribe gap) still receives it. Distinct from `CommitMessage`
    /// (the live push) so the client applies it conservatively — only when the
    /// matching session's commit dialog is open AND its draft is still empty,
    /// never clobbering an in-progress edit. Not sent when no message exists yet.
    CommitMessageSnapshot { session_id: String, message: String },
    /// Response to `BrowseDir`: the resolved directory and its entries, or an
    /// error string when the listing failed.
    DirEntries {
        path: String,
        entries: Vec<DirEntryView>,
        error: Option<String>,
    },
    /// Response to `GenerateAgentName`: a freshly generated two-word pet name the
    /// client fills into the new-agent dialog's input.
    AgentName { name: String },
    /// Response to `ListProjectWorktrees`: the project's managed worktree
    /// candidates, or an error string when the listing failed (unknown project
    /// or a git failure).
    ProjectWorktrees {
        project_id: String,
        entries: Vec<ProjectWorktreeEntryView>,
        error: Option<String>,
    },
    /// Response to `InspectProjectPath`: the candidate repo's current branch and
    /// its branch warning (`None` when already on the default branch), or an
    /// error string when the inspection failed (not a git repo, detached HEAD,
    /// etc.). `path` echoes the request so a late reply can be matched to (or
    /// discarded for) the currently selected repo.
    ProjectPathInspection {
        path: String,
        current_branch: Option<String>,
        warning: Option<BranchWarningView>,
        error: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscribe_message_round_trips() {
        let msg = ClientMessage::Subscribe {
            session_id: "s1".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"subscribe","session_id":"s1"}"#);
        let back: ClientMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(msg, back);
    }

    #[test]
    fn command_message_parses_with_nested_args() {
        let json = r#"{"type":"command","command":"toggle_agent_auto_reopen","args":{"session_id":"s1","enabled":true}}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        match msg {
            ClientMessage::Command { command, args } => {
                assert_eq!(command, "toggle_agent_auto_reopen");
                assert_eq!(args["session_id"], "s1");
                assert_eq!(args["enabled"], true);
                // The server reconstructs a WireCommand from this shape.
                let envelope = serde_json::json!({ "command": command, "args": args });
                let wire: dux_core::wire::WireCommand =
                    serde_json::from_value(envelope).expect("reconstruct wire command");
                assert_eq!(
                    wire,
                    dux_core::wire::WireCommand::ToggleAgentAutoReopen {
                        session_id: "s1".to_string(),
                        enabled: true,
                    }
                );
            }
            _ => panic!("expected Command variant"),
        }
    }

    #[test]
    fn commit_message_serializes() {
        let msg = ServerMessage::CommitMessage {
            session_id: "s1".to_string(),
            message: "Fix the thing".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json,
            r#"{"type":"commit_message","session_id":"s1","message":"Fix the thing"}"#
        );
    }

    #[test]
    fn commit_message_snapshot_serializes() {
        // The connect-snapshot frame must carry its own `type` tag so the client
        // routes it to the conservative (empty-draft-only) handler, distinct from
        // the live `commit_message` push. This tag is what `ws.ts` switches on.
        let msg = ServerMessage::CommitMessageSnapshot {
            session_id: "s1".to_string(),
            message: "Fix the thing".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json,
            r#"{"type":"commit_message_snapshot","session_id":"s1","message":"Fix the thing"}"#
        );
    }

    #[test]
    fn browse_dir_message_parses_with_and_without_path() {
        let with = r#"{"type":"browse_dir","path":"/some/dir"}"#;
        let msg: ClientMessage = serde_json::from_str(with).unwrap();
        assert_eq!(
            msg,
            ClientMessage::BrowseDir {
                path: Some("/some/dir".to_string()),
            }
        );

        let without = r#"{"type":"browse_dir"}"#;
        let msg: ClientMessage = serde_json::from_str(without).unwrap();
        assert_eq!(msg, ClientMessage::BrowseDir { path: None });
    }

    #[test]
    fn generate_agent_name_message_parses() {
        let json = r#"{"type":"generate_agent_name"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg, ClientMessage::GenerateAgentName);
    }

    #[test]
    fn agent_name_message_serializes() {
        let msg = ServerMessage::AgentName {
            name: "happy-otter".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"agent_name","name":"happy-otter"}"#);
    }

    #[test]
    fn list_project_worktrees_message_parses() {
        let json = r#"{"type":"list_project_worktrees","project_id":"p1"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert_eq!(
            msg,
            ClientMessage::ListProjectWorktrees {
                project_id: "p1".to_string(),
            }
        );
    }

    #[test]
    fn project_worktrees_message_serializes() {
        let msg = ServerMessage::ProjectWorktrees {
            project_id: "p1".to_string(),
            entries: vec![ProjectWorktreeEntryView {
                worktree_path: "/wt/feat".to_string(),
                branch_name: "feat".to_string(),
                adoptable: true,
                reason: None,
            }],
            error: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"project_worktrees""#), "{json}");
        assert!(json.contains(r#""adoptable":true"#), "{json}");
    }

    #[test]
    fn inspect_project_path_message_parses() {
        let json = r#"{"type":"inspect_project_path","path":"/repo"}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert_eq!(
            msg,
            ClientMessage::InspectProjectPath {
                path: "/repo".to_string(),
            }
        );
    }

    #[test]
    fn project_path_inspection_known_serializes() {
        let msg = ServerMessage::ProjectPathInspection {
            path: "/repo".to_string(),
            current_branch: Some("feature/x".to_string()),
            warning: Some(BranchWarningView::Known {
                default_branch: "main".to_string(),
            }),
            error: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains(r#""type":"project_path_inspection""#),
            "{json}"
        );
        assert!(json.contains(r#""kind":"known""#), "{json}");
        assert!(json.contains(r#""default_branch":"main""#), "{json}");
        assert!(json.contains(r#""current_branch":"feature/x""#), "{json}");
    }

    #[test]
    fn project_path_inspection_heuristic_serializes() {
        let msg = ServerMessage::ProjectPathInspection {
            path: "/repo".to_string(),
            current_branch: Some("dev".to_string()),
            warning: Some(BranchWarningView::Heuristic),
            error: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""kind":"heuristic""#), "{json}");
    }

    #[test]
    fn project_path_inspection_no_warning_serializes_null() {
        let msg = ServerMessage::ProjectPathInspection {
            path: "/repo".to_string(),
            current_branch: Some("main".to_string()),
            warning: None,
            error: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""warning":null"#), "{json}");
    }

    #[test]
    fn resize_message_parses() {
        let json = r#"{"type":"resize","session_id":"s1","rows":40,"cols":120}"#;
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(
            msg,
            ClientMessage::Resize {
                rows: 40,
                cols: 120,
                ..
            }
        ));
    }
}
