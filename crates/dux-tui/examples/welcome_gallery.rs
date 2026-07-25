//! Welcome / what's-new modal — a standalone playground for the two first-load
//! screens, which share one frame and differ only in text and buttons.
//!
//! Run it:   `cargo run --example welcome_gallery -p dux-tui`
//! With your own notes file:
//!           `cargo run --example welcome_gallery -p dux-tui -- path/to/notes.md`
//!
//! Keys:     `Tab`/`w` switch screen (welcome / what's new)
//!           `b` cycle the border treatment: rounded (today's modals), double,
//!               double+rounded-corner hybrid, thick, thick+rounded hybrid,
//!               quadrant-outside solid block
//!           `t` cycle the theme, which is what recolors the duck
//!           `j`/`k` or arrows scroll · `PgUp`/`PgDn` page · `h`/`l` move button
//!           `q`/`Esc` quit
//!
//! The layout is fixed: the duck in a left column, a ruled divider, scrollable
//! content on the right, and exactly two buttons. Border weight and theme are
//! live toggles because they are the two things still being chosen.
//!
//! Like `sidebar_gallery`, this file is deliberately self-contained: it hardcodes
//! the palette and its own copy of the duck art so it never couples to app
//! internals.
//!
//! COLOR: the duck takes each theme's LEADING color (`accent.primary`), never a
//! hardcoded orange, so it needs no new theme token. That also rules out
//! `session_detached`, the only amber in the theme, which carries the warning
//! semantic and would recolor the duck whenever a theme restyles warnings. The
//! modal border deliberately keeps the ordinary overlay border color: accenting
//! it too would make it the same hue as the duck and flatten the composition.

use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Paragraph};

// ── dux_dark palette (literal RGB, matching the theme) ───────────────────────
const APP_BG: Color = Color::Rgb(20, 20, 20);
const OVERLAY_BG: Color = Color::Rgb(26, 26, 26);
const DIM_FG: Color = Color::Rgb(58, 58, 58);
const MUTED: Color = Color::Rgb(100, 100, 100);
const BODY_FG: Color = Color::Rgb(190, 190, 190);
const NAME_FG: Color = Color::Rgb(210, 210, 210);
const BORDER: Color = Color::Rgb(80, 80, 80);
const ACCENT: Color = Color::Rgb(0, 229, 229); // bright cyan (selection accent)
const ACCENT_FG: Color = Color::Rgb(20, 20, 20); // near-black, on accent bg
/// Each theme's LEADING color, i.e. its `accent.primary`. These are the real
/// values read out of the bundled themes, not approximations.
///
/// The duck takes this rather than a hardcoded orange, so it belongs to whatever
/// theme the user runs and no new theme token is needed. `accent.primary` is
/// already the source for `dux.selection_bg`, `dux.title_focused`,
/// `dux.project_icon`, and `dux.help_section_header_fg`, so everything accented
/// in this modal moves together.
///
/// Worth noting: gruvbox-dark's leading color IS orange, so on that theme the
/// duck looks exactly like the original hardcoded idea.
const THEMES: &[(&str, Color)] = &[
    ("dux_dark", Color::Rgb(0, 229, 229)),
    ("gruvbox_dark", Color::Rgb(254, 128, 25)),
    ("catppuccin_mocha", Color::Rgb(203, 166, 247)),
    ("nord", Color::Rgb(136, 192, 208)),
    ("tokyo_night", Color::Rgb(187, 154, 247)),
    ("dracula", Color::Rgb(189, 147, 249)),
    ("everforest_dark", Color::Rgb(167, 192, 128)),
    ("solarized_dark", Color::Rgb(38, 139, 210)),
    ("rose_pine", Color::Rgb(196, 167, 231)),
];

