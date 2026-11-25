//! The contract for how the terminal UI reads its own terminal: input is never
//! duplicated, reordered or misrouted because of which reader happened to be
//! reading when it arrived, and a resize ends every reader's wait at once
//! without a storm of them ever holding a wait past its timeout. A hand-over
//! that a key causes passes every byte after that key on to the next reader.
//! One that no key causes (a dialog a worker opens, a program exiting, another
//! program selected) ends what the reader held unfinished: the start of a key
//! is dropped, and a paste is closed for the program it was going to and its
//! rest dropped up to its end marker, so it never acts as keys. The dropping
//! also ends at the next hand-over with no key, and before the first read
//! that comes `UNFINISHED_PASTE_WAIT` (one second) or more after the last
//! read of `PASTE_SIZED_READ` (256) bytes or more, or after the cut when no
//! read that size has come, or `CUT_PASTE_DROP_LIMIT` (five seconds) or more
//! after the cut; that read is read as keys. Keys typed by hand come in reads
//! far smaller than that, so they are dropped for at most one second after
//! the cut or the last read that size. What the maximized pane holds from
//! while its program was still loading ends the same way when the program
//! first prints, so nothing typed while it loaded reaches it. A held start of
//! the paste start marker three bytes or longer (`ESC [ 2` on) counts as a
//! paste once the bytes after it complete the marker; a held Escape or Alt-[
//! is a key typed by hand and is dropped alone. The end marker goes through
//! the typing gate, so a program in a row another device drives stays in the
//! paste. The windowed interface stays the reader while the place its keys go
//! changes, and ends what it holds the same way when that place changes
//! between polls, which no key it read did: a dialog a worker opens over it,
//! or the program its keys go to exiting. Where its keys go is the server log
//! viewer, else the open dialog, else the startup log viewer, else the macro
//! bar, else the focused pane and input target and, while keys are typed into
//! a program, which program; held input survives every other change, such as
//! a program starting or exiting or focus moving under an open dialog or the
//! macro bar. The
//! `resize` module says how each half is tested: the first by a signal that
//! cannot interrupt the waiting thread, so only the wake can end the wait.
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
    /// Turns bracketed paste on, as shells and the coding CLIs do.
    ReadsPastes,
    /// Prints nothing, so its pane stays in the phase where a program is
    /// still loading.
    Silent,
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

