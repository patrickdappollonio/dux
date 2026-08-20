---
title: The workspace in the browser
description: The three-pane web layout, deep links, the browser terminals, the one-writer take-over model, copy-on-select, right-click paste and image paste, press-and-hold selection on a touch screen, Shift-Enter soft newlines, self-clearing messages, companion terminals, and the mobile hub-and-spoke shell.
group: Web UI
order: 61
---

Open the server URL and you land in the workspace: your projects, your agents, and
their live terminals, all click-driven. This is one of dux's two front ends, and it
is a complete one. If you have never touched dux, this is a fine place to start,
because the browser puts everything a click away. If you know the terminal UI, the
shape will be familiar, because both front ends lay the workspace out the same way.

## The layout

On a desktop-width screen it is three panes:

- A collapsible **left sidebar** lists your agents in a single flat list, no
  longer grouped by project, with the dormant ones tucked into a collapsible
  Inactive tail. A sort control orders the list (Active first by default, or by
  recently updated, recently created, name, or a manual drag order), and a
  search box filters it. Drag rows to arrange them by hand, and use the launcher
  at the bottom to create work: one button for the next agent, and one `⋯`
  beside it holding every other way in, grouped into agents, terminals and
  projects. On an empty dux the button reads **Add project** instead, since that
  is the only useful next step. The Agents header carries a **+** of its own, and
  the Terminals divider one that opens a standalone shell. Toggle the sidebar
  with `Ctrl-b`.
- The **center pane** is the focused agent's live terminal, or a welcome screen
  when nothing is selected.
- The **right Changes pane** shows what the focused agent has changed. You can
  resize the split, or hide the Changes pane entirely when you want the terminal
  full width; dragging the split shut and letting go is just another way of
  hiding it, and the header grows a button to bring it back. See
  [Git without leaving the browser](/docs/web-git).

A slim header up top shows breadcrumbs (agent, provider, project, branch) and a
**cog** button that opens the app menu. The cog menu is the browser's map: it holds
your preferences, the configuration dialogs, and the actions that apply to the whole
workspace rather than to one thing. Anything that acts on a *specific* agent,
project, or file lives in the `⋯` menu on that row instead, right next to the thing
it acts on. Between those two, everything the web UI can do is discoverable by
pointing at it, which is the browser's idiom for what the terminal UI reaches
through its command palette and `?` overlay. The cog menu deliberately has no
keyboard shortcut. Tab reaches it, Enter opens it, the arrow keys move through it,
and Escape closes it.

The web UI is dark-only. Themes are a terminal UI feature: the browser ships one
tuned dark palette, and it does not follow your system or browser light/dark
preference.

## Deep links

The URL tracks what you are looking at, so you can bookmark a session or paste a
link to a teammate on the same instance and they land exactly where you are:

- An agent is `#/agent/<sessionId>`.
- An extra provider tab is `#/agent/<sessionId>/tab/<tabId>`.
- A companion terminal is `#/agent/<sessionId>/terminal/<terminalId>`.
- A project terminal is `#/project/<projectId>/terminal/<terminalId>`.
- A standalone terminal is `#/terminal/<terminalId>`, with no owner in the
  address, because it does not have one.
- The phone's Changes screen is the same link with `/changes` on the end.

The URL is the whole story of where you are, which is what makes the browser's
back button behave: every move to a different screen adds one entry, so Back
always returns you to the previous screen rather than doing nothing. Moves that
stay on the same screen do not stack up: switching between agents or tabs,
reconnecting after the wifi drops, and following a link back to where you already
were all rewrite the one entry instead of adding another.

Back is only ever your own Back: dux never presses it for you. The phone's back
chevron is an **up** control, so it takes you one level up (Changes to its agent,
an agent to the home screen) and can never step out of dux onto whatever page you
were on before it, which is exactly what a deep link opened in a fresh tab used
to do. Going up is a move like any other, so it adds its own entry and Back
returns you to the screen you just left.

Follow a link to an agent that has since been deleted and you get a plain
**Agent not found** screen rather than a silent bounce to the home screen. It
gives way on its own if that agent turns up in the workspace again. Its way out
is the one move that does not add an entry, since a bad address is not a place
worth keeping: leaving it corrects the URL rather than stacking on top of it, so
Back cannot drop you straight back onto the dead end. If the agent
you are watching is deleted out from under you, dux moves you to the next active
agent, or home when there is not one. A link to a terminal that has since closed
is not an error, since terminals come and go: it lands you on the agent that
owned it, or on the home screen for a terminal that belonged to a project, or to
nothing at all, and tidies the address bar to match.

