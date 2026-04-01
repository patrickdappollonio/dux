use std::io::Write;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use alacritty_terminal::event::{Event, EventListener, WindowSize};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{self, Config, Term};
use alacritty_terminal::vte::ansi::{
    Color as TermColor, CursorShape, NamedColor, Processor, Rgb, StdSyncHandler,
};
use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, NativePtySystem, PtySize, PtySystem};
use ratatui::style::{Color, Modifier};

use crate::logger;

#[derive(Clone, Debug)]
pub struct SnapshotCursor {
    pub row: u16,
    pub col: u16,
}

#[derive(Clone, Debug)]
pub struct SnapshotCell {
    pub row: u16,
    pub col: u16,
    pub symbol: String,
    pub fg: Color,
    pub bg: Color,
    pub modifier: Modifier,
}

#[derive(Clone, Debug)]
pub struct TerminalSnapshot {
    pub rows: u16,
    pub cols: u16,
    pub scrollback_offset: usize,
    pub scrollback_total: usize,
    pub cursor: Option<SnapshotCursor>,
    pub cells: Vec<SnapshotCell>,
}

/// A PTY-based client that spawns a CLI tool in a pseudo-terminal and keeps a
/// full terminal grid with scrollback using `alacritty_terminal`.
pub struct PtyClient {
    #[allow(dead_code)]
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    terminal: Arc<Mutex<TerminalState>>,
    child: Box<dyn Child + Send + Sync>,
    exited: Arc<AtomicBool>,
    has_output: Arc<AtomicBool>,
}

impl PtyClient {
    /// Spawn a CLI command in a new PTY with the given size.
    pub fn spawn(
        command: &str,
        args: &[String],
        cwd: &Path,
        rows: u16,
        cols: u16,
        scrollback_lines: usize,
    ) -> Result<Self> {
        let pty_system = NativePtySystem::default();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to open PTY")?;

        let mut cmd = CommandBuilder::new(command);
        for arg in args {
            cmd.arg(arg);
        }
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");

        let child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("failed to spawn '{command}' in PTY"))?;

        // Drop slave so reads on master get EOF when child exits.
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .context("failed to clone PTY reader")?;
        let writer = pair
            .master
            .take_writer()
            .context("failed to take PTY writer")?;

        let terminal = Arc::new(Mutex::new(TerminalState::new(rows, cols, scrollback_lines)));
        let writer = Arc::new(Mutex::new(writer));
        let exited = Arc::new(AtomicBool::new(false));
        let has_output = Arc::new(AtomicBool::new(false));

        let terminal_ref = Arc::clone(&terminal);
        let writer_ref = Arc::clone(&writer);
        let exited_ref = Arc::clone(&exited);
        let has_output_ref = Arc::clone(&has_output);
        thread::spawn(move || {
            Self::reader_loop(reader, terminal_ref, writer_ref, exited_ref, has_output_ref);
        });

