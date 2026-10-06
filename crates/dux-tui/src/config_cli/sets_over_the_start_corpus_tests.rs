//! `dux config set` over every file of the start corpus, for a fixed list of
//! sets, checked against each surface's real start: a set never makes a
//! surface that started with the file refuse it, what the set says about
//! each surface matches that surface's start, and a set is never refused on
//! a file when it is accepted on a clean one.
use super::*;

/// Sets of plain settings across sections, providers included.
const SETS: &[(&str, &str)] = &[
    ("ui.left_width_pct", "30"),
    ("ui.terminal_font_size", "16"),
    ("server.port", "4000"),
    ("server.host", "127.0.0.1"),
    ("server.serve_while_tui", "true"),
    ("server.tailscale", "no"),
    ("logging.level", "debug"),
    ("providers.claude.install_hint", "x"),
];

struct NoSecrets;
impl SecretSource for NoSecrets {
    fn read_stdin(&mut self) -> Result<Password> {
        bail!("no stdin")
    }
    fn prompt_twice(
        &mut self,
        _: &str,
        _: Option<Meter<'_>>,
    ) -> Result<Option<(Password, Password)>> {
        Ok(None)
    }
}

fn paths_in(dir: &std::path::Path) -> DuxPaths {
    DuxPaths {
        root: dir.to_path_buf(),
        config_path: dir.join("config.toml"),
        sessions_db_path: dir.join("sessions.sqlite3"),
        worktrees_root: dir.join("worktrees"),
        lock_path: dir.join("dux.lock"),
        socket_path: dir.join("dux.sock"),
    }
}

/// Whether the terminal UI starts with `text`, as its start runs.
fn terminal_ui_starts(text: &str) -> bool {
    let tmp = tempfile::tempdir().expect("tempdir");
    let paths = paths_in(tmp.path());
    std::fs::write(&paths.config_path, text).expect("seed");
    match crate::config::ensure_config(&paths) {
        Err(_) => false,
        Ok(config) => {
            crate::config::validate_keys(&config.keys).is_ok()
                && dux_core::config_sync::validate_project_records("config.toml", &config.projects)
                    .is_ok()
        }
    }
}

/// Whether `dux server` starts with `text`, with no `--bind` or `--port`:
/// its load, its listener plan and its start checks.
fn dux_server_starts(text: &str) -> bool {
    let tmp = tempfile::tempdir().expect("tempdir");
    let paths = paths_in(tmp.path());
    std::fs::write(&paths.config_path, text).expect("seed");
    match dux_core::config::load_config(&paths) {
        Err(_) => false,
        Ok(config) => {
            dux_core::config::resolve_server_plan(&config.server, &Default::default(), None).is_ok()
                && dux_core::config::start_refusal(text, dux_core::config::Surface::DuxServer)
                    .is_none()
        }
    }
}

/// `dux config set key value` on a file holding `text`: the outcome, what
/// it said, and the file after.
fn set_on(text: &str, key: &str, value: &str) -> (Result<()>, String, String) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let paths = paths_in(tmp.path());
    std::fs::write(&paths.config_path, text).expect("seed");
    let mut out = Vec::new();
    let result = run_set(
        &[key.to_string(), "--".to_string(), value.to_string()],
        &paths,
        &mut NoSecrets,
        &mut out,
    );
    let after = std::fs::read_to_string(&paths.config_path).unwrap_or_default();
    (result, String::from_utf8(out).expect("utf-8"), after)
}

/// What is wrong with every set of [`SETS`] on one corpus case.
fn wrong_sets_on(case: &dux_core::start_check_fixtures::CorpusCase, clean: &[bool]) -> Vec<String> {
    let mut wrong = Vec::new();
    let text = &case.text;
    let (tui_before, server_before) = (case.terminal_ui_starts, case.dux_server_starts);
    for ((key, value), accepted_on_a_clean_file) in SETS.iter().zip(clean) {
        let (result, said, after) = set_on(text, key, value);
        if let Err(error) = result {
            // A set cannot write inside a value that is not a table, and
            // says so; anything else refused here is accepted on a clean file.
            let message = format!("{error:#}");
            if *accepted_on_a_clean_file
                && tui_before
                && server_before
                && !message.contains("not a table")
            {
                wrong.push(format!(
                    "{text:?}: `set {key} {value}` refused on a file both surfaces start with: \
                     {message}"
                ));
            }
            continue;
        }
        let (tui_after, server_after) = (terminal_ui_starts(&after), dux_server_starts(&after));
        if (tui_before && !tui_after) || (server_before && !server_after) {
            wrong.push(format!(
                "{text:?}: `set {key} {value}` added a refusal: terminal UI \
                 {tui_before}->{tui_after}, dux server {server_before}->{server_after}"
            ));
        }
        let says_tui_refuses = said.contains("the terminal UI will not start");
        let says_tui_starts = said.contains("The terminal UI starts with this file.");
        let says_server_refuses = said.contains("dux server will not start");
        let says_server_starts = said.contains("dux server starts with this file.")
            || said.contains("dux server takes this file on a reload");
        let past_by_flag = said.contains("gets past this one");
        let silent =
            !says_tui_refuses && !says_tui_starts && !says_server_refuses && !says_server_starts;
        let untrue = if silent {
            !(tui_after && server_after)
        } else {
            (says_tui_refuses && tui_after)
                || (says_tui_starts && !tui_after)
                || (says_server_refuses && server_after && !past_by_flag)
                || (says_server_starts && !server_after && !past_by_flag)
        };
        if untrue {
            wrong.push(format!(
                "{text:?}: `set {key} {value}` said something untrue (terminal UI starts \
                 {tui_after}, dux server {server_after}):\n{said}"
            ));
        }
    }
    wrong
}

#[test]
fn every_set_over_the_start_corpus_keeps_each_surfaces_start_and_says_it_truly() {
    crate::config::install_canonical_renderer();
    let clean: Vec<bool> = SETS
        .iter()
        .map(|(key, value)| set_on("[ui]\nleft_width_pct = 25\n", key, value).0.is_ok())
        .collect();
    let corpus = dux_core::start_check_fixtures::start_corpus();
    // Four threads, each with its own share of the corpus: every case runs
    // in its own directories.
    let wrong: Vec<String> = std::thread::scope(|scope| {
        let workers: Vec<_> = corpus
            .chunks(corpus.len().div_ceil(4))
            .map(|share| {
                let clean = &clean;
                scope.spawn(move || {
                    share
                        .iter()
                        .flat_map(|case| wrong_sets_on(case, clean))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| worker.join().expect("worker"))
            .collect()
    });
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}
