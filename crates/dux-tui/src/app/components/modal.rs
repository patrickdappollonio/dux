use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Clear, Widget};

/// Paints the shared blank surface underneath modal-specific borders and content.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Modal {
    surface_style: Style,
    isolation_style: Style,
}

impl Modal {
    pub(crate) const fn new(surface_style: Style, isolation_style: Style) -> Self {
        Self {
            surface_style,
            isolation_style,
        }
    }

    pub(crate) fn render(self, area: Rect, bounds: Rect, buffer: &mut Buffer) {
        let left = area.x.saturating_sub(1).max(bounds.x);
        let top = area.y.saturating_sub(1).max(bounds.y);
        let right = area.right().saturating_add(1).min(bounds.right());
        let bottom = area.bottom().saturating_add(1).min(bounds.bottom());
        let isolation = Rect::new(
            left,
            top,
            right.saturating_sub(left),
            bottom.saturating_sub(top),
        );

        Clear.render(isolation, buffer);
        buffer.set_style(isolation, self.isolation_style);
        Clear.render(area, buffer);
        buffer.set_style(area, self.surface_style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    #[test]
    fn modal_paints_a_clipped_one_cell_isolation_ring() {
        let bounds = Rect::new(0, 0, 8, 6);
        let area = Rect::new(0, 1, 6, 4);
        let mut buffer = Buffer::empty(bounds);
        for cell in &mut buffer.content {
            cell.set_symbol("x");
        }
        let surface = Style::default().bg(Color::Blue);
        let isolation = Style::default().bg(Color::DarkGray);

        Modal::new(surface, isolation).render(area, bounds, &mut buffer);

        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                assert_eq!(buffer[(x, y)].symbol(), " ");
                assert_eq!(buffer[(x, y)].bg, Color::Blue);
            }
        }
        for (x, y) in [(6, 1), (6, 2), (6, 3), (6, 4), (0, 5), (5, 5)] {
            assert_eq!(buffer[(x, y)].symbol(), " ");
            assert_eq!(buffer[(x, y)].bg, Color::DarkGray);
        }
        assert_eq!(buffer[(7, 0)].symbol(), "x");
    }
}
