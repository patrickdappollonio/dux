//! The contract for how the terminal UI reads its own terminal: input is never
//! lost, duplicated, reordered or misrouted because of which reader happened
//! to be reading when it arrived, and a resize wakes every reader's wait.
//!
//! Run the suite with `cargo test -p dux-tui input_handover`. Each case drives
//! the run loop's own input poll against a socket standing in for the
//! terminal, with a real program in a pseudo-terminal behind the agent pane,
//! and compares what the program received and where dux ended up. A burst (all
//! bytes in one read) must end exactly where the same keys typed one at a time
//! end, which is what a person typing by hand gets. To add a case, pick the
//! module for the seam, start a [`Rig`] in the state before the key, and use
//! [`assert_burst_matches_paced`] or [`assert_every_cut_matches`] with explicit
//! expectations on the outcome.

use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dux_core::pty::PtyClient;

use super::test_support::{default_bindings, test_app};
use super::*;

/// Where the agent pane starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Pane {
    Windowed,
    Maximized,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Keys {
    Classic,
    Extended,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Program {
    Plain,
    /// Asks for SGR mouse reports and focus reports.
    ReadsMouse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Driver {
    ThisSurface,
    /// dux serves the web and a browser drives the program, so the take-over
    /// card covers it.
    Browser,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Setup {
    pub(super) pane: Pane,
    pub(super) keys: Keys,
    pub(super) program: Program,
    pub(super) driver: Driver,
}

pub(super) const WINDOWED: Setup = Setup {
    pane: Pane::Windowed,
    keys: Keys::Classic,
    program: Program::Plain,
    driver: Driver::ThisSurface,
};

pub(super) const MAXIMIZED: Setup = Setup {
    pane: Pane::Maximized,
    ..WINDOWED
};

/// The fullscreen toggle's default key, Ctrl-g.
pub(super) const TOGGLE: &[u8] = b"\x07";

/// Written to the program after a case, straight through its pty: once it has
/// come back, everything dux wrote before it has too.
const END_OF_CASE: &[u8] = b"<end of case>";

/// Where a case ended: what the program received, and the state of the
/// interface that decides where the next key goes.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Outcome {
    pub(super) received: String,
    pub(super) target: InputTarget,
    pub(super) fullscreen: FullscreenOverlay,
    pub(super) focus: FocusPane,
    pub(super) prompt: String,
    pub(super) macro_bar: bool,
    pub(super) debugger_lines: usize,
}

/// One app, a stand-in terminal it reads, and a program behind its agent pane.
pub(super) struct Rig {
    pub(super) app: App,
    terminal: UnixStream,
    keyboard: UnixStream,
    received: PathBuf,
    screen: ratatui::Terminal<ratatui::backend::TestBackend>,
    _dir: tempfile::TempDir,
    _serving: Option<Box<dyn std::any::Any>>,
}

impl Rig {
    pub(super) fn new(setup: Setup) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let received = dir.path().join("received");
        let (app, serving) = app_typing_into(&received, setup);
        let (terminal, keyboard) = UnixStream::pair().expect("a socket pair");
        let mut rig = Self {
            app,
            terminal,
            keyboard,
            received,
            screen: ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30))
                .expect("a test screen"),
            _dir: dir,
            _serving: serving,
        };
        rig.draw();
        rig
    }

    /// Bytes arriving from the terminal, in one write.
    pub(super) fn type_bytes(&mut self, bytes: &[u8]) {
        use std::io::Write as _;
        self.keyboard.write_all(bytes).expect("type into dux");
    }

    fn draw(&mut self) {
        let app = &mut self.app;
        self.screen
            .draw(|frame| app.render(frame))
            .expect("draw a frame");
    }

    /// One turn of the run loop: a frame, then one input poll.
    pub(super) fn turn(&mut self) {
        self.draw();
        assert!(
            !self.app.poll_run_input_from(self.terminal.as_fd()),
            "the app asked to exit"
        );
    }

    fn terminal_has_bytes(&self) -> bool {
        input::wait_for_input(&self.terminal.as_fd(), Duration::ZERO).expect("look at the socket")
    }

    /// Whether every byte typed so far has been taken by a reader that can
    /// act on it without more input.
    fn settled(&self) -> bool {
        !self.terminal_has_bytes()
            && !self.app.terminal_input.has_unread()
            && !self.app.raw_input_parser.pending_resolves_on_timeout()
            && !self.app.debug_raw_input.holds_partial_sequence()
    }

    /// Turn the loop until everything typed so far has been read and acted on.
    pub(super) fn settle(&mut self) {
        for _ in 0..40 {
            if self.settled() {
                return;
            }
            self.turn();
        }
        panic!("input never settled");
    }

    pub(super) fn outcome(self) -> Outcome {
        if let Some(client) = self.app.selected_terminal_surface_client() {
            client.write_bytes(END_OF_CASE).expect("mark the end");
        } else {
            std::fs::write(&self.received, END_OF_CASE).expect("mark the end");
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        let received = loop {
            let got = std::fs::read(&self.received).unwrap_or_default();
            if got.ends_with(END_OF_CASE) {
                break got[..got.len() - END_OF_CASE.len()].to_vec();
            }
            assert!(
                Instant::now() < deadline,
                "the program never received the end of the case: {:?}",
                String::from_utf8_lossy(&got)
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        let debugger_lines = match &self.app.prompt {
            PromptState::DebugInput { lines, .. } => lines.len(),
            _ => 0,
        };
        Outcome {
            received: String::from_utf8_lossy(&received).into_owned(),
            target: self.app.input_target,
            fullscreen: self.app.fullscreen_overlay,
            focus: self.app.focus,
            prompt: prompt_name(&self.app.prompt),
            macro_bar: self.app.macro_bar.is_some(),
            debugger_lines,
        }
    }
}

fn prompt_name(prompt: &PromptState) -> String {
    let debug = format!("{prompt:?}");
    debug
        .split([' ', '{', '('])
        .next()
        .unwrap_or_default()
        .to_string()
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
    let (mut app, serving) = if setup.driver == Driver::Browser {
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
        let client = PtyClient::spawn("sh", &["-c".to_string(), script], Path::new("."), 5, 80, 100)
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
    let deadline = Instant::now() + Duration::from_secs(10);
    while !app
        .selected_terminal_surface_client()
        .is_some_and(PtyClient::has_output)
    {
        assert!(Instant::now() < deadline, "the program never said ready");
        std::thread::sleep(Duration::from_millis(5));
    }
    if setup.pane == Pane::Maximized {
        app.input_target = InputTarget::Agent;
        app.fullscreen_overlay = FullscreenOverlay::Agent;
    }
    (app, serving)
}

/// `pieces` typed one at a time, each read and acted on before the next.
pub(super) fn paced(setup: Setup, pieces: &[&[u8]]) -> Outcome {
    let mut rig = Rig::new(setup);
    for piece in pieces {
        rig.type_bytes(piece);
        rig.settle();
    }
    rig.outcome()
}

/// Every byte of `typed` in one read.
pub(super) fn burst(setup: Setup, typed: &[u8]) -> Outcome {
    let mut rig = Rig::new(setup);
    rig.type_bytes(typed);
    rig.settle();
    rig.outcome()
}

/// `typed` in two reads, cut at `at`.
pub(super) fn cut(setup: Setup, typed: &[u8], at: usize) -> Outcome {
    let (first, second) = typed.split_at(at);
    let mut rig = Rig::new(setup);
    rig.type_bytes(first);
    rig.turn();
    rig.type_bytes(second);
    rig.settle();
    rig.outcome()
}

/// The burst of `pieces` ends where typing them one at a time ends. Returns
/// that outcome for the case's own expectations.
pub(super) fn assert_burst_matches_paced(setup: Setup, pieces: &[&[u8]]) -> Outcome {
    let typed = pieces.concat();
    let want = paced(setup, pieces);
    assert_eq!(burst(setup, &typed), want, "one read of {typed:?}");
    want
}

/// `typed` cut into two reads at every byte ends where `want` says. A cut just
/// after an Escape is skipped: there the time between the reads decides what
/// the Escape is, for a person typing as for dux.
pub(super) fn assert_every_cut_matches(setup: Setup, typed: &[u8], want: &Outcome) {
    for at in 1..typed.len() {
        if typed[at - 1] == 0x1b {
            continue;
        }
        assert_eq!(
            &cut(setup, typed, at),
            want,
            "reads {:?} and {:?}",
            &typed[..at],
            &typed[at..]
        );
    }
}

/// A resize wakes the wait for input in every mode, so the screen is redrawn
/// and the background web server serviced while the window is dragged.
mod resize {
    use super::*;

    /// This thread, which a helper thread can send a signal to.
    struct ThisThread(libc::pthread_t);

    // SAFETY: a pthread_t is an identifier; sending it to another thread only
    // lets that thread name this one to pthread_kill.
    unsafe impl Send for ThisThread {}

    /// Send SIGWINCH to this thread every `every` for `lasting`, as a window
    /// dragged by hand does, on a helper thread.
    fn resize_storm(every: Duration, lasting: Duration) -> std::thread::JoinHandle<()> {
        // SAFETY: pthread_self has no preconditions.
        let me = ThisThread(unsafe { libc::pthread_self() });
        std::thread::spawn(move || {
            let me = me;
            let ends = Instant::now() + lasting;
            while Instant::now() < ends {
                std::thread::sleep(every);
                // SAFETY: the target thread is alive until this helper is
                // joined, and SIGWINCH has a handler installed.
                unsafe { libc::pthread_kill(me.0, libc::SIGWINCH) };
            }
        })
    }

    /// Install real resize handlers in `app` for the length of a test.
    fn with_live_signals(app: &mut App) -> SignalHandles {
        let mut handles = register_signal_handles().expect("register the signal handlers");
        app.sigwinch_flag = Arc::clone(&handles.sigwinch_flag);
        app.resize_wake = handles.resize_wake.take();
        handles
    }

    fn unregister(app: &mut App, handles: SignalHandles) {
        if let Some(wake) = app.resize_wake.take() {
            wake.unregister();
        }
        if let Some(id) = handles.sigwinch_sig_id {
            signal_hook::low_level::unregister(id);
        }
        for id in handles.shutdown_sig_ids {
            signal_hook::low_level::unregister(id);
        }
    }

    /// Poll in `rig` while resizes arrive every 20ms for 600ms, and return the
    /// longest single poll. Without a wake, a poll lasts until the storm ends.
    fn longest_poll_under_a_resize_storm(rig: &mut Rig) -> Duration {
        let handles = with_live_signals(&mut rig.app);
        let storm = resize_storm(Duration::from_millis(20), Duration::from_millis(600));
        let mut longest = Duration::ZERO;
        while !storm.is_finished() {
            let started = Instant::now();
            assert!(!rig.app.poll_run_input_from(rig.terminal.as_fd()));
            longest = longest.max(started.elapsed());
        }
        storm.join().expect("the storm thread");
        unregister(&mut rig.app, handles);
        assert!(
            rig.app.sigwinch_flag.load(Ordering::Relaxed),
            "the resize was noted"
        );
        longest
    }

    /// The bound is generous: a woken poll returns within one resize's gap,
    /// and an unwoken one lasts the whole storm.
    const WOKEN_WITHIN: Duration = Duration::from_millis(300);

    #[test]
    fn a_resize_storm_wakes_the_windowed_interface_s_wait() {
        let mut rig = Rig::new(WINDOWED);
        let longest = longest_poll_under_a_resize_storm(&mut rig);
        assert!(longest < WOKEN_WITHIN, "a poll lasted {longest:?}");
    }

    #[test]
    fn a_resize_storm_wakes_the_maximized_pane_s_wait() {
        let mut rig = Rig::new(MAXIMIZED);
        let longest = longest_poll_under_a_resize_storm(&mut rig);
        assert!(longest < WOKEN_WITHIN, "a poll lasted {longest:?}");
    }

    #[test]
    fn a_resize_storm_wakes_the_input_debugger_s_wait() {
        let mut rig = Rig::new(WINDOWED);
        rig.app.prompt = PromptState::DebugInput {
            lines: Vec::new(),
            scroll_offset: 0,
        };
        let longest = longest_poll_under_a_resize_storm(&mut rig);
        assert!(longest < WOKEN_WITHIN, "a poll lasted {longest:?}");
        assert!(rig.app.input_debugger_open());
    }
}
