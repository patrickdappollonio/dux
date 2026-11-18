//! The standalone picker's project chooser: the user's projects as folder
//! shortcuts above the folder explorer. The rows are core's
//! (`dux_core::standalone_shortcuts`); these tests cover what the terminal UI
//! does with them.

use std::path::PathBuf;

use dux_core::standalone_shortcuts::{
    CAPTION, ProjectShortcut, ProjectShortcutList, ShortcutCheck, ShortcutState,
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
    app.set_checked_project_shortcuts(&ProjectShortcutList {
        caption: CAPTION,
        rows,
    });
}

/// Fill the explorer with `count` child folders, loaded.
fn fill_explorer(app: &mut App, count: usize) {
    if let PromptState::BrowseProjects {
        entries, loading, ..
    } = &mut app.prompt
    {
        *entries = (0..count)
            .map(|n| BrowserEntry {
                path: PathBuf::from(format!("/zz/child-{n:02}")),
                label: format!("child-{n:02}/"),
                is_git_repo: false,
                is_parent: false,
            })
            .collect();
        *loading = false;
    }
}

fn explorer_selected(app: &App) -> usize {
    match &app.prompt {
        PromptState::BrowseProjects { selected, .. } => *selected,
        other => panic!("expected the folder browser, got {other:?}"),
    }
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
    assert!(project_chooser.entries[0].is_pickable());
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
        0,
        "the cursor rests on the first row, greyed out or not"
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
        !painted.contains("run agent here"),
        "the explorer's keys are shown while it does not have them:\n{painted}"
    );
    assert!(painted.contains("<Tab> folders"), "{painted}");

    press(&mut app, KeyCode::Tab);
    let painted = screen(&mut app);
    assert!(painted.contains("run agent here"), "{painted}");
    assert!(
        !painted.contains("browse there"),
        "the chooser's keys are shown while it does not have them:\n{painted}"
    );
    assert!(painted.contains("<Tab> projects"), "{painted}");
}

/// The chooser is there, with the keys, in the very first frame: its rows
/// are the projects the engine already knows, before any folder has been
/// checked. A row not checked yet is drawn as an ordinary row and cannot be
/// picked into a wrong outcome; the worker's answer then fills the states in
/// without moving the cursor or the keys.
#[test]
fn the_chooser_has_the_keys_before_any_folder_is_checked() {
    let mut app = test_app(default_bindings());
    app.execute_command("new-standalone-agent").expect("opens");
    let (start_dir, focused) = explorer_and_focus(&app);
    assert!(
        focused,
        "the chooser has the keys before the worker answers"
    );
    assert_eq!(chooser(&app).entries.len(), 1);
    assert_eq!(
        chooser(&app).entries[0].shortcut,
        Some(ShortcutCheck::Pending)
    );
    let opening = app.status.text();
    assert!(
        opening.contains("project") && opening.contains("Tab"),
        "the opening status describes the half that has the keys: {opening}"
    );

    let painted = screen(&mut app);
    assert!(painted.contains("Start In A Project's Folder"), "{painted}");
    assert!(!painted.contains("could not be checked"), "{painted}");

    press(&mut app, KeyCode::Enter);
    assert_eq!(explorer_and_focus(&app).0, start_dir, "nothing was picked");
    assert!(
        app.status.text().contains("still being checked"),
        "{}",
        app.status.text()
    );

    finish_ui_jobs(&mut app);
    assert_eq!(
        chooser(&app).entries[0].shortcut,
        Some(ShortcutCheck::Checked(ShortcutState::Available))
    );
    assert!(
        explorer_and_focus(&app).1,
        "the answer did not move the keys"
    );
    press(&mut app, KeyCode::Enter);
    assert_ne!(
        explorer_and_focus(&app).0,
        start_dir,
        "now it can be picked"
    );
}

