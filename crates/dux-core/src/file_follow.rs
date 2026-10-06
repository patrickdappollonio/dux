//! Reading a log file the way a person watches it: the last lines, then every
//! complete line appended after, across the rotations that move the file away.
//!
//! The terminal UI's server log viewer and the server's log route both follow
//! `server.log` through [`FileFollower`].

use std::time::Duration;

/// How often the reader thread looks at `server.log` for new lines. Wall-clock,
/// and short enough that a line reads as live without the thread spinning.
pub const FOLLOW_INTERVAL: Duration = Duration::from_millis(200);

/// What a log read found: the lines wanted, and how many complete lines the
/// file held, which is where a later read picks up from.
///
/// With a cursor, `since` complete lines are skipped (lines count from one, so
/// a cursor is the count a previous read answered); with none, the last `tail`
/// lines are taken. A cursor past the end, which a rotation causes, gets no
/// lines and the file's current count.
pub fn open_log(
    path: &std::path::Path,
    since: Option<u64>,
    tail: usize,
) -> (Vec<String>, u64, FileFollower) {
    let (follower, all) = FileFollower::start(path.to_path_buf(), usize::MAX);
    let cursor = all.len() as u64;
    let skip = match since {
        Some(since) => usize::try_from(since).unwrap_or(usize::MAX).min(all.len()),
        None => all.len().saturating_sub(tail),
    };
    let lines = all.into_iter().skip(skip).collect();
    (lines, cursor, follower)
}

/// How much of the file one backward read takes while looking for the last
/// lines.
const TAIL_CHUNK: u64 = 64 * 1024;

/// Reads `server.log` from where it left off: first the last lines, then
/// whatever was appended. It keeps the file it is reading open, so when a
/// rotation moves that file away it finishes the old file's remaining bytes
/// first and only then switches to the file the name now means, starting it
/// from its first line. Two rotations inside one poll (over 10 MiB written in
/// 200 ms at the default size) lose the file in between; that is accepted.
pub struct FileFollower {
    path: std::path::PathBuf,
    file: Option<std::fs::File>,
    position: u64,
    /// The bytes of a line the writer has not finished.
    partial: Vec<u8>,
}

fn identity_of(meta: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (meta.dev(), meta.ino())
}

impl FileFollower {
    /// Open at the end of the file's last `last` lines. A file that does not
    /// exist yet is followed from its first byte once it does.
    pub fn start(path: std::path::PathBuf, last: usize) -> (Self, Vec<String>) {
        use std::io::{Read, Seek, SeekFrom};
        let mut follower = Self {
            path,
            file: None,
            position: 0,
            partial: Vec::new(),
        };
        let Ok(mut file) = std::fs::File::open(&follower.path) else {
            return (follower, Vec::new());
        };
        let Ok(meta) = file.metadata() else {
            return (follower, Vec::new());
        };
        let length = meta.len();
        let mut start = length;
        let mut bytes: Vec<u8> = Vec::new();
        let mut newlines = 0usize;
        while start > 0 && newlines <= last {
            let step = TAIL_CHUNK.min(start);
            start -= step;
            let mut chunk = vec![0u8; step as usize];
            if file.seek(SeekFrom::Start(start)).is_err() || file.read_exact(&mut chunk).is_err() {
                break;
            }
            newlines += chunk.iter().filter(|b| **b == b'\n').count();
            chunk.extend_from_slice(&bytes);
            bytes = chunk;
        }
        // The read began part-way into a line unless it reached the file's start.
        if start > 0 {
            if let Some(newline) = bytes.iter().position(|b| *b == b'\n') {
                bytes.drain(..=newline);
            } else {
                bytes.clear();
            }
        }
        // What follows the last newline is a line still being written.
        let complete = bytes
            .iter()
            .rposition(|b| *b == b'\n')
            .map_or(0, |newline| newline + 1);
        follower.position = length.saturating_sub((bytes.len() - complete) as u64);
        follower.file = Some(file);
        let mut lines = split_lines(&bytes[..complete]);
        let excess = lines.len().saturating_sub(last);
        lines.drain(..excess);
        (follower, lines)
    }

    /// Read what the held file has past `position`, restarting it if it shrank.
    fn read_held(&mut self) {
        use std::io::{Read, Seek, SeekFrom};
        let Some(file) = self.file.as_mut() else {
            return;
        };
        let Ok(meta) = file.metadata() else {
            return;
        };
        if meta.len() < self.position {
            self.position = 0;
            self.partial.clear();
        }
        if meta.len() == self.position || file.seek(SeekFrom::Start(self.position)).is_err() {
            return;
        }
        let mut fresh = Vec::new();
        if file
            .take(meta.len() - self.position)
            .read_to_end(&mut fresh)
            .is_err()
        {
            return;
        }
        self.position += fresh.len() as u64;
        self.partial.extend_from_slice(&fresh);
    }

