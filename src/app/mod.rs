use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::stdout;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::Utc;
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
    MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, ListState, Paragraph, StatefulWidget, Widget, Wrap,
};
use uuid::Uuid;

use crate::clipboard::Clipboard;
use crate::config::{
    Config, DuxPaths, MacroSurface, ProjectConfig, ProviderCommandConfig, check_provider_available,
    ensure_config, save_config, validate_keys,
};
use crate::diff::SyntaxCache;
use crate::editor::DetectedEditor;
use crate::git;
use crate::keybindings::{
    Action, BindingScope, HintContext, InteractiveBytePatterns, RuntimeBindings,
};
use crate::logger;
use crate::model::{
    AgentSession, ChangedFile, CompanionTerminalStatus, Project, ProviderKind, SessionStatus,
    SessionSurface,
};
use crate::provider;
use crate::pty::PtyClient;
use crate::pty::TerminalSnapshot;
use crate::statusline::{StatusLine, StatusTone};
use crate::storage::SessionStore;
use crate::theme::Theme;

use text_input::TextInput;

pub struct App {
    pub(crate) config: Config,
    pub(crate) paths: DuxPaths,
    pub(crate) bindings: RuntimeBindings,
    pub(crate) session_store: SessionStore,
    pub(crate) projects: Vec<Project>,
    pub(crate) sessions: Vec<AgentSession>,
    pub(crate) staged_files: Vec<ChangedFile>,
    pub(crate) unstaged_files: Vec<ChangedFile>,
    pub(crate) selected_left: usize,
    pub(crate) left_section: LeftSection,
    pub(crate) selected_terminal_index: usize,
    pub(crate) right_section: RightSection,
    pub(crate) files_index: usize,
    pub(crate) files_search: TextInput,
    pub(crate) files_search_active: bool,
    pub(crate) commit_input: TextInput,
    pub(crate) left_width_pct: u16,
    pub(crate) right_width_pct: u16,
    pub(crate) terminal_pane_height_pct: u16,
    pub(crate) staged_pane_height_pct: u16,
    pub(crate) commit_pane_height_pct: u16,
    pub(crate) focus: FocusPane,
    pub(crate) center_mode: CenterMode,
    pub(crate) left_collapsed: bool,
    pub(crate) right_collapsed: bool,
    pub(crate) right_hidden: bool,
    pub(crate) resize_mode: bool,
    pub(crate) help_scroll: Option<u16>,
    pub(crate) last_help_height: u16,
    pub(crate) last_help_lines: u16,
    pub(crate) fullscreen_overlay: FullscreenOverlay,
    pub(crate) status: StatusLine,
    pub(crate) prompt: PromptState,
    pub(crate) input_target: InputTarget,
    pub(crate) session_surface: SessionSurface,
    pub(crate) clipboard: Clipboard,
    pub(crate) worker_tx: Sender<WorkerEvent>,
    pub(crate) worker_rx: Receiver<WorkerEvent>,
    pub(crate) providers: HashMap<String, PtyClient>,
    pub(crate) companion_terminals: HashMap<String, CompanionTerminal>,
    pub(crate) active_terminal_id: Option<String>,
    pub(crate) terminal_return_to_list: bool,
    pub(crate) terminal_counter: usize,
    pub(crate) create_agent_in_flight: bool,
    pub(crate) pulls_in_flight: HashSet<String>,
    pub(crate) resource_stats_in_flight: bool,
    pub(crate) last_pty_size: (u16, u16),
    /// Tracks when each agent last received PTY data, for the streaming
    /// activity spinner in the left pane.
    pub(crate) last_pty_activity: HashMap<String, Instant>,
    pub(crate) prev_scrollback_offset: usize,
    pub(crate) show_diff_line_numbers: bool,
    pub(crate) last_diff_height: u16,
    pub(crate) last_diff_visual_lines: u16,
    pub(crate) theme: Theme,
    pub(crate) tick_count: u64,
    /// Wall-clock reference for time-based animations (spinners). Using
    /// elapsed time instead of `tick_count` keeps animation speed constant
    /// regardless of how fast the event loop is running.
    pub(crate) start_time: Instant,
    pub(crate) readonly_nudge_tick: Option<u64>,
    pub(crate) watched_worktree: Arc<Mutex<Option<PathBuf>>>,
    pub(crate) has_active_processes: Arc<AtomicBool>,
    pub(crate) collapsed_projects: HashSet<String>,
    pub(crate) left_items_cache: Vec<LeftItem>,
    pub(crate) mouse_layout: MouseLayoutState,
    pub(crate) overlay_layout: OverlayMouseLayoutState,
    pub(crate) mouse_drag: Option<ResizeDragState>,
    pub(crate) last_mouse_click: Option<RecentMouseClick>,
    pub(crate) interactive_patterns: InteractiveBytePatterns,
    pub(crate) raw_input_buf: Vec<u8>,
    pub(crate) macro_bar: Option<MacroBarState>,
    pub(crate) sigwinch_flag: Arc<AtomicBool>,
    pub(crate) force_redraw: bool,
    pub(crate) welcome_tip_index: usize,
    /// Whether the ASCII logo was rendered in the previous frame.
    pub(crate) welcome_logo_visible: bool,
    /// The left-pane selection index when the logo last rendered a tip.
    pub(crate) welcome_tip_selection: usize,
    pub(crate) branch_sync_sessions: Arc<Mutex<Vec<BranchSyncEntry>>>,
    pub(crate) gh_status: crate::model::GhStatus,
    pub(crate) github_integration_enabled: bool,
    pub(crate) pr_banner_at_bottom: bool,
    pub(crate) pr_statuses: HashMap<String, crate::model::PrInfo>,
    pub(crate) pr_sync_sessions: Arc<Mutex<Vec<PrSyncEntry>>>,
    pub(crate) pr_sync_enabled: Arc<AtomicBool>,
    /// Timestamps of the last PR check per session, to avoid hammering on rapid
    /// state transitions.
    pub(crate) pr_last_checked: HashMap<String, Instant>,
    /// File-system watcher for `.git/refs/heads/` directories. `None` if the
    /// watcher could not be created (graceful fallback to poll-only).
    pub(crate) refs_watcher: Option<Arc<Mutex<notify::RecommendedWatcher>>>,
    /// Maps watched worktree paths back to session IDs so the refs watcher
    /// can route change events.
    pub(crate) refs_watch_paths: HashMap<PathBuf, String>,
    /// Session IDs spawned with resume_args that should fall back to regular
    /// args if the PTY exits before producing any output.
    pub(crate) resume_fallback_candidates: HashSet<String>,
    /// Cached syntax highlighting resources shared across diff computations.
    pub(crate) syntax_cache: SyntaxCache,
    /// Reusable snapshot buffer to avoid per-frame allocation of terminal cells.
    pub(crate) snapshot_buf: TerminalSnapshot,
    /// ID of the provider that last populated `snapshot_buf`, used to detect
    /// agent switches and force a snapshot rebuild.
    last_snapshot_id: Option<String>,
    /// Active text selection in the terminal viewport, if any.
    pub(crate) terminal_selection: Option<TerminalSelection>,
}

/// Snapshot of session data shared with the branch-sync background worker.
#[derive(Clone, Debug)]
pub(crate) struct BranchSyncEntry {
    pub(crate) session_id: String,
    pub(crate) worktree_path: String,
    pub(crate) branch_name: String,
}