const DUCK: &[&str] = &[
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
const ART_WIDTH: u16 = 33;

/// DECIDED: the modal keeps the border every dux overlay already uses, the
/// rounded single line. The alternatives were built and compared in a terminal
/// (double, thick, quadrant-outside solid block, and two hand-authored hybrids
/// pairing light arc corners with double/thick edges) and rounded won: it reads
/// as part of the app rather than as a stranger.
///
/// Recorded so nobody re-litigates it: there is NO rounded double border in
/// Unicode. The double box-drawing block ships only sharp corners (`\u{2554} \u{2557} \u{255a} \u{255d}`)
/// and the arc corners (`\u{256d} \u{256e} \u{2570} \u{256f}`) exist only at light weight, so any
/// "rounded double" necessarily mixes two line weights and shows a step where a
/// thin corner meets a heavy edge.
const MODAL_BORDER: border::Set = border::ROUNDED;

/// Target modal width in COLUMNS. The terminal has no pixels; at a typical cell
/// width near 8px, 700px lands around 88-90 columns, so 90 is the translation of
/// "start somewhat wide". Deliberately wider than a routine dialog because this
/// one carries the art column plus prose. `centered` clamps it to the terminal,
/// so a narrow window simply gets less.
const MODAL_COLS: u16 = 90;

/// Below this many columns of prose the art column is dropped entirely: a duck
/// plus a 20-column ribbon of text is worse than no duck.
const MIN_PROSE_COLS: u16 = 30;

/// Whether the art column fits alongside a readable prose column.
fn shows_art(inner_width: u16) -> bool {
    inner_width >= ART_WIDTH + 2 + 3 + MIN_PROSE_COLS
}

/// The version the running binary reports. Real builds get this from
/// `DUX_DISPLAY_VERSION`; a dev build reports "development" and shows no
/// what's-new screen automatically.
const SAMPLE_VERSION: &str = "v0.6.0";
const RELEASE_URL: &str = "https://github.com/patrickdappollonio/dux/releases/tag/v0.6.0";
const WEBSITE_URL: &str = "https://getdux.app";

// ── the two screens ──────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Screen {
    /// First ever launch: no stored version at all.
    Welcome,
    /// The stored version differs from the running one.
    WhatsNew,
}

impl Screen {
    fn title(self) -> String {
        match self {
            Screen::Welcome => " Welcome to dux ".to_string(),
            Screen::WhatsNew => format!(" What's new in dux {SAMPLE_VERSION} "),
        }
    }

    /// Exactly two buttons per screen; the first is the primary.
    fn buttons(self) -> [&'static str; 2] {
        match self {
            Screen::Welcome => ["Add a project", "Visit getdux.app"],
            Screen::WhatsNew => ["Open full notes", "Close"],
        }
    }

    /// The link the secondary/primary button opens, for the footer hint.
    fn link(self) -> &'static str {
        match self {
            Screen::Welcome => WEBSITE_URL,
            Screen::WhatsNew => RELEASE_URL,
        }
    }
}

/// The numbered getting-started sequence. Numbered because it genuinely is a
/// sequence: you cannot create an agent without a project, or launch without an
/// agent.
const STEPS: &[(&str, &str)] = &[
    (
        "Add a project",
        "Point dux at any git repo. Your checkout is left alone.",
    ),
    (
        "Create an agent",
        "It gets its own worktree and branch, so two agents never collide.",
    ),
    (
        "Launch",
        "Your provider CLI runs in a real terminal you can watch and type into.",
    ),
];

const TAGLINE: &str = "One git worktree per coding agent, and a real terminal to watch it work.";

/// Where this machine keeps its config. The real screen resolves this at
/// runtime: `~/.dux/` on macOS, `$XDG_CONFIG_HOME/dux/` or `~/.config/dux/` on
/// Linux. Hardcoded here only because the gallery is standalone.
const CONFIG_PATH: &str = "~/.config/dux/config.toml";

/// Prescriptive on purpose: a new user should learn the model, not just the
/// keys. The numbered steps below repeat this in short form so someone who skips
/// the prose can still start.
fn welcome_paragraphs() -> Vec<String> {
    vec![
        "Start by adding a project: point dux at any git repo you already have. \
         Then create agents on that project."
            .to_string(),
        "Each agent gets its own git worktree and a branch-style name, so several \
         agents work at the same time without touching each other's files, or your \
         checkout."
            .to_string(),
        "Every agent can run any AI CLI: Claude, Codex, OpenCode, Copilot, or \
         anything else you name in config. There is no protocol layer and no \
         adapter to write."
            .to_string(),
        format!(
            "Your config lives at {CONFIG_PATH} and was just created, fully \
             commented. That file is the documentation: every setting is explained \
             inline, so you never have to leave it to understand an option."
        ),
    ]
}

// ── notes model + parser ─────────────────────────────────────────────────────

