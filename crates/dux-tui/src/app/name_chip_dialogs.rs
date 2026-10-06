//! Every name a dialog shows is the name chip, rendered rather than reasoned
//! about.
//!
//! Each test opens one dialog with distinctive names, renders the whole app
//! into a test backend, and asserts that every cell of every occurrence of each
//! name carries the dialog body's colors swapped (`overlay_bg` on `text_fg`) with no
//! bold, that the chip is padded by one chip-colored cell on each side, and that
//! no straight quote is left around it.

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::style::Modifier;

use super::test_support::{default_bindings, test_app};
use super::*;

const WIDTH: u16 = 120;
const HEIGHT: u16 = 40;

fn render(app: &mut App) -> Buffer {
    render_at(app, WIDTH, HEIGHT)
}

/// Give the fixture app `count` agents in one project and return that
/// project's id: the project confirmations count agents live at paint time,
/// so a fixture's number has to be real rather than written into the prompt.
fn project_with_agents(app: &mut App, count: usize) -> String {
    let template = app.engine.sessions[0].clone();
    let project_id = template
        .project_id()
        .expect("the fixture agent belongs to a project")
        .to_string();
    for n in 1..count {
        let mut extra = template.clone();
        extra.id = format!("{}-extra-{n}", template.id);
        app.engine.sessions.push(extra);
    }
    project_id
}

fn render_at(app: &mut App, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
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

/// Every place `name` appears on screen, as (x, y) of its first character.
/// Names in these tests are ASCII, so one character is one cell.
fn occurrences(buf: &Buffer, name: &str) -> Vec<(u16, u16)> {
    let chars: Vec<String> = name.chars().map(|c| c.to_string()).collect();
    let len = chars.len() as u16;
    let mut hits = Vec::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width.saturating_sub(len - 1) {
            if (0..len).all(|i| buf[(x + i, y)].symbol() == chars[usize::from(i)]) {
                hits.push((x, y));
            }
        }
    }
    hits
}

/// Assert `name` is on screen and every occurrence of it is a padded chip with
/// no quotes around it.
fn assert_chipped(app: &App, buf: &Buffer, name: &str) {
    let theme = &app.theme;
    let hits = occurrences(buf, name);
    let shown = screen(buf);
    assert!(!hits.is_empty(), "{name:?} is not on screen:\n{shown}");
    let len = name.chars().count() as u16;
    for (x, y) in hits {
        for cx in x..x + len {
            let cell = &buf[(cx, y)];
            assert_eq!(
                (cell.fg, cell.bg),
                (theme.overlay_bg, theme.text_fg),
                "{name:?} at ({x},{y}) is not in the chip colors at column {cx}:\n{shown}"
            );
            assert!(
                !cell.modifier.contains(Modifier::BOLD),
                "{name:?} at ({x},{y}) is still bold:\n{shown}"
            );
        }
        for pad in [x.checked_sub(1), Some(x + len)].into_iter().flatten() {
            if pad >= buf.area.width {
                continue;
            }
            let cell = &buf[(pad, y)];
            assert_eq!(
                (cell.symbol(), cell.bg),
                (" ", theme.text_fg),
                "{name:?} at ({x},{y}) has no chip padding at column {pad}:\n{shown}"
            );
        }
        for outside in [x.checked_sub(2), Some(x + len + 1)].into_iter().flatten() {
            if outside < buf.area.width {
                assert_ne!(
                    buf[(outside, y)].symbol(),
                    "\"",
                    "{name:?} at ({x},{y}) is still quoted:\n{shown}"
                );
            }
        }
    }
    assert!(
        !shown.contains(&format!("\"{name}\"")),
        "{name:?} is still quoted somewhere:\n{shown}"
    );
}

