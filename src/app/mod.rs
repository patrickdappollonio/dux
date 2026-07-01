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
    Config, DuxPaths, MacroSurface, ProviderCommandConfig, check_provider_available, ensure_config,
    save_config, validate_keys,
};
use crate::diff::SyntaxCache;
use crate::editor::DetectedEditor;
use crate::git;
use crate::keybindings::{
    Action, BindingScope, HintContext, InteractiveBytePatterns, RuntimeBindings,
};
use crate::lockfile::SingleInstanceLock;
use crate::logger;
use crate::model::{
    AgentSession, ChangedFile, CompanionTerminalStatus, Project, ProjectBranchStatus, ProviderKind,
    SessionStatus, SessionSurface,
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
    /// Single-line filter for the projects/agents list. Active state lives in
    /// `left_search_active`; an empty query while active shows the full list.
    pub(crate) left_search: TextInput,
    pub(crate) left_search_active: bool,
    /// Agent selected when search opened, used to restore the highlight if the
    /// query ends up matching nothing.
    pub(crate) left_search_origin_session: Option<String>,
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
    pub(crate) startup_log_viewer: Option<StartupLogViewer>,
    pub(crate) status: StatusLine,
    pub(crate) prompt: PromptState,
    pub(crate) input_target: InputTarget,
    pub(crate) session_surface: SessionSurface,
    pub(crate) clipboard: Clipboard,
    pub(crate) worker_tx: Sender<WorkerEvent>,
    pub(crate) worker_rx: Receiver<WorkerEvent>,
    pub(crate) providers: HashMap<String, PtyClient>,
    /// When a provider swap happens while the agent's PTY is still running,
    /// the currently-spawned provider is pinned here so UI labels keep
    /// showing what's actually running until the user exits and relaunches
    /// the agent. Cleared whenever the PTY is torn down.
    pub(crate) running_provider_pins: HashMap<String, ProviderKind>,
    pub(crate) companion_terminals: HashMap<String, CompanionTerminal>,
    pub(crate) active_terminal_id: Option<String>,
    pub(crate) terminal_return_to_list: bool,
    pub(crate) terminal_counter: usize,
    pub(crate) create_agent_in_flight: bool,
    pub(crate) agent_launches_in_flight: HashSet<String>,
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
    /// Tracks an in-flight modal-button press: which button received
    /// mouse-down and whether the cursor is still inside it. Set on
    /// `MouseEventKind::Down(Left)` over a button, updated on `Drag`,
    /// cleared on `Up` (firing the button's action only when the cursor
    /// is still inside) and on any keystroke or modal-close event.
    pub(crate) pressed_button: Option<components::PressedButton>,
    pub(crate) interactive_patterns: InteractiveBytePatterns,
    pub(crate) raw_input_parser: crate::raw_input::RawInputParser,
    pub(crate) raw_input_buf: Vec<u8>,
    /// Separate buffer for scanning ExitInteractive during the loading phase.
    /// Kept independent of `raw_input_buf` so that suppressed keystrokes
    /// cannot leak into the first post-loading `process_raw_input_bytes` call.
    pub(crate) loading_input_buf: Vec<u8>,
    /// True while processing bytes between bracket-paste markers
    /// (`ESC[200~` … `ESC[201~`). Inside a paste, intercept matching is
    /// skipped so pasted text doesn't trigger keybindings.
    pub(crate) in_bracket_paste: bool,
    pub(crate) macro_bar: Option<MacroBarState>,
    pub(crate) sigwinch_flag: Arc<AtomicBool>,
    pub(crate) force_redraw: bool,
    pub(crate) welcome_tip_index: usize,
    /// Whether the ASCII logo was rendered in the previous frame.
    pub(crate) welcome_logo_visible: bool,
    /// The left-pane selection index when the logo last rendered a tip.
    pub(crate) welcome_tip_selection: usize,
    /// When true, show the alternate (duck) logo instead of the text logo.
    pub(crate) welcome_logo_alt: bool,
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
    /// Session IDs spawned with resume args and the wall-clock time the resume
    /// attempt began. Used for one-shot fallbacks when resume exits quickly or
    /// hangs without rendering visible output.
    pub(crate) resume_fallback_candidates: HashMap<String, Instant>,
    /// Session IDs whose worktree is currently being removed by a background
    /// worker. Prevents duplicate delete requests from spawning a second
    /// worker while the first is still running; also drives the dimmed
    /// visual cue on the left pane row so the user can see the in-flight
    /// state.
    pub(crate) pending_deletions: HashSet<String>,
    /// Maps session IDs to the exact Busy message set by
    /// `begin_delete_session`. Used by the worker event handler to decide
    /// whether the current status-line content was set by this deletion (and
    /// should be cleared) or by an unrelated operation (and should be left
    /// alone). Cleared per-session when the worker event arrives.
    pub(crate) deletion_busy_messages: HashMap<String, String>,
    /// Cached syntax highlighting resources shared across diff computations.
    pub(crate) syntax_cache: SyntaxCache,
    /// Reusable snapshot buffer to avoid per-frame allocation of terminal cells.
    pub(crate) snapshot_buf: TerminalSnapshot,
    /// ID of the provider that last populated `snapshot_buf`, used to detect
    /// agent switches and force a snapshot rebuild.
    last_snapshot_id: Option<String>,
    /// Active text selection in the terminal viewport, if any.
    pub(crate) terminal_selection: Option<TerminalSelection>,
    /// Active text selection in the startup command log output pane, if any.
    pub(crate) startup_log_selection: Option<TerminalSelection>,
    /// Exclusive lock held for the lifetime of this `App` so only one dux
    /// instance runs against a given config directory. Released
    /// automatically on drop (including crashes), so there is nothing to
    /// clean up on exit.
    _single_instance_lock: SingleInstanceLock,
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
    StartupLog,
}

