use anyhow::Result;

const SERVER_USAGE: &str = "\
Usage: dux server [OPTIONS]

Run the dux web UI over the headless engine.

Options:
      --bind <ADDR:PORT>      Address and port to listen on (e.g. 127.0.0.1:8080).
                              Overrides the [server] bind value in config.toml.
      --disable-auth          Run with the login gate OFF even when [auth] users are
                              configured. Intended for deployments behind an upstream
                              auth proxy (e.g. oauth2-proxy) that handles login itself.
      --insecure-allow-remote Allow binding a non-loopback address even though no
                              login is configured. Anyone who can reach the address
                              can control your agents and worktrees.
  -h, --help                  Print this help and exit.";

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
                listener,
                url,
            } => {
                // Read everything the status screen needs BEFORE the engine and
                // listener move into `serve_with_engine`. The theme name lives
                // on the engine's config; `loopback` decides whether to show the
                // no-auth warning. `local_addr` failures default to treating the
                // bind as loopback (the safe, no-warning assumption is wrong if
                // it's actually remote, but the bind already passed the gate in
                // the TUI pre-flight, so a non-loopback bind without the opt-in
                // never reaches here).
                let theme_name = engine.config.ui.theme.clone();
                let paths = engine.paths.clone();
                let loopback = listener
                    .local_addr()
                    .map(|addr| addr.ip().is_loopback())
                    .unwrap_or(true);

                // Try to set up the interactive status screen. If it fails (no
                // TTY, raw-mode error), fall back to a plain line — the server
                // must still run. `screen` lives outside the tick closure so we
                // can drop it (restoring the terminal) AFTER serving returns.
                let mut screen =
                    match dux_tui::ServerStatusScreen::new(&url, loopback, &theme_name, &paths) {
                        Ok(screen) => Some(screen),
                        Err(err) => {
                            eprintln!(
                                "dux server running at {url} (status screen unavailable: {err}) \
                                 — press Ctrl-C to stop"
                            );
                            None
                        }
                    };

                let (engine, exit) = dux_web::serve_with_engine(*engine, listener, || {
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

fn run_server(mut args: impl Iterator<Item = String>) -> Result<()> {
    let mut cli_bind: Option<String> = None;
    let mut cli_insecure_allow_remote = false;
    let mut cli_disable_auth = false;

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("{SERVER_USAGE}");
                return Ok(());
            }
            "--insecure-allow-remote" => cli_insecure_allow_remote = true,
            "--disable-auth" => cli_disable_auth = true,
            "--bind" => match args.next() {
                Some(value) => cli_bind = Some(value),
                None => {
                    eprintln!("error: --bind requires a value (e.g. --bind 127.0.0.1:8080)");
                    eprintln!("{SERVER_USAGE}");
                    std::process::exit(2);
                }
            },
            other if other.starts_with("--bind=") => {
                cli_bind = Some(other.trim_start_matches("--bind=").to_string());
            }
            other => {
                eprintln!("error: unknown argument \"{other}\"");
                eprintln!("{SERVER_USAGE}");
                std::process::exit(2);
            }
        }
    }

    let paths = dux_core::config::DuxPaths::discover()?;
    std::fs::create_dir_all(&paths.root)?;
    let config = dux_core::config::load_config(&paths);

    let auth_enabled = dux_core::auth::auth_enabled(&config, cli_disable_auth);

    let addr = match dux_web::resolve_bind(
        &config.server.bind,
        config.server.insecure_allow_remote,
        cli_bind.as_deref(),
        cli_insecure_allow_remote,
        auth_enabled,
    ) {
        Ok(addr) => addr,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };

    // Loud warning when auth is deliberately disabled on a reachable address:
    // --disable-auth turned the gate off on a non-loopback bind. Only an
    // upstream auth proxy makes this safe.
    if cli_disable_auth && !addr.ip().is_loopback() {
        eprintln!(
            "WARNING: --disable-auth is set and dux is binding {addr}, a non-loopback \
             address, with NO login gate. Anyone who can reach this address can control \
             your agents and worktrees. Only do this when an upstream auth proxy is \
             handling authentication in front of dux."
        );
    }

    println!("dux server listening on http://{addr} — open it in your browser");
    dux_web::run_server(paths, addr, cli_disable_auth)
}