/// The words inside the dialog titled `title`: every row between its titled
/// top edge and its bottom edge, cut to the columns inside its frame and
/// joined with single spaces, so a wrapped sentence reads as one.
fn dialog_text(buf: &Buffer, title: &str) -> String {
    let edge = format!("\u{256d}{title}");
    let edge: Vec<String> = edge.chars().map(|c| c.to_string()).collect();
    let len = edge.len() as u16;
    let cell = |x: u16, y: u16| buf[(x, y)].symbol().to_string();
    let (top, left) = (0..buf.area.height)
        .find_map(|y| {
            (0..buf.area.width.saturating_sub(len))
                .find(|&x| (0..len).all(|i| cell(x + i, y) == edge[usize::from(i)]))
                .map(|x| (y, x))
        })
        .unwrap_or_else(|| panic!("no dialog titled {title:?}:\n{}", screen(buf)));
    let right = (left + 1..buf.area.width)
        .find(|&x| cell(x, top) == "\u{256e}")
        .expect("the dialog's top-right corner");
    let bottom = (top + 1..buf.area.height)
        .find(|&y| cell(left, y) == "\u{2570}")
        .expect("the dialog's bottom-left corner");
    (top + 1..bottom)
        .map(|y| (left + 1..right).map(|x| cell(x, y)).collect::<String>())
        .collect::<Vec<_>>()
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn open(app: &mut App, prompt: PromptState) -> Buffer {
    app.prompt = prompt;
    render(app)
}

#[test]
fn the_detach_dialog_chips_the_agent() {
    let mut app = test_app(default_bindings());
    let buf = open(
        &mut app,
        PromptState::ConfirmDetachAgent {
            attached: Vec::new(),
            session_id: "s1".to_string(),
            label: "feat-detach".to_string(),
            grace_seconds: 30,
            live_tabs: 2,
            focus: ConfirmFocus::Cancel,
        },
    );
    assert_chipped(&app, &buf, "feat-detach");
    assert!(
        screen(&buf).contains("tabs stop together."),
        "{}",
        screen(&buf)
    );
}

#[test]
fn the_recreate_dialog_chips_the_path_the_branches_and_the_provider() {
    let mut app = test_app(default_bindings());
    let buf = open(
        &mut app,
        PromptState::ConfirmRecreateWorkingCopy {
            session_id: "s1".to_string(),
            worktree_path: std::path::PathBuf::from("/srv/wt/repo/feat-rc"),
            branch_name: "feat-rc".to_string(),
            source_branch: "trunk-rc".to_string(),
            conversation_resumes: true,
            running_providers: vec!["gemini".to_string()],
            focus: ConfirmFocus::Cancel,
        },
    );
    assert_chipped(&app, &buf, "/srv/wt/repo/feat-rc");
    assert_chipped(&app, &buf, "origin/feat-rc");
    assert_chipped(&app, &buf, "trunk-rc");
    assert_chipped(&app, &buf, "Gemini");
    // The sentence ends where the body ends: nothing is clipped by the frame.
    assert!(screen(&buf).contains("recreated copy."), "{}", screen(&buf));
}

#[test]
fn the_checkout_default_branch_dialog_chips_the_project_and_its_base() {
    let mut app = test_app(default_bindings());
    let buf = open(
        &mut app,
        PromptState::ConfirmCheckoutDefaultBranch {
            project_id: "p1".to_string(),
            project_name: "proj-co".to_string(),
            stored_base: Some("base-co".to_string()),
            focus: ConfirmFocus::Cancel,
            return_to: None,
        },
    );
    assert_chipped(&app, &buf, "proj-co");
    assert_chipped(&app, &buf, "base-co");
}

/// The delete-project dialog prints the core body (the browser's words),
/// chips the project, names the cascade, and publishes a Cancel / Delete pair
/// whose Delete is the Danger kind.
#[test]
fn the_delete_project_dialog_chips_the_project_and_names_the_cascade() {
    let mut app = test_app(default_bindings());
    let project_id = project_with_agents(&mut app, 2);
    let buf = open(
        &mut app,
        PromptState::ConfirmDeleteProject {
            attached: Vec::new(),
            project_id,
            project_name: "proj-del".to_string(),
            agent_count: 2,
            focus: ConfirmFocus::Cancel,
            return_to: None,
        },
    );
    assert_chipped(&app, &buf, "proj-del");
    let flat = dialog_text(&buf, "Delete Project");
    for words in [
        "This deletes proj-del , its 2 agents,",
        "and their worktrees on disk from dux. This is irreversible. The source \
         checkout is kept.",
        "Cancel",
        "Delete",
    ] {
        assert!(flat.contains(words), "{words:?} missing:\n{}", screen(&buf));
    }
    let OverlayMouseLayout::ConfirmDeleteProject {
        cancel_button,
        confirm_button,
    } = app.overlay_layout.active
    else {
        panic!("the dialog must publish its buttons for the mouse");
    };
    // Delete is destructive: focused, it does not paint like the focused Cancel.
    let focused_cancel = buf[(cancel_button.x, cancel_button.y)].style();
    let PromptState::ConfirmDeleteProject { focus, .. } = &mut app.prompt else {
        unreachable!("the prompt is still open");
    };
    *focus = ConfirmFocus::Confirm;
    let buf = render(&mut app);
    assert_ne!(
        buf[(confirm_button.x, confirm_button.y)].style(),
        focused_cancel,
        "Delete must be the Danger kind"
    );
}

#[test]
fn the_remove_project_dialog_chips_the_project_and_keeps_the_worktrees() {
    let mut app = test_app(default_bindings());
    let project_id = project_with_agents(&mut app, 1);
    let buf = open(
        &mut app,
        PromptState::ConfirmRemoveProject {
            attached: Vec::new(),
            project_id,
            project_name: "proj-rm".to_string(),
            agent_count: 1,
            orphaned: true,
            focus: ConfirmFocus::Cancel,
            return_to: None,
        },
    );
    assert_chipped(&app, &buf, "proj-rm");
    let flat = dialog_text(&buf, "Remove Project");
    for words in [
        "This removes proj-rm and deletes its 1 agent from dux. Worktrees on disk are kept.",
        "Remove",
    ] {
        assert!(flat.contains(words), "{words:?} missing:\n{}", screen(&buf));
    }
    assert!(matches!(
        app.overlay_layout.active,
        OverlayMouseLayout::ConfirmRemoveProject { .. }
    ));
}

/// A name with spaces in it is one chip: at every width the dialog can be
/// drawn at, the whole name sits on one row between its two pads, whatever
/// the wrap does to the words around it.
#[test]
fn a_multi_word_name_is_never_split_across_rows() {
    for width in 24..=90u16 {
        let mut app = test_app(default_bindings());
        app.prompt = PromptState::ConfirmCheckoutDefaultBranch {
            project_id: "p1".to_string(),
            project_name: "My Cool Project".to_string(),
            stored_base: Some("base-co".to_string()),
            focus: ConfirmFocus::Cancel,
            return_to: None,
        };
        let buf = render_at(&mut app, width, HEIGHT);
        assert_chipped(&app, &buf, "My Cool Project");

        // A fixed-size dialog, whose body the renderer wraps at paint time. Its
        // body is a share of the screen, so below this width the chip is wider
        // than a whole row and has to be cut, which no wrapper can avoid.
        if width < 44 {
            continue;
        }
        app.prompt = PromptState::ConfirmDeleteTerminal {
            attached: Vec::new(),
            terminal_id: "t1".to_string(),
            terminal_label: "My Cool Terminal".to_string(),
            foreground_cmd: Some("vim".to_string()),
            focus: ConfirmFocus::Cancel,
        };
        let buf = render_at(&mut app, width, HEIGHT);
        assert_chipped(&app, &buf, "My Cool Terminal");
    }
}

#[test]
fn the_delete_terminal_dialog_chips_the_terminal() {
    let mut app = test_app(default_bindings());
    let buf = open(
        &mut app,
        PromptState::ConfirmDeleteTerminal {
            attached: Vec::new(),
            terminal_id: "t1".to_string(),
            terminal_label: "term-dt".to_string(),
            foreground_cmd: Some("vim".to_string()),
            focus: ConfirmFocus::Cancel,
        },
    );
    assert_chipped(&app, &buf, "term-dt");
}

#[test]
fn the_close_and_stop_tab_dialogs_chip_the_provider_the_agent_and_the_successor() {
    let mut app = test_app(default_bindings());
    let session_id = app.engine.sessions[0].id.clone();
    let agent = app.session_label(&app.engine.sessions[0]);
    let close = PromptState::ConfirmCloseTab {
        attached: Vec::new(),
        session_id: session_id.clone(),
        tab_id: "no-such-tab".to_string(),
        provider_label: "Prov-ct".to_string(),
        promoted_label: Some("Next-ct".to_string()),
        focus: ConfirmFocus::Cancel,
    };
    let stop = PromptState::ConfirmStopTab {
        session_id,
        tab_id: "no-such-tab".to_string(),
        provider_label: "Prov-st".to_string(),
        last_running: false,
        focus: ConfirmFocus::Cancel,
        attached: Vec::new(),
    };
    for (prompt, provider) in [(close, "Prov-ct"), (stop, "Prov-st")] {
        let buf = open(&mut app, prompt);
        assert_chipped(&app, &buf, provider);
        if provider == "Prov-ct" {
            assert_chipped(&app, &buf, "Next-ct");
        }
        // The agent's name also labels its sidebar row behind the overlay, so
        // only the occurrence inside the dialog is asked about.
        let dialog_row = occurrences(&buf, provider)[0].1;
        let in_dialog: Vec<_> = occurrences(&buf, &agent)
            .into_iter()
            .filter(|(_, y)| *y == dialog_row)
            .collect();
        assert!(!in_dialog.is_empty(), "{}", screen(&buf));
        for (x, y) in in_dialog {
            assert_eq!(
                (buf[(x, y)].fg, buf[(x, y)].bg),
                (app.theme.overlay_bg, app.theme.text_fg),
                "{}",
                screen(&buf)
            );
        }
    }
}

fn delete_agent_prompt(target: DeleteAgentTarget, delete_worktree: bool) -> PromptState {
    PromptState::ConfirmDeleteAgent {
        attached: Vec::new(),
        session_id: "s1".to_string(),
        agent_label: "launch-at-login".to_string(),
        target,
        focus: DeleteAgentFocus::Cancel,
        delete_worktree,
        delete_branch: true,
        unpushed_commits: Some(dux_core::git::UnpushedCommits {
            count: 2,
            has_remote_refs: true,
        }),
    }
}

/// Deleting an agent a browser is watching leaves the dialog open: it names
/// who as chips (the device, the tab and the agent), its confirm becomes
/// Delete anyway with Cancel focused, and the override deletes the agent.
#[test]
fn a_refused_agent_delete_names_who_is_attached_and_offers_to_delete_anyway() {
    let mut app = test_app(default_bindings());
    let agent = app.engine.sessions[0].id.clone();
    let tab = app.engine.sessions[0].slot_tab_id().to_string();
    app.engine.attachments.register(
        "browser-tab",
        dux_core::attachments::ConnectionFacts {
            surface: dux_core::attachments::Surface::Browser,
            device: Some(
                "Mozilla/5.0 (X11; Linux x86_64; rv:126.0) Gecko/20100101 Firefox/126.0"
                    .to_string(),
            ),
            address: Some("10.0.0.7".parse().unwrap()),
            verified: false,
            events: true,
        },
        None,
    );
    app.engine
        .attachments
        .attach(
            "browser-tab",
            dux_core::attachments::Target {
                kind: dux_core::attachments::TargetKind::Tab,
                id: tab,
                agent: Some(agent.clone()),
            },
            None,
            None,
        )
        .unwrap();
    app.confirm_delete_selected_session()
        .expect("open the delete dialog");

    app.resolve_confirm_delete_agent(true);

    let PromptState::ConfirmDeleteAgent { focus, .. } = &app.prompt else {
        panic!("the delete dialog stays open, got {:?}", app.prompt);
    };
    assert_eq!(*focus, DeleteAgentFocus::Cancel);
    let buf = render(&mut app);
    assert_chipped(&app, &buf, "Firefox on Linux");
    let text = dialog_text(&buf, "Delete Agent");
    assert!(
        text.contains("at 10.0.0.7 (unverified), watching tab"),
        "{text}"
    );
    assert!(text.contains("Delete anyway"), "{text}");
    assert!(app.engine.sessions.iter().any(|s| s.id == agent));

    // Somebody the dialog did not show attaches: the override is refused
    // again, naming both, with focus back on Cancel.
    app.engine.attachments.register(
        "second-browser",
        dux_core::attachments::ConnectionFacts {
            surface: dux_core::attachments::Surface::Browser,
            device: Some("Safari".to_string()),
            address: Some("10.0.0.8".parse().unwrap()),
            verified: false,
            events: true,
        },
        None,
    );
    app.engine
        .attachments
        .attach(
            "second-browser",
            dux_core::attachments::Target {
                kind: dux_core::attachments::TargetKind::Tab,
                id: app.engine.sessions[0].slot_tab_id().to_string(),
                agent: Some(agent.clone()),
            },
            None,
            None,
        )
        .unwrap();
    if let PromptState::ConfirmDeleteAgent { focus, .. } = &mut app.prompt {
        *focus = DeleteAgentFocus::Delete;
    }
    app.resolve_confirm_delete_agent(true);
    let PromptState::ConfirmDeleteAgent {
        focus, attached, ..
    } = &app.prompt
    else {
        panic!("the override is refused again, got {:?}", app.prompt);
    };
    assert_eq!(attached.len(), 2);
    assert_eq!(*focus, DeleteAgentFocus::Cancel);
    assert!(app.engine.sessions.iter().any(|s| s.id == agent));

    app.resolve_confirm_delete_agent(true);

    assert!(matches!(app.prompt, PromptState::None));
    assert!(
        !app.engine.sessions.iter().any(|s| s.id == agent),
        "the override deletes the agent"
    );
}

/// A double click on Delete never goes ahead over who the refusal named: the
/// second click arrives before the override has been drawn, so it lands on
/// nothing. The override takes a fresh press on the button once it is on
/// screen.
#[test]
fn a_double_click_on_a_refused_delete_never_presses_the_override() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut app = test_app(default_bindings());
    let agent = app.engine.sessions[0].id.clone();
    let tab = app.engine.sessions[0].slot_tab_id().to_string();
    crate::app::test_support::watch_from_a_browser(&app, &tab, &agent);
    app.confirm_delete_selected_session()
        .expect("open the delete dialog");
    let click = |app: &mut App, rect: ratatui::layout::Rect| {
        let (column, row) = (rect.x + rect.width / 2, rect.y + rect.height / 2);
        for kind in [
            MouseEventKind::Down(MouseButton::Left),
            MouseEventKind::Up(MouseButton::Left),
        ] {
            app.handle_mouse(MouseEvent {
                kind,
                column,
                row,
                modifiers: crossterm::event::KeyModifiers::NONE,
            });
        }
    };
    let delete_button = |app: &App| match app.overlay_layout.active {
        OverlayMouseLayout::ConfirmDeleteAgent { delete_button, .. } => delete_button,
        _ => panic!("the delete dialog publishes its buttons"),
    };

    render(&mut app);
    let first = delete_button(&app);
    // Both halves of the double click in one input batch, no frame between.
    click(&mut app, first);
    click(&mut app, first);

    assert!(
        app.engine.sessions.iter().any(|s| s.id == agent),
        "the second click must not go ahead"
    );
    assert!(!app.prompt.attached().is_empty(), "{:?}", app.prompt);

    render(&mut app);
    let drawn = delete_button(&app);
    click(&mut app, drawn);
    assert!(
        !app.engine.sessions.iter().any(|s| s.id == agent),
        "a fresh press on the drawn override goes ahead"
    );
}

