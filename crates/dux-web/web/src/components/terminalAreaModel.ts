import { dormantTabNeedsCard, slotTabIdOf } from "@/lib/agentTabs"
import type { DuxState } from "@/lib/store"
import { ownerSessionId as terminalOwnerSessionId } from "@/lib/terminalOwner"
import type { Bootstrap } from "@/lib/bootstrapApi"
import type { AgentTabView, PrView, SessionView } from "@/lib/types"

export type SelectedTarget = NonNullable<DuxState["selectedTarget"]>

// Everything the center pane's chrome and surface are decided from, resolved in
// one place so `TerminalArea` is a layout and not a derivation. The `spine`
// lookups are lossy on purpose: a project terminal has no owning session, and
// every session-scoped field here is agent-only or tolerates `undefined`.
export type TerminalAreaModel = {
  pr: PrView | null
  bannerAtBottom: boolean
  targetId: string
  paneKey: string
  session: SessionView | undefined
  tabs: AgentTabView[]
  focusedTab: AgentTabView | undefined
  slotTabId: string | undefined
  dormant: boolean
}

export function terminalAreaModel(ctx: {
  target: SelectedTarget
  spine: DuxState["spine"]
  bootstrap: Bootstrap | null
  selectedSessionId: string | null
  terminalEpoch: number
  startedDormantTabs: DuxState["startedDormantTabs"]
  pendingSlotTab: DuxState["pendingSlotTab"]
}): TerminalAreaModel {
  const { target, spine } = ctx
  // For an agent the streamed id is the FOCUSED TAB id; for a terminal it is
  // the terminal id. Key by that id so switching tabs/terminals remounts the
  // pane cleanly. A reconnect bumps `terminalEpoch` so an already-focused agent
  // pane remounts and re-subscribes to the freshly launched provider; terminals
  // don't reconnect, so the epoch only affects the agent key.
  const isAgent = target.kind === "agent"
  const targetId = isAgent ? target.tabId : target.terminalId
  const ownerSessionId = isAgent
    ? target.sessionId
    : terminalOwnerSessionId(target.owner)
  const session = spine?.sessions.find((s) => s.id === ownerSessionId)
  const tabs = session?.tabs ?? []
  const focusedTab = isAgent
    ? tabs.find((t) => t.id === target.tabId)
    : undefined
  const slotTabId = slotTabIdOf(ownerSessionId ?? "", session, ctx.pendingSlotTab)
  return {
    // The PR belongs to the owning session, so it shows whether the agent or one
    // of its companion terminals is focused, as in the TUI. Placement honours
    // the same config: "bottom" puts the lane below the terminal, anything else
    // above.
    pr: spine?.sessions.find((s) => s.id === ctx.selectedSessionId)?.pr ?? null,
    bannerAtBottom: ctx.bootstrap?.pr_banner_position === "bottom",
    targetId,
    paneKey: isAgent ? `${targetId}:${ctx.terminalEpoch}` : targetId,
    session,
    tabs,
    focusedTab,
    slotTabId,
    // Whether this tab gets the "Start session" card instead of the pane. The
    // helper owns the whole rule (a dormant extra tab waits; the agent's first
    // tab starts on selection unless its last run failed).
    dormant: dormantTabNeedsCard(
      target,
      session,
      focusedTab,
      ctx.startedDormantTabs,
      slotTabId,
    ),
  }
}
