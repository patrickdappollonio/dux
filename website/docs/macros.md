---
title: Managing Macros
description: Define reusable text snippets in config and send them to an agent or terminal with a quick keystroke.
group: Guides
order: 20
---

Macros are named text snippets stored in your config. When you trigger one, dux
shows only the macros that make sense for whatever is currently focused (agent
pane or terminal pane) and writes the selected text directly into the PTY as if
you had typed it yourself. Good for prompts you repeat constantly, long build
commands you never want to mistype, or anything you find yourself copy-pasting.

The same `[macros]` config drives both the terminal UI and the web UI. There is
one list, defined once; the TUI macro bar and the web macro picker both read it,
and edits made in either place (or by hand in the file) land in the same block.

## Defining macros

Macros live under `[macros]` in `config.toml`. Each entry is a quoted name
mapped to an inline table with two fields:

| Field     | Type   | Required | Description |
|-----------|--------|----------|-------------|
| `text`    | string | yes      | The text that gets sent to the PTY when you trigger this macro. |
| `surface` | string | yes      | Which pane the macro appears in. Accepted values: `"agent"`, `"terminal"`, or `"both"`. |

```toml
[macros]
"Review" = { text = "review this code for bugs", surface = "agent" }
"Build"  = { text = "cargo build --release",     surface = "terminal" }
"Lint"   = { text = "cargo clippy",              surface = "both" }
```

Names are arbitrary strings: use whatever is memorable and scannable in the
picker list. Declaration order in the file is preserved in the UI.

### Surface values

The `surface` field controls which pane the macro appears in when you open the
macro bar:

- `"agent"`: shown only when the agent pane is focused. Use this for prompts
  you send to the AI (review requests, refactoring instructions, etc.).
- `"terminal"`: shown only when the terminal pane is focused. Use this for
  shell commands you'd rather not retype.
- `"both"`: shown on either pane. Useful for text that makes sense in either
  context.

Macros that don't match the current surface are filtered out automatically, so
the picker stays short.

### Multi-line text

You can write multi-line text by including `\n` in a quoted string or by using
a TOML multi-line basic string. dux translates every newline to Alt+Enter
(ESC + CR) before writing to the PTY. That means the whole macro arrives as a
single composed prompt rather than submitting at each line break; you still
press Enter yourself to send.

```toml
[macros]
"Checklist" = { text = "check for:\n- logic errors\n- missing error handling\n- test coverage", surface = "agent" }
```

There is no variable or placeholder expansion in macro text. What you write is
exactly what gets sent.

## Sending a macro in the terminal UI

The macro bar is bound to **Ctrl-\\** by default (configurable under
`open_macro_bar` in `[keys]`). It is available while a pane is in interactive
mode. If no macros are defined for the current surface, dux shows a status
message and does nothing.

Once the bar is open:

- **Type** to filter by name or text content (name matches are ranked first).
- **Up / Down** to move through the list.
- **Tab** to expand the highlighted name into the search field.
- **Enter** to send the highlighted macro to the PTY and close the bar.
- **Esc** to dismiss without sending.

dux writes the macro bytes directly to the active PTY client and shows
`Sent macro "<name>".` in the status line.

## Sending a macro in the web UI

In the browser, every terminal pane (agent or companion terminal) has a macro
button in its corner. Click it to open a quick-picker popover listing the macros
that match that pane's surface — the same filtering the TUI macro bar does, just
scoped to the pane you clicked rather than whatever is focused. Type to filter,
then click a macro (or press Enter) to send it. The familiar
`Sent macro "<name>".` confirmation shows in the status line.

If a pane has no macros for its surface, the popover says so and points you at
the editor; if you have no macros at all, it links straight to **Edit macros**.

## Managing macros in the terminal UI

The `edit-macros` command palette action opens the macros editor overlay. You
can reach it through the command palette (open with **Ctrl-P** by default and
search for `edit-macros`). The `EditMacros` action has no default key binding;
the palette is the intended entry point.

Inside the list:

- The list shows all defined macros in declaration order.
- The list is an ordinary picker: the movement keys walk it, `confirm`
  (**Enter** by default) on a highlighted entry opens it for editing, and
  `close_overlay` (**Esc** by default) closes the overlay. Every key is
  resolved through `[keys]`, so rebinding one moves the footer hint with it.
- `new_macro` (**n** by default) creates a new macro. Either it or `confirm`
  opens the macro form described below.
- `delete_macro` (**d** or **Delete** by default) stages a deletion and shows
  a confirmation dialog.
- Rows are clickable: one click highlights a macro, a double click opens it.

The macro form is an ordinary modal, not a wizard. It shows the name field, the
text field, the Agent / Terminal / Both selector, and **Cancel** and **Save**
buttons all at once, and every one of them is a focus stop:

- The movement keys (`toggle_selection` in `[keys]`, **Tab** / **Shift-Tab** by
  default) move focus between the five controls. They never change a value.
- **Space** acts on whichever control has focus: it types a space in the name
  field or in the engaged text field, advances the surface selector, and
  activates a button.
- The name field takes typing immediately. The text field is multiline, so
  **Enter** there has to mean "new line" rather than "confirm", and it
  therefore has an edit mode. Three things engage it, and nothing else does:
  `confirm` (**Enter** by default) while the field has focus,
  `engage_commit_input` (**i** by default), and a double click on the field.
  Leave edit mode with `exit_commit_input` (**Esc** or **Ctrl-G**), which keeps
  the form open and your text intact. `clear_text_field` (**Ctrl-D** by
  default) empties the text field whenever the text field is the focused
  control, engaged or not; from any other focus stop it does nothing, so it can
  never wipe the body while you are on the name field or a button. Typing on an
  unengaged field does
  nothing: the footer names the key that starts editing, and the field draws no
  caret until it is really taking your keystrokes.
- **Esc** outside the text field's edit mode cancels the edit and writes
  nothing.
- Everything is clickable: clicking a field focuses it (clicking the text field
  again engages it), clicking a selector option picks it, clicking a button
  activates it.

Saving requires a name and some text, and refuses a name another macro already
uses; dux says which in the status line and keeps the form open.

All changes (additions, edits, and deletions) are persisted immediately to
`config.toml`. Hand-edits to the file are also respected: dux rewrites with
`toml_edit`, so your formatting and ordering survive.

## Managing macros in the web UI

The web UI has a full macro editor too. Open the cog menu and pick
**Configuration → Edit macros…**, or click **Edit macros** from any terminal
pane's macro popover. A dialog opens with the same list of macros in declaration order.

In the dialog:

- **Add macro** opens a form for a new entry: a name, the text, and a surface
  picker (`Agent` / `Terminal` / `Both`).
- The pencil button on a row edits it through the same form; renaming an entry
  keeps its position in the list.
- The trash button stages a deletion and asks you to confirm inline.
- **Save** writes the whole list at once; **Cancel** discards your changes.

Because it edits the same `[macros]` block, anything you save here shows up in
the terminal UI (and on disk) just like a hand-edit would, and vice versa.

## Adding macros directly in config

You can also manage macros entirely by hand. Open `config.toml` (use
`dux config path` to locate it), add entries under `[macros]`, and save. The
changes take effect the next time dux reads its config; no restart needed for
new sessions.

- **Linux:** `~/.config/dux/config.toml`
- **macOS:** `~/.dux/config.toml`
