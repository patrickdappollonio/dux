import {
  Suspense,
  useCallback,
  useLayoutEffect,
  useRef,
  useState,
  type ReactNode,
} from "react"
import type { GroupImperativeHandle, Layout } from "react-resizable-panels"

import { AgentTabsStrip } from "@/components/AgentTabsStrip"
import { ChunkBoundary } from "@/components/ChunkBoundary"
import { DockStrip } from "@/components/DockStrip"
import { DormantTabSurface } from "@/components/DormantTabSurface"
import { LazyTerminalStack } from "@/components/LazyTerminalPane"
import {
  StackLayoutPill,
  type LayoutPillPlacement,
} from "@/components/StackLayoutPill"
import { StoppedAgentBand, StoppedAgentShortCard } from "@/components/StoppedAgentParts"
import { TheaterChrome } from "@/components/TheaterChrome"
import { TheaterPill } from "@/components/TheaterPill"
import { DockDivider } from "@/components/DockDivider"
import { ResizablePanel, ResizablePanelGroup } from "@/components/ui/resizable"
import type { StackPartSpec } from "@/components/TerminalStack"
import {
  applyLayoutAction,
  type ComputerLayout,
  type LayoutAction,
} from "@/lib/layoutModel"
import {
  DIVIDER_STORAGE_KEYS,
  readStoredPanePercent,
  writeStoredPanePercent,
} from "@/lib/paneDivider"
import {
  chooseStackPart,
  reconcileStackPart,
  setStackLayout,
  settleStackOpening,
} from "@/lib/store"
import { cn } from "@/lib/utils"
import type { SessionView } from "@/lib/types"

import type { AgentStackModel } from "./agentStackModel"
import {
  NOT_TYPING_CLASS,
  STACK_PART_MIN_PERCENT,
  dividerRelease,
  panelSizes,
} from "./agentStackLayout"
import type { SelectedTarget } from "./terminalAreaModel"

const AGENT_PANEL = "stack-agent"
const DOCK_PANEL = "stack-dock"
const SPLIT_DEFAULT_PERCENT = 50

function readSplitPercent(): number {
  return readStoredPanePercent(
    DIVIDER_STORAGE_KEYS.stackAgentPercent,
    SPLIT_DEFAULT_PERCENT,
    STACK_PART_MIN_PERCENT,
    100 - STACK_PART_MIN_PERCENT,
  )
}