## Adding projects

**Add project…**, in the launcher's `⋯` menu at the bottom of the sidebar (and
the launcher button itself while you have no projects yet), opens a folder picker
that browses the server's filesystem. Pick a git repository and it joins the workspace. But here
is the trick worth knowing when you are on your phone with no terminal in
reach: the folder does **not** have to be a repository yet.

- Point it at a plain folder (via the pinned **Use this folder** row at the top
  of the list) and dux offers to **initialize a repository** right there: it
  runs `git init`, seeds a commented starter `.gitignore` for dependency and
  build folders it actually finds (`node_modules`, `target`, and friends),
  makes an empty initial commit, and adds the project. Your existing files are
  left exactly as they were, untracked and untouched.
- Need a fresh place to start? The picker's **New folder** button creates a
  directory right from the browser, so a brand-new project can go from nothing
  to "agent working in it" without ever opening a shell.
- Pick a folder that lives *inside* an existing repository and dux politely
  refuses, pointing you at the repository root instead, so you never end up
  with a project nested in another project's history.

The launcher's `⋯` holds both flavors under **Projects** ("Add project…" and
"Initialize a repository…"); either way the picker inspects your selection and
offers the right action. The TUI's project browser makes the same offer when
you point it at a plain folder.

## The browser terminals

Each agent runs its real CLI in a real PTY on the server, and the browser streams
it live through a WebSocket. Closing the tab, losing connectivity, or your phone
falling asleep does not stop the agent: it keeps running on the server exactly as
it was until you explicitly kill or delete it (see
[Agents from the browser](/docs/web-agents)). The moment you open a terminal, dux
subscribes you to that PTY and, if the provider is not already running,
**launches or resumes it**. Opening the view is what starts the agent. The full
scrollback replays on connect, so you never open into a blank screen mid-session.

If the connection blips, dux shows a quiet "Reconnecting…" overlay and keeps your
buffer. Only after several failed attempts does it fall back to a blocking
"Connection lost" card with a Reconnect button.

### One writer, many watchers

Every device pointed at the same terminal sees the same output at the same time,
but **only one device at a time can type.** That device is the owner, and
**you join as a watcher unless nobody is driving.** Open a phone on an agent your
desktop is already typing into and the phone watches; nothing you do by merely
arriving takes the keyboard away. If nobody holds it, the first device to look at
it picks it up.

**A watcher sees the take-over card.** A watched terminal is covered by a
full-pane card naming who has the keyboard ("Open on Chrome on macOS") with a
**Take over** button. The card is deliberate, and it is telling you something
specific: a device with a different screen size than yours is driving, and a
terminal only has one size at a time. Hit **Take over** and dux resizes the
PTY to your screen, repaints it fresh, and hands you the keyboard. Nothing is
lost, the other device simply becomes the watcher until it takes over in turn.
It is a polite hand-off, not a fight, and it happens only when somebody asks
for it: the device you took over from stays a watcher even when you go back to
it, until you press its own Take over. One tap is the whole cost of switching
devices, and in exchange nothing ever quietly grabs the keyboard mid-sentence.

Taking over always reattaches: the terminal reconnects, the server repaints
the current screen at your size, and the keyboard arrives with it. That takes
a moment (you will see "Reconnecting…") and it is deliberate, because a fresh
attach is what guarantees you start from a clean picture rather than someone
else's leftovers. Behind the card, dux keeps a watcher's copy of the terminal
at the driver's true size, shrinking the text until it fits, so nothing
mangled ever piles up in the scrollback and the screen you take over is clean
from the first frame.

If the driving device disconnects, everyone watching is told: every card
switches to **Nobody is driving** rather than naming a browser that has gone.
Nobody picks the terminal up for you, though. **Losing the keyboard is sticky.**
Sitting in front of an open take-over card is not a gesture, so a watcher left
on screen never quietly grabs a terminal somebody's flaky wifi dropped. Press
**Take over**, reload, or walk away and come back, and it is yours. The one
device that does get its terminal back on its own is the one that lost it to a
blip: when a dropped connection returns and finds the server still holding its
own dead session, it succeeds itself and carries on typing.
And if the watcher's own connection has given up entirely, the card
steps aside for the **Connection lost** notice and its **Reconnect** button,
because a Take over button over a socket that is not there would only look like
it worked.

### Clipboard: the classic terminal model

The web terminal copies and pastes the way a real terminal does, no menu required:

- **Select to copy.** Highlight text and it lands on your clipboard immediately.
  A small "Copied to clipboard" toast confirms it. This is governed by the
  `ui.copy_on_select` preference (on by default), which you can flip in
  **Preferences** (the cog menu).
- **Right-click to paste** (with a mouse or pen). It reads your browser clipboard
  and sends it to the agent. On plain HTTP, where the browser blocks clipboard
  reads, dux nudges you toward `Ctrl+v` instead. On a touch screen the same
  press-and-hold gesture belongs to selection instead, which is the next section.
- A fixed set of chords works too, and it is not user-configurable: `Ctrl+Shift+c`,
  `Ctrl+Insert`, or `Cmd+c` to copy, and `Ctrl+v`, `Ctrl+Shift+v`, or `Cmd+v` to
  paste, with `Ctrl+c` staying SIGINT as it should.
- **Paste an image with the keyboard and dux uploads it.** When what you paste is
  a picture rather than text, dux saves it on the server and pastes its **path**
  into the prompt, the same journey a dropped file takes. Text paste is
  untouched, and `Ctrl+Shift+v` (`Cmd+Shift+v`) forces the text when the
  clipboard carries both. This is the keyboard chords only: right-click paste
  reads text from your clipboard and can never carry an image. See
  [Dropping and pasting files onto an agent](/docs/dropping-files).

There is deliberately no right-click context menu, because select-to-copy and
right-click-paste already cover both directions and a menu would only fight the
paste gesture.

One escape hatch worth knowing: when the app inside the terminal grabs the mouse
(think a full-screen TUI that does its own mouse handling), a plain drag goes to
that app instead of selecting text. Hold **Shift** (on Linux and Windows) or
**Option** (on macOS) while dragging to force a local selection you can copy. dux
pops a one-time hint the first time this bites you.

### Selecting text with a finger

On a phone or tablet, **press and hold** on the terminal. The word under your
finger highlights, and dragging from there extends the selection. Drag past the
top or bottom edge and the terminal keeps scrolling on its own for as long as you
hold there, so a selection can run well beyond what fits on screen. **Lift your
finger and it is copied**, under the same `ui.copy_on_select` preference as the
desktop. The highlight stays up so you can see what you got; the next tap clears
it.

A press is not a tap, so the keyboard stays down. That is deliberate: a keyboard
sliding up over the text you just highlighted would cover the thing you were
reading.

This is also the touch version of the Shift and Option escape hatch above. A long
press **always** selects locally, even when a full-screen agent has taken the
mouse, so text inside a running Claude Code or opencode session is still
selectable with a finger.

Two limits worth knowing. There are no drag handles, so a selection cannot be
adjusted once you lift; press again to redo it. And if the agent is writing fast
enough to push lines out of the scrollback while you are mid-drag, the selection
can slide onto the wrong text, which pressing again also fixes.

Your agent can also write your clipboard directly through an `OSC 52` escape
sequence, and that write lands on **your** browser's clipboard, not the server's,
governed by the `clipboard_passthrough` capability. That story lives in
[Terminal capabilities](/docs/terminal-capabilities).

### Drag a file in, or paste one

Drag a screenshot (or any file) from your desktop onto the terminal, or simply
paste an image from your clipboard, and dux
saves it on the server, then pastes its path into the prompt. Dropped on an
agent it goes to that agent's upload folder (`.dux/uploads` in its worktree),
invisible to git and deleted along with the agent; dropped on a terminal it goes
to the folder that terminal is actually in right now. Nothing is ever
overwritten, your filename is kept as you had it, and only the device holding
input can drop or paste. On a phone, where there is no drag gesture, pasting an
image puts its path into your compose draft. See
[Dropping and pasting files onto an agent](/docs/dropping-files).

### Shift-Enter for a soft newline

In the browser, **Shift-Enter inserts a newline instead of submitting.** Plain
Enter still submits. This is a web-only convenience (the TUI cannot tell the two
apart) that lets you compose a multi-line prompt without firing it off line by
line. It is careful never to fire mid-composition, so it will not mangle a CJK
input method's confirm keystroke.

## Companion terminals

An agent is not limited to its provider CLI. You can spawn **companion terminals**
alongside it, plain shells in the same worktree, for running tests, poking at git,
or tailing a log while the agent works. They live in their own collapsible
**Terminals** section in the sidebar, with the owning agent named on the row, and
the title tracks whatever is running in the foreground
("vim", "htop"). Unlike agents, which detach when you kill them, killing a
companion terminal destroys it.