/// The trimmed shape of a GitHub release body: everything the modal needs and
/// nothing it doesn't. In the real feature this lives in `dux-core` so the TUI
/// and the web render identical data and neither needs a Markdown renderer.
#[derive(Debug, Default, PartialEq)]
struct Notes {
    /// The release headline, from the leading `## ...` line.
    headline: String,
    /// Intro prose, one entry per paragraph, before the first `### ...`.
    paragraphs: Vec<String>,
    /// The `### ...` feature titles.
    sections: Vec<String>,
}

/// Splits a release body into headline, intro paragraphs, and feature titles.
///
/// Stops at the second `## ` heading, which is where the auto-generated
/// `## What's Changed` commit list and the workflow-appended `## Installation`
/// boilerplate begin. Everything after that is machine-written and not worth
/// showing in a modal.
fn parse_notes(body: &str) -> Notes {
    let mut notes = Notes::default();
    let mut para = String::new();
    let mut seen_top_heading = false;
    let mut in_code = false;

    for raw in body.lines() {
        let trimmed = raw.trim();

        if trimmed.starts_with("```") {
            in_code = !in_code;
            continue;
        }
        if in_code {
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("## ") {
            if seen_top_heading {
                break; // "What's Changed" / "Installation" — stop here.
            }
            seen_top_heading = true;
            notes.headline = strip_inline_markup(rest);
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("### ") {
            flush(&mut para, &mut notes.paragraphs);
            notes.sections.push(strip_inline_markup(rest));
            continue;
        }
        if trimmed.is_empty() {
            flush(&mut para, &mut notes.paragraphs);
            continue;
        }
        // Only collect prose before the first feature section; the bodies of the
        // sections themselves are what we deliberately drop.
        if notes.sections.is_empty() {
            if !para.is_empty() {
                para.push(' ');
            }
            para.push_str(trimmed);
        }
    }
    flush(&mut para, &mut notes.paragraphs);
    notes
}

fn flush(para: &mut String, out: &mut Vec<String>) {
    if !para.trim().is_empty() {
        out.push(strip_inline_markup(para.trim()));
    }
    para.clear();
}

/// Removes the Markdown syntax the modal cannot render, keeping the readable
/// text. Char-based throughout: release prose is full of multi-byte punctuation
/// and byte slicing would panic mid-character.
fn strip_inline_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '*' | '`' | '_' => {}
            '[' => {
                // `[text](url)` — keep `text`, drop the target.
                let mut j = i + 1;
                let mut text = String::new();
                while j < chars.len() && chars[j] != ']' {
                    text.push(chars[j]);
                    j += 1;
                }
                if j < chars.len() && chars.get(j + 1) == Some(&'(') {
                    let mut k = j + 2;
                    while k < chars.len() && chars[k] != ')' {
                        k += 1;
                    }
                    out.push_str(&text);
                    i = k + 1;
                    continue;
                }
                out.push('[');
            }
            c => out.push(c),
        }
        i += 1;
    }
    out
}

