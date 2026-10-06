mod client_commands;
mod commands;
mod companion;

use anyhow::Result;
use clap::Parser;

fn main() -> Result<()> {
    // First of all: until the handler is installed, a hand reload's SIGUSR1 would end this
    // dux. A failure is reported where each serving mode starts, which installs it again.
    let _ = dux_core::reload_signal::install();
    // `dux config` owns every word after it, `--` and flag-looking words
    // included, so it is recognised before clap sees the line.
    let raw: Vec<String> = std::env::args().skip(1).collect();
    if let Some(config) = commands::split_config(&raw) {
        let selection = client_commands::Selection {
            remote: config.remote,
            local: config.local,
        };
        let remote = dux_core::config::DuxPaths::discover()
            .map_err(|error| {
                dux_core::client::CliError::new(
                    dux_core::client::Exit::Failed,
                    format!("{error:#}"),
                )
            })
            .and_then(|paths| selection.remote_name(&paths));
        let remote = match remote {
            Ok(remote) => remote,
            Err(error) => client_commands::finish(Err(error)),
        };
        if let Err(message) = commands::check_config_target(remote.as_deref(), selection.local) {
            usage_error(&message);
        }
        // A config command that ends with one of the command line's own codes
        // (a reload whose outcome is unknown) exits with it.
        if let Err(error) = dux_tui::run_config(&config.args) {
            return match error.downcast::<dux_core::client::CliError>() {
                Ok(cli) => client_commands::finish(Err(cli)),
                Err(error) => Err(error),
            };
        }
        return Ok(());
    }
    let cli = commands::Cli::parse();
    let selection = client_commands::Selection {
        remote: cli.remote.clone(),
        local: cli.local,
    };
    match cli.command {
        None => run_tui_with_flip(),
        Some(commands::Command::Server(server)) => {
            if let Err(message) = commands::listener_flags_with_subcommand(&server) {
                usage_error(&message);
            }
            if server.command.is_none()
                && let Err(message) =
                    commands::check_server_start_target(cli.remote.as_deref(), cli.local)
            {
                usage_error(&message);
            }
            match server.command {
                None => run_server(server.into_overrides()),
                Some(commands::ServerSub::Logs { follow, lines }) => {
                    client_commands::finish(client_commands::server_logs(follow, lines, &selection))
                }
                Some(commands::ServerSub::Connections { command }) => client_commands::finish(
                    client_commands::server_connections(command, &selection),
                ),
            }
        }
        // Every well-formed `config` line was split off above; clap only
        // reaches here for one that names both global flags, which it refuses.
        Some(commands::Command::Config(_)) => {
            usage_error("dux config takes --local or --remote before the word config, not both")
        }
        Some(commands::Command::Remote(remote)) => {
            client_commands::finish(client_commands::remote(remote.command, &selection))
        }
        Some(commands::Command::Operations(operations)) => client_commands::finish_with_code(
            client_commands::operations(operations.command, &selection),
        ),
        Some(commands::Command::Macros(macros)) => {
            client_commands::finish(client_commands::macros(macros.command, &selection))
        }
        Some(commands::Command::Providers(providers)) => {
            client_commands::finish(client_commands::providers(providers.command, &selection))
        }
        Some(commands::Command::Keys(keys)) => {
            client_commands::finish(client_commands::keys(keys.command, &selection))
        }
        Some(commands::Command::Themes(themes)) => {
            client_commands::finish(client_commands::themes(themes.command, &selection))
        }
        Some(commands::Command::Env(env)) => {
            client_commands::finish(client_commands::env(env.command, &selection))
        }
        Some(commands::Command::Projects(projects)) => {
            client_commands::finish(client_commands::projects(projects.command, &selection))
        }
        Some(commands::Command::Agents(agents)) => {
            client_commands::finish(client_commands::agents(agents.command, &selection))
        }
        Some(commands::Command::Terminals(terminals)) => {
            client_commands::finish(client_commands::terminals(terminals.command, &selection))
        }
    }
}

/// A refusal found while checking the command line, before anything starts.
fn usage_error(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(2);
}

