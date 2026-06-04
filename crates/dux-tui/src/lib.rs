//! Dux TUI library — the terminal user-interface surface over `dux-core`.

mod app;
mod cli;
mod clipboard;
mod config;
mod config_saver;
mod diff;
mod keybindings;
mod raw_input;
mod server_screen;
mod theme;
mod tui_color;

pub(crate) use config_saver::TuiConfigSaver;

/// Server status screen shown by the binary while serving after a TUI↔server
/// flip. Re-exported so `crates/dux/src/main.rs` can drive it as the
/// `serve_with_engine` tick.
pub use server_screen::{ServerScreenTick, ServerStatusScreen};

// Domain modules now live in dux-core. Re-export them at the crate root so
// existing `crate::<mod>::…` paths across the binary keep resolving unchanged.
pub(crate) use dux_core::{
    browser, editor, git, io_retry, lockfile, logger, model, provider, pty, startup, statusline,
    storage,
};

use std::path::Path;

use anyhow::Result;

use dux_core::engine::Engine;

/// How the TUI surface exited. `Done` ends the process; `FlipToServer` hands
/// the live engine (PTYs still running, single-instance lock held inside the
/// engine) and a pre-bound listener to the binary so the web server can take
/// over the same process. The binary resumes the TUI via
/// [`resume_after_server`] when the server stops.
pub enum TuiExit {
    Done,
    FlipToServer {
        engine: Box<Engine>,
        listener: std::net::TcpListener,
        url: String,
    },
}

/// Run dux (TUI mode or a `config` subcommand). Called by the `dux` binary
/// crate.
pub fn run() -> Result<TuiExit> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();

    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return Ok(TuiExit::Done);
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

        cli::run(config_args, &paths)?;
        return Ok(TuiExit::Done);
    }

    // TUI: always create the root directory (so the lockfile can be
    // opened), acquire the lock, then let bootstrap create everything
    // else. A losing process never touches shared state beyond the
    // empty root.
    std::fs::create_dir_all(&paths.root)?;
    let lock = acquire_lock_or_exit(&paths.lock_path);
    let app = app::App::bootstrap_with_lock(paths, lock)?;
    run_app(app)
}

/// Resume the TUI after the web server hands the engine back. The engine still
/// owns the live providers and the single-instance lock, so this rebuilds the
/// App view state around it (no session relaunch) and runs the loop. A resumed
/// TUI can flip to the server again, so the flip↔serve cycle repeats.
pub fn resume_after_server(engine: Box<Engine>) -> Result<TuiExit> {
    let app = app::App::resume(*engine)?;
    run_app(app)
}

/// Run an App's event loop and translate its [`app::RunExit`] into a
/// [`TuiExit`] for the binary's orchestration loop. On a flip, the engine is
/// moved out of the App (no `Drop` runs on the providers — neither `App` nor
/// `Engine` has a `Drop` impl, so this is a plain move) and boxed for the
/// caller; the single-instance lock rides along inside the engine.
fn run_app(mut app: app::App) -> Result<TuiExit> {
    match app.run()? {
        app::RunExit::Quit => Ok(TuiExit::Done),
        app::RunExit::FlipToServer { listener, url } => Ok(TuiExit::FlipToServer {
            engine: Box::new(app.into_engine()),
            listener,
            url,
        }),
    }
}

pub fn print_help() {
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