/// Char-safe greedy word wrap. Never slices by byte offset.
fn wrap(text: &str, width: usize) -> Vec<String> {
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

// ── main ─────────────────────────────────────────────────────────────────────

struct Ui {
    screen: Screen,
    theme: usize,
    scroll: u16,
    focus: usize,
}

impl Ui {
    /// The current theme's leading color. Everything accented in the modal, the
    /// duck included, derives from this one value.
    fn accent(&self) -> Color {
        THEMES[self.theme].1
    }
}

fn main() -> std::io::Result<()> {
    let body = match std::env::args().nth(1) {
        Some(p) => std::fs::read_to_string(p)?,
        None => include_str!("sample_release_notes.md").to_string(),
    };
    let notes = parse_notes(&body);

    let mut terminal = ratatui::init();
    let mut ui = Ui {
        screen: Screen::Welcome,
        theme: 0,
        scroll: 0,
        focus: 0,
    };
    let res = loop {
        if let Err(e) = terminal.draw(|f| draw(f, &notes, &ui)) {
            break Err(e);
        }
        match event::read() {
            Ok(Event::Key(k)) => match k.code {
                KeyCode::Char('q') | KeyCode::Esc => break Ok(()),
                KeyCode::Tab | KeyCode::Char('w') => {
                    ui.screen = match ui.screen {
                        Screen::Welcome => Screen::WhatsNew,
                        Screen::WhatsNew => Screen::Welcome,
                    };
                    ui.scroll = 0;
                    ui.focus = 0;
                }
                KeyCode::Char('t') => ui.theme = (ui.theme + 1) % THEMES.len(),
                KeyCode::Down | KeyCode::Char('j') => ui.scroll = ui.scroll.saturating_add(1),
                KeyCode::Up | KeyCode::Char('k') => ui.scroll = ui.scroll.saturating_sub(1),
                KeyCode::PageDown => ui.scroll = ui.scroll.saturating_add(8),
                KeyCode::PageUp => ui.scroll = ui.scroll.saturating_sub(8),
                KeyCode::Right | KeyCode::Char('l') => ui.focus = (ui.focus + 1) % 2,
                KeyCode::Left | KeyCode::Char('h') => ui.focus = (ui.focus + 1) % 2,
                _ => {}
            },
            Ok(_) => {}
            Err(e) => break Err(e),
        }
    };
    ratatui::restore();
    res
}

fn draw(f: &mut Frame, notes: &Notes, ui: &Ui) {
    let area = f.area();
    f.buffer_mut()
        .set_style(area, Style::default().bg(APP_BG).fg(NAME_FG));

    let [header, body, footer] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(4),
            Constraint::Length(1),
        ])
        .areas(area);

    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " dux first-load screens ",
                Style::default().fg(ACCENT_FG).bg(ACCENT),
            ),
            Span::styled(
                format!(
                    "  {}  ·  theme: ",
                    match ui.screen {
                        Screen::Welcome => "first launch",
                        Screen::WhatsNew => "version changed",
                    }
                ),
                Style::default().fg(MUTED),
            ),
            Span::styled(
                THEMES[ui.theme].0,
                Style::default()
                    .fg(ui.accent())
                    .add_modifier(Modifier::BOLD),
            ),
        ])),
        header,
    );
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " Tab screen · t theme (duck follows it) · j/k scroll · h/l button · q quit ",
            Style::default().fg(MUTED),
        ))),
        footer,
    );

    draw_app_skeleton(f, body, ui.screen);
    let modal = centered(body, MODAL_COLS, DUCK.len() as u16 + 6);
    draw_modal(f, modal, notes, ui);
}

/// A dimmed stand-in for the real three-pane layout, so the modal is judged
/// against the backdrop it will actually sit on. The first-launch case shows an
/// empty workspace, because that is what a new user has behind it.
fn draw_app_skeleton(f: &mut Frame, area: Rect, screen: Screen) {
    let [left, center, right] = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(32),
            Constraint::Min(20),
            Constraint::Length(30),
        ])
        .areas(area);
    let center_title = match screen {
        Screen::Welcome => " no agent selected ",
        Screen::WhatsNew => " agent · claude ",
    };
    for (rect, title) in [
        (left, " Projects "),
        (center, center_title),
        (right, " Changes "),
    ] {
        f.render_widget(
            Block::default()
                .title(Span::styled(title, Style::default().fg(DIM_FG)))
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(DIM_FG)),
            rect,
        );
    }
}

/// The one shared frame: duck on the left, ruled divider, scrollable content on
/// the right, exactly two buttons at the bottom.
fn draw_modal(f: &mut Frame, area: Rect, notes: &Notes, ui: &Ui) {
    f.render_widget(Clear, area);
    f.buffer_mut()
        .set_style(area, Style::default().bg(OVERLAY_BG).fg(BODY_FG));

    // The border keeps the standard overlay border color, exactly like every
    // other modal today. Accenting it as well would make it the same hue as the
    // duck and flatten the whole composition into one color.
    let block = Block::default()
        .title(Span::styled(
            ui.screen.title(),
            Style::default()
                .fg(ui.accent())
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_set(MODAL_BORDER)
        .border_style(Style::default().fg(BORDER));
    let inner = block.inner(area);
    f.render_widget(block, area);

    // On a narrow terminal the duck is dropped and the prose takes the whole
    // width, rather than both being squeezed.
    let right = if shows_art(inner.width) {
        let [art_col, rule_col, right] = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(ART_WIDTH + 2),
                Constraint::Length(3),
                Constraint::Min(MIN_PROSE_COLS),
            ])
            .areas(inner);

        // Duck, vertically centred in its column.
        let top = art_col.height.saturating_sub(DUCK.len() as u16) / 2;
        let art: Vec<Line> = std::iter::repeat_n(Line::from(""), top as usize)
            .chain(DUCK.iter().map(|l| {
                Line::from(Span::styled(
                    format!(" {l}"),
                    Style::default().fg(ui.accent()),
                ))
            }))
            .collect();
        f.render_widget(Paragraph::new(art), art_col);

        let rule: Vec<Line> = (0..rule_col.height)
            .map(|_| Line::from(Span::styled(" │ ", Style::default().fg(BORDER))))
            .collect();
        f.render_widget(Paragraph::new(rule), rule_col);
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

    let lines = match ui.screen {
        Screen::Welcome => welcome_lines(content.width, ui.accent()),
        Screen::WhatsNew => whats_new_lines(notes, content.width, ui.accent()),
    };
    render_scrollable(f, content, lines, ui.scroll, ui.accent());

    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(sep.width as usize),
            Style::default().fg(BORDER),
        ))),
        sep,
    );
    f.render_widget(
        Paragraph::new(button_row_with_link(
            ui.screen.buttons(),
            ui.focus,
            ui.screen.link(),
            buttons.width,
            ui.accent(),
        )),
        buttons,
    );
}

