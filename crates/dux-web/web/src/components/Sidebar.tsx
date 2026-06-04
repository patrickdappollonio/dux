import {
  Bot,
  Download,
  Ellipsis,
  Folder,
  FolderOpen,
  GitCommitHorizontal,
  GitPullRequest,
  Plus,
  RefreshCw,
  Send,
  Settings,
  SquareTerminal,
  Terminal,
  Trash2,
  Wifi,
  WifiOff,
  X,
} from "lucide-react"
import type * as React from "react"

import { StatusBadge } from "@/components/StatusBadge"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty"
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
import { partitionProjects } from "@/lib/projects"
import {
  createTerminal,
  deleteTerminal,
  openAddProject,
  openCommit,
  openCreateAgent,
  openDelete,
  openProjectSettings,
  openRemoveProject,
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
  SessionView,
  TerminalView,
} from "@/lib/types"

// Return a className for PR badge coloring. This is the ONE intentional
// semantic-color exception: GitHub PR states carry real-world meaning that
// maps directly to green/purple/red (matching the dux TUI colors).
function prBadgeClass(state: PrView["state"]): string {
  if (state === "open") return "border-transparent bg-green-600/15 text-green-500"
  if (state === "merged") return "border-transparent bg-purple-600/15 text-purple-400"
  return "border-transparent bg-red-600/15 text-red-400"
}

