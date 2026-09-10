//! The run loop's redraw gate.
//!
//! The poll cadence and the draw cadence are two different questions. The loop
//! polls as often as it always did (a browser's request latency while the
//! background server is serving, the animation cadence otherwise), but a full
//! `render` rebuilds the sidebar, the PTY grid, the changes pane and the header
//! from scratch, so it runs only when something could have changed since the
//! last frame: a mutation marked dirty, an animation frame that advanced, the
//! status line, or the once-a-second backstop.

use super::*;
use crate::app::pty_ownership::PtyTakeoverCard;

/// A full redraw at least this often whatever the gate thinks, so a mutation
/// point that forgot to mark dirty costs a one-second lag, not a stuck screen.
pub(crate) const REDRAW_BACKSTOP: Duration = Duration::from_secs(1);

/// The phase of every wall-clock animation the renderer reads, each quantized
/// to ITS OWN frame interval and present only while it is actually painted. The
/// gate compares phases rather than elapsed time, so every animation redraws at
/// its own cadence: a working row at the spinner's, its slower cue riding along,
/// and a screen with no animation at nothing but the backstop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AnimationPhase {
    /// Index into [`crate::theme::SPINNER_FRAMES`].
    spinner: Option<usize>,
    /// The working cue's step, counted at
    /// [`dux_core::working_cue::ELLIPSIS_STEP_MS`]: the state word's shade and
    /// its ellipsis both change on it, and neither has a glyph of its own.
    cue: Option<usize>,
    attention_on: Option<bool>,
    /// The modal refusal cue's own phase, false whenever the cue is not running.
    refusal_on: bool,
}

/// The per-row live state the sidebar reflects. Packed into one byte so a tick
/// can tell a state change (a row that started working) from mere output on a
/// pane nobody is looking at.
const ROW_ACTIVE: u8 = 1 << 0;
const ROW_WORKING: u8 = 1 << 1;
const ROW_TYPING: u8 = 1 << 2;
const ROW_ATTENTION: u8 = 1 << 3;

#[derive(Default)]
pub(crate) struct RedrawGate {
    dirty: bool,
    last_frame: Option<Instant>,
    phase: Option<AnimationPhase>,
    status_line: Option<(dux_core::statusline::StatusTone, String)>,
    /// The card over the shown pane. Ownership moves on the socket thread while
    /// nothing on this surface happens, so there is no mutation point to mark.
    takeover_card: Option<PtyTakeoverCard>,
    rows: HashMap<String, u8>,
    /// Frames actually drawn. Tests assert cadence against it.
    pub(crate) renders: u64,
    /// Iterations whose passthrough half ran, drawn or not.
    pub(crate) forwards: u64,
}

impl App {
    /// Something the renderer reads has changed; draw the next frame.
    pub(crate) fn mark_frame_dirty(&mut self) {
        self.redraw.dirty = true;
    }

    /// The one PTY whose output is on screen, if any. Output on every other PTY
    /// changes the sidebar's row state or nothing at all.
    pub(crate) fn visible_pty_id(&self) -> Option<String> {
        match self.fullscreen_overlay {
            FullscreenOverlay::StartupLog => None,
            FullscreenOverlay::Agent | FullscreenOverlay::Terminal => {
                self.selected_terminal_surface_id()
            }
            FullscreenOverlay::None => match self.center_mode {
                CenterMode::Agent => self.selected_terminal_surface_id(),
                CenterMode::Diff { .. } => None,
            },
        }
    }

    /// Whether the pane on screen holds grid changes the next snapshot would
    /// pick up. A non-consuming read: `refresh_snapshot_buf` is the consumer,
    /// and it runs inside the draw this answer decides.
    fn visible_pty_has_pending_output(&self) -> bool {
        self.visible_pty_id().is_some()
            && self
                .selected_terminal_surface_client()
                .is_some_and(|client| client.has_output() && client.has_pending_output())
    }

    /// Poll every PTY for activity (the consuming read the row indicators need)
    /// and mark the frame dirty only for output on the pane on screen. Output
    /// anywhere else reaches the screen through a row's state, if at all.
    pub(crate) fn note_visible_pty_output(&mut self) {
        self.engine.poll_pty_activity();
        if self.visible_pty_has_pending_output() {
            self.mark_frame_dirty();
        }
    }