// An agent's stack on a computer: the agent's tab with its strip on top and,
// when it has terminals, a dock below with a strip of its own, split by a
// divider. Either part can take the whole height and the other keeps only its
// strip, and a part folded away stays mounted in a box the size it has in the
// split, clipped, so its program never sees a sliver of a terminal.
export function AgentStack({
  model,
  session,
  target,
  theater,
  maxTabs,
  terminalEpoch,
}: {
  model: AgentStackModel
  session: SessionView
  target: SelectedTarget
  theater: boolean
  maxTabs: number | undefined
  terminalEpoch: number
}) {
  const { sessionId, layout, dock, parts: on } = model
  const hasDock = dock.length > 0

  // Record how this opening laid the stack out, so it holds while the agent is
  // worked in. A layout effect, so the frame painted is already the one held.
  const opening = model.openingToSettle
  useLayoutEffect(() => {
    if (opening !== null) settleStackOpening(sessionId, opening)
  }, [sessionId, opening])

  // One answer for where typing goes. When the part the store names is folded
  // away or has no live terminal, the stack routes typing to a part on screen,
  // and the store is told the same, so the header's Macros, the top menu, the
  // theater pill and the shell's cover read all act on that part too.
  const { chosenPtyId, typingPtyId } = model
  useLayoutEffect(() => {
    if (typingPtyId !== null && typingPtyId !== chosenPtyId) {
      reconcileStackPart(sessionId, typingPtyId)
    }
  }, [sessionId, chosenPtyId, typingPtyId])

  const onLayoutAction = (action: LayoutAction) =>
    setStackLayout(sessionId, applyLayoutAction(layout, action))

  const [splitPercent, setSplitPercent] = useState(readSplitPercent)
  const [agentHeaderPx, setAgentHeaderPx] = useState(0)

  const agentHeader = model.agentStopped ? (
    <StoppedAgentBand
      sessionId={sessionId}
      slotTab={session.tabs.find((t) => t.id === model.slotTabId)}
      withStart={layout === "terminal"}
    />
  ) : model.stripsShown ? (
    <AgentTabsStrip session={session} activeTabId={model.tabId} maxTabs={maxTabs} />
  ) : null
  const dockHeader =
    hasDock && model.stripsShown ? (
      <DockStrip
        sessionId={sessionId}
        terminals={dock}
        activeTerminal={model.activeTerminal}
        unlisted={model.unlistedTerminal}
        layout={layout}
        onLayoutAction={onLayoutAction}
      />
    ) : null

  const overlay = theater ? (
    <TheaterPill target={target} session={session} />
  ) : model.stripsShown || !hasDock ? null : (
    <StackLayoutPill
      session={session}
      layout={layout}
      placement={pillPlacement(layout, splitPercent, agentHeaderPx)}
      onLayoutAction={onLayoutAction}
    />
  )

  const stackParts: StackPartSpec[] = []
  if (on.agentLiveSocket) {
    stackParts.push({
      slot: "agent",
      // A reconnect replaces the agent's process, so its part re-attaches.
      instance: `${model.tabId}:${terminalEpoch}`,
      kind: "agent",
      id: model.tabId,
      sessionId,
      slotTabId: model.slotTabId,
      onScreen: on.agent,
    })
  }
  if (model.activeTerminal !== null) {
    stackParts.push({
      slot: "dock",
      instance: model.activeTerminal,
      kind: "terminal",
      id: model.activeTerminal,
      owner: { kind: "session", sessionId },
      onScreen: on.dock,
    })
  }
  // Only on screen: a card has no connection to keep alive while folded away.
  // A stopped agent's first tab beside its dock gets the short card; any other
  // dormant tab keeps its own card, whose Start is about that tab alone.
  const showCard = model.tab !== undefined && on.agent && model.agentCard !== null
  const agentCard = !showCard || model.tab === undefined ? null : model.agentCard === "short" ? (
    <StoppedAgentShortCard sessionId={sessionId} tab={model.tab} />
  ) : (
    <DormantTabSurface
      sessionId={sessionId}
      tabId={model.tab.id}
      provider={model.tab.provider}
      lastRunFailed={model.tab.last_run_failed === true}
      lastRunVerdict={model.tab.last_run_verdict}
    />
  )

  return (
    <ChunkBoundary>
      <Suspense fallback={null}>
        <LazyTerminalStack
          parts={stackParts}
          overlay={overlay}
          typingTarget={model.typingPtyId}
          onChooseTypingTarget={(ptyId) => chooseStackPart(sessionId, ptyId)}
          arrange={(rendered, typingTarget) => (
            <StackSplit
              layout={layout}
              hasDock={hasDock}
              theater={theater}
              splitPercent={splitPercent}
              onSplitPercent={setSplitPercent}
              onFold={(fold) => setStackLayout(sessionId, fold)}
              agentHeader={agentHeader}
              agentHeaderPx={agentHeaderPx}
              onAgentHeaderPx={setAgentHeaderPx}
              dockHeader={dockHeader}
              agent={rendered.agent ?? agentCard}
              dock={rendered.dock ?? null}
              agentOnScreen={on.agent}
              dockOnScreen={on.dock}
              // Grey goes on the live terminal that typing does not go to, in
              // the split only: a folded part is clipped anyway.
              greyAgent={
                layout === "split" && rendered.agent !== undefined && typingTarget !== model.tabId
              }
              greyDock={
                layout === "split" &&
                rendered.dock !== undefined &&
                typingTarget !== model.activeTerminal
              }
            />
          )}
        />
      </Suspense>
    </ChunkBoundary>
  )
}

function pillPlacement(
  layout: ComputerLayout,
  splitPercent: number,
  agentHeaderPx: number,
): LayoutPillPlacement {
  switch (layout) {
    case "split":
      return { kind: "divider", agentPercent: splitPercent }
    case "agent":
      return { kind: "bottom" }
    case "terminal":
      return { kind: "top", belowPx: agentHeaderPx }
  }
}

