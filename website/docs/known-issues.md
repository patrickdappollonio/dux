---
title: Known issues
description: Things that look broken until you know why, from agent sign-ins and copies on another device to terminals after a restart, plain HTTP, and shared lockouts.
group: Reference
order: 90
---

These are the things that look broken unless you know why. Each one says what is going
on, what to do about it, and where the full story lives.

## Agent CLIs

### Signing in to an agent CLI from another device goes nowhere

An agent's sign-in tries to open a browser on the machine dux runs on, which is not the
one in front of you when you drive dux from a laptop or a phone. Every CLI also prints a
sign-in link, and what to do with it depends on the CLI:

- **Claude Code**: open the printed link on any device, sign in, and paste the code the
  page shows back into the CLI. This works from anywhere.
- **Codex**: after you sign in, the browser is sent back to an address on the dux machine
  itself (`127.0.0.1`, port `1455`), so finish the sign-in on that machine. From another
  computer, forward the port over SSH with `ssh -L 1455:127.0.0.1:1455 <host>` and open
  the link on your own computer. `codex login --device-auth` works too, if your
  organisation allows device-code sign-in.
- **OpenCode**: its browser sign-ins also return to the dux machine (ChatGPT accounts use
  port `1455`), so the same advice as Codex applies.
- **Copilot**: set a GitHub token in the environment as `COPILOT_GITHUB_TOKEN`, `GH_TOKEN`
  or `GITHUB_TOKEN`. It has to be a fine-grained personal access token with the "Copilot
  Requests" permission; classic `ghp_` tokens are not accepted. You can put it in dux's
  `[env]` table so every agent gets it, or use device-code sign-in if your organisation
  allows it. See
  [keeping secrets out of `[env]`](/docs/configuration#keeping-secrets-out-of-env) before
  you paste a token into the file.

### Copying inside an agent CLI lands on the wrong clipboard

A copy made inside a CLI (Claude Code's `/copy`, say) reaches your browser only when the
CLI sends it through the terminal. Measured on macOS: Claude Code, OpenCode and Copilot
send it through the terminal and also write the Mac's own clipboard, while Codex writes
only the Mac's clipboard, so its copies never reach a remote browser. On Linux, a CLI that
can reach a desktop session may write that desktop's clipboard instead.

Even a copy that does arrive has to be accepted by your browser, which only allows it
within about 5 seconds of your last key press or click. A later copy shows a message with
the start of the text and a **Copy** button that puts it on your clipboard. See
[the clipboard section](/docs/web-workspace#clipboard-the-classic-terminal-model) and
[what plain HTTP costs](/docs/tailscale#plain-http-costs-you-a-few-browser-features).

The way around all of it: select the text in the terminal pane yourself. That always
copies on your own device.

### A second tab of the same CLI starts with a blank conversation

A CLI's "continue" picks up the newest conversation in the folder, not a particular tab's,
so two tabs of the same CLI would both open the same one. dux gives the resume to a tab
only when it is the only tab of its CLI coming up, and any tab you create starts fresh. To
reach an older conversation, use the CLI's own history or resume command in a fresh tab.
See [how resume works](/docs/agent-tabs#how-resume-works).

### A Copilot tab never resumes its conversation

Copilot's own "continue" picks up the most recent conversation from anywhere on your
machine, not the one that belongs to this worktree, so dux never uses it. Every Copilot
tab starts fresh. See [Copilot](/docs/supported-agents#copilot).

### The working spinner drops to idle for a moment while you type or scroll

For most agents, "working" means the agent printed something in the last second. Your own
typing is echoed back, and scrolling makes the agent redraw, so dux ignores the agent's
output for about a second after each key or scroll rather than calling your own echo
work. Agents that report their own progress (Claude Code and Copilot) don't do this. The
spinner comes back on its own as soon as the agent prints again.

### "Needs you" sometimes misses, or fires when nothing is needed

There is no standard way for a CLI to say it is waiting on you, so dux listens for the
terminal bell and desktop-notification codes, and what each CLI sends depends on the CLI
and its settings. Text an agent prints can also carry those codes by accident. See
[when nothing lights up](/docs/attention-indicators#when-nothing-lights-up).

### A custom agent shows as a failed run when its launcher exits right away

A tab that ends within 5 seconds with no input is treated as a CLI that never came up, so
it waits for you with its last output instead of closing. Any input, a macro included,
exempts a run. A launcher that starts the real program and exits, or hands off to a
program in the background, looks exactly the same. Make the launcher stay in the
foreground, for example by ending it with `exec` and the real program, so the tab lives as
long as the program does. See
[what happens on restart](/docs/agent-tabs#what-happens-on-restart) for the card such a
tab shows, and [custom agents](/docs/custom-agents) for setting one up.

## Terminals

### Terminals don't come back after dux restarts

A terminal exists only while its shell runs, so it ends when dux does. Agents come back
after a restart; terminals don't. See
[companion terminals](/docs/web-workspace#companion-terminals).

### A full-screen program looks garbled after you reconnect

Letters where box-drawing lines were, odd tab spacing, or a misplaced cursor: a few rarely
used terminal settings can't be restored when you reconnect, and some full-screen programs
use them. It fixes itself the next time the program redraws, so resize the window or use
the program's own redraw key.

### Dragging doesn't select text in a program that uses the mouse

When the program in the terminal takes the mouse, a plain drag goes to the program. Hold
**Shift** (**Option** on macOS) while dragging to select locally. See
[the clipboard section](/docs/web-workspace#clipboard-the-classic-terminal-model).

## In the browser

### Some browser features are missing over plain `http://`

Browsers switch off a few features on a plain-HTTP address that is not `localhost`:
right-click paste (`Ctrl+v` still works), desktop notifications, and installing dux as
an app. Copies from an agent still work within about 5 seconds of your last key press or
click, and show a **Copy** button after that. Open dux on `localhost`, or put HTTPS in
front of it with `tailscale serve`, and all of them come back. See
[what plain HTTP costs](/docs/tailscale#plain-http-costs-you-a-few-browser-features).

## Editor

### Deleting a file in the editor is permanent

There is no trash on the server: a delete removes the file, or the folder and everything
in it, straight from disk. For the same reason, moving something onto a name that is
already taken is refused rather than overwriting it. See
[creating, renaming, moving and deleting files](/docs/web-editor#creating-renaming-moving-and-deleting-files).

## Git

### Switching branches or pulling can delete ignored files

git treats ignored files as disposable, so when the incoming branch tracks a path where an
ignored file or folder stands, git replaces it. dux refuses when that path holds something
it knows about, such as an agent's worktree, and lets git have its way otherwise. Move
anything that matters aside first. See
[changing the base branch](/docs/creating-agents#changing-the-base-branch).

### Submodules are not updated by dux's pulls and switches

dux's pulls and branch switches move the submodule pointer but leave the submodule's files
where they are, even when your git config sets `submodule.recurse`. Run
`git submodule update` yourself when you want them to follow. See
[changing the base branch](/docs/creating-agents#changing-the-base-branch).

## Server

### A wrong-password lockout hits everyone behind the same address

Failed sign-ins are counted per address, and an address can be many people: a shared
office connection, or a proxy that hides the real addresses so every visitor looks the
same. Wrong guesses by anyone there slow down everyone there, and with the default
settings five of them block everyone there. See
[failed sign-ins and blocked addresses](/docs/web-login#failed-sign-ins-and-blocked-addresses)
and, behind a proxy,
[one client IP for everybody](/docs/public-hosting#the-console-shows-one-client-ip-for-everybody).

### Only one dux can run per config folder

A second dux started against the same config folder stops at once with an "already
running" message. One dux can keep the terminal UI and serve the browser at the same
time, so you rarely need a second. See
[running both at once](/docs/faq#can-i-run-the-terminal-ui-and-the-browser-at-the-same-time).
