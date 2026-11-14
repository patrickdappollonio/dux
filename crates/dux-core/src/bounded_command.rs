//! One bounded subprocess runner: spawn, drain both pipes on their own threads,
//! wait with a hard wall-clock cap, and kill and reap anything still running.
//!
//! Every place dux shells out to a third-party CLI whose output it reads needs
//! exactly this, and needs it to be impossible for that CLI to park a worker
//! thread forever: a wedged credential helper, a hung network call, a daemon
//! that stopped answering.
//!
//! Deliberately NOT `git::wait_child_or_kill`, which pipes only a tiny stderr
//! and never drains stdout, so it cannot be used where the output is the answer.

use std::collections::BTreeSet;
use std::os::unix::process::CommandExt as _;
use std::process::{Command, Output, Stdio};
use std::sync::{Mutex, TryLockError};
use std::time::{Duration, Instant};

/// The process groups of the children [`run_command_with_timeout`] is running
/// right now, by leader pid. A child in a group of its own gets none of the
/// terminal's Ctrl-C or hangup, so without this a wedged one keeps running
/// after dux exits; [`kill_live_child_groups`] ends them on dux's way out.
static LIVE_CHILD_GROUPS: Mutex<BTreeSet<i32>> = Mutex::new(BTreeSet::new());

/// Kill every process group a bounded call is running right now, on dux's way
/// out (a quit, a forced exit, a panic that ends the terminal UI), so a wedged
/// git, gh or tailscale call and whatever it started do not outlive dux.
///
/// Safe in a panic hook: it never blocks on the registry (a call holding it
/// for its insert or remove is skipped, as that child is ending anyway) and
/// allocates nothing.
pub fn kill_live_child_groups() {
    kill_groups_in(&LIVE_CHILD_GROUPS);
}

/// [`kill_live_child_groups`] over `registry`.
fn kill_groups_in(registry: &Mutex<BTreeSet<i32>>) {
    let groups = match registry.try_lock() {
        Ok(groups) => groups,
        Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
        Err(TryLockError::WouldBlock) => return,
    };
    for &leader in groups.iter() {
        if let Some(pid) = rustix::process::Pid::from_raw(leader) {
            let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
        }
    }
}

/// A child's group in [`LIVE_CHILD_GROUPS`] for as long as this lives. Dropped
/// right after the leader is reaped, so a pid the system hands out again is
/// never killed in its place.
struct LiveChildGroup(Option<i32>);

impl LiveChildGroup {
    /// Register `child`'s group when it leads one of its own (every
    /// [`ChildGroup`] arranges that); a child sharing dux's group is not
    /// registered, so dux never kills its own group.
    fn register(child: &std::process::Child) -> Self {
        let leader = child.id().cast_signed();
        let leads_its_group = rustix::process::Pid::from_raw(leader)
            .is_some_and(|pid| rustix::process::getpgid(Some(pid)).is_ok_and(|group| group == pid));
        if !leads_its_group {
            return Self(None);
        }
        LIVE_CHILD_GROUPS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(leader);
        Self(Some(leader))
    }
}

impl Drop for LiveChildGroup {
    fn drop(&mut self) {
        if let Some(leader) = self.0.take() {
            LIVE_CHILD_GROUPS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .remove(&leader);
        }
    }
}

/// Where the child's process group comes from. The cap kills the child's whole
/// group, so a program it started (a git hook, a per-submodule git, a
/// credential helper) cannot outlive the cap holding the pipes open; that only
/// works when the child leads a group of its own, so every caller says how it
/// gets one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChildGroup {
    /// The runner starts the child in a new process group of its own.
    New,
    /// The command already starts a session of its own (`setsid` in a
    /// `pre_exec` hook, as the no-prompt git command does), which makes it lead
    /// a group. The runner must not also give it one: a group leader cannot
    /// start a session, so the `setsid` would fail and the spawn with it.
    SessionSetByCaller,
}