        Ok(Self {
            master: pair.master,
            writer,
            terminal,
            child,
            exited,
            has_output,
        })
    }

    fn reader_loop(
        mut reader: Box<dyn std::io::Read + Send>,
        terminal: Arc<Mutex<TerminalState>>,
        writer: Arc<Mutex<Box<dyn Write + Send>>>,
        exited: Arc<AtomicBool>,
        has_output: Arc<AtomicBool>,
    ) {
        let mut buf = [0u8; 4096];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => {
                    exited.store(true, Ordering::Release);
                    break;
                }
                Ok(n) => {
                    let data = &buf[..n];
                    if let Ok(mut terminal) = terminal.lock() {
                        let replies = terminal.process(data);
                        if !replies.is_empty()
                            && let Ok(mut w) = writer.lock()
                        {
                            let _ = w.write_all(&replies);
                            let _ = w.flush();
                        }
                        if !has_output.load(Ordering::Acquire) && terminal.has_visible_output() {
                            has_output.store(true, Ordering::Release);
                        }
                    }
                }
                Err(err) => {
                    logger::debug(&format!("PTY reader error: {err}"));
                    exited.store(true, Ordering::Release);
                    break;
                }
            }
        }
    }

    /// Write raw bytes to the PTY (forwards keystrokes to the child process).
    pub fn write_bytes(&self, bytes: &[u8]) -> Result<()> {
        let mut writer = self.writer.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
        writer.write_all(bytes).context("failed to write to PTY")?;
        writer.flush().context("failed to flush PTY writer")?;
        Ok(())
    }

    /// Get an owned snapshot of the currently visible terminal viewport.
    pub fn snapshot(&self) -> TerminalSnapshot {
        let terminal = self.terminal.lock().expect("terminal mutex poisoned");
        terminal.snapshot()
    }

    pub fn scrollback_offset(&self) -> usize {
        let terminal = self.terminal.lock().expect("terminal mutex poisoned");
        terminal.scrollback_offset()
    }

    /// Atomically adjust the scrollback offset by the given amount in the
    /// given direction.
    pub fn scroll(&self, up: bool, amount: usize) {
        if let Ok(mut terminal) = self.terminal.lock() {
            terminal.scroll(up, amount);
        }
    }

    /// Set the scrollback offset (0 = normal view, positive = scrolled back).
    pub fn set_scrollback(&self, rows: usize) {
        if let Ok(mut terminal) = self.terminal.lock() {
            terminal.set_scrollback(rows);
        }
    }

    /// Resize the PTY and the internal terminal parser.
    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("failed to resize PTY")?;
        if let Ok(mut terminal) = self.terminal.lock() {
            terminal.resize(rows, cols);
        }
        Ok(())
    }

    /// Check whether the child process has exited (reader thread detected EOF).
    pub fn is_exited(&self) -> bool {
        self.exited.load(Ordering::Acquire)
    }

    /// Check whether the PTY has received any output from the child process.
    pub fn has_output(&self) -> bool {
        self.has_output.load(Ordering::Acquire)
    }

    /// Non-blocking check of the child's exit status.
    pub fn try_wait(&mut self) -> Option<portable_pty::ExitStatus> {
        self.child.try_wait().ok().flatten()
    }
}

impl Drop for PtyClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

struct TerminalState {
    term: Term<EventProxy>,
    parser: Processor<StdSyncHandler>,
    event_proxy: EventProxy,
    rows: u16,
    cols: u16,
}

impl TerminalState {
    fn new(rows: u16, cols: u16, scrollback_lines: usize) -> Self {
        Self::with_scrollback(rows, cols, scrollback_lines)
    }

    fn with_scrollback(rows: u16, cols: u16, scrollback: usize) -> Self {
        let event_proxy = EventProxy::new(rows, cols);
        let dimensions = TerminalDimensions::new(rows, cols);
        let config = Config {
            scrolling_history: scrollback,
            ..Config::default()
        };
        Self {
            term: Term::new(config, &dimensions, event_proxy.clone()),
            parser: Processor::new(),
            event_proxy,
            rows,
            cols,
        }
    }

    fn process(&mut self, data: &[u8]) -> Vec<u8> {
        self.parser.advance(&mut self.term, data);
        self.event_proxy.take_pending_bytes()
    }

    fn has_visible_output(&self) -> bool {
        self.term
            .renderable_content()
            .display_iter
            .any(|indexed| !indexed.cell.c.is_whitespace())
    }

