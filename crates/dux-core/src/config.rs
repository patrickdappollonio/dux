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

/// Default cap on concurrent WebSocket connections — see
/// [`ServerConfig::max_websocket_connections`]. Shared so the config default and
/// the server's router default cannot drift apart.
pub const DEFAULT_MAX_WEBSOCKET_CONNECTIONS: u32 = 128;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// LOCAL MODE port. The palette flip and the `dux server` no-`listen_addrs`
    /// fallback bind `127.0.0.1:port` (plus the machine's Tailscale address when
    /// `tailscale_enabled`). Default 8080.
    pub port: u16,
    /// OPT-OUT Tailscale binding for LOCAL MODE. When true, local mode also binds
    /// the machine's Tailscale address (100.64.0.0/10) so tailnet devices reach
    /// dux over WireGuard. Detection shells out to `tailscale ip`; when the CLI is
    /// missing or the daemon is down, dux WARNS and serves loopback only.
    pub tailscale_enabled: bool,
    /// FULL WEB MODE listeners, `dux server` only. Each entry is an `IP:port`
    /// SocketAddr. Empty = use LOCAL MODE (`port` + Tailscale). The palette flip
    /// NEVER reads this field.
    pub listen_addrs: Vec<String>,
    /// DEPRECATED: superseded by `port` + `listen_addrs`. Kept for serde so old
    /// configs parse; migrated away on load (see the TUI deprecation machinery).
    /// A loopback `bind` adopts its port into `port`; a non-loopback `bind` is
    /// appended to `listen_addrs`. The canonical renderer no longer emits it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bind: Option<String>,
    pub insecure_allow_remote: bool,
    /// Acknowledge serving UNENCRYPTED plain HTTP on a non-loopback (public)
    /// listen address. Mirrors the `--dangerously-listen-http` CLI flag so the
    /// choice is reviewable in config — and so a config-only rollback off
    /// `[server.acme]` does not brick a public server (the CLI flag alone would
    /// otherwise be the only escape). This only satisfies the ENCRYPTION half of
    /// the public-bind gate: a public bind ALSO requires auth ([auth] users or
    /// `insecure_allow_remote`), so setting this alone does not unblock startup.
    /// Prefer built-in TLS via `[server.acme]`; only set this when an upstream
    /// proxy terminates TLS or you accept the risk on a trusted network. Default
    /// false.
    pub dangerously_listen_http: bool,
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
    /// Maximum number of concurrent WebSocket (`/ws`) connections. Once this many
    /// are live, further upgrade attempts are rejected with HTTP 503 until a slot
    /// frees. A safety bound against connection exhaustion (a runaway tab loop, a
    /// buggy reconnector); the trusted single-operator deployment normally uses a
    /// handful. Default 128. A value of 0 is treated as "no new connections".
    /// Changing this requires a server restart to take effect — the connection-cap
    /// semaphore is built at startup and a config reload cannot resize it.
    pub max_websocket_connections: u32,
    pub acme: AcmeSettings,
}

/// Built-in ACME (Let's Encrypt) settings for `dux server`.
///
/// When `enabled` is true, `dux server` runs its own HTTP-01 ACME client:
/// it serves the challenge on `http_port`, redirects everything else to
/// HTTPS, and serves TLS on `https_port`. All fields are serde-defaulted so
/// older `config.toml` files without a `[server.acme]` section parse cleanly
/// into the safe (ACME off) defaults. Changing any of these requires a server
/// RESTART — the listeners are bound at startup and reload-config does not
/// rebind them.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AcmeSettings {
    pub enabled: bool,
    pub domains: Vec<String>,
    pub email: String,
    pub http_port: u16,
    pub https_port: u16,
    pub production: bool,
    pub cache_dir: Option<String>,
}

impl Default for AcmeSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            domains: Vec::new(),
            email: String::new(),
            http_port: 80,
            https_port: 443,
            production: true,
            cache_dir: None,
        }
    }
}

/// Web UI login credentials.
///
/// Each entry is an htpasswd-style `"username:bcrypt-hash"` string. This shape
/// renders as a self-documenting TOML array and round-trips trivially through
/// `config_write`. Auth turns ON automatically when at least one valid entry
/// exists (see [`crate::auth::auth_enabled`]); an empty list means the gate is
/// off. Entries are managed by the TUI palette commands (server-add-user /
/// server-remove-user) or by editing the config directly.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthConfig {
    pub users: Vec<String>,
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
    /// Seconds before a transient status-line message (a success/info
    /// confirmation) auto-clears. Busy/pending and warning/error messages are
    /// unaffected — they persist until replaced. 0 disables auto-clear entirely.
    pub status_clear_seconds: u16,
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

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 8080,
            tailscale_enabled: true,
            listen_addrs: Vec::new(),
            bind: None,
            insecure_allow_remote: false,
            dangerously_listen_http: false,
            color: "auto".to_string(),
            access_log: true,
            max_websocket_connections: DEFAULT_MAX_WEBSOCKET_CONNECTIONS,
            acme: AcmeSettings::default(),
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
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub auth: AuthConfig,
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
                pr_banner_position: "bottom".to_string(),
                theme: crate::theme::DEFAULT_THEME_NAME.to_string(),
            },
            editor: EditorConfig::default(),
            server: ServerConfig::default(),
            auth: AuthConfig::default(),
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

