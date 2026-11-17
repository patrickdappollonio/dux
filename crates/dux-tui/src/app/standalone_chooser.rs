//! The standalone picker's project chooser: the user's projects as folder
//! shortcuts above the folder explorer. The rows are core's
//! (`dux_core::standalone_shortcuts`); these tests cover what the terminal UI
//! does with them.

use std::path::PathBuf;

use dux_core::standalone_shortcuts::{
    CAPTION, ProjectShortcut, ProjectShortcutList, ShortcutState,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;

use super::test_support::{default_bindings, finish_ui_jobs, test_app};
use super::{App, BrowserEntry, OverlayMouseLayout, PromptState, StandaloneProjectChooser};

fn press(app: &mut App, code: KeyCode) {
    app.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
        .expect("the key is handled");
}

fn row(id: &str, path: &str, state: ShortcutState) -> ProjectShortcut {
    ProjectShortcut {
        project_id: id.to_string(),
        name: id.to_string(),
        folder_label: format!("~/{id}"),
        path: path.to_string(),
        state,
        reason: state.reason(),
    }
}

/// The picker's explorer folder, and whether the chooser has the keys.
fn explorer_and_focus(app: &App) -> (PathBuf, bool) {
    match &app.prompt {
        PromptState::BrowseProjects {
            current_dir,
            project_chooser,
            ..
        } => (current_dir.clone(), project_chooser.focused),
        other => panic!("expected the folder browser, got {other:?}"),
    }
}

fn buffer(app: &mut App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|frame| app.render(frame)).expect("render");
    terminal.backend().buffer().clone()
}

fn rows_of(buffer: &Buffer) -> Vec<String> {
    let width = usize::from(buffer.area.width);
    buffer
        .content()
        .chunks(width)
        .map(|cells| cells.iter().map(ratatui::buffer::Cell::symbol).collect())
        .collect()
}

/// The picker painted at 120x40, as text.
fn screen(app: &mut App) -> String {
    rows_of(&buffer(app, 120, 40)).join("\n")
}

/// The picker painted at 80x24, as text.
fn narrow_screen(app: &mut App) -> String {
    rows_of(&buffer(app, 80, 24)).join("\n")
}

fn mouse(app: &mut App, kind: MouseEventKind, column: u16, row: u16) {
    app.handle_mouse(MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    });
}

fn click(app: &mut App, column: u16, row: u16) {
    mouse(app, MouseEventKind::Down(MouseButton::Left), column, row);
}

/// Paint at 120x40 and return where the chooser's rows and the explorer's
/// rows landed, and the chooser's first row on screen.
fn rects(app: &mut App) -> (Rect, Rect, usize) {
    let _ = screen(app);
    match app.overlay_layout.active {
        OverlayMouseLayout::BrowseProjects {
            list,
            chooser: Some(chooser),
            ..
        } => (chooser.list, list, chooser.offset),
        ref other => panic!("unexpected layout {other:?}"),
    }
}

fn chooser(app: &App) -> &StandaloneProjectChooser {
    match &app.prompt {
        PromptState::BrowseProjects {
            project_chooser, ..
        } => project_chooser,
        other => panic!("expected the folder browser, got {other:?}"),
    }
}

/// Open the picker, let core's rows land, then put the explorer in a folder
/// with one child so a click on its first row has something to open.
fn open_with_child(app: &mut App) -> PathBuf {
    app.execute_command("new-standalone-agent").expect("opens");
    finish_ui_jobs(app);
    let dir = tempfile::tempdir().expect("tempdir").keep();
    let child = dir.join("child");
    std::fs::create_dir_all(&child).expect("mkdir");
    if let PromptState::BrowseProjects {
        current_dir,
        entries,
        loading,
        selected,
        ..
    } = &mut app.prompt
    {
        current_dir.clone_from(&dir);
        *entries = vec![BrowserEntry {
            path: child,
            label: "child/".to_string(),
            is_git_repo: false,
            is_parent: false,
        }];
        *loading = false;
        *selected = 0;
    }
    dir
}

