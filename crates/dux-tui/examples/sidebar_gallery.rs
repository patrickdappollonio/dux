//! Sidebar design gallery — a standalone playground for the dux agent-list row.
//!
//! Run it:  `cargo run --example sidebar_gallery -p dux-tui`
//! Quit:    `q` or `Esc`.   Cycle the selected/hovered rows: any arrow / j / k.
//!
//! It renders several rendering VARIANTS of the same agent row (the text content
//! is identical everywhere — only the layout, spacing, glyphs, and the
//! selected/hovered treatment differ) so we can compare them in a real terminal
//! with real colors and half-height blocks. This file is deliberately
//! self-contained (it hardcodes the dux_dark palette) so it never couples to the
//! app internals; edit the VARIANTS list and the `render_variant` match to try
//! new ideas.

use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

// ── dux_dark palette (literal RGB, matching the theme) ───────────────────────
const APP_BG: Color = Color::Rgb(20, 20, 20);
const SEL_BG: Color = Color::Rgb(0, 229, 229); // bright cyan
const SEL_FG: Color = Color::Rgb(20, 20, 20); // near-black
const MUTED: Color = Color::Rgb(100, 100, 100); // provider_label_fg
const NAME_FG: Color = Color::Rgb(210, 210, 210); // session_active-ish
const BORDER: Color = Color::Rgb(80, 80, 80);
const TITLE: Color = Color::Rgb(140, 140, 140);
// State-word colors, mirroring the web (Working=green, Needs you=cyan, etc.)
const C_WORKING: Color = Color::Rgb(84, 225, 185); // green/teal
const C_NEEDS: Color = Color::Rgb(150, 235, 245); // cyan-100
const C_IDLE: Color = MUTED;
const C_DETACHED: Color = Color::Rgb(200, 170, 70); // amber
const C_EXITED: Color = Color::Rgb(100, 100, 100);
const C_PR: Color = Color::Rgb(84, 225, 130); // pr open green
const HOVER_BG: Color = Color::Rgb(34, 34, 34); // faint row tint
const SEL_TINT: Color = Color::Rgb(6, 46, 46); // faint cyan tint (for bar/caret variants)
// Fainter selection backgrounds (page 2 explores "web-style faint bg" instead of
// the full-flood selection): a faint accent tint and a faint neutral tint.
const FAINT_ACCENT: Color = Color::Rgb(10, 44, 44); // faint teal wash
const FAINT_NEUTRAL: Color = Color::Rgb(40, 40, 40); // faint grey wash (web hover)

// The spinner + status glyphs used by the app.
const SPINNER: &str = "⠹";

#[derive(Clone, Copy, PartialEq)]
enum RowState {
    Normal,
    Selected,
    Hovered,
}

struct Agent {
    glyph: &'static str,
    glyph_color: Color,
    name: &'static str,
    name_color: Color,
    project: &'static str,
    state: &'static str,
    state_color: Color,
    branch: Option<&'static str>,
    tabs: Option<u8>,
    pr: Option<u32>,
}

fn samples() -> Vec<Agent> {
    vec![
        Agent {
            glyph: SPINNER,
            glyph_color: C_WORKING,
            name: "Auth refactor",
            name_color: NAME_FG,
            project: "dux",
            state: "Working",
            state_color: C_WORKING,
            branch: Some("feat/auth-v2"),
            tabs: Some(2),
            pr: Some(42),
        },
        Agent {
            glyph: "●",
            glyph_color: C_NEEDS,
            name: "og-images",
            name_color: NAME_FG,
            project: "website",
            state: "Needs you",
            state_color: C_NEEDS,
            branch: None,
            tabs: None,
            pr: None,
        },
        Agent {
            glyph: "●",
            glyph_color: NAME_FG,
            name: "server-mode",
            name_color: NAME_FG,
            project: "dux",
            state: "Idle",
            state_color: C_IDLE,
            branch: None,
            tabs: None,
            pr: None,
        },
        Agent {
            glyph: "◎",
            glyph_color: C_DETACHED,
            name: "refactor-store",
            name_color: NAME_FG,
            project: "dux",
            state: "Detached",
            state_color: C_DETACHED,
            branch: None,
            tabs: None,
            pr: None,
        },
        Agent {
            glyph: "○",
            glyph_color: C_EXITED,
            name: "old-spike",
            name_color: C_EXITED,
            project: "dux",
            state: "Exited",
            state_color: C_EXITED,
            branch: None,
            tabs: None,
            pr: None,
        },
    ]
}

