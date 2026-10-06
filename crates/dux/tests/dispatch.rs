//! Runs the built `dux` binary through its real command dispatch. Every run
//! gets its own empty `DUX_HOME`, so nothing here touches a real config folder,
//! and `stdin` is closed so a command that wrongly fell through to the terminal
//! UI fails instead of waiting for a keyboard.

use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

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

fn home(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dux-dispatch-{}-{test}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn dux(test: &str, args: &[&str]) -> Run {
    dux_in(&home(test), args, &[])
}

/// Run dux on `dir` as it stands, with `env` set and no `DUX_REMOTE` from
/// the shell running the tests.
fn dux_in(dir: &std::path::Path, args: &[&str], env: &[(&str, &str)]) -> Run {
    let out = Command::new(env!("CARGO_BIN_EXE_dux"))
        .args(args)
        .env("DUX_HOME", dir)
        .env_remove("DUX_REMOTE")
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .output()
        .expect("the dux binary runs");
    Run { out }
}

#[test]
fn version_prints_the_version_and_exits_zero() {
    let run = dux("version", &["--version"]);
    assert_eq!(run.code(), 0, "{}", run.stderr());
    assert!(run.stdout().starts_with("dux 0.1.0"), "{}", run.stdout());
}

#[test]
fn an_unknown_top_level_command_is_a_usage_error() {
    let run = dux("unknown", &["frobnicate"]);
    assert_eq!(run.code(), 2);
    assert!(run.stderr().contains("frobnicate"), "{}", run.stderr());
    assert!(run.stderr().contains("Usage:"), "{}", run.stderr());
}

#[test]
fn an_unknown_config_subcommand_keeps_its_message_and_exit_one() {
    let run = dux("config-bogus", &["config", "bogus"]);
    assert_eq!(run.code(), 1);
    assert_eq!(
        run.stderr(),
        "Error: unknown config subcommand: bogus\nRun `dux config --help` for usage.\n"
    );
}

#[test]
fn config_set_reaches_the_config_codes_own_refusal_for_a_negative_value() {
    let run = dux(
        "config-negative",
        &["config", "set", "ui.left_width_pct", "-1"],
    );
    assert_eq!(run.code(), 1);
    assert_eq!(
        run.stderr(),
        "Error: ui.left_width_pct cannot be -1: invalid value: integer `-1`, expected u16\n"
    );
}

#[test]
fn config_set_with_an_unknown_flag_gives_the_config_codes_flag_message() {
    let run = dux(
        "config-flag",
        &["config", "set", "server.port", "4000", "--bogus"],
    );
    assert_eq!(run.code(), 1);
    assert_eq!(
        run.stderr(),
        "Error: unknown flag (not repeated, in case it holds a value): `dux config get` takes --show and `dux config set` takes --stdin\n"
    );
}

#[test]
fn config_help_is_the_config_codes_own_help() {
    let run = dux("config-help", &["config", "--help"]);
    assert_eq!(run.code(), 0);
    assert!(
        run.stdout()
            .starts_with("dux config: manage the dux configuration file"),
        "{}",
        run.stdout()
    );
}

#[test]
fn a_nested_command_prints_its_own_help() {
    let run = dux("tabs-help", &["agents", "tabs", "ls", "--help"]);
    assert_eq!(run.code(), 0, "{}", run.stderr());
    assert!(
        run.stdout().contains("Usage: dux agents tabs ls"),
        "{}",
        run.stdout()
    );
}

#[test]
fn top_level_help_lists_the_commands_and_the_config_folder_variable() {
    let run = dux("top-help", &["--help"]);
    assert_eq!(run.code(), 0);
    let help = run.stdout();
    for needle in [
        "server",
        "config",
        "projects",
        "agents",
        "terminals",
        "macros",
        "operations",
        "--remote",
        "--local",
        "DUX_HOME",
    ] {
        assert!(help.contains(needle), "missing {needle}:\n{help}");
    }
}

#[test]
fn server_help_describes_the_server_and_its_flags() {
    let run = dux("server-help", &["server", "-h"]);
    assert_eq!(run.code(), 0);
    let help = run.stdout();
    for needle in [
        "--bind",
        "--port",
        "--no-tailscale",
        "dux config set server.auth.password",
    ] {
        assert!(help.contains(needle), "missing {needle}:\n{help}");
    }
}

#[test]
fn server_flags_are_checked_before_anything_starts() {
    for args in [
        &["server", "--port", "notaport"][..],
        &["server", "--bind", "a:1", "--bind", "b:2"][..],
        &["server", "--what-is-this"][..],
        &["server", "--port", "9", "logs"][..],
    ] {
        let run = dux("server-bad", args);
        assert_eq!(run.code(), 2, "{args:?}: {}", run.stderr());
    }
}

#[test]
fn remote_and_local_cannot_be_combined() {
    let run = dux(
        "remote-local",
        &["--local", "--remote", "box", "projects", "ls"],
    );
    assert_eq!(run.code(), 2);
}

#[test]
fn resource_commands_answer_with_no_dux_running_and_never_create_the_database() {
    const NOT_BUILT: &str = "this command is not built yet\n";
    const NOT_RUNNING: &str = "dux isn't running; start it with \"dux\" or \"dux server\"\n";
    for (args, code, stderr) in [
        (&["projects", "ls"][..], 2, NOT_BUILT),
        (&["projects", "list"][..], 2, NOT_BUILT),
        (&["projects", "worktrees", "ls", "p"][..], 2, NOT_BUILT),
        (&["agents", "rm", "a"][..], 2, NOT_BUILT),
        (&["agents", "remove", "a", "--yes"][..], 2, NOT_BUILT),
        (&["agents", "tabs", "stop", "a", "t"][..], 2, NOT_BUILT),
        (&["terminals", "ls", "-q"][..], 2, NOT_BUILT),
        (&["macros", "show", "m"][..], 2, NOT_BUILT),
        (&["env", "ls", "--show"][..], 2, NOT_BUILT),
        (&["server", "logs", "-f"][..], 2, NOT_BUILT),
        (&["server", "connections", "ls"][..], 2, NOT_BUILT),
        (&["operations", "show", "op1"][..], 4, NOT_RUNNING),
    ] {
        let dir = home("no-dux");
        let run = dux_in(&dir, args, &[]);
        assert_eq!(run.code(), code, "{args:?}: {}", run.stderr());
        assert_eq!(run.stderr(), stderr, "{args:?}");
        assert_eq!(run.stdout(), "", "{args:?}");
        assert!(!dir.join("sessions.sqlite3").exists(), "{args:?}");
    }
}

#[test]
fn remotes_are_saved_listed_made_default_and_forgotten() {
    let dir = home("remotes");
    let run = dux_in(
        &dir,
        &["remote", "add", "lan", "http://192.168.1.20:3890"],
        &[],
    );
    assert_eq!(run.code(), 2);
    assert!(run.stderr().contains("--insecure"), "{}", run.stderr());
    assert!(!dir.join("remotes.toml").exists());

    let run = dux_in(
        &dir,
        &["remote", "add", "home", "http://127.0.0.1:3890/"],
        &[],
    );
    assert_eq!(run.code(), 0, "{}", run.stderr());
    let run = dux_in(
        &dir,
        &[
            "remote",
            "add",
            "lan",
            "http://192.168.1.20:3890",
            "--insecure",
        ],
        &[],
    );
    assert_eq!(run.code(), 0, "{}", run.stderr());
    let run = dux_in(&dir, &["remote", "default", "lan"], &[]);
    assert_eq!(run.code(), 0, "{}", run.stderr());

    let run = dux_in(&dir, &["remote", "ls", "-q"], &[]);
    assert_eq!(run.stdout(), "home\nlan\n");
    let run = dux_in(&dir, &["remote", "list", "--format", "json"], &[]);
    let listed: serde_json::Value = serde_json::from_str(&run.stdout()).expect("one array");
    assert_eq!(listed[0]["url"], "http://127.0.0.1:3890");
    assert_eq!(listed[1]["default"], true);
    assert_eq!(listed[1]["insecure"], true);

    let run = dux_in(&dir, &["remote", "rm", "lan"], &[]);
    assert_eq!(run.code(), 0, "{}", run.stderr());
    let run = dux_in(&dir, &["remote", "ls"], &[]);
    assert_eq!(run.stdout().lines().count(), 2, "{}", run.stdout());
    assert!(run.stdout().starts_with("NAME "), "{}", run.stdout());
    let saved = std::fs::read_to_string(dir.join("remotes.toml")).unwrap();
    assert!(!saved.contains("default"), "{saved}");
}

#[test]
fn config_refuses_a_selected_remote_and_touches_nothing() {
    let refusal = "dux config edits this machine's config.toml; run \"dux --local config …\" to go ahead, or unset DUX_REMOTE\n";
    let saved_default =
        "default = \"work\"\n\n[remotes.work]\nurl = \"https://work.example.com\"\n";
    for (case, args, env, remotes) in [
        (
            "flag",
            &["--remote", "box", "config", "set", "server.port", "4000"][..],
            &[][..],
            None,
        ),
        (
            "variable",
            &["config", "set", "ui.theme", "x"][..],
            &[("DUX_REMOTE", "work")][..],
            None,
        ),
        (
            "default",
            &["config", "set", "ui.theme", "x"][..],
            &[][..],
            Some(saved_default),
        ),
    ] {
        let dir = home(&format!("config-remote-{case}"));
        if let Some(text) = remotes {
            std::fs::write(dir.join("remotes.toml"), text).unwrap();
        }
        let run = dux_in(&dir, args, env);
        assert_eq!(run.code(), 2, "{case}: {}", run.stderr());
        assert_eq!(run.stderr(), refusal, "{case}");
        assert_eq!(run.stdout(), "", "{case}");
        assert!(!dir.join("config.toml").exists(), "{case}");
    }
    let dir = home("config-remote-local");
    let run = dux_in(
        &dir,
        &["--local", "config", "set", "ui.theme", "dux_dark"],
        &[("DUX_REMOTE", "work")],
    );
    assert_eq!(run.code(), 0, "{}", run.stderr());
    assert!(dir.join("config.toml").exists());
}

#[test]
fn config_with_local_runs_as_normal() {
    let run = dux("config-local", &["--local", "config", "path"]);
    assert_eq!(run.code(), 0, "{}", run.stderr());
    assert!(run.stdout().ends_with("config.toml\n"), "{}", run.stdout());
}

#[test]
fn global_flags_work_beside_a_server_subcommand() {
    let run = dux("server-remote-logs", &["server", "--remote", "box", "logs"]);
    assert_eq!(run.stderr(), "this command is not built yet\n");
    assert_eq!(run.code(), 2);
}

#[test]
fn macros_and_env_changes_are_declared_but_not_built_yet() {
    for args in [
        &["macros", "add", "m"][..],
        &["macros", "rm", "m"][..],
        &["macros", "remove", "m"][..],
        &["env", "set", "TOKEN", "--stdin"][..],
        &["env", "rm", "TOKEN"][..],
    ] {
        let run = dux("not-built-edits", args);
        assert_eq!(run.code(), 2, "{args:?}: {}", run.stderr());
        assert_eq!(run.stderr(), "this command is not built yet\n", "{args:?}");
    }
}

#[test]
fn config_arguments_that_look_like_dux_flags_reach_the_config_code_untouched() {
    for (args, word) in [
        (&["config", "--"][..], "--"),
        (&["config", "--", "path"][..], "--"),
        (&["config", "--local", "path"][..], "--local"),
        (&["config", "--remote=x", "path"][..], "--remote=x"),
    ] {
        let run = dux("config-raw", args);
        assert_eq!(run.code(), 1, "{args:?}");
        assert_eq!(
            run.stderr(),
            format!(
                "Error: unknown config subcommand: {word}\nRun `dux config --help` for usage.\n"
            ),
            "{args:?}"
        );
        assert_eq!(run.stdout(), "", "{args:?}");
    }
}

#[test]
fn a_help_flag_inside_a_config_command_prints_the_config_help() {
    for args in [
        &["config", "get", "--help"][..],
        &["config", "set", "x", "y", "--help"][..],
        &["config", "diff", "--raw", "--help"][..],
        &["config", "set", "k", "v", "-h"][..],
    ] {
        let run = dux("config-inner-help", args);
        assert_eq!(run.code(), 0, "{args:?}: {}", run.stderr());
        assert!(
            run.stdout()
                .starts_with("dux config: manage the dux configuration file"),
            "{args:?}: {}",
            run.stdout()
        );
    }
}

#[test]
fn global_flags_before_config_still_apply_to_it() {
    let run = dux("config-global-before", &["--local", "config", "path"]);
    assert_eq!(run.code(), 0, "{}", run.stderr());
    let run = dux("config-global-before", &["--remote=box", "config", "path"]);
    assert_eq!(run.code(), 2);
}

#[test]
fn a_config_reset_refuses_while_another_dux_holds_the_lock_and_touches_nothing() {
    for (name, args) in [
        ("reset-locked", vec!["config", "reset"]),
        ("reset-all-locked", vec!["config", "reset", "--all"]),
    ] {
        let dir = home(name);
        let _lock = dux_core::lockfile::SingleInstanceLock::acquire(&dir.join("dux.lock"))
            .expect("this test plays the running dux");
        std::fs::write(dir.join("config.toml"), "# mine\n").unwrap();
        std::fs::write(dir.join("sessions.sqlite3"), "not really a database").unwrap();
        let out = Command::new(env!("CARGO_BIN_EXE_dux"))
            .args(&args)
            .env("DUX_HOME", &dir)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        let run = Run { out };
        assert_eq!(run.code(), 1, "{name}: {}", run.stderr());
        assert!(
            run.stderr()
                .contains("Another dux instance is already running"),
            "{name}: {}",
            run.stderr()
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("config.toml")).unwrap(),
            "# mine\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("sessions.sqlite3")).unwrap(),
            "not really a database"
        );
    }
}

