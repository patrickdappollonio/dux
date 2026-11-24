//! Input typed in one burst across a key that hands input from crossterm's
//! reader to the forwarding reader, or back.
//!
//! Each case runs the run loop's own input poll in a copy of this test binary
//! inside a pseudo-terminal, so the readers read a real terminal, and types
//! into it from the parent. A burst is judged against the same bytes typed
//! paced, one write at a time with time to draw between them, which is what a
//! person typing by hand produces: the two must reach the program and leave
//! dux in the same state.

use std::path::Path;
use std::time::{Duration, Instant};

use dux_core::pty::PtyClient;

use super::test_support::{default_bindings, test_app};
use super::*;
use crate::pty_child::{PtyChild, child_dir};

const CHILD_TEST: &str = "app::typed_burst_tests::child_runs_the_input_loop";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pane {
    Windowed,
    Fullscreen,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Keys {
    Classic,
    Extended,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Program {
    Plain,
    /// Asks for SGR mouse reports and focus reports.
    ReadsMouse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Driver {
    ThisSurface,
    /// dux serves the web and a browser drives the program, so the take-over
    /// card covers it.
    Browser,
}

/// How the child starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Setup {
    pane: Pane,
    keys: Keys,
    program: Program,
    driver: Driver,
}

const WINDOWED: Setup = Setup {
    pane: Pane::Windowed,
    keys: Keys::Classic,
    program: Program::Plain,
    driver: Driver::ThisSurface,
};
const FULLSCREEN: Setup = Setup {
    pane: Pane::Fullscreen,
    ..WINDOWED
};

impl Setup {
    fn encode(self) -> String {
        format!(
            "{:?} {:?} {:?} {:?}",
            self.pane, self.keys, self.program, self.driver
        )
    }

    fn decode(text: &str) -> Self {
        let words: Vec<&str> = text.split(' ').collect();
        Self {
            pane: if words[0] == "Fullscreen" {
                Pane::Fullscreen
            } else {
                Pane::Windowed
            },
            keys: if words[1] == "Extended" {
                Keys::Extended
            } else {
                Keys::Classic
            },
            program: if words[2] == "ReadsMouse" {
                Program::ReadsMouse
            } else {
                Program::Plain
            },
            driver: if words[3] == "Browser" {
                Driver::Browser
            } else {
                Driver::ThisSurface
            },
        }
    }
}

/// Not a test on its own: run as a child under a pseudo-terminal, it runs the
/// run loop's draw and input poll until the parent says it is done, then
/// writes what the program received and where input went to `<dir>/result`.
#[test]
fn child_runs_the_input_loop() {
    let Some(dir) = child_dir() else {
        return;
    };
    let setup = Setup::decode(&std::fs::read_to_string(dir.join("setup")).expect("setup"));
    let received = dir.join("received");
    let (mut app, _seat) = app_typing_into(&received, setup);
    let mut terminal =
        ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30)).expect("terminal");

    crossterm::terminal::enable_raw_mode().expect("raw mode on the child's terminal");
    std::fs::write(dir.join("ready"), b"").expect("ready marker");
    let mut done_since: Option<Instant> = None;
    loop {
        terminal.draw(|frame| app.render(frame)).expect("draw");
        assert!(!app.poll_run_input(), "the app asked to exit");
        if done_since.is_none() && dir.join("done").exists() {
            done_since = Some(Instant::now());
        }
        if done_since.is_some_and(|since| since.elapsed() > Duration::from_millis(600)) {
            break;
        }
    }
    let _ = crossterm::terminal::disable_raw_mode();

    let got = std::fs::read(&received).unwrap_or_default();
    let report = format!(
        "received {:?}\ntarget {:?}\nfullscreen {:?}\nfocus {:?}\nprompt {:?}\nmacro bar {}",
        String::from_utf8_lossy(&got),
        app.input_target,
        app.fullscreen_overlay,
        app.focus,
        std::mem::discriminant(&app.prompt),
        app.macro_bar.is_some(),
    );
    let tmp = dir.join("result.tmp");
    std::fs::write(&tmp, report).expect("result file");
    std::fs::rename(tmp, dir.join("result")).expect("result file");
}

