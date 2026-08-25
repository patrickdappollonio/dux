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
import { DIVIDER_CHROME, dividerKeyAction } from "@/lib/paneDivider"
import { useDividerDrag } from "@/hooks/use-divider-drag"
import {
  MAX_SIDEBAR_PX,
  MIN_SIDEBAR_PX,
  SIDEBAR_KEY_STEP_PX,
  sidebarResizeRelease,
  sidebarWidthToPx,
} from "@/lib/sidebarResize"
import { applyPendingOrders } from "@/lib/reorder"
import {
  openNewAgentPicker,
  selectSession,
  selectTerminal,
  setSidebarWidth,
  SIDEBAR_INITIAL_WIDTH,
  useDux,
} from "@/lib/store"
import type { SelectedTarget } from "@/lib/store"
import { cn } from "@/lib/utils"
import type { SessionView } from "@/lib/types"
import { sessionLabel } from "@/lib/agentWorkspace"

// The icon rail replaces the flat agent list when the sidebar collapses to
// `collapsible="icon"` mode. It renders every AGENT across all projects, flattened
// in project-then-agent order, keeping each row's working-bob / attention-blink
// cues and opening the same selection the expanded row's click does.
function CollapsedAgentIcon({
  session,
  projectName,
  selected,
}: {
  session: SessionView
  projectName: string
  selected: boolean
}) {
  const label = sessionLabel(session)
  const { shimmer, dimmed, attention, typing } = agentRowVisual(
    session.status,
    session.working,
    session.needs_attention,
    session.typing,
  )
  const { changes } = useDux()
  const changesCount = changesCountFor(changes, session.id)

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
  selectedTarget,
}: {
  projectIds: string[]
  grouped: Map<string, SessionView[]>
  /** The agents that belong to no project, in list order. They sit under no
   * project row, so grouping by project loses them, and the rail is the ONLY
   * way to reach an agent at icon width: without this they were unreachable
   * without expanding the sidebar. */
  standalone: SessionView[]
  projectName: (id: string) => string
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
      // See the original note: give the rail its own bounded, vertically
      // scrollable region so a long agent list is never clipped below the fold by
      // SidebarContent's icon-mode overflow-hidden.
      className="hidden min-h-0 flex-1 overflow-y-auto no-scrollbar group-data-[collapsible=icon]:flex"
    >
      <SidebarGroupContent>
        <SidebarMenu>
          {entries.map(({ session, projectLabel }) => (
            <CollapsedAgentIcon
              key={session.id}
              session={session}
              projectName={projectLabel}
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

// Edge affordance pinned to the sidebar's right edge: drag-to-resize when
// expanded, click-to-expand when collapsed. Desktop only.
//
// The GESTURE is the shared one: `useDividerDrag` and `DIVIDER_CHROME` are the
// same acquisition, grab band, touch-action suppression, cursor and keyboard
// vocabulary the Changes divider gets from react-resizable-panels, so a finger
// that can move one can move the other. The sidebar keeps its own band and its
// own collapse target (the icon rail rather than nothing), which is the one
// difference between the two dividers; that band lives in lib/sidebarResize.ts.
//
// Rebuilding the sidebar as a resizable panel group instead was evaluated and
// rejected: it would fight SidebarProvider's CSS-variable width model, which
// the collapsed rail and every `group-data-[collapsible=icon]` rule read.
function SidebarResizeHandle() {
  const { state, isMobile } = useSidebar()
  // KEEPING FOCUS ACROSS THE COLLAPSE. Collapsing and expanding swap the drag
  // edge for the expand strip and back, so the control the user was standing on
  // is unmounted by their own keystroke and focus would fall to the body, which
  // sends the next Tab back to the top of the document. This carries the intent
  // across the swap: whichever control mounts next claims it, once.
  //
  // State rather than a ref, because the claim is made in one component's event
  // handler and read during another's mount: the two are in the same commit, so
  // a re-render is exactly the mechanism that gets it there.
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
      // `detail` is 0 only for a click the keyboard synthesised, which is how a
      // button tells Enter and Space apart from a real press. A keyboard
      // expand hands focus on to the drag edge that replaces this strip; a
      // mouse click leaves focus where the mouse put it.
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

  // The live width, and the width the current gesture is measured from. Refs
  // because both change on pointer-move cadence and the listeners below are
  // installed once.
  const widthRef = useRef(sidebarWidth)
  useEffect(() => {
    widthRef.current = sidebarWidth
  })
  const grabbedPxRef = useRef(sidebarWidthToPx(sidebarWidth))

  // Every way of moving this divider ends here, so a drag, an arrow key and a
  // double-click cannot disagree about the band or about when the sidebar snaps
  // to its rail. Reports whether it collapsed, because a keyboard gesture that
  // collapses has to hand focus on to the strip that replaces this edge.
  const commit = (px: number): boolean => {
    const { widthRem, collapse } = sidebarResizeRelease(px)
    setSidebarWidth(widthRem, true)
    if (collapse) setOpen(false)
    return collapse
  }

  const ref = useDividerDrag({
    onGrab: () => {
      grabbedPxRef.current = sidebarWidthToPx(widthRef.current)
    },
    // Live, unpersisted: the width follows the finger by the DELTA from the
    // press, so a press that landed off centre in the grab band does not
    // teleport the divider on the first move.
    onDrag: (deltaX) => {
      const { widthRem } = sidebarResizeRelease(grabbedPxRef.current + deltaX)
      setSidebarWidth(widthRem)
    },
    onDrop: (deltaX) => commit(grabbedPxRef.current + deltaX),
    // A cancelled gesture writes nothing and leaves the sidebar where the
    // finger left it, which is what the panel library's divider does too.
    onCancel: () => {},
    // Back to the width the page loaded with, which is exactly what the
    // Changes divider's double-click restores on its side.
    onReset: () => void commit(sidebarWidthToPx(SIDEBAR_INITIAL_WIDTH)),
  })
  useFocusHandoff(focusOnMount, onFocusClaimed, ref)

  // The library's separator keyboard vocabulary, in the sidebar's own units.
  //
  // A DELIBERATE DEVIATION on the step size. The library steps by 5% of the
  // group its separator splits, which here is the window: on anything wider
  // than 960px that is more than the 48px between the sidebar's default width
  // and its auto-collapse threshold, so a single ArrowLeft would put the whole
  // sidebar away. The sidebar steps by 1rem instead, which is a nudge in a band
  // only 14rem wide. Home and End still run it to its ends.
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
      aria-valuenow={Math.round(sidebarWidthToPx(sidebarWidth))}
      tabIndex={0}
      onKeyDown={onKeyDown}
      className={cn(DIVIDER_CHROME, "absolute inset-y-0 -right-px")}
    />
  )
}

export function AppSidebar() {
  const {
    spine,
    bootstrap,
    selectedTarget,
    pendingSessionOrder,
    pendingProjectOrder,
  } = useDux()
  const rawSessions = spine?.sessions ?? []
  const rawProjects = spine?.projects ?? []
  const { projects, sessions } = applyPendingOrders(
    rawProjects,
    rawSessions,
    pendingSessionOrder,
    pendingProjectOrder,
  )
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
    // The drag edge IS this sidebar's right border now: it paints the same
    // hair-thin line the Changes divider does, in the same token, so the
    // container must not draw a second one a pixel to its left. Written with
    // the same variant the primitive uses, so tailwind-merge drops that one
    // rather than leaving two rules to fight over specificity.
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
