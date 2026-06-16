import {
  DndContext,
  PointerSensor,
  closestCenter,
  useSensor,
  useSensors,
} from "@dnd-kit/core"
import type { DragEndEvent } from "@dnd-kit/core"
import {
  SortableContext,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable"
import { CSS } from "@dnd-kit/utilities"
import {
  Bot,
  ClipboardCopy,
  Cpu,
  Download,
  Ellipsis,
  FileCode2,
  Folder,
  FolderOpen,
  GitCommitHorizontal,
  GitFork,
  GitPullRequest,
  Pencil,
  Plug,
  Plus,
  RefreshCw,
  RotateCcw,
  Send,
  SquareTerminal,
  Terminal,
  Trash2,
} from "lucide-react"
import { toast } from "sonner"
import type * as React from "react"
import { copyToClipboard } from "@/lib/clipboard"
import { git } from "@/lib/git"

import { AgentBeam } from "@/components/AgentBeam"
import { ProjectMenuItems } from "@/components/ProjectMenuItems"
import { SimpleTooltip } from "@/components/SimpleTooltip"
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
  SidebarTrigger,
} from "@/components/ui/sidebar"
import { useSidebar } from "@/components/ui/sidebar"
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip"
import { prIconClass, prIconHoverClass, prStateLabel } from "@/lib/pr"
import { projectBranchDisplay } from "@/lib/projectBranch"
import type { ProjectBranchDisplay } from "@/lib/projectBranch"
import { partitionProjects } from "@/lib/projects"
import {
  applyPendingOrders,
  moveItem,
  reorderProjectsInGroup,
} from "@/lib/reorder"
import {
  createTerminal,
  openAddProject,
  openChangeProvider,
  openCommit,
  openEditor,
  openDelete,
  openDeleteTerminal,
  openForkAgent,
  openRename,
  reconnectSession,
  reorderProjects,
  reorderSessions,
  selectSession,
  selectTerminal,
  setSidebarWidth,
  socket,
  useDux,
} from "@/lib/store"
import { terminalTitle } from "@/lib/terminals"
import type { SelectedTarget } from "@/lib/store"
import { cn } from "@/lib/utils"
import type { SessionView, TerminalView } from "@/lib/types"

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
  // Title follows the TUI precedence: the foreground command if one is running,
  // otherwise the static label. The static label rides along as the `title`
  // tooltip so "Terminal 1" stays discoverable when a command is shown.
  const title = terminalTitle(terminal)
  return (
    <SidebarMenuSubItem className="flex items-center">
      {/* In-flow ⋯ (mirrors the changes pane): the button is the flex-1 label and
          the ⋯ is a sibling whose max-width expands on reveal, so the label
          re-ellipsizes and slides to make room rather than the ⋯ popping in over
          it. Reveal is scoped to this row's own group/menu-sub-item. */}
      <SidebarMenuSubButton
        isActive={active}
        className="flex-1"
        onClick={() => selectTerminal(terminal.id, sessionId)}
      >
        <SquareTerminal />
        <SimpleTooltip content={terminal.label} side="right">
          <span className="flex-1 truncate">{title}</span>
        </SimpleTooltip>
      </SidebarMenuSubButton>
      {/* ⋯ menu replaces the bare ✕, matching the session rows' pattern: Stream
          selects this terminal (the macro popover lives on the pane, one click
          away after selecting), and Close… routes through the same confirm
          dialog the old ✕ opened. */}
      <DropdownMenu>
        <div className="flex shrink-0 items-center overflow-hidden transition-[max-width,opacity] duration-200 ease-out motion-reduce:transition-none max-md:max-w-none md:max-w-0 md:opacity-0 md:group-hover/menu-sub-item:max-w-6 md:group-hover/menu-sub-item:opacity-100 md:group-focus-within/menu-sub-item:max-w-6 md:group-focus-within/menu-sub-item:opacity-100 md:has-[[data-popup-open]]:max-w-6 md:has-[[data-popup-open]]:opacity-100">
          <SidebarMenuAction
            render={<DropdownMenuTrigger />}
            aria-label="Terminal actions"
            className="static shrink-0"
          >
            <Ellipsis />
          </SidebarMenuAction>
        </div>
        <DropdownMenuContent side="right" align="start">
          <DropdownMenuItem onClick={() => selectTerminal(terminal.id, sessionId)}>
            <Terminal />
            Stream
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          <DropdownMenuItem
            onClick={() => openDeleteTerminal(terminal.id)}
          >
            <Trash2 />
            Close…
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
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

  // The whole row is the drag handle. The enclosing PointerSensor's 6px
  // activation distance keeps a plain click a select, not a drag. `isDragging`
  // dims the lifted row for a clear "this is moving" affordance.
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } =
    useSortable({ id: session.id })
  const style: React.CSSProperties = {
    transform: CSS.Translate.toString(transform),
    transition,
    opacity: isDragging ? 0.6 : undefined,
  }

  function handleToggleAutoReopen() {
    socket.sendCommand("toggle_agent_auto_reopen", {
      session_id: session.id,
      enabled: !session.auto_reopen_enabled,
    })
  }

  function handlePush() {
    git
      .push(session.id)
      .catch((e) => toast.error(e instanceof Error ? e.message : "push failed"))
  }

  function handlePull() {
    git
      .pull(session.id)
      .catch((e) => toast.error(e instanceof Error ? e.message : "pull failed"))
  }

  return (
    <SidebarMenuSubItem ref={setNodeRef} style={style} className="flex items-center">
      {/* In-flow ⋯ (mirrors the changes pane): the button is the flex-1 row and
          the ⋯ is a sibling whose max-width expands on reveal, so the agent's
          right-aligned badges slide left to make room rather than the ⋯ popping
          in over them. Reveal is scoped to this row's own group/menu-sub-item. */}
      <SidebarMenuSubButton
        {...attributes}
        {...listeners}
        isActive={agentSelected}
        className={cn(
          "flex-1 touch-manipulation",
          // Positioning context for the beam overlay. Always relative: the beam
          // self-manages its lifetime (it lingers a moment past `working` to
          // finish its sweep), so the row can't gate the positioning context on
          // `working` without clipping that final pass.
          "relative"
        )}
        onClick={() => selectSession(session.id)}
      >
        {/* A light sweeps left→right across the row while the agent works (and
            finishes its current pass when work stops). Self-manages mount. */}
        <AgentBeam working={session.working} />
        {/* All agents use the same Bot icon — provider is shown as text. While
            the agent is streaming output it gently bounces (motion-safe) so the
            "working" state is unmistakable at a glance. The transition lets the
            icon settle back to rest when streaming stops mid-bounce instead of
            snapping from wherever the keyframe left it. */}
        <Bot
          className={cn(
            "motion-safe:transition-transform motion-safe:duration-300",
            session.working && "motion-safe:animate-agent-working"
          )}
        />
        <span className="truncate">{label}</span>
        <span className="ml-auto flex shrink-0 items-center gap-1">
          {session.pr ? (
            // Icon-only PR link: just the state-tinted glyph, with the full
            // "#N · title" revealed on hover so long PR numbers no longer eat
            // the row. The explicit hover classes fix the washed-out
            // (near-white-on-light-green) hover the old badge had.
            <TooltipProvider delay={300}>
              <Tooltip>
                <TooltipTrigger
                  render={
                    <a
                      href={session.pr.url}
                      target="_blank"
                      rel="noopener noreferrer"
                      aria-label={`PR #${session.pr.number} (${prStateLabel(session.pr.state)})`}
                      className={cn(
                        "inline-flex items-center rounded p-0.5 transition-colors",
                        prIconClass(session.pr.state),
                        prIconHoverClass(session.pr.state)
                      )}
                      onClick={(event) => {
                        event.stopPropagation()
                        window.open(
                          session.pr!.url,
                          "_blank",
                          "noopener",
                        )
                      }}
                    />
                  }
                >
                  <GitPullRequest className="size-3.5" />
                </TooltipTrigger>
                <TooltipContent side="right">
                  #{session.pr.number} · {session.pr.title} (
                  {prStateLabel(session.pr.state)})
                </TooltipContent>
              </Tooltip>
            </TooltipProvider>
          ) : null}
          <StatusBadge
            status={session.status}
            working={session.working}
            iconOnly
          />
        </span>
      </SidebarMenuSubButton>

      <DropdownMenu>
        <div className="flex shrink-0 items-center overflow-hidden transition-[max-width,opacity] duration-200 ease-out motion-reduce:transition-none max-md:max-w-none md:max-w-0 md:opacity-0 md:group-hover/menu-sub-item:max-w-6 md:group-hover/menu-sub-item:opacity-100 md:group-focus-within/menu-sub-item:max-w-6 md:group-focus-within/menu-sub-item:opacity-100 md:has-[[data-popup-open]]:max-w-6 md:has-[[data-popup-open]]:opacity-100">
          <SidebarMenuAction
            render={<DropdownMenuTrigger />}
            aria-label="Session actions"
            className="static shrink-0"
          >
            <Ellipsis />
          </SidebarMenuAction>
        </div>
        <DropdownMenuContent side="right" align="start">
          <DropdownMenuGroup>
            <DropdownMenuItem onClick={() => selectSession(session.id)}>
              <Terminal />
              Stream
            </DropdownMenuItem>
            <DropdownMenuItem onClick={() => reconnectSession(session.id, false)}>
              <Plug />
              Reconnect
            </DropdownMenuItem>
            <DropdownMenuItem onClick={() => reconnectSession(session.id, true)}>
              <RotateCcw />
              Force reconnect (fresh)
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem onClick={() => openRename(session.id)}>
              <Pencil />
              Rename…
            </DropdownMenuItem>
            <DropdownMenuItem onClick={() => openForkAgent(session.id)}>
              <GitFork />
              Fork agent…
            </DropdownMenuItem>
            <DropdownMenuItem onClick={() => openChangeProvider(session.id)}>
              <Cpu />
              Change provider…
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
            <DropdownMenuItem onClick={() => openEditor(session.id)}>
              <FileCode2 />
              Open editor
            </DropdownMenuItem>
            <DropdownMenuItem
              onClick={() => {
                void copyToClipboard(session.worktree_path).then((ok) =>
                  ok
                    ? toast.success("Copied local path to clipboard")
                    : toast.error("Couldn't copy the path"),
                )
              }}
            >
              <ClipboardCopy />
              Copy local path
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem onClick={() => createTerminal(session.id)}>
              <SquareTerminal />
              New terminal
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem
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

