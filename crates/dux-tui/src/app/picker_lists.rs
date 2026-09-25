//! Every Picker-family modal's list, rendered rather than reasoned about.
//!
//! The pickers are enumerated from the modal registry
//! ([`super::modal::modal_spec`]) over the registry's own fixture list, so a
//! new picker fails [`every_picker_has_a_tall_fixture`] until it is given one
//! here. Each test opens a picker with more rows than fit at 80x24, renders the
//! whole app into a test backend, and asserts on cells: the selected row stays
//! on screen and is the one highlighted, the list wears the same scroll
//! indicator the welcome and What's new screens do, a click on a visible row
//! selects that row, and the empty state is drawn.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;

use super::components::MARKER_GLYPHS;
use super::modal::{ModalFamily, modal_spec};
use super::test_support::{default_bindings, test_app};
use super::*;
use crate::model::ProviderKind;

const WIDTH: u16 = 80;
const HEIGHT: u16 = 24;

/// Enough rows that no picker at 80x24 can show them all.
const TALL: usize = 40;

/// One picker, tall and empty.
struct PickerFixture {
    /// The picker holding [`TALL`]-ish rows whose LAST visual row is selectable.
    tall: PromptState,
    /// A piece of text only the last selectable row carries, when the rows have
    /// one (the tailscale picker's three modes repeat, so it has none).
    last_label: Option<String>,
    /// The same picker with nothing to list.
    empty: PromptState,
    /// What its empty state says (a prefix is enough: narrow lists truncate).
    empty_text: &'static str,
    /// Screen rows one item occupies.
    row_height: u16,
}

fn render(app: &mut App) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).expect("terminal");
    terminal.draw(|frame| app.render(frame)).expect("render");
    terminal.backend().buffer().clone()
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

fn row_text(buf: &Buffer, y: u16) -> String {
    (0..buf.area.width)
        .map(|x| buf[(x, y)].symbol().to_string())
        .collect()
}

/// The screen rows painted with the selection highlight. The dim overlay
/// repaints everything behind a modal, so only the modal's own highlight can
/// match.
fn highlighted_rows(app: &App, buf: &Buffer) -> Vec<u16> {
    let bg = app.theme.selection_style().bg.expect("selection has a bg");
    (0..buf.area.height)
        .filter(|&y| {
            (0..buf.area.width)
                .filter(|&x| buf[(x, y)].bg == bg)
                .count()
                >= 8
        })
        .collect()
}

/// The list rect, item count and offset the picker published for clicks.
fn published_list(layout: OverlayMouseLayout) -> (Rect, usize, usize) {
    match layout {
        OverlayMouseLayout::Command {
            list,
            items,
            offset,
            ..
        }
        | OverlayMouseLayout::BrowseProjects {
            list,
            items,
            offset,
            ..
        }
        | OverlayMouseLayout::PickEditor {
            list,
            items,
            offset,
        }
        | OverlayMouseLayout::PickProjectWorktree {
            list,
            items,
            offset,
        }
        | OverlayMouseLayout::ManageWorktrees {
            list,
            items,
            offset,
        }
        | OverlayMouseLayout::PickProject {
            list,
            items,
            offset,
            ..
        }
        | OverlayMouseLayout::ChangeTheme {
            list,
            items,
            offset,
        }
        | OverlayMouseLayout::ChangeAgentProvider {
            list,
            items,
            offset,
        }
        | OverlayMouseLayout::ChangeDefaultProvider {
            list,
            items,
            offset,
        }
        | OverlayMouseLayout::ChangeProjectDefaultProvider {
            list,
            items,
            offset,
        }
        | OverlayMouseLayout::SetTailscaleMode {
            list,
            items,
            offset,
        }
        | OverlayMouseLayout::EditMacroList {
            list,
            items,
            offset,
        }
        | OverlayMouseLayout::ResourceMonitor {
            list,
            items,
            offset,
        }
        | OverlayMouseLayout::StartupCommandLogs {
            list,
            items,
            offset,
            ..
        }
        | OverlayMouseLayout::KillRunning {
            list,
            items,
            offset,
            ..
        } => (list, items, offset),
        other => panic!("not a picker list layout: {other:?}"),
    }
}