// ── content builders ─────────────────────────────────────────────────────────

fn welcome_lines(width: u16, accent: Color) -> Vec<Line<'static>> {
    let w = width.saturating_sub(1) as usize;
    // WRAP the tagline. As one span it is silently clipped: 72 chars into the
    // 50-column prose column, cutting off at "...and a real term".
    let mut lines: Vec<Line> = wrap(TAGLINE, w)
        .into_iter()
        .map(|l| {
            Line::from(Span::styled(
                l,
                Style::default().fg(NAME_FG).add_modifier(Modifier::BOLD),
            ))
        })
        .collect();
    lines.push(Line::from(""));
    for para in welcome_paragraphs() {
        for l in wrap(&para, w) {
            lines.push(Line::from(Span::styled(l, Style::default().fg(BODY_FG))));
        }
        lines.push(Line::from(""));
    }

    for (i, (title, desc)) in STEPS.iter().enumerate() {
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {} ", i + 1),
                Style::default()
                    .fg(ACCENT_FG)
                    .bg(accent)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                (*title).to_string(),
                Style::default().fg(NAME_FG).add_modifier(Modifier::BOLD),
            ),
        ]));
        for l in wrap(desc, w.saturating_sub(5)) {
            lines.push(Line::from(vec![
                Span::raw("     "),
                Span::styled(l, Style::default().fg(MUTED)),
            ]));
        }
        if i + 1 < STEPS.len() {
            lines.push(Line::from(""));
        }
    }
    lines
}

fn whats_new_lines(notes: &Notes, width: u16, accent: Color) -> Vec<Line<'static>> {
    let w = width.saturating_sub(1) as usize;
    // WRAP for the same reason as the tagline: release headlines routinely
    // exceed the prose column and would clip mid-word.
    let mut lines: Vec<Line> = wrap(&notes.headline, w)
        .into_iter()
        .map(|l| {
            Line::from(Span::styled(
                l,
                Style::default().fg(NAME_FG).add_modifier(Modifier::BOLD),
            ))
        })
        .collect();
    lines.push(Line::from(""));

    for para in &notes.paragraphs {
        for l in wrap(para, w) {
            lines.push(Line::from(Span::styled(l, Style::default().fg(BODY_FG))));
        }
        lines.push(Line::from(""));
    }

    if !notes.sections.is_empty() {
        // help_section_header_fg derives from accent.primary, so this label is
        // the leading color too.
        lines.push(Line::from(Span::styled(
            "In this release",
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        )));
        for s in &notes.sections {
            let avail = w.saturating_sub(4);
            for (i, l) in wrap(s, avail).into_iter().enumerate() {
                let prefix = if i == 0 { "  – " } else { "    " };
                lines.push(Line::from(vec![
                    Span::styled(prefix, Style::default().fg(MUTED)),
                    Span::styled(l, Style::default().fg(BODY_FG)),
                ]));
            }
        }
    }
    lines
}

// ── shared bits ──────────────────────────────────────────────────────────────

