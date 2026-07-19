use super::components::{
    Button, ButtonKind, ButtonPressedTarget, Checkbox, CheckboxState, button_state_for,
    shared_button_width,
};
use super::*;
use crate::tui_color::{to_ratatui_color, to_ratatui_modifier};
use ratatui::buffer::{CellDiffOption, CellWidth};
use std::path::Path;

/// How an agent row's project tag should be rendered. Decided purely from the
/// project the session points at, so both the full-width row's inline tag and
/// the collapsed icon rail agree on when to surface a warning marker.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProjectTagKind {
    /// The project record exists and its path is present: render its name plainly.
    Healthy,
    /// The project record exists but its worktree path is missing on disk:
    /// render a warning marker so the row surfaces the broken project.
    PathMissing,
    /// The project record is gone (orphaned session): render a "removed project"
    /// warning marker.
    Orphan,
}

/// Decide the [`ProjectTagKind`] for a session from its resolved project
/// (`None` when the project record no longer exists). Pure and unit-tested so
/// the two render paths cannot drift.
pub(crate) fn project_tag_kind(project: Option<&Project>) -> ProjectTagKind {
    match project {
        None => ProjectTagKind::Orphan,
        Some(project) if project.path_missing => ProjectTagKind::PathMissing,
        Some(_) => ProjectTagKind::Healthy,
    }
}

/// The colored "state word" shown on an agent row's second line, the honest,
/// field-backed stand-in for an activity string (dux has no such field). It reads
/// off the same flags that drive the working spinner and the attention pulse, so
/// the word can never disagree with the motion cue. Mirrors the web's `stateWord`
/// (`crates/dux-web/web/src/lib/flatList.ts`) exactly. Pure and unit-tested.
pub(crate) fn agent_state_word(
    status: crate::model::SessionStatus,
    working: bool,
    needs_attention: bool,
) -> &'static str {
    use crate::model::SessionStatus;
    if needs_attention {
        return "Needs you";
    }
    match status {
        SessionStatus::Active if working => "Working",
        SessionStatus::Active => "Idle",
        SessionStatus::Detached => "Detached",
        SessionStatus::Exited => "Exited",
    }
}

/// Build a screen-row -> item-index map for a `List` of variable-height items,
/// so mouse hit-testing survives the flat list's two-line agent rows (and any
/// scroll). `offset` is the `ListState` item offset AFTER rendering, `heights`
/// the rendered height of every item (in lines), and `area_height` the visible
/// list height. The returned vector has one entry per visible screen row (from
/// the list's top), each holding the item index that occupies that row. Pure and
/// unit-tested; ratatui scrolls whole items, so the top item is never clipped.
pub(crate) fn left_row_to_item(offset: usize, heights: &[u16], area_height: u16) -> Vec<usize> {
    let cap = area_height as usize;
    let mut map = Vec::with_capacity(cap);
    let mut item = offset;
    while map.len() < cap && item < heights.len() {
        for _ in 0..heights[item].max(1) {
            if map.len() >= cap {
                break;
            }
            map.push(item);
        }
        item += 1;
    }
    map
}

/// ASCII art logo displayed in the agent pane when no content is active.
/// Shared with the server status screen (`crate::server_screen`) so the flip's
/// "server running" view reuses the same wordmark instead of duplicating it.
pub(crate) const ASCII_LOGO: &[&str] = &[
    "       ░██                       ",
    "       ░██                       ",
    " ░████████ ░██    ░██ ░██    ░██ ",
    "░██    ░██ ░██    ░██  ░██  ░██  ",
    "░██    ░██ ░██    ░██   ░█████   ",
    "░██   ░███ ░██   ░███  ░██  ░██  ",
    " ░█████░██  ░█████░██ ░██    ░██ ",
];
/// Display width of each line in `ASCII_LOGO` (all lines are equal width).
pub(crate) const ASCII_LOGO_WIDTH: u16 = 33;
/// Number of lines in `ASCII_LOGO`.
const ASCII_LOGO_HEIGHT: u16 = 7;

/// Alternate braille-art duck logo, same width as the text logo.
const ASCII_LOGO_ALT: &[&str] = &[
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⣀⣤⠤⣄⣀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⠔⠉⠀⠀⢸⢰⢸⢰⢰⠉⠢⡀⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠀⡔⢰⠀⢸⢰⢸⢸⢸⢸⢸⢸⢸⢸⢈⢦⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⣰⢠⢸⠤⣤⠤⢸⢸⢸⢸⢸⠤⠤⢐⢸⢸⡆⠀⠀⠀⠀⠀⠀⠀",
    "⢠⠀⠀⠀⠀⠀⠀⠀⠀⢹⢸⢸⢸⢸⢸⣤⢰⢲⢤⣄⢸⢸⢸⢸⢸⡇⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠙⡄⠀⠀⠀⠀⠀⠀⠀⣄⢸⢰⣶⣿⣤⣤⣶⣤⣤⣬⣷⡤⢸⣰⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠈⢦⠀⠀⠀⠀⠀⠀⠈⢦⢸⢈⠛⠿⣿⣼⣼⠿⠛⠁⢸⡼⠀⠀⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠸⠉⢻⣍⠉⠤⣀⣠⣤⣾⢸⣿⣿⣿⣶⣾⣾⣿⣿⢸⢸⠓⠤⣄⠀⠀⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠈⠒⠭⣀⢸⢸⢈⢈⢸⢸⢸⠙⠻⢸⢸⢸⠿⠛⢸⢸⢸⢸⢸⢈⠑⢄⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⣼⢸⢸⢈⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⣠⠤⠒⠁⢸⣦⠀⠀⠀",
    "⠀⠀⠀⠀⠀⣿⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢩⡂⢸⢸⣀⣴⣿⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠹⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢉⠉⠉⠁⢸⠃⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⢳⢨⠘⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⢸⣴⢸⢸⢸⢸⡟⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠳⣀⢠⢨⢘⢘⢸⢸⢸⢸⢸⢸⢸⢸⣤⣿⢸⢸⢸⣀⠛⠀⠀⠀⠀⠀",
    "⠀⠀⠀⠀⠀⠀⠀⠀⠀⠈⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠉⠀⠀⠀⠀⠀⠀⠀⠀",
];
/// Display width of each line in `ASCII_LOGO_ALT`.
const ASCII_LOGO_ALT_WIDTH: u16 = 33;
/// Number of lines in `ASCII_LOGO_ALT`.
const ASCII_LOGO_ALT_HEIGHT: u16 = 15;

/// Maximum display width for a tip line (logo width + padding on each side).
const TIP_MAX_WIDTH: u16 = 47;
/// Blank lines between the bottom of the logo and the tip.
const TIP_GAP: u16 = 2;
/// Maximum number of wrapped lines a tip may occupy.
const TIP_MAX_LINES: u16 = 3;

/// Capitalize the first character of a string.
fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(first) => format!("{}{}", first.to_uppercase(), c.as_str()),
        None => String::new(),
    }
}

/// A one-column pad on each side of the left-pane row text: the left gutter
/// also hosts the selection frame, and the right gets an equal pad so the text
/// (and the Inactive rule) sit evenly between two matching margins.
const LEFT_PANE_GUTTER: u16 = 1;

/// Badge glyph marking a pull request on the agent row, standing in for the
/// letters "PR" to save a column. U+2387 (ALTERNATIVE KEY SYMBOL) renders as a
/// branch fork in most terminals and is width-1; the `#<number>` follows it.
const PR_BADGE_GLYPH: &str = "⎇";

/// Truncate `s` to at most `max_w` display columns, measured by real
/// terminal cell width (unicode-width via `CellWidth`), not byte or char
/// count. Stops before any character that would push the running width over
/// `max_w`, so multi-byte/double-width glyphs (CJK, emoji) are never split
/// mid-character and the result never overflows its budget.
fn truncate_to_width(s: &str, max_w: u16) -> String {
    if s.cell_width() <= max_w {
        return s.to_string();
    }
    let mut out = String::new();
    let mut w = 0u16;
    for ch in s.chars() {
        let mut buf = [0u8; 4];
        let cw = ch.encode_utf8(&mut buf).cell_width();
        if w + cw > max_w {
            break;
        }
        out.push(ch);
        w += cw;
    }
    out
}

/// Truncate a line of styled spans to `max_w` terminal cells, appending a
/// single-cell ellipsis (`…`) when any content is dropped. Width is measured in
/// display columns (unicode-width via `CellWidth`), so CJK/emoji count as two.
/// Each surviving span keeps its own style, and the ellipsis inherits the style
/// of the span it cut into so it matches the color of the text it replaced.
/// Returns the spans unchanged when they already fit.
fn ellipsize_spans(spans: Vec<Span<'static>>, max_w: u16) -> Vec<Span<'static>> {
    let total = spans
        .iter()
        .map(|s| s.content.as_ref().cell_width())
        .fold(0u16, |a, b| a.saturating_add(b));
    if total <= max_w {
        return spans;
    }
    if max_w == 0 {
        return Vec::new();
    }
    let budget = max_w - 1; // reserve one cell for the ellipsis
    let mut out: Vec<Span<'static>> = Vec::new();
    let mut used = 0u16;
    let mut ellipsis_style = Style::default();
    for span in spans {
        ellipsis_style = span.style;
        let w = span.content.as_ref().cell_width();
        if used + w <= budget {
            used += w;
            out.push(span);
        } else {
            let remaining = budget - used;
            if remaining > 0 {
                let head = truncate_to_width(span.content.as_ref(), remaining);
                if !head.is_empty() {
                    out.push(Span::styled(head, span.style));
                }
            }
            break;
        }
    }
    out.push(Span::styled("…", ellipsis_style));
    out
}

/// Lay a line out with `left` packed to the left and `right` flush to the right
/// edge of `total_w`, separated by at least `min_gap` blank cells. The left
/// group is ellipsized to whatever space remains after the right group and the
/// gap, so a long name yields `long-na…   PR#12` — the badge stays pinned to the
/// right and the name loses characters instead. Falls back to a single
/// ellipsized run when there is not even room for the right group plus the gap.
fn right_align_line(
    left: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    total_w: u16,
    min_gap: u16,
) -> Vec<Span<'static>> {
    let width = |spans: &[Span<'static>]| {
        spans
            .iter()
            .map(|s| s.content.as_ref().cell_width())
            .fold(0u16, |a, b| a.saturating_add(b))
    };
    let right_w = width(&right);
    // Not enough room to reserve the right group plus a gap: degrade to a single
    // ellipsized run of the whole line so nothing overflows the pane.
    if total_w <= right_w + min_gap {
        let mut all = left;
        all.extend(right);
        return ellipsize_spans(all, total_w);
    }
    let left_budget = total_w - right_w - min_gap;
    let mut out = ellipsize_spans(left, left_budget);
    // Pad the gap so the right group lands flush against the right edge. `pad` is
    // at least `min_gap` because the left group was capped at `left_budget`.
    let pad = total_w.saturating_sub(width(&out) + right_w);
    if pad > 0 {
        out.push(Span::raw(" ".repeat(pad as usize)));
    }
    out.extend(right);
    out
}

/// Column budget for the resource monitor table, computed from the inner
/// content width so every column stays readable instead of colliding on a
/// hardcoded per-column `Constraint`.
///
/// The old layout hardcoded `[Constraint::Min(30), Length(8), Length(6),
/// Length(8), Length(12)]` for `[Name, PID, Procs, CPU %, RSS]`, on the
/// assumption that `Min` is a soft lower bound and `Length` is rigid, so a
/// narrow terminal would squeeze the flexible `Min(30)` Name column down to
/// a couple of characters. Instrumenting the actual render with a
/// `TestBackend` (see the `resource_monitor_*_legible_at_narrow_width`
/// tests) showed the opposite: ratatui's `Table` constraint solver treats a
/// `Min` column as free to grow and greedily claims the available width,
/// while `Length` columns compress far below their stated size once the
/// total no longer fits. At a 50-column terminal the old layout kept a
/// generous ~31-character Name column while PID/Procs/CPU/RSS were crushed
/// to unreadable slivers (the header rendered as `"PI P CP R"`, a 5.0% CPU
/// reading as `"5."`, a 1.0 MiB RSS reading as `"1"`) - exactly backwards
/// from this monitor's purpose, since CPU and RSS are the numbers the user
/// opened the popup to read.
///
/// Fix: stop asking the constraint solver to guess and compute an exact
/// column plan from the real inner width instead. CPU and RSS always keep
/// their full width; they are never dropped or shrunk. PID is the least
/// actionable column at a glance (a raw number) so it is dropped first when
/// space runs out, then Procs. Name always keeps at least
/// `RESOURCE_MONITOR_NAME_MIN_WIDTH` columns once PID/Procs are gone; if a
/// name is still too long for that width, the caller truncates it with an
/// ellipsis (`truncate_status_text`, character-based) rather than letting
/// the table hard-chop it mid-character.
struct ResourceMonitorColumns {
    show_pid: bool,
    show_procs: bool,
    name_w: u16,
    pid_w: u16,
    procs_w: u16,
    cpu_w: u16,
    rss_w: u16,
}

impl ResourceMonitorColumns {
    /// Number of visible columns, used to compute inter-column spacing.
    fn visible_count(&self) -> u16 {
        3 + u16::from(self.show_pid) + u16::from(self.show_procs) // Name + CPU + RSS + optionals
    }
}

const RESOURCE_MONITOR_PID_W: u16 = 8;
const RESOURCE_MONITOR_PROCS_W: u16 = 6;
const RESOURCE_MONITOR_CPU_W: u16 = 8;
const RESOURCE_MONITOR_RSS_W: u16 = 12;
/// Floor below which the Name column is no longer considered readable and
/// we drop another column instead of shrinking it further.
const RESOURCE_MONITOR_NAME_MIN_WIDTH: u16 = 16;
/// ratatui's `Table` default `column_spacing` between adjacent columns.
const RESOURCE_MONITOR_COLUMN_SPACING: u16 = 1;

fn resource_monitor_columns(inner_width: u16) -> ResourceMonitorColumns {
    let try_plan = |show_pid: bool, show_procs: bool| -> Option<ResourceMonitorColumns> {
        let mut fixed = RESOURCE_MONITOR_CPU_W + RESOURCE_MONITOR_RSS_W;
        let mut visible_cols = 3u16; // Name, CPU %, RSS
        if show_pid {
            fixed += RESOURCE_MONITOR_PID_W;
            visible_cols += 1;
        }
        if show_procs {
            fixed += RESOURCE_MONITOR_PROCS_W;
            visible_cols += 1;
        }
        let gaps = RESOURCE_MONITOR_COLUMN_SPACING * visible_cols.saturating_sub(1);
        let used = fixed + gaps;
        let name_w = inner_width.checked_sub(used)?;
        // Only enforce the readability floor when dropping a column is still
        // an option; the last-resort plan (no PID, no Procs) takes whatever
        // is left, even below the floor, rather than rendering nothing.
        if name_w < RESOURCE_MONITOR_NAME_MIN_WIDTH && (show_pid || show_procs) {
            return None;
        }
        Some(ResourceMonitorColumns {
            show_pid,
            show_procs,
            name_w: name_w.max(1),
            pid_w: RESOURCE_MONITOR_PID_W,
            procs_w: RESOURCE_MONITOR_PROCS_W,
            cpu_w: RESOURCE_MONITOR_CPU_W,
            rss_w: RESOURCE_MONITOR_RSS_W,
        })
    };

    try_plan(true, true)
        .or_else(|| try_plan(false, true))
        .or_else(|| try_plan(false, false))
        .unwrap_or(ResourceMonitorColumns {
            show_pid: false,
            show_procs: false,
            name_w: inner_width
                .saturating_sub(RESOURCE_MONITOR_CPU_W + RESOURCE_MONITOR_RSS_W + 2)
                .max(1),
            pid_w: RESOURCE_MONITOR_PID_W,
            procs_w: RESOURCE_MONITOR_PROCS_W,
            cpu_w: RESOURCE_MONITOR_CPU_W,
            rss_w: RESOURCE_MONITOR_RSS_W,
        })
}

fn wrapped_line_count(lines: &[Line<'_>], width: u16, trim: bool) -> u16 {
    if width == 0 {
        return 0;
    }

    let max_width = usize::from(width);
    let mut total = 0u16;
    for line in lines {
        let mut current_width = 0usize;
        let mut line_count = 1u16;
        for span in &line.spans {
            let content = if trim {
                span.content.trim_end_matches(' ')
            } else {
                span.content.as_ref()
            };
            for segment in content.split('\n') {
                let segment_width = segment.chars().count();
                if segment_width == 0 {
                    continue;
                }

                let remaining = if current_width == 0 {
                    max_width
                } else {
                    max_width.saturating_sub(current_width)
                };
                if segment_width <= remaining {
                    current_width += segment_width;
                } else {
                    let needed = if current_width == 0 {
                        segment_width
                    } else {
                        segment_width - remaining
                    };
                    line_count = line_count.saturating_add(((needed - 1) / max_width) as u16 + 1);
                    current_width = needed % max_width;
                    if current_width == 0 {
                        current_width = max_width;
                    }
                }
            }
        }
        total = total.saturating_add(line_count);
    }
    total
}

fn macro_edit_text_inner_area(popup: Rect) -> Rect {
    let outer_inner = Block::bordered().inner(popup);
    let [_, bordered_area, _] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .areas(outer_inner);

    Block::bordered().inner(bordered_area)
}

fn sync_macro_text_input_layout(input: &mut TextInput, popup: Rect) {
    let text_inner = macro_edit_text_inner_area(popup);
    let wrap_w = text_inner.width.saturating_sub(1) as usize;
    input.set_display_width(if wrap_w > 0 { Some(wrap_w) } else { None });
    input.set_visible_lines(text_inner.height as usize);
    input.ensure_cursor_visible();
}

impl App {
    fn render_overlay_checkbox(
        &self,
        frame: &mut Frame,
        area: Rect,
        label: &str,
        checked: bool,
        state: CheckboxState,
        hint: Option<Line<'static>>,
    ) -> (Rect, u16) {
        let checkbox = Checkbox::new(label).checked(checked).state(state);
        let marker_style = checkbox.marker_style(match state {
            CheckboxState::Focused => Style::default().fg(self.theme.button_active_fg),
            CheckboxState::Hovered => Style::default().fg(self.theme.button_active_fg),
            CheckboxState::Disabled => Style::default().fg(self.theme.hint_desc_fg),
            CheckboxState::Normal => Style::default().fg(self.theme.hint_key_fg),
        });
        let label_style = checkbox.label_style(match state {
            CheckboxState::Focused => Style::default().fg(self.theme.button_active_fg),
            CheckboxState::Hovered => Style::default().fg(self.theme.button_active_fg),
            CheckboxState::Disabled => Style::default().fg(self.theme.hint_desc_fg),
            CheckboxState::Normal => Style::default().fg(self.theme.input_label_fg),
        });
        let layout = checkbox
            .layout(area.width, marker_style, label_style)
            .background(self.theme.overlay_bg);
        let checkbox_height = layout.height;
        let hint_height = u16::from(hint.is_some());
        let total_height = checkbox_height.saturating_add(hint_height);

        layout.render(
            Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: checkbox_height,
            },
            frame.buffer_mut(),
        );

        if let Some(hint_line) = hint {
            Paragraph::new(hint_line).render(
                Rect {
                    x: area.x,
                    y: area.y.saturating_add(checkbox_height),
                    width: area.width,
                    height: 1,
                },
                frame.buffer_mut(),
            );
        }

        (
            Rect {
                x: area.x,
                y: area.y,
                width: area.width,
                height: total_height,
            },
            total_height,
        )
    }

    pub(crate) fn render(&mut self, frame: &mut Frame) {
        // Pre-fill the whole frame with the theme's app background. Cells
        // that no widget paints over (gutters, modal interiors, the strip
        // under the PR banner caps) inherit this color, so light themes
        // actually look light end-to-end. Widgets that explicitly set
        // `Color::Reset` override this and still pass through to the
        // user's terminal default — that path is preserved for PTY cells
        // that emit a "reset background" SGR, so the embedded agent
        // terminal keeps rendering the CLI's own colors unchanged.
        let frame_area = frame.area();
        frame
            .buffer_mut()
            .set_style(frame_area, Style::default().bg(self.theme.app_bg));

        let status_tui = self.status.most_recent_tui();
        let status_text = status_tui
            .as_ref()
            .map_or(String::new(), |(_, t)| t.clone());
        let status_lines = status_footer_lines(&status_text, frame_area.width);
        let footer_h = 1 + status_lines; // 1 for hints + status lines
        let [header, body, footer] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(4),
                Constraint::Length(footer_h),
            ])
            .areas(frame.area());
        self.render_header(frame, header);
        let right_constraint = if self.right_hidden {
            Constraint::Length(0)
        } else if self.right_collapsed {
            Constraint::Length(3)
        } else {
            Constraint::Percentage(self.right_width_pct)
        };

        let [left, center, right] = if self.left_collapsed {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(4), Constraint::Min(20), right_constraint])
                .areas(body)
        } else {
            let right_pct = if self.right_hidden || self.right_collapsed {
                0
            } else {
                self.right_width_pct
            };
            let center_pct = 100u16
                .saturating_sub(self.left_width_pct + right_pct)
                .max(20);
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(self.left_width_pct),
                    Constraint::Percentage(center_pct),
                    right_constraint,
                ])
                .areas(body)
        };
        self.mouse_layout.reset(body, left, center, right);
        self.overlay_layout.reset();
        self.render_left(frame, left);
        self.render_center(frame, center);
        self.render_files(frame, right);
        self.render_footer(frame, footer);
        self.render_overlay(frame);
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let bg = self.theme.header_bg;
        let sep_fg = self.theme.header_separator_fg;
        let label_fg = self.theme.header_label_fg;
        let mut spans = vec![
            Span::styled(" dux ", Style::default().fg(label_fg).bg(bg)),
            Span::styled(
                env!("DUX_DISPLAY_VERSION"),
                Style::default().fg(self.theme.branch_fg).bg(bg),
            ),
        ];
        if let Some(project) = self.selected_project() {
            spans.push(Span::styled(" ╱ ", Style::default().fg(sep_fg).bg(bg)));
            spans.push(Span::styled(
                "project: ",
                Style::default().fg(label_fg).bg(bg),
            ));
            spans.push(Span::styled(
                project.name.clone(),
                Style::default().fg(self.theme.branch_fg).bg(bg),
            ));
            spans.push(Span::styled(" ╱ ", Style::default().fg(sep_fg).bg(bg)));
            spans.push(Span::styled(
                "branch: ",
                Style::default().fg(label_fg).bg(bg),
            ));
            spans.push(Span::styled(
                project.current_branch.clone(),
                Style::default().fg(self.theme.branch_fg).bg(bg),
            ));
            if let Some(session) = self.selected_session() {
                // Two independent reasons to show the agent crumb:
                //  - the agent sits on a different branch than the project's
                //    current branch, or
                //  - the agent's branch has DRIFTED from the branch it was
                //    created on (`initial_branch`).
                // The drift note must surface even when the agent happens to be on
                // the project's current branch, so it is gated on the drift alone —
                // not nested inside the project-current-branch comparison.
                let drifted = branch_drifted(&session.branch_name, &session.initial_branch);
                let differs_from_project = session.branch_name != project.current_branch;
                if differs_from_project || drifted {
                    // The helper appends "(orig: <initial>)" only on drift and
                    // returns the bare value (no label); we keep the themed
                    // "agent: " label and style the value ourselves.
                    let value =
                        top_bar_branch_suffix(&session.branch_name, &session.initial_branch);
                    spans.push(Span::styled(" ╱ ", Style::default().fg(sep_fg).bg(bg)));
                    spans.push(Span::styled(
                        "agent: ",
                        Style::default().fg(label_fg).bg(bg),
                    ));
                    spans.push(Span::styled(
                        value,
                        Style::default().fg(self.theme.branch_fg).bg(bg),
                    ));
                }
            }
            spans.push(Span::styled(" ╱ ", Style::default().fg(sep_fg).bg(bg)));
            let has_project_override = self
                .engine
                .project_uses_explicit_default_provider(&project.id);
            let provider_label = if has_project_override {
                "project provider: "
            } else {
                "default provider: "
            };
            spans.push(Span::styled(
                provider_label,
                Style::default().fg(label_fg).bg(bg),
            ));
            spans.push(Span::styled(
                project.default_provider.as_str().to_string(),
                Style::default().fg(self.theme.branch_fg).bg(bg),
            ));
            if has_project_override {
                spans.push(Span::styled(" ╱ ", Style::default().fg(sep_fg).bg(bg)));
                spans.push(Span::styled(
                    "global default: ",
                    Style::default().fg(label_fg).bg(bg),
                ));
                spans.push(Span::styled(
                    self.engine.config.default_provider().as_str().to_string(),
                    Style::default().fg(self.theme.branch_fg).bg(bg),
                ));
            }
            if let Some(session) = self.selected_session()
                && session.provider != project.default_provider
            {
                spans.push(Span::styled(" ╱ ", Style::default().fg(sep_fg).bg(bg)));
                spans.push(Span::styled(
                    "current provider: ",
                    Style::default().fg(label_fg).bg(bg),
                ));
                spans.push(Span::styled(
                    session.provider.as_str().to_string(),
                    Style::default().fg(self.theme.branch_fg).bg(bg),
                ));
            }
            let running_terminals = self.running_companion_terminal_count();
            if running_terminals > 0 {
                spans.push(Span::styled(" ╱ ", Style::default().fg(sep_fg).bg(bg)));
                let label = if running_terminals == 1 {
                    "● 1 terminal".to_string()
                } else {
                    format!("● {running_terminals} terminals")
                };
                spans.push(Span::styled(
                    label,
                    Style::default().fg(self.theme.session_active).bg(bg),
                ));
            }
        }
        Paragraph::new(Line::from(spans))
            .style(self.theme.header_style())
            .render(area, frame.buffer_mut());
    }

    /// Build the two-line flat agent row, mirroring the web sidebar row:
    /// line one is the status glyph + name + PR badge; line two is the dim
    /// `project · state word · branch (when it diverges from the name) · tabs`.
    /// The working spinner and attention pulse stay on the line-one glyph.
    fn render_agent_row(&self, session: &AgentSession, text_width: u16) -> ListItem<'static> {
        let label = session
            .title
            .clone()
            .unwrap_or_else(|| session.branch_name.clone());
        // Attention wins over the working spinner (a flagged agent may still be
        // streaming its permission prompt). Both cues are rolled up across the
        // agent's tabs. The attention glyph blinks on wall-clock time.
        let needs_attention = self.engine.config.ui.attention_indicator
            && self.engine.session_needs_attention(&session.id);
        let working = matches!(session.status, crate::model::SessionStatus::Active)
            && self.engine.session_is_streaming(&session.id);
        let (steady_dot, steady_color) = self.theme.session_dot(&session.status);
        let (dot, dot_color) = if needs_attention {
            let glyph = if self.attention_blink_on() {
                crate::theme::ATTENTION_GLYPH
            } else {
                " "
            };
            (glyph.to_string(), self.theme.session_attention)
        } else if working {
            let idx = self.spinner_frame_index();
            (
                crate::theme::SPINNER_FRAMES[idx].to_string(),
                self.theme.session_active,
            )
        } else {
            (steady_dot.to_string(), steady_color)
        };
        // A background delete dims and italicizes the whole row.
        let deleting = self.engine.pending_deletions.contains(&session.id);
        // The name takes the status/attention color, not the PR state; the PR
        // state rides its own badge (line one), matching the web row. While
        // blinking for attention, the glyph pulses the accent while the name
        // stays steady, so it falls back to the plain status-dot color.
        let name_color = if deleting {
            self.theme.session_deleting
        } else if needs_attention {
            steady_color
        } else {
            dot_color
        };
        let name_style = if deleting {
            Style::default()
                .fg(name_color)
                .add_modifier(Modifier::ITALIC)
        } else {
            Style::default().fg(name_color)
        };
        let glyph_style = if needs_attention && !deleting {
            Style::default().fg(dot_color)
        } else {
            name_style
        };

        // Line one: glyph + name packed left, and (if present) the PR badge
        // pinned to the right edge with the name ellipsized to fit.
        let line1_left = vec![
            Span::styled(format!("{dot} "), glyph_style),
            Span::styled(label.clone(), name_style),
        ];
        let pr_badge = self.engine.pr_statuses.get(&session.id).map(|pr| {
            let pr_color = match pr.state {
                crate::model::PrState::Merged => self.theme.pr_merged_label,
                crate::model::PrState::Closed => self.theme.pr_closed_label,
                crate::model::PrState::Open => self.theme.pr_open_label,
            };
            Span::styled(
                format!("{PR_BADGE_GLYPH}#{}", pr.number),
                Style::default().fg(pr_color),
            )
        });

        // Line two: project · state word [· branch] [· tabs]. Dim throughout;
        // only the state word carries a color.
        let muted = if deleting {
            self.theme.session_deleting
        } else {
            self.theme.provider_label_fg
        };
        let found = self
            .engine
            .projects
            .iter()
            .find(|p| p.id == session.project_id);
        // The project is marked with `※` (a folder stand-in) rather than a word,
        // and the marker sits directly under the agent name (two-space indent, so
        // the glyph column lines up with the name on line one).
        let project_span = match project_tag_kind(found) {
            ProjectTagKind::Healthy => Span::styled(
                format!("※ {}", found.map(|p| p.name.as_str()).unwrap_or("")),
                Style::default().fg(muted),
            ),
            ProjectTagKind::PathMissing => Span::styled(
                format!("⚠ {}", found.map(|p| p.name.as_str()).unwrap_or("")),
                Style::default().fg(self.theme.project_missing_fg),
            ),
            ProjectTagKind::Orphan => Span::styled(
                "⚠ removed project".to_string(),
                Style::default().fg(self.theme.project_missing_fg),
            ),
        };
        let word = agent_state_word(session.status, working, needs_attention);
        let word_color = if deleting {
            self.theme.session_deleting
        } else {
            match word {
                "Needs you" => self.theme.session_attention,
                "Working" => self.theme.session_active,
                "Detached" => steady_color,
                _ => self.theme.provider_label_fg,
            }
        };
        let sep = || Span::styled(" · ", Style::default().fg(muted));
        let mut line2 = vec![
            Span::raw("  "),
            project_span,
            sep(),
            Span::styled(word.to_string(), Style::default().fg(word_color)),
        ];
        // Show the branch only when it differs from the displayed name (i.e. a
        // title is set), so it is not repeated as the name on line one.
        let branch_diverges = session
            .title
            .as_deref()
            .is_some_and(|t| t != session.branch_name);
        if branch_diverges {
            line2.push(sep());
            line2.push(Span::styled(
                session.branch_name.clone(),
                Style::default().fg(muted),
            ));
        }
        let tab_count = self.session_tab_ids(&session.id).len();
        if tab_count > 1 {
            line2.push(sep());
            line2.push(Span::styled(
                format!("{tab_count} tabs"),
                Style::default().fg(muted),
            ));
        }

        // Line one: right-align the PR badge (name ellipsized to the space left
        // over) when there is one, else just ellipsize the name. Line two always
        // ellipsizes so a long project·state·branch·tabs run ends in `…` rather
        // than being hard-clipped mid-word at the right edge.
        let line1 = match pr_badge {
            Some(badge) => right_align_line(line1_left, vec![badge], text_width, 2),
            None => ellipsize_spans(line1_left, text_width),
        };
        let line2 = ellipsize_spans(line2, text_width);

        // A trailing blank line gives each agent breathing room: unselected rows
        // read as separated, and the selection highlight (which covers the whole
        // item) gains a half-step of padding below the text instead of butting
        // right up against the next row.
        ListItem::new(vec![Line::from(line1), Line::from(line2), Line::from("")])
    }

    /// Paint the left-pane selection by hand (the List widget renders with no
    /// highlight): a half-cell frame in the theme's faint selection tint — a `▄`
    /// top edge and a `▀` bottom edge on the boundary rows above/below the agent,
    /// and the same tint filling the two content rows edge to edge (both gutters
    /// included). The tint only sets the background, so the row keeps its own text
    /// colors (state word, PR badge, project) rather than being flattened by a
    /// full-flood highlight. The Inactive toggle marks only its label row (tint,
    /// no frame). `left_row_to_item` (already rebuilt for this frame) maps screen
    /// rows to items; `top_pad_y` is the reserved top-margin row for the very
    /// first agent's top edge.
    fn paint_left_selection(
        &self,
        buf: &mut ratatui::buffer::Buffer,
        list_inner: Rect,
        top_pad_y: Option<u16>,
    ) {
        let map = &self.mouse_layout.left_row_to_item;
        let sel = self.selected_left;
        let Some(rel_start) = map.iter().position(|&i| i == sel) else {
            return;
        };
        let rel_end = map.iter().rposition(|&i| i == sel).unwrap_or(rel_start);
        let items = self.left_items();
        let Some(item) = items.get(sel).copied() else {
            return;
        };
        let tint = self.theme.selection_bar_tint();
        let x0 = list_inner.x;
        let x1 = list_inner.x + list_inner.width;
        let row_at = |rel: usize| list_inner.y + rel as u16;

        // A content row: a faint tint across the full width, gutters included
        // (background only, so the row's own text colors survive on top).
        let paint_content = |buf: &mut ratatui::buffer::Buffer, rel: usize| {
            if map.get(rel) != Some(&sel) {
                return;
            }
            let y = row_at(rel);
            for x in x0..x1 {
                buf[(x, y)].set_bg(tint);
            }
        };
        // A half-cell frame edge across the full width: `▄` paints the bottom half
        // (a top edge that sits below the gap above), `▀` the top half (a bottom
        // edge that sits above the gap below). The painted half is the faint tint,
        // not the accent, so the edge reads as the selection background extended a
        // half-cell rather than a bright line a different color from the fill.
        let paint_edge = |buf: &mut ratatui::buffer::Buffer, y: u16, glyph: &str| {
            for x in x0..x1 {
                buf[(x, y)].set_symbol(glyph).set_fg(tint);
            }
        };

        match item {
            LeftItem::Session(_) => {
                // Top edge: the previous item's blank spacer, or the reserved
                // top-margin for the very first agent.
                if rel_start > 0 {
                    paint_edge(buf, row_at(rel_start - 1), "▄");
                } else if let Some(py) = top_pad_y {
                    paint_edge(buf, py, "▄");
                }
                paint_content(buf, rel_start);
                paint_content(buf, rel_start + 1);
                // Bottom edge: the agent's own trailing spacer.
                if map.get(rel_start + 2) == Some(&sel) {
                    paint_edge(buf, row_at(rel_start + 2), "▀");
                }
            }
            LeftItem::InactiveToggle => {
                // Only the label row (the toggle ends with a trailing spacer, so
                // the label is the second-to-last row); no frame edges.
                paint_content(buf, rel_end.saturating_sub(1));
            }
        }
    }

    /// Draw a dim rule from the end of the "Inactive (N)" label to the right
    /// gutter of the pane (stopping a column short of the edge, matching the row
    /// text padding), but only while the toggle is not the current selection (a
    /// selected toggle takes the full-width highlight instead).
    fn paint_inactive_rule(&self, buf: &mut ratatui::buffer::Buffer, list_content: Rect) {
        let items = self.left_items();
        let Some(toggle_idx) = items
            .iter()
            .position(|i| matches!(i, LeftItem::InactiveToggle))
        else {
            return;
        };
        if self.left_section == LeftSection::Projects && self.selected_left == toggle_idx {
            return;
        }
        let map = &self.mouse_layout.left_row_to_item;
        let Some(rel_end) = map.iter().rposition(|&i| i == toggle_idx) else {
            return;
        };
        // The toggle ends with a trailing spacer, so the label sits one row above
        // the item's last row.
        let y = list_content.y + rel_end.saturating_sub(1) as u16;
        let x0 = list_content.x;
        let x1 = list_content.x + list_content.width;
        // Stop a gutter short of the right edge so the rule keeps the same right
        // padding as the row text above it.
        let x_right = x1.saturating_sub(LEFT_PANE_GUTTER);
        // The label row's rightmost non-blank cell marks where the text ends.
        let mut text_end = x0;
        for x in x0..x1 {
            if buf[(x, y)].symbol() != " " {
                text_end = x;
            }
        }
        // One blank cell of breathing room, then the rule (in the heading's own
        // color) to the right gutter.
        for x in text_end.saturating_add(2)..x_right {
            buf[(x, y)]
                .set_symbol("─")
                .set_fg(self.theme.provider_label_fg);
        }
    }

    fn render_left(&mut self, frame: &mut Frame, area: Rect) {
        let focused = self.focus == FocusPane::Left;

        if self.left_collapsed {
            self.mouse_layout.left_list = self.themed_block("", focused).inner(area);
            let collapsed_left_items = self.left_items();
            let items = collapsed_left_items
                .iter()
                .map(|item| match item {
                    LeftItem::Session(index) => {
                        let Some(session) = self.engine.sessions.get(*index) else {
                            return ListItem::new(Line::from(""));
                        };
                        // Flat icon rail: a single status dot per agent (no tree
                        // connectors). Spinner while streaming (any-tab), else the
                        // steady status dot.
                        let (dot, dot_color) =
                            if matches!(session.status, crate::model::SessionStatus::Active)
                                && self.engine.session_is_streaming(&session.id)
                            {
                                (
                                    crate::theme::SPINNER_FRAMES[self.spinner_frame_index()]
                                        .to_string(),
                                    self.theme.session_active,
                                )
                            } else {
                                let (glyph, color) = self.theme.session_dot(&session.status);
                                (glyph.to_string(), color)
                            };
                        // Surface a warning glyph in the narrow rail too, when the
                        // agent's project has a missing path or its record is gone.
                        let found = self
                            .engine
                            .projects
                            .iter()
                            .find(|p| p.id == session.project_id);
                        let mut spans = vec![Span::styled(dot, Style::default().fg(dot_color))];
                        if !matches!(project_tag_kind(found), ProjectTagKind::Healthy) {
                            spans.push(Span::styled(
                                "⚠",
                                Style::default().fg(self.theme.project_missing_fg),
                            ));
                        }
                        ListItem::new(Line::from(spans))
                    }
                    LeftItem::InactiveToggle => ListItem::new(Line::from(Span::styled(
                        "─",
                        Style::default().fg(self.theme.header_separator_fg),
                    ))),
                })
                .collect::<Vec<_>>();
            let mut state = ListState::default().with_selected(Some(self.selected_left));
            StatefulWidget::render(
                List::new(items)
                    .block(self.themed_block("", focused))
                    .highlight_style(self.theme.selection_style()),
                area,
                frame.buffer_mut(),
                &mut state,
            );
            // Icon-rail rows are one line each, so the map is a plain per-item
            // walk from the scroll offset (kept uniform with the expanded path so
            // hit-testing goes through one code path).
            let heights = vec![1u16; collapsed_left_items.len()];
            self.mouse_layout.left_row_to_item =
                left_row_to_item(state.offset(), &heights, self.mouse_layout.left_list.height);
            return;
        }

        let terminal_items = self.terminal_items();
        let has_terminals = !terminal_items.is_empty();

        // Split area vertically: projects on top, terminals on bottom (if any).
        let (projects_area, terminals_area) = if has_terminals {
            let pct = self.terminal_pane_height_pct.clamp(10, 80);
            let projects_pct = 100u16.saturating_sub(pct).max(20);
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Percentage(projects_pct),
                    Constraint::Percentage(pct),
                ])
                .split(area);
            (chunks[0], Some(chunks[1]))
        } else {
            (area, None)
        };

        // Collect terminal display info for rendering.
        // Show the foreground command if one is running, otherwise the session label.
        // Each row carries an owner-derived display name (the agent's branch or
        // the project's name) so a generic engine label like "Terminal 3" is
        // never ambiguous between an agent terminal and a project terminal.
        let terminal_render_data: Vec<(String, Option<String>, String)> = terminal_items
            .iter()
            .map(|(_, t)| {
                let owner_name = match &t.owner {
                    TerminalOwner::Session(sid) => self
                        .engine
                        .sessions
                        .iter()
                        .find(|s| &s.id == sid)
                        .map(|s| s.title.clone().unwrap_or_else(|| s.branch_name.clone()))
                        .unwrap_or_else(|| sid.clone()),
                    TerminalOwner::Project(pid) => self
                        .engine
                        .projects
                        .iter()
                        .find(|p| &p.id == pid)
                        .map(|p| p.name.clone())
                        .unwrap_or_else(|| pid.clone()),
                };
                (t.label.clone(), t.foreground_cmd.clone(), owner_name)
            })
            .collect();

        let left_items = self.left_items();
        let projects_focused = focused && self.left_section == LeftSection::Projects;
        // While filtering, show the visible-over-total count (e.g. "Agents (2/7)")
        // so the pruned list is not mistaken for the whole roster.
        let total_agents = self.engine.sessions.len();
        let title = if self.agent_filter.is_some() {
            let visible_agents = left_items
                .iter()
                .filter(|item| matches!(item, LeftItem::Session(_)))
                .count();
            format!("Agents ({visible_agents}/{total_agents})")
        } else {
            format!("Agents ({total_agents})")
        };
        // Active agents always sort ahead of the Inactive toggle, so the list
        // leads with a Session iff any agent is active. The toggle only earns a
        // leading spacer row (separating it from the active agents) when there is
        // something above it; with only inactive agents it sits flush at the top.
        let has_active = matches!(left_items.first(), Some(LeftItem::Session(_)));
        // Build the block up front so the row builder knows the text width left
        // after both gutters are reserved, and can ellipsize overflowing lines.
        let block = self.themed_block(&title, projects_focused);
        let inner = block.inner(projects_area);
        let row_text_width = inner.width.saturating_sub(LEFT_PANE_GUTTER * 2);
        let items = left_items
            .iter()
            .map(|item| match item {
                LeftItem::InactiveToggle => {
                    // Count only the inactive rows visible under the current
                    // filter, so the toggle matches what expanding reveals.
                    let count = self.visible_inactive_count();
                    let icon = if self.inactive_collapsed {
                        "▸"
                    } else {
                        "▾"
                    };
                    let label = Line::from(vec![Span::styled(
                        format!("{icon} Inactive ({count})"),
                        Style::default().fg(self.theme.provider_label_fg),
                    )]);
                    // Layout: an optional leading separator (a plain unused row,
                    // never highlighted, present only when active agents sit above),
                    // the label, then a trailing spacer that serves as the boundary
                    // row for the first inactive agent's top padding.
                    if has_active {
                        ListItem::new(vec![Line::from(""), label, Line::from("")])
                    } else {
                        ListItem::new(vec![label, Line::from("")])
                    }
                }
                LeftItem::Session(index) => {
                    let Some(session) = self.engine.sessions.get(*index) else {
                        // Keep the fallback the same height the row map assumes for
                        // a Session item (three lines).
                        return ListItem::new(vec![Line::from(""), Line::from(""), Line::from("")]);
                    };
                    self.render_agent_row(session, row_text_width)
                }
            })
            .collect::<Vec<_>>();
        // Each item's rendered height, kept in lockstep with the arms above: an
        // agent row is three lines (name, metadata, trailing spacer) and the
        // Inactive toggle is three (leading separator + label + trailing spacer)
        // when active agents precede it, else two (label + trailing spacer).
        // Computed here, while `left_items` is still borrowed and before any
        // mutable `self` access, then consumed after render to rebuild the map.
        let item_heights: Vec<u16> = left_items
            .iter()
            .map(|it| match it {
                LeftItem::Session(_) => 3,
                LeftItem::InactiveToggle => {
                    if has_active {
                        3
                    } else {
                        2
                    }
                }
            })
            .collect();
        // Reserve a one-line search input at the TOP of the pane while filtering.
        // It carries a `/` affordance plus the live query and a block cursor, and is
        // themed through the same input-cursor tokens as the other pane inputs.
        let (search_area, list_inner) = if self.agent_filter.is_some() && inner.height >= 3 {
            let [sa, la] = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(2), Constraint::Min(1)])
                .areas(inner);
            (Some(sa), la)
        } else {
            (None, inner)
        };
        // Two reservations for the framed selection:
        //  - a one-row top margin (only when active agents lead the list) so the
        //    first agent's `▄` top edge has a row to paint into; and
        //  - a one-column gutter on each side, so the tinted selection frame has a
        //    margin and the text sits evenly padded left and right.
        // `list_content` is the click surface and the origin for the frame edges;
        // the list itself renders into `list_body`, inset by a gutter on each side.
        let top_margin: u16 = if has_active && list_inner.height >= 4 {
            1
        } else {
            0
        };
        let top_pad_y = (top_margin == 1).then_some(list_inner.y);
        let list_content = Rect {
            y: list_inner.y + top_margin,
            height: list_inner.height - top_margin,
            ..list_inner
        };
        let list_body = Rect {
            x: list_content.x + LEFT_PANE_GUTTER,
            width: list_content.width.saturating_sub(LEFT_PANE_GUTTER * 2),
            ..list_content
        };
        self.mouse_layout.left_list = list_content;
        block.render(projects_area, frame.buffer_mut());
        if let Some(search_area) = search_area {
            let (text, cursor) = self
                .agent_filter
                .as_ref()
                .map(|input| (input.text.clone(), input.cursor))
                .unwrap_or_default();
            Paragraph::new(render_single_line_cursor_input(
                "/ ",
                &text,
                cursor,
                self.theme.input_cursor_fg,
                self.theme.input_cursor_bg,
            ))
            .block(
                Block::default()
                    .borders(Borders::BOTTOM)
                    .border_style(Style::default().fg(self.theme.border_normal)),
            )
            .render(search_area, frame.buffer_mut());
        }
        let mut state =
            ListState::default().with_selected(if self.left_section == LeftSection::Projects {
                Some(self.selected_left)
            } else {
                None
            });
        // No widget highlight: the selection is painted by hand below (an accent
        // bar plus a faint tint) so it keeps each row's text colors and leaves the
        // Inactive separator row untouched — neither of which a whole-cell List
        // highlight can do. Rendered into the gutter-shifted body.
        StatefulWidget::render(List::new(items), list_body, frame.buffer_mut(), &mut state);
        // Agent rows are three lines tall, so a click row no longer maps 1:1 to a
        // list item. Rebuild the reverse map from the post-render scroll offset
        // and each item's rendered height (computed above).
        self.mouse_layout.left_row_to_item =
            left_row_to_item(state.offset(), &item_heights, list_content.height);
        // When an overlay is about to grayscale the body (a modal, the help page,
        // or a fullscreen view), skip the hand-painted selection: its half-height
        // blocks are drawn in the foreground, so `render_dim_overlay` would leave
        // them as visible grey strips instead of letting the selection blend into
        // the dim like every other row. The widget-highlight era got this for free
        // because the selection was a background that dimmed to the overlay color.
        let body_will_dim = self.help_scroll.is_some()
            || !matches!(self.prompt, PromptState::None)
            || !matches!(self.fullscreen_overlay, FullscreenOverlay::None);
        if self.left_section == LeftSection::Projects && !body_will_dim {
            self.paint_left_selection(frame.buffer_mut(), list_content, top_pad_y);
        }
        // A dim rule runs from the end of the "Inactive" label to the right edge,
        // but only while the toggle is not the current selection.
        if !body_will_dim {
            self.paint_inactive_rule(frame.buffer_mut(), list_content);
        }

        // Render terminals section if any terminals exist.
        if let Some(term_area) = terminals_area {
            let terminals_focused = focused && self.left_section == LeftSection::Terminals;
            let term_title = format!("Terminals ({})", terminal_render_data.len());
            self.mouse_layout.terminal_list = self
                .themed_block(&term_title, terminals_focused)
                .inner(term_area);
            let term_items: Vec<ListItem> = terminal_render_data
                .iter()
                .enumerate()
                .map(|(i, (label, fg_cmd, owner_name))| {
                    let color = self.theme.session_active;
                    let mut spans = vec![Span::styled("● ", Style::default().fg(color))];
                    if let Some(cmd) = fg_cmd {
                        // The running app's name is the row's label. Only when
                        // another terminal runs the same app do we disambiguate
                        // with the terminal's number ("vim (#1)"), mirroring the
                        // web terminalTitle rule.
                        let duplicate = terminal_render_data
                            .iter()
                            .enumerate()
                            .any(|(j, (_, other, _))| j != i && other.as_deref() == Some(cmd));
                        spans.push(Span::styled(cmd.clone(), Style::default().fg(color)));
                        let suffix = terminal_dup_suffix(label, duplicate);
                        if !suffix.is_empty() {
                            spans.push(Span::styled(
                                suffix,
                                Style::default().fg(self.theme.provider_label_fg),
                            ));
                        }
                    } else {
                        spans.push(Span::styled(label.clone(), Style::default().fg(color)));
                    }
                    // The owner name (agent branch/title or project name)
                    // disambiguates generic labels; skip it when the label
                    // already names the owner (the TUI's own spawn labels).
                    if !label.starts_with(owner_name.as_str()) {
                        spans.push(Span::styled(
                            format!(" · {owner_name}"),
                            Style::default().fg(self.theme.provider_label_fg),
                        ));
                    }
                    ListItem::new(Line::from(spans))
                })
                .collect();
            let mut term_state = ListState::default().with_selected(
                if self.left_section == LeftSection::Terminals {
                    Some(self.selected_terminal_index)
                } else {
                    None
                },
            );
            StatefulWidget::render(
                List::new(term_items)
                    .block(self.themed_block(&term_title, terminals_focused))
                    .highlight_style(self.theme.selection_style()),
                term_area,
                frame.buffer_mut(),
                &mut term_state,
            );
        } else {
            self.mouse_layout.terminal_list = Rect::default();
        }
    }

    fn render_center(&mut self, frame: &mut Frame, area: Rect) {
        let focused = self.focus == FocusPane::Center;

        // Determine if a PR banner should be shown above the center pane.
        let is_input = matches!(
            (self.input_target, self.session_surface),
            (InputTarget::Agent, SessionSurface::Agent)
                | (InputTarget::Terminal, SessionSurface::Terminal)
        );
        let pr_info = if !is_input {
            self.selected_session()
                .and_then(|s| self.engine.pr_statuses.get(&s.id))
                .cloned()
        } else {
            None
        };
        let pr_banner_height: u16 = if pr_info.is_some() { 1 } else { 0 };

        let (pr_area, pane_area) = if self.pr_banner_at_bottom {
            let [pane_area, pr_area] = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(pr_banner_height)])
                .areas(area);
            (pr_area, pane_area)
        } else {
            let [pr_area, pane_area] = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(pr_banner_height), Constraint::Min(1)])
                .areas(area);
            (pr_area, pane_area)
        };

        if let Some(ref pr) = pr_info {
            self.render_pr_banner(frame, pr_area, pr);
        }

        match &self.center_mode {
            CenterMode::Diff { .. } => {
                self.render_diff(frame, pane_area, focused);
            }
            CenterMode::Agent if !matches!(self.fullscreen_overlay, FullscreenOverlay::None) => {
                // Skip agent rendering here — fullscreen overlay handles it.
                // Rendering in both places causes the PTY to be resized twice
                // per frame (once to the small pane, once to the overlay).
                let title = self.center_pane_agent_title();
                self.themed_block(&title, focused)
                    .render(pane_area, frame.buffer_mut());
            }
            CenterMode::Agent => {
                let title = self.center_pane_agent_title();
                // Center pane always renders the agent; terminal is an overlay.
                let saved = self.session_surface;
                self.session_surface = SessionSurface::Agent;
                // Draw the tab strip (>=2 tabs) above the terminal and render
                // the terminal into the remaining area.
                let term_area = self.render_agent_tab_strip_if_needed(frame, pane_area, true);
                self.render_agent_terminal(frame, term_area, &title, focused);
                self.session_surface = saved;
            }
        }
    }

    fn render_diff(&mut self, frame: &mut Frame, area: Rect, focused: bool) {
        let (lines, scroll, gutter_width) = match &self.center_mode {
            CenterMode::Diff {
                lines,
                scroll,
                gutter_width,
                ..
            } => (Arc::clone(lines), *scroll, *gutter_width),
            _ => return,
        };

        let outer_block = self.themed_block("Diff", focused);
        let inner = outer_block.inner(area);
        outer_block.render(area, frame.buffer_mut());

        if inner.height < 3 || inner.width < 4 {
            return;
        }

        let hint_height = 2;
        let [content_area, hint_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(hint_height)])
            .areas(inner);
        self.mouse_layout.agent_term = Some(content_area);

        self.last_diff_height = content_area.height;

        let w = content_area.width.max(1) as usize;

        if gutter_width > 0 {
            // Gutter-aware wrapping: continuation lines are indented to align
            // with the content column past the gutter.
            let wrapped = crate::diff::wrap_diff_lines(&lines, w, gutter_width);
            self.last_diff_visual_lines = wrapped.len() as u16;

            let max_scroll = self
                .last_diff_visual_lines
                .saturating_sub(content_area.height);
            let scroll = scroll.min(max_scroll);

            Paragraph::new(wrapped)
                .scroll((scroll, 0))
                .render(content_area, frame.buffer_mut());
        } else {
            // No gutter — fall back to ratatui's built-in wrapping.
            self.last_diff_visual_lines = lines
                .iter()
                .map(|l| {
                    let lw = l.width();
                    if lw <= w { 1u16 } else { lw.div_ceil(w) as u16 }
                })
                .sum();

            let max_scroll = self
                .last_diff_visual_lines
                .saturating_sub(content_area.height);
            let scroll = scroll.min(max_scroll);

            Paragraph::new((*lines).clone())
                .wrap(Wrap { trim: false })
                .scroll((scroll, 0))
                .render(content_area, frame.buffer_mut());
        }

        // Hint bar with top border (same style as agent terminal).
        if hint_area.height > 0 {
            let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
            let scroll_down = self.bindings.labels_for(Action::ScrollPageDown);
            let scroll_up = self.bindings.labels_for(Action::ScrollPageUp);
            let scroll_line = self.bindings.label_for(Action::ScrollLineDown);
            let close = self.bindings.label_for(Action::CloseOverlay);
            let mut spans: Vec<Span> = Vec::new();

            if scroll > 0 {
                spans.push(Span::styled(
                    format!("Scrolled back {scroll} lines. "),
                    Style::default().fg(self.theme.hint_key_fg),
                ));
                spans.extend(self.theme.dim_key_badge_default(&scroll_down));
                spans.push(Span::styled(" down, ", desc_style));
                spans.extend(self.theme.dim_key_badge_default(&scroll_up));
                spans.push(Span::styled(" up, ", desc_style));
                spans.extend(self.theme.dim_key_badge_default(&scroll_line));
                spans.push(Span::styled(" one line. ", desc_style));
            } else {
                spans.extend(self.theme.dim_key_badge_default(&scroll_up));
                spans.push(Span::styled(" ", desc_style));
                spans.extend(self.theme.dim_key_badge_default(&scroll_down));
                spans.push(Span::styled(" to scroll. ", desc_style));
                spans.extend(self.theme.dim_key_badge_default(&scroll_line));
                spans.push(Span::styled(" one line. ", desc_style));
            }
            spans.extend(self.theme.dim_key_badge_default(&close));
            spans.push(Span::styled(" close diff.", desc_style));

            Paragraph::new(Line::from(spans))
                .block(
                    Block::default()
                        .borders(Borders::TOP)
                        .border_style(Style::default().fg(self.theme.border_normal)),
                )
                .render(hint_area, frame.buffer_mut());
        }
    }

    /// Render the ASCII "dux" logo centered in the given area, with an
    /// optional feature tip displayed below.
    /// Centered, provider-agnostic message shown when a focused tab has no live
    /// process (dormant, e.g. after a restart). dux does not restore a tab's
    /// conversation across a restart; the message points the user at their CLI's
    /// own history.
    fn render_dormant_support_tab(&mut self, frame: &mut Frame, area: Rect) {
        self.welcome_logo_visible = false;
        if area.height < 5 || area.width < 20 {
            return;
        }
        let title_style = Style::default()
            .fg(self.theme.title_focused)
            .add_modifier(Modifier::BOLD);
        let body_style = Style::default().fg(self.theme.hint_desc_fg);
        let dim_style = Style::default().fg(self.theme.hint_dim_desc_fg);
        let key_style = Style::default().fg(self.theme.hint_key_fg);
        let lines = vec![
            Line::from(Span::styled("Tab not running", title_style)),
            Line::from(""),
            Line::from(Span::styled(
                "dux doesn't restore a tab's conversation across a restart.",
                body_style,
            )),
            Line::from(Span::styled(
                "To pick up its previous conversation, start a fresh",
                dim_style,
            )),
            Line::from(Span::styled(
                "session and use your CLI's own history command.",
                dim_style,
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("Press ", body_style),
                Span::styled("Enter", key_style),
                Span::styled(" to start a fresh session.", body_style),
            ]),
        ];
        let h = lines.len() as u16;
        let y = area.y + (area.height.saturating_sub(h)) / 2;
        let card = Rect::new(area.x, y, area.width, h.min(area.height));
        Paragraph::new(lines)
            .alignment(ratatui::layout::Alignment::Center)
            .render(card, frame.buffer_mut());
    }

    fn render_ascii_logo(&mut self, frame: &mut Frame, area: Rect) {
        if area.width < ASCII_LOGO_WIDTH || area.height < ASCII_LOGO_HEIGHT {
            return;
        }

        // Rotate the tip and randomly pick a logo variant when the logo
        // becomes visible again after being hidden, or when the selected
        // left-pane item changes while the logo stays visible.
        if !self.welcome_logo_visible || self.welcome_tip_selection != self.selected_left {
            self.welcome_tip_index = self.welcome_tip_index.wrapping_add(1);
            self.welcome_logo_alt = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos() % 2 == 0)
                .unwrap_or(false);
        }
        self.welcome_logo_visible = true;
        self.welcome_tip_selection = self.selected_left;

        // Pick the active logo variant. Fall back to the text logo when the
        // area is too short for the taller duck.
        let use_alt = self.welcome_logo_alt && area.height >= ASCII_LOGO_ALT_HEIGHT;
        let (logo, logo_w, logo_h) = if use_alt {
            (ASCII_LOGO_ALT, ASCII_LOGO_ALT_WIDTH, ASCII_LOGO_ALT_HEIGHT)
        } else {
            (ASCII_LOGO, ASCII_LOGO_WIDTH, ASCII_LOGO_HEIGHT)
        };

        let total_height = logo_h + TIP_GAP + TIP_MAX_LINES;
        let show_tip = area.width >= TIP_MAX_WIDTH && area.height >= total_height;

        let block_height = if show_tip { total_height } else { logo_h };
        let x = area.x + (area.width - logo_w) / 2;
        let y = area.y + (area.height - block_height) / 2;

        // --- logo ---
        let style = Style::default().fg(self.theme.border_normal);
        let lines: Vec<Line> = logo.iter().map(|l| Line::styled(*l, style)).collect();
        Paragraph::new(lines).render(Rect::new(x, y, logo_w, logo_h), frame.buffer_mut());

        // --- tip pill ---
        if show_tip {
            let bindings = &self.bindings;
            let resolve = |action: Action| bindings.label_for(action);
            let tui_tips: Vec<_> = dux_core::welcome::WELCOME_TIPS
                .iter()
                .filter_map(|t| t.tui)
                .collect();
            let tip_text = tui_tips[self.welcome_tip_index % tui_tips.len()](&resolve);

            let pill_span = Span::styled(
                " Tip ",
                Style::default()
                    .fg(self.theme.tip_pill_fg)
                    .bg(self.theme.tip_pill_bg)
                    .add_modifier(Modifier::BOLD),
            );

            let normal = Style::default().fg(self.theme.tip_text_fg);
            let highlight = Style::default()
                .fg(self.theme.tip_highlight_fg)
                .add_modifier(Modifier::BOLD);

            let mut spans: Vec<Span> = vec![pill_span, Span::raw(" ")];
            let mut inside_backtick = false;
            for segment in tip_text.split('`') {
                if !segment.is_empty() {
                    spans.push(Span::styled(
                        segment.to_owned(),
                        if inside_backtick { highlight } else { normal },
                    ));
                }
                inside_backtick = !inside_backtick;
            }

            let tip_line = Line::from(spans);
            let tip_width = TIP_MAX_WIDTH.min(area.width.saturating_sub(2));
            let tip_x = area.x + (area.width - tip_width) / 2;
            let tip_y = y + logo_h + TIP_GAP;

            Paragraph::new(vec![tip_line])
                .wrap(Wrap { trim: false })
                .alignment(ratatui::layout::Alignment::Center)
                .render(
                    Rect::new(tip_x, tip_y, tip_width, TIP_MAX_LINES),
                    frame.buffer_mut(),
                );
        }
    }

    fn render_terminal_placeholder(
        &self,
        frame: &mut Frame,
        area: Rect,
        status: CompanionTerminalStatus,
        command_name: Option<&str>,
    ) {
        if area.width < 4 || area.height < 3 {
            return;
        }

        let (icon, label) = companion_terminal_status_meta(status);
        let command_name = command_name.unwrap_or("terminal");
        let lines = match status {
            CompanionTerminalStatus::NotLaunched => vec![
                Line::from(Span::styled(
                    format!("{icon} Companion terminal not launched"),
                    Style::default().fg(companion_terminal_status_color(&self.theme, status)),
                )),
                Line::from(Span::styled(
                    format!(
                        "Launch {command_name} explicitly when you need a shell in this worktree."
                    ),
                    Style::default().fg(self.theme.hint_dim_desc_fg),
                )),
            ],
            CompanionTerminalStatus::Running => vec![
                Line::from(Span::styled(
                    format!("{icon} Companion terminal {label}"),
                    Style::default().fg(companion_terminal_status_color(&self.theme, status)),
                )),
                Line::from(Span::styled(
                    "The PTY is alive even when hidden from the center pane.",
                    Style::default().fg(self.theme.hint_dim_desc_fg),
                )),
            ],
            CompanionTerminalStatus::Exited => vec![
                Line::from(Span::styled(
                    format!("{icon} Companion terminal exited"),
                    Style::default().fg(companion_terminal_status_color(&self.theme, status)),
                )),
                Line::from(Span::styled(
                    "Relaunch it explicitly to start a fresh shell.",
                    Style::default().fg(self.theme.hint_dim_desc_fg),
                )),
            ],
        };

        let height = lines.len() as u16;
        let y = area.y + area.height.saturating_sub(height) / 2;
        Paragraph::new(lines)
            .alignment(ratatui::layout::Alignment::Center)
            .render(
                Rect::new(area.x, y, area.width, height.max(1)),
                frame.buffer_mut(),
            );
    }

    /// Provider label for a specific tab of a session (Main resolves to the
    /// session's running provider; an extra tab to its pin/row provider).
    fn tab_provider_label(&self, session: &AgentSession, tab_id: &str) -> String {
        self.engine
            .tab_running_provider(session, tab_id)
            .as_str()
            .to_string()
    }

    /// If the selected agent has two or more tabs, draw a single-row desktop-style
    /// tab strip at the top of `area` and return the reduced rect for the terminal
    /// below it. With fewer than two tabs (or no room) returns `area` unchanged, so
    /// single-tab agents look exactly as before. When `record_clicks` is false
    /// (fullscreen) the strip is display-only and no hit-boxes are recorded.
    fn render_agent_tab_strip_if_needed(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        record_clicks: bool,
    ) -> Rect {
        self.agent_tab_regions.clear();
        self.agent_tab_add_region = None;

        let Some(session) = self.selected_session() else {
            return area;
        };
        let session_id = session.id.clone();
        let tab_ids = self.session_tab_ids(&session_id);
        let always_show = self.engine.config.ui.always_show_tab_strip;
        if (tab_ids.len() < 2 && !always_show) || area.height < 4 || area.width < 12 {
            return area;
        }
        let focused_id = self.focused_tab_id(&session_id);
        let at_cap = tab_ids.len() >= self.engine.agent_tabs_max() as usize;

        // Gather owned per-tab data under immutable borrows, then render/mutate.
        let providers: Vec<String> = tab_ids
            .iter()
            .map(|id| self.tab_provider_label(session, id))
            .collect();
        let labels = tab_labels(&providers.iter().map(|s| s.as_str()).collect::<Vec<_>>());

        let [strip_area, term_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .areas(area);

        // Add button occupies the rightmost columns. A closing separator sits
        // just left of it so the last tab reads as boxed-in, matching the
        // leading separator every tab carries (see `seg_content` below).
        let add_text = " + ";
        let add_w = add_text.chars().count() as u16;
        let add_total_w = add_w + 1;
        let avail = strip_area.width.saturating_sub(add_total_w);

        // Segment text/width per tab. All tabs are generic — no per-tab marker
        // — except the focused tab, which is prefixed with the shared solid
        // dot glyph so the active tab is unambiguous even without color
        // (matches the "●" = active/present convention used by
        // `session_dot`/`ATTENTION_GLYPH` elsewhere in the theme). Every tab,
        // focused or not, reserves the same dot-width gutter: an unfocused
        // tab renders spaces where the dot would go, so a tab's rendered
        // width never depends on whether it is focused and the strip doesn't
        // reflow/jitter as focus moves. Each segment carries a leading `│`
        // separator so adjacent tabs read as bordered/boxed rather than a
        // bare run of text; the separator is drawn in its own style, not
        // counted as part of the label content.
        const TAB_SEP: &str = "│";
        let tab_active_dot: &str = crate::theme::DOT_GLYPH;
        let dot_gutter: String = " ".repeat(tab_active_dot.cell_width().max(1) as usize);
        let mut seg_content: Vec<String> = labels
            .iter()
            .enumerate()
            .map(|(i, l)| {
                if tab_ids[i] == focused_id {
                    format!(" {tab_active_dot} {l} ")
                } else {
                    format!(" {dot_gutter} {l} ")
                }
            })
            .collect();
        // +1 per segment for the leading separator column. Measured in real
        // display columns (unicode-width via `CellWidth`), not
        // `chars().count()`: a char-count measure undercounts double-width
        // CJK/emoji glyphs in custom provider labels, which would overflow
        // the segment's recorded region and paint over the add button.
        let mut seg_w: Vec<u16> = seg_content
            .iter()
            .map(|t| t.as_str().cell_width() + 1)
            .collect();

        // Choose a start index so the focused tab is visible within `avail`.
        let focused_idx = tab_ids.iter().position(|i| *i == focused_id).unwrap_or(0);

        // If the focused segment alone is wider than the available strip
        // width, truncate its label (UTF-8/width-safe) so it fits within
        // `avail`. Without this, the scroll-into-view loop below can never
        // include the focused segment (its width alone exceeds `avail`, so
        // the inner loop always yields `count == 0` at `start == focused_idx`)
        // and `start` walks straight past it, leaving the focused tab
        // entirely off-screen.
        if let Some(focused_w) = seg_w.get(focused_idx).copied()
            && focused_w > avail
        {
            // Reserve 1 column for the leading separator; fit the rest of the
            // content (dot/gutter + label + padding) into what remains.
            let budget = avail.saturating_sub(1);
            seg_content[focused_idx] = truncate_to_width(&seg_content[focused_idx], budget);
            seg_w[focused_idx] = seg_content[focused_idx].as_str().cell_width() + 1;
        }

        let mut start = 0usize;
        loop {
            let mut w = 0u16;
            let mut count = 0usize;
            for width in seg_w.iter().skip(start) {
                if w + *width > avail {
                    break;
                }
                w += *width;
                count += 1;
            }
            let end = start + count;
            if focused_idx >= end && end < seg_w.len() {
                start += 1;
                if start >= seg_w.len() {
                    start = seg_w.len() - 1;
                    break;
                }
            } else {
                break;
            }
        }
        // Safety net: guarantee the focused tab is visible even if the loop
        // above couldn't include it (e.g. it walked `start` past
        // `focused_idx` before the truncation above narrowed the segment).
        if start > focused_idx {
            start = focused_idx;
        }

        let buf = frame.buffer_mut();
        // Base fill for the strip row. Painted with the shared bar background
        // (the same token the footer hint bar uses) so unfocused tabs, which
        // set only a foreground, read against a deliberate panel color
        // instead of whatever was left behind in the buffer.
        let base_style = Style::default().bg(self.theme.hint_bar_bg);
        for x in strip_area.x..strip_area.x + strip_area.width {
            buf[(x, strip_area.y)].set_symbol(" ").set_style(base_style);
        }

        // Separator style: a quiet chrome divider, matching the semantic
        // meaning of `border_normal` elsewhere (pane/box borders).
        let sep_style = Style::default()
            .fg(self.theme.border_normal)
            .bg(self.theme.hint_bar_bg);

        let mut x = strip_area.x;
        for i in start..seg_content.len() {
            if x + seg_w[i] > strip_area.x + avail {
                // No more room; show an overflow marker if any remain.
                if i < seg_content.len() {
                    let ell_style = Style::default().fg(self.theme.hint_dim_desc_fg);
                    if x < strip_area.x + avail {
                        buf[(x, strip_area.y)].set_symbol("…").set_style(ell_style);
                    }
                }
                break;
            }
            let active = tab_ids[i] == focused_id;
            // Focused tab: the same legible selection combo used everywhere
            // else in the UI (selection_fg on selection_bg) — not
            // `title_focused`, which in the default theme is the same color
            // as `selection_bg` and made the label invisible against its own
            // highlight. Unfocused tabs use `title_normal`, the semantic
            // "quiet but legible" pane-title color, on the strip's base fill.
            let style = if active {
                self.theme.selection_style()
            } else {
                Style::default().fg(self.theme.title_normal)
            };
            buf[(x, strip_area.y)]
                .set_symbol(TAB_SEP)
                .set_style(sep_style);
            buf.set_string(x + 1, strip_area.y, &seg_content[i], style);
            if record_clicks {
                self.agent_tab_regions
                    .push((tab_ids[i].clone(), Rect::new(x, strip_area.y, seg_w[i], 1)));
            }
            x += seg_w[i];
        }

        // Trailing add button, with a closing separator immediately to its
        // left so the last tab is boxed in on both sides.
        let add_x = strip_area.x + strip_area.width - add_w;
        buf[(add_x - 1, strip_area.y)]
            .set_symbol(TAB_SEP)
            .set_style(sep_style);
        let add_style = if at_cap {
            Style::default().fg(self.theme.hint_dim_desc_fg)
        } else {
            Style::default().fg(self.theme.hint_key_fg)
        };
        buf.set_string(add_x, strip_area.y, add_text, add_style);
        if record_clicks && !at_cap {
            self.agent_tab_add_region = Some(Rect::new(add_x, strip_area.y, add_w, 1));
        }

        term_area
    }

    fn render_agent_terminal(&mut self, frame: &mut Frame, area: Rect, title: &str, focused: bool) {
        let nudge_active = self.is_nudge_active();
        let outer_block = if nudge_active {
            self.themed_block(title, focused)
                .border_style(Style::default().fg(self.theme.nudge_border))
        } else {
            self.themed_block(title, focused)
        };
        let inner = outer_block.inner(area);
        outer_block.render(area, frame.buffer_mut());

        if inner.height < 2 || inner.width < 4 {
            return;
        }

        let active_surface = self.session_surface;
        let terminal_status = self.selected_companion_terminal_status();
        let is_input = matches!(
            (self.input_target, active_surface),
            (InputTarget::Agent, SessionSurface::Agent)
                | (InputTarget::Terminal, SessionSurface::Terminal)
        );
        let mut scrollback_offset: usize = 0;
        let mut rendered_content = false;

        // Reserve 2 lines at the bottom for the hint bar (top border + text).
        let hint_height = 2;
        let [term_area, hint_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(hint_height)])
            .areas(inner);
        self.mouse_layout.agent_term = Some(term_area);

        // Get the selected session's PTY screen. Resolve the FOCUSED tab so the
        // caption/liveness reflect the visible tab, not just the Main provider.
        let session_id = self.selected_session().map(|s| s.id.clone());
        let focused_tab = session_id.as_ref().map(|id| self.focused_tab_id(id));
        let session_provider_name = match active_surface {
            SessionSurface::Agent => self
                .selected_session()
                .map(|s| self.focused_tab_provider(s).as_str().to_owned()),
            SessionSurface::Terminal => Some(
                self.engine
                    .config
                    .terminal
                    .command
                    .rsplit(std::path::MAIN_SEPARATOR)
                    .next()
                    .unwrap_or(self.engine.config.terminal.command.as_str())
                    .to_string(),
            ),
        };
        let session_active = match active_surface {
            SessionSurface::Agent => focused_tab
                .as_ref()
                .map(|id| self.engine.providers.contains_key(id))
                .unwrap_or(false),
            SessionSurface::Terminal => terminal_status.is_running(),
        };
        let new_size = (term_area.height, term_area.width);
        let should_resize = new_size != self.last_pty_size && new_size.0 > 0 && new_size.1 > 0;
        if should_resize {
            self.last_pty_size = new_size;
        }

        if let Some(provider) = self.selected_terminal_surface_client() {
            rendered_content = true;
            // Resize PTY if needed.
            if should_resize {
                let _ = provider.resize(new_size.0, new_size.1);
            }

            if !provider.has_output() {
                // Show a centered loading card until the PTY produces output.
                let idx = self.spinner_frame_index();
                let spinner = crate::theme::SPINNER_FRAMES[idx];
                let (label_spans, label_len) = match session_provider_name.as_deref() {
                    Some(name) => {
                        let prefix = match active_surface {
                            SessionSurface::Agent => "Starting ",
                            SessionSurface::Terminal => "Launching ",
                        };
                        let text_len = prefix.len() + name.len() + "...".len();
                        let spans = vec![
                            Span::styled(prefix, Style::default().fg(self.theme.hint_desc_fg)),
                            Span::styled(
                                name.to_owned(),
                                Style::default().fg(self.theme.branch_fg),
                            ),
                            Span::styled("...", Style::default().fg(self.theme.hint_desc_fg)),
                        ];
                        (spans, text_len)
                    }
                    None => {
                        let text = match active_surface {
                            SessionSurface::Agent => "Starting agent...",
                            SessionSurface::Terminal => "Launching terminal...",
                        };
                        let spans = vec![Span::styled(
                            text,
                            Style::default().fg(self.theme.hint_desc_fg),
                        )];
                        (spans, text.len())
                    }
                };

                // Card dimensions: border + padding + content + padding + border.
                // +2 for spinner + space prefix.
                let content_w = label_len as u16 + 2;
                let card_w = (content_w + 2 + 6).min(term_area.width); // 2 borders + 6 padding
                let card_h: u16 = 5; // top border, blank, spinner, blank, bottom border

                if term_area.width >= card_w && term_area.height >= card_h {
                    let cx = term_area.x + (term_area.width - card_w) / 2;
                    let cy = term_area.y + (term_area.height - card_h) / 2;
                    let card_area = Rect::new(cx, cy, card_w, card_h);

                    let card_block = Block::default()
                        .borders(Borders::ALL)
                        .border_type(ratatui::widgets::BorderType::Rounded)
                        .border_style(Style::default().fg(self.theme.border_normal));
                    let card_inner = card_block.inner(card_area);
                    card_block.render(card_area, frame.buffer_mut());

                    // Render spinner + label centered inside the card.
                    let mut spans = vec![Span::styled(
                        format!("{spinner} "),
                        Style::default()
                            .fg(self.theme.hint_key_fg)
                            .add_modifier(Modifier::BOLD),
                    )];
                    spans.extend(label_spans);
                    let line = Line::from(spans);
                    Paragraph::new(line)
                        .alignment(ratatui::layout::Alignment::Center)
                        .render(
                            Rect::new(
                                card_inner.x,
                                card_inner.y + card_inner.height / 2,
                                card_inner.width,
                                1,
                            ),
                            frame.buffer_mut(),
                        );
                }
            } else {
                // Capture alt-screen state before mutable borrows — we need
                // it below (after `refresh_snapshot_buf` takes &mut self) to
                // decide whether the scrollback indicator applies.
                let is_alt_screen = provider.is_alt_screen();

                // Render the current terminal viewport into the ratatui
                // buffer, reusing the pre-allocated snapshot buffer to
                // avoid per-frame heap allocation.
                self.refresh_snapshot_buf();
                scrollback_offset = self.snapshot_buf.scrollback_offset;

                // When returning from scrollback to the live bottom,
                // clear the PTY area so stale cells don't linger in
                // ratatui's diff buffer.
                if scrollback_offset != self.prev_scrollback_offset {
                    Clear.render(term_area, frame.buffer_mut());
                }
                self.prev_scrollback_offset = scrollback_offset;

                let buf = frame.buffer_mut();
                for cell in &self.snapshot_buf.cells {
                    if cell.row >= self.snapshot_buf.rows
                        || cell.col >= self.snapshot_buf.cols
                        || cell.row >= term_area.height
                        || cell.col >= term_area.width
                    {
                        continue;
                    }
                    let x = term_area.x + cell.col;
                    let y = term_area.y + cell.row;
                    let (fg, bg) = pty_cell_colors(
                        to_ratatui_color(cell.fg),
                        to_ratatui_color(cell.bg),
                        is_input,
                        &self.theme,
                    );
                    let mut style = Style::default()
                        .fg(fg)
                        .add_modifier(to_ratatui_modifier(cell.modifier));
                    if let Some(bg) = bg {
                        style = style.bg(bg);
                    }
                    let ratatui_cell = &mut buf[(x, y)];
                    // When this cell carries an OSC 8 hyperlink and links are
                    // enabled, wrap its symbol in a self-contained OSC 8 open/close
                    // pair. Per-cell (open + close on every linked cell) is
                    // deliberate: ratatui may repaint any subset of cells in any
                    // order, so a self-contained cell can never leak an unclosed
                    // link; a shared `id` lets a host merge adjacent cells into one
                    // link.
                    let link_uri = self
                        .engine
                        .config
                        .capabilities
                        .hyperlinks
                        .then_some(cell.link)
                        .flatten()
                        .and_then(|idx| self.snapshot_buf.links.get(idx as usize));
                    if let Some(uri) = link_uri {
                        // The OSC 8 open/close bytes are non-printing, but ratatui's
                        // buffer diff derives a cell's on-screen width from its
                        // symbol string. Those escape bytes are NOT zero-width to
                        // unicode-width, so without an override ratatui miscounts the
                        // cell's width and drops the cells that follow it from the
                        // diff. Force the REAL display width of the underlying glyph
                        // (1, or 2 for a wide CJK/emoji cell) so diffing stays
                        // correct.
                        let width = cell.symbol.as_str().cell_width().max(1);
                        let forced =
                            std::num::NonZeroU16::new(width).expect("cell width is at least 1");
                        ratatui_cell
                            .set_symbol(&osc8_wrap_symbol(&cell.symbol, uri))
                            .set_diff_option(CellDiffOption::ForcedWidth(forced));
                    } else {
                        ratatui_cell.set_symbol(&cell.symbol);
                    }
                    ratatui_cell.set_style(style);

                    // Overlay selection highlight if this cell is selected.
                    if let Some(sel) = &self.terminal_selection
                        && sel.anchor != sel.end
                        && sel.contains(cell.row, cell.col)
                    {
                        ratatui_cell.set_style(self.theme.selection_style());
                    }
                }

                // Render cursor if in input mode.
                if is_input
                    && let Some(cursor) = self.snapshot_buf.cursor
                    && cursor.row < self.snapshot_buf.rows
                    && cursor.col < self.snapshot_buf.cols
                {
                    let cx = term_area.x + cursor.col;
                    let cy = term_area.y + cursor.row;
                    if cx < term_area.x + term_area.width && cy < term_area.y + term_area.height {
                        // Do NOT pre-paint the cursor cell into a block here.
                        // We move the real hardware cursor onto this cell below
                        // (`set_cursor_position`), and that hardware cursor IS
                        // the visible block on every host terminal. Painting the
                        // cell to *look* like a cursor as well caused it to
                        // vanish under Alacritty: Alacritty draws its block
                        // cursor by INVERTING the cell's colors, and inverting a
                        // cell we had already styled as a cursor (input_cursor_fg
                        // on prompt_cursor) cancelled back to an invisible caret.
                        // Terminals that draw a fixed-color cursor block stayed
                        // visible, which is why this only reproduced in Alacritty
                        // (issue: invisible caret in Alacritty host). Leaving the
                        // underlying glyph/colors untouched gives every terminal
                        // a normal cell to invert, so the hardware block shows.
                        //
                        // Move the real terminal cursor onto the embedded PTY
                        // cursor cell. IME composition popups (e.g. a Korean
                        // IME) are drawn by the terminal/OS at the hardware
                        // cursor; without this the composing character appears
                        // at the terminal origin instead of the agent prompt
                        // (issue #258). `set_cursor_position` preserves that
                        // alignment — it still lands exactly on the cursor cell.
                        //
                        // This must stay the last use of `buf` in this block:
                        // `set_cursor_position` reborrows `frame`, which is only
                        // valid because the `buf = frame.buffer_mut()` borrow
                        // above has ended. Do not add `buf[...]` accesses below.
                        frame.set_cursor_position((cx, cy));
                    }
                }

                // Suppress the scrollback indicator when the child is using
                // the alternate screen buffer — the alt grid has no history,
                // so the label would be misleading even if it somehow rendered.
                if !is_alt_screen
                    && let Some(label) = scrollback_indicator_label(
                        self.snapshot_buf.scrollback_offset,
                        self.snapshot_buf.scrollback_total,
                    )
                {
                    let badge_width = label.len() as u16;
                    if term_area.height > 0 && badge_width <= term_area.width {
                        Paragraph::new(label)
                            .style(
                                Style::default()
                                    .fg(self.theme.scroll_indicator_fg)
                                    .bg(self.theme.scroll_indicator_bg),
                            )
                            .render(
                                Rect::new(
                                    term_area.x + term_area.width - badge_width,
                                    term_area.y,
                                    badge_width,
                                    1,
                                ),
                                frame.buffer_mut(),
                            );
                    }
                }
            }
        }

        if rendered_content {
            self.welcome_logo_visible = false;
        } else {
            // A focused extra tab with no live process is "dormant" (e.g. after
            // a restart): show a provider-agnostic can't-resume message instead of
            // the welcome logo, with the launch key to start it fresh.
            let dormant_support = matches!(
                (&session_id, &focused_tab),
                (Some(sid), Some(fid)) if fid != sid
            );
            match active_surface {
                SessionSurface::Agent if dormant_support => {
                    self.welcome_logo_visible = false;
                    self.render_dormant_support_tab(frame, term_area);
                }
                SessionSurface::Agent => self.render_ascii_logo(frame, term_area),
                SessionSurface::Terminal => {
                    self.welcome_logo_visible = false;
                    self.render_terminal_placeholder(
                        frame,
                        term_area,
                        terminal_status,
                        session_provider_name.as_deref(),
                    );
                }
            }
        }

        // Macro bar overlays the hint area when active.
        if self.macro_bar.is_some() {
            self.render_macro_bar(frame, inner);
            return;
        }

        // Hint bar with top border.
        if hint_area.height > 0 {
            // Pre-compute all key labels so they outlive the Span borrows.
            let exit_key = self.bindings.label_for(Action::ExitInteractive);
            let scroll_down = self.bindings.labels_for(Action::ScrollPageDown);
            let scroll_up = self.bindings.labels_for(Action::ScrollPageUp);
            let scroll_line = self.bindings.label_for(Action::ScrollLineDown);
            let focus_agent = self.bindings.labels_for(Action::FocusAgent);
            let reconnect = self.bindings.labels_for(Action::ReconnectAgent);

            let macro_key = self.bindings.label_for(Action::OpenMacroBar);
            let hint_line = if is_input {
                let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
                let mut spans: Vec<Span> = Vec::new();
                spans.extend(self.theme.dim_key_badge_default(&exit_key));
                spans.push(Span::styled(" return  ", desc_style));
                spans.extend(self.theme.dim_key_badge_default(&scroll_up));
                spans.push(Span::styled(" up  ", desc_style));
                spans.extend(self.theme.dim_key_badge_default(&scroll_down));
                if scrollback_offset > 0 {
                    spans.push(Span::styled(" down  ", desc_style));
                    spans.extend(self.theme.dim_key_badge_default(&scroll_line));
                    spans.push(Span::styled(" down one line", desc_style));
                } else {
                    spans.push(Span::styled(" down", desc_style));
                }
                if !self.filtered_macros("").is_empty() && !macro_key.is_empty() {
                    spans.push(Span::styled(" ", desc_style));
                    spans.extend(self.theme.dim_key_badge_default(&macro_key));
                    spans.push(Span::styled(" macros.", desc_style));
                }
                Line::from(spans)
            } else if scrollback_offset > 0 {
                let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
                let mut spans: Vec<Span> = Vec::new();
                spans.push(Span::styled(
                    format!("Scrolled back {scrollback_offset} lines. "),
                    Style::default().fg(self.theme.hint_key_fg),
                ));
                spans.extend(self.theme.dim_key_badge_default(&scroll_down));
                spans.push(Span::styled(" down, ", desc_style));
                spans.extend(self.theme.dim_key_badge_default(&scroll_up));
                spans.push(Span::styled(" up, ", desc_style));
                spans.extend(self.theme.dim_key_badge_default(&scroll_line));
                spans.push(Span::styled(" one line.", desc_style));
                Line::from(spans)
            } else {
                let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
                let mut spans: Vec<Span> = Vec::new();
                if matches!(active_surface, SessionSurface::Terminal) {
                    match terminal_status {
                        CompanionTerminalStatus::Running => {
                            spans.push(Span::styled(
                                "Companion terminal is running. Hidden terminals stay alive in this worktree.",
                                desc_style,
                            ));
                        }
                        CompanionTerminalStatus::Exited => {
                            spans.push(Span::styled(
                                "Companion terminal exited. Relaunch it explicitly to start a fresh shell.",
                                desc_style,
                            ));
                        }
                        CompanionTerminalStatus::NotLaunched => {
                            spans.push(Span::styled(
                                "Companion terminal is not launched yet. Launch it explicitly when needed.",
                                desc_style,
                            ));
                        }
                    }
                } else if session_active && nudge_active {
                    let warn_style = Style::default().fg(self.theme.nudge_border);
                    spans.push(Span::styled(
                        "Read-only \u{2014} agent needs full keyboard control. ",
                        warn_style,
                    ));
                    spans.extend(self.theme.dim_key_badge_default(&focus_agent));
                    spans.push(Span::styled(" to interact.", desc_style));
                } else if session_active {
                    spans.extend(self.theme.dim_key_badge_default(&focus_agent));
                    spans.push(Span::styled(" to interact. ", desc_style));
                    spans.extend(self.theme.dim_key_badge_default(&scroll_up));
                    spans.push(Span::styled(" ", desc_style));
                    spans.extend(self.theme.dim_key_badge_default(&scroll_down));
                    spans.push(Span::styled(" to scroll. ", desc_style));
                    spans.extend(self.theme.dim_key_badge_default(&scroll_line));
                    spans.push(Span::styled(" one line.", desc_style));
                } else if session_id.is_some() {
                    spans.push(Span::styled("Agent CLI exited. Press ", desc_style));
                    spans.extend(self.theme.dim_key_badge_default(&reconnect));
                    spans.push(Span::styled(" to relaunch or ", desc_style));
                    spans.extend(self.theme.dim_key_badge_default(&focus_agent));
                    spans.push(Span::styled(" to interact.", desc_style));
                } else {
                    spans.push(Span::styled("No agent selected.", desc_style));
                }
                Line::from(spans)
            };
            Paragraph::new(hint_line)
                .block(
                    Block::default()
                        .borders(Borders::TOP)
                        .border_style(Style::default().fg(self.theme.border_normal)),
                )
                .render(hint_area, frame.buffer_mut());
        }
    }

    fn render_files(&mut self, frame: &mut Frame, area: Rect) {
        if self.right_hidden {
            return;
        }

        if self.right_collapsed {
            let focused = self.focus == FocusPane::Files;
            let all_files: Vec<(&str, Color)> = self
                .engine
                .unstaged_files
                .iter()
                .chain(self.engine.staged_files.iter())
                .map(|f| (f.status.as_str(), self.theme.file_status_fg))
                .collect();
            let items: Vec<ListItem> = all_files
                .iter()
                .map(|(s, color)| {
                    ListItem::new(Line::from(Span::styled(
                        s.to_string(),
                        Style::default().fg(*color),
                    )))
                })
                .collect();
            let mut state = ListState::default().with_selected(Some(self.files_index));
            StatefulWidget::render(
                List::new(items)
                    .block(self.themed_block("", focused))
                    .highlight_style(self.theme.selection_style()),
                area,
                frame.buffer_mut(),
                &mut state,
            );
            return;
        }

        let has_staged = !self.engine.staged_files.is_empty();
        let focused = self.focus == FocusPane::Files;

        if has_staged {
            let pct = self.staged_pane_height_pct.clamp(10, 80);
            let unstaged_pct = 100u16.saturating_sub(pct).max(20);
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Percentage(unstaged_pct), // Changes (unstaged) — always on top
                    Constraint::Percentage(pct),          // Staged Changes (with commit input)
                ])
                .split(area);
            let list_rect = self.render_file_list(
                frame,
                chunks[0],
                "Changes",
                &self.engine.unstaged_files,
                RightSection::Unstaged,
                true,
            );
            self.mouse_layout.unstaged_list = Some(list_rect);
            self.render_staged_with_commit(frame, chunks[1], focused);
        } else {
            let list_rect = self.render_file_list(
                frame,
                area,
                "Changes",
                &self.engine.unstaged_files,
                RightSection::Unstaged,
                true,
            );
            self.mouse_layout.unstaged_list = Some(list_rect);
        }
    }

    /// Render the "Staged Changes" file list and the commit input as two
    /// separate bordered blocks (bubbles).
    fn render_staged_with_commit(&mut self, frame: &mut Frame, area: Rect, pane_focused: bool) {
        let commit_pct = self.commit_pane_height_pct.clamp(10, 80);
        let staged_pct = 100u16.saturating_sub(commit_pct).max(20);
        let [files_area, commit_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(staged_pct),
                Constraint::Percentage(commit_pct),
            ])
            .areas(area);

        // Staged files — normal titled block.
        let list_rect = self.render_file_list(
            frame,
            files_area,
            "Staged Changes",
            &self.engine.staged_files,
            RightSection::Staged,
            false,
        );
        self.mouse_layout.staged_list = Some(list_rect);

        // Commit input block.
        self.render_commit_input_inner(frame, commit_area, pane_focused);
    }

    /// Render a file list inside a bordered block and return the inner `Rect`
    /// where file rows were actually placed.  Callers store this in
    /// `mouse_layout` so that mouse-hit detection matches the real rendering.
    fn render_file_list(
        &self,
        frame: &mut Frame,
        area: Rect,
        title_prefix: &str,
        files: &[ChangedFile],
        section: RightSection,
        show_hint: bool,
    ) -> Rect {
        let pane_focused = self.focus == FocusPane::Files;
        let is_active_section = pane_focused && self.right_section == section;
        let title = format!("{title_prefix} ({})", files.len());
        let block = self.themed_block(&title, is_active_section);
        let inner = block.inner(area);
        block.render(area, frame.buffer_mut());

        let show_search =
            is_active_section && (self.files_search_active || self.has_files_search());
        let (search_area, list_inner) = if show_search && inner.height >= 4 {
            let [sa, la] = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(2), Constraint::Min(1)])
                .areas(inner);
            (Some(sa), la)
        } else {
            (None, inner)
        };

        // Optionally reserve 2 lines at the bottom for the hint bar (border + text).
        let (list_area, hint_area) = if show_hint && pane_focused && list_inner.height >= 4 {
            let [la, ha] = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(1), Constraint::Length(2)])
                .areas(list_inner);
            (la, Some(ha))
        } else {
            (list_inner, None)
        };

        if let Some(search_area) = search_area {
            let query = format!("/ {}", self.files_search.text);
            Paragraph::new(query)
                .block(
                    Block::default()
                        .borders(Borders::BOTTOM)
                        .border_style(Style::default().fg(self.theme.border_normal)),
                )
                .render(search_area, frame.buffer_mut());
        }

        let inner_width = list_area.width as usize;
        // Reserve a one-column right margin so the stats don't butt against the
        // border, mirroring the ~1-column left inset from the status prefix.
        let content_width = inner_width.saturating_sub(1);
        let sel_style = self.theme.selection_style();
        let items = files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                let is_selected = is_active_section && index == self.files_index;

                // Build the right-aligned stats string, e.g. "+12 -3".
                let stats =
                    format_line_stats(file.additions, file.deletions, file.binary, &self.theme);
                let stats_width = stats.iter().map(|s| s.width()).sum::<usize>();

                // Status prefix takes 3 chars ("M  ").
                let prefix_width = 3;
                // Leave 1 char gap between path and stats.
                let path_budget = content_width
                    .saturating_sub(prefix_width)
                    .saturating_sub(stats_width)
                    .saturating_sub(1);

                let path = if is_selected {
                    file.path.clone()
                } else {
                    git::ellipsize_middle(&file.path, path_budget.max(10))
                };

                let path_display_width = path.chars().count();
                let padding = content_width
                    .saturating_sub(prefix_width)
                    .saturating_sub(path_display_width)
                    .saturating_sub(stats_width);

                let base_style = if is_selected {
                    sel_style
                } else {
                    Style::default()
                };

                let mut spans = vec![
                    Span::styled(
                        format!("{:>2} ", file.status),
                        base_style.fg(self.theme.file_status_fg),
                    ),
                    Span::styled(path, base_style),
                    Span::styled(" ".repeat(padding), base_style),
                ];
                // For stats spans, keep their green/red fg but apply selection bg when selected.
                let stats_base = if is_selected {
                    Style::default()
                        .bg(self.theme.selection_bg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                spans.extend(stats.into_iter().map(|s| {
                    let fg = s.style.fg.unwrap_or(Color::Reset);
                    Span::styled(s.content, stats_base.fg(fg))
                }));
                ListItem::new(Line::from(spans))
            })
            .collect::<Vec<_>>();
        let selected = if is_active_section {
            Some(self.files_index)
        } else {
            None
        };
        let mut state = ListState::default().with_selected(selected);
        StatefulWidget::render(List::new(items), list_area, frame.buffer_mut(), &mut state);

        // Hint bar inside the block (same style as agent terminal / diff view).
        if let Some(ha) = hint_area {
            let stage_key = self.bindings.label_for(Action::StageUnstage);
            let search_key = self.bindings.label_for(Action::SearchFiles);
            let next_key = self.bindings.label_for(Action::SearchNext);
            let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
            let mut spans: Vec<Span> = Vec::new();
            spans.extend(self.theme.dim_key_badge_default(&stage_key));
            spans.push(Span::styled(" stage/unstage.", desc_style));
            spans.push(Span::raw("  "));
            if self.files_search_active {
                spans.extend(self.theme.dim_key_badge_default("Enter"));
                spans.push(Span::styled(" done  ", desc_style));
                spans.extend(self.theme.dim_key_badge_default("Esc"));
                spans.push(Span::styled(" clear", desc_style));
            } else {
                spans.extend(self.theme.dim_key_badge_default(&search_key));
                spans.push(Span::styled(" search", desc_style));
                if self.has_files_search() {
                    spans.push(Span::raw("  "));
                    spans.extend(self.theme.dim_key_badge_default(&next_key));
                    spans.push(Span::styled(" next match", desc_style));
                }
            }
            Paragraph::new(Line::from(spans))
                .block(
                    Block::default()
                        .borders(Borders::TOP)
                        .border_style(Style::default().fg(self.theme.border_normal)),
                )
                .render(ha, frame.buffer_mut());
        }

        list_area
    }

    /// Render the commit input as its own bordered block.
    fn render_commit_input_inner(&mut self, frame: &mut Frame, area: Rect, pane_focused: bool) {
        self.mouse_layout.commit_area = Some(area);
        let is_active_section = pane_focused && self.right_section == RightSection::CommitInput;
        let focused = self.input_target == InputTarget::CommitMessage;

        let block = self.themed_block("Commit Message", is_active_section || focused);
        let inner = block.inner(area);
        block.render(area, frame.buffer_mut());

        // Reserve 1 line at the bottom for the hint bar.
        let [text_area, hint_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .areas(inner);
        self.mouse_layout.commit_text_area = Some(text_area);

        // Update TextInput's display dimensions to match the available area.
        let text_w = text_area.width as usize;
        self.commit_input
            .set_display_width(if text_w > 0 { Some(text_w) } else { None });
        self.commit_input
            .set_visible_lines(text_area.height as usize);

        if self.commit_input.is_empty() && !focused {
            // Show placeholder when unfocused and empty — nothing to render
            // (the placeholder is shown only when focused, below).
        } else {
            // Render visible lines from TextInput (handles wrapping + scroll).
            let visible = self.commit_input.visible_lines();
            let (cursor_row, cursor_col) = self.commit_input.cursor_display_position();
            let is_empty = self.commit_input.is_empty();

            // When empty and focused, show the placeholder.
            if is_empty {
                if let Some(ph) = self.commit_input.placeholder() {
                    Paragraph::new(ph.to_string())
                        .style(Style::default().fg(self.theme.hint_desc_fg))
                        .render(text_area, frame.buffer_mut());
                }
            } else {
                for (i, line_text) in visible.iter().enumerate() {
                    if i >= text_area.height as usize {
                        break;
                    }
                    let y = text_area.y + i as u16;
                    let line_area = Rect::new(text_area.x, y, text_area.width, 1);
                    Paragraph::new(line_text.as_str()).render(line_area, frame.buffer_mut());
                }
            }

            // Position the hardware cursor when focused.
            if focused && !is_empty {
                let cx = text_area.x + cursor_col as u16;
                let cy = text_area.y + cursor_row as u16;
                if cx < text_area.x + text_area.width && cy < text_area.y + text_area.height {
                    frame.set_cursor_position((cx, cy));
                }
            }
        }

        // Hint bar.
        if hint_area.height > 0 {
            let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
            let exit = self.bindings.labels_for(Action::ExitCommitInput);
            let engage = self.bindings.labels_for(Action::EngageCommitInput);
            let commit = self.bindings.label_for(Action::CommitChanges);
            let mut spans: Vec<Span> = Vec::new();
            if focused {
                spans.extend(self.theme.dim_key_badge_default(&exit));
                spans.push(Span::styled(" Exit", desc_style));
            } else {
                spans.extend(self.theme.dim_key_badge_default(&engage));
                spans.push(Span::styled(" Edit  ", desc_style));
                spans.extend(self.theme.dim_key_badge_default(&commit));
                spans.push(Span::styled(" Commit", desc_style));
            }
            Paragraph::new(Line::from(spans)).render(hint_area, frame.buffer_mut());
        }
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        let is_on_project = !matches!(
            self.left_items().get(self.selected_left),
            Some(LeftItem::Session(_))
        );
        let ctx = match self.focus {
            FocusPane::Left if self.left_section == LeftSection::Terminals => {
                HintContext::LeftTerminal
            }
            FocusPane::Left if is_on_project => HintContext::LeftProject,
            FocusPane::Left => HintContext::LeftSession,
            FocusPane::Center => HintContext::Center,
            FocusPane::Files => HintContext::Files,
        };
        let hints = self.footer_hints_for(ctx);
        let [hints_area, status_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .areas(area);
        let max_w = hints_area.width as usize;
        let ellipsis = "…";
        let ellipsis_w = 1;

        let mut hint_spans: Vec<Span> = Vec::new();
        let bar_bg = self.theme.hint_bar_bg;
        let mut used = 0usize;
        for (i, (key, desc)) in hints.iter().enumerate() {
            // width of this hint: separator + <key> + space + desc
            let sep = if i > 0 { 1 } else { 0 };
            let hint_w = sep + key.len() + 2 + 1 + desc.len();
            if used + hint_w > max_w {
                if used + ellipsis_w <= max_w {
                    hint_spans.push(Span::styled(
                        ellipsis,
                        Style::default().fg(self.theme.hint_desc_fg).bg(bar_bg),
                    ));
                }
                break;
            }
            if i > 0 {
                hint_spans.push(Span::styled(" ", Style::default().bg(bar_bg)));
            }
            hint_spans.extend(self.theme.key_badge(key, bar_bg));
            hint_spans.push(Span::styled(
                format!(" {desc}"),
                Style::default().fg(self.theme.hint_desc_fg).bg(bar_bg),
            ));
            used += hint_w;
        }

        Paragraph::new(Line::from(hint_spans))
            .style(Style::default().bg(self.theme.hint_bar_bg))
            .render(hints_area, frame.buffer_mut());

        let (tone, status_text) = self
            .status
            .most_recent_tui()
            .unwrap_or((StatusTone::Info, String::new()));
        let (dot, dot_color) = self.theme.status_dot(tone);
        let msg_color = match tone {
            StatusTone::Info => self.theme.status_info_fg,
            StatusTone::Busy => self.theme.status_busy_fg,
            StatusTone::Warning => self.theme.warning_fg,
            StatusTone::Error => self.theme.status_error_fg,
        };
        let status_bg = match tone {
            StatusTone::Info => self.theme.status_info_bg,
            StatusTone::Busy => self.theme.status_busy_bg,
            StatusTone::Warning => self.theme.status_info_bg,
            StatusTone::Error => self.theme.status_error_bg,
        };
        let prefix = format!(" {dot} ");
        let prefix_w = prefix.chars().count();
        let max_status_chars = (status_area.width as usize) * (status_area.height as usize);
        let available = max_status_chars.saturating_sub(prefix_w);
        let truncated = truncate_status_text(&status_text, available);
        let status_line = Line::from(vec![
            Span::styled(prefix, Style::default().fg(dot_color).bg(status_bg)),
            Span::styled(truncated, Style::default().fg(msg_color).bg(status_bg)),
        ]);
        Paragraph::new(status_line)
            .style(Style::default().bg(status_bg))
            .wrap(Wrap { trim: false })
            .render(status_area, frame.buffer_mut());
    }

    pub(crate) fn footer_hints_for(&self, ctx: HintContext) -> Vec<(String, &'static str)> {
        let mut hints = self.bindings.hints_for(ctx);
        if matches!(ctx, HintContext::Center) && self.current_pr_info().is_some() {
            let key = self.bindings.label_for(Action::OpenCurrentPullRequest);
            if !key.is_empty() {
                hints.insert(0, (key, "PR"));
            }
        }
        hints
    }

    fn render_help(&mut self, frame: &mut Frame) {
        self.render_dim_overlay(frame);
        let area = centered_rect(72, 70, frame.area());
        self.clear_overlay_area(frame, area);

        let outer_block = self.themed_overlay_block("Help");
        let inner = outer_block.inner(area);
        outer_block.render(area, frame.buffer_mut());

        if inner.height < 3 || inner.width < 4 {
            return;
        }

        let hint_height = 2;
        let [content_area, hint_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(hint_height)])
            .areas(inner);
        self.overlay_layout.active = OverlayMouseLayout::Help;

        // Build help content lines.
        let mut lines: Vec<Line> = Vec::new();
        let content_width = content_area.width as usize;

        let banner_style = Style::default()
            .fg(self.theme.help_banner_fg)
            .bg(self.theme.help_banner_bg)
            .add_modifier(Modifier::BOLD);
        let body_style = Style::default().fg(self.theme.help_body_fg);

        // Helper: push a full-width banner line.
        let push_banner = |lines: &mut Vec<Line>, title: &str, width: usize| {
            let padding = width.saturating_sub(title.chars().count() + 3);
            let text = format!(" {title}{}", " ".repeat(padding));
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled(text, banner_style)));
            lines.push(Line::from(""));
        };

        // ── About dux ──────────────────────────────────────────
        push_banner(&mut lines, "About dux", content_width);
        lines.push(Line::from(Span::styled(
            "dux is a terminal UI for orchestrating AI coding agents.",
            body_style,
        )));
        lines.push(Line::from(Span::styled(
            "Each project maps to a git worktree, and you can spawn",
            body_style,
        )));
        lines.push(Line::from(Span::styled(
            "unlimited agents — and unlimited companion terminals for",
            body_style,
        )));
        lines.push(Line::from(Span::styled(
            "each agent — all running side by side.",
            body_style,
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Any CLI tool can be a provider: Claude, Codex, Copilot,",
            body_style,
        )));
        lines.push(Line::from(Span::styled(
            "OpenCode, or anything else you configure.",
            body_style,
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Every keybinding shown below is fully rebindable.",
            body_style,
        )));
        lines.push(Line::from(vec![
            Span::styled(
                "Your config file is self-documented — open it and explore: ",
                body_style,
            ),
            Span::styled(
                self.engine.paths.config_path.display().to_string(),
                Style::default()
                    .fg(self.theme.hint_key_fg)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));

        // ── Keybindings ─────────────────────────────────────────
        push_banner(&mut lines, "Keybindings", content_width);

        let help_bindings = self.bindings.help_sections();
        for (i, (section, bindings)) in help_bindings.iter().enumerate() {
            if i > 0 {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled(
                section.to_string(),
                Style::default()
                    .fg(self.theme.help_section_header_fg)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )));
            for (key, desc) in bindings {
                let padding = 14usize.saturating_sub(key.len() + 2);
                let mut spans = vec![Span::raw("  ")];
                spans.extend(self.theme.key_badge_default(key));
                spans.push(Span::raw(" ".repeat(padding)));
                spans.push(Span::styled(
                    desc.to_string(),
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                lines.push(Line::from(spans));
            }
        }

        // ── Reference ───────────────────────────────────────────
        push_banner(&mut lines, "Reference", content_width);

        // Key notation legend
        lines.push(Line::from(Span::styled(
            "Key notation",
            Style::default()
                .fg(self.theme.help_section_header_fg)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )));
        {
            let key = "Ctrl-x";
            let desc = "Hold Ctrl and press X (e.g. Ctrl-p)";
            let padding = 14usize.saturating_sub(key.len() + 2);
            let mut spans = vec![Span::raw("  ")];
            spans.extend(self.theme.key_badge_default(key));
            spans.push(Span::raw(" ".repeat(padding)));
            spans.push(Span::styled(
                desc,
                Style::default().fg(self.theme.hint_desc_fg),
            ));
            lines.push(Line::from(spans));
        }
        // Session state legend
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Session states",
            Style::default()
                .fg(self.theme.help_section_header_fg)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )));
        let session_states: &[(&str, Color, &str)] = &[
            ("●", self.theme.session_active, "Active — agent is running"),
            (
                "◎",
                self.theme.session_detached,
                "Detached — agent process disconnected",
            ),
            (
                "○",
                self.theme.session_exited,
                "Exited — agent has finished",
            ),
        ];
        for (dot, color, desc) in session_states {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(*dot, Style::default().fg(*color)),
                Span::raw("  "),
                Span::styled(
                    desc.to_string(),
                    Style::default().fg(self.theme.hint_desc_fg),
                ),
            ]));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Companion terminal states",
            Style::default()
                .fg(self.theme.help_section_header_fg)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )));
        for status in [
            CompanionTerminalStatus::NotLaunched,
            CompanionTerminalStatus::Running,
            CompanionTerminalStatus::Exited,
        ] {
            let (icon, label) = companion_terminal_status_meta(status);
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    icon,
                    Style::default().fg(companion_terminal_status_color(&self.theme, status)),
                ),
                Span::raw("  "),
                Span::styled(
                    match status {
                        CompanionTerminalStatus::NotLaunched => {
                            format!("{label} — shell has not been started")
                        }
                        CompanionTerminalStatus::Running => {
                            format!("{label} — companion shell is alive")
                        }
                        CompanionTerminalStatus::Exited => {
                            format!("{label} — shell finished and awaits relaunch")
                        }
                    },
                    Style::default().fg(self.theme.hint_desc_fg),
                ),
            ]));
        }

        // GitHub integration status.
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "GitHub integration",
            Style::default()
                .fg(self.theme.help_section_header_fg)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )));
        {
            use crate::model::GhStatus;
            let (icon, desc) = if !self.engine.github_integration_enabled {
                (
                    "○",
                    "Disabled — enable via command palette (toggle-github-integration)".to_string(),
                )
            } else {
                match self.engine.gh_status {
                    GhStatus::Unknown => ("◐", "Checking gh CLI availability…".to_string()),
                    GhStatus::NotInstalled => (
                        "⚠",
                        "gh CLI not found — install from https://cli.github.com".to_string(),
                    ),
                    GhStatus::NotAuthenticated => (
                        "⚠",
                        "gh CLI not authenticated — run: gh auth login".to_string(),
                    ),
                    GhStatus::Available => {
                        let count = self.engine.pr_statuses.len();
                        let noun = if count == 1 { "session" } else { "sessions" };
                        ("✓", format!("Active — tracking PRs for {count} {noun}"))
                    }
                }
            };
            let icon_color = match self.engine.gh_status {
                GhStatus::Available if self.engine.github_integration_enabled => {
                    self.theme.session_active
                }
                _ if !self.engine.github_integration_enabled => self.theme.session_exited,
                _ => self.theme.warning_fg,
            };
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(icon, Style::default().fg(icon_color)),
                Span::raw("  "),
                Span::styled(desc, Style::default().fg(self.theme.hint_desc_fg)),
            ]));
        }

        // Track content size for scroll clamping in input handler.
        let total_lines = lines.len() as u16;
        self.last_help_lines = total_lines;
        self.last_help_height = content_area.height;

        // Clamp scroll offset.
        let max_scroll = total_lines.saturating_sub(content_area.height);
        let scroll = self.help_scroll.unwrap_or(0).min(max_scroll);

        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll, 0))
            .render(content_area, frame.buffer_mut());

        // Hint bar with top border (same pattern as diff view).
        if hint_area.height > 0 {
            let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
            let scroll_down = self.bindings.labels_for(Action::ScrollPageDown);
            let scroll_up = self.bindings.labels_for(Action::ScrollPageUp);
            let move_down = self.bindings.label_for(Action::MoveDown);
            let move_up = self.bindings.label_for(Action::MoveUp);
            let close = self.bindings.label_for(Action::CloseOverlay);
            let mut spans: Vec<Span> = Vec::new();

            if scroll > 0 {
                spans.push(Span::styled(
                    format!("Scrolled back {scroll} lines. "),
                    Style::default().fg(self.theme.hint_key_fg),
                ));
            }
            spans.extend(self.theme.dim_key_badge_default(&move_down));
            spans.push(Span::styled(" ", desc_style));
            spans.extend(self.theme.dim_key_badge_default(&move_up));
            spans.push(Span::styled(" or ", desc_style));
            spans.extend(self.theme.dim_key_badge_default("Space"));
            spans.push(Span::styled(" scroll, ", desc_style));
            spans.extend(self.theme.dim_key_badge_default(&scroll_down));
            spans.push(Span::styled(" ", desc_style));
            spans.extend(self.theme.dim_key_badge_default(&scroll_up));
            spans.push(Span::styled(" page. ", desc_style));
            spans.extend(self.theme.dim_key_badge_default(&close));
            spans.push(Span::styled(" close.", desc_style));

            Paragraph::new(Line::from(spans))
                .block(
                    Block::default()
                        .borders(Borders::TOP)
                        .border_style(Style::default().fg(self.theme.border_normal)),
                )
                .render(hint_area, frame.buffer_mut());
        }
    }

    fn render_prompt(&mut self, frame: &mut Frame) {
        match &self.prompt {
            PromptState::Command { input, selected } => {
                self.render_dim_overlay(frame);
                let popup = centered_rect(72, 40, frame.area());
                self.clear_overlay_area(frame, popup);
                let commands = self.filtered_palette_commands(&input.text);
                let items = if commands.is_empty() {
                    vec![ListItem::new("No matching commands.")]
                } else {
                    let name_col = commands
                        .iter()
                        .map(|b| b.palette_name.unwrap().len())
                        .max()
                        .unwrap_or(0);
                    let inner_w = popup.width as usize - 3;
                    let gap = 2usize;
                    commands
                        .iter()
                        .map(|binding| {
                            let name = binding.palette_name.unwrap();
                            let name_padded = format!("{name:name_col$}");
                            let mut spans = vec![Span::styled(
                                name_padded,
                                Style::default()
                                    .fg(self.theme.help_section_header_fg)
                                    .add_modifier(Modifier::BOLD),
                            )];
                            let desc_avail = inner_w.saturating_sub(name_col + gap);
                            let desc = binding.palette_description.unwrap_or("");
                            let desc_display =
                                if desc.chars().count() > desc_avail && desc_avail > 1 {
                                    let end: String = desc.chars().take(desc_avail - 1).collect();
                                    format!("  {end}\u{2026}")
                                } else {
                                    format!("  {desc:desc_avail$}")
                                };
                            spans.push(Span::styled(
                                desc_display,
                                Style::default().fg(self.theme.hint_desc_fg),
                            ));
                            ListItem::new(Line::from(spans))
                        })
                        .collect::<Vec<_>>()
                };
                let mut state = ListState::default()
                    .with_selected(Some((*selected).min(commands.len().saturating_sub(1))));
                let [input_area, list_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(3), Constraint::Min(3)])
                    .areas(popup);
                let title = "Command Palette";
                let confirm_key = self.bindings.label_for(Action::Confirm);
                let close_key = self.bindings.label_for(Action::CloseOverlay);
                let mut bottom_spans = vec![Span::raw(" ")];
                bottom_spans.extend(self.theme.key_badge_default(&confirm_key));
                bottom_spans.push(Span::styled(
                    " run  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                // Tab autocomplete is text-input behavior, not a rebindable action.
                bottom_spans.extend(self.theme.key_badge_default("Tab"));
                bottom_spans.push(Span::styled(
                    " complete  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&close_key));
                bottom_spans.push(Span::styled(
                    " cancel",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                let prompt_prefix = "> ";
                let input_block = self
                    .themed_overlay_block(title)
                    .title_bottom(Line::from(bottom_spans));
                let input_inner = input_block.inner(input_area);
                Paragraph::new(render_single_line_cursor_input(
                    prompt_prefix,
                    &input.text,
                    input.cursor,
                    self.theme.input_cursor_fg,
                    self.theme.input_cursor_bg,
                ))
                .block(input_block)
                .render(input_area, frame.buffer_mut());
                let list_block = Block::default()
                    .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                    .border_type(ratatui::widgets::BorderType::Rounded)
                    .border_style(Style::default().fg(self.theme.overlay_border))
                    .style(Style::default().bg(self.theme.overlay_bg));
                let list_inner = list_block.inner(list_area);
                StatefulWidget::render(
                    List::new(items)
                        .block(list_block)
                        .highlight_style(self.theme.selection_style()),
                    list_area,
                    frame.buffer_mut(),
                    &mut state,
                );
                self.overlay_layout.active = OverlayMouseLayout::Command {
                    input: input_inner,
                    list: list_inner,
                    items: commands.len(),
                    offset: state.offset(),
                };
            }
            PromptState::BrowseProjects {
                current_dir,
                entries,
                loading,
                selected,
                filter,
                searching,
                editing_path,
                path_input,
                tab_completions,
                tab_index,
            } => {
                self.render_dim_overlay(frame);
                let area = centered_rect(72, 70, frame.area());
                self.clear_overlay_area(frame, area);
                let visible: Vec<_> = if filter.is_empty() {
                    entries.iter().collect()
                } else {
                    let needle = filter.text.to_lowercase();
                    entries
                        .iter()
                        .filter(|e| e.label.to_lowercase().contains(&needle))
                        .collect()
                };
                let completion_items = tab_completions
                    .iter()
                    .map(|completion| {
                        ListItem::new(Line::from(vec![Span::styled(
                            path_completion_display_label(completion),
                            Style::default().fg(self.theme.text_fg),
                        )]))
                    })
                    .collect::<Vec<_>>();
                let items = if *editing_path {
                    if completion_items.is_empty() {
                        vec![ListItem::new("No matching directories.")]
                    } else {
                        completion_items
                    }
                } else if *loading {
                    let idx = self.spinner_frame_index();
                    let spinner = crate::theme::SPINNER_FRAMES[idx];
                    vec![ListItem::new(Line::from(vec![
                        Span::styled(
                            format!("{spinner} "),
                            Style::default().fg(self.theme.hint_desc_fg),
                        ),
                        Span::styled("Loading…", Style::default().fg(self.theme.text_fg)),
                    ]))]
                } else if visible.is_empty() {
                    vec![ListItem::new(if filter.is_empty() {
                        "No child directories here."
                    } else {
                        "No matching entries."
                    })]
                } else {
                    let last = visible.len() - 1;
                    visible
                        .iter()
                        .enumerate()
                        .map(|(i, entry)| {
                            let prefix = if entry.is_parent {
                                ""
                            } else if i == last {
                                "└── "
                            } else {
                                "├── "
                            };
                            ListItem::new(Line::from(vec![
                                Span::styled(
                                    prefix.to_string(),
                                    Style::default().fg(self.theme.hint_desc_fg),
                                ),
                                Span::styled(
                                    entry.label.clone(),
                                    Style::default().fg(self.theme.text_fg),
                                ),
                            ]))
                        })
                        .collect::<Vec<_>>()
                };
                let item_count = if *editing_path {
                    tab_completions.len()
                } else {
                    visible.len()
                };
                let selected_index = if *editing_path { *tab_index } else { *selected };
                let mut state = ListState::default()
                    .with_selected(Some(selected_index.min(item_count.saturating_sub(1))));
                let has_filter = !filter.is_empty();
                let show_top_input = *searching || has_filter || *editing_path;
                let (top_areas, list_render_area) = if show_top_input {
                    let [filter_area, list_area] = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([Constraint::Length(3), Constraint::Min(3)])
                        .areas(area);
                    (Some(filter_area), list_area)
                } else {
                    (None, area)
                };
                if let Some(filter_area) = top_areas {
                    let title = format!("Add Project: {}", current_dir.display());
                    let (prefix, text, cursor) = if *editing_path {
                        ("go: ", path_input.text.as_str(), path_input.cursor)
                    } else {
                        ("/ ", filter.text.as_str(), filter.cursor)
                    };
                    let input_block = self.themed_overlay_block(&title);
                    let input_inner = input_block.inner(filter_area);
                    Paragraph::new(render_single_line_cursor_input(
                        prefix,
                        text,
                        cursor,
                        self.theme.input_cursor_fg,
                        self.theme.input_cursor_bg,
                    ))
                    .block(input_block)
                    .render(filter_area, frame.buffer_mut());
                    let confirm_key = self.bindings.label_for(Action::Confirm);
                    let close_key = self.bindings.label_for(Action::CloseOverlay);
                    let search_key = self.bindings.label_for(Action::SearchToggle);
                    let open_key = self.bindings.label_for(Action::OpenEntry);
                    let goto_key = self.bindings.label_for(Action::GoToPath);
                    let exit_path_key = self.bindings.label_for(Action::ExitPathEditorOnProjectAdd);
                    let mut bottom_spans = vec![Span::raw(" ")];
                    if *editing_path {
                        // Path editor: Tab/Enter are text-input controls, not rebindable.
                        bottom_spans.extend(self.theme.key_badge_default("Tab"));
                        bottom_spans.push(Span::styled(
                            " complete  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge_default("Enter"));
                        bottom_spans.push(Span::styled(
                            " add  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge_default(&exit_path_key));
                        bottom_spans.push(Span::styled(
                            " browse",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                    } else if *searching {
                        bottom_spans.extend(self.theme.key_badge_default(&confirm_key));
                        bottom_spans.push(Span::styled(
                            " done  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge_default(&close_key));
                        bottom_spans.push(Span::styled(
                            " clear",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                    } else {
                        bottom_spans.extend(self.theme.key_badge_default(&search_key));
                        bottom_spans.push(Span::styled(
                            " search  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge_default(&open_key));
                        bottom_spans.push(Span::styled(
                            " open  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge_default(&goto_key));
                        bottom_spans.push(Span::styled(
                            " go to  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge_default(&close_key));
                        bottom_spans.push(Span::styled(
                            " cancel",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                    }
                    let list_block = Block::default()
                        .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                        .border_style(Style::default().fg(self.theme.overlay_border))
                        .style(Style::default().bg(self.theme.overlay_bg))
                        .title_bottom(Line::from(bottom_spans));
                    let list_inner = list_block.inner(list_render_area);
                    StatefulWidget::render(
                        List::new(items)
                            .block(list_block)
                            .highlight_style(self.theme.selection_style()),
                        list_render_area,
                        frame.buffer_mut(),
                        &mut state,
                    );
                    self.overlay_layout.active = OverlayMouseLayout::BrowseProjects {
                        input: Some(input_inner),
                        list: list_inner,
                        items: item_count,
                        offset: state.offset(),
                    };
                } else {
                    let search_key = self.bindings.label_for(Action::SearchToggle);
                    let open_key = self.bindings.label_for(Action::OpenEntry);
                    let add_key = self.bindings.label_for(Action::AddCurrentDir);
                    let goto_key = self.bindings.label_for(Action::GoToPath);
                    let close_key = self.bindings.label_for(Action::CloseOverlay);
                    let mut bottom_spans = vec![Span::raw(" ")];
                    bottom_spans.extend(self.theme.key_badge_default(&search_key));
                    bottom_spans.push(Span::styled(
                        " search  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge_default(&open_key));
                    bottom_spans.push(Span::styled(
                        " open  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge_default(&add_key));
                    bottom_spans.push(Span::styled(
                        " add current  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge_default(&goto_key));
                    bottom_spans.push(Span::styled(
                        " go to  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge_default(&close_key));
                    bottom_spans.push(Span::styled(
                        " cancel",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    let title = format!("Add Project: {}", current_dir.display());
                    let list_block = self
                        .themed_overlay_block(&title)
                        .title_bottom(Line::from(bottom_spans));
                    let list_inner = list_block.inner(list_render_area);
                    StatefulWidget::render(
                        List::new(items)
                            .block(list_block)
                            .highlight_style(self.theme.selection_style()),
                        list_render_area,
                        frame.buffer_mut(),
                        &mut state,
                    );
                    self.overlay_layout.active = OverlayMouseLayout::BrowseProjects {
                        input: None,
                        list: list_inner,
                        items: item_count,
                        offset: state.offset(),
                    };
                }
            }
            PromptState::ChangeAgentProvider(prompt) => {
                self.render_dim_overlay(frame);
                let area = centered_rect(72, 42, frame.area());
                self.clear_overlay_area(frame, area);

                let move_down = self.bindings.label_for(Action::MoveDown);
                let move_up = self.bindings.label_for(Action::MoveUp);
                let toggle_key = self.bindings.label_for(Action::ToggleSelection);
                let confirm_key = self.bindings.label_for(Action::Confirm);
                let close_key = self.bindings.label_for(Action::CloseOverlay);

                let mut bottom_spans = vec![Span::raw(" ")];
                bottom_spans.extend(self.theme.key_badge_default(&move_down));
                bottom_spans.push(Span::styled(
                    " down  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&move_up));
                bottom_spans.push(Span::styled(
                    " up  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&toggle_key));
                bottom_spans.push(Span::styled(
                    " buttons  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&confirm_key));
                bottom_spans.push(Span::styled(
                    " choose  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&close_key));
                bottom_spans.push(Span::styled(
                    " cancel",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));

                let [details_area, list_area, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(4),
                        Constraint::Min(6),
                        Constraint::Length(3),
                    ])
                    .areas(area);

                let detail_lines = vec![
                    Line::from(vec![
                        Span::styled(" Agent: ", Style::default().fg(self.theme.hint_desc_fg)),
                        Span::styled(
                            prompt.session_label.as_str(),
                            Style::default()
                                .fg(self.theme.text_fg)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled(" Path: ", Style::default().fg(self.theme.hint_desc_fg)),
                        Span::styled(
                            prompt.worktree_path.as_str(),
                            Style::default().fg(self.theme.text_fg),
                        ),
                    ]),
                ];
                let overlay_title = match prompt.mode {
                    ChangeAgentProviderMode::Retarget => "Change Agent Provider",
                    ChangeAgentProviderMode::NewTab => "New Tab Provider",
                };
                Paragraph::new(detail_lines)
                    .block(
                        self.themed_overlay_block(overlay_title)
                            .title_bottom(Line::from(bottom_spans)),
                    )
                    .render(details_area, frame.buffer_mut());

                let provider_col = prompt
                    .options
                    .iter()
                    .map(|option| option.provider.as_str().chars().count())
                    .max()
                    .unwrap_or(0)
                    .max(8);
                let items = prompt
                    .options
                    .iter()
                    .map(|option| {
                        // In NewTab mode a tab always launches fresh (create_tab
                        // never resumes), so the retarget-only resume/current
                        // language would be misleading here.
                        let status = if prompt.mode == ChangeAgentProviderMode::NewTab {
                            "will start a fresh tab"
                        } else if option.is_current {
                            "current"
                        } else if option.resume_available {
                            "a previous session was found; it'll be continued"
                        } else if !option.supports_resume {
                            "this provider doesn't support resume; it'll start fresh every time"
                        } else {
                            "no prior session; will start fresh"
                        };
                        let name =
                            format!("{:width$}", option.provider.as_str(), width = provider_col);
                        ListItem::new(Line::from(vec![
                            Span::styled(
                                name,
                                Style::default()
                                    .fg(self.theme.help_section_header_fg)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("  {status}"),
                                Style::default().fg(self.theme.hint_desc_fg),
                            ),
                        ]))
                    })
                    .collect::<Vec<_>>();
                let mut state = ListState::default().with_selected(Some(
                    prompt.selected.min(prompt.options.len().saturating_sub(1)),
                ));
                let list_block = Block::default()
                    .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                    .border_style(Style::default().fg(self.theme.overlay_border))
                    .style(Style::default().bg(self.theme.overlay_bg));
                let list_inner = list_block.inner(list_area);
                let highlight_style = if matches!(prompt.focus, ChangeAgentProviderFocus::List) {
                    self.theme.selection_style()
                } else {
                    Style::default()
                        .fg(self.theme.help_section_header_fg)
                        .add_modifier(Modifier::BOLD)
                };
                StatefulWidget::render(
                    List::new(items)
                        .block(list_block)
                        .highlight_style(highlight_style),
                    list_area,
                    frame.buffer_mut(),
                    &mut state,
                );

                let btn_width = 18u16;
                let gap = 2u16;
                let total = btn_width * 2 + gap;
                let left_offset = buttons_area.width.saturating_sub(total) / 2;
                let cancel_area = Rect {
                    x: buttons_area.x + left_offset,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };
                let apply_area = Rect {
                    x: cancel_area.x + btn_width + gap,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                // In NewTab mode there is no existing tab to compare against,
                // so `is_current` (which reflects the session's overall
                // provider, not the new tab) never gates the Apply button.
                let apply_enabled = prompt.mode == ChangeAgentProviderMode::NewTab
                    || prompt
                        .options
                        .get(prompt.selected)
                        .map(|option| !option.is_current)
                        .unwrap_or(false);

                Button::new("Cancel")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ChangeAgentProviderCancel,
                        self.pressed_button,
                        matches!(prompt.focus, ChangeAgentProviderFocus::Cancel),
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new("Use Provider")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ChangeAgentProviderApply,
                        self.pressed_button,
                        matches!(prompt.focus, ChangeAgentProviderFocus::Apply),
                        apply_enabled,
                    ))
                    .render(frame, apply_area, &self.theme);

                self.overlay_layout.active = OverlayMouseLayout::ChangeAgentProvider {
                    list: list_inner,
                    items: prompt.options.len(),
                    offset: state.offset(),
                    cancel_button: cancel_area,
                    apply_button: apply_area,
                };
            }
            PromptState::ChangeDefaultProvider(prompt) => {
                self.render_dim_overlay(frame);
                let area = centered_rect(72, 42, frame.area());
                self.clear_overlay_area(frame, area);

                let move_down = self.bindings.label_for(Action::MoveDown);
                let move_up = self.bindings.label_for(Action::MoveUp);
                let toggle_key = self.bindings.label_for(Action::ToggleSelection);
                let confirm_key = self.bindings.label_for(Action::Confirm);
                let close_key = self.bindings.label_for(Action::CloseOverlay);

                let mut bottom_spans = vec![Span::raw(" ")];
                bottom_spans.extend(self.theme.key_badge_default(&move_down));
                bottom_spans.push(Span::styled(
                    " down  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&move_up));
                bottom_spans.push(Span::styled(
                    " up  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&toggle_key));
                bottom_spans.push(Span::styled(
                    " buttons  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&confirm_key));
                bottom_spans.push(Span::styled(
                    " choose  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&close_key));
                bottom_spans.push(Span::styled(
                    " cancel",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));

                let [details_area, list_area, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(4),
                        Constraint::Min(6),
                        Constraint::Length(3),
                    ])
                    .areas(area);

                let detail_lines = vec![
                    Line::from(vec![
                        Span::styled(
                            " Current global default: ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ),
                        Span::styled(
                            prompt.current.as_str().to_string(),
                            Style::default()
                                .fg(self.theme.text_fg)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(vec![Span::styled(
                        " Projects with an explicit project provider keep their override. Existing agents keep their current provider.",
                        Style::default().fg(self.theme.hint_desc_fg),
                    )]),
                ];
                Paragraph::new(detail_lines)
                    .block(
                        self.themed_overlay_block("Change Default Provider")
                            .title_bottom(Line::from(bottom_spans)),
                    )
                    .render(details_area, frame.buffer_mut());

                let provider_col = prompt
                    .options
                    .iter()
                    .map(|option| option.provider.as_str().chars().count())
                    .max()
                    .unwrap_or(0)
                    .max(8);
                let items = prompt
                    .options
                    .iter()
                    .map(|option| {
                        let status = if option.is_current {
                            "current global default"
                        } else {
                            "available"
                        };
                        let name =
                            format!("{:width$}", option.provider.as_str(), width = provider_col);
                        ListItem::new(Line::from(vec![
                            Span::styled(
                                name,
                                Style::default()
                                    .fg(self.theme.help_section_header_fg)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("  {status}"),
                                Style::default().fg(self.theme.hint_desc_fg),
                            ),
                        ]))
                    })
                    .collect::<Vec<_>>();
                let mut state = ListState::default().with_selected(Some(
                    prompt.selected.min(prompt.options.len().saturating_sub(1)),
                ));
                let list_block = Block::default()
                    .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                    .border_style(Style::default().fg(self.theme.overlay_border))
                    .style(Style::default().bg(self.theme.overlay_bg));
                let list_inner = list_block.inner(list_area);
                let highlight_style = if matches!(prompt.focus, ChangeDefaultProviderFocus::List) {
                    self.theme.selection_style()
                } else {
                    Style::default()
                        .fg(self.theme.help_section_header_fg)
                        .add_modifier(Modifier::BOLD)
                };
                StatefulWidget::render(
                    List::new(items)
                        .block(list_block)
                        .highlight_style(highlight_style),
                    list_area,
                    frame.buffer_mut(),
                    &mut state,
                );

                let btn_width = 18u16;
                let gap = 2u16;
                let total = btn_width * 2 + gap;
                let left_offset = buttons_area.width.saturating_sub(total) / 2;
                let cancel_area = Rect {
                    x: buttons_area.x + left_offset,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };
                let apply_area = Rect {
                    x: cancel_area.x + btn_width + gap,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                let apply_enabled = prompt
                    .options
                    .get(prompt.selected)
                    .map(|option| !option.is_current)
                    .unwrap_or(false);

                Button::new("Cancel")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ChangeDefaultProviderCancel,
                        self.pressed_button,
                        matches!(prompt.focus, ChangeDefaultProviderFocus::Cancel),
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new("Set Global")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ChangeDefaultProviderApply,
                        self.pressed_button,
                        matches!(prompt.focus, ChangeDefaultProviderFocus::Apply),
                        apply_enabled,
                    ))
                    .render(frame, apply_area, &self.theme);

                self.overlay_layout.active = OverlayMouseLayout::ChangeDefaultProvider {
                    list: list_inner,
                    items: prompt.options.len(),
                    offset: state.offset(),
                    cancel_button: cancel_area,
                    apply_button: apply_area,
                };
            }
            PromptState::ChangeProjectDefaultProvider(prompt) => {
                self.render_dim_overlay(frame);
                let area = centered_rect(64, 60, frame.area());
                self.clear_overlay_area(frame, area);

                let move_down = self.bindings.label_for(Action::MoveDown);
                let move_up = self.bindings.label_for(Action::MoveUp);
                let toggle = self.bindings.label_for(Action::ToggleSelection);
                let confirm = self.bindings.label_for(Action::Confirm);
                let close_key = self.bindings.label_for(Action::CloseOverlay);

                let mut bottom_spans = vec![Span::raw(" ")];
                bottom_spans.extend(self.theme.key_badge_default(&move_down));
                bottom_spans.push(Span::styled(
                    "/",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&move_up));
                bottom_spans.push(Span::styled(
                    " move  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&toggle));
                bottom_spans.push(Span::styled(
                    " buttons  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&confirm));
                bottom_spans.push(Span::styled(
                    " choose  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&close_key));
                bottom_spans.push(Span::styled(
                    " cancel",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));

                let [details_area, list_area, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(5),
                        Constraint::Min(6),
                        Constraint::Length(3),
                    ])
                    .areas(area);

                let project_mode = if prompt.inherits_global_default {
                    "inherits global default"
                } else {
                    "project override"
                };
                let detail_lines = vec![
                    Line::from(vec![
                        Span::styled(" Project: ", Style::default().fg(self.theme.hint_desc_fg)),
                        Span::styled(
                            prompt.project_name.clone(),
                            Style::default()
                                .fg(self.theme.text_fg)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled(
                            " Current provider: ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ),
                        Span::styled(
                            format!("{} ({project_mode})", prompt.current.as_str()),
                            Style::default()
                                .fg(self.theme.text_fg)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled(
                            " Global default: ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ),
                        Span::styled(
                            prompt.global_default.as_str().to_string(),
                            Style::default()
                                .fg(self.theme.text_fg)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(vec![Span::styled(
                        " Choose \"inherit global default\" to remove the project-specific override. Existing agents keep their current provider.",
                        Style::default().fg(self.theme.hint_desc_fg),
                    )]),
                ];
                Paragraph::new(detail_lines)
                    .block(
                        self.themed_overlay_block("Change Project Provider")
                            .title_bottom(Line::from(bottom_spans)),
                    )
                    .render(details_area, frame.buffer_mut());

                let provider_col = prompt
                    .options
                    .iter()
                    .map(|option| match &option.provider {
                        Some(provider) => provider.as_str().chars().count(),
                        None => "inherit global default".chars().count(),
                    })
                    .max()
                    .unwrap_or(0)
                    .max(8);
                let items = prompt
                    .options
                    .iter()
                    .map(|option| {
                        let name = match &option.provider {
                            Some(provider) => provider.as_str().to_string(),
                            None => "inherit global default".to_string(),
                        };
                        let status = match (&option.provider, option.is_current) {
                            (None, true) => "current setting",
                            (None, false) => "remove project override",
                            (Some(_), true) => "current project override",
                            (Some(provider), false)
                                if provider.as_str() == prompt.global_default.as_str() =>
                            {
                                "matches global default"
                            }
                            _ => "available",
                        };
                        let name = format!("{name:width$}", width = provider_col);
                        ListItem::new(Line::from(vec![
                            Span::styled(
                                name,
                                Style::default()
                                    .fg(self.theme.help_section_header_fg)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("  {status}"),
                                Style::default().fg(self.theme.hint_desc_fg),
                            ),
                        ]))
                    })
                    .collect::<Vec<_>>();
                let mut state = ListState::default().with_selected(Some(
                    prompt.selected.min(prompt.options.len().saturating_sub(1)),
                ));
                let list_block = Block::default()
                    .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                    .border_style(Style::default().fg(self.theme.overlay_border))
                    .style(Style::default().bg(self.theme.overlay_bg));
                let list_inner = list_block.inner(list_area);
                let highlight_style = if matches!(prompt.focus, ChangeDefaultProviderFocus::List) {
                    self.theme.selection_style()
                } else {
                    Style::default()
                        .fg(self.theme.help_section_header_fg)
                        .add_modifier(Modifier::BOLD)
                };
                StatefulWidget::render(
                    List::new(items)
                        .block(list_block)
                        .highlight_style(highlight_style),
                    list_area,
                    frame.buffer_mut(),
                    &mut state,
                );

                let btn_width = 18u16;
                let gap = 2u16;
                let total = btn_width * 2 + gap;
                let left_offset = buttons_area.width.saturating_sub(total) / 2;
                let cancel_area = Rect {
                    x: buttons_area.x + left_offset,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };
                let apply_area = Rect {
                    x: cancel_area.x + btn_width + gap,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                let apply_enabled = prompt
                    .options
                    .get(prompt.selected)
                    .map(|option| !option.is_current)
                    .unwrap_or(false);

                Button::new("Cancel")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ChangeProjectDefaultProviderCancel,
                        self.pressed_button,
                        matches!(prompt.focus, ChangeDefaultProviderFocus::Cancel),
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new("Set Project")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ChangeProjectDefaultProviderApply,
                        self.pressed_button,
                        matches!(prompt.focus, ChangeDefaultProviderFocus::Apply),
                        apply_enabled,
                    ))
                    .render(frame, apply_area, &self.theme);

                self.overlay_layout.active = OverlayMouseLayout::ChangeProjectDefaultProvider {
                    list: list_inner,
                    items: prompt.options.len(),
                    offset: state.offset(),
                    cancel_button: cancel_area,
                    apply_button: apply_area,
                };
            }
            PromptState::ChangeTheme(prompt) => {
                self.render_dim_overlay(frame);
                let area = centered_rect(60, 60, frame.area());
                self.clear_overlay_area(frame, area);

                let move_down = self.bindings.label_for(Action::MoveDown);
                let move_up = self.bindings.label_for(Action::MoveUp);
                let confirm_key = self.bindings.label_for(Action::Confirm);
                let close_key = self.bindings.label_for(Action::CloseOverlay);

                let mut bottom_spans = vec![Span::raw(" ")];
                bottom_spans.extend(self.theme.key_badge_default(&move_down));
                bottom_spans.push(Span::styled(
                    " down  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&move_up));
                bottom_spans.push(Span::styled(
                    " up  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&confirm_key));
                bottom_spans.push(Span::styled(
                    " apply  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&close_key));
                bottom_spans.push(Span::styled(
                    " cancel",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));

                let [details_area, list_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(4), Constraint::Min(6)])
                    .areas(area);

                let detail_lines = vec![
                    Line::from(vec![
                        Span::styled(
                            " Current theme: ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ),
                        Span::styled(
                            prompt.current.clone(),
                            Style::default()
                                .fg(self.theme.text_fg)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(vec![Span::styled(
                        " Selecting a theme applies it instantly and saves it to config.toml.",
                        Style::default().fg(self.theme.hint_desc_fg),
                    )]),
                ];
                Paragraph::new(detail_lines)
                    .block(
                        self.themed_overlay_block("Change Theme")
                            .title_bottom(Line::from(bottom_spans)),
                    )
                    .render(details_area, frame.buffer_mut());

                let id_col = prompt
                    .options
                    .iter()
                    .map(|option| option.id.chars().count())
                    .max()
                    .unwrap_or(0)
                    .max(8);
                let items = prompt
                    .options
                    .iter()
                    .map(|option| {
                        let source_label = match option.source {
                            crate::theme::ThemeSource::Bundled => "bundled",
                            crate::theme::ThemeSource::Opaline => "opaline",
                            crate::theme::ThemeSource::User => "user",
                        };
                        let is_current = option.id == prompt.current;
                        let suffix = if is_current {
                            format!("  {source_label} · current")
                        } else {
                            format!("  {source_label}")
                        };
                        let id_padded = format!("{:width$}", option.id, width = id_col);
                        ListItem::new(Line::from(vec![
                            Span::styled(
                                id_padded,
                                Style::default()
                                    .fg(self.theme.help_section_header_fg)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                format!("  {}", option.display_name),
                                Style::default().fg(self.theme.hint_desc_fg),
                            ),
                            Span::styled(suffix, Style::default().fg(self.theme.hint_dim_desc_fg)),
                        ]))
                    })
                    .collect::<Vec<_>>();
                let mut state = ListState::default().with_selected(Some(
                    prompt.selected.min(prompt.options.len().saturating_sub(1)),
                ));
                let list_block = Block::default()
                    .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                    .border_style(Style::default().fg(self.theme.overlay_border))
                    .style(Style::default().bg(self.theme.overlay_bg));
                let list_inner = list_block.inner(list_area);
                StatefulWidget::render(
                    List::new(items)
                        .block(list_block)
                        .highlight_style(self.theme.selection_style()),
                    list_area,
                    frame.buffer_mut(),
                    &mut state,
                );

                self.overlay_layout.active = OverlayMouseLayout::ChangeTheme {
                    list: list_inner,
                    items: prompt.options.len(),
                    offset: state.offset(),
                };
            }
            PromptState::ConfigureStartupCommand {
                project_name,
                input,
                ..
            }
            | PromptState::ConfigureProjectEnv {
                project_name,
                input,
                ..
            }
            | PromptState::ConfigureGlobalEnv {
                project_name,
                input,
                ..
            } => {
                let is_env = matches!(
                    &self.prompt,
                    PromptState::ConfigureProjectEnv { .. }
                        | PromptState::ConfigureGlobalEnv { .. }
                );
                let is_global_env = matches!(&self.prompt, PromptState::ConfigureGlobalEnv { .. });
                self.render_dim_overlay(frame);
                let area = centered_rect_exact(76, if is_env { 18 } else { 16 }, frame.area());
                self.clear_overlay_area(frame, area);
                let outer = self.themed_overlay_block(if is_global_env {
                    "Configure Global Environment"
                } else if is_env {
                    "Configure Project Environment"
                } else {
                    "Configure Startup Command"
                });
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());
                let [label_area, input_area, hint_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(2),
                        Constraint::Min(3),
                        Constraint::Length(1),
                    ])
                    .areas(inner);
                Paragraph::new(vec![
                    Line::from(vec![
                        Span::styled(
                            if is_global_env {
                                " Scope: "
                            } else {
                                " Project: "
                            },
                            Style::default().fg(self.theme.input_label_fg),
                        ),
                        Span::styled(
                            project_name.clone(),
                            Style::default().fg(self.theme.input_label_fg),
                        ),
                    ]),
                    Line::from(Span::styled(
                        if is_env {
                            " Enter one variable per line as KEY=value:"
                        } else {
                            " Enter a command to run before the provider launches:"
                        },
                        Style::default().fg(self.theme.input_label_fg),
                    )),
                ])
                .render(label_area, frame.buffer_mut());

                let focused = self.input_target == InputTarget::StartupCommand;
                let input_block = Block::default()
                    .borders(Borders::ALL)
                    .border_set(border::ROUNDED)
                    .border_style(Style::default().fg(if focused {
                        self.theme.border_focused
                    } else {
                        self.theme.overlay_border
                    }));
                let text_area = input_block.inner(input_area);
                input_block.render(input_area, frame.buffer_mut());

                let mut render_input = input.clone();
                render_input
                    .set_display_width((text_area.width > 0).then_some(text_area.width as usize));
                render_input.set_visible_lines(text_area.height as usize);
                let visible = render_input.visible_lines();
                let is_empty = render_input.is_empty();
                if is_empty {
                    if let Some(placeholder) = render_input.placeholder() {
                        Paragraph::new(placeholder.to_string())
                            .style(Style::default().fg(self.theme.hint_desc_fg))
                            .render(text_area, frame.buffer_mut());
                    }
                } else {
                    for (index, line_text) in visible.iter().enumerate() {
                        if index >= text_area.height as usize {
                            break;
                        }
                        let line_area =
                            Rect::new(text_area.x, text_area.y + index as u16, text_area.width, 1);
                        Paragraph::new(line_text.as_str()).render(line_area, frame.buffer_mut());
                    }
                }
                if focused {
                    let (cursor_row, cursor_col) = render_input.cursor_display_position();
                    let cx = text_area.x + cursor_col as u16;
                    let cy = text_area.y + cursor_row as u16;
                    if cx < text_area.x + text_area.width && cy < text_area.y + text_area.height {
                        frame.set_cursor_position((cx, cy));
                    }
                }

                let confirm_key = self.bindings.label_for(Action::Confirm);
                let close_key = self.bindings.label_for(Action::CloseOverlay);
                let edit_key = self.bindings.label_for(Action::EngageCommitInput);
                let exit_key = self.bindings.labels_for(Action::ExitCommitInput);
                let clear_key = "Ctrl-d";
                let mut hints = vec![Span::raw(" ")];
                if focused {
                    hints.extend(self.theme.key_badge_default(&exit_key));
                    hints.push(Span::styled(
                        " exit edit  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    hints.extend(self.theme.key_badge_default(clear_key));
                    hints.push(Span::styled(
                        " clear",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                } else {
                    hints.extend(self.theme.key_badge_default(&edit_key));
                    hints.push(Span::styled(
                        " edit  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    hints.extend(self.theme.key_badge_default(&confirm_key));
                    hints.push(Span::styled(
                        " save  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    hints.extend(self.theme.key_badge_default(clear_key));
                    hints.push(Span::styled(
                        " clear  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    hints.extend(self.theme.key_badge_default(&close_key));
                    hints.push(Span::styled(
                        " cancel",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                }
                Paragraph::new(Line::from(hints)).render(hint_area, frame.buffer_mut());
                self.overlay_layout.active =
                    OverlayMouseLayout::ConfigureStartupCommand { input: text_area };
            }
            PromptState::StartupCommandLogs(prompt) => {
                self.render_dim_overlay(frame);
                let area = centered_rect(92, 82, frame.area());
                self.clear_overlay_area(frame, area);
                let close_key = self.bindings.label_for(Action::CloseOverlay);
                let mut bottom_spans = vec![Span::raw(" ")];
                let move_keys = self.bindings.labels_for(Action::MoveDown);
                let search_key = self.bindings.label_for(Action::SearchToggle);
                let open_file_key = self.bindings.label_for(Action::OpenStartupCommandLogFile);
                let open_folder_key = self.bindings.label_for(Action::OpenStartupCommandLogFolder);
                bottom_spans.extend(self.theme.key_badge_default(&move_keys));
                bottom_spans.push(Span::styled(
                    " logs  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&search_key));
                bottom_spans.push(Span::styled(
                    " search  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default("PgUp/PgDn"));
                bottom_spans.push(Span::styled(
                    " scroll  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&open_file_key));
                bottom_spans.push(Span::styled(
                    " Open file  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&open_folder_key));
                bottom_spans.push(Span::styled(
                    " Open folder  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&close_key));
                bottom_spans.push(Span::styled(
                    " close",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));

                let title = format!("Startup Command Logs - {}", prompt.scope_label);
                let block = self
                    .themed_overlay_block(&title)
                    .title_bottom(Line::from(bottom_spans))
                    .border_style(Style::default().fg(self.theme.overlay_border));
                let inner = block.inner(area);
                block.render(area, frame.buffer_mut());
                let [content_area, button_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(6), Constraint::Length(3)])
                    .areas(inner);
                let [left_area, _, body_area] = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Length(34),
                        Constraint::Length(1),
                        Constraint::Min(20),
                    ])
                    .areas(content_area);
                let (filter_area, list_area) = if prompt.searching || !prompt.filter.is_empty() {
                    let [filter_area, list_area] = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([Constraint::Length(3), Constraint::Min(1)])
                        .areas(left_area);
                    (Some(filter_area), list_area)
                } else {
                    (None, left_area)
                };
                let visible_indices = Self::startup_command_log_filtered_indices(prompt);
                let items = if prompt.entries.is_empty() {
                    vec![ListItem::new("No logs")]
                } else if visible_indices.is_empty() {
                    vec![ListItem::new("No matching logs")]
                } else {
                    visible_indices
                        .iter()
                        .filter_map(|index| prompt.entries.get(*index))
                        .map(|entry| {
                            let modified = entry
                                .modified_at
                                .map(|ts| ts.format("%Y-%m-%d %H:%M:%S").to_string())
                                .unwrap_or_else(|| "unknown time".to_string());
                            ListItem::new(vec![
                                Line::from(Span::styled(
                                    entry.display_name.clone(),
                                    Style::default()
                                        .fg(self.theme.help_section_header_fg)
                                        .add_modifier(Modifier::BOLD),
                                )),
                                Line::from(Span::styled(
                                    modified,
                                    Style::default().fg(self.theme.hint_desc_fg),
                                )),
                            ])
                        })
                        .collect::<Vec<_>>()
                };
                let selected_visual =
                    Self::startup_command_log_selected_visual_index(prompt, &visible_indices);
                let mut state = ListState::default().with_selected(selected_visual);
                if let Some(filter_area) = filter_area {
                    let filter_block = Block::default()
                        .title(" Filter ")
                        .borders(Borders::ALL)
                        .border_set(border::ROUNDED)
                        .border_style(Style::default().fg(if prompt.searching {
                            self.theme.border_focused
                        } else {
                            self.theme.overlay_border
                        }))
                        .style(Style::default().bg(self.theme.overlay_bg));
                    let filter_inner = filter_block.inner(filter_area);
                    filter_block.render(filter_area, frame.buffer_mut());
                    let text = if prompt.filter.is_empty() && prompt.searching {
                        "type to filter logs"
                    } else {
                        prompt.filter.text.as_str()
                    };
                    Paragraph::new(text)
                        .style(Style::default().fg(if prompt.filter.is_empty() {
                            self.theme.hint_desc_fg
                        } else {
                            self.theme.text_fg
                        }))
                        .render(filter_inner, frame.buffer_mut());
                    if prompt.searching {
                        let cursor_x = filter_inner
                            .x
                            .saturating_add(prompt.filter.cursor as u16)
                            .min(filter_inner.x + filter_inner.width.saturating_sub(1));
                        frame.set_cursor_position((cursor_x, filter_inner.y));
                    }
                }
                let list_block = Block::default()
                    .title(" Runs ")
                    .borders(Borders::ALL)
                    .border_set(border::ROUNDED)
                    .border_style(Style::default().fg(self.theme.overlay_border))
                    .style(Style::default().bg(self.theme.overlay_bg));
                let list_inner = list_block.inner(list_area);
                StatefulWidget::render(
                    List::new(items)
                        .block(list_block)
                        .highlight_style(self.theme.selection_style()),
                    list_area,
                    frame.buffer_mut(),
                    &mut state,
                );
                let body_block = Block::default()
                    .title(" Output ")
                    .borders(Borders::ALL)
                    .border_set(border::ROUNDED)
                    .border_style(Style::default().fg(self.theme.overlay_border))
                    .style(Style::default().bg(self.theme.overlay_bg));
                let body_inner = body_block.inner(body_area);
                body_block.render(body_area, frame.buffer_mut());
                let content_lines = crate::app::input::startup_command_log_visual_lines(
                    &prompt.content,
                    body_inner.width,
                );
                let max_scroll = u16::try_from(content_lines.len())
                    .unwrap_or(u16::MAX)
                    .saturating_sub(body_inner.height);
                let scroll_offset = prompt.scroll_offset.min(max_scroll);
                for (display_row, line) in content_lines
                    .iter()
                    .skip(scroll_offset as usize)
                    .take(body_inner.height as usize)
                    .enumerate()
                {
                    let y = body_inner.y + display_row as u16;
                    for (display_col, ch) in
                        line.chars().take(body_inner.width as usize).enumerate()
                    {
                        let x = body_inner.x + display_col as u16;
                        let selected =
                            self.startup_log_selection
                                .as_ref()
                                .is_some_and(|selection| {
                                    selection.anchor != selection.end
                                        && selection.contains(
                                            scroll_offset + display_row as u16,
                                            display_col as u16,
                                        )
                                });
                        let style = if selected {
                            self.theme.selection_style()
                        } else {
                            Style::default().fg(self.theme.text_fg)
                        };
                        frame.buffer_mut().set_string(x, y, ch.to_string(), style);
                    }
                }

                let close_width = 16;
                let close_area = Rect {
                    x: button_area.x + button_area.width.saturating_sub(close_width) / 2,
                    y: button_area.y,
                    width: close_width.min(button_area.width),
                    height: 3,
                };
                Button::new("Close")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::StartupCommandLogsClose,
                        self.pressed_button,
                        false,
                        true,
                    ))
                    .render(frame, close_area, &self.theme);
                self.overlay_layout.active = OverlayMouseLayout::StartupCommandLogs {
                    area,
                    list: list_inner,
                    body: body_inner,
                    items: visible_indices.len(),
                    offset: state.offset(),
                    close_button: close_area,
                };
            }
            PromptState::PickEditor {
                session_label,
                worktree_path,
                editors,
                selected,
            } => {
                self.render_dim_overlay(frame);
                let area = centered_rect(64, 34, frame.area());
                self.clear_overlay_area(frame, area);

                let confirm_key = self.bindings.label_for(Action::Confirm);
                let close_key = self.bindings.label_for(Action::CloseOverlay);
                let move_down = self.bindings.label_for(Action::MoveDown);
                let move_up = self.bindings.label_for(Action::MoveUp);
                let mut bottom_spans = vec![Span::raw(" ")];
                bottom_spans.extend(self.theme.key_badge_default(&move_down));
                bottom_spans.push(Span::styled(
                    " down  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&move_up));
                bottom_spans.push(Span::styled(
                    " up  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&confirm_key));
                bottom_spans.push(Span::styled(
                    " open  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&close_key));
                bottom_spans.push(Span::styled(
                    " cancel",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));

                let [details_area, list_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(4), Constraint::Min(4)])
                    .areas(area);

                let detail_lines = vec![
                    Line::from(vec![
                        Span::styled(" Agent: ", Style::default().fg(self.theme.hint_desc_fg)),
                        Span::styled(
                            session_label.as_str(),
                            Style::default()
                                .fg(self.theme.text_fg)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled(" Path: ", Style::default().fg(self.theme.hint_desc_fg)),
                        Span::styled(
                            worktree_path.as_str(),
                            Style::default().fg(self.theme.text_fg),
                        ),
                    ]),
                ];
                Paragraph::new(detail_lines)
                    .block(
                        self.themed_overlay_block("Open Worktree In")
                            .title_bottom(Line::from(bottom_spans)),
                    )
                    .render(details_area, frame.buffer_mut());

                let configured_default = self.engine.config.editor.default.trim();
                let items = editors
                    .iter()
                    .map(|editor| {
                        let mut spans = vec![Span::styled(
                            format!("{:<14}", editor.label),
                            Style::default()
                                .fg(self.theme.help_section_header_fg)
                                .add_modifier(Modifier::BOLD),
                        )];
                        spans.push(Span::styled(
                            format!(" {}", editor.command),
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        if crate::editor::matches_configured_editor(editor, configured_default) {
                            spans.push(Span::styled(
                                "  default",
                                Style::default().fg(self.theme.branch_fg),
                            ));
                        }
                        ListItem::new(Line::from(spans))
                    })
                    .collect::<Vec<_>>();
                let mut state = ListState::default()
                    .with_selected(Some((*selected).min(editors.len().saturating_sub(1))));
                let list_block = Block::default()
                    .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                    .border_style(Style::default().fg(self.theme.overlay_border))
                    .style(Style::default().bg(self.theme.overlay_bg));
                let list_inner = list_block.inner(list_area);
                StatefulWidget::render(
                    List::new(items)
                        .block(list_block)
                        .highlight_style(self.theme.selection_style()),
                    list_area,
                    frame.buffer_mut(),
                    &mut state,
                );
                self.overlay_layout.active = OverlayMouseLayout::PickEditor {
                    list: list_inner,
                    items: editors.len(),
                    offset: state.offset(),
                };
            }
            PromptState::PickProjectWorktree(prompt) => {
                self.render_dim_overlay(frame);
                let area = centered_rect(78, 58, frame.area());
                self.clear_overlay_area(frame, area);

                let confirm_key = self.bindings.label_for(Action::Confirm);
                let close_key = self.bindings.label_for(Action::CloseOverlay);
                let move_down = self.bindings.label_for(Action::MoveDown);
                let move_up = self.bindings.label_for(Action::MoveUp);
                let mut bottom_spans = vec![Span::raw(" ")];
                bottom_spans.extend(self.theme.key_badge_default(&move_down));
                bottom_spans.push(Span::styled(
                    " down  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&move_up));
                bottom_spans.push(Span::styled(
                    " up  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&confirm_key));
                bottom_spans.push(Span::styled(
                    " use  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&close_key));
                bottom_spans.push(Span::styled(
                    " cancel",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));

                let [details_area, list_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(4), Constraint::Min(6)])
                    .areas(area);

                let detail_lines = vec![
                    Line::from(vec![
                        Span::styled(" Project: ", Style::default().fg(self.theme.hint_desc_fg)),
                        Span::styled(
                            prompt.project.name.as_str(),
                            Style::default()
                                .fg(self.theme.text_fg)
                                .add_modifier(Modifier::BOLD),
                        ),
                    ]),
                    Line::from(vec![
                        Span::styled(" Repo: ", Style::default().fg(self.theme.hint_desc_fg)),
                        Span::styled(
                            prompt.project.path.as_str(),
                            Style::default().fg(self.theme.text_fg),
                        ),
                    ]),
                ];
                Paragraph::new(detail_lines)
                    .block(
                        self.themed_overlay_block("New Agent From Worktree")
                            .title_bottom(Line::from(bottom_spans)),
                    )
                    .render(details_area, frame.buffer_mut());

                let rows = project_worktree_visual_rows(
                    &prompt.entries,
                    prompt.loading,
                    prompt.error.as_deref(),
                );
                let path_col = prompt
                    .entries
                    .iter()
                    .map(|entry| entry.display_name().chars().count())
                    .max()
                    .unwrap_or(8)
                    .clamp(8, 24);
                let items = rows
                    .iter()
                    .map(|row| match row {
                        ProjectWorktreeVisualRow::Header(label) => {
                            ListItem::new(Line::from(Span::styled(
                                format!(" {label}"),
                                Style::default()
                                    .fg(self.theme.help_section_header_fg)
                                    .add_modifier(Modifier::BOLD),
                            )))
                        }
                        ProjectWorktreeVisualRow::Empty(message) => {
                            ListItem::new(Line::from(Span::styled(
                                format!("  {message}"),
                                Style::default().fg(self.theme.hint_dim_desc_fg),
                            )))
                        }
                        ProjectWorktreeVisualRow::Entry(index) => {
                            let entry = &prompt.entries[*index];
                            let style = if entry.is_selectable {
                                Style::default().fg(self.theme.text_fg)
                            } else {
                                Style::default().fg(self.theme.hint_dim_desc_fg)
                            };
                            let kind = if entry.is_project_checkout {
                                "project"
                            } else if entry.is_external {
                                "external"
                            } else {
                                "managed"
                            };
                            let session_suffix = entry
                                .existing_session_id
                                .as_ref()
                                .map(|id| format!("  agent {id}"))
                                .unwrap_or_default();
                            let kind_style = if !entry.is_selectable {
                                Style::default().fg(self.theme.hint_dim_desc_fg)
                            } else if entry.is_managed_by_dux {
                                Style::default().fg(self.theme.branch_fg)
                            } else {
                                Style::default().fg(self.theme.hint_desc_fg)
                            };
                            let branch_label_style = Style::default().fg(if entry.is_selectable {
                                self.theme.branch_fg
                            } else {
                                self.theme.hint_dim_desc_fg
                            });
                            let branch_value_style = Style::default().fg(if entry.is_selectable {
                                self.theme.hint_desc_fg
                            } else {
                                self.theme.hint_dim_desc_fg
                            });
                            let name = git::ellipsize_middle(&entry.display_name(), path_col);
                            ListItem::new(Line::from(vec![
                                Span::styled(
                                    format!("  {:path_col$}", name),
                                    style.add_modifier(Modifier::BOLD),
                                ),
                                Span::styled(format!("  {:<8}", kind), kind_style),
                                Span::styled("  branch: ", branch_label_style),
                                Span::styled(entry.branch_name.as_str(), branch_value_style),
                                Span::styled(
                                    session_suffix,
                                    Style::default().fg(self.theme.hint_dim_desc_fg),
                                ),
                            ]))
                        }
                    })
                    .collect::<Vec<_>>();
                let selected_visual = prompt.selected.and_then(|selected| {
                    rows.iter().position(|row| {
                        matches!(row, ProjectWorktreeVisualRow::Entry(index) if *index == selected)
                    })
                });
                let mut state = ListState::default().with_selected(selected_visual);
                let list_block = Block::default()
                    .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                    .border_style(Style::default().fg(self.theme.overlay_border))
                    .style(Style::default().bg(self.theme.overlay_bg));
                let list_inner = list_block.inner(list_area);
                StatefulWidget::render(
                    List::new(items)
                        .block(list_block)
                        .highlight_style(self.theme.selection_style()),
                    list_area,
                    frame.buffer_mut(),
                    &mut state,
                );
                self.overlay_layout.active = OverlayMouseLayout::PickProjectWorktree {
                    list: list_inner,
                    items: rows.len(),
                    offset: state.offset(),
                };
            }
            PromptState::PickProject {
                intent,
                entries,
                list,
            } => {
                self.render_dim_overlay(frame);
                let area = centered_rect(72, 58, frame.area());
                self.clear_overlay_area(frame, area);

                // Which entries survive the `/` filter (indices into `entries`).
                let visible: Vec<usize> = list.visible_indices(entries, pick_project_matches);

                let confirm_key = self.bindings.label_for(Action::Confirm);
                let close_key = self.bindings.label_for(Action::CloseOverlay);
                let move_down = self.bindings.label_for(Action::MoveDown);
                let move_up = self.bindings.label_for(Action::MoveUp);
                let search_key = self.bindings.label_for(Action::SearchToggle);
                let mut bottom_spans = vec![Span::raw(" ")];
                bottom_spans.extend(self.theme.key_badge_default(&move_down));
                bottom_spans.push(Span::styled(
                    " down  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&move_up));
                bottom_spans.push(Span::styled(
                    " up  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&search_key));
                bottom_spans.push(Span::styled(
                    " search  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&confirm_key));
                bottom_spans.push(Span::styled(
                    " choose  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                bottom_spans.extend(self.theme.key_badge_default(&close_key));
                bottom_spans.push(Span::styled(
                    " cancel",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));

                let [details_area, list_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(3), Constraint::Min(4)])
                    .areas(area);

                // Header row: the `/`-search input while searching (or once a
                // query has been typed), else a plain count line.
                let details_block = self
                    .themed_overlay_block(intent.title())
                    .title_bottom(Line::from(bottom_spans));
                if list.is_filtering() {
                    Paragraph::new(render_single_line_cursor_input(
                        "/ ",
                        &list.filter.text,
                        list.filter.cursor,
                        self.theme.input_cursor_fg,
                        self.theme.input_cursor_bg,
                    ))
                    .block(details_block)
                    .render(details_area, frame.buffer_mut());
                } else {
                    Paragraph::new(vec![Line::from(vec![Span::styled(
                        format!(" Pick a project to continue ({} available).", entries.len()),
                        Style::default().fg(self.theme.hint_desc_fg),
                    )])])
                    .block(details_block)
                    .render(details_area, frame.buffer_mut());
                }

                let list_block = Block::default()
                    .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                    .border_style(Style::default().fg(self.theme.overlay_border))
                    .style(Style::default().bg(self.theme.overlay_bg));
                let list_inner = list_block.inner(list_area);

                // Column plan, sized to the visible rows and the inner width, with a
                // symmetric one-space margin inside each border. The warning glyph
                // is a fixed gutter; the path takes the remaining width and shows
                // its TAIL (leading dirs elided) so the leaf stays readable.
                let start_dir = self.engine.config.defaults.start_directory.as_deref();
                let display_path = |entry: &ProjectChooserEntry| {
                    git::display_path_relative_to(&entry.path, start_dir)
                };
                let name_col = visible
                    .iter()
                    .filter_map(|i| entries.get(*i))
                    .map(|entry| entry.name.chars().count())
                    .max()
                    .unwrap_or(8)
                    .clamp(8, 28);
                let count_label = |n: usize| match n {
                    0 => "no agents".to_string(),
                    1 => "1 agent".to_string(),
                    n => format!("{n} agents"),
                };
                let count_col = visible
                    .iter()
                    .filter_map(|i| entries.get(*i))
                    .map(|entry| count_label(entry.agent_count).chars().count())
                    .max()
                    .unwrap_or(8);
                // margin(1) warn(1) sp(1) name sp(2) count sp(2) path margin(1)
                let fixed = 1 + 1 + 1 + name_col + 2 + count_col + 2 + 1;
                let path_col = (list_inner.width as usize).saturating_sub(fixed);

                let items = visible
                    .iter()
                    .filter_map(|i| entries.get(*i))
                    .map(|entry| {
                        let warn = if entry.path_missing {
                            Span::styled("⚠", Style::default().fg(self.theme.warning_fg))
                        } else {
                            Span::raw(" ")
                        };
                        let name = git::ellipsize_middle(&entry.name, name_col);
                        let path = git::ellipsize_start(&display_path(entry), path_col);
                        ListItem::new(Line::from(vec![
                            Span::raw(" "),
                            warn,
                            Span::raw(" "),
                            Span::styled(
                                format!("{name:name_col$}"),
                                Style::default()
                                    .fg(self.theme.text_fg)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::raw("  "),
                            Span::styled(
                                format!("{:count_col$}", count_label(entry.agent_count)),
                                Style::default().fg(self.theme.hint_desc_fg),
                            ),
                            Span::raw("  "),
                            Span::styled(path, Style::default().fg(self.theme.hint_dim_desc_fg)),
                        ]))
                    })
                    .collect::<Vec<_>>();

                let mut state = ListState::default().with_selected(Some(list.selected));
                StatefulWidget::render(
                    List::new(items)
                        .block(list_block)
                        .highlight_style(self.theme.selection_style()),
                    list_area,
                    frame.buffer_mut(),
                    &mut state,
                );
                self.overlay_layout.active = OverlayMouseLayout::PickProject {
                    list: list_inner,
                    items: visible.len(),
                    offset: state.offset(),
                };
            }
            PromptState::KillRunning(prompt) => {
                self.render_dim_overlay(frame);
                let popup = centered_rect(78, 72, frame.area());
                self.clear_overlay_area(frame, popup);

                let visible_indices = Self::visible_kill_running_indices(prompt);
                let items = if visible_indices.is_empty() {
                    vec![ListItem::new("No matching running agents or terminals.")]
                } else {
                    let label_col = visible_indices
                        .iter()
                        .filter_map(|index| prompt.runtimes.get(*index))
                        .map(|runtime| runtime.label.chars().count())
                        .max()
                        .unwrap_or(0)
                        .min(28);
                    visible_indices
                        .iter()
                        .filter_map(|index| prompt.runtimes.get(*index))
                        .map(|runtime| {
                            let checkbox = Checkbox::new("")
                                .checked(prompt.selected_ids.contains(&runtime.id))
                                .state(CheckboxState::Normal);
                            let label = if runtime.label.chars().count() > label_col {
                                runtime.label.chars().take(label_col).collect::<String>()
                            } else {
                                runtime.label.clone()
                            };
                            let label_padded = format!("{label:label_col$}");
                            let kind_color = match runtime.kind {
                                KillableRuntimeKind::Agent => self.theme.session_active,
                                KillableRuntimeKind::Terminal => self.theme.session_detached,
                            };
                            let mut spans =
                                checkbox.inline_prefix(Style::default().fg(self.theme.hint_key_fg));
                            spans.extend([
                                Span::styled(
                                    format!("{:>6} ", runtime.kind.badge()),
                                    Style::default().fg(kind_color).add_modifier(Modifier::BOLD),
                                ),
                                Span::styled(
                                    label_padded,
                                    Style::default().add_modifier(Modifier::BOLD),
                                ),
                            ]);
                            spans.extend(runtime_context_spans(
                                &format!("  {}", runtime.context),
                                Style::default()
                                    .fg(self.theme.hint_dim_desc_fg)
                                    .add_modifier(Modifier::DIM),
                                Style::default().fg(self.theme.runtime_context_value_fg),
                            ));
                            ListItem::new(Line::from(spans))
                        })
                        .collect::<Vec<_>>()
                };
                let mut state = ListState::default().with_selected(Some(
                    prompt
                        .hovered_visible_index
                        .min(visible_indices.len().saturating_sub(1)),
                ));
                let show_top_input = prompt.searching || !prompt.filter.is_empty();
                let (top_area, body_area) = if show_top_input {
                    let [input_area, rest] = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([Constraint::Length(3), Constraint::Min(6)])
                        .areas(popup);
                    (Some(input_area), rest)
                } else {
                    (None, popup)
                };
                let [list_area, legend_area, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(6),
                        Constraint::Length(2),
                        Constraint::Length(3),
                    ])
                    .areas(body_area);

                let search_key = self.bindings.label_for(Action::SearchToggle);
                let toggle_key = self.bindings.label_for(Action::ToggleMarked);
                let confirm_key = self.bindings.label_for(Action::Confirm);
                let close_key = self.bindings.label_for(Action::CloseOverlay);
                let next_key = self.bindings.label_for(Action::FocusNext);
                let prev_key = self.bindings.label_for(Action::FocusPrev);
                let mut hint_spans = vec![Span::raw(" ")];
                if prompt.searching {
                    hint_spans.extend(self.theme.key_badge_default(&confirm_key));
                    hint_spans.push(Span::styled(
                        " done  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    hint_spans.extend(self.theme.key_badge_default(&close_key));
                    hint_spans.push(Span::styled(
                        " clear",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                } else {
                    hint_spans.extend(self.theme.key_badge_default(&toggle_key));
                    hint_spans.push(Span::styled(
                        " select  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    hint_spans.extend(self.theme.key_badge_default(&search_key));
                    hint_spans.push(Span::styled(
                        " search  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    hint_spans.extend(self.theme.key_badge_default(&next_key));
                    hint_spans.push(Span::styled(
                        "/",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    hint_spans.extend(self.theme.key_badge_default(&prev_key));
                    hint_spans.push(Span::styled(
                        " actions  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    hint_spans.extend(self.theme.key_badge_default(&confirm_key));
                    hint_spans.push(Span::styled(
                        " use",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                }

                let title = if prompt.searching {
                    "Kill Running (searching)"
                } else {
                    "Kill Running"
                };
                if let Some(input_area) = top_area {
                    let input_block = self.themed_overlay_block(title);
                    let input_inner = input_block.inner(input_area);
                    Paragraph::new(render_single_line_cursor_input(
                        "/ ",
                        &prompt.filter.text,
                        prompt.filter.cursor,
                        self.theme.input_cursor_fg,
                        self.theme.input_cursor_bg,
                    ))
                    .block(input_block)
                    .render(input_area, frame.buffer_mut());
                    let list_block = Block::default()
                        .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                        .border_type(ratatui::widgets::BorderType::Rounded)
                        .border_style(Style::default().fg(self.theme.overlay_border))
                        .style(Style::default().bg(self.theme.overlay_bg))
                        .title_bottom(Line::from(hint_spans));
                    let list_inner = list_block.inner(list_area);
                    StatefulWidget::render(
                        List::new(items)
                            .block(list_block)
                            .highlight_style(self.theme.selection_style()),
                        list_area,
                        frame.buffer_mut(),
                        &mut state,
                    );
                    self.overlay_layout.active = OverlayMouseLayout::KillRunning {
                        input: Some(input_inner),
                        list: list_inner,
                        items: visible_indices.len(),
                        offset: state.offset(),
                        cancel_button: Rect::default(),
                        hovered_button: Rect::default(),
                        selected_button: Rect::default(),
                        visible_button: Rect::default(),
                    };
                } else {
                    let list_block = self
                        .themed_overlay_block(title)
                        .title_bottom(Line::from(hint_spans));
                    let list_inner = list_block.inner(list_area);
                    StatefulWidget::render(
                        List::new(items)
                            .block(list_block)
                            .highlight_style(self.theme.selection_style()),
                        list_area,
                        frame.buffer_mut(),
                        &mut state,
                    );
                    self.overlay_layout.active = OverlayMouseLayout::KillRunning {
                        input: None,
                        list: list_inner,
                        items: visible_indices.len(),
                        offset: state.offset(),
                        cancel_button: Rect::default(),
                        hovered_button: Rect::default(),
                        selected_button: Rect::default(),
                        visible_button: Rect::default(),
                    };
                }

                let legend = Line::from(vec![
                    Span::raw("  "),
                    Span::styled("Legend: ", Style::default().fg(self.theme.hint_desc_fg)),
                    Span::styled(
                        "AGENT",
                        Style::default()
                            .fg(self.theme.session_active)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        " = running agent CLI  |  ",
                        Style::default().fg(self.theme.hint_dim_desc_fg),
                    ),
                    Span::styled(
                        "TERM",
                        Style::default()
                            .fg(self.theme.session_detached)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(
                        " = companion terminal  |  dim text = source context",
                        Style::default().fg(self.theme.hint_dim_desc_fg),
                    ),
                    Span::raw("  "),
                ]);
                Paragraph::new(legend)
                    .wrap(Wrap { trim: false })
                    .render(legend_area, frame.buffer_mut());

                let buttons = [
                    KillRunningFooterAction::Cancel,
                    KillRunningFooterAction::Hovered,
                    KillRunningFooterAction::Selected,
                    KillRunningFooterAction::Visible,
                ];
                let gap = 2u16;
                // This footer pre-dates the Button widget's standard sizing —
                // its labels are long ("Kill Hovered" etc.) and it lays four
                // buttons in a row. The wider per-label width (label_chars + 6)
                // keeps the row visually balanced; Button still handles the
                // colors and bold-when-enabled rules.
                let button_widths = buttons.map(|action| {
                    let label_chars =
                        u16::try_from(action.button_label().chars().count()).unwrap_or(u16::MAX);
                    label_chars.saturating_add(6)
                });
                let total_width = button_widths.iter().sum::<u16>() + gap * 3;
                let start_x = buttons_area.x + buttons_area.width.saturating_sub(total_width) / 2;
                let mut cursor_x = start_x;
                let mut button_rects = [Rect::default(); 4];
                for (index, action) in buttons.iter().enumerate() {
                    let rect = Rect {
                        x: cursor_x,
                        y: buttons_area.y,
                        width: button_widths[index],
                        height: 3,
                    };
                    button_rects[index] = rect;
                    let enabled = Self::kill_running_footer_enabled(prompt, *action);
                    let focused = matches!(
                        prompt.focus,
                        KillRunningFocus::Footer(current) if current == *action
                    );
                    let kind = if matches!(action, KillRunningFooterAction::Cancel) {
                        ButtonKind::Confirm
                    } else {
                        ButtonKind::Danger
                    };
                    let press_target = match action {
                        KillRunningFooterAction::Cancel => ButtonPressedTarget::RuntimeKillCancel,
                        KillRunningFooterAction::Hovered => ButtonPressedTarget::RuntimeKillHovered,
                        KillRunningFooterAction::Selected => {
                            ButtonPressedTarget::RuntimeKillSelected
                        }
                        KillRunningFooterAction::Visible => ButtonPressedTarget::RuntimeKillVisible,
                    };
                    let state =
                        button_state_for(press_target, self.pressed_button, focused, enabled);
                    Button::new(action.button_label())
                        .kind(kind)
                        .state(state)
                        .render(frame, rect, &self.theme);
                    cursor_x += button_widths[index] + gap;
                }
                self.overlay_layout.active = OverlayMouseLayout::KillRunning {
                    input: match self.overlay_layout.active {
                        OverlayMouseLayout::KillRunning { input, .. } => input,
                        _ => None,
                    },
                    list: match self.overlay_layout.active {
                        OverlayMouseLayout::KillRunning { list, .. } => list,
                        _ => Rect::default(),
                    },
                    items: visible_indices.len(),
                    offset: state.offset(),
                    cancel_button: button_rects[0],
                    hovered_button: button_rects[1],
                    selected_button: button_rects[2],
                    visible_button: button_rects[3],
                };
            }
            PromptState::ConfirmKillRunning(confirm_prompt) => {
                self.render_dim_overlay(frame);
                let area = centered_rect(56, 32, frame.area());
                self.clear_overlay_area(frame, area);
                let outer = self.themed_overlay_block("Confirm Kill");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                let [body_area, _, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(1),
                        Constraint::Length(1),
                        Constraint::Length(3),
                    ])
                    .areas(inner);

                let targets = confirm_prompt.target_ids.len();
                let (agent_count, terminal_count) = confirm_prompt.target_ids.iter().fold(
                    (0usize, 0usize),
                    |(agents, terminals), target_id| match target_id {
                        RuntimeTargetId::Agent(_) | RuntimeTargetId::Tab(_) => {
                            (agents + 1, terminals)
                        }
                        RuntimeTargetId::Terminal(_) => (agents, terminals + 1),
                    },
                );
                let mut summary = Vec::new();
                if agent_count > 0 {
                    summary.push(format!(
                        "{agent_count} agent{}",
                        if agent_count == 1 { "" } else { "s" }
                    ));
                }
                if terminal_count > 0 {
                    summary.push(format!(
                        "{terminal_count} terminal{}",
                        if terminal_count == 1 { "" } else { "s" }
                    ));
                }
                let lines = vec![
                    Line::from(""),
                    Line::from(vec![
                        Span::raw(format!(
                            " {} will stop ",
                            confirm_prompt.action.button_label()
                        )),
                        Span::styled(
                            format!(
                                "{targets} running process{}",
                                if targets == 1 { "" } else { "es" }
                            ),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("."),
                    ]),
                    Line::from(format!(" Affected: {}", summary.join(" and "))),
                    Line::from(""),
                    Line::from(Span::styled(
                        " In-progress CLI work will be lost immediately.",
                        Style::default().fg(self.theme.warning_fg),
                    )),
                    Line::from(Span::styled(
                        " Worktree files remain on disk for review or relaunch.",
                        Style::default().fg(self.theme.hint_desc_fg),
                    )),
                ];
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .render(body_area, frame.buffer_mut());

                let btn_width = 16u16;
                let gap = 2u16;
                let total = btn_width * 2 + gap;
                let left_offset = buttons_area.width.saturating_sub(total) / 2;

                let cancel_area = Rect {
                    x: buttons_area.x + left_offset,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };
                let kill_area = Rect {
                    x: cancel_area.x + btn_width + gap,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                Button::new("Cancel")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmKillCancel,
                        self.pressed_button,
                        !confirm_prompt.confirm_selected,
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new("Kill")
                    .kind(ButtonKind::Danger)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmKillConfirm,
                        self.pressed_button,
                        confirm_prompt.confirm_selected,
                        true,
                    ))
                    .render(frame, kill_area, &self.theme);

                self.overlay_layout.active = OverlayMouseLayout::ConfirmKillRunning {
                    cancel_button: cancel_area,
                    kill_button: kill_area,
                };
            }
            PromptState::ConfigReloadFailed {
                error,
                recover_old_config,
                focus,
            } => {
                self.render_dim_overlay(frame);
                let dialog_width = 68.min(frame.area().width.max(1));
                let inner_width = dialog_width.saturating_sub(2);
                let checkbox_label = "Recover last working config";
                let checkbox_state = if *focus == ConfigReloadFailedFocus::Checkbox {
                    CheckboxState::Focused
                } else {
                    CheckboxState::Normal
                };
                let checkbox = Checkbox::new(checkbox_label)
                    .checked(*recover_old_config)
                    .state(checkbox_state);
                let checkbox_height = checkbox
                    .layout(
                        inner_width,
                        checkbox.marker_style(Style::default()),
                        checkbox.label_style(Style::default()),
                    )
                    .height;

                let mut body_lines = vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        " config.toml could not be reloaded, so the running config was kept.",
                        Style::default().fg(self.theme.warning_fg),
                    )),
                    Line::from(""),
                    Line::from(Span::styled(
                        " Validation error:",
                        Style::default().fg(self.theme.hint_desc_fg),
                    )),
                ];
                for line in error.lines().take(6) {
                    body_lines.push(Line::from(format!(" {line}")));
                }
                let body_height = wrapped_line_count(&body_lines, inner_width, false);
                let area = centered_rect_exact(
                    dialog_width,
                    2 + body_height + 1 + checkbox_height + 3,
                    frame.area(),
                );
                self.clear_overlay_area(frame, area);
                let outer = self.themed_overlay_block("Reload Config Failed");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                let [body_area, _, checkbox_area, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(body_height),
                        Constraint::Length(1),
                        Constraint::Length(checkbox_height),
                        Constraint::Length(3),
                    ])
                    .areas(inner);

                Paragraph::new(body_lines)
                    .wrap(Wrap { trim: false })
                    .render(body_area, frame.buffer_mut());

                let (checkbox_rect, _) = self.render_overlay_checkbox(
                    frame,
                    checkbox_area,
                    checkbox_label,
                    *recover_old_config,
                    checkbox_state,
                    None,
                );
                let checkbox = OverlayCheckbox {
                    id: OverlayCheckboxId::ConfigReloadRecoverOldConfig,
                    rect: checkbox_rect,
                };

                let btn_width = shared_button_width(&["Close", "Recover"]);
                let gap = 2u16;
                let total = btn_width * 2 + gap;
                let left_offset = buttons_area.width.saturating_sub(total) / 2;

                let close_area = Rect {
                    x: buttons_area.x + left_offset,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };
                let apply_area = Rect {
                    x: close_area.x + btn_width + gap,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                Button::new("Close")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfigReloadFailedClose,
                        self.pressed_button,
                        *focus == ConfigReloadFailedFocus::Close,
                        true,
                    ))
                    .render(frame, close_area, &self.theme);

                Button::new("Recover")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfigReloadFailedApply,
                        self.pressed_button,
                        *focus == ConfigReloadFailedFocus::Apply,
                        *recover_old_config,
                    ))
                    .render(frame, apply_area, &self.theme);

                self.overlay_layout.active = OverlayMouseLayout::ConfigReloadFailed {
                    close_button: close_area,
                    apply_button: apply_area,
                    checkbox,
                };
            }
            PromptState::AddProjectFailed { message, .. } => {
                self.render_dim_overlay(frame);
                let dialog_width = 68.min(frame.area().width.max(1));
                let inner_width = dialog_width.saturating_sub(2);
                let mut body_lines = vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        " The project could not be added.",
                        Style::default().fg(self.theme.warning_fg),
                    )),
                    Line::from(""),
                ];
                for line in message.lines().take(6) {
                    body_lines.push(Line::from(format!(" {line}")));
                }
                let body_height = wrapped_line_count(&body_lines, inner_width, false);
                let area = centered_rect_exact(dialog_width, 2 + body_height + 3, frame.area());
                self.clear_overlay_area(frame, area);
                let outer = self.themed_overlay_block("Add Project Failed");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                let [body_area, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(body_height), Constraint::Length(3)])
                    .areas(inner);

                Paragraph::new(body_lines)
                    .wrap(Wrap { trim: false })
                    .render(body_area, frame.buffer_mut());

                let btn_width = shared_button_width(&["OK"]);
                let ok_area = Rect {
                    x: buttons_area.x + buttons_area.width.saturating_sub(btn_width) / 2,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                Button::new("OK")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::AddProjectFailedOk,
                        self.pressed_button,
                        true,
                        true,
                    ))
                    .render(frame, ok_area, &self.theme);

                self.overlay_layout.active =
                    OverlayMouseLayout::AddProjectFailed { ok_button: ok_area };
            }
            PromptState::AgentInfo(prompt) => {
                self.render_dim_overlay(frame);
                let dialog_width = 72.min(frame.area().width.max(1));
                let inner_width = dialog_width.saturating_sub(2);
                let mut body_lines = vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        format!(" {}", prompt.session_label),
                        Style::default()
                            .fg(self.theme.text_fg)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(""),
                ];
                for (line, tone) in &prompt.lines {
                    // Style by the precomputed semantic tone (tag), never by
                    // re-parsing the prose. The drift note is the one line tagged
                    // Warning; everything else is neutral body text. All colors
                    // come from the theme.
                    let style = match tone {
                        AgentInfoTone::Warning => Style::default().fg(self.theme.warning_fg),
                        AgentInfoTone::Neutral => Style::default().fg(self.theme.text_fg),
                    };
                    body_lines.push(Line::from(Span::styled(format!(" {line}"), style)));
                }
                let body_height = wrapped_line_count(&body_lines, inner_width, false);
                let area = centered_rect_exact(dialog_width, 2 + body_height + 3, frame.area());
                self.clear_overlay_area(frame, area);

                let close_key = self.bindings.label_for(Action::CloseOverlay);
                let mut bottom = vec![Span::raw(" ")];
                bottom.extend(self.theme.key_badge_default(&close_key));
                bottom.push(Span::styled(
                    " close",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                let outer = self
                    .themed_overlay_block("Agent Info")
                    .title_bottom(Line::from(bottom));
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                let [body_area, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(body_height), Constraint::Length(3)])
                    .areas(inner);

                Paragraph::new(body_lines)
                    .wrap(Wrap { trim: false })
                    .render(body_area, frame.buffer_mut());

                let btn_width = shared_button_width(&["Close"]);
                let close_area = Rect {
                    x: buttons_area.x + buttons_area.width.saturating_sub(btn_width) / 2,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                // A read-only modal still gets a focused Close button that Space
                // activates (universal accessibility convention).
                Button::new("Close")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::AgentInfoClose,
                        self.pressed_button,
                        true,
                        true,
                    ))
                    .render(frame, close_area, &self.theme);

                self.overlay_layout.active = OverlayMouseLayout::AgentInfo {
                    close_button: close_area,
                };
            }
            PromptState::ConfirmDeleteAgent {
                branch_name,
                focus,
                delete_worktree,
                worktree_shared,
                ..
            } => {
                self.render_dim_overlay(frame);
                let dialog_width = 56.min(frame.area().width.max(1));
                let inner_width = dialog_width.saturating_sub(2);
                let checkbox_height = if *worktree_shared {
                    0
                } else {
                    let state = if *focus == DeleteAgentFocus::Checkbox {
                        CheckboxState::Focused
                    } else {
                        CheckboxState::Normal
                    };
                    let checkbox = Checkbox::new("Also delete the worktree and branch")
                        .checked(*delete_worktree)
                        .state(state);
                    checkbox
                        .layout(
                            inner_width,
                            checkbox.marker_style(Style::default()),
                            checkbox.label_style(Style::default()),
                        )
                        .height
                };

                // Body: question text + conditional warning/hint/shared note.
                // Long warning text is split into two explicit Lines so it
                // renders correctly even at narrow dialog widths.
                let mut body_lines = vec![
                    Line::from(""),
                    Line::from(vec![
                        Span::raw(" Are you sure you want to delete "),
                        Span::styled(
                            branch_name.as_str(),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("?"),
                    ]),
                    Line::from(""),
                ];
                if *worktree_shared {
                    body_lines.push(Line::from(Span::styled(
                        " Worktree is shared with another agent and will be preserved.",
                        Style::default().fg(self.theme.hint_desc_fg),
                    )));
                } else if *delete_worktree {
                    body_lines.push(Line::from(Span::styled(
                        " All uncommitted and unpushed changes in this",
                        Style::default().fg(self.theme.warning_fg),
                    )));
                    body_lines.push(Line::from(Span::styled(
                        " worktree will be permanently lost.",
                        Style::default().fg(self.theme.warning_fg),
                    )));
                } else {
                    body_lines.push(Line::from(Span::styled(
                        " Worktree and branch will be preserved on disk.",
                        Style::default().fg(self.theme.hint_desc_fg),
                    )));
                }
                let body_height = wrapped_line_count(&body_lines, inner_width, false);
                let checkbox_spacing = u16::from(!*worktree_shared);
                let button_spacing = u16::from(!*worktree_shared);
                let area = centered_rect_exact(
                    dialog_width,
                    2 + body_height + checkbox_spacing + checkbox_height + button_spacing + 3,
                    frame.area(),
                );
                self.clear_overlay_area(frame, area);
                let outer = self.themed_overlay_block("Delete Agent");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                let [body_area, _, checkbox_area, _, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(body_height),
                        Constraint::Length(checkbox_spacing),
                        Constraint::Length(checkbox_height),
                        Constraint::Length(button_spacing),
                        Constraint::Length(3),
                    ])
                    .areas(inner);

                Paragraph::new(body_lines)
                    .wrap(Wrap { trim: false })
                    .render(body_area, frame.buffer_mut());

                let checkbox_rect = if !*worktree_shared {
                    let checkbox_state = if *focus == DeleteAgentFocus::Checkbox {
                        CheckboxState::Focused
                    } else {
                        CheckboxState::Normal
                    };
                    let (rect, _) = self.render_overlay_checkbox(
                        frame,
                        checkbox_area,
                        "Also delete the worktree and branch",
                        *delete_worktree,
                        checkbox_state,
                        None,
                    );
                    Some(OverlayCheckbox {
                        id: OverlayCheckboxId::DeleteAgentWorktree,
                        rect,
                    })
                } else {
                    None
                };

                // Button area: two bordered panels side by side.
                let btn_width = 16u16;
                let gap = 2u16;
                let total = btn_width * 2 + gap;
                let left_offset = buttons_area.width.saturating_sub(total) / 2;

                let cancel_area = Rect {
                    x: buttons_area.x + left_offset,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };
                let delete_area = Rect {
                    x: cancel_area.x + btn_width + gap,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                Button::new("Cancel")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmDeleteCancel,
                        self.pressed_button,
                        *focus == DeleteAgentFocus::Cancel,
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new("Delete")
                    .kind(ButtonKind::Danger)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmDeleteConfirm,
                        self.pressed_button,
                        *focus == DeleteAgentFocus::Delete,
                        true,
                    ))
                    .render(frame, delete_area, &self.theme);

                self.overlay_layout.active = OverlayMouseLayout::ConfirmDeleteAgent {
                    cancel_button: cancel_area,
                    delete_button: delete_area,
                    checkbox: checkbox_rect,
                };
            }
            PromptState::ConfirmDeleteTerminal {
                terminal_label,
                foreground_cmd,
                confirm_selected,
                ..
            } => {
                self.render_dim_overlay(frame);
                let area = centered_rect(56, 30, frame.area());
                self.clear_overlay_area(frame, area);
                let outer = self.themed_overlay_block("Delete Terminal");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                let [body_area, _, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(1),
                        Constraint::Length(1),
                        Constraint::Length(3),
                    ])
                    .areas(inner);

                let mut lines = vec![
                    Line::from(""),
                    Line::from(vec![
                        Span::raw(" Are you sure you want to delete "),
                        Span::styled(
                            terminal_label.as_str(),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("?"),
                    ]),
                ];
                // Only warn about killing a process when an app is actually
                // running in the foreground. Closing an idle terminal merely
                // ends the bare shell, which is not worth a warning.
                if foreground_cmd.is_some() {
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        " The running process will be killed.",
                        Style::default().fg(self.theme.warning_fg),
                    )));
                }
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .render(body_area, frame.buffer_mut());

                let btn_width = 16u16;
                let gap = 2u16;
                let total = btn_width * 2 + gap;
                let left_offset = buttons_area.width.saturating_sub(total) / 2;

                let cancel_area = Rect {
                    x: buttons_area.x + left_offset,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };
                let delete_area = Rect {
                    x: cancel_area.x + btn_width + gap,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                Button::new("Cancel")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmDeleteTerminalCancel,
                        self.pressed_button,
                        !confirm_selected,
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new("Delete")
                    .kind(ButtonKind::Danger)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmDeleteTerminalConfirm,
                        self.pressed_button,
                        *confirm_selected,
                        true,
                    ))
                    .render(frame, delete_area, &self.theme);

                self.overlay_layout.active = OverlayMouseLayout::ConfirmDeleteTerminal {
                    cancel_button: cancel_area,
                    delete_button: delete_area,
                };
            }
            PromptState::ConfirmCloseTab {
                session_id,
                provider_label,
                is_main,
                confirm_selected,
                ..
            } => {
                self.render_dim_overlay(frame);
                let area = centered_rect(56, 30, frame.area());
                self.clear_overlay_area(frame, area);
                // Closing the agent's only tab detaches the agent instead of
                // ending a single tab; word the copy accordingly.
                let only_tab = self.engine.tab_ids_for_session(session_id).len() <= 1;
                let outer = self.themed_overlay_block("Close Tab");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                let [body_area, _, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(1),
                        Constraint::Length(1),
                        Constraint::Length(3),
                    ])
                    .areas(inner);

                let agent_name = self
                    .engine
                    .sessions
                    .iter()
                    .find(|s| &s.id == session_id)
                    .map(|s| self.session_label(s))
                    .unwrap_or_else(|| session_id.clone());

                let tail = confirm_close_tab_tail(only_tab, *is_main);
                let lines = vec![
                    Line::from(""),
                    Line::from(vec![
                        Span::raw(" Close the "),
                        Span::styled(
                            provider_label.as_str(),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(" tab on "),
                        Span::styled(
                            agent_name.as_str(),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("?"),
                    ]),
                    Line::from(""),
                    Line::from(Span::styled(
                        tail,
                        Style::default().fg(self.theme.warning_fg),
                    )),
                ];
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .render(body_area, frame.buffer_mut());

                let btn_width = 16u16;
                let gap = 2u16;
                let total = btn_width * 2 + gap;
                let left_offset = buttons_area.width.saturating_sub(total) / 2;

                let cancel_area = Rect {
                    x: buttons_area.x + left_offset,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };
                let confirm_area = Rect {
                    x: cancel_area.x + btn_width + gap,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                Button::new("Cancel")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmCloseTabCancel,
                        self.pressed_button,
                        !confirm_selected,
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new("Close")
                    .kind(ButtonKind::Danger)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmCloseTabConfirm,
                        self.pressed_button,
                        *confirm_selected,
                        true,
                    ))
                    .render(frame, confirm_area, &self.theme);

                self.overlay_layout.active = OverlayMouseLayout::ConfirmCloseTab {
                    cancel_button: cancel_area,
                    confirm_button: confirm_area,
                };
            }
            PromptState::ConfirmQuit {
                agent_count,
                terminal_count,
                confirm_selected,
            } => {
                self.render_dim_overlay(frame);
                let area = centered_rect(56, 30, frame.area());
                self.clear_overlay_area(frame, area);
                let outer = self.themed_overlay_block("Quit dux");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                let [body_area, _, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(1),
                        Constraint::Length(1),
                        Constraint::Length(3),
                    ])
                    .areas(inner);

                let process_desc = quit_process_description(*agent_count, *terminal_count);
                let lines = vec![
                    Line::from(""),
                    Line::from(vec![
                        Span::raw(format!(" {process_desc} will be ")),
                        Span::styled(
                            "killed",
                            Style::default()
                                .fg(self.theme.button_danger_border)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(" if you quit."),
                    ]),
                    Line::from(""),
                    Line::from(Span::styled(
                        " Any in-progress work will be lost.",
                        Style::default().fg(self.theme.warning_fg),
                    )),
                    Line::from(Span::styled(
                        " File changes in worktrees are preserved.",
                        Style::default().fg(self.theme.hint_desc_fg),
                    )),
                ];
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .render(body_area, frame.buffer_mut());

                let btn_width = 16u16;
                let gap = 2u16;
                let total = btn_width * 2 + gap;
                let left_offset = buttons_area.width.saturating_sub(total) / 2;

                let cancel_area = Rect {
                    x: buttons_area.x + left_offset,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };
                let quit_area = Rect {
                    x: cancel_area.x + btn_width + gap,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                Button::new("Cancel")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmQuitCancel,
                        self.pressed_button,
                        !confirm_selected,
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new("Quit")
                    .kind(ButtonKind::Danger)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmQuitConfirm,
                        self.pressed_button,
                        *confirm_selected,
                        true,
                    ))
                    .render(frame, quit_area, &self.theme);

                self.overlay_layout.active = OverlayMouseLayout::ConfirmQuit {
                    cancel_button: cancel_area,
                    quit_button: quit_area,
                };
            }
            PromptState::ConfirmDiscardFile {
                file_path,
                confirm_selected,
                ..
            } => {
                self.render_dim_overlay(frame);
                let area = centered_rect(56, 30, frame.area());
                self.clear_overlay_area(frame, area);
                let outer = self.themed_overlay_block("Discard Changes");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                let [body_area, _, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(1),
                        Constraint::Length(1),
                        Constraint::Length(3),
                    ])
                    .areas(inner);

                let lines = vec![
                    Line::from(""),
                    Line::from(vec![
                        Span::raw(" Discard all changes to \""),
                        Span::styled(
                            file_path.as_str(),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("\"?"),
                    ]),
                    Line::from(""),
                    Line::from(Span::styled(
                        " This action cannot be undone.",
                        Style::default().fg(self.theme.warning_fg),
                    )),
                ];
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .render(body_area, frame.buffer_mut());

                let btn_width = 16u16;
                let gap = 2u16;
                let total = btn_width * 2 + gap;
                let left_offset = buttons_area.width.saturating_sub(total) / 2;

                let cancel_area = Rect {
                    x: buttons_area.x + left_offset,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };
                let discard_area = Rect {
                    x: cancel_area.x + btn_width + gap,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                Button::new("Cancel")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmDiscardCancel,
                        self.pressed_button,
                        !confirm_selected,
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new("Discard")
                    .kind(ButtonKind::Danger)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmDiscardConfirm,
                        self.pressed_button,
                        *confirm_selected,
                        true,
                    ))
                    .render(frame, discard_area, &self.theme);

                self.overlay_layout.active = OverlayMouseLayout::ConfirmDiscardFile {
                    cancel_button: cancel_area,
                    discard_button: discard_area,
                };
            }
            PromptState::ConfirmCreateInitialCommit {
                path,
                confirm_selected,
                ..
            } => {
                self.render_dim_overlay(frame);
                let area = centered_rect(60, 36, frame.area());
                self.clear_overlay_area(frame, area);
                let outer = self.themed_overlay_block("Repository Has No Commits");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                let [body_area, _, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(1),
                        Constraint::Length(1),
                        Constraint::Length(3),
                    ])
                    .areas(inner);

                let lines = vec![
                    Line::from(""),
                    Line::from(vec![
                        Span::raw(" \""),
                        Span::styled(path.as_str(), Style::default().add_modifier(Modifier::BOLD)),
                        Span::raw("\" has no commits yet,"),
                    ]),
                    Line::from(" so agents can't branch worktrees from it."),
                    Line::from(""),
                    Line::from(Span::styled(
                        " Dux can make an empty initial commit so it works.",
                        Style::default().fg(self.theme.warning_fg),
                    )),
                    Line::from(Span::styled(
                        " Your existing files are left untouched (untracked).",
                        Style::default().fg(self.theme.hint_desc_fg),
                    )),
                ];
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .render(body_area, frame.buffer_mut());

                let btn_width = 22u16;
                let gap = 2u16;
                let total = btn_width * 2 + gap;
                let left_offset = buttons_area.width.saturating_sub(total) / 2;

                let cancel_area = Rect {
                    x: buttons_area.x + left_offset,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };
                let create_area = Rect {
                    x: cancel_area.x + btn_width + gap,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                Button::new("Cancel")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmCreateInitialCommitCancel,
                        self.pressed_button,
                        !confirm_selected,
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new("Create Commit & Add")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmCreateInitialCommitConfirm,
                        self.pressed_button,
                        *confirm_selected,
                        true,
                    ))
                    .render(frame, create_area, &self.theme);

                self.overlay_layout.active = OverlayMouseLayout::ConfirmCreateInitialCommit {
                    cancel_button: cancel_area,
                    create_button: create_area,
                };
            }
            PromptState::ConfirmInitRepo {
                path,
                candidates,
                confirm_selected,
                ..
            } => {
                self.render_dim_overlay(frame);
                let area = centered_rect(60, 40, frame.area());
                self.clear_overlay_area(frame, area);
                let outer = self.themed_overlay_block("Not a Git Repository");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                let [body_area, _, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(1),
                        Constraint::Length(1),
                        Constraint::Length(3),
                    ])
                    .areas(inner);

                let mut lines = vec![
                    Line::from(""),
                    Line::from(vec![
                        Span::raw(" \""),
                        Span::styled(path.as_str(), Style::default().add_modifier(Modifier::BOLD)),
                        Span::raw("\" is not a git repository."),
                    ]),
                    Line::from(""),
                    Line::from(Span::styled(
                        " Dux can initialize one and make an empty initial commit.",
                        Style::default().fg(self.theme.warning_fg),
                    )),
                ];
                if !candidates.is_empty() {
                    lines.push(Line::from(Span::styled(
                        format!(
                            " A starter .gitignore will cover: {}.",
                            candidates.join(", ")
                        ),
                        Style::default().fg(self.theme.hint_desc_fg),
                    )));
                }
                lines.push(Line::from(Span::styled(
                    " Your existing files are left untouched (untracked).",
                    Style::default().fg(self.theme.hint_desc_fg),
                )));
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .render(body_area, frame.buffer_mut());

                let btn_width = 22u16;
                let gap = 2u16;
                let total = btn_width * 2 + gap;
                let left_offset = buttons_area.width.saturating_sub(total) / 2;

                let cancel_area = Rect {
                    x: buttons_area.x + left_offset,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };
                let init_area = Rect {
                    x: cancel_area.x + btn_width + gap,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                Button::new("Cancel")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmInitRepoCancel,
                        self.pressed_button,
                        !confirm_selected,
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new("Initialize & Add")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmInitRepoConfirm,
                        self.pressed_button,
                        *confirm_selected,
                        true,
                    ))
                    .render(frame, init_area, &self.theme);

                self.overlay_layout.active = OverlayMouseLayout::ConfirmInitRepo {
                    cancel_button: cancel_area,
                    init_button: init_area,
                };
            }
            PromptState::ConfirmNonDefaultBranch {
                action,
                current_branch,
                kind,
                focus,
                checkout_default,
                ..
            } => {
                self.render_dim_overlay(frame);
                let dialog_width = 60u16.min(frame.area().width.max(1));
                let inner_width = dialog_width.saturating_sub(2);
                let has_checkbox =
                    matches!(kind, BranchWarningKind::Known { .. }) && action.allows_add_anyway();

                // Body: warning text + the "new worktrees branch from …" note,
                // plus a dim info line on the heuristic path explaining why dux
                // won't offer to switch branches automatically.
                let mut body_lines = vec![Line::from("")];
                match kind {
                    BranchWarningKind::Known { default_branch } => {
                        body_lines.push(Line::from(vec![
                            Span::raw(" This repository is on branch "),
                            Span::styled(
                                current_branch.as_str(),
                                Style::default().add_modifier(Modifier::BOLD),
                            ),
                            Span::raw(", but the"),
                        ]));
                        body_lines.push(Line::from(vec![
                            Span::raw(" remote default branch is "),
                            Span::styled(
                                default_branch.as_str(),
                                Style::default().add_modifier(Modifier::BOLD),
                            ),
                            Span::raw("."),
                        ]));
                    }
                    BranchWarningKind::Heuristic => {
                        body_lines.push(Line::from(vec![
                            Span::raw(" This repository is on branch "),
                            Span::styled(
                                current_branch.as_str(),
                                Style::default().add_modifier(Modifier::BOLD),
                            ),
                            Span::raw(","),
                        ]));
                        body_lines.push(Line::from(" which doesn't appear to be the main branch."));
                    }
                }
                body_lines.push(Line::from(""));
                let worktree_warning =
                    format!(" New worktrees will branch from \"{current_branch}\".");
                body_lines.push(Line::from(Span::styled(
                    worktree_warning,
                    Style::default().fg(self.theme.warning_fg),
                )));
                if matches!(kind, BranchWarningKind::Heuristic) {
                    body_lines.push(Line::from(""));
                    body_lines.push(Line::from(Span::styled(
                        " Dux can't confidently identify this repo's default",
                        Style::default().fg(self.theme.hint_desc_fg),
                    )));
                    body_lines.push(Line::from(Span::styled(
                        " branch, so it won't change branches for you.",
                        Style::default().fg(self.theme.hint_desc_fg),
                    )));
                }
                let body_height = wrapped_line_count(&body_lines, inner_width, false);

                // Checkbox height is measured up-front so the outer rect can
                // be sized exactly — mirrors the Delete Agent modal.
                let checkbox_height = if has_checkbox {
                    let BranchWarningKind::Known { default_branch } = kind else {
                        unreachable!("has_checkbox requires a known default branch");
                    };
                    let state = if *focus == ConfirmNonDefaultBranchFocus::Checkbox {
                        CheckboxState::Focused
                    } else {
                        CheckboxState::Normal
                    };
                    let label = format!("Check out \"{default_branch}\" before adding");
                    let checkbox = Checkbox::new(&label)
                        .checked(*checkout_default)
                        .state(state);
                    checkbox
                        .layout(
                            inner_width,
                            checkbox.marker_style(Style::default()),
                            checkbox.label_style(Style::default()),
                        )
                        .height
                } else {
                    0
                };
                let checkbox_spacing = u16::from(has_checkbox);

                let area = centered_rect_exact(
                    dialog_width,
                    2 + body_height + checkbox_spacing + checkbox_height + 3,
                    frame.area(),
                );
                self.clear_overlay_area(frame, area);
                let outer = self.themed_overlay_block("Non-Default Branch");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                let [body_area, _, checkbox_area, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(body_height),
                        Constraint::Length(checkbox_spacing),
                        Constraint::Length(checkbox_height),
                        Constraint::Length(3),
                    ])
                    .areas(inner);

                Paragraph::new(body_lines)
                    .wrap(Wrap { trim: false })
                    .render(body_area, frame.buffer_mut());

                let checkbox_rect = if has_checkbox {
                    let BranchWarningKind::Known { default_branch } = kind else {
                        unreachable!("has_checkbox requires a known default branch");
                    };
                    let checkbox_state = if *focus == ConfirmNonDefaultBranchFocus::Checkbox {
                        CheckboxState::Focused
                    } else {
                        CheckboxState::Normal
                    };
                    let label = format!("Check out \"{default_branch}\" before adding");
                    let (rect, _) = self.render_overlay_checkbox(
                        frame,
                        checkbox_area,
                        &label,
                        *checkout_default,
                        checkbox_state,
                        None,
                    );
                    Some(OverlayCheckbox {
                        id: OverlayCheckboxId::NonDefaultBranchCheckoutDefault,
                        rect,
                    })
                } else {
                    None
                };

                // Both buttons share a single width derived from the longest
                // label that could appear in either slot. Including
                // "Check Out & Add" and "Add Anyway" in the calculation keeps
                // the layout stable when the user toggles the checkbox —
                // otherwise the buttons would resize mid-modal.
                let btn_width = shared_button_width(&["Cancel", "Add Anyway", "Check Out & Add"]);
                let gap = 2u16;
                let total = btn_width * 2 + gap;
                let left_offset = buttons_area.width.saturating_sub(total) / 2;

                let cancel_area = Rect {
                    x: buttons_area.x + left_offset,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };
                let add_area = Rect {
                    x: cancel_area.x + btn_width + gap,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                // Swap the confirm button label so the user sees exactly what
                // pressing it will do. When the checkbox is on and we know the
                // default branch, the action is a two-step (switch + add),
                // otherwise it's the original "Add Anyway" add-as-is.
                let add_label = if has_checkbox && *checkout_default {
                    "Check Out & Add"
                } else {
                    "Add Anyway"
                };

                Button::new("Cancel")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmNonDefaultBranchCancel,
                        self.pressed_button,
                        *focus == ConfirmNonDefaultBranchFocus::Cancel,
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new(add_label)
                    .kind(ButtonKind::Danger)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmNonDefaultBranchAdd,
                        self.pressed_button,
                        *focus == ConfirmNonDefaultBranchFocus::Add,
                        true,
                    ))
                    .render(frame, add_area, &self.theme);

                self.overlay_layout.active = OverlayMouseLayout::ConfirmNonDefaultBranch {
                    cancel_button: cancel_area,
                    add_button: add_area,
                    checkbox: checkbox_rect,
                };
            }
            PromptState::ConfirmUseExistingBranch {
                branch_name,
                location,
                confirm_selected,
                ..
            } => {
                self.render_dim_overlay(frame);
                let area = centered_rect(60, 30, frame.area());
                self.clear_overlay_area(frame, area);
                let outer = self.themed_overlay_block("Branch Already Exists");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                let [body_area, _, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(1),
                        Constraint::Length(1),
                        Constraint::Length(3),
                    ])
                    .areas(inner);

                let location_label = match location {
                    crate::git::BranchLocation::Local => "local",
                    crate::git::BranchLocation::Remote => "remote",
                };
                let mut lines = vec![Line::from("")];
                lines.push(Line::from(vec![
                    Span::raw(" A "),
                    Span::raw(location_label),
                    Span::raw(" branch named "),
                    Span::styled(
                        branch_name.as_str(),
                        Style::default().add_modifier(Modifier::BOLD),
                    ),
                ]));
                lines.push(Line::from(" already exists."));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    " A new worktree will be created using this branch,",
                    Style::default().fg(self.theme.warning_fg),
                )));
                lines.push(Line::from(Span::styled(
                    " allowing you to continue working on it.",
                    Style::default().fg(self.theme.warning_fg),
                )));
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .render(body_area, frame.buffer_mut());

                let btn_width = 16u16;
                let gap = 2u16;
                let total = btn_width * 2 + gap;
                let left_offset = buttons_area.width.saturating_sub(total) / 2;

                let cancel_area = Rect {
                    x: buttons_area.x + left_offset,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };
                let use_area = Rect {
                    x: cancel_area.x + btn_width + gap,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                Button::new("Cancel")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmUseExistingBranchCancel,
                        self.pressed_button,
                        !confirm_selected,
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                // "Use Existing" reuses a branch that already exists — not
                // destructive, so it shares the Confirm kind with Cancel.
                Button::new("Use Existing")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmUseExistingBranchUse,
                        self.pressed_button,
                        *confirm_selected,
                        true,
                    ))
                    .render(frame, use_area, &self.theme);

                self.overlay_layout.active = OverlayMouseLayout::ConfirmUseExistingBranch {
                    cancel_button: cancel_area,
                    use_button: use_area,
                };
            }
            PromptState::RenameSession {
                input,
                rename_branch,
                ..
            } => {
                self.render_dim_overlay(frame);
                let checkbox = Checkbox::new("Also rename the git branch")
                    .checked(*rename_branch)
                    .state(CheckboxState::Normal);
                let dialog_width = 62.min(frame.area().width.max(1));
                let inner_width = dialog_width.saturating_sub(2);
                let checkbox_height = checkbox
                    .layout(
                        inner_width,
                        checkbox.marker_style(Style::default()),
                        checkbox.label_style(Style::default()),
                    )
                    .height
                    .saturating_add(1);
                let checkbox_spacing = 1;
                let area = centered_rect_exact(
                    dialog_width,
                    9 + checkbox_spacing + checkbox_height,
                    frame.area(),
                );
                self.clear_overlay_area(frame, area);

                let outer = self.themed_overlay_block("Rename Agent");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                let [label_area, input_area, _, checkbox_area, hint_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1),
                        Constraint::Length(3),
                        Constraint::Length(checkbox_spacing),
                        Constraint::Length(checkbox_height),
                        Constraint::Min(1),
                    ])
                    .areas(inner);

                Paragraph::new(Line::from(Span::styled(
                    " Enter a new name (empty to reset):",
                    Style::default().fg(self.theme.input_label_fg),
                )))
                .render(label_area, frame.buffer_mut());

                // Show the input with a cursor indicator.
                let display = if input.cursor < input.text.len() {
                    let (before, after) = input.text.split_at(input.cursor);
                    let (cursor_char, rest) = after.split_at(1);
                    Line::from(vec![
                        Span::raw(format!(" {before}")),
                        Span::styled(
                            cursor_char.to_string(),
                            Style::default()
                                .fg(self.theme.input_cursor_fg)
                                .bg(self.theme.input_cursor_bg),
                        ),
                        Span::raw(rest.to_string()),
                    ])
                } else {
                    Line::from(vec![
                        Span::raw(format!(" {}", &input.text)),
                        Span::styled(
                            " ",
                            Style::default()
                                .fg(self.theme.input_cursor_fg)
                                .bg(self.theme.input_cursor_bg),
                        ),
                    ])
                };
                let input_block = Block::default()
                    .borders(Borders::ALL)
                    .border_set(border::ROUNDED)
                    .border_style(Style::default().fg(self.theme.overlay_border));
                let input_inner = input_block.inner(input_area);
                Paragraph::new(display)
                    .block(input_block)
                    .render(input_area, frame.buffer_mut());

                let (checkbox_rect, _) = self.render_overlay_checkbox(
                    frame,
                    checkbox_area,
                    "Also rename the git branch",
                    *rename_branch,
                    CheckboxState::Normal,
                    Some(Line::from(Span::styled(
                        format!(
                            "{}Open PRs will still reference the old branch name",
                            Checkbox::indent()
                        ),
                        Style::default().fg(self.theme.hint_desc_fg),
                    ))),
                );

                let confirm_key = self.bindings.label_for(Action::Confirm);
                let close_key = self.bindings.label_for(Action::CloseOverlay);
                let toggle_key = self.bindings.label_for(Action::ToggleSelection);
                let mut hints = vec![Span::raw(" ")];
                hints.extend(self.theme.key_badge_default(&confirm_key));
                hints.push(Span::styled(
                    " confirm  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                hints.extend(self.theme.key_badge_default(&toggle_key));
                hints.push(Span::styled(
                    " toggle  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                hints.extend(self.theme.key_badge_default(&close_key));
                hints.push(Span::styled(
                    " cancel",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                Paragraph::new(Line::from(hints)).render(hint_area, frame.buffer_mut());
                self.overlay_layout.active = OverlayMouseLayout::RenameSession {
                    input: input_inner,
                    checkbox: Some(OverlayCheckbox {
                        id: OverlayCheckboxId::RenameSessionBranch,
                        rect: checkbox_rect,
                    }),
                };
            }
            PromptState::EditMacros { .. } => {
                // Full rendering implemented in Task #5.
                self.render_edit_macros(frame);
            }
            PromptState::ResourceMonitor {
                rows,
                scroll_offset,
                selected_row,
                expanded,
                short_window_sample,
                ..
            } => {
                let rows = rows.clone();
                let scroll_offset = *scroll_offset;
                let selected_row = *selected_row;
                let expanded = expanded.clone();
                let short_window_sample = *short_window_sample;
                self.render_resource_monitor(
                    frame,
                    &rows,
                    scroll_offset,
                    selected_row,
                    &expanded,
                    short_window_sample,
                );
            }
            PromptState::DebugInput {
                lines,
                scroll_offset,
            } => {
                self.render_dim_overlay(frame);
                let popup = centered_rect(80, 70, frame.area());
                self.clear_overlay_area(frame, popup);

                // Split: content area + 1-line footer hint.
                let chunks =
                    Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(popup);
                let content_area = chunks[0];
                let hint_area = chunks[1];

                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(self.theme.overlay_border))
                    .title(" Input Debugger ")
                    .title_style(
                        Style::default()
                            .fg(self.theme.help_section_header_fg)
                            .add_modifier(Modifier::BOLD),
                    );
                let inner = block.inner(content_area);
                block.render(content_area, frame.buffer_mut());

                // Compute the visible window.
                let visible_h = inner.height as usize;
                let total = lines.len();
                let max_offset = total.saturating_sub(visible_h);
                let offset = (*scroll_offset as usize).min(max_offset);

                // When scroll_offset exceeds max (auto-scroll sentinel), pin to bottom.
                let start = if *scroll_offset as usize >= total {
                    max_offset
                } else {
                    offset
                };

                let visible: Vec<Line> =
                    lines.iter().skip(start).take(visible_h).cloned().collect();

                let paragraph = Paragraph::new(visible);
                paragraph.render(inner, frame.buffer_mut());

                // Footer hint.
                let hint = Line::from(vec![
                    Span::styled("Esc", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(" close  "),
                    Span::styled("Scroll", Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw(" navigate"),
                ]);
                let hint_para = Paragraph::new(hint)
                    .alignment(ratatui::layout::Alignment::Center)
                    .style(
                        Style::default()
                            .fg(self.theme.hint_desc_fg)
                            .add_modifier(Modifier::DIM),
                    );
                hint_para.render(hint_area, frame.buffer_mut());
            }
            PromptState::NameNewAgent {
                request,
                input,
                randomize_name,
                copy_changes,
                focus,
                ..
            } => {
                self.render_dim_overlay(frame);
                let randomize_checkbox = Checkbox::new("Use randomized pet name")
                    .checked(*randomize_name)
                    .state(if *focus == NameNewAgentFocus::RandomizedNameCheckbox {
                        CheckboxState::Focused
                    } else {
                        CheckboxState::Normal
                    });
                let dialog_width = 60.min(frame.area().width.max(1));
                let inner_width = dialog_width.saturating_sub(2);
                let randomize_checkbox_height = randomize_checkbox
                    .layout(
                        inner_width,
                        randomize_checkbox.marker_style(Style::default()),
                        randomize_checkbox.label_style(Style::default()),
                    )
                    .height
                    .saturating_add(1);
                // Only fresh project agents expose the copy checkbox: forks
                // always copy and the other flows never do.
                let show_copy_checkbox = matches!(request, CreateAgentRequest::NewProject { .. });
                let copy_checkbox_label = "Copy uncommitted changes from the project checkout";
                let copy_checkbox_height = if show_copy_checkbox {
                    let copy_checkbox = Checkbox::new(copy_checkbox_label);
                    copy_checkbox
                        .layout(
                            inner_width,
                            copy_checkbox.marker_style(Style::default()),
                            copy_checkbox.label_style(Style::default()),
                        )
                        .height
                        .saturating_add(1)
                } else {
                    0
                };
                let checkbox_spacing = 1;
                let copy_checkbox_spacing = u16::from(show_copy_checkbox);
                let footer_spacing = 1;
                let context_line = match request {
                    CreateAgentRequest::ExistingManagedWorktree { worktree_path, .. } => {
                        Some(format!(
                            " This starts a fresh agent session in {}.",
                            worktree_path.display()
                        ))
                    }
                    CreateAgentRequest::ForkExternalWorktree {
                        source_worktree_path,
                        ..
                    } => Some(format!(
                        " External worktree will be copied into a fresh managed dux worktree: {}.",
                        source_worktree_path.display()
                    )),
                    _ => None,
                };
                let context_height = u16::from(context_line.is_some());
                let area = centered_rect_exact(
                    dialog_width,
                    8 + context_height
                        + checkbox_spacing
                        + randomize_checkbox_height
                        + copy_checkbox_spacing
                        + copy_checkbox_height
                        + footer_spacing,
                    frame.area(),
                );
                self.clear_overlay_area(frame, area);

                let outer = self.themed_overlay_block("Name New Agent");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                let [
                    label_area,
                    context_area,
                    input_area,
                    _,
                    randomize_checkbox_area,
                    _,
                    copy_checkbox_area,
                    _,
                    hint_area,
                ] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1),
                        Constraint::Length(context_height),
                        Constraint::Length(3),
                        Constraint::Length(checkbox_spacing),
                        Constraint::Length(randomize_checkbox_height),
                        Constraint::Length(copy_checkbox_spacing),
                        Constraint::Length(copy_checkbox_height),
                        Constraint::Length(footer_spacing),
                        Constraint::Min(1),
                    ])
                    .areas(inner);

                let label = if matches!(request, CreateAgentRequest::ExistingManagedWorktree { .. })
                {
                    " Enter a display name for the new agent:"
                } else {
                    " Enter a name for the new agent (used as branch name):"
                };
                Paragraph::new(Line::from(Span::styled(
                    label,
                    Style::default().fg(self.theme.input_label_fg),
                )))
                .render(label_area, frame.buffer_mut());
                if let Some(context_line) = context_line {
                    Paragraph::new(Line::from(Span::styled(
                        git::ellipsize_middle(&context_line, inner.width as usize),
                        Style::default().fg(self.theme.hint_desc_fg),
                    )))
                    .render(context_area, frame.buffer_mut());
                }

                // Input field with cursor indicator.
                let display = if input.cursor < input.text.len() {
                    let (before, after) = input.text.split_at(input.cursor);
                    let (cursor_char, rest) = after.split_at(1);
                    Line::from(vec![
                        Span::raw(format!(" {before}")),
                        Span::styled(
                            cursor_char.to_string(),
                            Style::default()
                                .fg(self.theme.input_cursor_fg)
                                .bg(self.theme.input_cursor_bg),
                        ),
                        Span::raw(rest.to_string()),
                    ])
                } else {
                    Line::from(vec![
                        Span::raw(format!(" {}", &input.text)),
                        Span::styled(
                            " ",
                            Style::default()
                                .fg(self.theme.input_cursor_fg)
                                .bg(self.theme.input_cursor_bg),
                        ),
                    ])
                };
                let input_block = Block::default()
                    .borders(Borders::ALL)
                    .border_set(border::ROUNDED)
                    .border_style(Style::default().fg(self.theme.overlay_border));
                let input_inner = input_block.inner(input_area);
                Paragraph::new(display)
                    .block(input_block)
                    .render(input_area, frame.buffer_mut());

                let (randomized_name_checkbox_rect, _) = self.render_overlay_checkbox(
                    frame,
                    randomize_checkbox_area,
                    "Use randomized pet name",
                    *randomize_name,
                    if *focus == NameNewAgentFocus::RandomizedNameCheckbox {
                        CheckboxState::Focused
                    } else {
                        CheckboxState::Normal
                    },
                    Some(Line::from(Span::styled(
                        format!(
                            "{}Fills this prompt with a fresh pet-tool name",
                            Checkbox::indent()
                        ),
                        Style::default().fg(self.theme.hint_desc_fg),
                    ))),
                );

                let copy_checkbox_rect = if show_copy_checkbox {
                    let (rect, _) = self.render_overlay_checkbox(
                        frame,
                        copy_checkbox_area,
                        copy_checkbox_label,
                        *copy_changes,
                        if *focus == NameNewAgentFocus::CopyChangesCheckbox {
                            CheckboxState::Focused
                        } else {
                            CheckboxState::Normal
                        },
                        Some(Line::from(Span::styled(
                            format!(
                                "{}Requires the checkout to be on the same commit",
                                Checkbox::indent()
                            ),
                            Style::default().fg(self.theme.hint_desc_fg),
                        ))),
                    );
                    Some(rect)
                } else {
                    None
                };

                let confirm_key = self.bindings.label_for(Action::Confirm);
                let close_key = self.bindings.label_for(Action::CloseOverlay);
                let toggle_key = self.bindings.label_for(Action::ToggleSelection);
                let mut hints = vec![Span::raw(" ")];
                hints.extend(self.theme.key_badge_default(&confirm_key));
                hints.push(Span::styled(
                    " confirm  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                hints.extend(self.theme.key_badge_default(&toggle_key));
                hints.push(Span::styled(
                    " focus  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                hints.push(Span::styled(
                    "Space toggle  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                hints.extend(self.theme.key_badge_default(&close_key));
                hints.push(Span::styled(
                    " cancel",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                Paragraph::new(Line::from(hints)).render(hint_area, frame.buffer_mut());
                self.overlay_layout.active = OverlayMouseLayout::NameNewAgent {
                    input: input_inner,
                    checkbox: Some(OverlayCheckbox {
                        id: OverlayCheckboxId::NameNewAgentRandomizedPetName,
                        rect: randomized_name_checkbox_rect,
                    }),
                    copy_checkbox: copy_checkbox_rect.map(|rect| OverlayCheckbox {
                        id: OverlayCheckboxId::NameNewAgentCopyChanges,
                        rect,
                    }),
                };
            }
            PromptState::PullRequestInput { project, input } => {
                self.render_dim_overlay(frame);
                let area = centered_rect_exact(64, 8, frame.area());
                self.clear_overlay_area(frame, area);

                let outer = self.themed_overlay_block("Create Agent From PR");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                let [label_area, input_area, hint_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(2),
                        Constraint::Length(3),
                        Constraint::Min(1),
                    ])
                    .areas(inner);

                Paragraph::new(vec![
                    Line::from(Span::styled(
                        format!(" Project: {}", project.name),
                        Style::default().fg(self.theme.input_label_fg),
                    )),
                    Line::from(Span::styled(
                        " Paste a GitHub PR URL or enter a PR number:",
                        Style::default().fg(self.theme.input_label_fg),
                    )),
                ])
                .render(label_area, frame.buffer_mut());

                let display = if input.cursor < input.text.len() {
                    let (before, after) = input.text.split_at(input.cursor);
                    let (cursor_char, rest) = after.split_at(1);
                    Line::from(vec![
                        Span::raw(format!(" {before}")),
                        Span::styled(
                            cursor_char.to_string(),
                            Style::default()
                                .fg(self.theme.input_cursor_fg)
                                .bg(self.theme.input_cursor_bg),
                        ),
                        Span::raw(rest.to_string()),
                    ])
                } else {
                    Line::from(vec![
                        Span::raw(format!(" {}", &input.text)),
                        Span::styled(
                            " ",
                            Style::default()
                                .fg(self.theme.input_cursor_fg)
                                .bg(self.theme.input_cursor_bg),
                        ),
                    ])
                };
                let input_block = Block::default()
                    .borders(Borders::ALL)
                    .border_set(border::ROUNDED)
                    .border_style(Style::default().fg(self.theme.overlay_border));
                Paragraph::new(display)
                    .block(input_block)
                    .render(input_area, frame.buffer_mut());

                let confirm_key = self.bindings.label_for(Action::Confirm);
                let close_key = self.bindings.label_for(Action::CloseOverlay);
                let mut hints = vec![Span::raw(" ")];
                hints.extend(self.theme.key_badge_default(&confirm_key));
                hints.push(Span::styled(
                    " resolve  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                hints.extend(self.theme.key_badge_default(&close_key));
                hints.push(Span::styled(
                    " cancel",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                Paragraph::new(Line::from(hints)).render(hint_area, frame.buffer_mut());
                self.overlay_layout.active = OverlayMouseLayout::None;
            }
            PromptState::None => {}
        }
    }

    fn render_edit_macros(&mut self, frame: &mut Frame) {
        use super::MacroEditStage;

        // Pre-compute the popup layout so we can set the display width for
        // soft-wrapping before taking the immutable borrow on self.prompt.
        let popup = centered_rect_exact(64, 20, frame.area());
        {
            // Temporarily borrow prompt mutably to set the text input's
            // viewport to match the available inner area after all borders,
            // labels, and hint rows have been removed.
            if let PromptState::EditMacros {
                editing: Some(edit_state),
                ..
            } = &mut self.prompt
                && edit_state.stage == MacroEditStage::EditText
            {
                sync_macro_text_input_layout(&mut edit_state.text_input, popup);
            }
        }

        let PromptState::EditMacros {
            entries,
            selected,
            editing,
            pending_delete,
        } = &self.prompt
        else {
            return;
        };

        self.render_dim_overlay(frame);
        self.clear_overlay_area(frame, popup);

        if let Some(edit_state) = editing {
            // ── Edit view ──
            let title = match &edit_state.id {
                Some(name) => format!("Edit Macro — {name}"),
                None => "New Macro".to_string(),
            };
            let outer = self.themed_overlay_block(&title);
            let inner = outer.inner(popup);
            outer.render(popup, frame.buffer_mut());

            match edit_state.stage {
                MacroEditStage::EditName => {
                    let [label_area, input_area, _, surface_area, _, hint_area] = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(1),
                            Constraint::Length(3),
                            Constraint::Length(1),
                            Constraint::Length(1),
                            Constraint::Min(1),
                            Constraint::Length(1),
                        ])
                        .areas(inner);

                    Paragraph::new(Line::from(Span::styled(
                        " Name (identifies this macro):",
                        Style::default().fg(self.theme.input_label_fg),
                    )))
                    .render(label_area, frame.buffer_mut());

                    self.render_single_line_input(&edit_state.name_input, input_area, frame);

                    // Surface radio buttons
                    let current = edit_state.surface;
                    let options = [
                        (MacroSurface::Agent, "Agent"),
                        (MacroSurface::Terminal, "Terminal"),
                        (MacroSurface::Both, "Both"),
                    ];
                    let mut radio_spans: Vec<Span> = vec![Span::styled(
                        " Surface:  ",
                        Style::default().fg(self.theme.input_label_fg),
                    )];
                    for (i, (variant, label)) in options.iter().enumerate() {
                        if i > 0 {
                            radio_spans.push(Span::styled("    ", Style::default()));
                        }
                        let bullet = if *variant == current { "● " } else { "○ " };
                        let style = if *variant == current {
                            Style::default().fg(self.theme.input_label_fg)
                        } else {
                            Style::default().fg(self.theme.hint_desc_fg)
                        };
                        radio_spans.push(Span::styled(bullet, style));
                        radio_spans.push(Span::styled(*label, style));
                    }
                    Paragraph::new(Line::from(radio_spans))
                        .render(surface_area, frame.buffer_mut());

                    let hints = self.edit_macro_hints(&[
                        ("Enter", "next"),
                        ("Tab/Shift-Tab", "surface"),
                        ("Esc", "cancel"),
                    ]);
                    Paragraph::new(Line::from(hints)).render(hint_area, frame.buffer_mut());
                }
                MacroEditStage::EditText => {
                    let [label_area, bordered_area, hint_area] = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([
                            Constraint::Length(1),
                            Constraint::Min(3),
                            Constraint::Length(1),
                        ])
                        .areas(inner);

                    let surface_desc = match edit_state.surface {
                        MacroSurface::Agent => "agent macro",
                        MacroSurface::Terminal => "terminal macro",
                        MacroSurface::Both => "agent + terminal macro",
                    };
                    Paragraph::new(Line::from(Span::styled(
                        format!(" Text for the {surface_desc}:"),
                        Style::default().fg(self.theme.input_label_fg),
                    )))
                    .render(label_area, frame.buffer_mut());

                    // Draw border around the text area; pass inner rect to renderer.
                    let block = Block::default()
                        .borders(Borders::ALL)
                        .border_set(border::ROUNDED)
                        .border_style(Style::default().fg(self.theme.overlay_border));
                    let text_inner = block.inner(bordered_area);
                    block.render(bordered_area, frame.buffer_mut());

                    self.render_multiline_input(&edit_state.text_input, text_inner, frame);

                    let hints =
                        self.edit_macro_hints(&[("Enter", "newline"), ("Esc", "save & close")]);
                    Paragraph::new(Line::from(hints)).render(hint_area, frame.buffer_mut());
                }
            }
        } else {
            // ── List view ──
            let outer = self.themed_overlay_block("Text Macros");
            let inner = outer.inner(popup);
            outer.render(popup, frame.buffer_mut());

            if entries.is_empty() {
                let [msg_area, _, hint_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(2),
                        Constraint::Min(1),
                        Constraint::Length(1),
                    ])
                    .areas(inner);

                Paragraph::new(vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        " No macros defined. Press n to create one.",
                        Style::default().fg(self.theme.hint_desc_fg),
                    )),
                ])
                .render(msg_area, frame.buffer_mut());

                let hints = self.edit_macro_hints(&[("n", "new"), ("Esc", "close")]);
                Paragraph::new(Line::from(hints)).render(hint_area, frame.buffer_mut());
            } else {
                let [list_area, hint_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(1), Constraint::Length(1)])
                    .areas(inner);

                let items: Vec<ListItem> = entries
                    .iter()
                    .map(|(name, text, surface)| {
                        let surface_label = format!(" ({})", surface.label());
                        let mut spans = vec![
                            Span::styled(
                                format!(" {name}"),
                                Style::default().fg(self.theme.input_label_fg),
                            ),
                            Span::styled(
                                surface_label.clone(),
                                Style::default().fg(self.theme.hint_dim_desc_fg),
                            ),
                            Span::styled(" — ", Style::default().fg(self.theme.input_label_fg)),
                        ];
                        let text_preview = text.replace('\n', "↵");
                        // " " + name + " (label)" + " — "
                        let prefix_len = 1 + name.len() + surface_label.len() + 3;
                        let max_len = (list_area.width as usize).saturating_sub(prefix_len + 2);
                        let truncated = if text_preview.len() > max_len {
                            format!("{}…", &text_preview[..max_len.saturating_sub(1)])
                        } else {
                            text_preview
                        };
                        spans.push(Span::styled(
                            truncated,
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        ListItem::new(Line::from(spans))
                    })
                    .collect();

                let list = List::new(items)
                    .highlight_style(self.theme.selection_style())
                    .highlight_symbol("");
                let mut state = ratatui::widgets::ListState::default();
                state.select(Some(*selected));
                ratatui::prelude::StatefulWidget::render(
                    list,
                    list_area,
                    frame.buffer_mut(),
                    &mut state,
                );

                let hints = self.edit_macro_hints(&[
                    ("Enter", "edit"),
                    ("n", "new"),
                    ("d", "delete"),
                    ("Esc", "close"),
                ]);
                Paragraph::new(Line::from(hints)).render(hint_area, frame.buffer_mut());
            }
        }

        let pending_delete_snapshot = pending_delete
            .as_ref()
            .map(|p| (p.name.clone(), p.confirm_selected));
        if let Some((name, confirm_selected)) = pending_delete_snapshot {
            self.render_confirm_delete_macro(frame, &name, confirm_selected);
        }
    }

    fn render_confirm_delete_macro(
        &mut self,
        frame: &mut Frame,
        name: &str,
        confirm_selected: bool,
    ) {
        self.render_dim_overlay(frame);
        let area = centered_rect(56, 30, frame.area());
        self.clear_overlay_area(frame, area);
        let outer = self.themed_overlay_block("Delete Macro");
        let inner = outer.inner(area);
        outer.render(area, frame.buffer_mut());

        let [body_area, _, buttons_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(1),
                Constraint::Length(1),
                Constraint::Length(3),
            ])
            .areas(inner);

        let lines = vec![
            Line::from(""),
            Line::from(vec![
                Span::raw(" Are you sure you want to delete "),
                Span::styled(name, Style::default().add_modifier(Modifier::BOLD)),
                Span::raw("?"),
            ]),
            Line::from(""),
        ];
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .render(body_area, frame.buffer_mut());

        let btn_width = 16u16;
        let gap = 2u16;
        let total = btn_width * 2 + gap;
        let left_offset = buttons_area.width.saturating_sub(total) / 2;

        let cancel_area = Rect {
            x: buttons_area.x + left_offset,
            y: buttons_area.y,
            width: btn_width,
            height: 3,
        };
        let delete_area = Rect {
            x: cancel_area.x + btn_width + gap,
            y: buttons_area.y,
            width: btn_width,
            height: 3,
        };

        Button::new("Cancel")
            .kind(ButtonKind::Confirm)
            .state(button_state_for(
                ButtonPressedTarget::ConfirmDeleteMacroCancel,
                self.pressed_button,
                !confirm_selected,
                true,
            ))
            .render(frame, cancel_area, &self.theme);

        Button::new("Delete")
            .kind(ButtonKind::Danger)
            .state(button_state_for(
                ButtonPressedTarget::ConfirmDeleteMacroConfirm,
                self.pressed_button,
                confirm_selected,
                true,
            ))
            .render(frame, delete_area, &self.theme);

        self.overlay_layout.active = OverlayMouseLayout::ConfirmDeleteMacro {
            cancel_button: cancel_area,
            delete_button: delete_area,
        };
    }

    /// Render a single-line TextInput with cursor in a bordered box.
    /// Uses the terminal's hardware cursor for a blinking caret.
    fn render_single_line_input(&self, input: &TextInput, area: Rect, frame: &mut Frame) {
        let display = Line::from(Span::raw(format!(" {}", &input.text)));
        let block = Block::default()
            .borders(Borders::ALL)
            .border_set(border::ROUNDED)
            .border_style(Style::default().fg(self.theme.overlay_border));
        let inner = block.inner(area);
        Paragraph::new(display)
            .block(block)
            .render(area, frame.buffer_mut());

        // Position the hardware cursor (blinking caret).
        // Cursor column in chars + 1 for the leading space padding.
        let cursor_col = input.text[..input.cursor.min(input.text.len())]
            .chars()
            .count();
        let cx = inner.x + cursor_col as u16 + 1;
        let cy = inner.y;
        if cx < inner.x + inner.width && cy < inner.y + inner.height {
            frame.set_cursor_position((cx, cy));
        }
    }

    /// Render a multiline TextInput into the given area.
    ///
    /// The caller is responsible for drawing any border — this method renders
    /// text directly into `area`. Uses the terminal's hardware cursor for a
    /// blinking caret.
    fn render_multiline_input(&self, input: &TextInput, area: Rect, frame: &mut Frame) {
        let visible = input.visible_lines();
        let (cursor_row, cursor_col) = input.cursor_display_position();

        for (i, line_text) in visible.iter().enumerate() {
            if i >= area.height as usize {
                break;
            }
            let y = area.y + i as u16;
            let line_area = Rect::new(area.x, y, area.width, 1);
            let line = Line::from(Span::raw(format!(" {line_text}")));
            Paragraph::new(line).render(line_area, frame.buffer_mut());
        }

        // Position the hardware cursor (blinking caret).
        // +1 for the leading space padding on each line.
        let cx = area.x + cursor_col as u16 + 1;
        let cy = area.y + cursor_row as u16;
        if cx < area.x + area.width && cy < area.y + area.height {
            frame.set_cursor_position((cx, cy));
        }
    }

    /// Build hint spans from alternating key/description pairs.
    /// Each pair is (key_label, description). Spans are fully owned.
    fn edit_macro_hints(&self, pairs: &[(&str, &str)]) -> Vec<Span<'static>> {
        let mut spans = vec![Span::raw(" ")];
        for (key, desc) in pairs {
            // key_badge ties lifetime to &str, so we convert to owned spans.
            let badge = self.theme.key_badge_default(key);
            spans.extend(
                badge
                    .into_iter()
                    .map(|s| Span::styled(s.content.to_string(), s.style)),
            );
            spans.push(Span::styled(
                format!(" {desc}  "),
                Style::default().fg(self.theme.hint_desc_fg),
            ));
        }
        spans
    }

    fn render_overlay(&mut self, frame: &mut Frame) {
        match self.fullscreen_overlay {
            FullscreenOverlay::Agent => {
                self.render_fullscreen_agent(frame);
                return;
            }
            FullscreenOverlay::Terminal => {
                self.render_fullscreen_terminal(frame);
                return;
            }
            FullscreenOverlay::StartupLog => {
                self.render_fullscreen_startup_log(frame);
                return;
            }
            FullscreenOverlay::None => {}
        }
        if !matches!(self.prompt, PromptState::None) {
            self.render_prompt(frame);
            return;
        }
        if self.help_scroll.is_some() {
            self.render_help(frame);
        }
    }

    fn render_fullscreen_agent(&mut self, frame: &mut Frame) {
        self.render_dim_overlay(frame);
        let area = centered_rect(96, 94, frame.area());
        Clear.render(area, frame.buffer_mut());
        // Clear leaves cells at Color::Reset (terminal default). Repaint
        // with app_bg so the fullscreen agent surface — borders, the
        // loading card area, the gap above the hint bar — tracks the
        // active theme instead of falling through to the user's terminal
        // default.
        frame
            .buffer_mut()
            .set_style(area, Style::default().bg(self.theme.app_bg));
        let title = match self.selected_session() {
            Some(session) => {
                // Reflect the focused tab's provider in the fullscreen title.
                let provider = capitalize(self.focused_tab_provider(session).as_str());
                let name = session.title.as_deref().unwrap_or(&session.branch_name);
                let pr_suffix = self
                    .engine
                    .pr_statuses
                    .get(&session.id)
                    .map(|pr| format!(" · {}#{}", pr.owner_repo, pr.number))
                    .unwrap_or_default();
                format!(" {provider} agent · {name}{pr_suffix} ")
            }
            None => " Agent ".to_string(),
        };
        let saved = self.session_surface;
        self.session_surface = SessionSurface::Agent;
        // Display-only tab strip in fullscreen (no switching / no click Rects).
        let term_area = self.render_agent_tab_strip_if_needed(frame, area, false);
        self.render_agent_terminal(frame, term_area, &title, true);
        self.session_surface = saved;
    }

    fn render_fullscreen_terminal(&mut self, frame: &mut Frame) {
        self.render_dim_overlay(frame);
        let area = centered_rect(96, 94, frame.area());
        Clear.render(area, frame.buffer_mut());
        // Same reasoning as render_fullscreen_agent — fill with app_bg so
        // the fullscreen terminal surface follows the active theme.
        frame
            .buffer_mut()
            .set_style(area, Style::default().bg(self.theme.app_bg));
        let saved = self.session_surface;
        self.session_surface = SessionSurface::Terminal;
        self.render_agent_terminal(frame, area, " Terminal ", true);
        self.session_surface = saved;
    }

    fn render_fullscreen_startup_log(&mut self, frame: &mut Frame) {
        self.render_dim_overlay(frame);
        let area = centered_rect(96, 94, frame.area());
        Clear.render(area, frame.buffer_mut());
        frame
            .buffer_mut()
            .set_style(area, Style::default().bg(self.theme.app_bg));

        let title = self
            .startup_log_viewer
            .as_ref()
            .map(|viewer| {
                format!(
                    " Startup command log · {} · {} ",
                    viewer.scope_label, viewer.display_name
                )
            })
            .unwrap_or_else(|| " Startup command log ".to_string());
        let outer_block = self.themed_block(&title, true);
        let inner = outer_block.inner(area);
        outer_block.render(area, frame.buffer_mut());
        if inner.height < 2 || inner.width < 4 {
            return;
        }

        let [term_area, hint_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(2)])
            .areas(inner);
        self.mouse_layout.agent_term = Some(term_area);

        let Some(viewer) = &mut self.startup_log_viewer else {
            return;
        };
        let lines =
            crate::app::input::startup_command_log_visual_lines(&viewer.content, term_area.width);
        let max_scroll = u16::try_from(lines.len())
            .unwrap_or(u16::MAX)
            .saturating_sub(term_area.height);
        viewer.scroll_offset = viewer.scroll_offset.min(max_scroll);
        let query = viewer.search.text.trim().to_lowercase();

        for (row, line) in lines
            .iter()
            .skip(viewer.scroll_offset as usize)
            .take(term_area.height as usize)
            .enumerate()
        {
            let line_style = if !query.is_empty() && line.to_lowercase().contains(&query) {
                self.theme.selection_style()
            } else {
                Style::default().fg(self.theme.text_fg)
            };
            let y = term_area.y + row as u16;
            for (col, ch) in line.chars().take(term_area.width as usize).enumerate() {
                let selected = self.terminal_selection.as_ref().is_some_and(|selection| {
                    selection.anchor != selection.end
                        && selection.contains(viewer.scroll_offset + row as u16, col as u16)
                });
                let style = if selected {
                    self.theme.selection_style()
                } else {
                    line_style
                };
                frame
                    .buffer_mut()
                    .set_string(term_area.x + col as u16, y, ch.to_string(), style);
            }
        }

        let close_key = self.bindings.label_for(Action::CloseOverlay);
        let search_key = self.bindings.label_for(Action::SearchToggle);
        let scroll_up = self.bindings.labels_for(Action::ScrollPageUp);
        let scroll_down = self.bindings.labels_for(Action::ScrollPageDown);
        let open_file = self.bindings.label_for(Action::OpenStartupCommandLogFile);
        let open_folder = self.bindings.label_for(Action::OpenStartupCommandLogFolder);
        let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
        let mut spans = Vec::new();
        if viewer.searching {
            spans.extend(self.theme.dim_key_badge_default(&close_key));
            spans.push(Span::styled(" close search", desc_style));
        } else {
            spans.extend(self.theme.dim_key_badge_default(&close_key));
            spans.push(Span::styled(" close  ", desc_style));
            spans.extend(self.theme.dim_key_badge_default(&scroll_up));
            spans.push(Span::styled("/", desc_style));
            spans.extend(self.theme.dim_key_badge_default(&scroll_down));
            spans.push(Span::styled(" scroll  ", desc_style));
            spans.extend(self.theme.dim_key_badge_default(&search_key));
            spans.push(Span::styled(" search  ", desc_style));
            spans.extend(self.theme.dim_key_badge_default(&open_file));
            spans.push(Span::styled(" Open file  ", desc_style));
            spans.extend(self.theme.dim_key_badge_default(&open_folder));
            spans.push(Span::styled(" Open folder", desc_style));
        }
        Paragraph::new(Line::from(spans))
            .block(
                Block::default()
                    .borders(Borders::TOP)
                    .border_style(Style::default().fg(self.theme.border_normal)),
            )
            .render(hint_area, frame.buffer_mut());

        if self
            .startup_log_viewer
            .as_ref()
            .is_some_and(|viewer| viewer.searching)
        {
            self.render_startup_log_search_bar(frame, inner);
        }
    }

    fn render_startup_log_search_bar(&mut self, frame: &mut Frame, area: Rect) {
        let Some(viewer) = &self.startup_log_viewer else {
            return;
        };
        let query = viewer.search.text.clone();
        let cursor = viewer.search.cursor.min(viewer.search.text.len());
        if area.height < 3 {
            return;
        }

        let bar_area = Rect::new(
            area.x,
            area.y + area.height.saturating_sub(3),
            area.width,
            3,
        );
        self.clear_overlay_area(frame, bar_area);

        let mut bottom_spans = vec![Span::raw(" ")];
        for (key, desc) in &[("Enter", "done"), ("Esc", "cancel")] {
            let badge = self.theme.key_badge_default(key);
            bottom_spans.extend(
                badge
                    .into_iter()
                    .map(|s| Span::styled(s.content.to_string(), s.style)),
            );
            bottom_spans.push(Span::styled(
                format!(" {desc}  "),
                Style::default().fg(self.theme.hint_desc_fg),
            ));
        }

        let input_block = self
            .themed_overlay_block("Search log")
            .title_bottom(Line::from(bottom_spans));
        let input_inner = input_block.inner(bar_area);
        Paragraph::new(render_single_line_cursor_input(
            "/ ",
            &query,
            cursor,
            self.theme.input_cursor_fg,
            self.theme.input_cursor_bg,
        ))
        .block(input_block)
        .render(bar_area, frame.buffer_mut());

        let cursor_col = query[..cursor].chars().count() + 2;
        let cx = input_inner.x + cursor_col as u16;
        let cy = input_inner.y;
        if cx < input_inner.x + input_inner.width && cy < input_inner.y + input_inner.height {
            frame.set_cursor_position((cx, cy));
        }
    }

    fn render_macro_bar(&mut self, frame: &mut Frame, area: Rect) {
        let (query, selected, cursor, cursor_fg, cursor_bg) = {
            let Some(bar) = &self.macro_bar else {
                return;
            };
            (
                bar.input.text.clone(),
                bar.selected,
                bar.input.cursor.min(bar.input.text.len()),
                self.theme.input_cursor_fg,
                self.theme.input_cursor_bg,
            )
        };

        let filtered = self.filtered_macros(&query);

        // Compute total height: input block (3) + list block (variable).
        // The list block shares borders with the input block (no top border).
        let list_content_h = (filtered.len() as u16).clamp(1, 8);
        // list block = content + bottom border (1). Left/right borders are sides.
        let list_block_h = list_content_h + 1; // +1 for bottom border
        let input_block_h: u16 = 3; // top border + input + bottom border (shared with list top)
        let total_h = (input_block_h + list_block_h).min(area.height);

        if area.height < 4 {
            return;
        }

        // Bottom-anchor the bar.
        let bar_area = Rect::new(
            area.x,
            area.y + area.height.saturating_sub(total_h),
            area.width,
            total_h,
        );
        self.clear_overlay_area(frame, bar_area);

        // Split into input area (top) and list area (bottom).
        let [input_area, list_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(input_block_h), Constraint::Min(1)])
            .areas(bar_area);

        // ── Input block (top, with title and hint badges) ──
        let mut bottom_spans = vec![Span::raw(" ")];
        for (key, desc) in &[("Enter", "send"), ("Tab", "complete"), ("Esc", "cancel")] {
            let badge = self.theme.key_badge_default(key);
            bottom_spans.extend(
                badge
                    .into_iter()
                    .map(|s| Span::styled(s.content.to_string(), s.style)),
            );
            bottom_spans.push(Span::styled(
                format!(" {desc}  "),
                Style::default().fg(self.theme.hint_desc_fg),
            ));
        }

        let input_block = self
            .themed_overlay_block("Macros")
            .title_bottom(Line::from(bottom_spans));
        let input_inner = input_block.inner(input_area);
        Paragraph::new(render_single_line_cursor_input(
            "", &query, cursor, cursor_fg, cursor_bg,
        ))
        .block(input_block)
        .render(input_area, frame.buffer_mut());

        // Place hardware cursor inside the input.
        let cursor_col = query[..cursor].chars().count();
        let cx = input_inner.x + cursor_col as u16;
        let cy = input_inner.y;
        if cx < input_inner.x + input_inner.width && cy < input_inner.y + input_inner.height {
            frame.set_cursor_position((cx, cy));
        }

        // ── List block (bottom, connected borders) ──
        let name_col = filtered
            .iter()
            .map(|&(name, _)| name.chars().count())
            .max()
            .unwrap_or(0);
        let inner_w = list_area.width.saturating_sub(3) as usize; // borders + padding
        let gap = 2usize;

        let items: Vec<ListItem> = if filtered.is_empty() {
            let msg = "No matching macros.";
            vec![ListItem::new(Span::styled(
                msg,
                Style::default().fg(self.theme.hint_desc_fg),
            ))]
        } else {
            filtered
                .iter()
                .map(|&(name, text)| {
                    let name_padded = format!("{name:name_col$}");
                    let mut spans = vec![Span::styled(
                        name_padded,
                        Style::default()
                            .fg(self.theme.help_section_header_fg)
                            .add_modifier(Modifier::BOLD),
                    )];
                    let text_preview = text.replace('\n', "↵");
                    let desc_avail = inner_w.saturating_sub(name_col + gap);
                    let desc_display =
                        if text_preview.chars().count() > desc_avail && desc_avail > 1 {
                            let end = text_preview
                                .char_indices()
                                .nth(desc_avail - 1)
                                .map(|(i, _)| i)
                                .unwrap_or(text_preview.len());
                            format!("  {}\u{2026}", &text_preview[..end])
                        } else {
                            format!("  {text_preview:desc_avail$}")
                        };
                    spans.push(Span::styled(
                        desc_display,
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    ListItem::new(Line::from(spans))
                })
                .collect()
        };

        let list_block = Block::default()
            .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
            .border_type(ratatui::widgets::BorderType::Rounded)
            .border_style(Style::default().fg(self.theme.overlay_border))
            .style(Style::default().bg(self.theme.overlay_bg));
        let mut list_state = ListState::default();
        if !filtered.is_empty() {
            list_state.select(Some(selected));
        }
        StatefulWidget::render(
            List::new(items)
                .block(list_block)
                .highlight_style(self.theme.selection_style()),
            list_area,
            frame.buffer_mut(),
            &mut list_state,
        );
    }

    fn themed_block<'a>(&self, title: &'a str, focused: bool) -> Block<'a> {
        Block::default()
            .title(Line::from(Span::styled(
                title,
                self.theme.title_style(focused),
            )))
            .borders(Borders::ALL)
            .border_set(border::ROUNDED)
            .border_style(self.theme.border_style(focused))
    }

    /// Reset `area` to a blank surface and fill it with the modal overlay
    /// background. Use this in place of a bare `Clear.render(area, ..)` for
    /// every modal popup so the entire popup — including any strip outside
    /// the inner `themed_overlay_block` (footer hints, gaps between stacked
    /// blocks, etc.) — tracks the active theme rather than reading whatever
    /// `Color::Reset` falls through to in the user's terminal.
    ///
    /// Fullscreen surfaces (the agent and terminal fullscreen overlays) want
    /// `app_bg` instead of `overlay_bg` and stay open-coded.
    fn clear_overlay_area(&self, frame: &mut Frame, area: Rect) {
        Clear.render(area, frame.buffer_mut());
        frame
            .buffer_mut()
            .set_style(area, Style::default().bg(self.theme.overlay_bg));
    }

    fn themed_overlay_block<'a>(&self, title: &'a str) -> Block<'a> {
        Block::default()
            .title(Line::from(Span::styled(
                title,
                Style::default()
                    .fg(self.theme.input_label_fg)
                    .add_modifier(Modifier::BOLD),
            )))
            .borders(Borders::ALL)
            .border_set(border::ROUNDED)
            .border_style(Style::default().fg(self.theme.overlay_border))
            // Modals are presented after a `Clear.render(..)` which resets the
            // popup cells to `Color::Reset`. Filling the block with overlay_bg
            // means the modal interior — borders, surrounding chrome, the gap
            // around the inner widgets — tracks the active theme instead of
            // reading terminal-default behind the border ring.
            .style(Style::default().bg(self.theme.overlay_bg))
    }

    fn center_pane_agent_title(&self) -> String {
        if let Some(session) = self.selected_session() {
            // Reflect the FOCUSED tab's provider, not just the Main one.
            let provider = capitalize(self.focused_tab_provider(session).as_str());
            let base = format!("{provider} agent");
            let count = self.session_terminal_count(&session.id);
            if count == 1 {
                return format!("{base} (+ 1 terminal)");
            } else if count > 1 {
                return format!("{base} (+ {count} terminals)");
            }
            return base;
        }
        "Agent".to_string()
    }

    fn render_resource_monitor(
        &mut self,
        frame: &mut Frame,
        rows: &[ResourceStats],
        scroll_offset: u16,
        selected_row: usize,
        expanded: &HashSet<u32>,
        short_window_sample: bool,
    ) {
        use ratatui::widgets::{Cell, Row, Table, TableState};

        self.render_dim_overlay(frame);
        let popup = centered_rect(85, 78, frame.area());
        self.clear_overlay_area(frame, popup);

        let chunks = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(popup);
        let content_area = chunks[0];
        let hint_area = chunks[1];

        let block = self.themed_overlay_block(" Resource Monitor ");
        let inner = block.inner(content_area);
        block.render(content_area, frame.buffer_mut());

        let header_style = Style::default()
            .fg(self.theme.help_section_header_fg)
            .add_modifier(Modifier::BOLD);

        // Rebalance the column budget from the real inner width instead of
        // hardcoding rigid widths for PID/Procs/CPU/RSS: at narrow terminal
        // sizes that left Name (the only flexible column) with only a
        // couple of characters. See `resource_monitor_columns` for the
        // rationale (CPU/RSS always survive; PID drops first, then Procs).
        let columns = resource_monitor_columns(inner.width);
        let name_w = usize::from(columns.name_w);

        let mut header_cells = vec![Cell::from("Name").style(header_style)];
        if columns.show_pid {
            header_cells.push(Cell::from("PID").style(header_style));
        }
        if columns.show_procs {
            header_cells.push(Cell::from("Procs").style(header_style));
        }
        header_cells.push(Cell::from("CPU %").style(header_style));
        header_cells.push(Cell::from("RSS").style(header_style));
        let header = Row::new(header_cells);

        let visual = build_visual_rows(rows, expanded);
        let dim_style = Style::default().fg(self.theme.hint_dim_desc_fg);

        let table_rows: Vec<Row> = visual
            .iter()
            .map(|vr| match vr {
                VisualRow::Parent(idx) => {
                    let stat = &rows[*idx];
                    let pid_str = stat.pid.map(|p| p.to_string()).unwrap_or_default();
                    // `~` marks a genuinely different, real measurement: the
                    // collector had to re-establish its CPU baseline for this
                    // sample (freshly opened, or reopened after a gap), so
                    // the reading spans only sysinfo's short
                    // MINIMUM_CPU_UPDATE_INTERVAL window rather than the
                    // monitor's normal ~2s poll interval. Real numbers, just
                    // noisier because they cover less wall-clock time; see
                    // `ResourceCollector::sample`'s `was_baseline`.
                    let cpu_str = if short_window_sample {
                        format!("~{:.1}%", stat.cpu_percent)
                    } else {
                        format!("{:.1}%", stat.cpu_percent)
                    };
                    let rss_str = format_bytes(stat.rss_bytes);
                    let procs_str = stat.process_count.to_string();

                    // Classify by `kind`, never by matching the label string:
                    // an agent or terminal literally titled "TOTAL" is a
                    // real, distinct row and must not render as the totals
                    // row just because its name happens to match.
                    let is_total = stat.kind == ResourceKind::Total;
                    let style = if is_total {
                        Style::default().add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                    };

                    // Expand indicator: ▶/▼ for expandable rows, space for
                    // others. Gated on `has_breakdown()` (core's rule), NOT on
                    // `children` being non-empty: `children` always contains
                    // the root itself, so a leaf process has exactly one entry
                    // and an is-empty test marks every row expandable. Expanding
                    // one then reveals a single child that is a duplicate of the
                    // row just expanded.
                    let indicator = if stat.has_breakdown() {
                        if let Some(pid) = stat.pid {
                            if expanded.contains(&pid) {
                                "▼ "
                            } else {
                                "▶ "
                            }
                        } else {
                            "  "
                        }
                    } else {
                        "  "
                    };
                    let label = truncate_status_text(&format!("{indicator}{}", stat.label), name_w);

                    let mut cells = vec![Cell::from(label)];
                    if columns.show_pid {
                        cells.push(Cell::from(pid_str));
                    }
                    if columns.show_procs {
                        cells.push(Cell::from(procs_str));
                    }
                    cells.push(Cell::from(cpu_str));
                    cells.push(Cell::from(rss_str));
                    Row::new(cells).style(style)
                }
                VisualRow::Child(parent_idx, child_idx) => {
                    let child = &rows[*parent_idx].children[*child_idx];
                    let is_last = *child_idx == rows[*parent_idx].children.len().saturating_sub(1);
                    let connector = if is_last { "└" } else { "├" };
                    // The root is part of its own breakdown on purpose (that is
                    // what makes the rows sum to the parent's total), so say so
                    // instead of letting it read as a phantom duplicate of the
                    // row above.
                    let name = if child.is_root {
                        format!("{} (this process)", child.name)
                    } else {
                        child.name.clone()
                    };
                    let label = truncate_status_text(&format!("    {connector} {name}"), name_w);
                    let cpu_str = if short_window_sample {
                        format!("~{:.1}%", child.cpu_percent)
                    } else {
                        format!("{:.1}%", child.cpu_percent)
                    };
                    let rss_str = format_bytes(child.rss_bytes);

                    let mut cells = vec![Cell::from(label)];
                    if columns.show_pid {
                        cells.push(Cell::from(child.pid.to_string()));
                    }
                    if columns.show_procs {
                        cells.push(Cell::from(""));
                    }
                    cells.push(Cell::from(cpu_str));
                    cells.push(Cell::from(rss_str));
                    Row::new(cells).style(dim_style)
                }
            })
            .collect();

        let mut widths = vec![Constraint::Length(columns.name_w)];
        if columns.show_pid {
            widths.push(Constraint::Length(columns.pid_w));
        }
        if columns.show_procs {
            widths.push(Constraint::Length(columns.procs_w));
        }
        widths.push(Constraint::Length(columns.cpu_w));
        widths.push(Constraint::Length(columns.rss_w));
        debug_assert_eq!(widths.len() as u16, columns.visible_count());

        let highlight_style = self.theme.selection_style();
        let table = Table::new(table_rows, widths)
            .header(header)
            .row_highlight_style(highlight_style);

        let mut table_state = TableState::default()
            .with_offset(scroll_offset as usize)
            .with_selected(Some(selected_row));
        StatefulWidget::render(table, inner, frame.buffer_mut(), &mut table_state);

        let row_area = Rect::new(
            inner.x,
            inner.y.saturating_add(1),
            inner.width,
            inner.height.saturating_sub(1),
        );
        self.overlay_layout.active = OverlayMouseLayout::ResourceMonitor {
            list: row_area,
            items: visual.len(),
            offset: table_state.offset(),
        };

        // Footer hint.
        let close_key = self.bindings.label_for(Action::CloseOverlay);
        let desc_style = Style::default().fg(self.theme.hint_desc_fg);
        let mut spans = vec![Span::raw(" ")];
        spans.extend(self.theme.key_badge_default(&close_key));
        spans.push(Span::styled(" close  ", desc_style));
        spans.extend(self.theme.key_badge_default("Enter"));
        spans.push(Span::styled(" expand/collapse  ", desc_style));
        spans.extend(self.theme.key_badge_default("Scroll"));
        spans.push(Span::styled(" navigate  ", desc_style));
        spans.push(Span::styled(
            "refreshes every ~2s",
            Style::default().fg(self.theme.hint_dim_desc_fg),
        ));
        let hint_para =
            Paragraph::new(Line::from(spans)).alignment(ratatui::layout::Alignment::Center);
        hint_para.render(hint_area, frame.buffer_mut());
    }

    fn render_dim_overlay(&self, frame: &mut Frame) {
        let full = frame.area();
        // Keep the statusline (bottom rows) undimmed so errors stay visible.
        let status_text = self
            .status
            .most_recent_tui()
            .map_or(String::new(), |(_, t)| t);
        let status_lines = status_footer_lines(&status_text, full.width);
        let footer_h = 1 + status_lines; // hints bar + status line(s)
        let dim_h = full.height.saturating_sub(footer_h);
        let buf = frame.buffer_mut();
        for y in full.y..full.y + dim_h {
            for x in full.x..full.x + full.width {
                let cell = &mut buf[(x, y)];
                cell.set_fg(self.theme.overlay_dim_fg);
                cell.set_bg(self.theme.overlay_dim_bg);
            }
        }
    }
}

/// Convert a [`Color`] to its grayscale equivalent using ITU-R BT.601
/// luminance weights (0.299 R + 0.587 G + 0.114 B).
///
/// - `Color::Rgb` values are converted directly.
/// - `Color::Indexed` values are resolved through the standard xterm-256
///   palette before conversion.
/// - All other variants (`Reset`, named ANSI colors, etc.) are returned as-is.
fn to_grayscale(color: Color) -> Color {
    match color {
        Color::Rgb(r, g, b) => {
            let l = (0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64).round() as u8;
            Color::Rgb(l, l, l)
        }
        Color::Indexed(idx) => {
            let (r, g, b) = xterm256_to_rgb(idx);
            let l = (0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64).round() as u8;
            Color::Rgb(l, l, l)
        }
        other => other,
    }
}

/// Resolve a xterm-256 palette index to (R, G, B).
fn xterm256_to_rgb(idx: u8) -> (u8, u8, u8) {
    #[rustfmt::skip]
    const BASIC: [(u8, u8, u8); 16] = [
        (0,0,0),       (128,0,0),     (0,128,0),     (128,128,0),
        (0,0,128),     (128,0,128),   (0,128,128),   (192,192,192),
        (128,128,128), (255,0,0),     (0,255,0),     (255,255,0),
        (0,0,255),     (255,0,255),   (0,255,255),   (255,255,255),
    ];

    match idx {
        0..=15 => BASIC[idx as usize],
        16..=231 => {
            let val = idx - 16;
            let r_idx = val / 36;
            let g_idx = (val % 36) / 6;
            let b_idx = val % 6;
            let to_level = |i: u8| if i == 0 { 0 } else { 55 + 40 * i };
            (to_level(r_idx), to_level(g_idx), to_level(b_idx))
        }
        232..=255 => {
            let level = 8 + 10 * (idx - 232);
            (level, level, level)
        }
    }
}

/// Choose foreground/background colors for a PTY cell.
///
/// In interactive mode (`is_input == true`) the cell's original colors are
/// returned. In non-interactive mode the foreground is replaced with the
/// theme's dim color and the background is converted to grayscale, giving
/// the pane a muted appearance that signals it is read-only.
///
/// Resolve the foreground/background a single PTY cell should render with.
///
/// The bg is `Option<Color>` to give the caller a way to say "leave the
/// underlying buffer background alone": the rendering loop only applies bg
/// when this is `Some`, so `None` lets whatever was already in the buffer
/// (the frame-wide `app_bg` pre-fill, the dim overlay, the parent block
/// fill, …) show through.
///
/// Interactive mode (fullscreen agent or focused agent/terminal) always
/// passes the CLI's colors through unchanged so the user's configured
/// palette renders end-to-end without dux fighting it.
///
/// Non-interactive (minimized / read-only) mode dims the foreground to a
/// single muted shade. Cells the CLI explicitly colored have their
/// background grayscaled, which is what produces the recognizable "this
/// pane is read-only" look. Cells the CLI emitted with `Color::Reset` are
/// returned with `bg = None` so the dim overlay / app surface continues to
/// show through them — only the solid CLI-painted backgrounds are
/// grayscaled, not the ambient surface around them.
fn pty_cell_colors(fg: Color, bg: Color, is_input: bool, theme: &Theme) -> (Color, Option<Color>) {
    if is_input {
        (fg, Some(bg))
    } else if bg == Color::Reset {
        (theme.overlay_dim_fg, None)
    } else {
        (theme.overlay_dim_fg, Some(to_grayscale(bg)))
    }
}

fn format_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;
    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.0} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{bytes} B")
    }
}

fn quit_process_description(agents: usize, terminals: usize) -> String {
    match (agents, terminals) {
        (0, 1) => "1 running terminal".to_string(),
        (0, t) => format!("{t} running terminals"),
        (1, 0) => "1 running agent".to_string(),
        (a, 0) => format!("{a} running agents"),
        (a, t) => {
            let agent_word = if a == 1 { "agent" } else { "agents" };
            let term_word = if t == 1 { "terminal" } else { "terminals" };
            format!("{a} running {agent_word} and {t} {term_word}")
        }
    }
}

/// The trailing counter in a "Terminal N" label, used only to disambiguate two
/// terminals running the same app (see `terminal_dup_suffix`). `None` for a
/// label without a trailing number, which the engine never produces but keeps
/// the helper total.
fn terminal_number(label: &str) -> Option<u32> {
    label.rsplit(' ').next()?.parse().ok()
}

/// The suffix appended after a running terminal's command in the left pane.
/// Empty when the command is unique among the terminals shown together;
/// otherwise " (#N)" using the terminal's counter so two terminals running the
/// same app stay distinct ("vim (#1)", "vim (#2)"), falling back to the label
/// in parentheses if it carries no number. Mirrors the web `terminalTitle`
/// rule (crates/dux-web/web/src/lib/terminals.ts).
fn terminal_dup_suffix(label: &str, duplicate: bool) -> String {
    if !duplicate {
        return String::new();
    }
    match terminal_number(label) {
        Some(n) => format!(" (#{n})"),
        None => format!(" ({label})"),
    }
}

fn companion_terminal_status_meta(status: CompanionTerminalStatus) -> (&'static str, &'static str) {
    match status {
        CompanionTerminalStatus::NotLaunched => ("○", "not launched"),
        CompanionTerminalStatus::Running => ("●", "running"),
        CompanionTerminalStatus::Exited => ("◐", "exited"),
    }
}

fn companion_terminal_status_color(theme: &Theme, status: CompanionTerminalStatus) -> Color {
    match status {
        CompanionTerminalStatus::NotLaunched => theme.terminal_hint_fg,
        CompanionTerminalStatus::Running => theme.session_active,
        CompanionTerminalStatus::Exited => theme.session_detached,
    }
}

/// Format additions/deletions as right-aligned colored spans.
/// Returns an empty vec when both counts are zero for text files.
pub(crate) fn format_line_stats(
    additions: usize,
    deletions: usize,
    binary: bool,
    theme: &crate::theme::Theme,
) -> Vec<Span<'static>> {
    if binary {
        return vec![Span::styled(
            "bin",
            Style::default().fg(theme.diff_binary_fg),
        )];
    }
    if additions == 0 && deletions == 0 {
        return Vec::new();
    }
    let mut spans = Vec::new();
    if additions > 0 {
        spans.push(Span::styled(
            format!("+{additions}"),
            Style::default().fg(theme.diff_stat_add_fg),
        ));
    }
    if additions > 0 && deletions > 0 {
        spans.push(Span::raw(" "));
    }
    if deletions > 0 {
        spans.push(Span::styled(
            format!("-{deletions}"),
            Style::default().fg(theme.diff_stat_remove_fg),
        ));
    }
    spans
}

/// Compact top-bar branch value for an agent (no label prefix — the caller owns
/// the themed "agent: " label). Returns just `<current>` normally; when the
/// current branch has drifted from the branch the agent was created on, it
/// appends `(orig: <initial>)` so the original is visible in the tight header.
/// Pure and unit-tested; the header renderer styles the pieces.
pub(crate) fn top_bar_branch_suffix(current: &str, initial: &str) -> String {
    if branch_drifted(current, initial) {
        format!("{current} (orig: {initial})")
    } else {
        current.to_string()
    }
}

pub(crate) fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area)[1];
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical)[1]
}

pub(crate) fn centered_rect_exact(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.min(area.width.max(1));
    let height = height.min(area.height.max(1));
    let x = area.x + area.width.saturating_sub(width) / 2;
    let y = area.y + area.height.saturating_sub(height) / 2;
    Rect::new(x, y, width, height)
}

fn render_single_line_cursor_input(
    prefix: &str,
    text: &str,
    cursor: usize,
    cursor_fg: Color,
    cursor_bg: Color,
) -> Line<'static> {
    let cursor = cursor.min(text.len());
    if cursor < text.len() {
        let (before, after) = text.split_at(cursor);
        let cursor_char = after.chars().next().expect("cursor within text");
        let cursor_len = cursor_char.len_utf8();
        let rest = &after[cursor_len..];
        Line::from(vec![
            Span::raw(prefix.to_string()),
            Span::raw(before.to_string()),
            Span::styled(
                cursor_char.to_string(),
                Style::default().fg(cursor_fg).bg(cursor_bg),
            ),
            Span::raw(rest.to_string()),
        ])
    } else {
        Line::from(vec![
            Span::raw(format!("{prefix}{text}")),
            Span::styled(" ", Style::default().fg(cursor_fg).bg(cursor_bg)),
        ])
    }
}

fn runtime_context_spans(
    context: &str,
    prose_style: Style,
    quoted_style: Style,
) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut buf = String::new();
    let mut in_quotes = false;

    for ch in context.chars() {
        if ch == '"' {
            if in_quotes {
                buf.push(ch);
                spans.push(Span::styled(std::mem::take(&mut buf), quoted_style));
                in_quotes = false;
            } else {
                if !buf.is_empty() {
                    spans.push(Span::styled(std::mem::take(&mut buf), prose_style));
                }
                buf.push(ch);
                in_quotes = true;
            }
        } else {
            buf.push(ch);
        }
    }

    if !buf.is_empty() {
        let style = if in_quotes { quoted_style } else { prose_style };
        spans.push(Span::styled(buf, style));
    }

    spans
}

fn scrollback_indicator_label(scrolled: usize, total: usize) -> Option<String> {
    if scrolled == 0 {
        return None;
    }

    let total = total.max(scrolled);
    let noun = if total == 1 { "line" } else { "lines" };
    Some(format!(" {scrolled}/{total} {noun} "))
}

fn path_completion_display_label(completion: &str) -> String {
    let trimmed = completion.trim_end_matches('/');
    let Some(folder) = Path::new(trimmed)
        .file_name()
        .and_then(|part| part.to_str())
    else {
        return completion.to_string();
    };

    format!(".../{folder}/")
}

impl App {
    /// Render the GitHub PR pill as a single-line pill using Unicode
    /// half-block characters for the caps and a solid background:
    /// `▐ owner/repo#1234 │ PR title ellipsized… ▌`
    ///
    /// The left cap `▐` (U+2590) paints the right half of the cell in the
    /// state color; the right cap `▌` (U+258C) paints the left half. This
    /// creates a pill-like shape without requiring Powerline/Nerd Fonts.
    /// The `│` divider uses terminal default colors so it blends with the
    /// user's background.
    fn render_pr_banner(&self, frame: &mut Frame, area: Rect, pr: &crate::model::PrInfo) {
        use crate::model::PrState;

        if area.height < 1 || area.width < 6 {
            return;
        }

        // Paint the row with the theme's app background first so the
        // half-block caps blend into a surface that actually changes with
        // the theme. Without this the caps render on top of whatever
        // cell colors the agent PTY left behind, which made the pill look
        // detached on light themes.
        frame
            .buffer_mut()
            .set_style(area, Style::default().bg(self.theme.app_bg));

        let bg = match pr.state {
            PrState::Open => self.theme.pr_open_bg,
            PrState::Merged => self.theme.pr_merged_bg,
            PrState::Closed => self.theme.pr_closed_bg,
        };
        let fg = self.theme.pr_banner_fg;
        // Half-block caps: fg is the pill color, bg is the freshly-painted
        // app surface so the pill arc sits on a theme-driven backdrop
        // instead of the terminal default.
        let cap_style = Style::default().fg(bg).bg(self.theme.app_bg);
        // Inner content: white text on colored background.
        let text_style = Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD);
        let title_style = Style::default()
            .fg(fg)
            .bg(bg)
            .add_modifier(Modifier::ITALIC);
        let fill_style = Style::default().fg(fg).bg(bg);

        // Half-block caps (universally supported Unicode).
        let left_cap = "\u{2590}"; // ▐ — right half block
        let right_cap = "\u{258c}"; // ▌ — left half block

        // Build the pill content as a single string.
        // With title:    " ⎇ owner/repo#1234 ▸ PR title here… "
        // Without title: " ⎇ owner/repo#1234 "
        let prefix = format!(" \u{2387} {}#{}", pr.owner_repo, pr.number);
        let title_trimmed = pr.title.trim();
        let has_title = !title_trimmed.is_empty();

        // Available content width inside the pill (between the two caps).
        let avail = area.width as usize;
        let inner_w = avail.saturating_sub(2); // 2 = left cap + right cap
        if inner_w < 4 {
            return;
        }

        let prefix_w = prefix.chars().count();
        let buf = frame.buffer_mut();
        let y = area.y;
        let sx = area.x;
        let mut x = sx;

        // Left cap.
        set_cell(buf, x, y, left_cap, cap_style);
        x += 1;

        if !has_title || inner_w <= prefix_w + 4 {
            // No title or not enough room — render just the prefix, padded.
            // " ⎇ owner/repo#1234 "
            let content = format!("{prefix} ");
            for ch in content.chars() {
                if (x - sx) as usize > inner_w {
                    break;
                }
                set_cell(buf, x, y, &ch.to_string(), text_style);
                x += 1;
            }
            // Fill remaining space.
            while (x - sx) as usize <= inner_w {
                set_cell(buf, x, y, " ", fill_style);
                x += 1;
            }
        } else {
            // Render prefix + arrow + title.
            // " ⎇ owner/repo#1234 ▸ PR title here "
            let arrow = " \u{2192} "; // " ▸ "
            let arrow_w = arrow.chars().count();

            // Write prefix.
            for ch in prefix.chars() {
                set_cell(buf, x, y, &ch.to_string(), text_style);
                x += 1;
            }

            // Write arrow separator.
            for ch in arrow.chars() {
                set_cell(buf, x, y, &ch.to_string(), fill_style);
                x += 1;
            }

            // Remaining space for the title + trailing space.
            let used = prefix_w + arrow_w;
            let title_budget = inner_w.saturating_sub(used + 1); // +1 for trailing space

            // Write title, ellipsized if needed.
            let title_w = title_trimmed.chars().count();
            if title_w > title_budget {
                for (i, ch) in title_trimmed.chars().enumerate() {
                    if i + 1 >= title_budget {
                        set_cell(buf, x, y, "…", title_style);
                        x += 1;
                        break;
                    }
                    set_cell(buf, x, y, &ch.to_string(), title_style);
                    x += 1;
                }
            } else {
                for ch in title_trimmed.chars() {
                    set_cell(buf, x, y, &ch.to_string(), title_style);
                    x += 1;
                }
            }

            // Fill remaining space to the right cap.
            while (x - sx) as usize <= inner_w {
                set_cell(buf, x, y, " ", fill_style);
                x += 1;
            }
        }

        // Right cap.
        set_cell(buf, x, y, right_cap, cap_style);
    }
}

/// A short, stable id for an OSC 8 hyperlink URI, so every cell of the same link
/// carries the same `id=` and a host terminal can merge them into one clickable
/// span. A hash (not the URI itself) keeps the id short; collisions only ever merge
/// two distinct links visually, never a safety concern.
fn osc8_link_id(uri: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    uri.hash(&mut hasher);
    hasher.finish()
}

/// Wrap a cell symbol in a self-contained OSC 8 open/close pair pointing at `uri`.
/// The `id=` (a hash of the URI) lets a host terminal merge adjacent cells sharing
/// the same link into one clickable span. The escape bytes are zero-width.
fn osc8_wrap_symbol(symbol: &str, uri: &str) -> String {
    let id = osc8_link_id(uri);
    format!("\x1b]8;id={id};{uri}\x1b\\{symbol}\x1b]8;;\x1b\\")
}

/// Set a single cell in the buffer, bounds-checked.
fn set_cell(buf: &mut ratatui::buffer::Buffer, x: u16, y: u16, symbol: &str, style: Style) {
    let area = buf.area();
    if x >= area.x + area.width || y >= area.y + area.height {
        return;
    }
    buf[(x, y)].set_symbol(symbol).set_style(style);
}

/// Truncate `text` to at most `available` **characters**, appending `…` when
/// trimmed. Using char-based counting avoids panics when the text contains
/// multi-byte UTF-8 (e.g. box-drawing or block characters).
fn truncate_status_text(text: &str, available: usize) -> String {
    let char_count = text.chars().count();
    if char_count <= available {
        return text.to_owned();
    }

    match available {
        0 => String::new(),
        1 => "…".to_string(),
        _ => {
            let mut truncated: String = text.chars().take(available - 1).collect();
            truncated.push('…');
            truncated
        }
    }
}

fn status_footer_lines(status_text: &str, width: u16) -> u16 {
    if width == 0 {
        return 1;
    }
    let status_text_len = status_text.chars().count() + 3; // " ● " prefix
    if status_text_len > width as usize {
        2
    } else {
        1
    }
}

/// The `ConfirmCloseTab` dialog's warning tail must match what
/// `resolve_confirm_close_tab` actually does, not just whether this is the
/// agent's only tab: closing the session-slot tab of a MULTI-tab agent only
/// stops that tab (no `agent_tabs` row to delete, the agent keeps running via
/// its siblings, and the session-slot tab itself stays reopenable) —
/// non-destructive, unlike closing an extra tab, which permanently deletes
/// that tab's row.
fn confirm_close_tab_tail(only_tab: bool, is_main: bool) -> &'static str {
    if only_tab {
        " It's this agent's only tab, so the agent detaches and stays in Projects, reopenable."
    } else if is_main {
        " Other tabs on this agent keep running; this tab stops and can be reopened fresh from the agent."
    } else {
        " dux can't reopen this exact conversation — a recent one can be recovered from a fresh tab via your provider's own history command."
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::app::test_support::{default_bindings, test_app, wait_for_agent_cursor};
    use crate::model::{CompanionTerminal, SessionSurface};
    use crate::pty::PtyClient;

    #[test]
    fn project_tag_kind_classifies_healthy_path_missing_and_orphan() {
        let app = test_app(default_bindings());
        let healthy = app.engine.projects[0].clone();
        assert_eq!(project_tag_kind(Some(&healthy)), ProjectTagKind::Healthy);

        let mut missing = healthy.clone();
        missing.path_missing = true;
        assert_eq!(
            project_tag_kind(Some(&missing)),
            ProjectTagKind::PathMissing
        );

        assert_eq!(project_tag_kind(None), ProjectTagKind::Orphan);
    }

    #[test]
    fn agent_state_word_mirrors_the_web_states() {
        use crate::model::SessionStatus::{Active, Detached, Exited};
        assert_eq!(agent_state_word(Active, true, false), "Working");
        assert_eq!(agent_state_word(Active, false, false), "Idle");
        assert_eq!(agent_state_word(Detached, false, false), "Detached");
        assert_eq!(agent_state_word(Exited, false, false), "Exited");
        // Needs-attention wins over every other state, including working.
        assert_eq!(agent_state_word(Active, true, true), "Needs you");
        assert_eq!(agent_state_word(Detached, false, true), "Needs you");
    }

    #[test]
    fn left_row_to_item_maps_two_line_rows_and_scroll() {
        // Two agents (2 lines each) then the Inactive toggle (1 line).
        let heights = [2u16, 2, 1];
        // No scroll: rows 0,1 -> item 0; rows 2,3 -> item 1; row 4 -> item 2.
        assert_eq!(left_row_to_item(0, &heights, 5), vec![0, 0, 1, 1, 2]);
        // A shorter area caps the map at the visible height.
        assert_eq!(left_row_to_item(0, &heights, 3), vec![0, 0, 1]);
        // Scrolled past the first agent: the window starts at item 1.
        assert_eq!(left_row_to_item(1, &heights, 5), vec![1, 1, 2]);
        // No items -> empty map.
        assert!(left_row_to_item(0, &[], 5).is_empty());
    }

    #[test]
    fn path_missing_project_renders_warning_tag_in_row() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        app.engine.projects[0].path_missing = true;
        // Widen the left pane so the tag is not truncated off the row.
        app.left_width_pct = 60;

        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let rendered: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(
            rendered.contains("⚠ demo"),
            "a path-missing project must render its tag with a warning marker",
        );
    }

    #[test]
    fn path_missing_project_renders_warning_in_collapsed_rail() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        app.engine.projects[0].path_missing = true;
        app.left_collapsed = true;

        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let rendered: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(
            rendered.contains('⚠'),
            "the collapsed icon rail must surface a warning glyph for a path-missing project",
        );
    }

    #[test]
    fn osc8_forced_width_keeps_following_cells_in_diff() {
        use ratatui::buffer::{Buffer, CellDiffOption};
        use ratatui::layout::Rect;

        let area = Rect::new(0, 0, 3, 1);
        let prev = Buffer::empty(area);
        let mut next = Buffer::empty(area);
        // Cell 0: an OSC-8-wrapped "X" whose forced width is the real glyph width
        // (1). Cells 1 and 2: plain "Y"/"Z".
        let wrapped = osc8_wrap_symbol("X", "https://example.com");
        next[(0, 0)]
            .set_symbol(&wrapped)
            .set_diff_option(CellDiffOption::ForcedWidth(
                std::num::NonZeroU16::new(1).unwrap(),
            ));
        next[(1, 0)].set_symbol("Y");
        next[(2, 0)].set_symbol("Z");

        let diff = prev.diff(&next);
        let xs: Vec<u16> = diff.iter().map(|(x, _, _)| *x).collect();
        // Without ForcedWidth the escape bytes make ratatui overcount the linked
        // cell's width and skip the following cells; with it, Y and Z remain.
        assert!(
            xs.contains(&1) && xs.contains(&2),
            "cells after an OSC 8 link must stay in the diff: {xs:?}"
        );
    }

    #[test]
    fn osc8_wrap_symbol_wraps_with_stable_id() {
        let uri = "https://example.com";
        let wrapped = osc8_wrap_symbol("X", uri);
        let id = osc8_link_id(uri);
        assert_eq!(
            wrapped,
            format!("\x1b]8;id={id};{uri}\x1b\\X\x1b]8;;\x1b\\")
        );
        // The id is stable per URI so adjacent cells merge into one link, and it
        // differs for a different URI.
        assert_eq!(osc8_link_id(uri), osc8_link_id(uri));
        assert_ne!(osc8_link_id(uri), osc8_link_id("https://other.example"));
    }

    /// F6 regression: closing the session-slot tab (`is_main`) while other
    /// tabs are live must show the non-destructive copy, not the "can't
    /// reopen this exact conversation" destructive copy meant for extra tabs.
    #[test]
    fn confirm_close_tab_tail_is_non_destructive_for_main_with_siblings() {
        let tail = confirm_close_tab_tail(false, true);
        assert!(
            tail.contains("keep running"),
            "expected non-destructive copy for is_main with siblings, got: {tail}"
        );
        assert!(!tail.contains("can't reopen"));
    }

    /// Closing an extra (non-main) tab while siblings are live is destructive
    /// (the row is permanently deleted) and must keep the original warning.
    #[test]
    fn confirm_close_tab_tail_is_destructive_for_extra_tab_with_siblings() {
        let tail = confirm_close_tab_tail(false, false);
        assert!(tail.contains("can't reopen"));
    }

    /// Closing the agent's only tab (main or extra) detaches the whole agent,
    /// which is the pre-existing non-destructive-but-detaching copy.
    #[test]
    fn confirm_close_tab_tail_only_tab_detaches_regardless_of_main() {
        assert!(confirm_close_tab_tail(true, true).contains("only tab"));
        assert!(confirm_close_tab_tail(true, false).contains("only tab"));
    }

    /// Render-level regression for the drift crumb gate: when the agent's
    /// branch equals the project's current branch but has DRIFTED from the
    /// branch it was created on, the header must still surface the "agent:"
    /// crumb with "(orig: <initial>)". Exercises the real `render_header`
    /// buffer, not just the pure `top_bar_branch_suffix` helper.
    #[test]
    fn header_shows_drift_crumb_when_agent_on_project_branch_but_drifted() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        // Force: current branch == project current branch, but != initial.
        let project_branch = app.engine.projects[0].current_branch.clone();
        {
            let session = &mut app.engine.sessions[0];
            session.branch_name = project_branch.clone();
            session.initial_branch = "server-mode".to_string();
        }

        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let rendered: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();
        assert!(
            rendered.contains("orig:"),
            "header should show the drift crumb even when the agent sits on the project branch; \
             rendered header did not contain 'orig:'"
        );
    }

    /// `always_show_tab_strip = false` (the default) must keep hiding the strip
    /// when the selected session has only its session-slot tab: the returned
    /// area is the full input area and no clickable tab regions are recorded.
    #[test]
    fn tab_strip_hidden_at_single_tab_by_default() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        assert!(!app.engine.config.ui.always_show_tab_strip);

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let area = Rect::new(0, 0, 80, 24);
        terminal
            .draw(|frame| {
                let term_area = app.render_agent_tab_strip_if_needed(frame, area, true);
                assert_eq!(
                    term_area, area,
                    "single tab with the preference off must not reserve a strip row"
                );
            })
            .expect("render frame");
        assert!(
            app.agent_tab_regions.is_empty(),
            "no clickable tab regions should be recorded when the strip is hidden"
        );
    }

    /// `always_show_tab_strip = true` must show the strip even with a single
    /// (session-slot-only) tab: the returned area is shrunk by the strip row
    /// and the single tab is recorded as a clickable region.
    #[test]
    fn tab_strip_shown_at_single_tab_when_always_show_enabled() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        app.engine.config.ui.always_show_tab_strip = true;

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let area = Rect::new(0, 0, 80, 24);
        terminal
            .draw(|frame| {
                let term_area = app.render_agent_tab_strip_if_needed(frame, area, true);
                assert_eq!(
                    term_area,
                    Rect::new(area.x, area.y + 1, area.width, area.height - 1),
                    "a single tab with the preference on must still reserve the strip row"
                );
            })
            .expect("render frame");
        assert_eq!(
            app.agent_tab_regions.len(),
            1,
            "the sole session-slot tab should be recorded as a clickable region"
        );
    }

    /// Seeds an extra provider tab for `session_id` so tests can exercise the
    /// two-or-more-tab strip layout. Mirrors the private helpers in
    /// `sessions.rs`/`input.rs` (there is no shared test util for this).
    fn seed_render_tab(app: &mut App, session_id: &str, tab_id: &str, provider: &str, order: i64) {
        app.engine.agent_tabs.insert(
            tab_id.to_string(),
            dux_core::model::AgentTab {
                id: tab_id.to_string(),
                session_id: session_id.to_string(),
                provider: dux_core::model::ProviderKind::new(provider),
                sort_order: order,
                created_at: chrono::Utc::now(),
            },
        );
    }

    /// Reads the strip row (y = `area.y`) of the test backend buffer back as
    /// `(symbol, fg, bg)` triples, so tests can assert on rendered glyphs and
    /// styles without reaching into private layout math.
    fn strip_row_cells(
        terminal: &ratatui::Terminal<ratatui::backend::TestBackend>,
        area: Rect,
    ) -> Vec<(String, Color, Color)> {
        let buf = terminal.backend().buffer();
        (area.x..area.x + area.width)
            .map(|x| {
                let cell = &buf[(x, area.y)];
                (cell.symbol().to_string(), cell.fg, cell.bg)
            })
            .collect()
    }

    /// The focused tab must carry the active-dot glyph and the shared
    /// selection style (selection_fg on selection_bg) so it is legible even
    /// in the default theme, where `title_focused` and `selection_bg` are the
    /// same color and would otherwise make focused-tab text disappear against
    /// its own highlight. Unfocused tabs must stay legible too, using
    /// `title_normal` rather than the near-invisible `hint_dim_desc_fg`.
    #[test]
    fn tab_strip_marks_focused_tab_with_dot_and_selection_style() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let session_id = app.engine.sessions[0].id.clone();
        // The session-slot tab defaults to "codex" (see test_support); give
        // the extra tab a distinct provider so labels don't get a
        // disambiguating " 2" suffix and stay simple to assert on.
        seed_render_tab(&mut app, &session_id, "tab-2", "claude", 1);
        app.set_focused_tab(&session_id, "tab-2");

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let area = Rect::new(0, 0, 80, 24);
        let strip_area = Rect::new(area.x, area.y, area.width, 1);
        terminal
            .draw(|frame| {
                app.render_agent_tab_strip_if_needed(frame, area, true);
            })
            .expect("render frame");

        let rendered: String = strip_row_cells(&terminal, strip_area)
            .into_iter()
            .map(|(sym, _, _)| sym)
            .collect();
        assert!(
            rendered.contains('●'),
            "the focused tab must carry the active-dot glyph, got: {rendered}"
        );
        assert!(
            rendered.contains('│'),
            "tabs must be separated by border glyphs, got: {rendered}"
        );

        let cells = strip_row_cells(&terminal, strip_area);
        let selection_style = app.theme.selection_style();
        let dot_cell = cells
            .iter()
            .find(|(sym, _, _)| sym == "●")
            .expect("dot glyph must be rendered");
        assert_eq!(
            (dot_cell.1, dot_cell.2),
            (
                selection_style.fg.expect("selection fg"),
                selection_style.bg.expect("selection bg")
            ),
            "the active-dot glyph must use the shared selection style, not a color that \
             matches its own background"
        );
        assert_ne!(
            dot_cell.1, dot_cell.2,
            "focused tab foreground and background must differ so the label is legible"
        );

        // "o" only appears in the unfocused session-slot tab's "codex" label,
        // not in the focused "claude" tab, so it unambiguously identifies an
        // unfocused-tab cell.
        let unfocused_fg = app.theme.title_normal;
        assert!(
            cells
                .iter()
                .any(|(sym, fg, _)| sym == "o" && *fg == unfocused_fg),
            "unfocused tab labels must use the legible title_normal color"
        );
    }

    /// Width/truncation math: the strip must never draw past the pane width,
    /// even once separators and the active dot widen each tab beyond a bare
    /// label, and the trailing add button (with its own closing separator)
    /// must still fit.
    #[test]
    fn tab_strip_width_math_stays_within_pane_with_many_tabs() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let session_id = app.engine.sessions[0].id.clone();
        for i in 0..6 {
            seed_render_tab(
                &mut app,
                &session_id,
                &format!("tab-{i}"),
                "codex",
                i as i64,
            );
        }
        // Focus the last tab so the scroll-into-view logic is exercised too.
        app.set_focused_tab(&session_id, "tab-5");

        let width = 40u16;
        let backend = TestBackend::new(width, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let area = Rect::new(0, 0, width, 24);
        terminal
            .draw(|frame| {
                app.render_agent_tab_strip_if_needed(frame, area, true);
            })
            .expect("render frame");

        for (tab_id, rect) in &app.agent_tab_regions {
            assert!(
                rect.x + rect.width <= area.x + area.width,
                "tab region for {tab_id} must stay within the pane width: {rect:?}"
            );
        }
        if let Some(add_region) = app.agent_tab_add_region {
            assert!(
                add_region.x + add_region.width <= area.x + area.width,
                "the add-tab region must stay within the pane width: {add_region:?}"
            );
        }
    }

    /// F1 regression: a custom provider label made of double-width CJK glyphs
    /// must be measured by real display columns (unicode-width), not
    /// `chars().count()`. A char-count-based width undercounts "克劳德" (3
    /// chars, 6 display columns) by half, so the label overflows its
    /// recorded region and paints over the add button.
    #[test]
    fn tab_strip_cjk_label_region_matches_rendered_width_and_no_add_button_overlap() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let session_id = app.engine.sessions[0].id.clone();
        seed_render_tab(&mut app, &session_id, "tab-2", "克劳德", 1);
        app.set_focused_tab(&session_id, "tab-2");

        let width = 40u16;
        let backend = TestBackend::new(width, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let area = Rect::new(0, 0, width, 24);
        terminal
            .draw(|frame| {
                app.render_agent_tab_strip_if_needed(frame, area, true);
            })
            .expect("render frame");

        let tab_region = app
            .agent_tab_regions
            .iter()
            .find(|(id, _)| id == "tab-2")
            .map(|(_, rect)| *rect)
            .expect("tab-2 region recorded");
        let add_region = app
            .agent_tab_add_region
            .expect("add button region recorded");

        assert!(
            tab_region.x + tab_region.width <= add_region.x,
            "the CJK tab's recorded region must not overlap the add button: \
             tab={tab_region:?} add={add_region:?}"
        );

        // The region must be wide enough to actually contain the rendered
        // label: its real display width (unicode-width, not char count) plus
        // the leading separator column and the dot gutter/padding.
        let expected_min_width = "克劳德".cell_width() + 1;
        assert!(
            tab_region.width >= expected_min_width,
            "tab region width {} must cover the CJK label's real display width {}",
            tab_region.width,
            expected_min_width
        );
    }

    /// F2 regression: in a narrow pane with long labels, the focused tab must
    /// always be at least partially visible (dot or truncated label
    /// rendered), regardless of which tab index is focused. Previously an
    /// over-wide focused segment starved the scroll-into-view loop, walking
    /// `start` straight past `focused_idx` and leaving the focused tab
    /// entirely off-screen.
    #[test]
    fn tab_strip_keeps_focused_tab_visible_for_every_focus_index_in_narrow_pane() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        for focus_idx in 0..6 {
            let mut app = test_app(default_bindings());
            let session_id = app.engine.sessions[0].id.clone();
            for i in 0..6 {
                seed_render_tab(
                    &mut app,
                    &session_id,
                    &format!("tab-{i}"),
                    "a-very-long-custom-provider-name",
                    i as i64,
                );
            }
            let focused_tab_id = format!("tab-{focus_idx}");
            app.set_focused_tab(&session_id, &focused_tab_id);

            let width = 30u16;
            let backend = TestBackend::new(width, 24);
            let mut terminal = Terminal::new(backend).expect("terminal");
            let area = Rect::new(0, 0, width, 24);
            terminal
                .draw(|frame| {
                    app.render_agent_tab_strip_if_needed(frame, area, true);
                })
                .expect("render frame");

            assert!(
                app.agent_tab_regions
                    .iter()
                    .any(|(id, _)| *id == focused_tab_id),
                "focused tab {focused_tab_id} must be visible/rendered in a narrow pane, \
                 focus_idx={focus_idx}"
            );
        }
    }

    /// F3 regression: the active-tab dot must come from the one shared glyph
    /// source in `theme.rs`, not a re-literaled `"●"` in render.rs.
    #[test]
    fn tab_active_dot_reuses_shared_theme_glyph() {
        assert_eq!(crate::theme::ATTENTION_GLYPH, crate::theme::DOT_GLYPH);
        assert_eq!(crate::theme::DOT_GLYPH, "●");
    }

    /// F4 regression: a tab's rendered width must not depend on whether it is
    /// focused. Previously only the focused tab carried the dot glyph, making
    /// it 2 columns wider than an unfocused tab and causing the strip to
    /// reflow/jitter as focus moved between tabs.
    #[test]
    fn tab_strip_segment_width_is_focus_independent() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let session_id = app.engine.sessions[0].id.clone();
        seed_render_tab(&mut app, &session_id, "tab-2", "claude", 1);

        let width = 60u16;
        let area = Rect::new(0, 0, width, 24);

        app.set_focused_tab(&session_id, "tab-2");
        let backend = TestBackend::new(width, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                app.render_agent_tab_strip_if_needed(frame, area, true);
            })
            .expect("render frame");
        let focused_width = app
            .agent_tab_regions
            .iter()
            .find(|(id, _)| id == "tab-2")
            .map(|(_, rect)| rect.width)
            .expect("tab-2 region recorded while focused");

        let session_id_clone = session_id.clone();
        app.set_focused_tab(&session_id, &session_id_clone);
        let backend2 = TestBackend::new(width, 24);
        let mut terminal2 = Terminal::new(backend2).expect("terminal");
        terminal2
            .draw(|frame| {
                app.render_agent_tab_strip_if_needed(frame, area, true);
            })
            .expect("render frame");
        let unfocused_width = app
            .agent_tab_regions
            .iter()
            .find(|(id, _)| id == "tab-2")
            .map(|(_, rect)| rect.width)
            .expect("tab-2 region recorded while unfocused");

        assert_eq!(
            focused_width, unfocused_width,
            "a tab's width must not change when focus moves onto/off of it, or the strip \
             reflows/jitters as the user tabs between agents"
        );
    }

    /// Regression test for issue #258: while the interactive agent terminal is
    /// rendered, the real (hardware) terminal cursor must be moved onto the
    /// embedded PTY cursor cell. IME composition popups (e.g. a Korean IME) are
    /// drawn by the terminal/OS at the hardware cursor position, so if it is
    /// left at the origin the composing character appears detached from the
    /// prompt near the top-left instead of at the agent's cursor.
    #[test]
    fn interactive_agent_aligns_hardware_cursor_with_pty_cursor() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let session_id = app.engine.sessions[0].id.clone();

        // `ESC[5;9H` moves the cursor to row 5, col 9 (1-based) = row 4, col 8
        // (0-based); printing 'X' there advances it to row 4, col 9 (0-based).
        // `sleep` keeps the child alive so the snapshot keeps a live cursor.
        let args = vec![
            "-c".to_string(),
            "printf '\\033[5;9HX'; sleep 30".to_string(),
        ];
        let client = PtyClient::spawn("/bin/sh", &args, std::path::Path::new("."), 24, 80, 100)
            .expect("spawn pty");
        app.engine.providers.insert(session_id, client);

        // Enter interactive fullscreen agent mode.
        app.input_target = InputTarget::Agent;
        app.session_surface = SessionSurface::Agent;
        app.fullscreen_overlay = FullscreenOverlay::Agent;

        // Wait until the child's cursor escape has been parsed (no fixed sleep).
        wait_for_agent_cursor(&mut app, 4, 9);

        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");

        let term_area = app
            .mouse_layout
            .agent_term
            .expect("agent terminal area should be recorded after render");
        // Confirm the renderer's input cursor really is the parked PTY cursor,
        // then derive the expected screen cell from the KNOWN (row 4, col 9)
        // literals — not from snapshot_buf — so a transposed or mis-offset
        // production computation cannot make this assertion tautologically true.
        let cursor = app
            .snapshot_buf
            .cursor
            .expect("interactive agent snapshot should expose a PTY cursor");
        assert_eq!(
            (cursor.row, cursor.col),
            (4, 9),
            "PTY should have parked its cursor at the expected cell"
        );
        let expected = (term_area.x + 9, term_area.y + 4);
        terminal.backend_mut().assert_cursor_position(expected);
    }

    /// The same hardware-cursor alignment must hold in the normal (non-
    /// fullscreen) center-pane agent view, not just the fullscreen overlay.
    #[test]
    fn inline_agent_aligns_hardware_cursor_with_pty_cursor() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let session_id = app.engine.sessions[0].id.clone();
        let args = vec![
            "-c".to_string(),
            "printf '\\033[5;9HX'; sleep 30".to_string(),
        ];
        let client = PtyClient::spawn("/bin/sh", &args, std::path::Path::new("."), 24, 80, 100)
            .expect("spawn pty");
        app.engine.providers.insert(session_id, client);

        // Inline agent view: no fullscreen overlay, center pane focused.
        app.input_target = InputTarget::Agent;
        app.session_surface = SessionSurface::Agent;
        app.fullscreen_overlay = FullscreenOverlay::None;
        app.center_mode = CenterMode::Agent;
        app.focus = FocusPane::Center;

        wait_for_agent_cursor(&mut app, 4, 9);

        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");

        let term_area = app
            .mouse_layout
            .agent_term
            .expect("agent terminal area should be recorded after render");
        // Guard against a tautological pass: the terminal must be offset from
        // the origin so the +9/+4 offset below is actually exercised (mirrors
        // the non-interactive test's setup assertion).
        assert!(
            term_area.x > 0 || term_area.y > 0,
            "test setup: agent terminal should be offset from the origin"
        );
        let cursor = app
            .snapshot_buf
            .cursor
            .expect("inline agent snapshot should expose a PTY cursor");
        assert_eq!((cursor.row, cursor.col), (4, 9));
        let expected = (term_area.x + 9, term_area.y + 4);
        terminal.backend_mut().assert_cursor_position(expected);
    }

    /// The hardware cursor must only track the PTY in interactive (input) mode.
    /// In a non-interactive agent view there is no IME input, so the cursor
    /// must not be repositioned — otherwise it leaves a stray blinking cursor
    /// over read-only output.
    #[test]
    fn non_interactive_agent_leaves_hardware_cursor_at_origin() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let session_id = app.engine.sessions[0].id.clone();
        let args = vec![
            "-c".to_string(),
            "printf '\\033[5;9HX'; sleep 30".to_string(),
        ];
        let client = PtyClient::spawn("/bin/sh", &args, std::path::Path::new("."), 24, 80, 100)
            .expect("spawn pty");
        app.engine.providers.insert(session_id, client);

        // session_surface is Agent so the snapshot is populated, but input is
        // NOT routed to the agent.
        app.session_surface = SessionSurface::Agent;
        app.fullscreen_overlay = FullscreenOverlay::Agent;
        wait_for_agent_cursor(&mut app, 4, 9);
        app.input_target = InputTarget::None;

        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");

        let term_area = app
            .mouse_layout
            .agent_term
            .expect("agent terminal area should be recorded after render");
        // The PTY genuinely has an off-origin cursor that the renderer COULD
        // have placed: term_area is offset and the PTY cursor is at (4, 9), so
        // a regression that wrongly set the hardware cursor would move it away
        // from the origin and fail the assertion below.
        assert!(
            term_area.x > 0 || term_area.y > 0,
            "test setup: agent terminal should be offset from the origin"
        );
        assert!(
            app.snapshot_buf.cursor.is_some(),
            "test setup: the PTY should still expose a cursor to (not) place"
        );

        // Not in input mode → the hardware cursor is never positioned and stays
        // at the backend origin.
        terminal.backend_mut().assert_cursor_position((0u16, 0u16));
    }

    /// The same hardware-cursor alignment must also hold for a companion
    /// terminal (the `SessionSurface::Terminal` path), which renders through the
    /// same `render_agent_terminal` code as the agent surface. Without coverage
    /// here, a future change that special-cased the agent surface could
    /// silently break IME composition placement for companion terminals.
    #[test]
    fn companion_terminal_aligns_hardware_cursor_with_pty_cursor() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let session_id = app.engine.sessions[0].id.clone();
        let args = vec![
            "-c".to_string(),
            "printf '\\033[5;9HX'; sleep 30".to_string(),
        ];
        let client = PtyClient::spawn("/bin/sh", &args, std::path::Path::new("."), 24, 80, 100)
            .expect("spawn pty");

        let terminal_id = "term-1".to_string();
        app.engine.companion_terminals.insert(
            terminal_id.clone(),
            CompanionTerminal {
                owner: dux_core::model::TerminalOwner::Session(session_id),
                label: "shell".to_string(),
                foreground_cmd: None,
                client,
            },
        );
        app.active_terminal_id = Some(terminal_id);

        // Interactive fullscreen companion-terminal mode.
        app.input_target = InputTarget::Terminal;
        app.session_surface = SessionSurface::Terminal;
        app.fullscreen_overlay = FullscreenOverlay::Terminal;

        wait_for_agent_cursor(&mut app, 4, 9);

        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");

        let term_area = app
            .mouse_layout
            .agent_term
            .expect("terminal area should be recorded after render");
        assert!(
            term_area.x > 0 || term_area.y > 0,
            "test setup: companion terminal should be offset from the origin"
        );
        let cursor = app
            .snapshot_buf
            .cursor
            .expect("companion terminal snapshot should expose a PTY cursor");
        assert_eq!((cursor.row, cursor.col), (4, 9));
        let expected = (term_area.x + 9, term_area.y + 4);
        terminal.backend_mut().assert_cursor_position(expected);
    }

    /// Regression test for the invisible-caret-in-Alacritty bug.
    ///
    /// The renderer used to pre-paint the cursor cell into a block
    /// (`fg(input_cursor_fg).bg(prompt_cursor)`) *and* move the real hardware
    /// cursor onto the same cell. Alacritty draws its block cursor by INVERTING
    /// the cell's colors, so inverting a cell that was already styled to look
    /// like a cursor cancelled it back to invisibility — the caret vanished
    /// only under Alacritty. The fix stops pre-painting the cell and relies on
    /// the hardware cursor (set via `set_cursor_position`) for the visible
    /// block, leaving Alacritty a normal cell to invert.
    ///
    /// This asserts both halves of the fix:
    ///   1. the cursor cell is no longer pre-painted into the invisible block
    ///      style (so Alacritty's inversion produces a visible cursor), and
    ///   2. the hardware cursor is still placed exactly on the cursor cell, so
    ///      IME composition alignment (issue #258) is preserved.
    #[test]
    fn interactive_agent_does_not_prepaint_cursor_cell_block() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let session_id = app.engine.sessions[0].id.clone();
        // Park the PTY cursor at row 4, col 9 (0-based), same as the #258 tests.
        let args = vec![
            "-c".to_string(),
            "printf '\\033[5;9HX'; sleep 30".to_string(),
        ];
        let client = PtyClient::spawn("/bin/sh", &args, std::path::Path::new("."), 24, 80, 100)
            .expect("spawn pty");
        app.engine.providers.insert(session_id, client);

        app.input_target = InputTarget::Agent;
        app.session_surface = SessionSurface::Agent;
        app.fullscreen_overlay = FullscreenOverlay::Agent;

        wait_for_agent_cursor(&mut app, 4, 9);

        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");

        let term_area = app
            .mouse_layout
            .agent_term
            .expect("agent terminal area should be recorded after render");
        assert!(
            term_area.x > 0 || term_area.y > 0,
            "test setup: agent terminal should be offset from the origin"
        );
        let cursor = app
            .snapshot_buf
            .cursor
            .expect("interactive agent snapshot should expose a PTY cursor");
        assert_eq!((cursor.row, cursor.col), (4, 9));

        let cx = term_area.x + 9;
        let cy = term_area.y + 4;

        // (1) The cursor cell must NOT carry the pre-painted block style that
        // Alacritty's inversion cancels to invisibility. In the test theme the
        // old style was `fg(Color::Black).bg(Color::Cyan)` (input_cursor_fg /
        // prompt_cursor); the cell must not be both at once.
        assert_eq!(
            app.theme.input_cursor_fg,
            ratatui::style::Color::Black,
            "test theme assumption: input_cursor_fg is Black"
        );
        assert_eq!(
            app.theme.prompt_cursor,
            ratatui::style::Color::Cyan,
            "test theme assumption: prompt_cursor is Cyan"
        );
        let cell = terminal
            .backend()
            .buffer()
            .cell((cx, cy))
            .expect("cursor cell should be within the rendered buffer");
        assert!(
            !(cell.fg == app.theme.input_cursor_fg && cell.bg == app.theme.prompt_cursor),
            "cursor cell must not be pre-painted into the invisible block style \
             (fg={:?}, bg={:?}); the hardware cursor provides the visible block",
            cell.fg,
            cell.bg,
        );

        // (2) The hardware cursor must still land exactly on the cursor cell so
        // IME composition popups align with the prompt (issue #258).
        terminal.backend_mut().assert_cursor_position((cx, cy));
    }

    #[test]
    fn scrollback_indicator_uses_fractional_label() {
        assert_eq!(
            scrollback_indicator_label(41, 800),
            Some(" 41/800 lines ".to_string())
        );
    }

    #[test]
    fn scrollback_indicator_handles_singular_total() {
        assert_eq!(
            scrollback_indicator_label(1, 1),
            Some(" 1/1 line ".to_string())
        );
    }

    #[test]
    fn scrollback_indicator_hides_at_live_bottom() {
        assert_eq!(scrollback_indicator_label(0, 800), None);
    }

    #[test]
    fn companion_terminal_status_meta_covers_v1_states() {
        assert_eq!(
            companion_terminal_status_meta(CompanionTerminalStatus::NotLaunched),
            ("○", "not launched")
        );
        assert_eq!(
            companion_terminal_status_meta(CompanionTerminalStatus::Running),
            ("●", "running")
        );
        assert_eq!(
            companion_terminal_status_meta(CompanionTerminalStatus::Exited),
            ("◐", "exited")
        );
    }

    #[test]
    fn runtime_context_spans_highlight_quoted_values() {
        let prose = Style::default().fg(Color::DarkGray);
        let quoted = Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD);
        let spans = runtime_context_spans(
            "on agent \"foxy-basilisk\" under project \"http-server\"",
            prose,
            quoted,
        );

        assert_eq!(spans.len(), 4);
        assert_eq!(spans[0].content.as_ref(), "on agent ");
        assert_eq!(spans[1].content.as_ref(), "\"foxy-basilisk\"");
        assert_eq!(spans[2].content.as_ref(), " under project ");
        assert_eq!(spans[3].content.as_ref(), "\"http-server\"");
        assert_eq!(spans[0].style, prose);
        assert_eq!(spans[1].style, quoted);
        assert_eq!(spans[3].style, quoted);
    }

    #[test]
    fn path_completion_display_label_handles_unicode_leaf() {
        assert_eq!(path_completion_display_label("/tmp/项目/"), ".../项目/");
    }

    #[test]
    fn path_completion_display_label_keeps_root_without_leaf() {
        assert_eq!(path_completion_display_label("/"), "/");
    }

    #[test]
    fn render_single_line_cursor_input_supports_empty_prefix() {
        let line = render_single_line_cursor_input("", "macro", 2, Color::White, Color::Black);

        assert_eq!(line.spans.len(), 4);
        assert_eq!(line.spans[0].content.as_ref(), "");
        assert_eq!(line.spans[1].content.as_ref(), "ma");
        assert_eq!(line.spans[2].content.as_ref(), "c");
        assert_eq!(line.spans[3].content.as_ref(), "ro");
    }

    #[test]
    fn top_bar_branch_suffix_shows_original_only_on_drift() {
        // No drift: just the bare current branch (no "branch: " label prefix —
        // the caller owns the label).
        assert_eq!(top_bar_branch_suffix("main", "main"), "main");
        // Drift: the original branch is appended.
        assert_eq!(
            top_bar_branch_suffix("agent-tabs", "server-mode"),
            "agent-tabs (orig: server-mode)"
        );
    }

    #[test]
    fn top_bar_branch_suffix_annotates_drift_even_on_project_current_branch() {
        // F-E regression: when the agent's branch equals the project's current
        // branch but has drifted from its initial branch, the suffix helper still
        // annotates the drift. (The header gate now shows the crumb on drift
        // alone, so this value reaches the screen.)
        assert_eq!(
            top_bar_branch_suffix("main", "server-mode"),
            "main (orig: server-mode)"
        );
    }

    #[test]
    fn top_bar_branch_suffix_omits_orig_when_initial_empty() {
        // An empty initial (legacy/never-backfilled row) must render the bare
        // current branch, never a phantom "(orig: )".
        assert_eq!(top_bar_branch_suffix("main", ""), "main");
    }

    #[test]
    fn centered_rect_exact_centers_requested_size() {
        let area = Rect::new(0, 0, 100, 40);
        assert_eq!(centered_rect_exact(56, 9, area), Rect::new(22, 15, 56, 9));
    }

    #[test]
    fn centered_rect_exact_clamps_to_available_area() {
        let area = Rect::new(0, 0, 40, 6);
        assert_eq!(centered_rect_exact(56, 9, area), area);
    }

    #[test]
    fn macro_edit_text_inner_area_accounts_for_borders_and_chrome() {
        let popup = Rect::new(0, 0, 64, 20);

        assert_eq!(macro_edit_text_inner_area(popup), Rect::new(2, 3, 60, 14));
    }

    #[test]
    fn macro_text_input_layout_uses_drawable_inner_height() {
        let popup = Rect::new(0, 0, 64, 20);
        let text = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut input = TextInput::with_text(text).with_multiline(8);

        assert_eq!(input.visible_lines().len(), 8);

        sync_macro_text_input_layout(&mut input, popup);

        assert_eq!(input.visible_lines().len(), 14);
        assert_eq!(input.scroll_offset(), 6);
        assert_eq!(
            input.visible_lines().first().map(String::as_str),
            Some("line 6")
        );
        assert_eq!(
            input.visible_lines().last().map(String::as_str),
            Some("line 19")
        );
    }

    #[test]
    fn wrapped_line_count_counts_unwrapped_lines() {
        let lines = vec![Line::from(" short line"), Line::from(" another short line")];

        assert_eq!(wrapped_line_count(&lines, 40, false), 2);
    }

    #[test]
    fn wrapped_line_count_grows_for_narrow_widths() {
        let lines = vec![Line::from(vec![
            Span::raw(" Are you sure you want to delete "),
            Span::styled(
                "very-long-branch-name",
                Style::default().add_modifier(Modifier::BOLD),
            ),
            Span::raw("?"),
        ])];

        assert!(wrapped_line_count(&lines, 20, false) > 1);
    }

    // ── Unit tests for capitalize ─────────────────────────────────

    #[test]
    fn capitalize_normal_string() {
        assert_eq!(capitalize("claude"), "Claude");
    }

    #[test]
    fn capitalize_already_capitalized() {
        assert_eq!(capitalize("Claude"), "Claude");
    }

    #[test]
    fn capitalize_single_char() {
        assert_eq!(capitalize("c"), "C");
    }

    #[test]
    fn capitalize_empty_string() {
        assert_eq!(capitalize(""), "");
    }

    #[test]
    fn capitalize_all_uppercase() {
        assert_eq!(capitalize("CODEX"), "CODEX");
    }

    // ── Unit tests for pty_cell_colors ────────────────────────────

    #[test]
    fn pty_cell_colors_passes_through_in_interactive_mode() {
        let theme = Theme::default_dark();
        let fg = Color::Rgb(200, 100, 50);
        let bg = Color::Rgb(10, 20, 30);
        assert_eq!(pty_cell_colors(fg, bg, true, &theme), (fg, Some(bg)));
    }

    #[test]
    fn pty_cell_colors_preserves_default_bg_in_interactive_mode() {
        // Color::Reset must pass through so the agent/terminal pane shows
        // whatever the user's CLI configured rather than fighting it with
        // a dux-themed surface.
        let theme = Theme::default_dark();
        let fg = Color::Rgb(200, 100, 50);
        assert_eq!(
            pty_cell_colors(fg, Color::Reset, true, &theme),
            (fg, Some(Color::Reset))
        );
    }

    #[test]
    fn pty_cell_colors_grayscales_solid_bg_in_non_interactive_mode() {
        let theme = Theme::default_dark();
        let fg = Color::Rgb(200, 100, 50);
        let bg = Color::Rgb(10, 20, 30);
        let (result_fg, result_bg) = pty_cell_colors(fg, bg, false, &theme);
        assert_eq!(result_fg, theme.overlay_dim_fg);
        // Solid CLI-painted backgrounds get grayscaled, marking them as
        // read-only.
        assert_eq!(result_bg, Some(to_grayscale(bg)));
        // Sanity: the grayscale value is a uniform grey, not near-black.
        let expected_l = (0.299 * 10.0_f64 + 0.587 * 20.0 + 0.114 * 30.0).round() as u8;
        assert_eq!(
            result_bg,
            Some(Color::Rgb(expected_l, expected_l, expected_l))
        );
    }

    #[test]
    fn pty_cell_colors_lets_default_bg_show_through_in_non_interactive_mode() {
        let theme = Theme::default_dark();
        let fg = Color::Rgb(200, 100, 50);
        // Cells the CLI emitted with no explicit background return `None`
        // for the bg so the rendering loop leaves the underlying buffer
        // bg untouched — that lets the dim overlay / app surface show
        // through the minimized PTY view, only grayscaling the cells the
        // CLI actually painted.
        assert_eq!(
            pty_cell_colors(fg, Color::Reset, false, &theme),
            (theme.overlay_dim_fg, None)
        );
    }

    // ── Unit tests for to_grayscale / xterm256_to_rgb ──────────────

    #[test]
    fn to_grayscale_rgb() {
        // Pure red (255,0,0) → luminance ≈ 76
        assert_eq!(to_grayscale(Color::Rgb(255, 0, 0)), Color::Rgb(76, 76, 76));
        // Pure green (0,255,0) → luminance ≈ 150
        assert_eq!(
            to_grayscale(Color::Rgb(0, 255, 0)),
            Color::Rgb(150, 150, 150)
        );
        // Pure white → stays white
        assert_eq!(
            to_grayscale(Color::Rgb(255, 255, 255)),
            Color::Rgb(255, 255, 255)
        );
        // Pure black → stays black
        assert_eq!(to_grayscale(Color::Rgb(0, 0, 0)), Color::Rgb(0, 0, 0));
    }

    #[test]
    fn to_grayscale_indexed() {
        // Index 1 = red (128,0,0) → luminance ≈ 38
        let result = to_grayscale(Color::Indexed(1));
        assert_eq!(result, Color::Rgb(38, 38, 38));
        // Index 244 = grayscale ramp entry → 8 + 10*(244-232) = 128
        assert_eq!(to_grayscale(Color::Indexed(244)), Color::Rgb(128, 128, 128));
    }

    #[test]
    fn to_grayscale_reset_passthrough() {
        assert_eq!(to_grayscale(Color::Reset), Color::Reset);
    }

    #[test]
    fn xterm256_basic_colors() {
        assert_eq!(xterm256_to_rgb(0), (0, 0, 0));
        assert_eq!(xterm256_to_rgb(7), (192, 192, 192));
        assert_eq!(xterm256_to_rgb(15), (255, 255, 255));
    }

    #[test]
    fn xterm256_cube_colors() {
        // Index 16 = (0,0,0)
        assert_eq!(xterm256_to_rgb(16), (0, 0, 0));
        // Index 196 = (255,0,0): (196-16)=180, r=180/36=5, g=0, b=0 → (255,0,0)
        assert_eq!(xterm256_to_rgb(196), (255, 0, 0));
    }

    #[test]
    fn xterm256_grayscale_ramp() {
        assert_eq!(xterm256_to_rgb(232), (8, 8, 8));
        assert_eq!(xterm256_to_rgb(255), (238, 238, 238));
    }

    #[test]
    fn format_bytes_zero() {
        assert_eq!(format_bytes(0), "0 B");
    }

    #[test]
    fn format_bytes_below_kib() {
        assert_eq!(format_bytes(1023), "1023 B");
    }

    #[test]
    fn format_bytes_exactly_one_kib() {
        assert_eq!(format_bytes(1024), "1 KiB");
    }

    #[test]
    fn format_bytes_mib_range() {
        assert_eq!(format_bytes(1024 * 1024), "1.0 MiB");
        assert_eq!(format_bytes(1024 * 1024 * 512), "512.0 MiB");
    }

    #[test]
    fn format_bytes_gib_range() {
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1.0 GiB");
        assert_eq!(format_bytes(1024 * 1024 * 1024 * 3), "3.0 GiB");
    }

    #[test]
    fn delete_terminal_overlay_warns_only_when_an_app_is_running() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        fn overlay_text(foreground_cmd: Option<String>) -> String {
            let mut app = test_app(default_bindings());
            app.prompt = PromptState::ConfirmDeleteTerminal {
                terminal_id: "term-1".to_string(),
                terminal_label: "Terminal 1".to_string(),
                confirm_selected: false,
                foreground_cmd,
            };
            let backend = TestBackend::new(100, 40);
            let mut terminal = Terminal::new(backend).expect("terminal");
            terminal
                .draw(|frame| app.render(frame))
                .expect("render frame");
            let buffer = terminal.backend().buffer();
            buffer.content.iter().map(|cell| cell.symbol()).collect()
        }

        // An app is in the foreground: the kill warning is shown.
        let running = overlay_text(Some("vim".to_string()));
        assert!(
            running.contains("will be killed"),
            "running-app overlay should warn about the kill: {running:?}"
        );

        // Only the bare shell is running: closing it is not killing an app, so
        // the overlay still confirms the delete but shows no kill warning.
        let idle = overlay_text(None);
        assert!(
            idle.contains("want to delete"),
            "idle overlay should still confirm the delete: {idle:?}"
        );
        assert!(
            !idle.contains("will be killed"),
            "idle overlay should not warn about killing a process: {idle:?}"
        );
    }

    #[test]
    fn terminal_number_parses_trailing_counter() {
        assert_eq!(terminal_number("Terminal 1"), Some(1));
        assert_eq!(terminal_number("Terminal 12"), Some(12));
    }

    #[test]
    fn terminal_number_is_none_without_a_trailing_number() {
        assert_eq!(terminal_number("Terminal"), None);
        assert_eq!(terminal_number("scratch"), None);
    }

    #[test]
    fn terminal_dup_suffix_is_empty_for_a_unique_command() {
        assert_eq!(terminal_dup_suffix("Terminal 1", false), "");
    }

    #[test]
    fn terminal_dup_suffix_uses_the_counter_on_collision() {
        // Two terminals running the same app are disambiguated by their number.
        assert_eq!(terminal_dup_suffix("Terminal 1", true), " (#1)");
        assert_eq!(terminal_dup_suffix("Terminal 2", true), " (#2)");
    }

    #[test]
    fn terminal_dup_suffix_falls_back_to_the_label_without_a_number() {
        assert_eq!(terminal_dup_suffix("scratch", true), " (scratch)");
    }

    #[test]
    fn path_completion_display_label_shows_folder_only() {
        assert_eq!(
            path_completion_display_label("/Users/patrick/project/"),
            ".../project/"
        );
    }

    #[test]
    fn truncate_status_text_ascii_short_enough() {
        assert_eq!(truncate_status_text("hello", 10), "hello");
    }

    #[test]
    fn truncate_status_text_ascii_exact_fit() {
        assert_eq!(truncate_status_text("hello", 5), "hello");
    }

    #[test]
    fn truncate_status_text_ascii_truncated() {
        assert_eq!(truncate_status_text("hello world", 6), "hello…");
    }

    #[test]
    fn truncate_status_text_multibyte_no_panic() {
        // Box-drawing char ─ is 3 bytes but 1 char.
        let text = "Copied: ─────end";
        let result = truncate_status_text(text, 10);
        assert_eq!(result.chars().count(), 10);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn truncate_status_text_block_characters() {
        // Block characters like ██▛▘ are multi-byte; slicing by byte would panic.
        let text = "██▛▘ Opus 4.6 (1M context) · Claude Max";
        let result = truncate_status_text(text, 12);
        assert_eq!(result.chars().count(), 12);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn truncate_status_text_available_zero() {
        assert_eq!(truncate_status_text("hello", 0), "");
    }

    #[test]
    fn truncate_status_text_available_one() {
        assert_eq!(truncate_status_text("hello", 1), "…");
    }

    #[test]
    fn truncate_status_text_empty_input() {
        assert_eq!(truncate_status_text("", 10), "");
    }

    fn spans_width(spans: &[Span<'static>]) -> u16 {
        spans
            .iter()
            .map(|s| s.content.as_ref().cell_width())
            .fold(0u16, |a, b| a.saturating_add(b))
    }

    fn spans_text(spans: &[Span<'static>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn ellipsize_spans_leaves_fitting_lines_untouched() {
        let spans = vec![Span::raw("abc".to_string()), Span::raw(" def".to_string())];
        let out = ellipsize_spans(spans, 20);
        assert_eq!(spans_text(&out), "abc def");
        assert!(!spans_text(&out).contains('…'));
    }

    #[test]
    fn ellipsize_spans_truncates_and_appends_one_ellipsis() {
        let spans = vec![
            Span::raw("session-".to_string()),
            Span::raw("one-very-long-branch".to_string()),
        ];
        let out = ellipsize_spans(spans, 10);
        // Never wider than the budget, and ends in a single ellipsis cell.
        assert!(spans_width(&out) <= 10, "fits within the width");
        let text = spans_text(&out);
        assert!(text.ends_with('…'), "ends with an ellipsis: {text:?}");
        assert_eq!(text.matches('…').count(), 1, "exactly one ellipsis");
    }

    #[test]
    fn ellipsize_spans_preserves_span_styles_up_to_the_cut() {
        let styled = Style::default().fg(Color::Cyan);
        let spans = vec![
            Span::styled("keep".to_string(), styled),
            Span::raw("droppable-tail".to_string()),
        ];
        let out = ellipsize_spans(spans, 6);
        // The surviving prefix keeps its cyan style.
        assert_eq!(out[0].style, styled);
        assert!(spans_width(&out) <= 6);
        assert!(spans_text(&out).ends_with('…'));
    }

    #[test]
    fn ellipsize_spans_handles_wide_glyphs_without_overflow() {
        // CJK characters are two cells wide; the result must respect display width.
        let spans = vec![Span::raw("项目名称很长".to_string())];
        let out = ellipsize_spans(spans, 5);
        assert!(spans_width(&out) <= 5, "wide glyphs counted as two cells");
        assert!(spans_text(&out).ends_with('…'));
    }

    #[test]
    fn ellipsize_spans_zero_width_yields_nothing() {
        let spans = vec![Span::raw("anything".to_string())];
        assert!(ellipsize_spans(spans, 0).is_empty());
    }

    #[test]
    fn right_align_line_pins_the_right_group_to_the_edge() {
        let left = vec![Span::raw("● ".to_string()), Span::raw("agent".to_string())];
        let right = vec![Span::raw("PR#12".to_string())];
        let out = right_align_line(left, right, 20, 2);
        // Padded to fill the full width, name on the left, badge flush right.
        assert_eq!(spans_width(&out), 20);
        let text = spans_text(&out);
        assert!(
            text.starts_with("● agent"),
            "name stays on the left: {text:?}"
        );
        assert!(text.ends_with("PR#12"), "badge is flush right: {text:?}");
    }

    #[test]
    fn right_align_line_ellipsizes_the_left_and_keeps_the_badge() {
        let left = vec![
            Span::raw("● ".to_string()),
            Span::raw("a-really-really-long-agent-name".to_string()),
        ];
        let right = vec![Span::raw("PR#7".to_string())];
        let out = right_align_line(left, right, 20, 2);
        assert!(spans_width(&out) <= 20, "never overflows the width");
        let text = spans_text(&out);
        assert!(text.contains('…'), "the name is ellipsized: {text:?}");
        assert!(text.ends_with("PR#7"), "the badge survives: {text:?}");
    }

    #[test]
    fn right_align_line_keeps_at_least_the_gap_between_groups() {
        let left = vec![Span::raw("ab".to_string())];
        let right = vec![Span::raw("PR#9".to_string())];
        let out = right_align_line(left, right, 20, 2);
        let text = spans_text(&out);
        // Everything between "ab" and "PR#9" is padding, at least the min gap.
        let gap = &text[2..text.len() - "PR#9".len()];
        assert!(gap.chars().all(|c| c == ' '), "gap is blank: {text:?}");
        assert!(
            gap.chars().count() >= 2,
            "gap is at least the minimum: {text:?}"
        );
    }

    #[test]
    fn right_align_line_degrades_when_too_narrow_for_the_badge() {
        let left = vec![Span::raw("● name".to_string())];
        let right = vec![Span::raw("PR#123".to_string())];
        // total (6) <= badge (6) + gap (2): fall back to one ellipsized run.
        let out = right_align_line(left, right, 6, 2);
        assert!(spans_width(&out) <= 6, "never overflows even when degraded");
    }

    #[test]
    fn status_footer_lines_allows_at_most_two_status_rows() {
        assert_eq!(status_footer_lines("short", 40), 1);
        assert_eq!(status_footer_lines("this message is too wide", 10), 2);
        assert_eq!(status_footer_lines("anything", 0), 1);
    }

    // --- resource_monitor_columns (pure column-budget helper) ---

    #[test]
    fn resource_monitor_columns_wide_terminal_keeps_all_columns() {
        // A wide inner width (as at an 80+ column terminal) keeps every
        // column and gives Name a generous, well-above-floor width.
        let cols = resource_monitor_columns(76);
        assert!(cols.show_pid);
        assert!(cols.show_procs);
        assert!(
            cols.name_w >= RESOURCE_MONITOR_NAME_MIN_WIDTH,
            "name width should stay comfortably above the floor at wide terminals: {}",
            cols.name_w
        );
    }

    #[test]
    fn resource_monitor_columns_narrow_terminal_drops_pid_and_procs() {
        // At an inner width around 40 (roughly what an 85%-wide popup on a
        // ~50-column terminal yields), the old layout crushed PID/Procs/CPU/
        // RSS to unreadable slivers (verified via TestBackend, see the
        // module doc comment on `resource_monitor_columns`). The rebalanced
        // plan drops PID and Procs first, since CPU and RSS are the point
        // of the monitor and must survive at their full width.
        let cols = resource_monitor_columns(40);
        assert!(!cols.show_pid, "PID should drop before Name collapses");
        assert!(!cols.show_procs, "Procs should drop before Name collapses");
        assert!(
            cols.name_w >= RESOURCE_MONITOR_NAME_MIN_WIDTH,
            "name width must not collapse to a couple of characters: {}",
            cols.name_w
        );
    }

    #[test]
    fn resource_monitor_columns_never_drops_cpu_or_rss() {
        for width in [10u16, 20, 40, 60, 100, 200] {
            let cols = resource_monitor_columns(width);
            assert_eq!(cols.cpu_w, RESOURCE_MONITOR_CPU_W);
            assert_eq!(cols.rss_w, RESOURCE_MONITOR_RSS_W);
        }
    }

    #[test]
    fn resource_monitor_columns_intermediate_width_drops_only_pid() {
        // Wide enough to keep Procs once PID is dropped, but not wide
        // enough to keep both.
        let cols = resource_monitor_columns(45);
        assert!(!cols.show_pid);
        assert!(cols.show_procs);
        assert!(cols.name_w >= RESOURCE_MONITOR_NAME_MIN_WIDTH);
    }

    /// Render-level regression, confirmed against the pre-fix code by
    /// instrumenting a `TestBackend` render before writing this fix (see the
    /// PR description): at a 50-column terminal (inner content width 40),
    /// the old hardcoded `[Min(30), Length(8), Length(6), Length(8),
    /// Length(12)]` widths did NOT protect PID/Procs/CPU/RSS as rigid
    /// columns the way the constraint names suggest. ratatui's Table
    /// constraint solver instead let the greedy `Min(30)` column consume
    /// nearly all the width and compressed every `Length` column far below
    /// its stated size, down to unreadable slivers: the header rendered as
    /// `"PI P CP R"` (CPU shrunk to "CP", RSS to "R") and the data rows as
    /// `"5."` for a 5.0% reading and `"1"` for a 1.0 MiB reading. That is
    /// exactly backwards from this monitor's purpose: CPU and RSS are the
    /// numbers the user opened this popup to read, and the tenet is that
    /// they must survive longest. This test proves they render in full
    /// after the fix; dropping PID and Procs (below) is what keeps CPU/RSS
    /// intact.
    #[test]
    fn resource_monitor_cpu_and_rss_columns_stay_legible_at_narrow_width() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let rows = vec![ResourceStats {
            id: Some("s1".into()),
            kind: ResourceKind::Agent,
            label: "Agent (claude): some-branch-name".into(),
            pid: Some(100),
            cpu_percent: 5.0,
            rss_bytes: 1024 * 1024,
            process_count: 2,
            children: vec![ProcessInfo {
                name: "rust-analyzer".into(),
                pid: 101,
                cpu_percent: 3.0,
                rss_bytes: 512 * 1024,
                is_root: false,
            }],
        }];
        let mut expanded = HashSet::new();
        expanded.insert(100u32);

        let backend = TestBackend::new(50, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                app.render_resource_monitor(frame, &rows, 0, 0, &expanded, false);
            })
            .expect("render frame");
        let rendered: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(
            rendered.contains("CPU %"),
            "CPU % header must render in full, not truncated to a couple of characters; rendered: {rendered}"
        );
        assert!(
            rendered.contains("5.0%"),
            "the parent row's CPU reading must render in full; rendered: {rendered}"
        );
        assert!(
            rendered.contains("1.0 MiB"),
            "the parent row's RSS reading must render in full; rendered: {rendered}"
        );
        // PID and Procs are the columns that yield their space to Name/CPU/RSS
        // at this width, per `resource_monitor_columns`.
        assert!(
            !rendered.contains("PID") && !rendered.contains("Procs"),
            "PID/Procs should be dropped (not rendered truncated/empty) at this width: {rendered}"
        );
    }

    /// Companion check for the child name itself at the same width as the
    /// CPU/RSS regression above: the fix's `RESOURCE_MONITOR_NAME_MIN_WIDTH`
    /// floor keeps the child's process name fully legible (not just "beyond
    /// two characters") once PID/Procs are dropped to make room. Name was
    /// not actually the column that collapsed pre-fix at this width (the
    /// greedy `Min(30)` gave it plenty of room while crushing CPU/RSS
    /// instead, see the test above) - this test guards against a *new*
    /// regression where rebalancing the budget toward CPU/RSS would in turn
    /// starve Name.
    #[test]
    fn resource_monitor_child_name_legible_at_narrow_width() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let rows = vec![ResourceStats {
            id: Some("s1".into()),
            kind: ResourceKind::Agent,
            label: "Agent (claude): some-branch-name".into(),
            pid: Some(100),
            cpu_percent: 5.0,
            rss_bytes: 1024 * 1024,
            process_count: 2,
            children: vec![ProcessInfo {
                name: "rust-analyzer".into(),
                pid: 101,
                cpu_percent: 3.0,
                rss_bytes: 512 * 1024,
                is_root: false,
            }],
        }];
        let mut expanded = HashSet::new();
        expanded.insert(100u32);

        let backend = TestBackend::new(50, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                app.render_resource_monitor(frame, &rows, 0, 0, &expanded, false);
            })
            .expect("render frame");
        let rendered: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        // The Name column floor (16) plus the child's 6-character tree
        // prefix ("    └ ") leaves room for an 11-character prefix of the
        // 13-character name before the ellipsis kicks in - well beyond the
        // "first couple of characters" the reported bug described.
        assert!(
            rendered.contains("rust-analyz"),
            "child process name must remain legible at narrow terminal widths; rendered: {rendered}"
        );
    }

    /// Parent labels are longer than child names (`Agent (<provider>): <branch>`)
    /// so they are even more exposed to a naive column rebalance; confirm
    /// they stay legible too, not just the child rows.
    #[test]
    fn resource_monitor_parent_label_legible_at_narrow_width() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let rows = vec![ResourceStats {
            id: Some("s1".into()),
            kind: ResourceKind::Agent,
            label: "Agent (claude): some-branch-name".into(),
            pid: Some(100),
            cpu_percent: 5.0,
            rss_bytes: 1024 * 1024,
            process_count: 1,
            children: Vec::new(),
        }];
        let expanded = HashSet::new();

        let backend = TestBackend::new(50, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                app.render_resource_monitor(frame, &rows, 0, 0, &expanded, false);
            })
            .expect("render frame");
        let rendered: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(
            rendered.contains("Agent (claude)"),
            "parent label must remain legible at narrow terminal widths; rendered: {rendered}"
        );
    }

    /// No-regression check at a comfortably wide terminal: both the parent
    /// label and the child process name render in full, and every column
    /// (including PID and Procs) is present.
    #[test]
    fn resource_monitor_renders_fully_at_wide_terminal() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let rows = vec![ResourceStats {
            id: Some("s1".into()),
            kind: ResourceKind::Agent,
            label: "Agent (claude): some-branch-name".into(),
            pid: Some(100),
            cpu_percent: 5.0,
            rss_bytes: 1024 * 1024,
            process_count: 2,
            children: vec![ProcessInfo {
                name: "rust-analyzer".into(),
                pid: 101,
                cpu_percent: 3.0,
                rss_bytes: 512 * 1024,
                is_root: false,
            }],
        }];
        let mut expanded = HashSet::new();
        expanded.insert(100u32);

        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                app.render_resource_monitor(frame, &rows, 0, 0, &expanded, false);
            })
            .expect("render frame");
        let rendered: String = terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect();

        assert!(rendered.contains("Agent (claude): some-branch-name"));
        assert!(rendered.contains("rust-analyzer"));
        assert!(rendered.contains("PID"));
        assert!(rendered.contains("Procs"));
        assert!(rendered.contains("100"), "parent PID should render");
    }

    /// Render the monitor at a wide terminal and return the flattened buffer.
    fn render_monitor_text(rows: &[ResourceStats], expanded: &HashSet<u32>) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                app.render_resource_monitor(frame, rows, 0, 0, expanded, false);
            })
            .expect("render frame");
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    fn leaf_row() -> ResourceStats {
        // A leaf: the collector always puts the root in `children`, so a
        // provider that spawned no subprocesses still has exactly one entry.
        ResourceStats {
            id: Some("s1".into()),
            kind: ResourceKind::Agent,
            label: "Agent (claude): leaf-branch".into(),
            pid: Some(100),
            cpu_percent: 5.0,
            rss_bytes: 1024 * 1024,
            process_count: 1,
            children: vec![ProcessInfo {
                name: "claude".into(),
                pid: 100,
                cpu_percent: 5.0,
                rss_bytes: 1024 * 1024,
                is_root: true,
            }],
        }
    }

    fn tree_row() -> ResourceStats {
        ResourceStats {
            id: Some("s2".into()),
            kind: ResourceKind::Agent,
            label: "Agent (claude): tree-branch".into(),
            pid: Some(200),
            cpu_percent: 8.0,
            rss_bytes: 2048 * 1024,
            process_count: 2,
            children: vec![
                ProcessInfo {
                    name: "claude".into(),
                    pid: 200,
                    cpu_percent: 5.0,
                    rss_bytes: 1024 * 1024,
                    is_root: true,
                },
                ProcessInfo {
                    name: "rust-analyzer".into(),
                    pid: 201,
                    cpu_percent: 3.0,
                    rss_bytes: 1024 * 1024,
                    is_root: false,
                },
            ],
        }
    }

    /// The display defect: `children` always contains the root, so a leaf row
    /// has `children.len() == 1` and the old `!children.is_empty()` gate marked
    /// EVERY row expandable. Expanding a leaf then revealed one child that was
    /// a duplicate of the row just expanded. A leaf must render no caret.
    #[test]
    fn resource_monitor_leaf_row_renders_no_expand_indicator() {
        let rendered = render_monitor_text(&[leaf_row()], &HashSet::new());
        assert!(
            rendered.contains("Agent (claude): leaf-branch"),
            "the leaf row itself must still render: {rendered}"
        );
        assert!(
            !rendered.contains('\u{25b6}') && !rendered.contains('\u{25bc}'),
            "a leaf row (its only child is itself) must render no expand caret: {rendered}"
        );
    }

    /// The other half of the gate: a row with a real subprocess still offers
    /// the caret, so suppressing the leaf case did not suppress everything.
    #[test]
    fn resource_monitor_row_with_real_breakdown_renders_expand_indicator() {
        let rendered = render_monitor_text(&[tree_row()], &HashSet::new());
        assert!(
            rendered.contains('\u{25b6}'),
            "a row with a real subprocess must render the collapsed caret: {rendered}"
        );

        let mut expanded = HashSet::new();
        expanded.insert(200u32);
        let rendered = render_monitor_text(&[tree_row()], &expanded);
        assert!(
            rendered.contains('\u{25bc}'),
            "an expanded row must render the expanded caret: {rendered}"
        );
        assert!(
            rendered.contains("rust-analyzer"),
            "the real subprocess must render in the breakdown: {rendered}"
        );
    }

    /// The root is in its own breakdown so the rows sum to the parent total.
    /// Mark it, so the entry restating the row above reads as the parent's own
    /// usage rather than a phantom duplicate process.
    #[test]
    fn resource_monitor_marks_the_root_entry_in_the_breakdown() {
        let mut expanded = HashSet::new();
        expanded.insert(200u32);
        let rendered = render_monitor_text(&[tree_row()], &expanded);
        assert!(
            rendered.contains("claude (this process)"),
            "the breakdown entry that IS the root must be labelled: {rendered}"
        );
    }

    /// An agent (or terminal) literally titled "TOTAL" must not be
    /// misclassified as the totals row: the row's `kind` is what marks the
    /// real totals row bold, never a label string match, precisely the
    /// class of bug the `ResourceKind` refactor exists to eliminate.
    #[test]
    fn resource_monitor_only_the_total_kind_row_renders_bold() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::style::Modifier;

        let mut app = test_app(default_bindings());
        let rows = vec![
            ResourceStats {
                id: None,
                kind: ResourceKind::Total,
                label: "TOTAL".into(),
                pid: None,
                cpu_percent: 9.0,
                rss_bytes: 2048,
                process_count: 3,
                children: Vec::new(),
            },
            ResourceStats {
                id: Some("s1".into()),
                kind: ResourceKind::Agent,
                label: "TOTAL".into(),
                pid: Some(200),
                cpu_percent: 1.0,
                rss_bytes: 1024,
                process_count: 1,
                children: Vec::new(),
            },
        ];
        let expanded = HashSet::new();

        let backend = TestBackend::new(60, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                app.render_resource_monitor(frame, &rows, 0, 0, &expanded, false);
            })
            .expect("render frame");

        let buf = terminal.backend().buffer();
        let mut bold_by_row_y: Vec<(u16, bool)> = Vec::new();
        for y in 0..buf.area.height {
            let row_text: String = (0..buf.area.width)
                .map(|x| buf.cell((x, y)).expect("cell in bounds").symbol())
                .collect();
            if let Some(byte_idx) = row_text.find("TOTAL") {
                let x = row_text[..byte_idx].chars().count() as u16;
                let bold = buf
                    .cell((x, y))
                    .expect("cell in bounds")
                    .modifier
                    .contains(Modifier::BOLD);
                bold_by_row_y.push((y, bold));
            }
        }

        assert_eq!(
            bold_by_row_y.len(),
            2,
            "expected both rows literally named TOTAL to render; found: {bold_by_row_y:?}"
        );
        assert!(
            bold_by_row_y[0].1,
            "the real ResourceKind::Total row must render bold"
        );
        assert!(
            !bold_by_row_y[1].1,
            "an Agent row merely titled \"TOTAL\" must NOT render bold: \
             classification must key off `kind`, not the label string"
        );
    }
}
