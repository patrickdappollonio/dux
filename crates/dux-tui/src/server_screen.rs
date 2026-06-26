//! Server status screen shown by the binary while the TUI↔server flip is
//! serving the web UI in this process.
//!
//! After a flip, the App's normal terminal teardown (`ratatui::restore()`) has
//! already run, so the terminal is back in cooked mode. This screen owns the
//! full raw/alt-screen/hidden-cursor lifecycle for the duration of serving and
//! restores ALL of it in `Drop` (best-effort, errors ignored) so no exit
//! path — including a panic — can leave the user with a wedged terminal.
//!
//! The binary drives it as the `serve_with_engine` tick closure: each engine
//! loop iteration calls [`ServerStatusScreen::tick`], which polls keys without
//! blocking and redraws when the displayed uptime second changes OR a new
//! activity event arrives (so the ~50ms engine loop does not cause per-tick
//! redraw churn — refresh cadence is wall-clock and event driven, not
//! tick-count driven, per the project tenets).
//!
//! Dependency note: dux-web never sees crossterm/ratatui — the tick closure is
//! a generic `FnMut`, and this dux-tui helper is wired into it by the binary
//! (`crates/dux/src/main.rs`), the only crate that depends on both.

use std::io::{Stdout, Write, stdout};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, poll as poll_event, read as read_event,
};
use crossterm::{cursor, execute, terminal};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph, Wrap};

use crate::app::ASCII_LOGO;
use crate::theme::Theme;
use dux_core::activity::{ActivityEvent, ActivityRing, ActivitySnapshot, ActivityTone};
use dux_core::config::DuxPaths;

/// What the status screen asks the binary to do after a tick. The binary maps
/// these straight onto `dux_web::ServerTick`.
pub enum ServerScreenTick {
    /// No exit key pressed — keep serving.
    Continue,
    /// `q`/`Q`/`Esc` — stop the server and flip back to the TUI.
    ReturnToTui,
    /// `Ctrl-C` — quit dux entirely.
    QuitProcess,
}

/// Semantic role for a rendered status line, mapped to concrete [`Theme`]
/// fields when building ratatui spans. Keeping the content builder
/// ([`header_lines`]) terminal-free and theme-free makes it unit-testable
/// without a TTY or a loaded theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    /// The "dux" wordmark — accent-styled, bold.
    Logo,
    /// Primary heading ("dux server running").
    Heading,
    /// The URL — accent/emphasis, bold.
    Url,
    /// Muted secondary text (the uptime line).
    Muted,
    /// The non-loopback security warning — warning-styled, bold.
    Warning,
    /// The quieter "authenticated mode" informational line shown when the login
    /// gate is on — muted, not alarming.
    AuthInfo,
    /// An exit hint's key, rendered as a `<…>` keycap badge matching the TUI
    /// footer (e.g. `<q>`, `<Esc>`, `<Ctrl-C>`).
    Key,
    /// An exit hint's description portion.
    HintDesc,
    /// One activity-log row's message, styled by its captured tone.
    Log(ActivityTone),
    /// Vertical spacer (empty line).
    Spacer,
}

/// A single rendered line: a sequence of `(text, role)` segments. Most lines
/// are a single segment; the exit hints pair a `HintKey` with a `HintDesc`.
type ScreenLine = Vec<(String, Role)>;

/// The interactive server status screen. Owns the terminal raw/alt-screen
/// lifecycle for as long as it lives and restores everything in `Drop`.
pub struct ServerStatusScreen {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    theme: Theme,
    /// Every bound URL (loopback plus the Tailscale address in LOCAL MODE, or all
    /// the FULL WEB MODE listeners). All are shown so the user can pick one.
    urls: Vec<String>,
    loopback: bool,
    /// Whether the web login gate is active for this serve. Controls whether the
    /// status screen shows the quiet "login required" line (auth on) or the loud
    /// no-auth warning on a non-loopback bind (auth off).
    auth_enabled: bool,
    /// Number of configured login users, shown in the auth-on informational line.
    user_count: usize,
    started: Instant,
    /// Uptime second most recently drawn, so [`tick`] redraws only when the
    /// visible value actually changes (wall-clock, not per engine-loop tick).
    last_drawn_secs: u64,
    /// The shared activity buffer fed by the web console. Snapshotted each draw.
    activity: ActivityRing,
    /// Activity generation most recently drawn, so [`tick`] redraws when a new
    /// event arrives (not only when the uptime second advances).
    last_drawn_generation: u64,
}