    /// Re-derive the sidebar's per-row live state and report whether any of it
    /// moved. This is what turns output on a hidden pane into a redraw exactly
    /// once: when the row it belongs to starts or stops working.
    pub(crate) fn refresh_row_activity(&mut self) -> bool {
        let attention_enabled = self.engine.config.ui.attention_indicator;
        let engine = &self.engine;
        let rows = &mut self.redraw.rows;
        let mut changed = false;
        let mut live = 0usize;

        let mut record = |id: &str, flags: u8| match rows.get_mut(id) {
            Some(previous) if *previous == flags => {}
            Some(previous) => {
                *previous = flags;
                changed = true;
            }
            None => {
                rows.insert(id.to_string(), flags);
                changed = true;
            }
        };

        for session in &engine.sessions {
            let active = matches!(session.status, crate::model::SessionStatus::Active);
            let mut flags = 0;
            if active {
                flags |= ROW_ACTIVE;
            }
            if engine.session_is_streaming(&session.id) {
                flags |= ROW_WORKING;
            }
            if engine.session_is_typing(&session.id) {
                flags |= ROW_TYPING;
            }
            if attention_enabled && engine.session_needs_attention(&session.id) {
                flags |= ROW_ATTENTION;
            }
            record(&session.id, flags);
            live += 1;
        }
        for id in engine.companion_terminals.keys() {
            let mut flags = ROW_ACTIVE;
            if engine.terminal_is_working(id) {
                flags |= ROW_WORKING;
            }
            if engine.is_typing(id) {
                flags |= ROW_TYPING;
            }
            record(id, flags);
            live += 1;
        }

        if rows.len() != live {
            rows.retain(|id, _| {
                engine.sessions.iter().any(|session| &session.id == id)
                    || engine.companion_terminals.contains_key(id)
            });
            changed = true;
        }
        changed
    }

    /// Whether a row is working: the one flag the spinner glyph and the pulsing
    /// state word both fire on.
    fn any_row_working(&self) -> bool {
        self.engine.sessions.iter().any(|session| {
            matches!(session.status, crate::model::SessionStatus::Active)
                && self.engine.session_is_streaming(&session.id)
        }) || self
            .engine
            .companion_terminals
            .keys()
            .any(|id| self.engine.terminal_is_working(id))
    }

    /// Whether a spinner is painted anywhere. Working rows carry one, and so do
    /// the two loading states the sidebar knows nothing about: a provider that
    /// has not produced output yet, and the project browser's own listing. A
    /// frozen spinner reads as a hung app.
    fn spinner_cue_visible(&self) -> bool {
        if self.any_row_working() {
            return true;
        }
        if matches!(
            self.prompt,
            PromptState::BrowseProjects { loading: true, .. }
        ) {
            return true;
        }
        self.visible_pty_id().is_some()
            && self
                .selected_terminal_surface_client()
                .is_some_and(|client| !client.has_output())
    }

    /// Whether an attention glyph is actually painted somewhere. Without this
    /// the blink phase would advance the gate on rows that carry no dot.
    fn attention_cue_visible(&self) -> bool {
        self.engine.config.ui.attention_indicator
            && self.engine.sessions.iter().any(|session| {
                matches!(session.status, crate::model::SessionStatus::Active)
                    && self.engine.session_needs_attention(&session.id)
            })
    }

    fn animation_phase_at(&self, now: Instant) -> Option<AnimationPhase> {
        let elapsed = now.saturating_duration_since(self.start_time).as_millis();
        let phase = AnimationPhase {
            spinner: self.spinner_cue_visible().then(|| {
                ((elapsed / crate::theme::SPINNER_FRAME_MS) as usize)
                    % crate::theme::SPINNER_FRAMES.len()
            }),
            cue: self
                .any_row_working()
                .then_some((elapsed / dux_core::working_cue::ELLIPSIS_STEP_MS as u128) as usize),
            attention_on: self
                .attention_cue_visible()
                .then(|| attention_blink_phase(elapsed)),
            refusal_on: self.refusal_blink_highlight(),
        };
        let resting = phase.spinner.is_none()
            && phase.cue.is_none()
            && phase.attention_on.is_none()
            && !phase.refusal_on;
        (!resting).then_some(phase)
    }

