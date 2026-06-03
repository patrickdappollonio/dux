import {
  Bot,
  ChevronRight,
  Ellipsis,
  Folder,
  FolderOpen,
  GitCommitHorizontal,
  GitPullRequest,
  Plus,
  RefreshCw,
  Send,
  SquareTerminal,
  Terminal,
  Wifi,
  WifiOff,
  X,
} from "lucide-react"
import type * as React from "react"

import { Badge } from "@/components/ui/badge"
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty"
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible"
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuAction,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarMenuSub,
  SidebarMenuSubButton,
  SidebarMenuSubItem,
  SidebarRail,
} from "@/components/ui/sidebar"
import { useSidebar } from "@/components/ui/sidebar"
import {
  createTerminal,
  deleteTerminal,
  openCommit,
  selectSession,
  selectTerminal,
  setSidebarWidth,
  socket,
  useDux,
} from "@/lib/store"
import type { SelectedTarget } from "@/lib/store"
import type {
  ConnState,
  PrView,
  SessionStatus,
  SessionView,
  TerminalView,
} from "@/lib/types"

// Map a session status onto a Badge variant + label. Status is communicated as
// a Badge, never as a colored dot.
const STATUS_BADGE: Record<
  SessionStatus,
  { variant: "default" | "secondary" | "outline"; label: string }
> = {
  active: { variant: "default", label: "active" },
  detached: { variant: "secondary", label: "detached" },
  exited: { variant: "outline", label: "exited" },
}

// Return a className for PR badge coloring. This is the ONE intentional
// semantic-color exception: GitHub PR states carry real-world meaning that
// maps directly to green/purple/red (matching the dux TUI colors).
function prBadgeClass(state: PrView["state"]): string {
  if (state === "open") return "border-transparent bg-green-600/15 text-green-500"
  if (state === "merged") return "border-transparent bg-purple-600/15 text-purple-400"
  return "border-transparent bg-red-600/15 text-red-400"
}

// A single companion terminal nested beneath its owning agent session. The
// terminal glyph is reserved for terminals — agents use a provider icon.
function TerminalSubItem({
  terminal,
  sessionId,
  active,
}: {
  terminal: TerminalView
  sessionId: string
  active: boolean
}) {
  return (
    <SidebarMenuSubItem>
      <ContextMenu>
        <ContextMenuTrigger
          render={
            <SidebarMenuSubButton
              isActive={active}
              className="pr-8"
              onClick={() => selectTerminal(terminal.id, sessionId)}
            />
          }
        >
          <SquareTerminal />
          <span className="flex-1 truncate">{terminal.label}</span>
        </ContextMenuTrigger>
        <ContextMenuContent>
          <ContextMenuItem
            className="cursor-pointer text-destructive"
            onClick={() => deleteTerminal(terminal.id)}
          >
            Close terminal
          </ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>
      <SidebarMenuAction
        showOnHover
        title="Close terminal"
        aria-label="Close terminal"
        onClick={(event) => {
          event.stopPropagation()
          deleteTerminal(terminal.id)
        }}
      >
        <X />
      </SidebarMenuAction>
    </SidebarMenuSubItem>
  )
}

