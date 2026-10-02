//! Server status screen shown by the binary while the TUI↔server flip is
//! serving the web UI in this process.
//!
//! After a flip the App's terminal teardown (`ratatui::restore()`) has already
//! run, so the terminal is back in cooked mode. This screen owns the full
//! raw/alt-screen/hidden-cursor lifecycle while serving and restores all of it
//! in `Drop` (best-effort, errors ignored), so no exit path, a panic included,
//! leaves the user with a wedged terminal. [`restore_terminal`] is the same
//! restore, exposed for the one exit that runs no destructor: a second stop
//! signal mid-shutdown, which ends the process outright.
//!
//! Its log viewer shows the server's console, the very lines `dux server`
//! prints, drawn from their segments through the theme rather than reworded or
//! filtered. The viewer scrolls back through the user's scroll bindings; while
//! it is scrolled back, new lines do not move what is on screen, and the panel
//! says how many arrived below.
//!
//! The binary drives it as the `serve_with_engine` tick closure: every engine
//! loop iteration calls [`ServerStatusScreen::tick`], which polls keys without
//! blocking and redraws when the displayed uptime second changes, a line
//! arrives, or the view scrolls, so the refresh cadence is wall-clock and event
//! driven rather than tick driven.
//!
//! dux-web never sees crossterm or ratatui: the tick closure is a generic
//! `FnMut`, wired up by the binary (`crates/dux/src/main.rs`), the only crate
//! that depends on both.

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
use crate::app::components::wrap_lines::display_width;
use crate::app::components::{
    Hint, HintTone, fitted_hint_spans, render_scroll_indicator, wrap_styled_lines,
};
use crate::keybindings::{Action, BindingScope, RuntimeBindings, server_screen_reaches};
use crate::theme::Theme;
use dux_core::activity::ActivityRing;
use dux_core::config::{DuxPaths, KeysConfig};
use dux_core::serve_log::{LogLine, LogRole, LogTone};

/// What the status screen asks the binary to do after a tick. The binary maps
/// these straight onto `dux_web::ServerTick`.
pub enum ServerScreenTick {
    /// No exit key pressed: keep serving.
    Continue,
    /// `q`/`Q`/`Esc`: stop the server and flip back to the TUI.
    ReturnToTui,
    /// `Ctrl-c`: quit dux entirely.
    QuitProcess,
}

/// Semantic role for a header line, mapped to concrete [`Theme`] fields when
/// building ratatui spans. Keeping the content builder ([`header_lines`])
/// terminal-free and theme-free makes it unit-testable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    /// The "dux" wordmark, accent-styled and bold.
    Logo,
    /// Primary heading ("dux server running").
    Heading,
    /// The URL, accent/emphasis and bold.
    Url,
    /// Muted secondary text (the uptime line).
    Muted,
    /// The reachability warning, warning-styled and bold.
    Warning,
    /// Vertical spacer (empty line).
    Spacer,
}

/// A single header line: a sequence of `(text, role)` segments.
type ScreenLine = Vec<(String, Role)>;

/// Where the log view sits, in wrapped rows counted up from the newest line, so
/// a line arriving at the bottom does not move a view that is scrolled back
/// unless the view is told how many rows arrived.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct LogScroll {
    /// Rows between the bottom of the view and the newest row. 0 follows.
    offset: usize,
    /// Lines that arrived below while scrolled back, for the "new lines" note.
    unseen: usize,
}

impl LogScroll {
    fn following(&self) -> bool {
        self.offset == 0
    }

    /// Lines arrived. Following, the view moves with them; scrolled back, it
    /// holds still by moving its offset up past the rows they took, and counts
    /// them.
    fn lines_arrived(&mut self, lines: usize, rows: usize) {
        if !self.following() {
            self.offset += rows;
            self.unseen += lines;
        }
    }

    fn page_up(&mut self, viewport: usize, total: usize) {
        self.offset = (self.offset + page(viewport)).min(max_offset(viewport, total));
    }

    fn page_down(&mut self, viewport: usize) {
        self.offset = self.offset.saturating_sub(page(viewport));
        if self.following() {
            self.unseen = 0;
        }
    }

    fn top(&mut self, viewport: usize, total: usize) {
        self.offset = max_offset(viewport, total);
    }

    fn bottom(&mut self) {
        self.offset = 0;
        self.unseen = 0;
    }

    /// Keep the offset inside what there is to show (lines dropped off the top
    /// of the buffer, a resize). Returns the first row of the window.
    fn clamp(&mut self, viewport: usize, total: usize) -> usize {
        self.offset = self.offset.min(max_offset(viewport, total));
        if self.following() {
            self.unseen = 0;
        }
        total.saturating_sub(viewport).saturating_sub(self.offset)
    }
}

