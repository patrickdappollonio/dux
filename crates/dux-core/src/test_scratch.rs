//! Scratch directories for tests that leave background work running.
//!
//! Tests leave worker threads running that nobody joins (a dispatch running a
//! subprocess, a config write, an engine actor still draining), so one may still
//! be writing into its directory at the moment the test's guard drops. A single
//! `remove_dir_all` then loses the race with ENOTEMPTY and `TempDir` swallows the
//! error, leaving the directory behind in the temp directory. [`ScratchDir`]
//! retries for a bounded window instead. The window is a heuristic, not a
//! guarantee: a worker still writing after it closes wins, and the guard then
//! names the leftover path on stderr, so a leak is visible rather than silent.
//!
//! Use it for the shared fixtures that boot an engine or spawn workers against a
//! directory; a plain `tempfile::tempdir()` is fine where nothing outlives the
//! test's own statements.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long a drop keeps trying before it gives up and reports the leak.
pub(crate) const REMOVAL_WINDOW: Duration = Duration::from_secs(2);
/// The pause between attempts, long enough for a writer to finish a file.
const RETRY_PAUSE: Duration = Duration::from_millis(25);

/// One scratch directory, removed with retries when it drops.
pub struct ScratchDir(Option<tempfile::TempDir>);

impl ScratchDir {
    /// A fresh directory in the system temp directory.
    pub fn new() -> Self {
        Self::from(tempfile::tempdir().expect("create a test scratch directory"))
    }

    pub fn path(&self) -> &Path {
        self.0
            .as_ref()
            .expect("a live scratch dir always holds its directory")
            .path()
    }

    /// Hand the directory over to the caller's removal: `TempDir`'s own single,
    /// silent attempt must not run.
    fn take_path(&mut self) -> Option<PathBuf> {
        self.0.take().map(tempfile::TempDir::keep)
    }
}

impl Default for ScratchDir {
    fn default() -> Self {
        Self::new()
    }
}

impl From<tempfile::TempDir> for ScratchDir {
    fn from(dir: tempfile::TempDir) -> Self {
        Self(Some(dir))
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        if let Some(path) = self.take_path() {
            remove_all_by(vec![path], Instant::now() + REMOVAL_WINDOW);
        }
    }
}

/// Several scratch directories owned together (a test app's config root and the
/// repositories a test hands it), removed against ONE shared deadline, so a
/// fixture holding N of them waits at most one window rather than N.
#[derive(Default)]
pub struct ScratchDirs(Vec<ScratchDir>);

impl ScratchDirs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, dir: impl Into<ScratchDir>) {
        self.0.push(dir.into());
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl<D: Into<ScratchDir>> From<D> for ScratchDirs {
    fn from(dir: D) -> Self {
        let mut dirs = Self::new();
        dirs.push(dir);
        dirs
    }
}

impl Drop for ScratchDirs {
    fn drop(&mut self) {
        let paths = self
            .0
            .iter_mut()
            .filter_map(ScratchDir::take_path)
            .collect();
        remove_all_by(paths, Instant::now() + REMOVAL_WINDOW);
    }
}

/// Try every path each round until all are gone or `deadline` passes, then name
/// whatever is left on stderr.
fn remove_all_by(mut pending: Vec<PathBuf>, deadline: Instant) {
    loop {
        let mut last_errors = Vec::new();
        pending.retain(|path| match std::fs::remove_dir_all(path) {
            Ok(()) => false,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
            Err(err) => {
                last_errors.push(err);
                true
            }
        });
        if pending.is_empty() {
            return;
        }
        if Instant::now() >= deadline {
            for (path, err) in pending.iter().zip(last_errors) {
                eprintln!(
                    "test scratch directory {} was left behind: {err}",
                    path.display()
                );
            }
            return;
        }
        std::thread::sleep(RETRY_PAUSE);
    }
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc;

    use super::*;

    /// A worker still writing into a scratch directory when its guard drops
    /// must not leave the directory behind.
    #[test]
    fn a_scratch_dir_is_removed_while_a_worker_is_still_writing_into_it() {
        let guard = ScratchDir::new();
        let path = guard.path().to_path_buf();
        let (first_write_tx, first_write_rx) = mpsc::channel();
        let writer_path = path.clone();
        let writer = std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_millis(300);
            let mut n = 0u64;
            while Instant::now() < until {
                if std::fs::write(writer_path.join(format!("f{n}")), b"x").is_ok() && n == 0 {
                    let _ = first_write_tx.send(());
                }
                n += 1;
            }
        });
        first_write_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("the writer started");
        drop(guard);
        writer.join().expect("the writer finished");
        assert!(
            !path.exists(),
            "the scratch directory {} outlived its guard",
            path.display()
        );
    }

    /// A directory nothing can remove (a read-only subdirectory holding a file)
    /// is the worst case for the window. Several of them held together must
    /// cost ONE window, not one each.
    #[test]
    fn several_stuck_directories_share_one_removal_deadline() {
        // Root ignores directory permissions, so nothing would be stuck.
        if std::process::Command::new("id")
            .arg("-u")
            .output()
            .is_ok_and(|out| out.stdout.starts_with(b"0\n"))
        {
            return;
        }
        let mut dirs = ScratchDirs::new();
        let mut locked = Vec::new();
        for _ in 0..3 {
            let dir = ScratchDir::new();
            let sub = dir.path().join("locked");
            std::fs::create_dir(&sub).unwrap();
            std::fs::write(sub.join("file"), b"x").unwrap();
            std::fs::set_permissions(&sub, std::fs::Permissions::from_mode(0o555)).unwrap();
            locked.push((dir.path().to_path_buf(), sub));
            dirs.push(dir);
        }

        let started = Instant::now();
        drop(dirs);
        let elapsed = started.elapsed();

        for (root, sub) in &locked {
            std::fs::set_permissions(sub, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::remove_dir_all(root).unwrap();
        }
        assert!(
            elapsed < REMOVAL_WINDOW + REMOVAL_WINDOW / 2,
            "three stuck directories took {elapsed:?}, more than one shared window"
        );
    }
}
