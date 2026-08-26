use std::process::{Command, Stdio};

use anyhow::{Context, Result};

pub fn open_url(url: &str) -> Result<()> {
    spawn_detached(default_browser_launcher(), url)
}

/// Hand one address to the platform's URL launcher and forget about it.
///
/// All three standard streams are `/dev/null`, never inherited: the terminal UI
/// owns the terminal, and a launcher that decided to print a warning (or worse,
/// read from stdin) would paint over the running interface or block behind a
/// prompt nobody can see. dux never reads the child's output, so there is
/// nothing to keep.
fn spawn_detached(launcher: &str, url: &str) -> Result<()> {
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

    /// The launched child must see `/dev/null` on all three streams. Measured
    /// rather than asserted: the child itself compares each of its own
    /// descriptors against `/dev/null` and writes the verdict to a file on a
    /// descriptor the harness opened for it. Run under `cargo test`, stdout and
    /// stderr are pipes, so an inherited stream fails this.
    #[test]
    fn the_launched_child_gets_null_stdio() {
        let dir = std::env::temp_dir().join(format!(
            "dux-browser-stdio-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).expect("create the scratch directory");
        let script = dir.join("probe.sh");
        let result = dir.join("result.txt");
        std::fs::write(
            &script,
            format!(
                "exec 3>'{result}'\n\
                 [ /dev/stdin -ef /dev/null ] && echo stdin-null >&3\n\
                 [ /dev/stdout -ef /dev/null ] && echo stdout-null >&3\n\
                 [ /dev/stderr -ef /dev/null ] && echo stderr-null >&3\n\
                 echo done >&3\n",
                result = result.display()
            ),
        )
        .expect("write the probe script");

        // The launcher is `sh` and the "url" is the probe script, so the spawn
        // under test is the very same one `open_url` performs.
        spawn_detached("sh", script.to_string_lossy().as_ref()).expect("spawn the probe");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut report = String::new();
        while std::time::Instant::now() < deadline {
            report = std::fs::read_to_string(&result).unwrap_or_default();
            if report.contains("done") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let _ = std::fs::remove_dir_all(&dir);

        assert!(
            report.contains("done"),
            "the probe never ran to completion; it reported {report:?}"
        );
        for stream in ["stdin", "stdout", "stderr"] {
            assert!(
                report.contains(&format!("{stream}-null")),
                "the child's {stream} must be /dev/null, never inherited; the probe reported \
                 {report:?}"
            );
        }
    }
}