/// One page is the viewport less a row of overlap, so a reader keeps their place.
fn page(viewport: usize) -> usize {
    viewport.saturating_sub(1).max(1)
}

fn max_offset(viewport: usize, total: usize) -> usize {
    total.saturating_sub(viewport)
}

/// What a key does on this screen: leave it, scroll the log, or nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScreenKey {
    Exit(ExitKind),
    Scroll(Action),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExitKind {
    ReturnToTui,
    QuitProcess,
}

/// The interactive server status screen. Owns the terminal raw/alt-screen
/// lifecycle for as long as it lives and restores everything in `Drop`.
pub struct ServerStatusScreen {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    theme: Theme,
    bindings: RuntimeBindings,
    /// Every bound URL, shown in the header so the user can pick one.
    urls: Vec<String>,
    /// Operator-facing reachability note, or None when the server is
    /// loopback-only.
    safety_note: Option<String>,
    started: Instant,
    /// Uptime second most recently drawn, so [`Self::tick`] redraws only when
    /// the visible value actually changes (wall-clock, not per engine-loop tick).
    last_drawn_secs: u64,
    /// The shared line buffer fed by the web console.
    activity: ActivityRing,
    /// The lines as of the last read, and the generation they were read at, so
    /// a redraw for the uptime alone does not copy the buffer.
    lines: Vec<LogLine>,
    generation: u64,
    connections: usize,
    scroll: LogScroll,
    /// The log pane's inner width and height at the last draw, for measuring
    /// arriving lines and sizing a page.
    log_width: usize,
    log_rows: usize,
    /// Set when the view changed (a scroll, a resize) so the next tick redraws.
    dirty: bool,
    /// Shutdown-in-progress status (e.g. "Stopping 2 agents..."), shown on its
    /// own muted line under the exit hints once teardown starts.
    shutdown_message: Option<String>,
}

impl ServerStatusScreen {
    /// Enter the alternate screen + raw mode + hide the cursor, load the theme
    /// and the key bindings, and draw the first frame. The caller (binary) falls
    /// back to a plain line if this returns `Err`: the server must still run
    /// even if the status screen cannot be set up.
    pub fn new(
        urls: &[String],
        safety_note: Option<String>,
        theme_name: &str,
        paths: &DuxPaths,
        keys: &KeysConfig,
        activity: ActivityRing,
    ) -> Result<Self> {
        // The status screen has no status line and the TUI already surfaces the
        // same warning, so the fallback warning string is dropped here.
        let (theme, _warning) = crate::theme::load_or_fallback(theme_name, paths);

        // `Drop` only runs once `Self` is constructed, so a setup step that
        // fails after raw mode is enabled must undo it by hand. `enter_terminal`
        // does that cleanup on error.
        let terminal = enter_terminal()?;

        let mut screen = Self {
            terminal,
            theme,
            bindings: RuntimeBindings::from_keys_config(keys),
            urls: urls.to_vec(),
            safety_note,
            started: Instant::now(),
            last_drawn_secs: 0,
            activity,
            lines: Vec::new(),
            generation: 0,
            connections: 0,
            scroll: LogScroll::default(),
            log_width: 0,
            log_rows: 0,
            dirty: false,
            shutdown_message: None,
        };
        screen.refresh_lines();
        screen.draw(0)?;
        Ok(screen)
    }

    /// Non-blocking poll: drain pending input, act on exit and scroll keys, and
    /// redraw when something visible changed. Returns the action the binary
    /// should take. Rendering errors are swallowed: a failed redraw must not
    /// crash the server or strand the user; the next tick retries.
    pub fn tick(&mut self) -> ServerScreenTick {
        while poll_event(Duration::ZERO).unwrap_or(false) {
            match read_event() {
                Ok(Event::Key(key)) => match screen_key(key, &self.bindings) {
                    Some(ScreenKey::Exit(ExitKind::ReturnToTui)) => {
                        return ServerScreenTick::ReturnToTui;
                    }
                    Some(ScreenKey::Exit(ExitKind::QuitProcess)) => {
                        return ServerScreenTick::QuitProcess;
                    }
                    Some(ScreenKey::Scroll(action)) => self.scroll_by(action),
                    None => {}
                },
                Ok(Event::Resize(_, _)) => self.dirty = true,
                Ok(_) => {}
                // A read error shouldn't kill the server; ignore and continue.
                Err(_) => break,
            }
        }

        let secs = self.started.elapsed().as_secs();
        // Cheap pre-check first: `generation()` is a single atomic load. Only
        // copy the buffer when a line actually arrived.
        if self.activity.generation() != self.generation {
            self.refresh_lines();
            self.dirty = true;
        }
        if self.activity.connections() != self.connections {
            self.dirty = true;
        }
        if !self.dirty && secs == self.last_drawn_secs {
            return ServerScreenTick::Continue;
        }
        // Only clear the dirty state on a successful draw; a failed render is
        // retried on the next tick instead of being silently skipped.
        if self.draw(secs).is_ok() {
            self.last_drawn_secs = secs;
            self.dirty = false;
        }
        ServerScreenTick::Continue
    }