/// Default arm: run the TUI, and when it flips to the web server, serve the same
/// engine in this process until the server stops, then resume the TUI, repeating
/// until the user quits from either surface. While serving, the terminal shows
/// [`dux_tui::ServerStatusScreen`], whose keys drive the flip alongside the
/// SIGINT/SIGTERM handling inside `serve_with_engine`.
fn run_tui_with_flip() -> Result<()> {
    let mut next = dux_tui::run(Box::new(companion::WebCompanion::new()))?;
    loop {
        match next {
            dux_tui::TuiExit::Done => break,
            dux_tui::TuiExit::FlipToServer {
                engine,
                listeners,
                urls,
                startup,
            } => {
                // Read before the engine and listeners move into
                // `serve_with_engine`. The reachability note is the startup
                // banner's, which the log viewer pins on top.
                let theme_name = engine.config.ui.theme.clone();
                let paths = engine.paths.clone();
                let keys = engine.config.keys.clone();

                // The log buffer is shared between the web console (the
                // producer, wired in serve_with_engine) and the status screen's
                // log viewer (the consumer). Created here so both get the same
                // handle, sized by `[server] log_viewer_lines`.
                let activity = dux_core::activity::ActivityRing::new(
                    dux_core::config::effective_log_viewer_lines(
                        engine.config.server.log_viewer_lines,
                    ),
                );

                // A failure here (no TTY, a raw-mode error) falls back to a plain
                // line, because the server must still run. `screen` lives outside
                // the tick closure so it can be dropped, restoring the terminal,
                // after serving returns. The `RefCell` is what lets
                // `serve_with_engine`'s two FnMut callbacks borrow it in turn: they
                // run one at a time on this thread, but a plain `&mut` capture would
                // be two simultaneous exclusive borrows.
                let screen = std::cell::RefCell::new(
                    match dux_tui::ServerStatusScreen::new(
                        &urls,
                        &theme_name,
                        &paths,
                        &keys,
                        activity.clone(),
                    ) {
                        Ok(screen) => Some(screen),
                        Err(err) => {
                            eprintln!(
                                "dux server running at {} (status screen unavailable: {err}). \
                                 Press Ctrl-C to stop",
                                urls.join(", ")
                            );
                            None
                        }
                    },
                );

                // Only a screen that actually took the terminal has anything to
                // give back, or keys to read: without one (no TTY) there is no
                // escape code to send and Ctrl-c is an ordinary signal.
                let hooks = if screen.borrow().is_some() {
                    dux_web::FlipHooks {
                        // A second stop signal mid-shutdown ends the process with
                        // no destructor run, so the terminal is given back first.
                        restore_terminal: Some(std::sync::Arc::new(dux_tui::restore_terminal)),
                    }
                } else {
                    dux_web::FlipHooks::default()
                };

                let (engine, exit) = dux_web::serve_with_engine(
                    *engine,
                    listeners,
                    activity,
                    startup,
                    hooks,
                    || {
                        // With the screen up, its keys drive the exit; without it,
                        // only SIGINT/SIGTERM (handled inside serve) can stop us.
                        match screen.borrow_mut().as_mut() {
                            Some(screen) => match screen.tick() {
                                dux_tui::ServerScreenTick::Continue => {
                                    dux_web::ServerTick::Continue
                                }
                                dux_tui::ServerScreenTick::ReturnToTui => {
                                    dux_web::ServerTick::ReturnToTui
                                }
                                dux_tui::ServerScreenTick::QuitProcess => {
                                    dux_web::ServerTick::QuitProcess
                                }
                            },
                            None => dux_web::ServerTick::Continue,
                        }
                    },
                    |message| {
                        // Through the status screen so it renders on its own themed
                        // line rather than as raw text wherever the cursor sits.
                        match screen.borrow_mut().as_mut() {
                            Some(screen) => screen.show_shutdown_message(message),
                            None => eprintln!("{message}"),
                        }
                    },
                    || {
                        // The screen holds the terminal in raw mode, so a second
                        // Ctrl-c during the shutdown wait is a key; it reads its
                        // keys and redraws on each turn of the wait.
                        match screen.borrow_mut().as_mut() {
                            Some(screen) => screen.shutdown_tick(),
                            None => false,
                        }
                    },
                )?;

                // Serving has stopped. Drop the status screen explicitly to
                // restore the terminal (leave raw mode + alt screen, show the
                // cursor) BEFORE resuming the TUI (which re-inits ratatui) or
                // before any final messages on quit.
                drop(screen.into_inner());

                match exit {
                    dux_web::ServerExit::QuitProcess => break,
                    dux_web::ServerExit::ForceQuit(handle) => {
                        // The terminal is the shell's again (the screen was just
                        // dropped), so the reason lands on its own screen; the
                        // engine goes first so queued config writes land.
                        dux_web::finish_forced_quit(engine, &handle, |code| {
                            std::process::exit(code)
                        });
                        // Still here: the signal hatch took the exit and is
                        // ending the process; a second exit must not race it.
                        loop {
                            std::thread::park();
                        }
                    }
                    dux_web::ServerExit::ReturnToTui => {
                        next = dux_tui::resume_after_server(
                            Box::new(engine),
                            // A fresh companion per resumed TUI: the previous
                            // serve's runtime is gone, and so is anything that
                            // was holding it.
                            Box::new(companion::WebCompanion::new()),
                        )?;
                    }
                }
            }
        }
    }
    Ok(())
}

