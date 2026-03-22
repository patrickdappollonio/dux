use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use serde_json::{Value, json};

#[derive(Clone, Debug)]
pub struct ProviderEvent {
    pub session_id: String,
    pub message: String,
}

pub struct AcpClient {
    child: Child,
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Arc<Mutex<HashMap<u64, Sender<Value>>>>,
    next_id: AtomicU64,
}

impl AcpClient {
    pub fn spawn(
        command: &str,
        args: &[String],
        cwd: &Path,
        session_id: &str,
        tx: Sender<ProviderEvent>,
    ) -> Result<Self> {
        let mut child = Command::new(command)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to spawn provider command {command}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow!("provider stdin unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| anyhow!("provider stdout unavailable"))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("provider stderr unavailable"))?;
        let pending = Arc::new(Mutex::new(HashMap::<u64, Sender<Value>>::new()));

        spawn_stdout_reader(
            stdout,
            Arc::clone(&pending),
            tx.clone(),
            session_id.to_string(),
        );
        spawn_stderr_reader(stderr, tx, session_id.to_string());

        Ok(Self {
            child,
            stdin: Arc::new(Mutex::new(stdin)),
            pending,
            next_id: AtomicU64::new(1),
        })
    }

    pub fn initialize(&self) -> Result<Value> {
        self.request(
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientInfo": {
                    "name": "dux",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {
                    "fs": {},
                    "terminal": {}
                }
            }),
        )
    }

    pub fn new_session(&self, cwd: &Path) -> Result<String> {
        let response = self.request(
            "session/new",
            json!({
                "cwd": cwd.to_string_lossy(),
            }),
        )?;
        session_id_from_response(&response)
    }

    pub fn load_session(&self, cwd: &Path, acp_session_id: &str) -> Result<String> {
        let response = self.request(
            "session/load",
            json!({
                "cwd": cwd.to_string_lossy(),
                "sessionId": acp_session_id,
            }),
        )?;
        session_id_from_response(&response)
    }

    pub fn prompt(
        &self,
        acp_session_id: String,
        prompt: String,
        tx: Sender<ProviderEvent>,
        app_session_id: String,
    ) {
        let stdin = Arc::clone(&self.stdin);
        let pending = Arc::clone(&self.pending);
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        thread::spawn(move || {
            let (response_tx, response_rx) = mpsc::channel();
            pending
                .lock()
                .expect("acp pending map")
                .insert(id, response_tx);
            let payload = json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "session/prompt",
                "params": {
                    "sessionId": acp_session_id,
                    "prompt": [
                        {
                            "type": "text",
                            "text": prompt
                        }
                    ]
                }
            });
            let body = format!("{payload}\n");
            if let Err(err) = stdin
                .lock()
                .expect("acp stdin lock")
                .write_all(body.as_bytes())
                .and_then(|_| stdin.lock().expect("acp stdin lock").flush())
            {
                let _ = tx.send(ProviderEvent {
                    session_id: app_session_id.clone(),
                    message: format!("failed to send prompt: {err}"),
                });
                return;
            }
            match response_rx.recv() {
                Ok(response) => {
                    let stop_reason = response
                        .get("result")
                        .and_then(|result| result.get("stopReason"))
                        .and_then(Value::as_str)
                        .unwrap_or("completed");
                    let _ = tx.send(ProviderEvent {
                        session_id: app_session_id,
                        message: format!("turn finished: {stop_reason}"),
                    });
                }
                Err(err) => {
                    let _ = tx.send(ProviderEvent {
                        session_id: app_session_id,
                        message: format!("prompt response failed: {err}"),
                    });
                }
            }
        });
    }

    pub fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        self.pending.lock().expect("acp pending map").insert(id, tx);
        let payload = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });
        let body = format!("{payload}\n");
        {
            let mut stdin = self.stdin.lock().expect("acp stdin lock");
            stdin.write_all(body.as_bytes())?;
            stdin.flush()?;
        }
        let response = rx
            .recv_timeout(Duration::from_secs(8))
            .with_context(|| format!("acp request {method} timed out waiting for a response"))?;
        if let Some(error) = response.get("error") {
            return Err(anyhow!("acp request {method} failed: {error}"));
        }
        Ok(response)
    }

    pub fn try_wait(&mut self) -> Result<Option<std::process::ExitStatus>> {
        self.child.try_wait().map_err(Into::into)
    }
}

fn session_id_from_response(response: &Value) -> Result<String> {
    response
        .get("result")
        .and_then(|result| result.get("sessionId"))
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .ok_or_else(|| anyhow!("acp response did not include sessionId"))
}

fn spawn_stdout_reader(
    stdout: std::process::ChildStdout,
    pending: Arc<Mutex<HashMap<u64, Sender<Value>>>>,
    tx: Sender<ProviderEvent>,
    app_session_id: String,
) {
    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<Value>(&line) {
                Ok(value) => {
                    if let Some(id) = value.get("id").and_then(Value::as_u64) {
                        if let Some(sender) = pending.lock().expect("acp pending map").remove(&id) {
                            let _ = sender.send(value);
                        }
                        continue;
                    }
                    if value.get("method").and_then(Value::as_str) == Some("session/update") {
                        if let Some(message) = summarize_session_update(&value) {
                            let _ = tx.send(ProviderEvent {
                                session_id: app_session_id.clone(),
                                message,
                            });
                        }
                    }
                }
                Err(_) => {
                    let _ = tx.send(ProviderEvent {
                        session_id: app_session_id.clone(),
                        message: line,
                    });
                }
            }
        }
    });
}

fn spawn_stderr_reader(
    stderr: std::process::ChildStderr,
    tx: Sender<ProviderEvent>,
    app_session_id: String,
) {
    thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines().map_while(Result::ok) {
            if line.trim().is_empty() {
                continue;
            }
            let _ = tx.send(ProviderEvent {
                session_id: app_session_id.clone(),
                message: format!("stderr: {line}"),
            });
        }
    });
}

fn summarize_session_update(value: &Value) -> Option<String> {
    let params = value.get("params")?;
    let update = params.get("update")?;
    let kind = update.get("sessionUpdate")?.as_str()?;
    match kind {
        "agent_message_chunk" => update
            .get("content")
            .and_then(|content| content.get("text"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        "agent_thought_chunk" => update
            .get("content")
            .and_then(|content| content.get("text"))
            .and_then(Value::as_str)
            .map(|text| format!("thought: {text}")),
        "tool_call" => update
            .get("title")
            .and_then(Value::as_str)
            .map(|title| format!("tool: {title}")),
        "tool_call_update" => update
            .get("rawOutput")
            .and_then(Value::as_str)
            .map(|output| format!("tool update: {output}")),
        "plan" => Some("plan updated".to_string()),
        "session_info_update" => update
            .get("title")
            .and_then(Value::as_str)
            .map(|title| format!("session: {title}")),
        "current_mode_update" => update
            .get("currentModeId")
            .and_then(Value::as_str)
            .map(|mode| format!("mode: {mode}")),
        _ => Some(format!("update: {kind}")),
    }
}
