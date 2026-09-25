//! The Confirm-family dialog: one frame and one button row for every modal the
//! registry declares [`super::modal::ModalFamily::Confirm`].
//!
//! Every such dialog is the same shape: a titled ring, a body of prose, an
//! optional block of controls (checkboxes) below it, and a Cancel / act pair
//! of buttons. [`App::render_confirm_dialog`] draws all of it from a
//! [`ConfirmDialog`] spec:
//!
//! - **Sized from its content, clamped to the screen.** The dialog is
//!   [`CONFIRM_DIALOG_WIDTH`] wide (or the screen's width, if that is less) and
//!   exactly as tall as its wrapped prose plus its controls and buttons. A
//!   fixed percentage of the screen used to leave room for one row of prose on
//!   an 80x24 terminal and drop the rest with nothing to say it had.
//! - **A body too tall for the screen scrolls; it is never cut.** The controls
//!   and buttons keep their rows and the prose gets what is left, scrolled the
//!   way the welcome and "What's new" screens scroll theirs (the shared
//!   [`render_scroll_view`], its marker in the right border column). The Help
//!   scope's scroll keys and the mouse wheel move it, and only while there is
//!   something to scroll, so they cannot take a key from a dialog that fits.
//! - **One button rule.** Both buttons are [`shared_button_width`] of their
//!   labels (plus any label the row must stay wide enough for),
//!   [`super::components::button::BUTTON_GAP`] apart and centred as a pair, focus and
//!   press drawn by [`button_state_for`].
//!
//! The frame publishes nothing itself: the caller still publishes its own
//! [`super::OverlayMouseLayout`] variant from the returned rects, so click
//! routing is unchanged.
//!
//! Only the reload-failed dialog keeps a frame of its own, because its body is
//! the error-dialog frame it shares with a Report dialog (a scroll offset kept
//! in the prompt, and a scroll hint in its border); its button row still comes
//! from [`App::render_confirm_buttons`].

use std::cell::Cell;

use ratatui::Frame;
use ratatui::crossterm::event::{KeyEvent, MouseEvent, MouseEventKind};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::text::Line;

use super::App;
use super::components::{
    BUTTON_HEIGHT, Button, ButtonKind, ButtonPressedTarget, ScrollViewRender, button_row,
    button_state_for, render_scroll_view, shared_button_width, wrap_styled_lines,
};
use super::modal::{ModalFamily, modal_spec};
use super::render::centered_rect_exact;
use crate::keybindings::{Action, BindingScope};

/// The width every Confirm dialog asks for, border included. Narrower only on
/// a narrower screen.
pub(crate) const CONFIRM_DIALOG_WIDTH: u16 = 60;

/// Rows one notch of the mouse wheel scrolls a Confirm body, as it scrolls the
/// welcome screen.
const WHEEL_ROWS: i32 = 3;

/// One of a Confirm dialog's two buttons.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ConfirmButton<'a> {
    label: &'a str,
    kind: ButtonKind,
    target: ButtonPressedTarget,
    focused: bool,
    enabled: bool,
}

impl<'a> ConfirmButton<'a> {
    /// A button the dialog's focus is (`focused`) or is not on. Press feedback
    /// comes from the app's pressed-button state, keyed by `target`.
    pub(crate) fn new(
        label: &'a str,
        kind: ButtonKind,
        target: ButtonPressedTarget,
        focused: bool,
    ) -> Self {
        Self {
            label,
            kind,
            target,
            focused,
            enabled: true,
        }
    }

    /// Draw the button disabled when `enabled` is false.
    pub(crate) fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }
}

/// Everything a Confirm dialog says and offers.
pub(crate) struct ConfirmDialog<'a> {
    /// The ring's title. `'static` because it is also the key the body's
    /// scroll offset is kept under: a different dialog starts at the top.
    pub(crate) title: &'static str,
    /// The prose, unwrapped. The frame wraps it to its own width with the
    /// shared wrapper, so a name chip stays whole on one row.
    pub(crate) body: Vec<Line<'static>>,
    /// Rows of controls (checkboxes) between the body and the buttons,
    /// measured at [`confirm_inner_width`]. Zero for none. A block of controls
    /// gets one blank row above it and one below, so a checkbox never sits
    /// flush against the prose or against a button.
    pub(crate) controls_height: u16,
    /// The safe button, on the left.
    pub(crate) cancel: ConfirmButton<'a>,
    /// The button that acts, on the right.
    pub(crate) act: ConfirmButton<'a>,
    /// Labels the row must stay wide enough for although neither button shows
    /// them now: a dialog that swaps a label with its state lists every label
    /// it can show, so a toggle never moves the buttons under the pointer.
    pub(crate) reserve_labels: &'a [&'a str],
}

