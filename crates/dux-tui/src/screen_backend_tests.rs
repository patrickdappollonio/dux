use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::widgets::Paragraph;
use ratatui::{TerminalOptions, Viewport};

use super::*;

fn draw_text(terminal: &mut Terminal<TestBackend>, text: &str, cursor: Option<(u16, u16)>) {
    terminal
        .draw(|frame| {
            frame.render_widget(Paragraph::new(text), frame.area());
            if let Some(cursor) = cursor {
                frame.set_cursor_position(cursor);
            }
        })
        .expect("draw");
}

/// What a force-redraw leaves on screen, the cursor included, is what
/// ratatui's own clear left there, both right after it and after the frame
/// drawn next.
#[test]
fn clearing_the_screen_leaves_it_as_ratatui_s_clear_does() {
    for cursor in [None, Some((3, 1))] {
        let mut theirs = Terminal::new(TestBackend::new(12, 4)).expect("terminal");
        let mut ours = Terminal::new(TestBackend::new(12, 4)).expect("terminal");
        for terminal in [&mut theirs, &mut ours] {
            draw_text(terminal, "first\nframe", cursor);
        }
        theirs.clear().expect("ratatui's clear");
        clear_screen(&mut ours).expect("dux's clear");
        assert_eq!(ours.backend().buffer(), theirs.backend().buffer());
        assert_eq!(
            ours.backend_mut().get_cursor_position().expect("cursor"),
            theirs.backend_mut().get_cursor_position().expect("cursor")
        );
        for terminal in [&mut theirs, &mut ours] {
            draw_text(terminal, "first\nframe", cursor);
        }
        assert_eq!(ours.backend().buffer(), theirs.backend().buffer());
        assert!(
            ours.backend()
                .buffer()
                .content()
                .iter()
                .any(|cell| cell.symbol() == "f"),
            "the unchanged frame was painted again"
        );
        assert_eq!(
            ours.backend_mut().get_cursor_position().expect("cursor"),
            theirs.backend_mut().get_cursor_position().expect("cursor")
        );
    }
}

/// Asking the screen's backend where the cursor is, directly or through
/// ratatui's clear, writes no question to the terminal and fails.
#[test]
fn the_screen_backend_never_asks_where_the_cursor_is() {
    let mut backend = ScreenBackend::new(Vec::new());
    let refused = backend.get_cursor_position().expect_err("refused");
    assert_eq!(refused.to_string(), CURSOR_QUESTION_REFUSED);

    let written = Written::default();
    let viewport = Viewport::Fixed(Rect::new(0, 0, 10, 3));
    let mut terminal = Terminal::with_options(
        ScreenBackend::new(written.clone()),
        TerminalOptions { viewport },
    )
    .expect("terminal");
    let refused = terminal.clear().expect_err("refused");
    assert_eq!(refused.to_string(), CURSOR_QUESTION_REFUSED);
    assert!(written.0.borrow().is_empty(), "nothing was written");
}

#[derive(Clone, Default)]
struct Written(std::rc::Rc<std::cell::RefCell<Vec<u8>>>);

impl Write for Written {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.borrow_mut().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
