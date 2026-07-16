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
  Check,
  ClipboardCopy,
  Cpu,
  Ellipsis,
  EllipsisVertical,
  FileCode2,
  Folder,
  FolderOpen,
  GitFork,
  GitPullRequest,
  Info,
  Pencil,
  Play,
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
import { useEffect, useRef } from "react"
import { agentRowVisual } from "@/lib/agentRow"
import { defaultProviderForSession } from "@/lib/agentTabs"
import { copyToClipboard } from "@/lib/clipboard"
import { resolveInstanceTitle } from "@/lib/instanceTitle"

import { AddProjectMenuItems } from "@/components/AddProjectMenuItems"
import { AgentVitalsTooltip } from "@/components/AgentVitalsTooltip"
import { ConnDot } from "@/components/ConnDot"
import { ProjectMenuItems } from "@/components/ProjectMenuItems"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { StatusBadge } from "@/components/StatusBadge"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { ButtonGroup } from "@/components/ui/button-group"
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
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
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
  SidebarTrigger,
} from "@/components/ui/sidebar"
import { useSidebar } from "@/components/ui/sidebar"
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip"
import { changesCountFor } from "@/lib/agentVitals"
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
  addTab,
  createTerminal,
  openAddProject,
  openAgentEnv,
  openAgentStartupCommand,
  openChangeProvider,
  openEditor,
  openDelete,
  openDeleteTerminal,
  openAgentInfo,
  openForceReconnect,
  openForkAgent,
  openRename,
  openStartupLogs,
  reorderProjects,
  reorderSessions,
  rerunStartupCommand,
  selectSession,
  selectTerminal,
  setProjectOpen,
  setSidebarWidth,
  toggleSessionAutoReopen,
  useDux,
} from "@/lib/store"
import { DEFAULT_AGENT_TABS_MAX } from "@/lib/bootstrapApi"
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
        {/* When no foreground command is running, `title` already equals
            `terminal.label` (see terminalTitle), so the tooltip would just
            repeat the visible text — only show it once the two diverge. */}
        <SimpleTooltip
          content={title !== terminal.label ? terminal.label : null}
          side="right"
        >
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
  projectName,
}: {
  session: SessionView
  selectedTarget: SelectedTarget | null
  projectName: string
}) {
  const label = session.title || session.branch_name
  const agentSelected =
    selectedTarget?.kind === "agent" && selectedTarget.sessionId === session.id
  // Running agents shimmer their name; non-running (detached/exited) recede.
  // `attention` adds a cyan dot when the agent needs the user (permission
  // prompt / finished turn), independent of the working cues.
  const { shimmer, dimmed, attention } = agentRowVisual(
    session.status,
    session.working,
    session.needs_attention
  )

  // "New agent tab" is reachable here at ANY tab count (including the common 1-tab
  // case) because the in-strip "+" only renders once a session already has two
  // or more tabs (AgentTabsStrip mounts only then) — without this menu item a
  // fresh 1-tab session could never reach its first extra tab from the web.
  // Mirrors the strip's own cap/in-flight disabling.
  const { bootstrap, spine, createTabInFlight, changes } = useDux()
  // The changed-files store slice only ever holds data for the currently
  // SELECTED session (see ChangesSlice in lib/store.ts), so a non-selected
  // row's vitals tooltip omits the changes count rather than showing stale or
  // wrong data for an agent that isn't loaded.
  const changesCount = changesCountFor(changes, session.id)
  const tabCap = bootstrap?.agent_tabs_max ?? DEFAULT_AGENT_TABS_MAX
  const atTabCap = session.tabs.length >= tabCap
  const addingTab = createTabInFlight.includes(session.id)
  const providers = bootstrap?.available_providers ?? []
  const defaultProvider = defaultProviderForSession(spine, session)

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
        {/* The "full vitals" tooltip anchors to this WHOLE row button, not just
            the name span: at narrow sidebar widths (min 14rem) a label-anchored
            trigger let the ~w-64 popup open on top of this same row's PR icon,
            status badge, and ⋯ trigger, intercepting their hover/click. Anchored
            to the full-width button, `side="right"` instead opens the popup
            clear of the row's own controls. SimpleTooltip's TooltipTrigger uses
            base-ui's `render` prop (a clone, not a wrapper element), so the
            button's own onClick/attributes/listeners are preserved — the ⋯
            menu's hover-reveal lives on the group/agent-row wrapper below, which
            this button remains inside.

            The tooltip carries a longer (600ms) hover delay than the default
            300ms so scanning down the sidebar doesn't strobe a card per row —
            only a deliberate pause over one agent opens it. */}
        <SimpleTooltip
          content={
            <AgentVitalsTooltip
              session={session}
              projectName={projectName}
              changesCount={changesCount}
            />
          }
          side="right"
          delay={600}
        >
          <SidebarMenuSubButton
            {...attributes}
            {...listeners}
            isActive={agentSelected}
            className={cn(
              "flex-1 touch-manipulation",
              // The wrapper (group/agent-row) owns the highlight now, so keep this
              // button transparent — otherwise it paints a second box that stops
              // short of the trailing ⋯.
              "hover:bg-transparent active:bg-transparent data-active:bg-transparent",
              // Non-running agents recede: dim the whole row (name, icon, and the
              // status indicator) so the running ones read first.
              dimmed && "opacity-70"
            )}
            onClick={() => selectSession(session.id)}
          >
            {/* All agents use the same Bot icon; the provider is shown as text.
                While the agent streams output the icon bobs (motion-safe) so the
                "working" state is unmistakable at a glance. The transition lets it
                settle back to rest (translateY(0)) when streaming stops mid-bounce
                instead of freezing at the top or bottom of the bob.

                The icon doubles as the attention indicator: when the agent needs
                the user it turns cyan and blinks in the same double-pulse-then-
                hold rhythm as the favicon-adjacent web chrome. The blink lives on
                this WRAPPER (opacity) while the bob lives on the inner icon
                (transform), because two Tailwind `animate-*` utilities on one
                element would fight over the `animation` property; nested, the two
                cues mix cleanly. Under reduced motion the icon holds steady cyan.
                COLOR PAIRING: cyan-100, matching `AttentionDot` and
                `ATTENTION_DOT_FILL` in lib/favicon.ts.

                SIZING/COLOR NOTE: wrapping the icon takes it out of reach of the
                sub-button's direct-child `[&>svg]` selectors, so the icon sizes
                itself (`size-4.5`, deliberately a step up from the old 16px) and
                the wrapper carries the color the selector used to apply. */}
            <span
              aria-label={attention ? "Needs attention" : undefined}
              className={cn(
                "inline-flex shrink-0",
                attention
                  ? "text-cyan-100 motion-safe:animate-attention-pulse motion-reduce:animate-none"
                  : "text-sidebar-accent-foreground"
              )}
            >
              <Bot
                className={cn(
                  "size-4.5 shrink-0 motion-safe:transition-transform motion-safe:duration-300",
                  shimmer && "motion-safe:animate-agent-working"
                )}
              />
            </span>
            {/* Its name also dims with a soft white highlight sweeping through (see
                .agent-name-shimmer), a second working cue alongside the bob. The
                base class is always applied so the fill cross-fades back to solid
                text when work stops; `--on` toggles the active sweep. */}
            <span
              className={cn(
                "truncate agent-name-shimmer",
                shimmer && "agent-name-shimmer--on"
              )}
            >
              {label}
            </span>
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
        </SimpleTooltip>

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
              {/* The most common action leads the menu: spawn another provider
                  tab in this agent's worktree. */}
              <DropdownMenuSub>
                <DropdownMenuSubTrigger disabled={atTabCap || addingTab}>
                  <Plus />
                  New agent tab…
                </DropdownMenuSubTrigger>
                <DropdownMenuSubContent>
                  {providers.map((p) => {
                    const isDefault = p === defaultProvider
                    return (
                      <DropdownMenuItem
                        key={p}
                        onClick={() => addTab(session.id, p)}
                      >
                        {isDefault ? <Check /> : <Bot />}
                        {p}
                        {isDefault ? (
                          <span className="ml-auto text-xs text-muted-foreground">
                            default
                          </span>
                        ) : null}
                      </DropdownMenuItem>
                    )
                  })}
                </DropdownMenuSubContent>
              </DropdownMenuSub>
              <DropdownMenuSeparator />
              {/* Connection lifecycle: the force-recreate action (confirmed,
                  since it abandons the current conversation for a fresh
                  session) plus the auto-reopen toggle. */}
              <DropdownMenuItem onClick={() => openForceReconnect(session.id)}>
                <RotateCcw />
                Force recreate agent…
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
              <DropdownMenuItem onClick={() => openAgentInfo(session.id)}>
                <Info />
                Agent info…
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
        <SidebarMenuSub className="mr-0 border-l-0 pr-0 dux-tree">
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
  projectName,
}: {
  projectId: string
  sessions: SessionView[]
  selectedTarget: SelectedTarget | null
  projectName: string
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
        <SidebarMenuSub className="mr-0 border-l-0 pr-0 dux-tree">
          {sessions.map((session) => (
            <SessionSubItem
              key={session.id}
              session={session}
              selectedTarget={selectedTarget}
              projectName={projectName}
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

  // Controlled open state, so a collapse survives re-renders and creating an
  // agent under a collapsed project can force it open. Absent from the store =>
  // default: open when the project has agents, collapsed when it's empty.
  const { projectOpen } = useDux()
  const open = projectOpen?.[id] ?? sessions.length > 0

  return (
    <Collapsible
      open={open}
      onOpenChange={(next) => setProjectOpen(id, next)}
      className="group/collapsible"
    >
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
            {/* The folder carries two signals, both as OUTLINES: open vs closed
                tracks the expand state instead of a chevron (outline silhouettes
                keep the two states distinguishable, unlike the old filled
                variants whose solid shapes read nearly identical), and an
                agent-less project renders dimmed since it has nothing to
                expand. The agent count badge carries the "has agents" signal. */}
            {sessions.length > 0 ? (
              // Crossfade the closed↔open folder on expand instead of an instant
              // swap: both icons are stacked in a fixed-size box and their
              // opacity + a subtle scale transition when the collapsible flips
              // open. Base UI's Collapsible marks the open root with `data-open`
              // (not `data-state=open`), so the reveal keys off that. Respects
              // reduced motion.
              <span className="relative inline-flex size-4 shrink-0">
                <Folder className="absolute inset-0 size-4 transition-[opacity,transform] duration-200 ease-out group-data-[open]/collapsible:scale-90 group-data-[open]/collapsible:opacity-0 motion-reduce:transition-none" />
                <FolderOpen className="absolute inset-0 size-4 scale-90 opacity-0 transition-[opacity,transform] duration-200 ease-out group-data-[open]/collapsible:scale-100 group-data-[open]/collapsible:opacity-100 motion-reduce:transition-none" />
              </span>
            ) : (
              <Folder className="opacity-60" />
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
              projectName={name}
            />
          ) : null}
        </CollapsibleContent>
      </SidebarMenuItem>
    </Collapsible>
  )
}

// The icon rail replaces the grouped project/agent tree when the sidebar
// collapses to `collapsible="icon"` mode. Project headers carry no meaning at
// icon width (they'd just be an unlabeled folder glyph), so instead of
// collapsing to folder icons the rail renders every AGENT across all projects,
// flattened in the same order they appear expanded (project order, then agent
// order within it) — a project with zero agents contributes nothing. Each icon
// keeps the row's working-bob/attention-blink cues and opens the same
// selection the expanded row's click does, so switching agents from the rail
// behaves identically to the expanded tree.
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
  const { shimmer, dimmed, attention } = agentRowVisual(
    session.status,
    session.working,
    session.needs_attention,
  )
  // The changed-files store slice only ever holds data for the currently
  // SELECTED session (see ChangesSlice in lib/store.ts), so a non-selected
  // rail icon's vitals tooltip omits the changes count.
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
          {/* Same wrapper-span pattern as the expanded agent row: the attention
              blink (opacity/color) lives on the wrapper, the working bob
              (transform) lives on the icon itself, so the two `animate-*`
              utilities never fight over the `animation` property.

              SIZING NOTE: unlike the expanded row (which sits inside
              SidebarMenuSubButton and its direct-child `[&>svg]` selector,
              defeated by this same wrapper), this rail button is a plain
              SidebarMenuButton, whose `[&_svg]:size-4` is a DESCENDANT
              selector — it still matches the icon through the wrapper span.
              The wrapper does nothing to stop it here; only equal-or-higher
              specificity wins, so `size-4.5` alone silently loses to it and
              the icon renders 16px instead of the intended 18px. Force it
              with `!important` to match the expanded row. */}
          <span
            aria-label={attention ? "Needs attention" : undefined}
            className={cn(
              "inline-flex shrink-0",
              attention
                ? "text-cyan-100 motion-safe:animate-attention-pulse motion-reduce:animate-none"
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
      // SidebarContent (ui/sidebar.tsx) sets `overflow-hidden` in icon mode —
      // upstream shadcn behavior that keeps a still-animating (width-collapsing)
      // label from bleeding a horizontal scrollbar during the expand/collapse
      // transition. This rail is SidebarContent's only visible child at icon
      // width (the project tree above is `hidden` there), so with that
      // ancestor clipping unconditionally, any list past a screenful (roughly
      // 30 agents at these size-8 rows) was clipped, unreachable, and
      // unclickable below the fold. Rather than loosen the shared
      // SidebarContent rule (and reopen the horizontal-bleed bug it exists to
      // prevent) for every consumer, give this rail its own bounded,
      // vertically scrollable region: `min-h-0` lets it shrink inside the
      // flex column instead of pushing past it, and `overflow-y-auto` scrolls
      // internally before the ancestor's clip boundary is ever reached.
      // `no-scrollbar` keeps the scrollbar-less look consistent with the
      // expanded sidebar's own scroll region.
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

// Edge affordance pinned to the sidebar's right edge. shadcn's `collapsible="icon"`
// only collapses; when expanded this lets the user resize the width by dragging,
// clamped to [14rem, 28rem] and persisted on release. When collapsed it becomes a
// click-to-expand strip (the only edge affordance, replacing the old SidebarRail)
// — it deliberately only expands, never collapses, so a stray click near the
// splitter can no longer shrink the panel. Desktop only: on mobile the sheet owns
// its own open/close, so the strip is suppressed there.
const MIN_SIDEBAR_PX = 14 * 16
const MAX_SIDEBAR_PX = 28 * 16

function SidebarResizeHandle() {
  const { state, isMobile, setOpen } = useSidebar()
  // A drag wires pointermove/up/cancel listeners onto `window`; if the handle
  // unmounts mid-drag (e.g. the sidebar collapses while dragging) React drops the
  // element but those listeners would linger. Tear them down on unmount.
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
        className="absolute inset-y-0 -right-1 z-30 w-1 cursor-e-resize hover:bg-sidebar-border"
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

    // Shared teardown so an interrupted drag (pointercancel — a stolen touch or
    // gesture) cleans up exactly like a normal release; otherwise the listeners
    // would leak and keep mutating the width on later pointer moves.
    const cleanup = () => {
      window.removeEventListener("pointermove", onMove)
      window.removeEventListener("pointerup", onUp)
      window.removeEventListener("pointercancel", cleanup)
      cleanupRef.current = null
    }

    const onUp = (up: PointerEvent) => {
      const px = Math.min(Math.max(up.clientX, MIN_SIDEBAR_PX), MAX_SIDEBAR_PX)
      setSidebarWidth(`${px / 16}rem`, true)
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
              <span className="relative shrink-0">
                <img
                  src="/dux-logo.png"
                  alt="dux"
                  className="size-8 rounded-lg"
                />
                {/* Connection health as a ring-separated badge on the logo
                    corner. It rides the logo (not an inline dot) so it stays
                    visible when the sidebar collapses to icon-only. */}
                <ConnDot className="absolute -right-0.5 -bottom-0.5 ring-2 ring-sidebar" />
              </span>
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
        {/* Grouped project/agent tree: hidden entirely at icon width. Project
            headers carry no meaning as a bare folder glyph, so the icon rail
            below takes over instead of letting these collapse to folder icons. */}
        <SidebarGroup className="group-data-[collapsible=icon]:hidden">
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
          <SidebarGroup className="group-data-[collapsible=icon]:hidden">
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

        {/* Icon rail: only visible at icon width, mirroring the tree above but
            flattened to agents across every project (agent-less projects
            contribute nothing — they have no agents to show). */}
        <CollapsedAgentRail
          projectIds={withAgents}
          grouped={grouped}
          projectName={projectName}
          selectedTarget={selectedTarget}
        />
      </SidebarContent>

      <SidebarFooter>
        {/* Add-project lives next to the collapse toggle (not in the scrolling
            project list, where it slid off-screen once there were enough
            projects). It keeps its "Add project" label whenever the sidebar is
            open and collapses to just the + icon on the icon rail. On mobile the
            hub keeps its own "Add project" entry — this footer is desktop-only. */}
        <div className="flex items-center gap-2 group-data-[collapsible=icon]:flex-col group-data-[collapsible=icon]:justify-center">
          {/* Split button: the primary segment keeps today's one-click "Add
              project"; the attached ⋯ segment opens the add-variants menu.
              Misclick tenet, resolved concretely: the tenet prevents an
              imprecise click from firing a DIFFERENT action. A misclick on the
              ⋯ segment opens a menu (nothing executes; one tap dismisses), and
              every item opens the same non-destructive picker, so the worst
              adjacency outcome is one extra click, never a wrong action. The
              segment gets min-w-8 and the group's border seam as the visual
              separator. */}
          <ButtonGroup className="flex-1 group-data-[collapsible=icon]:hidden">
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
              {/* Open-state styling keys off data-popup-open (base-ui does not
                  flip aria-expanded on an open menu trigger). */}
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
          {/* Collapsed icon rail: the 32px rail cannot hold two honest
              targets, so it keeps today's single + (the menu's actions remain
              reachable by expanding the sidebar or via the picker). */}
          <Button
            variant="outline"
            size="sm"
            aria-label="Add project"
            onClick={openAddProject}
            className="hidden group-data-[collapsible=icon]:flex group-data-[collapsible=icon]:size-8 group-data-[collapsible=icon]:flex-none group-data-[collapsible=icon]:p-0"
          >
            <Plus />
          </Button>
          <SidebarTrigger />
        </div>
      </SidebarFooter>
      <SidebarResizeHandle />
    </Sidebar>
  )
}
