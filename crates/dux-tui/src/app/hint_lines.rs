//! Every key-hint line, rendered rather than reasoned about.
//!
//! A hint line is the `<key> what it does` row a dialog paints on its bottom
//! edge, a pane paints under its content, and the footer paints above the status
//! line. These tests paint every dialog the registry's fixtures build, the bars
//! and full-screen views that are not dialogs, and the pane hint lines, then read
//! every key badge back off the screen and hold it to three rules:
//!
//! * a badge sits on the surface it is painted over, never on a background of
//!   its own;
//! * every line has the one shape: `<key> desc` segments joined by two spaces,
//!   with `<a>/<b> desc` for two keys that do the same thing, and no prose
//!   punctuation between segments;
//! * with every action rebound, no badge names a key that is not the rebound
//!   one, except the few keys a surface handles without a binding.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Widget};

use super::modal::tests::every_prompt;
use super::test_support::{default_bindings, test_app};
use super::*;
use crate::keybindings::{BINDING_DEFS, RuntimeBindings};

/// Three backgrounds nothing else uses, so a badge painted on the wrong one
/// cannot pass by coincidence (in the bundled theme all three are one color).
const APP_BG: Color = Color::Rgb(11, 12, 13);
const OVERLAY_BG: Color = Color::Rgb(31, 32, 33);
const BAR_BG: Color = Color::Rgb(51, 52, 53);

/// Bracket and key colors nothing else on screen uses, so a `<` is a badge's
/// only when it is painted in one of them.
const BRACKET: Color = Color::Rgb(201, 1, 1);
const KEY: Color = Color::Rgb(202, 2, 2);
const DIM_BRACKET: Color = Color::Rgb(101, 1, 1);
const DIM_KEY: Color = Color::Rgb(102, 2, 2);

fn distinct_colors(app: &mut App) {
    app.theme.app_bg = APP_BG;
    app.theme.overlay_bg = OVERLAY_BG;
    app.theme.hint_bar_bg = BAR_BG;
    app.theme.hint_bracket_fg = BRACKET;
    app.theme.hint_key_fg = KEY;
    app.theme.hint_dim_bracket_fg = DIM_BRACKET;
    app.theme.hint_dim_key_fg = DIM_KEY;
}

/// Every action bound to one key nobody would pick by default, all different.
fn unusual_bindings() -> RuntimeBindings {
    RuntimeBindings::new(
        |action| {
            let index = BINDING_DEFS
                .iter()
                .position(|def| def.action == action)
                .expect("every action has a definition");
            let f = u8::try_from(index + 1).expect("fewer than 256 actions");
            vec![crokey::KeyCombination::new(
                KeyCode::F(f),
                KeyModifiers::CONTROL | KeyModifiers::ALT,
            )]
        },
        true,
    )
}

/// Every label the rebound bindings can print.
fn rebound_labels(bindings: &RuntimeBindings) -> Vec<String> {
    BINDING_DEFS
        .iter()
        .map(|def| bindings.label_for(def.action))
        .collect()
}

#[derive(Debug)]
struct Badge {
    x: u16,
    y: u16,
    /// Column of the closing bracket.
    end: u16,
    label: String,
    dim: bool,
}