const VARIANTS: &[&str] = &[
    // Page 1 — bolder / structural treatments.
    "A · Current (half-block padding)",
    "B · Left accent bar",
    "C · Caret + accent name",
    "D · Clean full-bg, tight",
    "E · Underline selection",
    "F · Rounded card",
    // Page 2 — faint-background treatments (web-hover feel).
    "G · Faint accent wash",
    "H · Faint neutral wash",
    "I · Faint wash + thin bar",
    "J · Faint wash + accent name",
    "K · Faint wash + accent glyph",
    "L · Faint wash, name row only",
    // Page 3 — box-drawing / shape structure.
    "M · Corner brackets ⌜⌟",
    "N · Rounded L-frame ╰─",
    "O · Double frame ╔╝",
    "P · Capsule pill ▐▌",
    "Q · Dotted dividers ┈ + rail",
    "R · Corner ticks ◤◢",
];

const PER_PAGE: usize = 6;

fn main() -> std::io::Result<()> {
    let mut terminal = ratatui::init();
    let mut selected = 0usize;
    let mut hovered = 2usize;
    let mut page = 0usize;
    let pages = VARIANTS.len().div_ceil(PER_PAGE);
    let n = samples().len();
    let res = loop {
        if let Err(e) = terminal.draw(|f| draw(f, page, selected, hovered)) {
            break Err(e);
        }
        match event::read() {
            Ok(Event::Key(k)) => match k.code {
                KeyCode::Char('q') | KeyCode::Esc => break Ok(()),
                KeyCode::Tab | KeyCode::Char(' ') => page = (page + 1) % pages,
                KeyCode::Down | KeyCode::Char('j') => selected = (selected + 1) % n,
                KeyCode::Up | KeyCode::Char('k') => selected = (selected + n - 1) % n,
                KeyCode::Right | KeyCode::Char('l') => hovered = (hovered + 1) % n,
                KeyCode::Left | KeyCode::Char('h') => hovered = (hovered + n - 1) % n,
                _ => {}
            },
            Ok(_) => {}
            Err(e) => break Err(e),
        }
    };
    ratatui::restore();
    res
}

fn draw(f: &mut Frame, page: usize, selected: usize, hovered: usize) {
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

    let pages = VARIANTS.len().div_ceil(PER_PAGE);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                " dux sidebar gallery ",
                Style::default().fg(SEL_FG).bg(SEL_BG),
            ),
            Span::styled(
                format!(
                    "  page {}/{} — {}",
                    page + 1,
                    pages,
                    match page {
                        0 => "bolder / structural",
                        1 => "faint-background (web-hover feel)",
                        _ => "box-drawing / shape structure",
                    }
                ),
                Style::default().fg(MUTED),
            ),
        ])),
        header,
    );
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(
            " Tab/Space next page · j/k move selection · h/l move hover · q quit ",
            Style::default().fg(MUTED),
        ))),
        footer,
    );

    // One page = a 3 columns x 2 rows grid of variant panels.
    let cols = 3usize;
    let rows = PER_PAGE.div_ceil(cols);
    let row_areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints(vec![Constraint::Ratio(1, rows as u32); rows])
        .split(body);
    for (r, row_area) in row_areas.iter().enumerate() {
        let col_areas = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(vec![Constraint::Ratio(1, cols as u32); cols])
            .split(*row_area);
        for (c, cell) in col_areas.iter().enumerate() {
            let idx = page * PER_PAGE + r * cols + c;
            if idx < VARIANTS.len() {
                render_panel(f, *cell, idx, selected, hovered);
            }
        }
    }
}