/// Load config for a read-only consumer (the web server). Reads `config.toml` if
/// present and parses it; on a missing file or parse error, falls back to defaults
/// (logging the error). Always applies provider defaults. Unlike the TUI's
/// `ensure_config`, this never creates, migrates, or writes the config file — the
/// server must not mutate config (that's the TUI's canonical renderer).
pub fn load_config(paths: &DuxPaths) -> Config {
    let mut config = match std::fs::read_to_string(&paths.config_path) {
        Ok(raw) => match toml::from_str::<Config>(&raw) {
            Ok(cfg) => cfg,
            Err(e) => {
                crate::logger::error(&format!(
                    "failed to parse {}: {e}; using defaults",
                    paths.config_path.display()
                ));
                Config::default()
            }
        },
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

/// One address in a [`ServerPlan::PlainHttp`] plan, tagged with whether binding
/// it is REQUIRED or merely BEST-EFFORT.
///
/// - `required: true` — a deliberate listener (loopback, or any explicit
///   `listen_addrs` entry). A bind failure here is FATAL per the explicit-failure
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

/// LOCAL MODE bind addresses: loopback on `port` (REQUIRED), plus the machine's
/// Tailscale address on `port` when one was detected (BEST-EFFORT). This is the
/// SHARED resolution for both the palette flip and the `dux server`
/// empty-`listen_addrs` fallback.
///
/// It deliberately takes NO `listen_addrs` and NO safety flags: local mode is a
/// mode, not an address. Loopback is always safe (only this machine reaches it);
/// the Tailscale address is reachable only by the operator's own tailnet over
/// WireGuard-encrypted transit, so neither needs the public-bind gates. The flip
/// calls this and therefore can never open a public listener — that is enforced
/// structurally by this signature, not by a refusal branch.
///
/// The Tailscale leg is BEST-EFFORT: a third-party process already holding the
/// Tailscale `ip:port` must not block the whole serve — it degrades to
/// loopback-only with a warning, exactly as tailscale-not-DETECTED already does.
/// Loopback is REQUIRED: if even loopback cannot bind there is nothing to serve.
///
/// `tailscale_ip` is the already-fetched address (the caller runs detection on a
/// worker / at CLI startup); `None` means "Tailscale off or not detected" and
/// yields loopback only.
pub fn local_addrs(port: u16, tailscale_ip: Option<std::net::IpAddr>) -> Vec<PlanAddr> {
    let mut addrs = vec![PlanAddr::required(std::net::SocketAddr::from((
        [127, 0, 0, 1],
        port,
    )))];
    if let Some(ip) = tailscale_ip {
        let ts = std::net::SocketAddr::new(ip, port);
        // Guard against a Tailscale IP that is itself loopback (shouldn't happen,
        // but keeps the list deduplicated and the listener count honest).
        if !addrs.iter().any(|p| p.addr() == ts) {
            addrs.push(PlanAddr::best_effort(ts));
        }
    }
    addrs
}

/// The fully-resolved listening plan for `dux server`, the single source of
/// truth the binary hands to dux-web. Lives in dux-core because both server entry
/// points and the resolver
/// rules belong in the crate the binary and TUI share; keeping the plan type
/// here avoids a dux-web dependency from the resolver and keeps the bind rules
/// in one place.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServerPlan {
    /// Plain HTTP on one or more addresses (multi-listener). TLS is either absent
    /// (loopback dev / Tailscale / LAN testing) or terminated by an upstream
    /// proxy. The addresses are deduplicated and listed in a stable order, each
    /// tagged [`PlanAddr::required`] (a deliberate listener whose bind failure is
    /// fatal) or [`PlanAddr::best_effort`] (the Tailscale leg of LOCAL MODE, whose
    /// bind failure degrades to a warning and serves the remaining addresses).
    PlainHttp { addrs: Vec<PlanAddr> },
    /// Built-in ACME: serve the HTTP-01 challenge + HTTPS redirect on
    /// `http_addr`, serve TLS on `https_addr`. `cache_dir` holds the ACME
    /// account and certificate PRIVATE KEYS.
    Acme {
        http_addr: std::net::SocketAddr,
        https_addr: std::net::SocketAddr,
        domains: Vec<String>,
        email: String,
        production: bool,
        cache_dir: PathBuf,
    },
}

/// CLI overrides for the server plan. Every field is `None`/`false` when the
/// operator passed nothing, so config values win by default and a present CLI
/// value takes precedence (the existing `--bind` precedence, extended).
#[derive(Clone, Debug, Default)]
pub struct ServerCliOverrides {
    /// `--port`: LOCAL MODE port override (used only when no `--listen`/config
    /// `listen_addrs` exist — i.e. the local-mode fallback).
    pub port: Option<u16>,
    /// `--listen <ADDR:PORT>` (repeatable) and the deprecated `--bind` alias.
    /// When non-empty, replaces config `listen_addrs` entirely (FULL WEB MODE).
    pub listen: Vec<String>,
    /// `--no-tailscale`: force LOCAL MODE Tailscale binding off for this run,
    /// regardless of `tailscale_enabled` in config.
    pub no_tailscale: bool,
    pub insecure_allow_remote: bool,
    /// `--acme-domain` (repeatable). When non-empty, replaces config domains.
    pub acme_domains: Vec<String>,
    /// `--acme-email`. When set, overrides config email.
    pub acme_email: Option<String>,
    /// `--http-port`. When set, overrides config http_port.
    pub http_port: Option<u16>,
    /// `--https-port`. When set, overrides config https_port.
    pub https_port: Option<u16>,
    /// `--no-acme`: force ACME off regardless of config.
    pub no_acme: bool,
    /// `--dangerously-listen-http`: the named opt-in for public PLAIN HTTP.
    pub dangerously_listen_http: bool,
}

/// Resolve the complete `dux server` listening plan from config + auth state +
/// CLI overrides + the detected Tailscale address. This is the single source of
/// truth for the bind/ACME rules; the binary matches on the returned
/// [`ServerPlan`].
///
/// `auth_enabled` already accounts for `--disable-auth` (it is false when the
/// gate is deliberately disabled). `auth_explicitly_disabled` is threaded
/// separately so the ACME gate can distinguish "no users configured" (refuse)
/// from "deliberately disabled for an upstream auth proxy" (allowed, because it
/// was named explicitly). `tailscale_ip` is the detected Tailscale address (or
/// `None` when disabled / not detected); it is used both to fill LOCAL MODE and
/// to classify `listen_addrs` entries as local. `config_dir` derives the default
/// ACME cache dir.
///
/// Rules:
/// - ACME ON (config `enabled` and not `--no-acme`): unchanged from T1 — needs
///   ≥1 domain and (`auth_enabled` OR `auth_explicitly_disabled`); binds
///   `0.0.0.0:http_port` + `0.0.0.0:https_port`.
/// - ACME OFF → PlainHttp:
///   - EMPTY `listen_addrs` (and no `--listen`) → LOCAL MODE: loopback:port
///     (REQUIRED) plus the Tailscale address:port when detected (BEST-EFFORT — a
///     bind failure there degrades to loopback with a warning). Always safe; no
///     gates.
///   - NON-EMPTY → FULL WEB MODE: parse each entry as a SocketAddr (hostnames
///     rejected, no DNS). Classify each local (loopback OR == the Tailscale IP)
///     vs public. If ANY entry is public, the public-bind gates apply to the
///     whole plan: (`auth_enabled` OR `insecure_allow_remote`) AND
///     `dangerously_listen_http`; the refusal names the offending entry and the
///     exact missing piece (when BOTH the auth leg and the scary flag are
///     missing, a single message names both).
pub fn resolve_server_plan(
    server: &ServerConfig,
    auth_enabled: bool,
    auth_explicitly_disabled: bool,
    cli: &ServerCliOverrides,
    tailscale_ip: Option<std::net::IpAddr>,
    config_dir: &Path,
) -> Result<ServerPlan> {
    let acme_on = server.acme.enabled && !cli.no_acme;

    if acme_on {
        let domains: Vec<String> = if cli.acme_domains.is_empty() {
            server.acme.domains.clone()
        } else {
            cli.acme_domains.clone()
        };
        if domains.is_empty() {
            bail!(
                "refusing to start the ACME (Let's Encrypt) server: no domains are configured. \
                 ACME issues certificates for specific hostnames, so it needs at least one. \
                 Add domains to [server.acme] in config.toml (for example \
                 domains = [\"dux.example.com\"]) or pass --acme-domain dux.example.com \
                 (repeatable)."
            );
        }

        if !auth_enabled && !auth_explicitly_disabled {
            bail!(
                "refusing to start the ACME (Let's Encrypt) server with no authentication: \
                 the certificates make the web UI reachable over the public internet, \
                 so it must be protected. Add at least one user to [auth] in config.toml \
                 (or use the server-add-user palette command) so the login gate protects it. \
                 Alternatively, if an upstream auth proxy handles authentication, \
                 pass --disable-auth to acknowledge that explicitly."
            );
        }

        let http_port = cli.http_port.unwrap_or(server.acme.http_port);
        let https_port = cli.https_port.unwrap_or(server.acme.https_port);
        // Port 0 means "let the OS pick an ephemeral port", which is meaningless
        // for ACME: HTTP-01 needs the challenge reachable on the public :80 the
        // CA dials, and the published HTTPS URL needs a fixed port. Refuse it.
        if http_port == 0 || https_port == 0 {
            bail!(
                "refusing to start the ACME (Let's Encrypt) server: port 0 means \
                 \"pick any free port\", but ACME needs fixed, publicly reachable ports \
                 (HTTP-01 validation dials :80 and the certificate URL uses the HTTPS port). \
                 Set [server.acme] http_port and https_port in config.toml (the defaults are \
                 80 and 443), or pass --http-port / --https-port with non-zero values."
            );
        }
        // The two listeners cannot share a port — one terminates TLS, the other
        // answers the plaintext challenge + redirect, and they bind the SAME
        // wildcard address, so an identical port is a guaranteed bind clash.
        if http_port == https_port {
            bail!(
                "refusing to start the ACME (Let's Encrypt) server: the HTTP and HTTPS ports \
                 are both {http_port}, but they must differ — dux binds one plaintext listener \
                 (for the HTTP-01 challenge and the HTTPS redirect) and one TLS listener, and \
                 they cannot share a port. Set distinct [server.acme] http_port and https_port \
                 in config.toml (the defaults are 80 and 443), or pass --http-port / --https-port."
            );
        }
        let http_addr = std::net::SocketAddr::from(([0, 0, 0, 0], http_port));
        let https_addr = std::net::SocketAddr::from(([0, 0, 0, 0], https_port));

        let email = cli
            .acme_email
            .clone()
            .unwrap_or_else(|| server.acme.email.clone());

        let cache_dir = resolve_acme_cache_dir(server.acme.cache_dir.as_deref(), config_dir)?;

        return Ok(ServerPlan::Acme {
            http_addr,
            https_addr,
            domains,
            email,
            production: server.acme.production,
            cache_dir,
        });
    }

    // ACME OFF: plain HTTP. `--listen` (repeatable, plus the deprecated `--bind`
    // alias) replaces config `listen_addrs` entirely when present.
    let listen_addrs: &[String] = if cli.listen.is_empty() {
        &server.listen_addrs
    } else {
        &cli.listen
    };

    // EMPTY listen_addrs → LOCAL MODE fallback: loopback:port (+ Tailscale).
    if listen_addrs.is_empty() {
        let port = cli.port.unwrap_or(server.port);
        // Port 0 is "pick any free port", which would leave the operator with no
        // stable URL to open. Refuse it with a named fix.
        if port == 0 {
            bail!(
                "refusing to start the local server on port 0: port 0 means \
                 \"pick any free port\", so there would be no stable address to open. \
                 Set [server] port in config.toml (the default is 8080) or pass --port \
                 with a non-zero value."
            );
        }
        let ts = if server.tailscale_enabled && !cli.no_tailscale {
            tailscale_ip
        } else {
            None
        };
        return Ok(ServerPlan::PlainHttp {
            addrs: local_addrs(port, ts),
        });
    }

    // FULL WEB MODE: parse + classify each entry. Every explicit listen_addrs
    // entry is REQUIRED — the operator named it deliberately, so a bind failure
    // there stays fatal (explicit-failure tenet). Only LOCAL MODE's auto-added
    // Tailscale leg is best-effort.
    let mut addrs: Vec<PlanAddr> = Vec::with_capacity(listen_addrs.len());
    let mut public: Vec<std::net::SocketAddr> = Vec::new();
    for raw in listen_addrs {
        let addr: std::net::SocketAddr = raw.parse().map_err(|_| {
            anyhow!(
                "invalid listen address \"{raw}\": expected IP:port, \
                 e.g. 127.0.0.1:8080 or 0.0.0.0:8080 (hostnames are not resolved)"
            )
        })?;
        // Port 0 is "pick any free port" — useless for a server the operator
        // must reach at a known address. Refuse it with the offending entry.
        if addr.port() == 0 {
            bail!(
                "refusing to bind the listen address \"{raw}\": port 0 means \
                 \"pick any free port\", so there would be no stable address to reach dux at. \
                 Use a fixed port, e.g. {ip}:8080.",
                ip = addr.ip()
            );
        }
        if !addrs.iter().any(|p| p.addr() == addr) {
            addrs.push(PlanAddr::required(addr));
        }
        // A listen entry is LOCAL when it is loopback OR equals the detected
        // Tailscale address (reachable only over the operator's own tailnet).
        let is_local = addr.ip().is_loopback() || Some(addr.ip()) == tailscale_ip;
        if !is_local && !public.contains(&addr) {
            public.push(addr);
        }
    }

    // Any public entry subjects the whole plan to the public-bind gates.
    if let Some(offender) = public.first() {
        let allow_remote = cli.insecure_allow_remote || server.insecure_allow_remote;
        let auth_ok = auth_enabled || allow_remote;
        // If the offender is in the RFC 6598 shared CGNAT range (100.64.0.0/10) —
        // which Tailscale reuses for its IPv4 tailnet addresses — or Tailscale's
        // IPv6 range (fd7a:115c:a1e0::/48), and Tailscale was NOT detected at
        // startup, the operator most likely meant a tailnet-only bind and the
        // daemon is simply down. Without detection dux can't recognize it as local
        // and treats it as public, so name that so the refusal is not mystifying.
        let offender_in_cgnat_range = match offender.ip() {
            std::net::IpAddr::V4(v4) => {
                v4.octets()[0] == 100 && (64..=127).contains(&v4.octets()[1])
            }
            std::net::IpAddr::V6(v6) => {
                let s = v6.segments();
                s[0] == 0xfd7a && s[1] == 0x115c && s[2] == 0xa1e0
            }
        };
        let tailscale_note = if tailscale_ip.is_none() && offender_in_cgnat_range {
            " Note: this address is in the RFC 6598 shared CGNAT range (100.64.0.0/10), or \
             Tailscale's IPv6 range — Tailscale reuses these for tailnet addresses. The \
             Tailscale daemon was not detected at startup, so dux is treating it as a public \
             bind. If you meant a tailnet-only bind, ensure the Tailscale daemon is running \
             so dux can classify it as local; if this is a carrier-grade-NAT address from \
             your ISP, it is genuinely reachable beyond this host."
        } else {
            ""
        };
        // The "acknowledge unencrypted plain HTTP" escape exists as BOTH a CLI flag
        // and a config field, mirroring `insecure_allow_remote`, so a config-only
        // rollback off [server.acme] can re-open a public plain-HTTP bind without
        // editing the service unit's CLI args.
        let listen_http = cli.dangerously_listen_http || server.dangerously_listen_http;
        match (auth_ok, listen_http) {
            (false, false) => bail!(
                "refusing to serve plain HTTP on the non-loopback listen address {offender}: \
                 it has NO login configured (anyone who can reach it could control your agents \
                 and worktrees) AND the traffic (including the login password) would be \
                 unencrypted. Add at least one user to [auth] in config.toml \
                 (or use the server-add-user palette command) so the login gate protects it, \
                 OR pass --insecure-allow-remote if an upstream auth proxy handles login; \
                 then ALSO enable built-in TLS via [server.acme], or acknowledge the \
                 unencrypted public bind with --dangerously-listen-http (or set \
                 dangerously_listen_http = true under [server] in config.toml).{tailscale_note}"
            ),
            (false, true) => bail!(
                "refusing to bind the non-loopback listen address {offender}: the dux web UI \
                 has no login configured, so anyone who can reach it can control your agents \
                 and worktrees. Add at least one user to [auth] in config.toml \
                 (or use the server-add-user palette command) so the login gate protects it. \
                 Alternatively, if an upstream auth proxy handles authentication, \
                 re-run with --insecure-allow-remote or set insecure_allow_remote = true \
                 under [server] in config.toml.{tailscale_note}"
            ),
            (true, false) => bail!(
                "refusing to serve plain HTTP on the non-loopback listen address {offender}: \
                 traffic (including the login password) would travel unencrypted. \
                 To serve encrypted, enable built-in TLS via [server.acme] (set enabled = true \
                 and configure domains). If TLS is terminated by an upstream proxy, or you \
                 accept the risk on a trusted network, re-run with --dangerously-listen-http \
                 or set dangerously_listen_http = true under [server] in config.toml to \
                 acknowledge the unencrypted public bind explicitly.{tailscale_note}"
            ),
            (true, true) => {}
        }
    }

    Ok(ServerPlan::PlainHttp { addrs })
}

/// Resolve the ACME cache directory: the configured value (env-expanded like
/// other config paths) when set, otherwise `<config-dir>/acme`. This directory
/// holds the ACME account and certificate private keys.
fn resolve_acme_cache_dir(cfg_cache_dir: Option<&str>, config_dir: &Path) -> Result<PathBuf> {
    match cfg_cache_dir.map(str::trim).filter(|s| !s.is_empty()) {
        Some(raw) => {
            let expanded = expand_path(raw).ok_or_else(|| {
                anyhow!(
                    "invalid [server.acme] cache_dir \"{raw}\": must be an absolute path \
                     (env vars and a leading ~ are expanded; relative paths and `..` \
                     traversal are rejected)"
                )
            })?;
            Ok(PathBuf::from(expanded))
        }
        None => Ok(config_dir.join("acme")),
    }
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
mod resolve_server_plan_tests {
    use std::path::{Path, PathBuf};

    use super::{
        AcmeSettings, PlanAddr, ServerCliOverrides, ServerConfig, ServerPlan, resolve_server_plan,
    };

    fn cfg_dir() -> PathBuf {
        PathBuf::from("/home/user/.config/dux")
    }

    /// Build a ServerConfig with given listen_addrs, insecure flag, ACME, and the
    /// other LOCAL MODE defaults (port 8080, tailscale on).
    fn server_listen(listen: &[&str], insecure: bool, acme: AcmeSettings) -> ServerConfig {
        ServerConfig {
            listen_addrs: listen.iter().map(|s| s.to_string()).collect(),
            insecure_allow_remote: insecure,
            acme,
            ..ServerConfig::default()
        }
    }

    /// Back-compat shim for the ACME-leg tests that don't care about the plain
    /// listener shape: builds a config with empty listen_addrs (local mode) and a
    /// given insecure flag + ACME settings. The old `bind` arg is ignored — ACME
    /// always binds 0.0.0.0 regardless.
    fn server(_bind: &str, insecure: bool, acme: AcmeSettings) -> ServerConfig {
        ServerConfig {
            insecure_allow_remote: insecure,
            acme,
            ..ServerConfig::default()
        }
    }

    fn acme_on(domains: &[&str]) -> AcmeSettings {
        AcmeSettings {
            enabled: true,
            domains: domains.iter().map(|s| s.to_string()).collect(),
            ..AcmeSettings::default()
        }
    }

    fn resolve(
        cfg: &ServerConfig,
        auth_enabled: bool,
        auth_disabled: bool,
        cli: ServerCliOverrides,
    ) -> anyhow::Result<ServerPlan> {
        resolve_server_plan(cfg, auth_enabled, auth_disabled, &cli, None, &cfg_dir())
    }

    fn resolve_ts(
        cfg: &ServerConfig,
        auth_enabled: bool,
        auth_disabled: bool,
        cli: ServerCliOverrides,
        tailscale_ip: Option<std::net::IpAddr>,
    ) -> anyhow::Result<ServerPlan> {
        resolve_server_plan(
            cfg,
            auth_enabled,
            auth_disabled,
            &cli,
            tailscale_ip,
            &cfg_dir(),
        )
    }

    /// FULL WEB MODE expectation: every address is a REQUIRED listener (an
    /// explicit `listen_addrs` entry the operator named, so a bind failure is
    /// fatal).
    fn plain(addrs: &[&str]) -> ServerPlan {
        ServerPlan::PlainHttp {
            addrs: addrs
                .iter()
                .map(|s| PlanAddr::required(s.parse().unwrap()))
                .collect(),
        }
    }

    /// LOCAL MODE expectation: loopback is REQUIRED, the auto-added Tailscale leg
    /// is BEST-EFFORT. Pass the loopback address(es) first and the Tailscale
    /// address (if any) as the best-effort tail.
    fn plain_local(required: &[&str], best_effort: &[&str]) -> ServerPlan {
        let mut addrs: Vec<PlanAddr> = required
            .iter()
            .map(|s| PlanAddr::required(s.parse().unwrap()))
            .collect();
        addrs.extend(
            best_effort
                .iter()
                .map(|s| PlanAddr::best_effort(s.parse().unwrap())),
        );
        ServerPlan::PlainHttp { addrs }
    }

    // ── LOCAL MODE (empty listen_addrs) ───────────────────────────────────

    #[test]
    fn local_mode_loopback_only_when_no_tailscale() {
        // Empty listen_addrs + no detected Tailscale + no flags → loopback:port.
        // This is the flip's and the fallback's safe default, identical
        // regardless of the dangerously flag.
        let cfg = server_listen(&[], false, AcmeSettings::default());
        let plan = resolve(&cfg, false, false, ServerCliOverrides::default())
            .expect("local mode loopback ok");
        assert_eq!(plan, plain(&["127.0.0.1:8080"]));
    }

    #[test]
    fn local_mode_includes_tailscale_when_detected() {
        // LOCAL MODE: loopback is REQUIRED, the detected Tailscale leg is added as
        // BEST-EFFORT (a busy Tailscale port degrades to loopback + a warning
        // rather than failing the serve).
        let cfg = server_listen(&[], false, AcmeSettings::default());
        let ts = "100.101.102.103".parse().unwrap();
        let plan = resolve_ts(&cfg, false, false, ServerCliOverrides::default(), Some(ts))
            .expect("local mode + tailscale ok");
        assert_eq!(
            plan,
            plain_local(&["127.0.0.1:8080"], &["100.101.102.103:8080"])
        );
    }

    #[test]
    fn local_mode_drops_tailscale_when_disabled_in_config() {
        let mut cfg = server_listen(&[], false, AcmeSettings::default());
        cfg.tailscale_enabled = false;
        let ts = "100.101.102.103".parse().unwrap();
        let plan = resolve_ts(&cfg, false, false, ServerCliOverrides::default(), Some(ts))
            .expect("tailscale disabled → loopback only");
        assert_eq!(plan, plain(&["127.0.0.1:8080"]));
    }

    #[test]
    fn local_mode_drops_tailscale_when_no_tailscale_flag() {
        let cfg = server_listen(&[], false, AcmeSettings::default());
        let ts = "100.101.102.103".parse().unwrap();
        let cli = ServerCliOverrides {
            no_tailscale: true,
            ..ServerCliOverrides::default()
        };
        let plan =
            resolve_ts(&cfg, false, false, cli, Some(ts)).expect("--no-tailscale → loopback only");
        assert_eq!(plan, plain(&["127.0.0.1:8080"]));
    }

    #[test]
    fn local_mode_cli_port_overrides_config_port() {
        let cfg = server_listen(&[], false, AcmeSettings::default());
        let cli = ServerCliOverrides {
            port: Some(9090),
            ..ServerCliOverrides::default()
        };
        let plan = resolve(&cfg, false, false, cli).expect("cli port override ok");
        assert_eq!(plan, plain(&["127.0.0.1:9090"]));
    }

    // ── FULL WEB MODE (non-empty listen_addrs) matrix ─────────────────────

    #[test]
    fn listen_loopback_passes_with_no_flags() {
        let cfg = server_listen(&["127.0.0.1:8080"], false, AcmeSettings::default());
        let plan = resolve(&cfg, false, false, ServerCliOverrides::default()).expect("loopback ok");
        assert_eq!(plan, plain(&["127.0.0.1:8080"]));
    }

    #[test]
    fn listen_loopback_unaffected_by_dangerously_flag() {
        let cfg = server_listen(&["127.0.0.1:8080"], false, AcmeSettings::default());
        let plan = resolve(&cfg, false, false, ServerCliOverrides::default())
            .expect("loopback resolves regardless of the scary flag");
        assert_eq!(plan, plain(&["127.0.0.1:8080"]));
    }

    #[test]
    fn listen_tailscale_entry_classified_local() {
        // An explicit listen entry equal to the detected Tailscale IP is LOCAL,
        // so it needs none of the public-bind gates.
        let cfg = server_listen(&["100.64.0.1:8080"], false, AcmeSettings::default());
        let ts = "100.64.0.1".parse().unwrap();
        let plan = resolve_ts(&cfg, false, false, ServerCliOverrides::default(), Some(ts))
            .expect("tailscale listen entry is local");
        assert_eq!(plan, plain(&["100.64.0.1:8080"]));
    }

    #[test]
    fn listen_mixed_local_and_public_refuses_naming_the_public_entry() {
        // Loopback + a public entry: the public one drives the gates, and the
        // refusal must NAME it (not the loopback one).
        let cfg = server_listen(
            &["127.0.0.1:8080", "0.0.0.0:9000"],
            false,
            AcmeSettings::default(),
        );
        let err = resolve(&cfg, false, false, ServerCliOverrides::default())
            .expect_err("a public entry must trigger the gates");
        let msg = err.to_string();
        assert!(
            msg.contains("0.0.0.0:9000"),
            "should name the offender: {msg}"
        );
        assert!(
            !msg.contains("127.0.0.1:8080"),
            "should not name the local entry: {msg}"
        );
    }

    #[test]
    fn listen_public_no_auth_no_flags_refuses_naming_both_fixes() {
        // T1-review obligation 4: when BOTH the auth leg and the scary flag are
        // missing, a single message names both fixes (no two-step convergence).
        let cfg = server_listen(&["0.0.0.0:8080"], false, AcmeSettings::default());
        let err = resolve(&cfg, false, false, ServerCliOverrides::default())
            .expect_err("public plain HTTP with no auth + no flag must refuse");
        let msg = err.to_string();
        assert!(msg.contains("[auth]"), "should name auth as a fix: {msg}");
        assert!(
            msg.contains("--insecure-allow-remote"),
            "should mention the insecure flag: {msg}"
        );
        assert!(
            msg.contains("--dangerously-listen-http"),
            "should ALSO name the unencrypted opt-in in the same message: {msg}"
        );
    }

    #[test]
    fn listen_public_auth_on_without_dangerously_refuses_naming_the_flag() {
        let cfg = server_listen(&["0.0.0.0:8080"], false, AcmeSettings::default());
        let err = resolve(&cfg, true, false, ServerCliOverrides::default())
            .expect_err("public plain HTTP needs --dangerously-listen-http even with auth");
        let msg = err.to_string();
        assert!(
            msg.contains("--dangerously-listen-http"),
            "should name the unencrypted opt-in: {msg}"
        );
        assert!(
            msg.contains("[server.acme]"),
            "should point at the TLS alternative: {msg}"
        );
    }

    #[test]
    fn listen_public_no_auth_with_dangerously_refuses_naming_auth() {
        // The scary flag clears the encryption leg, but auth is still missing.
        let cfg = server_listen(&["0.0.0.0:8080"], false, AcmeSettings::default());
        let cli = ServerCliOverrides {
            dangerously_listen_http: true,
            ..ServerCliOverrides::default()
        };
        let err = resolve(&cfg, false, false, cli)
            .expect_err("dangerously alone is not enough without auth");
        let msg = err.to_string();
        assert!(msg.contains("[auth]"), "should name auth as the fix: {msg}");
        assert!(
            msg.contains("--insecure-allow-remote"),
            "should mention the insecure flag: {msg}"
        );
    }

    #[test]
    fn listen_public_auth_on_with_dangerously_passes() {
        let cfg = server_listen(&["0.0.0.0:8080"], false, AcmeSettings::default());
        let cli = ServerCliOverrides {
            dangerously_listen_http: true,
            ..ServerCliOverrides::default()
        };
        let plan = resolve(&cfg, true, false, cli).expect("auth + dangerously ok");
        assert_eq!(plan, plain(&["0.0.0.0:8080"]));
    }

    #[test]
    fn listen_public_auth_on_with_config_dangerously_passes() {
        // The config-file equivalent of --dangerously-listen-http must ALSO satisfy
        // the gate (mirroring insecure_allow_remote), so a config-only rollback off
        // [server.acme] can re-open a public plain-HTTP bind without editing the
        // service's CLI args.
        let cfg = ServerConfig {
            dangerously_listen_http: true,
            ..server_listen(&["0.0.0.0:8080"], false, AcmeSettings::default())
        };
        let plan = resolve(&cfg, true, false, ServerCliOverrides::default())
            .expect("auth + config dangerously_listen_http ok");
        assert_eq!(plan, plain(&["0.0.0.0:8080"]));
    }

    #[test]
    fn listen_public_no_auth_with_config_dangerously_still_refuses() {
        // The config field only satisfies the ENCRYPTION half of the gate. With no
        // auth (no users, no insecure_allow_remote), a public bind must STILL be
        // refused even when dangerously_listen_http=true is set in config — the
        // field must not become an auth bypass.
        let cfg = ServerConfig {
            dangerously_listen_http: true,
            ..server_listen(&["0.0.0.0:8080"], false, AcmeSettings::default())
        };
        let err = resolve(&cfg, false, false, ServerCliOverrides::default())
            .expect_err("config dangerously_listen_http without auth must still refuse");
        assert!(
            err.to_string().contains("[auth]"),
            "the refusal must still name auth as the fix: {err}"
        );
    }

    #[test]
    fn listen_public_insecure_with_dangerously_passes() {
        let cfg = server_listen(&["0.0.0.0:8080"], true, AcmeSettings::default());
        let cli = ServerCliOverrides {
            dangerously_listen_http: true,
            ..ServerCliOverrides::default()
        };
        let plan = resolve(&cfg, false, false, cli).expect("insecure + dangerously ok");
        assert_eq!(plan, plain(&["0.0.0.0:8080"]));
    }

    #[test]
    fn listen_public_insecure_without_dangerously_still_refuses() {
        let cfg = server_listen(&["0.0.0.0:8080"], true, AcmeSettings::default());
        let err = resolve(&cfg, false, false, ServerCliOverrides::default())
            .expect_err("insecure alone is not enough for public plain HTTP");
        assert!(
            err.to_string().contains("--dangerously-listen-http"),
            "should still demand the unencrypted opt-in: {err}"
        );
    }

    #[test]
    fn listen_cli_overrides_config_listen_addrs() {
        // --listen replaces config listen_addrs entirely.
        let cfg = server_listen(&["0.0.0.0:8080"], false, AcmeSettings::default());
        let cli = ServerCliOverrides {
            listen: vec!["127.0.0.1:9999".to_string()],
            ..ServerCliOverrides::default()
        };
        let plan = resolve(&cfg, false, false, cli).expect("cli listen override ok");
        assert_eq!(plan, plain(&["127.0.0.1:9999"]));
    }

    #[test]
    fn listen_dedups_repeated_entries() {
        let cfg = server_listen(
            &["127.0.0.1:8080", "127.0.0.1:8080"],
            false,
            AcmeSettings::default(),
        );
        let plan = resolve(&cfg, false, false, ServerCliOverrides::default()).expect("dedup ok");
        assert_eq!(plan, plain(&["127.0.0.1:8080"]));
    }

    #[test]
    fn listen_invalid_entry_errors_with_shape() {
        let cfg = server_listen(&["not-an-addr"], false, AcmeSettings::default());
        let err = resolve(&cfg, true, false, ServerCliOverrides::default())
            .expect_err("invalid listen entry must error");
        let msg = err.to_string();
        assert!(msg.contains("not-an-addr"), "should name the value: {msg}");
        assert!(msg.contains("IP:port"), "should explain the shape: {msg}");
    }

    #[test]
    fn listen_hostname_entry_rejected_no_dns() {
        let cfg = server_listen(&["dux.local:8080"], false, AcmeSettings::default());
        let err = resolve(&cfg, true, false, ServerCliOverrides::default())
            .expect_err("hostnames are not resolved");
        assert!(
            err.to_string().contains("dux.local:8080"),
            "should name the bad entry: {err}"
        );
    }

    #[test]
    fn no_acme_flag_forces_plain_path_from_acme_config() {
        // Config enables ACME, but --no-acme forces the plain-HTTP path → here
        // empty listen_addrs means LOCAL MODE (loopback).
        let cfg = server_listen(&[], false, acme_on(&["dux.example.com"]));
        let cli = ServerCliOverrides {
            no_acme: true,
            ..ServerCliOverrides::default()
        };
        let plan = resolve(&cfg, false, false, cli).expect("--no-acme falls back to plain");
        assert_eq!(plan, plain(&["127.0.0.1:8080"]));
    }

    // ── ACME (on) matrix ──────────────────────────────────────────────────

    #[test]
    fn acme_on_no_domains_refuses_naming_the_fix() {
        let cfg = server("127.0.0.1:8080", false, acme_on(&[]));
        let err = resolve(&cfg, true, false, ServerCliOverrides::default())
            .expect_err("ACME with no domains must refuse");
        let msg = err.to_string();
        assert!(msg.contains("domains"), "should name domains: {msg}");
        assert!(
            msg.contains("--acme-domain"),
            "should mention the CLI override: {msg}"
        );
    }

    #[test]
    fn acme_on_with_domains_but_no_auth_refuses_naming_auth_and_disable() {
        let cfg = server("127.0.0.1:8080", false, acme_on(&["dux.example.com"]));
        let err = resolve(&cfg, false, false, ServerCliOverrides::default())
            .expect_err("ACME with no auth and not explicitly disabled must refuse");
        let msg = err.to_string();
        assert!(msg.contains("[auth]"), "should name auth: {msg}");
        assert!(
            msg.contains("--disable-auth"),
            "should offer the explicit-disable escape hatch: {msg}"
        );
    }

    #[test]
    fn acme_on_with_auth_enabled_produces_plan() {
        let cfg = server("127.0.0.1:8080", false, acme_on(&["dux.example.com"]));
        let plan = resolve(&cfg, true, false, ServerCliOverrides::default()).expect("acme ok");
        assert_eq!(
            plan,
            ServerPlan::Acme {
                http_addr: "0.0.0.0:80".parse().unwrap(),
                https_addr: "0.0.0.0:443".parse().unwrap(),
                domains: vec!["dux.example.com".to_string()],
                email: String::new(),
                production: true,
                cache_dir: Path::new("/home/user/.config/dux/acme").to_path_buf(),
            }
        );
    }

    #[test]
    fn acme_on_with_explicit_disable_auth_is_allowed() {
        // auth_enabled is false (the gate is off), but it was DELIBERATELY
        // disabled — that counts as "named explicitly" for the proxy-auth case.
        let cfg = server("127.0.0.1:8080", false, acme_on(&["dux.example.com"]));
        let plan = resolve(&cfg, false, true, ServerCliOverrides::default())
            .expect("explicit --disable-auth satisfies the ACME gate");
        match plan {
            ServerPlan::Acme { domains, .. } => {
                assert_eq!(domains, vec!["dux.example.com".to_string()]);
            }
            other => panic!("expected Acme plan, got {other:?}"),
        }
    }

    #[test]
    fn acme_cli_domains_email_and_ports_override_config() {
        let cfg = server(
            "127.0.0.1:8080",
            false,
            AcmeSettings {
                enabled: true,
                domains: vec!["config.example.com".to_string()],
                email: "config@example.com".to_string(),
                http_port: 80,
                https_port: 443,
                production: false,
                cache_dir: None,
            },
        );
        let cli = ServerCliOverrides {
            acme_domains: vec!["cli.example.com".to_string()],
            acme_email: Some("cli@example.com".to_string()),
            http_port: Some(8080),
            https_port: Some(8443),
            ..ServerCliOverrides::default()
        };
        let plan = resolve(&cfg, true, false, cli).expect("acme cli overrides ok");
        assert_eq!(
            plan,
            ServerPlan::Acme {
                http_addr: "0.0.0.0:8080".parse().unwrap(),
                https_addr: "0.0.0.0:8443".parse().unwrap(),
                domains: vec!["cli.example.com".to_string()],
                email: "cli@example.com".to_string(),
                production: false,
                cache_dir: Path::new("/home/user/.config/dux/acme").to_path_buf(),
            }
        );
    }

    #[test]
    fn acme_cache_dir_from_config_is_used_when_absolute() {
        let cfg = server(
            "127.0.0.1:8080",
            false,
            AcmeSettings {
                enabled: true,
                domains: vec!["dux.example.com".to_string()],
                cache_dir: Some("/var/lib/dux/acme".to_string()),
                ..AcmeSettings::default()
            },
        );
        let plan = resolve(&cfg, true, false, ServerCliOverrides::default()).expect("acme ok");
        match plan {
            ServerPlan::Acme { cache_dir, .. } => {
                assert_eq!(cache_dir, Path::new("/var/lib/dux/acme"));
            }
            other => panic!("expected Acme plan, got {other:?}"),
        }
    }

    #[test]
    fn acme_cache_dir_relative_value_is_rejected() {
        let cfg = server(
            "127.0.0.1:8080",
            false,
            AcmeSettings {
                enabled: true,
                domains: vec!["dux.example.com".to_string()],
                cache_dir: Some("relative/acme".to_string()),
                ..AcmeSettings::default()
            },
        );
        let err = resolve(&cfg, true, false, ServerCliOverrides::default())
            .expect_err("relative cache_dir must be rejected");
        assert!(
            err.to_string().contains("cache_dir"),
            "error should name the bad field: {err}"
        );
    }

    #[test]
    fn acme_on_ignores_plain_bind_rules() {
        // ACME binds 0.0.0.0:80/443 regardless of listen_addrs; the plain-HTTP
        // non-loopback gate does not apply to the ACME path.
        let cfg = server_listen(&["0.0.0.0:8080"], false, acme_on(&["dux.example.com"]));
        let plan = resolve(&cfg, true, false, ServerCliOverrides::default())
            .expect("acme path ignores the plain bind gate");
        match plan {
            ServerPlan::Acme {
                http_addr,
                https_addr,
                ..
            } => {
                assert_eq!(http_addr.to_string(), "0.0.0.0:80");
                assert_eq!(https_addr.to_string(), "0.0.0.0:443");
            }
            other => panic!("expected Acme plan, got {other:?}"),
        }
    }

    // ── Obligation 2: port 0 / http==https collision refusals ─────────────

    #[test]
    fn acme_rejects_http_port_zero() {
        let mut acme = acme_on(&["dux.example.com"]);
        acme.http_port = 0;
        let cfg = server("", false, acme);
        let err = resolve(&cfg, true, false, ServerCliOverrides::default())
            .expect_err("port 0 must be refused for ACME");
        assert!(err.to_string().contains("port 0"), "names the cause: {err}");
    }

    #[test]
    fn acme_rejects_https_port_zero() {
        let mut acme = acme_on(&["dux.example.com"]);
        acme.https_port = 0;
        let cfg = server("", false, acme);
        let err = resolve(&cfg, true, false, ServerCliOverrides::default())
            .expect_err("https port 0 must be refused for ACME");
        assert!(err.to_string().contains("port 0"), "names the cause: {err}");
    }

    #[test]
    fn acme_rejects_equal_http_and_https_ports() {
        let mut acme = acme_on(&["dux.example.com"]);
        acme.http_port = 8443;
        acme.https_port = 8443;
        let cfg = server("", false, acme);
        let err = resolve(&cfg, true, false, ServerCliOverrides::default())
            .expect_err("identical http/https ports must be refused");
        assert!(
            err.to_string().contains("must differ"),
            "names the collision: {err}"
        );
    }

    #[test]
    fn acme_port_collision_via_cli_overrides_is_refused() {
        // The collision check sees the CLI-resolved ports, not just config.
        let cfg = server("", false, acme_on(&["dux.example.com"]));
        let cli = ServerCliOverrides {
            http_port: Some(9000),
            https_port: Some(9000),
            ..ServerCliOverrides::default()
        };
        let err = resolve(&cfg, true, false, cli)
            .expect_err("CLI-induced port collision must be refused");
        assert!(err.to_string().contains("must differ"), "names it: {err}");
    }

    #[test]
    fn acme_rejects_http_port_zero_via_cli_override() {
        // A non-zero config http_port overridden to 0 on the CLI must still be
        // refused — the resolver checks the CLI-resolved port, not just config.
        let mut acme = acme_on(&["dux.example.com"]);
        acme.http_port = 80;
        let cfg = server("", false, acme);
        let cli = ServerCliOverrides {
            http_port: Some(0),
            ..ServerCliOverrides::default()
        };
        let err =
            resolve(&cfg, true, false, cli).expect_err("--http-port 0 must be refused for ACME");
        assert!(err.to_string().contains("port 0"), "names the cause: {err}");
    }

    #[test]
    fn acme_rejects_https_port_zero_via_cli_override() {
        let mut acme = acme_on(&["dux.example.com"]);
        acme.https_port = 443;
        let cfg = server("", false, acme);
        let cli = ServerCliOverrides {
            https_port: Some(0),
            ..ServerCliOverrides::default()
        };
        let err =
            resolve(&cfg, true, false, cli).expect_err("--https-port 0 must be refused for ACME");
        assert!(err.to_string().contains("port 0"), "names the cause: {err}");
    }

    #[test]
    fn local_mode_rejects_port_zero() {
        let mut cfg = server_listen(&[], false, AcmeSettings::default());
        cfg.port = 0;
        let err = resolve(&cfg, false, false, ServerCliOverrides::default())
            .expect_err("local-mode port 0 must be refused");
        assert!(err.to_string().contains("port 0"), "names it: {err}");
    }

    #[test]
    fn local_mode_rejects_port_zero_via_cli_override() {
        // A healthy config port overridden to 0 on the CLI (--port 0) must be
        // refused: the CLI value wins via `unwrap_or`, so the zero must be caught.
        let mut cfg = server_listen(&[], false, AcmeSettings::default());
        cfg.port = 8080;
        let cli = ServerCliOverrides {
            port: Some(0),
            ..ServerCliOverrides::default()
        };
        let err = resolve(&cfg, false, false, cli)
            .expect_err("--port 0 must be refused for the local server");
        assert!(err.to_string().contains("port 0"), "names it: {err}");
    }

    #[test]
    fn listen_addrs_rejects_port_zero_entry() {
        let cfg = server_listen(&["127.0.0.1:0"], false, AcmeSettings::default());
        let err = resolve(&cfg, false, false, ServerCliOverrides::default())
            .expect_err("a :0 listen entry must be refused");
        assert!(
            err.to_string().contains("port 0") && err.to_string().contains("127.0.0.1:0"),
            "names the offending entry and the cause: {err}"
        );
    }
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
        // safe LOCAL MODE defaults (port 8080, Tailscale opt-out on, no listeners).
        let config: Config = toml::from_str("").expect("empty config should parse");
        assert_eq!(config.server.port, 8080);
        assert!(config.server.tailscale_enabled);
        assert!(config.server.listen_addrs.is_empty());
        assert!(config.server.bind.is_none());
        assert!(!config.server.insecure_allow_remote);
    }

    #[test]
    fn server_config_parses_full_section() {
        let config: Config = toml::from_str(
            r#"
[server]
port = 9000
tailscale_enabled = false
listen_addrs = ["0.0.0.0:9000"]
insecure_allow_remote = true
"#,
        )
        .expect("config with full [server] should parse");
        assert_eq!(config.server.port, 9000);
        assert!(!config.server.tailscale_enabled);
        assert_eq!(config.server.listen_addrs, vec!["0.0.0.0:9000".to_string()]);
        assert!(config.server.insecure_allow_remote);
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
        assert_eq!(config.server.port, 9000);
        assert!(config.server.tailscale_enabled);
        assert!(config.server.listen_addrs.is_empty());
        assert!(!config.server.insecure_allow_remote);
    }

    #[test]
    fn server_config_deprecated_bind_still_parses() {
        // Old configs that only set `bind` must still deserialize (serde keeps
        // the field); the TUI deprecation machinery migrates it to port /
        // listen_addrs on load. Here we just prove the raw struct still parses.
        let config: Config = toml::from_str(
            r#"
[server]
bind = "0.0.0.0:9000"
"#,
        )
        .expect("config with deprecated [server] bind should parse");
        assert_eq!(config.server.bind.as_deref(), Some("0.0.0.0:9000"));
        // The new fields fall back to defaults until migration runs.
        assert_eq!(config.server.port, 8080);
        assert!(config.server.listen_addrs.is_empty());
    }

    #[test]
    fn acme_config_defaults_when_section_absent() {
        // An old config with [server] but no [server.acme] must parse into the
        // safe ACME-off defaults.
        let config: Config = toml::from_str(
            r#"
[server]
bind = "0.0.0.0:9000"
"#,
        )
        .expect("config without [server.acme] should parse");
        assert!(!config.server.acme.enabled);
        assert!(config.server.acme.domains.is_empty());
        assert_eq!(config.server.acme.email, "");
        assert_eq!(config.server.acme.http_port, 80);
        assert_eq!(config.server.acme.https_port, 443);
        assert!(config.server.acme.production);
        assert!(config.server.acme.cache_dir.is_none());
    }

    #[test]
    fn acme_config_parses_full_section() {
        let config: Config = toml::from_str(
            r#"
[server]
bind = "0.0.0.0:9000"

[server.acme]
enabled = true
domains = ["dux.example.com", "www.example.com"]
email = "ops@example.com"
http_port = 8080
https_port = 8443
production = false
cache_dir = "/var/lib/dux/acme"
"#,
        )
        .expect("config with full [server.acme] should parse");
        assert!(config.server.acme.enabled);
        assert_eq!(
            config.server.acme.domains,
            vec!["dux.example.com".to_string(), "www.example.com".to_string()]
        );
        assert_eq!(config.server.acme.email, "ops@example.com");
        assert_eq!(config.server.acme.http_port, 8080);
        assert_eq!(config.server.acme.https_port, 8443);
        assert!(!config.server.acme.production);
        assert_eq!(
            config.server.acme.cache_dir.as_deref(),
            Some("/var/lib/dux/acme")
        );
    }

    #[test]
    fn acme_config_partial_section_defaults_remaining_fields() {
        // Only `enabled` and `domains` are provided; the rest default.
        let config: Config = toml::from_str(
            r#"
[server.acme]
enabled = true
domains = ["dux.example.com"]
"#,
        )
        .expect("config with partial [server.acme] should parse");
        assert!(config.server.acme.enabled);
        assert_eq!(
            config.server.acme.domains,
            vec!["dux.example.com".to_string()]
        );
        assert_eq!(config.server.acme.http_port, 80);
        assert_eq!(config.server.acme.https_port, 443);
        assert!(config.server.acme.production);
        assert!(config.server.acme.cache_dir.is_none());
    }

    #[test]
    fn auth_config_defaults_when_section_absent() {
        // A config TOML with no [auth] section must still parse and yield an
        // empty user list (auth off).
        let config: Config = toml::from_str("").expect("empty config should parse");
        assert!(config.auth.users.is_empty());
    }

    #[test]
    fn auth_config_parses_user_entries() {
        let config: Config = toml::from_str(
            r#"
[auth]
users = ["alice:$2y$12$abc", "bob:$2y$12$def"]
"#,
        )
        .expect("config with [auth] users should parse");
        assert_eq!(
            config.auth.users,
            vec!["alice:$2y$12$abc".to_string(), "bob:$2y$12$def".to_string()]
        );
    }

    #[test]
    fn auth_config_empty_users_array_parses() {
        // The canonical first-boot value `users = []` must parse cleanly.
        let config: Config = toml::from_str(
            r#"
[auth]
users = []
"#,
        )
        .expect("config with empty [auth] users should parse");
        assert!(config.auth.users.is_empty());
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
}
