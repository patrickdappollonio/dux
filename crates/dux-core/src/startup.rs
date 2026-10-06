use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, Local, Utc};

use crate::config::{DuxPaths, StartupCommandTerminalConfig};
use crate::model::{AgentSession, Project};

pub const LOG_ROOT: &str = "startup-command-logs";

#[derive(Clone, Debug)]
pub struct StartupCommandRun {
    pub project: Project,
    pub session: AgentSession,
    /// The agent's managed working copy, carried beside the session rather than
    /// read back out of it.
    ///
    /// A startup command is a WORKTREE PROVISIONING step belonging to a
    /// project, so it only ever runs for a managed agent. Requiring the managed
    /// payload here is what makes that structural: a standalone agent has no
    /// value of this type to offer, so no caller can build a run for one.
    pub managed: crate::model::ManagedWorkspace,
    pub command: String,
    pub terminal: StartupCommandTerminalConfig,
    pub env: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct StartupCommandResult {
    pub session_id: String,
    pub project_name: String,
    pub log_path: PathBuf,
    pub status: Result<(), String>,
}

#[derive(Clone, Debug)]
pub struct StartupCommandLogEntry {
    pub path: PathBuf,
    pub display_name: String,
    pub modified_at: Option<DateTime<Local>>,
}

#[derive(Clone, Debug)]
pub enum StartupCommandLogScope {
    Agent {
        project_id: String,
        session_id: String,
    },
    Project {
        project_id: String,
    },
}

/// Every run recorded for one scope, newest first, plus the newest run's
/// contents pre-loaded.
///
/// The pre-load lets a picker render the newest run's output in the frame it
/// opens, with no second round of file I/O and no loading placeholder.
/// `content` is empty exactly when `entries` is, which is also the signal that
/// the scope has never run its startup command.
#[derive(Clone, Debug, Default)]
pub struct StartupCommandLogListing {
    pub entries: Vec<StartupCommandLogEntry>,
    pub content: String,
}

pub fn agent_log_dir(paths: &DuxPaths, project_id: &str, session_id: &str) -> PathBuf {
    paths.root.join(LOG_ROOT).join(project_id).join(session_id)
}

pub fn delete_agent_logs(paths: &DuxPaths, project_id: &str, session_id: &str) -> Result<()> {
    let dir = agent_log_dir(paths, project_id, session_id);
    if !dir.exists() {
        return Ok(());
    }
    fs::remove_dir_all(&dir).with_context(|| format!("failed to delete {}", dir.display()))
}

/// Fire-and-forget background deletion of an agent's startup-command logs.
/// Errors are logged but not surfaced to the caller, since session deletion
/// has already succeeded by the time this runs.
pub fn spawn_delete_startup_command_logs(paths: DuxPaths, project_id: String, session_id: String) {
    std::thread::spawn(move || {
        if let Err(err) = delete_agent_logs(&paths, &project_id, &session_id) {
            crate::logger::error(&format!(
                "failed to delete startup command logs for session {session_id}: {err:#}"
            ));
        }
    });
}

pub fn list_agent_logs(
    paths: &DuxPaths,
    project_id: &str,
    session_id: &str,
) -> Result<Vec<StartupCommandLogEntry>> {
    list_logs_in_dir(&agent_log_dir(paths, project_id, session_id))
}

pub fn list_project_logs(
    paths: &DuxPaths,
    project_id: &str,
) -> Result<Vec<StartupCommandLogEntry>> {
    let root = paths.root.join(LOG_ROOT).join(project_id);
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut logs = Vec::new();
    for entry in
        fs::read_dir(&root).with_context(|| format!("failed to read {}", root.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            logs.extend(list_logs_in_dir(&path)?);
        }
    }
    logs.sort_by(|a, b| {
        b.modified_at
            .cmp(&a.modified_at)
            .then_with(|| b.path.cmp(&a.path))
    });
    Ok(logs)
}

fn list_logs_in_dir(dir: &Path) -> Result<Vec<StartupCommandLogEntry>> {
    if !dir.exists() {
        return Ok(Vec::new());
    }
    let mut logs = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("log") {
            continue;
        }
        let modified_at = entry
            .metadata()
            .ok()
            .and_then(|meta| meta.modified().ok())
            .map(DateTime::<Local>::from);
        let display_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("startup-command.log")
            .to_string();
        logs.push(StartupCommandLogEntry {
            path,
            display_name,
            modified_at,
        });
    }
    logs.sort_by(|a, b| {
        b.modified_at
            .cmp(&a.modified_at)
            .then_with(|| b.path.cmp(&a.path))
    });
    Ok(logs)
}

pub fn read_log(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))
}

/// Every log run recorded for `scope`, newest first.
///
/// The one place the scope is matched. Callers ask here rather than re-deriving
/// "agent means this directory, project means every session directory under it",
/// so the two spellings cannot drift apart.
pub fn list_logs_for_scope(
    paths: &DuxPaths,
    scope: StartupCommandLogScope,
) -> Result<Vec<StartupCommandLogEntry>> {
    match scope {
        StartupCommandLogScope::Agent {
            project_id,
            session_id,
        } => list_agent_logs(paths, &project_id, &session_id),
        StartupCommandLogScope::Project { project_id } => list_project_logs(paths, &project_id),
    }
}

/// `scope`'s runs plus the newest run's contents, in one worker-thread trip.
///
/// Both halves are file I/O, so they belong on the same off-thread hop: a
/// caller that listed here and then read the newest on the UI thread would put
/// exactly the read this exists to avoid back on the UI thread.
pub fn load_logs_for_scope(
    paths: &DuxPaths,
    scope: StartupCommandLogScope,
) -> Result<StartupCommandLogListing> {
    let entries = list_logs_for_scope(paths, scope)?;
    let content = match entries.first() {
        Some(entry) => read_log(&entry.path)?,
        None => String::new(),
    };
    Ok(StartupCommandLogListing { entries, content })
}

pub fn open_path(path: &Path) -> Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    // Test builds only: never open a file on the developer's desktop.
    #[cfg(any(test, feature = "test-support"))]
    crate::test_provider::refuse_unlisted_launch("file opener", opener)?;
    Command::new(opener)
        .arg(path)
        .spawn()
        .with_context(|| format!("failed to run {opener} {}", path.display()))?;
    Ok(())
}

/// Run an agent's startup command in its worktree and record the run's log.
///
/// The command runs in a SESSION of its own (`setsid`), registered for the
/// agent in `registry`, so deleting the agent with its worktree ends it and
/// anything it left running, the same way it ends the agent's PTYs. One run
/// per agent at a time: a second one while the first is still going is
/// refused rather than run beside it in the same worktree.
///
/// When the agent is deleted while the command runs, no log is written: the
/// agent's log folder went with it and is never recreated for a run nobody can
/// look at any more.
pub fn run_startup_command(
    paths: &DuxPaths,
    run: StartupCommandRun,
    registry: &crate::process_sessions::AgentProcessRegistry,
) -> StartupCommandResult {
    match registry.begin_startup_run(&run.session.id) {
        Some(guard) => run_claimed_startup_command(paths, run, guard),
        None => {
            let log_path = startup_log_path(paths, &run);
            StartupCommandResult {
                status: Err(startup_already_running_message(
                    &run.session.display_label(),
                )),
                session_id: run.session.id,
                project_name: run.project.name,
                log_path,
            }
        }
    }
}

fn startup_log_path(paths: &DuxPaths, run: &StartupCommandRun) -> PathBuf {
    let log_dir = agent_log_dir(paths, &run.project.id, &run.session.id);
    let file_stamp = Utc::now().format("%Y%m%dT%H%M%SZ");
    let safe_branch = sanitize_file_component(&run.managed.branch_name);
    log_dir.join(format!("{file_stamp}-{safe_branch}.log"))
}

