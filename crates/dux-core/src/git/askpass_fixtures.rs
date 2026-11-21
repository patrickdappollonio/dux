//! Test fixtures for network git calls that could ask for a credential: a
//! process with none of the developer's prompts in it, a recording askpass, and
//! a local remote that wants a password.

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Set in the test process [`runs_here_without_the_developers_prompts`]
/// starts, so the test it runs knows it is already there.
pub(crate) const PROMPTLESS: &str = "DUX_TEST_PROMPTLESS_PROCESS";

/// Whether the calling test's body runs in this process. The first call runs
/// the test `name` (its full path, as `cargo test` prints it) again in a test
/// process of its own whose environment names no askpass, display or git
/// configuration of the developer's, asserts it passed there, and returns
/// false; in that process it returns true.
///
/// The production git a test runs inherits this process's environment, and a
/// test can add variables to it but not take any away, so a developer's
/// `SSH_ASKPASS` or `GIT_ASKPASS` would otherwise be what git and a stand-in
/// ssh ask, in a window on their desktop.
pub(crate) fn runs_here_without_the_developers_prompts(name: &str) -> bool {
    if std::env::var_os(PROMPTLESS).is_some() {
        return true;
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([name, "--exact", "--test-threads=1"])
        .env(PROMPTLESS, "1")
        .env_remove("SSH_ASKPASS")
        .env_remove("SSH_ASKPASS_REQUIRE")
        .env_remove("GIT_ASKPASS")
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_SYSTEM", "/dev/null")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .output()
        .unwrap();
    let said = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success() && said.contains("test result: ok. 1 passed"),
        "{name} did not pass on its own:\n{said}"
    );
    false
}

/// A stand-in askpass at `dir/askpass` that records each run in `dir/asked`
/// and answers `answer`, the way a helper answering from a wallet does.
pub(crate) fn recording_askpass(dir: &Path, answer: &str) -> String {
    let script = dir.join("askpass");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\necho '{answer}'\n",
            dir.join("asked").display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    script.to_string_lossy().into_owned()
}

/// How many times the [`recording_askpass`] in `dir` has run.
pub(crate) fn times_asked(dir: &Path) -> usize {
    std::fs::read_to_string(dir.join("asked")).map_or(0, |text| text.lines().count())
}

/// A remote at `127.0.0.1` that answers every request with a 401 asking for
/// Basic credentials, except one carrying `user:stored`. That one gets
/// `refs` as a dumb-HTTP ref advertisement when it asks for `info/refs` and
/// `refs` is given, and a 404 otherwise. The `Authorization` header of each
/// request, or `none`, is recorded.
pub(crate) fn credential_wanting_remote(refs: Option<String>) -> (String, Arc<Mutex<Vec<String>>>) {
    use std::io::{BufRead as _, Write as _};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/repo.git", listener.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { return };
            let mut reader = std::io::BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            let _ = reader.read_line(&mut request);
            let mut auth = "none".to_string();
            let mut line = String::new();
            while reader.read_line(&mut line).is_ok_and(|read| read > 2) {
                if let Some(value) = line.strip_prefix("Authorization: ") {
                    auth = value.trim().to_string();
                }
                line.clear();
            }
            let stored = auth == "Basic dXNlcjpzdG9yZWQ=";
            record.lock().unwrap().push(auth);
            let reply = match &refs {
                Some(refs) if stored && request.contains("/info/refs") => format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {}\r\n\
                     Connection: close\r\n\r\n{refs}",
                    refs.len()
                ),
                _ if stored => {
                    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        .to_string()
                }
                _ => "HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Basic realm=\"x\"\r\n\
                      Content-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_string(),
            };
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    (url, seen)
}
