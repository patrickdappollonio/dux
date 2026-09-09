import { Suspense, type ReactNode } from "react"

import { AgentNotFound } from "@/components/AgentNotFound"
import { AgentTabsStrip } from "@/components/AgentTabsStrip"
import { ChunkBoundary } from "@/components/ChunkBoundary"
import { DormantTabSurface } from "@/components/DormantTabSurface"
import { LazyTerminalPane } from "@/components/LazyTerminalPane"
import { PrBanner } from "@/components/PrBanner"
import { TheaterChrome } from "@/components/TheaterChrome"
import { TheaterPill } from "@/components/TheaterPill"
import { Welcome } from "@/components/Welcome"
import {
  dormantTabNeedsCard,
  shouldShowTabStrip,
  slotTabIdOf,
} from "@/lib/agentTabs"
import { useDux } from "@/lib/store"
import type { DuxState } from "@/lib/store"
import { ownerSessionId as terminalOwnerSessionId } from "@/lib/terminalOwner"
import type { Bootstrap } from "@/lib/bootstrapApi"
import type { AgentTabView, PrView, SessionView } from "@/lib/types"

type SelectedTarget = NonNullable<DuxState["selectedTarget"]>

function PrLane({
  pr,
  atBottom,
  position,
}: {
  pr: PrView | null
  atBottom: boolean
  position: "top" | "bottom"
}) {
  if (!pr || atBottom !== (position === "bottom")) return null
  return <PrBanner pr={pr} position={position} />
}

function AgentTabLane({
  target,
  session,
  tabs,
  bootstrap,
}: {
  target: SelectedTarget
  session: SessionView | undefined
  tabs: AgentTabView[]
  bootstrap: Bootstrap | null
}) {
  if (target.kind !== "agent" || !session) return null
  if (!shouldShowTabStrip(tabs, bootstrap?.always_show_tab_strip ?? false)) {
    return null
  }
  return (
    <AgentTabsStrip
      session={session}
      activeTabId={target.tabId}
      maxTabs={bootstrap?.agent_tabs_max}
    />
  )
}

function LiveTerminalPane({
  target,
  paneKey,
  targetId,
  slotTabId,
  overlay,
}: {
  target: SelectedTarget
  paneKey: string
  targetId: string
  slotTabId: string | undefined
  overlay: ReactNode
}) {
  if (target.kind === "agent") {
    return (
      <LazyTerminalPane
        key={paneKey}
        kind="agent"
        id={targetId}
        sessionId={target.sessionId}
        slotTabId={slotTabId}
        overlay={overlay}
      />
    )
  }
  return (
    <LazyTerminalPane
      key={paneKey}
      kind="terminal"
      id={targetId}
      owner={target.owner}
      overlay={overlay}
    />
  )
}

function TerminalSurface({
  target,
  paneKey,
  targetId,
  slotTabId,
  dormant,
  focusedTab,
  overlay,
}: {
  target: SelectedTarget
  paneKey: string
  targetId: string
  slotTabId: string | undefined
  dormant: boolean
  focusedTab: AgentTabView | undefined
  overlay: ReactNode
}) {
  // A dormant tab has no pane to paint over, so the overlay rides the column
  // itself. There are no input rows under a card, which is exactly what makes
  // that safe here and unsafe over a live terminal.
  if (dormant && focusedTab && target.kind === "agent") {
    return (
      <>
        <DormantTabSurface
          sessionId={target.sessionId}
          tabId={focusedTab.id}
          provider={focusedTab.provider}
          lastRunFailed={focusedTab.last_run_failed === true}
          lastRunVerdict={focusedTab.last_run_verdict}
        />
        {overlay}
      </>
    )
  }
  return (
    <ChunkBoundary>
      <Suspense fallback={null}>
        <LiveTerminalPane
          target={target}
          paneKey={paneKey}
          targetId={targetId}
          slotTabId={slotTabId}
          overlay={overlay}
        />
      </Suspense>
    </ChunkBoundary>
  )
}

