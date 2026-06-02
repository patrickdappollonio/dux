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