function StackSplit({
  layout,
  hasDock,
  theater,
  splitPercent,
  onSplitPercent,
  onFold,
  agentHeader,
  agentHeaderPx,
  onAgentHeaderPx,
  dockHeader,
  agent,
  dock,
  agentOnScreen,
  dockOnScreen,
  greyAgent,
  greyDock,
}: {
  layout: ComputerLayout
  hasDock: boolean
  theater: boolean
  splitPercent: number
  onSplitPercent: (percent: number) => void
  onFold: (layout: ComputerLayout) => void
  agentHeader: ReactNode
  agentHeaderPx: number
  onAgentHeaderPx: (px: number) => void
  dockHeader: ReactNode
  agent: ReactNode
  dock: ReactNode
  agentOnScreen: boolean
  dockOnScreen: boolean
  greyAgent: boolean
  greyDock: boolean
}) {
  const groupRef = useRef<GroupImperativeHandle | null>(null)
  const [groupPx, setGroupPx] = useState(0)
  const [dockHeaderPx, setDockHeaderPx] = useState(0)
  const groupEl = useObservedHeight(setGroupPx)
  const agentHeaderEl = useObservedHeight(onAgentHeaderPx)
  const dockHeaderEl = useObservedHeight(setDockHeaderPx)

  const sizes = panelSizes(layout, hasDock, splitPercent, {
    group: groupPx,
    agentHeader: agentHeaderPx,
    dockHeader: dockHeaderPx,
  })
  // The library owns the sizes during a drag; every other change of layout or
  // of a strip's height is pushed to it here.
  const agentSize = sizes.agent
  useLayoutEffect(() => {
    if (!hasDock) return
    groupRef.current?.setLayout({ [AGENT_PANEL]: agentSize, [DOCK_PANEL]: 100 - agentSize })
  }, [hasDock, agentSize])

  const split = layout === "split" && hasDock
  const onLayoutChanged = (next: Layout) => {
    if (!split) return
    const release = dividerRelease(
      next[AGENT_PANEL] ?? splitPercent,
      next[DOCK_PANEL] ?? 100 - splitPercent,
    )
    if ("fold" in release) {
      onFold(release.fold)
      return
    }
    if (release.persist === splitPercent) return
    onSplitPercent(release.persist)
    writeStoredPanePercent(DIVIDER_STORAGE_KEYS.stackAgentPercent, release.persist)
  }

  // The box a part keeps while it is folded away: the height it has in the
  // split, measured against the whole stack, so its program keeps a real grid.
  const foldedBox = (share: number) =>
    ({ height: `${share}cqh` }) as React.CSSProperties

  return (
    <div ref={groupEl} className="relative h-full min-h-0 w-full [container-type:size]">
      <ResizablePanelGroup
        orientation="vertical"
        groupRef={groupRef}
        disabled={!split}
        onLayoutChanged={onLayoutChanged}
      >
        <ResizablePanel
          id={AGENT_PANEL}
          defaultSize={`${sizes.agent}%`}
          minSize={split ? `${STACK_PART_MIN_PERCENT}%` : "0%"}
          collapsible={split}
          collapsedSize="0%"
        >
          <div className="relative flex h-full min-h-0 flex-col overflow-hidden">
            <div ref={agentHeaderEl} className="shrink-0">
              <TheaterChrome hidden={theater}>{agentHeader}</TheaterChrome>
            </div>
            <PartBox
              onScreen={agentOnScreen}
              folded={foldedBox(splitPercent)}
              grey={greyAgent}
              testId="stack-agent-box"
            >
              {agent}
            </PartBox>
          </div>
        </ResizablePanel>
        {hasDock ? (
          <>
            {/* Theater draws no strip, and the divider is the dock's strip
              * while the strips are hidden. */}
            {theater ? null : <DockDivider disabled={!split} />}
            <ResizablePanel
              id={DOCK_PANEL}
              defaultSize={`${100 - sizes.agent}%`}
              minSize={split ? `${STACK_PART_MIN_PERCENT}%` : "0%"}
              collapsible={split}
              collapsedSize="0%"
            >
              <div className="relative flex h-full min-h-0 flex-col overflow-hidden">
                <div ref={dockHeaderEl} className="shrink-0">
                  <TheaterChrome hidden={theater}>{dockHeader}</TheaterChrome>
                </div>
                <PartBox
                  onScreen={dockOnScreen}
                  folded={foldedBox(100 - splitPercent)}
                  grey={greyDock}
                  testId="stack-dock-box"
                >
                  {dock}
                </PartBox>
              </div>
            </ResizablePanel>
          </>
        ) : null}
      </ResizablePanelGroup>
    </div>
  )
}

// One part's box. On screen it fills what its panel leaves under the strip;
// folded away it keeps its split height below the panel's edge, where the
// panel clips it.
function PartBox({
  onScreen,
  folded,
  grey,
  testId,
  children,
}: {
  onScreen: boolean
  folded: React.CSSProperties
  grey: boolean
  testId: string
  children: ReactNode
}) {
  return (
    <div
      data-testid={testId}
      data-on-screen={onScreen}
      className={cn(
        onScreen
          ? "relative flex min-h-0 w-full flex-1 flex-col"
          : "absolute inset-x-0 top-full flex flex-col",
        // The part typing does not go to, on its terminal box only: its strip
        // sits outside this box and stays as it is.
        grey && NOT_TYPING_CLASS,
      )}
      style={onScreen ? undefined : folded}
    >
      {children}
    </div>
  )
}

function useObservedHeight(
  onHeight: (px: number) => void,
): (el: HTMLElement | null) => void {
  const observer = useRef<ResizeObserver | null>(null)
  return useCallback(
    (el: HTMLElement | null) => {
      observer.current?.disconnect()
      observer.current = null
      if (el === null) return
      onHeight(el.getBoundingClientRect().height)
      if (typeof ResizeObserver === "undefined") return
      observer.current = new ResizeObserver(() => onHeight(el.getBoundingClientRect().height))
      observer.current.observe(el)
    },
    [onHeight],
  )
}
