use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

fn main() {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines().map_while(Result::ok) {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let id = message.get("id").cloned().unwrap_or(json!(null));
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match method {
            "initialize" => {
                writeln!(
                    stdout,
                    "{}",
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "protocolVersion": 1,
                            "capabilities": {
                                "session": {
                                    "list": {}
                                },
                                "loadSession": {}
                            }
                        }
                    })
                )
                .ok();
            }
            "session/new" => {
                writeln!(
                    stdout,
                    "{}",
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "sessionId": "mock-session"
                        }
                    })
                )
                .ok();
            }
            "session/load" => {
                writeln!(
                    stdout,
                    "{}",
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "sessionId": "mock-session"
                        }
                    })
                )
                .ok();
            }
            "session/prompt" => {
                let session_id = message["params"]["sessionId"]
                    .as_str()
                    .unwrap_or("mock-session");
                writeln!(
                    stdout,
                    "{}",
                    json!({
                        "jsonrpc": "2.0",
                        "method": "session/update",
                        "params": {
                            "sessionId": session_id,
                            "update": {
                                "sessionUpdate": "agent_message_chunk",
                                "content": {
                                    "type": "text",
                                    "text": "mock reply"
                                }
                            }
                        }
                    })
                )
                .ok();
                writeln!(
                    stdout,
                    "{}",
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "stopReason": "end_turn"
                        }
                    })
                )
                .ok();
            }
            _ => {
                writeln!(
                    stdout,
                    "{}",
                    json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": -32601,
                            "message": "method not found"
                        }
                    })
                )
                .ok();
            }
        }
        stdout.flush().ok();
    }
}
