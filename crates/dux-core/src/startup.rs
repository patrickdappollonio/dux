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
    let timestamp = Utc::now();
    let file_stamp = timestamp.format("%Y%m%dT%H%M%SZ");
    let safe_branch = sanitize_file_component(&run.managed.branch_name);
    let log_path = log_dir.join(format!("{file_stamp}-{safe_branch}.log"));
    let result = (|| -> Result<CommandOutcome> {
        fs::create_dir_all(&log_dir)
            .with_context(|| format!("failed to create {}", log_dir.display()))?;
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
        // Output goes to files, never pipes. A job the command backgrounds
        // without redirecting its output (`npm run dev &`) inherits whatever
        // the command writes to; a pipe would stay open for as long as that
        // job lives, so dux could not tell the command had ended (and could
        // not reap it) until the job ended too. A file has no such tie: the
        // command is reaped the moment it exits, and what it left running is
        // recorded at that moment.
        let mut stdout = capture_file(&log_dir)?;
        let mut stderr = capture_file(&log_dir)?;
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::from(stdout.try_clone()?))
            .stderr(std::process::Stdio::from(stderr.try_clone()?));
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
        let child = command
            .spawn()
            .with_context(|| format!("failed to run startup command through {shell}"))?;
        let process = crate::process_sessions::ProcessSession::started_now(child.id());
        crate::process_sessions::note_spawned(process);
        guard.register_session(process, std::path::Path::new(&run.managed.worktree_path));
        guard.label(process, "an agent's startup command");
        // Dropped here, so the only copies of the capture files' descriptors
        // left are the command's own (and those of anything it starts).
        drop(command);
        let mut child = child;
        let status = child.wait();
        if status.is_ok() {
            crate::process_sessions::note_reaped(process);
        }
        let status =
            status.with_context(|| format!("failed to run startup command through {shell}"))?;
        // The command (the session's leader) has exited: what it left running
        // is recorded now, the only evidence those processes are dux's later.
        guard.record_survivors(process);
        let ended = Utc::now();
        // What the command wrote before it exited. A job it left running may
        // still be writing; that is the job's output, not the command's.
        let stdout = read_capture(&mut stdout)?;
        let stderr = read_capture(&mut stderr)?;
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

/// A file the startup command writes one of its output streams into: created
/// in the run's log folder and unlinked at once, so it leaves nothing behind
/// whoever holds it open and for however long.
fn capture_file(dir: &Path) -> Result<fs::File> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let name = format!(
        ".startup-output-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    );
    let path = dir.join(name);
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| format!("failed to create {}", path.display()))?;
    fs::remove_file(&path).with_context(|| format!("failed to remove {}", path.display()))?;
    Ok(file)
}

/// Everything written into a capture file so far.
fn read_capture(file: &mut fs::File) -> Result<Vec<u8>> {
    use std::io::{Read, Seek, SeekFrom};
    let written = file.metadata()?.len();
    file.seek(SeekFrom::Start(0))?;
    let mut bytes = Vec::new();
    file.take(written).read_to_end(&mut bytes)?;
    Ok(bytes)
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

    fn test_paths(root: &Path) -> DuxPaths {
        DuxPaths {
            root: root.to_path_buf(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
        }
    }

    fn test_project(root: &Path) -> Project {
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

    fn test_session(worktree: &Path) -> AgentSession {
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
        assert!(done.exists(), "the command ran to completion");
        let err = result.status.expect_err("the run reports the deletion");
        assert!(
            !err.contains("dux stopped the command"),
            "the command ran to completion, but the user is told: {err}"
        );
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
}
