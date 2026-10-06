//! `git clone`, run so that it can never ask for anything and never hangs
//! unnoticed.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Why a clone did not finish.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloneError {
    /// git ran and failed, or could not start; what it said, with any user and
    /// token in an address removed.
    Failed(String),
    /// git printed nothing for this long, so dux stopped it.
    Stalled(Duration),
    /// dux is quitting, and stopped it.
    Stopped,
}

/// The clones running right now, so quitting dux can stop them: each one runs
/// in a session of its own, which a quit would otherwise leave running.
#[derive(Clone, Debug, Default)]
pub struct CloneProcesses(Arc<Mutex<CloneProcessState>>);

#[derive(Debug, Default)]
struct CloneProcessState {
    next: u64,
    /// Each running clone's process group, by the token its runner holds.
    running: HashMap<u64, i32>,
    /// Set once by [`CloneProcesses::stop_all`]: dux is quitting, and a clone
    /// starting after it is stopped as it starts.
    stopping: bool,
}

impl CloneProcesses {
    fn lock(&self) -> std::sync::MutexGuard<'_, CloneProcessState> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Start a clone with `start` and record its process group, under the
    /// lock [`Self::stop_all`] takes: a quit that begins while a clone starts
    /// waits for it to be recorded (as long as starting a process takes) and
    /// kills it, and once a quit has begun nothing starts.
    pub(crate) fn spawn(
        &self,
        start: impl FnOnce() -> std::io::Result<std::process::Child>,
    ) -> Result<(std::process::Child, u64), SpawnRefused> {
        let mut state = self.lock();
        if state.stopping {
            return Err(SpawnRefused::Stopping);
        }
        let child = start().map_err(SpawnRefused::Failed)?;
        state.next += 1;
        let token = state.next;
        state.running.insert(token, child.id() as i32);
        Ok((child, token))
    }

    /// Forget the clone behind `token`; whether dux stopped it.
    fn finish(&self, token: u64) -> bool {
        let mut state = self.lock();
        state.running.remove(&token);
        state.stopping
    }

    /// Stop every running clone, and every clone that starts from now on.
    pub fn stop_all(&self) {
        let mut state = self.lock();
        state.stopping = true;
        for group in state.running.values() {
            kill_group(*group);
        }
    }
}

/// Why [`CloneProcesses::spawn`] started nothing.
#[derive(Debug)]
pub(crate) enum SpawnRefused {
    /// dux is quitting.
    Stopping,
    /// The process could not start.
    Failed(std::io::Error),
}

fn kill_group(group: i32) {
    if let Some(group) = rustix::process::Pid::from_raw(group) {
        let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
    }
}

/// Clone `url` into `dest` with the user's own git configuration and
/// credentials, never asking for anything: git runs in a session of its own
/// with no terminal, so neither git nor ssh has anywhere to prompt, and one
/// that would have to prompt fails at once with its own message. There is no
/// time limit, because a big repository is slow rather than stuck; a clone
/// that prints nothing for `stall` (its progress keeps stderr moving) is
/// stopped. `processes` records it, so quitting dux stops it too.
pub fn clone_repository(
    url: &str,
    dest: &Path,
    stall: Duration,
    processes: &CloneProcesses,
) -> Result<(), CloneError> {
    clone_repository_with_env(url, dest, stall, processes, &[])
}

