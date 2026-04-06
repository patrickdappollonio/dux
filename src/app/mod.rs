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
    pub(crate) last_pty_size: (u16, u16),
    pub(crate) prev_scrollback_offset: usize,
    pub(crate) last_diff_height: u16,
    pub(crate) last_diff_visual_lines: u16,
    pub(crate) theme: Theme,
    pub(crate) tick_count: u64,
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
    pub(crate) branch_sync_sessions: Arc<Mutex<Vec<BranchSyncEntry>>>,
}

/// Snapshot of session data shared with the branch-sync background worker.
#[derive(Clone, Debug)]
pub(crate) struct BranchSyncEntry {
    pub(crate) session_id: String,
    pub(crate) worktree_path: String,
    pub(crate) branch_name: String,
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
        lines: Vec<Line<'static>>,
        scroll: u16,
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
    LeftRow(usize),
    TerminalRow(usize),
    CenterPane,
    UnstagedFile(usize),
    StagedFile(usize),
    CommandItem(usize),
    BrowseProjectItem(usize),
    PickEditorItem(usize),
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct RecentMouseClick {
    pub(crate) target: MouseClickTarget,
    pub(crate) at: Instant,
    pub(crate) threshold: Duration,
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
    },
    ForkSession {
        project: Project,
        source_session: Box<AgentSession>,
        source_label: String,
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
    PullCompleted(Result<(), String>),
    BrowserEntriesReady {
        dir: PathBuf,
        entries: Vec<BrowserEntry>,
    },
    ClipboardCopyCompleted {
        path: String,
        result: Result<(), String>,
    },
    BranchSyncReady(Vec<(String, String)>),
    BranchRenameCompleted {
        session_id: String,
        new_branch: String,
        previous_title: Option<String>,
        result: Result<(), String>,
    },
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
        let mut app = Self {
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
            last_pty_size: (0, 0),
            prev_scrollback_offset: 0,
            last_diff_height: 0,
            last_diff_visual_lines: 0,
            theme: Theme::default_dark(),
            tick_count: 0,
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
            branch_sync_sessions: Arc::new(Mutex::new(Vec::new())),
        };
        app.restore_sessions();
        app.rebuild_left_items();
        app.reload_changed_files();
        app.update_branch_sync_sessions();
        Ok(app)
    }

    pub fn run(&mut self) -> Result<()> {
        self.spawn_changed_files_poller();
        self.spawn_branch_sync_worker();
        let mut terminal = ratatui::init();
        execute!(stdout(), EnableMouseCapture)?;

        let result: Result<()> = {
            loop {
                self.drain_events();
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
                        let event = match crate::io_retry::retry_on_interrupt(event::read) {
                            Ok(event) => event,
                            Err(err) => {
                                self.report_runtime_error(
                                    "event read failed; input handling was skipped",
                                    &err,
                                );
                                continue;
                            }
                        };
                        match event {
                            Event::Key(key) => {
                                let should_exit = match self.handle_key(key) {
                                    Ok(should_exit) => should_exit,
                                    Err(err) => {
                                        self.report_runtime_error(
                                            "key handling failed",
                                            err.as_ref(),
                                        );
                                        false
                                    }
                                };
                                if should_exit {
                                    break;
                                }
                            }
                            Event::Mouse(mouse) => {
                                if self.handle_mouse(mouse) {
                                    break;
                                }
                            }
                            Event::Resize(_, _) => {}
                            _ => {}
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

    fn report_runtime_error(&mut self, context: &str, err: &dyn std::error::Error) {
        logger::error(&format!("{context}: {err}"));
        self.set_error(format!("{context}: {err}"));
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
            "show-terminal" => self.show_companion_terminal(),
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
                input: TextInput::with_text(current_name),
                rename_branch: false,
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
