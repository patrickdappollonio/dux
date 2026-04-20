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
mod remote;
mod statusline;
mod storage;
mod theme;

use std::path::Path;

use anyhow::Result;

fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();

    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return Ok(());
    }

    let paths = config::DuxPaths::discover()?;

    if args.first().map(|s| s.as_str()) == Some("remote") {
        return run_remote_subcommand(&args[1..], &paths);
    }

    if args.first().map(|s| s.as_str()) == Some("serve") {
        return run_remote_serve(&args[1..], &paths);
    }

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

        return cli::run(config_args, &paths);
    }

    // TUI: always create the root directory (so the lockfile can be
    // opened), acquire the lock, then let bootstrap create everything
    // else. A losing process never touches shared state beyond the
    // empty root.
    std::fs::create_dir_all(&paths.root)?;
    let lock = acquire_lock_or_exit(&paths.lock_path);
    let mut app = app::App::bootstrap_with_lock(paths, lock)?;
    app.run()
}

fn print_help() {
    println!(
        "dux\n\n\
         Terminal UI for AI worktree sessions.\n\n\
         Usage:\n\
          dux                      Launch the TUI\n\
          dux config               Manage the configuration file\n\
          dux remote share         Host a remote-share session (with local TUI)\n\
          dux remote connect <c>   Connect to a host using pairing code c\n\
          dux serve                Run headless; remote-only host mode\n\n\
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

fn run_remote_subcommand(args: &[String], paths: &config::DuxPaths) -> Result<()> {
    let sub = args.first().map(|s| s.as_str()).unwrap_or("");
    match sub {
        "share" => run_remote_share(paths),
        "connect" => {
            let code = args.get(1).ok_or_else(|| {
                anyhow::anyhow!("missing pairing code; usage: dux remote connect <code>")
            })?;
            run_remote_connect(code, paths)
        }
        "" | "--help" | "-h" => {
            print_remote_help();
            Ok(())
        }
        other => {
            eprintln!("unknown remote subcommand: {other}");
            print_remote_help();
            std::process::exit(2);
        }
    }
}

fn print_remote_help() {
    println!(
        "dux remote\n\n\
         Share or connect to a dux session over an encrypted peer-to-peer\n\
         link. All traffic is end-to-end encrypted via iroh.\n\n\
         Usage:\n\
           dux remote share               Host a session; prints a pairing code.\n\
           dux remote connect <code>      Connect to a host using its code.\n\n\
         Remote config lives under [remote] in the config file\n\
         (dux config path)."
    );
}

fn run_remote_share(paths: &config::DuxPaths) -> Result<()> {
    use std::io::stdout;
    use std::time::Duration;

    use crossterm::event::EnableMouseCapture;
    use crossterm::execute;
    use crossterm::terminal::{EnterAlternateScreen, enable_raw_mode};
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;

    // Acquire the same single-instance lock the TUI uses. Remote share is
    // the TUI with a different transport wired in, so sharing a config
    // directory with a plain `dux` instance is still forbidden.
    std::fs::create_dir_all(&paths.root)?;
    let lock = acquire_lock_or_exit(&paths.lock_path);

    // Bootstrap the app first so errors (config parse, db open, etc.)
    // surface before we touch the terminal.
    let mut app = app::App::bootstrap_with_lock(paths.clone(), lock)?;
    let remote_cfg = app.remote_config_snapshot();
    if !remote_cfg.enabled {
        eprintln!(
            "error: `dux remote share` is disabled by config\n\
             (set [remote].enabled = true in {} to use remote share)",
            paths.config_path.display()
        );
        std::process::exit(2);
    }
    let ttl_secs = remote_cfg.code_ttl_secs;

    // Build the tokio runtime that hosts iroh. It lives on this thread —
    // we'll hand off to ratatui for the UI side.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build tokio runtime: {e}"))?;

    // Bind the iroh endpoint and generate the pairing code.
    let relay_url = remote_cfg.relay_url.clone();
    let prepared = rt
        .block_on(async {
            remote::server::prepare_host_session(Duration::from_secs(ttl_secs), relay_url).await
        })
        .map_err(|e| anyhow::anyhow!("remote: failed to bring up endpoint: {e:#}"))?;

    // Show the code on stderr before the UI takes over. After the
    // alternate-screen switch, stderr is still visible in the terminal's
    // normal buffer so the user can find it there if they miss it.
    println!("Pairing code (valid for {ttl_secs}s):");
    println!();
    println!("    {}", prepared.code);
    println!();
    println!("Run on the client:   dux remote connect <code>");
    println!("Press any key in the dux TUI when ready; Ctrl-C to abort.");
    println!();

    // Wire up TeeBackend -> session capture path, plus inbound bridge
    // that turns `RemoteInboundEvent`s from the network into the app's
    // `WorkerEvent::Remote*` variants. Capture is bounded so a slow
    // network path drops frames rather than growing memory; the
    // inbound path is unbounded because its producers (handshake +
    // input) are rate-limited by human typing.
    let (capture_tx, capture_rx) = tokio::sync::mpsc::channel::<remote::CaptureEvent>(
        remote::tee_backend::DEFAULT_CAPTURE_CAPACITY,
    );
    let (inbound_tx, mut inbound_rx) =
        tokio::sync::mpsc::unbounded_channel::<remote::server::RemoteInboundEvent>();
    let host_label = hostname_label();
    let secret = prepared.secret.clone();
    let endpoint = prepared.endpoint.clone();
    let rt_handle = rt.handle().clone();
    rt_handle.spawn(async move {
        match remote::server::serve_host_session(
            endpoint, secret, host_label, capture_rx, inbound_tx,
        )
        .await
        {
            Ok(outcome) => logger::info(&format!("remote: session ended: {outcome:?}")),
            Err(err) => logger::error(&format!("remote: session failed: {err:#}")),
        }
    });

    // Bridge the remote session's inbound events into the app's worker
    // event channel. The App processes them in `drain_events` just like
    // any other background event.
    let worker_tx = app.worker_tx_clone();
    rt_handle.spawn(async move {
        while let Some(event) = inbound_rx.recv().await {
            use remote::server::RemoteInboundEvent;
            let worker_event = match event {
                RemoteInboundEvent::Paired { peer } => {
                    app::WorkerEvent::RemotePaired { peer_label: peer }
                }
                RemoteInboundEvent::Disconnected { reason } => {
                    app::WorkerEvent::RemoteDisconnected { reason }
                }
                RemoteInboundEvent::InputKey(key) => app::WorkerEvent::RemoteInputKey(key),
                RemoteInboundEvent::LeadRequested => app::WorkerEvent::RemoteLeadRequested,
            };
            if worker_tx.send(worker_event).is_err() {
                break;
            }
        }
    });

    // Install the ratatui panic hook equivalent ourselves so a panic
    // restores the terminal; without this the user's shell would be left
    // in raw mode if the app panics.
    let default_panic = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::terminal::disable_raw_mode();
        let _ = crossterm::execute!(std::io::stdout(), crossterm::terminal::LeaveAlternateScreen);
        default_panic(info);
    }));

    enable_raw_mode()?;
    execute!(stdout(), EnterAlternateScreen, EnableMouseCapture)?;

    let raw_backend = CrosstermBackend::new(stdout());
    let tee = remote::TeeBackend::new(raw_backend, capture_tx);
    let terminal = Terminal::new(tee)?;
    let result = app.run_with_terminal(terminal);

    let _ = execute!(
        stdout(),
        crossterm::event::DisableMouseCapture,
        crossterm::terminal::LeaveAlternateScreen
    );
    let _ = crossterm::terminal::disable_raw_mode();

    // Tear down the iroh runtime last. Drop order matters: drop the
    // runtime after the capture channel is already dead (app exited),
    // so the session task has a chance to finish its graceful Bye.
    drop(prepared.endpoint);
    drop(rt);

    result
}