fn render_scrollable(
    f: &mut Frame,
    area: Rect,
    lines: Vec<Line<'static>>,
    scroll: u16,
    accent: Color,
) {
    // Reserve the last column for the scroll marker so it never paints over
    // prose. It previously overwrote the final character of the bottom row,
    // which is the line the reader is heading toward.
    let area = if lines.len() > area.height as usize {
        Rect::new(area.x, area.y, area.width.saturating_sub(1), area.height)
    } else {
        area
    };
    let total = lines.len();
    let vis = area.height as usize;
    let max_scroll = total.saturating_sub(vis);
    let off = (scroll as usize).min(max_scroll);
    let slice: Vec<Line> = lines.into_iter().skip(off).take(vis).collect();
    f.render_widget(Paragraph::new(slice), area);

    if max_scroll > 0 {
        let marker = if off == 0 {
            "↓"
        } else if off >= max_scroll {
            "↑"
        } else {
            "↕"
        };
        f.render_widget(
            Paragraph::new(Line::from(Span::styled(
                marker,
                Style::default().fg(accent),
            ))),
            Rect::new(
                area.x + area.width, // the reserved column, just past the prose
                area.y + area.height.saturating_sub(1),
                1,
                1,
            ),
        );
    }
}

/// Two pill buttons; the focused one takes the accent fill, mirroring the app's
/// confirm-button treatment.
fn button_row(labels: [&'static str; 2], focus: usize, accent: Color) -> Line<'static> {
    let mut spans: Vec<Span> = Vec::new();
    for (i, label) in labels.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw("  "));
        }
        let style = if i == focus {
            Style::default()
                .fg(ACCENT_FG)
                .bg(accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(MUTED)
        };
        spans.push(Span::styled(format!(" {label} "), style));
    }
    Line::from(spans)
}

/// The button row plus the destination the primary button opens, right-aligned
/// and muted, so the user can see where a link button will take them before
/// pressing it. Dropped when the column is too narrow to hold both.
fn button_row_with_link(
    labels: [&'static str; 2],
    focus: usize,
    link: &'static str,
    width: u16,
    accent: Color,
) -> Line<'static> {
    let row = button_row(labels, focus, accent);
    let used: usize = row.spans.iter().map(|s| s.content.chars().count()).sum();
    let link_len = link.chars().count();
    let mut spans = row.spans;
    if used + 2 + link_len <= width as usize {
        let pad = width as usize - used - link_len;
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled(link, Style::default().fg(DIM_FG)));
    }
    Line::from(spans)
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width.saturating_sub(w)) / 2,
        area.y + (area.height.saturating_sub(h)) / 2,
        w,
        h,
    )
}

