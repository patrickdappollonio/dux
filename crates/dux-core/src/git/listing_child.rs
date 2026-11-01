//! The git children a changed-files listing runs, each held to a deadline.
//!
//! A listing runs in its worktree's lane (see [`crate::changes_lane`]), so a
//! `git status` that never finishes (a wedged fsmonitor hook, a stalled
//! network mount) would keep every change behind it waiting. Each child gets
//! [`DEADLINE`]; past it the child's process group is sent `SIGTERM`, which
//! git answers by removing its lock files, then `SIGKILL` after [`GRACE`] if it
//! is still there, and the call returns only once the child has exited, so the
//! turn the caller holds is never given back while git still runs.
//!
//! A stop is remembered for the listing that ran it ([`stopped_within`]), so a
//! child whose failure the listing would otherwise absorb (a numstat costs the
//! rows their line counts and nothing else) still fails the whole read.

use std::cell::RefCell;
use std::io::{self, Read};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How long one git child of a listing may run.
pub(crate) const DEADLINE: Duration = Duration::from_secs(30);

/// How long a child sent `SIGTERM` has to exit before it is sent `SIGKILL`.
pub(crate) const GRACE: Duration = Duration::from_secs(2);

/// How long the output readers are waited for once the child has exited: a
/// grandchild the child left behind may hold a pipe open.
const READER_DRAIN: Duration = Duration::from_secs(2);

/// What a stopped read meant for the person waiting on it, which is what
/// its sentence tells them beyond that it was stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Purpose {
    /// Nothing more can be said truthfully: a folder's contents, say, which
    /// the next poll does not fetch again and no change was waiting on.
    Neutral,
    /// The changes list itself: it stays as it was, and the next poll or
    /// refresh lists again.
    Display,
    /// A read a change runs before it acts: the change was not made.
    Change,
}

thread_local! {
    /// What this thread's reads are for, as the outermost read that said so
    /// decided; `None` reads as [`Purpose::Neutral`].
    static PURPOSE: std::cell::Cell<Option<Purpose>> = const { std::cell::Cell::new(None) };
    /// The stop the current listing's children met, if any.
    static STOPPED: RefCell<Option<String>> = const { RefCell::new(None) };
    /// The deadline and grace this thread's children are held to.
    static LIMITS: RefCell<(Duration, Duration)> = const { RefCell::new((DEADLINE, GRACE)) };
}

/// Run `command`, its output captured, held to the deadline. `label` names it
/// in the sentence a stop produces.
pub(crate) fn output(command: &mut Command, label: &str) -> io::Result<Output> {
    run(command, None, None, label)
}

/// [`output`], reading at most `limit` bytes of standard output and closing
/// the pipe after them.
pub(crate) fn output_prefix(command: &mut Command, limit: u64, label: &str) -> io::Result<Output> {
    run(command, None, Some(limit), label)
}

/// [`output`], writing `input` to the child's standard input.
pub(crate) fn output_with_input(
    command: &mut Command,
    input: Vec<u8>,
    label: &str,
) -> io::Result<Output> {
    run(command, Some(input), None, label)
}

/// Runs its closure when dropped, a panic's unwinding included, which is
/// how every thread-local setting below is put back as it was.
struct Restore<F: FnOnce()>(Option<F>);

impl<F: FnOnce()> Drop for Restore<F> {
    fn drop(&mut self) {
        if let Some(restore) = self.0.take() {
            restore();
        }
    }
}

/// Run `read`, and answer the sentence for any child of it that was stopped
/// at its deadline, whatever `read` itself made of that child's failure.
pub(crate) fn stopped_within<T>(read: impl FnOnce() -> T) -> (T, Option<String>) {
    let outer = STOPPED.with(|stopped| stopped.borrow_mut().take());
    let restore = Restore(Some(move || {
        STOPPED.with(|stopped| *stopped.borrow_mut() = outer);
    }));
    let value = read();
    let stopped = STOPPED.with(|stopped| stopped.borrow_mut().take());
    drop(restore);
    (value, stopped)
}

/// Run `read` for `purpose`, so a child stopped inside it says what that
/// stop meant. The outermost read decides: a folder expansion that lists the
/// whole worktree on a cold cache is still a folder expansion.
pub(crate) fn with_purpose<T>(purpose: Purpose, read: impl FnOnce() -> T) -> T {
    let outer = PURPOSE.with(|current| current.replace(current.get().or(Some(purpose))));
    let _restore = Restore(Some(move || PURPOSE.with(|current| current.set(outer))));
    read()
}

/// Hold this thread's listing children to `deadline` and `grace` while `run`
/// runs.
#[cfg(test)]
pub(crate) fn with_limits<T>(deadline: Duration, grace: Duration, run: impl FnOnce() -> T) -> T {
    let outer =
        LIMITS.with(|limits| std::mem::replace(&mut *limits.borrow_mut(), (deadline, grace)));
    let _restore = Restore(Some(move || {
        LIMITS.with(|limits| *limits.borrow_mut() = outer);
    }));
    run()
}

