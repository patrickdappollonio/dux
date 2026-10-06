//! Reading a log file the way a person watches it: the last lines, then every
//! complete line appended after, across the rotations that move the file away.
//!
//! The terminal UI's server log viewer and the server's log route both follow
//! `server.log` through [`FileFollower`]. The file is never opened through a
//! symbolic link (the caller names the file the log's writer really opened,
//! with any link resolved once at its start), so a link put there later is
//! refused rather than read.

use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::os::unix::fs::{FileExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How often the reader thread looks at `server.log` for new lines. Wall-clock,
/// and short enough that a line reads as live without the thread spinning.
pub const FOLLOW_INTERVAL: Duration = Duration::from_millis(200);

/// The most lines one non-streaming read answers.
pub const MAX_LINES: usize = 10_000;

/// Open `path` for reading without following a symbolic link at its last
/// component.
fn open_nofollow(path: &Path) -> std::io::Result<std::fs::File> {
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

fn is_link_refusal(error: &std::io::Error) -> bool {
    error.raw_os_error() == Some(libc::ELOOP)
}

/// Why a log could not be read.
#[derive(Debug, PartialEq, Eq)]
pub enum LogError {
    /// The log's path is a symbolic link, which is never followed.
    Symlink(PathBuf),
    /// A cursor that is not `<generation>:<line>`.
    BadCursor,
    /// Anything else the file system said.
    Io(String),
}

impl std::fmt::Display for LogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogError::Symlink(path) => write!(
                f,
                "{} is a symbolic link now, and dux does not read a log through one: the log it \
                 writes is the file itself",
                path.display()
            ),
            LogError::BadCursor => f.write_str(
                "that cursor is not one dux gave: it reads <generation>:<line>, as an earlier \
                 answer's cursor did",
            ),
            LogError::Io(why) => write!(f, "the log could not be read: {why}"),
        }
    }
}

impl std::error::Error for LogError {}

/// What a log read found.
#[derive(Debug, PartialEq, Eq)]
pub struct LogRead {
    pub lines: Vec<String>,
    /// Where a later read picks up: `<generation>:<line>`, the file (renamed by a
    /// rotation) and the complete lines of it read so far.
    pub cursor: String,
    /// A sentence when the answer is not the whole of what was asked for: it
    /// was cut at the cap, or a rotation may have taken lines out of reach.
    pub note: Option<String>,
}

/// What to collect from a forward pass over a file.
enum Want {
    /// The last `n` complete lines.
    Tail(usize),
    /// At most `cap` complete lines after the first `skip`.
    After { skip: u64, cap: usize },
}

struct Scan {
    lines: Vec<String>,
    /// The complete lines in the file.
    total: u64,
    /// The offset just past the last complete line.
    position: u64,
}

fn trim_line(raw: &[u8]) -> String {
    let mut end = raw.len();
    while end > 0 && (raw[end - 1] == b'\n' || raw[end - 1] == b'\r') {
        end -= 1;
    }
    String::from_utf8_lossy(&raw[..end]).into_owned()
}

/// One forward pass over `file` from its start, keeping only what `want`
/// asks for, so a big file costs time and not memory.
fn scan(file: &std::fs::File, want: &Want) -> std::io::Result<Scan> {
    let mut reader = BufReader::new(file.try_clone()?);
    reader.seek(SeekFrom::Start(0))?;
    let mut kept = std::collections::VecDeque::new();
    let (mut total, mut position) = (0u64, 0u64);
    let mut raw = Vec::new();
    loop {
        raw.clear();
        let read = reader.read_until(b'\n', &mut raw)?;
        if read == 0 || !raw.ends_with(b"\n") {
            break;
        }
        position += read as u64;
        let index = total;
        total += 1;
        match want {
            Want::Tail(0) => {}
            Want::Tail(n) => {
                if kept.len() == *n {
                    kept.pop_front();
                }
                kept.push_back(trim_line(&raw));
            }
            Want::After { skip, cap } => {
                if index >= *skip && kept.len() < *cap {
                    kept.push_back(trim_line(&raw));
                }
            }
        }
    }
    Ok(Scan {
        lines: kept.into(),
        total,
        position,
    })
}

