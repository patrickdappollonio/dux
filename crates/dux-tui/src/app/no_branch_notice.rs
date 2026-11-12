//! The changes pane's notice for an agent whose worktree has no branch checked
//! out (a detached checkout, a rebase or a bisect in progress).
//!
//! The agent keeps showing the branch it was last on, which is no longer what
//! the worktree is on, so the pane says so for as long as it lasts. The fact is
//! branch sync's, kept on the engine; the notice goes the pass after a branch is
//! checked out again.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Padding, Paragraph, Widget};

use super::components::wrap_lines::wrap_styled_lines;
use super::{App, RightSection};
use crate::theme::{FocusState, Theme};

/// The notice's heading, the same words the web's strip uses.
const HEADING: &str = "No branch checked out";

/// Columns the box itself takes from each row: a border and a padding cell on
/// either side.
const FRAME_COLUMNS: u16 = 4;

/// Rows the box itself takes: its top and bottom border.
const FRAME_ROWS: usize = 2;

impl App {
    /// Draw the notice at the top of `area` while the selected agent's worktree
    /// has no branch checked out, and hand back what is left of `area` for the
    /// list under it.
    ///
    /// Only the pane's top list, `section` [`RightSection::Unstaged`], carries
    /// it: there it heads the list over its search row, the way the browser's
    /// strip heads its filter.
    ///
    /// The box is as tall as its wrapped text, which is never cut: a branch name
    /// longer than a row carries on into the next. When the area cannot hold the
    /// box and at least one list row as well, the notice is left out and the
    /// list keeps the whole area.
    pub(super) fn render_no_branch_notice(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        section: RightSection,
    ) -> Rect {
        if section != RightSection::Unstaged {
            return area;
        }
        let Some(branch) = self.no_branch_notice_branch() else {
            return area;
        };
        let text_width = usize::from(area.width.saturating_sub(FRAME_COLUMNS));
        let rows = wrap_styled_lines(&notice_lines(branch, &self.theme), text_width);
        if rows.is_empty() {
            return area;
        }
        let Ok(height) = u16::try_from(rows.len() + FRAME_ROWS) else {
            return area;
        };
        if height >= area.height {
            return area;
        }
        let notice = Rect { height, ..area };
        let block = self
            .themed_block("", FocusState::Unfocused)
            .border_style(Style::default().fg(self.theme.warning_fg))
            .padding(Padding::horizontal(1));
        let inner = block.inner(notice);
        block.render(notice, frame.buffer_mut());
        Paragraph::new(rows).render(inner, frame.buffer_mut());
        Rect {
            y: area.y + height,
            height: area.height - height,
            ..area
        }
    }

    /// The branch the selected agent still shows, while its worktree has no
    /// branch checked out; `None` otherwise.
    fn no_branch_notice_branch(&self) -> Option<&str> {
        let session = self.selected_session()?;
        if !self.engine.no_branch_checked_out(&session.id) {
            return None;
        }
        session.branch_name()
    }
}

