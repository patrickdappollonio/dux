//! Auth settings written where dux does not read them, which dux starts
//! with, reading the mistake as less protection.
use dux_core::auth::{Password, hash_password};
use dux_core::config::{config_from_text_as_loaded, start_problems_of};

fn hash() -> String {
    hash_password(&Password::new(
        "correct horse battery staple veranda".into(),
    ))
    .unwrap()
}

fn assert_refused(text: &str) {
    let loaded = config_from_text_as_loaded(text);
    assert!(
        loaded.is_err() && !start_problems_of(text).is_empty(),
        "dux starts with this file, reading require = {:?}, blocked_addresses = {:?}:\n{text}",
        loaded.as_ref().map(|c| c.server.auth.require).ok(),
        loaded
            .as_ref()
            .map(|c| c.server.auth.blocked_addresses.clone())
            .ok(),
    );
}

/// `[server.auths]` (a table) is refused; the same table written as an
/// array of tables is read as nothing.
#[test]
fn an_array_of_auth_like_tables_under_server_is_refused() {
    let table = format!(
        "[server.auth]\npassword_hash = \"{}\"\n\n[server.auths]\nrequire = \"everywhere\"\n",
        hash()
    );
    assert!(
        !start_problems_of(&table).is_empty(),
        "precondition: the table form is refused"
    );
    assert_refused(&format!(
        "[server.auth]\npassword_hash = \"{}\"\n\n[[server.auths]]\nrequire = \"everywhere\"\n",
        hash()
    ));
}

/// `[Server.auth]` holding a password hash is refused; the same table
/// holding `require` beside the real password is read as nothing, so the
/// password is asked for off this machine only.
#[test]
fn an_auth_table_under_a_differently_cased_server_is_refused() {
    assert_refused(&format!(
        "[server.auth]\npassword_hash = \"{}\"\n\n[Server.auth]\nrequire = \"everywhere\"\n",
        hash()
    ));
}

/// `blocked_addresses` applies with or without a password; under
/// `[SERVER.auth]` it is read as nothing and the address is not blocked.
#[test]
fn blocked_addresses_under_a_differently_cased_server_is_refused() {
    assert_refused("[SERVER.auth]\nblocked_addresses = [\"203.0.113.7\"]\n");
}

/// `"auth.password_hash"` written as one key under `[server]` is refused;
/// `"auth.require"` in the same place is read as nothing.
#[test]
fn a_dotted_auth_setting_written_as_one_key_under_server_is_refused() {
    let hashed = format!("[server]\n\"auth.password_hash\" = \"{}\"\n", hash());
    assert!(
        !start_problems_of(&hashed).is_empty(),
        "precondition: the hash form is refused"
    );
    assert_refused(&format!(
        "[server]\n\"auth.require\" = \"everywhere\"\n\n[server.auth]\npassword_hash = \"{}\"\n",
        hash()
    ));
}

/// The whole header quoted as one name: `["server.auth"]`. Its password
/// hash is refused; its `blocked_addresses` is read as nothing.
#[test]
fn a_quoted_server_auth_header_holding_blocked_addresses_is_refused() {
    let hashed = format!("[\"server.auth\"]\npassword_hash = \"{}\"\n", hash());
    assert!(
        !start_problems_of(&hashed).is_empty(),
        "precondition: the hash form is refused"
    );
    assert_refused("[\"server.auth\"]\nblocked_addresses = [\"203.0.113.7\"]\n");
}
