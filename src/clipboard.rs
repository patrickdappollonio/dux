use anyhow::{Result, anyhow};

#[cfg(target_os = "linux")]
use std::collections::HashSet;
#[cfg(target_os = "linux")]
use std::env;
#[cfg(target_os = "linux")]
use std::ffi::OsStr;
#[cfg(target_os = "linux")]
use std::io::Write;
#[cfg(target_os = "linux")]
use std::path::Path;
#[cfg(target_os = "linux")]
use std::process::{Command, Stdio};
#[cfg(target_os = "linux")]
use std::sync::mpsc;
#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(target_os = "linux")]
use anyhow::Context;

#[cfg(target_os = "linux")]
const CLIPBOARD_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Clone, Copy)]
pub(crate) struct Clipboard {
    copy_text_fn: fn(&str) -> Result<()>,
}

impl Clipboard {
    pub(crate) fn new() -> Self {
        Self {
            copy_text_fn: copy_text_impl,
        }
    }

    pub(crate) fn copy_text(&self, text: &str) -> Result<()> {
        (self.copy_text_fn)(text)
    }

    #[cfg(test)]
    pub(crate) fn from_fn(copy_text_fn: fn(&str) -> Result<()>) -> Self {
        Self { copy_text_fn }
    }
}

fn copy_text_impl(text: &str) -> Result<()> {
    #[cfg(target_os = "linux")]
    {
        copy_text_linux(text)
    }

    #[cfg(not(target_os = "linux"))]
    {
        copy_text_arboard(text)
    }
}

#[cfg(not(target_os = "linux"))]
fn copy_text_arboard(text: &str) -> Result<()> {
    let mut clipboard =
        arboard::Clipboard::new().map_err(|e| anyhow!("Failed to access clipboard: {e}"))?;
    clipboard
        .set_text(text)
        .map_err(|e| anyhow!("Failed to copy to clipboard: {e}"))?;
    Ok(())
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LinuxClipboardBackend {
    WlCopy,
    Xclip,
    Xsel,
}

#[cfg(target_os = "linux")]
impl LinuxClipboardBackend {
    fn label(self) -> &'static str {
        match self {
            Self::WlCopy => "wl-copy",
            Self::Xclip => "xclip",
            Self::Xsel => "xsel",
        }
    }

    fn command_spec(self) -> LinuxClipboardCommand {
        match self {
            Self::WlCopy => LinuxClipboardCommand {
                program: "wl-copy",
                args: &[],
            },
            Self::Xclip => LinuxClipboardCommand {
                program: "xclip",
                args: &["-selection", "clipboard"],
            },
            Self::Xsel => LinuxClipboardCommand {
                program: "xsel",
                args: &["--clipboard", "--input"],
            },
        }
    }
}

#[cfg(target_os = "linux")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct LinuxClipboardCommand {
    program: &'static str,
    args: &'static [&'static str],
}

#[cfg(target_os = "linux")]
#[derive(Debug, Default)]
struct LinuxClipboardEnvironment {
    wayland_display: bool,
    x11_display: bool,
    executables: HashSet<String>,
}

#[cfg(target_os = "linux")]
impl LinuxClipboardEnvironment {
    fn current() -> Self {
        let executables = env::var_os("PATH")
            .map(|path| executable_names_on_path(&path))
            .unwrap_or_default();
        Self {
            wayland_display: has_env_var("WAYLAND_DISPLAY"),
            x11_display: has_env_var("DISPLAY"),
            executables,
        }
    }

    fn has_command(&self, command: &str) -> bool {
        self.executables.contains(command)
    }
}

#[cfg(target_os = "linux")]
fn has_env_var(key: &str) -> bool {
    env::var_os(key)
        .map(|value| !value.is_empty())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn executable_names_on_path(path: &OsStr) -> HashSet<String> {
    let mut names = HashSet::new();

    for dir in env::split_paths(path) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str() {
                names.insert(normalize_executable_name(name));
            }
        }
    }

    names
}

#[cfg(target_os = "linux")]
fn normalize_executable_name(value: &str) -> String {
    Path::new(value)
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or(value)
        .to_ascii_lowercase()
}

#[cfg(target_os = "linux")]
fn linux_backends(env: &LinuxClipboardEnvironment) -> Vec<LinuxClipboardBackend> {
    let mut backends = Vec::new();

    if env.wayland_display && env.has_command("wl-copy") {
        backends.push(LinuxClipboardBackend::WlCopy);
    }
    if env.x11_display && env.has_command("xclip") {
        backends.push(LinuxClipboardBackend::Xclip);
    }
    if env.x11_display && env.has_command("xsel") {
        backends.push(LinuxClipboardBackend::Xsel);
    }

    backends
}

