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

use crate::config::{
    Config, DuxPaths, ProjectConfig, ProviderCommandConfig, check_provider_available,
    ensure_config, save_config, validate_keys,
};
use crate::editor::DetectedEditor;
use crate::git;
use crate::keybindings::{Action, BindingScope, HintContext, RuntimeBindings};
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
    pub(crate) files_search_query: String,
    pub(crate) files_search_active: bool,
    pub(crate) commit_input: String,
    pub(crate) commit_input_cursor: usize,
    pub(crate) commit_scroll: u16,
    pub(crate) commit_generating: bool,
    pub(crate) left_width_pct: u16,
    pub(crate) right_width_pct: u16,
    pub(crate) terminal_pane_height_pct: u16,
    pub(crate) staged_pane_height_pct: u16,
    pub(crate) commit_pane_height_pct: u16,
    pub(crate) focus: FocusPane,
    pub(crate) center_mode: CenterMode,
    pub(crate) left_collapsed: bool,
    pub(crate) resize_mode: bool,
    pub(crate) help_scroll: Option<u16>,
    pub(crate) last_help_height: u16,
    pub(crate) last_help_lines: u16,
    pub(crate) fullscreen_overlay: FullscreenOverlay,
    pub(crate) status: StatusLine,
    pub(crate) prompt: PromptState,
    pub(crate) input_target: InputTarget,
    pub(crate) session_surface: SessionSurface,
    pub(crate) worker_tx: Sender<WorkerEvent>,
    pub(crate) worker_rx: Receiver<WorkerEvent>,
    pub(crate) providers: HashMap<String, PtyClient>,
    pub(crate) companion_terminals: HashMap<String, CompanionTerminal>,
    pub(crate) active_terminal_id: Option<String>,
    pub(crate) terminal_counter: usize,
    pub(crate) create_agent_in_flight: bool,
    pub(crate) last_pty_size: (u16, u16),
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

#[derive(Clone, Debug)]
pub(crate) enum PromptState {
    None,
    Command {
        input: String,
        cursor: usize,
        selected: usize,
        searching: bool,
    },
    BrowseProjects {
        current_dir: PathBuf,
        entries: Vec<BrowserEntry>,
        loading: bool,
        selected: usize,
        filter: String,
        filter_cursor: usize,
        searching: bool,
        editing_path: bool,
        path_input: String,
        path_cursor: usize,
        tab_completions: Vec<String>,
        tab_index: usize,
    },
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
        input: String,
        cursor: usize,
    },
    PickEditor {
        session_label: String,
        worktree_path: String,
        editors: Vec<DetectedEditor>,
        selected: usize,
    },
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
}

mod input;
mod render;
mod sessions;
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
            files_search_query: String::new(),
            files_search_active: false,
            commit_input: String::new(),
            commit_input_cursor: 0,
            commit_scroll: 0,
            commit_generating: false,
            left_collapsed: false,
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
            worker_tx,
            worker_rx,
            providers: HashMap::new(),
            companion_terminals: HashMap::new(),
            active_terminal_id: None,
            terminal_counter: 0,
            create_agent_in_flight: false,
            last_pty_size: (0, 0),
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
        };
        app.restore_sessions();
        app.rebuild_left_items();
        app.reload_changed_files();
        Ok(app)
    }

    pub fn run(&mut self) -> Result<()> {
        self.spawn_changed_files_poller();
        let mut terminal = ratatui::init();
        execute!(stdout(), EnableMouseCapture)?;

        let result = (|| -> Result<()> {
            loop {
                self.drain_events();
                self.tick_count = self.tick_count.wrapping_add(1);
                terminal.draw(|frame| self.render(frame))?;
                if event::poll(Duration::from_millis(100))? {
                    match event::read()? {
                        Event::Key(key) => {
                            if self.handle_key(key)? {
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
            Ok(())
        })();

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
            self.fullscreen_overlay = FullscreenOverlay::None;
            self.session_surface = SessionSurface::Agent;
            self.input_target = InputTarget::None;
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

    pub(crate) fn set_error(&mut self, message: impl Into<String>) {
        self.status.error(message);
    }

    pub(crate) fn execute_command(&mut self, command: String) -> Result<()> {
        let command = command.trim();
        match command {
            "new-agent" => self.create_agent_for_selected_project(),
            "provider" => self.cycle_selected_project_provider(),
            "refresh-project" => self.refresh_selected_project(),
            "delete-project" => self.delete_selected_project(),
            "remove-project" => self.remove_selected_project(),
            "delete-agent" => self.confirm_delete_selected_session(),
            "rename-agent" => self.open_rename_session(),
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
            "" => Ok(()),
            other => {
                self.set_error(format!("Unknown command: \"{other}\""));
                Ok(())
            }
        }
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
        !self.files_search_query.is_empty()
    }

    pub(crate) fn clear_files_search(&mut self) {
        self.files_search_query.clear();
        self.files_search_active = false;
    }

    pub(crate) fn update_files_search(&mut self, query: String) -> bool {
        self.files_search_query = query;
        if self.files_search_query.is_empty() {
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
        if self.files_search_query.is_empty() {
            return Vec::new();
        }

        let needle = self.files_search_query.to_lowercase();
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
            let cursor = current_name.len();
            self.input_target = InputTarget::None;
            self.fullscreen_overlay = FullscreenOverlay::None;
            self.prompt = PromptState::RenameSession {
                session_id: session.id,
                input: current_name,
                cursor,
            };
        } else {
            self.set_error("No agent session selected.");
        }
        Ok(())
    }

    pub(crate) fn apply_rename_session(&mut self, session_id: &str, new_name: String) {
        let name = new_name.trim().to_string();
        if name.is_empty() {
            self.set_error("Name cannot be empty.");
            return;
        }

        // Gather session info we need before mutating.
        let Some(session) = self.sessions.iter().find(|s| s.id == session_id) else {
            return;
        };
        let old_branch = session.branch_name.clone();
        let worktree = session.worktree_path.clone();

        // Skip the git rename if the branch name is unchanged.
        if name != old_branch
            && let Err(e) = git::rename_branch(Path::new(&worktree), &old_branch, &name)
        {
            self.set_error(format!("Rename failed: {e}"));
            return;
        }

        // Git rename succeeded — update the session.
        if let Some(session) = self.sessions.iter_mut().find(|s| s.id == session_id) {
            session.branch_name = name.clone();
            session.title = Some(name.clone());
            session.updated_at = Utc::now();
        }
        if let Some(session) = self.sessions.iter().find(|s| s.id == session_id) {
            let _ = self.session_store.upsert_session(session);
        }
        self.set_info(format!("Renamed agent to \"{name}\" (branch updated)."));
        self.rebuild_left_items();
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
