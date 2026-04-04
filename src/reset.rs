use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow, bail};

use crate::config::{self, DuxPaths};
use crate::git;
use crate::logger;
use crate::storage::SessionStore;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResetOptions {
    pub delete_agent_data: bool,
}

impl ResetOptions {
    pub fn from_args(args: &[String]) -> Result<Self> {
        let mut options = Self::default();
        for arg in args {
            match arg.as_str() {
                "--delete-agent-data" => options.delete_agent_data = true,
                "--keep-config" => {
                    bail!(
                        "--keep-config has been removed; `dux reset` now always resets the config"
                    );
                }
                other => bail!("unknown reset flag: {other}"),
            }
        }
        Ok(options)
    }
}

pub fn run(paths: &DuxPaths, options: ResetOptions) -> Result<()> {
    if !paths.root.exists() {
        println!("nothing to reset: {} does not exist", paths.root.display());
        return Ok(());
    }

    let log_path = resolve_reset_log_path(paths);

    if options.delete_agent_data {
        reset_agent_data(paths)?;
    }

    remove_file_with_message(&log_path)?;
    prune_empty_ancestors(&log_path, &paths.root)?;
    remove_file_with_message(&paths.config_path)?;
    prune_empty_ancestors(&paths.config_path, &paths.root)?;
    remove_root_if_empty_with_message(&paths.root)?;

    println!("reset complete");
    Ok(())
}

fn reset_agent_data(paths: &DuxPaths) -> Result<()> {
    if paths.sessions_db_path.exists() {
        match SessionStore::open(&paths.sessions_db_path) {
            Ok(store) => match store.load_sessions() {
                Ok(sessions) => {
                    for session in &sessions {
                        remove_session_worktree(paths, session);
                    }
                    println!("removed {} session worktree(s)", sessions.len());
                }
                Err(error) => {
                    eprintln!("warning: could not load sessions from database: {error}");
                }
            },
            Err(error) => {
                eprintln!("warning: could not open session database: {error}");
            }
        }
    }

    remove_dir_with_message(&paths.worktrees_root)?;
    remove_file_with_message(&paths.sessions_db_path)?;
    Ok(())
}

fn remove_session_worktree(paths: &DuxPaths, session: &crate::model::AgentSession) {
    let worktree = Path::new(&session.worktree_path);
    if !git::is_under(&paths.worktrees_root, worktree) {
        eprintln!(
            "warning: skipping worktree outside of managed root: {}",
            session.worktree_path
        );
        return;
    }

    if worktree.exists() {
        // Best-effort: try git worktree remove, fall back to rm.
        let _ = std::process::Command::new("git")
            .args(["worktree", "remove", "--force"])
            .arg(worktree)
            .output();
        if worktree.exists() {
            let _ = fs::remove_dir_all(worktree);
        }
    }

    if let Some(project_path) = session.project_path.as_deref() {
        let _ = std::process::Command::new("git")
            .arg("-C")
            .arg(project_path)
            .arg("branch")
            .arg("-D")
            .arg(&session.branch_name)
            .output();
    }
}

fn resolve_reset_log_path(paths: &DuxPaths) -> PathBuf {
    let logging = if paths.config_path.exists() {
        fs::read_to_string(&paths.config_path)
            .ok()
            .and_then(|raw| toml::from_str::<config::Config>(&raw).ok())
            .map(|config| config.logging)
            .unwrap_or_default()
    } else {
        config::LoggingConfig::default()
    };
    logger::resolve_log_path(&logging, paths)
}

fn remove_file_with_message(path: &Path) -> Result<()> {
    if remove_file_if_present(path)? {
        println!("removed {}", path.display());
    }
    Ok(())
}

fn remove_dir_with_message(path: &Path) -> Result<()> {
    if remove_dir_if_present(path)? {
        println!("removed {}", path.display());
    }
    Ok(())
}

fn remove_root_if_empty_with_message(path: &Path) -> Result<()> {
    if remove_dir_if_empty(path)? {
        println!("removed {}", path.display());
    }
    Ok(())
}

fn remove_file_if_present(path: &Path) -> Result<bool> {
    match fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(anyhow!("failed to remove {}: {error}", path.display())),
    }
}

fn remove_dir_if_present(path: &Path) -> Result<bool> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(anyhow!("failed to remove {}: {error}", path.display())),
    }
}

fn remove_dir_if_empty(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let mut entries = fs::read_dir(path)
        .map_err(|error| anyhow!("failed to inspect {}: {error}", path.display()))?;
    if entries.next().is_some() {
        return Ok(false);
    }
    fs::remove_dir(path)
        .map_err(|error| anyhow!("failed to remove {}: {error}", path.display()))?;
    Ok(true)
}

