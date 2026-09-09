import { Bot, EllipsisVertical, Plus } from "lucide-react"
import type * as React from "react"
import { useCallback, useEffect, useRef, useState } from "react"

import { agentRowVisual } from "@/lib/agentRow"

import { CreationOverflowMenuItems } from "@/components/CreationOverflowMenuItems"
import { LauncherCorner } from "@/components/LauncherCorner"
import { AgentVitalsTooltip } from "@/components/AgentVitalsTooltip"
import { ConnDot } from "@/components/ConnDot"
import { FlatAgentList } from "@/components/FlatAgentList"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarTrigger,
} from "@/components/ui/sidebar"
import { useSidebar } from "@/components/ui/sidebar"
import { changesCountFor } from "@/lib/agentVitals"
import { resolveInstanceTitle } from "@/lib/instanceTitle"
import { partitionProjects } from "@/lib/projects"
import { workspaceProjectId } from "@/lib/agentWorkspace"
import {
  DIVIDER_CHROME,
  DIVIDER_DRAG_THRESHOLD_PX,
  dividerKeyAction,
  SIDEBAR_RESIZING_ATTR,
} from "@/lib/paneDivider"
import { beginLayoutGesture, endLayoutGesture } from "@/lib/layoutGesture"
import { useDividerDrag } from "@/hooks/use-divider-drag"
import {
  MAX_SIDEBAR_PX,
  MIN_SIDEBAR_PX,
  SIDEBAR_KEY_STEP_PX,
  sidebarResizeRelease,
  sidebarWidthToPx,
} from "@/lib/sidebarResize"
import {
  openNewAgentPicker,
  selectSession,
  selectTerminal,
  setSidebarWidth,
  SIDEBAR_INITIAL_WIDTH,
  useDux,
} from "@/lib/store"
import type { ChangesSlice, SelectedTarget } from "@/lib/store"
import { cn } from "@/lib/utils"
import type { SessionView } from "@/lib/types"
import { sessionLabel } from "@/lib/agentWorkspace"

// The icon rail replaces the flat agent list at `collapsible="icon"` width: every
// agent, flattened in project-then-agent order, with the same cues and selection.
function CollapsedAgentIcon({
  session,
  projectName,
  changesCount,
  selected,
}: {
  session: SessionView
  projectName: string
  changesCount: number | null
  selected: boolean
}) {
  const label = sessionLabel(session)
  const { shimmer, dimmed, attention, typing } = agentRowVisual(
    session.status,
    session.working,
    session.needs_attention,
    session.typing,
  )
  return (
    <SidebarMenuItem>
      <SimpleTooltip
        content={
          <AgentVitalsTooltip
            session={session}
            projectName={projectName}
            changesCount={changesCount}
          />
        }
        side="right"
      >
        <SidebarMenuButton
          isActive={selected}
          aria-label={projectName ? `${label} (${projectName})` : label}
          onClick={() => selectSession(session.id)}
          className={cn("touch-manipulation", dimmed && "opacity-70")}
        >
          <span
            aria-label={
              attention ? "Needs attention" : typing ? "Typing" : undefined
            }
            className={cn(
              "inline-flex shrink-0",
              attention
                ? "text-cyan-100 motion-safe:animate-attention-pulse motion-reduce:animate-none"
                : // Typing tints the rail icon violet (no bob) so the icon-only
                  // rail still distinguishes typing from working.
                  typing
                  ? "text-dux-typing"
                  : "text-sidebar-accent-foreground",
            )}
          >
            <Bot
              className={cn(
                "size-4.5! shrink-0 motion-safe:transition-transform motion-safe:duration-300",
                shimmer && "motion-safe:animate-agent-working",
              )}
            />
          </span>
        </SidebarMenuButton>
      </SimpleTooltip>
    </SidebarMenuItem>
  )
}

