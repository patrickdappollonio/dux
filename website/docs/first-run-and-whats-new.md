---
title: First run & what's new
description: The one-time welcome screen a fresh install gets, the what's-new screen after an upgrade, where those release notes come from, how to reopen either screen on demand, and the two settings that turn the automatic ones off.
group: Getting started
order: 3
---

dux shows you a screen exactly twice in its life: once when it is brand new to your
machine, and once after each update. Both are read-once, both are dismissible with
a button, and both can be summoned again later if you closed one too fast.

## The welcome screen

The very first launch of a fresh install opens a welcome screen instead of dropping
you into an empty sidebar. It is a short orientation, not a tour:

- **What a project is:** any git repo on this machine. Your checkout is left alone.
- **What an agent is:** a session on a project that gets **its own git worktree and
  a branch-style name**, so several agents work in parallel without tripping over
  each other's files.
- **What a provider is:** whatever AI CLI you point it at. No protocol layer, no
  adapter to write. If a tool runs in a terminal, it can be a provider.
- **Where your config file lives**, spelled out as the real path on *this* machine,
  along with the fact that it was written fully commented. That file is the
  documentation.

Underneath, the same three numbered steps that the prose describes: add a project,
create an agent, launch. Two buttons close it out: one opens the project picker so
you can add your first project right there, and one opens
[getdux.app](https://getdux.app).

Because the config path is resolved at runtime rather than hardcoded, the screen
shows `~/.config/dux/config.toml` on Linux and `~/.dux/config.toml` on macOS
without you having to know which is which.

## What's new after an update

When dux starts and the version you are running is not the version whose screen you
last saw, you get a **What's new** screen for the release you just moved to. It
renders that release's headline, its opening paragraphs, and the titles of its
feature sections, then gets out of the way. The GitHub changelog and the installer
boilerplate that follow in the release body are deliberately left out; a modal is
not the place for a commit list.

One button opens the full notes for that release on GitHub, the other closes.

The screen always describes **the version you actually have**. dux asks GitHub for
its own tag by name, not for "whatever is newest", so upgrading to a release while a
newer one is already published never shows you features you do not have yet.

> [!NOTE]
> A development build never shows the what's-new screen on its own. There is no
> published release to describe. It still gets the first-run welcome, since a fresh
> dev install is still a fresh install.

## Where the notes come from

At launch, dux fetches the matching release from GitHub's public API,
**unauthenticated** (no token, nothing to configure) and on a background worker, so
a slow network never stalls the UI. Only the one release being run is ever
requested. The result is cached on disk next to your config as
`release_notes.json`, so relaunching a dozen times in an afternoon does not mean a
dozen requests; deleting that file costs exactly one.

If the fetch cannot get through, dux shows nothing at all and says nothing about
it. What happens next depends on *why*:

- **Something that might work later** (offline, a timeout, a rate limit, a GitHub
  hiccup): the version is **not** recorded as seen, so the notes are still waiting
  for you on a later launch that has a network. Starting dux on a plane does not
  cost you a release's notes.
- **A definitive "no such release"** (a tag GitHub has no release page for, which is
  perfectly normal for a locally built or not-yet-published binary): the answer
  cannot change by asking again, so dux records the version and stops asking.

## Opening either screen on demand

Neither screen is a one-shot you can lose. Both are available whenever you want
them:

- **In the terminal app:** the command palette has `show-welcome-screen` and
  `show-release-notes`. Neither has a keybinding, because a read-once screen does
  not deserve a hotkey.
- **In the browser (server mode):** the cog menu has **Welcome screen…** and
  **What's new…**.

The record of which version you have seen is stored once and shared by both
surfaces, so dismissing the what's-new screen in the terminal also dismisses it for
the browser. There is no per-browser copy to get out of sync.

## Turning the automatic screens off

Both are opt-out, under `[ui]`, and both default to `false` (that is, both screens
are on):

```toml
[ui]
disable_automated_welcome_screen = false
disable_release_notes            = false
```

Read them precisely, because it is easy to assume more than they do. Each one
suppresses only the **automatic** appearance:

- `disable_automated_welcome_screen` stops the first-run screen from showing
  itself. Opening it deliberately still works.
- `disable_release_notes` stops the what's-new screen from showing itself **and**
  skips the launch-time fetch entirely, so nothing touches the network at startup.
  Opening the release notes yourself still works, and still fetches them: the
  setting controls the automatic screen, not what that screen is allowed to show.

Turning either one off still moves your seen-version marker forward rather than
pinning you at an old version. Switch `disable_release_notes` back on later and you
get the *next* update's notes, not a backlog of everything you skipped.

If you would rather not hand-edit the file, both live in the web UI's cog menu under
**Preferences…**, phrased the positive way round: *Show the welcome screen on a new
install* and *Show what's new after an update*.
