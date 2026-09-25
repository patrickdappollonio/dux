//! The list half of every Picker-family modal: rows with a selection cursor.
//!
//! One component owns everything a picker's list does the same way: painting
//! the rows, highlighting the selected one with the theme's selection style,
//! scrolling so that row is always on screen, the scroll indicator the welcome
//! and What's new screens wear (see [`super::scroll_view`]), the empty state,
//! and the geometry a click is resolved against. What the rows SAY stays with
//! each picker, which builds its own [`ListItem`]s.
//!
//! The selection highlight marks the row the confirm key would act on, so an
//! empty list, which has nothing to act on, draws its empty state with no
//! highlight at all.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::Text;
use ratatui::widgets::{Block, List, ListItem, ListState, Paragraph, StatefulWidget, Widget};

use super::scroll_view::render_scroll_indicator;
use crate::theme::Theme;

/// A picker's list, ready to render.
pub(crate) struct PickerList<'a> {
    rows: Vec<ListItem<'a>>,
    selected: Option<usize>,
    empty: Text<'a>,
    block: Option<Block<'a>>,
    indicator_frame: Option<Rect>,
}

/// What a rendered picker list publishes for the mouse: the rows' rect, how
/// many items there are, and the first one on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PickerListLayout {
    pub(crate) list: Rect,
    pub(crate) items: usize,
    pub(crate) offset: usize,
}

impl<'a> PickerList<'a> {
    /// `selected` indexes `rows` and is clamped to the last one; `None` draws
    /// no highlight. `empty` is what the list says when `rows` is empty; every
    /// picker names one, so no list is ever a silent blank.
    pub(crate) fn new(
        rows: Vec<ListItem<'a>>,
        selected: Option<usize>,
        empty: impl Into<Text<'a>>,
    ) -> Self {
        Self {
            rows,
            selected,
            empty: empty.into(),
            block: None,
            indicator_frame: None,
        }
    }

    /// The block the rows sit in. Its right border is where the scroll
    /// indicator goes, unless [`Self::indicator_frame`] names another.
    pub(crate) fn block(mut self, block: Block<'a>) -> Self {
        self.block = Some(block);
        self
    }

    /// The rect whose right border column carries the scroll indicator, for a
    /// list drawn with no block of its own inside a bordered modal.
    pub(crate) fn indicator_frame(mut self, frame: Rect) -> Self {
        self.indicator_frame = Some(frame);
        self
    }

    /// Paint the list into `area` and return what a click needs.
    pub(crate) fn render(self, frame: &mut Frame, area: Rect, theme: &Theme) -> PickerListLayout {
        let list = self.block.as_ref().map_or(area, |block| block.inner(area));
        let indicator_frame = self.indicator_frame.unwrap_or(area);
        let items = self.rows.len();

        if items == 0 {
            if let Some(block) = self.block {
                block.render(area, frame.buffer_mut());
            }
            Paragraph::new(self.empty).render(list, frame.buffer_mut());
            return PickerListLayout {
                list,
                items: 0,
                offset: 0,
            };
        }

        let heights: Vec<usize> = self.rows.iter().map(ListItem::height).collect();
        let mut state =
            ListState::default().with_selected(self.selected.map(|index| index.min(items - 1)));
        let mut widget = List::new(self.rows).highlight_style(theme.selection_style());
        if let Some(block) = self.block {
            widget = widget.block(block);
        }
        StatefulWidget::render(widget, area, frame.buffer_mut(), &mut state);
        let offset = state.offset();

        render_scroll_indicator(
            frame,
            indicator_frame,
            list,
            offset,
            items_that_fit(&heights, offset, list.height),
            items,
            theme,
        );
        PickerListLayout {
            list,
            items,
            offset,
        }
    }
}

impl PickerListLayout {
    /// The item under a click, for a list whose items are each `rows_per_item`
    /// screen rows tall, or `None` for a click outside the rows or below the
    /// last item.
    pub(crate) fn item_at(self, rows_per_item: u16, column: u16, row: u16) -> Option<usize> {
        let inside = column >= self.list.x
            && column < self.list.x.saturating_add(self.list.width)
            && row >= self.list.y
            && row < self.list.y.saturating_add(self.list.height);
        if !inside {
            return None;
        }
        let relative_row = usize::from(row - self.list.y);
        let index = self
            .offset
            .saturating_add(relative_row / usize::from(rows_per_item.max(1)));
        (index < self.items).then_some(index)
    }
}