function CollapsedAgentRail({
  projectIds,
  grouped,
  standalone,
  projectName,
  changes,
  selectedTarget,
}: {
  projectIds: string[]
  grouped: Map<string, SessionView[]>
  /** The agents that belong to no project, in list order: grouping by project
   * loses them, and the rail is the only way to reach an agent at icon width. */
  standalone: SessionView[]
  projectName: (id: string) => string
  changes: ChangesSlice
  selectedTarget: SelectedTarget | null
}) {
  const entries = [
    ...projectIds.flatMap((projectId) =>
      (grouped.get(projectId) ?? []).map((session) => ({
        session,
        // The tooltip's project line, empty for an agent with no project; the
        // tooltip drops the separator with it and names the folder instead.
        projectLabel: projectName(projectId),
      })),
    ),
    ...standalone.map((session) => ({ session, projectLabel: "" })),
  ]

  if (entries.length === 0) return null

  return (
    <SidebarGroup
      data-testid="collapsed-agent-rail"
      // The rail gets its own bounded scrollable region, or SidebarContent's
      // icon-mode `overflow-hidden` clips a long agent list below the fold.
      className="hidden min-h-0 flex-1 overflow-y-auto no-scrollbar group-data-[collapsible=icon]:flex"
    >
      <SidebarGroupContent>
        <SidebarMenu>
          {entries.map(({ session, projectLabel }) => (
            <CollapsedAgentIcon
              key={session.id}
              session={session}
              projectName={projectLabel}
              changesCount={changesCountFor(changes, session.id)}
              selected={
                selectedTarget?.kind === "agent" &&
                selectedTarget.sessionId === session.id
              }
            />
          ))}
        </SidebarMenu>
      </SidebarGroupContent>
    </SidebarGroup>
  )
}

// Edge affordance on the sidebar's right edge: drag-to-resize when expanded,
// click-to-expand when collapsed, desktop only. The gesture is the Changes
// divider's shared one; only the band and the collapse target differ.
function SidebarResizeHandle() {
  const { state, isMobile } = useSidebar()
  // A collapse unmounts the control the keyboard was standing on, so this carries
  // the focus intent across the swap. State, because the claim crosses components.
  const [focusOnMount, setFocusOnMount] = useState(false)
  const claimFocus = useCallback(() => setFocusOnMount(true), [])
  const releaseFocusClaim = useCallback(() => setFocusOnMount(false), [])

  if (state === "collapsed") {
    return isMobile ? null : (
      <SidebarExpandStrip
        focusOnMount={focusOnMount}
        onFocusClaimed={releaseFocusClaim}
        claimFocus={claimFocus}
      />
    )
  }
  return (
    <SidebarDragEdge
      focusOnMount={focusOnMount}
      onFocusClaimed={releaseFocusClaim}
      claimFocus={claimFocus}
    />
  )
}

// Take a pending focus handoff, once, on mount.
function useFocusHandoff(
  focusOnMount: boolean,
  onFocusClaimed: () => void,
  target: React.RefObject<HTMLElement | null>,
) {
  useEffect(() => {
    if (!focusOnMount) return
    onFocusClaimed()
    target.current?.focus({ preventScroll: true })
  }, [focusOnMount, onFocusClaimed, target])
}

interface SidebarEdgeProps {
  focusOnMount: boolean
  onFocusClaimed: () => void
  claimFocus: () => void
}

function SidebarExpandStrip({
  focusOnMount,
  onFocusClaimed,
  claimFocus,
}: SidebarEdgeProps) {
  const { setOpen } = useSidebar()
  const ref = useRef<HTMLButtonElement | null>(null)
  useFocusHandoff(focusOnMount, onFocusClaimed, ref)
  return (
    <button
      ref={ref}
      type="button"
      data-sidebar="expand-handle"
      aria-label="Expand sidebar"
      // `detail` is 0 only for a keyboard-synthesised click. A keyboard expand
      // hands focus to the drag edge; a mouse click leaves focus where it is.
      onClick={(event) => {
        if (event.detail === 0) claimFocus()
        setOpen(true)
      }}
      className={cn(
        DIVIDER_CHROME,
        // Stacking comes from the shared chrome, which both dividers wear.
        "absolute inset-y-0 -right-px cursor-e-resize",
      )}
    />
  )
}

