//! `dux server` serves its API on the control socket beside its web listener.
//!
//! A test binary of its own, because the serve is stopped the way an operator
//! stops it, with SIGTERM to this process, and no other test may be running in
//! it when that signal arrives.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use dux_core::config::{DuxPaths, PlanAddr, ServerPlan, TailscaleMode};
use dux_core::lockfile::LockFileContents;

fn get_over_socket(path: &std::path::Path, uri: &str) -> String {
    let mut stream = std::os::unix::net::UnixStream::connect(path).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .unwrap();
    write!(
        stream,
        "GET {uri} HTTP/1.1\r\nHost: dux\r\nConnection: close\r\n\r\n"
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("an answer");
    response
}

#[test]
fn dux_server_answers_on_its_control_socket_and_removes_it_on_exit() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().to_path_buf();
    let paths = DuxPaths {
        root: root.clone(),
        config_path: root.join("config.toml"),
        sessions_db_path: root.join("sessions.sqlite3"),
        worktrees_root: root.join("worktrees"),
        lock_path: root.join("dux.lock"),
        socket_path: root.join("dux.sock"),
    };
    std::fs::write(
        &paths.config_path,
        "[ui]\ngithub_integration = false\n\n[server]\ntailscale = \"no\"\ncontrol_socket = \"ctl.sock\"\n",
    )
    .unwrap();
    // Nothing the serve might run (gh, tailscale, git) is the developer's own.
    // SAFETY: this binary runs this one test, and no other thread is running
    // yet.
    unsafe { std::env::set_var("PATH", root.join("no-tools")) };
    let loopback = std::net::SocketAddr::from(([127, 0, 0, 1], 0));
    let plan = ServerPlan {
        addrs: vec![PlanAddr::required(loopback)],
        primary: loopback,
        tailscale: TailscaleMode::No,
        forced_no: false,
    };
    let lock_path = paths.lock_path.clone();
    let serve = std::thread::spawn(move || {
        dux_web::run_server(paths, plan, "test".to_string(), Vec::new())
    });

    let socket = root.join("ctl.sock");
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let named = std::fs::read_to_string(&lock_path)
            .map(|text| LockFileContents::parse(&text).control_socket)
            .unwrap_or_default();
        if named.as_deref() == Some(socket.as_path()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut answer = String::new();
    while Instant::now() < deadline {
        answer = get_over_socket(&socket, "/api/v1/workspace");
        if answer.starts_with("HTTP/1.1 200") {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(answer.starts_with("HTTP/1.1 200"), "{answer}");
    assert!(answer.contains("\"projects\""), "{answer}");

    rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::TERM)
        .expect("signal this process");
    serve
        .join()
        .expect("the serve thread")
        .expect("dux server stops cleanly");

    assert!(!socket.exists(), "a clean exit removes the socket");
    let lock = std::fs::read_to_string(&lock_path).unwrap();
    assert!(!lock.contains("control-socket"), "{lock}");
}