/// Quitting with a browser attached names it as a chip, and the confirm says
/// it quits anyway.
#[test]
fn the_quit_dialog_names_who_is_attached_and_offers_to_quit_anyway() {
    let mut app = test_app(default_bindings());
    let agent = app.engine.sessions[0].id.clone();
    let tab = app.engine.sessions[0].slot_tab_id().to_string();
    crate::app::test_support::watch_from_a_browser(&app, &tab, &agent);
    assert!(!app.begin_quit(), "asks first");

    let buf = render(&mut app);
    assert_chipped(&app, &buf, "Firefox");
    let text = dialog_text(&buf, "Quit dux");
    assert!(
        text.contains("Someone else is using this right now. Going ahead cuts them off:"),
        "{text}"
    );
    assert!(text.contains("Quit anyway"), "{text}");
}

#[test]
fn the_delete_agent_dialog_chips_the_agent_and_every_branch_it_names() {
    let mut app = test_app(default_bindings());
    let buf = open(
        &mut app,
        delete_agent_prompt(
            DeleteAgentTarget::Managed {
                branch_name: "now-br".to_string(),
                initial_branch: "born-br".to_string(),
                branch_provenance: dux_core::model::BranchProvenance::AttachedExisting,
                worktree_shared: false,
            },
            true,
        ),
    );
    assert_chipped(&app, &buf, "launch-at-login");
    // Named in the branch checkbox AND in the drift warning above it.
    assert_chipped(&app, &buf, "now-br");
    assert_chipped(&app, &buf, "born-br");
    assert!(occurrences(&buf, "born-br").len() >= 2, "{}", screen(&buf));
    assert!(screen(&buf).contains("Delete"), "{}", screen(&buf));
}

