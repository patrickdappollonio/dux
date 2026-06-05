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
  ChevronLeft,
  Download,
  Ellipsis,
  FolderOpen,
  GitCommitHorizontal,
  GitFork,
  GitPullRequest,
  Pencil,
  Plug,
  Plus,
  RefreshCw,
  RotateCcw,
  Search,
  Send,
  Settings,
  SquareTerminal,
  Trash2,
  X,
} from "lucide-react"
import type { CSSProperties } from "react"
import { Suspense } from "react"

import { ChangedFiles } from "@/components/ChangedFiles"
import { ChunkBoundary } from "@/components/ChunkBoundary"
import { LazyTerminalPane } from "@/components/LazyTerminalPane"
import { StatusBadge } from "@/components/StatusBadge"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty"
import { ScrollArea } from "@/components/ui/scroll-area"
import { CONN_BADGE } from "@/lib/conn"
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
  mobileNavigate,
  openAddProject,
  openCommit,
  openCreateAgent,
  openDelete,
  openDeleteTerminal,
  openForkAgent,
  openProjectSettings,
  openRemoveProject,
  openRename,
  pullProject,
  reconnectSession,
  reorderProjects,
  reorderSessions,
  selectSession,
  selectTerminal,
  setPaletteOpen,
  socket,
  useDux,
} from "@/lib/store"
import type { SelectedTarget } from "@/lib/store"
import { terminalTitle } from "@/lib/terminals"
import type { PrView, SessionView } from "@/lib/types"

// Mirror the sidebar's PR badge coloring (the one intentional semantic-color
// exception: GitHub PR states carry real-world green/purple/red meaning).
function prBadgeClass(state: PrView["state"]): string {
  if (state === "open") return "border-transparent bg-green-600/15 text-green-500"
  if (state === "merged") return "border-transparent bg-purple-600/15 text-purple-400"
  return "border-transparent bg-red-600/15 text-red-400"
}

// The set of actions the sidebar's ⋯ menu offers for a session, reused verbatim
// here so the mobile menu and the desktop menu never drift.
function SessionActions({ session }: { session: SessionView }) {
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
    <DropdownMenuGroup>
      <DropdownMenuItem onClick={() => selectAndOpen(session.id)}>
        <SquareTerminal />
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
  )
}

// Tapping a session on the hub focuses it AND drives the spoke navigation, so
// the user lands on the full-screen terminal in one tap.
function selectAndOpen(sessionId: string): void {
  selectSession(sessionId)
  mobileNavigate("terminal")
}

function selectTerminalAndOpen(terminalId: string, sessionId: string): void {
  selectTerminal(terminalId, sessionId)
  mobileNavigate("terminal")
}

// One companion terminal nested under its session — touch-sized, with a close
// action spaced away from the row tap target.
function TerminalRow({
  terminal,
  sessionId,
  active,
}: {
  terminal: SessionView["terminals"][number]
  sessionId: string
  active: boolean
}) {
  // Title follows the TUI precedence: foreground command if running, else label.
  const title = terminalTitle(terminal)
  return (
    <div className="flex items-center gap-1 pl-6">
      <Button
        variant={active ? "secondary" : "ghost"}
        className="min-h-11 flex-1 justify-start gap-2 px-2"
        onClick={() => selectTerminalAndOpen(terminal.id, sessionId)}
      >
        <SquareTerminal />
        <span className="flex-1 truncate text-left" title={terminal.label}>
          {title}
        </span>
      </Button>
      <Button
        variant="ghost"
        size="icon"
        className="size-11 shrink-0"
        aria-label="Close terminal"
        onClick={() => openDeleteTerminal(terminal.id)}
      >
        <X />
      </Button>
    </div>
  )
}

