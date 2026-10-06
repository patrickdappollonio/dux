//! `dux server connections ls` and `dux server logs`: who is connected to the
//! server, and what its log says.
//!
//! Both ask a running dux. The log is also a plain file, so with no dux
//! running it is read from the file itself, the way `dux config get` reads
//! `config.toml`.

use std::ops::ControlFlow;
use std::path::PathBuf;

use serde::Deserialize;

use super::connect::Client;
use super::output::{self, Listing, Row, Shape};
use super::transport::Request;
use super::{CliError, Exit};
use crate::attachments::{ConnectionView, Surface, TargetKind};
use crate::background_serve::TUI_DEVICE_LABEL;
use crate::config::DuxPaths;
use crate::file_follow::{LogError, MAX_LINES, open_log};

/// How many lines `dux server logs` shows when `--lines` does not say.
pub const DEFAULT_LINES: usize = 100;

/// What a dux answers the log route with when it writes no server log: it is
/// not serving the web UI, or it could not open the file.
pub const NO_SERVER_LOG: &str = "this dux is not writing a server log right now: it is not \
     serving the web UI, or it could not open the file (dux.log says why)";

/// The path of the log route; `follow` streams, and `lines` is how many of
/// the last lines to start from.
const LOG_ROUTE: &str = "/api/v1/server/log";

/// `dux server connections ls`.
pub fn connections_ls(client: &Client, shape: Shape) -> Result<String, CliError> {
    let connections: Vec<ConnectionView> = client.get_json("/api/v1/server/connections")?;
    let rows = connections
        .iter()
        .map(|connection| Row {
            id: connection.id.clone(),
            cells: vec![
                connection.id.clone(),
                device_cell(connection),
                address_cell(connection),
                connection.since.clone(),
                attached_cell(connection),
            ],
            json: serde_json::to_value(connection).unwrap_or_default(),
        })
        .collect();
    Ok(output::render(
        &Listing {
            headers: vec!["CONNECTION", "DEVICE", "ADDRESS", "SINCE", "ATTACHED"],
            rows,
        },
        shape,
    ))
}

/// The device as a person reads it: the browser and system the `User-Agent`
/// names, else a word for the kind of client.
fn device_cell(connection: &ConnectionView) -> String {
    connection
        .device
        .as_deref()
        .and_then(crate::device_label::short_device_label)
        .unwrap_or_else(|| match connection.surface {
            Surface::Browser => "a browser".to_string(),
            Surface::TerminalUi => TUI_DEVICE_LABEL.to_string(),
        })
}

/// The address, marked when it is only the peer's.
fn address_cell(connection: &ConnectionView) -> String {
    match (&connection.address, connection.verified) {
        (Some(address), true) => address.clone(),
        (Some(address), false) => format!("{address} (unverified)"),
        (None, _) => "-".to_string(),
    }
}

