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
  Ellipsis,
  FileCode2,
  Folder,
  FolderOpen,
  GitFork,
  GitPullRequest,
  Pencil,
  Play,
  Plug,
  Plus,
  RefreshCw,
  RotateCcw,
  ScrollText,
  SquareChevronRight,
  SquareTerminal,
  Terminal,
  Trash2,
  Variable,
} from "lucide-react"
import { toast } from "sonner"
import type * as React from "react"
import { copyToClipboard } from "@/lib/clipboard"
import { resolveInstanceTitle } from "@/lib/instanceTitle"

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
  openAgentEnv,
  openAgentStartupCommand,
  openChangeProvider,
  openEditor,
  openDelete,
  openDeleteTerminal,
  openForkAgent,
  openRename,
  openStartupLogs,
  reconnectSession,
  reorderProjects,
  reorderSessions,
  rerunStartupCommand,
  selectSession,
  selectTerminal,
  setSidebarWidth,
  toggleSessionAutoReopen,
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
  siblings,
  sessionId,
  active,
}: {
  terminal: TerminalView
  siblings: readonly TerminalView[]
  sessionId: string
  active: boolean
}) {
  // Title is the foreground command when one is running, otherwise the stable
  // "Terminal N" label. When a sibling runs the same app the title gains the
  // terminal's number ("vim (#1)") so the two rows stay distinct. The full
  // "Terminal N" label still rides along as the hover tooltip below.
  const title = terminalTitle(terminal, siblings)
  return (
    <SidebarMenuSubItem
      className={cn(
        // The row owns the hover/selected highlight (rounded, full-width) so it
        // spans the trailing ⋯ too — mirroring the agent rows, project header,
        // and changes pane. The button stays transparent (below) so this is the
        // single highlight surface; pr-1 keeps the ⋯ off the rounded right edge.
        "flex items-center rounded-md pr-1 transition-colors group/terminal-row",
        "hover:bg-sidebar-accent hover:text-sidebar-accent-foreground",
        active && "bg-sidebar-accent text-sidebar-accent-foreground"
      )}
    >
      {/* In-flow ⋯: the button is the flex-1 label and the ⋯ is a sibling whose
          max-width expands on reveal, so the label re-ellipsizes and slides to
          make room. Reveal is scoped to this terminal row (group/terminal-row)
          so hovering the parent agent doesn't reveal it. */}
      <SidebarMenuSubButton
        isActive={active}
        className="flex-1 hover:bg-transparent active:bg-transparent data-active:bg-transparent"
        onClick={() => selectTerminal(terminal.id, sessionId)}
      >
        <SquareTerminal />
        <SimpleTooltip content={terminal.label} side="right">
          <span className="flex-1 truncate">{title}</span>
        </SimpleTooltip>
      </SidebarMenuSubButton>
      {/* ⋯ menu replaces the bare ✕: Stream selects this terminal (the macro
          popover lives on the pane, one click away after selecting) — kept here
          because Close… alone would not warrant a dropdown — and Close… routes
          through the same confirm dialog the old ✕ opened. */}
      <DropdownMenu>
        <div className="flex shrink-0 items-center overflow-hidden transition-[max-width,opacity] duration-200 ease-out motion-reduce:transition-none max-md:max-w-none md:max-w-0 md:opacity-0 md:group-hover/terminal-row:max-w-6 md:group-hover/terminal-row:opacity-100 md:group-focus-within/terminal-row:max-w-6 md:group-focus-within/terminal-row:opacity-100 md:has-[[data-popup-open]]:max-w-6 md:has-[[data-popup-open]]:opacity-100">
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
    toggleSessionAutoReopen(session.id, !session.auto_reopen_enabled)
  }

  return (
    <SidebarMenuSubItem ref={setNodeRef} style={style}>
      {/* The agent button + its ⋯ share ONE flex line inside a scoped group, so
          the ⋯ reveals only when this agent's own header row is hovered — not
          when a nested terminal row below is hovered (mirrors the project
          header's group/project-header). The terminal sub-list is a block
          sibling BELOW this row, so terminals nest UNDER the agent like a tree
          instead of riding alongside it. */}
      <div
        className={cn(
          // The row wrapper owns the hover/selected highlight (rounded,
          // full-width) so it spans the trailing ⋯ too — mirroring the changes
          // pane, where the row (not the inner label button) carries the
          // background. The button below keeps its own background transparent,
          // so this wrapper is the single highlight surface for the whole row.
          "flex items-center rounded-md pr-1 transition-colors group/agent-row",
          "hover:bg-sidebar-accent hover:text-sidebar-accent-foreground",
          agentSelected && "bg-sidebar-accent text-sidebar-accent-foreground"
        )}
      >
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
            "relative",
            // The wrapper (group/agent-row) owns the highlight now, so keep this
            // button transparent — otherwise it paints a second box that stops
            // short of the trailing ⋯.
            "hover:bg-transparent active:bg-transparent data-active:bg-transparent"
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
          <div className="flex shrink-0 items-center overflow-hidden transition-[max-width,opacity] duration-200 ease-out motion-reduce:transition-none max-md:max-w-none md:max-w-0 md:opacity-0 md:group-hover/agent-row:max-w-6 md:group-hover/agent-row:opacity-100 md:group-focus-within/agent-row:max-w-6 md:group-focus-within/agent-row:opacity-100 md:has-[[data-popup-open]]:max-w-6 md:has-[[data-popup-open]]:opacity-100">
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
              {/* Connection lifecycle: reconnect actions plus the auto-reopen
                  toggle, which is just the automatic form of reopening. */}
              <DropdownMenuItem onClick={() => reconnectSession(session.id, false)}>
                <Plug />
                Reconnect
              </DropdownMenuItem>
              <DropdownMenuItem onClick={() => reconnectSession(session.id, true)}>
                <RotateCcw />
                Force reconnect (fresh)
              </DropdownMenuItem>
              <DropdownMenuItem onClick={handleToggleAutoReopen}>
                <RefreshCw />
                {session.auto_reopen_enabled
                  ? "Disable agent auto-reopen"
                  : "Enable agent auto-reopen"}
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              {/* Agent identity and provider. */}
              <DropdownMenuItem onClick={() => openRename(session.id)}>
                <Pencil />
                Rename agent…
              </DropdownMenuItem>
              <DropdownMenuItem onClick={() => openForkAgent(session.id)}>
                <GitFork />
                Fork agent…
              </DropdownMenuItem>
              <DropdownMenuItem onClick={() => openChangeProvider(session.id)}>
                <Cpu />
                Change agent provider…
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              {/* Startup command + env: these are project-scoped (no per-agent
                  env in dux), surfaced here for quick per-agent access mirroring
                  the TUI's palette commands. The dialogs make the project scope
                  explicit. "Rerun" runs the project startup command in THIS
                  agent's worktree; "logs" views its captured output. */}
              <DropdownMenuItem onClick={() => openAgentStartupCommand(session.id)}>
                <SquareChevronRight />
                Configure startup command…
              </DropdownMenuItem>
              <DropdownMenuItem onClick={() => openAgentEnv(session.id)}>
                <Variable />
                Configure environment variables…
              </DropdownMenuItem>
              <DropdownMenuItem onClick={() => rerunStartupCommand(session.id)}>
                <Play />
                Rerun startup command
              </DropdownMenuItem>
              <DropdownMenuItem onClick={() => openStartupLogs(session.id)}>
                <ScrollText />
                Startup command logs…
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              {/* Worktree access: open the agent's worktree in the editor or a
                  terminal, or copy its path. */}
              <DropdownMenuItem onClick={() => openEditor(session.id)}>
                <FileCode2 />
                Open editor
              </DropdownMenuItem>
              <DropdownMenuItem onClick={() => createTerminal(session.id)}>
                <SquareTerminal />
                New terminal
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
              {/* Destructive action, isolated. Deliberately tinted red here (dim
                  at rest, bright on hover) at the user's request — this is the
                  one menu entry that opts out of the neutral-destructive rule;
                  the confirmation dialog still gates it. */}
              <DropdownMenuItem
                variant="destructive"
                className="not-focus:text-destructive/70! not-focus:*:[svg]:text-destructive/70!"
                onClick={() => openDelete(session.id)}
              >
                <Trash2 />
                Delete agent…
              </DropdownMenuItem>
            </DropdownMenuGroup>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      {session.terminals.length > 0 ? (
        // mr-0/pr-0 drop the nested list's right inset (the left side is the
        // tree indent) so terminal rows reach the same right edge as the rest.
        <SidebarMenuSub className="mr-0 pr-0">
          {session.terminals.map((terminal) => (
            <TerminalSubItem
              key={terminal.id}
              terminal={terminal}
              siblings={session.terminals}
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
        <div
          className={cn(
            // The header row owns the hover highlight (rounded, full-width) so it
            // spans the trailing ⋯ too, mirroring the agent rows and the changes
            // pane. The button below stays transparent so this is the single
            // highlight surface; pr-1 keeps the ⋯ off the rounded right edge.
            "flex items-center rounded-md pr-1 transition-colors group/project-header",
            "hover:bg-sidebar-accent hover:text-sidebar-accent-foreground"
          )}
        >
          <CollapsibleTrigger
            {...attributes}
            {...listeners}
            render={
              <SidebarMenuButton className="min-w-0 flex-1 touch-manipulation hover:bg-transparent active:bg-transparent data-active:bg-transparent group-has-data-[sidebar=menu-action]/menu-item:pr-2" />
            }
          >
            {/* The folder doubles as two signals: it is FILLED when the project
                has agents and a shallow outline when it has none, and (for
                projects that do have agents) open vs closed tracks the expand
                state instead of a chevron. Agent-less projects have nothing to
                expand, so they simply stay a closed shallow folder. */}
            {sessions.length > 0 ? (
              // Crossfade the closed↔open folder on expand instead of an instant
              // swap: both icons are stacked in a fixed-size box and their
              // opacity + a subtle scale transition when the collapsible flips
              // open. Base UI's Collapsible marks the open root with `data-open`
              // (not `data-state=open`), so the reveal keys off that. Respects
              // reduced motion.
              <span className="relative inline-flex size-4 shrink-0">
                <Folder
                  fill="currentColor"
                  className="absolute inset-0 size-4 transition-[opacity,transform] duration-200 ease-out group-data-[open]/collapsible:scale-90 group-data-[open]/collapsible:opacity-0 motion-reduce:transition-none"
                />
                <FolderOpen
                  fill="currentColor"
                  className="absolute inset-0 size-4 scale-90 opacity-0 transition-[opacity,transform] duration-200 ease-out group-data-[open]/collapsible:scale-100 group-data-[open]/collapsible:opacity-100 motion-reduce:transition-none"
                />
              </span>
            ) : (
              <Folder />
            )}
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
    spine,
    bootstrap,
    selectedTarget,
    pendingSessionOrder,
    pendingProjectOrder,
  } = useDux()
  const rawSessions = spine?.sessions ?? []
  const rawProjects = spine?.projects ?? []
  // Fold any in-flight drag-and-drop overlay over the server order so the rows
  // don't snap back during the ≤50ms round-trip (see `applyPendingOrders`).
  const { projects, sessions } = applyPendingOrders(
    rawProjects,
    rawSessions,
    pendingSessionOrder,
    pendingProjectOrder,
  )

  const { grouped, withAgents, withoutAgents, realOrder, projectName } =
    partitionProjects(spine?.sidebar, projects, sessions)
  // Resolve a project id to its branch-row display (or null when there's
  // nothing to render — empty/unknown branch). Orphan ids (a session whose
  // project is absent) resolve to null, so no stray branch span is emitted.
  const projectBranch = (id: string): ProjectBranchDisplay | null => {
    const project = projects.find((p) => p.id === id)
    return project ? projectBranchDisplay(project) : null
  }

  const instanceTitle = resolveInstanceTitle(bootstrap?.title)

  return (
    <Sidebar collapsible="icon">
      <SidebarHeader>
        <SidebarMenu>
          <SidebarMenuItem>
            <SidebarMenuButton size="lg">
              <img src="/dux-logo.png" alt="dux" className="size-8 rounded-lg" />
              <div className="flex min-w-0 flex-1 flex-col gap-0.5 leading-none">
                <span className="truncate font-semibold">{instanceTitle}</span>
                <span className="text-sm text-sidebar-foreground/70">
                  {bootstrap?.dux_version}
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

      </SidebarContent>

      <SidebarFooter>
        {/* Add-project lives next to the collapse toggle (not in the scrolling
            project list, where it slid off-screen once there were enough
            projects). It keeps its "Add project" label whenever the sidebar is
            open and collapses to just the + icon on the icon rail. On mobile the
            hub keeps its own "Add project" entry — this footer is desktop-only. */}
        <div className="flex items-center gap-2 group-data-[collapsible=icon]:flex-col group-data-[collapsible=icon]:justify-center">
          <Button
            variant="outline"
            size="sm"
            aria-label="Add project"
            onClick={openAddProject}
            className="flex-1 group-data-[collapsible=icon]:size-8 group-data-[collapsible=icon]:flex-none group-data-[collapsible=icon]:p-0"
          >
            <Plus />
            <span className="group-data-[collapsible=icon]:hidden">Add project</span>
          </Button>
          <SidebarTrigger />
        </div>
      </SidebarFooter>
      <SidebarRail />
      <SidebarResizeHandle />
    </Sidebar>
  )
}