fn run(
    command: &mut Command,
    input: Option<Vec<u8>>,
    stdout_limit: Option<u64>,
    label: &str,
) -> io::Result<Output> {
    let (deadline, grace) = LIMITS.with(|limits| *limits.borrow());
    // A group of its own, so a stop also reaches the hooks git started.
    command
        .process_group(0)
        .stdin(if input.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn()?;
    // Fed from a thread of its own: a child can fill its output pipe before
    // it has read all of its input.
    let writer = match (input, child.stdin.take()) {
        (Some(payload), Some(mut stdin)) => Some(std::thread::spawn(move || {
            let _ = io::Write::write_all(&mut stdin, &payload);
        })),
        _ => None,
    };
    let stdout = drain(child.stdout.take(), stdout_limit);
    let stderr = drain(child.stderr.take(), None);

    let stopped_sentence = || {
        let seconds = crate::duration_text::whole_seconds(deadline);
        let sentence = match PURPOSE
            .with(std::cell::Cell::get)
            .unwrap_or(Purpose::Neutral)
        {
            Purpose::Neutral => {
                format!("{label} ran for {seconds} seconds without finishing, so dux stopped it.")
            }
            Purpose::Display => format!(
                "{label} ran for {seconds} seconds without finishing, so dux stopped it and \
                 the list of changed files wasn't updated. dux tries again on its next check."
            ),
            Purpose::Change => format!(
                "{label} ran for {seconds} seconds without finishing, so dux stopped it before \
                 making the change, and nothing was changed. Try again."
            ),
        };
        STOPPED.with(|stopped| {
            stopped.borrow_mut().get_or_insert_with(|| sentence.clone());
        });
        io::Error::new(io::ErrorKind::TimedOut, sentence)
    };
    let status = match exited_within(&child, deadline) {
        Exit::Exited => child.wait()?,
        Exit::Running => {
            stop(&mut child, grace);
            return Err(stopped_sentence());
        }
        Exit::Unknown => {
            // The exit could not be waited for directly, so it is polled.
            let started = Instant::now();
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => break status,
                    Ok(None) if started.elapsed() >= deadline => {
                        stop(&mut child, grace);
                        return Err(stopped_sentence());
                    }
                    Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                    Err(err) => {
                        stop(&mut child, Duration::ZERO);
                        return Err(err);
                    }
                }
            }
        }
    };
    if let Some(writer) = writer {
        let _ = writer.join();
    }
    Ok(Output {
        status,
        stdout: stdout.recv_timeout(READER_DRAIN).unwrap_or_default(),
        stderr: stderr.recv_timeout(READER_DRAIN).unwrap_or_default(),
    })
}

/// What [`exited_within`] saw.
enum Exit {
    /// The child exited, and is left for the caller to reap.
    Exited,
    /// It was still running at the deadline.
    Running,
    /// Waiting for it failed.
    Unknown,
}

/// Wait up to `deadline` for `child` to exit, leaving it unreaped. The wait
/// runs on a thread of its own so the answer comes the moment the child exits.
fn exited_within(child: &Child, deadline: Duration) -> Exit {
    use rustix::process::{WaitId, WaitIdOptions, waitid};
    let Some(pid) = rustix::process::Pid::from_raw(child.id().cast_signed()) else {
        return Exit::Unknown;
    };
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let waited = loop {
            match waitid(
                WaitId::Pid(pid),
                WaitIdOptions::EXITED | WaitIdOptions::NOWAIT,
            ) {
                Err(rustix::io::Errno::INTR) => {}
                other => break other.is_ok(),
            }
        };
        let _ = tx.send(waited);
    });
    match rx.recv_timeout(deadline) {
        Ok(true) => Exit::Exited,
        Ok(false) | Err(mpsc::RecvTimeoutError::Disconnected) => Exit::Unknown,
        Err(mpsc::RecvTimeoutError::Timeout) => Exit::Running,
    }
}

/// Read a pipe to its end, or to `limit` bytes, on a thread of its own; the
/// pipe is closed once read.
fn drain<R: Read + Send + 'static>(pipe: Option<R>, limit: Option<u64>) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    if let Some(pipe) = pipe {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            let _ = pipe.take(limit.unwrap_or(u64::MAX)).read_to_end(&mut buf);
            let _ = tx.send(buf);
        });
    }
    rx
}

/// `SIGTERM` to the child's group, then, once the child has exited or after
/// `grace`, `SIGKILL` to whatever is left of the group (git if it still runs, a
/// hook that outlived it), and reap the child. The child is reaped last, so its
/// id, which is the group's, cannot have been handed to another process while
/// the group is signalled.
fn stop(child: &mut Child, grace: Duration) {
    let Some(group) = rustix::process::Pid::from_raw(child.id().cast_signed()) else {
        let _ = child.kill();
        let _ = child.wait();
        return;
    };
    let _ = rustix::process::kill_process_group(group, rustix::process::Signal::TERM);
    let asked = Instant::now();
    while !has_exited(group) && asked.elapsed() < grace {
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
    let _ = child.wait();
}

/// Whether the child `pid` has exited, leaving it waitable.
fn has_exited(pid: rustix::process::Pid) -> bool {
    use rustix::process::{WaitId, WaitIdOptions, waitid};
    matches!(
        waitid(
            WaitId::Pid(pid),
            WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT
        ),
        Ok(Some(_))
    )
}
