import {
  dormantTabNeedsCard,
  resolveFocusedTab,
  slotTabIdOf,
} from "@/lib/agentTabs"
import {
  partsOnScreen,
  resolveStackOpening,
  shouldShowStackStrips,
  type AgentTabState,
  type ComputerLayout,
  type StackParts,
} from "@/lib/layoutModel"
import { reorderById } from "@/lib/reorder"
import type { StackMemory } from "@/lib/stackMemory"
import type { DuxState } from "@/lib/store"
import type { AgentTabView, SessionView, TerminalView } from "@/lib/types"

import type { SelectedTarget } from "./terminalAreaModel"

// Everything an agent's stack on a computer is drawn from: the agent's tab on
// top, its dock below, the layout, which part is on screen and which part typing
// goes to. Pure, so the stack component is a layout and not a derivation.
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
  layout: ComputerLayout
  /** The layout to record as this opening's, while none is recorded yet. */
  openingToSettle: ComputerLayout | null
  agentTab: AgentTabState
  parts: StackParts
  /** The pty typing goes to: the agent's tab, or the dock terminal in front. */
  typingPtyId: string
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
  memory: StackMemory
  startedDormantTabs: DuxState["startedDormantTabs"]
  pendingSlotTab: DuxState["pendingSlotTab"]
  endedInViewTab: DuxState["endedInViewTab"]
  pendingTerminalOrder: DuxState["pendingTerminalOrder"]
  alwaysShowTabStrip: boolean
}): AgentStackModel {
  const { target, session } = ctx
  const sessionId = session.id
  const own = ctx.terminals
    .filter((t) => t.owner.kind === "session" && t.owner.session_id === sessionId)
    .slice()
    .sort((a, b) => a.sort_order - b.sort_order)
  // A drag just made shows at once; the server's order replaces it.
  const dock = ctx.pendingTerminalOrder ? reorderById(own, ctx.pendingTerminalOrder) : own
  const dockIds = dock.map((t) => t.id)

  const tabId = target.kind === "agent" ? target.tabId : resolveFocusedTab(session)
  const tab = session.tabs.find((t) => t.id === tabId)
  const slotTabId = slotTabIdOf(sessionId, session, ctx.pendingSlotTab) ?? session.slot_tab_id
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
  const layout: ComputerLayout = terminalRunning ? (recorded ?? opening.layout) : "agent"
  const activeTerminal =
    target.kind === "terminal" && dockIds.includes(target.terminalId)
      ? target.terminalId
      : opening.activeTerminal

  const parts = partsOnScreen({ surface: "computer", layout, agentTab })
  return {
    sessionId,
    tabId,
    tab,
    slotTabId,
    dock,
    activeTerminal,
    layout,
    openingToSettle: terminalRunning && recorded === undefined ? opening.layout : null,
    agentTab,
    parts,
    typingPtyId:
      target.kind === "terminal" && activeTerminal !== null ? activeTerminal : tabId,
    stripsShown: shouldShowStackStrips(session.tabs, dock.length, ctx.alwaysShowTabStrip),
    agentStopped,
  }
}