fn render_panel(f: &mut Frame, area: Rect, variant: usize, selected: usize, hovered: usize) {
    let block = Block::default()
        .title(Span::styled(
            format!(" {} ", VARIANTS[variant]),
            Style::default().fg(TITLE),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(BORDER));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let agents = samples();
    // Each variant lays its own rows out and reports the height it uses per row.
    render_variant(f, inner, variant, &agents, selected, hovered);
}

/// Row states resolved for a given index.
fn state_of(i: usize, selected: usize, hovered: usize) -> RowState {
    if i == selected {
        RowState::Selected
    } else if i == hovered {
        RowState::Hovered
    } else {
        RowState::Normal
    }
}

fn render_variant(
    f: &mut Frame,
    area: Rect,
    variant: usize,
    agents: &[Agent],
    selected: usize,
    hovered: usize,
) {
    // Row height by variant: D is a tight 2-line (no spacer); others use a
    // 3-line block (name, meta, spacer) so selection padding has room.
    let tight = variant == 3;
    let row_h: u16 = if tight { 2 } else { 3 };
    let x0 = area.x;
    let width = area.width;
    let mut y = area.y + if tight { 0 } else { 1 }; // a little top breathing room

    for (i, a) in agents.iter().enumerate() {
        if y + row_h > area.y + area.height {
            break;
        }
        let st = state_of(i, selected, hovered);
        match variant {
            0 => row_current(f, x0, y, width, a, st),
            1 => row_left_bar(f, x0, y, width, a, st),
            2 => row_caret(f, x0, y, width, a, st),
            3 => row_tight_fullbg(f, x0, y, width, a, st),
            4 => row_underline(f, x0, y, width, a, st),
            5 => row_card(f, x0, y, width, a, st),
            6 => row_wash(f, x0, y, width, a, st, FAINT_ACCENT, None, false),
            7 => row_wash(f, x0, y, width, a, st, FAINT_NEUTRAL, None, false),
            8 => row_wash(f, x0, y, width, a, st, FAINT_ACCENT, Some("▏"), false),
            9 => row_wash(f, x0, y, width, a, st, FAINT_ACCENT, None, true),
            10 => row_wash_accent_glyph(f, x0, y, width, a, st),
            11 => row_wash_name_only(f, x0, y, width, a, st),
            12 => row_corner_brackets(f, x0, y, width, a, st),
            13 => row_l_frame(f, x0, y, width, a, st),
            14 => row_double_frame(f, x0, y, width, a, st, area.y),
            15 => row_capsule(f, x0, y, width, a, st),
            16 => row_dotted_dividers(f, x0, y, width, a, st),
            17 => row_corner_ticks(f, x0, y, width, a, st),
            _ => {}
        }
        y += row_h;
    }
}

// ── Shared line builders (identical text content across variants) ────────────

fn line1_spans(a: &Agent, glyph_col: Color, name_col: Color) -> Vec<Span<'static>> {
    let mut spans = vec![
        Span::styled(format!("{} ", a.glyph), Style::default().fg(glyph_col)),
        Span::styled(a.name.to_string(), Style::default().fg(name_col)),
    ];
    if let Some(pr) = a.pr {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(format!("PR#{pr}"), Style::default().fg(C_PR)));
    }
    spans
}

fn line2_spans(a: &Agent, muted: Color) -> Vec<Span<'static>> {
    let sep = || Span::styled(" · ", Style::default().fg(muted));
    let mut spans = vec![
        Span::styled(format!("※ {}", a.project), Style::default().fg(muted)),
        sep(),
        Span::styled(a.state.to_string(), Style::default().fg(a.state_color)),
    ];
    if let Some(b) = a.branch {
        spans.push(sep());
        spans.push(Span::styled(b.to_string(), Style::default().fg(muted)));
    }
    if let Some(t) = a.tabs {
        spans.push(sep());
        spans.push(Span::styled(
            format!("{t} tabs"),
            Style::default().fg(muted),
        ));
    }
    spans
}

fn put_line(f: &mut Frame, x: u16, y: u16, width: u16, spans: Vec<Span<'static>>) {
    f.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect {
            x,
            y,
            width,
            height: 1,
        },
    );
}

fn fill_bg(f: &mut Frame, x: u16, y: u16, width: u16, bg: Color) {
    let buf = f.buffer_mut();
    for xx in x..x + width {
        buf[(xx, y)].set_bg(bg);
    }
}

fn set_row_style(f: &mut Frame, x: u16, y: u16, width: u16, style: Style) {
    let buf = f.buffer_mut();
    for xx in x..x + width {
        buf[(xx, y)].set_style(style);
    }
}

