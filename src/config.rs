use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

use crate::model::ProviderKind;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub defaults: Defaults,
    pub shell: ShellConfig,
    pub providers: ProvidersConfig,
    pub logging: LoggingConfig,
    pub projects: Vec<ProjectConfig>,
    pub ui: UiConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Defaults {
    pub provider: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellConfig {
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ProvidersConfig {
    pub claude: ProviderCommandConfig,
    pub codex: ProviderCommandConfig,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
    pub path: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderCommandConfig {
    pub command: String,
    pub args: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub path: String,
    pub name: Option<String>,
    pub default_provider: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct UiConfig {
    pub left_width_pct: u16,
    pub right_width_pct: u16,
    pub right_top_height_pct: u16,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            defaults: Defaults {
                provider: "codex".to_string(),
            },
            shell: ShellConfig {
                command: "/bin/bash".to_string(),
                args: vec!["-l".to_string()],
            },
            providers: ProvidersConfig {
                claude: ProviderCommandConfig {
                    command: "claude".to_string(),
                    args: Vec::new(),
                },
                codex: ProviderCommandConfig {
                    command: "codex".to_string(),
                    args: Vec::new(),
                },
            },
            logging: LoggingConfig {
                level: "info".to_string(),
                path: "dux.log".to_string(),
            },
            projects: Vec::new(),
            ui: UiConfig {
                left_width_pct: 24,
                right_width_pct: 28,
                right_top_height_pct: 45,
            },
        }
    }
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            provider: "codex".to_string(),
        }
    }
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            command: "/bin/bash".to_string(),
            args: vec!["-l".to_string()],
        }
    }
}

impl Default for ProvidersConfig {
    fn default() -> Self {
        Self {
            claude: ProviderCommandConfig {
                command: "claude".to_string(),
                args: Vec::new(),
            },
            codex: ProviderCommandConfig {
                command: "codex".to_string(),
                args: Vec::new(),
            },
        }
    }
}

impl Default for ProviderCommandConfig {
    fn default() -> Self {
        Self {
            command: String::new(),
            args: Vec::new(),
        }
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

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            left_width_pct: 24,
            right_width_pct: 28,
            right_top_height_pct: 45,
        }
    }
}

