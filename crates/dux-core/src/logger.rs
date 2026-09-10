use std::cell::Cell;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, OnceLock, PoisonError};

use chrono::Utc;

use crate::config::{DuxPaths, LoggingConfig};

static LOGGER: OnceLock<Logger> = OnceLock::new();

/// The rotation settings the process-global logger reads, published separately
/// from [`LOGGER`] so a reload can retune them through [`set_rotation`] whether
/// or not a log file was ever opened.
static ROTATION: LazyLock<Arc<RotationCell>> = LazyLock::new(|| {
    Arc::new(RotationCell::new(RotationSettings::from_config(
        &LoggingConfig::default(),
    )))
});

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
    set_rotation(config);
    if let Ok(log) = RotatingLog::open(path.clone(), Arc::clone(&ROTATION)) {
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
    ROTATION.store(RotationSettings::from_config(config));
}

/// The rotation settings the logger would read right now, as `(max_bytes, keep,
/// compress)`.
///
/// This is the cell the real logger reads on every line, so a test can watch a
/// config reload reach it. Held under
/// [`level_test_guard`][crate::logger::level_test_guard], like every other test
/// that moves process-global logger state.
#[cfg(test)]
pub(crate) fn rotation_for_test() -> (u64, u32, bool) {
    let settings = ROTATION.load();
    (settings.max_bytes, settings.keep, settings.compress)
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
            keep: crate::config::normalized_log_keep(config.keep),
            compress: config.compress,
        }
    }
}

/// The live rotation settings, shared so a reload can retune the log without
/// waiting on the write lock.
#[derive(Debug)]
struct RotationCell {
    max_bytes: AtomicU64,
    keep: AtomicU32,
    compress: AtomicBool,
}

impl RotationCell {
    fn new(settings: RotationSettings) -> Self {
        Self {
            max_bytes: AtomicU64::new(settings.max_bytes),
            keep: AtomicU32::new(settings.keep),
            compress: AtomicBool::new(settings.compress),
        }
    }

    fn store(&self, settings: RotationSettings) {
        self.max_bytes.store(settings.max_bytes, Ordering::Relaxed);
        self.keep.store(settings.keep, Ordering::Relaxed);
        self.compress.store(settings.compress, Ordering::Relaxed);
    }

    fn load(&self) -> RotationSettings {
        RotationSettings {
            max_bytes: self.max_bytes.load(Ordering::Relaxed),
            keep: self.keep.load(Ordering::Relaxed),
            compress: self.compress.load(Ordering::Relaxed),
        }
    }
}

/// Everything a write touches, behind one lock.
struct LogState {
    file: std::fs::File,
    /// Tracked rather than stat'd per line, since dux is the only writer.
    size: u64,
    compressing: bool,
    /// Set when a rotation could not complete. Rotation is never attempted
    /// again for the life of the process, because a size that stays over the
    /// limit would otherwise re-run the sweep on every single line and empty
    /// the directory in `keep` of them.
    rotation_broken: bool,
}

/// A file's identity, so a name can be checked for still meaning the same file.
type FileId = (u64, u64);

fn file_id(meta: &fs::Metadata) -> FileId {
    (meta.dev(), meta.ino())
}

fn lock_state(state: &Mutex<LogState>) -> MutexGuard<'_, LogState> {
    // A panic in another writer must not silently stop logging: the state it
    // left behind is a file handle and a byte count, both still usable.
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

thread_local! {
    /// Set while this thread is inside [`RotatingLog::write_line`]. The panic
    /// hook logs, and a panic raised under the write lock would otherwise
    /// re-enter and deadlock on it.
    static WRITING: Cell<bool> = const { Cell::new(false) };
}

/// Claims the thread for one write. `None` means a write is already in progress
/// further up this thread's stack, and the nested line is dropped.
struct WriteGuard;

impl WriteGuard {
    fn acquire() -> Option<Self> {
        WRITING.with(|writing| {
            if writing.get() {
                return None;
            }
            writing.set(true);
            Some(Self)
        })
    }
}

impl Drop for WriteGuard {
    fn drop(&mut self) {
        WRITING.with(|writing| writing.set(false));
    }
}