/// The heading on a line of its own, and the sentence naming the branch dux
/// keeps showing.
fn notice_lines<'a>(branch: &str, theme: &Theme) -> [Line<'a>; 2] {
    [
        Line::from(Span::styled(
            HEADING,
            Style::default()
                .fg(theme.warning_fg)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::styled(
            format!("dux keeps showing {branch} until one is checked out again."),
            Style::default().fg(theme.text_fg),
        )),
    ]
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::style::Modifier;

    use crate::app::test_support::{default_bindings, test_app};
    use crate::app::{App, RightSection};
    use dux_core::worker::BranchCheckout;

    /// The app with its one agent selected, last seen on `branch`, with its
    /// worktree in `checkout`.
    fn app_on(branch: &str, checkout: BranchCheckout) -> App {
        let mut app = test_app(default_bindings());
        app.selected_left = 1;
        let session = &mut app.engine.sessions[0];
        if let dux_core::model::AgentWorkspace::Managed(managed) = &mut session.workspace {
            managed.branch_name = branch.to_string();
        }
        if checkout == BranchCheckout::NoBranch {
            let id = session.id.clone();
            app.engine.no_branch_checked_out.insert(id);
        }
        app
    }

    /// The notice drawn into a `width` x `height` area: the buffer, and the
    /// area it left for the list.
    fn draw(app: &App, width: u16, height: u16) -> (Buffer, Rect) {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        let mut left = Rect::default();
        terminal
            .draw(|frame| {
                left = app.render_no_branch_notice(frame, frame.area(), RightSection::Unstaged);
            })
            .expect("render frame");
        (terminal.backend().buffer().clone(), left)
    }

    /// The same, as rows of text.
    fn render(app: &App, width: u16, height: u16) -> (Vec<String>, Rect) {
        let (buffer, left) = draw(app, width, height);
        let rows = (0..height)
            .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
            .collect();
        (rows, left)
    }

    #[test]
    fn the_notice_names_the_branch_dux_keeps_showing_and_the_list_goes_under_it() {
        let app = app_on("feat-login", BranchCheckout::NoBranch);
        let (rows, left) = render(&app, 60, 12);
        assert_eq!(
            rows[1],
            "│ No branch checked out                                    │"
        );
        assert_eq!(
            rows[2],
            "│ dux keeps showing feat-login until one is checked out    │"
        );
        assert_eq!(
            rows[3],
            "│ again.                                                   │"
        );
        assert!(rows[0].starts_with('╭') && rows[4].starts_with('╰'));
        assert_eq!(
            left,
            Rect::new(0, 5, 60, 7),
            "the list starts under the box"
        );
        let (buffer, _) = draw(&app, 60, 12);
        assert_eq!(
            buffer[(0, 2)].fg,
            app.theme.warning_fg,
            "the box is outlined in the warning colour"
        );
        assert_eq!(buffer[(2, 1)].fg, app.theme.warning_fg);
        assert!(
            buffer[(2, 1)].modifier.contains(Modifier::BOLD),
            "the heading is bold"
        );
        assert_eq!(
            buffer[(2, 2)].fg,
            app.theme.text_fg,
            "the sentence is body text"
        );
    }

    #[test]
    fn a_branch_name_longer_than_a_row_wraps_rather_than_being_cut() {
        let branch = "a-very-long-branch-name-that-cannot-fit-on-one-row";
        let app = app_on(branch, BranchCheckout::NoBranch);
        let (rows, left) = render(&app, 30, 20);
        let text: String = rows
            .iter()
            .filter(|row| row.starts_with('│'))
            .map(|row| {
                let inner: String = row.chars().skip(2).take(26).collect();
                inner.trim_end().to_string()
            })
            .collect();
        assert!(
            text.contains(branch),
            "every character of the name is drawn, in order: {rows:#?}"
        );
        assert!(!text.contains('…'), "nothing is ellipsized: {rows:#?}");
        let bottom = rows.iter().position(|row| row.starts_with('╰'));
        assert_eq!(bottom.map(|row| row + 1), Some(usize::from(left.y)));
    }

    #[test]
    fn nothing_is_drawn_while_a_branch_is_checked_out() {
        let app = app_on("feat-login", BranchCheckout::OnBranch);
        let (rows, left) = render(&app, 60, 12);
        assert!(rows.iter().all(|row| row.trim().is_empty()), "{rows:#?}");
        assert_eq!(left, Rect::new(0, 0, 60, 12));
    }

    #[test]
    fn only_the_top_list_carries_the_notice() {
        let app = app_on("feat-login", BranchCheckout::NoBranch);
        let mut terminal = Terminal::new(TestBackend::new(60, 12)).expect("terminal");
        let mut left = Rect::default();
        terminal
            .draw(|frame| {
                left = app.render_no_branch_notice(frame, frame.area(), RightSection::Staged);
            })
            .expect("render frame");
        assert_eq!(left, Rect::new(0, 0, 60, 12));
    }

    #[test]
    fn the_notice_steps_aside_when_it_would_leave_no_row_for_the_list() {
        let app = app_on("feat-login", BranchCheckout::NoBranch);
        let (rows, left) = render(&app, 60, 5);
        assert!(rows.iter().all(|row| row.trim().is_empty()), "{rows:#?}");
        assert_eq!(left, Rect::new(0, 0, 60, 5));
    }
}
