//! Changes that finish before the command does. Every change is sent with
//! `?operation=1`, which answers its operation's id at once; the client then
//! reads the record by that id in short waits until it has an outcome.
//!
//! A wait read can end early for reasons that are not an outcome: a dux
//! handing its engine from one servicing core to another answers every
//! waiting read with the record as it stands, and a connection can drop. In
//! both cases the record is read again by the same id, so the change's
//! outcome is never lost. A wait that runs out says the outcome is unknown;
//! it never claims the change stopped.

use std::path::Path;
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::connect::Client;
use super::transport::{Method, Request};
use super::{CliError, Exit};

/// The longest one read of a record waits for an outcome, which is the
/// longest the API holds one.
pub const READ_WAIT: Duration = Duration::from_secs(25);

/// How long to pause before reading again after a read got no reply.
const RETRY_PAUSE: Duration = Duration::from_millis(250);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordState {
    Running,
    Succeeded,
    Failed,
    Partial,
    Unknown,
}

impl RecordState {
    pub fn is_final(self) -> bool {
        !matches!(self, RecordState::Running | RecordState::Unknown)
    }

    fn word(self) -> &'static str {
        match self {
            RecordState::Running => "running",
            RecordState::Succeeded => "succeeded",
            RecordState::Failed => "failed",
            RecordState::Partial => "partial",
            RecordState::Unknown => "unknown",
        }
    }
}

/// One piece of a change and what became of it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct RecordPart {
    pub part: String,
    pub subject: String,
    pub outcome: String,
    #[serde(default)]
    pub reason: Option<String>,
}

/// An operation record as the API answers it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct OperationRecord {
    pub id: String,
    pub kind: String,
    pub state: RecordState,
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub created: Vec<String>,
    #[serde(default)]
    pub removed: Vec<String>,
    #[serde(default)]
    pub parts: Vec<RecordPart>,
}

/// A change the dux refused, with the operation it named as in the way, when
/// the refusal named one.
pub struct Refused {
    pub error: CliError,
    pub operation: Option<String>,
}

impl From<CliError> for Refused {
    fn from(error: CliError) -> Self {
        Self {
            error,
            operation: None,
        }
    }
}

impl Client {
    /// Send a change as an operation and return its record as the change
    /// answered it: already final when the change finished inside the call.
    pub fn change(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
    ) -> Result<OperationRecord, CliError> {
        self.try_change(method, path, body, None)
            .map_err(|refused| refused.error)
    }

    /// [`Self::change`], keeping the operation a refusal names, and giving the request
    /// only `timeout` to answer.
    pub fn try_change(
        &self,
        method: Method,
        path: &str,
        body: Option<serde_json::Value>,
        timeout: Option<Duration>,
    ) -> Result<OperationRecord, Refused> {
        let separator = if path.contains('?') { '&' } else { '?' };
        let reply = self.send(Request {
            method,
            path: format!("{path}{separator}operation=1"),
            body: body.map(|json| json.to_string().into_bytes()),
            bearer: None,
            timeout,
        })?;
        if reply.status != 202 {
            let operation = serde_json::from_slice::<serde_json::Value>(&reply.body)
                .ok()
                .and_then(|json| json.get("operation")?.as_str().map(str::to_string));
            return Err(Refused {
                error: self.refusal(&reply),
                operation,
            });
        }
        Ok(self.parse(&reply)?)
    }

    /// The record `id` as it stands, without waiting.
    pub fn operation(&self, id: &str) -> Result<OperationRecord, CliError> {
        let reply = self.send(Request::get(operation_path(id, 0)))?;
        match reply.status {
            200 => self.parse(&reply),
            404 => Err(CliError::new(
                Exit::Failed,
                format!(
                    "{} knows no operation {id}; a finished one is kept for \
                     [server] operation_retention_seconds, and none survive a restart",
                    self.target()
                ),
            )),
            _ => Err(self.refusal(&reply)),
        }
    }

    /// Read `record` again by its id until it has an outcome or `timeout` passes. A read
    /// that drops is retried by the same id; an outcome that arrived is reported, however late.
    ///
    /// # Errors
    ///
    /// [`Exit::Unknown`], naming the id, when `timeout` runs out first.
    pub fn wait(
        &self,
        record: OperationRecord,
        timeout: Duration,
    ) -> Result<OperationRecord, CliError> {
        let started = Instant::now();
        self.wait_loop(record, started, started + timeout, false)
    }

    /// Wait for somebody else's operation `id`, until `deadline`, as [`Self::wait`] does, except
    /// that an id the dux no longer knows has finished and been forgotten (it counts as an
    /// outcome), and a server error is read again within the time rather than ending the wait.
    pub fn wait_for_other(&self, id: &str, deadline: Instant) -> Result<(), CliError> {
        let stand_in = OperationRecord {
            id: id.to_string(),
            kind: String::new(),
            state: RecordState::Running,
            message: String::new(),
            created: Vec::new(),
            removed: Vec::new(),
            parts: Vec::new(),
        };
        self.wait_loop(stand_in, Instant::now(), deadline, true)
            .map(|_| ())
    }