/// [`clone_repository`] with extra variables in git's environment, so a test
/// can hand git a stand-in transport.
pub(crate) fn clone_repository_with_env(
    url: &str,
    dest: &Path,
    stall: Duration,
    processes: &CloneProcesses,
    env: &[(&str, &str)],
) -> Result<(), CloneError> {
    use std::io::Read as _;
    use std::os::unix::process::CommandExt as _;
    use std::sync::mpsc::RecvTimeoutError;

    let mut command = std::process::Command::new("git");
    // `--origin origin` pins the remote's name against a user's
    // `clone.defaultRemoteName`, and `--progress` keeps stderr moving while
    // the clone works, which is what the stall clock watches.
    for (key, value) in env {
        command.env(key, value);
    }
    // Nothing may ask for an answer, whatever the inherited environment says:
    // no terminal prompt, an askpass for git and for ssh that fails at once,
    // and no interactive credential helper. A helper that answers without
    // asking (a stored token, a keychain) still works.
    command
        .args(["-c", "credential.interactive=false"])
        .args(["clone", "--progress", "--origin", "origin", "--"])
        .arg(url)
        .arg(dest)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "false")
        .env("SSH_ASKPASS", "false")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env("GCM_INTERACTIVE", "never")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    // SAFETY: `setsid` is async-signal-safe and touches no Rust state. A new
    // session has no controlling terminal, so ssh cannot open `/dev/tty` to
    // ask for a passphrase or a host key, and it is a process group of its
    // own, so the whole clone can be stopped at once.
    unsafe {
        command.pre_exec(|| {
            rustix::process::setsid().map_err(std::io::Error::from)?;
            Ok(())
        });
    }
    let (mut child, token) =
        processes
            .spawn(|| command.spawn())
            .map_err(|refused| match refused {
                SpawnRefused::Stopping => CloneError::Stopped,
                SpawnRefused::Failed(error) => {
                    CloneError::Failed(format!("couldn't run git: {error}"))
                }
            })?;
    let group = child.id() as i32;

    // Every chunk git writes is progress. Read on a thread of its own and handed
    // over a channel, so the stall clock can wait on it with a bound.
    let mut pipe = child.stderr.take();
    let (chunks_tx, chunks) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let Some(pipe) = pipe.as_mut() else { return };
        let mut buffer = [0u8; 4096];
        while let Ok(read) = pipe.read(&mut buffer) {
            if read == 0 || chunks_tx.send(buffer[..read].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut said = StderrTail::default();
    let mut last_progress = std::time::Instant::now();
    let status = loop {
        match chunks.recv_timeout(Duration::from_millis(50)) {
            Ok(chunk) => {
                said.push(&chunk);
                last_progress = std::time::Instant::now();
            }
            Err(RecvTimeoutError::Timeout) => {}
            // git closed stderr; wait on the process without spinning.
            Err(RecvTimeoutError::Disconnected) => {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(error) => break Err(error),
        }
        if last_progress.elapsed() >= stall {
            kill_group(group);
            let _ = child.kill();
            let _ = child.wait();
            let stopped = processes.finish(token);
            return Err(if stopped {
                CloneError::Stopped
            } else {
                CloneError::Stalled(stall)
            });
        }
    };
    // What git said last, without waiting on a helper that escaped the group
    // and still holds stderr open.
    let deadline = std::time::Instant::now() + crate::bounded_command::DEFAULT_READER_DRAIN;
    while let Ok(chunk) =
        chunks.recv_timeout(deadline.saturating_duration_since(std::time::Instant::now()))
    {
        said.push(&chunk);
    }
    let stopped = processes.finish(token);
    if stopped {
        return Err(CloneError::Stopped);
    }
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(CloneError::Failed(said.failure(status))),
        Err(error) => Err(CloneError::Failed(format!(
            "couldn't wait for git: {error}"
        ))),
    }
}

/// The end of what git printed, bounded so a long clone's progress cannot grow
/// it without limit.
#[derive(Default)]
struct StderrTail(Vec<u8>);

impl StderrTail {
    const KEEP: usize = 64 * 1024;

    fn push(&mut self, chunk: &[u8]) {
        self.0.extend_from_slice(chunk);
        if self.0.len() > Self::KEEP {
            let excess = self.0.len() - Self::KEEP;
            self.0.drain(..excess);
        }
    }

    /// What git said about why it failed: its lines without the progress
    /// ones, any user and token in an address removed.
    fn failure(&self, status: std::process::ExitStatus) -> String {
        let text = String::from_utf8_lossy(&self.0);
        let lines: Vec<&str> = text
            .split(['\r', '\n'])
            .map(str::trim)
            .filter(|line| {
                !line.is_empty()
                    && !line.starts_with("Cloning into")
                    && !line.contains("% (")
                    && !line.ends_with(", done.")
            })
            .collect();
        if lines.is_empty() {
            return format!("git clone exited with {status}");
        }
        crate::clone_project::redact_remote_userinfo(&lines.join(" "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::test_support::{bare_remote_with_commit, git_command};
    use std::os::unix::fs::PermissionsExt;
    use std::time::Instant;

    /// A stand-in for `ssh` at `dir/ssh`, running `body`.
    fn fake_ssh(dir: &Path, body: &str) -> String {
        let path = dir.join("ssh");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path.to_string_lossy().into_owned()
    }

    fn read_git(dir: &Path, args: &[&str]) -> String {
        let out = git_command()
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    /// The pid a stand-in wrote to `file`, once it has.
    fn wait_for_pid(file: &Path) -> u32 {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(text) = std::fs::read_to_string(file)
                && let Ok(pid) = text.trim().parse()
            {
                return pid;
            }
            assert!(Instant::now() < deadline, "the stand-in never started");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_gone(pid: u32) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while crate::file_drop::process_can_answer(pid) {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    }

    #[test]
    fn a_clone_checks_out_the_remotes_default_branch_with_the_remote_named_origin() {
        let tmp = tempfile::tempdir().unwrap();
        let remote = bare_remote_with_commit(tmp.path());
        let dest = tmp.path().join("clone");

        let result = clone_repository(
            remote.to_str().unwrap(),
            &dest,
            Duration::from_secs(60),
            &CloneProcesses::default(),
        );

        assert_eq!(result, Ok(()));
        assert_eq!(
            read_git(&dest, &["symbolic-ref", "HEAD"]),
            "refs/heads/main"
        );
        assert_eq!(
            read_git(&dest, &["symbolic-ref", "refs/remotes/origin/HEAD"]),
            "refs/remotes/origin/main"
        );
        assert_eq!(
            read_git(
                &dest,
                &["rev-parse", "--verify", "refs/remotes/origin/feature"]
            )
            .len(),
            40,
            "a full clone brings the other branches too"
        );
    }

    #[test]
    /// Read as an option, the address would make git clone FROM the
    /// destination through the command it names, which is how a repository
    /// there turns it into a command run (measured on git 2.53).
    fn clone_repository_reads_an_option_looking_address_as_an_address() {
        let tmp = tempfile::tempdir().unwrap();
        let marker = tmp.path().join("obeyed");
        let dest = crate::git::test_support::empty_bare_remote(tmp.path());
        let address = format!("--upload-pack=touch {}", marker.display());

        let result = clone_repository(
            &address,
            &dest,
            Duration::from_secs(60),
            &CloneProcesses::default(),
        );

        assert!(matches!(result, Err(CloneError::Failed(_))), "{result:?}");
        assert!(!marker.exists(), "the address was obeyed as an option");
    }

    /// git and ssh have no terminal to ask on, so a transport that wants one
    /// fails at once with its own words, told by the environment never to ask.
    #[test]
    fn a_transport_that_reads_the_terminal_fails_at_once_and_nothing_asks() {
        let tmp = tempfile::tempdir().unwrap();
        let seen = tmp.path().join("seen");
        let ssh = fake_ssh(
            tmp.path(),
            &format!(
                "{{ echo \"prompt=$GIT_TERMINAL_PROMPT askpass=$SSH_ASKPASS_REQUIRE\"; \
                 ps -o pgid= -p $$; }} > '{}'\n\
                 if read answer < /dev/tty; then echo 'read the terminal' >&2; fi\n\
                 echo 'ssh: no terminal to ask on for https://me:ghp_secret@example.invalid/x' >&2\n\
                 exit 255",
                seen.display()
            ),
        );
        let started = Instant::now();

        let result = clone_repository_with_env(
            "ssh://example.invalid/repo.git",
            &tmp.path().join("clone"),
            Duration::from_secs(60),
            &CloneProcesses::default(),
            &[("GIT_SSH_COMMAND", &ssh)],
        );

        assert!(
            started.elapsed() < Duration::from_secs(20),
            "it waited on a terminal"
        );
        let Err(CloneError::Failed(message)) = result else {
            panic!("expected a failure, got {result:?}");
        };
        assert!(message.contains("no terminal to ask on"), "{message}");
        assert!(
            !message.contains("ghp_secret"),
            "a token was relayed: {message}"
        );
        assert!(message.contains("https://example.invalid/x"), "{message}");
        let seen = std::fs::read_to_string(&seen).unwrap();
        let mut lines = seen.lines();
        assert_eq!(lines.next(), Some("prompt=0 askpass=never"));
        let group: i32 = lines.next().unwrap().trim().parse().unwrap();
        assert_ne!(
            group,
            rustix::process::getpgrp().as_raw_nonzero().get(),
            "the clone ran in dux's own process group"
        );
    }

    /// A credential prompt is never launched: git's own and ssh's askpass fail
    /// at once, and git does not ask interactively, even when the environment
    /// dux inherited names askpass programs that would wait for an answer.
    #[test]
    fn no_askpass_program_is_ever_waited_on() {
        let tmp = tempfile::tempdir().unwrap();
        let hang = tmp.path().join("hang");
        std::fs::write(&hang, "#!/bin/sh\nexec sleep 60\n").unwrap();
        std::fs::set_permissions(&hang, std::fs::Permissions::from_mode(0o755)).unwrap();
        let hang = hang.to_string_lossy().into_owned();
        let ssh = fake_ssh(
            tmp.path(),
            "printf 'protocol=https\\nhost=example.invalid\\n\\n' \
             | git -c credential.helper= credential fill >/dev/null 2>&1\n\
             \"$SSH_ASKPASS\" 'Password:' </dev/null >/dev/null 2>&1\n\
             echo 'ssh: nothing to ask with' >&2\n\
             exit 255",
        );
        let started = Instant::now();

        let result = clone_repository_with_env(
            "ssh://example.invalid/repo.git",
            &tmp.path().join("clone"),
            Duration::from_secs(30),
            &CloneProcesses::default(),
            &[
                ("GIT_SSH_COMMAND", &ssh),
                ("GIT_ASKPASS", &hang),
                ("SSH_ASKPASS", &hang),
            ],
        );

        assert!(
            started.elapsed() < Duration::from_secs(15),
            "an askpass was waited on: {result:?}"
        );
        let Err(CloneError::Failed(message)) = result else {
            panic!("expected a failure, got {result:?}");
        };
        assert!(message.contains("nothing to ask with"), "{message}");
    }

    #[test]
    fn a_clone_that_keeps_printing_progress_is_never_stalled() {
        let tmp = tempfile::tempdir().unwrap();
        let ssh = fake_ssh(
            tmp.path(),
            "for i in 1 2 3 4 5 6 7 8; do echo \"remote: still working $i\" >&2; sleep 0.3; done\n\
             echo 'ssh: done talking' >&2\n\
             exit 255",
        );

        let result = clone_repository_with_env(
            "ssh://example.invalid/repo.git",
            &tmp.path().join("clone"),
            Duration::from_secs(1),
            &CloneProcesses::default(),
            &[("GIT_SSH_COMMAND", &ssh)],
        );

        let Err(CloneError::Failed(message)) = result else {
            panic!("a clone printing progress was stopped: {result:?}");
        };
        assert!(message.contains("done talking"), "{message}");
    }

    /// A clone starting while dux begins to quit is killed before the quit
    /// moves on, and once it has begun none starts at all.
    #[test]
    fn quitting_waits_for_a_starting_clone_and_refuses_later_ones() {
        use std::os::unix::process::CommandExt as _;
        use std::sync::atomic::{AtomicU32, Ordering};
        let processes = CloneProcesses::default();
        let pid = Arc::new(AtomicU32::new(0));
        let starting = {
            let processes = processes.clone();
            let pid = Arc::clone(&pid);
            std::thread::spawn(move || {
                processes.spawn(|| {
                    let child = std::process::Command::new("sleep")
                        .arg("30")
                        .process_group(0)
                        .spawn()?;
                    pid.store(child.id(), Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(600));
                    Ok(child)
                })
            })
        };
        while pid.load(Ordering::SeqCst) == 0 {
            std::thread::sleep(Duration::from_millis(10));
        }

        processes.stop_all();

        let deadline = Instant::now() + Duration::from_millis(150);
        let mut alive = true;
        while alive && Instant::now() < deadline {
            alive = crate::file_drop::process_can_answer(pid.load(Ordering::SeqCst));
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !alive,
            "the quit moved on before the starting clone was killed"
        );
        if let Ok((mut child, _)) = starting.join().unwrap() {
            let _ = child.wait();
        }
        let mut started = false;
        let late = processes.spawn(|| {
            started = true;
            std::process::Command::new("true").spawn()
        });
        assert!(matches!(late, Err(SpawnRefused::Stopping)), "{late:?}");
        assert!(!started, "a clone started after the quit began");
    }

    #[test]
    fn a_clone_that_prints_nothing_for_the_stall_time_is_stopped() {
        let tmp = tempfile::tempdir().unwrap();
        let pid_file = tmp.path().join("pid");
        let ssh = fake_ssh(
            tmp.path(),
            &format!("echo $$ > '{}'\nexec sleep 60", pid_file.display()),
        );
        let started = Instant::now();

        let result = clone_repository_with_env(
            "ssh://example.invalid/repo.git",
            &tmp.path().join("clone"),
            Duration::from_secs(1),
            &CloneProcesses::default(),
            &[("GIT_SSH_COMMAND", &ssh)],
        );

        assert_eq!(result, Err(CloneError::Stalled(Duration::from_secs(1))));
        assert!(started.elapsed() < Duration::from_secs(20));
        assert!(
            wait_gone(wait_for_pid(&pid_file)),
            "the transport outlived the stopped clone"
        );
    }

    #[test]
    fn quitting_dux_stops_a_running_clone_and_any_that_starts() {
        let tmp = tempfile::tempdir().unwrap();
        let pid_file = tmp.path().join("pid");
        let ssh = fake_ssh(
            tmp.path(),
            &format!("echo $$ > '{}'\nexec sleep 60", pid_file.display()),
        );
        let (mut engine, _engine_dir) = crate::engine::test_support::test_engine();
        let running = engine.clones.processes.clone();
        let dest = tmp.path().join("clone");
        let clone = {
            let ssh = ssh.clone();
            std::thread::spawn(move || {
                clone_repository_with_env(
                    "ssh://example.invalid/repo.git",
                    &dest,
                    Duration::from_secs(120),
                    &running,
                    &[("GIT_SSH_COMMAND", &ssh)],
                )
            })
        };
        let pid = wait_for_pid(&pid_file);

        engine.shutdown_ptys(Duration::ZERO);

        assert_eq!(clone.join().unwrap(), Err(CloneError::Stopped));
        assert!(wait_gone(pid), "the transport outlived the quit");
        let late = clone_repository_with_env(
            "ssh://example.invalid/repo.git",
            &tmp.path().join("late"),
            Duration::from_secs(120),
            &engine.clones.processes,
            &[("GIT_SSH_COMMAND", &ssh)],
        );
        assert_eq!(late, Err(CloneError::Stopped));
    }
}