function SessionSubItem({
  session,
  selectedTarget,
}: {
  session: SessionView
  selectedTarget: SelectedTarget | null
}) {
  const status = STATUS_BADGE[session.status]
  const label = session.title || session.branch_name
  const agentSelected =
    selectedTarget?.kind === "agent" && selectedTarget.sessionId === session.id

  function handleToggleAutoReopen() {
    socket.sendCommand("toggle_agent_auto_reopen", {
      session_id: session.id,
      enabled: !session.auto_reopen_enabled,
    })
  }

  function handlePush() {
    socket.sendCommand("push", { session_id: session.id })
  }

  return (
    <SidebarMenuSubItem>
      <ContextMenu>
        <ContextMenuTrigger
          render={
            // pr-14 reserves space for the two SidebarMenuAction buttons
            // (⋯ at right-1, + at right-7) so they never overlap the badges.
            <SidebarMenuSubButton
              isActive={agentSelected}
              className="pr-14"
              onClick={() => selectSession(session.id)}
            />
          }
        >
          {/* All agents use the same Bot icon — provider is shown as text. */}
          <Bot />
          <span className="truncate">{label}</span>
          {/* Badges sit inline in the content area so hover actions (SidebarMenuAction)
              have their own right-edge slot and cannot overlap the badges. */}
          <span className="ml-auto flex shrink-0 items-center gap-1">
            {session.pr ? (
              <Badge
                className={prBadgeClass(session.pr.state)}
                title={session.pr.title}
                render={
                  <a
                    href={session.pr.url}
                    target="_blank"
                    rel="noopener noreferrer"
                    onClick={(event) => {
                      event.stopPropagation()
                      window.open(
                        session.pr!.url,
                        "_blank",
                        "noopener",
                      )
                    }}
                  >
                    <GitPullRequest data-icon="inline-start" />#
                    {session.pr.number}
                  </a>
                }
              />
            ) : null}
            <Badge variant={status.variant}>{status.label}</Badge>
          </span>
        </ContextMenuTrigger>
        <ContextMenuContent>
          <ContextMenuItem
            className="cursor-pointer"
            onClick={() => selectSession(session.id)}
          >
            Stream
          </ContextMenuItem>
          <ContextMenuSeparator />
          <ContextMenuItem
            className="cursor-pointer"
            onClick={handleToggleAutoReopen}
          >
            Toggle auto-reopen
          </ContextMenuItem>
          <ContextMenuItem className="cursor-pointer" onClick={handlePush}>
            Push
          </ContextMenuItem>
          <ContextMenuItem
            className="cursor-pointer"
            onClick={() => openCommit(session.id)}
          >
            Commit…
          </ContextMenuItem>
          <ContextMenuSeparator />
          <ContextMenuItem
            className="cursor-pointer"
            onClick={() => createTerminal(session.id)}
          >
            New terminal
          </ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>

      <SidebarMenuAction
        showOnHover
        className="right-7"
        title="New terminal"
        aria-label="New terminal"
        onClick={() => createTerminal(session.id)}
      >
        <Plus />
      </SidebarMenuAction>

      <DropdownMenu>
        <SidebarMenuAction
          showOnHover
          render={<DropdownMenuTrigger />}
          aria-label="Session actions"
        >
          <Ellipsis />
        </SidebarMenuAction>
        <DropdownMenuContent side="right" align="start">
          <DropdownMenuGroup>
            <DropdownMenuItem onClick={() => selectSession(session.id)}>
              <Terminal />
              Stream
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem onClick={handleToggleAutoReopen}>
              <RefreshCw />
              {session.auto_reopen_enabled
                ? "Disable auto-reopen"
                : "Enable auto-reopen"}
            </DropdownMenuItem>
            <DropdownMenuItem onClick={handlePush}>
              <Send />
              Push
            </DropdownMenuItem>
            <DropdownMenuItem onClick={() => openCommit(session.id)}>
              <GitCommitHorizontal />
              Commit…
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem onClick={() => createTerminal(session.id)}>
              <SquareTerminal />
              New terminal
            </DropdownMenuItem>
          </DropdownMenuGroup>
        </DropdownMenuContent>
      </DropdownMenu>

      {session.terminals.length > 0 ? (
        <SidebarMenuSub>
          {session.terminals.map((terminal) => (
            <TerminalSubItem
              key={terminal.id}
              terminal={terminal}
              sessionId={session.id}
              active={
                selectedTarget?.kind === "terminal" &&
                selectedTarget.terminalId === terminal.id
              }
            />
          ))}
        </SidebarMenuSub>
      ) : null}
    </SidebarMenuSubItem>
  )
}

function ProjectItem({
  name,
  sessions,
  selectedTarget,
}: {
  name: string
  sessions: SessionView[]
  selectedTarget: SelectedTarget | null
}) {
  return (
    <Collapsible defaultOpen className="group/collapsible">
      <SidebarMenuItem>
        <CollapsibleTrigger render={<SidebarMenuButton />}>
          <Folder />
          {/* font-semibold makes project names visually distinct from agent rows. */}
          <span className="truncate font-semibold">{name}</span>
          {/* Session count badge sits inline, right after the name. */}
          <Badge variant="secondary" className="shrink-0">{sessions.length}</Badge>
          <ChevronRight className="ml-auto shrink-0 transition-transform group-data-[state=open]/collapsible:rotate-90" />
        </CollapsibleTrigger>
        <CollapsibleContent>
          <SidebarMenuSub>
            {sessions.map((session) => (
              <SessionSubItem
                key={session.id}
                session={session}
                selectedTarget={selectedTarget}
              />
            ))}
          </SidebarMenuSub>
        </CollapsibleContent>
      </SidebarMenuItem>
    </Collapsible>
  )
}

const CONN_BADGE: Record<
  ConnState,
  { variant: "default" | "secondary" | "outline"; label: string }
