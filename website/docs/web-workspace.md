---
title: The workspace in the browser
description: The three-pane web layout, deep links, the browser terminals, one-writer take-over, clipboard and touch selection, companion terminals, self-clearing messages, and the mobile hub-and-spoke shell.
group: Web UI
order: 61
---

Open the server URL and you land in the workspace: your projects, your agents, and
their live terminals, all click-driven. Both front ends lay the workspace out the same
way, so if you know the terminal UI the shape will be familiar.

## The layout

On a desktop-width screen it is three panes.

![The dux web workspace on a desktop screen: the sidebar of agents and terminals on the left, the running agent's terminal in the middle, and the list of changed files on the right.](/screens/web-workspace-layout.png#full)

**The collapsible left sidebar** lists your agents in a single flat list, with the
dormant ones tucked into a collapsible Inactive tail. Toggle it with `Ctrl-b`. It
carries:

- A sort control (Active first by default, or by recently updated, recently created,
  name, or a manual drag order). Drag rows to arrange them by hand.
- A search box that filters the list.
- A launcher at the bottom: one button for the next agent, and one `⋯` beside it holding
  every other way in, grouped into agents, terminals and projects. On an empty dux the
  button reads **Add project** instead.
- A **+** on the Agents header, and one on the Terminals divider that opens a standalone
  shell.

**The center pane** is the focused agent's live terminal, or a welcome screen when
nothing is selected.

**The right Changes pane** shows what the focused agent has changed. Resize the split,
or hide the pane entirely; dragging the split shut and letting go also hides it, and the
header grows a button to bring it back. See
[Git without leaving the browser](/docs/web-git).

**Both splits are yours to move, with the same gesture.** Drag the sidebar's right edge
or the Changes divider, with a mouse or a finger; double-click either one to put it back
where it was when the page loaded. Both remember where you left them. Tab to a divider
and the arrow keys nudge it, `Home` and `End` run it to its ends, and `Enter` puts that
side away. The sidebar's arrows move it one step of `1rem` at a time, small enough that a
single press cannot fold the whole thing away. Drag the sidebar narrow enough and it
snaps to its icon rail, which you reopen by clicking the same edge; drag the Changes
split shut and the pane hides.

Either divider lights up while you hold it, so a finger can tell it has the split and not
the pane beside it. Only a real drag counts: tapping near a divider, or starting a drag
the browser decides was a scroll, leaves both splits exactly where they were.

A slim header shows breadcrumbs (agent, provider, project, branch) and a **cog** button
that opens the app menu. The cog menu holds your preferences, the configuration dialogs,
and the actions that apply to the whole workspace. Anything that acts on a *specific*
agent, project, or file lives in the `⋯` menu on that row instead. The cog menu has no
keyboard shortcut: Tab reaches it, Enter opens it, the arrow keys move through it, and
Escape closes it.

> [!NOTE]
> The web UI is dark-only. Themes are a terminal UI feature. The browser ships one tuned
> dark palette and does not follow your system light/dark preference.

## Deep links

The URL tracks what you are looking at, so you can bookmark a session or send a link to
someone on the same instance:

- An agent is `#/agent/<sessionId>`.
- An extra provider tab is `#/agent/<sessionId>/tab/<tabId>`.
- A companion terminal is `#/agent/<sessionId>/terminal/<terminalId>`.
- A project terminal is `#/project/<projectId>/terminal/<terminalId>`.
- A standalone terminal is `#/terminal/<terminalId>`, with no owner in the address.
- The phone's Changes screen is the same link with `/changes` on the end.

Every move to a different screen adds one history entry, so Back always returns you to
the previous screen. Moves that stay on the same screen rewrite the one entry instead:
switching between agents or tabs, reconnecting after the wifi drops, and following a
link back to where you already were.

dux never presses Back for you. The phone's back chevron is an **up** control: it takes
you one level up (Changes to its agent, an agent to the home screen), adds its own
history entry, and can never step out of dux.

Three things can go wrong with a link:

- **The agent was deleted.** You get a plain **Agent not found** screen rather than a
  silent bounce home. It gives way on its own if that agent reappears, and leaving it
  corrects the URL rather than stacking on top of it, so Back cannot drop you back onto
  the dead end.
- **The agent you are watching is deleted out from under you.** dux moves you to the next
  active agent, or home when there is not one.
- **The terminal has since closed.** It lands you on the agent that owned it, or on the
  home screen for a terminal that belonged to a project or to nothing at all, and tidies
  the address bar to match.

## Adding projects

**Add project…** lives in the launcher's `⋯` menu at the bottom of the sidebar (and on
the launcher button itself while you have no projects yet). It opens a folder picker that
browses the server's filesystem. Pick a git repository and it joins the workspace.

