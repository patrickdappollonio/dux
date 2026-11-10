import { dormantTabNeedsCard, slotTabIdOf } from "@/lib/agentTabs"
import {
  partsOnScreen,
  resolveStackOpening,
  shouldShowStackStrips,
  stackTypingTarget,
  type AgentTabState,
  type ComputerLayout,
  type PhoneView,
  type StackParts,
} from "@/lib/layoutModel"
import { reorderById } from "@/lib/reorder"
import type { StackMemory } from "@/lib/stackMemory"
import type { DuxState } from "@/lib/store"
import type { AgentTabView, SessionView, TerminalView } from "@/lib/types"

import type { SelectedTarget } from "./terminalAreaModel"

// Everything an agent's stack is drawn from: the agent's tab on top, its dock
// below, the layout (or a phone's one part), which part is on screen and which
// part typing goes to. Pure, so the stack component is a layout and not a
// derivation.
export interface AgentStackModel {
  sessionId: string
  /** The provider tab on top. */
  tabId: string
  tab: AgentTabView | undefined
  slotTabId: string
  /** The agent's own terminals, in dock order. */
  dock: TerminalView[]
  /** The dock terminal in front, or null when the agent has none. */
  activeTerminal: string | null
  /** A terminal selected the moment its create answered, before the workspace
   * lists it, or null. */
  unlistedTerminal: string | null
  layout: ComputerLayout
  /** The part a phone shows: the part the selection names, which on a phone is
   * the part on screen and the one typing goes to. */
  view: PhoneView
  /** The layout to record as this opening's, while none is recorded yet. */
  openingToSettle: ComputerLayout | null
  agentTab: AgentTabState
  parts: StackParts
  /** The pty the store names as the one typing goes to. */
  chosenPtyId: string
  /** The pty typing goes to: the chosen one while it is a part on screen, else
   * a part that is, or null with no part at all. */
  typingPtyId: string | null
  /** What the agent's slot shows when it has no live part: the short card of a
   * stopped agent beside its dock, the full card otherwise. */
  agentCard: "short" | "full" | null
  /** Both strips show, or neither. */
  stripsShown: boolean
  /** The agent is stopped while its terminals run, so its strip gives way to a
   * band saying so. */
  agentStopped: boolean
}