> = {
  open: { variant: "default", label: "Connected" },
  connecting: { variant: "secondary", label: "Connecting" },
  closed: { variant: "outline", label: "Offline" },
  failed: { variant: "outline", label: "Failed" },
}

function ConnFooter() {
  const { conn } = useDux()
  const badge = CONN_BADGE[conn]
  const WifiIcon = conn === "open" ? Wifi : WifiOff

  return (
    <SidebarMenu>
      <SidebarMenuItem>
        <SidebarMenuButton className="pointer-events-none">
          <WifiIcon />
          <span className="flex-1 truncate">Connection</span>
          <Badge variant={badge.variant}>{badge.label}</Badge>
        </SidebarMenuButton>
      </SidebarMenuItem>
    </SidebarMenu>
  )
}

// Drag handle pinned to the sidebar's right edge. shadcn's `collapsible="icon"`
// only collapses; this lets the user resize the expanded width by dragging,
// clamped to [14rem, 28rem] and persisted on release. Hidden while collapsed.
const MIN_SIDEBAR_PX = 14 * 16
const MAX_SIDEBAR_PX = 28 * 16

function SidebarResizeHandle() {
  const { state } = useSidebar()

  if (state === "collapsed") {
    return null
  }

  function handlePointerDown(event: React.PointerEvent<HTMLDivElement>) {
    event.preventDefault()
    const target = event.currentTarget
    target.setPointerCapture(event.pointerId)

    const onMove = (move: PointerEvent) => {
      const px = Math.min(Math.max(move.clientX, MIN_SIDEBAR_PX), MAX_SIDEBAR_PX)
      setSidebarWidth(`${px / 16}rem`)
    }

    const onUp = (up: PointerEvent) => {
      const px = Math.min(Math.max(up.clientX, MIN_SIDEBAR_PX), MAX_SIDEBAR_PX)
      setSidebarWidth(`${px / 16}rem`, true)
      window.removeEventListener("pointermove", onMove)
      window.removeEventListener("pointerup", onUp)
    }

    window.addEventListener("pointermove", onMove)
    window.addEventListener("pointerup", onUp)
  }

  return (
    <div
      onPointerDown={handlePointerDown}
      className="absolute inset-y-0 -right-1 z-30 w-1 cursor-col-resize hover:bg-sidebar-border"
    />
  )
}

export function AppSidebar() {
  const { viewModel, selectedTarget } = useDux()
  const sessions = viewModel?.sessions ?? []
  const projects = viewModel?.projects ?? []

  // Group sessions by their owning project, preserving first-seen order.
  const order: string[] = []
  const grouped = new Map<string, SessionView[]>()
  for (const session of sessions) {
    let bucket = grouped.get(session.project_id)
    if (!bucket) {
      bucket = []
      grouped.set(session.project_id, bucket)
      order.push(session.project_id)
    }
    bucket.push(session)
  }

  const projectName = (id: string) =>
    projects.find((p) => p.id === id)?.name ?? id.slice(0, 8)

  return (
    <Sidebar collapsible="icon">
      <SidebarHeader>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton size="lg">
              <img src="/dux-logo.png" alt="dux" className="size-8 rounded-lg" />
              <div className="flex flex-1 flex-col gap-0.5 leading-none">
                <span className="font-semibold">dux</span>
                <span className="text-xs text-sidebar-foreground/70">
                  agent sessions
                </span>
              </div>
            </SidebarMenuButton>
          </SidebarMenuItem>
        </SidebarMenu>
      </SidebarHeader>

      <SidebarContent>
        <SidebarGroup>
          <SidebarGroupLabel>Projects</SidebarGroupLabel>
          {order.length === 0 ? (
            <SidebarGroupContent>
              <Empty className="border-0 p-4">
                <EmptyHeader>
                  <EmptyMedia variant="icon">
                    <FolderOpen />
                  </EmptyMedia>
                  <EmptyTitle>No sessions</EmptyTitle>
                  <EmptyDescription>
                    Create an agent in the dux TUI to see it here.
                  </EmptyDescription>
                </EmptyHeader>
              </Empty>
            </SidebarGroupContent>
          ) : (
            <SidebarMenu>
              {order.map((projectId) => (
                <ProjectItem
                  key={projectId}
                  name={projectName(projectId)}
                  sessions={grouped.get(projectId)!}
                  selectedTarget={selectedTarget}
                />
              ))}
            </SidebarMenu>
          )}
        </SidebarGroup>
      </SidebarContent>

      <SidebarFooter>
        <ConnFooter />
      </SidebarFooter>

      <SidebarRail />
      <SidebarResizeHandle />
    </Sidebar>
  )
}
