# Terminal UI

## Where code goes

- `src/app/` splits the TUI into files that each hold an `impl App` block for one concern: `mod.rs` (the `App` struct, types, enums, bootstrap, run loop; read it first), `input.rs` (keys and mouse), `render.rs` (drawing), `sessions.rs` (project and agent CRUD), `workers.rs` (background threads).
- A new method goes in the file of its concern and a new type or enum in `mod.rs`; a concern that grows past about 200 lines gets its own file with an `impl App` block that imports what it uses by name (`use super::*;` only inside `mod tests`).
- Keep a change to one submodule where possible.

## Look

- Every color and style comes from `Theme` in `src/theme.rs`, never a raw `Color::*`; reuse a semantic field that matches, or add one and wire it through every theme in the same change.
- Icons come from `theme::Glyphs`: `PLAIN` by default, `NERD` when `[ui] nerd_font_icons` is on; a Nerd Font glyph is never in the retired U+F500 to U+FD46 range.
- Shared pieces live in `src/app/components/` and `src/app/modal.rs` (modal chrome, pane card, focus ring, key ladder, click routing, hint bar, `name_chip`, text input); reuse them and never write a second copy.
- A frame is drawn only when something it renders changed or an animation frame advanced: a new source of change calls `mark_frame_dirty` where it mutates, and a new animation joins the redraw gate at its own frame interval.
- The embedded PTY's caret is the host terminal's hardware cursor, never a painted cell, because IME composition popups anchor to it.

## Keys

- Tab and Shift-Tab move between panes; an agent's terminal dock, while drawn, is a pane between the agent and the Changes pane.
- Keys are bound per pane. In interactive mode every key goes to the PTY and dux's bindings, global ones included, are suppressed.
- Ctrl, Alt, Super, Hyper and Meta chords belong to dux and typing belongs to the agent; Tab is dux's unless `tab_reaches_agent` is on.
- Movement keys move focus and never change a value; Space and Enter act on the focused control, and Space types a space in a text field.
- A modal with more than one focusable control shows which one has focus.
- A key label shown to the user comes from `RuntimeBindings::label_for()` and is cased like `Ctrl-g`; `^X` is allowed in footer hint bars.
- Every key dux hands to a program is the classic form an xterm sends; Shift-Enter is a line feed (`0x0a`), a key with no classic form is dropped, and a key release never reaches a program.

## Modals and text

- Every modal is a `PromptState` variant, so Escape behaves the same everywhere.
- All text entry goes through the shared `TextInput`; never push characters onto a `String` by hand.
- A single-line field has no mode: while it has focus binding lookup is suppressed, so arrow keys reach the caret.
- A multiline field has an explicit engage step: unengaged, the modal owns every key; engaged, every key goes to the text and the exit-edit binding leaves edit mode; typing never engages a field.
- Filters, search rows and the command palette take typing at once, with no engage step.
- Every field with a caret publishes its rect into the overlay mouse layout, so a click places the caret.
- Caret positions are display columns, converted with the shared column and byte helpers; single-line fields render through the shared renderer.

## Rows another device drives

- Every engine call that changes an agent's row goes through `src/app/row_changes.rs`, behind the question of who drives the row; `row_card.rs` classifies which actions change a row.
