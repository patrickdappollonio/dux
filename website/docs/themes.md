---
title: Managing Themes
description: Switch the built-in themes, preview them live, or write your own from scratch.
group: Guides
order: 40
---

dux ships with a generous set of built-in themes and lets you write your own.

> [!IMPORTANT]
> Theming is a terminal UI feature. The web UI is dark-only: it ships one tuned dark
> palette, ignores your `theme` setting, and does not follow your system light/dark
> preference. A theme you write changes the terminal UI only.

## Changing the theme

Open the **theme picker** from the terminal UI's command palette and arrow through the
options with a live preview. It discovers every built-in plus anything you have
authored, and labels where each one came from.

![The terminal UI theme picker open over the workspace, listing dux_dark as the current theme with the bundled themes below it.](/screens/tui-theme-picker.png)

To set it by hand instead, edit the `[ui]` section of `config.toml` and restart:

```toml
[ui]
theme = "dux_dark"   # the default
```

Built-in names use underscores:

```toml
theme = "catppuccin_mocha"
theme = "nord"
theme = "dracula"
theme = "tokyo_night"
theme = "gruvbox_dark"
```

There are far more: Catppuccin's four flavors, Tokyo Night's variants, Solarized,
Rose Pine, Everforest, Kanagawa, One Dark, and others. The picker is the list that
never goes stale.

## Nerd Font icons

Running a [Nerd Font](https://www.nerdfonts.com) in your terminal? dux can trade its
plain icons for proper ones: a folder for the project, a star for standalone rows, the
pull-request and warning marks, the disclosure chevrons, and the serving and terminals
chips in the header. Out of the box dux draws the plain set every screenshot in these
docs shows, so it looks right in any font:

![The terminal UI sidebar: three active agents, one of them running three tabs, and a collapsed Inactive tail.](/screens/tui-agent-list-two-line.png)

Flip it with the `toggle-nerd-font-icons` command in the terminal UI's command palette.
It takes effect on the spot and is saved for next time. You can also find it as **Nerd
Font icons** in the web UI's Preferences dialog, or set it by hand:

```toml
[ui]
nerd_font_icons = true   # off by default
```

> [!IMPORTANT]
> This needs a Nerd Font (a patched font) set as your terminal's font. Without one the
> icons come out as empty boxes; flip the setting back and the plain set returns. It
> changes the terminal UI only: the web UI draws its own icons either way.

## How a theme name is resolved

When you set `theme = "<name>"`, dux looks in this order:

1. A user theme at `<config dir>/themes/<name>.toml`. Your own themes win first.
2. The bundled `dux_dark` theme.
3. An [Opaline](https://github.com/hyperb1iss/opaline) built-in (Catppuccin, Nord,
   Dracula, and friends).

> [!NOTE]
> A name that matches nothing is not fatal. dux tells you so and falls back to a safe
> default rather than launching into an unreadable color scheme.

## Writing your own theme

Custom themes are TOML files. Put one at:

- **Linux:** `~/.config/dux/themes/<name>.toml`
- **macOS:** `~/.dux/themes/<name>.toml`

then point your config at it:

```toml
[ui]
theme = "my_theme"   # matches my_theme.toml in the themes directory
```

The file name, without `.toml`, is the theme's id.

> [!TIP]
> Start from the theme dux already uses. Grab
> [`assets/themes/dux_dark.toml`](https://github.com/patrickdappollonio/dux/blob/main/assets/themes/dux_dark.toml)
> from the repository, drop it in your themes directory under a new name, and recolor
> it. It defines every surface dux paints, so you will never hit a missing color.

### The shape of a theme file

Theme files use the Opaline format. Three parts matter most:

```toml
[meta]
name = "My Theme"
author = "you"
variant = "dark"        # or "light"
description = "A custom dux theme"

[palette]
# Name your colors once, reuse them everywhere below.
bg     = "#11111d"
fg     = "#f4f7ff"
accent = "#00d4ff"
pink   = "#ff4fd8"

[tokens]
# dux.* tokens map a color onto a specific dux UI surface.
"dux.app_bg"         = "bg"
"dux.text_fg"        = "fg"
"dux.border_focused" = "accent"
"dux.title_focused"  = "accent"
"dux.selection_bg"   = "pink"
# ...and so on for every surface you want to control.
```

You have two ways to define colors:

- **Set `dux.*` tokens explicitly** for control over each surface: borders, headers,
  selection, diff colors, status line, and the rest. This is what the bundled theme
  does.
- **Rely on the fallback.** Any `dux.*` token you leave out is derived from Opaline's
  standard semantic tokens (`text.*`, `bg.*`, `accent.*`, `border.*`, `code.*`). A
  complete, standard Opaline theme works in dux as-is, even if it was never written
  with dux in mind.

Names inside dialogs (branches, paths, agent and project names) are drawn as small
chips in your dialog's own colors, swapped: the dialog background (`dux.overlay_bg`)
on the dialog text color (`dux.text_fg`). A chip is therefore exactly as readable as
the sentence around it, and there is nothing extra to set.

The cards that show up inside a pane, like the note in the changes pane when a
folder has no repository or a tab that isn't running, are not dialogs, so they sit on
the pane's own background (`dux.app_bg`) with just their border to frame them. Want
them to stand out? Set `dux.pane_card_bg` and they get a fill of their own.

Save the file and open the theme picker: your theme is listed alongside the built-ins,
labeled as user-authored, with the same live preview. Tweak, save, re-pick, repeat.