#[cfg(target_os = "linux")]
fn copy_text_linux(text: &str) -> Result<()> {
    let env = LinuxClipboardEnvironment::current();
    let backends = linux_backends(&env);

    if backends.is_empty() {
        return Err(anyhow!(format_linux_clipboard_error(&env, &[])));
    }

    let mut errors = Vec::new();
    for backend in backends {
        match run_linux_clipboard_command(backend.command_spec(), text) {
            Ok(()) => return Ok(()),
            Err(err) => errors.push(format!("{}: {err}", backend.label())),
        }
    }

    Err(anyhow!(format_linux_clipboard_error(&env, &errors)))
}

#[cfg(target_os = "linux")]
fn run_linux_clipboard_command(command: LinuxClipboardCommand, text: &str) -> Result<()> {
    let mut child = Command::new(command.program)
        .args(command.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("Failed to launch {}", command.program))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(text.as_bytes())
            .with_context(|| format!("Failed to write to {}", command.program))?;
    }
    // stdin is dropped here, closing the pipe so the child can proceed.

    let program = command.program;
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(child.wait_with_output());
    });

    let output = rx
        .recv_timeout(CLIPBOARD_TIMEOUT)
        .map_err(|_| {
            anyhow!(
                "{program} timed out after {}ms",
                CLIPBOARD_TIMEOUT.as_millis()
            )
        })?
        .with_context(|| format!("Failed to wait for {program}"))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let detail = if stderr.is_empty() {
        format!("exited with status {}", output.status)
    } else {
        stderr
    };
    Err(anyhow!("{detail}"))
}

#[cfg(target_os = "linux")]
fn format_linux_clipboard_error(env: &LinuxClipboardEnvironment, errors: &[String]) -> String {
    let mut message = String::from("Clipboard copy failed.");

    if !errors.is_empty() {
        message.push_str(" Tried ");
        message.push_str(&errors.join("; "));
        message.push('.');
    }

    if env.wayland_display {
        message.push_str(
            " Install `wl-clipboard` (provides `wl-copy`) for Wayland clipboard support.",
        );
    } else if env.x11_display {
        message.push_str(" Install `xclip` or `xsel` for X11 clipboard support.");
    } else {
        message.push_str(
            " No display session detected (`WAYLAND_DISPLAY` and `DISPLAY` are both unset). \
             Clipboard requires a graphical session with `wl-clipboard` (Wayland) or `xclip`/`xsel` (X11) installed.",
        );
    }

    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    fn env_with(
        wayland_display: bool,
        x11_display: bool,
        executables: &[&str],
    ) -> LinuxClipboardEnvironment {
        LinuxClipboardEnvironment {
            wayland_display,
            x11_display,
            executables: executables
                .iter()
                .map(|name| (*name).to_string())
                .collect::<HashSet<_>>(),
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_prefers_wayland_before_x11_helpers() {
        let env = env_with(true, true, &["wl-copy", "xclip", "xsel"]);

        assert_eq!(
            linux_backends(&env),
            vec![
                LinuxClipboardBackend::WlCopy,
                LinuxClipboardBackend::Xclip,
                LinuxClipboardBackend::Xsel,
            ]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_uses_x11_helpers_when_wayland_is_unavailable() {
        let env = env_with(false, true, &["xclip", "xsel"]);

        assert_eq!(
            linux_backends(&env),
            vec![LinuxClipboardBackend::Xclip, LinuxClipboardBackend::Xsel,]
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_returns_empty_when_no_tools_are_available() {
        let env = env_with(false, false, &[]);

        assert!(linux_backends(&env).is_empty());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_backend_command_specs_match_expected_args() {
        assert_eq!(
            LinuxClipboardBackend::WlCopy.command_spec(),
            LinuxClipboardCommand {
                program: "wl-copy",
                args: &[],
            }
        );
        assert_eq!(
            LinuxClipboardBackend::Xclip.command_spec(),
            LinuxClipboardCommand {
                program: "xclip",
                args: &["-selection", "clipboard"],
            }
        );
        assert_eq!(
            LinuxClipboardBackend::Xsel.command_spec(),
            LinuxClipboardCommand {
                program: "xsel",
                args: &["--clipboard", "--input"],
            }
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn run_linux_clipboard_command_times_out_for_slow_process() {
        let cmd = LinuxClipboardCommand {
            program: "sleep",
            args: &["60"],
        };
        let start = std::time::Instant::now();
        let result = run_linux_clipboard_command(cmd, "test");
        let elapsed = start.elapsed();

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("timed out"));
        // Should complete well under the 60s sleep — within ~1s of the 500ms timeout.
        assert!(elapsed < Duration::from_secs(2));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn run_linux_clipboard_command_succeeds_for_fast_command() {
        // `cat` reads stdin to stdout (which is /dev/null here) and exits 0.
        let cmd = LinuxClipboardCommand {
            program: "cat",
            args: &[],
        };
        let result = run_linux_clipboard_command(cmd, "hello");
        assert!(result.is_ok());
    }
}
