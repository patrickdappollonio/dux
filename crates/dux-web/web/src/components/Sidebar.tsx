import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "@/components/ui/context-menu"
import {
  Sidebar as SidebarRoot,
  SidebarContent,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarMenu,
  SidebarMenuBadge,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarProvider,
  SidebarRail,
} from "@/components/ui/sidebar"
import { openCommit, selectSession, socket, useDux } from "@/lib/store"
import { cn } from "@/lib/utils"
import type { SessionStatus, SessionView } from "@/lib/types"

const STATUS_DOT: Record<SessionStatus, string> = {
  active: "bg-emerald-500",
  detached: "bg-amber-500",
  exited: "bg-muted-foreground",
}

function SessionRow({
  session,
  selected,
}: {
  session: SessionView
  selected: boolean
}) {
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
    <SidebarMenuItem>
      <ContextMenu>
        <ContextMenuTrigger>
          <SidebarMenuButton
            isActive={selected}
            onClick={() => selectSession(session.id)}
          >
            <span>
              {session.provider} · {session.branch_name}
            </span>
          </SidebarMenuButton>
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
      <SidebarMenuBadge>
        <span
          className={cn("size-1.5 rounded-full", STATUS_DOT[session.status])}
        />
      </SidebarMenuBadge>
    </SidebarMenuItem>
  )
}

export function Sidebar() {
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
    <SidebarProvider className="h-full min-h-0">
      <SidebarRoot collapsible="none" className="h-full w-full">
        <SidebarHeader>
          <SidebarGroupLabel>Sessions</SidebarGroupLabel>
        </SidebarHeader>
        <SidebarContent>
          {order.length === 0 && (
            <SidebarGroup>
              <SidebarGroupContent className="px-2 py-1 text-muted-foreground">
                No sessions yet.
              </SidebarGroupContent>
            </SidebarGroup>
          )}
          {order.map((projectId) => (
            <SidebarGroup key={projectId}>
              <SidebarGroupLabel>{projectName(projectId)}</SidebarGroupLabel>
              <SidebarGroupContent>
                <SidebarMenu>
                  {grouped.get(projectId)!.map((session) => (
                    <SessionRow
                      key={session.id}
                      session={session}
                      selected={session.id === selectedSessionId}
                    />
                  ))}
                </SidebarMenu>
              </SidebarGroupContent>
            </SidebarGroup>
          ))}
        </SidebarContent>
        <SidebarRail />
      </SidebarRoot>
    </SidebarProvider>
  )
}
