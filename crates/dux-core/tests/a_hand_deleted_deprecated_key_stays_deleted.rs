//! A deprecated key the user deletes by hand while dux runs comes back, as
//! its carried-over replacement, on the next unrelated save.
use dux_core::config_queue::ConfigWriteQueue;

fn read(p: &std::path::Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

#[test]
fn a_hand_deleted_public_bind_is_not_written_back_as_host_and_port() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[server]\nbind = \"0.0.0.0:4000\"\n\n[ui]\ncopy_on_select = true\n",
    )
    .unwrap();
    let mut memory = dux_core::config::load_config_file(&path).expect("load");
    assert_eq!(memory.server.host, "0.0.0.0");
    let q = ConfigWriteQueue::with_base(path.clone(), &memory);

    // The user removes the public bind by hand, back to the loopback default.
    std::fs::write(&path, "[ui]\ncopy_on_select = true\n").unwrap();

    // dux saves an unrelated preference.
    memory.ui.copy_on_select = false;
    q.save_eager(memory).unwrap();

    let after = dux_core::config::load_config_file(&path).unwrap();
    assert_eq!(
        after.server.host,
        "127.0.0.1",
        "the hand-deleted public bind came back:\n{}",
        read(&path)
    );
}

#[test]
fn a_hand_deleted_tailscale_enabled_is_not_written_back_as_tailscale() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[server]\ntailscale_enabled = false\n\n[ui]\ncopy_on_select = true\n",
    )
    .unwrap();
    let mut memory = dux_core::config::load_config_file(&path).expect("load");
    assert_eq!(memory.server.tailscale, "no");
    let q = ConfigWriteQueue::with_base(path.clone(), &memory);
    std::fs::write(&path, "[ui]\ncopy_on_select = true\n").unwrap();
    memory.ui.copy_on_select = false;
    q.save_eager(memory).unwrap();
    let after = dux_core::config::load_config_file(&path).unwrap();
    assert_eq!(
        after.server.tailscale,
        "auto",
        "a hand deletion was undone:\n{}",
        read(&path)
    );
}

/// The same for `prompt_for_name`, whose value lands in the opposite
/// `enable_randomized_pet_name_by_default`.
#[test]
fn a_hand_deleted_prompt_for_name_is_not_written_back_as_its_replacement() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[defaults]\nprompt_for_name = false\n\n[ui]\ncopy_on_select = true\n",
    )
    .unwrap();
    let mut memory = dux_core::config::load_config_file(&path).expect("load");
    assert!(memory.defaults.enable_randomized_pet_name_by_default);
    let q = ConfigWriteQueue::with_base(path.clone(), &memory);
    std::fs::write(&path, "[ui]\ncopy_on_select = true\n").unwrap();
    memory.ui.copy_on_select = false;
    q.save_eager(memory).unwrap();
    let text = read(&path);
    assert!(
        !text.contains("enable_randomized_pet_name_by_default"),
        "{text}"
    );
    let after = dux_core::config::load_config_file(&path).unwrap();
    assert!(
        !after.defaults.enable_randomized_pet_name_by_default,
        "{text}"
    );
}

/// A deprecated key still in the file is not written out again as its new
/// keys beside it: the file says it once, the way the user wrote it.
#[test]
fn a_deprecated_key_still_in_the_file_is_not_written_again_as_its_new_keys() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("config.toml");
    std::fs::write(
        &path,
        "[server]\nbind = \"0.0.0.0:4000\"\n\n[ui]\ncopy_on_select = true\n",
    )
    .unwrap();
    let mut memory = dux_core::config::load_config_file(&path).expect("load");
    let q = ConfigWriteQueue::with_base(path.clone(), &memory);
    memory.ui.copy_on_select = false;
    q.save_eager(memory).unwrap();
    let text = read(&path);
    assert!(text.contains("bind = \"0.0.0.0:4000\""), "{text}");
    assert!(!text.contains("host ="), "{text}");
    assert!(!text.contains("port ="), "{text}");
}
