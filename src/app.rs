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

use crate::config::{
    Config, DuxPaths, ProjectConfig, ProviderCommandConfig, check_provider_available,
    ensure_config, save_config,
};
use crate::git;
use crate::logger;
use crate::model::{AgentSession, ChangedFile, Project, ProviderKind, SessionStatus};
use crate::pty::PtyClient;
use crate::statusline::{StatusLine, StatusTone};
use crate::storage::SessionStore;
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
    left_collapsed: bool,
    resize_mode: bool,
    help_overlay: bool,
    status: StatusLine,
    prompt: PromptState,
    input_target: InputTarget,
    worker_tx: Sender<WorkerEvent>,
    worker_rx: Receiver<WorkerEvent>,
    providers: HashMap<String, PtyClient>,
    create_agent_in_flight: bool,
    last_pty_size: (u16, u16),
    theme: Theme,
    tick_count: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FocusPane {
    Left,
    Center,
    Files,
}

impl FocusPane {
    fn next(self) -> Self {
        match self {
            Self::Left => Self::Center,
            Self::Center => Self::Files,
            Self::Files => Self::Left,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::Left => Self::Files,
            Self::Center => Self::Left,
            Self::Files => Self::Center,
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
        searching: bool,
    },
    BrowseProjects {
        current_dir: PathBuf,
        entries: Vec<BrowserEntry>,
        selected: usize,
        filter: String,
        searching: bool,
        editing_path: bool,
        path_input: String,
    },
    AddProject {
        path: String,
        name: String,
        field: PromptField,
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
        client: PtyClient,
        pty_size: (u16, u16), // (rows, cols) the PTY was spawned with
    },
    CreateAgentFailed(String),
}

#[derive(Clone, Copy)]
struct CommandDef {
    name: &'static str,
    description: &'static str,
    shortcut: Option<&'static str>,
}

const COMMANDS: &[CommandDef] = &[
    CommandDef {
        name: "new-agent",
        description: "Create a new agent for the selected project",
        shortcut: Some("n"),
    },
    CommandDef {
        name: "provider",
        description: "Toggle the selected project's default provider",
        shortcut: Some("d"),
    },
    CommandDef {
        name: "refresh-project",
        description: "Git pull the selected project checkout",
        shortcut: Some("u"),
    },
    CommandDef {
        name: "delete-project",
        description: "Remove the selected project and its sessions",
        shortcut: None,
    },
    CommandDef {
        name: "delete-agent",
        description: "Delete the selected agent session",
        shortcut: None,
    },
    CommandDef {
        name: "reconnect-agent",
        description: "Restart the CLI for the selected agent",
        shortcut: None,
    },
    CommandDef {
        name: "add-project",
        description: "Open the project browser",
        shortcut: Some("a"),
    },
    CommandDef {
        name: "add-project-manual",
        description: "Open manual project entry",
        shortcut: None,
    },
    CommandDef {
        name: "toggle-sidebar",
        description: "Collapse or expand the projects sidebar",
        shortcut: Some("["),
    },
    CommandDef {
        name: "copy-path",
        description: "Copy the selected agent's worktree path",
        shortcut: Some("y"),
    },
    CommandDef {
        name: "help",
        description: "Open the help overlay",
        shortcut: Some("?"),
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
            theme: Theme::default_dark(),
            tick_count: 0,
        };
        app.restore_sessions();
        app.reload_changed_files();
        Ok(app)
    }