/// An append-only log that rotates itself by size.
///
/// dux rotates rather than leaving it to logrotate, because the log lives in the
/// user's config directory on a laptop rather than in `/var/log` on a machine
/// with a cron. Rotation happens on a write and only on a write.
struct RotatingLog {
    path: PathBuf,
    state: Arc<Mutex<LogState>>,
    rotation: Arc<RotationCell>,
    /// Test-only: forces the post-rotation reopen to fail. No filesystem shape
    /// builds that branch without also breaking the rename before it.
    #[cfg(test)]
    fail_reopen: AtomicBool,
}

impl RotatingLog {
    fn open(path: PathBuf, rotation: Arc<RotationCell>) -> std::io::Result<Self> {
        // Rotation renames the log, which for a symlinked `logging.path` would
        // move the LINK and orphan the file the user pointed it at, so the link
        // is resolved once here and the numbered copies land beside the target.
        let path = match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => fs::canonicalize(&path).unwrap_or(path),
            _ => path,
        };
        let file = open_log_file(&path)?;
        // Seeded from the file on disk, so a log that grew past the limit under
        // an older dux rotates on the first line rather than after another
        // whole limit's worth.
        let size = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        Ok(Self {
            path,
            state: Arc::new(Mutex::new(LogState {
                file,
                size,
                compressing: false,
                rotation_broken: false,
            })),
            rotation,
            #[cfg(test)]
            fail_reopen: AtomicBool::new(false),
        })
    }

    /// Append one line, rotating first if it would not fit.
    ///
    /// The whole line is written under one lock, so a rotation can never land
    /// between two halves of a line and concurrent writers cannot interleave.
    fn write_line(&self, line: &str) {
        let Some(_guard) = WriteGuard::acquire() else {
            return;
        };
        let bytes = line.as_bytes();
        let mut state = lock_state(&self.state);
        let settings = self.rotation.load();
        // A line longer than the whole limit is written where it is: rotating an
        // empty file would push a real copy out of the keep window to make room
        // for nothing.
        if settings.max_bytes > 0
            && !state.rotation_broken
            && state.size > 0
            && state.size.saturating_add(bytes.len() as u64) > settings.max_bytes
        {
            self.rotate(&mut state, settings);
        }

        // Counted by what actually reached the file: a short write would
        // otherwise leave the tracked size ahead of or behind the real one, and
        // every later rotation decision is made from it.
        let mut written = 0usize;
        while written < bytes.len() {
            match state.file.write(&bytes[written..]) {
                Ok(0) => break,
                Ok(n) => written += n,
                Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => break,
            }
        }
        state.size = state.size.saturating_add(written as u64);
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

    /// The positions actually on disk, ascending, and the abandoned temporary
    /// files found alongside them.
    ///
    /// Read from the directory rather than probed position by position: a set
    /// with a hole in it would leave everything above the hole orphaned, and
    /// probing costs one syscall per position, which a large `keep` turns into a
    /// stall on every line.
    fn scan(&self) -> (Vec<u32>, Vec<PathBuf>) {
        let (Some(dir), Some(base)) = (self.path.parent(), self.path.file_name()) else {
            return (Vec::new(), Vec::new());
        };
        let prefix = format!("{}.", base.to_string_lossy());
        let mut positions = Vec::new();
        let mut temps = Vec::new();
        let Ok(entries) = fs::read_dir(dir) else {
            return (positions, temps);
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(rest) = name.strip_prefix(&prefix) else {
                continue;
            };
            if rest.ends_with(".tmp") {
                temps.push(entry.path());
                continue;
            }
            let digits = rest.strip_suffix(".gz").unwrap_or(rest);
            if let Ok(n) = digits.parse::<u32>() {
                positions.push(n);
            }
        }
        positions.sort_unstable();
        positions.dedup();
        (positions, temps)
    }

    /// Shift the numbered copies up, move the live log into position 1, and open
    /// a fresh one. Called with the state lock held.
    fn rotate(&self, state: &mut LogState, settings: RotationSettings) {
        let keep = settings.keep;
        let (mut positions, temps) = self.scan();

        // A temporary left behind by a killed process is dead weight, and the
        // name is reused, so it goes now rather than blocking the next gzip.
        for temp in temps {
            let _ = fs::remove_file(temp);
        }

        // Everything at or above `keep` is past the end, which also cleans up
        // what a larger `keep` left behind when the user lowered it.
        let first_doomed = if keep == 0 { 1 } else { keep };
        positions.retain(|&n| {
            if n < first_doomed {
                return true;
            }
            let _ = fs::remove_file(self.numbered(n));
            let _ = fs::remove_file(self.compressed(n));
            false
        });

        // Highest first, or a shift would land on a name still in use. A copy
        // may be plain (not compressed yet, or compression off) or gzipped, so
        // both spellings move or a mixed set loses its ordering.
        for &n in positions.iter().rev() {
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
            static REPORTED: std::sync::Once = std::sync::Once::new();
            report_rotation_failure(
                &REPORTED,
                &format!(
                    "dux could not rotate the log at {}: {err}. Logging continues to the same file",
                    self.path.display()
                ),
            );
            state.rotation_broken = true;
            return;
        }

        match self.reopen() {
            Ok(file) => {
                state.file = file;
                state.size = 0;
            }
            Err(err) => {
                // The handle still points at the file that was just renamed, so
                // putting it back under the live name keeps later lines where a
                // reader expects them.
                let restored = fs::rename(self.numbered(1), &self.path).is_ok();
                static REPORTED: std::sync::Once = std::sync::Once::new();
                report_rotation_failure(
                    &REPORTED,
                    &format!(
                        "dux rotated the log at {} but could not open a new one: {err}. \
                         Logging continues to the same file and dux will not rotate again \
                         this run (the rotated copy was {})",
                        self.path.display(),
                        if restored {
                            "moved back"
                        } else {
                            "left where it was"
                        }
                    ),
                );
                state.rotation_broken = true;
                return;
            }
        }

        if settings.compress && keep > 0 && !state.compressing {
            let pending: Vec<PathBuf> = self
                .scan()
                .0
                .into_iter()
                .map(|n| self.numbered(n))
                .filter(|plain| plain.exists())
                .collect();
            if !pending.is_empty() {
                state.compressing = true;
                spawn_compression(pending, Arc::clone(&self.state));
            }
        }
    }

    fn reopen(&self) -> std::io::Result<std::fs::File> {
        #[cfg(test)]
        if self.fail_reopen.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("forced reopen failure"));
        }
        open_log_file(&self.path)
    }
}