fn prune_empty_ancestors(path: &Path, root: &Path) -> Result<()> {
    let Ok(relative) = path.strip_prefix(root) else {
        return Ok(());
    };
    if relative.as_os_str().is_empty() {
        return Ok(());
    }

    let mut current = path.parent();
    while let Some(dir) = current {
        if dir == root {
            break;
        }
        if !remove_dir_if_empty(dir)? {
            break;
        }
        current = dir.parent();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use chrono::Utc;
    use tempfile::TempDir;

    use super::*;
    use crate::config::{self, Config};
    use crate::keybindings::RuntimeBindings;
    use crate::model::{AgentSession, ProviderKind, SessionStatus};

    #[test]
    fn reset_rejects_removed_keep_config_flag() {
        let error = ResetOptions::from_args(&["--keep-config".to_string()]).unwrap_err();
        assert!(error.to_string().contains("--keep-config has been removed"));
    }

    #[test]
    fn reset_rejects_unknown_flags() {
        let error = ResetOptions::from_args(&["--wat".to_string()]).unwrap_err();
        assert!(error.to_string().contains("unknown reset flag"));
    }

    #[test]
    fn default_reset_removes_config_and_logs_but_keeps_agent_data() {
        let harness = ResetHarness::new();
        harness.write_config_with_log_path("logs/custom.log");
        harness.write_log("logs/custom.log");
        let worktree = harness.create_session("agent-1");

        run(&harness.paths, ResetOptions::default()).expect("reset");

        assert!(!harness.paths.config_path.exists());
        assert!(!harness.paths.root.join("logs/custom.log").exists());
        assert!(harness.paths.sessions_db_path.exists());
        assert!(worktree.exists());

        let _config = config::ensure_config(&harness.paths).expect("config recreated");
        let store = SessionStore::open(&harness.paths.sessions_db_path).expect("store");
        let sessions = store.load_sessions().expect("sessions");
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].worktree_path, worktree.to_string_lossy());
    }

    #[test]
    fn delete_agent_data_flag_wipes_database_and_worktrees() {
        let harness = ResetHarness::new();
        harness.write_config_with_log_path("logs/custom.log");
        harness.write_log("logs/custom.log");
        harness.create_session("agent-1");

        run(
            &harness.paths,
            ResetOptions {
                delete_agent_data: true,
            },
        )
        .expect("reset");

        assert!(!harness.paths.root.exists());
    }

    #[test]
    fn reset_succeeds_when_paths_are_already_missing() {
        let harness = ResetHarness::new();
        fs::create_dir_all(&harness.paths.root).expect("root");

        run(&harness.paths, ResetOptions::default()).expect("reset");

        assert!(!harness.paths.root.exists());
    }

    #[test]
    fn delete_agent_data_removes_worktrees_without_database() {
        let harness = ResetHarness::new();
        fs::create_dir_all(harness.paths.worktrees_root.join("orphan")).expect("orphan worktree");

        run(
            &harness.paths,
            ResetOptions {
                delete_agent_data: true,
            },
        )
        .expect("reset");

        assert!(!harness.paths.root.exists());
    }

    struct ResetHarness {
        _tempdir: TempDir,
        paths: DuxPaths,
    }

    impl ResetHarness {
        fn new() -> Self {
            let tempdir = TempDir::new().expect("tempdir");
            let root = tempdir.path().join("dux");
            fs::create_dir_all(&root).expect("root");
            let paths = DuxPaths {
                config_path: root.join("config.toml"),
                sessions_db_path: root.join("sessions.sqlite3"),
                worktrees_root: root.join("worktrees"),
                root,
            };
            Self {
                _tempdir: tempdir,
                paths,
            }
        }

        fn write_config_with_log_path(&self, log_path: &str) {
            let mut config = Config::default();
            config.logging.path = log_path.to_string();
            let bindings = RuntimeBindings::from_keys_config(&config.keys);
            config::save_config(&self.paths.config_path, &config, &bindings).expect("config");
        }

        fn write_log(&self, relative_path: &str) {
            let path = self.paths.root.join(relative_path);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("log dir");
            }
            fs::write(path, "log").expect("log");
        }

        fn create_session(&self, id: &str) -> PathBuf {
            fs::create_dir_all(&self.paths.worktrees_root).expect("worktrees root");
            let worktree = self.paths.worktrees_root.join(id);
            fs::create_dir_all(&worktree).expect("worktree");

            let store = SessionStore::open(&self.paths.sessions_db_path).expect("store");
            let now = Utc::now();
            store
                .upsert_session(&AgentSession {
                    id: id.to_string(),
                    project_id: "proj".to_string(),
                    project_path: None,
                    provider: ProviderKind::new("claude"),
                    source_branch: "main".to_string(),
                    branch_name: format!("branch-{id}"),
                    worktree_path: worktree.to_string_lossy().to_string(),
                    title: None,
                    status: SessionStatus::Active,
                    created_at: now,
                    updated_at: now,
                })
                .expect("session");
            worktree
        }
    }
}