export function agentStackModel(ctx: {
  target: SelectedTarget
  session: SessionView
  terminals: readonly TerminalView[]
  stackLayouts: DuxState["stackLayouts"]
  stackTerminals: DuxState["stackTerminals"]
  stackTabs: DuxState["stackTabs"]
  memory: StackMemory
  startedDormantTabs: DuxState["startedDormantTabs"]
  pendingSlotTab: DuxState["pendingSlotTab"]
  endedInViewTab: DuxState["endedInViewTab"]
  pendingTerminalOrder: DuxState["pendingTerminalOrder"]
  alwaysShowTabStrip: boolean
  /** Theater shows the part typing goes to alone, maximized. */
  theater: boolean
  /** A phone shows one part at a time and never lays the two out together. */
  surface?: "computer" | "phone"
}): AgentStackModel {
  const phone = ctx.surface === "phone"
  const { target, session } = ctx
  const sessionId = session.id
  const own = ctx.terminals
    .filter((t) => t.owner.kind === "session" && t.owner.session_id === sessionId)
    .slice()
    .sort((a, b) => a.sort_order - b.sort_order)
  // A drag just made shows at once; the server's order replaces it.
  const ordered = ctx.pendingTerminalOrder ? reorderById(own, ctx.pendingTerminalOrder) : own
  // A terminal is selected the moment its create answers, before the workspace
  // that lists it arrives. Until then it is the dock's newest member, so it
  // shows and takes typing at once; the next workspace confirms it, or the
  // selection's own prune drops it.
  const unlisted =
    target.kind === "terminal" &&
    target.owner.kind === "session" &&
    target.owner.sessionId === sessionId &&
    !ordered.some((t) => t.id === target.terminalId)
      ? target.terminalId
      : null
  const dock = unlisted === null ? ordered : [...ordered, unlistedTerminal(unlisted, sessionId)]
  const dockIds = dock.map((t) => t.id)

  const slotTabId = slotTabIdOf(sessionId, session, ctx.pendingSlotTab) ?? session.slot_tab_id
  // With typing in the dock, the agent keeps the tab this device last showed on
  // top, a reload included. The agent's shared last-focused tab is another device's answer too,
  // and following it would swap the tab under the user.
  const shownHere = ctx.stackTabs[sessionId] ?? ctx.memory.tab ?? undefined
  const tabId =
    target.kind === "agent"
      ? target.tabId
      : shownHere !== undefined && session.tabs.some((t) => t.id === shownHere)
        ? shownHere
        : slotTabId
  const tab = session.tabs.find((t) => t.id === tabId)
  const terminalRunning = dock.length > 0
  const agentStopped = terminalRunning && session.status !== "active"

  const live =
    tab?.has_live_process === true || (tab !== undefined && ctx.startedDormantTabs.includes(tab.id))
  const card = dormantTabNeedsCard(
    { kind: "agent", sessionId, tabId },
    session,
    tab,
    ctx.startedDormantTabs,
    slotTabId,
    ctx.endedInViewTab,
    terminalRunning,
  )
  const agentTab: AgentTabState = live ? "live" : card ? "dormant" : "startsOnSight"

  const opening = resolveStackOpening({
    addressTerminal: target.kind === "terminal" ? target.terminalId : null,
    rememberedTerminal: ctx.stackTerminals[sessionId] ?? ctx.memory.terminal,
    rememberedLayout: ctx.memory.layout,
    dockTerminals: dockIds,
    agentStopped,
    anyTerminalRunning: terminalRunning,
  })
  const recorded = ctx.stackLayouts[sessionId]
  // With no dock there is nothing to lay out, and whatever was recorded waits
  // for the next terminal.
  const laidOut: ComputerLayout = terminalRunning ? (recorded ?? opening.layout) : "agent"
  const activeTerminal =
    target.kind === "terminal" && dockIds.includes(target.terminalId)
      ? target.terminalId
      : opening.activeTerminal
  // Theater maximizes the part typing goes to without touching the layout the
  // stack is in, so leaving it puts that layout back.
  const typingInDock = target.kind === "terminal" && activeTerminal !== null
  const layout: ComputerLayout = !ctx.theater
    ? laidOut
    : typingInDock
      ? "terminal"
      : "agent"

  const view: PhoneView = typingInDock ? "terminals" : "agent"
  const parts = partsOnScreen(
    phone ? { surface: "phone", view, agentTab } : { surface: "computer", layout, agentTab },
  )
  const stackParts = [
    ...(parts.agentLiveSocket ? [{ id: tabId, onScreen: parts.agent }] : []),
    ...(activeTerminal !== null ? [{ id: activeTerminal, onScreen: parts.dock }] : []),
  ]
  const chosenPtyId = typingInDock ? activeTerminal : tabId
  return {
    sessionId,
    tabId,
    tab,
    slotTabId,
    dock,
    activeTerminal,
    unlistedTerminal: unlisted,
    layout,
    view,
    openingToSettle:
      !phone && terminalRunning && recorded === undefined ? opening.layout : null,
    agentTab,
    parts,
    chosenPtyId,
    typingPtyId: stackTypingTarget(stackParts, chosenPtyId),
    // The short card is for an agent sharing the screen with its terminals,
    // which a phone never does.
    agentCard: parts.agentLiveSocket
      ? null
      : !phone && agentStopped && tabId === slotTabId
        ? "short"
        : "full",
    stripsShown: shouldShowStackStrips(session.tabs, dock.length, ctx.alwaysShowTabStrip),
    agentStopped,
  }
}

// What a terminal the workspace does not list yet looks like until it does.
function unlistedTerminal(id: string, sessionId: string): TerminalView {
  return {
    id,
    owner: { kind: "session", session_id: sessionId },
    label: "Terminal",
    has_output: false,
    working: false,
    typing: false,
    foreground_cmd: null,
    sort_order: Number.MAX_SAFE_INTEGER,
    created_at: "",
    updated_at: "",
    list_after: null,
  }
}