#[test]
fn the_standalone_delete_dialog_chips_the_agent_and_its_folder() {
    let mut app = test_app(default_bindings());
    let buf = open(
        &mut app,
        delete_agent_prompt(
            DeleteAgentTarget::Folder {
                folder_label: "~/notes-sa".to_string(),
            },
            false,
        ),
    );
    assert_chipped(&app, &buf, "launch-at-login");
    assert_chipped(&app, &buf, "~/notes-sa");
}

#[test]
fn the_discard_dialog_chips_the_file() {
    let mut app = test_app(default_bindings());
    let buf = open(
        &mut app,
        PromptState::ConfirmDiscardFile {
            file_path: "src/discard-me.rs".to_string(),
            kind: dux_core::model::ChangedFileKind::File,
            focus: ConfirmFocus::Cancel,
        },
    );
    assert_chipped(&app, &buf, "src/discard-me.rs");
}

#[test]
fn the_initial_commit_dialog_chips_the_path() {
    let mut app = test_app(default_bindings());
    let buf = open(
        &mut app,
        PromptState::ConfirmCreateInitialCommit {
            path: "/srv/empty-repo".to_string(),
            name: "empty-repo".to_string(),
            focus: ConfirmFocus::Cancel,
        },
    );
    assert_chipped(&app, &buf, "/srv/empty-repo");
}

#[test]
fn the_init_repo_dialog_chips_the_path_and_every_seeded_candidate() {
    let mut app = test_app(default_bindings());
    let buf = open(
        &mut app,
        PromptState::ConfirmInitRepo {
            path: "/srv/plain-dir".to_string(),
            name: "plain-dir".to_string(),
            candidates: vec!["node_modules/".to_string(), ".env".to_string()],
            focus: ConfirmFocus::Cancel,
            return_prompt: Box::new(PromptState::None),
        },
    );
    assert_chipped(&app, &buf, "/srv/plain-dir");
    assert_chipped(&app, &buf, "node_modules/");
    assert_chipped(&app, &buf, ".env");
}

#[test]
fn the_non_default_branch_dialog_chips_both_branches_the_note_and_the_checkbox() {
    let mut app = test_app(default_bindings());
    let project_path = app.engine.projects[0].path.clone();
    let buf = open(
        &mut app,
        PromptState::ConfirmNonDefaultBranch {
            add: PendingProjectAdd {
                path: project_path,
                name: "demo".to_string(),
            },
            current_branch: "topic-nd".to_string(),
            kind: dux_core::worker::BranchWarningKind::Known {
                default_branch: "main-nd".to_string(),
            },
            focus: ConfirmNonDefaultBranchFocus::Cancel,
            checkout_default: false,
        },
    );
    // Once in the warning, once in the worktree note.
    assert_chipped(&app, &buf, "topic-nd");
    assert!(occurrences(&buf, "topic-nd").len() >= 2, "{}", screen(&buf));
    // Once in the warning, once in the checkbox label.
    assert_chipped(&app, &buf, "main-nd");
    assert!(occurrences(&buf, "main-nd").len() >= 2, "{}", screen(&buf));
}

/// The warning's sentences are dux-core's, shared with the web word for word;
/// the terminal UI breaks their rows exactly where it always has.
#[test]
fn the_non_default_branch_dialog_keeps_its_line_breaks() {
    let rows_of = |kind: dux_core::worker::BranchWarningKind| {
        let mut app = test_app(default_bindings());
        let project_path = app.engine.projects[0].path.clone();
        let buf = open(
            &mut app,
            PromptState::ConfirmNonDefaultBranch {
                add: PendingProjectAdd {
                    path: project_path,
                    name: "demo".to_string(),
                },
                current_branch: "topic".to_string(),
                kind,
                focus: ConfirmNonDefaultBranchFocus::Cancel,
                checkout_default: false,
            },
        );
        screen(&buf).lines().map(str::to_string).collect::<Vec<_>>()
    };
    // A body row as the dialog paints it: the text right after the frame's
    // left edge, then nothing but padding up to its right edge.
    let is_row = |row: &str, text: &str| {
        row.split('│')
            .any(|cell| cell.trim_end() == text && (text.is_empty() || cell.starts_with(text)))
    };
    let contains_in_order = |rows: &[String], want: &[&str]| {
        let start = rows
            .iter()
            .position(|row| is_row(row, want[0]))
            .unwrap_or_else(|| panic!("no row {:?} in:\n{}", want[0], rows.join("\n")));
        for (offset, text) in want.iter().enumerate() {
            assert!(
                is_row(&rows[start + offset], text),
                "row {} should read {text:?}:\n{}",
                start + offset,
                rows.join("\n")
            );
        }
    };

    let known = rows_of(dux_core::worker::BranchWarningKind::Known {
        default_branch: "main".to_string(),
    });
    contains_in_order(
        &known,
        &[
            " This repository is on branch  topic , but the",
            " remote default branch is  main .",
            "",
            " New worktrees will branch from  topic .",
        ],
    );

    let heuristic = rows_of(dux_core::worker::BranchWarningKind::Heuristic);
    contains_in_order(
        &heuristic,
        &[
            " This repository is on branch  topic ,",
            " which doesn't appear to be the main branch.",
            "",
            " New worktrees will branch from  topic .",
            "",
            " Dux can't confidently identify this repo's default",
            " branch, so it won't change branches for you.",
        ],
    );
}

