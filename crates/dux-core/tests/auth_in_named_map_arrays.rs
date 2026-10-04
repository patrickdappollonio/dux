//! An auth setting inside an ARRAY written where a map of user-chosen names
//! goes: the key is a field of an array element, never a name the user chose.
use dux_core::auth::{Password, hash_password};
use dux_core::config::{Surface, check_start, config_from_text_as_loaded};

fn hash() -> String {
    hash_password(&Password::new(
        "correct horse battery staple veranda".into(),
    ))
    .unwrap()
}

fn assert_dux_server_refuses(text: &str) {
    let loaded = config_from_text_as_loaded(text);
    let stops = check_start(text)
        .problems
        .iter()
        .any(|p| p.stops(Surface::DuxServer));
    assert!(
        loaded.is_err() || stops,
        "dux server starts with this file, with password_hash = {:?} and require = {:?}:\n{text}",
        loaded
            .as_ref()
            .map(|c| c.server.auth.password_hash.clone())
            .ok(),
        loaded.as_ref().map(|c| c.server.auth.require).ok(),
    );
}

#[test]
fn a_password_hash_in_an_env_array_of_tables_is_refused() {
    assert_dux_server_refuses(&format!("[[env]]\npassword_hash = \"{}\"\n", hash()));
}

#[test]
fn require_in_a_providers_array_of_tables_is_refused() {
    assert_dux_server_refuses(&format!(
        "[server.auth]\npassword_hash = \"{}\"\n\n[[providers]]\nrequire = \"everywhere\"\n",
        hash()
    ));
}

#[test]
fn a_password_hash_in_an_inline_macros_array_is_refused() {
    assert_dux_server_refuses(&format!(
        "macros = [{{ password_hash = \"{}\" }}]\n",
        hash()
    ));
}
