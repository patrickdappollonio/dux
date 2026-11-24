//! The bytes dux has read from its own terminal and not yet given to a reader.
//!
//! Every reader of the terminal (the windowed interface, the maximized pane,
//! the input debugger, the start-web-server status screen) takes its input from
//! here and gives back what it did not consume, so a key that hands input from
//! one reader to another leaves what followed it for the next reader, whole and
//! in order. The windowed interface and the status screen read these bytes as
//! the events crossterm's own reader would make of them.

use std::collections::VecDeque;
use std::sync::Mutex;

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
    /// Whether the last read filled [`READ_SIZE`] and its last byte is still
    /// unread, which tells a lone Escape there to wait for the next read.
    unread_ends_full_read: bool,
    /// The start of a sequence the event decoder has taken but not finished.
    partial: Vec<u8>,
}

/// What one surface of the terminal UI left unread when it handed the terminal
/// to the next: the windowed interface to the status screen on the flip, and
/// the status screen back to a new interface on the resume.
static BETWEEN_SURFACES: Mutex<TerminalInput> = Mutex::new(TerminalInput::new());

/// Leave what `input` holds for the next surface to read, after anything an
/// earlier surface left that nobody has taken yet.
pub(crate) fn leave_for_next_surface(mut input: TerminalInput) {
    if input.is_empty() {
        return;
    }
    let mut slot = BETWEEN_SURFACES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let bytes = input.take_bytes();
    slot.unread.extend(bytes);
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
            unread_ends_full_read: false,
            partial: Vec::new(),
        }
    }

    /// Add one read's bytes.
    pub(crate) fn push_read(&mut self, bytes: &[u8]) {
        self.unread.extend(bytes);
        self.unread_ends_full_read = bytes.len() == READ_SIZE;
    }

    /// Whether bytes are waiting that no reader has looked at yet.
    pub(crate) fn has_unread(&self) -> bool {
        !self.unread.is_empty()
    }

    /// Whether nothing is held at all, a begun sequence included.
    pub(crate) fn is_empty(&self) -> bool {
        self.unread.is_empty() && self.partial.is_empty()
    }

    /// Hand every byte held, a sequence the event decoder has begun included,
    /// to a reader that splits bytes itself.
    pub(crate) fn take_bytes(&mut self) -> Vec<u8> {
        let mut bytes = std::mem::take(&mut self.partial);
        bytes.extend(self.unread.drain(..));
        self.unread_ends_full_read = false;
        bytes
    }

    /// Put bytes a reader did not use back ahead of anything held, a sequence
    /// the event decoder has begun included.
    pub(crate) fn give_back(&mut self, bytes: &[u8]) {
        let begun = std::mem::take(&mut self.partial);
        for &byte in bytes.iter().chain(&begun).rev() {
            self.unread.push_front(byte);
        }
    }

    /// The next event crossterm would have made of the bytes held, consuming
    /// only that event's bytes. `None` when the bytes run out first.
    pub(crate) fn next_event(&mut self) -> Option<Event> {
        while let Some(byte) = self.unread.pop_front() {
            let next = if !self.unread.is_empty() || self.unread_ends_full_read {
                Next::MayFollow
            } else {
                Next::EndOfInput
            };
            if self.unread.is_empty() {
                self.unread_ends_full_read = false;
            }
            self.partial.push(byte);
            match crossterm_decode(&self.partial, next) {
                Decoded::Incomplete => {}
                Decoded::Event(event) => {
                    self.partial.clear();
                    return Some(event);
                }
                Decoded::Nothing => self.partial.clear(),
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