/// Creates the config folder owner-only: `dux server` never runs `DuxPaths::ensure_dirs`,
/// so a first run would otherwise leave it at the umask default.
fn create_config_root(paths: &dux_core::config::DuxPaths) -> Result<()> {
    dux_core::file_modes::create_private_dir_all(&paths.root)?;
    Ok(())
}

fn init_server_logger(
    logging: &dux_core::config::LoggingConfig,
    paths: &dux_core::config::DuxPaths,
) {
    dux_core::logger::init(logging, paths);
    // The folder was tightened before the log existed, dropping its warnings; tightening is
    // idempotent, so running it again puts them in the log.
    dux_core::file_modes::restrict_to_owner_best_effort(&paths.root, "directory");
}

fn run_server(overrides: dux_core::config::ServerCliOverrides) -> Result<()> {
    let paths = dux_core::config::DuxPaths::discover()?;
    create_config_root(&paths)?;
    // `dux server` never calls `ensure_config`, so without this the bootstrap's
    // project-sync would create a comment-free config.toml on a first run that
    // starts in server mode. Registering the TUI's canonical renderer keeps
    // "the config file is the documentation" true on both entry points.
    dux_tui::install_canonical_renderer();
    // Fails closed: when [server.auth] cannot be read, dux does not serve
    // at all rather than serve with no password.
    let config = match dux_core::config::load_config(&paths) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("error: dux server cannot start: {error}");
            std::process::exit(1);
        }
    };

    // Initialize the logger early so every subsequent logger::* call in the server
    // path (bootstrap, bind) actually reaches dux.log.
    // OnceLock::set is idempotent, so it is safe if the TUI already initialized it (flip).
    init_server_logger(&config.logging, &paths);
    dux_core::logger::info("bootstrapping dux server");

    // Detected up front to feed the Tailscale leg of the bind plan; blocking is fine
    // at CLI startup and the call is bounded. A failed detection warns and proceeds
    // on the configured host only, never blocks, and under `auto` the serve path
    // keeps watching, so the warning says so rather than sounding final.
    let tailscale_mode = dux_core::config::effective_tailscale_mode(
        config.server.tailscale_mode(),
        overrides.no_tailscale,
    );
    // Warnings raised here print as the first lines of the server's log (the
    // same timestamped warning lines the flip's viewer shows), ahead of any bind.
    let mut startup_warnings = Vec::new();
    let mut undetected = None;
    let tailscale_ip = if tailscale_mode.wants_tailscale() {
        match dux_core::tailscale::detect_ip() {
            Ok(ip) => Some(ip),
            Err(reason) => {
                undetected = Some(reason);
                None
            }
        }
    } else {
        None
    };

    let plan = match dux_core::config::resolve_server_plan(&config.server, &overrides, tailscale_ip)
    {
        Ok(plan) => plan,
        Err(e) => {
            // No log will ever open, so the detection warning goes to stderr
            // ahead of the error rather than vanishing with the start.
            let warning = undetected.map(|reason| {
                dux_core::tailscale::undetected_warning(
                    tailscale_mode,
                    reason,
                    "the configured host",
                )
            });
            report_plan_failure(&mut std::io::stderr(), warning.as_deref(), &e.to_string());
            std::process::exit(1);
        }
    };

    // Worded once the plan says what is being served instead: on a loopback-only
    // plan these are the flip's exact words for the same situation. It opens the
    // log, and reaches stderr too whenever stdout is not a terminal.
    if let Some(reason) = undetected {
        startup_warnings.push(dux_web::StartupWarning {
            text: dux_core::tailscale::undetected_warning(
                tailscale_mode,
                reason,
                serving_without_tailscale(&plan),
            ),
            already_on_stderr: false,
        });
    }

    // Loud warning when binding a non-loopback address with no password set:
    // anyone who can reach the address can control your agents and worktrees.
    // Printed before the bind so it is visible even if a bind then fails.
    let is_local = |a: &std::net::SocketAddr| a.ip().is_loopback() || Some(a.ip()) == tailscale_ip;
    let alarms: Vec<String> = plan
        .addrs
        .iter()
        .filter(|p| !is_local(&p.addr()))
        .filter_map(|p| non_loopback_warning(p.addr(), &config.server.auth))
        .collect();
    raise_security_alarms(
        &alarms,
        dux_core::serve_log::StdStreams::current(),
        &mut std::io::stderr(),
        &mut startup_warnings,
    );

    dux_web::run_server(
        paths,
        plan,
        // Same display version as the TUI footer and the web sidebar
        // ("vX.Y.Z" for release builds, "development" otherwise) so all three
        // surfaces always show the same thing.
        dux_core::display_version().to_string(),
        startup_warnings,
    )
}

