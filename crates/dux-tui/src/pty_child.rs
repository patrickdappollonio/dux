//! Runs one test of this binary again as a child inside a pseudo-terminal, so
//! the child reads a real terminal on stdin and the parent can type into it.
//! The child's test is ignored, so it runs only when a parent asks for it.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use dux_core::pty::PtyClient;

const CHILD_DIR_ENV: &str = "DUX_PTY_CHILD_DIR";

/// How long a child may take to start or to finish: a cold test binary on a
/// loaded machine is slow to start, and a wait for a condition costs nothing
/// when the condition comes early.
const CHILD_LIMIT: Duration = Duration::from_secs(60);

/// The directory the parent gave this child, or `None` when the test is
/// running as itself rather than as a child.
pub(crate) fn child_dir() -> Option<PathBuf> {
    std::env::var_os(CHILD_DIR_ENV).map(PathBuf::from)
}

pub(crate) struct PtyChild {
    pub(crate) dir: tempfile::TempDir,
    client: PtyClient,
}

impl PtyChild {
    /// Start `test` (its full path) as a child, with `setup` written to
    /// `<dir>/setup`, and wait for it to create `<dir>/ready`.
    pub(crate) fn start(test: &str, setup: &str) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("setup"), setup).expect("setup file");
        let exe = std::env::current_exe().expect("test binary");
        let command = format!(
            "{CHILD_DIR_ENV}='{}' exec '{}' --exact {test} --include-ignored --nocapture \
             --test-threads=1",
            dir.path().display(),
            exe.display()
        );
        let client = PtyClient::spawn("sh", &["-c".to_string(), command], dir.path(), 24, 80, 100)
            .expect("spawn the child test");
        wait_for_file(&dir.path().join("ready"), CHILD_LIMIT);
        Self { dir, client }
    }

    pub(crate) fn type_bytes(&self, bytes: &[u8]) {
        self.client.write_bytes(bytes).expect("type into the child");
    }

    /// Resize the child's terminal, which sends it SIGWINCH.
    pub(crate) fn resize(&self, rows: u16, cols: u16) {
        self.client.resize(rows, cols).expect("resize the child");
    }

    /// Tell the child to finish and return what it wrote to `<dir>/result`.
    pub(crate) fn finish(self) -> String {
        std::fs::write(self.dir.path().join("done"), b"").expect("done marker");
        let result = self.dir.path().join("result");
        wait_for_file(&result, CHILD_LIMIT);
        std::fs::read_to_string(result).expect("result")
    }
}

/// Wait until `ready` says yes, checking every few milliseconds, and fail the
/// test after `limit`.
pub(crate) fn wait_until(limit: Duration, what: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + limit;
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

pub(crate) fn wait_for_file(path: &Path, limit: Duration) {
    wait_until(limit, &path.display().to_string(), || path.exists());
}
