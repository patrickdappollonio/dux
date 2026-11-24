//! The ratatui backend every screen of the terminal UI draws through.
//!
//! It is crossterm's backend, except that it never asks the terminal where
//! its cursor is: crossterm answers that by reading the terminal through its
//! own event reader, which keeps every key it reads meanwhile in a queue dux
//! never looks at.

use std::io::{self, Stdout, Write};

use ratatui::Terminal;
use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};

/// What asking for the cursor's position answers.
pub(crate) const CURSOR_QUESTION_REFUSED: &str =
    "dux never asks the terminal where its cursor is: the answer arrives as input";

/// A ratatui terminal on the real screen.
pub(crate) type ScreenTerminal = Terminal<ScreenBackend<Stdout>>;

#[derive(Debug)]
pub(crate) struct ScreenBackend<W: Write> {
    inner: CrosstermBackend<W>,
}

impl<W: Write> ScreenBackend<W> {
    pub(crate) fn new(writer: W) -> Self {
        Self {
            inner: CrosstermBackend::new(writer),
        }
    }
}

/// A terminal drawing to standard output.
pub(crate) fn screen_terminal() -> io::Result<ScreenTerminal> {
    Terminal::new(ScreenBackend::new(io::stdout()))
}

/// Clear the screen and make the next draw repaint every cell: what
/// `Terminal::clear` does on a full-screen viewport, without asking where the
/// cursor is so it can put it back. The next draw places or hides the cursor.
pub(crate) fn clear_screen<B: Backend>(terminal: &mut Terminal<B>) -> Result<(), B::Error> {
    let area = terminal.get_frame().area();
    terminal.resize(area)
}

impl<W: Write> Backend for ScreenBackend<W> {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.inner.draw(content)
    }

    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.inner.append_lines(n)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> io::Result<Position> {
        Err(io::Error::other(CURSOR_QUESTION_REFUSED))
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> io::Result<Size> {
        self.inner.size()
    }

    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.inner.window_size()
    }

    fn flush(&mut self) -> io::Result<()> {
        Backend::flush(&mut self.inner)
    }
}

#[cfg(test)]
#[path = "screen_backend_tests.rs"]
mod tests;