fn screen(buf: &Buffer) -> String {
    let width = usize::from(buf.area.width);
    buf.content()
        .iter()
        .map(|c| c.symbol().to_string())
        .collect::<Vec<_>>()
        .chunks(width)
        .map(|row| row.concat())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every key badge on screen: a `<` in a bracket color, bold key cells, and the
/// matching `>`.
fn badges(buf: &Buffer) -> Vec<Badge> {
    let mut found = Vec::new();
    for y in 0..buf.area.height {
        let mut x = 0;
        while x < buf.area.width {
            let cell = &buf[(x, y)];
            let dim = match cell.fg {
                BRACKET => false,
                DIM_BRACKET => true,
                _ => {
                    x += 1;
                    continue;
                }
            };
            if cell.symbol() != "<"
                || x + 1 >= buf.area.width
                || !buf[(x + 1, y)].modifier.contains(Modifier::BOLD)
            {
                x += 1;
                continue;
            }
            // The key runs in the key colors up to the closing bracket; a badge
            // cut off by the edge of what it is painted in has none, and is
            // not read.
            let key_end = (x + 1..buf.area.width)
                .find(|&cx| !matches!(buf[(cx, y)].fg, KEY | DIM_KEY))
                .unwrap_or(buf.area.width);
            let closed = key_end < buf.area.width
                && buf[(key_end, y)].symbol() == ">"
                && buf[(key_end, y)].fg == cell.fg;
            if !closed {
                x = key_end;
                continue;
            }
            let end = key_end;
            let label: String = (x + 1..end).map(|cx| buf[(cx, y)].symbol()).collect();
            found.push(Badge {
                x,
                y,
                end,
                label,
                dim,
            });
            x = end + 1;
        }
    }
    found
}

fn render_at(app: &mut App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|frame| app.render(frame)).expect("render");
    terminal.backend().buffer().clone()
}

/// Paint one line on a surface of `bg`, the way a pane paints its hint row.
fn paint_line(line: Line<'static>, bg: Color) -> Buffer {
    let area = Rect::new(0, 0, 240, 1);
    let mut buf = Buffer::empty(area);
    Paragraph::new(line)
        .style(ratatui::style::Style::default().bg(bg))
        .render(area, &mut buf);
    buf
}

/// A surface on screen: a name for failure messages, and what was painted.
struct Painted {
    name: String,
    buf: Buffer,
    /// Whether this surface must show hints of its own (a line nothing else on
    /// screen paints).
    must_hint: bool,
    /// Keys the surface handles without a binding, which may therefore be
    /// named as they are.
    fixed: &'static [&'static str],
}

/// The prompts that show no hint line at all. A confirmation's keys are its
/// buttons, which say what they do themselves; an error dialog names its scroll
/// keys only once the message is long enough to scroll, and the fixture's is
/// not.
const HINTLESS_PROMPTS: &[&str] = &[
    "AddProjectFailed",
    "ConfigReloadFailed",
    "ConfirmCheckoutDefaultBranch",
    "ConfirmCloseTab",
    "ConfirmCreateInitialCommit",
    "ConfirmDeleteAgent",
    "ConfirmDeleteProject",
    "ConfirmDeleteTerminal",
    "ConfirmDeleteWorktree",
    "ConfirmDetachAgent",
    "ConfirmDiscardFile",
    "ConfirmInitRepo",
    "ConfirmKillRunning",
    "ConfirmNonDefaultBranch",
    "ConfirmQuit",
    "ConfirmRecreateWorkingCopy",
    "ConfirmRemoveProject",
    "ConfirmUseExistingBranch",
    "EditMacros(delete-confirm)",
];

/// Keys a surface handles itself, not through a binding, by fixture name.
fn fixed_keys(name: &str) -> &'static [&'static str] {
    match name {
        // Completion is the literal Tab key in the palette's filter.
        "Command" => &["Tab"],
        "BrowseProjects(path)" => &["Tab", "Enter"],
        "DebugInput" => &["Esc", "Scroll"],
        "ResourceMonitor" => &["Scroll"],
        "MacroBar" => &["Enter", "Tab", "Esc"],
        "FilesSearching" => &["Enter", "Esc"],
        "Help" | "HelpScrolled" => &["Space", "Ctrl-x"],
        _ => &[],
    }
}