// A single companion terminal nested beneath its owning agent session. The
// terminal glyph is reserved for companion terminals; agents use a consistent
// Bot icon (provider shown as text).
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
      {/* The close button's slot only exists while THIS row is hovered/focused
          (always on touch layouts), so the label keeps the full width otherwise.
          Visibility is scoped to the row's own group — shadcn's showOnHover keys
          off the ancestor menu-item, which here is the whole project block. */}
      <SidebarMenuSubButton
        isActive={active}
        className="max-md:pr-8 group-focus-within/menu-sub-item:pr-8 group-hover/menu-sub-item:pr-8"
        onClick={() => selectTerminal(terminal.id, sessionId)}
      >
        <SquareTerminal />
        <span className="flex-1 truncate">{terminal.label}</span>
      </SidebarMenuSubButton>
      <SidebarMenuAction
        title="Close terminal"
        aria-label="Close terminal"
        // top-1 vertically centers the 20px action in the 28px sub row; the
        // component's default offsets are calibrated for the taller menu button.
        className="top-1 md:opacity-0 group-focus-within/menu-sub-item:opacity-100 group-hover/menu-sub-item:opacity-100"
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

  function handlePull() {
    socket.sendCommand("pull", { session_id: session.id })
  }

  return (
    <SidebarMenuSubItem>
      {/* The ⋯ slot only exists while THIS row is hovered/focused or its menu
          is open (always on touch layouts), so badges sit flush right
          otherwise. Reveal is scoped to the row's own group — shadcn's
          showOnHover keys off the ancestor menu-item, i.e. the whole project. */}
      <SidebarMenuSubButton
        isActive={agentSelected}
        className="max-md:pr-8 group-focus-within/menu-sub-item:pr-8 group-hover/menu-sub-item:pr-8 group-has-[[aria-expanded=true]]/menu-sub-item:pr-8"
        onClick={() => selectSession(session.id)}
      >
        {/* All agents use the same Bot icon — provider is shown as text. */}
        <Bot />
        <span className="truncate">{label}</span>
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
          <StatusBadge status={session.status} iconOnly />
        </span>
      </SidebarMenuSubButton>

      <DropdownMenu>
        <SidebarMenuAction
          render={<DropdownMenuTrigger />}
          aria-label="Session actions"
          // top-1 vertically centers the 20px action in the 28px sub row; the
          // component's default offsets are calibrated for the taller menu button.
          className="top-1 md:opacity-0 group-focus-within/menu-sub-item:opacity-100 group-hover/menu-sub-item:opacity-100 aria-expanded:opacity-100"
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
            <DropdownMenuItem onClick={handlePull}>
              <Download />
              Pull
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
            <DropdownMenuSeparator />
            <DropdownMenuItem
              className="text-destructive"
              onClick={() => openDelete(session.id)}
            >
              <Trash2 />
              Delete…
            </DropdownMenuItem>
          </DropdownMenuGroup>
        </DropdownMenuContent>
      </DropdownMenu>

      {session.terminals.length > 0 ? (
        // mr-0/pr-0 drop the nested list's right inset (the left side is the
        // tree indent) so terminal rows reach the same right edge as the rest.
        <SidebarMenuSub className="mr-0 pr-0">
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
  id,
  name,
  sessions,
  selectedTarget,
}: {
  id: string
  name: string
  sessions: SessionView[]
  selectedTarget: SelectedTarget | null
}) {
  return (
    // Agent-less projects start collapsed — there's nothing inside to show.
    <Collapsible defaultOpen={sessions.length > 0} className="group/collapsible">
      <SidebarMenuItem>
        <CollapsibleTrigger render={<SidebarMenuButton />}>
          {/* The folder itself signals the expand state — open when the project
              is expanded, closed when collapsed — instead of a chevron. */}
          <Folder className="group-data-[state=open]/collapsible:hidden" />
          <FolderOpen className="hidden group-data-[state=open]/collapsible:block" />
          {/* font-semibold makes project names visually distinct from agent rows. */}
          <span className="truncate font-semibold">{name}</span>
          {/* Session count badge sits inline, right after the name — omitted
              for agent-less projects (their group heading already says so). */}
          {sessions.length > 0 ? (
            <Badge variant="secondary" className="shrink-0">{sessions.length}</Badge>
          ) : null}
        </CollapsibleTrigger>
        {/* The dropdown trigger is a sibling of the CollapsibleTrigger so its
            click does not toggle the collapsible. */}
        <DropdownMenu>
          <SidebarMenuAction
            showOnHover
            render={<DropdownMenuTrigger />}
            aria-label="Project actions"
          >
            <Ellipsis />
          </SidebarMenuAction>
          <DropdownMenuContent side="right" align="start">
            <DropdownMenuItem onClick={() => openCreateAgent(id)}>
              <Bot />
              New agent…
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem onClick={() => openProjectSettings(id)}>
              <Settings />
              Project settings…
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem
              className="text-destructive"
              onClick={() => openRemoveProject(id)}
            >
              <Trash2 />
              Remove project…
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        <CollapsibleContent>
          {sessions.length > 0 ? (
            // mr-0/pr-0 drop the nested list's right inset (the left side is the
            // tree indent) so agent rows use the sidebar's full width.
            <SidebarMenuSub className="mr-0 pr-0">
              {sessions.map((session) => (
                <SessionSubItem
                  key={session.id}
                  session={session}
                  selectedTarget={selectedTarget}
                />
              ))}
            </SidebarMenuSub>
          ) : null}
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

  const { grouped, withAgents, withoutAgents, projectName } = partitionProjects(
    projects,
    sessions,
  )

  const renderProject = (projectId: string) => (
    <ProjectItem
      key={projectId}
      id={projectId}
      name={projectName(projectId)}
      sessions={grouped.get(projectId)!}
      selectedTarget={selectedTarget}
    />
  )

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
          {withAgents.length === 0 && withoutAgents.length === 0 ? (
            <SidebarGroupContent>
              <Empty className="border-0 p-4">
                <EmptyHeader>
                  <EmptyMedia variant="icon">
                    <FolderOpen />
                  </EmptyMedia>
                  <EmptyTitle>No projects</EmptyTitle>
                  <EmptyDescription>
                    Add a project to get started.
                  </EmptyDescription>
                </EmptyHeader>
              </Empty>
            </SidebarGroupContent>
          ) : (
            <SidebarMenu>{withAgents.map(renderProject)}</SidebarMenu>
          )}
        </SidebarGroup>

        {withoutAgents.length > 0 ? (
          // Mirrors the TUI's "Projects with no agents" separator: agent-less
          // projects sink below the active ones under their own heading.
          <SidebarGroup>
            <SidebarGroupLabel>Projects with no agents</SidebarGroupLabel>
            <SidebarMenu>{withoutAgents.map(renderProject)}</SidebarMenu>
          </SidebarGroup>
        ) : null}

        {/* A real button, not a fake list row. Hidden in icon-collapse mode
            (a full-width labeled button can't shrink to the icon rail). */}
        <SidebarGroup className="group-data-[collapsible=icon]:hidden">
          <SidebarGroupContent className="px-1">
            <Button
              variant="outline"
              size="sm"
              className="w-full"
              onClick={openAddProject}
            >
              <Plus />
              Add project
            </Button>
          </SidebarGroupContent>
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