/// A key the folder browser binds, pressed while the project list has the
/// keys, is not swallowed in silence: the status line says who has the keys
/// and which key reaches the folder browser. A key bound to nothing stays
/// silent.
#[test]
fn a_folder_browser_key_pressed_in_the_chooser_says_where_the_keys_are() {
    let mut app = test_app(default_bindings());
    app.execute_command("new-standalone-agent").expect("opens");
    finish_ui_jobs(&mut app);
    let before = app.status.text();

    press(&mut app, KeyCode::Char('z'));
    assert_eq!(app.status.text(), before, "an unbound key says nothing");

    press(&mut app, KeyCode::Char('o'));
    assert!(matches!(app.prompt, PromptState::BrowseProjects { .. }));
    let said = app.status.text();
    assert!(
        said.contains("project list has the keys") && said.contains("Tab"),
        "{said}"
    );
}

/// The wheel over the half without the keys hands it the keys, as a click
/// there does, and then moves, so a notch always shows.
#[test]
fn the_wheel_over_a_resting_half_takes_the_keys_and_moves() {
    let mut app = test_app(default_bindings());
    open_with_rows(&mut app, many_rows());
    fill_explorer(&mut app, 60);
    let (chooser_list, explorer_list, _) = rects(&mut app);

    mouse(
        &mut app,
        MouseEventKind::ScrollDown,
        explorer_list.x + 1,
        explorer_list.y + 1,
    );
    assert!(!explorer_and_focus(&app).1, "the explorer took the keys");
    assert_eq!(explorer_selected(&app), 3);

    mouse(
        &mut app,
        MouseEventKind::ScrollDown,
        chooser_list.x + 1,
        chooser_list.y,
    );
    assert!(explorer_and_focus(&app).1, "the chooser took the keys back");
    assert_eq!(chooser(&app).list.selected, 3);
}

/// The wheel never moves the explorer's cursor past its last entry, and does
/// nothing to it while the path editor is up, in the Add Project browser as
/// in this one.
#[test]
fn the_wheel_keeps_the_explorer_cursor_on_a_real_row() {
    let mut app = test_app(default_bindings());
    app.execute_command("add-project").expect("opens");
    finish_ui_jobs(&mut app);
    fill_explorer(&mut app, 2);
    let _ = screen(&mut app);
    let OverlayMouseLayout::BrowseProjects { list, .. } = app.overlay_layout.active else {
        panic!("the browser is drawn");
    };
    for _ in 0..4 {
        mouse(&mut app, MouseEventKind::ScrollDown, list.x + 1, list.y);
    }
    assert_eq!(explorer_selected(&app), 1, "the cursor left the entries");

    if let PromptState::BrowseProjects {
        selected,
        editing_path,
        tab_completions,
        ..
    } = &mut app.prompt
    {
        *selected = 0;
        *editing_path = true;
        *tab_completions = (0..40).map(|n| format!("/zz/c{n:02}/")).collect();
    }
    let _ = screen(&mut app);
    let OverlayMouseLayout::BrowseProjects { list, .. } = app.overlay_layout.active else {
        panic!("the browser is drawn");
    };
    for _ in 0..4 {
        mouse(&mut app, MouseEventKind::ScrollDown, list.x + 1, list.y + 1);
    }
    assert_eq!(
        explorer_selected(&app),
        0,
        "the wheel moved the hidden selection under the path editor"
    );
}