    /// The complete lines in `partial`, which keeps the unfinished tail.
    fn take_lines(&mut self) -> Vec<String> {
        let complete = self
            .partial
            .iter()
            .rposition(|b| *b == b'\n')
            .map_or(0, |newline| newline + 1);
        let lines = split_lines(&self.partial[..complete]);
        self.partial.drain(..complete);
        lines
    }

    /// The complete lines written since the last look.
    pub fn poll(&mut self) -> Vec<String> {
        self.poll_after_snapshot(|| {})
    }

    /// [`Self::poll`] with a hook that runs between the first read of the held
    /// file and the look at its name, so a test can write in that window.
    pub(crate) fn poll_after_snapshot(&mut self, between: impl FnOnce()) -> Vec<String> {
        // The file in hand first, to its end, so nothing written before a
        // rotation is lost to it.
        self.read_held();
        let mut lines = self.take_lines();
        between();
        // Then the name: another file under it means a rotation.
        let renamed = std::fs::File::open(&self.path).ok().filter(|opened| {
            let current = opened.metadata().ok().map(|meta| identity_of(&meta));
            let held = self
                .file
                .as_ref()
                .and_then(|file| file.metadata().ok())
                .map(|meta| identity_of(&meta));
            current.is_some() && current != held
        });
        if let Some(next) = renamed {
            // What the old file got between the first read and the look at the
            // name is still its own.
            self.read_held();
            lines.extend(self.take_lines());
            self.file = Some(next);
            self.position = 0;
            // A line the old file never finished has no ending to wait for.
            self.partial.clear();
            self.read_held();
            lines.extend(self.take_lines());
        }
        lines
    }
}

fn split_lines(bytes: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The log viewer opens on the file's last lines and then picks up what is
    /// appended, never showing a line the writer has not finished.
    #[test]
    fn a_followed_file_gives_its_last_lines_or_those_after_a_cursor_then_the_complete_lines_appended()
     {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.log");
        std::fs::write(&path, "one\ntwo\nthree\nfour\nfive\n").unwrap();
        let (mut follower, first) = FileFollower::start(path.clone(), 3);
        assert_eq!(first, ["three", "four", "five"]);
        assert!(follower.poll().is_empty(), "nothing new yet");

        // The same file by cursor: lines are counted from one, the cursor
        // answered is the count of complete lines, and a cursor past the end
        // gets nothing and the count.
        let read = |since, tail| {
            let (lines, cursor, _) = open_log(&path, since, tail);
            (lines, cursor)
        };
        assert_eq!(read(None, 2), (vec!["four".into(), "five".into()], 5));
        assert_eq!(
            read(Some(3), 2),
            (vec!["four".into(), "five".into()], 5),
            "a cursor wins over the tail"
        );
        assert_eq!(read(Some(0), 2).0.len(), 5);
        assert_eq!(read(Some(5), 2), (vec![], 5));
        assert_eq!(read(Some(99), 2), (vec![], 5));

        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        std::io::Write::write_all(&mut file, b"six\nsev").unwrap();
        assert_eq!(
            follower.poll(),
            ["six"],
            "half a line waits for its newline"
        );
        assert_eq!(
            read(None, 1),
            (vec!["six".into()], 6),
            "an unfinished line is not counted either"
        );
        std::io::Write::write_all(&mut file, b"en\n").unwrap();
        assert_eq!(follower.poll(), ["seven"]);
    }

    /// A rotation moves the file away and starts a new one under the same name;
    /// the viewer carries on with the new file from its first line.
    #[test]
    fn a_followed_file_that_rotates_is_read_again_from_its_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.log");
        std::fs::write(&path, "old one\nold two\n").unwrap();
        let (mut follower, _) = FileFollower::start(path.clone(), 10);
        // A line written just before the rotation is still shown.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        std::io::Write::write_all(&mut file, b"last before rotation\n").unwrap();
        std::fs::rename(&path, dir.path().join("server.log.1")).unwrap();
        std::fs::write(&path, "fresh\n").unwrap();
        assert_eq!(follower.poll(), ["last before rotation", "fresh"]);

        // A line the old file gets after the first read of it, while the name is
        // being looked at, is shown too.
        let (mut follower, _) = FileFollower::start(path.clone(), 10);
        let rotated = dir.path().join("server.log.2");
        let lines = follower.poll_after_snapshot(|| {
            let mut old = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            std::io::Write::write_all(&mut old, b"late\n").unwrap();
            std::fs::rename(&path, &rotated).unwrap();
            std::fs::write(&path, "newest\n").unwrap();
        });
        assert_eq!(lines, ["late", "newest"]);
    }

    /// A log that does not exist yet is followed from its first byte once it
    /// does.
    #[test]
    fn a_followed_file_that_appears_later_is_read_from_the_start() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.log");
        let (mut follower, first) = FileFollower::start(path.clone(), 10);
        assert!(first.is_empty());
        std::fs::write(&path, "hello\n").unwrap();
        assert_eq!(follower.poll(), ["hello"]);
    }
}
