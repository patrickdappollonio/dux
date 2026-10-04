//! SIGUSR1: "config.toml changed, reload it".
//!
//! `dux config set` writes the file and then tells the running dux to reload
//! by sending it SIGUSR1, so a change (a new password above all) takes effect
//! at once instead of at the next start. A hand edit can do the same with
//! `kill -USR1 <pid>`. Not SIGHUP, which a terminal sends its processes when it
//! closes. There is deliberately no file watcher: it would turn the explicit
//! reload model of every setting into an implicit one and could apply a file
//! halfway through an editor's save.
//!
//! Two halves live here:
//!
//! - The receiver: [`install`] puts a handler in place that only sets a flag
//!   (all a signal handler may safely do), as early as the process starts and
//!   BEFORE dux takes its single-instance lock, because SIGUSR1's default
//!   action is to terminate and `dux config set` signals whoever holds that
//!   lock. Each serving mode drains the flag with [`take_pending`] and runs
//!   its own existing reload path: the terminal UI's loop (which also covers
//!   its background server), and the engine actor's reload arm for
//!   `dux server` and the start-web-server flip.
//! - The sender: [`signal_running_dux`] finds the process that HOLDS
//!   `dux.lock` (not merely the PID written in it, which can be stale and
//!   reused by an unrelated process) and signals it.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

/// Set by the signal handler, cleared by [`take_pending`].
fn pending() -> &'static Arc<AtomicBool> {
    static PENDING: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    PENDING.get_or_init(|| Arc::new(AtomicBool::new(false)))
}

/// Install the SIGUSR1 handler. Idempotent: the first call registers it and
/// every later call returns that first answer. Never unregistered, so a
/// signal between one serving mode and the next (a flip back to the terminal
/// UI) is recorded rather than killing the process.
pub fn install() -> Result<(), String> {
    static INSTALLED: OnceLock<Result<(), String>> = OnceLock::new();
    INSTALLED
        .get_or_init(|| {
            signal_hook::flag::register(signal_hook::consts::SIGUSR1, Arc::clone(pending()))
                .map(|_| ())
                .map_err(|e| format!("could not install the SIGUSR1 (reload config) handler: {e}"))
        })
        .clone()
}

/// Whether a SIGUSR1 arrived since the last call. Clears the flag, so exactly
/// one serving mode acts on each signal (several signals before the next
/// check are one reload, which is all they could ask for).
pub fn take_pending() -> bool {
    pending().swap(false, Ordering::SeqCst)
}

/// What [`signal_running_dux`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SignalOutcome {
    /// The dux holding the lock was sent SIGUSR1; it reloads its config now.
    Sent { pid: u32 },
    /// No dux holds the lock, so the change applies at the next start.
    NotRunning,
    /// Something holds the lock but it could not be signalled; the reason is
    /// for the user (who can run `kill -USR1` or reload from the app).
    Failed { pid: Option<u32>, reason: String },
}

/// Send SIGUSR1 to the dux that holds `lock_path` (`dux.lock`), if one does.
///
/// The PID written in the lock file is only trusted once the lock is shown
/// to be HELD: on Linux, by finding the flock on that very file in
/// `/proc/locks`, which names its holder's PID; elsewhere, by failing to take
/// a shared lock on it, and then requiring the written PID to be a running
/// process named `dux`. A lock file left by a dux that exited holds no lock,
/// so its PID, possibly reused by something else, is never signalled.
pub fn signal_running_dux(lock_path: &Path) -> SignalOutcome {
    match holder(lock_path, Some("dux")) {
        Holder::None => SignalOutcome::NotRunning,
        Holder::Unknown(reason) => SignalOutcome::Failed { pid: None, reason },
        Holder::Pid(pid) => send(pid),
    }
}

