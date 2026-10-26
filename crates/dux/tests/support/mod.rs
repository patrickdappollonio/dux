//! What the command line's journeys share: running the built binary on a
//! config folder, making that folder, and serving a real dux on it in this
//! process.
//!
//! A journey that serves dux is a test binary of its own, because the dux it
//! starts runs until the process ends.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use dux_core::config::{DuxPaths, PlanAddr, ServerPlan, TailscaleMode};
use dux_core::lockfile::LockFileContents;

pub(crate) struct Run {
    out: Output,
}

impl Run {
    pub(crate) fn code(&self) -> i32 {
        self.out.status.code().expect("exited normally")
    }
    pub(crate) fn stdout(&self) -> String {
        String::from_utf8_lossy(&self.out.stdout).into_owned()
    }
    pub(crate) fn stderr(&self) -> String {
        String::from_utf8_lossy(&self.out.stderr).into_owned()
    }
}

/// Run the built dux on the config folder `home`, feeding it `input`.
pub(crate) fn dux(home: &Path, args: &[&str], input: &str) -> Run {
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
        .expect("stdin is piped")
        .write_all(input.as_bytes())
        .expect("feed dux its input");
    Run {
        out: child.wait_with_output().expect("dux ends"),
    }
}

/// An empty, owner-only config folder named for `name`.
pub(crate) fn home(prefix: &str, name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dux-{prefix}-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("make the config folder");
    std::fs::set_permissions(
        &dir,
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o700),
    )
    .expect("make the config folder owner-only");
    dir
}

/// Serve dux on `root` in this process, on a loopback port, and wait until
/// its control socket answers. Returns the port.
pub(crate) fn serve(root: &Path) -> u16 {
    let paths = DuxPaths {
        root: root.to_path_buf(),
        config_path: root.join("config.toml"),
        sessions_db_path: root.join("sessions.sqlite3"),
        worktrees_root: root.join("worktrees"),
        lock_path: root.join("dux.lock"),
        socket_path: root.join("dux.sock"),
    };
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind a loopback port")
        .local_addr()
        .expect("the bound port")
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
    std::thread::spawn(move || dux_web::run_server(&paths, plan, "test".to_string(), &[]));
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