    /// Show a persistent shutdown status line under the exit hints and redraw
    /// immediately, picking up the matching log line the console just recorded.
    /// Render errors are swallowed: the process is about to exit either way.
    pub fn show_shutdown_message(&mut self, message: impl Into<String>) {
        self.shutdown_message = Some(message.into());
        self.refresh_lines();
        let secs = self.started.elapsed().as_secs();
        let _ = self.draw(secs);
    }

    /// Read the buffer again and tell the scroll position how much arrived, so
    /// a view that is scrolled back holds still.
    fn refresh_lines(&mut self) {
        let snapshot = self.activity.snapshot();
        let arrived = usize::try_from(snapshot.generation.saturating_sub(self.generation))
            .unwrap_or(usize::MAX)
            .min(snapshot.lines.len());
        if arrived > 0 && self.log_width > 0 {
            let fresh = &snapshot.lines[snapshot.lines.len() - arrived..];
            let rows = wrapped_log_rows(fresh, &self.theme, self.log_width).len();
            self.scroll.lines_arrived(arrived, rows);
        }
        self.generation = snapshot.generation;
        self.connections = snapshot.connections;
        self.lines = snapshot.lines;
    }

    fn scroll_by(&mut self, action: Action) {
        let total = wrapped_log_rows(&self.lines, &self.theme, self.log_width.max(1)).len();
        let viewport = self.log_rows.max(1);
        match action {
            Action::ScrollPageUp => self.scroll.page_up(viewport, total),
            Action::ScrollPageDown => self.scroll.page_down(viewport),
            Action::ScrollToTop => self.scroll.top(viewport, total),
            Action::ScrollToBottom => self.scroll.bottom(),
            _ => return,
        }
        self.dirty = true;
    }

    /// Draw one frame: the header (logo + status), the log panel, and the
    /// footer hints.
    fn draw(&mut self, uptime_secs: u64) -> Result<()> {
        let theme = &self.theme;
        let header = header_lines(&self.urls, self.safety_note.as_deref(), uptime_secs);
        let shutdown_message = self.shutdown_message.as_deref();
        let bindings = &self.bindings;
        let lines = &self.lines;
        let connections = self.connections;
        let scroll = &mut self.scroll;
        let mut measured = (self.log_width, self.log_rows);
        self.terminal.draw(|frame| {
            let area = frame.area();
            frame.render_widget(Clear, area);
            let bg = Block::default().style(Style::default().bg(theme.app_bg));
            frame.render_widget(bg, area);

            // Inset the whole screen so nothing is glued to the terminal edges.
            const V_MARGIN: u16 = 1;
            const H_MARGIN: u16 = 2;

            let inner_width = area.width.saturating_sub(2 * H_MARGIN).max(1);
            let footer = footer_lines(theme, bindings, shutdown_message, inner_width);
            let header_rows: u16 = header
                .iter()
                .map(|segs| wrapped_row_count(segs, inner_width))
                .sum();
            let footer_rows = footer.len() as u16 + 1;
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .vertical_margin(V_MARGIN)
                .horizontal_margin(H_MARGIN)
                .constraints([
                    Constraint::Length(header_rows),
                    Constraint::Min(3),
                    Constraint::Length(footer_rows),
                ])
                .split(area);

            // ── Header (centered, no border) ────────────────────────────────
            let header_text: Vec<Line> = header.iter().map(|s| header_line(s, theme)).collect();
            let header_para = Paragraph::new(header_text)
                .alignment(Alignment::Center)
                // chip-free: the serving screen's header is constant words and URLs.
                .wrap(Wrap { trim: false })
                .style(Style::default().bg(theme.app_bg));
            frame.render_widget(header_para, chunks[0]);

            // ── Log panel (rounded, themed) ─────────────────────────────────
            let mut block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(theme.overlay_border))
                .style(Style::default().bg(theme.app_bg))
                .padding(Padding::horizontal(1))
                .title(Line::from(Span::styled(
                    " Log ",
                    Style::default()
                        .fg(theme.title_focused)
                        .add_modifier(Modifier::BOLD),
                )))
                .title(
                    Line::from(Span::styled(
                        format!(" {connections} connected "),
                        Style::default().fg(theme.provider_label_fg),
                    ))
                    .right_aligned(),
                );
            let end_label = bindings
                .labels_reaching(Action::ScrollToBottom, server_screen_reaches)
                .into_iter()
                .next();
            if let Some(note) = scrolled_back_note(scroll, end_label.as_deref()) {
                block = block.title_bottom(
                    Line::from(Span::styled(
                        format!(" {note} "),
                        Style::default().fg(theme.title_focused),
                    ))
                    .right_aligned(),
                );
            }
            let content = block.inner(chunks[1]);
            let width = usize::from(content.width).max(1);
            let viewport = usize::from(content.height);
            let rows = wrapped_log_rows(lines, theme, width);
            let total = rows.len();
            let start = scroll.clamp(viewport, total);
            let end = (start + viewport).min(total);
            let visible: Vec<Line> = rows[start..end].to_vec();
            frame.render_widget(Paragraph::new(visible).block(block), chunks[1]);
            render_scroll_indicator(frame, chunks[1], content, start, viewport, total, theme);
            measured = (width, viewport);

            // ── Footer hints (centered) ─────────────────────────────────────
            let footer_para = Paragraph::new(footer)
                .alignment(Alignment::Center)
                .style(Style::default().bg(theme.app_bg));
            frame.render_widget(footer_para, chunks[2]);
        })?;
        (self.log_width, self.log_rows) = measured;
        Ok(())
    }
}

