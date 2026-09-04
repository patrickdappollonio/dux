//! Shared test fixtures used by more than one `app` submodule's test module
//! (currently `input.rs` and `render.rs`). Render-behaviour tests live next to
//! the render code, but they need the same `App` builder and PTY-cursor polling
//! helper as the input tests, so those fixtures live here rather than being
//! duplicated. Compiled only under `#[cfg(test)]` (see the module declaration
//! in `mod.rs`).

use crate::app::{
    App, CenterMode, ChangeAgentProviderMode, ChangeAgentProviderOption, ChangeAgentProviderPrompt,
    ChangeDefaultProviderOption, ChangeDefaultProviderPrompt, ChangeProjectDefaultProviderOption,
    ChangeProjectDefaultProviderPrompt, FocusPane, FullscreenOverlay, InputTarget,
    MouseLayoutState, OverlayMouseLayoutState, PromptState, RightSection, TextInput,
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
        slot_tab_id: "session-1-slot".to_string(),
        provider: ProviderKind::from_str("codex"),
        title: None,
        started_providers: Vec::new(),
        desired_running: false,
        auto_reopen_enabled: true,
        status: SessionStatus::Detached,
        created_at: now,
        updated_at: now,
        last_focused_tab: None,
        workspace: dux_core::model::AgentWorkspace::Managed(dux_core::model::ManagedWorkspace {
            project_id: project.id.clone(),
            project_path: Some(project.path.clone()),
            source_branch: "main".to_string(),
            branch_name: "agent-branch".to_string(),
            initial_branch: "agent-branch".to_string(),
            branch_provenance: dux_core::model::BranchProvenance::CreatedByDux,
            worktree_path: paths.worktrees_root.to_string_lossy().to_string(),
        }),
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
        surface_kind: dux_core::term_identity::SurfaceKind::Tui,
        resource_collector: Default::default(),
        host_env: dux_core::term_identity::HostEnvProbe::default(),
        worker_tx,
        worker_rx,
        config_writer,
        surface: Box::new(crate::TuiConfigSurface),
        reloading: false,
        command_applies: 0,
        deferred_commands: Vec::new(),
        reload_guard: None,
        providers: std::collections::HashMap::new(),
        running_provider_pins: std::collections::HashMap::new(),
        launched_drop_paste: Default::default(),
        companion_terminals: std::collections::HashMap::new(),
        agent_tabs: std::collections::HashMap::new(),
        terminating_ptys: Vec::new(),
        pending_group_removals: Vec::new(),
        gh_status: crate::model::GhStatus::Unknown,
        gh_probe: Default::default(),
        pr_statuses: std::collections::HashMap::new(),
        pr_overrides: std::collections::HashMap::new(),
        pr_suppressions: std::collections::HashSet::new(),
        branch_sync_sessions: Arc::new(Mutex::new(Vec::new())),
        pr_sync_sessions: Arc::new(Mutex::new(Vec::new())),
        pr_sync: Arc::new(Default::default()),
        pr_poll_interval_secs: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        branch_sync_interval_secs: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        branch_sync_wait: Arc::new(Default::default()),
        pr_backoff: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
        refs_watcher: None,
        refs_watch_paths: std::collections::HashMap::new(),
        resume_fallback_candidates: std::collections::HashMap::new(),
        pending_deletions: std::collections::HashSet::new(),
        folder_repo_statuses: std::collections::HashMap::new(),
        closing_sessions: std::collections::HashSet::new(),
        deletion_busy_messages: std::collections::HashMap::new(),
        watched_worktree: Arc::new(Mutex::new(None::<PathBuf>)),
        changed_files_refresh: Default::default(),
        watched_session_id: None,
        current_origin: Default::default(),
        has_active_processes: Arc::new(AtomicBool::new(false)),
        in_flight: std::collections::HashSet::new(),
        rename_expected: std::collections::HashMap::new(),
        pr_last_checked: std::collections::HashMap::new(),
        changed_files_poller_started: AtomicBool::new(false),
        branch_sync_worker_started: AtomicBool::new(false),
        pty_activity: std::collections::HashMap::new(),
        pty_input: std::collections::HashMap::new(),
        pty_pointer: std::collections::HashMap::new(),
        needs_attention: std::collections::HashSet::new(),
        failed_tab_runs: std::collections::HashSet::new(),
        pty_progress: std::collections::HashMap::new(),
        agent_viewed: std::collections::HashMap::new(),
        last_foreground_refresh: None,
        pending_web_checkout_ops: std::collections::HashMap::new(),
        pending_web_add_project_ops: std::collections::HashMap::new(),
        pending_web_pr_lookup_ops: std::collections::HashMap::new(),
        pending_pr_attach_ops: std::collections::HashMap::new(),
        pending_delete_ops_web: std::collections::HashMap::new(),
        pending_create_ops: std::collections::HashMap::new(),
        pending_web_launch_ops: std::collections::HashMap::new(),
        live_status_keys: Default::default(),
        last_created_op_id: None,
        created_session_by_op: std::collections::HashMap::new(),
    };
    let app_live_status_keys = engine.live_status_keys.clone();
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
        last_first_load_height: 0,
        last_first_load_lines: 0,
        last_error_dialog_height: 0,
        last_error_dialog_lines: 0,
        pending_first_load: None,
        unpushed_count_rx: None,
        notes_fetch_rx: None,
        deferred_first_load_notes: None,
        notes_fetch_explicit_request: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(
            false,
        )),
        fullscreen_overlay: FullscreenOverlay::None,
        startup_log_viewer: None,
        status: KeyedStatusController::with_clear_after(std::time::Duration::ZERO)
            .with_live_keys(app_live_status_keys),
        missing_project_warning_gen: None,
        prompt: PromptState::None,
        input_target: InputTarget::None,
        session_surface: crate::model::SessionSurface::Agent,
        clipboard: Clipboard::new(),
        active_terminal_id: None,
        focused_tabs: std::collections::HashMap::new(),
        host_forward_carry: Vec::new(),
        host_forward_error_logged_at: None,
        agent_tab_regions: Vec::new(),
        terminal_return_to_list: false,
        last_pty_size: (0, 0),
        last_pty_resize_target: None,
        tui_launched_ptys: Default::default(),
        create_agent_started_here: false,
        pending_pty_takeover: None,
        last_refused_pty_resize: None,
        grid_generation: 0,
        scroll_mode: std::collections::HashSet::new(),
        last_diff_height: 0,
        last_diff_visual_lines: 0,
        theme: Theme::default_dark(),
        tick_count: 0,
        start_time: std::time::Instant::now(),
        refusal_blink: None,
        inactive_collapsed: false,
        inactive_search_dismissed: None,
        inactive_collapse_overridden: false,
        left_items_cache: Vec::new(),
        mouse_layout: MouseLayoutState::default(),
        overlay_layout: OverlayMouseLayoutState::default(),
        mouse_drag: None,
        row_drag: None,
        center_mouse_forward: None,
        last_mouse_click: None,
        pressed_button: None,
        takeover_press: None,
        interactive_patterns: crate::keybindings::InteractiveBytePatterns {
            bindings: Vec::new(),
        },
        raw_input_parser: crate::raw_input::RawInputParser::default(),
        raw_input_buf: Vec::new(),
        loading_input_buf: Vec::new(),
        in_bracket_paste: false,
        raw_paste_normalize: false,
        raw_paste_prev_cr: false,
        terminal_focus: crate::focus::TerminalFocus::new(),
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
        syntax_cache: std::sync::Arc::new(crate::diff::SyntaxCache::new()),
        pending_diff: None,
        diff_request_seq: 0,
        snapshot_buf: crate::pty::TerminalSnapshot::empty(),
        last_snapshot_id: None,
        terminal_selection: None,
        pending_link_click: None,
        pending_pr_banner_press: None,
        last_link_open: None,
        url_opener: crate::app::default_url_opener(),
        startup_log_selection: None,
        pending_server_flip: None,
        companion: None,
        background_server_preflight_pending: false,
        background_server_wanted: false,
        companion_followup_ran: false,
        pending_background_server_op: None,
        pending_tailscale_mode_op: None,
        server_flip_preflight_pending: false,
        pending_persist_ops: std::collections::HashMap::new(),
        pending_worktree_ops: std::collections::HashMap::new(),
        pending_pr_lookup_ops: std::collections::HashMap::new(),
        pending_pr_reference: None,
        pending_pr_reference_op: None,
        dispatched_pr_lookups: Vec::new(),
        pending_delete_ops: std::collections::HashMap::new(),
        pending_reconnect_ops: std::collections::HashMap::new(),
        pending_checkout_inspect_ops: std::collections::HashMap::new(),
        pending_changed_files_refresh: None,
        pending_server_flip_op: None,
        pending_config_reload_op: None,
        project_chooser_context: None,
        agent_filter: None,
    };
    app.interactive_patterns = app.bindings.interactive_byte_patterns();
    app.rebuild_left_items();
    app.selected_left = 1;
    app
}

