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
/// its control socket and its port answer. Returns the port. The port is one
/// the system handed out and this function let go, so another process can take
/// it first; the serve then stops and this panics saying so.
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
    let server =
        std::thread::spawn(move || dux_web::run_server(&paths, plan, "test".to_string(), &[]));
    let lock_path = root.join("dux.lock");
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        // A serve that stopped (the port was taken between the probe above
        // and its bind, or it could not start) says why.
        if server.is_finished() {
            match server.join() {
                Ok(Ok(())) => panic!("dux stopped serving before it answered"),
                Ok(Err(error)) => panic!("dux could not serve: {error:#}"),
                Err(payload) => std::panic::resume_unwind(payload),
            }
        }
        let socket = std::fs::read_to_string(&lock_path)
            .map(|text| LockFileContents::parse(&text).control_socket)
            .unwrap_or_default();
        // The control socket is bound before the port is, so the port is
        // ready only once it answers as this process's dux.
        if socket.is_some_and(|socket| std::os::unix::net::UnixStream::connect(socket).is_ok())
            && answers_as_this_dux(port)
        {
            return port;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("dux never answered on its control socket and its port");
}

/// Whether `GET /api/v1/build` on the loopback `port` names this process's
/// server run, which no other program on the port can.
fn answers_as_this_dux(port: u16) -> bool {
    use std::io::{Read, Write};

    let addr = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let timeout = Duration::from_secs(2);
    let Ok(mut stream) = std::net::TcpStream::connect_timeout(&addr, timeout) else {
        return false;
    };
    if stream.set_read_timeout(Some(timeout)).is_err()
        || write!(
            stream,
            "GET /api/v1/build HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n"
        )
        .is_err()
    {
        return false;
    }
    let mut response = String::new();
    stream.read_to_string(&mut response).is_ok()
        && response.contains(&dux_web::build_routes::build_identity().process)
}
