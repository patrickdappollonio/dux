//! SIGUSR1: "config.toml changed, reload it", for a person reloading by hand
//! with `kill -USR1 <pid>` after editing the file.
//!
//! `dux config set` and dux's own reloads (a password change, a ban) do not use
//! it: they ask the running dux over its control socket and hear how the reload
//! went (see [`crate::client::reload`]). Not SIGHUP, which a terminal sends its
//! processes when it closes. There is deliberately no file watcher: it would turn
//! the explicit reload model of every setting into an implicit one and could
//! apply a file halfway through an editor's save.
//!
//! The receiver: [`install`] puts a handler in place that only sets a flag
//! (all a signal handler may safely do), as early as the process starts and
//! BEFORE dux takes its single-instance lock, because SIGUSR1's default
//! action is to terminate and a person signals whoever holds that lock. Each
//! serving mode drains the flag with [`take_pending`] and runs its own
//! existing reload path: the terminal UI's loop (which also covers its
//! background server), and the engine actor's reload arm for `dux server`
//! and the start-web-server flip.
//!
//! The lock helpers here ([`lock_holder`], [`dux_may_be_running`]) say who
//! holds `dux.lock`, which the command line reads to tell a dux that is not
//! running from one that cannot be asked.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

/// Set by the signal handler, cleared by [`take_pending`].
fn pending() -> &'static Arc<AtomicBool> {
    static PENDING: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    PENDING.get_or_init(|| Arc::new(AtomicBool::new(false)))
}

static INSTALLED: OnceLock<Result<(), String>> = OnceLock::new();

/// Install the SIGUSR1 handler. Idempotent: the first call registers it and
/// every later call returns that first answer. Never unregistered, so a
/// signal between one serving mode and the next (a flip back to the terminal
/// UI) is recorded rather than killing the process.
pub fn install() -> Result<(), String> {
    INSTALLED
        .get_or_init(|| {
            signal_hook::flag::register(signal_hook::consts::SIGUSR1, Arc::clone(pending()))
                .map(|_| ())
                .map_err(|e| format!("could not install the SIGUSR1 (reload config) handler: {e}"))
        })
        .clone()
}

/// Whether this process's SIGUSR1 handler is installed, which is what lets
/// its lock file advertise that it handles the reload signal.
pub fn is_installed() -> bool {
    matches!(INSTALLED.get(), Some(Ok(())))
}

/// Whether a SIGUSR1 arrived since the last call. Clears the flag, so exactly
/// one serving mode acts on each signal (several signals before the next
/// check are one reload, which is all they could ask for).
pub fn take_pending() -> bool {
    pending().swap(false, Ordering::SeqCst)
}

/// Whether a dux may be running: something holds `lock_path`, or the lock
/// cannot be checked at all (its file unreadable, the holder's process id
/// unreadable). Fails closed, for refusals that protect a running dux, and
/// does not check the process name: a held lock is reason enough.
pub fn dux_may_be_running(lock_path: &Path) -> bool {
    !matches!(holder(lock_path), Holder::None)
}

/// Who holds `lock_path`, by the same checks as [`dux_may_be_running`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LockHolder {
    Free,
    Held(u32),
    /// The lock could not be checked; the reason says why.
    Unknown(String),
}

/// [`LockHolder`] for `lock_path`.
pub fn lock_holder(lock_path: &Path) -> LockHolder {
    match holder(lock_path) {
        Holder::None => LockHolder::Free,
        Holder::Pid(pid) => LockHolder::Held(pid),
        Holder::Unknown(reason) => LockHolder::Unknown(reason),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Holder {
    None,
    Pid(u32),
    Unknown(String),
}

/// Who holds the flock on `lock_path`. Holding this file's lock is what makes
/// a process the running dux.
fn holder(lock_path: &Path) -> Holder {
    let table = std::fs::read_to_string("/proc/locks").ok();
    holder_with_table(lock_path, table.as_deref())
}

/// [`holder`] with the `/proc/locks` text passed in (`None` where there is
/// no such file), so the fallback can be tested.
fn holder_with_table(lock_path: &Path, table: Option<&str>) -> Holder {
    if !lock_path.exists() {
        return Holder::None;
    }
    if let Some(table) = table
        && let Some(pid) = holder_from_proc_locks(lock_path, table)
    {
        return Holder::Pid(pid);
    }
    holder_by_probe(lock_path)
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

/// The portable answer: a non-blocking shared lock, released at once. Taken,
/// nobody runs; refused, a dux holds it and its PID is read from the file.
fn holder_by_probe(lock_path: &Path) -> Holder {
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
        .and_then(|text| crate::lockfile::LockFileContents::parse(&text).pid)
    else {
        return Holder::Unknown(format!(
            "a dux holds {} but its process id could not be read from it",
            lock_path.display()
        ));
    };
    Holder::Pid(pid)
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
    fn a_running_dux_holding_the_lock_is_found_and_reloads_on_sigusr1() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut child = spawn_stand_in_dux(dir.path());
        assert!(
            wait_for(&dir.path().join("ready")),
            "the child never took the lock"
        );

        // The Linux check finds the holder from the lock itself, the portable
        // one by probing it; both must name the child.
        let lock = dir.path().join("dux.lock");
        assert_eq!(holder(&lock), Holder::Pid(child.id()));
        assert_eq!(holder_by_probe(&lock), Holder::Pid(child.id()));
        assert_eq!(lock_holder(&lock), LockHolder::Held(child.id()));

        // A hand-sent `kill -USR1` reaches the handler the child installed
        // before taking the lock, and does not end it.
        let pid = rustix::process::Pid::from_raw(i32::try_from(child.id()).unwrap()).unwrap();
        rustix::process::kill_process(pid, rustix::process::Signal::USR1).expect("signal");
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
            holder_with_table(&lock, Some(&elsewhere)),
            Holder::Pid(child.id()),
            "a device mismatch must not read as not running"
        );
        let read_only = format!("1: FLOCK  ADVISORY  READ  999 00:00:{ino} 0 EOF\n");
        assert_eq!(
            flock_holder_in(&read_only, 0, 0, ino),
            None,
            "a READ lock is not dux's"
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    /// A lock dux cannot even check counts as a running dux for a refusal:
    /// failing closed is what keeps `set` from writing under it.
    #[test]
    fn a_lock_that_cannot_be_checked_counts_as_running() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().expect("tempdir");
        let lock = dir.path().join("dux.lock");
        std::fs::write(&lock, "").expect("lock file");
        std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o000)).unwrap();
        let held = dux_may_be_running(&lock);
        std::fs::set_permissions(&lock, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(held, "an unreadable lock is treated as held");
        assert!(!dux_may_be_running(&dir.path().join("absent.lock")));
    }

    #[test]
    fn a_stale_pid_in_an_unheld_lock_file_is_not_a_holder() {
        let dir = tempfile::tempdir().expect("tempdir");
        // A live process that is NOT dux and does not hold the lock, whose PID
        // a dead dux left behind. SIGUSR1's default action would kill it.
        let mut bystander = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn sleep");
        let lock = dir.path().join("dux.lock");
        std::fs::write(&lock, format!("{}\n", bystander.id())).expect("stale lock file");

        assert_eq!(lock_holder(&lock), LockHolder::Free);
        assert_eq!(holder_by_probe(&lock), Holder::None);
        let _ = bystander.kill();
        let _ = bystander.wait();
    }

    #[test]
    fn no_lock_file_means_nothing_is_running() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(lock_holder(&dir.path().join("dux.lock")), LockHolder::Free);
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
