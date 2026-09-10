use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use chrono::Utc;

use crate::config::{DuxPaths, LoggingConfig};

static LOGGER: OnceLock<Logger> = OnceLock::new();

/// The threshold [`log`] gates on, kept outside [`LOGGER`] because the file is
/// opened once for the process while the level moves with the config: a reload
/// retunes it through [`set_level`].
static LEVEL: AtomicU8 = AtomicU8::new(LogLevel::Info as u8);

struct Logger {
    log: RotatingLog,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
}

impl LogLevel {
    fn from_str(value: &str) -> Self {
        match value {
            "debug" => Self::Debug,
            "error" => Self::Error,
            "warn" => Self::Warn,
            _ => Self::Info,
        }
    }

    /// The inverse of the `as u8` cast used to store the level. Exhaustive on
    /// the stored discriminants; anything else means a corrupted store, which
    /// degrades to the same default `from_str` uses.
    fn from_u8(value: u8) -> Self {
        match value {
            v if v == Self::Error as u8 => Self::Error,
            v if v == Self::Warn as u8 => Self::Warn,
            v if v == Self::Debug as u8 => Self::Debug,
            _ => Self::Info,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
        }
    }
}

pub fn init(config: &LoggingConfig, paths: &DuxPaths) {
    let path = resolve_log_path(config, paths);
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    set_level(&config.level);
    if let Ok(log) = RotatingLog::open(path.clone(), RotationSettings::from_config(config)) {
        let _ = LOGGER.set(Logger { log });
        info(&format!("logger initialized at {}", path.display()));
        install_panic_hook();
    }
}

/// Route every panic through the log file before the default hook prints it to
/// stderr: the engine runs on a dedicated OS thread, where a panic would
/// otherwise leave its message on a stderr nobody captured. Records the thread,
/// message and location, then runs the previous hook, so terminal users and
/// `RUST_BACKTRACE` behavior are unchanged. Installed once, only after the logger
/// has a file to write to.
fn install_panic_hook() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let thread = std::thread::current();
            let name = thread.name().unwrap_or("<unnamed>");
            let message = info
                .payload()
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| info.payload().downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "<non-string panic payload>".to_string());
            let location = info
                .location()
                .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
                .unwrap_or_else(|| "<unknown location>".to_string());
            error(&format!(
                "thread '{name}' panicked at {location}: {message}"
            ));
            previous(info);
        }));
    });
}

pub fn warn(message: &str) {
    log(LogLevel::Warn, message);
}

pub fn info(message: &str) {
    log(LogLevel::Info, message);
}

pub fn debug(message: &str) {
    log(LogLevel::Debug, message);
}

pub fn error(message: &str) {
    log(LogLevel::Error, message);
}

/// Adopt a new `logging.level`. Takes effect on the next line written; the log
/// FILE is opened once for the process, so `logging.path` is startup-only and a
/// reload cannot move it.
pub fn set_level(level: &str) {
    LEVEL.store(LogLevel::from_str(level) as u8, Ordering::Relaxed);
}

/// Adopt new rotation settings. They are read afresh on every line written, so
/// a reload takes effect at the next write; the log FILE stays the one opened at
/// startup, exactly as `logging.path` does.
pub fn set_rotation(config: &LoggingConfig) {
    if let Some(logger) = LOGGER.get() {
        logger.log.apply(RotationSettings::from_config(config));
    }
}

/// The name of the level `log` currently gates on.
pub fn current_level() -> &'static str {
    match LogLevel::from_u8(LEVEL.load(Ordering::Relaxed)) {
        LogLevel::Error => "error",
        LogLevel::Warn => "warn",
        LogLevel::Info => "info",
        LogLevel::Debug => "debug",
    }
}

fn log(level: LogLevel, message: &str) {
    if level > LogLevel::from_u8(LEVEL.load(Ordering::Relaxed)) {
        return;
    }
    let Some(logger) = LOGGER.get() else {
        return;
    };
    let line = format!(
        "{} {:<5} {}\n",
        Utc::now().to_rfc3339(),
        level.as_str(),
        message
    );
    logger.log.write_line(&line);
}

/// What the rotation machinery reads on every line written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RotationSettings {
    max_bytes: u64,
    keep: u32,
    compress: bool,
}

impl RotationSettings {
    fn from_config(config: &LoggingConfig) -> Self {
        Self {
            max_bytes: config.max_bytes,
            keep: config.keep,
            compress: config.compress,
        }
    }
}