/// Names a file for as long as it is the same one, so neither a rotation nor a
/// reused inode number makes an old cursor look current.
fn generation(file: &std::fs::File) -> std::io::Result<String> {
    let mut head = [0u8; 512];
    let read = file.read_at(&mut head, 0)?;
    let head = &head[..read];
    let first = head
        .iter()
        .position(|b| *b == b'\n')
        .map_or(&head[..0], |end| &head[..end]);
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in first {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0100_0000_01b3);
    }
    Ok(format!("{:x}.{:x}", file.metadata()?.ino(), hash))
}

fn parse_cursor(text: &str) -> Result<(&str, u64), LogError> {
    let (generation, line) = text.split_once(':').ok_or(LogError::BadCursor)?;
    let line = line.parse::<u64>().map_err(|_| LogError::BadCursor)?;
    if generation.is_empty() || generation.contains(':') {
        return Err(LogError::BadCursor);
    }
    Ok((generation, line))
}

fn capped_note(cap: usize) -> String {
    format!(
        "Showing {cap} lines, the most one answer holds; ask again from the cursor for the rest."
    )
}

/// Read the log at `path`: at most `cap` lines after the cursor `since` (else the
/// last `tail`), and a follower that carries on from the end of the current file.
///
/// A cursor from before a rotation reads on in the uncompressed kept copy, or,
/// with it gone, from the new file's start with a note; a missing file is empty.
pub fn open_log(
    path: &Path,
    since: Option<&str>,
    tail: usize,
    cap: usize,
) -> Result<(LogRead, FileFollower), LogError> {
    let cursor = since.map(parse_cursor).transpose()?;
    let io = |error: std::io::Error| {
        if is_link_refusal(&error) {
            LogError::Symlink(path.to_path_buf())
        } else {
            LogError::Io(error.to_string())
        }
    };
    let file = match open_nofollow(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let read = LogRead {
                lines: Vec::new(),
                cursor: "0.0:0".to_string(),
                note: None,
            };
            return Ok((read, FileFollower::unopened(path.to_path_buf())));
        }
        Err(error) => return Err(io(error)),
    };
    let current = generation(&file).map_err(io)?;
    let mut note = None;
    let (lines, end, scanned) = match cursor {
        None => {
            if tail > cap {
                note = Some(format!(
                    "Showing the last {cap} lines, the most one answer holds; ask for lines \
                     after a cursor to read more."
                ));
            }
            let mut scanned = scan(&file, &Want::Tail(tail.min(cap))).map_err(io)?;
            let lines = std::mem::take(&mut scanned.lines);
            let total = scanned.total;
            (lines, format!("{current}:{total}"), scanned)
        }
        Some((generation, line)) if generation == current => {
            let mut scanned = scan(&file, &Want::After { skip: line, cap }).map_err(io)?;
            let lines = std::mem::take(&mut scanned.lines);
            let reached = line + lines.len() as u64;
            let end = if reached < scanned.total {
                note = Some(capped_note(cap));
                reached
            } else {
                scanned.total
            };
            (lines, format!("{current}:{end}"), scanned)
        }
        Some((generation, line)) => {
            let mut lines = Vec::new();
            if let Some(old) = kept_copy(path, generation, line, cap).map_err(io)? {
                if old.more {
                    let scanned = scan(&file, &Want::Tail(0)).map_err(io)?;
                    let cursor = format!("{generation}:{}", old.end);
                    return Ok(finish(
                        path,
                        file,
                        scanned,
                        old.lines,
                        cursor,
                        Some(capped_note(cap)),
                    ));
                }
                lines = old.lines;
            } else if line > 0 {
                note = Some(
                    "The log rotated since that cursor, and the lines after it are no longer \
                     readable, so some lines may have been skipped."
                        .to_string(),
                );
            }
            let room = cap - lines.len();
            let mut scanned = scan(&file, &Want::After { skip: 0, cap: room }).map_err(io)?;
            let fresh = std::mem::take(&mut scanned.lines);
            let taken = fresh.len() as u64;
            lines.extend(fresh);
            let end = if taken < scanned.total {
                note = Some(capped_note(cap));
                taken
            } else {
                scanned.total
            };
            (lines, format!("{current}:{end}"), scanned)
        }
    };
    Ok(finish(path, file, scanned, lines, end, note))
}