Projects get the same treatment: a **project terminal** is a plain shell opened at
the project's repo root with no agent attached. It is the escape hatch when dux
won't do something for you remotely: open a shell at the repo and do it by hand,
even over Tailscale with no local terminal in sight. Spawn one from the project's
⋯ menu ("New project terminal"); it joins the same **Terminals** section with the
project named as its owner, shows
up in the Task Manager, and is destroyed on close exactly like any other terminal.
Removing the project closes its project terminals.

And then there is the third kind, which belongs to nothing at all. A **standalone
terminal** opens in your home directory with no agent and no project behind it,
so you can reach for one before you have added a single project. It is the plain
shell you would have opened on the machine anyway, except it is in the browser.
Open one from the **Terminals** group of the launcher's `⋯` menu, from the
**+** on the sidebar's Terminals divider, or from the same entry in the cog
menu's **New** submenu. Its row shows the
directory it opened in, shortened with `~`, where the other two show their owner,
so the sidebar search finds it by path.

Every terminal row's `⋯` menu also carries **Open editor here** and **Open
editor in new tab**, rooted at the directory that terminal opened in, so a
shell and a real editor over the same files are one click apart. A terminal
that belongs to an agent opens that agent's editor instead, since it is the
same worktree. See [The code editor](/docs/web-editor) for what a
terminal-rooted editor does and does not carry.

Nothing closes a standalone terminal for you. Removing a project closes that
project's terminals and deleting an agent closes that agent's; neither has
anything to do with this one. It ends when you close it, or when dux shuts
down.

## Messages

Progress and results arrive as small messages at the bottom of the screen, and
**they clear themselves.** How long a finished one stays depends on how much it
matters: a success or a note uses your `ui.status_clear_seconds` window (six
seconds by default), a warning stays twice that, and an error four times, so the
thing you most need to read is the thing that waits longest. Setting
`ui.status_clear_seconds = 0` turns auto-clearing off entirely. You can dismiss a
finished message early by swiping it away, or with its close button.

A message about work still in flight is a spinner, and it behaves differently on
purpose: no close button and no swipe, because what it is reporting has not
happened yet. It is replaced in place by its own result the moment there is one.
If that result never arrives, because the connection dropped mid-operation, the
spinner gives up after a minute rather than claiming forever that something is
still running.

A small number stay until you dismiss them, and the rule is narrow on purpose:
a message waits only when **you have to go and do something outside it**, or when
something may have been lost or left half-finished. A file that was saved but
never handed to the agent waits, because the message holds the only copy of where
it went. A worktree that could not be removed waits, because it is still on disk.
A failed pull does not wait, because nothing was lost and you can simply try
again.

If several arrive at once they stack, five deep on a desktop and three on a
phone, and the rest queue behind them. They stack here, in the workspace tab:
the standalone editor tab opts out of workspace-wide messages and shows only
what you do in it.

One thing worth knowing if you keep a tab open on a flaky connection: when the
browser reconnects, dux tells it about work that is **still running**, plus any
result from the last thirty seconds, so an outcome that landed while you were
offline still reaches you. Older results are not repeated. Before this, every
page load replayed every warning and error since the server started.

## Macros

A floating **Macros…** button drops prewritten prompt snippets into the focused
terminal without submitting them, so you review and press Enter yourself. Each
macro is scoped to agents, terminals, or both, and multi-line macros are inserted
so they do not submit line by line. Full details in
[Managing Macros](/docs/macros).

## On your phone

Below tablet width, the web UI becomes a **hub-and-spoke** shell built for one
thumb, not a squished desktop:

- The **home** screen is the hub: your projects and sessions with the same `⋯`
  menus as desktop, the same launcher pair along the bottom, and a **cog** button
  that opens the app menu as a bottom sheet (submenus drill down in place, with a
  back arrow). The launcher's `⋯` opens as a bottom sheet too, headings and
  all.
  Tap a session to jump straight into its terminal.
- The **terminal** screen is a full-screen terminal with a slim bar on top (Back,
  branch name, an optional PR chip, and a chip showing the changed-file count) and
  the tab strip when the agent has more than one tab.
- The **changes** screen is the full Changes pane.

An **accessory bar** sits above the soft keyboard with the keys a phone keyboard
lacks: Esc, Tab, a sticky Ctrl and Alt latch, arrow keys, PgUp/PgDn, and a
dedicated **⇧↵** key for the soft newline (since a phone keyboard cannot produce
the Shift-Enter chord). Touch targets are sized generously so you are not
fat-fingering the wrong control. The soft keyboard is handled by the browser
shrinking the layout, so the accessory bar sits flush on top of the keyboard with
no fiddly per-pane math, and there is no fullscreen mode to fight with.