fn press(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
        .expect("key");
}

fn click(app: &mut App, column: u16, row: u16) {
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Up(MouseButton::Left),
    ] {
        app.handle_mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers: KeyModifiers::NONE,
        });
    }
}

fn leak(text: String) -> &'static str {
    Box::leak(text.into_boxed_str())
}

fn project_worktree_entry(name: &str) -> ProjectWorktreeEntry {
    ProjectWorktreeEntry {
        path: PathBuf::from(format!("/repo/worktrees/{name}")),
        branch_name: name.to_string(),
        branch: Some(name.to_string()),
        is_managed_by_dux: true,
        existing_session_id: None,
        is_external: false,
        is_project_checkout: false,
        is_selectable: true,
    }
}

fn managed_worktree(name: &str) -> dux_core::worktree_manager::ManagedWorktree {
    dux_core::worktree_manager::ManagedWorktree {
        path: PathBuf::from(format!("/repo/worktrees/{name}")),
        label: name.to_string(),
        branch: Some(name.to_string()),
        dirty: false,
        attached_session_id: None,
    }
}

fn resource_row(label: &str, pid: u32) -> ResourceStats {
    ResourceStats {
        id: Some(label.to_string()),
        kind: ResourceKind::Agent,
        label: label.to_string(),
        pid: Some(pid),
        cpu_percent: 1.0,
        rss_bytes: 1024,
        process_count: 1,
        children: Vec::new(),
    }
}

fn names(prefix: &str) -> Vec<String> {
    (0..TALL).map(|n| format!("{prefix}{n:02}")).collect()
}

fn last(names: &[String]) -> Option<String> {
    names.last().cloned()
}

