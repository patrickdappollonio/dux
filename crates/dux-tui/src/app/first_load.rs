//! The two first-load screens: the first-run welcome and the post-upgrade
//! what's-new. They share ONE modal frame and differ only in text and buttons.
//!
//! The gate (which screen, and whether the launch stamps the running version as
//! seen) is [`dux_core::first_load`]; the CONTENT is
//! [`dux_core::welcome_screen`] and [`dux_core::release_notes`]. Nothing in this
//! module re-authors prose or re-parses a release body — it only lays the shared
//! data out for a terminal.
//!
//! # The approved layout
//!
//! The braille duck sits in a LEFT column, a ruled vertical divider separates it
//! from scrollable prose on the RIGHT, and exactly TWO buttons sit at the bottom.
//! The border is the ordinary rounded overlay border every dux modal already
//! uses — that was decided after building and comparing double, thick,
//! quadrant-outside, and two hybrid treatments in a real terminal; rounded reads
//! as part of the app rather than as a stranger. Do not make it heavier.
//!
//! The duck takes the theme's LEADING color (whatever derives from
//! `accent.primary`), so it belongs to whatever theme the user runs and needs no
//! new theme token. That deliberately rules out `session_detached`, the theme's
//! only amber, which carries the WARNING semantic and would recolor the duck
//! whenever a theme restyles warnings. The modal border keeps the normal overlay
//! border color: accenting it too would make it the same hue as the duck and
//! flatten the composition.

use super::*;

use crate::app::components::render_scroll_marker;
use crate::app::render::centered_rect_exact;
use dux_core::release_notes::ReleaseNotes;
use dux_core::welcome_screen::WelcomeScreen;

