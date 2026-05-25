---
title: Managing Macros
description: Define reusable text snippets in config and send them to an agent or terminal with a quick keystroke.
group: Guides
order: 20
---

Macros are named text snippets stored in your config. When you open the macro
bar, dux shows only the macros that make sense for whatever is currently focused
(agent pane or terminal pane) and writes the selected text directly into the
PTY as if you had typed it yourself. Good for prompts you repeat constantly,
long build commands you never want to mistype, or anything you find yourself
copy-pasting.

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

## Opening the macro bar

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

## Managing macros in the app

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

## Adding macros directly in config

You can also manage macros entirely by hand. Open `config.toml` (use
`dux config path` to locate it), add entries under `[macros]`, and save. The
changes take effect the next time dux reads its config; no restart needed for
new sessions.

- **Linux:** `~/.config/dux/config.toml`
- **macOS:** `~/.dux/config.toml`