/// Everything a write touches, behind one lock: the handle, how many bytes are
/// already in it, and whether a compression thread is running.
///
/// The size lives here rather than being stat'd per line because the check runs
/// on every line and dux is the only writer of its own log.
struct LogState {
    file: std::fs::File,
    size: u64,
    compressing: bool,
}

/// An append-only log that rotates itself by size.
///
/// dux rotates rather than leaving it to logrotate, because the log lives in the
/// user's config directory on a laptop, not in `/var/log` on a machine with a
/// cron. Rotation happens on a write and only on a write: a dux that is not
/// logging never touches the files.
pub(crate) struct RotatingLog {
    path: PathBuf,
    state: Arc<Mutex<LogState>>,
    max_bytes: AtomicU64,
    keep: AtomicU32,
    compress: AtomicBool,
}

impl RotatingLog {
    fn open(path: PathBuf, settings: RotationSettings) -> std::io::Result<Self> {
        let file = open_log_file(&path)?;
        // Seeded from the file already on disk, so a log that grew past the
        // limit under an older dux rotates on the first line rather than after
        // another whole limit's worth. An unreadable length seeds 0, which
        // costs at most one late rotation.
        let size = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        Ok(Self {
            path,
            state: Arc::new(Mutex::new(LogState {
                file,
                size,
                compressing: false,
            })),
            max_bytes: AtomicU64::new(settings.max_bytes),
            keep: AtomicU32::new(settings.keep),
            compress: AtomicBool::new(settings.compress),
        })
    }

    fn apply(&self, settings: RotationSettings) {
        self.max_bytes.store(settings.max_bytes, Ordering::Relaxed);
        self.keep.store(settings.keep, Ordering::Relaxed);
        self.compress.store(settings.compress, Ordering::Relaxed);
    }

    fn settings(&self) -> RotationSettings {
        RotationSettings {
            max_bytes: self.max_bytes.load(Ordering::Relaxed),
            keep: self.keep.load(Ordering::Relaxed),
            compress: self.compress.load(Ordering::Relaxed),
        }
    }

    /// Append one line, rotating first if it would not fit.
    ///
    /// The whole line is written under one lock, so a rotation can never land
    /// between two halves of a line and concurrent writers cannot interleave.
    fn write_line(&self, line: &str) {
        let bytes = line.as_bytes();
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let settings = self.settings();
        // A line longer than the whole limit still goes somewhere: rotating an
        // already-empty file would loop forever and lose it.
        if settings.max_bytes > 0
            && state.size > 0
            && state.size.saturating_add(bytes.len() as u64) > settings.max_bytes
        {
            self.rotate(&mut state, settings);
        }
        if state.file.write_all(bytes).is_ok() {
            state.size = state.size.saturating_add(bytes.len() as u64);
        }
        let _ = state.file.flush();
    }

    /// `dux.log.3`, the plain rotated copy at position `n`.
    fn numbered(&self, n: u32) -> PathBuf {
        let mut name = self.path.clone().into_os_string();
        name.push(format!(".{n}"));
        PathBuf::from(name)
    }

    /// `dux.log.3.gz`, the compressed rotated copy at position `n`.
    fn compressed(&self, n: u32) -> PathBuf {
        let mut name = self.numbered(n).into_os_string();
        name.push(".gz");
        PathBuf::from(name)
    }

