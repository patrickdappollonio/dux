//! Attribution of a start problem to the setting a `set` changes is made
//! on dotted strings, so a name holding a dot makes a problem that is about
//! another entry look like the set's own, and the set is refused.
use dux_core::auth::Password;
use dux_core::config_keys;

fn seed(body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, body).unwrap();
    (dir, path)
}

/// `[env]` holds an entry whose name is not a variable name and contains a
/// dot (`FOO.BAR`), which stops the terminal UI. Setting `env.FOO`, a
/// different, valid variable, adds no problem, yet it is refused.
#[test]
fn an_unrelated_env_set_is_not_blocked_by_a_dotted_name() {
    let (_d, path) = seed("[env]\n\"FOO.BAR\" = \"x\"\n");
    // Precondition: the file already has a problem, about the entry FOO.BAR.
    let before = dux_core::config::start_problems_of(&std::fs::read_to_string(&path).unwrap());
    assert!(!before.is_empty());
    let key = config_keys::lookup("env.FOO").expect("a valid name");
    let result = config_keys::set_secret_text(&path, &key, &Password::new("y".to_string()));
    assert!(
        result.is_ok(),
        "setting env.FOO was refused over a problem about another entry: {:#}",
        result.unwrap_err()
    );
}

/// The same with a provider named `a.command` whose `args` is the wrong
/// type: setting `providers.a.command` (a different provider) is refused.
#[test]
fn an_unrelated_provider_set_is_not_blocked_by_a_dotted_name() {
    let (_d, path) = seed("[providers.\"a.command\"]\ncommand = \"x\"\nargs = 5\n");
    let key = config_keys::lookup("providers.a.command").expect("a key");
    let result = config_keys::set_plain(&path, &key, "mytool");
    assert!(
        result.is_ok(),
        "setting providers.a.command was refused over a problem about provider \"a.command\": {:#}",
        result.unwrap_err()
    );
}
