---
title: Managing Macros
description: Define reusable text snippets in config and send them to an agent or terminal with a quick keystroke.
group: Guides
order: 20
---

Macros are named text snippets stored in your config. Trigger one and dux types it into
the focused agent or terminal for you. Good for prompts you repeat constantly, long
build commands you never want to mistype, or anything you keep copy-pasting.

One `[macros]` list drives both the terminal UI and the web UI. Edits made in either
place, or by hand in the file, land in the same block.

## Defining macros

Macros live under `[macros]` in `config.toml`. Each entry is a quoted name mapped to an
inline table with two fields:

| Field     | Type   | Required | Description |
|-----------|--------|----------|-------------|
| `text`    | string | yes      | The text sent when you trigger this macro. |
| `surface` | string | yes      | Which pane the macro appears in: `"agent"`, `"terminal"`, or `"both"`. |

```toml
[macros]
"Review" = { text = "review this code for bugs", surface = "agent" }
"Build"  = { text = "cargo build --release",     surface = "terminal" }
"Lint"   = { text = "cargo clippy",              surface = "both" }
```

Names are arbitrary strings: use whatever is scannable in the picker list. Declaration
order in the file is preserved in the UI.

> [!NOTE]
> A macro can also be written as its own `[macros.<name>]` section. TOML always reads
> those after every `name = { ... }` line, whatever order you write them in, so a macro
> in a section can never come before an inline one. dux never changes how a macro is
> written: when you reorder macros in dux, inline macros and section macros each take
> the new order among themselves. Keep all macros in one form to choose any order.

### Surface values

- `"agent"`: shown only when the agent pane is focused. For prompts you send to the AI.
- `"terminal"`: shown only when the terminal pane is focused. For shell commands you
  would rather not retype.
- `"both"`: shown on either pane.

Macros that do not match the current surface are filtered out, so the picker stays
short.

### Multi-line text

Write multi-line text with `\n` in a quoted string, or with a TOML multi-line basic
string:

```toml
[macros]
"Checklist" = { text = "check for:\n- logic errors\n- missing error handling\n- test coverage", surface = "agent" }
```

> [!NOTE]
> Every newline arrives as a soft line break, not a submit, so the whole macro lands as
> one composed prompt. You press Enter yourself to send it. There is no variable or
> placeholder expansion: what you write is exactly what gets sent.

## Sending a macro in the terminal UI

The macro bar opens with the `open_macro_bar` binding in `[keys]`; the in-app help
overlay shows the key it is currently bound to. It works whenever the agent or terminal
pane has your keys, windowed or fullscreen, so you can fire a macro mid-typing. With no
macros defined for the current surface, dux says so in the status line and does nothing.

The bar opens at the bottom of the pane, over the agent it will type into, listing only
the macros that pane can take:

![The terminal UI macro bar open over an agent pane, with a search field above a list of macro names and the text each one sends.](/screens/tui-macro-bar.png)

Once the bar is open:

- **Type** to filter by name or text content. Name matches rank first.
- **Up / Down** to move through the list.
- **Tab** to expand the highlighted name into the search field.
- **Enter** to send the highlighted macro and close the bar.
- **Esc** to dismiss without sending.

dux confirms with `Sent macro "<name>".` in the status line.