/// [`run_startup_command`] for a caller that has already claimed the agent's
/// one run, on its own thread, so a second request cannot slip in between the
/// claim and the worker starting. The surfaces' reruns claim on the engine
/// thread and refuse there, with a sentence, when the claim is taken.
pub fn run_claimed_startup_command(
    paths: &DuxPaths,
    run: StartupCommandRun,
    guard: crate::process_sessions::StartupRunGuard,
) -> StartupCommandResult {
    let log_dir = agent_log_dir(paths, &run.project.id, &run.session.id);
    // Checked under the claim, before anything is created or started: an
    // agent deleted after its run was claimed gets neither its command nor
    // its log folder back.
    let Some(created) = guard.unless_deleted(|| fs::create_dir_all(&log_dir)) else {
        crate::logger::info(&format!(
            "the startup command for agent {} was not run: the agent was deleted before it \
             started",
            run.session.id
        ));
        return StartupCommandResult {
            status: Err(format!(
                "agent \"{}\" was deleted before its startup command started, so dux did not \
                 run it",
                run.session.display_label()
            )),
            session_id: run.session.id,
            project_name: run.project.name,
            log_path: log_dir.join("not-run.log"),
        };
    };
    let timestamp = Utc::now();
    let file_stamp = timestamp.format("%Y%m%dT%H%M%SZ");
    let safe_branch = sanitize_file_component(&run.managed.branch_name);
    let log_path = log_dir.join(format!("{file_stamp}-{safe_branch}.log"));
    let result = (|| -> Result<CommandOutcome> {
        created.with_context(|| format!("failed to create {}", log_dir.display()))?;
        let shell = startup_shell_command(&run.terminal.command);
        let shell_args = run.terminal.args.clone();
        // Test builds only: a startup command runs through a stand-in shell,
        // never the developer's own login shell and its profile.
        #[cfg(any(test, feature = "test-support"))]
        crate::test_provider::refuse_unlisted_launch("startup-command shell", &shell)?;
        let started = Utc::now();
        let started_instant = Instant::now();
        let mut command = Command::new(&shell);
        command
            .args(&shell_args)
            .arg(&run.command)
            .current_dir(&run.managed.worktree_path)
            .env("DUX_PROJECT_PATH", &run.project.path)
            .env("DUX_WORKTREE_PATH", &run.managed.worktree_path)
            .env("DUX_AGENT_ID", &run.session.id)
            .env("DUX_AGENT_BRANCH", &run.managed.branch_name)
            .env("DUX_PROVIDER", run.session.provider.as_str())
            .env("DUX_STARTUP_COMMAND_LOG", &log_path);
        // Output goes to pipes that dux's own reader threads drain. A job the
        // command backgrounds without redirecting its output (`npm run dev
        // &`) inherits them, so they can stay open for as long as that job
        // lives: dux therefore never waits for them to close. It waits for
        // the COMMAND (`wait`, not `wait_with_output`), so the command is
        // reaped the moment it exits and what it left running is recorded at
        // that moment; what the readers have by then is the log, and from
        // then on they read and throw away whatever the job writes, so it can
        // neither block on a full pipe nor fill anything on disk.
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        for (name, value) in &run.env {
            command.env(name, value);
        }
        // SAFETY: the hook runs in the forked child before exec and calls only
        // `setsid`, which is async-signal-safe and touches no Rust state.
        unsafe {
            use std::os::unix::process::CommandExt;
            command.pre_exec(|| {
                rustix::process::setsid().map_err(std::io::Error::from)?;
                Ok(())
            });
        }
        // Announced before the spawn and kept until the session is
        // registered, so a removal of the worktree either sees the session or
        // came after the delete that refuses the spawn here.
        let worktree = std::path::Path::new(&run.managed.worktree_path);
        let Some(spawning) = guard.begin_spawn(worktree) else {
            anyhow::bail!(NOT_STARTED_DELETED);
        };
        let mut child = command
            .spawn()
            .with_context(|| format!("failed to run startup command through {shell}"))?;
        let stdout = OutputDrain::start(child.stdout.take());
        let stderr = OutputDrain::start(child.stderr.take());
        let process = crate::process_sessions::ProcessSession::started_now(child.id());
        crate::process_sessions::note_spawned(process);
        #[cfg(test)]
        tests_hooks::delay_registration(worktree);
        // Ended at once, by the registration itself, when the agent was
        // deleted between the check above and here.
        let _ended = guard.register_session(process, worktree);
        drop(spawning);
        guard.label(process, "an agent's startup command");
        drop(command);
        // The command (the session's leader) is waited for WITHOUT being
        // reaped first. While it is an unreaped zombie its session number
        // still names this session, so a removal looking now still finds
        // everything the command left running there by that number. What it
        // left running is recorded next, the only evidence those processes
        // are dux's once the number proves nothing; only then is the leader
        // reaped. Reaping first left a window (measured: a removal fell into
        // it under load) where the number no longer counted and nothing was
        // recorded yet, so a job the command left writing in the worktree was
        // invisible to the removal.
        wait_for_exit_unreaped(child.id());
        #[cfg(test)]
        tests_hooks::delay_after_exit(worktree);
        guard.record_survivors(process);
        let status = child.wait();
        if status.is_ok() {
            crate::process_sessions::note_reaped(process);
        }
        let status =
            status.with_context(|| format!("failed to run startup command through {shell}"))?;
        let ended = Utc::now();
        // What the command wrote before it exited. A job it left running may
        // still be writing; that is the job's output, not the command's.
        let stdout = stdout.finish();
        let stderr = stderr.finish();
        Ok(CommandOutcome {
            shell,
            shell_args,
            started,
            ended,
            duration_ms: started_instant.elapsed().as_millis(),
            code: status.code(),
            success: status.success(),
            stdout: String::from_utf8_lossy(&stdout).to_string(),
            stderr: String::from_utf8_lossy(&stderr).to_string(),
        })
    })();

    if guard.agent_deleted() {
        // Said as it happened: the delete ends the command, but a command
        // that finished first, or would not stop, is not "stopped". The logs
        // went with the agent, so none is pointed at.
        let label = run.session.display_label();
        let how = match &result {
            Ok(outcome) if outcome.code.is_some() => format!(
                "the command finished on its own first (exit status {})",
                format_exit_code(outcome.code)
            ),
            Ok(_) => "dux stopped the command".to_string(),
            Err(err) if err.to_string() == NOT_STARTED_DELETED => {
                "the command was never started".to_string()
            }
            Err(err) => format!("dux could not follow the command to its end ({err:#})"),
        };
        return StartupCommandResult {
            status: Err(format!(
                "agent \"{label}\" was deleted while its startup command was running: {how}, \
                 and no log of the run was kept because the agent's logs were deleted with it"
            )),
            session_id: run.session.id,
            project_name: run.project.name,
            log_path,
        };
    }

    let status = match result {
        Ok(outcome) => {
            let write_result = write_log(&log_path, &run, &outcome);
            if let Err(err) = write_result {
                Err(format!("{err:#}"))
            } else if outcome.success {
                Ok(())
            } else {
                Err(format!(
                    "exit status {}",
                    outcome
                        .code
                        .map(|code| code.to_string())
                        .unwrap_or_else(|| "terminated by signal".to_string())
                ))
            }
        }
        Err(err) => {
            let fallback = CommandOutcome {
                shell: startup_shell_command(&run.terminal.command),
                shell_args: run.terminal.args.clone(),
                started: timestamp,
                ended: Utc::now(),
                duration_ms: 0,
                code: None,
                success: false,
                stdout: String::new(),
                stderr: format!("{err:#}"),
            };
            let _ = write_log(&log_path, &run, &fallback);
            Err(format!("{err:#}"))
        }
    };

    StartupCommandResult {
        session_id: run.session.id,
        project_name: run.project.name,
        log_path,
        status,
    }
}

/// Block until the child `pid` has exited, leaving it unreaped (`WNOWAIT`),
/// so its pid, and the session number it leads, stay allocated until the
/// caller reaps it. Interrupted waits are retried; any other error returns at
/// once and the caller's ordinary `wait` reaps as before.
fn wait_for_exit_unreaped(pid: u32) {
    let Some(pid) = rustix::process::Pid::from_raw(pid as i32) else {
        return;
    };
    loop {
        match rustix::process::waitid(
            rustix::process::WaitId::Pid(pid),
            rustix::process::WaitIdOptions::EXITED | rustix::process::WaitIdOptions::NOWAIT,
        ) {
            Err(rustix::io::Errno::INTR) => continue,
            _ => return,
        }
    }
}