function SidebarDragEdge({
  focusOnMount,
  onFocusClaimed,
  claimFocus,
}: SidebarEdgeProps) {
  const { setOpen } = useSidebar()
  const { sidebarWidth } = useDux()

  // The live width, and the width the current gesture is measured from. Refs: both
  // change at pointer-move cadence and the listeners below are installed once.
  const widthRef = useRef(sidebarWidth)
  useEffect(() => {
    widthRef.current = sidebarWidth
  })
  const grabbedPxRef = useRef(sidebarWidthToPx(sidebarWidth))

  // The wrapper `--sidebar-width` lives on, resolved from the edge's own element:
  // it belongs to the sidebar primitive, several levels above this component.
  const wrapperRef = useRef<HTMLElement | null>(null)
  // The width the DOM is painted at, ahead of the store during a gesture. Every
  // path that ends one reads it, so a cancel leaves the sidebar where it was.
  const paintedRef = useRef(sidebarWidth)

  // The live width is painted onto the wrapper, not stored: a per-move store write
  // re-renders every `useDux` consumer. The store is written once, at the end.
  const paintWidth = useCallback((widthRem: string) => {
    paintedRef.current = widthRem
    wrapperRef.current?.style.setProperty("--sidebar-width", widthRem)
  }, [])

  // For a gesture's whole duration the width does not animate (a tween would trail
  // the finger) and the terminal owes one refit, at the geometry it settles on.
  const gestureRef = useRef(false)
  const beginGesture = useCallback(() => {
    if (gestureRef.current) return
    gestureRef.current = true
    wrapperRef.current?.setAttribute(SIDEBAR_RESIZING_ATTR, "")
    beginLayoutGesture()
  }, [])
  const endGesture = useCallback(() => {
    if (!gestureRef.current) return
    gestureRef.current = false
    wrapperRef.current?.removeAttribute(SIDEBAR_RESIZING_ATTR)
    endLayoutGesture()
  }, [])

  // Every way of moving this divider ends here, so none can disagree about the
  // band or the snap. The paint and the store write carry the SAME value, or a
  // committed width equal to the last rendered one leaves the paint standing.
  const commit = (px: number): boolean => {
    const { widthRem, collapse } = sidebarResizeRelease(px)
    paintWidth(widthRem)
    setSidebarWidth(widthRem, true)
    if (collapse) setOpen(false)
    return collapse
  }

  // Whether the current gesture ever moved the sidebar at all, so a press that
  // went nowhere can put back exactly what it found.
  const draggedRef = useRef(false)

  const ref = useDividerDrag({
    onGrab: () => {
      grabbedPxRef.current = sidebarWidthToPx(widthRef.current)
      draggedRef.current = false
      paintedRef.current = widthRef.current
      beginGesture()
    },
    // Live and unpersisted: the width follows the DELTA from the press, so a press
    // off centre in the grab band does not teleport the divider on the first move.
    onDrag: (deltaX) => {
      draggedRef.current = true
      const { widthRem } = sidebarResizeRelease(grabbedPxRef.current + deltaX)
      paintWidth(widthRem)
    },
    // A press under the shared drag threshold persists nothing and cannot collapse:
    // storage records a width the user chose, and a tap is not a choice.
    onDrop: (deltaX) => {
      if (Math.abs(deltaX) < DIVIDER_DRAG_THRESHOLD_PX) {
        if (draggedRef.current) {
          const { widthRem } = sidebarResizeRelease(grabbedPxRef.current)
          paintWidth(widthRem)
          setSidebarWidth(widthRem)
        }
        endGesture()
        return
      }
      commit(grabbedPxRef.current + deltaX)
      endGesture()
    },
    // A cancelled gesture persists nothing but still squares the store with what
    // the drag painted, or the next unrelated render puts the old width back.
    onCancel: () => {
      if (draggedRef.current) setSidebarWidth(paintedRef.current)
      endGesture()
    },
    // Back to the width the page loaded with, which is exactly what the
    // Changes divider's double-click restores on its side.
    onReset: () => void commit(sidebarWidthToPx(SIDEBAR_INITIAL_WIDTH)),
  })

  // A gesture must not outlive the edge that started it: the drag hook's teardown
  // calls no handler, so an edge unmounted mid-drag would hold the refit for good.
  useEffect(() => {
    wrapperRef.current =
      ref.current?.closest<HTMLElement>('[data-slot="sidebar-wrapper"]') ?? null
    return () => endGesture()
  }, [ref, endGesture])

  useFocusHandoff(focusOnMount, onFocusClaimed, ref)

  // The separator keyboard vocabulary in the sidebar's own units: 1rem a step,
  // not the library's 5% of the window, which would collapse it in one press.
  const onKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    const action = dividerKeyAction(event.key)
    if (!action) return
    event.preventDefault()
    if (action.kind === "toggle") {
      claimFocus()
      setOpen(false)
      return
    }
    const step = action.toEnd
      ? MAX_SIDEBAR_PX - MIN_SIDEBAR_PX
      : SIDEBAR_KEY_STEP_PX
    const collapsed = commit(
      sidebarWidthToPx(widthRef.current) + action.direction * step,
    )
    if (collapsed) claimFocus()
  }

  return (
    <div
      ref={ref}
      data-sidebar="resize-handle"
      role="separator"
      aria-label="Resize sidebar"
      aria-orientation="vertical"
      aria-valuemin={MIN_SIDEBAR_PX}
      aria-valuemax={MAX_SIDEBAR_PX}
      // The COMMITTED width, correct at rest, which is when assistive technology
      // reads it; during a drag it lags the painted width and catches up at the end.
      aria-valuenow={Math.round(sidebarWidthToPx(sidebarWidth))}
      tabIndex={0}
      onKeyDown={onKeyDown}
      className={cn(DIVIDER_CHROME, "absolute inset-y-0 -right-px")}
    />
  )
}

