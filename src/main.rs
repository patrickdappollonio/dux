mod app;
mod config;
mod diff;
mod git;
mod logger;
mod model;
mod pty;
mod statusline;
mod storage;
mod theme;

use anyhow::Result;

fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();

    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return Ok(());
    }

    if args.first().map(|s| s.as_str()) == Some("reset") {
        let keep_config = args.iter().any(|arg| arg == "--keep-config");
        return run_reset(keep_config);
    }

    let mut app = app::App::bootstrap()?;
    app.run()
}

fn run_reset(keep_config: bool) -> Result<()> {
    let paths = config::DuxPaths::discover()?;

    if !paths.root.exists() {
        println!("nothing to reset: {} does not exist", paths.root.display());
        return Ok(());
    }

    // Remove all git worktrees tracked in the DB, if the DB exists.
    if paths.sessions_db_path.exists() {
        match storage::SessionStore::open(&paths.sessions_db_path) {
            Ok(store) => {
                if let Ok(sessions) = store.load_sessions() {
                    for session in &sessions {
                        let wt = std::path::Path::new(&session.worktree_path);
                        if !git::is_under(&paths.worktrees_root, wt) {
                            eprintln!(
                                "warning: skipping worktree outside of managed root: {}",
                                session.worktree_path
                            );
                            continue;
                        }
                        if wt.exists() {
                            // Best-effort: try git worktree remove, fall back to rm.
                            let _ = std::process::Command::new("git")
                                .args(["worktree", "remove", "--force"])
                                .arg(wt)
                                .output();
                            if wt.exists() {
                                let _ = std::fs::remove_dir_all(wt);
                            }
                        }
                        // Try to delete the branch in the source repo.
                        if let Some(ref project_path) = session.project_path {
                            let _ = std::process::Command::new("git")
                                .arg("-C")
                                .arg(project_path)
                                .arg("branch")
                                .arg("-D")
                                .arg(&session.branch_name)
                                .output();
                        }
                    }
                    println!("removed {} session worktree(s)", sessions.len());
                }
            }
            Err(e) => {
                eprintln!("warning: could not open session database: {e}");
            }
        }
    }

    // Remove the worktrees directory (may contain orphaned dirs).
    if paths.worktrees_root.exists() {
        std::fs::remove_dir_all(&paths.worktrees_root)?;
        println!("removed {}", paths.worktrees_root.display());
    }

    // Remove the SQLite database.
    if paths.sessions_db_path.exists() {
        std::fs::remove_file(&paths.sessions_db_path)?;
        println!("removed {}", paths.sessions_db_path.display());
    }

    // Remove log files (resolve the same way logger.rs does).
    let log_config = if paths.config_path.exists() {
        std::fs::read_to_string(&paths.config_path)
            .ok()
            .and_then(|raw| toml::from_str::<config::Config>(&raw).ok())
            .map(|cfg| cfg.logging)
            .unwrap_or_default()
    } else {
        config::LoggingConfig::default()
    };
    let log_path = logger::resolve_log_path(&log_config, &paths);
    if log_path.exists() {
        std::fs::remove_file(&log_path)?;
        println!("removed {}", log_path.display());
    }

    // Remove config unless --keep-config.
    if keep_config {
        println!("kept {}", paths.config_path.display());
    } else if paths.config_path.exists() {
        std::fs::remove_file(&paths.config_path)?;
        println!("removed {}", paths.config_path.display());
    }

    // Remove the root dir if it's now empty.
    if paths.root.exists() {
        let is_empty = paths
            .root
            .read_dir()
            .map_or(true, |mut d| d.next().is_none());
        if is_empty {
            std::fs::remove_dir(&paths.root)?;
            println!("removed {}", paths.root.display());
        }
    }

    println!("reset complete");
    Ok(())
}

fn print_help() {
    println!(
        "dux\n\n\
         Terminal UI for AI worktree sessions.\n\n\
         Usage:\n\
           dux              Launch the TUI\n\
           dux reset        Remove all sessions, worktrees, database, logs, and config\n\
           dux reset --keep-config\n\
                            Same as reset but preserve config.toml\n\n\
         First run writes a full default config to:\n\
           macOS: ~/.dux/config.toml\n\
           Linux: $XDG_CONFIG_HOME/dux/config.toml or ~/.config/dux/config.toml\n\
         Session state is stored in:\n\
           macOS: ~/.dux/sessions.sqlite3\n\
           Linux: $XDG_CONFIG_HOME/dux/sessions.sqlite3 or ~/.config/dux/sessions.sqlite3"
    );
}