/// The app, and the serving state that has to outlive it when a browser drives.
fn app_typing_into(received: &Path, setup: Setup) -> (App, Option<Box<dyn std::any::Any>>) {
    let enable = if setup.program == Program::ReadsMouse {
        "\\033[?1000h\\033[?1006h\\033[?1004h"
    } else {
        ""
    };
    let script = format!(
        "stty raw -echo; printf '{enable}ready'; exec cat > '{}'",
        received.display()
    );
    let (mut app, seat) = if setup.driver == Driver::Browser {
        let (app, recorded, seat) =
            super::pty_ownership::tests::app_with_a_live_pty_running(&script);
        let browser = seat.owners.next_conn_id();
        seat.owners
            .claim_for_resize(
                "session-1-slot",
                browser,
                dux_core::pty_owners::ResizeIntent::Plain,
                Some("Chrome"),
                |_| {},
            )
            .epoch
            .expect("the browser claimed the pty");
        let keep: Box<dyn std::any::Any> = Box::new((recorded, seat));
        (app, Some(keep))
    } else {
        let mut app = test_app(default_bindings());
        let slot_tab = app.engine.sessions[0].slot_tab_id().to_string();
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
        app.center_mode = CenterMode::Agent;
        app.session_surface = crate::model::SessionSurface::Agent;
        (app, None)
    };
    if setup.keys == Keys::Extended {
        app.set_extended_keys(crate::keybindings::ExtendedKeys::On);
    }
    app.focus = FocusPane::Center;
    let deadline = Instant::now() + Duration::from_secs(5);
    while !app
        .selected_terminal_surface_client()
        .is_some_and(PtyClient::has_output)
    {
        assert!(Instant::now() < deadline, "the program never said ready");
        std::thread::sleep(Duration::from_millis(10));
    }
    if setup.pane == Pane::Fullscreen {
        app.input_target = InputTarget::Agent;
        app.fullscreen_overlay = FullscreenOverlay::Agent;
    }
    (app, seat)
}

/// Type `writes` into a fresh child, waiting `gap` after each, and return its
/// report.
fn run(setup: Setup, writes: &[&[u8]], gap: Duration) -> String {
    let child = PtyChild::start(CHILD_TEST, &setup.encode());
    // The program's own start and the first frame.
    std::thread::sleep(Duration::from_millis(200));
    for write in writes {
        child.type_bytes(write);
        std::thread::sleep(gap);
    }
    child.finish()
}

/// Every byte of `typed` in one write.
fn burst(setup: Setup, typed: &[u8]) -> String {
    run(setup, &[typed], Duration::from_millis(100))
}

/// `typed` as a person types it: one write per piece, with a pause after each.
fn paced(setup: Setup, pieces: &[&[u8]]) -> String {
    run(setup, pieces, Duration::from_millis(400))
}

/// Assert that the burst of `pieces` joined, and the same bytes split into two
/// writes at every boundary, end exactly where typing them paced ends.
fn assert_burst_matches_paced(setup: Setup, pieces: &[&[u8]]) -> String {
    let typed: Vec<u8> = pieces.concat();
    let want = paced(setup, pieces);
    assert_eq!(burst(setup, &typed), want, "one write of {typed:?}");
    want
}

fn assert_every_split_matches(setup: Setup, typed: &[u8], want: &str) {
    for at in 1..typed.len() {
        let (first, second) = typed.split_at(at);
        assert_eq!(
            run(setup, &[first, second], Duration::from_millis(150)),
            want,
            "two writes {first:?} and {second:?}"
        );
    }
}

fn received_line(report: &str) -> &str {
    report.lines().next().expect("received line")
}

const TOGGLE: &[u8] = b"\x07";

#[test]
fn text_around_the_maximize_key_reaches_the_program_in_order() {
    let want = assert_burst_matches_paced(WINDOWED, &[b"ab", TOGGLE, b"cd"]);
    assert!(
        want.starts_with("received \"abcd\"\ntarget Agent\nfullscreen Agent"),
        "{want}"
    );
    assert_every_split_matches(WINDOWED, b"ab\x07cd", &want);
}

#[test]
fn text_around_the_leave_key_reaches_the_program_in_order() {
    let want = assert_burst_matches_paced(FULLSCREEN, &[b"ab", TOGGLE, b"cd"]);
    assert!(
        want.starts_with("received \"abcd\"\ntarget None\nfullscreen None"),
        "{want}"
    );
    assert_every_split_matches(FULLSCREEN, b"ab\x07cd", &want);
}

#[test]
fn maximize_leave_and_maximize_again_in_one_write() {
    let want =
        assert_burst_matches_paced(WINDOWED, &[b"a", TOGGLE, b"b", TOGGLE, b"c", TOGGLE, b"d"]);
    assert!(
        want.starts_with("received \"abcd\"\ntarget Agent\nfullscreen Agent"),
        "{want}"
    );
}