    /// Whether the run loop should draw this iteration. Consumes the dirty mark
    /// and re-stamps the gate, so it must be called exactly once per iteration.
    pub(crate) fn frame_needed(&mut self, now: Instant) -> bool {
        let phase = self.animation_phase_at(now);
        // Compared rather than marked: statuses are set from hundreds of call
        // sites, and this one value is the whole of what the line renders.
        let status_line = self.status.most_recent_tui();
        // Compared rather than marked: a browser claims or releases a pty on the
        // socket thread, so no path on this surface runs at all.
        let takeover_card = self.focused_pty_takeover_card();
        let needed = self.redraw.dirty
            || self.redraw.last_frame.is_none()
            || phase != self.redraw.phase
            || status_line != self.redraw.status_line
            || takeover_card != self.redraw.takeover_card
            || self
                .redraw
                .last_frame
                .is_some_and(|at| now.saturating_duration_since(at) >= REDRAW_BACKSTOP);
        if needed {
            self.redraw.dirty = false;
            self.redraw.phase = phase;
            self.redraw.status_line = status_line;
            self.redraw.takeover_card = takeover_card;
            self.redraw.last_frame = Some(now);
        }
        needed
    }

    /// The draw half of one run-loop iteration: EXACTLY ONE gate decision, and a
    /// skipped draw is not a skipped iteration, so the passthrough still runs.
    /// Returns whether the iteration may carry on to its input half.
    pub(crate) fn draw_and_forward(
        &mut self,
        now: Instant,
        draw: impl FnOnce(&mut Self) -> bool,
    ) -> bool {
        if self.frame_needed(now) && !draw(self) {
            return false;
        }
        self.redraw.forwards = self.redraw.forwards.wrapping_add(1);
        self.forward_host_passthrough();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::{default_bindings, test_app};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn terminal() -> Terminal<TestBackend> {
        Terminal::new(TestBackend::new(120, 40)).expect("test terminal")
    }

    /// One run-loop iteration's draw half, with the clock injected: the gate
    /// decides, and `render` is what counts the frame.
    fn tick(app: &mut App, terminal: &mut Terminal<TestBackend>, now: Instant) {
        if app.frame_needed(now) {
            terminal.draw(|frame| app.render(frame)).expect("draw");
        }
    }

    fn app_at_rest() -> (App, Terminal<TestBackend>, Instant) {
        let mut app = test_app(default_bindings());
        let mut terminal = terminal();
        let start = Instant::now();
        // The first frame is never gated.
        tick(&mut app, &mut terminal, start);
        assert_eq!(app.redraw.renders, 1, "the first frame always draws");
        (app, terminal, start)
    }

    #[test]
    fn an_idle_tick_draws_nothing() {
        let (mut app, mut terminal, start) = app_at_rest();
        for step in 1..=20u64 {
            tick(
                &mut app,
                &mut terminal,
                start + Duration::from_millis(step * 33),
            );
        }
        assert_eq!(
            app.redraw.renders, 1,
            "nothing changed, so nothing after the first frame is drawn"
        );
    }

    #[test]
    fn a_key_event_draws_the_next_frame() {
        let (mut app, mut terminal, start) = app_at_rest();
        tick(&mut app, &mut terminal, start + Duration::from_millis(33));
        assert_eq!(app.redraw.renders, 1);

        app.handle_terminal_event(Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE)));
        tick(&mut app, &mut terminal, start + Duration::from_millis(66));
        assert_eq!(app.redraw.renders, 2, "a keystroke draws");

