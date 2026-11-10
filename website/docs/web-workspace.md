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

**The collapsible left sidebar** lists your agents and your project and standalone terminals in one list, titled **Agents & Terminals**, with the dormant agents tucked into a collapsible Inactive tail. Toggle it with `Ctrl-b` (`Cmd-b` on a Mac). It carries:

- A sort control (Active first by default, or by recently updated, recently created, name, or a manual drag order). Drag rows to arrange them by hand.
- A search box that filters the list.
- The launcher at the bottom: **New agent…**, with a **▾** beside it that lists every other way to create something, grouped into agents, terminals and projects. See [Agents from the browser](/docs/web-agents#creating-an-agent).

**The center pane** is the focused agent's live terminal, with its own terminals docked underneath it (see [Companion terminals](#companion-terminals)), or a welcome screen when nothing is selected.

**The right Changes pane** shows what the focused agent has changed. Resize the split, or hide the pane entirely; dragging the split shut and letting go also hides it, and the header grows a button to bring it back. See [Git without leaving the browser](/docs/web-git).

### Moving the splits

**Both splits are yours to move, with the same gesture.**

- **Drag** the sidebar's right edge or the Changes divider, with a mouse or a finger. Either divider lights up while you hold it, so a finger can tell it has the split and not the pane beside it.
- **Double-click** either one to put it back where it was when the page loaded. Both remember where you left them.
- **Keyboard:** Tab to a divider and the arrow keys nudge it, `Home` and `End` run it to its ends, and `Enter` puts that side away. The sidebar's arrows move it one step of `1rem` at a time, small enough that a single press cannot fold the whole thing away.
- **Changes split:** drag it shut and the pane hides.

Only a real drag counts: tapping near a divider, or starting a drag the browser decides was a scroll, leaves both splits exactly where they were.

### The icon rail

Drag the sidebar narrow enough and it snaps to its icon rail, which you reopen by clicking the same edge.

- **What it shows.** The rail is for what is alive: your active agents and your project and standalone terminals, in the list's order, one icon each. Agents in the Inactive tail sit it out until you wake them.
- **Peek.** Rest your mouse on a rail icon and the whole sidebar slides out over the page, Inactive tail and all, without nudging your terminal an inch. Pick something and it tucks itself away again.
- **Touch.** On a touchscreen a tap still just picks the icon.

### The header and the cog

A slim header shows breadcrumbs (agent, provider, project, branch) and a **cog** button that opens the app menu. The breadcrumbs name the row you picked in the list: an agent, with how many terminals it has, stays named the same whichever of its tabs or terminals you are typing into, and a project or standalone terminal names itself. **Macros…** beside them sends to the part you are typing into, the agent or one of its terminals. The cog menu holds:

- your preferences
- the configuration dialogs
- the actions that apply to the whole workspace
- **Projects…**, the list of every project (see [Managing projects](#managing-projects))

Anything that acts on a *specific* agent, project, or file lives in the `⋯` menu on that row instead. The cog menu has no keyboard shortcut: Tab reaches it, Enter opens it, the arrow keys move through it, and Escape closes it.

> [!NOTE]
> The web UI is dark-only. Themes are a terminal UI feature. The browser ships one tuned
> dark palette and does not follow your system light/dark preference.

## Deep links

The URL tracks what you are looking at, so you can bookmark a session or send a link to someone on the same instance:

- An agent is `#/agent/<sessionId>`.
- An extra provider tab is `#/agent/<sessionId>/tab/<tabId>`.
- One of an agent's terminals is `#/agent/<sessionId>/terminal/<terminalId>`, or `#/agent/<sessionId>/tab/<tabId>/terminal/<terminalId>` with an extra tab on top of the agent. The link names the part you were typing into, and a link naming a terminal opens with that terminal on screen beside its agent. The layout (split or maximized) is not part of the link.
- A project terminal is `#/project/<projectId>/terminal/<terminalId>`.
- A standalone terminal is `#/terminal/<terminalId>`, with no owner in the address.
- The phone's Changes screen is the same link with `/changes` on the end.

### Back and Forward

Every move to a different place adds one history entry, so Back always returns you to where you were: select three agents in turn and Back walks them in reverse. Every tab and every terminal you look at is a place of its own, like browser tabs: switch from one of an agent's tabs to another, or from one of its terminals to the next, and Back takes you to the one before.

Changing the layout, or clicking from the agent into its terminal and back, adds nothing: the address follows the part you are typing into, rewritten in place. Closing the tab you are on adds nothing either: its step becomes the tab beside it. If Back or Forward lands on a tab that has since closed, dux says so, tells you where you are now, and keeps going the way you were: Back to the agent (or past it, if you just came from there), Forward to the step after it, or back where you came from when there is nothing after it. Moves that stay where you already are rewrite the one entry instead:

- reconnecting after the wifi drops
- following a link back to where you already were

dux never presses Back for you. The phone's back chevron is an **up** control: it takes you one level up (Changes to its agent, an agent to the home screen), adds its own history entry, and can never step out of dux.

### Dialogs and Back

An open dialog, sheet or menu is one step of its own, with no change to the address. Back (or your phone's back gesture) closes it exactly as Cancel or `Escape` would: nothing it offered is saved, confirmed or sent, and the page underneath stays where it is. Press Back again to move the page.

- Closing a dialog any other way takes its step away too, so Back is never spent on something already closed, and Forward never brings a closed dialog's step back.
- A dialog opened from another one stacks, and each Back closes the top one.
- A dialog that is in the middle of saving when you press Back closes as soon as the save finishes.
- Two cases keep the plain behavior, where Back moves the page and leaves the dialog alone: some older browsers, and a dialog dux raises by itself rather than in answer to a click or a tap, such as the welcome screen.

### When a link goes wrong

Three things can go wrong with a link:

- **The agent was deleted.** You get a plain **Agent not found** screen rather than a
  silent bounce home. It gives way on its own if that agent reappears, and leaving it
  corrects the URL rather than stacking on top of it, so Back cannot drop you back onto
  the dead end.
- **The agent you are watching is deleted out from under you.** dux moves you to the next
  active agent, or home when there is not one.
- **The terminal has since closed.** You get a **Terminal not found** screen with a
  **New terminal** button that opens a fresh one in the same place: in that agent's
  worktree, at that project's root, or a new standalone terminal. It shows every time you
  arrive at such a link, Back included, so close three terminals and walk back and you
  see it three times rather than landing somewhere the link never named.

![The Terminal not found screen: a short explanation naming the terminal's id, a New terminal button, and Back to agents.](/screens/terminal-not-found.png)

## Adding projects

**Add project** lives in the **Projects** group of the menu the **▾** beside **New
agent…** at the bottom of the sidebar opens (with no projects yet, that group comes
first). It opens a
folder picker that browses the server's filesystem. Pick a git repository and it joins the
workspace.

> [!TIP]
> The folder does not have to be a repository yet, which is what makes this work from a
> phone with no terminal in reach.

- **Point it at a plain folder** (via the pinned **Use this folder** row at the top of the list) and dux offers to **initialize a repository**. It:
  - runs `git init`
  - seeds a commented starter `.gitignore` for dependency and build folders it actually finds (`node_modules`, `target`, and friends)
  - makes an empty initial commit
  - adds the project

  Your existing files are left untracked and untouched.
- The picker's **New folder** button creates a directory from the browser.
- Pick a folder *inside* an existing repository and dux refuses, pointing you at the repository root instead.

> [!NOTE]
> If git on that machine cannot work out an identity for that first commit, dux fills in `dux@localhost` as the address (keeping your `user.name` if git knows one, and signing as `dux` if it does not) so adding the folder still succeeds. That applies to dux's own empty first commit and nothing else: your later commits still need an identity you have configured.

The launcher's **▾** menu holds both flavors under **Projects** ("Add project…" and "Initialize a repository…"); either way the picker inspects your selection and offers the right action. The TUI's project browser makes the same offer.

### Cloning a repository

Not on the server yet? **Clone a repository** sits right beside them, under **Projects** in the launcher's **▾** menu and in the app menu's **Add project** submenu. Paste the remote's address (a short `owner/repo` or a GitHub page straight from your browser works too) and dux clones it, adds it as a project and starts its first agent, all in one go.

- The destination folder fills itself in from the address (your start directory plus the repository's name) until you type over it.
- The agent gets the name you give it or a random one, exactly like **New agent**.
- When the agent appears, dux selects it for you, however long the clone took.

The full story, including what happens when something goes wrong halfway, is in [Creating agents](/docs/creating-agents#cloning-a-repository).

## Managing projects

**Projects…** in the app menu (the cog), right under **Add project**, lists every project dux knows about, with a search box on top. It is the place to manage a project that has no agents yet.

- **Order:** whichever happened last, adding the project or creating its newest agent, latest first.
- **Each row** shows the project's folder, how many agents it has, and its base branch (the branch new agents start from), plus a warning when the folder is missing.

Click a row, or its `⋯`, for that project's menu.

- Dialogs you open from it (delete, remove, project info, project settings, worktrees, checking out the default branch, changing the base branch) open on top of the list, and closing them takes you back to it.
- Anything that ends with a new agent or a new terminal (including adopting a worktree from **Worktrees…**) closes the list and takes you there.

If agents are left over from a project whose record is gone, they show up in the list under their group's name, and the only thing on offer is **Remove project…**, which clears them out.

### Changing the base branch

**Change base branch…** in a project's menu lets you pick any branch as the one new agents
start from: a local branch, or one that so far exists only on `origin` (dux creates the
local branch, tracking origin's, when you pick it). Handy when a project was added from a
clone that sat on the wrong branch.

- **Opening it** fetches `origin` first. If that fails or times out, the list shows the branches as they were last fetched and says why.
- **The current base** is marked.
- **A branch another worktree has checked out** (usually an agent's) is shown but cannot be picked, because git will not check a branch out twice. The row names the agent, or the folder when no agent owns it.
- **After you confirm,** dux switches the project folder to that branch and then saves it as the base. If git refuses the switch (say, uncommitted changes in the folder would be overwritten), you get an error and the base stays where it was.

> [!IMPORTANT]
> This moves the project folder itself onto the branch. Uncommitted changes that do not
> conflict come along with it, the same as with **Check out default branch…**.

> [!CAUTION]
> **Git treats ignored files as disposable.** Switching the project folder, **Pull project** and an agent's **Pull** make git delete or overwrite an ignored folder or file when the incoming branch tracks that same path.
>
> - dux **refuses, and changes nothing,** when that folder holds, or that file sits inside, an agent's worktree, a standalone agent's folder, a project's repository or a terminal working there, and says which.
> - Any other ignored folder or file git still replaces, so move it aside first if it matters.
> - A folder the incoming commit deletes every file of goes too, as does a link it deletes or replaces (tracked or not). dux refuses the same way when one of those lives or works there.

> [!NOTE]
> These pulls and switches never update submodules, even when your git config sets
> `submodule.recurse`. Run `git submodule update` yourself when you want them to follow.

Only one of **Pull project**, **Check out default branch…** and **Change base branch…**
runs on a project at a time; starting another while one is running is refused and names
the one in progress.

## The browser terminals

Each agent runs its real CLI on the server and the browser shows it live. Closing the
tab, losing connectivity, or your phone falling asleep does not stop the agent: it keeps
running until you explicitly kill or delete it (see
[Agents from the browser](/docs/web-agents)).

> [!IMPORTANT]
> Opening a terminal is what starts the agent: it **launches or resumes** the provider if
> it is not already running.

The full scrollback comes back when you open it, and the "Reconnecting…" cover stays up until that screen has actually been painted, not merely until the connection is back, so you never end up looking at an empty black pane wondering whether anything is happening.

### Reconnecting

**dux keeps trying while the page is open, tells you how it is going, and reconnects the moment you come back.** A visible tab retries with a widening gap between attempts, and the cover says which attempt failed and counts down to the next one.

The knobs all live under `[server]` in your config file (see [Server mode](/docs/server-mode#the-server-config-keys)):

- **`reconnect_backoff_cap_seconds`** (10 by default) is the longest the gap between attempts grows to.
- **`reconnect_attempts`** (8 by default) is how many attempts dux makes before it stops. Set it to `0` to keep trying indefinitely.
- **`reconnect_attempt_timeout_seconds`** (10 by default) abandons an attempt that never connects and counts it as a failure, because a host you cannot reach does not refuse the connection, it just never answers.
- **`replay_wait_seconds`** (8 by default) is how long the cover waits for the terminal's screen to arrive once the connection is up.
- **`heartbeat_seconds`** (15 by default) and **`heartbeat_deadline_seconds`** (30 by default) are the dead-connection check described below.

When the attempts run out, dux stops rather than spinning forever in front of a server that is not there, says so plainly, and offers a **Reconnect** button that starts over.

**The budget resets** when anything connects, and also when you come back to the tab, unlock the phone or the network returns.

**In the background:**

- The page's own connection keeps counting while the tab is hidden, so that it can still reach you with attention alerts, and it can run out of attempts back there. You will find it stopped when you return, and returning is itself what starts it over.
- Your terminals are the half that waits: a hidden tab stops retrying them altogether rather than burning your battery on a screen you are not looking at, and picks them straight back up when you switch back to it, unlock the phone, or the network returns.

Your scrollback and anything half-typed in the compose box survive all of it.

Two things can still stop the wait, and both offer you a **Reconnect** button rather than leaving you stuck:

- The terminal's screen has not arrived within `replay_wait_seconds` of the connection coming up. The cover says so and offers to try again.
- The connection is really gone. The blocking **Connection lost** card takes over.

> [!NOTE]
> A phone that switches from Wi-Fi to cellular can leave a connection that looks alive but answers nothing. dux checks, from the browser, every `heartbeat_seconds`, and if the answer has not come back within `heartbeat_deadline_seconds` it reconnects rather than leaving you typing into a dead terminal.

### Theater mode: one pane, no chrome

The **Theater** button hands the whole screen to the terminal. The header, the pull-request band and the tab strip slide away and the terminal grows into the space they were using.

**Where the button is.**

- **On a computer** it is in the terminal's header, beside **Macros…** and the pane's own `⋯`.
- **On a phone** it is the first button in the flap that hangs off the band above the terminal. That band is the tab strip when the agent has more than one tab and the header itself when it does not, and the flap takes the colour of whichever one it is hanging from, so it always reads as part of it.

**What happens when you go in.**

- On a phone the app header goes with the rest, and the flap lifts off the band and flies to the floating pill, which is the same buttons in the same order.
- On a computer both side panels leave too, the agent list and the changes list. They come back as you had them: open, collapsed to the rail, hidden, and at the widths you dragged them to.
- Open a link that goes straight into theater and there was no layout on screen to remember, so leaving lands on your saved preferences instead.
- Theater is the only thing that takes the top bar away, and it all comes back when you leave.
- On an agent with terminals, theater shows only the part you are typing into, the agent or the terminal in front of its dock, with no tab strip and no dock strip. The link says which, so a reload or Back in theater shows the same part. On a phone the agent/terminals switch rides in the pill, so you can flip between the agent and the terminal you used last without leaving. On a computer nothing inside theater switches to the other part: leave theater, pick it, and go back in. Leaving puts the split, or whichever layout you had, back.

![Theater mode on a phone: the top bar is gone, the floating pill hovers over the terminal, and the keys and message box are still there.](/screens/phone-theater-pill.png)

**What stays.** Notice what did **not** leave: the terminal keys and the message box. Theater takes the chrome above the terminal, not the way you answer it. The compose box and the terminal keys stay exactly as you had them, and the pill carries the controls for them, so theater is as bare or as equipped as you want it.

- If you have asked to type straight into the terminal, the message box goes and the keys stay.
- Hide the keys as well and there is nothing below the terminal at all, and the pill brings them back: on a phone with its keys button, on a computer from the **Input** group in its `⋯`.

#### The pill

A small floating pill in the bottom-right corner is the only thing left over the terminal.

- **On a computer** it holds the way out, the macro picker, the changed-file count where there is one, and the pane's own `⋯`, so nothing you could reach before you went full screen has gone anywhere. The count leaves theater there and brings the changes list back, showing it even if you had hidden it.
- **On a phone** it is the flap that flew there: the theater button (now showing the way out), the macro picker, the changed-file count where there is one, the terminal-keys button, the agent/terminals switch on an agent with terminals, and the same `⋯`, opening the same menu it opened on the band. The changed-file count, the keys button and the switch are buttons beside the menu rather than rows in it.

The pill's `⋯` menu holds, in order, each group under its own heading (the way out of theater under **Theater**, the drill under **App**):

- the **Input** group (**Attach a file…**, the typing-surface switch, and on a computer the terminal-keys row; see [On your phone](#on-your-phone) for what the group holds)
- the pane's own actions
- the way out of theater, while the mode is on
- **Settings**, which drills into the app menu the cog carries

**Settings** is there because theater took the cog off screen with the rest of the chrome. The pane's `⋯` in the terminal header, where the cog is still a couple of buttons away, does not repeat it.

**The pill carries no tab status.** What the other tabs are doing is in the agents list and the tab strip, and one copy of that is enough. Theater on a phone hides both, so a tab you cannot see that needs you will be waiting when you come back out: that is the trade this mode makes for a screen with nothing on it but the terminal.

**Moving the pill.** It does not have to stay in that corner. Its leading edge is a grip.

- Drag the grip, with a finger or with the mouse, and the pill goes anywhere over the terminal, never further than its edges. There is nothing to hold first; it moves as soon as you do.
- A quick tap does nothing, so the buttons beside the grip keep working as buttons.
- If you would rather not drag, focus the grip and the arrow keys move it a step at a time.
- Where you leave it is remembered on that device, for every agent and every terminal.
- It is nudged back inside if you rotate the phone or shrink the window, and back to where you put it once there is room for it again.

#### Getting out

The button you came in by is not a way back, because the header it lives in is one of the things that left.

- **The pill.**
- **Leave theater mode** in the pill's own `⋯` menu, on every surface: the pill is the menu the phone has too.
- **`Escape`** on a computer, whenever you are not typing into the message box or the terminal itself.
- **Your browser's Back button**, on a computer. Back works because theater is part of the address: a link copied in theater opens in theater on whatever device you send it to.

Opening the file editor or the changes screen steps out of the mode, since neither of those is the terminal; closing them puts you back in it.

#### When a card covers the terminal

One thing does bring the top bar back on either surface without asking you to leave the mode: a card over the whole terminal. That happens when:

- another device took the driving seat,
- nobody is driving it yet (it is running in the background and this device has not claimed it), or
- the connection is being put back together.

The pill goes with the terminal underneath it, so the header returns with **Back** and the agent's name and you are never stuck staring at a card you cannot walk away from.

On a computer the top bar comes back the same way, with the **Theater** button in it, since a tablet gets that layout on width alone and has no Escape key to leave with; the side panels stay away. The moment the card goes, theater picks up where it left off. A brief spinner during a quick reconnect is not a card and changes nothing.

> [!TIP]
> Theater is remembered per row of the agents list, in that browser, so the agent you like watching full-screen comes back full-screen and the others do not. Switching between an agent's tabs or terminals never takes you in or out of it; a project or standalone terminal keeps its own. If another device takes over the terminal, theater ends and the memory goes with it, so you always come back to the full layout to decide what to do next.

### One writer, many watchers

Every device pointed at the same terminal sees the same output at the same time, but
**only one device at a time can type.** That device is the owner, and **you join as a
watcher unless nobody is driving.** If nobody holds the terminal, the first device to
look at it picks it up.

**A device drives an agent and its terminals together.** Whoever has the keyboard for an
agent's tab has it for every tab and every companion terminal of that agent, and taking
one over takes them all, so one device never drives the agent while another drives its
dev server. A terminal you open on an agent somebody else is driving is theirs too: you
watch it until you press **Take over**. A project or standalone terminal is driven on its
own.

![A card covering the terminal that says the agent is active on another device, with a Take over button.](/screens/take-over-card.png)

**The terminal UI is one of those devices.** When dux is [serving in the background of a running TUI](/docs/server-mode#serve-in-the-background-and-keep-the-tui), that terminal can hold a terminal or be a watcher like any browser, and the card names it as `the dux TUI`. It shows the same card, with the same words, when your browser is the one driving.

**A watcher sees the take-over card**, a full-pane card naming who has the keyboard ("Active on Chrome on macOS") with a **Take over** button. Hit it and dux resizes the PTY to your screen, repaints it fresh, and hands you the keyboard. Nothing is lost; the other device becomes the watcher.

Taking over reattaches, so it takes a moment and you will see "Reconnecting…" until the screen is really there. Behind the card, a watcher's terminal renders at the driver's true size with the text shrunk to fit, so the screen you take over is clean from the first frame.

> [!IMPORTANT]
> **A take-over interrupted by a network drop is not retried; press Take over again.** The reconnect that follows a dropped signal is a plain attach, on purpose, so a request you made minutes ago on a train cannot surface later and yank the terminal out from under whoever is using it now.
>
> The button always responds: if the attempt did not land, pressing it again starts a fresh one.

> [!IMPORTANT]
> **Losing the keyboard is sticky.** Nothing passive gives a terminal back, so a watcher left on screen never grabs a terminal somebody's flaky wifi dropped. Press **Take over**, reload, or navigate away and back, and it is yours. One tap is the whole cost of switching devices.

**When the driver goes away.** If the driving device disconnects, every card switches to **Running in the background** rather than naming a browser that has gone: the session kept running without its driver.

The card also keeps naming the device that had the keyboard even while your own connection is wobbling, so a flaky signal does not turn it into a vague "Active on another device".

#### Reconnecting never takes the terminal from anybody

Every automatic reconnect joins as a watcher unless nobody is driving. Taking a terminal away from a device that holds it is always something you press the button for.

- **The one exception** takes nothing from anyone. The device that lost the keyboard to a blip gets it back when it returns, provided the server is still holding its old, dead session and nobody else has claimed the terminal in the meantime.
- **If somebody has claimed it,** you come back as a watcher with the card up, and one tap puts you back in the driver's seat.
- **If your page cannot confirm it is talking to the very same dux** it started against, because dux was restarted while you were away or the check could not get through, you land as a watcher and press the button. Coming back automatically is only safe when the old session is provably still the one you left.
- **A page in the background** stays a watcher until it is in front again.
- **If your own connection has given up entirely,** the card steps aside for the **Connection lost** notice and its **Reconnect** button.

### Clipboard: the classic terminal model

The web terminal copies and pastes the way a real terminal does, no menu required:

- **Select to copy.** Highlight text and it lands on your clipboard immediately, with a "Copied to clipboard" toast. Governed by the `ui.copy_on_select` preference (on by default), which you can flip in **Preferences** (the cog menu).
- **Right-click to paste** (mouse or pen). On plain HTTP, where the browser blocks clipboard reads, dux nudges you toward `Ctrl+v` instead. On a touch screen the same press-and-hold gesture belongs to selection, which is the next section.
- A fixed set of chords works too, and it is not user-configurable: `Ctrl+Shift+c`, `Ctrl+Insert`, or `Cmd+c` to copy, and `Ctrl+v`, `Ctrl+Shift+v`, or `Cmd+v` to paste, with `Ctrl+c` staying SIGINT.
- **Paste an image with the keyboard and dux uploads it**, saving it on the server and pasting its **path** into the prompt. Text paste is untouched, and `Ctrl+Shift+v` (`Cmd+Shift+v`) forces the text when the clipboard carries both. Keyboard chords only: right-click paste can never carry an image. See [Dropping and pasting files onto an agent](/docs/dropping-files).

There is deliberately no right-click context menu, because it would fight the paste gesture.

> [!TIP]
> When the app inside the terminal grabs the mouse, a plain drag goes to that app instead of selecting text. Hold **Shift** (Linux and Windows) or **Option** (macOS) while dragging to force a local selection. dux pops a one-time hint the first time this bites you.

### Programs that write your clipboard

Your agent can also write your clipboard directly through an `OSC 52` escape sequence. That write lands on **your** browser's clipboard, not the server's, and is governed by the `clipboard_passthrough` capability (`"focused"` by default; see [Terminal capabilities](/docs/terminal-capabilities)).

- **Time limit.** The browser accepts the write only within a few seconds (about 5 in Chrome) of your last key press or click in the page. That limit is the browser's rule, not a dux setting.
- **A copy that arrives later** shows a message with the start of the text and a **Copy** button. Press it and the whole text lands on your clipboard.
- **If the browser refuses even that,** the message offers **Show text**, which opens the text already selected so you can copy it yourself.

### Selecting text with a finger

On a phone or tablet, **press and hold** on the terminal. The word under your finger highlights, and dragging extends the selection.

- While your finger is down, the terminal stops following new output, so the text you are selecting stays where it is. It catches up with the output when you lift.
- Drag past the top or bottom edge and the terminal keeps scrolling for as long as you hold there.
- **Lift your finger and it is copied**, under the same `ui.copy_on_select` preference.
- Press on blank space and nothing highlights; dux tells you there is nothing to copy there.
- If the terminal changes under your finger (the screen resizes, or a full-screen app opens or closes), nothing is copied and dux says so; press again.
- The highlight stays on the text you copied until the next tap clears it. If the terminal was following output, that text may scroll out of view as it catches up, highlight and all.
- A press is not a tap, so the keyboard stays down and does not cover what you highlighted.

A long press **always** selects locally, even when a full-screen agent has taken the mouse, so this is the touch version of the Shift and Option hatch above.

There are no drag handles, so a selection cannot be adjusted once you lift (press again to redo it).

### Drag a file in, or paste one

Drag a screenshot (or any file) from your desktop onto the terminal, or paste an image, and dux saves it on the server and pastes its path into the prompt.

- **Dropped on an agent,** it goes to that agent's upload folder (`.dux/uploads` in its worktree), invisible to git and deleted along with the agent.
- **Dropped on a terminal,** it goes to the folder that terminal is in right now.
- **On a phone,** pasting an image puts its path into your compose draft.
- Only the device holding input can drop or paste.

How names are kept or cleaned, and how the pasted path is written for each provider, is in [Dropping and pasting files onto an agent](/docs/dropping-files).

### Shift-Enter for a soft newline

In the browser, **Shift-Enter inserts a newline instead of submitting.** Plain Enter still
submits. This is web-only (the TUI cannot tell the two apart), and it never fires
mid-composition, so it will not mangle a CJK input method's confirm keystroke.

## Companion terminals

There are three kinds of plain shell:

- A **companion terminal** runs in an agent's worktree, for tests, git, or tailing a log while the agent works. It lives in the agent's **terminal dock**, under the agent, and has no row of its own in the sidebar: the agent's row stands for it. See [The terminal dock](#the-terminal-dock) below.
  - Open one from the agent's `⋯` menu, from the dock's **+**, or from **New terminal for an agent…** in the launcher's **▾** menu, which asks which agent: every agent is listed under its project, standalone ones last, with a search box to narrow them down.
- A **project terminal** opens at the project's repo root with no agent attached. It is a row of the sidebar's one list, sorted with your agents. It is the escape hatch when dux will not do something for you remotely, even over Tailscale with no local terminal in sight.
  - Spawn one from the project's `⋯` menu ("New terminal at the project root").
  - It shows up in the Task Manager and is destroyed on close.
- A **standalone terminal** opens in your home directory with no agent and no project, so you can reach for one before you have added a single project. It is a row of the one list too.
  - Open one from the **Terminals** group of the launcher's **▾** menu, or from the cog menu's **New** submenu.
  - Its row shows the directory it opened in, shortened with `~` and marked with the `✷` standalone star, so the sidebar search finds it by path.
  - The other two show the `↳` arrow and their owner. The star always means the same thing: this one lives in your folder, not a working copy dux manages.

![New terminal for an agent: a search box over every agent, grouped under its project with the standalone ones last, each row naming its state and the folder the terminal will open in.](/screens/new-terminal-for-agent.png)

![A standalone agent in the sidebar wearing the star over the folder it runs in, with the changes panel saying the folder has no git repository.](/screens/sidebar-standalone.png)

### The terminal dock

When an agent has a terminal of its own, the center pane splits: the agent and its tab strip on top, the dock below with a strip of its own, one pill per terminal.

![An agent on top of the center pane and its two terminals docked below it, each terminal with its own pill in the dock's strip, and the layout buttons at the strip's right end.](/screens/web-terminal-dock.png#full)

- **Pick a terminal** by clicking its pill. Its glyph pulses and bounces while something runs in it, like a tab pill's. Drag a pill along the strip to reorder the dock; the order lasts until dux restarts. A pill's `⋯` closes that terminal. When the terminal in front ends, the dock shows the one to its right, or to its left when it was the last; with none left the dock goes and the agent shows alone.
- **Type into either part** by clicking into it. The part you are not typing in turns grey and fades, so you can always tell where your keystrokes go; its strip stays as it is.
- **Three layouts.** The dock strip's right end carries **Hide terminals** and **Maximize terminal**. Either one gives the whole height to one part and leaves the other its strip, with a button to bring the split back. Dragging the divider past a part's minimum folds that part away the same way. Each agent reopens in the layout you last left it in on that browser, and an agent you open with its terminal in front opens at least in the split.
- **Strips show together.** Both strips appear once the agent has two tabs or two terminals, or while **Always show tab strip** is on. Otherwise neither does: the divider is a thin grip band, and a small floating pill on its right end carries the layout buttons and a **+** that asks whether you want a new agent tab (on the agent's default provider; pick another from the agent's `⋯`) or a new terminal. With the terminal maximized, the pill also carries a dot for what the agent is doing: green and pulsing while it works, cyan and blinking when it needs you.

![The same agent with the strips hidden: a thin grip band between the agent and its terminal, and the floating pill at the band's right end.](/screens/web-terminal-dock-hidden-strips.png#full)

**On a phone** there is no room for both, so an agent's screen shows the agent or its terminals, never both, with only that part's strip. The **agent/terminals switch** in the action flap moves between them (see [The action flap](#the-action-flap)), and Back moves back. Each agent reopens on whichever part you last left it on in that browser, and on the agent the first time. A lone tab beside a lone terminal shows no strip at all; make a second one from the flap's `⋯` with **New agent tab…** or **New terminal**.

![An agent's terminal on a phone: the dock strip with two terminal pills under the slim header, and the action flap carrying the agent switch beside the menu.](/screens/phone-terminal-view.png)

**An agent you stopped while its terminals keep running** opens on its terminal, maximized, and nothing starts it behind your back. A band in place of its tab strip says it is not running and carries **Start**, which starts its first tab and puts the layout back the way you had it. If its last run ended badly, the band says how, with its last output a click away. While the agent shares the screen with its dock, its card is a short one: a line and **Start**. On a phone it opens on its terminals, and the switch takes you to its card. Such an agent stays in the main list until its last terminal ends, and if you are looking at it when that happens it stays stopped, on its idle screen with **Start session**.

Every terminal row's `⋯` menu also carries **Open editor here** and **Open editor in new
tab**, rooted at the directory that terminal opened in. A terminal that belongs to an
agent opens that agent's editor instead. See [The code editor](/docs/web-editor).

> [!WARNING]
> Killing a companion terminal **destroys** it. Agents detach when you kill them;
> terminals do not.

Closing a companion terminal is guarded the way stopping its agent is: while another
device has any of the agent's tabs or terminals open, dux refuses and names that device,
and you can go ahead anyway from the dialog. Closing or stopping one of the agent's tabs is
guarded the same way. A project or standalone terminal is refused
only over a device that has that terminal open.

Nothing closes a standalone terminal for you. Removing a project closes that project's
terminals and deleting an agent closes that agent's; neither touches a standalone one. It
ends when you close it, or when dux shuts down.

## Messages

Progress and results arrive as small messages at the bottom of the screen, and **they clear themselves.** Dismiss one early by swiping it away or with its close button.

How long a finished one stays depends on how much it matters:

- **A success or a note** uses your `ui.status_clear_seconds` window (six seconds by default).
- **A warning** stays three times that.
- **An error** stays four times that.
- Setting `ui.status_clear_seconds = 0` turns auto-clearing off.

### Pausing to read

A message that clears itself shows how long it has left as a thin bar along its bottom edge, in the message's own color, draining toward the left.

- **Point at the stack** and every countdown stops: the stack opens up, the bars stop and dim, and they pick up where they left off once the pointer leaves.
- **On a phone,** tapping the stack does the same, and it stays paused until you tap somewhere else.
- **Holding a finger or the mouse button down** on a message also stops the clock for as long as you hold it.
- **The countdown waits** while the tab is in the background, too.
- **Reduced motion:** if your system asks for it, the bar steps down once a second instead of gliding.

### Spinners and messages that stay

A message about work still in flight is a spinner: no close button and no swipe, replaced in place by its own result the moment there is one. If the connection drops mid-operation and no result arrives, the spinner gives up after a minute.

A small number of messages stay until you dismiss them, and only for two reasons: **you have to go and do something outside the message**, or something may have been lost or left half-finished.

- A file that was saved but never handed to the agent waits, because the message holds the only copy of where it went.
- A worktree that could not be removed waits, because it is still on disk.
- A failed pull does not wait.

### Stacking

If several arrive at once they stack, five deep on a desktop and three on a phone, and the rest queue behind them. They stack in the workspace tab: the standalone editor tab shows only what you do in it.

> [!NOTE]
> After a reconnect you still see work that is **still running**, plus any result from the
> last thirty seconds. Older results are not repeated.

## Macros

The **Macros…** button in a terminal pane's header (in the action flap on a phone) drops
prewritten prompt snippets into that terminal without submitting them, so you review and
press Enter yourself. It is greyed out while a take-over card covers the terminal. Each
macro is scoped to agents, terminals, or both, and multi-line macros are inserted so they
do not submit line by line. Full details in [Managing Macros](/docs/macros).

## On your phone

Below tablet width, the web UI becomes a **hub-and-spoke** shell built for one thumb.

### The screens

![The dux home screen on a phone, listing agents with their state: one waiting on you, the rest working.](/screens/phone-hub.png)

- **Home** is the hub: your projects and sessions with the same `⋯` menus as desktop, the same **New agent…** launcher along the bottom, and a **cog** button that opens the app menu as a bottom sheet (submenus drill down in place, with a back arrow). The launcher's **▾** opens its list as a bottom sheet too. Tap a session to jump into its terminal.
- **The terminal screen** is a full-screen terminal with a slim bar on top (Back, the agent's name over its assistant, branch and project, and an optional PR chip) and the tab strip when the agent has more than one tab.
- **An agent's own terminals** are on the same screen, one part at a time: the agent/terminals switch in the flap shows them in place of the agent, with the dock strip when there are two or more. The header, the menu and the count stay the agent's, and the menu adds a **Terminal** group for the terminal in front of you: **Open editor in new tab** and **Close…**. The message box says what it is talking to: "Write a message to the agent…" over the agent, "Type a command…" over a terminal.
- **A project or standalone terminal** gets the same screen and the same flap, minus the two things a terminal does not have: there is no changed-file count and no PR chip. Its `⋯` opens the terminal's own menu, with the **Input** group, then a **Terminal** group holding **Open editor in new tab** and **Close…**, then the same **Settings** drill.
- **The changes screen** is the full Changes pane. Tapping a file opens its diff in the standalone editor in the same browser tab, and Back comes back to the Changes screen.

### The action flap

The actions hang off the band on the right, in a small tab-shaped flap over the terminal. It is the whole reason the header can stay as quiet as it is.

![The phone agent screen: the slim top bar naming the agent, its project and its assistant, and the action flap hanging off the band with theater mode, the macro picker, the changed-file count, the terminal-keys button, the terminals switch with its count and the pane's menu.](/screens/phone-terminal-flap.png)

It holds:

- theater mode
- the macro picker
- the changed-file count (agents only)
- the terminal-keys button, which hides and shows the key row, while this device is typing into the terminal
- the **agent/terminals switch**, on an agent with at least one terminal. Over the agent it shows the terminal icon and how many terminals there are; over a terminal, the agent icon. The icon tells you what the other part is doing: green and pulsing while it works, cyan and blinking when the agent needs you (needs-you wins), plain otherwise. Tap it to show that part and type into it.
- the session's `⋯` menu, which carries the **Input** group, the agent's actions (**New agent tab…** and **New terminal** among them) and a **Settings** drill into the app menu

The flap takes no room from the terminal, so theater does not take it away: it lifts off the band and flies to the floating pill instead (see [Theater mode](#theater-mode-one-pane-no-chrome)).

### Typing on a phone

An **accessory bar** sits above the soft keyboard with the keys a phone keyboard lacks: Esc, Tab, a sticky Ctrl and Alt latch, arrow keys, PgUp/PgDn, and a dedicated **⇧↵** key for the soft newline. Touch targets are sized generously. The soft keyboard is handled by the browser shrinking the layout, so the bar sits flush on top of it, and there is no fullscreen mode to fight with.

![The phone typing bar: two rows of terminal keys above a message box with a send button.](/screens/phone-compose-bar.png)

Below it sits the **compose bar**: a real text box where you type the message and hit **Send**. This is where your keyboard's autocorrect, swipe typing, and voice input actually work.

- **Enter** adds a newline instead of submitting.
- **Send** delivers the whole message and presses Enter for you.
- **An empty Send** is a plain Enter, for confirming menus and prompts.
- **Tapping the terminal** drops you into the compose box.
- **A refused send** keeps your draft and tells you why.
- The box holds the keyboard focus the whole time, so the terminal's caret stays a solid block rather than hollowing out.

**A physical keyboard** types into the compose box. The keys a text box has no use for, **Esc** and **F1** through **F12**, go straight to the terminal, so Esc on a tablet's keyboard case interrupts a running agent exactly like the accessory bar's Esc key.

- A few F-keys are grabbed by the browser itself before any page can see them, F12's developer tools being the classic, so those may trigger both.
- Modified presses stay with your browser (Ctrl+C keeps meaning copy, not SIGINT).
- For every keystroke on the wire, the Direct typing surface is one tap away.

### When the compose bar appears

Whether the compose bar appears is the `ui.compose_bar` setting, a Preferences row with three values:

- **Automatic** (the default) asks your browser whether you point at the screen with a finger, and uses the answer as the starting point. Your own choice from the typing-surface switch wins from then on.
- **Always** and **Never** are for the device dux guesses wrong on.
- **Never** restores typing straight into the terminal. On a touch device it keeps the terminal keys, since a finger still cannot produce Esc or a Ctrl chord and there is no switch under **Never** to bring them back with. Only the message box goes.

> [!IMPORTANT]
> **Width decides the layout; your pointer decides which typing surface you start with.**
>
> A tablet in landscape gets the desktop panes *and* the accessory keys and compose box. A mouse on a narrow window starts with neither. Rotating a tablet never swaps your typing surface out from under you.

### The typing-surface switch

What the browser cannot see is a keyboard case, so there is a **typing-surface switch**: **Type directly in the terminal**, and **Use virtual input** to come back. Both live in the **Input** group of the pane's own `⋯` menu (see below), always: the switch never moves, whatever is left under the terminal.

**Your choice wins, on any device, in both directions.** The pointer only decides where you start.

- Ask for the message box on a laptop and you get it, keys and all, and while it is up your keystrokes go into the box rather than straight to the terminal.
- Switching to Direct takes the **message box** away and leaves the terminal keys where they are. A phone keeps Esc, Tab and the Ctrl latch while every keystroke goes straight to the terminal: tap the virtual **Ctrl**, then press the letter on your keyboard.
- Hide the terminal keys too and there is nothing under the terminal at all, and the **Input** group is still where you bring things back from.
- Your choice sticks across reloads and survives folding a convertible or unplugging a mouse, until you change it.
- It is not a setting: it is remembered on that device and changes nothing in your config.
- It appears only under **Automatic**, since **Always** and **Never** have already decided. Switching the preference to **Always** or **Never** does not erase it either: the choice stays remembered in that browser, and comes back the moment you put the preference on **Automatic** again.

While the message box is up it really is the typing surface. Clicking into the terminal puts your cursor back in the box, right-clicking pastes the clipboard into your draft instead of sending it, and the terminal is skipped when you tab backwards out of the pane. Switch to Direct and all of that goes back to the way a plain terminal behaves.

### Terminal keys

The message box and the terminal keys are two separate switches, so all four combinations are yours to pick: both, the box alone, the keys alone, or neither.

Terminal rows are precious on a phone, so the terminal keys are hideable. On a phone, a keys button in the action cluster, between the changed-file count and the `⋯`, hides and shows them, in the flap and in the theater pill alike. On a computer the same toggle is a row, **Hide terminal keys** or **Show terminal keys**, in the **Input** group of the pane's own `⋯`. Either way it stays on offer while the keys are hidden, so hiding them never takes away the way to bring them back. It is backed by a preference that is also a row in Preferences.

The terminal-keys preference lives on the server, so it follows you to every device, and the keys travel with your pointer: hiding them from your phone also hides them on the tablet you pick up next.

For the top bar the answer is [theater mode](#theater-mode-one-pane-no-chrome), not a preference. It takes the header and the tab strip together, and it hands you the way back at the same time: the floating pill, and **Leave theater mode** in the menu it opens. A preference that hid the same chrome would be a second way to reach the same place, with no way back of its own.

### The Input group

The input has no menu of its own under the terminal: nothing hangs off the message box or the key row but the box, its send button and the keys. Its controls live in the **Input** group at the top of the pane's own `⋯` menu: the flap's on a phone, the pane header's on a computer, the floating pill's in theater. It is on screen whatever the pane is doing, so you can always reach it:

- **Attach a file…** (see [Dropping and pasting files](/docs/dropping-files)).
- **Type directly in the terminal** or **Use virtual input**, whichever way the switch would take you, wherever the preference is **Automatic**.
- **Hide terminal keys** or **Show terminal keys** on a computer, wherever the keys are on offer for the device. A phone has that toggle as a button in the action cluster instead, so it is in one place at each width.

The group is there only while this device is driving the terminal, and a pane that offers none of those rows shows no group at all. Every group in the menu carries a heading, this one included.

## Install it like an app

Your browser will offer to add dux to your home screen or dock, where it opens standalone
without browser chrome. Offline it shows a small "dux is unreachable" page and nothing else;
the app itself always loads fresh. When you lose the connection mid-session, dux grays the
app out behind a "Reconnecting…" overlay and reconnects when it can, without navigating
away, so you land right back on the screen you were looking at.
