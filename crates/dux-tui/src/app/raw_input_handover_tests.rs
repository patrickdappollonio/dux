//! Keys that arrive in the same read as a key that moves input from the
//! forwarding path back to crossterm's reader or to a prompt.

use std::path::Path;
use std::time::{Duration, Instant};

use dux_core::pty::PtyClient;

use super::test_support::{default_bindings, test_app};
use super::*;

/// An app whose selected agent runs a program that writes what it receives to
/// `received`, fullscreen and reading through the forwarding path.
fn fullscreen_app_typing_into(received: &Path) -> App {
    let mut app = test_app(default_bindings());
    let slot_tab = app.engine.sessions[0].slot_tab_id().to_string();
    let script = format!(
        "stty raw -echo; printf ready; exec cat > '{}'",
        received.display()
    );
    let client = PtyClient::spawn(
        "sh",
        &["-c".to_string(), script],
        Path::new("."),
        5,
        80,
        100,
    )
    .expect("spawn the embedded program");
    app.engine.providers.insert(TabId::new(slot_tab), client);
    app.focus = FocusPane::Center;
    app.center_mode = CenterMode::Agent;
    app.session_surface = crate::model::SessionSurface::Agent;
    let deadline = Instant::now() + Duration::from_secs(5);
    while !app
        .selected_terminal_surface_client()
        .is_some_and(PtyClient::has_output)
    {
        assert!(
            Instant::now() < deadline,
            "the program never said it is raw"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(app.center_typeable(), "the windowed pane takes typing");
    app.input_target = InputTarget::Agent;
    app.fullscreen_overlay = FullscreenOverlay::Agent;
    assert!(app.should_poll_raw_input());
    app
}

fn wait_for_received(received: &Path, want: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let got =
            String::from_utf8_lossy(&std::fs::read(received).unwrap_or_default()).into_owned();
        if got.len() >= want.len() || Instant::now() >= deadline {
            return got;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// What follows the fullscreen toggle in the same read is typing for the
/// windowed pane it leads back to, as it would be a moment later.
#[test]
fn keys_read_with_the_toggle_out_of_fullscreen_reach_the_windowed_pane() {
    let dir = tempfile::tempdir().expect("tempdir");
    let received = dir.path().join("received");
    let mut app = fullscreen_app_typing_into(&received);

    assert!(!app.process_raw_input_bytes(b"ab\x07cd"));
    assert_eq!(app.input_target, InputTarget::None);
    while let Some(event) = app.terminal_input.next_event() {
        assert!(!app.handle_terminal_event(event));
    }

    assert_eq!(wait_for_received(&received, "abcd"), "abcd");
}

/// The macro bar key with no macros to show leaves input where it was, so
/// what follows it in the same read is still typing for the program.
#[test]
fn keys_read_with_a_macro_bar_key_that_opens_nothing_still_reach_the_program() {
    let dir = tempfile::tempdir().expect("tempdir");
    let received = dir.path().join("received");
    let mut app = fullscreen_app_typing_into(&received);
    let mut typed = b"ab".to_vec();
    typed.extend(
        app.bindings
            .byte_patterns_for(Action::OpenMacroBar)
            .into_iter()
            .next()
            .expect("the macro bar has a default key"),
    );
    typed.extend_from_slice(b"cd");

    assert!(!app.process_raw_input_bytes(&typed));
    let rest = app.terminal_input.take_bytes();
    assert!(!app.process_raw_input_bytes(&rest));

    assert!(app.macro_bar.is_none(), "test premise: no macros to show");
    assert_eq!(app.input_target, InputTarget::Agent);
    assert_eq!(wait_for_received(&received, "abcd"), "abcd");
}
