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
    /// Maximum number of visual lines visible at once for rendering.
    visible_lines: usize,
    /// Index of the first visible visual line (0-based scroll position).
    scroll_offset: usize,
    /// Display width in characters for soft-wrapping. `None` means no wrapping.
    display_width: Option<usize>,
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
            display_width: None,
        });
        self
    }

    /// Set the display width for soft-wrapping in multiline mode.
    /// Lines longer than this width are wrapped visually. Pass `None` to disable.
    pub fn set_display_width(&mut self, width: Option<usize>) {
        if let Some(m) = &mut self.multiline {
            m.display_width = width;
        }
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

    /// Move cursor to the same column on the previous visual line.
    /// If the previous line is shorter, the cursor lands at its end.
    /// No-op when already on the first line or in single-line mode.
    pub fn move_up(&mut self) {
        if self.multiline.is_none() {
            return;
        }
        let width = self.wrap_width();
        let (vrow, vcol) = cursor_visual_pos(&self.text, self.cursor, width);
        if vrow == 0 {
            return;
        }
        self.cursor = byte_offset_at_visual(&self.text, vrow - 1, vcol, width);
        self.ensure_cursor_visible();
    }

    /// Move cursor to the same column on the next visual line.
    /// If the next line is shorter, the cursor lands at its end.
    /// No-op when already on the last line or in single-line mode.
    pub fn move_down(&mut self) {
        if self.multiline.is_none() {
            return;
        }
        let width = self.wrap_width();
        let (vrow, vcol) = cursor_visual_pos(&self.text, self.cursor, width);
        let total = visual_line_count(&self.text, width);
        if vrow + 1 >= total {
            return;
        }
        self.cursor = byte_offset_at_visual(&self.text, vrow + 1, vcol, width);
        self.ensure_cursor_visible();
    }

    /// Move cursor to the start of the current visual line (multiline) or
    /// to the start of the entire text (single-line).
    pub fn move_line_home(&mut self) {
        if self.multiline.is_none() {
            self.cursor = 0;
            return;
        }
        let width = self.wrap_width();
        let (vrow, _) = cursor_visual_pos(&self.text, self.cursor, width);
        self.cursor = byte_offset_at_visual(&self.text, vrow, 0, width);
    }

    /// Move cursor to the end of the current visual line (multiline) or
    /// to the end of the entire text (single-line).
    pub fn move_line_end(&mut self) {
        if self.multiline.is_none() {
            self.cursor = self.text.len();
            return;
        }
        let width = self.wrap_width();
        let (vrow, _) = cursor_visual_pos(&self.text, self.cursor, width);
        // Move to a very large column — byte_offset_at_visual clamps to line end.
        self.cursor = byte_offset_at_visual(&self.text, vrow, usize::MAX, width);
    }

    /// Total number of visual lines (accounting for soft-wrap).
    #[allow(dead_code)] // public API for callers; exercised by tests
    pub fn total_lines(&self) -> usize {
        visual_line_count(&self.text, self.wrap_width())
    }

    /// Current scroll offset (first visible visual line index). Returns 0 in single-line mode.
    pub fn scroll_offset(&self) -> usize {
        self.multiline
            .as_ref()
            .map(|m| m.scroll_offset)
            .unwrap_or(0)
    }

    /// Returns the visual lines currently visible for rendering
    /// (accounting for soft-wrap and scroll offset).
    pub fn visible_lines(&self) -> Vec<String> {
        let width = self.wrap_width();
        let all = compute_visual_lines(&self.text, width);
        match &self.multiline {
            Some(m) => {
                let start = m.scroll_offset.min(all.len());
                let end = (start + m.visible_lines).min(all.len());
                all[start..end].to_vec()
            }
            None => all,
        }
    }

    /// Returns `(row, col)` of the cursor relative to the scroll offset, for rendering.
    /// In single-line mode, returns `(0, cursor_position)`.
    pub fn cursor_display_position(&self) -> (usize, usize) {
        let width = self.wrap_width();
        let (vrow, vcol) = cursor_visual_pos(&self.text, self.cursor, width);
        let offset = self.scroll_offset();
        (vrow.saturating_sub(offset), vcol)
    }

    /// Get the effective wrap width. `None` means no wrapping (use usize::MAX).
    fn wrap_width(&self) -> Option<usize> {
        self.multiline.as_ref().and_then(|m| m.display_width)
    }

    /// Adjust scroll offset so the cursor's visual row is within the visible window.
    fn ensure_cursor_visible(&mut self) {
        let Some(m) = &mut self.multiline else {
            return;
        };
        let width = m.display_width;
        let (cursor_vrow, _) = cursor_visual_pos(&self.text, self.cursor, width);
        if cursor_vrow < m.scroll_offset {
            m.scroll_offset = cursor_vrow;
        } else if cursor_vrow >= m.scroll_offset + m.visible_lines {
            m.scroll_offset = cursor_vrow + 1 - m.visible_lines;
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

/// Compute visual lines from text, soft-wrapping at `width` characters.
/// Each entry is an owned string representing one visual row.
/// If `width` is `None`, lines are only split at `\n`.
fn compute_visual_lines(text: &str, width: Option<usize>) -> Vec<String> {
    let mut result = Vec::new();
    for logical in text.split('\n') {
        match width {
            Some(w) if w > 0 => {
                if logical.is_empty() {
                    result.push(String::new());
                } else {
                    let chars: Vec<char> = logical.chars().collect();
                    for chunk in chars.chunks(w) {
                        result.push(chunk.iter().collect());
                    }
                }
            }
            _ => result.push(logical.to_string()),
        }
    }
    result
}

/// Count the total number of visual lines (accounting for soft-wrap).
fn visual_line_count(text: &str, width: Option<usize>) -> usize {
    let mut count = 0;
    for logical in text.split('\n') {
        match width {
            Some(w) if w > 0 => {
                let char_len = logical.chars().count();
                if char_len == 0 {
                    count += 1;
                } else {
                    count += char_len.div_ceil(w); // ceil division
                }
            }
            _ => count += 1,
        }
    }
    count
}

/// Find the visual (row, col) of a cursor byte position, accounting for soft-wrap.
fn cursor_visual_pos(text: &str, cursor: usize, width: Option<usize>) -> (usize, usize) {
    let idx = clamp_cursor(text, cursor);
    let before = &text[..idx];
    let mut vrow = 0;

    // Process each logical line in the text before the cursor
    for (i, logical) in text.split('\n').enumerate() {
        let logical_start = if i == 0 {
            0
        } else {
            // Sum of all previous logical lines + their \n separators
            text.split('\n').take(i).map(|l| l.len() + 1).sum::<usize>()
        };
        let logical_end = logical_start + logical.len();

        if idx <= logical_end {
            // Cursor is on this logical line
            let col_in_logical = before[logical_start..].chars().count();
            match width {
                Some(w) if w > 0 => {
                    let wrapped_row = col_in_logical / w;
                    let wrapped_col = col_in_logical % w;
                    return (vrow + wrapped_row, wrapped_col);
                }
                _ => return (vrow, col_in_logical),
            }
        }

        // Count visual rows for this logical line
        match width {
            Some(w) if w > 0 => {
                let char_len = logical.chars().count();
                if char_len == 0 {
                    vrow += 1;
                } else {
                    vrow += char_len.div_ceil(w);
                }
            }
            _ => vrow += 1,
        }
    }

    // Fallback: cursor at end
    (vrow.saturating_sub(1), 0)
}

/// Compute the byte offset for a given visual (row, col), accounting for soft-wrap.
/// If the target row's visual line is shorter than `col`, clamps to line end.
fn byte_offset_at_visual(
    text: &str,
    target_vrow: usize,
    target_vcol: usize,
    width: Option<usize>,
) -> usize {
    let mut vrow = 0;
    let mut byte_pos = 0;

    for (i, logical) in text.split('\n').enumerate() {
        if i > 0 {
            byte_pos += 1; // account for the \n
        }
        let logical_start = byte_pos;

        match width {
            Some(w) if w > 0 => {
                let chars: Vec<char> = logical.chars().collect();
                let visual_rows = if chars.is_empty() {
                    1
                } else {
                    chars.len().div_ceil(w)
                };

                if target_vrow < vrow + visual_rows {
                    // Target is within this logical line
                    let row_within = target_vrow - vrow;
                    let char_start = row_within * w;
                    let line_char_len = chars.len();
                    let row_end = ((row_within + 1) * w).min(line_char_len);
                    let target_char = char_start.saturating_add(target_vcol).min(row_end);

                    // Convert char offset to byte offset
                    let byte_offset: usize =
                        chars[..target_char].iter().map(|c| c.len_utf8()).sum();
                    return logical_start + byte_offset;
                }
                vrow += visual_rows;
            }
            _ => {
                if vrow == target_vrow {
                    let chars: Vec<char> = logical.chars().collect();
                    let target_char = target_vcol.min(chars.len());
                    let byte_offset: usize =
                        chars[..target_char].iter().map(|c| c.len_utf8()).sum();
                    return logical_start + byte_offset;
                }
                vrow += 1;
            }
        }

        byte_pos = logical_start + logical.len();
    }

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

    // ── Multiline tests (no wrapping) ────────────────────────────

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
        let mut ti = multiline_input("hello\nworld\nfoo", 10);
        // Cursor at end (byte 15), visual row 2, col 3
        assert_eq!(ti.cursor_display_position(), (2, 3));

        ti.move_up();
        assert_eq!(ti.cursor_display_position(), (1, 3));
        assert_eq!(ti.cursor, 9);

        ti.move_up();
        assert_eq!(ti.cursor_display_position(), (0, 3));
        assert_eq!(ti.cursor, 3);

        // Already at row 0, no-op
        ti.move_up();
        assert_eq!(ti.cursor_display_position(), (0, 3));

        ti.move_down();
        assert_eq!(ti.cursor_display_position(), (1, 3));

        ti.move_down();
        assert_eq!(ti.cursor_display_position(), (2, 3));

        // Already at last row, no-op
        ti.move_down();
        assert_eq!(ti.cursor_display_position(), (2, 3));
    }

    #[test]
    fn multiline_up_clamps_to_shorter_line() {
        let mut ti = multiline_input("hi\nworld", 10);
        assert_eq!(ti.cursor_display_position(), (1, 5));

        ti.move_up();
        assert_eq!(ti.cursor_display_position(), (0, 2));
        assert_eq!(ti.cursor, 2);
    }

    #[test]
    fn multiline_down_clamps_to_shorter_line() {
        let mut ti = multiline_input("world\nhi", 10);
        ti.cursor = 5; // end of "world", col 5

        ti.move_down();
        assert_eq!(ti.cursor_display_position(), (1, 2));
        assert_eq!(ti.cursor, 8);
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
        ti.cursor = 9; // "world" col 3

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
        let mut ti = multiline_input("a\nb\nc\nd\ne", 3);
        ti.cursor = 0;
        assert_eq!(ti.scroll_offset(), 0);

        ti.move_down();
        ti.move_down();
        assert_eq!(ti.scroll_offset(), 0);

        ti.move_down(); // row 3 — scrolls
        assert_eq!(ti.scroll_offset(), 1);

        ti.move_down(); // row 4 — scrolls more
        assert_eq!(ti.scroll_offset(), 2);
    }

    #[test]
    fn scroll_offset_adjusts_on_move_up() {
        let mut ti = multiline_input("a\nb\nc\nd\ne", 3);
        ti.ensure_cursor_visible();
        assert_eq!(ti.scroll_offset(), 2);

        ti.move_up(); // row 3
        assert_eq!(ti.scroll_offset(), 2);

        ti.move_up(); // row 2
        assert_eq!(ti.scroll_offset(), 2);

        ti.move_up(); // row 1 — scrolls
        assert_eq!(ti.scroll_offset(), 1);

        ti.move_up(); // row 0
        assert_eq!(ti.scroll_offset(), 0);
    }

    #[test]
    fn visible_lines_returns_correct_slice() {
        let mut ti = multiline_input("a\nb\nc\nd\ne", 3);
        ti.cursor = 0;
        assert_eq!(ti.visible_lines(), vec!["a", "b", "c"]);

        ti.move_down();
        ti.move_down();
        ti.move_down();
        assert_eq!(ti.visible_lines(), vec!["b", "c", "d"]);
    }

    #[test]
    fn cursor_display_position_accounts_for_scroll() {
        let mut ti = multiline_input("a\nb\nc\nd\ne", 3);
        ti.cursor = 0;
        assert_eq!(ti.cursor_display_position(), (0, 0));

        ti.move_down();
        ti.move_down();
        ti.move_down();
        assert_eq!(ti.scroll_offset(), 1);
        assert_eq!(ti.cursor_display_position(), (2, 0)); // row 3 - offset 1 = 2
    }

    #[test]
    fn total_lines_count_no_wrap() {
        // Without display_width set, total_lines counts logical lines
        assert_eq!(TextInput::new().total_lines(), 1);
        assert_eq!(TextInput::with_text("a".into()).total_lines(), 1);
        assert_eq!(TextInput::with_text("a\nb".into()).total_lines(), 2);
        assert_eq!(TextInput::with_text("a\nb\nc".into()).total_lines(), 3);
        assert_eq!(TextInput::with_text("\n".into()).total_lines(), 2);
    }

    // ── Visual line helper tests ──────────────────────────────────

    #[test]
    fn byte_offset_at_visual_no_wrap() {
        let text = "hello\nworld\nfoo";
        assert_eq!(byte_offset_at_visual(text, 0, 0, None), 0);
        assert_eq!(byte_offset_at_visual(text, 0, 3, None), 3);
        assert_eq!(byte_offset_at_visual(text, 0, 5, None), 5);
        assert_eq!(byte_offset_at_visual(text, 1, 0, None), 6);
        assert_eq!(byte_offset_at_visual(text, 1, 3, None), 9);
        assert_eq!(byte_offset_at_visual(text, 2, 0, None), 12);
        assert_eq!(byte_offset_at_visual(text, 2, 3, None), 15);
    }

    #[test]
    fn byte_offset_at_visual_clamps_col() {
        let text = "hi\nworld";
        assert_eq!(byte_offset_at_visual(text, 0, 5, None), 2);
    }

    // ── Soft-wrap tests ───────────────────────────────────────────

    #[test]
    fn compute_visual_lines_no_wrap() {
        assert_eq!(
            compute_visual_lines("hello\nworld", None),
            vec!["hello", "world"]
        );
    }

    #[test]
    fn compute_visual_lines_with_wrap() {
        // "abcdefgh" with width 3 → ["abc", "def", "gh"]
        assert_eq!(
            compute_visual_lines("abcdefgh", Some(3)),
            vec!["abc", "def", "gh"]
        );
    }

    #[test]
    fn compute_visual_lines_wrap_with_newlines() {
        // "abcde\nfgh" with width 3 → ["abc", "de", "fgh"]
        assert_eq!(
            compute_visual_lines("abcde\nfgh", Some(3)),
            vec!["abc", "de", "fgh"]
        );
    }

    #[test]
    fn compute_visual_lines_empty_line() {
        assert_eq!(compute_visual_lines("a\n\nb", Some(5)), vec!["a", "", "b"]);
    }

    #[test]
    fn compute_visual_lines_exact_width() {
        // "abc" with width 3 → ["abc"] (no extra empty line)
        assert_eq!(compute_visual_lines("abc", Some(3)), vec!["abc"]);
    }

    #[test]
    fn visual_line_count_with_wrap() {
        // "abcdefgh" with width 3 → 3 visual lines
        assert_eq!(visual_line_count("abcdefgh", Some(3)), 3);
        // "abcde\nfgh" with width 3 → 2 + 1 = 3
        assert_eq!(visual_line_count("abcde\nfgh", Some(3)), 3);
        // "abc" with width 3 → 1
        assert_eq!(visual_line_count("abc", Some(3)), 1);
    }

    #[test]
    fn cursor_visual_pos_with_wrap() {
        // "abcdefgh" with width 3
        // Visual lines: ["abc", "def", "gh"]
        // Cursor at byte 0 ('a') → row 0, col 0
        assert_eq!(cursor_visual_pos("abcdefgh", 0, Some(3)), (0, 0));
        // Cursor at byte 2 ('c') → row 0, col 2
        assert_eq!(cursor_visual_pos("abcdefgh", 2, Some(3)), (0, 2));
        // Cursor at byte 3 ('d') → row 1, col 0
        assert_eq!(cursor_visual_pos("abcdefgh", 3, Some(3)), (1, 0));
        // Cursor at byte 6 ('g') → row 2, col 0
        assert_eq!(cursor_visual_pos("abcdefgh", 6, Some(3)), (2, 0));
        // Cursor at byte 8 (end) → row 2, col 2
        assert_eq!(cursor_visual_pos("abcdefgh", 8, Some(3)), (2, 2));
    }

    #[test]
    fn cursor_visual_pos_wrap_with_newline() {
        // "abcde\nfg" with width 3
        // Visual lines: ["abc", "de", "fg"]
        // Cursor at byte 4 ('e') → logical line 0, char col 4, row=4/3=1, col=4%3=1
        assert_eq!(cursor_visual_pos("abcde\nfg", 4, Some(3)), (1, 1));
        // Cursor at byte 6 ('f') → logical line 1, char col 0, row=2, col=0
        assert_eq!(cursor_visual_pos("abcde\nfg", 6, Some(3)), (2, 0));
    }

    #[test]
    fn byte_offset_at_visual_with_wrap() {
        // "abcdefgh" with width 3
        // Visual lines: ["abc", "def", "gh"]
        assert_eq!(byte_offset_at_visual("abcdefgh", 0, 0, Some(3)), 0); // 'a'
        assert_eq!(byte_offset_at_visual("abcdefgh", 1, 0, Some(3)), 3); // 'd'
        assert_eq!(byte_offset_at_visual("abcdefgh", 1, 2, Some(3)), 5); // 'f'
        assert_eq!(byte_offset_at_visual("abcdefgh", 2, 0, Some(3)), 6); // 'g'
        assert_eq!(byte_offset_at_visual("abcdefgh", 2, 1, Some(3)), 7); // 'h'
        // Clamp col beyond line end
        assert_eq!(byte_offset_at_visual("abcdefgh", 2, 5, Some(3)), 8); // end
    }

    #[test]
    fn multiline_up_down_with_soft_wrap() {
        // "abcdef" with width 3, visible 10
        // Visual lines: ["abc", "def"]
        let mut ti = multiline_input("abcdef", 10);
        ti.set_display_width(Some(3));
        ti.cursor = 0; // row 0, col 0

        ti.move_down(); // → row 1, col 0 → byte 3 ('d')
        assert_eq!(ti.cursor, 3);
        assert_eq!(ti.cursor_display_position(), (1, 0));

        ti.move_up(); // → row 0, col 0 → byte 0 ('a')
        assert_eq!(ti.cursor, 0);
        assert_eq!(ti.cursor_display_position(), (0, 0));
    }

    #[test]
    fn multiline_home_end_with_soft_wrap() {
        // "abcdef" with width 3
        // Visual lines: ["abc", "def"]
        let mut ti = multiline_input("abcdef", 10);
        ti.set_display_width(Some(3));
        ti.cursor = 4; // row 1, col 1 ('e')

        ti.move_line_home(); // → start of visual row 1 → byte 3
        assert_eq!(ti.cursor, 3);

        ti.move_line_end(); // → end of visual row 1 → byte 6 (end)
        assert_eq!(ti.cursor, 6);
    }

    #[test]
    fn total_lines_with_wrap() {
        let mut ti = multiline_input("abcdefgh", 10);
        ti.set_display_width(Some(3));
        assert_eq!(ti.total_lines(), 3); // "abc", "def", "gh"
    }

    #[test]
    fn visible_lines_with_wrap() {
        let mut ti = multiline_input("abcdefgh\nxy", 3);
        ti.set_display_width(Some(3));
        ti.cursor = 0;
        // Visual lines: "abc", "def", "gh", "xy" — show first 3
        assert_eq!(ti.visible_lines(), vec!["abc", "def", "gh"]);
    }

    #[test]
    fn scroll_with_wrapped_lines() {
        // "abcdef\ng" with width 3
        // Visual lines: ["abc", "def", "g"] — 3 visual lines
        let mut ti = multiline_input("abcdef\ng", 2);
        ti.set_display_width(Some(3));
        ti.cursor = 0;
        assert_eq!(ti.scroll_offset(), 0);

        ti.move_down(); // row 1
        assert_eq!(ti.scroll_offset(), 0);

        ti.move_down(); // row 2 — scrolls
        assert_eq!(ti.scroll_offset(), 1);
        assert_eq!(ti.visible_lines(), vec!["def", "g"]);
    }
}