fn send(pid: u32) -> SignalOutcome {
    let Some(raw) = i32::try_from(pid)
        .ok()
        .and_then(rustix::process::Pid::from_raw)
    else {
        return SignalOutcome::Failed {
            pid: Some(pid),
            reason: format!("{pid} is not a process id"),
        };
    };
    match rustix::process::kill_process(raw, rustix::process::Signal::USR1) {
        Ok(()) => SignalOutcome::Sent { pid },
        // It exited between the check and the signal.
        Err(rustix::io::Errno::SRCH) => SignalOutcome::NotRunning,
        Err(error) => SignalOutcome::Failed {
            pid: Some(pid),
            reason: format!("could not signal process {pid}: {error}"),
        },
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Holder {
    None,
    Pid(u32),
    Unknown(String),
}

/// Who holds the flock on `lock_path`. `expected_name` is the process name
/// the portable check requires; the Linux check needs none, because holding
/// this file's lock is what makes a process the running dux.
fn holder(lock_path: &Path, expected_name: Option<&str>) -> Holder {
    let table = std::fs::read_to_string("/proc/locks").ok();
    holder_with_table(lock_path, table.as_deref(), expected_name)
}

/// [`holder`] with the `/proc/locks` text passed in (`None` where there is
/// no such file), so the fallback can be tested.
fn holder_with_table(lock_path: &Path, table: Option<&str>, expected_name: Option<&str>) -> Holder {
    if !lock_path.exists() {
        return Holder::None;
    }
    if let Some(table) = table
        && let Some(pid) = holder_from_proc_locks(lock_path, table)
    {
        return Holder::Pid(pid);
    }
    holder_by_probe(lock_path, expected_name)
}

/// The Linux answer, read without touching the lock: `/proc/locks` lists
/// every flock with its holder's PID and the device and inode of its file.
/// Only a positive answer counts. No matching line is NOT "not running":
/// on btrfs and overlayfs `stat` reports a different device number than
/// the kernel prints there, and a holder in another PID namespace shows as 0,
/// so every miss falls back to the portable probe.
fn holder_from_proc_locks(lock_path: &Path, table: &str) -> Option<u32> {
    let stat = rustix::fs::stat(lock_path).ok()?;
    let (major, minor, inode) = (
        rustix::fs::major(stat.st_dev),
        rustix::fs::minor(stat.st_dev),
        stat.st_ino,
    );
    flock_holder_in(table, major, minor, inode).filter(|pid| *pid != 0)
}

/// The PID holding a FLOCK on the file with this device and inode, from the
/// text of `/proc/locks`. Only an exclusive (WRITE) lock counts, which is
/// the one dux takes; lines for waiters (`->`) are skipped.
fn flock_holder_in(table: &str, major: u32, minor: u32, inode: u64) -> Option<u32> {
    for line in table.lines() {
        if line.contains("->") {
            continue;
        }
        let fields: Vec<&str> = line.split_whitespace().collect();
        // "1:" "FLOCK" "ADVISORY" "WRITE" "<pid>" "<maj>:<min>:<inode>" ...
        if fields.len() < 6 || fields[1] != "FLOCK" || fields[3] != "WRITE" {
            continue;
        }
        let mut id = fields[5].split(':');
        let (Some(maj), Some(min), Some(ino)) = (id.next(), id.next(), id.next()) else {
            continue;
        };
        let matches = u32::from_str_radix(maj, 16).ok() == Some(major)
            && u32::from_str_radix(min, 16).ok() == Some(minor)
            && ino.parse::<u64>().ok() == Some(inode);
        if matches {
            return fields[4].parse().ok();
        }
    }
    None
}

/// The portable answer: try a non-blocking SHARED lock. Taking it means
/// nobody holds the exclusive one, so nobody is running (it is released at
/// once; a dux starting in that same instant would see the lock as taken and
/// say so, which is the cost of this fallback). Failing to take it means a
/// dux holds it; its PID is then read from the file and must be a live
/// process with the expected name.
fn holder_by_probe(lock_path: &Path, expected_name: Option<&str>) -> Holder {
    use rustix::fs::{FlockOperation, flock};
    let file = match std::fs::File::open(lock_path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Holder::None,
        Err(error) => {
            return Holder::Unknown(format!("could not open {}: {error}", lock_path.display()));
        }
    };
    match crate::io_retry::retry_on_interrupt_errno(|| {
        flock(&file, FlockOperation::NonBlockingLockShared)
    }) {
        Ok(()) => {
            let _ = flock(&file, FlockOperation::Unlock);
            return Holder::None;
        }
        Err(err) if err == rustix::io::Errno::WOULDBLOCK || err == rustix::io::Errno::AGAIN => {}
        Err(err) => {
            return Holder::Unknown(format!("could not check {}: {err}", lock_path.display()));
        }
    }
    let Some(pid) = std::fs::read_to_string(lock_path)
        .ok()
        .and_then(|text| text.trim().parse::<u32>().ok())
    else {
        return Holder::Unknown(format!(
            "a dux holds {} but its process id could not be read from it",
            lock_path.display()
        ));
    };
    if let Some(name) = expected_name
        && process_name(pid).as_deref() != Some(name)
    {
        return Holder::Unknown(format!(
            "{} is held, but process {pid} named in it is not a running {name}",
            lock_path.display()
        ));
    }
    Holder::Pid(pid)
}

/// The executable's file name for a live process, or `None`.
fn process_name(pid: u32) -> Option<String> {
    if let Ok(comm) = std::fs::read_to_string(format!("/proc/{pid}/comm")) {
        return Some(comm.trim().to_string());
    }
    let output = std::process::Command::new("ps")
        .args(["-p", &pid.to_string(), "-o", "comm="])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let name = Path::new(&text).file_name()?.to_string_lossy().into_owned();
    (!name.is_empty()).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    const CHILD_ENV: &str = "DUX_TEST_RELOAD_SIGNAL_CHILD";
    const CHILD_TEST: &str = "reload_signal::tests::child_holds_the_lock_and_waits_for_the_signal";

    /// Not a test on its own: re-run as a child process by the tests below,
    /// it is a stand-in dux that holds the lock and records a reload.
    #[test]
    fn child_holds_the_lock_and_waits_for_the_signal() {
        let Ok(dir) = std::env::var(CHILD_ENV) else {
            return;
        };
        let dir = std::path::PathBuf::from(dir);
        install().expect("install");
        let _lock =
            crate::lockfile::SingleInstanceLock::acquire(&dir.join("dux.lock")).expect("lock");
        std::fs::write(dir.join("ready"), std::process::id().to_string()).expect("ready");
        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if take_pending() {
                std::fs::write(dir.join("reloaded"), "yes").expect("reloaded");
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_for(path: &Path) -> bool {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if path.exists() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    fn spawn_stand_in_dux(dir: &Path) -> std::process::Child {
        std::process::Command::new(std::env::current_exe().expect("test binary"))
            .args(["--exact", CHILD_TEST, "--test-threads=1"])
            .env(CHILD_ENV, dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn child")
    }

    #[test]
    fn a_running_dux_holding_the_lock_is_signalled_and_reloads() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut child = spawn_stand_in_dux(dir.path());
        assert!(
            wait_for(&dir.path().join("ready")),
            "the child never took the lock"
        );

        // The portable check wants a process named dux; the Linux one finds
        // the holder from the lock itself. Both must name the child.
        let lock = dir.path().join("dux.lock");
        assert_eq!(holder(&lock, None), Holder::Pid(child.id()));
        assert_eq!(holder_by_probe(&lock, None), Holder::Pid(child.id()));
        assert!(
            matches!(holder_by_probe(&lock, Some("dux")), Holder::Unknown(_)),
            "a lock holder whose name is not dux is not signalled by the portable check"
        );

        assert_eq!(send(child.id()), SignalOutcome::Sent { pid: child.id() });
        assert!(
            wait_for(&dir.path().join("reloaded")),
            "the child never saw the signal"
        );
        assert!(
            child.wait().expect("wait").success(),
            "SIGUSR1 did not kill it"
        );
    }

    /// On btrfs and overlayfs, `stat` reports a device number that differs
    /// from the one `/proc/locks` prints, so no line matches. That is not
    /// "nothing is running": the portable check answers instead.
    #[test]
    fn a_proc_locks_table_with_no_matching_line_falls_back_to_the_probe() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut child = spawn_stand_in_dux(dir.path());
        assert!(
            wait_for(&dir.path().join("ready")),
            "the child never took the lock"
        );
        let lock = dir.path().join("dux.lock");
        let ino = rustix::fs::stat(&lock).expect("stat").st_ino;
        let elsewhere = format!(
            "1: FLOCK  ADVISORY  WRITE {} ab:cd:{ino} 0 EOF\n",
            child.id()
        );
        assert_eq!(
            holder_with_table(&lock, Some(&elsewhere), None),
            Holder::Pid(child.id()),
            "a device mismatch must not read as not running"
        );
        let read_only = format!("1: FLOCK  ADVISORY  READ  999 00:00:{ino} 0 EOF\n");
        assert_eq!(
            flock_holder_in(&read_only, 0, 0, ino),
            None,
            "a READ lock is not dux's"
        );
        send(child.id());
        let _ = child.wait();
    }

    #[test]
    fn a_stale_pid_in_an_unheld_lock_file_is_never_signalled() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A live process that is NOT dux and does not hold the lock, whose PID
        // a dead dux left behind. SIGUSR1's default action would kill it.
        let mut bystander = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let lock = dir.path().join("dux.lock");
        std::fs::write(&lock, format!("{}\n", bystander.id())).expect("stale lock file");

        assert_eq!(signal_running_dux(&lock), SignalOutcome::NotRunning);
        assert_eq!(holder_by_probe(&lock, Some("dux")), Holder::None);
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            bystander.try_wait().expect("try_wait").is_none(),
            "the bystander was not signalled"
        );
        let _ = bystander.kill();
        let _ = bystander.wait();
    }

    #[test]
    fn no_lock_file_means_nothing_is_running() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(
            signal_running_dux(&dir.path().join("dux.lock")),
            SignalOutcome::NotRunning
        );
    }

    #[test]
    fn proc_locks_lines_are_matched_by_device_and_inode() {
        let table = "\
1: POSIX  ADVISORY  WRITE 900 fd:01:555 0 EOF
2: FLOCK  ADVISORY  WRITE 1234 fd:01:777 0 EOF
2: -> FLOCK  ADVISORY  WRITE 4321 fd:01:777 0 EOF
3: FLOCK  ADVISORY  WRITE 99 08:02:777 0 EOF
";
        assert_eq!(flock_holder_in(table, 0xfd, 1, 777), Some(1234));
        assert_eq!(flock_holder_in(table, 8, 2, 777), Some(99));
        assert_eq!(
            flock_holder_in(table, 0xfd, 1, 555),
            None,
            "a POSIX lock is not dux's"
        );
        assert_eq!(flock_holder_in(table, 0xfd, 1, 1), None);
    }

    #[test]
    fn install_is_idempotent_and_the_flag_is_taken_once() {
        install().expect("install");
        install().expect("again");
        pending().store(true, Ordering::SeqCst);
        assert!(take_pending());
        assert!(!take_pending());
    }
}