    fn wait_loop(
        &self,
        record: OperationRecord,
        started: Instant,
        deadline: Instant,
        other: bool,
    ) -> Result<OperationRecord, CliError> {
        let mut record = record;
        loop {
            if record.state.is_final() {
                return Ok(record);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(unknown_after(&record.id, started.elapsed()));
            }
            // The dux answers a wait at its end, so it is asked to end a
            // second before the time left, to leave the answer room to arrive.
            let wait = left
                .saturating_sub(Duration::from_secs(1))
                .min(READ_WAIT)
                .as_secs();
            let read = Request {
                timeout: Some(left),
                ..Request::get(operation_path(&record.id, wait))
            };
            match self.try_send(read) {
                Ok(reply) if reply.status == 200 => {
                    record = self.parse(&reply)?;
                    if wait == 0 && !record.state.is_final() {
                        std::thread::sleep(RETRY_PAUSE.min(left));
                    }
                }
                Ok(reply) if other && reply.status == 404 => return Ok(record),
                Ok(reply) if other && reply.status >= 500 => {
                    std::thread::sleep(RETRY_PAUSE.min(left));
                }
                Ok(reply) if reply.status == 404 => {
                    return Err(CliError::new(
                        Exit::Unknown,
                        format!(
                            "{} no longer knows operation {}, so how it ended is unknown; it may \
                             have restarted",
                            self.target(),
                            record.id
                        ),
                    ));
                }
                Ok(reply) => return Err(self.refusal(&reply)),
                Err(error @ super::transport::TransportError::Refused(_)) => {
                    return Err(self.no_reply(&error));
                }
                Err(_) => std::thread::sleep(
                    RETRY_PAUSE.min(deadline.saturating_duration_since(Instant::now())),
                ),
            }
        }
    }
}

/// What a path segment escapes: everything but the URL's unreserved marks.
const SEGMENT: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// `text` escaped to stand as one segment of a request path.
pub(super) fn segment(text: &str) -> String {
    percent_encoding::utf8_percent_encode(text, SEGMENT).to_string()
}

fn operation_path(id: &str, wait_seconds: u64) -> String {
    let id = segment(id);
    if wait_seconds == 0 {
        format!("/api/v1/operations/{id}")
    } else {
        format!("/api/v1/operations/{id}?wait_seconds={wait_seconds}")
    }
}

/// A wait that ran out after `waited`.
fn unknown_after(id: &str, waited: Duration) -> CliError {
    let seconds = waited.as_secs();
    let unit = if seconds == 1 { "second" } else { "seconds" };
    CliError::new(
        Exit::Unknown,
        format!(
            "operation {id} was still running after {seconds} {unit}, so its outcome is unknown; \
             it has not been stopped. Look it up with \"dux operations show {id}\""
        ),
    )
}

/// A record read without waiting that has no outcome yet.
fn still_running(id: &str) -> CliError {
    CliError::new(
        Exit::Unknown,
        format!(
            "operation {id} is still running, so its outcome is not known yet. Look it up again \
             with \"dux operations show {id}\""
        ),
    )
}

/// How long a change waits: `--wait-timeout` when given, else `[cli]
/// wait_timeout_seconds` from this machine's config file.
pub fn wait_timeout(flag: Option<u64>, config_path: &Path) -> Result<Duration, CliError> {
    if let Some(seconds) = flag {
        return Ok(Duration::from_secs(seconds));
    }
    let config = crate::config::load_config_file(config_path)
        .map_err(|error| CliError::new(Exit::Failed, error.to_string()))?;
    Ok(Duration::from_secs(config.cli.wait_timeout_seconds))
}

/// A finished record as the command ends on it: its sentence on success, or
/// the error a failed or partly done change exits with.
pub fn outcome(record: &OperationRecord) -> Result<String, CliError> {
    match record.state {
        RecordState::Succeeded => Ok(record.message.clone()),
        RecordState::Failed | RecordState::Partial => {
            Err(CliError::new(Exit::Failed, describe(record)))
        }
        RecordState::Running | RecordState::Unknown => Err(still_running(&record.id)),
    }
}

/// The exit code a record calls for; `None` when it succeeded.
pub fn exit_of(record: &OperationRecord) -> Option<Exit> {
    match record.state {
        RecordState::Succeeded => None,
        RecordState::Failed | RecordState::Partial => Some(Exit::Failed),
        RecordState::Running | RecordState::Unknown => Some(Exit::Unknown),
    }
}