/// The FLOOR on how long to wait for the output-reader threads once the child is
/// gone before abandoning them. They finish on their own when the pipe closes;
/// the bound stops a grandchild holding the pipe open from freezing the caller.
/// A floor rather than the whole allowance, because a reader that has already
/// read everything still has to be scheduled to hand its buffer over, and on a
/// loaded machine a short fixed window loses that race and silently returns
/// empty output. See [`run_command_with_timeout`] for the ceiling.
pub(crate) const DEFAULT_READER_DRAIN: Duration = Duration::from_secs(2);

/// Outcome of a bounded invocation. `Failed` carries the failure text (a spawn or
/// wait error) so callers can log the real cause instead of conflating it with a
/// timeout.
#[derive(Debug)]
pub(crate) enum CommandOutcome {
    /// The child exited on its own; the output is whatever was drained.
    Completed(Output),
    /// The wall-clock cap elapsed first. The child has been killed and reaped.
    TimedOut,
    /// The child could not be spawned for any reason but its program being
    /// absent (a permission refused, a resource exhausted, a pipe that could not
    /// be made), or waiting on it failed.
    Failed(String),
    /// The program is not there: spawning it failed with "not found". The one
    /// launch error that says something about what is installed.
    NotFound(String),
}

/// Run `cmd` with piped stdout/stderr drained on threads and a hard wall-clock
/// cap. On every non-[`CommandOutcome::Completed`] exit the child is killed and
/// reaped and the reader threads are drained with a bounded wait and then
/// abandoned, so the caller can never block.
///
/// A reader is waited on until the command's own cap runs out, or for
/// `reader_drain` when less than that is left, so the call returns within
/// roughly `timeout + reader_drain` however wedged the pipe is. Spending the
/// cap's leftover on the readers is what keeps a fast command's output from
/// being dropped on a loaded machine, where handing the buffer over is a
/// scheduling race rather than a wedge.
///
/// `group` says how the child comes to lead the process group the cap kills
/// (see [`ChildGroup`]). `label` names the program in failure text and is only
/// used for messages.
pub(crate) fn run_command_with_timeout(
    mut cmd: Command,
    group: ChildGroup,
    timeout: Duration,
    reader_drain: Duration,
    label: &str,
) -> CommandOutcome {
    match group {
        ChildGroup::New => {
            cmd.process_group(0);
        }
        ChildGroup::SessionSetByCaller => {}
    }
    let mut child = match cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return CommandOutcome::NotFound(err.to_string());
        }
        Err(err) => return CommandOutcome::Failed(err.to_string()),
    };
    let live = LiveChildGroup::register(&child);

    // Drain the pipes on their own threads (so a full pipe buffer can't wedge the
    // child) and hand each buffer back over a channel, so we can wait for them
    // with a deadline and abandon them if a grandchild keeps the pipe open.
    let (out_tx, out_rx) = std::sync::mpsc::channel();
    let (err_tx, err_rx) = std::sync::mpsc::channel();
    if let (Some(mut out), Some(mut err)) = (child.stdout.take(), child.stderr.take()) {
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
    } else {
        // Should be unreachable (we just set piped stdio), but never leak the
        // spawned child if a pipe handle is somehow missing.
        kill_with_group(&mut child);
        let _ = child.wait();
        drop(live);
        return CommandOutcome::Failed(format!("{label} stdout/stderr pipe unavailable"));
    }

    let start = Instant::now();
    let deadline = start + timeout;
    // Whatever is left of the command's own cap, and never less than the floor.
    // Both drains share the one deadline, so two wedged pipes cost the floor
    // once each on top of the cap rather than a multiple of it.
    let drain = |rx: &std::sync::mpsc::Receiver<Vec<u8>>| {
        let wait = deadline
            .saturating_duration_since(Instant::now())
            .max(reader_drain);
        rx.recv_timeout(wait).unwrap_or_default()
    };

    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                drop(live);
                return CommandOutcome::Completed(Output {
                    status,
                    stdout: drain(&out_rx),
                    stderr: drain(&err_rx),
                });
            }
            Ok(None) => {
                if start.elapsed() >= timeout {
                    kill_with_group(&mut child);
                    let _ = child.wait();
                    drop(live);
                    // Best-effort drain; the readers unblock once the killed
                    // child's pipes close.
                    let _ = drain(&out_rx);
                    let _ = drain(&err_rx);
                    return CommandOutcome::TimedOut;
                }
                std::thread::sleep(POLL_INTERVAL);
            }
            Err(err) => {
                kill_with_group(&mut child);
                let _ = child.wait();
                drop(live);
                let _ = drain(&out_rx);
                let _ = drain(&err_rx);
                return CommandOutcome::Failed(format!("waiting for {label} failed: {err}"));
            }
        }
    }
}

