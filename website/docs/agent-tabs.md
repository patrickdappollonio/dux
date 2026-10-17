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
one tab, and the strip stays hidden until you add a second. Turn on **Always show tab
strip** (a Preferences row on the web, `toggle-always-show-tabs` in the terminal UI's
palette, or `always_show_tab_strip` under `[ui]`) if you would rather see it from the first
tab on.

> [!NOTE]
> The browser's [theater mode](/docs/web-workspace#theater-mode-one-pane-no-chrome) takes
> the strip away along with the rest of the chrome. The tabs keep running, and leaving
> theater brings the strip back where you left it.
>
> While you are in it, a tab that needs you shows up like this:
>
> - **Agents list and the tab's pill:** both come back when you leave theater.
> - **Desktop notification:** only if you turned them on and the dux window is in the
>   background.
> - **On a computer:** the browser tab's title and favicon still count it.
> - **On a phone:** nothing says so until you leave the mode.

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
> A per-agent cap keeps the strip sane: 20 tabs by default. Change it with
> `agent_tabs_max` under `[ui]` (`0` means the default, and values above 100 are held to
> 100). Adding a tab past the cap is refused.

## How resume works

A provider's "continue where we left off" flag is tied to the *directory* **and to that
provider**, not to a specific conversation. Claude, Codex (on a recent enough build) and
OpenCode each keep their own history scoped to the worktree, so a Claude tab and a Codex
tab can both resume at once. Two tabs of the *same* provider would both reopen the
identical conversation.

So dux hands the resume slot to a tab **only when it is the sole tab of its provider
coming up**, meaning no other tab running that provider is already live or launching.

- **A different-provider sibling** never blocks it.
- **Tab position** has nothing to do with it.
- **Reopening a fully stopped agent:** each provider picks up where it left off.
- **A second Claude tab** launched on top of a running one starts clean.

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

Walk it through:

1. You have a Claude tab mid-conversation, and you quit dux.
2. Every tab comes back dormant, and you launch that Claude tab.
3. It comes up alone for Claude, so dux passes `--continue`.
4. Claude reopens the latest conversation in that folder, which is the one you were just in.

It *looks* like dux resumed that exact tab. It did not: a different Claude tab, launched
first, would have taken the very same conversation.

> [!TIP]
> dux cannot target an **older** conversation for you. Start a fresh tab and use the
> provider's own "resume a past session" or history command.

## Switching between tabs

On the web, click a pill. Tabs are deep-linkable, so the URL points right at one and a
reload brings you back to it.

dux also remembers, per agent, whichever tab you had focused last. Jump to a different
agent and back (on the web, a sidebar click or the plain `#/agent/<id>` link; in the
terminal UI, reselecting the agent) and you land on that same tab.

- **The memory survives restarts** and is shared between the terminal UI and the web.
- **A closed tab is never resurrected.** You land on the agent's first tab instead, and so
  does a deep link to a tab that has since been closed.

> [!NOTE]
> An explicit deep link always wins over the remembered tab, and following one does not
> overwrite what dux remembers. A link a coworker sends you never changes which tab you
> land on next time you reopen the agent yourself.

In the terminal UI, switching is keyboard-driven and every key is yours to rebind; the
in-app help overlay shows your current bindings.

- **Chords:** the defaults are modifier chords, and chords stay dux's even while you type
  into the agent in the windowed pane, so you can hop tabs mid-sentence.
- **Numbers:** each pill carries its position number, so the switch-by-number keys have a
  visible address.
- **Activity:** a pill reports what its own tab is up to, with the same cues the agent
  list uses: a spinner while that tab's provider is working, and a blinking dot when it
  wants you.
- **Highlight:** the pill you are looking at is the highlighted one.

This is the terminal UI strip: the third pill is highlighted because it is the tab on
screen.

