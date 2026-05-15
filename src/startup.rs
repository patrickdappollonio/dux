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

#[derive(Clone, Debug)]
pub struct StartupCommandLatestLog {
    pub path: Option<PathBuf>,
    pub display_name: String,
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

pub fn latest_log_for_scope(
    paths: &DuxPaths,
    scope: StartupCommandLogScope,
) -> Result<StartupCommandLatestLog> {
    let entries = match scope {
        StartupCommandLogScope::Agent {
            project_id,
            session_id,
        } => list_agent_logs(paths, &project_id, &session_id)?,
        StartupCommandLogScope::Project { project_id } => list_project_logs(paths, &project_id)?,
    };

    match entries.first() {
        Some(entry) => Ok(StartupCommandLatestLog {
            path: Some(entry.path.clone()),
            display_name: entry.display_name.clone(),
            content: read_log(&entry.path)?,
        }),
        None => Ok(StartupCommandLatestLog {
            path: None,
            display_name: "No startup command log".to_string(),
            content: "No startup command logs found.".to_string(),
        }),
    }
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
        let output = Command::new(&shell)
            .args(&shell_args)
            .arg(&run.command)
            .current_dir(&run.session.worktree_path)
            .env("DUX_PROJECT_PATH", &run.project.path)
            .env("DUX_WORKTREE_PATH", &run.session.worktree_path)
            .env("DUX_AGENT_ID", &run.session.id)
            .env("DUX_AGENT_BRANCH", &run.session.branch_name)
            .env("DUX_PROVIDER", run.session.provider.as_str())
            .env("DUX_STARTUP_COMMAND_LOG", &log_path)
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
            current_branch: "main".to_string(),
            branch_status: ProjectBranchStatus::Leading,
            path_missing: false,
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
            worktree_path: worktree.to_string_lossy().to_string(),
            title: None,
            started_providers: Vec::new(),
            desired_running: true,
            auto_reopen_enabled: true,
            status: SessionStatus::Active,
            created_at: now,
            updated_at: now,
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
    fn delete_agent_logs_removes_session_directory() {
        let tmp = tempdir().expect("tempdir");
        let paths = test_paths(tmp.path());
        let dir = agent_log_dir(&paths, "project-1", "session-1");
        fs::create_dir_all(&dir).expect("log dir");
        fs::write(dir.join("one.log"), "log").expect("log file");

        delete_agent_logs(&paths, "project-1", "session-1").expect("delete logs");

        assert!(!dir.exists());
    }

    #[test]
    fn latest_log_for_scope_reads_newest_project_log() {
        let tmp = tempdir().expect("tempdir");
        let paths = test_paths(tmp.path());
        let old_dir = agent_log_dir(&paths, "project-1", "session-1");
        let new_dir = agent_log_dir(&paths, "project-1", "session-2");
        fs::create_dir_all(&old_dir).expect("old log dir");
        fs::create_dir_all(&new_dir).expect("new log dir");
        fs::write(old_dir.join("20260101T000000Z-old.log"), "old").expect("old log");
        let newest = new_dir.join("20260101T000001Z-new.log");
        fs::write(&newest, "new").expect("new log");

        let latest = latest_log_for_scope(
            &paths,
            StartupCommandLogScope::Project {
                project_id: "project-1".to_string(),
            },
        )
        .expect("latest log");

        assert_eq!(latest.path, Some(newest));
        assert_eq!(latest.display_name, "20260101T000001Z-new.log");
        assert_eq!(latest.content, "new");
    }

    #[test]
    fn latest_log_for_scope_reports_empty_state() {
        let tmp = tempdir().expect("tempdir");
        let paths = test_paths(tmp.path());

        let latest = latest_log_for_scope(
            &paths,
            StartupCommandLogScope::Agent {
                project_id: "project-1".to_string(),
                session_id: "session-1".to_string(),
            },
        )
        .expect("latest log");

        assert!(latest.path.is_none());
        assert_eq!(latest.display_name, "No startup command log");
        assert_eq!(latest.content, "No startup command logs found.");
    }
}