impl Drop for ServerStatusScreen {
    /// Restore the terminal unconditionally and best-effort. Errors are ignored
    /// because `Drop` cannot return them and a failed restore must not panic
    /// during unwinding.
    fn drop(&mut self) {
        restore_terminal();
    }
}

/// Give the terminal back to the shell: leave the alternate screen, show the
/// cursor, leave raw mode. Best-effort and idempotent, so it is safe to call
/// from the status screen's `Drop` and again from a forced exit that runs no
/// destructor.
pub fn restore_terminal() {
    let _ = execute!(stdout(), terminal::LeaveAlternateScreen, cursor::Show);
    let _ = terminal::disable_raw_mode();
    let _ = stdout().flush();
}

/// Enable raw mode, enter the alternate screen, hide the cursor, and build the
/// ratatui terminal. On any failure after raw mode is enabled, undo the partial
/// setup before returning the error.
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

/// Rendered display width of a header line in columns.
fn line_render_width(segments: &ScreenLine) -> usize {
    segments.iter().map(|(text, _)| display_width(text)).sum()
}

/// Estimate how many rows a header line occupies once wrapped to `inner_width`.
fn wrapped_row_count(segments: &ScreenLine, inner_width: u16) -> u16 {
    let width = inner_width.max(1) as usize;
    let chars = line_render_width(segments);
    if chars == 0 {
        return 1;
    }
    (chars.div_ceil(width)) as u16
}

/// Map a key to what it does here. The return and quit keys are not bindings:
/// the TUI keybinding system is not running in server mode, so `q`/`Q`/`Esc`
/// return to the TUI and `Ctrl-c` quits, answered before the bindings are
/// consulted. Everything else goes to the bindings, where the scroll actions
/// live.
fn screen_key(key: KeyEvent, bindings: &RuntimeBindings) -> Option<ScreenKey> {
    if let Some(exit) = action_for_key(key) {
        return Some(ScreenKey::Exit(exit));
    }
    if key.kind == KeyEventKind::Release {
        return None;
    }
    match bindings.lookup(&key, BindingScope::ServerScreen) {
        Some(
            action @ (Action::ScrollPageUp
            | Action::ScrollPageDown
            | Action::ScrollToTop
            | Action::ScrollToBottom),
        ) => Some(ScreenKey::Scroll(action)),
        _ => None,
    }
}

/// The screen's own exit keys. Release events are ignored so one press is one
/// action on terminals that report them (kitty protocol).
fn action_for_key(key: KeyEvent) -> Option<ExitKind> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    match key.code {
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            Some(ExitKind::QuitProcess)
        }
        KeyCode::Char('q') | KeyCode::Char('Q') => Some(ExitKind::ReturnToTui),
        KeyCode::Esc => Some(ExitKind::ReturnToTui),
        _ => None,
    }
}