/// The braille duck. 33 columns wide, 15 rows tall.
pub(crate) const DUCK: &[&str] = &[
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

/// Visible width of one [`DUCK`] row, in columns.
pub(crate) const ART_WIDTH: u16 = 33;

/// Target modal width in COLUMNS. Deliberately wider than a routine dialog
/// because this one carries the art column PLUS prose.
///
/// Sized from the READING MEASURE rather than picked round: the duck column and
/// the divider spend a fixed 38 columns, so the prose column is whatever is left.
/// At 104 that leaves 63 characters, inside the classic 60-70 band where prose is
/// comfortable to read. The earlier 90 left only 49, which read cramped next to
/// the art. [`centered_rect_exact`] clamps it down, so a narrow window simply
/// gets less, and below [`shows_art`]'s threshold the duck drops out entirely and
/// the prose takes the full width.
pub(crate) const MODAL_COLS: u16 = 104;

/// Below this many columns of prose the art column is dropped entirely: a duck
/// plus a 20-column ribbon of text is worse than no duck.
pub(crate) const MIN_PROSE_COLS: u16 = 30;

/// Columns the divider column occupies (`" │ "`).
const RULE_COLS: u16 = 3;

/// One column of padding on each side of the art.
const ART_PADDING: u16 = 2;

/// Whether the art column fits alongside a readable prose column.
///
/// 33 art + 2 padding + 3 divider + 30 minimum prose = 68 columns of INNER
/// width. Under that the duck is dropped and the prose takes the whole width
/// rather than both being squeezed.
pub(crate) fn shows_art(inner_width: u16) -> bool {
    inner_width >= ART_WIDTH + ART_PADDING + RULE_COLS + MIN_PROSE_COLS
}

/// Char-safe greedy word wrap. Never slices by byte offset: release prose and
/// config paths are full of multi-byte punctuation, CJK, and emoji, and byte
/// slicing would panic mid-character.
pub(crate) fn wrap(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut line = String::new();
    let mut len = 0usize;
    for word in text.split_whitespace() {
        let wlen = word.chars().count();
        if len == 0 {
            line.push_str(word);
            len = wlen;
        } else if len + 1 + wlen <= width {
            line.push(' ');
            line.push_str(word);
            len += 1 + wlen;
        } else {
            lines.push(std::mem::take(&mut line));
            line.push_str(word);
            len = wlen;
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// Which of the modal's two buttons has keyboard focus. The first is always the
/// primary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FirstLoadButton {
    Primary,
    Secondary,
}

impl FirstLoadButton {
    /// Tab / Shift-Tab / arrows all move between exactly two buttons, so one
    /// toggle covers every direction.
    pub(crate) fn toggled(self) -> Self {
        match self {
            Self::Primary => Self::Secondary,
            Self::Secondary => Self::Primary,
        }
    }

    fn index(self) -> usize {
        match self {
            Self::Primary => 0,
            Self::Secondary => 1,
        }
    }
}

/// Which screen the shared modal is showing, carrying that screen's content.
#[derive(Clone, Debug)]
pub(crate) enum FirstLoadScreen {
    /// First ever launch. Content from [`dux_core::welcome_screen`].
    Welcome(WelcomeScreen),
    /// The recorded version differs from the running one. Content from a fetched
    /// [`ReleaseNotes`]. Boxed because `ReleaseNotes` is much larger than the
    /// welcome variant and `PromptState` is moved around by value.
    WhatsNew(Box<ReleaseNotes>),
}

/// The first-load modal's state.
#[derive(Clone, Debug)]
pub(crate) struct FirstLoadPrompt {
    pub(crate) screen: FirstLoadScreen,
    pub(crate) scroll: u16,
    pub(crate) focus: FirstLoadButton,
    /// Whether DISMISSING this screen should record the running version as seen.
    ///
    /// Carried from [`dux_core::first_load::FirstLoadPlan::mark_seen`] and acted
    /// on at dismissal, never when the plan was computed — that timing is the
    /// core module's binding contract, and it is what lets the same shared SQLite
    /// row serve a long-lived web server too. A screen the user opened
    /// deliberately (a palette command) carries `false`: an explicit look must
    /// not consume a pending upgrade's notes.
    pub(crate) mark_seen: bool,
}

impl FirstLoadPrompt {
    pub(crate) fn welcome(screen: WelcomeScreen, mark_seen: bool) -> Self {
        Self {
            screen: FirstLoadScreen::Welcome(screen),
            scroll: 0,
            focus: FirstLoadButton::Primary,
            mark_seen,
        }
    }

    pub(crate) fn whats_new(notes: ReleaseNotes, mark_seen: bool) -> Self {
        Self {
            screen: FirstLoadScreen::WhatsNew(Box::new(notes)),
            scroll: 0,
            focus: FirstLoadButton::Primary,
            mark_seen,
        }
    }

    /// The modal title. The what's-new screen names the version the user is
    /// actually ON, taken from the fetched release's own tag (falling back to the
    /// running build's version if a release payload somehow carried no tag).
    pub(crate) fn title(&self) -> String {
        match &self.screen {
            FirstLoadScreen::Welcome(_) => " Welcome to dux ".to_string(),
            FirstLoadScreen::WhatsNew(notes) => {
                let version = if notes.version.trim().is_empty() {
                    dux_core::display_version()
                } else {
                    notes.version.as_str()
                };
                format!(" What's new in dux {version} ")
            }
        }
    }

    /// Exactly two buttons per screen; the first is the primary.
    pub(crate) fn buttons(&self) -> [&'static str; 2] {
        match &self.screen {
            FirstLoadScreen::Welcome(_) => ["Add a project", "Visit getdux.app"],
            FirstLoadScreen::WhatsNew(_) => ["Open full notes", "Close"],
        }
    }

    /// The destination the screen's link button opens, shown right-aligned and
    /// dimmed next to the buttons so the user can see where a link goes before
    /// pressing it.
    pub(crate) fn link(&self) -> String {
        match &self.screen {
            FirstLoadScreen::Welcome(_) => dux_core::urls::WEBSITE.to_string(),
            FirstLoadScreen::WhatsNew(notes) => dux_core::release_notes::notes_url(Some(notes)),
        }
    }
}

/// What a first-load button does when activated. Pure, so the mapping from
/// (screen, focused button) to behavior is testable without an `App`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FirstLoadAction {
    /// Dismiss the screen and open the project browser.
    AddProject,
    /// Dismiss the screen and open `url` in the default browser.
    OpenUrl(String),
    /// Dismiss the screen and do nothing else.
    Dismiss,
}

/// What activating the focused button should do.
pub(crate) fn button_action(prompt: &FirstLoadPrompt) -> FirstLoadAction {
    match (&prompt.screen, prompt.focus) {
        (FirstLoadScreen::Welcome(_), FirstLoadButton::Primary) => FirstLoadAction::AddProject,
        (FirstLoadScreen::Welcome(_), FirstLoadButton::Secondary) => {
            FirstLoadAction::OpenUrl(prompt.link())
        }
        (FirstLoadScreen::WhatsNew(_), FirstLoadButton::Primary) => {
            FirstLoadAction::OpenUrl(prompt.link())
        }
        (FirstLoadScreen::WhatsNew(_), FirstLoadButton::Secondary) => FirstLoadAction::Dismiss,
    }
}

// ---------------------------------------------------------------------------
// Content assembly
// ---------------------------------------------------------------------------

/// The semantic colors this modal paints with, all resolved from the active
/// [`Theme`] so no visual value is hardcoded here.
///
/// `accent` is the theme's LEADING color (`dux.title_focused`, which derives
/// from `accent.primary` exactly like `dux.selection_bg`,
/// `dux.help_section_header_fg`, and `dux.project_icon`), and `on_accent` /
/// `accent_fill` are the selection pair used for anything filled with it.
pub(crate) struct FirstLoadColors {
    accent: Color,
    accent_fill: Color,
    on_accent: Color,
    heading: Color,
    body: Color,
    detail: Color,
    dim: Color,
    rule: Color,
}

impl FirstLoadColors {
    pub(crate) fn from_theme(theme: &Theme) -> Self {
        Self {
            accent: theme.title_focused,
            accent_fill: theme.selection_bg,
            on_accent: theme.selection_fg,
            heading: theme.input_label_fg,
            body: theme.help_body_fg,
            detail: theme.hint_desc_fg,
            dim: theme.hint_dim_desc_fg,
            rule: theme.overlay_border,
        }
    }

    fn filled(&self) -> Style {
        Style::default()
            .fg(self.on_accent)
            .bg(self.accent_fill)
            .add_modifier(Modifier::BOLD)
    }
}

/// The welcome screen's body: the tagline, the prose paragraphs (which name this
/// machine's resolved config path), then the three NUMBERED steps.
pub(crate) fn welcome_lines(
    screen: &WelcomeScreen,
    width: u16,
    colors: &FirstLoadColors,
) -> Vec<Line<'static>> {
    let w = width.saturating_sub(1) as usize;
    // The tagline WRAPS. The gallery renders it as one unwrapped span, which
    // silently clips it at the prose column's edge (the tagline is 71 columns and
    // the prose column at the 90-column target is 50) — the approved intent is a
    // headline the reader can actually read, so it is wrapped here.
    let mut lines: Vec<Line<'static>> = wrap(screen.tagline, w)
        .into_iter()
        .map(|l| {
            Line::from(Span::styled(
                l,
                Style::default()
                    .fg(colors.heading)
                    .add_modifier(Modifier::BOLD),
            ))
        })
        .collect();
    lines.push(Line::from(""));
    for para in &screen.paragraphs {
        for l in wrap(para, w) {
            lines.push(Line::from(Span::styled(
                l,
                Style::default().fg(colors.body),
            )));
        }
        lines.push(Line::from(""));
    }

    for (i, step) in screen.steps.iter().enumerate() {
        lines.push(Line::from(vec![
            // The number is CARRIED by the core step, not derived from the
            // index, so the numbering is the same on both surfaces.
            Span::styled(format!(" {} ", step.number), colors.filled()),
            Span::raw(" "),
            Span::styled(
                step.title.to_string(),
                Style::default()
                    .fg(colors.heading)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        for l in wrap(step.detail, w.saturating_sub(5)) {
            lines.push(Line::from(vec![
                Span::raw("     "),
                Span::styled(l, Style::default().fg(colors.detail)),
            ]));
        }
        if i + 1 < screen.steps.len() {
            lines.push(Line::from(""));
        }
    }
    lines
}

/// The label above the release's feature titles.
pub(crate) const IN_THIS_RELEASE: &str = "In this release";

/// The what's-new screen's body: the headline, the intro paragraphs, then the
/// feature titles under an "In this release" label.
pub(crate) fn whats_new_lines(
    notes: &ReleaseNotes,
    width: u16,
    colors: &FirstLoadColors,
) -> Vec<Line<'static>> {
    let w = width.saturating_sub(1) as usize;
    let mut lines = Vec::new();
    if !notes.headline.trim().is_empty() {
        // Wrapped for the same reason as the welcome tagline: a release headline
        // is authored for a web page and routinely outruns the prose column.
        for l in wrap(&notes.headline, w) {
            lines.push(Line::from(Span::styled(
                l,
                Style::default()
                    .fg(colors.heading)
                    .add_modifier(Modifier::BOLD),
            )));
        }
        lines.push(Line::from(""));
    }

    // A release whose body had nothing the parser could read gets an explanation
    // and stops here. The guard is `has_renderable_body`, NOT `lines.is_empty()`:
    // a headline alone makes `lines` non-empty while leaving the body blank, and
    // that shape is what GitHub's APPENDED `## What's Changed` plus the release
    // workflow's APPENDED horizontal rule and `## Installation` leave behind when
    // the human writes a one-line headline and no prose. (Both generators append,
    // after the human's own sections; the rule is why "no body" cannot simply mean
    // "no text".) See `dux_core::release_notes` for the format a release body has
    // to follow.
    if !notes.has_renderable_body() {
        lines.push(Line::from(Span::styled(
            dux_core::release_notes::NO_NOTES_EXPLANATION.to_string(),
            Style::default().fg(colors.body),
        )));
        return lines;
    }

    for para in &notes.paragraphs {
        for l in wrap(para, w) {
            lines.push(Line::from(Span::styled(
                l,
                Style::default().fg(colors.body),
            )));
        }
        lines.push(Line::from(""));
    }

    if !notes.sections.is_empty() {
        lines.push(Line::from(Span::styled(
            IN_THIS_RELEASE.to_string(),
            Style::default()
                .fg(colors.accent)
                .add_modifier(Modifier::BOLD),
        )));
        for section in &notes.sections {
            let avail = w.saturating_sub(4);
            for (i, l) in wrap(section, avail).into_iter().enumerate() {
                let prefix = if i == 0 { "  \u{2013} " } else { "    " };
                lines.push(Line::from(vec![
                    Span::styled(prefix, Style::default().fg(colors.detail)),
                    Span::styled(l, Style::default().fg(colors.body)),
                ]));
            }
        }
    }

    lines
}

/// The body lines for whichever screen is showing, wrapped to `width`.
pub(crate) fn content_lines(
    prompt: &FirstLoadPrompt,
    width: u16,
    colors: &FirstLoadColors,
) -> Vec<Line<'static>> {
    match &prompt.screen {
        FirstLoadScreen::Welcome(screen) => welcome_lines(screen, width, colors),
        FirstLoadScreen::WhatsNew(notes) => whats_new_lines(notes, width, colors),
    }
}

/// Where each of the two one-row pill buttons sits inside `row`.
///
/// The pills are `" Label "`, laid out left to right with a two-column gap,
/// exactly as the approved layout renders them. Returned so the renderer and the
/// mouse hit-test share one geometry.
pub(crate) fn button_rects(row: Rect, labels: [&str; 2]) -> [Rect; 2] {
    let primary_w = u16::try_from(labels[0].chars().count() + 2).unwrap_or(u16::MAX);
    let secondary_w = u16::try_from(labels[1].chars().count() + 2).unwrap_or(u16::MAX);
    let primary = Rect {
        x: row.x,
        y: row.y,
        width: primary_w.min(row.width),
        height: 1,
    };
    let secondary_x = row.x.saturating_add(primary_w).saturating_add(2);
    let secondary_width = if secondary_x >= row.x + row.width {
        0
    } else {
        secondary_w.min(row.x + row.width - secondary_x)
    };
    let secondary = Rect {
        x: secondary_x.min(row.x + row.width),
        y: row.y,
        width: secondary_width,
        height: 1,
    };
    [primary, secondary]
}

/// The button row: two pills (the focused one takes the accent fill, mirroring
/// the app's confirm-button treatment) plus, when the column is wide enough, the
/// destination the link button opens, right-aligned and dimmed.
pub(crate) fn button_row(
    labels: [&str; 2],
    focus: FirstLoadButton,
    link: &str,
    width: u16,
    colors: &FirstLoadColors,
) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for (i, label) in labels.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        let style = if i == focus.index() {
            colors.filled()
        } else {
            Style::default().fg(colors.detail)
        };
        spans.push(Span::styled(format!(" {label} "), style));
    }
    let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
    let link_len = link.chars().count();
    if used + 2 + link_len <= width as usize {
        let pad = width as usize - used - link_len;
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled(
            link.to_string(),
            Style::default().fg(colors.dim),
        ));
    }
    Line::from(spans)
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/// Rows the modal spends on chrome: the border ring (2), the blank the content
/// pane naturally leaves, the button rule (1), and the button row (1), plus a
/// row of breathing space.
const CHROME_ROWS: u16 = 6;

/// Rows the BODY wants, and what actually sets the modal's height.
///
/// The duck used to set it, at 15 rows, which left the prose pane 17 rows. Both
/// screens carry more than that — the welcome has a wrapped tagline, four
/// paragraphs and three numbered steps; a release has a headline, its intro and
/// its feature titles — so both opened already scrolled, and on the welcome that
/// meant step 3 of a three-step "how to start" guide was below the fold on the
/// first screen a new user ever sees.
///
/// 24 rows shows appreciably more of both before any scrolling. It is a target,
/// not a demand: [`centered_rect_exact`] clamps to the terminal, so a short
/// window simply gets less and the scroll marker says so.
const MIN_BODY_ROWS: u16 = 24;

/// Where the modal sits. Deliberately one function so the renderer and any test
/// asking "did the duck fit?" agree on the geometry.
///
/// The height is the taller of the body target and the duck, so the art can
/// never be clipped by a body that wants fewer rows than the duck occupies.
pub(crate) fn modal_area(area: Rect) -> Rect {
    let duck_rows = u16::try_from(DUCK.len()).unwrap_or(u16::MAX);
    let height = duck_rows.max(MIN_BODY_ROWS).saturating_add(CHROME_ROWS);
    centered_rect_exact(MODAL_COLS, height, area)
}

/// What the renderer measured, handed back so the caller can record the mouse
/// geometry and the scroll extent without the renderer needing `&mut App`.
pub(crate) struct FirstLoadRender {
    pub(crate) primary_button: Rect,
    pub(crate) secondary_button: Rect,
    /// Visible height of the content pane, in rows.
    pub(crate) content_height: u16,
    /// Total content lines at the rendered width.
    pub(crate) content_lines: u16,
}

/// Paint the shared modal: the duck in a left column, a ruled vertical divider,
/// scrollable content on the right, and exactly two buttons at the bottom.
///
/// A free function rather than an `App` method so the caller can keep its
/// `&self.prompt` borrow alive while writing the measured geometry back into its
/// own (disjoint) fields.
pub(crate) fn render_modal(
    frame: &mut Frame,
    area: Rect,
    prompt: &FirstLoadPrompt,
    theme: &Theme,
) -> FirstLoadRender {
    let colors = FirstLoadColors::from_theme(theme);

    // The ordinary rounded overlay border, in the ordinary overlay border color.
    // Only the TITLE takes the accent: coloring the border too would make it the
    // same hue as the duck and flatten the composition.
    let block = Block::default()
        .title(Line::from(Span::styled(
            prompt.title(),
            Style::default()
                .fg(colors.accent)
                .add_modifier(Modifier::BOLD),
        )))
        .borders(Borders::ALL)
        .border_set(border::ROUNDED)
        .border_style(Style::default().fg(theme.overlay_border))
        .style(Style::default().bg(theme.overlay_bg));
    let inner = block.inner(area);
    block.render(area, frame.buffer_mut());

    // On a narrow terminal the duck is dropped and the prose takes the whole
    // width, rather than both being squeezed.
    let right = if shows_art(inner.width) {
        let [art_col, rule_col, right] = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(ART_WIDTH + ART_PADDING),
                Constraint::Length(RULE_COLS),
                Constraint::Min(MIN_PROSE_COLS),
            ])
            .areas(inner);

        let duck_rows = u16::try_from(DUCK.len()).unwrap_or(u16::MAX);
        let top = art_col.height.saturating_sub(duck_rows) / 2;
        let art: Vec<Line> = std::iter::repeat_n(Line::from(""), top as usize)
            .chain(DUCK.iter().map(|row| {
                Line::from(Span::styled(
                    format!(" {row}"),
                    Style::default().fg(colors.accent),
                ))
            }))
            .collect();
        Paragraph::new(art).render(art_col, frame.buffer_mut());

        let rule: Vec<Line> = (0..rule_col.height)
            .map(|_| Line::from(Span::styled(" │ ", Style::default().fg(colors.rule))))
            .collect();
        Paragraph::new(rule).render(rule_col, frame.buffer_mut());
        right
    } else {
        inner
    };

    let [content, sep, buttons] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(4),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(right);

    let lines = content_lines(prompt, content.width, &colors);
    let total = u16::try_from(lines.len()).unwrap_or(u16::MAX);
    render_scrollable(frame, area, content, lines, prompt.scroll, &colors);

    Paragraph::new(Line::from(Span::styled(
        "─".repeat(sep.width as usize),
        Style::default().fg(colors.rule),
    )))
    .render(sep, frame.buffer_mut());

    let labels = prompt.buttons();
    let link = prompt.link();
    Paragraph::new(button_row(
        labels,
        prompt.focus,
        &link,
        buttons.width,
        &colors,
    ))
    .render(buttons, frame.buffer_mut());

    let [primary_button, secondary_button] = button_rects(buttons, labels);
    FirstLoadRender {
        primary_button,
        secondary_button,
        content_height: content.height,
        content_lines: total,
    }
}

