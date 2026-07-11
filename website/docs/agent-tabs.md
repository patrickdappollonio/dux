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
coming up**, when no other tab running the *same* provider is already live or
launching. A tab that starts alongside a live same-provider sibling always starts
fresh; a different-provider sibling doesn't block it. In practice that means: reopen
an agent that was fully stopped and each provider picks up where it left off (a
Claude tab and a Codex tab both resume, that's two resumes); add a *second* Claude
tab on top of a running one and it's a clean slate.

### "Resume" reopens the *newest* conversation, not a particular tab

This is the part that surprises people, so it's worth being blunt: dux doesn't track
which conversation belonged to which tab. When it hands a tab the resume slot, all it
does is pass the provider's own continue flag, and that flag always grabs the
**most-recent** conversation in the worktree.

Walk it through. You have a Claude tab mid-conversation. You close it, then open a new
Claude tab. The new tab comes up alone for Claude, so dux passes `--continue`, and
Claude reopens the latest conversation in that folder, which is the one you were just
in. It *looks* like dux resumed your closed tab. It didn't: it started a fresh tab that
happened to reattach to the newest conversation. The distinction matters the moment
you want an *older* one, because dux can't target it for you. Reach for it with the
provider's own "resume a past session" or history command from a fresh tab.

### Why not just track each tab's conversation?

This is a deliberate choice, not a missing feature. To resume a *specific* tab, dux
would need each provider's conversation id and a way to reopen that exact one, and
there are only two ways to get there:

- **Read a session id out of each agent.** There's no shared standard for this. Every
  CLI names and stores its conversation identity differently, if it exposes one at all,
  and the formats aren't normalized. Following every provider reliably across versions
  is a fragile game dux would rather not stake correctness on.
- **Hook into the agents.** The stable path is a *hook*: dux watching or intercepting
  what the agent does to capture that handle. But a hook fires on the agent's own
  actions and, by its nature, reaches into the agent's data.

dux's stance is to **not hijack an agent's data unless it's absolutely necessary**, so
it takes neither path today. "Resume the newest conversation, no per-tab tracking" is
the honest consequence of that choice. It's also a tradeoff, not a dead end. Hook-based
per-tab resume isn't especially hard to build, and if the community decides the tighter
integration is worth the extra intrusion, the door is open.

## Switching between tabs

On the web, click a pill, and the tab is deep-linkable, so the URL points right at
it and a reload brings you back to the same one.

dux also remembers, per agent, whichever tab you had focused last. Jump to a
different agent and back — on the web that's a sidebar click or the plain
`#/agent/<id>` link, in the terminal UI it's just reselecting the agent — and
you land back on that same tab, not always the first one. This persists across
restarts too, and it's shared between the terminal UI and the web (they read the
same store), so switching a tab in one surface is remembered by the other. A
tab you'd closed since is never resurrected by this; you just land on the
agent's default tab instead. An explicit deep link always wins over the
remembered tab, so a link someone hands you always opens exactly what it says.

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

### Closing a tab is one-way

Closing a tab throws away the tab itself: its slot in the strip, its position, and
whatever provider you'd retargeted it to. There's no undo; to get a tab back you make
a new one. What's *not* thrown away is the conversation. dux never stored it in the
first place (see [how resume works](#how-resume-works) above), so it's still sitting in
your provider's own per-directory history. So "dux can't reopen this exact tab" is
true, and "your conversation is gone" is not. Start a fresh tab and use the provider's
history command to dig the previous one back up.

## What happens on restart

When dux restarts, tabs come back **dormant**: the pills are there, no processes
are running, and each tab shows a "start fresh" prompt instead of an old session.
Press it (or click *Start session* on the web) and a brand-new session spins
up. A tab that comes up alone for its provider may resume automatically, per the rule
above, which again means the newest conversation in the worktree, not a specific tab's.

To pick a *particular* past conversation, start a fresh session and use the provider's
own "previous conversations" command.
