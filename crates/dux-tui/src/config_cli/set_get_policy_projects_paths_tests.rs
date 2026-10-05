//! `set` and `get` on files holding a misspelled minimum, projects, a
//! provider that would turn into a retired stock block, and malformed paths.
use super::*;

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

const BASES: &[&str] = &[
    "",
    "[server]\nbind = \"10.0.0.5:9000\"\n",
    "[server]\nbind = \"127.0.0.1:9000\"\n",
    "[server]\ntailscale_enabled = true\n",
    "[defaults]\nprompt_for_name = true\n",
    "[server.auth]\nrequire = \"always\"\n",
    "[server.auth]\nminimum_password_length = 12\n",
    "[providers.gemini]\ncommand = \"gemini\"\nargs = []\nresume_args = [\"--resume\"]\nresume_wait_timeout_ms = 0\ninstall_hint = \"npm install -g @google/gemini-cli\"\n",
    "[keys]\nquit = \"q\"\n",
    "[[projects]]\nid = \"a\"\npath = \"/tmp/a\"\n",
    "server = { port = 8080 }\n",
    "server.port = 8080\n",
    "[auth]\nusername = \"bob\"\n",
    "[ui]\ncompose_bar = \"x\"\n",
];

const SETS: &[(&str, &str)] = &[
    ("server.port", "0"),
    ("server.port", "4000"),
    ("server.host", "0.0.0.0"),
    ("server.host", "127.0.0.1"),
    ("server.bind", "x"),
    ("server.bind", "10.1.1.1:80"),
    ("server.tailscale_enabled", "5"),
    ("server.tailscale_enabled", "true"),
    ("server.tailscale", "auto"),
    ("defaults.prompt_for_name", "5"),
    ("defaults.prompt_for_name", "true"),
    ("defaults.provider", "nope"),
    ("server.auth.require", "always"),
    ("server.auth.require", "everywhere"),
    ("server.auth.require", "never"),
    ("server.auth.max_password_bytes", "4"),
    ("server.auth.minimum_password_length", "5000"),
    ("server.auth.blocked_addresses", "[\"1.2.3.4\"]"),
    ("server.auth.blocked_addresses", "[\"bad\"]"),
    ("server.auth.cookie_secure", "x"),
    ("server.auth.password_hash", "abc"),
    ("server.auth.password_hash", ""),
    ("providers.gemini.command", "gemini"),
    ("providers.gemini.install_hint", "x"),
    ("providers.newp.command", "x"),
    ("providers.newp.args", "[\"a\"]"),
    ("providers.claude.args", "5"),
    ("macros.m.text", "hi"),
    ("macros.m.surface", "x"),
    ("ui.compose_bar", "x"),
    ("ui.terminal_font_size", "500"),
    ("logging.max_bytes", "-1"),
    ("auth.username", "x"),
    ("auth", "1"),
    ("server.auth", "{}"),
    ("server", "{}"),
    ("editor.default", "x"),
];

#[test]
fn sets_never_add_a_refusal() {
    crate::config::install_canonical_renderer();
    let mut wrong = Vec::new();
    for base in BASES {
        let (tb, sb) = (terminal_ui_starts(base), dux_server_starts(base));
        for (key, value) in SETS {
            let (result, said, after) = set_on(base, key, value);
            if result.is_err() {
                continue;
            }
            let (ta, sa) = (terminal_ui_starts(&after), dux_server_starts(&after));
            if (tb && !ta) || (sb && !sa) {
                wrong.push(format!("{base:?} set {key}={value}: tui {tb}->{ta} server {sb}->{sa}\nafter: {after:?}\nsaid: {said}"));
            }
        }
    }
    assert!(wrong.is_empty(), "{}", wrong.join("\n\n"));
}

fn get_on(text: &str, args: &[&str]) -> (Result<()>, String, String) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let paths = paths_in(tmp.path());
    std::fs::write(&paths.config_path, text).expect("seed");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let r = run_get(&args, &paths, &mut out, &mut err);
    (
        r,
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap(),
    )
}

