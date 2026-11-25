# Browser UI

React, Vite and Tailwind, dark-only. Paths in `lib/`, `components/` and `hooks/` are under `src/`; the server's rules are in `../AGENTS.md`.

## Components and look

- Style through the shadcn/base-ui token CSS variables, never a hardcoded color.
- Reuse a `components/ui/*` primitive before writing one; a hover hint uses the shared tooltip, never `title=`.
- A piece the UI repeats is one shared component that owns its look, its touch floor and what a screen reader hears; a caller passes words and named choices, never a class or a style.
- Touch targets are at least 40px. A control may be smaller along one axis only when every neighbour on that axis is inert or measurably clear, and a comment at the call site names them.
- A control's height is set explicitly, never inherited from padding or content; controls that sit together share one height token.
- An action cluster is all outline buttons at one height; it may carry one filled primary action that commits the user's work, and a control that navigates may be quieter, with the reason written at the site.
- On a phone prefer icon-only controls, except where the text is data (a count, a size).
- Every git status marker renders through `FileStatusIcon`.
- The tests titled `USER-LOCKED: working glyph bounces with the pulse and settles at rest` guard a product decision: change that behaviour only on an explicit request for that specific change; editing such a test to make it pass is changing the decision.

## Menus and actions

- A row or surface with more than one action gets one `⋯` menu, through `EllipsisMenuTrigger`; it exists for as long as its surface, never renders empty, and appears once per surface.
- Every menu item has a leading icon, and an item that opens a dialog or asks for confirmation ends in `…`.
- Destructive menu items are neutral in color; the agent menu's Delete is the one red item.
- A control that must work when a surface is gone lives in the menu of a surface that outlives it.
- Each pane surface has one top menu (the flap's `⋯` on a phone, the pane header's `⋯` on a computer, the floating pill's in theater), and the input's controls live in its labelled Input group only.
- The sidebar row's menu and the floating pill's menu are the same body as the pane's top menu; an anchor changes where it opens, never what the pane put in it.
- Every menu group has a heading, and a group with nothing in it renders no heading and no separator.
- `lib/appMenu.ts` is pure data that both the desktop flyout and the phone's bottom sheet render.
- Creating lives in the launcher corner, the New dialog and the cog's create submenus, all built from `lib/creationMenus.ts`; list sections carry no creation buttons.
- The two second doors to creation are "Add standalone agent…" in the New agent picker, and the `+` on an agent's tab strips with the New tab… button.
- A preference is a row in the settings descriptors shown in the Preferences dialog, never a menu item; one needing a side effect gets its own write target routed to its dedicated endpoint.

## Phone and pointer

- `lib/shellLayout.ts` alone decides the phone or computer layout, read through `useIsMobile` and the `md:`/`max-md:` classes; a per-device choice from the app menu wins over detection.
- Ask what the hardware can do with `isPhoneDevice`, never the layout.
- The pointer decides only whether typing starts in the compose bar, and a per-device choice wins over it; detection changing moves the default only.
- Theater is the one way to hide the phone's top bar; nothing else hides it.
- In theater on a phone, a hidden tab that needs attention has no on-screen signal.
- The floating pill carries controls that act, never status; its `⋯` is the pane's whole menu.
- Never add the Fullscreen API, Keyboard Lock or a fullscreen toggle; every directive of the viewport meta in `index.html` is load-bearing.

## Typing into terminals

- The compose bar, wherever it is up, is the typing surface for the input owner; xterm is not typed into directly.
- The message box and the terminal key row are independent and every combination of them is valid; the key row carries terminal keys only, never a control that changes a mode.
- One input area serves an agent and its terminals and sends through the typing target's own connection; an input that completes later lands where it was aimed when it started.
- Non-owners get no typing surfaces, and every write, macros included, goes through the ownership gate; a refusal never reports that something was sent.
- In the terminal pane, selecting copies and right-click pastes with no context menu; clipboard chords are classified by physical key (`ev.code`), never `ev.key`.
- A touch gesture forwarded to a program is a replayed DOM event that xterm encodes; dux never encodes a mouse report itself.
- A dropped, pasted or picked file is saved and its path pasted by the browser over its own gated connection; the upload route never writes to a terminal.

## Notifications

- `lib/notify.ts` is the one place that raises a toast.
- A toast is sticky only when the user must act elsewhere to recover or something may be lost; keep sticky toasts few.

## Position and reloads

- The URL hash names the whole position: a move within a position replaces, a move to a different position pushes, so Back always visibly moves.
- In-app back and up controls push a named destination, never `history.back()`.
- A URL naming a missing agent or a closed terminal shows a not-found screen, never a silent fallback.
- The page reloads itself only when a reconnect finds a different server version or process id, or when a code chunk fails to load (`ChunkBoundary`); a different dux instance covers the page and asks.

## Rows another device drives

- While the take-over card is up, every control that would change the row (menus, strips, the Changes pane's git writes, the editor's writes) is disabled with a reason that names the driver or says nobody drives; reading stays.
- A part of the row knows whether this page drives it only from the server; until it has heard, it shows the attaching cover and offers no input.

## Editor

- An editor is rooted at an agent's worktree or at the directory a terminal started in; a session-owned terminal's editor is its agent's.
- dux has no file watcher: an open buffer and the file tree are re-checked on the changed-files broadcast, window focus and tab activation.
- A file deleted while its editor tab keeps focus goes unnoticed when no event reports it: an ignored file, any file under a terminal-rooted editor (which gets no changed-files broadcast), and any file in the info panel, which re-checks only when opened and on window focus.
- A save sends the token its read returned, and the server refuses a mismatch.

## Checks

- After a UI change run `tools/preview-env/checks/run.sh`; a change to a behaviour a check covers updates that check, a new dialog or recurring behaviour gets one, and a stale check is fixed or deleted, never skipped (see `tools/preview-env/checks/README.md`).