// One project's sessions, made sortable within a DndContext scoped to THIS
// project so a session drag never leaks into the project drag (separate
// contexts, distinct sortable ids). On drop it recomputes the project's full
// session order and sends it — the server requires the complete set.
function SessionList({
  projectId,
  sessions,
  selectedTarget,
}: {
  projectId: string
  sessions: SessionView[]
  selectedTarget: SelectedTarget | null
}) {
  // 6px activation distance: a plain click still selects; a small drag starts a
  // reorder. Tuned low so selection feels instant yet drags are intentional.
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 6 } }),
  )

  function handleDragEnd(event: DragEndEvent) {
    const { active, over } = event
    if (!over || active.id === over.id) return
    const ids = sessions.map((s) => s.id)
    reorderSessions(
      projectId,
      moveItem(ids, String(active.id), String(over.id)),
    )
  }

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={closestCenter}
      onDragEnd={handleDragEnd}
    >
      <SortableContext
        items={sessions.map((s) => s.id)}
        strategy={verticalListSortingStrategy}
      >
        {/* mr-0/pr-0 drop the nested list's right inset (the left side is the
            tree indent) so agent rows use the sidebar's full width. */}
        <SidebarMenuSub className="mr-0 pr-0">
          {sessions.map((session) => (
            <SessionSubItem
              key={session.id}
              session={session}
              selectedTarget={selectedTarget}
            />
          ))}
        </SidebarMenuSub>
      </SortableContext>
    </DndContext>
  )
}

