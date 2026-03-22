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
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
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
    ensure_config, save_config,
};
use crate::git;
use crate::keybindings::{self, Action, BindingScope, HintContext};
use crate::logger;
use crate::provider;
use crate::model::{AgentSession, ChangedFile, Project, ProviderKind, SessionStatus};
use crate::pty::PtyClient;
use crate::statusline::{StatusLine, StatusTone};
use crate::storage::SessionStore;
use crate::theme::Theme;

pub struct App {
    pub(crate) config: Config,
    pub(crate) paths: DuxPaths,
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
    pub(crate) help_overlay: bool,
    pub(crate) status: StatusLine,
    pub(crate) prompt: PromptState,
    pub(crate) input_target: InputTarget,
    pub(crate) worker_tx: Sender<WorkerEvent>,
    pub(crate) worker_rx: Receiver<WorkerEvent>,
    pub(crate) providers: HashMap<String, PtyClient>,
    pub(crate) create_agent_in_flight: bool,
    pub(crate) last_pty_size: (u16, u16),
    pub(crate) last_diff_height: u16,
    pub(crate) theme: Theme,
    pub(crate) tick_count: u64,
    pub(crate) watched_worktree: Arc<Mutex<Option<PathBuf>>>,
    pub(crate) has_active_agent: Arc<AtomicBool>,
    pub(crate) collapsed_projects: HashSet<String>,
    pub(crate) left_items_cache: Vec<LeftItem>,
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
    Staged,
    Unstaged,
}

impl RightSection {
    /// Returns the next section, or `None` to exit the pane.
    /// Order: Unstaged (top) → Staged (bottom).
    pub(crate) fn next(self, has_staged: bool) -> Option<Self> {
        match self {
            Self::Unstaged if has_staged => Some(Self::Staged),
            Self::Unstaged | Self::Staged => None,
        }
    }

    /// Returns the previous section, or `None` to exit the pane.
    pub(crate) fn previous(self) -> Option<Self> {
        match self {
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
            Self::Staged
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

#[derive(Clone, Copy, Debug)]
pub(crate) enum LeftItem {
    Project(usize),
    Session(usize),
}

pub(crate) enum WorkerEvent {
    CreateAgentProgress(String),
    CreateAgentReady {
        session: AgentSession,
        client: PtyClient,
        pty_size: (u16, u16), // (rows, cols) the PTY was spawned with
    },
    CreateAgentFailed(String),
    ChangedFilesReady {
        staged: Vec<ChangedFile>,
        unstaged: Vec<ChangedFile>,
    },
    CommitMessageGenerated(String),
    CommitMessageFailed(String),
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
        let session_store = SessionStore::open(&paths.sessions_db_path)?;
        let projects = load_projects(&config);
        let sessions = session_store.load_sessions()?;
        let (worker_tx, worker_rx) = mpsc::channel();
        let watched_worktree: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
        let mut app = Self {
            left_width_pct: config.ui.left_width_pct,
            right_width_pct: config.ui.right_width_pct,
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
            help_overlay: false,
            status: StatusLine::new("Press p to add a project, a to create an agent, ? for help."),
            prompt: PromptState::None,
            input_target: InputTarget::None,
            worker_tx,
            worker_rx,
            providers: HashMap::new(),
            create_agent_in_flight: false,
            last_pty_size: (0, 0),
            last_diff_height: 0,
            theme: Theme::default_dark(),
            tick_count: 0,
            watched_worktree: Arc::clone(&watched_worktree),
            has_active_agent: Arc::new(AtomicBool::new(false)),
            collapsed_projects: HashSet::new(),
            left_items_cache: Vec::new(),
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
        if self.help_overlay {
            self.help_overlay = false;
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
            "reconnect-agent" => self.reconnect_selected_session(),
            "add-project" => self.open_project_browser(),
            "copy-path" => self.copy_selected_path(),
            "toggle-project" => {
                self.toggle_collapse_selected_project();
                Ok(())
            }
            "toggle-sidebar" => {
                self.left_collapsed = !self.left_collapsed;
                Ok(())
            }
            "help" => {
                self.help_overlay = true;
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
                self.collapsed_projects.insert(id);
            }
            self.rebuild_left_items();
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
        }
    }

    pub(crate) fn current_files_len(&self) -> usize {
        match self.right_section {
            RightSection::Staged => self.staged_files.len(),
            RightSection::Unstaged => self.unstaged_files.len(),
        }
    }

    pub(crate) fn clamp_files_cursor(&mut self) {
        let len = self.current_files_len();
        if len == 0 {
            self.files_index = 0;
        } else if self.files_index >= len {
            self.files_index = len.saturating_sub(1);
        }
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
            args: Vec::new(),
        })
}
