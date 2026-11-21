//! `git clone`, and the command every network git call starts from: neither
//! ever asks on dux's terminal or hangs unnoticed.

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
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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
        state.running.insert(token, child.id().cast_signed());
        Ok((child, token))
    }

    /// Forget the clone behind `token`; whether dux stopped it.
    fn finish(&self, token: u64) -> bool {
        let mut state = self.lock();
        state.running.remove(&token);
        state.stopping
    }

    /// Stop every running clone, and every clone that starts from now on.
    pub(crate) fn stop_all(&self) {
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

/// Who started a network git call, which decides whether anything may ask the
/// user for a credential while it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StartedBy {
    /// The user asked for it and is waiting on it: an askpass they configured
    /// may answer, or ask them.
    User,
    /// dux runs it on its own: no askpass of any kind runs, so only a
    /// credential helper that answers without asking can sign it in.
    Background,
}

/// A `git` command that can never ask for an answer on dux's terminal: no
/// terminal prompt, no interactive Git Credential Manager, and a session of its
/// own, so with no controlling terminal ssh cannot open `/dev/tty` to ask for a
/// passphrase or a host key. It is a process group of its own too, so a clone
/// can be stopped at once. Null stdin; the caller sets the rest.
///
/// For [`StartedBy::User`] an askpass configured in `GIT_ASKPASS`,
/// `SSH_ASKPASS` or `core.askPass` still runs; where none is, ssh gets one
/// that fails at once. For [`StartedBy::Background`] none runs: an empty
/// `GIT_ASKPASS` outranks `core.askPass` and `SSH_ASKPASS` for git, and
/// `SSH_ASKPASS_REQUIRE=never` stops ssh's.
///
/// `env` is extra variables for git's environment, set first (a test hands git
/// a stand-in transport or askpass this way).
pub(crate) fn network_git_command(
    started_by: StartedBy,
    env: &[(&str, &str)],
) -> std::process::Command {
    use std::os::unix::process::CommandExt as _;

    let mut command = std::process::Command::new("git");
    for (key, value) in env {
        command.env(key, value);
    }
    command
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(std::process::Stdio::null());
    let configured = |key: &str| match env.iter().find(|(name, _)| *name == key) {
        Some((_, value)) => !value.is_empty(),
        None => std::env::var_os(key).is_some_and(|value| !value.is_empty()),
    };
    match started_by {
        StartedBy::User => {
            if !configured("SSH_ASKPASS") {
                command
                    .env("SSH_ASKPASS", "false")
                    .env("SSH_ASKPASS_REQUIRE", "never");
            }
        }
        StartedBy::Background => {
            command
                .env("GIT_ASKPASS", "")
                .env("SSH_ASKPASS_REQUIRE", "never");
        }
    }
    // SAFETY: `setsid` is async-signal-safe and touches no Rust state.
    unsafe {
        command.pre_exec(|| {
            rustix::process::setsid().map_err(std::io::Error::from)?;
            Ok(())
        });
    }
    command
}

/// Whether a failed network git call's stderr says the remote wanted something
/// only a prompt could give: git's credential prompt (refused with "terminal
/// prompts disabled"), or ssh's host confirmation or key passphrase, which ssh
/// reports only as the refusal that followed.
pub(crate) fn wanted_a_prompt(stderr: &str) -> bool {
    stderr.contains("terminal prompts disabled")
        || stderr.contains("Host key verification failed")
        || stderr.contains("Permission denied (")
}

