---
title: Agent tabs
description: Run several provider sessions inside a single agent, all sharing one worktree, with resume handled automatically per provider for whichever tab is on its own.
group: Guides
order: 11
---

Sometimes one conversation isn't enough. You want Claude thinking about the
refactor in one place and Codex sanity-checking a migration in another, both
editing the *same* checkout. That's what agent tabs are for: a single agent can
hold several provider sessions side by side, all pointed at its one git worktree.

Tabs live in a strip at the top of the agent's terminal. A brand-new agent has
exactly one tab and looks no different from before; the strip only shows up once
you add a second.

## Every tab is equal

There's no "main" tab and no "support" tab. Every tab is just a provider session
in the agent's shared worktree, and they all look and behave the same. Add one,
close one, switch between them freely.

Add a tab with the `+` button on the strip (or the new-tab command). It spawns the
project's default provider immediately. Want a different provider on that tab?
Retarget it from its `⋯` menu on the web, or with the change-provider action in the
TUI. There's a per-agent cap (20 by default, and fully configurable) so the strip
stays sane; the `+` button politely bows out once you hit it.

## How resume works

Here's the one bit of physics worth knowing. A provider's "continue where we left
off" flag is tied to the *directory* **and to that provider**, not to a specific
conversation. Claude, Codex, and OpenCode (with a sufficiently recent Codex build)
each keep their own history scoped to the worktree, so a Claude tab and a Codex tab
can both resume at once without stepping on each other. But two tabs of the *same*
provider would both reopen the exact same conversation and show you identical
output. Copilot's `--continue` isn't directory-scoped the way the others are, so
resuming it could reattach to a conversation from a completely different
directory. dux plays it safe and excludes Copilot from resume entirely, so a
Copilot tab always starts fresh, no matter what else is running.

So dux hands the resume slot to a tab **only when it's the sole tab of its provider
coming up** — when no other tab running the *same* provider is already live or
launching. A tab that starts alongside a live same-provider sibling always starts
fresh; a different-provider sibling doesn't block it. In practice that means: reopen
an agent that was fully stopped and each provider picks up where it left off (a
Claude tab and a Codex tab both resume, that's two resumes); add a *second* Claude
tab on top of a running one and it's a clean slate.

## Switching between tabs

On the web, click a pill, and the tab is deep-linkable, so the URL points right at
it and a reload brings you back to the same one.

In the terminal UI, switching is keyboard-driven and the keys are yours to rebind
(the `?` overlay always shows your current bindings). One deliberate quirk: tab
switching is a non-interactive-mode action. While you're actively typing into an
agent in fullscreen, those keys belong to the agent, not to dux, so drop out of
the agent first to hop between tabs.

## Closing tabs

Closing a tab ends that session, so dux asks you to confirm. If it's the agent's
**last** tab, closing it detaches the whole agent: it leaves the sidebar's active
list but stays in Projects, ready to reopen. Nothing is lost.

There's also a separate **Detach agent** action that stops every one of the
agent's tabs at once and parks it in Projects, reopenable. And deleting the whole
agent, of course, takes every tab with it.

## What happens on restart

When dux restarts, tabs come back **dormant**: the pills are there, no processes
are running, and each tab shows a "start fresh" prompt instead of an old session.
Press it (or click *Start fresh session* on the web) and a brand-new session spins
up. (Each tab that comes up alone for its provider may resume automatically, per
the rule above.)

Why not just resume every tab? Same reason as above, dux can't. But the
conversation you had usually isn't gone: most provider CLIs keep their own
per-directory history. Start a fresh session in the tab and use the provider's own
"previous conversations" command to dig the old one back up.