        tick(&mut app, &mut terminal, start + Duration::from_millis(99));
        assert_eq!(app.redraw.renders, 2, "and the tick after it does not");
    }

    #[test]
    fn a_resize_draws_immediately() {
        let (mut app, mut terminal, start) = app_at_rest();
        app.handle_terminal_event(Event::Resize(100, 30));
        tick(&mut app, &mut terminal, start + Duration::from_millis(1));
        assert_eq!(app.redraw.renders, 2, "a resize is never deferred");
    }

    #[test]
    fn the_backstop_draws_once_a_second() {
        let (mut app, mut terminal, start) = app_at_rest();
        // A whole second of 33ms ticks, nothing changing.
        for step in 1..=30u64 {
            tick(
                &mut app,
                &mut terminal,
                start + Duration::from_millis(step * 33),
            );
        }
        assert_eq!(app.redraw.renders, 1, "990ms in, the backstop is not due");
        tick(
            &mut app,
            &mut terminal,
            start + Duration::from_millis(1_000),
        );
        assert_eq!(
            app.redraw.renders, 2,
            "a missed dirty mark costs a one-second lag, not a stuck screen"
        );
    }

    /// Draw one simulated second of 33ms polls and report the frames it cost.
    fn frames_per_simulated_second(app: &mut App, terminal: &mut Terminal<TestBackend>) -> u64 {
        let start = Instant::now();
        app.start_time = start;
        app.redraw.renders = 0;
        for step in 1..=30u64 {
            tick(app, terminal, start + Duration::from_millis(step * 33));
        }
        app.redraw.renders
    }

    #[test]
    fn a_spinning_row_draws_at_the_spinner_cadence_not_the_poll_cadence() {
        let (mut app, mut terminal, _start) = app_at_rest();
        // A spinner with no working cue beside it: the project browser's listing.
        app.prompt = PromptState::BrowseProjects {
            purpose: crate::app::BrowsePurpose::AddProject,
            current_dir: std::path::PathBuf::from("/tmp"),
            entries: Vec::new(),
            loading: true,
            selected: 0,
            filter: TextInput::new(),
            searching: false,
            editing_path: false,
            path_input: TextInput::new(),
            tab_completions: Vec::new(),
            tab_index: 0,
        };

        let drawn = frames_per_simulated_second(&mut app, &mut terminal);
        let spinner_rate = 1_000 / crate::theme::SPINNER_FRAME_MS as u64;
        assert!(
            drawn.abs_diff(spinner_rate) <= 2,
            "a second of spinner must cost about {spinner_rate} frames, not 30; drew {drawn}"
        );
    }

    /// Put the fixture's only session into the working state: Active, with
    /// streaming keyed by TAB id (the session's slot tab is the only tab it has).
    fn make_the_row_work(app: &mut App) {
        app.engine.sessions[0].status = crate::model::SessionStatus::Active;
        app.engine
            .pty_activity
            .insert("session-1-slot".to_string(), Instant::now());
        assert!(app.any_row_animating(), "the row must be animating");
    }

    #[test]
    fn a_working_row_draws_at_the_spinner_cadence() {
        let (mut app, mut terminal, _start) = app_at_rest();
        // A streaming agent carries the spinner AND the working cue, and the
        // spinner is the faster of the two, so it sets the pace.
        make_the_row_work(&mut app);

        let drawn = frames_per_simulated_second(&mut app, &mut terminal);
        let spinner_rate = 1_000 / crate::theme::SPINNER_FRAME_MS as u64;
        assert!(
            drawn.abs_diff(spinner_rate) <= 2,
            "a working row must draw at the spinner's cadence of about \
             {spinner_rate} frames a second; drew {drawn}"
        );
    }

    #[test]
    fn the_working_cue_advances_two_and_a_half_times_a_second() {
        let (mut app, _terminal, start) = app_at_rest();
        make_the_row_work(&mut app);
        // The cue is the pulsing state word and its ellipsis: no glyph of its
        // own, so its step IS its frame interval. Sampled across one simulated
        // second at the poll cadence, it must advance at 2.5 frames a second.
        let steps: Vec<Option<usize>> = (0..30u64)
            .map(|step| {
                app.animation_phase_at(start + Duration::from_millis(step * 33))
                    .expect("a working row animates")
                    .cue
            })
            .collect();
        assert!(steps.iter().all(Option::is_some), "the cue must be present");
        let advances = steps.windows(2).filter(|pair| pair[0] != pair[1]).count();
        assert_eq!(
            advances,
            2,
            "the cue must advance every {}ms, not every frame; saw {advances} \
             advances in a second",
            dux_core::working_cue::ELLIPSIS_STEP_MS
        );
    }

    #[test]
    fn a_resting_row_carries_no_working_cue() {
        let (app, _terminal, start) = app_at_rest();
        assert!(
            app.animation_phase_at(start).is_none(),
            "nothing on screen animates, so there is no phase at all"
        );
    }

    #[test]
    fn output_on_the_visible_pane_marks_the_frame_and_a_hidden_pane_does_not() {
        let (mut app, _terminal, _start) = app_at_rest();
        app.engine.config.terminal.command = "cat".to_string();
        app.engine.config.terminal.args = Vec::new();
        let (terminal_id, _) = app
            .engine
            .create_companion_terminal("session-1", 24, 80)
            .expect("companion terminal");
        app.session_surface = SessionSurface::Terminal;
        app.active_terminal_id = Some(terminal_id.clone());
        assert_eq!(
            app.visible_pty_id().as_deref(),
            Some(terminal_id.as_str()),
            "the selected terminal is the pane on screen"
        );

        // Settle whatever the spawn itself painted, so the write below is the
        // change under test.
        app.note_visible_pty_output();
        app.refresh_snapshot_buf();
        app.redraw.dirty = false;

        app.engine
            .companion_terminals
            .get(&terminal_id)
            .expect("terminal is live")
            .client
            .write_bytes(b"hello\n")
            .expect("write to terminal");

        let deadline = Instant::now() + Duration::from_secs(5);
        while !app.redraw.dirty {
            assert!(
                Instant::now() < deadline,
                "output on the visible pane never marked the frame"
            );
            std::thread::sleep(Duration::from_millis(10));
            app.note_visible_pty_output();
        }

        // The same PTY with nothing of it on screen: its output is not a redraw.
        app.fullscreen_overlay = FullscreenOverlay::StartupLog;
        assert_eq!(app.visible_pty_id(), None);
        app.redraw.dirty = false;
        app.engine
            .companion_terminals
            .get(&terminal_id)
            .expect("terminal is live")
            .client
            .write_bytes(b"more\n")
            .expect("write to terminal");
        for _ in 0..30 {
            std::thread::sleep(Duration::from_millis(10));
            app.note_visible_pty_output();
        }
        assert!(
            !app.redraw.dirty,
            "output nobody can see must not cost a frame"
        );
    }

    #[test]
    fn a_hidden_rows_working_state_costs_exactly_one_frame() {
        let (mut app, mut terminal, start) = app_at_rest();
        app.engine.sessions[0].status = crate::model::SessionStatus::Active;
        app.refresh_row_activity();
        assert!(!app.refresh_row_activity(), "the fixture starts settled");

        // Output on a pane nobody is looking at still moves the row into
        // "working", which the sidebar shows: one frame, then quiet.
        app.engine
            .pty_activity
            .insert("session-1-slot".to_string(), Instant::now());
        assert!(
            app.refresh_row_activity(),
            "the sidebar's working state changed"
        );
        app.mark_frame_dirty();
        tick(&mut app, &mut terminal, start + Duration::from_millis(33));
        assert_eq!(app.redraw.renders, 2);

        app.engine
            .pty_activity
            .insert("session-1-slot".to_string(), Instant::now());
        assert!(
            !app.refresh_row_activity(),
            "still working: nothing the sidebar shows has changed"
        );
    }

    #[test]
    fn a_status_message_draws_the_next_frame() {
        let (mut app, mut terminal, start) = app_at_rest();
        app.set_error("something the user must read");
        tick(&mut app, &mut terminal, start + Duration::from_millis(33));
        assert_eq!(app.redraw.renders, 2, "a new status takes the line");
        tick(&mut app, &mut terminal, start + Duration::from_millis(66));
        assert_eq!(app.redraw.renders, 2);
    }

    #[test]
    fn a_repaint_that_reports_no_content_change_still_draws() {
        let (mut app, _terminal, _start) = app_at_rest();
        app.engine.config.terminal.command = "cat".to_string();
        app.engine.config.terminal.args = Vec::new();
        let (terminal_id, _) = app
            .engine
            .create_companion_terminal("session-1", 24, 80)
            .expect("companion terminal");
        app.session_surface = SessionSurface::Terminal;
        app.active_terminal_id = Some(terminal_id.clone());

        app.engine
            .companion_terminals
            .get(&terminal_id)
            .expect("terminal is live")
            .client
            .write_bytes(b"hello\n")
            .expect("write to terminal");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !app
            .selected_terminal_surface_client()
            .expect("terminal is live")
            .has_output()
        {
            assert!(Instant::now() < deadline, "the child never echoed");
            std::thread::sleep(Duration::from_millis(10));
        }

        // A cursor move, a resized child's repaint burst: the grid has changes
        // to show, and the activity stamp the row indicators use reports none.
        app.engine.poll_pty_activity();
        app.engine.pty_activity.clear();
        app.redraw.dirty = false;
        app.selected_terminal_surface_client()
            .expect("terminal is live")
            .mark_dirty();
        app.note_visible_pty_output();
        assert!(
            app.redraw.dirty,
            "the pane on screen has something new to show"
        );
        assert!(
            !app.engine.pty_activity.contains_key(&terminal_id),
            "and it is not the activity stamp that said so"
        );
    }

    /// A browser claiming or releasing the shown pty happens on the socket
    /// thread: no path on this surface runs, and the card that appears is the
    /// only thing telling the user why their keystrokes stopped landing.
    #[test]
    fn a_browser_claiming_the_shown_pty_draws_the_next_frame() {
        let (mut app, mut terminal, start) = app_at_rest();
        let (companion, _recorded, seat) =
            crate::app::background_server::tests::FakeCompanion::serving_with_ownership();
        app.companion = Some(companion);
        app.engine.config.terminal.command = "cat".to_string();
        app.engine.config.terminal.args = Vec::new();
        let (terminal_id, _) = app
            .engine
            .create_companion_terminal("session-1", 24, 80)
            .expect("companion terminal");
        app.session_surface = SessionSurface::Terminal;
        app.active_terminal_id = Some(terminal_id.clone());
        app.claim_launched_pty(&terminal_id);

        // Get the pane past its "starting…" spinner, so the only thing left that
        // can draw is the card.
        app.engine
            .companion_terminals
            .get(&terminal_id)
            .expect("terminal is live")
            .client
            .write_bytes(b"hello\n")
            .expect("write to terminal");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !app
            .selected_terminal_surface_client()
            .expect("terminal is live")
            .has_output()
        {
            assert!(Instant::now() < deadline, "the child never echoed");
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            app.animation_phase_at(start),
            None,
            "nothing on this screen animates"
        );

        // Settle the gate with this surface driving and no card over the pane.
        tick(&mut app, &mut terminal, start + Duration::from_millis(33));
        let settled = app.redraw.renders;
        tick(&mut app, &mut terminal, start + Duration::from_millis(66));
        assert_eq!(app.redraw.renders, settled, "the gate is settled");

        let browser = seat.owners.next_conn_id();
        seat.owners.claim(&terminal_id, browser);
        assert!(seat.owners.is_owner(&terminal_id, browser));
        tick(&mut app, &mut terminal, start + Duration::from_millis(99));
        assert_eq!(
            app.redraw.renders,
            settled + 1,
            "the card appearing must draw on the next tick, not wait for the backstop"
        );

        // And releasing it changes the card's sentence to the unowned one.
        seat.owners.release_all(browser);
        tick(&mut app, &mut terminal, start + Duration::from_millis(132));
        assert_eq!(
            app.redraw.renders,
            settled + 2,
            "the card becoming the unowned one must draw too"
        );
    }

    /// The run loop's contract: one gate decision per iteration, and a skipped
    /// draw is not a skipped iteration.
    #[test]
    fn a_skipped_draw_still_forwards_the_hosts_passthrough() {
        let (mut app, mut terminal, start) = app_at_rest();
        let forwards = app.redraw.forwards;

        let mut drew = false;
        let carried_on = app.draw_and_forward(start + Duration::from_millis(33), |app| {
            drew = true;
            terminal.draw(|frame| app.render(frame)).expect("draw");
            true
        });
        assert!(carried_on, "the iteration carries on to its input half");
        assert!(!drew, "nothing changed, so the frame is skipped");
        assert_eq!(
            app.redraw.forwards,
            forwards + 1,
            "a skipped draw is not a skipped iteration"
        );

        app.mark_frame_dirty();
        let carried_on = app.draw_and_forward(start + Duration::from_millis(66), |app| {
            drew = true;
            terminal.draw(|frame| app.render(frame)).expect("draw");
            true
        });
        assert!(carried_on);
        assert!(drew, "a marked frame draws");
        assert_eq!(app.redraw.forwards, forwards + 2);

        // One decision per iteration: the mark is consumed by the call that
        // acted on it, so a second ask at the same instant answers no.
        app.mark_frame_dirty();
        let now = start + Duration::from_millis(99);
        assert!(app.frame_needed(now));
        assert!(
            !app.frame_needed(now),
            "the gate is consuming, so it must be asked exactly once per iteration"
        );
    }
}