> [!TIP]
> The folder does not have to be a repository yet, which is what makes this work from a
> phone with no terminal in reach.

- Point it at a plain folder (via the pinned **Use this folder** row at the top of the
  list) and dux offers to **initialize a repository**: it runs `git init`, seeds a
  commented starter `.gitignore` for dependency and build folders it actually finds
  (`node_modules`, `target`, and friends), makes an empty initial commit, and adds the
  project. Your existing files are left untracked and untouched.
- The picker's **New folder** button creates a directory from the browser.
- Pick a folder *inside* an existing repository and dux refuses, pointing you at the
  repository root instead.

The launcher's `⋯` holds both flavors under **Projects** ("Add project…" and "Initialize
a repository…"); either way the picker inspects your selection and offers the right
action. The TUI's project browser makes the same offer.

## The browser terminals

Each agent runs its real CLI on the server and the browser shows it live. Closing the
tab, losing connectivity, or your phone falling asleep does not stop the agent: it keeps
running until you explicitly kill or delete it (see
[Agents from the browser](/docs/web-agents)).

> [!IMPORTANT]
> Opening a terminal is what starts the agent: it **launches or resumes** the provider if
> it is not already running.

The full scrollback comes back when you open it, and the "Reconnecting…" cover stays up
until that screen has actually been painted, not merely until the connection is back, so
you never end up looking at an empty black pane wondering whether anything is happening.

**dux keeps trying while the page is open, and reconnects the moment you come back.**
There is no give-up count. A visible tab retries with a widening gap between attempts, up
to `reconnect_backoff_cap_seconds` (10 seconds by default), for as long as you leave it
open. A hidden tab stops trying altogether rather than burning your battery on a
connection you are not looking at, and picks it straight back up when you switch back to
it, unlock the phone, or the network returns. Your scrollback and anything half-typed in
the compose box survive all of it.

Two things can still stop the wait, and both offer you a **Reconnect** button rather than
leaving you stuck. If the terminal's screen has not arrived within
`replay_wait_seconds` (8 seconds by default) of the connection coming up, the cover says
so and offers to try again. And if the connection is really gone, the blocking
**Connection lost** card takes over.

