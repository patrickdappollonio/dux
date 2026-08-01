---
title: Ordering the sidebar
description: Sort your agents and terminals by activity, name, or recency, or hand-place them exactly how you like, by dragging in the browser or with the move commands in the terminal UI. Terminals ride the same sort as agents.
group: Guides
order: 12
---

The left sidebar is home to every agent and every terminal. Once that list grows
past a handful, you'll want it in *some* order, and dux gives you two ways to get
there: pick a sort mode and let dux keep the list arranged, or take the wheel and
place things by hand.

Whichever you choose applies to **both** agents and terminals. There's one sort
control, not one per kind.

## The sidebar, top to bottom

Agents sit up top; terminals get their own section below them. In the terminal UI those
are two panes; on the web they're two labelled groups (the Terminals group is
collapsible and lives right under the agents). Dormant agents fold away into a
collapsible "Inactive" tail so finished work isn't in your face.

```text
▾ Agents
  ● auth-refactor        ⎇ #42   Working
  ▍ docs-pass                    Typing
  ◐ billing-fix                  Detached
  ▸ Inactive (2)
▾ Terminals
  ● cargo test    ↳ auth-refactor@dux   Running
  ○ zsh           ↳ project              Idle
```

A terminal's second line names its owner as `agent@project` (or just the project,
for a project terminal), so you always know whose shell you're looking at.

## Sorting

Reach for the sort control, the sort dropdown on the web or the `sort-agents`
command in the TUI's command palette (it cycles through the modes), and choose:

- **Active first**: anything working or waiting on you floats to the
  top; everything else keeps its order. This is the default, and the one you'll
  live in.
- **Recently updated** / **Recently created** — newest at the top.
- **By name** — alphabetical by title, or by branch when an agent is untitled.
- **Manual** — your exact hand-placed order, left untouched by dux.

Terminals follow the same choice. Sort by name and the terminals sort by their
command too; pick manual and each holds the spot you put it in.

## Hand-ordering

Sometimes you just want *this* one above *that* one. Two ways, depending on where
you are.

**In the browser, drag it.** Grab a row and drop it where you want. Agents
reorder among agents and terminals among terminals; a terminal never jumps up
into the agent list, and vice versa. The instant you drag, the sort flips to
**manual**, because a computed sort would only snap your row back.

**In the TUI, use the move commands.** There's no dragging in a terminal, so the
command palette carries the equivalent: `move-agent-up`, `move-agent-down`,
`move-agent-top`, and `move-agent-bottom`, plus the matching `move-terminal-*`
set. Each moves the selected row and, exactly like a drag, switches the sort to
manual. The terminal commands appear only when you actually have a terminal to
move; the agent ones are always there.

> [!NOTE]
> Reordering shares its mode with sorting, so hand-placing a single agent flips
> the whole list, agents and terminals, to manual. Cycle the sort back to "active
> first" whenever you'd like dux to take over the arranging again.

## What sticks around

Your **agent** order is saved. Quit dux, reopen it, and your agents come back in
the order you left them.

Your **terminal** order is not, but only because terminals themselves don't
survive a restart. A terminal is a live shell; when dux stops, it's gone, so
there's nothing left to hold an order for. Spin up new terminals next time and
they'll start in creation order until you rearrange them again.