/// Whole items that fit in `height` rows starting at `offset`: the scroll
/// indicator's viewport, in the same unit as its offset and total.
fn items_that_fit(heights: &[usize], offset: usize, height: u16) -> usize {
    let mut left = usize::from(height);
    heights
        .iter()
        .skip(offset)
        .take_while(|&&rows| {
            let fits = rows <= left;
            left = left.saturating_sub(rows);
            fits
        })
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::components::MARKER_GLYPHS;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::widgets::Borders;

    fn draw(list: PickerList<'static>, theme: &Theme) -> (Buffer, PickerListLayout) {
        let mut terminal = Terminal::new(TestBackend::new(20, 7)).expect("terminal");
        let mut layout = None;
        terminal
            .draw(|frame| layout = Some(list.render(frame, Rect::new(0, 0, 20, 7), theme)))
            .expect("draw");
        (terminal.backend().buffer().clone(), layout.expect("layout"))
    }

    fn rows(count: usize) -> Vec<ListItem<'static>> {
        (0..count)
            .map(|n| ListItem::new(format!("row {n:02}")))
            .collect()
    }

    fn highlighted(buf: &Buffer, theme: &Theme) -> Vec<u16> {
        let bg = theme.selection_style().bg;
        (0..buf.area.height)
            .filter(|&y| (1..buf.area.width - 1).all(|x| Some(buf[(x, y)].bg) == bg))
            .collect()
    }

    #[test]
    fn the_selection_is_scrolled_into_view_and_highlighted() {
        let theme = Theme::default_dark();
        let list = PickerList::new(rows(30), Some(20), "nothing")
            .block(Block::default().borders(Borders::ALL));
        let (buf, layout) = draw(list, &theme);
        assert_eq!(layout.items, 30);
        let lit = highlighted(&buf, &theme);
        assert_eq!(lit.len(), 1, "one highlighted row");
        let y = lit[0];
        let text: String = (1..8).map(|x| buf[(x, y)].symbol().to_string()).collect();
        assert_eq!(text, "row 20 ");
        assert_eq!(layout.item_at(1, 3, y), Some(20), "a click there is row 20");
    }

    #[test]
    fn a_tall_list_carries_the_scroll_indicator() {
        let theme = Theme::default_dark();
        let list = PickerList::new(rows(30), Some(0), "nothing")
            .block(Block::default().borders(Borders::ALL));
        let (buf, _) = draw(list, &theme);
        let marker = &buf[(19, 5)];
        assert_eq!(marker.symbol(), MARKER_GLYPHS[0], "more below");
        assert_eq!(
            marker.fg,
            crate::app::components::scroll_indicator_color(&theme)
        );
    }

    #[test]
    fn an_empty_list_says_so_and_highlights_nothing() {
        let theme = Theme::default_dark();
        let list = PickerList::new(Vec::new(), Some(0), "Nothing here.")
            .block(Block::default().borders(Borders::ALL));
        let (buf, layout) = draw(list, &theme);
        let text: String = (1..20).map(|x| buf[(x, 1)].symbol().to_string()).collect();
        assert!(text.starts_with("Nothing here."), "{text:?}");
        assert!(highlighted(&buf, &theme).is_empty());
        assert_eq!(layout.items, 0);
        assert_eq!(
            layout.item_at(1, 3, 1),
            None,
            "the empty state is not a row"
        );
    }

    #[test]
    fn multi_row_items_count_whole_items() {
        assert_eq!(items_that_fit(&[2, 2, 2, 2], 0, 5), 2);
        assert_eq!(items_that_fit(&[1, 1, 1], 1, 5), 2);
        let layout = PickerListLayout {
            list: Rect::new(0, 0, 10, 6),
            items: 5,
            offset: 1,
        };
        assert_eq!(layout.item_at(2, 0, 3), Some(2));
        assert_eq!(layout.item_at(2, 0, 7), None, "below the rows");
        assert_eq!(layout.item_at(2, 11, 3), None, "right of the rows");
    }
}