/// Open the picker and replace its rows with `rows`.
fn open_with_rows(app: &mut App, rows: Vec<ProjectShortcut>) {
    app.execute_command("new-standalone-agent").expect("opens");
    finish_ui_jobs(app);
    app.show_standalone_project_shortcuts(&ProjectShortcutList {
        caption: CAPTION,
        rows,
    });
}

fn mixed_rows() -> Vec<ProjectShortcut> {
    vec![
        row(
            "taken",
            "/srv/taken",
            ShortcutState::AlreadyHasStandaloneAgent,
        ),
        row("free", "/srv/free", ShortcutState::Available),
        row("gone", "/srv/gone", ShortcutState::FolderMissing),
    ]
}

/// The journey: open the standalone picker with a project registered. Core's
/// rows arrive from the worker, the chooser takes the keys, and picking the
/// project moves the explorer to its folder and hands the keys down to it.
#[test]
fn picking_a_project_moves_the_explorer_to_its_folder() {
    let mut app = test_app(default_bindings());
    let project_path = PathBuf::from(&app.engine.projects[0].path);

    app.execute_command("new-standalone-agent")
        .expect("the picker opens");
    finish_ui_jobs(&mut app);

    let PromptState::BrowseProjects {
        project_chooser, ..
    } = &app.prompt
    else {
        panic!("expected the folder browser");
    };
    assert_eq!(project_chooser.entries.len(), 1);
    assert_eq!(project_chooser.entries[0].id, "project-1");
    assert_eq!(project_chooser.entries[0].blocked(), None);
    assert!(
        project_chooser.focused,
        "the chooser leads, so it has the keys"
    );

    press(&mut app, KeyCode::Enter);

    let (current_dir, chooser_focused) = explorer_and_focus(&app);
    assert_eq!(current_dir, project_path);
    assert!(!chooser_focused, "the explorer has the keys after a pick");
    assert!(
        matches!(
            app.prompt,
            PromptState::BrowseProjects { loading: true, .. }
        ),
        "the explorer is listing the project's folder"
    );
}

/// The cursor visits every row, greyed-out ones included, so they can be
/// read at any size; a greyed-out row under the cursor never looks like a
/// pickable selection, and picking it leaves the explorer where it was and
/// says why in core's words.
#[test]
fn a_greyed_out_project_is_reachable_but_cannot_be_picked() {
    let mut app = test_app(default_bindings());
    open_with_rows(&mut app, mixed_rows());
    let (start_dir, _) = explorer_and_focus(&app);
    assert_eq!(
        chooser(&app).list.selected,
        1,
        "the cursor starts on the row that can be picked"
    );

    press(&mut app, KeyCode::Up);
    assert_eq!(
        chooser(&app).list.selected,
        0,
        "and moves onto a greyed-out one"
    );

    let (chooser_list, _, _) = rects(&mut app);
    let painted = buffer(&mut app, 120, 40);
    let cell = &painted[(chooser_list.x + 4, chooser_list.y)];
    assert_ne!(
        Some(cell.bg),
        app.theme.selection_style().bg,
        "a greyed-out row under the cursor is painted like a pickable selection"
    );
    assert_eq!(
        Some(cell.bg),
        app.theme.unavailable_selection_style().bg,
        "it wears the selected-but-unavailable look"
    );

    press(&mut app, KeyCode::Enter);
    let (current_dir, chooser_focused) = explorer_and_focus(&app);
    assert_eq!(current_dir, start_dir, "the explorer did not move");
    assert!(chooser_focused);
    assert!(
        app.status.text().contains("already has a standalone agent"),
        "{}",
        app.status.text()
    );

    press(&mut app, KeyCode::Down);
    press(&mut app, KeyCode::Down);
    assert_eq!(
        chooser(&app).list.selected,
        2,
        "the last row is greyed out too"
    );
    assert!(explorer_and_focus(&app).1);
    press(&mut app, KeyCode::Down);
    assert!(
        !explorer_and_focus(&app).1,
        "down past the last row leads into the explorer"
    );
}

