use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
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

use crate::acp::{AcpClient, ProviderEvent};
use crate::config::{
    Config, DuxPaths, ProjectConfig, ProviderCommandConfig, ensure_config, save_config,
};
use crate::git;
use crate::logger;
use crate::model::{AgentSession, ChangedFile, Project, ProviderKind, SessionStatus};
use crate::statusline::{StatusLine, StatusTone};
use crate::storage::SessionStore;
use crate::terminal::{TerminalKind, TerminalOutput, TerminalSession};
use crate::theme::Theme;

pub struct App {
    config: Config,
    paths: DuxPaths,
    session_store: SessionStore,
    projects: Vec<Project>,
    sessions: Vec<AgentSession>,
    changed_files: Vec<ChangedFile>,
    selected_left: usize,
    selected_file: usize,
    left_width_pct: u16,
    right_width_pct: u16,
    right_top_height_pct: u16,
    focus: FocusPane,
    center_mode: CenterMode,
    resize_mode: bool,
    help_overlay: bool,
    status: StatusLine,
    prompt: PromptState,
    input_buffer: String,
    input_target: InputTarget,
    provider_tx: Sender<ProviderEvent>,
    provider_rx: Receiver<ProviderEvent>,
    shell_tx: Sender<TerminalOutput>,
    shell_rx: Receiver<TerminalOutput>,
    worker_tx: Sender<WorkerEvent>,
    worker_rx: Receiver<WorkerEvent>,
    providers: HashMap<String, AcpClient>,
    provider_buffers: HashMap<String, Vec<String>>,
    shell_terminals: HashMap<String, TerminalSession>,
    create_agent_in_flight: bool,
    theme: Theme,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FocusPane {
    Left,
    Center,
    Files,
    Shell,
}

impl FocusPane {
    fn next(self) -> Self {
        match self {
            Self::Left => Self::Center,
            Self::Center => Self::Files,
            Self::Files => Self::Shell,
            Self::Shell => Self::Left,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Left => Self::Shell,
            Self::Center => Self::Left,
            Self::Files => Self::Center,
            Self::Shell => Self::Files,
        }
    }
}

#[derive(Clone, Debug)]
enum CenterMode {
    Agent,
    Diff(String),
}

#[derive(Clone, Debug)]
enum PromptField {
    Path,
    Name,
}

#[derive(Clone, Debug)]
enum PromptState {
    None,
    Command {
        input: String,
        selected: usize,
        direct: bool,
    },
    BrowseProjects {
        current_dir: PathBuf,
        entries: Vec<BrowserEntry>,
        selected: usize,
    },
    AddProject {
        path: String,
        name: String,
        field: PromptField,
    },
}

#[derive(Clone, Debug)]
struct BrowserEntry {
    path: PathBuf,
    label: String,
    is_git_repo: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InputTarget {
    None,
    Agent,
    Shell,
}

#[derive(Clone, Debug)]
enum LeftItem {
    Project(usize),
    Session(usize),
}

enum WorkerEvent {
    CreateAgentProgress(String),
    CreateAgentReady {
        session: AgentSession,
        client: AcpClient,
    },
    CreateAgentFailed(String),
}

#[derive(Clone, Copy)]
struct CommandDef {
    name: &'static str,
    description: &'static str,
}

const COMMANDS: &[CommandDef] = &[
    CommandDef {
        name: "new-agent",
        description: "Create a new agent for the selected project",
    },
    CommandDef {
        name: "provider",
        description: "Toggle the selected project's default provider",
    },
    CommandDef {
        name: "refresh-project",
        description: "Git pull the selected project checkout",
    },
    CommandDef {
        name: "delete-project",
        description: "Remove the selected project and its sessions",
    },
    CommandDef {
        name: "delete-agent",
        description: "Delete the selected agent session",
    },
    CommandDef {
        name: "reconnect-agent",
        description: "Reconnect the selected detached agent session",
    },
    CommandDef {
        name: "add-project",
        description: "Open the project browser",
    },
    CommandDef {
        name: "add-project-manual",
        description: "Open manual project entry",
    },
    CommandDef {
        name: "help",
        description: "Open the help overlay",
    },
];

impl App {
    pub fn bootstrap() -> Result<Self> {
        let paths = DuxPaths::discover()?;
        let config = ensure_config(&paths)?;
        logger::init(&config.logging, &paths);
        logger::info("bootstrapping dux");
        let session_store = SessionStore::open(&paths.sessions_db_path)?;
        let projects = load_projects(&config);
        let sessions = session_store.load_sessions()?;
        let (provider_tx, provider_rx) = mpsc::channel();
        let (shell_tx, shell_rx) = mpsc::channel();
        let (worker_tx, worker_rx) = mpsc::channel();
        let mut app = Self {
            left_width_pct: config.ui.left_width_pct,
            right_width_pct: config.ui.right_width_pct,
            right_top_height_pct: config.ui.right_top_height_pct,
            config,
            paths,
            session_store,
            projects,
            sessions,
            changed_files: Vec::new(),
            selected_left: 0,
            selected_file: 0,
            focus: FocusPane::Left,
            center_mode: CenterMode::Agent,
            resize_mode: false,
            help_overlay: false,
            status: StatusLine::new("Press p to add a project, a to create an agent, ? for help."),
            prompt: PromptState::None,
            input_buffer: String::new(),
            input_target: InputTarget::None,
            provider_tx,
            provider_rx,
            shell_tx,
            shell_rx,
            worker_tx,
            worker_rx,
            providers: HashMap::new(),
            provider_buffers: HashMap::new(),
            shell_terminals: HashMap::new(),
            create_agent_in_flight: false,
            theme: Theme::default_dark(),
        };
        app.restore_sessions();
        app.reload_changed_files();
        Ok(app)
    }

