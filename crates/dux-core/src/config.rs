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

    /// The canonical config/wire string for this surface, matching the
    /// `#[serde(rename_all = "lowercase")]` representation. Use this anywhere a
    /// `MacroSurface` crosses into TOML or JSON so the casing stays in one place.
    pub fn as_config_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Terminal => "terminal",
            Self::Both => "both",
        }
    }

    /// Parse the canonical config/wire string back into a `MacroSurface`.
    /// Returns `None` for an unrecognized value.
    pub fn from_config_str(s: &str) -> Option<Self> {
        match s {
            "agent" => Some(Self::Agent),
            "terminal" => Some(Self::Terminal),
            "both" => Some(Self::Both),
            _ => None,
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

/// Default cap on concurrent events (`/ws`) WebSocket connections; see
/// [`ServerConfig::max_websocket_events_connections`]. Shared so the config
/// default and the server's router default cannot drift apart.
pub const DEFAULT_MAX_WEBSOCKET_EVENTS_CONNECTIONS: u32 = 32;
/// Default cap on concurrent agent-PTY WebSocket connections — see
/// [`ServerConfig::max_websocket_agent_connections`].
pub const DEFAULT_MAX_WEBSOCKET_AGENT_CONNECTIONS: u32 = 32;
/// Default cap on concurrent terminal-PTY WebSocket connections — see
/// [`ServerConfig::max_websocket_terminal_connections`].
pub const DEFAULT_MAX_WEBSOCKET_TERMINAL_CONNECTIONS: u32 = 64;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// LOCAL MODE bind host. `dux server` binds `host:port` (plus the machine's
    /// Tailscale address when `tailscale_enabled`). Must be an IP literal such as
    /// `127.0.0.1` (loopback, the safe default) or `0.0.0.0` (all interfaces);
    /// hostnames are not resolved. Default `127.0.0.1`.
    pub host: String,
    /// LOCAL MODE port. `dux server` and the palette flip bind `host:port` (plus
    /// the machine's Tailscale address when `tailscale_enabled`). Default 8080.
    pub port: u16,
    /// OPT-OUT Tailscale binding. When true, the server also binds the machine's
    /// Tailscale address (100.64.0.0/10) so tailnet devices reach dux over
    /// WireGuard. Detection shells out to `tailscale ip`; when the CLI is missing
    /// or the daemon is down, dux WARNS and serves the configured host only.
    pub tailscale_enabled: bool,
    /// Extra `Host` header values to accept when the request is NOT same-origin.
    /// dux is trusted-local: it always serves on `host:port` (loopback by default)
    /// and accepts same-origin requests. List any additional hostnames a reverse
    /// proxy or tailnet name forwards under (e.g. `box.tailnet.ts.net`) so those
    /// requests are not rejected by the host guard. Empty by default.
    pub allowed_hosts: Vec<String>,
    /// Colored, vite-style console output for `dux server`. One of `"auto"`
    /// (default — color only when stdout is a terminal, `NO_COLOR` is unset, and
    /// `TERM` is not `dumb`), `"always"` (force color), or `"never"` (plain text).
    /// An unrecognized value is treated as `"auto"` with a warning. The TUI flip's
    /// status screen is unaffected — this only governs the `dux server` CLI.
    pub color: String,
    /// Whether `dux server` prints a per-request access log line (method, path,
    /// status, latency) to its console. The `/healthz` probe is always skipped.
    /// Default true. The access log is console-only (never written to `dux.log`),
    /// so piping `dux server`'s stdout captures it.
    pub access_log: bool,
    /// Maximum number of concurrent events (`/ws`) WebSocket connections. This is
    /// the status/changed-files event stream every browser tab opens. Once this
    /// many are live, further upgrade attempts are rejected with HTTP 503 until a
    /// slot frees. Default 32. A value of 0 permanently blocks this connection
    /// class until the server restarts. Changing this requires a server restart to
    /// take effect: the connection-cap semaphore is built at startup and a config
    /// reload cannot resize it.
    pub max_websocket_events_connections: u32,
    /// Maximum number of concurrent agent-PTY WebSocket connections. This is the
    /// embedded-terminal stream for an agent session. Once this many are live,
    /// further upgrade attempts are rejected with HTTP 503 until a slot frees.
    /// Default 32. A value of 0 permanently blocks this connection class until the
    /// server restarts. Changing this requires a server restart to take effect:
    /// the connection-cap semaphore is built at startup and a config reload cannot
    /// resize it.
    pub max_websocket_agent_connections: u32,
    /// Maximum number of concurrent terminal-PTY WebSocket connections. This is the
    /// standalone scratch-terminal stream. Once this many are live, further upgrade
    /// attempts are rejected with HTTP 503 until a slot frees. Default 64. A value
    /// of 0 permanently blocks this connection class until the server restarts.
    /// Changing this requires a server restart to take effect: the connection-cap
    /// semaphore is built at startup and a config reload cannot resize it.
    pub max_websocket_terminal_connections: u32,
    /// WEB-ONLY display name for this dux instance. Drives the browser tab
    /// `<title>` and the brand wordmark in the web projects pane (the version
    /// line stays directly below it). Set a distinct value per instance (e.g.
    /// "dux #1" or "dux (prod)") to tell several dux tabs apart at a glance.
    /// Default "dux". An empty/whitespace value falls back to "dux" in the UI.
    pub title: String,
    /// WEB-ONLY favicon for this dux instance, so several dux tabs are easy to
    /// tell apart. Empty (default) keeps the bundled dux logo. Otherwise one of:
    /// a COLOUR (a hex value like "#863bff", or a name: violet, purple, blue,
    /// sky, cyan, teal, green, lime, amber, orange, red, pink, rose, slate, gray,
    /// white, black), which renders the dux logo OUTLINE in that colour; or a
    /// custom favicon URL beginning with "http://", "https://", or "/".
    /// Unrecognized values fall back to the bundled logo.
    pub favicon: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderCommandConfig {
    pub command: String,
    pub args: Vec<String>,
    pub resume_args: Option<Vec<String>>,
    pub resume_wait_timeout_ms: Option<u64>,
    pub install_hint: Option<String>,
    /// Scroll-forwarding policy for the wheel and PgUp/PgDn over this
    /// provider's embedded PTY. Tri-state:
    ///
    /// - `None` (key absent) — auto: forward to the child only when it owns
    ///   the screen and asked for the wheel (alternate screen + mouse
    ///   reporting for the wheel; alternate screen alone for the page keys),
    ///   otherwise scroll dux's own host scrollback. This adapts to apps like
    ///   Claude Code that switch to a fullscreen alt-screen renderer.
    /// - `Some(true)` — always forward scroll and page keys to the child.
    /// - `Some(false)` — never forward; always use dux host scrollback.
    pub forward_scroll: Option<bool>,
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
    /// Seconds before a transient status-line message (a success/info
    /// confirmation) auto-clears. Busy/pending and warning/error messages are
    /// unaffected — they persist until replaced. 0 disables auto-clear entirely.
    pub status_clear_seconds: u16,
    pub branch_sync_interval: u16,
    pub show_diff_line_numbers: bool,
    pub diff_tab_width: u16,
    pub github_integration: bool,
    pub auto_reopen_agents: bool,
    /// Show the right-hand Changes pane (the changed-files list) by default.
    /// Toggling it from the command palette or the web's Changes actions menu
    /// persists the new value here immediately — it is not a per-session
    /// override.
    pub show_changes_pane: bool,
    pub pr_banner_position: String,
    pub theme: String,
}

impl Default for Defaults {
    fn default() -> Self {
        let start_directory = home::home_dir().map(|p| p.to_string_lossy().to_string());
        Self {
            provider: "claude".to_string(),
            start_directory,
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

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "127.0.0.1".to_string(),
            port: 8080,
            tailscale_enabled: true,
            allowed_hosts: Vec::new(),
            color: "auto".to_string(),
            access_log: true,
            max_websocket_events_connections: DEFAULT_MAX_WEBSOCKET_EVENTS_CONNECTIONS,
            max_websocket_agent_connections: DEFAULT_MAX_WEBSOCKET_AGENT_CONNECTIONS,
            max_websocket_terminal_connections: DEFAULT_MAX_WEBSOCKET_TERMINAL_CONNECTIONS,
            title: "dux".to_string(),
            favicon: String::new(),
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
            status_clear_seconds: 6,
            branch_sync_interval: 30,
            show_diff_line_numbers: false,
            diff_tab_width: 4,
            github_integration: true,
            auto_reopen_agents: false,
            show_changes_pane: true,
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

pub fn default_provider_commands() -> [(&'static str, ProviderCommandConfig); 4] {
    [
        (
            "claude",
            ProviderCommandConfig {
                command: "claude".to_string(),
                args: Vec::new(),
                resume_args: Some(vec!["--continue".to_string()]),
                resume_wait_timeout_ms: None,
                install_hint: Some("curl -fsSL https://claude.ai/install.sh | bash".to_string()),
                forward_scroll: None,
            },
        ),
        (
            "codex",
            ProviderCommandConfig {
                command: "codex".to_string(),
                args: Vec::new(),
                resume_args: Some(vec!["resume".to_string(), "--last".to_string()]),
                resume_wait_timeout_ms: None,
                install_hint: Some("brew install --cask codex".to_string()),
                forward_scroll: None,
            },
        ),
        (
            "opencode",
            ProviderCommandConfig {
                command: "opencode".to_string(),
                args: Vec::new(),
                resume_args: Some(vec!["--continue".to_string()]),
                resume_wait_timeout_ms: Some(3_000),
                install_hint: Some("curl -fsSL https://opencode.ai/install | bash".to_string()),
                forward_scroll: None,
            },
        ),
        (
            "copilot",
            ProviderCommandConfig {
                command: "copilot".to_string(),
                args: Vec::new(),
                // Copilot's --continue resumes the most recent session
                // globally, not scoped to the current working directory.
                // Unlike claude/codex/opencode, there is no flag
                // to limit resume to the CWD, so we disable it.
                resume_args: None,
                resume_wait_timeout_ms: None,
                install_hint: Some("curl -fsSL https://gh.io/copilot-install | bash".to_string()),
                forward_scroll: None,
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
    #[serde(default)]
    pub server: ServerConfig,
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
                status_clear_seconds: 6,
                branch_sync_interval: 30,
                show_diff_line_numbers: false,
                diff_tab_width: 4,
                github_integration: true,
                auto_reopen_agents: false,
                show_changes_pane: true,
                pr_banner_position: "bottom".to_string(),
                theme: crate::theme::DEFAULT_THEME_NAME.to_string(),
            },
            editor: EditorConfig::default(),
            server: ServerConfig::default(),
            keys: KeysConfig::default(),
            macros: MacrosConfig::default(),
        }
    }
}

impl Config {
    pub fn default_provider(&self) -> crate::model::ProviderKind {
        crate::model::ProviderKind::from_str(&self.defaults.provider)
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

/// Parse and validate `s` as a complete [`Config`] — the same `toml::from_str`
/// check [`load_config`] performs — returning the parsed value on success or a
/// user-facing error message on failure. The web's raw config editor calls this
/// to reject invalid TOML before it overwrites `config.toml`; it also uses the
/// returned value to compare security-sensitive sections against the running
/// config. Note the returned value is the raw parse (no provider defaults
/// applied); callers that want to *adopt* the config should reload from disk via
/// [`load_config`] so provider defaults are reapplied consistently.
pub fn validate_config_str(s: &str) -> Result<Config, String> {
    toml::from_str::<Config>(s).map_err(|e| e.to_string())
}

/// Load config for a read-only consumer (the web server). Reads `config.toml` if
/// present and parses it; on a missing file or parse error, falls back to defaults
/// (logging the error). Always applies provider defaults. Unlike the TUI's
/// `ensure_config`, this never creates, migrates, or writes the config file — the
/// server must not mutate config (that's the TUI's canonical renderer).
pub fn load_config(paths: &DuxPaths) -> Config {
    let mut config = match std::fs::read_to_string(&paths.config_path) {
        Ok(raw) => {
            // One-time migration notice: the single `[server] max_websocket_connections`
            // cap was split into three per-class caps. The unknown key is ignored on
            // load (ServerConfig has no deny_unknown_fields), so warn once so the
            // operator knows their old value is no longer in effect.
            warn_on_removed_max_websocket_connections(&raw);
            match toml::from_str::<Config>(&raw) {
                Ok(cfg) => cfg,
                Err(e) => {
                    crate::logger::error(&format!(
                        "failed to parse {}: {e}; using defaults",
                        paths.config_path.display()
                    ));
                    Config::default()
                }
            }
        }
        Err(_) => Config::default(),
    };
    config.providers.ensure_defaults();
    // Surface a stale/unrecognized editor preference instead of silently falling
    // back to the first editor detected on PATH — e.g. a config left pointing at a
    // now-removed editor like "antigravity"/"windsurf".
    let configured_editor = config.editor.default.trim();
    if !configured_editor.is_empty() && crate::editor::editor_label(configured_editor).is_none() {
        crate::logger::warn(&format!(
            "config editor.default = \"{configured_editor}\" is not a recognized editor; \
             open-in-editor will fall back to the first one detected on PATH \
             (supported: cursor, vscode/code, zed, vscodium, sublime)"
        ));
    }
    config
}

/// Warn once when a `config.toml` still carries the removed
/// `[server] max_websocket_connections` key. Parses the raw TOML generically so a
/// commented-out line never trips the warning, then logs the three replacements
/// and the `=0` semantics change. The key itself is silently ignored on load (no
/// `deny_unknown_fields`), so this is the only place the operator learns their old
/// value stopped taking effect.
fn warn_on_removed_max_websocket_connections(raw: &str) {
    if raw_has_removed_max_websocket_connections(raw) {
        crate::logger::warn(
            "[server] max_websocket_connections has been removed and is being ignored. It \
             was split into max_websocket_events_connections, \
             max_websocket_agent_connections, and max_websocket_terminal_connections. Set \
             those per-class caps instead; a value of 0 still means disable (refuse all \
             new connections of that class until restart).",
        );
    }
}

/// Pure predicate behind the migration warning: true when the raw TOML has a
/// `[server] max_websocket_connections` key. Parses generically so a commented-out
/// line is not detected; a parse failure returns false (the loader surfaces the
/// real parse error separately).
///
/// `pub(crate)` so `config_write` can check the same condition on the write/strip
/// path (where the TUI saves config) and emit the warning there too.
pub(crate) fn raw_has_removed_max_websocket_connections(raw: &str) -> bool {
    toml::from_str::<toml::Value>(raw)
        .ok()
        .and_then(|value| {
            value
                .get("server")
                .and_then(toml::Value::as_table)
                .map(|server| server.contains_key("max_websocket_connections"))
        })
        .unwrap_or(false)
}

/// Check whether a provider command is available on PATH.
/// Returns `Ok(())` if found, or `Err(message)` with a user-friendly install hint.
pub fn check_provider_available(config: &ProviderCommandConfig) -> std::result::Result<(), String> {
    if provider_command_available(&config.command) {
        return Ok(());
    }

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

fn provider_command_available(command: &str) -> bool {
    if command.trim().is_empty() {
        return false;
    }

    let path = Path::new(command);
    if path.components().count() > 1 {
        return is_executable_file(path);
    }

    env::var_os("PATH")
        .map(|paths| provider_command_available_in_path(command, &paths))
        .unwrap_or(false)
}

fn provider_command_available_in_path(command: &str, paths: &std::ffi::OsStr) -> bool {
    env::split_paths(paths).any(|dir| {
        let candidate = dir.join(command);
        is_executable_file(&candidate)
    })
}

fn is_executable_file(path: &Path) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }

    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o111 != 0
}

/// One address in a [`ServerPlan`], tagged with whether binding it is REQUIRED or
/// merely BEST-EFFORT.
///
/// - `required: true` — a deliberate listener (the configured `host:port` or an
///   explicit `--bind`). A bind failure here is FATAL per the explicit-failure
///   tenet: the operator asked for this address, so refusing to serve it silently
///   would hide their intent.
/// - `required: false` — an opportunistic add-on. Today this is ONLY the
///   Tailscale leg of LOCAL MODE: it is auto-added when a Tailscale address is
///   detected, mirroring how tailscale-NOT-detected already degrades to loopback
///   with a warning. A bind failure here must NOT block the server — it warns
///   loudly and serves on the remaining (bound) addresses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanAddr {
    addr: std::net::SocketAddr,
    required: bool,
}

impl PlanAddr {
    /// A required (deliberate) listener — its bind failure is fatal.
    pub fn required(addr: std::net::SocketAddr) -> Self {
        Self {
            addr,
            required: true,
        }
    }

    /// A best-effort (opportunistic) listener — its bind failure degrades to a
    /// warning and the server continues on the remaining addresses.
    pub fn best_effort(addr: std::net::SocketAddr) -> Self {
        Self {
            addr,
            required: false,
        }
    }

    /// The socket address this listener targets.
    pub fn addr(&self) -> std::net::SocketAddr {
        self.addr
    }

    /// Whether a bind failure on this address is fatal. `false` is only
    /// constructible via [`PlanAddr::best_effort`], so the best-effort invariant
    /// is enforced by the type rather than by call-site discipline.
    pub fn is_required(&self) -> bool {
        self.required
    }
}

/// The fully-resolved listening plan for `dux server`, the single source of
/// truth the binary hands to dux-web. Lives in dux-core because both server entry
/// points and the resolver rules belong in the crate the binary and TUI share;
/// keeping the plan type here avoids a dux-web dependency from the resolver and
/// keeps the bind rules in one place.
///
/// The addresses are deduplicated and listed in a stable order, each tagged
/// [`PlanAddr::required`] (the configured/`--bind` primary, whose bind failure is
/// fatal) or [`PlanAddr::best_effort`] (the Tailscale leg, whose bind failure
/// degrades to a warning and serves the remaining addresses).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerPlan {
    pub addrs: Vec<PlanAddr>,
}

/// CLI overrides for the server plan. Every field is `None`/`false` when the
/// operator passed nothing, so config values win by default and a present CLI
/// value takes precedence.
#[derive(Clone, Debug, Default)]
pub struct ServerCliOverrides {
    /// `--bind <ADDR:PORT>`: bind this exact address, overriding config host+port.
    pub bind: Option<String>,
    /// `--port <PORT>`: override `[server] port` only. Ignored when `bind` is set.
    pub port: Option<u16>,
    /// `--no-tailscale`: do not bind the Tailscale leg this run.
    pub no_tailscale: bool,
}

/// Resolve the complete `dux server` listening plan from config + CLI overrides +
/// the detected Tailscale address. This is the single source of truth for the
/// bind rules; the binary reads the returned [`ServerPlan`]'s addresses.
///
/// dux is trusted-local: the primary listener is always the configured
/// `host:port` (loopback by default) or an explicit `--bind`. There is no auth
/// gate and no public-bind refusal; the operator chooses the host directly and
/// the host guard (config `allowed_hosts`) governs which `Host` headers are
/// accepted. `tailscale_ip` is the detected Tailscale address (or `None` when
/// disabled / not detected); when present and not already covered by the primary
/// bind it is added as a BEST-EFFORT leg.
pub fn resolve_server_plan(
    server: &ServerConfig,
    cli: &ServerCliOverrides,
    tailscale_ip: Option<std::net::IpAddr>,
) -> Result<ServerPlan> {
    let bind: std::net::SocketAddr = match cli.bind.as_deref() {
        Some(raw) => raw.parse().map_err(|_| {
            anyhow!(
                "invalid --bind address \"{raw}\": expected IP:port, e.g. 0.0.0.0:8080 \
                 (hostnames are not resolved)"
            )
        })?,
        None => {
            let host: std::net::IpAddr = server.host.parse().map_err(|_| {
                anyhow!(
                    "invalid [server] host \"{}\": expected an IP address such as 127.0.0.1 \
                     or 0.0.0.0 (hostnames are not resolved). Set [server] host in config.toml \
                     or pass --bind IP:port.",
                    server.host
                )
            })?;
            std::net::SocketAddr::new(host, cli.port.unwrap_or(server.port))
        }
    };
    if bind.port() == 0 {
        bail!(
            "refusing to bind {bind}: port 0 means \"pick any free port\", so there would be no \
             stable address to open. Set [server] port (default 8080) or pass --port / --bind with \
             a non-zero port."
        );
    }
    let ts = if server.tailscale_enabled && !cli.no_tailscale {
        tailscale_ip
    } else {
        None
    };
    Ok(ServerPlan {
        addrs: plan_addrs(bind, ts),
    })
}

/// Primary address (REQUIRED) plus the Tailscale leg (BEST-EFFORT) when detected and
/// not already covered. A wildcard primary (0.0.0.0 / ::) already binds the Tailscale
/// interface, and an explicit bind to the Tailscale address is already in the list, so
/// both cases skip the extra leg.
pub(crate) fn plan_addrs(
    bind: std::net::SocketAddr,
    tailscale_ip: Option<std::net::IpAddr>,
) -> Vec<PlanAddr> {
    let mut addrs = vec![PlanAddr::required(bind)];
    if let Some(ip) = tailscale_ip {
        let ts = std::net::SocketAddr::new(ip, bind.port());
        let subsumed = bind.ip().is_unspecified() || bind.ip() == ip;
        if !subsumed && !addrs.iter().any(|p| p.addr() == ts) {
            addrs.push(PlanAddr::best_effort(ts));
        }
    }
    addrs
}

/// LOCAL MODE bind addresses for the TUI palette flip: loopback (REQUIRED) plus the
/// Tailscale leg. A thin wrapper over `plan_addrs` so the flip can never open a
/// non-loopback primary listener.
pub fn local_addrs(port: u16, tailscale_ip: Option<std::net::IpAddr>) -> Vec<PlanAddr> {
    plan_addrs(
        std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        tailscale_ip,
    )
}

#[cfg(test)]
mod local_addrs_tests {
    use super::{PlanAddr, local_addrs};

    #[test]
    fn loopback_only_when_no_tailscale() {
        let addrs = local_addrs(8080, None);
        assert_eq!(
            addrs,
            vec![PlanAddr::required("127.0.0.1:8080".parse().unwrap())]
        );
    }

    #[test]
    fn loopback_required_tailscale_best_effort_when_present() {
        // Loopback is REQUIRED (a bind failure is fatal); the auto-added Tailscale
        // leg is BEST-EFFORT (a bind failure degrades to loopback + a warning).
        let ts = "100.101.102.103".parse().unwrap();
        let addrs = local_addrs(9090, Some(ts));
        assert_eq!(
            addrs,
            vec![
                PlanAddr::required("127.0.0.1:9090".parse().unwrap()),
                PlanAddr::best_effort("100.101.102.103:9090".parse().unwrap()),
            ]
        );
        assert!(addrs[0].is_required(), "loopback must be required");
        assert!(
            !addrs[1].is_required(),
            "the Tailscale leg must be best-effort"
        );
    }

    #[test]
    fn tailscale_ipv6_uses_bracketed_socketaddr() {
        let ts = "fd7a:115c:a1e0::1".parse().unwrap();
        let addrs = local_addrs(8080, Some(ts));
        assert_eq!(
            addrs,
            vec![
                PlanAddr::required("127.0.0.1:8080".parse().unwrap()),
                PlanAddr::best_effort("[fd7a:115c:a1e0::1]:8080".parse().unwrap()),
            ]
        );
    }
}

#[cfg(test)]
mod resolve_plan_tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};
    fn ts() -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(100, 100, 0, 1))
    }
    fn cli() -> ServerCliOverrides {
        ServerCliOverrides::default()
    }

    #[test]
    fn default_loopback_only_without_tailscale() {
        let p = resolve_server_plan(&ServerConfig::default(), &cli(), None).unwrap();
        assert_eq!(
            p.addrs,
            vec![PlanAddr::required("127.0.0.1:8080".parse().unwrap())]
        );
    }
    #[test]
    fn default_adds_best_effort_tailscale_leg() {
        let p = resolve_server_plan(&ServerConfig::default(), &cli(), Some(ts())).unwrap();
        assert_eq!(p.addrs.len(), 2);
        assert!(!p.addrs[1].is_required());
    }
    #[test]
    fn no_tailscale_suppresses_leg() {
        let c = ServerCliOverrides {
            no_tailscale: true,
            ..cli()
        };
        assert_eq!(
            resolve_server_plan(&ServerConfig::default(), &c, Some(ts()))
                .unwrap()
                .addrs
                .len(),
            1
        );
    }
    #[test]
    fn bind_wildcard_overrides_and_subsumes_tailscale() {
        let c = ServerCliOverrides {
            bind: Some("0.0.0.0:9000".into()),
            ..cli()
        };
        let p = resolve_server_plan(&ServerConfig::default(), &c, Some(ts())).unwrap();
        assert_eq!(
            p.addrs,
            vec![PlanAddr::required("0.0.0.0:9000".parse().unwrap())]
        );
    }
    #[test]
    fn port_flag_overrides_only_port() {
        let c = ServerCliOverrides {
            port: Some(7000),
            ..cli()
        };
        let p = resolve_server_plan(&ServerConfig::default(), &c, None).unwrap();
        assert_eq!(
            p.addrs,
            vec![PlanAddr::required("127.0.0.1:7000".parse().unwrap())]
        );
    }
    #[test]
    fn bind_beats_port() {
        let c = ServerCliOverrides {
            bind: Some("127.0.0.1:1234".into()),
            port: Some(7000),
            ..cli()
        };
        let p = resolve_server_plan(&ServerConfig::default(), &c, None).unwrap();
        assert_eq!(
            p.addrs,
            vec![PlanAddr::required("127.0.0.1:1234".parse().unwrap())]
        );
    }
    #[test]
    fn port_zero_refused() {
        let c = ServerConfig {
            port: 0,
            ..ServerConfig::default()
        };
        assert!(resolve_server_plan(&c, &cli(), None).is_err());
    }
    #[test]
    fn invalid_bind_refused() {
        let c = ServerCliOverrides {
            bind: Some("nope".into()),
            ..cli()
        };
        assert!(resolve_server_plan(&ServerConfig::default(), &c, None).is_err());
    }
    #[test]
    fn invalid_host_refused() {
        let c = ServerConfig {
            host: "example.com".into(),
            ..ServerConfig::default()
        };
        assert!(resolve_server_plan(&c, &cli(), None).is_err());
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn validate_config_str_accepts_a_full_render_and_rejects_garbage() {
        // The canonical plain render of the default config must round-trip
        // through the validator (this is exactly what the web editor writes).
        let rendered = crate::config_write::render_config_plain(&Config::default());
        assert!(
            validate_config_str(&rendered).is_ok(),
            "rendered default config must validate:\n{rendered}"
        );
        assert!(
            validate_config_str("this is = = not valid toml").is_err(),
            "garbage must be rejected"
        );
        // Structurally-valid TOML with a wrong-typed field must also be rejected
        // (deserialization failure, not just a parse failure) — otherwise the web
        // editor would accept a value the engine can't load.
        assert!(
            validate_config_str("[ui]\nagent_scrollback_lines = \"lots\"\n").is_err(),
            "a string for a numeric field must be rejected"
        );
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

    fn write_executable(path: &Path) {
        std::fs::write(path, "#!/bin/sh\nexit 0\n").expect("write fixture command");
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = std::fs::metadata(path)
            .expect("fixture metadata")
            .permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).expect("chmod fixture command");
    }

    #[test]
    fn provider_command_path_lookup_accepts_executable_from_path() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let command = dir.path().join("custom-tool");
        write_executable(&command);
        let paths = std::env::join_paths([dir.path()]).expect("join path");

        assert!(provider_command_available_in_path(
            "custom-tool",
            paths.as_os_str()
        ));
    }

    #[test]
    fn provider_command_path_lookup_rejects_missing_command() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let paths = std::env::join_paths([dir.path()]).expect("join path");

        assert!(!provider_command_available_in_path(
            "missing-tool",
            paths.as_os_str()
        ));
    }

    #[test]
    fn provider_command_path_lookup_accepts_absolute_executable() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let command = dir.path().join("custom-tool");
        write_executable(&command);

        assert!(provider_command_available(&command.to_string_lossy()));
    }

    #[test]
    fn provider_command_path_lookup_rejects_non_executable_path() {
        let dir = tempfile::TempDir::new().expect("tempdir");
        let command = dir.path().join("custom-tool");
        std::fs::write(&command, "#!/bin/sh\n").expect("write fixture command");

        assert!(!provider_command_available(&command.to_string_lossy()));
    }

    #[test]
    fn provider_availability_error_uses_install_hint() {
        let cfg = ProviderCommandConfig {
            command: "definitely-missing-provider-command".to_string(),
            install_hint: Some("install custom-tool".to_string()),
            ..Default::default()
        };

        let err = check_provider_available(&cfg).expect_err("command should be missing");
        assert!(err.contains("definitely-missing-provider-command"));
        assert!(err.contains("Install with: install custom-tool"));
    }

    // ── load_config tests ────────────────────────────────────────────────

    fn make_test_paths(root: &std::path::Path) -> DuxPaths {
        DuxPaths {
            config_path: root.join("config.toml"),
            sessions_db_path: root.join("sessions.sqlite3"),
            worktrees_root: root.join("worktrees"),
            lock_path: root.join("dux.lock"),
            root: root.to_path_buf(),
        }
    }

    #[test]
    fn load_config_reads_custom_provider_command() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = make_test_paths(dir.path());
        std::fs::write(
            &paths.config_path,
            r#"
[providers.claude]
command = "/custom/claude"

[ui]
github_integration = false
"#,
        )
        .expect("write config");

        let config = load_config(&paths);

        assert_eq!(
            config.providers.commands["claude"].command, "/custom/claude",
            "custom provider command should be loaded from config.toml"
        );
        assert!(
            !config.ui.github_integration,
            "ui.github_integration should be false per config.toml"
        );
        // Provider defaults must still be populated (e.g. codex should exist).
        assert!(
            config.providers.commands.contains_key("codex"),
            "ensure_defaults should add missing default providers"
        );
    }

    #[test]
    fn load_config_falls_back_to_defaults_when_file_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = make_test_paths(dir.path());
        // No config.toml written — file does not exist.

        let config = load_config(&paths);

        // Provider defaults must be present.
        assert!(
            config.providers.commands.contains_key("claude"),
            "claude provider should be present via defaults"
        );
        assert!(
            config.providers.commands.contains_key("codex"),
            "codex provider should be present via defaults"
        );
    }

    #[test]
    fn server_config_defaults_when_section_absent() {
        // A config TOML with no [server] section must still parse and yield the
        // safe local defaults (loopback host, port 8080, Tailscale opt-out on).
        let config: Config = toml::from_str("").expect("empty config should parse");
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 8080);
        assert!(config.server.tailscale_enabled);
        assert!(config.server.allowed_hosts.is_empty());
    }

    #[test]
    fn server_title_defaults_to_dux_and_parses_override() {
        // No [server] section: title defaults to the product name.
        let default: Config = toml::from_str("").expect("empty config should parse");
        assert_eq!(default.server.title, "dux");

        // An explicit title (e.g. to tell multiple instances apart) round-trips.
        let config: Config = toml::from_str(
            r#"
[server]
title = "dux #1"
"#,
        )
        .expect("config with [server] title should parse");
        assert_eq!(config.server.title, "dux #1");
    }

    #[test]
    fn server_favicon_defaults_empty_and_parses_override() {
        // No [server] section: favicon is empty, meaning "use the bundled logo".
        let default: Config = toml::from_str("").expect("empty config should parse");
        assert_eq!(default.server.favicon, "");

        // An explicit favicon (a colour, here) round-trips verbatim; the web
        // interprets the string (colour vs URL vs default).
        let config: Config = toml::from_str(
            r#"
[server]
favicon = "violet"
"#,
        )
        .expect("config with [server] favicon should parse");
        assert_eq!(config.server.favicon, "violet");
    }

    #[test]
    fn server_config_parses_full_section() {
        let config: Config = toml::from_str(
            r#"
[server]
host = "0.0.0.0"
port = 9000
tailscale_enabled = false
allowed_hosts = ["box.tailnet.ts.net"]
"#,
        )
        .expect("config with full [server] should parse");
        assert_eq!(config.server.host, "0.0.0.0");
        assert_eq!(config.server.port, 9000);
        assert!(!config.server.tailscale_enabled);
        assert_eq!(
            config.server.allowed_hosts,
            vec!["box.tailnet.ts.net".to_string()]
        );
    }

    #[test]
    fn server_config_partial_section_defaults_remaining_fields() {
        // Only `port` is provided; the rest default.
        let config: Config = toml::from_str(
            r#"
[server]
port = 9000
"#,
        )
        .expect("config with partial [server] should parse");
        assert_eq!(config.server.host, "127.0.0.1");
        assert_eq!(config.server.port, 9000);
        assert!(config.server.tailscale_enabled);
        assert!(config.server.allowed_hosts.is_empty());
    }

    /// Deserializing a `[server]` table that omits all three `max_websocket_*` keys
    /// must yield the expected defaults via the container `#[serde(default)]` plus
    /// the manual `Default` impl. Pinned so a serde refactor cannot silently zero
    /// out the caps.
    #[test]
    fn server_config_websocket_caps_default_when_keys_absent() {
        let config: Config = toml::from_str(
            r#"
[server]
port = 8080
"#,
        )
        .expect("config without max_websocket_* keys should parse");
        assert_eq!(
            config.server.max_websocket_events_connections,
            DEFAULT_MAX_WEBSOCKET_EVENTS_CONNECTIONS,
            "events cap must default to {DEFAULT_MAX_WEBSOCKET_EVENTS_CONNECTIONS}"
        );
        assert_eq!(
            config.server.max_websocket_agent_connections, DEFAULT_MAX_WEBSOCKET_AGENT_CONNECTIONS,
            "agent cap must default to {DEFAULT_MAX_WEBSOCKET_AGENT_CONNECTIONS}"
        );
        assert_eq!(
            config.server.max_websocket_terminal_connections,
            DEFAULT_MAX_WEBSOCKET_TERMINAL_CONNECTIONS,
            "terminal cap must default to {DEFAULT_MAX_WEBSOCKET_TERMINAL_CONNECTIONS}"
        );
    }

    #[test]
    fn load_config_falls_back_to_defaults_on_malformed_toml() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = make_test_paths(dir.path());
        std::fs::write(&paths.config_path, "this is not valid toml ][[[")
            .expect("write bad config");

        // Must not panic; must return usable defaults.
        let config = load_config(&paths);

        assert!(
            config.providers.commands.contains_key("claude"),
            "claude provider should be present via defaults after parse failure"
        );
    }

    #[test]
    fn old_max_websocket_connections_key_still_loads_and_is_ignored() {
        // Back-compat: the removed `max_websocket_connections` key parses without
        // error because `ServerConfig` has no `#[serde(deny_unknown_fields)]` (TOML
        // simply ignores unknown keys; this is not a `serde(default)` effect), and
        // the three new split fields take their per-field defaults.
        let toml = r#"[server]
max_websocket_connections = 16
"#;
        let cfg: Config = toml::from_str(toml).expect("old config must still parse");
        assert_eq!(cfg.server.max_websocket_events_connections, 32);
        assert_eq!(cfg.server.max_websocket_agent_connections, 32);
        assert_eq!(cfg.server.max_websocket_terminal_connections, 64);
    }

    #[test]
    fn detects_removed_max_websocket_connections_key_for_migration_warning() {
        assert!(raw_has_removed_max_websocket_connections(
            "[server]\nmax_websocket_connections = 16\n"
        ));
        // A commented-out line must NOT trip the warning.
        assert!(!raw_has_removed_max_websocket_connections(
            "[server]\n# max_websocket_connections = 16\n"
        ));
        // The new split keys must NOT trip the warning.
        assert!(!raw_has_removed_max_websocket_connections(
            "[server]\nmax_websocket_events_connections = 16\n"
        ));
        assert!(!raw_has_removed_max_websocket_connections("[server]\n"));
    }
}