// ── Variant A: current — half-block padding, full-bg content rows ────────────
fn row_current(f: &mut Frame, x: u16, y: u16, width: u16, a: &Agent, st: RowState) {
    put_line(
        f,
        x + 1,
        y,
        width - 1,
        line1_spans(a, a.glyph_color, a.name_color),
    );
    put_line(f, x + 1, y + 1, width - 1, line2_spans(a, MUTED));
    match st {
        RowState::Selected => {
            let sel = Style::default()
                .fg(SEL_FG)
                .bg(SEL_BG)
                .add_modifier(Modifier::BOLD);
            set_row_style(f, x, y, width, sel);
            set_row_style(f, x, y + 1, width, sel);
            // top-half block below (padding), bottom-half block above.
            let buf = f.buffer_mut();
            for xx in x..x + width {
                buf[(xx, y + 2)].set_symbol("▀").set_fg(SEL_BG);
                if y > 0 {
                    buf[(xx, y - 1)].set_symbol("▄").set_fg(SEL_BG);
                }
            }
        }
        RowState::Hovered => {
            fill_bg(f, x, y, width, HOVER_BG);
            fill_bg(f, x, y + 1, width, HOVER_BG);
        }
        RowState::Normal => {}
    }
}

// ── Variant B: left accent bar + faint tint ──────────────────────────────────
fn row_left_bar(f: &mut Frame, x: u16, y: u16, width: u16, a: &Agent, st: RowState) {
    put_line(
        f,
        x + 2,
        y,
        width - 2,
        line1_spans(a, a.glyph_color, a.name_color),
    );
    put_line(f, x + 2, y + 1, width - 2, line2_spans(a, MUTED));
    match st {
        RowState::Selected => {
            let buf = f.buffer_mut();
            for yy in [y, y + 1] {
                buf[(x, yy)].set_symbol("▌").set_fg(SEL_BG);
                for xx in x + 1..x + width {
                    buf[(xx, yy)].set_bg(SEL_TINT);
                }
            }
        }
        RowState::Hovered => {
            fill_bg(f, x, y, width, HOVER_BG);
            fill_bg(f, x, y + 1, width, HOVER_BG);
        }
        RowState::Normal => {}
    }
}

// ── Variant C: caret gutter + accent name, no background ─────────────────────
fn row_caret(f: &mut Frame, x: u16, y: u16, width: u16, a: &Agent, st: RowState) {
    let (caret, name_col) = match st {
        RowState::Selected => ("❯ ", SEL_BG),
        _ => ("  ", a.name_color),
    };
    let mut l1 = vec![Span::styled(caret.to_string(), Style::default().fg(SEL_BG))];
    l1.extend(line1_spans(a, a.glyph_color, name_col));
    put_line(f, x, y, width, l1);
    put_line(f, x + 2, y + 1, width - 2, line2_spans(a, MUTED));
    if st == RowState::Hovered {
        put_line(f, x, y, width, {
            let mut v = vec![Span::styled("· ".to_string(), Style::default().fg(MUTED))];
            v.extend(line1_spans(a, a.glyph_color, a.name_color));
            v
        });
    }
}

// ── Variant D: tight 2-line, clean full-bg selection, no spacer ──────────────
fn row_tight_fullbg(f: &mut Frame, x: u16, y: u16, width: u16, a: &Agent, st: RowState) {
    put_line(
        f,
        x + 1,
        y,
        width - 1,
        line1_spans(a, a.glyph_color, a.name_color),
    );
    put_line(f, x + 1, y + 1, width - 1, line2_spans(a, MUTED));
    match st {
        RowState::Selected => {
            let sel = Style::default()
                .fg(SEL_FG)
                .bg(SEL_BG)
                .add_modifier(Modifier::BOLD);
            set_row_style(f, x, y, width, sel);
            set_row_style(f, x, y + 1, width, sel);
        }
        RowState::Hovered => {
            fill_bg(f, x, y, width, HOVER_BG);
            fill_bg(f, x, y + 1, width, HOVER_BG);
        }
        RowState::Normal => {}
    }
}

// ── Variant E: underline the selected row ────────────────────────────────────
fn row_underline(f: &mut Frame, x: u16, y: u16, width: u16, a: &Agent, st: RowState) {
    put_line(
        f,
        x + 1,
        y,
        width - 1,
        line1_spans(a, a.glyph_color, a.name_color),
    );
    put_line(f, x + 1, y + 1, width - 1, line2_spans(a, MUTED));
    match st {
        RowState::Selected => {
            let buf = f.buffer_mut();
            for xx in x..x + width {
                buf[(xx, y + 2)].set_symbol("─").set_fg(SEL_BG);
            }
            // brighten the name a touch
            put_line(
                f,
                x + 1,
                y,
                width - 1,
                line1_spans(a, a.glyph_color, SEL_BG),
            );
        }
        RowState::Hovered => {
            fill_bg(f, x, y, width, HOVER_BG);
            fill_bg(f, x, y + 1, width, HOVER_BG);
        }
        RowState::Normal => {}
    }
}