    pub fn run(&mut self) -> Result<()> {
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

    fn handle_key(&mut self, key: KeyEvent) -> Result<bool> {
        if key.code == KeyCode::Esc && self.close_top_overlay() {
            return Ok(false);
        }
        if !matches!(self.prompt, PromptState::None) {
            return self.handle_prompt_key(key);
        }
        // In interactive mode, ALL keys go to the PTY except ctrl+g (handled
        // inside handle_agent_input). This must be checked before quit / help /
        // palette so that typing 'q', ctrl+c, '?', ctrl+p etc. reaches the CLI.
        if self.input_target == InputTarget::Agent {
            return self.handle_agent_input(key);
        }
        if (key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c'))
            || key.code == KeyCode::Char('q')
        {
            let active_count = self.providers.len();
            if active_count > 0 {
                self.prompt = PromptState::ConfirmQuit {
                    active_count,
                    confirm_selected: false,
                };
                return Ok(false);
            }
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
                searching: false,
            };
            self.set_info("Command palette opened.");
            return Ok(false);
        }
        if key.code == KeyCode::Tab {
            self.focus = self.focus.next();
            return Ok(false);
        }
        if key.code == KeyCode::BackTab {
            self.focus = self.focus.previous();
            return Ok(false);
        }
        if key.code == KeyCode::Char('[') {
            self.left_collapsed = !self.left_collapsed;
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

        match self.focus {
            FocusPane::Left => self.handle_left_key(key)?,
            FocusPane::Center => self.handle_center_key(key)?,
            FocusPane::Files => self.handle_files_key(key)?,
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
            KeyCode::Char('a') => {
                self.open_project_browser()?;
            }
            KeyCode::Char('A') => {
                self.prompt = PromptState::AddProject {
                    path: String::new(),
                    name: String::new(),
                    field: PromptField::Path,
                };
            }
            KeyCode::Char('n') => self.create_agent_for_selected_project()?,
            KeyCode::Char('u') => self.refresh_selected_project()?,
            KeyCode::Char('x') => self.confirm_delete_selected_session()?,
            KeyCode::Char('d') => self.cycle_selected_project_provider()?,
            KeyCode::Char('r') => self.reconnect_selected_session()?,
            KeyCode::Char('y') => self.copy_selected_path()?,
            KeyCode::Char('i') => {
                if self.selected_session().is_some()
                    && self
                        .selected_session()
                        .map(|s| self.providers.contains_key(&s.id))
                        .unwrap_or(false)
                {
                    self.focus = FocusPane::Center;
                    self.center_mode = CenterMode::Agent;
                    self.input_target = InputTarget::Agent;
                    self.set_info("Interactive mode. Keys forwarded to agent. ctrl+g exits.");
                } else {
                    self.set_error("No active agent. Press r to restart or n to create.");
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_center_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Char('i') => {
                if self.selected_session().is_some()
                    && self
                        .selected_session()
                        .map(|s| self.providers.contains_key(&s.id))
                        .unwrap_or(false)
                {
                    self.input_target = InputTarget::Agent;
                    self.set_info("Interactive mode. Keys forwarded to agent. ctrl+g exits.");
                } else {
                    self.set_error("No active agent. Press r to restart or n to create.");
                }
            }
            KeyCode::Char('r') => {
                // Allow relaunching an exited agent from the center pane.
                let has_provider = self
                    .selected_session()
                    .map(|s| self.providers.contains_key(&s.id))
                    .unwrap_or(false);
                if !has_provider {
                    self.reconnect_selected_session()?;
                }
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

    fn handle_agent_input(&mut self, key: KeyEvent) -> Result<bool> {
        let session_id = match self.selected_session() {
            Some(s) => s.id.clone(),
            None => {
                self.input_target = InputTarget::None;
                return Ok(false);
            }
        };
        let provider = match self.providers.get(&session_id) {
            Some(p) => p,
            None => {
                self.input_target = InputTarget::None;
                self.set_error("Agent disconnected.");
                return Ok(false);
            }
        };

        // ctrl+g exits interactive mode (like classic terminal escape).
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('g') {
            self.input_target = InputTarget::None;
            self.set_info("Exited interactive mode.");
            return Ok(false);
        }

        match key.code {
            KeyCode::Esc => {
                let _ = provider.write_bytes(b"\x1b");
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let _ = provider.write_bytes(b"\x03");
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let _ = provider.write_bytes(b"\x04");
            }
            KeyCode::Char('z') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                let _ = provider.write_bytes(b"\x1a");
            }
            KeyCode::Char(c) => {
                let mut buf = [0u8; 4];
                let bytes = c.encode_utf8(&mut buf);
                let _ = provider.write_bytes(bytes.as_bytes());
            }
            KeyCode::Enter => {
                let _ = provider.write_bytes(b"\r");
            }
            KeyCode::Backspace => {
                let _ = provider.write_bytes(b"\x7f");
            }
            KeyCode::Tab => {
                let _ = provider.write_bytes(b"\t");
            }
            KeyCode::Up => {
                let _ = provider.write_bytes(b"\x1b[A");
            }
            KeyCode::Down => {
                let _ = provider.write_bytes(b"\x1b[B");
            }
            KeyCode::Right => {
                let _ = provider.write_bytes(b"\x1b[C");
            }
            KeyCode::Left => {
                let _ = provider.write_bytes(b"\x1b[D");
            }
            KeyCode::Home => {
                let _ = provider.write_bytes(b"\x1b[H");
            }
            KeyCode::End => {
                let _ = provider.write_bytes(b"\x1b[F");
            }
            KeyCode::Delete => {
                let _ = provider.write_bytes(b"\x1b[3~");
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_prompt_key(&mut self, key: KeyEvent) -> Result<bool> {
        if let PromptState::Command {
            input,
            selected,
            searching,
        } = &mut self.prompt
        {
            match key.code {
                KeyCode::Esc => {
                    if *searching {
                        *searching = false;
                    } else {
                        self.prompt = PromptState::None;
                    }
                }
                KeyCode::Char('/') if !*searching => {
                    *searching = true;
                }
                KeyCode::Char('j') | KeyCode::Down if !*searching => {
                    let count = filtered_commands(input).len();
                    if *selected + 1 < count {
                        *selected += 1;
                    }
                }
                KeyCode::Char('k') | KeyCode::Up if !*searching => {
                    if *selected > 0 {
                        *selected -= 1;
                    }
                }
                KeyCode::Down if *searching => {
                    let count = filtered_commands(input).len();
                    if *selected + 1 < count {
                        *selected += 1;
                    }
                }
                KeyCode::Up if *searching => {
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
                    if *searching {
                        *searching = false;
                    } else {
                        let command = if let Some(command) = filtered_commands(input).get(*selected)
                        {
                            command.name.to_string()
                        } else {
                            input.trim().to_string()
                        };
                        self.prompt = PromptState::None;
                        self.execute_command(command)?;
                    }
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
            filter,
            searching,
            editing_path,
            path_input,
        } = &mut self.prompt
        {
            if *editing_path {
                let mut error_msg = None;
                match key.code {
                    KeyCode::Esc => {
                        *editing_path = false;
                        path_input.clear();
                    }
                    KeyCode::Backspace => {
                        path_input.pop();
                    }
                    KeyCode::Enter => {
                        let new_dir = PathBuf::from(path_input.trim());
                        if new_dir.is_dir() {
                            *current_dir = new_dir;
                            *entries = browser_entries(current_dir);
                            *selected = 0;
                            filter.clear();
                        } else {
                            error_msg =
                                Some(format!("{} is not a directory.", path_input.trim()));
                        }
                        *editing_path = false;
                        path_input.clear();
                    }
                    KeyCode::Char(c) => {
                        if !key.modifiers.contains(KeyModifiers::CONTROL) {
                            path_input.push(c);
                        }
                    }
                    _ => {}
                }
                if let Some(msg) = error_msg {
                    self.set_error(msg);
                }
                return Ok(false);
            }

            let filtered_len = if filter.is_empty() {
                entries.len()
            } else {
                let needle = filter.to_lowercase();
                entries
                    .iter()
                    .filter(|e| e.label.to_lowercase().contains(&needle))
                    .count()
            };
            match key.code {
                KeyCode::Esc => {
                    if *searching {
                        *searching = false;
                    } else if !filter.is_empty() {
                        filter.clear();
                        *selected = 0;
                    } else {
                        self.prompt = PromptState::None;
                    }
                }
                KeyCode::Char('/') if !*searching => {
                    *searching = true;
                }
                KeyCode::Char('j') | KeyCode::Down if !*searching => {
                    if *selected + 1 < filtered_len {
                        *selected += 1;
                    }
                }
                KeyCode::Char('k') | KeyCode::Up if !*searching => {
                    if *selected > 0 {
                        *selected -= 1;
                    }
                }
                KeyCode::Down if *searching => {
                    if *selected + 1 < filtered_len {
                        *selected += 1;
                    }
                }
                KeyCode::Up if *searching => {
                    if *selected > 0 {
                        *selected -= 1;
                    }
                }
                KeyCode::Backspace if *searching => {
                    filter.pop();
                    *selected = 0;
                }
                KeyCode::Char('m') if !*searching => {
                    self.prompt = PromptState::AddProject {
                        path: current_dir.to_string_lossy().to_string(),
                        name: String::new(),
                        field: PromptField::Path,
                    };
                }
                KeyCode::Char('g') if !*searching => {
                    *editing_path = true;
                    *path_input = current_dir.to_string_lossy().to_string();
                }
                KeyCode::Enter if *searching => {
                    *searching = false;
                }
                KeyCode::Enter | KeyCode::Right | KeyCode::Char('l') if !*searching => {
                    let visible: Vec<_> = if filter.is_empty() {
                        entries.iter().collect()
                    } else {
                        let needle = filter.to_lowercase();
                        entries
                            .iter()
                            .filter(|e| e.label.to_lowercase().contains(&needle))
                            .collect()
                    };
                    if let Some(entry) = visible.get(*selected).cloned() {
                        if entry.is_git_repo {
                            let path = entry.path.to_string_lossy().to_string();
                            self.add_project(path, String::new())?;
                            self.prompt = PromptState::None;
                        } else {
                            let new_dir = entry.path.clone();
                            *current_dir = new_dir;
                            *entries = browser_entries(current_dir);
                            *selected = 0;
                            filter.clear();
                        }
                    }
                }
                KeyCode::Char(c) if *searching => {
                    if !key.modifiers.contains(KeyModifiers::CONTROL) {
                        filter.push(c);
                        *selected = 0;
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

        if let PromptState::ConfirmDeleteAgent {
            session_id,
            confirm_selected,
            ..
        } = &mut self.prompt
        {
            match key.code {
                KeyCode::Esc => self.prompt = PromptState::None,
                KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::Char('h') | KeyCode::Char('l') => {
                    *confirm_selected = !*confirm_selected;
                }
                KeyCode::Enter => {
                    if *confirm_selected {
                        let id = session_id.clone();
                        self.prompt = PromptState::None;
                        self.do_delete_session(&id)?;
                    } else {
                        self.prompt = PromptState::None;
                    }
                }
                _ => {}
            }
        }

        if let PromptState::ConfirmQuit {
            confirm_selected, ..
        } = &mut self.prompt
        {
            match key.code {
                KeyCode::Esc => self.prompt = PromptState::None,
                KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::Char('h') | KeyCode::Char('l') => {
                    *confirm_selected = !*confirm_selected;
                }
                KeyCode::Enter => {
                    if *confirm_selected {
                        return Ok(true);
                    } else {
                        self.prompt = PromptState::None;
                    }
                }
                _ => {}
            }
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
        let start_dir = self
            .config
            .defaults
            .start_directory
            .as_ref()
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| {
                std::env::var("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|_| {
                        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                    })
            });
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
            filter: String::new(),
            searching: false,
            editing_path: false,
            path_input: String::new(),
        };
        self.set_info(
            "Project browser: Enter opens or adds a repo, / to search, m switches to manual entry.",
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
        let worker_tx = self.worker_tx.clone();
        let term_size = crossterm::terminal::size().unwrap_or((80, 24));
        thread::spawn(move || {
            run_create_agent_job(project, paths, config, worker_tx, term_size);
        });
        Ok(())
    }

    fn spawn_pty_for_session(&self, session: &AgentSession) -> Result<PtyClient> {
        let cfg = provider_config(&self.config, &session.provider);
        let (rows, cols) = if self.last_pty_size != (0, 0) {
            self.last_pty_size
        } else {
            (24, 80)
        };
        logger::debug(&format!(
            "spawning PTY {:?} {:?} in {} ({}x{})",
            cfg.command, cfg.args, session.worktree_path, cols, rows
        ));
        PtyClient::spawn(
            &cfg.command,
            &cfg.args,
            Path::new(&session.worktree_path),
            rows,
            cols,
        )
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

    fn confirm_delete_selected_session(&mut self) -> Result<()> {
        let Some(session) = self.selected_session().cloned() else {
            self.set_error("Select a session first.");
            return Ok(());
        };
        self.prompt = PromptState::ConfirmDeleteAgent {
            session_id: session.id.clone(),
            branch_name: session.branch_name.clone(),
            confirm_selected: false, // Cancel is default
        };
        Ok(())
    }

    fn do_delete_session(&mut self, session_id: &str) -> Result<()> {
        let Some(session) = self
            .sessions
            .iter()
            .find(|s| s.id == session_id)
            .cloned()
        else {
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
                self.do_delete_session(&session_id)?;
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
            self.set_error("Select a stopped agent first.");
            return Ok(());
        };
        logger::info(&format!("reconnecting session {}", session.id));
        if self.providers.contains_key(&session.id) {
            self.set_info("Session is already connected.");
            return Ok(());
        }
        if !Path::new(&session.worktree_path).exists() {
            self.set_error("Worktree no longer exists. Delete and re-create the agent.");
            return Ok(());
        }
        match self.spawn_pty_for_session(&session) {
            Ok(client) => {
                self.providers.insert(session.id.clone(), client);
                self.mark_session_status(&session.id, SessionStatus::Active);
                self.focus = FocusPane::Center;
                self.center_mode = CenterMode::Agent;
                self.input_target = InputTarget::Agent;
                self.set_info(format!("Relaunched {}", session.branch_name));
            }
            Err(err) => {
                self.set_error(format!("Reconnect failed: {err}"));
            }
        }
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
                WorkerEvent::CreateAgentReady { session, client, pty_size } => {
                    self.create_agent_in_flight = false;
                    self.last_pty_size = pty_size;
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
                    self.focus = FocusPane::Center;
                    self.center_mode = CenterMode::Agent;
                    self.input_target = InputTarget::Agent;
                    self.set_info(format!("Created {}", session.branch_name));
                }
                WorkerEvent::CreateAgentFailed(message) => {
                    self.create_agent_in_flight = false;
                    self.set_error(message);
                }
            }
        }
        // Detect PTY exits.
        let mut exited = Vec::new();
        for (session_id, provider) in &mut self.providers {
            if provider.is_exited() || provider.try_wait().is_some() {
                exited.push(session_id.clone());
            }
        }
        for session_id in &exited {
            self.providers.remove(session_id);
            self.mark_session_status(session_id, SessionStatus::Detached);
        }
        if !exited.is_empty() {
            // If the currently-viewed session just exited, leave interactive mode.
            if let Some(current) = self.selected_session() {
                if exited.contains(&current.id) {
                    self.input_target = InputTarget::None;
                    self.focus = FocusPane::Left;
                    self.set_info("Agent CLI exited. Press r to relaunch.");
                }
            }
        }
    }

    fn render(&mut self, frame: &mut Frame) {
        let term_w = frame.area().width as usize;
        let status_text_len = self.status.text().len() + 3; // " ● " prefix
        let status_lines: u16 = if term_w > 0 && status_text_len > term_w { 2 } else { 1 };
        let footer_h = 1 + status_lines; // 1 for hints + status lines
        let [header, body, footer] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(4),
                Constraint::Length(footer_h),
            ])
            .areas(frame.area());
        self.render_header(frame, header);
        let [left, center, right] = if self.left_collapsed {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(4),
                    Constraint::Min(20),
                    Constraint::Percentage(self.right_width_pct),
                ])
                .areas(body)
        } else {
            let center_pct = 100u16
                .saturating_sub(self.left_width_pct + self.right_width_pct)
                .max(20);
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(self.left_width_pct),
                    Constraint::Percentage(center_pct),
                    Constraint::Percentage(self.right_width_pct),
                ])
                .areas(body)
        };
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
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(self.theme.border_normal))
            .render(shell, frame.buffer_mut());
        self.render_footer(frame, footer);
        self.render_overlay(frame);
    }

    fn render_header(&self, frame: &mut Frame, area: Rect) {
        let bg = self.theme.header_bg;
        let sep_fg = self.theme.header_separator_fg;
        let label_fg = self.theme.header_label_fg;
        let mut spans = vec![
            Span::styled(" dux ", Style::default().fg(label_fg).bg(bg)),
            Span::styled(
                format!("v{}", env!("CARGO_PKG_VERSION")),
                Style::default().fg(self.theme.branch_fg).bg(bg),
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
                Style::default().fg(self.theme.branch_fg).bg(bg),
            ));
            spans.push(Span::styled(" ╱ ", Style::default().fg(sep_fg).bg(bg)));
            spans.push(Span::styled(
                "branch: ",
                Style::default().fg(label_fg).bg(bg),
            ));
            spans.push(Span::styled(
                project.current_branch.clone(),
                Style::default().fg(self.theme.branch_fg).bg(bg),
            ));
            spans.push(Span::styled(" ╱ ", Style::default().fg(sep_fg).bg(bg)));
            spans.push(Span::styled(
                "provider: ",
                Style::default().fg(label_fg).bg(bg),
            ));
            spans.push(Span::styled(
                project.default_provider.as_str().to_string(),
                Style::default().fg(self.theme.branch_fg).bg(bg),
            ));
        }
        Paragraph::new(Line::from(spans))
            .style(self.theme.header_style())
            .render(area, frame.buffer_mut());
    }

    fn render_left(&self, frame: &mut Frame, area: Rect) {
        let focused = self.focus == FocusPane::Left;

        if self.left_collapsed {
            let items = self
                .left_items()
                .into_iter()
                .map(|item| match item {
                    LeftItem::Project(_) => ListItem::new(Line::from(Span::styled(
                        "▸",
                        Style::default().fg(self.theme.project_icon),
                    ))),
                    LeftItem::Session(index) => {
                        let session = &self.sessions[index];
                        let (dot, dot_color) = self.theme.session_dot(&session.status);
                        ListItem::new(Line::from(Span::styled(
                            dot.to_string(),
                            Style::default().fg(dot_color),
                        )))
                    }
                })
                .collect::<Vec<_>>();
            let mut state = ListState::default().with_selected(Some(self.selected_left));
            StatefulWidget::render(
                List::new(items)
                    .block(self.themed_block("", focused))
                    .highlight_style(self.theme.selection_style()),
                area,
                frame.buffer_mut(),
                &mut state,
            );
            return;
        }

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
                        Span::styled(format!("{dot} "), Style::default().fg(dot_color)),
                        Span::styled(label, Style::default().fg(dot_color)),
                        Span::styled(
                            format!(" ({})", session.provider.as_str()),
                            Style::default().fg(self.theme.provider_label_fg),
                        ),
                    ]))
                }
            })
            .collect::<Vec<_>>();
        let title = format!("Projects ({})", self.projects.len());
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

