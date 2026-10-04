//! A value dux corrected at load is written back over the user's own hand
//! fix of that same value, made while dux runs, on the next unrelated save.
use dux_core::config::Config;
use dux_core::config_queue::ConfigWriteQueue;

fn read(path: &std::path::Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn a_hand_fix_of_a_value_dux_corrected_survives_an_unrelated_save() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    // A typo dux server corrects to "auto" in memory at load.
    std::fs::write(
        &path,
        "[server]\ntailscale = \"maybe\"\n\n[ui]\ncopy_on_select = true\n",
    )
    .unwrap();
    let mut memory = dux_core::config::load_config_file(&path).expect("load");
    assert_eq!(memory.server.tailscale, "auto");
    let q = ConfigWriteQueue::with_base(path.clone(), &memory);

    // The user reads the warning and fixes the file by hand, choosing "no".
    std::fs::write(
        &path,
        "[server]\ntailscale = \"no\"\n\n[ui]\ncopy_on_select = true\n",
    )
    .unwrap();

    // dux then saves an unrelated change (a preference toggled in the web UI).
    memory.ui.copy_on_select = false;
    q.save_eager(memory).unwrap();

    let after: Config = toml::from_str(&read(&path)).unwrap();
    assert!(!after.ui.copy_on_select, "{}", read(&path));
    assert_eq!(
        after.server.tailscale,
        "no",
        "the user's hand fix was overwritten by dux's in-memory correction:\n{}",
        read(&path)
    );
}

#[test]
fn a_hand_fix_of_a_corrected_font_size_survives_an_unrelated_save() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[ui]\nterminal_font_size = 500\ncopy_on_select = true\n",
    )
    .unwrap();
    let mut memory = dux_core::config::load_config_file(&path).expect("load");
    let q = ConfigWriteQueue::with_base(path.clone(), &memory);
    std::fs::write(
        &path,
        "[ui]\nterminal_font_size = 16\ncopy_on_select = true\n",
    )
    .unwrap();
    memory.ui.copy_on_select = false;
    q.save_eager(memory).unwrap();
    let after: Config = toml::from_str(&read(&path)).unwrap();
    assert_eq!(
        after.ui.terminal_font_size,
        16,
        "hand fix lost:\n{}",
        read(&path)
    );
}