/// The dialog sizes itself to its body, so the height must be the rows the
/// wrapper actually produces: an estimate that counts characters instead of
/// whole words and whole chips comes up short and clips the last sentence.
#[test]
fn the_non_default_branch_dialog_is_as_tall_as_its_wrapped_body() {
    let gap_to_buttons = |buf: &Buffer, width: u16| {
        let shown = screen(buf);
        let rows: Vec<&str> = shown.lines().collect();
        let last = rows
            .iter()
            .position(|row| row.contains("you."))
            .unwrap_or_else(|| panic!("at width {width} the last sentence is clipped:\n{shown}"));
        let cancel = rows
            .iter()
            .position(|row| row.contains("Cancel"))
            .expect("the Cancel button");
        cancel.checked_sub(last)
    };
    let prompt = |app: &App| PromptState::ConfirmNonDefaultBranch {
        add: PendingProjectAdd {
            path: app.engine.projects[0].path.clone(),
            name: "demo".to_string(),
        },
        current_branch: "My Topic Branch".to_string(),
        kind: dux_core::worker::BranchWarningKind::Heuristic,
        focus: ConfirmNonDefaultBranchFocus::Cancel,
        checkout_default: false,
    };
    // Wide enough that nothing wraps: the gap every narrower drawing must keep.
    let mut app = test_app(default_bindings());
    app.prompt = prompt(&app);
    let unwrapped = gap_to_buttons(&render_at(&mut app, 160, HEIGHT), 160);
    for width in 30..=90u16 {
        let mut app = test_app(default_bindings());
        app.prompt = prompt(&app);
        let buf = render_at(&mut app, width, HEIGHT);
        assert_eq!(
            occurrences(&buf, "My Topic Branch").len(),
            2,
            "at width {width} the branch must be whole in the warning and the note:\n{}",
            screen(&buf)
        );
        // The body's last row sits exactly as far above the buttons as it
        // does unwrapped: an estimate that is too short clips the sentence,
        // one that is too tall leaves a blank band.
        assert_eq!(
            gap_to_buttons(&buf, width),
            unwrapped,
            "at width {width} the body is not exactly as tall as its wrapped rows:\n{}",
            screen(&buf)
        );
    }
}

#[test]
fn the_use_existing_branch_dialog_chips_the_branch() {
    let mut app = test_app(default_bindings());
    let request = CreateAgentRequest::NewProject {
        project: app.engine.projects[0].clone(),
        custom_name: Some("exists-ub".to_string()),
        use_existing_branch: false,
        pull_before_create: false,
        copy_uncommitted_changes: false,
    };
    let buf = open(
        &mut app,
        PromptState::ConfirmUseExistingBranch {
            request,
            branch_name: "exists-ub".to_string(),
            location: crate::git::BranchLocation::Local,
            focus: ConfirmFocus::Cancel,
        },
    );
    assert_chipped(&app, &buf, "exists-ub");
}

/// [`assert_chipped`] for a name that also appears unchipped elsewhere on
/// screen (a picker row, a path it is part of): only its occurrences on rows
/// that also carry `row_marker` are asked about.
fn assert_chipped_on_row(app: &App, buf: &Buffer, row_marker: &str, name: &str) {
    let theme = &app.theme;
    let shown = screen(buf);
    let marked_rows: Vec<u16> = shown
        .lines()
        .enumerate()
        .filter(|(_, row)| row.contains(row_marker))
        .map(|(y, _)| y as u16)
        .collect();
    assert!(
        !marked_rows.is_empty(),
        "no row carries {row_marker:?}:\n{shown}"
    );
    let hits: Vec<(u16, u16)> = occurrences(buf, name)
        .into_iter()
        .filter(|(_, y)| marked_rows.contains(y))
        .collect();
    assert!(
        !hits.is_empty(),
        "{name:?} is not on the {row_marker:?} row:\n{shown}"
    );
    let len = name.chars().count() as u16;
    for (x, y) in hits {
        for cx in x..x + len {
            let cell = &buf[(cx, y)];
            assert_eq!(
                (cell.fg, cell.bg),
                (theme.overlay_bg, theme.text_fg),
                "{name:?} at ({x},{y}) is not in the chip colors:\n{shown}"
            );
        }
        for pad in [x.checked_sub(1), Some(x + len)].into_iter().flatten() {
            assert_eq!(
                (buf[(pad, y)].symbol(), buf[(pad, y)].bg),
                (" ", theme.text_fg),
                "{name:?} at ({x},{y}) has no chip padding:\n{shown}"
            );
        }
    }
    assert!(
        !shown.contains(&format!("\"{name}\"")),
        "{name:?} is still quoted:\n{shown}"
    );
}

#[test]
fn the_delete_worktree_dialog_chips_the_worktree_its_path_and_its_branch() {
    let mut app = test_app(default_bindings());
    let mut project = app.engine.projects[0].clone();
    project.name = "proj-dw".to_string();
    let buf = open(
        &mut app,
        PromptState::ConfirmDeleteWorktree(Box::new(ConfirmDeleteWorktreePrompt {
            previous: ManageWorktreesPrompt {
                return_to: None,
                project: project.clone(),
                entries: Vec::new(),
                loading: false,
                selected: None,
                error: None,
            },
            project,
            path: std::path::PathBuf::from("/srv/wt/free-dw"),
            label: "br-dw".to_string(),
            branch: Some("br-dw".to_string()),
            dirty: true,
            delete_branch: true,
            focus: DeleteWorktreeFocus::Cancel,
        })),
    );
    // The question, the branch sentence and the checkbox all name it.
    assert_chipped(&app, &buf, "br-dw");
    assert!(occurrences(&buf, "br-dw").len() >= 3, "{}", screen(&buf));
    assert_chipped(&app, &buf, "/srv/wt/free-dw");
}