/// The branch the remote `origin` of the repository at `path` calls its
/// default, asked of the remote itself, for a folder that does not record it.
/// `None` when the remote cannot say: unreachable, slow past the bound, or an
/// answer that names no branch. `env` is extra variables for git's
/// environment, so a test can hand git a stand-in transport.
pub(crate) fn remote_default_branch_from_remote(
    path: &Path,
    started_by: StartedBy,
    env: &[(&str, &str)],
) -> Option<String> {
    let mut command = network_git_command(started_by, env);
    command
        .arg("-C")
        .arg(path)
        .args(["ls-remote", "--symref", "origin", "HEAD"]);
    let crate::bounded_command::CommandOutcome::Completed(output) =
        crate::bounded_command::run_command_with_timeout(
            command,
            crate::bounded_command::ChildGroup::SessionSetByCaller,
            crate::gh::GH_CALL_TIMEOUT,
            crate::bounded_command::DEFAULT_READER_DRAIN,
            "git ls-remote",
        )
    else {
        return None;
    };
    if !output.status.success() {
        return None;
    }
    // The first line is `ref: refs/heads/<branch>`, a tab, then `HEAD`.
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()?
        .strip_prefix("ref: refs/heads/")?
        .split('\t')
        .next()
        .filter(|branch| !branch.is_empty())
        .map(str::to_string)
}

/// Clone `url` into `dest` with the user's own git configuration and
/// credentials, never asking on dux's terminal: git runs in a session of its
/// own with no terminal, and only an askpass the user configured can ask for
/// a credential ([`StartedBy::User`]). There is no
/// time limit, because a big repository is slow rather than stuck; a clone
/// that prints nothing for `stall` (its progress keeps stderr moving) is
/// stopped. `processes` records it, so quitting dux stops it too.
///
/// # Errors
///
/// Fails when git cannot be started, the clone fails or would have to prompt, or it stalls for
/// `stall`.
pub fn clone_repository(
    url: &str,
    dest: &Path,
    stall: Duration,
    processes: &CloneProcesses,
    started_by: StartedBy,
) -> Result<(), CloneError> {
    clone_repository_with_env(url, dest, stall, processes, started_by, &[])
}

