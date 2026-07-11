---
title: Agents from the browser
description: Create, fork, adopt, and manage agents in server mode, plus provider tabs, dormant tabs after a restart, attention indicators and browser notifications, and the difference between killing and deleting an agent.
group: Server mode
order: 64
---

Server mode is not a read-only dashboard. You can run the whole agent lifecycle
from the browser: spin one up, fork it, adopt an orphaned worktree, retarget its
provider, and tear it down, all with the same worktree-per-agent model the TUI
uses. If the core ideas of projects, agents, and providers are new to you, start
with [Creating agents](/docs/creating-agents), then come back for the browser
specifics.

## Creating an agent

The **New agent** dialog (from a project's `⋯` menu) offers three ways in:

- **New** creates a fresh git worktree and branch and launches the agent. The
  branch name is optional; leave it blank and dux generates a memorable pet name.
- **Fork** copies the current files of an existing agent into a new worktree and
  branch, so you can take a running piece of work in a second direction without
  disturbing the first.
- **From PR** fetches a pull request's head branch into a new worktree. Give it a
  PR URL, `#123`, or just `123`. This one appears only when GitHub integration and
  the `gh` CLI are available.

Whichever you pick, creating the agent launches it immediately, no separate start
step. The name input sanitizes itself to valid branch characters as you type.

You can also **adopt an existing worktree**: "New agent from existing worktree…"
lists dux-managed worktrees that have no agent attached and turns one back into a
live agent on its existing branch. Handy after a restart, or for reclaiming work a
deleted agent left on disk.

## Managing an agent

An agent's `⋯` menu is where the rest lives: rename it (a display title, the
branch keeps its own name), change its provider (effective the next time it
launches, never yanking a running session out from under itself), view its info,
inspect its project's environment and startup command, and read startup-command
logs. **Change provider** and **Force recreate** are the two knobs around resume:
changing the provider resumes that provider's prior conversation on the worktree
when one exists, and Force recreate is the explicit "start clean, abandon the
conversation" button.

## Provider tabs

An agent can run more than one provider session in its single shared worktree, as
**tabs**. When an agent has two or more, a strip of Chrome-style pills appears
atop the terminal, each a live provider session against the same files. Add one
with the split **+** button (the main button uses the project's default provider,
the caret lets you pick), switch with a click, retarget or close from a pill's
`⋯` menu. Every tab is generic and uniform, there is no privileged "main" tab.

Resume is automatic and decided per provider, never a toggle you flip. A launching
tab resumes its provider's prior conversation when it is the only live tab of that
provider, and starts fresh otherwise, so a Claude tab and an OpenCode tab can both
resume side by side while two Claude tabs would not collide. The full model,
including how closing a tab works and the per-agent tab cap, is in
[Agent tabs](/docs/agent-tabs).

### Dormant tabs after a restart

When the server restarts, your tabs come back **dormant**, not running. dux does
not restore a tab's conversation across a restart, so instead of silently
relaunching (and instead of quietly re-attaching, which would force a launch), a
dormant tab shows a card explaining it is not running with a **Start session**
button. Nothing launches until you ask it to. Your provider CLI likely still has
the conversation in its own history, so a started tab often picks up where it left
off, and every provider offers its own command to browse and choose a past
conversation if you want a specific one.

## Attention and notifications

You do not have to babysit every tab. When an agent needs you (a permission
prompt, a finished turn), its sidebar icon and tab pill light up amber, the
browser tab title gains a count like `(2) dux`, and the favicon grows a small
amber dot. The flag clears the moment you actually look at that agent. The whole
model, and how to make sure your agents actually emit the signal, is covered in
[Attention indicators](/docs/attention-indicators).

Server mode can go one step further and raise a **real browser desktop
notification** when a backgrounded agent asks for you, bridged from the agent's
own notification escape codes. It is strictly opt-in: dux never auto-prompts for
permission. Run **Enable browser notifications** from the command palette once,
grant permission, and you are set. It fires only while the tab is in the
background, so an agent you are watching never nags you. This is governed by the
`web_notifications` capability, detailed in
[Terminal capabilities](/docs/terminal-capabilities).

## Kill versus delete

Two very different endings, and the difference matters:

- **Kill (detach)** stops the running session but keeps the agent and its
  worktree. It moves to a detached, reopenable state, and you can reconnect to it
  later. The palette's **kill-running** command opens a modal listing every active
  agent and companion terminal so you can stop them one by one. Companion
  terminals, unlike agents, are **destroyed** when killed, not detached.
- **Delete** removes the agent from dux entirely. The confirmation includes an
  unchecked "Also delete the git worktree on disk (irreversible)" box, so by
  default your worktree and its work survive even a delete. This is the one
  destructive per-agent action dux tints red, and it always confirms first.

Worktrees are your data, so dux never removes or mutates them casually. Deleting
an agent leaves its worktree on disk unless you explicitly opt in, which is
exactly why "adopt an existing worktree" above can bring one back.