    pub fn run(&mut self) -> Result<()> {
        let mut terminal = ratatui::init();
        loop {
            self.drain_events();
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
        let existing = self.sessions.clone();
        for session in existing {
            if Path::new(&session.worktree_path).exists() {
                if let Err(err) = self.spawn_shell_for_session(&session) {
                    self.push_provider_line(&session.id, format!("shell restore failed: {err}"));
                }
                if let Some(acp_session_id) = session.acp_session_id.clone() {
                    match self.connect_provider(&session, true, Some(acp_session_id)) {
                        Ok(client) => {
                            self.providers.insert(session.id.clone(), client);
                            self.mark_session_status(&session.id, SessionStatus::Active);
                        }
                        Err(err) => {
                            self.mark_session_status(&session.id, SessionStatus::Detached);
                            self.push_provider_line(
                                &session.id,
                                format!("restore failed, session kept detached: {err}"),
                            );
                        }
                    }
                } else {
                    self.mark_session_status(&session.id, SessionStatus::Detached);
                    self.push_provider_line(
                        &session.id,
                        "No ACP session id was stored. Start a new agent for this project."
                            .to_string(),
                    );
                }
            } else {
                self.mark_session_status(&session.id, SessionStatus::Exited);
                self.push_provider_line(
                    &session.id,
                    "Stored worktree path no longer exists.".to_string(),
                );
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        if key.code == KeyCode::Esc && self.close_top_overlay() {
            return Ok(false);
        }
        if !matches!(self.prompt, PromptState::None) {
            return self.handle_prompt_key(key);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Ok(true);
        }
        if key.code == KeyCode::Char('q') {
            return Ok(true);
        }
        if key.code == KeyCode::Char('?') {
            self.help_overlay = !self.help_overlay;
            return Ok(false);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('p') {
            self.prompt = PromptState::Command {
                input: String::new(),
                selected: 0,
                direct: false,
            };
            self.set_info("Command palette opened.");
            return Ok(false);
        }
        if key.code == KeyCode::Char(':') {
            self.prompt = PromptState::Command {
                input: String::new(),
                selected: 0,
                direct: true,
            };
            self.set_info("Command mode opened.");
            return Ok(false);
        }
        if key.code == KeyCode::Tab {
            self.focus = self.focus.next();
            self.input_target = InputTarget::None;
            self.input_buffer.clear();
            return Ok(false);
        }
        if key.code == KeyCode::BackTab {
            self.focus = self.focus.previous();
            self.input_target = InputTarget::None;
            self.input_buffer.clear();
            return Ok(false);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('w') {
            self.resize_mode = !self.resize_mode;
            if self.resize_mode {
                self.set_info("Resize mode on: h/l resize side panes, j/k resize right split.");
            } else {
                self.set_info("Resize mode off.");
            }
            return Ok(false);
        }
        if self.resize_mode {
            self.handle_resize_key(key);
            return Ok(false);
        }
        if self.input_target == InputTarget::Agent {
            return self.handle_agent_input(key);
        }
        if self.input_target == InputTarget::Shell {
            self.handle_shell_input(key)?;
            return Ok(false);
        }

        match self.focus {
            FocusPane::Left => self.handle_left_key(key)?,
            FocusPane::Center => self.handle_center_key(key)?,
            FocusPane::Files => self.handle_files_key(key)?,
            FocusPane::Shell => self.handle_shell_key(key)?,
        }
        Ok(false)
    }

    fn handle_left_key(&mut self, key: KeyEvent) -> Result<()> {
        let items = self.left_items();
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if self.selected_left + 1 < items.len() {
                    self.selected_left += 1;
                    self.reload_changed_files();
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.selected_left > 0 {
                    self.selected_left -= 1;
                    self.reload_changed_files();
                }
            }
            KeyCode::Enter => {
                self.center_mode = CenterMode::Agent;
                self.focus = FocusPane::Center;
                self.reload_changed_files();
            }
            KeyCode::Char('p') => {
                self.open_project_browser()?;
            }
            KeyCode::Char('P') => {
                self.prompt = PromptState::AddProject {
                    path: String::new(),
                    name: String::new(),
                    field: PromptField::Path,
                };
            }
            KeyCode::Char('a') => self.create_agent_for_selected_project()?,
            KeyCode::Char('u') => self.refresh_selected_project()?,
            KeyCode::Char('x') => self.delete_selected_session()?,
            KeyCode::Char('d') => self.cycle_selected_project_provider()?,
            KeyCode::Char('r') => self.reconnect_selected_session()?,
            _ => {}
        }
        Ok(())
    }

    fn handle_center_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Char('i') => {
                self.input_target = InputTarget::Agent;
                self.input_buffer.clear();
                self.set_info("Agent prompt mode. Type a prompt and press Enter. Esc cancels.");
            }
            KeyCode::Esc => {
                self.center_mode = CenterMode::Agent;
                self.set_info("Returned to agent view.");
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_files_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                if self.selected_file + 1 < self.changed_files.len() {
                    self.selected_file += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.selected_file > 0 {
                    self.selected_file -= 1;
                }
            }
            KeyCode::Enter => self.open_diff_for_selected_file()?,
            _ => {}
        }
        Ok(())
    }

    fn handle_shell_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.code == KeyCode::Char('i') {
            self.input_target = InputTarget::Shell;
            self.set_info("Shell input mode. Esc exits input mode.");
        }
        Ok(())
    }

    fn handle_agent_input(&mut self, key: KeyEvent) -> Result<bool> {
        match key.code {
            KeyCode::Esc => {
                self.input_target = InputTarget::None;
                self.input_buffer.clear();
                self.set_info("Agent prompt cancelled.");
            }
            KeyCode::Backspace => {
                self.input_buffer.pop();
            }
            KeyCode::Enter => {
                let prompt = self.input_buffer.trim().to_string();
                if !prompt.is_empty() {
                    self.submit_prompt(prompt)?;
                }
                self.input_buffer.clear();
                self.input_target = InputTarget::None;
            }
            KeyCode::Char(c) => {
                if !key.modifiers.contains(KeyModifiers::CONTROL) {
                    self.input_buffer.push(c);
                }
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_shell_input(&mut self, key: KeyEvent) -> Result<()> {
        if key.code == KeyCode::Esc {
            self.input_target = InputTarget::None;
            self.set_info("Shell input mode off.");
            return Ok(());
        }
        let Some(session_id) = self.selected_session().map(|session| session.id.clone()) else {
            return Ok(());
        };
        if let Some(shell) = self.shell_terminals.get_mut(&session_id) {
            match key.code {
                KeyCode::Enter => shell.send("\n")?,
                KeyCode::Backspace => shell.send("\u{7f}")?,
                KeyCode::Tab => shell.send("\t")?,
                KeyCode::Left => shell.send("\u{1b}[D")?,
                KeyCode::Right => shell.send("\u{1b}[C")?,
                KeyCode::Up => shell.send("\u{1b}[A")?,
                KeyCode::Down => shell.send("\u{1b}[B")?,
                KeyCode::Char(c) => {
                    if !key.modifiers.contains(KeyModifiers::CONTROL) {
                        shell.send(&c.to_string())?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn handle_prompt_key(&mut self, key: KeyEvent) -> Result<bool> {
        if let PromptState::Command {
            input,
            selected,
            direct: _,
        } = &mut self.prompt
        {
            match key.code {
                KeyCode::Esc => self.prompt = PromptState::None,
                KeyCode::Char('j') | KeyCode::Down => {
                    let count = filtered_commands(input).len();
                    if *selected + 1 < count {
                        *selected += 1;
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    if *selected > 0 {
                        *selected -= 1;
                    }
                }
                KeyCode::Backspace => {
                    input.pop();
                    *selected = 0;
                }
                KeyCode::Tab => {
                    if let Some(command) = filtered_commands(input).get(*selected) {
                        *input = command.name.to_string();
                        *selected = 0;
                    }
                }
                KeyCode::Enter => {
                    let command = if let Some(command) = filtered_commands(input).get(*selected) {
                        command.name.to_string()
                    } else {
                        input.trim().to_string()
                    };
                    self.prompt = PromptState::None;
                    self.execute_command(command)?;
                }
                KeyCode::Char(c) => {
                    if !key.modifiers.contains(KeyModifiers::CONTROL) {
                        input.push(c);
                        *selected = 0;
                    }
                }
                _ => {}
            }
            return Ok(false);
        }

        if let PromptState::BrowseProjects {
            current_dir,
            entries,
            selected,
        } = &mut self.prompt
        {
            match key.code {
                KeyCode::Esc => self.prompt = PromptState::None,
                KeyCode::Char('j') | KeyCode::Down => {
                    if *selected + 1 < entries.len() {
                        *selected += 1;
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    if *selected > 0 {
                        *selected -= 1;
                    }
                }
                KeyCode::Backspace | KeyCode::Left | KeyCode::Char('h') => {
                    if let Some(parent) = current_dir.parent() {
                        *current_dir = parent.to_path_buf();
                        let new_entries = browser_entries(current_dir);
                        *entries = new_entries;
                        *selected = 0;
                    }
                }
                KeyCode::Char('m') => {
                    self.prompt = PromptState::AddProject {
                        path: current_dir.to_string_lossy().to_string(),
                        name: String::new(),
                        field: PromptField::Path,
                    };
                }
                KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') => {
                    if let Some(entry) = entries.get(*selected).cloned() {
                        if entry.is_git_repo {
                            self.add_project(
                                entry.path.to_string_lossy().to_string(),
                                String::new(),
                            )?;
                            self.prompt = PromptState::None;
                        } else {
                            *current_dir = entry.path;
                            *entries = browser_entries(current_dir);
                            *selected = 0;
                        }
                    }
                }
                _ => {}
            }
            return Ok(false);
        }

        let mut submit = None;
        if let PromptState::AddProject { path, name, field } = &mut self.prompt {
            match key.code {
                KeyCode::Esc => self.prompt = PromptState::None,
                KeyCode::Tab => {
                    *field = match field {
                        PromptField::Path => PromptField::Name,
                        PromptField::Name => PromptField::Path,
                    };
                }
                KeyCode::Backspace => match field {
                    PromptField::Path => {
                        path.pop();
                    }
                    PromptField::Name => {
                        name.pop();
                    }
                },
                KeyCode::Enter => {
                    submit = Some((path.clone(), name.clone()));
                }
                KeyCode::Char(c) => {
                    if !key.modifiers.contains(KeyModifiers::CONTROL) {
                        match field {
                            PromptField::Path => path.push(c),
                            PromptField::Name => name.push(c),
                        }
                    }
                }
                _ => {}
            }
        }
        if let Some((path, name)) = submit {
            self.add_project(path, name)?;
            self.prompt = PromptState::None;
        }
        Ok(false)
    }

    fn handle_resize_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('h') => {
                self.left_width_pct = self.left_width_pct.saturating_sub(2).max(18)
            }
            KeyCode::Char('l') => {
                self.left_width_pct = self.left_width_pct.saturating_add(2).min(38)
            }
            KeyCode::Char('j') => {
                self.right_top_height_pct = self.right_top_height_pct.saturating_add(3).min(75)
            }
            KeyCode::Char('k') => {
                self.right_top_height_pct = self.right_top_height_pct.saturating_sub(3).max(20)
            }
            _ => {}
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::Down(_) => {
                self.set_info(
                    "Mouse support is available for wheel navigation; resize has a keyboard fallback via Ctrl-w.",
                );
            }
            MouseEventKind::ScrollDown => match self.focus {
                FocusPane::Left => {
                    let items = self.left_items();
                    if self.selected_left + 1 < items.len() {
                        self.selected_left += 1;
                        self.reload_changed_files();
                    }
                }
                FocusPane::Files => {
                    if self.selected_file + 1 < self.changed_files.len() {
                        self.selected_file += 1;
                    }
                }
                _ => {}
            },
            MouseEventKind::ScrollUp => match self.focus {
                FocusPane::Left => {
                    if self.selected_left > 0 {
                        self.selected_left -= 1;
                        self.reload_changed_files();
                    }
                }
                FocusPane::Files => {
                    if self.selected_file > 0 {
                        self.selected_file -= 1;
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn open_project_browser(&mut self) -> Result<()> {
        let start_dir = std::env::var("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let entries = browser_entries(&start_dir);
        logger::debug(&format!(
            "opened project browser at {} with {} entries",
            start_dir.display(),
            entries.len()
        ));
        self.prompt = PromptState::BrowseProjects {
            current_dir: start_dir,
            entries,
            selected: 0,
        };
        self.set_info(
            "Project browser: Enter opens or adds a repo, h goes up, m switches to manual entry.",
        );
        Ok(())
    }

    fn add_project(&mut self, raw_path: String, name: String) -> Result<()> {
        let path = PathBuf::from(raw_path.trim());
        logger::info(&format!("attempting to add project {}", path.display()));
        if !path.exists() || !git::is_git_repo(&path) {
            logger::error(&format!("add project rejected for {}", path.display()));
            self.set_error(format!("{} is not a git repository.", path.display()));
            return Ok(());
        }
        if self
            .projects
            .iter()
            .any(|project| Path::new(&project.path) == path.as_path())
        {
            self.set_error(format!("{} is already registered.", path.display()));
            return Ok(());
        }
        let branch = git::current_branch(&path)?;
        let display_name = if name.trim().is_empty() {
            path.file_name()
                .and_then(|part| part.to_str())
                .unwrap_or("project")
                .to_string()
        } else {
            name.trim().to_string()
        };
        self.config.projects.push(ProjectConfig {
            path: path.to_string_lossy().to_string(),
            name: Some(display_name.clone()),
            default_provider: None,
        });
        save_config(&self.paths.config_path, &self.config)?;
        self.projects.push(Project {
            id: self
                .projects
                .iter()
                .map(|project| project.id)
                .max()
                .unwrap_or_default()
                + 1,
            name: display_name.clone(),
            path: path.to_string_lossy().to_string(),
            default_provider: self.config.default_provider(),
            current_branch: branch,
        });
        logger::info(&format!("registered project {}", path.display()));
        self.set_info(format!("Added project {display_name}"));
        Ok(())
    }

    fn create_agent_for_selected_project(&mut self) -> Result<()> {
        if self.create_agent_in_flight {
            self.set_error("An agent is already being created.");
            return Ok(());
        }
        let Some(project) = self.selected_project().cloned() else {
            self.set_error("Select a project first.");
            return Ok(());
        };
        logger::info(&format!("creating agent for project {}", project.path));
        self.create_agent_in_flight = true;
        self.set_busy(format!("Creating worktree for {}...", project.name));
        let paths = self.paths.clone();
        let config = self.config.clone();
        let provider_tx = self.provider_tx.clone();
        let worker_tx = self.worker_tx.clone();
        thread::spawn(move || {
            run_create_agent_job(project, paths, config, provider_tx, worker_tx);
        });
        Ok(())
    }

    fn connect_provider(
        &self,
        session: &AgentSession,
        restore: bool,
        acp_session_id: Option<String>,
    ) -> Result<AcpClient> {
        let provider_config = provider_config(&self.config, &session.provider);
        logger::debug(&format!(
            "spawning provider command {:?} {:?} in {}",
            provider_config.command, provider_config.args, session.worktree_path
        ));
        let client = AcpClient::spawn(
            &provider_config.command,
            &provider_config.args,
            Path::new(&session.worktree_path),
            &session.id,
            self.provider_tx.clone(),
        )?;
        client.initialize()?;
        logger::debug(&format!(
            "initialized ACP provider for session {}",
            session.id
        ));
        if restore {
            if let Some(acp_session_id) = acp_session_id {
                logger::info(&format!(
                    "loading ACP session {} for app session {}",
                    acp_session_id, session.id
                ));
                let _ = client.load_session(Path::new(&session.worktree_path), &acp_session_id)?;
            }
        }
        Ok(client)
    }

    fn spawn_shell_for_session(&mut self, session: &AgentSession) -> Result<()> {
        if self.shell_terminals.contains_key(&session.id) {
            return Ok(());
        }
        logger::debug(&format!("spawning shell for session {}", session.id));
        let shell = TerminalSession::spawn(
            TerminalKind::Shell,
            Path::new(&session.worktree_path),
            &self.config.shell.command,
            &self.config.shell.args,
            self.shell_tx.clone(),
        )?;
        self.shell_terminals.insert(session.id.clone(), shell);
        Ok(())
    }

    fn refresh_selected_project(&mut self) -> Result<()> {
        let Some(project) = self.selected_project().cloned() else {
            self.set_error("Select a project first.");
            return Ok(());
        };
        logger::info(&format!("refreshing project {}", project.path));
        let path = Path::new(&project.path);
        if git::is_dirty(path)? {
            self.set_error("Refresh blocked because the source checkout has uncommitted changes.");
            return Ok(());
        }
        let output = git::pull_current_branch(path)?;
        if let Some(existing) = self
            .projects
            .iter_mut()
            .find(|candidate| candidate.id == project.id)
        {
            existing.current_branch =
                git::current_branch(path).unwrap_or_else(|_| existing.current_branch.clone());
        }
        self.set_info(format!("Refreshed {}: {}", project.name, output.trim()));
        Ok(())
    }

    fn delete_selected_session(&mut self) -> Result<()> {
        let Some(session) = self.selected_session().cloned() else {
            self.set_error("Select a session first.");
            return Ok(());
        };
        logger::info(&format!(
            "deleting session {} at {}",
            session.id, session.worktree_path
        ));
        let Some(project) = self
            .projects
            .iter()
            .find(|project| project.id == session.project_id)
            .cloned()
        else {
            return Ok(());
        };
        git::remove_worktree(
            Path::new(&project.path),
            Path::new(&session.worktree_path),
            &session.branch_name,
        )?;
        self.providers.remove(&session.id);
        self.shell_terminals.remove(&session.id);
        self.provider_buffers.remove(&session.id);
        self.sessions.retain(|candidate| candidate.id != session.id);
        self.session_store.delete_session(&session.id)?;
        self.selected_left = self.selected_left.saturating_sub(1);
        self.reload_changed_files();
        self.set_info(format!("Deleted {}", session.branch_name));
        Ok(())
    }

    fn cycle_selected_project_provider(&mut self) -> Result<()> {
        let Some(project) = self.selected_project().cloned() else {
            self.set_error("Select a project first.");
            return Ok(());
        };
        let next = match project.default_provider {
            ProviderKind::Claude => ProviderKind::Codex,
            ProviderKind::Codex => ProviderKind::Claude,
        };
        if let Some(existing) = self
            .projects
            .iter_mut()
            .find(|candidate| candidate.id == project.id)
        {
            existing.default_provider = next.clone();
        }
        if let Some(project_config) = self
            .config
            .projects
            .iter_mut()
            .find(|candidate| Path::new(&candidate.path) == Path::new(&project.path))
        {
            project_config.default_provider = Some(next.as_str().to_string());
        }
        save_config(&self.paths.config_path, &self.config)?;
        self.set_info(format!(
            "Default provider for {} is now {}",
            project.name,
            next.as_str()
        ));
        Ok(())
    }

    fn delete_selected_project(&mut self) -> Result<()> {
        let Some(project) = self.selected_project().cloned() else {
            self.set_error("Select a project first.");
            return Ok(());
        };
        logger::info(&format!("deleting project {}", project.path));
        let session_ids = self
            .sessions
            .iter()
            .filter(|session| session.project_id == project.id)
            .map(|session| session.id.clone())
            .collect::<Vec<_>>();
        for session_id in session_ids {
            if let Some(index) = self
                .sessions
                .iter()
                .position(|session| session.id == session_id)
            {
                self.selected_left = self
                    .left_items()
                    .iter()
                    .position(
                        |item| matches!(item, LeftItem::Session(session_index) if *session_index == index),
                    )
                    .unwrap_or(self.selected_left);
                self.delete_selected_session()?;
            }
        }
        self.projects.retain(|candidate| candidate.id != project.id);
        self.config
            .projects
            .retain(|candidate| Path::new(&candidate.path) != Path::new(&project.path));
        save_config(&self.paths.config_path, &self.config)?;
        self.selected_left = self.selected_left.saturating_sub(1);
        self.reload_changed_files();
        self.set_info(format!("Deleted project {}", project.name));
        Ok(())
    }

    fn reconnect_selected_session(&mut self) -> Result<()> {
        let Some(session) = self.selected_session().cloned() else {
            self.set_error("Select a detached session first.");
            return Ok(());
        };
        logger::info(&format!("reconnecting session {}", session.id));
        if self.providers.contains_key(&session.id) {
            self.set_info("Session is already connected.");
            return Ok(());
        }
        let Some(acp_session_id) = session.acp_session_id.clone() else {
            self.set_error("This session has no stored ACP id.");
            return Ok(());
        };
        let client = self.connect_provider(&session, true, Some(acp_session_id))?;
        self.providers.insert(session.id.clone(), client);
        self.mark_session_status(&session.id, SessionStatus::Active);
        self.set_info(format!("Reconnected {}", session.branch_name));
        Ok(())
    }

    fn submit_prompt(&mut self, prompt: String) -> Result<()> {
        let Some(session) = self.selected_session().cloned() else {
            self.set_error("Select an agent session first.");
            return Ok(());
        };
        logger::info(&format!("submitting prompt for session {}", session.id));
        let Some(acp_session_id) = session.acp_session_id.clone() else {
            self.set_error("Selected session has no ACP id.");
            return Ok(());
        };
        if !self.providers.contains_key(&session.id) {
            self.set_error("Selected session is detached. Press r to reconnect.");
            return Ok(());
        }
        self.push_provider_line(&session.id, format!("> {prompt}"));
        let provider = self
            .providers
            .get(&session.id)
            .expect("provider must exist after contains_key");
        provider.prompt(
            acp_session_id,
            prompt,
            self.provider_tx.clone(),
            session.id.clone(),
        );
        self.set_info("Prompt sent.");
        Ok(())
    }

    fn open_diff_for_selected_file(&mut self) -> Result<()> {
        let Some(session) = self.selected_session() else {
            self.set_error("Select a session first.");
            return Ok(());
        };
        let Some(file) = self.changed_files.get(self.selected_file) else {
            return Ok(());
        };
        let diff = git::diff_for_file(Path::new(&session.worktree_path), &file.path)?;
        self.center_mode = CenterMode::Diff(diff);
        self.focus = FocusPane::Center;
        Ok(())
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.worker_rx.try_recv() {
            match event {
                WorkerEvent::CreateAgentProgress(message) => self.set_busy(message),
                WorkerEvent::CreateAgentReady { session, client } => {
                    self.create_agent_in_flight = false;
                    self.provider_buffers.insert(
                        session.id.clone(),
                        vec![format!(
                            "{} session ready in {}",
                            session.provider.as_str(),
                            session.worktree_path
                        )],
                    );
                    if let Err(err) = self.spawn_shell_for_session(&session) {
                        logger::error(&format!("shell spawn failed for {}: {err}", session.id));
                        self.set_error(format!("Shell failed to start: {err}"));
                        continue;
                    }
                    if let Err(err) = self.session_store.upsert_session(&session) {
                        logger::error(&format!(
                            "session store upsert failed for {}: {err}",
                            session.id
                        ));
                        self.set_error(format!("Failed to persist session: {err}"));
                        continue;
                    }
                    self.providers.insert(session.id.clone(), client);
                    self.sessions.insert(0, session.clone());
                    self.selected_left = self
                        .left_items()
                        .iter()
                        .position(|item| matches!(item, LeftItem::Session(index) if self.sessions.get(*index).map(|candidate| candidate.id.as_str()) == Some(session.id.as_str())))
                        .unwrap_or(0);
                    self.reload_changed_files();
                    self.set_info(format!("Created {}", session.branch_name));
                }
                WorkerEvent::CreateAgentFailed(message) => {
                    self.create_agent_in_flight = false;
                    self.set_error(message);
                }
            }
        }
        while let Ok(event) = self.provider_rx.try_recv() {
            logger::debug(&format!(
                "provider event for {}: {}",
                event.session_id, event.message
            ));
            self.push_provider_line(&event.session_id, event.message);
            self.mark_session_updated(&event.session_id);
        }
        while let Ok(_event) = self.shell_rx.try_recv() {
            self.reload_changed_files();
        }
        let mut exited = Vec::new();
        for (session_id, provider) in &mut self.providers {
            if provider.try_wait().ok().flatten().is_some() {
                exited.push(session_id.clone());
            }
        }
        for session_id in exited {
            self.providers.remove(&session_id);
            self.mark_session_status(&session_id, SessionStatus::Detached);
            self.push_provider_line(
                &session_id,
                "Provider process exited; session is now detached.".to_string(),
            );
        }
    }

    fn render(&self, frame: &mut Frame) {
        let [header, body, footer] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(4),
                Constraint::Length(3),
            ])
            .areas(frame.area());
        self.render_header(frame, header);
        let center_pct = 100u16
            .saturating_sub(self.left_width_pct + self.right_width_pct)
            .max(20);
        let [left, center, right] = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(self.left_width_pct),
                Constraint::Percentage(center_pct),
                Constraint::Percentage(self.right_width_pct),
            ])
            .areas(body);
        let [files, shell] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(self.right_top_height_pct),
                Constraint::Percentage(100 - self.right_top_height_pct),
            ])
            .areas(right);

        self.render_left(frame, left);
        self.render_center(frame, center);
        self.render_files(frame, files);
        self.render_shell(frame, shell);
        self.render_footer(frame, footer);
        self.render_overlay(frame);
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let bg = self.theme.header_bg;
        let sep_fg = self.theme.header_separator_fg;
        let label_fg = self.theme.header_label_fg;
        let mut spans = vec![
            Span::styled(
                " dux ",
                Style::default()
                    .fg(Color::White)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("v{}", env!("CARGO_PKG_VERSION")),
                Style::default().fg(label_fg).bg(bg),
            ),
        ];
        if let Some(project) = self.selected_project() {
            spans.push(Span::styled(" ╱ ", Style::default().fg(sep_fg).bg(bg)));
            spans.push(Span::styled(
                "project: ",
                Style::default().fg(label_fg).bg(bg),
            ));
            spans.push(Span::styled(
                project.name.clone(),
                Style::default()
                    .fg(Color::White)
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(" ╱ ", Style::default().fg(sep_fg).bg(bg)));
            spans.push(Span::styled(
                "branch: ",
                Style::default().fg(label_fg).bg(bg),
            ));
            spans.push(Span::styled(
                project.current_branch.clone(),
                Style::default().fg(Color::Cyan).bg(bg),
            ));
            spans.push(Span::styled(" ╱ ", Style::default().fg(sep_fg).bg(bg)));
            spans.push(Span::styled(
                "provider: ",
                Style::default().fg(label_fg).bg(bg),
            ));
            spans.push(Span::styled(
                project.default_provider.as_str().to_string(),
                Style::default().fg(self.theme.provider_label_fg).bg(bg),
            ));
        }
        Paragraph::new(Line::from(spans))
            .style(self.theme.header_style())
            .render(area, frame.buffer_mut());
    }

    fn render_left(&self, frame: &mut Frame, area: Rect) {
        let session_counts: HashMap<i64, usize> = {
            let mut counts = HashMap::new();
            for session in &self.sessions {
                *counts.entry(session.project_id).or_insert(0) += 1;
            }
            counts
        };
        let items = self
            .left_items()
            .into_iter()
            .map(|item| match item {
                LeftItem::Project(index) => {
                    let project = &self.projects[index];
                    let count = session_counts.get(&project.id).copied().unwrap_or(0);
                    let mut spans = vec![
                        Span::styled("▸ ", Style::default().fg(self.theme.project_icon)),
                        Span::raw(project.name.clone()),
                    ];
                    if count > 0 {
                        spans.push(Span::styled(
                            format!(" ({count})"),
                            Style::default().fg(self.theme.provider_label_fg),
                        ));
                    }
                    ListItem::new(Line::from(spans))
                }
                LeftItem::Session(index) => {
                    let session = &self.sessions[index];
                    let label = session
                        .title
                        .clone()
                        .unwrap_or_else(|| session.branch_name.clone());
                    let (dot, dot_color) = self.theme.session_dot(&session.status);
                    ListItem::new(Line::from(vec![
                        Span::raw("  "),
                        Span::styled(
                            format!("{dot} "),
                            Style::default().fg(dot_color),
                        ),
                        Span::styled(label, Style::default().fg(dot_color)),
                        Span::styled(
                            format!(" ({})", session.provider.as_str()),
                            Style::default().fg(self.theme.provider_label_fg),
                        ),
                    ]))
                }
            })
            .collect::<Vec<_>>();
        let focused = self.focus == FocusPane::Left;
        let title = format!(
            "Projects ({})",
            self.projects.len()
        );
        let mut state = ListState::default().with_selected(Some(self.selected_left));
        StatefulWidget::render(
            List::new(items)
                .block(self.themed_block(&title, focused))
                .highlight_style(self.theme.selection_style()),
            area,
            frame.buffer_mut(),
            &mut state,
        );
    }

    fn render_center(&self, frame: &mut Frame, area: Rect) {
        let title = match self.center_mode {
            CenterMode::Agent => "Agent",
            CenterMode::Diff(_) => "Diff",
        };
        let focused = self.focus == FocusPane::Center;
        match &self.center_mode {
            CenterMode::Diff(diff) => {
                let styled_lines: Vec<Line> = if diff.trim().is_empty() {
                    vec![Line::from("No diff for this file.")]
                } else {
                    diff.lines()
                        .map(|line| {
                            if line.starts_with("+++") || line.starts_with("---") {
                                Line::from(Span::styled(
                                    line.to_string(),
                                    Style::default()
                                        .fg(self.theme.diff_file_header)
                                        .add_modifier(Modifier::BOLD),
                                ))
                            } else if line.starts_with("@@") {
                                Line::from(Span::styled(
                                    line.to_string(),
                                    Style::default().fg(self.theme.diff_hunk),
                                ))
                            } else if line.starts_with('+') {
                                Line::from(Span::styled(
                                    line.to_string(),
                                    Style::default().fg(self.theme.diff_add),
                                ))
                            } else if line.starts_with('-') {
                                Line::from(Span::styled(
                                    line.to_string(),
                                    Style::default().fg(self.theme.diff_remove),
                                ))
                            } else {
                                Line::from(line.to_string())
                            }
                        })
                        .collect()
                };
                Paragraph::new(styled_lines)
                    .block(self.themed_block(title, focused))
                    .wrap(Wrap { trim: false })
                    .render(area, frame.buffer_mut());
            }
            CenterMode::Agent => {
                let mut lines = self
                    .selected_session()
                    .and_then(|session| self.provider_buffers.get(&session.id))
                    .cloned()
                    .unwrap_or_else(|| vec!["No active agent session selected.".to_string()]);
                if self.input_target == InputTarget::Agent {
                    lines.push(String::new());
                    lines.push(format!("Prompt> {}", self.input_buffer));
                }
                Paragraph::new(lines.join("\n"))
                    .block(self.themed_block(title, focused))
                    .wrap(Wrap { trim: false })
                    .render(area, frame.buffer_mut());
            }
        }
    }

    fn render_files(&self, frame: &mut Frame, area: Rect) {
        let width = area.width.saturating_sub(6) as usize;
        let items = self
            .changed_files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                let path = if index == self.selected_file {
                    file.path.clone()
                } else {
                    git::ellipsize_middle(&file.path, width.max(10))
                };
                ListItem::new(Line::from(vec![
                    Span::styled(
                        format!("{:>2} ", file.status),
                        Style::default().fg(self.theme.file_status_fg),
                    ),
                    Span::raw(path),
                ]))
            })
            .collect::<Vec<_>>();
        let focused = self.focus == FocusPane::Files;
        let title = format!("Changed Files ({})", self.changed_files.len());
        let mut state = ListState::default().with_selected(Some(self.selected_file));
        StatefulWidget::render(
            List::new(items)
                .block(self.themed_block(&title, focused))
                .highlight_style(self.theme.selection_style()),
            area,
            frame.buffer_mut(),
            &mut state,
        );
    }

    fn render_shell(&self, frame: &mut Frame, area: Rect) {
        let mut lines = self
            .selected_session()
            .and_then(|session| self.shell_terminals.get(&session.id))
            .map(TerminalSession::snapshot)
            .unwrap_or_else(|| vec!["No shell attached to the current session.".to_string()]);
        if self.input_target == InputTarget::Shell {
            lines.push(String::new());
            lines.push("[shell input mode]".to_string());
        }
        let focused = self.focus == FocusPane::Shell;
        Paragraph::new(lines.join("\n"))
            .block(self.themed_block("Shell", focused))
            .wrap(Wrap { trim: false })
            .render(area, frame.buffer_mut());
    }

    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        let hints: &[(&str, &str)] = match self.focus {
            FocusPane::Left => &[
                ("j/k", "Move"),
                ("a", "Agent"),
                ("p", "Add"),
                ("^P", "Palette"),
                (":", "Cmd"),
                ("d", "Provider"),
                ("u", "Pull"),
                ("?", "Help"),
                ("q", "Quit"),
            ],
            FocusPane::Center => &[
                ("i", "Prompt"),
                ("^P", "Palette"),
                (":", "Cmd"),
                ("Esc", "Close diff"),
                ("Tab", "Next"),
                ("?", "Help"),
                ("q", "Quit"),
            ],
            FocusPane::Files => &[
                ("j/k", "Move"),
                ("Enter", "Diff"),
                ("^P", "Palette"),
                (":", "Cmd"),
                ("Tab", "Next"),
                ("?", "Help"),
                ("q", "Quit"),
            ],
            FocusPane::Shell => &[
                ("i", "Input"),
                ("^P", "Palette"),
                (":", "Cmd"),
                ("Esc", "Leave"),
                ("Tab", "Next"),
                ("?", "Help"),
                ("q", "Quit"),
            ],
        };
        let mut hint_spans: Vec<Span> = Vec::new();
        let bar_bg = self.theme.hint_bar_bg;
        for (i, (key, desc)) in hints.iter().enumerate() {
            if i > 0 {
                hint_spans.push(Span::styled(" ", Style::default().bg(bar_bg)));
            }
            hint_spans.extend(self.theme.key_badge(key, bar_bg));
            hint_spans.push(Span::styled(
                format!(" {desc}"),
                Style::default().fg(self.theme.hint_desc_fg).bg(bar_bg),
            ));
        }

        let [hints_area, status_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Length(2)])
            .areas(area);
        Paragraph::new(Line::from(hint_spans))
            .style(Style::default().bg(self.theme.hint_bar_bg))
            .render(hints_area, frame.buffer_mut());

        let tone = self.status.tone();
        let (dot, dot_color) = self.theme.status_dot(tone);
        let status_text = self.status.text();
        let msg_color = match tone {
            StatusTone::Info => self.theme.status_info_fg,
            StatusTone::Busy => self.theme.status_busy_fg,
            StatusTone::Error => self.theme.status_error_fg,
        };
        let status_bg = match tone {
            StatusTone::Info => self.theme.status_info_bg,
            StatusTone::Busy => self.theme.status_busy_bg,
            StatusTone::Error => self.theme.status_error_bg,
        };
        let status_line = Line::from(vec![
            Span::styled(
                format!(" {dot} "),
                Style::default().fg(dot_color).bg(status_bg),
            ),
            Span::styled(
                status_text,
                Style::default().fg(msg_color).bg(status_bg),
            ),
        ]);
        Paragraph::new(status_line)
            .style(Style::default().bg(status_bg))
            .wrap(Wrap { trim: false })
            .render(status_area, frame.buffer_mut());
    }

    fn render_help(&self, frame: &mut Frame) {
        self.render_dim_overlay(frame);
        let area = centered_rect(72, 70, frame.area());
        Clear.render(area, frame.buffer_mut());
        let help_bindings: &[(&str, &[(&str, &str)])] = &[
            (
                "Global",
                &[
                    ("Tab", "Focus next pane"),
                    ("Ctrl-p", "Open command palette"),
                    (":", "Open command mode"),
                    ("Ctrl-w", "Resize mode (h/l side, j/k split)"),
                    ("?", "Toggle help"),
                    ("q", "Quit"),
                ],
            ),
            (
                "Left pane",
                &[
                    ("j/k", "Move through projects and sessions"),
                    ("p", "Open project browser"),
                    ("P", "Open manual path entry"),
                    ("a", "Create worktree-backed agent session"),
                    ("d", "Cycle default provider"),
                    ("u", "Refresh checkout (git pull --ff-only)"),
                    ("r", "Reconnect detached ACP session"),
                    ("x", "Delete selected session/worktree"),
                ],
            ),
            (
                "Center pane",
                &[
                    ("i", "Start a prompt turn for the agent"),
                    ("Esc", "Close diff view"),
                ],
            ),
            (
                "Files pane",
                &[("Enter", "Open selected file diff")],
            ),
            (
                "Shell pane",
                &[("i", "Send raw input to worktree shell")],
            ),
        ];
        let mut lines: Vec<Line> = Vec::new();
        for (section_idx, (section, bindings)) in help_bindings.iter().enumerate() {
            if section_idx > 0 {
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled(
                section.to_string(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
            )));
            for (key, desc) in *bindings {
                let padding = 14usize.saturating_sub(key.len() + 2);
                let mut spans = vec![Span::raw("  ")];
                spans.extend(self.theme.key_badge(key, Color::Reset));
                spans.push(Span::raw(" ".repeat(padding)));
                spans.push(Span::styled(
                    desc.to_string(),
                    Style::default().fg(self.theme.hint_desc_fg),
                ));
                lines.push(Line::from(spans));
            }
        }
        Paragraph::new(lines)
            .block(self.themed_overlay_block("Help"))
            .wrap(Wrap { trim: false })
            .render(area, frame.buffer_mut());
    }

    fn render_prompt(&self, frame: &mut Frame) {
        match &self.prompt {
            PromptState::Command {
                input,
                selected,
                direct,
            } => {
                self.render_dim_overlay(frame);
                let popup = centered_rect(72, 40, frame.area());
                Clear.render(popup, frame.buffer_mut());
                let commands = filtered_commands(input);
                let items = if commands.is_empty() {
                    vec![ListItem::new("No matching commands.")]
                } else {
                    commands
                        .iter()
                        .map(|command| {
                            ListItem::new(Line::from(vec![
                                Span::styled(
                                    command.name.to_string(),
                                    Style::default()
                                        .fg(Color::Cyan)
                                        .add_modifier(Modifier::BOLD),
                                ),
                                Span::styled(
                                    format!("  {}", command.description),
                                    Style::default().fg(self.theme.hint_desc_fg),
                                ),
                            ]))
                        })
                        .collect::<Vec<_>>()
                };
                let mut state = ListState::default()
                    .with_selected(Some((*selected).min(commands.len().saturating_sub(1))));
                let [input_area, list_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(3), Constraint::Min(3)])
                    .areas(popup);
                let title = if *direct {
                    "Command"
                } else {
                    "Command Palette"
                };
                let mut bottom_spans = vec![Span::raw(" ")];
                bottom_spans.extend(self.theme.key_badge("Enter", Color::Reset));
                bottom_spans.push(Span::styled(" run  ", Style::default().fg(self.theme.hint_desc_fg)));
                bottom_spans.extend(self.theme.key_badge("Tab", Color::Reset));
                bottom_spans.push(Span::styled(" complete  ", Style::default().fg(self.theme.hint_desc_fg)));
                bottom_spans.extend(self.theme.key_badge("Esc", Color::Reset));
                bottom_spans.push(Span::styled(" cancel", Style::default().fg(self.theme.hint_desc_fg)));
                Paragraph::new(format!("{}{}", if *direct { ":" } else { "> " }, input))
                    .block(
                        self.themed_overlay_block(title)
                            .title_bottom(Line::from(bottom_spans)),
                    )
                    .render(input_area, frame.buffer_mut());
                StatefulWidget::render(
                    List::new(items)
                        .block(
                            Block::default()
                                .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                                .border_style(Style::default().fg(self.theme.overlay_border)),
                        )
                        .highlight_style(self.theme.selection_style()),
                    list_area,
                    frame.buffer_mut(),
                    &mut state,
                );
            }
            PromptState::BrowseProjects {
                current_dir,
                entries,
                selected,
            } => {
                self.render_dim_overlay(frame);
                let area = centered_rect(72, 70, frame.area());
                Clear.render(area, frame.buffer_mut());
                let items = if entries.is_empty() {
                    vec![ListItem::new("No child directories here.")]
                } else {
                    entries
                        .iter()
                        .map(|entry| {
                            let (prefix, prefix_color) = if entry.is_git_repo {
                                ("●", Color::Green)
                            } else {
                                ("○", self.theme.provider_label_fg)
                            };
                            ListItem::new(Line::from(vec![
                                Span::styled(
                                    format!("{prefix} "),
                                    Style::default().fg(prefix_color),
                                ),
                                Span::raw(entry.label.clone()),
                            ]))
                        })
                        .collect::<Vec<_>>()
                };
                let mut state = ListState::default()
                    .with_selected(Some((*selected).min(entries.len().saturating_sub(1))));
                StatefulWidget::render(
                    List::new(items)
                        .block(
                            self.themed_overlay_block(&format!(
                                "Add Project: {}",
                                current_dir.display()
                            ))
                            .title_bottom({
                                let mut s = vec![Span::raw(" ")];
                                s.extend(self.theme.key_badge("Enter", Color::Reset));
                                s.push(Span::styled(" open  ", Style::default().fg(self.theme.hint_desc_fg)));
                                s.extend(self.theme.key_badge("h", Color::Reset));
                                s.push(Span::styled(" up  ", Style::default().fg(self.theme.hint_desc_fg)));
                                s.extend(self.theme.key_badge("m", Color::Reset));
                                s.push(Span::styled(" manual  ", Style::default().fg(self.theme.hint_desc_fg)));
                                s.extend(self.theme.key_badge("Esc", Color::Reset));
                                s.push(Span::styled(" cancel", Style::default().fg(self.theme.hint_desc_fg)));
                                Line::from(s)
                            }),
                        )
                        .highlight_style(self.theme.selection_style()),
                    area,
                    frame.buffer_mut(),
                    &mut state,
                );
            }
            PromptState::AddProject { path, name, field } => {
                self.render_dim_overlay(frame);
                let area = centered_rect(72, 70, frame.area());
                Clear.render(area, frame.buffer_mut());
                let cursor = "█";
                let path_cursor = if matches!(field, PromptField::Path) {
                    cursor
                } else {
                    ""
                };
                let name_cursor = if matches!(field, PromptField::Name) {
                    cursor
                } else {
                    ""
                };
                let lines = vec![
                    Line::from(""),
                    Line::from(vec![
                        Span::styled(
                            "  Path: ",
                            Style::default()
                                .fg(if matches!(field, PromptField::Path) {
                                    Color::Cyan
                                } else {
                                    self.theme.hint_desc_fg
                                })
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(format!("{path}{path_cursor}")),
                    ]),
                    Line::from(vec![
                        Span::styled(
                            "  Name: ",
                            Style::default()
                                .fg(if matches!(field, PromptField::Name) {
                                    Color::Cyan
                                } else {
                                    self.theme.hint_desc_fg
                                })
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw(format!("{name}{name_cursor}")),
                    ]),
                    Line::from(""),
                    Line::from({
                        let mut s: Vec<Span> = vec![Span::raw("  ")];
                        s.extend(self.theme.key_badge("Tab", Color::Reset));
                        s.push(Span::styled(" switch fields  ", Style::default().fg(self.theme.hint_desc_fg)));
                        s.extend(self.theme.key_badge("Enter", Color::Reset));
                        s.push(Span::styled(" save  ", Style::default().fg(self.theme.hint_desc_fg)));
                        s.extend(self.theme.key_badge("Esc", Color::Reset));
                        s.push(Span::styled(" cancel", Style::default().fg(self.theme.hint_desc_fg)));
                        s
                    }),
                ];
                Paragraph::new(lines)
                    .block(self.themed_overlay_block("Manual Project Entry"))
                    .wrap(Wrap { trim: false })
                    .render(area, frame.buffer_mut());
            }
            PromptState::None => {}
        }
    }

    fn render_overlay(&self, frame: &mut Frame) {
        if !matches!(self.prompt, PromptState::None) {
            self.render_prompt(frame);
            return;
        }
        if self.help_overlay {
            self.render_help(frame);
        }
    }

    fn close_top_overlay(&mut self) -> bool {
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
        false
    }

    fn set_info(&mut self, message: impl Into<String>) {
        self.status.info(message);
    }

    fn set_busy(&mut self, message: impl Into<String>) {
        self.status.busy(message);
    }

    fn set_error(&mut self, message: impl Into<String>) {
        self.status.error(message);
    }

    fn execute_command(&mut self, command: String) -> Result<()> {
        let command = command.trim().trim_start_matches(':');
        match command {
            "new-agent" => self.create_agent_for_selected_project(),
            "provider" => self.cycle_selected_project_provider(),
            "refresh-project" => self.refresh_selected_project(),
            "delete-project" => self.delete_selected_project(),
            "delete-agent" => self.delete_selected_session(),
            "reconnect-agent" => self.reconnect_selected_session(),
            "add-project" => self.open_project_browser(),
            "add-project-manual" => {
                self.prompt = PromptState::AddProject {
                    path: String::new(),
                    name: String::new(),
                    field: PromptField::Path,
                };
                Ok(())
            }
            "help" => {
                self.help_overlay = true;
                Ok(())
            }
            "" => Ok(()),
            other => {
                self.set_error(format!("Unknown command: {other}"));
                Ok(())
            }
        }
    }

    fn left_items(&self) -> Vec<LeftItem> {
        let mut items = Vec::new();
        for (project_index, project) in self.projects.iter().enumerate() {
            items.push(LeftItem::Project(project_index));
            for (session_index, session) in self.sessions.iter().enumerate() {
                if session.project_id == project.id {
                    items.push(LeftItem::Session(session_index));
                }
            }
        }
        items
    }

    fn selected_project(&self) -> Option<&Project> {
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

    fn selected_session(&self) -> Option<&AgentSession> {
        match self.left_items().get(self.selected_left) {
            Some(LeftItem::Session(index)) => self.sessions.get(*index),
            _ => None,
        }
    }

    fn reload_changed_files(&mut self) {
        self.changed_files = self
            .selected_session()
            .and_then(|session| git::changed_files(Path::new(&session.worktree_path)).ok())
            .unwrap_or_default();
        if self.selected_file >= self.changed_files.len() {
            self.selected_file = self.changed_files.len().saturating_sub(1);
        }
    }

    fn mark_session_status(&mut self, session_id: &str, status: SessionStatus) {
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

    fn mark_session_updated(&mut self, session_id: &str) {
        if let Some(session) = self
            .sessions
            .iter_mut()
            .find(|candidate| candidate.id == session_id)
        {
            session.updated_at = Utc::now();
            let _ = self.session_store.upsert_session(session);
        }
        self.reload_changed_files();
    }

    fn themed_block<'a>(&self, title: &'a str, focused: bool) -> Block<'a> {
        let focus_indicator = if focused { " █" } else { "" };
        Block::default()
            .title(Line::from(vec![
                Span::styled(title, self.theme.title_style(focused)),
                Span::styled(focus_indicator, Style::default().fg(self.theme.border_focused)),
            ]))
            .borders(Borders::ALL)
            .border_set(border::ROUNDED)
            .border_style(self.theme.border_style(focused))
    }

    fn themed_overlay_block<'a>(&self, title: &'a str) -> Block<'a> {
        Block::default()
            .title(Line::from(Span::styled(
                title,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )))
            .borders(Borders::ALL)
            .border_set(border::ROUNDED)
            .border_style(Style::default().fg(self.theme.overlay_border))
    }

    fn render_dim_overlay(&self, frame: &mut Frame) {
        let area = frame.area();
        let buf = frame.buffer_mut();
        for y in area.y..area.y + area.height {
            for x in area.x..area.x + area.width {
                let cell = &mut buf[(x, y)];
                cell.set_fg(Color::DarkGray);
                cell.set_bg(Color::Rgb(10, 10, 10));
            }
        }
    }

    fn push_provider_line(&mut self, session_id: &str, line: String) {
        if let Some(title) = line.strip_prefix("session: ") {
            if let Some(session) = self
                .sessions
                .iter_mut()
                .find(|candidate| candidate.id == session_id)
            {
                session.title = Some(title.to_string());
                let _ = self.session_store.upsert_session(session);
            }
        }
        let buffer = self
            .provider_buffers
            .entry(session_id.to_string())
            .or_default();
        for physical_line in line.lines() {
            if buffer.len() >= 500 {
                buffer.remove(0);
            }
            buffer.push(physical_line.to_string());
        }
    }
}

fn run_create_agent_job(
    project: Project,
    paths: DuxPaths,
    config: Config,
    provider_tx: Sender<ProviderEvent>,
    worker_tx: Sender<WorkerEvent>,
) {
    let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(format!(
        "Creating worktree for {}...",
        project.name
    )));
    let repo_path = PathBuf::from(&project.path);
    let (branch_name, worktree_path) =
        match git::create_worktree(&repo_path, &paths.worktrees_root, &project.name) {
            Ok(result) => result,
            Err(err) => {
                logger::error(&format!(
                    "worktree creation failed for {}: {err}",
                    project.path
                ));
                let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                    "Worktree creation failed: {err}"
                )));
                return;
            }
        };
    logger::info(&format!(
        "created worktree {} on branch {}",
        worktree_path.display(),
        branch_name
    ));
    let mut session = AgentSession {
        id: Uuid::new_v4().to_string(),
        project_id: project.id,
        provider: project.default_provider.clone(),
        source_branch: project.current_branch.clone(),
        branch_name,
        worktree_path: worktree_path.to_string_lossy().to_string(),
        acp_session_id: None,
        title: None,
        status: SessionStatus::Active,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(format!(
        "Launching {} adapter...",
        session.provider.as_str()
    )));
    let client = match connect_provider_background(&config, &session, provider_tx) {
        Ok(client) => client,
        Err(err) => {
            logger::error(&format!(
                "provider startup failed for {}: {err}",
                session.id
            ));
            let _ = git::remove_worktree(
                &repo_path,
                Path::new(&session.worktree_path),
                &session.branch_name,
            );
            let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                "Provider failed to start: {err}. Check {} and configure an ACP adapter.",
                paths.root.display()
            )));
            return;
        }
    };
    let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(
        "Requesting ACP session...".to_string(),
    ));
    let acp_session_id = match client.new_session(Path::new(&session.worktree_path)) {
        Ok(session_id) => session_id,
        Err(err) => {
            logger::error(&format!("session/new failed for {}: {err}", session.id));
            let _ = git::remove_worktree(
                &repo_path,
                Path::new(&session.worktree_path),
                &session.branch_name,
            );
            let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                "ACP session creation failed: {err}. Check {} and configure an ACP adapter.",
                paths.root.display()
            )));
            return;
        }
    };
    logger::info(&format!(
        "provider session created for {} with acp id {}",
        session.id, acp_session_id
    ));
    session.acp_session_id = Some(acp_session_id);
    let _ = worker_tx.send(WorkerEvent::CreateAgentReady { session, client });
}