/// What a serve without its Tailscale leg is serving on, as the "Tailscale not
/// detected" warning names it. A loopback-only plan says "loopback", the word
/// the in-app flip uses for the same situation, so the two logs match.
fn serving_without_tailscale(plan: &dux_core::config::ServerPlan) -> &'static str {
    if plan.addrs.iter().all(|a| a.addr().ip().is_loopback()) {
        "loopback"
    } else {
        "the configured host"
    }
}

/// The "NO password" alarms go to stderr at once, before the engine loads, so a
/// redirected stdout (`dux server > access.log`) or a start that fails while
/// loading can never hide them; they also open the log itself, the same lines
/// the flip's viewer would show.
///
/// Each prints to stderr exactly once: the log line is marked as already there,
/// so neither the log's stderr echo nor a failed load prints it again.
fn raise_security_alarms(
    alarms: &[String],
    streams: dux_core::serve_log::StdStreams,
    stderr: &mut dyn std::io::Write,
    startup_warnings: &mut Vec<dux_web::StartupWarning>,
) {
    let early = streams.early_alarm_on_stderr();
    for alarm in alarms {
        if early {
            let _ = writeln!(stderr, "WARNING: {alarm}");
        }
        startup_warnings.push(dux_web::StartupWarning {
            text: alarm.clone(),
            already_on_stderr: early,
        });
    }
}

/// A start that failed while resolving what to bind: print the Tailscale
/// warning it had (no log is going to carry it) and then the error.
fn report_plan_failure(stderr: &mut dyn std::io::Write, warning: Option<&str>, error: &str) {
    if let Some(warning) = warning {
        let _ = writeln!(stderr, "WARNING: {warning}");
    }
    let _ = writeln!(stderr, "error: {error}");
}

