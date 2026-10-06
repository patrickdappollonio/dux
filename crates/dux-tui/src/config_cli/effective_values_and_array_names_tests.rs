//! Effective values, fixed-value settings, missing-field attribution and names inside arrays.
use super::*;

fn paths_with(body: &str) -> (tempfile::TempDir, DuxPaths) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let paths = DuxPaths {
        root: tmp.path().to_path_buf(),
        config_path: tmp.path().join("config.toml"),
        sessions_db_path: tmp.path().join("sessions.sqlite3"),
        worktrees_root: tmp.path().join("worktrees"),
        lock_path: tmp.path().join("dux.lock"),
        socket_path: tmp.path().join("dux.sock"),
    };
    std::fs::write(&paths.config_path, body).expect("seed");
    (tmp, paths)
}

fn get(body: &str, key: &str) -> (String, String) {
    let (_t, paths) = paths_with(body);
    let (mut out, mut err) = (Vec::new(), Vec::new());
    run_get(&[key.to_string()], &paths, &mut out, &mut err).expect("get");
    (
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

/// dux's logger reads any level other than debug/warn/error as info, so a
/// file saying `level = "DEBUG"` logs at info. `get` must report what is used.
#[test]
fn get_logging_level_reports_the_level_dux_logs_at() {
    crate::config::install_canonical_renderer();
    let (out, err) = get("[logging]\nlevel = \"DEBUG\"\n", "logging.level");
    assert!(
        out.contains("info") || err.contains("info"),
        "dux logs at info, but get says: out={out:?} err={err:?}"
    );
}

/// The terminal UI sorts an unknown `agent_sort` as `active`.
#[test]
fn get_agent_sort_reports_the_sort_the_terminal_ui_uses() {
    crate::config::install_canonical_renderer();
    let (out, err) = get("[ui]\nagent_sort = \"bogus\"\n", "ui.agent_sort");
    assert!(
        out.contains("active") || err.contains("active"),
        "the terminal UI sorts by active, but get says: out={out:?} err={err:?}"
    );
}

/// `dux config set` refuses an unknown value for every enum-like setting
/// it validates (agent_sort, compose_bar, tailscale, ...), but writes any
/// string to logging.level, which dux then silently reads as info.
#[test]
fn set_logging_level_refuses_a_level_dux_does_not_know() {
    crate::config::install_canonical_renderer();
    struct NoSecrets;
    impl SecretSource for NoSecrets {
        fn read_stdin(&mut self) -> Result<Password> {
            bail!("no")
        }
        fn prompt_twice(
            &mut self,
            _: &str,
            _: Option<Meter<'_>>,
        ) -> Result<Option<(Password, Password)>> {
            Ok(None)
        }
    }
    let (_t, paths) = paths_with("");
    let mut out = Vec::new();
    let result = run_set(
        &["logging.level".to_string(), "loud".to_string()],
        &paths,
        &mut NoSecrets,
        &mut out,
    );
    assert!(
        result.is_err(),
        "set accepted logging.level = loud: {}",
        String::from_utf8_lossy(&out)
    );
}

/// A macro missing its `surface` is refused by the terminal UI. The reasons
/// printed blame `text`, which is present and well typed, and never name
/// `surface`, the field actually missing.
#[test]
fn a_macro_missing_its_surface_is_blamed_on_the_right_field() {
    crate::config::install_canonical_renderer();
    let body = "[macros.greet]\ntext = \"hi\"\n";
    let problems = dux_core::config::start_problems_of(body);
    let messages: Vec<&str> = problems.iter().map(|p| p.message.as_str()).collect();
    assert!(
        messages.iter().any(|m| m.contains("surface")),
        "no start problem names the missing field `surface`: {messages:?}"
    );
    assert!(
        !messages.iter().any(|m| m.contains("greet.text")),
        "a start problem blames greet.text, which is fine: {messages:?}"
    );
    let tmp = tempfile::tempdir().unwrap();
    let (_t, paths) = paths_with(body);
    drop(tmp);
    let error = format!(
        "{:#}",
        crate::config::ensure_config(&paths).expect_err("the terminal UI refuses it")
    );
    assert!(
        error.contains("surface") && !error.contains("greet.text"),
        "the terminal UI's refusal blames the wrong field: {error}"
    );
}

/// dux clamps shutdown_timeout_seconds to 600 (see `shutdown_grace`), the
/// same kind of in-memory correction `get` reports for terminal_font_size.
#[test]
fn get_shutdown_timeout_reports_the_clamped_wait_dux_uses() {
    crate::config::install_canonical_renderer();
    assert_eq!(
        dux_core::config::shutdown_grace(9999),
        std::time::Duration::from_secs(600),
        "precondition: dux waits 600 seconds"
    );
    let (out, err) = get(
        "shutdown_timeout_seconds = 9999\n",
        "shutdown_timeout_seconds",
    );
    assert!(
        out.contains("600") || err.contains("600"),
        "dux waits 600 seconds, but get says: out={out:?} err={err:?}"
    );
}

/// A name inside an array (an array of tables where the schema has a table,
/// or a table inside a list setting) is no schema key at that position, so
/// `get` without `--show` must place it by its line, never print it.
#[test]
fn get_never_prints_a_name_inside_an_array_without_show() {
    crate::config::install_canonical_renderer();
    const TOKEN: &str = "sk-proj-AbCdEf0123456789";
    let mut leaked = Vec::new();
    for (body, key) in [
        (
            format!("[server]\nauth = [{{ \"{TOKEN}\" = 1 }}]\n"),
            "server",
        ),
        (format!("[[ui]]\n\"{TOKEN}\" = 1\n"), "ui"),
        (format!("[[providers]]\n\"{TOKEN}\" = 1\n"), "providers"),
        (
            format!("[server]\nallowed_hosts = [{{ \"{TOKEN}\" = 1 }}]\n"),
            "server.allowed_hosts",
        ),
    ] {
        let (out, err) = get(&body, key);
        if out.contains(TOKEN) || err.contains(TOKEN) {
            leaked.push(format!(
                "get {key} on {body:?}:\n  out={out:?}\n  err={err:?}"
            ));
        }
    }
    assert!(leaked.is_empty(), "{}", leaked.join("\n"));
}
