//! A name that is not a setting name (here a token pasted where a key goes)
//! must never be printed or logged without `--show`.

const TOKEN: &str = "sk-proj-AbCdEf0123456789";

fn paths(dir: &std::path::Path) -> dux_core::config::DuxPaths {
    dux_core::config::DuxPaths {
        root: dir.to_path_buf(),
        config_path: dir.join("config.toml"),
        sessions_db_path: dir.join("sessions.sqlite3"),
        worktrees_root: dir.join("worktrees"),
        lock_path: dir.join("dux.lock"),
    }
}

/// A token pasted as a key inside an `[env]` value written as a table: the
/// start checks name the entry by it, so it reaches the terminal UI's start
/// error and every `dux config set`/`get` that lists the problem.
#[test]
fn a_token_pasted_as_a_key_inside_an_env_value_is_not_quoted_by_the_start_checks() {
    let raw = format!("[env]\nA = {{ \"{TOKEN} x\" = 1 }}\n");
    let problems = dux_core::config::start_problems_of(&raw);
    assert!(
        !problems.is_empty(),
        "precondition: it stops the terminal UI"
    );
    for p in &problems {
        assert!(
            !p.message.contains(TOKEN),
            "start check quotes the token: {}",
            p.message
        );
    }
}

/// A token pasted as a key under `[defaults]`, holding a misplaced password
/// hash: the refusal both surfaces start with quotes it.
#[test]
fn a_token_pasted_as_a_key_holding_a_misplaced_hash_is_not_quoted_by_the_start_checks() {
    let raw = format!("[defaults]\n\"{TOKEN}\" = {{ password_hash = \"x\" }}\n");
    let problems = dux_core::config::start_problems_of(&raw);
    assert!(!problems.is_empty(), "precondition: both surfaces refuse");
    for p in &problems {
        assert!(
            !p.message.contains(TOKEN),
            "start check quotes the token: {}",
            p.message
        );
    }
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    std::fs::write(&paths.config_path, &raw).unwrap();
    let error = format!(
        "{:#}",
        dux_core::config::load_config(&paths).expect_err("refused")
    );
    assert!(
        !error.contains(TOKEN),
        "dux server's start error quotes the token: {error}"
    );
}
