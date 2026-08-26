use std::process::{Command, Stdio};

use anyhow::{Context, Result};

pub fn open_url(url: &str) -> Result<()> {
    spawn_detached(default_browser_launcher(), url)
}

/// How long to watch the launcher before deciding it succeeded.
///
/// A launcher that is going to fail (no handler configured, a broken desktop
/// file) fails in milliseconds; one that is still running after this has
/// already handed the address to a browser, and some `xdg-open` paths then stay
/// alive for the browser's whole lifetime. Waiting on those would hold the
/// status spinner open for hours, so the grace window is short and a launcher
/// still running at the end of it counts as a success.
const LAUNCHER_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

/// How often to look in on the launcher during the grace window.
const LAUNCHER_POLL: std::time::Duration = std::time::Duration::from_millis(10);

/// Hand one address to the platform's URL launcher, watch it briefly, and never
/// leave it behind as a zombie.
///
/// All three standard streams are `/dev/null`, never inherited: the terminal UI
/// owns the terminal, and a launcher that decided to print a warning (or worse,
/// read from stdin) would paint over the running interface or block behind a
/// prompt nobody can see. dux never reads the child's output, so there is
/// nothing to keep.
///
/// The child is REAPED either way. Within the grace window a non-zero exit is
/// reported as the failure it is, because "nothing happened and dux said it
/// worked" is the worst outcome here; a launcher still running when the window
/// closes is handed to a small thread whose only job is to wait for it, so
/// clicking twenty links never accumulates twenty zombies.
fn spawn_detached(launcher: &str, url: &str) -> Result<()> {
    let mut child = spawn_launcher(launcher, url)?;

    let deadline = std::time::Instant::now() + LAUNCHER_GRACE;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => {
                anyhow::bail!("{launcher} exited with {status} without opening the address")
            }
            Ok(None) => {}
            Err(err) => {
                // The child is unreachable, which is not a reason to claim the
                // address never opened; it is a reason to stop watching.
                crate::logger::warn(&format!("could not check on {launcher}: {err}"));
                return Ok(());
            }
        }
        if std::time::Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(LAUNCHER_POLL);
    }

    // Still running: it did its job. Reap it off to the side.
    if let Err(err) = std::thread::Builder::new()
        .name("dux-browser-reaper".into())
        .spawn(move || {
            let _ = child.wait();
        })
    {
        crate::logger::warn(&format!(
            "could not start the browser reaper thread, so one launcher process may linger: {err}"
        ));
    }
    Ok(())
}

/// The spawn itself: the launcher, the address, and three `/dev/null` streams.
/// Separated from the watching policy above so a test can hold the child it
/// started and prove what it was handed.
fn spawn_launcher(launcher: &str, url: &str) -> Result<std::process::Child> {
    Command::new(launcher)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .with_context(|| format!("failed to launch default browser via {launcher}"))
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

    /// Quote one path for a POSIX shell: single quotes, with any embedded
    /// single quote closed, escaped and reopened. Temporary directories can
    /// contain anything a user's `TMPDIR` contains, and an unescaped path would
    /// silently rewrite the probe script rather than fail.
    fn sh_quote(path: &std::path::Path) -> String {
        format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"))
    }

    /// A scratch directory of this test's own, removed by the caller once the
    /// child it launched is known to be gone.
    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "dux-browser-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the scratch directory");
        dir
    }

    fn write_script(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
        let script = dir.join("probe.sh");
        std::fs::write(&script, body).expect("write the probe script");
        script
    }

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
    ///
    /// `test -ef` (same device and inode) is POSIX XSI and is implemented by
    /// dash, bash and busybox ash alike, which covers every shell `sh` resolves
    /// to on the platforms dux targets.
    #[test]
    fn the_launched_child_gets_null_stdio() {
        let dir = scratch("stdio");
        let result = dir.join("result.txt");
        let script = write_script(
            &dir,
            &format!(
                "exec 3>{result}\n\
                 [ /dev/stdin -ef /dev/null ] && echo stdin-null >&3\n\
                 [ /dev/stdout -ef /dev/null ] && echo stdout-null >&3\n\
                 [ /dev/stderr -ef /dev/null ] && echo stderr-null >&3\n\
                 echo done >&3\n",
                result = sh_quote(&result)
            ),
        );

        // The launcher is `sh` and the "url" is the probe script, so the spawn
        // under test is the very one `open_url` performs.
        let mut child =
            spawn_launcher("sh", script.to_string_lossy().as_ref()).expect("spawn the probe");

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut report = String::new();
        while std::time::Instant::now() < deadline {
            report = std::fs::read_to_string(&result).unwrap_or_default();
            if report.contains("done") {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // Never pull the directory out from under a child that is still
        // running: stop it and collect it first, or the removal races the
        // script it is executing.
        let _ = child.kill();
        let _ = child.wait();
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

    /// A launcher that exits non-zero opened nothing, and saying otherwise
    /// would leave the user staring at a browser that never came up.
    #[test]
    fn a_launcher_that_fails_is_reported_as_a_failure() {
        let dir = scratch("exit1");
        let script = write_script(&dir, "exit 1\n");

        let err = spawn_detached("sh", script.to_string_lossy().as_ref())
            .expect_err("a non-zero exit must be an error");

        let _ = std::fs::remove_dir_all(&dir);
        let message = format!("{err:#}");
        assert!(
            message.contains("exited with") && message.contains("without opening"),
            "the failure must say the launcher exited without opening anything; got {message:?}"
        );
    }

    /// A launcher that exits cleanly is a success, and it is reaped inside the
    /// grace window rather than left for anyone else to collect.
    #[test]
    fn a_launcher_that_exits_cleanly_succeeds() {
        let dir = scratch("exit0");
        let script = write_script(&dir, "exit 0\n");

        spawn_detached("sh", script.to_string_lossy().as_ref()).expect("a clean exit is a success");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A launcher still running when the grace window closes has already handed
    /// the address over (some `xdg-open` paths live as long as the browser), so
    /// it counts as a success and goes to the reaper rather than being waited
    /// on. The test proves both halves: the success, and that the call returned
    /// long before the child did.
    #[test]
    fn a_launcher_that_outlives_the_grace_window_still_succeeds_promptly() {
        let dir = scratch("slow");
        // `exec`, so the shell is REPLACED by the sleeper and the script file is
        // no longer being read: the scratch directory can go immediately, and
        // the reaper thread collects the sleeper a couple of seconds later.
        let script = write_script(&dir, "exec sleep 2\n");

        let started = std::time::Instant::now();
        spawn_detached("sh", script.to_string_lossy().as_ref())
            .expect("a launcher that is still running has not failed");
        let waited = started.elapsed();
        let _ = std::fs::remove_dir_all(&dir);

        assert!(
            waited < std::time::Duration::from_secs(2),
            "the grace window must not wait out the browser's lifetime; waited {waited:?}"
        );
    }
}