#[test]
fn the_agent_provider_picker_chips_the_agent_and_its_path() {
    let mut app = test_app(default_bindings());
    let mut prompt = super::test_support::agent_provider_prompt();
    prompt.session_label = "agent-cap".to_string();
    prompt.worktree_path = "/srv/wt-cap".to_string();
    let buf = open(&mut app, PromptState::ChangeAgentProvider(prompt));
    assert_chipped(&app, &buf, "agent-cap");
    assert_chipped(&app, &buf, "/srv/wt-cap");
}

#[test]
fn the_default_provider_picker_chips_the_current_default() {
    let mut app = test_app(default_bindings());
    let mut prompt = super::test_support::default_provider_prompt();
    prompt.current = ProviderKind::new("prov-gd");
    let buf = open(&mut app, PromptState::ChangeDefaultProvider(prompt));
    assert_chipped(&app, &buf, "prov-gd");
}

#[test]
fn the_project_provider_picker_chips_the_project_and_both_providers() {
    let mut app = test_app(default_bindings());
    let mut prompt = super::test_support::project_default_provider_prompt(
        "p1".to_string(),
        "proj-pp".to_string(),
    );
    prompt.current = ProviderKind::new("prov-cur");
    prompt.global_default = ProviderKind::new("prov-glob");
    let buf = open(&mut app, PromptState::ChangeProjectDefaultProvider(prompt));
    assert_chipped(&app, &buf, "proj-pp");
    assert_chipped(&app, &buf, "prov-cur");
    assert_chipped(&app, &buf, "prov-glob");
}

#[test]
fn the_theme_picker_chips_the_current_theme() {
    let mut app = test_app(default_bindings());
    let options = crate::theme::discover_available(&app.engine.paths);
    let buf = open(
        &mut app,
        PromptState::ChangeTheme(ChangeThemePrompt {
            options,
            selected: 0,
            current: "theme-cur".to_string(),
        }),
    );
    assert_chipped(&app, &buf, "theme-cur");
}

#[test]
fn the_editor_picker_chips_the_agent_and_its_path() {
    let mut app = test_app(default_bindings());
    let buf = open(
        &mut app,
        PromptState::PickEditor {
            session_label: "agent-pe".to_string(),
            worktree_path: "/srv/wt-pe".to_string(),
            editors: Vec::new(),
            selected: 0,
        },
    );
    assert_chipped(&app, &buf, "agent-pe");
    assert_chipped(&app, &buf, "/srv/wt-pe");
}

fn named_project(app: &App, name: &str, path: &str) -> Project {
    let mut project = app.engine.projects[0].clone();
    project.name = name.to_string();
    project.path = path.to_string();
    project
}

#[test]
fn the_worktree_manager_chips_the_project_and_its_repository() {
    let mut app = test_app(default_bindings());
    let project = named_project(&app, "proj-mw", "/srv/repo-mw");
    let buf = open(
        &mut app,
        PromptState::ManageWorktrees(ManageWorktreesPrompt {
            return_to: None,
            project,
            entries: Vec::new(),
            loading: false,
            selected: None,
            error: None,
        }),
    );
    assert_chipped(&app, &buf, "proj-mw");
    assert_chipped(&app, &buf, "/srv/repo-mw");
}

#[test]
fn the_worktree_picker_chips_the_project_and_its_repository() {
    let mut app = test_app(default_bindings());
    let project = named_project(&app, "proj-pw", "/srv/repo-pw");
    let buf = open(
        &mut app,
        PromptState::PickProjectWorktree(PickProjectWorktreePrompt {
            project,
            entries: Vec::new(),
            loading: false,
            selected: None,
            error: None,
        }),
    );
    assert_chipped(&app, &buf, "proj-pw");
    assert_chipped(&app, &buf, "/srv/repo-pw");
}

#[test]
fn the_pull_request_dialog_chips_the_project() {
    let mut app = test_app(default_bindings());
    let project = named_project(&app, "proj-pr", "/srv/repo-pr");
    let buf = open(
        &mut app,
        PromptState::PullRequestInput {
            project: Some(project),
            input: TextInput::new(),
            focus: PullRequestInputFocus::Input,
        },
    );
    assert_chipped(&app, &buf, "proj-pr");
}

#[test]
fn the_attach_pull_request_dialog_chips_the_pull_request_it_replaces() {
    let mut app = test_app(default_bindings());
    let buf = open(
        &mut app,
        PromptState::AttachPullRequestInput {
            session_id: "s1".to_string(),
            current_pr: Some("#42 (open) Fix the frobnicator".to_string()),
            input: TextInput::new(),
        },
    );
    assert_chipped(&app, &buf, "#42 (open) Fix the frobnicator");
}

#[test]
fn the_standalone_name_dialog_chips_the_default_name_and_the_folder() {
    let mut app = test_app(default_bindings());
    let buf = open(
        &mut app,
        PromptState::NameStandaloneAgent {
            folder: "/srv/notes-ns".to_string(),
            input: TextInput::new(),
        },
    );
    assert_chipped_on_row(&app, &buf, "defaults to", "notes-ns");
    assert_chipped(&app, &buf, "/srv/notes-ns");
}

#[test]
fn the_configure_dialog_chips_the_project() {
    let mut app = test_app(default_bindings());
    let buf = open(
        &mut app,
        PromptState::ConfigureStartupCommand {
            return_to: None,
            project_id: "p1".to_string(),
            project_name: "proj-cfg".to_string(),
            input: TextInput::with_text("npm install".to_string()).with_multiline(6),
            focus: ConfigureFieldFocus::default(),
        },
    );
    assert_chipped(&app, &buf, "proj-cfg");
}

#[test]
fn the_browse_dialog_chips_the_folder_in_its_title() {
    let mut app = test_app(default_bindings());
    let buf = open(
        &mut app,
        PromptState::BrowseProjects {
            purpose: BrowsePurpose::AddProject,
            current_dir: std::path::PathBuf::from("/srv/browse-dir"),
            entries: Vec::new(),
            loading: false,
            selected: 0,
            filter: TextInput::new(),
            searching: false,
            editing_path: false,
            path_input: TextInput::new(),
            tab_completions: Vec::new(),
            tab_index: 0,
        },
    );
    assert_chipped_on_row(&app, &buf, "Add Project", "/srv/browse-dir");
}