/// [`clone_repository`] with extra variables in git's environment, so a test
/// can hand git a stand-in transport.
pub(crate) fn clone_repository_with_env(
    url: &str,
    dest: &Path,
    stall: Duration,
    processes: &CloneProcesses,
    started_by: StartedBy,
    env: &[(&str, &str)],
) -> Result<(), CloneError> {
    use std::io::Read as _;
    use std::sync::mpsc::RecvTimeoutError;

    let mut command = network_git_command(started_by, env);
    // `--origin origin` pins the remote's name against a user's
    // `clone.defaultRemoteName`, and `--progress` keeps stderr moving while
    // the clone works, which is what the stall clock watches.
    command
        .args(["clone", "--progress", "--origin", "origin", "--"])
        .arg(url)
        .arg(dest)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    let (mut child, token) =
        processes
            .spawn(|| command.spawn())
            .map_err(|refused| match refused {
                SpawnRefused::Stopping => CloneError::Stopped,
                SpawnRefused::Failed(error) => {
                    CloneError::Failed(format!("couldn't run git: {error}"))
                }
            })?;
    let group = child.id().cast_signed();

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

    const PROMPTLESS: &str = "DUX_TEST_PROMPTLESS_PROCESS";

    /// Whether the calling test's body runs in this process. The first call
    /// runs the test `name` again in a test process of its own whose
    /// environment names no askpass, display or git configuration of the
    /// developer's, asserts it passed there, and returns false; in that process
    /// it returns true.
    ///
    /// The production git a clone test runs inherits this process's
    /// environment, and a test can add variables to it but not take any away,
    /// so a developer's `SSH_ASKPASS` or `GIT_ASKPASS` would otherwise be what
    /// git and the stand-in ssh ask, in a window on their desktop.
    fn runs_here_without_the_developers_prompts(name: &str) -> bool {
        if std::env::var_os(PROMPTLESS).is_some() {
            return true;
        }
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([name, "--exact", "--test-threads=1"])
            .env(PROMPTLESS, "1")
            .env_remove("SSH_ASKPASS")
            .env_remove("SSH_ASKPASS_REQUIRE")
            .env_remove("GIT_ASKPASS")
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("GIT_CONFIG_COUNT")
            .env_remove("GIT_CONFIG_PARAMETERS")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        let said = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success() && said.contains("test result: ok. 1 passed"),
            "{name} did not pass on its own:\n{said}"
        );
        false
    }

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

    /// The clone tests, run again where the developer's environment names an
    /// askpass and a display and their git configuration names an askpass and
    /// a credential helper, each a stand-in that records being run. None may
    /// be run: on a desktop the real ones open a password window and wait.
    #[test]
    fn no_clone_test_runs_a_prompt_the_developer_configured() {
        let tmp = tempfile::tempdir().unwrap();
        let ran = tmp.path().join("ran");
        let stand_in = tmp.path().join("prompt");
        std::fs::write(
            &stand_in,
            format!("#!/bin/sh\necho \"$0 $*\" >> '{}'\nexit 1\n", ran.display()),
        )
        .unwrap();
        std::fs::set_permissions(&stand_in, std::fs::Permissions::from_mode(0o755)).unwrap();
        let global = tmp.path().join("gitconfig");
        std::fs::write(
            &global,
            format!(
                "[core]\n\taskPass = {0}\n[credential]\n\thelper = {0}\n",
                stand_in.display()
            ),
        )
        .unwrap();

        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["git::clone::tests::", "--test-threads=4", "--skip"])
            .arg("git::clone::tests::no_clone_test_runs_a_prompt_the_developer_configured")
            .env("SSH_ASKPASS", &stand_in)
            .env("GIT_ASKPASS", &stand_in)
            .env("GIT_CONFIG_GLOBAL", &global)
            .env("DISPLAY", ":99")
            .env_remove(PROMPTLESS)
            .output()
            .unwrap();

        assert!(
            out.status.success(),
            "a clone test failed beside the stand-ins:\n{}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert_eq!(std::fs::read_to_string(&ran).ok(), None, "a prompt was run");
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
            StartedBy::User,
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
            StartedBy::User,
        );

        assert!(matches!(result, Err(CloneError::Failed(_))), "{result:?}");
        assert!(!marker.exists(), "the address was obeyed as an option");
    }

    /// git and ssh have no terminal to ask on, so a transport that wants one
    /// fails at once with its own words, told by the environment never to ask.
    #[test]
    fn a_transport_that_reads_the_terminal_fails_at_once_and_nothing_asks() {
        if !runs_here_without_the_developers_prompts(
            "git::clone::tests::a_transport_that_reads_the_terminal_fails_at_once_and_nothing_asks",
        ) {
            return;
        }
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
            StartedBy::User,
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

    /// With no askpass configured, git's own and ssh's askpass fail at once,
    /// so nothing is waited on and the transport's own words come back.
    #[test]
    fn with_no_askpass_configured_nothing_is_waited_on() {
        if !runs_here_without_the_developers_prompts(
            "git::clone::tests::with_no_askpass_configured_nothing_is_waited_on",
        ) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
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
            StartedBy::User,
            &[("GIT_SSH_COMMAND", &ssh)],
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

    /// Ask the no-prompt command for a credential, the way a push or a fetch
    /// asks when the remote wants one. `config` is git configuration for the
    /// call; no credential helper answers, so only an askpass can.
    fn credential_answer(started_by: StartedBy, env: &[(&str, &str)], config: &[&str]) -> String {
        use std::io::Write as _;

        let mut command = network_git_command(started_by, env);
        command
            .args(["-c", "credential.helper="])
            .args(config)
            .args(["credential", "fill"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        let mut child = command.spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"protocol=https\nhost=example.invalid\n\n")
            .unwrap();
        String::from_utf8_lossy(&child.wait_with_output().unwrap().stdout).into_owned()
    }

    /// A stand-in askpass at `dir/askpass` that records each run in
    /// `dir/asked` and answers `stored-secret`.
    fn recording_askpass(dir: &Path) -> String {
        let script = dir.join("askpass");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\necho \"$*\" >> '{}'\necho stored-secret\n",
                dir.join("asked").display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        script.to_string_lossy().into_owned()
    }

    fn times_asked(dir: &Path) -> usize {
        std::fs::read_to_string(dir.join("asked")).map_or(0, |text| text.lines().count())
    }

    /// An askpass the user configured answers a call they started, wherever
    /// it is named, and is never run for one dux started by itself.
    #[test]
    fn a_configured_askpass_answers_a_user_started_call_and_never_a_background_one() {
        if !runs_here_without_the_developers_prompts(
            "git::clone::tests::a_configured_askpass_answers_a_user_started_call_and_never_a_background_one",
        ) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let script = recording_askpass(tmp.path());
        let core_askpass = format!("core.askPass={script}");
        let sources = [
            (
                "GIT_ASKPASS",
                vec![("GIT_ASKPASS", script.as_str())],
                vec![],
            ),
            (
                "SSH_ASKPASS",
                vec![("SSH_ASKPASS", script.as_str())],
                vec![],
            ),
            ("core.askPass", vec![], vec!["-c", core_askpass.as_str()]),
        ];

        for (source, env, config) in &sources {
            let (env, config) = (env.as_slice(), config.as_slice());
            let answer = credential_answer(StartedBy::User, env, config);
            assert!(
                answer.contains("password=stored-secret"),
                "{source}: {answer}"
            );
            let asked = times_asked(tmp.path());
            assert!(asked > 0, "{source}: a user-started call never ran it");

            let answer = credential_answer(StartedBy::Background, env, config);
            assert!(!answer.contains("password="), "{source}: {answer}");
            assert_eq!(
                times_asked(tmp.path()),
                asked,
                "{source}: a background call ran it"
            );
        }
    }

    /// ssh runs `SSH_ASKPASS` with no terminal unless `SSH_ASKPASS_REQUIRE` is
    /// `never` (measured on OpenSSH 10.2 for a host confirmation, a password and
    /// a key passphrase), so that is what tells it.
    #[test]
    fn ssh_may_run_a_configured_askpass_only_for_a_user_started_call() {
        if !runs_here_without_the_developers_prompts(
            "git::clone::tests::ssh_may_run_a_configured_askpass_only_for_a_user_started_call",
        ) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let script = recording_askpass(tmp.path());
        let seen = tmp.path().join("seen");
        let ssh = fake_ssh(
            tmp.path(),
            &format!(
                "echo \"require=$SSH_ASKPASS_REQUIRE\" >> '{}'\nexit 255",
                seen.display()
            ),
        );

        for (n, started_by) in [StartedBy::User, StartedBy::Background]
            .into_iter()
            .enumerate()
        {
            let result = clone_repository_with_env(
                "ssh://example.invalid/repo.git",
                &tmp.path().join(format!("clone{n}")),
                Duration::from_secs(30),
                &CloneProcesses::default(),
                started_by,
                &[("GIT_SSH_COMMAND", &ssh), ("SSH_ASKPASS", &script)],
            );
            assert!(matches!(result, Err(CloneError::Failed(_))), "{result:?}");
        }

        assert_eq!(
            std::fs::read_to_string(&seen).unwrap(),
            "require=\nrequire=never\n"
        );
    }

    /// A remote at `127.0.0.1` that answers every request with a 401 asking
    /// for Basic credentials, except one carrying `user:stored`, which gets a
    /// 404. The `Authorization` header of each request, or `none`, is recorded.
    fn credential_wanting_remote() -> (String, Arc<Mutex<Vec<String>>>) {
        use std::io::{BufRead as _, Write as _};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/repo.git", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let record = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
                let mut auth = "none".to_string();
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|read| read > 2) {
                    if let Some(value) = line.strip_prefix("Authorization: ") {
                        auth = value.trim().to_string();
                    }
                    line.clear();
                }
                let stored = auth == "Basic dXNlcjpzdG9yZWQ=";
                record.lock().unwrap().push(auth);
                let reply = if stored {
                    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                } else {
                    "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"x\"\r\n\
                     Content-Length: 0\r\nConnection: close\r\n\r\n"
                };
                let _ = stream.write_all(reply.as_bytes());
            }
        });
        (url, seen)
    }

    /// A credential helper that answers from its store signs a call in
    /// whoever started it, and no askpass is run beside it.
    #[test]
    fn a_stored_credential_signs_in_a_call_whoever_started_it() {
        if !runs_here_without_the_developers_prompts(
            "git::clone::tests::a_stored_credential_signs_in_a_call_whoever_started_it",
        ) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let script = recording_askpass(tmp.path());
        let (url, seen) = credential_wanting_remote();
        let store = tmp.path().join("credentials");
        let host = url
            .trim_start_matches("http://")
            .trim_end_matches("/repo.git");
        std::fs::write(
            &store,
            format!("http://user:stored@{}\n", host.replace(':', "%3a")),
        )
        .unwrap();
        let helper = format!("credential.helper=store --file={}", store.display());

        for started_by in [StartedBy::User, StartedBy::Background] {
            seen.lock().unwrap().clear();
            let out = network_git_command(started_by, &[("GIT_ASKPASS", &script)])
                .args(["-c", &helper, "ls-remote", &url])
                .output()
                .unwrap();
            let said = String::from_utf8_lossy(&out.stderr);
            assert!(said.contains("not found"), "{started_by:?}: {said}");
            assert!(
                seen.lock()
                    .unwrap()
                    .contains(&"Basic dXNlcjpzdG9yZWQ=".to_string()),
                "{started_by:?}: the stored credential never reached the remote"
            );
        }
        assert_eq!(
            times_asked(tmp.path()),
            0,
            "an askpass ran beside the store"
        );
    }

    /// The branch listing fetches by itself, so an askpass the repository
    /// names is not run; the listing says once, without a retry, what origin
    /// wanted and what to do.
    #[test]
    fn a_branch_listing_whose_origin_wants_a_credential_asks_nobody_and_says_so() {
        if !runs_here_without_the_developers_prompts(
            "git::clone::tests::a_branch_listing_whose_origin_wants_a_credential_asks_nobody_and_says_so",
        ) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let script = recording_askpass(tmp.path());
        let (url, seen) = credential_wanting_remote();
        let repo = tmp.path().join("repo");
        for args in [
            vec!["init", "-q", "-b", "main", repo.to_str().unwrap()],
            vec![
                "-C",
                repo.to_str().unwrap(),
                "remote",
                "add",
                "origin",
                &url,
            ],
            vec![
                "-C",
                repo.to_str().unwrap(),
                "config",
                "core.askPass",
                &script,
            ],
            vec![
                "-C",
                repo.to_str().unwrap(),
                "-c",
                "user.name=dux test",
                "-c",
                "user.email=dux-test@example.invalid",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "first",
            ],
        ] {
            assert!(
                git_command().args(&args).status().unwrap().success(),
                "{args:?}"
            );
        }

        let listing =
            crate::base_branch::load_branch_listing(&repo, Duration::from_secs(30)).unwrap();

        let note = listing.fetch_error().unwrap();
        assert!(
            note.starts_with(
                "origin wants a password, a key passphrase or a host confirmation, which dux \
                 does not ask for when it fetches by itself to list branches. Pull the project \
                 once"
            ),
            "{note}"
        );
        assert!(note.contains("terminal prompts disabled"), "{note}");
        assert_eq!(
            times_asked(tmp.path()),
            0,
            "the repository's askpass was run"
        );
        assert_eq!(seen.lock().unwrap().len(), 1, "the fetch was tried again");
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
            StartedBy::User,
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
            StartedBy::User,
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
                    StartedBy::User,
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
            StartedBy::User,
            &[("GIT_SSH_COMMAND", &ssh)],
        );
        assert_eq!(late, Err(CloneError::Stopped));
    }
}
