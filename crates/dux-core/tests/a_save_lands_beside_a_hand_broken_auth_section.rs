//! A save from memory lands beside a mistake the user made in [server.auth].
use dux_core::config::Config;
use dux_core::config_queue::ConfigWriteQueue;

/// While dux runs, the user hand-edits config.toml and leaves a mistake in
/// [server.auth]. dux then saves an unrelated preference (toggled in the web
/// UI). The save does not touch [server.auth], so it must land; refusing it
/// loses a change dux made, while `dux config set` of the same key is taken.
#[test]
fn an_unrelated_save_lands_while_server_auth_is_hand_broken() {
    for broken in [
        "[server.auth]\nrequire = \"evrywhere\"\n",
        "[server]\nrequire = \"everywhere\"\n",
        "[server.auth]\nminimum_password_length = \"20\"\n",
    ] {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "[ui]\ncopy_on_select = true\n").unwrap();
        let mut memory = dux_core::config::load_config_file(&path).expect("load");
        let q = ConfigWriteQueue::with_base(path.clone(), &memory);
        std::fs::write(&path, format!("[ui]\ncopy_on_select = true\n\n{broken}")).unwrap();
        memory.ui.copy_on_select = false;
        let saved = q.save_eager(memory);
        let text = std::fs::read_to_string(&path).unwrap();
        let after: toml::Table = toml::from_str(&text).unwrap();
        let ui = after["ui"].as_table().unwrap();
        assert_eq!(
            ui.get("copy_on_select").and_then(toml::Value::as_bool),
            Some(false),
            "with {broken:?} in the file, dux's own change was not saved ({saved:?}):\n{text}"
        );
        let _ = Config::default();
    }
}