#[test]
fn the_new_agent_dialog_chips_the_worktree_it_starts_in() {
    let mut app = test_app(default_bindings());
    let project = app.engine.projects[0].clone();
    let buf = open(
        &mut app,
        PromptState::NameNewAgent {
            request: CreateAgentRequest::ExistingManagedWorktree {
                project,
                worktree_path: std::path::PathBuf::from("/srv/wt-na"),
                branch_name: "existing-na".to_string(),
                custom_name: None,
            },
            input: TextInput::new(),
            randomize_name: false,
            randomized_name: None,
            copy_changes: false,
            focus: NameNewAgentFocus::Input,
        },
    );
    assert_chipped(&app, &buf, "/srv/wt-na");
}

#[test]
fn the_clone_dialog_chips_the_project_it_adds() {
    let mut app = test_app(default_bindings());
    let buf = open(
        &mut app,
        PromptState::CloneProject {
            address: TextInput::with_text("https://example.com/acme/widget-cl".to_string()),
            destination: TextInput::with_text("/srv/code/widget-cl".to_string()),
            destination_edited: false,
            start_folder: std::path::PathBuf::from("/srv/code"),
            agent_name: TextInput::new(),
            randomize_name: false,
            randomized_name: None,
            focus: CloneProjectFocus::Address,
        },
    );
    assert_chipped_on_row(&app, &buf, "Adds the project", "widget-cl");
}

#[test]
fn the_macro_dialogs_chip_the_macro() {
    let mut app = test_app(default_bindings());
    let buf = open(
        &mut app,
        PromptState::EditMacros {
            entries: Vec::new(),
            selected: 0,
            editing: None,
            pending_delete: Some(PendingMacroDelete {
                name: "macro-dm".to_string(),
                focus: ConfirmFocus::Cancel,
            }),
        },
    );
    assert_chipped(&app, &buf, "macro-dm");

    let buf = open(
        &mut app,
        PromptState::EditMacros {
            entries: Vec::new(),
            selected: 0,
            editing: Some(MacroEditState {
                id: Some("macro-ed".to_string()),
                name_input: TextInput::with_text("renamed".to_string()),
                text_input: TextInput::with_text("hello".to_string()).with_multiline(8),
                surface: crate::config::MacroSurface::Both,
                focus: MacroEditFocus::Name,
            }),
            pending_delete: None,
        },
    );
    assert_chipped(&app, &buf, "macro-ed");
}

/// The production half of a source file: everything before its test module.
fn production_source(source: &str) -> &str {
    source
        .find("#[cfg(test)]\nmod tests")
        .map_or(source, |end| &source[..end])
}