/// The error a run gives itself when its agent was deleted before the command
/// could start.
const NOT_STARTED_DELETED: &str = "the agent was deleted before the command started";

/// Test-only hooks into the run.
#[cfg(test)]
pub(crate) mod tests_hooks {
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::Duration;

    static DELAYS: Mutex<Option<HashMap<PathBuf, Duration>>> = Mutex::new(None);

    /// Hold every run in `worktree` for `delay` between starting its command
    /// and registering its session: the window a removal must not slip into.
    pub(crate) fn delay_registration_in(worktree: &Path, delay: Duration) {
        DELAYS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_or_insert_with(HashMap::new)
            .insert(worktree.to_path_buf(), delay);
    }

    static EXIT_DELAYS: Mutex<Option<HashMap<PathBuf, Duration>>> = Mutex::new(None);

    /// Hold every run in `worktree` for `delay` between its command exiting
    /// and what the command left running being recorded: the window a
    /// removal must not lose a job in.
    pub(crate) fn delay_after_exit_in(worktree: &Path, delay: Duration) {
        EXIT_DELAYS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_or_insert_with(HashMap::new)
            .insert(worktree.to_path_buf(), delay);
    }

    pub(super) fn delay_after_exit(worktree: &Path) {
        let delay = EXIT_DELAYS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(|delays| delays.get(worktree).copied());
        if let Some(delay) = delay {
            std::thread::sleep(delay);
        }
    }

    pub(super) fn delay_registration(worktree: &Path) {
        let delay = DELAYS
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .and_then(|delays| delays.get(worktree).copied());
        if let Some(delay) = delay {
            std::thread::sleep(delay);
        }
    }
}

/// The most of one output stream a startup command's log keeps.
const OUTPUT_LOG_CAP: usize = 16 * 1024 * 1024;

/// How long, once the command has exited, its readers get to take in what it
/// wrote before exiting (it is already in the pipe) before what they have is
/// the log.
const OUTPUT_SETTLE: std::time::Duration = std::time::Duration::from_millis(500);

/// One of the startup command's output pipes and the thread draining it.
struct OutputDrain {
    shared: Option<std::sync::Arc<DrainShared>>,
    /// The reader thread, which hands the pipe back when it is told to stop.
    reader: Option<std::thread::JoinHandle<Option<std::os::fd::OwnedFd>>>,
    /// The write end of the reader's wake pipe: closing it tells the reader
    /// to stop reading and hand the pipe back.
    wake: Option<std::os::fd::OwnedFd>,
}

#[derive(Default)]
struct DrainShared {
    state: std::sync::Mutex<DrainState>,
    changed: std::sync::Condvar,
}

#[derive(Default)]
struct DrainState {
    kept: Vec<u8>,
    /// From now on what is read is thrown away: the log was taken.
    discarding: bool,
    /// The pipe reached end of input (every holder closed it).
    ended: bool,
}

impl OutputDrain {
    /// Start draining `pipe` on a thread of its own, keeping what it reads
    /// until [`Self::finish`]. The thread never holds dux up and never lets a
    /// writer block.
    fn start(pipe: Option<impl Into<std::os::fd::OwnedFd>>) -> Self {
        let none = Self {
            shared: None,
            reader: None,
            wake: None,
        };
        let Some(pipe) = pipe else {
            return none;
        };
        let fd: std::os::fd::OwnedFd = pipe.into();
        // Without a wake pipe the reader cannot be told to stop, so it reads
        // for as long as dux runs, which still never lets a writer block.
        let (wake_read, wake_write) = match std::io::pipe() {
            Ok((read, write)) => (
                Some(std::os::fd::OwnedFd::from(read)),
                Some(std::os::fd::OwnedFd::from(write)),
            ),
            Err(_) => (None, None),
        };
        let shared = std::sync::Arc::new(DrainShared::default());
        let drain = std::sync::Arc::clone(&shared);
        let spawned = std::thread::Builder::new()
            .name("startup-output".to_string())
            .spawn(move || Self::read_until_done(fd, wake_read, &drain));
        match spawned {
            Ok(reader) => Self {
                shared: Some(shared),
                reader: Some(reader),
                wake: wake_write,
            },
            Err(_) => none,
        }
    }

    /// Read `fd` until it ends (answering `None`) or until `wake` reports
    /// its write end closed (answering the pipe, unread further, for handing
    /// on). Reads only once the pipe has something, and nothing else reads it
    /// while this runs, so a read never waits.
    fn read_until_done(
        fd: std::os::fd::OwnedFd,
        wake: Option<std::os::fd::OwnedFd>,
        drain: &DrainShared,
    ) -> Option<std::os::fd::OwnedFd> {
        use rustix::event::{PollFd, PollFlags};
        let mut buf = [0u8; 8192];
        loop {
            let (pipe_ready, woken) = {
                let mut fds = vec![PollFd::new(&fd, PollFlags::IN)];
                if let Some(wake) = &wake {
                    fds.push(PollFd::new(wake, PollFlags::IN));
                }
                match rustix::event::poll(&mut fds, None) {
                    Err(rustix::io::Errno::INTR) => continue,
                    Err(_) => (true, false),
                    Ok(_) => (
                        !fds[0].revents().is_empty(),
                        fds.get(1).is_some_and(|wake| !wake.revents().is_empty()),
                    ),
                }
            };
            if woken {
                return Some(fd);
            }
            if !pipe_ready {
                continue;
            }
            let read =
                crate::io_retry::retry_on_interrupt_errno(|| rustix::io::read(&fd, &mut buf));
            let mut state = drain
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match read {
                Ok(0) | Err(_) => {
                    state.ended = true;
                    drain.changed.notify_all();
                    return None;
                }
                Ok(n) => {
                    if !state.discarding {
                        let room = OUTPUT_LOG_CAP.saturating_sub(state.kept.len());
                        state.kept.extend_from_slice(&buf[..n.min(room)]);
                    }
                }
            }
        }
    }

    /// What was read, once the command has exited: everything if the pipe
    /// closes within [`OUTPUT_SETTLE`] (nothing else holds it), otherwise
    /// what had arrived by then.
    ///
    /// When something the command left running still holds the pipe, the
    /// pipe has to outlive dux, or that job's next write after dux quits
    /// raises SIGPIPE and kills it. So the reader thread is woken, ends and
    /// hands the read end back, and the read end goes to a small drain
    /// process of its own (see [`spawn_output_drain`]), which reads and
    /// discards until the last writer closes; dux keeps no copy. If that
    /// cannot start, a reader thread keeps discarding for as long as dux
    /// runs, and the log says the job may lose its output when dux quits.
    fn finish(mut self) -> Vec<u8> {
        let Some(shared) = self.shared.take() else {
            return Vec::new();
        };
        let deadline = std::time::Instant::now() + OUTPUT_SETTLE;
        let (kept, ended) = {
            let mut state = shared
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            while !state.ended {
                let now = std::time::Instant::now();
                if now >= deadline {
                    break;
                }
                state = shared
                    .changed
                    .wait_timeout(state, deadline - now)
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .0;
            }
            state.discarding = true;
            (std::mem::take(&mut state.kept), state.ended)
        };
        let Some(wake) = self.wake.take() else {
            if !ended {
                crate::logger::warn(
                    "could not hand a startup command's output to a drain of its own (no wake \
                     pipe for its reader); a job it left running may lose its output channel \
                     when dux quits",
                );
            }
            return kept;
        };
        // Wake the reader and wait for it: it is either gone already (the
        // pipe ended) or answers at once with the pipe.
        drop(wake);
        let handed_back = self
            .reader
            .take()
            .and_then(|reader| reader.join().ok())
            .flatten();
        if let Some(fd) = handed_back
            && let Err(err) = spawn_output_drain(&fd)
        {
            crate::logger::warn(&format!(
                "could not hand a startup command's output to a drain of its own ({err}); \
                 a job it left running may lose its output channel when dux quits"
            ));
            let drain = std::sync::Arc::clone(&shared);
            let _ = std::thread::Builder::new()
                .name("startup-output".to_string())
                .spawn(move || Self::read_until_done(fd, None, &drain));
        }
        kept
    }
}