    /// Shift the numbered copies up, move the live log into position 1, and open
    /// a fresh one. Called with the state lock held.
    fn rotate(&self, state: &mut LogState, settings: RotationSettings) {
        let keep = settings.keep;

        // Everything from `keep` upward is past the end. The walk continues
        // past that point so lowering `keep` cleans up the copies the old value
        // left behind, in both the plain and the compressed spelling.
        let mut doomed = if keep == 0 { 1 } else { keep };
        while self.numbered(doomed).exists() || self.compressed(doomed).exists() {
            let _ = fs::remove_file(self.numbered(doomed));
            let _ = fs::remove_file(self.compressed(doomed));
            doomed = doomed.saturating_add(1);
        }

        // A copy may be plain (not compressed yet, or compression off) or
        // gzipped, so both spellings shift or a mixed set loses its ordering.
        for n in (1..keep).rev() {
            let _ = fs::rename(self.numbered(n), self.numbered(n + 1));
            let _ = fs::rename(self.compressed(n), self.compressed(n + 1));
        }

        let moved = if keep == 0 {
            fs::remove_file(&self.path)
        } else {
            fs::rename(&self.path, self.numbered(1))
        };
        if let Err(err) = moved {
            // Nothing is lost: the old handle is still open on the old file and
            // the caller appends to it. Stderr rather than the log, because the
            // log is the thing that just misbehaved.
            report_rotation_failure(&format!(
                "dux could not rotate the log at {}: {err}. Logging continues to the same file",
                self.path.display()
            ));
            return;
        }

        match open_log_file(&self.path) {
            Ok(file) => {
                state.file = file;
                state.size = 0;
            }
            Err(err) => {
                report_rotation_failure(&format!(
                    "dux rotated the log at {} but could not open a new one: {err}. \
                     Logging continues to the rotated copy",
                    self.path.display()
                ));
                return;
            }
        }

        if settings.compress && keep > 0 && !state.compressing {
            let pending: Vec<PathBuf> = (1..=keep)
                .map(|n| self.numbered(n))
                .filter(|plain| plain.exists())
                .collect();
            if !pending.is_empty() {
                state.compressing = true;
                spawn_compression(pending, Arc::clone(&self.state));
            }
        }
    }
}

/// Gzip each plain rotated copy on a thread of its own, then release the claim.
///
/// One thread at a time: a rotation while this runs queues nothing, because the
/// next rotation's own sweep picks up whatever is still plain. Warnings go
/// through the logger, which is safe because the flag is only cleared once every
/// warning has been written, so a rotation triggered by one of them spawns
/// nothing.
fn spawn_compression(pending: Vec<PathBuf>, state: Arc<Mutex<LogState>>) {
    std::thread::spawn(move || {
        for plain in pending {
            let mut target = plain.clone().into_os_string();
            target.push(".gz");
            if let Err(err) = compress_file(&plain, Path::new(&target)) {
                warn(&format!(
                    "could not compress the rotated log at {}: {err}. \
                     It was left uncompressed and dux will try again on the next rotation",
                    plain.display()
                ));
            }
        }
        if let Ok(mut state) = state.lock() {
            state.compressing = false;
        }
    });
}

/// Gzip `source` to `target` through a temporary file in the same directory, so
/// a crash or a failure never leaves a half-written `.gz` in place of a readable
/// plain copy. The plain copy is removed only once the gzip is in place.
fn compress_file(source: &Path, target: &Path) -> std::io::Result<()> {
    let bytes = fs::read(source)?;

    let mut temp = target.to_path_buf().into_os_string();
    temp.push(format!(".{}.tmp", std::process::id()));
    let temp = PathBuf::from(temp);
    let _ = fs::remove_file(&temp);

    let result = (|| -> std::io::Result<()> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        crate::file_modes::restrict_to_owner_best_effort(&temp, "compressed log file");
        let mut encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        encoder.write_all(&bytes)?;
        encoder.finish()?.sync_all()?;
        // A rotation racing this pass may already have shifted the source out
        // from under us; renaming then would put stale bytes where a newer
        // copy's name is about to be.
        if !source.exists() {
            return Err(std::io::Error::other(
                "the rotated copy moved while it was being compressed",
            ));
        }
        fs::rename(&temp, target)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp);
        return result;
    }
    fs::remove_file(source)
}

/// Say once, on stderr, that rotation is not working. Once because the failure
/// repeats on every line past the limit, and on stderr because the log file is
/// exactly what is in doubt.
fn report_rotation_failure(message: &str) {
    static REPORTED: std::sync::Once = std::sync::Once::new();
    REPORTED.call_once(|| {
        eprintln!("{message}");
    });
}

/// Open the log for appending, creating it if absent, and restrict it to its
/// owner. The log records the user's project paths, agent names, and error text,
/// so it gets the same treatment as the rest of the config directory; see
/// [`crate::file_modes`] for why the directory's own mode is what really settles
/// this. Tightening runs on every open, so a log left `0644` by an older
/// installation is corrected rather than left as it was.
///
/// The tightening is best effort and its failure is not this function's failure.
/// `logging.path` accepts any absolute path, so a log under `/var/log` owned by
/// an admin, on a Windows mount under WSL2, or on a FAT or NFS volume is a path
/// dux can append to but cannot `chmod`, and a slightly loose log beats no log
/// at all. [`crate::storage`] does the same for the database.
///
/// Separate from [`init`] because `init` installs a process-global logger and a
/// panic hook, neither of which a test can do twice.
fn open_log_file(path: &PathBuf) -> std::io::Result<std::fs::File> {
    let file = OpenOptions::new().create(true).append(true).open(path)?;
    // A warning about the log file itself may have nowhere to go: `init`
    // installs the logger it warns through just after this returns.
    crate::file_modes::restrict_to_owner_best_effort(path, "log file");
    Ok(file)
}

