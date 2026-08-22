//! One bounded subprocess runner: spawn, drain both pipes on their own threads,
//! wait with a hard wall-clock cap, and kill and reap anything still running.
//!
//! Every place dux shells out to a third-party CLI whose output it reads needs
//! exactly this, and needs it to be impossible for that CLI to park a worker
//! thread forever: a wedged credential helper, a hung network call, a daemon
//! that stopped answering. `gh` needed it first; the Tailscale watcher needs it
//! for the same reason, only more so, because a suspended-and-resumed
//! `tailscaled` is precisely the situation the watcher exists to survive.
//!
//! This is deliberately NOT `git::wait_child_or_kill`, which pipes only a tiny
//! stderr and never drains stdout, so it cannot be used where the output is the
//! answer.

use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// How long to wait for the output-reader threads after the child is gone before
/// abandoning them. They finish on their own once the pipe closes; the bound is
/// there so a grandchild holding the pipe open can never freeze the caller.
pub const DEFAULT_READER_DRAIN: Duration = Duration::from_secs(2);

/// Outcome of a bounded invocation. `Failed` carries the failure text (a spawn or
/// wait error) so callers can log the real cause instead of conflating it with a
/// timeout.
#[derive(Debug)]
pub enum CommandOutcome {
    /// The child exited on its own; the output is whatever was drained.
    Completed(Output),
    /// The wall-clock cap elapsed first. The child has been killed and reaped.
    TimedOut,
    /// The child could not be spawned, or waiting on it failed.
    Failed(String),
}

/// Run `cmd` with piped stdout/stderr drained on threads and a hard wall-clock
/// cap. On every non-[`CommandOutcome::Completed`] exit the child is killed and
/// reaped, the reader threads are drained with a bounded wait and then abandoned
/// (they self-terminate at EOF), so the caller can never block.
///
/// `label` names the program in failure text (e.g. `gh`, `tailscale`); it is only
/// used for messages.
pub fn run_command_with_timeout(
    mut cmd: Command,
    timeout: Duration,
    reader_drain: Duration,
    label: &str,
) -> CommandOutcome {
    let mut child = match cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => return CommandOutcome::Failed(err.to_string()),
    };

    // Drain the pipes on their own threads (so a full pipe buffer can't wedge the
    // child) and hand each buffer back over a channel, so we can wait for them
    // with a deadline and abandon them if a grandchild keeps the pipe open.
    let (out_tx, out_rx) = std::sync::mpsc::channel();
    let (err_tx, err_rx) = std::sync::mpsc::channel();
    match (child.stdout.take(), child.stderr.take()) {
        (Some(mut out), Some(mut err)) => {
            std::thread::spawn(move || {
                use std::io::Read;
                let mut buf = Vec::new();
                let _ = out.read_to_end(&mut buf);
                let _ = out_tx.send(buf);
            });
            std::thread::spawn(move || {
                use std::io::Read;
                let mut buf = Vec::new();
                let _ = err.read_to_end(&mut buf);
                let _ = err_tx.send(buf);
            });
        }
        _ => {
            // Should be unreachable (we just set piped stdio), but never leak the
            // spawned child if a pipe handle is somehow missing.
            let _ = child.kill();
            let _ = child.wait();
            return CommandOutcome::Failed(format!("{label} stdout/stderr pipe unavailable"));
        }
    }

    let drain =
        |rx: &std::sync::mpsc::Receiver<Vec<u8>>| rx.recv_timeout(reader_drain).unwrap_or_default();

    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return CommandOutcome::Completed(Output {
                    status,
                    stdout: drain(&out_rx),
                    stderr: drain(&err_rx),
                });
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    // Best-effort drain; the readers unblock once the killed
                    // child's pipes close.
                    let _ = drain(&out_rx);
                    let _ = drain(&err_rx);
                    return CommandOutcome::TimedOut;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(err) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = drain(&out_rx);
                let _ = drain(&err_rx);
                return CommandOutcome::Failed(format!("waiting for {label} failed: {err}"));
            }
        }
    }
}

/// How often the wait loop asks whether the child is gone. Small enough that a
/// fast command is not noticeably delayed, large enough that a long timeout
/// costs a handful of wakeups per second and nothing else.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_completed_command_returns_its_output() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf hello; printf oops >&2"]);
        match run_command_with_timeout(cmd, Duration::from_secs(5), DEFAULT_READER_DRAIN, "sh") {
            CommandOutcome::Completed(output) => {
                assert!(output.status.success());
                assert_eq!(String::from_utf8_lossy(&output.stdout), "hello");
                assert_eq!(String::from_utf8_lossy(&output.stderr), "oops");
            }
            other => panic!("expected a completed run, got {other:?}"),
        }
    }

    #[test]
    fn a_command_that_outlives_the_cap_times_out_and_is_reaped() {
        // A real sleeping child, killed by the cap. Deliberately a genuine
        // subprocess rather than a mock: the contract being pinned is that the
        // caller returns promptly, which is a property of the kill, not of the
        // bookkeeping.
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "sleep 30"]);
        let start = Instant::now();
        let outcome =
            run_command_with_timeout(cmd, Duration::from_millis(200), DEFAULT_READER_DRAIN, "sh");
        assert!(
            matches!(outcome, CommandOutcome::TimedOut),
            "expected a timeout, got {outcome:?}"
        );
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "the cap must return promptly, took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn a_missing_program_fails_rather_than_timing_out() {
        let cmd = Command::new("dux-no-such-program-9f3a");
        let outcome =
            run_command_with_timeout(cmd, Duration::from_secs(5), DEFAULT_READER_DRAIN, "nope");
        assert!(
            matches!(outcome, CommandOutcome::Failed(_)),
            "a spawn failure is Failed, never TimedOut: {outcome:?}"
        );
    }
}