/// A search matches what the rows show: the folder as written on the row,
/// not the absolute path behind it.
#[test]
fn a_search_matches_the_folder_the_row_shows() {
    let mut app = test_app(default_bindings());
    open_with_rows(&mut app, mixed_rows());
    press(&mut app, KeyCode::Char('/'));
    for ch in "~/free".chars() {
        press(&mut app, KeyCode::Char(ch));
    }
    assert_eq!(
        chooser(&app).visible().len(),
        1,
        "the text on the row finds it"
    );

    let mut app = test_app(default_bindings());
    open_with_rows(&mut app, mixed_rows());
    press(&mut app, KeyCode::Char('/'));
    for ch in "srv".chars() {
        press(&mut app, KeyCode::Char(ch));
    }
    assert_eq!(
        chooser(&app).visible().len(),
        0,
        "a part of the path no row shows matched"
    );
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

/// A worker that stops before answering leaves the rows listed, greyed out as
/// unchecked and impossible to pick, and says so. An answer or a failure that
/// belongs to a picker since closed says nothing.
#[test]
fn a_shortcut_worker_that_fails_says_so_and_only_to_its_own_picker() {
    let mut app = test_app(default_bindings());
    super::ui_jobs::fail_next_quiet_spawn();
    app.execute_command("new-standalone-agent").expect("opens");
    finish_ui_jobs(&mut app);

    assert!(chooser(&app).is_shown(), "the projects stay listed");
    assert_eq!(
        chooser(&app).entries[0].shortcut,
        Some(ShortcutCheck::Failed)
    );
    assert!(!chooser(&app).entries[0].is_pickable());
    assert_eq!(app.status.tone(), crate::statusline::StatusTone::Warning);
    assert!(screen(&mut app).contains("could not be checked"));
    let (start_dir, _) = explorer_and_focus(&app);
    press(&mut app, KeyCode::Enter);
    assert_eq!(explorer_and_focus(&app).0, start_dir);

    // The same failure arriving for a picker the user has closed.
    let mut app = test_app(default_bindings());
    app.execute_command("new-standalone-agent").expect("opens");
    finish_ui_jobs(&mut app);
    let instance = app.browser_add.instance;
    press(&mut app, KeyCode::Esc);
    assert!(matches!(app.prompt, PromptState::None));
    let before = app.status.text();
    app.finish_standalone_shortcut_check(instance, None);
    assert_eq!(
        app.status.text(),
        before,
        "a closed picker was warned about"
    );
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

    assert!(
        painted.contains("already has a standalone agent"),
        "the reason is cut:\n{painted}"
    );
    assert!(
        painted.contains("browse there"),
        "the key that acts is gone from the chooser's footer:\n{painted}"
    );

    // Every row, the greyed-out ones included, can be brought on screen.
    let mut seen = String::new();
    for _ in 0..3 {
        seen.push_str(&narrow_screen(&mut app));
        press(&mut app, KeyCode::Down);
    }
    assert!(seen.contains("folder missing"), "{seen}");

    // The last press went down past the last row, into the explorer, whose
    // half now names the way back up and still names the key that acts.
    assert!(!explorer_and_focus(&app).1);
    let painted = narrow_screen(&mut app);
    assert!(
        painted.contains("<Tab> projects"),
        "no way back up:\n{painted}"
    );
    assert!(
        painted.contains("run agent here"),
        "the key that acts is gone from the explorer's footer:\n{painted}"
    );
}

/// The caption is the sentence that says a shortcut brings the folder and
/// nothing else, so it is whole wherever the terminal can hold it over one
/// list row, and where it cannot it ends in an ellipsis rather than stopping
/// mid-sentence.
#[test]
fn the_caption_is_whole_or_visibly_cut_on_short_and_narrow_screens() {
    for (width, height) in [(80, 16), (50, 20), (40, 24)] {
        let mut app = test_app(default_bindings());
        open_with_rows(&mut app, mixed_rows());
        let painted = rows_of(&buffer(&mut app, width, height)).join("\n");
        assert!(
            painted.contains("Uses the") && painted.contains("command."),
            "the caption is cut at {width}x{height}:\n{painted}"
        );
        assert!(
            painted.contains("taken") || painted.contains("free") || painted.contains("gone"),
            "no project row is left at {width}x{height}:\n{painted}"
        );
        assert!(
            painted.contains("Standalone Agent In"),
            "the explorer is gone at {width}x{height}:\n{painted}"
        );
    }

    let mut app = test_app(default_bindings());
    open_with_rows(&mut app, mixed_rows());
    let painted = rows_of(&buffer(&mut app, 80, 14));
    let caption: Vec<&String> = painted
        .iter()
        .filter(|row| row.contains("Uses the project"))
        .collect();
    assert_eq!(
        caption.len(),
        1,
        "no caption at 80x14:\n{}",
        painted.join("\n")
    );
    assert!(
        caption[0].contains('…'),
        "the caption stops mid-sentence at 80x14: {}",
        caption[0]
    );
}

/// The header's first line follows the caption's rules: on a narrow screen it
/// wraps, keeping the count of available projects, and where there is no row
/// to wrap into it ends in an ellipsis rather than stopping at the border.
#[test]
fn the_count_line_wraps_or_is_visibly_cut_on_narrow_screens() {
    for (width, height) in [(60, 20), (50, 20)] {
        let mut app = test_app(default_bindings());
        open_with_rows(&mut app, mixed_rows());
        let painted = rows_of(&buffer(&mut app, width, height)).join("\n");
        assert!(
            painted.contains("available)."),
            "the count line is cut at {width}x{height}:\n{painted}"
        );
        assert!(
            painted.contains("Uses the") && painted.contains("command."),
            "the caption is cut at {width}x{height}:\n{painted}"
        );
        assert!(
            painted.contains("taken") || painted.contains("free") || painted.contains("gone"),
            "no project row is left at {width}x{height}:\n{painted}"
        );
        assert!(
            painted.contains("Standalone Agent In"),
            "the explorer is gone at {width}x{height}:\n{painted}"
        );
    }

    let mut app = test_app(default_bindings());
    open_with_rows(&mut app, mixed_rows());
    let painted = rows_of(&buffer(&mut app, 50, 14));
    let count = painted
        .iter()
        .find(|row| row.contains("Pick a project"))
        .unwrap_or_else(|| panic!("no count line at 50x14:\n{}", painted.join("\n")));
    assert!(
        count.contains('…'),
        "the count line stops at the border at 50x14: {count}"
    );
}

/// On a short terminal the picker's own status wraps to a second row and the
/// footer grows to three; the picker ends above it rather than drawing its last border over
/// the key hints.
#[test]
fn the_picker_stays_off_the_footer_on_a_short_screen() {
    for (width, height) in [(100, 15), (80, 15), (60, 15)] {
        let mut app = test_app(default_bindings());
        open_with_rows(&mut app, mixed_rows());
        let painted = rows_of(&buffer(&mut app, width, height));
        let (above, footer) = painted.split_at(painted.len() - 3);
        assert!(
            !footer[2].trim().is_empty(),
            "the picker's own status no longer wraps at {width}x{height}:\n{}",
            painted.join("\n")
        );
        assert!(
            above.iter().any(|row| row.contains("<Tab> folders")),
            "the picker's last row is not above the footer at {width}x{height}:\n{}",
            painted.join("\n")
        );
        assert!(
            !footer.iter().any(|row| row.contains(['╰', '╯', '│'])),
            "the picker is drawn over the footer at {width}x{height}:\n{}",
            painted.join("\n")
        );
    }
}

/// The header states how many projects are available only once their folders
/// have been checked: a number given before that could drop a moment later.
#[test]
fn the_count_waits_for_the_folders_to_be_checked() {
    let mut app = test_app(default_bindings());
    app.execute_command("new-standalone-agent").expect("opens");
    let pending = screen(&mut app);
    assert!(
        pending.contains("Pick a project to browse to its folder."),
        "{pending}"
    );
    assert!(!pending.contains("available"), "{pending}");

    finish_ui_jobs(&mut app);
    let checked = screen(&mut app);
    assert!(checked.contains("(1 available)"), "{checked}");

    let mut app = test_app(default_bindings());
    super::ui_jobs::fail_next_quiet_spawn();
    app.execute_command("new-standalone-agent").expect("opens");
    finish_ui_jobs(&mut app);
    assert!(!screen(&mut app).contains("available"));
}

/// The explorer's footer says what its commit key does in this picker: it
/// runs an agent here. The Add Project browser keeps its own words.
#[test]
fn the_explorer_footer_names_what_the_key_does_in_each_browser() {
    let mut app = test_app(default_bindings());
    app.execute_command("new-standalone-agent").expect("opens");
    finish_ui_jobs(&mut app);
    press(&mut app, KeyCode::Tab);
    let painted = screen(&mut app);
    assert!(painted.contains("<o> run agent here"), "{painted}");
    assert!(!painted.contains("add current"), "{painted}");

    let mut app = test_app(default_bindings());
    app.execute_command("add-project").expect("opens");
    finish_ui_jobs(&mut app);
    assert!(screen(&mut app).contains("<o> add current"));
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
        project_chooser.entries[0].shown_folder = "~/work/clients/acme/services/free".to_string();
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
