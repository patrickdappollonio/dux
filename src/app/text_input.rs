use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Reusable single-field text input with cursor tracking.
///
/// Handles character-level and word-level editing, cursor movement, and
/// common key dispatch. All cursor positions are byte indices into the
/// underlying UTF-8 string.
#[derive(Clone, Debug)]
pub struct TextInput {
    pub text: String,
    pub cursor: usize,
}

impl Default for TextInput {
    fn default() -> Self {
        Self::new()
    }
}

impl TextInput {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
        }
    }

    pub fn with_text(text: String) -> Self {
        let cursor = text.len();
        Self { text, cursor }
    }

    // ── Character-level operations ──────────────────────────────────

    pub fn move_left(&mut self) {
        self.cursor = prev_char_boundary(&self.text, self.cursor);
    }

    pub fn move_right(&mut self) {
        self.cursor = next_char_boundary(&self.text, self.cursor);
    }

    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.text.len();
    }

    pub fn insert_char(&mut self, ch: char) {
        let index = clamp_cursor(&self.text, self.cursor);
        self.text.insert(index, ch);
        self.cursor = index + ch.len_utf8();
    }

    pub fn backspace(&mut self) {
        let index = clamp_cursor(&self.text, self.cursor);
        if index == 0 {
            return;
        }
        let prev = prev_char_boundary(&self.text, index);
        self.text.replace_range(prev..index, "");
        self.cursor = prev;
    }

    pub fn delete(&mut self) {
        let index = clamp_cursor(&self.text, self.cursor);
        if index >= self.text.len() {
            return;
        }
        let next = next_char_boundary(&self.text, index);
        self.text.replace_range(index..next, "");
        self.cursor = index;
    }

    // ── Word-level operations ───────────────────────────────────────

    pub fn move_left_word(&mut self) {
        self.cursor = prev_word_boundary(&self.text, self.cursor);
    }

    pub fn move_right_word(&mut self) {
        self.cursor = next_word_boundary(&self.text, self.cursor);
    }

    pub fn backspace_word(&mut self) {
        let index = clamp_cursor(&self.text, self.cursor);
        let target = prev_word_boundary(&self.text, index);
        if target < index {
            self.text.replace_range(target..index, "");
            self.cursor = target;
        }
    }

    pub fn delete_word(&mut self) {
        let index = clamp_cursor(&self.text, self.cursor);
        let target = next_word_boundary(&self.text, index);
        if target > index {
            self.text.replace_range(index..target, "");
            self.cursor = index;
        }
    }

    // ── Bulk operations ─────────────────────────────────────────────

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    pub fn set_text(&mut self, text: String) {
        self.cursor = text.len();
        self.text = text;
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    // ── Key dispatch ────────────────────────────────────────────────

    /// Handle common text-editing keys. Returns `true` if the key was consumed.
    ///
    /// Handled keys:
    /// - `Char(c)` (without Ctrl) → insert
    /// - `Backspace` → delete char backward; `Alt+Backspace` / `Ctrl+W` → delete word backward
    /// - `Delete` → delete char forward; `Alt+Delete` / `Ctrl+Delete` → delete word forward
    /// - `Left` / `Right` → move char; `Alt+Left/Right` / `Ctrl+Left/Right` → move word
    /// - `Home` / `End` → jump to start/end
    ///
    /// Everything else (Enter, Esc, Tab, …) returns `false` for the caller to handle.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        let has_alt = key.modifiers.contains(KeyModifiers::ALT);
        let has_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        match key.code {
            KeyCode::Backspace if has_alt => {
                self.backspace_word();
                true
            }
            KeyCode::Backspace => {
                self.backspace();
                true
            }
            KeyCode::Delete if has_alt || has_ctrl => {
                self.delete_word();
                true
            }
            KeyCode::Delete => {
                self.delete();
                true
            }
            KeyCode::Left if has_alt || has_ctrl => {
                self.move_left_word();
                true
            }
            KeyCode::Left => {
                self.move_left();
                true
            }
            KeyCode::Right if has_alt || has_ctrl => {
                self.move_right_word();
                true
            }
            KeyCode::Right => {
                self.move_right();
                true
            }
            KeyCode::Home => {
                self.move_home();
                true
            }
            KeyCode::End => {
                self.move_end();
                true
            }
            KeyCode::Char('w') if has_ctrl => {
                self.backspace_word();
                true
            }
            KeyCode::Char(c) if !has_ctrl => {
                self.insert_char(c);
                true
            }
            _ => false,
        }
    }
}

// ── Private helpers ─────────────────────────────────────────────────

fn prev_char_boundary(text: &str, index: usize) -> usize {
    let index = index.min(text.len());
    text[..index]
        .char_indices()
        .last()
        .map(|(i, _)| i)
        .unwrap_or(0)
}

fn next_char_boundary(text: &str, index: usize) -> usize {
    let index = index.min(text.len());
    text[index..]
        .char_indices()
        .nth(1)
        .map(|(offset, _)| index + offset)
        .unwrap_or(text.len())
}