/// The fixture for the picker the registry knows as `name`.
///
/// Panics for a picker with no fixture: that is the failure a new picker
/// earns until it is given one here.
fn fixture(app: &App, name: &str) -> PickerFixture {
    let project = app.engine.projects[0].clone();
    match name {
        "Command" => {
            let commands: Vec<String> = app
                .filtered_palette_commands("")
                .iter()
                .filter_map(|binding| binding.palette_name.map(str::to_string))
                .collect();
            PickerFixture {
                tall: PromptState::Command {
                    input: TextInput::new(),
                    selected: 0,
                },
                last_label: last(&commands),
                empty: PromptState::Command {
                    input: TextInput::with_text("zzzz-no-such-command".to_string()),
                    selected: 0,
                },
                empty_text: "No matching commands.",
                row_height: 1,
            }
        }
        "BrowseProjects" => {
            let labels = names("folder-");
            let browse = |entries: Vec<BrowserEntry>| PromptState::BrowseProjects {
                purpose: BrowsePurpose::AddProject,
                current_dir: PathBuf::from("/home/ada"),
                entries,
                loading: false,
                selected: 0,
                filter: TextInput::new(),
                searching: false,
                editing_path: false,
                path_input: TextInput::new(),
                tab_completions: Vec::new(),
                tab_index: 0,
            };
            PickerFixture {
                tall: browse(
                    labels
                        .iter()
                        .map(|label| BrowserEntry {
                            path: PathBuf::from(format!("/home/ada/{label}")),
                            label: label.clone(),
                            is_git_repo: false,
                            is_parent: false,
                        })
                        .collect(),
                ),
                last_label: last(&labels),
                empty: browse(Vec::new()),
                empty_text: "No child directories here.",
                row_height: 1,
            }
        }
        "PickEditor" => {
            let labels = names("editor-");
            let pick = |labels: &[String]| PromptState::PickEditor {
                session_label: "agent".to_string(),
                worktree_path: "/tmp/wt".to_string(),
                editors: labels
                    .iter()
                    .map(|label| dux_core::editor::DetectedEditor {
                        kind: dux_core::editor::EditorKind::Zed,
                        label: leak(label.clone()),
                        config_key: "zed",
                        command: "zed".to_string(),
                    })
                    .collect(),
                selected: 0,
            };
            PickerFixture {
                tall: pick(&labels),
                last_label: last(&labels),
                empty: pick(&[]),
                empty_text: "No editors",
                row_height: 1,
            }
        }
        "PickProject" => {
            let labels = names("project-");
            let pick = |labels: &[String]| PromptState::PickProject {
                intent: ProjectChooserIntent::NewAgent,
                entries: labels
                    .iter()
                    .map(|label| ProjectChooserEntry {
                        id: label.clone(),
                        name: label.clone(),
                        path: format!("/code/{label}"),
                        agent_count: 0,
                        path_missing: false,
                    })
                    .collect(),
                list: SearchableList::new(),
            };
            PickerFixture {
                tall: pick(&labels),
                last_label: last(&labels),
                empty: pick(&[]),
                empty_text: "No projects",
                row_height: 1,
            }
        }
        "PickProjectWorktree" => {
            let labels = names("tree-");
            let pick = |labels: &[String], selected| {
                PromptState::PickProjectWorktree(PickProjectWorktreePrompt {
                    project: project.clone(),
                    entries: labels.iter().map(|l| project_worktree_entry(l)).collect(),
                    loading: false,
                    selected,
                    error: None,
                })
            };
            PickerFixture {
                tall: pick(&labels, Some(0)),
                last_label: last(&labels),
                empty: pick(&[], None),
                empty_text: "No available worktrees.",
                row_height: 1,
            }
        }
        "ManageWorktrees" => {
            let labels = names("tree-");
            let manage = |labels: &[String], selected| {
                PromptState::ManageWorktrees(ManageWorktreesPrompt {
                    project: project.clone(),
                    entries: labels.iter().map(|l| managed_worktree(l)).collect(),
                    loading: false,
                    selected,
                    error: None,
                })
            };
            PickerFixture {
                tall: manage(&labels, Some(0)),
                last_label: last(&labels),
                empty: manage(&[], None),
                empty_text: "No removable worktrees.",
                row_height: 1,
            }
        }
        "ChangeTheme" => {
            // Ids no theme loader knows, so moving the cursor previews nothing
            // and the screen keeps one palette to assert against.
            let labels = names("no-such-theme-");
            let change = |labels: &[String]| {
                PromptState::ChangeTheme(ChangeThemePrompt {
                    options: labels
                        .iter()
                        .map(|id| crate::theme::ThemeListing {
                            id: id.clone(),
                            display_name: id.clone(),
                            source: crate::theme::ThemeSource::User,
                        })
                        .collect(),
                    selected: 0,
                    current: "dux-dark".to_string(),
                })
            };
            PickerFixture {
                tall: change(&labels),
                last_label: last(&labels),
                empty: change(&[]),
                empty_text: "No themes",
                row_height: 1,
            }
        }
        "ChangeAgentProvider" => {
            let labels = names("provider-");
            let change = |labels: &[String]| {
                PromptState::ChangeAgentProvider(ChangeAgentProviderPrompt {
                    session_id: "s1".to_string(),
                    tab_id: "s1".to_string(),
                    session_label: "agent".to_string(),
                    worktree_path: "/tmp/wt".to_string(),
                    options: labels
                        .iter()
                        .map(|p| ChangeAgentProviderOption {
                            provider: ProviderKind::new(p),
                            supports_resume: true,
                            resume_available: false,
                            is_current: false,
                        })
                        .collect(),
                    selected: 0,
                    mode: ChangeAgentProviderMode::Retarget,
                })
            };
            PickerFixture {
                tall: change(&labels),
                last_label: last(&labels),
                empty: change(&[]),
                empty_text: "No providers",
                row_height: 1,
            }
        }
        "ChangeDefaultProvider" => {
            let labels = names("provider-");
            let change = |labels: &[String]| {
                PromptState::ChangeDefaultProvider(ChangeDefaultProviderPrompt {
                    current: ProviderKind::new("claude"),
                    options: labels
                        .iter()
                        .map(|p| ChangeDefaultProviderOption {
                            provider: ProviderKind::new(p),
                            is_current: false,
                        })
                        .collect(),
                    selected: 0,
                })
            };
            PickerFixture {
                tall: change(&labels),
                last_label: last(&labels),
                empty: change(&[]),
                empty_text: "No providers",
                row_height: 1,
            }
        }
        "ChangeProjectDefaultProvider" => {
            let labels = names("provider-");
            let change = |labels: &[String]| {
                PromptState::ChangeProjectDefaultProvider(ChangeProjectDefaultProviderPrompt {
                    project_id: project.id.clone(),
                    project_name: project.name.clone(),
                    current: ProviderKind::new("claude"),
                    global_default: ProviderKind::new("claude"),
                    inherits_global_default: true,
                    options: labels
                        .iter()
                        .map(|p| ChangeProjectDefaultProviderOption {
                            provider: Some(ProviderKind::new(p)),
                            is_current: false,
                        })
                        .collect(),
                    selected: 0,
                })
            };
            PickerFixture {
                tall: change(&labels),
                last_label: last(&labels),
                empty: change(&[]),
                empty_text: "No providers",
                row_height: 1,
            }
        }
        "SetTailscaleMode" => {
            use dux_core::config::TailscaleMode;
            let set = |count: usize| {
                PromptState::SetTailscaleMode(SetTailscaleModePrompt {
                    current: TailscaleMode::Auto,
                    options: [TailscaleMode::Auto, TailscaleMode::Yes, TailscaleMode::No]
                        .into_iter()
                        .cycle()
                        .take(count)
                        .map(|mode| SetTailscaleModeOption {
                            mode,
                            is_current: false,
                        })
                        .collect(),
                    selected: 0,
                    serving: false,
                })
            };
            PickerFixture {
                tall: set(TALL),
                last_label: None,
                empty: set(0),
                empty_text: "No modes",
                row_height: 1,
            }
        }
        "KillRunning" => {
            let labels = names("runtime-");
            let kill = |labels: &[String]| {
                PromptState::KillRunning(KillRunningPrompt {
                    runtimes: labels
                        .iter()
                        .map(|label| KillableRuntime {
                            id: RuntimeTargetId::Agent(label.clone()),
                            kind: KillableRuntimeKind::Agent,
                            label: label.clone(),
                            context: "project".to_string(),
                            search_text: label.clone(),
                        })
                        .collect(),
                    list: SearchableList::new(),
                    selected_ids: HashSet::new(),
                    focus: KillRunningFocus::List,
                })
            };
            PickerFixture {
                tall: kill(&labels),
                last_label: last(&labels),
                empty: kill(&[]),
                empty_text: "No matching running agents or terminals.",
                row_height: 1,
            }
        }
        "StartupCommandLogs" => {
            let labels = names("run-");
            let logs = |labels: &[String]| {
                PromptState::StartupCommandLogs(StartupCommandLogPrompt {
                    scope_label: "project".to_string(),
                    entries: labels
                        .iter()
                        .map(|label| crate::startup::StartupCommandLogEntry {
                            path: PathBuf::from(format!("/nonexistent/{label}.log")),
                            display_name: label.clone(),
                            modified_at: None,
                        })
                        .collect(),
                    selected: 0,
                    filter: TextInput::new(),
                    searching: false,
                    content: String::new(),
                    scroll_offset: 0,
                    wrap_width: 0,
                    focus: StartupCommandLogFocus::List,
                })
            };
            PickerFixture {
                tall: logs(&labels),
                last_label: last(&labels),
                empty: logs(&[]),
                empty_text: "No logs",
                row_height: crate::app::input::STARTUP_LOG_ROW_HEIGHT,
            }
        }
        "ResourceMonitor" => {
            let labels = names("agent-");
            let monitor = |labels: &[String]| PromptState::ResourceMonitor {
                rows: labels
                    .iter()
                    .enumerate()
                    .map(|(n, label)| resource_row(label, 100 + n as u32))
                    .collect(),
                scroll_offset: 0,
                selected_row: 0,
                expanded: HashSet::new(),
                last_refresh: Instant::now(),
                short_window_sample: false,
            };
            PickerFixture {
                tall: monitor(&labels),
                last_label: last(&labels),
                empty: monitor(&[]),
                empty_text: "Waiting for the first sample",
                row_height: 1,
            }
        }
        "EditMacros(list)" => {
            let labels = names("macro-");
            let macros = |labels: &[String]| PromptState::EditMacros {
                entries: labels
                    .iter()
                    .map(|label| {
                        (
                            label.clone(),
                            "hello".to_string(),
                            crate::config::MacroSurface::Both,
                        )
                    })
                    .collect(),
                selected: 0,
                editing: None,
                pending_delete: None,
            };
            PickerFixture {
                tall: macros(&labels),
                last_label: last(&labels),
                empty: macros(&[]),
                empty_text: "No macros defined.",
                row_height: 1,
            }
        }
        other => panic!(
            "the {other} picker has no fixture in picker_lists.rs; give it a tall one, an \
             empty one and its empty-state text"
        ),
    }
}