function ProjectItem({
  id,
  name,
  branch,
  sessions,
  selectedTarget,
}: {
  id: string
  name: string
  branch: ProjectBranchDisplay | null
  sessions: SessionView[]
  selectedTarget: SelectedTarget | null
}) {
  // Only the project HEADER row is the project drag handle (not the whole
  // block, whose body hosts the sessions' own SortableContext). `isDragging`
  // dims the lifted project for a clear affordance.
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } =
    useSortable({ id })
  const style: React.CSSProperties = {
    transform: CSS.Translate.toString(transform),
    transition,
    opacity: isDragging ? 0.6 : undefined,
  }

  return (
    // Agent-less projects start collapsed — there's nothing inside to show.
    <Collapsible defaultOpen={sessions.length > 0} className="group/collapsible">
      <SidebarMenuItem ref={setNodeRef} style={style}>
        {/* The header is its own flex line with a scoped group: the in-flow ⋯ is
            a sibling of the project button, so on reveal it expands its max-width
            and the flex-1 label + count badge slide to make room (mirroring the
            changes pane). Hover/reveal is scoped to this header group, NOT the
            whole menu-item — whose collapsible agent list would otherwise reveal
            the project ⋯ when an agent row is hovered. */}
        <div className="flex items-center group/project-header">
          <CollapsibleTrigger
            {...attributes}
            {...listeners}
            render={
              <SidebarMenuButton className="min-w-0 flex-1 touch-manipulation group-has-data-[sidebar=menu-action]/menu-item:pr-2" />
            }
          >
            {/* The folder itself signals the expand state — open when the project
                is expanded, closed when collapsed — instead of a chevron. */}
            <Folder className="group-data-[state=open]/collapsible:hidden" />
            <FolderOpen className="hidden group-data-[state=open]/collapsible:block" />
            {/* Name + branch share a baseline-aligned inner flex so the smaller
                text-xs branch sits on the name's baseline instead of floating
                high like a superscript (the outer button is items-center, which
                would vertically-center the two different font sizes). flex-1 lets
                the label fill the row so the count badge rides the right edge and
                slides when the ⋯ opens; min-w-0 lets each span shrink-truncate. */}
            <span className="flex min-w-0 flex-1 items-baseline gap-1.5">
              {/* font-semibold makes project names visually distinct from agent rows. */}
              <span className="min-w-0 truncate font-semibold">{name}</span>
              {/* Current branch as a muted, monospace secondary span after the
                  name. A non-leading branch is tinted with the web's warning
                  convention and explains itself via the title tooltip. Omitted
                  entirely for empty/unknown branches (e.g. path_missing). */}
              {branch ? (
                <SimpleTooltip content={branch.tooltip ?? undefined} side="right">
                  <span
                    className={`min-w-0 truncate font-mono text-sm ${
                      branch.warn ? "text-amber-500" : "text-muted-foreground"
                    }`}
                  >
                    {branch.branch}
                  </span>
                </SimpleTooltip>
              ) : null}
            </span>
            {/* Session count badge rides the right edge (after the flex-1 label)
                so it slides left as the ⋯ opens — omitted for agent-less projects
                (their group heading already says so). */}
            {sessions.length > 0 ? (
              <Badge variant="secondary" className="shrink-0">{sessions.length}</Badge>
            ) : null}
          </CollapsibleTrigger>
          {/* The dropdown trigger is a sibling of the CollapsibleTrigger so its
              click does not toggle the collapsible. */}
          <DropdownMenu>
            <div className="flex shrink-0 items-center overflow-hidden transition-[max-width,opacity] duration-200 ease-out motion-reduce:transition-none max-md:max-w-none md:max-w-0 md:opacity-0 md:group-hover/project-header:max-w-6 md:group-hover/project-header:opacity-100 md:group-focus-within/project-header:max-w-6 md:group-focus-within/project-header:opacity-100 md:has-[[data-popup-open]]:max-w-6 md:has-[[data-popup-open]]:opacity-100">
              <SidebarMenuAction
                render={<DropdownMenuTrigger />}
                aria-label="Project actions"
                className="static shrink-0"
              >
                <Ellipsis />
              </SidebarMenuAction>
            </div>
            <DropdownMenuContent side="right" align="start">
              <ProjectMenuItems id={id} />
            </DropdownMenuContent>
          </DropdownMenu>
        </div>
        <CollapsibleContent>
          {sessions.length > 0 ? (
            <SessionList
              projectId={id}
              sessions={sessions}
              selectedTarget={selectedTarget}
            />
          ) : null}
        </CollapsibleContent>
      </SidebarMenuItem>
    </Collapsible>
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

// One visual project group (with-agents or no-agents) made sortable. Each group
// gets its OWN DndContext so a project drag can't cross group boundaries; on
// drop it splices the group's new internal order back into the full project list
// (`fullOrder`) because the server requires the complete ordered set of ALL
// project ids. A single-item group is rendered without DnD scaffolding (nothing
// to reorder).
function ProjectGroup({
  members,
  fullOrder,
  grouped,
  projectName,
  projectBranch,
  selectedTarget,
}: {
  members: string[]
  fullOrder: string[]
  grouped: Map<string, SessionView[]>
  projectName: (id: string) => string
  projectBranch: (id: string) => ProjectBranchDisplay | null
  selectedTarget: SelectedTarget | null
}) {
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 6 } }),
  )

  function handleDragEnd(event: DragEndEvent) {
    const { active, over } = event
    if (!over || active.id === over.id) return
    reorderProjects(
      reorderProjectsInGroup(
        fullOrder,
        members,
        String(active.id),
        String(over.id),
      ),
    )
  }

  const items = members.map((projectId) => (
    <ProjectItem
      key={projectId}
      id={projectId}
      name={projectName(projectId)}
      branch={projectBranch(projectId)}
      sessions={grouped.get(projectId) ?? []}
      selectedTarget={selectedTarget}
    />
  ))

  return (
    <SidebarMenu>
      <DndContext
        sensors={sensors}
        collisionDetection={closestCenter}
        onDragEnd={handleDragEnd}
      >
        <SortableContext items={members} strategy={verticalListSortingStrategy}>
          {items}
        </SortableContext>
      </DndContext>
    </SidebarMenu>
  )
}

