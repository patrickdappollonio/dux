---
title: First run & what's new
description: The one-time welcome screen, the what's-new screen after an upgrade, how to reopen either on demand, and the two settings that turn the automatic ones off.
group: Getting started
order: 3
---

dux shows you a screen exactly twice in its life: once when it is brand new to your
machine, and once after each update. Both are dismissible with a button, and both can
be reopened later.

## The welcome screen

The first launch of a fresh install opens a welcome screen instead of dropping you into
an empty sidebar. It is a short orientation:

- **What a project is:** any git repo on this machine. Your checkout is left alone.
- **What an agent is:** a session on a project that gets **its own git worktree and a
  branch-style name**, so several agents work in parallel without tripping over each
  other's files.
- **What a provider is:** whatever AI CLI you point it at. If a tool runs in a
  terminal, it can be a provider.
- **Where your config file lives**, as the real path on *this* machine:
  `~/.config/dux/config.toml` on Linux, `~/.dux/config.toml` on macOS. That file is
  written fully commented, and it is the documentation.

Underneath sit the same three steps: add a project, create an agent, launch. Two
buttons close it out, one opening the project picker so you can add your first project
right there, the other simply closing the screen. The address of
[getdux.app](https://getdux.app) sits alongside them so you know where to find the rest,
and the frame itself tells you which key closes the screen.

Here is the whole thing on a fresh install, rubber duck included:

![The dux welcome screen in the terminal UI: a rubber duck drawn in braille dots on the left, the orientation text on the right explaining projects, agents and providers, an Add a project button beside a Close button and a link to getdux.app along the bottom, and the close key named on the frame itself.](/screens/tui-welcome-screen.png)

## What's new after an update

When dux starts and you are running a version whose screen you have not seen, you get a
**What's new** screen for the release you moved to. It renders that release's headline,
its opening paragraphs, and the titles of its feature sections. The GitHub changelog
and installer boilerplate are left out. One button opens the full notes on GitHub, the
other closes.

The screen always describes **the version you actually have**, so upgrading to a
release while a newer one is already published never shows you features you do not have
yet.

> [!NOTE]
> A development build never shows the what's-new screen on its own, because there is no
> published release to describe. It still gets the first-run welcome.

## Where the notes come from

dux fetches the matching release from GitHub's public API at launch. No token, nothing
to configure, and the result is cached next to your config so relaunching a dozen times
does not mean a dozen requests.

If the fetch cannot get through, dux shows nothing and says nothing. What happens next
depends on why:

- **Something that might work later** (offline, a timeout, a rate limit, a GitHub
  hiccup): the version is **not** recorded as seen, so the notes are waiting for you on
  a later launch that has a network. Starting dux on a plane does not cost you a
  release's notes.
- **A definitive "no such release"** (normal for a locally built or unpublished
  binary): the answer cannot change, so dux records the version and stops asking.

## Opening either screen on demand

Neither screen is a one-shot you can lose:

- **Terminal UI:** the command palette has `show-welcome-screen` and
  `show-release-notes`. Neither has a keybinding.
- **Browser (server mode):** the cog menu has **Welcome screen…** and **What's new…**.

> [!NOTE]
> Both surfaces share one record of which version you have seen. Dismissing the
> what's-new screen in the terminal also dismisses it in the browser.

## Turning the automatic screens off

Both are opt-out, under `[ui]`, and both default to `false`, meaning both screens are
on:

```toml
[ui]
disable_automated_welcome_screen = false
disable_release_notes            = false
```

> [!IMPORTANT]
> Each setting suppresses only the **automatic** appearance.
> `disable_automated_welcome_screen` stops the first-run screen from showing itself.
> `disable_release_notes` stops the what's-new screen from showing itself **and** skips
> the launch-time fetch, so nothing touches the network at startup. Opening either
> screen yourself still works, and the release notes still fetch when you do.

Turning either one off still moves your seen-version marker forward. Switch
`disable_release_notes` back on later and you get the *next* update's notes, not a
backlog of everything you skipped.

If you would rather not hand-edit the file, both live in the web UI's cog menu under
**Preferences…**, phrased the positive way round: *Show the welcome screen on a new
install* and *Show what's new after an update*.
