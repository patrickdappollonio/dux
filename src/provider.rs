use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;

use crate::config::ProviderCommandConfig;
use crate::model::ProviderKind;

/// Trait abstracting CLI provider capabilities beyond interactive PTY sessions.
pub trait Provider: Send {
    /// The provider kind (Claude, Codex).
    fn kind(&self) -> ProviderKind;

    /// The CLI command name (e.g. "claude", "codex").
    fn command(&self) -> &str;

    /// Build a [`Command`] for a non-interactive one-shot prompt.
    fn build_oneshot_command(&self, prompt: &str, cwd: &Path) -> (Command, Option<PathBuf>);

    /// Run a non-interactive prompt and return the response text.
    fn run_oneshot(&self, prompt: &str, cwd: &Path) -> Result<String> {
        let (mut cmd, tmpfile) = self.build_oneshot_command(prompt, cwd);
        let output = cmd.output()?;
        if let Some(path) = tmpfile {
            let text = std::fs::read_to_string(&path)?;
            let _ = std::fs::remove_file(&path);
            Ok(text.trim().to_string())
        } else {
            if !output.status.success() {
                anyhow::bail!(
                    "{} failed: {}",
                    self.command(),
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        }
    }
}

pub struct ClaudeProvider {
    pub config: ProviderCommandConfig,
}

impl Provider for ClaudeProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Claude
    }

    fn command(&self) -> &str {
        &self.config.command
    }

    fn build_oneshot_command(&self, prompt: &str, cwd: &Path) -> (Command, Option<PathBuf>) {
        let mut cmd = Command::new(&self.config.command);
        cmd.current_dir(cwd);
        cmd.args(["-p", prompt]);
        (cmd, None)
    }
}

pub struct CodexProvider {
    pub config: ProviderCommandConfig,
}

impl Provider for CodexProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Codex
    }

    fn command(&self) -> &str {
        &self.config.command
    }

    fn build_oneshot_command(&self, prompt: &str, cwd: &Path) -> (Command, Option<PathBuf>) {
        let tmpfile =
            std::env::temp_dir().join(format!("dux-codex-{}.txt", std::process::id()));
        let mut cmd = Command::new(&self.config.command);
        cmd.current_dir(cwd);
        cmd.args(["exec", "-o", &tmpfile.to_string_lossy(), prompt]);
        (cmd, Some(tmpfile))
    }
}

/// Create a boxed [`Provider`] from a [`ProviderKind`] and its config.
pub fn create_provider(
    kind: &ProviderKind,
    config: ProviderCommandConfig,
) -> Box<dyn Provider> {
    match kind {
        ProviderKind::Claude => Box::new(ClaudeProvider { config }),
        ProviderKind::Codex => Box::new(CodexProvider { config }),
    }
}