// ── Variant F: rounded card around the selected row ──────────────────────────
fn row_card(f: &mut Frame, x: u16, y: u16, width: u16, a: &Agent, st: RowState) {
    match st {
        RowState::Selected => {
            let card = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(SEL_BG));
            let r = Rect {
                x,
                y: y.saturating_sub(0),
                width,
                height: 3,
            };
            let inner = card.inner(r);
            f.render_widget(card, r);
            put_line(
                f,
                inner.x,
                inner.y,
                inner.width,
                line1_spans(a, a.glyph_color, a.name_color),
            );
            put_line(f, inner.x, inner.y + 1, inner.width, line2_spans(a, MUTED));
        }
        RowState::Hovered => {
            put_line(
                f,
                x + 1,
                y,
                width - 1,
                line1_spans(a, a.glyph_color, a.name_color),
            );
            put_line(f, x + 1, y + 1, width - 1, line2_spans(a, MUTED));
            fill_bg(f, x, y, width, HOVER_BG);
            fill_bg(f, x, y + 1, width, HOVER_BG);
        }
        RowState::Normal => {
            put_line(
                f,
                x + 1,
                y,
                width - 1,
                line1_spans(a, a.glyph_color, a.name_color),
            );
            put_line(f, x + 1, y + 1, width - 1, line2_spans(a, MUTED));
        }
    }
}

// ── Variant G/H/I/J: faint wash family ───────────────────────────────────────
// A faint background over the two content rows (text keeps its own colors, like
// the web hover), optionally with a thin left bar and/or the name in the accent.
#[allow(clippy::too_many_arguments)] // a scratch gallery knob, not production API
fn row_wash(
    f: &mut Frame,
    x: u16,
    y: u16,
    width: u16,
    a: &Agent,
    st: RowState,
    wash: Color,
    bar: Option<&str>,
    accent_name: bool,
) {
    let gutter = if bar.is_some() { 2 } else { 0 };
    let name_col = if accent_name && st == RowState::Selected {
        SEL_BG
    } else {
        a.name_color
    };
    put_line(
        f,
        x + gutter,
        y,
        width - gutter,
        line1_spans(a, a.glyph_color, name_col),
    );
    put_line(f, x + gutter, y + 1, width - gutter, line2_spans(a, MUTED));
    match st {
        RowState::Selected => {
            fill_bg(f, x, y, width, wash);
            fill_bg(f, x, y + 1, width, wash);
            if let Some(b) = bar {
                let buf = f.buffer_mut();
                for yy in [y, y + 1] {
                    buf[(x, yy)].set_symbol(b).set_fg(SEL_BG);
                }
            }
        }
        RowState::Hovered => {
            fill_bg(f, x, y, width, HOVER_BG);
            fill_bg(f, x, y + 1, width, HOVER_BG);
        }
        RowState::Normal => {}
    }
}

// ── Variant K: faint wash + the status glyph recolored to the accent ─────────
fn row_wash_accent_glyph(f: &mut Frame, x: u16, y: u16, width: u16, a: &Agent, st: RowState) {
    let glyph_col = if st == RowState::Selected {
        SEL_BG
    } else {
        a.glyph_color
    };
    put_line(f, x, y, width, line1_spans(a, glyph_col, a.name_color));
    put_line(f, x, y + 1, width, line2_spans(a, MUTED));
    match st {
        RowState::Selected => {
            fill_bg(f, x, y, width, FAINT_ACCENT);
            fill_bg(f, x, y + 1, width, FAINT_ACCENT);
        }
        RowState::Hovered => {
            fill_bg(f, x, y, width, HOVER_BG);
            fill_bg(f, x, y + 1, width, HOVER_BG);
        }
        RowState::Normal => {}
    }
}