/// Snapshot of session data shared with the PR-sync background worker.
#[derive(Clone, Debug)]
pub(crate) struct PrSyncEntry {
    pub(crate) session_id: String,
    pub(crate) branch_name: String,
    pub(crate) worktree_path: String,
    /// If we already know a PR for this session, the worker can use `gh pr view`
    /// (works even after branch deletion) and skip terminal states (merged/closed).
    pub(crate) known_pr: Option<crate::storage::StoredPr>,
    /// Whether the agent process has exited. Used to skip PR discovery calls
    /// for sessions that are both exited and in a terminal PR state — nobody
    /// is pushing to that branch anymore.
    pub(crate) agent_exited: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FocusPane {
    Left,
    Center,
    Files,
}

impl FocusPane {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Left => Self::Center,
            Self::Center => Self::Files,
            Self::Files => Self::Left,
        }
    }

    pub(crate) fn previous(self) -> Self {
        match self {
            Self::Left => Self::Files,
            Self::Center => Self::Left,
            Self::Files => Self::Center,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RightSection {
    Unstaged,
    Staged,
    CommitInput,
}

impl RightSection {
    /// Returns the next section, or `None` to exit the pane.
    /// Order: Unstaged → Staged → CommitInput.
    pub(crate) fn next(self, has_staged: bool) -> Option<Self> {
        match self {
            Self::Unstaged if has_staged => Some(Self::Staged),
            Self::Unstaged => None,
            Self::Staged => Some(Self::CommitInput),
            Self::CommitInput => None,
        }
    }

    /// Returns the previous section, or `None` to exit the pane.
    pub(crate) fn previous(self) -> Option<Self> {
        match self {
            Self::CommitInput => Some(Self::Staged),
            Self::Staged => Some(Self::Unstaged),
            Self::Unstaged => None,
        }
    }

    /// First section when entering the pane (always Changes/Unstaged on top).
    pub(crate) fn first() -> Self {
        Self::Unstaged
    }

    pub(crate) fn last(has_staged: bool) -> Self {
        if has_staged {
            Self::CommitInput
        } else {
            Self::Unstaged
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FullscreenOverlay {
    None,
    Agent,
    Terminal,
}

#[derive(Clone, Debug)]
pub(crate) enum CenterMode {
    Agent,
    Diff {
        lines: Arc<Vec<Line<'static>>>,
        scroll: u16,
        /// Source paths for re-generating the diff on setting changes.
        worktree_path: String,
        rel_path: String,
    },
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum KillableRuntimeKind {
    Agent,
    Terminal,
}

impl KillableRuntimeKind {
    pub(crate) fn noun(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Terminal => "terminal",
        }
    }

    pub(crate) fn badge(self) -> &'static str {
        match self {
            Self::Agent => "AGENT",
            Self::Terminal => "TERM",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum RuntimeTargetId {
    Agent(String),
    Terminal(String),
}

#[derive(Clone, Debug)]
pub(crate) struct KillableRuntime {
    pub(crate) id: RuntimeTargetId,
    pub(crate) kind: KillableRuntimeKind,
    pub(crate) label: String,
    pub(crate) context: String,
    pub(crate) search_text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KillRunningAction {
    Hovered,
    Selected,
    Visible,
}

impl KillRunningAction {
    pub(crate) fn button_label(self) -> &'static str {
        match self {
            Self::Hovered => "Kill Hovered",
            Self::Selected => "Kill Selected",
            Self::Visible => "Kill Visible",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KillRunningFooterAction {
    Cancel,
    Hovered,
    Selected,
    Visible,
}

impl KillRunningFooterAction {
    pub(crate) fn button_label(self) -> &'static str {
        match self {
            Self::Cancel => "Cancel",
            Self::Hovered => KillRunningAction::Hovered.button_label(),
            Self::Selected => KillRunningAction::Selected.button_label(),
            Self::Visible => KillRunningAction::Visible.button_label(),
        }
    }

    pub(crate) fn action(self) -> Option<KillRunningAction> {
        match self {
            Self::Cancel => None,
            Self::Hovered => Some(KillRunningAction::Hovered),
            Self::Selected => Some(KillRunningAction::Selected),
            Self::Visible => Some(KillRunningAction::Visible),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KillRunningFocus {
    List,
    Footer(KillRunningFooterAction),
}

#[derive(Clone, Debug)]
pub(crate) struct KillRunningPrompt {
    pub(crate) runtimes: Vec<KillableRuntime>,
    pub(crate) filter: TextInput,
    pub(crate) searching: bool,
    pub(crate) hovered_visible_index: usize,
    pub(crate) selected_ids: HashSet<RuntimeTargetId>,
    pub(crate) focus: KillRunningFocus,
}

#[derive(Clone, Debug)]
pub(crate) struct ConfirmKillRunningPrompt {
    pub(crate) previous: KillRunningPrompt,
    pub(crate) action: KillRunningAction,
    pub(crate) target_ids: Vec<RuntimeTargetId>,
    pub(crate) confirm_selected: bool,
}

#[derive(Clone, Debug)]
pub(crate) enum PromptState {
    None,
    Command {
        input: TextInput,
        selected: usize,
    },
    BrowseProjects {
        current_dir: PathBuf,
        entries: Vec<BrowserEntry>,
        loading: bool,
        selected: usize,
        filter: TextInput,
        searching: bool,
        editing_path: bool,
        path_input: TextInput,
        tab_completions: Vec<String>,
        tab_index: usize,
    },
    KillRunning(KillRunningPrompt),
    ConfirmKillRunning(ConfirmKillRunningPrompt),
    ConfirmDeleteAgent {
        session_id: String,
        branch_name: String,
        confirm_selected: bool, // false = Cancel (default), true = Delete
    },
    ConfirmQuit {
        agent_count: usize,
        terminal_count: usize,
        confirm_selected: bool, // false = Cancel (default), true = Quit
    },
    ConfirmDiscardFile {
        file_path: String,
        is_untracked: bool,
        confirm_selected: bool, // false = Cancel (default), true = Discard
    },
    RenameSession {
        session_id: String,
        input: TextInput,
        rename_branch: bool,
    },
    NameNewAgent {
        request: CreateAgentRequest,
        input: TextInput,
    },
    PickEditor {
        session_label: String,
        worktree_path: String,
        editors: Vec<DetectedEditor>,
        selected: usize,
    },
    EditMacros {
        entries: Vec<(String, String, MacroSurface)>,
        selected: usize,
        editing: Option<MacroEditState>,
    },
    DebugInput {
        lines: Vec<Line<'static>>,
        scroll_offset: u16,
    },
    ResourceMonitor {
        rows: Vec<ResourceStats>,
        scroll_offset: u16,
        last_refresh: Instant,
        first_sample: bool,
    },
}

#[derive(Clone, Debug)]
pub(crate) struct ResourceStats {
    pub(crate) label: String,
    pub(crate) pid: Option<u32>,
    pub(crate) cpu_percent: f32,
    pub(crate) rss_bytes: u64,
    pub(crate) process_count: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct MacroBarState {
    pub(crate) input: TextInput,
    pub(crate) selected: usize,
    pub(crate) previous_input_target: InputTarget,
}

#[derive(Clone, Debug)]
pub(crate) struct MacroEditState {
    pub(crate) id: Option<String>,
    pub(crate) name_input: TextInput,
    pub(crate) text_input: TextInput,
    pub(crate) surface: MacroSurface,
    pub(crate) stage: MacroEditStage,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MacroEditStage {
    EditName,
    EditText,
}

#[derive(Clone, Debug)]
pub(crate) struct BrowserEntry {
    pub(crate) path: PathBuf,
    pub(crate) label: String,
    pub(crate) is_git_repo: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InputTarget {
    None,
    Agent,
    Terminal,
    CommitMessage,
}

#[derive(Clone, Copy)]
pub(crate) enum ScrollDirection {
    Up,
    Down,
}

/// A position within the terminal grid (0-based row and column).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct TermGridPos {
    pub row: u16,
    pub col: u16,
}

/// Active text selection in the terminal viewport.
#[derive(Clone, Debug)]
pub(crate) struct TerminalSelection {
    /// Where the drag started (anchor). Fixed during drag.
    pub anchor: TermGridPos,
    /// Current end of selection. Moves during drag.
    pub end: TermGridPos,
    /// True while the mouse button is held (still dragging).
    pub dragging: bool,
}

impl TerminalSelection {
    /// Returns (start, end) in reading order (top-left to bottom-right).
    pub fn ordered(&self) -> (TermGridPos, TermGridPos) {
        if self.anchor.row < self.end.row
            || (self.anchor.row == self.end.row && self.anchor.col <= self.end.col)
        {
            (self.anchor, self.end)
        } else {
            (self.end, self.anchor)
        }
    }

    /// Returns true if the given (row, col) is within the selection.
    /// Uses line-based (not rectangular) selection semantics.
    pub fn contains(&self, row: u16, col: u16) -> bool {
        let (start, end) = self.ordered();
        if row < start.row || row > end.row {
            return false;
        }
        if start.row == end.row {
            return col >= start.col && col <= end.col;
        }
        if row == start.row {
            return col >= start.col;
        }
        if row == end.row {
            return col <= end.col;
        }
        true // middle rows are fully selected
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct MouseLayoutState {
    pub(crate) body: Rect,
    pub(crate) left: Rect,
    pub(crate) center: Rect,
    pub(crate) right: Rect,
    pub(crate) left_list: Rect,
    pub(crate) terminal_list: Rect,
    pub(crate) agent_term: Option<Rect>,
    pub(crate) unstaged_list: Option<Rect>,
    pub(crate) staged_list: Option<Rect>,
    pub(crate) commit_area: Option<Rect>,
    pub(crate) commit_text_area: Option<Rect>,
}

impl MouseLayoutState {
    pub(crate) fn reset(&mut self, body: Rect, left: Rect, center: Rect, right: Rect) {
        self.body = body;
        self.left = left;
        self.center = center;
        self.right = right;
        self.left_list = Rect::default();
        self.terminal_list = Rect::default();
        self.agent_term = None;
        self.unstaged_list = None;
        self.staged_list = None;
        self.commit_area = None;
        self.commit_text_area = None;
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct OverlayMouseLayoutState {
    pub(crate) active: OverlayMouseLayout,
}

impl OverlayMouseLayoutState {
    pub(crate) fn reset(&mut self) {
        self.active = OverlayMouseLayout::None;
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) enum OverlayMouseLayout {
    #[default]
    None,
    Help,
    Command {
        input: Rect,
        list: Rect,
        items: usize,
        offset: usize,
    },
    BrowseProjects {
        input: Option<Rect>,
        list: Rect,
        items: usize,
        offset: usize,
    },
    PickEditor {
        list: Rect,
        items: usize,
        offset: usize,
    },
    KillRunning {
        input: Option<Rect>,
        list: Rect,
        items: usize,
        offset: usize,
        cancel_button: Rect,
        hovered_button: Rect,
        selected_button: Rect,
        visible_button: Rect,
    },
    ConfirmKillRunning {
        cancel_button: Rect,
        kill_button: Rect,
    },
    ConfirmDeleteAgent {
        cancel_button: Rect,
        delete_button: Rect,
    },
    ConfirmQuit {
        cancel_button: Rect,
        quit_button: Rect,
    },
    ConfirmDiscardFile {
        cancel_button: Rect,
        discard_button: Rect,
    },
    RenameSession {
        input: Rect,
    },
    NameNewAgent {
        input: Rect,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub(crate) enum ResizeDragState {
    LeftDivider,
    RightDivider,
    TerminalDivider,
    StagedDivider,
    CommitDivider,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MouseClickTarget {
    LeftPane,
    CenterPane,
    UnstagedPane,
    StagedPane,
    CommandPalette,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RecentMouseClick {
    pub(crate) target: MouseClickTarget,
    pub(crate) item_index: Option<usize>,
    pub(crate) at: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LeftSection {
    Projects,
    Terminals,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum LeftItem {
    Project(usize),
    Session(usize),
}

pub(crate) struct CompanionTerminal {
    pub(crate) session_id: String,
    pub(crate) label: String,
    pub(crate) foreground_cmd: Option<String>,
    pub(crate) client: PtyClient,
}

pub(crate) struct AgentReadyData {
    pub session: AgentSession,
    pub client: PtyClient,
    pub pty_size: (u16, u16), // (rows, cols) the PTY was spawned with
    pub status_message: String,
}

#[derive(Clone, Debug)]
pub(crate) enum CreateAgentRequest {
    NewProject {
        project: Project,
        custom_name: Option<String>,
    },
    ForkSession {
        project: Project,
        source_session: Box<AgentSession>,
        source_label: String,
        custom_name: Option<String>,
    },
}

pub(crate) enum WorkerEvent {
    CreateAgentProgress(String),
    CreateAgentReady(Box<AgentReadyData>),
    CreateAgentFailed(String),
    ChangedFilesReady {
        staged: Vec<ChangedFile>,
        unstaged: Vec<ChangedFile>,
    },
    CommitMessageGenerated(String),
    CommitMessageFailed(String),
    PushCompleted(Result<(), String>),
    PullCompleted {
        repo_path: String,
        target: PullTarget,
        result: Result<Option<String>, String>,
    },
    BrowserEntriesReady {
        dir: PathBuf,
        entries: Vec<BrowserEntry>,
    },
    ClipboardCopyCompleted {
        /// Human-readable success message shown in the status bar.
        label: String,
        result: Result<(), String>,
    },
    BranchSyncReady(Vec<(String, String)>),
    BranchRenameCompleted {
        session_id: String,
        new_branch: String,
        previous_title: Option<String>,
        result: Result<(), String>,
    },
    ResourceStatsReady(Vec<ResourceStats>),
    GhStatusChecked(crate::model::GhStatus),
    PrStatusReady(Vec<(String, Option<crate::model::PrInfo>)>),
    RefsChanged(String),
}

#[derive(Clone, Debug)]
pub(crate) enum PullTarget {
    Project {
        project_id: String,
        project_name: String,
    },
    Session,
}

mod input;
mod render;
mod sessions;
pub(crate) mod text_input;
mod workers;

impl App {
    pub fn bootstrap() -> Result<Self> {
        let paths = DuxPaths::discover()?;
        let config = ensure_config(&paths)?;
        logger::init(&config.logging, &paths);
        logger::info("bootstrapping dux");

        // Validate and build runtime keybindings from config.
        if let Err(msg) = validate_keys(&config.keys) {
            eprintln!(
                "Configuration error in {}: {msg}",
                paths.config_path.display()
            );
            std::process::exit(1);
        }
        let bindings = RuntimeBindings::from_keys_config(&config.keys);
        let interactive_patterns = bindings.interactive_byte_patterns();

        // Register SIGWINCH handler so we can detect terminal resizes even when
        // bypassing crossterm's event reader during interactive mode.
        let sigwinch_flag = Arc::new(AtomicBool::new(false));
        signal_hook::flag::register(signal_hook::consts::SIGWINCH, Arc::clone(&sigwinch_flag))?;

        let session_store = SessionStore::open(&paths.sessions_db_path)?;
        let projects = load_projects(&config);
        let sessions = session_store.load_sessions()?;
        let (worker_tx, worker_rx) = mpsc::channel();
        let watched_worktree: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
        let initial_status = format!(
            "Press {} to add a project, {} to create an agent, {} for help.",
            bindings.label_for(Action::OpenProjectBrowser),
            bindings.label_for(Action::NewAgent),
            bindings.label_for(Action::ToggleHelp),
        );
        let gh_integration_val = config.ui.github_integration;
        let pr_banner_at_bottom = config.ui.pr_banner_position == "bottom";
        let mut app = Self {
            show_diff_line_numbers: config.ui.show_diff_line_numbers,
            left_width_pct: config.ui.left_width_pct,
            right_width_pct: config.ui.right_width_pct,
            terminal_pane_height_pct: config.ui.terminal_pane_height_pct,
            staged_pane_height_pct: config.ui.staged_pane_height_pct,
            commit_pane_height_pct: config.ui.commit_pane_height_pct,
            bindings,
            config,
            paths,
            session_store,
            projects,
            sessions,
            staged_files: Vec::new(),
            unstaged_files: Vec::new(),
            selected_left: 0,
            left_section: LeftSection::Projects,
            selected_terminal_index: 0,
            right_section: RightSection::Unstaged,
            files_index: 0,
            files_search: TextInput::new(),
            files_search_active: false,
            commit_input: TextInput::new()
                .with_multiline(4)
                .with_placeholder("Type your commit message\u{2026}"),
            left_collapsed: false,
            right_collapsed: false,
            right_hidden: false,
            focus: FocusPane::Left,
            center_mode: CenterMode::Agent,
            resize_mode: false,
            help_scroll: None,
            last_help_height: 0,
            last_help_lines: 0,
            fullscreen_overlay: FullscreenOverlay::None,
            status: StatusLine::new(initial_status),
            prompt: PromptState::None,
            input_target: InputTarget::None,
            session_surface: SessionSurface::Agent,
            clipboard: Clipboard::new(),
            worker_tx,
            worker_rx,
            providers: HashMap::new(),
            companion_terminals: HashMap::new(),
            active_terminal_id: None,
            terminal_return_to_list: false,
            terminal_counter: 0,
            create_agent_in_flight: false,
            pulls_in_flight: HashSet::new(),
            resource_stats_in_flight: false,
            last_pty_size: (0, 0),
            last_pty_activity: HashMap::new(),
            prev_scrollback_offset: 0,
            last_diff_height: 0,
            last_diff_visual_lines: 0,
            theme: Theme::default_dark(),
            tick_count: 0,
            start_time: Instant::now(),
            readonly_nudge_tick: None,
            watched_worktree: Arc::clone(&watched_worktree),
            has_active_processes: Arc::new(AtomicBool::new(false)),
            collapsed_projects: HashSet::new(),
            left_items_cache: Vec::new(),
            mouse_layout: MouseLayoutState::default(),
            overlay_layout: OverlayMouseLayoutState::default(),
            mouse_drag: None,
            last_mouse_click: None,
            interactive_patterns,
            raw_input_buf: Vec::new(),
            macro_bar: None,
            sigwinch_flag,
            force_redraw: false,
            welcome_tip_index: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as usize)
                .unwrap_or(0),
            welcome_logo_visible: false,
            welcome_tip_selection: usize::MAX,
            branch_sync_sessions: Arc::new(Mutex::new(Vec::new())),
            gh_status: crate::model::GhStatus::Unknown,
            github_integration_enabled: gh_integration_val,
            pr_banner_at_bottom,
            pr_statuses: HashMap::new(),
            pr_sync_sessions: Arc::new(Mutex::new(Vec::new())),
            pr_sync_enabled: Arc::new(AtomicBool::new(false)),
            pr_last_checked: HashMap::new(),
            refs_watcher: None,
            refs_watch_paths: HashMap::new(),
            resume_fallback_candidates: HashSet::new(),
            syntax_cache: SyntaxCache::new(),
            snapshot_buf: TerminalSnapshot::empty(),
            last_snapshot_id: None,
            terminal_selection: None,
        };
        app.restore_sessions();
        app.seed_pr_statuses_from_db();
        app.rebuild_left_items();
        app.reload_changed_files();
        app.update_branch_sync_sessions();
        Ok(app)
    }

    pub fn run(&mut self) -> Result<()> {
        self.spawn_changed_files_poller();
        self.spawn_branch_sync_worker();
        self.spawn_gh_status_check();
        let mut terminal = ratatui::init();
        execute!(stdout(), EnableMouseCapture)?;

        let result: Result<()> = {
            loop {
                self.drain_events();
                self.poll_pty_activity();
                self.tick_count = self.tick_count.wrapping_add(1);

                // Check SIGWINCH — needed when bypassing crossterm's event
                // reader (which would otherwise deliver Resize events).
                if self.sigwinch_flag.swap(false, Ordering::Relaxed)
                    && let Err(err) = crate::io_retry::retry_on_interrupt(|| terminal.autoresize())
                {
                    self.report_runtime_error("terminal resize failed", &err);
                }

                if self.force_redraw {
                    self.force_redraw = false;
                    if let Err(err) = terminal.clear() {
                        self.report_runtime_error("force redraw failed", &err);
                    }
                    // Re-enable mouse capture — terminal.clear() resets
                    // terminal state which drops the mouse capture mode.
                    let _ = execute!(stdout(), EnableMouseCapture);
                }

                if let Err(err) = terminal.draw(|frame| self.render(frame)) {
                    self.report_runtime_error("terminal draw failed", &err);
                    thread::sleep(Duration::from_millis(100));
                    continue;
                }

                if matches!(
                    self.input_target,
                    InputTarget::Agent | InputTarget::Terminal
                ) {
                    // Interactive mode: read raw stdin and forward to PTY.
                    // crossterm's event reader is not called — all bytes
                    // (keyboard, mouse, paste) go to the child process
                    // except intercepted bindings.
                    let should_exit = match self.poll_and_forward_raw_input() {
                        Ok(should_exit) => should_exit,
                        Err(err) => {
                            self.report_runtime_error(
                                "interactive input failed; staying in the current session",
                                err.as_ref(),
                            );
                            false
                        }
                    };
                    if should_exit {
                        break;
                    }
                } else {
                    // Normal UI mode: use crossterm's structured event reader.
                    // Block up to 100ms for the first event, then drain any
                    // remaining queued events before rendering so that
                    // intermediate events (Up, scroll, etc.) don't each cost
                    // a full render cycle.  This keeps double-click timestamps
                    // close to wall-clock time.
                    let ready = match crate::io_retry::retry_on_interrupt(|| {
                        event::poll(Duration::from_millis(100))
                    }) {
                        Ok(ready) => ready,
                        Err(err) => {
                            self.report_runtime_error(
                                "event polling failed; input handling was skipped",
                                &err,
                            );
                            false
                        }
                    };
                    if ready {
                        let mut should_exit = false;
                        loop {
                            let event = match crate::io_retry::retry_on_interrupt(event::read) {
                                Ok(event) => event,
                                Err(err) => {
                                    self.report_runtime_error(
                                        "event read failed; input handling was skipped",
                                        &err,
                                    );
                                    break;
                                }
                            };
                            match event {
                                Event::Key(key) => {
                                    should_exit = match self.handle_key(key) {
                                        Ok(exit) => exit,
                                        Err(err) => {
                                            self.report_runtime_error(
                                                "key handling failed",
                                                err.as_ref(),
                                            );
                                            false
                                        }
                                    };
                                }
                                Event::Mouse(mouse) => {
                                    should_exit = self.handle_mouse(mouse);
                                }
                                Event::Resize(_, _) => {}
                                _ => {}
                            }

                            // Stop draining if exit was requested.
                            if should_exit {
                                break;
                            }

                            // Stop draining if we switched to interactive
                            // mode — remaining events must go through the
                            // raw stdin path.
                            if matches!(
                                self.input_target,
                                InputTarget::Agent | InputTarget::Terminal
                            ) {
                                break;
                            }

                            // Check for more queued events without blocking.
                            match crate::io_retry::retry_on_interrupt(|| {
                                event::poll(Duration::ZERO)
                            }) {
                                Ok(true) => continue,
                                _ => break,
                            }
                        }
                        if should_exit {
                            break;
                        }
                    }
                }
            }
            Ok(())
        };

        let _ = execute!(stdout(), DisableMouseCapture);
        ratatui::restore();
        result
    }

    fn restore_sessions(&mut self) {
        logger::info(&format!(
            "restoring {} persisted sessions",
            self.sessions.len()
        ));
        let ids: Vec<(String, bool)> = self
            .sessions
            .iter()
            .map(|s| (s.id.clone(), Path::new(&s.worktree_path).exists()))
            .collect();
        for (id, exists) in ids {
            if exists {
                self.mark_session_status(&id, SessionStatus::Detached);
            } else {
                self.mark_session_status(&id, SessionStatus::Exited);
            }
        }
    }

    /// Populate the in-memory PR status map from the database so the UI shows
    /// PR state immediately on startup, before the first background poll.
    fn seed_pr_statuses_from_db(&mut self) {
        if !self.github_integration_enabled {
            return;
        }
        let stored = self.session_store.load_all_latest_prs().unwrap_or_default();
        for pr in stored {
            use crate::model::{PrInfo, PrState};
            let state = match pr.state.as_str() {
                "OPEN" => PrState::Open,
                "MERGED" => PrState::Merged,
                "CLOSED" => PrState::Closed,
                _ => continue,
            };
            self.pr_statuses.insert(
                pr.session_id,
                PrInfo {
                    number: pr.pr_number,
                    state,
                    title: pr.title,
                    owner_repo: pr.owner_repo,
                },
            );
        }
        if !self.pr_statuses.is_empty() {
            logger::info(&format!(
                "[gh-integration] seeded {} PR statuses from database",
                self.pr_statuses.len(),
            ));
        }
    }

    pub(crate) fn close_top_overlay(&mut self) -> bool {
        if matches!(self.fullscreen_overlay, FullscreenOverlay::Terminal) {
            let return_to_list = self.terminal_return_to_list;
            self.fullscreen_overlay = FullscreenOverlay::None;
            self.session_surface = SessionSurface::Agent;
            self.input_target = InputTarget::None;
            if return_to_list {
                self.left_section = LeftSection::Terminals;
                self.clamp_terminal_cursor();
                self.focus = FocusPane::Left;
            }
            let key = self.bindings.label_for(Action::ToggleFullscreen);
            self.set_info(format!(
                "Closed fullscreen terminal. Press {key} to reopen."
            ));
            return true;
        }
        if !matches!(self.prompt, PromptState::None) {
            self.prompt = PromptState::None;
            self.set_info("Dismissed dialog. Resume your work in the current pane.");
            return true;
        }
        if self.help_scroll.is_some() {
            self.help_scroll = None;
            let key = self.bindings.label_for(Action::ToggleHelp);
            self.set_info(format!("Closed help overlay. Press {key} to reopen."));
            return true;
        }
        if matches!(self.center_mode, CenterMode::Diff { .. }) {
            self.center_mode = CenterMode::Agent;
            self.focus = FocusPane::Files;
            self.set_info("Closed diff view, returned to agent output.");
            return true;
        }
        false
    }

    /// Returns the current braille spinner frame index based on wall-clock
    /// time (80ms per frame). Unlike `tick_count`, this stays constant-speed
    /// regardless of event loop frequency.
    pub(crate) fn spinner_frame_index(&self) -> usize {
        ((self.start_time.elapsed().as_millis() / 80) as usize) % crate::theme::SPINNER_FRAMES.len()
    }

    /// Poll each PTY provider for recent data and update the per-agent
    /// activity timestamp used by the left-pane streaming indicator.
    fn poll_pty_activity(&mut self) {
        let now = Instant::now();
        for (session_id, provider) in &self.providers {
            if provider.take_received_data() {
                self.last_pty_activity.insert(session_id.clone(), now);
            }
        }
    }

    /// Returns `true` if the given agent received PTY data within the last
    /// second, indicating it is actively streaming output.
    pub(crate) fn is_agent_streaming(&self, session_id: &str) -> bool {
        self.last_pty_activity
            .get(session_id)
            .is_some_and(|t| t.elapsed() < Duration::from_secs(1))
    }

    pub(crate) fn set_info(&mut self, message: impl Into<String>) {
        self.status.info(message);
    }

    pub(crate) fn set_busy(&mut self, message: impl Into<String>) {
        self.status.busy(message);
    }

    pub(crate) fn set_warning(&mut self, message: impl Into<String>) {
        self.status.warning(message);
    }

    pub(crate) fn set_error(&mut self, message: impl Into<String>) {
        self.status.error(message);
    }

    const NUDGE_DURATION_TICKS: u64 = 15; // ~1.5s at 100ms/tick

    pub(crate) fn is_nudge_active(&self) -> bool {
        self.readonly_nudge_tick
            .is_some_and(|t| self.tick_count.wrapping_sub(t) < Self::NUDGE_DURATION_TICKS)
    }

    fn report_runtime_error(&mut self, context: &str, err: &dyn std::error::Error) {
        logger::error(&format!("{context}: {err}"));
        self.set_error(format!("{context}: {err}"));
    }

    pub(crate) fn open_resource_monitor(&mut self) {
        self.prompt = PromptState::ResourceMonitor {
            rows: Vec::new(),
            scroll_offset: 0,
            last_refresh: Instant::now(),
            first_sample: true,
        };
        self.spawn_resource_stats_worker();
    }

    /// Gather the labeled PIDs that the resource monitor should report on.
    /// Each entry is `(label, root_pid)` — the worker will aggregate the
    /// full process tree under each root.
    fn resource_monitor_targets(&self) -> Vec<(String, u32)> {
        let mut targets = Vec::new();
        for session in &self.sessions {
            if let Some(pty) = self.providers.get(&session.id)
                && let Some(pid) = pty.child_process_id()
            {
                let title = session.title.as_deref().unwrap_or(&session.branch_name);
                let provider = session.provider.as_str();
                targets.push((format!("Agent ({provider}): {title}"), pid));
            }
        }
        for terminal in self.companion_terminals.values() {
            if let Some(pid) = terminal.client.child_process_id() {
                let label = match &terminal.foreground_cmd {
                    Some(cmd) => format!("Terminal ({cmd}): {}", terminal.label),
                    None => format!("Terminal: {}", terminal.label),
                };
                targets.push((label, pid));
            }
        }
        targets
    }

    pub(crate) fn spawn_resource_stats_worker(&mut self) {
        if self.resource_stats_in_flight {
            return;
        }
        self.resource_stats_in_flight = true;
        let targets = self.resource_monitor_targets();
        let tx = self.worker_tx.clone();
        thread::spawn(move || {
            let rows = collect_resource_stats(targets);
            let _ = tx.send(WorkerEvent::ResourceStatsReady(rows));
        });
    }

    pub(crate) fn execute_command(&mut self, command: String) -> Result<()> {
        let command = command.trim();
        match command {
            "new-agent" => self.create_agent_for_selected_project(),
            "fork-agent" => self.fork_selected_session(),
            "provider" => self.cycle_selected_project_provider(),
            "pull-project" => self.refresh_selected_project(),
            "delete-project" => self.delete_selected_project(),
            "remove-project" => self.remove_selected_project(),
            "delete-agent" => self.confirm_delete_selected_session(),
            "rename-agent" => self.open_rename_session(),
            "kill-running" => self.open_kill_running(),
            "reconnect-agent" => self.reconnect_selected_session(),
            "show-agent" => self.activate_center_agent(),
            "show-terminal" => self.show_or_open_first_terminal(),
            "new-terminal" => self.new_companion_terminal(),
            "add-project" => self.open_project_browser(),
            "copy-path" => self.copy_selected_path(),
            "open-worktree" => self.open_selected_worktree_in_default_editor(),
            "open-worktree-with" => self.open_worktree_editor_picker(),
            "toggle-project" => {
                self.toggle_collapse_selected_project();
                Ok(())
            }
            "toggle-sidebar" => {
                self.left_collapsed = !self.left_collapsed;
                Ok(())
            }
            "toggle-git-pane" => {
                self.right_collapsed = !self.right_collapsed;
                if self.right_collapsed && self.focus == FocusPane::Files {
                    self.focus = FocusPane::Center;
                }
                Ok(())
            }
            "toggle-remove-git-pane" => {
                self.right_hidden = !self.right_hidden;
                if self.right_hidden && self.focus == FocusPane::Files {
                    self.focus = FocusPane::Center;
                }
                Ok(())
            }
            "help" => {
                self.help_scroll = Some(0);
                Ok(())
            }
            "sort-agents-by-updated" => {
                self.sort_sessions_by_updated();
                Ok(())
            }
            "sort-agents-by-created" => {
                self.sort_sessions_by_created();
                Ok(())
            }
            "sort-agents-by-name" => {
                self.sort_sessions_by_name();
                Ok(())
            }
            "edit-macros" => {
                self.open_edit_macros();
                Ok(())
            }
            "input-debugging" => {
                self.prompt = PromptState::DebugInput {
                    lines: Vec::new(),
                    scroll_offset: 0,
                };
                Ok(())
            }
            "resource-monitor" => {
                self.open_resource_monitor();
                Ok(())
            }
            "toggle-diff-line-numbers" => {
                self.show_diff_line_numbers = !self.show_diff_line_numbers;
                self.config.ui.show_diff_line_numbers = self.show_diff_line_numbers;
                let _ = save_config(&self.paths.config_path, &self.config, &self.bindings);
                let _ = self.refresh_current_diff();
                let state = if self.show_diff_line_numbers {
                    "enabled"
                } else {
                    "disabled"
                };
                let palette_key = self.bindings.label_for(Action::OpenPalette);
                self.set_info(format!(
                    "Diff line numbers {state}. Press {palette_key} to open the palette and toggle back."
                ));
                Ok(())
            }
            "toggle-github-integration" => {
                self.github_integration_enabled = !self.github_integration_enabled;
                self.config.ui.github_integration = self.github_integration_enabled;
                let _ = save_config(&self.paths.config_path, &self.config, &self.bindings);
                if self.github_integration_enabled
                    && matches!(self.gh_status, crate::model::GhStatus::Available)
                {
                    self.update_pr_sync_sessions();
                    self.spawn_initial_pr_refresh();
                    self.pr_sync_enabled.store(true, Ordering::Relaxed);
                } else if !self.github_integration_enabled {
                    self.pr_statuses.clear();
                    self.pr_sync_enabled.store(false, Ordering::Relaxed);
                    self.rebuild_left_items();
                }
                let state = if self.github_integration_enabled {
                    "enabled"
                } else {
                    "disabled"
                };
                self.set_info(format!("GitHub integration {state}."));
                Ok(())
            }
            "toggle-prompt-for-name" => {
                self.config.defaults.prompt_for_name = !self.config.defaults.prompt_for_name;
                let _ = save_config(&self.paths.config_path, &self.config, &self.bindings);
                let state = if self.config.defaults.prompt_for_name {
                    "enabled — you'll be prompted for a name"
                } else {
                    "disabled — random names will be generated"
                };
                let palette_key = self.bindings.label_for(Action::OpenPalette);
                self.set_info(format!(
                    "Prompt for agent name {state}. Press {palette_key} to toggle back."
                ));
                Ok(())
            }
            "toggle-pr-banner-position" => {
                self.pr_banner_at_bottom = !self.pr_banner_at_bottom;
                let pos = if self.pr_banner_at_bottom {
                    "bottom"
                } else {
                    "top"
                };
                self.config.ui.pr_banner_position = pos.to_string();
                let _ = save_config(&self.paths.config_path, &self.config, &self.bindings);
                self.set_info(format!("PR banner moved to {pos} of agent pane."));
                Ok(())
            }
            "force-redraw" => {
                self.force_redraw = true;
                self.set_info("Interface redrawn. All screen contents have been repainted.");
                Ok(())
            }
            "" => Ok(()),
            other => {
                self.set_error(format!("Unknown command: \"{other}\""));
                Ok(())
            }
        }
    }

    pub(crate) fn open_edit_macros(&mut self) {
        let mut entries: Vec<(String, String, MacroSurface)> = self
            .config
            .macros
            .entries
            .iter()
            .map(|(k, v)| (k.clone(), v.text.clone(), v.surface))
            .collect();
        entries.sort_by(|(a, _, _), (b, _, _)| a.cmp(b));
        self.prompt = PromptState::EditMacros {
            entries,
            selected: 0,
            editing: None,
        };
    }

    /// Return macros matching `query` and the current session surface,
    /// searching name first then text content.
    /// If `query` is empty, returns all surface-matching macros in config order.
    pub(crate) fn filtered_macros(&self, query: &str) -> Vec<(&str, &str)> {
        let surface = self.session_surface;
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return self
                .config
                .macros
                .entries
                .iter()
                .filter(|(_, entry)| entry.surface.matches(surface))
                .map(|(name, entry)| (name.as_str(), entry.text.as_str()))
                .collect();
        }
        let mut name_matches = Vec::new();
        let mut text_matches = Vec::new();
        for (name, entry) in &self.config.macros.entries {
            if !entry.surface.matches(surface) {
                continue;
            }
            if name.to_lowercase().contains(&needle) {
                name_matches.push((name.as_str(), entry.text.as_str()));
            } else if entry.text.to_lowercase().contains(&needle) {
                text_matches.push((name.as_str(), entry.text.as_str()));
            }
        }
        name_matches.extend(text_matches);
        name_matches
    }

    pub(crate) fn left_items(&self) -> &[LeftItem] {
        &self.left_items_cache
    }

    pub(crate) fn rebuild_left_items(&mut self) {
        let mut items = Vec::new();
        for (project_index, project) in self.projects.iter().enumerate() {
            items.push(LeftItem::Project(project_index));
            if self.collapsed_projects.contains(&project.id) {
                continue;
            }
            for (session_index, session) in self.sessions.iter().enumerate() {
                if session.project_id == project.id {
                    items.push(LeftItem::Session(session_index));
                }
            }
        }
        self.left_items_cache = items;
    }

    pub(crate) fn sort_sessions_by_updated(&mut self) {
        self.sessions
            .sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        self.rebuild_left_items();
        self.set_info("Agents sorted by most recently updated.");
    }

    pub(crate) fn sort_sessions_by_created(&mut self) {
        self.sessions
            .sort_by(|a, b| b.created_at.cmp(&a.created_at));
        self.rebuild_left_items();
        self.set_info("Agents sorted by creation date (newest first).");
    }

    pub(crate) fn sort_sessions_by_name(&mut self) {
        self.sessions.sort_by(|a, b| {
            let name_a = a.title.as_deref().unwrap_or(&a.branch_name);
            let name_b = b.title.as_deref().unwrap_or(&b.branch_name);
            name_a.to_lowercase().cmp(&name_b.to_lowercase())
        });
        self.rebuild_left_items();
        self.set_info("Agents sorted alphabetically by name.");
    }

    pub(crate) fn toggle_collapse_selected_project(&mut self) {
        if let Some(project) = self.selected_project() {
            let id = project.id.clone();
            let has_sessions = self.sessions.iter().any(|s| s.project_id == id);
            if !has_sessions {
                return;
            }
            if self.collapsed_projects.contains(&id) {
                self.collapsed_projects.remove(&id);
            } else {
                self.collapsed_projects.insert(id.clone());
            }
            self.rebuild_left_items();

            // Move selection to the toggled project so collapsing from a
            // child session leaves the cursor on the parent header.
            if let Some(new_index) = self.left_items().iter().position(
                |item| matches!(item, LeftItem::Project(pi) if self.projects[*pi].id == id),
            ) {
                self.selected_left = new_index;
            }
        }
    }

    pub(crate) fn selected_project(&self) -> Option<&Project> {
        match self.left_items().get(self.selected_left) {
            Some(LeftItem::Project(index)) => self.projects.get(*index),
            Some(LeftItem::Session(index)) => self.sessions.get(*index).and_then(|session| {
                self.projects
                    .iter()
                    .find(|project| project.id == session.project_id)
            }),
            None => None,
        }
    }

    pub(crate) fn selected_session(&self) -> Option<&AgentSession> {
        match self.left_items().get(self.selected_left) {
            Some(LeftItem::Session(index)) => self.sessions.get(*index),
            _ => None,
        }
    }

    pub(crate) fn project_name_for_session(&self, session: &AgentSession) -> String {
        self.projects
            .iter()
            .find(|p| p.id == session.project_id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "unknown".to_string())
    }

    pub(crate) fn reload_changed_files(&mut self) {
        let session_id = self.selected_session().map(|s| s.id.clone());
        let worktree = self
            .selected_session()
            .map(|s| PathBuf::from(&s.worktree_path));
        // Keep the background poller in sync with the currently selected session.
        if let Ok(mut guard) = self.watched_worktree.lock() {
            *guard = worktree.clone();
        }
        let (staged, unstaged) = worktree
            .and_then(|p| git::changed_files(&p).ok())
            .unwrap_or_default();
        self.staged_files = staged;
        self.unstaged_files = unstaged;
        self.clamp_files_cursor();
        // Opportunistically check PR status for the newly-selected session.
        if let Some(sid) = session_id {
            self.spawn_pr_check_for_session(&sid);
        }
    }

    pub(crate) fn selected_changed_file(&self) -> Option<&ChangedFile> {
        match self.right_section {
            RightSection::Staged => self.staged_files.get(self.files_index),
            RightSection::Unstaged => self.unstaged_files.get(self.files_index),
            RightSection::CommitInput => None,
        }
    }

    pub(crate) fn current_files_len(&self) -> usize {
        match self.right_section {
            RightSection::Staged => self.staged_files.len(),
            RightSection::Unstaged => self.unstaged_files.len(),
            RightSection::CommitInput => 0,
        }
    }

    pub(crate) fn clamp_files_cursor(&mut self) {
        if self.right_section == RightSection::CommitInput {
            return;
        }
        let len = self.current_files_len();
        if len == 0 {
            self.files_index = 0;
        } else if self.files_index >= len {
            self.files_index = len.saturating_sub(1);
        }
    }

    pub(crate) fn has_files_search(&self) -> bool {
        !self.files_search.is_empty()
    }

    pub(crate) fn clear_files_search(&mut self) {
        self.files_search.clear();
        self.files_search_active = false;
    }

    pub(crate) fn update_files_search(&mut self, query: String) -> bool {
        self.files_search.set_text(query);
        if self.files_search.is_empty() {
            return false;
        }
        self.select_files_search_match(0)
    }

    pub(crate) fn advance_files_search_match(&mut self) -> bool {
        let matches = self.files_search_matches();
        if matches.is_empty() {
            return false;
        }

        let current = (self.right_section, self.files_index);
        let next_index = matches
            .iter()
            .position(|candidate| *candidate == current)
            .map(|index| (index + 1) % matches.len())
            .unwrap_or(0);

        self.apply_files_match(matches[next_index]);
        true
    }

    fn select_files_search_match(&mut self, match_index: usize) -> bool {
        let matches = self.files_search_matches();
        if matches.is_empty() {
            return false;
        }

        let target = matches[match_index.min(matches.len().saturating_sub(1))];
        self.apply_files_match(target);
        true
    }

    fn apply_files_match(&mut self, target: (RightSection, usize)) {
        self.right_section = target.0;
        self.files_index = target.1;
        self.clamp_files_cursor();
    }

    fn files_search_matches(&self) -> Vec<(RightSection, usize)> {
        if self.files_search.is_empty() {
            return Vec::new();
        }

        let needle = self.files_search.text.to_lowercase();
        let mut matches = Vec::new();
        matches.extend(
            self.unstaged_files
                .iter()
                .enumerate()
                .filter(|(_, file)| file.path.to_lowercase().contains(&needle))
                .map(|(index, _)| (RightSection::Unstaged, index)),
        );
        matches.extend(
            self.staged_files
                .iter()
                .enumerate()
                .filter(|(_, file)| file.path.to_lowercase().contains(&needle))
                .map(|(index, _)| (RightSection::Staged, index)),
        );
        matches
    }

    pub(crate) fn open_rename_session(&mut self) -> Result<()> {
        if let Some(session) = self.selected_session().cloned() {
            let current_name = session.title.unwrap_or_else(|| session.branch_name.clone());
            self.input_target = InputTarget::None;
            self.fullscreen_overlay = FullscreenOverlay::None;
            self.prompt = PromptState::RenameSession {
                session_id: session.id,
                input: TextInput::with_text(current_name)
                    .with_char_map(crate::git::agent_name_char_map),
                rename_branch: true,
            };
        } else {
            self.set_error("No agent session selected.");
        }
        Ok(())
    }

    pub(crate) fn apply_rename_session(
        &mut self,
        session_id: &str,
        new_name: String,
        rename_branch: bool,
    ) {
        let name = new_name.trim().to_string();
        if name.is_empty() {
            self.set_error("Name cannot be empty.");
            return;
        }
        if !git::is_valid_agent_name(&name) {
            self.set_error(
                "Agent name may only contain letters, digits, dashes, underscores, or slashes. \
                 It cannot start with \"-\" or \"/\", end with \"/\", or contain \"//\".",
            );
            return;
        }

        // Capture the previous title before mutating, in case we need to
        // revert on a failed branch rename.
        let previous_title = self
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .and_then(|s| s.title.clone());

        // Always update the display title immediately.
        if let Some(session) = self.sessions.iter_mut().find(|s| s.id == session_id) {
            session.title = Some(name.clone());
            session.updated_at = Utc::now();
        }
        if let Some(session) = self.sessions.iter().find(|s| s.id == session_id) {
            let _ = self.session_store.upsert_session(session);
        }
        self.rebuild_left_items();

        // Optionally rename the git branch in a background worker.
        if rename_branch {
            let Some(session) = self.sessions.iter().find(|s| s.id == session_id) else {
                return;
            };
            let old_branch = session.branch_name.clone();
            if name == old_branch {
                self.set_info(format!("Renamed agent to \"{name}\"."));
                return;
            }
            let worktree = session.worktree_path.clone();
            let sid = session.id.clone();
            let new_branch = name.clone();
            let tx = self.worker_tx.clone();
            std::thread::spawn(move || {
                let result = git::rename_branch(Path::new(&worktree), &old_branch, &new_branch)
                    .map_err(|e| e.to_string());
                let _ = tx.send(WorkerEvent::BranchRenameCompleted {
                    session_id: sid,
                    new_branch,
                    previous_title,
                    result,
                });
            });
            self.set_busy(format!("Renaming branch to \"{name}\"\u{2026}"));
        } else {
            self.set_info(format!("Renamed agent to \"{name}\"."));
            self.update_branch_sync_sessions();
        }
    }

    pub(crate) fn mark_session_status(&mut self, session_id: &str, status: SessionStatus) {
        if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|candidate| candidate.id == session_id)
        {
            if session.status == status {
                return;
            }
            session.status = status;
            session.updated_at = Utc::now();
            let _ = self.session_store.upsert_session(session);
        }
    }

    /// Refreshes the shared session snapshot used by the branch-sync worker.
    pub(crate) fn update_branch_sync_sessions(&self) {
        if let Ok(mut guard) = self.branch_sync_sessions.lock() {
            *guard = self
                .sessions
                .iter()
                .map(|s| BranchSyncEntry {
                    session_id: s.id.clone(),
                    worktree_path: s.worktree_path.clone(),
                    branch_name: s.branch_name.clone(),
                })
                .collect();
        }
    }

    /// Derives a companion terminal status for a session from the multi-terminal map.
    /// Running if any terminal exists for this session, NotLaunched otherwise.
    pub(crate) fn companion_terminal_status(&self, session_id: &str) -> CompanionTerminalStatus {
        if self.session_terminal_count(session_id) > 0 {
            CompanionTerminalStatus::Running
        } else {
            CompanionTerminalStatus::NotLaunched
        }
    }

    pub(crate) fn selected_companion_terminal_status(&self) -> CompanionTerminalStatus {
        self.selected_session()
            .map(|session| self.companion_terminal_status(&session.id))
            .unwrap_or(CompanionTerminalStatus::NotLaunched)
    }

    pub(crate) fn clear_companion_terminals_for_session(&mut self, session_id: &str) {
        self.companion_terminals
            .retain(|_, t| t.session_id != session_id);
        if let Some(ref id) = self.active_terminal_id
            && !self.companion_terminals.contains_key(id)
        {
            self.active_terminal_id = None;
        }
    }

    pub(crate) fn running_process_count(&self) -> usize {
        self.providers.len() + self.companion_terminals.len()
    }

    pub(crate) fn running_companion_terminal_count(&self) -> usize {
        self.companion_terminals.len()
    }

    /// Returns all running companion terminals as (terminal_id, terminal) pairs,
    /// sorted by creation order (terminal_id encodes the counter).
    pub(crate) fn terminal_items(&self) -> Vec<(&String, &CompanionTerminal)> {
        let mut items: Vec<_> = self.companion_terminals.iter().collect();
        items.sort_by_key(|(id, _)| (*id).clone());
        items
    }

    pub(crate) fn has_terminal_items(&self) -> bool {
        !self.companion_terminals.is_empty()
    }

    pub(crate) fn clamp_terminal_cursor(&mut self) {
        let count = self.terminal_items().len();
        if count == 0 {
            self.selected_terminal_index = 0;
            if self.left_section == LeftSection::Terminals {
                self.left_section = LeftSection::Projects;
            }
        } else if self.selected_terminal_index >= count {
            self.selected_terminal_index = count.saturating_sub(1);
        }
    }

    pub(crate) fn next_terminal_id(&mut self) -> String {
        self.terminal_counter += 1;
        format!("term-{}", self.terminal_counter)
    }

    /// Returns the number of running companion terminals for a given session.
    pub(crate) fn session_terminal_count(&self, session_id: &str) -> usize {
        self.companion_terminals
            .values()
            .filter(|t| t.session_id == session_id)
            .count()
    }

    pub(crate) fn selected_terminal_surface_client(&self) -> Option<&PtyClient> {
        match self.session_surface {
            SessionSurface::Agent => {
                let session_id = self.selected_session()?.id.as_str();
                self.providers.get(session_id)
            }
            SessionSurface::Terminal => {
                let id = self.active_terminal_id.as_ref()?;
                self.companion_terminals.get(id).map(|t| &t.client)
            }
        }
    }

    /// Refresh `self.snapshot_buf` from the currently selected terminal
    /// surface, reusing the existing cell allocation. Returns `true` if a
    /// provider was found and the snapshot was updated.
    pub(crate) fn refresh_snapshot_buf(&mut self) -> bool {
        let (client_id, client): (String, Option<&PtyClient>) = match self.session_surface {
            SessionSurface::Agent => {
                let session_id = match self.selected_session() {
                    Some(s) => s.id.clone(),
                    None => return false,
                };
                let provider = self.providers.get(&session_id);
                (session_id, provider)
            }
            SessionSurface::Terminal => {
                let id = match self.active_terminal_id.as_ref() {
                    Some(id) => id.clone(),
                    None => return false,
                };
                let provider = self.companion_terminals.get(&id).map(|t| &t.client);
                (id, provider)
            }
        };
        if let Some(provider) = client {
            if self.last_snapshot_id.as_deref() != Some(&client_id) {
                provider.mark_dirty();
                self.last_snapshot_id = Some(client_id);
                self.terminal_selection = None;
            }
            provider.snapshot_into(&mut self.snapshot_buf);
            true
        } else {
            false
        }
    }
}

pub(crate) fn load_projects(config: &Config) -> Vec<Project> {
    let mut projects = Vec::new();
    for project in config.projects.iter() {
        let path = PathBuf::from(&project.path);
        if !path.exists() || !git::is_git_repo(&path) {
            continue;
        }
        let provider = project
            .default_provider
            .as_deref()
            .map(ProviderKind::from_str)
            .unwrap_or_else(|| config.default_provider());
        projects.push(Project {
            id: project.id.clone(),
            name: project.name.clone().unwrap_or_else(|| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("project")
                    .to_string()
            }),
            path: path.to_string_lossy().to_string(),
            default_provider: provider,
            current_branch: git::current_branch(&path).unwrap_or_else(|_| "main".to_string()),
        });
    }
    projects
}

// ── Resource monitor helpers ───────────────────────────────────────────────

/// Collect CPU and memory stats for dux itself plus each labeled target
/// process tree. Runs on a background thread — no `&self` needed.
fn collect_resource_stats(targets: Vec<(String, u32)>) -> Vec<ResourceStats> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

    let mut sys = System::new();
    let refresh_kind = ProcessRefreshKind::nothing().with_cpu().with_memory();
    sys.refresh_processes_specifics(ProcessesToUpdate::All, true, refresh_kind);

    let mut rows = Vec::new();

    // Row: dux itself.
    let self_pid = Pid::from_u32(std::process::id());
    if let Some(proc_info) = sys.process(self_pid) {
        rows.push(ResourceStats {
            label: "dux (this process)".into(),
            pid: Some(std::process::id()),
            cpu_percent: proc_info.cpu_usage(),
            rss_bytes: proc_info.memory(),
            process_count: 1,
        });
    }

    // Rows: each labeled target (agents and companion terminals).
    for (label, root_pid) in &targets {
        let (cpu, rss, count) = aggregate_tree(&sys, Pid::from_u32(*root_pid));
        rows.push(ResourceStats {
            label: label.clone(),
            pid: Some(*root_pid),
            cpu_percent: cpu,
            rss_bytes: rss,
            process_count: count,
        });
    }

    // Total row.
    let total_cpu: f32 = rows.iter().map(|r| r.cpu_percent).sum();
    let total_rss: u64 = rows.iter().map(|r| r.rss_bytes).sum();
    let total_procs: usize = rows.iter().map(|r| r.process_count).sum();
    rows.push(ResourceStats {
        label: "TOTAL".into(),
        pid: None,
        cpu_percent: total_cpu,
        rss_bytes: total_rss,
        process_count: total_procs,
    });

    rows
}

/// Check whether `pid` is a descendant (child, grandchild, ...) of `ancestor`
/// by walking up the process tree.
fn is_descendant_of(sys: &sysinfo::System, pid: sysinfo::Pid, ancestor: sysinfo::Pid) -> bool {
    use sysinfo::Pid;

    let mut current = pid;
    // Depth limit prevents infinite loops if the tree has a cycle (shouldn't
    // happen, but be defensive).
    for _ in 0..64 {
        if let Some(proc) = sys.process(current) {
            if let Some(parent) = proc.parent() {
                if parent == ancestor {
                    return true;
                }
                if parent == Pid::from_u32(0) {
                    return false;
                }
                current = parent;
            } else {
                return false;
            }
        } else {
            return false;
        }
    }
    false
}

/// Aggregate CPU% and RSS across a root PID and all its descendants.
fn aggregate_tree(sys: &sysinfo::System, root: sysinfo::Pid) -> (f32, u64, usize) {
    let mut cpu = 0.0f32;
    let mut rss = 0u64;
    let mut count = 0usize;
    for (pid, proc_info) in sys.processes() {
        if *pid == root || is_descendant_of(sys, *pid, root) {
            cpu += proc_info.cpu_usage();
            rss += proc_info.memory();
            count += 1;
        }
    }
    (cpu, rss, count)
}

pub(crate) fn provider_config(config: &Config, provider: &ProviderKind) -> ProviderCommandConfig {
    config
        .providers
        .get(provider.as_str())
        .cloned()
        .unwrap_or_else(|| ProviderCommandConfig {
            command: provider.as_str().to_string(),
            ..Default::default()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_process_is_descendant_of_pid_1() {
        use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

        let mut sys = System::new();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        let self_pid = Pid::from_u32(std::process::id());
        let init_pid = Pid::from_u32(1);
        assert!(
            is_descendant_of(&sys, self_pid, init_pid),
            "current process should be a descendant of PID 1"
        );
    }

    #[test]
    fn aggregate_tree_includes_self_process() {
        use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

        let mut sys = System::new();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_memory(),
        );
        let self_pid = Pid::from_u32(std::process::id());
        let (_cpu, rss, count) = aggregate_tree(&sys, self_pid);
        assert!(count >= 1, "should include at least the root process");
        assert!(rss > 0, "current process should have nonzero RSS");
    }

    #[test]
    fn is_descendant_of_returns_false_for_unrelated_pid() {
        use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};

        let mut sys = System::new();
        sys.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing(),
        );
        // PID 1 is not a descendant of the current process.
        let self_pid = Pid::from_u32(std::process::id());
        let init_pid = Pid::from_u32(1);
        assert!(!is_descendant_of(&sys, init_pid, self_pid));
    }

    #[test]
    fn terminal_selection_ordered_forward() {
        let sel = TerminalSelection {
            anchor: TermGridPos { row: 2, col: 5 },
            end: TermGridPos { row: 4, col: 10 },
            dragging: false,
        };
        let (start, end) = sel.ordered();
        assert_eq!(start, TermGridPos { row: 2, col: 5 });
        assert_eq!(end, TermGridPos { row: 4, col: 10 });
    }

    #[test]
    fn terminal_selection_ordered_reverse() {
        let sel = TerminalSelection {
            anchor: TermGridPos { row: 4, col: 10 },
            end: TermGridPos { row: 2, col: 5 },
            dragging: false,
        };
        let (start, end) = sel.ordered();
        assert_eq!(start, TermGridPos { row: 2, col: 5 });
        assert_eq!(end, TermGridPos { row: 4, col: 10 });
    }

    #[test]
    fn terminal_selection_ordered_same_row() {
        let sel = TerminalSelection {
            anchor: TermGridPos { row: 3, col: 15 },
            end: TermGridPos { row: 3, col: 5 },
            dragging: false,
        };
        let (start, end) = sel.ordered();
        assert_eq!(start, TermGridPos { row: 3, col: 5 });
        assert_eq!(end, TermGridPos { row: 3, col: 15 });
    }

    #[test]
    fn terminal_selection_contains_single_row() {
        let sel = TerminalSelection {
            anchor: TermGridPos { row: 3, col: 5 },
            end: TermGridPos { row: 3, col: 10 },
            dragging: false,
        };
        assert!(sel.contains(3, 5));
        assert!(sel.contains(3, 7));
        assert!(sel.contains(3, 10));
        assert!(!sel.contains(3, 4));
        assert!(!sel.contains(3, 11));
        assert!(!sel.contains(2, 7));
        assert!(!sel.contains(4, 7));
    }

    #[test]
    fn terminal_selection_contains_multi_row() {
        let sel = TerminalSelection {
            anchor: TermGridPos { row: 2, col: 10 },
            end: TermGridPos { row: 4, col: 5 },
            dragging: false,
        };
        // First row: from anchor col to end of line.
        assert!(sel.contains(2, 10));
        assert!(sel.contains(2, 50));
        assert!(!sel.contains(2, 9));
        // Middle row: fully selected.
        assert!(sel.contains(3, 0));
        assert!(sel.contains(3, 100));
        // Last row: from start of line to end col.
        assert!(sel.contains(4, 0));
        assert!(sel.contains(4, 5));
        assert!(!sel.contains(4, 6));
        // Outside rows.
        assert!(!sel.contains(1, 10));
        assert!(!sel.contains(5, 0));
    }

    #[test]
    fn terminal_selection_contains_reverse_anchor() {
        // Anchor after end — should still work via ordered().
        let sel = TerminalSelection {
            anchor: TermGridPos { row: 4, col: 5 },
            end: TermGridPos { row: 2, col: 10 },
            dragging: false,
        };
        assert!(sel.contains(2, 10));
        assert!(sel.contains(3, 0));
        assert!(sel.contains(4, 5));
        assert!(!sel.contains(2, 9));
        assert!(!sel.contains(4, 6));
    }
}