/// A search that leaves only a greyed-out row does not paint it as a
/// pickable selection either.
#[test]
fn a_search_that_leaves_only_a_greyed_out_row_keeps_it_greyed() {
    let mut app = test_app(default_bindings());
    open_with_rows(&mut app, mixed_rows());
    press(&mut app, KeyCode::Char('/'));
    for ch in "taken".chars() {
        press(&mut app, KeyCode::Char(ch));
    }
    let (chooser_list, _, _) = rects(&mut app);
    let painted = buffer(&mut app, 120, 40);
    assert_ne!(
        Some(painted[(chooser_list.x + 4, chooser_list.y)].bg),
        app.theme.selection_style().bg
    );
}

/// The pane-focus keys move between the chooser and the explorer, and so do
/// the movement keys at the seam between them.
#[test]
fn focus_moves_between_the_chooser_and_the_explorer() {
    let mut app = test_app(default_bindings());
    app.execute_command("new-standalone-agent")
        .expect("the picker opens");
    finish_ui_jobs(&mut app);
    assert!(explorer_and_focus(&app).1);

    press(&mut app, KeyCode::Tab);
    assert!(!explorer_and_focus(&app).1, "the focus key hands over");
    press(&mut app, KeyCode::Tab);
    assert!(explorer_and_focus(&app).1, "and hands back");

    press(&mut app, KeyCode::Down);
    assert!(
        !explorer_and_focus(&app).1,
        "down past the last project leads into the explorer"
    );
    press(&mut app, KeyCode::Up);
    assert!(
        explorer_and_focus(&app).1,
        "up from the explorer's first row leads back"
    );
}

/// With no projects there is no chooser, and the explorer has the keys.
#[test]
fn no_projects_means_no_chooser() {
    let mut app = test_app(default_bindings());
    app.engine.sessions.clear();
    app.engine.projects.clear();
    app.execute_command("new-standalone-agent")
        .expect("the picker opens");
    finish_ui_jobs(&mut app);

    assert!(!chooser(&app).is_shown());
    assert!(!screen(&mut app).contains("Start In A Project's Folder"));
}

/// One click on a row of each list is two single clicks, whichever comes
/// first: the two lists do not count each other's clicks toward a double.
#[test]
fn a_click_on_each_list_is_never_a_double_click() {
    let mut app = test_app(default_bindings());
    let dir = open_with_child(&mut app);
    let (chooser_list, explorer_list, _) = rects(&mut app);
    click(&mut app, chooser_list.x + 1, chooser_list.y);
    let _ = rects(&mut app);
    click(&mut app, explorer_list.x + 1, explorer_list.y);
    assert_eq!(
        explorer_and_focus(&app).0,
        dir,
        "a single click on the explorer's first row opened it"
    );

    let mut app = test_app(default_bindings());
    let dir = open_with_child(&mut app);
    let (chooser_list, explorer_list, _) = rects(&mut app);
    click(&mut app, explorer_list.x + 1, explorer_list.y);
    let _ = rects(&mut app);
    click(&mut app, chooser_list.x + 1, chooser_list.y);
    assert_eq!(
        explorer_and_focus(&app).0,
        dir,
        "a single click on the chooser's first row picked the project"
    );
}

/// The chooser's own mouse paths: a click selects the row and takes the
/// keys, a second click on it picks the project, and a click into the search
/// field lands the caret there.
#[test]
fn the_chooser_answers_the_mouse() {
    let mut app = test_app(default_bindings());
    open_with_rows(&mut app, mixed_rows());
    press(&mut app, KeyCode::Tab);
    assert!(!explorer_and_focus(&app).1);

    let (chooser_list, _, _) = rects(&mut app);
    click(&mut app, chooser_list.x + 1, chooser_list.y + 1);
    assert!(
        explorer_and_focus(&app).1,
        "a click gives the chooser the keys"
    );
    assert_eq!(chooser(&app).list.selected, 1);
    assert_ne!(explorer_and_focus(&app).0, PathBuf::from("/srv/free"));

    let _ = rects(&mut app);
    click(&mut app, chooser_list.x + 1, chooser_list.y + 1);
    let (current_dir, chooser_focused) = explorer_and_focus(&app);
    assert_eq!(
        current_dir,
        PathBuf::from("/srv/free"),
        "a double click picks"
    );
    assert!(!chooser_focused);

    // The search field, once it is the thing drawn.
    press(&mut app, KeyCode::Tab);
    press(&mut app, KeyCode::Char('/'));
    press(&mut app, KeyCode::Char('f'));
    press(&mut app, KeyCode::Tab);
    assert!(!explorer_and_focus(&app).1);
    let _ = screen(&mut app);
    let OverlayMouseLayout::BrowseProjects {
        chooser: Some(layout),
        ..
    } = app.overlay_layout.active
    else {
        panic!("the chooser is drawn");
    };
    let field = layout.input.expect("the search field publishes its rect");
    click(&mut app, field.x + 2, field.y);
    assert!(
        explorer_and_focus(&app).1,
        "a click in the field takes the keys"
    );
    assert!(chooser(&app).list.searching);
    assert_eq!(
        chooser(&app).list.filter.cursor,
        0,
        "the caret lands at the click"
    );
}

