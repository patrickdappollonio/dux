//! The bytes dux has read from its own terminal and not yet given to a reader.
//!
//! Every reader of the terminal (the windowed interface, the maximized pane,
//! the input debugger, the start-web-server status screen) takes its input from
//! here and gives back what it did not consume, so a key that hands input from
//! one reader to another leaves what followed it for the next reader, whole and
//! in order. The windowed interface and the status screen read these bytes as
//! the events crossterm's own reader would make of them.

use std::collections::VecDeque;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd, RawFd};
use std::sync::{Mutex, OnceLock};

use crossterm::event::{Event, KeyCode, KeyEvent};

/// The size of one read, crossterm's own: a lone Escape that ends a read this
/// long is held for the next read rather than taken as the Escape key.
pub(crate) const READ_SIZE: usize = 1024;

const PASTE_START: &[u8] = b"\x1b[200~";

/// What crossterm makes of the bytes of one sequence so far.
#[derive(Debug, PartialEq)]
enum Decoded {
    /// The sequence may still go on.
    Incomplete,
    /// A whole sequence crossterm reads as this event.
    Event(Event),
    /// A whole sequence crossterm drops, or one it cannot parse.
    Nothing,
}

/// Whether more input may follow the byte being decoded without another wait.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Next {
    MayFollow,
    EndOfInput,
}

#[derive(Debug, Default)]
pub(crate) struct TerminalInput {
    unread: VecDeque<u8>,
    /// Whether the last byte held, in `unread` or else in `partial`, ended a
    /// read that filled [`READ_SIZE`]: a lone Escape there waits for the next.
    ends_full_read: bool,
    /// The start of a sequence the event decoder has taken but not finished.
    partial: Vec<u8>,
}

/// Where dux reads its keys, chosen as crossterm chooses: standard input
/// when it is a terminal, otherwise the controlling terminal.
enum KeySource {
    Stdin,
    ControllingTerminal(OwnedFd),
}

static SOURCE: OnceLock<KeySource> = OnceLock::new();

/// The terminal every reader of dux's keys reads; standard input when neither
/// it nor a controlling terminal can be had.
pub(crate) fn terminal() -> BorrowedFd<'static> {
    let stdin = rustix::stdio::stdin();
    let source = SOURCE.get_or_init(|| {
        if rustix::termios::isatty(stdin) {
            return KeySource::Stdin;
        }
        std::fs::File::options()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .map_or(KeySource::Stdin, |tty| {
                KeySource::ControllingTerminal(tty.into())
            })
    });
    match source {
        KeySource::Stdin => stdin,
        KeySource::ControllingTerminal(tty) => tty.as_fd(),
    }
}

/// Whether [`terminal`] opened `/dev/tty` because standard input is no terminal.
#[cfg(test)]
pub(crate) fn reads_dev_tty() -> bool {
    matches!(SOURCE.get(), Some(KeySource::ControllingTerminal(_)))
}

/// Whether `fd` is the `/dev/tty` that [`terminal`] opened.
fn is_opened_dev_tty(fd: BorrowedFd<'_>) -> bool {
    matches!(SOURCE.get(), Some(KeySource::ControllingTerminal(tty))
        if tty.as_raw_fd() == fd.as_raw_fd())
}

/// What a wait saw on one descriptor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Readiness {
    Waiting,
    /// A read will not block: bytes, the end of the input, or an error.
    Ready,
    /// The wait cannot watch it, so a read could block for good.
    Unwatchable,
}

