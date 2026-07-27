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
/// The pre-load is what lets a picker render the newest run's output in the
/// same frame it opens, with no second round of file I/O and no "loading"
/// placeholder for the row it starts on. `content` is empty exactly when
/// `entries` is, which is also the signal that the scope has never run its
/// startup command.
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
/// The one place the scope is matched. Callers that need a scope's runs ask
/// here rather than re-deriving "agent means this directory, project means
/// every session directory under it", which is how the two spellings drifted
/// apart before.
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
    Command::new(opener)
        .arg(path)
        .spawn()
        .with_context(|| format!("failed to run {opener} {}", path.display()))?;
    Ok(())
}

pub fn run_startup_command(paths: &DuxPaths, run: StartupCommandRun) -> StartupCommandResult {
    let log_dir = agent_log_dir(paths, &run.project.id, &run.session.id);
    let timestamp = Utc::now();
    let file_stamp = timestamp.format("%Y%m%dT%H%M%SZ");
    let safe_branch = sanitize_file_component(&run.session.branch_name);
    let log_path = log_dir.join(format!("{file_stamp}-{safe_branch}.log"));
    let result = (|| -> Result<CommandOutcome> {
        fs::create_dir_all(&log_dir)
            .with_context(|| format!("failed to create {}", log_dir.display()))?;
        let shell = startup_shell_command(&run.terminal.command);
        let shell_args = run.terminal.args.clone();
        let started = Utc::now();
        let started_instant = Instant::now();
        let mut command = Command::new(&shell);
        command
            .args(&shell_args)
            .arg(&run.command)
            .current_dir(&run.session.worktree_path)
            .env("DUX_PROJECT_PATH", &run.project.path)
            .env("DUX_WORKTREE_PATH", &run.session.worktree_path)
            .env("DUX_AGENT_ID", &run.session.id)
            .env("DUX_AGENT_BRANCH", &run.session.branch_name)
            .env("DUX_PROVIDER", run.session.provider.as_str())
            .env("DUX_STARTUP_COMMAND_LOG", &log_path);
        for (name, value) in &run.env {
            command.env(name, value);
        }
        let output = command
            .output()
            .with_context(|| format!("failed to run startup command through {shell}"))?;
        let ended = Utc::now();
        Ok(CommandOutcome {
            shell,
            shell_args,
            started,
            ended,
            duration_ms: started_instant.elapsed().as_millis(),
            code: output.status.code(),
            success: output.status.success(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        })
    })();

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
            let _ = fs::create_dir_all(&log_dir);
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
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let mut body = String::new();
    body.push_str("dux startup command log\n");
    body.push_str(&format!("started_at = {}\n", outcome.started.to_rfc3339()));
    body.push_str(&format!("ended_at = {}\n", outcome.ended.to_rfc3339()));
    body.push_str(&format!("duration_ms = {}\n", outcome.duration_ms));
    body.push_str(&format!("project_id = {}\n", run.project.id));
    body.push_str(&format!("project_name = {}\n", run.project.name));
    body.push_str(&format!("project_path = {}\n", run.project.path));
    body.push_str(&format!("agent_id = {}\n", run.session.id));
    body.push_str(&format!("agent_branch = {}\n", run.session.branch_name));
    body.push_str(&format!("worktree_path = {}\n", run.session.worktree_path));
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
            project_id: "project-1".to_string(),
            project_path: Some(worktree.to_string_lossy().to_string()),
            provider: ProviderKind::from_str("codex"),
            source_branch: "main".to_string(),
            branch_name: "feature/setup".to_string(),
            initial_branch: "feature/setup".to_string(),
            worktree_path: worktree.to_string_lossy().to_string(),
            title: None,
            started_providers: Vec::new(),
            desired_running: true,
            auto_reopen_enabled: true,
            status: SessionStatus::Active,
            created_at: now,
            updated_at: now,
            last_focused_tab: None,
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
                session,
                command: "printf hello".to_string(),
                terminal: StartupCommandTerminalConfig {
                    command: "/bin/sh".to_string(),
                    args: vec!["-c".to_string()],
                },
                env: Vec::new(),
            },
        );

        assert!(result.status.is_ok());
        let log = read_log(&result.log_path).expect("log");
        assert!(log.contains("success = true"));
        assert!(log.contains("command = printf hello"));
        assert!(log.contains("--- stdout ---\nhello"));
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
                session,
                command: "printf nope >&2; exit 7".to_string(),
                terminal: StartupCommandTerminalConfig {
                    command: "/bin/sh".to_string(),
                    args: vec!["-c".to_string()],
                },
                env: Vec::new(),
            },
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
        );

        assert!(result.status.is_ok());
        let log = read_log(&result.log_path).expect("log");
        assert!(log.contains("--- stdout ---\ntrue:secret"));
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