> [!IMPORTANT]
> A macro is a write like any other, so it goes through input ownership. If another
> device is driving that terminal, which can only happen while dux is
> [serving in the background](/docs/server-mode#serve-in-the-background-and-keep-the-tui),
> the macro is not sent. The status line names the device holding it and tells you to
> take it over first. Nothing half-sends.

## Sending a macro in the web UI

Every terminal pane, agent or companion terminal, has a macro button in its corner.
Click it for a picker of the macros matching that pane's surface, scoped to the pane you
clicked rather than whatever is focused. Type to filter, then click a macro or press
Enter to send it. The text simply appears at the prompt; there is no confirmation toast.

![The macro picker open in the browser above an agent's terminal, listing the macros for that pane with a search field and an Edit macros link at the bottom.](/screens/macro-popover.png)

If a pane has no macros for its surface, the popover says so and points you at the
editor. With no macros at all, it links straight to **Edit macros**.

> [!WARNING]
> The ownership rule applies here too, and it is quieter: if you are watching a terminal
> somebody else is driving, the macro is dropped and nothing tells you so. Use the
> pane's **Take over** button first.

## Managing macros in the terminal UI

Run `edit-macros` from the command palette to open the macros editor overlay. The action
has no default key binding; the palette is the intended entry point.

Inside the list:

- The list opens in declaration order. After you add or edit a macro it re-sorts
  alphabetically for the rest of the session. The order in `config.toml` is unchanged,
  except that renaming a macro moves its entry to the end.
- It is an ordinary picker: movement keys walk it, confirming on an entry opens it for
  editing, and the close-overlay key closes it. Every key resolves through `[keys]`, so
  rebinding one moves the footer hint with it.
- `new_macro` creates a macro, `delete_macro` stages a deletion and asks you to confirm.
- Rows are clickable: one click highlights, a double click opens.

The macro form is an ordinary modal, not a wizard. The name field, the text field, the
Agent / Terminal / Both selector, and the Cancel and Save buttons are all on screen at
once, and each is a focus stop. The movement keys (`toggle_selection` in `[keys]`) move
focus and never change a value; Space acts on whatever has focus. The name field takes
typing immediately. The text field is multiline, so it has an explicit edit mode you
enter deliberately (`confirm` while it has focus, `engage_commit_input`, or a double
click) and leave with `exit_commit_input`, which keeps your text. Typing on the field
before you engage it does nothing, and it draws no caret. Everything is clickable.
Cancelling writes nothing.

> [!NOTE]
> `clear_text_field` empties the text field only while the text field is the
> focused control. From any other focus stop it does nothing, so it cannot wipe the body
> while you are on the name field or a button.

Saving requires a name and some text, and refuses a name another macro already uses. dux
says which in the status line and keeps the form open.

Additions, edits, and deletions are written to `config.toml` immediately, and your
hand-edits survive: dux preserves your formatting and ordering.

## Managing macros in the web UI

Open the cog menu and pick **Configuration → Edit macros…**, or click **Edit macros**
from any terminal pane's macro popover. A dialog opens with the same list in declaration
order:

- **Add macro** opens a form for a new entry: a name, the text, and a surface picker
  (`Agent` / `Terminal` / `Both`).
- The pencil button on a row edits it through the same form. Renaming keeps its position
  in the list.
- The trash button stages a deletion and asks you to confirm inline.
- **Save** writes the whole list at once; **Cancel** discards your changes.

If the list changed somewhere else while the dialog was open (another browser, or a
script), Save is refused and nothing is written, so the other change is not lost. The
dialog keeps your edits and offers to reload the list, which drops them, or to keep
editing so you can copy what you need first.

## Managing macros from the command line

```bash
dux macros ls                                   # name, surface and the first line of text
dux macros show review                          # one macro, its whole text included
dux macros add review "Review this diff" --surface both
dux macros rm review
```

`add` saves a new macro, or replaces the one with that name where it stands in the list;
`--surface` takes `agent` (the default), `terminal` or `both`. Both changes ask before
they go ahead, and need `--yes` when there is no terminal to ask on. With dux running,
the change goes through it and applies at once; with dux stopped, it is written straight
into `config.toml`, comments kept, and applies the next time dux starts. `--remote
<name>` lists and changes a remote dux's macros instead.

A macro whose name is anything other than letters, digits, `_` and `-` keeps its text
hidden on the command line, the same rule `dux config get` follows: `ls` lists it under a
placeholder name, and `show` finds it by its real name but prints only that placeholder and
its surface. The browser's macro editor shows it in full.

## Adding macros directly in config

You can manage macros entirely by hand. Open `config.toml` (`dux config path` locates
it), add entries under `[macros]`, and save. The changes take effect the next time dux
reads its config; new sessions need no restart.

- **Linux:** `~/.config/dux/config.toml`
- **macOS:** `~/.dux/config.toml`