fn finish(
    path: &Path,
    file: std::fs::File,
    scanned: Scan,
    lines: Vec<String>,
    cursor: String,
    note: Option<String>,
) -> (LogRead, FileFollower) {
    let follower = FileFollower {
        path: path.to_path_buf(),
        file: Some(file),
        position: scanned.position,
        partial: Vec::new(),
        refused: false,
    };
    (
        LogRead {
            lines,
            cursor,
            note,
        },
        follower,
    )
}

struct KeptRead {
    lines: Vec<String>,
    /// The line of the kept copy the read stopped at.
    end: u64,
    /// Whether the cap cut it short of the copy's end.
    more: bool,
}

/// The lines after `line` of the kept copy of the log, when that copy is the
/// file generation `wanted` named.
fn kept_copy(
    path: &Path,
    wanted: &str,
    line: u64,
    cap: usize,
) -> std::io::Result<Option<KeptRead>> {
    let mut name = path.as_os_str().to_os_string();
    name.push(".1");
    let Ok(old) = open_nofollow(Path::new(&name)) else {
        return Ok(None);
    };
    if generation(&old)? != wanted {
        return Ok(None);
    }
    let mut scanned = scan(&old, &Want::After { skip: line, cap })?;
    let lines = std::mem::take(&mut scanned.lines);
    let end = line + lines.len() as u64;
    Ok(Some(KeptRead {
        more: end < scanned.total,
        end: end.min(scanned.total),
        lines,
    }))
}

/// Reads `server.log` from where it left off, finishing a rotated-away file
/// before the new one; two rotations inside one poll lose the file in between.
pub struct FileFollower {
    path: PathBuf,
    file: Option<std::fs::File>,
    position: u64,
    /// The bytes of a line the writer has not finished.
    partial: Vec<u8>,
    refused: bool,
}

fn identity_of(meta: &std::fs::Metadata) -> (u64, u64) {
    (meta.dev(), meta.ino())
}

impl FileFollower {
    fn unopened(path: PathBuf) -> Self {
        Self {
            path,
            file: None,
            position: 0,
            partial: Vec::new(),
            refused: false,
        }
    }

    /// Whether the log's path turned out to be a symbolic link, which is never
    /// read: nothing more comes from a follower that says so.
    pub fn refused(&self) -> bool {
        self.refused
    }

    /// Open at the end of the file's last `last` lines. A file that does not
    /// exist yet is followed from its first byte once it does.
    pub fn start(path: PathBuf, last: usize) -> (Self, Vec<String>) {
        let mut follower = Self::unopened(path);
        let file = match open_nofollow(&follower.path) {
            Ok(file) => file,
            Err(error) => {
                follower.refused = is_link_refusal(&error);
                return (follower, Vec::new());
            }
        };
        let Ok(scanned) = scan(&file, &Want::Tail(last)) else {
            return (follower, Vec::new());
        };
        follower.position = scanned.position;
        follower.file = Some(file);
        (follower, scanned.lines)
    }