struct Stdin(&'static str);
impl SecretSource for Stdin {
    fn read_stdin(&mut self) -> Result<Password> {
        Ok(Password::new(self.0.to_string()))
    }
    fn prompt_twice(
        &mut self,
        _: &str,
        _: Option<Meter<'_>>,
    ) -> Result<Option<(Password, Password)>> {
        Ok(None)
    }
}

/// The file asks for 40-character passwords but spells the setting with
/// dashes. `set server.auth.password` takes a 16-character one against the
/// default minimum; once the spelling is fixed, dux starts with a password
/// shorter than the minimum the file sets.
#[test]
fn set_password_with_a_misspelled_minimum_is_refused() {
    crate::config::install_canonical_renderer();
    let tmp = tempfile::tempdir().expect("tempdir");
    let paths = paths_in(tmp.path());
    std::fs::write(
        &paths.config_path,
        "[server.auth]\nminimum-password-length = 40\n",
    )
    .unwrap();
    let mut out = Vec::new();
    let result = run_set(
        &["server.auth.password".to_string(), "--stdin".to_string()],
        &paths,
        &mut Stdin("Tr0ub4dor&3-zqx!"),
        &mut out,
    );
    let said = String::from_utf8(out).unwrap();
    let after = std::fs::read_to_string(&paths.config_path).unwrap();
    // The user fixes the spelling, as the refusal tells them to.
    let fixed = after.replace("minimum-password-length", "minimum_password_length");
    std::fs::write(&paths.config_path, &fixed).unwrap();
    let starts = crate::config::ensure_config(&paths).is_ok();
    assert!(
        result.is_err(),
        "a 16-character password was stored although the file asks for 40 \
         (terminal UI starts after the spelling fix: {starts}):\n{said}\n{fixed}"
    );
}

/// A project's path and name live below `projects`, which `get` prints only
/// with `--show`; `set` and `get` of an unrelated key print them anyway.
#[test]
fn project_values_never_printed_without_show() {
    crate::config::install_canonical_renderer();
    let mut leaks: Vec<String> = Vec::new();
    for text in [
        "[[projects]]\npath = \"/home/me/acme-secret-client\"\nenv = { FOO = \"${\" }\n",
        "[[projects]]\nid = \"a\"\npath = \"/home/me/acme-secret-client\"\n[[projects]]\nid = \"b\"\npath = \"/home/me/acme-secret-client\"\n",
        "[[projects]]\nid = \"a\"\nname = \"acme-secret-client\"\npath = \"/x\"\nenv = { FOO = \"${\" }\n",
    ] {
        let (_r, said, _after) = set_on(text, "ui.left_width_pct", "30");
        let (_r2, out, err) = get_on(text, &["ui.left_width_pct"]);
        let (_r3, pout, perr) = get_on(text, &["projects"]);
        for (what, s) in [
            ("get projects", format!("{pout}{perr}")),
            ("set ui.left_width_pct", said),
            ("get ui.left_width_pct", format!("{out}{err}")),
        ] {
            if s.contains("acme-secret-client") {
                leaks.push(format!("{text:?} `{what}` printed:\n{s}"));
            }
        }
    }
    assert!(leaks.is_empty(), "{}", leaks.join("\n\n"));
}

/// A kept (customized) gemini block; `set` puts its install hint back to the
/// stock text, which makes the load drop the whole provider. `set` says the
/// change applies at the next start; `get` right after says dux drops it.
#[test]
fn a_set_that_makes_dux_drop_the_provider_says_so() {
    crate::config::install_canonical_renderer();
    let kept = "[providers.gemini]\ncommand = \"gemini\"\nargs = []\nresume_args = [\"--resume\"]\nresume_wait_timeout_ms = 0\ninstall_hint = \"my own hint\"\n";
    let (_r0, out0, err0) = get_on(kept, &["providers.gemini.command"]);
    assert_eq!(out0.trim(), "gemini", "kept before the set: {err0}");
    let (r, said, after) = set_on(
        kept,
        "providers.gemini.install_hint",
        "brew install gemini-cli",
    );
    let (_r, out, err) = get_on(&after, &["providers.gemini.command"]);
    let dropped = err.contains("drops that entry");
    assert!(
        r.is_err() || !dropped || said.contains("drop"),
        "set reported success with no word of the drop:\n{said}\nget after:\n{out}{err}"
    );
}

/// The docs: a refused path repeats only the part of the name that exists,
/// never what was typed after it, in case that was a value typed where a
/// name goes. A rule-breaking env name, a path with an empty part and an
/// unknown flag are repeated whole.
#[test]
fn a_malformed_path_never_repeats_what_was_typed_after_the_known_part() {
    crate::config::install_canonical_renderer();
    let mut leaks = Vec::new();
    for args in [
        vec!["env.ghp-SECRET123", "--stdin"],
        vec!["ui..ghpSECRET123", "1"],
        vec!["server.auth..ghpSECRET123"],
    ] {
        let tmp = tempfile::tempdir().unwrap();
        let paths = paths_in(tmp.path());
        std::fs::write(&paths.config_path, "").unwrap();
        let mut out = Vec::new();
        let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let r = run_set(&owned, &paths, &mut NoSecrets, &mut out);
        let s = format!(
            "{}{:?}",
            String::from_utf8_lossy(&out),
            r.err().map(|e| format!("{e:#}"))
        );
        if s.contains("SECRET123") {
            leaks.push(format!("set {args:?}: {s}"));
        }
        let (r, o, e) = get_on("", &args[..1]);
        let s = format!("{o}{e}{:?}", r.err().map(|e| format!("{e:#}")));
        if s.contains("SECRET123") {
            leaks.push(format!("get {:?}: {s}", &args[..1]));
        }
    }
    assert!(leaks.is_empty(), "{}", leaks.join("\n"));
}

/// A set inside a section the load already resets over another of its
/// values says the value stays at its default until that is fixed.
#[test]
fn a_set_held_back_by_an_existing_problem_says_so() {
    crate::config::install_canonical_renderer();
    let (result, said, _after) = set_on(
        "[ui]\nleft_width_pct = \"a\"\nright_width_pct = \"b\"\n",
        "ui.theme",
        "nord",
    );
    result.expect("the set is taken");
    assert!(
        said.contains(
            "ui.theme is written, but it stays at its default until the problems below are fixed"
        ),
        "{said}"
    );
    assert!(!said.contains("\"a\""), "{said}");
}
