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

/// How many lines `dux server logs` shows when `--lines` does not say.
pub const DEFAULT_LINES: usize = 100;

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
}

/// The last `lines` lines of a running dux's server log.
pub fn log_tail(client: &Client, lines: usize) -> Result<Vec<String>, CliError> {
    let read: LogRead = client.get_json(&format!("{LOG_ROUTE}?lines={lines}"))?;
    Ok(read.lines)
}

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
    let mut pending: Vec<u8> = Vec::new();
    let mut stopped = false;
    client.stream(
        Request::get(format!("{LOG_ROUTE}?follow=true&lines={lines}")),
        &mut |code, piece| {
            status = code;
            if code != 200 {
                refusal.extend_from_slice(piece);
                return ControlFlow::Continue(());
            }
            pending.extend_from_slice(piece);
            while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                let line: Vec<u8> = pending.drain(..=end).collect();
                let text = String::from_utf8_lossy(&line[..end]);
                if on_line(text.trim_end_matches('\r')).is_break() {
                    stopped = true;
                    return ControlFlow::Break(());
                }
            }
            ControlFlow::Continue(())
        },
    )?;
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

/// Where the server log is on this machine: `[server] log_path`, read from
/// `config.toml`, which is the defaults when there is none.
pub fn log_file(paths: &DuxPaths) -> Result<PathBuf, CliError> {
    let config = crate::config::load_config(paths)
        .map_err(|error| CliError::new(Exit::Failed, error.to_string()))?;
    Ok(crate::logger::resolve_server_log_path(
        &config.server,
        paths,
    ))
}

/// The last `lines` lines of the log file, which may not exist yet.
pub fn file_tail(path: &std::path::Path, lines: usize) -> Vec<String> {
    crate::file_follow::open_log(path, None, lines).0
}

/// The last `lines` lines of the log file, then every line written to it
/// after, across rotations, handed to `on_line` until it breaks.
pub fn file_follow(
    path: &std::path::Path,
    lines: usize,
    on_line: &mut dyn FnMut(&str) -> ControlFlow<()>,
) {
    let (first, _, mut follower) = crate::file_follow::open_log(path, None, lines);
    for line in first {
        if on_line(&line).is_break() {
            return;
        }
    }
    loop {
        std::thread::sleep(crate::file_follow::FOLLOW_INTERVAL);
        for line in follower.poll() {
            if on_line(&line).is_break() {
                return;
            }
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

        let refused = log_follow(&client, 2, &mut |_| ControlFlow::Continue(())).unwrap_err();
        assert_eq!(
            (refused.exit, refused.message.as_str()),
            (Exit::Refused, "no log")
        );
    }

    #[test]
    fn the_tail_of_a_running_dux_is_what_its_route_answers() {
        let dir = private_dir();
        let (_fake, _lock, client) = local_dux(dir.path(), |path| match path {
            "/api/v1/server/log?lines=2" => Reply::json(200, r#"{"lines":["a","b"],"cursor":9}"#),
            _ => Reply::json(404, "{}"),
        });
        assert_eq!(log_tail(&client, 2).unwrap(), ["a", "b"]);
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
        std::fs::write(
            &paths.config_path,
            "[server]\nlog_path = \"logs/web.log\"\n",
        )
        .unwrap();
        let named = log_file(&paths).unwrap();
        assert_eq!(named, dir.path().join("logs/web.log"));

        assert!(file_tail(&named, 2).is_empty(), "no file yet, no lines");
        std::fs::create_dir_all(named.parent().unwrap()).unwrap();
        std::fs::write(&named, "one\ntwo\nthree\n").unwrap();
        assert_eq!(file_tail(&named, 2), ["two", "three"]);

        let mut seen = Vec::new();
        file_follow(&named, 1, &mut |line| {
            seen.push(line.to_string());
            ControlFlow::Break(())
        });
        assert_eq!(seen, ["three"]);
    }
}