/// The note on the log panel's bottom border while it is scrolled back: how
/// many lines arrived below, and which key follows the log again. `None` while
/// following, when there is nothing to say.
fn scrolled_back_note(scroll: &LogScroll, end_label: Option<&str>) -> Option<String> {
    if scroll.following() {
        return None;
    }
    let head = if scroll.unseen > 0 {
        format!(
            "{} below",
            dux_core::text::count_of(scroll.unseen, "new line")
        )
    } else {
        "Scrolled back".to_string()
    };
    Some(match end_label {
        Some(label) => format!("{head} · {label} for the latest"),
        None => head,
    })
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

/// Build the header content (logo, heading, URLs, uptime, reachability line).
fn header_lines(urls: &[String], safety_note: Option<&str>, uptime_secs: u64) -> Vec<ScreenLine> {
    let mut lines: Vec<ScreenLine> = Vec::new();

    for logo_line in ASCII_LOGO {
        lines.push(vec![(logo_line.to_string(), Role::Logo)]);
    }

    lines.push(vec![(String::new(), Role::Spacer)]);
    lines.push(vec![("dux server running".to_string(), Role::Heading)]);
    for url in urls {
        lines.push(vec![(url.to_string(), Role::Url)]);
    }
    lines.push(vec![(
        format!("up {}", format_uptime(uptime_secs)),
        Role::Muted,
    )]);

    if let Some(note) = safety_note {
        lines.push(vec![(String::new(), Role::Spacer)]);
        lines.push(vec![(note.to_string(), Role::Warning)]);
    }

    lines
}

/// The footer's rows, each fitted to `width` columns: the log's scroll keys
/// (read from the bindings, and only those that reach this screen), the two
/// exit hints, and a muted shutdown status row once teardown has started.
///
/// The exit keys are not bindings: [`action_for_key`] answers them itself, so
/// they are named as they are.
fn footer_lines(
    theme: &Theme,
    bindings: &RuntimeBindings,
    shutdown_message: Option<&str>,
    width: u16,
) -> Vec<Line<'static>> {
    let row = |hints: &[Hint]| {
        Line::from(fitted_hint_spans(theme, HintTone::Modal, hints, usize::from(width)).spans)
    };
    let first = |action| {
        bindings
            .labels_reaching(action, server_screen_reaches)
            .into_iter()
            .next()
            .unwrap_or_default()
    };
    let mut lines = vec![
        row(&[
            Hint::keys(
                [first(Action::ScrollPageUp), first(Action::ScrollPageDown)],
                "scroll the log",
            ),
            Hint::key(first(Action::ScrollToTop), "oldest"),
            Hint::key(first(Action::ScrollToBottom), "latest"),
        ]),
        row(&[Hint::fixed_keys(["q", "Esc"], "stop the server and return to dux").pinned()]),
        row(&[Hint::fixed("Ctrl-c", "quit dux entirely").pinned()]),
    ];
    if let Some(message) = shutdown_message {
        lines.push(Line::from(Span::styled(
            message.to_string(),
            Style::default().fg(theme.provider_label_fg),
        )));
    }
    lines
}

/// Map a header line's `(text, Role)` segments onto themed ratatui spans.
fn header_line<'a>(segments: &'a ScreenLine, theme: &Theme) -> Line<'a> {
    let mut spans: Vec<Span<'a>> = Vec::new();
    for (text, role) in segments {
        let style = match role {
            Role::Logo | Role::Url => Style::default()
                .fg(theme.title_focused)
                .add_modifier(Modifier::BOLD),
            Role::Heading => Style::default()
                .fg(theme.text_fg)
                .add_modifier(Modifier::BOLD),
            Role::Muted => Style::default().fg(theme.provider_label_fg),
            Role::Warning => Style::default()
                .fg(theme.warning_fg)
                .add_modifier(Modifier::BOLD),
            Role::Spacer => Style::default(),
        };
        spans.push(Span::styled(text.as_str(), style));
    }
    Line::from(spans)
}

/// The theme color a tone reads in. `status_info_fg` is deliberately not used
/// for Ok: it equals `provider_label_fg` in the default theme, which would make
/// Ok and a timestamp indistinguishable.
fn tone_color(theme: &Theme, tone: LogTone) -> ratatui::style::Color {
    match tone {
        LogTone::Info => theme.title_focused,
        LogTone::Ok => theme.diff_add,
        LogTone::Warn => theme.warning_fg,
        LogTone::Error => theme.status_error_fg,
    }
}

