//! Every place the terminal UI cuts a name, a path, a label or a title to fit,
//! rendered with wide characters rather than reasoned about.
//!
//! Each test puts a CJK name somewhere a cut is forced, renders the whole app
//! (or the one pane a surface paints) into a test backend, and reads the screen
//! the way a terminal shows it: a wide glyph covers the cell after it, so the
//! reader steps over that cell instead of reading whatever was left there. It
//! then asserts that the cut is marked with `…`, that the text kept before the
//! mark is intact, and that whatever comes after the cut column (the next
//! column of the row, the border, a trailing word) is exactly where it is on a
//! row whose name is plain ASCII.

use std::path::PathBuf;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::components::wrap_lines::display_width;
use super::test_support::{default_bindings, test_app};
use super::*;

/// Forty columns of CJK: wider than every column cap these rows use.
const LONG_CJK: &str = "日本語のとても長い名前がここにありますよ";

fn render_at(app: &mut App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|frame| app.render(frame)).expect("render");
    terminal.backend().buffer().clone()
}

/// Row `y` as a terminal shows it: `(x, glyph)` for every glyph, skipping the
/// cells a wide glyph covers.
fn shown(buf: &Buffer, y: u16) -> Vec<(u16, String)> {
    let mut out = Vec::new();
    let mut x = 0u16;
    while x < buf.area.width {
        let symbol = buf[(x, y)].symbol().to_string();
        let width = u16::try_from(display_width(&symbol)).unwrap_or(1).max(1);
        out.push((x, symbol));
        x = x.saturating_add(width);
    }
    out
}

fn row_text(buf: &Buffer, y: u16) -> String {
    shown(buf, y).into_iter().map(|(_, s)| s).collect()
}

