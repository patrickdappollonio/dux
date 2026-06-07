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

Inside the editor:

- The list shows all defined macros in declaration order.
- **n** creates a new macro. dux asks for a name first, then the text, then
  lets you cycle the surface with **Tab** / **Shift-Tab**.
- **Enter** on a highlighted entry opens it for editing, following the same
  name → text → surface flow.
- **d** or **Delete** stages a deletion and shows a confirmation dialog.
- **Esc** in the name step cancels the edit; **Esc** in the text step saves
  the entry (provided the text is non-empty).
- **Esc** in the list view closes the overlay.

All changes (additions, edits, and deletions) are persisted immediately to
`config.toml`. Hand-edits to the file are also respected: dux rewrites with
`toml_edit`, so your formatting and ordering survive.

## Managing macros in the web UI

The web UI has a full macro editor too. Open the command palette (**Ctrl/⌘K**)
and pick **Edit macros**, or click **Edit macros** from any terminal pane's
macro popover. A dialog opens with the same list of macros in declaration order.

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