// ── Variant L: faint wash on the name row only ───────────────────────────────
fn row_wash_name_only(f: &mut Frame, x: u16, y: u16, width: u16, a: &Agent, st: RowState) {
    put_line(f, x, y, width, line1_spans(a, a.glyph_color, a.name_color));
    put_line(f, x, y + 1, width, line2_spans(a, MUTED));
    match st {
        RowState::Selected => fill_bg(f, x, y, width, FAINT_ACCENT),
        RowState::Hovered => fill_bg(f, x, y, width, HOVER_BG),
        RowState::Normal => {}
    }
}

// ── Variant M: corner brackets ⌜ ⌝ ⌞ ⌟ around the selected row ───────────────
// Four quotation-corner ticks framing the two content rows, over a faint wash.
fn row_corner_brackets(f: &mut Frame, x: u16, y: u16, width: u16, a: &Agent, st: RowState) {
    put_line(
        f,
        x + 2,
        y,
        width - 2,
        line1_spans(a, a.glyph_color, a.name_color),
    );
    put_line(f, x + 2, y + 1, width - 2, line2_spans(a, MUTED));
    match st {
        RowState::Selected => {
            fill_bg(f, x, y, width, FAINT_ACCENT);
            fill_bg(f, x, y + 1, width, FAINT_ACCENT);
            let right = x + width - 1;
            let buf = f.buffer_mut();
            buf[(x, y)].set_symbol("⌜").set_fg(SEL_BG);
            buf[(right, y)].set_symbol("⌝").set_fg(SEL_BG);
            buf[(x, y + 1)].set_symbol("⌞").set_fg(SEL_BG);
            buf[(right, y + 1)].set_symbol("⌟").set_fg(SEL_BG);
        }
        RowState::Hovered => {
            fill_bg(f, x, y, width, HOVER_BG);
            fill_bg(f, x, y + 1, width, HOVER_BG);
        }
        RowState::Normal => {}
    }
}

// ── Variant N: rounded L-frame ╭ │ ╰─ (left rail + bottom rule) ──────────────
// A rounded left rail hugging the selected row, running out along the spacer
// row as a bottom rule — an open "L" rather than a closed box.
fn row_l_frame(f: &mut Frame, x: u16, y: u16, width: u16, a: &Agent, st: RowState) {
    put_line(
        f,
        x + 2,
        y,
        width - 2,
        line1_spans(a, a.glyph_color, a.name_color),
    );
    put_line(f, x + 2, y + 1, width - 2, line2_spans(a, MUTED));
    match st {
        RowState::Selected => {
            let buf = f.buffer_mut();
            buf[(x, y)].set_symbol("╭").set_fg(SEL_BG);
            buf[(x, y + 1)].set_symbol("│").set_fg(SEL_BG);
            buf[(x, y + 2)].set_symbol("╰").set_fg(SEL_BG);
            for xx in x + 1..x + width {
                buf[(xx, y + 2)].set_symbol("─").set_fg(SEL_BG);
            }
        }
        RowState::Hovered => {
            fill_bg(f, x, y, width, HOVER_BG);
            fill_bg(f, x, y + 1, width, HOVER_BG);
        }
        RowState::Normal => {}
    }
}

// ── Variant O: double-line frame ╔═╗ ║ ╚═╝ around the selected row ───────────
// A full double-rule box: top border on the row above (the previous spacer),
// side rails beside the content, bottom border on this row's spacer.
fn row_double_frame(f: &mut Frame, x: u16, y: u16, width: u16, a: &Agent, st: RowState, top: u16) {
    put_line(
        f,
        x + 2,
        y,
        width - 3,
        line1_spans(a, a.glyph_color, a.name_color),
    );
    put_line(f, x + 2, y + 1, width - 3, line2_spans(a, MUTED));
    match st {
        RowState::Selected => {
            let right = x + width - 1;
            let buf = f.buffer_mut();
            // Top border: only when the row above is still inside the panel.
            if y > top {
                buf[(x, y - 1)].set_symbol("╔").set_fg(SEL_BG);
                for xx in x + 1..right {
                    buf[(xx, y - 1)].set_symbol("═").set_fg(SEL_BG);
                }
                buf[(right, y - 1)].set_symbol("╗").set_fg(SEL_BG);
            }
            for yy in [y, y + 1] {
                buf[(x, yy)].set_symbol("║").set_fg(SEL_BG);
                buf[(right, yy)].set_symbol("║").set_fg(SEL_BG);
            }
            buf[(x, y + 2)].set_symbol("╚").set_fg(SEL_BG);
            for xx in x + 1..right {
                buf[(xx, y + 2)].set_symbol("═").set_fg(SEL_BG);
            }
            buf[(right, y + 2)].set_symbol("╝").set_fg(SEL_BG);
        }
        RowState::Hovered => {
            fill_bg(f, x, y, width, HOVER_BG);
            fill_bg(f, x, y + 1, width, HOVER_BG);
        }
        RowState::Normal => {}
    }
}

