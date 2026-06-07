use anyhow::Result;

const SERVER_USAGE: &str = "\
Usage: dux server [OPTIONS]

Run the dux web UI over the headless engine.

Options:
      --port <PORT>            LOCAL MODE port. dux binds 127.0.0.1:port (and the
                               machine's Tailscale address:port unless disabled).
                               Used only when no --listen / [server] listen_addrs
                               are set. Overrides [server] port (default 8080).
      --listen <ADDR:PORT>     FULL WEB MODE listener (repeatable). Each is an
                               IP:port socket address (hostnames are NOT resolved).
                               Replaces [server] listen_addrs entirely. Only used
                               when built-in ACME/TLS is off.
      --bind <ADDR:PORT>       DEPRECATED alias for --listen (accepted with a note).
      --no-tailscale           Skip Tailscale detection for LOCAL MODE this run
                               (serve loopback only).
      --disable-auth           Run with the login gate OFF even when [auth] users are
                               configured. Intended for deployments behind an upstream
                               auth proxy (e.g. oauth2-proxy) that handles login itself.
      --insecure-allow-remote  Allow a non-loopback plain-HTTP listen_addrs entry even
                               though no login is configured. Anyone who can reach the
                               address can control your agents and worktrees.
      --acme-domain <DOMAIN>   Domain to request a Let's Encrypt certificate for
                               (repeatable). Overrides [server.acme] domains.
      --acme-email <EMAIL>     Contact email for Let's Encrypt. Overrides config.
      --http-port <PORT>       Port for the ACME HTTP-01 challenge and HTTPS redirect
                               (default 80). Overrides [server.acme] http_port.
      --https-port <PORT>      Port the TLS web UI listens on (default 443).
                               Overrides [server.acme] https_port.
      --no-acme                Force built-in ACME/TLS off even if config enables it.
      --dangerously-listen-http
                               Allow serving PLAIN HTTP on a non-loopback address.
                               Traffic (including the login password) is unencrypted.
  -h, --help                   Print this help and exit.";

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
                // on the engine's config. The flip is local-only by construction
                // (loopback + optional Tailscale), so it is always "all local" —
                // there is no public, no-auth path to warn about here.
                let theme_name = engine.config.ui.theme.clone();
                let paths = engine.paths.clone();
                let loopback = true;

                // The flip never passes --disable-auth (that is a `dux server`
                // CLI flag), so the gate is on iff [auth] has valid users. The
                // user count drives the quiet "login required" line; the served
                // engine rebuilds its AuthState from this same config.
                let auth_enabled = dux_core::auth::auth_enabled(&engine.config, false);
                let user_count = dux_core::auth::parse_users(&engine.config.auth.users).len();

                // Try to set up the interactive status screen. If it fails (no
                // TTY, raw-mode error), fall back to a plain line — the server
                // must still run. `screen` lives outside the tick closure so we
                // can drop it (restoring the terminal) AFTER serving returns.
                let mut screen = match dux_tui::ServerStatusScreen::new(
                    &urls,
                    loopback,
                    auth_enabled,
                    user_count,
                    &theme_name,
                    &paths,
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

                let (engine, exit) = dux_web::serve_with_engine(*engine, listeners, || {
                    // With the screen up, its keys drive the exit; without it,
                    // only SIGINT/SIGTERM (handled inside serve) can stop us.
                    match screen.as_mut() {
                        Some(screen) => match screen.tick() {
                            dux_tui::ServerScreenTick::Continue => dux_web::ServerTick::Continue,
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

    let cli_disable_auth = parsed.disable_auth;
    let overrides = parsed.into_overrides();

    let paths = dux_core::config::DuxPaths::discover()?;
    std::fs::create_dir_all(&paths.root)?;
    let config = dux_core::config::load_config(&paths);

    let auth_enabled = dux_core::auth::auth_enabled(&config, cli_disable_auth);

    // Detect the Tailscale address up front (blocking is fine at CLI startup).
    // It feeds LOCAL MODE and the per-entry classification in the resolver. When
    // detection fails but the user opted in (tailscale_enabled, no --no-tailscale),
    // warn and proceed on loopback only — never block.
    let tailscale_wanted = config.server.tailscale_enabled && !overrides.no_tailscale;
    let tailscale_ip = if tailscale_wanted {
        match dux_core::tailscale::detect_ip() {
            Ok(ip) => Some(ip),
            Err(reason) => {
                eprintln!(
                    "WARNING: Tailscale not detected ({}) — serving on loopback only. \
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

    let plan = match dux_core::config::resolve_server_plan(
        &config.server,
        auth_enabled,
        cli_disable_auth,
        &overrides,
        tailscale_ip,
        &paths.root,
    ) {
        Ok(plan) => plan,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };

    // Print the startup banner / warnings from the resolved plan, then hand the
    // WHOLE plan to dux-web, which serves plain HTTP or built-in TLS accordingly.
    match &plan {
        dux_core::config::ServerPlan::PlainHttp { addrs } => {
            // An address is LOCAL when it is loopback OR the detected Tailscale
            // address; only PUBLIC addresses are gated and warned about. The plan
            // carries each address tagged required/best-effort; the banner only
            // needs the SocketAddr, so project it out.
            let is_local =
                |a: &std::net::SocketAddr| a.ip().is_loopback() || Some(a.ip()) == tailscale_ip;
            let all_addrs_local = addrs.iter().all(|p| is_local(&p.addr));

            // Loud warning when auth is deliberately disabled on a reachable
            // (public) address: --disable-auth turned the gate off. Only an
            // upstream auth proxy makes this safe.
            if cli_disable_auth && !all_addrs_local {
                for plan_addr in addrs.iter().filter(|p| !is_local(&p.addr)) {
                    eprintln!(
                        "WARNING: --disable-auth is set and dux is binding {}, a non-loopback \
                         address, with NO login gate. Anyone who can reach this address can control \
                         your agents and worktrees. Only do this when an upstream auth proxy is \
                         handling authentication in front of dux.",
                        plan_addr.addr
                    );
                }
            }

            // A best-effort (Tailscale) leg may fail to bind at serve time and
            // degrade to loopback; note that in the banner so the printed URL list
            // is understood as the intended set, not a guarantee.
            let urls = addrs
                .iter()
                .map(|p| format!("http://{}", p.addr))
                .collect::<Vec<_>>()
                .join(", ");
            println!("dux server listening on {urls} — open it in your browser");
        }
        dux_core::config::ServerPlan::Acme {
            https_addr,
            domains,
            production,
            ..
        } => {
            // Normalize the same way the TLS path does so the banner shows the
            // hostname the certificate will actually carry. Fall back to the raw
            // first domain if normalization somehow fails (the resolver already
            // validated there is ≥1 domain; build_acme_state re-validates).
            let primary = dux_web::tls::normalize_domains(domains)
                .ok()
                .and_then(|d| d.into_iter().next())
                .unwrap_or_else(|| domains.first().cloned().unwrap_or_default());
            let suffix = if https_addr.port() == 443 {
                String::new()
            } else {
                format!(":{}", https_addr.port())
            };
            // Loud warning when auth is deliberately disabled on a built-in-TLS
            // server. Unlike the plain-HTTP arm, an ACME server is ALWAYS public
            // (a browser-trusted certificate on :443), so there is no "local"
            // exception — the gate-off banner fires whenever --disable-auth is set.
            if cli_disable_auth {
                eprintln!("{}", dux_web::acme_disable_auth_warning());
            }
            if !*production {
                eprintln!(
                    "NOTE: [server.acme] production = false — using the Let's Encrypt STAGING \
                     environment. Certificates will NOT be trusted by browsers; set production = \
                     true once the staging flow succeeds."
                );
            }
            println!(
                "dux server listening with built-in TLS on https://{primary}{suffix}/ — \
                 plain HTTP on :80 redirects here and answers ACME HTTP-01 challenges. \
                 Open it in your browser once the certificate is issued."
            );
        }
    }

    dux_web::run_server(paths, plan, cli_disable_auth)
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
    port: Option<u16>,
    /// `--listen` (repeatable) and the deprecated `--bind` alias, in order.
    listen: Vec<String>,
    no_tailscale: bool,
    insecure_allow_remote: bool,
    disable_auth: bool,
    acme_domains: Vec<String>,
    acme_email: Option<String>,
    http_port: Option<u16>,
    https_port: Option<u16>,
    no_acme: bool,
    dangerously_listen_http: bool,
}

impl ServerArgs {
    fn into_overrides(self) -> dux_core::config::ServerCliOverrides {
        dux_core::config::ServerCliOverrides {
            port: self.port,
            listen: self.listen,
            no_tailscale: self.no_tailscale,
            insecure_allow_remote: self.insecure_allow_remote,
            acme_domains: self.acme_domains,
            acme_email: self.acme_email,
            http_port: self.http_port,
            https_port: self.https_port,
            no_acme: self.no_acme,
            dangerously_listen_http: self.dangerously_listen_http,
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
            "--insecure-allow-remote" => out.insecure_allow_remote = true,
            "--disable-auth" => out.disable_auth = true,
            "--no-acme" => out.no_acme = true,
            "--no-tailscale" => out.no_tailscale = true,
            "--dangerously-listen-http" => out.dangerously_listen_http = true,
            "--port" => match take_port("--port", inline, &mut args) {
                Ok(p) => out.port = Some(p),
                Err(e) => return ParsedServerArgs::Error(e),
            },
            "--listen" => match take_value("--listen", inline, &mut args) {
                Ok(v) => out.listen.push(v),
                Err(e) => return ParsedServerArgs::Error(e),
            },
            "--bind" => match take_value("--bind", inline, &mut args) {
                Ok(v) => {
                    eprintln!(
                        "note: --bind is deprecated; use --listen {v} instead (treating it as --listen)."
                    );
                    out.listen.push(v);
                }
                Err(e) => return ParsedServerArgs::Error(e),
            },
            "--acme-domain" => match take_value("--acme-domain", inline, &mut args) {
                Ok(v) => out.acme_domains.push(v),
                Err(e) => return ParsedServerArgs::Error(e),
            },
            "--acme-email" => match take_value("--acme-email", inline, &mut args) {
                Ok(v) => out.acme_email = Some(v),
                Err(e) => return ParsedServerArgs::Error(e),
            },
            "--http-port" => match take_port("--http-port", inline, &mut args) {
                Ok(p) => out.http_port = Some(p),
                Err(e) => return ParsedServerArgs::Error(e),
            },
            "--https-port" => match take_port("--https-port", inline, &mut args) {
                Ok(p) => out.https_port = Some(p),
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
        assert!(a.port.is_none());
        assert!(a.listen.is_empty());
        assert!(!a.no_tailscale);
        assert!(!a.insecure_allow_remote);
        assert!(!a.disable_auth);
        assert!(a.acme_domains.is_empty());
        assert!(a.acme_email.is_none());
        assert!(a.http_port.is_none());
        assert!(a.https_port.is_none());
        assert!(!a.no_acme);
        assert!(!a.dangerously_listen_http);
    }

    #[test]
    fn port_parses_as_number() {
        let a = ok(&["--port", "9090"]);
        assert_eq!(a.port, Some(9090));
        let a = ok(&["--port=7000"]);
        assert_eq!(a.port, Some(7000));
    }

    #[test]
    fn listen_is_repeatable() {
        let a = ok(&["--listen", "127.0.0.1:8080", "--listen=0.0.0.0:9000"]);
        assert_eq!(
            a.listen,
            vec!["127.0.0.1:8080".to_string(), "0.0.0.0:9000".to_string()]
        );
    }

    #[test]
    fn bind_is_a_deprecated_alias_for_listen() {
        let a = ok(&["--bind", "0.0.0.0:8080"]);
        assert_eq!(a.listen, vec!["0.0.0.0:8080".to_string()]);
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
    fn acme_domain_is_repeatable() {
        let a = ok(&[
            "--acme-domain",
            "a.example.com",
            "--acme-domain",
            "b.example.com",
        ]);
        assert_eq!(
            a.acme_domains,
            vec!["a.example.com".to_string(), "b.example.com".to_string()]
        );
    }

    #[test]
    fn acme_domain_accepts_equals_form() {
        let a = ok(&["--acme-domain=a.example.com", "--acme-domain=b.example.com"]);
        assert_eq!(
            a.acme_domains,
            vec!["a.example.com".to_string(), "b.example.com".to_string()]
        );
    }

    #[test]
    fn ports_parse_as_numbers() {
        let a = ok(&["--http-port", "8080", "--https-port=8443"]);
        assert_eq!(a.http_port, Some(8080));
        assert_eq!(a.https_port, Some(8443));
    }

    #[test]
    fn non_numeric_port_errors() {
        let msg = err(&["--http-port", "nope"]);
        assert!(msg.contains("--http-port"), "should name the flag: {msg}");
        assert!(msg.contains("nope"), "should echo the bad value: {msg}");
    }

    #[test]
    fn out_of_range_port_errors() {
        let msg = err(&["--https-port", "70000"]);
        assert!(msg.contains("--https-port"), "should name the flag: {msg}");
    }

    #[test]
    fn listen_and_email_take_values() {
        let a = ok(&[
            "--listen",
            "0.0.0.0:8080",
            "--acme-email",
            "ops@example.com",
        ]);
        assert_eq!(a.listen, vec!["0.0.0.0:8080".to_string()]);
        assert_eq!(a.acme_email.as_deref(), Some("ops@example.com"));
    }

    #[test]
    fn boolean_flags_set_their_fields() {
        let a = ok(&[
            "--insecure-allow-remote",
            "--disable-auth",
            "--no-acme",
            "--dangerously-listen-http",
        ]);
        assert!(a.insecure_allow_remote);
        assert!(a.disable_auth);
        assert!(a.no_acme);
        assert!(a.dangerously_listen_http);
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
