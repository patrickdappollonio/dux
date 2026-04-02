use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use chrono::Utc;
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
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
use crate::model::{AgentSession, ChangedFile, Project, ProviderKind, SessionStatus};
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
    pub(crate) right_section: RightSection,
    pub(crate) files_index: usize,
    pub(crate) commit_input: String,
    pub(crate) commit_input_cursor: usize,
    pub(crate) commit_scroll: u16,
    pub(crate) commit_generating: bool,
    pub(crate) left_width_pct: u16,
    pub(crate) right_width_pct: u16,
    pub(crate) focus: FocusPane,
    pub(crate) center_mode: CenterMode,
    pub(crate) left_collapsed: bool,
    pub(crate) resize_mode: bool,
    pub(crate) help_scroll: Option<u16>,
    pub(crate) last_help_height: u16,
    pub(crate) last_help_lines: u16,
    pub(crate) fullscreen_agent: bool,
    pub(crate) status: StatusLine,
    pub(crate) prompt: PromptState,
    pub(crate) input_target: InputTarget,
    pub(crate) worker_tx: Sender<WorkerEvent>,
    pub(crate) worker_rx: Receiver<WorkerEvent>,
    pub(crate) providers: HashMap<String, PtyClient>,
    pub(crate) create_agent_in_flight: bool,
    pub(crate) last_pty_size: (u16, u16),
    pub(crate) last_diff_height: u16,
    pub(crate) last_diff_visual_lines: u16,
    pub(crate) theme: Theme,
    pub(crate) tick_count: u64,
    pub(crate) watched_worktree: Arc<Mutex<Option<PathBuf>>>,
    pub(crate) has_active_agent: Arc<AtomicBool>,
    pub(crate) collapsed_projects: HashSet<String>,
    pub(crate) left_items_cache: Vec<LeftItem>,
    pub(crate) mouse_layout: MouseLayoutState,
    pub(crate) mouse_drag: Option<ResizeDragState>,
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
        selected: usize,
        searching: bool,
    },
    BrowseProjects {
        current_dir: PathBuf,
        entries: Vec<BrowserEntry>,
        loading: bool,
        selected: usize,
        filter: String,
        searching: bool,
        editing_path: bool,
        path_input: String,
        tab_completions: Vec<String>,
        tab_index: usize,
    },
    ConfirmDeleteAgent {
        session_id: String,
        branch_name: String,
        confirm_selected: bool, // false = Cancel (default), true = Delete
    },
    ConfirmQuit {
        active_count: usize,
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
        self.agent_term = None;
        self.unstaged_list = None;
        self.staged_list = None;
        self.commit_area = None;
        self.commit_text_area = None;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResizeDragState {
    LeftDivider,
    RightDivider,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum LeftItem {
    Project(usize),
    Session(usize),
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
            bindings,
            config,
            paths,
            session_store,
            projects,
            sessions,
            staged_files: Vec::new(),
            unstaged_files: Vec::new(),
            selected_left: 0,
            right_section: RightSection::Unstaged,
            files_index: 0,
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
            fullscreen_agent: false,
            status: StatusLine::new(initial_status),
            prompt: PromptState::None,
            input_target: InputTarget::None,
            worker_tx,
            worker_rx,
            providers: HashMap::new(),
            create_agent_in_flight: false,
            last_pty_size: (0, 0),
            last_diff_height: 0,
            last_diff_visual_lines: 0,
            theme: Theme::default_dark(),
            tick_count: 0,
            watched_worktree: Arc::clone(&watched_worktree),
            has_active_agent: Arc::new(AtomicBool::new(false)),
            collapsed_projects: HashSet::new(),
            left_items_cache: Vec::new(),
            mouse_layout: MouseLayoutState::default(),
            mouse_drag: None,
        };
        app.restore_sessions();
        app.rebuild_left_items();
        app.reload_changed_files();
        Ok(app)
    }

    pub fn run(&mut self) -> Result<()> {
        self.spawn_changed_files_poller();
        let mut terminal = ratatui::init();
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
                    Event::Mouse(mouse) => self.handle_mouse(mouse),
                    Event::Resize(_, _) => {}
                    _ => {}
                }
            }
        }
        ratatui::restore();
        Ok(())
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
        if !matches!(self.prompt, PromptState::None) {
            self.prompt = PromptState::None;
            self.set_info("Closed overlay.");
            return true;
        }
        if self.help_scroll.is_some() {
            self.help_scroll = None;
            self.set_info("Closed overlay.");
            return true;
        }
        if matches!(self.center_mode, CenterMode::Diff { .. }) {
            self.center_mode = CenterMode::Agent;
            self.focus = FocusPane::Files;
            self.set_info("Returned to agent view.");
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

    pub(crate) fn open_rename_session(&mut self) -> Result<()> {
        if let Some(session) = self.selected_session().cloned() {
            let current_name = session.title.unwrap_or_else(|| session.branch_name.clone());
            let cursor = current_name.len();
            self.input_target = InputTarget::None;
            self.fullscreen_agent = false;
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
            session.status = status;
            session.updated_at = Utc::now();
            let _ = self.session_store.upsert_session(session);
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