fn screen(buf: &Buffer) -> String {
    (0..buf.area.height)
        .map(|y| row_text(buf, y))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The column at which `needle` begins on row `y`, as displayed.
fn column_of(buf: &Buffer, y: u16, needle: &str) -> Option<u16> {
    let glyphs = shown(buf, y);
    let wanted: Vec<String> = needle.chars().map(|c| c.to_string()).collect();
    (0..glyphs.len()).find_map(|start| {
        let matches = wanted
            .iter()
            .enumerate()
            .all(|(i, w)| glyphs.get(start + i).is_some_and(|(_, s)| s == w));
        matches.then(|| glyphs[start].0)
    })
}

/// The first row on which `needle` is displayed.
fn row_of(buf: &Buffer, needle: &str) -> u16 {
    (0..buf.area.height)
        .find(|&y| column_of(buf, y, needle).is_some())
        .unwrap_or_else(|| panic!("{needle:?} is not on screen:\n{}", screen(buf)))
}

/// Assert row `y` shows a cut of `full`: a non-empty prefix of it, displayed
/// intact, immediately followed by the one `…`. Returns the mark's column.
fn assert_cut_after_prefix(buf: &Buffer, y: u16, full: &str) -> u16 {
    let glyphs = shown(buf, y);
    let first: String = full.chars().next().unwrap().to_string();
    let start = glyphs
        .iter()
        .position(|(_, s)| *s == first)
        .unwrap_or_else(|| panic!("row {y} does not show {full:?}:\n{}", screen(buf)));
    let mut kept = 0usize;
    for (want, (_, got)) in full.chars().zip(glyphs[start..].iter()) {
        if got != &want.to_string() {
            break;
        }
        kept += 1;
    }
    assert!(
        kept > 0 && kept < full.chars().count(),
        "row {y}: {}",
        row_text(buf, y)
    );
    let (mark_x, mark) = &glyphs[start + kept];
    assert_eq!(
        mark,
        "\u{2026}",
        "row {y} must mark the cut right after the kept text: {}",
        row_text(buf, y)
    );
    *mark_x
}

// ── Pickers: a name column cut at its end, then the next column ──

#[test]
fn the_kill_running_list_cuts_a_wide_label_and_keeps_the_context_column_aligned() {
    let mut app = test_app(default_bindings());
    let runtime = |id: &str, label: &str, context: &str| KillableRuntime {
        id: RuntimeTargetId::Terminal(id.to_string()),
        kind: KillableRuntimeKind::Terminal,
        label: label.to_string(),
        context: context.to_string(),
        search_text: label.to_string(),
    };
    app.prompt = PromptState::KillRunning(KillRunningPrompt {
        runtimes: vec![
            runtime("t1", LONG_CJK, "ctxone"),
            runtime("t2", "short", "ctxtwo"),
        ],
        list: SearchableList::new(),
        selected_ids: std::collections::HashSet::new(),
        focus: KillRunningFocus::List,
    });
    for (w, h) in [(120, 40), (80, 24)] {
        let buf = render_at(&mut app, w, h);
        let wide = row_of(&buf, "日本");
        let plain = row_of(&buf, "short");
        assert_cut_after_prefix(&buf, wide, LONG_CJK);
        assert_eq!(
            column_of(&buf, wide, "ctxone"),
            column_of(&buf, plain, "ctxtwo"),
            "at {w}x{h} the context column must line up:\n{}",
            screen(&buf)
        );
    }
}

fn add_macro(app: &mut App, name: &str, text: &str) {
    app.engine.config.macros.entries.insert(
        name.to_string(),
        crate::config::MacroEntry {
            text: text.to_string(),
            surface: crate::config::MacroSurface::Agent,
        },
    );
}

#[test]
fn the_macro_bar_pads_a_wide_name_by_columns_and_cuts_a_wide_body() {
    let mut app = test_app(default_bindings());
    add_macro(&mut app, "日本語マクロ", &LONG_CJK.repeat(4));
    add_macro(&mut app, "hi", "plainbody");
    app.macro_bar = Some(MacroBarState {
        input: TextInput::new(),
        selected: 0,
        previous_input_target: InputTarget::None,
    });
    for (w, h) in [(120, 40), (80, 24)] {
        let buf = render_at(&mut app, w, h);
        let wide = row_of(&buf, "日本語マクロ");
        let plain = row_of(&buf, "plainbody");
        let body_x = column_of(&buf, wide, "日本語のとて").expect("the wide body is shown");
        assert_eq!(
            Some(body_x),
            column_of(&buf, plain, "plainbody"),
            "at {w}x{h} the bodies must start in one column:\n{}",
            screen(&buf)
        );
        let mark = shown(&buf, wide)
            .into_iter()
            .find(|(x, s)| *x > body_x && s == "\u{2026}");
        assert!(mark.is_some(), "the cut body is marked:\n{}", screen(&buf));
    }
}

#[test]
fn the_macro_list_cuts_a_wide_preview_behind_a_wide_name() {
    let mut app = test_app(default_bindings());
    add_macro(&mut app, "日本語マクロ", &LONG_CJK.repeat(4));
    app.open_edit_macros();
    for (w, h) in [(120, 40), (80, 24)] {
        let buf = render_at(&mut app, w, h);
        let y = row_of(&buf, "日本語マクロ");
        let body_x = column_of(&buf, y, "日本語のとて").expect("the preview is shown");
        let mark = shown(&buf, y)
            .into_iter()
            .find(|(x, s)| *x > body_x && s == "\u{2026}");
        assert!(
            mark.is_some(),
            "at {w}x{h} the preview must end in the mark, not run into the border:\n{}",
            screen(&buf)
        );
    }
}

fn managed(name: &str, branch: &str) -> dux_core::worktree_manager::ManagedWorktree {
    dux_core::worktree_manager::ManagedWorktree {
        path: PathBuf::from(format!("/srv/worktrees/demo/{name}")),
        label: branch.to_string(),
        branch: Some(branch.to_string()),
        dirty: false,
        attached_session_id: None,
    }
}

#[test]
fn the_worktree_manager_cuts_a_wide_folder_name_in_the_middle_by_columns() {
    let mut app = test_app(default_bindings());
    let project = app.engine.projects[0].clone();
    app.prompt = PromptState::ManageWorktrees(ManageWorktreesPrompt {
        project,
        entries: vec![managed(LONG_CJK, "brone"), managed("plain", "brtwo")],
        loading: false,
        selected: Some(0),
        error: None,
    });
    for (w, h) in [(120, 40), (80, 24)] {
        let buf = render_at(&mut app, w, h);
        let wide = row_of(&buf, "brone");
        let plain = row_of(&buf, "brtwo");
        assert_cut_after_prefix(&buf, wide, LONG_CJK);
        assert_eq!(
            column_of(&buf, wide, "branch: brone"),
            column_of(&buf, plain, "branch: brtwo"),
            "at {w}x{h} the branch column must line up:\n{}",
            screen(&buf)
        );
    }
}

fn picker_entry(name: &str, branch: &str) -> ProjectWorktreeEntry {
    ProjectWorktreeEntry {
        path: PathBuf::from(format!("/srv/worktrees/demo/{name}")),
        branch_name: branch.to_string(),
        branch: Some(branch.to_string()),
        is_managed_by_dux: true,
        existing_session_id: None,
        is_external: false,
        is_project_checkout: false,
        is_selectable: true,
    }
}

#[test]
fn the_worktree_picker_cuts_a_wide_folder_name_in_the_middle_by_columns() {
    let mut app = test_app(default_bindings());
    let project = app.engine.projects[0].clone();
    app.prompt = PromptState::PickProjectWorktree(PickProjectWorktreePrompt {
        project,
        entries: vec![
            picker_entry(LONG_CJK, "brone"),
            picker_entry("plain", "brtwo"),
        ],
        loading: false,
        selected: Some(0),
        error: None,
    });
    for (w, h) in [(120, 40), (80, 24)] {
        let buf = render_at(&mut app, w, h);
        let wide = row_of(&buf, "brone");
        let plain = row_of(&buf, "brtwo");
        assert_cut_after_prefix(&buf, wide, LONG_CJK);
        assert_eq!(
            column_of(&buf, wide, "branch: brone"),
            column_of(&buf, plain, "branch: brtwo"),
            "at {w}x{h} the branch column must line up:\n{}",
            screen(&buf)
        );
    }
}

#[test]
fn the_project_picker_cuts_a_wide_name_and_keeps_the_tail_of_a_wide_path() {
    let mut app = test_app(default_bindings());
    let entry = |id: &str, name: &str, path: &str| ProjectChooserEntry {
        id: id.to_string(),
        name: name.to_string(),
        path: path.to_string(),
        agent_count: 0,
        path_missing: false,
    };
    let wide_path = format!("/srv/{}/{}/リーフ", LONG_CJK, LONG_CJK);
    app.prompt = PromptState::PickProject {
        intent: ProjectChooserIntent::NewAgent,
        entries: vec![
            entry("p1", LONG_CJK, &wide_path),
            entry("p2", "plain", "/srv/plain"),
        ],
        list: SearchableList::new(),
    };
    for (w, h) in [(120, 40), (80, 24)] {
        let buf = render_at(&mut app, w, h);
        let wide = row_of(&buf, "日本");
        let plain = row_of(&buf, "plain");
        assert_cut_after_prefix(&buf, wide, LONG_CJK);
        assert_eq!(
            column_of(&buf, wide, "no agents"),
            column_of(&buf, plain, "no agents"),
            "at {w}x{h} the count column must line up:\n{}",
            screen(&buf)
        );
        // The path keeps its leaf behind the mark, and the wide row's right
        // border is where the plain row's is.
        let text = row_text(&buf, wide);
        assert!(
            text.contains("\u{2026}"),
            "the path is marked as cut:\n{}",
            screen(&buf)
        );
        let right_border = |y: u16| {
            shown(&buf, y)
                .into_iter()
                .rev()
                .find(|(_, s)| s == "\u{2502}")
                .map(|(x, _)| x)
        };
        let leaf = column_of(&buf, wide, "リーフ").expect("the leaf is shown");
        let border = right_border(plain).expect("the list has a right border");
        assert!(
            leaf + 6 <= border,
            "the leaf ends inside the list:\n{}",
            screen(&buf)
        );
        assert_eq!(right_border(wide), Some(border), "{}", screen(&buf));
    }
}

// ── The changes pane: a path cut in the middle, stats pinned right ──

#[test]
fn the_changes_list_cuts_a_wide_path_and_keeps_the_stats_right_aligned() {
    let mut app = test_app(default_bindings());
    let file = |path: &str| dux_core::model::ChangedFile {
        path: path.to_string(),
        status: "M".to_string(),
        additions: 12,
        deletions: 3,
        binary: false,
        diff_excluded: false,
        renamed_from: None,
    };
    let wide_path = format!("docs/{LONG_CJK}/{LONG_CJK}.md");
    app.engine.unstaged_files = vec![file("plain.md"), file(&wide_path)];
    app.focus = FocusPane::Files;
    app.right_section = RightSection::Unstaged;
    // The plain row is selected, so the wide one is cut like any other row.
    app.files_index = 0;
    app.right_hidden = false;
    // Wide enough that the stats fit beside a path: below that the path keeps a
    // ten-column floor and the stats are clipped on every row, plain or wide.
    for (w, h) in [(120, 40), (160, 40)] {
        let buf = render_at(&mut app, w, h);
        let plain = row_of(&buf, "plain.md");
        let wide = row_of(&buf, "docs");
        assert!(
            row_text(&buf, wide).contains('\u{2026}'),
            "at {w}x{h} the wide path is marked as cut:\n{}",
            screen(&buf)
        );
        let last = |y: u16| {
            shown(&buf, y)
                .into_iter()
                .rev()
                .find(|(_, s)| s == "3")
                .map(|(x, _)| x)
        };
        assert_eq!(
            last(wide),
            last(plain),
            "at {w}x{h} the stats must end in one column:\n{}",
            screen(&buf)
        );
    }
}

// ── A name chip inside a sentence ──

#[test]
fn the_new_agent_dialog_cuts_a_wide_worktree_path_and_keeps_the_full_stop() {
    let mut app = test_app(default_bindings());
    let project = app.engine.projects[0].clone();
    let path = format!("/srv/{LONG_CJK}/{LONG_CJK}");
    app.prompt = PromptState::NameNewAgent {
        request: CreateAgentRequest::ExistingManagedWorktree {
            project,
            worktree_path: PathBuf::from(&path),
            branch_name: "existing".to_string(),
            custom_name: None,
        },
        input: TextInput::new(),
        randomize_name: false,
        randomized_name: None,
        copy_changes: false,
        focus: NameNewAgentFocus::Input,
    };
    for (w, h) in [(120, 40), (80, 24)] {
        let buf = render_at(&mut app, w, h);
        let y = row_of(&buf, "/srv/");
        let text = row_text(&buf, y);
        assert!(
            text.contains('\u{2026}'),
            "at {w}x{h} the path is cut in the middle:\n{}",
            screen(&buf)
        );
        // The chip's closing pad and the sentence's full stop both survive.
        let glyphs = shown(&buf, y);
        let tail = glyphs
            .iter()
            .position(|(_, s)| s == "\u{2026}")
            .map(|i| &glyphs[i..])
            .unwrap();
        let chars: String = tail.iter().map(|(_, s)| s.as_str()).collect();
        assert!(
            chars.contains(" ."),
            "at {w}x{h} the full stop must follow the chip:\n{}",
            screen(&buf)
        );
    }
}

// ── The status line: a long message cut across its rows ──

#[test]
fn a_long_wide_status_message_is_marked_as_cut_on_its_last_row() {
    for message in [LONG_CJK.repeat(10), "word ".repeat(80)] {
        let mut app = test_app(default_bindings());
        app.set_info(message.clone());
        for (w, h) in [(120, 40), (80, 24)] {
            let buf = render_at(&mut app, w, h);
            let last = row_text(&buf, h - 1);
            assert!(
                last.trim_end().ends_with('\u{2026}'),
                "at {w}x{h} the status must end in the mark:\n{}",
                screen(&buf)
            );
        }
    }
}

/// A presentation selector makes `⚠` two columns wide as drawn. The status
/// line is wrapped by what is drawn, so the one column it adds moves the tail
/// to the second row rather than pushing a letter off the edge of the first.
#[test]
fn a_status_with_an_emoji_sequence_loses_no_letter_at_the_row_edge() {
    let mut app = test_app(default_bindings());
    // " ● " is three columns, "⚠️ " three more: 75 letters make 81 columns.
    let letters = "a".repeat(75);
    app.set_info(format!("\u{26a0}\u{fe0f} {letters}"));
    let buf = render_at(&mut app, 80, 24);
    let footer: String = [22u16, 23].iter().map(|&y| row_text(&buf, y)).collect();
    assert_eq!(
        footer.matches('a').count(),
        75,
        "every letter is on one of the two status rows:\n{}",
        screen(&buf)
    );
}

// ── The resource monitor: a table cell ──

#[test]
fn the_resource_monitor_marks_a_wide_name_cut_by_its_column() {
    let mut app = test_app(default_bindings());
    app.prompt = PromptState::ResourceMonitor {
        rows: vec![ResourceStats {
            id: Some("t1".to_string()),
            kind: ResourceKind::Terminal,
            label: LONG_CJK.repeat(3),
            pid: Some(4242),
            cpu_percent: 1.5,
            rss_bytes: 1024,
            process_count: 1,
            children: Vec::new(),
        }],
        scroll_offset: 0,
        selected_row: 0,
        expanded: std::collections::HashSet::new(),
        last_refresh: std::time::Instant::now(),
        short_window_sample: false,
    };
    for (w, h) in [(120, 40), (80, 24)] {
        let buf = render_at(&mut app, w, h);
        let y = row_of(&buf, "日本");
        let mark = assert_cut_after_prefix(&buf, y, &LONG_CJK.repeat(3));
        let cpu = column_of(&buf, y, "1.5%").expect("the CPU cell is shown");
        assert!(mark < cpu, "the name's mark sits inside its own column");
    }
}

// ── The PR banner: a pill whose title is cut before its right cap ──

#[test]
fn the_pr_banner_cuts_a_wide_title_before_its_right_cap() {
    use crate::model::{PrInfo, PrState};
    let mut app = test_app(default_bindings());
    let id = app.selected_session().expect("a selected agent").id.clone();
    app.engine.pr_statuses.insert(
        id,
        PrInfo {
            number: 42,
            state: PrState::Open,
            title: LONG_CJK.repeat(3),
            host: "github.com".to_string(),
            owner_repo: "owner/repo".to_string(),
            url: "https://github.com/owner/repo/pull/42".to_string(),
        },
    );
    for (w, h) in [(120, 40), (80, 24)] {
        let buf = render_at(&mut app, w, h);
        let y = row_of(&buf, "owner/repo#42");
        assert_cut_after_prefix(&buf, y, &LONG_CJK.repeat(3));
        let banner = app.mouse_layout.pr_banner.expect("the banner is painted");
        let right_edge = banner.x + banner.width - 1;
        assert_eq!(
            buf[(right_edge, y)].symbol(),
            "\u{258c}",
            "at {w}x{h} the right cap closes the band:\n{}",
            screen(&buf)
        );
    }
}

// ── The loading card: a provider name inside a one-line card ──

#[test]
fn the_loading_card_cuts_a_wide_provider_name_to_the_card() {
    let app = test_app(default_bindings());
    for width in [26u16, 30, 40] {
        let mut terminal = Terminal::new(TestBackend::new(width, 9)).expect("terminal");
        terminal
            .draw(|frame| {
                app.render_terminal_loading(
                    frame,
                    Rect::new(0, 0, width, 9),
                    Some(LONG_CJK),
                    SessionSurface::Agent,
                );
            })
            .expect("render");
        let buf = terminal.backend().buffer().clone();
        let y = row_of(&buf, "Starting");
        assert_cut_after_prefix(&buf, y, LONG_CJK);
        let text = row_text(&buf, y);
        // The cut's mark already says there is more; the loading dots after it
        // would be a second mark.
        assert!(
            !text.contains("..."),
            "at width {width} a cut name carries one mark, not two:\n{}",
            screen(&buf)
        );
        let mark = column_of(&buf, y, "\u{2026}").expect("the mark is shown");
        let border = shown(&buf, y)
            .into_iter()
            .rev()
            .find(|(_, s)| s == "\u{2502}")
            .map(|(x, _)| x)
            .expect("the card has a right border");
        assert!(
            mark < border,
            "the mark is inside the card:\n{}",
            screen(&buf)
        );
    }
}

/// A short wide name fits, and the card is sized to its real width: the
/// label is centred between the borders, not pushed off by a byte count.
#[test]
fn the_loading_card_is_sized_by_columns() {
    let app = test_app(default_bindings());
    let mut terminal = Terminal::new(TestBackend::new(80, 9)).expect("terminal");
    terminal
        .draw(|frame| {
            app.render_terminal_loading(
                frame,
                Rect::new(0, 0, 80, 9),
                Some("日本語"),
                SessionSurface::Agent,
            );
        })
        .expect("render");
    let buf = terminal.backend().buffer().clone();
    let y = row_of(&buf, "Starting");
    let glyphs = shown(&buf, y);
    let left = glyphs.iter().find(|(_, s)| s == "\u{2502}").unwrap().0;
    let right = glyphs
        .iter()
        .rev()
        .find(|(_, s)| s == "\u{2502}")
        .unwrap()
        .0;
    // "Starting 日本語..." is 9 + 6 + 3 = 18 columns, and the card is ten wider
    // (its borders, the spinner and the padding): 28 wide, so its borders are
    // 27 columns apart. A byte count makes the name nine wide instead of six.
    assert_eq!(right - left, 27, "{}", screen(&buf));
}

// ── The footer hint bar: hints dropped behind one mark ──

#[test]
fn footer_hints_are_measured_in_columns() {
    let app = test_app(default_bindings());
    // Two hints of `<é> ab` (six columns, seven bytes) and the mark of the
    // third, each after the two-column separator: seventeen columns. A byte
    // count calls it nineteen and drops the second hint too.
    let hints = vec![
        ("é".to_string(), "ab"),
        ("é".to_string(), "cd"),
        ("é".to_string(), "ef"),
    ];
    let spans = app.footer_hint_spans(&hints, 17);
    let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
    assert_eq!(text, "<é> ab  <é> cd  …");
}

// ── Picker columns sized by an id or a provider name ──

/// The column after a padded name starts at `after` on every row the rows
/// matched by `names` are on.
fn assert_column_after(buf: &Buffer, names: &[&str], after: &str) {
    let columns: Vec<Option<u16>> = names
        .iter()
        .map(|name| column_of(buf, row_of(buf, name), after))
        .collect();
    assert!(
        columns[0].is_some(),
        "{after:?} is not shown:\n{}",
        screen(buf)
    );
    assert!(
        columns.windows(2).all(|pair| pair[0] == pair[1]),
        "{after:?} must start in one column on every row, got {columns:?}:\n{}",
        screen(buf)
    );
}

const WIDE_ID: &str = "日本語のテーマ名前";

#[test]
fn the_theme_picker_pads_a_wide_id_by_columns() {
    let mut app = test_app(default_bindings());
    let listing = |id: &str, display: &str| crate::theme::ThemeListing {
        id: id.to_string(),
        display_name: display.to_string(),
        source: crate::theme::ThemeSource::User,
    };
    app.prompt = PromptState::ChangeTheme(ChangeThemePrompt {
        options: vec![listing(WIDE_ID, "Wideone"), listing("plain", "Plainone")],
        selected: 0,
        current: "none".to_string(),
    });
    for (w, h) in [(120, 40), (80, 24)] {
        let buf = render_at(&mut app, w, h);
        let wide = column_of(&buf, row_of(&buf, "Wideone"), "Wideone");
        let plain = column_of(&buf, row_of(&buf, "Plainone"), "Plainone");
        assert_eq!(wide, plain, "at {w}x{h}:\n{}", screen(&buf));
    }
}

#[test]
fn the_default_provider_picker_pads_a_wide_provider_by_columns() {
    let mut app = test_app(default_bindings());
    let option = |name: &str| ChangeDefaultProviderOption {
        provider: ProviderKind::new(name),
        is_current: false,
    };
    app.prompt = PromptState::ChangeDefaultProvider(ChangeDefaultProviderPrompt {
        current: ProviderKind::new("claude"),
        options: vec![option(WIDE_ID), option("plain")],
        selected: 0,
    });
    for (w, h) in [(120, 40), (80, 24)] {
        let buf = render_at(&mut app, w, h);
        assert_column_after(&buf, &[WIDE_ID, "plain"], "available");
    }
}

#[test]
fn the_agent_provider_picker_pads_a_wide_provider_by_columns() {
    let mut app = test_app(default_bindings());
    let option = |name: &str| ChangeAgentProviderOption {
        provider: ProviderKind::new(name),
        supports_resume: true,
        resume_available: false,
        is_current: false,
    };
    app.prompt = PromptState::ChangeAgentProvider(ChangeAgentProviderPrompt {
        session_id: "session-1".to_string(),
        tab_id: "session-1".to_string(),
        session_label: "agent".to_string(),
        worktree_path: "/srv/wt".to_string(),
        options: vec![option(WIDE_ID), option("plain")],
        selected: 0,
        mode: ChangeAgentProviderMode::Retarget,
    });
    for (w, h) in [(120, 40), (80, 24)] {
        let buf = render_at(&mut app, w, h);
        assert_column_after(&buf, &[WIDE_ID, "plain"], "no prior");
    }
}

#[test]
fn the_project_provider_picker_pads_a_wide_provider_by_columns() {
    let mut app = test_app(default_bindings());
    let option = |name: &str| ChangeProjectDefaultProviderOption {
        provider: Some(ProviderKind::new(name)),
        is_current: false,
    };
    app.prompt = PromptState::ChangeProjectDefaultProvider(ChangeProjectDefaultProviderPrompt {
        project_id: app.engine.projects[0].id.clone(),
        project_name: "demo".to_string(),
        current: ProviderKind::new("claude"),
        global_default: ProviderKind::new("claude"),
        inherits_global_default: true,
        options: vec![option(WIDE_ID), option("plain")],
        selected: 0,
    });
    for (w, h) in [(120, 40), (80, 24)] {
        let buf = render_at(&mut app, w, h);
        assert_column_after(&buf, &[WIDE_ID, "plain"], "available");
    }
}

// ── The startup log: wide glyphs drawn at their own width ──

/// Every glyph of `source` that appears on screen, in reading order, keeping
/// only glyphs `source` itself uses: the log's text with the chrome around it
/// filtered out.
fn log_glyphs(buf: &Buffer, source: &str) -> String {
    (0..buf.area.height)
        .flat_map(|y| shown(buf, y))
        .map(|(_, s)| s)
        .filter(|s| s.chars().count() == 1 && source.contains(s.as_str()))
        .collect()
}

const WIDE_LOG: &str = "起動コマンドの出力がここに表示されます";

#[test]
fn the_startup_log_picker_draws_and_wraps_wide_glyphs_by_their_width() {
    let mut app = test_app(default_bindings());
    let content = WIDE_LOG.repeat(4);
    app.prompt = PromptState::StartupCommandLogs(StartupCommandLogPrompt {
        scope_label: "demo".to_string(),
        entries: Vec::new(),
        selected: 0,
        filter: TextInput::new(),
        searching: false,
        content: content.clone(),
        scroll_offset: 0,
        wrap_width: 0,
        focus: StartupCommandLogFocus::List,
    });
    for (w, h) in [(120, 40), (80, 24)] {
        let buf = render_at(&mut app, w, h);
        assert_eq!(
            log_glyphs(&buf, WIDE_LOG),
            content,
            "at {w}x{h} every glyph is drawn once, whole and in order:\n{}",
            screen(&buf)
        );
    }
}

#[test]
fn the_startup_log_viewer_draws_and_wraps_wide_glyphs_by_their_width() {
    let mut app = test_app(default_bindings());
    let content = WIDE_LOG.repeat(4);
    app.startup_log_viewer = Some(StartupLogViewer {
        scope_label: "demo".to_string(),
        path: None,
        display_name: "run".to_string(),
        content: content.clone(),
        scroll_offset: 0,
        wrap_width: 0,
        search: TextInput::new(),
        searching: false,
        return_to: None,
    });
    app.fullscreen_overlay = FullscreenOverlay::StartupLog;
    for (w, h) in [(120, 40), (80, 24)] {
        let buf = render_at(&mut app, w, h);
        assert_eq!(
            log_glyphs(&buf, WIDE_LOG),
            content,
            "at {w}x{h} every glyph is drawn once, whole and in order:\n{}",
            screen(&buf)
        );
    }
}
