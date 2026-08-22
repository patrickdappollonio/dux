use super::components::{
    Button, ButtonKind, ButtonPressedTarget, Checkbox, CheckboxState, Hint, button_state_for,
    button_width_for, modal_hint_line, render_scroll_marker, shared_button_width,
    wrap_styled_lines,
};
use super::*;
use crate::tui_color::{to_ratatui_color, to_ratatui_modifier};
use ratatui::buffer::{CellDiffOption, CellWidth};
use std::path::Path;

/// What an error dialog's shared message pane laid out: the pane itself, the
/// rows left below it inside the border ring, and the message's total wrapped
/// row count (the number the scroll keys clamp against).
struct ErrorDialogLayout {
    body: Rect,
    rest: Rect,
    total_rows: u16,
}

/// The index of the first tab the strip draws, chosen so the focused tab is
/// visible within `avail` display columns.
///
/// Pure, so the renderer can ask it twice: once at the full strip width to learn
/// whether anything ends up hidden to the left (which costs a column for the
/// leading `…`), then again at the narrowed width. Narrowing `avail` can only
/// move the answer LATER in the list, never back to 0, which is what makes that
/// two-pass reservation stable.
///
/// `seg_w` holds each tab's total width (box borders and inter-box gap
/// included). Never scrolls further than it must: the answer is 0 whenever the
/// focused tab is already reachable from the left edge.
fn tab_strip_start_index(seg_w: &[u16], avail: u16, focused_idx: usize) -> usize {
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
                start = seg_w.len().saturating_sub(1);
                break;
            }
        } else {
            break;
        }
    }
    // Safety net: guarantee the focused tab is visible even if the walk above
    // couldn't include it (e.g. it stepped past `focused_idx` because that
    // segment alone is wider than the strip).
    start.min(focused_idx)
}

/// The ordinal cell a tab pill carries in its own SEGMENT, left of the label
/// and behind a full-height divider (`│ 1 │ codex │`): one space, the tab's
/// strip POSITION (1-based), one space, so the cell's width follows the
/// number's own width (` 1 `, ` 10 `). The ordinal is the tab's switch-key
/// address (the Ctrl-1..9 defaults, and the count Ctrl-Left/Right walks
/// through), so it follows strip order (session-slot tab first, then extra
/// tabs in creation order) and RENUMBERS when a tab closes: it is a
/// position, never a stable id. Every pill is numbered, including positions
/// past 9 (which have no Ctrl-N default but are still an address for
/// Ctrl-Left/Right counting and for rebinding): a mixed strip where some
/// pills carry a numbered segment and some don't reads like two kinds of
/// tab, and the disambiguation suffix (`codex 2`) would make an un-numbered
/// tenth pill genuinely ambiguous next to a numbered second one. Position 4
/// is not special-cased either: `select_tab_4` ships unbound (legacy
/// terminals send the same byte for Ctrl-4 and the macro bar's Ctrl-\), but
/// the pill is still the address users rebind to and count against.
fn tab_pill_ordinal_cell(position: usize) -> String {
    format!(" {position} ")
}

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

/// What an agent row's second line says about where the agent lives: its
/// project, or (for a standalone agent) the folder it runs in.
///
/// A standalone agent takes the [`AgentRowOwnerTag::Folder`] arm and can never
/// take a project arm, which matters most for `Orphan`: that arm means "this
/// agent's project record is gone", and a standalone agent has not lost a
/// project, it never had one. Rendering it as a removed project would be a
/// warning about nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AgentRowOwnerTag {
    Project(ProjectTagKind, String),
    /// A standalone agent's folder, already shortened against the server's home
    /// directory for display.
    Folder {
        label: String,
    },
}

/// Decide what an agent row's second line names. Pure and unit-tested so the
/// row and any future consumer cannot drift.
pub(crate) fn agent_row_owner_tag(
    session: &AgentSession,
    project: Option<&Project>,
) -> AgentRowOwnerTag {
    match &session.workspace {
        dux_core::model::AgentWorkspace::Managed(_) => AgentRowOwnerTag::Project(
            project_tag_kind(project),
            project.map(|p| p.name.clone()).unwrap_or_default(),
        ),
        dux_core::model::AgentWorkspace::Folder(folder) => AgentRowOwnerTag::Folder {
            label: dux_core::home_path::shorten_home(std::path::Path::new(&folder.folder_path)),
        },
    }
}

/// The branch an agent row's second line shows, or `None` when it shows none.
///
/// Two reasons to show none, and they are different: the agent has no branch at
/// all (a standalone agent), or its branch is already the row's name (no title
/// is set, so repeating it would be noise). Returning `None` rather than an
/// empty string keeps the separator logic from drawing a stray divider.
pub(crate) fn agent_row_branch_segment(session: &AgentSession) -> Option<String> {
    let branch = session.branch_name()?;
    let title = session.title.as_deref()?;
    (title != branch).then(|| branch.to_string())
}

/// The colored "state word" shown on an agent row's second line, the honest,
/// field-backed stand-in for an activity string (dux has no such field). It reads
/// off the same flags that drive the working spinner and the attention pulse, so
/// the word can never disagree with the motion cue. Mirrors the web's `stateWord`
/// (`crates/dux-web/web/src/lib/flatList.ts`) exactly. Pure and unit-tested.
///
/// Priority for an Active session (kept in lockstep with the web's `stateWord`
/// so the two surfaces never disagree): `needs_attention` -> "Needs you", else
/// `typing` -> "Typing", else `working` -> "Working", else "Idle". Typing and
/// working never apply to a non-Active session, so Detached/Exited win outright.
/// When the web phase lands, mirror this exact ordering there.
pub(crate) fn agent_state_word(
    status: crate::model::SessionStatus,
    working: bool,
    typing: bool,
    needs_attention: bool,
) -> &'static str {
    // The priority ladder is the core-owned `row_state::agent_row_state`
    // (cross-language twin of the web's `stateWord`); this surface only maps the
    // typed state to the TUI's word. The busy state is "Working" for an agent.
    use dux_core::row_state::RowState;
    match dux_core::row_state::agent_row_state(status, working, typing, needs_attention) {
        RowState::NeedsAttention => "Needs you",
        RowState::Typing => "Typing",
        RowState::Busy => "Working",
        RowState::Idle => "Idle",
        RowState::Detached => "Detached",
        RowState::Exited => "Exited",
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

/// Split `label` into up to three spans around a matched CHAR range
/// (`dux_core::agent_search::match_char_range` semantics: start inclusive, end
/// exclusive, char indices): the text before the hit in `base`, the hit in
/// `matched`, the rest back in `base`. Empty side segments are omitted. All
/// slicing is char-based, never byte offsets, per the CLAUDE.md truncation
/// rule (labels carry emoji/CJK and a byte slice can panic mid-character).
fn search_highlight_spans(
    label: &str,
    base: Style,
    matched: Style,
    range: (usize, usize),
) -> Vec<Span<'static>> {
    let chars: Vec<char> = label.chars().collect();
    let end = range.1.min(chars.len());
    let start = range.0.min(end);
    let mut spans = Vec::new();
    let pre: String = chars[..start].iter().collect();
    if !pre.is_empty() {
        spans.push(Span::styled(pre, base));
    }
    let hit: String = chars[start..end].iter().collect();
    if !hit.is_empty() {
        spans.push(Span::styled(hit, matched));
    }
    let post: String = chars[end..].iter().collect();
    if !post.is_empty() {
        spans.push(Span::styled(post, base));
    }
    spans
}

/// Wrap two content lines into the standard left-pane row item: the two lines
/// plus a trailing blank spacer. Both agent rows and terminal rows funnel
/// through here so their three-line structure (and thus the framed-selection
/// geometry that `paint_framed_row_selection` relies on) can never drift apart.
fn framed_row_item(line1: Line<'static>, line2: Line<'static>) -> ListItem<'static> {
    ListItem::new(vec![line1, line2, Line::from("")])
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

/// Styling knobs for `fit_agent_meta_line`, bundled so the field spans stay
/// positional without pushing the signature past clippy's argument-count lint:
/// the `" · "` separator style, and the optional search-hit highlight (the raw
/// query plus the match style).
struct MetaLineStyle<'q> {
    sep: Style,
    highlight: Option<(&'q str, Style)>,
}

/// Fit one flexible meta-line field into `alloc` cells and, when a live search
/// query is supplied, overlay the match emphasis on the EXACT text that will
/// render. The plain fit runs first (`ellipsize_spans` on the single field
/// span), so characters and widths are byte-identical with the unhighlighted
/// path; the highlight is a pure styling split on top. Recomputing
/// `match_char_range` against the fitted text (instead of clamping a
/// full-field range) is what makes truncation safe: a match the ellipsis
/// swallowed simply is not present in the rendered text, so nothing highlights
/// and nothing can land misaligned. The trailing `…` is re-styled with the
/// field's base style explicitly, so even a match cut mid-way never bleeds its
/// emphasis onto the ellipsis. All splitting is char-based (see
/// `search_highlight_spans`), never byte offsets.
fn ellipsize_field_highlighted(
    field: Span<'static>,
    alloc: u16,
    highlight: Option<(&str, Style)>,
) -> Vec<Span<'static>> {
    let base_style = field.style;
    let fitted = ellipsize_spans(vec![field], alloc);
    let Some((query, match_style)) = highlight else {
        return fitted;
    };
    let mut out: Vec<Span<'static>> = Vec::new();
    for span in fitted {
        if span.content.as_ref() == "…" {
            out.push(Span::styled("…", base_style));
            continue;
        }
        match dux_core::agent_search::match_char_range(span.content.as_ref(), query) {
            Some(range) => out.extend(search_highlight_spans(
                span.content.as_ref(),
                base_style,
                match_style,
                range,
            )),
            None => out.push(span),
        }
    }
    out
}

/// The line-two segment naming how many browsers have an agent open, or `None`
/// when none do.
///
/// "Remote" rather than "viewers" or "watching", for two reasons: the reader is
/// sitting at this terminal, so the interesting half is that somebody ELSEWHERE has
/// the same agent open, and one of those somebodies may be the device currently
/// driving it, which "watching" would deny. Absent at zero rather than "0 remote",
/// which is the state almost every row is in almost all the time.
///
/// No singular form, deliberately: "remote" is not a countable noun here, so "1
/// remote" and "4 remote" are the same shape and a singular arm would only imply a
/// plural rule that does not exist.
fn remote_viewers_segment(count: usize) -> Option<String> {
    if count == 0 {
        return None;
    }
    Some(format!("{count} remote"))
}

/// Assemble the agent row's second line (`<marker> project · State [· branch]
/// [· trailing…]`) so it matches the web sidebar: the marker, the state word, and
/// every `trailing` segment are FIXED and stay fully visible, while the project
/// name and the branch truncate (each ending in `…`) to share whatever width is
/// left. This mirrors the web's per-field flex-shrink instead of ellipsizing the
/// whole line from the right, which would drop the tab count first. `marker`
/// already includes the leading indent (e.g. `"  ※ "`); `sep_style` styles the
/// `" · "` separators.
///
/// `trailing` is the short fixed tail (the tab count, the remote-viewer count),
/// each segment separated from the one before it. A list rather than one slot per
/// fact: two adjacent `Option<Span>` parameters of the same type are a swap the
/// compiler cannot catch.
///
/// `style.highlight` is the live search query plus the match style: the
/// project name and the branch are searched fields the line renders, so a hit
/// inside either gets the emphasis (via `ellipsize_field_highlighted`,
/// computed on the final fitted text). `None` (no filter, or the terminal row,
/// whose fields the TUI search does not filter) renders byte-identically to
/// the pre-highlight code.
fn fit_agent_meta_line(
    total_w: u16,
    marker: Span<'static>,
    name: Option<Span<'static>>,
    word: Span<'static>,
    branch: Option<Span<'static>>,
    trailing: Vec<Span<'static>>,
    style: MetaLineStyle<'_>,
) -> Vec<Span<'static>> {
    let MetaLineStyle {
        sep: sep_style,
        highlight,
    } = style;
    let width = |s: &Span<'static>| s.content.as_ref().cell_width();
    let sep = || Span::styled(" · ", sep_style);
    const SEP_W: u16 = 3; // " · "

    // Everything except the two truncatable fields is fixed: the marker, the
    // name->word separator + word, the separator before the branch (its text is
    // flexible, its separator is not), and the separator + tab count.
    let mut fixed = width(&marker)
        .saturating_add(SEP_W)
        .saturating_add(width(&word));
    if branch.is_some() {
        fixed = fixed.saturating_add(SEP_W);
    }
    for segment in &trailing {
        fixed = fixed.saturating_add(SEP_W).saturating_add(width(segment));
    }

    let budget = total_w.saturating_sub(fixed);
    let name_nat = name.as_ref().map(width).unwrap_or(0);
    let branch_nat = branch.as_ref().map(width).unwrap_or(0);

    // Share the flexible budget between the name and the branch, proportional to
    // their natural widths (flex-shrink). No truncation when they already fit.
    let (name_alloc, branch_alloc) = if name_nat + branch_nat <= budget {
        (name_nat, branch_nat)
    } else if budget == 0 {
        (0, 0)
    } else {
        let total_nat = u32::from(name_nat + branch_nat).max(1);
        let mut n = (u32::from(budget) * u32::from(name_nat) / total_nat) as u16;
        // Keep at least one cell for a present field when the other can spare it.
        if name_nat > 0 && n == 0 && budget > 1 {
            n = 1;
        }
        let mut b = budget.saturating_sub(n);
        if branch_nat > 0 && b == 0 && n > 1 {
            n = budget - 1;
            b = 1;
        }
        (n, b)
    };

    let mut out: Vec<Span<'static>> = vec![marker];
    if let Some(name) = name {
        out.extend(ellipsize_field_highlighted(name, name_alloc, highlight));
    }
    out.push(sep());
    out.push(word);
    if let Some(branch) = branch {
        out.push(sep());
        out.extend(ellipsize_field_highlighted(branch, branch_alloc, highlight));
    }
    for segment in trailing {
        out.push(sep());
        out.push(segment);
    }
    // Safety net: when even the fixed parts overflow a very narrow pane, ellipsize
    // the whole line so nothing hard-clips mid-glyph at the right edge.
    ellipsize_spans(out, total_w)
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

/// The delete-agent checkbox label, which depends on whether dux may delete the
/// branch at all.
///
/// ONE function because the label is rendered twice: once in the measurement
/// pass that sizes the dialog and once in the render pass. Two copies of the
/// string would drift and the dialog's height would stop matching its contents.
pub(super) fn delete_agent_checkbox_label(
    provenance: dux_core::model::BranchProvenance,
) -> &'static str {
    if provenance.dux_may_delete_branch() {
        "Also delete the worktree and branch"
    } else {
        // The branch is not dux's to delete; the body copy says which one
        // survives and why.
        "Also delete the worktree (branch kept)"
    }
}

/// The worktree-manager checkbox label, naming the branch it would delete.
///
/// ONE function for the same reason [`delete_agent_checkbox_label`] is one: the
/// label is rendered twice, once in the measurement pass that sizes the dialog
/// and once in the render pass, and two copies would drift the dialog's height
/// away from its contents. `None` means a detached worktree, which has no
/// branch and therefore no checkbox; the caller must not render one.
pub(super) fn delete_worktree_checkbox_label(branch: Option<&str>) -> String {
    match branch {
        Some(branch) => format!("Also delete the branch {branch}"),
        None => String::new(),
    }
}

// The worktree-removal confirmation's copy, sentence for sentence the web
// dialog's (`WorktreesDialog.tsx`).
//
// Written out as named constants rather than inline in the render, because
// that is the only thing that makes the parity checkable: the two dialogs are
// parallel implementations of one piece of copy, each pinned by a test in its
// own suite. The first version of this dialog claimed the parity in a comment
// and quietly dropped three sentences, which is exactly what a comment cannot
// catch and a test can.

/// The question, naming the worktree by its ROW LABEL (the branch when there
/// is one, the "detached <sha>" stand-in when there is not), so the sentence
/// reads for a detached worktree too.
pub(super) fn delete_worktree_title(label: &str) -> String {
    format!("Delete the worktree for {label}?")
}

pub(super) const DELETE_WORKTREE_FORCED: &str =
    "This action cannot be undone: dux has no trash and removes the directory forcibly.";

/// The follow-up sentence matters as much as the first half: "they go with it"
/// alone reads like work that is committed somewhere and merely inconvenient
/// to get back.
pub(super) const DELETE_WORKTREE_DIRTY: &str = "This worktree has uncommitted changes, and they go with it. Nothing in there that \
     is not committed exists anywhere else.";

/// Detached: say there is no CHOICE here, not merely what happens. The absent
/// checkbox is otherwise unexplained.
pub(super) const DELETE_WORKTREE_DETACHED: &str = "This worktree is not on a branch, so there is no branch to keep or delete. Only \
     the working directory is removed.";

pub(super) fn delete_worktree_branch_line(branch: &str, delete_branch: bool) -> String {
    if delete_branch {
        format!(
            "The branch \"{branch}\" will be deleted with it, forcibly. Any commits on it that \
             are not merged anywhere else go too."
        )
    } else {
        format!("The branch \"{branch}\" is kept. Only the working directory is removed.")
    }
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

/// The macro editor's popup size. Tall enough that the body still gets a
/// usable field once the name field, the selector, the misclick-safe spacer,
/// the buttons, and the hint row have taken their rows.
pub(crate) const MACRO_EDIT_POPUP: (u16, u16) = (66, 24);

/// The macro editor's vertical layout, as a single source of truth: the
/// renderer draws through it and `macro_edit_text_inner_area` measures through
/// it, so the text input's wrap width can never drift from what is painted.
///
/// Order: name label, name field, body label, body field, blank, selector,
/// misclick-safe spacer, buttons, hints.
fn macro_edit_rows(popup: Rect) -> [Rect; 9] {
    let outer_inner = Block::bordered().inner(popup);
    Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .areas(outer_inner)
}

fn macro_edit_text_inner_area(popup: Rect) -> Rect {
    Block::bordered().inner(macro_edit_rows(popup)[3])
}

fn sync_macro_text_input_layout(input: &mut TextInput, popup: Rect) {
    let text_inner = macro_edit_text_inner_area(popup);
    let wrap_w = text_inner.width.saturating_sub(1) as usize;
    input.set_display_width(if wrap_w > 0 { Some(wrap_w) } else { None });
    input.set_visible_lines(text_inner.height as usize);
    input.ensure_cursor_visible();
}

impl App {
    /// Render the Delete Agent modal's chrome and its Cancel/Delete pair for a
    /// dialog with NO checkbox: the standalone case, where there is no removal
    /// to offer.
    ///
    /// Its own path rather than a flag through the managed layout, because the
    /// managed one sizes itself around a checkbox row that does not exist here
    /// and would leave a blank band the user reads as a missing control.
    fn render_delete_agent_frame(
        &mut self,
        frame: &mut Frame,
        dialog_width: u16,
        inner_width: u16,
        body_lines: Vec<Line<'static>>,
        focus: DeleteAgentFocus,
    ) {
        let body_height = wrapped_line_count(&body_lines, inner_width, false);
        let area = centered_rect_exact(dialog_width, 2 + body_height + 1 + 3, frame.area());
        self.clear_overlay_area(frame, area);
        let outer = self.themed_overlay_block("Delete Agent");
        let inner = outer.inner(area);
        outer.render(area, frame.buffer_mut());

        let [body_area, _, buttons_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(body_height),
                Constraint::Length(1),
                Constraint::Length(3),
            ])
            .areas(inner);

        Paragraph::new(body_lines)
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
                ButtonPressedTarget::ConfirmDeleteCancel,
                self.pressed_button,
                focus == DeleteAgentFocus::Cancel,
                true,
            ))
            .render(frame, cancel_area, &self.theme);

        Button::new("Delete")
            .kind(ButtonKind::Danger)
            .state(button_state_for(
                ButtonPressedTarget::ConfirmDeleteConfirm,
                self.pressed_button,
                focus == DeleteAgentFocus::Delete,
                true,
            ))
            .render(frame, delete_area, &self.theme);

        self.overlay_layout.active = OverlayMouseLayout::ConfirmDeleteAgent {
            cancel_button: cancel_area,
            delete_button: delete_area,
            // No checkbox exists, so none can be clicked.
            checkbox: None,
        };
    }

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
        // Last, with this frame's rects recorded: the startup-log surfaces keep
        // indices into text wrapped at one width, so a width change has to retire
        // them before any key or click can act on them.
        self.reconcile_startup_log_wrap_width();
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
        // FIRST crumb, and OUTSIDE the two selection arms below. Outside because a
        // listener is a fact about the process rather than about whatever row is
        // selected, so it has to be said with nothing selected at all. First
        // because this header does not ellipsize: it is a plain paragraph, so a
        // narrow terminal simply clips the tail, and appending it would make the
        // one crumb that must never vanish silently the first one to go. Its
        // siblings are recoverable from the panes; a running network listener is
        // not visible anywhere else.
        self.push_live_header_chip(&mut spans, self.serving_chip());
        // A STANDALONE agent has no project and no branch, so it gets the one
        // crumb that IS true of it: the folder it runs in, home-collapsed. It
        // takes the slot the project crumb occupies, which is the same fact
        // ("which thing am I in") for the other kind of agent.
        //
        // Its own arm rather than a hole in the project arm, because the bar
        // used to wrap its whole body in "a project is selected" and a
        // project-less agent therefore lost the provider and terminal-count
        // crumbs too, which it does have.
        if let Some(folder) = self
            .selected_session()
            .and_then(|session| session.folder_path())
            .map(|folder| dux_core::home_path::shorten_home(std::path::Path::new(folder)))
        {
            spans.push(Span::styled(" ╱ ", Style::default().fg(sep_fg).bg(bg)));
            spans.push(Span::styled(
                "folder: ",
                Style::default().fg(label_fg).bg(bg),
            ));
            spans.push(Span::styled(
                folder,
                Style::default().fg(self.theme.branch_fg).bg(bg),
            ));
            if let Some(session) = self.selected_session() {
                spans.push(Span::styled(" ╱ ", Style::default().fg(sep_fg).bg(bg)));
                spans.push(Span::styled(
                    "provider: ",
                    Style::default().fg(label_fg).bg(bg),
                ));
                spans.push(Span::styled(
                    session.provider.as_str().to_string(),
                    Style::default().fg(self.theme.branch_fg).bg(bg),
                ));
            }
            self.push_live_header_chip(&mut spans, self.running_terminals_chip());
        } else if let Some(project) = self.selected_project() {
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
                //
                // A STANDALONE agent has no branch, so there is no crumb to
                // show. It cannot reach here at all: the folder arm above
                // handles it, and that arm names its folder instead.
                if let Some(managed) = session.workspace.as_managed()
                    && let drifted = branch_drifted(&managed.branch_name, &managed.initial_branch)
                    && let differs_from_project = managed.branch_name != project.current_branch
                    && (differs_from_project || drifted)
                {
                    // The helper appends "(orig: <initial>)" only on drift and
                    // returns the bare value (no label); we keep the themed
                    // "agent: " label and style the value ourselves.
                    let value =
                        top_bar_branch_suffix(&managed.branch_name, &managed.initial_branch);
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
            self.push_live_header_chip(&mut spans, self.running_terminals_chip());
        }
        Paragraph::new(Line::from(spans))
            .style(self.theme.header_style())
            .render(area, frame.buffer_mut());
    }

    /// Append a live-state header crumb: the separator, then `label` in the
    /// live tone. Nothing at all for `None`.
    ///
    /// One helper for every such chip because there are now two of them and the
    /// terminal-count one already existed twice, verbatim, in the two arms above.
    fn push_live_header_chip(&self, spans: &mut Vec<Span<'static>>, label: Option<String>) {
        let Some(label) = label else {
            return;
        };
        let bg = self.theme.header_bg;
        spans.push(Span::styled(
            " ╱ ",
            Style::default().fg(self.theme.header_separator_fg).bg(bg),
        ));
        spans.push(Span::styled(
            label,
            Style::default().fg(self.theme.session_active).bg(bg),
        ));
    }

    /// The live-terminal count chip's text, or `None` when none are running.
    fn running_terminals_chip(&self) -> Option<String> {
        match self.running_companion_terminal_count() {
            0 => None,
            1 => Some("● 1 terminal".to_string()),
            count => Some(format!("● {count} terminals")),
        }
    }

    /// Build the two-line flat agent row, mirroring the web sidebar row:
    /// line one is the status glyph + name + PR badge; line two is the dim
    /// `project · state word · branch (when it diverges from the name) · tabs`.
    /// The working spinner and attention pulse stay on the line-one glyph.
    fn render_agent_row(&self, session: &AgentSession, text_width: u16) -> ListItem<'static> {
        let label = session.display_label();
        // Attention wins over the working spinner (a flagged agent may still be
        // streaming its permission prompt). Both cues are rolled up across the
        // agent's tabs. The attention glyph blinks on wall-clock time.
        let needs_attention = self.engine.config.ui.attention_indicator
            && self.engine.session_needs_attention(&session.id);
        let working = matches!(session.status, crate::model::SessionStatus::Active)
            && self.engine.session_is_streaming(&session.id);
        // Typing is an Active-only cue and takes precedence over the working
        // spinner (typing suppresses streaming in the engine anyway), but sits
        // below attention. Rolled up across all of the agent's tabs.
        let typing = matches!(session.status, crate::model::SessionStatus::Active)
            && self.engine.session_is_typing(&session.id);
        let (steady_dot, steady_color) = self.theme.session_dot(&session.status);
        // The line-one glyph SHAPE encodes the state (attention blink, typing
        // caret, working spinner, else the steady dot); its COLOR does not, so
        // identity stays stable. Only attention keeps an accent so the "act now"
        // cue still pops. The live-state color lives on the state word (line two).
        let dot = if needs_attention {
            if self.attention_blink_on() {
                crate::theme::ATTENTION_GLYPH
            } else {
                " "
            }
            .to_string()
        } else if typing {
            crate::theme::TYPING_GLYPH.to_string()
        } else if working {
            crate::theme::SPINNER_FRAMES[self.spinner_frame_index()].to_string()
        } else {
            steady_dot.to_string()
        };
        // A background delete dims and italicizes the whole row.
        let deleting = self.engine.pending_deletions.contains(&session.id);
        // Identity color for the NAME: neutral for a live agent, dimmed for
        // detached/exited, never the typing/working color. The GLYPH keeps the
        // lifecycle color (steady_color: neutral active, amber detached, muted
        // exited) plus the attention accent.
        let base_color = if deleting {
            self.theme.session_deleting
        } else if matches!(session.status, crate::model::SessionStatus::Active) {
            self.theme.session_active
        } else {
            self.theme.session_exited
        };
        let glyph_color = if deleting {
            self.theme.session_deleting
        } else if needs_attention {
            self.theme.session_attention
        } else {
            steady_color
        };
        let italic = |style: Style| {
            if deleting {
                style.add_modifier(Modifier::ITALIC)
            } else {
                style
            }
        };
        let name_style = italic(Style::default().fg(base_color));
        let glyph_style = italic(Style::default().fg(glyph_color));

        // A live filter query that matched the NAME emphasizes the matched part
        // (theme `search_match_fg` + BOLD; the same range logic the web uses).
        // Only the name field is checked, so a row that matched on its project,
        // branch, or provider highlights nothing, exactly what the filter
        // matched on this visible text. While a hit is highlighted the working
        // shimmer stands down for that row: the search is transient and seeing
        // WHAT matched is its whole point.
        let search_range = self
            .agent_filter
            .as_ref()
            .and_then(|input| dux_core::agent_search::match_char_range(&label, &input.text));
        // Otherwise the name shimmers while the agent is operating (a live cue
        // that replaces the old state coloring), or renders as one plain span.
        let name_spans: Vec<Span<'static>> = if let Some(range) = search_range {
            search_highlight_spans(
                &label,
                name_style,
                italic(
                    Style::default()
                        .fg(self.theme.search_match_fg)
                        .add_modifier(Modifier::BOLD),
                ),
                range,
            )
        } else {
            match (working && !deleting, base_color) {
                (true, Color::Rgb(r, g, b)) => crate::shimmer::shimmer_spans(
                    &label,
                    (r, g, b),
                    self.start_time.elapsed().as_millis(),
                ),
                _ => vec![Span::styled(label.clone(), name_style)],
            }
        };

        // Line one: glyph + name packed left, and (if present) the PR badge
        // pinned to the right edge with the name ellipsized to fit.
        let mut line1_left = vec![Span::styled(format!("{dot} "), glyph_style)];
        line1_left.extend(name_spans);
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
        let found = session
            .project_id()
            .and_then(|project_id| self.engine.projects.iter().find(|p| p.id == project_id));
        // The project is marked with `※` (a folder stand-in) rather than a word,
        // and the marker sits directly under the agent name (two-space indent, so
        // the glyph column lines up with the name on line one). The marker span
        // includes that indent; the project NAME is a separate, truncatable span.
        //
        // A STANDALONE agent names its folder here instead, marked with the
        // standalone star in the standalone identity tone: "this agent lives
        // in your folder". Identity, not state, so the tone stays on line two.
        // The standalone terminal row wears the same star; owned terminal rows
        // keep their return arrow, where it means "owned by". Same indent,
        // same column.
        let missing_fg = self.theme.project_missing_fg;
        let standalone_fg = self.theme.standalone_location_fg;
        let (marker, name_span): (Span<'static>, Option<Span<'static>>) =
            match agent_row_owner_tag(session, found) {
                AgentRowOwnerTag::Project(ProjectTagKind::Healthy, project_name) => (
                    Span::styled("  ※ ", Style::default().fg(muted)),
                    Some(Span::styled(project_name, Style::default().fg(muted))),
                ),
                AgentRowOwnerTag::Project(ProjectTagKind::PathMissing, project_name) => (
                    Span::styled("  ⚠ ", Style::default().fg(missing_fg)),
                    Some(Span::styled(project_name, Style::default().fg(missing_fg))),
                ),
                AgentRowOwnerTag::Project(ProjectTagKind::Orphan, _) => (
                    Span::styled("  ⚠ removed project", Style::default().fg(missing_fg)),
                    None,
                ),
                AgentRowOwnerTag::Folder { label } => (
                    Span::styled(
                        format!("  {} ", crate::theme::STANDALONE_GLYPH),
                        Style::default().fg(standalone_fg),
                    ),
                    Some(Span::styled(label, Style::default().fg(standalone_fg))),
                ),
            };
        let word = agent_state_word(session.status, working, typing, needs_attention);
        let word_color = if deleting {
            self.theme.session_deleting
        } else {
            match word {
                "Needs you" => self.theme.session_attention,
                "Typing" => self.theme.session_typing,
                "Working" => self.theme.session_working,
                "Detached" => steady_color,
                _ => self.theme.provider_label_fg,
            }
        };
        // Show the branch only when it differs from the displayed name (i.e. a
        // title is set), so it is not repeated as the name on line one, and
        // never at all for an agent that has no branch.
        let branch_span = agent_row_branch_segment(session)
            .map(|branch| Span::styled(branch, Style::default().fg(muted)));
        // Resolved once and shared with the remote-viewer count below: this is the
        // render path, and `session_tab_ids` allocates.
        let tab_ids = self.session_tab_ids(&session.id);
        let tab_count = tab_ids.len();
        let tabs_span = (tab_count > 1)
            .then(|| Span::styled(format!("{tab_count} tabs"), Style::default().fg(muted)));
        // How many browsers are watching this agent's terminals. In the MUTED tone
        // rather than the standalone identity tone: that tone says "this agent
        // lives in your folder", a different fact, and this segment has to read
        // quieter than the state words beside it, which is exactly what muted is
        // for on this line. Absent at zero, and structurally zero when nothing is
        // serving, so a TUI on its own renders what it always did.
        let remote_span = remote_viewers_segment(self.remote_viewer_count(&session.id, &tab_ids))
            .map(|label| Span::styled(label, Style::default().fg(muted)));

        // Line one: right-align the PR badge (name ellipsized to the space left
        // over) when there is one, else just ellipsize the name. Line two keeps
        // the marker, state word, and tab count fully visible and truncates the
        // project name and branch to fit (matching the web's per-field shrink),
        // rather than ellipsizing the whole run and dropping the tab count first.
        let line1 = match pr_badge {
            Some(badge) => right_align_line(line1_left, vec![badge], text_width, 2),
            None => ellipsize_spans(line1_left, text_width),
        };
        // Line two renders two more searched fields (project name, branch), so
        // a live filter hit inside either gets the same emphasis as the name on
        // line one. The style matches the name highlight; the range is computed
        // per field on the exact fitted text (see ellipsize_field_highlighted).
        let line2_highlight = self.agent_filter.as_ref().and_then(|input| {
            let query = input.text.as_str();
            (!dux_core::agent_search::normalize_query(query).is_empty()).then(|| {
                (
                    query,
                    italic(
                        Style::default()
                            .fg(self.theme.search_match_fg)
                            .add_modifier(Modifier::BOLD),
                    ),
                )
            })
        });
        let line2 = fit_agent_meta_line(
            text_width,
            marker,
            name_span,
            Span::styled(word.to_string(), Style::default().fg(word_color)),
            branch_span,
            [tabs_span, remote_span].into_iter().flatten().collect(),
            MetaLineStyle {
                sep: Style::default().fg(muted),
                highlight: line2_highlight,
            },
        );

        // A trailing blank line gives each agent breathing room: unselected rows
        // read as separated, and the selection highlight (which covers the whole
        // item) gains a half-step of padding below the text instead of butting
        // right up against the next row. Shared with the terminal rows via
        // `framed_row_item` so both lists have the same three-line shape.
        framed_row_item(Line::from(line1), Line::from(line2))
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
        let items = self.left_items();
        let Some(item) = items.get(sel).copied() else {
            return;
        };
        match item {
            LeftItem::Session(_) => {
                // The three-line framed selection, shared with terminal rows.
                self.paint_framed_row_selection(buf, list_inner, map, sel, top_pad_y);
            }
            LeftItem::InactiveToggle => {
                // Only the label row (the toggle ends with a trailing spacer, so
                // the label is the second-to-last row); no frame edges.
                let Some(rel_end) = map.iter().rposition(|&i| i == sel) else {
                    return;
                };
                let rel = rel_end.saturating_sub(1);
                if map.get(rel) == Some(&sel) {
                    let tint = self.theme.selection_bar_tint();
                    let y = list_inner.y + rel as u16;
                    for x in list_inner.x..list_inner.x + list_inner.width {
                        buf[(x, y)].set_bg(tint);
                    }
                }
            }
        }
    }

    /// Paint the shared three-line framed selection over `area`: a `▄` top edge
    /// on the boundary row above the selected item (the previous item's spacer,
    /// or `top_pad_y` for the first item), the faint tint filling the two content
    /// rows edge to edge (both gutters included, background only so the row keeps
    /// its own text colors), and a `▀` bottom edge on the item's own trailing
    /// spacer. `map` is the row-to-item map for `area`, `selected` the item index.
    /// Used identically by agent rows and terminal rows so both frame the same.
    fn paint_framed_row_selection(
        &self,
        buf: &mut ratatui::buffer::Buffer,
        area: Rect,
        map: &[usize],
        selected: usize,
        top_pad_y: Option<u16>,
    ) {
        let Some(rel_start) = map.iter().position(|&i| i == selected) else {
            return;
        };
        let tint = self.theme.selection_bar_tint();
        let x0 = area.x;
        let x1 = area.x + area.width;
        let row_at = |rel: usize| area.y + rel as u16;

        // A content row: a faint tint across the full width, gutters included
        // (background only, so the row's own text colors survive on top).
        let paint_content = |buf: &mut ratatui::buffer::Buffer, rel: usize| {
            if map.get(rel) != Some(&selected) {
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

        // Top edge: the previous item's blank spacer, or the reserved top-margin
        // for the very first item.
        if rel_start > 0 {
            paint_edge(buf, row_at(rel_start - 1), "▄");
        } else if let Some(py) = top_pad_y {
            paint_edge(buf, py, "▄");
        }
        paint_content(buf, rel_start);
        paint_content(buf, rel_start + 1);
        // Bottom edge: the item's own trailing spacer.
        if map.get(rel_start + 2) == Some(&selected) {
            paint_edge(buf, row_at(rel_start + 2), "▀");
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
                        // A standalone agent has no project, so no project
                        // warning can apply to it: `agent_row_owner_tag` takes
                        // the folder arm and the glyph is skipped.
                        let found = session.project_id().and_then(|project_id| {
                            self.engine.projects.iter().find(|p| p.id == project_id)
                        });
                        let mut spans = vec![Span::styled(dot, Style::default().fg(dot_color))];
                        if matches!(
                            agent_row_owner_tag(session, found),
                            AgentRowOwnerTag::Project(
                                ProjectTagKind::PathMissing | ProjectTagKind::Orphan,
                                _
                            )
                        ) {
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

        // The VISIBLE terminals: sorted, then pruned by the live sidebar query
        // exactly as the agent list above is. Every index below (the rows, the
        // selection, the mouse map) is an index into this list.
        let terminal_items = self.terminal_items();
        let has_terminals = !terminal_items.is_empty();
        // The whole list, for the two questions that are not about what is on
        // screen: which terminals a running one has to disambiguate its title
        // against (a "(#2)" that renumbered as the user typed a query would be
        // worse than useless), and how many terminals exist at all (the pane's
        // "visible / total" count). The count is taken as a plain number rather
        // than kept borrowed, because the title is built well after this pane
        // has started writing its mouse layout back into `self`.
        let all_terminals = self.sorted_terminal_items();
        let total_terminal_count = all_terminals.len();

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
        // The middle field is the resolved DISPLAY TITLE (None when idle -> the row
        // reads a plain "Terminal"): the foreground app name, normalized and
        // collision-disambiguated ("vim (#N)") through the shared core rule
        // `terminal_title` so the sidebar, the Kill overlay, and the web all agree
        // when two same-owner terminals run the same app.
        // Each row carries an owner-derived display name (the agent's branch or
        // the project's name) so a generic engine label like "Terminal 3" is
        // never ambiguous between an agent terminal and a project terminal.
        let terminal_render_data: Vec<(String, Option<String>, String, bool)> = terminal_items
            .iter()
            .map(|(id, t)| {
                // The owner element, resolved by the one shared rule the sidebar
                // filter also matches against, so a row can never say one thing
                // and be searched by another.
                let owner_name = self.terminal_owner_label(t);
                // Whether the row wears the standalone star instead of the
                // owned-by arrow. Exhaustive, so a new owner kind must decide
                // its marker here before this compiles.
                let standalone = match t.owner.as_ref() {
                    dux_core::model::TerminalOwnerRef::Standalone => true,
                    dux_core::model::TerminalOwnerRef::Session(_)
                    | dux_core::model::TerminalOwnerRef::Project(_) => false,
                };
                // Idle (foreground normalizes to nothing) -> None -> "Terminal".
                // Running -> the collision-resolved title, disambiguated against
                // the OTHER same-owner terminals' foregrounds.
                let display_title = if dux_core::terminal_title::terminal_foreground_display(
                    t.foreground_cmd.as_deref(),
                )
                .is_some()
                {
                    let siblings: Vec<Option<&str>> = all_terminals
                        .iter()
                        .filter(|(other_id, other)| other_id != id && other.owner == t.owner)
                        .map(|(_, other)| other.foreground_cmd.as_deref())
                        .collect();
                    Some(dux_core::terminal_title::terminal_title(
                        &t.label,
                        t.foreground_cmd.as_deref(),
                        &siblings,
                    ))
                } else {
                    None
                };
                ((*id).clone(), display_title, owner_name, standalone)
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
                true,
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

        // Render terminals section if any terminals exist. This mirrors the agent
        // path above exactly: the block draws the border, and the list renders
        // into a gutter-inset body offset by a one-row top margin, with the
        // framed selection hand-painted (no widget highlight) so a terminal row
        // and an agent row are pixel-identical in framing, spacing, and selection.
        if let Some(term_area) = terminals_area {
            let terminals_focused = focused && self.left_section == LeftSection::Terminals;
            let term_count = terminal_render_data.len();
            // The same visible-over-total count the Agents title carries while a
            // query is live, for the same reason: a pruned list must not read as
            // the whole roster.
            let term_title = if self.agent_filter.is_some() {
                format!("Terminals ({term_count}/{total_terminal_count})")
            } else {
                format!("Terminals ({term_count})")
            };
            let term_block = self.themed_block(&term_title, terminals_focused);
            let term_inner = term_block.inner(term_area);
            // Two reservations for the framed selection, matching the agent path:
            //  - a one-row top margin (only with room and at least one terminal)
            //    so the first terminal's `▄` top edge has a row to paint into; and
            //  - a one-column gutter on each side, so the tinted frame has a margin
            //    and the text sits evenly padded left and right.
            // `term_content` is the click surface and the origin for the frame
            // edges; the list itself renders into `term_body`, inset by a gutter.
            let term_top_margin: u16 = if term_count > 0 && term_inner.height >= 4 {
                1
            } else {
                0
            };
            let term_top_pad_y = (term_top_margin == 1).then_some(term_inner.y);
            let term_content = Rect {
                y: term_inner.y + term_top_margin,
                height: term_inner.height - term_top_margin,
                ..term_inner
            };
            let term_body = Rect {
                x: term_content.x + LEFT_PANE_GUTTER,
                width: term_content.width.saturating_sub(LEFT_PANE_GUTTER * 2),
                ..term_content
            };
            self.mouse_layout.terminal_list = term_content;
            let term_text_width = term_body.width;
            let spinner = crate::theme::SPINNER_FRAMES[self.spinner_frame_index()];
            // A terminal row's owner element is a searched field, so a live hit
            // inside it gets the same emphasis an agent row's project and branch
            // get. Same style, same per-field range on the exact fitted text.
            let term_highlight = self.agent_filter.as_ref().and_then(|input| {
                let query = input.text.as_str();
                (!dux_core::agent_search::normalize_query(query).is_empty()).then(|| {
                    (
                        query,
                        Style::default()
                            .fg(self.theme.search_match_fg)
                            .add_modifier(Modifier::BOLD),
                    )
                })
            });
            let term_items: Vec<ListItem> = terminal_render_data
                .iter()
                .map(|(term_id, fg_cmd, owner_name, standalone)| {
                    // A terminal is either alive or gone (never detached / needs
                    // you), so the state reduces to typing -> working -> idle. It
                    // is Working when streaming output OR running a foreground app
                    // (busy while quiet), via the shared `terminal_is_working`; a
                    // terminal id (`term-N`) keys both engine predicates.
                    let typing = self.engine.is_typing(term_id);
                    let working = self.engine.terminal_is_working(term_id);
                    let (line1, line2) = terminal_row_lines(
                        &self.theme,
                        typing,
                        working,
                        spinner,
                        fg_cmd.as_deref(),
                        owner_name,
                        *standalone,
                        term_text_width,
                        self.start_time.elapsed().as_millis(),
                        term_highlight,
                    );
                    // Same three-line row shape as the agents (see `framed_row_item`).
                    framed_row_item(line1, line2)
                })
                .collect();
            // Every terminal row is exactly three lines tall; keep the height
            // vector in lockstep so the post-render mouse map lands on the right
            // row even after the list scrolls.
            let term_heights: Vec<u16> = vec![3; term_count];
            // No widget highlight: the selection is hand-painted below. The widget
            // selection is still set (when focused) purely so the list scrolls to
            // keep the selected terminal visible, exactly as the agent list does.
            let mut term_state = ListState::default().with_selected(
                if self.left_section == LeftSection::Terminals {
                    Some(self.selected_terminal_index)
                } else {
                    None
                },
            );
            term_block.render(term_area, frame.buffer_mut());
            StatefulWidget::render(
                List::new(term_items),
                term_body,
                frame.buffer_mut(),
                &mut term_state,
            );
            // Rebuild the reverse map from the post-render scroll offset, the
            // three-tall heights, and the content height (mirrors the agent path).
            self.mouse_layout.terminal_row_to_item =
                left_row_to_item(term_state.offset(), &term_heights, term_content.height);
            // Hand-paint the framed selection into the content surface, reusing the
            // same `body_will_dim` gate the agent path computed above.
            if self.left_section == LeftSection::Terminals && !body_will_dim {
                self.paint_framed_row_selection(
                    frame.buffer_mut(),
                    term_content,
                    &self.mouse_layout.terminal_row_to_item,
                    self.selected_terminal_index,
                    term_top_pad_y,
                );
            }
        } else {
            self.mouse_layout.terminal_list = Rect::default();
            self.mouse_layout.terminal_row_to_item.clear();
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

        // The offset actually DRAWN, which is the outer `scroll` clamped to the
        // wrapped extent. Both wrapping paths below produce it, and the scroll
        // marker is rendered once from it after the branch, so the two paths
        // cannot drift into disagreeing about what the marker says.
        let drawn_scroll;

        if gutter_width > 0 {
            // Gutter-aware wrapping: continuation lines are indented to align
            // with the content column past the gutter.
            let wrapped = crate::diff::wrap_diff_lines(&lines, w, gutter_width);
            self.last_diff_visual_lines = wrapped.len() as u16;

            let max_scroll = self
                .last_diff_visual_lines
                .saturating_sub(content_area.height);
            let scroll = scroll.min(max_scroll);
            drawn_scroll = scroll;

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
            drawn_scroll = scroll;

            Paragraph::new((*lines).clone())
                .wrap(Wrap { trim: false })
                .scroll((scroll, 0))
                .render(content_area, frame.buffer_mut());
        }

        // Scroll marker in the pane's right border column, on the content
        // pane's last row. Units are wrapped VISUAL lines (what the diff already
        // measures with), never the logical line count. The cell lies outside
        // `content_area`, which is also `mouse_layout.agent_term`, so text
        // selection is untouched.
        render_scroll_marker(
            frame,
            area,
            content_area,
            drawn_scroll as usize,
            content_area.height as usize,
            self.last_diff_visual_lines as usize,
            self.theme.hint_key_fg,
        );

        // Hint bar with top border (same style as agent terminal).
        if hint_area.height > 0 {
            let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
            let scroll_down = self.bindings.labels_for(Action::ScrollPageDown);
            let scroll_up = self.bindings.labels_for(Action::ScrollPageUp);
            let scroll_line = self.bindings.label_for(Action::ScrollLineDown);
            let close = self.bindings.label_for(Action::CloseOverlay);
            let mut spans: Vec<Span> = Vec::new();

            // Report the offset actually DRAWN, not the raw one held in
            // `center_mode`. The two differ whenever the content shrank under a
            // stale offset (a shorter file, a diff refresh): the view clamps to
            // what exists, and an unclamped number here would overstate where the
            // reader is until the next key press.
            if drawn_scroll > 0 {
                spans.push(Span::styled(
                    format!("Scrolled back {drawn_scroll} lines. "),
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

        let Some(session) = self.selected_session() else {
            return area;
        };
        let session_id = session.id.clone();
        let tab_ids = self.session_tab_ids(&session_id);
        let always_show = self.engine.config.ui.always_show_tab_strip;
        // The strip is a 3-row band of rounded boxes (top border, label,
        // bottom border) — the same bordered-and-rounded idiom every other
        // dux surface uses — so it needs a taller minimum than a flat row.
        if (tab_ids.len() < 2 && !always_show) || area.height < 6 || area.width < 12 {
            return area;
        }
        let focused_id = self.focused_tab_id(&session_id);

        // Gather owned per-tab data under immutable borrows, then render/mutate.
        let providers: Vec<String> = tab_ids
            .iter()
            .map(|id| self.tab_provider_label(session, id))
            .collect();
        // Each pill carries its strip ordinal in its own SEGMENT left of the
        // label (`│ 1 │ codex │`): the visible address for the tab switch
        // keys. See `tab_pill_ordinal_cell` for why every pill is numbered
        // and why ordinals renumber on close.
        let labels: Vec<String> =
            tab_labels(&providers.iter().map(|s| s.as_str()).collect::<Vec<_>>());
        let seg_ordinal: Vec<String> = (1..=labels.len()).map(tab_pill_ordinal_cell).collect();

        let [strip_area, term_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(1)])
            .areas(area);

        // No "+" add button: new tabs are created via the `new-agent-tab`
        // palette command (or the NewTab keybinding), so the boxes get the
        // full strip width — minus one column when a leading truncation mark is
        // needed (decided below, once the segment widths are known).
        let strip_width = strip_area.width;

        // Label-cell text per tab (the content right of the ordinal segment's
        // divider). All tabs are
        // generic — no per-tab marker — except the focused tab, which is
        // prefixed with the shared solid dot glyph so the active tab is
        // unambiguous even without color (matches the "●" = active/present
        // convention used by `session_dot`/`ATTENTION_GLYPH` elsewhere in the
        // theme). Every tab, focused or not, reserves the same dot-width
        // gutter: an unfocused tab renders spaces where the dot would go, so
        // a tab's rendered width never depends on whether it is focused and
        // the strip doesn't reflow/jitter as focus moves.
        let tab_active_dot: &str = crate::theme::DOT_GLYPH;
        let dot_gutter: String = " ".repeat(tab_active_dot.cell_width().max(1) as usize);
        // The label is padded symmetrically: the right margin mirrors the
        // left one (space + dot-width + space) so the text sits centered in
        // its box instead of hugging the right border.
        let mut seg_content: Vec<String> = labels
            .iter()
            .enumerate()
            .map(|(i, l)| {
                if tab_ids[i] == focused_id {
                    format!(" {tab_active_dot} {l} {dot_gutter} ")
                } else {
                    format!(" {dot_gutter} {l} {dot_gutter} ")
                }
            })
            .collect();
        // Per segment: the ordinal cell, +2 for the box's border columns, +1
        // for the full-height divider between the ordinal and label cells,
        // +1 for the gap that separates adjacent boxes. Measured in real
        // display columns (unicode-width via `CellWidth`), not
        // `chars().count()`: a char-count measure undercounts double-width
        // CJK/emoji glyphs in custom provider labels, which would overflow
        // the segment's recorded region.
        let seg_ord_w: Vec<u16> = seg_ordinal
            .iter()
            .map(|t| t.as_str().cell_width())
            .collect();
        let mut seg_w: Vec<u16> = seg_content
            .iter()
            .zip(&seg_ord_w)
            .map(|(t, ow)| t.as_str().cell_width() + ow + 4)
            .collect();

        // Choose a start index so the focused tab is visible within `avail`.
        let focused_idx = tab_ids.iter().position(|i| *i == focused_id).unwrap_or(0);

        // Tabs can be hidden to the LEFT as well as to the right: the
        // scroll-into-view choice below advances the start index to reach a
        // focused tab further along. That needs its own leading truncation mark
        // (mirroring the trailing `…`), and the mark needs a column of its own so
        // it never sits under the first box.
        //
        // Deciding it takes two passes over the same pure choice, because the
        // column it costs can itself change where the strip has to start. The
        // first pass asks the question at the full width; the second re-asks it
        // with the narrower strip. Narrowing can only push the start LATER
        // (unit-tested in `tab_strip_start_index_scrolls_only_far_enough…`), so
        // the two passes agree on whether anything is hidden and the reservation
        // cannot oscillate.
        let leading_hidden = tab_strip_start_index(&seg_w, strip_width, focused_idx) > 0;
        let avail = strip_width.saturating_sub(u16::from(leading_hidden));
        let strip_x = strip_area.x + u16::from(leading_hidden);

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
            // Truncation applies to the LABEL cell only: the ordinal segment
            // always renders whole (it is the switch-key address). Reserve
            // the ordinal cell, the box's 2 border columns, the divider and
            // the inter-box gap; fit the rest of the content (dot/gutter +
            // label + padding) into what remains.
            let budget = avail.saturating_sub(seg_ord_w[focused_idx] + 4);
            seg_content[focused_idx] = truncate_to_width(&seg_content[focused_idx], budget);
            seg_w[focused_idx] =
                seg_content[focused_idx].as_str().cell_width() + seg_ord_w[focused_idx] + 4;
        }

        let start = tab_strip_start_index(&seg_w, avail, focused_idx);

        let buf = frame.buffer_mut();
        // Base fill for the whole 3-row strip band, painted with the app
        // background so leftover buffer cells never bleed between the boxes.
        let base_style = Style::default().bg(self.theme.app_bg);
        for y in strip_area.y..strip_area.y + strip_area.height {
            for x in strip_area.x..strip_area.x + strip_area.width {
                buf[(x, y)].set_symbol(" ").set_style(base_style);
            }
        }

        // Each tab is a miniature pane: the shared rounded border set with
        // the shared focused/unfocused border and title styles, so the strip
        // follows the exact bordered-and-rounded idiom of every other dux
        // surface (`themed_block`). The focused tab additionally carries the
        // active dot inside its label, so it stays unambiguous without color.
        let corners = border::ROUNDED;
        let (top_y, mid_y, bot_y) = (strip_area.y, strip_area.y + 1, strip_area.y + 2);
        // Leading truncation mark: same one-cell `…` in the same dim color as the
        // trailing one, in the column reserved for it above. Horizontal
        // truncation, so this is not the vertical `scroll_marker` treatment.
        if start > 0 {
            buf[(strip_area.x, mid_y)]
                .set_symbol("…")
                .set_style(Style::default().fg(self.theme.hint_dim_desc_fg));
        }
        let mut x = strip_x;
        for i in start..seg_content.len() {
            if x + seg_w[i] > strip_x + avail {
                // No more room; show an overflow marker if any remain.
                if i < seg_content.len() {
                    let ell_style = Style::default().fg(self.theme.hint_dim_desc_fg);
                    if x < strip_x + avail {
                        buf[(x, mid_y)].set_symbol("…").set_style(ell_style);
                    }
                }
                break;
            }
            let active = tab_ids[i] == focused_id;
            let border_style = self.theme.border_style(active);
            let label_style = self.theme.title_style(active);
            // The pill is two cells behind one frame: the ordinal cell, a
            // full-height divider joined to the frame with light-set tees
            // (matching the rounded corners), then the label cell. Widths in
            // display columns: the ordinal cell is `ord_w`, the label cell is
            // the segment minus the ordinal, the 2 border columns, the
            // divider and the trailing inter-box gap.
            let ord_w = seg_ord_w[i];
            let label_w = seg_w[i].saturating_sub(ord_w + 4);
            let top = format!(
                "{}{}{}{}{}",
                corners.top_left,
                corners.horizontal_top.repeat(ord_w as usize),
                ratatui::symbols::line::HORIZONTAL_DOWN,
                corners.horizontal_top.repeat(label_w as usize),
                corners.top_right
            );
            let bottom = format!(
                "{}{}{}{}{}",
                corners.bottom_left,
                corners.horizontal_bottom.repeat(ord_w as usize),
                ratatui::symbols::line::HORIZONTAL_UP,
                corners.horizontal_bottom.repeat(label_w as usize),
                corners.bottom_right
            );
            buf.set_string(x, top_y, &top, border_style);
            buf[(x, mid_y)]
                .set_symbol(corners.vertical_left)
                .set_style(border_style);
            buf.set_string(x + 1, mid_y, &seg_ordinal[i], label_style);
            // The divider takes the border style, like the tees: it is part
            // of the frame, not the text.
            buf[(x + 1 + ord_w, mid_y)]
                .set_symbol(ratatui::symbols::line::VERTICAL)
                .set_style(border_style);
            buf.set_string(x + 2 + ord_w, mid_y, &seg_content[i], label_style);
            buf[(x + 2 + ord_w + label_w, mid_y)]
                .set_symbol(corners.vertical_right)
                .set_style(border_style);
            buf.set_string(x, bot_y, &bottom, border_style);
            if record_clicks {
                // The whole box (all 3 rows, borders included) is clickable;
                // the trailing gap column is not.
                self.agent_tab_regions.push((
                    tab_ids[i].clone(),
                    Rect::new(x, top_y, seg_w[i].saturating_sub(1), 3),
                ));
            }
            x += seg_w[i];
        }

        term_area
    }

    /// The hint-bar line shown while the focused surface is in scroll mode and
    /// interactive: it says that keystrokes are being dropped and names the key
    /// that returns to the live edge.
    ///
    /// The drop itself is deliberate (scroll mode routes keys to the mode, as
    /// tmux's copy mode does), what is not acceptable is doing it silently. The
    /// wording never hardcodes a key: every binding is user-configurable, so the
    /// labels come from `RuntimeBindings` and stay right after a rebind. The
    /// colors come from `Theme`: `nudge_border` is the existing semantic
    /// "something needs your attention in this pane" field, so no new theme
    /// token is needed.
    pub(crate) fn scroll_mode_cue_line(&self) -> Line<'static> {
        let warn_style = Style::default().fg(self.theme.nudge_border);
        let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
        let target = match self.session_surface {
            SessionSurface::Agent => "agent",
            SessionSurface::Terminal => "terminal",
        };
        let live_edge = self.bindings.labels_for(Action::ScrollToBottom);
        let scroll_up = self.bindings.labels_for(Action::ScrollPageUp);
        let scroll_down = self.bindings.labels_for(Action::ScrollPageDown);
        let exit_key = self.bindings.label_for(Action::ToggleFullscreen);

        let mut spans: Vec<Span> = vec![Span::styled(
            format!("Scroll mode: keys are not reaching the {target}. "),
            warn_style,
        )];
        spans.extend(self.theme.dim_key_badge_default(&live_edge));
        spans.push(Span::styled(" resume at the live edge  ", desc_style));
        spans.extend(self.theme.dim_key_badge_default(&scroll_up));
        spans.push(Span::styled(" up  ", desc_style));
        spans.extend(self.theme.dim_key_badge_default(&scroll_down));
        spans.push(Span::styled(" down  ", desc_style));
        // This line REPLACES the whole hint bar, so the exit key has to come
        // along: while the mode is on, every other key is being swallowed and
        // this is the only place left on screen that says how to leave
        // fullscreen.
        spans.extend(self.theme.dim_key_badge_default(&exit_key));
        spans.push(Span::styled(" minimize", desc_style));
        // The key badges borrow the label strings, which are locals here, so
        // hand back owned spans (same pattern as `hint_bar::modal_hint_line`).
        Line::from(
            spans
                .into_iter()
                .map(|span| Span::styled(span.content.into_owned(), span.style))
                .collect::<Vec<_>>(),
        )
    }

    /// The hint line shown while ANOTHER DEVICE is driving the terminal in the
    /// center pane, replacing the usual hints the way the scroll-mode cue does and
    /// for the same reason: while it is up, this keyboard is not reaching the
    /// child, so a line listing keys that go nowhere would be a lie.
    ///
    /// It carries its own way out (the palette, and the command to run there),
    /// because that is the only gesture that takes the terminal back, and the
    /// minimize key, because this line is the whole hint bar and in fullscreen it
    /// would otherwise be the only thing on screen with no way back.
    ///
    /// Reads the registry live, on every frame, which is what makes it disappear
    /// by itself the moment the other device lets go.
    ///
    /// `width` is the room the line actually has, which is the inner width of the
    /// center pane and NOT the window's. The line is built to fit it: the fixed
    /// half (the way out) is measured first and the DEVICE NAME is what gives way,
    /// because a cue that names a problem with no way out of it is worse than one
    /// that names the device approximately. The clause about the keys is the
    /// second thing dropped, in the narrow panes where even a cut name would not
    /// leave room for it.
    pub(crate) fn remote_driver_cue_line(&self, device: &str, width: u16) -> Line<'static> {
        let cue_style = Style::default().fg(self.theme.remote_driver_fg);
        let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
        let target = match self.session_surface {
            SessionSurface::Agent => "agent",
            SessionSurface::Terminal => "terminal",
        };
        let palette = self.bindings.label_for(Action::OpenPalette);
        let exit_key = self.bindings.label_for(Action::ToggleFullscreen);

        // The way out, built first because it is the half that must survive.
        let mut tail: Vec<Span> = Vec::new();
        if palette.is_empty() {
            // The palette has been unbound, so there is no key to name. Naming
            // the command alone is still the honest answer.
            tail.push(Span::styled(
                "Run take-over-terminal to type here",
                desc_style,
            ));
        } else {
            tail.extend(self.theme.dim_key_badge_default(&palette));
            tail.push(Span::styled(" take-over-terminal", desc_style));
        }
        if !exit_key.is_empty() && self.fullscreen_overlay != FullscreenOverlay::None {
            tail.push(Span::styled("  ", desc_style));
            tail.extend(self.theme.dim_key_badge_default(&exit_key));
            tail.push(Span::styled(" minimize", desc_style));
        }
        let tail_width: usize = tail.iter().map(|span| span.content.chars().count()).sum();

        // Two spellings of the sentence, the fuller one preferred. Both name who
        // is driving; only the fuller one also says what that means for the keys,
        // which is the part a user can work out from the pane not responding.
        // The spellings, longest first, and the first one that FITS in what the
        // tail left over is the one used. What they give up, in order, is the
        // clause about the keys (which a user can infer from a pane that does not
        // respond) and then the device's name.
        let room = (width as usize).saturating_sub(tail_width);
        // Fewer characters than this is not a name any more, so the next spelling
        // is a better answer than cutting further into the device.
        const MIN_DEVICE_CHARS: usize = 3;
        let named = [
            format!(" is driving this {target}; your keys go nowhere. "),
            format!(" is driving this {target}. "),
        ];
        let mut prefix = String::new();
        for prose in named {
            if let Some(budget) = room.checked_sub(prose.chars().count())
                && budget >= MIN_DEVICE_CHARS
            {
                prefix = format!(
                    "{}{prose}",
                    dux_core::device_label::truncate_chars(device, budget)
                );
                break;
            }
        }
        if prefix.is_empty() {
            // No room for a name at all. The fact still fits, and in a pane this
            // narrow the way out matters more than which device took it.
            let nameless = "Driving elsewhere. ";
            if nameless.chars().count() <= room {
                prefix = nameless.to_string();
            }
        }

        let mut spans: Vec<Span> = vec![Span::styled(prefix, cue_style)];
        spans.extend(tail);
        // The key badges borrow locals, so hand back owned spans (the same
        // pattern `scroll_mode_cue_line` uses).
        Line::from(
            spans
                .into_iter()
                .map(|span| Span::styled(span.content.into_owned(), span.style))
                .collect::<Vec<_>>(),
        )
    }

    fn render_agent_terminal(&mut self, frame: &mut Frame, area: Rect, title: &str, focused: bool) {
        let outer_block = self.themed_block(title, focused);
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
        // The hardware caret follows KEYS, not just fullscreen-interactive
        // mode: the minimized typeable pane receives keystrokes too, and the
        // caret is both the "your keys land here" cue and what anchors IME
        // composition popups. While scrolled back the cursor cell maps out of
        // the viewport (the bounds check below skips it), so the caret
        // vanishes there on both regimes without extra gating.
        let receives_keys = is_input || self.center_typeable();
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
        // Keyed by target, so a switch to a different PTY always sends once even
        // when the pane happens to measure the same. See `last_pty_resize_target`.
        let resize_target = self.selected_terminal_surface_id();
        let target_changed = self.last_pty_resize_target != resize_target;
        let should_resize =
            (new_size != self.last_pty_size || target_changed) && new_size.0 > 0 && new_size.1 > 0;
        // THE SIZING CLAIM. One PTY has one authoritative grid, the driver's, so
        // while a background web server is serving this pane may only re-grid the
        // child when it is the one driving. A refusal is not a failure: this pane
        // renders the child's real grid (clipped when it is larger than the pane,
        // which it already does safely) and the hint bar names the device whose
        // geometry it is. A GRANTED resize publishes its grid through the seam, so
        // web watchers adopt it.
        //
        // Asked only when a resize would actually be sent, so merely looking at a
        // pane claims nothing, and asked BEFORE the dedupe state is written: an
        // armed take-over is spent inside this call.
        let resize_granted = should_resize
            && match resize_target.as_deref() {
                Some(pty_id) => {
                    let pty_id = pty_id.to_string();
                    self.resize_pty_if_permitted(&pty_id, new_size.0, new_size.1)
                }
                // Nothing running under the cursor: there is no child to size and
                // no ownership question to ask.
                None => true,
            };
        // An armed take-over has to be spent or dropped by the FIRST render after
        // it was armed, so it cannot fire later on a pane the user has moved away
        // from. `resize_pty_if_permitted` spends it; this drops it on the frames where no
        // resize is sent at all.
        self.expire_stale_pty_takeover(resize_target.as_deref());
        // Recorded only for a resize that was GRANTED. Recording a refused one
        // was the trap: the pane remembers having sent a geometry it never sent,
        // so when this surface later gets the pty back at the same pane size,
        // nothing is sent and the child keeps the other device's grid for good.
        if should_resize && resize_granted {
            self.last_pty_size = new_size;
            self.last_pty_resize_target = resize_target;
        }

        if let Some(provider) = self.selected_terminal_surface_client() {
            rendered_content = true;
            // The resize itself belongs to `resize_pty_if_permitted`, which owns the whole
            // sizing decision: the claim, the apply order, the child, and the grid
            // announcement that follows a resize which really happened.

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
                // A selection stamped against a full history ring cannot be
                // translated once the grid moves, so retire it here rather than
                // painting the highlight over whatever text has taken its rows.
                self.drop_drifted_selection();
                scrollback_offset = self.snapshot_buf.scrollback_offset;

                // Deliberately NO `Clear.render(term_area, ..)` here. This used
                // to clear the pane whenever the offset differed from the
                // previous frame's, to stop stale cells lingering in ratatui's
                // diff buffer. Measured against ratatui 0.30: it cannot linger
                // and the clear cannot help.
                //
                // `Terminal::swap_buffers` calls `reset()` on the buffer the
                // next frame draws into, and `Clear` is exactly `Cell::reset()`
                // per cell, so the clear only redid work the frame start had
                // already done. What it did NOT redo is the frame-wide `app_bg`
                // fill, which runs before this: a reset cell's background is
                // `Color::Reset` (the host terminal's default). The loop below
                // paints every snapshot cell's colors verbatim, but it skips
                // `WIDE_CHAR_SPACER` cells and anything outside the child's
                // grid, and those skipped cells rely on the frame-wide fill.
                // The clear stripped the theme colour off exactly those cells,
                // and only on the frames where it fired, so the pane flickered.
                //
                // It used to fire on a single frame per user scroll, which
                // nobody could see. Now output always flows and the terminal
                // library holds a scrolled-back view still by incrementing the
                // offset per arriving line, so it fired on nearly every frame
                // and the pane background visibly flipped while the agent
                // talked. Measured at 1800 of 1815 pane cells falling to the
                // terminal default, pinned by
                // `agent_pane_background_survives_a_scrollback_offset_change`.
                //
                // Wide-character spacers do not need it either: the snapshot
                // skips `WIDE_CHAR_SPACER` cells, but a skipped cell is already
                // blank from the frame reset, and `Buffer::diff` widens its
                // invalidation window by `max(current, previous)` symbol width,
                // so a wide glyph replaced by a narrow one still repaints its
                // trailing cell.

                // Read once, from the same snapshot the cells below come from,
                // so every cell in this frame is translated against one scroll
                // state rather than a re-read that could move mid-loop.
                let selection_now = self.snapshot_selection_origin();

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
                    // The child's colors render verbatim in every mode. The
                    // pane used to desaturate when not interactive (dim
                    // foreground, grayscaled background) as a read-only cue;
                    // that cue is now the caret and the hint bar, so the CLI's
                    // own palette always shows end-to-end.
                    let style = Style::default()
                        .fg(to_ratatui_color(cell.fg))
                        .bg(to_ratatui_color(cell.bg))
                        .add_modifier(to_ratatui_modifier(cell.modifier));
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
                    // `contains_live`, not `contains`: the selection's rows are
                    // viewport rows from the frame the drag started in, and the
                    // grid has moved since if the user scrolled or the child
                    // wrote. Testing the raw row would leave the highlight
                    // pinned to screen coordinates while its text slid away.
                    if let Some(sel) = &self.terminal_selection
                        && sel.anchor != sel.end
                        && sel.contains_live(cell.row, cell.col, selection_now)
                    {
                        ratatui_cell.set_style(self.theme.selection_style());
                    }
                }

                // Render the caret whenever this pane receives keys.
                if receives_keys
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
                    && let Some(label) =
                        scrollback_indicator_label(self.snapshot_buf.scrollback_offset)
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
            let exit_key = self.bindings.label_for(Action::ToggleFullscreen);
            let scroll_down = self.bindings.labels_for(Action::ScrollPageDown);
            let scroll_up = self.bindings.labels_for(Action::ScrollPageUp);
            let scroll_line = self.bindings.label_for(Action::ScrollLineDown);
            let focus_agent = self.bindings.labels_for(Action::FocusAgent);
            let reconnect = self.bindings.labels_for(Action::ReconnectAgent);

            let macro_key = self.bindings.label_for(Action::OpenMacroBar);
            let next_pane = self.bindings.label_for(Action::FocusNext);
            let next_tab = self.bindings.label_for(Action::NextTab);
            let live_edge = self.bindings.labels_for(Action::ScrollToBottom);
            // Resolved before the ladder so the branch below stays a plain
            // condition, and asked of the live registry rather than a cached flag.
            let driven_elsewhere = self.focused_pty_driven_elsewhere();
            let hint_line = if is_input && self.scroll_mode_active() {
                // Scroll mode swallows every non-scroll key (see
                // `process_raw_input_bytes`), so while it is on, this line says
                // so instead of listing the usual keys. Without it the only
                // signal is a pane that happens not to be moving, which stops
                // being a signal the moment the pane is live again.
                self.scroll_mode_cue_line()
            } else if let Some(device) = driven_elsewhere.as_deref() {
                // Ordered AFTER scroll mode, which the user turned on themselves
                // and whose own line names the key that turns it off, and BEFORE
                // every hint that names a key aimed at the child: while another
                // device drives this pty, none of those keys reach it.
                // The width the line really has is this pane's, not the window's:
                // the cue is built to fit it, and what gives way is the device
                // name rather than the way out. See `remote_driver_cue_line`.
                self.remote_driver_cue_line(device, hint_area.width)
            } else if is_input {
                // Fullscreen interactive: keys go to the child verbatim, so
                // the line names the way back plus the scroll keys.
                let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
                let mut spans: Vec<Span> = Vec::new();
                spans.extend(self.theme.dim_key_badge_default(&exit_key));
                spans.push(Span::styled(" minimize  ", desc_style));
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
                // Scrolled back: the scroll vocabulary owns the pane. In a
                // typeable pane that also means typing is paused, and the line
                // must say so: the keys silently stop reaching the agent
                // otherwise, and the live-edge key is the way back.
                let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
                let mut spans: Vec<Span> = Vec::new();
                let prefix = if self.center_typeable() {
                    format!("Scrolled back {scrollback_offset} lines. Typing is paused. ")
                } else {
                    format!("Scrolled back {scrollback_offset} lines. ")
                };
                spans.push(Span::styled(
                    prefix,
                    Style::default().fg(self.theme.hint_key_fg),
                ));
                spans.extend(self.theme.dim_key_badge_default(&scroll_down));
                spans.push(Span::styled(" down, ", desc_style));
                spans.extend(self.theme.dim_key_badge_default(&scroll_up));
                spans.push(Span::styled(" up, ", desc_style));
                spans.extend(self.theme.dim_key_badge_default(&scroll_line));
                spans.push(Span::styled(" one line, ", desc_style));
                spans.extend(self.theme.dim_key_badge_default(&live_edge));
                spans.push(Span::styled(" live edge.", desc_style));
                Line::from(spans)
            } else if self.center_typeable() {
                // Windowed typing: keystrokes reach the focused surface's PTY
                // while dux keeps its chords, so the line says where typing
                // goes and names the chords that stay dux's (all resolved
                // through the bindings, never hardcoded).
                let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
                let target = match active_surface {
                    SessionSurface::Agent => "agent",
                    SessionSurface::Terminal => "terminal",
                };
                let mut spans: Vec<Span> = vec![Span::styled(
                    format!("Typing goes to the {target}. "),
                    Style::default().fg(self.theme.hint_key_fg),
                )];
                spans.extend(self.theme.dim_key_badge_default(&exit_key));
                spans.push(Span::styled(" fullscreen  ", desc_style));
                spans.extend(self.theme.dim_key_badge_default(&next_pane));
                spans.push(Span::styled(" next pane", desc_style));
                // Decision 6 says the surviving tab-switch chords are loud in
                // HINTS, not only docs: with plain arrows now typing into the
                // agent, the chords are the only tab keys left, so an agent
                // with something to switch to names the next-tab chord here.
                // A tab hint on a single-tab agent is noise, so it needs 2+.
                let tab_count = self
                    .selected_session()
                    .map(|s| self.session_tab_ids(&s.id).len())
                    .unwrap_or(0);
                if matches!(active_surface, SessionSurface::Agent)
                    && tab_count >= 2
                    && !next_tab.is_empty()
                {
                    spans.push(Span::styled("  ", desc_style));
                    spans.extend(self.theme.dim_key_badge_default(&next_tab));
                    spans.push(Span::styled(" next tab", desc_style));
                }
                if !self.filtered_macros("").is_empty() && !macro_key.is_empty() {
                    spans.push(Span::styled("  ", desc_style));
                    spans.extend(self.theme.dim_key_badge_default(&macro_key));
                    spans.push(Span::styled(" macros", desc_style));
                }
                spans.push(Span::styled(".", desc_style));
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
                } else if session_active {
                    // A live agent whose pane is NOT focused: the activate key
                    // focuses the pane, and from there typing reaches the agent.
                    spans.extend(self.theme.dim_key_badge_default(&focus_agent));
                    spans.push(Span::styled(" focus and type. ", desc_style));
                    spans.extend(self.theme.dim_key_badge_default(&scroll_up));
                    spans.push(Span::styled(" ", desc_style));
                    spans.extend(self.theme.dim_key_badge_default(&scroll_down));
                    spans.push(Span::styled(" to scroll. ", desc_style));
                    spans.extend(self.theme.dim_key_badge_default(&scroll_line));
                    spans.push(Span::styled(" one line.", desc_style));
                } else if session_id.is_some() {
                    spans.push(Span::styled("Agent CLI exited. Press ", desc_style));
                    spans.extend(self.theme.dim_key_badge_default(&reconnect));
                    spans.push(Span::styled(" or ", desc_style));
                    spans.extend(self.theme.dim_key_badge_default(&focus_agent));
                    spans.push(Span::styled(" to launch it again.", desc_style));
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

        // A STANDALONE agent whose folder is not a repository: the region is
        // quiet, and it says WHICH quiet rather than showing an empty file list
        // the user cannot interpret. The sentence is the engine's, shared with
        // the browser, so both surfaces describe the same folder the same way.
        //
        // Ahead of the collapsed rail too: a rail of nothing is no more
        // informative than an empty list, and the pane is the only place this
        // can be said.
        if let Some(reason) = self.quiet_changes_reason() {
            self.render_quiet_changes(frame, area, &reason);
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
    /// Why the changes region is quiet for the selected agent, or `None` when it
    /// is not. Reads the one engine verdict, so this and the mutation gate can
    /// never disagree about a folder.
    fn quiet_changes_reason(&self) -> Option<String> {
        let session = self.selected_session()?;
        let access = self.engine.session_git_access(&session.id)?;
        if access.changes_panel_works() {
            return None;
        }
        let folder = dux_core::home_path::shorten_home(access.directory());
        Some(format!(
            "{}\n\n{folder}",
            access
                .quiet_reason()
                .unwrap_or("dux cannot work with git in this folder.")
        ))
    }

    /// The quiet changes region: the pane's own frame, and the reason inside it.
    ///
    /// Wrapped rather than truncated, because the reason is a sentence that
    /// tells the user what to do next and a clipped one tells them nothing.
    fn render_quiet_changes(&mut self, frame: &mut Frame, area: Rect, reason: &str) {
        let focused = self.focus == FocusPane::Files;
        let block = self.themed_block("Changes", focused);
        let inner = block.inner(area);
        block.render(area, frame.buffer_mut());
        Paragraph::new(reason)
            .wrap(Wrap { trim: false })
            .style(Style::default().fg(self.theme.hint_desc_fg))
            .render(inner, frame.buffer_mut());
        // No list and no rows, so nothing here is clickable: clear the click
        // maps rather than leaving the previous agent's rects behind.
        self.mouse_layout.unstaged_list = None;
        self.mouse_layout.staged_list = None;
    }

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

    /// The help overlay's content, as unwrapped logical lines.
    ///
    /// Split out from [`Self::render_help`] so the wrap can be tested against
    /// ratatui's own `Wrap { trim: false }` on the REAL content: the renderer
    /// pre-wraps these lines itself (see the call site for why), and the only
    /// way to know that pre-wrapping did not change the page's appearance is to
    /// paint both and compare.
    ///
    /// `content_width` sizes the full-width section banners, nothing else.
    fn help_content_lines(&self, content_width: usize) -> Vec<Line<'static>> {
        // Build help content lines.
        let mut lines: Vec<Line<'static>> = Vec::new();

        let banner_style = Style::default()
            .fg(self.theme.help_banner_fg)
            .bg(self.theme.help_banner_bg)
            .add_modifier(Modifier::BOLD);
        let body_style = Style::default().fg(self.theme.help_body_fg);

        // Helper: push a full-width banner line.
        let push_banner = |lines: &mut Vec<Line<'static>>, title: &str, width: usize| {
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
            "dux has two front ends over one engine: a terminal UI",
            body_style,
        )));
        lines.push(Line::from(Span::styled(
            "and a web UI, both driving the same workspace.",
            body_style,
        )));
        lines.push(Line::from(Span::styled(
            "Each project maps to a git worktree, and you can spawn",
            body_style,
        )));
        lines.push(Line::from(Span::styled(
            "unlimited agents, and unlimited companion terminals",
            body_style,
        )));
        lines.push(Line::from(Span::styled(
            "for each agent, all running side by side.",
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
                spans.extend(owned_key_badge(&self.theme, key));
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
            spans.extend(owned_key_badge(&self.theme, key));
            spans.push(Span::raw(" ".repeat(padding)));
            spans.push(Span::styled(
                desc,
                Style::default().fg(self.theme.hint_desc_fg),
            ));
            lines.push(Line::from(spans));
        }
        // Agent pane modes: the authoritative one-paragraph description of the
        // two key-routing regimes, with every key resolved through the
        // bindings so a rebind keeps this text true.
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Agent pane modes",
            Style::default()
                .fg(self.theme.help_section_header_fg)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )));
        {
            let fullscreen_key = self.bindings.label_for(Action::ToggleFullscreen);
            let next_pane_key = self.bindings.label_for(Action::FocusNext);
            let mode_lines = [
                "The agent pane has two modes. Windowed (the normal".to_string(),
                "3-pane layout), typing reaches the agent while dux".to_string(),
                format!("keeps its chords: {next_pane_key} still moves panes, and the tab,"),
                "palette, and scroll keys stay dux's. Fullscreen".to_string(),
                format!("({fullscreen_key}), every key goes to the agent verbatim and"),
                format!("{fullscreen_key} is the way back. A chord dux keeps never"),
                "reaches the agent windowed; rebind it in [keys] to".to_string(),
                "hand its key to the agent.".to_string(),
            ];
            for text in mode_lines {
                lines.push(Line::from(Span::styled(
                    text,
                    Style::default().fg(self.theme.hint_desc_fg),
                )));
            }
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

        lines
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

        let lines = self.help_content_lines(content_area.width as usize);

        // Wrap the content HERE rather than letting the `Paragraph` do it with
        // `Wrap { trim: false }`.
        //
        // This is the fix for a real bug: the clamp below is built from the line
        // count, but a wrapping paragraph renders MORE rows than it has lines and
        // does not report how many. On an 80-column terminal the help pane's
        // content column is ~55 wide while its keybinding rows run to 70+, so
        // they wrapped and the bottom of the page was simply unreachable — the
        // whole Reference section included. Pre-wrapping makes `wrapped.len()`
        // the RENDERED height by construction (every line is at most
        // `content_area.width` wide, so it occupies exactly one row), instead of
        // a guess that has to match ratatui's internal algorithm.
        let wrapped = wrap_styled_lines(&lines, content_area.width as usize);

        // Track content size for scroll clamping in input handler.
        let total_lines = u16::try_from(wrapped.len()).unwrap_or(u16::MAX);
        self.last_help_lines = total_lines;
        self.last_help_height = content_area.height;

        // Clamp scroll offset.
        let max_scroll = total_lines.saturating_sub(content_area.height);
        let scroll = self.help_scroll.unwrap_or(0).min(max_scroll);

        Paragraph::new(wrapped)
            .scroll((scroll, 0))
            .render(content_area, frame.buffer_mut());

        // Scroll marker in the modal's right border column, on the content
        // pane's last row — above the hint bar's own top border. Units are
        // wrapped rows, which is exactly what the clamp above uses.
        render_scroll_marker(
            frame,
            area,
            content_area,
            scroll as usize,
            content_area.height as usize,
            total_lines as usize,
            self.theme.hint_key_fg,
        );

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
        // The standalone delete dialog is rendered ahead of the main match,
        // with its content copied out first, because it needs `&mut self` for
        // the shared frame renderer and the match below borrows `self.prompt`
        // for the whole arm.
        if let PromptState::ConfirmDeleteAgent {
            agent_label,
            target: DeleteAgentTarget::Folder { folder_label },
            focus,
            ..
        } = &self.prompt
        {
            let (agent_label, folder_label, focus) =
                (agent_label.clone(), folder_label.clone(), *focus);
            self.render_dim_overlay(frame);
            let dialog_width = 56.min(frame.area().width.max(1));
            let inner_width = dialog_width.saturating_sub(2);
            // Say plainly that the folder is not dux's to remove and is not
            // being touched, so the user is never left wondering what else went
            // with the record.
            let body_lines = vec![
                Line::from(""),
                Line::from(vec![
                    Span::raw(" Are you sure you want to delete "),
                    Span::styled(agent_label, Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw("?"),
                ]),
                Line::from(""),
                Line::from(Span::styled(
                    " This removes dux's record of the agent only.",
                    Style::default().fg(self.theme.hint_desc_fg),
                )),
                Line::from(Span::styled(
                    format!(" Its folder \"{folder_label}\" is left untouched."),
                    Style::default().fg(self.theme.hint_desc_fg),
                )),
            ];
            self.render_delete_agent_frame(frame, dialog_width, inner_width, body_lines, focus);
            return;
        }
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
                    true,
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
                // Scroll marker in the list block's right border column. This is
                // an ITEM-offset surface, not a line-offset one: a `ListState`
                // offset counts whole items and never clips the top one, so the
                // unit here is items — `state.offset()` (read AFTER the render,
                // which is what scrolls it to the selection), the list viewport
                // in rows (one row per item here), and the command count.
                render_scroll_marker(
                    frame,
                    list_area,
                    list_inner,
                    state.offset(),
                    list_inner.height as usize,
                    commands.len(),
                    self.theme.hint_key_fg,
                );
                self.overlay_layout.active = OverlayMouseLayout::Command {
                    input: input_inner,
                    list: list_inner,
                    items: commands.len(),
                    offset: state.offset(),
                };
            }
            PromptState::BrowseProjects {
                purpose,
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
                    // The browser is shared; only what a pick DOES differs, so the
                    // title says which act the user is in the middle of.
                    let verb = match purpose {
                        BrowsePurpose::AddProject => "Add Project",
                        BrowsePurpose::StandaloneAgent => "Standalone Agent In",
                    };
                    let title = format!("{verb}: {}", current_dir.display());
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
                        true,
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
                    // Scroll marker in the list block's right border column. ITEM
                    // units: a `ListState` offset counts whole entries and never
                    // clips the top one, and every directory row is one line.
                    // `item_count` is the count that matches `items` (completions
                    // while editing a path, filtered entries otherwise); the
                    // placeholder rows ("No matching entries.") report 0, which
                    // reads as unscrollable, as it should.
                    render_scroll_marker(
                        frame,
                        list_render_area,
                        list_inner,
                        state.offset(),
                        list_inner.height as usize,
                        item_count,
                        self.theme.hint_key_fg,
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
                    // The browser is shared; only what a pick DOES differs, so the
                    // title says which act the user is in the middle of.
                    let verb = match purpose {
                        BrowsePurpose::AddProject => "Add Project",
                        BrowsePurpose::StandaloneAgent => "Standalone Agent In",
                    };
                    let title = format!("{verb}: {}", current_dir.display());
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
                    // Same marker on the no-filter layout, where the list fills
                    // the whole modal. ITEM units (see the sibling branch).
                    render_scroll_marker(
                        frame,
                        list_render_area,
                        list_inner,
                        state.offset(),
                        list_inner.height as usize,
                        item_count,
                        self.theme.hint_key_fg,
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

                let bottom_spans = self.provider_picker_footer();

                let [details_area, list_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(4), Constraint::Min(6)])
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
                        // Only a Retarget can be a no-op: a NewTab always
                        // creates something, whatever provider it names.
                        let is_no_op =
                            prompt.mode == ChangeAgentProviderMode::Retarget && option.is_current;
                        let name =
                            format!("{:width$}", option.provider.as_str(), width = provider_col);
                        ListItem::new(Line::from(vec![
                            active_provider_marker_span(is_no_op, &self.theme),
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
                // The rows are the only thing in here, so the selection is
                // always live: there is no button for focus to move to.
                let highlight_style = self.theme.selection_style();
                StatefulWidget::render(
                    List::new(items)
                        .block(list_block)
                        .highlight_style(highlight_style),
                    list_area,
                    frame.buffer_mut(),
                    &mut state,
                );

                self.overlay_layout.active = OverlayMouseLayout::ChangeAgentProvider {
                    list: list_inner,
                    items: prompt.options.len(),
                    offset: state.offset(),
                };
            }
            PromptState::ChangeDefaultProvider(prompt) => {
                self.render_dim_overlay(frame);
                let area = centered_rect(72, 42, frame.area());
                self.clear_overlay_area(frame, area);

                let bottom_spans = self.provider_picker_footer();

                let [details_area, list_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(4), Constraint::Min(6)])
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
                            active_provider_marker_span(option.is_current, &self.theme),
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
                let highlight_style = self.theme.selection_style();
                StatefulWidget::render(
                    List::new(items)
                        .block(list_block)
                        .highlight_style(highlight_style),
                    list_area,
                    frame.buffer_mut(),
                    &mut state,
                );

                self.overlay_layout.active = OverlayMouseLayout::ChangeDefaultProvider {
                    list: list_inner,
                    items: prompt.options.len(),
                    offset: state.offset(),
                };
            }
            PromptState::ChangeProjectDefaultProvider(prompt) => {
                self.render_dim_overlay(frame);
                let area = centered_rect(64, 60, frame.area());
                self.clear_overlay_area(frame, area);

                let bottom_spans = self.provider_picker_footer();

                let [details_area, list_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(5), Constraint::Min(6)])
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
                            active_provider_marker_span(option.is_current, &self.theme),
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
                let highlight_style = self.theme.selection_style();
                StatefulWidget::render(
                    List::new(items)
                        .block(list_block)
                        .highlight_style(highlight_style),
                    list_area,
                    frame.buffer_mut(),
                    &mut state,
                );

                self.overlay_layout.active = OverlayMouseLayout::ChangeProjectDefaultProvider {
                    list: list_inner,
                    items: prompt.options.len(),
                    offset: state.offset(),
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

                // Scroll marker in the list block's right border column (the
                // block carries LEFT|RIGHT|BOTTOM, so that column exists). This
                // is an ITEM-offset surface: a `ListState` offset counts whole
                // items and never clips the top one, and every theme row is a
                // single line, so items and rows agree. `state.offset()` is read
                // AFTER the render, which is what scrolls it to the selection.
                render_scroll_marker(
                    frame,
                    list_area,
                    list_inner,
                    state.offset(),
                    list_inner.height as usize,
                    prompt.options.len(),
                    self.theme.hint_key_fg,
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
                focus,
                ..
            }
            | PromptState::ConfigureProjectEnv {
                project_name,
                input,
                focus,
                ..
            }
            | PromptState::ConfigureGlobalEnv {
                project_name,
                input,
                focus,
                ..
            } => {
                let focus = *focus;
                let is_env = matches!(
                    &self.prompt,
                    PromptState::ConfigureProjectEnv { .. }
                        | PromptState::ConfigureGlobalEnv { .. }
                );
                let is_global_env = matches!(&self.prompt, PromptState::ConfigureGlobalEnv { .. });
                self.render_dim_overlay(frame);
                // Four rows taller than the field-only modal used to be: a
                // blank misclick-safety row plus the three-row Cancel/Save pair
                // the dual-mode rule requires of a full-text field.
                // Wider than the field-only modal was: the footer now names
                // the focus, engage, activate, clear and cancel keys, and a
                // truncated footer is a footer that stops being an answer.
                let area = centered_rect_exact(84, if is_env { 22 } else { 20 }, frame.area());
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
                let [label_area, input_area, _, buttons_area, hint_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(2),
                        Constraint::Min(3),
                        Constraint::Length(1),
                        Constraint::Length(3),
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

                // Two different questions, and conflating them is the bug this
                // modal used to have: FOCUS is "the next keystroke is aimed
                // here", ENGAGED is "the field is swallowing keystrokes". A
                // focused-but-unengaged field takes nothing, so it gets the
                // focus border and no caret, and the footer names the key that
                // engages it.
                let field_focused = focus == ConfigureFieldFocus::Input;
                let engaged = self.input_target == InputTarget::StartupCommand;
                let text_area = self.render_modal_text_field_frame(
                    frame,
                    input_area,
                    field_focused,
                    engaged.then(|| {
                        self.bindings
                            .label_for_reaching(Action::ExitCommitInput, |_| true)
                            .unwrap_or_default()
                    }),
                );

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
                // Exactly one caret, and only on an ENGAGED field. A caret on a
                // field that will not receive your keys is a lie.
                if engaged {
                    let (cursor_row, cursor_col) = render_input.cursor_display_position();
                    let cx = text_area.x + cursor_col as u16;
                    let cy = text_area.y + cursor_row as u16;
                    if cx < text_area.x + text_area.width && cy < text_area.y + text_area.height {
                        frame.set_cursor_position((cx, cy));
                    }
                }

                // The body can hold more than the pane shows, so it carries the
                // shared one-cell marker. Units are wrapped visual rows, which
                // is what `render_input` counts after `set_display_width`.
                render_scroll_marker(
                    frame,
                    input_area,
                    text_area,
                    render_input.scroll_offset(),
                    text_area.height as usize,
                    render_input.total_lines(),
                    self.theme.hint_desc_fg,
                );

                // ── Cancel / Save ─────────────────────────────────────────
                // Cancel discards typing, so the pair sits behind a blank row
                // and a wide gap (the misclick-safety tenet).
                let btn_width = 16u16;
                let gap = 6u16;
                let total = btn_width * 2 + gap;
                let left_offset = buttons_area.width.saturating_sub(total) / 2;
                let cancel_button = Rect {
                    x: buttons_area.x + left_offset,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };
                let save_button = Rect {
                    x: cancel_button.x + btn_width + gap,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };
                Button::new("Cancel")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfigureFieldCancel,
                        self.pressed_button,
                        focus == ConfigureFieldFocus::Cancel,
                        true,
                    ))
                    .render(frame, cancel_button, &self.theme);
                Button::new("Save")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfigureFieldSave,
                        self.pressed_button,
                        focus == ConfigureFieldFocus::Save,
                        true,
                    ))
                    .render(frame, save_button, &self.theme);

                // ── Hints. Every key is resolved through the bindings. ────
                let close_key = self.bindings.label_for(Action::CloseOverlay);
                let hints: Vec<Hint> = if engaged {
                    vec![
                        Hint::key(
                            self.bindings.labels_for(Action::ExitCommitInput),
                            "stop editing",
                        ),
                        Hint::key(self.bindings.label_for(Action::ClearTextField), "clear"),
                    ]
                } else {
                    let mut hints = vec![Hint::maybe_key(
                        self.bindings
                            .label_for_text_field_dialog(Action::ToggleSelection),
                        "move focus",
                    )];
                    if field_focused {
                        // Named because an unengaged full-text field takes no
                        // keystrokes, so the user has to be told how to start.
                        hints.push(Hint::key(
                            self.bindings.label_for(Action::EngageCommitInput),
                            "edit text",
                        ));
                    }
                    if !field_focused {
                        // Space-on-focus is hardcoded (the accessibility
                        // tenet), so there is no binding to resolve for it.
                        // The segment is a promise about what Space does RIGHT
                        // NOW, so it is as state-aware as the `move focus`
                        // segment above: on the unengaged body Space does
                        // nothing at all, and a footer may be incomplete but
                        // may never be WRONG.
                        hints.push(Hint::plain("Space act on focus"));
                    }
                    if field_focused {
                        // The clear key empties the FOCUSED full-text field, so
                        // it only answers while the body has focus; naming it
                        // from a button stop would promise a key nothing
                        // answers.
                        hints.push(Hint::key(
                            self.bindings.label_for(Action::ClearTextField),
                            "clear",
                        ));
                    }
                    hints.push(Hint::key(close_key, "cancel"));
                    hints
                };
                Paragraph::new(modal_hint_line(&self.theme, &hints))
                    .render(hint_area, frame.buffer_mut());
                self.overlay_layout.active = OverlayMouseLayout::ConfigureStartupCommand {
                    input: text_area,
                    cancel_button,
                    save_button,
                };
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
                let focus_key = self
                    .bindings
                    .label_for_text_field_dialog(Action::ToggleSelection);
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
                let page_keys = format!(
                    "{}/{}",
                    self.bindings.label_for(Action::ScrollPageUp),
                    self.bindings.label_for(Action::ScrollPageDown)
                );
                bottom_spans.extend(self.theme.key_badge_default(&page_keys));
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
                // The focus key, named through the bindings and skipping any
                // key the filter would type instead. Without it the Close
                // button is reachable but undiscoverable.
                if let Some(focus_key) = &focus_key {
                    bottom_spans.extend(self.theme.key_badge_default(focus_key));
                    bottom_spans.push(Span::styled(
                        " focus  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                }
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
                            let lines = vec![
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
                            ];
                            // The click mapping divides a screen row by this
                            // height to reach an item index; a third line here
                            // would silently select the wrong run.
                            debug_assert_eq!(
                                lines.len(),
                                usize::from(crate::app::input::STARTUP_LOG_ROW_HEIGHT)
                            );
                            ListItem::new(lines)
                        })
                        .collect::<Vec<_>>()
                };
                let selected_visual =
                    Self::startup_command_log_selected_visual_index(prompt, &visible_indices);
                let mut state = ListState::default().with_selected(selected_visual);
                let mut filter_input_rect: Option<Rect> = None;
                if let Some(filter_area) = filter_area {
                    let filter_block = Block::default()
                        .title(" Filter ")
                        .borders(Borders::ALL)
                        .border_set(border::ROUNDED)
                        // See `Theme::overlay_field_border_style`: drawing the
                        // unfocused state from `overlay_border` and the focused
                        // one from `border_focused` is the same colour.
                        .border_style(self.theme.overlay_field_border_style(prompt.searching))
                        .style(Style::default().bg(self.theme.overlay_bg));
                    let filter_inner = filter_block.inner(filter_area);
                    filter_input_rect = Some(filter_inner);
                    filter_block.render(filter_area, frame.buffer_mut());
                    let text = if prompt.filter.is_empty() && prompt.searching {
                        "type to filter logs"
                    } else {
                        prompt.filter.text.as_str()
                    };
                    if prompt.filter.is_empty() {
                        Paragraph::new(text)
                            .style(Style::default().fg(self.theme.hint_desc_fg))
                            .render(filter_inner, frame.buffer_mut());
                    } else {
                        // The one single-line renderer, never a hand-rolled
                        // copy.
                        Paragraph::new(render_single_line_cursor_input(
                            "",
                            &prompt.filter.text,
                            prompt.filter.cursor,
                            self.theme.input_cursor_fg,
                            self.theme.input_cursor_bg,
                            prompt.searching,
                        ))
                        .style(Style::default().fg(self.theme.text_fg))
                        .render(filter_inner, frame.buffer_mut());
                    }
                    if prompt.searching {
                        // A caret column is a DISPLAY column. `filter.cursor`
                        // is a byte offset, so using it directly put the caret
                        // past the end of anything non-ASCII.
                        let cursor_x = filter_inner
                            .x
                            .saturating_add(single_line_caret_column(
                                &prompt.filter.text,
                                prompt.filter.cursor,
                                0,
                            ))
                            .min(filter_inner.x + filter_inner.width.saturating_sub(1));
                        frame.set_cursor_position((cursor_x, filter_inner.y));
                    }
                }
                let list_focused = prompt.focus == StartupCommandLogFocus::List;
                let list_block = Block::default()
                    .title(" Runs ")
                    .borders(Borders::ALL)
                    .border_set(border::ROUNDED)
                    // Focus you cannot see is not focus: the Runs block owns
                    // one of the two focus stops, so its border says whether
                    // it holds it. The row highlight cannot say it instead, a
                    // selection is a value and stays visible either way.
                    .border_style(self.theme.overlay_field_border_style(list_focused))
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

                // Scroll marker in the Output block's own right BORDER column.
                // The body is painted cell-by-cell, so a full-width log line
                // owns its last content column; the border column is the only
                // cell nothing else can want. Units are wrapped visual ROWS
                // (`startup_command_log_visual_lines` pre-splits to the body
                // width), the same measure `max_scroll` above clamps with. The
                // sibling "Runs" list is deliberately left unmarked.
                render_scroll_marker(
                    frame,
                    body_area,
                    body_inner,
                    scroll_offset as usize,
                    body_inner.height as usize,
                    content_lines.len(),
                    self.theme.hint_key_fg,
                );

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
                        prompt.focus == StartupCommandLogFocus::Close,
                        true,
                    ))
                    .render(frame, close_area, &self.theme);
                self.overlay_layout.active = OverlayMouseLayout::StartupCommandLogs {
                    input: filter_input_rect,
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
            PromptState::ManageWorktrees(prompt) => {
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
                    " remove  ",
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
                        self.themed_overlay_block("Manage Worktrees")
                            .title_bottom(Line::from(bottom_spans)),
                    )
                    .render(details_area, frame.buffer_mut());

                let rows = manage_worktree_visual_rows(
                    &prompt.entries,
                    prompt.loading,
                    prompt.error.as_deref(),
                );
                let path_col = prompt
                    .entries
                    .iter()
                    .map(|entry| {
                        entry
                            .path
                            .file_name()
                            .map_or(0, |name| name.to_string_lossy().chars().count())
                    })
                    .max()
                    .unwrap_or(8)
                    .clamp(8, 24);
                let items = rows
                    .iter()
                    .map(|row| match row {
                        ManageWorktreeVisualRow::Header(label) => {
                            ListItem::new(Line::from(Span::styled(
                                format!(" {label}"),
                                Style::default()
                                    .fg(self.theme.help_section_header_fg)
                                    .add_modifier(Modifier::BOLD),
                            )))
                        }
                        ManageWorktreeVisualRow::Empty(message) => {
                            ListItem::new(Line::from(Span::styled(
                                format!("  {message}"),
                                Style::default().fg(self.theme.hint_dim_desc_fg),
                            )))
                        }
                        ManageWorktreeVisualRow::Entry(index) => {
                            let entry = &prompt.entries[*index];
                            let removable = entry.is_removable();
                            let name_style = if removable {
                                Style::default().fg(self.theme.text_fg)
                            } else {
                                Style::default().fg(self.theme.hint_dim_desc_fg)
                            };
                            let branch_label_style = Style::default().fg(if removable {
                                self.theme.branch_fg
                            } else {
                                self.theme.hint_dim_desc_fg
                            });
                            let branch_value_style = Style::default().fg(if removable {
                                self.theme.hint_desc_fg
                            } else {
                                self.theme.hint_dim_desc_fg
                            });
                            // Dirtiness and the holding agent are the two facts
                            // the web's list carries too, so the two managers
                            // show the same thing.
                            let mut suffix_spans = Vec::new();
                            if entry.dirty {
                                suffix_spans.push(Span::styled(
                                    "  uncommitted changes",
                                    Style::default().fg(self.theme.warning_fg),
                                ));
                            }
                            if !removable {
                                suffix_spans.push(Span::styled(
                                    "  held by an agent",
                                    Style::default().fg(self.theme.hint_dim_desc_fg),
                                ));
                            }
                            let name = git::ellipsize_middle(
                                entry
                                    .path
                                    .file_name()
                                    .map(|name| name.to_string_lossy().to_string())
                                    .unwrap_or_else(|| entry.path.display().to_string())
                                    .as_str(),
                                path_col,
                            );
                            let mut spans = vec![
                                Span::styled(
                                    format!("  {:path_col$}", name),
                                    name_style.add_modifier(Modifier::BOLD),
                                ),
                                Span::styled("  branch: ", branch_label_style),
                                Span::styled(entry.label.as_str(), branch_value_style),
                            ];
                            spans.extend(suffix_spans);
                            ListItem::new(Line::from(spans))
                        }
                    })
                    .collect::<Vec<_>>();
                let selected_visual = prompt.selected.and_then(|selected| {
                    rows.iter().position(|row| {
                        matches!(row, ManageWorktreeVisualRow::Entry(index) if *index == selected)
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
                self.overlay_layout.active = OverlayMouseLayout::ManageWorktrees {
                    list: list_inner,
                    items: rows.len(),
                    offset: state.offset(),
                };
            }
            PromptState::ConfirmDeleteWorktree(prompt) => {
                self.render_dim_overlay(frame);
                let dialog_width = 60.min(frame.area().width.max(1));
                let inner_width = dialog_width.saturating_sub(2);
                let has_checkbox = prompt.has_branch_checkbox();
                let checkbox_label = delete_worktree_checkbox_label(prompt.branch.as_deref());
                let checkbox_height = if has_checkbox {
                    let state = if prompt.focus == DeleteWorktreeFocus::Checkbox {
                        CheckboxState::Focused
                    } else {
                        CheckboxState::Normal
                    };
                    let checkbox = Checkbox::new(checkbox_label.as_str())
                        .checked(prompt.delete_branch)
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

                // The copy is the web dialog's, sentence for sentence, and both
                // sides pin it (see `delete_worktree_title` and the constants
                // beside it).
                let mut body_lines = vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        format!(" {}", delete_worktree_title(&prompt.label)),
                        Style::default().add_modifier(Modifier::BOLD),
                    )),
                    Line::from(""),
                    Line::from(vec![
                        Span::raw(" "),
                        Span::styled(
                            prompt.path.display().to_string(),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            format!(" will be removed from disk. {DELETE_WORKTREE_FORCED}"),
                            Style::default().fg(self.theme.warning_fg),
                        ),
                    ]),
                ];
                if prompt.dirty {
                    body_lines.push(Line::from(Span::styled(
                        format!(" {DELETE_WORKTREE_DIRTY}"),
                        Style::default().fg(self.theme.warning_fg),
                    )));
                }
                match (prompt.branch.as_deref(), prompt.delete_branch) {
                    (None, _) => body_lines.push(Line::from(Span::styled(
                        format!(" {DELETE_WORKTREE_DETACHED}"),
                        Style::default().fg(self.theme.hint_desc_fg),
                    ))),
                    (Some(branch), delete_branch) => body_lines.push(Line::from(Span::styled(
                        format!(" {}", delete_worktree_branch_line(branch, delete_branch)),
                        Style::default().fg(if delete_branch {
                            self.theme.warning_fg
                        } else {
                            self.theme.hint_desc_fg
                        }),
                    ))),
                }
                let body_height = wrapped_line_count(&body_lines, inner_width, false);
                let checkbox_spacing = u16::from(has_checkbox);
                let area = centered_rect_exact(
                    dialog_width,
                    2 + body_height + checkbox_spacing + checkbox_height + 1 + 3,
                    frame.area(),
                );
                self.clear_overlay_area(frame, area);
                let outer = self.themed_overlay_block("Delete Worktree");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                let [body_area, _, checkbox_area, _, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(body_height),
                        Constraint::Length(checkbox_spacing),
                        Constraint::Length(checkbox_height),
                        // Misclick-safe spacing: the checkbox never sits flush
                        // against the destructive button.
                        Constraint::Length(1),
                        Constraint::Length(3),
                    ])
                    .areas(inner);

                Paragraph::new(body_lines)
                    .wrap(Wrap { trim: false })
                    .render(body_area, frame.buffer_mut());

                let checkbox_rect = if has_checkbox {
                    let checkbox_state = if prompt.focus == DeleteWorktreeFocus::Checkbox {
                        CheckboxState::Focused
                    } else {
                        CheckboxState::Normal
                    };
                    let (rect, _) = self.render_overlay_checkbox(
                        frame,
                        checkbox_area,
                        checkbox_label.as_str(),
                        prompt.delete_branch,
                        checkbox_state,
                        None,
                    );
                    Some(OverlayCheckbox {
                        id: OverlayCheckboxId::DeleteWorktreeBranch,
                        rect,
                    })
                } else {
                    None
                };

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
                let delete_area = Rect {
                    x: cancel_area.x + btn_width + gap,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                Button::new("Cancel")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmDeleteWorktreeCancel,
                        self.pressed_button,
                        prompt.focus == DeleteWorktreeFocus::Cancel,
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new("Delete worktree")
                    .kind(ButtonKind::Danger)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmDeleteWorktreeConfirm,
                        self.pressed_button,
                        prompt.focus == DeleteWorktreeFocus::Delete,
                        true,
                    ))
                    .render(frame, delete_area, &self.theme);

                self.overlay_layout.active = OverlayMouseLayout::ConfirmDeleteWorktree {
                    cancel_button: cancel_area,
                    delete_button: delete_area,
                    checkbox: checkbox_rect,
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
                if list.searching {
                    // Search mode takes over most of this vocabulary: the
                    // vertical and search keys are plain characters that the
                    // filter now swallows, and the close key leaves search
                    // instead of cancelling. Only the confirm key still means
                    // what it says. The two filterable peers (the project
                    // browser and the kill-running dialog) swap the same way;
                    // they say `done` where this one says `choose`, because
                    // their confirm ends the search while this one PICKS the
                    // highlighted row.
                    bottom_spans.extend(self.theme.key_badge_default(&confirm_key));
                    bottom_spans.push(Span::styled(
                        " choose  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge_default(&close_key));
                    bottom_spans.push(Span::styled(
                        " clear",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                } else {
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
                }

                let [details_area, list_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(3), Constraint::Min(4)])
                    .areas(area);

                // Header row: the `/`-search input while searching (or once a
                // query has been typed), else a plain count line.
                let details_block = self
                    .themed_overlay_block(intent.title())
                    .title_bottom(Line::from(bottom_spans));
                // A field the user cannot click into is a gap: the filter rect
                // is published whenever the filter is the thing being drawn.
                let filter_input_rect = list
                    .is_filtering()
                    .then(|| details_block.inner(details_area));
                if list.is_filtering() {
                    Paragraph::new(render_single_line_cursor_input(
                        "/ ",
                        &list.filter.text,
                        list.filter.cursor,
                        self.theme.input_cursor_fg,
                        self.theme.input_cursor_bg,
                        true,
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
                    input: filter_input_rect,
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
                        .list
                        .selected
                        .min(visible_indices.len().saturating_sub(1)),
                ));
                let show_top_input = prompt.list.is_filtering();
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
                if prompt.list.searching {
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

                let title = if prompt.list.searching {
                    "Kill Running (searching)"
                } else {
                    "Kill Running"
                };
                if let Some(input_area) = top_area {
                    let input_block = self.themed_overlay_block(title);
                    let input_inner = input_block.inner(input_area);
                    Paragraph::new(render_single_line_cursor_input(
                        "/ ",
                        &prompt.list.filter.text,
                        prompt.list.filter.cursor,
                        self.theme.input_cursor_fg,
                        self.theme.input_cursor_bg,
                        true,
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
                // NOTE for whoever comes here to "finish" the picker cleanup:
                // the three provider pickers lost their Cancel/Apply pair
                // because a picker confirms by PICKING and a button label
                // cannot stay truthful once a key is rebound. That rule does
                // not reach these four. They are not a confirm/cancel pair
                // restating Enter; they are four DISTINCT actions (cancel, kill
                // the hovered runtime, kill the marked ones, kill everything
                // the filter currently shows), and three of them have no
                // keyboard equivalent that a footer hint could name instead.
                // Leave them.
                //
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
                        !confirm_prompt.focus.is_confirm(),
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new("Kill")
                    .kind(ButtonKind::Danger)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmKillConfirm,
                        self.pressed_button,
                        confirm_prompt.focus.is_confirm(),
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
                scroll,
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
                // The WHOLE error, never a `take(6)`. A TOML validation failure
                // runs long and its tail is usually the part naming the actual
                // problem, so the dialog scrolls (marker below) instead of
                // dropping lines with nothing to say it did.
                for line in error.lines() {
                    body_lines.push(Line::from(format!(" {line}")));
                }
                let dialog = self.render_error_dialog_body(
                    frame,
                    "Reload Config Failed",
                    dialog_width,
                    body_lines,
                    1 + checkbox_height + 3,
                    *scroll,
                );
                self.last_error_dialog_height = dialog.body.height;
                self.last_error_dialog_lines = dialog.total_rows;

                let [_, checkbox_area, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1),
                        Constraint::Length(checkbox_height),
                        Constraint::Length(3),
                    ])
                    .areas(dialog.rest);

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
            PromptState::AddProjectFailed {
                message, scroll, ..
            } => {
                self.render_dim_overlay(frame);
                let dialog_width = 68.min(frame.area().width.max(1));
                let mut body_lines = vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        " The project could not be added.",
                        Style::default().fg(self.theme.warning_fg),
                    )),
                    Line::from(""),
                ];
                // The WHOLE message, never a `take(6)`: a rejected path plus the
                // git error explaining it runs past six lines, and the tail is
                // the part that says why. The body scrolls instead (marker in the
                // border column).
                for line in message.lines() {
                    body_lines.push(Line::from(format!(" {line}")));
                }
                let dialog = self.render_error_dialog_body(
                    frame,
                    "Add Project Failed",
                    dialog_width,
                    body_lines,
                    3,
                    *scroll,
                );
                self.last_error_dialog_height = dialog.body.height;
                self.last_error_dialog_lines = dialog.total_rows;
                let buttons_area = dialog.rest;

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
            PromptState::FirstLoad(prompt) => {
                self.render_dim_overlay(frame);
                // Height comes from the BODY's row target (floored so the duck is
                // never clipped) plus the border ring, the button row and its
                // rule; `centered_rect_exact` clamps on a short terminal. See
                // `first_load::MIN_BODY_ROWS` for why the body sets it and not
                // the duck.
                let area = first_load::modal_area(frame.area());
                self.clear_overlay_area(frame, area);
                let rendered = first_load::render_modal(frame, area, prompt, &self.theme);
                self.last_first_load_height = rendered.content_height;
                self.last_first_load_lines = rendered.content_lines;
                self.overlay_layout.active = OverlayMouseLayout::FirstLoad {
                    primary_button: rendered.primary_button,
                    secondary_button: rendered.secondary_button,
                };
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
                agent_label,
                target,
                focus,
                delete_worktree,
                ..
            } => {
                self.render_dim_overlay(frame);
                let dialog_width = 56.min(frame.area().width.max(1));
                let inner_width = dialog_width.saturating_sub(2);
                // The managed identity, when there is one. A STANDALONE agent
                // has none, and every "also remove the worktree" affordance
                // below hangs off this `Some`: there is no removal to offer, so
                // there is no checkbox to render and none to tick.
                let managed = match target {
                    crate::app::DeleteAgentTarget::Managed {
                        branch_name,
                        initial_branch,
                        branch_provenance,
                        worktree_shared,
                    } => Some((
                        branch_name.as_str(),
                        initial_branch.as_str(),
                        *branch_provenance,
                        *worktree_shared,
                    )),
                    crate::app::DeleteAgentTarget::Folder { .. } => None,
                };
                let offers_checkbox =
                    managed.is_some_and(|(_, _, _, worktree_shared)| !worktree_shared);
                let checkbox_height = if !offers_checkbox {
                    0
                } else {
                    let (_, _, branch_provenance, _) =
                        managed.expect("offers_checkbox implies a managed target");
                    let state = if *focus == DeleteAgentFocus::Checkbox {
                        CheckboxState::Focused
                    } else {
                        CheckboxState::Normal
                    };
                    let checkbox = Checkbox::new(delete_agent_checkbox_label(branch_provenance))
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
                            agent_label.as_str(),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("?"),
                    ]),
                    Line::from(""),
                ];
                // The FOLDER target is handled before this match (it needs
                // `&mut self` for the shared frame renderer, which the borrow
                // here would not allow), so only the managed shape reaches this
                // arm.
                let Some((branch_name, initial_branch, branch_provenance, worktree_shared)) =
                    managed
                else {
                    return;
                };
                if worktree_shared {
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
                    // dux deletes only the branch it created. When the branch is
                    // the user's, say which one survives and why, so the warning
                    // above is not read as covering the branch too.
                    if !branch_provenance.dux_may_delete_branch() {
                        let kept = if initial_branch.is_empty() {
                            branch_name
                        } else {
                            initial_branch
                        };
                        // The shared reason, never a second wording of it:
                        // this dialog had its own shorter copy, which drifted
                        // from the status line's on the adopted case and would
                        // have claimed "existed before this agent" of a
                        // provenance a future dux writes and this one cannot
                        // read.
                        let reason = branch_provenance.kept_reason();
                        body_lines.push(Line::from(Span::styled(
                            format!(" Branch \"{kept}\" {reason} and is kept."),
                            Style::default().fg(self.theme.hint_desc_fg),
                        )));
                    }
                } else {
                    body_lines.push(Line::from(Span::styled(
                        " Worktree and branch will be preserved on disk.",
                        Style::default().fg(self.theme.hint_desc_fg),
                    )));
                }
                let body_height = wrapped_line_count(&body_lines, inner_width, false);
                let checkbox_spacing = u16::from(!worktree_shared);
                let button_spacing = u16::from(!worktree_shared);
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

                let checkbox_rect = if !worktree_shared {
                    let checkbox_state = if *focus == DeleteAgentFocus::Checkbox {
                        CheckboxState::Focused
                    } else {
                        CheckboxState::Normal
                    };
                    let (rect, _) = self.render_overlay_checkbox(
                        frame,
                        checkbox_area,
                        delete_agent_checkbox_label(branch_provenance),
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
                focus,
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
                        !focus.is_confirm(),
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new("Delete")
                    .kind(ButtonKind::Danger)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmDeleteTerminalConfirm,
                        self.pressed_button,
                        focus.is_confirm(),
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
                focus,
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
                        !focus.is_confirm(),
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new("Close")
                    .kind(ButtonKind::Danger)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmCloseTabConfirm,
                        self.pressed_button,
                        focus.is_confirm(),
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
                focus,
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
                        !focus.is_confirm(),
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new("Quit")
                    .kind(ButtonKind::Danger)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmQuitConfirm,
                        self.pressed_button,
                        focus.is_confirm(),
                        true,
                    ))
                    .render(frame, quit_area, &self.theme);

                self.overlay_layout.active = OverlayMouseLayout::ConfirmQuit {
                    cancel_button: cancel_area,
                    quit_button: quit_area,
                };
            }
            PromptState::ConfirmDiscardFile {
                file_path, focus, ..
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
                        !focus.is_confirm(),
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new("Discard")
                    .kind(ButtonKind::Danger)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmDiscardConfirm,
                        self.pressed_button,
                        focus.is_confirm(),
                        true,
                    ))
                    .render(frame, discard_area, &self.theme);

                self.overlay_layout.active = OverlayMouseLayout::ConfirmDiscardFile {
                    cancel_button: cancel_area,
                    discard_button: discard_area,
                };
            }
            PromptState::ConfirmCreateInitialCommit { path, focus, .. } => {
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
                        !focus.is_confirm(),
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new("Create Commit & Add")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmCreateInitialCommitConfirm,
                        self.pressed_button,
                        focus.is_confirm(),
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
                focus,
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
                        !focus.is_confirm(),
                        true,
                    ))
                    .render(frame, cancel_area, &self.theme);

                Button::new("Initialize & Add")
                    .kind(ButtonKind::Confirm)
                    .state(button_state_for(
                        ButtonPressedTarget::ConfirmInitRepoConfirm,
                        self.pressed_button,
                        focus.is_confirm(),
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
                focus,
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
                        !focus.is_confirm(),
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
                        focus.is_confirm(),
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
                focus,
                branch_named,
                ..
            } => {
                let checkbox_state = if *focus == RenameSessionFocus::RenameBranchCheckbox {
                    CheckboxState::Focused
                } else {
                    CheckboxState::Normal
                };
                let checkbox = Checkbox::new("Also rename the git branch")
                    .checked(*rename_branch)
                    .state(checkbox_state);
                let dialog_width = 62.min(frame.area().width.max(1));
                let inner_width = dialog_width.saturating_sub(2);
                // A standalone agent has no branch, so the checkbox is ABSENT
                // rather than present-and-inert, and the modal shrinks to the
                // one control it really has.
                let checkbox_height = if *branch_named {
                    checkbox
                        .layout(
                            inner_width,
                            checkbox.marker_style(Style::default()),
                            checkbox.label_style(Style::default()),
                        )
                        .height
                        .saturating_add(1)
                } else {
                    0
                };
                let checkbox_spacing = if *branch_named { 1 } else { 0 };
                let area = centered_rect_exact(
                    dialog_width,
                    9 + checkbox_spacing + checkbox_height,
                    frame.area(),
                );
                // The shared chrome trio (dim, clear-and-claim, titled ring).
                // See `modal::App::open_modal_frame` for why the rect claim and
                // the border ring cannot be hand-rolled per modal.
                let inner = self.open_modal_frame(frame, "Rename Agent", area).inner;

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

                // The field draws its caret only while it HAS focus: with the
                // checkbox focused the field takes no keystrokes, so a caret
                // there would be a lie (and the key handler already drops
                // every key on that basis).
                let input_focused = *focus == RenameSessionFocus::Input;
                let input_block = Block::default()
                    .borders(Borders::ALL)
                    .border_set(border::ROUNDED)
                    .border_style(self.theme.overlay_field_border_style(input_focused));
                let input_inner = input_block.inner(input_area);
                Paragraph::new(render_single_line_cursor_input(
                    " ",
                    &input.text,
                    input.cursor,
                    self.theme.input_cursor_fg,
                    self.theme.input_cursor_bg,
                    input_focused,
                ))
                .block(input_block)
                .render(input_area, frame.buffer_mut());

                let checkbox_rect = if *branch_named {
                    let (rect, _) = self.render_overlay_checkbox(
                        frame,
                        checkbox_area,
                        "Also rename the git branch",
                        *rename_branch,
                        checkbox_state,
                        Some(Line::from(Span::styled(
                            format!(
                                "{}Open PRs will still reference the old branch name",
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
                // Same rule as the new-agent modal: the name field owns the
                // letters and the horizontal arrows, so the hint names the
                // first key of the action that still reaches focus movement
                // here (see `text_field_owns_key`). If a rebinding leaves none,
                // the segment is dropped: naming a key that types a character
                // is worse than naming none.
                // Nothing to move focus to when the checkbox is absent, so the
                // segment goes with it rather than naming a key that does
                // nothing.
                let focus_key = if *branch_named {
                    self.bindings
                        .label_for_text_field_dialog(Action::ToggleSelection)
                } else {
                    None
                };
                let hints = modal_hint_line(
                    &self.theme,
                    &[
                        Hint::key(confirm_key, "confirm"),
                        // Dropped entirely when the field swallows every key the
                        // movement action is bound to; the builder owns that rule.
                        Hint::maybe_key(focus_key, "focus"),
                        // Space-on-focus is hardcoded (the accessibility tenet),
                        // so there is no binding to resolve for it. Dropped
                        // while the name field has focus, where Space is a
                        // typed character and toggles nothing.
                        // An empty segment is dropped by the builder.
                        Hint::plain(if *focus == RenameSessionFocus::Input {
                            ""
                        } else {
                            "Space toggle"
                        }),
                        Hint::key(close_key, "cancel"),
                    ],
                );
                Paragraph::new(hints).render(hint_area, frame.buffer_mut());
                self.overlay_layout.active = OverlayMouseLayout::RenameSession {
                    input: input_inner,
                    checkbox: checkbox_rect.map(|rect| OverlayCheckbox {
                        id: OverlayCheckboxId::RenameSessionBranch,
                        rect,
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

                // Same rule as the rename modal: the caret only appears while
                // the field itself has focus.
                let input_focused = *focus == NameNewAgentFocus::Input;
                let input_block = Block::default()
                    .borders(Borders::ALL)
                    .border_set(border::ROUNDED)
                    .border_style(self.theme.overlay_field_border_style(input_focused));
                let input_inner = input_block.inner(input_area);
                Paragraph::new(render_single_line_cursor_input(
                    " ",
                    &input.text,
                    input.cursor,
                    self.theme.input_cursor_fg,
                    self.theme.input_cursor_bg,
                    input_focused,
                ))
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
                // Same rule as the rename modal: the name field owns the
                // letters and the horizontal arrows, so the hint names the
                // first key of the action that still reaches focus movement
                // here, and drops the segment when a rebinding leaves none.
                let toggle_key = self
                    .bindings
                    .label_for_text_field_dialog(Action::ToggleSelection);
                let mut hints = vec![Span::raw(" ")];
                hints.extend(self.theme.key_badge_default(&confirm_key));
                hints.push(Span::styled(
                    " confirm  ",
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                if let Some(toggle_key) = &toggle_key {
                    hints.extend(self.theme.key_badge_default(toggle_key));
                    hints.push(Span::styled(
                        " focus  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                }
                // Dropped while the name field has focus: Space is a typed
                // character there and toggles nothing.
                if *focus != NameNewAgentFocus::Input {
                    hints.push(Span::styled(
                        "Space toggle  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                }
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
            PromptState::PullRequestInput {
                project,
                input,
                focus,
            } => {
                self.render_dim_overlay(frame);
                // The reference-first shape carries a secondary action under the
                // field, so it needs four more rows than the project-first one.
                let has_project = project.is_some();
                let height = if has_project { 8 } else { 12 };
                let area = centered_rect_exact(64, height, frame.area());
                self.clear_overlay_area(frame, area);

                let outer = self.themed_overlay_block("Create Agent From PR");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                let [label_area, input_area, action_area, hint_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(2),
                        Constraint::Length(3),
                        Constraint::Length(if has_project { 0 } else { 4 }),
                        Constraint::Min(1),
                    ])
                    .areas(inner);

                let labels = match project {
                    Some(project) => vec![
                        Line::from(Span::styled(
                            format!(" Project: {}", project.name),
                            Style::default().fg(self.theme.input_label_fg),
                        )),
                        Line::from(Span::styled(
                            " Paste a GitHub PR URL or enter a PR number:",
                            Style::default().fg(self.theme.input_label_fg),
                        )),
                    ],
                    None => vec![
                        Line::from(Span::styled(
                            " Paste a pull request link, or type owner/repo#123:",
                            Style::default().fg(self.theme.input_label_fg),
                        )),
                        Line::from(Span::styled(
                            " dux finds the project that repository is open in.",
                            Style::default().fg(self.theme.hint_desc_fg),
                        )),
                    ],
                };
                Paragraph::new(labels).render(label_area, frame.buffer_mut());

                // Focus is visible or it is not focus: the border and the caret
                // both follow it, so a field that will not take your keystrokes
                // never draws a caret.
                let field_focused = *focus == PullRequestInputFocus::Input;
                let input_block = Block::default()
                    .borders(Borders::ALL)
                    .border_set(border::ROUNDED)
                    .border_style(self.theme.overlay_field_border_style(field_focused));
                let input_inner = input_block.inner(input_area);
                Paragraph::new(render_single_line_cursor_input(
                    " ",
                    &input.text,
                    input.cursor,
                    self.theme.input_cursor_fg,
                    self.theme.input_cursor_bg,
                    field_focused,
                ))
                .block(input_block)
                .render(input_area, frame.buffer_mut());

                // The secondary action, and the misclick-safe blank row above
                // it. Only offered when no project has been chosen: with one
                // already chosen this modal is exactly what it always was.
                let mut choose_button = None;
                if !has_project && action_area.height >= 4 {
                    // The same words the inline refusal of a bare number uses,
                    // so the message and the control it points at agree.
                    let label = "Choose an existing project…";
                    let width = button_width_for(label).min(action_area.width);
                    let button = Rect {
                        x: action_area.x + (action_area.width.saturating_sub(width)) / 2,
                        y: action_area.y + 1,
                        width,
                        height: 3,
                    };
                    Button::new(label)
                        .kind(ButtonKind::Confirm)
                        .state(button_state_for(
                            ButtonPressedTarget::PullRequestChooseProject,
                            self.pressed_button,
                            !field_focused,
                            true,
                        ))
                        .render(frame, button, &self.theme);
                    choose_button = Some(button);
                }

                let confirm_key = self.bindings.label_for(Action::Confirm);
                let close_key = self.bindings.label_for(Action::CloseOverlay);
                let mut hints = vec![Hint::key(
                    confirm_key,
                    if field_focused {
                        "resolve"
                    } else {
                        "choose a project"
                    },
                )];
                if !has_project {
                    hints.push(Hint::maybe_key(
                        self.bindings
                            .label_for_text_field_dialog(Action::ToggleSelection),
                        "move focus",
                    ));
                }
                hints.push(Hint::key(close_key, "cancel"));
                Paragraph::new(modal_hint_line(&self.theme, &hints))
                    .render(hint_area, frame.buffer_mut());
                self.overlay_layout.active = OverlayMouseLayout::PullRequestInput {
                    input: input_inner,
                    choose_project: choose_button,
                };
            }
            PromptState::AttachPullRequestInput {
                current_pr, input, ..
            } => {
                self.render_dim_overlay(frame);
                // Two body rows always (the accepted-forms hint plus spacing),
                // plus one more when there is a current PR to name.
                let has_current = current_pr.is_some();
                let height = if has_current { 9 } else { 8 };
                let area = centered_rect_exact(64, height, frame.area());
                self.clear_overlay_area(frame, area);

                let outer = self.themed_overlay_block("Attach Pull Request");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                let [label_area, input_area, hint_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(if has_current { 3 } else { 2 }),
                        Constraint::Length(3),
                        Constraint::Min(1),
                    ])
                    .areas(inner);

                let mut labels = Vec::new();
                if let Some(current) = current_pr {
                    labels.push(Line::from(Span::styled(
                        format!(" Currently showing {current}; attaching replaces it."),
                        Style::default().fg(self.theme.hint_desc_fg),
                    )));
                }
                labels.push(Line::from(Span::styled(
                    " Enter a PR URL, owner/repo#123, #123, or 123:",
                    Style::default().fg(self.theme.input_label_fg),
                )));
                labels.push(Line::from(Span::styled(
                    " Attaching pins the PR and pauses autodetection.",
                    Style::default().fg(self.theme.hint_desc_fg),
                )));
                Paragraph::new(labels).render(label_area, frame.buffer_mut());

                // The field is the modal's only control, so it is always
                // focused: focused border, caret drawn.
                let input_block = Block::default()
                    .borders(Borders::ALL)
                    .border_set(border::ROUNDED)
                    .border_style(self.theme.overlay_field_border_style(true));
                let input_inner = input_block.inner(input_area);
                Paragraph::new(render_single_line_cursor_input(
                    " ",
                    &input.text,
                    input.cursor,
                    self.theme.input_cursor_fg,
                    self.theme.input_cursor_bg,
                    true,
                ))
                .block(input_block)
                .render(input_area, frame.buffer_mut());

                let confirm_key = self.bindings.label_for(Action::Confirm);
                let close_key = self.bindings.label_for(Action::CloseOverlay);
                let hints = vec![
                    Hint::key(confirm_key, "attach"),
                    Hint::key(close_key, "cancel"),
                ];
                Paragraph::new(modal_hint_line(&self.theme, &hints))
                    .render(hint_area, frame.buffer_mut());
                self.overlay_layout.active =
                    OverlayMouseLayout::AttachPullRequestInput { input: input_inner };
            }
            PromptState::None => {}
        }
    }

    fn render_edit_macros(&mut self, frame: &mut Frame) {
        // Pre-compute the popup layout so we can set the display width for
        // soft-wrapping before taking the immutable borrow on self.prompt.
        let popup = centered_rect_exact(MACRO_EDIT_POPUP.0, MACRO_EDIT_POPUP.1, frame.area());
        {
            // Temporarily borrow prompt mutably to set the text input's
            // viewport to match the available inner area after all borders,
            // labels, and hint rows have been removed. The body is a permanent
            // field now rather than a wizard stage, so it is always synced.
            if let PromptState::EditMacros {
                editing: Some(edit_state),
                ..
            } = &mut self.prompt
            {
                sync_macro_text_input_layout(&mut edit_state.text_input, popup);
            }
        }

        let editing_snapshot = match &self.prompt {
            PromptState::EditMacros { editing, .. } => editing.clone(),
            _ => return,
        };

        // The nested delete-confirm is a small box painted over the still-drawn
        // list, so the list's footer would otherwise stay readable below it,
        // advertising four keys the confirm has taken over: the new and delete
        // keys do nothing, and the confirm and close keys mean Delete and
        // Cancel. Its peers (Delete Terminal, Close Tab, Discard File) render
        // no footer at all, so this one renders none either.
        let delete_confirm_open = matches!(
            &self.prompt,
            PromptState::EditMacros {
                pending_delete: Some(_),
                ..
            }
        );

        self.render_dim_overlay(frame);
        self.clear_overlay_area(frame, popup);

        // Claim None up front so a previous modal's stale rects can never be
        // hit-tested against this one. Each of the three states below then
        // publishes its own: the list its rows, the editor its controls, and
        // the nested delete-confirm its buttons, over the top of either.
        self.overlay_layout.active = OverlayMouseLayout::None;

        if let Some(edit_state) = editing_snapshot {
            self.render_macro_editor(frame, popup, &edit_state);
        } else {
            let PromptState::EditMacros {
                entries, selected, ..
            } = &self.prompt
            else {
                return;
            };
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

                let new_key = self.bindings.label_for(Action::NewMacro);
                Paragraph::new(vec![
                    Line::from(""),
                    Line::from(Span::styled(
                        format!(" No macros defined. Press {new_key} to create one."),
                        Style::default().fg(self.theme.hint_desc_fg),
                    )),
                ])
                .render(msg_area, frame.buffer_mut());

                if !delete_confirm_open {
                    Paragraph::new(modal_hint_line(&self.theme, &self.macro_list_hints()))
                        .render(hint_area, frame.buffer_mut());
                }
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
                            // Three characters, matching `prefix_len` below.
                            Span::styled(" - ", Style::default().fg(self.theme.input_label_fg)),
                        ];
                        let text_preview = text.replace('\n', "↵");
                        // " " + name + " (label)" + " — ", counted in CHARACTERS:
                        // a macro name or surface label can hold multi-byte text
                        // just as the preview can.
                        let prefix_len =
                            1 + name.chars().count() + surface_label.chars().count() + 3;
                        let max_len = (list_area.width as usize).saturating_sub(prefix_len + 2);
                        spans.push(Span::styled(
                            truncate_macro_preview(&text_preview, max_len),
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        ListItem::new(Line::from(spans))
                    })
                    .collect();

                let item_count = items.len();
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
                // A picker's rows are clickable everywhere else in dux; this
                // one published nothing, so it was the one list a mouse could
                // not reach.
                let list_layout = OverlayMouseLayout::EditMacroList {
                    list: list_area,
                    items: item_count,
                    offset: state.offset(),
                };

                if !delete_confirm_open {
                    Paragraph::new(modal_hint_line(&self.theme, &self.macro_list_hints()))
                        .render(hint_area, frame.buffer_mut());
                }
                self.overlay_layout.active = list_layout;
            }
        }

        let pending_delete_snapshot = match &self.prompt {
            PromptState::EditMacros { pending_delete, .. } => {
                pending_delete.as_ref().map(|p| (p.name.clone(), p.focus))
            }
            _ => None,
        };
        if let Some((name, focus)) = pending_delete_snapshot {
            self.render_confirm_delete_macro(frame, &name, focus);
        }
    }

    /// The macro EDITOR: an ordinary modal with a focus concept.
    ///
    /// Layout, top to bottom: name label, name field, body label, body field,
    /// blank, the surface selector, a blank misclick-safe spacer, the
    /// Cancel/Save buttons, and the hint row. Every one of the five controls is
    /// a focus stop and every one renders its focus, because focus you cannot
    /// see is not focus.
    fn render_macro_editor(&mut self, frame: &mut Frame, popup: Rect, state: &MacroEditState) {
        let title = match &state.id {
            Some(name) => format!("Edit Macro: {name}"),
            None => "New Macro".to_string(),
        };
        let outer = self.themed_overlay_block(&title);
        outer.render(popup, frame.buffer_mut());

        let [
            name_label,
            name_area,
            text_label,
            text_area,
            _,
            surface_area,
            _,
            buttons_area,
            hint_area,
        ] = macro_edit_rows(popup);

        let focus = state.focus;
        let engaged = self.macro_text_engaged();
        let label_style = Style::default().fg(self.theme.input_label_fg);

        // ── Name (single line: typing is immediate, no mode) ──────────────
        Paragraph::new(Line::from(Span::styled(
            " Name (identifies this macro):",
            label_style,
        )))
        .render(name_label, frame.buffer_mut());
        let name_inner = self.render_modal_text_field_frame(
            frame,
            name_area,
            focus == MacroEditFocus::Name,
            None,
        );
        // The one single-line renderer, never a hand-rolled copy: it owns the
        // caret model and the character-boundary clamp.
        Paragraph::new(render_single_line_cursor_input(
            " ",
            &state.name_input.text,
            state.name_input.cursor,
            self.theme.input_cursor_fg,
            self.theme.input_cursor_bg,
            focus == MacroEditFocus::Name,
        ))
        .render(name_inner, frame.buffer_mut());

        // ── Body (multiline: needs the engage step, Enter is content) ─────
        let surface_desc = match state.surface {
            MacroSurface::Agent => "agent macro",
            MacroSurface::Terminal => "terminal macro",
            MacroSurface::Both => "agent + terminal macro",
        };
        Paragraph::new(Line::from(Span::styled(
            format!(" Text for the {surface_desc}:"),
            label_style,
        )))
        .render(text_label, frame.buffer_mut());
        let text_inner = self.render_modal_text_field_frame(
            frame,
            text_area,
            focus == MacroEditFocus::Text,
            engaged.then(|| {
                self.bindings
                    .label_for_reaching(Action::ExitCommitInput, |_| true)
                    .unwrap_or_default()
            }),
        );
        for (index, line_text) in state.text_input.visible_lines().iter().enumerate() {
            if index >= text_inner.height as usize {
                break;
            }
            let line_area = Rect::new(
                text_inner.x,
                text_inner.y + index as u16,
                text_inner.width,
                1,
            );
            Paragraph::new(Line::from(Span::raw(format!(" {line_text}"))))
                .render(line_area, frame.buffer_mut());
        }

        // The body can hold more than the pane shows, so it carries the shared
        // one-cell marker. Units are the body's own visual rows.
        render_scroll_marker(
            frame,
            text_area,
            text_inner,
            state.text_input.scroll_offset(),
            text_inner.height as usize,
            state.text_input.total_lines(),
            self.theme.hint_desc_fg,
        );

        // Exactly one hardware caret: on the name field while it has focus, or
        // on the body while the body is ENGAGED. An unengaged body takes no
        // keystrokes, so showing a caret there would be a lie.
        if focus == MacroEditFocus::Name {
            let cursor_col =
                single_line_caret_column(&state.name_input.text, state.name_input.cursor, 1);
            let (cx, cy) = (name_inner.x + cursor_col, name_inner.y);
            if cx < name_inner.x + name_inner.width && cy < name_inner.y + name_inner.height {
                frame.set_cursor_position((cx, cy));
            }
        } else if focus == MacroEditFocus::Text && engaged {
            let (cursor_row, cursor_col) = state.text_input.cursor_display_position();
            let (cx, cy) = (
                text_inner.x + cursor_col as u16 + 1,
                text_inner.y + cursor_row as u16,
            );
            if cx < text_inner.x + text_inner.width && cy < text_inner.y + text_inner.height {
                frame.set_cursor_position((cx, cy));
            }
        }

        // ── Surface selector ──────────────────────────────────────────────
        let selector_focused = focus == MacroEditFocus::Surface;
        let options = [
            (MacroSurface::Agent, "Agent"),
            (MacroSurface::Terminal, "Terminal"),
            (MacroSurface::Both, "Both"),
        ];
        const SURFACE_PREFIX: &str = " Surface:  ";
        const SURFACE_GAP: &str = "    ";
        let mut radio_spans: Vec<Span> = vec![Span::styled(SURFACE_PREFIX, label_style)];
        let mut surface_options = [Rect::default(); 3];
        let mut cursor_x = surface_area.x + SURFACE_PREFIX.chars().count() as u16;
        for (i, (variant, label)) in options.iter().enumerate() {
            if i > 0 {
                radio_spans.push(Span::raw(SURFACE_GAP));
                cursor_x += SURFACE_GAP.chars().count() as u16;
            }
            let selected = *variant == state.surface;
            let bullet = if selected { "● " } else { "○ " };
            // Focus wins over selection: the focused GROUP is what the user is
            // about to act on, and it uses the same `button_active_fg` the
            // focused checkbox marker does.
            let style = if selector_focused {
                Style::default()
                    .fg(self.theme.button_active_fg)
                    .add_modifier(Modifier::BOLD)
            } else if selected {
                label_style
            } else {
                Style::default().fg(self.theme.hint_desc_fg)
            };
            radio_spans.push(Span::styled(bullet, style));
            radio_spans.push(Span::styled(*label, style));
            let width = (bullet.chars().count() + label.chars().count()) as u16;
            surface_options[i] = Rect::new(cursor_x, surface_area.y, width, 1);
            cursor_x += width;
        }
        Paragraph::new(Line::from(radio_spans)).render(surface_area, frame.buffer_mut());

        // ── Cancel / Save ─────────────────────────────────────────────────
        // Cancel discards typing, so the two targets are separated by more than
        // the usual gap and by a blank row above (the misclick-safety tenet).
        let btn_width = 16u16;
        let gap = 6u16;
        let total = btn_width * 2 + gap;
        let left_offset = buttons_area.width.saturating_sub(total) / 2;
        let cancel_button = Rect {
            x: buttons_area.x + left_offset,
            y: buttons_area.y,
            width: btn_width,
            height: 3,
        };
        let save_button = Rect {
            x: cancel_button.x + btn_width + gap,
            y: buttons_area.y,
            width: btn_width,
            height: 3,
        };

        Button::new("Cancel")
            .kind(ButtonKind::Confirm)
            .state(button_state_for(
                ButtonPressedTarget::EditMacroCancel,
                self.pressed_button,
                focus == MacroEditFocus::Cancel,
                true,
            ))
            .render(frame, cancel_button, &self.theme);
        Button::new("Save")
            .kind(ButtonKind::Confirm)
            .state(button_state_for(
                ButtonPressedTarget::EditMacroSave,
                self.pressed_button,
                focus == MacroEditFocus::Save,
                true,
            ))
            .render(frame, save_button, &self.theme);

        // ── Hints. Every key is resolved through the bindings. ────────────
        let hints: Vec<Hint> = if focus == MacroEditFocus::Text && engaged {
            vec![
                Hint::key(
                    self.bindings.labels_for(Action::ExitCommitInput),
                    "stop editing",
                ),
                Hint::key(self.bindings.label_for(Action::ClearTextField), "clear"),
            ]
        } else {
            let mut hints = vec![Hint::maybe_key(
                self.bindings
                    .label_for_text_field_dialog(Action::ToggleSelection),
                "move focus",
            )];
            if focus == MacroEditFocus::Text {
                hints.push(Hint::key(
                    self.bindings.label_for(Action::EngageCommitInput),
                    "edit text",
                ));
            }
            if !matches!(focus, MacroEditFocus::Name | MacroEditFocus::Text) {
                // Space is CONTENT in both kinds of text field: typed into the
                // single-line name, and swallowed by the unengaged body. The
                // segment therefore only appears on the selector and the two
                // buttons, where Space really does act.
                hints.push(Hint::plain("Space act on focus"));
            }
            // The clear key answers on the focused body here too, but the
            // editor's footer is already at the popup's 64-cell inner width
            // with the four hints above, so naming a fifth would truncate the
            // line. The help overlay stays the authoritative reference; a
            // footer may be incomplete, it may never be WRONG.
            hints.push(Hint::key(
                self.bindings.label_for(Action::CloseOverlay),
                "cancel",
            ));
            hints
        };
        Paragraph::new(modal_hint_line(&self.theme, &hints)).render(hint_area, frame.buffer_mut());

        self.overlay_layout.active = OverlayMouseLayout::EditMacros {
            name_input: name_inner,
            text_input: text_inner,
            surface_options,
            cancel_button,
            save_button,
        };
    }

    /// Draw one modal text field's border and return its inner area.
    ///
    /// The focused field is drawn by the shared `overlay_field_border_style`
    /// (`button_active_fg` plus BOLD), never from `border_focused`: that token
    /// is exactly what `overlay_border` defaults to, so a focused/unfocused
    /// pair built from the two is one colour in every shipped theme.
    ///
    /// `engaged_exit_key`, when present, marks the field as ENGAGED (taking
    /// keystrokes) and names the key that leaves edit mode.
    fn render_modal_text_field_frame(
        &self,
        frame: &mut Frame,
        area: Rect,
        focused: bool,
        engaged_exit_key: Option<String>,
    ) -> Rect {
        let border_style = self.theme.overlay_field_border_style(focused);
        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_set(border::ROUNDED)
            .border_style(border_style);
        if let Some(exit_key) = engaged_exit_key {
            block = block.title_bottom(
                Line::from(vec![
                    Span::styled(
                        " editing: ",
                        Style::default().fg(self.theme.button_active_fg),
                    ),
                    Span::styled(exit_key, Style::default().fg(self.theme.hint_key_fg)),
                    Span::styled(" to stop ", Style::default().fg(self.theme.hint_desc_fg)),
                ])
                .right_aligned(),
            );
        }
        let inner = block.inner(area);
        block.render(area, frame.buffer_mut());
        inner
    }

    fn render_confirm_delete_macro(&mut self, frame: &mut Frame, name: &str, focus: ConfirmFocus) {
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
                !focus.is_confirm(),
                true,
            ))
            .render(frame, cancel_area, &self.theme);

        Button::new("Delete")
            .kind(ButtonKind::Danger)
            .state(button_state_for(
                ButtonPressedTarget::ConfirmDeleteMacroConfirm,
                self.pressed_button,
                focus.is_confirm(),
                true,
            ))
            .render(frame, delete_area, &self.theme);

        self.overlay_layout.active = OverlayMouseLayout::ConfirmDeleteMacro {
            cancel_button: cancel_area,
            delete_button: delete_area,
        };
    }

    /// The macro LIST's footer. Every key is resolved through the bindings,
    /// so a rebind moves the hint with it; the four labels used to be the
    /// literals `Enter`, `n`, `d` and `Esc`.
    fn macro_list_hints(&self) -> Vec<Hint> {
        vec![
            Hint::key(self.bindings.label_for(Action::Confirm), "edit"),
            Hint::key(self.bindings.label_for(Action::NewMacro), "new"),
            Hint::key(self.bindings.label_for(Action::DeleteMacro), "delete"),
            Hint::key(self.bindings.label_for(Action::CloseOverlay), "close"),
        ]
    }

    fn render_overlay(&mut self, frame: &mut Frame) {
        // No fullscreen surface draws a tab strip, and the windowed center
        // pane skips `render_agent_tab_strip_if_needed` (the only place that
        // clears the registry) while one is up. Drop the rects here so the
        // geometry a maximized frame leaves behind can never be clicked, even
        // if some future path reaches the tab hit-test.
        if !matches!(self.fullscreen_overlay, FullscreenOverlay::None) {
            self.agent_tab_regions.clear();
        }
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
                let name = session.display_label();
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
        // No tab strip in fullscreen: tabs cannot be switched there, so the
        // boxes would be dead chrome spending three rows of a surface whose
        // whole point is maximum terminal space. The windowed center pane is
        // where tabs render and switch.
        self.render_agent_terminal(frame, area, &title, true);
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

        // Scroll marker in the modal's right BORDER column, on the content
        // pane's last row. It has to be the border column: the log is painted
        // cell-by-cell with `set_string`, so a full-width line owns the pane's
        // last content column and a marker drawn there would eat a character.
        // Units are wrapped visual ROWS — `startup_command_log_visual_lines`
        // pre-splits the content to the pane width, so one entry is one row,
        // which is the same measure `max_scroll` above clamps with.
        let drawn_scroll = viewer.scroll_offset;
        let total_rows = lines.len();
        render_scroll_marker(
            frame,
            area,
            term_area,
            drawn_scroll as usize,
            term_area.height as usize,
            total_rows,
            self.theme.hint_key_fg,
        );

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
        self.clear_overlay_bar_area(frame, bar_area);

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
            true,
        ))
        .block(input_block)
        .render(bar_area, frame.buffer_mut());

        let cursor_col = single_line_caret_column(&query, cursor, 2);
        let cx = input_inner.x + cursor_col;
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
        self.clear_overlay_bar_area(frame, bar_area);

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
            "", &query, cursor, cursor_fg, cursor_bg, true,
        ))
        .block(input_block)
        .render(input_area, frame.buffer_mut());

        // Place hardware cursor inside the input.
        let cursor_col = single_line_caret_column(&query, cursor, 0);
        let cx = input_inner.x + cursor_col;
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
    ///
    /// This is also where the modal's outer rect is RECORDED for the
    /// click-outside dismissal engine (`overlay_dismiss`): a modal rect is a
    /// render-local value that is gone by the time a click arrives, so the one
    /// chokepoint every modal already passes through is what stores it. Last
    /// write wins, so a nested modal (painted after its parent) is the rect
    /// that ends up stored — see [`OverlayMouseLayoutState::frame`].
    ///
    /// Use [`Self::clear_overlay_bar_area`] instead for a strip that is NOT a
    /// modal of its own.
    pub(super) fn clear_overlay_area(&self, frame: &mut Frame, area: Rect) {
        self.overlay_layout.frame.set(Some(area));
        self.clear_overlay_bar_area(frame, area);
    }

    /// Paint `area` as a modal surface WITHOUT claiming it as the topmost
    /// modal's outer rect. For sub-strips that live inside another surface (the
    /// fullscreen log viewer's search bar, the agent pane's macro bar): they
    /// are not dismissible modals, and recording them would hand the dismissal
    /// engine a rect that is smaller than — or unrelated to — the modal the
    /// user sees, turning clicks inside the real modal into dismissals.
    fn clear_overlay_bar_area(&self, frame: &mut Frame, area: Rect) {
        Clear.render(area, frame.buffer_mut());
        frame
            .buffer_mut()
            .set_style(area, Style::default().bg(self.theme.overlay_bg));
    }

    pub(super) fn themed_overlay_block<'a>(&self, title: &'a str) -> Block<'a> {
        Block::default()
            .title(Line::from(Span::styled(
                title,
                Style::default()
                    .fg(self.theme.input_label_fg)
                    .add_modifier(Modifier::BOLD),
            )))
            .borders(Borders::ALL)
            .border_set(border::ROUNDED)
            // The border ring doubles as the modal's refusal cue: while the
            // one-shot blink armed by an outside click on a modal that cannot
            // be dismissed (see `overlay_dismiss`) is in a highlight phase, the
            // ring flashes `overlay_border_refused`. Reusing the border rather
            // than adding an overlay keeps the cue on the one element that
            // already outlines "this window", and nothing inside the modal
            // moves. `refusal_blink_highlight` is false once the cue is over,
            // so the ring returns to `overlay_border` and stays there.
            .border_style(if self.refusal_blink_highlight() {
                Style::default()
                    .fg(self.theme.overlay_border_refused)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(self.theme.overlay_border)
            })
            // Modals are presented after a `Clear.render(..)` which resets the
            // popup cells to `Color::Reset`. Filling the block with overlay_bg
            // means the modal interior — borders, surrounding chrome, the gap
            // around the inner widgets — tracks the active theme instead of
            // reading terminal-default behind the border ring.
            .style(Style::default().bg(self.theme.overlay_bg))
    }

    /// Lay out and paint the scrollable message pane the two error dialogs share
    /// (`ConfigReloadFailed`, `AddProjectFailed`), and hand back the geometry the
    /// caller needs for the controls below it.
    ///
    /// `extra_inner_rows` is everything the caller lays out INSIDE the border ring
    /// below the message (a spacer, a checkbox, a button row). The message pane
    /// takes as many rows as the message needs, capped at what the terminal can
    /// show, so the buttons never get squeezed out by a long error; anything that
    /// does not fit is reached by scrolling.
    fn render_error_dialog_body(
        &self,
        frame: &mut Frame,
        title: &str,
        dialog_width: u16,
        body_lines: Vec<Line<'static>>,
        extra_inner_rows: u16,
        scroll: u16,
    ) -> ErrorDialogLayout {
        let inner_width = dialog_width.saturating_sub(2);
        // Pre-wrap rather than letting the `Paragraph` do it: `wrapped.len()` is
        // then the RENDERED row count by construction, which is the unit the
        // scroll clamp and the marker are measured in. A wrapping paragraph draws
        // more rows than it has lines and never reports how many — the trap the
        // help page hit, where the bottom of the page was unreachable.
        let wrapped = wrap_styled_lines(&body_lines, inner_width as usize);
        let total_rows = u16::try_from(wrapped.len()).unwrap_or(u16::MAX);

        // Cap the message pane so the dialog still fits the terminal WITH its
        // controls. Without the cap a 200-line error would size the dialog past
        // the screen and the layout solver would eat the button row.
        let max_body = frame
            .area()
            .height
            .saturating_sub(2 + extra_inner_rows)
            .max(1);
        let body_height = total_rows.min(max_body);
        let area = centered_rect_exact(
            dialog_width,
            2 + body_height + extra_inner_rows,
            frame.area(),
        );
        self.clear_overlay_area(frame, area);

        // A scroll hint in the bottom border, but only when there is something to
        // scroll: the keys are the Help scope's, so the labels come from the
        // bindings rather than being hardcoded.
        let scrollable = total_rows > body_height;
        let mut block = self.themed_overlay_block(title);
        if scrollable {
            let scroll_up = self.bindings.labels_for(Action::ScrollPageUp);
            let scroll_down = self.bindings.labels_for(Action::ScrollPageDown);
            // Owned spans: `key_badge_default` borrows its label, and the block
            // outlives these locals.
            let owned = |spans: Vec<Span<'_>>| -> Vec<Span<'static>> {
                spans
                    .into_iter()
                    .map(|s| Span::styled(s.content.to_string(), s.style))
                    .collect()
            };
            let mut hint: Vec<Span<'static>> = vec![Span::raw(" ")];
            hint.extend(owned(self.theme.key_badge_default(&scroll_up)));
            hint.push(Span::styled(
                "/",
                Style::default().fg(self.theme.hint_desc_fg),
            ));
            hint.extend(owned(self.theme.key_badge_default(&scroll_down)));
            hint.push(Span::styled(
                " scroll the message",
                Style::default().fg(self.theme.hint_desc_fg),
            ));
            block = block.title_bottom(Line::from(hint));
        }
        let inner = block.inner(area);
        block.render(area, frame.buffer_mut());

        let body = Rect::new(inner.x, inner.y, inner.width, body_height.min(inner.height));
        let rest = Rect::new(
            inner.x,
            inner.y + body.height,
            inner.width,
            inner.height.saturating_sub(body.height),
        );

        // Clamp what we DRAW, so a stale offset (the message just got shorter, or
        // the terminal grew) agrees with the marker below.
        let max_scroll = total_rows.saturating_sub(body.height);
        let scroll = scroll.min(max_scroll);
        Paragraph::new(wrapped)
            .scroll((scroll, 0))
            .render(body, frame.buffer_mut());

        // Marker in the dialog's right BORDER column, on the message pane's last
        // row — clear of the checkbox and buttons below. Units are wrapped rows,
        // exactly what the clamp above uses.
        render_scroll_marker(
            frame,
            area,
            body,
            scroll as usize,
            body.height as usize,
            total_rows as usize,
            self.theme.hint_key_fg,
        );

        ErrorDialogLayout {
            body,
            rest,
            total_rows,
        }
    }

    fn center_pane_agent_title(&self) -> String {
        if let Some(session) = self.selected_session() {
            // Reflect the FOCUSED tab's provider, not just the Main one.
            let provider = capitalize(self.focused_tab_provider(session).as_str());
            let base = format!("{provider} agent");
            // Who else is looking at the terminal this caption names. The FOCUSED
            // tab's own count, not the agent's total: the caption titles one pane
            // showing one tab, and the sidebar row is where the agent-wide number
            // belongs.
            let remote = self
                .engine
                .providers
                .get(&self.focused_tab_id(&session.id))
                .map(|client| client.subscriber_count())
                .and_then(remote_viewers_segment)
                .map(|segment| format!(" · {segment}"))
                .unwrap_or_default();
            let base = format!("{base}{remote}");
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
        // Resolved, not hardcoded: the handler answers to `Action::Confirm`,
        // so the badge has to follow a rebind. "Scroll" below is a mouse
        // gesture and has no binding to look up.
        let expand_key = self.bindings.label_for(Action::Confirm);
        spans.extend(self.theme.key_badge_default(&expand_key));
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

    pub(super) fn render_dim_overlay(&self, frame: &mut Frame) {
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

/// Build a terminal row's three lines (primary label, owner + state word, and a
/// trailing blank spacer) so a companion-terminal row is structurally identical
/// to an agent row and the two lists space evenly.
///
/// Line one: a state glyph plus the primary label. The glyph follows the same
/// typing -> working -> idle rule the agent row uses, minus the detached and
/// attention states a terminal can never be in: `TYPING_GLYPH` in
/// `session_typing` while typing, the shared spinner in `session_working` while
/// working, else a steady dot in `session_active`. The primary label is the foreground command when
/// something is running, otherwise a plain "Terminal".
///
/// Line two: an owner marker, the owner's display name (the agent's title or
/// branch, or the project's name), and the colored state word — reusing
/// `fit_agent_meta_line` so the marker and word stay fixed while the owner name
/// truncates char-safely. The state word wording matches the agent row exactly
/// ("Typing" / "Working" / "Idle").
///
/// A STANDALONE terminal has no owner to mark, so its marker is the standalone
/// star in the standalone identity tone, with the directory label wearing the
/// tone alongside it: the same indicator a standalone agent's folder line
/// wears, meaning "this one lives in your folder". Owned terminals keep the
/// muted return arrow, where it means "owned by".
#[allow(clippy::too_many_arguments)]
fn terminal_row_lines(
    theme: &crate::theme::Theme,
    typing: bool,
    working: bool,
    spinner: char,
    // The resolved display title (already normalized + collision-disambiguated by
    // the shared `terminal_title` rule), or `None` when the terminal is idle.
    display_title: Option<&str>,
    owner_name: &str,
    // Whether this terminal is standalone (owned by nothing at all), resolved by
    // the caller through an exhaustive match on the owner.
    standalone: bool,
    text_width: u16,
    elapsed_ms: u128,
    // The live sidebar query and the style to emphasize its hit with, or `None`
    // when nothing is being filtered. The owner element is a searched field, so
    // what matched is shown as matched.
    highlight: Option<(&str, Style)>,
) -> (Line<'static>, Line<'static>) {
    // The glyph SHAPE encodes state (typing caret, running spinner, else the
    // steady dot); its color is the neutral identity color, never the live-state
    // color (that lives on the state word below).
    let glyph = if typing {
        crate::theme::TYPING_GLYPH.to_string()
    } else if working {
        spinner.to_string()
    } else {
        crate::theme::DOT_GLYPH.to_string()
    };
    let base_color = theme.session_active;
    // A running terminal names the row with its display title (the foreground app,
    // with a "(#N)" ordinal when a same-owner sibling runs the same app); an idle
    // terminal reads a plain "Terminal" (the owner on line two and row order
    // distinguish several idle terminals, so the "Terminal N" number is not
    // surfaced here).
    let primary = match display_title {
        Some(title) if !title.is_empty() => title,
        _ => "Terminal",
    };
    // The label shimmers while the terminal is Running (a live cue that replaces
    // the old state coloring); otherwise it is a single plain span.
    let name_spans: Vec<Span<'static>> = match (working, base_color) {
        (true, Color::Rgb(r, g, b)) => {
            crate::shimmer::shimmer_spans(primary, (r, g, b), elapsed_ms)
        }
        _ => vec![Span::styled(
            primary.to_string(),
            Style::default().fg(base_color),
        )],
    };
    let mut spans = vec![Span::styled(
        format!("{glyph} "),
        Style::default().fg(base_color),
    )];
    spans.extend(name_spans);
    let line1 = ellipsize_spans(spans, text_width);

    let muted = theme.provider_label_fg;
    // Priority is the core-owned `row_state::terminal_row_state` (twin of the
    // web's `terminalStateWord`); this surface only words it. The busy word is
    // "Running" (a terminal runs a process; it does not "work" like an agent);
    // agents keep "Working".
    let word = match dux_core::row_state::terminal_row_state(working, typing) {
        dux_core::row_state::RowState::Typing => "Typing",
        dux_core::row_state::RowState::Busy => "Running",
        _ => "Idle",
    };
    let word_color = if typing {
        theme.session_typing
    } else if working {
        theme.session_working
    } else {
        muted
    };
    // A two-space indent aligns the owner marker under the label column,
    // echoing the agent row's "  ※ " project marker. An owned terminal wears
    // the muted return arrow ("owned by"); a standalone terminal wears the
    // standalone star and its directory in the standalone identity tone, the
    // one indicator every standalone row shares.
    let (marker, marker_fg) = if standalone {
        (
            format!("  {} ", crate::theme::STANDALONE_GLYPH),
            theme.standalone_location_fg,
        )
    } else {
        ("  ↳ ".to_string(), muted)
    };
    let line2 = fit_agent_meta_line(
        text_width,
        Span::styled(marker, Style::default().fg(marker_fg)),
        Some(Span::styled(
            owner_name.to_string(),
            Style::default().fg(marker_fg),
        )),
        Span::styled(word.to_string(), Style::default().fg(word_color)),
        None,
        Vec::new(),
        MetaLineStyle {
            sep: Style::default().fg(muted),
            highlight,
        },
    );

    // The two content lines; the trailing blank spacer is added by
    // `framed_row_item` at the call site so the shape stays shared with agents.
    (Line::from(line1), Line::from(line2))
}

/// The theme's key badge with OWNED content.
///
/// `Theme::key_badge_default` borrows the key label, which is fine for spans
/// built and rendered in one breath. The help content is built into
/// `Line<'static>` so it can be pre-wrapped and measured before it is drawn, and
/// the labels it badges are `String`s owned by a local, so those spans have to
/// own their text.
fn owned_key_badge(theme: &Theme, key: &str) -> Vec<Span<'static>> {
    theme
        .key_badge_default(key)
        .into_iter()
        .map(|span| Span::styled(span.content.into_owned(), span.style))
        .collect()
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

/// The one footer the three provider pickers share.
///
/// Every key is resolved through the bindings, never spelled out: a rebound
/// key must not be able to make this hint lie. The list of stops is short
/// because a picker HAS no other controls, only rows.
impl App {
    fn provider_picker_footer(&self) -> Vec<Span<'static>> {
        let move_down = self.bindings.label_for(Action::MoveDown);
        let move_up = self.bindings.label_for(Action::MoveUp);
        let confirm = self.bindings.label_for(Action::Confirm);
        let close = self.bindings.label_for(Action::CloseOverlay);
        let desc = Style::default().fg(self.theme.hint_desc_fg);

        // The badges borrow their key string, so take ownership before the
        // locals go out of scope.
        let badge = |key: &str| -> Vec<Span<'static>> {
            self.theme
                .key_badge_default(key)
                .into_iter()
                .map(|span| Span::styled(span.content.into_owned(), span.style))
                .collect()
        };

        let mut spans = vec![Span::raw(" ")];
        spans.extend(badge(&move_down));
        spans.push(Span::styled("/", desc));
        spans.extend(badge(&move_up));
        spans.push(Span::styled(" move  ", desc));
        spans.extend(badge(&confirm));
        spans.push(Span::styled(" choose  ", desc));
        spans.extend(badge(&close));
        spans.push(Span::styled(" cancel", desc));
        spans
    }
}

/// The leading marker column every provider row carries: the marker itself on
/// the row that is already in effect, and blanks of the SAME WIDTH on the rest
/// so the provider names still line up.
fn active_provider_marker_span(is_active: bool, theme: &Theme) -> Span<'static> {
    let marker = if is_active {
        format!(" {ACTIVE_PROVIDER_MARKER} ")
    } else {
        " ".repeat(ACTIVE_PROVIDER_MARKER.chars().count() + 2)
    };
    Span::styled(marker, Style::default().fg(theme.hint_desc_fg))
}

/// Marks the provider row that is ALREADY in effect, so picking it would do
/// nothing.
///
/// This cue used to be the Apply button greying out. The button is gone (a
/// picker confirms by picking, see the `Picker` family in `super::modal`), so
/// the cue moved onto the row that owns the information. It is a marker IN THE
/// TEXT rather than a dimmed style on purpose: the moment the cue matters most
/// is when that row is HIGHLIGHTED, and `Theme::selection_style` sets fg, bg
/// and BOLD, so any style the row set for itself is patched away underneath the
/// selection. A glyph in the string survives.
pub(crate) const ACTIVE_PROVIDER_MARKER: &str = "\u{2713}";

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

/// Trim a macro-list preview to `max_len` COLUMNS, appending an ellipsis when
/// it had to cut.
///
/// Counts and cuts by character, never by byte: the previous byte slice
/// panicked whenever the cut landed inside a multi-byte character, which any
/// macro body holding an accent or an emoji could arrange.
fn truncate_macro_preview(text: &str, max_len: usize) -> String {
    if text.chars().count() <= max_len {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max_len.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Column offset, in DISPLAY CELLS, of a single-line field's caret.
///
/// `cursor` is a BYTE offset into `text` (that is what `TextInput` stores) and
/// `prefix_width` is the cell width of whatever the renderer pads the field
/// with. Neither a byte offset nor a character count is a column: a CJK glyph
/// or an emoji is two cells wide, and a byte offset is wider still. Placing the
/// hardware caret from either drifts right of the glyph it belongs to.
///
/// This is the exact inverse of `input::cursor_from_single_line_position`, so a
/// click and the caret it produces agree about where the caret is.
fn single_line_caret_column(text: &str, cursor: usize, prefix_width: u16) -> u16 {
    let mut cursor = cursor.min(text.len());
    while cursor > 0 && !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
    prefix_width.saturating_add(text[..cursor].cell_width())
}

/// The one single-line text-field renderer.
///
/// `focused` decides whether the caret is painted at all: a field that cannot
/// take a keystroke must not look like it can, so callers whose modal has more
/// than one control pass whether focus actually sits on the field. Callers that
/// own the only control pass `true`.
///
/// The caret offset is a BYTE offset and is clamped to a character boundary
/// before any slicing. Hand-rolled copies of this that split by byte panicked
/// on any name holding an accent or an emoji; that is why they were removed in
/// favour of this function.
fn render_single_line_cursor_input(
    prefix: &str,
    text: &str,
    cursor: usize,
    cursor_fg: Color,
    cursor_bg: Color,
    focused: bool,
) -> Line<'static> {
    if !focused {
        return Line::from(Span::raw(format!("{prefix}{text}")));
    }
    let mut cursor = cursor.min(text.len());
    while cursor > 0 && !text.is_char_boundary(cursor) {
        cursor -= 1;
    }
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

/// The scrollback badge painted at the top-right of the PTY view.
///
/// The badge names the DISTANCE FROM THE LIVE EDGE, and says so in words. That
/// is a deliberate decision, not the incidental shape of the old label, so here
/// is the reasoning. The number is a distance to a moving end, not a position in
/// a fixed document: while the user holds perfectly still, the child keeps
/// printing and the live edge keeps receding, so the number climbs on its own.
/// That is something the user really sees, not a theoretical case: scrolling
/// back holds the VIEW still and nothing else, the reader parses every chunk
/// regardless, so reading history during a busy build means watching this
/// number rise without touching anything. The old
/// `41/800 lines` form reads as a progress ratio through a document of 800
/// lines, and against that reading a numerator that climbs by itself looks like
/// the view scrolling under the user. `41 lines below` reads as "the live edge
/// is 41 lines below you", and a number that climbs is then exactly what the
/// user expects: more output arrived beneath them. The total is dropped for the
/// same reason: it is a moving denominator, and printing it only invites the
/// ratio reading it cannot honestly support.
fn scrollback_indicator_label(scrolled: usize) -> Option<String> {
    if scrolled == 0 {
        return None;
    }

    let noun = if scrolled == 1 { "line" } else { "lines" };
    Some(format!(" {scrolled} {noun} below "))
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

    use crate::app::test_support::{
        agent_provider_prompt, default_bindings, default_provider_prompt, enter_scroll_mode,
        project_default_provider_prompt, test_app, wait_for_agent_cursor,
    };
    use crate::model::{CompanionTerminal, SessionSurface};
    use crate::pty::PtyClient;

    /// The top bar is not blank for a standalone agent: it names the folder
    /// where a project agent's bar names the project and the branch, and it
    /// still carries the provider.
    ///
    /// The old bar wrapped its entire body in "if a project is selected", so a
    /// project-less agent got the dux name and version and nothing else, losing
    /// the provider and the terminal count with it.
    #[test]
    fn the_top_bar_names_a_standalone_agents_folder_and_provider() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        app.selected_left = 1;
        app.engine.sessions[0].workspace =
            dux_core::model::AgentWorkspace::Folder(dux_core::model::FolderWorkspace {
                folder_path: "/home/someone/work/notes".to_string(),
            });
        // No project selected, which is the real shape: a standalone agent's row
        // belongs to no project group.
        app.engine.projects.clear();

        let mut terminal = Terminal::new(TestBackend::new(160, 40)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            rendered.contains("folder:"),
            "the bar must name the folder crumb; got:\n{rendered}"
        );
        assert!(
            rendered.contains("~/work/notes") || rendered.contains("/home/someone/work/notes"),
            "and the folder itself; got:\n{rendered}"
        );
        assert!(
            rendered.contains("provider:"),
            "and keep the provider crumb it used to lose; got:\n{rendered}"
        );
    }

    /// The changes pane says WHY it is quiet for a standalone agent whose folder
    /// is not a repository, matching the browser rather than showing an empty
    /// list the user cannot interpret. Rendered rather than reasoned about: the
    /// sentence has to actually fit on screen and reach the buffer.
    #[test]
    fn the_changes_pane_says_why_a_standalone_folder_is_quiet() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        app.selected_left = 1;
        let id = app.engine.sessions[0].id.clone();
        app.engine.sessions[0].workspace =
            dux_core::model::AgentWorkspace::Folder(dux_core::model::FolderWorkspace {
                folder_path: "/home/someone/notes".to_string(),
            });
        app.engine
            .folder_repo_statuses
            .insert(id, dux_core::git::FolderRepoStatus::NoRepo);

        let mut terminal = Terminal::new(TestBackend::new(140, 40)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        // Asserted as fragments that each fit on one wrapped line: the pane
        // wraps the sentence, and the flattened buffer has no newlines, so a
        // longer phrase would be split by the pane border between rows.
        assert!(
            rendered.contains("This folder has no git"),
            "the pane must say why it is quiet; got:\n{rendered}"
        );
        assert!(
            rendered.contains("/home/someone/notes"),
            "and name the folder it is talking about; got:\n{rendered}"
        );
        assert!(
            !rendered.to_lowercase().contains("busy"),
            "and never that a repository is busy; got:\n{rendered}"
        );
    }

    /// The rename modal for a standalone agent has NO branch checkbox: there is
    /// no branch to rename, and a checkbox that cannot do anything is worse than
    /// none. Rendered rather than reasoned about, because the modal's height is
    /// computed from the checkbox and a stale layout would leave a hole.
    #[test]
    fn the_rename_modal_offers_no_branch_checkbox_for_a_standalone_agent() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        app.selected_left = 1;
        app.engine.sessions[0].title = Some("My Notes".to_string());
        app.engine.sessions[0].workspace =
            dux_core::model::AgentWorkspace::Folder(dux_core::model::FolderWorkspace {
                folder_path: "/home/someone/My Notes".to_string(),
            });
        app.open_rename_session().expect("open the rename modal");

        // The pre-filled name survives verbatim: the refname char map, which
        // rewrites a space to a dash, must not be attached here or the name dux
        // itself derived from the folder could not be submitted as shown.
        let PromptState::RenameSession {
            input,
            rename_branch,
            branch_named,
            ..
        } = &app.prompt
        else {
            panic!("expected the rename modal, got {:?}", app.prompt);
        };
        assert_eq!(input.text, "My Notes");
        assert!(!*branch_named, "a standalone agent has no branch");
        assert!(!*rename_branch, "so no branch rename is requested");

        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            !rendered.contains("Also rename the git branch"),
            "the branch checkbox must be absent, not present-and-inert; got:\n{rendered}"
        );
        assert!(
            rendered.contains("My Notes"),
            "and the pre-filled name is on screen; got:\n{rendered}"
        );

        // A managed agent still gets it, so the absence above is about the
        // workspace and not about the modal having lost its checkbox.
        let mut app = test_app(default_bindings());
        app.selected_left = 1;
        app.open_rename_session().expect("open the rename modal");
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            rendered.contains("Also rename the git branch"),
            "a managed agent's rename still offers the branch checkbox; got:\n{rendered}"
        );
    }

    /// `refresh-changes` on a standalone agent whose folder is not a repository
    /// refuses with the FOLDER's reason, read from the one engine verdict every
    /// other refusal reads, rather than describing the agent's shape in words of
    /// its own ("no worktree to read") that disagreed with the pane beside it.
    #[test]
    fn refreshing_changes_for_a_standalone_folder_gives_the_shared_reason() {
        let mut app = test_app(default_bindings());
        app.selected_left = 1;
        let id = app.engine.sessions[0].id.clone();
        app.engine.sessions[0].workspace =
            dux_core::model::AgentWorkspace::Folder(dux_core::model::FolderWorkspace {
                folder_path: "/home/someone/notes".to_string(),
            });
        app.engine
            .folder_repo_statuses
            .insert(id, dux_core::git::FolderRepoStatus::NoRepo);

        app.refresh_changed_files_now().expect("refresh is handled");
        let message = app.status.message();
        assert!(
            message.contains("This folder has no git repository"),
            "the refusal is the folder's own quiet reason, got: {message}"
        );
        assert!(
            !message.contains("no worktree to read"),
            "and not a sentence about the agent's shape, got: {message}"
        );
    }

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

    fn standalone_row_session(folder: &str) -> AgentSession {
        let app = test_app(default_bindings());
        let mut session = app.engine.sessions[0].clone();
        session.title = Some("notes".to_string());
        session.workspace =
            dux_core::model::AgentWorkspace::Folder(dux_core::model::FolderWorkspace {
                folder_path: folder.to_string(),
            });
        session
    }

    /// A standalone agent's second line names its FOLDER where an ordinary
    /// agent names its project, shortened against the server's home directory.
    /// The orphan arm ("removed project") must be unreachable for it: it has no
    /// project, which is not the same as having lost one.
    #[test]
    fn a_standalone_agents_row_names_its_folder_instead_of_a_project() {
        let session = standalone_row_session("/home/someone/notes");
        let tag = agent_row_owner_tag(&session, None);
        match tag {
            AgentRowOwnerTag::Folder { label } => {
                assert!(label.contains("notes"), "got {label:?}");
            }
            other => panic!("a standalone agent must never take a project arm, got {other:?}"),
        }
    }

    /// The rendered sidebar: a standalone agent's second line carries the
    /// standalone star and its folder in the standalone identity tone, while
    /// the managed row beside it keeps its muted project marker exactly as
    /// before. The star is identity, not state, so it must never borrow a
    /// state color and must never leak onto the managed row.
    #[test]
    fn a_standalone_agents_row_wears_the_standalone_star_in_the_identity_tone() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let mut standalone = app.engine.sessions[0].clone();
        standalone.id = "session-2".to_string();
        standalone.title = Some("Notes".to_string());
        standalone.workspace =
            dux_core::model::AgentWorkspace::Folder(dux_core::model::FolderWorkspace {
                folder_path: "/srv/notes".to_string(),
            });
        app.engine.sessions.push(standalone);
        app.focus = FocusPane::Left;
        app.rebuild_left_items();

        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let buf = terminal.backend().buffer();
        let cell_at = |sym: &str| {
            (0..buf.area.height)
                .flat_map(|y| (0..buf.area.width).map(move |x| (x, y)))
                .find(|&(x, y)| buf[(x, y)].symbol() == sym)
                .unwrap_or_else(|| panic!("no `{sym}` cell rendered"))
        };

        // The star glyph and the folder path share the identity tone.
        let (hx, hy) = cell_at(crate::theme::STANDALONE_GLYPH);
        assert_eq!(
            buf[(hx, hy)].fg,
            app.theme.standalone_location_fg,
            "the standalone star must wear the standalone identity tone"
        );
        // The folder path follows two cells after the glyph ("✷ /srv/notes").
        let path_start = (hx + 2, hy);
        assert_eq!(buf[path_start].symbol(), "/");
        assert_eq!(
            buf[path_start].fg, app.theme.standalone_location_fg,
            "the folder path must wear the identity tone too"
        );
        // No return arrow anywhere: the agent row dropped it and no terminal
        // row exists in this app to legitimately carry one.
        let rendered: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(
            !rendered.contains('↳'),
            "the standalone agent row must not use the return arrow; got:\n{rendered}"
        );

        // The managed row is untouched: the project marker and name keep the
        // muted provider-label color they always had.
        let (mx, my) = cell_at("※");
        assert_eq!(
            buf[(mx, my)].fg,
            app.theme.provider_label_fg,
            "the managed row's project marker must stay muted"
        );
        let project_start = (mx + 2, my);
        assert_eq!(
            buf[project_start].symbol(),
            "d",
            "expected `demo` after `※ `"
        );
        assert_eq!(
            buf[project_start].fg, app.theme.provider_label_fg,
            "the managed row's project name must stay muted"
        );
    }

    /// The branch segment is CONDITIONAL on the row already, so an agent with
    /// no branch must produce no segment at all rather than an empty one that
    /// renders as a stray separator.
    #[test]
    fn a_standalone_agents_row_has_no_branch_segment_at_all() {
        let session = standalone_row_session("/home/someone/notes");
        assert_eq!(agent_row_branch_segment(&session), None);
    }

    /// And the ordinary case still shows the branch when a title hides it.
    #[test]
    fn a_titled_managed_agents_row_still_shows_its_branch() {
        let app = test_app(default_bindings());
        let mut session = app.engine.sessions[0].clone();
        session.title = Some("a nice name".to_string());
        session
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .branch_name = "feature/x".to_string();
        assert_eq!(
            agent_row_branch_segment(&session),
            Some("feature/x".to_string())
        );
    }

    #[test]
    fn agent_state_word_mirrors_the_web_states() {
        use crate::model::SessionStatus::{Active, Detached, Exited};
        // Args are (status, working, typing, needs_attention).
        assert_eq!(agent_state_word(Active, true, false, false), "Working");
        assert_eq!(agent_state_word(Active, false, false, false), "Idle");
        assert_eq!(agent_state_word(Detached, false, false, false), "Detached");
        assert_eq!(agent_state_word(Exited, false, false, false), "Exited");
        // Needs-attention wins over every other state, including working.
        assert_eq!(agent_state_word(Active, true, false, true), "Needs you");
        assert_eq!(agent_state_word(Detached, false, false, true), "Needs you");
    }

    /// The search-hit split: styled spans around the matched char range, the
    /// match carrying the emphasis style, the rest the base style.
    #[test]
    fn search_highlight_spans_split_around_the_match_with_the_match_style() {
        let base = Style::default().fg(Color::White);
        let hit = Style::default().fg(Color::Yellow);
        let spans = search_highlight_spans("API-Refactor", base, hit, (4, 12));
        assert_eq!(
            spans.iter().map(|s| s.content.as_ref()).collect::<Vec<_>>(),
            vec!["API-", "Refactor"],
        );
        assert_eq!(spans[0].style, base);
        assert_eq!(spans[1].style, hit);
    }

    #[test]
    fn search_highlight_spans_split_by_chars_never_bytes() {
        let base = Style::default();
        let hit = Style::default().fg(Color::Yellow);
        // The duck emoji is one CHAR but four UTF-8 bytes; a byte-based split
        // would panic or land mid-character. Range (2, 6) is char indices.
        let spans = search_highlight_spans("🦆 duck", base, hit, (2, 6));
        assert_eq!(
            spans.iter().map(|s| s.content.as_ref()).collect::<Vec<_>>(),
            vec!["🦆 ", "duck"],
        );
        assert_eq!(spans[1].style, hit);
    }

    #[test]
    fn search_highlight_spans_whole_label_match_is_one_emphasized_span() {
        let base = Style::default();
        let hit = Style::default().fg(Color::Yellow);
        let spans = search_highlight_spans("abc", base, hit, (0, 3));
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].content.as_ref(), "abc");
        assert_eq!(spans[0].style, hit);
    }

    #[test]
    fn terminal_row_lines_render_two_content_lines_and_a_spacer() {
        let theme = crate::theme::Theme::default_dark();
        let width = 40;
        let line_text = |line: &Line| -> String {
            line.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        };
        let word_span = |line: &Line, word: &str| {
            line.spans
                .iter()
                .find(|s| s.content.as_ref() == word)
                .unwrap_or_else(|| panic!("expected a `{word}` span"))
                .style
                .fg
        };

        // Idle terminal: a steady dot, a plain "Terminal" (not the shell label),
        // and a muted "Idle" word. `terminal_row_lines` now returns the two
        // content lines; the trailing spacer is added by `framed_row_item`.
        let (idle0, idle1) = terminal_row_lines(
            &theme,
            false,
            false,
            '⠋',
            None,
            "my-branch",
            false,
            width,
            0,
            None,
        );
        assert!(line_text(&idle0).contains(crate::theme::DOT_GLYPH));
        assert!(line_text(&idle0).contains("Terminal"));
        assert!(!line_text(&idle0).contains("zsh"));
        // Line one carries no live-state color: the glyph is the neutral identity
        // color, never the typing/working color (that stays on the state word).
        assert_eq!(idle0.spans[0].style.fg, Some(theme.session_active));
        assert!(line_text(&idle1).contains("my-branch"));
        assert_eq!(word_span(&idle1, "Idle"), Some(theme.provider_label_fg));
        // An OWNED terminal keeps the muted return arrow: there the marker
        // means "owned by", and the standalone star must never leak onto it.
        assert_eq!(idle1.spans[0].content.as_ref(), "  ↳ ");
        assert_eq!(idle1.spans[0].style.fg, Some(theme.provider_label_fg));
        assert_eq!(
            word_span(&idle1, "my-branch"),
            Some(theme.provider_label_fg)
        );

        // Busy terminal: the foreground command replaces the label, the spinner
        // glyph shows in the NEUTRAL color, the label shimmers (split per char),
        // and only the word is "Running" in the busy color.
        let (working0, working1) = terminal_row_lines(
            &theme,
            false,
            true,
            '⠙',
            Some("cargo test"),
            "proj",
            false,
            width,
            0,
            None,
        );
        assert!(line_text(&working0).contains("cargo test"));
        assert!(!line_text(&working0).contains("zsh"));
        assert!(line_text(&working0).contains('⠙'));
        assert_eq!(working0.spans[0].style.fg, Some(theme.session_active));
        // The shimmer splits the label into per-character spans (glyph + chars).
        assert!(
            working0.spans.len() > 3,
            "expected a shimmered (per-char) label"
        );
        assert_eq!(word_span(&working1, "Running"), Some(theme.session_working));

        // Typing wins over working: the typing glyph shows, but line one stays
        // neutral; only the "Typing" word carries the session_typing color.
        let (typing0, typing1) = terminal_row_lines(
            &theme,
            true,
            true,
            '⠹',
            Some("vim"),
            "proj",
            false,
            width,
            0,
            None,
        );
        assert!(line_text(&typing0).contains(crate::theme::TYPING_GLYPH));
        assert!(line_text(&typing0).contains("vim"));
        assert_eq!(typing0.spans[0].style.fg, Some(theme.session_active));
        assert_eq!(word_span(&typing1, "Typing"), Some(theme.session_typing));
    }

    /// A STANDALONE terminal's second line wears the same standalone star and
    /// identity tone a standalone agent's folder line wears: one indicator,
    /// learned once, meaning "this one lives in your folder". The directory
    /// label wears the tone with it; the state word is untouched.
    #[test]
    fn a_standalone_terminal_row_wears_the_standalone_star_in_the_identity_tone() {
        let theme = crate::theme::Theme::default_dark();
        let (_, line2) = terminal_row_lines(
            &theme, false, false, '⠋', None, "~/notes", true, 40, 0, None,
        );
        let marker = &line2.spans[0];
        assert_eq!(
            marker.content.as_ref(),
            format!("  {} ", crate::theme::STANDALONE_GLYPH),
            "the standalone terminal's marker must be the standalone star"
        );
        assert_eq!(
            marker.style.fg,
            Some(theme.standalone_location_fg),
            "the star must wear the standalone identity tone"
        );
        let dir_fg = line2
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "~/notes")
            .expect("the directory label span")
            .style
            .fg;
        assert_eq!(
            dir_fg,
            Some(theme.standalone_location_fg),
            "the directory label must wear the identity tone too"
        );
        let word_fg = line2
            .spans
            .iter()
            .find(|s| s.content.as_ref() == "Idle")
            .expect("the state word span")
            .style
            .fg;
        assert_eq!(
            word_fg,
            Some(theme.provider_label_fg),
            "the state word keeps its own color; the tone is identity, not state"
        );
    }

    #[test]
    fn framed_row_item_wraps_two_lines_with_a_trailing_spacer() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        use ratatui::widgets::List;

        // The shared row component always produces a three-line item: the two
        // content lines verbatim plus a blank spacer, so agent rows and terminal
        // rows keep the identical shape the framed selection geometry assumes.
        // ListItem exposes no lines accessor, so render it and read the buffer.
        let item = framed_row_item(Line::from("primary"), Line::from("meta"));
        let backend = TestBackend::new(10, 3);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                ratatui::widgets::Widget::render(
                    List::new(vec![item]),
                    frame.area(),
                    frame.buffer_mut(),
                );
            })
            .expect("render frame");
        let buf = terminal.backend().buffer();
        let row_text =
            |y: u16| -> String { (0..10).map(|x| buf[(x, y)].symbol()).collect::<String>() };
        assert!(
            row_text(0).starts_with("primary"),
            "line one is the primary text"
        );
        assert!(row_text(1).starts_with("meta"), "line two is the meta text");
        assert!(
            row_text(2).trim().is_empty(),
            "third line is a blank spacer"
        );
    }

    /// Spawn two idle companion terminals owned by the first session so the left
    /// pane renders a two-row Terminals list. Returns the rendered terminal and
    /// the app so callers can inspect the post-render mouse map and buffer.
    /// An agent watched from a browser says so on line two, and stops saying it
    /// when the browser leaves. Nothing renders while nobody is watching, which is
    /// also every row's state whenever nothing is serving.
    #[test]
    fn an_agent_row_names_how_many_browsers_are_watching_it() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let session_id = app.engine.sessions[0].id.clone();
        let client = PtyClient::spawn(
            "/bin/sh",
            &["-c".to_string(), "sleep 30".to_string()],
            std::path::Path::new("."),
            24,
            80,
            100,
        )
        .expect("spawn pty");
        app.engine.providers.insert(session_id.clone(), client);
        app.focus = FocusPane::Left;
        app.rebuild_left_items();

        // The SIDEBAR ROW's own line two, found by the project marker it carries.
        // A whole-frame search would be answered by the center pane's caption,
        // which names the same fact somewhere else entirely; that is exactly how
        // the first version of this test passed with the row's segment deleted.
        let row_line = |app: &mut App| -> String {
            let mut terminal = Terminal::new(TestBackend::new(160, 40)).expect("terminal");
            terminal
                .draw(|frame| app.render(frame))
                .expect("render frame");
            let buf = terminal.backend().buffer();
            (0..buf.area.height)
                .map(|y| {
                    (0..buf.area.width)
                        .map(|x| buf[(x, y)].symbol().to_string())
                        .collect::<String>()
                })
                .find(|line| line.contains('※'))
                .expect("the agent row's line two is on screen")
        };

        assert!(
            !row_line(&mut app).contains("remote"),
            "an unwatched agent must render exactly what it always did"
        );

        let provider = app
            .engine
            .providers
            .get(&session_id)
            .expect("the seeded provider");
        let (one, _rx1) = provider.subscribe();
        let (two, _rx2) = provider.subscribe();
        assert_eq!(
            app.remote_viewer_count(&session_id, &app.session_tab_ids(&session_id)),
            2
        );
        let rendered = row_line(&mut app);
        assert!(
            rendered.contains("· 2 remote"),
            "line two must name the watchers: {rendered}"
        );

        drop(two);
        assert!(row_line(&mut app).contains("· 1 remote"), "singular at one");
        drop(one);
        assert!(
            !row_line(&mut app).contains("remote"),
            "the segment goes away with the last watcher"
        );
    }

    /// The center pane's caption names the watchers of the terminal it is showing,
    /// which is the FOCUSED tab's own count rather than the agent's total.
    #[test]
    fn the_center_pane_caption_names_the_watchers_of_the_tab_it_shows() {
        let mut app = test_app(default_bindings());
        let session_id = app.engine.sessions[0].id.clone();
        app.rebuild_left_items();
        app.selected_left = app
            .left_items_cache
            .iter()
            .position(|item| matches!(item, LeftItem::Session(_)))
            .expect("the test app has an agent row");
        assert!(
            !app.center_pane_agent_title().contains("remote"),
            "an unwatched agent's caption says nothing about watchers"
        );

        let client = PtyClient::spawn(
            "/bin/sh",
            &["-c".to_string(), "sleep 30".to_string()],
            std::path::Path::new("."),
            24,
            80,
            100,
        )
        .expect("spawn pty");
        let (_guard, _rx) = client.subscribe();
        app.engine.providers.insert(session_id.clone(), client);

        let title = app.center_pane_agent_title();
        assert!(
            title.contains("· 1 remote"),
            "the caption must name the one browser watching this tab: {title}"
        );
    }

    /// A browser watching one of the agent's COMPANION TERMINALS counts too.
    ///
    /// The agent's row is the only place this is shown at all (terminal rows carry
    /// no count of their own), so leaving those watchers out would put "nobody is
    /// watching" on a row while a second device demonstrably had one of its
    /// terminals open. Project and standalone terminals belong to nobody's agent
    /// and must not leak into anyone's row.
    #[test]
    fn a_browser_watching_an_agents_terminal_counts_toward_that_agent() {
        let mut app = app_with_two_terminals();
        let session_id = app.engine.sessions[0].id.clone();
        assert_eq!(
            app.remote_viewer_count(&session_id, &app.session_tab_ids(&session_id)),
            0,
            "nobody yet"
        );

        let watched = app
            .engine
            .companion_terminals
            .get("term-1")
            .expect("the seeded terminal");
        let (guard, _rx) = watched.client.subscribe();
        assert_eq!(
            app.remote_viewer_count(&session_id, &app.session_tab_ids(&session_id)),
            1,
            "a watcher on the agent's own terminal is a watcher of the agent"
        );

        // A standalone terminal's watchers belong to no agent at all.
        let standalone = PtyClient::spawn(
            "/bin/sh",
            &["-c".to_string(), "sleep 30".to_string()],
            std::path::Path::new("."),
            24,
            80,
            100,
        )
        .expect("spawn pty");
        // Subscribed before the client moves into the map; the guard holds the
        // shared subscriber list, not the client.
        let (_lone, _lone_rx) = standalone.subscribe();
        app.engine.companion_terminals.insert(
            "standalone-1".to_string(),
            CompanionTerminal {
                owner: dux_core::model::TerminalOwner::Standalone,
                label: "standalone".to_string(),
                foreground_cmd: None,
                client: standalone,
                sort_order: 9,
                created_at: chrono::Utc::now(),
            },
        );
        assert_eq!(
            app.remote_viewer_count(&session_id, &app.session_tab_ids(&session_id)),
            1,
            "a standalone terminal's watcher must not turn up on an agent's row"
        );

        drop(guard);
        assert_eq!(
            app.remote_viewer_count(&session_id, &app.session_tab_ids(&session_id)),
            0
        );
    }

    /// The count is per agent and it is the SUM over the agent's tabs, the same way
    /// the row's liveness ORs over them: the row is about the agent.
    #[test]
    fn the_remote_count_sums_over_an_agents_tabs() {
        let mut app = test_app(default_bindings());
        let session_id = app.engine.sessions[0].id.clone();
        seed_render_tab(&mut app, &session_id, "tab-2", "claude", 1);
        let mut guards = Vec::new();
        for tab_id in [session_id.clone(), "tab-2".to_string()] {
            let client = PtyClient::spawn(
                "/bin/sh",
                &["-c".to_string(), "sleep 30".to_string()],
                std::path::Path::new("."),
                24,
                80,
                100,
            )
            .expect("spawn pty");
            let (guard, _rx) = client.subscribe();
            guards.push((guard, _rx));
            app.engine.providers.insert(tab_id, client);
        }

        assert_eq!(
            app.remote_viewer_count(&session_id, &app.session_tab_ids(&session_id)),
            2,
            "one watcher on each of the agent's two tabs is two watchers of the agent"
        );
    }

    fn app_with_two_terminals() -> App {
        let mut app = test_app(default_bindings());
        let session_id = app.engine.sessions[0].id.clone();
        for (i, term_id) in ["term-1", "term-2"].iter().enumerate() {
            let client = PtyClient::spawn(
                "/bin/sh",
                &["-c".to_string(), "sleep 30".to_string()],
                std::path::Path::new("."),
                24,
                80,
                100,
            )
            .expect("spawn pty");
            app.engine.companion_terminals.insert(
                (*term_id).to_string(),
                CompanionTerminal {
                    owner: dux_core::model::TerminalOwner::Session(session_id.clone()),
                    label: format!("shell {i}"),
                    foreground_cmd: None,
                    client,
                    sort_order: i as u64,
                    created_at: chrono::Utc::now(),
                },
            );
        }
        app
    }

    /// The journey: a standalone terminal appears in the sidebar, and because it
    /// has no owner to name, its second line names the DIRECTORY it opened in,
    /// with the home directory collapsed to `~`. Rendered rather than reasoned
    /// about: this reads the row out of the frame buffer.
    #[test]
    fn a_standalone_terminal_row_names_its_directory_with_a_tilde() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        // A real directory under `$HOME`, so the `~` collapse has something to
        // collapse; skipped where no home resolves, which is not this feature's
        // problem to assert here (`home_path` owns that case).
        let Some(home) = home::home_dir() else {
            return;
        };
        let client = PtyClient::spawn(
            "/bin/sh",
            &["-c".to_string(), "sleep 30".to_string()],
            &home,
            24,
            80,
            100,
        )
        .expect("spawn pty");
        app.engine.companion_terminals.insert(
            "term-1".to_string(),
            CompanionTerminal {
                owner: dux_core::model::TerminalOwner::Standalone,
                label: "Terminal 1".to_string(),
                foreground_cmd: None,
                client,
                sort_order: 1,
                created_at: chrono::Utc::now(),
            },
        );
        app.focus = FocusPane::Left;
        app.left_section = LeftSection::Terminals;
        app.terminal_pane_height_pct = 50;

        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");

        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(
            rendered.contains("✷ ~ "),
            "the standalone row's second line must name its directory as `~` behind the standalone star; got:\n{rendered}"
        );
        assert!(
            !rendered.contains('↳'),
            "a standalone terminal has no owner, so its row must not wear the owned-by arrow; got:\n{rendered}"
        );
    }

    /// The journey: two terminals are in the sidebar, one owned by an agent and
    /// one owned by nothing. The user opens the sidebar filter and types `~`.
    /// Only the standalone terminal names a directory, so it is the only
    /// terminal left: the agent's terminal leaves the rendered list, the pane
    /// count, and the click map with it.
    ///
    /// Rendered rather than reasoned about, deliberately. The test this replaced
    /// called the matcher helper directly, so it never turned the filter on and
    /// never looked at what was on screen, and it passed while the terminal list
    /// was not filtered at all.
    #[test]
    fn the_sidebar_filter_hides_terminals_that_do_not_match_the_query() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        // The `~` collapse needs a home to collapse; the no-home case belongs to
        // `home_path`, not to the filter (same skip as the row-rendering test).
        let Some(home) = home::home_dir() else {
            return;
        };
        let mut app = test_app(default_bindings());
        let session_id = app.engine.sessions[0].id.clone();
        // The agent's terminal, whose row names `agent-branch@demo`, and the
        // standalone one, whose row names `~`. Neither label nor foreground can
        // match `~`, so the owner element is the only thing the query can hit.
        for (term_id, owner, dir) in [
            (
                "term-1",
                dux_core::model::TerminalOwner::Session(session_id.clone()),
                std::path::Path::new("."),
            ),
            (
                "term-2",
                dux_core::model::TerminalOwner::Standalone,
                home.as_path(),
            ),
        ] {
            let client = PtyClient::spawn(
                "/bin/sh",
                &["-c".to_string(), "sleep 30".to_string()],
                dir,
                24,
                80,
                100,
            )
            .expect("spawn pty");
            app.engine.companion_terminals.insert(
                term_id.to_string(),
                CompanionTerminal {
                    owner,
                    label: format!("Terminal {}", &term_id[5..]),
                    foreground_cmd: None,
                    client,
                    sort_order: 1,
                    created_at: chrono::Utc::now(),
                },
            );
        }
        app.terminal_pane_height_pct = 50;
        app.focus = FocusPane::Left;
        app.left_section = LeftSection::Projects;
        app.rebuild_left_items();

        // `/` opens the filter; `~` is typed into it, exactly as a user would.
        app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE))
            .expect("open the filter");
        app.handle_key(KeyEvent::new(KeyCode::Char('~'), KeyModifiers::NONE))
            .expect("type the query");
        assert_eq!(
            app.agent_filter.as_ref().map(|i| i.text.as_str()),
            Some("~")
        );

        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let rendered: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();

        assert!(
            rendered.contains("✷ ~ "),
            "the standalone terminal matches `~` and must still be on screen; got:\n{rendered}"
        );
        // The prefix, not the whole `agent-branch@demo`: the left pane is narrow
        // enough that the owner element ellipsizes, so the full string is absent
        // even when the row is right there on screen.
        assert!(
            !rendered.contains("↳ agent-br"),
            "the agent's terminal matches nothing and must be gone from the list; got:\n{rendered}"
        );
        assert!(
            rendered.contains("Terminals (1/2)"),
            "the pane count must report the filtered list over the total; got:\n{rendered}"
        );

        // The visible list itself, which is what selection and the click map are
        // indexed against.
        assert_eq!(
            app.terminal_items()
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>(),
            vec!["term-2"],
        );
        // The click map, pinned by LENGTH and by CONTENTS. `.iter().all(…)` is
        // what this used to say, and an empty map satisfies it, so the check
        // passed whether or not the one surviving terminal was clickable at all.
        // A terminal row is exactly three lines tall (two content lines and the
        // spacer, see `framed_row_item`), and there is exactly one row left, so
        // the map is three entries and every one of them names item 0. That
        // fails when the map is empty AND when it names a different row.
        assert_eq!(
            app.mouse_layout.terminal_row_to_item,
            vec![0, 0, 0],
            "the one visible row is three lines tall and every one of them must \
             click through to it; got {:?}",
            app.mouse_layout.terminal_row_to_item
        );
    }

    #[test]
    fn terminal_mouse_map_accounts_for_top_margin_and_gutter() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = app_with_two_terminals();
        app.focus = FocusPane::Left;
        app.left_section = LeftSection::Terminals;
        // Give the terminals pane plenty of height so the top margin is reserved.
        app.terminal_pane_height_pct = 50;

        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");

        // The click surface is recorded (the content area after the margin), and
        // it is offset one row below the block's inner top (the reserved margin).
        let term_content = app.mouse_layout.terminal_list;
        assert!(
            term_content.height >= 6 && term_content.width > 0,
            "terminal content surface should be recorded with room for two rows"
        );
        // Each terminal row is three lines tall, so the first three content rows
        // map to item 0 and the next three to item 1, even with the top margin
        // and the side gutters in play.
        let map = &app.mouse_layout.terminal_row_to_item;
        assert!(
            map.len() >= 6,
            "map should cover both three-line rows, got {}",
            map.len()
        );
        assert_eq!(&map[0..6], &[0, 0, 0, 1, 1, 1], "3-tall rows in order");
    }

    #[test]
    fn selected_terminal_gets_tinted_frame() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = app_with_two_terminals();
        app.focus = FocusPane::Left;
        app.left_section = LeftSection::Terminals;
        app.selected_terminal_index = 0;
        app.terminal_pane_height_pct = 50;

        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");

        let buf = terminal.backend().buffer();
        // The content surface origin; the first terminal's two content rows sit at
        // y0 and y0+1, its trailing spacer at y0+2, and the reserved top-margin
        // row (its `▄` top edge) at y0-1.
        let content = app.mouse_layout.terminal_list;
        let gx = content.x;
        let x1 = content.x + content.width;
        let y0 = content.y;
        let tint = app.theme.selection_bar_tint();

        // The tint fills the two content rows edge to edge, both gutters included,
        // exactly like an agent row (background only, no full-cell flood).
        for x in [gx, gx + 1, x1 - 2, x1 - 1] {
            assert_eq!(buf[(x, y0)].bg, tint, "name row cell {x} is tinted");
            assert_eq!(buf[(x, y0 + 1)].bg, tint, "meta row cell {x} is tinted");
        }
        // Framed by half-cell edges in the tint: a `▄` top edge on the reserved
        // margin row above, a `▀` bottom edge on the trailing spacer below.
        assert_eq!(buf[(gx + 1, y0 - 1)].symbol(), "▄", "top frame edge");
        assert_eq!(buf[(gx + 1, y0 + 2)].symbol(), "▀", "bottom frame edge");
        assert_eq!(buf[(gx + 1, y0 - 1)].fg, tint, "top edge is the tint");
        assert_eq!(buf[(gx + 1, y0 + 2)].fg, tint, "bottom edge is the tint");
    }

    #[test]
    fn agent_state_word_typing_sits_below_attention_and_above_working() {
        use crate::model::SessionStatus::{Active, Detached, Exited};
        // Typing is surfaced for an Active, typing session.
        assert_eq!(agent_state_word(Active, false, true, false), "Typing");
        // Attention still wins over typing.
        assert_eq!(agent_state_word(Active, false, true, true), "Needs you");
        // Typing wins over working when both are set.
        assert_eq!(agent_state_word(Active, true, true, false), "Typing");
        // Typing never applies to a non-Active session.
        assert_eq!(agent_state_word(Detached, false, true, false), "Detached");
        assert_eq!(agent_state_word(Exited, false, true, false), "Exited");
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
            session
                .workspace
                .as_managed_mut()
                .expect("managed test session")
                .branch_name = project_branch.clone();
            session
                .workspace
                .as_managed_mut()
                .expect("managed test session")
                .initial_branch = "server-mode".to_string();
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

    /// The header's own row, so an assertion cannot be answered by a chip that
    /// wrapped somewhere else or by the same word appearing in a pane.
    fn header_row(app: &mut App, width: u16) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(width, 40)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let buf = terminal.backend().buffer();
        (0..buf.area.width)
            .map(|x| buf[(x, 0)].symbol().to_string())
            .collect()
    }

    /// The header carries the serving chip while a background server is up, in the
    /// live tone, and nothing at all while none is.
    ///
    /// Rendered rather than asserted on the pure label, because the chip lives
    /// outside the header's two selection arms on purpose and the thing worth
    /// pinning is that it reaches the frame.
    #[test]
    fn the_header_says_it_is_serving_and_how_many_browsers_are_on_it() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        use crate::app::background_server::tests::FakeCompanion;

        let mut app = test_app(default_bindings());
        assert!(
            !header_row(&mut app, 160).contains("serving"),
            "a TUI that is not serving must not claim a listener"
        );

        let (companion, recorded) = FakeCompanion::serving();
        app.companion = Some(companion);
        recorded.lock().expect("not poisoned").connections = 2;
        assert!(
            header_row(&mut app, 160).contains("serving :8080 · 2 connected"),
            "the header must name the port and the connection count: {}",
            header_row(&mut app, 160)
        );

        // And in the live tone, like the terminal-count chip beside it. Located
        // within the header row rather than the whole frame, so another "serving"
        // somewhere on screen cannot retarget the probe.
        let mut terminal = Terminal::new(TestBackend::new(160, 40)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let buf = terminal.backend().buffer();
        let x = (0..buf.area.width)
            .find(|&x| {
                "serving".char_indices().all(|(i, c)| {
                    let x = x + i as u16;
                    x < buf.area.width && buf[(x, 0)].symbol() == c.to_string()
                })
            })
            .expect("the serving chip is on the header row");
        assert_eq!(
            buf[(x, 0)].fg,
            app.theme.session_active,
            "the serving chip wears the same live tone as the terminal-count chip"
        );
    }

    /// A listener is a fact about the PROCESS, so the chip has to survive the two
    /// things that would hide it: nothing being selected, and a narrow terminal.
    ///
    /// The header does not ellipsize (it is a plain paragraph, so a narrow width
    /// clips the tail), which is exactly why the chip leads rather than trails. A
    /// regression that moved it back inside a selection arm, or to the end of the
    /// line, is caught here and nowhere else.
    #[test]
    fn the_serving_chip_survives_no_selection_and_a_narrow_terminal() {
        use crate::app::background_server::tests::FakeCompanion;

        let mut app = test_app(default_bindings());
        let (companion, _recorded) = FakeCompanion::serving();
        app.companion = Some(companion);

        // No project and no agent: both of the header's selection arms are skipped
        // entirely, and the chip must still be there.
        app.engine.sessions.clear();
        app.engine.projects.clear();
        app.rebuild_left_items();
        app.selected_left = 0;
        assert!(
            header_row(&mut app, 160).contains("serving :8080"),
            "with nothing selected the header still has to say a listener is up: {}",
            header_row(&mut app, 160)
        );

        // And on a narrow terminal, where the crumbs that CAN be recovered from the
        // panes are the ones allowed to fall off the end.
        let app_narrow = &mut test_app(default_bindings());
        let (companion, _recorded) = FakeCompanion::serving();
        app_narrow.companion = Some(companion);
        let narrow = header_row(app_narrow, 40);
        assert!(
            narrow.contains("serving :8080"),
            "the chip must not be the first crumb a narrow header clips: {narrow:?}"
        );
    }

    /// The terminal-count chip still renders after being extracted out of the two
    /// verbatim copies it used to live in. Nothing else in the crate pinned it to a
    /// frame, so the extraction rested on inspection alone.
    #[test]
    fn the_header_still_counts_running_terminals() {
        let mut app = app_with_two_terminals();
        app.rebuild_left_items();
        let header = header_row(&mut app, 160);
        assert!(
            header.contains("● 2 terminals"),
            "the extracted terminal-count chip must still reach the header: {header:?}"
        );
    }

    /// The maximized (fullscreen) agent pane must NOT render the tab strip:
    /// tabs cannot be switched there, so the boxes would be dead chrome
    /// eating three rows. Only the windowed center pane shows the strip.
    /// With the strip gone, the only rounded box in a bare fullscreen render
    /// is the agent pane itself — exactly one top-left corner glyph.
    #[test]
    fn fullscreen_agent_renders_no_tab_strip() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let session_id = app.engine.sessions[0].id.clone();
        seed_render_tab(&mut app, &session_id, "tab-2", "claude", 1);
        app.set_focused_tab(&session_id, "tab-2");
        app.fullscreen_overlay = FullscreenOverlay::Agent;

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                app.render_fullscreen_agent(frame);
            })
            .expect("render frame");

        let buf = terminal.backend().buffer();
        let corner_count = (0..24)
            .flat_map(|y| (0..80).map(move |x| (x, y)))
            .filter(|&(x, y)| buf[(x, y)].symbol() == "╭")
            .count();
        assert_eq!(
            corner_count, 1,
            "fullscreen must draw only the agent pane's own box — no tab boxes"
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
    /// (session-slot-only) tab: the returned area is shrunk by the 3-row
    /// boxed strip and the single tab is recorded as a clickable region.
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
                    Rect::new(area.x, area.y + 3, area.width, area.height - 3),
                    "a single tab with the preference on must still reserve the boxed strip"
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

    /// Each tab renders as a miniature rounded pane: rounded corners on the
    /// border rows, the shared focused/unfocused border colors on the box,
    /// and the shared title styles on the label. The focused tab additionally
    /// carries the active-dot glyph so it stays unambiguous without color.
    #[test]
    fn tab_strip_renders_rounded_boxes_with_focused_and_unfocused_styles() {
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
        let top_row = Rect::new(area.x, area.y, area.width, 1);
        let label_row = Rect::new(area.x, area.y + 1, area.width, 1);
        let bottom_row = Rect::new(area.x, area.y + 2, area.width, 1);
        terminal
            .draw(|frame| {
                app.render_agent_tab_strip_if_needed(frame, area, true);
            })
            .expect("render frame");

        let top: String = strip_row_cells(&terminal, top_row)
            .into_iter()
            .map(|(sym, _, _)| sym)
            .collect();
        assert!(
            top.contains('╭') && top.contains('╮'),
            "each tab must open with rounded top corners, got: {top}"
        );
        let bottom: String = strip_row_cells(&terminal, bottom_row)
            .into_iter()
            .map(|(sym, _, _)| sym)
            .collect();
        assert!(
            bottom.contains('╰') && bottom.contains('╯'),
            "each tab must close with rounded bottom corners, got: {bottom}"
        );

        let label_syms: Vec<String> = strip_row_cells(&terminal, label_row)
            .into_iter()
            .map(|(sym, _, _)| sym)
            .collect();
        let labels: String = label_syms.concat();
        assert!(
            labels.contains('●'),
            "the focused tab must carry the active-dot glyph, got: {labels}"
        );
        assert!(
            labels.contains('│'),
            "the label row must carry the boxes' vertical borders, got: {labels}"
        );
        // The ordinal (the tab switch-key address) sits in its own SEGMENT
        // left of the label, behind a full-height divider: session-slot
        // "codex" is position 1, the extra "claude" tab position 2. The label
        // cell keeps its symmetric padding (the right margin mirrors the
        // left: space + dot-width + space).
        assert!(
            labels.contains("│ 2 │ ● claude   │"),
            "the focused pill must carry its ordinal segment and padded label, got: {labels}"
        );
        assert!(
            labels.contains("│ 1 │   codex   │"),
            "the unfocused pill must carry its ordinal segment and padded label, got: {labels}"
        );

        // The divider is JOINED to the frame: each pill's top border carries a
        // `┬` tee directly above the divider column and the bottom border a
        // `┴` directly below it (light box-drawing set, matching ╭╮╰╯).
        let top_syms: Vec<String> = strip_row_cells(&terminal, top_row)
            .into_iter()
            .map(|(sym, _, _)| sym)
            .collect();
        let bottom_syms: Vec<String> = strip_row_cells(&terminal, bottom_row)
            .into_iter()
            .map(|(sym, _, _)| sym)
            .collect();
        let tee_columns: Vec<usize> = top_syms
            .iter()
            .enumerate()
            .filter(|(_, sym)| *sym == "┬")
            .map(|(x, _)| x)
            .collect();
        assert_eq!(
            tee_columns.len(),
            2,
            "each of the two pills must carry exactly one top tee, got: {top}",
            top = top_syms.concat()
        );
        for x in tee_columns {
            assert_eq!(
                label_syms[x], "│",
                "the label row must carry the divider under each top tee (col {x})"
            );
            assert_eq!(
                bottom_syms[x], "┴",
                "the bottom border must carry a `┴` tee under each divider (col {x})"
            );
        }

        // The focused box uses the shared focused border/title styles (the
        // mini-pane idiom of `themed_block`); the unfocused box the normal
        // ones. "●" only exists in the focused tab; "o" only in the unfocused
        // "codex" label, so each unambiguously identifies its box.
        let label_cells = strip_row_cells(&terminal, label_row);
        let dot_cell = label_cells
            .iter()
            .find(|(sym, _, _)| sym == "●")
            .expect("dot glyph must be rendered");
        assert_eq!(
            dot_cell.1, app.theme.title_focused,
            "the focused tab's label must use the shared focused title color"
        );
        assert!(
            label_cells
                .iter()
                .any(|(sym, fg, _)| sym == "o" && *fg == app.theme.title_normal),
            "unfocused tab labels must use the legible title_normal color"
        );
        let top_cells = strip_row_cells(&terminal, top_row);
        assert!(
            top_cells
                .iter()
                .any(|(sym, fg, _)| sym == "╭" && *fg == app.theme.border_focused),
            "the focused tab's box must use the focused border color"
        );
        assert!(
            top_cells
                .iter()
                .any(|(sym, fg, _)| sym == "╭" && *fg == app.theme.border_normal),
            "unfocused tab boxes must use the normal border color"
        );
        // The tees are part of the frame, so they take the pill's border style
        // (focused and unfocused alike); no new theme token.
        assert!(
            top_cells
                .iter()
                .any(|(sym, fg, _)| sym == "┬" && *fg == app.theme.border_focused),
            "the focused pill's tee must use the focused border color"
        );
        assert!(
            top_cells
                .iter()
                .any(|(sym, fg, _)| sym == "┬" && *fg == app.theme.border_normal),
            "the unfocused pill's tee must use the normal border color"
        );
    }

    /// Decision 7: every pill leads with its strip ordinal — the visible
    /// address the tab switch keys count against — and the ordinal is a
    /// POSITION, never a stable id: closing a tab renumbers every pill after
    /// it. Position 4 renders like any other (its Ctrl-4 default is absent
    /// because legacy terminals send the same byte as the macro bar's Ctrl-\,
    /// but the address itself stays visible and rebindable).
    #[test]
    fn tab_strip_ordinals_renumber_when_a_tab_closes() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let session_id = app.engine.sessions[0].id.clone();
        // Session-slot tab is "codex" (test_support); add two extras.
        seed_render_tab(&mut app, &session_id, "tab-2", "claude", 1);
        seed_render_tab(&mut app, &session_id, "tab-3", "opencode", 2);
        app.set_focused_tab(&session_id, &session_id);

        let area = Rect::new(0, 0, 80, 24);
        let label_row = Rect::new(area.x, area.y + 1, area.width, 1);
        let render_labels = |app: &mut App| -> String {
            let backend = TestBackend::new(80, 24);
            let mut terminal = Terminal::new(backend).expect("terminal");
            terminal
                .draw(|frame| {
                    app.render_agent_tab_strip_if_needed(frame, area, true);
                })
                .expect("render frame");
            strip_row_cells(&terminal, label_row)
                .into_iter()
                .map(|(sym, _, _)| sym)
                .collect()
        };

        let labels = render_labels(&mut app);
        for expected in ["│ 1 │ ● codex", "│ 2 │   claude", "│ 3 │   opencode"] {
            assert!(
                labels.contains(expected),
                "each pill must carry its strip ordinal in its own segment; wanted \
                 {expected:?} in: {labels}"
            );
        }

        // Close the middle tab: opencode moves from position 3 to position 2.
        app.engine.agent_tabs.remove("tab-2");
        let labels = render_labels(&mut app);
        assert!(
            labels.contains("│ 2 │   opencode"),
            "closing a tab must renumber later pills (positions, not ids), got: {labels}"
        );
        assert!(
            !labels.contains("│ 3 │"),
            "the old ordinal must not survive the close, got: {labels}"
        );
        assert!(
            !labels.contains("claude"),
            "the closed tab must be gone from the strip, got: {labels}"
        );
    }

    /// `tab_pill_ordinal_cell` is position-driven and numbers EVERY pill,
    /// including positions past 9 (no Ctrl-N default, still an address for
    /// Ctrl-Left/Right counting and rebinds): the cell is one space, the
    /// number, one space, so its width follows the number's own width.
    #[test]
    fn tab_pill_ordinal_cell_numbers_all_positions() {
        assert_eq!(tab_pill_ordinal_cell(1), " 1 ");
        assert_eq!(tab_pill_ordinal_cell(4), " 4 ");
        assert_eq!(tab_pill_ordinal_cell(10), " 10 ");
    }

    /// A two-digit ordinal widens its own segment and never leaks into the
    /// label cell: position 10 renders as `│ 10 │` with the divider intact.
    #[test]
    fn tab_strip_two_digit_ordinal_keeps_its_own_segment() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let session_id = app.engine.sessions[0].id.clone();
        for i in 0..9 {
            seed_render_tab(
                &mut app,
                &session_id,
                &format!("tab-{i}"),
                "codex",
                i as i64,
            );
        }
        app.set_focused_tab(&session_id, "tab-8");

        let width = 200u16;
        let backend = TestBackend::new(width, 24);
        let mut terminal = Terminal::new(backend).expect("terminal");
        let area = Rect::new(0, 0, width, 24);
        terminal
            .draw(|frame| {
                app.render_agent_tab_strip_if_needed(frame, area, true);
            })
            .expect("render frame");

        let labels: String = strip_row_cells(&terminal, Rect::new(0, 1, width, 1))
            .into_iter()
            .map(|(sym, _, _)| sym)
            .collect();
        assert!(
            labels.contains("│ 10 │"),
            "position 10 must keep its ordinal segment whole, got: {labels}"
        );
    }

    /// Width/truncation math: the strip must never draw past the pane width,
    /// even once the box borders and the active dot widen each tab beyond a
    /// bare label.
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
    }

    /// F1 regression: a custom provider label made of double-width CJK glyphs
    /// must be measured by real display columns (unicode-width), not
    /// `chars().count()`. A char-count-based width undercounts "克劳德" (3
    /// chars, 6 display columns) by half, so the label would overflow its
    /// recorded region and paint past its box.
    #[test]
    fn tab_strip_cjk_label_region_matches_rendered_width() {
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
        assert!(
            tab_region.x + tab_region.width <= area.x + area.width,
            "the CJK tab's recorded region must stay within the pane: {tab_region:?}"
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

    /// The start-index choice is pure, so the two passes the renderer makes over
    /// it (one to learn whether a leading indicator is needed, one with the
    /// column it costs) can be checked without a frame.
    #[test]
    fn tab_strip_start_index_scrolls_only_far_enough_to_show_the_focused_tab() {
        // Four 10-wide segments in a 25-wide strip: two fit at a time.
        let seg_w = [10u16, 10, 10, 10];
        assert_eq!(tab_strip_start_index(&seg_w, 25, 0), 0);
        assert_eq!(tab_strip_start_index(&seg_w, 25, 1), 0);
        assert_eq!(tab_strip_start_index(&seg_w, 25, 2), 1);
        assert_eq!(tab_strip_start_index(&seg_w, 25, 3), 2);
        // Everything fits: never scroll.
        assert_eq!(tab_strip_start_index(&seg_w, 100, 3), 0);
        // Narrowing the strip can only push the start later, which is what makes
        // the renderer's two-pass reservation stable.
        for focused in 0..seg_w.len() {
            assert!(
                tab_strip_start_index(&seg_w, 24, focused)
                    >= tab_strip_start_index(&seg_w, 25, focused)
            );
        }
        // Degenerate inputs must not panic or wrap around.
        assert_eq!(tab_strip_start_index(&[], 10, 0), 0);
        assert_eq!(tab_strip_start_index(&[40], 10, 0), 0);
    }

    /// Render `count` tabs into a `width`-wide pane with `focus_idx` focused and
    /// return the app plus the buffer.
    fn tab_strip_frame(
        count: usize,
        width: u16,
        focus_idx: usize,
    ) -> (App, Rect, ratatui::buffer::Buffer) {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let session_id = app.engine.sessions[0].id.clone();
        for i in 0..count {
            seed_render_tab(
                &mut app,
                &session_id,
                &format!("tab-{i}"),
                &format!("provider-{i}"),
                i as i64,
            );
        }
        let tab_ids = app.session_tab_ids(&session_id);
        let focused = tab_ids[focus_idx].clone();
        app.set_focused_tab(&session_id, &focused);

        let area = Rect::new(0, 0, width, 24);
        let mut terminal = Terminal::new(TestBackend::new(width, 24)).expect("terminal");
        terminal
            .draw(|frame| {
                app.render_agent_tab_strip_if_needed(frame, area, true);
            })
            .expect("render frame");
        let buf = terminal.backend().buffer().clone();
        (app, area, buf)
    }

    /// Tabs hidden to the LEFT must say so, mirroring the trailing `…` the strip
    /// already paints when tabs are hidden to the right. Without it those tabs
    /// are invisible with no hint that they exist.
    #[test]
    fn tab_strip_marks_tabs_hidden_to_the_left() {
        // Six tabs in a pane too narrow for them all, focused on the last: the
        // scroll-into-view logic has to advance the start index.
        let (app, area, buf) = tab_strip_frame(6, 34, 6);
        let mid_y = area.y + 1;
        assert_eq!(
            buf[(area.x, mid_y)].symbol(),
            "…",
            "the strip scrolled right, so its first column must mark the hidden tabs"
        );
        let first_box_x = app
            .agent_tab_regions
            .iter()
            .map(|(_, rect)| rect.x)
            .min()
            .expect("at least one tab rendered");
        assert!(
            first_box_x > area.x,
            "the leading indicator must have its own column, not sit under a tab box"
        );
        for (tab_id, rect) in &app.agent_tab_regions {
            assert!(
                rect.x + rect.width <= area.x + area.width,
                "reserving the leading column must not push {tab_id} past the pane: {rect:?}"
            );
        }
    }

    #[test]
    fn tab_strip_has_no_leading_marker_at_its_leftmost_position() {
        // Same pane, focused on the FIRST tab: nothing is hidden to the left, so
        // a leading indicator would be a lie (the trailing one still shows).
        let (app, area, buf) = tab_strip_frame(6, 34, 0);
        let mid_y = area.y + 1;
        assert_ne!(
            buf[(area.x, mid_y)].symbol(),
            "…",
            "nothing is hidden to the left at the leftmost position"
        );
        let first_box_x = app
            .agent_tab_regions
            .iter()
            .map(|(_, rect)| rect.x)
            .min()
            .expect("at least one tab rendered");
        assert_eq!(
            first_box_x, area.x,
            "with no indicator to make room for, the first box keeps column zero"
        );

        // And a pane wide enough for every tab has no indicator on either side.
        let (_, area, buf) = tab_strip_frame(6, 200, 6);
        let row: String = (area.x..area.x + area.width)
            .map(|x| buf[(x, area.y + 1)].symbol().to_string())
            .collect();
        assert!(
            !row.contains('…'),
            "a strip that fits needs no truncation marks: {row:?}"
        );
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

    /// Build an app with a live agent PTY that has parked its cursor at
    /// (row 4, col 9), for the caret-placement tests below.
    fn app_with_parked_agent_cursor() -> App {
        let mut app = test_app(default_bindings());
        let session_id = app.engine.sessions[0].id.clone();
        let args = vec![
            "-c".to_string(),
            "printf '\\033[5;9HX'; sleep 30".to_string(),
        ];
        let client = PtyClient::spawn("/bin/sh", &args, std::path::Path::new("."), 24, 80, 100)
            .expect("spawn pty");
        app.engine.providers.insert(session_id, client);
        app.session_surface = SessionSurface::Agent;
        app
    }

    /// Draw a frame and return (terminal, term_area).
    fn draw_caret_frame(
        app: &mut App,
    ) -> (
        ratatui::Terminal<ratatui::backend::TestBackend>,
        ratatui::layout::Rect,
    ) {
        let backend = ratatui::backend::TestBackend::new(100, 40);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
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
        (terminal, term_area)
    }

    /// The hardware caret follows KEYS, not fullscreen: the minimized center
    /// pane shows the caret whenever it is typeable, because keystrokes land
    /// in the agent's PTY there and IME composition anchors to the hardware
    /// cursor.
    #[test]
    fn minimized_typeable_agent_places_the_hardware_caret() {
        let mut app = app_with_parked_agent_cursor();
        app.center_mode = CenterMode::Agent;
        app.focus = FocusPane::Center;
        app.input_target = InputTarget::None;
        app.fullscreen_overlay = FullscreenOverlay::None;
        wait_for_agent_cursor(&mut app, 4, 9);
        assert!(app.center_typeable(), "test setup: pane must be typeable");

        let (mut terminal, term_area) = draw_caret_frame(&mut app);

        let expected = (term_area.x + 9, term_area.y + 4);
        terminal.backend_mut().assert_cursor_position(expected);
    }

    /// The windowed typeable pane's hint line says where typing goes and names
    /// the chords that stay dux's (fullscreen toggle, next pane), every label
    /// resolved through the bindings rather than hardcoded.
    #[test]
    fn typeable_pane_hint_says_typing_goes_to_the_agent() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = app_with_parked_agent_cursor();
        app.center_mode = CenterMode::Agent;
        app.focus = FocusPane::Center;
        app.input_target = InputTarget::None;
        app.fullscreen_overlay = FullscreenOverlay::None;
        assert!(app.center_typeable(), "test setup: pane must be typeable");

        let backend = TestBackend::new(180, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");

        let rows = buffer_rows(terminal.backend().buffer());
        let hint = rows
            .iter()
            .find(|row| row.contains("Typing goes to the agent."))
            .unwrap_or_else(|| {
                panic!(
                    "the typeable pane must say where typing goes; frame was:\n{}",
                    rows.join("\n")
                )
            });
        let fullscreen_key = app.bindings.label_for(Action::ToggleFullscreen);
        let next_pane_key = app.bindings.label_for(Action::FocusNext);
        assert!(
            hint.contains(&fullscreen_key) && hint.contains("fullscreen"),
            "the hint must name the fullscreen toggle {fullscreen_key:?}; row was {hint:?}"
        );
        assert!(
            hint.contains(&next_pane_key) && hint.contains("next pane"),
            "the hint must name the next-pane key {next_pane_key:?}; row was {hint:?}"
        );
        assert!(
            !hint.contains("next tab"),
            "a single-tab agent gets no tab hint (it would be noise); row was {hint:?}"
        );
    }

    /// With two or more tabs the typeable hint names the tab-switch chord
    /// (decision 6: the surviving chords are loud in HINTS, not only docs).
    #[test]
    fn typeable_pane_hint_names_the_tab_switch_chord_with_two_tabs() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = app_with_parked_agent_cursor();
        app.center_mode = CenterMode::Agent;
        app.focus = FocusPane::Center;
        app.input_target = InputTarget::None;
        app.fullscreen_overlay = FullscreenOverlay::None;
        let session_id = app.engine.sessions[0].id.clone();
        seed_render_tab(&mut app, &session_id, "tab-2", "claude", 1);
        assert!(app.center_typeable(), "test setup: pane must be typeable");

        let backend = TestBackend::new(180, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");

        let rows = buffer_rows(terminal.backend().buffer());
        let hint = rows
            .iter()
            .find(|row| row.contains("Typing goes to the agent."))
            .unwrap_or_else(|| {
                panic!(
                    "the typeable pane must say where typing goes; frame was:\n{}",
                    rows.join("\n")
                )
            });
        let next_tab_key = app.bindings.label_for(Action::NextTab);
        assert!(
            hint.contains(&next_tab_key) && hint.contains("next tab"),
            "with 2+ tabs the hint must name the tab-switch chord {next_tab_key:?}; row was {hint:?}"
        );
    }

    /// A live agent whose pane is NOT focused offers the activate key as
    /// "focus and type": the old "to interact" wording described the removed
    /// coupled fullscreen-on-Enter model.
    #[test]
    fn unfocused_live_pane_hint_offers_focus_and_type() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = app_with_parked_agent_cursor();
        app.center_mode = CenterMode::Agent;
        app.focus = FocusPane::Left;
        app.input_target = InputTarget::None;
        app.fullscreen_overlay = FullscreenOverlay::None;
        assert!(!app.center_typeable());

        let backend = TestBackend::new(180, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");

        let rows = buffer_rows(terminal.backend().buffer());
        assert!(
            rows.iter().any(|row| row.contains("focus and type")),
            "an unfocused live pane must offer focus-and-type; frame was:\n{}",
            rows.join("\n")
        );
        assert!(
            !rows.iter().any(|row| row.contains("to interact")),
            "the old 'to interact' wording must be gone"
        );
    }

    /// With no live process behind the pane, the hint names BOTH launch keys
    /// and says they launch (not "interact": there is nothing to type into).
    #[test]
    fn exited_agent_hint_names_the_launch_keys() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        app.center_mode = CenterMode::Agent;
        app.focus = FocusPane::Center;
        assert!(app.engine.providers.is_empty(), "test setup: no live PTY");

        let backend = TestBackend::new(180, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");

        let rows = buffer_rows(terminal.backend().buffer());
        let hint = rows
            .iter()
            .find(|row| row.contains("Agent CLI exited."))
            .unwrap_or_else(|| {
                panic!(
                    "the exited pane must say the CLI exited; frame was:\n{}",
                    rows.join("\n")
                )
            });
        assert!(
            hint.contains("to launch it again"),
            "the exited hint must say the keys LAUNCH; row was {hint:?}"
        );
    }

    /// A pane that does not RECEIVE keys must not show the caret: a blinking
    /// cursor over output nothing types into is a lie. Each state below ends
    /// typeability, so each must leave the hardware cursor at the origin.
    #[test]
    fn non_typeable_agent_views_leave_the_hardware_cursor_at_origin() {
        // Focus elsewhere: the minimized pane is visible but not focused.
        {
            let mut app = app_with_parked_agent_cursor();
            app.center_mode = CenterMode::Agent;
            app.focus = FocusPane::Left;
            app.input_target = InputTarget::None;
            app.fullscreen_overlay = FullscreenOverlay::None;
            wait_for_agent_cursor(&mut app, 4, 9);
            let (mut terminal, _) = draw_caret_frame(&mut app);
            terminal.backend_mut().assert_cursor_position((0u16, 0u16));
        }
        // A dormant-agent fullscreen overlay (input not routed to the PTY).
        {
            let mut app = app_with_parked_agent_cursor();
            app.fullscreen_overlay = FullscreenOverlay::Agent;
            wait_for_agent_cursor(&mut app, 4, 9);
            app.input_target = InputTarget::None;
            let (mut terminal, _) = draw_caret_frame(&mut app);
            assert!(
                app.snapshot_buf.cursor.is_some(),
                "test setup: the PTY should still expose a cursor to (not) place"
            );
            terminal.backend_mut().assert_cursor_position((0u16, 0u16));
        }
        // A prompt on top of a typeable pane swallows the keys, so no caret.
        {
            let mut app = app_with_parked_agent_cursor();
            app.center_mode = CenterMode::Agent;
            app.focus = FocusPane::Center;
            app.input_target = InputTarget::None;
            app.fullscreen_overlay = FullscreenOverlay::None;
            wait_for_agent_cursor(&mut app, 4, 9);
            app.prompt = PromptState::ConfirmQuit {
                agent_count: 1,
                terminal_count: 0,
                focus: ConfirmFocus::Cancel,
            };
            assert!(!app.center_typeable());
            let (mut terminal, _) = draw_caret_frame(&mut app);
            terminal.backend_mut().assert_cursor_position((0u16, 0u16));
        }
    }

    /// Scrolled back, the PTY cursor cell is off-screen and the snapshot
    /// exposes no cursor, so the caret vanishes (mirroring fullscreen).
    #[test]
    fn a_scrolled_back_typeable_pane_shows_no_caret() {
        let mut app = test_app(default_bindings());
        let session_id = app.engine.sessions[0].id.clone();
        // Enough lines to overflow the pane even after the first render
        // resizes the PTY to the pane's height, so real history remains.
        let args = vec![
            "-c".to_string(),
            "printf 'L%s\\n' $(seq 1 100); sleep 30".to_string(),
        ];
        let client = PtyClient::spawn("/bin/sh", &args, std::path::Path::new("."), 5, 40, 200)
            .expect("spawn pty");
        app.engine.providers.insert(session_id, client);
        app.session_surface = SessionSurface::Agent;
        app.center_mode = CenterMode::Agent;
        app.focus = FocusPane::Center;
        // First draw resizes the PTY to the pane; then wait for history at
        // the final size before scrolling back.
        let (_, _) = draw_caret_frame(&mut app);
        for _ in 0..200 {
            app.refresh_snapshot_buf();
            if app.snapshot_buf.scrollback_total > 0 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(
            app.snapshot_buf.scrollback_total > 0,
            "test setup: the child must have produced scrollback"
        );
        app.selected_terminal_surface_client()
            .expect("provider")
            .scroll(true, 3);
        app.refresh_snapshot_buf();
        assert!(app.center_typeable(), "typeability is not scroll state");

        let (mut terminal, _) = draw_caret_frame(&mut app);

        // Scrolled back, the cursor cell maps below the viewport (or not at
        // all), so the render gate must leave the hardware cursor alone.
        assert!(
            app.snapshot_buf
                .cursor
                .is_none_or(|c| c.row >= app.snapshot_buf.rows),
            "test premise: the PTY cursor cell must be out of the viewport"
        );
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
                sort_order: 1,
                created_at: chrono::Utc::now(),
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

    /// Switching to a different agent must resize THAT agent's PTY, even when
    /// the centre pane measures exactly what it measured for the previous one.
    /// The dedupe used to compare geometry alone against one workspace-wide slot,
    /// so the second agent kept whatever geometry it was spawned with for as long
    /// as it lived.
    #[test]
    fn switching_agents_resizes_the_newly_shown_pty_at_an_unchanged_pane_size() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let first_id = app.engine.sessions[0].id.clone();
        let mut second = app.engine.sessions[0].clone();
        second.id = "session-two".to_string();
        second
            .workspace
            .as_managed_mut()
            .expect("managed test session")
            .branch_name = "second".to_string();
        let second_id = second.id.clone();
        app.engine.sessions.push(second);
        app.rebuild_left_items();

        let args = vec!["-c".to_string(), "sleep 30".to_string()];
        for id in [&first_id, &second_id] {
            let client = PtyClient::spawn("/bin/sh", &args, std::path::Path::new("."), 5, 40, 100)
                .expect("spawn pty");
            app.engine.providers.insert(id.clone(), client);
        }

        let select = |app: &mut App, index: usize| {
            let at = app
                .left_items()
                .iter()
                .position(|item| matches!(item, LeftItem::Session(i) if *i == index))
                .expect("session row");
            app.selected_left = at;
        };
        let draw = |app: &mut App| {
            let backend = TestBackend::new(100, 40);
            let mut terminal = Terminal::new(backend).expect("terminal");
            terminal
                .draw(|frame| app.render(frame))
                .expect("render frame");
        };

        app.center_mode = CenterMode::Agent;
        app.session_surface = SessionSurface::Agent;
        select(&mut app, 0);
        draw(&mut app);
        let pane = app.last_pty_size;
        assert_ne!(
            pane,
            (5, 40),
            "test setup: the pane must differ from the spawn geometry"
        );
        assert_eq!(
            app.last_pty_resize_target.as_deref(),
            Some(first_id.as_str())
        );

        // Same pane, different agent. Comparing geometry alone says "nothing
        // changed" here, which is exactly the bug.
        select(&mut app, 1);
        draw(&mut app);
        assert_eq!(
            app.last_pty_size, pane,
            "test premise: the pane did not change between the two frames"
        );
        assert_eq!(
            app.last_pty_resize_target.as_deref(),
            Some(second_id.as_str())
        );
        let snapshot = app.engine.providers[&second_id].snapshot();
        assert_eq!(
            (snapshot.rows, snapshot.cols),
            pane,
            "the newly shown agent kept its spawn geometry"
        );

        // A redraw with neither the pane nor the target moving still dedupes.
        app.engine.providers[&second_id]
            .resize(9, 9)
            .expect("resize");
        draw(&mut app);
        let snapshot = app.engine.providers[&second_id].snapshot();
        assert_eq!(
            (snapshot.rows, snapshot.cols),
            (9, 9),
            "an unchanged pane on an unchanged target must send nothing"
        );
    }

    /// The cue has to be ON SCREEN, not merely constructible: the hint bar is
    /// the only surface that can say "your typing is going nowhere" while the
    /// pane itself looks perfectly alive. Renders a real frame over a real,
    /// scrolled-back PTY and reads the cue back out of the buffer, so a hint
    /// branch that never runs cannot pass.
    #[test]
    fn the_scroll_mode_cue_reaches_the_hint_bar() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let session_id = app.engine.sessions[0].id.clone();
        // Print more lines than the 6-row grid holds so there is real history
        // to scroll into, then stay alive.
        let args = vec![
            "-c".to_string(),
            "printf 'L%s\\n' 1 2 3 4 5 6 7 8 9 10 11 12; sleep 30".to_string(),
        ];
        let client = PtyClient::spawn("/bin/sh", &args, std::path::Path::new("."), 6, 80, 100)
            .expect("spawn pty");
        app.engine.providers.insert(session_id, client);

        app.input_target = InputTarget::Agent;
        app.session_surface = SessionSurface::Agent;
        app.fullscreen_overlay = FullscreenOverlay::Agent;
        enter_scroll_mode(&mut app, 3);

        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");

        let rows = buffer_rows(terminal.backend().buffer());
        let cue = rows
            .iter()
            .find(|row| row.contains("keys are not reaching the agent"))
            .unwrap_or_else(|| {
                panic!(
                    "scroll mode must say so on screen; frame was:\n{}",
                    rows.join("\n")
                )
            });
        let live_edge = app.bindings.labels_for(Action::ScrollToBottom);
        assert!(
            cue.contains(&live_edge),
            "the cue must name the live-edge key {live_edge:?}; row was {cue:?}"
        );
    }

    /// The scrollback badge paints its OWN themed background over the pane's
    /// top-right corner and its width tracks the offset's digit count, so it is
    /// not part of the pane surface these tests measure. Mirrors the render
    /// site's geometry rather than guessing it.
    fn scrollback_badge_rect(app: &App, term_area: Rect) -> Option<Rect> {
        let label = scrollback_indicator_label(app.snapshot_buf.scrollback_offset)?;
        let width = label.len() as u16;
        (term_area.height > 0 && width <= term_area.width)
            .then(|| Rect::new(term_area.x + term_area.width - width, term_area.y, width, 1))
    }

    /// Every background colour inside `area`, with a tally, so a failure can
    /// say WHAT the pane was painted rather than just that it was wrong.
    fn pane_bg_tally(
        buffer: &ratatui::buffer::Buffer,
        area: Rect,
        skip: Option<Rect>,
    ) -> std::collections::BTreeMap<String, usize> {
        let mut tally = std::collections::BTreeMap::new();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                if skip.is_some_and(|rect| rect.contains(ratatui::layout::Position::new(x, y))) {
                    continue;
                }
                *tally.entry(format!("{:?}", buffer[(x, y)].bg)).or_insert(0) += 1;
            }
        }
        tally
    }

    /// The agent pane's themed background must be the SAME whether or not the
    /// scrollback offset moved since the previous frame.
    ///
    /// This is a regression test for a real, measured defect. The pane used to
    /// `Clear.render(term_area, ..)` whenever the offset differed from the
    /// previous frame's. `Clear` is `Cell::reset()` per cell, which drops the
    /// background to `Color::Reset` (the host terminal's default), and it ran
    /// AFTER the frame-wide `app_bg` fill, so every cell the snapshot loop
    /// leaves alone lost its theme colour. That branch used to be unreachable
    /// while a user read history, because output parsing stopped on scroll; now
    /// output always flows and the terminal library holds the view still by
    /// incrementing the offset per arriving line, so the branch fired on
    /// essentially every frame and the pane background visibly flipped between
    /// themed and terminal-default while the agent talked.
    ///
    /// PTY cell colors now render verbatim in every mode, so a child cell with
    /// a default background legitimately carries `Color::Reset` here. What this
    /// test pins is STABILITY: the pane's background distribution must be
    /// byte-identical between a frame where the offset moved and one where it
    /// did not, which is exactly what the old clear broke (it fired only on
    /// offset-change frames and stripped the frame-wide fill off every cell
    /// the snapshot loop leaves alone, so the pane flickered while the agent
    /// talked).
    #[test]
    fn agent_pane_background_survives_a_scrollback_offset_change() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let session_id = app.engine.sessions[0].id.clone();
        // More lines than the 6-row grid holds, so there is real history to
        // scroll into, then stay alive.
        let args = vec![
            "-c".to_string(),
            "i=1; while [ $i -le 200 ]; do echo L$i; i=$((i+1)); done; sleep 30".to_string(),
        ];
        let client = PtyClient::spawn("/bin/sh", &args, std::path::Path::new("."), 6, 80, 2000)
            .expect("spawn pty");
        app.engine.providers.insert(session_id, client);

        app.input_target = InputTarget::None;
        app.session_surface = SessionSurface::Agent;
        app.fullscreen_overlay = FullscreenOverlay::None;

        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");

        // A first frame settles the PTY resize to the pane's real size. It has
        // to come before the scroll: the resize reflows the grid and returns
        // the view to the live edge, so a scroll staged earlier is undone.
        terminal
            .draw(|frame| app.render(frame))
            .expect("render sizing frame");
        enter_scroll_mode(&mut app, 3);
        // Then one frame to carry the 0 -> N offset change, so the next frame
        // is genuinely the "offset unchanged" case.
        terminal
            .draw(|frame| app.render(frame))
            .expect("render settling frame");

        let term_area = app
            .mouse_layout
            .agent_term
            .expect("agent terminal area should be recorded after render");
        assert!(
            app.snapshot_buf.scrollback_offset > 0,
            "test setup: the pane must be parked in scrollback"
        );

        // The reference the user sees while the agent is quiet.
        terminal
            .draw(|frame| app.render(frame))
            .expect("render quiet frame");
        let quiet_offset = app.snapshot_buf.scrollback_offset;
        let quiet_badge = scrollback_badge_rect(&app, term_area);
        let quiet = pane_bg_tally(terminal.backend().buffer(), term_area, quiet_badge);

        // The "offset moved" case, which is what an arriving line does to a
        // view the terminal library is holding still.
        app.selected_terminal_surface_client()
            .expect("provider")
            .scroll(true, 1);
        terminal
            .draw(|frame| app.render(frame))
            .expect("render moved frame");
        assert_ne!(
            app.snapshot_buf.scrollback_offset, quiet_offset,
            "test setup: the second frame must carry a CHANGED scrollback offset"
        );
        let moved_badge = scrollback_badge_rect(&app, term_area);
        let moved = pane_bg_tally(terminal.backend().buffer(), term_area, moved_badge);

        assert_eq!(
            quiet, moved,
            "the agent pane background must not depend on whether the scrollback \
             offset moved this frame"
        );
    }

    /// The other half of removing that clear: prove it was protecting nothing.
    ///
    /// Its comment claimed it stopped stale cells lingering in ratatui's diff
    /// buffer across a scroll, and a review suggested it was also scrubbing the
    /// spacer cells wide characters leave behind (the snapshot skips
    /// `WIDE_CHAR_SPACER`, so the painting loop never touches them). This walks
    /// the pane cell by cell after scrolling a grid that holds wide CJK glyphs
    /// and asserts the buffer says exactly what the current snapshot says:
    /// every snapshot cell present, every uncovered position blank. A leftover
    /// glyph from the previous frame, in a spacer or anywhere else, fails here.
    #[test]
    fn scrolling_the_agent_pane_leaves_no_stale_cells_without_a_clear() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let session_id = app.engine.sessions[0].id.clone();
        // Wide CJK lines first, then narrow ASCII ones, so scrolling replaces
        // double-width glyphs (and their spacers) with single-width text.
        let args = vec![
            "-c".to_string(),
            "i=1; while [ $i -le 120 ]; do echo 日本語日本語$i; i=$((i+1)); done; \
             i=1; while [ $i -le 120 ]; do echo n$i; i=$((i+1)); done; sleep 30"
                .to_string(),
        ];
        let client = PtyClient::spawn("/bin/sh", &args, std::path::Path::new("."), 6, 80, 2000)
            .expect("spawn pty");
        app.engine.providers.insert(session_id, client);

        app.input_target = InputTarget::None;
        app.session_surface = SessionSurface::Agent;
        app.fullscreen_overlay = FullscreenOverlay::None;

        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render sizing frame");
        // Deep enough to land in the wide-glyph half of the history.
        enter_scroll_mode(&mut app, 150);
        terminal
            .draw(|frame| app.render(frame))
            .expect("render wide frame");
        let term_area = app
            .mouse_layout
            .agent_term
            .expect("agent terminal area should be recorded after render");
        assert!(
            app.snapshot_buf
                .cells
                .iter()
                .any(|cell| cell.symbol.as_str() == "日"),
            "test setup: the scrolled-back view must contain wide glyphs"
        );

        // Scroll forward into the narrow half. Any cell the previous frame
        // painted and this one does not must be gone.
        app.selected_terminal_surface_client()
            .expect("provider")
            .scroll(false, 130);
        terminal
            .draw(|frame| app.render(frame))
            .expect("render narrow frame");

        // The scrollback badge overpaints the top-right corner after the cell
        // loop, so those cells legitimately disagree with the snapshot.
        let badge = scrollback_badge_rect(&app, term_area);
        let overpainted = |x: u16, y: u16| {
            badge.is_some_and(|rect| rect.contains(ratatui::layout::Position::new(x, y)))
        };

        let buffer = terminal.backend().buffer();
        let mut covered = std::collections::HashSet::new();
        for cell in &app.snapshot_buf.cells {
            if cell.row >= term_area.height || cell.col >= term_area.width {
                continue;
            }
            let pos = (term_area.x + cell.col, term_area.y + cell.row);
            covered.insert(pos);
            if overpainted(pos.0, pos.1) {
                continue;
            }
            assert_eq!(
                buffer[pos].symbol(),
                cell.symbol.as_str(),
                "pane cell at {pos:?} does not match the snapshot"
            );
        }
        for y in term_area.y..term_area.y + term_area.height {
            for x in term_area.x..term_area.x + term_area.width {
                if covered.contains(&(x, y)) || overpainted(x, y) {
                    continue;
                }
                assert_eq!(
                    buffer[(x, y)].symbol(),
                    " ",
                    "an uncovered pane cell at {:?} kept stale content",
                    (x, y)
                );
            }
        }
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

    /// The badge names a distance to the live edge, not a position in a
    /// document: no denominator, and the word "below" so a number that climbs
    /// while the user sits still reads as output arriving beneath them. See the
    /// function's own comment for why that reading matters.
    #[test]
    fn scrollback_indicator_names_the_distance_below() {
        assert_eq!(
            scrollback_indicator_label(41),
            Some(" 41 lines below ".to_string())
        );
    }

    #[test]
    fn scrollback_indicator_handles_a_single_line() {
        assert_eq!(
            scrollback_indicator_label(1),
            Some(" 1 line below ".to_string())
        );
    }

    #[test]
    fn scrollback_indicator_hides_at_live_bottom() {
        assert_eq!(scrollback_indicator_label(0), None);
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
        let line =
            render_single_line_cursor_input("", "macro", 2, Color::White, Color::Black, true);

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

    /// The macro editor's popup, at the size the renderer actually asks for.
    fn macro_edit_popup() -> Rect {
        Rect::new(0, 0, MACRO_EDIT_POPUP.0, MACRO_EDIT_POPUP.1)
    }

    #[test]
    fn macro_edit_text_inner_area_accounts_for_borders_and_chrome() {
        // The body no longer owns the whole modal: it shares it with the name
        // field above and the selector, spacer, buttons, and hint row below.
        assert_eq!(
            macro_edit_text_inner_area(macro_edit_popup()),
            Rect::new(2, 7, 62, 8)
        );
    }

    #[test]
    fn macro_text_input_layout_uses_drawable_inner_height() {
        let text = (0..20)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut input = TextInput::with_text(text).with_multiline(3);

        assert_eq!(input.visible_lines().len(), 3);

        sync_macro_text_input_layout(&mut input, macro_edit_popup());

        assert_eq!(input.visible_lines().len(), 8);
        assert_eq!(input.scroll_offset(), 12);
        assert_eq!(
            input.visible_lines().first().map(String::as_str),
            Some("line 12")
        );
        assert_eq!(
            input.visible_lines().last().map(String::as_str),
            Some("line 19")
        );
    }

    /// Open the macro editor on a single stored macro, with focus on `focus`.
    fn macro_editor_app(focus: super::MacroEditFocus) -> App {
        let mut app = test_app(default_bindings());
        app.engine.config.macros.entries.insert(
            "greet".to_string(),
            crate::config::MacroEntry {
                text: "hello".to_string(),
                surface: crate::config::MacroSurface::Agent,
            },
        );
        app.open_edit_macros();
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
            .expect("open the editor");
        match &mut app.prompt {
            PromptState::EditMacros {
                editing: Some(state),
                ..
            } => state.focus = focus,
            other => panic!("expected an open macro editor, got {other:?}"),
        }
        app
    }

    #[test]
    fn the_macro_body_shows_engaged_apart_from_focused_and_marks_an_overflow() {
        use super::super::components::{MARKER_GLYPHS, scroll_marker_rect};

        let mut app = macro_editor_app(super::MacroEditFocus::Text);
        let engage_key = app.bindings.label_for(Action::EngageCommitInput);

        app.input_target = InputTarget::None;
        let unengaged = rendered_rows(&mut app).join("\n");
        app.input_target = InputTarget::MacroText;
        let engaged = rendered_rows(&mut app).join("\n");

        assert!(
            !unengaged.contains("editing:"),
            "a body that takes no keystrokes must not claim to be editing"
        );
        assert!(
            engaged.contains("editing:"),
            "the engaged body must say so, or it looks like the focused one"
        );
        assert!(
            unengaged.contains(&engage_key) && unengaged.contains("edit text"),
            "an unengaged body must name the key that starts editing"
        );

        // A body longer than its pane carries the shared one-cell marker.
        let mut app = macro_editor_app(super::MacroEditFocus::Text);
        match &mut app.prompt {
            PromptState::EditMacros {
                editing: Some(state),
                ..
            } => {
                state.text_input = TextInput::with_text(
                    (0..40)
                        .map(|n| format!("line {n}"))
                        .collect::<Vec<_>>()
                        .join("\n"),
                )
                .with_multiline(8);
            }
            other => panic!("expected an open macro editor, got {other:?}"),
        }
        let rows = rendered_rows(&mut app);
        let inner = macro_text_rect(&app.overlay_layout.active);
        let area = Rect::new(inner.x - 1, inner.y - 1, inner.width + 2, inner.height + 2);
        let cell = scroll_marker_rect(area, inner);
        let glyph = rows[cell.y as usize]
            .chars()
            .nth(cell.x as usize)
            .expect("marker cell")
            .to_string();
        assert!(
            MARKER_GLYPHS.contains(&glyph.as_str()),
            "an overflowing macro body must carry the shared scroll marker, got {glyph:?}"
        );
    }

    /// The style of the top-left corner cell of a rect, from a real frame.
    fn corner_style(app: &mut App, pick: impl Fn(&OverlayMouseLayout) -> Rect) -> Style {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let rect = pick(&app.overlay_layout.active);
        // The published rect is the field's INNER area; step out onto its border.
        let cell =
            &terminal.backend().buffer()[(rect.x.saturating_sub(1), rect.y.saturating_sub(1))];
        Style::default()
            .fg(cell.fg)
            .bg(cell.bg)
            .add_modifier(cell.modifier)
    }

    fn macro_name_rect(layout: &OverlayMouseLayout) -> Rect {
        match layout {
            OverlayMouseLayout::EditMacros { name_input, .. } => *name_input,
            other => panic!("expected the macro editor layout, got {other:?}"),
        }
    }

    fn macro_text_rect(layout: &OverlayMouseLayout) -> Rect {
        match layout {
            OverlayMouseLayout::EditMacros { text_input, .. } => *text_input,
            other => panic!("expected the macro editor layout, got {other:?}"),
        }
    }

    /// A caret column is a DISPLAY column. Feeding a byte offset (what
    /// `TextInput::cursor` holds) straight into a column, as the startup-log
    /// filter used to, puts the caret three cells past the end of a CJK word.
    #[test]
    fn single_line_caret_column_counts_cells_not_bytes_or_chars() {
        use super::single_line_caret_column;

        let text = "日本語";
        assert_eq!(text.len(), 9, "nine bytes");
        assert_eq!(text.chars().count(), 3, "three characters");
        assert_eq!(
            single_line_caret_column(text, text.len(), 0),
            6,
            "six display cells"
        );
        assert_eq!(single_line_caret_column(text, 0, 0), 0);
        assert_eq!(single_line_caret_column(text, 3, 0), 2, "after one glyph");
        assert_eq!(single_line_caret_column(text, 3, 1), 3, "plus the pad");
        // A cursor landing inside a character clamps back to its boundary.
        assert_eq!(single_line_caret_column(text, 4, 0), 2);
        // Past the end clamps to the end.
        assert_eq!(single_line_caret_column(text, 99, 0), 6);
        assert_eq!(single_line_caret_column("🚀a", "🚀".len(), 0), 2);
    }

    /// The macro editor's name field is a SINGLE-LINE field, so it must be
    /// rendered by the one single-line renderer and its hardware caret must sit
    /// on the DISPLAY column of the caret, not on a character count. With a CJK
    /// name every glyph is two cells wide, so a char count lands the caret in
    /// the middle of the text.
    #[test]
    fn macro_editor_name_caret_is_display_width_aware() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = macro_editor_app(super::MacroEditFocus::Name);
        match &mut app.prompt {
            PromptState::EditMacros {
                editing: Some(state),
                ..
            } => {
                state.name_input = TextInput::new();
                state.name_input.text = "日本語".to_string();
                state.name_input.cursor = state.name_input.text.len();
            }
            other => panic!("expected an open macro editor, got {other:?}"),
        }
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let name_rect = macro_name_rect(&app.overlay_layout.active);
        let cursor = ratatui::backend::Backend::get_cursor_position(terminal.backend_mut())
            .expect("a focused name field paints a caret");
        // The field is padded by one leading space and "日本語" is six cells.
        assert_eq!(
            cursor.x,
            name_rect.x + 1 + 6,
            "the caret must sit past six display cells, not past three characters"
        );
    }

    #[test]
    fn macro_editor_focused_control_renders_differently_from_the_unfocused_ones() {
        use super::MacroEditFocus;

        // The two text fields: whichever has focus draws a focused border.
        let name_when_name_focused =
            corner_style(&mut macro_editor_app(MacroEditFocus::Name), macro_name_rect);
        let name_when_body_focused =
            corner_style(&mut macro_editor_app(MacroEditFocus::Text), macro_name_rect);
        assert_ne!(
            name_when_name_focused, name_when_body_focused,
            "the name field must look focused only while it has focus"
        );

        let body_when_body_focused =
            corner_style(&mut macro_editor_app(MacroEditFocus::Text), macro_text_rect);
        let body_when_name_focused =
            corner_style(&mut macro_editor_app(MacroEditFocus::Name), macro_text_rect);
        assert_eq!(
            body_when_body_focused, name_when_name_focused,
            "both fields use the one focused-field idiom"
        );
        assert_ne!(
            body_when_body_focused, body_when_name_focused,
            "focus you cannot see is not focus"
        );

        // The selector marker.
        let selector_marker = |focus: MacroEditFocus| -> Style {
            use ratatui::Terminal;
            use ratatui::backend::TestBackend;
            let mut app = macro_editor_app(focus);
            let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("terminal");
            terminal
                .draw(|frame| app.render(frame))
                .expect("render frame");
            let buf = terminal.backend().buffer();
            let OverlayMouseLayout::EditMacros {
                surface_options, ..
            } = app.overlay_layout.active
            else {
                panic!("expected the macro editor layout");
            };
            let rect = surface_options[0];
            let cell = &buf[(rect.x, rect.y)];
            Style::default()
                .fg(cell.fg)
                .bg(cell.bg)
                .add_modifier(cell.modifier)
        };
        assert_ne!(
            selector_marker(MacroEditFocus::Surface),
            selector_marker(MacroEditFocus::Name),
            "the focused surface selector must look focused"
        );

        // The two buttons, read off their own published rects (never a byte
        // offset into a row string: the modal's box-drawing glyphs are
        // multi-byte, so a byte index is not a column).
        let save_rect = |layout: &OverlayMouseLayout| match layout {
            OverlayMouseLayout::EditMacros { save_button, .. } => *save_button,
            other => panic!("expected the macro editor layout, got {other:?}"),
        };
        let cancel_rect = |layout: &OverlayMouseLayout| match layout {
            OverlayMouseLayout::EditMacros { cancel_button, .. } => *cancel_button,
            other => panic!("expected the macro editor layout, got {other:?}"),
        };
        // `corner_style` steps one cell out of the rect it is given; the button
        // rects are OUTER rects, so offset back in to land on the border.
        let button_border = |focus: MacroEditFocus, pick: &dyn Fn(&OverlayMouseLayout) -> Rect| {
            corner_style(&mut macro_editor_app(focus), |layout| {
                let r = pick(layout);
                Rect::new(r.x + 1, r.y + 1, r.width, r.height)
            })
        };
        assert_ne!(
            button_border(MacroEditFocus::Save, &save_rect),
            button_border(MacroEditFocus::Cancel, &save_rect),
            "the focused Save button must look focused"
        );
        assert_ne!(
            button_border(MacroEditFocus::Cancel, &cancel_rect),
            button_border(MacroEditFocus::Save, &cancel_rect),
            "the focused Cancel button must look focused"
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

    // ── PTY cell colors render verbatim in every mode ──────────────

    /// The non-interactive (minimized, not focused) agent pane must paint the
    /// child's colors verbatim. The old desaturation (dim foreground,
    /// grayscaled background) is gone: the caret and the hint bar carry the
    /// mode signal now, not a washed-out palette.
    #[test]
    fn non_interactive_pane_paints_child_colors_verbatim() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let session_id = app.engine.sessions[0].id.clone();
        // Red foreground on blue background, at a known cell.
        let args = vec![
            "-c".to_string(),
            "printf '\\033[5;9H\\033[31;44mX'; sleep 30".to_string(),
        ];
        let client = PtyClient::spawn("/bin/sh", &args, std::path::Path::new("."), 24, 80, 100)
            .expect("spawn pty");
        app.engine.providers.insert(session_id, client);

        // Non-interactive: the pane is visible but input is NOT routed to it.
        app.session_surface = SessionSurface::Agent;
        app.fullscreen_overlay = FullscreenOverlay::None;
        app.center_mode = CenterMode::Agent;
        app.input_target = InputTarget::None;

        // The cursor lands one cell right of the X once it is drawn.
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
        let cell = &terminal.backend().buffer()[(term_area.x + 8, term_area.y + 4)];
        assert_eq!(cell.symbol(), "X");
        // The emulator reports the standard palette as indexed colors: SGR 31
        // is palette index 1 (red) and SGR 44 is palette index 4 (blue).
        assert_eq!(
            cell.fg,
            Color::Indexed(1),
            "the child's SGR 31 foreground must render verbatim, not dimmed"
        );
        assert_eq!(
            cell.bg,
            Color::Indexed(4),
            "the child's SGR 44 background must render verbatim, not grayscaled"
        );
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
                focus: ConfirmFocus::Cancel,
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
    fn fit_agent_meta_line_fits_without_truncation() {
        let out = fit_agent_meta_line(
            80,
            Span::raw("  ※ ".to_string()),
            Some(Span::raw("proj".to_string())),
            Span::raw("Idle".to_string()),
            None,
            Vec::new(),
            MetaLineStyle {
                sep: Style::default(),
                highlight: None,
            },
        );
        assert_eq!(spans_text(&out), "  ※ proj · Idle");
        assert!(!spans_text(&out).contains('…'));
    }

    #[test]
    fn fit_agent_meta_line_truncates_name_keeps_state_and_tabs() {
        let out = fit_agent_meta_line(
            30,
            Span::raw("  ※ ".to_string()),
            Some(Span::raw("a-very-long-project-name".to_string())),
            Span::raw("Idle".to_string()),
            None,
            vec![Span::raw("3 tabs".to_string())],
            MetaLineStyle {
                sep: Style::default(),
                highlight: None,
            },
        );
        assert!(spans_width(&out) <= 30, "never overflows the width");
        let text = spans_text(&out);
        assert!(text.starts_with("  ※ "), "marker stays: {text:?}");
        assert!(text.contains("Idle"), "state word stays fixed: {text:?}");
        assert!(text.contains("3 tabs"), "tab count stays fixed: {text:?}");
        assert!(
            text.contains('…'),
            "the project name is what truncates: {text:?}"
        );
    }

    #[test]
    fn fit_agent_meta_line_shares_budget_between_name_and_branch() {
        let out = fit_agent_meta_line(
            34,
            Span::raw("  ※ ".to_string()),
            Some(Span::raw("longproject".to_string())),
            Span::raw("Working".to_string()),
            Some(Span::raw("feature/some-long-branch".to_string())),
            Vec::new(),
            MetaLineStyle {
                sep: Style::default(),
                highlight: None,
            },
        );
        assert!(spans_width(&out) <= 34);
        let text = spans_text(&out);
        assert!(text.contains("Working"), "state word stays fixed: {text:?}");
        assert!(text.contains('…'), "a flexible field truncated: {text:?}");
    }

    // --- fit_agent_meta_line search-hit highlighting ---

    fn meta_hl_style() -> Style {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    }

    /// Build the same meta line with and without a highlight so tests can pin
    /// that the emphasis is a pure styling overlay: rendered characters stay
    /// byte-identical.
    fn meta_line_pair(
        total_w: u16,
        project: &str,
        branch: &str,
        highlight: Option<(&str, Style)>,
    ) -> (Vec<Span<'static>>, Vec<Span<'static>>) {
        let build = |hl: Option<(&str, Style)>| {
            fit_agent_meta_line(
                total_w,
                Span::raw("  ".to_string()),
                Some(Span::raw(project.to_string())),
                Span::raw("Idle".to_string()),
                Some(Span::raw(branch.to_string())),
                Vec::new(),
                MetaLineStyle {
                    sep: Style::default(),
                    highlight: hl,
                },
            )
        };
        (build(highlight), build(None))
    }

    #[test]
    fn meta_line_highlights_a_project_hit_without_changing_the_text() {
        let (hl, plain) =
            meta_line_pair(80, "demo-web", "feat/login", Some(("web", meta_hl_style())));
        let mark = hl
            .iter()
            .find(|s| s.content.as_ref() == "web")
            .expect("the matched project substring gets its own span");
        assert_eq!(mark.style, meta_hl_style());
        assert_eq!(spans_text(&hl), spans_text(&plain));
    }

    #[test]
    fn meta_line_highlights_a_branch_hit_without_changing_the_text() {
        let (hl, plain) = meta_line_pair(
            80,
            "demo-web",
            "feat/login",
            Some(("login", meta_hl_style())),
        );
        let mark = hl
            .iter()
            .find(|s| s.content.as_ref() == "login")
            .expect("the matched branch substring gets its own span");
        assert_eq!(mark.style, meta_hl_style());
        assert_eq!(spans_text(&hl), spans_text(&plain));
    }

    /// The truncation-cuts-the-match case: the emphasis is recomputed on the
    /// EXACT fitted text, so a match the ellipsis swallowed simply does not
    /// highlight, nothing shifts onto the "…", and nothing panics. Multi-byte
    /// project text exercises the char-based paths.
    #[test]
    fn meta_line_truncated_match_never_emphasizes_the_ellipsis() {
        // Width 18: fixed parts (marker 2 + sep 3 + "Idle" 4 + sep 3) leave a
        // 6-cell budget shared by the two fields, so "branch" (the match, at
        // the branch's tail) is guaranteed to be cut off.
        let (hl, plain) = meta_line_pair(
            18,
            "日本語プロジェクト",
            "silver-branch",
            Some(("branch", meta_hl_style())),
        );
        assert!(
            spans_text(&plain).contains('…'),
            "the fixture must truncate"
        );
        assert_eq!(spans_text(&hl), spans_text(&plain));
        assert!(
            hl.iter().all(|s| s.style != meta_hl_style()),
            "a match cut off by the truncation must not highlight anything: {:?}",
            hl,
        );
    }

    #[test]
    fn meta_line_match_that_survives_truncation_still_highlights() {
        // Wide enough that the project keeps its "日本" head even though the
        // branch truncates; the multi-byte hit still gets its emphasis span.
        let (hl, plain) = meta_line_pair(
            30,
            "日本語プロジェクト",
            "silver-branch-very-long",
            Some(("日本", meta_hl_style())),
        );
        assert_eq!(spans_text(&hl), spans_text(&plain));
        let mark = hl
            .iter()
            .find(|s| s.content.as_ref() == "日本")
            .expect("the multi-byte hit keeps its emphasis when it survives the fit");
        assert_eq!(mark.style, meta_hl_style());
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

    // ── scrolling: the bottom must be reachable, and say so ───────────────

    /// Every row of `buf` as a string, in order. Rendered text only: styles are
    /// irrelevant to reachability.
    fn buffer_rows(buf: &ratatui::buffer::Buffer) -> Vec<String> {
        (buf.area.y..buf.area.y + buf.area.height)
            .map(|y| {
                (buf.area.x..buf.area.x + buf.area.width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn the_help_overlay_bottom_is_reachable_at_a_narrow_width() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        // 80 columns is the case that broke: the help pane is
        // `centered_rect(72, 70, ..)`, so its content column is ~55 wide while
        // help lines run to 70+ (a key badge plus a description up to 53
        // characters). They wrap, and a clamp built from the count of LOGICAL
        // lines then stops short of the wrapped bottom.
        let mut app = test_app(default_bindings());
        app.help_scroll = Some(0);
        let backend = TestBackend::new(80, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");

        // Scroll as far as the input handler will allow — this is what the
        // ScrollToBottom key does with the numbers the renderer recorded.
        let max = app
            .last_help_lines
            .saturating_sub(app.last_help_height.max(1));
        app.help_scroll = Some(max);
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");

        // The last thing in the help content is the GitHub integration row,
        // whose description ends in the palette command name. With
        // `github_integration_enabled` false (the test engine's default) that
        // tail is stable.
        let rows = buffer_rows(terminal.backend().buffer());
        assert!(
            rows.iter()
                .any(|row| row.contains("(toggle-github-integration)")),
            "scrolled to the maximum, the LAST line of help content must be on \
             screen; got:\n{}",
            rows.join("\n")
        );
    }

    /// Pre-wrapping the help page must not change how it LOOKS, only how many
    /// rows it admits to being. Measured on the real content, at the widths that
    /// wrap it: paint the same lines with ratatui's `Wrap { trim: false }` (what
    /// the overlay used to do) and with our pre-wrap, and compare every cell.
    #[test]
    fn pre_wrapping_the_help_page_paints_what_wrap_did() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let app = test_app(default_bindings());
        // The no-wrap width is MEASURED from the content, not guessed. The help
        // page prints the ABSOLUTE path of the config file, so its longest line
        // is as long as wherever that config happens to live. This test used to
        // hardcode 100, which held only while the suite ran with a short
        // `TMPDIR`: pointing `TMPDIR` at a deeper directory pushed that one line
        // to 106 columns, and the test then failed reporting a wrap that had
        // nothing to do with the wrapping code it exists to check. Asking the
        // content how wide it is keeps the assertion about wrapping.
        let widest = app
            .help_content_lines(400)
            .iter()
            .map(|l| l.width())
            .max()
            .unwrap_or(0);
        let wide = u16::try_from(widest.max(100)).unwrap_or(u16::MAX);
        for width in [40u16, 55, 72, wide] {
            let lines = app.help_content_lines(width as usize);
            // Tall enough that neither rendering is clipped at the bottom.
            let height = u16::try_from(lines.len() * 3).unwrap_or(u16::MAX).max(10);

            let mut legacy = Terminal::new(TestBackend::new(width, height)).expect("terminal");
            legacy
                .draw(|frame| {
                    Paragraph::new(lines.clone())
                        .wrap(Wrap { trim: false })
                        .render(frame.area(), frame.buffer_mut());
                })
                .expect("render frame");

            let wrapped = wrap_styled_lines(&lines, width as usize);
            let mut ours = Terminal::new(TestBackend::new(width, height)).expect("terminal");
            ours.draw(|frame| {
                Paragraph::new(wrapped.clone()).render(frame.area(), frame.buffer_mut());
            })
            .expect("render frame");

            let before = buffer_rows(legacy.backend().buffer());
            let after = buffer_rows(ours.backend().buffer());
            for (y, (want, got)) in before.iter().zip(after.iter()).enumerate() {
                assert_eq!(
                    want.trim_end(),
                    got.trim_end(),
                    "help row {y} at width {width} changed appearance\n  was: {want:?}\n  now: {got:?}"
                );
            }
            // ...and at the narrow widths the wrap really did happen, or the
            // comparison would prove nothing. At a width no narrower than the
            // widest line nothing wraps, which is itself worth pinning: a wide
            // terminal's help page is untouched.
            if width < wide {
                assert!(
                    wrapped.len() > lines.len(),
                    "width {width} must wrap the help content"
                );
            } else {
                assert_eq!(
                    wrapped.len(),
                    lines.len(),
                    "nothing should wrap at {width} columns (widest line is {widest})"
                );
            }
        }
    }

    /// Every scroll-direction glyph the shared marker can draw.
    const MARKERS: [&str; 3] = ["↓", "↑", "↕"];

    /// The single marker glyph inside `rect`, or `None`. Panics if there are
    /// several: a surface must never draw two.
    fn marker_in(buf: &ratatui::buffer::Buffer, rect: Rect) -> Option<String> {
        let mut found: Vec<(u16, u16, String)> = Vec::new();
        for y in rect.y..rect.y + rect.height {
            for x in rect.x..rect.x + rect.width {
                let symbol = buf[(x, y)].symbol().to_string();
                if MARKERS.contains(&symbol.as_str()) {
                    found.push((x, y, symbol));
                }
            }
        }
        assert!(
            found.len() <= 1,
            "expected at most one scroll marker in {rect:?}, found {found:?}"
        );
        found.pop().map(|(_, _, symbol)| symbol)
    }

    /// The help overlay's rects: the modal, its content pane, and the border
    /// column the marker is allowed to use. Mirrors `render_help`'s layout.
    fn help_rects(frame: Rect) -> (Rect, Rect, Rect) {
        let area = centered_rect(72, 70, frame);
        let inner = Rect::new(
            area.x + 1,
            area.y + 1,
            area.width - 2,
            area.height.saturating_sub(2),
        );
        let content = Rect::new(inner.x, inner.y, inner.width, inner.height - 2);
        let border_column = Rect::new(area.x + area.width - 1, area.y, 1, area.height);
        (area, content, border_column)
    }

    /// Render the help overlay at `size` with `scroll` applied, and hand back the
    /// app plus the frame buffer.
    fn help_frame(size: (u16, u16), scroll: u16) -> (App, ratatui::buffer::Buffer) {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        app.help_scroll = Some(scroll);
        let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let buf = terminal.backend().buffer().clone();
        (app, buf)
    }

    #[test]
    fn the_help_overlay_marker_points_the_way_at_top_middle_and_bottom() {
        let frame = Rect::new(0, 0, 80, 40);
        let (_, content, border_column) = help_rects(frame);

        // Top: only down.
        let (app, buf) = help_frame((80, 40), 0);
        assert_eq!(marker_in(&buf, border_column).as_deref(), Some("↓"));
        let max = app
            .last_help_lines
            .saturating_sub(app.last_help_height.max(1));
        assert!(
            max > 1,
            "the fixture must overflow for this test to mean anything"
        );

        // Middle: both ways.
        let (_, buf) = help_frame((80, 40), max / 2);
        assert_eq!(marker_in(&buf, border_column).as_deref(), Some("↕"));

        // Bottom: only up.
        let (_, buf) = help_frame((80, 40), max);
        assert_eq!(marker_in(&buf, border_column).as_deref(), Some("↑"));

        // And in no case did it land where content lives.
        for scroll in [0, max / 2, max] {
            let (_, buf) = help_frame((80, 40), scroll);
            assert_eq!(
                marker_in(&buf, content),
                None,
                "the marker must stay in the border column, off the content pane"
            );
        }
    }

    #[test]
    fn the_help_overlay_shows_no_marker_when_everything_fits() {
        // A terminal roomy enough that the whole help page fits unwrapped: with
        // nothing off-screen there is nothing to point at, and a marker would be
        // a lie.
        let (app, buf) = help_frame((200, 200), 0);
        assert!(
            app.last_help_lines <= app.last_help_height,
            "fixture must actually fit: {} lines in {} rows",
            app.last_help_lines,
            app.last_help_height
        );
        let (_, _, border_column) = help_rects(Rect::new(0, 0, 200, 200));
        assert_eq!(marker_in(&buf, border_column), None);
    }

    /// Open the command palette with `filter` typed and `selected` highlighted,
    /// render, and return the app and buffer.
    fn palette_frame(
        size: (u16, u16),
        filter: &str,
        selected: usize,
    ) -> (App, ratatui::buffer::Buffer) {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let mut input = crate::app::text_input::TextInput::new();
        for ch in filter.chars() {
            input.insert_char(ch);
        }
        app.prompt = PromptState::Command { input, selected };
        let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let buf = terminal.backend().buffer().clone();
        (app, buf)
    }

    /// The palette list's inner rect and the border column beside it, taken from
    /// the layout the renderer itself recorded rather than re-derived.
    fn palette_rects(app: &App) -> (Rect, Rect) {
        match app.overlay_layout.active {
            OverlayMouseLayout::Command { list, .. } => {
                (list, Rect::new(list.x + list.width, list.y, 1, list.height))
            }
            ref other => panic!("expected the palette layout, got {other:?}"),
        }
    }

    #[test]
    fn the_command_palette_marker_tracks_the_item_offset() {
        // The palette is a LIST: its offset counts whole items, not rows of
        // wrapped text, and that is the unit the marker must be fed.
        let (app, buf) = palette_frame((120, 40), "", 0);
        let (list, border_column) = palette_rects(&app);
        let items = match app.overlay_layout.active {
            OverlayMouseLayout::Command { items, .. } => items,
            _ => unreachable!(),
        };
        assert!(
            items > list.height as usize + 1,
            "the palette must overflow its list for this test to mean anything: \
             {items} commands in {} rows",
            list.height
        );
        assert_eq!(marker_in(&buf, border_column).as_deref(), Some("↓"));
        assert_eq!(
            marker_in(&buf, list),
            None,
            "a marker inside the list would sit on a command's own row"
        );

        // Selecting an item just past the viewport scrolls the list by one, which
        // is the middle.
        let (app, buf) = palette_frame((120, 40), "", list.height as usize);
        let (_, border_column) = palette_rects(&app);
        assert_eq!(marker_in(&buf, border_column).as_deref(), Some("↕"));

        // Selecting the last command scrolls to the end.
        let (app, buf) = palette_frame((120, 40), "", items - 1);
        let (_, border_column) = palette_rects(&app);
        assert_eq!(marker_in(&buf, border_column).as_deref(), Some("↑"));
    }

    #[test]
    fn the_command_palette_shows_no_marker_when_the_filtered_list_fits() {
        // A filter narrow enough that every match is on screen.
        let (app, buf) = palette_frame((120, 40), "toggle-github", 0);
        let (list, border_column) = palette_rects(&app);
        let items = match app.overlay_layout.active {
            OverlayMouseLayout::Command { items, .. } => items,
            _ => unreachable!(),
        };
        assert!(
            items > 0 && items <= list.height as usize,
            "fixture must fit: {items} commands in {} rows",
            list.height
        );
        assert_eq!(marker_in(&buf, border_column), None);
    }

    /// Put the center pane in diff mode with `count` synthetic lines and render.
    fn diff_frame(
        size: (u16, u16),
        count: usize,
        gutter_width: usize,
        scroll: u16,
    ) -> (App, ratatui::buffer::Buffer) {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        app.focus = FocusPane::Center;
        let lines: Vec<Line<'static>> = (0..count)
            .map(|i| {
                if gutter_width > 0 {
                    Line::from(vec![
                        Span::raw(format!("{:>4}│", i + 1)),
                        Span::raw(format!(" line {i}")),
                    ])
                } else {
                    Line::from(format!("line {i}"))
                }
            })
            .collect();
        app.center_mode = CenterMode::Diff {
            lines: Arc::new(lines),
            scroll,
            gutter_width,
            worktree_path: "/tmp/does-not-matter".to_string(),
            rel_path: "src/main.rs".to_string(),
        };
        let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let buf = terminal.backend().buffer().clone();
        (app, buf)
    }

    /// The diff pane's content rect (which is also the text-selection surface)
    /// and the border column beside it.
    fn diff_rects(app: &App) -> (Rect, Rect) {
        let content = app
            .mouse_layout
            .agent_term
            .expect("the diff records its content area for text selection");
        (
            content,
            Rect::new(content.x + content.width, content.y, 1, content.height),
        )
    }

    #[test]
    fn the_diff_marker_points_the_way_on_both_wrapping_paths() {
        // Both branches of `render_diff` must produce a marker, and the same
        // one: the gutter-aware path and ratatui's own wrapping path.
        for gutter_width in [0usize, 6] {
            let (app, buf) = diff_frame((120, 40), 400, gutter_width, 0);
            let (content, border_column) = diff_rects(&app);
            assert!(
                app.last_diff_visual_lines > content.height,
                "the fixture must overflow (gutter {gutter_width})"
            );
            assert_eq!(
                marker_in(&buf, border_column).as_deref(),
                Some("↓"),
                "at the top of the diff (gutter {gutter_width})"
            );

            let max = app.last_diff_visual_lines - content.height;
            let (app, buf) = diff_frame((120, 40), 400, gutter_width, max / 2);
            let (_, border_column) = diff_rects(&app);
            assert_eq!(
                marker_in(&buf, border_column).as_deref(),
                Some("↕"),
                "in the middle of the diff (gutter {gutter_width})"
            );

            let (app, buf) = diff_frame((120, 40), 400, gutter_width, max);
            let (content, border_column) = diff_rects(&app);
            assert_eq!(
                marker_in(&buf, border_column).as_deref(),
                Some("↑"),
                "at the bottom of the diff (gutter {gutter_width})"
            );
            assert_eq!(
                marker_in(&buf, content),
                None,
                "the marker must never sit on a diff row (gutter {gutter_width})"
            );
        }
    }

    #[test]
    fn a_short_diff_gets_no_marker() {
        for gutter_width in [0usize, 6] {
            let (app, buf) = diff_frame((120, 40), 3, gutter_width, 0);
            let (content, border_column) = diff_rects(&app);
            assert!(app.last_diff_visual_lines <= content.height, "fixture fits");
            assert_eq!(marker_in(&buf, border_column), None);
        }
    }

    #[test]
    fn the_diff_marker_leaves_text_selection_geometry_untouched() {
        // `mouse_layout.agent_term` is the drag-to-select surface. The marker
        // lands in the border column OUTSIDE it, so a short diff (no marker) and
        // a long one (marker) must record the same rect, and the marker cell must
        // never be inside it.
        for gutter_width in [0usize, 6] {
            let (short, _) = diff_frame((120, 40), 3, gutter_width, 0);
            let (long, _) = diff_frame((120, 40), 400, gutter_width, 7);
            let short_rect = short.mouse_layout.agent_term.expect("selection surface");
            let long_rect = long.mouse_layout.agent_term.expect("selection surface");
            assert_eq!(
                short_rect, long_rect,
                "drawing a scroll marker must not resize the selection surface"
            );

            let cell = crate::app::components::scroll_marker_rect(
                centered_diff_pane_area(&long),
                long_rect,
            );
            assert!(
                cell.x >= long_rect.x + long_rect.width,
                "the marker cell {cell:?} is inside the selection surface {long_rect:?}"
            );
            assert!(
                cell.y >= long_rect.y && cell.y < long_rect.y + long_rect.height,
                "the marker must stay on the content pane's rows"
            );
        }
    }

    /// Which of the two error dialogs to put on screen. They are separate
    /// `PromptState` variants with the same problem: a long, multi-line message
    /// (a TOML validation error is normally many lines) used to be cut at six
    /// lines with nothing to say so.
    #[derive(Clone, Copy)]
    enum ErrorDialog {
        ConfigReload,
        AddProject,
    }

    /// Render one of the error dialogs carrying `message`, with `scroll` applied.
    fn error_dialog_frame(
        which: ErrorDialog,
        size: (u16, u16),
        message: String,
        scroll: u16,
    ) -> (App, ratatui::buffer::Buffer) {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        app.prompt = match which {
            ErrorDialog::ConfigReload => PromptState::ConfigReloadFailed {
                error: message,
                recover_old_config: false,
                focus: ConfigReloadFailedFocus::Close,
                scroll,
            },
            ErrorDialog::AddProject => PromptState::AddProjectFailed {
                message,
                return_prompt: Box::new(PromptState::None),
                scroll,
            },
        };
        let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let buf = terminal.backend().buffer().clone();
        (app, buf)
    }

    /// The error dialogs are a fixed 68 columns wide, centered, so their right
    /// border column (the only cell the marker may use) is derivable.
    fn error_dialog_border_column(size: (u16, u16)) -> Rect {
        let width = 68u16.min(size.0.max(1));
        let x = size.0.saturating_sub(width) / 2;
        Rect::new(x + width - 1, 0, 1, size.1)
    }

    /// A message with numbered lines, so a test can name the one it is looking
    /// for. Line 7 onward is what the old `take(6)` threw away.
    fn long_error_message(lines: usize) -> String {
        (0..lines)
            .map(|i| format!("error detail line {i}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_long_error_dialog_message_is_fully_reachable() {
        // Both dialogs are how a user learns their config is broken, and the tail
        // of a TOML validation error is usually the part naming the problem, so
        // nothing may be dropped: the lines past the old six-line cut must be
        // readable, and the LAST line must be retrievable.
        for which in [ErrorDialog::ConfigReload, ErrorDialog::AddProject] {
            let count = 40;
            let message = long_error_message(count);
            let (app, buf) = error_dialog_frame(which, (100, 30), message.clone(), 0);
            let rows = buffer_rows(&buf);
            assert!(
                rows.iter().any(|row| row.contains("error detail line 0")),
                "the first line must be on screen:\n{}",
                rows.join("\n")
            );
            assert!(
                app.last_error_dialog_lines > app.last_error_dialog_height,
                "the fixture must overflow the dialog: {} lines in {} rows",
                app.last_error_dialog_lines,
                app.last_error_dialog_height
            );

            // Scroll as far as the input handler will allow, which is what the
            // scroll-to-bottom key does with the numbers the renderer recorded.
            let max = app
                .last_error_dialog_lines
                .saturating_sub(app.last_error_dialog_height.max(1));
            let (_, buf) = error_dialog_frame(which, (100, 30), message.clone(), max);
            let rows = buffer_rows(&buf);
            assert!(
                rows.iter()
                    .any(|row| row.contains(&format!("error detail line {}", count - 1))),
                "scrolled to the bottom, the LAST line of the error must be on \
                 screen:\n{}",
                rows.join("\n")
            );

            // And every line in between is reachable at some offset: walk the
            // whole message a page at a time and tick each one off.
            let page = app.last_error_dialog_height.max(1);
            let mut seen = vec![false; count];
            let mut scroll = 0u16;
            loop {
                let (_, buf) = error_dialog_frame(which, (100, 30), message.clone(), scroll);
                let rows = buffer_rows(&buf).join("\n");
                for (i, seen) in seen.iter_mut().enumerate() {
                    if rows.contains(&format!("error detail line {i} ")) {
                        *seen = true;
                    }
                }
                if scroll >= max {
                    break;
                }
                scroll = (scroll + page).min(max);
            }
            let missing: Vec<usize> = seen
                .iter()
                .enumerate()
                .filter(|(_, seen)| !**seen)
                .map(|(i, _)| i)
                .collect();
            assert!(
                missing.is_empty(),
                "every line of the message must be reachable; never saw {missing:?}"
            );
        }
    }

    #[test]
    fn the_error_dialog_marker_points_the_way_at_top_middle_and_bottom() {
        for which in [ErrorDialog::ConfigReload, ErrorDialog::AddProject] {
            let border_column = error_dialog_border_column((100, 30));
            let message = long_error_message(40);
            let (app, buf) = error_dialog_frame(which, (100, 30), message.clone(), 0);
            assert_eq!(marker_in(&buf, border_column).as_deref(), Some("↓"));
            let max = app
                .last_error_dialog_lines
                .saturating_sub(app.last_error_dialog_height.max(1));
            assert!(max > 1, "the fixture must overflow");

            let (_, buf) = error_dialog_frame(which, (100, 30), message.clone(), max / 2);
            assert_eq!(marker_in(&buf, border_column).as_deref(), Some("↕"));

            let (_, buf) = error_dialog_frame(which, (100, 30), message, max);
            assert_eq!(marker_in(&buf, border_column).as_deref(), Some("↑"));
        }
    }

    #[test]
    fn a_short_error_dialog_message_gets_no_marker() {
        // A one-line error fits, and a marker on a dialog that cannot scroll
        // would be a lie.
        for which in [ErrorDialog::ConfigReload, ErrorDialog::AddProject] {
            let (app, buf) = error_dialog_frame(
                which,
                (100, 30),
                "invalid value for `ui.right_width_pct`".to_string(),
                0,
            );
            assert!(
                app.last_error_dialog_lines <= app.last_error_dialog_height,
                "fixture must fit: {} lines in {} rows",
                app.last_error_dialog_lines,
                app.last_error_dialog_height
            );
            let border_column = error_dialog_border_column((100, 30));
            assert_eq!(marker_in(&buf, border_column), None);
        }
    }

    #[test]
    fn a_huge_error_message_still_leaves_the_dialog_controls_on_screen() {
        // The message pane now sizes itself to the message, so an unbounded one
        // could have sized the dialog past the terminal and let the layout solver
        // eat the buttons. It is capped at what the terminal can show instead.
        let message = long_error_message(400);
        let (_, buf) = error_dialog_frame(ErrorDialog::ConfigReload, (100, 20), message.clone(), 0);
        let rows = buffer_rows(&buf).join("\n");
        assert!(
            rows.contains("Close") && rows.contains("Recover"),
            "both buttons must survive a 400-line error:\n{rows}"
        );
        assert!(
            rows.contains("Recover last working config"),
            "the checkbox must survive too:\n{rows}"
        );

        let (_, buf) = error_dialog_frame(ErrorDialog::AddProject, (100, 20), message, 0);
        let rows = buffer_rows(&buf).join("\n");
        assert!(rows.contains("OK"), "the OK button must survive:\n{rows}");
    }

    #[test]
    fn the_error_dialog_marker_never_covers_a_full_width_message_line() {
        // The message is rendered with a leading space in a 66-column content
        // pane, so a 65-character line reaches the pane's last column. The marker
        // lives one cell further out, in the dialog's border column.
        for which in [ErrorDialog::ConfigReload, ErrorDialog::AddProject] {
            let message = (0..40)
                .map(|_| "X".repeat(65))
                .collect::<Vec<_>>()
                .join("\n");
            let (_, buf) = error_dialog_frame(which, (100, 30), message, 3);
            let border_column = error_dialog_border_column((100, 30));
            assert_eq!(marker_in(&buf, border_column).as_deref(), Some("↕"));
            // Every row that holds message text keeps its last content column.
            let last_content = border_column.x - 1;
            let rows_with_text = (0..30u16)
                .filter(|y| buf[(border_column.x - 2, *y)].symbol() == "X")
                .count();
            assert!(rows_with_text > 3, "the fixture must paint several rows");
            for y in 0..30u16 {
                if buf[(border_column.x - 2, y)].symbol() == "X" {
                    assert_eq!(
                        buf[(last_content, y)].symbol(),
                        "X",
                        "row {y}'s last content column must still hold the message"
                    );
                }
            }
        }
    }

    #[test]
    fn the_diff_hint_bar_reports_the_clamped_scroll_position() {
        // The hint text used to print the raw `center_mode` scroll while the view
        // rendered a clamped one, so after the content shrank (a shorter file, a
        // refresh) the number overstated where the reader actually was until the
        // next key press.
        for gutter_width in [0usize, 6] {
            // A stale offset far past the end of a long diff: the hint must
            // report the position the view actually drew.
            let (app, buf) = diff_frame((120, 40), 400, gutter_width, 10_000);
            let (content, _) = diff_rects(&app);
            let max = app.last_diff_visual_lines - content.height;
            let rows = buffer_rows(&buf);
            let hint = rows
                .iter()
                .find(|row| row.contains("Scrolled back"))
                .unwrap_or_else(|| panic!("expected a scroll hint (gutter {gutter_width})"));
            assert!(
                hint.contains(&format!("Scrolled back {max} lines")),
                "the hint must report the clamped position {max} (gutter \
                 {gutter_width}); got {hint:?}"
            );
            assert!(
                !hint.contains("10000"),
                "the raw offset must never reach the hint (gutter {gutter_width}): {hint:?}"
            );

            // A diff short enough that nothing can scroll: the clamped position
            // is 0, so the hint must be the un-scrolled variant rather than
            // "Scrolled back 0 lines".
            let (_, buf) = diff_frame((120, 40), 3, gutter_width, 50);
            let rows = buffer_rows(&buf);
            assert!(
                !rows.iter().any(|row| row.contains("Scrolled back")),
                "a diff that fits is not scrolled back (gutter {gutter_width}):\n{}",
                rows.join("\n")
            );
        }
    }

    /// Put the fullscreen startup-log viewer on screen with `rows` log lines and
    /// `scroll` applied.
    ///
    /// The viewer measures in wrapped visual ROWS: its content is pre-split to
    /// the pane width by `startup_command_log_visual_lines`, so one entry is one
    /// row. Each fixture line is short enough not to wrap, which makes the row
    /// count equal to `rows` and the arithmetic in the assertions checkable.
    fn startup_log_frame(
        size: (u16, u16),
        rows: usize,
        scroll: u16,
        long_line: bool,
    ) -> (App, ratatui::buffer::Buffer) {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let content = (0..rows)
            .map(|i| {
                if long_line {
                    // Exactly as wide as the content pane, so every rendered row
                    // reaches its last content column: the marker must not eat it.
                    let pane = centered_rect(96, 94, Rect::new(0, 0, size.0, size.1));
                    "X".repeat(pane.width.saturating_sub(2) as usize)
                } else {
                    format!("log line {i}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        app.fullscreen_overlay = FullscreenOverlay::StartupLog;
        app.startup_log_viewer = Some(StartupLogViewer {
            scope_label: "project my-proj".to_string(),
            path: None,
            display_name: "startup.log".to_string(),
            content,
            scroll_offset: scroll,
            wrap_width: 0,
            search: crate::app::text_input::TextInput::new(),
            searching: false,
            return_to: None,
        });
        let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let buf = terminal.backend().buffer().clone();
        (app, buf)
    }

    /// The startup-log viewer's content pane (recorded by the renderer) and the
    /// border column beside its modal.
    fn startup_log_rects(app: &App, frame: Rect) -> (Rect, Rect) {
        let content = app
            .mouse_layout
            .agent_term
            .expect("the startup log records its content area");
        let area = centered_rect(96, 94, frame);
        (
            content,
            Rect::new(area.x + area.width - 1, area.y, 1, area.height),
        )
    }

    #[test]
    fn the_fullscreen_startup_log_marker_points_the_way_at_top_middle_and_bottom() {
        let frame = Rect::new(0, 0, 100, 30);
        let rows = 400;

        let (app, buf) = startup_log_frame((100, 30), rows, 0, false);
        let (content, border_column) = startup_log_rects(&app, frame);
        assert!(
            rows > content.height as usize,
            "the fixture must overflow: {rows} rows in {}",
            content.height
        );
        assert_eq!(marker_in(&buf, border_column).as_deref(), Some("↓"));

        let max = rows as u16 - content.height;
        let (_, buf) = startup_log_frame((100, 30), rows, max / 2, false);
        assert_eq!(marker_in(&buf, border_column).as_deref(), Some("↕"));

        let (_, buf) = startup_log_frame((100, 30), rows, max, false);
        assert_eq!(marker_in(&buf, border_column).as_deref(), Some("↑"));

        for scroll in [0, max / 2, max] {
            let (_, buf) = startup_log_frame((100, 30), rows, scroll, false);
            assert_eq!(
                marker_in(&buf, content),
                None,
                "the marker must stay in the border column, off the log's cells"
            );
        }
    }

    #[test]
    fn a_short_startup_log_gets_no_marker() {
        let frame = Rect::new(0, 0, 100, 30);
        let (app, buf) = startup_log_frame((100, 30), 3, 0, false);
        let (content, border_column) = startup_log_rects(&app, frame);
        assert!(content.height > 3, "fixture must fit");
        assert_eq!(marker_in(&buf, border_column), None);
    }

    #[test]
    fn the_startup_log_marker_never_covers_a_full_width_log_line() {
        // The log is painted cell-by-cell with `set_string`, so a line as wide as
        // the pane occupies its LAST content column. The marker lives in the
        // modal's border column, one cell further out, so that character survives.
        let frame = Rect::new(0, 0, 100, 30);
        let (app, buf) = startup_log_frame((100, 30), 400, 5, true);
        let (content, border_column) = startup_log_rects(&app, frame);
        assert_eq!(marker_in(&buf, border_column).as_deref(), Some("↕"));
        let last_col = content.x + content.width - 1;
        for y in content.y..content.y + content.height {
            assert_eq!(
                buf[(last_col, y)].symbol(),
                "X",
                "row {y}'s last content column must still hold the log's own text"
            );
        }
    }

    #[test]
    fn the_startup_log_marker_clears_the_search_bar() {
        // The search bar overlays the bottom three rows of the modal's INNER
        // area, which includes the marker's row. It must not reach the border
        // column the marker uses.
        let frame = Rect::new(0, 0, 100, 30);
        let (mut app, _) = startup_log_frame((100, 30), 400, 5, false);
        let (_, border_column) = startup_log_rects(&app, frame);
        if let Some(viewer) = &mut app.startup_log_viewer {
            viewer.searching = true;
        }
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        assert_eq!(
            marker_in(terminal.backend().buffer(), border_column).as_deref(),
            Some("↕"),
            "the search bar must not paint over the marker"
        );
    }

    /// Open the Startup Command Logs overlay with `rows` lines of output and
    /// `scroll` applied to the Output body.
    fn startup_command_logs_frame(
        size: (u16, u16),
        rows: usize,
        scroll: u16,
    ) -> (App, ratatui::buffer::Buffer) {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let content = (0..rows)
            .map(|i| format!("output line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        app.prompt = PromptState::StartupCommandLogs(crate::app::StartupCommandLogPrompt {
            scope_label: "my-proj".to_string(),
            entries: vec![dux_core::startup::StartupCommandLogEntry {
                path: std::path::PathBuf::from("/tmp/startup.log"),
                display_name: "startup.log".to_string(),
                modified_at: None,
            }],
            selected: 0,
            filter: crate::app::text_input::TextInput::new(),
            searching: false,
            content,
            scroll_offset: scroll,
            wrap_width: 0,
            focus: StartupCommandLogFocus::List,
        });
        let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let buf = terminal.backend().buffer().clone();
        (app, buf)
    }

    /// The Output body pane and the border column of its own bordered block.
    fn startup_command_logs_rects(app: &App) -> (Rect, Rect) {
        match app.overlay_layout.active {
            OverlayMouseLayout::StartupCommandLogs { body, .. } => {
                (body, Rect::new(body.x + body.width, body.y, 1, body.height))
            }
            ref other => panic!("expected the startup command logs layout, got {other:?}"),
        }
    }

    #[test]
    fn the_startup_command_logs_output_marker_points_the_way_at_top_middle_and_bottom() {
        // The Output body measures in wrapped visual ROWS, like the fullscreen
        // viewer: its content is pre-split to the body width.
        let rows = 400;
        let (app, buf) = startup_command_logs_frame((120, 34), rows, 0);
        let (body, border_column) = startup_command_logs_rects(&app);
        assert!(
            rows > body.height as usize,
            "the fixture must overflow the body"
        );
        assert_eq!(marker_in(&buf, border_column).as_deref(), Some("↓"));

        let max = rows as u16 - body.height;
        let (_, buf) = startup_command_logs_frame((120, 34), rows, max / 2);
        assert_eq!(marker_in(&buf, border_column).as_deref(), Some("↕"));

        let (_, buf) = startup_command_logs_frame((120, 34), rows, max);
        assert_eq!(marker_in(&buf, border_column).as_deref(), Some("↑"));

        for scroll in [0, max / 2, max] {
            let (_, buf) = startup_command_logs_frame((120, 34), rows, scroll);
            assert_eq!(
                marker_in(&buf, body),
                None,
                "the marker must stay in the body block's border column"
            );
        }
    }

    #[test]
    fn a_short_startup_command_log_output_gets_no_marker() {
        let (app, buf) = startup_command_logs_frame((120, 34), 3, 0);
        let (body, border_column) = startup_command_logs_rects(&app);
        assert!(body.height > 3, "fixture must fit");
        assert_eq!(marker_in(&buf, border_column), None);
    }

    /// Open the Change Theme picker with `count` synthetic themes and `selected`
    /// highlighted.
    fn change_theme_frame(
        size: (u16, u16),
        count: usize,
        selected: usize,
    ) -> (App, ratatui::buffer::Buffer) {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let options = (0..count)
            .map(|i| crate::theme::ThemeListing {
                id: format!("theme-{i}"),
                display_name: format!("Theme {i}"),
                source: crate::theme::ThemeSource::Bundled,
            })
            .collect::<Vec<_>>();
        app.prompt = PromptState::ChangeTheme(crate::app::ChangeThemePrompt {
            options,
            selected,
            current: "theme-0".to_string(),
        });
        let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let buf = terminal.backend().buffer().clone();
        (app, buf)
    }

    /// The theme list's inner rect and the border column beside it.
    fn change_theme_rects(app: &App) -> (Rect, Rect, usize) {
        match app.overlay_layout.active {
            OverlayMouseLayout::ChangeTheme { list, items, .. } => (
                list,
                Rect::new(list.x + list.width, list.y, 1, list.height),
                items,
            ),
            ref other => panic!("expected the change theme layout, got {other:?}"),
        }
    }

    #[test]
    fn the_change_theme_marker_tracks_the_item_offset() {
        // The theme picker is a LIST: its `ListState` offset counts whole items
        // and never clips the top one, so items is the unit the marker gets.
        let (app, buf) = change_theme_frame((90, 30), 60, 0);
        let (list, border_column, items) = change_theme_rects(&app);
        assert!(
            items > list.height as usize + 1,
            "the picker must overflow: {items} themes in {} rows",
            list.height
        );
        assert_eq!(marker_in(&buf, border_column).as_deref(), Some("↓"));
        assert_eq!(
            marker_in(&buf, list),
            None,
            "a marker inside the list would sit on a theme's own row"
        );

        let (app, buf) = change_theme_frame((90, 30), 60, list.height as usize);
        let (_, border_column, _) = change_theme_rects(&app);
        assert_eq!(marker_in(&buf, border_column).as_deref(), Some("↕"));

        let (app, buf) = change_theme_frame((90, 30), 60, items - 1);
        let (_, border_column, _) = change_theme_rects(&app);
        assert_eq!(marker_in(&buf, border_column).as_deref(), Some("↑"));
    }

    #[test]
    fn a_short_theme_list_gets_no_marker() {
        let (app, buf) = change_theme_frame((90, 30), 3, 0);
        let (list, border_column, items) = change_theme_rects(&app);
        assert!(
            items > 0 && items <= list.height as usize,
            "fixture must fit: {items} themes in {} rows",
            list.height
        );
        assert_eq!(marker_in(&buf, border_column), None);
    }

    /// Open the project browser with `count` synthetic directory entries and
    /// `selected` highlighted. `filter` non-empty exercises the variant that
    /// splits a filter input off the top of the modal.
    fn browse_projects_frame(
        size: (u16, u16),
        count: usize,
        selected: usize,
        filter: &str,
    ) -> (App, ratatui::buffer::Buffer) {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut app = test_app(default_bindings());
        let root = std::path::PathBuf::from(&app.engine.projects[0].path);
        let entries = (0..count)
            .map(|i| crate::app::BrowserEntry {
                path: root.join(format!("dir-{i}")),
                label: format!("dir-{i}/"),
                is_git_repo: false,
                is_parent: false,
            })
            .collect::<Vec<_>>();
        let mut input = crate::app::text_input::TextInput::new();
        for ch in filter.chars() {
            input.insert_char(ch);
        }
        app.prompt = PromptState::BrowseProjects {
            purpose: crate::app::BrowsePurpose::AddProject,
            current_dir: root,
            entries,
            loading: false,
            selected,
            filter: input,
            searching: false,
            editing_path: false,
            path_input: crate::app::text_input::TextInput::new(),
            tab_completions: Vec::new(),
            tab_index: 0,
        };
        let mut terminal = Terminal::new(TestBackend::new(size.0, size.1)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let buf = terminal.backend().buffer().clone();
        (app, buf)
    }

    /// The directory list's inner rect, the border column beside it, and the
    /// item count the renderer recorded.
    fn browse_projects_rects(app: &App) -> (Rect, Rect, usize) {
        match app.overlay_layout.active {
            OverlayMouseLayout::BrowseProjects { list, items, .. } => (
                list,
                Rect::new(list.x + list.width, list.y, 1, list.height),
                items,
            ),
            ref other => panic!("expected the browse projects layout, got {other:?}"),
        }
    }

    #[test]
    fn the_browse_projects_marker_tracks_the_item_offset() {
        // A directory listing is a LIST: item units, not rows of wrapped text.
        // Both layout variants are covered — no filter (one full-height list) and
        // a filter typed (an input strip plus a shorter list).
        for filter in ["", "dir-"] {
            let (app, buf) = browse_projects_frame((90, 30), 60, 0, filter);
            let (list, border_column, items) = browse_projects_rects(&app);
            assert!(
                items > list.height as usize + 1,
                "the browser must overflow (filter {filter:?}): {items} entries in {} rows",
                list.height
            );
            assert_eq!(
                marker_in(&buf, border_column).as_deref(),
                Some("↓"),
                "at the top (filter {filter:?})"
            );
            assert_eq!(
                marker_in(&buf, list),
                None,
                "a marker inside the list would sit on a directory's own row"
            );

            let (app, buf) = browse_projects_frame((90, 30), 60, list.height as usize, filter);
            let (_, border_column, _) = browse_projects_rects(&app);
            assert_eq!(
                marker_in(&buf, border_column).as_deref(),
                Some("↕"),
                "in the middle (filter {filter:?})"
            );

            let (app, buf) = browse_projects_frame((90, 30), 60, items - 1, filter);
            let (_, border_column, _) = browse_projects_rects(&app);
            assert_eq!(
                marker_in(&buf, border_column).as_deref(),
                Some("↑"),
                "at the bottom (filter {filter:?})"
            );
        }
    }

    #[test]
    fn a_short_directory_listing_gets_no_marker() {
        for filter in ["", "dir-"] {
            let (app, buf) = browse_projects_frame((90, 30), 3, 0, filter);
            let (list, border_column, items) = browse_projects_rects(&app);
            assert!(
                items > 0 && items <= list.height as usize,
                "fixture must fit: {items} entries in {} rows",
                list.height
            );
            assert_eq!(marker_in(&buf, border_column), None);
        }
    }

    // ------------------------------------------------------------------
    // Footer hints for the two modals that pair a text field with checkboxes.
    //
    // `Action::ToggleSelection` is bound by default to `h`/`l`/`Left`/`Right`
    // as well as `Tab`/`Shift-Tab`, and `label_for` names the FIRST key. In the
    // rename and new-agent-name modals the field owns the letters and the
    // horizontal arrows, so those keys never reach the toggle there: the hint
    // must name the first key that is actually reachable.
    // ------------------------------------------------------------------

    /// Full screen text of a rendered frame, one line per row.
    fn rendered_screen(app: &mut App) -> String {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let buf = terminal.backend().buffer();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Style of the checkbox marker cell (`[`) on the row whose text contains
    /// `label`, from a real rendered frame.
    ///
    /// Panics when the label is not on screen: a focus-indication test that
    /// silently compared nothing would be worthless.
    fn checkbox_marker_style(app: &mut App, label: &str) -> Style {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let buf = terminal.backend().buffer();
        for y in 0..buf.area.height {
            let row: String = (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect();
            if !row.contains(label) {
                continue;
            }
            let marker = row.find('[').expect("a checkbox row carries a marker");
            let cell = &buf[(marker as u16, y)];
            return Style::default()
                .fg(cell.fg)
                .bg(cell.bg)
                .add_modifier(cell.modifier);
        }
        panic!("checkbox labelled {label:?} was not rendered");
    }

    #[test]
    fn rename_focused_checkbox_renders_differently_from_the_unfocused_one() {
        const LABEL: &str = "Also rename the git branch";
        let mut app = test_app(default_bindings());

        app.prompt = rename_prompt();
        let unfocused = checkbox_marker_style(&mut app, LABEL);

        app.prompt = match rename_prompt() {
            PromptState::RenameSession {
                session_id,
                input,
                rename_branch,
                ..
            } => PromptState::RenameSession {
                session_id,
                input,
                rename_branch,
                focus: RenameSessionFocus::RenameBranchCheckbox,
                branch_named: true,
            },
            other => panic!("expected RenameSession, got {other:?}"),
        };
        let focused = checkbox_marker_style(&mut app, LABEL);

        assert_ne!(
            focused, unfocused,
            "a focused checkbox must look focused; moving focus somewhere \
             invisible is the bug this guards"
        );
    }

    #[test]
    fn name_new_agent_focused_checkboxes_render_differently_from_the_unfocused_ones() {
        const RANDOMIZED: &str = "Use randomized pet name";
        const COPY: &str = "Copy uncommitted changes";
        let mut app = test_app(default_bindings());

        let with_focus = |app: &App, focus: NameNewAgentFocus| match name_new_agent_prompt(app) {
            PromptState::NameNewAgent {
                request,
                input,
                randomize_name,
                randomized_name,
                copy_changes,
                ..
            } => PromptState::NameNewAgent {
                request,
                input,
                randomize_name,
                randomized_name,
                copy_changes,
                focus,
            },
            other => panic!("expected NameNewAgent, got {other:?}"),
        };

        app.prompt = with_focus(&app, NameNewAgentFocus::Input);
        let randomized_unfocused = checkbox_marker_style(&mut app, RANDOMIZED);
        let copy_unfocused = checkbox_marker_style(&mut app, COPY);

        app.prompt = with_focus(&app, NameNewAgentFocus::RandomizedNameCheckbox);
        assert_ne!(
            checkbox_marker_style(&mut app, RANDOMIZED),
            randomized_unfocused,
            "the focused randomized-name checkbox must look focused"
        );
        assert_eq!(
            checkbox_marker_style(&mut app, COPY),
            copy_unfocused,
            "only the focused checkbox changes"
        );

        app.prompt = with_focus(&app, NameNewAgentFocus::CopyChangesCheckbox);
        assert_ne!(
            checkbox_marker_style(&mut app, COPY),
            copy_unfocused,
            "the focused copy-changes checkbox must look focused"
        );
    }

    // ------------------------------------------------------------------
    // Overlay text fields must LOOK focused. These assert on drawn cells, not
    // on state: the bug they guard drew the focused border from
    // `border_focused` and the unfocused one from `overlay_border`, which the
    // state was perfectly happy about and which resolves to a single colour in
    // every loadable theme (see
    // `theme::tests::the_overlay_field_focus_is_visible_in_every_loadable_theme`).
    // ------------------------------------------------------------------

    /// Style of a cell in a rendered frame, folding in the modifier so a BOLD
    /// border is not mistaken for a plain one.
    fn cell_style(buf: &ratatui::buffer::Buffer, x: u16, y: u16) -> Style {
        let cell = &buf[(x, y)];
        Style::default()
            .fg(cell.fg)
            .bg(cell.bg)
            .add_modifier(cell.modifier)
    }

    /// Style of the top-left corner of the startup-command/env modal's text
    /// field frame, from a real rendered frame.
    ///
    /// The field rect comes from the mouse hit-test layout the renderer
    /// publishes, so the test reads the border of the very box the user clicks
    /// into rather than guessing at layout math.
    fn startup_command_field_corner_style(app: &mut App) -> Style {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let OverlayMouseLayout::ConfigureStartupCommand { input, .. } = app.overlay_layout.active
        else {
            panic!("the modal must publish its text field's hit-test rect");
        };
        let buf = terminal.backend().buffer();
        assert_eq!(
            buf[(input.x - 1, input.y - 1)].symbol(),
            "╭",
            "expected the text field's top-left border corner"
        );
        cell_style(buf, input.x - 1, input.y - 1)
    }

    // ── The three configure modals, now that they are dual-mode forms ───────
    //
    // FOCUS and ENGAGEMENT are two different things and must render as two
    // different things: a focused-but-unengaged full-text field takes no
    // keystrokes, so it gets the focus border, NO caret, and a footer naming
    // the key that starts editing.

    /// One of the three configure modals, with focus wherever the caller wants.
    fn configure_modal_prompt(which: &str, text: &str, focus: ConfigureFieldFocus) -> PromptState {
        let field = || {
            TextInput::with_text(text.to_string())
                .with_multiline(6)
                .with_placeholder("Enter startup command...")
        };
        match which {
            "ConfigureStartupCommand" => PromptState::ConfigureStartupCommand {
                project_id: "p1".to_string(),
                project_name: "demo".to_string(),
                input: field(),
                focus,
            },
            "ConfigureProjectEnv" => PromptState::ConfigureProjectEnv {
                project_id: "p1".to_string(),
                project_name: "demo".to_string(),
                input: field(),
                focus,
            },
            "ConfigureGlobalEnv" => PromptState::ConfigureGlobalEnv {
                project_name: "demo".to_string(),
                input: field(),
                focus,
            },
            other => panic!("unknown configure modal {other}"),
        }
    }

    const CONFIGURE_MODAL_NAMES: [&str; 3] = [
        "ConfigureStartupCommand",
        "ConfigureProjectEnv",
        "ConfigureGlobalEnv",
    ];

    /// Render and return the whole frame's text, one row per line.
    fn rendered_rows(app: &mut App) -> Vec<String> {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let buf = terminal.backend().buffer();
        (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn startup_command_and_env_modals_render_their_focused_field_as_focused() {
        for name in CONFIGURE_MODAL_NAMES {
            let mut app = test_app(default_bindings());

            app.prompt = configure_modal_prompt(name, "cargo build", ConfigureFieldFocus::Cancel);
            app.input_target = InputTarget::None;
            let unfocused = startup_command_field_corner_style(&mut app);

            app.prompt = configure_modal_prompt(name, "cargo build", ConfigureFieldFocus::Input);
            let focused = startup_command_field_corner_style(&mut app);

            assert_ne!(
                focused, unfocused,
                "{name}: the focused text field must render as focused; \
                 focus you cannot see is not focus"
            );
            assert_ne!(
                focused.fg, unfocused.fg,
                "{name}: the focused field must differ in colour, not only in weight"
            );
        }
    }

    #[test]
    fn a_configure_modal_shows_engaged_apart_from_focused_and_says_how_to_start() {
        for name in CONFIGURE_MODAL_NAMES {
            let mut app = test_app(default_bindings());
            let engage_key = app.bindings.label_for(Action::EngageCommitInput);

            app.prompt = configure_modal_prompt(name, "cargo build", ConfigureFieldFocus::Input);
            app.input_target = InputTarget::None;
            let unengaged = rendered_rows(&mut app).join("\n");

            app.input_target = InputTarget::StartupCommand;
            let engaged = rendered_rows(&mut app).join("\n");

            assert!(
                !unengaged.contains("editing:"),
                "{name}: a field that takes no keystrokes must not claim to be editing"
            );
            assert!(
                engaged.contains("editing:"),
                "{name}: the engaged field must say so, or it looks like the focused one"
            );
            assert!(
                unengaged.contains(&engage_key) && unengaged.contains("edit text"),
                "{name}: an unengaged field must name the key that starts editing; \
                 looked for {engage_key:?} and \"edit text\""
            );
        }
    }

    /// The clear key empties the FOCUSED full-text field, so the footer may
    /// only name it while that field has focus. Naming it from a button stop
    /// would promise a key nothing answers, which is the class of bug the
    /// focus gate on `Action::ClearTextField` was added to close.
    #[test]
    fn a_configure_modal_names_the_clear_key_only_while_the_body_has_focus() {
        let clear_key = {
            let app = test_app(default_bindings());
            app.bindings.label_for(Action::ClearTextField)
        };
        for name in CONFIGURE_MODAL_NAMES {
            let mut app = test_app(default_bindings());
            app.prompt = configure_modal_prompt(name, "cargo build", ConfigureFieldFocus::Input);
            app.input_target = InputTarget::None;
            let on_body = rendered_rows(&mut app).join("\n");
            assert!(
                on_body.contains(&clear_key) && on_body.contains("clear"),
                "{name}: the focused body must name the clear key"
            );

            for focus in [ConfigureFieldFocus::Cancel, ConfigureFieldFocus::Save] {
                app.prompt = configure_modal_prompt(name, "cargo build", focus);
                let on_button = rendered_rows(&mut app).join("\n");
                assert!(
                    !on_button.contains("clear"),
                    "{name}: {focus:?} does not answer the clear key, so the footer \
                     must not name it"
                );
            }
        }
    }

    #[test]
    fn a_configure_modal_publishes_a_confirm_button() {
        for name in CONFIGURE_MODAL_NAMES {
            let mut app = test_app(default_bindings());
            app.prompt = configure_modal_prompt(name, "cargo build", ConfigureFieldFocus::Input);
            let _ = rendered_rows(&mut app);
            assert!(
                super::super::modal::layout_publishes_confirm_button(&app.overlay_layout.active),
                "{name}: a modal with a full-text field must publish a confirm button"
            );
            let rows = rendered_rows(&mut app).join("\n");
            assert!(rows.contains("Save"), "{name}: the Save button must render");
            assert!(
                rows.contains("Cancel"),
                "{name}: the Cancel button must render"
            );
        }
    }

    #[test]
    fn a_configure_modal_marks_a_body_that_overflows_its_pane() {
        use super::super::components::{MARKER_GLYPHS, scroll_marker_rect};

        for name in CONFIGURE_MODAL_NAMES {
            let short = {
                let mut app = test_app(default_bindings());
                app.prompt = configure_modal_prompt(name, "one line", ConfigureFieldFocus::Input);
                let rows = rendered_rows(&mut app);
                let OverlayMouseLayout::ConfigureStartupCommand { input, .. } =
                    app.overlay_layout.active
                else {
                    panic!("{name}: the modal must publish its field rect");
                };
                // The field's outer rect is its inner rect plus the border ring.
                let area = Rect::new(input.x - 1, input.y - 1, input.width + 2, input.height + 2);
                let cell = scroll_marker_rect(area, input);
                rows[cell.y as usize]
                    .chars()
                    .nth(cell.x as usize)
                    .expect("marker cell")
            };
            assert!(
                !MARKER_GLYPHS.contains(&short.to_string().as_str()),
                "{name}: a body that fits must not claim it can scroll, got {short:?}"
            );

            let long = {
                let body = (0..40)
                    .map(|n| format!("KEY{n}=value"))
                    .collect::<Vec<_>>()
                    .join("\n");
                let mut app = test_app(default_bindings());
                app.prompt = configure_modal_prompt(name, &body, ConfigureFieldFocus::Input);
                let rows = rendered_rows(&mut app);
                let OverlayMouseLayout::ConfigureStartupCommand { input, .. } =
                    app.overlay_layout.active
                else {
                    panic!("{name}: the modal must publish its field rect");
                };
                let area = Rect::new(input.x - 1, input.y - 1, input.width + 2, input.height + 2);
                let cell = scroll_marker_rect(area, input);
                rows[cell.y as usize]
                    .chars()
                    .nth(cell.x as usize)
                    .expect("marker cell")
            };
            assert!(
                MARKER_GLYPHS.contains(&long.to_string().as_str()),
                "{name}: an overflowing body must carry the shared scroll marker, got {long:?}"
            );
        }
    }

    /// Style of the startup-log modal's filter-box top-left corner. The box
    /// carries a " Filter " title on its top border row, which is what locates
    /// it in the buffer.
    fn startup_log_filter_corner_style(app: &mut App) -> Style {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let buf = terminal.backend().buffer();
        for y in 0..buf.area.height {
            let row: String = (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect();
            if !row.contains(" Filter ") {
                continue;
            }
            let corner = row.find('╭').expect("the filter box has a top-left corner");
            let x = row
                .char_indices()
                .position(|(byte, _)| byte == corner)
                .expect("corner column") as u16;
            return cell_style(buf, x, y);
        }
        panic!("the filter box was not rendered");
    }

    #[test]
    fn startup_log_filter_box_renders_as_focused_while_searching() {
        let log_prompt = |searching: bool| {
            PromptState::StartupCommandLogs(StartupCommandLogPrompt {
                scope_label: "demo".to_string(),
                entries: Vec::new(),
                selected: 0,
                // A non-empty filter keeps the box on screen while NOT
                // searching, which is the unfocused presentation.
                filter: TextInput::with_text("boot".to_string()),
                searching,
                content: String::new(),
                scroll_offset: 0,
                wrap_width: 0,
                focus: StartupCommandLogFocus::List,
            })
        };

        let mut app = test_app(default_bindings());
        app.prompt = log_prompt(false);
        let unfocused = startup_log_filter_corner_style(&mut app);

        app.prompt = log_prompt(true);
        let focused = startup_log_filter_corner_style(&mut app);

        assert_ne!(
            focused, unfocused,
            "the filter box must render as focused while it is taking keystrokes"
        );
        assert_ne!(
            focused.fg, unfocused.fg,
            "the focused filter box must differ in colour, not only in weight"
        );
    }

    /// Bindings with every default key list except `action`, which gets `keys`.
    fn bindings_with(action: Action, keys: Vec<crokey::KeyCombination>) -> RuntimeBindings {
        RuntimeBindings::new(
            move |a| {
                if a == action {
                    keys.clone()
                } else {
                    crate::keybindings::BINDING_DEFS
                        .iter()
                        .find(|d| d.action == a)
                        .map(|d| d.default_keys.to_vec())
                        .unwrap_or_default()
                }
            },
            true,
        )
    }

    /// A single-code key combination as the key event a user would produce.
    fn press(key: crokey::KeyCombination) -> KeyEvent {
        let crokey::OneToThree::One(code) = key.codes else {
            panic!("test fixtures use single-code combinations only");
        };
        KeyEvent::new(code, key.modifiers)
    }

    /// The toggle key the two text-field modals actually route to the toggle.
    fn reachable_toggle(app: &App) -> Option<crokey::KeyCombination> {
        app.bindings
            .first_key_reaching(Action::ToggleSelection, |k| !text_field_owns_key(k))
    }

    fn rename_prompt() -> PromptState {
        PromptState::RenameSession {
            session_id: "session-1".to_string(),
            input: TextInput::with_text("agent".to_string()),
            rename_branch: false,
            focus: RenameSessionFocus::Input,
            branch_named: true,
        }
    }

    fn name_new_agent_prompt(app: &App) -> PromptState {
        PromptState::NameNewAgent {
            request: CreateAgentRequest::NewProject {
                project: app.engine.projects[0].clone(),
                custom_name: None,
                use_existing_branch: false,
                pull_before_create: false,
                copy_uncommitted_changes: false,
            },
            input: TextInput::with_text("agent".to_string()),
            randomize_name: false,
            randomized_name: None,
            copy_changes: false,
            focus: NameNewAgentFocus::Input,
        }
    }

    #[test]
    fn rename_footer_names_a_key_that_really_moves_focus() {
        let mut app = test_app(default_bindings());
        app.prompt = rename_prompt();

        let label = app
            .bindings
            .label_for_text_field_dialog(Action::ToggleSelection)
            .expect("default bindings leave a reachable focus key");
        assert_ne!(
            label, "h",
            "`h` types into the name field here; it must never be the advertised focus key"
        );

        let screen = rendered_screen(&mut app);
        assert!(
            screen.contains(&format!("<{label}> focus")),
            "rename footer should advertise <{label}>; got:\n{screen}"
        );
        assert!(
            !screen.contains("<h> focus"),
            "rename footer must not advertise `h`:\n{screen}"
        );
        assert!(
            !screen.contains("Space toggle"),
            "with the NAME FIELD focused Space is a typed character, not a \
             toggle; see `the_space_hint_only_appears_when_space_acts_on_something`:\n{screen}"
        );

        // The label is tied to behaviour: the advertised key moves focus, and
        // leaves the checkbox value alone.
        let key = reachable_toggle(&app).expect("reachable toggle key");
        app.handle_key(press(key)).expect("handle key");
        let screen = rendered_screen(&mut app);
        assert!(
            screen.contains("Space toggle"),
            "once focus is on the checkbox the footer says Space toggles it:\n{screen}"
        );
        match &app.prompt {
            PromptState::RenameSession {
                focus,
                rename_branch,
                ..
            } => {
                assert_eq!(
                    *focus,
                    RenameSessionFocus::RenameBranchCheckbox,
                    "the key the footer names must move focus onto the checkbox"
                );
                assert!(!*rename_branch, "moving focus must not change the value");
            }
            other => panic!("expected RenameSession, got {other:?}"),
        }
    }

    #[test]
    fn name_new_agent_footer_names_a_key_that_really_moves_focus() {
        let mut app = test_app(default_bindings());
        app.prompt = name_new_agent_prompt(&app);

        let label = app
            .bindings
            .label_for_text_field_dialog(Action::ToggleSelection)
            .expect("default bindings leave a reachable toggle key");
        assert_ne!(
            label, "h",
            "`h` types into the name field here; it must never be the advertised focus key"
        );

        let screen = rendered_screen(&mut app);
        assert!(
            screen.contains(&format!("<{label}> focus")),
            "new-agent footer should advertise <{label}>; got:\n{screen}"
        );
        assert!(
            !screen.contains("<h> focus"),
            "new-agent footer must not advertise `h`:\n{screen}"
        );

        let key = reachable_toggle(&app).expect("reachable toggle key");
        app.handle_key(press(key)).expect("handle key");
        match &app.prompt {
            PromptState::NameNewAgent { focus, .. } => assert_ne!(
                *focus,
                NameNewAgentFocus::Input,
                "the key the footer names must move focus off the name field"
            ),
            other => panic!("expected NameNewAgent, got {other:?}"),
        }
    }

    #[test]
    fn rebinding_past_a_suppressed_first_key_moves_the_hint_to_the_reachable_one() {
        // `Left` is owned by the caret in this modal, so the hint has to skip
        // it and name `F2`, the first key that still reaches the toggle.
        let keys = vec![
            crokey::KeyCombination::one_key(KeyCode::Left, KeyModifiers::NONE),
            crokey::KeyCombination::one_key(KeyCode::F(2), KeyModifiers::NONE),
        ];
        let mut app = test_app(bindings_with(Action::ToggleSelection, keys));
        app.prompt = rename_prompt();

        let label = app
            .bindings
            .label_for_text_field_dialog(Action::ToggleSelection)
            .expect("F2 still reaches focus movement");
        assert_ne!(label, "Left", "the caret owns Left in this modal");

        let screen = rendered_screen(&mut app);
        assert!(
            screen.contains(&format!("<{label}> focus")),
            "footer should advertise the reachable <{label}>; got:\n{screen}"
        );

        app.handle_key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE))
            .expect("handle key");
        match &app.prompt {
            PromptState::RenameSession { focus, .. } => assert_eq!(
                *focus,
                RenameSessionFocus::RenameBranchCheckbox,
                "the advertised F2 must move focus"
            ),
            other => panic!("expected RenameSession, got {other:?}"),
        }
    }

    #[test]
    fn a_fully_suppressed_toggle_binding_drops_the_hint_instead_of_lying() {
        // Every key of the action is owned by the text field here, so there is
        // no honest key to name: the segment is dropped rather than naming a
        // key that types a letter or rendering an empty badge.
        let keys = vec![
            crokey::KeyCombination::one_key(KeyCode::Char('h'), KeyModifiers::NONE),
            crokey::KeyCombination::one_key(KeyCode::Char('l'), KeyModifiers::NONE),
        ];
        let mut app = test_app(bindings_with(Action::ToggleSelection, keys));
        assert!(
            app.bindings
                .label_for_text_field_dialog(Action::ToggleSelection)
                .is_none(),
            "no key of the action reaches the toggle in this modal"
        );

        for prompt in [rename_prompt(), name_new_agent_prompt(&app)] {
            app.prompt = prompt;
            let screen = rendered_screen(&mut app);
            assert!(
                !screen.contains("toggle") || !screen.contains("<h>"),
                "must not advertise the letter that types:\n{screen}"
            );
            assert!(
                !screen.contains("<> "),
                "must not render an empty key badge:\n{screen}"
            );
            assert!(
                !screen.contains("<h> focus") && !screen.contains("<l> focus"),
                "must not advertise a typing letter as the focus key:\n{screen}"
            );
            // The rest of the footer survives.
            assert!(
                screen.contains("confirm") && screen.contains("cancel"),
                "the remaining hints must still render:\n{screen}"
            );
        }
    }

    /// The text-field dialogs suppress the typing keys when they pick a focus
    /// hint. That suppression must stay LOCAL to them: the shared label the
    /// rest of the app renders is still the action's real first key.
    ///
    /// This used to be checked by rendering the provider pickers, which
    /// advertised the unsuppressed `<h>` for "move to the buttons". They have
    /// no buttons now, so there is nothing left to render that would show it,
    /// and the claim is asserted against the bindings directly instead.
    #[test]
    fn suppressing_the_focus_key_in_a_text_field_dialog_leaves_the_shared_label_alone() {
        let app = test_app(default_bindings());
        assert_eq!(
            app.bindings.label_for(Action::ToggleSelection),
            "h",
            "default first key of the shared action"
        );
        let in_text_field_dialog = app
            .bindings
            .label_for_text_field_dialog(Action::ToggleSelection)
            .expect("some key of the action still reaches focus movement");
        assert_ne!(
            in_text_field_dialog, "h",
            "a dialog with a text field must not advertise a key that types"
        );
    }

    /// The provider pickers are the counterpart to the test above: they have no
    /// buttons for focus to move between, so naming a focus key would be a
    /// hint pointing at nothing. Their footers advertise move / choose / cancel
    /// and each of those keys is resolved through the bindings.
    #[test]
    fn the_provider_pickers_advertise_only_the_keys_they_answer() {
        let mut app = test_app(default_bindings());
        let project = app.engine.projects[0].clone();
        let prompts = [
            (
                "ChangeAgentProvider",
                PromptState::ChangeAgentProvider(agent_provider_prompt()),
            ),
            (
                "ChangeDefaultProvider",
                PromptState::ChangeDefaultProvider(default_provider_prompt()),
            ),
            (
                "ChangeProjectDefaultProvider",
                PromptState::ChangeProjectDefaultProvider(project_default_provider_prompt(
                    project.id.clone(),
                    project.name.clone(),
                )),
            ),
        ];
        for (name, prompt) in prompts {
            app.prompt = prompt;
            let screen = rendered_screen(&mut app);
            assert!(
                !screen.contains("buttons"),
                "{name}: there are no buttons left to advertise:\n{screen}"
            );
            for (action, word) in [
                (Action::MoveDown, "move"),
                (Action::Confirm, "choose"),
                (Action::CloseOverlay, "cancel"),
            ] {
                let key = app.bindings.label_for(action);
                assert!(
                    screen.contains(&format!("<{key}>")) && screen.contains(word),
                    "{name}: the footer must name the bound key for {word:?}, \
                     expected <{key}>:\n{screen}"
                );
            }
        }
    }

    /// The diff pane's outer rect, reconstructed from the content rect it
    /// recorded: the block's border ring is one cell on each side.
    fn centered_diff_pane_area(app: &App) -> Rect {
        let content = app.mouse_layout.agent_term.expect("selection surface");
        // The content pane is the block's inner area minus the two hint rows at
        // the bottom, so the outer pane is one cell out on the left/top and two
        // rows taller at the bottom plus the border.
        Rect::new(
            content.x - 1,
            content.y - 1,
            content.width + 2,
            content.height + 2 + 2,
        )
    }

    // ── Single-line modal text fields: UTF-8 safety and honest focus ──────
    //
    // Three modals used to hand-roll their caret with a BYTE split, so a caret
    // sitting before a multi-byte character panicked, and they drew the caret
    // unconditionally, so the field looked focused while a checkbox had focus.
    // These tests pin both, against the drawn buffer.

    /// A name with a two-byte character and a four-byte one, so a byte-based
    /// split has two different ways to land inside a character.
    const MULTIBYTE_NAME: &str = "café 🙂 x";

    /// Every valid caret byte offset in `text`, including the end.
    fn caret_positions(text: &str) -> Vec<usize> {
        text.char_indices()
            .map(|(i, _)| i)
            .chain(std::iter::once(text.len()))
            .collect()
    }

    fn draw(app: &mut App) -> ratatui::buffer::Buffer {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;

        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        terminal.backend().buffer().clone()
    }

    /// Columns of `row` inside `inner` painted with the caret background.
    fn caret_columns(buf: &ratatui::buffer::Buffer, inner: Rect, cursor_bg: Color) -> Vec<u16> {
        (inner.x..inner.x + inner.width)
            .filter(|x| buf[(*x, inner.y)].style().bg == Some(cursor_bg))
            .collect()
    }

    /// The foreground colour and modifiers of the field frame's left border
    /// cell, derived from the published INNER rect (never from a string index,
    /// which would read the wrong cell once box drawing is in play). The cell's
    /// background comes from the modal underneath, so it is not compared.
    fn field_border_look(buf: &ratatui::buffer::Buffer, inner: Rect) -> (Option<Color>, Modifier) {
        let style = buf[(inner.x - 1, inner.y)].style();
        (style.fg, style.add_modifier)
    }

    fn expected_border_look(theme: &Theme, focused: bool) -> (Option<Color>, Modifier) {
        let style = theme.overlay_field_border_style(focused);
        (style.fg, style.add_modifier)
    }

    fn multibyte_rename_prompt(app: &App, cursor: usize, focus: RenameSessionFocus) -> PromptState {
        let mut input = TextInput::with_text(MULTIBYTE_NAME.to_string());
        input.cursor = cursor;
        PromptState::RenameSession {
            session_id: app.engine.sessions[0].id.clone(),
            input,
            rename_branch: false,
            focus,
            branch_named: true,
        }
    }

    fn multibyte_name_new_agent_prompt(
        app: &App,
        cursor: usize,
        focus: NameNewAgentFocus,
    ) -> PromptState {
        let mut input = TextInput::with_text(MULTIBYTE_NAME.to_string());
        input.cursor = cursor;
        PromptState::NameNewAgent {
            request: CreateAgentRequest::NewProject {
                project: app.engine.projects[0].clone(),
                custom_name: None,
                use_existing_branch: false,
                pull_before_create: false,
                copy_uncommitted_changes: false,
            },
            input,
            randomize_name: false,
            randomized_name: None,
            copy_changes: false,
            focus,
        }
    }

    fn multibyte_pull_request_prompt(app: &App, cursor: usize) -> PromptState {
        let mut input = TextInput::with_text(MULTIBYTE_NAME.to_string());
        input.cursor = cursor;
        PromptState::PullRequestInput {
            project: Some(app.engine.projects[0].clone()),
            input,
            focus: PullRequestInputFocus::Input,
        }
    }

    #[test]
    fn rename_modal_survives_a_caret_before_every_multibyte_character() {
        for cursor in caret_positions(MULTIBYTE_NAME) {
            let mut app = test_app(default_bindings());
            app.prompt = multibyte_rename_prompt(&app, cursor, RenameSessionFocus::Input);
            draw(&mut app);
        }
    }

    #[test]
    fn name_new_agent_modal_survives_a_caret_before_every_multibyte_character() {
        for cursor in caret_positions(MULTIBYTE_NAME) {
            let mut app = test_app(default_bindings());
            app.prompt = multibyte_name_new_agent_prompt(&app, cursor, NameNewAgentFocus::Input);
            draw(&mut app);
        }
    }

    #[test]
    fn pull_request_modal_survives_a_caret_before_every_multibyte_character() {
        for cursor in caret_positions(MULTIBYTE_NAME) {
            let mut app = test_app(default_bindings());
            app.prompt = multibyte_pull_request_prompt(&app, cursor);
            draw(&mut app);
        }
    }

    #[test]
    fn macro_list_preview_survives_every_truncation_boundary() {
        // The list width is fixed by the popup, so the truncation point is
        // swept by growing the NAME, which shrinks the room left for the
        // preview one column at a time and walks it across multi-byte chars.
        let text = "áéíóú 🙂🙃🙁 ñ";
        for name_len in 1..40usize {
            let mut app = test_app(default_bindings());
            app.prompt = PromptState::EditMacros {
                entries: vec![("n".repeat(name_len), text.to_string(), MacroSurface::Agent)],
                selected: 0,
                editing: None,
                pending_delete: None,
            };
            draw(&mut app);
        }
    }

    #[test]
    fn rename_field_shows_a_caret_only_while_it_has_focus() {
        let cursor_bg = {
            let app = test_app(default_bindings());
            app.theme.input_cursor_bg
        };

        // Focus on the field: caret drawn, border in the focused style.
        let mut app = test_app(default_bindings());
        app.prompt = multibyte_rename_prompt(&app, 2, RenameSessionFocus::Input);
        let buf = draw(&mut app);
        let OverlayMouseLayout::RenameSession { input, .. } = app.overlay_layout.active else {
            panic!("rename modal must publish its input rect");
        };
        assert_eq!(
            caret_columns(&buf, input, cursor_bg).len(),
            1,
            "a focused field draws exactly one caret cell"
        );
        assert_eq!(
            field_border_look(&buf, input),
            expected_border_look(&app.theme, true),
            "a focused field draws the focused border"
        );

        // Focus on the checkbox: no caret, unfocused border.
        let mut app = test_app(default_bindings());
        app.prompt = multibyte_rename_prompt(&app, 2, RenameSessionFocus::RenameBranchCheckbox);
        let buf = draw(&mut app);
        let OverlayMouseLayout::RenameSession { input, .. } = app.overlay_layout.active else {
            panic!("rename modal must publish its input rect");
        };
        assert!(
            caret_columns(&buf, input, cursor_bg).is_empty(),
            "an unfocused field must draw no caret: focus you cannot see is not focus"
        );
        assert_eq!(
            field_border_look(&buf, input),
            expected_border_look(&app.theme, false),
            "an unfocused field draws the unfocused border"
        );
    }

    #[test]
    fn name_new_agent_field_shows_a_caret_only_while_it_has_focus() {
        let cursor_bg = {
            let app = test_app(default_bindings());
            app.theme.input_cursor_bg
        };

        let mut app = test_app(default_bindings());
        app.prompt = multibyte_name_new_agent_prompt(&app, 2, NameNewAgentFocus::Input);
        let buf = draw(&mut app);
        let OverlayMouseLayout::NameNewAgent { input, .. } = app.overlay_layout.active else {
            panic!("new-agent modal must publish its input rect");
        };
        assert_eq!(caret_columns(&buf, input, cursor_bg).len(), 1);
        assert_eq!(
            field_border_look(&buf, input),
            expected_border_look(&app.theme, true)
        );

        let mut app = test_app(default_bindings());
        app.prompt =
            multibyte_name_new_agent_prompt(&app, 2, NameNewAgentFocus::RandomizedNameCheckbox);
        let buf = draw(&mut app);
        let OverlayMouseLayout::NameNewAgent { input, .. } = app.overlay_layout.active else {
            panic!("new-agent modal must publish its input rect");
        };
        assert!(
            caret_columns(&buf, input, cursor_bg).is_empty(),
            "an unfocused field must draw no caret"
        );
        assert_eq!(
            field_border_look(&buf, input),
            expected_border_look(&app.theme, false)
        );
    }

    #[test]
    fn pull_request_modal_publishes_a_clickable_input_rect() {
        let mut app = test_app(default_bindings());
        app.prompt = multibyte_pull_request_prompt(&app, 0);
        let buf = draw(&mut app);

        let OverlayMouseLayout::PullRequestInput { input, .. } = app.overlay_layout.active else {
            panic!(
                "the PR modal must publish its input rect, got {:?}",
                app.overlay_layout.active
            );
        };
        assert!(input.width > 0 && input.height > 0);
        // It is the only control, so it is always focused.
        assert_eq!(
            caret_columns(&buf, input, app.theme.input_cursor_bg).len(),
            1
        );
    }

    fn multibyte_attach_pull_request_prompt(cursor: usize) -> PromptState {
        let mut input = TextInput::with_text(MULTIBYTE_NAME.to_string());
        input.cursor = cursor;
        PromptState::AttachPullRequestInput {
            session_id: "session-1".to_string(),
            current_pr: Some("#42 (open) Fix the frobnicator (manually attached)".to_string()),
            input,
        }
    }

    #[test]
    fn attach_pull_request_modal_survives_a_caret_before_every_multibyte_character() {
        for cursor in caret_positions(MULTIBYTE_NAME) {
            let mut app = test_app(default_bindings());
            app.prompt = multibyte_attach_pull_request_prompt(cursor);
            draw(&mut app);
        }
    }

    #[test]
    fn attach_pull_request_modal_publishes_a_clickable_input_rect() {
        // Both shapes publish: with a current-PR line in the body and without.
        for current_pr in [Some("#42 (open) Fix the frobnicator".to_string()), None] {
            let mut app = test_app(default_bindings());
            app.prompt = PromptState::AttachPullRequestInput {
                session_id: "session-1".to_string(),
                current_pr,
                input: TextInput::new(),
            };
            let buf = draw(&mut app);

            let OverlayMouseLayout::AttachPullRequestInput { input } = app.overlay_layout.active
            else {
                panic!(
                    "the attach modal must publish its input rect, got {:?}",
                    app.overlay_layout.active
                );
            };
            assert!(input.width > 0 && input.height > 0);
            // It is the only control, so it is always focused and draws a caret.
            assert_eq!(
                caret_columns(&buf, input, app.theme.input_cursor_bg).len(),
                1
            );
        }
    }

    #[test]
    fn truncate_macro_preview_never_splits_a_character() {
        let text = "áéíóú 🙂🙃🙁 ñ";
        for max_len in 0..=text.chars().count() + 4 {
            let out = truncate_macro_preview(text, max_len);
            assert!(
                out.chars().count() <= max_len.max(1),
                "max_len={max_len} produced {out:?}"
            );
        }
        assert_eq!(truncate_macro_preview("áé🙂", 10), "áé🙂");
        assert_eq!(truncate_macro_preview("áé🙂ñ", 3), "áé…");
    }

    #[test]
    fn render_single_line_cursor_input_drops_the_caret_when_unfocused() {
        let focused =
            render_single_line_cursor_input("", "macro", 2, Color::White, Color::Black, true);
        assert_eq!(focused.spans.len(), 4);

        let unfocused =
            render_single_line_cursor_input("", "macro", 2, Color::White, Color::Black, false);
        let text: String = unfocused
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(text, "macro");
        assert!(
            unfocused
                .spans
                .iter()
                .all(|s| s.style.bg != Some(Color::Black)),
            "an unfocused field paints no caret cell"
        );
    }

    #[test]
    fn render_single_line_cursor_input_clamps_a_caret_inside_a_character() {
        // A byte offset that is not a char boundary must not panic; it clamps
        // back to the start of the character it landed in.
        let line = render_single_line_cursor_input("", "áb", 1, Color::White, Color::Black, true);
        let text: String = line.spans.iter().map(|s| s.content.to_string()).collect();
        assert_eq!(text, "áb");
    }

    // ── Change A: the no-op cue lives on the row, not on a button ───────────

    /// Read one screen row of `rect` back out of a rendered buffer.
    fn row_text(buf: &ratatui::buffer::Buffer, rect: Rect, row: u16) -> String {
        (rect.x..rect.x + rect.width)
            .map(|x| buf[(x, rect.y + row)].symbol().to_string())
            .collect()
    }

    fn render_provider_picker(prompt: PromptState) -> (App, ratatui::buffer::Buffer) {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut app = test_app(default_bindings());
        app.prompt = prompt;
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let buf = terminal.backend().buffer().clone();
        (app, buf)
    }

    /// The Apply button used to grey out when the highlighted row was already
    /// the active provider. With the button gone that cue has to live on the
    /// row, and it has to survive the selection highlight (the cue matters
    /// exactly when that row IS highlighted), so it is a MARKER IN THE TEXT and
    /// not a style. Row 0 is the active provider in every fixture here.
    #[test]
    fn the_already_active_provider_row_is_marked_as_a_no_op() {
        let cases: Vec<(&str, PromptState)> = {
            let app = test_app(default_bindings());
            let project = app.engine.projects[0].clone();
            vec![
                (
                    "ChangeAgentProvider",
                    PromptState::ChangeAgentProvider(agent_provider_prompt()),
                ),
                (
                    "ChangeDefaultProvider",
                    PromptState::ChangeDefaultProvider(default_provider_prompt()),
                ),
                (
                    "ChangeProjectDefaultProvider",
                    PromptState::ChangeProjectDefaultProvider(project_default_provider_prompt(
                        project.id.clone(),
                        project.name.clone(),
                    )),
                ),
            ]
        };
        for (name, prompt) in cases {
            let (app, buf) = render_provider_picker(prompt);
            let list = match app.overlay_layout.active {
                OverlayMouseLayout::ChangeAgentProvider { list, .. }
                | OverlayMouseLayout::ChangeDefaultProvider { list, .. }
                | OverlayMouseLayout::ChangeProjectDefaultProvider { list, .. } => list,
                ref other => panic!("{name}: expected a provider picker layout, got {other:?}"),
            };
            let active = row_text(&buf, list, 0);
            let other = row_text(&buf, list, 1);
            assert!(
                active.contains(super::ACTIVE_PROVIDER_MARKER),
                "{name}: the already-active row must carry the no-op marker, got {active:?}"
            );
            assert!(
                !other.contains(super::ACTIVE_PROVIDER_MARKER),
                "{name}: a row that would really change the provider must not be marked, \
                 got {other:?}"
            );
        }
    }

    // ══════════════════════════════════════════════════════════════════════
    // Change A: the macro modals' footers and title
    //
    // `edit_macro_hints` was a second copy of `components::hint_bar::
    // modal_hint_line` that PADDED each segment with two trailing spaces
    // instead of JOINING with them, so its output was not byte-identical and
    // the list footer ended in whitespace. The list's four labels were also
    // hardcoded literals, and the editor's title carried an em-dash.
    // ══════════════════════════════════════════════════════════════════════

    fn macro_list_app(bindings: RuntimeBindings) -> App {
        let mut app = test_app(bindings);
        app.engine.config.macros.entries.insert(
            "greet".to_string(),
            crate::config::MacroEntry {
                text: "hello".to_string(),
                surface: crate::config::MacroSurface::Agent,
            },
        );
        app.open_edit_macros();
        app
    }

    fn render_to_buffer(app: &mut App) -> ratatui::buffer::Buffer {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal.draw(|frame| app.render(frame)).expect("render");
        terminal.backend().buffer().clone()
    }

    /// The macro modal's footer row, read out of the popup's own inner rect
    /// rather than off the whole terminal row (which carries the dimmed app
    /// behind it).
    fn macro_modal_footer(buf: &ratatui::buffer::Buffer) -> String {
        let popup = super::centered_rect_exact(
            super::MACRO_EDIT_POPUP.0,
            super::MACRO_EDIT_POPUP.1,
            buf.area,
        );
        let y = popup.y + popup.height - 2;
        ((popup.x + 1)..(popup.x + popup.width - 1))
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect::<String>()
    }

    /// The list footer is the shared component's output, verbatim: one leading
    /// space, two-space joins, and NO trailing pad.
    #[test]
    fn the_macro_list_footer_is_the_shared_hint_line() {
        let mut app = macro_list_app(default_bindings());
        let buf = render_to_buffer(&mut app);
        let expected = {
            let line = modal_hint_line(&app.theme, &app.macro_list_hints());
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>()
        };
        let footer = macro_modal_footer(&buf);
        assert!(
            footer.trim_end() == expected,
            "footer {:?} is not the shared hint line {:?}",
            footer.trim_end(),
            expected
        );
        assert!(
            !expected.ends_with(' '),
            "the shared hint line never pads: {expected:?}"
        );
        // The hand-rolled copy PADDED each segment with two trailing spaces
        // rather than joining with them, and those two cells were painted in
        // the description colour. The shared line leaves them untouched, so
        // the pad is visible in the buffer even though the text trims equal.
        let popup = super::centered_rect_exact(
            super::MACRO_EDIT_POPUP.0,
            super::MACRO_EDIT_POPUP.1,
            buf.area,
        );
        let y = popup.y + popup.height - 2;
        let last_text_col = popup.x + 1 + expected.chars().count() as u16;
        assert_ne!(
            buf[(last_text_col, y)].fg,
            app.theme.hint_desc_fg,
            "the footer still paints a trailing pad past its last segment"
        );
    }

    /// Rebinding any of the four macro-list keys moves its footer label.
    #[test]
    fn the_macro_list_footer_names_the_bound_keys() {
        let bindings = RuntimeBindings::new(
            |action| match action {
                Action::NewMacro => vec![crokey::parse("ctrl-t").expect("binding")],
                Action::DeleteMacro => vec![crokey::parse("ctrl-x").expect("binding")],
                other => crate::keybindings::BINDING_DEFS
                    .iter()
                    .find(|d| d.action == other)
                    .map(|d| d.default_keys.to_vec())
                    .unwrap_or_default(),
            },
            true,
        );
        let mut app = macro_list_app(bindings);
        let buf = render_to_buffer(&mut app);
        let footer = macro_modal_footer(&buf);
        assert!(
            footer.contains("Ctrl-t") && footer.contains("Ctrl-x"),
            "the footer must name the rebound keys, got {footer:?}"
        );
    }

    /// The project chooser's footer is state-aware, like its two filterable
    /// peers (the project browser and the kill-running dialog).
    ///
    /// It used to be built unconditionally, so while the search row was up it
    /// still read `<j> down  <k> up  </> search  <Enter> choose  <Esc> cancel`
    /// even though `j`, `k` and `/` were all being typed into the filter and
    /// Escape was leaving search rather than cancelling. Only two of its five
    /// segments survive contact with search mode, so only those two are drawn.
    ///
    /// Note the deliberate difference from the two peers: they say
    /// `<Enter> done`, but this chooser's confirm key PICKS the highlighted row
    /// (pinned by `pick_project_confirm_picks_the_highlighted_row_while_searching`),
    /// so it says `choose`. Matching their wording would have been a new lie.
    #[test]
    fn the_project_chooser_footer_is_state_aware_while_searching() {
        /// The chooser's footer, read off the bottom border of its header block.
        fn footer(app: &mut App) -> String {
            let buf = render_to_buffer(app);
            let area = super::centered_rect(72, 58, buf.area);
            let y = area.y + 2;
            ((area.x + 1)..(area.x + area.width - 1))
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
        }

        let mut app = test_app(default_bindings());
        app.open_project_chooser(ProjectChooserIntent::NewAgent)
            .expect("open the project chooser");

        let resting = footer(&mut app);
        for word in ["down", "up", "search", "choose", "cancel"] {
            assert!(
                resting.contains(word),
                "the resting footer must still name {word}, got {resting:?}"
            );
        }

        // Turn the search row on the way a user does.
        let search_key = app
            .bindings
            .first_key_reaching(Action::SearchToggle, |_| true)
            .expect("a search binding");
        app.handle_key(press(search_key)).expect("begin search");
        assert!(
            matches!(
                app.prompt,
                PromptState::PickProject {
                    list: super::SearchableList {
                        searching: true,
                        ..
                    },
                    ..
                }
            ),
            "the search row must be up or this test proves nothing"
        );

        let searching = footer(&mut app);
        for dead in ["down", "up", "search", "cancel"] {
            assert!(
                !searching.contains(dead),
                "the search footer must not promise {dead}, got {searching:?}"
            );
        }
        assert!(
            searching.contains("choose") && searching.contains("clear"),
            "the search footer must name what the two live keys do, got {searching:?}"
        );
    }

    /// The nested delete-confirm is a small box painted OVER the still-drawn
    /// macro list, so the list's footer stayed readable below it and every
    /// word of it was false: the new-macro and delete-macro keys are dead
    /// while the confirm is up, and the confirm/close keys mean Delete/Cancel
    /// rather than edit/close. Its peer confirmations (Delete Terminal, Close
    /// Tab, Discard File) render no footer at all, so this one renders none
    /// either.
    #[test]
    fn the_macro_delete_confirm_hides_the_footer_it_contradicts() {
        let mut app = macro_list_app(default_bindings());
        let delete_key = app
            .bindings
            .first_key_reaching(Action::DeleteMacro, |_| true)
            .expect("a delete-macro binding");
        let new_key = app
            .bindings
            .first_key_reaching(Action::NewMacro, |_| true)
            .expect("a new-macro binding");
        app.handle_key(press(delete_key)).expect("stage the delete");
        assert!(
            matches!(
                app.prompt,
                PromptState::EditMacros {
                    pending_delete: Some(_),
                    ..
                }
            ),
            "the delete-confirm must be up or this test proves nothing"
        );

        // The two keys the footer advertises really are dead while the confirm
        // is up, which is what makes leaving the footer on screen a lie.
        let before = format!("{:?}", app.prompt);
        app.handle_key(press(new_key)).expect("new macro");
        app.handle_key(press(delete_key)).expect("delete macro");
        assert_eq!(
            format!("{:?}", app.prompt),
            before,
            "neither advertised key does anything while the delete-confirm is up"
        );

        let buf = render_to_buffer(&mut app);
        assert_eq!(
            macro_modal_footer(&buf).trim(),
            "",
            "the macro list's footer must not stay readable under the delete-confirm"
        );
    }

    /// No em-dash in anything a user can see.
    #[test]
    fn the_macro_editor_title_carries_no_em_dash() {
        let mut app = macro_list_app(default_bindings());
        app.handle_key(ratatui::crossterm::event::KeyEvent::new(
            ratatui::crossterm::event::KeyCode::Enter,
            ratatui::crossterm::event::KeyModifiers::NONE,
        ))
        .expect("open the editor");
        let buf = render_to_buffer(&mut app);
        let title = (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .find(|row| row.contains("Edit Macro"))
            .expect("the editor must paint its title");
        assert!(
            !title.contains('\u{2014}'),
            "shipped title still holds an em-dash: {title:?}"
        );
        assert!(title.contains("Edit Macro: greet"), "got {title:?}");
    }

    /// A footer may be incomplete; it may never be WRONG. The
    /// `Space act on focus` / `Space toggle` segment is a promise about what
    /// Space does RIGHT NOW, and with a text field focused Space is content:
    /// it types a space into a single-line field, and on an unengaged
    /// full-text field it does nothing at all. The segment must therefore be
    /// as state-aware as its `move focus` neighbour, which already drops
    /// itself when no key reaches focus movement.
    /// The delete dialog must promise what the delete will actually do. dux
    /// deletes only the branch it created, so an attached or adopted agent gets
    /// a checkbox that says the branch is kept, and a line naming which one.
    #[test]
    fn the_delete_dialog_says_whether_the_branch_goes_or_stays() {
        fn delete_prompt(
            provenance: dux_core::model::BranchProvenance,
            branch: &str,
            initial: &str,
        ) -> PromptState {
            PromptState::ConfirmDeleteAgent {
                session_id: "s1".to_string(),
                agent_label: branch.to_string(),
                target: crate::app::DeleteAgentTarget::Managed {
                    branch_name: branch.to_string(),
                    initial_branch: initial.to_string(),
                    branch_provenance: provenance,
                    worktree_shared: false,
                },
                focus: super::DeleteAgentFocus::Checkbox,
                delete_worktree: true,
            }
        }

        let mut app = test_app(default_bindings());
        app.prompt = delete_prompt(
            dux_core::model::BranchProvenance::CreatedByDux,
            "feat",
            "feat",
        );
        let screen = rendered_screen(&mut app);
        assert!(
            screen.contains("Also delete the worktree and branch"),
            "a branch dux created is deleted with the worktree:\n{screen}"
        );

        app.prompt = delete_prompt(
            dux_core::model::BranchProvenance::AttachedExisting,
            "develop",
            "develop",
        );
        let screen = rendered_screen(&mut app);
        assert!(
            screen.contains("Also delete the worktree (branch kept)"),
            "an attached branch is not dux's to delete:\n{screen}"
        );
        // The sentence wraps inside a 56-column dialog, so assert its pieces.
        assert!(
            screen.contains("Branch \"develop\" existed before this agent and is")
                && screen.contains("kept."),
            "the dialog must name the surviving branch and why:\n{screen}"
        );

        // Drifted: the branch that survives by provenance is the birth branch.
        app.prompt = delete_prompt(
            dux_core::model::BranchProvenance::Adopted,
            "feature-x",
            "main",
        );
        let screen = rendered_screen(&mut app);
        assert!(
            screen.contains("Branch \"main\" came with the worktree this")
                && screen.contains("kept."),
            "an adopted branch came with the worktree:\n{screen}"
        );
    }

    /// The worktree manager's removal confirmation says what the web dialog
    /// says: forcible removal, the uncommitted-work sentence when dirty, and
    /// what happens to the branch, with the checkbox naming it.
    #[test]
    fn the_worktree_removal_dialog_warns_and_names_the_branch() {
        fn confirm(
            app: &App,
            branch: Option<&str>,
            dirty: bool,
            delete_branch: bool,
        ) -> PromptState {
            let project = app.engine.projects[0].clone();
            PromptState::ConfirmDeleteWorktree(Box::new(super::ConfirmDeleteWorktreePrompt {
                previous: super::ManageWorktreesPrompt {
                    project: project.clone(),
                    entries: Vec::new(),
                    loading: false,
                    selected: None,
                    error: None,
                },
                project,
                path: std::path::PathBuf::from("/tmp/worktrees/demo/free"),
                label: branch.unwrap_or("detached 1a2b3c4").to_string(),
                branch: branch.map(str::to_string),
                dirty,
                delete_branch,
                focus: super::DeleteWorktreeFocus::Cancel,
            }))
        }

        let mut app = test_app(default_bindings());
        app.prompt = confirm(&app, Some("free"), false, true);
        let screen = rendered_screen(&mut app);
        assert!(
            screen.contains("Delete the worktree for free?"),
            "the dialog asks the web dialog's question:\n{screen}"
        );
        assert!(
            screen.contains("Delete worktree"),
            "and offers the web dialog's button:\n{screen}"
        );
        assert!(
            screen.contains("forcibly") && screen.contains("no trash"),
            "the removal is forced and cannot be undone:\n{screen}"
        );
        assert!(
            screen.contains("Also delete the branch free"),
            "the checkbox names the branch:\n{screen}"
        );
        assert!(
            !screen.contains("uncommitted changes"),
            "a clean worktree gets no dirty sentence:\n{screen}"
        );

        app.prompt = confirm(&app, Some("free"), true, false);
        let screen = rendered_screen(&mut app);
        assert!(
            screen.contains("uncommitted changes"),
            "a dirty worktree says its work goes with it:\n{screen}"
        );
        assert!(
            screen.contains("is not committed exists"),
            "and that nothing uncommitted in there exists anywhere else:\n{screen}"
        );
        assert!(
            screen.contains("is kept"),
            "with the checkbox off the branch survives, and the copy says so:\n{screen}"
        );

        app.prompt = confirm(&app, None, false, true);
        let screen = rendered_screen(&mut app);
        assert!(
            screen.contains("Delete the worktree for detached 1a2b3c4?"),
            "a detached worktree is named by its row label:\n{screen}"
        );
        assert!(
            !screen.contains("Also delete the branch"),
            "a detached worktree offers no branch checkbox:\n{screen}"
        );
        assert!(
            screen.contains("not on a branch"),
            "and says why:\n{screen}"
        );
        assert!(
            screen.contains("so there is no branch"),
            "spelling out that there is no choice to make (the sentence wraps):\n{screen}"
        );
    }

    /// The removal confirmation's words, pinned.
    ///
    /// The web's `WorktreesDialog` says the same sentences and pins them in its
    /// own suite. The two dialogs are parallel implementations of one piece of
    /// copy, and nothing but a test on each side keeps them in step: the TUI's
    /// drifted away from the web's once already, silently, in the change that
    /// introduced it.
    #[test]
    fn the_worktree_removal_copy_is_the_web_dialogs_copy() {
        assert_eq!(
            super::delete_worktree_title("free"),
            "Delete the worktree for free?"
        );
        assert_eq!(
            super::DELETE_WORKTREE_FORCED,
            "This action cannot be undone: dux has no trash and removes the directory forcibly."
        );
        assert_eq!(
            super::DELETE_WORKTREE_DIRTY,
            "This worktree has uncommitted changes, and they go with it. Nothing in there that \
             is not committed exists anywhere else."
        );
        assert_eq!(
            super::DELETE_WORKTREE_DETACHED,
            "This worktree is not on a branch, so there is no branch to keep or delete. Only \
             the working directory is removed."
        );
        assert_eq!(
            super::delete_worktree_branch_line("free", true),
            "The branch \"free\" will be deleted with it, forcibly. Any commits on it that are \
             not merged anywhere else go too."
        );
        assert_eq!(
            super::delete_worktree_branch_line("free", false),
            "The branch \"free\" is kept. Only the working directory is removed."
        );
    }

    /// The checkbox label is measured once to size the dialog and rendered
    /// again to draw it. One helper, or the two drift and the dialog's height
    /// stops matching its contents.
    #[test]
    fn the_delete_checkbox_label_comes_from_one_helper() {
        for provenance in [
            dux_core::model::BranchProvenance::CreatedByDux,
            dux_core::model::BranchProvenance::AttachedExisting,
            dux_core::model::BranchProvenance::Adopted,
        ] {
            let mut app = test_app(default_bindings());
            app.prompt = PromptState::ConfirmDeleteAgent {
                session_id: "s1".to_string(),
                agent_label: "b".to_string(),
                target: crate::app::DeleteAgentTarget::Managed {
                    branch_name: "b".to_string(),
                    initial_branch: "b".to_string(),
                    branch_provenance: provenance,
                    worktree_shared: false,
                },
                focus: super::DeleteAgentFocus::Checkbox,
                delete_worktree: false,
            };
            let screen = rendered_screen(&mut app);
            assert!(
                screen.contains(super::delete_agent_checkbox_label(provenance)),
                "{provenance:?}: the rendered label must be the helper's:\n{screen}"
            );
        }
    }

    #[test]
    fn the_space_hint_only_appears_when_space_acts_on_something() {
        // ── The three configure modals: focus on the unengaged full-text
        // body, where Space does nothing whatsoever.
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::ConfigureStartupCommand {
            project_id: "p1".to_string(),
            project_name: "p1".to_string(),
            input: TextInput::with_text("make dev".to_string()).with_multiline(8),
            focus: super::ConfigureFieldFocus::Input,
        };
        let screen = rendered_screen(&mut app);
        assert!(
            !screen.contains("Space act on focus"),
            "the configure body takes no Space; the footer must not promise one:\n{screen}"
        );

        // On a button, Space really does act, so the segment comes back.
        app.prompt = PromptState::ConfigureStartupCommand {
            project_id: "p1".to_string(),
            project_name: "p1".to_string(),
            input: TextInput::with_text("make dev".to_string()).with_multiline(8),
            focus: super::ConfigureFieldFocus::Save,
        };
        let screen = rendered_screen(&mut app);
        assert!(
            screen.contains("Space act on focus"),
            "Space activates the focused button; say so:\n{screen}"
        );

        // ── The macro editor: the name field is single-line, so Space is
        // typed into it, and the unengaged body takes nothing.
        for focus in [super::MacroEditFocus::Name, super::MacroEditFocus::Text] {
            let mut app = macro_editor_app(focus);
            let screen = rendered_screen(&mut app);
            assert!(
                !screen.contains("Space act on focus"),
                "{focus:?}: Space is content here, not an action:\n{screen}"
            );
        }
        let mut app = macro_editor_app(super::MacroEditFocus::Save);
        let screen = rendered_screen(&mut app);
        assert!(
            screen.contains("Space act on focus"),
            "Space activates the focused button; say so:\n{screen}"
        );

        // ── Rename: Space types into the name field, and toggles the checkbox.
        let mut app = test_app(default_bindings());
        app.prompt = rename_prompt();
        let screen = rendered_screen(&mut app);
        assert!(
            !screen.contains("Space toggle"),
            "the rename name field types a space; it toggles nothing:\n{screen}"
        );
        app.prompt = PromptState::RenameSession {
            session_id: "session-1".to_string(),
            input: TextInput::with_text("agent".to_string()),
            rename_branch: false,
            focus: RenameSessionFocus::RenameBranchCheckbox,
            branch_named: true,
        };
        let screen = rendered_screen(&mut app);
        assert!(
            screen.contains("Space toggle"),
            "with the checkbox focused Space really does toggle:\n{screen}"
        );

        // ── New agent name: same shape.
        let mut app = test_app(default_bindings());
        app.prompt = name_new_agent_prompt(&app);
        let screen = rendered_screen(&mut app);
        assert!(
            !screen.contains("Space toggle"),
            "the new-agent name field types a space:\n{screen}"
        );
        let PromptState::NameNewAgent { focus, .. } = &mut app.prompt else {
            unreachable!()
        };
        *focus = NameNewAgentFocus::RandomizedNameCheckbox;
        let screen = rendered_screen(&mut app);
        assert!(
            screen.contains("Space toggle"),
            "with the checkbox focused Space really does toggle:\n{screen}"
        );
    }
}