/// Start the process that keeps a left-running job's output pipe open after
/// dux quits: `sh -c 'exec cat >/dev/null'` reading the pipe, in a session of
/// its own and with `/` as its folder so it never stands in a worktree. It
/// exits by itself when the last writer closes the pipe. dux does not follow
/// it as one of the agent's processes and never waits on it: a thread of its
/// own reaps it whenever it ends.
fn spawn_output_drain(read_end: &std::os::fd::OwnedFd) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    #[cfg(test)]
    if FAIL_NEXT_DRAIN.with(|fail| fail.replace(false)) {
        return Err(std::io::Error::other("a drain spawn failed on purpose"));
    }
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "exec cat >/dev/null"])
        .current_dir("/")
        .stdin(std::process::Stdio::from(read_end.try_clone()?))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    // SAFETY: the hook runs in the forked child before exec and calls only
    // `setsid`, which is async-signal-safe and touches no Rust state.
    unsafe {
        command.pre_exec(|| {
            rustix::process::setsid().map_err(std::io::Error::from)?;
            Ok(())
        });
    }
    let mut child = command.spawn()?;
    // The child holds the read end now; the parent's copy closes here.
    drop(command);
    #[cfg(test)]
    SPAWNED_DRAINS.with(|drains| drains.borrow_mut().push(child.id()));
    let _ = std::thread::Builder::new()
        .name("startup-output-drain-reaper".to_string())
        .spawn(move || {
            let _ = child.wait();
        });
    Ok(())
}

#[cfg(test)]
thread_local! {
    /// The drain processes this thread's startup runs started.
    static SPAWNED_DRAINS: std::cell::RefCell<Vec<u32>> = const { std::cell::RefCell::new(Vec::new()) };
    /// Make the next drain spawn on this thread fail.
    static FAIL_NEXT_DRAIN: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The refusal for a second startup-command run of one agent while the first
/// is still going. Both surfaces say it in these words.
pub fn startup_already_running_message(agent_label: &str) -> String {
    format!(
        "The startup command for agent \"{agent_label}\" is still running. Wait for it to \
         finish, then run it again; its log will show how the current run went."
    )
}

fn startup_shell_command(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "$SHELL" || trimmed == "${SHELL}" {
        return std::env::var("SHELL")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "/bin/sh".to_string());
    }

    crate::config::expand_env_vars(trimmed).unwrap_or_else(|| trimmed.to_string())
}

struct CommandOutcome {
    shell: String,
    shell_args: Vec<String>,
    started: DateTime<Utc>,
    ended: DateTime<Utc>,
    duration_ms: u128,
    code: Option<i32>,
    success: bool,
    stdout: String,
    stderr: String,
}

fn write_log(path: &Path, run: &StartupCommandRun, outcome: &CommandOutcome) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("startup command log path has no parent"))?;
    // Never created here: the run made the folder when it started, and a
    // folder that is gone by now was deleted with its agent. Recreating it
    // would leave a log folder behind for an agent that no longer exists.
    if !parent.is_dir() {
        return Err(anyhow!(
            "the startup command log folder {} no longer exists",
            parent.display()
        ));
    }
    let mut body = String::new();
    body.push_str("dux startup command log\n");
    body.push_str(&format!("started_at = {}\n", outcome.started.to_rfc3339()));
    body.push_str(&format!("ended_at = {}\n", outcome.ended.to_rfc3339()));
    body.push_str(&format!("duration_ms = {}\n", outcome.duration_ms));
    body.push_str(&format!("project_id = {}\n", run.project.id));
    body.push_str(&format!("project_name = {}\n", run.project.name));
    body.push_str(&format!("project_path = {}\n", run.project.path));
    body.push_str(&format!("agent_id = {}\n", run.session.id));
    body.push_str(&format!("agent_branch = {}\n", run.managed.branch_name));
    body.push_str(&format!("worktree_path = {}\n", run.managed.worktree_path));
    body.push_str(&format!("provider = {}\n", run.session.provider.as_str()));
    body.push_str(&format!("shell = {}\n", outcome.shell));
    body.push_str(&format!("shell_args = {:?}\n", outcome.shell_args));
    body.push_str(&format!("command = {}\n", run.command));
    body.push_str(&format!("exit_code = {}\n", format_exit_code(outcome.code)));
    body.push_str(&format!("success = {}\n", outcome.success));
    body.push_str("\n--- stdout ---\n");
    body.push_str(&outcome.stdout);
    if !outcome.stdout.ends_with('\n') {
        body.push('\n');
    }
    body.push_str("\n--- stderr ---\n");
    body.push_str(&outcome.stderr);
    if !outcome.stderr.ends_with('\n') {
        body.push('\n');
    }
    fs::write(path, body).with_context(|| format!("failed to write {}", path.display()))
}

fn format_exit_code(code: Option<i32>) -> String {
    code.map(|code| code.to_string())
        .unwrap_or_else(|| "none".to_string())
}

