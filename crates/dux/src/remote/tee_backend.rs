//! Ratatui `Backend` wrapper that tees every render call to both the real
//! terminal backend and a capture channel.
//!
//! The `TeeBackend` is the host's primary capture surface for the remote
//! share feature. Wrapping it around the normal `CrosstermBackend<Stdout>`
//! is zero-cost when no one is listening: the capture sender is a
//! bounded tokio `Sender` whose `try_send` returns an error if the
//! receiver has been dropped or the channel is full, and the emit
//! helper swallows that outcome.
//!
//! Why a `Backend` wrapper instead of post-frame `current_buffer_mut()`?
//! Because ratatui's `Backend::draw` already yields a diff stream — only
//! cells that changed since the last frame are passed through. Hooking in
//! here means we ship exactly one cell per actual on-screen change, with
//! no additional buffer cloning or diffing.

use ratatui::backend::{Backend, ClearType, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
use ratatui::style::{Color, Modifier};
use tokio::sync::mpsc::Sender as BoundedSender;

use super::messages::{WireCell, WireColor};

/// Default capacity for the capture channel. Sized to buffer roughly a
/// couple of seconds of render-rate cell updates. A slow network path
/// that fills the channel causes the TeeBackend's `emit` to drop the
/// oldest events via `try_send` rather than blocking rendering.
pub const DEFAULT_CAPTURE_CAPACITY: usize = 512;

/// Events captured by the `TeeBackend` and shipped to the remote worker.
///
/// The worker batches these into `RemoteMessage::FrameDiff` and related
/// messages before sending on the wire.
///
/// Cursor fields on `CursorPosition` and `CursorVisible(bool)` are
/// observed by tests only today — the wire protocol does not yet carry
/// cursor state to the client — but the fields are kept so the server
/// loop can react without a second capture pathway.
#[derive(Clone, Debug)]
pub enum CaptureEvent {
    /// Cells changed in a single `Backend::draw` call. Ratatui's double-
    /// buffer only yields cells that differ from the previous frame, so
    /// this is already a diff.
    Cells(Vec<WireCell>),
    /// Cursor moved.
    #[allow(dead_code)]
    CursorPosition { col: u16, row: u16 },
    /// Cursor visibility changed.
    #[allow(dead_code)]
    CursorVisible(bool),
    /// The whole screen was cleared. Remote should clear its local mirror.
    Clear,
    /// A `ClearType::*` variant other than `All`. Included for completeness;
    /// the remote can request a keyframe to resync.
    ClearRegion,
    /// Viewport size changed (host-side only — host never accepts client
    /// resize).
    Resize { cols: u16, rows: u16 },
}

/// Backend wrapper that forwards every call to an inner backend and also
/// tees a capture event stream.
pub struct TeeBackend<B: Backend> {
    inner: B,
    capture: BoundedSender<CaptureEvent>,
    /// Last known viewport size; used to detect and emit resize events when
    /// the terminal is polled for size.
    last_size: Option<Size>,
}

impl<B: Backend> TeeBackend<B> {
    pub fn new(inner: B, capture: BoundedSender<CaptureEvent>) -> Self {
        Self {
            inner,
            capture,
            last_size: None,
        }
    }

    fn emit(&self, event: CaptureEvent) {
        // Fire-and-forget, bounded. If the remote worker has dropped the
        // receiver (no peer, or subsystem is off) or the channel is full
        // (slow peer stalling the session task), we silently drop the
        // event. Rendering must never block on network state, and a
        // dropped cell update is always recoverable on the next
        // `Backend::draw` pass (ratatui re-emits any dirty cell).
        let _ = self.capture.try_send(event);
    }
}

impl<B: Backend> Backend for TeeBackend<B> {
    type Error = B::Error;

    fn draw<'a, I>(&mut self, content: I) -> Result<(), Self::Error>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        // Materialize into a Vec so we can emit a capture event AND forward
        // to the inner backend (iterators consume themselves).
        let owned: Vec<(u16, u16, Cell)> = content.map(|(x, y, c)| (x, y, c.clone())).collect();

        if !owned.is_empty() {
            let wire = owned
                .iter()
                .map(|(x, y, cell)| WireCell {
                    row: *y,
                    col: *x,
                    symbol: cell.symbol().to_string(),
                    fg: to_wire_color(cell.fg),
                    bg: to_wire_color(cell.bg),
                    modifier: cell.modifier.bits(),
                })
                .collect();
            self.emit(CaptureEvent::Cells(wire));
        }

        self.inner.draw(owned.iter().map(|(x, y, c)| (*x, *y, c)))
    }

    fn hide_cursor(&mut self) -> Result<(), Self::Error> {
        self.emit(CaptureEvent::CursorVisible(false));
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> Result<(), Self::Error> {
        self.emit(CaptureEvent::CursorVisible(true));
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> Result<Position, Self::Error> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<(), Self::Error> {
        let pos = position.into();
        self.emit(CaptureEvent::CursorPosition {
            col: pos.x,
            row: pos.y,
        });
        self.inner.set_cursor_position(pos)
    }

    fn clear(&mut self) -> Result<(), Self::Error> {
        self.emit(CaptureEvent::Clear);
        self.inner.clear()
    }

    fn clear_region(&mut self, clear_type: ClearType) -> Result<(), Self::Error> {
        match clear_type {
            ClearType::All => self.emit(CaptureEvent::Clear),
            _ => self.emit(CaptureEvent::ClearRegion),
        }
        self.inner.clear_region(clear_type)
    }

    fn size(&self) -> Result<Size, Self::Error> {
        self.inner.size()
    }

    fn window_size(&mut self) -> Result<WindowSize, Self::Error> {
        let ws = self.inner.window_size()?;
        let size = ws.columns_rows;
        if self.last_size != Some(size) {
            self.last_size = Some(size);
            self.emit(CaptureEvent::Resize {
                cols: size.width,
                rows: size.height,
            });
        }
        Ok(ws)
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        self.inner.flush()
    }

    fn append_lines(&mut self, n: u16) -> Result<(), Self::Error> {
        self.inner.append_lines(n)
    }
}

/// Convert a ratatui `Color` to the serde-friendly wire form. All variants
/// are represented; unsupported future variants would trigger a compile
/// error here (good — this is the single place the mapping lives).
fn to_wire_color(c: Color) -> WireColor {
    match c {
        Color::Reset => WireColor::Reset,
        Color::Black => WireColor::Black,
        Color::Red => WireColor::Red,
        Color::Green => WireColor::Green,
        Color::Yellow => WireColor::Yellow,
        Color::Blue => WireColor::Blue,
        Color::Magenta => WireColor::Magenta,
        Color::Cyan => WireColor::Cyan,
        Color::Gray => WireColor::Gray,
        Color::DarkGray => WireColor::DarkGray,
        Color::LightRed => WireColor::LightRed,
        Color::LightGreen => WireColor::LightGreen,
        Color::LightYellow => WireColor::LightYellow,
        Color::LightBlue => WireColor::LightBlue,
        Color::LightMagenta => WireColor::LightMagenta,
        Color::LightCyan => WireColor::LightCyan,
        Color::White => WireColor::White,
        Color::Rgb(r, g, b) => WireColor::Rgb(r, g, b),
        Color::Indexed(i) => WireColor::Indexed(i),
    }
}

/// Inverse of `to_wire_color` — used by the client when rendering received
/// cells back to ratatui-style values.
pub fn from_wire_color(c: WireColor) -> Color {
    match c {
        WireColor::Reset => Color::Reset,
        WireColor::Black => Color::Black,
        WireColor::Red => Color::Red,
        WireColor::Green => Color::Green,
        WireColor::Yellow => Color::Yellow,
        WireColor::Blue => Color::Blue,
        WireColor::Magenta => Color::Magenta,
        WireColor::Cyan => Color::Cyan,
        WireColor::Gray => Color::Gray,
        WireColor::DarkGray => Color::DarkGray,
        WireColor::LightRed => Color::LightRed,
        WireColor::LightGreen => Color::LightGreen,
        WireColor::LightYellow => Color::LightYellow,
        WireColor::LightBlue => Color::LightBlue,
        WireColor::LightMagenta => Color::LightMagenta,
        WireColor::LightCyan => Color::LightCyan,
        WireColor::White => Color::White,
        WireColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
        WireColor::Indexed(i) => Color::Indexed(i),
    }
}

/// Decode a modifier bitmask from the wire back into ratatui's `Modifier`.
pub fn modifier_from_bits(bits: u16) -> Modifier {
    Modifier::from_bits_truncate(bits)
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Cell;
    use ratatui::layout::Rect;
    use ratatui::style::{Color, Modifier, Style};
    use ratatui::text::Line;
    use ratatui::widgets::{Block, Paragraph};
    use tokio::sync::mpsc::{Receiver, channel};

    use super::*;
    use crate::remote::messages::WireColor;

    /// Drain every currently-pending event from a tokio unbounded receiver.
    /// Works outside a tokio runtime because `try_recv` is synchronous.
    fn drain(rx: &mut Receiver<CaptureEvent>) -> Vec<CaptureEvent> {
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            out.push(ev);
        }
        out
    }

    /// The TeeBackend forwards draw calls and produces a capture event that
    /// carries the same cells the inner backend received.
    #[test]
    fn draw_tees_cells_to_capture_channel() {
        let (tx, mut rx) = channel::<CaptureEvent>(64);
        let inner = TestBackend::new(10, 2);
        let tee = TeeBackend::new(inner, tx);
        let mut terminal = Terminal::new(tee).expect("terminal");

        terminal
            .draw(|frame| {
                let area = Rect::new(0, 0, 10, 1);
                frame.render_widget(
                    Paragraph::new(Line::from("hello"))
                        .style(Style::default().fg(Color::Red).bg(Color::Reset)),
                    area,
                );
            })
            .expect("draw");

        let events = drain(&mut rx);
        let mut any_cells = false;
        let mut saw_red_h = false;
        for ev in events {
            if let CaptureEvent::Cells(cells) = ev {
                any_cells = true;
                for cell in cells {
                    if cell.symbol == "h"
                        && cell.row == 0
                        && cell.col == 0
                        && cell.fg == WireColor::Red
                    {
                        saw_red_h = true;
                    }
                }
            }
        }
        assert!(any_cells, "expected at least one Cells capture event");
        assert!(
            saw_red_h,
            "expected the 'h' at (0,0) with fg=Red to be captured"
        );
    }

    /// A hidden-then-shown cursor round-trip flows through as two cursor-
    /// visibility capture events.
    #[test]
    fn cursor_visibility_is_captured() {
        let (tx, mut rx) = channel::<CaptureEvent>(64);
        let inner = TestBackend::new(5, 2);
        let mut tee = TeeBackend::new(inner, tx);
        tee.hide_cursor().unwrap();
        tee.show_cursor().unwrap();
        let visibilities: Vec<bool> = drain(&mut rx)
            .into_iter()
            .filter_map(|e| match e {
                CaptureEvent::CursorVisible(v) => Some(v),
                _ => None,
            })
            .collect();
        assert_eq!(visibilities, vec![false, true]);
    }

    /// `clear()` emits a `Clear` capture event.
    #[test]
    fn clear_is_captured() {
        let (tx, mut rx) = channel::<CaptureEvent>(64);
        let inner = TestBackend::new(5, 2);
        let mut tee = TeeBackend::new(inner, tx);
        tee.clear().unwrap();
        let got_clear = drain(&mut rx)
            .into_iter()
            .any(|e| matches!(e, CaptureEvent::Clear));
        assert!(got_clear, "expected a Clear event");
    }

    /// When the capture receiver is dropped, draws still succeed.
    #[test]
    fn drops_events_gracefully_after_receiver_drops() {
        let (tx, rx) = channel::<CaptureEvent>(64);
        drop(rx);
        let inner = TestBackend::new(5, 2);
        let tee = TeeBackend::new(inner, tx);
        let mut terminal = Terminal::new(tee).expect("terminal");
        terminal
            .draw(|frame| {
                frame.render_widget(Block::bordered(), Rect::new(0, 0, 5, 2));
            })
            .expect("draw must succeed with no receiver");
    }

    /// Round-trip via `to_wire_color` / `from_wire_color` preserves every
    /// ratatui color variant.
    #[test]
    fn color_roundtrip_covers_all_variants() {
        let cases = [
            Color::Reset,
            Color::Black,
            Color::Red,
            Color::Green,
            Color::Yellow,
            Color::Blue,
            Color::Magenta,
            Color::Cyan,
            Color::Gray,
            Color::DarkGray,
            Color::LightRed,
            Color::LightGreen,
            Color::LightYellow,
            Color::LightBlue,
            Color::LightMagenta,
            Color::LightCyan,
            Color::White,
            Color::Rgb(10, 20, 30),
            Color::Indexed(7),
        ];
        for c in cases {
            let round = from_wire_color(to_wire_color(c));
            assert_eq!(c, round, "roundtrip failed for {c:?}");
        }
    }

    /// Modifier bits survive a bitmask round-trip through the wire form.
    #[test]
    fn modifier_bits_roundtrip() {
        let m = Modifier::BOLD | Modifier::UNDERLINED | Modifier::REVERSED;
        let (tx, mut rx) = channel::<CaptureEvent>(64);
        let inner = TestBackend::new(2, 1);
        let tee = TeeBackend::new(inner, tx);
        let mut terminal = Terminal::new(tee).expect("terminal");
        terminal
            .draw(|frame| {
                let buf = frame.buffer_mut();
                let mut cell = Cell::default();
                cell.set_symbol("x")
                    .set_style(Style::default().add_modifier(m));
                *buf = ratatui::buffer::Buffer::empty(Rect::new(0, 0, 2, 1));
                buf[(0, 0)] = cell;
            })
            .expect("draw");
        let got_modifier = drain(&mut rx).into_iter().find_map(|ev| match ev {
            CaptureEvent::Cells(cells) => cells
                .into_iter()
                .find(|c| c.symbol == "x")
                .map(|c| c.modifier),
            _ => None,
        });
        let bits = got_modifier.expect("expected a cell with symbol 'x'");
        assert_eq!(modifier_from_bits(bits), m);
    }
}
