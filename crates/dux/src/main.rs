use anyhow::Result;

const SERVER_USAGE: &str = "\
Usage: dux server [OPTIONS]

Run the dux web UI over the headless engine.

Options:
      --bind <ADDR:PORT>      Address and port to listen on (e.g. 127.0.0.1:8080).
                              Overrides the [server] bind value in config.toml.
      --insecure-allow-remote Allow binding a non-loopback address even though the
                              web UI has no authentication yet. Anyone who can reach
                              the address can control your agents and worktrees.
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
/// This intermediate step uses a `Continue`-only tick, so the only way out of
/// the server is SIGINT/SIGTERM (→ `QuitProcess`); sub-step 5c replaces the
/// tick with the interactive status screen that adds a return-to-TUI key.
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
                println!(
                    "dux server running at {url} — Ctrl-C stops it (status screen lands in the next change)"
                );
                let (engine, exit) = dux_web::serve_with_engine(*engine, listener, || {
                    dux_web::ServerTick::Continue
                })?;
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

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("{SERVER_USAGE}");
                return Ok(());
            }
            "--insecure-allow-remote" => cli_insecure_allow_remote = true,
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

    let addr = match dux_web::resolve_bind(
        &config.server.bind,
        config.server.insecure_allow_remote,
        cli_bind.as_deref(),
        cli_insecure_allow_remote,
    ) {
        Ok(addr) => addr,
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    };

    println!("dux server listening on http://{addr} — open it in your browser");
    dux_web::run_server(paths, addr)
}
