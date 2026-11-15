// The browser-tab title: where this tab is, so a strip of dux tabs (and the
// browser's own tab search) can tell them apart the way their addresses would.
//
// The most specific part comes first because browsers cut a tab title off at
// the right: "<file> · <root> · Editor — <instance>". Building the title is
// pure and split from reading the store, so each position is tested on its own.

import { sessionLabel } from "./agentWorkspace"
import { slotTabIdOf, tabLabels, type PendingSlotTab } from "./agentTabs"
import type { EditorTabsState } from "./editorTabs"
import { rootKey, type EditorRoot } from "./editorRoot"
import { formatTabTitle } from "./attention"
import { baseName } from "./paths"
import type { Spine } from "./workspaceApi"

// What the tab is looking at, already reduced to the names the UI shows.
export type TabPosition =
  // The workspace with nothing selected (the hub, home, or a spine not yet
  // loaded): the instance title alone.
  | { kind: "home" }
  // An agent, with the docked terminal typing goes to (or the address names)
  // and the provider tab on screen when it is not the agent's first.
  | { kind: "agent"; agent: string; tab: string | null; terminal: string | null }
  // A project or standalone terminal.
  | { kind: "terminal"; terminal: string }
  // The editor, in-page or in a tab of its own. `root` is null only while the
  // spine that names it has not arrived.
  | { kind: "editor"; root: string | null; file: string | null; dirty: boolean }
  | { kind: "not-found"; what: "agent" | "terminal" }

const PART_SEPARATOR = " · "
const INSTANCE_SEPARATOR = " — "
const DIRTY_MARK = "● "

// A name is user text: a tab title stops at a newline, so whitespace runs
// collapse to one space, as the instance title's do.
function clean(part: string): string {
  return part.replace(/\s+/g, " ").trim()
}

function positionParts(position: TabPosition): string[] {
  switch (position.kind) {
    case "home":
      return []
    case "agent":
      return [position.terminal, position.tab, position.agent].filter(
        (p): p is string => p !== null,
      )
    case "terminal":
      return [position.terminal]
    case "editor": {
      const file =
        position.file === null
          ? null
          : `${position.dirty ? DIRTY_MARK : ""}${position.file}`
      return [file, position.root, "Editor"].filter((p): p is string => p !== null)
    }
    case "not-found":
      return [position.what === "agent" ? "Agent not found" : "Terminal not found"]
  }
}

/** The whole tab title: the `(N) ` attention prefix, the position's parts most
 * specific first, then the instance title. */
export function tabTitle(
  position: TabPosition,
  instanceTitle: string,
  attentionCount: number,
): string {
  const parts = positionParts(position)
    .map(clean)
    .filter((p) => p !== "")
  const base =
    parts.length === 0
      ? instanceTitle
      : `${parts.join(PART_SEPARATOR)}${INSTANCE_SEPARATOR}${instanceTitle}`
  return formatTabTitle(base, attentionCount)
}

// The store fields the position is read from, narrowed so the derivation can
// be tested without the store.
export interface TabPositionInput {
  spine: Spine | null
  selectedTarget:
    | { kind: "agent"; sessionId: string; tabId: string }
    | { kind: "terminal"; terminalId: string; owner: { kind: string } }
    | null
  routeNotFound: { kind: "agent" | "terminal" } | null
  editorRoute: { root: EditorRoot; path: string | null } | null
  editorTabs: Record<string, EditorTabsState>
  pendingSlotTab: Record<string, PendingSlotTab>
}

function agentName(spine: Spine | null, sessionId: string): string | null {
  const session = spine?.sessions.find((s) => s.id === sessionId)
  return session ? sessionLabel(session) : null
}

// A terminal is named by the label it was opened with, as its row is, never by
// its live title, which churns with whatever runs in it.
function terminalName(spine: Spine | null, terminalId: string): string | null {
  return spine?.terminals.find((t) => t.id === terminalId)?.label ?? null
}

function terminalOwnerSession(spine: Spine | null, terminalId: string): string | null {
  const terminal = spine?.terminals.find((t) => t.id === terminalId)
  return terminal?.owner.kind === "session" ? terminal.owner.session_id : null
}

function editorPosition(input: TabPositionInput, route: NonNullable<TabPositionInput["editorRoute"]>): TabPosition {
  const root =
    route.root.kind === "agent"
      ? agentName(input.spine, route.root.sessionId)
      : terminalName(input.spine, route.root.terminalId)
  const tabs = input.editorTabs[rootKey(route.root)]
  const active = tabs?.tabs.find((t) => t.id === tabs.activeId)
  const path = active?.path ?? route.path
  return {
    kind: "editor",
    root,
    file: path === null ? null : baseName(path),
    dirty: active?.dirty ?? false,
  }
}

/** Where the tab is, from the store's position fields. The editor wins over
 * the selection under it, because it covers the pane. */
export function tabPosition(input: TabPositionInput): TabPosition {
  if (input.editorRoute !== null) return editorPosition(input, input.editorRoute)
  if (input.routeNotFound !== null) {
    return { kind: "not-found", what: input.routeNotFound.kind }
  }
  const target = input.selectedTarget
  if (target === null) return { kind: "home" }
  const { spine } = input
  if (target.kind === "agent") {
    const session = spine?.sessions.find((s) => s.id === target.sessionId)
    if (!session) return { kind: "home" }
    const slot = slotTabIdOf(target.sessionId, session, input.pendingSlotTab)
    const index = session.tabs.findIndex((t) => t.id === target.tabId)
    const tab =
      session.tabs.length >= 2 && index >= 0 && target.tabId !== slot
        ? tabLabels(session.tabs)[index]
        : null
    return { kind: "agent", agent: sessionLabel(session), tab, terminal: null }
  }
  const terminal = terminalName(spine, target.terminalId)
  if (terminal === null) return { kind: "home" }
  const owner = terminalOwnerSession(spine, target.terminalId)
  if (owner === null) return { kind: "terminal", terminal }
  const agent = agentName(spine, owner)
  return agent === null
    ? { kind: "terminal", terminal }
    : { kind: "agent", agent, tab: null, terminal }
}
