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
    let out = Command::new(env!("CARGO_BIN_EXE_dux"))
        .args(args)
        .env("DUX_HOME", home(test))
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
fn resource_commands_are_declared_but_not_built_yet() {
    for args in [
        &["projects", "ls"][..],
        &["projects", "list"][..],
        &["projects", "worktrees", "ls", "p"][..],
        &["agents", "rm", "a"][..],
        &["agents", "remove", "a", "--yes"][..],
        &["agents", "tabs", "stop", "a", "t"][..],
        &["terminals", "ls", "-q"][..],
        &["macros", "show", "m"][..],
        &["env", "ls", "--show"][..],
        &["remote", "default", "--unset"][..],
        &["operations", "show", "op1"][..],
        &["server", "logs", "-f"][..],
        &["server", "connections", "ls"][..],
    ] {
        let run = dux("not-built", args);
        assert_eq!(run.code(), 2, "{args:?}: {}", run.stderr());
        assert_eq!(run.stderr(), "this command is not built yet\n", "{args:?}");
        assert_eq!(run.stdout(), "", "{args:?}");
    }
}

#[test]
fn config_refuses_an_explicit_remote_and_touches_nothing() {
    let dir = home("config-remote");
    let out = Command::new(env!("CARGO_BIN_EXE_dux"))
        .args(["--remote", "box", "config", "set", "server.port", "4000"])
        .env("DUX_HOME", &dir)
        .stdin(Stdio::null())
        .output()
        .unwrap();
    let run = Run { out };
    assert_eq!(run.code(), 2);
    assert_eq!(
        run.stderr(),
        "dux config edits this machine's config.toml; run \"dux --local config …\" to go ahead, or unset DUX_REMOTE\n"
    );
    assert_eq!(run.stdout(), "");
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
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