impl Config {
    pub fn default_provider(&self) -> ProviderKind {
        ProviderKind::from_str(&self.defaults.provider)
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
        let dirs = ProjectDirs::from("", "", "dux")
            .ok_or_else(|| anyhow!("failed to determine user config directory"))?;
        let root = dirs.config_dir().to_path_buf();
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
    let config: Config = toml::from_str(&raw)
        .with_context(|| format!("failed to parse {}", paths.config_path.display()))?;
    let _ = save_config(&paths.config_path, &config);
    Ok(config)
}

pub fn render_default_config() -> String {
    let default = Config::default();
    format!(
        r#"# dux configuration
# Every value is materialized here so the file doubles as documentation.

[defaults]
# Which provider new sessions use unless a project overrides it.
provider = "{provider}"

[shell]
# Manual terminal command for the lower-right pane.
command = "{shell_command}"
args = ["{shell_arg}"]

[providers.claude]
# Command used to launch an ACP-compatible Claude adapter.
# Example: command = "claude-agent-acp"
command = "{claude_command}"
args = []

[providers.codex]
# Command used to launch an ACP-compatible Codex adapter.
# Example: command = "codex-acp"
command = "{codex_command}"
args = []

[logging]
# Log level can be error, info, or debug.
level = "{log_level}"
# Relative paths are resolved from ~/.config/dux/.
path = "{log_path}"

[ui]
# Initial pane sizing percentages. They can still be resized at runtime.
left_width_pct = {left_width}
right_width_pct = {right_width}
right_top_height_pct = {right_top_height}

# Projects are registered here by the UI. The folder name is used when name is omitted.
# default_provider can override the global default for one project.
projects = []
"#,
        provider = default.defaults.provider,
        shell_command = default.shell.command,
        shell_arg = default.shell.args.first().cloned().unwrap_or_default(),
        claude_command = default.providers.claude.command,
        codex_command = default.providers.codex.command,
        log_level = default.logging.level,
        log_path = default.logging.path,
        left_width = default.ui.left_width_pct,
        right_width = default.ui.right_width_pct,
        right_top_height = default.ui.right_top_height_pct,
    )
}

pub fn save_config(config_path: &Path, config: &Config) -> Result<()> {
    let body = render_config(config);
    fs::write(config_path, body)
        .with_context(|| format!("failed to write {}", config_path.display()))?;
    Ok(())
}

fn render_config(config: &Config) -> String {
    let mut out = String::new();
    out.push_str("# dux configuration\n");
    out.push_str("# Every value is materialized here so the file doubles as documentation.\n\n");
    out.push_str("[defaults]\n");
    out.push_str("# Which provider new sessions use unless a project overrides it.\n");
    out.push_str(&format!("provider = \"{}\"\n\n", config.defaults.provider));
    out.push_str("[shell]\n");
    out.push_str("# Manual terminal command for the lower-right pane.\n");
    out.push_str(&format!("command = \"{}\"\n", config.shell.command));
    out.push_str(&format!(
        "args = {}\n\n",
        render_string_list(&config.shell.args)
    ));
    out.push_str("[providers.claude]\n");
    out.push_str("# Command used to launch an ACP-compatible Claude adapter.\n");
    out.push_str("# Example: command = \"claude-agent-acp\"\n");
    out.push_str(&format!(
        "command = \"{}\"\n",
        config.providers.claude.command
    ));
    out.push_str(&format!(
        "args = {}\n\n",
        render_string_list(&config.providers.claude.args)
    ));
    out.push_str("[providers.codex]\n");
    out.push_str("# Command used to launch an ACP-compatible Codex adapter.\n");
    out.push_str("# Example: command = \"codex-acp\"\n");
    out.push_str(&format!(
        "command = \"{}\"\n",
        config.providers.codex.command
    ));
    out.push_str(&format!(
        "args = {}\n\n",
        render_string_list(&config.providers.codex.args)
    ));
    out.push_str("[logging]\n");
    out.push_str("# Log level can be error, info, or debug.\n");
    out.push_str(&format!("level = \"{}\"\n", config.logging.level));
    out.push_str("# Relative paths are resolved from ~/.config/dux/.\n");
    out.push_str(&format!("path = \"{}\"\n\n", config.logging.path));
    out.push_str("[ui]\n");
    out.push_str("# Initial pane sizing percentages. They can still be resized at runtime.\n");
    out.push_str(&format!("left_width_pct = {}\n", config.ui.left_width_pct));
    out.push_str(&format!(
        "right_width_pct = {}\n",
        config.ui.right_width_pct
    ));
    out.push_str(&format!(
        "right_top_height_pct = {}\n\n",
        config.ui.right_top_height_pct
    ));
    out.push_str(
        "# Projects are registered here by the UI. The folder name is used when name is omitted.\n",
    );
    out.push_str("# default_provider can override the global default for one project.\n");
    for project in &config.projects {
        out.push_str("[[projects]]\n");
        out.push_str(&format!("path = \"{}\"\n", project.path));
        if let Some(name) = &project.name {
            out.push_str(&format!("name = \"{}\"\n", name));
        }
        if let Some(provider) = &project.default_provider {
            out.push_str(&format!("default_provider = \"{}\"\n", provider));
        }
        out.push('\n');
    }
    if config.projects.is_empty() {
        out.push_str("projects = []\n");
    }
    out
}

fn render_string_list(values: &[String]) -> String {
    let rendered = values
        .iter()
        .map(|value| format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\"")))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{rendered}]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_commented_and_complete() {
        let rendered = render_default_config();
        assert!(rendered.contains("# dux configuration"));
        assert!(rendered.contains("[defaults]"));
        assert!(rendered.contains("[providers.claude]"));
        assert!(rendered.contains("[providers.codex]"));
        assert!(rendered.contains("[ui]"));
    }
}
