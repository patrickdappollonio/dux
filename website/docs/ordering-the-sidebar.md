---
title: Ordering the sidebar
description: Sort agents and terminals by activity, name, or recency, or hand-place them by dragging with the mouse on either surface.
group: Guides
order: 12
---

The left sidebar holds every agent and every terminal. You have two ways to arrange
it: pick a sort mode and let dux keep the list in order, or place things by hand.

> [!NOTE]
> There is one sort control, not one per kind. Whichever mode you pick applies to
> agents and terminals alike.

## The sidebar, top to bottom

Dormant agents fold away into a collapsible "Inactive" tail at the bottom.

- **Both surfaces:** one list, titled **Agents & Terminals** with a count of each. Project
  and standalone terminals are rows of it, sorted together with the agents. An agent's own
  terminals are not rows: they dock under the agent in the center pane (see
  [Terminals beside their agent](#terminals-beside-their-agent), and in the browser
  [the terminal dock](/docs/web-workspace#the-terminal-dock)). An agent you stopped while
  one of its terminals keeps running stays in the list, out of the Inactive tail, until its
  last terminal ends, and terminals are never in the tail.
- **Every row** is two lines: the name, then who it belongs to and what it is doing.

The terminal UI looks like this (the `※` before a project name marks the project, and the
spinner glyph animates while an agent works):

![The terminal UI sidebar: three active agents, one of them running three tabs, and a collapsed Inactive tail.](/screens/tui-agent-list-two-line.png)

A terminal's second line names its owner: the project for a project terminal, or
`agent@project` for an agent's terminal in the browser's list. A standalone terminal has no
owner, so it names the directory it opened in instead, shortened with `~`. Whichever it
shows is what the sidebar search matches on.

Typing in the filter narrows the list to the rows that match, with the matching text
picked out, and the pane title counts what survived out of how many, for agents and
terminals each.

![The same sidebar filtered by the word retry: the title reads Agents & Terminals, 2 of 5 agents and 0 of 0 terminals, and only the two matching agents remain.](/screens/tui-sidebar-search.png)

The `✷` star means the same thing wherever you see it: this one lives in your own folder,
not in a working copy dux manages. A
[standalone agent](/docs/creating-agents#running-an-agent-in-a-folder-you-already-have) wears
it over its folder and a standalone terminal wears it over its directory. Anything owned
keeps the `↳` arrow pointing at its owner.

![The terminal UI with a standalone agent selected: its row carries a star over the folder path, and the changes panel says the folder has no git repository.](/screens/tui-standalone-star.png)

## Sorting

On the web, use the **Sort** dropdown above the agent list. In the terminal UI, run the
`sort-agents` command from the command palette, which cycles the modes. The modes:

- **Active first**: anything working or waiting on you floats to the top, and everything else
  keeps its order. This is the default. The Inactive tail is ordered most recently active
  first.
- **Recently updated** and **Recently created**: newest at the top.
- **Name (A to Z)**: alphabetical by the agent's name, or by its branch when it has no name,
  or by its folder's name for a standalone agent.
- **Name (Z to A)**: the exact reverse. The terminal UI's cycle includes it; the web dropdown
  does not offer it, but shows it (checked) while the terminal UI has it set.
- **Manual**: your exact hand-placed order, left untouched by dux. The terminal UI's cycle
  does not include it, since it is what you get by placing a row by hand. The web dropdown
  lists it as **Manual order**.

Terminals follow the same choice, on both surfaces, sorting among the agents. By name, a
terminal uses the name it was opened with (`Terminal 3`), never what is running in it, so a
program starting does not move the row. By activity, a terminal running something counts as
busy. By updated, its last output counts. In manual order it keeps the place you put it in
among the agents until dux restarts.

> [!NOTE]
> The web's cog menu also has **Sort agents by** (Recently updated, Created, Name). That is
> not a sort mode: it is a one-shot reorder that rewrites each project's hand-placed order
> once. It has no terminal UI counterpart.

## Hand-ordering

**Drag it, on either surface.** Grab a row with the mouse and drop it where you want,
in the browser or in the terminal UI. Agents move freely across projects, because the
order is one list rather than one list per project. A project or standalone terminal row
can be dropped between agent rows, since it is a row of the same list.

In the terminal UI:

- The drag starts once you leave the row you pressed on, so an ordinary click still just
  selects.
- A line marks the gap the row will drop into while you carry it.
- Release to drop it there; drop it back where it started and nothing happens.
- Dormant agents in the Inactive tail are not places to drop something, and neither is
  anything a search filter is currently hiding: the rows you can see are the rows you can
  drop onto. Rows the filter hides keep their places in the order underneath.
- The list does not scroll while you drag.

**In the terminal UI you can also move a row without a mouse.** The command palette
carries `move-agent-up`, `move-agent-down`, `move-agent-top`, and `move-agent-bottom`,
plus the matching `move-terminal-*` set. Each moves the selected row. The terminal
commands move a project or standalone terminal's row and refuse, saying so, when the
selected row is not one. The terminal commands appear only when you have a terminal to
move; the agent ones are always there.

> [!IMPORTANT]
> Dragging or moving a row switches the sort to **manual**, for the whole list, agents
> and terminals together, whichever surface you did it on. Otherwise a computed sort would snap your row straight back.
> Cycle the sort to "active first" to hand the arranging back to dux.

## What sticks around

Your **agent** order is saved. Quit dux, reopen it, and your agents come back in the
order you left them. So is the order of each agent's [provider tabs](/docs/agent-tabs#reordering-tabs).

Your **terminal** order is not, because terminals themselves do not survive a restart.
A terminal is a live shell; when dux stops, it is gone. New terminals start in creation
order until you rearrange them. The same goes for the order of an agent's terminal dock.

## Terminals beside their agent

An agent's own terminals are not rows of the list: they dock under the agent in the center
pane, on both surfaces, each with a pill in the dock's strip, and the agent's row stands for
all of them. The browser's dock is described in [the terminal dock](/docs/web-workspace#the-terminal-dock);
this is the terminal UI's, where the pills are numbered:

![The terminal UI with an agent on top of the center pane and its two terminals docked below it, each with a numbered pill, the agent | split | term control at the right of the dock's strip, and the list titled Agents & Terminals.](/screens/tui-terminal-dock.png)

Reorder them along the dock by dragging a pill with the mouse, on either surface, or in the
terminal UI with the `move-tab-left` and `move-tab-right` palette commands while the dock
has focus. The agent's first tab keeps its place as the tab that starts whatever order its
strip is in, and nothing about the dock's order is kept across a restart.
