import { Bot, EllipsisVertical, Plus } from "lucide-react"
import type * as React from "react"
import { useEffect, useRef } from "react"

import { agentRowVisual } from "@/lib/agentRow"

import { AddProjectMenuItems } from "@/components/AddProjectMenuItems"
import { NewAgentSplitButton } from "@/components/NewAgentSplitButton"
import { AgentVitalsTooltip } from "@/components/AgentVitalsTooltip"
import { ConnDot } from "@/components/ConnDot"
import { FlatAgentList } from "@/components/FlatAgentList"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import { ButtonGroup } from "@/components/ui/button-group"
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
import {
  MAX_SIDEBAR_PX,
  MIN_SIDEBAR_PX,
  sidebarResizeRelease,
} from "@/lib/sidebarResize"
import { applyPendingOrders } from "@/lib/reorder"
import {
  openAddProject,
  openNewAgentPicker,
  selectSession,
  selectTerminal,
  setSidebarWidth,
  useDux,
} from "@/lib/store"
import type { SelectedTarget } from "@/lib/store"
import { cn } from "@/lib/utils"
import type { SessionView } from "@/lib/types"

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
  const label = session.title || session.branch_name
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
          aria-label={`${label} (${projectName})`}
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
  projectName,
  selectedTarget,
}: {
  projectIds: string[]
  grouped: Map<string, SessionView[]>
  projectName: (id: string) => string
  selectedTarget: SelectedTarget | null
}) {
  const entries = projectIds.flatMap((projectId) =>
    (grouped.get(projectId) ?? []).map((session) => ({ session, projectId })),
  )

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
          {entries.map(({ session, projectId }) => (
            <CollapsedAgentIcon
              key={session.id}
              session={session}
              projectName={projectName(projectId)}
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

// The transparent hit slop both edge affordances wear, so a FINGER can find a
// 4px line. The painted strip stays 1 unit wide; a centred ::after grows to 5
// units (20px, the panel library's own coarse-pointer minimum for a resize
// target) only under a coarse pointer, so a mouse near the sidebar edge keeps
// hitting the content behind it. Same pseudo-element trick as
// components/ui/resizable.tsx.
const EDGE_HIT_SLOP =
  "after:absolute after:inset-y-0 after:left-1/2 after:w-1 after:-translate-x-1/2 pointer-coarse:after:w-5"

// Edge affordance pinned to the sidebar's right edge: drag-to-resize when
// expanded, click-to-expand when collapsed. Desktop only. The clamp band and the
// auto-collapse decision live in lib/sidebarResize.ts so they stay unit-testable.
function SidebarResizeHandle() {
  const { state, isMobile, setOpen } = useSidebar()
  const cleanupRef = useRef<(() => void) | null>(null)
  useEffect(() => () => cleanupRef.current?.(), [])

  if (state === "collapsed") {
    if (isMobile) {
      return null
    }
    return (
      <button
        type="button"
        data-sidebar="expand-handle"
        aria-label="Expand sidebar"
        onClick={() => setOpen(true)}
        className={cn(
          "absolute inset-y-0 -right-1 z-30 w-1 cursor-e-resize hover:bg-sidebar-border",
          EDGE_HIT_SLOP,
        )}
      />
    )
  }

  function handlePointerDown(event: React.PointerEvent<HTMLDivElement>) {
    event.preventDefault()
    const target = event.currentTarget
    target.setPointerCapture(event.pointerId)

    const onMove = (move: PointerEvent) => {
      const px = Math.min(Math.max(move.clientX, MIN_SIDEBAR_PX), MAX_SIDEBAR_PX)
      setSidebarWidth(`${px / 16}rem`)
    }

    // On release, persist the clamped width; if the user dragged below the
    // auto-collapse threshold, snap to the icon rail (same state the footer
    // collapse button / Ctrl-b drive), which the edge's click-to-expand undoes.

    const cleanup = () => {
      window.removeEventListener("pointermove", onMove)
      window.removeEventListener("pointerup", onUp)
      window.removeEventListener("pointercancel", cleanup)
      cleanupRef.current = null
    }

    const onUp = (up: PointerEvent) => {
      const { widthRem, collapse } = sidebarResizeRelease(up.clientX)
      setSidebarWidth(widthRem, true)
      if (collapse) {
        setOpen(false)
      }
      cleanup()
    }

    cleanupRef.current = cleanup
    window.addEventListener("pointermove", onMove)
    window.addEventListener("pointerup", onUp)
    window.addEventListener("pointercancel", cleanup)
  }

  return (
    <div
      data-sidebar="resize-handle"
      onPointerDown={handlePointerDown}
      // `touch-none` is load-bearing, not decoration: without it the browser
      // claims a finger's horizontal drag as a page pan and answers with
      // `pointercancel`, which this handler (correctly) treats as drag-end, so
      // the divider never moves under a finger. The panel library hard-codes
      // the same `touch-action: none` on its own Separator for this exact
      // reason (react-resizable-panels issue 662).
      className={cn(
        "absolute inset-y-0 -right-1 z-30 w-1 cursor-col-resize touch-none hover:bg-sidebar-border",
        EDGE_HIT_SLOP,
      )}
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

  const instanceTitle = resolveInstanceTitle(bootstrap?.title)

  return (
    <Sidebar collapsible="icon">
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
          projectName={projectName}
          selectedTarget={selectedTarget}
        />
      </SidebarContent>

      {/* @container: the footer is a container-query context tracking the
          sidebar's OWN (user-resizable) width, so the two split buttons stack
          when the sidebar is too narrow to fit them side by side instead of
          overflowing into the center pane. */}
      <SidebarFooter className="@container">
        {/* Below the @[18rem] container width the row becomes a full-width
            vertical stack (one on top, one on the bottom); at/above it they sit
            side by side. Icon-rail mode keeps its own centered column. */}
        <div className="flex flex-col items-stretch gap-2 @[18rem]:flex-row @[18rem]:items-center group-data-[collapsible=icon]:flex-col group-data-[collapsible=icon]:items-center group-data-[collapsible=icon]:justify-center">
          {/* Primary New-agent action (moved here from the list header): a split
              button whose ⋯ offers the from-PR / from-worktree variants, beside the
              Add-project split button. Both collapse to bare icons in the rail.
              Full-width when stacked; grows to share the row when side by side. */}
          <NewAgentSplitButton className="w-full @[18rem]:w-auto @[18rem]:flex-1 group-data-[collapsible=icon]:hidden" />
          {/* Collapsed rail: New agent as a bare icon. */}
          <Button
            size="sm"
            aria-label="New agent"
            onClick={() => openNewAgentPicker("new")}
            className="hidden group-data-[collapsible=icon]:flex group-data-[collapsible=icon]:size-8 group-data-[collapsible=icon]:flex-none group-data-[collapsible=icon]:p-0"
          >
            <Plus />
          </Button>
          <ButtonGroup className="w-full @[18rem]:w-fit group-data-[collapsible=icon]:hidden [&>button:last-of-type]:rounded-r-lg">
            <Button
              variant="outline"
              size="sm"
              aria-label="Add project"
              onClick={openAddProject}
              className="flex-1"
            >
              <Plus />
              <span>Add project</span>
            </Button>
            <DropdownMenu>
              <DropdownMenuTrigger
                render={
                  <Button
                    variant="outline"
                    size="sm"
                    aria-label="More ways to add a project"
                    className="min-w-8"
                  >
                    <EllipsisVertical />
                  </Button>
                }
              />
              <DropdownMenuContent align="end" side="top">
                <AddProjectMenuItems />
              </DropdownMenuContent>
            </DropdownMenu>
          </ButtonGroup>
          {/* Collapsed rail: Add project as a bare icon under New agent. */}
          <Button
            variant="outline"
            size="sm"
            aria-label="Add project"
            onClick={openAddProject}
            className="hidden group-data-[collapsible=icon]:flex group-data-[collapsible=icon]:size-8 group-data-[collapsible=icon]:flex-none group-data-[collapsible=icon]:p-0"
          >
            <Plus />
          </Button>
        </div>
      </SidebarFooter>
      <SidebarResizeHandle />
    </Sidebar>
  )
}
