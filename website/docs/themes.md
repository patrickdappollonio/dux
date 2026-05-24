---
title: Managing Themes
description: Switch the built-in themes, preview them live, or write your own from scratch.
group: Guides
order: 40
---

dux ships with a generous set of built-in themes and lets you write your own. Color
is part of staying sane while juggling agents, so dux treats theming as a
first-class setting rather than an afterthought.

## Changing the theme

The theme lives under the `[ui]` section of `config.toml`:

```toml
[ui]
theme = "dux_dark"   # the default
```

Set it to any built-in name and restart, or — easier — open the **theme picker**
from the command palette and arrow through the options with a live preview. No
config editing, no guessing what a palette looks like.

Built-in names use underscores. A few you might recognize:

```toml
theme = "catppuccin_mocha"
theme = "nord"
theme = "dracula"
theme = "tokyo_night"
theme = "gruvbox_dark"
```

There are far more than these — Catppuccin's four flavors, Tokyo Night's variants,
Solarized, Rose Pine, Everforest, Kanagawa, One Dark, and others. Rather than
memorize a list that drifts over time, open the theme picker: it discovers every
built-in plus anything you've authored, and labels where each one came from.

## How a theme name is resolved

When you set `theme = "<name>"`, dux looks in this order:

1. A user theme at `<config dir>/themes/<name>.toml` — your own themes win first.
2. The bundled `dux_dark` theme, embedded in the binary.
3. An [Opaline](https://github.com/hyperb1iss/opaline) built-in (Catppuccin, Nord,
   Dracula, and friends).

If none match, dux tells you so and falls back to a safe default rather than
launching into an unreadable color scheme.

## Writing your own theme

Custom themes are TOML files. Put one at:

- **Linux:** `~/.config/dux/themes/<name>.toml`
- **macOS:** `~/.dux/themes/<name>.toml`

then point your config at it:

```toml
[ui]
theme = "my_theme"   # matches my_theme.toml in the themes directory
```

The file name (without `.toml`) is the theme's id.

### Start from the bundled theme

The most reliable way to build a theme is to copy the one dux already uses and
recolor it. Grab
[`assets/themes/dux_dark.toml`](https://github.com/patrickdappollonio/dux/blob/main/assets/themes/dux_dark.toml)
from the repository, drop it in your themes directory under a new name, and start
changing values. It defines every surface dux paints, so you'll never hit a missing
color.

### The shape of a theme file

Theme files use the Opaline format and have three parts that matter most:

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

- **Set `dux.*` tokens explicitly** for pixel-level control over each surface
  (borders, headers, selection, diff colors, status line, and the rest). This is
  what the bundled theme does.
- **Rely on the fallback.** Any `dux.*` token you leave out is derived from
  Opaline's standard semantic tokens (`text.*`, `bg.*`, `accent.*`, `border.*`,
  `code.*`). So a complete, standard Opaline theme works in dux as-is, even if it
  was never written with dux in mind.

### Preview as you go

After saving a theme file, open the theme picker from the command palette — your
theme shows up alongside the built-ins, labeled as user-authored, with the same
live preview. Tweak, save, re-pick, repeat until it feels right.
