use std::process::{Command, Stdio};

use anyhow::{Context, Result};

pub fn open_url(url: &str) -> Result<()> {
    let launcher = default_browser_launcher();
    Command::new(launcher)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to launch default browser via {launcher}"))?;
    Ok(())
}

fn default_browser_launcher() -> &'static str {
    if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_launcher_matches_supported_platform() {
        if cfg!(target_os = "macos") {
            assert_eq!(default_browser_launcher(), "open");
        } else {
            assert_eq!(default_browser_launcher(), "xdg-open");
        }
    }
}
