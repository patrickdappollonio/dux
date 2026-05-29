//! dux configuration data model: the serde structs persisted in `config.toml`
//! and their defaults, plus `DuxPaths` and path resolution (`resolve_root`,
//! `discover_root`) and the env-expansion helpers (`expand_env_vars`,
//! `expand_path`, `resolve_project_env`, …). The keybinding-aware *renderer* of
//! the documented config, plus `ensure_config`/`save_config` orchestration and
//! the toml_edit patching, live in the binary's `config` module — not here.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow, bail};
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const DEFAULT_COMMIT_PROMPT: &str = "\
Write a commit message for the following staged diff.

Rules:
- Subject line: use Conventional Commits (feat:, fix:, refactor:, docs:, test:, chore:, style:, perf:, ci:, build:). Imperative mood, max 72 chars, no period at the end.
- Trivial changes (typo, rename, one-liner): ONLY the subject line, nothing else.
- Small changes (2-3 files, single logical concern): subject line, blank line, then a short paragraph (2-3 sentences max) explaining the motivation and impact. Do NOT use bullet points for this case.
- Larger changes (4+ files or multiple distinct logical concerns): subject line, blank line, then concise bullet points (one per logical change, each under 80 chars). Use \"- \" for bullets.
- This is a plain text commit message, not markdown. NEVER use backticks, asterisks, code fences, or any markdown syntax. Refer to functions and files by name without formatting.
- Focus on intent and impact, not mechanical description of lines added/removed.
- Output ONLY the raw commit message. No preamble, no quotes, no explanation.";

/// Which surface(s) a macro is available on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum MacroSurface {
    #[default]
    Agent,
    Terminal,
    Both,
}

impl MacroSurface {
    /// Human-readable label for UI display.
    pub fn label(self) -> &'static str {
        match self {
            Self::Agent => "agent only",
            Self::Terminal => "terminal only",
            Self::Both => "agent + terminal",
        }
    }

    /// Cycle to the next variant: Agent -> Terminal -> Both -> Agent.
    pub fn next(self) -> Self {
        match self {
            Self::Agent => Self::Terminal,
            Self::Terminal => Self::Both,
            Self::Both => Self::Agent,
        }
    }

    /// Cycle to the previous variant: Agent -> Both -> Terminal -> Agent.
    pub fn prev(self) -> Self {
        match self {
            Self::Agent => Self::Both,
            Self::Both => Self::Terminal,
            Self::Terminal => Self::Agent,
        }
    }

    /// Whether this surface matches the given session surface.
    pub fn matches(self, session: crate::model::SessionSurface) -> bool {
        match self {
            Self::Both => true,
            Self::Agent => session == crate::model::SessionSurface::Agent,
            Self::Terminal => session == crate::model::SessionSurface::Terminal,
        }
    }
}

/// A single text macro entry with surface restriction.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MacroEntry {
    pub text: String,
    pub surface: MacroSurface,
}