/// Every Picker-family modal the registry knows, by its fixture name.
fn pickers(app: &App) -> Vec<&'static str> {
    super::modal::tests::every_prompt(app)
        .into_iter()
        .filter(|(_, prompt)| {
            modal_spec(prompt).map(|spec| spec.family) == Some(ModalFamily::Picker)
        })
        .map(|(name, _)| name)
        .collect()
}

/// Run `check` once per picker, reporting every picker that fails rather than
/// stopping at the first.
fn for_each_picker(check: impl Fn(&'static str)) {
    let failures: Vec<String> = pickers(&test_app(default_bindings()))
        .into_iter()
        .filter_map(|name| {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| check(name)))
                .err()
                .map(|panic| {
                    let message = panic
                        .downcast_ref::<String>()
                        .cloned()
                        .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_string()))
                        .unwrap_or_default();
                    message.lines().next().unwrap_or("").to_string()
                })
        })
        .collect();
    assert!(
        failures.is_empty(),
        "failing pickers:\n{}",
        failures.join("\n")
    );
}

#[test]
fn every_picker_has_a_tall_fixture() {
    let app = test_app(default_bindings());
    let pickers = pickers(&app);
    assert!(pickers.len() >= 15, "the registry lists {pickers:?}");
    for name in pickers {
        let _ = fixture(&app, name);
    }
}