pub(crate) fn clamp_cursor(text: &str, cursor: usize) -> usize {
    if cursor >= text.len() {
        return text.len();
    }
    if text.is_char_boundary(cursor) {
        cursor
    } else {
        prev_char_boundary(text, cursor)
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Scan backward from `index` to find the previous word boundary.
/// Skips non-word chars, then skips word chars — standard terminal behaviour.
fn prev_word_boundary(text: &str, index: usize) -> usize {
    let index = index.min(text.len());
    let before = &text[..index];
    let mut chars = before.char_indices().rev().peekable();

    // Skip non-word characters (whitespace, punctuation).
    while let Some(&(_, c)) = chars.peek() {
        if is_word_char(c) {
            break;
        }
        chars.next();
    }
    // Skip word characters.
    let mut boundary = 0;
    while let Some(&(i, c)) = chars.peek() {
        if !is_word_char(c) {
            boundary = i + c.len_utf8();
            break;
        }
        boundary = i;
        chars.next();
    }
    boundary
}

/// Scan forward from `index` to find the next word boundary.
/// Skips non-word chars, then skips word chars — standard terminal behaviour.
fn next_word_boundary(text: &str, index: usize) -> usize {
    let index = index.min(text.len());
    let after = &text[index..];
    let mut chars = after.char_indices().peekable();

    // Skip word characters first (we're inside or at the start of a word).
    while let Some(&(_, c)) = chars.peek() {
        if !is_word_char(c) {
            break;
        }
        chars.next();
    }
    // Skip non-word characters.
    while let Some(&(_, c)) = chars.peek() {
        if is_word_char(c) {
            break;
        }
        chars.next();
    }
    match chars.peek() {
        Some(&(offset, _)) => index + offset,
        None => text.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn key_alt(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::ALT,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn key_ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    // ── Word boundary helpers ──────────────────────────────────────

    #[test]
    fn prev_word_boundary_simple() {
        let text = "hello world";
        assert_eq!(prev_word_boundary(text, 11), 6); // end → start of "world"
        assert_eq!(prev_word_boundary(text, 6), 0); // start of "world" → start of "hello"
        assert_eq!(prev_word_boundary(text, 5), 0); // space → start of "hello"
        assert_eq!(prev_word_boundary(text, 3), 0); // mid-word → start
        assert_eq!(prev_word_boundary(text, 0), 0); // at start → stay
    }

    #[test]
    fn next_word_boundary_simple() {
        let text = "hello world";
        assert_eq!(next_word_boundary(text, 0), 6); // start → after "hello "
        assert_eq!(next_word_boundary(text, 6), 11); // start of "world" → end
        assert_eq!(next_word_boundary(text, 11), 11); // end → stay
        assert_eq!(next_word_boundary(text, 3), 6); // mid-word → after space
    }

    #[test]
    fn word_boundary_multiple_spaces() {
        let text = "foo   bar";
        assert_eq!(prev_word_boundary(text, 9), 6); // end → start of "bar"
        assert_eq!(prev_word_boundary(text, 6), 0); // start of "bar" → start of "foo"
        assert_eq!(next_word_boundary(text, 0), 6); // start → start of "bar" (skip word + spaces)
    }

    #[test]
    fn word_boundary_punctuation() {
        let text = "hello, world!";
        assert_eq!(prev_word_boundary(text, 13), 7); // end → start of "world"
        assert_eq!(prev_word_boundary(text, 7), 0); // skip ", " → start of "hello"
        assert_eq!(next_word_boundary(text, 0), 7); // skip "hello, " → start of "world"
    }

    #[test]
    fn word_boundary_underscore() {
        let text = "foo_bar baz";
        // Underscore is a word char, so foo_bar is one word.
        assert_eq!(prev_word_boundary(text, 11), 8);
        assert_eq!(prev_word_boundary(text, 8), 0);
        assert_eq!(next_word_boundary(text, 0), 8);
    }

    #[test]
    fn word_boundary_empty() {
        assert_eq!(prev_word_boundary("", 0), 0);
        assert_eq!(next_word_boundary("", 0), 0);
    }

    #[test]
    fn word_boundary_unicode() {
        let text = "café latte";
        // "café" is 5 bytes (é = 2 bytes), "latte" starts at byte 6
        assert_eq!(next_word_boundary(text, 0), 6);
        assert_eq!(prev_word_boundary(text, 11), 6);
        assert_eq!(prev_word_boundary(text, 6), 0);
    }

    // ── TextInput operations ───────────────────────────────────────

    #[test]
    fn backspace_word_deletes_word() {
        let mut ti = TextInput::with_text("hello world".into());
        ti.backspace_word();
        assert_eq!(ti.text, "hello ");
        assert_eq!(ti.cursor, 6);

        ti.backspace_word();
        assert_eq!(ti.text, "");
        assert_eq!(ti.cursor, 0);
    }

    #[test]
    fn delete_word_deletes_forward() {
        let mut ti = TextInput {
            text: "hello world".into(),
            cursor: 0,
        };
        ti.delete_word();
        assert_eq!(ti.text, "world");
        assert_eq!(ti.cursor, 0);
    }

    #[test]
    fn move_left_word_and_right_word() {
        let mut ti = TextInput::with_text("hello world foo".into());
        ti.move_left_word();
        assert_eq!(ti.cursor, 12); // start of "foo"
        ti.move_left_word();
        assert_eq!(ti.cursor, 6); // start of "world"
        ti.move_right_word();
        assert_eq!(ti.cursor, 12); // after "world "
    }

    #[test]
    fn backspace_word_at_start_is_noop() {
        let mut ti = TextInput {
            text: "hello".into(),
            cursor: 0,
        };
        ti.backspace_word();
        assert_eq!(ti.text, "hello");
        assert_eq!(ti.cursor, 0);
    }

    #[test]
    fn delete_word_at_end_is_noop() {
        let mut ti = TextInput::with_text("hello".into());
        ti.delete_word();
        assert_eq!(ti.text, "hello");
        assert_eq!(ti.cursor, 5);
    }

    // ── handle_key dispatch ────────────────────────────────────────

    #[test]
    fn handle_key_plain_char() {
        let mut ti = TextInput::new();
        assert!(ti.handle_key(key(KeyCode::Char('a'))));
        assert_eq!(ti.text, "a");
        assert_eq!(ti.cursor, 1);
    }

    #[test]
    fn handle_key_backspace() {
        let mut ti = TextInput::with_text("ab".into());
        assert!(ti.handle_key(key(KeyCode::Backspace)));
        assert_eq!(ti.text, "a");
    }

    #[test]
    fn handle_key_alt_backspace() {
        let mut ti = TextInput::with_text("hello world".into());
        assert!(ti.handle_key(key_alt(KeyCode::Backspace)));
        assert_eq!(ti.text, "hello ");
    }

    #[test]
    fn handle_key_ctrl_w() {
        let mut ti = TextInput::with_text("hello world".into());
        assert!(ti.handle_key(key_ctrl(KeyCode::Char('w'))));
        assert_eq!(ti.text, "hello ");
    }

    #[test]
    fn handle_key_alt_left_right() {
        let mut ti = TextInput::with_text("hello world".into());
        assert!(ti.handle_key(key_alt(KeyCode::Left)));
        assert_eq!(ti.cursor, 6);
        assert!(ti.handle_key(key_alt(KeyCode::Right)));
        assert_eq!(ti.cursor, 11);
    }

    #[test]
    fn handle_key_ctrl_left_right() {
        let mut ti = TextInput::with_text("hello world".into());
        assert!(ti.handle_key(key_ctrl(KeyCode::Left)));
        assert_eq!(ti.cursor, 6);
        assert!(ti.handle_key(key_ctrl(KeyCode::Right)));
        assert_eq!(ti.cursor, 11);
    }

    #[test]
    fn handle_key_alt_delete() {
        let mut ti = TextInput {
            text: "hello world".into(),
            cursor: 0,
        };
        assert!(ti.handle_key(key_alt(KeyCode::Delete)));
        assert_eq!(ti.text, "world");
    }

    #[test]
    fn handle_key_ctrl_delete() {
        let mut ti = TextInput {
            text: "hello world".into(),
            cursor: 0,
        };
        assert!(ti.handle_key(key_ctrl(KeyCode::Delete)));
        assert_eq!(ti.text, "world");
    }

    #[test]
    fn handle_key_returns_false_for_enter() {
        let mut ti = TextInput::new();
        assert!(!ti.handle_key(key(KeyCode::Enter)));
    }

    #[test]
    fn handle_key_returns_false_for_esc() {
        let mut ti = TextInput::new();
        assert!(!ti.handle_key(key(KeyCode::Esc)));
    }

    #[test]
    fn handle_key_returns_false_for_tab() {
        let mut ti = TextInput::new();
        assert!(!ti.handle_key(key(KeyCode::Tab)));
    }

    #[test]
    fn handle_key_ctrl_char_not_inserted() {
        let mut ti = TextInput::new();
        // Ctrl+A should not insert 'a'
        assert!(!ti.handle_key(key_ctrl(KeyCode::Char('a'))));
        assert!(ti.text.is_empty());
    }

    #[test]
    fn handle_key_home_end() {
        let mut ti = TextInput::with_text("hello".into());
        assert!(ti.handle_key(key(KeyCode::Home)));
        assert_eq!(ti.cursor, 0);
        assert!(ti.handle_key(key(KeyCode::End)));
        assert_eq!(ti.cursor, 5);
    }

    #[test]
    fn insert_and_navigate() {
        let mut ti = TextInput::new();
        for ch in "hello".chars() {
            ti.insert_char(ch);
        }
        assert_eq!(ti.text, "hello");
        assert_eq!(ti.cursor, 5);
        ti.move_left();
        ti.move_left();
        assert_eq!(ti.cursor, 3);
        ti.insert_char('X');
        assert_eq!(ti.text, "helXlo");
        assert_eq!(ti.cursor, 4);
    }
}
