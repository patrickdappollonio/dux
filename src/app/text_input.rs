use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Reusable text input with cursor tracking and optional multiline support.
///
/// Handles character-level and word-level editing, cursor movement, and
/// common key dispatch. All cursor positions are byte indices into the
/// underlying UTF-8 string.
///
/// By default operates in single-line mode. Call [`TextInput::with_multiline`]
/// to enable multiline editing where Enter inserts newlines and Up/Down
/// navigate between lines.
#[derive(Clone, Debug)]
pub struct TextInput {
    pub text: String,
    pub cursor: usize,
    multiline: Option<MultilineState>,
}

#[derive(Clone, Debug)]
struct MultilineState {
    /// Maximum number of lines visible at once for rendering.
    visible_lines: usize,
    /// Index of the first visible line (0-based scroll position).
    scroll_offset: usize,
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
            multiline: None,
        }
    }

    pub fn with_text(text: String) -> Self {
        let cursor = text.len();
        Self {
            text,
            cursor,
            multiline: None,
        }
    }

    /// Enable multiline editing with a visible line limit for rendering.
    pub fn with_multiline(mut self, visible_lines: usize) -> Self {
        self.multiline = Some(MultilineState {
            visible_lines,
            scroll_offset: 0,
        });
        self
    }

    pub fn is_multiline(&self) -> bool {
        self.multiline.is_some()
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

    // ── Multiline operations ───────────────────────────────────────

    /// Move cursor to the same column on the previous line.
    /// If the previous line is shorter, the cursor lands at its end.
    /// No-op when already on the first line or in single-line mode.
    pub fn move_up(&mut self) {
        if self.multiline.is_none() {
            return;
        }
        let (line, col) = self.cursor_line_col();
        if line == 0 {
            return;
        }
        self.cursor = byte_offset_at(&self.text, line - 1, col);
        self.ensure_cursor_visible();
    }

    /// Move cursor to the same column on the next line.
    /// If the next line is shorter, the cursor lands at its end.
    /// No-op when already on the last line or in single-line mode.
    pub fn move_down(&mut self) {
        if self.multiline.is_none() {
            return;
        }
        let (line, col) = self.cursor_line_col();
        let total = self.total_lines();
        if line + 1 >= total {
            return;
        }
        self.cursor = byte_offset_at(&self.text, line + 1, col);
        self.ensure_cursor_visible();
    }

    /// Move cursor to the start of the current line (multiline) or
    /// to the start of the entire text (single-line).
    pub fn move_line_home(&mut self) {
        if self.multiline.is_none() {
            self.cursor = 0;
            return;
        }
        let idx = clamp_cursor(&self.text, self.cursor);
        self.cursor = self.text[..idx].rfind('\n').map(|p| p + 1).unwrap_or(0);
    }

    /// Move cursor to the end of the current line (multiline) or
    /// to the end of the entire text (single-line).
    pub fn move_line_end(&mut self) {
        if self.multiline.is_none() {
            self.cursor = self.text.len();
            return;
        }
        let idx = clamp_cursor(&self.text, self.cursor);
        self.cursor = self.text[idx..]
            .find('\n')
            .map(|p| idx + p)
            .unwrap_or(self.text.len());
    }

    /// Total number of lines in the text (count of `\n` + 1).
    pub fn total_lines(&self) -> usize {
        self.text.chars().filter(|&c| c == '\n').count() + 1
    }

    /// Current scroll offset (first visible line index). Returns 0 in single-line mode.
    pub fn scroll_offset(&self) -> usize {
        self.multiline
            .as_ref()
            .map(|m| m.scroll_offset)
            .unwrap_or(0)
    }

    /// Maximum number of visible lines, if multiline is enabled.
    pub fn visible_line_count(&self) -> Option<usize> {
        self.multiline.as_ref().map(|m| m.visible_lines)
    }

    /// Returns the lines currently visible for rendering (accounting for scroll offset).
    pub fn visible_lines(&self) -> Vec<&str> {
        let lines: Vec<&str> = self.text.split('\n').collect();
        match &self.multiline {
            Some(m) => {
                let start = m.scroll_offset.min(lines.len());
                let end = (start + m.visible_lines).min(lines.len());
                lines[start..end].to_vec()
            }
            None => lines,
        }
    }

    /// Returns `(row, col)` of the cursor relative to the scroll offset, for rendering.
    /// In single-line mode, returns `(0, cursor_position)`.
    pub fn cursor_display_position(&self) -> (usize, usize) {
        let (line, col) = self.cursor_line_col();
        let offset = self.scroll_offset();
        (line.saturating_sub(offset), col)
    }

    /// Compute the (line, column) of the cursor in character units.
    fn cursor_line_col(&self) -> (usize, usize) {
        let idx = clamp_cursor(&self.text, self.cursor);
        let before = &self.text[..idx];
        let line = before.chars().filter(|&c| c == '\n').count();
        let line_start = before.rfind('\n').map(|p| p + 1).unwrap_or(0);
        let col = before[line_start..].chars().count();
        (line, col)
    }

    /// Adjust scroll offset so the cursor line is within the visible window.
    fn ensure_cursor_visible(&mut self) {
        let Some(m) = &mut self.multiline else {
            return;
        };
        let (cursor_line, _) = {
            let idx = clamp_cursor(&self.text, self.cursor);
            let before = &self.text[..idx];
            let line = before.chars().filter(|&c| c == '\n').count();
            (line, 0)
        };
        if cursor_line < m.scroll_offset {
            m.scroll_offset = cursor_line;
        } else if cursor_line >= m.scroll_offset + m.visible_lines {
            m.scroll_offset = cursor_line + 1 - m.visible_lines;
        }
    }

    // ── Key dispatch ────────────────────────────────────────────────

    /// Handle common text-editing keys. Returns `true` if the key was consumed.
    ///
    /// Handled keys:
    /// - `Char(c)` (without Ctrl) → insert
    /// - `Backspace` → delete char backward; `Alt+Backspace` / `Ctrl+W` → delete word backward
    /// - `Delete` → delete char forward; `Alt+Delete` / `Ctrl+Delete` → delete word forward
    /// - `Left` / `Right` → move char; `Alt+Left/Right` / `Ctrl+Left/Right` → move word
    /// - `Home` / `End` → jump to start/end of line (multiline) or text (single-line)
    ///
    /// In multiline mode, additionally:
    /// - `Enter` → insert newline
    /// - `Up` / `Down` → move between lines
    ///
    /// Everything else (Esc, Tab, and Enter/Up/Down in single-line mode)
    /// returns `false` for the caller to handle.
    pub fn handle_key(&mut self, key: KeyEvent) -> bool {
        let has_alt = key.modifiers.contains(KeyModifiers::ALT);
        let has_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let is_multiline = self.multiline.is_some();

        match key.code {
            // Multiline-only: Enter inserts newline
            KeyCode::Enter if is_multiline => {
                self.insert_char('\n');
                self.ensure_cursor_visible();
                true
            }
            // Multiline-only: Up/Down navigate lines
            KeyCode::Up if is_multiline && !has_alt && !has_ctrl => {
                self.move_up();
                true
            }
            KeyCode::Down if is_multiline && !has_alt && !has_ctrl => {
                self.move_down();
                true
            }
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
            // Home/End: line-level in multiline, text-level in single-line
            KeyCode::Home if is_multiline => {
                self.move_line_home();
                true
            }
            KeyCode::End if is_multiline => {
                self.move_line_end();
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
                if is_multiline {
                    self.ensure_cursor_visible();
                }
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

/// Compute the byte offset of a given (line, column) position in character units.
/// If the target line is shorter than `col`, the offset lands at the line's end.
fn byte_offset_at(text: &str, target_line: usize, target_col: usize) -> usize {
    let mut line = 0;
    let mut byte = 0;
    for (i, ch) in text.char_indices() {
        if line == target_line {
            // We're on the target line — count columns.
            let line_start = byte;
            for (col, (j, c)) in text[line_start..].char_indices().enumerate() {
                if c == '\n' || col == target_col {
                    return line_start + j;
                }
            }
            // Reached end of text on this line.
            return text.len();
        }
        if ch == '\n' {
            line += 1;
        }
        byte = i + ch.len_utf8();
    }
    // target_line beyond total lines — return end of text.
    text.len()
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
        let mut ti = TextInput::with_text("hello world".into());
        ti.cursor = 0;
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
        let mut ti = TextInput::with_text("hello".into());
        ti.cursor = 0;
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
        let mut ti = TextInput::with_text("hello world".into());
        ti.cursor = 0;
        assert!(ti.handle_key(key_alt(KeyCode::Delete)));
        assert_eq!(ti.text, "world");
    }

    #[test]
    fn handle_key_ctrl_delete() {
        let mut ti = TextInput::with_text("hello world".into());
        ti.cursor = 0;
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

    // ── Multiline tests ───────────────────────────────────────────

    fn multiline_input(text: &str, visible: usize) -> TextInput {
        TextInput::with_text(text.to_string()).with_multiline(visible)
    }

    #[test]
    fn multiline_enter_inserts_newline() {
        let mut ti = TextInput::new().with_multiline(5);
        assert!(ti.handle_key(key(KeyCode::Char('a'))));
        assert!(ti.handle_key(key(KeyCode::Enter)));
        assert!(ti.handle_key(key(KeyCode::Char('b'))));
        assert_eq!(ti.text, "a\nb");
        assert_eq!(ti.total_lines(), 2);
    }

    #[test]
    fn singleline_enter_not_consumed() {
        let mut ti = TextInput::new();
        assert!(!ti.handle_key(key(KeyCode::Enter)));
        assert!(ti.text.is_empty());
    }

    #[test]
    fn multiline_up_down_navigation() {
        // "hello\nworld\nfoo"
        let mut ti = multiline_input("hello\nworld\nfoo", 10);
        // Cursor at end (byte 15), line 2, col 3
        assert_eq!(ti.cursor_line_col(), (2, 3));

        ti.move_up();
        // Line 1, col 3 → byte 6 + 3 = 9 ("worl|d")
        assert_eq!(ti.cursor_line_col(), (1, 3));
        assert_eq!(ti.cursor, 9);

        ti.move_up();
        // Line 0, col 3 → byte 3 ("hel|lo")
        assert_eq!(ti.cursor_line_col(), (0, 3));
        assert_eq!(ti.cursor, 3);

        // Already at line 0, move_up is a no-op
        ti.move_up();
        assert_eq!(ti.cursor_line_col(), (0, 3));

        ti.move_down();
        assert_eq!(ti.cursor_line_col(), (1, 3));

        ti.move_down();
        assert_eq!(ti.cursor_line_col(), (2, 3));

        // Already at last line, move_down is a no-op
        ti.move_down();
        assert_eq!(ti.cursor_line_col(), (2, 3));
    }

    #[test]
    fn multiline_up_clamps_to_shorter_line() {
        // "hi\nworld" — line 0 has 2 chars, line 1 has 5
        let mut ti = multiline_input("hi\nworld", 10);
        // Cursor at end of "world", col 5
        assert_eq!(ti.cursor_line_col(), (1, 5));

        ti.move_up();
        // Line 0 only has 2 chars, so clamp to col 2 → end of "hi"
        assert_eq!(ti.cursor_line_col(), (0, 2));
        assert_eq!(ti.cursor, 2);
    }

    #[test]
    fn multiline_down_clamps_to_shorter_line() {
        // "world\nhi" — line 0 has 5 chars, line 1 has 2
        let mut ti = multiline_input("world\nhi", 10);
        ti.cursor = 5; // end of "world", col 5

        ti.move_down();
        // Line 1 only has 2 chars, so clamp to col 2 → end of "hi"
        assert_eq!(ti.cursor_line_col(), (1, 2));
        assert_eq!(ti.cursor, 8); // "world\nhi".len()
    }

    #[test]
    fn singleline_up_down_not_consumed() {
        let mut ti = TextInput::with_text("hello".into());
        assert!(!ti.handle_key(key(KeyCode::Up)));
        assert!(!ti.handle_key(key(KeyCode::Down)));
    }

    #[test]
    fn multiline_up_down_consumed() {
        let mut ti = multiline_input("a\nb", 5);
        assert!(ti.handle_key(key(KeyCode::Up)));
        assert!(ti.handle_key(key(KeyCode::Down)));
    }

    #[test]
    fn multiline_home_end_line_level() {
        let mut ti = multiline_input("hello\nworld", 10);
        ti.cursor = 9; // "world" col 3 → "wor|ld"

        ti.handle_key(key(KeyCode::Home));
        assert_eq!(ti.cursor, 6); // start of "world"

        ti.handle_key(key(KeyCode::End));
        assert_eq!(ti.cursor, 11); // end of "world"
    }

    #[test]
    fn singleline_home_end_text_level() {
        let mut ti = TextInput::with_text("hello".into());
        ti.handle_key(key(KeyCode::Home));
        assert_eq!(ti.cursor, 0);
        ti.handle_key(key(KeyCode::End));
        assert_eq!(ti.cursor, 5);
    }

    #[test]
    fn scroll_offset_adjusts_on_move_down() {
        // 5 lines, visible_lines = 3
        let mut ti = multiline_input("a\nb\nc\nd\ne", 3);
        ti.cursor = 0; // line 0
        assert_eq!(ti.scroll_offset(), 0);

        ti.move_down(); // line 1
        ti.move_down(); // line 2
        assert_eq!(ti.scroll_offset(), 0); // still visible

        ti.move_down(); // line 3 — scrolls
        assert_eq!(ti.scroll_offset(), 1);
        assert_eq!(ti.cursor_line_col(), (3, 0));

        ti.move_down(); // line 4 — scrolls more
        assert_eq!(ti.scroll_offset(), 2);
    }

    #[test]
    fn scroll_offset_adjusts_on_move_up() {
        let mut ti = multiline_input("a\nb\nc\nd\ne", 3);
        // Start at line 4
        assert_eq!(ti.cursor_line_col(), (4, 1));
        ti.ensure_cursor_visible();
        assert_eq!(ti.scroll_offset(), 2); // lines 2,3,4 visible

        ti.move_up(); // line 3
        assert_eq!(ti.scroll_offset(), 2); // still in window

        ti.move_up(); // line 2
        assert_eq!(ti.scroll_offset(), 2); // still in window

        ti.move_up(); // line 1 — scrolls up
        assert_eq!(ti.scroll_offset(), 1);

        ti.move_up(); // line 0 — scrolls up
        assert_eq!(ti.scroll_offset(), 0);
    }

    #[test]
    fn visible_lines_returns_correct_slice() {
        let mut ti = multiline_input("a\nb\nc\nd\ne", 3);
        ti.cursor = 0;
        assert_eq!(ti.visible_lines(), vec!["a", "b", "c"]);

        // Scroll to show lines 1,2,3
        ti.move_down();
        ti.move_down();
        ti.move_down(); // line 3, scroll_offset = 1
        assert_eq!(ti.visible_lines(), vec!["b", "c", "d"]);
    }

    #[test]
    fn cursor_display_position_accounts_for_scroll() {
        let mut ti = multiline_input("a\nb\nc\nd\ne", 3);
        ti.cursor = 0; // line 0, col 0
        assert_eq!(ti.cursor_display_position(), (0, 0));

        // Move to line 3, col 0 — scroll_offset becomes 1
        ti.move_down();
        ti.move_down();
        ti.move_down();
        assert_eq!(ti.cursor_line_col(), (3, 0));
        assert_eq!(ti.scroll_offset(), 1);
        assert_eq!(ti.cursor_display_position(), (2, 0)); // line 3 - offset 1 = display row 2
    }

    #[test]
    fn total_lines_count() {
        assert_eq!(TextInput::new().total_lines(), 1);
        assert_eq!(TextInput::with_text("a".into()).total_lines(), 1);
        assert_eq!(TextInput::with_text("a\nb".into()).total_lines(), 2);
        assert_eq!(TextInput::with_text("a\nb\nc".into()).total_lines(), 3);
        assert_eq!(TextInput::with_text("\n".into()).total_lines(), 2);
    }

    #[test]
    fn byte_offset_at_various() {
        let text = "hello\nworld\nfoo";
        assert_eq!(byte_offset_at(text, 0, 0), 0);
        assert_eq!(byte_offset_at(text, 0, 3), 3);
        assert_eq!(byte_offset_at(text, 0, 5), 5); // end of "hello" (before \n)
        assert_eq!(byte_offset_at(text, 1, 0), 6);
        assert_eq!(byte_offset_at(text, 1, 3), 9);
        assert_eq!(byte_offset_at(text, 2, 0), 12);
        assert_eq!(byte_offset_at(text, 2, 3), 15); // end of text
    }

    #[test]
    fn byte_offset_at_clamps_col() {
        // Line "hi" has only 2 chars; requesting col 5 gives end of line
        let text = "hi\nworld";
        assert_eq!(byte_offset_at(text, 0, 5), 2); // clamped to end of "hi"
    }
}