fn run_remote_connect(code: &str, paths: &config::DuxPaths) -> Result<()> {
    // Load the user's config so the client can honor `[remote].relay_url`
    // when the host is on a self-hosted relay. We deliberately do NOT
    // acquire the single-instance lock: a client connection is read-only
    // against the local state — it doesn't mutate sessions.sqlite3 or
    // the worktrees root — and gating it behind the lock would prevent
    // running `dux` and `dux remote connect` on the same machine.
    let relay_url = match config::ensure_config(paths) {
        Ok(cfg) => cfg.remote.relay_url,
        Err(_) => None,
    };
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build tokio runtime: {e}"))?;
    let label = hostname_label();
    let code = code.to_string();
    rt.block_on(async move { remote::run_client(&code, &label, relay_url).await })
}

fn run_remote_serve(args: &[String], paths: &config::DuxPaths) -> Result<()> {
    use std::io::Write;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use ratatui::Terminal;

    // Parse flags. Supported:
    //   --cols N          viewport width
    //   --rows N          viewport height
    //   --code-file PATH  also write the pairing code to PATH
    //   --idle-exit-secs N exit after N seconds with no client connected
    let mut cols = remote::headless_backend::DEFAULT_COLS;
    let mut rows = remote::headless_backend::DEFAULT_ROWS;
    let mut code_file: Option<std::path::PathBuf> = None;
    let mut idle_exit_secs: Option<u64> = None;

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--help" | "-h" => {
                print_serve_help();
                return Ok(());
            }
            "--cols" => {
                i += 1;
                cols = args
                    .get(i)
                    .and_then(|v| v.parse().ok())
                    .ok_or_else(|| anyhow::anyhow!("--cols requires a positive integer"))?;
            }
            "--rows" => {
                i += 1;
                rows = args
                    .get(i)
                    .and_then(|v| v.parse().ok())
                    .ok_or_else(|| anyhow::anyhow!("--rows requires a positive integer"))?;
            }
            "--code-file" => {
                i += 1;
                code_file = Some(
                    args.get(i)
                        .ok_or_else(|| anyhow::anyhow!("--code-file requires a path"))?
                        .into(),
                );
            }
            "--idle-exit-secs" => {
                i += 1;
                idle_exit_secs =
                    Some(args.get(i).and_then(|v| v.parse().ok()).ok_or_else(|| {
                        anyhow::anyhow!("--idle-exit-secs requires a positive integer")
                    })?);
            }
            other => {
                eprintln!("unknown flag: {other}");
                print_serve_help();
                std::process::exit(2);
            }
        }
        i += 1;
    }

    std::fs::create_dir_all(&paths.root)?;
    let lock = acquire_lock_or_exit(&paths.lock_path);

    let mut app = app::App::bootstrap_with_lock(paths.clone(), lock)?;
    let remote_cfg = app.remote_config_snapshot();
    if !remote_cfg.enabled {
        eprintln!(
            "error: `dux serve` is disabled by config\n\
             (set [remote].enabled = true in {} to use headless serve)",
            paths.config_path.display()
        );
        std::process::exit(2);
    }
    if !remote_cfg.allow_remote_input {
        eprintln!(
            "error: `dux serve` requires [remote].allow_remote_input = true\n\
             (headless mode is useless without remote input)."
        );
        std::process::exit(2);
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build tokio runtime: {e}"))?;

    let relay_url = remote_cfg.relay_url.clone();
    let prepared = rt
        .block_on(async {
            remote::server::prepare_host_session(
                Duration::from_secs(remote_cfg.code_ttl_secs),
                relay_url,
            )
            .await
        })
        .map_err(|e| anyhow::anyhow!("remote: failed to bring up endpoint: {e:#}"))?;

    println!("{}", prepared.code);
    if let Some(path) = &code_file
        && let Err(err) = std::fs::write(path, &prepared.code)
    {
        eprintln!(
            "warning: failed to write code-file {}: {err}",
            path.display()
        );
    }
    eprintln!(
        "dux serve: listening for client (code valid for {}s)",
        remote_cfg.code_ttl_secs
    );
    let _ = std::io::stdout().flush();

    let (capture_tx, capture_rx) = tokio::sync::mpsc::channel::<remote::CaptureEvent>(
        remote::tee_backend::DEFAULT_CAPTURE_CAPACITY,
    );
    let (inbound_tx, mut inbound_rx) =
        tokio::sync::mpsc::unbounded_channel::<remote::server::RemoteInboundEvent>();
    let shutdown_flag = Arc::new(AtomicBool::new(false));
    let client_ever_connected = Arc::new(AtomicBool::new(false));

    // SIGTERM / SIGINT → set shutdown flag.
    {
        let flag = Arc::clone(&shutdown_flag);
        signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&flag))?;
        signal_hook::flag::register(signal_hook::consts::SIGINT, flag)?;
    }

    let host_label = hostname_label();
    let secret = prepared.secret.clone();
    let endpoint = prepared.endpoint.clone();
    rt.handle().spawn(async move {
        match remote::server::serve_host_session(
            endpoint, secret, host_label, capture_rx, inbound_tx,
        )
        .await
        {
            Ok(outcome) => logger::info(&format!("dux serve: session ended: {outcome:?}")),
            Err(err) => logger::error(&format!("dux serve: session failed: {err:#}")),
        }
    });

    // Bridge from iroh session → WorkerEvent.
    let worker_tx = app.worker_tx_clone();
    let seen_flag = Arc::clone(&client_ever_connected);
    rt.handle().spawn(async move {
        while let Some(event) = inbound_rx.recv().await {
            use remote::server::RemoteInboundEvent;
            let worker_event = match event {
                RemoteInboundEvent::Paired { peer } => {
                    seen_flag.store(true, Ordering::Relaxed);
                    eprintln!("dux serve: client '{peer}' connected");
                    app::WorkerEvent::RemotePaired { peer_label: peer }
                }
                RemoteInboundEvent::Disconnected { reason } => {
                    eprintln!("dux serve: client disconnected ({reason})");
                    app::WorkerEvent::RemoteDisconnected { reason }
                }
                RemoteInboundEvent::InputKey(key) => app::WorkerEvent::RemoteInputKey(key),
                RemoteInboundEvent::LeadRequested => app::WorkerEvent::RemoteLeadRequested,
            };
            if worker_tx.send(worker_event).is_err() {
                break;
            }
        }
    });

    // Idle-exit watchdog: if configured and no client has ever connected
    // within N seconds, flip the shutdown flag.
    if let Some(secs) = idle_exit_secs {
        let flag = Arc::clone(&shutdown_flag);
        let seen = Arc::clone(&client_ever_connected);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(secs));
            if !seen.load(Ordering::Relaxed) {
                eprintln!("dux serve: idle timeout ({secs}s) with no client — exiting");
                flag.store(true, Ordering::Relaxed);
            }
        });
    }

    // Build headless Terminal with TeeBackend on top of HeadlessBackend.
    let inner = remote::HeadlessBackend::new(cols, rows);
    let tee = remote::TeeBackend::new(inner, capture_tx);
    let terminal = Terminal::new(tee)?;

    let result = app.run_headless_with_terminal(terminal, Arc::clone(&shutdown_flag));

    drop(prepared.endpoint);
    drop(rt);

    result
}

fn print_serve_help() {
    println!(
        "dux serve\n\n\
         Run dux headlessly — no TTY, no local UI. A pairing code is\n\
         printed to stdout; the first `dux remote connect <code>` takes\n\
         control. Useful for home servers, CI runners, and other remote\n\
         boxes.\n\n\
         Usage:\n\
           dux serve [flags]\n\n\
         Flags:\n\
           --cols N              Viewport width  (default 200)\n\
           --rows N              Viewport height (default 60)\n\
           --code-file PATH      Also write the pairing code to PATH.\n\
           --idle-exit-secs N    Exit after N seconds if no client has\n\
                                 connected.\n"
    );
}

fn hostname_label() -> String {
    std::env::var("HOSTNAME")
        .ok()
        .or_else(|| {
            rustix::system::uname()
                .nodename()
                .to_str()
                .ok()
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "dux".to_string())
}
