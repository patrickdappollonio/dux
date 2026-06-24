//! Shared test fixtures used by more than one `app` submodule's test module
//! (currently `input.rs` and `render.rs`). Render-behaviour tests live next to
//! the render code, but they need the same `App` builder and PTY-cursor polling
//! helper as the input tests, so those fixtures live here rather than being
//! duplicated. Compiled only under `#[cfg(test)]` (see the module declaration
//! in `mod.rs`).

use crate::app::{
    App, CenterMode, FocusPane, FullscreenOverlay, InputTarget, MouseLayoutState,
    OverlayMouseLayoutState, PromptState, RightSection, TextInput,
};
use crate::clipboard::Clipboard;
use crate::config::{Config, DuxPaths, ProjectConfig};
use crate::keybindings::{BINDING_DEFS, RuntimeBindings};
use crate::model::{AgentSession, Project, ProjectBranchStatus, ProviderKind, SessionStatus};
use crate::statusline::KeyedStatusController;
use crate::storage::SessionStore;
use crate::theme::Theme;
use chrono::Utc;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, mpsc};
use tempfile::tempdir;

pub(crate) fn default_bindings() -> RuntimeBindings {
    RuntimeBindings::new(
        |action| {
            BINDING_DEFS
                .iter()
                .find(|d| d.action == action)
                .map(|d| d.default_keys.to_vec())
                .unwrap_or_default()
        },
        true,
    )
}

pub(crate) fn run_git(cwd: &std::path::Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}

pub(crate) fn init_test_repo(path: &std::path::Path) {
    run_git(path, &["init", "-b", "main"]);
    run_git(path, &["config", "user.name", "test"]);
    run_git(path, &["config", "user.email", "t@t"]);
    run_git(path, &["commit", "--allow-empty", "-m", "init"]);
}