impl ServerStatusScreen {
    /// Enter the alternate screen + raw mode + hide the cursor, load the theme,
    /// and draw the first frame. The caller (binary) falls back to a plain
    /// println if this returns `Err` — the server must still run even if the
    /// status screen cannot be set up.
    pub fn new(
        urls: &[String],
        loopback: bool,
        auth_enabled: bool,
        user_count: usize,
        theme_name: &str,
        paths: &DuxPaths,
        activity: ActivityRing,
    ) -> Result<Self> {
        // The theme name comes from `engine.config.ui.theme`; fall back to the
        // bundled default (and log) if it cannot be loaded. We deliberately
        // drop the fallback warning string here — the status screen has no
        // status line, and the same warning already surfaces in the TUI.
        let (theme, _warning) = crate::theme::load_or_fallback(theme_name, paths);

        // Own the full lifecycle: the App already ran `ratatui::restore()`
        // before the flip returned, so the terminal is in cooked mode now.
        //
        // Drop only runs once `Self` is constructed, so any setup step that
        // fails AFTER raw mode is enabled but BEFORE construction must undo it
        // by hand — otherwise the caller's fallback println path would inherit
        // a raw-mode terminal. `enter_terminal` does that cleanup on error.
        let terminal = enter_terminal()?;

        let started = Instant::now();
        let mut screen = Self {
            terminal,
            theme,
            urls: urls.to_vec(),
            loopback,
            auth_enabled,
            user_count,
            started,
            // Force the first `tick` redraw by seeding an impossible "last
            // drawn" value; the initial frame is drawn explicitly below.
            last_drawn_secs: u64::MAX,
            activity,
            last_drawn_generation: 0,
        };
        let snapshot = screen.activity.snapshot(dux_core::activity::ACTIVITY_CAP);
        screen.last_drawn_generation = snapshot.generation;
        screen.draw(0, &snapshot)?;
        screen.last_drawn_secs = 0;
        Ok(screen)
    }

    /// Non-blocking poll: drain pending input, act on exit keys, and redraw on
    /// resize or when the displayed uptime second advances. Returns the action
    /// the binary should take. Rendering errors are swallowed — a failed redraw
    /// must not crash the server or strand the user; the next tick retries.
    pub fn tick(&mut self) -> ServerScreenTick {
        // Drain every queued event without blocking so a burst of input (or a
        // resize) is handled in one tick.
        while poll_event(Duration::ZERO).unwrap_or(false) {
            match read_event() {
                Ok(Event::Key(key)) => {
                    if let Some(action) = action_for_key(key) {
                        return action;
                    }
                }
                Ok(Event::Resize(_, _)) => {
                    // Force a redraw on the next step below regardless of the
                    // uptime second by invalidating the cached value.
                    self.last_drawn_secs = u64::MAX;
                }
                Ok(_) => {}
                // A read error shouldn't kill the server; ignore and continue.
                Err(_) => break,
            }
        }

        let secs = self.started.elapsed().as_secs();
        // Cheap pre-check first: `generation()` is a single lock-free atomic load.
        // Only take the (locking, cloning) snapshot when a redraw is actually due,
        // so an idle screen does not clone the ring on every ~50ms tick.
        let generation = self.activity.generation();
        if !needs_redraw(
            secs,
            self.last_drawn_secs,
            generation,
            self.last_drawn_generation,
        ) {
            return ServerScreenTick::Continue;
        }
        let snapshot = self.activity.snapshot(dux_core::activity::ACTIVITY_CAP);
        // Only advance the cached state on a successful draw; a failed render is
        // then retried on the next tick instead of being silently skipped.
        if self.draw(secs, &snapshot).is_ok() {
            self.last_drawn_secs = secs;
            self.last_drawn_generation = snapshot.generation;
        }
        ServerScreenTick::Continue
    }