/// Ctrl-Shift-g, Ctrl-i and Ctrl-m reach the program as their classic bytes and
/// the pane stays maximized; a Super chord with no classic form is dropped.
#[test]
fn extended_chords_after_the_maximize_key_are_read_by_the_forwarding_path() {
    let setup = Setup {
        keys: Keys::Extended,
        ..WINDOWED
    };
    let want = assert_burst_matches_paced(
        setup,
        &[
            TOGGLE,
            b"\x1b[103;6u",
            b"\x1b[105;5u",
            b"\x1b[109;5u",
            b"\x1b[103;9u",
            b"z",
        ],
    );
    assert!(
        want.starts_with("received \"\\u{7}\\t\\rz\"\ntarget Agent\nfullscreen Agent"),
        "{want}"
    );
}

/// With extended keys off the forwarding path forwards a `CSI … u` as it came.
#[test]
fn chords_after_the_maximize_key_with_extended_keys_off_are_forwarded_as_sent() {
    let want = assert_burst_matches_paced(WINDOWED, &[TOGGLE, b"\x1b[103;6u", b"z"]);
    assert!(
        want.starts_with("received \"\\u{1b}[103;6uz\"\ntarget Agent\nfullscreen Agent"),
        "{want}"
    );
}

#[test]
fn extended_chords_after_the_leave_key_are_read_by_crossterm_s_reader() {
    let setup = Setup {
        keys: Keys::Extended,
        ..FULLSCREEN
    };
    assert_burst_matches_paced(setup, &[TOGGLE, b"\x1b[105;5u", b"\x1b[109;5u", b"z"]);
}

/// The program never enabled bracketed paste, so the forwarding path sends the
/// paste's text with its line breaks as carriage returns: loose keys would keep
/// the line feed.
#[test]
fn a_paste_just_after_the_maximize_key_arrives_whole() {
    let want = assert_burst_matches_paced(WINDOWED, &[TOGGLE, b"\x1b[200~p1\np2\x1b[201~", b"z"]);
    assert!(want.starts_with("received \"p1\\rp2z\""), "{want}");
}

#[test]
fn a_paste_after_the_maximize_key_whose_end_comes_in_a_later_read_arrives_whole() {
    let want = paced(WINDOWED, &[TOGGLE, b"\x1b[200~p1\n", b"p2\x1b[201~z"]);
    assert!(want.starts_with("received \"p1\\rp2z\""), "{want}");
    assert_eq!(
        run(
            WINDOWED,
            &[b"\x07\x1b[200~p1\n", b"p2\x1b[201~z"],
            Duration::from_millis(150)
        ),
        want
    );
}

/// A toggle inside a paste is pasted text, before the switch as after it.
#[test]
fn a_paste_that_holds_the_toggle_and_ends_in_a_later_read_is_one_paste() {
    let want = paced(WINDOWED, &[b"\x1b[200~a\x07", b"b\x1b[201~", TOGGLE, b"c"]);
    assert!(want.contains("fullscreen Agent"), "{want}");
    assert_eq!(
        run(
            WINDOWED,
            &[b"\x1b[200~a\x07", b"b\x1b[201~\x07c"],
            Duration::from_millis(150)
        ),
        want
    );
}

#[test]
fn a_paste_just_after_the_leave_key_arrives_whole() {
    let want = assert_burst_matches_paced(FULLSCREEN, &[TOGGLE, b"\x1b[200~q1\nq2\x1b[201~"]);
    assert!(received_line(&want).contains("q1"), "{want}");
}

#[test]
fn a_paste_after_the_leave_key_whose_end_comes_in_a_later_read_arrives_whole() {
    let want = paced(FULLSCREEN, &[TOGGLE, b"\x1b[200~q1\n", b"q2\x1b[201~"]);
    assert!(received_line(&want).contains("q2"), "{want}");
    assert_eq!(
        run(
            FULLSCREEN,
            &[b"\x07\x1b[200~q1\n", b"q2\x1b[201~"],
            Duration::from_millis(150)
        ),
        want
    );
}

#[test]
fn a_mouse_click_and_focus_reports_straddling_the_maximize_key() {
    let setup = Setup {
        program: Program::ReadsMouse,
        ..WINDOWED
    };
    let want = assert_burst_matches_paced(
        setup,
        &[
            b"\x1b[I",
            TOGGLE,
            b"\x1b[<0;20;10M",
            b"\x1b[<0;20;10m",
            b"\x1b[O",
        ],
    );
    assert!(received_line(&want).contains("[<0;"), "{want}");
}