pub fn resolve_log_path(config: &LoggingConfig, paths: &DuxPaths) -> PathBuf {
    let configured = PathBuf::from(&config.path);
    if configured.as_os_str().is_empty() {
        return paths.root.join("dux.log");
    }
    if configured.is_absolute() {
        configured
    } else {
        paths.root.join(configured)
    }
}

/// Serializes the tests that move the process-wide [`LEVEL`], so a parallel run
/// cannot read another test's threshold.
#[cfg(test)]
static LEVEL_TEST_LOCK: Mutex<()> = Mutex::new(());

/// Claim the process-wide log level for the duration of a test.
///
/// EVERY test that moves the level must hold this, including the ones that move
/// it only as a side effect of `Engine::apply_reloaded_config`: cargo runs tests
/// in parallel threads and the level is one static, so an unguarded reload
/// storing its own config's level lands in the middle of another test's
/// assertion window.
#[cfg(test)]
pub(crate) fn level_test_guard() -> std::sync::MutexGuard<'static, ()> {
    LEVEL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Whether the level guard is held right now, for a test that needs to see the
/// exclusion rather than trust it.
#[cfg(test)]
pub(crate) fn level_guard_is_held() -> bool {
    LEVEL_TEST_LOCK.try_lock().is_err()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn set_level_retunes_the_threshold_log_gates_on() {
        let _guard = level_test_guard();
        set_level("info");
        assert!(LogLevel::Debug > LogLevel::from_u8(LEVEL.load(Ordering::Relaxed)));

        set_level("debug");
        assert_eq!(current_level(), "debug");
        assert!(LogLevel::Debug <= LogLevel::from_u8(LEVEL.load(Ordering::Relaxed)));

        set_level("warn");
        assert!(LogLevel::Info > LogLevel::from_u8(LEVEL.load(Ordering::Relaxed)));
        assert!(LogLevel::Error <= LogLevel::from_u8(LEVEL.load(Ordering::Relaxed)));

        set_level("info");
    }

    /// The reload path stores the level unconditionally, so the exclusion is what
    /// keeps a sibling test's reload out of another test's assertion window.
    /// Checked from a second thread, because the guard is not reentrant.
    #[test]
    fn the_level_guard_shuts_a_second_holder_out() {
        let guard = level_test_guard();
        let held = std::thread::spawn(level_guard_is_held)
            .join()
            .expect("the probe thread");
        assert!(
            held,
            "a sibling reload waits instead of storing its own level"
        );
        drop(guard);
    }

    /// `dux.log` records paths, project names, and error text from the user's
    /// own work, so it gets the same owner-only treatment as the rest of the
    /// config directory. `init` installs a process-global logger and so cannot
    /// be called from a test; the open is the seam.
    #[test]
    fn open_log_file_leaves_it_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dux.log");
        drop(open_log_file(&path).unwrap());
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode & 0o077, 0, "expected owner-only, got {mode:o}");
    }

    #[test]
    fn open_log_file_tightens_a_log_left_world_readable_by_an_older_install() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dux.log");
        fs::write(&path, "old\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        drop(open_log_file(&path).unwrap());
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0600, got {mode:o}");
    }

    /// A log dux can append to but cannot tighten must still be OPENED.
    /// `open_log_file`'s error was swallowed whole by `init`'s
    /// `if let Ok(file)`, so a `logging.path` dux could not `chmod` (under
    /// `/var/log`, on a Windows mount under WSL2, on FAT or NFS) meant no
    /// logging at all and no message anywhere.
    ///
    /// Note what this test does NOT cover, because it reads as though it does:
    /// on a symlink the tightening is SKIPPED, returning `Ok(())` before any
    /// chmod is attempted, so there is no error here for a fatal version to
    /// propagate and this test passes either way. The chmod-actually-fails
    /// case is pinned separately below.
    #[test]
    fn open_log_file_still_opens_when_the_mode_cannot_be_applied() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("real.log");
        fs::write(&target, "old\n").unwrap();
        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        let path = dir.path().join("dux.log");
        std::os::unix::fs::symlink(&target, &path).unwrap();

        {
            use std::io::Write;
            let mut file = open_log_file(&path).expect("the log must still open");
            file.write_all(b"new\n").unwrap();
        }

        assert_eq!(fs::read_to_string(&target).unwrap(), "old\nnew\n");
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o644,
            "the symlink target's mode must have been left alone"
        );
    }

    /// The real one: a log dux can APPEND to but genuinely cannot `chmod` must
    /// still open. It was claimed this case could not be built unprivileged.
    /// It can, and this is the MEASURED shape: `/dev/null` is mode `0666`, so
    /// the tightening is attempted, and the chmod is refused with
    /// `PermissionDenied` for a non-root caller while the append itself
    /// succeeds. Making `open_log_file`'s tightening fatal fails this test and
    /// nothing else in the crate.
    ///
    /// Root is the one caller for whom that chmod would SUCCEED, and would
    /// change the mode of `/dev/null` system-wide, so root is skipped out loud
    /// rather than allowed to pass meaninglessly.
    #[test]
    fn open_log_file_still_opens_when_the_chmod_itself_fails() {
        if rustix::process::geteuid().is_root() {
            eprintln!("SKIPPED: running as root, so the chmod would succeed and change /dev/null");
            return;
        }
        let path = PathBuf::from("/dev/null");
        if !path.exists() {
            eprintln!("SKIPPED: no /dev/null here");
            return;
        }
        let before = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_ne!(
            before & 0o077,
            0,
            "the test is only meaningful while a tightening is actually attempted"
        );
        assert!(
            crate::file_modes::restrict_to_owner(&path).is_err(),
            "the test is only meaningful while the chmod really fails"
        );

        {
            use std::io::Write;
            let mut file = open_log_file(&path).expect("the log must still open");
            file.write_all(b"a line that goes nowhere\n").unwrap();
        }

        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            before,
            "nothing should have been changed"
        );
    }

    #[test]
    fn open_log_file_appends_rather_than_truncating() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dux.log");
        fs::write(&path, "old\n").unwrap();
        drop(open_log_file(&path).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "old\n");
    }

    /// The rotation tests drive a [`RotatingLog`] directly rather than the
    /// process-global logger, which can only be initialized once and would leave
    /// every test after the first measuring nothing.
    mod rotation {
        use super::*;
        use std::io::Read;

        fn settings(max_bytes: u64, keep: u32, compress: bool) -> RotationSettings {
            RotationSettings {
                max_bytes,
                keep,
                compress,
            }
        }

        fn log_in(dir: &Path, settings: RotationSettings) -> RotatingLog {
            RotatingLog::open(dir.join("dux.log"), settings).expect("open the log")
        }

        /// Wait for the background compression to land. Bounded, and a timeout
        /// fails the test rather than passing quietly.
        fn wait_for(what: &str, mut ready: impl FnMut() -> bool) {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
            while std::time::Instant::now() < deadline {
                if ready() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            panic!("timed out waiting for {what}");
        }

        /// Write a line and wait for any compression it started to finish, so
        /// the files on disk have settled before the next assertion. One
        /// compression thread runs at a time by design, so a burst of rotations
        /// deliberately leaves a backlog for the next one; that is what these
        /// tests must not race.
        fn write_and_settle(log: &RotatingLog, line: &str) {
            log.write_line(line);
            wait_for("the compression thread to finish", || {
                !log.state.lock().unwrap().compressing
            });
        }

        fn gunzip(path: &Path) -> Vec<u8> {
            let file = fs::File::open(path).expect("open the compressed copy");
            let mut out = Vec::new();
            flate2::read::GzDecoder::new(file)
                .read_to_end(&mut out)
                .expect("decompress");
            out
        }

        #[test]
        fn a_write_past_the_limit_moves_the_old_log_aside() {
            let dir = tempfile::tempdir().unwrap();
            let log = log_in(dir.path(), settings(24, 3, false));

            log.write_line("first line\n");
            log.write_line("second line\n");
            log.write_line("third line\n");

            assert_eq!(
                fs::read_to_string(dir.path().join("dux.log")).unwrap(),
                "third line\n",
                "the fresh log must hold only the line that did not fit"
            );
            assert_eq!(
                fs::read_to_string(dir.path().join("dux.log.1")).unwrap(),
                "first line\nsecond line\n"
            );
            assert_eq!(
                log.state.lock().unwrap().size,
                "third line\n".len() as u64,
                "the counter must restart with the new file"
            );
        }

        #[test]
        fn max_bytes_zero_never_rotates() {
            let dir = tempfile::tempdir().unwrap();
            let log = log_in(dir.path(), settings(0, 3, true));
            for n in 0..200 {
                log.write_line(&format!("line {n}\n"));
            }
            assert!(!dir.path().join("dux.log.1").exists());
            assert_eq!(
                fs::read_to_string(dir.path().join("dux.log"))
                    .unwrap()
                    .lines()
                    .count(),
                200
            );
        }

        #[test]
        fn five_rotations_with_keep_three_leave_exactly_three_copies() {
            let dir = tempfile::tempdir().unwrap();
            let log = log_in(dir.path(), settings(12, 3, true));
            for n in 1..=6 {
                write_and_settle(&log, &format!("line {n}\n"));
            }

            assert!(dir.path().join("dux.log").exists());
            for n in 1..=3 {
                assert!(
                    dir.path().join(format!("dux.log.{n}.gz")).exists(),
                    "expected a compressed copy at position {n}"
                );
                assert!(
                    !dir.path().join(format!("dux.log.{n}")).exists(),
                    "the plain copy at position {n} should be gone"
                );
            }
            for n in 4..=8 {
                assert!(
                    !dir.path().join(format!("dux.log.{n}")).exists()
                        && !dir.path().join(format!("dux.log.{n}.gz")).exists(),
                    "nothing older than the third copy may survive, found position {n}"
                );
            }
        }

        #[test]
        fn keep_zero_rotates_and_leaves_only_the_live_log() {
            let dir = tempfile::tempdir().unwrap();
            let log = log_in(dir.path(), settings(12, 0, true));
            for n in 1..=6 {
                log.write_line(&format!("line {n}\n"));
            }
            assert_eq!(
                fs::read_dir(dir.path()).unwrap().count(),
                1,
                "keep = 0 discards the old log instead of numbering it"
            );
            assert!(dir.path().join("dux.log").exists());
        }

        #[test]
        fn a_compressed_copy_decompresses_to_the_original_bytes() {
            let dir = tempfile::tempdir().unwrap();
            let log = log_in(dir.path(), settings(24, 3, true));
            log.write_line("first line\n");
            log.write_line("second line\n");
            log.write_line("third line\n");

            wait_for("the first copy to be compressed", || {
                dir.path().join("dux.log.1.gz").exists()
            });
            assert!(
                !dir.path().join("dux.log.1").exists(),
                "the plain copy must be removed once the gzip is in place"
            );
            assert_eq!(
                gunzip(&dir.path().join("dux.log.1.gz")),
                b"first line\nsecond line\n"
            );
        }

        #[test]
        fn compression_off_leaves_the_copies_plain() {
            let dir = tempfile::tempdir().unwrap();
            let log = log_in(dir.path(), settings(12, 3, false));
            for n in 1..=6 {
                log.write_line(&format!("line {n}\n"));
            }
            for n in 1..=3 {
                assert!(dir.path().join(format!("dux.log.{n}")).exists());
                assert!(!dir.path().join(format!("dux.log.{n}.gz")).exists());
            }
        }

        /// A compression that cannot finish must cost nothing: the readable
        /// plain copy stays and the live log keeps taking lines. Built by
        /// putting a DIRECTORY where the gzip has to land, so the rename over it
        /// fails for every caller, root included.
        #[test]
        fn a_failed_compression_leaves_the_plain_copy_and_the_log_keeps_working() {
            let dir = tempfile::tempdir().unwrap();
            fs::create_dir(dir.path().join("dux.log.1.gz")).unwrap();
            let log = log_in(dir.path(), settings(24, 1, true));

            log.write_line("first line\n");
            log.write_line("second line\n");
            log.write_line("third line\n");

            wait_for("the compression attempt to clean up after itself", || {
                !fs::read_dir(dir.path()).unwrap().any(|entry| {
                    entry
                        .unwrap()
                        .file_name()
                        .to_string_lossy()
                        .ends_with(".tmp")
                })
            });

            assert_eq!(
                fs::read_to_string(dir.path().join("dux.log.1")).unwrap(),
                "first line\nsecond line\n",
                "the plain copy must survive a compression that could not finish"
            );
            assert!(dir.path().join("dux.log.1.gz").is_dir());

            log.write_line("fourth line\n");
            assert_eq!(
                fs::read_to_string(dir.path().join("dux.log")).unwrap(),
                "third line\nfourth line\n"
            );
        }

        /// The whole point of rotating under the write lock: with several
        /// threads writing across the limit, every line must land somewhere,
        /// exactly once, whole.
        #[test]
        fn concurrent_writers_lose_no_line_across_a_rotation() {
            let dir = tempfile::tempdir().unwrap();
            // Compression stays off so the reader below sees a set of files
            // nothing is still rewriting, and `keep` is far above the number of
            // rotations so nothing is deleted.
            let log = Arc::new(log_in(dir.path(), settings(200, 10_000, false)));

            let writers = 8;
            let per_writer = 200;
            let barrier = Arc::new(std::sync::Barrier::new(writers));
            let mut handles = Vec::new();
            for writer in 0..writers {
                let log = Arc::clone(&log);
                let barrier = Arc::clone(&barrier);
                handles.push(std::thread::spawn(move || {
                    barrier.wait();
                    for n in 0..per_writer {
                        log.write_line(&format!("writer {writer} line {n}\n"));
                    }
                }));
            }
            for handle in handles {
                handle.join().expect("a writer thread");
            }

            let mut seen: Vec<String> = fs::read_to_string(dir.path().join("dux.log"))
                .unwrap()
                .lines()
                .map(str::to_string)
                .collect();
            let mut n = 1;
            while dir.path().join(format!("dux.log.{n}")).exists() {
                seen.extend(
                    fs::read_to_string(dir.path().join(format!("dux.log.{n}")))
                        .unwrap()
                        .lines()
                        .map(str::to_string),
                );
                n += 1;
            }
            assert!(n > 2, "the test is only meaningful if it actually rotated");

            let mut expected: Vec<String> = (0..writers)
                .flat_map(|writer| {
                    (0..per_writer).map(move |line| format!("writer {writer} line {line}"))
                })
                .collect();
            expected.sort();
            seen.sort();
            assert_eq!(
                seen, expected,
                "every line must appear exactly once and unbroken"
            );
        }

        #[test]
        fn the_size_is_seeded_from_a_log_that_already_exists() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("dux.log");
            let existing = "x".repeat(4096);
            fs::write(&path, &existing).unwrap();

            let log = log_in(dir.path(), settings(64, 3, false));
            assert_eq!(log.state.lock().unwrap().size, existing.len() as u64);

            log.write_line("the first line after the upgrade\n");
            assert_eq!(
                fs::read_to_string(dir.path().join("dux.log.1")).unwrap(),
                existing,
                "an oversized log must rotate on the very first write"
            );
        }

        #[test]
        fn a_reload_changes_the_limit_at_the_next_write() {
            let dir = tempfile::tempdir().unwrap();
            let log = log_in(dir.path(), settings(0, 3, false));
            for n in 1..=20 {
                log.write_line(&format!("line {n}\n"));
            }
            assert!(
                !dir.path().join("dux.log.1").exists(),
                "nothing may rotate while the limit is 0"
            );

            log.apply(settings(16, 3, false));
            log.write_line("after the reload\n");

            assert_eq!(
                fs::read_to_string(dir.path().join("dux.log")).unwrap(),
                "after the reload\n",
                "the new limit must apply to the very next line"
            );
            assert!(dir.path().join("dux.log.1").exists());
        }

        #[test]
        fn every_file_rotation_produces_is_owner_only() {
            let dir = tempfile::tempdir().unwrap();
            let log = log_in(dir.path(), settings(12, 3, true));
            for n in 1..=6 {
                write_and_settle(&log, &format!("line {n}\n"));
            }

            for entry in fs::read_dir(dir.path()).unwrap() {
                let entry = entry.unwrap();
                let mode = entry.metadata().unwrap().permissions().mode() & 0o777;
                assert_eq!(
                    mode & 0o077,
                    0,
                    "{} is not owner-only, got {mode:o}",
                    entry.path().display()
                );
            }
        }
    }
}
