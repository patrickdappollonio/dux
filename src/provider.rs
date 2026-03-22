use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;

use crate::config::{OneshotOutput, ProviderCommandConfig};

/// A config-driven provider that builds oneshot commands from templates.
pub struct GenericProvider {
    pub name: String,
    pub config: ProviderCommandConfig,
}

impl GenericProvider {
    pub fn command(&self) -> &str {
        &self.config.command
    }

    /// Build a [`Command`] for a non-interactive one-shot prompt.
    ///
    /// Substitutes `{prompt}` and `{tempfile}` placeholders in `oneshot_args`.
    pub fn build_oneshot_command(&self, prompt: &str, cwd: &Path) -> (Command, Option<PathBuf>) {
        let unique: u64 = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let tmpfile = std::env::temp_dir().join(format!(
            "dux-{}-{}-{}.txt",
            self.name,
            std::process::id(),
            unique
        ));
        let mut cmd = Command::new(&self.config.command);
        cmd.current_dir(cwd);
        for arg_template in &self.config.oneshot_args {
            let arg = arg_template
                .replace("{prompt}", prompt)
                .replace("{tempfile}", &tmpfile.to_string_lossy());
            cmd.arg(arg);
        }
        let tmpfile_option = match self.config.oneshot_output {
            OneshotOutput::Tempfile => Some(tmpfile),
            OneshotOutput::Stdout => None,
        };
        (cmd, tmpfile_option)
    }

    /// Run a non-interactive prompt and return the response text.
    pub fn run_oneshot(&self, prompt: &str, cwd: &Path) -> Result<String> {
        let (mut cmd, tmpfile) = self.build_oneshot_command(prompt, cwd);
        let output = cmd.output()?;
        if !output.status.success() {
            anyhow::bail!(
                "{} failed: {}",
                self.command(),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        if let Some(path) = tmpfile {
            let text = std::fs::read_to_string(&path)?;
            let _ = std::fs::remove_file(&path);
            Ok(text.trim().to_string())
        } else {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        }
    }
}

/// Create a [`GenericProvider`] from a provider name and its config.
pub fn create_provider(name: &str, config: ProviderCommandConfig) -> GenericProvider {
    GenericProvider {
        name: name.to_string(),
        config,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude_like_config() -> ProviderCommandConfig {
        ProviderCommandConfig {
            command: "echo".to_string(),
            args: Vec::new(),
            oneshot_args: vec!["-p".to_string(), "{prompt}".to_string()],
            oneshot_output: OneshotOutput::Stdout,
            install_hint: None,
        }
    }

    fn codex_like_config() -> ProviderCommandConfig {
        ProviderCommandConfig {
            command: "bash".to_string(),
            args: Vec::new(),
            oneshot_args: vec!["-c".to_string(), "echo {prompt} > {tempfile}".to_string()],
            oneshot_output: OneshotOutput::Tempfile,
            install_hint: None,
        }
    }

    #[test]
    fn claude_style_stdout_build_command() {
        let prov = create_provider("claude", claude_like_config());
        let cwd = std::env::temp_dir();
        let (cmd, tmpfile) = prov.build_oneshot_command("hello world", &cwd);
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(args, vec!["-p", "hello world"]);
        assert!(
            tmpfile.is_none(),
            "stdout mode should not produce a tempfile"
        );
    }

    #[test]
    fn codex_style_tempfile_build_command() {
        let prov = create_provider("codex", codex_like_config());
        let cwd = std::env::temp_dir();
        let (cmd, tmpfile) = prov.build_oneshot_command("hello world", &cwd);
        let args: Vec<_> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert!(
            tmpfile.is_some(),
            "tempfile mode should produce a tempfile path"
        );
        let tf = tmpfile.unwrap();
        assert_eq!(args[0], "-c");
        assert!(
            args[1].contains("hello world"),
            "prompt should be substituted"
        );
        assert!(
            args[1].contains(&tf.to_string_lossy().to_string()),
            "tempfile path should be substituted"
        );
    }

    #[test]
    fn claude_style_run_oneshot() {
        let prov = create_provider("claude", claude_like_config());
        let cwd = std::env::temp_dir();
        let result = prov.run_oneshot("hello world", &cwd).unwrap();
        // echo prints: -p hello world
        assert_eq!(result, "-p hello world");
    }

    #[test]
    fn codex_style_run_oneshot_via_tempfile() {
        let prov = create_provider("codex", codex_like_config());
        let cwd = std::env::temp_dir();
        let result = prov.run_oneshot("hello world", &cwd).unwrap();
        assert_eq!(result, "hello world");
    }

    #[test]
    fn run_oneshot_reports_failure() {
        let config = ProviderCommandConfig {
            command: "false".to_string(),
            args: Vec::new(),
            oneshot_args: Vec::new(),
            oneshot_output: OneshotOutput::Stdout,
            install_hint: None,
        };
        let prov = create_provider("bad", config);
        let cwd = std::env::temp_dir();
        let result = prov.run_oneshot("anything", &cwd);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("failed"));
    }

    #[test]
    fn custom_provider_with_extra_args() {
        let config = ProviderCommandConfig {
            command: "echo".to_string(),
            args: Vec::new(),
            oneshot_args: vec![
                "--format".to_string(),
                "plain".to_string(),
                "{prompt}".to_string(),
            ],
            oneshot_output: OneshotOutput::Stdout,
            install_hint: None,
        };
        let prov = create_provider("custom", config);
        let cwd = std::env::temp_dir();
        let result = prov.run_oneshot("test prompt", &cwd).unwrap();
        assert_eq!(result, "--format plain test prompt");
    }

    #[test]
    fn tempfile_cleaned_up_after_read() {
        let prov = create_provider("cleanup-test", codex_like_config());
        let cwd = std::env::temp_dir();
        // run_oneshot should create a tempfile, read it, then delete it.
        // Check that no dux-cleanup-test-* files remain afterward.
        let _ = prov.run_oneshot("check cleanup", &cwd).unwrap();
        let leftover: Vec<_> = std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("dux-cleanup-test-")
            })
            .collect();
        assert!(
            leftover.is_empty(),
            "tempfile should be cleaned up after run_oneshot, found: {leftover:?}",
        );
    }
}