Below the accessory bar sits the **compose bar**: a real text box where you type
the message and hit **Send** when it is ready. This is where your keyboard's
autocorrect, swipe typing, and voice input actually work, because typing straight
into a terminal gives a phone keyboard nothing to fix. Enter adds a newline
instead of submitting, so multi-line prompts read the way you wrote them; Send
delivers the whole message and presses Enter for you, and an empty Send is a
plain Enter for confirming menus and prompts. Tapping the terminal drops you into
the compose box (a refused send keeps your draft and tells you why). Because the
box holds the keyboard focus the whole time, the terminal's own caret stays a
solid block rather than hollowing out the way an unfocused terminal normally
would: the prompt you are writing to should not look asleep.

Whether the compose bar appears is the `ui.compose_bar` setting, a Preferences
row with three values. **Automatic** (the default) asks your browser whether you
point at the screen with a finger. That is a question about your *input*, not
about your screen size, which matters because rotating a tablet used to cross a
width threshold and swap your typing surface out from under you mid-session.
What it genuinely cannot see is a keyboard case: an Android tablet with one
attached and the same tablet without report the browser exactly the same
capabilities. So **Always** and **Never** are there for the device dux guesses
wrong on, and Never restores typing straight into the terminal. An older config
that still says `compose_bar = true` or `false` keeps working; `true` is read as
Automatic and `false` as Never.

Because that is a question about your input rather than your screen, **both bars
follow your pointer and not the layout**. A tablet in landscape is wide enough
for the full desktop workspace, and it is still being typed on with a finger, so
it gets the desktop panes *and* the accessory keys and compose box below the
terminal. A mouse on a narrow window gets neither. The two questions are
separate: how much room there is decides which layout you see, and what is doing
the typing decides which typing surface you get.

Since a keyboard case is exactly what dux cannot see, the accessory bar carries a
**typing-surface toggle** at the end of its key row, and the input `⋯` menu
below carries the same switch worded as a sentence. It says which state it is
in, **Box** while you are typing into the message box and **Direct** while your
keystrokes go straight to the terminal, and one tap swaps them. It sits in the accessory bar
because that is where your thumb already is, and it is in the menu as well
because the menu is the surface that is there in every state, hidden bars
included, so turning the box off never strands you without a way back. The toggle is not a setting: it is
remembered on that device (so a reload does not snap you back) and it changes
nothing in your config. It appears only under **Automatic**, since Always and
Never have already answered the question.

Terminal rows are precious on a phone, so the chrome around the terminal is
hideable. Every phone terminal screen's `⋯` menu (agent, project, and standalone
terminals alike) has **Hide top bar** (which on the agent screen also takes
the tab strip with it) and **Hide terminal keys**, each backed by its own
preference, and the reclaimed rows go straight back to the terminal. Both
toggles live in Preferences too.

The way back is a `⋯` **input menu** that sits at the left edge of whatever
input row you have: beside the message box, or in the key row when the box is
off, or on its own slim row when you have hidden both. It is there whether or
not anything is hidden, which is the point: a button that only turns up once you
are stuck is a way back, never a way there. Inside it are **Attach a file…**
(see [Dropping and pasting files](/docs/dropping-files)), the typing-surface
switch, and a **Show** entry for each bar you have hidden, so you get back the
one you are missing rather than both.

Those preferences live on the server, so they follow you to every device. The
top bar is the phone shell's own chrome and simply does not exist in the wide
layout, but the terminal keys travel with your pointer, so hiding them from your
phone also hides them on the tablet you pick up next. The input menu therefore
appears wherever the keys themselves would, the wide touch layout included:
turning the keys off from one device never leaves another without a way to ask
for them back. Watching someone else's terminal rather than driving it? The
menu appears exactly when you need it: hide the header on a phone and it shows
up with the top-bar entry in it, so you can never end up with a screen you
cannot get out of.

## Install it like an app

dux ships a small PWA manifest, so your browser will offer to add it to your home
screen or dock, where it opens standalone without browser chrome. The offline
story is deliberately minimal: the service worker caches only a small "dux is
unreachable" fallback page, nothing else. The app itself always loads fresh from
the server, so there is zero risk of a stale bundle talking to a newer server.
When you genuinely lose the connection mid-session, dux grays the app out behind a
"Reconnecting…" overlay and reconnects when it can. The overlay sits on top of the
app rather than navigating away from it, so once you are back online you land
right back on the agent or screen you were already looking at.