/// The scrollable content pane, plus the shared one-cell direction marker in the
/// modal's border column (see [`crate::app::components::scroll_marker`], which
/// owns the geometry and the glyph table for every scrollable surface) when
/// there is more to see.
///
/// `outer` is the modal, `area` the content pane inside its border ring.
fn render_scrollable(
    frame: &mut Frame,
    outer: Rect,
    area: Rect,
    lines: Vec<Line<'static>>,
    scroll: u16,
    colors: &FirstLoadColors,
) {
    let total = lines.len();
    let visible = area.height as usize;
    let max_scroll = total.saturating_sub(visible);
    let offset = (scroll as usize).min(max_scroll);
    let slice: Vec<Line> = lines.into_iter().skip(offset).take(visible).collect();
    Paragraph::new(slice).render(area, frame.buffer_mut());

    render_scroll_marker(frame, outer, area, offset, visible, total, colors.accent);
}

// ---------------------------------------------------------------------------
// Release-notes worker plumbing
// ---------------------------------------------------------------------------

/// Why a release-notes fetch was started. The two paths differ in how a failure
/// is surfaced and in whether the result feeds the startup gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NotesFetchPurpose {
    /// The startup gate asked for the notes. A failure is silent (the user did
    /// not ask for anything, and `after_fetch` decides whether the notes get
    /// another chance on a later launch).
    Automatic,
    /// The user ran the `show-release-notes` palette command. A failure MUST be
    /// reported, because they asked.
    Explicit,
}

/// What a release-notes worker hands back to the UI thread.
///
/// The keyed busy→final status rides the engine's own worker channel as a
/// [`WorkerEvent::StatusOpCompleted`]; this carries the PAYLOAD, which that
/// event has no room for.
pub(crate) struct NotesFetched {
    /// The notes, or the formatted failure (already rendered to a string so
    /// nothing non-`Send` crosses the channel).
    pub(crate) result: Result<ReleaseNotes, String>,
    /// The gate outcome, classified by core (`release_notes::outcome_of`) so the
    /// definitive 404 and a transient failure stay distinguishable.
    pub(crate) outcome: dux_core::first_load::NotesOutcome,
    pub(crate) purpose: NotesFetchPurpose,
}

/// The keyed busy→final status for one release-notes fetch.
///
/// Split out of [`App::spawn_release_notes_fetch`] so the failure branch is
/// testable without a network round trip, and so the ESCALATION flag has one
/// documented reader. `explicit_waiting` is the shared
/// `App::notes_fetch_explicit_request`: the closure below is invoked inside the
/// worker at genuine failure time (`op.resolve`), so it sees a request that
/// arrived after the spawn.
fn notes_status_op(
    purpose: NotesFetchPurpose,
    explicit_waiting: Arc<AtomicBool>,
) -> dux_core::engine::StatusOp<ReleaseNotes, dux_core::release_notes::FetchError> {
    dux_core::engine::status_op("Fetching the dux release notes from GitHub...")
        .on_success(|_: &ReleaseNotes| {
            // The modal IS the visible result, so no second message.
            dux_core::engine::Final::clear()
        })
        // The asymmetry below is DELIBERATE; do not "fix" the quiet branch into
        // an error. CLAUDE.md's "prefer explicit failure over silent waiting"
        // governs operations the USER initiated. An unsolicited background
        // version check is not one: raising an error status here would put a
        // failure toast on screen at every launch on a plane, a train, a
        // locked-down network, or whenever GitHub is having a bad hour — pure
        // noise about something the user never asked for and cannot act on. The
        // core gate already guarantees a retry (a transient failure does NOT
        // mark the version seen, so the notes reappear on a launch with
        // network), and the reason is always written to dux.log. The explicit
        // `show-release-notes` path fails loudly, which is where the tenet
        // actually applies — INCLUDING when the user asked while an automatic
        // fetch was already running, which is what `explicit_waiting` carries.
        .on_failure(
            move |err: &dux_core::release_notes::FetchError| match purpose {
                NotesFetchPurpose::Automatic
                    if !explicit_waiting.load(std::sync::atomic::Ordering::SeqCst) =>
                {
                    dux_core::engine::Final::clear()
                }
                // The user asked, so say what happened.
                _ => dux_core::engine::Final::error(format!(
                    "Could not load the dux release notes: {err}. Check your network and try the \
                     show-release-notes command again."
                )),
            },
        )
}

impl App {
    /// Record the running version as seen.
    ///
    /// A failure here is logged, not surfaced: the only consequence is that the
    /// screen may appear once more on a later launch, which is far less
    /// disruptive than an error the user cannot act on.
    pub(crate) fn mark_first_load_version_seen(&mut self) {
        let version = dux_core::display_version();
        if let Err(err) = self.engine.session_store.set_last_seen_version(version) {
            logger::warn(&format!(
                "failed to record dux version {version} as seen: {err:#}"
            ));
        }
    }

    /// STEP 1 of the gate, at startup. Runs on a cold boot only — the caller
    /// gates on [`SessionRestore`], so a web-server→TUI flip never re-shows
    /// either screen.
    pub(crate) fn begin_first_load(&mut self) {
        let last_seen = match self.engine.session_store.last_seen_version() {
            Ok(v) => v,
            Err(err) => {
                // Treat an unreadable row as "unknown" rather than failing the
                // boot; the cost is at most one extra screen.
                logger::warn(&format!(
                    "failed to read the last-seen dux version: {err:#}"
                ));
                None
            }
        };
        let plan = dux_core::first_load::plan(
            last_seen.as_deref(),
            dux_core::display_version(),
            self.engine.config.ui.disable_automated_welcome_screen,
            self.engine.config.ui.disable_release_notes,
        );
        self.apply_first_load_plan(plan);
    }

    /// Act on a computed plan. Split out from [`Self::begin_first_load`] so the
    /// stamp timing is testable without a store read.
    pub(crate) fn apply_first_load_plan(&mut self, plan: dux_core::first_load::FirstLoadPlan) {
        match plan.screen {
            dux_core::first_load::FirstLoad::Welcome => {
                // The welcome needs no network: show it now. It stamps on
                // DISMISSAL, per the core contract.
                let screen =
                    dux_core::welcome_screen::welcome_screen(&self.engine.paths.config_path);
                self.prompt =
                    PromptState::FirstLoad(FirstLoadPrompt::welcome(screen, plan.mark_seen));
            }
            dux_core::first_load::FirstLoad::WhatsNew => {
                // The notes BLOCK, so they are fetched on a worker; the plan is
                // held until the result lands and `after_fetch` folds it in.
                self.pending_first_load = Some(plan);
                self.spawn_release_notes_fetch(NotesFetchPurpose::Automatic);
            }
            dux_core::first_load::FirstLoad::Nothing => {
                // No screen to dismiss, so there is nothing to wait for.
                if plan.mark_seen {
                    self.mark_first_load_version_seen();
                }
            }
        }
    }

    /// Start the BLOCKING release-notes fetch on a background worker, with a
    /// keyed busy status that the worker's own final always replaces.
    pub(crate) fn spawn_release_notes_fetch(&mut self, purpose: NotesFetchPurpose) {
        let root = self.engine.paths.root.clone();
        let version = dux_core::display_version().to_string();
        let worker_tx = self.engine.worker_tx.clone();
        let (tx, rx) = mpsc::channel();
        self.notes_fetch_rx = Some(rx);
        // A fresh flag per fetch, so a request awaited on an earlier fetch can
        // never escalate this one. An EXPLICIT fetch starts already-escalated.
        let explicit_waiting = Arc::new(AtomicBool::new(purpose == NotesFetchPurpose::Explicit));
        self.notes_fetch_explicit_request = Arc::clone(&explicit_waiting);

        let op = notes_status_op(purpose, Arc::clone(&explicit_waiting));
        let pending = op.pending_status();

        thread::spawn(move || {
            let result = dux_core::release_notes::load_release_notes(&root, &version);
            let outcome = dux_core::release_notes::outcome_of(&result);
            if let Err(err) = &result {
                // The automatic path is deliberately silent on screen, so this
                // log line is the operator's only signal — which means the warn
                // stream must stay actionable. A definitive `NoSuchRelease` is
                // routine (a dev, local, or CI-tagged build simply has no
                // published release) and nothing can be done about it.
                let line = format!("release-notes fetch failed: {err}");
                if err.is_definitive() {
                    logger::info(&line);
                } else {
                    logger::warn(&line);
                }
            }
            let resolved = op.resolve(&result);
            let _ = worker_tx.send(WorkerEvent::StatusOpCompleted { resolved });
            let _ = tx.send(NotesFetched {
                result: result.map_err(|err| err.to_string()),
                outcome,
                purpose,
            });
        });

        self.apply_reaction(dux_core::engine::EventReaction::Status(pending));
    }

