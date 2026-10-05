//! `dux config get` must report the value `dux server` actually uses after
//! its load recovery. These files are loaded by `dux server` (the terminal UI
//! refuses them), and the recovery drops more than the one wrong field.

use dux_core::config::load_config_file;
use dux_core::config_keys::{GetValue, get_report, lookup};

fn server_uses(raw: &str) -> dux_core::config::Config {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(&path, raw).unwrap();
    load_config_file(&path).expect("dux server loads this file")
}

/// A custom provider with one wrong-typed field: dux server drops the whole
/// provider, but `get providers.mytool.command` says it uses the file's command.
#[test]
fn get_reports_a_dropped_custom_providers_command_as_in_use() {
    let raw = "[providers.mytool]\ncommand = \"my-tool\"\nargs = 5\n";
    let config = server_uses(raw);
    assert!(
        !config.providers.commands.contains_key("mytool"),
        "precondition: dux server drops the whole provider"
    );
    let report = get_report(raw, &lookup("providers.mytool.command").unwrap()).unwrap();
    assert!(
        !(matches!(&report.value, GetValue::Set(v) if v == "my-tool")
            && report.corrections.is_empty()),
        "get claims dux server uses command my-tool for a provider it dropped: {report:?}"
    );
}

/// A stock provider with a custom command and one wrong-typed field: dux
/// server falls back to the stock provider (command "claude"), but get
/// reports the custom command with no correction.
#[test]
fn get_reports_a_stock_providers_custom_command_dux_server_replaced() {
    let raw = "[providers.claude]\ncommand = \"my-claude\"\nargs = 5\n";
    let config = server_uses(raw);
    let used = &config.providers.commands["claude"].command;
    assert_ne!(
        used, "my-claude",
        "precondition: dux server does not use the file's command"
    );
    let report = get_report(raw, &lookup("providers.claude.command").unwrap()).unwrap();
    assert!(
        matches!(&report.value, GetValue::Set(v) if v == used) || !report.corrections.is_empty(),
        "dux server uses {used:?}, get says {report:?}"
    );
}

/// Two wrong-typed fields in [ui] make dux server reset the whole section,
/// so a valid ui value beside them is not used; get reports it as in use.
#[test]
fn get_reports_a_valid_ui_value_dux_server_reset_with_its_section() {
    let raw =
        "[ui]\nleft_width_pct = 40\nright_width_pct = \"wide\"\nstatus_clear_seconds = \"long\"\n";
    let config = server_uses(raw);
    assert_ne!(
        config.ui.left_width_pct, 40,
        "precondition: dux server reset [ui]"
    );
    let report = get_report(raw, &lookup("ui.left_width_pct").unwrap()).unwrap();
    assert!(
        !(matches!(&report.value, GetValue::Set(v) if v == "40") && report.corrections.is_empty()),
        "dux server uses {}, get says {report:?}",
        config.ui.left_width_pct
    );
}