    /// Draw one frame: the header (logo + status), the Activity panel, and the
    /// footer hints. `snapshot` is taken by the caller so `tick` can compare the
    /// generation without snapshotting twice.
    fn draw(&mut self, uptime_secs: u64, snapshot: &ActivitySnapshot) -> Result<()> {
        let theme = &self.theme;
        let header = header_lines(
            &self.urls,
            self.loopback,
            self.auth_enabled,
            self.user_count,
            uptime_secs,
        );
        let footer = footer_hint_lines();
        self.terminal.draw(|frame| {
            let area = frame.area();
            // Pre-fill the whole frame with the theme background so the alt
            // screen doesn't inherit the user's terminal default.
            frame.render_widget(Clear, area);
            let bg = Block::default().style(Style::default().bg(theme.app_bg));
            frame.render_widget(bg, area);

            // Vertical split: header (its content height), Activity (fills the
            // rest, min 3 rows for a border + one line), footer (its rows + a
            // one-row gap above for breathing room).
            let full_width = area.width.max(1);
            let header_rows: u16 = header
                .iter()
                .map(|segs| wrapped_row_count(segs, full_width))
                .sum();
            let footer_rows = footer.len() as u16 + 1;
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(header_rows),
                    Constraint::Min(3),
                    Constraint::Length(footer_rows),
                ])
                .split(area);

            // ── Header (centered, no border) ────────────────────────────────
            let header_text: Vec<Line> = header.iter().map(|s| line_for(s, theme)).collect();
            let header_para = Paragraph::new(header_text)
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: false })
                .style(Style::default().bg(theme.app_bg));
            frame.render_widget(header_para, chunks[0]);

            // ── Activity panel (rounded, themed) ────────────────────────────
            let count = snapshot.connections;
            let count_label = format!(" {count} connected ");
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme.overlay_border))
                .style(Style::default().bg(theme.app_bg))
                .padding(Padding::horizontal(1))
                .title(Line::from(Span::styled(
                    " Activity ",
                    Style::default()
                        .fg(theme.title_focused)
                        .add_modifier(Modifier::BOLD),
                )))
                .title(
                    Line::from(Span::styled(
                        count_label,
                        Style::default().fg(theme.provider_label_fg),
                    ))
                    .right_aligned(),
                );
            // Visible rows = inner height (panel height minus the two borders).
            let inner_rows = chunks[1].height.saturating_sub(2) as usize;
            let log = activity_lines(&snapshot.events, inner_rows);
            let log_text: Vec<Line> = log.iter().map(|s| line_for(s, theme)).collect();
            let log_para = Paragraph::new(log_text)
                .alignment(Alignment::Left)
                .block(block);
            frame.render_widget(log_para, chunks[1]);

            // ── Footer hints (centered) ─────────────────────────────────────
            let footer_text: Vec<Line> = footer.iter().map(|s| line_for(s, theme)).collect();
            let footer_para = Paragraph::new(footer_text)
                .alignment(Alignment::Center)
                .style(Style::default().bg(theme.app_bg));
            frame.render_widget(footer_para, chunks[2]);
        })?;
        Ok(())
    }
}

impl Drop for ServerStatusScreen {
    /// Restore the terminal unconditionally and best-effort: leave raw mode,
    /// exit the alt screen, and show the cursor again. Errors are ignored
    /// because `Drop` cannot return them and a failed restore must not panic
    /// during unwinding.
    fn drop(&mut self) {
        let _ = execute!(stdout(), terminal::LeaveAlternateScreen, cursor::Show);
        let _ = terminal::disable_raw_mode();
        let _ = stdout().flush();
    }
}

/// Enable raw mode, enter the alternate screen, hide the cursor, and build the
/// ratatui terminal. On any failure after raw mode is enabled, undo the partial
/// setup before returning the error so the caller never inherits a raw-mode
/// terminal (Drop can't help yet — `Self` isn't constructed).
fn enter_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    terminal::enable_raw_mode()?;
    if let Err(err) = execute!(stdout(), terminal::EnterAlternateScreen, cursor::Hide) {
        let _ = terminal::disable_raw_mode();
        return Err(err.into());
    }
    match Terminal::new(CrosstermBackend::new(stdout())) {
        Ok(terminal) => Ok(terminal),
        Err(err) => {
            let _ = execute!(stdout(), terminal::LeaveAlternateScreen, cursor::Show);
            let _ = terminal::disable_raw_mode();
            Err(err.into())
        }
    }
}

