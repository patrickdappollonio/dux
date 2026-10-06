//! Dux TUI library: the terminal user-interface surface over `dux-core`.

mod app;
mod cli;
mod clipboard;
mod config;
mod config_cli;
mod config_saver;
mod diff;
// The terminal-focus grace state machine is core-owned (`dux_core::focus`),
// shared by rule with the web's viewed-ping grace.
pub(crate) use dux_core::focus;
mod key_encode;
mod keybindings;
mod raw_input;
mod server_screen;
mod theme;
mod tui_color;

pub(crate) use config_saver::TuiConfigSurface;

/// Server status screen shown by the binary while serving after a TUI↔server
/// flip. Re-exported so `crates/dux/src/main.rs` can drive it as the
/// `serve_with_engine` tick.
pub use server_screen::{ServerScreenTick, ServerStatusScreen, restore_terminal};

/// Register the fully-commented config renderer with `dux-core`, so that any
/// surface which CREATES `config.toml` writes the documented template rather
/// than a bare one. Re-exported so `crates/dux/src/main.rs` can call it on the
/// `dux server` path, which never goes through the TUI's `ensure_config`.
pub use config::install_canonical_renderer;

// Domain modules now live in dux-core. Re-export them at the crate root so
// existing `crate::<mod>::…` paths across the binary keep resolving unchanged.
pub(crate) use dux_core::{
    editor, git, io_retry, lockfile, logger, model, pty, startup, statusline, storage,
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
        /// What the pre-flight learned (its warnings, the Tailscale bind
        /// failures, whether an address was detected), so the server's log opens
        /// with the same lines `dux server` prints.
        startup: dux_core::serve_log::StartupNotes,
    },
}

/// Run `dux config <args>`: the arguments after the word `config`, exactly as
/// the user typed them. The binary's command tree hands them over untouched, so
/// every message and exit code here is the config code's own.
pub fn run_config(config_args: &[String]) -> Result<()> {
    let paths = config::DuxPaths::discover()?;
    // Help anywhere in a config command (before a literal `--`) prints the
    // config help, before any lock is taken.
    if cli::asks_for_help(config_args) {
        return cli::run(&["--help".to_string()], &paths);
    }
    let _lock = lock_for_config_subcommand(config_args, &paths)?;

    cli::run(config_args, &paths)
}

/// Run the terminal UI. Called by the `dux` binary crate when no command was
/// given.
///
/// `companion` is the background web server this TUI may serve through. It is a
/// `dux-core` trait object because this crate never sees `dux-web`: the binary
/// implements it and is the only place the two surfaces meet.
pub fn run(
    companion: Box<dyn dux_core::background_serve::BackgroundServeCompanion>,
) -> Result<TuiExit> {
    let paths = config::DuxPaths::discover()?;

    // The SIGUSR1 (reload config) handler goes in BEFORE the lock: `dux
    // config set` signals whoever holds the lock, and the signal's default
    // action would end this process. Idempotent, so the binary having
    // installed it already is fine.
    if let Err(err) = dux_core::reload_signal::install() {
        eprintln!("warning: {err}; `dux config set` cannot reach this dux, so reload by hand");
    }

    // TUI: always create the root directory (so the lockfile can be
    // opened), acquire the lock, then let bootstrap create everything
    // else. A losing process never touches shared state beyond the
    // empty root.
    create_private_root(&paths)?;
    let lock = acquire_lock_or_exit(&paths.lock_path);
    let app = app::App::bootstrap_with_lock(paths, lock)?;
    run_app(app, companion)
}

/// Resume the TUI after the web server hands the engine back. The engine still
/// owns the live providers and the single-instance lock, so this rebuilds the
/// App view state around it (no session relaunch) and runs the loop. A resumed
/// TUI can flip to the server again, so the flip↔serve cycle repeats.
pub fn resume_after_server(
    mut engine: Box<Engine>,
    companion: Box<dyn dux_core::background_serve::BackgroundServeCompanion>,
) -> Result<TuiExit> {
    // Back under the TUI: `auto`/`mirror` identity resolves against the real host
    // terminal again. Already-running PTYs keep their spawn-time env until they
    // are relaunched.
    engine.surface_kind = dux_core::term_identity::SurfaceKind::Tui;
    // Capture stayed on while the server owned the host; drop whatever accumulated
    // so the resumed TUI does not replay a stale passthrough backlog to the host
    // terminal it is only now taking back.
    engine.discard_passthrough_backlog();
    let app = app::App::resume(*engine)?;
    run_app(app, companion)
}

