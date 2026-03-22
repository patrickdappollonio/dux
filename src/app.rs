use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use chrono::Utc;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

use crate::keybindings::{self, Action, BindingScope, HintContext};
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
    watched_worktree: Arc<Mutex<Option<PathBuf>>>,
    has_active_agent: Arc<AtomicBool>,
    collapsed_projects: HashSet<String>,
    left_items_cache: Vec<LeftItem>,
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
    Diff(Vec<Line<'static>>),
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

#[derive(Clone, Copy)]
enum ScrollDirection {
    Up,
    Down,
}

#[derive(Clone, Copy, Debug)]
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
    ChangedFilesReady(Vec<ChangedFile>),
    BrowserEntriesReady {
        dir: PathBuf,
        entries: Vec<BrowserEntry>,
    },
}


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
        if let Some(action) = keybindings::lookup(&key, BindingScope::Global) {
            match action {
                Action::Quit => {
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
                Action::ToggleHelp => {
                    self.help_overlay = !self.help_overlay;
                }
                Action::OpenPalette => {
                    self.prompt = PromptState::Command {
                        input: String::new(),
                        selected: 0,
                        searching: false,
                    };
                    self.set_info("Command palette opened.");
                }
                Action::FocusNext => {
                    self.focus = self.focus.next();
                }
                Action::FocusPrev => {
                    self.focus = self.focus.previous();
                }
                Action::ToggleSidebar => {
                    self.left_collapsed = !self.left_collapsed;
                }
                Action::ToggleResizeMode => {
                    self.resize_mode = !self.resize_mode;
                    if self.resize_mode {
                        self.set_info("Resize mode on: h/l/←/→ resize side panes.");
                    } else {
                        self.persist_pane_widths();
                        self.set_info("Resize mode off.");
                    }
                }
                _ => {}
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
        let item_count = self.left_items().len();
        if let Some(action) = keybindings::lookup(&key, BindingScope::Left) {
            match action {
                Action::MoveDown => {
                    if self.selected_left + 1 < item_count {
                        self.selected_left += 1;
                        self.reload_changed_files();
                    }
                }
                Action::MoveUp => {
                    if self.selected_left > 0 {
                        self.selected_left -= 1;
                        self.reload_changed_files();
                    }
                }
                Action::FocusAgent => {
                    match self.left_items().get(self.selected_left) {
                        Some(LeftItem::Project(project_index)) => {
                            let project_id = self.projects[*project_index].id.clone();
                            let has_sessions =
                                self.sessions.iter().any(|s| s.project_id == project_id);
                            if has_sessions {
                                if self.collapsed_projects.contains(&project_id) {
                                    self.collapsed_projects.remove(&project_id);
                                    self.rebuild_left_items();
                                }
                                if let Some(pos) =
                                    self.left_items().iter().position(|item| {
                                        matches!(item, LeftItem::Session(si) if self.sessions[*si].project_id == project_id)
                                    })
                                {
                                    self.selected_left = pos;
                                    self.center_mode = CenterMode::Agent;
                                    self.focus = FocusPane::Center;
                                    self.reload_changed_files();
                                    if self
                                        .selected_session()
                                        .map(|s| self.providers.contains_key(&s.id))
                                        .unwrap_or(false)
                                    {
                                        self.input_target = InputTarget::Agent;
                                    }
                                }
                            } else {
                                self.create_agent_for_selected_project()?;
                            }
                        }
                        Some(LeftItem::Session(_)) => {
                            self.center_mode = CenterMode::Agent;
                            self.focus = FocusPane::Center;
                            self.reload_changed_files();
                            if self
                                .selected_session()
                                .map(|s| self.providers.contains_key(&s.id))
                                .unwrap_or(false)
                            {
                                self.input_target = InputTarget::Agent;
                            }
                        }
                        None => {}
                    }
                }
                Action::OpenProjectBrowser => {
                    self.open_project_browser()?;
                }
                Action::NewAgent => self.create_agent_for_selected_project()?,
                Action::RefreshProject => self.refresh_selected_project()?,
                Action::DeleteSession => self.confirm_delete_selected_session()?,
                Action::CycleProvider => self.cycle_selected_project_provider()?,
                Action::ReconnectAgent => self.reconnect_selected_session()?,
                Action::CopyPath => self.copy_selected_path()?,
                Action::ToggleProject => self.toggle_collapse_selected_project(),
                Action::InteractAgent => {
                    if self.selected_session().is_some()
                        && self
                            .selected_session()
                            .map(|s| self.providers.contains_key(&s.id))
                            .unwrap_or(false)
                    {
                        self.focus = FocusPane::Center;
                        self.center_mode = CenterMode::Agent;
                        self.input_target = InputTarget::Agent;
                        self.set_info(
                            "Interactive mode. Keys forwarded to agent. ctrl+g exits.",
                        );
                    } else {
                        self.set_error(
                            "No active agent. Press \"r\" to restart or \"n\" to create a new one.",
                        );
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn handle_center_key(&mut self, key: KeyEvent) -> Result<()> {
        if let Some(action) = keybindings::lookup(&key, BindingScope::Center) {
            match action {
                Action::InteractAgent => {
                    if self.selected_session().is_some()
                        && self
                            .selected_session()
                            .map(|s| self.providers.contains_key(&s.id))
                            .unwrap_or(false)
                    {
                        self.reset_pty_scrollback();
                        self.input_target = InputTarget::Agent;
                        self.set_info(
                            "Interactive mode. Keys forwarded to agent. ctrl+g exits.",
                        );
                    } else {
                        self.set_error(
                            "No active agent. Press \"r\" to restart or \"n\" to create a new one.",
                        );
                    }
                }
                Action::ReconnectAgent => {
                    // Allow relaunching an exited agent from the center pane,
                    // or entering interactive mode if the agent is active.
                    let has_provider = self
                        .selected_session()
                        .map(|s| self.providers.contains_key(&s.id))
                        .unwrap_or(false);
                    if has_provider {
                        self.reset_pty_scrollback();
                        self.input_target = InputTarget::Agent;
                    } else if self.selected_session().is_some() {
                        self.reconnect_selected_session()?;
                    }
                }
                Action::ScrollPageUp => {
                    self.scroll_pty(ScrollDirection::Up, self.last_pty_size.0 as usize);
                }
                Action::ScrollPageDown => {
                    self.scroll_pty(ScrollDirection::Down, self.last_pty_size.0 as usize);
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn scroll_pty(&mut self, direction: ScrollDirection, amount: usize) {
        let sid = match self.selected_session() {
            Some(s) => s.id.clone(),
            None => return,
        };
        let provider = match self.providers.get(&sid) {
            Some(p) => p,
            None => return,
        };
        let up = matches!(direction, ScrollDirection::Up);
        provider.scroll(up, amount);
    }

    fn reset_pty_scrollback(&self) {
        if let Some(session) = self.selected_session() {
            if let Some(provider) = self.providers.get(&session.id) {
                provider.set_scrollback(0);
            }
        }
    }

    fn handle_files_key(&mut self, key: KeyEvent) -> Result<()> {
        if let Some(action) = keybindings::lookup(&key, BindingScope::Files) {
            match action {
                Action::MoveDown => {
                    if self.selected_file + 1 < self.changed_files.len() {
                        self.selected_file += 1;
                    }
                }
                Action::MoveUp => {
                    if self.selected_file > 0 {
                        self.selected_file -= 1;
                    }
                }
                Action::OpenDiff => self.open_diff_for_selected_file()?,
                _ => {}
            }
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
            KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Map ctrl+a..=ctrl+z to the corresponding control character (0x01..=0x1a).
                if c.is_ascii_lowercase() {
                    let ctrl_byte = c as u8 - b'a' + 1;
                    let _ = provider.write_bytes(&[ctrl_byte]);
                }
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
            KeyCode::PageUp => {
                let _ = provider.write_bytes(b"\x1b[5~");
            }
            KeyCode::PageDown => {
                let _ = provider.write_bytes(b"\x1b[6~");
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
                    let count = keybindings::filtered_palette(input).len();
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
                    let count = keybindings::filtered_palette(input).len();
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
                    if let Some(binding) = keybindings::filtered_palette(input).get(*selected) {
                        *input = binding.palette.as_ref().unwrap().name.to_string();
                        *selected = 0;
                    }
                }
                KeyCode::Enter => {
                    if *searching {
                        *searching = false;
                    } else {
                        let command = if let Some(binding) = keybindings::filtered_palette(input).get(*selected)
                        {
                            binding.palette.as_ref().unwrap().name.to_string()
                        } else {
                            input.trim().to_string()
                        };
                        self.prompt = PromptState::None;
                        if let Err(e) = self.execute_command(command) {
                            self.set_error(format!("{e:#}"));
                        }
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
            loading,
            selected,
            filter,
            searching,
            editing_path,
            path_input,
            tab_completions,
            tab_index,
        } = &mut self.prompt
        {
            let mut browse_to: Option<PathBuf> = None;

            if *editing_path {
                let mut error_msg = None;
                match key.code {
                    KeyCode::Esc => {
                        *editing_path = false;
                        path_input.clear();
                        tab_completions.clear();
                        *tab_index = 0;
                    }
                    KeyCode::Backspace => {
                        path_input.pop();
                        tab_completions.clear();
                        *tab_index = 0;
                    }
                    KeyCode::Tab | KeyCode::BackTab => {
                        if tab_completions.is_empty() {
                            // Build completions from current input
                            let input_path = PathBuf::from(path_input.as_str());
                            let (search_dir, prefix) = if input_path.is_dir()
                                && path_input.ends_with('/')
                            {
                                (input_path.clone(), String::new())
                            } else {
                                let parent = input_path
                                    .parent()
                                    .unwrap_or_else(|| std::path::Path::new("/"));
                                let file_name = input_path
                                    .file_name()
                                    .map(|f| f.to_string_lossy().to_string())
                                    .unwrap_or_default();
                                (parent.to_path_buf(), file_name)
                            };
                            if let Ok(read) = std::fs::read_dir(&search_dir) {
                                let prefix_lower = prefix.to_lowercase();
                                let mut candidates: Vec<String> = read
                                    .filter_map(|e| e.ok())
                                    .filter(|e| {
                                        e.file_type().map(|ft| ft.is_dir()).unwrap_or(false)
                                    })
                                    .filter(|e| {
                                        let name =
                                            e.file_name().to_string_lossy().to_lowercase();
                                        !name.starts_with('.')
                                            && name.starts_with(&prefix_lower)
                                    })
                                    .map(|e| {
                                        let mut full =
                                            search_dir.join(e.file_name()).to_string_lossy().to_string();
                                        full.push('/');
                                        full
                                    })
                                    .collect();
                                candidates.sort();
                                *tab_completions = candidates;
                                *tab_index = 0;
                            }
                        } else {
                            // Cycle through existing completions
                            if key.code == KeyCode::BackTab {
                                if *tab_index == 0 {
                                    *tab_index = tab_completions.len().saturating_sub(1);
                                } else {
                                    *tab_index -= 1;
                                }
                            } else {
                                *tab_index = (*tab_index + 1) % tab_completions.len();
                            }
                        }
                        if let Some(completion) = tab_completions.get(*tab_index) {
                            *path_input = completion.clone();
                        }
                    }
                    KeyCode::Enter => {
                        let new_dir = PathBuf::from(path_input.trim());
                        if new_dir.is_dir() {
                            *current_dir = new_dir.clone();
                            entries.clear();
                            *loading = true;
                            *selected = 0;
                            filter.clear();
                            browse_to = Some(new_dir);
                        } else {
                            error_msg = Some(format!("{} is not a directory.", path_input.trim()));
                        }
                        *editing_path = false;
                        path_input.clear();
                        tab_completions.clear();
                        *tab_index = 0;
                    }
                    KeyCode::Char(c) => {
                        if !key.modifiers.contains(KeyModifiers::CONTROL) {
                            path_input.push(c);
                            tab_completions.clear();
                            *tab_index = 0;
                        }
                    }
                    _ => {}
                }
                if let Some(msg) = error_msg {
                    self.set_error(msg);
                }
                if let Some(dir) = browse_to {
                    self.spawn_browser_entries(&dir);
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
                KeyCode::Char('g') if !*searching => {
                    *editing_path = true;
                    let mut p = current_dir.to_string_lossy().to_string();
                    if !p.ends_with('/') {
                        p.push('/');
                    }
                    *path_input = p;
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
                            self.prompt = PromptState::None;
                            if let Err(e) = self.add_project(path, String::new()) {
                                self.set_error(format!("{e:#}"));
                            }
                        } else {
                            let new_dir = entry.path.clone();
                            *current_dir = new_dir.clone();
                            entries.clear();
                            *loading = true;
                            *selected = 0;
                            filter.clear();
                            browse_to = Some(new_dir);
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
            if let Some(dir) = browse_to {
                self.spawn_browser_entries(&dir);
            }
            return Ok(false);
        }

        if let PromptState::ConfirmDeleteAgent {
            session_id,
            confirm_selected,
            ..
        } = &mut self.prompt
        {
            match key.code {
                KeyCode::Esc => self.prompt = PromptState::None,
                KeyCode::Left
                | KeyCode::Right
                | KeyCode::Tab
                | KeyCode::Char('h')
                | KeyCode::Char('l') => {
                    *confirm_selected = !*confirm_selected;
                }
                KeyCode::Enter => {
                    if *confirm_selected {
                        let id = session_id.clone();
                        self.prompt = PromptState::None;
                        if let Err(e) = self.do_delete_session(&id) {
                            self.set_error(format!("{e:#}"));
                        }
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
                KeyCode::Left
                | KeyCode::Right
                | KeyCode::Tab
                | KeyCode::Char('h')
                | KeyCode::Char('l') => {
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
            KeyCode::Char('h') | KeyCode::Left => {
                self.left_width_pct = self.left_width_pct.saturating_sub(2).max(14)
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.left_width_pct = self.left_width_pct.saturating_add(2).min(38)
            }
            _ => {}
        }
    }

    fn persist_pane_widths(&mut self) {
        if self.config.ui.left_width_pct != self.left_width_pct
            || self.config.ui.right_width_pct != self.right_width_pct
        {
            self.config.ui.left_width_pct = self.left_width_pct;
            self.config.ui.right_width_pct = self.right_width_pct;
            let _ = save_config(&self.paths.config_path, &self.config);
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
        self.prompt = PromptState::BrowseProjects {
            current_dir: start_dir.clone(),
            entries: Vec::new(),
            loading: true,
            selected: 0,
            filter: String::new(),
            searching: false,
            editing_path: false,
            path_input: String::new(),
            tab_completions: Vec::new(),
            tab_index: 0,
        };
        self.spawn_browser_entries(&start_dir);
        self.set_info(
            "Project browser: Enter opens or adds a repo, / to search, g to go to a path.",
        );
        Ok(())
    }

    fn spawn_browser_entries(&self, dir: &Path) {
        let tx = self.worker_tx.clone();
        let dir = dir.to_path_buf();
        thread::spawn(move || {
            let entries = browser_entries(&dir);
            logger::debug(&format!(
                "browser loaded {} with {} entries",
                dir.display(),
                entries.len()
            ));
            let _ = tx.send(WorkerEvent::BrowserEntriesReady {
                dir: dir.clone(),
                entries,
            });
        });
    }

    fn add_project(&mut self, raw_path: String, name: String) -> Result<()> {
        let path = PathBuf::from(raw_path.trim())
            .canonicalize()
            .unwrap_or_else(|_| PathBuf::from(raw_path.trim()));
        logger::info(&format!("attempting to add project {}", path.display()));
        if !path.exists() || !git::is_git_repo(&path) {
            logger::error(&format!("add project rejected for {}", path.display()));
            self.set_error(format!("\"{}\" is not a git repository.", path.display()));
            return Ok(());
        }
        if self
            .projects
            .iter()
            .any(|project| Path::new(&project.path) == path.as_path())
        {
            self.set_error(format!(
                "\"{}\" is already registered as a project.",
                path.display()
            ));
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
        let project_id = Uuid::new_v4().to_string();
        self.config.projects.push(ProjectConfig {
            id: project_id.clone(),
            path: path.to_string_lossy().to_string(),
            name: Some(display_name.clone()),
            default_provider: None,
        });
        save_config(&self.paths.config_path, &self.config)?;
        self.projects.push(Project {
            id: project_id,
            name: display_name.clone(),
            path: path.to_string_lossy().to_string(),
            default_provider: self.config.default_provider(),
            current_branch: branch,
        });
        self.rebuild_left_items();
        logger::info(&format!("registered project {}", path.display()));
        self.set_info(format!("Added project \"{}\" to workspace", display_name));
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
        self.set_busy(format!(
            "Creating worktree for project \"{}\"...",
            project.name
        ));
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
        self.set_info(format!(
            "Refreshed project \"{}\": {}",
            project.name,
            output.trim()
        ));
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
        let Some(session) = self.sessions.iter().find(|s| s.id == session_id).cloned() else {
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
        self.rebuild_left_items();
        self.selected_left = self.selected_left.saturating_sub(1);
        self.reload_changed_files();
        self.set_info(format!(
            "Deleted {} agent from project \"{}\" with branch \"{}\"",
            session.provider.as_str(),
            project.name,
            session.branch_name
        ));
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
        for session in self
            .sessions
            .iter_mut()
            .filter(|s| s.project_id == project.id)
        {
            session.provider = next.clone();
            self.session_store.upsert_session(session)?;
        }
        self.set_info(format!("Changed CLI agent to \"{}\"", next.as_str()));
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
        self.rebuild_left_items();
        self.selected_left = self.selected_left.saturating_sub(1);
        self.reload_changed_files();
        self.set_info(format!(
            "Deleted project \"{}\" and all its agents",
            project.name
        ));
        Ok(())
    }

    fn reconnect_selected_session(&mut self) -> Result<()> {
        let Some(session) = self.selected_session().cloned() else {
            self.set_error("Select a stopped agent first to reconnect.");
            return Ok(());
        };
        logger::info(&format!("reconnecting session {}", session.id));
        if self.providers.contains_key(&session.id) {
            self.set_info(format!(
                "Agent \"{}\" is already connected.",
                session.branch_name
            ));
            return Ok(());
        }
        if !Path::new(&session.worktree_path).exists() {
            self.set_error(format!(
                "Worktree for agent \"{}\" no longer exists. Delete and re-create the agent.",
                session.branch_name
            ));
            return Ok(());
        }
        match self.spawn_pty_for_session(&session) {
            Ok(client) => {
                self.providers.insert(session.id.clone(), client);
                self.mark_session_status(&session.id, SessionStatus::Active);
                self.focus = FocusPane::Center;
                self.center_mode = CenterMode::Agent;
                self.input_target = InputTarget::Agent;
                let proj_name = self.project_name_for_session(&session);
                self.set_info(format!(
                    "Relaunched {} agent \"{}\" in project \"{}\"",
                    session.provider.as_str(),
                    session.branch_name,
                    proj_name
                ));
            }
            Err(err) => {
                self.set_error(format!(
                    "Reconnect failed for agent \"{}\": {err}",
                    session.branch_name
                ));
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
        let output =
            crate::diff::diff_file(Path::new(&session.worktree_path), &file.path, &self.theme)?;
        self.center_mode = CenterMode::Diff(output.lines);
        self.focus = FocusPane::Center;
        Ok(())
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.worker_rx.try_recv() {
            match event {
                WorkerEvent::CreateAgentProgress(message) => self.set_busy(message),
                WorkerEvent::CreateAgentReady {
                    session,
                    client,
                    pty_size,
                } => {
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
                    self.rebuild_left_items();
                    self.selected_left = self
                        .left_items()
                        .iter()
                        .position(|item| matches!(item, LeftItem::Session(index) if self.sessions.get(*index).map(|candidate| candidate.id.as_str()) == Some(session.id.as_str())))
                        .unwrap_or(0);
                    self.reload_changed_files();
                    self.focus = FocusPane::Center;
                    self.center_mode = CenterMode::Agent;
                    self.input_target = InputTarget::Agent;
                    let proj_name = self.project_name_for_session(&session);
                    self.set_info(format!(
                        "Created {} agent \"{}\" in project \"{}\"",
                        session.provider.as_str(),
                        session.branch_name,
                        proj_name
                    ));
                }
                WorkerEvent::CreateAgentFailed(message) => {
                    self.create_agent_in_flight = false;
                    self.set_error(message);
                }
                WorkerEvent::ChangedFilesReady(files) => {
                    self.changed_files = files;
                    if self.selected_file >= self.changed_files.len() {
                        self.selected_file = self.changed_files.len().saturating_sub(1);
                    }
                }
                WorkerEvent::BrowserEntriesReady { dir, entries } => {
                    if let PromptState::BrowseProjects {
                        current_dir,
                        entries: current_entries,
                        loading,
                        selected,
                        ..
                    } = &mut self.prompt
                    {
                        if *current_dir == dir {
                            *current_entries = entries;
                            *loading = false;
                            *selected = 0;
                        }
                    }
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
                    self.set_info("Agent CLI process has exited. Press \"r\" to relaunch.");
                }
            }
        }
        // Keep the poller's interval flag in sync with whether any agent is running.
        self.has_active_agent
            .store(!self.providers.is_empty(), Ordering::Relaxed);
    }

    fn render(&mut self, frame: &mut Frame) {
        let term_w = frame.area().width as usize;
        let status_text_len = self.status.text().len() + 3; // " ● " prefix
        let status_lines: u16 = if term_w > 0 && status_text_len > term_w {
            2
        } else {
            1
        };
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
        self.render_left(frame, left);
        self.render_center(frame, center);
        self.render_files(frame, right);
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
            let collapsed_left_items = self.left_items();
            let items = collapsed_left_items
                .iter()
                .enumerate()
                .map(|(i, item)| match item {
                    LeftItem::Project(index) => {
                        let project = &self.projects[*index];
                        let icon = if self.collapsed_projects.contains(&project.id) {
                            "▸"
                        } else {
                            "▾"
                        };
                        ListItem::new(Line::from(Span::styled(
                            icon,
                            Style::default().fg(self.theme.project_icon),
                        )))
                    }
                    LeftItem::Session(index) => {
                        let session = &self.sessions[*index];
                        let (dot, dot_color) = self.theme.session_dot(&session.status);
                        let is_last = !collapsed_left_items
                            .get(i + 1)
                            .is_some_and(|next| matches!(next, LeftItem::Session(_)));
                        let connector = if is_last { "└" } else { "├" };
                        ListItem::new(Line::from(vec![
                            Span::styled(connector, Style::default().fg(self.theme.project_icon)),
                            Span::styled(dot.to_string(), Style::default().fg(dot_color)),
                        ]))
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

        let session_counts: HashMap<String, usize> = {
            let mut counts = HashMap::new();
            for session in &self.sessions {
                *counts.entry(session.project_id.clone()).or_insert(0) += 1;
            }
            counts
        };
        let left_items = self.left_items();
        let items = left_items
            .iter()
            .enumerate()
            .map(|(i, item)| match item {
                LeftItem::Project(index) => {
                    let project = &self.projects[*index];
                    let count = session_counts.get(&project.id).copied().unwrap_or(0);
                    let icon = if self.collapsed_projects.contains(&project.id) {
                        "▸ "
                    } else {
                        "▾ "
                    };
                    let mut spans = vec![
                        Span::styled(icon, Style::default().fg(self.theme.project_icon)),
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
                    let session = &self.sessions[*index];
                    let is_last = !left_items
                        .get(i + 1)
                        .is_some_and(|next| matches!(next, LeftItem::Session(_)));
                    let connector = if is_last { "└ " } else { "├ " };
                    let label = session
                        .title
                        .clone()
                        .unwrap_or_else(|| session.branch_name.clone());
                    let (dot, dot_color) = self.theme.session_dot(&session.status);
                    ListItem::new(Line::from(vec![
                        Span::styled(connector, Style::default().fg(self.theme.project_icon)),
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
            CenterMode::Diff(diff_lines) => {
                Paragraph::new(diff_lines.clone())
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
        let mut scrollback_offset: usize = 0;

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
                                Span::styled("...", Style::default().fg(self.theme.hint_desc_fg)),
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
                    // Use a single lock to get both screen and scrollback
                    // offset atomically, avoiding race conditions with the
                    // background reader thread.
                    let (screen, sb_offset) = provider.screen_and_scrollback();
                    scrollback_offset = sb_offset;
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
                                let style = Style::default().fg(fg).bg(bg).add_modifier(modifier);
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
                        if cx < term_area.x + term_area.width && cy < term_area.y + term_area.height
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
                let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
                let mut spans: Vec<Span> = Vec::new();
                let cli_name = session_provider_name.as_deref().unwrap_or("agent");
                let capitalized = {
                    let mut c = cli_name.chars();
                    match c.next() {
                        Some(first) => format!("{}{}", first.to_uppercase(), c.as_str()),
                        None => String::new(),
                    }
                };
                spans.push(Span::styled(
                    format!("{capitalized} is holding focus. Press "),
                    desc_style,
                ));
                spans.extend(self.theme.dim_key_badge("^G", Color::Reset));
                spans.push(Span::styled(
                    " to bring the focus back to dux.",
                    desc_style,
                ));
                Line::from(spans)
            } else if scrollback_offset > 0 {
                let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
                let mut spans: Vec<Span> = Vec::new();
                spans.push(Span::styled(
                    format!("Scrolled back {scrollback_offset} lines. "),
                    Style::default().fg(self.theme.hint_key_fg),
                ));
                spans.extend(self.theme.dim_key_badge("ctrl+f", Color::Reset));
                spans.push(Span::styled("/", desc_style));
                spans.extend(self.theme.dim_key_badge("PgDn", Color::Reset));
                spans.push(Span::styled(" down, ", desc_style));
                spans.extend(self.theme.dim_key_badge("ctrl+b", Color::Reset));
                spans.push(Span::styled("/", desc_style));
                spans.extend(self.theme.dim_key_badge("PgUp", Color::Reset));
                spans.push(Span::styled(" up.", desc_style));
                Line::from(spans)
            } else {
                let desc_style = Style::default().fg(self.theme.hint_dim_desc_fg);
                let mut spans: Vec<Span> = Vec::new();
                if session_active {
                    spans.push(Span::styled("Press ", desc_style));
                    spans.extend(self.theme.dim_key_badge("i", Color::Reset));
                    spans.push(Span::styled(" or ", desc_style));
                    spans.extend(self.theme.dim_key_badge("enter", Color::Reset));
                    spans.push(Span::styled(" to interact with the agent. ", desc_style));
                    spans.extend(self.theme.dim_key_badge("^B", Color::Reset));
                    spans.push(Span::styled("/", desc_style));
                    spans.extend(self.theme.dim_key_badge("PgUp", Color::Reset));
                    spans.push(Span::styled(" ", desc_style));
                    spans.extend(self.theme.dim_key_badge("^F", Color::Reset));
                    spans.push(Span::styled("/", desc_style));
                    spans.extend(self.theme.dim_key_badge("PgDn", Color::Reset));
                    spans.push(Span::styled(" to scroll.", desc_style));
                } else if session_id.is_some() {
                    spans.push(Span::styled("Agent CLI exited. Press ", desc_style));
                    spans.extend(self.theme.dim_key_badge("r", Color::Reset));
                    spans.push(Span::styled(" or ", desc_style));
                    spans.extend(self.theme.dim_key_badge("enter", Color::Reset));
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
        let inner_width = area.width.saturating_sub(2) as usize; // minus borders
        let sel_style = self.theme.selection_style();
        let items = self
            .changed_files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                let is_selected = index == self.selected_file;

                // Build the right-aligned stats string, e.g. "+12 -3".
                let stats = format_line_stats(file.additions, file.deletions);
                let stats_width = stats.iter().map(|s| s.width()).sum::<usize>();

                // Status prefix takes 3 chars ("M  ").
                let prefix_width = 3;
                // Leave 1 char gap between path and stats.
                let path_budget = inner_width
                    .saturating_sub(prefix_width)
                    .saturating_sub(stats_width)
                    .saturating_sub(1);

                let path = if is_selected {
                    file.path.clone()
                } else {
                    git::ellipsize_middle(&file.path, path_budget.max(10))
                };

                let path_display_width = path.chars().count();
                let padding = inner_width
                    .saturating_sub(prefix_width)
                    .saturating_sub(path_display_width)
                    .saturating_sub(stats_width);

                let base_style = if is_selected { sel_style } else { Style::default() };

                let mut spans = vec![
                    Span::styled(
                        format!("{:>2} ", file.status),
                        base_style.fg(self.theme.file_status_fg),
                    ),
                    Span::styled(path, base_style),
                    Span::styled(" ".repeat(padding), base_style),
                ];
                // For stats spans, keep their green/red fg but apply selection bg when selected.
                let stats_base = if is_selected {
                    Style::default()
                        .bg(self.theme.selection_bg)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                spans.extend(stats.into_iter().map(|s| {
                    let fg = s.style.fg.unwrap_or(Color::Reset);
                    Span::styled(s.content, stats_base.fg(fg))
                }));
                ListItem::new(Line::from(spans))
            })
            .collect::<Vec<_>>();
        let focused = self.focus == FocusPane::Files;
        let title = format!("Changed Files ({})", self.changed_files.len());
        let mut state = ListState::default().with_selected(Some(self.selected_file));
        StatefulWidget::render(
            List::new(items)
                .block(self.themed_block(&title, focused)),
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
        let ctx = match self.focus {
            FocusPane::Left if is_on_project => HintContext::LeftProject,
            FocusPane::Left => HintContext::LeftSession,
            FocusPane::Center => HintContext::Center,
            FocusPane::Files => HintContext::Files,
        };
        let hints = keybindings::hints_for(ctx);
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
            Span::styled(prefix, Style::default().fg(dot_color).bg(status_bg)),
            Span::styled(truncated, Style::default().fg(msg_color).bg(status_bg)),
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
        let help_bindings = keybindings::help_sections();
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
            for (key, desc) in bindings {
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
        // Key notation legend
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Key notation",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )));
        {
            let key = "^X";
            let desc = "Hold Ctrl and press X (e.g. ^P = Ctrl+P)";
            let padding = 14usize.saturating_sub(key.len() + 2);
            let mut spans = vec![Span::raw("  ")];
            spans.extend(self.theme.key_badge(key, Color::Reset));
            spans.push(Span::raw(" ".repeat(padding)));
            spans.push(Span::styled(
                desc,
                Style::default().fg(self.theme.hint_desc_fg),
            ));
            lines.push(Line::from(spans));
        }
        // Session state legend
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Session states",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        )));
        let session_states: &[(&str, Color, &str)] = &[
            ("●", self.theme.session_active, "Active — agent is running"),
            (
                "◐",
                self.theme.session_detached,
                "Detached — agent process disconnected",
            ),
            (
                "○",
                self.theme.session_exited,
                "Exited — agent has finished",
            ),
        ];
        for (dot, color, desc) in session_states {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(*dot, Style::default().fg(*color)),
                Span::raw("  "),
                Span::styled(
                    desc.to_string(),
                    Style::default().fg(self.theme.hint_desc_fg),
                ),
            ]));
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
                let commands = keybindings::filtered_palette(input);
                let items = if commands.is_empty() {
                    vec![ListItem::new("No matching commands.")]
                } else {
                    commands
                        .iter()
                        .map(|binding| {
                            let p = binding.palette.as_ref().unwrap();
                            let mut left_spans = vec![
                                Span::styled(
                                    p.name.to_string(),
                                    Style::default()
                                        .fg(Color::Cyan)
                                        .add_modifier(Modifier::BOLD),
                                ),
                                Span::styled(
                                    format!("  {}", p.description),
                                    Style::default().fg(self.theme.hint_desc_fg),
                                ),
                            ];
                            if let Some(shortcut) = p.shortcut {
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
                loading,
                selected,
                filter,
                searching,
                editing_path,
                path_input,
                ..
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
                let items = if *loading {
                    let spinner = match (self.tick_count / 2) % 4 {
                        0 => "⠋",
                        1 => "⠙",
                        2 => "⠹",
                        _ => "⠸",
                    };
                    vec![ListItem::new(Line::from(vec![
                        Span::styled(
                            format!("{spinner} "),
                            Style::default().fg(self.theme.hint_desc_fg),
                        ),
                        Span::raw("Loading…"),
                    ]))]
                } else if visible.is_empty() {
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
                        bottom_spans.extend(self.theme.key_badge("Tab", Color::Reset));
                        bottom_spans.push(Span::styled(" complete  ", Style::default().fg(self.theme.hint_desc_fg)));
                        bottom_spans.extend(self.theme.key_badge("Enter", Color::Reset));
                        bottom_spans.push(Span::styled(
                            " go  ",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
                        bottom_spans.extend(self.theme.key_badge("Esc", Color::Reset));
                        bottom_spans.push(Span::styled(
                            " cancel",
                            Style::default().fg(self.theme.hint_desc_fg),
                        ));
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
                let left_offset = buttons_area.width.saturating_sub(total) / 2;

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

                let agent_word = if *active_count == 1 {
                    "agent"
                } else {
                    "agents"
                };
                let lines = vec![
                    Line::from(""),
                    Line::from(vec![
                        Span::raw(format!(" {active_count} running {agent_word} will be ")),
                        Span::styled(
                            "killed",
                            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                        ),
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
        if matches!(self.center_mode, CenterMode::Diff(_)) {
            self.center_mode = CenterMode::Agent;
            self.focus = FocusPane::Files;
            self.set_info("Returned to agent view.");
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

    fn left_items(&self) -> &[LeftItem] {
        &self.left_items_cache
    }

    fn rebuild_left_items(&mut self) {
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

    fn toggle_collapse_selected_project(&mut self) {
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

    fn project_name_for_session(&self, session: &AgentSession) -> String {
        self.projects
            .iter()
            .find(|p| p.id == session.project_id)
            .map(|p| p.name.clone())
            .unwrap_or_else(|| "unknown".to_string())
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
                self.set_info(format!("Copied path to clipboard: \"{p}\""));
                Ok(())
            }
            None => {
                self.set_error("No project or agent selected. Select one from the sidebar first.");
                Ok(())
            }
        }
    }

    fn reload_changed_files(&mut self) {
        let worktree = self
            .selected_session()
            .map(|s| PathBuf::from(&s.worktree_path));
        // Keep the background poller in sync with the currently selected session.
        if let Ok(mut guard) = self.watched_worktree.lock() {
            *guard = worktree.clone();
        }
        self.changed_files = worktree
            .and_then(|p| git::changed_files(&p).ok())
            .unwrap_or_default();
        if self.selected_file >= self.changed_files.len() {
            self.selected_file = self.changed_files.len().saturating_sub(1);
        }
    }

    fn spawn_changed_files_poller(&self) {
        let tx = self.worker_tx.clone();
        let watched = Arc::clone(&self.watched_worktree);
        let has_agent = Arc::clone(&self.has_active_agent);
        thread::spawn(move || {
            loop {
                let interval = if has_agent.load(Ordering::Relaxed) {
                    Duration::from_secs(2)
                } else {
                    Duration::from_secs(10)
                };
                thread::sleep(interval);
                let path = watched.lock().ok().and_then(|guard| guard.clone());
                if let Some(worktree_path) = path {
                    if let Ok(files) = git::changed_files(&worktree_path) {
                        if tx.send(WorkerEvent::ChangedFilesReady(files)).is_err() {
                            break; // receiver dropped, app is shutting down
                        }
                    }
                }
            }
        });
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
            .title(Line::from(Span::styled(
                title,
                self.theme.title_style(focused),
            )))
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
        let full = frame.area();
        // Keep the statusline (bottom rows) undimmed so errors stay visible.
        let status_text_len = self.status.text().len() + 3;
        let status_lines: u16 = if full.width > 0 && status_text_len > full.width as usize {
            2
        } else {
            1
        };
        let footer_h = 1 + status_lines; // hints bar + status line(s)
        let dim_h = full.height.saturating_sub(footer_h);
        let buf = frame.buffer_mut();
        for y in full.y..full.y + dim_h {
            for x in full.x..full.x + full.width {
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
        "Creating worktree for project \"{}\"...",
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
        project_id: project.id.clone(),
        project_path: Some(project.path.clone()),
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
    let _ = worker_tx.send(WorkerEvent::CreateAgentReady {
        session,
        client,
        pty_size: (rows, cols),
    });
}

/// Format additions/deletions as right-aligned colored spans.
/// Returns an empty vec when both counts are zero.
fn format_line_stats(additions: usize, deletions: usize) -> Vec<Span<'static>> {
    if additions == 0 && deletions == 0 {
        return Vec::new();
    }
    let mut spans = Vec::new();
    if additions > 0 {
        spans.push(Span::styled(
            format!("+{additions}"),
            Style::default().fg(Color::Green),
        ));
    }
    if additions > 0 && deletions > 0 {
        spans.push(Span::raw(" "));
    }
    if deletions > 0 {
        spans.push(Span::styled(
            format!("-{deletions}"),
            Style::default().fg(Color::Red),
        ));
    }
    spans
}

fn load_projects(config: &Config) -> Vec<Project> {
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
            let is_git_repo = path.join(".git").exists();
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