/// What it streams, each tab or terminal with whether it drives it.
fn attached_cell(connection: &ConnectionView) -> String {
    if connection.attachments.is_empty() {
        return "-".to_string();
    }
    connection
        .attachments
        .iter()
        .map(|attachment| {
            let what = match attachment.target.kind {
                TargetKind::Tab => "tab",
                TargetKind::Terminal => "terminal",
            };
            let driving = if attachment.driving { " (driving)" } else { "" };
            format!("{what} {}{driving}", attachment.target.id)
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Deserialize)]
struct LogRead {
    lines: Vec<String>,
    #[serde(default)]
    note: Option<String>,
}

/// The last `lines` lines of a running dux's server log, and the sentence it
/// adds when the answer was cut short.
pub fn log_tail(client: &Client, lines: usize) -> Result<(Vec<String>, Option<String>), CliError> {
    let read: LogRead = client.get_json(&format!("{LOG_ROUTE}?lines={lines}"))?;
    Ok((read.lines, read.note))
}

/// The most of an error's body the client keeps, and the longest log line it
/// reads: a dux that sends more is not one to keep listening to.
const ERROR_BODY_LIMIT: usize = 64 * 1024;
const LINE_LIMIT: usize = 1024 * 1024;

/// The last `lines` lines of a running dux's server log, then every line it
/// writes after, handed to `on_line` as they arrive, until `on_line` breaks.
/// The stream ending on its own means dux stopped serving it: it is stopping,
/// or this sign-in ended.
pub fn log_follow(
    client: &Client,
    lines: usize,
    on_line: &mut dyn FnMut(&str) -> ControlFlow<()>,
) -> Result<(), CliError> {
    let mut status = 200;
    let mut refusal = Vec::new();
    let mut oversized = None;
    let mut line: Vec<u8> = Vec::new();
    let mut stopped = false;
    client.stream(
        Request::get(format!("{LOG_ROUTE}?follow=true&lines={lines}")),
        &mut |code, piece| {
            status = code;
            if code != 200 {
                refusal.extend_from_slice(piece);
                if refusal.len() > ERROR_BODY_LIMIT {
                    oversized = Some("an error longer than 64 KiB");
                    return ControlFlow::Break(());
                }
                return ControlFlow::Continue(());
            }
            // Only the bytes that just arrived are looked at for a line end.
            let mut rest = piece;
            while let Some(end) = rest.iter().position(|byte| *byte == b'\n') {
                line.extend_from_slice(&rest[..end]);
                rest = &rest[end + 1..];
                let text = String::from_utf8_lossy(&line).into_owned();
                line.clear();
                if on_line(text.trim_end_matches('\r')).is_break() {
                    stopped = true;
                    return ControlFlow::Break(());
                }
            }
            line.extend_from_slice(rest);
            if line.len() > LINE_LIMIT {
                oversized = Some("a log line longer than 1 MiB");
                return ControlFlow::Break(());
            }
            ControlFlow::Continue(())
        },
    )?;
    if let Some(what) = oversized {
        return Err(CliError::new(
            Exit::Failed,
            format!(
                "{} sent {what}, so the client stopped reading it (HTTP status {status})",
                client.target()
            ),
        ));
    }
    if status != 200 {
        return Err(client.refusal(&super::transport::Response {
            status,
            body: refusal,
        }));
    }
    if stopped {
        return Ok(());
    }
    Err(CliError::new(
        Exit::NotRunning,
        format!(
            "{} ended the log stream: it is stopping, or this sign-in ended; run the command \
             again",
            client.target()
        ),
    ))
}

fn writes_no_log(error: &CliError) -> bool {
    error.message == NO_SERVER_LOG
}

/// [`log_tail`], reading the file at `paths` instead when the dux answered
/// that it writes no log and `paths` is given (the dux is this machine's).
pub fn tail_or_file(
    client: &Client,
    paths: Option<&DuxPaths>,
    lines: usize,
) -> Result<(Vec<String>, Option<String>), CliError> {
    match (log_tail(client, lines), paths) {
        (Err(error), Some(paths)) if writes_no_log(&error) => file_tail(&log_file(paths)?, lines),
        (answer, _) => answer,
    }
}

/// [`log_follow`], following the file at `paths` instead when the dux
/// answered that it writes no log and `paths` is given.
pub fn follow_or_file(
    client: &Client,
    paths: Option<&DuxPaths>,
    lines: usize,
    on_line: &mut dyn FnMut(&str) -> ControlFlow<()>,
) -> Result<(), CliError> {
    match (log_follow(client, lines, on_line), paths) {
        (Err(error), Some(paths)) if writes_no_log(&error) => {
            file_follow(&log_file(paths)?, lines, on_line)
        }
        (answer, _) => answer,
    }
}

/// Where the server log is on this machine: `[server] log_path`, read from
/// `config.toml` (the defaults when there is none), with a link at that path
/// resolved the way the server's own log resolves it.
pub fn log_file(paths: &DuxPaths) -> Result<PathBuf, CliError> {
    let config = crate::config::load_config(paths)
        .map_err(|error| CliError::new(Exit::Failed, error.to_string()))?;
    Ok(crate::logger::server_log_file(&config.server, paths))
}

fn unreadable(error: LogError) -> CliError {
    CliError::new(Exit::Failed, error.to_string())
}

/// The last `lines` lines of the log file, which may not exist yet, and the
/// sentence that says the answer was cut short, if it was.
pub fn file_tail(
    path: &std::path::Path,
    lines: usize,
) -> Result<(Vec<String>, Option<String>), CliError> {
    let (read, _) = open_log(path, None, lines, MAX_LINES).map_err(unreadable)?;
    Ok((read.lines, read.note))
}

/// The last `lines` lines of the log file, then every line written to it
/// after, across rotations, handed to `on_line` until it breaks. A link put
/// where the file is ends it with an error.
pub fn file_follow(
    path: &std::path::Path,
    lines: usize,
    on_line: &mut dyn FnMut(&str) -> ControlFlow<()>,
) -> Result<(), CliError> {
    let (first, mut follower) = open_log(path, None, lines, usize::MAX).map_err(unreadable)?;
    for line in first.lines {
        if on_line(&line).is_break() {
            return Ok(());
        }
    }
    loop {
        std::thread::sleep(crate::file_follow::FOLLOW_INTERVAL);
        for line in follower.poll() {
            if on_line(&line).is_break() {
                return Ok(());
            }
        }
        if follower.refused() {
            return Err(unreadable(LogError::Symlink(path.to_path_buf())));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::connect::{Target, connect};
    use crate::client::test_server::{FakeDux, Reply, private_dir};

    const BUILD: &str = r#"{"version":"v1","process":"p","api":1}"#;

    const CONNECTIONS: &str = r#"[
        {"id":"e1","surface":"browser",
         "device":"Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
         "address":"198.51.100.7","verified":false,"since":"2026-10-06T10:00:00Z","driving":true,
         "attachments":[{"kind":"tab","id":"t1","agent":"s1","driving":true},
                        {"kind":"terminal","id":"x2","driving":false}]},
        {"id":"terminal-ui","surface":"terminal_ui","device":"the dux TUI","address":null,
         "verified":false,"since":"2026-10-06T09:00:00Z","driving":false,"attachments":[]},
        {"id":"e3","surface":"browser","device":null,"address":"203.0.113.9","verified":true,
         "since":"2026-10-06T11:30:05Z","driving":false,"attachments":[]}
    ]"#;

    /// A stand-in local dux answering with `handler` beside its build, and a
    /// client of it.
    fn local_dux(
        dir: &std::path::Path,
        handler: impl Fn(&str) -> Reply + Send + Sync + 'static,
    ) -> (
        FakeDux,
        crate::lockfile::SingleInstanceLock,
        crate::client::connect::Client,
    ) {
        let socket = dir.join("dux.sock");
        let fake = FakeDux::unix(&socket, move |seen| match seen.path.as_str() {
            "/api/v1/build" => Reply::json(200, BUILD),
            other => handler(other),
        });
        let lock_path = dir.join("dux.lock");
        let lock = crate::lockfile::SingleInstanceLock::acquire(&lock_path).unwrap();
        std::fs::write(
            &lock_path,
            format!(
                "{}\ncontrol-socket={}\n",
                std::process::id(),
                socket.display()
            ),
        )
        .unwrap();
        let client = connect(&Target::Local, &lock_path).unwrap();
        (fake, lock, client)
    }

    /// A followed log's reply: these lines, `gap` apart, then its last chunk
    /// when `ends`, else the connection just closes.
    fn chunked(lines: &[&str], gap: std::time::Duration, ends: bool) -> Reply {
        let mut pieces = vec!["HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n".to_string()];
        for line in lines {
            pieces.push(format!("{:x}\r\n{line}\n\r\n", line.len() + 1));
        }
        if ends {
            pieces.push("0\r\n\r\n".to_string());
        }
        Reply::Slow(pieces, gap)
    }

    #[test]
    fn connections_print_in_the_three_shapes_with_each_address_marked_and_each_attachment_named() {
        let dir = private_dir();
        let (_fake, _lock, client) = local_dux(dir.path(), |path| match path {
            "/api/v1/server/connections" => Reply::json(200, CONNECTIONS),
            _ => Reply::json(404, "{}"),
        });
        assert_eq!(
            connections_ls(&client, Shape::Table).unwrap(),
            [
                "CONNECTION    DEVICE            ADDRESS                     SINCE                  ATTACHED\n",
                "e1            Chrome on Linux   198.51.100.7 (unverified)   2026-10-06T10:00:00Z   tab t1 (driving), terminal x2\n",
                "terminal-ui   the dux TUI       -                           2026-10-06T09:00:00Z   -\n",
                "e3            a browser         203.0.113.9                 2026-10-06T11:30:05Z   -\n",
            ]
            .concat()
        );
        assert_eq!(
            connections_ls(&client, Shape::Ids).unwrap(),
            "e1\nterminal-ui\ne3\n"
        );
        let json: serde_json::Value =
            serde_json::from_str(&connections_ls(&client, Shape::Json).unwrap()).unwrap();
        assert_eq!(json[0]["address"], "198.51.100.7");
        assert_eq!(json[0]["verified"], false);
        assert_eq!(json[0]["attachments"][0]["id"], "t1");
        assert_eq!(json[0]["attachments"][0]["driving"], true);
    }

    #[test]
    fn a_followed_log_hands_over_each_line_as_it_arrives_and_stops_when_told() {
        let dir = private_dir();
        let (fake, _lock, client) = local_dux(dir.path(), |path| {
            if path.starts_with("/api/v1/server/log") {
                chunked(
                    &["one", "two", "three", "four"],
                    std::time::Duration::from_millis(300),
                    false,
                )
            } else {
                Reply::json(404, "{}")
            }
        });
        let started = std::time::Instant::now();
        let mut seen = Vec::new();
        let ended = log_follow(&client, 7, &mut |line| {
            seen.push((started.elapsed(), line.to_string()));
            if line == "two" {
                ControlFlow::Break(())
            } else {
                ControlFlow::Continue(())
            }
        });
        assert_eq!(ended, Ok(()));
        assert_eq!(
            seen.iter()
                .map(|(_, line)| line.as_str())
                .collect::<Vec<_>>(),
            ["one", "two"]
        );
        assert!(
            seen[1].0 < std::time::Duration::from_millis(1500),
            "stopped before the server was done: {seen:?}"
        );
        assert!(
            fake.seen()
                .iter()
                .any(|request| request.path == "/api/v1/server/log?follow=true&lines=7"),
            "{:?}",
            fake.seen()
        );
    }

    #[test]
    fn a_log_stream_that_ends_by_itself_or_is_refused_says_why() {
        let dir = private_dir();
        let (_fake, _lock, client) = local_dux(dir.path(), |path| match path {
            "/api/v1/server/log?follow=true&lines=1" => {
                chunked(&["only"], std::time::Duration::ZERO, true)
            }
            "/api/v1/server/log?follow=true&lines=4" => Reply::Raw(format!(
                "HTTP/1.1 500 X\r\ncontent-length: 70000\r\n\r\n{}",
                "e".repeat(70_000)
            )),
            "/api/v1/server/log?follow=true&lines=5" => {
                let line = "a".repeat(1_500_000);
                Reply::Raw(format!(
                    "HTTP/1.1 200 OK\r\ntransfer-encoding: chunked\r\n\r\n{:x}\r\n{line}\r\n",
                    line.len()
                ))
            }
            "/api/v1/server/log?follow=true&lines=6" => {
                Reply::Raw("HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\n\r\n".to_string())
            }
            "/api/v1/server/log?follow=true&lines=3" => {
                chunked(&["cut"], std::time::Duration::ZERO, false)
            }
            "/api/v1/server/log?follow=true&lines=2" => Reply::json(409, r#"{"error":"no log"}"#),
            _ => Reply::json(404, "{}"),
        });
        let mut seen = Vec::new();
        let ended = log_follow(&client, 1, &mut |line| {
            seen.push(line.to_string());
            ControlFlow::Continue(())
        })
        .unwrap_err();
        assert_eq!(seen, ["only"]);
        assert_eq!(ended.exit, Exit::NotRunning);
        assert!(ended.message.contains("ended the log stream"), "{ended}");

        // Cut off rather than finished, it is the same to the person.
        let cut = log_follow(&client, 3, &mut |_| ControlFlow::Continue(())).unwrap_err();
        assert_eq!(cut.exit, Exit::NotRunning);

        // An error body past 64 KiB and a line past 1 MiB end the read with
        // one sentence, never echoing what they carried.
        let flood = log_follow(&client, 4, &mut |_| ControlFlow::Continue(())).unwrap_err();
        assert_eq!(flood.exit, Exit::Failed);
        assert!(flood.message.contains("64 KiB"), "{}", flood.message);
        assert!(flood.message.len() < 400, "{}", flood.message.len());
        let mut lines_seen = 0;
        let long = log_follow(&client, 5, &mut |_| {
            lines_seen += 1;
            ControlFlow::Continue(())
        })
        .unwrap_err();
        assert_eq!((long.exit, lines_seen), (Exit::Failed, 0));
        assert!(long.message.contains("1 MiB"), "{}", long.message);
        // A refusal with no body at all is still a refusal, not a dux that
        // went away.
        let bare = log_follow(&client, 6, &mut |_| ControlFlow::Continue(())).unwrap_err();
        assert_eq!(bare.exit, Exit::Failed);
        assert!(bare.message.contains("401"), "{}", bare.message);

        let refused = log_follow(&client, 2, &mut |_| ControlFlow::Continue(())).unwrap_err();
        assert_eq!(
            (refused.exit, refused.message.as_str()),
            (Exit::Refused, "no log")
        );
    }

    #[test]
    fn a_remote_that_asks_for_a_sign_in_says_so_even_when_its_refusal_has_no_body() {
        let (_fake, addr) = FakeDux::tcp(|seen| match seen.path.as_str() {
            "/api/v1/auth/status" => Reply::json(200, r#"{"required_here":false}"#),
            "/api/v1/build" => Reply::json(200, BUILD),
            _ => Reply::Raw("HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\n\r\n".into()),
        });
        let target = Target::Remote {
            name: "work".into(),
            remote: crate::client::remotes::Remote {
                url: format!("http://{addr}"),
                insecure: false,
                token: None,
            },
        };
        let client = connect(&target, std::path::Path::new("/nonexistent/dux.lock")).unwrap();
        let refused = log_follow(&client, 5, &mut |_| ControlFlow::Continue(())).unwrap_err();
        assert_eq!(refused.exit, Exit::PasswordNeeded);
        assert!(
            refused.message.contains("dux remote login work"),
            "{refused}"
        );
    }

    #[test]
    fn the_tail_of_a_running_dux_is_what_its_route_answers() {
        let dir = private_dir();
        let (_fake, _lock, client) = local_dux(dir.path(), |path| match path {
            "/api/v1/server/log?lines=2" => Reply::json(
                200,
                r#"{"lines":["a","b"],"cursor":"1.2:9","note":"cut short"}"#,
            ),
            _ => Reply::json(404, "{}"),
        });
        assert_eq!(
            log_tail(&client, 2).unwrap(),
            (
                vec!["a".to_string(), "b".to_string()],
                Some("cut short".to_string())
            )
        );
    }

    #[test]
    fn with_dux_stopped_the_log_is_the_file_the_config_names() {
        let dir = private_dir();
        let paths = DuxPaths {
            root: dir.path().to_path_buf(),
            config_path: dir.path().join("config.toml"),
            sessions_db_path: dir.path().join("sessions.sqlite3"),
            worktrees_root: dir.path().join("worktrees"),
            lock_path: dir.path().join("dux.lock"),
            socket_path: dir.path().join("dux.sock"),
        };
        assert_eq!(log_file(&paths).unwrap(), dir.path().join("server.log"));

        // A dux on this machine that writes no log (a terminal UI not serving)
        // leaves the file to be read, as with none running; for a remote
        // there is no file here to read, and its sentence is shown.
        std::fs::write(dir.path().join("server.log"), "from file\n").unwrap();
        let (_fake, _lock, client) = local_dux(dir.path(), |path| {
            if path.starts_with("/api/v1/server/log") {
                Reply::Raw(format!(
                    "HTTP/1.1 404 X\r\ncontent-length: {}\r\n\r\n{NO_SERVER_LOG}",
                    NO_SERVER_LOG.len()
                ))
            } else {
                Reply::json(404, "{}")
            }
        });
        assert_eq!(
            tail_or_file(&client, Some(&paths), 5).unwrap(),
            (vec!["from file".to_string()], None)
        );
        let mut seen = Vec::new();
        follow_or_file(&client, Some(&paths), 5, &mut |line| {
            seen.push(line.to_string());
            ControlFlow::Break(())
        })
        .unwrap();
        assert_eq!(seen, ["from file"]);
        let shown = tail_or_file(&client, None, 5).unwrap_err();
        assert_eq!(shown.message, NO_SERVER_LOG);
        let shown = follow_or_file(&client, None, 5, &mut |_| ControlFlow::Break(())).unwrap_err();
        assert_eq!(shown.message, NO_SERVER_LOG);
        std::fs::write(
            &paths.config_path,
            "[server]\nlog_path = \"logs/web.log\"\n",
        )
        .unwrap();
        let named = log_file(&paths).unwrap();
        assert_eq!(named, dir.path().join("logs/web.log"));

        assert!(file_tail(&named, 2).unwrap().0.is_empty(), "no file yet");
        std::fs::create_dir_all(named.parent().unwrap()).unwrap();
        std::fs::write(&named, "one\ntwo\nthree\n").unwrap();
        assert_eq!(file_tail(&named, 2).unwrap().0, ["two", "three"]);

        let mut seen = Vec::new();
        file_follow(&named, 1, &mut |line| {
            seen.push(line.to_string());
            ControlFlow::Break(())
        })
        .unwrap();
        assert_eq!(seen, ["three"]);

        // A link put where the log was is refused, not read.
        let secret = dir.path().join("secret.txt");
        std::fs::write(&secret, "secret\n").unwrap();
        std::fs::remove_file(&named).unwrap();
        std::os::unix::fs::symlink(&secret, &named).unwrap();
        let refused = file_tail(&named, 2).unwrap_err();
        assert_eq!(refused.exit, Exit::Failed);
        assert!(refused.message.contains("symbolic link"), "{refused}");
    }
}
