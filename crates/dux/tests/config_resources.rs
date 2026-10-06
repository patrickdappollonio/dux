//! `dux macros`, `providers`, `keys`, `themes` and `env` against a real dux:
//! one served in this process on a temporary config folder, reached by the
//! built binary over the control socket and, as a saved remote, over HTTP.
//! Then the same binary with no dux running, editing the file itself.
//!
//! A test binary of its own, because the dux it starts runs until this
//! process ends.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use dux_core::config::{DuxPaths, PlanAddr, ServerPlan, TailscaleMode};
use dux_core::lockfile::LockFileContents;

struct Run {
    out: Output,
}

impl Run {
    fn code(&self) -> i32 {
        self.out.status.code().expect("exited normally")
    }
    fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.out.stdout).into_owned()
    }
    fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.out.stderr).into_owned()
    }
}

/// Run the built dux on the config folder `home`, feeding it `input`.
fn dux(home: &Path, args: &[&str], input: &str) -> Run {
    use std::io::Write;
    let mut child = Command::new(env!("CARGO_BIN_EXE_dux"))
        .args(args)
        .env("DUX_HOME", home)
        .env_remove("DUX_REMOTE")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the dux binary runs");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    Run {
        out: child.wait_with_output().unwrap(),
    }
}

/// The `PATH` this test process started with, before it points every program
/// the serve might run at nothing.
static SHELL_PATH: std::sync::OnceLock<std::ffi::OsString> = std::sync::OnceLock::new();

fn append(path: &Path, text: &str) {
    use std::io::Write;
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap()
        .write_all(text.as_bytes())
        .unwrap();
}

/// A `dux … logs -f` that keeps running: its stdout lines arrive on a
/// channel as it prints them.
struct Follower {
    child: std::process::Child,
    lines: std::sync::mpsc::Receiver<String>,
}

impl Follower {
    fn start(home: &Path, args: &[&str]) -> Self {
        use std::io::BufRead;
        let mut child = Command::new(env!("CARGO_BIN_EXE_dux"))
            .args(args)
            .env("DUX_HOME", home)
            .env_remove("DUX_REMOTE")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("the dux binary runs");
        let stdout = child.stdout.take().unwrap();
        let (sender, lines) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if sender.send(line).is_err() {
                    return;
                }
            }
        });
        Self { child, lines }
    }

    /// Read lines until `want` is printed, failing after ten seconds.
    fn wait_for(&mut self, want: &str) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.lines.recv_timeout(left) {
                Ok(line) if line.contains(want) => return,
                Ok(_) => {}
                Err(_) => panic!("{want:?} was never printed"),
            }
        }
    }

    /// Press Ctrl-C on it and answer the signal that ended it.
    fn interrupt(&mut self) -> Option<i32> {
        use std::os::unix::process::ExitStatusExt;
        let pid = self.child.id().to_string();
        let sent = Command::new("kill")
            .args(["-INT", &pid])
            .env("PATH", SHELL_PATH.get().unwrap())
            .status()
            .unwrap();
        assert!(sent.success());
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.signal();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        panic!("the follow did not end on Ctrl-C");
    }
}

/// An empty, owner-only config folder.
fn home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dux-cfgres-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::set_permissions(
        &dir,
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
    )
    .unwrap();
    dir
}

const CONFIG: &str = "# kept by hand\n[macros]\n# says hi\nhi = { text = \"hello\", surface = \"agent\" }\n\n[ui]\ngithub_integration = false\n\n[server]\ntailscale = \"no\"\naccess_log = false\n";

