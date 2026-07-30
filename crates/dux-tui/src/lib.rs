//! Dux TUI library — the terminal user-interface surface over `dux-core`.

mod app;
mod cli;
mod clipboard;
mod config;
mod config_saver;
mod diff;
// The terminal-focus grace state machine is core-owned (`dux_core::focus`),
// shared by rule with the web's viewed-ping grace. Re-exported so existing
// `crate::focus::...` paths keep resolving.
pub(crate) use dux_core::focus;
mod keybindings;
mod raw_input;
mod server_screen;
mod shimmer;
mod theme;
mod tui_color;

pub(crate) use config_saver::TuiConfigSurface;

/// Server status screen shown by the binary while serving after a TUI↔server
/// flip. Re-exported so `crates/dux/src/main.rs` can drive it as the
/// `serve_with_engine` tick.
pub use server_screen::{ServerScreenTick, ServerStatusScreen};

/// Register the fully-commented config renderer with `dux-core`, so that any
/// surface which CREATES `config.toml` writes the documented template rather
/// than a bare one. Re-exported so `crates/dux/src/main.rs` can call it on the
/// `dux server` path, which never goes through the TUI's `ensure_config`.
pub use config::install_canonical_renderer;

// Domain modules now live in dux-core. Re-export them at the crate root so
// existing `crate::<mod>::…` paths across the binary keep resolving unchanged.
pub(crate) use dux_core::{
    browser, editor, git, io_retry, lockfile, logger, model, pty, startup, statusline, storage,
};

use std::path::Path;

use anyhow::Result;

use dux_core::engine::Engine;

/// How the TUI surface exited. `Done` ends the process; `FlipToServer` hands
/// the live engine (PTYs still running, single-instance lock held inside the
/// engine) and pre-bound listeners to the binary so the web server can take
/// over the same process. The binary resumes the TUI via
/// [`resume_after_server`] when the server stops. LOCAL MODE may bind more than
/// one address (loopback + Tailscale), so `listeners`/`urls` are vectors.
pub enum TuiExit {
    Done,
    FlipToServer {
        engine: Box<Engine>,
        listeners: Vec<std::net::TcpListener>,
        urls: Vec<String>,
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
pub fn resume_after_server(mut engine: Box<Engine>) -> Result<TuiExit> {
    // Back under the TUI: `auto`/`mirror` identity resolves against the real host
    // terminal again. Already-running PTYs keep their spawn-time env until they
    // are relaunched.
    engine.surface_kind = dux_core::term_identity::SurfaceKind::Tui;
    // Capture stayed on while the server owned the host; drop whatever accumulated
    // so the resumed TUI does not replay a stale passthrough backlog to the host
    // terminal it is only now taking back.
    engine.discard_passthrough_backlog();
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
        app::RunExit::FlipToServer { listeners, urls } => {
            // Serving headless: `auto` identity now resolves to the forced
            // ghostty identity for agents launched under the server. Existing
            // PTYs keep their spawn-time env until relaunch.
            let mut engine = app.into_engine();
            engine.surface_kind = dux_core::term_identity::SurfaceKind::WebHeadless;
            // Drop any TUI-era passthrough backlog so the server does not inherit a
            // stale ring; capture continues under the server for the web bridge.
            engine.discard_passthrough_backlog();
            Ok(TuiExit::FlipToServer {
                engine: Box::new(engine),
                listeners,
                urls,
            })
        }
    }
}

pub fn print_help() {
    println!("{}", help_text());
}

/// The `dux --help` body. Split out of [`print_help`] so the text is a value the
/// tests can assert on: a `println!` straight to stdout is not checkable, which
/// is how `dux server` stayed missing from the listing while being a real
/// subcommand.
pub fn help_text() -> &'static str {
    "dux\n\n\
         Terminal UI for AI worktree sessions.\n\n\
         Usage:\n\
          dux              Launch the TUI\n\
          dux server       Serve the web UI over the headless engine\n\
          dux config       Manage the configuration file\n\n\
         Server subcommand:\n\
          dux server                     Serve on the configured host and port\n\
          dux server --bind <ADDR:PORT>  Bind this exact address instead\n\
          dux server --port <PORT>       Override the port only\n\
          dux server --no-tailscale      Skip Tailscale detection this run\n\
          There is no login: everyone who can reach the address shares this\n\
          workspace, so keep a non-loopback bind on a network you trust.\n\n\
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

#[cfg(test)]
mod tests {
    use super::*;

    /// `dux server` is a real subcommand (`crates/dux/src/main.rs` dispatches on
    /// it), so `--help` must list it next to the TUI and `config`. Without this,
    /// an installed user has no way to discover the web UI exists.
    #[test]
    fn help_lists_the_server_subcommand() {
        let help = help_text();
        assert!(
            help.contains("dux server"),
            "--help must name the `dux server` subcommand:\n{help}"
        );
    }

    /// The flags `parse_server_args` already accepts must be discoverable from
    /// the top-level help, not only from `dux server --help`.
    #[test]
    fn help_names_the_server_flags() {
        let help = help_text();
        for flag in ["--bind", "--port", "--no-tailscale"] {
            assert!(
                help.contains(flag),
                "--help must mention the `dux server` flag {flag}:\n{help}"
            );
        }
    }

    /// The trust model currently appears only deep in the docs. `--help` is the
    /// one place a user is guaranteed to look, so it must say that there is no
    /// login and that everyone who can reach the address shares the workspace.
    #[test]
    fn help_states_the_server_has_no_login() {
        let help = help_text();
        assert!(
            help.contains("no login"),
            "--help must state that the server has no login:\n{help}"
        );
        assert!(
            help.contains("shares"),
            "--help must state that reachable clients share the workspace:\n{help}"
        );
    }
}