// ── tests (run with `cargo test --examples -p dux-tui`) ──────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = include_str!("sample_release_notes.md");

    #[test]
    fn parses_headline_intro_and_sections_from_the_real_release_body() {
        let n = parse_notes(SAMPLE);
        assert_eq!(n.headline, "Quieter plumbing, louder failures");
        assert_eq!(n.paragraphs.len(), 2, "two intro paragraphs: {n:#?}");
        assert!(n.paragraphs[0].starts_with("Version 0.6.0 is a tune-up release"));
        assert_eq!(n.sections.len(), 6, "six feature titles: {:#?}", n.sections);
        assert_eq!(n.sections[0], "Environment config for agents and terminals");
        assert_eq!(n.sections[5], "A website exists now");
    }

    #[test]
    fn stops_before_the_autogenerated_boilerplate() {
        let n = parse_notes(SAMPLE);
        assert!(
            !n.sections.iter().any(|s| s.contains("Installation")),
            "must not reach the appended Installation section"
        );
        for p in &n.paragraphs {
            assert!(
                !p.contains("install.sh"),
                "install boilerplate leaked into the intro: {p}"
            );
        }
    }

    #[test]
    fn drops_code_fences_and_inline_markup() {
        let body = "## Title\n\nSome `code` and **bold** and a [link](https://x.dev).\n\n```toml\nkey = 1\n```\n\n### A feature\n";
        let n = parse_notes(body);
        assert_eq!(n.paragraphs, vec!["Some code and bold and a link."]);
        assert_eq!(n.sections, vec!["A feature"]);
    }

    #[test]
    fn wrap_is_char_safe_on_multibyte_prose() {
        // Box-drawing and CJK would panic under byte slicing.
        let lines = wrap("░██ 環境変数 とても長い行 ░██ ░██", 8);
        assert!(lines.iter().all(|l| l.chars().count() <= 8), "{lines:?}");
        assert_eq!(
            lines.concat().replace(' ', "").chars().count(),
            "░██環境変数とても長い行░██░██".chars().count()
        );
    }

    #[test]
    fn empty_body_yields_empty_notes() {
        assert_eq!(parse_notes(""), Notes::default());
    }

    #[test]
    fn each_screen_has_exactly_two_buttons_with_the_agreed_labels() {
        assert_eq!(
            Screen::Welcome.buttons(),
            ["Add a project", "Visit getdux.app"]
        );
        assert_eq!(Screen::WhatsNew.buttons(), ["Open full notes", "Close"]);
    }

    #[test]
    fn whats_new_title_carries_the_running_version() {
        assert!(
            Screen::WhatsNew.title().contains(SAMPLE_VERSION),
            "the title must name the version the user is on: {}",
            Screen::WhatsNew.title()
        );
    }

    #[test]
    fn welcome_links_to_the_website_and_whats_new_to_the_release() {
        assert_eq!(Screen::Welcome.link(), "https://getdux.app");
        assert!(Screen::WhatsNew.link().contains("/releases/tag/"));
    }

    #[test]
    fn the_welcome_copy_teaches_the_model_and_names_the_config_path() {
        let all = welcome_paragraphs().join(" ");
        for must in [
            "adding a project",
            "worktree",
            "branch-style name",
            "any AI CLI",
            CONFIG_PATH,
            "documentation",
        ] {
            assert!(
                all.contains(must),
                "welcome copy must mention {must:?}: {all}"
            );
        }
    }

    #[test]
    fn the_getting_started_steps_start_with_adding_a_project() {
        assert_eq!(STEPS.len(), 3);
        assert_eq!(STEPS[0].0, "Add a project");
        assert_eq!(STEPS[2].0, "Launch");
    }

    #[test]
    fn the_modal_starts_wide_and_drops_the_duck_only_when_prose_would_be_squeezed() {
        assert_eq!(MODAL_COLS, 90, "roughly 700px at a typical cell width");
        // Wide terminal: duck fits.
        assert!(shows_art(MODAL_COLS - 2));
        assert!(shows_art(68), "33 art + 2 pad + 3 rule + 30 prose");
        // Narrow terminal: prose wins.
        assert!(!shows_art(67));
        assert!(!shows_art(40));
    }

    #[test]
    fn the_tagline_wraps_instead_of_clipping_in_the_prose_column() {
        // Regression: at the 90-column target the prose column is 50 wide and the
        // tagline is 72 chars, so rendering it as one span cut it off mid-word.
        let prose_cols = 50usize;
        assert!(
            TAGLINE.chars().count() > prose_cols,
            "only meaningful while the tagline overflows"
        );
        let lines = welcome_lines(prose_cols as u16, ACCENT);
        assert!(
            lines.len() > 1,
            "the tagline must occupy more than one line"
        );
        for l in &lines {
            let w: usize = l.spans.iter().map(|s| s.content.chars().count()).sum();
            assert!(
                w <= prose_cols,
                "line would be clipped at {prose_cols}: {l:?}"
            );
        }
    }

    #[test]
    fn welcome_body_fits_a_narrow_column_without_panicking() {
        // The modal can be squeezed on a small terminal; wrapping must hold.
        for w in [12u16, 20, 34, 60] {
            let lines = welcome_lines(w, ACCENT);
            assert!(!lines.is_empty(), "width {w} produced nothing");
        }
    }

    #[test]
    fn the_duck_uses_a_themes_leading_color_and_never_a_hardcoded_one() {
        // dux_dark leads, so the gallery opens on the default theme.
        assert_eq!(THEMES[0].0, "dux_dark");
        assert_eq!(
            THEMES[0].1, ACCENT,
            "dux_dark's leading color is its accent"
        );
        // Every theme contributes a distinct leading color, which is the whole
        // point of deriving the duck from it.
        let mut seen = std::collections::HashSet::new();
        for (name, color) in THEMES {
            assert!(seen.insert(color), "duplicate leading color for {name}");
        }
        assert!(
            THEMES.len() >= 5,
            "keep a useful spread of themes to compare"
        );
    }

    #[test]
    fn gruvbox_leading_color_is_the_orange_the_design_started_from() {
        let gruvbox = THEMES
            .iter()
            .find(|(n, _)| *n == "gruvbox_dark")
            .expect("gruvbox_dark present");
        assert_eq!(gruvbox.1, Color::Rgb(254, 128, 25));
    }
}
