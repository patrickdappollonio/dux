mod companion;

use anyhow::Result;

const SERVER_USAGE: &str = "\
Usage: dux server [OPTIONS]

Run the dux web UI over the headless engine. dux is a trusted-local tool with no
login gate; only run a non-loopback bind on a network you trust.

Options:
      --bind <ADDR:PORT>  Bind this exact address, overriding [server] host+port.
                          An IP:port socket address (hostnames are NOT resolved),
                          e.g. 0.0.0.0:3890. May be given only once.
      --port <PORT>       Override [server] port only (ignored when --bind is set).
                          dux binds host:port (and the machine's Tailscale address
                          unless disabled). Default port 3890.
      --no-tailscale      Skip Tailscale detection this run (serve the configured
                          host only).
  -h, --help              Print this help and exit.";

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("server") => run_server(args),
        _ => run_tui_with_flip(),
    }
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
                    dux_core::config::log_viewer_capacity(engine.config.server.log_viewer_lines),
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
                        // The screen holds the terminal in raw mode, so a second
                        // Ctrl-c during the shutdown wait is a key to watch for.
                        force_quit_key: Some(Box::new(dux_tui::wait_for_force_quit_key)),
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
                )?;

                // Serving has stopped. Drop the status screen explicitly to
                // restore the terminal (leave raw mode + alt screen, show the
                // cursor) BEFORE resuming the TUI (which re-inits ratatui) or
                // before any final messages on quit.
                drop(screen.into_inner());

                match exit {
                    dux_web::ServerExit::QuitProcess => break,
                    dux_web::ServerExit::ForceQuit => {
                        // The terminal is the shell's again (the screen was just
                        // dropped), so the reason lands on its own screen; the
                        // engine goes first so queued config writes land.
                        dux_web::finish_forced_quit(engine, |code| std::process::exit(code));
                        unreachable!("finish_forced_quit exits");
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

fn run_server(args: impl Iterator<Item = String>) -> Result<()> {
    let parsed = match parse_server_args(args) {
        ParsedServerArgs::HelpRequested => {
            println!("{SERVER_USAGE}");
            return Ok(());
        }
        ParsedServerArgs::Error(msg) => {
            eprintln!("error: {msg}");
            eprintln!("{SERVER_USAGE}");
            std::process::exit(2);
        }
        ParsedServerArgs::Ok(parsed) => parsed,
    };

    let overrides = parsed.into_overrides();

    let paths = dux_core::config::DuxPaths::discover()?;
    std::fs::create_dir_all(&paths.root)?;
    // `dux server` never calls `ensure_config`, so without this the bootstrap's
    // project-sync would create a comment-free config.toml on a first run that
    // starts in server mode. Registering the TUI's canonical renderer keeps
    // "the config file is the documentation" true on both entry points.
    dux_tui::install_canonical_renderer();
    let config = dux_core::config::load_config(&paths);

    // Initialize the logger early so every subsequent logger::* call in the server
    // path (bootstrap, bind) actually reaches dux.log.
    // OnceLock::set is idempotent, so it is safe if the TUI already initialized it (flip).
    dux_core::logger::init(&config.logging, &paths);
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

    // Loud warning when binding a non-loopback address: dux has no login gate, so
    // anyone who can reach the address can control your agents and worktrees.
    // Printed before the bind so it is visible even if a bind then fails.
    let is_local = |a: &std::net::SocketAddr| a.ip().is_loopback() || Some(a.ip()) == tailscale_ip;
    let alarms: Vec<String> = plan
        .addrs
        .iter()
        .filter(|p| !is_local(&p.addr()))
        .map(|p| non_loopback_warning(p.addr()))
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

/// The "NO login" alarms go to stderr at once, before the engine loads, so a
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
/// Tailscale address): there is no login gate in front of it.
fn non_loopback_warning(addr: std::net::SocketAddr) -> String {
    format!(
        "dux is binding {addr}, a non-loopback address, with NO login gate. Anyone who can \
         reach this address can control your agents and worktrees. Only do this on a network \
         you trust, or front dux with an upstream auth proxy."
    )
}

/// Outcome of parsing `dux server` arguments. Separated from `run_server` so the
/// argument parser is unit-testable without touching config/discovery.
enum ParsedServerArgs {
    Ok(ServerArgs),
    HelpRequested,
    Error(String),
}

/// Raw parsed `dux server` flags before config is loaded.
#[derive(Default)]
struct ServerArgs {
    /// `--bind <ADDR:PORT>`: an exact bind address, overriding config host+port.
    /// May be given only once.
    bind: Option<String>,
    port: Option<u16>,
    no_tailscale: bool,
}

impl ServerArgs {
    fn into_overrides(self) -> dux_core::config::ServerCliOverrides {
        dux_core::config::ServerCliOverrides {
            bind: self.bind,
            port: self.port,
            no_tailscale: self.no_tailscale,
        }
    }
}

fn parse_server_args(mut args: impl Iterator<Item = String>) -> ParsedServerArgs {
    let mut out = ServerArgs::default();

    // Pull the value for a `--flag VALUE` or `--flag=VALUE` form. `inline` is
    // Some when the `=` form was used.
    fn take_value(
        name: &str,
        inline: Option<String>,
        args: &mut impl Iterator<Item = String>,
    ) -> Result<String, String> {
        match inline {
            Some(v) => Ok(v),
            None => args
                .next()
                .ok_or_else(|| format!("{name} requires a value")),
        }
    }

    fn parse_port(name: &str, raw: &str) -> Result<u16, String> {
        raw.parse::<u16>()
            .map_err(|_| format!("{name} expects a port number 0-65535, got \"{raw}\""))
    }

    // Pull a port-valued flag's value and parse it in one step, so the three
    // port arms (`--port`/`--http-port`/`--https-port`) collapse to a single line
    // each that only differs in the field they assign.
    fn take_port(
        name: &str,
        inline: Option<String>,
        args: &mut impl Iterator<Item = String>,
    ) -> Result<u16, String> {
        let raw = take_value(name, inline, args)?;
        parse_port(name, &raw)
    }

    while let Some(arg) = args.next() {
        // Split `--flag=value` once; bare flags have no `=`.
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) => (f.to_string(), Some(v.to_string())),
            None => (arg.clone(), None),
        };

        match flag.as_str() {
            "--help" | "-h" => return ParsedServerArgs::HelpRequested,
            "--no-tailscale" => out.no_tailscale = true,
            "--port" => match take_port("--port", inline, &mut args) {
                Ok(p) => out.port = Some(p),
                Err(e) => return ParsedServerArgs::Error(e),
            },
            "--bind" => match take_value("--bind", inline, &mut args) {
                Ok(v) => {
                    if out.bind.is_some() {
                        return ParsedServerArgs::Error(
                            "--bind may be given only once".to_string(),
                        );
                    }
                    out.bind = Some(v);
                }
                Err(e) => return ParsedServerArgs::Error(e),
            },
            other => {
                return ParsedServerArgs::Error(format!("unknown argument \"{other}\""));
            }
        }
    }

    ParsedServerArgs::Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> ParsedServerArgs {
        parse_server_args(args.iter().map(|s| s.to_string()))
    }

    fn ok(args: &[&str]) -> ServerArgs {
        match parse(args) {
            ParsedServerArgs::Ok(a) => a,
            ParsedServerArgs::HelpRequested => panic!("unexpected help"),
            ParsedServerArgs::Error(e) => panic!("unexpected error: {e}"),
        }
    }

    fn err(args: &[&str]) -> String {
        match parse(args) {
            ParsedServerArgs::Error(e) => e,
            other => panic!("expected error, got {}", matches_label(&other)),
        }
    }

    fn matches_label(p: &ParsedServerArgs) -> &'static str {
        match p {
            ParsedServerArgs::Ok(_) => "Ok",
            ParsedServerArgs::HelpRequested => "HelpRequested",
            ParsedServerArgs::Error(_) => "Error",
        }
    }

    /// The "NO login" alarm reaches stderr at once, before anything is loaded,
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
            &[non_loopback_warning("0.0.0.0:3890".parse().unwrap())],
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
            &[non_loopback_warning("0.0.0.0:3890".parse().unwrap())],
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
        let w = non_loopback_warning("0.0.0.0:3890".parse().unwrap());
        assert!(w.starts_with("dux is binding 0.0.0.0:3890, a non-loopback address"));
        assert!(w.contains("NO login gate"));
        assert!(
            !w.starts_with("WARNING:"),
            "the log line carries its own warning glyph"
        );
    }

    #[test]
    fn empty_args_parse_to_defaults() {
        let a = ok(&[]);
        assert!(a.bind.is_none());
        assert!(a.port.is_none());
        assert!(!a.no_tailscale);
    }

    #[test]
    fn port_parses_as_number() {
        let a = ok(&["--port", "9090"]);
        assert_eq!(a.port, Some(9090));
        let a = ok(&["--port=7000"]);
        assert_eq!(a.port, Some(7000));
    }

    #[test]
    fn bind_parses_once() {
        assert_eq!(
            ok(&["--bind", "0.0.0.0:8888"]).bind.as_deref(),
            Some("0.0.0.0:8888")
        );
    }

    #[test]
    fn second_bind_is_rejected() {
        assert!(err(&["--bind", "a:1", "--bind", "b:2"]).contains("once"));
    }

    #[test]
    fn removed_flags_unknown() {
        for f in [
            "--listen",
            "--disable-auth",
            "--insecure-allow-remote",
            "--acme-domain",
            "--no-acme",
            "--dangerously-listen-http",
        ] {
            assert!(
                err(&[f]).contains("unknown argument")
                    || err(&[f, "x"]).contains("unknown argument")
            );
        }
    }

    #[test]
    fn no_tailscale_sets_its_field() {
        let a = ok(&["--no-tailscale"]);
        assert!(a.no_tailscale);
    }

    #[test]
    fn help_flags_request_help() {
        assert!(matches!(
            parse(&["--help"]),
            ParsedServerArgs::HelpRequested
        ));
        assert!(matches!(parse(&["-h"]), ParsedServerArgs::HelpRequested));
    }

    #[test]
    fn unknown_flag_errors() {
        let msg = err(&["--what-is-this"]);
        assert!(
            msg.contains("--what-is-this"),
            "should name the unknown flag: {msg}"
        );
    }

    #[test]
    fn value_flag_without_value_errors() {
        let msg = err(&["--bind"]);
        assert!(msg.contains("--bind"), "should name the flag: {msg}");
        assert!(msg.contains("requires a value"), "should explain: {msg}");
    }
}