    fn render_center(&mut self, frame: &mut Frame, area: Rect) {
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
                self.render_agent_terminal(frame, area, title, focused);
            }
        }
    }

    fn render_agent_terminal(&mut self, frame: &mut Frame, area: Rect, title: &str, focused: bool) {
        let outer_block = self.themed_block(title, focused);
        let inner = outer_block.inner(area);
        outer_block.render(area, frame.buffer_mut());

        if inner.height < 2 || inner.width < 4 {
            return;
        }

        let is_input = self.input_target == InputTarget::Agent;

        // Reserve 2 lines at the bottom for the hint bar (top border + text).
        let hint_height = 2;
        let [term_area, hint_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(hint_height)])
            .areas(inner);

        // Get the selected session's PTY screen.
        let session_id = self.selected_session().map(|s| s.id.clone());
        let session_provider_name = self
            .selected_session()
            .map(|s| s.provider.as_str().to_owned());
        let session_active = session_id
            .as_ref()
            .map(|id| self.providers.contains_key(id))
            .unwrap_or(false);

        if let Some(ref sid) = session_id {
            if let Some(provider) = self.providers.get(sid) {
                // Resize PTY if needed.
                let new_size = (term_area.height, term_area.width);
                if new_size != self.last_pty_size && new_size.0 > 0 && new_size.1 > 0 {
                    let _ = provider.resize(new_size.0, new_size.1);
                    self.last_pty_size = new_size;
                }

                if !provider.has_output() {
                    // Show a centered loading card until the PTY produces output.
                    let spinner_chars = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
                    let idx = (self.tick_count as usize) % spinner_chars.len();
                    let spinner = spinner_chars[idx];
                    let (label_spans, label_len) = match session_provider_name.as_deref() {
                        Some(name) => {
                            let text_len = "Starting ".len() + name.len() + "...".len();
                            let spans = vec![
                                Span::styled(
                                    "Starting ",
                                    Style::default().fg(self.theme.hint_desc_fg),
                                ),
                                Span::styled(
                                    name.to_owned(),
                                    Style::default().fg(self.theme.branch_fg),
                                ),
                                Span::styled(
                                    "...",
                                    Style::default().fg(self.theme.hint_desc_fg),
                                ),
                            ];
                            (spans, text_len)
                        }
                        None => {
                            let text = "Starting agent...";
                            let spans = vec![Span::styled(
                                text,
                                Style::default().fg(self.theme.hint_desc_fg),
                            )];
                            (spans, text.len())
                        }
                    };

                    // Card dimensions: border + padding + content + padding + border.
                    // +2 for spinner + space prefix.
                    let content_w = label_len as u16 + 2;
                    let card_w = (content_w + 2 + 6).min(term_area.width); // 2 borders + 6 padding
                    let card_h: u16 = 5; // top border, blank, spinner, blank, bottom border

                    if term_area.width >= card_w && term_area.height >= card_h {
                        let cx = term_area.x + (term_area.width - card_w) / 2;
                        let cy = term_area.y + (term_area.height - card_h) / 2;
                        let card_area = Rect::new(cx, cy, card_w, card_h);

                        let card_block = Block::default()
                            .borders(Borders::ALL)
                            .border_type(ratatui::widgets::BorderType::Rounded)
                            .border_style(Style::default().fg(self.theme.border_normal));
                        let card_inner = card_block.inner(card_area);
                        card_block.render(card_area, frame.buffer_mut());

                        // Render spinner + label centered inside the card.
                        let mut spans = vec![Span::styled(
                            format!("{spinner} "),
                            Style::default()
                                .fg(self.theme.hint_key_fg)
                                .add_modifier(Modifier::BOLD),
                        )];
                        spans.extend(label_spans);
                        let line = Line::from(spans);
                        Paragraph::new(line)
                            .alignment(ratatui::layout::Alignment::Center)
                            .render(
                                Rect::new(
                                    card_inner.x,
                                    card_inner.y + card_inner.height / 2,
                                    card_inner.width,
                                    1,
                                ),
                                frame.buffer_mut(),
                            );
                    }
                } else {
                    // Render vt100 screen into ratatui buffer.
                    let screen = provider.screen();
                    let buf = frame.buffer_mut();
                    let (screen_rows, screen_cols) = screen.size();
                    for row in 0..screen_rows.min(term_area.height) {
                        for col in 0..screen_cols.min(term_area.width) {
                            let cell = screen.cell(row, col);
                            if let Some(cell) = cell {
                                if cell.is_wide_continuation() {
                                    continue;
                                }
                                let x = term_area.x + col;
                                let y = term_area.y + row;
                                let fg = convert_vt100_color(cell.fgcolor());
                                let bg = convert_vt100_color(cell.bgcolor());
                                let mut modifier = Modifier::empty();
                                if cell.bold() {
                                    modifier |= Modifier::BOLD;
                                }
                                if cell.italic() {
                                    modifier |= Modifier::ITALIC;
                                }
                                if cell.underline() {
                                    modifier |= Modifier::UNDERLINED;
                                }
                                if cell.inverse() {
                                    modifier |= Modifier::REVERSED;
                                }
                                let style =
                                    Style::default().fg(fg).bg(bg).add_modifier(modifier);
                                let contents = cell.contents();
                                let ratatui_cell = &mut buf[(x, y)];
                                if contents.is_empty() {
                                    ratatui_cell.set_symbol(" ");
                                } else {
                                    ratatui_cell.set_symbol(&contents);
                                }
                                ratatui_cell.set_style(style);
                            }
                        }
                    }

                    // Render cursor if in input mode.
                    if is_input && !screen.hide_cursor() {
                        let (cursor_row, cursor_col) = screen.cursor_position();
                        let cx = term_area.x + cursor_col;
                        let cy = term_area.y + cursor_row;
                        if cx < term_area.x + term_area.width
                            && cy < term_area.y + term_area.height
                        {
                            let cursor_cell = &mut buf[(cx, cy)];
                            cursor_cell.set_style(
                                Style::default()
                                    .fg(Color::Black)
                                    .bg(self.theme.prompt_cursor),
                            );
                        }
                    }
                }
            }
        }

        // Hint bar with top border.
        if hint_area.height > 0 {
            let hint_line = if is_input {
                let desc_style = Style::default().fg(self.theme.hint_desc_fg);
                let mut spans: Vec<Span> = Vec::new();
                let cli_name = session_provider_name
                    .as_deref()
                    .unwrap_or("the agent");
                spans.push(Span::styled("Press ", desc_style));
                spans.extend(self.theme.key_badge("ctrl+g", Color::Reset));
                spans.push(Span::styled(
                    format!(" to manage dux instead of {cli_name}."),
                    desc_style,
                ));
                Line::from(spans)
            } else {
                let desc_style = Style::default().fg(self.theme.hint_desc_fg);
                let mut spans: Vec<Span> = Vec::new();
                if session_active {
                    spans.push(Span::styled("Press ", desc_style));
                    spans.extend(self.theme.key_badge("i", Color::Reset));
                    spans.push(Span::styled(" to interact with the agent.", desc_style));
                } else if session_id.is_some() {
                    spans.push(Span::styled("Agent CLI exited. Press ", desc_style));
                    spans.extend(self.theme.key_badge("r", Color::Reset));
                    spans.push(Span::styled(" to relaunch.", desc_style));
                } else {
                    spans.push(Span::styled("No agent selected.", desc_style));
                }
                Line::from(spans)
            };
            Paragraph::new(hint_line)
                .block(
                    Block::default()
                        .borders(Borders::TOP)
                        .border_style(Style::default().fg(self.theme.border_normal)),
                )
                .render(hint_area, frame.buffer_mut());
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

    fn render_footer(&self, frame: &mut Frame, area: Rect) {
        let is_on_project = !matches!(
            self.left_items().get(self.selected_left),
            Some(LeftItem::Session(_))
        );
        let left_project_hints: &[(&str, &str)] = &[
            ("j/k", "Move"),
            ("n", "New agent"),
            ("a", "Add project"),
            ("d", "Provider"),
            ("u", "Pull"),
            ("^P", "Palette"),
            ("?", "Help"),
            ("q", "Quit"),
        ];
        let left_session_hints: &[(&str, &str)] = &[
            ("j/k", "Move"),
            ("Enter", "Focus"),
            ("a", "Add project"),
            ("r", "Reconnect"),
            ("x", "Delete"),
            ("^P", "Palette"),
            ("?", "Help"),
            ("q", "Quit"),
        ];
        let hints: &[(&str, &str)] = match self.focus {
            FocusPane::Left => {
                if is_on_project {
                    left_project_hints
                } else {
                    left_session_hints
                }
            }
            FocusPane::Center => &[
                ("i", "Interact"),
                ("Esc", "Close diff"),
                ("Tab", "Next"),
                ("^P", "Palette"),
                ("?", "Help"),
                ("q", "Quit"),
            ],
            FocusPane::Files => &[
                ("j/k", "Move"),
                ("Enter", "Diff"),
                ("Tab", "Next"),
                ("^P", "Palette"),
                ("?", "Help"),
                ("q", "Quit"),
            ],
        };
        let [hints_area, status_area] = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(1)])
            .areas(area);
        let max_w = hints_area.width as usize;
        let ellipsis = "…";
        let ellipsis_w = 1;

        let mut hint_spans: Vec<Span> = Vec::new();
        let bar_bg = self.theme.hint_bar_bg;
        let mut used = 0usize;
        for (i, (key, desc)) in hints.iter().enumerate() {
            // width of this hint: separator + <key> + space + desc
            let sep = if i > 0 { 1 } else { 0 };
            let hint_w = sep + key.len() + 2 + 1 + desc.len();
            if used + hint_w > max_w {
                if used + ellipsis_w <= max_w {
                    hint_spans.push(Span::styled(
                        ellipsis,
                        Style::default().fg(self.theme.hint_desc_fg).bg(bar_bg),
                    ));
                }
                break;
            }
            if i > 0 {
                hint_spans.push(Span::styled(" ", Style::default().bg(bar_bg)));
            }
            hint_spans.extend(self.theme.key_badge(key, bar_bg));
            hint_spans.push(Span::styled(
                format!(" {desc}"),
                Style::default().fg(self.theme.hint_desc_fg).bg(bar_bg),
            ));
            used += hint_w;
        }

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
        let prefix = format!(" {dot} ");
        let prefix_w = prefix.len();
        let max_status_chars = (status_area.width as usize) * (status_area.height as usize);
        let available = max_status_chars.saturating_sub(prefix_w);
        let truncated = if status_text.len() > available && available > 1 {
            format!("{}…", &status_text[..available - 1])
        } else {
            status_text
        };
        let status_line = Line::from(vec![
            Span::styled(
                prefix,
                Style::default().fg(dot_color).bg(status_bg),
            ),
            Span::styled(
                truncated,
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
                    ("^P", "Open command palette"),
                    ("^W", "Resize mode (h/l side, j/k split)"),
                    ("[", "Toggle sidebar"),
                    ("?", "Toggle help"),
                    ("q", "Quit"),
                ],
            ),
            (
                "Projects pane",
                &[
                    ("j/k", "Move through projects and sessions"),
                    ("a", "Open project browser"),
                    ("A", "Manual path entry"),
                    ("n", "New agent session (creates worktree)"),
                    ("d", "Cycle default provider"),
                    ("u", "Refresh checkout (git pull --ff-only)"),
                    ("r", "Restart agent CLI"),
                    ("x", "Delete selected session/worktree"),
                ],
            ),
            (
                "Agent pane",
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
                "Key notation",
                &[
                    ("^X", "Hold Ctrl and press X (e.g. ^P = Ctrl+P)"),
                ],
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
                searching,
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
                            let mut left_spans = vec![
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
                            ];
                            if let Some(shortcut) = command.shortcut {
                                let left_len: usize = left_spans.iter().map(|s| s.width()).sum();
                                // +3 for badge brackets <k>, +2 for borders, +1 for right padding
                                let badge_len = shortcut.len() + 3;
                                let avail = popup.width as usize;
                                let pad = avail.saturating_sub(left_len + badge_len + 3);
                                left_spans.push(Span::raw(" ".repeat(pad.max(2))));
                                left_spans.extend(self.theme.key_badge(shortcut, Color::Reset));
                            }
                            ListItem::new(Line::from(left_spans))
                        })
                        .collect::<Vec<_>>()
                };
                let mut state = ListState::default()
                    .with_selected(Some((*selected).min(commands.len().saturating_sub(1))));
                let [input_area, list_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(3), Constraint::Min(3)])
                    .areas(popup);
                let title = if *searching {
                    "Command Palette (searching)"
                } else {
                    "Command Palette"
                };
                let mut bottom_spans = vec![Span::raw(" ")];
                if *searching {
                    bottom_spans.extend(self.theme.key_badge("Enter", Color::Reset));
                    bottom_spans.push(Span::styled(
                        " done  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge("Esc", Color::Reset));
                    bottom_spans.push(Span::styled(
                        " clear",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                } else {
                    bottom_spans.extend(self.theme.key_badge("/", Color::Reset));
                    bottom_spans.push(Span::styled(
                        " search  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge("Enter", Color::Reset));
                    bottom_spans.push(Span::styled(
                        " run  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge("Tab", Color::Reset));
                    bottom_spans.push(Span::styled(
                        " complete  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge("Esc", Color::Reset));
                    bottom_spans.push(Span::styled(
                        " cancel",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                }
                let prompt_prefix = if *searching { "/ " } else { "> " };
                Paragraph::new(format!("{}{}", prompt_prefix, input))
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
                filter,
                searching,
                editing_path,
                path_input,
            } => {
                self.render_dim_overlay(frame);
                let area = centered_rect(72, 70, frame.area());
                Clear.render(area, frame.buffer_mut());
                let visible: Vec<_> = if filter.is_empty() {
                    entries.iter().collect()
                } else {
                    let needle = filter.to_lowercase();
                    entries
                        .iter()
                        .filter(|e| e.label.to_lowercase().contains(&needle))
                        .collect()
                };
                let items = if visible.is_empty() {
                    vec![ListItem::new(if filter.is_empty() {
                        "No child directories here."
                    } else {
                        "No matching entries."
                    })]
                } else {
                    let last = visible.len() - 1;
                    visible
                        .iter()
                        .enumerate()
                        .map(|(i, entry)| {
                            let prefix = if entry.label == "../" {
                                ""
                            } else if i == last {
                                "└── "
                            } else {
                                "├── "
                            };
                            ListItem::new(Line::from(vec![
                                Span::styled(
                                    prefix.to_string(),
                                    Style::default().fg(self.theme.hint_desc_fg),
                                ),
                                Span::raw(entry.label.clone()),
                            ]))
                        })
                        .collect::<Vec<_>>()
                };
                let mut state = ListState::default()
                    .with_selected(Some((*selected).min(visible.len().saturating_sub(1))));
                let has_filter = !filter.is_empty();
                let show_top_input = *searching || has_filter || *editing_path;
                let (top_areas, list_render_area) = if show_top_input {
                    let [filter_area, list_area] = Layout::default()
                        .direction(Direction::Vertical)
                        .constraints([Constraint::Length(3), Constraint::Min(3)])
                        .areas(area);
                    (Some(filter_area), list_area)
                } else {
                    (None, area)
                };
                if let Some(filter_area) = top_areas {
                    let title = format!("Add Project: {}", current_dir.display());
                    let input_text = if *editing_path {
                        format!("go: {}█", path_input)
                    } else {
                        format!("/ {}", filter)
                    };
                    Paragraph::new(input_text)
                        .block(self.themed_overlay_block(&title))
                        .render(filter_area, frame.buffer_mut());
                    let mut bottom_spans = vec![Span::raw(" ")];
                    if *editing_path {
                        bottom_spans.extend(self.theme.key_badge("Enter", Color::Reset));
                        bottom_spans.push(Span::styled(" go  ", Style::default().fg(self.theme.hint_desc_fg)));
                        bottom_spans.extend(self.theme.key_badge("Esc", Color::Reset));
                        bottom_spans.push(Span::styled(" cancel", Style::default().fg(self.theme.hint_desc_fg)));
                    } else if *searching {
                        bottom_spans.extend(self.theme.key_badge("Enter", Color::Reset));
                        bottom_spans.push(Span::styled(
                            " done  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge("Esc", Color::Reset));
                        bottom_spans.push(Span::styled(
                            " clear",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                    } else {
                        bottom_spans.extend(self.theme.key_badge("/", Color::Reset));
                        bottom_spans.push(Span::styled(
                            " search  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge("Enter", Color::Reset));
                        bottom_spans.push(Span::styled(
                            " open  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge("g", Color::Reset));
                        bottom_spans.push(Span::styled(
                            " go to  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge("m", Color::Reset));
                        bottom_spans.push(Span::styled(
                            " manual  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge("Esc", Color::Reset));
                        bottom_spans.push(Span::styled(
                            " cancel",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                    }
                    StatefulWidget::render(
                        List::new(items)
                            .block(
                                Block::default()
                                    .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
                                    .border_style(Style::default().fg(self.theme.overlay_border))
                                    .title_bottom(Line::from(bottom_spans)),
                            )
                            .highlight_style(self.theme.selection_style()),
                        list_render_area,
                        frame.buffer_mut(),
                        &mut state,
                    );
                } else {
                    let mut bottom_spans = vec![Span::raw(" ")];
                    bottom_spans.extend(self.theme.key_badge("/", Color::Reset));
                    bottom_spans.push(Span::styled(
                        " search  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge("Enter", Color::Reset));
                    bottom_spans.push(Span::styled(
                        " open  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge("g", Color::Reset));
                    bottom_spans.push(Span::styled(
                        " go to  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge("m", Color::Reset));
                    bottom_spans.push(Span::styled(
                        " manual  ",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    bottom_spans.extend(self.theme.key_badge("Esc", Color::Reset));
                    bottom_spans.push(Span::styled(
                        " cancel",
                        Style::default().fg(self.theme.hint_desc_fg),
                    ));
                    StatefulWidget::render(
                        List::new(items)
                            .block(
                                self.themed_overlay_block(&format!(
                                    "Add Project: {}",
                                    current_dir.display()
                                ))
                                .title_bottom(Line::from(bottom_spans)),
                            )
                            .highlight_style(self.theme.selection_style()),
                        list_render_area,
                        frame.buffer_mut(),
                        &mut state,
                    );
                }
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
                        s.push(Span::styled(
                            " switch fields  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        s.extend(self.theme.key_badge("Enter", Color::Reset));
                        s.push(Span::styled(
                            " save  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        s.extend(self.theme.key_badge("Esc", Color::Reset));
                        s.push(Span::styled(
                            " cancel",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        s
                    }),
                ];
                Paragraph::new(lines)
                    .block(self.themed_overlay_block("Manual Project Entry"))
                    .wrap(Wrap { trim: false })
                    .render(area, frame.buffer_mut());
            }
            PromptState::ConfirmDeleteAgent {
                branch_name,
                confirm_selected,
                ..
            } => {
                self.render_dim_overlay(frame);
                // Outer dialog: border + title.
                let area = centered_rect(56, 30, frame.area());
                Clear.render(area, frame.buffer_mut());
                let outer = self.themed_overlay_block("Delete Agent");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                // Body text.
                let [body_area, _, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(1),
                        Constraint::Length(1),
                        Constraint::Length(3),
                    ])
                    .areas(inner);

                let lines = vec![
                    Line::from(""),
                    Line::from(vec![
                        Span::raw(" Are you sure you want to delete "),
                        Span::styled(
                            branch_name.as_str(),
                            Style::default().add_modifier(Modifier::BOLD),
                        ),
                        Span::raw("?"),
                    ]),
                    Line::from(""),
                    Line::from(Span::styled(
                        " All uncommitted and unpushed changes in this",
                        Style::default().fg(Color::Yellow),
                    )),
                    Line::from(Span::styled(
                        " worktree will be permanently lost.",
                        Style::default().fg(Color::Yellow),
                    )),
                ];
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .render(body_area, frame.buffer_mut());

                // Button area: two bordered panels side by side.
                let btn_width = 16u16;
                let gap = 2u16;
                let total = btn_width * 2 + gap;
                let left_offset = buttons_area
                    .width
                    .saturating_sub(total)
                    / 2;

                let cancel_area = Rect {
                    x: buttons_area.x + left_offset,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };
                let delete_area = Rect {
                    x: cancel_area.x + btn_width + gap,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                let (cancel_border, cancel_fg) = if !confirm_selected {
                    (Color::Cyan, Color::White)
                } else {
                    (self.theme.border_normal, self.theme.hint_desc_fg)
                };
                let (delete_border, delete_fg) = if *confirm_selected {
                    (Color::Red, Color::White)
                } else {
                    (self.theme.border_normal, self.theme.hint_desc_fg)
                };

                Paragraph::new(Line::from(Span::styled(
                    "Cancel",
                    Style::default().fg(cancel_fg).add_modifier(Modifier::BOLD),
                )))
                .alignment(ratatui::layout::Alignment::Center)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_set(border::ROUNDED)
                        .border_style(Style::default().fg(cancel_border)),
                )
                .render(cancel_area, frame.buffer_mut());

                Paragraph::new(Line::from(Span::styled(
                    "Delete",
                    Style::default().fg(delete_fg).add_modifier(Modifier::BOLD),
                )))
                .alignment(ratatui::layout::Alignment::Center)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_set(border::ROUNDED)
                        .border_style(Style::default().fg(delete_border)),
                )
                .render(delete_area, frame.buffer_mut());
            }
            PromptState::ConfirmQuit {
                active_count,
                confirm_selected,
            } => {
                self.render_dim_overlay(frame);
                let area = centered_rect(56, 30, frame.area());
                Clear.render(area, frame.buffer_mut());
                let outer = self.themed_overlay_block("Quit dux");
                let inner = outer.inner(area);
                outer.render(area, frame.buffer_mut());

                let [body_area, _, buttons_area] = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Min(1),
                        Constraint::Length(1),
                        Constraint::Length(3),
                    ])
                    .areas(inner);

                let agent_word = if *active_count == 1 { "agent" } else { "agents" };
                let lines = vec![
                    Line::from(""),
                    Line::from(vec![
                        Span::raw(format!(" {active_count} running {agent_word} will be ")),
                        Span::styled("killed", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
                        Span::raw(" if you quit."),
                    ]),
                    Line::from(""),
                    Line::from(Span::styled(
                        " Any in-progress work by those agents will be lost.",
                        Style::default().fg(Color::Yellow),
                    )),
                    Line::from(Span::styled(
                        " File changes in worktrees are preserved.",
                        Style::default().fg(self.theme.hint_desc_fg),
                    )),
                ];
                Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .render(body_area, frame.buffer_mut());

                let btn_width = 16u16;
                let gap = 2u16;
                let total = btn_width * 2 + gap;
                let left_offset = buttons_area.width.saturating_sub(total) / 2;

                let cancel_area = Rect {
                    x: buttons_area.x + left_offset,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };
                let quit_area = Rect {
                    x: cancel_area.x + btn_width + gap,
                    y: buttons_area.y,
                    width: btn_width,
                    height: 3,
                };

                let (cancel_border, cancel_fg) = if !confirm_selected {
                    (Color::Cyan, Color::White)
                } else {
                    (self.theme.border_normal, self.theme.hint_desc_fg)
                };
                let (quit_border, quit_fg) = if *confirm_selected {
                    (Color::Red, Color::White)
                } else {
                    (self.theme.border_normal, self.theme.hint_desc_fg)
                };

                Paragraph::new(Line::from(Span::styled(
                    "Cancel",
                    Style::default().fg(cancel_fg).add_modifier(Modifier::BOLD),
                )))
                .alignment(ratatui::layout::Alignment::Center)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_set(border::ROUNDED)
                        .border_style(Style::default().fg(cancel_border)),
                )
                .render(cancel_area, frame.buffer_mut());

                Paragraph::new(Line::from(Span::styled(
                    "Quit",
                    Style::default().fg(quit_fg).add_modifier(Modifier::BOLD),
                )))
                .alignment(ratatui::layout::Alignment::Center)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_set(border::ROUNDED)
                        .border_style(Style::default().fg(quit_border)),
                )
                .render(quit_area, frame.buffer_mut());
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
        let command = command.trim();
        match command {
            "new-agent" => self.create_agent_for_selected_project(),
            "provider" => self.cycle_selected_project_provider(),
            "refresh-project" => self.refresh_selected_project(),
            "delete-project" => self.delete_selected_project(),
            "delete-agent" => self.confirm_delete_selected_session(),
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
            "copy-path" => self.copy_selected_path(),
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

    fn copy_selected_path(&mut self) -> Result<()> {
        let path = match self.left_items().get(self.selected_left) {
            Some(LeftItem::Session(index)) => {
                self.sessions.get(*index).map(|s| s.worktree_path.clone())
            }
            Some(LeftItem::Project(index)) => self.projects.get(*index).map(|p| p.path.clone()),
            None => None,
        };
        match path {
            Some(p) => {
                let mut clipboard = arboard::Clipboard::new()
                    .map_err(|e| anyhow::anyhow!("Failed to access clipboard: {e}"))?;
                clipboard
                    .set_text(&p)
                    .map_err(|e| anyhow::anyhow!("Failed to copy to clipboard: {e}"))?;
                self.set_info(format!("Copied: {p}"));
                Ok(())
            }
            None => {
                self.set_error("No project or agent selected.");
                Ok(())
            }
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

    fn themed_block<'a>(&self, title: &'a str, focused: bool) -> Block<'a> {
        Block::default()
            .title(Line::from(Span::styled(title, self.theme.title_style(focused))))
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
}

fn run_create_agent_job(
    project: Project,
    paths: DuxPaths,
    config: Config,
    worker_tx: Sender<WorkerEvent>,
    term_size: (u16, u16),
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
    let session = AgentSession {
        id: Uuid::new_v4().to_string(),
        project_id: project.id,
        provider: project.default_provider.clone(),
        source_branch: project.current_branch.clone(),
        branch_name,
        worktree_path: worktree_path.to_string_lossy().to_string(),
        title: None,
        status: SessionStatus::Active,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    };
    let provider_cfg = provider_config(&config, &session.provider);
    if let Err(hint) = check_provider_available(session.provider.as_str(), &provider_cfg.command) {
        logger::error(&format!("provider not found for {}: {hint}", session.id));
        let _ = git::remove_worktree(
            &repo_path,
            Path::new(&session.worktree_path),
            &session.branch_name,
        );
        let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(hint));
        return;
    }
    let _ = worker_tx.send(WorkerEvent::CreateAgentProgress(format!(
        "Launching {}...",
        session.provider.as_str()
    )));
    // crossterm::terminal::size() returns (cols, rows).
    let (cols, rows) = term_size;
    let client = match PtyClient::spawn(
        &provider_cfg.command,
        &provider_cfg.args,
        &worktree_path,
        rows,
        cols,
    ) {
        Ok(client) => client,
        Err(err) => {
            logger::error(&format!("PTY spawn failed for {}: {err}", session.id));
            let _ = git::remove_worktree(
                &repo_path,
                Path::new(&session.worktree_path),
                &session.branch_name,
            );
            let _ = worker_tx.send(WorkerEvent::CreateAgentFailed(format!(
                "Failed to start {}: {err}",
                provider_cfg.command
            )));
            return;
        }
    };
    logger::info(&format!("PTY session started for {}", session.id));
    let _ = worker_tx.send(WorkerEvent::CreateAgentReady { session, client, pty_size: (rows, cols) });
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
            command: provider.as_str().to_string(),
            args: Vec::new(),
        })
}

fn convert_vt100_color(color: vt100::Color) -> Color {
    match color {
        vt100::Color::Default => Color::Reset,
        vt100::Color::Idx(n) => Color::Indexed(n),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
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
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('.') {
                return None;
            }
            let is_git_repo = git::is_git_repo(&path);
            let label = if is_git_repo {
                name
            } else {
                format!("{name}/")
            };
            Some(BrowserEntry {
                is_git_repo,
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
    if let Some(parent) = dir.parent() {
        entries.insert(
            0,
            BrowserEntry {
                path: parent.to_path_buf(),
                label: "../".to_string(),
                is_git_repo: false,
            },
        );
    }
    entries
}

fn filtered_commands(input: &str) -> Vec<&'static CommandDef> {
    let needle = input.trim().to_lowercase();
    if needle.is_empty() {
        return COMMANDS.iter().collect();
    }
    let mut name_matches: Vec<&'static CommandDef> = Vec::new();
    let mut desc_matches: Vec<&'static CommandDef> = Vec::new();
    for command in COMMANDS.iter() {
        if command.name.contains(&needle) {
            name_matches.push(command);
        } else if command.description.to_lowercase().contains(&needle) {
            desc_matches.push(command);
        }
    }
    name_matches.extend(desc_matches);
    name_matches
}
