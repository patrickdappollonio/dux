use std::collections::BTreeMap;
use std::env;
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::keybindings;
use crate::model::ProviderKind;

pub const DEFAULT_COMMIT_PROMPT: &str = "\
You are a commit message generator. Look at the staged changes (git diff --cached) and write a commit message.

Rules:
- Subject line: use Conventional Commits (feat:, fix:, refactor:, docs:, test:, chore:, style:, perf:, ci:, build:). Imperative mood, max 72 chars, no period at the end.
- Trivial changes (typo, rename, one-liner): ONLY the subject line, nothing else.
- Small changes (2-3 files, single logical concern): subject line, blank line, then a short paragraph (2-3 sentences max) explaining the motivation and impact. Do NOT use bullet points for this case.
- Larger changes (4+ files or multiple distinct logical concerns): subject line, blank line, then concise bullet points (one per logical change, each under 80 chars). Use \"- \" for bullets.
- This is a plain text commit message, not markdown. NEVER use backticks, asterisks, code fences, or any markdown syntax. Refer to functions and files by name without formatting.
- Focus on intent and impact, not mechanical description of lines added/removed.
- Output ONLY the raw commit message text.";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub defaults: Defaults,
    pub providers: ProvidersConfig,
    pub terminal: TerminalConfig,
    pub logging: LoggingConfig,
    pub projects: Vec<ProjectConfig>,
    pub ui: UiConfig,
    pub editor: EditorConfig,
    pub keys: KeysConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct KeysConfig {
    pub show_terminal_keys: bool,
    #[serde(flatten)]
    pub bindings: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Defaults {
    pub provider: String,
    pub start_directory: Option<String>,
    pub commit_prompt: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ProvidersConfig {
    #[serde(flatten)]
    pub commands: IndexMap<String, ProviderCommandConfig>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalConfig {
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct EditorConfig {
    pub default: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OneshotOutput {
    Stdout,
    Tempfile,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderCommandConfig {
    pub command: String,
    pub args: Vec<String>,
    pub resume_args: Option<Vec<String>>,
    pub oneshot_args: Vec<String>,
    pub oneshot_output: OneshotOutput,
    pub install_hint: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectConfig {
    #[serde(default = "new_project_id")]
    pub id: String,
    pub path: String,
    pub name: Option<String>,
    pub default_provider: Option<String>,
    pub commit_prompt: Option<String>,
}

fn new_project_id() -> String {
    Uuid::new_v4().to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub left_width_pct: u16,
    pub right_width_pct: u16,
    pub terminal_pane_height_pct: u16,
    pub staged_pane_height_pct: u16,
    pub agent_scrollback_lines: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            defaults: Defaults::default(),
            providers: ProvidersConfig::default(),
            terminal: TerminalConfig::default(),
            logging: LoggingConfig {
                level: "info".to_string(),
                path: "dux.log".to_string(),
            },
            projects: Vec::new(),
            ui: UiConfig {
                left_width_pct: 20,
                right_width_pct: 23,
                terminal_pane_height_pct: 35,
                staged_pane_height_pct: 50,
                agent_scrollback_lines: 10_000,
            },
            editor: EditorConfig::default(),
            keys: KeysConfig::default(),
        }
    }
}

impl Default for KeysConfig {
    fn default() -> Self {
        let mut bindings = BTreeMap::new();
        for def in keybindings::BINDING_DEFS {
            if def.default_keys.is_empty() {
                continue;
            }
            let keys: Vec<String> = def
                .default_keys
                .iter()
                .map(|k| keybindings::format_key_for_config(*k))
                .collect();
            bindings.insert(def.action.config_name().to_string(), keys);
        }
        Self {
            show_terminal_keys: true,
            bindings,
        }
    }
}

impl Default for Defaults {
    fn default() -> Self {
        let start_directory = home::home_dir().map(|p| p.to_string_lossy().to_string());
        Self {
            provider: "claude".to_string(),
            start_directory,
            commit_prompt: Some(DEFAULT_COMMIT_PROMPT.to_string()),
        }
    }
}

impl Default for ProvidersConfig {
    fn default() -> Self {
        let commands = default_provider_commands()
            .into_iter()
            .map(|(name, config)| (name.to_string(), config))
            .collect();
        Self { commands }
    }
}

impl Default for ProviderCommandConfig {
    fn default() -> Self {
        Self {
            command: String::new(),
            args: Vec::new(),
            resume_args: None,
            oneshot_args: Vec::new(),
            oneshot_output: OneshotOutput::Stdout,
            install_hint: None,
        }
    }
}

impl ProviderCommandConfig {
    pub fn interactive_args(&self, resume_session: bool) -> &[String] {
        if resume_session
            && let Some(resume_args) = self.resume_args.as_deref().filter(|args| !args.is_empty())
        {
            return resume_args;
        }
        &self.args
    }

    pub fn supports_session_resume(&self) -> bool {
        self.resume_args
            .as_ref()
            .map(|args| !args.is_empty())
            .unwrap_or(false)
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            path: "dux.log".to_string(),
        }
    }
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            command: default_terminal_command(),
            args: default_terminal_args(),
        }
    }
}

impl Default for EditorConfig {
    fn default() -> Self {
        Self {
            default: "cursor".to_string(),
        }
    }
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            left_width_pct: 17,
            right_width_pct: 19,
            terminal_pane_height_pct: 35,
            staged_pane_height_pct: 50,
            agent_scrollback_lines: 10_000,
        }
    }
}