> [!NOTE]
> A phone that switches from Wi-Fi to cellular can leave a connection that looks alive
> but answers nothing. dux checks, from the browser, every `heartbeat_seconds`
> (15 by default), and if the answer has not come back within
> `heartbeat_deadline_seconds` (30 by default) it reconnects rather than leaving you
> typing into a dead terminal. All four of those live under `[server]` in your config
> file; see [Server mode](/docs/server-mode#the-server-config-keys).

### Theater mode: one pane, no chrome

The expand button hands the whole screen to the terminal. On a computer it is in the
terminal's header, next to **Macros…**; on a phone it is the first button in the flap
that hangs off the band above the terminal. That band is the tab strip when the agent has
more than one tab and the header itself when it does not, and the flap takes the colour of
whichever one it is hanging from, so it always reads as part of it. The header, the
pull-request band and the tab strip slide away and the terminal grows into the space they
were using; on a phone the app header goes with them, and the flap lifts off the band and
flies to the floating pill, which is the same four buttons in the same order.
The compose box and the terminal keys stay exactly as you had them, and the pill's `⋯`
keeps offering the way back to them, so theater is as bare or as equipped as
you want it. Theater is the only thing that takes the top bar away, and it all comes
back when you leave. On a computer both
side panels leave too, the agent list and the changes list, and they come back as you had
them: open, collapsed to the rail, hidden, and at the widths you dragged them to. Open a
link that goes straight into theater and there was no layout on screen to remember, so
leaving lands on your saved preferences instead.

A small floating pill in the bottom-right corner is the only thing left over the
terminal. On a computer it holds the way out, the macro picker and a `⋯` carrying the
same **Settings** menu the header's cog carries, so Preferences, new agents and
everything else stay one tap away, with **Leave theater mode** at the bottom of it. On a
phone it is the flap that flew there: the theater button (now showing the way out), the
macro picker, the changed-file count and the same `⋯`, opening the same menu it opened on
the band. That menu opens with the **Input** group (**Attach a file…**, and the way
back to the typing bar when you have turned it off), then the agent's own actions, the way
out of theater while the mode is on, and **Settings**, which drills into
the app menu the cog carries, so nothing about an agent goes out of reach because you
went full screen. The changed-file count is the button beside it rather than a row in it.

![Theater mode on a phone: the top bar is gone, the floating pill hovers over the terminal, and the keys and message box are still there.](/screens/phone-theater-pill.png)

Notice what did **not** leave: the terminal keys and the message box. Theater takes the
chrome above the terminal, not the way you answer it. If you have asked to type straight
into the terminal, there is nothing below it either, and the pill's `⋯` is the way back.

The pill carries no tab status. What the other tabs are doing is in the agents list and
the tab strip, and one copy of that is enough. Theater on a phone hides both, so a tab
you cannot see that needs you will be waiting when you come back out: that is the trade
this mode makes for a screen with nothing on it but the terminal.

The pill does not have to stay in that corner. Its leading edge is a grip: hold the grip
and drag, with a finger or with the mouse, and the pill goes anywhere over the terminal,
never further than its edges. A quick tap does nothing, so the buttons beside the grip
keep working as buttons. If you would rather not drag, focus the grip and the arrow keys
move it a step at a time. Where you leave it is remembered on that device, for every
agent and every terminal, and it is nudged back inside if you rotate the phone or shrink
the window, and back to where you put it once there is room for it again.

The button you came in by is not a way back, because the header it lives in is one of the
things that left. On a computer there are three ways out: the pill, `Escape` whenever you
are not typing into the message box or the terminal itself, and your browser's Back
button. There is a fourth in the pill's own `⋯` menu, as **Leave theater mode**, and that
one is on every surface: the pill is the menu the phone has too.

Back works because theater is part of the address: a link copied in theater opens in
theater on whatever device you send it to. Opening the file editor or the changes screen
steps out of the mode, since neither of those is the terminal; closing them puts you back
in it.

> [!TIP]
> Theater is remembered per tab, in that browser, so the agent you like watching
> full-screen comes back full-screen and the others do not. If another device takes over
> the terminal, theater ends and the memory goes with it, so you always come back to the
> full layout to decide what to do next.

### One writer, many watchers

Every device pointed at the same terminal sees the same output at the same time, but
**only one device at a time can type.** That device is the owner, and **you join as a
watcher unless nobody is driving.** If nobody holds the terminal, the first device to
look at it picks it up.

![A card covering the terminal that says the agent is open on another device, with a Take over button.](/screens/take-over-card.png)

**The terminal UI is one of those devices.** When dux is
[serving in the background of a running TUI](/docs/server-mode#serve-in-the-background-and-keep-the-tui),
that terminal can hold a terminal or be a watcher like any browser, and the card names it
as `the dux TUI`. It shows the same card, with the same words, when your browser is the
one driving.

**A watcher sees the take-over card**, a full-pane card naming who has the keyboard
("Active on Chrome on macOS") with a **Take over** button. Hit it and dux resizes the PTY
to your screen, repaints it fresh, and hands you the keyboard. Nothing is lost; the other
device becomes the watcher.

Taking over reattaches, so it takes a moment and you will see "Reconnecting…" until the
screen is really there. Behind the card, a watcher's terminal renders at the driver's true
size with the text shrunk to fit, so the screen you take over is clean from the first
frame.

> [!IMPORTANT]
> **A take-over interrupted by a network drop is not retried; press Take over again.** The
> reconnect that follows a dropped signal is a plain attach, on purpose, so a request you
> made minutes ago on a train cannot surface later and yank the terminal out from under
> whoever is using it now. The button always responds: if the attempt did not land,
> pressing it again starts a fresh one.

> [!IMPORTANT]
> **Losing the keyboard is sticky.** Nothing passive gives a terminal back, so a watcher
> left on screen never grabs a terminal somebody's flaky wifi dropped. Press **Take
> over**, reload, or navigate away and back, and it is yours. One tap is the whole cost of
> switching devices.

If the driving device disconnects, every card switches to **Running in the background**
rather than naming a browser that has gone: the session kept running without its driver. The card also keeps naming the device that had the
keyboard even while your own connection is wobbling, so a flaky signal no longer turns
"Active on Chrome on Linux" into a vague "Active on another device".

**Reconnecting never takes the terminal from anybody.** Every automatic reconnect joins as
a watcher unless nobody is driving; taking a terminal away from a device that holds it is
always something you press the button for. The one exception takes nothing from anyone:
the device that lost the keyboard to a blip gets it back when it returns, provided the
server is still holding its old, dead session and nobody else has claimed the terminal in
the meantime. If somebody has, you come back as a watcher with the card up, and one tap
puts you back in the driver's seat. The same goes if your page cannot confirm it is
talking to the very same dux it started against, because dux was restarted while you were
away or the check could not get through: coming back automatically is only safe when the
old session is provably still the one you left, so you land as a watcher and press the
button. A page in the background stays a watcher until it is
in front again. If your own connection has given up entirely, the card steps aside for the
**Connection lost** notice and its **Reconnect** button.

### Clipboard: the classic terminal model

The web terminal copies and pastes the way a real terminal does, no menu required:

- **Select to copy.** Highlight text and it lands on your clipboard immediately, with a
  "Copied to clipboard" toast. Governed by the `ui.copy_on_select` preference (on by
  default), which you can flip in **Preferences** (the cog menu).
- **Right-click to paste** (mouse or pen). On plain HTTP, where the browser blocks
  clipboard reads, dux nudges you toward `Ctrl+v` instead. On a touch screen the same
  press-and-hold gesture belongs to selection, which is the next section.
- A fixed set of chords works too, and it is not user-configurable: `Ctrl+Shift+c`,
  `Ctrl+Insert`, or `Cmd+c` to copy, and `Ctrl+v`, `Ctrl+Shift+v`, or `Cmd+v` to paste,
  with `Ctrl+c` staying SIGINT.
- **Paste an image with the keyboard and dux uploads it**, saving it on the server and
  pasting its **path** into the prompt. Text paste is untouched, and `Ctrl+Shift+v`
  (`Cmd+Shift+v`) forces the text when the clipboard carries both. Keyboard chords only:
  right-click paste can never carry an image. See
  [Dropping and pasting files onto an agent](/docs/dropping-files).

There is deliberately no right-click context menu, because it would fight the paste
gesture.

> [!TIP]
> When the app inside the terminal grabs the mouse, a plain drag goes to that app instead
> of selecting text. Hold **Shift** (Linux and Windows) or **Option** (macOS) while
> dragging to force a local selection. dux pops a one-time hint the first time this bites
> you.

### Selecting text with a finger

On a phone or tablet, **press and hold** on the terminal. The word under your finger
highlights, and dragging extends the selection. Drag past the top or bottom edge and the
terminal keeps scrolling for as long as you hold there. **Lift your finger and it is
copied**, under the same `ui.copy_on_select` preference. The highlight stays up so you can
see what you got; the next tap clears it. A press is not a tap, so the keyboard stays down
and does not cover what you highlighted.

A long press **always** selects locally, even when a full-screen agent has taken the
mouse, so this is the touch version of the Shift and Option hatch above.

Two limits: there are no drag handles, so a selection cannot be adjusted once you lift
(press again to redo it), and if the agent is writing fast enough to push lines out of the
scrollback mid-drag the selection can slide onto the wrong text, which pressing again also
fixes.

Your agent can also write your clipboard directly through an `OSC 52` escape sequence, and
that write lands on **your** browser's clipboard, not the server's, governed by the
`clipboard_passthrough` capability. See
[Terminal capabilities](/docs/terminal-capabilities).

### Drag a file in, or paste one

Drag a screenshot (or any file) from your desktop onto the terminal, or paste an image,
and dux saves it on the server and pastes its path into the prompt. Dropped on an agent it
goes to that agent's upload folder (`.dux/uploads` in its worktree), invisible to git and
deleted along with the agent; dropped on a terminal it goes to the folder that terminal is
in right now. Nothing is ever overwritten, your filename is kept as you had it, and only
the device holding input can drop or paste. On a phone, pasting an image puts its path
into your compose draft. See
[Dropping and pasting files onto an agent](/docs/dropping-files).

### Shift-Enter for a soft newline

In the browser, **Shift-Enter inserts a newline instead of submitting.** Plain Enter still
submits. This is web-only (the TUI cannot tell the two apart), and it never fires
mid-composition, so it will not mangle a CJK input method's confirm keystroke.

## Companion terminals

Three kinds of plain shell live in their own collapsible **Terminals** section in the
sidebar:

- A **companion terminal** runs in an agent's worktree, for tests, git, or tailing a log
  while the agent works. Its row names the owning agent, and the title tracks whatever is
  in the foreground ("vim", "htop").
- A **project terminal** opens at the project's repo root with no agent attached. It is
  the escape hatch when dux will not do something for you remotely, even over Tailscale
  with no local terminal in sight. Spawn one from the project's ⋯ menu ("New project
  terminal"); it shows up in the Task Manager and is destroyed on close.
- A **standalone terminal** opens in your home directory with no agent and no project, so
  you can reach for one before you have added a single project. Open one from the
  **Terminals** group of the launcher's `⋯` menu, from the **+** on the Terminals divider,
  or from the cog menu's **New** submenu. Its row shows the directory it opened in,
  shortened with `~` and marked with the `✷` standalone star, where the other two show the
  `↳` arrow and their owner, so the sidebar search finds it by path. The star always means
  the same thing: this one lives in your folder, not a working copy dux manages.

Every terminal row's `⋯` menu also carries **Open editor here** and **Open editor in new
tab**, rooted at the directory that terminal opened in. A terminal that belongs to an
agent opens that agent's editor instead. See [The code editor](/docs/web-editor).

> [!WARNING]
> Killing a companion terminal **destroys** it. Agents detach when you kill them;
> terminals do not.

Nothing closes a standalone terminal for you. Removing a project closes that project's
terminals and deleting an agent closes that agent's; neither touches a standalone one. It
ends when you close it, or when dux shuts down.

## Messages

Progress and results arrive as small messages at the bottom of the screen, and **they
clear themselves.** How long a finished one stays depends on how much it matters: a
success or a note uses your `ui.status_clear_seconds` window (six seconds by default), a
warning stays three times that, and an error four times. Setting `ui.status_clear_seconds = 0`
turns auto-clearing off. Dismiss one early by swiping it away or with its close button.

A message about work still in flight is a spinner: no close button and no swipe, replaced
in place by its own result the moment there is one. If the connection drops mid-operation
and no result arrives, the spinner gives up after a minute.

A small number stay until you dismiss them, and only for two reasons: **you have to go and
do something outside the message**, or something may have been lost or left half-finished.
A file that was saved but never handed to the agent waits, because the message holds the
only copy of where it went. A worktree that could not be removed waits, because it is
still on disk. A failed pull does not wait.

If several arrive at once they stack, five deep on a desktop and three on a phone, and the
rest queue behind them. They stack in the workspace tab: the standalone editor tab shows
only what you do in it.

> [!NOTE]
> After a reconnect you still see work that is **still running**, plus any result from the
> last thirty seconds. Older results are not repeated.

## Macros

A floating **Macros…** button drops prewritten prompt snippets into the focused terminal
without submitting them, so you review and press Enter yourself. Each macro is scoped to
agents, terminals, or both, and multi-line macros are inserted so they do not submit line
by line. Full details in [Managing Macros](/docs/macros).

## On your phone

Below tablet width, the web UI becomes a **hub-and-spoke** shell built for one thumb:

![The dux home screen on a phone, listing agents with their state: one waiting on you, the rest working.](/screens/phone-hub.png)

- The **home** screen is the hub: your projects and sessions with the same `⋯` menus as
  desktop, the same launcher pair along the bottom, and a **cog** button that opens the
  app menu as a bottom sheet (submenus drill down in place, with a back arrow). The
  launcher's `⋯` opens as a bottom sheet too. Tap a session to jump into its terminal.
- The **terminal** screen is a full-screen terminal with a slim bar on top (Back, the
  agent's name over its assistant, branch and project, and an optional PR chip) and the
  tab strip when the agent has more than one tab. The actions hang off the strip on the
  right, in a small tab-shaped flap over the terminal: theater mode, the macro picker,
  the changed-file count, and the session's `⋯` menu, which carries the **Input** group,
  the agent's actions and a **Settings** drill into the app
  menu. The flap takes no room from the terminal, so theater does not take it away: it
  lifts off the band and flies to the floating pill instead.
- The **changes** screen is the full Changes pane.

An **accessory bar** sits above the soft keyboard with the keys a phone keyboard lacks:
Esc, Tab, a sticky Ctrl and Alt latch, arrow keys, PgUp/PgDn, and a dedicated **⇧↵** key
for the soft newline. Touch targets are sized generously. The soft keyboard is handled by
the browser shrinking the layout, so the bar sits flush on top of it, and there is no
fullscreen mode to fight with.

![The phone typing bar: two rows of terminal keys above a message box with a send button.](/screens/phone-compose-bar.png)

Below it sits the **compose bar**: a real text box where you type the message and hit
**Send**. This is where your keyboard's autocorrect, swipe typing, and voice input actually
work. Enter adds a newline instead of submitting; Send delivers the whole message and
presses Enter for you, and an empty Send is a plain Enter for confirming menus and prompts.
Tapping the terminal drops you into the compose box, and a refused send keeps your draft
and tells you why. The box holds the keyboard focus the whole time, so the terminal's caret
stays a solid block rather than hollowing out.

A physical keyboard types into the compose box. The keys a text box has no use for, **Esc**
and **F1** through **F12**, go straight to the terminal, so Esc on a tablet's keyboard case
interrupts a running agent exactly like the accessory bar's Esc key. A few F-keys are
grabbed by the browser itself before any page can see them, F12's developer tools being the
classic, so those may trigger both. Modified presses stay with your browser (Ctrl+C keeps
meaning copy, not SIGINT); for every keystroke on the wire, the Direct typing surface is one
tap away.

Whether the compose bar appears is the `ui.compose_bar` setting, a Preferences row with
three values:

- **Automatic** (the default) asks your browser whether you point at the screen with a
  finger, and uses the answer as the starting point. Your own choice from the typing-surface
  switch wins from then on.
- **Always** and **Never** are for the device dux guesses wrong on. Never restores typing
  straight into the terminal. On a touch device it keeps the terminal keys, since a finger
  still cannot produce Esc or a Ctrl chord and there is no switch under **Never** to bring
  them back with; it is only the message box that goes.

An older config that says `compose_bar = true` or `false` keeps working: `true` is read as
Automatic and `false` as Never.

> [!IMPORTANT]
> **Width decides the layout; your pointer decides which typing surface you start with.** A
> tablet in landscape gets the desktop panes *and* the accessory keys and compose box. A
> mouse on a narrow window starts with neither. Rotating a tablet never swaps your typing
> surface out from under you.

What the browser cannot see is a keyboard case, so there is a **typing-surface toggle**.
Turning it off is at hand while the typing bar is up: **Direct** at the end of the
accessory bar's key row, and **Type directly in the terminal** in the input `⋯` beside the
message box. Turning it back on is **Use virtual input**, in the **Input** group at the top
of the pane's own `⋯` menu (the flap's on a phone, the row menu in the sidebar on a
computer), because by then the typing bar is gone and so is everything that was on it.

**Your choice wins, on any device, in both directions.** The pointer only decides where you
start. Ask for the message box on a laptop and you get it, keys and all, and while it is up
your keystrokes go into the box rather than straight to the terminal; switch back to Direct
and the whole bar goes, message box, keys and menu together, giving the terminal every row
on the screen. The first time you do that dux tells you once, on that device, where the way
back is. Your choice sticks across reloads and survives folding a
convertible or unplugging a mouse, until you change it. It is not a setting: it is
remembered on that device and changes nothing in your config, and it appears only under
**Automatic**, since **Always** and **Never** have already decided. Switching the
preference to **Always** or **Never** does not erase it either: the choice stays remembered
in that browser, and comes back the moment you put the preference on **Automatic** again.

While the message box is up it really is the typing surface. Clicking into the terminal
puts your cursor back in the box, right-clicking pastes the clipboard into your draft
instead of sending it, and the terminal is skipped when you tab backwards out of the pane.
Switch to Direct and all of that goes back to the way a plain terminal behaves.

Terminal rows are precious on a phone, so the terminal keys are hideable. Every phone
terminal screen's `⋯` menu (in the flap on the agent screen, in the header on project and
standalone terminals) has **Hide terminal keys**, backed by a preference that is also a
row in Preferences.

For the top bar the answer is [theater mode](#theater-mode-one-pane-no-chrome), not a preference. It takes
the header and the tab strip together, and it hands you the way back at the same time: the
floating pill, and **Leave theater mode** in the menu it opens. A preference that hid the same
chrome would be a second way to reach the same place, with no way back of its own.

The input `⋯` sits at the left edge of whatever input row you have: beside the message box,
or in the key row when the box is off. It belongs to the typing bar and it goes when the bar
does, so what is inside it is only what that bar is about: **Type directly in the terminal**
and **Hide terminal keys**.

Everything you must be able to reach when the bar is not there is one level up, in the
**Input** group at the top of the pane's own `⋯` menu: the flap's on a phone, the agent's
or terminal's row menu in the sidebar on a computer, the floating pill's in theater. That
group carries **Attach a
file…** (see [Dropping and pasting files](/docs/dropping-files)) and **Use virtual input**,
which only appears while the typing bar is down, since while it is up the menu beside the
message box already offers the other direction. **Show terminal keys** joins the group the
same way, in the one case where the message box is switched off in Preferences and you have
hidden the keys as well.

The terminal-keys preference lives on the server, so it follows you to every device, and
the keys travel with your pointer, so hiding them from your phone also hides them on the
tablet you pick up next.

## Install it like an app

Your browser will offer to add dux to your home screen or dock, where it opens standalone
without browser chrome. Offline it shows a small "dux is unreachable" page and nothing else;
the app itself always loads fresh. When you lose the connection mid-session, dux grays the
app out behind a "Reconnecting…" overlay and reconnects when it can, without navigating
away, so you land right back on the screen you were looking at.
