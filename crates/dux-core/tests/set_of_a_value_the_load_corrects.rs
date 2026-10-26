//! `dux config set` refuses a value the load would use something else in
//! place of: reset, dropped or clamped. Each case is its own test.

use dux_core::config_keys::{lookup, set_plain};

/// Precondition and check for one case: the load, given `value` written
/// directly, uses `used`; a set of it is refused, names what dux would use,
/// and leaves the file as it was.
fn refused_naming_what_dux_uses(key: &str, value: &str, used: u64) {
    let (table, field) = key
        .split_once('.')
        .expect("the key names a table and a field");
    let written = format!("[{table}]\n{field} = {value}\n");
    let loaded =
        dux_core::config::effective_config_from_text(&written).expect("the written value loads");
    assert_eq!(
        serde_json::to_value(&loaded).expect("a config serializes to JSON")[table][field],
        serde_json::json!(used),
        "precondition: the load uses {used} for {key} = {value}"
    );
    let dir = tempfile::tempdir().expect("create a temporary config directory");
    let path = dir.path().join("config.toml");
    std::fs::write(&path, "").expect("write an empty config file");
    let error = match set_plain(&path, &lookup(key).expect("the key names a setting"), value) {
        Ok(report) => panic!(
            "{key} = {value} was reported as set (now = {}), but every surface uses {used}",
            report.now
        ),
        Err(error) => format!("{error:#}"),
    };
    assert!(
        error.contains(&format!("dux would use {used} instead of {value}")),
        "{key}: {error}"
    );
    assert!(error.contains("nothing was changed"), "{key}: {error}");
    assert_eq!(
        std::fs::read_to_string(&path).expect("read the config file back"),
        ""
    );
}

#[test]
fn a_github_probe_interval_below_its_floor_is_refused() {
    refused_naming_what_dux_uses("ui.github_probe_interval_secs", "1", 30);
}

#[test]
fn an_upload_pasted_text_chars_below_its_floor_is_refused() {
    refused_naming_what_dux_uses("ui.upload_pasted_text_chars", "1", 200);
}

#[test]
fn a_terminal_font_size_out_of_range_is_refused() {
    refused_naming_what_dux_uses("ui.terminal_font_size", "500", 14);
}