    /// Read what the held file has past `position`, restarting it if it shrank.
    fn read_held(&mut self) {
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
        if self.refused {
            return Vec::new();
        }
        // The file in hand first, to its end, so nothing written before a
        // rotation is lost to it.
        self.read_held();
        let mut lines = self.take_lines();
        between();
        // Then the name: another file under it means a rotation.
        let opened = match open_nofollow(&self.path) {
            Ok(opened) => Some(opened),
            Err(error) => {
                if is_link_refusal(&error) {
                    self.refused = true;
                    self.file = None;
                }
                None
            }
        };
        let renamed = opened.filter(|opened| {
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

        // The same file by cursor: a cursor is `<generation>:<line>`, lines count
        // from one, the cursor answered is the count of complete lines, and a
        // cursor past the end gets nothing and the count.
        let read = |since: Option<&str>, tail, cap| {
            let (read, _) = open_log(&path, since, tail, cap).expect("readable");
            (read.lines, read.cursor, read.note)
        };
        let (lines, cursor, note) = read(None, 2, 100);
        assert_eq!(
            (lines, note),
            (vec!["four".to_string(), "five".to_string()], None)
        );
        let (generation, count) = cursor.split_once(':').unwrap();
        assert_eq!(count, "5");
        let at = |line: u64| format!("{generation}:{line}");
        let (lines, cursor, _) = read(Some(at(3).as_str()), 2, 100);
        assert_eq!(lines, ["four", "five"], "a cursor wins over the tail");
        assert_eq!(cursor, at(5));
        assert_eq!(read(Some(at(0).as_str()), 2, 100).0.len(), 5);
        assert_eq!(read(Some(at(5).as_str()), 2, 100).0.len(), 0);
        assert_eq!(read(Some(at(99).as_str()), 2, 100), (vec![], at(5), None));
        // A capped answer ends at the cursor it names, so the next read goes on
        // from there, and says it was cut short.
        let (lines, cursor, note) = read(Some(at(1).as_str()), 2, 2);
        assert_eq!((lines, cursor), (vec!["two".into(), "three".into()], at(3)));
        assert!(note.unwrap().contains("2 lines"));
        let (lines, _, note) = read(None, 100, 3);
        assert_eq!(lines, ["three", "four", "five"]);
        assert!(note.unwrap().contains("last 3 lines"));
        for bad in ["5", "x:y", ":3", "a:b:c"] {
            assert!(
                matches!(open_log(&path, Some(bad), 2, 100), Err(LogError::BadCursor)),
                "{bad}"
            );
        }

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
            read(None, 1, 100),
            (vec!["six".into()], at(6), None),
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
        let (before, _) = open_log(&path, None, 10, 100).unwrap();
        // A line written just before the rotation is still shown.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        std::io::Write::write_all(&mut file, b"last before rotation\n").unwrap();
        std::fs::rename(&path, dir.path().join("server.log.1")).unwrap();
        std::fs::write(&path, "fresh\n").unwrap();
        assert_eq!(follower.poll(), ["last before rotation", "fresh"]);

        // A cursor from before the rotation reads on in the kept copy, then
        // the new file, with nothing skipped and nothing said. With the copy
        // gone, the new file is read from its start and the answer says lines
        // may be missing; a cursor that was at the very start misses none.
        let (resumed, _) = open_log(&path, Some(&before.cursor), 10, 100).unwrap();
        assert_eq!(resumed.lines, ["last before rotation", "fresh"]);
        assert_eq!(resumed.note, None);
        let (generation, _) = before.cursor.split_once(':').unwrap();
        let (fresh_cursor, _) = resumed.cursor.split_once(':').unwrap();
        assert_ne!(generation, fresh_cursor, "the new file is a new generation");
        std::fs::remove_file(dir.path().join("server.log.1")).unwrap();
        let (lost, _) = open_log(&path, Some(&before.cursor), 10, 100).unwrap();
        assert_eq!(lost.lines, ["fresh"]);
        assert!(lost.note.unwrap().contains("may have been skipped"));
        let (at_start, _) = open_log(&path, Some(&format!("{generation}:0")), 10, 100).unwrap();
        assert_eq!(
            (at_start.lines, at_start.note),
            (vec!["fresh".to_string()], None)
        );

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

        // A link put where the log was is never followed, whatever it points
        // at: nothing of its target is read, at the start or after one.
        let secret = dir.path().join("secret.txt");
        std::fs::write(&secret, "secret line\n").unwrap();
        let (mut follower, _) = FileFollower::start(path.clone(), 10);
        std::fs::rename(&path, dir.path().join("server.log.3")).unwrap();
        std::os::unix::fs::symlink(&secret, &path).unwrap();
        assert!(follower.poll().is_empty());
        assert!(follower.refused());
        assert!(matches!(
            open_log(&path, None, 10, 100),
            Err(LogError::Symlink(_))
        ));
        let (linked, first) = FileFollower::start(path.clone(), 10);
        assert!(first.is_empty() && linked.refused());
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