/// Rendered display width of a content line in columns. Key segments render as
/// `<…>` badges (their text plus the two bracket columns), and adjacent badges
/// are separated by a space, so the width is more than a raw character sum.
/// Uses character count (not bytes) so multi-byte text measures correctly.
fn line_render_width(segments: &ScreenLine) -> usize {
    let mut width = 0usize;
    let mut prev_was_key = false;
    for (text, role) in segments {
        let chars = text.chars().count();
        if *role == Role::Key {
            if prev_was_key {
                width += 1; // separating space between adjacent badges
            }
            width += chars + 2; // the surrounding `<` and `>`
            prev_was_key = true;
        } else {
            width += chars;
            prev_was_key = false;
        }
    }
    width
}

/// Estimate how many rows a content line occupies once wrapped to `inner_width`,
/// so the box height accounts for the long warning line instead of clipping it.
/// Uses rendered width (badges included) so multi-byte text and keycaps wrap
/// correctly, and counts at least one row even for the empty spacer lines.
fn wrapped_row_count(segments: &ScreenLine, inner_width: u16) -> u16 {
    let width = inner_width.max(1) as usize;
    let chars = line_render_width(segments);
    if chars == 0 {
        return 1;
    }
    (chars.div_ceil(width)) as u16
}

/// Map a key event to a screen action, or `None` to keep serving.
///
/// These keys are NOT user-configurable bindings: the TUI keybinding system
/// isn't running in server mode, so naming them literally here (and in the
/// on-screen hints) is correct rather than a tenet violation. `q`/`Q` and `Esc`
/// return to the TUI; `Ctrl-C` quits the process; everything else is ignored.
fn action_for_key(key: KeyEvent) -> Option<ServerScreenTick> {
    // Ignore key-release events so a single press maps to a single action on
    // terminals that report them (kitty protocol). Repeat events are not
    // filtered: crossterm only emits them under keyboard-enhancement flags this
    // screen never enables, and a repeated exit key would be benign anyway.
    if key.kind == KeyEventKind::Release {
        return None;
    }
    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(ServerScreenTick::QuitProcess)
        }
        KeyCode::Char('q') | KeyCode::Char('Q') => Some(ServerScreenTick::ReturnToTui),
        KeyCode::Esc => Some(ServerScreenTick::ReturnToTui),
        _ => None,
    }
}

/// Whether the status screen needs a redraw this tick: either the displayed
/// uptime second advanced (wall-clock cadence) or a new activity event arrived
/// (the generation changed). Pure so the redraw cadence is unit-testable without
/// a terminal.
fn needs_redraw(secs: u64, last_secs: u64, generation: u64, last_generation: u64) -> bool {
    secs != last_secs || generation != last_generation
}