/// Where the highlight sits, as an index into the published list, asserting
/// there is exactly one highlighted item and it is on screen inside the list.
fn highlighted_index(app: &App, buf: &Buffer, name: &str, row_height: u16) -> usize {
    let (list, _, offset) = published_list(app.overlay_layout.active);
    let rows = highlighted_rows(app, buf);
    let shown = screen(buf);
    assert_eq!(
        rows.len(),
        usize::from(row_height),
        "{name}: expected one highlighted item of {row_height} row(s), got rows {rows:?}:\n{shown}"
    );
    let y = rows[0];
    assert!(
        y >= list.y && y + row_height <= list.y + list.height,
        "{name}: the highlight at row {y} is outside the list {list:?}:\n{shown}"
    );
    offset + usize::from((y - list.y) / row_height)
}

#[test]
fn the_cursor_moved_past_the_bottom_stays_visible_and_highlighted() {
    for_each_picker(|name| {
        // A fresh app per picker: a click on one must not pair with a click on
        // another into a double click.
        let mut app = test_app(default_bindings());
        let fixture = fixture(&app, name);
        app.prompt = fixture.tall;
        let buf = render(&mut app);
        let (list, items, _) = published_list(app.overlay_layout.active);
        assert!(
            items * usize::from(fixture.row_height) > usize::from(list.height),
            "{name}: {items} items fit in {list:?}, so nothing is tested:\n{}",
            screen(&buf)
        );
        for _ in 0..items + 3 {
            press(&mut app, KeyCode::Down);
        }
        let buf = render(&mut app);
        let (_, items, _) = published_list(app.overlay_layout.active);
        let index = highlighted_index(&app, &buf, name, fixture.row_height);
        assert_eq!(
            index,
            items - 1,
            "{name}: the cursor ran to the last row, but row {index} is highlighted:\n{}",
            screen(&buf)
        );
        if let Some(label) = &fixture.last_label {
            let y = highlighted_rows(&app, &buf)[0];
            assert!(
                row_text(&buf, y).contains(label.as_str()),
                "{name}: the highlighted row does not carry {label:?}:\n{}",
                screen(&buf)
            );
        }
    });
}