export function AppSidebar() {
  const { spine, bootstrap, selectedTarget, changes } = useDux()
  const sessions = spine?.sessions ?? []
  const projects = spine?.projects ?? []
  const { grouped, withAgents, projectName } = partitionProjects(
    spine?.sidebar,
    projects,
    sessions,
  )
  // The agents `partitionProjects` groups under no project, kept in list order.
  const standaloneSessions = sessions.filter(
    (session) => workspaceProjectId(session.workspace) === null,
  )

  const instanceTitle = resolveInstanceTitle(bootstrap?.title)

  return (
    // The drag edge paints this sidebar's right border, so the container must not
    // draw a second one; the same variant lets tailwind-merge drop the primitive's.
    <Sidebar
      collapsible="icon"
      className="group-data-[side=left]:border-r-0"
    >
      <SidebarHeader>
        <SidebarMenu>
          <SidebarMenuItem className="flex items-center gap-1">
            {/* overflow-visible in icon mode so the collapsed logo's corner
                connection dot is not clipped by the button's rounded box.
                Clicking the brand block goes HOME: selectSession(null) clears
                the selected target (the center pane falls back to the Welcome
                tips, no PTY attached) and rewrites the URL hash back to root,
                the same clear path an agent exit takes. */}
            <SidebarMenuButton
              size="lg"
              aria-label="Go to home"
              onClick={() => selectSession(null)}
              className="flex-1 group-data-[collapsible=icon]:overflow-visible"
            >
              <span className="relative shrink-0">
                <img
                  src="/dux-logo.png"
                  alt="dux"
                  className="size-8 rounded-lg"
                />
                <ConnDot className="absolute -right-0.5 -bottom-0.5 ring-2 ring-sidebar" />
              </span>
              {/* Hidden in icon mode: with overflow-visible (for the dot) the
                  title would otherwise spill past the narrow rail. */}
              <div className="flex min-w-0 flex-1 flex-col gap-0.5 leading-none group-data-[collapsible=icon]:hidden">
                <span className="truncate font-semibold">{instanceTitle}</span>
                <span className="text-sm text-sidebar-foreground/70">
                  {bootstrap?.dux_version}
                </span>
              </div>
            </SidebarMenuButton>
            {/* Collapse toggle: shown only while expanded. When collapsed it
                hides (the row is too narrow) and the rail trigger below takes
                over so the sidebar can still be reopened without the edge handle. */}
            <SidebarTrigger className="shrink-0 group-data-[collapsible=icon]:hidden" />
          </SidebarMenuItem>
          {/* Rail-only expand button: visible ONLY when collapsed, centered under
              the logo, so there is always a discoverable control to reopen the
              sidebar (the edge handle alone was too easy to miss). */}
          <SidebarMenuItem className="hidden group-data-[collapsible=icon]:flex group-data-[collapsible=icon]:justify-center">
            <SidebarTrigger aria-label="Expand sidebar" className="size-8" />
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarHeader>

      <SidebarContent>
        {/* The flat agent list: hidden at icon width, where the rail takes over. */}
        <div className="flex min-h-0 flex-1 flex-col group-data-[collapsible=icon]:hidden">
          <FlatAgentList
            handlers={{
              onSelectSession: selectSession,
              onSelectTerminal: selectTerminal,
            }}
          />
        </div>

        {/* Icon rail: only visible at icon width, flattened to agents. */}
        <CollapsedAgentRail
          projectIds={withAgents}
          grouped={grouped}
          standalone={standaloneSessions}
          projectName={projectName}
          changes={changes}
          selectedTarget={selectedTarget}
        />
      </SidebarContent>

      {/* No @container context: the corner is ONE verb whose label truncates
          plus a fixed-width ⋯ trigger, so nothing stacks and nothing can
          overflow into the center pane at any width the resize handle
          allows. If the corner ever grows a third control, the scaffolding
          (`@container` here, `@[18rem]:` on the row) is what to bring back. */}
      <SidebarFooter>
        <div className="flex flex-col items-stretch gap-2 group-data-[collapsible=icon]:items-center group-data-[collapsible=icon]:justify-center">
          {/* The launcher corner: the shared component, never a copy. */}
          <LauncherCorner className="group-data-[collapsible=icon]:hidden" />
          {/* Collapsed rail: the same two controls as bare icons, stacked. The
              verb does NOT flip here (a rail is too narrow to say what it would
              flip to); everything else, Add project included, is one tap away
              in the ⋯, which renders the very same grouped menu as the corner
              above. */}
          <div className="hidden flex-col items-center gap-2 group-data-[collapsible=icon]:flex">
            <Button
              size="sm"
              aria-label="New agent"
              onClick={() => openNewAgentPicker("new")}
              className="size-8 flex-none p-0"
            >
              <Plus />
            </Button>
            <DropdownMenu>
              {/* Outline like the corner's ⋯ (one primary per cluster; a
                  menu-revealer stays quieter, and outline carries the
                  data-popup-open open tint). */}
              <DropdownMenuTrigger
                render={
                  <Button
                    size="sm"
                    variant="outline"
                    aria-label="More ways to create"
                    className="size-8 flex-none p-0"
                  >
                    <EllipsisVertical />
                  </Button>
                }
              />
              {/* Anchored to the rail's own edge: the rail hugs the left of the
                  window, where an end-aligned popup has nowhere to go. */}
              <DropdownMenuContent align="start" side="top">
                <CreationOverflowMenuItems />
              </DropdownMenuContent>
            </DropdownMenu>
          </div>
        </div>
      </SidebarFooter>
      <SidebarResizeHandle />
    </Sidebar>
  )
}