/// Whichever half has the keys is the half the screen describes: the status
/// line at opening says how to reach the folders rather than advertising keys
/// the chooser does not answer, and a half without the keys shows only how to
/// reach it.
#[test]
fn the_screen_describes_the_half_that_has_the_keys() {
    let mut app = test_app(default_bindings());
    app.execute_command("new-standalone-agent").expect("opens");
    let opening = app.status.text();
    finish_ui_jobs(&mut app);
    assert!(explorer_and_focus(&app).1);
    assert!(
        opening.contains("Tab") && opening.contains("project"),
        "the opening status says how the two halves are reached: {opening}"
    );

    let painted = screen(&mut app);
    assert!(painted.contains("browse there"), "{painted}");
    assert!(
        !painted.contains("add current"),
        "the explorer's keys are shown while it does not have them:\n{painted}"
    );
    assert!(painted.contains("<Tab> folders"), "{painted}");

    press(&mut app, KeyCode::Tab);
    let painted = screen(&mut app);
    assert!(painted.contains("add current"), "{painted}");
    assert!(
        !painted.contains("browse there"),
        "the chooser's keys are shown while it does not have them:\n{painted}"
    );
    assert!(painted.contains("<Tab> projects"), "{painted}");
}

/// Rows that land after the user has already pressed a key in the picker do
/// not take the keys from under them, even with the explorer's cursor back
/// on its first row; the chooser appears, unfocused.
#[test]
fn rows_landing_after_a_keypress_do_not_take_the_keys() {
    let mut app = test_app(default_bindings());
    app.execute_command("new-standalone-agent").expect("opens");
    press(&mut app, KeyCode::Down);
    finish_ui_jobs(&mut app);

    assert!(chooser(&app).is_shown());
    assert!(
        !explorer_and_focus(&app).1,
        "the chooser took the keys mid-browse"
    );
    press(&mut app, KeyCode::Tab);
    assert!(explorer_and_focus(&app).1, "the focus key still reaches it");
}

fn many_rows() -> Vec<ProjectShortcut> {
    (0..30)
        .map(|n| {
            row(
                &format!("service-{n:02}"),
                &format!("/srv/service-{n:02}"),
                ShortcutState::Available,
            )
        })
        .collect()
}

/// The chooser keeps its scroll position when the keys leave it.
#[test]
fn the_chooser_keeps_its_place_when_focus_leaves() {
    let mut app = test_app(default_bindings());
    open_with_rows(&mut app, many_rows());
    for _ in 0..20 {
        press(&mut app, KeyCode::Down);
    }
    let (_, _, focused_offset) = rects(&mut app);
    assert!(focused_offset > 0, "the list scrolled");

    press(&mut app, KeyCode::Tab);
    assert!(!explorer_and_focus(&app).1);
    let (_, _, resting_offset) = rects(&mut app);
    assert_eq!(resting_offset, focused_offset, "the list snapped back");
}