/// Everything that paints a hint line, painted with `bindings`.
fn every_surface(bindings: fn() -> RuntimeBindings, width: u16, height: u16) -> Vec<Painted> {
    let mut out = Vec::new();
    let fresh = || {
        let mut app = test_app(bindings());
        distinct_colors(&mut app);
        app
    };

    let mut app = fresh();
    let baseline = render_at(&mut app, width, height);
    // The baseline every other surface's own badges are counted against, so it
    // cannot be asked to have badges of its own; the badge scan below still
    // reads its footer and pane hint lines.
    out.push(Painted {
        name: "main screen".to_string(),
        buf: baseline,
        must_hint: false,
        fixed: &[],
    });

    let mut prompts: Vec<(String, PromptState)> = every_prompt(&app)
        .into_iter()
        .map(|(name, prompt)| (name.to_string(), prompt))
        .collect();
    for (name, prompt) in every_prompt(&app) {
        match prompt {
            PromptState::BrowseProjects {
                purpose,
                current_dir,
                entries,
                loading,
                selected,
                filter,
                path_input,
                tab_completions,
                tab_index,
                ..
            } => {
                for (label, searching, editing_path) in [
                    ("BrowseProjects(search)", true, false),
                    ("BrowseProjects(path)", false, true),
                ] {
                    prompts.push((
                        label.to_string(),
                        PromptState::BrowseProjects {
                            purpose,
                            current_dir: current_dir.clone(),
                            entries: entries.clone(),
                            loading,
                            selected,
                            filter: filter.clone(),
                            searching,
                            editing_path,
                            path_input: path_input.clone(),
                            tab_completions: tab_completions.clone(),
                            tab_index,
                        },
                    ));
                }
            }
            PromptState::AddProjectFailed { return_prompt, .. } => {
                // Long enough to scroll, which is when the scroll keys are named.
                prompts.push((
                    "AddProjectFailed(long)".to_string(),
                    PromptState::AddProjectFailed {
                        message: (0..120)
                            .map(|i| format!("reason {i}"))
                            .collect::<Vec<_>>()
                            .join("\n"),
                        return_prompt,
                        scroll: 0,
                    },
                ));
            }
            PromptState::KillRunning(mut prompt) if name == "KillRunning" => {
                prompt.list.searching = true;
                prompts.push((
                    "KillRunning(search)".to_string(),
                    PromptState::KillRunning(prompt),
                ));
            }
            _ => {}
        }
    }
    for (name, prompt) in prompts {
        let mut app = fresh();
        app.prompt = prompt;
        let buf = render_at(&mut app, width, height);
        out.push(Painted {
            must_hint: !HINTLESS_PROMPTS.contains(&name.as_str()),
            fixed: fixed_keys(&name),
            name,
            buf,
        });
    }

    let mut app = fresh();
    app.macro_bar = Some(MacroBarState {
        input: TextInput::new(),
        selected: 0,
        previous_input_target: InputTarget::None,
    });
    out.push(Painted {
        name: "MacroBar".to_string(),
        buf: render_at(&mut app, width, height),
        must_hint: true,
        fixed: fixed_keys("MacroBar"),
    });

    for searching in [false, true] {
        let mut app = fresh();
        app.fullscreen_overlay = FullscreenOverlay::StartupLog;
        app.startup_log_viewer = Some(StartupLogViewer {
            scope_label: "project demo".to_string(),
            path: None,
            display_name: "startup.log".to_string(),
            content: (0..200)
                .map(|i| format!("log line {i}"))
                .collect::<Vec<_>>()
                .join("\n"),
            scroll_offset: 0,
            wrap_width: 0,
            search: TextInput::new(),
            searching,
            return_to: None,
        });
        out.push(Painted {
            name: format!("StartupLog(searching: {searching})"),
            buf: render_at(&mut app, width, height),
            must_hint: true,
            fixed: &[],
        });
    }

    for scroll in [0u16, 3] {
        let mut app = fresh();
        app.focus = FocusPane::Center;
        app.center_mode = CenterMode::Diff {
            lines: Arc::new((0..200).map(|i| Line::from(format!("line {i}"))).collect()),
            scroll,
            gutter_width: 0,
            worktree_path: "/tmp/does-not-matter".to_string(),
            rel_path: "src/main.rs".to_string(),
        };
        out.push(Painted {
            name: format!("Diff(scroll {scroll})"),
            buf: render_at(&mut app, width, height),
            must_hint: true,
            fixed: &[],
        });
    }

    for (name, scroll) in [("Help", 0u16), ("HelpScrolled", 4)] {
        let mut app = fresh();
        app.help_scroll = Some(scroll);
        out.push(Painted {
            name: name.to_string(),
            buf: render_at(&mut app, width, height),
            must_hint: true,
            fixed: fixed_keys(name),
        });
    }

    // The pane hint lines, painted on the pane's own surface.
    let app = fresh();
    let context =
        |surface, is_input, session_active, session_id: Option<&str>| AgentTerminalContext {
            active_surface: surface,
            terminal_status: CompanionTerminalStatus::Running,
            is_input,
            receives_keys: is_input,
            session_id: session_id.map(str::to_string),
            focused_tab: None,
            provider_name: None,
            session_active,
        };
    let pane_lines: Vec<(&str, Line<'static>, &'static [&'static str])> = vec![
        ("scroll mode cue", app.scroll_mode_cue_line(200), &[]),
        (
            "typeable",
            app.typeable_hint_line(SessionSurface::Agent, 200),
            &[],
        ),
        ("take-over", app.takeover_hint_line(200), &[]),
        (
            "interactive",
            app.interactive_terminal_hint_line(0, 200),
            &[],
        ),
        (
            "interactive scrolled",
            app.interactive_terminal_hint_line(3, 200),
            &[],
        ),
        ("scrolled", app.scrolled_terminal_hint_line(3, 200), &[]),
        (
            "inactive live agent",
            app.inactive_terminal_hint_line(
                &context(SessionSurface::Agent, false, true, Some("session-1")),
                200,
            ),
            &[],
        ),
        (
            "inactive exited agent",
            app.inactive_terminal_hint_line(
                &context(SessionSurface::Agent, false, false, Some("session-1")),
                200,
            ),
            &[],
        ),
        ("files", app.files_hint_line(200), &[]),
        ("commit (unfocused)", app.commit_hint_line(false, 200), &[]),
        ("commit (focused)", app.commit_hint_line(true, 200), &[]),
    ];
    for (name, line, fixed) in pane_lines {
        out.push(Painted {
            name: format!("pane line: {name}"),
            buf: paint_line(line, APP_BG),
            must_hint: true,
            fixed,
        });
    }
    let mut app = fresh();
    app.files_search_active = true;
    out.push(Painted {
        name: "pane line: files (searching)".to_string(),
        buf: paint_line(app.files_hint_line(200), APP_BG),
        must_hint: true,
        fixed: fixed_keys("FilesSearching"),
    });

    // The branches the plain fixture never reaches: a search that has matches
    // to step through, macros to open, a second tab to switch to, and Tab
    // handed to the agent so the pane chords are named instead.
    let mut app = fresh();
    app.files_search.set_text("src".to_string());
    out.push(Painted {
        name: "pane line: files (with a search)".to_string(),
        buf: paint_line(app.files_hint_line(200), APP_BG),
        must_hint: true,
        fixed: &[],
    });
    let mut app = fresh();
    app.engine.config.macros.entries.insert(
        "greet".to_string(),
        crate::config::MacroEntry {
            text: "hello".to_string(),
            surface: crate::config::MacroSurface::Both,
        },
    );
    let session_id = app.engine.sessions[0].id.clone();
    app.engine.agent_tabs.insert(
        TabId::new("second-tab"),
        crate::model::AgentTab {
            id: "second-tab".to_string(),
            session_id,
            provider: crate::model::ProviderKind::from_str("codex"),
            sort_order: 1,
            created_at: chrono::Utc::now(),
        },
    );
    app.engine.config.ui.tab_reaches_agent = true;
    for (name, line) in [
        (
            "typeable (macros, tabs, Tab to the agent)",
            app.typeable_hint_line(SessionSurface::Agent, 200),
        ),
        (
            "interactive (macros)",
            app.interactive_terminal_hint_line(3, 200),
        ),
    ] {
        out.push(Painted {
            name: format!("pane line: {name}"),
            buf: paint_line(line, APP_BG),
            must_hint: true,
            fixed: &[],
        });
    }

    // The footer in each focus context it has hints for.
    type Setup = fn(&mut App);
    let footer_contexts: [(&str, Setup); 5] = [
        ("footer: project row", |app| {
            app.focus = FocusPane::Left;
            app.selected_left = 0;
        }),
        ("footer: agent row", |app| {
            app.focus = FocusPane::Left;
            app.selected_left = app.left_items().len().saturating_sub(1);
        }),
        ("footer: terminals", |app| {
            app.focus = FocusPane::Left;
            app.left_section = LeftSection::Terminals;
        }),
        ("footer: center", |app| app.focus = FocusPane::Center),
        ("footer: changes", |app| app.focus = FocusPane::Files),
    ];
    for (name, setup) in footer_contexts {
        let mut app = fresh();
        setup(&mut app);
        out.push(Painted {
            name: name.to_string(),
            buf: render_at(&mut app, width, height),
            must_hint: false,
            fixed: &[],
        });
    }
    out
}

/// Whether the text between two badges is a join the one style allows: `/`
/// between two keys that do the same thing, or one space, a description with
/// no prose punctuation, and the two-space separator.
fn join_is_allowed(gap: &str) -> bool {
    if gap == "/" {
        return true;
    }
    let Some(body) = gap.strip_prefix(' ') else {
        return false;
    };
    let Some(desc) = body.strip_suffix("  ") else {
        return false;
    };
    !desc.is_empty()
        && !desc.starts_with(' ')
        && !desc.ends_with(' ')
        && !desc.contains("   ")
        && !desc.contains([',', '.', ':'])
}

/// A badge's cells all sit on the background right after it, which is the
/// surface the line is painted over.
#[test]
fn every_hint_badge_sits_on_the_surface_it_is_painted_over() {
    let mut offenders = Vec::new();
    for (width, height) in [(160, 60), (80, 24)] {
        for surface in every_surface(default_bindings, width, height) {
            let buf = &surface.buf;
            for badge in badges(buf) {
                let after = badge.end + 1;
                if after >= buf.area.width {
                    continue;
                }
                let surface_bg = buf[(after, badge.y)].bg;
                for x in badge.x..=badge.end {
                    let bg = buf[(x, badge.y)].bg;
                    if bg != surface_bg {
                        offenders.push(format!(
                            "{} at {width}x{height}: <{}> at ({},{}) has background {bg:?} on a \
                             {surface_bg:?} surface",
                            surface.name, badge.label, badge.x, badge.y
                        ));
                        break;
                    }
                }
            }
        }
    }
    assert!(offenders.is_empty(), "{}", offenders.join("\n"));
}

/// Every hint line has the one shape and the one badge, and every surface that
/// should say how to use it does.
#[test]
fn every_hint_line_uses_the_one_style() {
    let mut offenders = Vec::new();
    for (width, height) in [(160, 60), (80, 24)] {
        let surfaces = every_surface(default_bindings, width, height);
        let baseline: Vec<(u16, u16, String)> = badges(&surfaces[0].buf)
            .into_iter()
            .map(|b| (b.x, b.y, b.label))
            .collect();
        for surface in &surfaces {
            let buf = &surface.buf;
            let found = badges(buf);
            let own = found
                .iter()
                .filter(|b| !baseline.contains(&(b.x, b.y, b.label.clone())))
                .count();
            // Only at the roomy size: a narrow screen may clip a dialog's hint.
            if width == 160 && surface.must_hint && own == 0 {
                offenders.push(format!(
                    "{}: paints no key badge of its own:\n{}",
                    surface.name,
                    screen(buf)
                ));
            }
            for badge in &found {
                let key_fg = if badge.dim { DIM_KEY } else { KEY };
                for x in badge.x + 1..badge.end {
                    let cell = &buf[(x, badge.y)];
                    if cell.fg != key_fg || !cell.modifier.contains(Modifier::BOLD) {
                        offenders.push(format!(
                            "{}: <{}> has a key cell that is not the bold key color",
                            surface.name, badge.label
                        ));
                        break;
                    }
                }
            }
            for pair in found.windows(2) {
                let (a, b) = (&pair[0], &pair[1]);
                if a.y != b.y {
                    continue;
                }
                let gap: String = (a.end + 1..b.x).map(|x| buf[(x, a.y)].symbol()).collect();
                // Two lines that happen to share a row (a dialog's edge over the
                // screen behind it) are not one line.
                let crosses_a_frame = gap.chars().any(|c| ('\u{2500}'..='\u{257f}').contains(&c));
                if crosses_a_frame || gap.chars().count() > 60 {
                    continue;
                }
                if a.dim != b.dim {
                    offenders.push(format!(
                        "{} at {width}x{height}: <{}> and <{}> mix the modal and pane tones",
                        surface.name, a.label, b.label
                    ));
                }
                if !join_is_allowed(&gap) {
                    offenders.push(format!(
                        "{} at {width}x{height}: <{}>{gap:?}<{}> is not the one style",
                        surface.name, a.label, b.label
                    ));
                }
            }
        }
    }
    offenders.dedup();
    assert!(offenders.is_empty(), "{}", offenders.join("\n"));
}

/// With every action rebound, no hint anywhere still names a default key: every
/// badge names a rebound key, except a key the surface handles without a
/// binding (a text-input context's literal key, a mouse gesture).
#[test]
fn rebinding_every_action_leaves_no_default_key_in_any_hint() {
    let labels = rebound_labels(&unusual_bindings());
    assert!(
        labels.iter().all(|label| label.contains("F")),
        "the rebound labels are {labels:?}"
    );
    let bindings = unusual_bindings();
    let action_of = |label: &str| {
        BINDING_DEFS
            .iter()
            .map(|def| def.action)
            .find(|action| bindings.label_for(*action) == label)
    };
    let mut offenders = Vec::new();
    for (width, height) in [(160, 60), (80, 24)] {
        for surface in every_surface(unusual_bindings, width, height) {
            let found = badges(&surface.buf);
            for (index, badge) in found.iter().enumerate() {
                if surface.fixed.contains(&badge.label.as_str()) {
                    continue;
                }
                // One badge can carry two actions' keys (the footer's
                // `<j/k> move`): every key must be a rebound one, and the
                // description must be one of theirs.
                let actions: Option<Vec<Action>> = badge.label.split('/').map(&action_of).collect();
                let Some(actions) = actions else {
                    offenders.push(format!(
                        "{} at {width}x{height}: <{}> is not a rebound key",
                        surface.name, badge.label
                    ));
                    continue;
                };
                let desc = description_after(&surface.buf, &found, index);
                // A dialog painted over a line can hide a badge's description;
                // there is nothing left to hold that badge to.
                if desc.is_empty() {
                    continue;
                }
                if !actions.iter().any(|action| describes(&desc, *action)) {
                    offenders.push(format!(
                        "{} at {width}x{height}: <{}> is {actions:?}, which is not what {desc:?} \
                         describes",
                        surface.name, badge.label
                    ));
                }
            }
        }
    }
    offenders.sort();
    offenders.dedup();
    assert!(offenders.is_empty(), "{}", offenders.join("\n"));
}

/// The words a badge's hint says it does: past any `/<key>` that shares the
/// description, one space, then up to the next separator or frame edge.
fn description_after(buf: &Buffer, found: &[Badge], index: usize) -> String {
    let badge = &found[index];
    // `<a>/<b> desc`: the description belongs to the last key of the run.
    let joined = found.get(index + 1).is_some_and(|next| {
        next.y == badge.y
            && next.x == badge.end + 2
            && buf[(badge.end + 1, badge.y)].symbol() == "/"
    });
    if joined {
        return description_after(buf, found, index + 1);
    }
    let row: String = (badge.end + 1..buf.area.width)
        .map(|x| buf[(x, badge.y)].symbol().to_string())
        .collect();
    let row = row.trim_start();
    let end = row
        .char_indices()
        .find(|(i, c)| row[*i..].starts_with("  ") || ('\u{2500}'..='\u{257f}').contains(c))
        .map_or(row.len(), |(i, _)| i);
    row[..end].trim().to_string()
}

/// Whether `desc` is a description the bindings give `action`: a footer hint
/// or help entry of its own, or one of the words a dialog or pane uses for it.
fn describes(desc: &str, action: Action) -> bool {
    let def = BINDING_DEFS
        .iter()
        .find(|def| def.action == action)
        .expect("every action has a definition");
    if def.hint_contexts.iter().any(|(_, text)| *text == desc)
        || def
            .help
            .as_ref()
            .is_some_and(|help| help.description == desc)
    {
        return true;
    }
    DIALOG_WORDS
        .iter()
        .any(|(word, actions)| *word == desc && actions.contains(&action))
}

/// The words dialogs and pane hint lines use for an action. Each word lists
/// every action a hint may name with it; a key under a word that is not its
/// action's is a hint that names the wrong key.
const DIALOG_WORDS: &[(&str, &[Action])] = &[
    ("cancel", &[Action::CloseOverlay]),
    ("close", &[Action::CloseOverlay]),
    ("clear", &[Action::CloseOverlay, Action::ClearTextField]),
    ("close diff", &[Action::CloseOverlay]),
    ("close search", &[Action::CloseOverlay]),
    (
        "down",
        &[
            Action::MoveDown,
            Action::ScrollPageDown,
            Action::ScrollLineDown,
        ],
    ),
    ("up", &[Action::MoveUp, Action::ScrollPageUp]),
    (
        "scroll",
        &[
            Action::MoveDown,
            Action::MoveUp,
            Action::ScrollPageDown,
            Action::ScrollPageUp,
        ],
    ),
    ("page", &[Action::ScrollPageDown, Action::ScrollPageUp]),
    (
        "scroll the message",
        &[Action::ScrollPageDown, Action::ScrollPageUp],
    ),
    ("one line", &[Action::ScrollLineDown]),
    ("down one line", &[Action::ScrollLineDown]),
    ("live edge", &[Action::ScrollToBottom]),
    ("resume at the live edge", &[Action::ScrollToBottom]),
    ("logs", &[Action::MoveDown, Action::MoveUp]),
    ("focus", &[Action::ToggleSelection]),
    ("move focus", &[Action::ToggleSelection]),
    ("minimize", &[Action::ToggleFullscreen]),
    ("fullscreen", &[Action::ToggleFullscreen]),
    ("search", &[Action::SearchToggle, Action::SearchFiles]),
    ("next match", &[Action::SearchNext]),
    ("stage/unstage", &[Action::StageUnstage]),
    (
        "launch it again",
        &[Action::ReconnectAgent, Action::FocusAgent],
    ),
    ("focus and type", &[Action::FocusAgent]),
    ("take over", &[Action::FocusAgent]),
    ("next pane", &[Action::FocusNext]),
    ("previous pane", &[Action::FocusPrev]),
    ("actions", &[Action::FocusNext, Action::FocusPrev]),
    ("next tab", &[Action::NextTab]),
    ("macros", &[Action::OpenMacroBar]),
    ("edit text", &[Action::EngageCommitInput]),
    ("edit", &[Action::EngageCommitInput, Action::Confirm]),
    ("commit", &[Action::CommitChanges]),
    ("exit", &[Action::ExitCommitInput]),
    ("browse", &[Action::ExitPathEditorOnProjectAdd]),
    ("open", &[Action::OpenEntry, Action::Confirm]),
    ("go to", &[Action::GoToPath]),
    ("add current", &[Action::AddCurrentDir]),
    ("select", &[Action::ToggleMarked]),
    ("open file", &[Action::OpenStartupCommandLogFile]),
    ("open folder", &[Action::OpenStartupCommandLogFolder]),
    ("new", &[Action::NewMacro]),
    ("delete", &[Action::DeleteMacro]),
    ("standalone", &[Action::NewStandaloneAgent]),
    ("done", &[Action::Confirm]),
    ("confirm", &[Action::Confirm]),
    ("choose", &[Action::Confirm]),
    ("apply", &[Action::Confirm]),
    ("apply now", &[Action::Confirm]),
    ("save for next time", &[Action::Confirm]),
    ("use", &[Action::Confirm]),
    ("run", &[Action::Confirm]),
    ("remove", &[Action::Confirm]),
    ("resolve", &[Action::Confirm]),
    ("attach", &[Action::Confirm]),
    ("create agent", &[Action::Confirm]),
    ("expand/collapse", &[Action::Confirm]),
];

/// Every badge-opening bracket on screen that is never closed: a badge the
/// edge of its line cut through.
fn clipped_badges(buf: &Buffer) -> Vec<(u16, u16)> {
    let whole: Vec<(u16, u16)> = badges(buf).iter().map(|b| (b.x, b.y)).collect();
    let mut cut = Vec::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width.saturating_sub(1) {
            let cell = &buf[(x, y)];
            if cell.symbol() == "<"
                && matches!(cell.fg, BRACKET | DIM_BRACKET)
                && buf[(x + 1, y)].modifier.contains(Modifier::BOLD)
                && !whole.contains(&(x, y))
            {
                cut.push((x, y));
            }
        }
    }
    cut
}

