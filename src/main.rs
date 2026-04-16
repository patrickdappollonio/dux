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

use anyhow::Result;

fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();

    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return Ok(());
    }

    // Resolve the config directory and acquire the single-instance lock
    // before touching any shared state. Only the root directory is created
    // here (so the lockfile can be opened); remaining directories
    // (worktrees, etc.) are created by downstream code after the lock is
    // held. Every dux entrypoint — TUI and all `dux config` subcommands —
    // goes through this gate so exactly one process operates on a given
    // config directory at a time.
    let paths = config::DuxPaths::discover()?;
    std::fs::create_dir_all(&paths.root)?;
    let lock = match lockfile::SingleInstanceLock::acquire(&paths.lock_path) {
        Ok(lock) => lock,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    };

    if args.first().map(|s| s.as_str()) == Some("config") {
        return cli::run(&args[1..], &paths);
    }

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