/// `source` with every comment and every string or char literal's contents
/// blanked to spaces (newlines kept), so a brace or a word inside one can
/// neither look like code nor unbalance the nesting count. Same length in
/// chars as the input, so a position in one is the same position in the other.
fn code_only(source: &str) -> Vec<char> {
    let chars: Vec<char> = source.chars().collect();
    let mut out = chars.clone();
    let blank = |out: &mut Vec<char>, i: usize| {
        if out[i] != '\n' {
            out[i] = ' ';
        }
    };
    let mut i = 0;
    while i < chars.len() {
        let next = chars.get(i + 1).copied();
        match chars[i] {
            '/' if next == Some('/') => {
                while i < chars.len() && chars[i] != '\n' {
                    blank(&mut out, i);
                    i += 1;
                }
            }
            '/' if next == Some('*') => {
                while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    blank(&mut out, i);
                    i += 1;
                }
                for _ in 0..2 {
                    if i < chars.len() {
                        blank(&mut out, i);
                        i += 1;
                    }
                }
            }
            'r' if matches!(next, Some('"') | Some('#'))
                && (i == 0 || !(chars[i - 1].is_alphanumeric() || chars[i - 1] == '_')) =>
            {
                let mut j = i + 1;
                let mut hashes = 0;
                while chars.get(j) == Some(&'#') {
                    hashes += 1;
                    j += 1;
                }
                if chars.get(j) != Some(&'"') {
                    i += 1;
                    continue;
                }
                j += 1;
                while j < chars.len() {
                    if chars[j] == '"' && (1..=hashes).all(|k| chars.get(j + k) == Some(&'#')) {
                        break;
                    }
                    blank(&mut out, j);
                    j += 1;
                }
                i = j + 1 + hashes;
            }
            '"' => {
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' {
                        blank(&mut out, i);
                        i += 1;
                    }
                    if i < chars.len() {
                        blank(&mut out, i);
                        i += 1;
                    }
                }
                i += 1;
            }
            // A char literal ('x', '\n', '{'); a lifetime ('a) has no closing
            // quote two or more places on and is left alone.
            '\'' if next == Some('\\') || chars.get(i + 2) == Some(&'\'') => {
                i += 1;
                while i < chars.len() && chars[i] != '\'' {
                    if chars[i] == '\\' {
                        blank(&mut out, i);
                        i += 1;
                    }
                    if i < chars.len() {
                        blank(&mut out, i);
                        i += 1;
                    }
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    out
}

/// The 1-based line of every ratatui `Wrap` struct literal in `source` whose
/// statement carries no `chip-free:` reason: not in the statement's own text
/// (comments included), and not on the comment lines directly above it. The
/// literal is found as a token, wherever the formatter put its line breaks and
/// however its path is qualified, so a call split across lines, a
/// `ratatui::widgets::` prefix or a literal bound to a variable first are all
/// seen.
fn unreasoned_ratatui_wraps(source: &str) -> Vec<usize> {
    let original: Vec<char> = source.chars().collect();
    let code = code_only(source);
    let ident = |c: char| c.is_alphanumeric() || c == '_';
    let line_of = |pos: usize| original[..pos].iter().filter(|c| **c == '\n').count() + 1;
    let word: Vec<char> = concat!("Wr", "ap").chars().collect();
    let mut found = Vec::new();
    let mut p = 0;
    while p + word.len() <= code.len() {
        let is_token = code[p..p + word.len()] == word[..]
            && (p == 0 || !ident(code[p - 1]))
            && code.get(p + word.len()).is_none_or(|c| !ident(*c));
        let literal = is_token && {
            let mut q = p + word.len();
            while code.get(q).is_some_and(|c| c.is_whitespace()) {
                q += 1;
            }
            code.get(q) == Some(&'{')
        };
        if !literal {
            p += 1;
            continue;
        }
        // The enclosing statement: back to the `;`, `{` or `}` that ends the
        // one before it, forward to the `;` or `}` that ends this one.
        // Walking backwards, an unmatched `(` or `[` is a call or index the
        // literal sits inside, so the statement goes on; an unmatched `{` is the
        // block it sits in, and a `}` or `;` at this level ends the one before.
        let mut depth = 0u32;
        let mut start = 0;
        for i in (0..p).rev() {
            match code[i] {
                ';' | '}' | '{' if depth == 0 => {
                    start = i + 1;
                    break;
                }
                ')' | ']' | '}' => depth += 1,
                '(' | '[' | '{' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        let mut depth = 0i32;
        let mut end = code.len();
        for (i, c) in code.iter().enumerate().skip(p) {
            match c {
                '(' | '[' | '{' => depth += 1,
                ')' | ']' => depth -= 1,
                '}' if depth <= 0 => {
                    end = i;
                    break;
                }
                '}' => depth -= 1,
                ';' if depth <= 0 => {
                    end = i;
                    break;
                }
                _ => {}
            }
        }
        let statement: String = original[start..end].iter().collect();
        let first_line = line_of(start + statement.len() - statement.trim_start().len());
        let lines: Vec<&str> = source.lines().collect();
        let above = lines[..first_line - 1]
            .iter()
            .rev()
            .take_while(|line| line.trim_start().starts_with("//"))
            .any(|line| line.contains("chip-free:"));
        if !statement.contains("chip-free:") && !above {
            found.push(line_of(p));
        }
        p += word.len();
    }
    found
}

/// The guard's scanner on the shapes a formatter or a hand can give the call:
/// split across lines, a qualified path, the literal bound first, and the
/// reason written anywhere in the statement or on the line above it.
#[test]
fn the_wrap_scan_sees_every_shape_of_a_ratatui_wrap() {
    let w = concat!("Wr", "ap");
    let cases: Vec<(String, Vec<usize>)> = vec![
        (format!("p.wrap({w} {{ trim: false }});\n"), vec![1]),
        (
            format!(
                "frame.render_widget(\n    Paragraph::new(lines)\n        .wrap(\n            \
                 {w} {{ trim: false }},\n        ),\n    area,\n);\n"
            ),
            vec![4],
        ),
        (
            format!("p.wrap(ratatui::widgets::{w} {{ trim: true }});\n"),
            vec![1],
        ),
        (
            format!("let wrap = {w}{{ trim: false }};\np.wrap(wrap);\n"),
            vec![1],
        ),
        (
            format!("// chip-free: constant words.\np.wrap({w} {{ trim: false }});\n"),
            vec![],
        ),
        (
            format!(
                "Paragraph::new(x)\n    // chip-free: a legend.\n    .wrap(\n        {w} {{ trim: \
                 false }},\n    )\n    .render(a, b);\n"
            ),
            vec![],
        ),
        (
            format!(
                "// chip-free: only the first.\na.wrap({w} {{ trim: false }});\nb.wrap({w} {{ \
                 trim: false }});\n"
            ),
            vec![3],
        ),
        (
            format!(
                "/// Renders like `{w} {{ trim: false }}`.\nlet s = \"{w} {{\";\nNo{w} {{ x: 1 }};\n"
            ),
            vec![],
        ),
    ];
    for (source, expected) in cases {
        assert_eq!(unreasoned_ratatui_wraps(&source), expected, "{source}");
    }
}

/// ratatui's `Wrap` breaks at a chip's pads and between the words of a
/// multi-word name, so a body that can carry a chip must be pre-wrapped by the
/// shared wrapper instead. A paragraph that genuinely never carries one says
/// so on the line before, with its reason.
#[test]
fn no_paragraph_that_can_carry_a_chip_is_wrapped_by_ratatui() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut stack = vec![src];
    let mut offenders = Vec::new();
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).expect("read src") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().is_none_or(|ext| ext != "rs") {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("read source");
            for line in unreasoned_ratatui_wraps(production_source(&source)) {
                offenders.push(format!("{}:{line}", path.display()));
            }
        }
    }
    assert!(
        offenders.is_empty(),
        "these paragraphs wrap with ratatui's Wrap; pre-wrap them with \
         wrap_styled_lines (render_wrapped_body) or mark them `// chip-free: <reason>`:\n{}",
        offenders.join("\n")
    );
}

/// Assert every cell of every occurrence of `words` is the dialog body text:
/// the theme's `text_fg` on the modal surface, never the host terminal's
/// default foreground.
fn assert_body_text(app: &App, buf: &Buffer, words: &str) {
    let hits = occurrences(buf, words);
    let shown = screen(buf);
    assert!(!hits.is_empty(), "{words:?} is not on screen:\n{shown}");
    let len = words.chars().count() as u16;
    for (x, y) in hits {
        for cx in x..x + len {
            let cell = &buf[(cx, y)];
            assert_eq!(
                (cell.fg, cell.bg),
                (app.theme.text_fg, app.theme.overlay_bg),
                "{words:?} at ({x},{y}) is not body text at column {cx}:\n{shown}"
            );
        }
    }
}

/// A light theme is where the terminal's default foreground (white in a dark
/// terminal) disappears into the modal surface, so the body text is asked
/// about there: the prose around a chip, in two dialogs.
#[test]
fn dialog_body_text_is_the_themes_text_color_on_a_light_theme() {
    let mut app = test_app(default_bindings());
    app.theme = crate::theme::load("github_light", &app.engine.paths).expect("github_light");
    let project_id = project_with_agents(&mut app, 2);
    let buf = open(
        &mut app,
        PromptState::ConfirmDeleteProject {
            attached: Vec::new(),
            project_id: project_id.clone(),
            project_name: "proj-light".to_string(),
            agent_count: 2,
            focus: ConfirmFocus::Cancel,
            return_to: None,
        },
    );
    assert_body_text(&app, &buf, "This is irreversible.");
    assert_chipped(&app, &buf, "proj-light");

    let buf = open(
        &mut app,
        PromptState::ConfirmRemoveProject {
            attached: Vec::new(),
            project_id,
            project_name: "proj-light".to_string(),
            agent_count: 2,
            orphaned: false,
            focus: ConfirmFocus::Cancel,
            return_to: None,
        },
    );
    assert_body_text(&app, &buf, "This removes");
}
