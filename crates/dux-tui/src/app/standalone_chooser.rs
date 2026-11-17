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
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::test_support::{default_bindings, finish_ui_jobs, test_app};
use super::{App, PromptState};

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

fn screen(app: &mut App) -> String {
    let mut terminal = Terminal::new(TestBackend::new(120, 40)).expect("terminal");
    terminal.draw(|frame| app.render(frame)).expect("render");
    let buffer = terminal.backend().buffer().clone();
    let width = usize::from(buffer.area.width);
    buffer
        .content()
        .chunks(width)
        .map(|cells| cells.iter().map(ratatui::buffer::Cell::symbol).collect())
        .collect::<Vec<String>>()
        .join("\n")
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

/// A greyed-out row is never picked: the cursor steps over it, and a pick
/// forced onto it (a click lands anywhere) leaves the explorer where it was
/// and says why in core's words.
#[test]
fn a_greyed_out_project_cannot_be_picked() {
    let mut app = test_app(default_bindings());
    app.execute_command("new-standalone-agent")
        .expect("the picker opens");
    finish_ui_jobs(&mut app);
    let (start_dir, _) = explorer_and_focus(&app);
    app.show_standalone_project_shortcuts(&ProjectShortcutList {
        caption: CAPTION,
        rows: vec![
            row(
                "taken",
                "/srv/taken",
                ShortcutState::AlreadyHasStandaloneAgent,
            ),
            row("free", "/srv/free", ShortcutState::Available),
            row("gone", "/srv/gone", ShortcutState::FolderMissing),
        ],
    });

    let selected = |app: &App| match &app.prompt {
        PromptState::BrowseProjects {
            project_chooser, ..
        } => project_chooser.list.selected,
        other => panic!("expected the folder browser, got {other:?}"),
    };
    assert_eq!(
        selected(&app),
        1,
        "the cursor starts on the row that can be picked"
    );
    press(&mut app, KeyCode::Up);
    assert_eq!(selected(&app), 1, "and never moves onto a greyed-out one");

    let painted = screen(&mut app);
    assert!(
        painted.contains("already has a standalone agent"),
        "{painted}"
    );
    assert!(painted.contains("folder missing"), "{painted}");
    assert!(painted.contains("~/free"), "{painted}");
    assert!(painted.contains(CAPTION), "{painted}");

    if let PromptState::BrowseProjects {
        project_chooser, ..
    } = &mut app.prompt
    {
        project_chooser.list.selected = 0;
    }
    press(&mut app, KeyCode::Enter);

    let (current_dir, chooser_focused) = explorer_and_focus(&app);
    assert_eq!(current_dir, start_dir, "the explorer did not move");
    assert!(chooser_focused);
    assert!(
        app.status.text().contains("already has a standalone agent"),
        "{}",
        app.status.text()
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

    let PromptState::BrowseProjects {
        project_chooser, ..
    } = &app.prompt
    else {
        panic!("expected the folder browser");
    };
    assert!(!project_chooser.is_shown());
    assert!(!screen(&mut app).contains("Start In A Project's Folder"));
}
