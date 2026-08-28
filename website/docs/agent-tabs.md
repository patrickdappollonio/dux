---
title: Agent tabs
description: Run several provider sessions inside a single agent, all sharing one worktree, with resume handled automatically per provider for whichever tab is on its own.
group: Guides
order: 11
---

A single agent can hold several provider sessions side by side, all pointed at its one
git worktree. Claude thinking about the refactor in one tab, Codex sanity-checking a
migration in another, both editing the *same* checkout.

Tabs live in a strip at the top of the agent's terminal. A brand-new agent has exactly
one tab, and the strip stays hidden until you add a second.

## Every tab is equal

There is no "main" tab and no "support" tab. Every tab is a provider session in the
agent's shared worktree, and they all behave the same. This is the strip in the browser:

![Three provider tabs on one agent, the first one active, with a plus button for adding another.](/screens/agent-tabs-strip.png)

- **On the web**, the `+` button on the strip adds a tab running the project's default
  provider immediately. The dropdown attached to it picks a different one.
- **In the terminal UI**, the `new-agent-tab` palette command opens a picker of your
  configured providers with the project default preselected. With only one provider
  configured it skips the picker.
- **To retarget a tab later**, use its `⋯` menu on the web, or the
  `change-agent-provider` palette command in the terminal UI.

> [!NOTE]
> A per-agent cap keeps the strip sane: 20 tabs by default, and configurable. Adding a
> tab past the cap is refused.

## How resume works

A provider's "continue where we left off" flag is tied to the *directory* **and to that
provider**, not to a specific conversation. Claude, Codex, and OpenCode (with a recent
enough Codex build) each keep their own history scoped to the worktree, so a Claude tab
and a Codex tab can both resume at once. Two tabs of the *same* provider would both
reopen the identical conversation.

So dux hands the resume slot to a tab **only when it is the sole tab of its provider
coming up**, meaning no other tab running that provider is already live or launching. A
different-provider sibling never blocks it, and tab position has nothing to do with it.
In practice: reopen an agent that was fully stopped and each provider picks up where it
left off, but launch a *second* Claude tab on top of a running one and it starts clean.

> [!NOTE]
> A tab you **create** always starts fresh, whatever else is running. The resume slot is
> for tabs coming back up: an agent you reopen, or a dormant tab you launch. Making a new
> tab never reaches for a previous conversation.

> [!IMPORTANT]
> Copilot never resumes. Its `--continue` is not directory-scoped the way the others
> are, so resuming could reattach to a conversation from a completely different
> directory. A Copilot tab always starts fresh.

### "Resume" reopens the *newest* conversation, not a particular tab

dux does not track which conversation belonged to which tab. When it hands a tab the
resume slot, it passes the provider's own continue flag, and that flag grabs the
**most-recent** conversation in the worktree.

Walk it through. You have a Claude tab mid-conversation, and you quit dux. Every tab
comes back dormant, and you launch that Claude tab. It comes up alone for Claude, so dux
passes `--continue`, and Claude reopens the latest conversation in that folder, which is
the one you were just in. It *looks* like dux resumed that exact tab. It did not: a
different Claude tab, launched first, would have taken the very same conversation.

> [!TIP]
> dux cannot target an **older** conversation for you. Start a fresh tab and use the
> provider's own "resume a past session" or history command.

## Switching between tabs

On the web, click a pill. Tabs are deep-linkable, so the URL points right at one and a
reload brings you back to it.

dux remembers, per agent, whichever tab you had focused last. Jump to a different agent
and back (on the web, a sidebar click or the plain `#/agent/<id>` link; in the terminal
UI, reselecting the agent) and you land on that same tab. The memory survives restarts and is shared
between the terminal UI and the web. A tab you closed is never resurrected by it; you
land on the agent's default tab instead.

> [!NOTE]
> An explicit deep link always wins over the remembered tab, and following one does not
> overwrite what dux remembers. A link a coworker sends you never changes which tab you
> land on next time you reopen the agent yourself.

In the terminal UI, switching is keyboard-driven and every key is yours to rebind; the
in-app help overlay shows your current bindings. The defaults are modifier chords, and
chords stay dux's even while you type into the agent in the windowed pane, so you can
hop tabs mid-sentence. Each pill carries its position number, so the switch-by-number
keys have a visible address. A pill also reports what its own tab is up to, with the
same cues the agent list uses: a spinner while that tab's provider is working, and a
blinking dot when it wants you. The pill you are looking at is the highlighted one.

That is the terminal UI strip: each pill leads with its position number, and the first one
is highlighted because it is the tab on screen.

![The terminal UI tab strip above an agent's terminal, three numbered pills for the same provider with the first one highlighted.](/screens/tui-tabs-strip-ordinals.png)

> [!IMPORTANT]
> Fullscreen gives every key to the agent verbatim and does not draw the strip at all.
> Minimize first to hop between tabs.

Switch-to-tab-4 ships with no default key, because most terminals send the same byte for
`Ctrl-4` and `Ctrl-\` and dux gives that byte to the macro bar. Step to it with the
next and previous tab keys, or bind your own key to `select_tab_4`, which the config
file carries as a commented-out row ready to fill in.

## Closing tabs

Closing a tab ends that session, so dux asks you to confirm. Closing the agent's
**last** tab detaches the whole agent: it leaves the sidebar's active list but stays in
Projects, ready to reopen.

![A tab's menu open over the tab strip, offering Change provider and Close tab.](/screens/agent-tab-actions-menu.png)

A separate **Detach agent** action stops every one of the agent's tabs at once and parks
it in Projects. Deleting the agent takes every tab with it.

### Closing a tab is one-way

> [!CAUTION]
> Closing a tab is one-way. It throws away the tab's slot, its position, and whatever
> provider you retargeted it to, with no undo. Your **conversation** is not thrown away:
> dux never stored it (see [how resume works](#how-resume-works)), so it is still in
> your provider's own per-directory history. Start a fresh tab and use the provider's
> history command to dig it back up.

## What happens on restart

Tabs come back **dormant**: the pills are there, no processes are running, and each tab
says it isn't running instead of showing an old session. Press the key it names, or click
*Start session* on the web, and the tab launches. A tab that comes up alone for its
provider resumes automatically, per the rule above, which again means the newest
conversation in the worktree rather than a specific tab's.