/// Wait up to `timeout` for any of `fds` to have something to read. A signal
/// ends the wait with `INTR`.
pub(crate) fn wait_readable<const N: usize>(
    fds: [BorrowedFd<'_>; N],
    timeout: &rustix::time::Timespec,
) -> rustix::io::Result<[Readiness; N]> {
    if poll_cannot_watch_dev_tty() && fds.iter().any(|fd| is_opened_dev_tty(*fd)) {
        select_readable(fds, timeout)
    } else {
        poll_readable(fds, timeout)
    }
}

/// Has [`wait_readable`] wait on the opened `/dev/tty` with `select`, as on
/// macOS, when a pseudo-terminal child sets it.
#[cfg(test)]
pub(crate) static SELECT_ON_EVERY_SYSTEM: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// macOS's `poll` reports `/dev/tty` itself invalid while `select` watches
/// it, which is how crossterm's own `/dev/tty` reader waits there.
fn poll_cannot_watch_dev_tty() -> bool {
    #[cfg(test)]
    if SELECT_ON_EVERY_SYSTEM.load(std::sync::atomic::Ordering::Relaxed) {
        return true;
    }
    cfg!(target_os = "macos")
}

fn poll_readable<const N: usize>(
    fds: [BorrowedFd<'_>; N],
    timeout: &rustix::time::Timespec,
) -> rustix::io::Result<[Readiness; N]> {
    use rustix::event::{PollFd, PollFlags, poll};

    let mut polled = fds.map(|fd| PollFd::from_borrowed_fd(fd, PollFlags::IN));
    poll(&mut polled, Some(timeout))?;
    Ok(polled.map(|fd| {
        let revents = fd.revents();
        if revents.contains(PollFlags::NVAL) {
            Readiness::Unwatchable
        } else if revents.is_empty() {
            Readiness::Waiting
        } else {
            Readiness::Ready
        }
    }))
}

fn select_readable<const N: usize>(
    fds: [BorrowedFd<'_>; N],
    timeout: &rustix::time::Timespec,
) -> rustix::io::Result<[Readiness; N]> {
    use rustix::event::{FdSetElement, FdSetIter, fd_set_insert, fd_set_num_elements, select};

    let nfds = fds.iter().map(|fd| fd.as_raw_fd() + 1).max().unwrap_or(0);
    let mut readable = vec![FdSetElement::default(); fd_set_num_elements(N, nfds)];
    for fd in &fds {
        fd_set_insert(&mut readable, fd.as_raw_fd());
    }
    // Whole microseconds: rounding a nanosecond count up can reach a full
    // second of them, which `select` refuses.
    let timeout = rustix::time::Timespec {
        tv_sec: timeout.tv_sec,
        tv_nsec: timeout.tv_nsec / 1000 * 1000,
    };
    // SAFETY: every descriptor in the set is borrowed for the whole call.
    match unsafe { select(nfds, Some(&mut readable), None, None, Some(&timeout)) } {
        Ok(_) => {
            let ready: Vec<RawFd> = FdSetIter::new(&readable).collect();
            Ok(fds.map(|fd| {
                if ready.contains(&fd.as_raw_fd()) {
                    Readiness::Ready
                } else {
                    Readiness::Waiting
                }
            }))
        }
        // `select` names no descriptor when one is invalid.
        Err(rustix::io::Errno::BADF) => Ok(fds.map(|fd| {
            if rustix::io::fcntl_getfd(fd).is_err() {
                Readiness::Unwatchable
            } else {
                Readiness::Waiting
            }
        })),
        Err(err) => Err(err),
    }
}

/// What one surface of the terminal UI left unread when it handed the terminal
/// to the next: the windowed interface to the status screen on the flip, and
/// the status screen back to a new interface on the resume.
static BETWEEN_SURFACES: Mutex<TerminalInput> = Mutex::new(TerminalInput::new());

/// Leave `bytes` for the next surface to read, after anything an earlier
/// surface left that nobody has taken yet.
pub(crate) fn leave_for_next_surface(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    BETWEEN_SURFACES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .unread
        .extend(bytes);
}

/// Take what the last surface left unread.
pub(crate) fn take_from_last_surface() -> TerminalInput {
    let mut slot = BETWEEN_SURFACES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    std::mem::take(&mut *slot)
}

impl TerminalInput {
    pub(crate) const fn new() -> Self {
        Self {
            unread: VecDeque::new(),
            ends_full_read: false,
            partial: Vec::new(),
        }
    }

    /// Add one read's bytes.
    pub(crate) fn push_read(&mut self, bytes: &[u8]) {
        self.push_end_of_read(bytes, bytes.len());
    }

    /// Add `bytes`, the last bytes of a read of `read_len` bytes.
    pub(crate) fn push_end_of_read(&mut self, bytes: &[u8], read_len: usize) {
        self.unread.extend(bytes);
        self.ends_full_read = !bytes.is_empty() && read_len == READ_SIZE;
    }

    /// Drop the start of a sequence the event decoder has taken but not
    /// finished, and return it.
    pub(crate) fn end_begun_sequence(&mut self) -> Vec<u8> {
        if self.unread.is_empty() {
            self.ends_full_read = false;
        }
        std::mem::take(&mut self.partial)
    }

    /// Whether bytes are waiting that no reader has looked at yet.
    pub(crate) fn has_unread(&self) -> bool {
        !self.unread.is_empty()
    }

    /// Whether nothing is held at all, a begun sequence included.
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.unread.is_empty() && self.partial.is_empty()
    }

    /// The start of a sequence the event decoder has taken but not finished.
    #[cfg(test)]
    pub(crate) fn begun_sequence(&self) -> &[u8] {
        &self.partial
    }

    /// Hand every byte held, a sequence the event decoder has begun included,
    /// to a reader that splits bytes itself.
    pub(crate) fn take_bytes(&mut self) -> Vec<u8> {
        let mut bytes = std::mem::take(&mut self.partial);
        bytes.extend(self.unread.drain(..));
        self.ends_full_read = false;
        bytes
    }

    /// Put bytes a reader did not use back ahead of anything held, a begun
    /// sequence included; giving back nothing leaves that sequence waiting.
    pub(crate) fn give_back(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let begun = std::mem::take(&mut self.partial);
        for &byte in bytes.iter().chain(&begun).rev() {
            self.unread.push_front(byte);
        }
    }

    /// The next event crossterm would have made of the bytes held, consuming
    /// only that event's bytes. `None` when the bytes run out first.
    pub(crate) fn next_event(&mut self) -> Option<Event> {
        while let Some(byte) = self.unread.pop_front() {
            let last = self.unread.is_empty();
            let next = if !last || self.ends_full_read {
                Next::MayFollow
            } else {
                Next::EndOfInput
            };
            self.partial.push(byte);
            let decoded = crossterm_decode(&self.partial, next);
            if decoded == Decoded::Incomplete {
                continue;
            }
            self.partial.clear();
            if last {
                self.ends_full_read = false;
            }
            if let Decoded::Event(event) = decoded {
                return Some(event);
            }
        }
        None
    }
}

/// crossterm 0.29's `parse_event`, built on terminput's port of it with the
/// places the port departs from crossterm put back.
fn crossterm_decode(buffer: &[u8], next: Next) -> Decoded {
    match buffer {
        [0x1b] if next == Next::MayFollow => return Decoded::Incomplete,
        [0x1b, 0x1b] => return Decoded::Event(Event::Key(KeyEvent::from(KeyCode::Esc))),
        // crossterm has no `ESC [ R` key.
        [0x1b, b'[', b'R'] => return Decoded::Nothing,
        // crossterm waits for a kitty flags report of at least five bytes.
        [0x1b, b'[', b'?', b'u'] => return Decoded::Incomplete,
        // A numbered `CSI … R` is a cursor-position report to crossterm, which
        // its event reader never returns, a modified F3 included.
        [0x1b, b'[', b'0'..=b'9', .., b'R'] if !buffer.starts_with(PASTE_START) => {
            return Decoded::Nothing;
        }
        // An X10 mouse coordinate byte below 33 wraps in crossterm.
        [0x1b, b'[', b'M', _, x, y] if *x < 33 || *y < 33 => {
            return x10_mouse_with_wrapped_coordinates(buffer);
        }
        _ => {}
    }
    match terminput::Event::parse_from(buffer) {
        Ok(None) => Decoded::Incomplete,
        Ok(Some(event)) => to_crossterm(event).map_or(Decoded::Nothing, Decoded::Event),
        Err(_) => Decoded::Nothing,
    }
}

fn to_crossterm(event: terminput::Event) -> Option<Event> {
    match event {
        terminput::Event::Key(key) => Some(Event::Key(crate::key_encode::from_terminput(key))),
        other => terminput_crossterm::to_crossterm(other).ok(),
    }
}

fn x10_mouse_with_wrapped_coordinates(buffer: &[u8]) -> Decoded {
    let mut shifted = buffer.to_vec();
    shifted[4] = 33;
    shifted[5] = 33;
    let Ok(Some(terminput::Event::Mouse(mouse))) = terminput::Event::parse_from(&shifted) else {
        return Decoded::Nothing;
    };
    let Ok(mut mouse) = terminput_crossterm::to_crossterm_mouse(mouse) else {
        return Decoded::Nothing;
    };
    let coordinate = |byte: u8| u16::from(byte.saturating_sub(32)).wrapping_sub(1);
    mouse.column = coordinate(buffer[4]);
    mouse.row = coordinate(buffer[5]);
    Decoded::Event(Event::Mouse(mouse))
}

#[cfg(test)]
#[path = "terminal_input_tests.rs"]
mod tests;
