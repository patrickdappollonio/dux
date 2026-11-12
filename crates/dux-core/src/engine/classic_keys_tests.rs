//! A program started through dux's real spawn path is never offered Kitty
//! keys: its keyboard query gets no answer, so it stays on classic keys.

use std::time::{Duration, Instant};

use crate::engine::test_support::{sample_project, sample_session, test_engine};

#[test]
fn a_companion_terminal_asking_for_kitty_keys_gets_no_answer() {
    let (mut engine, _tmp) = test_engine();
    let worktree = tempfile::tempdir().expect("worktree dir");
    let path = worktree.path().to_string_lossy().to_string();
    engine.projects.push(sample_project("p1", &path));
    let mut session = sample_session("s1", "p1", "feature");
    session
        .workspace
        .as_managed_mut()
        .expect("managed test session")
        .worktree_path = path;
    engine.sessions.push(session);
    // The shell queries and pushes Kitty flags, then echoes everything it
    // reads in caret notation, so an answer to the query would show as `^[[?`.
    engine.config.terminal.command = "sh".to_string();
    engine.config.terminal.args = vec![
        "-c".to_string(),
        r"stty raw -echo; printf '\033[?u\033[>1u'; printf READY; exec cat -v".to_string(),
    ];
    let (terminal_id, _) = engine
        .create_companion_terminal("s1", 24, 80)
        .expect("create companion terminal");
    let client = &engine.companion_terminals[&terminal_id].client;
    let grid = || {
        client
            .snapshot()
            .cells
            .iter()
            .map(|cell| cell.symbol.as_str())
            .collect::<String>()
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while !grid().contains("READY") {
        assert!(Instant::now() < deadline, "the shell never printed READY");
        std::thread::sleep(Duration::from_millis(10));
    }
    std::thread::sleep(Duration::from_millis(300));
    let shown = grid();
    assert!(
        !shown.contains("^[[?"),
        "the Kitty query was answered: {shown:?}"
    );
}
