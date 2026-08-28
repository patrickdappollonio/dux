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

Agents sit up top; terminals get their own section below them. In the terminal UI those
are two panes; on the web they are two labelled groups, and the Terminals group is
collapsible. Dormant agents fold away into a collapsible "Inactive" tail. On the web the
Terminals group sits above that tail; in the terminal UI the tail closes the Agents pane
and Terminals is its own pane below. Every row is two lines: the name, then who it
belongs to and what it is doing. The terminal UI looks like this (the spinner glyph
animates while an agent works):

![The terminal UI sidebar: three active agents, one of them running three tabs, a collapsed Inactive tail, and a Terminals section below with one terminal.](/screens/tui-agent-list-two-line.png)

A terminal's second line names its owner as `agent@project`, or just the project for a
project terminal. A standalone terminal has no owner, so it names the directory it
opened in instead, shortened with `~`. Whichever it shows is what the sidebar search
matches on.

Typing in the filter narrows the list to the rows that match, with the matching text
picked out, and the pane title counts what survived.

![The same sidebar filtered by the word retry: the title reads Agents 2 of 5 and only the two matching agents remain.](/screens/tui-sidebar-search.png)

> [!NOTE]
> The `✷` star means the same thing wherever you see it: this one lives in your own
> folder, not in a working copy dux manages. A
> [standalone agent](/docs/creating-agents#running-an-agent-in-a-folder-you-already-have)
> wears it over its folder and a standalone terminal wears it over its directory.
> Anything owned keeps the `↳` arrow pointing at its owner.

## Sorting

Use the sort dropdown on the web, or the `sort-agents` command in the terminal UI's
command palette, which cycles the modes:

- **Active first**: anything working or waiting on you floats to the top, and
  everything else keeps its order. This is the default.
- **Recently updated** and **Recently created**: newest at the top.
- **By name**: alphabetical by title, or by branch when an agent is untitled.
- **Manual**: your exact hand-placed order, left untouched by dux.

Terminals follow the same choice. Sort by name and terminals sort by their command
too; pick manual and each holds the spot you put it in.

## Hand-ordering

**Drag it, on either surface.** Grab a row with the mouse and drop it where you want,
in the browser or in the terminal UI. Agents reorder among agents and terminals among
terminals; a terminal never jumps into the agent list, and vice versa. Agents move
freely across projects, because the order is one list rather than one list per project.

In the terminal UI the drag starts once you leave the row you pressed on, so an
ordinary click still just selects, and a line marks the gap the row will drop into
while you carry it. Release to drop it there; drop it back where it started and
nothing happens. Dormant agents in the Inactive tail are not places to drop something,
and neither is anything a search filter is currently hiding: the rows you can see are
the rows you can drop onto, and the list does not scroll while you drag. Rows the
filter hides keep their places in the order underneath.

**In the terminal UI you can also move a row without a mouse.** The command palette
carries `move-agent-up`, `move-agent-down`, `move-agent-top`, and `move-agent-bottom`,
plus the matching `move-terminal-*` set. Each moves the selected row. The terminal
commands appear only when you have a terminal to move; the agent ones are always there.

> [!IMPORTANT]
> Dragging or moving a row switches the sort to **manual**, for the whole list, agents
> and terminals together, whichever surface you did it on. Otherwise a computed sort would snap your row straight back.
> Cycle the sort to "active first" to hand the arranging back to dux.

## What sticks around

Your **agent** order is saved. Quit dux, reopen it, and your agents come back in the
order you left them.

Your **terminal** order is not, because terminals themselves do not survive a restart.
A terminal is a live shell; when dux stops, it is gone. New terminals start in creation
order until you rearrange them.