// A session row on the hub: tap to stream, with the same ⋯ actions the sidebar
// offers and the companion terminals nested beneath.
function SessionRow({
  session,
  selectedTarget,
}: {
  session: SessionView
  selectedTarget: SelectedTarget | null
}) {
  const label = session.title || session.branch_name
  const agentSelected =
    selectedTarget?.kind === "agent" && selectedTarget.sessionId === session.id

  // Long-press (250ms) starts the drag so taps still select and vertical scroll
  // isn't hijacked — see the sensor config on the enclosing DndContext. The
  // session BUTTON is the handle; nested terminal rows ride along in the node
  // but aren't themselves draggable.
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } =
    useSortable({ id: session.id })
  const style: CSSProperties = {
    transform: CSS.Translate.toString(transform),
    transition,
    opacity: isDragging ? 0.6 : undefined,
  }

  return (
    <div ref={setNodeRef} style={style} className="flex flex-col gap-1">
      <div className="flex items-center gap-1">
        <Button
          {...attributes}
          {...listeners}
          variant={agentSelected ? "secondary" : "ghost"}
          className="min-h-11 flex-1 touch-manipulation justify-start gap-2 px-2"
          onClick={() => selectAndOpen(session.id)}
        >
          <Bot />
          <span className="flex-1 truncate text-left">{label}</span>
          <span className="flex shrink-0 items-center gap-1">
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
                      window.open(session.pr!.url, "_blank", "noopener")
                    }}
                  >
                    <GitPullRequest data-icon="inline-start" />#
                    {session.pr.number}
                  </a>
                }
              />
            ) : null}
            <StatusBadge
              status={session.status}
              working={session.working}
              iconOnly
            />
          </span>
        </Button>
        <DropdownMenu>
          <DropdownMenuTrigger
            render={
              <Button
                variant="ghost"
                size="icon"
                className="size-11 shrink-0"
                aria-label="Session actions"
              />
            }
          >
            <Ellipsis />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <SessionActions session={session} />
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      {session.terminals.map((terminal) => (
        <TerminalRow
          key={terminal.id}
          terminal={terminal}
          sessionId={session.id}
          active={
            selectedTarget?.kind === "terminal" &&
            selectedTarget.terminalId === terminal.id
          }
        />
      ))}
    </div>
  )
}

// A project heading plus its session rows (or "New agent" entry point when it
// has none), with the same ⋯ actions the sidebar's project menu offers. The
// sessions get their OWN DndContext (scoped to this project) so a session drag
// never bubbles into the project drag.
function ProjectBlock({
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
  // The project HEADER is the drag handle (not the whole block, whose body
  // hosts the sessions' own SortableContext). Long-press starts the drag.
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } =
    useSortable({ id })
  const style: CSSProperties = {
    transform: CSS.Translate.toString(transform),
    transition,
    opacity: isDragging ? 0.6 : undefined,
  }

  // Per-project session sensor: long-press (250ms) + 8px tolerance so taps
  // select and scrolling isn't hijacked.
  const sessionSensors = useSensors(
    useSensor(PointerSensor, {
      activationConstraint: { delay: 250, tolerance: 8 },
    }),
  )

  function handleSessionDragEnd(event: DragEndEvent) {
    const { active, over } = event
    if (!over || active.id === over.id) return
    const ids = sessions.map((s) => s.id)
    reorderSessions(id, moveItem(ids, String(active.id), String(over.id)))
  }

  return (
    <div ref={setNodeRef} style={style} className="flex flex-col gap-1">
      <div className="flex items-center gap-1">
        <div
          {...attributes}
          {...listeners}
          className="flex min-h-11 flex-1 touch-manipulation items-center gap-2 px-2"
        >
          <span className="truncate font-semibold">{name}</span>
          {/* Current branch as a muted, monospace secondary span; non-leading
              branches are warning-tinted (amber) with an explanatory title.
              Omitted entirely for empty/unknown branches (e.g. path_missing). */}
          {branch ? (
            <span
              className={`truncate font-mono text-xs ${
                branch.warn ? "text-amber-500" : "text-muted-foreground"
              }`}
              title={branch.tooltip ?? undefined}
            >
              {branch.branch}
            </span>
          ) : null}
          {sessions.length > 0 ? (
            <Badge variant="secondary" className="shrink-0">
              {sessions.length}
            </Badge>
          ) : null}
        </div>
        <DropdownMenu>
          <DropdownMenuTrigger
            render={
              <Button
                variant="ghost"
                size="icon"
                className="size-11 shrink-0"
                aria-label="Project actions"
              />
            }
          >
            <Ellipsis />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem onClick={() => openCreateAgent(id)}>
              <Bot />
              New agent…
            </DropdownMenuItem>
            <DropdownMenuItem onClick={() => pullProject(id)}>
              <Download />
              Pull project…
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
      </div>

      {sessions.length > 0 ? (
        <DndContext
          sensors={sessionSensors}
          collisionDetection={closestCenter}
          onDragEnd={handleSessionDragEnd}
        >
          <SortableContext
            items={sessions.map((s) => s.id)}
            strategy={verticalListSortingStrategy}
          >
            {sessions.map((session) => (
              <SessionRow
                key={session.id}
                session={session}
                selectedTarget={selectedTarget}
              />
            ))}
          </SortableContext>
        </DndContext>
      ) : null}
    </div>
  )
}