    fn snapshot(&self) -> TerminalSnapshot {
        let renderable = self.term.renderable_content();
        let display_offset = renderable.display_offset;
        let history_size = self.term.grid().history_size();
        let colors = renderable.colors;
        let cursor = if renderable.cursor.shape == CursorShape::Hidden {
            None
        } else {
            term::point_to_viewport(display_offset, renderable.cursor.point).map(|point| {
                SnapshotCursor {
                    row: point.line as u16,
                    col: point.column.0 as u16,
                }
            })
        };

        let mut cells = Vec::with_capacity(usize::from(self.rows) * usize::from(self.cols));
        for indexed in renderable.display_iter {
            let cell = indexed.cell;
            if cell
                .flags
                .intersects(Flags::WIDE_CHAR_SPACER | Flags::LEADING_WIDE_CHAR_SPACER)
            {
                continue;
            }

            let Some(point) = term::point_to_viewport(display_offset, indexed.point) else {
                continue;
            };

            let mut symbol = String::new();
            symbol.push(cell.c);
            if let Some(zerowidth) = cell.zerowidth() {
                for ch in zerowidth {
                    symbol.push(*ch);
                }
            }

            let mut modifier = Modifier::empty();
            if cell.flags.contains(Flags::BOLD) {
                modifier |= Modifier::BOLD;
            }
            if cell.flags.contains(Flags::ITALIC) {
                modifier |= Modifier::ITALIC;
            }
            if cell.flags.intersects(Flags::ALL_UNDERLINES) {
                modifier |= Modifier::UNDERLINED;
            }
            if cell.flags.contains(Flags::INVERSE) {
                modifier |= Modifier::REVERSED;
            }
            if cell.flags.contains(Flags::DIM) {
                modifier |= Modifier::DIM;
            }
            if cell.flags.contains(Flags::STRIKEOUT) {
                modifier |= Modifier::CROSSED_OUT;
            }

            cells.push(SnapshotCell {
                row: point.line as u16,
                col: point.column.0 as u16,
                symbol,
                fg: convert_terminal_color(cell.fg, colors),
                bg: convert_terminal_color(cell.bg, colors),
                modifier,
            });
        }

        TerminalSnapshot {
            rows: self.rows,
            cols: self.cols,
            scrollback_offset: display_offset,
            scrollback_total: history_size,
            cursor,
            cells,
        }
    }

    fn scrollback_offset(&self) -> usize {
        self.term.grid().display_offset()
    }

    fn scroll(&mut self, up: bool, amount: usize) {
        let delta = if up { amount as i32 } else { -(amount as i32) };
        self.term.scroll_display(Scroll::Delta(delta));
    }

    fn set_scrollback(&mut self, rows: usize) {
        let current = self.term.grid().display_offset();
        let target = rows.min(self.term.grid().history_size());
        let delta = target as i32 - current as i32;
        self.term.scroll_display(Scroll::Delta(delta));
    }

    fn resize(&mut self, rows: u16, cols: u16) {
        self.rows = rows;
        self.cols = cols;
        self.event_proxy.set_size(rows, cols);
        self.term.resize(TerminalDimensions::new(rows, cols));
    }
}

#[derive(Clone)]
struct EventProxy {
    pending: Arc<Mutex<Vec<u8>>>,
    size: Arc<Mutex<(u16, u16)>>,
}

impl EventProxy {
    fn new(rows: u16, cols: u16) -> Self {
        Self {
            pending: Arc::new(Mutex::new(Vec::new())),
            size: Arc::new(Mutex::new((rows, cols))),
        }
    }

    fn push_bytes(&self, bytes: &[u8]) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.extend_from_slice(bytes);
        }
    }

    fn take_pending_bytes(&self) -> Vec<u8> {
        self.pending
            .lock()
            .map(|mut pending| std::mem::take(&mut *pending))
            .unwrap_or_default()
    }

    fn set_size(&self, rows: u16, cols: u16) {
        if let Ok(mut size) = self.size.lock() {
            *size = (rows, cols);
        }
    }
}

impl EventListener for EventProxy {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => self.push_bytes(text.as_bytes()),
            Event::TextAreaSizeRequest(formatter) => {
                let (rows, cols) = self.size.lock().map(|size| *size).unwrap_or((24, 80));
                let response = formatter(WindowSize {
                    num_lines: rows,
                    num_cols: cols,
                    cell_width: 0,
                    cell_height: 0,
                });
                self.push_bytes(response.as_bytes());
            }
            _ => {}
        }
    }
}

struct TerminalDimensions {
    rows: usize,
    cols: usize,
}

impl TerminalDimensions {
    fn new(rows: u16, cols: u16) -> Self {
        Self {
            rows: usize::from(rows),
            cols: usize::from(cols),
        }
    }
}

impl Dimensions for TerminalDimensions {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.cols
    }
}

