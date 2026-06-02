import {
  Bot,
  Boxes,
  ChevronRight,
  Ellipsis,
  Folder,
  FolderOpen,
  GitCommitHorizontal,
  RefreshCw,
  Send,
  Sparkles,
  SquareTerminal,
  Terminal,
  Wifi,
  WifiOff,
} from "lucide-react"
import type { ComponentType } from "react"

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
  SidebarMenuBadge,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarMenuSub,
  SidebarMenuSubButton,
  SidebarMenuSubItem,
  SidebarRail,
} from "@/components/ui/sidebar"
import { openCommit, selectSession, socket, useDux } from "@/lib/store"
import type { ConnState, SessionStatus, SessionView } from "@/lib/types"

// Pick a lucide glyph that hints at the provider behind a session.
function providerIcon(provider: string): ComponentType {
  switch (provider.toLowerCase()) {
    case "claude":
      return Bot
    case "codex":
      return SquareTerminal
    case "gemini":
      return Sparkles
    default:
      return Terminal
  }
}

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

function SessionSubItem({
  session,
  selected,
}: {
  session: SessionView
  selected: boolean
}) {
  const Icon = providerIcon(session.provider)
  const status = STATUS_BADGE[session.status]
  const label = session.title || session.branch_name

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
            <SidebarMenuSubButton
              isActive={selected}
              onClick={() => selectSession(session.id)}
            />
          }
        >
          <Icon />
          <span className="flex-1 truncate">{label}</span>
          <Badge variant={status.variant}>{status.label}</Badge>
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
        </ContextMenuContent>
      </ContextMenu>

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
          </DropdownMenuGroup>
        </DropdownMenuContent>
      </DropdownMenu>
    </SidebarMenuSubItem>
  )
}

function ProjectItem({
  name,
  sessions,
  selectedSessionId,
}: {
  name: string
  sessions: SessionView[]
  selectedSessionId: string | null
}) {
  return (
    <Collapsible defaultOpen className="group/collapsible">
      <SidebarMenuItem>
        <CollapsibleTrigger render={<SidebarMenuButton />}>
          <Folder />
          <span className="flex-1 truncate">{name}</span>
          <ChevronRight className="ml-auto transition-transform group-data-[state=open]/collapsible:rotate-90" />
        </CollapsibleTrigger>
        <SidebarMenuBadge>{sessions.length}</SidebarMenuBadge>
        <CollapsibleContent>
          <SidebarMenuSub>
            {sessions.map((session) => (
              <SessionSubItem
                key={session.id}
                session={session}
                selected={session.id === selectedSessionId}
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

export function AppSidebar() {
  const { viewModel, selectedSessionId } = useDux()
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
              <div className="flex aspect-square size-8 items-center justify-center rounded-lg bg-sidebar-primary text-sidebar-primary-foreground">
                <Boxes className="size-4" />
              </div>
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
                  selectedSessionId={selectedSessionId}
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
    </Sidebar>
  )
}