// The center pane: the agent's terminal or a companion terminal's, the tab strip
// above it, and the PR banner. Its own module rather than inline in App.tsx so it
// can be mounted without `GlobalOverlays`, which eagerly imports Monaco; it pulls
// the terminal pane in behind `React.lazy`.
export function TerminalArea() {
  const {
    spine,
    bootstrap,
    selectedSessionId,
    selectedTarget,
    theater,
    terminalEpoch,
    startedDormantTabs,
    pendingSlotTab,
    routeNotFound,
  } = useDux()

  // The URL names an agent this workspace does not have (a stale bookmark, or
  // Back landing on a deleted agent). Say so rather than showing the idle
  // welcome screen, which would read as "nothing was selected".
  if (routeNotFound) {
    return <AgentNotFound sessionId={routeNotFound.sessionId} />
  }

  // Idle center pane: the duck + logo + a tip, exactly like the TUI's welcome
  // screen. It vanishes the moment a target is selected (the loading state is
  // the terminal pane's readiness spinner, not this).
  if (!selectedTarget) {
    return <Welcome />
  }

  // The PR belongs to the owning session, so it shows whether the agent or one of
  // its companion terminals is focused, as in the TUI. Placement honours the same
  // config: "bottom" puts the lane below the terminal, anything else above.
  const pr =
    spine?.sessions.find((s) => s.id === selectedSessionId)?.pr ?? null
  const bannerAtBottom = bootstrap?.pr_banner_position === "bottom"

  // For an agent the streamed id is the FOCUSED TAB id; for a terminal it is
  // the terminal id. Key by that id so switching tabs/terminals remounts the
  // pane cleanly.
  const targetId =
    selectedTarget.kind === "terminal"
      ? selectedTarget.terminalId
      : selectedTarget.tabId
  // A reconnect bumps `terminalEpoch` so an already-focused agent pane remounts
  // and re-subscribes to the freshly launched provider. Terminals don't
  // reconnect, so the epoch only affects the agent key.
  const paneKey =
    selectedTarget.kind === "agent" ? `${targetId}:${terminalEpoch}` : targetId

  // The owning session, and for an agent the focused tab, which is what the tab
  // strip and the dormant-card gate need. A project terminal has none, and every
  // session-scoped branch below is agent-only or tolerates `undefined`, so the
  // lossy `ownerSessionId` answers exactly the question asked.
  const ownerSessionId =
    selectedTarget.kind === "agent"
      ? selectedTarget.sessionId
      : terminalOwnerSessionId(selectedTarget.owner)
  const session = spine?.sessions.find((s) => s.id === ownerSessionId)
  const tabs = session?.tabs ?? []
  const focusedTab =
    selectedTarget.kind === "agent"
      ? tabs.find((t) => t.id === selectedTarget.tabId)
      : undefined
  const slotTabId = slotTabIdOf(ownerSessionId ?? "", session, pendingSlotTab)
  // Whether this tab gets the "Start session" card instead of the pane. The
  // helper owns the whole rule (a dormant extra tab waits; the agent's first tab
  // starts on selection unless its last run failed); see `dormantTabNeedsCard`.
  const dormant = dormantTabNeedsCard(
    selectedTarget,
    session,
    focusedTab,
    startedDormantTabs,
    slotTabId,
  )

  // The Suspense fallback is null because TerminalPane shows its own readiness
  // spinner on mount, and ChunkBoundary wraps Suspense rather than sitting inside
  // it so a lazy import that fails after a redeploy is recovered.
  //
  // `overflow-hidden` is load-bearing: a resized terminal keeps its old size
  // until the next-rAF refit, so for one frame it overflows this box. The
  // ResizablePanel's inner wrapper is `overflow: auto`, so unclipped that frame
  // sprouts scrollbars, which shrink the content box, which refits, which
  // toggles them again: a visible jitter loop.
  return (
    <div className="flex h-full min-h-0 flex-col">
      {/* The pane's own chrome stack, and the one real loss theater costs: the
        * pull-request band is a status glance and a click target at once. It is
        * one tap away again through the pill's exit. */}
      <TheaterChrome hidden={theater}>
        <PrLane pr={pr} atBottom={bannerAtBottom} position="top" />
        <AgentTabLane
          target={selectedTarget}
          session={session}
          tabs={tabs}
          bootstrap={bootstrap}
        />
      </TheaterChrome>
      <div className="relative min-h-0 flex-1 overflow-hidden">
        <TerminalSurface
          target={selectedTarget}
          paneKey={paneKey}
          targetId={targetId}
          slotTabId={slotTabId}
          dormant={dormant}
          focusedTab={focusedTab}
          overlay={
            theater ? (
              <TheaterPill target={selectedTarget} session={session} />
            ) : null
          }
        />
      </div>
      <TheaterChrome hidden={theater}>
        <PrLane pr={pr} atBottom={bannerAtBottom} position="bottom" />
      </TheaterChrome>
    </div>
  )
}