/// Where a Confirm dialog put its parts, for the caller to paint its controls
/// into and to publish its button rects under its own layout variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ConfirmLayout {
    /// The block reserved for the dialog's controls (no rows when it asked for
    /// none).
    pub(crate) controls: Rect,
    pub(crate) cancel: Rect,
    pub(crate) act: Rect,
}

/// The open Confirm dialog's body scroll: the offset the user has moved it to,
/// and the extent the last frame painted.
///
/// Kept per frame in [`super::OverlayMouseLayoutState`] rather than in each
/// prompt, so no dialog carries a scroll field of its own. It follows the
/// dialog on screen: a frame that paints a different dialog starts it at the
/// top, and a frame that paints none forgets it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ConfirmBodyScroll {
    /// Which dialog (by title) the offset belongs to.
    owner: Option<&'static str>,
    offset: usize,
    viewport: ScrollViewRender,
    /// Whether the current frame painted a Confirm body. Read after the frame,
    /// it says whether the viewport describes a dialog that is on screen.
    painted: bool,
}

impl ConfirmBodyScroll {
    /// Start a frame: a dialog the previous frame did not paint is gone, and
    /// its offset goes with it.
    pub(crate) fn new_frame(cell: &Cell<Self>) {
        let mut state = cell.get();
        if !state.painted {
            state = Self::default();
        }
        state.painted = false;
        cell.set(state);
    }

    /// The offset to paint `owner`'s body from.
    fn offset_for(self, owner: &'static str) -> usize {
        if self.owner == Some(owner) {
            self.offset
        } else {
            0
        }
    }

    /// What `owner`'s body painted this frame.
    fn painted(owner: &'static str, viewport: ScrollViewRender) -> Self {
        Self {
            owner: Some(owner),
            offset: viewport.offset,
            viewport,
            painted: true,
        }
    }

    /// Whether a Confirm body on screen has rows out of view.
    fn scrollable(self) -> bool {
        self.painted && self.viewport.scrollable()
    }

    /// The offset moved by `delta` rows, clamped to the painted extent.
    fn scrolled_by(mut self, delta: i32) -> Self {
        let max = i64::try_from(self.viewport.max_offset()).unwrap_or(i64::MAX);
        let next =
            (i64::try_from(self.offset).unwrap_or(i64::MAX) + i64::from(delta)).clamp(0, max);
        self.offset = usize::try_from(next).unwrap_or(0);
        self
    }
}

/// The width inside a Confirm dialog's border ring on `screen`: the width its
/// body wraps to and its controls are measured at.
pub(crate) fn confirm_inner_width(screen: Rect) -> u16 {
    confirm_width(screen).saturating_sub(2)
}

fn confirm_width(screen: Rect) -> u16 {
    CONFIRM_DIALOG_WIDTH.min(screen.width.max(1))
}

