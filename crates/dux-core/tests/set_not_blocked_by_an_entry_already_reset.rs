//! `dux config set` must never be blocked, for an unrelated key, by problems
//! already in the file. A `[ui]` holding two values of the wrong type is
//! read by `dux server` as its defaults as a whole, so every key in it is
//! already reset before the set. Setting `ui.theme` adds nothing to that,
//! yet it is refused, blaming the change ("that change would make dux reset
//! ui.theme"), while the same situation for a DROPPED entry is let through
//! as the first of the repairs.

use dux_core::config_keys::{lookup, set_plain};

#[test]
fn an_unrelated_key_in_a_section_already_reset_can_still_be_set() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let before = "[ui]\nleft_width_pct = \"a\"\nright_width_pct = \"b\"\n";
    std::fs::write(&path, before).unwrap();
    // Precondition: the section is already read as its defaults.
    let loaded = dux_core::config::effective_config_from_text(before).unwrap();
    assert_eq!(loaded.ui, dux_core::config::Config::default().ui);

    let result = set_plain(&path, &lookup("ui.theme").unwrap(), "x");
    assert!(
        result.is_ok(),
        "an unrelated key is blocked by the problems already in [ui]: {:#}",
        result.unwrap_err()
    );
}

#[test]
fn a_provider_command_can_be_set_while_another_field_already_resets_the_entry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[providers.claude]\nargs = 5\n").unwrap();
    let result = set_plain(
        &path,
        &lookup("providers.claude.command").unwrap(),
        "claude2",
    );
    assert!(
        result.is_ok(),
        "providers.claude.command is blocked by the args problem already in the file: {:#}",
        result.unwrap_err()
    );
}

/// The contrast: the same situation for an entry the load DROPS is let
/// through (this passes), so only the reset case is blocked.
#[test]
fn contrast_a_dropped_entry_lets_the_set_through() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "[providers.newp]\ncommand = \"x\"\nargs = 5\n").unwrap();
    let result = set_plain(&path, &lookup("providers.newp.command").unwrap(), "y");
    assert!(result.is_ok(), "{:#}", result.unwrap_err());
}