#[derive(Clone, Debug)]
pub(crate) enum CenterMode {
    Agent,
    Diff {
        lines: Arc<Vec<Line<'static>>>,
        scroll: u16,
        /// Display-column width of the gutter (0 when line numbers are off).
        gutter_width: usize,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChangeAgentProviderFocus {
    List,
    Cancel,
    Apply,
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
pub(crate) struct ChangeAgentProviderOption {
    pub(crate) provider: ProviderKind,
    /// True when this provider's config has `resume_args`. Providers
    /// without resume support (e.g. Copilot CLI) always start fresh.
    pub(crate) supports_resume: bool,
    /// True when `supports_resume` AND this provider has been launched on
    /// this worktree before, so the next launch will actually resume.
    pub(crate) resume_available: bool,
    pub(crate) is_current: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ChangeAgentProviderPrompt {
    pub(crate) session_id: String,
    pub(crate) session_label: String,
    pub(crate) worktree_path: String,
    pub(crate) options: Vec<ChangeAgentProviderOption>,
    pub(crate) selected: usize,
    pub(crate) focus: ChangeAgentProviderFocus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ChangeDefaultProviderFocus {
    List,
    Cancel,
    Apply,
}

#[derive(Clone, Debug)]
pub(crate) struct ChangeDefaultProviderOption {
    pub(crate) provider: ProviderKind,
    pub(crate) is_current: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ChangeDefaultProviderPrompt {
    pub(crate) current: ProviderKind,
    pub(crate) options: Vec<ChangeDefaultProviderOption>,
    pub(crate) selected: usize,
    pub(crate) focus: ChangeDefaultProviderFocus,
}

#[derive(Clone, Debug)]
pub(crate) struct ChangeProjectDefaultProviderOption {
    pub(crate) provider: Option<ProviderKind>,
    pub(crate) is_current: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ChangeProjectDefaultProviderPrompt {
    pub(crate) project_id: String,
    pub(crate) project_name: String,
    pub(crate) current: ProviderKind,
    pub(crate) global_default: ProviderKind,
    pub(crate) inherits_global_default: bool,
    pub(crate) options: Vec<ChangeProjectDefaultProviderOption>,
    pub(crate) selected: usize,
    pub(crate) focus: ChangeDefaultProviderFocus,
}

#[derive(Clone, Debug)]
pub(crate) struct ChangeThemePrompt {
    pub(crate) options: Vec<crate::theme::ThemeListing>,
    pub(crate) selected: usize,
    pub(crate) current: String,
}

#[derive(Clone, Debug)]
pub(crate) struct StartupCommandLogPrompt {
    pub(crate) scope_label: String,
    pub(crate) entries: Vec<crate::startup::StartupCommandLogEntry>,
    pub(crate) selected: usize,
    pub(crate) filter: TextInput,
    pub(crate) searching: bool,
    pub(crate) content: String,
    pub(crate) scroll_offset: u16,
}

#[derive(Clone, Debug)]
pub(crate) struct StartupLogViewer {
    pub(crate) scope_label: String,
    pub(crate) path: Option<PathBuf>,
    pub(crate) display_name: String,
    pub(crate) content: String,
    pub(crate) scroll_offset: u16,
    pub(crate) search: TextInput,
    pub(crate) searching: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ProjectWorktreeEntry {
    pub(crate) path: PathBuf,
    pub(crate) branch_name: String,
    pub(crate) is_managed_by_dux: bool,
    pub(crate) existing_session_id: Option<String>,
    pub(crate) is_external: bool,
    pub(crate) is_project_checkout: bool,
    pub(crate) is_selectable: bool,
}

impl ProjectWorktreeEntry {
    pub(crate) fn display_name(&self) -> String {
        self.path
            .file_name()
            .and_then(|part| part.to_str())
            .map(str::to_string)
            .unwrap_or_else(|| self.path.display().to_string())
    }
}

#[derive(Clone, Debug)]
pub(crate) enum ProjectWorktreeVisualRow {
    Header(&'static str),
    Empty(String),
    Entry(usize),
}

#[derive(Clone, Debug)]
pub(crate) struct PickProjectWorktreePrompt {
    pub(crate) project: Project,
    pub(crate) entries: Vec<ProjectWorktreeEntry>,
    pub(crate) loading: bool,
    pub(crate) selected: Option<usize>,
    pub(crate) error: Option<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct ConfirmKillRunningPrompt {
    pub(crate) previous: KillRunningPrompt,
    pub(crate) action: KillRunningAction,
    pub(crate) target_ids: Vec<RuntimeTargetId>,
    pub(crate) confirm_selected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConfigReloadFailedFocus {
    Close,
    Apply,
    Checkbox,
}

/// Which selectable element has focus in the Delete Agent confirmation modal.
/// Focus cycles through all three via Tab / arrow keys / h / l.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeleteAgentFocus {
    Cancel,
    Delete,
    Checkbox,
}

/// Which selectable element has focus in the Non-Default Branch confirmation
/// modal. `Checkbox` is only reachable when `BranchWarningKind::Known` — the
/// heuristic path has no checkbox to focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ConfirmNonDefaultBranchFocus {
    Cancel,
    Add,
    Checkbox,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NameNewAgentFocus {
    Input,
    RandomizedNameCheckbox,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PullRequestLookup {
    pub(crate) host: String,
    pub(crate) owner_repo: String,
    pub(crate) number: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct ResolvedPullRequest {
    pub(crate) project: Project,
    pub(crate) host: String,
    pub(crate) owner_repo: String,
    pub(crate) number: u64,
    pub(crate) title: String,
    pub(crate) state: String,
    pub(crate) head_ref_name: String,
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
    AddProjectFailed {
        message: String,
        return_prompt: Box<PromptState>,
    },
    ChangeAgentProvider(ChangeAgentProviderPrompt),
    ChangeDefaultProvider(ChangeDefaultProviderPrompt),
    ChangeProjectDefaultProvider(ChangeProjectDefaultProviderPrompt),
    ChangeTheme(ChangeThemePrompt),
    ConfigureStartupCommand {
        project_id: String,
        project_name: String,
        input: TextInput,
    },
    ConfigureProjectEnv {
        project_id: String,
        project_name: String,
        input: TextInput,
    },
    ConfigureGlobalEnv {
        project_name: String,
        input: TextInput,
    },
    #[allow(dead_code)]
    StartupCommandLogs(StartupCommandLogPrompt),
    PickProjectWorktree(PickProjectWorktreePrompt),
    KillRunning(KillRunningPrompt),
    ConfirmKillRunning(ConfirmKillRunningPrompt),
    ConfigReloadFailed {
        error: String,
        recover_old_config: bool,
        focus: ConfigReloadFailedFocus,
    },
    ConfirmDeleteAgent {
        session_id: String,
        branch_name: String,
        focus: DeleteAgentFocus,
        delete_worktree: bool,
        /// True when one or more other sessions share this worktree. In that
        /// case the worktree is always preserved regardless of the user's
        /// choice, so the checkbox is hidden and a note is shown instead.
        worktree_shared: bool,
    },
    ConfirmDeleteTerminal {
        terminal_id: String,
        terminal_label: String,
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
    PullRequestInput {
        project: Project,
        input: TextInput,
    },
    NameNewAgent {
        request: CreateAgentRequest,
        input: TextInput,
        randomize_name: bool,
        randomized_name: Option<String>,
        focus: NameNewAgentFocus,
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
        pending_delete: Option<PendingMacroDelete>,
    },
    ConfirmNonDefaultBranch {
        action: NonDefaultBranchAction,
        current_branch: String,
        kind: BranchWarningKind,
        focus: ConfirmNonDefaultBranchFocus,
        /// When true and `kind == Known`, dux runs `git switch
        /// <default_branch>` in the source repo before registering the project.
        /// Ignored for `BranchWarningKind::Heuristic` because we can't
        /// confidently identify the target.
        checkout_default: bool,
    },
    ConfirmUseExistingBranch {
        request: CreateAgentRequest,
        branch_name: String,
        location: crate::git::BranchLocation,
        confirm_selected: bool, // false = Cancel (default), true = Use Existing
    },
    DebugInput {
        lines: Vec<Line<'static>>,
        scroll_offset: u16,
    },
    ResourceMonitor {
        rows: Vec<ResourceStats>,
        scroll_offset: u16,
        selected_row: usize,
        expanded: HashSet<u32>,
        last_refresh: Instant,
        first_sample: bool,
    },
}

#[derive(Clone, Debug)]
pub(crate) enum BranchWarningKind {
    /// We resolved `origin/HEAD` and know the default branch for certain.
    Known { default_branch: String },
    /// `origin/HEAD` unavailable; current branch is not `main` or `master`.
    Heuristic,
}

pub(crate) fn branch_warning_kind(path: &Path, branch: &str) -> Option<BranchWarningKind> {
    match git::remote_default_branch(path) {
        Some(default) if default != branch => Some(BranchWarningKind::Known {
            default_branch: default,
        }),
        Some(_) => None,
        None if branch != "main" && branch != "master" => Some(BranchWarningKind::Heuristic),
        None => None,
    }
}

pub(crate) fn branch_status_from_warning(
    warning_kind: Option<&BranchWarningKind>,
) -> ProjectBranchStatus {
    match warning_kind {
        Some(_) => ProjectBranchStatus::NotLeading,
        None => ProjectBranchStatus::Leading,
    }
}

pub(crate) fn leading_branch_for_project(path: &Path, current_branch: &str) -> String {
    match git::remote_default_branch(path) {
        Some(default) => default,
        None => current_branch.to_string(),
    }
}

#[derive(Clone, Debug)]
pub(crate) enum NonDefaultBranchAction {
    AddProject {
        path: String,
        name: String,
        leading_branch: String,
    },
    CheckoutProjectDefault {
        project: Project,
    },
}

impl NonDefaultBranchAction {
    pub(crate) fn repo_path(&self) -> &str {
        match self {
            Self::AddProject { path, .. } => path,
            Self::CheckoutProjectDefault { project } => &project.path,
        }
    }

    pub(crate) fn allows_add_anyway(&self) -> bool {
        matches!(self, Self::AddProject { .. })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CreateAgentBranchInspection {
    pub(crate) current_branch: String,
    pub(crate) leading_branch: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ProcessInfo {
    pub(crate) name: String,
    pub(crate) pid: u32,
    pub(crate) cpu_percent: f32,
    pub(crate) rss_bytes: u64,
}

#[derive(Clone, Debug)]
pub(crate) struct ResourceStats {
    pub(crate) label: String,
    pub(crate) pid: Option<u32>,
    pub(crate) cpu_percent: f32,
    pub(crate) rss_bytes: u64,
    pub(crate) process_count: usize,
    pub(crate) children: Vec<ProcessInfo>,
}

#[derive(Clone, Debug)]
pub(crate) enum VisualRow {
    /// Index into the `ResourceStats` rows vec.
    Parent(usize),
    /// (parent row index, child index within that parent's `children`).
    Child(usize, usize),
}

pub(crate) fn build_visual_rows(rows: &[ResourceStats], expanded: &HashSet<u32>) -> Vec<VisualRow> {
    let mut visual = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        visual.push(VisualRow::Parent(i));
        if let Some(pid) = row.pid
            && expanded.contains(&pid)
        {
            for (j, _) in row.children.iter().enumerate() {
                visual.push(VisualRow::Child(i, j));
            }
        }
    }
    visual
}

pub(crate) fn project_worktree_visual_rows(
    entries: &[ProjectWorktreeEntry],
    loading: bool,
    error: Option<&str>,
) -> Vec<ProjectWorktreeVisualRow> {
    if loading {
        return vec![ProjectWorktreeVisualRow::Empty(
            "Loading project worktrees...".to_string(),
        )];
    }
    if let Some(error) = error {
        return vec![ProjectWorktreeVisualRow::Empty(format!(
            "Could not load worktrees: {error}"
        ))];
    }

    let available = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.is_selectable)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let project_checkout = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.is_project_checkout)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    let disabled = entries
        .iter()
        .enumerate()
        .filter(|(_, entry)| !entry.is_selectable && !entry.is_project_checkout)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();

    let mut rows = Vec::new();
    rows.push(ProjectWorktreeVisualRow::Header("Available Worktrees"));
    if available.is_empty() {
        rows.push(ProjectWorktreeVisualRow::Empty(
            "No available worktrees. Worktrees that already have agents are shown below."
                .to_string(),
        ));
    } else {
        rows.extend(available.into_iter().map(ProjectWorktreeVisualRow::Entry));
    }
    if !disabled.is_empty() {
        rows.push(ProjectWorktreeVisualRow::Header("Already Has Agent"));
        rows.extend(disabled.into_iter().map(ProjectWorktreeVisualRow::Entry));
    }
    if !project_checkout.is_empty() {
        rows.push(ProjectWorktreeVisualRow::Header("Project Checkout"));
        rows.extend(
            project_checkout
                .into_iter()
                .map(ProjectWorktreeVisualRow::Entry),
        );
    }
    rows
}

pub(crate) fn selectable_project_worktree_indices(entries: &[ProjectWorktreeEntry]) -> Vec<usize> {
    entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| entry.is_selectable.then_some(index))
        .collect()
}

fn canonical_or_original(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

pub(crate) fn classify_project_worktrees(
    project: &Project,
    paths: &DuxPaths,
    sessions: &[AgentSession],
    worktrees: Vec<git::GitWorktree>,
) -> Vec<ProjectWorktreeEntry> {
    let managed_project_root = paths.worktrees_root.join(&project.name);
    let project_checkout_path = canonical_or_original(Path::new(&project.path));
    let session_by_path = sessions
        .iter()
        .map(|session| {
            (
                canonical_or_original(Path::new(&session.worktree_path)),
                session.id.clone(),
            )
        })
        .collect::<HashMap<_, _>>();

    let mut entries = worktrees
        .into_iter()
        .map(|worktree| {
            let canonical_path = canonical_or_original(&worktree.path);
            let existing_session_id = session_by_path.get(&canonical_path).cloned();
            let is_project_checkout = canonical_path == project_checkout_path;
            let is_managed_by_dux = git::is_under(&managed_project_root, &worktree.path);
            let is_external = !is_managed_by_dux;
            let is_selectable = existing_session_id.is_none() && !is_project_checkout;
            ProjectWorktreeEntry {
                path: canonical_path,
                branch_name: worktree.label(),
                is_managed_by_dux,
                existing_session_id,
                is_external,
                is_project_checkout,
                is_selectable,
            }
        })
        .collect::<Vec<_>>();

    entries.sort_by(|a, b| {
        a.is_selectable
            .cmp(&b.is_selectable)
            .reverse()
            .then_with(|| a.is_project_checkout.cmp(&b.is_project_checkout))
            .then_with(|| {
                a.branch_name
                    .to_lowercase()
                    .cmp(&b.branch_name.to_lowercase())
            })
            .then_with(|| a.path.cmp(&b.path))
    });
    entries
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
pub(crate) struct PendingMacroDelete {
    pub(crate) name: String,
    pub(crate) confirm_selected: bool,
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
    StartupCommand,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OverlayCheckboxId {
    DeleteAgentWorktree,
    RenameSessionBranch,
    NonDefaultBranchCheckoutDefault,
    NameNewAgentRandomizedPetName,
    ConfigReloadRecoverOldConfig,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OverlayCheckbox {
    pub(crate) id: OverlayCheckboxId,
    pub(crate) rect: Rect,
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
    AddProjectFailed {
        ok_button: Rect,
    },
    ChangeAgentProvider {
        list: Rect,
        items: usize,
        offset: usize,
        cancel_button: Rect,
        apply_button: Rect,
    },
    ChangeDefaultProvider {
        list: Rect,
        items: usize,
        offset: usize,
        cancel_button: Rect,
        apply_button: Rect,
    },
    ChangeProjectDefaultProvider {
        list: Rect,
        items: usize,
        offset: usize,
        cancel_button: Rect,
        apply_button: Rect,
    },
    PickEditor {
        list: Rect,
        items: usize,
        offset: usize,
    },
    PickProjectWorktree {
        list: Rect,
        items: usize,
        offset: usize,
    },
    ChangeTheme {
        list: Rect,
        items: usize,
        offset: usize,
    },
    ResourceMonitor {
        list: Rect,
        items: usize,
        offset: usize,
    },
    StartupCommandLogs {
        area: Rect,
        list: Rect,
        body: Rect,
        items: usize,
        offset: usize,
        close_button: Rect,
    },
    ConfigureStartupCommand {
        input: Rect,
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
        checkbox: Option<OverlayCheckbox>,
    },
    ConfirmDeleteTerminal {
        cancel_button: Rect,
        delete_button: Rect,
    },
    ConfirmDeleteMacro {
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
    ConfirmNonDefaultBranch {
        cancel_button: Rect,
        add_button: Rect,
        checkbox: Option<OverlayCheckbox>,
    },
    ConfirmUseExistingBranch {
        cancel_button: Rect,
        use_button: Rect,
    },
    ConfigReloadFailed {
        close_button: Rect,
        apply_button: Rect,
        checkbox: OverlayCheckbox,
    },
    RenameSession {
        input: Rect,
        checkbox: Option<OverlayCheckbox>,
    },
    NameNewAgent {
        input: Rect,
        checkbox: Option<OverlayCheckbox>,
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
    StartupCommandInput,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LeftItem {
    Project(usize),
    Session(usize),
    EmptyProjectsSpacer,
    EmptyProjectsSeparator,
}

impl LeftItem {
    pub(crate) fn is_selectable(self) -> bool {
        matches!(self, LeftItem::Project(_) | LeftItem::Session(_))
    }
}

/// Build the flattened left-pane item list. Module-private so the only
/// production entry point is [`App::rebuild_left_items`], which supplies the
/// active search query (`None` when search is inactive) and re-anchors the
/// selection — calling this directly with `None` while a search is active would
/// silently produce an unfiltered list.
fn build_left_items(
    projects: &[Project],
    sessions: &[AgentSession],
    collapsed_projects: &HashSet<String>,
    empty_project_separator_min_projects: u16,
    search: Option<&str>,
) -> Vec<LeftItem> {
    // When a search query is active, build a flat filtered view: project
    // headers followed by their matching agents, with non-matching and
    // agent-less projects dropped entirely. Collapse state and the
    // empty-project grouping are ignored so matches are always revealed.
    // Matching is case-insensitive; the needle is lowercased here so callers
    // do not have to.
    let search = search.map(str::trim).filter(|n| !n.is_empty());
    if let Some(needle) = search {
        let needle = needle.to_lowercase();
        let needle = needle.as_str();
        let mut items = Vec::new();
        for (project_index, project) in projects.iter().enumerate() {
            let project_matches = project.name.to_lowercase().contains(needle);
            let matching: Vec<usize> = sessions
                .iter()
                .enumerate()
                .filter(|(_, session)| session.project_id == project.id)
                .filter(|(_, session)| {
                    project_matches
                        || session
                            .title
                            .as_deref()
                            .is_some_and(|t| t.to_lowercase().contains(needle))
                        || session.branch_name.to_lowercase().contains(needle)
                })
                .map(|(session_index, _)| session_index)
                .collect();
            if matching.is_empty() {
                continue;
            }
            items.push(LeftItem::Project(project_index));
            for session_index in matching {
                items.push(LeftItem::Session(session_index));
            }
        }
        return items;
    }

    let split_empty_projects = empty_project_separator_min_projects > 0
        && projects.len() >= usize::from(empty_project_separator_min_projects);
    let mut items = Vec::new();
    let mut empty_projects = Vec::new();

    for (project_index, project) in projects.iter().enumerate() {
        let has_sessions = sessions
            .iter()
            .any(|session| session.project_id == project.id);
        if split_empty_projects && !has_sessions {
            empty_projects.push(project_index);
            continue;
        }
        push_project_left_items(
            &mut items,
            project_index,
            project,
            sessions,
            collapsed_projects,
        );
    }

    if !items.is_empty() && !empty_projects.is_empty() {
        items.push(LeftItem::EmptyProjectsSpacer);
        items.push(LeftItem::EmptyProjectsSeparator);
        for project_index in empty_projects {
            let project = &projects[project_index];
            push_project_left_items(
                &mut items,
                project_index,
                project,
                sessions,
                collapsed_projects,
            );
        }
    } else {
        for project_index in empty_projects {
            let project = &projects[project_index];
            push_project_left_items(
                &mut items,
                project_index,
                project,
                sessions,
                collapsed_projects,
            );
        }
    }

    items
}

fn push_project_left_items(
    items: &mut Vec<LeftItem>,
    project_index: usize,
    project: &Project,
    sessions: &[AgentSession],
    collapsed_projects: &HashSet<String>,
) {
    items.push(LeftItem::Project(project_index));
    if project.path_missing || collapsed_projects.contains(&project.id) {
        return;
    }
    for (session_index, session) in sessions.iter().enumerate() {
        if session.project_id == project.id {
            items.push(LeftItem::Session(session_index));
        }
    }
}

pub(crate) struct CompanionTerminal {
    pub(crate) session_id: String,
    pub(crate) label: String,
    pub(crate) foreground_cmd: Option<String>,
    pub(crate) client: PtyClient,
}

#[derive(Clone, Debug)]
pub(crate) enum AgentLaunchKind {
    Create {
        status_message: String,
        repo_path: String,
        owns_worktree: bool,
        startup_result: Option<crate::startup::StartupCommandResult>,
    },
    Reconnect {
        status_message: String,
    },
    ForceReconnect {
        status_message: String,
    },
    ResumeFallback {
        status_message: String,
    },
    StartupAutoReopen,
}

#[derive(Clone, Debug)]
pub(crate) struct AgentLaunchRequest {
    pub(crate) session: AgentSession,
    pub(crate) provider_config: ProviderCommandConfig,
    pub(crate) env: Vec<(String, String)>,
    pub(crate) resume: bool,
    pub(crate) pty_size: (u16, u16),
    pub(crate) scrollback_lines: usize,
    pub(crate) kind: AgentLaunchKind,
}

pub(crate) struct AgentLaunchReadyData {
    pub(crate) request: AgentLaunchRequest,
    pub(crate) client: PtyClient,
}

#[derive(Clone, Debug)]
pub(crate) struct AgentLaunchFailedData {
    pub(crate) request: AgentLaunchRequest,
    pub(crate) message: String,
}

#[derive(Clone, Debug)]
pub(crate) enum CreateAgentRequest {
    NewProject {
        project: Project,
        custom_name: Option<String>,
        use_existing_branch: bool,
        pull_before_create: bool,
    },
    PullRequest {
        project: Project,
        host: String,
        owner_repo: String,
        number: u64,
        title: String,
        state: String,
        head_branch: String,
        custom_name: Option<String>,
        use_existing_branch: bool,
    },
    ForkSession {
        project: Project,
        source_session: Box<AgentSession>,
        source_label: String,
        custom_name: Option<String>,
    },
    ExistingManagedWorktree {
        project: Project,
        worktree_path: PathBuf,
        branch_name: String,
        custom_name: Option<String>,
    },
    ForkExternalWorktree {
        project: Project,
        source_worktree_path: PathBuf,
        source_label: String,
        source_branch: String,
        custom_name: Option<String>,
    },
}

pub(crate) enum WorkerEvent {
    CreateAgentProgress(String),
    CreateAgentFailed(String),
    AgentLaunchReady(Box<AgentLaunchReadyData>),
    AgentLaunchFailed(Box<AgentLaunchFailedData>),
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
    ProjectWorktreesReady {
        project_id: String,
        result: Result<Vec<ProjectWorktreeEntry>, String>,
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
    PullRequestResolved {
        result: Result<ResolvedPullRequest, String>,
    },
    RefsChanged(String),
    /// Background `git worktree remove` for a session-initiated delete has
    /// finished. On `Ok`, the boolean indicates whether the branch was
    /// already gone (used for the status message). On `Err`, the message is
    /// the formatted error; the session record must be preserved so the user
    /// can retry.
    WorktreeRemoveCompleted {
        session_id: String,
        result: Result<bool, String>,
    },
    /// Background `git switch <target_branch>` run from a non-default branch
    /// warning modal has finished. On `Ok`, the main loop continues the
    /// original action. On `Err`, the formatted git error is surfaced.
    NonDefaultBranchCheckoutCompleted {
        action: NonDefaultBranchAction,
        target_branch: String,
        result: Result<(), String>,
    },
    /// Background inspection of the selected project checkout before opening
    /// the New Agent prompt.
    CreateAgentBranchInspected {
        project: Project,
        result: Result<CreateAgentBranchInspection, String>,
    },
    ProjectBranchStatusReady {
        project_id: String,
        result: Result<(String, ProjectBranchStatus), String>,
    },
    CheckoutProjectDefaultBranchInspected {
        project: Project,
        result: Result<(String, Option<BranchWarningKind>), String>,
    },
    ConfigReloadReady(Box<Result<Config, String>>),
    ConfigRecoverCompleted(Result<(), String>),
    ProjectPersistenceCompleted {
        action: ProjectPersistenceAction,
        result: Result<(), String>,
    },
    GlobalEnvPersistenceCompleted {
        env: std::collections::BTreeMap<String, String>,
        result: Result<(), String>,
    },
    StartupCommandRerunCompleted(crate::startup::StartupCommandResult),
    StartupCommandLogsLoaded {
        scope_label: String,
        result: Result<crate::startup::StartupCommandLatestLog, String>,
    },
    OpenPathCompleted {
        target: String,
        result: Result<(), String>,
    },
}

#[derive(Clone, Debug)]
pub(crate) enum PullTarget {
    Project {
        project_id: String,
        project_name: String,
        leading_branch: Option<String>,
    },
    Session,
}

#[derive(Clone, Debug)]
pub(crate) enum ProjectPersistenceAction {
    Add {
        project: Project,
        status_message: String,
    },
    Remove {
        project_id: String,
        project_name: String,
    },
    Delete {
        project_id: String,
        project_name: String,
    },
    UpdateDefaultProvider {
        project_id: String,
        project_name: String,
        provider: Option<ProviderKind>,
        global_default: ProviderKind,
    },
    UpdateAutoReopen {
        project_id: String,
        project_name: String,
        auto_reopen_agents: Option<bool>,
    },
    UpdateStartupCommand {
        project_id: String,
        project_name: String,
        startup_command: Option<String>,
    },
    UpdateEnv {
        project_id: String,
        project_name: String,
        env: std::collections::BTreeMap<String, String>,
    },
}

mod components;
mod input;
mod render;
mod sessions;
pub(crate) mod text_input;
mod workers;

impl App {
    /// Bootstrap the TUI. The caller must have already resolved `paths`,
    /// created its directories, and acquired the single-instance lock.
    /// This ensures the lock covers every entrypoint (TUI + config
    /// subcommands) and that a losing process never touches shared state.
    pub fn bootstrap_with_lock(
        paths: DuxPaths,
        single_instance_lock: SingleInstanceLock,
    ) -> Result<Self> {
        let mut config = ensure_config(&paths)?;

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
        sync_config_projects_with_store(&mut config, &paths, &bindings, &session_store)?;
        let projects = load_projects(&session_store.load_projects()?, &config);
        persist_runtime_projects_to_config_and_store(
            &projects,
            &mut config,
            &paths,
            &bindings,
            &session_store,
        )?;
        let sessions = session_store.load_sessions()?;
        let (worker_tx, worker_rx) = mpsc::channel();
        let watched_worktree: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
        let initial_status = format!(
            "Press {} to add a project, {} to create an agent, {} for help.",
            bindings.label_for(Action::OpenProjectBrowser),
            bindings.label_for(Action::NewAgent),
            bindings.label_for(Action::ToggleHelp),
        );
        let (theme, theme_warning) = crate::theme::load_or_fallback(&config.ui.theme, &paths);
        let mut status = StatusLine::new(initial_status);
        if let Some(message) = theme_warning {
            status.warning(message);
        }
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
            left_search: TextInput::new(),
            left_search_active: false,
            left_search_origin_session: None,
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
            startup_log_viewer: None,
            status,
            prompt: PromptState::None,
            input_target: InputTarget::None,
            session_surface: SessionSurface::Agent,
            clipboard: Clipboard::new(),
            worker_tx,
            worker_rx,
            providers: HashMap::new(),
            running_provider_pins: HashMap::new(),
            companion_terminals: HashMap::new(),
            active_terminal_id: None,
            terminal_return_to_list: false,
            terminal_counter: 0,
            create_agent_in_flight: false,
            agent_launches_in_flight: HashSet::new(),
            pulls_in_flight: HashSet::new(),
            resource_stats_in_flight: false,
            last_pty_size: (0, 0),
            last_pty_activity: HashMap::new(),
            prev_scrollback_offset: 0,
            last_diff_height: 0,
            last_diff_visual_lines: 0,
            theme,
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
            pressed_button: None,
            interactive_patterns,
            raw_input_parser: crate::raw_input::RawInputParser::default(),
            raw_input_buf: Vec::new(),
            loading_input_buf: Vec::new(),
            in_bracket_paste: false,
            macro_bar: None,
            sigwinch_flag,
            force_redraw: false,
            welcome_tip_index: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as usize)
                .unwrap_or(0),
            welcome_logo_visible: false,
            welcome_logo_alt: false,
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
            resume_fallback_candidates: HashMap::new(),
            pending_deletions: HashSet::new(),
            deletion_busy_messages: HashMap::new(),
            syntax_cache: SyntaxCache::new(),
            snapshot_buf: TerminalSnapshot::empty(),
            last_snapshot_id: None,
            terminal_selection: None,
            startup_log_selection: None,
            _single_instance_lock: single_instance_lock,
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
        self.spawn_project_branch_status_checks();
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

                if self.should_poll_raw_input() {
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

    fn should_poll_raw_input(&self) -> bool {
        matches!(self.prompt, PromptState::None)
            && !matches!(self.fullscreen_overlay, FullscreenOverlay::StartupLog)
            && matches!(
                self.input_target,
                InputTarget::Agent | InputTarget::Terminal
            )
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
        self.auto_reopen_eligible_sessions();
    }

    fn auto_reopen_eligible_sessions(&mut self) {
        if !self.config.ui.auto_reopen_agents {
            return;
        }

        let sessions = self.sessions.clone();
        for session in sessions {
            if !session.desired_running
                || !session.auto_reopen_enabled
                || !Path::new(&session.worktree_path).exists()
                || !self.project_allows_auto_reopen(&session.project_id)
            {
                continue;
            }

            let cfg = provider_config(&self.config, &session.provider);
            if !cfg.supports_session_resume() {
                continue;
            }

            let request =
                self.agent_launch_request(session, true, AgentLaunchKind::StartupAutoReopen);
            self.dispatch_agent_launch(request);
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
                    host: pr.host,
                    owner_repo: pr.owner_repo,
                    url: pr.url,
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
        if matches!(self.fullscreen_overlay, FullscreenOverlay::StartupLog) {
            self.fullscreen_overlay = FullscreenOverlay::None;
            self.startup_log_viewer = None;
            self.terminal_selection = None;
            self.startup_log_selection = None;
            self.set_info("Closed startup command log.");
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

    /// Closes the diff overlay if one is open, leaving other state (focus,
    /// input target, status line) untouched. Called when the left-pane
    /// selection moves to a different item so the middle pane falls back
    /// to the newly-selected agent's terminal. Silent by design: the user
    /// moved a cursor, they did not dismiss a dialog.
    pub(crate) fn close_diff_view(&mut self) {
        if matches!(self.center_mode, CenterMode::Diff { .. }) {
            self.center_mode = CenterMode::Agent;
        }
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

    /// Show a status-line warning when a missing project is highlighted, or
    /// clear the warning when the selection moves away from one.
    pub(crate) fn update_missing_project_warning(&mut self) {
        let missing_path = self
            .left_items()
            .get(self.selected_left)
            .copied()
            .and_then(|item| match item {
                LeftItem::Project(idx) => {
                    let p = self.projects.get(idx)?;
                    p.path_missing.then(|| p.path.clone())
                }
                _ => None,
            });
        if let Some(path) = missing_path {
            self.set_warning(format!("Project path not found: {path}"));
            return;
        }
        // Only clear if the current tone is Warning – don't clobber Info/Busy/Error.
        if matches!(self.status.tone(), crate::statusline::StatusTone::Warning) {
            self.set_info("");
        }
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
            selected_row: 0,
            expanded: HashSet::new(),
            last_refresh: Instant::now(),
            first_sample: true,
        };
        self.spawn_resource_stats_worker();
    }

    fn is_palette_action_available(&self, action: Action) -> bool {
        match action {
            Action::OpenCurrentPullRequest => self.current_pr_info().is_some(),
            _ => true,
        }
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

    pub(crate) fn github_pr_agent_command_available(&self) -> bool {
        self.github_integration_enabled
            && matches!(self.gh_status, crate::model::GhStatus::Available)
    }

    pub(crate) fn persist_config_projects_from_runtime(&mut self) -> Result<()> {
        let existing_projects = self.config.projects.clone();
        self.config.projects = self
            .projects
            .iter()
            .map(|project| runtime_project_to_config(project, &existing_projects))
            .collect();
        save_config(&self.paths.config_path, &self.config, &self.bindings)
    }

    pub(crate) fn persist_projects_to_config_and_store(&mut self) -> Result<()> {
        persist_runtime_projects_to_config_and_store(
            &self.projects,
            &mut self.config,
            &self.paths,
            &self.bindings,
            &self.session_store,
        )
    }

    pub(crate) fn filtered_palette_commands(
        &self,
        input: &str,
    ) -> Vec<&crate::keybindings::RuntimeBinding> {
        self.bindings
            .filtered_palette(input)
            .into_iter()
            .filter(|binding| {
                self.is_palette_action_available(binding.action)
                    && (binding.action != Action::NewAgentFromPr
                        || self.github_pr_agent_command_available())
            })
            .collect()
    }

    pub(crate) fn execute_command(&mut self, command: String) -> Result<()> {
        let command = command.trim();
        match command {
            "new-agent" => self.create_agent_for_selected_project(),
            "new-agent-from-pr" => self.open_new_agent_from_pr_prompt(),
            "new-agent-from-worktree" => self.create_agent_from_existing_worktree(),
            "fork-agent" => self.fork_selected_session(),
            "change-agent-provider" => self.open_change_agent_provider_prompt(),
            "change-default-provider" => self.open_change_default_provider_prompt(),
            "change-project-default-provider" => self.open_change_project_default_provider_prompt(),
            "change-theme" => self.open_change_theme_prompt(),
            "reload-config" => self.reload_config_from_disk(),
            "toggle-project-auto-reopen-agents" => self.toggle_project_auto_reopen_agents(),
            "toggle-agent-auto-reopen" => self.toggle_agent_auto_reopen(),
            "configure-startup-command" => self.open_configure_startup_command(),
            "configure-global-env" => self.open_configure_global_env(),
            "configure-project-env" => self.open_configure_project_env(),
            "rerun-startup-command-on-agent" => self.rerun_startup_command_on_agent(),
            "read-startup-command-logs" => self.open_startup_command_logs(),
            "pull-project" => self.refresh_selected_project(),
            "delete-project" => self.delete_selected_project(),
            "remove-project" => self.remove_selected_project(),
            "delete-agent" => self.confirm_delete_selected_session(),
            "rename-agent" => self.open_rename_session(),
            "kill-running" => self.open_kill_running(),
            "reconnect-agent" => self.reconnect_selected_session(),
            "force-reconnect-agent" => self.force_reconnect_agent(),
            "show-agent" => self.activate_center_agent(),
            "show-terminal" => self.show_or_open_first_terminal(),
            "new-terminal" => self.new_companion_terminal(),
            "add-project" => self.open_project_browser(),
            "copy-path" => self.copy_selected_path(),
            "open-worktree" => self.open_selected_worktree_in_default_editor(),
            "open-worktree-with" => self.open_worktree_editor_picker(),
            "open-current-pr" => self.open_current_pr_in_browser(),
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
                let save_result =
                    save_config(&self.paths.config_path, &self.config, &self.bindings);
                let _ = self.refresh_current_diff();
                let state = if self.show_diff_line_numbers {
                    "enabled"
                } else {
                    "disabled"
                };
                if let Err(err) = save_result {
                    self.set_error(format!(
                        "Diff line numbers {state} for this session, but couldn't persist the change to config: {err:#}"
                    ));
                } else {
                    let palette_key = self.bindings.label_for(Action::OpenPalette);
                    self.set_info(format!(
                        "Diff line numbers {state}. Press {palette_key} to open the palette and toggle back."
                    ));
                }
                Ok(())
            }
            "toggle-github-integration" => {
                self.github_integration_enabled = !self.github_integration_enabled;
                self.config.ui.github_integration = self.github_integration_enabled;
                let save_result =
                    save_config(&self.paths.config_path, &self.config, &self.bindings);
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
                if let Err(err) = save_result {
                    self.set_error(format!(
                        "GitHub integration {state} for this session, but couldn't persist the change to config: {err:#}"
                    ));
                } else {
                    self.set_info(format!("GitHub integration {state}."));
                }
                Ok(())
            }
            "toggle-randomized-pet-name-default" => {
                self.config.defaults.enable_randomized_pet_name_by_default =
                    !self.config.defaults.enable_randomized_pet_name_by_default;
                let save_result =
                    save_config(&self.paths.config_path, &self.config, &self.bindings);
                let state = if self.config.defaults.enable_randomized_pet_name_by_default {
                    "enabled — new agent prompts start with a random pet name"
                } else {
                    "disabled — new agent prompts start empty"
                };
                if let Err(err) = save_result {
                    self.set_error(format!(
                        "Random pet-name defaults {state} for this session, but couldn't persist the change to config: {err:#}"
                    ));
                } else {
                    let palette_key = self.bindings.label_for(Action::OpenPalette);
                    self.set_info(format!(
                        "Random pet-name defaults {state}. Press {palette_key} to toggle back."
                    ));
                }
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
                if let Err(err) = save_config(&self.paths.config_path, &self.config, &self.bindings)
                {
                    self.set_error(format!(
                        "PR banner moved to {pos} for this session, but couldn't persist the change to config: {err:#}"
                    ));
                } else {
                    self.set_info(format!("PR banner moved to {pos} of agent pane."));
                }
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

    pub(crate) fn reload_config_from_disk(&mut self) -> Result<()> {
        self.spawn_config_reload_worker();
        self.set_busy("Reloading config.toml.");
        Ok(())
    }

    fn open_config_reload_failed_modal(&mut self, error: String) {
        self.prompt = PromptState::ConfigReloadFailed {
            error,
            recover_old_config: false,
            focus: ConfigReloadFailedFocus::Close,
        };
    }

    fn apply_reloaded_config(&mut self, mut config: Config) -> Result<()> {
        let bindings = RuntimeBindings::from_keys_config(&config.keys);
        self.interactive_patterns = bindings.interactive_byte_patterns();
        self.bindings = bindings;

        let (theme, theme_warning) = crate::theme::load_or_fallback(&config.ui.theme, &self.paths);
        self.theme = theme;
        self.show_diff_line_numbers = config.ui.show_diff_line_numbers;
        self.left_width_pct = config.ui.left_width_pct;
        self.right_width_pct = config.ui.right_width_pct;
        self.terminal_pane_height_pct = config.ui.terminal_pane_height_pct;
        self.staged_pane_height_pct = config.ui.staged_pane_height_pct;
        self.commit_pane_height_pct = config.ui.commit_pane_height_pct;
        self.github_integration_enabled = config.ui.github_integration;
        self.pr_banner_at_bottom = config.ui.pr_banner_position == "bottom";
        self.projects = load_projects(&self.session_store.load_projects()?, &config);
        persist_runtime_projects_to_config_and_store(
            &self.projects,
            &mut config,
            &self.paths,
            &self.bindings,
            &self.session_store,
        )?;
        self.config = config;

        refresh_project_defaults(&mut self.projects, &self.config);
        self.selected_left = self
            .selected_left
            .min(self.projects.len().saturating_sub(1));
        self.rebuild_left_items();
        if self.selected_left >= self.left_items_cache.len() {
            self.selected_left = self.left_items_cache.len().saturating_sub(1);
        }
        self.update_branch_sync_sessions();
        if self.github_integration_enabled
            && matches!(self.gh_status, crate::model::GhStatus::Available)
        {
            self.update_pr_sync_sessions();
            self.spawn_initial_pr_refresh();
            self.pr_sync_enabled.store(true, Ordering::Relaxed);
        } else {
            self.pr_statuses.clear();
            self.pr_sync_enabled.store(false, Ordering::Relaxed);
        }
        self.reload_changed_files();
        self.refresh_current_diff()?;
        if let Some(message) = theme_warning {
            self.set_warning(message);
        }
        Ok(())
    }

    pub(crate) fn open_edit_macros(&mut self) {
        let entries: Vec<(String, String, MacroSurface)> = self
            .config
            .macros
            .entries
            .iter()
            .map(|(k, v)| (k.clone(), v.text.clone(), v.surface))
            .collect();
        // Preserve declaration order from config file (IndexMap iteration order).
        self.prompt = PromptState::EditMacros {
            entries,
            selected: 0,
            editing: None,
            pending_delete: None,
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
        // While searching, keep the highlight anchored to the same agent across
        // rebuilds (e.g. a background branch/PR refresh re-running the filter)
        // rather than letting `ensure_selectable_left_item` move it. The typing
        // path overrides this afterward via `select_first_left_result`.
        let anchor = self
            .left_search_active
            .then(|| self.selected_session().map(|s| s.id.clone()))
            .flatten();
        // `build_left_items` trims, lowercases, and ignores an empty query, so
        // the raw text is passed through when search is active.
        let search = self
            .left_search_active
            .then(|| self.left_search.text.clone());
        self.left_items_cache = build_left_items(
            &self.projects,
            &self.sessions,
            &self.collapsed_projects,
            self.config.ui.empty_project_separator_min_projects,
            search.as_deref(),
        );
        self.ensure_selectable_left_item();
        if let Some(id) = anchor
            && let Some(index) = self.left_items_cache.iter().position(|item| {
                matches!(item, LeftItem::Session(si) if self.sessions.get(*si).is_some_and(|s| s.id == id))
            })
        {
            self.selected_left = index;
        }
    }

    /// Whether keyboard or mouse navigation may land the cursor on `item`. Both
    /// input paths route selection through this predicate. It matches
    /// `LeftItem::is_selectable()`, except that while searching, project headers
    /// act as plain context so selection only moves between agent rows.
    /// (Deliberate, search-gated state transitions — e.g. re-selecting a project
    /// after toggling its collapse — set the cursor directly and are exempt.)
    pub(crate) fn left_item_is_nav_target(&self, item: LeftItem) -> bool {
        item.is_selectable() && !(self.left_search_active && matches!(item, LeftItem::Project(_)))
    }

    /// Drop any active in-pane search (agents and files) without restoring a
    /// prior selection. Used when a background event takes over the UI — e.g. an
    /// agent launch completes and focus moves to the new agent — so a stale
    /// filter or search box never lingers. Rebuilds the list when an agent
    /// search was active so it is no longer filtered.
    pub(crate) fn cancel_in_pane_searches(&mut self) {
        let was_searching = self.left_search_active;
        self.left_search_active = false;
        self.left_search.clear();
        self.left_search_origin_session = None;
        self.clear_files_search();
        if was_searching {
            self.rebuild_left_items();
        }
    }

    pub(crate) fn is_selectable_left_item(&self, index: usize) -> bool {
        self.left_items()
            .get(index)
            .is_some_and(|item| self.left_item_is_nav_target(*item))
    }

    pub(crate) fn next_selectable_left_item_after(&self, index: usize) -> Option<usize> {
        self.left_items()
            .iter()
            .enumerate()
            .skip(index.saturating_add(1))
            .find_map(|(idx, item)| self.left_item_is_nav_target(*item).then_some(idx))
    }

    pub(crate) fn previous_selectable_left_item_before(&self, index: usize) -> Option<usize> {
        self.left_items()
            .iter()
            .enumerate()
            .take(index)
            .rev()
            .find_map(|(idx, item)| self.left_item_is_nav_target(*item).then_some(idx))
    }

    pub(crate) fn ensure_selectable_left_item(&mut self) {
        if self.left_items_cache.is_empty() {
            self.selected_left = 0;
            return;
        }
        if self.selected_left >= self.left_items_cache.len() {
            self.selected_left = self.left_items_cache.len().saturating_sub(1);
        }
        if self.left_item_is_nav_target(self.left_items_cache[self.selected_left]) {
            return;
        }
        if let Some(next) = self.next_selectable_left_item_after(self.selected_left) {
            self.selected_left = next;
        } else if let Some(prev) = self.previous_selectable_left_item_before(self.selected_left) {
            self.selected_left = prev;
        }
    }

    pub(crate) fn sort_sessions_by_updated(&mut self) {
        self.sessions
            .sort_by_key(|b| std::cmp::Reverse(b.updated_at));
        self.rebuild_left_items();
        self.set_info("Agents sorted by most recently updated.");
    }

    pub(crate) fn sort_sessions_by_created(&mut self) {
        self.sessions
            .sort_by_key(|b| std::cmp::Reverse(b.created_at));
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
            Some(LeftItem::EmptyProjectsSpacer) => None,
            Some(LeftItem::EmptyProjectsSeparator) => None,
            None => None,
        }
    }

    pub(crate) fn project_explicit_default_provider(
        &self,
        project_id: &str,
    ) -> Option<ProviderKind> {
        self.projects
            .iter()
            .find(|project| project.id == project_id)
            .and_then(|project| project.explicit_default_provider.clone())
    }

    pub(crate) fn project_uses_explicit_default_provider(&self, project_id: &str) -> bool {
        self.project_explicit_default_provider(project_id).is_some()
    }

    pub(crate) fn project_allows_auto_reopen(&self, project_id: &str) -> bool {
        self.projects
            .iter()
            .find(|project| project.id == project_id)
            .and_then(|project| project.auto_reopen_agents)
            .unwrap_or(true)
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

    /// Provider currently driving the session's live PTY, if any. After an
    /// in-place provider swap while the agent is still running, this returns
    /// the *original* provider until the user exits and relaunches — so the
    /// pane title doesn't lie about what's actually on screen.
    pub(crate) fn running_provider_for(&self, session: &AgentSession) -> ProviderKind {
        self.running_provider_pins
            .get(&session.id)
            .cloned()
            .unwrap_or_else(|| session.provider.clone())
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

    pub(crate) fn mark_session_desired_running(&mut self, session_id: &str, desired: bool) {
        if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|candidate| candidate.id == session_id)
        {
            if session.desired_running == desired {
                return;
            }
            session.desired_running = desired;
            session.updated_at = Utc::now();
            let _ = self.session_store.upsert_session(session);
        } else {
            let _ = self.session_store.set_desired_running(session_id, desired);
        }
    }

    pub(crate) fn mark_session_provider_started(&mut self, session_id: &str) {
        let Some(session) = self
            .sessions
            .iter_mut()
            .find(|candidate| candidate.id == session_id)
        else {
            return;
        };

        let provider = session.provider.clone();
        if !session.mark_provider_started(&provider) {
            return;
        }

        session.updated_at = Utc::now();
        let _ = self.session_store.upsert_session(session);
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

/// Re-resolve the in-memory `default_provider` for each project against the
/// current config. Projects with an explicit `default_provider` keep their
/// override; projects without one pick up the new global default.
pub(crate) fn refresh_project_defaults(projects: &mut [Project], config: &Config) {
    let fallback = config.default_provider();
    for project in projects.iter_mut() {
        project.default_provider = project
            .explicit_default_provider
            .clone()
            .unwrap_or_else(|| fallback.clone());
    }
}

pub(crate) fn load_projects(
    project_configs: &[crate::config::ProjectConfig],
    config: &Config,
) -> Vec<Project> {
    let mut projects = Vec::new();
    for project in project_configs {
        let (path, missing) = match crate::config::expand_path(&project.path) {
            Some(expanded) => {
                let p = PathBuf::from(&expanded);
                let missing = !p.exists() || !git::is_git_repo(&p);
                (p, missing)
            }
            None => {
                // Unsafe or invalid path – treat as missing.
                (PathBuf::from(&project.path), true)
            }
        };
        let provider = project
            .default_provider
            .as_deref()
            .map(ProviderKind::from_str)
            .unwrap_or_else(|| config.default_provider());
        let current_branch = if missing {
            String::new()
        } else {
            git::current_branch(&path).unwrap_or_else(|_| "main".to_string())
        };
        let leading_branch = project
            .leading_branch
            .clone()
            .or_else(|| (!missing).then(|| leading_branch_for_project(&path, &current_branch)));
        projects.push(Project {
            id: project.id.clone(),
            name: project.name.clone().unwrap_or_else(|| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("project")
                    .to_string()
            }),
            path: path.to_string_lossy().to_string(),
            explicit_default_provider: project
                .default_provider
                .as_deref()
                .map(ProviderKind::from_str),
            default_provider: provider,
            leading_branch,
            auto_reopen_agents: project.auto_reopen_agents,
            startup_command: project.startup_command.clone(),
            env: project.env.clone(),
            current_branch,
            branch_status: ProjectBranchStatus::Unknown,
            path_missing: missing,
        });
    }
    projects
}

pub(crate) fn persist_runtime_projects_to_config_and_store(
    projects: &[Project],
    config: &mut Config,
    paths: &DuxPaths,
    bindings: &RuntimeBindings,
    session_store: &SessionStore,
) -> Result<()> {
    let existing_projects = config.projects.clone();
    let stored_project_configs = projects
        .iter()
        .map(|project| runtime_project_to_config(project, &existing_projects))
        .collect::<Vec<_>>();
    let config_project_configs = stored_project_configs
        .iter()
        .cloned()
        .map(|mut project| {
            project.leading_branch = None;
            project
        })
        .collect::<Vec<_>>();

    let stored_projects = session_store.load_projects()?;
    for (index, project_config) in stored_project_configs.iter().enumerate() {
        let stored_project = stored_projects.iter().find(|stored| {
            stored.id == project_config.id || same_expanded_project_path(stored, project_config)
        });
        if stored_project != Some(project_config) {
            session_store.upsert_project_at(project_config, index as i64)?;
        }
    }

    if config.projects != config_project_configs {
        config.projects = config_project_configs;
        save_config(&paths.config_path, config, bindings)?;
    }

    Ok(())
}

pub(crate) fn sync_config_projects_with_store(
    config: &mut Config,
    paths: &DuxPaths,
    bindings: &RuntimeBindings,
    session_store: &SessionStore,
) -> Result<()> {
    validate_project_records("config.toml", &config.projects)?;
    let mut stored = session_store.load_projects()?;
    validate_project_records("SQLite", &stored)?;

    let mut changed_config = false;
    let mut changed_store = false;
    let mut merged = config.projects.clone();

    for (index, cfg_project) in config.projects.iter().enumerate() {
        match stored.iter().position(|stored_project| {
            stored_project.id == cfg_project.id
                || same_expanded_project_path(stored_project, cfg_project)
        }) {
            Some(stored_index) => {
                let stored_project = &stored[stored_index];
                let (merged_config_project, merged_stored_project) =
                    merge_project_records(cfg_project, stored_project)?;
                if &merged_config_project != cfg_project {
                    merged[index] = merged_config_project;
                    changed_config = true;
                }
                if &merged_stored_project != stored_project {
                    session_store.upsert_project_at(&merged_stored_project, stored_index as i64)?;
                    stored[stored_index] = merged_stored_project;
                    changed_store = true;
                }
            }
            None => {
                session_store.upsert_project_at(cfg_project, index as i64)?;
                changed_store = true;
                stored.push(cfg_project.clone());
            }
        }
    }

    for stored_project in stored {
        let exists = merged.iter().any(|cfg_project| {
            cfg_project.id == stored_project.id
                || same_expanded_project_path(cfg_project, &stored_project)
        });
        if !exists {
            let mut portable = stored_project;
            portable.path = portable_project_path(&portable.path);
            merged.push(portable);
            changed_config = true;
        }
    }

    if changed_config {
        config.projects = merged;
        save_config(&paths.config_path, config, bindings)?;
    } else if changed_store {
        // Keep the on-disk config normalized when the database was repaired
        // from config-only projects.
        save_config(&paths.config_path, config, bindings)?;
    }
    Ok(())
}

fn validate_project_records(source: &str, projects: &[crate::config::ProjectConfig]) -> Result<()> {
    for (index, project) in projects.iter().enumerate() {
        for other in projects.iter().skip(index + 1) {
            if project.id == other.id {
                anyhow::bail!(
                    "Project sync conflict in {source}: duplicate project id \"{}\". Remove or rename one [[projects]] entry, then restart dux.",
                    project.id
                );
            }
            if same_expanded_project_path(project, other) {
                anyhow::bail!(
                    "Project sync conflict in {source}: duplicate project path \"{}\". Remove one duplicate project entry, then restart dux.",
                    expanded_project_path(project).unwrap_or_else(|| project.path.clone())
                );
            }
        }
    }
    Ok(())
}

fn merge_project_records(
    config_project: &crate::config::ProjectConfig,
    stored_project: &crate::config::ProjectConfig,
) -> Result<(crate::config::ProjectConfig, crate::config::ProjectConfig)> {
    let config_path = expanded_project_path(config_project);
    let stored_path = expanded_project_path(stored_project);
    if config_project.id == stored_project.id && config_path != stored_path {
        anyhow::bail!(
            "Project sync conflict for id \"{}\": config.toml points to \"{}\" but SQLite points to \"{}\". Edit config.toml or remove/re-add the project so both stores agree.",
            config_project.id,
            config_project.path,
            stored_project.path
        );
    }
    if config_path == stored_path && config_project.id != stored_project.id {
        anyhow::bail!(
            "Project sync conflict for path \"{}\": config.toml uses id \"{}\" but SQLite uses id \"{}\". Edit config.toml or remove/re-add the project so both stores agree.",
            config_path.unwrap_or_else(|| config_project.path.clone()),
            config_project.id,
            stored_project.id
        );
    }

    let mut merged_config = config_project.clone();
    let mut merged_stored = stored_project.clone();

    sync_config_authoritative_project_field(&mut merged_config.name, &mut merged_stored.name);
    sync_config_authoritative_project_field(
        &mut merged_config.default_provider,
        &mut merged_stored.default_provider,
    );
    if merged_stored.leading_branch.is_none() {
        merged_stored.leading_branch = merged_config.leading_branch.clone();
    }
    merged_config.leading_branch = None;
    sync_config_authoritative_project_field(
        &mut merged_config.startup_command,
        &mut merged_stored.startup_command,
    );
    sync_config_authoritative_project_field(
        &mut merged_config.auto_reopen_agents,
        &mut merged_stored.auto_reopen_agents,
    );
    merged_stored.env = merged_config.env.clone();

    Ok((merged_config, merged_stored))
}

fn sync_config_authoritative_project_field<T>(
    config_value: &mut Option<T>,
    stored_value: &mut Option<T>,
) where
    T: Clone,
{
    match config_value.as_ref() {
        Some(config) => {
            *stored_value = Some(config.clone());
        }
        None => {
            *config_value = stored_value.clone();
        }
    }
}

fn same_expanded_project_path(
    left: &crate::config::ProjectConfig,
    right: &crate::config::ProjectConfig,
) -> bool {
    expanded_project_path(left).is_some_and(|left_path| {
        expanded_project_path(right).is_some_and(|right_path| left_path == right_path)
    })
}

fn expanded_project_path(project: &crate::config::ProjectConfig) -> Option<String> {
    crate::config::expand_path(&project.path)
}

pub(crate) fn portable_project_path(path: &str) -> String {
    let Some(home) = home::home_dir() else {
        return path.to_string();
    };
    let path_buf = Path::new(path);
    if let Ok(relative) = path_buf.strip_prefix(&home) {
        let relative = relative.to_string_lossy();
        if relative.is_empty() {
            "$HOME".to_string()
        } else {
            format!("$HOME/{}", relative)
        }
    } else {
        path.to_string()
    }
}

pub(crate) fn runtime_project_to_config(
    project: &Project,
    existing_projects: &[crate::config::ProjectConfig],
) -> crate::config::ProjectConfig {
    let path = existing_projects
        .iter()
        .find(|existing| {
            existing.id == project.id
                && expanded_project_path(existing).is_some_and(|expanded| expanded == project.path)
        })
        .map(|existing| existing.path.clone())
        .unwrap_or_else(|| portable_project_path(&project.path));

    crate::config::ProjectConfig {
        id: project.id.clone(),
        path,
        name: Some(project.name.clone()),
        default_provider: project
            .explicit_default_provider
            .as_ref()
            .map(|provider| provider.as_str().to_string()),
        leading_branch: project.leading_branch.clone(),
        auto_reopen_agents: project.auto_reopen_agents,
        startup_command: project.startup_command.clone(),
        env: project.env.clone(),
    }
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
            children: Vec::new(),
        });
    }

    // Rows: each labeled target (agents and companion terminals).
    for (label, root_pid) in &targets {
        let (cpu, rss, count, children) = aggregate_tree(&sys, Pid::from_u32(*root_pid));
        rows.push(ResourceStats {
            label: label.clone(),
            pid: Some(*root_pid),
            cpu_percent: cpu,
            rss_bytes: rss,
            process_count: count,
            children,
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
        children: Vec::new(),
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
/// Returns `(total_cpu, total_rss, process_count, top_children)` where
/// `top_children` contains the top 10 individual processes by RSS.
fn aggregate_tree(
    sys: &sysinfo::System,
    root: sysinfo::Pid,
) -> (f32, u64, usize, Vec<ProcessInfo>) {
    let mut cpu = 0.0f32;
    let mut rss = 0u64;
    let mut count = 0usize;
    let mut children = Vec::new();
    for (pid, proc_info) in sys.processes() {
        if *pid == root || is_descendant_of(sys, *pid, root) {
            cpu += proc_info.cpu_usage();
            rss += proc_info.memory();
            count += 1;
            children.push(ProcessInfo {
                name: proc_info.name().to_string_lossy().into_owned(),
                pid: pid.as_u32(),
                cpu_percent: proc_info.cpu_usage(),
                rss_bytes: proc_info.memory(),
            });
        }
    }
    children.sort_by_key(|b| std::cmp::Reverse(b.rss_bytes));
    children.truncate(10);
    (cpu, rss, count, children)
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

    fn test_project(id: &str) -> Project {
        Project {
            id: id.to_string(),
            name: id.to_string(),
            path: format!("/tmp/{id}"),
            explicit_default_provider: None,
            default_provider: ProviderKind::from_str("codex"),
            leading_branch: Some("main".to_string()),
            auto_reopen_agents: None,
            startup_command: None,
            env: Default::default(),
            current_branch: "main".to_string(),
            branch_status: ProjectBranchStatus::Unknown,
            path_missing: false,
        }
    }

    fn test_session(id: &str, project_id: &str, created_offset: i64) -> AgentSession {
        let now = Utc::now() + chrono::Duration::seconds(created_offset);
        AgentSession {
            id: id.to_string(),
            project_id: project_id.to_string(),
            project_path: Some(format!("/tmp/{project_id}")),
            provider: ProviderKind::from_str("codex"),
            source_branch: "main".to_string(),
            branch_name: id.to_string(),
            worktree_path: format!("/tmp/worktrees/{id}"),
            title: None,
            started_providers: Vec::new(),
            desired_running: false,
            auto_reopen_enabled: true,
            status: SessionStatus::Detached,
            created_at: now,
            updated_at: now,
        }
    }

    #[test]
    fn build_left_items_does_not_split_below_threshold() {
        let projects = vec![
            test_project("project-1"),
            test_project("project-2"),
            test_project("project-3"),
            test_project("project-4"),
        ];
        let sessions = vec![test_session("session-1", "project-2", 0)];

        let items = build_left_items(&projects, &sessions, &HashSet::new(), 5, None);

        assert_eq!(
            items,
            vec![
                LeftItem::Project(0),
                LeftItem::Project(1),
                LeftItem::Session(0),
                LeftItem::Project(2),
                LeftItem::Project(3),
            ]
        );
    }

    #[test]
    fn build_left_items_filters_by_title() {
        let projects = vec![test_project("project-1"), test_project("project-2")];
        let mut s0 = test_session("session-1", "project-1", 0);
        s0.title = Some("Fix the parser".to_string());
        let mut s1 = test_session("session-2", "project-2", 0);
        s1.title = Some("Add logging".to_string());
        let sessions = vec![s0, s1];

        let items = build_left_items(&projects, &sessions, &HashSet::new(), 0, Some("parser"));

        assert_eq!(items, vec![LeftItem::Project(0), LeftItem::Session(0)]);
    }

    #[test]
    fn build_left_items_filters_by_branch_name() {
        let projects = vec![test_project("project-1")];
        // test_session sets branch_name == id.
        let sessions = vec![
            test_session("feature-login", "project-1", 0),
            test_session("chore-docs", "project-1", 0),
        ];

        let items = build_left_items(&projects, &sessions, &HashSet::new(), 0, Some("login"));

        assert_eq!(items, vec![LeftItem::Project(0), LeftItem::Session(0)]);
    }

    #[test]
    fn build_left_items_project_name_match_surfaces_all_agents() {
        let projects = vec![test_project("backend"), test_project("frontend")];
        let sessions = vec![
            test_session("alpha", "backend", 0),
            test_session("beta", "backend", 0),
            test_session("gamma", "frontend", 0),
        ];

        // "backend" matches the project name, so both of its agents appear even
        // though their own titles/branches don't contain the needle. The
        // unrelated project is dropped.
        let items = build_left_items(&projects, &sessions, &HashSet::new(), 0, Some("backend"));

        assert_eq!(
            items,
            vec![
                LeftItem::Project(0),
                LeftItem::Session(0),
                LeftItem::Session(1)
            ]
        );
    }

    #[test]
    fn build_left_items_filter_hides_empty_projects_and_ignores_collapse() {
        let projects = vec![
            test_project("alpha"), // has a matching agent, but is collapsed
            test_project("beta"),  // no agents at all
            test_project("gamma"), // an agent that does not match
        ];
        let mut s0 = test_session("s0", "alpha", 0);
        s0.title = Some("login flow".to_string());
        let mut s1 = test_session("s1", "gamma", 0);
        s1.title = Some("unrelated".to_string());
        let sessions = vec![s0, s1];

        let mut collapsed = HashSet::new();
        collapsed.insert("alpha".to_string());

        let items = build_left_items(&projects, &sessions, &collapsed, 5, Some("login"));

        // alpha is revealed despite being collapsed; the empty project and the
        // non-matching project are hidden; no spacer/separator in search view.
        assert_eq!(items, vec![LeftItem::Project(0), LeftItem::Session(0)]);
    }

    #[test]
    fn build_left_items_empty_query_is_unfiltered() {
        let projects = vec![test_project("p1")];
        let sessions = vec![test_session("s1", "p1", 0)];

        let filtered = build_left_items(&projects, &sessions, &HashSet::new(), 0, Some(""));
        let unfiltered = build_left_items(&projects, &sessions, &HashSet::new(), 0, None);

        assert_eq!(filtered, unfiltered);
    }

    #[test]
    fn build_left_items_splits_empty_projects_at_threshold() {
        let projects = vec![
            test_project("project-1"),
            test_project("project-2"),
            test_project("project-3"),
            test_project("project-4"),
            test_project("project-5"),
        ];
        let sessions = vec![
            test_session("session-1", "project-2", 0),
            test_session("session-2", "project-4", 0),
        ];

        let items = build_left_items(&projects, &sessions, &HashSet::new(), 5, None);

        assert_eq!(
            items,
            vec![
                LeftItem::Project(1),
                LeftItem::Session(0),
                LeftItem::Project(3),
                LeftItem::Session(1),
                LeftItem::EmptyProjectsSpacer,
                LeftItem::EmptyProjectsSeparator,
                LeftItem::Project(0),
                LeftItem::Project(2),
                LeftItem::Project(4),
            ]
        );
    }

    #[test]
    fn build_left_items_moves_project_above_separator_when_session_is_added() {
        let projects = vec![
            test_project("project-1"),
            test_project("project-2"),
            test_project("project-3"),
            test_project("project-4"),
            test_project("project-5"),
        ];
        let sessions = vec![
            test_session("session-1", "project-2", 0),
            test_session("session-2", "project-4", 0),
            test_session("session-3", "project-3", 0),
        ];

        let items = build_left_items(&projects, &sessions, &HashSet::new(), 5, None);

        assert_eq!(
            items,
            vec![
                LeftItem::Project(1),
                LeftItem::Session(0),
                LeftItem::Project(2),
                LeftItem::Session(2),
                LeftItem::Project(3),
                LeftItem::Session(1),
                LeftItem::EmptyProjectsSpacer,
                LeftItem::EmptyProjectsSeparator,
                LeftItem::Project(0),
                LeftItem::Project(4),
            ]
        );
    }

    #[test]
    fn build_left_items_keeps_session_sort_order_within_project_grouping() {
        let projects = vec![
            test_project("project-1"),
            test_project("project-2"),
            test_project("project-3"),
            test_project("project-4"),
            test_project("project-5"),
        ];
        let mut sessions = vec![
            test_session("older", "project-2", 0),
            test_session("newer", "project-2", 10),
            test_session("other", "project-4", 5),
        ];
        sessions.sort_by_key(|session| std::cmp::Reverse(session.created_at));

        let items = build_left_items(&projects, &sessions, &HashSet::new(), 5, None);

        assert_eq!(
            items,
            vec![
                LeftItem::Project(1),
                LeftItem::Session(0),
                LeftItem::Session(2),
                LeftItem::Project(3),
                LeftItem::Session(1),
                LeftItem::EmptyProjectsSpacer,
                LeftItem::EmptyProjectsSeparator,
                LeftItem::Project(0),
                LeftItem::Project(2),
                LeftItem::Project(4),
            ]
        );
    }

    #[test]
    fn build_left_items_can_disable_empty_project_split() {
        let projects = vec![
            test_project("project-1"),
            test_project("project-2"),
            test_project("project-3"),
            test_project("project-4"),
            test_project("project-5"),
        ];
        let sessions = vec![test_session("session-1", "project-2", 0)];

        let items = build_left_items(&projects, &sessions, &HashSet::new(), 0, None);

        assert!(!items.contains(&LeftItem::EmptyProjectsSeparator));
        assert!(!items.contains(&LeftItem::EmptyProjectsSpacer));
        assert_eq!(items[0], LeftItem::Project(0));
    }

    #[test]
    fn build_left_items_omits_separator_when_no_project_has_sessions() {
        let projects = vec![
            test_project("project-1"),
            test_project("project-2"),
            test_project("project-3"),
            test_project("project-4"),
            test_project("project-5"),
        ];

        let items = build_left_items(&projects, &[], &HashSet::new(), 5, None);

        assert_eq!(
            items,
            vec![
                LeftItem::Project(0),
                LeftItem::Project(1),
                LeftItem::Project(2),
                LeftItem::Project(3),
                LeftItem::Project(4),
            ]
        );
    }

    #[test]
    fn config_only_project_is_synced_to_sqlite_and_preserved() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let root = dir.path().to_path_buf();
        let paths = DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root: root.clone(),
        };
        std::fs::create_dir_all(&paths.worktrees_root).expect("worktrees");
        std::fs::write(
            &paths.config_path,
            r#"
[defaults]
provider = "codex"

[[projects]]
id = "project-1"
path = "$CODE/dux"
name = "dux"
default_provider = "claude"
leading_branch = "main"
"#,
        )
        .expect("write config");

        let mut config = ensure_config(&paths).expect("load config");
        let bindings = RuntimeBindings::from_keys_config(&config.keys);
        let store = SessionStore::open(&paths.sessions_db_path).expect("store");

        sync_config_projects_with_store(&mut config, &paths, &bindings, &store)
            .expect("sync projects");

        assert_eq!(config.projects.len(), 1);
        let projects = store.load_projects().expect("load projects");
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].id, "project-1");
        assert_eq!(projects[0].path, "$CODE/dux");
        assert_eq!(projects[0].name.as_deref(), Some("dux"));
        assert_eq!(projects[0].default_provider.as_deref(), Some("claude"));
        assert_eq!(projects[0].leading_branch.as_deref(), Some("main"));

        let saved = std::fs::read_to_string(&paths.config_path).expect("read config");
        assert!(saved.contains("[[projects]]"));
        assert!(saved.contains("project-1"));
        assert!(!saved.contains("leading_branch"));
    }

    #[test]
    fn sqlite_only_project_is_written_to_config() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let root = dir.path().to_path_buf();
        let paths = DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root: root.clone(),
        };
        paths.ensure_dirs().expect("dirs");
        std::fs::write(&paths.config_path, "[defaults]\nprovider = \"codex\"\n").expect("config");
        let mut config = ensure_config(&paths).expect("load config");
        let bindings = RuntimeBindings::from_keys_config(&config.keys);
        let store = SessionStore::open(&paths.sessions_db_path).expect("store");
        store
            .upsert_project(&crate::config::ProjectConfig {
                id: "project-db".to_string(),
                path: root.join("repo").to_string_lossy().to_string(),
                name: Some("repo".to_string()),
                default_provider: Some("codex".to_string()),
                leading_branch: Some("main".to_string()),
                auto_reopen_agents: None,
                startup_command: Some("npm install".to_string()),
                env: Default::default(),
            })
            .expect("seed project");

        sync_config_projects_with_store(&mut config, &paths, &bindings, &store)
            .expect("sync projects");

        assert_eq!(config.projects.len(), 1);
        let saved = std::fs::read_to_string(&paths.config_path).expect("read config");
        assert!(saved.contains("id = \"project-db\""));
        assert!(saved.contains("startup_command = \"npm install\""));
        assert!(!saved.contains("leading_branch"));
    }

    #[test]
    fn config_project_backfills_missing_sqlite_optional_fields() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let root = dir.path().to_path_buf();
        let paths = DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root: root.clone(),
        };
        paths.ensure_dirs().expect("dirs");
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        std::fs::write(
            &paths.config_path,
            format!(
                "[defaults]\nprovider = \"codex\"\n\n[[projects]]\nid = \"project-1\"\npath = \"{}\"\nname = \"repo\"\nleading_branch = \"main\"\n",
                repo.display()
            ),
        )
        .expect("config");
        let mut config = ensure_config(&paths).expect("load config");
        let bindings = RuntimeBindings::from_keys_config(&config.keys);
        let store = SessionStore::open(&paths.sessions_db_path).expect("store");
        store
            .upsert_project(&crate::config::ProjectConfig {
                id: "project-1".to_string(),
                path: repo.to_string_lossy().to_string(),
                name: Some("repo".to_string()),
                default_provider: None,
                leading_branch: None,
                auto_reopen_agents: None,
                startup_command: None,
                env: Default::default(),
            })
            .expect("seed project");

        sync_config_projects_with_store(&mut config, &paths, &bindings, &store)
            .expect("sync projects");

        let projects = store.load_projects().expect("load projects");
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].leading_branch.as_deref(), Some("main"));
        let saved = std::fs::read_to_string(&paths.config_path).expect("read config");
        assert!(!saved.contains("leading_branch"));
    }

    #[test]
    fn derived_project_leading_branch_is_persisted_to_sqlite_only() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let root = dir.path().to_path_buf();
        let paths = DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root: root.clone(),
        };
        paths.ensure_dirs().expect("dirs");
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        std::process::Command::new("git")
            .arg("init")
            .arg(&repo)
            .output()
            .expect("git init");
        std::process::Command::new("git")
            .arg("checkout")
            .arg("-b")
            .arg("main")
            .current_dir(&repo)
            .output()
            .expect("git checkout main");
        std::fs::write(
            &paths.config_path,
            format!(
                "[defaults]\nprovider = \"codex\"\n\n[[projects]]\nid = \"project-1\"\npath = \"{}\"\nname = \"repo\"\n",
                repo.display()
            ),
        )
        .expect("config");
        let mut config = ensure_config(&paths).expect("load config");
        let bindings = RuntimeBindings::from_keys_config(&config.keys);
        let store = SessionStore::open(&paths.sessions_db_path).expect("store");
        store
            .upsert_project(&crate::config::ProjectConfig {
                id: "project-1".to_string(),
                path: repo.to_string_lossy().to_string(),
                name: Some("repo".to_string()),
                default_provider: None,
                leading_branch: None,
                auto_reopen_agents: None,
                startup_command: None,
                env: Default::default(),
            })
            .expect("seed project");

        sync_config_projects_with_store(&mut config, &paths, &bindings, &store)
            .expect("sync projects");
        let projects = load_projects(&store.load_projects().expect("load projects"), &config);
        assert_eq!(projects[0].leading_branch.as_deref(), Some("main"));

        persist_runtime_projects_to_config_and_store(
            &projects,
            &mut config,
            &paths,
            &bindings,
            &store,
        )
        .expect("persist derived projects");

        let saved = std::fs::read_to_string(&paths.config_path).expect("read config");
        assert!(!saved.contains("leading_branch"));
        let stored = store.load_projects().expect("reload projects");
        assert_eq!(stored[0].leading_branch.as_deref(), Some("main"));
    }

    #[test]
    fn config_project_values_update_sqlite_on_sync() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let root = dir.path().to_path_buf();
        let paths = DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root: root.clone(),
        };
        paths.ensure_dirs().expect("dirs");
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).expect("repo");
        std::fs::write(
            &paths.config_path,
            format!(
                "[defaults]\nprovider = \"codex\"\n\n[[projects]]\nid = \"project-1\"\npath = \"{}\"\nname = \"repo\"\nstartup_command = \"npm install\"\n",
                repo.display()
            ),
        )
        .expect("config");
        let mut config = ensure_config(&paths).expect("load config");
        let bindings = RuntimeBindings::from_keys_config(&config.keys);
        let store = SessionStore::open(&paths.sessions_db_path).expect("store");
        store
            .upsert_project(&crate::config::ProjectConfig {
                id: "project-1".to_string(),
                path: repo.to_string_lossy().to_string(),
                name: Some("repo".to_string()),
                default_provider: None,
                leading_branch: None,
                auto_reopen_agents: None,
                startup_command: Some("pnpm install".to_string()),
                env: Default::default(),
            })
            .expect("seed project");

        sync_config_projects_with_store(&mut config, &paths, &bindings, &store)
            .expect("sync projects");

        let projects = store.load_projects().expect("load projects");
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].startup_command.as_deref(), Some("npm install"));
    }

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
        let (_cpu, rss, count, _children) = aggregate_tree(&sys, self_pid);
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
    fn build_visual_rows_respects_expansion() {
        let rows = vec![
            ResourceStats {
                label: "dux".into(),
                pid: Some(1),
                cpu_percent: 0.0,
                rss_bytes: 0,
                process_count: 1,
                children: Vec::new(),
            },
            ResourceStats {
                label: "Agent".into(),
                pid: Some(100),
                cpu_percent: 5.0,
                rss_bytes: 1024,
                process_count: 3,
                children: vec![
                    ProcessInfo {
                        name: "node".into(),
                        pid: 101,
                        cpu_percent: 3.0,
                        rss_bytes: 512,
                    },
                    ProcessInfo {
                        name: "claude".into(),
                        pid: 102,
                        cpu_percent: 2.0,
                        rss_bytes: 256,
                    },
                ],
            },
            ResourceStats {
                label: "TOTAL".into(),
                pid: None,
                cpu_percent: 5.0,
                rss_bytes: 1024,
                process_count: 4,
                children: Vec::new(),
            },
        ];

        // Nothing expanded: 3 visual rows (one per parent).
        let visual = build_visual_rows(&rows, &HashSet::new());
        assert_eq!(visual.len(), 3);

        // Expand PID 100: 3 parents + 2 children = 5 visual rows.
        let mut expanded = HashSet::new();
        expanded.insert(100);
        let visual = build_visual_rows(&rows, &expanded);
        assert_eq!(visual.len(), 5);
        assert!(matches!(visual[0], VisualRow::Parent(0)));
        assert!(matches!(visual[1], VisualRow::Parent(1)));
        assert!(matches!(visual[2], VisualRow::Child(1, 0)));
        assert!(matches!(visual[3], VisualRow::Child(1, 1)));
        assert!(matches!(visual[4], VisualRow::Parent(2)));

        // Expanding a PID that doesn't match any row: no effect.
        let mut expanded = HashSet::new();
        expanded.insert(999);
        let visual = build_visual_rows(&rows, &expanded);
        assert_eq!(visual.len(), 3);
    }

    #[test]
    fn classify_project_worktrees_marks_managed_external_and_existing_agent() {
        let root =
            std::env::temp_dir().join(format!("dux-classify-worktrees-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let repo = root.join("repo");
        let managed = root.join("worktrees").join("demo").join("managed-orphan");
        let external = root.join("external checkout");
        let existing = root.join("worktrees").join("demo").join("existing-agent");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir_all(&managed).unwrap();
        fs::create_dir_all(&external).unwrap();
        fs::create_dir_all(&existing).unwrap();

        let project = Project {
            id: "project-1".to_string(),
            name: "demo".to_string(),
            path: repo.to_string_lossy().to_string(),
            explicit_default_provider: None,
            default_provider: ProviderKind::new("codex"),
            leading_branch: Some("main".to_string()),
            auto_reopen_agents: None,
            startup_command: None,
            env: Default::default(),
            current_branch: "main".to_string(),
            branch_status: ProjectBranchStatus::Leading,
            path_missing: false,
        };
        let paths = DuxPaths {
            root: root.clone(),
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("lock"),
        };
        let sessions = vec![AgentSession {
            id: "session-1".to_string(),
            project_id: project.id.clone(),
            project_path: Some(project.path.clone()),
            provider: ProviderKind::new("codex"),
            source_branch: "main".to_string(),
            branch_name: "existing".to_string(),
            worktree_path: existing.to_string_lossy().to_string(),
            title: None,
            started_providers: Vec::new(),
            desired_running: false,
            auto_reopen_enabled: true,
            status: SessionStatus::Detached,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }];
        let worktrees = vec![
            git::GitWorktree {
                path: repo.clone(),
                head: Some("0000000".to_string()),
                branch_name: Some("main".to_string()),
                detached: false,
            },
            git::GitWorktree {
                path: managed.clone(),
                head: Some("1111111".to_string()),
                branch_name: Some("managed-orphan".to_string()),
                detached: false,
            },
            git::GitWorktree {
                path: external.clone(),
                head: Some("2222222".to_string()),
                branch_name: Some("feature".to_string()),
                detached: false,
            },
            git::GitWorktree {
                path: existing.clone(),
                head: Some("3333333".to_string()),
                branch_name: Some("existing".to_string()),
                detached: false,
            },
        ];

        let entries = classify_project_worktrees(&project, &paths, &sessions, worktrees);
        let managed_entry = entries
            .iter()
            .find(|entry| entry.path == managed.canonicalize().unwrap())
            .unwrap();
        assert!(managed_entry.is_managed_by_dux);
        assert!(!managed_entry.is_external);
        assert!(managed_entry.is_selectable);

        let external_entry = entries
            .iter()
            .find(|entry| entry.path == external.canonicalize().unwrap())
            .unwrap();
        assert!(!external_entry.is_managed_by_dux);
        assert!(external_entry.is_external);
        assert!(external_entry.is_selectable);

        let existing_entry = entries
            .iter()
            .find(|entry| entry.path == existing.canonicalize().unwrap())
            .unwrap();
        assert_eq!(
            existing_entry.existing_session_id.as_deref(),
            Some("session-1")
        );
        assert!(!existing_entry.is_selectable);

        let project_checkout_entry = entries
            .iter()
            .find(|entry| entry.path == repo.canonicalize().unwrap())
            .unwrap();
        assert!(project_checkout_entry.is_project_checkout);
        assert!(!project_checkout_entry.is_selectable);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn project_worktree_visual_rows_separate_project_checkout() {
        let entries = vec![
            ProjectWorktreeEntry {
                path: PathBuf::from("/repo/managed"),
                branch_name: "feature".to_string(),
                is_managed_by_dux: true,
                existing_session_id: None,
                is_external: false,
                is_project_checkout: false,
                is_selectable: true,
            },
            ProjectWorktreeEntry {
                path: PathBuf::from("/repo/main"),
                branch_name: "main".to_string(),
                is_managed_by_dux: false,
                existing_session_id: None,
                is_external: true,
                is_project_checkout: true,
                is_selectable: false,
            },
        ];

        let rows = project_worktree_visual_rows(&entries, false, None);

        assert!(matches!(
            rows.first(),
            Some(ProjectWorktreeVisualRow::Header("Available Worktrees"))
        ));
        assert!(
            rows.iter()
                .any(|row| matches!(row, ProjectWorktreeVisualRow::Header("Project Checkout")))
        );
        assert_eq!(selectable_project_worktree_indices(&entries), vec![0]);
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