/// Answer each request on a socket in `dir` from `reply`, as a running dux
/// would, and hold `dux.lock` naming that socket.
fn serve_as_dux(
    dir: &std::path::Path,
    reply: fn(&str) -> (u16, &'static str),
) -> dux_core::lockfile::SingleInstanceLock {
    use std::io::{BufRead, BufReader, Write};
    let socket = dir.join("dux.sock");
    let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            let mut reader = BufReader::new(stream);
            let mut request_line = String::new();
            let _ = reader.read_line(&mut request_line);
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
                    break;
                }
            }
            let path = request_line.split_whitespace().nth(1).unwrap_or_default();
            let (status, body) = match path {
                "/api/v1/build" => (200, r#"{"version":"v1","process":"p","api":1}"#),
                other => reply(other),
            };
            let _ = write!(
                reader.get_mut(),
                "HTTP/1.1 {status} X\r\ncontent-length: {}\r\n\r\n{body}",
                body.len()
            );
        }
    });
    let lock = dux_core::lockfile::SingleInstanceLock::acquire(&dir.join("dux.lock")).unwrap();
    std::fs::write(
        dir.join("dux.lock"),
        format!(
            "{}\ncontrol-socket={}\n",
            std::process::id(),
            socket.display()
        ),
    )
    .unwrap();
    lock
}