/// Draw one server log line from its segments, with every segment's text
/// exactly as `dux server` prints it (its rich spelling: the tone glyph, never
/// color alone) and each role styled through the theme.
fn log_line(line: &LogLine, theme: &Theme) -> Line<'static> {
    let spans: Vec<Span<'static>> = line
        .segments
        .iter()
        .map(|segment| {
            let style = match segment.role {
                LogRole::Timestamp | LogRole::Version => {
                    Style::default().fg(theme.provider_label_fg)
                }
                LogRole::Marker(tone) => Style::default().fg(tone_color(theme, tone)),
                LogRole::Message(LogTone::Warn | LogTone::Error) => {
                    let tone = match segment.role {
                        LogRole::Message(tone) => tone,
                        _ => LogTone::Info,
                    };
                    Style::default().fg(tone_color(theme, tone))
                }
                LogRole::Message(_) | LogRole::Plain => Style::default().fg(theme.text_fg),
                LogRole::Name => Style::default()
                    .fg(theme.title_focused)
                    .add_modifier(Modifier::BOLD),
                LogRole::Label | LogRole::Method => Style::default()
                    .fg(theme.text_fg)
                    .add_modifier(Modifier::BOLD),
                LogRole::Url => Style::default().fg(theme.title_focused),
                LogRole::Status(code) => Style::default().fg(match code {
                    200..=299 => theme.diff_add,
                    300..=399 => theme.title_focused,
                    400..=499 => theme.warning_fg,
                    500..=599 => theme.status_error_fg,
                    _ => theme.text_fg,
                }),
            };
            Span::styled(segment.text.clone(), style)
        })
        .collect();
    Line::from(spans)
}