/// Text macros: a map from name to entry.
/// Each entry is triggered from the macro bar (Ctrl+\).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MacrosConfig {
    #[serde(flatten)]
    pub entries: IndexMap<String, MacroEntry>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Defaults {
    pub provider: String,
    pub start_directory: Option<String>,
    pub commit_prompt: Option<String>,
    pub enable_randomized_pet_name_by_default: bool,
    #[serde(default = "default_true")]
    pub pull_before_creating_agent_by_default: bool,
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
pub struct StartupCommandTerminalConfig {
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
    pub resume_wait_timeout_ms: Option<u64>,
    pub oneshot_args: Vec<String>,
    pub oneshot_output: OneshotOutput,
    pub install_hint: Option<String>,
    pub forward_scroll: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectConfig {
    #[serde(default = "new_project_id")]
    pub id: String,
    pub path: String,
    pub name: Option<String>,
    pub default_provider: Option<String>,
    pub leading_branch: Option<String>,
    pub auto_reopen_agents: Option<bool>,
    pub startup_command: Option<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

pub fn new_project_id() -> String {
    Uuid::new_v4().to_string()
}

pub fn default_true() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub left_width_pct: u16,
    pub right_width_pct: u16,
    pub terminal_pane_height_pct: u16,
    pub empty_project_separator_min_projects: u16,
    pub staged_pane_height_pct: u16,
    pub commit_pane_height_pct: u16,
    pub agent_scrollback_lines: usize,
    pub branch_sync_interval: u16,
    pub show_diff_line_numbers: bool,
    pub diff_tab_width: u16,
    pub github_integration: bool,
    pub auto_reopen_agents: bool,
    pub pr_banner_position: String,
    pub theme: String,
}

impl Default for Defaults {
    fn default() -> Self {
        let start_directory = home::home_dir().map(|p| p.to_string_lossy().to_string());
        Self {
            provider: "claude".to_string(),
            start_directory,
            commit_prompt: Some(DEFAULT_COMMIT_PROMPT.to_string()),
            enable_randomized_pet_name_by_default: false,
            pull_before_creating_agent_by_default: true,
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
            resume_wait_timeout_ms: None,
            oneshot_args: Vec::new(),
            oneshot_output: OneshotOutput::Stdout,
            install_hint: None,
            forward_scroll: false,
        }
    }
}

impl ProviderCommandConfig {
    pub fn interactive_args(&self, resume_session: bool) -> Vec<String> {
        let mut args = self.args.clone();
        if resume_session
            && let Some(resume_args) = self.resume_args.as_deref().filter(|args| !args.is_empty())
        {
            args.extend(resume_args.iter().cloned());
        }
        args
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

impl Default for StartupCommandTerminalConfig {
    fn default() -> Self {
        Self {
            command: "$SHELL".to_string(),
            args: vec!["-l".to_string(), "-c".to_string()],
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
            empty_project_separator_min_projects: 5,
            staged_pane_height_pct: 50,
            commit_pane_height_pct: 40,
            agent_scrollback_lines: 10_000,
            branch_sync_interval: 30,
            show_diff_line_numbers: false,
            diff_tab_width: 4,
            github_integration: true,
            auto_reopen_agents: false,
            pr_banner_position: "bottom".to_string(),
            theme: crate::theme::DEFAULT_THEME_NAME.to_string(),
        }
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
                    if entry.get().resume_wait_timeout_ms.is_none() {
                        entry.get_mut().resume_wait_timeout_ms = config.resume_wait_timeout_ms;
                    }
                }
            }
        }
    }
}

pub fn default_terminal_command() -> String {
    env::var("SHELL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "/bin/sh".to_string())
}

pub fn default_terminal_args() -> Vec<String> {
    // Launch as a login shell so the user's profile, aliases, and prompt
    // are loaded. The -l flag is supported by bash, zsh, fish, dash, and
    // all POSIX shells.
    vec!["-l".to_string()]
}

pub fn default_provider_commands() -> [(&'static str, ProviderCommandConfig); 5] {
    [
        (
            "claude",
            ProviderCommandConfig {
                command: "claude".to_string(),
                args: Vec::new(),
                resume_args: Some(vec!["--continue".to_string()]),
                resume_wait_timeout_ms: None,
                oneshot_args: vec![
                    "--bare".to_string(),
                    "-p".to_string(),
                    "{prompt}".to_string(),
                    "--tools".to_string(),
                    String::new(),
                    "--max-turns".to_string(),
                    "1".to_string(),
                ],
                oneshot_output: OneshotOutput::Stdout,
                install_hint: Some("curl -fsSL https://claude.ai/install.sh | bash".to_string()),
                forward_scroll: false,
            },
        ),
        (
            "codex",
            ProviderCommandConfig {
                command: "codex".to_string(),
                args: Vec::new(),
                resume_args: Some(vec!["resume".to_string(), "--last".to_string()]),
                resume_wait_timeout_ms: None,
                oneshot_args: vec![
                    "exec".to_string(),
                    "--ephemeral".to_string(),
                    "--full-auto".to_string(),
                    "--color".to_string(),
                    "never".to_string(),
                    "-o".to_string(),
                    "{tempfile}".to_string(),
                    "{prompt}".to_string(),
                ],
                oneshot_output: OneshotOutput::Tempfile,
                install_hint: Some("brew install --cask codex".to_string()),
                forward_scroll: false,
            },
        ),
        (
            "gemini",
            ProviderCommandConfig {
                command: "gemini".to_string(),
                args: Vec::new(),
                resume_args: Some(vec!["--resume".to_string()]),
                resume_wait_timeout_ms: None,
                oneshot_args: vec!["-p".to_string(), "{prompt}".to_string()],
                oneshot_output: OneshotOutput::Stdout,
                install_hint: Some("brew install gemini-cli".to_string()),
                forward_scroll: false,
            },
        ),
        (
            "opencode",
            ProviderCommandConfig {
                command: "opencode".to_string(),
                args: Vec::new(),
                resume_args: Some(vec!["--continue".to_string()]),
                resume_wait_timeout_ms: Some(3_000),
                oneshot_args: vec!["run".to_string(), "{prompt}".to_string()],
                oneshot_output: OneshotOutput::Stdout,
                install_hint: Some("curl -fsSL https://opencode.ai/install | bash".to_string()),
                forward_scroll: true,
            },
        ),
        (
            "copilot",
            ProviderCommandConfig {
                command: "copilot".to_string(),
                args: Vec::new(),
                // Copilot's --continue resumes the most recent session
                // globally, not scoped to the current working directory.
                // Unlike claude/codex/gemini/opencode, there is no flag
                // to limit resume to the CWD, so we disable it.
                resume_args: None,
                resume_wait_timeout_ms: None,
                oneshot_args: vec![
                    "-p".to_string(),
                    "{prompt}".to_string(),
                    "--allow-all-tools".to_string(),
                ],
                oneshot_output: OneshotOutput::Stdout,
                install_hint: Some("curl -fsSL https://gh.io/copilot-install | bash".to_string()),
                forward_scroll: false,
            },
        ),
    ]
}

// ---------------------------------------------------------------------------
// DuxPaths: canonical locations for runtime files
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct DuxPaths {
    pub root: PathBuf,
    pub config_path: PathBuf,
    pub sessions_db_path: PathBuf,
    pub worktrees_root: PathBuf,
    /// Path to the lockfile that enforces a single dux instance per
    /// config directory. Contains the PID of the holder.
    pub lock_path: PathBuf,
}

impl DuxPaths {
    pub fn discover() -> Result<Self> {
        let root = resolve_root(
            env::var_os("DUX_HOME"),
            home::home_dir(),
            env::var_os("XDG_CONFIG_HOME"),
        )?;
        Ok(Self {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
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

pub fn resolve_root(
    dux_home: Option<std::ffi::OsString>,
    home: Option<PathBuf>,
    xdg_config_home: Option<std::ffi::OsString>,
) -> Result<PathBuf> {
    if let Some(dux_home) = dux_home.map(PathBuf::from) {
        if dux_home.is_absolute() {
            return Ok(dux_home);
        }
        bail!(
            "DUX_HOME must be an absolute path, got: {}",
            dux_home.display()
        );
    }

    let home = home.ok_or_else(|| anyhow!("failed to determine user home directory"))?;
    Ok(discover_root(&home, xdg_config_home))
}

pub fn discover_root(home: &Path, xdg_config_home: Option<std::ffi::OsString>) -> PathBuf {
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

// ---------------------------------------------------------------------------
// Env/path helpers
// ---------------------------------------------------------------------------

/// Expand environment variables (`$VAR`, `${VAR}`) in a config string.
/// Returns `None` when variable syntax is invalid.
pub fn expand_env_vars(raw: &str) -> Option<String> {
    let mut result = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '$' {
            let braced = chars.peek() == Some(&'{');
            if braced {
                chars.next();
            }
            let mut var_name = String::new();
            while let Some(&c) = chars.peek() {
                if braced {
                    if c == '}' {
                        chars.next();
                        break;
                    }
                } else if !c.is_ascii_alphanumeric() && c != '_' {
                    break;
                }
                var_name.push(c);
                chars.next();
            }
            if var_name.is_empty() || !is_valid_var_name(&var_name) {
                return None;
            }
            match std::env::var(&var_name) {
                Ok(value) => result.push_str(&value),
                Err(_) => {
                    result.push('$');
                    if braced {
                        result.push('{');
                    }
                    result.push_str(&var_name);
                    if braced {
                        result.push('}');
                    }
                }
            }
        } else {
            result.push(ch);
        }
    }

    Some(result)
}

pub fn resolve_project_env(env: &BTreeMap<String, String>) -> Result<Vec<(String, String)>> {
    let mut resolved = Vec::with_capacity(env.len());
    for (name, value) in env {
        validate_project_env_name(name)?;
        let expanded = expand_env_vars(value)
            .ok_or_else(|| anyhow!("environment variable {name} has invalid expansion syntax"))?;
        if expanded.contains('\0') {
            bail!("environment variable {name} contains a NUL byte");
        }
        resolved.push((name.clone(), expanded));
    }
    Ok(resolved)
}

pub fn resolve_agent_env(
    global_env: &BTreeMap<String, String>,
    project_env: &BTreeMap<String, String>,
) -> Result<Vec<(String, String)>> {
    let mut merged = global_env.clone();
    merged.extend(project_env.clone());
    resolve_project_env(&merged)
}

pub fn project_env_to_lines(env: &BTreeMap<String, String>) -> String {
    env.iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn parse_project_env_lines(raw: &str) -> Result<BTreeMap<String, String>> {
    let mut env = BTreeMap::new();
    for (index, line) in raw.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let Some((name, value)) = trimmed.split_once('=') else {
            bail!("line {} must use KEY=value syntax", index + 1);
        };
        let name = name.trim();
        validate_project_env_name(name)
            .with_context(|| format!("line {} has an invalid variable name", index + 1))?;
        if value.contains('\0') {
            bail!("line {} contains a NUL byte", index + 1);
        }
        expand_env_vars(value).ok_or_else(|| {
            anyhow!(
                "line {} has invalid environment variable expansion syntax",
                index + 1
            )
        })?;
        env.insert(name.to_string(), value.to_string());
    }
    Ok(env)
}

fn validate_project_env_name(name: &str) -> Result<()> {
    if is_valid_var_name(name) {
        Ok(())
    } else {
        bail!("expected [A-Za-z_][A-Za-z0-9_]*")
    }
}

/// Expand environment variables (`$VAR`, `${VAR}`) and tilde (`~`) in a path
/// string.  Returns `None` when the result is unsafe (relative path, directory
/// traversal via `..`, or invalid variable names).
pub fn expand_path(raw: &str) -> Option<String> {
    let mut result = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();

    // Handle leading tilde.
    if chars.peek() == Some(&'~') {
        chars.next(); // consume '~'
        let home = home::home_dir()?;
        result.push_str(&home.to_string_lossy());
        // Allow `~/...` but also bare `~`.
        if chars.peek() == Some(&'/') {
            // keep the slash – the next iteration will push it
        } else if chars.peek().is_some() {
            // `~user` style – not supported, reject.
            return None;
        }
    }

    while let Some(ch) = chars.next() {
        if ch == '$' {
            // Try `${VAR}` or `$VAR`.
            let braced = chars.peek() == Some(&'{');
            if braced {
                chars.next(); // consume '{'
            }
            let mut var_name = String::new();
            while let Some(&c) = chars.peek() {
                if braced {
                    if c == '}' {
                        chars.next(); // consume '}'
                        break;
                    }
                } else if !c.is_ascii_alphanumeric() && c != '_' {
                    break;
                }
                var_name.push(c);
                chars.next();
            }
            // Validate variable name: [A-Za-z_][A-Za-z0-9_]*
            if var_name.is_empty() || !is_valid_var_name(&var_name) {
                return None;
            }
            match std::env::var(&var_name) {
                Ok(value) => result.push_str(&value),
                Err(_) => {
                    // Unresolved variable – keep the literal token so the user
                    // can see which variable failed in the warning message.
                    result.push('$');
                    if braced {
                        result.push('{');
                    }
                    result.push_str(&var_name);
                    if braced {
                        result.push('}');
                    }
                }
            }
        } else {
            result.push(ch);
        }
    }

    let path = std::path::Path::new(&result);

    // Must be absolute.
    if !path.is_absolute() {
        return None;
    }

    // Reject directory traversal (`..`).
    if path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return None;
    }

    Some(result)
}

/// Returns `true` when `name` matches `[A-Za-z_][A-Za-z0-9_]*`.
fn is_valid_var_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

// ---------------------------------------------------------------------------
// Top-level Config and KeysConfig
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub defaults: Defaults,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub providers: ProvidersConfig,
    pub terminal: TerminalConfig,
    pub startup_command_terminal: StartupCommandTerminalConfig,
    pub logging: LoggingConfig,
    pub projects: Vec<ProjectConfig>,
    pub ui: UiConfig,
    pub editor: EditorConfig,
    pub keys: KeysConfig,
    pub macros: MacrosConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct KeysConfig {
    pub show_terminal_keys: bool,
    #[serde(flatten)]
    pub bindings: BTreeMap<String, Vec<String>>,
}

impl Default for KeysConfig {
    /// Returns a `KeysConfig` with no explicit bindings.
    ///
    /// The empty `bindings` map is intentional: default key assignments are
    /// resolved at runtime by `RuntimeBindings::from_keys_config` (in the TUI
    /// crate), which falls back to `BINDING_DEFS` for any action not present
    /// here. `dux-core` cannot reference `BINDING_DEFS` (it is `crokey`/
    /// `crossterm`-coupled, binary-only), so the defaults are deliberately
    /// omitted from this impl rather than duplicated.
    fn default() -> Self {
        Self {
            show_terminal_keys: true,
            bindings: BTreeMap::new(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            defaults: Defaults::default(),
            env: BTreeMap::new(),
            providers: ProvidersConfig::default(),
            terminal: TerminalConfig::default(),
            startup_command_terminal: StartupCommandTerminalConfig::default(),
            logging: LoggingConfig {
                level: "info".to_string(),
                path: "dux.log".to_string(),
            },
            projects: Vec::new(),
            ui: UiConfig {
                left_width_pct: 20,
                right_width_pct: 23,
                terminal_pane_height_pct: 35,
                empty_project_separator_min_projects: 5,
                staged_pane_height_pct: 50,
                commit_pane_height_pct: 40,
                agent_scrollback_lines: 10_000,
                branch_sync_interval: 30,
                show_diff_line_numbers: false,
                diff_tab_width: 4,
                github_integration: true,
                auto_reopen_agents: false,
                pr_banner_position: "bottom".to_string(),
                theme: crate::theme::DEFAULT_THEME_NAME.to_string(),
            },
            editor: EditorConfig::default(),
            keys: KeysConfig::default(),
            macros: MacrosConfig::default(),
        }
    }
}

impl Config {
    pub fn default_provider(&self) -> crate::model::ProviderKind {
        crate::model::ProviderKind::from_str(&self.defaults.provider)
    }

    pub fn default_commit_prompt(&self) -> String {
        self.defaults
            .commit_prompt
            .as_ref()
            .filter(|s| !s.is_empty())
            .cloned()
            .unwrap_or_else(|| DEFAULT_COMMIT_PROMPT.to_string())
    }
}

pub fn provider_config(
    config: &Config,
    provider: &crate::model::ProviderKind,
) -> ProviderCommandConfig {
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
    use std::path::Path;

    use super::*;

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
    fn resolve_root_uses_dux_home_when_absolute() {
        let root = resolve_root(Some("/custom/dux".into()), None, None).unwrap();
        assert_eq!(root, PathBuf::from("/custom/dux"));
    }

    #[test]
    fn resolve_root_errors_on_relative_dux_home() {
        let err = resolve_root(
            Some("relative/path".into()),
            Some(PathBuf::from("/home")),
            None,
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("DUX_HOME must be an absolute path"),
            "unexpected error: {err}"
        );
        assert!(
            err.to_string().contains("relative/path"),
            "error should contain the bad path: {err}"
        );
    }

    #[test]
    fn resolve_root_errors_on_empty_dux_home() {
        let err = resolve_root(Some("".into()), Some(PathBuf::from("/home")), None).unwrap_err();
        assert!(
            err.to_string()
                .contains("DUX_HOME must be an absolute path"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn resolve_root_falls_through_when_dux_home_unset() {
        let root = resolve_root(None, Some(PathBuf::from("/example/home")), None).unwrap();
        // Should delegate to discover_root with platform defaults
        #[cfg(target_os = "macos")]
        assert_eq!(root, PathBuf::from("/example/home/.dux"));
        #[cfg(not(target_os = "macos"))]
        assert_eq!(root, PathBuf::from("/example/home/.config/dux"));
    }

    // ── expand_path tests ────────────────────────────────────────────────

    #[test]
    fn expand_path_absolute_unchanged() {
        assert_eq!(
            expand_path("/absolute/path").as_deref(),
            Some("/absolute/path")
        );
    }

    #[test]
    fn expand_path_tilde() {
        let home = home::home_dir().expect("home dir");
        let result = expand_path("~/projects/foo").unwrap();
        assert_eq!(result, format!("{}/projects/foo", home.display()));
    }

    #[test]
    fn expand_path_bare_tilde() {
        let home = home::home_dir().expect("home dir");
        assert_eq!(expand_path("~").unwrap(), home.to_string_lossy());
    }

    #[test]
    fn expand_path_dollar_var() {
        // SAFETY: test-only env manipulation; tests are run with --test-threads=1
        // or use unique variable names to avoid races.
        unsafe { std::env::set_var("DUX_TEST_VAR_1", "/test/value") };
        let result = expand_path("$DUX_TEST_VAR_1/subdir").unwrap();
        assert_eq!(result, "/test/value/subdir");
        unsafe { std::env::remove_var("DUX_TEST_VAR_1") };
    }

    #[test]
    fn expand_path_braced_var() {
        unsafe { std::env::set_var("DUX_TEST_VAR_2", "/braced") };
        let result = expand_path("${DUX_TEST_VAR_2}/sub").unwrap();
        assert_eq!(result, "/braced/sub");
        unsafe { std::env::remove_var("DUX_TEST_VAR_2") };
    }

    #[test]
    fn expand_path_unresolved_var_kept_literal() {
        // Unresolved var is preserved literally; if the overall path is still
        // absolute the function succeeds (the path just won't exist on disk).
        let result = expand_path("/prefix/$NONEXISTENT_DUX_VAR_999/suffix");
        assert_eq!(
            result.as_deref(),
            Some("/prefix/$NONEXISTENT_DUX_VAR_999/suffix")
        );
    }

    #[test]
    fn expand_path_rejects_relative() {
        assert!(expand_path("relative/path").is_none());
    }

    #[test]
    fn expand_path_rejects_dotdot_relative() {
        assert!(expand_path("../relative/path").is_none());
    }

    #[test]
    fn expand_path_rejects_traversal() {
        unsafe { std::env::set_var("DUX_TEST_VAR_3", "/safe") };
        assert!(expand_path("$DUX_TEST_VAR_3/../etc/passwd").is_none());
        unsafe { std::env::remove_var("DUX_TEST_VAR_3") };
    }

    #[test]
    fn expand_path_rejects_command_substitution() {
        assert!(expand_path("$(whoami)/foo").is_none());
    }

    #[test]
    fn expand_path_rejects_tilde_user() {
        // `~otheruser/foo` is not supported.
        assert!(expand_path("~otheruser/foo").is_none());
    }

    #[test]
    fn expand_path_rejects_empty_var_name() {
        assert!(expand_path("$/foo").is_none());
    }

    #[test]
    fn expand_path_rejects_empty_braced_var_name() {
        assert!(expand_path("${}/foo").is_none());
    }

    #[test]
    fn project_env_lines_parse_and_expand() {
        unsafe { std::env::set_var("DUX_TEST_PROJECT_ENV_SOURCE", "secret") };
        let env = parse_project_env_lines("EDITOR=true\nAPI_KEY=${DUX_TEST_PROJECT_ENV_SOURCE}")
            .expect("parse env");
        let resolved = resolve_project_env(&env).expect("resolve env");
        assert!(resolved.contains(&("EDITOR".to_string(), "true".to_string())));
        assert!(resolved.contains(&("API_KEY".to_string(), "secret".to_string())));
        unsafe { std::env::remove_var("DUX_TEST_PROJECT_ENV_SOURCE") };
    }

    #[test]
    fn project_env_lines_reject_invalid_names_and_expansions() {
        assert!(parse_project_env_lines("1BAD=value").is_err());
        assert!(parse_project_env_lines("GOOD=${}").is_err());
        assert!(parse_project_env_lines("MISSING_EQUALS").is_err());
    }

    #[test]
    fn agent_env_merges_global_and_project_with_project_override() {
        let global = BTreeMap::from([
            ("EDITOR".to_string(), "true".to_string()),
            ("API_KEY".to_string(), "global".to_string()),
        ]);
        let project = BTreeMap::from([("API_KEY".to_string(), "project".to_string())]);

        let resolved = resolve_agent_env(&global, &project).expect("resolve env");

        assert!(resolved.contains(&("EDITOR".to_string(), "true".to_string())));
        assert!(resolved.contains(&("API_KEY".to_string(), "project".to_string())));
        assert!(!resolved.contains(&("API_KEY".to_string(), "global".to_string())));
    }
}