export function AppSidebar() {
  const {
    viewModel,
    selectedTarget,
    pendingSessionOrder,
    pendingProjectOrder,
  } = useDux()
  const rawSessions = viewModel?.sessions ?? []
  const rawProjects = viewModel?.projects ?? []
  // Fold any in-flight drag-and-drop overlay over the server order so the rows
  // don't snap back during the ≤50ms round-trip (see `applyPendingOrders`).
  const { projects, sessions } = applyPendingOrders(
    rawProjects,
    rawSessions,
    pendingSessionOrder,
    pendingProjectOrder,
  )

  const { grouped, withAgents, withoutAgents, realOrder, projectName } =
    partitionProjects(viewModel?.sidebar, projects, sessions)
  // Resolve a project id to its branch-row display (or null when there's
  // nothing to render — empty/unknown branch). Orphan ids (a session whose
  // project is absent) resolve to null, so no stray branch span is emitted.
  const projectBranch = (id: string): ProjectBranchDisplay | null => {
    const project = projects.find((p) => p.id === id)
    return project ? projectBranchDisplay(project) : null
  }

  return (
    <Sidebar collapsible="icon">
      <SidebarHeader>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton size="lg">
              <img src="/dux-logo.png" alt="dux" className="size-8 rounded-lg" />
              <div className="flex flex-1 flex-col gap-0.5 leading-none">
                <span className="font-semibold">dux</span>
                <span className="text-sm text-sidebar-foreground/70">
                  {viewModel?.dux_version}
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
            <ProjectGroup
              members={withAgents}
              fullOrder={realOrder}
              grouped={grouped}
              projectName={projectName}
              projectBranch={projectBranch}
              selectedTarget={selectedTarget}
            />
          )}
        </SidebarGroup>

        {withoutAgents.length > 0 ? (
          // Mirrors the TUI's "Projects with no agents" separator: agent-less
          // projects sink below the active ones under their own heading.
          <SidebarGroup>
            <SidebarGroupLabel>Projects with no agents</SidebarGroupLabel>
            <ProjectGroup
              members={withoutAgents}
              fullOrder={realOrder}
              grouped={grouped}
              projectName={projectName}
              projectBranch={projectBranch}
              selectedTarget={selectedTarget}
            />
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
        {/* Right-aligned when expanded; centered on the icon rail so it lines up
            with the collapsed nav icons above it. */}
        <div className="flex justify-end group-data-[collapsible=icon]:justify-center">
          <SidebarTrigger />
        </div>
      </SidebarFooter>
      <SidebarRail />
      <SidebarResizeHandle />
    </Sidebar>
  )
}