/// The wheel moves through the chooser's rows, and through the project
/// chooser modal's, which shares its drawing.
#[test]
fn the_wheel_scrolls_both_project_choosers() {
    let mut app = test_app(default_bindings());
    open_with_rows(&mut app, many_rows());
    let (chooser_list, _, _) = rects(&mut app);
    mouse(
        &mut app,
        MouseEventKind::ScrollDown,
        chooser_list.x + 1,
        chooser_list.y,
    );
    assert!(chooser(&app).list.selected > 0, "the wheel did nothing");
    mouse(
        &mut app,
        MouseEventKind::ScrollUp,
        chooser_list.x + 1,
        chooser_list.y,
    );
    assert_eq!(chooser(&app).list.selected, 0);

    let mut app = test_app(default_bindings());
    let mut second = app.engine.projects[0].clone();
    second.id = "project-2".to_string();
    second.name = "second".to_string();
    app.engine.projects.push(second);
    app.execute_command("new-agent").expect("the chooser opens");
    let _ = screen(&mut app);
    let OverlayMouseLayout::PickProject { list, .. } = app.overlay_layout.active else {
        panic!("the project chooser is drawn");
    };
    mouse(&mut app, MouseEventKind::ScrollDown, list.x + 1, list.y);
    let PromptState::PickProject { list, .. } = &app.prompt else {
        panic!("the project chooser is open");
    };
    assert_eq!(list.selected, 1, "the wheel did nothing");
}

/// A worker that never answers leaves no chooser, and says so rather than
/// leaving the user wondering where their projects went.
#[test]
fn a_shortcut_worker_that_fails_says_so() {
    let mut app = test_app(default_bindings());
    super::ui_jobs::fail_next_quiet_spawn();
    app.execute_command("new-standalone-agent").expect("opens");
    finish_ui_jobs(&mut app);

    assert!(!chooser(&app).is_shown());
    assert!(
        app.status.text().contains("projects"),
        "{}",
        app.status.text()
    );
    assert_eq!(app.status.tone(), crate::statusline::StatusTone::Warning);
}

/// At 80x24: the caption wraps rather than being cut, a folder is shown
/// whole where it fits, a greyed-out row's reason never squeezes a row past
/// the modal, the way between the halves is still named, and every row can be
/// reached.
#[test]
fn the_picker_holds_up_on_a_narrow_screen() {
    let mut app = test_app(default_bindings());
    open_with_rows(&mut app, mixed_rows());

    let painted = narrow_screen(&mut app);
    assert!(
        painted.contains("environment") && painted.contains("startup command."),
        "the caption is cut:\n{painted}"
    );
    assert!(painted.contains("~/free"), "the folder is cut:\n{painted}");
    assert!(
        painted.contains("folders"),
        "no way between halves:\n{painted}"
    );
    for line in painted.lines() {
        assert_eq!(line.chars().count(), 80, "a row overflowed: {line}");
    }

    // Every row, the greyed-out ones included, can be brought on screen.
    let mut seen = String::new();
    press(&mut app, KeyCode::Up);
    for _ in 0..3 {
        seen.push_str(&narrow_screen(&mut app));
        press(&mut app, KeyCode::Down);
    }
    assert!(seen.contains("already has a standalone"), "{seen}");
    assert!(seen.contains("folder missing"), "{seen}");

    // The last press went down past the last row, into the explorer, whose
    // half now names the way back up.
    assert!(!explorer_and_focus(&app).1);
    let painted = narrow_screen(&mut app);
    assert!(
        painted.contains("<Tab> projects"),
        "no way back up:\n{painted}"
    );
}

/// With no greyed-out row among those shown, no room is set aside for a
/// reason: the folder takes it.
#[test]
fn a_reason_takes_room_only_when_a_greyed_out_row_is_shown() {
    let mut app = test_app(default_bindings());
    open_with_rows(
        &mut app,
        vec![
            row("free", "/srv/free", ShortcutState::Available),
            row(
                "taken",
                "/srv/taken",
                ShortcutState::AlreadyHasStandaloneAgent,
            ),
        ],
    );
    if let PromptState::BrowseProjects {
        project_chooser, ..
    } = &mut app.prompt
    {
        project_chooser.entries[0]
            .shortcut
            .as_mut()
            .expect("a shortcut row")
            .folder_label = "~/work/clients/acme/services/free".to_string();
    }
    press(&mut app, KeyCode::Char('/'));
    for ch in "free".chars() {
        press(&mut app, KeyCode::Char(ch));
    }
    let painted = narrow_screen(&mut app);
    assert!(
        painted.contains("~/work/clients/acme/services/free"),
        "the folder is cut though nothing else needs the room:\n{painted}"
    );
}
