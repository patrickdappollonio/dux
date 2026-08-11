---
title: The workspace in the browser
description: The three-pane web layout, deep links, the browser terminals, the one-writer take-over model, copy-on-select, right-click paste and image paste, Shift-Enter soft newlines, companion terminals, and the mobile hub-and-spoke shell.
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
  search box filters it. Drag rows to arrange them by hand, and use the New
  agent and Add project controls (each a split button with a menu) to create
  work or reach a project. Toggle the sidebar with `Ctrl-b`.
- The **center pane** is the focused agent's live terminal, or a welcome screen
  when nothing is selected.
- The **right Changes pane** shows what the focused agent has changed. You can
  resize the split, or hide the Changes pane entirely when you want the terminal
  full width. See [Git without leaving the browser](/docs/web-git).

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

The sidebar's **Add project** button opens a folder picker that browses the
server's filesystem. Pick a git repository and it joins the workspace. But here
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

The `⋯` half of the Add-project button holds both flavors ("Add project…" and
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
but **only one device at a time can type.** That device is the owner. Whoever
attaches with the tab in the foreground claims ownership automatically; attaching
with the tab in the background joins as a silent read-only watcher, so you can
peek without stealing the keyboard out from under whoever is driving.

A read-only view shows a full-cover card naming who has it ("Open on Chrome on
macOS") with a **Take over** button. Click it and input snaps to you, most-recent
claim wins. Nothing is lost, the other device simply becomes the watcher until it
takes over in turn. It is a polite hand-off, not a fight.

### Clipboard: the classic terminal model

The web terminal copies and pastes the way a real terminal does, no menu required:

- **Select to copy.** Highlight text and it lands on your clipboard immediately.
  A small "Copied to clipboard" toast confirms it. This is governed by the
  `ui.copy_on_select` preference (on by default), which you can flip in
  **Preferences** (the cog menu).
- **Right-click to paste** (with a mouse or pen). It reads your browser clipboard
  and sends it to the agent. On plain HTTP, where the browser blocks clipboard
  reads, dux nudges you toward `Ctrl+v` instead.
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
Open one from the cog menu ("New standalone terminal"). Its row shows the
directory it opened in, shortened with `~`, where the other two show their owner,
so the sidebar search finds it by path.

Nothing closes a standalone terminal for you. Removing a project closes that
project's terminals and deleting an agent closes that agent's; neither has
anything to do with this one. It ends when you close it, or when dux shuts
down.

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
  menus as desktop, an **Add project** button, and a **cog** button that opens the
  app menu as a bottom sheet (submenus drill down in place, with a back arrow).
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
the compose box (a refused send keeps your draft and tells you why).

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

Terminal rows are precious on a phone, so the chrome around the terminal is
hideable. Every terminal screen's `⋯` menu (agent, project, and standalone
terminals alike) has **Hide top bar** (which on the agent screen also takes
the tab strip with it) and **Hide terminal keys**, each backed by its own
preference, and the reclaimed rows go straight back to the terminal. While
anything is hidden, a show-bars button appears below the terminal (next to
the compose box, or on its own slim row if you keep the compose bar off): one
tap brings both bars back. Both toggles live in Preferences too.

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
