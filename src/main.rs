mod app;
mod cli;
mod clipboard;
mod config;
mod diff;
mod editor;
mod git;
mod io_retry;
mod keybindings;
mod lockfile;
mod logger;
mod model;
mod provider;
mod pty;
mod raw_input;
mod statusline;
mod storage;
mod theme;

use std::path::Path;

use anyhow::Result;

fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();

    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return Ok(());
    }

    let paths = config::DuxPaths::discover()?;

    if args.first().map(|s| s.as_str()) == Some("config") {
        let config_args = &args[1..];
        let sub = config_args.first().map(|s| s.as_str()).unwrap_or("");

        // Acquire the single-instance lock only for subcommands that
        // mutate shared on-disk state. Read-only operations (path, diff,
        // regenerate preview) skip the lock entirely.
        let _lock = match sub {
            // reset mutates state when root exists. When root is absent,
            // run_reset's fast-path reports "nothing to reset" and exits,
            // so we avoid creating the directory just to take a lock.
            "reset" if paths.root.exists() => Some(acquire_lock_or_exit(&paths.lock_path)),

            // regenerate --yes creates directories and writes config.
            // Create root (so the lockfile can be opened) and lock before
            // any writes, preventing a concurrent TUI from starting
            // between directory creation and the config write.
            "regenerate" if config_args.iter().any(|a| a == "--yes") => {
                std::fs::create_dir_all(&paths.root)?;
                Some(acquire_lock_or_exit(&paths.lock_path))
            }

            // Everything else is read-only or prints help — no shared
            // state to protect. This includes: path, diff, diff --raw,
            // regenerate (preview without --yes), --help, and empty.
            _ => None,
        };

        return cli::run(config_args, &paths);
    }

    // TUI: always create the root directory (so the lockfile can be
    // opened), acquire the lock, then let bootstrap create everything
    // else. A losing process never touches shared state beyond the
    // empty root.
    std::fs::create_dir_all(&paths.root)?;
    let lock = acquire_lock_or_exit(&paths.lock_path);
    let mut app = app::App::bootstrap_with_lock(paths, lock)?;
    app.run()
}

fn print_help() {
    println!(
        "dux\n\n\
         Terminal UI for AI worktree sessions.\n\n\
         Usage:\n\
          dux              Launch the TUI\n\
          dux config       Manage the configuration file\n\n\
         Config subcommands:\n\
          dux config path          Print the config file path\n\
          dux config diff          Show settings that differ from defaults\n\
          dux config diff --raw    Show a unified diff against the default config\n\
          dux config reset         Remove config and logs (keeps agents and worktrees)\n\
          dux config reset --all   Full factory reset (config, logs, sessions, worktrees)\n\
          dux config regenerate    Preview a fresh default config (shows diff)\n\
          dux config regenerate --yes\n\
                                   Overwrite the config file with fresh defaults\n\n\
         Environment variables:\n\
           DUX_HOME    Override the config directory (must be an absolute path).\n\
                       When unset, defaults to:\n\
                         macOS: ~/.dux/\n\
                         Linux: $XDG_CONFIG_HOME/dux/ or ~/.config/dux/\n\n\
         First run writes a full default config to:\n\
           macOS: ~/.dux/config.toml\n\
           Linux: $XDG_CONFIG_HOME/dux/config.toml or ~/.config/dux/config.toml\n\
         Session state is stored in:\n\
           macOS: ~/.dux/sessions.sqlite3\n\
           Linux: $XDG_CONFIG_HOME/dux/sessions.sqlite3 or ~/.config/dux/sessions.sqlite3"
    );
}

fn acquire_lock_or_exit(path: &Path) -> lockfile::SingleInstanceLock {
    match lockfile::SingleInstanceLock::acquire(path) {
        Ok(lock) => lock,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    }
}