/// Put the selected terminal surface into SCROLL MODE the way a user does:
/// wait until the child has actually produced scrollback history, scroll up by
/// `lines`, and record the gesture through the same entry point the scroll keys
/// and the wheel use. Polls instead of sleeping a fixed amount, so the test is
/// waiting on the fact it depends on (real history) rather than on a guess.
/// Panics if no history appears within ~2s.
pub(crate) fn enter_scroll_mode(app: &mut App, lines: usize) {
    for _ in 0..200 {
        if let Some(provider) = app.selected_terminal_surface_client() {
            provider.scroll(true, lines);
            if provider.scrollback_offset() > 0 {
                app.note_user_scroll();
                assert!(
                    app.scroll_mode_active(),
                    "a user scroll above the live edge must enter scroll mode"
                );
                return;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("PTY produced no scrollback to scroll into within 2s");
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

/// Block until the PTY client keyed by `key` is in the exact state the exit
/// prune requires, END OF INPUT plus a REAPED EXIT STATUS, then return so the
/// caller can `drain_events()` exactly ONCE and assert the prune happened.
///
/// Both arms are load-bearing, and waiting on either alone has raced in the
/// past. They are different facts arriving from different places: `is_exited` is
/// set by the reader thread when the read side EOFs, while `try_wait` reaps the
/// child and stamps the reap instant. The prune policy is
/// `dux_core::engine::agent_pty_ready_to_prune(exit_status_known, since_eof,
/// since_reap)`, which REFUSES to prune until it holds both, until
/// `REAPED_DRAIN_GRACE` (250ms) expires on either clock. Each fact is read
/// exactly once and cannot be recovered afterwards, so pruning early loses one:
/// without the drain the crash excerpt comes off a half-filled buffer, and
/// without the status `exit_success` is `None`, which stops a clean exit from
/// closing its tab row. Break on `is_exited` alone and the test lands inside the
/// window where the reader has EOFed but the child is not yet waitable (the
/// kernel closes a dying task's descriptors before it makes the task waitable),
/// one drain sees nothing, and the prune assertion fires: that is the roughly
/// 1-in-40 flake this helper exists to remove.
///
/// `try_wait` is memoized on `PtyClient`, so polling it here does not steal the
/// status from the prune that follows.
///
/// A single drain, rather than a retry loop, is also deliberate: with both facts
/// in hand the prune MUST fire. That is a product guarantee, and a loop would
/// stop pinning it.
pub(crate) fn wait_for_pty_eof(app: &mut App, key: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while !app
        .engine
        .providers
        .get_mut(dux_core::ids::TabIdRef::new(key))
        .is_some_and(|c| c.is_exited() && c.try_wait().is_some())
    {
        assert!(
            std::time::Instant::now() < deadline,
            "PTY {key} never reached end of input with a reaped exit status"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// Build the three provider pickers in a state a user can really reach:
/// two options, the FIRST one already active (so it is the no-op row) and
/// the second one a real change.
pub(crate) fn agent_provider_prompt() -> ChangeAgentProviderPrompt {
    ChangeAgentProviderPrompt {
        session_id: "s1".to_string(),
        tab_id: "s1".to_string(),
        session_label: "agent".to_string(),
        worktree_path: "/tmp/wt".to_string(),
        options: vec![
            ChangeAgentProviderOption {
                provider: ProviderKind::new("claude"),
                supports_resume: true,
                resume_available: false,
                is_current: true,
            },
            ChangeAgentProviderOption {
                provider: ProviderKind::new("codex"),
                supports_resume: true,
                resume_available: false,
                is_current: false,
            },
        ],
        selected: 0,
        mode: ChangeAgentProviderMode::Retarget,
    }
}

pub(crate) fn default_provider_prompt() -> ChangeDefaultProviderPrompt {
    ChangeDefaultProviderPrompt {
        current: ProviderKind::new("claude"),
        options: vec![
            ChangeDefaultProviderOption {
                provider: ProviderKind::new("claude"),
                is_current: true,
            },
            ChangeDefaultProviderOption {
                provider: ProviderKind::new("codex"),
                is_current: false,
            },
        ],
        selected: 0,
    }
}

pub(crate) fn project_default_provider_prompt(
    project_id: String,
    project_name: String,
) -> ChangeProjectDefaultProviderPrompt {
    ChangeProjectDefaultProviderPrompt {
        project_id,
        project_name,
        current: ProviderKind::new("claude"),
        global_default: ProviderKind::new("claude"),
        inherits_global_default: true,
        options: vec![
            ChangeProjectDefaultProviderOption {
                provider: None,
                is_current: true,
            },
            ChangeProjectDefaultProviderOption {
                provider: Some(ProviderKind::new("codex")),
                is_current: false,
            },
        ],
        selected: 0,
    }
}
