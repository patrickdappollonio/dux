//! A plaintext password, or an auth setting under a `projects` table, is never
//! read as less protection.
use dux_core::auth::{Password, hash_password};
use dux_core::config::{Surface, check_start, config_from_text_as_loaded};

fn hash() -> String {
    hash_password(&Password::new(
        "correct horse battery staple veranda".into(),
    ))
    .unwrap()
}

fn refuses(text: &str, surface: Surface) -> bool {
    let loaded = config_from_text_as_loaded(text);
    let stops = check_start(text).problems.iter().any(|p| p.stops(surface));
    loaded.is_err() || stops
}

#[test]
fn password_hash_in_projects_written_as_a_table_env_is_refused() {
    let text = format!("[projects.env]\npassword_hash = \"{}\"\n", hash());
    assert!(
        refuses(&text, Surface::DuxServer),
        "dux server starts:\n{text}"
    );
}

#[test]
fn require_in_projects_table_env_is_refused() {
    let text = format!(
        "[server.auth]\npassword_hash = \"{}\"\n\n[projects.env]\nrequire = \"everywhere\"\n",
        hash()
    );
    assert!(
        refuses(&text, Surface::DuxServer),
        "dux server starts:\n{text}"
    );
}

#[test]
fn the_password_setting_under_server_is_refused() {
    let text = "[server]\npassword = \"correct horse battery staple veranda\"\n";
    assert!(
        refuses(text, Surface::DuxServer),
        "dux server starts:\n{text}"
    );
}

#[test]
fn the_password_setting_under_a_misspelled_auth_table_is_refused() {
    let text = "[server.Auth]\npassword = \"correct horse battery staple veranda\"\n";
    assert!(
        refuses(text, Surface::DuxServer),
        "dux server starts:\n{text}"
    );
}

#[test]
fn the_password_setting_at_the_top_level_is_refused() {
    let text = "password = \"correct horse battery staple veranda\"\n";
    assert!(
        refuses(text, Surface::DuxServer),
        "dux server starts:\n{text}"
    );
}