/// The picker lists wear the scroll indicator the welcome and What's new
/// screens wear: the shared one-cell marker, in the same color.
#[test]
fn a_tall_picker_shows_the_welcome_screens_scroll_indicator() {
    for_each_picker(|name| {
        // A fresh app per picker: a click on one must not pair with a click on
        // another into a double click.
        let mut app = test_app(default_bindings());
        // Key badges and the indicator share a color in most themes; pull them
        // apart so the indicator has to be the welcome screen's own.
        app.theme.hint_key_fg = ratatui::style::Color::Rgb(0x12, 0x34, 0x56);
        let fixture = fixture(&app, name);
        app.prompt = fixture.tall;
        let buf = render(&mut app);
        let (list, _, _) = published_list(app.overlay_layout.active);
        let color = components::scroll_indicator_color(&app.theme);
        // The marker sits in the border column right of the list, on its
        // last row.
        let cell = &buf[(list.x + list.width, list.y + list.height - 1)];
        assert!(
            MARKER_GLYPHS.contains(&cell.symbol()) && cell.fg == color,
            "{name}: no scroll indicator ({:?} in {color:?}) beside the list's last row, found \
             {:?} in {:?}:\n{}",
            MARKER_GLYPHS,
            cell.symbol(),
            cell.fg,
            screen(&buf)
        );
    });
}

#[test]
fn a_click_on_a_visible_row_selects_that_row() {
    for_each_picker(|name| {
        // A fresh app per picker: a click on one must not pair with a click on
        // another into a double click.
        let mut app = test_app(default_bindings());
        let fixture = fixture(&app, name);
        app.prompt = fixture.tall;
        render(&mut app);
        let (_, items, _) = published_list(app.overlay_layout.active);
        for _ in 0..items + 3 {
            press(&mut app, KeyCode::Down);
        }
        let buf = render(&mut app);
        let (list, _, _) = published_list(app.overlay_layout.active);
        let y = highlighted_rows(&app, &buf)[0];
        let before = highlighted_index(&app, &buf, name, fixture.row_height);
        // The row just above the highlight: visible, and a different item.
        click(&mut app, list.x + 2, y - fixture.row_height);
        let buf = render(&mut app);
        assert!(
            !matches!(app.prompt, PromptState::None),
            "{name}: one click closed the picker:\n{}",
            screen(&buf)
        );
        let after = highlighted_index(&app, &buf, name, fixture.row_height);
        assert_eq!(
            after,
            before - 1,
            "{name}: clicking the row above the cursor did not select it:\n{}",
            screen(&buf)
        );
    });
}

