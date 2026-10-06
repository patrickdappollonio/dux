//! `get` and `set` never print a value below a key dux does not name.
use super::*;

const SECRET: &str = "sk-live-VALUE0123456789";

fn get_all(body: &str, key: &str) -> String {
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
    let (mut out, mut err) = (Vec::new(), Vec::new());
    run_get(&[key.to_string()], &paths, &mut out, &mut err).expect("get");
    format!(
        "{}{}",
        String::from_utf8(out).unwrap(),
        String::from_utf8(err).unwrap()
    )
}

#[test]
fn get_a_table_never_prints_the_value_of_a_hidden_key() {
    crate::config::install_canonical_renderer();
    let mut leaks = Vec::new();
    for (body, key) in [
        (format!("[ui]\n\"sk-proj-Ab x\" = \"{SECRET}\"\n"), "ui"),
        (format!("[ui]\napi_key = \"{SECRET}\"\n"), "ui"),
        (
            format!("[providers.claude]\napi_key = \"{SECRET}\"\n"),
            "providers.claude",
        ),
        (
            format!("[macros.foo]\ntext = \"x\"\nsurface = \"both\"\napi_key = \"{SECRET}\"\n"),
            "macros.foo",
        ),
        (
            format!("[macros.foo]\ntext = \"x\"\napi_key = \"{SECRET}\"\n"),
            "macros.foo",
        ),
        (format!("[server]\nzzz = [\"{SECRET}\"]\n"), "server"),
        (format!("[ui]\nzzz = {{ a = \"{SECRET}\" }}\n"), "ui"),
        // A provider whose NAME breaks the rule, dropped by dux server's load:
        // everything below it is printed.
        (
            format!("[providers.\"sk-proj x\"]\ncommand = 5\nargs = [\"{SECRET}\"]\n"),
            "providers",
        ),
    ] {
        let said = get_all(&body, key);
        if said.contains(SECRET) {
            leaks.push(format!("get {key} on {body:?}:\n{said}"));
        }
    }
    assert!(
        leaks.is_empty(),
        "values below hidden keys were printed:\n{}",
        leaks.join("\n----\n")
    );
}

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

fn set_all(body: &str, key: &str, value: &str) -> String {
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
    let mut out = Vec::new();
    let result = run_set(
        &[key.to_string(), value.to_string()],
        &paths,
        &mut NoSecrets,
        &mut out,
    );
    format!("{result:?}\n{}", String::from_utf8(out).unwrap())
}

/// `set` reports what it replaced, but never a value holding a table, whose
/// keys are names the file chose. A table nested one array deeper is printed
/// whole, the pasted name with it.
#[test]
fn set_never_prints_a_name_from_the_value_it_replaced() {
    crate::config::install_canonical_renderer();
    let token = "sk-proj-AbCdEf0123456789";
    let said = set_all(
        &format!("[ui]\ntheme = [[{{ \"{token}\" = 1 }}]]\n"),
        "ui.theme",
        "dux_dark",
    );
    assert!(said.starts_with("Ok"), "{said}");
    assert!(
        !said.contains(token),
        "set printed the pasted name:\n{said}"
    );
}

/// A setting the file wrote as a table (`[ui.theme]`) was set; `set` must
/// not say it was not.
#[test]
fn set_over_a_table_does_not_say_the_setting_was_not_set() {
    crate::config::install_canonical_renderer();
    let said = set_all("[ui.theme]\nname = \"x\"\n", "ui.theme", "dux_dark");
    assert!(said.starts_with("Ok"), "{said}");
    assert!(
        !said.contains("(not set)"),
        "set says the table it replaced was not set:\n{said}"
    );
}