![The terminal UI tab strip above an agent's terminal, three numbered pills for the same provider with the third one highlighted.](/screens/tui-tabs-strip-ordinals.png)

> [!IMPORTANT]
> Fullscreen gives every key to the agent verbatim and does not draw the strip at all.
> Minimize first to hop between tabs.

Switch-to-tab-4 ships with no default key, because most terminals cannot tell `Ctrl-4`
from `Ctrl-\`. Step to it with the next and previous tab keys, or bind your own key to
`select_tab_4`, which the config file carries as a commented-out row ready to fill in.

## Closing tabs

Every tab closes, the **first** one included, as long as the agent has another tab to
fall back on. Closing one ends that session, so dux asks you to confirm. Closing the
agent's **last running** tab detaches the whole agent: it leaves the sidebar's active list
but stays in Projects, ready to reopen.

![A running tab's menu open over the tab strip, offering Change provider, Stop tab and Close tab.](/screens/agent-tab-actions-menu.png)

A separate **Detach agent** action stops every one of the agent's tabs at once and parks
it in Projects. Deleting the agent takes every tab with it. See
[Detaching an agent](/docs/web-agents#detaching-an-agent) for what detaching does and how
long it waits.

### Closing the first tab hands its place on

Closing an agent's first tab promotes the **next tab** to first, and the confirmation
names it before you commit. The promoted tab is untouched otherwise: same conversation,
same process if it was running, same link. It simply becomes the tab the agent opens on.

> [!IMPORTANT]
> An agent always has a first tab, so its **only** tab cannot be closed. dux says so
> rather than closing it, in the same words on both surfaces: on the web the *Close tab*
> entry is greyed out with the reason above it, and in the terminal UI the close key
> answers with that note instead of a confirmation.
>
> Add another tab first, or detach the agent to stop everything it is running (the Task
> Manager's **Force stop** on the agent's row does the same, without the shutdown wait).

### Stopping a tab keeps it

To end a tab's session without throwing the tab away, stop it instead. dux asks first,
then ends the tab's process; the pill stays in the strip, dormant, ready to start again.

- **On the web:** **Stop tab** in a running tab's menu.
- **In the terminal UI:** the `stop-agent-tab` palette command, which acts on the focused
  tab.
- **From the command line:** `dux agents tabs stop <agent> <tab>`.

Stopping the agent's **last running** tab detaches the agent, the same as closing it
would. Any tab can be stopped, an agent's only tab included. A tab that is not running has
nothing to stop, so the web menu leaves the entry out for it.

### Closing a tab is one-way

> [!CAUTION]
> Closing a tab is one-way. It throws away the tab's slot, its position, and whatever
> provider you retargeted it to, with no undo. Your **conversation** is not thrown away:
> dux never stored it (see [how resume works](#how-resume-works)), so it is still in
> your provider's own per-directory history. Start a fresh tab and use the provider's
> history command to dig it back up.

## What happens on restart

Tabs come back **dormant**: the pills are there and no processes are running. A tab that
comes up alone for its provider resumes automatically, per the rule above, which again
means the newest conversation in the worktree rather than a specific tab's.

### Normal restart

- **On the web,** selecting the agent starts its **first tab** right there: that is the
  tab the agent is, and asking for the agent is asking for it. Extra tabs wait: each says
  it isn't running until you click *Start session*, so a tab you added deliberately does
  not spring back to life just because you looked at it.
- **If the agent's only tab stops while you are looking at it** (you quit the provider, or
  stopped the tab), the web stays on the agent rather than sending you home, and the tab
  shows the idle screen described below. It does not start again until you click *Start
  session* or click the agent's row again.
- **In the terminal UI,** nothing starts just because you looked at it, the first tab
  included. A dormant extra tab shows a *Tab not running* card naming the key that
  launches it, and a detached agent comes back through the reconnect action.

### When the last run ended badly

There is one exception, and it is the useful one. If a tab's **last run ended badly**, that
tab waits for you too, first tab included. It says it isn't running and only *Start
session* launches it. A run ended badly when:

- **it failed to launch,**
- **the provider exited with an error,** or
- **it was over in under five seconds,** whatever status it exited with, because a provider
  that quits that fast usually never came up.

The five-second rule only applies when nobody used the run. Anything you send it counts: a
keystroke, a paste, a dropped file, a click or scroll in the pane, or a macro. So opening
an agent and quitting it straight away, even with a macro that types `/exit` for you, is
you ending the run rather than the run failing, and the tab goes on starting when you
select it.

Without this gate, a tab that cannot come up would try again every single time you
selected the agent, and there would be no way to look at the agent without restarting the
thing that keeps failing. Once a run succeeds, or you stop the tab yourself, or you restart
dux, the tab is back to starting on selection.

### What a dormant tab shows

It is the same on the terminal UI and the web UI, and it is one of two things:

- **A tab that is simply not running** shows the dux idle screen, logo and rotating tip and
  all, with a *Start session* button under it.
- **A tab whose last run ended badly** shows a card instead: how the run ended and how
  long ago (a failed launch and the error it gave, an exit status, or a run that was over
  in under five seconds with nothing sent to it), and, under *Last output*, the last few
  lines the run had on screen.

That last output is usually where the answer is: a provider refusing to resume a
conversation that is already open somewhere else says so on its way out.

### When a resume is refused

You do not have to open the agent to see why a resume failed. When a resume was over in
seconds, nobody had a chance to send it anything, and it left something readable behind,
however short, dux raises a notice. It says the agent could not resume its previous
session and quotes the provider's own last words, remedy and all: a toast in the browser,
and in the terminal UI the status line when that agent is the one on screen.

The card still has the full output.

What happens to the tab depends on what the run left on screen:

- **Something readable** (even a single line like *No conversation found to continue*):
  dux keeps the tab exactly as it ended, repeats those words, and waits for you to start a
  fresh session yourself. It makes no difference whether the CLI treated that as an error:
  plenty of them print their refusal and exit perfectly happily, and you get the message
  either way.
- **Nothing readable at all,** a blank screen or only the control codes a CLI writes on
  its way in and out: there is nothing to quote, so dux treats it as nothing to resume,
  starts a fresh session on its own, and tells you it did.
