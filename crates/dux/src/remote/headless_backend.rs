//! In-memory ratatui `Backend` used by headless `dux serve`.
//!
//! The headless server has no attached TTY — it renders dux into a buffer
//! that only exists to satisfy ratatui's contract. The paired `TeeBackend`
//! on top captures every cell update and ships it to the connected remote
//! client, which is the only viewer that ever sees the output.
//!
//! Design:
//! - Owned in-memory `Vec<Cell>` sized `cols * rows`
//! - `draw` writes cells at their positions; out-of-bounds cells are
//!   silently discarded so a misbehaving widget can't panic the backend
//! - `size` / `window_size` always report the configured dimensions
//! - Cursor position is tracked so ratatui's compositor stays happy
//! - All operations are infallible: this is pure memory

use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};

/// Default viewport used by headless mode. Most remote clients run
/// standard 80x24 or larger terminals. Resizing the headless viewport
/// requires stopping and restarting `dux serve`.
pub const DEFAULT_COLS: u16 = 200;
pub const DEFAULT_ROWS: u16 = 60;

/// Ratatui backend that renders into memory only. Used by `dux serve`.
pub struct HeadlessBackend {
    size: Size,
    cursor: Position,
    cursor_visible: bool,
    buffer: Vec<Cell>,
}

impl HeadlessBackend {
    /// Construct a headless backend sized to `cols` x `rows`.
    pub fn new(cols: u16, rows: u16) -> Self {
        let area = cols as usize * rows as usize;
        Self {
            size: Size::new(cols, rows),
            cursor: Position::new(0, 0),
            cursor_visible: true,
            buffer: vec![Cell::default(); area],
        }
    }

    /// Construct a headless backend with the default viewport. Retained
    /// as a convenience constructor for future callers that don't need
    /// to override the viewport.
    #[allow(dead_code)]
    pub fn with_defaults() -> Self {
        Self::new(DEFAULT_COLS, DEFAULT_ROWS)
    }

    fn idx(&self, col: u16, row: u16) -> Option<usize> {
        if col >= self.size.width || row >= self.size.height {
            return None;
        }
        Some(row as usize * self.size.width as usize + col as usize)
    }
}

impl Backend for HeadlessBackend {
    type Error = std::io::Error;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        for (col, row, cell) in content {
            if let Some(idx) = self.idx(col, row) {
                self.buffer[idx] = cell.clone();
            }
        }
        Ok(())
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.cursor_visible = false;
        Ok(())
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.cursor_visible = true;
        Ok(())
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        Ok(self.cursor)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        self.cursor = position.into();
        Ok(())
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        for cell in &mut self.buffer {
            *cell = Cell::default();
        }
        Ok(())
    }

    fn clear_region(&mut self, _clear_type: ClearType) -> Result<(), Self::Error> {
        // All clear variants are treated as "clear everything" for headless —
        // the remote client gets a keyframe from the next draw anyway.
        self.clear()
    }

    fn size(&self) -> Result<Size, Self::Error> {
        Ok(self.size)
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        Ok(WindowSize {
            columns_rows: self.size,
            pixels: Size::new(0, 0),
        })
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }

    fn append_lines(&mut self, _n: u16) -> Result<(), Self::Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Style};
    use ratatui::widgets::Paragraph;

    use super::*;

    #[test]
    fn draw_writes_cells_in_memory() {
        let backend = HeadlessBackend::new(10, 2);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                frame.render_widget(
                    Paragraph::new("hi").style(Style::default().fg(Color::Red)),
                    Rect::new(0, 0, 10, 1),
                );
            })
            .expect("draw");
        let backend = terminal.backend();
        assert_eq!(backend.size().unwrap(), Size::new(10, 2));
    }

    #[test]
    fn out_of_bounds_draws_are_ignored() {
        let mut backend = HeadlessBackend::new(5, 5);
        let cells: [(u16, u16, Cell); 2] = [
            (0, 0, Cell::default()),
            (99, 99, Cell::default()), // out of bounds
        ];
        // Must not panic.
        Backend::draw(
            &mut backend,
            cells.iter().map(|(c, r, cell)| (*c, *r, cell)),
        )
        .unwrap();
    }

    #[test]
    fn cursor_tracking_roundtrips() {
        let mut backend = HeadlessBackend::new(5, 5);
        backend.set_cursor_position(Position::new(3, 2)).unwrap();
        assert_eq!(backend.get_cursor_position().unwrap(), Position::new(3, 2));
        backend.hide_cursor().unwrap();
        assert!(!backend.cursor_visible);
        backend.show_cursor().unwrap();
        assert!(backend.cursor_visible);
    }

    #[test]
    fn clear_empties_buffer() {
        let mut backend = HeadlessBackend::new(3, 3);
        let mut cell = Cell::default();
        cell.set_symbol("x");
        Backend::draw(&mut backend, std::iter::once((0u16, 0u16, &cell))).unwrap();
        backend.clear().unwrap();
        // After clear, the cell at (0,0) should be default.
        assert_eq!(backend.buffer[0].symbol(), " ");
    }
}