/// Every log line drawn and wrapped to `width`, one entry per screen row, so
/// scrolling counts real rows and a long line is shown whole.
fn wrapped_log_rows(lines: &[LogLine], theme: &Theme, width: usize) -> Vec<Line<'static>> {
    let drawn: Vec<Line<'static>> = lines.iter().map(|line| log_line(line, theme)).collect();
    wrap_styled_lines(&drawn, width)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keybindings::BINDING_DEFS;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    fn bindings() -> RuntimeBindings {
        RuntimeBindings::new(
            |action| {
                BINDING_DEFS
                    .iter()
                    .find(|d| d.action == action)
                    .map(|d| d.default_keys.to_vec())
                    .unwrap_or_default()
            },
            true,
        )
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn q_returns_to_tui() {
        assert_eq!(
            action_for_key(key(KeyCode::Char('q'), KeyModifiers::NONE)),
            Some(ExitKind::ReturnToTui)
        );
    }

    #[test]
    fn uppercase_q_returns_to_tui() {
        assert_eq!(
            action_for_key(key(KeyCode::Char('Q'), KeyModifiers::SHIFT)),
            Some(ExitKind::ReturnToTui)
        );
    }

    #[test]
    fn esc_returns_to_tui() {
        assert_eq!(
            action_for_key(key(KeyCode::Esc, KeyModifiers::NONE)),
            Some(ExitKind::ReturnToTui)
        );
    }

    #[test]
    fn ctrl_c_quits_process() {
        assert_eq!(
            action_for_key(key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(ExitKind::QuitProcess)
        );
    }

    #[test]
    fn plain_c_is_ignored() {
        assert!(action_for_key(key(KeyCode::Char('c'), KeyModifiers::NONE)).is_none());
    }

    #[test]
    fn key_release_is_ignored() {
        let mut ev = key(KeyCode::Char('q'), KeyModifiers::NONE);
        ev.kind = KeyEventKind::Release;
        assert!(screen_key(ev, &bindings()).is_none());
    }

    #[test]
    fn the_scroll_keys_come_from_the_bindings() {
        let b = bindings();
        let press = |code| screen_key(key(code, KeyModifiers::NONE), &b);
        assert_eq!(
            press(KeyCode::PageUp),
            Some(ScreenKey::Scroll(Action::ScrollPageUp))
        );
        assert_eq!(
            press(KeyCode::PageDown),
            Some(ScreenKey::Scroll(Action::ScrollPageDown))
        );
        assert_eq!(
            press(KeyCode::Home),
            Some(ScreenKey::Scroll(Action::ScrollToTop))
        );
        assert_eq!(
            press(KeyCode::End),
            Some(ScreenKey::Scroll(Action::ScrollToBottom))
        );
        assert_eq!(press(KeyCode::Char('x')), None);
    }

    /// `q` is bound to "jump to latest" elsewhere, but here it is the way back to
    /// dux, and the way back always wins.
    #[test]
    fn q_leaves_the_screen_even_though_a_scroll_action_is_bound_to_it() {
        assert_eq!(
            screen_key(key(KeyCode::Char('q'), KeyModifiers::NONE), &bindings()),
            Some(ScreenKey::Exit(ExitKind::ReturnToTui))
        );
    }

    #[test]
    fn a_rebound_page_key_scrolls_here_too() {
        let mut keys = KeysConfig::default();
        keys.bindings
            .insert("scroll_page_up".to_string(), vec!["ctrl-u".to_string()]);
        let b = RuntimeBindings::from_keys_config(&keys);
        assert_eq!(
            screen_key(key(KeyCode::Char('u'), KeyModifiers::CONTROL), &b),
            Some(ScreenKey::Scroll(Action::ScrollPageUp))
        );
        assert_eq!(
            screen_key(key(KeyCode::PageUp, KeyModifiers::NONE), &b),
            None
        );
    }

    // ── The scroll position ────────────────────────────────────────────────

    #[test]
    fn following_moves_with_new_lines() {
        let mut scroll = LogScroll::default();
        scroll.lines_arrived(3, 4);
        assert!(scroll.following());
        assert_eq!(scroll.clamp(10, 40), 30, "the window is the newest rows");
    }

    /// Scrolled back, a line arriving must not yank the view: the same rows stay
    /// on screen, and the panel counts what arrived.
    #[test]
    fn scrolled_back_the_view_holds_still_and_counts_what_arrived() {
        let mut scroll = LogScroll::default();
        scroll.page_up(10, 40);
        let before = scroll.clamp(10, 40);
        assert_eq!(before, 21, "one page up, keeping a row of overlap");
        // Two lines arrive, taking three rows.
        scroll.lines_arrived(2, 3);
        assert_eq!(scroll.clamp(10, 43), before, "the window did not move");
        assert_eq!(scroll.unseen, 2);
        assert_eq!(
            scrolled_back_note(&scroll, Some("End")).as_deref(),
            Some("2 new lines below · End for the latest")
        );
    }

    #[test]
    fn the_latest_key_follows_again_and_clears_the_count() {
        let mut scroll = LogScroll::default();
        scroll.page_up(10, 40);
        scroll.lines_arrived(1, 1);
        scroll.bottom();
        assert!(scroll.following());
        assert_eq!(scroll.unseen, 0);
        assert_eq!(scrolled_back_note(&scroll, Some("End")), None);
    }

    #[test]
    fn paging_down_to_the_bottom_follows_again() {
        let mut scroll = LogScroll::default();
        scroll.page_up(10, 40);
        scroll.lines_arrived(1, 1);
        scroll.page_down(10);
        scroll.page_down(10);
        assert!(scroll.following());
        assert_eq!(scroll.unseen, 0);
    }

    #[test]
    fn the_oldest_key_reaches_the_first_row_and_no_further() {
        let mut scroll = LogScroll::default();
        scroll.top(10, 40);
        assert_eq!(scroll.clamp(10, 40), 0);
        scroll.page_up(10, 40);
        assert_eq!(scroll.clamp(10, 40), 0, "nothing above the oldest row");
        assert_eq!(
            scrolled_back_note(&scroll, Some("End")).as_deref(),
            Some("Scrolled back · End for the latest")
        );
    }

    #[test]
    fn a_log_shorter_than_the_view_cannot_scroll() {
        let mut scroll = LogScroll::default();
        scroll.page_up(10, 4);
        assert!(scroll.following());
        assert_eq!(scroll.clamp(10, 4), 0);
    }

    // ── Drawing a log line ─────────────────────────────────────────────────

    /// The viewer draws a line's text exactly as `dux server` prints it (its
    /// rich spelling), and its tone reads in the glyph as well as the color.
    #[test]
    fn a_log_line_is_drawn_word_for_word_with_its_glyph() {
        let theme = Theme::default_dark();
        let line = LogLine::event("12:00:00", LogTone::Warn, "the Tailscale leg stopped");
        let drawn = log_line(&line, &theme);
        assert_eq!(line_text(&drawn), line.text());
        let marker = drawn
            .spans
            .iter()
            .find(|s| s.content == LogTone::Warn.glyph())
            .expect("the warning glyph is drawn");
        assert_eq!(marker.style.fg, Some(theme.warning_fg));
        let stamp = &drawn.spans[0];
        assert_eq!(stamp.content, "12:00:00");
        assert_eq!(stamp.style.fg, Some(theme.provider_label_fg));
    }

    #[test]
    fn an_access_line_colors_its_status_by_class_through_the_theme() {
        let theme = Theme::default_dark();
        for (code, color) in [
            (200, theme.diff_add),
            (304, theme.title_focused),
            (404, theme.warning_fg),
            (502, theme.status_error_fg),
        ] {
            let line = LogLine::access("12:00:00", "GET", "/x", code, 1);
            let drawn = log_line(&line, &theme);
            assert_eq!(line_text(&drawn), line.text());
            let status = drawn
                .spans
                .iter()
                .find(|s| s.content == code.to_string())
                .unwrap();
            assert_eq!(status.style.fg, Some(color));
        }
    }

    /// A banner warning is longer than any terminal; the viewer wraps it rather
    /// than cutting it off, so every word of it is on screen.
    #[test]
    fn a_long_line_wraps_instead_of_being_cut() {
        let theme = Theme::default_dark();
        let words = "word ".repeat(30);
        let line = LogLine::event("12:00:00", LogTone::Warn, words.trim_end());
        let rows = wrapped_log_rows(std::slice::from_ref(&line), &theme, 40);
        assert!(rows.len() > 1);
        let rejoined: String = rows.iter().map(line_text).collect::<Vec<_>>().join(" ");
        assert_eq!(
            rejoined.split_whitespace().count(),
            line.text().split_whitespace().count()
        );
    }

    // ── Header and footer ──────────────────────────────────────────────────

    fn one(url: &str) -> Vec<String> {
        vec![url.to_string()]
    }

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
    fn uptime_formats_minutes_and_seconds() {
        assert_eq!(format_uptime(0), "0:00");
        assert_eq!(format_uptime(5), "0:05");
        assert_eq!(format_uptime(125), "2:05");
    }

    #[test]
    fn uptime_formats_hours() {
        assert_eq!(format_uptime(3600), "1:00:00");
        assert_eq!(format_uptime(3661), "1:01:01");
    }

    #[test]
    fn wrapped_row_count_handles_empty_short_and_long_lines() {
        assert_eq!(
            wrapped_row_count(&vec![(String::new(), Role::Spacer)], 10),
            1
        );
        assert_eq!(
            wrapped_row_count(&vec![("0123456789".to_string(), Role::Muted)], 10),
            1
        );
        assert_eq!(
            wrapped_row_count(&vec![("01234567890".to_string(), Role::Muted)], 10),
            2
        );
    }

    #[test]
    fn content_includes_url_heading_uptime_and_wordmark() {
        let lines = header_lines(&one("http://127.0.0.1:8080"), None, 42);
        let text = plain_text(&lines);
        assert!(text.contains("dux server running"));
        assert!(text.contains("http://127.0.0.1:8080"));
        assert!(text.contains("up 0:42"));
        assert_eq!(lines[0][0].1, Role::Logo);
        assert_eq!(lines[0][0].0, ASCII_LOGO[0]);
        assert!(!text.contains("return to dux"));
    }

    #[test]
    fn content_lists_all_bound_urls() {
        let urls = vec![
            "http://127.0.0.1:8080".to_string(),
            "http://100.101.102.103:8080".to_string(),
        ];
        let lines = header_lines(&urls, None, 0);
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
    fn the_safety_note_is_a_warning_row_only_when_there_is_one() {
        let none = header_lines(&one("http://127.0.0.1:8080"), None, 0);
        assert!(!none.iter().flatten().any(|(_, r)| *r == Role::Warning));
        let some = header_lines(&one("http://127.0.0.1:8080"), Some("Reachable."), 0);
        assert!(some.iter().flatten().any(|(_, r)| *r == Role::Warning));
    }

    /// The footer names the scroll keys from the bindings and the exit keys the
    /// screen answers itself, and every key it names does what it says.
    #[test]
    fn the_footer_names_keys_that_do_what_it_says() {
        let theme = Theme::default_dark();
        let b = bindings();
        let lines = footer_lines(&theme, &b, None, 100);
        let text: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(
            text,
            vec![
                "<PageUp>/<PageDown> scroll the log  <Home> oldest  <End> latest".to_string(),
                "<q>/<Esc> stop the server and return to dux".to_string(),
                "<Ctrl-c> quit dux entirely".to_string(),
            ]
        );
        for span in lines.iter().flat_map(|line| &line.spans) {
            assert_eq!(span.style.bg, None, "{span:?} names a background");
        }
        let press = |code| screen_key(key(code, KeyModifiers::NONE), &b);
        assert_eq!(
            press(KeyCode::End),
            Some(ScreenKey::Scroll(Action::ScrollToBottom))
        );
        assert_eq!(
            press(KeyCode::Esc),
            Some(ScreenKey::Exit(ExitKind::ReturnToTui))
        );

        // A narrow screen drops the scroll hints before the way out.
        let narrow = footer_lines(&theme, &b, None, 30);
        assert_eq!(line_text(&narrow[2]), "<Ctrl-c> quit dux entirely");
    }

    #[test]
    fn the_footer_appends_the_shutdown_message_as_its_own_muted_line() {
        let theme = Theme::default_dark();
        let lines = footer_lines(&theme, &bindings(), Some("Stopping 2 agents..."), 80);
        assert_eq!(lines.len(), 4, "the hints plus the shutdown line");
        let last = lines.last().expect("shutdown line present");
        assert_eq!(line_text(last), "Stopping 2 agents...");
        assert_eq!(last.spans[0].style.fg, Some(theme.provider_label_fg));
    }
}