impl App {
    /// Paint a Confirm dialog: dim the app, open the titled frame sized to its
    /// content, then the body (scrolling when the screen is too short for it)
    /// and the button pair. The caller paints its controls into the returned
    /// `controls` block and publishes the button rects.
    pub(crate) fn render_confirm_dialog(
        &self,
        frame: &mut Frame,
        dialog: ConfirmDialog<'_>,
    ) -> ConfirmLayout {
        let screen = frame.area();
        let wrapped = wrap_styled_lines(&dialog.body, usize::from(confirm_inner_width(screen)));
        let total = u16::try_from(wrapped.len()).unwrap_or(u16::MAX);

        let controls_gap = u16::from(dialog.controls_height > 0);
        let below_body = controls_gap
            .saturating_add(dialog.controls_height)
            .saturating_add(1)
            .saturating_add(BUTTON_HEIGHT);
        // The controls and buttons keep their rows; the prose gets the rest and
        // scrolls when that is not all of it.
        let body_rows = total.min(
            screen
                .height
                .saturating_sub(below_body.saturating_add(2))
                .max(1),
        );
        let area = centered_rect_exact(
            confirm_width(screen),
            body_rows.saturating_add(below_body).saturating_add(2),
            screen,
        );
        let inner = self.open_modal_frame(frame, dialog.title, area).inner;

        let [body, _, controls, _, buttons] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(body_rows),
                Constraint::Length(controls_gap),
                Constraint::Length(dialog.controls_height),
                Constraint::Length(1),
                Constraint::Length(BUTTON_HEIGHT),
            ])
            .areas(inner);

        let scroll = self.overlay_layout.confirm_scroll.get();
        let viewport = render_scroll_view(
            frame,
            area,
            body,
            wrapped,
            scroll.offset_for(dialog.title),
            &self.theme,
        );
        self.overlay_layout
            .confirm_scroll
            .set(ConfirmBodyScroll::painted(dialog.title, viewport));

        let [cancel, act] = self.render_confirm_buttons(
            frame,
            buttons,
            dialog.cancel,
            dialog.act,
            dialog.reserve_labels,
        );
        ConfirmLayout {
            controls,
            cancel,
            act,
        }
    }

    /// The Confirm family's button pair at the top of `area`: one shared width
    /// for both (wide enough for every label in `reserve_labels` too), the one
    /// gap apart, centred as a pair. Returns the Cancel-side and act-side rects.
    pub(crate) fn render_confirm_buttons(
        &self,
        frame: &mut Frame,
        area: Rect,
        cancel: ConfirmButton<'_>,
        act: ConfirmButton<'_>,
        reserve_labels: &[&str],
    ) -> [Rect; 2] {
        let mut labels = vec![cancel.label, act.label];
        labels.extend_from_slice(reserve_labels);
        let rects = button_row::<2>(area, shared_button_width(&labels));
        for (button, rect) in [cancel, act].into_iter().zip(rects) {
            Button::new(button.label)
                .kind(button.kind)
                .state(button_state_for(
                    button.target,
                    self.pressed_button,
                    button.focused,
                    button.enabled,
                ))
                .render(frame, rect, &self.theme);
        }
        rects
    }

    /// Apply `key` to the open Confirm dialog's body scroll, reporting whether
    /// it scrolled (and so must not reach the dialog's own keys).
    ///
    /// Only while the body has rows out of view: a dialog that fits keeps every
    /// key it had. The vocabulary is the Help scope's, the one the error
    /// dialogs scroll by, so it stays rebindable.
    pub(crate) fn scroll_confirm_body_for(&mut self, key: &KeyEvent) -> bool {
        if !self.confirm_body_scrollable() {
            return false;
        }
        let Some(action) = self.bindings.lookup(key, BindingScope::Help) else {
            return false;
        };
        let visible = self.overlay_layout.confirm_scroll.get().viewport.viewport;
        let page = i32::try_from(visible.max(1)).unwrap_or(i32::MAX);
        let delta = match action {
            Action::MoveDown => 1,
            Action::MoveUp => -1,
            Action::ScrollPageDown => page,
            Action::ScrollPageUp => -page,
            Action::ScrollToBottom => i32::from(u16::MAX),
            Action::ScrollToTop => -i32::from(u16::MAX),
            _ => return false,
        };
        self.scroll_confirm_body(delta);
        true
    }

    /// The mouse wheel over an open Confirm dialog whose body scrolls. `None`
    /// for every other event, so it reaches the dialog's own routing.
    pub(crate) fn handle_confirm_body_wheel(&mut self, mouse: &MouseEvent) -> Option<bool> {
        let delta = match mouse.kind {
            MouseEventKind::ScrollDown => WHEEL_ROWS,
            MouseEventKind::ScrollUp => -WHEEL_ROWS,
            _ => return None,
        };
        if !self.confirm_body_scrollable() {
            return None;
        }
        self.scroll_confirm_body(delta);
        Some(false)
    }

    fn confirm_body_scrollable(&self) -> bool {
        modal_spec(&self.prompt).map(|spec| spec.family) == Some(ModalFamily::Confirm)
            && self.overlay_layout.confirm_scroll.get().scrollable()
    }

    fn scroll_confirm_body(&mut self, delta: i32) {
        let cell = &self.overlay_layout.confirm_scroll;
        cell.set(cell.get().scrolled_by(delta));
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::crossterm::event::{
        KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    };
    use ratatui::layout::Rect;

    use super::super::components::button::BUTTON_GAP;
    use super::super::components::shared_button_width;
    use super::super::modal::tests::every_prompt;
    use super::super::modal::{ModalFamily, modal_spec};
    use super::super::test_support::{default_bindings, test_app};
    use super::super::*;

    /// The smallest terminal dux is expected to be usable in.
    const SMALL: (u16, u16) = (80, 24);
    /// Tall and wide enough that nothing a confirm dialog says is ever cut.
    const ROOMY: (u16, u16) = (200, 80);

    fn render_at(app: &mut App, prompt: PromptState, (width, height): (u16, u16)) -> Buffer {
        app.prompt = prompt;
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal.draw(|frame| app.render(frame)).expect("render");
        terminal.backend().buffer().clone()
    }

    /// Paint the prompt that is open now again, after a key or a click.
    fn rerender(app: &mut App, size: (u16, u16)) -> Buffer {
        let prompt = app.prompt.clone();
        render_at(app, prompt, size)
    }

    fn row_text(buf: &Buffer, y: u16, from: u16, to: u16) -> String {
        (from..to).map(|x| buf[(x, y)].symbol()).collect()
    }

    fn screen(buf: &Buffer) -> String {
        (0..buf.area.height)
            .map(|y| row_text(buf, y, 0, buf.area.width))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The words painted inside the modal's border ring, top to bottom. Two
    /// renders of the same dialog that wrap at different widths still say the
    /// same words, so a word missing from one of them was cut.
    fn modal_words(buf: &Buffer, modal: Rect) -> Vec<String> {
        let inner_x = modal.x + 1;
        let inner_right = modal.right().saturating_sub(1);
        (modal.y + 1..modal.bottom().saturating_sub(1))
            .flat_map(|y| {
                row_text(buf, y, inner_x, inner_right)
                    .split_whitespace()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// The Cancel-side and act-side button rects a Confirm dialog published, in
    /// that order.
    fn confirm_buttons(layout: &OverlayMouseLayout) -> Option<[Rect; 2]> {
        use OverlayMouseLayout as L;
        Some(match *layout {
            L::ConfirmKillRunning {
                cancel_button,
                kill_button,
            } => [cancel_button, kill_button],
            L::ConfirmDeleteAgent {
                cancel_button,
                delete_button,
                ..
            }
            | L::ConfirmDeleteWorktree {
                cancel_button,
                delete_button,
                ..
            }
            | L::ConfirmDeleteTerminal {
                cancel_button,
                delete_button,
            }
            | L::ConfirmDeleteMacro {
                cancel_button,
                delete_button,
            } => [cancel_button, delete_button],
            L::ConfirmCloseTab {
                cancel_button,
                confirm_button,
            }
            | L::ConfirmDetachAgent {
                cancel_button,
                confirm_button,
            }
            | L::ConfirmRecreateWorkingCopy {
                cancel_button,
                confirm_button,
            }
            | L::ConfirmCheckoutDefaultBranch {
                cancel_button,
                confirm_button,
            }
            | L::ConfirmDeleteProject {
                cancel_button,
                confirm_button,
            }
            | L::ConfirmRemoveProject {
                cancel_button,
                confirm_button,
            } => [cancel_button, confirm_button],
            L::ConfirmQuit {
                cancel_button,
                quit_button,
            } => [cancel_button, quit_button],
            L::ConfirmDiscardFile {
                cancel_button,
                discard_button,
            } => [cancel_button, discard_button],
            L::ConfirmCreateInitialCommit {
                cancel_button,
                create_button,
            } => [cancel_button, create_button],
            L::ConfirmInitRepo {
                cancel_button,
                init_button,
            } => [cancel_button, init_button],
            L::ConfirmNonDefaultBranch {
                cancel_button,
                add_button,
                ..
            } => [cancel_button, add_button],
            L::ConfirmUseExistingBranch {
                cancel_button,
                use_button,
            } => [cancel_button, use_button],
            L::ConfigReloadFailed {
                close_button,
                apply_button,
                ..
            } => [close_button, apply_button],
            _ => return None,
        })
    }

    /// Labels a button row reserves width for without painting them in this
    /// state, with the reason: (fixture, label).
    ///
    /// The non-default-branch dialog swaps its act label with its checkbox, and
    /// sizes the row for both so a toggle never moves the buttons under the
    /// pointer.
    const RESERVED_LABELS: &[(&str, &str)] = &[
        ("ConfirmNonDefaultBranch", "Check Out & Add"),
        ("ConfirmNonDefaultBranch(heuristic)", "Check Out & Add"),
    ];

    /// Every Confirm-family dialog: the registry's own fixtures, so a new one is
    /// covered the moment it has a fixture, plus the heaviest real states of the
    /// dialogs whose bodies grow with their state.
    fn confirm_dialogs(app: &App) -> Vec<(&'static str, PromptState)> {
        let mut dialogs: Vec<(&'static str, PromptState)> = every_prompt(app)
            .into_iter()
            .filter(|(_, prompt)| {
                modal_spec(prompt).map(|spec| spec.family) == Some(ModalFamily::Confirm)
            })
            .collect();
        assert!(
            dialogs.len() >= 18,
            "the registry declares fewer Confirm dialogs than expected: {:?}",
            dialogs.iter().map(|(name, _)| *name).collect::<Vec<_>>()
        );
        let project = app.engine.projects[0].clone();
        dialogs.extend([
            (
                "ConfirmDeleteAgent(worktree, existing branch)",
                PromptState::ConfirmDeleteAgent {
                    delete_branch: false,
                    unpushed_commits: Some(dux_core::git::UnpushedCommits {
                        count: 3,
                        has_remote_refs: true,
                    }),
                    session_id: "s1".to_string(),
                    agent_label: "my cool agent".to_string(),
                    target: DeleteAgentTarget::Managed {
                        branch_name: "feature/drifted-name".to_string(),
                        initial_branch: "feature/original-name".to_string(),
                        branch_provenance: dux_core::model::BranchProvenance::AttachedExisting,
                        worktree_shared: false,
                    },
                    focus: DeleteAgentFocus::Cancel,
                    delete_worktree: true,
                },
            ),
            (
                "ConfirmDeleteAgent(shared worktree)",
                PromptState::ConfirmDeleteAgent {
                    delete_branch: false,
                    unpushed_commits: None,
                    session_id: "s1".to_string(),
                    agent_label: "my cool agent".to_string(),
                    target: DeleteAgentTarget::Managed {
                        branch_name: "b".to_string(),
                        initial_branch: "b".to_string(),
                        branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
                        worktree_shared: true,
                    },
                    focus: DeleteAgentFocus::Cancel,
                    delete_worktree: false,
                },
            ),
            (
                "ConfirmDeleteAgent(standalone)",
                PromptState::ConfirmDeleteAgent {
                    delete_branch: false,
                    unpushed_commits: None,
                    session_id: "s1".to_string(),
                    agent_label: "my cool agent".to_string(),
                    target: DeleteAgentTarget::Folder {
                        folder_label: "~/notes/my project".to_string(),
                    },
                    focus: DeleteAgentFocus::Cancel,
                    delete_worktree: false,
                },
            ),
            (
                "ConfirmDeleteWorktree(dirty)",
                PromptState::ConfirmDeleteWorktree(Box::new(ConfirmDeleteWorktreePrompt {
                    previous: ManageWorktreesPrompt {
                        project: project.clone(),
                        entries: Vec::new(),
                        loading: false,
                        selected: None,
                        error: None,
                    },
                    project: project.clone(),
                    path: std::path::PathBuf::from("/tmp/worktrees/demo/some-long-branch"),
                    label: "some-long-branch".to_string(),
                    branch: Some("some-long-branch".to_string()),
                    dirty: true,
                    delete_branch: true,
                    focus: DeleteWorktreeFocus::Cancel,
                })),
            ),
            (
                "ConfirmNonDefaultBranch(heuristic)",
                PromptState::ConfirmNonDefaultBranch {
                    add: PendingProjectAdd {
                        path: project.path.clone(),
                        name: project.name.clone(),
                    },
                    current_branch: "feature".to_string(),
                    kind: dux_core::worker::BranchWarningKind::Heuristic,
                    focus: ConfirmNonDefaultBranchFocus::Cancel,
                    checkout_default: false,
                },
            ),
            (
                "ConfirmInitRepo(gitignore)",
                PromptState::ConfirmInitRepo {
                    path: "/home/ada/projects/something".to_string(),
                    name: "something".to_string(),
                    candidates: vec![
                        "node_modules/".to_string(),
                        "target/".to_string(),
                        ".env".to_string(),
                    ],
                    focus: ConfirmFocus::Cancel,
                    return_prompt: Box::new(PromptState::None),
                },
            ),
            (
                "ConfirmKillRunning(mixed)",
                PromptState::ConfirmKillRunning(ConfirmKillRunningPrompt {
                    previous: KillRunningPrompt {
                        runtimes: Vec::new(),
                        list: SearchableList::new(),
                        selected_ids: std::collections::HashSet::new(),
                        focus: KillRunningFocus::List,
                    },
                    action: KillRunningAction::Visible,
                    target_ids: vec![
                        RuntimeTargetId::Agent("a".to_string()),
                        RuntimeTargetId::Terminal("t".to_string()),
                    ],
                    focus: ConfirmFocus::Cancel,
                }),
            ),
            (
                "ConfirmDeleteTerminal(foreground)",
                PromptState::ConfirmDeleteTerminal {
                    terminal_id: "t1".to_string(),
                    terminal_label: "My Cool Terminal".to_string(),
                    foreground_cmd: Some("vim".to_string()),
                    focus: ConfirmFocus::Cancel,
                },
            ),
        ]);
        dialogs
    }

    /// The row of the scroll marker in `modal`'s right border column, and the
    /// glyph, when the dialog scrolls.
    fn scroll_marker(buf: &Buffer, modal: Rect) -> Option<(u16, char)> {
        let column = modal.right() - 1;
        (modal.y + 1..modal.bottom() - 1).find_map(|y| {
            let glyph = buf[(column, y)].symbol().chars().next()?;
            matches!(glyph, '↓' | '↑' | '↕').then_some((y, glyph))
        })
    }

    /// Every word the open dialog says, read off the screen at `size`. A body
    /// that scrolls is read the way a user reads it: one row at a time with the
    /// dialog's own line-scroll key until the marker says there is nothing
    /// left below, collecting each row that scrolls into view.
    fn read_whole_dialog(app: &mut App, size: (u16, u16)) -> (Vec<String>, Buffer, Rect) {
        let first = rerender(app, size);
        let modal = app.overlay_layout.frame.get().expect("a modal was painted");
        let Some((marker_row, _)) = scroll_marker(&first, modal) else {
            return (modal_words(&first, modal), first, modal);
        };
        let inner = |buf: &Buffer, y: u16| row_text(buf, y, modal.x + 1, modal.right() - 1);
        let mut rows: Vec<String> = (modal.y + 1..=marker_row)
            .map(|y| inner(&first, y))
            .collect();
        let mut last = first;
        for _ in 0..500 {
            if scroll_marker(&last, modal).is_some_and(|(_, glyph)| glyph == '↑') {
                break;
            }
            app.handle_key(key(KeyCode::Down)).expect("scroll one row");
            last = rerender(app, size);
            rows.push(inner(&last, marker_row));
        }
        rows.extend((marker_row + 1..modal.bottom() - 1).map(|y| inner(&last, y)));
        let words = rows
            .iter()
            .flat_map(|row| row.split_whitespace().map(str::to_string))
            .collect();
        (words, last, modal)
    }

    /// Every Confirm dialog, painted on an 80x24 terminal, says every word it
    /// says on a roomy one, and shows both of its buttons whole. A body too
    /// tall for the screen may scroll, and then scrolling it must reach every
    /// word. A dialog sized by a percentage of the screen used to leave room for
    /// one row of prose at this size and drop the rest without a word.
    #[test]
    fn every_confirm_dialog_fits_an_80x24_terminal_uncut() {
        let mut app = test_app(default_bindings());
        let mut failures = Vec::new();
        for (name, prompt) in confirm_dialogs(&app) {
            let roomy = render_at(&mut app, prompt.clone(), ROOMY);
            let roomy_modal = app.overlay_layout.frame.get().expect("a modal was painted");
            let want = modal_words(&roomy, roomy_modal);

            app.prompt = prompt;
            let (got, small, modal) = read_whole_dialog(&mut app, SMALL);
            if got != want {
                failures.push(format!(
                    "{name}: at 80x24 it says\n  {got:?}\nbut on a roomy screen\n  {want:?}\n{}",
                    screen(&small)
                ));
                continue;
            }
            let Some(buttons) = confirm_buttons(&app.overlay_layout.active) else {
                failures.push(format!("{name}: published no Cancel/act button pair"));
                continue;
            };
            for button in buttons {
                let inside = button.x > modal.x
                    && button.right() < modal.right()
                    && button.y > modal.y
                    && button.bottom() < modal.bottom();
                if !inside || button.height != 3 {
                    failures.push(format!(
                        "{name}: button {button:?} is not whole inside the dialog {modal:?}\n{}",
                        screen(&small)
                    ));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    }

    /// Every Confirm dialog's two buttons obey one rule: the same width, the
    /// width the shared sizing gives its labels, the one gap between them, and
    /// centred as a pair. Checked on an 80x24 terminal and a roomy one.
    #[test]
    fn every_confirm_dialogs_buttons_share_one_width_and_gap_rule() {
        let mut app = test_app(default_bindings());
        let mut failures = Vec::new();
        for size in [SMALL, ROOMY] {
            for (name, prompt) in confirm_dialogs(&app) {
                let buf = render_at(&mut app, prompt, size);
                let modal = app.overlay_layout.frame.get().expect("a modal was painted");
                let Some([left, right]) = confirm_buttons(&app.overlay_layout.active) else {
                    failures.push(format!("{name}: published no Cancel/act button pair"));
                    continue;
                };
                let label = |rect: Rect| {
                    row_text(&buf, rect.y + 1, rect.x + 1, rect.right() - 1)
                        .trim()
                        .to_string()
                };
                let mut labels = vec![label(left), label(right)];
                labels.extend(
                    RESERVED_LABELS
                        .iter()
                        .filter(|(fixture, _)| *fixture == name)
                        .map(|(_, reserved)| reserved.to_string()),
                );
                let label_refs: Vec<&str> = labels.iter().map(String::as_str).collect();
                let want_width = shared_button_width(&label_refs);
                let gap = right.x.saturating_sub(left.right());
                let left_slack = left.x - (modal.x + 1);
                let right_slack = (modal.right() - 1) - right.right();
                if left.width != want_width
                    || right.width != want_width
                    || gap != BUTTON_GAP
                    || left.y != right.y
                    || right_slack.abs_diff(left_slack) > 1
                {
                    failures.push(format!(
                        "{name} at {size:?}: buttons {left:?} and {right:?} labelled {labels:?} \
                         want width {want_width} and gap {BUTTON_GAP}, centred in {modal:?}"
                    ));
                }
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// A Confirm dialog whose body cannot fit the terminal scrolls it the way
    /// the welcome and "What's new" screens scroll theirs: the rows that fit,
    /// a direction marker in the right border column, and the buttons still
    /// whole below. The dialog's scroll keys reach the rest.
    #[test]
    fn a_confirm_body_too_tall_for_the_screen_scrolls_with_the_shared_marker() {
        let mut app = test_app(default_bindings());
        let prompt = PromptState::ConfirmRecreateWorkingCopy {
            session_id: "s1".to_string(),
            worktree_path: std::path::PathBuf::from("/tmp/worktrees/repo/feat"),
            branch_name: "feat".to_string(),
            source_branch: "main".to_string(),
            conversation_resumes: false,
            running_providers: vec!["claude".to_string(), "codex".to_string()],
            focus: ConfirmFocus::Cancel,
        };
        let roomy = render_at(&mut app, prompt.clone(), ROOMY);
        let roomy_modal = app.overlay_layout.frame.get().expect("modal");
        let [roomy_cancel, _] = confirm_buttons(&app.overlay_layout.active).expect("buttons");
        // The body's last word: the last one above the buttons.
        let last_word = (roomy_modal.y + 1..roomy_cancel.y)
            .flat_map(|y| {
                row_text(&roomy, y, roomy_modal.x + 1, roomy_modal.right() - 1)
                    .split_whitespace()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .last()
            .expect("a body");

        let tiny = (60, 14);
        let top = render_at(&mut app, prompt, tiny);
        let modal = app.overlay_layout.frame.get().expect("modal");
        assert!(
            modal.height <= tiny.1,
            "the dialog is clamped to the screen"
        );
        let border_column: String = (modal.y..modal.bottom())
            .map(|y| top[(modal.right() - 1, y)].symbol().to_string())
            .collect();
        assert!(
            border_column.contains('↓'),
            "the more-below marker sits in the right border column:\n{}",
            screen(&top)
        );
        assert!(
            !modal_words(&top, modal).contains(&last_word),
            "the body's tail is below the fold at first:\n{}",
            screen(&top)
        );
        let buttons = confirm_buttons(&app.overlay_layout.active).expect("buttons");
        assert!(buttons.iter().all(|b| b.bottom() < modal.bottom()));

        app.handle_key(key(KeyCode::End))
            .expect("scroll to the end");
        let scrolled = app.prompt.clone();
        let bottom = render_at(&mut app, scrolled, tiny);
        let words = modal_words(&bottom, modal);
        assert!(
            words.contains(&last_word),
            "scrolling to the end reveals the last word {last_word:?}:\n{}",
            screen(&bottom)
        );
        let border_column: String = (modal.y..modal.bottom())
            .map(|y| bottom[(modal.right() - 1, y)].symbol().to_string())
            .collect();
        assert!(border_column.contains('↑'), "{}", screen(&bottom));
        assert!(
            words.contains(&"Cancel".to_string()) && words.contains(&"Recreate".to_string()),
            "the buttons stay on screen while the body scrolls:\n{}",
            screen(&bottom)
        );
        assert!(
            matches!(app.prompt, PromptState::ConfirmRecreateWorkingCopy { .. }),
            "a scroll key never acts on the dialog"
        );
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn click(app: &mut App, column: u16, row: u16) {
        app.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), column, row));
        app.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), column, row));
    }

    /// The wheel scrolls a Confirm body that does not fit, a notch at a time,
    /// and the marker turns once the tail is in view.
    #[test]
    fn the_wheel_scrolls_a_confirm_body_that_does_not_fit() {
        let mut app = test_app(default_bindings());
        let tiny = (60, 14);
        let prompt = PromptState::ConfirmRecreateWorkingCopy {
            session_id: "s1".to_string(),
            worktree_path: std::path::PathBuf::from("/tmp/worktrees/repo/feat"),
            branch_name: "feat".to_string(),
            source_branch: "main".to_string(),
            conversation_resumes: true,
            running_providers: vec!["claude".to_string()],
            focus: ConfirmFocus::Cancel,
        };
        let top = render_at(&mut app, prompt, tiny);
        let modal = app.overlay_layout.frame.get().expect("modal");
        let (marker_row, glyph) = scroll_marker(&top, modal).expect("the body scrolls");
        assert_eq!(glyph, '↓');
        let first_row = row_text(&top, modal.y + 1, modal.x + 1, modal.right() - 1);

        app.handle_mouse(mouse(MouseEventKind::ScrollDown, modal.x + 5, marker_row));
        let scrolled = rerender(&mut app, tiny);
        let fourth_row = row_text(&top, modal.y + 4, modal.x + 1, modal.right() - 1);
        assert_eq!(
            row_text(&scrolled, modal.y + 1, modal.x + 1, modal.right() - 1),
            fourth_row,
            "one notch scrolls three rows:\n{}",
            screen(&scrolled)
        );

        for _ in 0..50 {
            app.handle_mouse(mouse(MouseEventKind::ScrollDown, modal.x + 5, marker_row));
        }
        let bottom = rerender(&mut app, tiny);
        assert_eq!(scroll_marker(&bottom, modal).map(|(_, g)| g), Some('↑'));

        for _ in 0..50 {
            app.handle_mouse(mouse(MouseEventKind::ScrollUp, modal.x + 5, marker_row));
        }
        let back = rerender(&mut app, tiny);
        assert_eq!(
            row_text(&back, modal.y + 1, modal.x + 1, modal.right() - 1),
            first_row
        );
    }

    /// A Confirm dialog that fits keeps every key it had: the scroll keys are
    /// not taken from it, so the focus keys and the rest behave as before.
    #[test]
    fn a_confirm_dialog_that_fits_keeps_its_keys() {
        let mut app = test_app(default_bindings());
        let prompt = PromptState::ConfirmQuit {
            agent_count: 1,
            terminal_count: 0,
            focus: ConfirmFocus::Cancel,
        };
        let buf = render_at(&mut app, prompt, SMALL);
        let modal = app.overlay_layout.frame.get().expect("modal");
        assert_eq!(scroll_marker(&buf, modal), None, "{}", screen(&buf));
        assert!(!app.scroll_confirm_body_for(&key(KeyCode::Down)));
        app.handle_key(key(KeyCode::Right)).expect("move focus");
        assert!(matches!(
            app.prompt,
            PromptState::ConfirmQuit {
                focus: ConfirmFocus::Confirm,
                ..
            }
        ));
    }

    /// A scroll offset belongs to the dialog it was scrolled in: another dialog
    /// opens at the top, and so does the same one opened again after closing.
    #[test]
    fn a_scroll_offset_does_not_outlive_its_dialog() {
        let mut app = test_app(default_bindings());
        let tiny = (60, 14);
        let prompt = PromptState::ConfirmRecreateWorkingCopy {
            session_id: "s1".to_string(),
            worktree_path: std::path::PathBuf::from("/tmp/worktrees/repo/feat"),
            branch_name: "feat".to_string(),
            source_branch: "main".to_string(),
            conversation_resumes: true,
            running_providers: vec!["claude".to_string()],
            focus: ConfirmFocus::Cancel,
        };
        let top = render_at(&mut app, prompt.clone(), tiny);
        let modal = app.overlay_layout.frame.get().expect("modal");
        let first_row = row_text(&top, modal.y + 1, modal.x + 1, modal.right() - 1);
        app.handle_key(key(KeyCode::End)).expect("scroll");
        rerender(&mut app, tiny);
        render_at(&mut app, PromptState::None, tiny);
        let reopened = render_at(&mut app, prompt, tiny);
        assert_eq!(
            row_text(&reopened, modal.y + 1, modal.x + 1, modal.right() - 1),
            first_row
        );
    }

    /// Every Confirm dialog's buttons still take clicks where they are now
    /// painted: a click on the Cancel-side button closes the dialog, and a
    /// click on the gap between the two buttons does nothing at all.
    #[test]
    fn every_confirm_dialogs_moved_buttons_take_clicks() {
        let mut app = test_app(default_bindings());
        let mut failures = Vec::new();
        for (name, prompt) in confirm_dialogs(&app) {
            render_at(&mut app, prompt, SMALL);
            // As painted: a project confirmation recounts its agents at paint.
            let painted = format!("{:?}", app.prompt);
            let [cancel, act] = confirm_buttons(&app.overlay_layout.active).expect("buttons");

            let gap_column = cancel.right() + (act.x - cancel.right()) / 2;
            click(&mut app, gap_column, cancel.y + 1);
            if format!("{:?}", app.prompt) != painted {
                failures.push(format!("{name}: a click between the buttons changed it"));
                continue;
            }

            click(&mut app, cancel.x + cancel.width / 2, cancel.y + 1);
            if modal_spec(&app.prompt).map(|spec| spec.family) == Some(ModalFamily::Confirm) {
                failures.push(format!(
                    "{name}: a click on its Cancel-side button at {cancel:?} left it open"
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n"));
    }
}
