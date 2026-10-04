//! `dux server` reads a deprecated key (`[server] tailscale_enabled`,
//! `[defaults] prompt_for_name`) through the load migration, in memory only.
//! Every save removes the deprecated key, but the three-way save now treats
//! the carried-over replacement as "seen and unchanged", so it never writes
//! it. One unrelated save (a preference toggled in the web UI) therefore
//! silently resets the user's setting to its default at the next start.
//! 7437238c kept it (its full patch wrote the replacement).
use dux_core::config::DuxPaths;
use dux_core::config_queue::ConfigWriteQueue;

fn paths(dir: &std::path::Path) -> DuxPaths {
    DuxPaths {
        root: dir.to_path_buf(),
        config_path: dir.join("config.toml"),
        sessions_db_path: dir.join("sessions.sqlite3"),
        worktrees_root: dir.join("worktrees"),
        lock_path: dir.join("dux.lock"),
    }
}

#[test]
fn tailscale_disabled_by_the_deprecated_key_stays_disabled_after_an_unrelated_save() {
    let dir = tempfile::TempDir::new().unwrap();
    let paths = paths(dir.path());
    std::fs::write(
        &paths.config_path,
        "[server]\ntailscale_enabled = false\n\n[ui]\ncopy_on_select = true\n",
    )
    .unwrap();
    let mut memory = dux_core::config::load_config(&paths).expect("load");
    assert_eq!(memory.server.tailscale, "no", "precondition: read as no");
    let writer = ConfigWriteQueue::with_base(paths.config_path.clone(), &memory);

    // An unrelated preference saved from the web UI.
    memory.ui.copy_on_select = false;
    writer.save_eager(memory).unwrap();

    let next_start = dux_core::config::load_config(&paths).expect("reload");
    assert_eq!(
        next_start.server.tailscale,
        "no",
        "the user's Tailscale opt-out was lost by an unrelated save:\n{}",
        std::fs::read_to_string(&paths.config_path).unwrap()
    );
}

#[test]
fn pet_names_enabled_by_the_deprecated_key_stay_enabled_after_an_unrelated_save() {
    let dir = tempfile::TempDir::new().unwrap();
    let paths = paths(dir.path());
    std::fs::write(
        &paths.config_path,
        "[defaults]\nprompt_for_name = false\n\n[ui]\ncopy_on_select = true\n",
    )
    .unwrap();
    let mut memory = dux_core::config::load_config(&paths).expect("load");
    assert!(
        memory.defaults.enable_randomized_pet_name_by_default,
        "precondition"
    );
    let writer = ConfigWriteQueue::with_base(paths.config_path.clone(), &memory);
    memory.ui.copy_on_select = false;
    writer.save_eager(memory).unwrap();
    let next_start = dux_core::config::load_config(&paths).expect("reload");
    assert!(
        next_start.defaults.enable_randomized_pet_name_by_default,
        "the setting was lost by an unrelated save:\n{}",
        std::fs::read_to_string(&paths.config_path).unwrap()
    );
}