fn convert_terminal_color(
    color: TermColor,
    palette: &alacritty_terminal::term::color::Colors,
) -> Color {
    match color {
        TermColor::Spec(Rgb { r, g, b }) => Color::Rgb(r, g, b),
        TermColor::Indexed(index) => palette[index as usize]
            .map(|rgb| Color::Rgb(rgb.r, rgb.g, rgb.b))
            .unwrap_or(Color::Indexed(index)),
        TermColor::Named(named) => palette[named]
            .map(|rgb| Color::Rgb(rgb.r, rgb.g, rgb.b))
            .unwrap_or_else(|| named_color_to_tui(named)),
    }
}

fn named_color_to_tui(color: NamedColor) -> Color {
    match color {
        NamedColor::Black => Color::Indexed(0),
        NamedColor::Red => Color::Indexed(1),
        NamedColor::Green => Color::Indexed(2),
        NamedColor::Yellow => Color::Indexed(3),
        NamedColor::Blue => Color::Indexed(4),
        NamedColor::Magenta => Color::Indexed(5),
        NamedColor::Cyan => Color::Indexed(6),
        NamedColor::White => Color::Indexed(7),
        NamedColor::BrightBlack => Color::Indexed(8),
        NamedColor::BrightRed => Color::Indexed(9),
        NamedColor::BrightGreen => Color::Indexed(10),
        NamedColor::BrightYellow => Color::Indexed(11),
        NamedColor::BrightBlue => Color::Indexed(12),
        NamedColor::BrightMagenta => Color::Indexed(13),
        NamedColor::BrightCyan => Color::Indexed(14),
        NamedColor::BrightWhite => Color::Indexed(15),
        NamedColor::DimBlack => Color::Indexed(0),
        NamedColor::DimRed => Color::Indexed(1),
        NamedColor::DimGreen => Color::Indexed(2),
        NamedColor::DimYellow => Color::Indexed(3),
        NamedColor::DimBlue => Color::Indexed(4),
        NamedColor::DimMagenta => Color::Indexed(5),
        NamedColor::DimCyan => Color::Indexed(6),
        NamedColor::DimWhite => Color::Indexed(7),
        NamedColor::Foreground
        | NamedColor::Background
        | NamedColor::Cursor
        | NamedColor::BrightForeground
        | NamedColor::DimForeground => Color::Reset,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewport_lines(snapshot: &TerminalSnapshot) -> Vec<String> {
        let mut rows = vec![String::new(); usize::from(snapshot.rows)];
        for cell in &snapshot.cells {
            if let Some(line) = rows.get_mut(usize::from(cell.row)) {
                while line.len() < usize::from(cell.col) {
                    line.push(' ');
                }
                line.push_str(&cell.symbol);
            }
        }
        rows
    }

    #[test]
    fn scrollback_reaches_beyond_visible_rows() {
        let mut terminal = TerminalState::with_scrollback(3, 12, 100);
        terminal.process(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\n");

        assert_eq!(terminal.term.grid().history_size(), 3);

        terminal.set_scrollback(terminal.term.grid().history_size());
        let top = terminal.snapshot();
        let lines = viewport_lines(&top);

        assert_eq!(top.scrollback_total, 3);
        assert!(lines.iter().any(|line| line.contains("one")));
        assert!(lines.iter().any(|line| line.contains("two")));
    }

    #[test]
    fn scrolling_while_output_arrives_keeps_valid_offset() {
        let mut terminal = TerminalState::with_scrollback(3, 16, 100);
        terminal.process(b"one\r\ntwo\r\nthree\r\nfour\r\nfive\r\n");
        terminal.set_scrollback(terminal.term.grid().history_size());

        terminal.process(b"six\r\nseven\r\n");
        let snapshot = terminal.snapshot();

        assert!(snapshot.scrollback_offset <= terminal.term.grid().history_size());
        assert_eq!(
            snapshot.scrollback_total,
            terminal.term.grid().history_size()
        );
        assert!(terminal.term.grid().history_size() >= 5);
    }

    #[test]
    fn scrollback_offset_accessor_matches_grid_state() {
        let mut terminal = TerminalState::with_scrollback(3, 16, 100);
        terminal.process(b"one\r\ntwo\r\nthree\r\nfour\r\n");

        assert_eq!(terminal.scrollback_offset(), 0);

        terminal.set_scrollback(2);

        assert_eq!(terminal.scrollback_offset(), 2);
    }
}