#[test]
fn a_mouse_click_and_focus_reports_straddling_the_leave_key() {
    let setup = Setup {
        program: Program::ReadsMouse,
        ..FULLSCREEN
    };
    assert_burst_matches_paced(
        setup,
        &[
            b"\x1b[I",
            TOGGLE,
            b"\x1b[<0;20;10M",
            b"\x1b[<0;20;10m",
            b"\x1b[O",
        ],
    );
}

#[test]
fn a_lone_escape_just_after_the_maximize_key_reaches_the_program() {
    let want = assert_burst_matches_paced(WINDOWED, &[TOGGLE, b"\x1b"]);
    assert!(want.starts_with("received \"\\u{1b}\""), "{want}");
}

#[test]
fn a_lone_escape_just_after_the_leave_key() {
    assert_burst_matches_paced(FULLSCREEN, &[TOGGLE, b"\x1b"]);
}

#[test]
fn an_escape_sequence_cut_by_the_read_after_the_maximize_key_reaches_the_program_whole() {
    let want = paced(WINDOWED, &[TOGGLE, b"\x1b[Ax"]);
    assert!(want.starts_with("received \"\\u{1b}[Ax\""), "{want}");
    assert_eq!(
        run(WINDOWED, &[b"\x07\x1b[", b"Ax"], Duration::from_millis(150)),
        want
    );
}

#[test]
fn an_escape_sequence_cut_by_the_read_after_the_leave_key() {
    let want = paced(FULLSCREEN, &[TOGGLE, b"\x1b[Ax"]);
    assert_eq!(
        run(
            FULLSCREEN,
            &[b"\x07\x1b[", b"Ax"],
            Duration::from_millis(150)
        ),
        want
    );
}

#[test]
fn non_latin_text_and_a_character_cut_by_the_read_cross_the_maximize_key() {
    let want = assert_burst_matches_paced(WINDOWED, &["漢".as_bytes(), TOGGLE, "字é".as_bytes()]);
    assert!(want.starts_with("received \"漢字é\""), "{want}");
    let cut = paced(WINDOWED, &[TOGGLE, "漢z".as_bytes()]);
    assert_eq!(
        run(
            WINDOWED,
            &[b"\x07\xe6\xbc", b"\xa2z"],
            Duration::from_millis(150)
        ),
        cut
    );
}

#[test]
fn non_latin_text_and_a_character_cut_by_the_read_cross_the_leave_key() {
    assert_burst_matches_paced(FULLSCREEN, &["漢".as_bytes(), TOGGLE, "字é".as_bytes()]);
    let cut = paced(FULLSCREEN, &[TOGGLE, "漢z".as_bytes()]);
    assert_eq!(
        run(
            FULLSCREEN,
            &[b"\x07\xe6\xbc", b"\xa2z"],
            Duration::from_millis(150)
        ),
        cut
    );
}

/// The palette key in the window opens the palette and what follows is typed
/// into its search field, as it is today.
#[test]
fn the_palette_key_in_a_burst_opens_the_palette_and_takes_what_follows() {
    let palette = default_bindings()
        .byte_patterns_for(Action::OpenPalette)
        .into_iter()
        .next()
        .expect("the palette has a default key");
    let want = assert_burst_matches_paced(WINDOWED, &[b"ab", &palette, b"cd"]);
    assert!(want.starts_with("received \"ab\""), "{want}");
}

/// The macro bar key with no macros to show leaves typing with the program.
#[test]
fn the_macro_bar_key_in_a_burst_in_fullscreen_keeps_typing_with_the_program() {
    let macro_bar = default_bindings()
        .byte_patterns_for(Action::OpenMacroBar)
        .into_iter()
        .next()
        .expect("the macro bar has a default key");
    let want = assert_burst_matches_paced(FULLSCREEN, &[b"ab", &macro_bar, b"cd"]);
    assert!(want.starts_with("received \"abcd\""), "{want}");
}

/// With the take-over card up in fullscreen, typing reaches nothing until the
/// take-over key; what follows it in the same write reaches the program once
/// the claim has landed, as it does typed a moment later.
#[test]
fn keys_after_the_take_over_key_reach_the_program_once_it_is_taken_over() {
    let setup = Setup {
        driver: Driver::Browser,
        ..FULLSCREEN
    };
    let take_over = default_bindings()
        .byte_patterns_for(Action::FocusAgent)
        .into_iter()
        .next()
        .expect("the take-over has a default key");
    let want = assert_burst_matches_paced(setup, &[b"ab", &take_over, b"cd"]);
    assert!(
        want.starts_with("received \"cd\"\ntarget Agent\nfullscreen Agent"),
        "{want}"
    );
}
