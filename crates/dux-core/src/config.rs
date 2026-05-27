//! dux configuration data model: the serde structs persisted in `config.toml`,
//! their defaults, and value helpers. The keybinding-aware *renderer* of the
//! documented config, plus `ensure_config`/`save_config` orchestration and the
//! toml_edit patching, live in the binary's `config` module — not here.
//!
//! Note: `Config` and `KeysConfig` live in the binary's `config` module rather
//! than here because their `Default` impls depend on `keybindings::BINDING_DEFS`,
//! a binary-only symbol.

use std::collections::BTreeMap;
use std::env;

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