fn connect_provider_background(
    config: &Config,
    session: &AgentSession,
    provider_tx: Sender<ProviderEvent>,
) -> Result<AcpClient> {
    let provider_config = provider_config(config, &session.provider);
    logger::debug(&format!(
        "spawning provider command {:?} {:?} in {}",
        provider_config.command, provider_config.args, session.worktree_path
    ));
    let client = AcpClient::spawn(
        &provider_config.command,
        &provider_config.args,
        Path::new(&session.worktree_path),
        &session.id,
        provider_tx,
    )?;
    client.initialize()?;
    logger::debug(&format!(
        "initialized ACP provider for session {}",
        session.id
    ));
    Ok(client)
}

fn load_projects(config: &Config) -> Vec<Project> {
    let mut projects = Vec::new();
    for (index, project) in config.projects.iter().enumerate() {
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
            id: index as i64 + 1,
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

fn provider_config(config: &Config, provider: &ProviderKind) -> ProviderCommandConfig {
    config
        .providers
        .get(provider.as_str())
        .cloned()
        .unwrap_or_else(|| ProviderCommandConfig {
            command: format!("{}-acp", provider.as_str()),
            args: Vec::new(),
        })
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area)[1];
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical)[1]
}

fn browser_entries(dir: &Path) -> Vec<BrowserEntry> {
    let mut entries = fs::read_dir(dir)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(Result::ok))
        .filter_map(|entry| {
            let path = entry.path();
            if !path.is_dir() {
                return None;
            }
            let label = entry.file_name().to_string_lossy().to_string();
            if label.starts_with('.') {
                return None;
            }
            Some(BrowserEntry {
                is_git_repo: git::is_git_repo(&path),
                path,
                label,
            })
        })
        .collect::<Vec<_>>();
    entries.sort_by(|a, b| {
        b.is_git_repo
            .cmp(&a.is_git_repo)
            .then_with(|| a.label.to_lowercase().cmp(&b.label.to_lowercase()))
    });
    entries
}

fn filtered_commands(input: &str) -> Vec<&'static CommandDef> {
    let needle = input.trim().trim_start_matches(':').to_lowercase();
    COMMANDS
        .iter()
        .filter(|command| {
            needle.is_empty()
                || command.name.contains(&needle)
                || command.description.to_lowercase().contains(&needle)
        })
        .collect()
}
