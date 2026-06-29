use anyhow::Result;

const SERVER_USAGE: &str = "\
Usage: dux server [OPTIONS]

Run the dux web UI over the headless engine. dux is a trusted-local tool with no
login gate; only run a non-loopback bind on a network you trust.

Options:
      --bind <ADDR:PORT>  Bind this exact address, overriding [server] host+port.
                          An IP:port socket address (hostnames are NOT resolved),
                          e.g. 0.0.0.0:8080. May be given only once.
      --port <PORT>       Override [server] port only (ignored when --bind is set).
                          dux binds host:port (and the machine's Tailscale address
                          unless disabled). Default port 8080.
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

/// Default arm: run the TUI, and when it flips to the web server, serve the
/// same engine in this process until the server stops, then resume the TUI.
/// The cycle repeats until the user quits from either surface.
///
/// While serving, the terminal shows the dux-tui status screen
/// ([`dux_tui::ServerStatusScreen`]); its keys drive the flip — `q`/`Esc`
/// returns to the TUI, `Ctrl-C` quits the process — alongside SIGINT/SIGTERM
/// (→ `QuitProcess`) handled inside `serve_with_engine`.
fn run_tui_with_flip() -> Result<()> {
    let mut next = dux_tui::run()?;
    loop {
        match next {
            dux_tui::TuiExit::Done => break,
            dux_tui::TuiExit::FlipToServer {
                engine,
                listeners,
                urls,
            } => {
                // Read everything the status screen needs BEFORE the engine and
                // listeners move into `serve_with_engine`. The theme name lives
                // on the engine's config. The flip is LOCAL MODE: the primary addr
                // is always loopback, and the only non-loopback addr is the
                // Tailscale best-effort leg (if it bound). Derive the safety note
                // from the URLs: a non-loopback URL means the Tailscale leg
                // successfully bound and the server is reachable on the tailnet.
                let theme_name = engine.config.ui.theme.clone();
                let paths = engine.paths.clone();
                let safety_note = if urls.iter().any(|u| {
                    u.strip_prefix("http://")
                        .and_then(|rest| rest.rsplit_once(':'))
                        .map(|(host, _)| {
                            let ip = host.trim_start_matches('[').trim_end_matches(']');
                            ip != "127.0.0.1" && ip != "::1"
                        })
                        .unwrap_or(false)
                }) {
                    Some(
                        "Reachable by other devices on your tailnet (no login). \
                         Disable with tailscale_enabled = false under [server]."
                            .to_string(),
                    )
                } else {
                    None
                };

                // The activity buffer is shared between the web console (the
                // producer, wired in serve_with_engine) and the status screen
                // (the consumer). Created here so both get the same handle.
                let activity = dux_core::activity::ActivityRing::new();

                // Try to set up the interactive status screen. If it fails (no
                // TTY, raw-mode error), fall back to a plain line — the server
                // must still run. `screen` lives outside the tick closure so we
                // can drop it (restoring the terminal) AFTER serving returns.
                let mut screen = match dux_tui::ServerStatusScreen::new(
                    &urls,
                    safety_note,
                    &theme_name,
                    &paths,
                    activity.clone(),
                ) {
                    Ok(screen) => Some(screen),
                    Err(err) => {
                        eprintln!(
                            "dux server running at {} (status screen unavailable: {err}) \
                                 — press Ctrl-C to stop",
                            urls.join(", ")
                        );
                        None
                    }
                };

                let (engine, exit) =
                    dux_web::serve_with_engine(*engine, listeners, activity, || {
                        // With the screen up, its keys drive the exit; without it,
                        // only SIGINT/SIGTERM (handled inside serve) can stop us.
                        match screen.as_mut() {
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
                    })?;

                // Serving has stopped. Drop the status screen explicitly to
                // restore the terminal (leave raw mode + alt screen, show the
                // cursor) BEFORE resuming the TUI (which re-inits ratatui) or
                // before any final messages on quit.
                drop(screen);

                match exit {
                    dux_web::ServerExit::QuitProcess => break,
                    dux_web::ServerExit::ReturnToTui => {
                        next = dux_tui::resume_after_server(Box::new(engine))?;
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
    let config = dux_core::config::load_config(&paths);

    // Initialize the logger early so every subsequent logger::* call in the server
    // path (bootstrap, bind) actually reaches dux.log.
    // OnceLock::set is idempotent — safe if the TUI already initialized it (flip).
    dux_core::logger::init(&config.logging, &paths);
    dux_core::logger::info("bootstrapping dux server");

    // Detect the Tailscale address up front (blocking is fine at CLI startup).
    // It feeds the Tailscale leg of the bind plan. When detection fails but the
    // user opted in (tailscale_enabled, no --no-tailscale), warn and proceed on
    // the configured host only — never block.
    let tailscale_wanted = config.server.tailscale_enabled && !overrides.no_tailscale;
    let tailscale_ip = if tailscale_wanted {
        match dux_core::tailscale::detect_ip() {
            Ok(ip) => Some(ip),
            Err(reason) => {
                eprintln!(
                    "WARNING: Tailscale not detected ({}) — serving on the configured host only. \
                     Set tailscale_enabled = false in [server] (or pass --no-tailscale) to \
                     silence this warning.",
                    reason.reason()
                );
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
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };

    // Loud warning when binding a non-loopback address: dux has no login gate, so
    // anyone who can reach the address can control your agents and worktrees.
    // Fires pre-bind (stderr) so it is visible even if a bind then fails.
    let is_local = |a: &std::net::SocketAddr| a.ip().is_loopback() || Some(a.ip()) == tailscale_ip;
    for plan_addr in plan.addrs.iter().filter(|p| !is_local(&p.addr())) {
        eprintln!(
            "WARNING: dux is binding {}, a non-loopback address, with NO login gate. Anyone \
             who can reach this address can control your agents and worktrees. Only do this on \
             a network you trust, or front dux with an upstream auth proxy.",
            plan_addr.addr()
        );
    }

    dux_web::run_server(
        paths,
        plan,
        // Same display version as the TUI footer and the web sidebar
        // ("vX.Y.Z" for release builds, "development" otherwise) so all three
        // surfaces always show the same thing.
        dux_core::display_version().to_string(),
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