pub(crate) fn test_app(bindings: RuntimeBindings) -> App {
    let tmp = tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();
    std::mem::forget(tmp);
    init_test_repo(&root);

    let paths = DuxPaths {
        config_path: root.join("config.toml"),
        sessions_db_path: root.join("sessions.sqlite3"),
        worktrees_root: root.join("worktrees"),
        lock_path: root.join("dux.lock"),
        root: root.clone(),
    };
    std::fs::create_dir_all(&paths.worktrees_root).expect("worktrees dir");
    let session_store = SessionStore::open(&paths.sessions_db_path).expect("session store");
    let now = Utc::now();
    let project = Project {
        id: "project-1".to_string(),
        name: "demo".to_string(),
        path: root.to_string_lossy().to_string(),
        explicit_default_provider: None,
        default_provider: ProviderKind::from_str("codex"),
        leading_branch: Some("main".to_string()),
        auto_reopen_agents: None,
        startup_command: None,
        env: Default::default(),
        current_branch: "main".to_string(),
        branch_status: ProjectBranchStatus::Unknown,
        path_missing: false,
        created_at: None,
    };
    session_store
        .upsert_project(&ProjectConfig {
            id: project.id.clone(),
            path: project.path.clone(),
            name: Some(project.name.clone()),
            default_provider: None,
            leading_branch: project.leading_branch.clone(),
            auto_reopen_agents: project.auto_reopen_agents,
            startup_command: project.startup_command.clone(),
            env: project.env.clone(),
        })
        .expect("seed project");
    let session = AgentSession {
        id: "session-1".to_string(),
        project_id: project.id.clone(),
        project_path: Some(project.path.clone()),
        provider: ProviderKind::from_str("codex"),
        source_branch: "main".to_string(),
        branch_name: "agent-branch".to_string(),
        worktree_path: paths.worktrees_root.to_string_lossy().to_string(),
        title: None,
        started_providers: Vec::new(),
        desired_running: false,
        auto_reopen_enabled: true,
        status: SessionStatus::Detached,
        created_at: now,
        updated_at: now,
    };
    let (worker_tx, worker_rx) = mpsc::channel();
    let single_instance_lock = crate::lockfile::SingleInstanceLock::acquire(&paths.lock_path)
        .expect("single-instance lock for test App");
    let config_writer = dux_core::config_queue::ConfigWriteQueue::new(paths.config_path.clone());
    let engine = dux_core::engine::Engine {
        config: Config::default(),
        paths,
        session_store,
        projects: vec![project],
        sessions: vec![session],
        staged_files: Vec::new(),
        unstaged_files: Vec::new(),
        terminal_counter: 0,
        github_integration_enabled: false,
        single_instance_lock,
        worker_tx,
        worker_rx,
        config_writer,
        surface: Box::new(crate::TuiConfigSurface),
        reloading: false,
        deferred_commands: Vec::new(),
        reload_guard: None,
        providers: std::collections::HashMap::new(),
        running_provider_pins: std::collections::HashMap::new(),
        companion_terminals: std::collections::HashMap::new(),
        gh_status: crate::model::GhStatus::Unknown,
        pr_statuses: std::collections::HashMap::new(),
        branch_sync_sessions: Arc::new(Mutex::new(Vec::new())),
        pr_sync_sessions: Arc::new(Mutex::new(Vec::new())),
        pr_sync_enabled: Arc::new(AtomicBool::new(false)),
        refs_watcher: None,
        refs_watch_paths: std::collections::HashMap::new(),
        resume_fallback_candidates: std::collections::HashMap::new(),
        pending_deletions: std::collections::HashSet::new(),
        deletion_busy_messages: std::collections::HashMap::new(),
        watched_worktree: Arc::new(Mutex::new(None::<PathBuf>)),
        watched_session_id: None,
        has_active_processes: Arc::new(AtomicBool::new(false)),
        in_flight: std::collections::HashSet::new(),
        pr_last_checked: std::collections::HashMap::new(),
        changed_files_poller_started: AtomicBool::new(false),
        branch_sync_worker_started: AtomicBool::new(false),
        pty_activity: std::collections::HashMap::new(),
        pty_input: std::collections::HashMap::new(),
        last_foreground_refresh: None,
        pending_auth_users: None,
        pending_web_checkout_ops: std::collections::HashMap::new(),
        pending_web_add_project_ops: std::collections::HashMap::new(),
        pending_web_pr_lookup_ops: std::collections::HashMap::new(),
        pending_delete_ops_web: std::collections::HashMap::new(),
    };
    let mut app = App {
        engine,
        bindings,
        selected_left: 0,
        left_section: crate::app::LeftSection::Projects,
        selected_terminal_index: 0,
        right_section: RightSection::Unstaged,
        files_index: 0,
        files_search: TextInput::new(),
        files_search_active: false,
        commit_input: TextInput::new()
            .with_multiline(4)
            .with_placeholder("Type your commit message\u{2026}"),
        show_diff_line_numbers: false,
        left_width_pct: 20,
        right_width_pct: 23,
        terminal_pane_height_pct: 35,
        staged_pane_height_pct: 50,
        commit_pane_height_pct: 40,
        focus: FocusPane::Left,
        center_mode: CenterMode::Agent,
        left_collapsed: false,
        right_collapsed: false,
        right_hidden: false,
        resize_mode: false,
        help_scroll: None,
        last_help_height: 0,
        last_help_lines: 0,
        fullscreen_overlay: FullscreenOverlay::None,
        startup_log_viewer: None,
        status: KeyedStatusController::with_clear_after(std::time::Duration::ZERO),
        prompt: PromptState::None,
        input_target: InputTarget::None,
        session_surface: crate::model::SessionSurface::Agent,
        clipboard: Clipboard::new(),
        active_terminal_id: None,
        terminal_return_to_list: false,
        last_pty_size: (0, 0),
        prev_scrollback_offset: 0,
        last_diff_height: 0,
        last_diff_visual_lines: 0,
        theme: Theme::default_dark(),
        tick_count: 0,
        start_time: std::time::Instant::now(),
        readonly_nudge_tick: None,
        collapsed_projects: std::collections::HashSet::new(),
        left_items_cache: Vec::new(),
        mouse_layout: MouseLayoutState::default(),
        overlay_layout: OverlayMouseLayoutState::default(),
        mouse_drag: None,
        last_mouse_click: None,
        pressed_button: None,
        interactive_patterns: crate::keybindings::InteractiveBytePatterns {
            bindings: Vec::new(),
        },
        raw_input_parser: crate::raw_input::RawInputParser::default(),
        raw_input_buf: Vec::new(),
        loading_input_buf: Vec::new(),
        in_bracket_paste: false,
        macro_bar: None,
        sigwinch_flag: Arc::new(AtomicBool::new(false)),
        sigwinch_sig_id: None,
        shutdown_flag: Arc::new(AtomicBool::new(false)),
        shutdown_sig_ids: Vec::new(),
        force_redraw: false,
        welcome_tip_index: 0,
        welcome_logo_visible: false,
        welcome_logo_alt: false,
        welcome_tip_selection: usize::MAX,
        pr_banner_at_bottom: true,
        syntax_cache: crate::diff::SyntaxCache::new(),
        snapshot_buf: crate::pty::TerminalSnapshot::empty(),
        last_snapshot_id: None,
        terminal_selection: None,
        startup_log_selection: None,
        pending_server_flip: None,
        server_flip_preflight_pending: false,
        pending_persist_ops: std::collections::HashMap::new(),
        pending_auth_ops: std::collections::HashMap::new(),
        pending_worktree_ops: std::collections::HashMap::new(),
        pending_pr_lookup_ops: std::collections::HashMap::new(),
        pending_delete_ops: std::collections::HashMap::new(),
    };
    app.interactive_patterns = app.bindings.interactive_byte_patterns();
    app.rebuild_left_items();
    app.selected_left = 1;
    app
}

/// Deterministically wait until the PTY child for the active terminal surface
/// has parked its cursor at the given (row, col), polling the live snapshot
/// instead of guessing a fixed sleep. The caller must have set up the surface
/// so `refresh_snapshot_buf` resolves a provider — either `session_surface ==
/// Agent` with the session's provider in `app.engine.providers`, or
/// `session_surface == Terminal` with `active_terminal_id` pointing at an
/// `app.engine.companion_terminals` entry. Panics with the observed cursor if
/// the child does not reach the expected position within ~2s.
pub(crate) fn wait_for_agent_cursor(app: &mut App, row: u16, col: u16) {
    for _ in 0..200 {
        app.refresh_snapshot_buf();
        if matches!(app.snapshot_buf.cursor, Some(c) if c.row == row && c.col == col) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!(
        "PTY did not park its cursor at row {row}, col {col} within 2s (got {:?})",
        app.snapshot_buf.cursor
    );
}