/// The alarm for a listener beyond loopback (and beyond the machine's own
/// Tailscale address) with no password set, or `None` when a password is set:
/// every client from the network is then asked for it, whatever `require` says.
fn non_loopback_warning(
    addr: std::net::SocketAddr,
    auth: &dux_core::config::ServerAuthConfig,
) -> Option<String> {
    if auth.has_password() {
        return None;
    }
    Some(format!(
        "dux is binding {addr}, a non-loopback address, with NO password set. Anyone who can \
         reach this address can control your agents and worktrees. Set one with `dux config \
         set server.auth.password`, or only do this on a network you trust."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths_in(root: &std::path::Path) -> dux_core::config::DuxPaths {
        dux_core::config::DuxPaths {
            root: root.to_path_buf(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            socket_path: root.join("dux.sock"),
        }
    }

    #[test]
    fn a_symlinked_config_folder_warning_reaches_the_log_of_a_server_start() {
        let parent = std::env::temp_dir().join(format!("dux-server-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&parent);
        let target = parent.join("real");
        std::fs::create_dir_all(&target).unwrap();
        let link = parent.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let paths = paths_in(&link);

        create_config_root(&paths).unwrap();
        init_server_logger(&dux_core::config::LoggingConfig::default(), &paths);

        let log = std::fs::read_to_string(target.join("dux.log")).unwrap();
        let _ = std::fs::remove_dir_all(&parent);
        assert!(
            log.contains("is a symlink, so its permissions were left alone"),
            "the symlink warning was lost before the logger opened:\n{log}"
        );
    }

    #[test]
    fn server_start_creates_a_missing_config_folder_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let parent = std::env::temp_dir().join(format!("dux-server-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&parent);
        std::fs::create_dir_all(&parent).unwrap();
        let root = parent.join("fresh-home");

        create_config_root(&paths_in(&root)).unwrap();

        let mode = std::fs::metadata(&root).unwrap().permissions().mode() & 0o777;
        let _ = std::fs::remove_dir_all(&parent);
        assert_eq!(mode, 0o700);
    }

    /// The "NO password" alarm reaches stderr at once, before anything is loaded,
    /// so `dux server > access.log` still shows it and a failed start does not
    /// swallow it. It also opens the log, where the flip's viewer shows it too.
    #[test]
    fn the_security_alarm_goes_to_stderr_at_once_and_into_the_log() {
        let mut stderr = Vec::new();
        let mut startup_warnings = vec![dux_web::StartupWarning {
            text: "Tailscale not detected.".to_string(),
            already_on_stderr: false,
        }];
        // `dux server > access.log`: stdout a file, stderr the terminal.
        let streams = dux_core::serve_log::StdStreams {
            stdout_is_terminal: false,
            stderr_is_terminal: true,
            same_file: false,
        };
        raise_security_alarms(
            &[non_loopback_warning(
                "0.0.0.0:3890".parse().unwrap(),
                &dux_core::config::ServerAuthConfig::default(),
            )
            .unwrap()],
            streams,
            &mut stderr,
            &mut startup_warnings,
        );
        let printed = String::from_utf8(stderr).unwrap();
        assert!(
            printed.starts_with("WARNING: dux is binding 0.0.0.0:3890"),
            "{printed}"
        );
        assert_eq!(
            printed.lines().count(),
            1,
            "only the alarm, not the other warnings"
        );
        assert_eq!(startup_warnings.len(), 2);
        assert!(
            startup_warnings[1]
                .text
                .starts_with("dux is binding 0.0.0.0:3890")
        );
        assert!(
            startup_warnings[1].already_on_stderr,
            "so the log's stderr echo and a failed load leave it alone"
        );
        assert!(!startup_warnings[0].already_on_stderr);
    }

    /// On an interactive terminal the alarm is not printed early: the log line
    /// on that same terminal says it, and saying it twice is noise.
    #[test]
    fn on_a_terminal_the_alarm_is_left_to_the_log_line() {
        let streams = dux_core::serve_log::StdStreams {
            stdout_is_terminal: true,
            stderr_is_terminal: true,
            same_file: false,
        };
        let mut stderr = Vec::new();
        let mut startup_warnings = Vec::new();
        raise_security_alarms(
            &[non_loopback_warning(
                "0.0.0.0:3890".parse().unwrap(),
                &dux_core::config::ServerAuthConfig::default(),
            )
            .unwrap()],
            streams,
            &mut stderr,
            &mut startup_warnings,
        );
        assert!(stderr.is_empty());
        assert_eq!(startup_warnings.len(), 1);
        assert!(
            !startup_warnings[0].already_on_stderr,
            "so a start that fails before the log prints it then"
        );
    }

    /// A bad `--bind` ends the start before the log exists, so the Tailscale
    /// warning would be swallowed: it goes to stderr ahead of the error.
    #[test]
    fn a_failed_plan_still_prints_the_tailscale_warning() {
        let mut stderr = Vec::new();
        report_plan_failure(
            &mut stderr,
            Some("Tailscale not detected (test)."),
            "--bind expects an IP:port",
        );
        assert_eq!(
            String::from_utf8(stderr).unwrap(),
            "WARNING: Tailscale not detected (test).\nerror: --bind expects an IP:port\n"
        );
    }

    #[test]
    fn a_loopback_plan_is_called_loopback_as_the_flip_calls_it() {
        let plan = |addrs: Vec<dux_core::config::PlanAddr>| dux_core::config::ServerPlan {
            primary: addrs[0].addr(),
            addrs,
            tailscale: dux_core::config::TailscaleMode::Auto,
            forced_no: false,
        };
        let loopback = plan(vec![dux_core::config::PlanAddr::required(
            "127.0.0.1:3890".parse().unwrap(),
        )]);
        assert_eq!(serving_without_tailscale(&loopback), "loopback");
        let wide = plan(vec![dux_core::config::PlanAddr::required(
            "0.0.0.0:3890".parse().unwrap(),
        )]);
        assert_eq!(serving_without_tailscale(&wide), "the configured host");
    }

    #[test]
    fn the_non_loopback_alarm_names_the_address_and_the_risk() {
        let open = dux_core::config::ServerAuthConfig::default();
        let w = non_loopback_warning("0.0.0.0:3890".parse().unwrap(), &open)
            .expect("no password: an alarm");
        assert!(w.starts_with("dux is binding 0.0.0.0:3890, a non-loopback address"));
        assert!(w.contains("NO password"), "{w}");
        assert!(w.contains("dux config set server.auth.password"), "{w}");
        assert!(
            !w.starts_with("WARNING:"),
            "the log line carries its own warning glyph"
        );
        let guarded = dux_core::config::ServerAuthConfig {
            password_hash: dux_core::auth::hash_password(&dux_core::auth::Password::new(
                "orbit velvet quarry lantern cobalt".to_string(),
            ))
            .unwrap(),
            ..Default::default()
        };
        assert_eq!(
            non_loopback_warning("0.0.0.0:3890".parse().unwrap(), &guarded),
            None,
            "with a password every client from the network signs in"
        );
    }
}