/// Format an uptime as `M:SS` or `H:MM:SS` (e.g. `0:05`, `1:00:05`).
fn format_uptime(secs: u64) -> String {
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

/// Build the header content (logo, heading, URLs, uptime, auth/security line) as
/// a pure, terminal-free, theme-free description: each line is a list of
/// `(text, Role)` segments. The exit hints live in [`footer_hint_lines`]; the
/// activity log in [`activity_lines`].
///
/// The auth/security line is re-keyed off the login gate:
/// - auth ON  → a quiet informational line naming the configured user count;
/// - auth OFF + non-loopback → the loud no-auth security warning;
/// - auth OFF + loopback → nothing (the bind is unreachable off the machine).
fn header_lines(
    urls: &[String],
    loopback: bool,
    auth_enabled: bool,
    user_count: usize,
    uptime_secs: u64,
) -> Vec<ScreenLine> {
    let mut lines: Vec<ScreenLine> = Vec::new();

    for logo_line in ASCII_LOGO {
        lines.push(vec![(logo_line.to_string(), Role::Logo)]);
    }

    lines.push(vec![(String::new(), Role::Spacer)]);
    lines.push(vec![("dux server running".to_string(), Role::Heading)]);
    // One URL line per bound address (loopback + Tailscale in LOCAL MODE).
    for url in urls {
        lines.push(vec![(url.to_string(), Role::Url)]);
    }
    lines.push(vec![(
        format!("up {}", format_uptime(uptime_secs)),
        Role::Muted,
    )]);

    if auth_enabled {
        let noun = if user_count == 1 { "user" } else { "users" };
        lines.push(vec![(String::new(), Role::Spacer)]);
        lines.push(vec![(
            format!("Login required. {user_count} {noun} configured."),
            Role::AuthInfo,
        )]);
    } else if !loopback {
        lines.push(vec![(String::new(), Role::Spacer)]);
        lines.push(vec![(
            "Listening beyond this machine with NO authentication. \
             Anyone on the network can control your agents."
                .to_string(),
            Role::Warning,
        )]);
    }

    lines
}

/// The two exit-hint rows shown in the footer (`<q>`/`<Esc>` return, `<Ctrl-C>`
/// quit). These keys are NOT user-configurable bindings: the TUI keybinding
/// system isn't running in server mode, so naming them literally is correct.
fn footer_hint_lines() -> Vec<ScreenLine> {
    vec![
        vec![
            ("q".to_string(), Role::Key),
            ("Esc".to_string(), Role::Key),
            (
                " stop the server and return to dux".to_string(),
                Role::HintDesc,
            ),
        ],
        vec![
            ("Ctrl-C".to_string(), Role::Key),
            (" quit dux entirely".to_string(), Role::HintDesc),
        ],
    ]
}

/// Build the activity log rows: the last `max_rows` events, oldest first, each a
/// muted `HH:MM:SS` timestamp segment followed by the toned message segment.
fn activity_lines(events: &[ActivityEvent], max_rows: usize) -> Vec<ScreenLine> {
    let start = events.len().saturating_sub(max_rows);
    events[start..]
        .iter()
        .map(|e| {
            vec![
                (format!("{}  ", e.hms), Role::Muted),
                (e.message.clone(), Role::Log(e.tone)),
            ]
        })
        .collect()
}

/// Map a content line's `(text, Role)` segments onto themed ratatui spans.
/// This is the only place that touches the [`Theme`]; the content builder above
/// stays theme-free for testing.
fn line_for<'a>(segments: &'a ScreenLine, theme: &Theme) -> Line<'a> {
    let mut spans: Vec<Span<'a>> = Vec::new();
    let mut prev_was_key = false;
    for (text, role) in segments {
        // Exit-hint keys render as `<…>` keycap badges via the shared TUI helper,
        // so `<q> <Esc>` matches the footer exactly. Adjacent badges are spaced.
        if *role == Role::Key {
            if prev_was_key {
                spans.push(Span::styled(" ", Style::default().bg(theme.app_bg)));
            }
            spans.extend(theme.key_badge(text.as_str(), theme.app_bg));
            prev_was_key = true;
            continue;
        }
        prev_was_key = false;
        let style = match role {
            // Wordmark: accent (the focused-title color), bold.
            Role::Logo => Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
            // Heading: primary body text, bold.
            Role::Heading => Style::default()
                .fg(theme.text_fg)
                .add_modifier(Modifier::BOLD),
            // URL: accent emphasis, bold.
            Role::Url => Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
            // Uptime: muted secondary text.
            Role::Muted => Style::default().fg(theme.provider_label_fg),
            // Security warning: the dedicated warning color, bold.
            Role::Warning => Style::default()
                .fg(theme.warning_fg)
                .add_modifier(Modifier::BOLD),
            // Auth-on info line: muted secondary text, informational (not
            // alarming), since the login gate is protecting the bind.
            Role::AuthInfo => Style::default().fg(theme.provider_label_fg),
            // Exit-hint description: muted hint text.
            Role::HintDesc => Style::default().fg(theme.hint_desc_fg),
            // Activity-log message: colored by its captured tone, reusing
            // existing semantic theme fields so the four tones stay visually
            // distinct (Info→muted, Ok→success-green [the same token diff
            // additions use], Warn→warning, Error→error). `status_info_fg` is NOT
            // used for Ok because it equals `provider_label_fg` in the default
            // theme, which would make Ok and Info indistinguishable.
            Role::Log(tone) => {
                let fg = match tone {
                    ActivityTone::Info => theme.provider_label_fg,
                    ActivityTone::Ok => theme.diff_add,
                    ActivityTone::Warn => theme.warning_fg,
                    ActivityTone::Error => theme.status_error_fg,
                };
                Style::default().fg(fg)
            }
            // `Key` is handled above; `Spacer` is empty.
            Role::Key | Role::Spacer => Style::default(),
        };
        spans.push(Span::styled(text.as_str(), style));
    }
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use super::*;

    // No terminal-touching tests live here: raw mode / alt screen in CI is a
    // non-starter (there is no TTY). The terminal lifecycle is the thin shell
    // in `new`/`draw`/`Drop`; only the pure helpers (`action_for_key`,
    // `header_lines`, `format_uptime`) are unit-tested below.

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn q_returns_to_tui() {
        assert!(matches!(
            action_for_key(key(KeyCode::Char('q'), KeyModifiers::NONE)),
            Some(ServerScreenTick::ReturnToTui)
        ));
    }

    #[test]
    fn uppercase_q_returns_to_tui() {
        // Shift-Q arrives as Char('Q'); accept it too so a capslocked user
        // isn't stuck.
        assert!(matches!(
            action_for_key(key(KeyCode::Char('Q'), KeyModifiers::SHIFT)),
            Some(ServerScreenTick::ReturnToTui)
        ));
    }

    #[test]
    fn esc_returns_to_tui() {
        assert!(matches!(
            action_for_key(key(KeyCode::Esc, KeyModifiers::NONE)),
            Some(ServerScreenTick::ReturnToTui)
        ));
    }

    #[test]
    fn ctrl_c_quits_process() {
        assert!(matches!(
            action_for_key(key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(ServerScreenTick::QuitProcess)
        ));
    }

    #[test]
    fn plain_c_is_ignored() {
        // Only Ctrl-C quits; a bare 'c' must not.
        assert!(action_for_key(key(KeyCode::Char('c'), KeyModifiers::NONE)).is_none());
    }

    #[test]
    fn other_keys_are_ignored() {
        assert!(action_for_key(key(KeyCode::Char('x'), KeyModifiers::NONE)).is_none());
        assert!(action_for_key(key(KeyCode::Enter, KeyModifiers::NONE)).is_none());
        assert!(action_for_key(key(KeyCode::Char('a'), KeyModifiers::CONTROL)).is_none());
    }

    #[test]
    fn key_release_is_ignored() {
        // A release event for an exit key must not double-fire the action.
        let mut ev = key(KeyCode::Char('q'), KeyModifiers::NONE);
        ev.kind = KeyEventKind::Release;
        assert!(action_for_key(ev).is_none());
    }

    #[test]
    fn uptime_formats_minutes_and_seconds() {
        assert_eq!(format_uptime(0), "0:00");
        assert_eq!(format_uptime(5), "0:05");
        assert_eq!(format_uptime(42), "0:42");
        assert_eq!(format_uptime(60), "1:00");
        assert_eq!(format_uptime(125), "2:05");
    }

    #[test]
    fn uptime_formats_hours() {
        assert_eq!(format_uptime(3600), "1:00:00");
        assert_eq!(format_uptime(3605), "1:00:05");
        assert_eq!(format_uptime(3661), "1:01:01");
    }

    #[test]
    fn wrapped_row_count_handles_empty_short_and_long_lines() {
        // Empty spacer still occupies one row.
        assert_eq!(
            wrapped_row_count(&vec![(String::new(), Role::Spacer)], 10),
            1
        );
        // A line that fits is one row.
        assert_eq!(
            wrapped_row_count(&vec![("hello".to_string(), Role::Heading)], 10),
            1
        );
        // Exactly the width is still one row; one over needs two.
        assert_eq!(
            wrapped_row_count(&vec![("0123456789".to_string(), Role::Muted)], 10),
            1
        );
        assert_eq!(
            wrapped_row_count(&vec![("01234567890".to_string(), Role::Muted)], 10),
            2
        );
        // A keycap segment renders as `<key>`, so its width includes the two
        // bracket columns: `<aaaaa>` (7) + `bbbbbb` (6) = 13 → two rows at width 10.
        assert_eq!(
            wrapped_row_count(
                &vec![
                    ("aaaaa".to_string(), Role::Key),
                    ("bbbbbb".to_string(), Role::HintDesc),
                ],
                10
            ),
            2
        );
    }

    #[test]
    fn line_render_width_counts_keycap_badges_and_separators() {
        // `<q>` (3) + separating space (1) + `<Esc>` (5) + ` hi` (3) = 12.
        let line = vec![
            ("q".to_string(), Role::Key),
            ("Esc".to_string(), Role::Key),
            (" hi".to_string(), Role::HintDesc),
        ];
        assert_eq!(line_render_width(&line), 12);
    }

    /// Wrap one URL in the `&[String]` shape `header_lines` expects.
    fn one(url: &str) -> Vec<String> {
        vec![url.to_string()]
    }

    /// Collapse a `header_lines` result into the joined plain text of every
    /// segment so content assertions don't care about segment boundaries.
    fn plain_text(lines: &[ScreenLine]) -> String {
        lines
            .iter()
            .map(|segments| {
                segments
                    .iter()
                    .map(|(text, _)| text.as_str())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn content_includes_url_and_heading_and_uptime() {
        let lines = header_lines(&one("http://127.0.0.1:8080"), true, false, 0, 42);
        let text = plain_text(&lines);
        assert!(text.contains("dux server running"));
        assert!(text.contains("http://127.0.0.1:8080"));
        assert!(text.contains("up 0:42"));
    }

    #[test]
    fn content_lists_all_bound_urls() {
        // LOCAL MODE binds loopback + Tailscale; both URLs must be shown so the
        // user can copy either.
        let urls = vec![
            "http://127.0.0.1:8080".to_string(),
            "http://100.101.102.103:8080".to_string(),
        ];
        let lines = header_lines(&urls, true, false, 0, 0);
        let text = plain_text(&lines);
        assert!(text.contains("http://127.0.0.1:8080"));
        assert!(text.contains("http://100.101.102.103:8080"));
        // Each URL is its own Url-role line.
        assert_eq!(
            lines
                .iter()
                .flatten()
                .filter(|(_, role)| *role == Role::Url)
                .count(),
            2
        );
    }

    #[test]
    fn loopback_auth_off_omits_the_warning() {
        let lines = header_lines(&one("http://127.0.0.1:8080"), true, false, 0, 0);
        let text = plain_text(&lines);
        assert!(!text.contains("NO authentication"));
        assert!(!text.contains("Login required"));
        // No line should carry the Warning or AuthInfo role either.
        assert!(
            !lines
                .iter()
                .flatten()
                .any(|(_, role)| matches!(role, Role::Warning | Role::AuthInfo))
        );
    }

    #[test]
    fn non_loopback_auth_off_includes_the_loud_warning() {
        let lines = header_lines(&one("http://0.0.0.0:8080"), false, false, 0, 0);
        let text = plain_text(&lines);
        assert!(text.contains("NO authentication"));
        assert!(text.contains("control your agents"));
        assert!(
            lines
                .iter()
                .flatten()
                .any(|(_, role)| *role == Role::Warning)
        );
        // And it must NOT also show the quiet auth line.
        assert!(
            !lines
                .iter()
                .flatten()
                .any(|(_, role)| *role == Role::AuthInfo)
        );
    }

    #[test]
    fn auth_on_shows_quiet_login_line_not_the_warning() {
        // Auth on: a non-loopback bind is fine — show the quiet informational
        // line, never the loud no-auth warning.
        let lines = header_lines(&one("http://0.0.0.0:8080"), false, true, 3, 0);
        let text = plain_text(&lines);
        assert!(text.contains("Login required"));
        assert!(text.contains("3 users configured"));
        assert!(!text.contains("NO authentication"));
        assert!(
            lines
                .iter()
                .flatten()
                .any(|(_, role)| *role == Role::AuthInfo)
        );
        assert!(
            !lines
                .iter()
                .flatten()
                .any(|(_, role)| *role == Role::Warning)
        );
    }

    #[test]
    fn auth_on_singular_user_uses_singular_noun() {
        let lines = header_lines(&one("http://127.0.0.1:8080"), true, true, 1, 0);
        let text = plain_text(&lines);
        assert!(text.contains("1 user configured"));
        assert!(!text.contains("1 users"));
    }

    #[test]
    fn footer_hint_lines_carry_both_exit_keys() {
        let lines = footer_hint_lines();
        let text = plain_text(&lines);
        assert!(text.contains("return to dux"));
        assert!(text.contains("quit dux entirely"));
        // The exit keys are rendered as `<…>` keycap badges, so they live in
        // their own `Role::Key` segments rather than as inline "q or Esc" text.
        let keys: Vec<&str> = lines
            .iter()
            .flatten()
            .filter(|(_, role)| *role == Role::Key)
            .map(|(text, _)| text.as_str())
            .collect();
        assert!(keys.contains(&"q"));
        assert!(keys.contains(&"Esc"));
        assert!(keys.contains(&"Ctrl-C"));
    }

    #[test]
    fn header_lines_have_no_exit_hints() {
        // The exit hints moved to the footer — the header must not carry them.
        let lines = header_lines(&one("http://127.0.0.1:8080"), true, false, 0, 0);
        let text = plain_text(&lines);
        assert!(!text.contains("return to dux"));
        assert!(!text.contains("quit dux entirely"));
        assert!(!lines.iter().flatten().any(|(_, role)| *role == Role::Key));
    }

    fn ev(hms: &str, msg: &str, tone: ActivityTone) -> ActivityEvent {
        ActivityEvent {
            hms: hms.to_string(),
            tone,
            message: msg.to_string(),
        }
    }

    #[test]
    fn activity_lines_map_each_tone_to_a_log_role() {
        let events = vec![
            ev(
                "10:00:00",
                "client connected from 10.0.0.5",
                ActivityTone::Info,
            ),
            ev("10:00:01", "login ok for \"pat\"", ActivityTone::Ok),
            ev("10:00:02", "login failed from 10.0.0.9", ActivityTone::Warn),
            ev("10:00:03", "order failed", ActivityTone::Error),
        ];
        let lines = activity_lines(&events, 10);
        assert_eq!(lines.len(), 4);
        // Each row carries the timestamp (muted) and the toned message.
        let roles: Vec<Role> = lines
            .iter()
            .map(|segs| {
                segs.iter()
                    .find(|(_, r)| matches!(r, Role::Log(_)))
                    .unwrap()
                    .1
            })
            .collect();
        assert_eq!(
            roles,
            vec![
                Role::Log(ActivityTone::Info),
                Role::Log(ActivityTone::Ok),
                Role::Log(ActivityTone::Warn),
                Role::Log(ActivityTone::Error),
            ]
        );
        assert!(plain_text(&lines).contains("client connected from 10.0.0.5"));
        assert!(plain_text(&lines).contains("10:00:00"));
    }

    #[test]
    fn activity_lines_show_only_the_last_max_rows() {
        let events: Vec<ActivityEvent> = (0..20)
            .map(|n| ev("10:00:00", &format!("event{n}"), ActivityTone::Info))
            .collect();
        let lines = activity_lines(&events, 5);
        assert_eq!(lines.len(), 5, "only the last 5 events are rendered");
        let text = plain_text(&lines);
        assert!(text.contains("event15"));
        assert!(text.contains("event19"));
        assert!(!text.contains("event14"));
    }

    #[test]
    fn activity_lines_empty_is_empty() {
        assert!(activity_lines(&[], 10).is_empty());
    }

    #[test]
    fn needs_redraw_fires_on_new_second_or_new_event() {
        // Nothing changed → no redraw.
        assert!(!needs_redraw(5, 5, 10, 10));
        // The uptime second advanced → redraw (wall-clock cadence).
        assert!(needs_redraw(6, 5, 10, 10));
        // A new activity event arrived (generation advanced) → redraw.
        assert!(needs_redraw(5, 5, 11, 10));
        // Both changed → redraw.
        assert!(needs_redraw(6, 5, 11, 10));
    }

    #[test]
    fn content_includes_the_wordmark() {
        // The first lines are the shared ASCII wordmark; assert one of its
        // distinctive rows is present so a future logo-export break is caught.
        let lines = header_lines(&one("http://127.0.0.1:8080"), true, false, 0, 0);
        assert!(lines.len() >= ASCII_LOGO.len());
        assert_eq!(lines[0][0].1, Role::Logo);
        assert_eq!(lines[0][0].0, ASCII_LOGO[0]);
    }
}