impl Config {
    pub fn default_provider(&self) -> ProviderKind {
        ProviderKind::from_str(&self.defaults.provider)
    }

    /// Returns the effective commit prompt for a project, checking project-level
    /// override first, then system default, then the hardcoded fallback.
    pub fn commit_prompt_for_project(&self, project_path: &str) -> String {
        if let Some(project) = self.projects.iter().find(|p| p.path == project_path)
            && let Some(ref prompt) = project.commit_prompt
            && !prompt.is_empty()
        {
            return prompt.clone();
        }
        self.defaults
            .commit_prompt
            .as_ref()
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| DEFAULT_COMMIT_PROMPT.to_string())
    }
}

impl ProvidersConfig {
    pub fn get(&self, name: &str) -> Option<&ProviderCommandConfig> {
        self.commands.get(name)
    }

    pub fn ensure_defaults(&mut self) {
        for (name, config) in default_provider_commands() {
            match self.commands.entry(name.to_string()) {
                indexmap::map::Entry::Vacant(entry) => {
                    entry.insert(config);
                }
                indexmap::map::Entry::Occupied(mut entry) => {
                    if entry.get().resume_args.is_none() {
                        entry.get_mut().resume_args = config.resume_args;
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct DuxPaths {
    pub root: PathBuf,
    pub config_path: PathBuf,
    pub sessions_db_path: PathBuf,
    pub worktrees_root: PathBuf,
}

impl DuxPaths {
    pub fn discover() -> Result<Self> {
        let home =
            home::home_dir().ok_or_else(|| anyhow!("failed to determine user home directory"))?;
        let root = discover_root(&home, env::var_os("XDG_CONFIG_HOME"));
        Ok(Self {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            root,
        })
    }

    pub fn ensure_dirs(&self) -> Result<()> {
        fs::create_dir_all(&self.root)
            .with_context(|| format!("failed to create {}", self.root.display()))?;
        fs::create_dir_all(&self.worktrees_root)
            .with_context(|| format!("failed to create {}", self.worktrees_root.display()))?;
        Ok(())
    }
}

pub fn ensure_config(paths: &DuxPaths) -> Result<Config> {
    paths.ensure_dirs()?;
    if !paths.config_path.exists() {
        fs::write(&paths.config_path, render_default_config())
            .with_context(|| format!("failed to write {}", paths.config_path.display()))?;
    }

    let raw = fs::read_to_string(&paths.config_path)
        .with_context(|| format!("failed to read {}", paths.config_path.display()))?;
    let mut config: Config = toml::from_str(&raw)
        .with_context(|| format!("failed to parse {}", paths.config_path.display()))?;
    config.providers.ensure_defaults();
    let bindings = crate::keybindings::RuntimeBindings::from_keys_config(&config.keys);
    let _ = save_config(&paths.config_path, &config, &bindings);
    Ok(config)
}

// ---------------------------------------------------------------------------
// Config schema: defines the layout, comments, and value accessors for the
// TOML config file. Adding a new setting means adding a struct field, its
// Default value, and one entry here — comments live in exactly one place.
// ---------------------------------------------------------------------------

/// A value extracted from [`Config`] for rendering into TOML.
enum FieldValue {
    Str(String),
    OptStr(Option<String>),
    U16(u16),
    Usize(usize),
    MultilineStr(Option<String>),
}

/// A comment source: static string or dynamically built.
enum CommentSource {
    Static(&'static str),
    Dynamic(String),
}

/// One entry in the config file layout.
enum ConfigEntry {
    /// A comment line (must include the leading `#`).
    Comment(&'static str),
    /// A blank line for spacing.
    Blank,
    /// A TOML section header, e.g. `[defaults]`.
    Section(&'static str),
    /// A key = value line with an optional comment above it.
    Field {
        key: &'static str,
        comment: Option<CommentSource>,
        value_fn: fn(&Config) -> FieldValue,
    },
    /// Renders all `[providers.*]` sub-tables dynamically.
    Providers,
    /// Renders the `[terminal]` section.
    Terminal,
    /// Renders the `[[projects]]` array.
    Projects,
    /// Renders the `[keys]` section with all keybindings.
    Keys,
}

fn config_schema(generate_commit_key: &str) -> Vec<ConfigEntry> {
    vec![
        ConfigEntry::Comment("# dux configuration"),
        ConfigEntry::Comment(
            "# Every value is materialized here so the file doubles as documentation.",
        ),
        ConfigEntry::Blank,
        ConfigEntry::Section("defaults"),
        ConfigEntry::Field {
            key: "provider",
            comment: Some(CommentSource::Static(
                "# Which provider new sessions use unless a project overrides it.",
            )),
            value_fn: |c| FieldValue::Str(c.defaults.provider.clone()),
        },
        ConfigEntry::Field {
            key: "start_directory",
            comment: Some(CommentSource::Static(
                "# Starting directory for the project browser.",
            )),
            value_fn: |c| FieldValue::OptStr(c.defaults.start_directory.clone()),
        },
        ConfigEntry::Blank,
        ConfigEntry::Field {
            key: "commit_prompt",
            comment: Some(CommentSource::Dynamic(format!(
                "# Prompt sent to the AI provider when generating commit messages ({generate_commit_key}).\n\
                 # The provider will inspect the staged diff on its own.\n\
                 # Override per-project by adding commit_prompt in a [[projects]] entry.",
            ))),
            value_fn: |c| FieldValue::MultilineStr(c.defaults.commit_prompt.clone()),
        },
        ConfigEntry::Blank,
        ConfigEntry::Providers,
        ConfigEntry::Terminal,
        ConfigEntry::Section("logging"),
        ConfigEntry::Field {
            key: "level",
            comment: Some(CommentSource::Static(
                "# Log level can be error, info, or debug.",
            )),
            value_fn: |c| FieldValue::Str(c.logging.level.clone()),
        },
        ConfigEntry::Field {
            key: "path",
            comment: Some(CommentSource::Static(
                "# Relative paths are resolved from the dux config directory.",
            )),
            value_fn: |c| FieldValue::Str(c.logging.path.clone()),
        },
        ConfigEntry::Blank,
        ConfigEntry::Section("ui"),
        ConfigEntry::Field {
            key: "left_width_pct",
            comment: Some(CommentSource::Static(
                "# Initial pane sizing percentages. They can still be resized at runtime.",
            )),
            value_fn: |c| FieldValue::U16(c.ui.left_width_pct),
        },
        ConfigEntry::Field {
            key: "right_width_pct",
            comment: None,
            value_fn: |c| FieldValue::U16(c.ui.right_width_pct),
        },
        ConfigEntry::Field {
            key: "terminal_pane_height_pct",
            comment: Some(CommentSource::Static(
                "# Maximum height percentage of the left pane used by the companion terminals list.",
            )),
            value_fn: |c| FieldValue::U16(c.ui.terminal_pane_height_pct),
        },
        ConfigEntry::Field {
            key: "staged_pane_height_pct",
            comment: Some(CommentSource::Static(
                "# Height percentage of the right pane used by the staged changes and commit sections.\n# The remaining space goes to the unstaged changes list.",
            )),
            value_fn: |c| FieldValue::U16(c.ui.staged_pane_height_pct),
        },
        ConfigEntry::Field {
            key: "agent_scrollback_lines",
            comment: Some(CommentSource::Static(
                "# Maximum number of lines retained in the embedded agent terminal scrollback.",
            )),
            value_fn: |c| FieldValue::Usize(c.ui.agent_scrollback_lines),
        },
        ConfigEntry::Blank,
        ConfigEntry::Section("editor"),
        ConfigEntry::Field {
            key: "default",
            comment: Some(CommentSource::Static(
                "# Preferred editor when opening a selected agent worktree.\n# Supported values are matched against popular editor CLIs on PATH\n# (for example: cursor, vscode/code, zed, antigravity).",
            )),
            value_fn: |c| FieldValue::Str(c.editor.default.clone()),
        },
        ConfigEntry::Blank,
        ConfigEntry::Keys,
        ConfigEntry::Blank,
        ConfigEntry::Projects,
    ]
}

fn render_config(config: &Config, bindings: &crate::keybindings::RuntimeBindings) -> String {
    let generate_commit_key = bindings.label_for(crate::keybindings::Action::GenerateCommitMessage);
    let mut out = String::new();
    for entry in config_schema(&generate_commit_key) {
        match entry {
            ConfigEntry::Comment(text) => {
                out.push_str(text);
                out.push('\n');
            }
            ConfigEntry::Blank => out.push('\n'),
            ConfigEntry::Section(name) => {
                let _ = writeln!(out, "[{name}]");
            }
            ConfigEntry::Field {
                key,
                comment,
                value_fn,
            } => {
                if let Some(c) = comment {
                    match c {
                        CommentSource::Static(s) => out.push_str(s),
                        CommentSource::Dynamic(s) => out.push_str(&s),
                    }
                    out.push('\n');
                }
                match value_fn(config) {
                    FieldValue::Str(s) => {
                        let _ = writeln!(out, "{key} = \"{}\"", escape_toml_string(&s));
                    }
                    FieldValue::OptStr(Some(s)) => {
                        let _ = writeln!(out, "{key} = \"{}\"", escape_toml_string(&s));
                    }
                    FieldValue::OptStr(None) => {
                        let _ = writeln!(out, "{key} = \"\"");
                    }
                    FieldValue::U16(n) => {
                        let _ = writeln!(out, "{key} = {n}");
                    }
                    FieldValue::Usize(n) => {
                        let _ = writeln!(out, "{key} = {n}");
                    }
                    FieldValue::MultilineStr(Some(s)) => {
                        let escaped = escape_toml_multiline(&s);
                        let _ = writeln!(out, "{key} = \"\"\"\n{escaped}\"\"\"");
                    }
                    FieldValue::MultilineStr(None) => {
                        let _ = writeln!(out, "{key} = \"\"");
                    }
                }
            }
            ConfigEntry::Providers => render_provider_configs(&mut out, &config.providers),
            ConfigEntry::Terminal => render_terminal_config(&mut out, &config.terminal),
            ConfigEntry::Projects => render_projects(&mut out, &config.projects),
            ConfigEntry::Keys => render_keys_config(&mut out, &config.keys, bindings),
        }
    }
    out
}

pub fn render_default_config() -> String {
    let bindings = crate::keybindings::RuntimeBindings::from_keys_config(&KeysConfig::default());
    render_config(&Config::default(), &bindings)
}

pub fn save_config(
    config_path: &Path,
    config: &Config,
    bindings: &crate::keybindings::RuntimeBindings,
) -> Result<()> {
    let body = render_config(config, bindings);
    fs::write(config_path, body)
        .with_context(|| format!("failed to write {}", config_path.display()))?;
    Ok(())
}

fn render_keys_config(
    out: &mut String,
    keys: &KeysConfig,
    bindings: &crate::keybindings::RuntimeBindings,
) {
    out.push_str("[keys]\n");
    out.push_str("# Keybindings configuration. Each action maps to one or more key combos.\n");
    out.push_str(
        "# Key format: single chars (\"j\"), special names (\"up\", \"enter\", \"space\",\n",
    );
    out.push_str(
        "# \"tab\", \"shift-tab\", \"pageup\", \"esc\"), or modifier combos (\"ctrl-d\").\n",
    );
    out.push_str("#\n");
    out.push_str("# Some keys shown in hints are terminal conventions (e.g. ctrl-j for newline)\n");
    out.push_str("# that dux documents but does not control. Set this to false to hide them.\n");
    let _ = writeln!(out, "show_terminal_keys = {}", keys.show_terminal_keys);
    out.push('\n');

    let mut last_section: Option<&str> = None;
    for def in keybindings::BINDING_DEFS {
        if def.default_keys.is_empty() {
            continue;
        }
        let config_name = def.action.config_name();

        // Section header based on help section.
        let section = def.action.help_section().unwrap_or("Other");
        if last_section != Some(section) {
            if last_section.is_some() {
                out.push('\n');
            }
            let _ = writeln!(out, "# -- {section} --");
            last_section = Some(section);
        }

        // Description comment — dynamic override for actions that reference other keys.
        let desc = if def.action == keybindings::Action::ToggleResizeMode {
            format!(
                "Enter resize mode ({} to resize side panes).",
                bindings.combined_label(
                    keybindings::Action::ResizeGrow,
                    keybindings::Action::ResizeShrink,
                ),
            )
        } else {
            def.action.config_description().to_string()
        };
        let _ = writeln!(out, "# {desc}");

        // Value from config (or defaults if missing).
        let key_strs = keys.bindings.get(config_name).cloned().unwrap_or_else(|| {
            def.default_keys
                .iter()
                .map(|k| keybindings::format_key_for_config(*k))
                .collect()
        });
        let _ = writeln!(out, "{config_name} = {}", render_string_list(&key_strs));
    }
    out.push('\n');
}

fn render_projects(out: &mut String, projects: &[ProjectConfig]) {
    out.push_str(
        "# Projects are registered here by the UI. The folder name is used when name is omitted.\n",
    );
    out.push_str("# default_provider can override the global default for one project.\n");
    if projects.is_empty() {
        out.push_str("# [[projects]]\n");
        out.push_str("# path = \"/path/to/your/repo\"\n");
    } else {
        for project in projects {
            out.push_str("[[projects]]\n");
            let _ = writeln!(out, "id = \"{}\"", escape_toml_string(&project.id));
            let _ = writeln!(out, "path = \"{}\"", escape_toml_string(&project.path));
            if let Some(name) = &project.name {
                let _ = writeln!(out, "name = \"{}\"", escape_toml_string(name));
            }
            if let Some(provider) = &project.default_provider {
                let _ = writeln!(
                    out,
                    "default_provider = \"{}\"",
                    escape_toml_string(provider)
                );
            }
            if let Some(prompt) = &project.commit_prompt {
                let escaped = escape_toml_multiline(prompt);
                let _ = writeln!(out, "commit_prompt = \"\"\"\n{escaped}\"\"\"");
            }
            out.push('\n');
        }
    }
}

/// Escape triple-quotes in a TOML multiline basic string.
/// Per the TOML spec, `"""` inside `"""..."""` can be included by escaping
/// at least one quote: `""\"`.
fn escape_toml_multiline(value: &str) -> String {
    value.replace("\"\"\"", "\"\"\\\"")
}

fn escape_toml_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                out.push_str(&format!("\\u{:04X}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

fn render_string_list(values: &[String]) -> String {
    let rendered = values
        .iter()
        .map(|value| format!("\"{}\"", escape_toml_string(value)))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{rendered}]")
}

fn default_terminal_command() -> String {
    env::var("SHELL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "/bin/sh".to_string())
}

fn default_terminal_args() -> Vec<String> {
    // Launch as a login shell so the user's profile, aliases, and prompt
    // are loaded. The -l flag is supported by bash, zsh, fish, dash, and
    // all POSIX shells.
    vec!["-l".to_string()]
}

fn default_provider_commands() -> [(&'static str, ProviderCommandConfig); 2] {
    [
        (
            "claude",
            ProviderCommandConfig {
                command: "claude".to_string(),
                args: Vec::new(),
                resume_args: Some(vec!["--continue".to_string()]),
                oneshot_args: vec!["-p".to_string(), "{prompt}".to_string()],
                oneshot_output: OneshotOutput::Stdout,
                install_hint: Some("npm install -g @anthropic-ai/claude-code".to_string()),
            },
        ),
        (
            "codex",
            ProviderCommandConfig {
                command: "codex".to_string(),
                args: Vec::new(),
                resume_args: Some(vec!["resume".to_string(), "--last".to_string()]),
                oneshot_args: vec![
                    "exec".to_string(),
                    "-o".to_string(),
                    "{tempfile}".to_string(),
                    "{prompt}".to_string(),
                ],
                oneshot_output: OneshotOutput::Tempfile,
                install_hint: Some("npm install -g @openai/codex".to_string()),
            },
        ),
    ]
}

fn render_provider_configs(out: &mut String, providers: &ProvidersConfig) {
    for (name, config) in &providers.commands {
        render_provider_config(out, name, config);
    }
}

fn render_terminal_config(out: &mut String, terminal: &TerminalConfig) {
    out.push_str("[terminal]\n");
    out.push_str(
        "# CLI command dux should use for the companion terminal bound to an agent session.\n",
    );
    out.push_str(&format!(
        "command = \"{}\"\n",
        escape_toml_string(&terminal.command)
    ));
    out.push_str(
        "# Arguments for the companion terminal command. The default [\"-l\"] launches a login\n# shell so your profile, aliases, and prompt are loaded.\n",
    );
    out.push_str(&format!(
        "args = {}\n\n",
        render_string_list(&terminal.args)
    ));
}

fn render_provider_config(out: &mut String, name: &str, config: &ProviderCommandConfig) {
    out.push_str(&format!("[providers.{name}]\n"));
    out.push_str(&format!("# CLI command for {name} sessions.\n"));
    out.push_str(&format!(
        "command = \"{}\"\n",
        escape_toml_string(&config.command)
    ));
    out.push_str(&format!("args = {}\n", render_string_list(&config.args)));
    out.push_str(
        "# Optional args dux should use when reconnecting a detached session.\n\
         # Leave this empty for CLIs that do not support cwd/repo-scoped session resume.\n",
    );
    out.push_str(&format!(
        "resume_args = {}\n",
        render_string_list(config.resume_args.as_deref().unwrap_or(&[]))
    ));
    out.push_str("# Oneshot args for non-interactive use (e.g. AI commit messages).\n");
    out.push_str("# Placeholders: {prompt} = the prompt text, {tempfile} = temp file path.\n");
    out.push_str(&format!(
        "oneshot_args = {}\n",
        render_string_list(&config.oneshot_args)
    ));
    let output_str = match config.oneshot_output {
        OneshotOutput::Stdout => "stdout",
        OneshotOutput::Tempfile => "tempfile",
    };
    out.push_str(&format!("oneshot_output = \"{output_str}\"\n"));
    if let Some(hint) = &config.install_hint {
        out.push_str(&format!(
            "install_hint = \"{}\"\n",
            escape_toml_string(hint)
        ));
    }
    out.push('\n');
}

/// Validate all key bindings in the config. Returns a descriptive error on failure.
///
/// Checks:
/// 1. Every action name is known (present in `BINDING_DEFS`).
/// 2. Every key string parses successfully after normalization
///    (bare uppercase letters like `"P"` are rewritten to `"shift-p"`).
/// 3. No two actions bind the same normalized key in overlapping scopes.
pub fn validate_keys(keys: &KeysConfig) -> Result<(), String> {
    for (name, key_strs) in &keys.bindings {
        let valid = keybindings::BINDING_DEFS
            .iter()
            .any(|d| d.action.config_name() == name);
        if !valid {
            return Err(format!("[keys] unknown action: \"{name}\""));
        }
        for s in key_strs {
            let normalized = keybindings::normalize_key_string(s);
            crokey::parse(&normalized)
                .map_err(|_| format!("[keys] invalid key \"{s}\" for action \"{name}\""))?;
        }
    }

    // Detect conflicting bindings (same key in overlapping scopes).
    let conflicts = keybindings::detect_conflicts(keys);
    if !conflicts.is_empty() {
        let mut msg = String::from("[keys] conflicting keybindings detected:");
        for c in &conflicts {
            msg.push_str(&format!(
                "\n  - \"{}\" is bound to both \"{}\" and \"{}\" in {}",
                c.key_label,
                c.action_a,
                c.action_b,
                c.scope.display_name(),
            ));
        }
        msg.push_str(
            "\nCheck your [keys.bindings] configuration and ensure each key is unique within its scope.",
        );
        return Err(msg);
    }

    Ok(())
}

/// Check whether a provider command is available on PATH.
/// Returns `Ok(())` if found, or `Err(message)` with a user-friendly install hint.
pub fn check_provider_available(config: &ProviderCommandConfig) -> std::result::Result<(), String> {
    use std::process::Command as StdCommand;
    match StdCommand::new("which").arg(&config.command).output() {
        Ok(output) if output.status.success() => Ok(()),
        _ => {
            let hint = config
                .install_hint
                .as_ref()
                .map(|h| format!("Install with: {h}"))
                .unwrap_or_else(|| {
                    format!(
                        "Make sure '{}' is installed and on your PATH.",
                        config.command
                    )
                });
            Err(format!(
                "CLI tool '{}' not found on PATH. {hint}",
                config.command
            ))
        }
    }
}

fn discover_root(home: &Path, xdg_config_home: Option<std::ffi::OsString>) -> PathBuf {
    #[cfg(target_os = "macos")]
    {
        let _ = xdg_config_home;
        home.join(".dux")
    }

    #[cfg(not(target_os = "macos"))]
    {
        if let Some(xdg) = xdg_config_home.map(PathBuf::from)
            && xdg.is_absolute()
        {
            return xdg.join("dux");
        }
        home.join(".config").join("dux")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Render config using default keybinding labels (for tests that don't need custom bindings).
    fn render_config_default(config: &Config) -> String {
        let bindings =
            crate::keybindings::RuntimeBindings::from_keys_config(&KeysConfig::default());
        render_config(config, &bindings)
    }

    #[test]
    fn default_config_is_commented_and_complete() {
        let rendered = render_default_config();
        assert!(rendered.contains("# dux configuration"));
        assert!(rendered.contains("[defaults]"));
        assert!(rendered.contains("provider = \"claude\""));
        assert!(rendered.contains("[providers.claude]"));
        assert!(rendered.contains("[providers.codex]"));
        assert!(rendered.contains("oneshot_args = "));
        assert!(rendered.contains("oneshot_output = "));
        assert!(rendered.contains("resume_args = "));
        assert!(rendered.contains("[terminal]"));
        assert!(rendered.contains("command = "));
        assert!(rendered.contains("args = []"));
        assert!(rendered.contains("[ui]"));
        assert!(rendered.contains("agent_scrollback_lines = 10000"));
        assert!(rendered.contains("staged_pane_height_pct = "));
        assert!(rendered.contains("[editor]"));
        assert!(rendered.contains("default = \"cursor\""));
        assert!(rendered.contains("[keys]"));
        assert!(rendered.contains("show_terminal_keys = true"));
        assert!(rendered.contains("move_down = "));
        assert!(rendered.contains("quit = "));
        assert!(rendered.contains("commit_prompt = \"\"\""));
        assert!(rendered.contains("Conventional Commits"));
    }

    #[test]
    fn validate_keys_accepts_valid_config() {
        let keys = KeysConfig::default();
        assert!(validate_keys(&keys).is_ok());
    }

    #[test]
    fn validate_keys_rejects_bad_key() {
        let mut keys = KeysConfig::default();
        keys.bindings
            .insert("quit".to_string(), vec!["badkey!!!".to_string()]);
        let result = validate_keys(&keys);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("badkey!!!"));
    }

    #[test]
    fn validate_keys_rejects_unknown_action() {
        let mut keys = KeysConfig::default();
        keys.bindings
            .insert("nonexistent_action".to_string(), vec!["q".to_string()]);
        let result = validate_keys(&keys);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("nonexistent_action"));
    }

    #[test]
    fn render_config_escapes_special_chars() {
        let mut config = Config::default();
        config.projects.push(ProjectConfig {
            id: new_project_id(),
            path: r#"/home/user/"test"\project"#.to_string(),
            name: Some(r#"te"st"#.to_string()),
            default_provider: None,
            commit_prompt: None,
        });
        let rendered = render_config_default(&config);
        let parsed: Config = toml::from_str(&rendered).expect("should parse back");
        assert_eq!(parsed.projects[0].path, config.projects[0].path);
        assert_eq!(parsed.projects[0].name, config.projects[0].name);
    }

    #[test]
    fn render_config_escapes_newlines_and_control_chars() {
        let mut config = Config::default();
        config.projects.push(ProjectConfig {
            id: new_project_id(),
            path: "/home/user/path\nwith\nnewlines".to_string(),
            name: Some("name\twith\ttabs".to_string()),
            default_provider: None,
            commit_prompt: None,
        });
        let rendered = render_config_default(&config);
        let parsed: Config = toml::from_str(&rendered).expect("should parse back");
        assert_eq!(parsed.projects[0].path, config.projects[0].path);
        assert_eq!(parsed.projects[0].name, config.projects[0].name);
    }

    #[test]
    fn default_config_round_trips_through_toml() {
        let rendered = render_default_config();
        let parsed: Config = toml::from_str(&rendered).expect("default config should parse");
        let re_rendered = render_config_default(&parsed);
        assert_eq!(
            rendered, re_rendered,
            "render → parse → render should be stable"
        );
    }

    #[test]
    fn default_config_round_trips_agent_scrollback_lines() {
        let mut config = Config::default();
        config.ui.agent_scrollback_lines = 12_345;
        let rendered = render_config_default(&config);
        let parsed: Config = toml::from_str(&rendered).expect("config should parse");
        assert_eq!(parsed.ui.agent_scrollback_lines, 12_345);
    }

    #[test]
    fn default_config_round_trips_default_editor() {
        let mut config = Config::default();
        config.editor.default = "zed".to_string();
        let rendered = render_config_default(&config);
        let parsed: Config = toml::from_str(&rendered).expect("config should parse");
        assert_eq!(parsed.editor.default, "zed");
    }

    #[test]
    fn default_config_round_trips_terminal_command() {
        let mut config = Config::default();
        config.terminal.command = "fish".to_string();
        config.terminal.args = vec!["-l".to_string()];
        let rendered = render_config_default(&config);
        let parsed: Config = toml::from_str(&rendered).expect("config should parse");
        assert_eq!(parsed.terminal.command, "fish");
        assert_eq!(parsed.terminal.args, vec!["-l"]);
    }

    #[test]
    fn default_config_round_trips_staged_pane_height() {
        let mut config = Config::default();
        config.ui.staged_pane_height_pct = 65;
        let rendered = render_config_default(&config);
        let parsed: Config = toml::from_str(&rendered).expect("config should parse");
        assert_eq!(parsed.ui.staged_pane_height_pct, 65);
    }

    #[test]
    fn old_config_missing_staged_pane_height_defaults_to_50() {
        let toml_str = r#"
[ui]
left_width_pct = 20
right_width_pct = 23
terminal_pane_height_pct = 35
agent_scrollback_lines = 10000
"#;
        let parsed: Config = toml::from_str(toml_str).expect("should parse");
        assert_eq!(parsed.ui.staged_pane_height_pct, 50);
    }

    #[test]
    fn built_in_providers_ship_resume_args() {
        let config = Config::default();
        assert_eq!(config.defaults.provider, "claude");
        let claude = config
            .providers
            .get("claude")
            .expect("claude provider should exist");
        assert_eq!(
            claude.resume_args.clone(),
            Some(vec!["--continue".to_string()])
        );
        assert!(claude.supports_session_resume());

        let codex = config
            .providers
            .get("codex")
            .expect("codex provider should exist");
        assert_eq!(
            codex.resume_args.clone(),
            Some(vec!["resume".to_string(), "--last".to_string()])
        );
        assert!(codex.supports_session_resume());
    }

    #[test]
    fn provider_command_config_selects_resume_args_only_when_available() {
        let cfg = ProviderCommandConfig {
            command: "example".to_string(),
            args: vec!["--interactive".to_string()],
            resume_args: Some(vec!["--resume".to_string(), "--last".to_string()]),
            oneshot_args: Vec::new(),
            oneshot_output: OneshotOutput::Stdout,
            install_hint: None,
        };
        assert_eq!(cfg.interactive_args(false), ["--interactive"]);
        assert_eq!(cfg.interactive_args(true), ["--resume", "--last"]);

        let unsupported = ProviderCommandConfig {
            command: "example".to_string(),
            args: vec!["--interactive".to_string()],
            resume_args: None,
            oneshot_args: Vec::new(),
            oneshot_output: OneshotOutput::Stdout,
            install_hint: None,
        };
        assert_eq!(unsupported.interactive_args(true), ["--interactive"]);
        assert!(!unsupported.supports_session_resume());
    }

    #[test]
    fn ensure_defaults_backfills_missing_resume_args_for_builtins() {
        let mut providers = ProvidersConfig {
            commands: IndexMap::from([(
                "claude".to_string(),
                ProviderCommandConfig {
                    command: "claude".to_string(),
                    args: Vec::new(),
                    resume_args: None,
                    oneshot_args: Vec::new(),
                    oneshot_output: OneshotOutput::Stdout,
                    install_hint: None,
                },
            )]),
        };

        providers.ensure_defaults();

        let claude = providers
            .get("claude")
            .expect("claude provider should still exist");
        assert_eq!(
            claude.resume_args.clone(),
            Some(vec!["--continue".to_string()])
        );
    }

    #[test]
    fn ensure_defaults_preserves_explicit_resume_disable() {
        let mut providers = ProvidersConfig {
            commands: IndexMap::from([(
                "claude".to_string(),
                ProviderCommandConfig {
                    command: "claude".to_string(),
                    args: Vec::new(),
                    resume_args: Some(Vec::new()),
                    oneshot_args: Vec::new(),
                    oneshot_output: OneshotOutput::Stdout,
                    install_hint: None,
                },
            )]),
        };

        providers.ensure_defaults();

        let claude = providers
            .get("claude")
            .expect("claude provider should still exist");
        assert_eq!(claude.resume_args, Some(Vec::new()));
        assert!(!claude.supports_session_resume());
    }

    #[test]
    fn provider_configs_without_resume_args_still_parse() {
        let parsed: Config = toml::from_str(
            r#"
            [defaults]
            provider = "claude"

            [logging]
            level = "info"
            path = "dux.log"

            [ui]
            left_width_pct = 20
            right_width_pct = 23
            agent_scrollback_lines = 10000

            [editor]
            default = "cursor"

            [keys]
            show_terminal_keys = true

            [providers.custom]
            command = "custom-agent"
            args = ["chat"]
            oneshot_args = ["ask", "{prompt}"]
            oneshot_output = "stdout"
            "#,
        )
        .expect("legacy provider config should parse");

        let provider = parsed
            .providers
            .get("custom")
            .expect("custom provider should exist");
        assert_eq!(provider.resume_args, None);
        assert_eq!(provider.interactive_args(true), ["chat"]);
    }

    #[test]
    fn legacy_provider_config_without_resume_args_still_parses() {
        let parsed: ProviderCommandConfig = toml::from_str(
            r#"
command = "legacy-agent"
args = ["serve"]
oneshot_args = ["--prompt", "{prompt}"]
oneshot_output = "stdout"
"#,
        )
        .expect("legacy provider config should parse");
        assert_eq!(parsed.command, "legacy-agent");
        assert_eq!(parsed.args, vec!["serve"]);
        assert_eq!(parsed.resume_args, None);
        assert!(!parsed.supports_session_resume());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn config_root_uses_hidden_home_dir_on_macos() {
        let root = discover_root(Path::new("/example/home"), Some("/tmp/ignored".into()));
        assert_eq!(root, PathBuf::from("/example/home/.dux"));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn config_root_uses_xdg_config_home_when_absolute() {
        let root = discover_root(Path::new("/example/home"), Some("/tmp/xdg".into()));
        assert_eq!(root, PathBuf::from("/tmp/xdg/dux"));
    }

    #[test]
    fn default_config_keys_valid_after_round_trip() {
        let rendered = render_default_config();
        let parsed: Config = toml::from_str(&rendered).expect("default config should parse");
        validate_keys(&parsed.keys).expect("round-tripped keys should be valid");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn config_root_falls_back_to_dot_config_when_xdg_missing() {
        let root = discover_root(Path::new("/example/home"), None);
        assert_eq!(root, PathBuf::from("/example/home/.config/dux"));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn config_root_ignores_relative_xdg_config_home() {
        let root = discover_root(Path::new("/example/home"), Some("relative/path".into()));
        assert_eq!(root, PathBuf::from("/example/home/.config/dux"));
    }

    #[test]
    fn commit_prompt_resolution_uses_project_override() {
        let mut config = Config::default();
        config.projects.push(ProjectConfig {
            id: new_project_id(),
            path: "/my/project".to_string(),
            name: Some("test".to_string()),
            default_provider: None,
            commit_prompt: Some("custom project prompt".to_string()),
        });

        // Project override takes precedence.
        assert_eq!(
            config.commit_prompt_for_project("/my/project"),
            "custom project prompt"
        );

        // Unknown project falls back to system default.
        assert_eq!(
            config.commit_prompt_for_project("/other/project"),
            DEFAULT_COMMIT_PROMPT
        );

        // Empty project prompt falls back to system default.
        config.projects[0].commit_prompt = Some(String::new());
        assert_eq!(
            config.commit_prompt_for_project("/my/project"),
            DEFAULT_COMMIT_PROMPT
        );
    }

    #[test]
    fn commit_prompt_resolution_uses_system_default() {
        let mut config = Config::default();
        config.defaults.commit_prompt = Some("custom system prompt".to_string());
        assert_eq!(
            config.commit_prompt_for_project("/any/project"),
            "custom system prompt"
        );

        // Empty system prompt falls back to hardcoded constant.
        config.defaults.commit_prompt = Some(String::new());
        assert_eq!(
            config.commit_prompt_for_project("/any/project"),
            DEFAULT_COMMIT_PROMPT
        );

        // None falls back to hardcoded constant.
        config.defaults.commit_prompt = None;
        assert_eq!(
            config.commit_prompt_for_project("/any/project"),
            DEFAULT_COMMIT_PROMPT
        );
    }

    #[test]
    fn multiline_string_with_triple_quotes_roundtrips() {
        let mut config = Config::default();
        config.defaults.commit_prompt =
            Some("Use triple \"\"\" quotes in your prompt.".to_string());
        let rendered = render_config_default(&config);
        let parsed: Config = toml::from_str(&rendered).expect("should parse back");
        assert_eq!(
            parsed.defaults.commit_prompt.as_deref(),
            Some("Use triple \"\"\" quotes in your prompt."),
        );
    }

    #[test]
    fn config_comment_uses_dynamic_keybinding_label() {
        let rendered = render_default_config();
        // The default binding for GenerateCommitMessage is Ctrl-g.
        assert!(
            rendered.contains("(Ctrl-g)"),
            "config comment should include the dynamic keybinding label"
        );
        assert!(
            !rendered.contains("(Ctrl+G)"),
            "config comment should NOT contain hardcoded Ctrl+G"
        );
    }

    #[test]
    fn validate_keys_normalizes_bare_uppercase() {
        let mut keys = KeysConfig::default();
        keys.bindings
            .insert("quit".to_string(), vec!["P".to_string()]);
        // Should succeed — "P" is normalized to "shift-p" before parsing.
        assert!(
            validate_keys(&keys).is_ok(),
            "bare uppercase 'P' should be normalized to 'shift-p' and accepted"
        );
    }

    #[test]
    fn validate_keys_detects_conflict() {
        let mut keys = KeysConfig::default();
        // Bind the same key to two actions that share the Left scope.
        keys.bindings
            .insert("toggle_project".to_string(), vec!["x".to_string()]);
        keys.bindings
            .insert("new_agent".to_string(), vec!["x".to_string()]);
        let result = validate_keys(&keys);
        assert!(result.is_err(), "duplicate key in same scope should error");
        let msg = result.unwrap_err();
        assert!(
            msg.contains("conflicting"),
            "error should mention conflict: {msg}"
        );
        assert!(
            msg.contains("toggle_project"),
            "error should name first action: {msg}"
        );
        assert!(
            msg.contains("new_agent"),
            "error should name second action: {msg}"
        );
    }
}