/// A hint line too long for its space leaves whole segments out and says so;
/// it never runs off the edge through the middle of a badge.
#[test]
fn no_hint_line_runs_off_its_edge_through_a_badge() {
    let mut offenders = Vec::new();
    for (width, height) in [(160, 60), (120, 40), (80, 24)] {
        for surface in every_surface(default_bindings, width, height) {
            for (x, y) in clipped_badges(&surface.buf) {
                // The fullscreen startup log paints its frame over the main
                // footer's row, which cuts that line's badges without the line
                // itself running long.
                if surface.name.starts_with("StartupLog") && y == height - 2 {
                    continue;
                }
                offenders.push(format!(
                    "{} at {width}x{height}: a badge at ({x},{y}) is cut off:\n{}",
                    surface.name,
                    screen(&surface.buf)
                ));
            }
        }
    }
    assert!(offenders.is_empty(), "{}", offenders.join("\n"));
}

/// The row of `buf` holding `needle`, if any.
fn row_with(buf: &Buffer, needle: &str) -> Option<String> {
    screen(buf)
        .lines()
        .find(|row| row.contains(needle))
        .map(str::to_string)
}

/// Where a hint line is cut, the way out survives and the cut is marked: the
/// help overlay scrolled back at 120 columns, the startup-log list at 80, and
/// the diff view's line in an 80-column window.
#[test]
fn a_cut_hint_line_keeps_its_way_out_and_marks_the_cut() {
    let close = default_bindings().label_for(Action::CloseOverlay);

    let mut app = test_app(default_bindings());
    app.help_scroll = Some(4);
    let buf = render_at(&mut app, 120, 40);
    let row = row_with(&buf, "Scrolled back")
        .unwrap_or_else(|| panic!("no scrolled help hint:\n{}", screen(&buf)));
    assert!(row.contains(&format!("<{close}> close")), "{row}");
    assert!(row.contains('\u{2026}'), "the cut must be marked: {row}");

    let mut app = test_app(default_bindings());
    for (name, prompt) in every_prompt(&app) {
        if name == "StartupCommandLogs" {
            app.prompt = prompt;
        }
    }
    let buf = render_at(&mut app, 80, 24);
    let row = row_with(&buf, &format!("<{close}> close"))
        .unwrap_or_else(|| panic!("the logs dialog lost its close hint:\n{}", screen(&buf)));
    assert!(row.contains('\u{2026}'), "the cut must be marked: {row}");

    let mut app = test_app(default_bindings());
    app.focus = FocusPane::Center;
    app.center_mode = CenterMode::Diff {
        lines: Arc::new((0..200).map(|i| Line::from(format!("line {i}"))).collect()),
        scroll: 0,
        gutter_width: 0,
        worktree_path: "/tmp/does-not-matter".to_string(),
        rel_path: "src/main.rs".to_string(),
    };
    let buf = render_at(&mut app, 80, 24);
    let row = row_with(&buf, &format!("<{close}> close diff"))
        .unwrap_or_else(|| panic!("the diff lost its close hint:\n{}", screen(&buf)));
    assert!(row.contains('\u{2026}'), "the cut must be marked: {row}");
}