// ── Variant P: capsule pill with half-block caps ▐ … ▌ ───────────────────────
// A full-flood selection like variant D, but the row ends taper via half-block
// caps so the highlight reads as a rounded pill instead of a hard rectangle.
fn row_capsule(f: &mut Frame, x: u16, y: u16, width: u16, a: &Agent, st: RowState) {
    put_line(
        f,
        x + 2,
        y,
        width - 2,
        line1_spans(a, a.glyph_color, a.name_color),
    );
    put_line(f, x + 2, y + 1, width - 2, line2_spans(a, MUTED));
    match st {
        RowState::Selected => {
            let right = x + width - 1;
            let sel = Style::default()
                .fg(SEL_FG)
                .bg(SEL_BG)
                .add_modifier(Modifier::BOLD);
            set_row_style(f, x + 1, y, width - 2, sel);
            set_row_style(f, x + 1, y + 1, width - 2, sel);
            let buf = f.buffer_mut();
            for yy in [y, y + 1] {
                buf[(x, yy)].set_symbol("▐").set_fg(SEL_BG).set_bg(APP_BG);
                buf[(right, yy)]
                    .set_symbol("▌")
                    .set_fg(SEL_BG)
                    .set_bg(APP_BG);
            }
        }
        RowState::Hovered => {
            fill_bg(f, x, y, width, HOVER_BG);
            fill_bg(f, x, y + 1, width, HOVER_BG);
        }
        RowState::Normal => {}
    }
}

// ── Variant Q: dotted dividers ┈ between agents + heavy rail on selection ────
// Every agent gets a faint dotted rule on its spacer row, so the list reads as
// separated entries; the selected row adds a heavy left rail ┃ over a tint.
fn row_dotted_dividers(f: &mut Frame, x: u16, y: u16, width: u16, a: &Agent, st: RowState) {
    put_line(
        f,
        x + 2,
        y,
        width - 2,
        line1_spans(a, a.glyph_color, a.name_color),
    );
    put_line(f, x + 2, y + 1, width - 2, line2_spans(a, MUTED));
    {
        let buf = f.buffer_mut();
        for xx in x..x + width {
            buf[(xx, y + 2)].set_symbol("┈").set_fg(BORDER);
        }
    }
    match st {
        RowState::Selected => {
            let buf = f.buffer_mut();
            for yy in [y, y + 1] {
                buf[(x, yy)].set_symbol("┃").set_fg(SEL_BG);
                for xx in x + 1..x + width {
                    buf[(xx, yy)].set_bg(SEL_TINT);
                }
            }
        }
        RowState::Hovered => {
            fill_bg(f, x + 1, y, width - 1, HOVER_BG);
            fill_bg(f, x + 1, y + 1, width - 1, HOVER_BG);
        }
        RowState::Normal => {}
    }
}

// ── Variant R: two-tone corner ticks ◤ ◢ over a faint wash ───────────────────
// A triangular tick in the top-left and bottom-right corners of the selected
// row, on a faint accent wash — a diagonal "notched" selection.
fn row_corner_ticks(f: &mut Frame, x: u16, y: u16, width: u16, a: &Agent, st: RowState) {
    put_line(
        f,
        x + 2,
        y,
        width - 2,
        line1_spans(a, a.glyph_color, a.name_color),
    );
    put_line(f, x + 2, y + 1, width - 2, line2_spans(a, MUTED));
    match st {
        RowState::Selected => {
            fill_bg(f, x, y, width, FAINT_ACCENT);
            fill_bg(f, x, y + 1, width, FAINT_ACCENT);
            let right = x + width - 1;
            let buf = f.buffer_mut();
            buf[(x, y)].set_symbol("◤").set_fg(SEL_BG);
            buf[(right, y + 1)].set_symbol("◢").set_fg(SEL_BG);
        }
        RowState::Hovered => {
            fill_bg(f, x, y, width, HOVER_BG);
            fill_bg(f, x, y + 1, width, HOVER_BG);
        }
        RowState::Normal => {}
    }
}