    /// Fold a finished release-notes fetch into the UI. Called from
    /// `drain_events` each tick.
    pub(crate) fn drain_notes_fetch(&mut self) {
        // A fetch that landed while the user had another modal open parked its
        // notes instead of stealing the slot. `drain_events` calls this every
        // tick, so this is the re-offer point.
        self.offer_deferred_first_load();
        let Some(rx) = self.notes_fetch_rx.as_ref() else {
            return;
        };
        let Ok(fetched) = rx.try_recv() else {
            return;
        };
        self.notes_fetch_rx = None;
        self.apply_notes_fetch(fetched);
    }

    /// Re-offer a first-load screen whose notes landed while the user had another
    /// modal open. A no-op until the prompt slot is free, and it never refetches:
    /// the notes were kept, and the plan was never consumed.
    pub(crate) fn offer_deferred_first_load(&mut self) {
        if !matches!(self.prompt, PromptState::None) || self.deferred_first_load_notes.is_none() {
            return;
        }
        let Some(notes) = self.deferred_first_load_notes.take() else {
            return;
        };
        let Some(plan) = self.pending_first_load.take() else {
            // The plan was consumed elsewhere (a reload, or a screen the user
            // opened by hand), so there is nothing left to offer.
            return;
        };
        // The notes are in hand, which is exactly `NotesOutcome::Fetched`; folding
        // it through core keeps the stamp decision in one place.
        let plan =
            dux_core::first_load::after_fetch(plan, dux_core::first_load::NotesOutcome::Fetched);
        self.prompt = PromptState::FirstLoad(FirstLoadPrompt::whats_new(*notes, plan.mark_seen));
    }