// One sortable group of project blocks (with-agents or no-agents). Its OWN
// DndContext keeps a project drag from crossing into the other group; on drop it
// splices the group's new order into the full project list the server requires.
function ProjectGroupList({
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
    useSensor(PointerSensor, {
      activationConstraint: { delay: 250, tolerance: 8 },
    }),
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

  return (
    <DndContext
      sensors={sensors}
      collisionDetection={closestCenter}
      onDragEnd={handleDragEnd}
    >
      <SortableContext items={members} strategy={verticalListSortingStrategy}>
        {members.map((projectId) => (
          <ProjectBlock
            key={projectId}
            id={projectId}
            name={projectName(projectId)}
            branch={projectBranch(projectId)}
            sessions={grouped.get(projectId) ?? []}
            selectedTarget={selectedTarget}
          />
        ))}
      </SortableContext>
    </DndContext>
  )
}

// The hub: a grouped, tappable list of every project and its sessions, mirroring
// the sidebar's partition (projects with agents first, agent-less ones under
// their own heading).
function HomeScreen() {
  const { viewModel, selectedTarget, conn, pendingSessionOrder, pendingProjectOrder } =
    useDux()
  const rawSessions = viewModel?.sessions ?? []
  const rawProjects = viewModel?.projects ?? []
  // Fold the optimistic drag overlays over the server order (see
  // `applyPendingOrders`) so rows don't snap back during the round-trip.
  const { projects, sessions } = applyPendingOrders(
    rawProjects,
    rawSessions,
    pendingSessionOrder,
    pendingProjectOrder,
  )
  const { grouped, withAgents, withoutAgents, projectName } = partitionProjects(
    projects,
    sessions,
  )
  // Resolve a project id to its branch-row display (or null when there's
  // nothing to render). Orphan ids (a session whose project is absent) resolve
  // to null, so no stray branch span is emitted.
  const projectBranch = (id: string): ProjectBranchDisplay | null => {
    const project = projects.find((p) => p.id === id)
    return project ? projectBranchDisplay(project) : null
  }
  const fullOrder = [...withAgents, ...withoutAgents]
  const badge = CONN_BADGE[conn]
  const hasProjects = projects.length > 0 || sessions.length > 0

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      <header className="flex h-12 shrink-0 items-center gap-2 border-b px-3">
        <span className="font-semibold">dux</span>
        <div className="ml-auto flex items-center gap-2">
          <Button
            variant="outline"
            size="icon"
            className="size-11"
            aria-label="Search"
            onClick={() => setPaletteOpen(true)}
          >
            <Search />
          </Button>
          <Badge variant={badge.variant}>{badge.label}</Badge>
        </div>
      </header>

      {hasProjects ? (
        <ScrollArea className="min-h-0 flex-1">
          <div className="flex flex-col gap-4 p-3">
            <div className="flex flex-col gap-2">
              {withAgents.length > 0 ? (
                <p className="px-2 text-xs font-medium text-muted-foreground">
                  Projects
                </p>
              ) : null}
              {withAgents.length > 0 ? (
                <ProjectGroupList
                  members={withAgents}
                  fullOrder={fullOrder}
                  grouped={grouped}
                  projectName={projectName}
                  projectBranch={projectBranch}
                  selectedTarget={selectedTarget}
                />
              ) : null}
            </div>

            {withoutAgents.length > 0 ? (
              <div className="flex flex-col gap-2">
                <p className="px-2 text-xs font-medium text-muted-foreground">
                  Projects with no agents
                </p>
                <ProjectGroupList
                  members={withoutAgents}
                  fullOrder={fullOrder}
                  grouped={grouped}
                  projectName={projectName}
                  projectBranch={projectBranch}
                  selectedTarget={selectedTarget}
                />
              </div>
            ) : null}

            <Button
              variant="outline"
              className="min-h-11 w-full"
              onClick={openAddProject}
            >
              <Plus />
              Add project
            </Button>
          </div>
        </ScrollArea>
      ) : (
        <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-4 p-6">
          <Empty className="border-0">
            <EmptyHeader>
              <EmptyMedia variant="icon">
                <FolderOpen />
              </EmptyMedia>
              <EmptyTitle>No projects</EmptyTitle>
              <EmptyDescription>Add a project to get started.</EmptyDescription>
            </EmptyHeader>
          </Empty>
          <Button
            variant="outline"
            className="min-h-11"
            onClick={openAddProject}
          >
            <Plus />
            Add project
          </Button>
        </div>
      )}
    </div>
  )
}