/// A record as `dux operations show` prints it, one fact per line.
pub fn describe(record: &OperationRecord) -> String {
    let mut lines = vec![
        format!("id:       {}", record.id),
        format!("kind:     {}", record.kind),
        format!("state:    {}", record.state.word()),
    ];
    if !record.message.is_empty() {
        lines.push(format!("message:  {}", record.message));
    }
    if !record.created.is_empty() {
        lines.push(format!("created:  {}", record.created.join(", ")));
    }
    if !record.removed.is_empty() {
        lines.push(format!("removed:  {}", record.removed.join(", ")));
    }
    lines.extend(record.parts.iter().map(part_line));
    lines.join("\n")
}

/// One piece of a change and what became of it, as one line:
/// `branch: web refused (not merged)`.
pub fn part_line(part: &RecordPart) -> String {
    let mut line = format!("{}: {} {}", part.part, part.subject, part.outcome);
    if let Some(reason) = &part.reason {
        line.push_str(&format!(" ({reason})"));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::connect::{Target, connect};
    use crate::client::test_server::{FakeDux, Reply, private_dir};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const BUILD: &str = r#"{"version":"v1","process":"p","api":1}"#;
    const RUNNING: &str = r#"{"id":"op-7","kind":"agent.delete","state":"running","message":"","created":[],"removed":["a1"],"parts":[]}"#;
    const DONE: &str = r#"{"id":"op-7","kind":"agent.delete","state":"partial","message":"Deleted agent web; its branch was kept.","created":[],"removed":["a1"],"parts":[{"part":"worktree","subject":"/w/web","outcome":"removed"},{"part":"branch","subject":"web","outcome":"refused","reason":"not merged"}]}"#;

    /// A local stand-in dux whose operation reads go through `reads`.
    fn local(
        dir: &Path,
        reads: impl Fn(usize, &str) -> Reply + Send + Sync + 'static,
    ) -> (FakeDux, crate::lockfile::SingleInstanceLock, Client) {
        let socket = dir.join("dux.sock");
        let count = Arc::new(AtomicUsize::new(0));
        let fake = FakeDux::unix(&socket, move |seen| {
            if seen.path == "/api/v1/build" {
                return Reply::json(200, BUILD);
            }
            if seen.path.starts_with("/api/v1/operations/") {
                return reads(count.fetch_add(1, Ordering::SeqCst), &seen.path);
            }
            if seen.path.ends_with("operation=1") {
                return Reply::json(202, RUNNING);
            }
            Reply::json(404, "{}")
        });
        let lock_path = dir.join("dux.lock");
        let lock = crate::lockfile::SingleInstanceLock::acquire(&lock_path).unwrap();
        std::fs::write(
            &lock_path,
            format!(
                "{}\ncontrol-socket={}\n",
                std::process::id(),
                socket.display()
            ),
        )
        .unwrap();
        let client = connect(&Target::Local, &lock_path).unwrap();
        (fake, lock, client)
    }

    #[test]
    fn a_read_cut_short_or_dropped_is_read_again_by_the_same_id_until_it_ends() {
        let dir = private_dir();
        // Read 0 ends early with the record still running (a hand-over),
        // read 1 drops the connection, read 2 has the outcome.
        let (fake, _lock, client) = local(dir.path(), |n, _| match n {
            0 => Reply::json(200, RUNNING),
            1 => Reply::Close,
            _ => Reply::json(200, DONE),
        });
        let started = client
            .change(
                Method::Delete,
                "/api/v1/sessions/a1?delete_worktree=true",
                None,
            )
            .unwrap();
        assert_eq!(started.state, RecordState::Running);
        let finished = client.wait(started, Duration::from_secs(30)).unwrap();
        assert_eq!(finished.state, RecordState::Partial);
        let reads: Vec<String> = fake
            .seen()
            .into_iter()
            .filter(|s| s.path.starts_with("/api/v1/operations/"))
            .map(|s| s.path)
            .collect();
        assert_eq!(reads.len(), 3);
        assert!(
            reads
                .iter()
                .all(|p| p.starts_with("/api/v1/operations/op-7?wait_seconds=")),
            "{reads:?}"
        );
        let change = fake
            .seen()
            .into_iter()
            .find(|s| s.method == "DELETE")
            .unwrap();
        assert_eq!(
            change.path,
            "/api/v1/sessions/a1?delete_worktree=true&operation=1"
        );

        let ended = outcome(&finished).unwrap_err();
        assert_eq!(ended.exit, Exit::Failed);
        assert!(
            ended
                .message
                .contains("Deleted agent web; its branch was kept.")
        );
        assert!(
            ended.message.contains("branch: web refused (not merged)"),
            "{}",
            ended.message
        );
    }

    #[test]
    fn a_wait_that_runs_out_is_unknown_and_names_the_id() {
        let dir = private_dir();
        // The first read is never answered: the wait still ends on time.
        let (_fake, _lock, client) = local(dir.path(), |n, _| match n {
            0 => Reply::Hang(Duration::from_secs(10)),
            _ => Reply::json(200, RUNNING),
        });
        let started = client
            .change(Method::Post, "/api/v1/sessions/a1/kill", None)
            .unwrap();
        let began = Instant::now();
        let error = client.wait(started, Duration::from_secs(2)).unwrap_err();
        assert!(
            began.elapsed() < Duration::from_secs(4),
            "every read is bounded by the time left: {:?}",
            began.elapsed()
        );
        assert_eq!(error.exit, Exit::Unknown);
        assert!(
            error.message.contains("after 2 seconds"),
            "{}",
            error.message
        );
        assert!(
            error.message.contains("dux operations show op-7"),
            "{}",
            error.message
        );
        assert!(
            error.message.contains("has not been stopped"),
            "{}",
            error.message
        );
    }

    #[test]
    fn a_change_the_dux_refuses_exits_with_its_sentence() {
        let dir = private_dir();
        let socket = dir.path().join("dux.sock");
        let _fake = FakeDux::unix(&socket, |seen| match seen.path.as_str() {
            "/api/v1/build" => Reply::json(200, BUILD),
            "/api/v1/sessions/a2?operation=1" => Reply::json(
                409,
                r#"{"error":"attached","blockers":[{"surface":"browser","device":null,"address":"192.168.1.5","verified":true,"driving":true,"target":{"kind":"tab","id":"t9","agent":"a2"}}]}"#,
            ),
            _ => Reply::json(
                409,
                r#"{"error":"busy","message":"op-1 is still deleting web"}"#,
            ),
        });
        let lock_path = dir.path().join("dux.lock");
        let _lock = crate::lockfile::SingleInstanceLock::acquire(&lock_path).unwrap();
        std::fs::write(
            &lock_path,
            format!(
                "{}\ncontrol-socket={}\n",
                std::process::id(),
                socket.display()
            ),
        )
        .unwrap();
        let client = connect(&Target::Local, &lock_path).unwrap();
        let error = client
            .change(Method::Delete, "/api/v1/sessions/a1", None)
            .unwrap_err();
        assert_eq!(error.exit, Exit::Refused);
        assert_eq!(error.message, "op-1 is still deleting web");

        // Somebody attached: who, and how to go ahead over them.
        let error = client
            .change(Method::Delete, "/api/v1/sessions/a2", None)
            .unwrap_err();
        assert_eq!(error.exit, Exit::Refused);
        assert!(
            error
                .message
                .contains("a browser at 192.168.1.5, typing in tab t9"),
            "{}",
            error.message
        );
        assert!(
            error.message.contains("--dangerously-ignore-connected"),
            "{}",
            error.message
        );
        assert!(!error.message.contains("{"), "{}", error.message);
    }

    #[test]
    fn a_record_is_shown_with_its_parts_and_an_unknown_id_fails() {
        let dir = private_dir();
        let (_fake, _lock, client) = local(dir.path(), |_, path| {
            if path.contains("op-7") {
                Reply::json(200, DONE)
            } else {
                Reply::json(404, r#"{"error":"unknown_operation"}"#)
            }
        });
        let shown = describe(&client.operation("op-7").unwrap());
        assert!(shown.contains("op-7"), "{shown}");
        assert!(shown.contains("partial"), "{shown}");
        assert!(shown.contains("worktree: /w/web removed"), "{shown}");
        assert_eq!(
            exit_of(&client.operation("op-7").unwrap()),
            Some(Exit::Failed)
        );
        let running: OperationRecord = serde_json::from_str(RUNNING).unwrap();
        assert_eq!(exit_of(&running), Some(Exit::Unknown));
        let still = outcome(&running).unwrap_err();
        assert_eq!(still.exit, Exit::Unknown);
        assert!(!still.message.contains("0 seconds"), "{}", still.message);
        assert!(still.message.contains("still running"), "{}", still.message);
        let missing = client.operation("op-9").unwrap_err();
        assert_eq!(missing.exit, Exit::Failed);
        assert!(missing.message.contains("op-9"));
    }

    #[test]
    fn the_wait_timeout_comes_from_the_flag_else_the_config_file() {
        let dir = private_dir();
        let config = dir.path().join("config.toml");
        assert_eq!(
            wait_timeout(Some(5), &config).unwrap(),
            Duration::from_secs(5)
        );
        assert_eq!(
            wait_timeout(None, &config).unwrap(),
            Duration::from_secs(600)
        );
        std::fs::write(&config, "[cli]\nwait_timeout_seconds = 42\n").unwrap();
        assert_eq!(
            wait_timeout(None, &config).unwrap(),
            Duration::from_secs(42)
        );
    }
}
