import { Suspense } from "react"

import { AgentNotFound } from "@/components/AgentNotFound"
import { AgentTabsStrip } from "@/components/AgentTabsStrip"
import { ChunkBoundary } from "@/components/ChunkBoundary"
import { DormantTabCard } from "@/components/DormantTabCard"
import { LazyTerminalPane } from "@/components/LazyTerminalPane"
import { PrBanner } from "@/components/PrBanner"
import { Welcome } from "@/components/Welcome"
import { isExtraTabDormant, shouldShowTabStrip } from "@/lib/agentTabs"
import { useDux } from "@/lib/store"
import { ownerSessionId as terminalOwnerSessionId } from "@/lib/terminalOwner"

// The center pane: the agent's terminal (or a companion terminal's), the tab
// strip above it, and the PR banner. Split into its own module (rather than
// living inline in App.tsx) so it can be unit-tested without pulling in
// `GlobalOverlays` -> `ConfigEditorDialog`, which eagerly imports the multi-MB
// Monaco bundle that cannot initialize under vitest (see the note in
// `lib/pathExt.ts`). `TerminalArea` itself only pulls in the terminal pane
// behind `React.lazy`, so it mounts cleanly in tests (see `App.test.tsx`,
// which covers the dormant-tab gating this component owns).
export function TerminalArea() {
  const {
    spine,
    bootstrap,
    selectedSessionId,
    selectedTarget,
    terminalEpoch,
    startedDormantTabs,
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

  // The PR belongs to the owning session, so it shows whether the agent or one
  // of its companion terminals is focused (mirroring the TUI, which shares the
  // session's PR across surfaces). Placement honours the same config the TUI
  // does: "bottom" puts the lane below the terminal, anything else above.
  const pr =
    spine?.sessions.find((s) => s.id === selectedSessionId)?.pr ?? null
  const bannerAtBottom = bootstrap?.pr_banner_position === "bottom"

  // For an agent the streamed id is the FOCUSED TAB id (the session-slot tab's equals the
  // session id); for a terminal it is the terminal id. Key by that id so
  // switching tabs/terminals remounts the pane cleanly.
  const targetId =
    selectedTarget.kind === "terminal"
      ? selectedTarget.terminalId
      : selectedTarget.tabId
  // A reconnect bumps `terminalEpoch` so an already-focused agent pane remounts
  // and re-subscribes to the freshly launched provider. Terminals don't
  // reconnect, so the epoch only affects the agent key.
  const paneKey =
    selectedTarget.kind === "agent" ? `${targetId}:${terminalEpoch}` : targetId

  // Resolve the owning session + (for an agent) the focused tab so we can render
  // the tab strip and gate the dormant card. A project terminal has NO owning
  // session; every session-scoped branch below is agent-only or tolerates
  // `undefined`.
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
  // A focused extra tab with no live process is DORMANT (reopened after a
  // restart): render its card WITHOUT mounting the pane, because mounting opens
  // the PTY socket, which force-launches the provider. Only the card's "Start
  // session" button (via `startedDormantTabs`) launches it.
  const isExtraDormant = isExtraTabDormant(
    selectedTarget,
    focusedTab,
    startedDormantTabs,
  )

  // TerminalPane owns its own background and padding (via inline style) so the
  // padding area is seamlessly part of the terminal surface.
  // Suspense fallback is null: the lazy chunk loads fast and TerminalPane shows
  // its own readiness spinner the moment it mounts, so a fallback spinner here
  // would just double up.
  // ChunkBoundary wraps Suspense (not inside it) so a failed lazy import after a
  // server redeploy is caught and recovered instead of unmounting the tree.
  //
  // overflow-hidden is load-bearing: during a divider/window resize the
  // terminal keeps its previous size until the next-rAF refit, so for one
  // frame it overflows this box. The ResizablePanel's inner wrapper is
  // `overflow: auto` — left unclipped, that one-frame overflow sprouts real
  // div scrollbars whose width shrinks the content box, which retriggers the
  // ResizeObserver, which refits, which toggles the scrollbar again: a
  // visible jitter loop. Clipping here means the transient overflow is
  // simply invisible and the loop can never start.
  return (
    <div className="flex h-full min-h-0 flex-col">
      {pr && !bannerAtBottom ? <PrBanner pr={pr} position="top" /> : null}
      {selectedTarget.kind === "agent" &&
      session &&
      shouldShowTabStrip(tabs, bootstrap?.always_show_tab_strip ?? false) ? (
        <AgentTabsStrip
          session={session}
          activeTabId={selectedTarget.tabId}
          maxTabs={bootstrap?.agent_tabs_max}
        />
      ) : null}
      <div className="min-h-0 flex-1 overflow-hidden">
        {isExtraDormant && focusedTab && selectedTarget.kind === "agent" ? (
          <DormantTabCard
            sessionId={selectedTarget.sessionId}
            tabId={focusedTab.id}
            provider={focusedTab.provider}
          />
        ) : (
          <ChunkBoundary>
            <Suspense fallback={null}>
              {selectedTarget.kind === "agent" ? (
                <LazyTerminalPane
                  key={paneKey}
                  kind="agent"
                  id={targetId}
                  sessionId={selectedTarget.sessionId}
                />
              ) : (
                <LazyTerminalPane
                  key={paneKey}
                  kind="terminal"
                  id={targetId}
                  owner={selectedTarget.owner}
                />
              )}
            </Suspense>
          </ChunkBoundary>
        )}
      </div>
      {pr && bannerAtBottom ? <PrBanner pr={pr} position="bottom" /> : null}
    </div>
  )
}