fn sanitize_file_component(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    sanitized
        .trim_matches('-')
        .chars()
        .take(80)
        .collect::<String>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use tempfile::tempdir;

    use crate::model::{ProjectBranchStatus, ProviderKind, SessionStatus};

    pub(super) fn test_paths(root: &Path) -> DuxPaths {
        DuxPaths {
            root: root.to_path_buf(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            socket_path: root.join("dux.sock"),
        }
    }

    pub(super) fn test_project(root: &Path) -> Project {
        Project {
            id: "project-1".to_string(),
            name: "demo".to_string(),
            path: root.to_string_lossy().to_string(),
            explicit_default_provider: None,
            default_provider: ProviderKind::from_str("codex"),
            leading_branch: Some("main".to_string()),
            auto_reopen_agents: None,
            startup_command: Some("echo setup".to_string()),
            env: Default::default(),
            current_branch: "main".to_string(),
            branch_status: ProjectBranchStatus::Leading,
            path_missing: false,
            created_at: None,
        }
    }

    pub(super) fn test_session(worktree: &Path) -> AgentSession {
        let now = Utc::now();
        AgentSession {
            id: "session-1".to_string(),
            slot_tab_id: "session-1-slot".to_string(),
            provider: ProviderKind::from_str("codex"),
            title: None,
            started_providers: Vec::new(),
            desired_running: true,
            auto_reopen_enabled: true,
            status: SessionStatus::Active,
            created_at: now,
            updated_at: now,
            last_focused_tab: None,
            workspace: crate::model::AgentWorkspace::Managed(crate::model::ManagedWorkspace {
                project_id: "project-1".to_string(),
                project_path: Some(worktree.to_string_lossy().to_string()),
                source_branch: "main".to_string(),
                branch_name: "feature/setup".to_string(),
                initial_branch: "feature/setup".to_string(),
                branch_provenance: crate::model::BranchProvenance::CreatedByDux,
                worktree_path: worktree.to_string_lossy().to_string(),
            }),
        }
    }

    #[test]
    fn startup_command_success_writes_log() {
        let tmp = tempdir().expect("tempdir");
        let paths = test_paths(tmp.path());
        let project = test_project(tmp.path());
        let session = test_session(tmp.path());
        let result = run_startup_command(
            &paths,
            StartupCommandRun {
                project,
                managed: session
                    .workspace
                    .as_managed()
                    .expect("test_session builds a managed agent")
                    .clone(),
                session,
                command: "printf hello".to_string(),
                terminal: StartupCommandTerminalConfig {
                    command: "/bin/sh".to_string(),
                    args: vec!["-c".to_string()],
                },
                env: Vec::new(),
            },
            &crate::process_sessions::AgentProcessRegistry::default(),
        );

        assert!(result.status.is_ok());
        let log = read_log(&result.log_path).expect("log");
        assert!(log.contains("success = true"));
        assert!(log.contains("command = printf hello"));
        assert!(log.contains("--- stdout ---\nhello"));
    }

    /// A shell outside the allowlist (the developer's own login shell, say) is
    /// refused before it runs, and the refusal is the recorded failure.
    #[test]
    fn a_startup_command_through_an_unlisted_shell_is_refused_in_test_builds() {
        let tmp = tempdir().expect("tempdir");
        let paths = test_paths(tmp.path());
        let project = test_project(tmp.path());
        let session = test_session(tmp.path());
        let result = run_startup_command(
            &paths,
            StartupCommandRun {
                project,
                managed: session
                    .workspace
                    .as_managed()
                    .expect("test_session builds a managed agent")
                    .clone(),
                session,
                command: "printf hello".to_string(),
                terminal: StartupCommandTerminalConfig {
                    command: "/usr/bin/zsh".to_string(),
                    args: vec!["-l".to_string(), "-c".to_string()],
                },
                env: Vec::new(),
            },
            &crate::process_sessions::AgentProcessRegistry::default(),
        );
        let err = result
            .status
            .expect_err("an unlisted shell must be refused");
        assert!(err.starts_with("test guard:"), "{err}");
    }

    #[test]
    fn opening_a_path_is_refused_in_test_builds() {
        let tmp = tempdir().expect("tempdir");
        let err = open_path(tmp.path()).unwrap_err();
        assert!(err.to_string().starts_with("test guard:"), "{err}");
    }

    #[test]
    fn startup_command_shell_defaults_to_login_non_interactive_mode() {
        let terminal = StartupCommandTerminalConfig::default();
        assert_eq!(terminal.command, "$SHELL");
        assert_eq!(terminal.args, ["-l", "-c"]);
    }

    #[test]
    fn startup_command_shell_expands_config_env_vars() {
        unsafe { std::env::set_var("DUX_TEST_STARTUP_SHELL", "/bin/sh") };
        assert_eq!(startup_shell_command("$DUX_TEST_STARTUP_SHELL"), "/bin/sh");
        unsafe { std::env::remove_var("DUX_TEST_STARTUP_SHELL") };
    }

    #[test]
    fn startup_command_failure_is_logged_without_erroring_log_write() {
        let tmp = tempdir().expect("tempdir");
        let paths = test_paths(tmp.path());
        let project = test_project(tmp.path());
        let session = test_session(tmp.path());
        let result = run_startup_command(
            &paths,
            StartupCommandRun {
                project,
                managed: session
                    .workspace
                    .as_managed()
                    .expect("test_session builds a managed agent")
                    .clone(),
                session,
                command: "printf nope >&2; exit 7".to_string(),
                terminal: StartupCommandTerminalConfig {
                    command: "/bin/sh".to_string(),
                    args: vec!["-c".to_string()],
                },
                env: Vec::new(),
            },
            &crate::process_sessions::AgentProcessRegistry::default(),
        );

        assert!(result.status.is_err());
        let log = read_log(&result.log_path).expect("log");
        assert!(log.contains("success = false"));
        assert!(log.contains("exit_code = 7"));
        assert!(log.contains("--- stderr ---"));
        assert!(log.contains("nope"));
    }

    #[test]
    fn startup_command_receives_project_env() {
        let tmp = tempdir().expect("tempdir");
        let paths = test_paths(tmp.path());
        let project = test_project(tmp.path());
        let session = test_session(tmp.path());
        let result = run_startup_command(
            &paths,
            StartupCommandRun {
                project,
                managed: session
                    .workspace
                    .as_managed()
                    .expect("test_session builds a managed agent")
                    .clone(),
                session,
                command: "printf \"$EDITOR:$API_KEY\"".to_string(),
                terminal: StartupCommandTerminalConfig {
                    command: "/bin/sh".to_string(),
                    args: vec!["-c".to_string()],
                },
                env: vec![
                    ("EDITOR".to_string(), "true".to_string()),
                    ("API_KEY".to_string(), "secret".to_string()),
                ],
            },
            &crate::process_sessions::AgentProcessRegistry::default(),
        );

        assert!(result.status.is_ok());
        let log = read_log(&result.log_path).expect("log");
        assert!(log.contains("--- stdout ---\ntrue:secret"));
    }

    fn sleeper_run(tmp: &Path, command: &str) -> StartupCommandRun {
        let session = test_session(tmp);
        StartupCommandRun {
            project: test_project(tmp),
            managed: session
                .workspace
                .as_managed()
                .expect("test_session builds a managed agent")
                .clone(),
            session,
            command: command.to_string(),
            terminal: StartupCommandTerminalConfig {
                command: "/bin/sh".to_string(),
                args: vec!["-c".to_string()],
            },
            env: Vec::new(),
        }
    }

    fn wait_for_pid(path: &Path) -> i32 {
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Some(pid) = fs::read_to_string(path)
                .ok()
                .and_then(|text| text.trim().parse().ok())
            {
                return pid;
            }
            assert!(Instant::now() < deadline, "the command never started");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// A command that backgrounds a job without redirecting its output ends
    /// when the command ends, not when the job does: dux reaps it at once,
    /// records the job as what it left running, and logs exactly what the
    /// command wrote, in the same shape as ever.
    #[test]
    fn a_startup_job_holding_the_output_does_not_hold_the_run() {
        let tmp = tempdir().expect("tempdir");
        let paths = test_paths(tmp.path());
        let pidfile = tmp.path().join("job.pid");
        let registry = crate::process_sessions::AgentProcessRegistry::default();
        let started = Instant::now();
        let result = run_startup_command(
            &paths,
            sleeper_run(
                tmp.path(),
                &format!(
                    "echo before; echo oops >&2; (sleep 30; echo late) & echo $! > '{}'; exit 0",
                    pidfile.display()
                ),
            ),
            &registry,
        );
        let elapsed = started.elapsed();
        let job = wait_for_pid(&pidfile);
        let session = registry.sessions_of("session-1");
        let recorded = registry.survivors_of(&session);
        // The job and its own `sleep` are in the command's process group (the
        // command led its session and group): end them all.
        for leader in &session {
            if let Some(group) = rustix::process::Pid::from_raw(leader.sid as i32) {
                let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
            }
        }
        if let Some(pid) = rustix::process::Pid::from_raw(job) {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
        }
        assert!(
            elapsed < std::time::Duration::from_secs(10),
            "the run waited {elapsed:?} for the job it left running"
        );
        assert!(result.status.is_ok(), "{:?}", result.status);
        assert!(
            recorded.iter().any(|identity| identity.pid as i32 == job),
            "the job was recorded as what the command left running: {recorded:?}"
        );
        let log = read_log(&result.log_path).expect("log");
        assert!(log.contains("--- stdout ---\nbefore\n"), "{log}");
        assert!(log.contains("--- stderr ---\noops\n"), "{log}");
        assert!(
            !log.contains("\nlate\n"),
            "the job's later output is not the command's: {log}"
        );
        let leftovers: Vec<_> = fs::read_dir(result.log_path.parent().unwrap())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".startup-output")
            })
            .collect();
        assert!(leftovers.is_empty(), "no capture file is left behind");
    }

    /// One run per agent: a second run while the first is going is refused
    /// with a sentence, and nothing of it runs.
    #[test]
    fn a_second_run_for_the_same_agent_is_refused_while_the_first_runs() {
        let tmp = tempdir().expect("tempdir");
        let paths = test_paths(tmp.path());
        let registry = crate::process_sessions::AgentProcessRegistry::default();
        let _first = registry.begin_startup_run("session-1").expect("first run");
        let result = run_startup_command(&paths, sleeper_run(tmp.path(), "touch ran"), &registry);
        let err = result.status.expect_err("the second run is refused");
        assert!(err.contains("is still running"), "{err}");
        assert!(
            !tmp.path().join("ran").exists(),
            "the refused run never ran"
        );
    }

    /// The command leads a session of its own, registered for its agent, so a
    /// worktree removal can end it and whatever it leaves running.
    #[test]
    fn the_startup_command_runs_in_a_session_of_its_own_registered_for_its_agent() {
        let tmp = tempdir().expect("tempdir");
        let paths = test_paths(tmp.path());
        let registry = crate::process_sessions::AgentProcessRegistry::default();
        let run = sleeper_run(tmp.path(), "echo $$ > pid.txt; sleep 1");
        let thread_registry = registry.clone();
        let handle = std::thread::spawn(move || run_startup_command(&paths, run, &thread_registry));
        let pid = wait_for_pid(&tmp.path().join("pid.txt"));
        let sid = rustix::process::getsid(rustix::process::Pid::from_raw(pid))
            .expect("getsid")
            .as_raw_nonzero()
            .get();
        assert_eq!(sid, pid, "the command leads its own session");
        assert!(registry.startup_running("session-1"));
        let result = handle.join().expect("run thread");
        assert!(result.status.is_ok(), "{:?}", result.status);
        assert!(
            !registry.startup_running("session-1"),
            "the claim is released"
        );
        assert!(
            registry
                .sessions_of("session-1")
                .iter()
                .any(|session| session.sid == pid as u32),
            "its session is registered for the agent"
        );
    }

    /// review8: an agent deleted with its worktree KEPT while its startup
    /// command runs: nothing ends the command, it runs to completion (here it
    /// succeeds), and the result still tells the user dux stopped it.
    #[test]
    fn review8_a_keep_worktree_delete_does_not_claim_it_stopped_the_command() {
        let tmp = tempdir().expect("tempdir");
        let paths = test_paths(tmp.path());
        let registry = crate::process_sessions::AgentProcessRegistry::default();
        let done = tmp.path().join("done.txt");
        let run = sleeper_run(
            tmp.path(),
            &format!("echo $$ > pid.txt; sleep 1; echo ok > '{}'", done.display()),
        );
        let thread_paths = paths.clone();
        let thread_registry = registry.clone();
        let handle =
            std::thread::spawn(move || run_startup_command(&thread_paths, run, &thread_registry));
        wait_for_pid(&tmp.path().join("pid.txt"));
        // What a keep-worktree delete does: the record goes, nothing is ended.
        delete_agent_logs(&paths, "project-1", "session-1").expect("delete logs");
        let _ = registry.forget_agent("session-1");
        let result = handle.join().expect("run thread");
        let err = result.status.expect_err("the run reports the deletion");
        // Forgetting the agent now ends its startup command (review 10), so
        // the run is stopped; what matters is that the sentence says what
        // actually happened either way.
        if done.exists() {
            assert!(
                !err.contains("dux stopped the command"),
                "the command ran to completion, but the user is told: {err}"
            );
        } else {
            assert!(
                err.contains("dux stopped the command"),
                "the command was ended, but the user is told: {err}"
            );
        }
        assert!(!err.contains("log:"), "no deleted log is pointed at: {err}");
    }

    /// An agent deleted while its startup command runs takes its log folder
    /// with it, and the run must not put the folder back.
    #[test]
    fn a_run_whose_agent_was_deleted_does_not_recreate_its_log_folder() {
        let tmp = tempdir().expect("tempdir");
        let paths = test_paths(tmp.path());
        let registry = crate::process_sessions::AgentProcessRegistry::default();
        let run = sleeper_run(tmp.path(), "echo $$ > pid.txt; sleep 1");
        let thread_paths = paths.clone();
        let thread_registry = registry.clone();
        let handle =
            std::thread::spawn(move || run_startup_command(&thread_paths, run, &thread_registry));
        wait_for_pid(&tmp.path().join("pid.txt"));
        // What deleting the agent does to its logs and its registration.
        delete_agent_logs(&paths, "project-1", "session-1").expect("delete logs");
        let _ = registry.forget_agent("session-1");
        let result = handle.join().expect("run thread");
        let err = result.status.expect_err("the run reports the deletion");
        assert!(err.contains("was deleted"), "{err}");
        assert!(
            !agent_log_dir(&paths, "project-1", "session-1").exists(),
            "the deleted agent's log folder stays gone"
        );
    }

    /// review10: a rerun is claimed on the engine thread and its worker starts
    /// a moment later. An agent deleted (worktree kept) in between has its
    /// logs deleted and its registration forgotten; `startup_session_of` is
    /// still `None`, so the delete ends nothing. The worker must not then
    /// start the deleted agent's command anyway, nor bring its log folder back.
    #[test]
    fn review10_a_rerun_claimed_before_a_delete_never_starts_for_the_deleted_agent() {
        let tmp = tempdir().expect("tempdir");
        let paths = test_paths(tmp.path());
        let registry = crate::process_sessions::AgentProcessRegistry::default();
        // claim_startup_rerun, on the engine thread.
        let guard = registry.begin_startup_run("session-1").expect("claim");
        // The delete lands before the worker runs: what a keep-worktree
        // delete does (logs gone, registry forgotten, nothing to end yet).
        assert!(registry.startup_session_of("session-1").is_none());
        let _ = delete_agent_logs(&paths, "project-1", "session-1");
        let _ = registry.forget_agent("session-1");
        // Now the worker.
        let marker = tmp.path().join("ran-after-delete");
        let result = run_claimed_startup_command(
            &paths,
            sleeper_run(tmp.path(), &format!("touch '{}'", marker.display())),
            guard,
        );
        let ran = marker.exists();
        let log_dir_back = agent_log_dir(&paths, "project-1", "session-1").exists();
        assert!(
            !ran && !log_dir_back,
            "deleted agent's startup command ran: {ran}; its deleted log folder was recreated: \
             {log_dir_back}; result: {:?}",
            result.status
        );
    }

    /// Writing a run's log never creates its folder: a folder that is gone by
    /// then was deleted with its agent.
    #[test]
    fn writing_a_log_never_recreates_a_deleted_log_folder() {
        let tmp = tempdir().expect("tempdir");
        let gone = tmp.path().join("deleted-agent");
        let outcome = CommandOutcome {
            shell: "/bin/sh".to_string(),
            shell_args: Vec::new(),
            started: Utc::now(),
            ended: Utc::now(),
            duration_ms: 0,
            code: Some(0),
            success: true,
            stdout: String::new(),
            stderr: String::new(),
        };
        let err = write_log(
            &gone.join("run.log"),
            &sleeper_run(tmp.path(), "true"),
            &outcome,
        )
        .expect_err("no folder, no log");
        assert!(err.to_string().contains("no longer exists"), "{err:#}");
        assert!(!gone.exists(), "the folder was not put back");
    }

    #[test]
    fn delete_agent_logs_removes_session_directory() {
        let tmp = tempdir().expect("tempdir");
        let paths = test_paths(tmp.path());
        let dir = agent_log_dir(&paths, "project-1", "session-1");
        fs::create_dir_all(&dir).expect("log dir");
        fs::write(dir.join("one.log"), "log").expect("log file");

        delete_agent_logs(&paths, "project-1", "session-1").expect("delete logs");

        assert!(!dir.exists());
    }

    /// Seed two agent runs an hour apart and return `(older, newer)` paths.
    /// The listing sorts by mtime then path, so the mtimes are set explicitly
    /// rather than relying on write order.
    fn seed_two_runs(paths: &DuxPaths, session_id: &str) -> (PathBuf, PathBuf) {
        let dir = agent_log_dir(paths, "project-1", session_id);
        fs::create_dir_all(&dir).expect("log dir");
        let older = dir.join("20260101T000000Z-old.log");
        let newer = dir.join("20260101T010000Z-new.log");
        fs::write(&older, "older run output").expect("older log");
        fs::write(&newer, "newer run output").expect("newer log");
        let base =
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_767_225_600);
        set_mtime(&older, base);
        set_mtime(&newer, base + std::time::Duration::from_secs(3600));
        (older, newer)
    }

    fn set_mtime(path: &Path, when: std::time::SystemTime) {
        let file = fs::File::options()
            .write(true)
            .open(path)
            .expect("open for mtime");
        file.set_modified(when).expect("set mtime");
    }

    #[test]
    fn load_logs_for_scope_lists_every_run_newest_first_with_the_newest_preloaded() {
        let tmp = tempdir().expect("tempdir");
        let paths = test_paths(tmp.path());
        let (older, newer) = seed_two_runs(&paths, "session-1");

        let listing = load_logs_for_scope(
            &paths,
            StartupCommandLogScope::Agent {
                project_id: "project-1".to_string(),
                session_id: "session-1".to_string(),
            },
        )
        .expect("listing");

        assert_eq!(
            listing
                .entries
                .iter()
                .map(|entry| entry.path.clone())
                .collect::<Vec<_>>(),
            vec![newer, older],
            "every run must be listed, newest first"
        );
        assert_eq!(
            listing.content, "newer run output",
            "and the newest run's contents must arrive pre-loaded"
        );
    }

    #[test]
    fn load_logs_for_scope_spans_every_session_of_a_project() {
        let tmp = tempdir().expect("tempdir");
        let paths = test_paths(tmp.path());
        seed_two_runs(&paths, "session-1");
        let (_, newest) = seed_two_runs(&paths, "session-2");
        // Push session-2's newest past session-1's so the project scope has an
        // unambiguous head.
        set_mtime(
            &newest,
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_767_400_000),
        );
        fs::write(&newest, "session two output").expect("rewrite");
        set_mtime(
            &newest,
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_767_400_000),
        );

        let listing = load_logs_for_scope(
            &paths,
            StartupCommandLogScope::Project {
                project_id: "project-1".to_string(),
            },
        )
        .expect("listing");

        assert_eq!(listing.entries.len(), 4, "both sessions' runs are in scope");
        assert_eq!(listing.entries[0].path, newest);
        assert_eq!(listing.content, "session two output");
    }

    #[test]
    fn load_logs_for_scope_reports_a_scope_that_has_never_run() {
        let tmp = tempdir().expect("tempdir");
        let paths = test_paths(tmp.path());

        let listing = load_logs_for_scope(
            &paths,
            StartupCommandLogScope::Agent {
                project_id: "project-1".to_string(),
                session_id: "session-1".to_string(),
            },
        )
        .expect("listing");

        assert!(listing.entries.is_empty());
        assert!(
            listing.content.is_empty(),
            "an empty scope carries no placeholder prose; the caller decides \
             what to say about it"
        );
    }

    /// A job the startup command left running keeps writing into the capture
    /// file dux made for the command's output, which was unlinked at once:
    /// after the run has returned and its log is written, nothing ever reads
    /// it, no file names it, and it grows on disk in dux's config folder for
    /// as long as the job runs.
    #[test]
    fn review15_a_left_running_job_does_not_fill_an_invisible_file_in_the_config_folder() {
        let tmp = tempdir().expect("tempdir");
        let paths = test_paths(tmp.path());
        let pidfile = tmp.path().join("job.pid");
        // The job measures its own standard output once it has written, with
        // fstat, and reports the size through a file: the same answer on Linux
        // and macOS, with no /proc. The report is renamed into place so a
        // partial one is never read.
        let report = tmp.path().join("stdout-size");
        let registry = crate::process_sessions::AgentProcessRegistry::default();
        let result = run_startup_command(
            &paths,
            sleeper_run(
                tmp.path(),
                &format!(
                    "(sleep 0.3; head -c 20000000 /dev/zero; \
                     python3 -c \"import os,sys; t=sys.argv[1]+'.tmp'; \
                     open(t,'w').write(str(os.fstat(1).st_size)); os.rename(t,sys.argv[1])\" '{}'; \
                     sleep 30) & echo $! > '{}'; exit 0",
                    report.display(),
                    pidfile.display()
                ),
            ),
            &registry,
        );
        assert!(result.status.is_ok(), "{:?}", result.status);
        let job = wait_for_pid(&pidfile);
        let deadline = Instant::now() + std::time::Duration::from_secs(20);
        let size = loop {
            if let Some(size) = fs::read_to_string(&report)
                .ok()
                .and_then(|text| text.trim().parse::<u64>().ok())
            {
                break Some(size);
            }
            if Instant::now() >= deadline {
                break None;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        };
        for leader in &registry.sessions_of("session-1") {
            if let Some(group) = rustix::process::Pid::from_raw(leader.sid as i32) {
                let _ = rustix::process::kill_process_group(group, rustix::process::Signal::KILL);
            }
        }
        if let Some(pid) = rustix::process::Pid::from_raw(job) {
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
        }
        let size = size.expect("the job never reported the size of its standard output");
        assert!(
            size < 1_000_000,
            "the job's standard output holds {size} bytes: an unlinked file in dux's own \
             folder that nothing reads and no listing shows"
        );
    }
}