/// Kill `child`, and its whole process group when it leads one of its own
/// (every [`ChildGroup`] arranges that), so a program it started and left
/// running does not outlive the cap holding the pipes. The leader check stays
/// so a child that somehow shares dux's own group is never group-killed.
fn kill_with_group(child: &mut std::process::Child) {
    if let Some(pid) = rustix::process::Pid::from_raw(child.id().cast_signed())
        && rustix::process::getpgid(Some(pid)).is_ok_and(|group| group == pid)
    {
        let _ = rustix::process::kill_process_group(pid, rustix::process::Signal::KILL);
    }
    let _ = child.kill();
}

/// How often the wait loop asks whether the child is gone. Small enough that a
/// fast command is not noticeably delayed, large enough that a long timeout
/// costs a handful of wakeups per second and nothing else.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

#[cfg(test)]
mod tests {
    use super::*;

    /// A bounded call's group is in the registry while the call runs and
    /// gone once it returns.
    #[test]
    fn a_running_call_registers_its_group_until_it_returns() {
        let dir = crate::test_scratch::ScratchDir::new();
        let pid_file = dir.path().join("pid");
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg("echo $$ > \"$1\"; exec sleep 30")
            .arg("sh")
            .arg(&pid_file);
        let call = std::thread::spawn(move || {
            run_command_with_timeout(
                cmd,
                ChildGroup::New,
                Duration::from_secs(3),
                DEFAULT_READER_DRAIN,
                "sh",
            )
        });
        let deadline = Instant::now() + Duration::from_secs(3);
        let leader = loop {
            if let Some(pid) = std::fs::read_to_string(&pid_file)
                .ok()
                .and_then(|text| text.trim().parse::<i32>().ok())
            {
                break pid;
            }
            assert!(Instant::now() < deadline, "the child never started");
            std::thread::sleep(Duration::from_millis(20));
        };
        assert!(
            LIVE_CHILD_GROUPS.lock().unwrap().contains(&leader),
            "a running call's group is registered"
        );

        let outcome = call.join().unwrap();

        assert!(matches!(outcome, CommandOutcome::TimedOut), "{outcome:?}");
        assert!(
            !LIVE_CHILD_GROUPS.lock().unwrap().contains(&leader),
            "a call that returned leaves nothing to kill"
        );
    }

    /// The shutdown helper kills a registered group. Run over a registry of
    /// the test's own, so it cannot reach a call another test has running;
    /// nothing signals the leader by its pid, so its death is the group kill.
    #[test]
    fn the_shutdown_helper_kills_a_registered_group() {
        let mut leader = Command::new("sleep")
            .arg("30")
            .process_group(0)
            .spawn()
            .unwrap();
        let registry = Mutex::new(BTreeSet::from([leader.id().cast_signed()]));

        kill_groups_in(&registry);

        let status = leader.wait().unwrap();
        assert_eq!(
            std::os::unix::process::ExitStatusExt::signal(&status),
            Some(9)
        );
    }

    #[test]
    fn only_a_program_that_is_not_there_is_told_apart_as_not_found() {
        let missing = Command::new("/nonexistent/dux/no-such-program");
        assert!(matches!(
            run_command_with_timeout(
                missing,
                ChildGroup::New,
                Duration::from_secs(5),
                DEFAULT_READER_DRAIN,
                "t"
            ),
            CommandOutcome::NotFound(_)
        ));
        // A file that is there but cannot be executed is a failure, not absent.
        let dir = crate::test_scratch::ScratchDir::new();
        let plain = dir.path().join("not-executable");
        std::fs::write(&plain, "#!/bin/sh\n").unwrap();
        let outcome = run_command_with_timeout(
            Command::new(&plain),
            ChildGroup::New,
            Duration::from_secs(5),
            DEFAULT_READER_DRAIN,
            "t",
        );
        assert!(matches!(outcome, CommandOutcome::Failed(_)), "{outcome:?}");
    }