// The focused-terminal spoke: a slim top bar (back · project·branch · changes
// count · ⋯ actions) over the full-screen shared terminal.
function TerminalScreen() {
  const { viewModel, selectedSessionId, selectedTarget, terminalEpoch } = useDux()
  const session = viewModel?.sessions.find((s) => s.id === selectedSessionId)
  const project = session
    ? viewModel?.projects.find((p) => p.id === session.project_id)
    : undefined
  const changed = viewModel?.changed_files ?? { staged: [], unstaged: [] }
  const changeCount = changed.staged.length + changed.unstaged.length

  // Defensive fallback: the agent exited (TerminalPane reset the selection) or
  // the target was pruned while we sat here. Show the hub content rather than a
  // dead terminal. No navigation side effects in render — the next Back press
  // still unwinds cleanly via the popstate listener.
  if (!selectedTarget || !session) {
    return <HomeScreen />
  }

  const targetId =
    selectedTarget.kind === "terminal"
      ? selectedTarget.terminalId
      : selectedTarget.sessionId
  // Remount on reconnect (see App.tsx TerminalArea): a bumped epoch forces the
  // focused agent pane to re-subscribe to the freshly launched provider.
  const paneKey =
    selectedTarget.kind === "agent" ? `${targetId}:${terminalEpoch}` : targetId

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      <header className="flex h-12 shrink-0 items-center gap-2 border-b px-3">
        <Button
          variant="ghost"
          size="icon"
          className="size-11 shrink-0"
          aria-label="Back"
          onClick={() => history.back()}
        >
          <ChevronLeft />
        </Button>
        <div className="flex min-w-0 flex-1 items-center gap-1 text-sm">
          <span className="truncate">{project?.name ?? "dux"}</span>
          <span className="text-muted-foreground">·</span>
          <span className="truncate font-mono">{session.branch_name}</span>
        </div>
        <Button
          variant="outline"
          size="sm"
          className="min-h-11 shrink-0"
          aria-label={`${changeCount} changed files`}
          onClick={() => mobileNavigate("changes")}
        >
          ±{changeCount}
        </Button>
        <DropdownMenu>
          <DropdownMenuTrigger
            render={
              <Button
                variant="ghost"
                size="icon"
                className="size-11 shrink-0"
                aria-label="Session actions"
              />
            }
          >
            <Ellipsis />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <SessionActions session={session} />
          </DropdownMenuContent>
        </DropdownMenu>
      </header>

      <div className="min-h-0 flex-1">
        {/* Suspense fallback null: the lazy chunk loads fast and TerminalPane
            shows its own readiness spinner once mounted. ChunkBoundary wraps
            Suspense so a stale-bundle import failure recovers instead of
            unmounting the tree. */}
        <ChunkBoundary>
          <Suspense fallback={null}>
            <LazyTerminalPane
              key={paneKey}
              kind={selectedTarget.kind}
              id={targetId}
            />
          </Suspense>
        </ChunkBoundary>
      </div>
    </div>
  )
}

// The changes spoke: a slim back bar over the full-screen shared changed-files
// pane (its diff Sheet is already 90vw-wide, i.e. near-full on phones).
function ChangesScreen() {
  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      <header className="flex h-12 shrink-0 items-center gap-2 border-b px-3">
        <Button
          variant="ghost"
          size="icon"
          className="size-11 shrink-0"
          aria-label="Back"
          onClick={() => history.back()}
        >
          <ChevronLeft />
        </Button>
        <span className="text-sm font-medium">Changes</span>
      </header>
      <div className="min-h-0 flex-1">
        <ChangedFiles />
      </div>
    </div>
  )
}

export function MobileShell() {
  const { mobileScreen } = useDux()

  if (mobileScreen === "terminal") return <TerminalScreen />
  if (mobileScreen === "changes") return <ChangesScreen />
  return <HomeScreen />
}