#[cfg(test)]
mod review16_tests {
    use super::*;
    use std::time::{Duration, Instant};

    /// The descriptors this process holds on the pipe `inode` names.
    #[cfg(target_os = "linux")]
    fn own_handles_on(inode: &str) -> usize {
        std::fs::read_dir("/proc/self/fd")
            .unwrap()
            .filter_map(|entry| std::fs::read_link(entry.ok()?.path()).ok())
            .filter(|target| target.to_string_lossy() == inode)
            .count()
    }

    #[cfg(target_os = "linux")]
    fn pipe_inode(fd: &std::os::fd::OwnedFd) -> String {
        use std::os::fd::AsRawFd;
        std::fs::read_link(format!("/proc/self/fd/{}", fd.as_raw_fd()))
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    /// The hand-off ends dux's reader for good: once the output is handed to
    /// the drain process, no descriptor of dux's (the reader thread's
    /// included) is left on the pipe, and the drain still reads it.
    #[test]
    #[cfg(target_os = "linux")]
    fn the_hand_off_leaves_dux_no_reader_on_the_pipe() {
        let (read_end, write_end) = std::io::pipe().unwrap();
        let (read_end, write_end) = (
            std::os::fd::OwnedFd::from(read_end),
            std::os::fd::OwnedFd::from(write_end),
        );
        let inode = pipe_inode(&read_end);
        let drain = OutputDrain::start(Some(read_end));
        rustix::io::write(&write_end, b"before\n").unwrap();
        let kept = drain.finish();
        assert_eq!(kept, b"before\n");
        let drains = SPAWNED_DRAINS.with(|drains| std::mem::take(&mut *drains.borrow_mut()));
        assert_eq!(
            drains.len(),
            1,
            "the still-held pipe went to a drain process"
        );
        assert_eq!(
            own_handles_on(&inode),
            1,
            "the only descriptor left on the pipe is the writer's (the job's): dux holds no \
             reader on it any more"
        );
        // The drain reads on: a writer never blocks, well past a pipe's buffer.
        let writer = std::thread::spawn(move || {
            let chunk = [b'x'; 65536];
            for _ in 0..32 {
                rustix::io::write(&write_end, &chunk).unwrap();
            }
            write_end
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while !writer.is_finished() {
            assert!(Instant::now() < deadline, "the drain stopped reading");
            std::thread::sleep(Duration::from_millis(10));
        }
        drop(writer.join().unwrap());
        // The last writer closed: the drain ends by itself.
        let deadline = Instant::now() + Duration::from_secs(10);
        while std::path::Path::new(&format!("/proc/{}", drains[0])).exists() {
            assert!(
                Instant::now() < deadline,
                "the drain outlived its last writer"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// A drain process that cannot start leaves a reader of dux's draining
    /// the pipe, so the job still never blocks while dux runs.
    #[test]
    fn a_drain_that_cannot_start_leaves_dux_draining() {
        let (read_end, write_end) = std::io::pipe().unwrap();
        let (read_end, write_end) = (
            std::os::fd::OwnedFd::from(read_end),
            std::os::fd::OwnedFd::from(write_end),
        );
        let drain = OutputDrain::start(Some(read_end));
        FAIL_NEXT_DRAIN.with(|fail| fail.set(true));
        assert!(drain.finish().is_empty());
        assert!(
            SPAWNED_DRAINS.with(|drains| drains.borrow().is_empty()),
            "no drain process started"
        );
        let writer = std::thread::spawn(move || {
            let chunk = [b'x'; 65536];
            for _ in 0..32 {
                rustix::io::write(&write_end, &chunk).unwrap();
            }
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        while !writer.is_finished() {
            assert!(Instant::now() < deadline, "nothing drained the pipe");
            std::thread::sleep(Duration::from_millis(10));
        }
        writer.join().unwrap();
    }

    /// Runs only when the parent test asks for it, in a process of its own:
    /// it stands in for a dux that runs a startup command and then quits.
    #[test]
    #[ignore]
    fn review16_helper_run_startup_command_then_quit() {
        let Ok(dir) = std::env::var("DUX_REVIEW16_DIR") else {
            return;
        };
        let dir = PathBuf::from(dir);
        let pidfile = dir.join("job.pid");
        let session = super::tests::test_session(&dir);
        let run = StartupCommandRun {
            project: super::tests::test_project(&dir),
            managed: session.workspace.as_managed().unwrap().clone(),
            session,
            command: format!(
                "(while true; do echo tick; sleep 0.1; done) & echo $! > '{}'; exit 0",
                pidfile.display()
            ),
            terminal: crate::config::StartupCommandTerminalConfig {
                command: "/bin/sh".to_string(),
                args: vec!["-c".to_string()],
            },
            env: Vec::new(),
        };
        let paths = super::tests::test_paths(&dir);
        let registry = crate::process_sessions::AgentProcessRegistry::default();
        let result = run_startup_command(&paths, run, &registry);
        assert!(result.status.is_ok(), "{:?}", result.status);
        // dux quits now (the process exits when this test returns).
    }

    /// The docs say a job the startup command leaves running in the
    /// background keeps running after the command exits. It does, only for
    /// as long as dux does: its output is a pipe whose only reader is a dux
    /// thread, so when dux quits the job's next write gets SIGPIPE and the
    /// job (a dev server, say) is killed.
    #[test]
    fn review16_a_left_running_job_survives_dux_quitting() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = std::env::current_exe().unwrap();
        let status = std::process::Command::new(exe)
            .args([
                "startup::review16_tests::review16_helper_run_startup_command_then_quit",
                "--exact",
                "--ignored",
                "--test-threads=1",
                "--nocapture",
            ])
            .env("DUX_REVIEW16_DIR", tmp.path())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "the helper run failed");
        let pidfile = tmp.path().join("job.pid");
        let job: i32 = fs::read_to_string(&pidfile)
            .expect("job pid")
            .trim()
            .parse()
            .unwrap();
        let started = Instant::now();
        std::thread::sleep(Duration::from_millis(1500));
        let alive = alive(job as u32);
        // Clean up the job's whole group, whatever happened.
        if let Some(pid) = rustix::process::Pid::from_raw(job) {
            if let Ok(pgid) = rustix::process::getpgid(Some(pid)) {
                let _ = rustix::process::kill_process_group(pgid, rustix::process::Signal::KILL);
            }
            let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
        }
        let _ = started;
        assert!(
            alive,
            "the job the startup command left running died once dux quit: its stdout was a \
             pipe only dux read, so its next write raised SIGPIPE"
        );
    }

    /// Running and not a zombie, on any platform.
    fn alive(pid: u32) -> bool {
        crate::file_drop::process_can_answer(pid)
    }

    fn wait_gone(pid: u32) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while alive(pid) {
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        true
    }

    /// A command that leaves nothing running holding its output gets no
    /// drain process: its pipes close with it.
    #[test]
    fn no_drain_is_started_when_nothing_holds_the_output() {
        let tmp = tempfile::tempdir().unwrap();
        let paths = super::tests::test_paths(tmp.path());
        let session = super::tests::test_session(tmp.path());
        SPAWNED_DRAINS.with(|drains| drains.borrow_mut().clear());
        let result = run_startup_command(
            &paths,
            StartupCommandRun {
                project: super::tests::test_project(tmp.path()),
                managed: session.workspace.as_managed().unwrap().clone(),
                session,
                command: "echo done; (sleep 0.1 >/dev/null 2>&1 &) ; exit 0".to_string(),
                terminal: crate::config::StartupCommandTerminalConfig {
                    command: "/bin/sh".to_string(),
                    args: vec!["-c".to_string()],
                },
                env: Vec::new(),
            },
            &crate::process_sessions::AgentProcessRegistry::default(),
        );
        assert!(result.status.is_ok(), "{:?}", result.status);
        assert!(
            SPAWNED_DRAINS.with(|drains| drains.borrow().is_empty()),
            "no drain for a command whose output nothing else holds"
        );
    }

    /// The drain process a left-running job's output is handed to stands in
    /// `/`, belongs to no session dux follows, never occupies the worktree,
    /// and ends by itself once the job is ended by a removal.
    #[test]
    fn the_drain_never_occupies_the_worktree_and_ends_with_the_job() {
        let tmp = tempfile::tempdir().unwrap();
        let worktree = tmp.path().join("wt");
        fs::create_dir_all(&worktree).unwrap();
        let paths = super::tests::test_paths(tmp.path());
        let session = super::tests::test_session(&worktree);
        let registry = crate::process_sessions::AgentProcessRegistry::default();
        SPAWNED_DRAINS.with(|drains| drains.borrow_mut().clear());
        let result = run_startup_command(
            &paths,
            StartupCommandRun {
                project: super::tests::test_project(tmp.path()),
                managed: session.workspace.as_managed().unwrap().clone(),
                session,
                // A job that leaves the worktree and keeps writing.
                command: "(cd /; while true; do echo tick; sleep 0.1; done) & exit 0".to_string(),
                terminal: crate::config::StartupCommandTerminalConfig {
                    command: "/bin/sh".to_string(),
                    args: vec!["-c".to_string()],
                },
                env: Vec::new(),
            },
            &registry,
        );
        // Whatever happens below, the job's session is ended when the test
        // ends, and with it the drains (their last writer closes).
        struct EndSessions(Vec<crate::process_sessions::ProcessSession>);
        impl Drop for EndSessions {
            fn drop(&mut self) {
                for session in &self.0 {
                    if let Some(group) = rustix::process::Pid::from_raw(session.sid as i32) {
                        let _ = rustix::process::kill_process_group(
                            group,
                            rustix::process::Signal::KILL,
                        );
                    }
                }
            }
        }
        let sessions = registry.sessions_in(&worktree);
        let _end = EndSessions(sessions.clone());
        assert!(result.status.is_ok(), "{:?}", result.status);
        let drains = SPAWNED_DRAINS.with(|drains| drains.borrow().clone());
        // The job holds both of the command's output streams: one drain each.
        assert_eq!(drains.len(), 2, "each held stream was handed to a drain");
        for drain in &drains {
            assert!(alive(*drain), "the drain runs while the job does");
            assert_eq!(
                crate::file_drop::process_cwds(&[*drain]).found.get(drain),
                Some(&std::path::PathBuf::from("/"))
            );
        }
        // Not in the worktree's way: no tracked session holds it, and its
        // folder is `/`.
        assert_eq!(registry.cwd_occupant(&worktree, &[]), None);
        // A removal of the worktree ends the job (a session dux recorded
        // there) and is not held up by the drains, which then end by
        // themselves.
        assert!(!sessions.is_empty());
        let outcome = crate::process_sessions::purge(
            &mut crate::process_sessions::SystemProcesses,
            &sessions,
            &registry.survivors_of(&sessions),
            Duration::from_secs(1),
        );
        assert!(
            matches!(outcome, crate::process_sessions::PurgeOutcome::Clean { .. }),
            "{outcome:?}"
        );
        for drain in &drains {
            assert!(
                wait_gone(*drain),
                "the drain ended once the last writer closed"
            );
        }
    }
}