/// Serve dux on `root` in this process, on a loopback port, and wait until
/// its control socket answers. Returns the port.
fn serve(root: &Path) -> u16 {
    let paths = DuxPaths {
        root: root.to_path_buf(),
        config_path: root.join("config.toml"),
        sessions_db_path: root.join("sessions.sqlite3"),
        worktrees_root: root.join("worktrees"),
        lock_path: root.join("dux.lock"),
        socket_path: root.join("dux.sock"),
    };
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let plan = ServerPlan {
        addrs: vec![PlanAddr::required(addr)],
        primary: addr,
        tailscale: TailscaleMode::No,
        forced_no: false,
    };
    // `dux keys ls` through the API reads the terminal UI's bindings, which
    // the dux binary installs before it serves.
    dux_tui::install_canonical_renderer();
    std::thread::spawn(move || dux_web::run_server(paths, plan, "test".to_string(), Vec::new()));
    let lock_path = root.join("dux.lock");
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let socket = std::fs::read_to_string(&lock_path)
            .map(|text| LockFileContents::parse(&text).control_socket)
            .unwrap_or_default();
        if socket.is_some_and(|socket| std::os::unix::net::UnixStream::connect(socket).is_ok()) {
            return port;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("dux never answered on its control socket");
}

#[test]
fn config_file_resources_through_a_running_dux_and_with_none() {
    let root = home("running");
    std::fs::write(root.join("config.toml"), CONFIG).unwrap();
    std::fs::create_dir_all(root.join("themes")).unwrap();
    std::fs::write(root.join("themes/alpha.toml"), "").unwrap();
    // `kill` is found on the path the shell gave, which is replaced next.
    SHELL_PATH
        .set(std::env::var_os("PATH").unwrap_or_default())
        .unwrap();
    // Nothing the serve might run (gh, tailscale, git) is the developer's own.
    // SAFETY: this binary runs this one test, and no other thread is running
    // yet.
    unsafe { std::env::set_var("PATH", root.join("no-tools")) };
    let port = serve(&root);
    let run = dux(
        &root,
        &["remote", "add", "here", &format!("http://127.0.0.1:{port}")],
        "",
    );
    assert_eq!(run.code(), 0, "{}", run.stderr());

    // A macro saved with dux running is in the running dux at once, which
    // the remote's API answers from, and in the file, comments kept.
    let run = dux(
        &root,
        &[
            "macros",
            "add",
            "greet",
            "hello there",
            "--surface",
            "both",
            "--yes",
        ],
        "",
    );
    assert_eq!(run.code(), 0, "{}", run.stderr());
    assert!(run.stdout().contains("greet"), "{}", run.stdout());
    let run = dux(&root, &["--remote", "here", "macros", "ls"], "");
    assert_eq!(run.code(), 0, "{}", run.stderr());
    assert_eq!(
        run.stdout(),
        "NAME    SURFACE   TEXT\nhi      agent     hello\ngreet   both      hello there\n"
    );
    let file = std::fs::read_to_string(root.join("config.toml")).unwrap();
    assert!(
        file.contains(
            "# kept by hand\n[macros]\n# says hi\nhi = { text = \"hello\", surface = \"agent\" }\n\
             greet = { text = \"hello there\", surface = \"both\" }\n"
        ),
        "{file}"
    );

    // Removing one that is not there is the running dux's refusal.
    let run = dux(&root, &["macros", "rm", "nope", "--yes"], "");
    assert_eq!(run.code(), 1, "{}", run.stderr());
    assert!(run.stderr().contains("nope"), "{}", run.stderr());

    // A value goes in through standard input and is never printed back.
    let run = dux(
        &root,
        &["env", "set", "API_TOKEN", "--stdin", "--yes"],
        "zzsecret\n",
    );
    assert_eq!(run.code(), 0, "{}", run.stderr());
    assert!(!run.stdout().contains("zzsecret"), "{}", run.stdout());
    let run = dux(&root, &["--remote", "here", "env", "ls"], "");
    assert_eq!(run.stdout(), "NAME\nAPI_TOKEN\n");
    let run = dux(&root, &["--remote", "here", "env", "ls", "--show"], "");
    assert_eq!(run.stdout(), "NAME        VALUE\nAPI_TOKEN   zzsecret\n");

    // The themes are the picker's, in its order, here and through the remote.
    let here = dux(&root, &["themes", "ls", "-q"], "");
    assert_eq!(here.code(), 0, "{}", here.stderr());
    assert!(
        here.stdout().starts_with("dux_dark\nalpha\nayu_dark\n"),
        "{}",
        here.stdout()
    );
    let remote = dux(&root, &["--remote", "here", "themes", "ls", "-q"], "");
    assert_eq!(remote.code(), 0, "{}", remote.stderr());
    assert_eq!(remote.stdout(), here.stdout());

    for listing in [&["keys", "ls", "-q"][..], &["providers", "ls", "-q"][..]] {
        let here = dux(&root, listing, "");
        let mut through = vec!["--remote", "here"];
        through.extend_from_slice(listing);
        let remote = dux(&root, &through, "");
        assert_eq!(here.code(), 0, "{listing:?}: {}", here.stderr());
        assert_eq!(remote.code(), 0, "{listing:?}: {}", remote.stderr());
        assert!(!here.stdout().is_empty(), "{listing:?}");
        assert_eq!(remote.stdout(), here.stdout(), "{listing:?}");
    }

    // Nobody is connected to this dux, over the socket or the remote.
    for through in [&[][..], &["--remote", "here"][..]] {
        let with = |rest: &[&str]| {
            let mut args = through.to_vec();
            args.extend_from_slice(rest);
            dux(&root, &args, "")
        };
        let run = with(&["server", "connections", "ls"]);
        assert_eq!(run.code(), 0, "{through:?}: {}", run.stderr());
        assert_eq!(
            run.stdout(),
            "CONNECTION   DEVICE   ADDRESS   SINCE   ATTACHED\n"
        );
        assert_eq!(with(&["server", "connections", "ls", "-q"]).stdout(), "");
        assert_eq!(
            with(&["server", "connections", "list", "--format", "json"]).stdout(),
            "[]\n"
        );
    }

    // The server log, a tail and a follow, over the socket and the remote.
    for (turn, through) in [&[][..], &["--remote", "here"][..]].into_iter().enumerate() {
        let (first, last, live) = (
            format!("tail {turn} first"),
            format!("tail {turn} last"),
            format!("written while following {turn}"),
        );
        append(&root.join("server.log"), &format!("{first}\n{last}\n"));
        let mut args = through.to_vec();
        args.extend_from_slice(&["server", "logs", "--lines", "2"]);
        let run = dux(&root, &args, "");
        assert_eq!(run.code(), 0, "{through:?}: {}", run.stderr());
        assert_eq!(run.stdout(), format!("{first}\n{last}\n"), "{through:?}");

        let mut args = through.to_vec();
        args.extend_from_slice(&["server", "logs", "-f", "--lines", "1"]);
        let mut follower = Follower::start(&root, &args);
        follower.wait_for(&last);
        append(&root.join("server.log"), &format!("{live}\n"));
        follower.wait_for(&live);
        assert_eq!(
            follower.interrupt(),
            Some(2),
            "{through:?}: Ctrl-C ends the follow"
        );
    }

    // With no dux running the command edits the file, comments kept.
    let stopped = home("stopped");
    std::fs::write(stopped.join("config.toml"), CONFIG).unwrap();
    let run = dux(&stopped, &["macros", "add", "bye", "see you", "--yes"], "");
    assert_eq!(run.code(), 0, "{}", run.stderr());
    assert!(
        run.stdout().contains("applies the next time dux starts"),
        "{}",
        run.stdout()
    );
    assert_eq!(
        std::fs::read_to_string(stopped.join("config.toml")).unwrap(),
        CONFIG.replace(
            "surface = \"agent\" }\n",
            "surface = \"agent\" }\nbye = { text = \"see you\", surface = \"agent\" }\n"
        )
    );
    let run = dux(&stopped, &["macros", "ls", "-q"], "");
    assert_eq!(run.stdout(), "hi\nbye\n");
    assert!(!stopped.join("sessions.sqlite3").exists());

    let _ = std::fs::remove_dir_all(&stopped);
    let _ = std::fs::remove_dir_all(&root);
}