    #[test]
    fn a_completed_command_returns_its_output() {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf hello; printf oops >&2"]);
        match run_command_with_timeout(
            cmd,
            ChildGroup::New,
            Duration::from_secs(5),
            DEFAULT_READER_DRAIN,
            "sh",
        ) {
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
        let outcome = run_command_with_timeout(
            cmd,
            ChildGroup::New,
            Duration::from_millis(200),
            DEFAULT_READER_DRAIN,
            "sh",
        );
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
    fn a_command_in_a_session_of_its_own_takes_its_whole_group_down_on_the_cap() {
        use std::os::unix::process::CommandExt as _;
        let dir = crate::test_scratch::ScratchDir::new();
        let pid_file = dir.path().join("grandchild.pid");
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(format!(
            "sleep 60 & echo $! > '{}'; wait",
            pid_file.display()
        ));
        // SAFETY: `setsid` is async-signal-safe and touches no Rust state.
        unsafe {
            cmd.pre_exec(|| {
                rustix::process::setsid().map_err(std::io::Error::from)?;
                Ok(())
            });
        }
        let outcome = run_command_with_timeout(
            cmd,
            ChildGroup::SessionSetByCaller,
            Duration::from_millis(500),
            DEFAULT_READER_DRAIN,
            "sh",
        );
        assert!(matches!(outcome, CommandOutcome::TimedOut), "{outcome:?}");
        let pid: u32 = std::fs::read_to_string(&pid_file)
            .expect("the grandchild started")
            .trim()
            .parse()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while crate::file_drop::process_can_answer(pid) {
            assert!(
                Instant::now() < deadline,
                "the grandchild outlived the command's group"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_plain_command_takes_its_whole_group_down_on_the_cap() {
        // The grandchild keeps stderr open, so if it survived the kill the call
        // would also overrun its cap by the drain floor.
        let dir = crate::test_scratch::ScratchDir::new();
        let pid_file = dir.path().join("grandchild.pid");
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(format!(
            "sleep 60 & echo $! > '{}'; wait",
            pid_file.display()
        ));
        let start = Instant::now();
        let outcome = run_command_with_timeout(
            cmd,
            ChildGroup::New,
            Duration::from_millis(500),
            DEFAULT_READER_DRAIN,
            "sh",
        );
        let took = start.elapsed();
        assert!(matches!(outcome, CommandOutcome::TimedOut), "{outcome:?}");
        assert!(
            took < Duration::from_millis(500) + DEFAULT_READER_DRAIN,
            "nothing may hold the pipe past the cap, took {took:?}"
        );
        let pid: u32 = std::fs::read_to_string(&pid_file)
            .expect("the grandchild started")
            .trim()
            .parse()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(10);
        while crate::file_drop::process_can_answer(pid) {
            assert!(
                Instant::now() < deadline,
                "the grandchild outlived the command's group"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_grandchild_holding_the_pipe_open_cannot_outlast_the_cap_plus_the_floor() {
        // The child exits at once, so the wait loop is done immediately, but a
        // background grandchild keeps stdout open for far longer than either
        // bound. Spending the cap's leftover on the readers must not turn into
        // waiting for the pipe: the call returns with what it has.
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "printf hello; sleep 30 &"]);
        let start = Instant::now();
        let outcome = run_command_with_timeout(
            cmd,
            ChildGroup::New,
            Duration::from_millis(300),
            Duration::from_millis(100),
            "sh",
        );
        assert!(
            matches!(outcome, CommandOutcome::Completed(_)),
            "the child exited on its own: {outcome:?}"
        );
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "a held-open pipe must not outlast the cap plus the floor, took {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn a_missing_program_fails_rather_than_timing_out() {
        let cmd = Command::new("dux-no-such-program-9f3a");
        let outcome = run_command_with_timeout(
            cmd,
            ChildGroup::New,
            Duration::from_secs(5),
            DEFAULT_READER_DRAIN,
            "nope",
        );
        assert!(
            matches!(outcome, CommandOutcome::NotFound(_)),
            "a missing program is NotFound, never TimedOut: {outcome:?}"
        );
    }
}