#[test]
fn an_operation_is_shown_and_exits_with_the_code_of_its_outcome() {
    let dir = home("operations-show");
    let _dux = serve_as_dux(&dir, |path| match path {
        "/api/v1/operations/op-ok" => (
            200,
            r#"{"id":"op-ok","kind":"tab.close","state":"succeeded","message":"Closed the tab.","created":[],"removed":[],"parts":[]}"#,
        ),
        "/api/v1/operations/op-part" => (
            200,
            r#"{"id":"op-part","kind":"agent.delete","state":"partial","message":"Deleted web; kept its branch.","created":[],"removed":[],"parts":[]}"#,
        ),
        "/api/v1/operations/op-run" => (
            200,
            r#"{"id":"op-run","kind":"agent.delete","state":"running","message":"","created":[],"removed":[],"parts":[]}"#,
        ),
        _ => (404, r#"{"error":"unknown_operation"}"#),
    });
    for (id, code, state) in [
        ("op-ok", 0, "succeeded"),
        ("op-part", 1, "partial"),
        ("op-run", 6, "running"),
    ] {
        let run = dux_in(&dir, &["operations", "show", id], &[]);
        assert_eq!(run.code(), code, "{id}: {}", run.stderr());
        assert!(run.stdout().contains(id), "{id}: {}", run.stdout());
        assert!(run.stdout().contains(state), "{id}: {}", run.stdout());
    }
    let run = dux_in(&dir, &["operations", "show", "op-gone"], &[]);
    assert_eq!(run.code(), 1);
    assert!(run.stderr().contains("op-gone"), "{}", run.stderr());
}
