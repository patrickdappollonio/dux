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
    /// Optional placeholder text shown when the input is empty.
    placeholder: Option<String>,
    /// Temporary overlay message that replaces the display (e.g. a loading
    /// indicator). While set, the underlying text is preserved and any edits
    /// are applied normally — when the overlay is dismissed the current text
    /// is shown.
    overlay: Option<String>,
    /// Optional mapper consulted before each character insertion. Receives
    /// the current text, the cursor byte-offset where the character would be
    /// inserted, and the candidate character. Return `Some(c)` to insert `c`
    /// (which may differ from the input), or `None` to silently reject.
    char_map: Option<fn(&str, usize, char) -> Option<char>>,
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
            placeholder: None,
            overlay: None,
            char_map: None,
        }
    }

    pub fn with_text(text: String) -> Self {
        let cursor = text.len();
        Self {
            text,
            cursor,
            multiline: None,
            placeholder: None,
            overlay: None,
            char_map: None,
        }
    }

    /// Set placeholder text shown when the input is empty.
    pub fn with_placeholder(mut self, placeholder: impl Into<String>) -> Self {
        self.placeholder = Some(placeholder.into());
        self
    }

    /// Returns the placeholder text, if set, for rendering when the input is empty.
    pub fn placeholder(&self) -> Option<&str> {
        self.placeholder.as_deref()
    }

    /// Set a temporary overlay message that replaces the display.
    /// The underlying text is preserved; edits applied while the overlay
    /// is active take effect normally and are visible once dismissed.
    pub fn set_overlay(&mut self, message: impl Into<String>) {
        self.overlay = Some(message.into());
    }

    /// Dismiss the overlay, restoring the normal text display.
    pub fn clear_overlay(&mut self) {
        self.overlay = None;
    }

    /// Returns the overlay message if one is active.
    pub fn overlay(&self) -> Option<&str> {
        self.overlay.as_deref()
    }

    /// Set a character mapper that is consulted before each insertion.
    /// The mapper receives the current text, cursor byte-offset, and candidate
    /// character. Return `Some(c)` to insert `c` (which may differ from the
    /// input for transparent substitution), or `None` to silently reject.
    pub fn with_char_map(mut self, map: fn(&str, usize, char) -> Option<char>) -> Self {
        self.char_map = Some(map);
        self
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
        let ch = if let Some(map) = self.char_map {
            match map(&self.text, index, ch) {
                Some(c) => c,
                None => return,
            }
        } else {
            ch
        };
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

    /// Set the cursor position from a visual (row, col) relative to the scroll
    /// offset, as produced by a mouse click. The row is relative to the visible
    /// area (0 = first visible line), not the absolute visual row.
    pub fn set_cursor_from_display_pos(&mut self, display_row: usize, display_col: usize) {
        let offset = self.scroll_offset();
        let abs_row = offset + display_row;
        let width = self.wrap_width();
        self.cursor = byte_offset_at_visual(&self.text, abs_row, display_col, width);
    }

    /// Scroll by a signed number of lines (positive = down, negative = up).
    /// Clamps to valid range. No-op in single-line mode.
    pub fn scroll_by(&mut self, delta: isize) {
        let Some(m) = &mut self.multiline else {
            return;
        };
        let total = visual_line_count(&self.text, m.display_width);
        let max_scroll = total.saturating_sub(m.visible_lines);
        if delta >= 0 {
            m.scroll_offset = (m.scroll_offset + delta as usize).min(max_scroll);
        } else {
            m.scroll_offset = m.scroll_offset.saturating_sub(delta.unsigned_abs());
        }
    }

    /// Update the visible line count (e.g. when the render area height changes).
    pub fn set_visible_lines(&mut self, visible: usize) {
        if let Some(m) = &mut self.multiline {
            m.visible_lines = visible;
        }
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

/// Compute visual lines using word-aware soft-wrapping.
fn compute_visual_lines(text: &str, width: Option<usize>) -> Vec<String> {
    wordwrap_visual_lines(text, width)
}

/// Count total visual lines using word-aware wrapping.
fn visual_line_count(text: &str, width: Option<usize>) -> usize {
    wordwrap_line_count(text, width)
}

/// Find the visual (row, col) of a cursor byte position using word-aware wrapping.
fn cursor_visual_pos(text: &str, cursor: usize, width: Option<usize>) -> (usize, usize) {
    wordwrap_cursor_pos(text, cursor, width)
}

/// Compute byte offset for a visual (row, col) using word-aware wrapping.
fn byte_offset_at_visual(
    text: &str,
    target_vrow: usize,
    target_vcol: usize,
    width: Option<usize>,
) -> usize {
    wordwrap_byte_offset(text, target_vrow, target_vcol, width)
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

// ── Word-aware wrapping ────────────────────────────────────────────
//
// These functions wrap text preferring to break at word boundaries (spaces).
// When a word is longer than the available width, it falls back to hard
// character-level splitting. They replace the naive `chars.chunks(w)`
// approach used previously.

/// Split a single logical line (no `\n`) into visual rows, preferring
/// breaks at the last space within `width`. Returns a vec of
/// `(char_start, char_end)` ranges into the char array.
fn wrap_logical_line(chars: &[char], width: usize) -> Vec<(usize, usize)> {
    if chars.is_empty() {
        return vec![(0, 0)];
    }
    let mut rows = Vec::new();
    let mut pos = 0;
    while pos < chars.len() {
        let remaining = chars.len() - pos;
        if remaining <= width {
            rows.push((pos, chars.len()));
            break;
        }
        // Look for the last space within [pos..pos+width].
        let window_end = pos + width;
        let mut break_at = None;
        for i in (pos..window_end).rev() {
            if chars[i] == ' ' {
                // Break after the space — the space stays on this line.
                break_at = Some(i + 1);
                break;
            }
        }
        match break_at {
            Some(bp) if bp > pos => {
                rows.push((pos, bp));
                pos = bp;
            }
            _ => {
                // No space found — hard break at width.
                rows.push((pos, window_end));
                pos = window_end;
            }
        }
    }
    rows
}

/// Compute visual lines from text using word-aware soft-wrapping.
/// If `width` is `None`, lines are only split at `\n`.
fn wordwrap_visual_lines(text: &str, width: Option<usize>) -> Vec<String> {
    let mut result = Vec::new();
    for logical in text.split('\n') {
        match width {
            Some(w) if w > 0 => {
                let chars: Vec<char> = logical.chars().collect();
                for (start, end) in wrap_logical_line(&chars, w) {
                    result.push(chars[start..end].iter().collect());
                }
            }
            _ => result.push(logical.to_string()),
        }
    }
    result
}

/// Count total visual lines using word-aware wrapping.
fn wordwrap_line_count(text: &str, width: Option<usize>) -> usize {
    let mut count = 0;
    for logical in text.split('\n') {
        match width {
            Some(w) if w > 0 => {
                let chars: Vec<char> = logical.chars().collect();
                count += wrap_logical_line(&chars, w).len();
            }
            _ => count += 1,
        }
    }
    count
}

/// Find the visual (row, col) of a cursor byte position using word-aware wrapping.
fn wordwrap_cursor_pos(text: &str, cursor: usize, width: Option<usize>) -> (usize, usize) {
    let idx = clamp_cursor(text, cursor);
    let before = &text[..idx];
    let mut vrow = 0;

    for (i, logical) in text.split('\n').enumerate() {
        let logical_start = if i == 0 {
            0
        } else {
            text.split('\n').take(i).map(|l| l.len() + 1).sum::<usize>()
        };
        let logical_end = logical_start + logical.len();

        if idx <= logical_end {
            let col_in_logical = before[logical_start..].chars().count();
            match width {
                Some(w) if w > 0 => {
                    let chars: Vec<char> = logical.chars().collect();
                    let rows = wrap_logical_line(&chars, w);
                    for (ri, &(start, end)) in rows.iter().enumerate() {
                        let is_last_row = ri + 1 == rows.len();
                        let col_offset = col_in_logical - start;
                        let row_len = end - start;
                        if col_in_logical >= start && (col_offset < row_len || is_last_row) {
                            return (vrow + ri, col_offset);
                        }
                    }
                    // Fallback: past the last row
                    let last = rows.len().saturating_sub(1);
                    return (vrow + last, col_in_logical - rows[last].0);
                }
                _ => return (vrow, col_in_logical),
            }
        }

        match width {
            Some(w) if w > 0 => {
                let chars: Vec<char> = logical.chars().collect();
                vrow += wrap_logical_line(&chars, w).len();
            }
            _ => vrow += 1,
        }
    }

    (vrow.saturating_sub(1), 0)
}

/// Compute byte offset for a visual (row, col) using word-aware wrapping.
fn wordwrap_byte_offset(
    text: &str,
    target_vrow: usize,
    target_vcol: usize,
    width: Option<usize>,
) -> usize {
    let mut vrow = 0;
    let mut byte_pos = 0;

    for (i, logical) in text.split('\n').enumerate() {
        if i > 0 {
            byte_pos += 1; // \n
        }
        let logical_start = byte_pos;

        match width {
            Some(w) if w > 0 => {
                let chars: Vec<char> = logical.chars().collect();
                let rows = wrap_logical_line(&chars, w);

                if target_vrow < vrow + rows.len() {
                    let ri = target_vrow - vrow;
                    let (row_char_start, row_char_end) = rows[ri];
                    let row_len = row_char_end - row_char_start;
                    let target_char = row_char_start + target_vcol.min(row_len);
                    let byte_offset: usize =
                        chars[..target_char].iter().map(|c| c.len_utf8()).sum();
                    return logical_start + byte_offset;
                }
                vrow += rows.len();
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

    // ── Word-aware wrapping tests ─────────────────────────────────

    #[test]
    fn wordwrap_no_wrap_needed() {
        assert_eq!(
            wordwrap_visual_lines("hello world", None),
            vec!["hello world"]
        );
        assert_eq!(
            wordwrap_visual_lines("hello world", Some(20)),
            vec!["hello world"]
        );
    }

    #[test]
    fn wordwrap_breaks_at_space() {
        // "hello world" at width 8: "hello " fits (6 chars), "world" on next line
        assert_eq!(
            wordwrap_visual_lines("hello world", Some(8)),
            vec!["hello ", "world"]
        );
    }

    #[test]
    fn wordwrap_breaks_at_last_space_in_window() {
        // "one two three" at width 10: "one two " (8 chars) fits, "three" next
        assert_eq!(
            wordwrap_visual_lines("one two three", Some(10)),
            vec!["one two ", "three"]
        );
    }

    #[test]
    fn wordwrap_hard_break_when_no_space() {
        // "abcdefghij" at width 4: no spaces, hard breaks
        assert_eq!(
            wordwrap_visual_lines("abcdefghij", Some(4)),
            vec!["abcd", "efgh", "ij"]
        );
    }

    #[test]
    fn wordwrap_long_word_then_short() {
        // "abcdefgh xy" at width 5: "abcde" hard, "fgh " word, "xy"
        assert_eq!(
            wordwrap_visual_lines("abcdefgh xy", Some(5)),
            vec!["abcde", "fgh ", "xy"]
        );
    }

    #[test]
    fn wordwrap_multiple_spaces() {
        // "a  b  c" at width 4: "a  " (break at last space), "b  c" fits (4 chars)
        assert_eq!(
            wordwrap_visual_lines("a  b  c", Some(4)),
            vec!["a  ", "b  c"]
        );
    }

    #[test]
    fn wordwrap_with_newlines() {
        assert_eq!(
            wordwrap_visual_lines("hello world\nfoo bar", Some(8)),
            vec!["hello ", "world", "foo bar"]
        );
    }

    #[test]
    fn wordwrap_empty() {
        assert_eq!(wordwrap_visual_lines("", Some(10)), vec![""]);
        assert_eq!(wordwrap_visual_lines("", None), vec![""]);
    }

    #[test]
    fn wordwrap_exact_width() {
        // "abcde" at width 5: fits exactly, no wrap
        assert_eq!(wordwrap_visual_lines("abcde", Some(5)), vec!["abcde"]);
    }

    #[test]
    fn wordwrap_space_at_boundary() {
        // "abcd efgh" at width 5: "abcd " (break after space), "efgh"
        assert_eq!(
            wordwrap_visual_lines("abcd efgh", Some(5)),
            vec!["abcd ", "efgh"]
        );
    }

    #[test]
    fn wordwrap_line_count_matches_lines() {
        let text = "hello world foo bar";
        let width = Some(8);
        let lines = wordwrap_visual_lines(text, width);
        assert_eq!(wordwrap_line_count(text, width), lines.len());
    }

    #[test]
    fn wordwrap_line_count_with_newlines() {
        let text = "hello world\nfoo bar baz";
        let width = Some(8);
        let lines = wordwrap_visual_lines(text, width);
        assert_eq!(wordwrap_line_count(text, width), lines.len());
    }

    #[test]
    fn wordwrap_cursor_pos_simple() {
        // "hello world" at width 8 → ["hello ", "world"]
        // Cursor at byte 0 ('h') → row 0, col 0
        assert_eq!(wordwrap_cursor_pos("hello world", 0, Some(8)), (0, 0));
        // Cursor at byte 5 (' ') → row 0, col 5 (space is on first line)
        assert_eq!(wordwrap_cursor_pos("hello world", 5, Some(8)), (0, 5));
        // Cursor at byte 6 ('w') → row 1, col 0
        assert_eq!(wordwrap_cursor_pos("hello world", 6, Some(8)), (1, 0));
        // Cursor at byte 11 (end) → row 1, col 5
        assert_eq!(wordwrap_cursor_pos("hello world", 11, Some(8)), (1, 5));
    }

    #[test]
    fn wordwrap_cursor_pos_hard_break() {
        // "abcdefgh" at width 5 → ["abcde", "fgh"]
        assert_eq!(wordwrap_cursor_pos("abcdefgh", 0, Some(5)), (0, 0));
        assert_eq!(wordwrap_cursor_pos("abcdefgh", 4, Some(5)), (0, 4));
        assert_eq!(wordwrap_cursor_pos("abcdefgh", 5, Some(5)), (1, 0));
        assert_eq!(wordwrap_cursor_pos("abcdefgh", 7, Some(5)), (1, 2));
    }

    #[test]
    fn wordwrap_byte_offset_simple() {
        // "hello world" at width 8 → ["hello ", "world"]
        assert_eq!(wordwrap_byte_offset("hello world", 0, 0, Some(8)), 0); // 'h'
        assert_eq!(wordwrap_byte_offset("hello world", 0, 5, Some(8)), 5); // ' '
        assert_eq!(wordwrap_byte_offset("hello world", 1, 0, Some(8)), 6); // 'w'
        assert_eq!(wordwrap_byte_offset("hello world", 1, 4, Some(8)), 10); // 'd'
    }

    #[test]
    fn wordwrap_byte_offset_clamps() {
        // "hello world" at width 8 → ["hello ", "world"]
        // Row 1 has 5 chars; requesting col 10 clamps to end
        assert_eq!(wordwrap_byte_offset("hello world", 1, 10, Some(8)), 11);
    }

    #[test]
    fn wordwrap_consistency_roundtrip() {
        // For every cursor position, cursor_pos → byte_offset should round-trip
        let text = "the quick brown fox jumps over the lazy dog";
        let width = Some(10);
        for cursor in 0..=text.len() {
            if !text.is_char_boundary(cursor) {
                continue;
            }
            let (row, col) = wordwrap_cursor_pos(text, cursor, width);
            let back = wordwrap_byte_offset(text, row, col, width);
            assert_eq!(
                back, cursor,
                "round-trip failed: cursor={cursor} → ({row},{col}) → {back}"
            );
        }
    }

    #[test]
    fn wordwrap_long_path_no_spaces() {
        // A long path with no spaces — hard breaks at width
        let path = "/var/home/user/.config/dux/worktrees/project";
        assert_eq!(
            wordwrap_visual_lines(path, Some(10)),
            vec![
                "/var/home/",
                "user/.conf",
                "ig/dux/wor",
                "ktrees/pro",
                "ject"
            ]
        );
    }

    #[test]
    fn wordwrap_path_with_spaces_around() {
        // Path embedded in text with spaces
        let text = "check /usr/local/bin/dux for details";
        // "check " (6) fits, "/usr/local/bin/" (15) fits exactly,
        // "dux for details" (15) fits exactly
        assert_eq!(
            wordwrap_visual_lines(text, Some(15)),
            vec!["check ", "/usr/local/bin/", "dux for details"]
        );
    }

    #[test]
    fn wordwrap_single_very_long_word() {
        assert_eq!(
            wordwrap_visual_lines("abcdefghijklmnopqrst", Some(7)),
            vec!["abcdefg", "hijklmn", "opqrst"]
        );
    }

    #[test]
    fn wordwrap_trailing_spaces() {
        // Trailing spaces should be preserved
        assert_eq!(
            wordwrap_visual_lines("hello   ", Some(10)),
            vec!["hello   "]
        );
    }

    #[test]
    fn wordwrap_leading_spaces() {
        assert_eq!(
            wordwrap_visual_lines("   hello world", Some(8)),
            vec!["   ", "hello ", "world"]
        );
    }

    #[test]
    fn wordwrap_width_one() {
        // Width 1: every char is its own line (hard break, no room for space logic)
        assert_eq!(
            wordwrap_visual_lines("ab cd", Some(1)),
            vec!["a", "b", " ", "c", "d"]
        );
    }

    #[test]
    fn wordwrap_width_two_with_spaces() {
        assert_eq!(
            wordwrap_visual_lines("a b c d", Some(2)),
            vec!["a ", "b ", "c ", "d"]
        );
    }

    #[test]
    fn wordwrap_unicode_path() {
        let text = "café/résumé/naïve";
        let lines = wordwrap_visual_lines(text, Some(6));
        // Should not break mid-character
        assert_eq!(lines, vec!["café/r", "ésumé/", "naïve"]);
    }

    #[test]
    fn wordwrap_long_path_roundtrip() {
        let path = "/var/home/user/.config/dux/worktrees/project/src/main.rs";
        let width = Some(12);
        for cursor in 0..=path.len() {
            if !path.is_char_boundary(cursor) {
                continue;
            }
            let (row, col) = wordwrap_cursor_pos(path, cursor, width);
            let back = wordwrap_byte_offset(path, row, col, width);
            assert_eq!(
                back, cursor,
                "path round-trip failed: cursor={cursor} → ({row},{col}) → {back}"
            );
        }
    }

    #[test]
    fn wordwrap_consistency_with_newlines_roundtrip() {
        let text = "hello world\nfoo bar baz\nqux";
        let width = Some(8);
        for cursor in 0..=text.len() {
            if !text.is_char_boundary(cursor) {
                continue;
            }
            let (row, col) = wordwrap_cursor_pos(text, cursor, width);
            let back = wordwrap_byte_offset(text, row, col, width);
            assert_eq!(
                back, cursor,
                "round-trip failed: cursor={cursor} → ({row},{col}) → {back}"
            );
        }
    }

    // ── Cursor boundary edge cases ────────────────────────────────

    #[test]
    fn cursor_at_end_of_exact_width_text() {
        // Text fills width exactly — cursor at end should be on the same row
        // at the column equal to the width, not overflow to a new row.
        let mut ti = multiline_input("abcde", 10);
        ti.set_display_width(Some(5));
        // "abcde" at width 5 = ["abcde"], cursor at end = byte 5
        let (row, col) = ti.cursor_display_position();
        assert_eq!((row, col), (0, 5));
        // The row must be within visible_lines
        let visible = ti.visible_lines();
        assert!(
            row < visible.len(),
            "cursor row {row} outside visible range ({})",
            visible.len()
        );
    }

    #[test]
    fn cursor_at_end_of_wrapped_text() {
        // "hello world" at width 8 → ["hello ", "world"]
        // Cursor at end (byte 11) → row 1, col 5
        let mut ti = multiline_input("hello world", 10);
        ti.set_display_width(Some(8));
        let (row, col) = ti.cursor_display_position();
        assert_eq!((row, col), (1, 5));
        let visible = ti.visible_lines();
        assert!(row < visible.len());
    }

    #[test]
    fn cursor_at_end_of_hard_wrapped_text() {
        // "abcdefghij" at width 5 → ["abcde", "fghij"]
        // Cursor at end (byte 10) → row 1, col 5
        let mut ti = multiline_input("abcdefghij", 10);
        ti.set_display_width(Some(5));
        let (row, col) = ti.cursor_display_position();
        assert_eq!((row, col), (1, 5));
        let visible = ti.visible_lines();
        assert!(row < visible.len());
    }

    #[test]
    fn cursor_stays_in_visible_area_after_newline_at_width_boundary() {
        // "abcde\n" at width 5 → ["abcde", ""]
        // Cursor at end (byte 6) → row 1, col 0
        let mut ti = multiline_input("abcde\n", 10);
        ti.set_display_width(Some(5));
        let (row, col) = ti.cursor_display_position();
        assert_eq!((row, col), (1, 0));
        let visible = ti.visible_lines();
        assert!(
            row < visible.len(),
            "cursor row {row} >= visible lines ({})",
            visible.len()
        );
    }

    #[test]
    fn cursor_display_never_exceeds_visible_lines() {
        // Exhaustive check: for every cursor position in various texts,
        // display row must be < visible_lines().len().
        let cases = &[
            ("hello world", 8usize),
            ("abcdefghij", 5),
            ("a b c d e f", 4),
            ("test\n", 10),
            ("line1\nline2\nline3", 6),
            ("/usr/local/bin/dux", 7),
        ];
        for &(text, width) in cases {
            let mut ti = multiline_input(text, 20);
            ti.set_display_width(Some(width));
            let visible = ti.visible_lines();
            for cursor in 0..=text.len() {
                if !text.is_char_boundary(cursor) {
                    continue;
                }
                ti.cursor = cursor;
                let (row, _col) = ti.cursor_display_position();
                assert!(
                    row < visible.len(),
                    "text={text:?} width={width} cursor={cursor}: \
                     display row {row} >= visible count {}",
                    visible.len()
                );
            }
        }
    }

    // ── char_map tests ─────────────────────────────────────────────

    /// A mapper that rejects the character 'x'.
    fn reject_x(_text: &str, _cursor: usize, ch: char) -> Option<char> {
        if ch == 'x' { None } else { Some(ch) }
    }

    #[test]
    fn map_rejects_char() {
        let mut input = TextInput::new().with_char_map(reject_x);
        input.handle_key(key(KeyCode::Char('a')));
        input.handle_key(key(KeyCode::Char('x')));
        input.handle_key(key(KeyCode::Char('b')));
        assert_eq!(input.text, "ab");
    }

    #[test]
    fn no_map_allows_all() {
        let mut input = TextInput::new();
        input.handle_key(key(KeyCode::Char('a')));
        input.handle_key(key(KeyCode::Char('x')));
        input.handle_key(key(KeyCode::Char('b')));
        assert_eq!(input.text, "axb");
    }

    /// A mapper that enforces a max length of 3 characters.
    fn max_three(text: &str, _cursor: usize, ch: char) -> Option<char> {
        if text.len() < 3 { Some(ch) } else { None }
    }

    #[test]
    fn map_receives_current_text() {
        let mut input = TextInput::new().with_char_map(max_three);
        for ch in "abcde".chars() {
            input.handle_key(key(KeyCode::Char(ch)));
        }
        assert_eq!(input.text, "abc");
    }

    #[test]
    fn map_applies_to_insert_char_directly() {
        let mut input = TextInput::new().with_char_map(reject_x);
        input.insert_char('x');
        input.insert_char('y');
        assert_eq!(input.text, "y");
    }

    /// A mapper that converts 'a' to 'A'.
    fn upcase_a(_text: &str, _cursor: usize, ch: char) -> Option<char> {
        if ch == 'a' { Some('A') } else { Some(ch) }
    }

    #[test]
    fn map_transforms_char() {
        let mut input = TextInput::new().with_char_map(upcase_a);
        input.handle_key(key(KeyCode::Char('a')));
        input.handle_key(key(KeyCode::Char('b')));
        input.handle_key(key(KeyCode::Char('a')));
        assert_eq!(input.text, "AbA");
    }
}