/// Run an App's event loop and translate its [`app::RunExit`] into a
/// [`TuiExit`] for the binary's orchestration loop. On a flip, the engine is
/// moved out of the App (no `Drop` runs on the providers, since neither `App`
/// nor `Engine` has a `Drop` impl, so this is a plain move) and boxed for the
/// caller; the single-instance lock rides along inside the engine.
fn run_app(
    mut app: app::App,
    companion: Box<dyn dux_core::background_serve::BackgroundServeCompanion>,
) -> Result<TuiExit> {
    // Installed here, before the loop, so `App::run` can honor
    // `[server] serve_while_tui` on its very first iteration.
    app.companion = Some(companion);
    match app.run()? {
        app::RunExit::Quit => Ok(TuiExit::Done),
        app::RunExit::FlipToServer {
            listeners,
            urls,
            startup,
        } => {
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
                startup,
            })
        }
    }
}

/// Takes the single-instance lock for the `config` subcommands that mutate
/// shared on-disk state, creating the config folder owner-only first so the
/// lock file never sits in a folder other users can read.
fn lock_for_config_subcommand(
    config_args: &[String],
    paths: &config::DuxPaths,
) -> Result<Option<lockfile::SingleInstanceLock>> {
    let sub = config_args.first().map(|s| s.as_str()).unwrap_or("");
    // Acquire the single-instance lock only for subcommands that
    // mutate shared on-disk state. Read-only operations (path, diff,
    // regenerate preview) skip the lock entirely.
    Ok(match sub {
        // reset mutates state when root exists. When root is absent,
        // run_reset's fast-path reports "nothing to reset" and exits,
        // so we avoid creating the directory just to take a lock.
        "reset" if paths.root.exists() => {
            create_private_root(paths)?;
            Some(acquire_lock_or_exit(&paths.lock_path))
        }

        // regenerate --yes creates directories and writes config.
        // Create root (so the lockfile can be opened) and lock before
        // any writes, preventing a concurrent TUI from starting
        // between directory creation and the config write.
        "regenerate" if config_args.iter().any(|a| a == "--yes") => {
            create_private_root(paths)?;
            Some(acquire_lock_or_exit(&paths.lock_path))
        }

        // `set` deliberately runs beside a live dux: its write takes the
        // config file's own lock (never this one, which the running dux
        // holds for its whole life), and it then signals that dux to
        // reload. Everything else is read-only or prints help, so there
        // is no shared state to protect.
        _ => None,
    })
}

/// Creates the config folder owner-only. Every path that is about to open the
/// lock file calls this first, so the lock never sits in a folder other users
/// can read.
fn create_private_root(paths: &config::DuxPaths) -> Result<()> {
    dux_core::file_modes::create_private_dir_all(&paths.root)?;
    Ok(())
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

    #[test]
    fn config_reset_tightens_an_existing_loose_folder_and_a_missing_one_is_left_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("restored-home");
        std::fs::create_dir(&root).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
        let paths = config::DuxPaths {
            root: root.clone(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            socket_path: root.join("dux.sock"),
        };

        let lock = lock_for_config_subcommand(&["reset".to_string()], &paths).unwrap();
        drop(lock);
        let mode = std::fs::metadata(&root).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);

        let missing = dir.path().join("never-created");
        let absent = config::DuxPaths {
            root: missing.clone(),
            lock_path: missing.join("dux.lock"),
            socket_path: missing.join("dux.sock"),
            ..paths
        };
        assert!(
            lock_for_config_subcommand(&["reset".to_string()], &absent)
                .unwrap()
                .is_none()
        );
        assert!(!missing.exists());
    }

    #[test]
    fn the_config_folder_is_created_owner_only_before_the_lock_file_exists() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("fresh-home");
        let paths = config::DuxPaths {
            root: root.clone(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            socket_path: root.join("dux.sock"),
        };

        create_private_root(&paths).unwrap();

        let mode = std::fs::metadata(&root).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        assert!(!paths.lock_path.exists());
    }
}