    /// The pure-ish half of [`Self::drain_notes_fetch`], so the fold is testable
    /// without a worker thread.
    pub(crate) fn apply_notes_fetch(&mut self, fetched: NotesFetched) {
        // An explicit request that arrived while THIS (automatic) fetch was
        // already running escalates it: the user asked, so the result is theirs
        // to see and its failure is theirs to hear. The flag is the same one the
        // worker's failure closure read.
        let purpose = if self
            .notes_fetch_explicit_request
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            NotesFetchPurpose::Explicit
        } else {
            fetched.purpose
        };
        match purpose {
            NotesFetchPurpose::Automatic => {
                // PEEK, do not take: a modal the user opened during the fetch
                // window owns the single `PromptState` slot, and replacing it
                // would throw away in-progress input. Park the notes with the
                // plan still pending and stamp NOTHING — the stamp belongs to a
                // dismissal of a screen that was actually shown, and stamping a
                // never-shown screen would discard this version's notes forever.
                if !matches!(self.prompt, PromptState::None)
                    && self.pending_first_load.is_some()
                    && fetched.result.is_ok()
                {
                    if let Ok(notes) = fetched.result {
                        self.deferred_first_load_notes = Some(Box::new(notes));
                    }
                    return;
                }
                let plan = self.pending_first_load.take();
                let Some(plan) = plan else {
                    // The plan was consumed elsewhere (a reload, or a screen the
                    // user opened by hand). Nothing left to decide.
                    return;
                };
                let plan = dux_core::first_load::after_fetch(plan, fetched.outcome);
                match (plan.screen, fetched.result) {
                    (dux_core::first_load::FirstLoad::WhatsNew, Ok(notes)) => {
                        self.prompt = PromptState::FirstLoad(FirstLoadPrompt::whats_new(
                            notes,
                            plan.mark_seen,
                        ));
                    }
                    _ => {
                        // No screen: stamp immediately when the plan says to (a
                        // definitive 404), and leave the version unmarked when it
                        // does not (a transient failure), so the notes reappear on
                        // a launch that can reach GitHub.
                        if plan.mark_seen {
                            self.mark_first_load_version_seen();
                        }
                    }
                }
            }
            NotesFetchPurpose::Explicit => {
                // An explicitly opened screen never stamps (`mark_seen: false`):
                // the user asked to LOOK, which must not consume a pending
                // upgrade's notes. On a failure there is nothing to do here — the
                // worker's keyed final already carries the reason.
                if let Ok(notes) = fetched.result {
                    self.prompt = PromptState::FirstLoad(FirstLoadPrompt::whats_new(notes, false));
                }
            }
        }
    }

    /// The `show-welcome-screen` palette command. Works even when
    /// `disable_automated_welcome_screen` is set: the flag suppresses only what
    /// dux does on its own.
    pub(crate) fn show_welcome_screen_command(&mut self) -> Result<()> {
        let screen = dux_core::welcome_screen::welcome_screen(&self.engine.paths.config_path);
        self.prompt = PromptState::FirstLoad(FirstLoadPrompt::welcome(screen, false));
        Ok(())
    }

    /// The `show-release-notes` palette command. Works even when
    /// `disable_release_notes` is set, and may fetch, because the user asked for
    /// it. `load_release_notes` already routes a real version to its own tag and
    /// a development build to the newest release.
    pub(crate) fn show_release_notes_command(&mut self) -> Result<()> {
        if self.notes_fetch_rx.is_some() {
            // The running fetch may have been dispatched as AUTOMATIC, whose
            // failure branch is silent. Tell it that someone is waiting, so a
            // transient failure reports instead of silently dropping this
            // request. The flag is shared with the worker thread, which is the
            // only way to reach a decision already baked in by `move` at spawn.
            self.notes_fetch_explicit_request
                .store(true, std::sync::atomic::Ordering::SeqCst);
            self.set_info(
                "Already loading the dux release notes from GitHub. They will open when they arrive.",
            );
            return Ok(());
        }
        self.spawn_release_notes_fetch(NotesFetchPurpose::Explicit);
        Ok(())
    }

    /// Dismiss the first-load modal, honoring the stamp-on-dismissal contract.
    /// Every exit path (Esc, either button, the generic overlay dismissal) routes
    /// here so the stamp can never be skipped by one of them.
    pub(crate) fn dismiss_first_load_prompt(&mut self) {
        if let PromptState::FirstLoad(prompt) =
            std::mem::replace(&mut self.prompt, PromptState::None)
            && prompt.mark_seen
        {
            self.mark_first_load_version_seen();
        }
    }

    /// Activate the focused button. Returns whether the app should quit (always
    /// false; the signature matches the other `resolve_*` button handlers so the
    /// mouse press machinery can call it uniformly).
    pub(crate) fn activate_first_load_button(&mut self) -> bool {
        let PromptState::FirstLoad(prompt) = &self.prompt else {
            return false;
        };
        let action = button_action(prompt);
        // Dismiss FIRST: `PromptState` holds one modal at a time, so opening the
        // project browser replaces this screen rather than stacking on it, and
        // the stamp must land before the state is overwritten.
        self.dismiss_first_load_prompt();
        match action {
            FirstLoadAction::AddProject => {
                if let Err(err) = self.open_project_browser() {
                    self.set_error(format!("Could not open the project browser: {err:#}"));
                }
            }
            FirstLoadAction::OpenUrl(url) => match dux_core::browser::open_url(&url) {
                Ok(()) => self.set_info(format!("Opened {url} in your default browser.")),
                Err(err) => self.set_error(format!(
                    "Could not open {url} in your default browser: {err:#}. Copy the address and \
                     open it by hand."
                )),
            },
            FirstLoadAction::Dismiss => {
                self.set_info("Closed the release notes. Run show-release-notes to reopen them.");
            }
        }
        false
    }

    /// Scroll the modal's content by `delta` lines, clamped to the last rendered
    /// content extent.
    pub(crate) fn scroll_first_load(&mut self, delta: i32) {
        let max = self
            .last_first_load_lines
            .saturating_sub(self.last_first_load_height.max(1));
        if let PromptState::FirstLoad(prompt) = &mut self.prompt {
            let next = if delta >= 0 {
                prompt.scroll.saturating_add(delta as u16)
            } else {
                prompt.scroll.saturating_sub(delta.unsigned_abs() as u16)
            };
            prompt.scroll = next.min(max);
        }
    }

    /// The height of one content page, for the page-scroll keys.
    pub(crate) fn first_load_page(&self) -> i32 {
        i32::from(self.last_first_load_height.max(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::components::scroll_marker_rect;
    use crate::app::test_support::{default_bindings, test_app};
    use dux_core::first_load::{FirstLoad, FirstLoadPlan, NotesOutcome};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use std::path::PathBuf;

    fn sample_notes() -> ReleaseNotes {
        ReleaseNotes {
            version: "v0.7.0".to_string(),
            headline: "Quieter plumbing, louder failures".to_string(),
            paragraphs: vec!["A tune-up release with fewer surprises.".to_string()],
            sections: vec![
                "Environment config for agents and terminals".to_string(),
                "A website exists now".to_string(),
            ],
            html_url: "https://github.com/patrickdappollonio/dux/releases/tag/v0.7.0".to_string(),
        }
    }

    fn sample_welcome() -> WelcomeScreen {
        dux_core::welcome_screen::welcome_screen(&PathBuf::from(
            "/home/ada/.config/dux/config.toml",
        ))
    }

    fn colors() -> FirstLoadColors {
        FirstLoadColors::from_theme(&Theme::default_dark())
    }

    /// Terminal size for tests that need the content to OVERFLOW its pane.
    ///
    /// `MIN_BODY_ROWS` deliberately makes the modal tall enough that the welcome
    /// fits without scrolling on a roomy terminal, so a scroll test cannot rely
    /// on the default fixture overflowing at 120x40 any more; it has to squeeze
    /// the terminal so the modal clamps and the content genuinely runs off.
    const OVERFLOW_TERM: (u16, u16) = (120, 18);

    fn render_prompt_to_string(app: &mut App, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    // ── layout thresholds ────────────────────────────────────────────────

    #[test]
    fn the_modal_starts_wide_and_drops_the_duck_only_when_prose_would_be_squeezed() {
        // The width is chosen from the reading measure, so assert the PROPERTY
        // that motivates it rather than only the magic number: with the duck and
        // divider spending their fixed columns, the prose column must land in the
        // 60-70 character band where prose reads comfortably.
        let prose_cols = MODAL_COLS - 2 - (ART_WIDTH + ART_PADDING + RULE_COLS) - 1;
        assert!(
            (60..=70).contains(&prose_cols),
            "prose column is {prose_cols}, outside the comfortable 60-70 measure"
        );
        assert!(shows_art(MODAL_COLS - 2), "the target width keeps the duck");
        assert!(shows_art(68), "33 art + 2 pad + 3 rule + 30 prose");
        assert!(!shows_art(67), "one column short and the prose wins");
        assert!(!shows_art(40));
        assert!(!shows_art(0));
    }

    #[test]
    fn the_body_sets_the_height_and_the_duck_only_floors_it() {
        let duck_rows = u16::try_from(DUCK.len()).unwrap_or(u16::MAX);
        assert!(
            MIN_BODY_ROWS > duck_rows,
            "the body target must exceed the duck, or the duck is back to \
             setting the height and both screens open scrolled again"
        );

        // On a roomy terminal the modal gets the full target.
        let roomy = modal_area(Rect::new(0, 0, 200, 60));
        assert_eq!(roomy.height, MIN_BODY_ROWS + CHROME_ROWS);

        // The prose pane is what the extra rows are FOR: body rows are the
        // modal minus the border ring, the button rule and the button row.
        let prose_rows = roomy.height - 2 - 1 - 1;
        assert!(
            prose_rows >= duck_rows,
            "prose pane {prose_rows} is shorter than the duck it sits beside"
        );

        // A short terminal clamps instead of overflowing.
        let cramped = modal_area(Rect::new(0, 0, 80, 12));
        assert!(
            cramped.height <= 12,
            "modal is {} rows in a 12-row terminal",
            cramped.height
        );
    }

    #[test]
    fn the_duck_is_the_documented_size() {
        assert_eq!(DUCK.len(), 15);
        for row in DUCK {
            assert_eq!(
                row.chars().count(),
                ART_WIDTH as usize,
                "every duck row must be exactly {ART_WIDTH} columns: {row}"
            );
        }
    }

    #[test]
    fn wrap_is_char_safe_on_multibyte_prose() {
        // Box-drawing, CJK, and emoji would panic under byte slicing.
        let lines = wrap("░██ 環境変数 とても長い行 🦆 ░██", 8);
        assert!(lines.iter().all(|l| l.chars().count() <= 8), "{lines:?}");
        assert_eq!(
            lines.concat().replace(' ', "").chars().count(),
            "░██環境変数とても長い行🦆░██".chars().count()
        );
        assert!(wrap("anything", 0).is_empty(), "zero width yields nothing");
    }

    // ── buttons ──────────────────────────────────────────────────────────

    #[test]
    fn each_screen_has_exactly_two_buttons_with_the_agreed_labels() {
        assert_eq!(
            FirstLoadPrompt::welcome(sample_welcome(), false).buttons(),
            ["Add a project", "Visit getdux.app"]
        );
        assert_eq!(
            FirstLoadPrompt::whats_new(sample_notes(), false).buttons(),
            ["Open full notes", "Close"]
        );
    }

    #[test]
    fn the_whats_new_title_names_the_version_the_user_is_on() {
        let prompt = FirstLoadPrompt::whats_new(sample_notes(), false);
        assert!(prompt.title().contains("v0.7.0"), "{}", prompt.title());
        // A release payload with no tag falls back to the running build rather
        // than showing a blank version.
        let mut untagged = sample_notes();
        untagged.version = "  ".to_string();
        assert!(
            FirstLoadPrompt::whats_new(untagged, false)
                .title()
                .contains(dux_core::display_version())
        );
    }

    #[test]
    fn the_welcome_links_to_the_website_and_the_notes_link_to_their_own_release() {
        assert_eq!(
            FirstLoadPrompt::welcome(sample_welcome(), false).link(),
            dux_core::urls::WEBSITE
        );
        assert_eq!(
            FirstLoadPrompt::whats_new(sample_notes(), false).link(),
            "https://github.com/patrickdappollonio/dux/releases/tag/v0.7.0"
        );
        // A release with no link falls back to the releases index, never "".
        let mut linkless = sample_notes();
        linkless.html_url = String::new();
        assert_eq!(
            FirstLoadPrompt::whats_new(linkless, false).link(),
            dux_core::urls::RELEASES
        );
    }

    #[test]
    fn the_focused_button_decides_what_activation_does() {
        let mut welcome = FirstLoadPrompt::welcome(sample_welcome(), false);
        assert_eq!(button_action(&welcome), FirstLoadAction::AddProject);
        welcome.focus = FirstLoadButton::Secondary;
        assert_eq!(
            button_action(&welcome),
            FirstLoadAction::OpenUrl(dux_core::urls::WEBSITE.to_string())
        );

        let mut whats_new = FirstLoadPrompt::whats_new(sample_notes(), false);
        assert_eq!(
            button_action(&whats_new),
            FirstLoadAction::OpenUrl(
                "https://github.com/patrickdappollonio/dux/releases/tag/v0.7.0".to_string()
            )
        );
        whats_new.focus = FirstLoadButton::Secondary;
        assert_eq!(button_action(&whats_new), FirstLoadAction::Dismiss);
    }

    #[test]
    fn button_rects_sit_side_by_side_with_a_misclick_safe_gap() {
        let row = Rect::new(4, 9, 50, 1);
        let [primary, secondary] = button_rects(row, ["Add a project", "Visit getdux.app"]);
        assert_eq!(primary.x, 4);
        assert_eq!(primary.width, 15, "\" Add a project \"");
        assert_eq!(secondary.x, 4 + 15 + 2, "two-column gap between targets");
        assert_eq!(secondary.width, 18, "\" Visit getdux.app \"");
        assert!(
            secondary.x > primary.x + primary.width,
            "the two click targets must not touch"
        );
    }

    #[test]
    fn button_rects_never_run_past_a_narrow_row() {
        let row = Rect::new(0, 0, 10, 1);
        let [primary, secondary] = button_rects(row, ["Open full notes", "Close"]);
        assert!(primary.x + primary.width <= row.x + row.width);
        assert!(secondary.x + secondary.width <= row.x + row.width);
    }

    #[test]
    fn the_button_row_shows_the_link_only_when_it_fits() {
        let colors = colors();
        let wide = button_row(
            ["Open full notes", "Close"],
            FirstLoadButton::Primary,
            "https://x.dev/v1",
            80,
            &colors,
        );
        let wide_text: String = wide.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(wide_text.contains("https://x.dev/v1"), "{wide_text}");

        let narrow = button_row(
            ["Open full notes", "Close"],
            FirstLoadButton::Primary,
            "https://x.dev/v1",
            30,
            &colors,
        );
        let narrow_text: String = narrow.spans.iter().map(|s| s.content.as_ref()).collect();
        assert!(
            !narrow_text.contains("https://x.dev/v1"),
            "the link must be dropped rather than wrap: {narrow_text}"
        );
        assert!(narrow_text.contains("Open full notes"));
    }

    // ── content assembly ─────────────────────────────────────────────────

    #[test]
    fn the_welcome_body_carries_the_tagline_the_prose_and_the_numbered_steps() {
        let screen = sample_welcome();
        let lines = welcome_lines(&screen, 60, &colors());
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.contains("One git worktree per coding agent"), "{text}");
        // This machine's resolved config path, straight from core.
        assert!(text.contains("/home/ada/.config/dux/config.toml"), "{text}");
        for n in ["1", "2", "3"] {
            assert!(text.contains(&format!(" {n} ")), "step {n} missing: {text}");
        }
        for step in screen.steps {
            assert!(text.contains(step.title), "{} missing: {text}", step.title);
        }
    }

    #[test]
    fn the_whats_new_body_carries_the_headline_prose_and_feature_titles() {
        let notes = sample_notes();
        let lines = whats_new_lines(&notes, 60, &colors());
        let text: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.contains("Quieter plumbing, louder failures"), "{text}");
        assert!(text.contains(IN_THIS_RELEASE), "{text}");
        for section in &notes.sections {
            assert!(text.contains(section.as_str()), "{section} missing: {text}");
        }
    }

    #[test]
    fn a_release_with_no_body_still_says_something() {
        let notes = ReleaseNotes {
            version: "v0.7.1".to_string(),
            ..Default::default()
        };
        let lines = whats_new_lines(&notes, 60, &colors());
        assert!(
            !lines.is_empty(),
            "an empty pane is not an acceptable state"
        );
        assert!(
            flatten(&lines).contains(dux_core::release_notes::NO_NOTES_EXPLANATION),
            "{:?}",
            flatten(&lines)
        );
    }

    /// REGRESSION. The empty-body guard used to be `lines.is_empty()`, which is
    /// FALSE the moment a headline exists, so a release body that parsed to a
    /// headline and nothing else rendered a title above a blank pane with no
    /// explanation. That shape is very reachable: GitHub APPENDS
    /// `## What's Changed` and the release workflow APPENDS `## Installation`
    /// after it, so a one-line human headline is all the parser is left with.
    #[test]
    fn a_release_whose_body_is_only_a_headline_still_explains_itself() {
        let notes = ReleaseNotes {
            version: "v0.7.0".to_string(),
            headline: "Quieter plumbing, louder failures".to_string(),
            html_url: "https://example.invalid/v0.7.0".to_string(),
            ..Default::default()
        };
        assert!(
            !notes.has_renderable_body(),
            "the fixture must be the shape under test"
        );
        let text = flatten(&whats_new_lines(&notes, 60, &colors()));
        assert!(
            text.contains(dux_core::release_notes::NO_NOTES_EXPLANATION),
            "a blank body with no explanation: {text:?}"
        );
    }

    /// THE SHAPE THE RELEASE PIPELINE ACTUALLY PRODUCES, parsed rather than
    /// hand-built, because the hand-built fixture above cannot catch this one.
    /// `.github/workflows/release.yml` appends a horizontal rule before its
    /// `## Installation` section, so a one-line human note leaves the parser a
    /// headline plus a paragraph of `---`, and the screen rendered a title above a
    /// literal dash-run. Neither surface tested the pipeline's own output shape.
    #[test]
    fn the_body_the_release_pipeline_produces_explains_itself_rather_than_showing_a_rule() {
        let body =
            "## Quieter plumbing, louder failures\n\n---\n\n## Installation\n\nbrew install dux\n";
        let notes = ReleaseNotes {
            version: "v0.7.0".to_string(),
            html_url: "https://example.invalid/v0.7.0".to_string(),
            ..from_parsed(dux_core::release_notes::parse_release_body(body))
        };
        let text = flatten(&whats_new_lines(&notes, 60, &colors()));
        assert!(
            text.contains(dux_core::release_notes::NO_NOTES_EXPLANATION),
            "the appended rule was rendered as if it were notes: {text:?}"
        );
        assert!(
            !text.contains("---"),
            "a horizontal rule is not a body: {text:?}"
        );
    }

    /// Fills the parsed fields of a `ReleaseNotes`, so a test can start from a
    /// real release BODY rather than from fields it made up.
    fn from_parsed(parsed: dux_core::release_notes::ParsedBody) -> ReleaseNotes {
        ReleaseNotes {
            headline: parsed.headline,
            paragraphs: parsed.paragraphs,
            sections: parsed.sections,
            ..Default::default()
        }
    }

    /// A release body that parsed to one EMPTY section (a `### **__**` heading
    /// collapses to `""` once inline markup is stripped) rendered the "In this
    /// release" label above a single blank bullet. That is an empty screen with
    /// extra steps, so it takes the explanation path too.
    #[test]
    fn a_release_whose_only_section_is_blank_explains_itself_rather_than_showing_a_bullet() {
        let notes = ReleaseNotes {
            version: "v0.7.0".to_string(),
            sections: vec![String::new()],
            ..Default::default()
        };
        let text = flatten(&whats_new_lines(&notes, 60, &colors()));
        assert!(
            text.contains(dux_core::release_notes::NO_NOTES_EXPLANATION),
            "{text:?}"
        );
        assert!(
            !text.contains(IN_THIS_RELEASE),
            "a label over nothing is worse than no label: {text:?}"
        );
    }

    /// Real notes must NOT take the explanation path.
    #[test]
    fn a_release_with_real_notes_never_shows_the_no_notes_explanation() {
        let text = flatten(&whats_new_lines(&sample_notes(), 60, &colors()));
        assert!(
            !text.contains(dux_core::release_notes::NO_NOTES_EXPLANATION),
            "{text:?}"
        );
    }

    fn flatten(lines: &[Line<'static>]) -> String {
        lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// REGRESSION. The approved mock rendered the tagline and the release
    /// headline as one unwrapped span, and at the 90-column target the prose
    /// column is only ~50 wide, so the 72-character tagline was silently cut off
    /// mid-word at "...and a real term". Nobody noticed until the mock was
    /// actually rendered rather than read.
    ///
    /// This asserts the END of each string survives, which is the part clipping
    /// eats, and that no produced line overflows the column it was given.
    #[test]
    fn long_prose_wraps_instead_of_being_clipped_at_the_column_edge() {
        let colors = colors();
        let prose_cols = 50u16;

        let welcome = sample_welcome();
        assert!(
            welcome.tagline.chars().count() > prose_cols as usize,
            "only meaningful while the tagline overflows the column"
        );
        let lines = welcome_lines(&welcome, prose_cols, &colors);
        let flat: String = lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect::<Vec<_>>()
            .join(" ");
        let tail = welcome
            .tagline
            .rsplit_once(' ')
            .map(|(_, last)| last)
            .expect("tagline has spaces");
        assert!(
            flat.contains(tail),
            "the tagline was clipped before its final word {tail:?}: {flat}"
        );

        // A long release headline must survive the same way.
        let notes = sample_notes();
        let notes_lines = whats_new_lines(&notes, prose_cols, &colors);
        for group in [&lines, &notes_lines] {
            for line in group.iter() {
                let width: usize = line.spans.iter().map(|s| s.content.chars().count()).sum();
                assert!(
                    width <= prose_cols as usize,
                    "line is wider than its column and would be clipped: {line:?}"
                );
            }
        }
    }

    #[test]
    fn every_screen_renders_at_every_width_without_panicking() {
        let colors = colors();
        for w in [1u16, 4, 12, 20, 34, 60, 120] {
            assert!(
                !welcome_lines(&sample_welcome(), w, &colors).is_empty(),
                "{w}"
            );
            assert!(
                !whats_new_lines(&sample_notes(), w, &colors).is_empty(),
                "{w}"
            );
        }
    }

    // ── colors ───────────────────────────────────────────────────────────

    #[test]
    fn the_duck_takes_the_themes_leading_color_and_never_the_warning_amber() {
        let theme = Theme::default_dark();
        let colors = FirstLoadColors::from_theme(&theme);
        assert_eq!(
            colors.accent, theme.title_focused,
            "the duck must derive from accent.primary"
        );
        assert_ne!(
            colors.accent, theme.session_detached,
            "session_detached carries the WARNING semantic; the duck must not use it"
        );
        // The border stays the ordinary overlay border, not the accent, so the
        // composition does not flatten into one hue.
        assert_eq!(colors.rule, theme.overlay_border);
    }

    // ── render ───────────────────────────────────────────────────────────

    #[test]
    fn the_welcome_modal_paints_the_duck_both_buttons_and_the_prose() {
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::FirstLoad(FirstLoadPrompt::welcome(sample_welcome(), true));
        let rendered = render_prompt_to_string(&mut app, 120, 40);
        assert!(rendered.contains("Welcome to dux"), "title missing");
        assert!(rendered.contains('⣿'), "the duck must be painted");
        assert!(rendered.contains('│'), "the ruled divider must be painted");
        assert!(rendered.contains("Add a project"), "primary button missing");
        assert!(
            rendered.contains("Visit getdux.app"),
            "secondary button missing"
        );
        assert!(
            rendered.contains("One git worktree per coding agent"),
            "the tagline must be painted"
        );
    }

    #[test]
    fn the_whats_new_modal_paints_its_headline_features_and_buttons() {
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::FirstLoad(FirstLoadPrompt::whats_new(sample_notes(), true));
        let rendered = render_prompt_to_string(&mut app, 120, 40);
        assert!(
            rendered.contains("What's new in dux v0.7.0"),
            "title missing"
        );
        assert!(rendered.contains('⣿'), "the duck must be painted");
        assert!(rendered.contains("Quieter plumbing"), "headline missing");
        assert!(rendered.contains(IN_THIS_RELEASE), "release label missing");
        assert!(
            rendered.contains("Open full notes"),
            "primary button missing"
        );
        assert!(rendered.contains("Close"), "secondary button missing");
    }

    #[test]
    fn a_narrow_terminal_drops_the_duck_instead_of_squeezing_the_prose() {
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::FirstLoad(FirstLoadPrompt::welcome(sample_welcome(), true));
        // 60 columns of terminal leaves under 68 columns of inner width.
        let rendered = render_prompt_to_string(&mut app, 60, 30);
        assert!(
            !rendered.contains('⣿'),
            "the duck must be dropped, not squeezed alongside a ribbon of text"
        );
        assert!(
            rendered.contains("Add a project"),
            "the buttons must survive the narrow path"
        );
        assert!(
            rendered.contains("One git worktree per coding agent"),
            "the prose takes the full width instead"
        );
    }

    #[test]
    fn rendering_records_the_button_geometry_for_the_mouse() {
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::FirstLoad(FirstLoadPrompt::welcome(sample_welcome(), true));
        let _ = render_prompt_to_string(&mut app, 120, 40);
        match app.overlay_layout.active {
            OverlayMouseLayout::FirstLoad {
                primary_button,
                secondary_button,
            } => {
                assert!(primary_button.width > 0 && primary_button.height == 1);
                assert!(secondary_button.width > 0);
                assert!(secondary_button.x > primary_button.x + primary_button.width);
            }
            other => panic!("expected the first-load overlay layout, got {other:?}"),
        }
    }

    #[test]
    fn rendering_records_the_scroll_extent_so_keys_can_clamp() {
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::FirstLoad(FirstLoadPrompt::welcome(sample_welcome(), true));
        let _ = render_prompt_to_string(&mut app, 120, 40);
        assert!(app.last_first_load_height > 0);
        assert!(app.last_first_load_lines > 0);
    }

    // ── the gate's stamp timing ──────────────────────────────────────────

    #[test]
    fn a_shown_screen_stamps_only_on_dismissal() {
        let mut app = test_app(default_bindings());
        app.apply_first_load_plan(FirstLoadPlan {
            screen: FirstLoad::Welcome,
            mark_seen: true,
        });
        assert!(
            matches!(app.prompt, PromptState::FirstLoad(_)),
            "the welcome must be on screen"
        );
        assert_eq!(
            app.engine.session_store.last_seen_version().unwrap(),
            None,
            "a screen on the display must NOT have stamped yet"
        );

        app.dismiss_first_load_prompt();
        assert_eq!(
            app.engine
                .session_store
                .last_seen_version()
                .unwrap()
                .as_deref(),
            Some(dux_core::display_version()),
            "dismissal is what stamps"
        );
    }

    #[test]
    fn a_config_suppressed_plan_stamps_immediately() {
        let mut app = test_app(default_bindings());
        // What `plan(None, v, disable_welcome = true, _)` returns: nothing to
        // show, but keep the state moving forward.
        app.apply_first_load_plan(FirstLoadPlan {
            screen: FirstLoad::Nothing,
            mark_seen: true,
        });
        assert!(matches!(app.prompt, PromptState::None), "no screen");
        assert_eq!(
            app.engine
                .session_store
                .last_seen_version()
                .unwrap()
                .as_deref(),
            Some(dux_core::display_version()),
            "there is no screen to dismiss, so there is nothing to wait for"
        );
    }

    #[test]
    fn a_nothing_to_do_plan_writes_nothing() {
        let mut app = test_app(default_bindings());
        app.apply_first_load_plan(FirstLoadPlan {
            screen: FirstLoad::Nothing,
            mark_seen: false,
        });
        assert_eq!(app.engine.session_store.last_seen_version().unwrap(), None);
    }

    #[test]
    fn an_explicitly_opened_screen_never_stamps() {
        let mut app = test_app(default_bindings());
        app.show_welcome_screen_command().expect("open welcome");
        app.dismiss_first_load_prompt();
        assert_eq!(
            app.engine.session_store.last_seen_version().unwrap(),
            None,
            "an explicit look must not consume a pending upgrade's notes"
        );
    }

    #[test]
    fn a_transient_fetch_failure_shows_nothing_and_leaves_the_version_unmarked() {
        let mut app = test_app(default_bindings());
        app.pending_first_load = Some(FirstLoadPlan {
            screen: FirstLoad::WhatsNew,
            mark_seen: true,
        });
        app.apply_notes_fetch(NotesFetched {
            result: Err("connection refused".to_string()),
            outcome: NotesOutcome::TemporarilyUnavailable,
            purpose: NotesFetchPurpose::Automatic,
        });
        assert!(matches!(app.prompt, PromptState::None));
        assert_eq!(
            app.engine.session_store.last_seen_version().unwrap(),
            None,
            "marking seen here would hide this version's notes forever"
        );
    }

    #[test]
    fn a_definitive_404_shows_nothing_but_stamps_immediately() {
        let mut app = test_app(default_bindings());
        app.pending_first_load = Some(FirstLoadPlan {
            screen: FirstLoad::WhatsNew,
            mark_seen: true,
        });
        app.apply_notes_fetch(NotesFetched {
            result: Err("no such release".to_string()),
            outcome: NotesOutcome::NoSuchRelease,
            purpose: NotesFetchPurpose::Automatic,
        });
        assert!(matches!(app.prompt, PromptState::None));
        assert_eq!(
            app.engine
                .session_store
                .last_seen_version()
                .unwrap()
                .as_deref(),
            Some(dux_core::display_version()),
            "a definitive answer must not be re-asked every launch"
        );
    }

    #[test]
    fn a_successful_automatic_fetch_shows_the_screen_and_defers_the_stamp() {
        let mut app = test_app(default_bindings());
        app.pending_first_load = Some(FirstLoadPlan {
            screen: FirstLoad::WhatsNew,
            mark_seen: true,
        });
        app.apply_notes_fetch(NotesFetched {
            result: Ok(sample_notes()),
            outcome: NotesOutcome::Fetched,
            purpose: NotesFetchPurpose::Automatic,
        });
        assert!(matches!(app.prompt, PromptState::FirstLoad(_)));
        assert_eq!(app.engine.session_store.last_seen_version().unwrap(), None);
        app.dismiss_first_load_prompt();
        assert_eq!(
            app.engine
                .session_store
                .last_seen_version()
                .unwrap()
                .as_deref(),
            Some(dux_core::display_version())
        );
    }

    // ── a late fetch must not clobber a modal the user opened ────────────

    /// The palette, with a half-typed command in it: the shape of the
    /// in-progress input a late fetch would otherwise throw away. `PromptState`
    /// is a single slot, so writing the what's-new screen into it destroys this.
    fn half_typed_palette() -> PromptState {
        let mut input = TextInput::new();
        input.insert_char('n');
        input.insert_char('e');
        input.insert_char('w');
        PromptState::Command { input, selected: 0 }
    }

    fn palette_text(prompt: &PromptState) -> Option<String> {
        match prompt {
            PromptState::Command { input, .. } => Some(input.text.clone()),
            _ => None,
        }
    }

    #[test]
    fn a_late_fetch_never_replaces_a_modal_the_user_opened() {
        let mut app = test_app(default_bindings());
        app.pending_first_load = Some(FirstLoadPlan {
            screen: FirstLoad::WhatsNew,
            mark_seen: true,
        });
        app.prompt = half_typed_palette();

        app.apply_notes_fetch(NotesFetched {
            result: Ok(sample_notes()),
            outcome: NotesOutcome::Fetched,
            purpose: NotesFetchPurpose::Automatic,
        });

        assert_eq!(
            palette_text(&app.prompt).as_deref(),
            Some("new"),
            "the user's in-progress modal must survive the fetch landing"
        );
        assert_eq!(
            app.engine.session_store.last_seen_version().unwrap(),
            None,
            "nothing was shown, so nothing may be stamped"
        );
        assert!(
            app.pending_first_load.is_some(),
            "the plan must stay pending so the screen can still be offered"
        );
        assert!(
            app.deferred_first_load_notes.is_some(),
            "the fetched notes must be stashed so no refetch is needed"
        );
    }

    #[test]
    fn a_deferred_screen_is_offered_once_the_users_modal_closes() {
        let mut app = test_app(default_bindings());
        app.pending_first_load = Some(FirstLoadPlan {
            screen: FirstLoad::WhatsNew,
            mark_seen: true,
        });
        app.prompt = half_typed_palette();
        app.apply_notes_fetch(NotesFetched {
            result: Ok(sample_notes()),
            outcome: NotesOutcome::Fetched,
            purpose: NotesFetchPurpose::Automatic,
        });

        // A later tick with the modal still open changes nothing.
        app.drain_notes_fetch();
        assert!(palette_text(&app.prompt).is_some(), "still the palette");

        // The user closes their modal; the next tick offers the screen.
        app.prompt = PromptState::None;
        app.drain_notes_fetch();
        assert!(
            matches!(app.prompt, PromptState::FirstLoad(_)),
            "the deferred screen must be offered on a later tick, got {:?}",
            app.prompt
        );
        assert!(
            app.deferred_first_load_notes.is_none() && app.pending_first_load.is_none(),
            "the deferred state is consumed once the screen is on screen"
        );
        assert_eq!(
            app.engine.session_store.last_seen_version().unwrap(),
            None,
            "the stamp still waits for the dismissal"
        );

        app.dismiss_first_load_prompt();
        assert_eq!(
            app.engine
                .session_store
                .last_seen_version()
                .unwrap()
                .as_deref(),
            Some(dux_core::display_version()),
            "the stamp lands only on dismissal"
        );
    }

    // ── an explicit request must fail loudly ─────────────────────────────

    #[test]
    fn an_explicit_request_escalates_an_in_flight_automatic_fetch() {
        // The in-flight fetch's failure closure was built for the AUTOMATIC path,
        // which is silent. A user who asks while it is running must still be told
        // when it fails: the escalation flag is shared with the running thread.
        let flag = Arc::new(AtomicBool::new(false));
        let op = notes_status_op(NotesFetchPurpose::Automatic, flag.clone());
        flag.store(true, Ordering::SeqCst);
        let resolved = op.resolve(&Err(dux_core::release_notes::FetchError::Transient(
            anyhow::anyhow!("connection refused"),
        )));
        match resolved.outcome {
            dux_core::engine::Final::Message { tone, text, .. } => {
                assert_eq!(tone, dux_core::statusline::StatusTone::Error);
                assert!(text.contains("connection refused"), "{text}");
            }
            other => panic!("an escalated failure must surface an error, got {other:?}"),
        }
    }

    #[test]
    fn an_unescalated_automatic_failure_stays_silent() {
        let flag = Arc::new(AtomicBool::new(false));
        let op = notes_status_op(NotesFetchPurpose::Automatic, flag);
        let resolved = op.resolve(&Err(dux_core::release_notes::FetchError::Transient(
            anyhow::anyhow!("offline"),
        )));
        assert_eq!(
            resolved.outcome,
            dux_core::engine::Final::clear(),
            "an unsolicited background check must not toast a failure"
        );
    }

    #[test]
    fn the_explicit_command_marks_an_in_flight_fetch_as_awaited() {
        let mut app = test_app(default_bindings());
        // Stand in for a fetch in flight: a channel whose sender is still alive.
        let (tx, rx) = mpsc::channel::<NotesFetched>();
        app.notes_fetch_rx = Some(rx);
        app.show_release_notes_command().expect("command runs");
        assert!(
            app.notes_fetch_explicit_request.load(Ordering::SeqCst),
            "the running fetch must learn that someone is now waiting on it"
        );
        drop(tx);
    }

    #[test]
    fn an_escalated_automatic_fetch_takes_the_explicit_path() {
        let mut app = test_app(default_bindings());
        app.pending_first_load = Some(FirstLoadPlan {
            screen: FirstLoad::WhatsNew,
            mark_seen: true,
        });
        app.notes_fetch_explicit_request
            .store(true, Ordering::SeqCst);
        app.apply_notes_fetch(NotesFetched {
            result: Ok(sample_notes()),
            outcome: NotesOutcome::Fetched,
            purpose: NotesFetchPurpose::Automatic,
        });
        assert!(matches!(app.prompt, PromptState::FirstLoad(_)));
        app.dismiss_first_load_prompt();
        assert_eq!(
            app.engine.session_store.last_seen_version().unwrap(),
            None,
            "an explicit look must not consume a pending upgrade's notes"
        );
    }

    // ── the scroll marker lives in chrome, never on content ──────────────

    #[test]
    fn the_scroll_marker_never_occupies_a_cell_content_could_use() {
        // The marker sits in the modal's right BORDER column, which is chrome by
        // construction, so no content cell can ever collide with it. (The prose
        // column's own last column is NOT safe: `wrap` emits an over-long
        // unbreakable word as a line wider than the wrap width, and the
        // `Paragraph` then fills the pane's full width including that column.)
        for (aw, ah) in [(90u16, 21u16), (68, 21), (60, 21), (30, 12), (8, 6)] {
            let area = Rect::new(3, 2, aw, ah);
            // The two shapes the renderer actually produces: the prose column to
            // the right of the art + divider (wide path), and the full inner width
            // (narrow path, duck dropped). Both are inside the border ring, which
            // is what makes the border column safe.
            let inner_w = aw.saturating_sub(2);
            let art_cols = ART_WIDTH + ART_PADDING + RULE_COLS;
            let candidates = [
                Rect::new(
                    area.x + 1 + art_cols.min(inner_w),
                    area.y + 1,
                    inner_w.saturating_sub(art_cols),
                    ah.saturating_sub(4),
                ),
                Rect::new(area.x + 1, area.y + 1, inner_w, ah.saturating_sub(4)),
            ];
            for content in candidates {
                let marker = scroll_marker_rect(area, content);
                assert!(
                    marker.x >= content.x + content.width,
                    "marker at x={} is inside the content pane {content:?}",
                    marker.x
                );
                assert!(
                    marker.x < area.x + area.width,
                    "marker at x={} escaped the modal {area:?}",
                    marker.x
                );
            }
        }
    }

    #[test]
    fn an_overlong_unbreakable_word_keeps_every_column_it_is_given() {
        // The regression the marker's old bottom-right-of-content position would
        // cause: a word too long to wrap fills the pane's full width, and a marker
        // drawn in that pane's last column would silently eat a character of it.
        let mut notes = sample_notes();
        // One 140-column unbreakable token, which `wrap` cannot split, so it is
        // emitted as a single line WIDER than the wrap width and the `Paragraph`
        // fills the pane's full width with it. Repeated enough times to overflow
        // the pane, or the marker would never draw and the test would prove
        // nothing.
        let long = "supercalifragilisticexpialidocious-".repeat(4);
        notes.paragraphs = vec![long.clone(); 12];
        notes.sections = vec![long.clone(); 4];

        let mut app = test_app(default_bindings());
        app.prompt = PromptState::FirstLoad(FirstLoadPrompt::whats_new(notes, true));
        let backend = TestBackend::new(120, 40);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| app.render(frame))
            .expect("render frame");
        let buf = terminal.backend().buffer().clone();

        let area = first_load::modal_area(Rect::new(0, 0, 120, 40));
        // There must be something to scroll, or the marker never draws at all.
        assert!(
            app.last_first_load_lines > app.last_first_load_height,
            "the fixture must overflow the pane for this test to mean anything"
        );
        let marker = ["↓", "↑", "↕"];
        for x in area.x + 1..area.x + area.width - 1 {
            for y in area.y + 1..area.y + area.height - 1 {
                let symbol = buf[(x, y)].symbol();
                assert!(
                    !marker.contains(&symbol),
                    "the scroll marker landed at ({x},{y}), inside the modal's \
                     interior where content lives"
                );
            }
        }
        // ...and it really did draw, in the border column.
        let border_x = area.x + area.width - 1;
        let found =
            (area.y..area.y + area.height).any(|y| marker.contains(&buf[(border_x, y)].symbol()));
        assert!(
            found,
            "the marker must still be drawn, in the border column"
        );
    }

    // ── keys and mouse ───────────────────────────────────────────────────

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn focus_of(app: &App) -> FirstLoadButton {
        match &app.prompt {
            PromptState::FirstLoad(prompt) => prompt.focus,
            other => panic!("expected the first-load modal, got {other:?}"),
        }
    }

    fn scroll_of(app: &App) -> u16 {
        match &app.prompt {
            PromptState::FirstLoad(prompt) => prompt.scroll,
            other => panic!("expected the first-load modal, got {other:?}"),
        }
    }

    fn app_with_welcome() -> App {
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::FirstLoad(FirstLoadPrompt::welcome(sample_welcome(), true));
        app
    }

    #[test]
    fn q_dismisses_the_screen_rather_than_jumping_to_the_bottom_of_it() {
        // Deliberately UNLIKE the help overlay, where `q` is scroll-to-bottom.
        // This is the first screen a brand-new user ever sees, and `q` is the most
        // likely "get me out of this" keypress.
        let mut app = app_with_welcome();
        let _ = render_prompt_to_string(&mut app, 120, 40);
        app.handle_key(key(KeyCode::Char('q'))).expect("q");
        assert!(
            matches!(app.prompt, PromptState::None),
            "q must dismiss the modal, got {:?}",
            app.prompt
        );
        // It dismisses through the same path as Esc, so it stamps identically.
        assert_eq!(
            app.engine
                .session_store
                .last_seen_version()
                .unwrap()
                .as_deref(),
            Some(dux_core::display_version())
        );
    }

    #[test]
    fn ctrl_c_still_quits_dux_and_is_not_treated_as_a_dismissal() {
        // `Ctrl-c` is a PROCESS-level convention, not a UI gesture. Swallowing the
        // first press to close a modal reads as dux refusing to exit. Only the
        // modifier-free quit binding (`q`) dismisses; do not widen this.
        let mut app = app_with_welcome();
        let quit = app
            .handle_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
            .expect("ctrl-c");
        assert!(
            quit,
            "Ctrl-c must ask the run loop to exit, not be consumed as a dismissal"
        );

        // Contrast, so the test pins the DIFFERENCE and not just Ctrl-c's return:
        // the modifier-free binding dismisses and does NOT quit.
        let mut app = app_with_welcome();
        let quit = app
            .handle_key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE))
            .expect("q");
        assert!(!quit, "q must not quit dux");
        assert!(matches!(app.prompt, PromptState::None), "q dismisses");
    }

    #[test]
    fn end_still_scrolls_to_the_bottom_so_nothing_is_lost() {
        let mut app = app_with_welcome();
        let _ = render_prompt_to_string(&mut app, OVERFLOW_TERM.0, OVERFLOW_TERM.1);
        let max = app
            .last_first_load_lines
            .saturating_sub(app.last_first_load_height.max(1));
        assert!(max > 0);
        app.handle_key(key(KeyCode::End)).expect("end");
        assert!(
            matches!(app.prompt, PromptState::FirstLoad(_)),
            "End must not dismiss"
        );
        assert_eq!(scroll_of(&app), max);
    }

    #[test]
    fn esc_closes_the_screen_and_stamps_it() {
        let mut app = app_with_welcome();
        app.handle_key(key(KeyCode::Esc)).expect("handle esc");
        assert!(matches!(app.prompt, PromptState::None), "Esc must dismiss");
        assert_eq!(
            app.engine
                .session_store
                .last_seen_version()
                .unwrap()
                .as_deref(),
            Some(dux_core::display_version())
        );
    }

    #[test]
    fn tab_and_the_arrow_keys_move_between_the_two_buttons() {
        let mut app = app_with_welcome();
        assert_eq!(focus_of(&app), FirstLoadButton::Primary);
        for code in [
            KeyCode::Tab,
            KeyCode::BackTab,
            KeyCode::Right,
            KeyCode::Left,
            KeyCode::Char('l'),
            KeyCode::Char('h'),
        ] {
            let before = focus_of(&app);
            app.handle_key(key(code)).expect("handle focus key");
            assert_ne!(
                focus_of(&app),
                before,
                "{code:?} should move focus between the two buttons"
            );
        }
    }

    #[test]
    fn space_activates_the_focused_button() {
        // The primary on the welcome screen opens the project browser, replacing
        // this modal rather than stacking on it.
        let mut app = app_with_welcome();
        app.handle_key(key(KeyCode::Char(' '))).expect("space");
        assert!(
            matches!(app.prompt, PromptState::BrowseProjects { .. }),
            "Space on \"Add a project\" must open the project browser, got {:?}",
            app.prompt
        );
        // ...and it dismissed the screen on the way, so the version is stamped.
        assert_eq!(
            app.engine
                .session_store
                .last_seen_version()
                .unwrap()
                .as_deref(),
            Some(dux_core::display_version())
        );
    }

    #[test]
    fn space_on_the_secondary_button_of_the_whats_new_screen_just_closes_it() {
        let mut app = test_app(default_bindings());
        let mut prompt = FirstLoadPrompt::whats_new(sample_notes(), true);
        prompt.focus = FirstLoadButton::Secondary;
        app.prompt = PromptState::FirstLoad(prompt);
        app.handle_key(key(KeyCode::Char(' '))).expect("space");
        assert!(matches!(app.prompt, PromptState::None));
    }

    #[test]
    fn enter_activates_the_focused_button_exactly_like_space() {
        let mut app = app_with_welcome();
        app.handle_key(key(KeyCode::Enter)).expect("enter");
        assert!(matches!(app.prompt, PromptState::BrowseProjects { .. }));
    }

    #[test]
    fn the_scroll_keys_scroll_the_content_and_clamp_to_the_rendered_extent() {
        let mut app = app_with_welcome();
        // The extent is a render output, so render once to establish it.
        let _ = render_prompt_to_string(&mut app, OVERFLOW_TERM.0, OVERFLOW_TERM.1);
        let max = app
            .last_first_load_lines
            .saturating_sub(app.last_first_load_height.max(1));
        assert!(max > 0, "the welcome copy must overflow a 40-row terminal");

        assert_eq!(scroll_of(&app), 0);
        app.handle_key(key(KeyCode::Down)).expect("down");
        assert_eq!(scroll_of(&app), 1, "Down scrolls one line");
        app.handle_key(key(KeyCode::Char('j'))).expect("j");
        assert_eq!(scroll_of(&app), 2, "j scrolls one line");
        app.handle_key(key(KeyCode::Char('k'))).expect("k");
        assert_eq!(scroll_of(&app), 1);
        app.handle_key(key(KeyCode::Up)).expect("up");
        assert_eq!(scroll_of(&app), 0);
        app.handle_key(key(KeyCode::Up)).expect("up at the top");
        assert_eq!(scroll_of(&app), 0, "scrolling up at the top is a no-op");

        // Page keys always initiate scrolling, and nothing can run past the end.
        app.handle_key(key(KeyCode::PageDown)).expect("pagedown");
        assert!(scroll_of(&app) > 0 && scroll_of(&app) <= max);
        app.handle_key(key(KeyCode::End)).expect("end");
        assert_eq!(scroll_of(&app), max, "End lands on the last page");
        app.handle_key(key(KeyCode::Down)).expect("down at the end");
        assert_eq!(scroll_of(&app), max, "the scroll is clamped");
        app.handle_key(key(KeyCode::Home)).expect("home");
        assert_eq!(scroll_of(&app), 0);
    }

    #[test]
    fn the_wheel_scrolls_the_content() {
        let mut app = app_with_welcome();
        let _ = render_prompt_to_string(&mut app, OVERFLOW_TERM.0, OVERFLOW_TERM.1);
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 10,
            row: 10,
            modifiers: KeyModifiers::NONE,
        });
        assert!(scroll_of(&app) > 0, "the wheel must scroll the prose");
    }

    #[test]
    fn clicking_a_pill_focuses_it_and_activates_it() {
        let mut app = test_app(default_bindings());
        // The what's-new screen's secondary button is a plain dismiss, which makes
        // the assertion about the click alone rather than about a browser launch.
        app.prompt = PromptState::FirstLoad(FirstLoadPrompt::whats_new(sample_notes(), true));
        let _ = render_prompt_to_string(&mut app, 120, 40);
        let OverlayMouseLayout::FirstLoad {
            secondary_button, ..
        } = app.overlay_layout.active
        else {
            panic!("expected the first-load overlay layout");
        };
        let cx = secondary_button.x + secondary_button.width / 2;
        let cy = secondary_button.y;
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: cx,
            row: cy,
            modifiers: KeyModifiers::NONE,
        });
        app.handle_mouse(MouseEvent {
            kind: MouseEventKind::Up(MouseButton::Left),
            column: cx,
            row: cy,
            modifiers: KeyModifiers::NONE,
        });
        assert!(
            matches!(app.prompt, PromptState::None),
            "a click on Close must dismiss the modal, got {:?}",
            app.prompt
        );
    }

    #[test]
    fn an_explicit_fetch_failure_opens_no_screen() {
        let mut app = test_app(default_bindings());
        app.apply_notes_fetch(NotesFetched {
            result: Err("offline".to_string()),
            outcome: NotesOutcome::TemporarilyUnavailable,
            purpose: NotesFetchPurpose::Explicit,
        });
        assert!(matches!(app.prompt, PromptState::None));
        assert_eq!(app.engine.session_store.last_seen_version().unwrap(), None);
    }
}