/// How long a case watches the input poll with nothing typed, and the most
/// polls a poll that waits can make in that time. The shortest wait is 33ms,
/// so a waiting poll makes at most four; one that skips its wait makes
/// thousands.
const WAIT_CHECK: Duration = Duration::from_millis(120);
const MOST_POLLS_IN_WAIT_CHECK: usize = 10;

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
    terminal: std::os::fd::OwnedFd,
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
            terminal: terminal.into(),
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

    /// Read `terminal` from now on, at the descriptor number the rig's own
    /// terminal has, as a terminal that changes under a reader does.
    fn swap_terminal(&mut self, terminal: impl AsFd) {
        rustix::io::dup2(terminal, &mut self.terminal).expect("swap the terminal");
    }

    /// The terminal's input ends: every read from now on returns nothing.
    fn hang_up(&mut self) {
        self.keyboard
            .shutdown(std::net::Shutdown::Write)
            .expect("end the terminal's input");
    }

    /// How many input polls the run loop makes in `interval` with nothing
    /// more typed.
    fn polls_in(&mut self, interval: Duration) -> usize {
        let ends = Instant::now() + interval;
        let mut polls = 0;
        while Instant::now() < ends {
            assert!(
                !self.app.poll_run_input_from(self.terminal.as_fd()),
                "the app asked to exit"
            );
            polls += 1;
        }
        polls
    }

    /// Panic unless the run loop's input poll waits when nothing more is
    /// typed.
    fn assert_poll_waits(&mut self) {
        let polls = self.polls_in(WAIT_CHECK);
        assert!(
            polls <= MOST_POLLS_IN_WAIT_CHECK,
            "{polls} input polls in {WAIT_CHECK:?} with nothing typed: a poll skipped its wait"
        );
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
        input::wait_for_input_or_wake(self.terminal.as_fd(), None, Duration::ZERO)
            .expect("look at the socket")
            .input
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

    /// Where the case ended, once the poll has been seen to wait: every case
    /// checks that what it left behind does not make the loop spin.
    pub(super) fn outcome(mut self) -> Outcome {
        self.assert_poll_waits();
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

/// The shell script of a program that says ready and writes what it receives
/// to `received`.
fn program_script(received: &Path, program: Program) -> String {
    let enable = match program {
        Program::Plain => "",
        Program::ReadsMouse => "\\033[?1000h\\033[?1006h\\033[?1004h",
        Program::ReadsPastes => "\\033[?2004h",
        Program::Silent => {
            return format!("stty raw -echo; exec cat > '{}'", received.display());
        }
    };
    format!(
        "stty raw -echo; printf '{enable}ready'; exec cat > '{}'",
        received.display()
    )
}

/// A program writing what it receives to `received`, once it has said ready.
fn ready_program(received: &Path, program: Program) -> PtyClient {
    let client = PtyClient::spawn(
        "sh",
        &["-c".to_string(), program_script(received, program)],
        Path::new("."),
        5,
        80,
        100,
    )
    .expect("spawn a program");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !client.has_output() {
        assert!(Instant::now() < deadline, "the program never said ready");
        std::thread::sleep(Duration::from_millis(5));
    }
    client
}

/// The app, and the serving state that has to outlive it when a browser drives.
fn app_typing_into(received: &Path, setup: Setup) -> (App, Option<Box<dyn std::any::Any>>) {
    let script = program_script(received, setup.program);
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
    let deadline = Instant::now() + Duration::from_secs(10);
    // A silent program is ready once its shell has opened the file it writes
    // to, which it does after turning echo off.
    let ready = |app: &App| match setup.program {
        Program::Silent => received.exists(),
        _ => app
            .selected_terminal_surface_client()
            .is_some_and(PtyClient::has_output),
    };
    while !ready(&app) {
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
///
/// Two things are promised and each has its own tests. A resize ends a wait at
/// once, through the wake the signal handler writes to; the cases deliver the
/// signal on a thread that is not the one waiting, so nothing but the wake can
/// end the wait early. A storm of resizes cannot hold a wait past its timeout;
/// that one is about the wait resuming with the time it has left after a
/// signal interrupts it, and the cases send the signal to the waiting thread.
mod resize {
    use std::sync::{Mutex, MutexGuard, PoisonError};

    use super::*;

    /// A signal and its handlers are the process's, and the wake a handler
    /// writes to is every registered one, so the cases take turns.
    fn one_at_a_time() -> MutexGuard<'static, ()> {
        static TURN: Mutex<()> = Mutex::new(());
        TURN.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// This thread, which a helper thread can send a signal to.
    struct ThisThread(libc::pthread_t);

    // SAFETY: a pthread_t is an identifier; sending it to another thread only
    // lets that thread name this one to pthread_kill.
    unsafe impl Send for ThisThread {}

    /// Send SIGWINCH to this thread every `every` for `lasting`, as a window
    /// dragged by hand does, on a helper thread. The signal interrupts this
    /// thread's `poll`, which is what a storm does to a wait.
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

    /// Deliver one SIGWINCH to the process on a thread of its own, after
    /// `delay`, and finish once its handlers have run. The thread polling is
    /// not the one signalled, so its `poll` is not interrupted: only the wake
    /// the handler writes to can end its wait.
    fn resize_elsewhere(delay: Duration) -> std::thread::JoinHandle<()> {
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            // SAFETY: an all-zero sigset_t is a valid value, and sigemptyset sets it below.
            let mut set: libc::sigset_t = unsafe { std::mem::zeroed() };
            // SAFETY: `set` is a valid, initialised signal set.
            unsafe { libc::sigemptyset(&raw mut set) };
            // SAFETY: as above, and SIGWINCH is a valid signal number.
            unsafe { libc::sigaddset(&raw mut set, libc::SIGWINCH) };
            // A thread inherits its spawner's mask, and this one must take the
            // signal itself.
            // SAFETY: `set` is valid and the old mask is not asked for.
            unsafe {
                libc::pthread_sigmask(libc::SIG_UNBLOCK, &raw const set, std::ptr::null_mut())
            };
            // SAFETY: SIGWINCH has a handler installed, so raising it runs that
            // handler on this thread, where it is unblocked, before raise
            // returns, and ends nothing.
            unsafe { libc::raise(libc::SIGWINCH) };
        })
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Wake {
        Working,
        /// The resize wake could not be set up.
        Unavailable,
    }

    /// Install real resize handlers in `app` for the length of a test.
    fn with_live_signals(app: &mut App, wake: Wake) -> SignalHandles {
        let mut handles = match wake {
            Wake::Working => register_signal_handles(),
            Wake::Unavailable => {
                register_signal_handles_with(|| Err(std::io::Error::other("no socket pair")))
            }
        }
        .expect("register the signal handlers");
        assert_eq!(handles.resize_wake.is_some(), wake == Wake::Working);
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

    /// Whether a resize has written to the app's wake and nothing has read it.
    fn wake_is_pending(app: &App) -> bool {
        let wake = app.resize_wake.as_ref().expect("a resize wake");
        input::wait_for_input_or_wake(wake.as_fd(), None, Duration::ZERO)
            .expect("look at the wake")
            .input
    }

    #[derive(Clone, Copy, Debug)]
    enum Mode {
        Windowed,
        Maximized,
        InputDebugger,
    }

    const MODES: [Mode; 3] = [Mode::Windowed, Mode::Maximized, Mode::InputDebugger];

    fn rig_in(mode: Mode) -> Rig {
        let mut rig = Rig::new(match mode {
            Mode::Windowed | Mode::InputDebugger => WINDOWED,
            Mode::Maximized => MAXIMIZED,
        });
        if let Mode::InputDebugger = mode {
            rig.app.prompt = PromptState::DebugInput {
                lines: Vec::new(),
                scroll_offset: 0,
            };
        }
        rig
    }

    /// The input poll in `mode`, started with a resize already delivered and
    /// nothing typed, ends by the wake: it reads the wake and notes the
    /// resize. Nothing but the wake does either, because the signal's own
    /// handler ran before the poll and the flag it set is cleared first.
    fn assert_a_resize_wakes_the_wait_of(mode: Mode) {
        let _turn = one_at_a_time();
        let mut rig = rig_in(mode);
        let handles = with_live_signals(&mut rig.app, Wake::Working);
        resize_elsewhere(Duration::ZERO)
            .join()
            .expect("deliver the resize");
        rig.app.sigwinch_flag.store(false, Ordering::Relaxed);
        assert!(wake_is_pending(&rig.app), "the resize wrote the wake");

        assert!(!rig.app.poll_run_input_from(rig.terminal.as_fd()));

        assert!(
            !wake_is_pending(&rig.app),
            "the poll left the resize's wake unread"
        );
        assert!(
            rig.app.sigwinch_flag.load(Ordering::Relaxed),
            "the poll did not note the resize"
        );
        unregister(&mut rig.app, handles);
    }

    #[test]
    fn a_resize_wakes_the_windowed_interface_s_wait() {
        assert_a_resize_wakes_the_wait_of(Mode::Windowed);
    }

    #[test]
    fn a_resize_wakes_the_maximized_pane_s_wait() {
        assert_a_resize_wakes_the_wait_of(Mode::Maximized);
    }

    #[test]
    fn a_resize_wakes_the_input_debugger_s_wait() {
        assert_a_resize_wakes_the_wait_of(Mode::InputDebugger);
    }

    /// A wait of `LONG_WAIT` in each state the reader can be in ends as soon
    /// as a resize arrives, on a signal that does not interrupt the waiting
    /// thread. A wait that ignores the wake lasts the whole time.
    #[test]
    fn a_resize_ends_a_long_wait_at_once_in_every_reading_state() {
        const LONG_WAIT: Duration = Duration::from_secs(10);
        const ENDED_WITHIN: Duration = Duration::from_secs(3);

        let _turn = one_at_a_time();
        for state in [
            TerminalReading::Reading,
            TerminalReading::Ended,
            TerminalReading::Failed,
        ] {
            let mut rig = Rig::new(WINDOWED);
            let handles = with_live_signals(&mut rig.app, Wake::Working);
            rig.app.terminal_reading = state;
            rig.app.sigwinch_flag.store(false, Ordering::Relaxed);

            let resize = resize_elsewhere(Duration::from_millis(50));
            let started = Instant::now();
            let input = rig
                .app
                .wait_for_terminal(rig.terminal.as_fd(), LONG_WAIT)
                .expect("wait");
            let waited = started.elapsed();
            resize.join().expect("deliver the resize");

            assert!(!input, "{state:?}: nothing was typed");
            assert!(
                waited < ENDED_WITHIN,
                "{state:?}: the wait lasted {waited:?} of {LONG_WAIT:?} with a resize sent"
            );
            assert!(
                rig.app.sigwinch_flag.load(Ordering::Relaxed),
                "{state:?}: the wait did not note the resize"
            );
            unregister(&mut rig.app, handles);
        }
    }

    /// Poll in `rig` while resizes arrive every 20ms, each interrupting the
    /// poll, for as long as the storm lasts, and return the longest single
    /// poll. A wait that started over after each interruption would last until
    /// the storm ends.
    fn longest_poll_under_a_resize_storm(rig: &mut Rig, wake: Wake) -> Duration {
        let handles = with_live_signals(&mut rig.app, wake);
        let storm = resize_storm(Duration::from_millis(20), Duration::from_millis(1500));
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

    /// A poll waits at most 100ms, so a poll that outlasts this was held by
    /// the storm rather than by its own timeout.
    const NOT_HELD_PAST: Duration = Duration::from_millis(500);

    #[test]
    fn a_resize_storm_cannot_hold_a_wait_past_its_timeout_in_any_mode() {
        let _turn = one_at_a_time();
        for mode in MODES {
            let mut rig = rig_in(mode);
            let longest = longest_poll_under_a_resize_storm(&mut rig, Wake::Working);
            assert!(
                longest < NOT_HELD_PAST,
                "{mode:?}: a poll lasted {longest:?}"
            );
            assert_eq!(
                rig.app.input_debugger_open(),
                matches!(mode, Mode::InputDebugger)
            );
        }
    }

    /// Without its resize wake the terminal UI still starts, and a resize
    /// ends no wait early: the poll lasts its own timeout and notes the
    /// resize only then. This pins that bound, not the wake.
    #[test]
    fn without_its_resize_wake_a_resize_is_noticed_when_the_poll_times_out() {
        // The poll's timeout is 100ms; the margin is for the clock's rounding.
        const NOT_BEFORE: Duration = Duration::from_millis(90);

        let _turn = one_at_a_time();
        let mut rig = Rig::new(MAXIMIZED);
        let handles = with_live_signals(&mut rig.app, Wake::Unavailable);
        resize_elsewhere(Duration::ZERO)
            .join()
            .expect("deliver the resize");

        let started = Instant::now();
        assert!(!rig.app.poll_run_input_from(rig.terminal.as_fd()));
        let waited = started.elapsed();
        unregister(&mut rig.app, handles);

        assert!(
            waited >= NOT_BEFORE,
            "the poll ended after {waited:?}, before its timeout"
        );
        assert!(waited < NOT_HELD_PAST, "the poll lasted {waited:?}");
        assert!(
            rig.app.sigwinch_flag.load(Ordering::Relaxed),
            "the resize was noted"
        );
    }

    /// The same without a wake under a storm: the timeout still bounds a poll.
    #[test]
    fn without_its_resize_wake_a_resize_storm_cannot_hold_a_wait_past_its_timeout() {
        let _turn = one_at_a_time();
        let mut rig = Rig::new(MAXIMIZED);
        let longest = longest_poll_under_a_resize_storm(&mut rig, Wake::Unavailable);
        assert!(longest < NOT_HELD_PAST, "a poll lasted {longest:?}");
    }
}

/// Every reader waits out its poll's timeout when nothing it can act on has
/// arrived: idle, holding the start of a key, a character or a paste whose
/// rest has not come, or reading a terminal whose input has ended. A poll that
/// skips its wait turns the whole run loop, the background web server's
/// servicing included, as fast as the processor allows.
mod waiting {
    use super::*;

    #[derive(Clone, Copy, Debug)]
    enum Reader {
        Windowed,
        Maximized,
        Debugger,
    }

    const READERS: [Reader; 3] = [Reader::Windowed, Reader::Maximized, Reader::Debugger];

    /// Input whose rest has not arrived: Alt-Shift-O and Alt-[ as a classic
    /// terminal sends them, or the start of a longer key; the first byte of a
    /// three-byte character; a bracketed paste whose end marker has not come.
    const UNFINISHED: [&[u8]; 4] = [b"\x1bO", b"\x1b[", b"\xe6", b"\x1b[200~abc"];

    const INTERVAL: Duration = Duration::from_millis(400);

    /// The most polls in [`INTERVAL`] from a poll that waits at least 33ms.
    const MOST_POLLS: usize = 25;

    fn rig(reader: Reader) -> Rig {
        match reader {
            Reader::Windowed => Rig::new(WINDOWED),
            Reader::Maximized => Rig::new(MAXIMIZED),
            Reader::Debugger => {
                let mut rig = Rig::new(WINDOWED);
                rig.app.prompt = PromptState::DebugInput {
                    lines: Vec::new(),
                    scroll_offset: 0,
                };
                rig
            }
        }
    }

    fn assert_waits(rig: &mut Rig, what: &str) {
        let polls = rig.polls_in(INTERVAL);
        assert!(
            polls <= MOST_POLLS,
            "{what}: {polls} input polls in {INTERVAL:?}"
        );
    }

    #[test]
    fn an_idle_reader_waits() {
        for reader in READERS {
            assert_waits(&mut rig(reader), &format!("{reader:?}, idle"));
        }
    }

    #[test]
    fn a_reader_holding_unfinished_input_waits() {
        for reader in READERS {
            for unfinished in UNFINISHED {
                let mut rig = rig(reader);
                rig.type_bytes(unfinished);
                assert_waits(&mut rig, &format!("{reader:?}, holding {unfinished:?}"));
            }
        }
    }

    #[test]
    fn a_reader_whose_terminal_has_ended_waits() {
        for reader in READERS {
            for typed in [&b""[..], b"\x1b["] {
                let mut rig = rig(reader);
                rig.type_bytes(typed);
                rig.hang_up();
                assert_waits(&mut rig, &format!("{reader:?}, ended after {typed:?}"));
            }
        }
    }

    /// A read that fails with an I/O error is a terminal that has gone for
    /// good, as one read by a process group its session has orphaned is.
    #[test]
    fn a_reader_whose_terminal_reads_an_io_error_waits() {
        for reader in READERS {
            let mut rig = rig(reader);
            rig.swap_terminal(pty_reading_an_io_error());
            assert_waits(&mut rig, &format!("{reader:?}, reading an I/O error"));
            assert_eq!(
                rig.app.terminal_reading,
                TerminalReading::Ended,
                "{reader:?}: the input ended"
            );
        }
    }

    /// A terminal the wait reports invalid, as macOS's `poll` reports the
    /// controlling terminal, ends the input unread: a read could block for good.
    #[test]
    fn a_terminal_the_wait_cannot_watch_ends_the_input_unread() {
        // SAFETY: no descriptor is open at this number, so the wait reports it
        // invalid and nothing reads or closes it.
        let unwatchable = unsafe { std::os::fd::BorrowedFd::borrow_raw(1 << 30) };
        let mut rig = Rig::new(WINDOWED);
        let started = Instant::now();
        let input = rig
            .app
            .wait_for_terminal(unwatchable, Duration::from_secs(10))
            .expect("wait");
        assert!(!input, "an unwatchable terminal was taken for input");
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(rig.app.terminal_reading, TerminalReading::Ended);
    }

    /// A read error that does not say the terminal has gone is retried after
    /// a wait, and once the terminal reads again its input arrives.
    #[test]
    fn a_reader_whose_read_fails_waits_and_reads_again() {
        use crate::app::input::READ_FAILS_WITH;
        for reader in READERS {
            let mut failing = rig(reader);
            READ_FAILS_WITH.set(Some(rustix::io::Errno::INVAL));
            failing.type_bytes(b"x");
            assert_waits(&mut failing, &format!("{reader:?}, failing reads"));
            assert!(
                failing.app.terminal_reading != TerminalReading::Ended,
                "{reader:?}: the input goes on"
            );
            READ_FAILS_WITH.set(None);
            failing.settle();
            let mut never_failed = rig(reader);
            never_failed.type_bytes(b"x");
            never_failed.settle();
            assert_eq!(failing.outcome(), never_failed.outcome(), "{reader:?}");
        }
    }

    /// The leading end of a pseudo-terminal whose other end is closed, which
    /// Linux reads as an I/O error and macOS as the end of the input.
    fn pty_reading_an_io_error() -> std::os::fd::OwnedFd {
        use std::os::fd::FromRawFd as _;
        let (mut leader, mut follower) = (-1, -1);
        // SAFETY: openpty writes two descriptors into the integers it is
        // given and reads nothing through its null arguments. They are
        // `null_mut` because macOS declares the last two `*mut`, which a
        // `*const` parameter on Linux accepts too.
        let opened = unsafe {
            libc::openpty(
                &raw mut leader,
                &raw mut follower,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        assert_eq!(opened, 0, "openpty");
        // SAFETY: openpty just returned both descriptors, owned by nobody else.
        drop(unsafe { std::os::fd::OwnedFd::from_raw_fd(follower) });
        // SAFETY: as above.
        unsafe { std::os::fd::OwnedFd::from_raw_fd(leader) }
    }

    /// A lone Escape that ends a read of [`READ_SIZE`](crate::terminal_input::READ_SIZE)
    /// bytes is held for the next read, as crossterm holds it, however long
    /// that read takes: with the next byte it is Alt-x.
    #[test]
    fn a_lone_escape_ending_a_full_read_waits_for_the_next_read() {
        let mut typed = vec![b'a'; crate::terminal_input::READ_SIZE - 1];
        typed.push(0x1b);
        let mut rig = Rig::new(WINDOWED);
        rig.type_bytes(&typed);
        rig.turn();
        assert_waits(&mut rig, "holding the Escape that ends a full read");
        assert_eq!(rig.app.terminal_input.begun_sequence(), b"\x1b");
        rig.type_bytes(b"x");
        rig.settle();
        let received = rig.outcome().received;
        assert!(received.ends_with("a\x1bx"), "{:?}", &received[1000..]);
    }

    /// The maximized reader decides a lone Escape by waiting out the Escape
    /// timeout and looking again, whatever the read before it held: it has no
    /// "the read was full" rule of its own, and the one the windowed reader
    /// keeps is not carried across the hand-over (the maximized pane reads
    /// 4096 bytes at a time, the windowed one 1024). So a lone Escape that
    /// ends a full read there is an Escape key, exactly as one that ends a
    /// short read is, and the bytes the program receives are the same.
    #[test]
    fn a_lone_escape_ending_a_full_read_is_the_maximized_pane_s_escape_key() {
        let escape_then_x = |setup: Setup, before: &[u8]| {
            let mut full = before.to_vec();
            full.push(0x1b);
            let mut rig = Rig::new(setup);
            rig.type_bytes(&full);
            rig.settle();
            assert!(!rig.app.terminal_input.has_unread());
            assert!(!rig.app.raw_input_parser.pending_resolves_on_timeout());
            rig.type_bytes(b"x");
            rig.settle();
            let after_a_full_read = rig.outcome();
            let after_a_short_read = paced(setup, &[before, b"\x1b", b"x"]);
            assert_eq!(after_a_full_read, after_a_short_read);
            assert!(after_a_full_read.received.ends_with("a\x1bx"));
        };

        // The windowed read is full at 1024 bytes and hands the rest after its
        // maximize key to the maximized reader.
        let mut handed_over = vec![TOGGLE[0]];
        handed_over.extend(vec![b'a'; crate::terminal_input::READ_SIZE - 2]);
        escape_then_x(WINDOWED, &handed_over);
        // The maximized reader's own read is full at 4096.
        escape_then_x(MAXIMIZED, &[b'a'; 4095]);
    }

    /// The maximized pane waits out the Escape timeout before deciding a lone
    /// Escape and reads once more; an event in that read that asks dux to
    /// quit ends the poll with the answer to quit. The event comes from a
    /// test byte, since no key typed into the pane quits dux; it is typed on
    /// another thread a third of the way into the wait, and a case where it
    /// lands after the wait's read is run again.
    #[test]
    fn a_quit_read_after_the_escape_wait_ends_the_poll() {
        use crate::app::input::{ESC_AMBIGUITY_TIMEOUT, RAW_BYTE_ASKS_TO_QUIT};
        use std::io::Write as _;

        for _ in 0..20 {
            let mut rig = Rig::new(MAXIMIZED);
            rig.type_bytes(b"\x1b");
            let mut keyboard = rig.keyboard.try_clone().expect("the keyboard");
            let typist = std::thread::spawn(move || {
                std::thread::sleep(ESC_AMBIGUITY_TIMEOUT / 3);
                keyboard.write_all(b"q").expect("type q");
            });
            RAW_BYTE_ASKS_TO_QUIT.set(Some(b'q'));
            let quit = rig.app.poll_run_input_from(rig.terminal.as_fd());
            RAW_BYTE_ASKS_TO_QUIT.set(None);
            typist.join().expect("the typist");
            if !quit && rig.terminal_has_bytes() {
                continue;
            }
            assert!(
                quit,
                "the read after the Escape wait asked to quit and the poll went on"
            );
            return;
        }
        panic!("the byte never landed inside the Escape wait");
    }

    /// The status screen's turn reads what is ready and returns: the serve
    /// loop that calls it does the waiting.
    #[test]
    fn the_status_screen_s_turn_returns_with_unfinished_input_or_an_ended_terminal() {
        use crate::server_screen::{ScreenKeys, read_screen_keys};

        let _turn = super::status_screen::take_turn();
        for unfinished in UNFINISHED {
            for ended in [false, true] {
                let (terminal, mut keyboard) = UnixStream::pair().expect("a socket pair");
                std::io::Write::write_all(&mut keyboard, unfinished).expect("type");
                if ended {
                    keyboard
                        .shutdown(std::net::Shutdown::Write)
                        .expect("end the input");
                }
                let (done, finished) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let mut keys = ScreenKeys::unwatched();
                    for _ in 0..3 {
                        assert_eq!(
                            read_screen_keys(&mut keys, terminal.as_fd(), |_| None),
                            None
                        );
                    }
                    drop(keys);
                    let _ = done.send(());
                });
                finished
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap_or_else(|_| {
                        panic!("a turn holding {unfinished:?} (ended {ended}) never returned")
                    });
                drop(crate::terminal_input::take_from_last_surface());
            }
        }
    }
}

/// Keys around the fullscreen toggle, which hands input from the windowed
/// interface's reader to the maximized pane's and back.
mod maximize_and_back {
    use super::*;

    #[test]
    fn text_around_the_maximize_key_reaches_the_program_in_order() {
        let want = assert_burst_matches_paced(WINDOWED, &[b"ab", TOGGLE, b"cd"]);
        assert_eq!(want.received, "abcd");
        assert_eq!(
            (want.target, want.fullscreen),
            (InputTarget::Agent, FullscreenOverlay::Agent)
        );
        assert_every_cut_matches(WINDOWED, b"ab\x07cd", &want);
    }

    #[test]
    fn text_around_the_leave_key_reaches_the_program_in_order() {
        let want = assert_burst_matches_paced(MAXIMIZED, &[b"ab", TOGGLE, b"cd"]);
        assert_eq!(want.received, "abcd");
        assert_eq!(
            (want.target, want.fullscreen),
            (InputTarget::None, FullscreenOverlay::None)
        );
        assert_every_cut_matches(MAXIMIZED, b"ab\x07cd", &want);
    }

    #[test]
    fn maximize_leave_and_maximize_again_in_one_read() {
        let want =
            assert_burst_matches_paced(WINDOWED, &[b"a", TOGGLE, b"b", TOGGLE, b"c", TOGGLE, b"d"]);
        assert_eq!(want.received, "abcd");
        assert_eq!(want.fullscreen, FullscreenOverlay::Agent);
    }

    /// Ctrl-Shift-g, Ctrl-i and Ctrl-m reach the program as their classic
    /// bytes and the pane stays maximized; a Super chord with no classic form
    /// is dropped.
    #[test]
    fn extended_chords_after_the_maximize_key_reach_the_program_in_their_classic_form() {
        let setup = Setup {
            keys: Keys::Extended,
            ..WINDOWED
        };
        let pieces: &[&[u8]] = &[
            TOGGLE,
            b"\x1b[103;6u",
            b"\x1b[105;5u",
            b"\x1b[109;5u",
            b"\x1b[103;9u",
            b"z",
        ];
        let want = assert_burst_matches_paced(setup, pieces);
        assert_eq!(want.received, "\x07\t\rz");
        assert_eq!(want.fullscreen, FullscreenOverlay::Agent);
        assert_every_cut_matches(setup, &pieces.concat(), &want);
    }

    /// With extended keys off the maximized pane forwards a `CSI … u` as it came.
    #[test]
    fn chords_after_the_maximize_key_with_extended_keys_off_are_forwarded_as_sent() {
        let want = assert_burst_matches_paced(WINDOWED, &[TOGGLE, b"\x1b[103;6u", b"z"]);
        assert_eq!(want.received, "\x1b[103;6uz");
    }

    #[test]
    fn extended_chords_after_the_leave_key_reach_the_windowed_interface() {
        let setup = Setup {
            keys: Keys::Extended,
            ..MAXIMIZED
        };
        let pieces: &[&[u8]] = &[TOGGLE, b"\x1b[105;5u", b"\x1b[109;5u", b"z"];
        let want = assert_burst_matches_paced(setup, pieces);
        assert_every_cut_matches(setup, &pieces.concat(), &want);
    }

    /// The program never enabled bracketed paste, so the maximized pane sends
    /// the paste's line breaks as carriage returns: loose keys would keep the
    /// line feed.
    #[test]
    fn a_paste_just_after_the_maximize_key_arrives_whole() {
        let pieces: &[&[u8]] = &[TOGGLE, b"\x1b[200~p1\np2\x1b[201~", b"z"];
        let want = assert_burst_matches_paced(WINDOWED, pieces);
        assert_eq!(want.received, "p1\rp2z");
        assert_every_cut_matches(WINDOWED, &pieces.concat(), &want);
    }

    /// A toggle inside a paste is pasted text, before the switch as after it.
    #[test]
    fn a_paste_that_holds_the_toggle_is_one_paste() {
        let pieces: &[&[u8]] = &[b"\x1b[200~a\x07b\x1b[201~", TOGGLE, b"c"];
        let want = assert_burst_matches_paced(WINDOWED, pieces);
        assert_eq!(want.received, "a\x07bc");
        assert_eq!(want.fullscreen, FullscreenOverlay::Agent);
        assert_every_cut_matches(WINDOWED, &pieces.concat(), &want);
    }

    #[test]
    fn a_paste_just_after_the_leave_key_arrives_whole() {
        let pieces: &[&[u8]] = &[TOGGLE, b"\x1b[200~q1\nq2\x1b[201~"];
        let want = assert_burst_matches_paced(MAXIMIZED, pieces);
        assert_eq!(want.received, "q1\rq2");
        assert_every_cut_matches(MAXIMIZED, &pieces.concat(), &want);
    }

    #[test]
    fn a_mouse_click_and_focus_reports_straddling_the_maximize_key() {
        let setup = Setup {
            program: Program::ReadsMouse,
            ..WINDOWED
        };
        let pieces: &[&[u8]] = &[
            b"\x1b[I",
            TOGGLE,
            b"\x1b[<0;20;10M",
            b"\x1b[<0;20;10m",
            b"\x1b[O",
        ];
        let want = assert_burst_matches_paced(setup, pieces);
        assert!(want.received.contains("[<0;"), "{want:?}");
        assert_every_cut_matches(setup, &pieces.concat(), &want);
    }

    #[test]
    fn a_mouse_click_and_focus_reports_straddling_the_leave_key() {
        let setup = Setup {
            program: Program::ReadsMouse,
            ..MAXIMIZED
        };
        let pieces: &[&[u8]] = &[
            b"\x1b[I",
            TOGGLE,
            b"\x1b[<0;20;10M",
            b"\x1b[<0;20;10m",
            b"\x1b[O",
        ];
        let want = assert_burst_matches_paced(setup, pieces);
        assert_every_cut_matches(setup, &pieces.concat(), &want);
    }

    #[test]
    fn a_lone_escape_just_after_the_maximize_key_reaches_the_program() {
        let want = assert_burst_matches_paced(WINDOWED, &[TOGGLE, b"\x1b"]);
        assert_eq!(want.received, "\x1b");
    }

    #[test]
    fn a_lone_escape_just_after_the_leave_key_reaches_the_windowed_interface() {
        let want = assert_burst_matches_paced(MAXIMIZED, &[TOGGLE, b"\x1b"]);
        assert_eq!(
            want.received, "\x1b",
            "the windowed pane types into the program"
        );
        assert_eq!(want.fullscreen, FullscreenOverlay::None);
    }

    #[test]
    fn an_arrow_key_after_either_key_arrives_whole_however_the_reads_cut_it() {
        for setup in [WINDOWED, MAXIMIZED] {
            let want = assert_burst_matches_paced(setup, &[TOGGLE, b"\x1b[A", b"x"]);
            assert_every_cut_matches(setup, b"\x07\x1b[Ax", &want);
        }
    }

    #[test]
    fn non_latin_text_crosses_either_key_whole_however_the_reads_cut_it() {
        for setup in [WINDOWED, MAXIMIZED] {
            let pieces: &[&[u8]] = &["漢".as_bytes(), TOGGLE, "字é".as_bytes()];
            let want = assert_burst_matches_paced(setup, pieces);
            assert_every_cut_matches(setup, &pieces.concat(), &want);
        }
        let want = paced(WINDOWED, &["漢".as_bytes(), TOGGLE, "字é".as_bytes()]);
        assert_eq!(want.received, "漢字é");
    }

    /// Nothing is left in any reader's queue once a burst has been read.
    #[test]
    fn nothing_is_left_held_after_a_burst_across_either_key() {
        for setup in [WINDOWED, MAXIMIZED] {
            let mut rig = Rig::new(setup);
            rig.type_bytes(b"a\x07b\x07c\x1b[A");
            rig.settle();
            assert!(rig.app.terminal_input.is_empty());
            assert_eq!(rig.app.raw_input_parser.pending(), b"");
            assert_eq!(rig.app.raw_input_buf, b"");
            assert!(!rig.app.debug_raw_input.holds_partial_sequence());
        }
    }

    const LOADING: Setup = Setup {
        program: Program::Silent,
        ..MAXIMIZED
    };

    /// While the program has printed nothing yet the maximized pane drops what
    /// is typed at it, but keys read with the leave key, after it, reach the
    /// windowed interface as they do typed a moment later: here the palette
    /// key opens the palette.
    #[test]
    fn keys_after_the_leave_key_while_the_program_loads_reach_the_windowed_interface() {
        let palette = default_bindings()
            .byte_patterns_for(Action::OpenPalette)
            .into_iter()
            .next()
            .expect("the palette has a default key");
        let pieces: &[&[u8]] = &[b"ab", TOGGLE, &palette];
        let want = assert_burst_matches_paced(LOADING, pieces);
        assert_eq!(want.prompt, "Command");
        assert_eq!(want.fullscreen, FullscreenOverlay::None);
        assert_eq!(want.received, "");
        assert_every_cut_matches(LOADING, &pieces.concat(), &want);
    }

    /// A paste that holds the leave key while the program loads is one paste:
    /// the pane stays maximized and nothing of it acts in the window.
    #[test]
    fn a_paste_that_holds_the_leave_key_while_the_program_loads_is_one_paste() {
        let typed = b"\x1b[200~a\x07b\r\x1b[201~";
        let want = burst(LOADING, typed);
        assert_eq!(want.fullscreen, FullscreenOverlay::Agent);
        assert_eq!(want.prompt, "None");
        assert_eq!(want.received, "");
        assert_every_cut_matches(LOADING, typed, &want);
    }

    /// A lone Escape typed while the program loads is dropped when the wait
    /// that decides it ends, as is what is typed after it.
    #[test]
    fn a_lone_escape_typed_while_the_program_loads_never_reaches_it() {
        let mut rig = Rig::new(LOADING);
        rig.type_bytes(b"\x1b");
        for _ in 0..5 {
            rig.turn();
        }
        rig.type_bytes(b"ab");
        for _ in 0..5 {
            rig.turn();
        }
        let outcome = rig.outcome();
        assert_eq!(outcome.received, "");
        assert_eq!(outcome.fullscreen, FullscreenOverlay::Agent);
    }

    /// A program that prints nothing until `go` exists and then writes what
    /// it receives to the rig's file, put behind the agent pane in place of
    /// the one there; returns `go`.
    fn a_program_that_loads_until_told(rig: &mut Rig) -> PathBuf {
        let go = rig.received.with_file_name("go");
        let script = format!(
            "stty raw -echo; while [ ! -e '{}' ]; do sleep 0.01; done; \
             printf ready; exec cat > '{}'",
            go.display(),
            rig.received.display()
        );
        let client = PtyClient::spawn(
            "sh",
            &["-c".to_string(), script],
            Path::new("."),
            5,
            80,
            100,
        )
        .expect("spawn a program");
        let tab = TabId::new(rig.app.engine.sessions[0].slot_tab_id().to_string());
        rig.app.engine.providers.insert(tab, client);
        go
    }

    /// The program `a_program_that_loads_until_told` put behind the pane
    /// prints, ending the phase where it is still loading.
    fn the_program_finishes_loading(rig: &Rig, go: &Path) {
        std::fs::write(go, b"").expect("tell the program to print");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !rig
            .app
            .selected_terminal_surface_client()
            .is_some_and(PtyClient::has_output)
        {
            assert!(Instant::now() < deadline, "the program never printed");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Alt-[ typed while the program loads, held for the key that may follow,
    /// is dropped when the program prints: the next key typed reaches the
    /// program as itself.
    #[test]
    fn an_alt_bracket_typed_while_the_program_loads_never_joins_the_key_after_it() {
        let mut rig = Rig::new(LOADING);
        let go = a_program_that_loads_until_told(&mut rig);
        rig.type_bytes(b"\x1b[");
        rig.turn();
        assert_eq!(rig.app.raw_input_parser.pending(), b"\x1b[");
        the_program_finishes_loading(&rig, &go);
        rig.type_bytes(b"A");
        rig.settle();
        let outcome = rig.outcome();
        assert_eq!(outcome.received, "A");
        assert_eq!(outcome.fullscreen, FullscreenOverlay::Agent);
    }

    /// A lone Escape typed while the program loads and still held when the
    /// program prints, before the wait that decides it has ended, never
    /// reaches the program.
    #[test]
    fn an_escape_typed_while_the_program_loads_never_reaches_it_once_it_prints() {
        let mut rig = Rig::new(LOADING);
        let go = a_program_that_loads_until_told(&mut rig);
        rig.type_bytes(b"\x1b");
        rig.turn();
        assert_eq!(rig.app.raw_input_parser.pending(), b"\x1b");
        the_program_finishes_loading(&rig, &go);
        rig.settle();
        rig.type_bytes(b"x");
        rig.settle();
        let outcome = rig.outcome();
        assert_eq!(outcome.received, "x");
        assert_eq!(outcome.fullscreen, FullscreenOverlay::Agent);
    }
}

/// Keys around a key that opens a dialog or an overlay, or takes over the pane
/// from another device, in the same read.
mod dialogs_and_take_over {
    use super::*;

    fn default_key(action: Action) -> Vec<u8> {
        default_bindings()
            .byte_patterns_for(action)
            .into_iter()
            .next()
            .expect("the action has a default key")
    }

    /// The palette key in the window opens the palette and what follows is
    /// typed into its search field.
    #[test]
    fn the_palette_key_opens_the_palette_and_takes_what_follows() {
        let palette = default_key(Action::OpenPalette);
        let pieces: &[&[u8]] = &[b"ab", &palette, b"cd"];
        let want = assert_burst_matches_paced(WINDOWED, pieces);
        assert_eq!(want.received, "ab");
        assert_eq!(want.prompt, "Command");
        assert_every_cut_matches(WINDOWED, &pieces.concat(), &want);
    }

    /// The macro bar key with no macros to show leaves typing with the program.
    #[test]
    fn the_macro_bar_key_with_nothing_to_show_keeps_typing_with_the_program() {
        let macro_bar = default_key(Action::OpenMacroBar);
        let pieces: &[&[u8]] = &[b"ab", &macro_bar, b"cd"];
        let want = assert_burst_matches_paced(MAXIMIZED, pieces);
        assert_eq!(want.received, "abcd");
        assert_every_cut_matches(MAXIMIZED, &pieces.concat(), &want);
    }

    /// The macro bar key with nothing to show ends the maximized pane's batch
    /// and hands the rest of its read on; bytes the pane reads after that
    /// read, here because the typing is longer than one read, reach the
    /// program after the rest that was handed on.
    #[test]
    fn keys_in_the_read_after_one_the_macro_bar_key_ended_reach_the_program_after_it() {
        let macro_bar = default_key(Action::OpenMacroBar);
        let long: Vec<u8> = (0..2 * 4096)
            .map(|i| b'a' + u8::try_from(i % 26).expect("a letter"))
            .collect();
        let pieces: &[&[u8]] = &[b"ab", &macro_bar, &long];
        let typed = [&b"ab"[..], &long].concat();
        for (how, outcome) in [
            ("typed one at a time", paced(MAXIMIZED, pieces)),
            ("in one write", burst(MAXIMIZED, &pieces.concat())),
        ] {
            let received = outcome.received.as_bytes();
            let first_difference = received
                .iter()
                .zip(&typed)
                .position(|(got, sent)| got != sent);
            assert!(
                received == typed,
                "{how}: the program received {} of {} bytes, the first out of place at {:?}",
                received.len(),
                typed.len(),
                first_difference
            );
            assert_eq!(outcome.fullscreen, FullscreenOverlay::Agent, "{how}");
        }
    }

    /// With the take-over card up in fullscreen, typing reaches nothing until
    /// the take-over key; what follows it in the same read reaches the program
    /// once the claim has landed, as it does typed a moment later.
    #[test]
    fn keys_after_the_take_over_key_reach_the_program_once_it_is_taken_over() {
        let setup = Setup {
            driver: Driver::Browser,
            ..MAXIMIZED
        };
        let take_over = default_key(Action::FocusAgent);
        let pieces: &[&[u8]] = &[b"ab", &take_over, b"cd"];
        let want = assert_burst_matches_paced(setup, pieces);
        assert_eq!(want.received, "cd");
        assert_eq!(
            (want.target, want.fullscreen),
            (InputTarget::Agent, FullscreenOverlay::Agent)
        );
        assert_every_cut_matches(setup, &pieces.concat(), &want);
    }

    /// With the take-over card up in fullscreen, the palette key opens the
    /// palette, and a key read just after it, in the next read the maximized
    /// pane takes within its frame, reaches the palette: here Escape, which
    /// closes it. The palette key ends the pane's own read buffer, so the
    /// Escape comes in a read of its own.
    #[test]
    fn a_key_in_the_read_after_the_palette_key_reaches_the_palette() {
        let setup = Setup {
            driver: Driver::Browser,
            ..MAXIMIZED
        };
        let palette = default_key(Action::OpenPalette);
        let filler = vec![b'a'; 4096 - palette.len()];
        let pieces: &[&[u8]] = &[&filler, &palette, b"\x1b"];
        let want = assert_burst_matches_paced(setup, pieces);
        assert_eq!(want.prompt, "None", "the Escape closed the palette");
    }

    /// A paste begun in the read that carries the take-over key, its line
    /// break cut across two reads, arrives as one paste whose line breaks
    /// are the program's, however the reads cut it.
    #[test]
    fn a_paste_after_the_take_over_key_arrives_whole_however_the_reads_cut_it() {
        let setup = Setup {
            driver: Driver::Browser,
            ..MAXIMIZED
        };
        let take_over = default_key(Action::FocusAgent);
        let pieces: &[&[u8]] = &[&take_over, b"\x1b[200~a\r", b"\nb\x1b[201~"];
        let want = assert_burst_matches_paced(setup, pieces);
        assert_eq!(want.received, "a\rb");
        assert_every_cut_matches(setup, &pieces.concat(), &want);
    }
}

/// The maximized pane's reader stopping for a reason other than a key it
/// read, such as the agent's tab exiting.
mod hand_overs_without_a_key {
    use super::*;

    /// The start of a key the maximized pane holds when its tab exits is
    /// dropped with the rest of its typing state, and what follows reaches
    /// the windowed interface on its own. That is what dux did before the
    /// terminal UI read its own terminal, pinned so a change is deliberate:
    /// handing it on instead would turn Alt-[ followed by a key, typed by
    /// hand, into some other key.
    #[test]
    fn the_start_of_a_key_held_when_the_tab_exits_is_dropped() {
        let mut rig = Rig::new(MAXIMIZED);
        rig.type_bytes(b"\x1b[");
        rig.turn();
        assert_eq!(rig.app.raw_input_parser.pending(), b"\x1b[");
        rig.app.reset_raw_input_state();
        rig.app.fullscreen_overlay = FullscreenOverlay::None;
        rig.type_bytes(b"x");
        rig.settle();
        let outcome = rig.outcome();
        assert_eq!(
            outcome.received, "x",
            "the windowed pane types into the program"
        );
    }

    /// The dialog a config reload that failed opens, as the worker's answer
    /// opens it.
    fn a_worker_opens_a_dialog(rig: &mut Rig) {
        rig.app.apply_reaction(
            dux_core::engine::EventReaction::OpenConfigReloadFailedModal(
                "the config does not parse".to_string(),
            ),
        );
        assert_eq!(prompt_name(&rig.app.prompt), "ConfigReloadFailed");
    }

    /// Escape, in a read of its own, closes the dialog.
    fn close_the_dialog(rig: &mut Rig) {
        rig.type_bytes(b"\x1b");
        rig.settle();
        assert_eq!(prompt_name(&rig.app.prompt), "None");
    }

    /// The program behind the agent pane ends and the run loop's sweep sees it.
    fn the_agent_exits(rig: &mut Rig) {
        let tab = TabId::new(rig.app.engine.sessions[0].slot_tab_id().to_string());
        rig.app.engine.providers[&tab].terminate();
        sweep_until(rig, |app| !app.engine.providers.contains_key(&tab));
    }

    fn sweep_until(rig: &mut Rig, done: impl Fn(&App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(&rig.app) {
            assert!(Instant::now() < deadline, "the program never exited");
            std::thread::sleep(Duration::from_millis(5));
            rig.app.drain_events();
        }
    }

    /// The agent pane maximized again over a program that has just started.
    fn maximize_a_new_agent_program(rig: &mut Rig) {
        let tab = TabId::new(rig.app.engine.sessions[0].slot_tab_id().to_string());
        let client = ready_program(&rig.received, Program::Plain);
        rig.app.engine.providers.insert(tab, client);
        rig.app.focus = FocusPane::Center;
        rig.app.session_surface = crate::model::SessionSurface::Agent;
        rig.app.input_target = InputTarget::Agent;
        rig.app.fullscreen_overlay = FullscreenOverlay::Agent;
    }

    /// Alt-[ as a classic terminal sends it, held by the maximized pane for a
    /// key that may follow, when a worker opens a dialog: the next key typed
    /// into the pane once the dialog is closed is that key, as it is typed
    /// with no Alt-[ before it.
    #[test]
    fn the_start_of_a_key_held_when_a_worker_opens_a_dialog_is_dropped() {
        let mut rig = Rig::new(MAXIMIZED);
        rig.type_bytes(b"\x1b[");
        rig.turn();
        a_worker_opens_a_dialog(&mut rig);
        rig.turn();
        close_the_dialog(&mut rig);
        rig.type_bytes(b"x");
        rig.settle();
        let outcome = rig.outcome();
        assert_eq!(outcome.received, "x");
        assert_eq!(outcome.fullscreen, FullscreenOverlay::Agent);
    }

    /// A paste the maximized pane is in when a worker opens a dialog ends
    /// there: its end marker is dropped, and once the dialog is closed the
    /// fullscreen toggle leaves the pane rather than being pasted.
    #[test]
    fn a_paste_open_when_a_worker_opens_a_dialog_ends_with_the_hand_over() {
        let mut rig = Rig::new(MAXIMIZED);
        rig.type_bytes(b"\x1b[200~abc");
        rig.settle();
        a_worker_opens_a_dialog(&mut rig);
        rig.turn();
        rig.type_bytes(b"\x1b[201~");
        rig.settle();
        close_the_dialog(&mut rig);
        rig.type_bytes(TOGGLE);
        rig.settle();
        let outcome = rig.outcome();
        assert_eq!(outcome.received, "abc");
        assert_eq!(outcome.fullscreen, FullscreenOverlay::None);
    }

    /// The start of a key the maximized pane holds when its agent exits is
    /// not put in front of what is typed into the pane once the agent is
    /// back and the pane maximized again.
    #[test]
    fn the_start_of_a_key_held_when_the_agent_exits_never_reaches_its_next_program() {
        let mut rig = Rig::new(MAXIMIZED);
        rig.type_bytes(b"\x1b[");
        rig.turn();
        the_agent_exits(&mut rig);
        assert_eq!(rig.app.fullscreen_overlay, FullscreenOverlay::None);
        rig.turn();
        maximize_a_new_agent_program(&mut rig);
        rig.type_bytes(b"x");
        rig.settle();
        assert_eq!(rig.outcome().received, "x");
    }

    /// The same for a paste open when the agent exits: the pane maximized
    /// over its next program is not in it, so the toggle leaves the pane.
    #[test]
    fn a_paste_open_when_the_agent_exits_does_not_reach_its_next_program() {
        let mut rig = Rig::new(MAXIMIZED);
        rig.type_bytes(b"\x1b[200~abc");
        rig.settle();
        the_agent_exits(&mut rig);
        rig.turn();
        maximize_a_new_agent_program(&mut rig);
        rig.type_bytes(b"x");
        rig.settle();
        rig.type_bytes(TOGGLE);
        rig.settle();
        let outcome = rig.outcome();
        assert_eq!(outcome.received, "x");
        assert_eq!(outcome.fullscreen, FullscreenOverlay::None);
    }

    /// A terminal in the agent's dock running a program that writes what it
    /// receives to `received`, maximized; returns its id.
    fn maximize_a_dock_terminal(rig: &mut Rig, received: &Path, program: Program) -> String {
        let id = "dock-terminal".to_string();
        rig.app.engine.companion_terminals.insert(
            id.clone(),
            dux_core::model::CompanionTerminal {
                owner: dux_core::model::TerminalOwner::Session("session-1".to_string()),
                label: id.clone(),
                foreground_cmd: None,
                client: ready_program(received, program),
                sort_order: 0,
                created_at: chrono::Utc::now(),
                list_after: None,
            },
        );
        rig.app.rebuild_left_items();
        rig.app.active_terminal_id = Some(id.clone());
        rig.app.session_surface = crate::model::SessionSurface::Terminal;
        rig.app.focus = FocusPane::Center;
        rig.app.input_target = InputTarget::Terminal;
        rig.app.fullscreen_overlay = FullscreenOverlay::Terminal;
        rig.draw();
        assert_eq!(rig.app.selected_terminal_surface_id(), Some(id.clone()));
        id
    }

    /// The start of a key a maximized terminal holds when it exits is not
    /// put in front of what is typed into the agent pane maximized after it.
    #[test]
    fn the_start_of_a_key_held_when_a_terminal_exits_never_reaches_the_agent() {
        let mut rig = Rig::new(WINDOWED);
        let elsewhere = rig.received.with_file_name("terminal received");
        let id = maximize_a_dock_terminal(&mut rig, &elsewhere, Program::Plain);
        rig.type_bytes(b"\x1b[");
        rig.turn();
        assert_eq!(rig.app.raw_input_parser.pending(), b"\x1b[");
        rig.app.engine.companion_terminals[&id].client.terminate();
        sweep_until(&mut rig, |app| {
            !app.engine.companion_terminals.contains_key(&id)
        });
        assert_eq!(rig.app.fullscreen_overlay, FullscreenOverlay::None);
        rig.turn();
        rig.app.session_surface = crate::model::SessionSurface::Agent;
        rig.app.focus = FocusPane::Center;
        rig.app.input_target = InputTarget::Agent;
        rig.app.fullscreen_overlay = FullscreenOverlay::Agent;
        rig.type_bytes(b"x");
        rig.settle();
        assert_eq!(rig.outcome().received, "x");
    }

    /// The start of a key held by the maximized pane of one program is not
    /// put in front of what is typed into another program's maximized pane,
    /// even when no poll ran between them while the pane was not maximized.
    #[test]
    fn the_start_of_a_key_held_for_one_program_never_reaches_another() {
        let mut rig = Rig::new(WINDOWED);
        let elsewhere = rig.received.with_file_name("terminal received");
        maximize_a_dock_terminal(&mut rig, &elsewhere, Program::Plain);
        rig.type_bytes(b"\x1b[");
        rig.turn();
        assert_eq!(rig.app.raw_input_parser.pending(), b"\x1b[");
        rig.app.session_surface = crate::model::SessionSurface::Agent;
        rig.app.input_target = InputTarget::Agent;
        rig.app.fullscreen_overlay = FullscreenOverlay::Agent;
        rig.type_bytes(b"x");
        rig.settle();
        assert_eq!(rig.outcome().received, "x");
    }

    /// Everything `client`'s program has written to `received`, once all that
    /// dux wrote to it before now has arrived.
    fn everything_received(client: &PtyClient, received: &Path) -> String {
        client.write_bytes(END_OF_CASE).expect("mark the end");
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let got = std::fs::read(received).unwrap_or_default();
            if let Some(before) = got.strip_suffix(END_OF_CASE) {
                return String::from_utf8_lossy(before).into_owned();
            }
            assert!(
                Instant::now() < deadline,
                "the program never received the end of the case: {:?}",
                String::from_utf8_lossy(&got)
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    const READS_PASTES: Setup = Setup {
        program: Program::ReadsPastes,
        ..MAXIMIZED
    };

    /// A paste the maximized pane was forwarding with its markers when a
    /// worker opens a dialog is closed for the program there, so what is
    /// typed into the pane once the dialog is closed is typing again.
    #[test]
    fn a_paste_a_dialog_cuts_off_is_closed_for_a_program_that_reads_pastes() {
        let mut rig = Rig::new(READS_PASTES);
        assert!(
            rig.app
                .selected_terminal_surface_client()
                .is_some_and(PtyClient::has_bracketed_paste)
        );
        rig.type_bytes(b"\x1b[200~abc");
        rig.settle();
        a_worker_opens_a_dialog(&mut rig);
        rig.turn();
        rig.type_bytes(b"\x1b[201~");
        rig.settle();
        close_the_dialog(&mut rig);
        rig.type_bytes(b"x\r");
        rig.settle();
        assert_eq!(rig.outcome().received, "\x1b[200~abc\x1b[201~x\r");
    }

    /// The rest of a paste a dialog cut off, up to its end marker, is dropped:
    /// its line break does not answer the dialog, and the program keeps the
    /// part it had before the dialog opened.
    #[test]
    fn the_rest_of_a_paste_a_dialog_cuts_off_never_acts_on_the_dialog() {
        let mut rig = Rig::new(MAXIMIZED);
        rig.type_bytes(b"\x1b[200~abc");
        rig.settle();
        a_worker_opens_a_dialog(&mut rig);
        rig.turn();
        rig.type_bytes(b"de\r");
        rig.settle();
        assert_eq!(prompt_name(&rig.app.prompt), "ConfigReloadFailed");
        rig.type_bytes(b"f\r\x1b[201~");
        rig.settle();
        close_the_dialog(&mut rig);
        rig.type_bytes(b"x");
        rig.settle();
        let outcome = rig.outcome();
        assert_eq!(outcome.received, "abcx");
        assert_eq!(outcome.fullscreen, FullscreenOverlay::Agent);
    }

    /// A paste cut off by another program being selected is closed for the
    /// program it was going to, and none of its rest reaches the program
    /// selected now; what follows its end marker does.
    #[test]
    fn a_paste_cut_off_by_selecting_another_program_ends_in_the_program_it_was_going_to() {
        let mut rig = Rig::new(WINDOWED);
        let elsewhere = rig.received.with_file_name("terminal received");
        let id = maximize_a_dock_terminal(&mut rig, &elsewhere, Program::ReadsPastes);
        rig.type_bytes(b"\x1b[200~abc");
        rig.settle();
        rig.app.session_surface = crate::model::SessionSurface::Agent;
        rig.app.input_target = InputTarget::Agent;
        rig.app.fullscreen_overlay = FullscreenOverlay::Agent;
        rig.type_bytes(b"d\re\x1b[201~x");
        rig.settle();
        let terminal_received =
            everything_received(&rig.app.engine.companion_terminals[&id].client, &elsewhere);
        assert_eq!(terminal_received, "\x1b[200~abc\x1b[201~");
        assert_eq!(rig.outcome().received, "x");
    }

    /// The start of a key the windowed interface holds for a dialog over the
    /// maximized pane is dropped when the dialog closes with no key, so the
    /// next key typed into the pane is that key.
    #[test]
    fn the_start_of_a_key_held_for_a_dialog_that_closes_by_itself_is_dropped() {
        let mut rig = Rig::new(MAXIMIZED);
        a_worker_opens_a_dialog(&mut rig);
        rig.type_bytes(b"\x1b[");
        rig.turn();
        assert_eq!(rig.app.terminal_input.begun_sequence(), b"\x1b[");
        // A dialog closing because of something other than a key.
        rig.app.prompt = PromptState::None;
        rig.type_bytes(b"x");
        rig.settle();
        assert_eq!(rig.outcome().received, "x");
    }

    /// A paste the windowed interface is reading for a dialog over the
    /// maximized pane, when the dialog closes with no key, reaches neither
    /// the dialog nor the program: what follows its end marker does.
    #[test]
    fn a_paste_held_for_a_dialog_that_closes_by_itself_never_reaches_the_program() {
        let mut rig = Rig::new(MAXIMIZED);
        a_worker_opens_a_dialog(&mut rig);
        rig.type_bytes(b"\x1b[200~ab");
        rig.turn();
        assert_eq!(rig.app.terminal_input.begun_sequence(), b"\x1b[200~ab");
        rig.app.prompt = PromptState::None;
        rig.type_bytes(b"c\r\x1b[201~x");
        rig.settle();
        assert_eq!(rig.outcome().received, "x");
    }

    /// A paste the input debugger is counting when a worker opens a dialog
    /// over it is dropped up to its end marker, so its line break does not
    /// answer the dialog.
    #[test]
    fn the_rest_of_a_paste_the_input_debugger_counts_never_acts_on_the_dialog_after_it() {
        let mut rig = Rig::new(WINDOWED);
        rig.app.prompt = PromptState::DebugInput {
            lines: Vec::new(),
            scroll_offset: 0,
        };
        rig.type_bytes(b"\x1b[200~abc");
        rig.settle();
        a_worker_opens_a_dialog(&mut rig);
        rig.turn();
        rig.type_bytes(b"d\re\x1b[201~");
        rig.settle();
        assert_eq!(prompt_name(&rig.app.prompt), "ConfigReloadFailed");
    }

    /// The rest of a paste open when the agent exits is dropped up to its end
    /// marker, so a dux key inside it does nothing in the windowed interface.
    #[test]
    fn the_rest_of_a_paste_open_when_the_agent_exits_never_acts_on_the_window() {
        let mut rig = Rig::new(MAXIMIZED);
        rig.type_bytes(b"\x1b[200~abc");
        rig.settle();
        the_agent_exits(&mut rig);
        rig.turn();
        let palette = default_bindings()
            .byte_patterns_for(Action::OpenPalette)
            .into_iter()
            .next()
            .expect("the palette has a default key");
        rig.type_bytes(&[&b"d"[..], &palette, b"\x1b[201~"].concat());
        rig.settle();
        assert_eq!(prompt_name(&rig.app.prompt), "None");
    }

    /// The start of a paste's start marker, cut off by the read and held by
    /// the maximized pane when a worker opens a dialog, starts a paste as the
    /// whole marker does: the rest of the paste, its line break included,
    /// never acts on the dialog.
    #[test]
    fn a_paste_whose_start_marker_a_dialog_cuts_in_two_never_acts_on_the_dialog() {
        let mut rig = Rig::new(MAXIMIZED);
        rig.type_bytes(b"\x1b[20");
        rig.turn();
        assert_eq!(rig.app.raw_input_parser.pending(), b"\x1b[20");
        a_worker_opens_a_dialog(&mut rig);
        rig.turn();
        rig.type_bytes(b"0~de\r\x1b[201~");
        rig.settle();
        assert_eq!(prompt_name(&rig.app.prompt), "ConfigReloadFailed");
        close_the_dialog(&mut rig);
        rig.type_bytes(b"x");
        rig.settle();
        assert_eq!(rig.outcome().received, "x");
    }

    /// The start of a key that only looked like a paste's start marker is
    /// dropped as any held key start is, and what follows it is read as
    /// keys: here F9 cut after `ESC[20`, then Escape, which closes the
    /// dialog. Neither F9 nor its `~` reaches the program, and a key typed
    /// into the pane after the dialog does.
    #[test]
    fn a_held_key_start_that_is_no_paste_lets_what_follows_act() {
        let mut rig = Rig::new(MAXIMIZED);
        rig.type_bytes(b"\x1b[20");
        rig.turn();
        a_worker_opens_a_dialog(&mut rig);
        rig.turn();
        rig.type_bytes(b"~");
        rig.settle();
        close_the_dialog(&mut rig);
        rig.type_bytes(b"x");
        rig.settle();
        let outcome = rig.outcome();
        assert_eq!(outcome.received, "x");
        assert_eq!(outcome.fullscreen, FullscreenOverlay::Agent);
    }

    /// The same for the start of a paste's start marker the windowed
    /// interface holds for a dialog that closes by itself: the paste reaches
    /// neither the dialog nor the program, and what follows its end marker
    /// does.
    #[test]
    fn a_paste_start_held_for_a_dialog_that_closes_by_itself_never_reaches_the_program() {
        let mut rig = Rig::new(MAXIMIZED);
        a_worker_opens_a_dialog(&mut rig);
        rig.type_bytes(b"\x1b[20");
        rig.turn();
        assert_eq!(rig.app.terminal_input.begun_sequence(), b"\x1b[20");
        rig.app.prompt = PromptState::None;
        rig.type_bytes(b"0~c\r\x1b[201~x");
        rig.settle();
        assert_eq!(rig.outcome().received, "x");
    }

    /// The same for the start of a paste's start marker the input debugger
    /// holds when a worker opens a dialog over it.
    #[test]
    fn a_paste_start_the_input_debugger_holds_never_acts_on_the_dialog_after_it() {
        let mut rig = Rig::new(WINDOWED);
        rig.app.prompt = PromptState::DebugInput {
            lines: Vec::new(),
            scroll_offset: 0,
        };
        rig.type_bytes(b"\x1b[20");
        rig.turn();
        assert!(rig.app.debug_raw_input.holds_partial_sequence());
        a_worker_opens_a_dialog(&mut rig);
        rig.turn();
        rig.type_bytes(b"0~d\re\x1b[201~");
        rig.settle();
        assert_eq!(prompt_name(&rig.app.prompt), "ConfigReloadFailed");
    }

    /// The start of a key the windowed interface holds when a worker opens a
    /// dialog over it is dropped although the reader stays the same, so the
    /// next key typed is that key in the dialog: here Escape, which closes it.
    #[test]
    fn the_start_of_a_key_the_window_holds_when_a_worker_opens_a_dialog_over_it_is_dropped() {
        let mut rig = Rig::new(WINDOWED);
        rig.type_bytes(b"\x1b[");
        rig.turn();
        assert_eq!(rig.app.terminal_input.begun_sequence(), b"\x1b[");
        a_worker_opens_a_dialog(&mut rig);
        rig.turn();
        close_the_dialog(&mut rig);
    }

    /// A paste whose start marker the program's full write queue dropped never
    /// opened a paste in the program, so a worker's dialog sends no end marker
    /// for it.
    #[test]
    fn a_paste_start_a_full_write_queue_dropped_is_never_closed() {
        let mut rig = Rig::new(MAXIMIZED);
        let go = rig.received.with_file_name("go");
        let script = format!(
            "stty raw -echo; printf '\\033[?2004hready'; \
             while [ ! -e '{}' ]; do sleep 0.01; done; exec cat > '{}'",
            go.display(),
            rig.received.display()
        );
        let client = PtyClient::spawn(
            "sh",
            &["-c".to_string(), script],
            Path::new("."),
            5,
            80,
            100,
        )
        .expect("spawn a program");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !client.has_bracketed_paste() {
            assert!(Instant::now() < deadline, "the program never said ready");
            std::thread::sleep(Duration::from_millis(5));
        }
        // The program reads nothing until `go` exists, so its queue fills; it
        // is full for good once a wait frees no room in it.
        loop {
            while client.enqueue_bytes(b"q") {}
            std::thread::sleep(Duration::from_millis(50));
            if !client.enqueue_bytes(b"q") {
                break;
            }
        }
        let tab = TabId::new(rig.app.engine.sessions[0].slot_tab_id().to_string());
        rig.app.engine.providers.insert(tab.clone(), client);
        rig.type_bytes(b"\x1b[200~abc");
        rig.settle();
        std::fs::write(&go, b"").expect("let the program read");
        while !rig.app.engine.providers[&tab].enqueue_bytes(b"") {
            assert!(
                Instant::now() < deadline,
                "the program never read its queue"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
        a_worker_opens_a_dialog(&mut rig);
        rig.turn();
        let received = rig.outcome().received;
        assert!(
            !received.contains('\x1b'),
            "the program received a paste marker: {:?}",
            received.trim_start_matches('q')
        );
    }

    /// A maximized program that reads pastes, in a row a browser drove until
    /// this window took it over.
    fn rig_driving_a_row_a_browser_drove() -> Rig {
        let mut rig = Rig::new(Setup {
            driver: Driver::Browser,
            program: Program::ReadsPastes,
            ..MAXIMIZED
        });
        take_over_from_the_card(&mut rig);
        rig
    }

    /// Press the take-over key and draw the frame that carries the claim.
    fn take_over_from_the_card(rig: &mut Rig) {
        let take_over = default_bindings()
            .byte_patterns_for(Action::FocusAgent)
            .into_iter()
            .next()
            .expect("the take-over key has a default");
        assert!(rig.app.focused_pty_takeover_card().is_some());
        rig.type_bytes(&take_over);
        rig.settle();
        rig.draw();
        assert_eq!(rig.app.focused_pty_takeover_card(), None);
    }

    /// A browser takes the row over from this window.
    fn a_browser_takes_over(rig: &mut Rig) {
        let seat = rig.app.pty_ownership().expect("dux serves the web");
        let browser = seat.owners.next_conn_id();
        seat.owners
            .claim_for_resize(
                "session-1-slot",
                browser,
                dux_core::pty_owners::ResizeIntent::TakeOver {
                    expected_owner: None,
                },
                Some("Chrome"),
                |_| {},
            )
            .epoch
            .expect("the browser took the row over");
        assert!(rig.app.focused_pty_takeover_card().is_some());
    }

    /// A paste this window opened in a program it still drives is closed for
    /// that program when a worker opens a dialog.
    #[test]
    fn a_paste_a_dialog_cuts_off_is_closed_for_a_program_this_window_drives() {
        let mut rig = rig_driving_a_row_a_browser_drove();
        rig.type_bytes(b"\x1b[200~abc");
        rig.settle();
        a_worker_opens_a_dialog(&mut rig);
        rig.turn();
        assert_eq!(rig.outcome().received, "\x1b[200~abc\x1b[201~");
    }

    /// A paste this window opened is not closed for a program whose row a
    /// browser has taken over: the end marker is a write like any other, and
    /// the browser may be in a paste of its own that it would end early.
    #[test]
    fn a_paste_a_dialog_cuts_off_is_left_open_in_a_row_a_browser_took_over() {
        let mut rig = rig_driving_a_row_a_browser_drove();
        rig.type_bytes(b"\x1b[200~abc");
        rig.settle();
        a_browser_takes_over(&mut rig);
        a_worker_opens_a_dialog(&mut rig);
        rig.turn();
        assert!(!rig.app.raw_paste.program_in_paste);
        assert_eq!(rig.outcome().received, "\x1b[200~abc");
    }

    /// A program left inside a paste whose end marker was refused, because a
    /// browser drove the row when it arrived, is closed once this window
    /// drives the row again and the leave key hands input to the window.
    #[test]
    fn a_paste_left_open_by_a_refused_end_is_closed_when_the_leave_key_hands_over() {
        let mut rig = rig_driving_a_row_a_browser_drove();
        rig.type_bytes(b"\x1b[200~abc");
        rig.settle();
        a_browser_takes_over(&mut rig);
        rig.type_bytes(b"de\x1b[201~");
        rig.settle();
        take_over_from_the_card(&mut rig);
        rig.type_bytes(TOGGLE);
        rig.settle();
        let outcome = rig.outcome();
        assert_eq!(outcome.fullscreen, FullscreenOverlay::None);
        assert_eq!(outcome.received, "\x1b[200~abc\x1b[201~");
    }

    /// A cut-off paste whose end marker never comes stops being dropped once
    /// it has gone as long without a byte as any paste may, so a key typed
    /// after that acts.
    #[test]
    fn a_cut_off_paste_whose_end_never_comes_is_dropped_only_until_it_goes_quiet() {
        let mut rig = Rig::new(MAXIMIZED);
        rig.type_bytes(b"\x1b[200~abc");
        rig.settle();
        a_worker_opens_a_dialog(&mut rig);
        rig.turn();
        std::thread::sleep(crate::raw_input::UNFINISHED_PASTE_WAIT + Duration::from_millis(50));
        close_the_dialog(&mut rig);
        rig.type_bytes(b"x");
        rig.settle();
        assert_eq!(rig.outcome().received, "abcx");
    }

    /// The time the rest of a cut-off paste is measured by, stopped from now
    /// on this thread and moved only by the case, so how long a step takes on
    /// a loaded machine changes nothing.
    struct PasteClock {
        now: Instant,
    }

    impl PasteClock {
        fn stopped() -> Self {
            let now = Instant::now();
            PASTE_CLOCK.set(Some(now));
            Self { now }
        }

        fn advance(&mut self, by: Duration) {
            self.now += by;
            PASTE_CLOCK.set(Some(self.now));
        }
    }

    impl Drop for PasteClock {
        fn drop(&mut self) {
            PASTE_CLOCK.set(None);
        }
    }

    /// A maximized paste cut off by a worker's dialog, its end marker still
    /// to come.
    fn a_dialog_cuts_off_a_paste(rig: &mut Rig) {
        rig.type_bytes(b"\x1b[200~abc");
        rig.settle();
        a_worker_opens_a_dialog(rig);
        rig.turn();
    }

    /// `n` bytes of a paste's text, the last a line break, which closes the
    /// dialog if it is read as a key.
    fn pasted_lines(n: usize) -> Vec<u8> {
        let mut text = vec![b'p'; n - 1];
        text.push(b'\r');
        text
    }

    /// Escape typed every half second after a cut-off paste whose end marker
    /// never comes: the Escape typed half a second after the cut is still
    /// taken for the paste, and the one typed a second after it closes the
    /// dialog. Typing keeps nothing dropped past that second.
    #[test]
    fn steady_typing_after_a_cut_off_paste_acts_a_second_after_the_cut() {
        let mut clock = PasteClock::stopped();
        let mut rig = Rig::new(MAXIMIZED);
        a_dialog_cuts_off_a_paste(&mut rig);
        let half = crate::raw_input::UNFINISHED_PASTE_WAIT / 2;
        clock.advance(half);
        rig.type_bytes(b"\x1b");
        rig.settle();
        assert_eq!(prompt_name(&rig.app.prompt), "ConfigReloadFailed");
        clock.advance(half);
        close_the_dialog(&mut rig);
        rig.type_bytes(b"x");
        rig.settle();
        assert_eq!(rig.outcome().received, "abcx");
    }

    /// The rest of a cut-off paste arriving as a slow link delivers it, in
    /// reads of paste size with a stall just short of the quiet wait between
    /// them, is dropped to its end marker: none of its line breaks closes the
    /// dialog.
    #[test]
    fn the_rest_of_a_paste_arriving_with_stalls_is_dropped_to_its_end_marker() {
        let mut clock = PasteClock::stopped();
        let mut rig = Rig::new(MAXIMIZED);
        a_dialog_cuts_off_a_paste(&mut rig);
        let stall = crate::raw_input::UNFINISHED_PASTE_WAIT.mul_f32(0.9);
        let mut waited = Duration::ZERO;
        while waited + stall * 2 < crate::raw_input::CUT_PASTE_DROP_LIMIT {
            clock.advance(stall);
            waited += stall;
            rig.type_bytes(&pasted_lines(crate::raw_input::PASTE_SIZED_READ));
            rig.settle();
            assert_eq!(prompt_name(&rig.app.prompt), "ConfigReloadFailed");
        }
        clock.advance(stall);
        rig.type_bytes(b"q\r\x1b[201~");
        rig.settle();
        assert_eq!(prompt_name(&rig.app.prompt), "ConfigReloadFailed");
        close_the_dialog(&mut rig);
        rig.type_bytes(b"x");
        rig.settle();
        assert_eq!(rig.outcome().received, "abcx");
    }

    /// The rest of a cut-off paste that keeps arriving in reads of paste size
    /// and never brings its end marker is dropped for at most
    /// [`crate::raw_input::CUT_PASTE_DROP_LIMIT`] after the cut: a key read
    /// then acts.
    #[test]
    fn the_rest_of_a_cut_off_paste_is_dropped_for_at_most_its_limit() {
        let mut clock = PasteClock::stopped();
        let mut rig = Rig::new(MAXIMIZED);
        a_dialog_cuts_off_a_paste(&mut rig);
        let step = crate::raw_input::UNFINISHED_PASTE_WAIT / 2;
        let mut waited = Duration::ZERO;
        while waited + step < crate::raw_input::CUT_PASTE_DROP_LIMIT {
            clock.advance(step);
            waited += step;
            rig.type_bytes(&pasted_lines(crate::raw_input::PASTE_SIZED_READ));
            rig.settle();
            assert_eq!(prompt_name(&rig.app.prompt), "ConfigReloadFailed");
        }
        clock.advance(crate::raw_input::CUT_PASTE_DROP_LIMIT.saturating_sub(waited));
        close_the_dialog(&mut rig);
    }

    /// Alt-[ typed at one maximized program and held for the key that may
    /// follow, then another program maximized: a 2 typed next reaches the
    /// new program, because a hand-typed Alt-[ is never taken for the start
    /// of a paste.
    #[test]
    fn a_key_typed_after_a_dropped_alt_bracket_reaches_the_next_program() {
        let mut rig = Rig::new(WINDOWED);
        let elsewhere = rig.received.with_file_name("terminal received");
        maximize_a_dock_terminal(&mut rig, &elsewhere, Program::Plain);
        rig.type_bytes(b"\x1b[");
        rig.turn();
        assert_eq!(rig.app.raw_input_parser.pending(), b"\x1b[");
        rig.app.session_surface = crate::model::SessionSurface::Agent;
        rig.app.input_target = InputTarget::Agent;
        rig.app.fullscreen_overlay = FullscreenOverlay::Agent;
        rig.type_bytes(b"2x");
        rig.settle();
        assert_eq!(rig.outcome().received, "2x");
    }

    /// The same for Escape, held while a program loads, then another program
    /// maximized: a `[` typed next reaches the new program.
    #[test]
    fn a_key_typed_after_a_dropped_escape_reaches_the_next_program() {
        let mut rig = Rig::new(Setup {
            program: Program::Silent,
            ..MAXIMIZED
        });
        rig.type_bytes(b"\x1b");
        rig.turn();
        assert_eq!(rig.app.raw_input_parser.pending(), b"\x1b");
        let elsewhere = rig.received.with_file_name("terminal received");
        let id = maximize_a_dock_terminal(&mut rig, &elsewhere, Program::Plain);
        rig.type_bytes(b"[x");
        rig.settle();
        assert_eq!(
            everything_received(&rig.app.engine.companion_terminals[&id].client, &elsewhere),
            "[x"
        );
    }

    /// The shortest held start of the paste start marker that is taken for a
    /// paste, `ESC [ 2`, held when a worker opens a dialog: the rest of the
    /// paste never acts on the dialog.
    #[test]
    fn a_paste_whose_start_marker_a_dialog_cuts_after_its_third_byte_never_acts_on_the_dialog() {
        let mut rig = Rig::new(MAXIMIZED);
        rig.type_bytes(b"\x1b[2");
        rig.turn();
        assert_eq!(rig.app.raw_input_parser.pending(), b"\x1b[2");
        a_worker_opens_a_dialog(&mut rig);
        rig.turn();
        rig.type_bytes(b"00~de\r\x1b[201~");
        rig.settle();
        assert_eq!(prompt_name(&rig.app.prompt), "ConfigReloadFailed");
        close_the_dialog(&mut rig);
        rig.type_bytes(b"x");
        rig.settle();
        assert_eq!(rig.outcome().received, "x");
    }

    /// What takes the windowed interface's keys in a case where held input
    /// must outlast a change around it.
    #[derive(Clone, Copy, Debug)]
    enum Taker {
        Palette,
        /// A dialog with a text field: renaming the agent.
        RenameDialog,
        MacroBar,
    }

    const TAKERS: [Taker; 3] = [Taker::Palette, Taker::RenameDialog, Taker::MacroBar];

    fn default_key(action: Action) -> Vec<u8> {
        default_bindings()
            .byte_patterns_for(action)
            .into_iter()
            .next()
            .expect("the action has a default key")
    }

    /// Open `taker` from the agents list.
    fn open(rig: &mut Rig, taker: Taker) {
        rig.app.focus = FocusPane::Left;
        match taker {
            Taker::Palette => {
                rig.type_bytes(&default_key(Action::OpenPalette));
                rig.settle();
                assert_eq!(prompt_name(&rig.app.prompt), "Command");
            }
            Taker::RenameDialog => {
                rig.type_bytes(&default_key(Action::RenameSession));
                rig.settle();
                assert_eq!(prompt_name(&rig.app.prompt), "RenameSession");
            }
            Taker::MacroBar => {
                rig.app.macro_bar = Some(MacroBarState {
                    input: TextInput::new(),
                    selected: 0,
                    previous_input_target: rig.app.input_target,
                });
                rig.app.input_target = InputTarget::None;
            }
        }
    }

    /// The text typed into `taker`'s field.
    fn typed_into(app: &App, taker: Taker) -> String {
        match (taker, &app.prompt) {
            (Taker::Palette, PromptState::Command { input, .. })
            | (Taker::RenameDialog, PromptState::RenameSession { input, .. }) => input.text.clone(),
            (Taker::MacroBar, _) => app
                .macro_bar
                .as_ref()
                .expect("the macro bar is open")
                .input
                .text
                .clone(),
            (_, prompt) => panic!("{taker:?} is not open: {}", prompt_name(prompt)),
        }
    }

    /// A paste typed into `taker`, cut by a poll, `change` made while it is
    /// held and another poll made, arrives whole in `taker`.
    fn assert_a_paste_outlasts(rig: &mut Rig, taker: Taker, change: impl FnOnce(&mut Rig)) {
        rig.type_bytes(b"\x1b[200~abc");
        rig.turn();
        assert_eq!(rig.app.terminal_input.begun_sequence(), b"\x1b[200~abc");
        change(rig);
        rig.turn();
        rig.type_bytes(b"def\x1b[201~");
        rig.settle();
        assert_eq!(typed_into(&rig.app, taker), "abcdef", "{taker:?}");
    }

    /// A terminal of the agent's dock, running a program, brought in front of
    /// the agent without moving the keys.
    fn bring_a_dock_terminal_to_the_front(rig: &mut Rig) {
        let received = rig.received.with_file_name("terminal received");
        let id = "dock-terminal".to_string();
        rig.app.engine.companion_terminals.insert(
            id.clone(),
            dux_core::model::CompanionTerminal {
                owner: dux_core::model::TerminalOwner::Session("session-1".to_string()),
                label: id.clone(),
                foreground_cmd: None,
                client: ready_program(&received, Program::Plain),
                sort_order: 0,
                created_at: chrono::Utc::now(),
                list_after: None,
            },
        );
        rig.app.rebuild_left_items();
        rig.app.active_terminal_id = Some(id);
        rig.app.session_surface = crate::model::SessionSurface::Terminal;
    }

    #[test]
    fn a_paste_into_a_dialog_or_the_macro_bar_outlasts_the_selected_program_exiting() {
        for taker in TAKERS {
            let mut rig = Rig::new(WINDOWED);
            open(&mut rig, taker);
            assert_a_paste_outlasts(&mut rig, taker, the_agent_exits);
        }
    }

    #[test]
    fn a_paste_into_a_dialog_or_the_macro_bar_outlasts_the_selected_program_starting() {
        for taker in TAKERS {
            let mut rig = Rig::new(WINDOWED);
            the_agent_exits(&mut rig);
            rig.turn();
            open(&mut rig, taker);
            assert_a_paste_outlasts(&mut rig, taker, |rig| {
                let tab = TabId::new(rig.app.engine.sessions[0].slot_tab_id().to_string());
                let client = ready_program(&rig.received, Program::Plain);
                rig.app.engine.providers.insert(tab, client);
            });
        }
    }

    #[test]
    fn a_paste_into_a_dialog_or_the_macro_bar_outlasts_another_program_coming_to_the_front() {
        for taker in TAKERS {
            let mut rig = Rig::new(WINDOWED);
            open(&mut rig, taker);
            assert_a_paste_outlasts(&mut rig, taker, bring_a_dock_terminal_to_the_front);
        }
    }

    #[test]
    fn a_paste_into_a_dialog_or_the_macro_bar_outlasts_focus_moving_under_it() {
        for taker in TAKERS {
            let mut rig = Rig::new(WINDOWED);
            open(&mut rig, taker);
            assert_a_paste_outlasts(&mut rig, taker, |rig| rig.app.focus = FocusPane::Center);
        }
    }

    /// A paste the windowed interface is reading for a dialog a worker opened
    /// over the macro bar, when the dialog closes with no key, reaches
    /// neither: the macro bar gets only what follows the paste's end marker.
    #[test]
    fn a_paste_the_window_holds_when_its_dialog_closes_by_itself_never_reaches_the_macro_bar() {
        let mut rig = Rig::new(WINDOWED);
        open(&mut rig, Taker::MacroBar);
        a_worker_opens_a_dialog(&mut rig);
        rig.type_bytes(b"\x1b[200~abc");
        rig.turn();
        assert_eq!(rig.app.terminal_input.begun_sequence(), b"\x1b[200~abc");
        // A dialog closing because of something other than a key.
        rig.app.prompt = PromptState::None;
        rig.turn();
        rig.type_bytes(b"def\x1b[201~x");
        rig.settle();
        assert_eq!(typed_into(&rig.app, Taker::MacroBar), "x");
    }

    /// The palette, opened over a row a browser drives, keeps a paste while
    /// another browser takes the row over.
    #[test]
    fn a_paste_into_the_palette_outlasts_a_browser_taking_the_row_over() {
        let mut rig = Rig::new(Setup {
            driver: Driver::Browser,
            ..WINDOWED
        });
        open(&mut rig, Taker::Palette);
        assert_a_paste_outlasts(&mut rig, Taker::Palette, |rig| {
            let seat = rig.app.pty_ownership().expect("dux serves the web");
            let browser = seat.owners.next_conn_id();
            seat.owners
                .claim_for_resize(
                    "session-1-slot",
                    browser,
                    dux_core::pty_owners::ResizeIntent::TakeOver {
                        expected_owner: None,
                    },
                    Some("Firefox"),
                    |_| {},
                )
                .epoch
                .expect("the other browser took the row over");
        });
    }
}

/// Mouse reports in the same read as a click that leaves the maximized pane.
mod clicks {
    use super::*;

    fn left(kind: u8, column: u16, row: u16) -> Vec<u8> {
        format!("\x1b[<0;{};{}{}", column + 1, row + 1, char::from(kind)).into_bytes()
    }

    /// A press outside the maximized pane leaves it; its release, read with
    /// it, reaches the windowed interface, as it does when it comes a moment
    /// later.
    #[test]
    fn the_release_of_a_click_that_leaves_the_maximized_pane_reaches_the_windowed_interface() {
        let pieces: &[&[u8]] = &[&left(b'M', 0, 0), &left(b'm', 0, 0), b"z"];
        let want = assert_burst_matches_paced(MAXIMIZED, pieces);
        assert_eq!(want.fullscreen, FullscreenOverlay::None);
        assert_eq!(
            want.received, "z",
            "the windowed pane types into the program"
        );
        assert_every_cut_matches(MAXIMIZED, &pieces.concat(), &want);
    }

    /// Text read with a click that leaves the maximized pane reaches the
    /// program as typing, before the pane is left.
    #[test]
    fn text_read_with_a_click_that_leaves_the_maximized_pane_reaches_the_program_as_typing() {
        let pieces: &[&[u8]] = &[b"ab", &left(b'M', 0, 0)];
        let want = assert_burst_matches_paced(MAXIMIZED, pieces);
        assert_eq!(want.received, "ab");
        assert_eq!(want.fullscreen, FullscreenOverlay::None);

        let mut rig = Rig::new(MAXIMIZED);
        let tab = rig.app.engine.sessions[0].slot_tab_id().to_string();
        rig.type_bytes(&pieces.concat());
        rig.settle();
        assert!(
            rig.app.engine.pty_input.contains_key(&tab),
            "the text counted as typing"
        );
    }
}

/// Keys around the hand-over of the terminal to the start-web-server status
/// screen and back to a new interface when serving stops.
mod status_screen {
    use std::sync::MutexGuard;

    use crossterm::event::KeyCode;

    use super::*;
    use crate::server_screen::{ExitKind, ScreenKeys, read_screen_keys};

    /// What one surface leaves for the next is process-wide, so the cases that
    /// pass bytes through it take turns.
    static BETWEEN_SURFACES: Mutex<()> = Mutex::new(());

    pub(super) fn take_turn() -> MutexGuard<'static, ()> {
        let turn = BETWEEN_SURFACES
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        drop(crate::terminal_input::take_from_last_surface());
        turn
    }

    /// One turn of the status screen over `typed`, where `q` returns to dux.
    /// Returns the keys it acted on, the return key included.
    fn screen_turn(keys: &mut ScreenKeys, typed: &[u8]) -> (Vec<KeyCode>, Option<ExitKind>) {
        use std::io::Write as _;
        let (terminal, mut keyboard) = UnixStream::pair().expect("a socket pair");
        keyboard.write_all(typed).expect("type into the screen");
        let mut seen = Vec::new();
        let exit = read_screen_keys(keys, terminal.as_fd(), |key| {
            seen.push(key.code);
            (key.code == KeyCode::Char('q')).then_some(ExitKind::ReturnToTui)
        });
        (seen, exit)
    }

    #[test]
    fn half_a_key_held_by_the_maximized_pane_at_the_flip_completes_on_the_status_screen() {
        let _turn = take_turn();
        let mut rig = Rig::new(MAXIMIZED);
        rig.type_bytes(b"\x1b[");
        rig.turn();
        rig.app.leave_unread_input_for_next_surface();
        let mut keys = ScreenKeys::unwatched();
        assert_eq!(screen_turn(&mut keys, b"A"), (vec![KeyCode::Up], None));
    }

    #[test]
    fn half_a_key_held_by_the_windowed_interface_at_the_flip_completes_on_the_status_screen() {
        let _turn = take_turn();
        let mut rig = Rig::new(WINDOWED);
        rig.type_bytes(b"\x1b[");
        rig.turn();
        rig.app.leave_unread_input_for_next_surface();
        let mut keys = ScreenKeys::unwatched();
        assert_eq!(screen_turn(&mut keys, b"A"), (vec![KeyCode::Up], None));
    }

    /// Keys read with the status screen's return key reach the interface that
    /// comes back, and never a later status screen.
    #[test]
    fn keys_read_with_the_return_key_reach_the_interface_that_comes_back() {
        let _turn = take_turn();
        let mut keys = ScreenKeys::unwatched();
        assert_eq!(
            screen_turn(&mut keys, b"q\x1b[Ax"),
            (vec![KeyCode::Char('q')], Some(ExitKind::ReturnToTui))
        );
        drop(keys);

        let mut rig = Rig::new(WINDOWED);
        rig.app.take_unread_input_from_last_surface();
        rig.settle();
        assert!(
            crate::terminal_input::take_from_last_surface().is_empty(),
            "nothing is left for a later status screen"
        );
        let outcome = rig.outcome();
        assert_eq!(
            outcome.received, "\x1b[Ax",
            "the windowed pane types into the program"
        );
    }

    #[test]
    fn half_a_key_read_with_the_return_key_completes_in_the_interface_that_comes_back() {
        let _turn = take_turn();
        let mut keys = ScreenKeys::unwatched();
        assert_eq!(
            screen_turn(&mut keys, b"q\x1b["),
            (vec![KeyCode::Char('q')], Some(ExitKind::ReturnToTui))
        );
        drop(keys);

        let mut rig = Rig::new(WINDOWED);
        rig.app.take_unread_input_from_last_surface();
        rig.type_bytes(b"A");
        rig.settle();
        assert_eq!(rig.outcome().received, "\x1b[A");
    }

    /// When the status screen cannot take the terminal, what the interface
    /// left unread at the flip waits for the interface that comes back, as
    /// the keys crossterm had read and not handed out waited in its queue.
    #[test]
    fn keys_left_at_a_flip_with_no_status_screen_reach_the_interface_that_comes_back() {
        let _turn = take_turn();
        let mut rig = Rig::new(WINDOWED);
        rig.app.terminal_input.push_read(b"xy");
        rig.app.leave_unread_input_for_next_surface();
        drop(rig);

        let mut rig = Rig::new(WINDOWED);
        rig.app.take_unread_input_from_last_surface();
        rig.settle();
        assert_eq!(
            rig.outcome().received,
            "xy",
            "the windowed pane types into the program"
        );
    }

    const FLIP_CHILD: &str =
        "app::input_handover_tests::status_screen::child_flips_to_the_status_screen_and_back";

    /// Runs the flip as the binary does on a real terminal: the interface
    /// reads half a key, flips, the status screen takes the terminal until its
    /// return key, and the interface that comes back reads on. Serving is
    /// left out: the screen's turn is what the serve loop calls.
    #[test]
    #[ignore = "runs only as the child of a test in this module, in a pseudo-terminal"]
    fn child_flips_to_the_status_screen_and_back() {
        use crate::pty_child::{child_dir, wait_until};

        let Some(dir) = child_dir() else {
            return;
        };
        let (mut app, _serving) = app_typing_into(&dir.join("received"), WINDOWED);
        crossterm::terminal::enable_raw_mode().expect("raw mode on the child's terminal");
        std::fs::write(dir.join("ready"), b"").expect("ready marker");
        wait_until(Duration::from_secs(30), "half a key to be read", || {
            assert!(!app.poll_run_input(), "the app asked to exit");
            app.terminal_input.begun_sequence() == b"\x1b["
        });

        let exit = RunExit::FlipToServer {
            listeners: Vec::new(),
            urls: vec!["http://127.0.0.1:1".to_string()],
            startup: dux_core::serve_log::StartupNotes::default(),
        };
        let crate::TuiExit::FlipToServer { engine, urls, .. } = crate::tui_exit(app, exit) else {
            panic!("the flip did not flip");
        };
        let mut screen = crate::ServerStatusScreen::new(
            &urls,
            &engine.config.ui.theme,
            &engine.paths,
            &engine.config.keys,
            dux_core::activity::ActivityRing::new(100),
        )
        .expect("the status screen takes the terminal");
        std::fs::write(dir.join("screen"), b"").expect("screen marker");
        wait_until(Duration::from_secs(30), "the return key", || {
            matches!(screen.tick(), crate::ServerScreenTick::ReturnToTui)
        });
        drop(screen);

        let mut app = crate::resumed_app(engine).expect("the interface comes back");
        // As the interface was left: the agent selected, typing to its program.
        app.center_mode = CenterMode::Agent;
        app.session_surface = crate::model::SessionSurface::Agent;
        app.focus = FocusPane::Center;
        app.rebuild_left_items();
        app.selected_left = 1;
        crossterm::terminal::enable_raw_mode().expect("raw mode, as the run loop sets it");
        while !dir.join("done").exists() {
            assert!(!app.poll_run_input(), "the app asked to exit");
        }
        let _ = crossterm::terminal::disable_raw_mode();
        std::fs::write(dir.join("result.tmp"), b"done").expect("result file");
        std::fs::rename(dir.join("result.tmp"), dir.join("result")).expect("result file");
    }

    /// Half a key read before the flip completes on the status screen as its
    /// return key, Escape in its extended form, and what was read with it
    /// reaches the interface that comes back, through the functions the
    /// binary calls. Lost at either hand-over, the case never ends.
    #[test]
    fn keys_cross_the_real_flip_and_return_on_a_real_terminal() {
        use crate::pty_child::{PtyChild, wait_for_file, wait_until};

        let child = PtyChild::start(FLIP_CHILD, "");
        child.type_bytes(b"\x1b[");
        wait_for_file(&child.dir.path().join("screen"), Duration::from_secs(30));
        child.type_bytes(b"27uxy");
        let received = child.dir.path().join("received");
        wait_until(Duration::from_secs(30), "the program to receive xy", || {
            std::fs::read(&received).unwrap_or_default() == b"xy"
        });
        assert_eq!(child.finish(), "done");
    }

    /// A flip and a return with nothing typed leave nothing behind.
    #[test]
    fn a_flip_and_a_return_with_nothing_typed_carry_nothing() {
        let _turn = take_turn();
        let mut rig = Rig::new(WINDOWED);
        rig.app.leave_unread_input_for_next_surface();
        let mut keys = ScreenKeys::unwatched();
        assert_eq!(screen_turn(&mut keys, b""), (vec![], None));
        drop(keys);
        assert!(crate::terminal_input::take_from_last_surface().is_empty());
    }
}

/// Keys around the input debugger opening and closing in the same read.
mod input_debugger {
    use super::*;

    /// An unmodified Escape the debugger reads without waiting out the Escape
    /// ambiguity, as a terminal with extended keys sends it.
    const CLOSE: &[u8] = b"\x1b[27u";

    fn open_debugger(rig: &mut Rig) {
        rig.app.prompt = PromptState::DebugInput {
            lines: Vec::new(),
            scroll_offset: 0,
        };
    }

    fn with_debugger_open(pieces: &[&[u8]]) -> Outcome {
        let mut rig = Rig::new(WINDOWED);
        open_debugger(&mut rig);
        for piece in pieces {
            rig.type_bytes(piece);
            rig.settle();
        }
        rig.outcome()
    }

    /// Whatever follows the closing Escape in its read does what it does when
    /// it arrives after the debugger has closed.
    #[test]
    fn keys_read_with_the_closing_escape_act_as_they_do_after_it() {
        for rest in [
            &b"ab"[..],
            b"\x1b[A",
            b"\x1b[1;5R",
            b"\x1bOx",
            b"\x1b[M #(",
            b"\x1b[200~p1\np2\x1b[201~",
            "漢é".as_bytes(),
            b"\x1b[105;5u",
            b"\x1b]11;rgb:0/0/0\x07",
        ] {
            let paced = with_debugger_open(&[CLOSE, rest]);
            let burst = with_debugger_open(&[&[CLOSE, rest].concat()]);
            assert_eq!(burst, paced, "{rest:?} read with the closing Escape");
            assert_eq!(paced.prompt, "None");
        }
    }

    #[test]
    fn a_key_cut_by_the_read_after_the_closing_escape_arrives_whole() {
        let typed = [CLOSE, b"\x1b[Ax"].concat();
        let want = with_debugger_open(&[CLOSE, b"\x1b[Ax"]);
        assert_eq!(want.received, "\x1b[Ax");
        for at in CLOSE.len()..typed.len() {
            if typed[at - 1] == 0x1b {
                continue;
            }
            let mut rig = Rig::new(WINDOWED);
            open_debugger(&mut rig);
            rig.type_bytes(&typed[..at]);
            rig.turn();
            rig.type_bytes(&typed[at..]);
            rig.settle();
            assert_eq!(rig.outcome(), want, "cut at {at}");
        }
    }

    /// Bytes the windowed interface had not read when a key opened the
    /// debugger are the debugger's to show.
    #[test]
    fn keys_read_with_the_key_that_opens_the_debugger_are_shown_by_it() {
        let mut rig = Rig::new(WINDOWED);
        rig.app.terminal_input.push_read(b"x");
        open_debugger(&mut rig);
        rig.settle();
        let outcome = rig.outcome();
        assert_eq!(outcome.received, "");
        assert_eq!(outcome.debugger_lines, 2, "a key row and its bytes row");
    }

    /// Keys still waiting on the terminal when the keys the debugger took
    /// from the windowed interface close it reach the windowed interface,
    /// after those keys.
    #[test]
    fn keys_waiting_behind_held_keys_that_close_the_debugger_arrive_in_order() {
        let mut rig = Rig::new(WINDOWED);
        rig.app.terminal_input.push_read(&[CLOSE, b"a"].concat());
        open_debugger(&mut rig);
        rig.type_bytes(b"b");
        rig.settle();
        let outcome = rig.outcome();
        assert_eq!(outcome.received, "ab");
        assert_eq!(outcome.prompt, "None");
    }

    #[test]
    fn nothing_is_left_with_the_debugger_after_it_closes() {
        let mut rig = Rig::new(WINDOWED);
        open_debugger(&mut rig);
        rig.type_bytes(&[CLOSE, b"\x1b["].concat());
        rig.settle();
        assert!(!rig.app.input_debugger_open());
        assert!(!rig.app.debug_raw_input.holds_partial_sequence());
        assert_eq!(rig.app.run_input_reader(), RunInputReader::Structured);
    }
}

/// What only a real terminal shows: the run loop reading its own terminal in
/// raw mode, whatever its standard input is, and the kernel's SIGWINCH when
/// the window is resized. The child runs in a pseudo-terminal the test types
/// into and resizes.
mod real_terminal {
    use super::*;
    use crate::pty_child::{ChildStdin, PtyChild, child_dir, crossterm_kept_input, wait_until};

    const CHILD: &str = "app::input_handover_tests::real_terminal::child_runs_the_run_loop_s_input";

    /// The child's setup that has it wait on the controlling terminal with
    /// `select`, as macOS does, on every system.
    const SELECT: &str = "select";

    #[test]
    #[ignore = "runs only as the child of a test in this module, in a pseudo-terminal"]
    fn child_runs_the_run_loop_s_input() {
        let Some(dir) = child_dir() else {
            return;
        };
        if std::fs::read_to_string(dir.join("setup")).expect("setup file") == SELECT {
            crate::terminal_input::SELECT_ON_EVERY_SYSTEM.store(true, Ordering::Relaxed);
        }
        let (mut app, _serving) = app_typing_into(&dir.join("received"), WINDOWED);
        let mut handles = register_signal_handles().expect("register the signal handlers");
        app.sigwinch_flag = Arc::clone(&handles.sigwinch_flag);
        app.resize_wake = handles.resize_wake.take();
        let mut screen = ratatui::Terminal::new(ratatui::backend::TestBackend::new(100, 30))
            .expect("a test screen");
        crossterm::terminal::enable_raw_mode().expect("raw mode on the child's terminal");
        std::fs::write(dir.join("ready"), b"").expect("ready marker");
        let mut resizes = 0;
        let mut polls = 0u64;
        let started = Instant::now();
        while !dir.join("done").exists() {
            screen.draw(|frame| app.render(frame)).expect("draw");
            assert!(!app.poll_run_input(), "the app asked to exit");
            polls += 1;
            if app.sigwinch_flag.swap(false, Ordering::Relaxed) {
                resizes += 1;
            }
        }
        let polled_for = started.elapsed();
        // The resize was sent before the done marker was written, so it has
        // landed by now even when it landed after the last poll.
        if app.sigwinch_flag.swap(false, Ordering::Relaxed) {
            resizes += 1;
        }
        let kept = crossterm_kept_input();
        let _ = crossterm::terminal::disable_raw_mode();
        let report = format!(
            "crossterm kept input {kept}\nresized {}\ntarget {:?}\nfullscreen {:?}\npolls {polls} \
             in {} ms",
            resizes > 0,
            app.input_target,
            app.fullscreen_overlay,
            polled_for.as_millis()
        );
        std::fs::write(dir.join("result.tmp"), report).expect("result file");
        std::fs::rename(dir.join("result.tmp"), dir.join("result")).expect("result file");
    }

    /// Type a burst across the maximize key into a child whose standard
    /// input is `stdin`, resize it, and check where it ended and that its
    /// input poll waited. A poll that waits makes at most thirty a second.
    fn a_burst_and_a_resize_reach_the_run_loop(stdin: ChildStdin) {
        a_burst_and_a_resize_reach_the_run_loop_set_up(stdin, "");
    }

    fn a_burst_and_a_resize_reach_the_run_loop_set_up(stdin: ChildStdin, setup: &str) {
        let child = PtyChild::start_with_stdin(CHILD, setup, stdin);
        let received = child.dir.path().join("received");
        child.type_bytes(b"ab\x07cd");
        wait_until(
            Duration::from_secs(30),
            "the program to receive abcd",
            || std::fs::read(&received).unwrap_or_default() == b"abcd",
        );
        child.resize(30, 100);
        let report = child.finish();
        let (ended, polls) = report.rsplit_once('\n').expect("a polls line");
        assert_eq!(
            ended, "crossterm kept input false\nresized true\ntarget Agent\nfullscreen Agent",
            "{report}"
        );
        let counts: Vec<u128> = polls
            .split(' ')
            .filter_map(|word| word.parse().ok())
            .collect();
        let [polls, millis] = counts[..] else {
            panic!("{report}");
        };
        assert!(polls <= 30 * (millis / 1000 + 1), "{report}");
    }

    #[test]
    fn a_burst_across_the_maximize_key_and_a_resize_reach_the_run_loop_from_a_real_terminal() {
        a_burst_and_a_resize_reach_the_run_loop(ChildStdin::Terminal);
    }

    /// With standard input elsewhere, dux reads the controlling terminal, as
    /// crossterm does.
    #[test]
    fn the_run_loop_reads_the_terminal_when_standard_input_is_dev_null() {
        a_burst_and_a_resize_reach_the_run_loop(ChildStdin::DevNull);
    }

    #[test]
    fn the_run_loop_reads_the_terminal_when_standard_input_is_a_closed_pipe() {
        a_burst_and_a_resize_reach_the_run_loop(ChildStdin::ClosedPipe);
    }

    /// The wait macOS needs for the controlling terminal, on any system.
    #[test]
    fn the_run_loop_reads_the_terminal_through_select_when_standard_input_is_dev_null() {
        a_burst_and_a_resize_reach_the_run_loop_set_up(ChildStdin::DevNull, SELECT);
    }

    const FORCE_REDRAW_CHILD: &str =
        "app::input_handover_tests::real_terminal::child_force_redraws_with_keys_waiting";

    /// Runs the run loop on the screen dux draws on, and once keys are waiting
    /// in the terminal, forces a redraw before reading them.
    #[test]
    #[ignore = "runs only as the child of a test in this module, in a pseudo-terminal"]
    fn child_force_redraws_with_keys_waiting() {
        let Some(dir) = child_dir() else {
            return;
        };
        let (mut app, _serving) = app_typing_into(&dir.join("received"), WINDOWED);
        let mut screen = enter_tui_terminal().expect("the screen dux draws on");
        std::fs::write(dir.join("ready"), b"").expect("ready marker");
        let terminal = crate::terminal_input::terminal();
        wait_until(Duration::from_secs(30), "keys to be typed", || {
            input::wait_for_input_or_wake(terminal, None, Duration::ZERO)
                .expect("look at the terminal")
                .input
        });
        app.notes.force_redraw = true;
        app.refresh_run_terminal(&mut screen);
        while !dir.join("done").exists() {
            app.draw_run_frame(&mut screen);
            assert!(!app.poll_run_input(), "the app asked to exit");
        }
        let kept = crossterm_kept_input();
        ratatui::restore();
        let report = format!("crossterm kept input {kept}");
        std::fs::write(dir.join("result.tmp"), report).expect("result file");
        std::fs::rename(dir.join("result.tmp"), dir.join("result")).expect("result file");
    }

    /// Keys waiting in the terminal when a redraw is forced reach the reader
    /// after it, and a cursor-position report among them types nothing.
    #[test]
    fn keys_waiting_at_a_force_redraw_reach_the_interface() {
        let child = PtyChild::start(FORCE_REDRAW_CHILD, "");
        let received = child.dir.path().join("received");
        child.type_bytes(b"abc\x1b[1;1R");
        wait_until(
            Duration::from_secs(20),
            "the program to receive abc",
            || std::fs::read(&received).unwrap_or_default() == b"abc",
        );
        assert_eq!(child.finish(), "crossterm kept input false");
    }
}

/// A terminal's late answer to a question dux asked, such as its background
/// colour, arriving as input.
mod host_replies {
    use super::*;

    const COLOUR_REPLY: &[u8] = b"\x1b]11;rgb:0/0/0\x07";

    /// A known bug, kept on purpose for parity with what is published: the
    /// windowed interface reads a late reply as crossterm does, Alt-], then
    /// the reply's characters as typed keys, and its closing BEL as Ctrl-g,
    /// which is the maximize key. The case pins it so that fixing it is a
    /// deliberate change to this expectation.
    #[test]
    fn known_bug_kept_on_purpose_the_windowed_interface_types_a_late_reply_as_keys() {
        let want = burst(WINDOWED, COLOUR_REPLY);
        assert_eq!(want.received, "\x1b]11;rgb:0/0/0");
        assert_eq!(want.fullscreen, FullscreenOverlay::Agent);
    }

    /// The maximized pane knows a reply's shape and types none of it.
    #[test]
    fn the_maximized_pane_types_none_of_a_late_reply() {
        let want = burst(MAXIMIZED, COLOUR_REPLY);
        assert_eq!(want.received, "");
        assert_eq!(want.fullscreen, FullscreenOverlay::Agent);
    }
}

/// Input typed while dux asks the host whether it speaks Kitty keys, before
/// the first frame, arriving in the reads that carry the host's answer.
mod startup_question {
    use super::*;

    const DEVICE_ATTRIBUTES_REPLY: &[u8] = b"\x1b[?62;22c";

    /// What the question read besides the host's answer reaches the first
    /// reader as it would had it been typed a moment later, a paste, a mouse
    /// report and a key cut by the end of the question included.
    #[test]
    fn input_read_by_the_kitty_question_reaches_the_first_reader_whole() {
        let cases: [(&[u8], &[u8]); 3] = [
            (b"\x1b[200~a\nb\x1b[201~", b""),
            (b"\x1b[<0;3;2M\x1b[<0;3;2m", b"z"),
            (b"\x1b[", b"A"),
        ];
        for (with_reply, after) in cases {
            let want = burst(WINDOWED, &[b"x", with_reply, after].concat());
            let mut rig = Rig::new(WINDOWED);
            let (question, mut host) = UnixStream::pair().expect("a socket pair");
            std::io::Write::write_all(
                &mut host,
                &[b"x", DEVICE_ATTRIBUTES_REPLY, with_reply].concat(),
            )
            .expect("answer");
            let query =
                crate::host_keys::query_kitty(&question, &mut Vec::new(), Duration::from_secs(10));
            assert_eq!(query.answer, crate::host_keys::KittyAnswer::No);
            rig.app.take_kitty_query_input(&query.typed);
            rig.type_bytes(after);
            rig.settle();
            assert_eq!(rig.outcome(), want, "{with_reply:?} then {after:?}");
        }
    }
}