/// Gzip each plain rotated copy on a thread of its own, then release the claim.
///
/// One thread at a time: a rotation while this runs queues nothing, because the
/// next rotation's own sweep picks up whatever is still plain.
fn spawn_compression(
    pending: Vec<PathBuf>,
    state: Arc<Mutex<LogState>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        for plain in pending {
            let mut target = plain.clone().into_os_string();
            target.push(".gz");
            if let Err(err) = compress_file(&plain, Path::new(&target), &state) {
                warn(&format!(
                    "could not compress the rotated log at {}: {err}. \
                     It was left uncompressed and dux will try again on the next rotation",
                    plain.display()
                ));
            }
        }
        lock_state(&state).compressing = false;
    })
}

/// Gzip `source` to `target` through a temporary file in the same directory, so
/// a crash or a failure never leaves a half-written `.gz` in place of a readable
/// plain copy.
fn compress_file(source: &Path, target: &Path, state: &Mutex<LogState>) -> std::io::Result<()> {
    let input = fs::File::open(source)?;
    let id = file_id(&input.metadata()?);
    compress_open_file(source, input, id, target, state)
}

/// The body of [`compress_file`], entered with the source already open so its
/// identity is fixed before anything else can touch the name.
///
/// The gzip runs with no lock held, and the tightening and the caller's failure
/// warning can themselves write a log line and so rotate. That is safe only
/// because the rename and the delete happen inside the same lock a rotation
/// holds, keyed on the source's device and inode: a name that now points at a
/// different file is somebody else's, and touching it would delete a copy the
/// rotation just put there.
fn compress_open_file(
    source: &Path,
    mut input: fs::File,
    id: FileId,
    target: &Path,
    state: &Mutex<LogState>,
) -> std::io::Result<()> {
    let mut temp = target.to_path_buf().into_os_string();
    temp.push(format!(".{}.tmp", std::process::id()));
    let temp = PathBuf::from(temp);
    let _ = fs::remove_file(&temp);

    let result = (|| -> std::io::Result<()> {
        let file = create_private_temp(&temp)?;
        crate::file_modes::restrict_to_owner_best_effort(&temp, "compressed log file");
        let mut encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
        // Streamed rather than read whole: the source is a full rotation's worth
        // of log and there is no reason to hold it all in memory.
        std::io::copy(&mut input, &mut encoder)?;
        encoder.finish()?.sync_all()?;

        let _guard = lock_state(state);
        let still_ours = fs::symlink_metadata(source)
            .map(|meta| file_id(&meta) == id)
            .unwrap_or(false);
        if !still_ours {
            return Err(std::io::Error::other(
                "the rotated copy moved while it was being compressed",
            ));
        }
        fs::rename(&temp, target)?;
        fs::remove_file(source)
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Create the gzip's temporary file owner-only from the first instant it exists,
/// rather than under the umask with a tightening a moment later.
fn create_private_temp(path: &Path) -> std::io::Result<fs::File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(crate::file_modes::PRIVATE_FILE_MODE)
        .open(path)
}

/// Say once, on stderr, that rotation is not working. Once per call site, since
/// a rename failure must not silence the worse reopen message, and on stderr
/// because the log file is exactly what is in doubt. `writeln!` rather than
/// `eprintln!`, which panics on a closed stderr while the write lock is held.
fn report_rotation_failure(reported: &'static std::sync::Once, message: &str) {
    reported.call_once(|| {
        let _ = writeln!(std::io::stderr(), "{message}");
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
            RotatingLog::open(dir.join("dux.log"), Arc::new(RotationCell::new(settings)))
                .expect("open the log")
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

            log.rotation.store(settings(16, 3, false));
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

        /// Every path under the log's directory, so a test can say what the
        /// whole set looks like rather than probing the names it expects.
        fn names_in(dir: &Path) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(dir)
                .unwrap()
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }

        /// The line the compression thread must never delete is the one a
        /// rotation put at its name while it was working. `exists()` cannot see
        /// the difference, so the check is on the source's device and inode, and
        /// it happens inside the same lock the rotation holds.
        ///
        /// Built with that lock held for the whole swap, so the ordering is
        /// settled rather than raced: the thread has the source open before it
        /// starts, and cannot reach the rename until the swap is done.
        #[test]
        fn compression_never_touches_a_copy_a_rotation_put_at_its_name() {
            let dir = tempfile::tempdir().unwrap();
            let log = log_in(dir.path(), settings(0, 3, false));
            let source = dir.path().join("dux.log.1");
            let target = dir.path().join("dux.log.1.gz");
            fs::write(&source, b"the copy being compressed\n").unwrap();

            let input = fs::File::open(&source).unwrap();
            let id = file_id(&input.metadata().unwrap());

            let guard = lock_state(&log.state);
            let compressor = {
                let state = Arc::clone(&log.state);
                let (source, target) = (source.clone(), target.clone());
                std::thread::spawn(move || compress_open_file(&source, input, id, &target, &state))
            };

            // What a rotation does under that lock: the copy moves up a place
            // and a newer one takes the name it had.
            fs::rename(&source, dir.path().join("dux.log.2")).unwrap();
            fs::write(&source, b"the copy a rotation just made\n").unwrap();
            drop(guard);

            assert!(
                compressor.join().expect("the compression thread").is_err(),
                "compressing a file that is no longer at that name must be refused"
            );
            assert_eq!(
                fs::read(&source).unwrap(),
                b"the copy a rotation just made\n",
                "the newer copy must still be there"
            );
            assert_eq!(
                fs::read(dir.path().join("dux.log.2")).unwrap(),
                b"the copy being compressed\n",
                "the copy that shifted up must be untouched"
            );
            assert!(
                !target.exists(),
                "stale bytes must not be written under the newer copy's name"
            );
            assert_eq!(
                names_in(dir.path()),
                vec!["dux.log", "dux.log.1", "dux.log.2"],
                "the abandoned temporary must be cleaned up"
            );
        }

        /// A rotation that renames the log away and then cannot open a new one
        /// must stop rotating for good. Left alone, the tracked size stays over
        /// the limit, so every following line re-runs the sweep and the whole
        /// directory is emptied in `keep` lines while dux appends to a file
        /// nobody can find.
        #[test]
        fn a_reopen_failure_stops_rotating_and_keeps_every_copy() {
            let dir = tempfile::tempdir().unwrap();
            let log = log_in(dir.path(), settings(8, 3, false));
            fs::write(dir.path().join("dux.log.1"), "older one\n").unwrap();
            fs::write(dir.path().join("dux.log.2"), "oldest one\n").unwrap();
            log.fail_reopen.store(true, Ordering::Relaxed);

            for n in 1..=4 {
                log.write_line(&format!("line {n}\n"));
            }

            assert!(
                log.state.lock().unwrap().rotation_broken,
                "a rotation dux could not finish must not be attempted again"
            );
            let mut seen: Vec<String> = Vec::new();
            for name in names_in(dir.path()) {
                seen.extend(
                    fs::read_to_string(dir.path().join(name))
                        .unwrap()
                        .lines()
                        .map(str::to_string),
                );
            }
            seen.sort();
            let mut expected = vec![
                "line 1".to_string(),
                "line 2".to_string(),
                "line 3".to_string(),
                "line 4".to_string(),
                "older one".to_string(),
                "oldest one".to_string(),
            ];
            expected.sort();
            assert_eq!(seen, expected, "no line may be lost by a failed rotation");
        }

        /// The panic hook logs, so a panic raised while the write lock is held
        /// would re-enter and deadlock on it. The nested line is dropped instead.
        #[test]
        fn a_nested_write_is_dropped_so_the_panic_hook_cannot_re_enter() {
            let dir = tempfile::tempdir().unwrap();
            let log = log_in(dir.path(), settings(0, 3, false));

            let outer = WriteGuard::acquire().expect("the first write claims the thread");
            log.write_line("a line the panic hook tried to write\n");
            assert_eq!(
                fs::read_to_string(dir.path().join("dux.log")).unwrap(),
                "",
                "a write nested inside another must go nowhere"
            );

            drop(outer);
            log.write_line("a line written once the thread is free\n");
            assert_eq!(
                fs::read_to_string(dir.path().join("dux.log")).unwrap(),
                "a line written once the thread is free\n"
            );
        }

        /// A set with a hole in it used to leave everything above the hole
        /// behind, because the sweep stopped at the first missing position.
        #[test]
        fn the_sweep_cleans_positions_above_keep_across_a_hole() {
            let dir = tempfile::tempdir().unwrap();
            let log = log_in(dir.path(), settings(8, 3, false));
            for name in ["dux.log.1", "dux.log.2", "dux.log.5", "dux.log.6.gz"] {
                fs::write(dir.path().join(name), "old\n").unwrap();
            }

            log.write_line("line one\n");
            log.write_line("line two\n");

            // The two survivors shift up and the live log takes position one;
            // the pair above the hole is what must be gone.
            assert_eq!(
                names_in(dir.path()),
                vec!["dux.log", "dux.log.1", "dux.log.2", "dux.log.3"],
                "everything at or above keep must go, hole or no hole"
            );
        }

        #[test]
        fn the_sweep_removes_a_temporary_an_earlier_run_abandoned() {
            let dir = tempfile::tempdir().unwrap();
            let log = log_in(dir.path(), settings(8, 3, false));
            fs::write(dir.path().join("dux.log.1.gz.999999.tmp"), "half a gzip").unwrap();

            log.write_line("line one\n");
            log.write_line("line two\n");

            assert_eq!(names_in(dir.path()), vec!["dux.log", "dux.log.1"]);
        }

        /// Rotation reads the directory once instead of probing every position
        /// up to `keep`, so what a rotation costs follows the number of copies
        /// that exist rather than the setting.
        ///
        /// Measured against a small `keep` in the same run rather than against a
        /// fixed duration, because the whole suite runs in parallel and a wall
        /// clock reading here is mostly other tests. Probing ran two failing
        /// renames per position per rotation, which is three orders of magnitude
        /// more work than the bound below allows.
        #[test]
        fn a_large_keep_does_not_slow_rotation_down() {
            fn rotate_a_hundred_times(keep: u32) -> std::time::Duration {
                let dir = tempfile::tempdir().unwrap();
                let log = log_in(dir.path(), settings(8, keep, false));
                let started = std::time::Instant::now();
                for n in 1..=100 {
                    log.write_line(&format!("line {n}\n"));
                }
                started.elapsed()
            }

            let small = rotate_a_hundred_times(3);
            let large = rotate_a_hundred_times(10_000);
            assert!(
                large < small * 20 + std::time::Duration::from_millis(500),
                "keep = 10000 took {large:?} against {small:?} for keep = 3"
            );
        }

        /// The temporary is created owner-only rather than under the umask and
        /// tightened a moment later. Vacuous under a umask that already denies
        /// group and other, which is why the assertion is on the exact mode.
        #[test]
        fn the_gzip_temporary_is_owner_only_from_the_instant_it_exists() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("dux.log.1.gz.tmp");
            drop(create_private_temp(&path).expect("create the temporary"));
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                crate::file_modes::PRIVATE_FILE_MODE
            );
        }

        /// A line bigger than the whole limit is written where it is. Rotating
        /// an empty file to make room for it would push a real copy out of the
        /// keep window and gain nothing.
        #[test]
        fn a_line_larger_than_the_limit_is_written_whole() {
            let dir = tempfile::tempdir().unwrap();
            let log = log_in(dir.path(), settings(10, 3, false));
            let huge = format!("{}\n", "x".repeat(200));

            log.write_line(&huge);
            assert_eq!(
                fs::read_to_string(dir.path().join("dux.log")).unwrap(),
                huge
            );
            assert_eq!(
                names_in(dir.path()),
                vec!["dux.log"],
                "no empty copy may be created to make room"
            );

            log.write_line("the next line\n");
            assert_eq!(
                fs::read_to_string(dir.path().join("dux.log")).unwrap(),
                "the next line\n",
                "the oversized line leaves the file over the limit, so the next one rotates"
            );
        }

        /// A symlinked `logging.path` is resolved once at open, or rotation
        /// would rename the LINK aside and orphan the file the user pointed it
        /// at.
        #[test]
        fn a_symlinked_path_rotates_beside_its_target() {
            let dir = tempfile::tempdir().unwrap();
            let target = dir.path().join("real.log");
            fs::write(&target, "").unwrap();
            let link = dir.path().join("dux.log");
            std::os::unix::fs::symlink(&target, &link).unwrap();

            let log = RotatingLog::open(
                link.clone(),
                Arc::new(RotationCell::new(settings(8, 3, false))),
            )
            .unwrap();
            log.write_line("line one\n");
            log.write_line("line two\n");

            assert!(
                link.symlink_metadata().unwrap().file_type().is_symlink(),
                "the link itself must still be a link"
            );
            assert_eq!(
                fs::read_to_string(dir.path().join("real.log.1")).unwrap(),
                "line one\n",
                "the copy must land beside the target"
            );
            assert_eq!(fs::read_to_string(&target).unwrap(), "line two\n");
        }

        /// A panic in one writer must not stop the log: the state it left behind
        /// is a handle and a byte count, both still usable.
        #[test]
        fn a_poisoned_lock_still_writes() {
            let dir = tempfile::tempdir().unwrap();
            let log = Arc::new(log_in(dir.path(), settings(0, 3, false)));
            let poisoner = {
                let log = Arc::clone(&log);
                std::thread::spawn(move || {
                    let _held = log.state.lock().unwrap();
                    panic!("poison the lock");
                })
            };
            assert!(poisoner.join().is_err(), "the poisoning thread must panic");
            assert!(log.state.is_poisoned());

            log.write_line("after the poisoning\n");
            assert_eq!(
                fs::read_to_string(dir.path().join("dux.log")).unwrap(),
                "after the poisoning\n"
            );
        }
    }
}