#[test]
fn the_empty_state_renders_without_a_highlight() {
    for_each_picker(|name| {
        // A fresh app per picker: a click on one must not pair with a click on
        // another into a double click.
        let mut app = test_app(default_bindings());
        let fixture = fixture(&app, name);
        app.prompt = fixture.empty;
        let buf = render(&mut app);
        let shown = screen(&buf);
        assert!(
            shown.contains(fixture.empty_text),
            "{name}: the empty state {:?} is not on screen:\n{shown}",
            fixture.empty_text
        );
        assert!(
            highlighted_rows(&app, &buf).is_empty(),
            "{name}: an empty list has nothing to pick, yet a row is highlighted:\n{shown}"
        );
    });
}

/// The kill-running list publishes its layout once, after its footer buttons,
/// so the scroll offset a click resolves against must be the one the list just
/// drew. Scroll well past the first page, click a visible row, and the runtime
/// named on that row is the one the cursor lands on.
#[test]
fn a_click_on_a_scrolled_kill_running_list_selects_the_clicked_row() {
    let mut app = test_app(default_bindings());
    let fixture = fixture(&app, "KillRunning");
    app.prompt = fixture.tall;
    render(&mut app);
    for _ in 0..TALL {
        press(&mut app, KeyCode::Down);
    }
    let buf = render(&mut app);
    let (list, items, offset) = published_list(app.overlay_layout.active);
    assert_eq!(items, TALL, "every runtime is published");
    assert!(
        offset > 0,
        "the list must be scrolled for this to mean anything"
    );
    // Two rows above the highlight: visible, and a runtime the cursor is not on.
    let y = highlighted_rows(&app, &buf)[0] - 2;
    let clicked = row_text(&buf, y);
    let expected = (0..TALL)
        .find(|n| clicked.contains(&format!("runtime-{n:02}")))
        .expect("the clicked row names a runtime");
    click(&mut app, list.x + 2, y);
    let PromptState::KillRunning(prompt) = &app.prompt else {
        panic!("the click closed the picker");
    };
    assert_eq!(
        prompt.list.selected,
        expected,
        "clicked the row naming runtime-{expected:02}:\n{}",
        screen(&buf)
    );
}

/// The other half of "the same indicator": the welcome and What's new screens
/// draw theirs through the same shared scroll view, in the same color, so the
/// pickers and those two screens cannot drift apart.
#[test]
fn the_welcome_and_whats_new_screens_wear_the_same_indicator() {
    use super::first_load::FirstLoadPrompt;
    let notes = dux_core::release_notes::ReleaseNotes {
        version: "v0.7.0".to_string(),
        headline: "Quieter plumbing, louder failures".to_string(),
        paragraphs: vec!["A tune-up release with fewer surprises.".to_string()],
        sections: (0..TALL).map(|n| format!("Feature number {n}")).collect(),
        html_url: "https://example.invalid/v0.7.0".to_string(),
    };
    let app = test_app(default_bindings());
    let welcome = dux_core::welcome_screen::welcome_screen(&app.engine.paths.config_path);
    for (label, prompt) in [
        ("welcome", FirstLoadPrompt::welcome(welcome, false)),
        ("What's new", FirstLoadPrompt::whats_new(notes, false)),
    ] {
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::FirstLoad(prompt);
        let buf = render(&mut app);
        let color = components::scroll_indicator_color(&app.theme);
        let markers: Vec<_> = buf
            .content()
            .iter()
            .filter(|cell| MARKER_GLYPHS.contains(&cell.symbol()))
            .collect();
        assert!(
            !markers.is_empty() && markers.iter().all(|cell| cell.fg == color),
            "the {label} screen at 80x24 must scroll and mark it in {color:?}:\n{}",
            screen(&buf)
        );
    }
}
