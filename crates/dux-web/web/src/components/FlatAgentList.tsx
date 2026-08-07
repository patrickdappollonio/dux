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
import {
  ArrowUpDown,
  Bot,
  Check,
  ChevronDown,
  ChevronRight,
  ClipboardCopy,
  Cpu,
  Ellipsis,
  ExternalLink,
  FileCode2,
  Folder,
  GitFork,
  GitPullRequest,
  Info,
  Keyboard,
  KeyboardOff,
  PanelTopClose,
  PanelTopOpen,
  Pencil,
  Play,
  Plus,
  RotateCcw,
  RefreshCw,
  ScrollText,
  Search,
  SquareChevronRight,
  SquareTerminal,
  Trash2,
  Unlink,
  Variable,
  X,
} from "lucide-react"
import type { CSSProperties } from "react"
import { useState } from "react"

import { useIsMobile } from "@/hooks/use-mobile"
import { AgentVitalsTooltip } from "@/components/AgentVitalsTooltip"
import { ProjectMenuItems } from "@/components/ProjectMenuItems"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
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
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty"
import { agentRowVisual } from "@/lib/agentRow"
import { defaultProviderForSession } from "@/lib/agentTabs"
import {
  matchCharRange,
  matchesSessionQuery,
  matchesTerminalQuery,
  normalizeQuery,
} from "@/lib/agentSearch"
import { changesCountFor } from "@/lib/agentVitals"
import { DEFAULT_AGENT_TABS_MAX } from "@/lib/bootstrapApi"
import { clipboardWorktree } from "@/lib/flatClipboard"
import {
  displayedSessionOrder,
  FLAT_SORT_LABELS,
  partitionQuiet,
  quietTailForcedOpen,
  sortMainSessions,
  sortQuietTail,
  stateWord,
  type FlatSortKey,
} from "@/lib/flatList"
import {
  assembleFlatTerminals,
  displayedTerminalOrder,
  sortFlatTerminals,
  terminalStateWord,
  type FlatTerminal,
} from "@/lib/flatTerminals"
import { prIconClass, prIconHoverClass, prStateLabel } from "@/lib/pr"
import { partitionProjects } from "@/lib/projects"
import { moveItem, ordersMatch, reorderById } from "@/lib/reorder"
import {
  addTab,
  agentSortValue,
  createTerminal,
  detachPullRequest,
  mobileAccessoryBarVisible,
  mobileTopBarVisible,
  setMobileBarVisibility,
  openAgentEnv,
  openAgentInfo,
  openAgentStartupCommand,
  openAttachPullRequest,
  openChangeProvider,
  openDelete,
  openDeleteTerminal,
  openEditor,
  standaloneEditorHash,
  openForceReconnect,
  openForkAgent,
  openRename,
  openStartupLogs,
  reorderAgents,
  reorderTerminals,
  rerunStartupCommand,
  setAgentSearch,
  setAgentSort,
  toggleSessionAutoReopen,
  useDux,
} from "@/lib/store"
import { terminalForeground, terminalTitle } from "@/lib/terminals"
import type { SelectedTarget, TerminalOwnerRef } from "@/lib/store"
import type { SessionView, TerminalView } from "@/lib/types"
import { cn } from "@/lib/utils"

// How a flat row's tap resolves. Desktop just selects; mobile also drives the
// hub-to-terminal navigation. Passed in so the one shared list serves both.
export interface FlatSelectHandlers {
  onSelectSession: (sessionId: string) => void
  onSelectTerminal: (terminalId: string, owner: TerminalOwnerRef) => void
}

// The shared agent ⋯ actions menu body, every per-agent action from the parity
// inventory, in one place so desktop and mobile can never drift. Reused verbatim
// by both surfaces (this is the SessionActions menu the redesign must preserve).
//
// `context` says which screen the menu opened FROM: `"hub"` (the default; the
// hub/sidebar row menus on both surfaces) or `"terminal"` (the mobile terminal
// screen's header menu, passed by MobileShell). The mobile-bar quick toggles
// render only in the terminal context, because they toggle chrome that only
// the terminal screen shows: an unscoped gate would leak them into the hub's
// row menus (and the desktop sidebar), where that chrome is not even visible.
export function AgentActionsMenu({
  session,
  context = "hub",
}: {
  session: SessionView
  context?: "hub" | "terminal"
}) {
  const duxState = useDux()
  const { bootstrap, spine, createTabInFlight } = duxState
  const tabCap = bootstrap?.agent_tabs_max ?? DEFAULT_AGENT_TABS_MAX
  const atTabCap = session.tabs.length >= tabCap
  const addingTab = createTabInFlight.includes(session.id)
  const providers = bootstrap?.available_providers ?? []
  const defaultProvider = defaultProviderForSession(spine, session)
  const topBarVisible = mobileTopBarVisible(duxState)
  const accessoryBarVisible = mobileAccessoryBarVisible(duxState)
  // The quick toggles need the viewport too, not just the context: the chrome
  // they hide is mobile-only, so a desktop viewport must never offer them
  // even when a terminal-context menu renders.
  const isMobile = useIsMobile()
  const ghAvailable = bootstrap?.gh_available ?? false
  const prOverridden = session.pr?.overridden ?? false

  return (
    <DropdownMenuGroup>
      {context === "terminal" && isMobile ? (
        <>
          {/* Quick toggles for the two hideable mobile bars (`ui.mobile_top_bar`,
              `ui.mobile_accessory_bar`). Neutral color and no trailing ellipsis:
              they act immediately (an optimistic override plus the generic
              settings PATCH), no dialog and nothing destructive. Restore lives
              on the show-bars button below the terminal and in Preferences. */}
          <DropdownMenuItem
            onClick={() => void setMobileBarVisibility("top", !topBarVisible)}
          >
            {topBarVisible ? <PanelTopClose /> : <PanelTopOpen />}
            {topBarVisible ? "Hide top bar" : "Show top bar"}
          </DropdownMenuItem>
          <DropdownMenuItem
            onClick={() =>
              void setMobileBarVisibility("accessory", !accessoryBarVisible)
            }
          >
            {accessoryBarVisible ? <KeyboardOff /> : <Keyboard />}
            {accessoryBarVisible ? "Hide terminal keys" : "Show terminal keys"}
          </DropdownMenuItem>
          <DropdownMenuSeparator />
        </>
      ) : null}
      <DropdownMenuSub>
        <DropdownMenuSubTrigger disabled={atTabCap || addingTab}>
          <Plus />
          New agent tab…
        </DropdownMenuSubTrigger>
        <DropdownMenuSubContent>
          {providers.map((p) => {
            const isDefault = p === defaultProvider
            return (
              <DropdownMenuItem key={p} onClick={() => addTab(session.id, p)}>
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
      {/* Project actions live here (and in the New-agent picker) now that the
          flat list has no project header. */}
      <DropdownMenuSub>
        <DropdownMenuSubTrigger>
          <Folder />
          Project…
        </DropdownMenuSubTrigger>
        <DropdownMenuSubContent>
          <ProjectMenuItems id={session.project_id} />
        </DropdownMenuSubContent>
      </DropdownMenuSub>
      <DropdownMenuSeparator />
      <DropdownMenuItem onClick={() => openForceReconnect(session.id)}>
        <RotateCcw />
        Force recreate agent…
      </DropdownMenuItem>
      <DropdownMenuItem onClick={() => toggleSessionAutoReopen(session.id, !session.auto_reopen_enabled)}>
        <RefreshCw />
        {session.auto_reopen_enabled
          ? "Disable agent auto-reopen"
          : "Enable agent auto-reopen"}
      </DropdownMenuItem>
      <DropdownMenuSeparator />
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
      {/* GitHub-gated, like the project menu's from-PR item: without a usable
          gh there is nothing to attach. The label flips on the OVERRIDE (a
          manually attached PR), not on mere PR presence, because attaching
          over an autodetected badge is still a first manual attach. */}
      {ghAvailable && (
        <DropdownMenuItem onClick={() => openAttachPullRequest(session.id)}>
          <GitPullRequest />
          {prOverridden
            ? "Change attached pull request…"
            : "Attach pull request…"}
        </DropdownMenuItem>
      )}
      {/* No confirm and no ellipsis: detaching is reversible (autodetection
          resumes, and the PR can be re-attached any time). */}
      {prOverridden && (
        <DropdownMenuItem onClick={() => detachPullRequest(session.id)}>
          <Unlink />
          Detach pull request
        </DropdownMenuItem>
      )}
      <DropdownMenuItem onClick={() => openAgentInfo(session.id)}>
        <Info />
        Agent info…
      </DropdownMenuItem>
      <DropdownMenuSeparator />
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
      {/* Two editor entries, named to distinguish their surfaces. The in-app
          overlay cannot open on a phone (EditorOverlay is desktop-only), so
          its item is CSS-hidden there rather than left as a dead no-op; the
          new-tab item, which opens the standalone surface, is the only
          editor entry on phones. Final copy was left to PR review. */}
      <DropdownMenuItem
        className="max-md:hidden"
        onClick={() => openEditor(session.id)}
      >
        <FileCode2 />
        Open editor here
      </DropdownMenuItem>
      {/* A real anchor, matching the editor header's affordance: middle-click
          and ctrl/cmd-click keep their native new-tab semantics, which a
          window.open handler would flatten. */}
      <DropdownMenuItem
        render={
          <a
            href={standaloneEditorHash(session.id)}
            target="_blank"
            rel="noopener"
          />
        }
      >
        <ExternalLink />
        Open editor in new tab
      </DropdownMenuItem>
      <DropdownMenuItem onClick={() => createTerminal(session.id)}>
        <SquareTerminal />
        New terminal
      </DropdownMenuItem>
      <DropdownMenuItem onClick={() => clipboardWorktree(session.worktree_path)}>
        <ClipboardCopy />
        Copy local path
      </DropdownMenuItem>
      <DropdownMenuSeparator />
      {/* The one deliberate red-tinted destructive menu item (dim at rest, bright
          on hover), per the CLAUDE.md web-UI menu tenet; the confirm dialog gates it. */}
      <DropdownMenuItem
        variant="destructive"
        className="not-focus:text-destructive/70! not-focus:*:[svg]:text-destructive/70!"
        onClick={() => openDelete(session.id)}
      >
        <Trash2 />
        Delete agent…
      </DropdownMenuItem>
    </DropdownMenuGroup>
  )
}

// Display-only project label on a row's second line. It shows which project the
// agent belongs to; the project's ACTIONS live in the agent's ⋯ menu (a "Project"
// submenu) and in the New-agent picker, so this label stays a plain span and the
// whole row remains one clean click target for selecting the agent. The project
// name is a searched field, so a query hit inside it gets the match emphasis
// (a result that matched on its project must explain itself).
function ProjectTag({ name, query }: { name: string; query: string }) {
  return (
    <span className="flex min-w-0 shrink items-center gap-1 text-muted-foreground">
      <Folder className="size-3 shrink-0" />
      <span className="min-w-0 truncate">
        <HighlightedText text={name} query={query} />
      </span>
    </span>
  )
}

function Dot({ className }: { className: string }) {
  return <span className={cn("size-1 shrink-0 rounded-full bg-current opacity-50", className)} />
}

// The typing cue: a thin blinking caret in the soft-violet typing token, shared by
// the agent and terminal rows so the two surfaces (and the two row kinds) never
// drift. Distinct from the working bob/shimmer so "typing" and "working" read
// differently. Motion-reduce drops the blink and rests the caret fully opaque.
function TypingCaret() {
  return (
    <span
      aria-hidden
      className="inline-block h-3.5 w-0.5 shrink-0 rounded-full bg-dux-typing align-middle motion-safe:animate-typing-caret motion-reduce:opacity-100"
    />
  )
}

// A row label with the part the live search query matched wrapped in a
// token-styled emphasis span (bg-primary at low alpha, never a hardcoded
// color). The range comes from the pure `matchCharRange` (code-point safe, the
// TS twin of dux-core's `match_char_range`), computed against the DISPLAYED
// string only, so nothing highlights that the filter did not match on this
// row's visible text. Splitting via Array.from keeps emoji/CJK intact.
function HighlightedText({ text, query }: { text: string; query: string }) {
  const range = matchCharRange(text, query)
  if (!range) return <>{text}</>
  const chars = Array.from(text)
  return (
    <>
      {chars.slice(0, range.start).join("")}
      <span className="rounded-[2px] bg-primary/25">
        {chars.slice(range.start, range.end).join("")}
      </span>
      {chars.slice(range.end).join("")}
    </>
  )
}

// The two-line agent row: line one is the Bot (with the verbatim working bob +
// attention pulse + name shimmer cues) + name + PR link + relative time; line two
// is the clickable project tag, a colored state word, and (when they diverge) the
// branch and a tab count. Uses ONLY fields that exist today.
function AgentFlatRow({
  session,
  projectName,
  selectedTarget,
  handlers,
  sortable,
  query,
}: {
  session: SessionView
  projectName: string
  selectedTarget: SelectedTarget | null
  handlers: FlatSelectHandlers
  sortable: boolean
  // The live search query, for the match highlight ("" renders plain).
  query: string
}) {
  const label = session.title || session.branch_name
  const agentSelected =
    selectedTarget?.kind === "agent" && selectedTarget.sessionId === session.id
  const { shimmer, dimmed, attention, typing } = agentRowVisual(
    session.status,
    session.working,
    session.needs_attention,
    session.typing,
  )
  const word = stateWord(session)
  const branchDiverges = session.branch_name !== label
  const tabCount = session.tabs.length

  const { changes } = useDux()
  const changesCount = changesCountFor(changes, session.id)

  const { attributes, listeners, setNodeRef, transform, transition, isDragging } =
    useSortable({ id: session.id })
  const style: CSSProperties = {
    // Vertical reorder list: lock the drag to the Y axis. Without this, dragging
    // right translates the row past the sidebar's edge (the scroll container is
    // overflow-x visible), so it flies out over the center pane. Zeroing x keeps
    // every row inside the column; siblings only ever shift vertically anyway.
    transform: transform
      ? `translate3d(0, ${Math.round(transform.y)}px, 0)`
      : undefined,
    transition,
    opacity: isDragging ? 0.6 : undefined,
  }
  const dragProps = sortable ? { ...attributes, ...listeners } : {}

  return (
    <div ref={setNodeRef} style={style} className="flex flex-col">
      <div
        className={cn(
          // The wrapper owns the highlight (rounded, full-width) so it spans both
          // lines AND the trailing ⋯, matching the app's other rows. The button
          // below is transparent and fills the row, so a click anywhere on either
          // line selects the agent.
          "group/flat-row relative flex items-stretch rounded-md pr-1 transition-colors",
          "hover:bg-sidebar-accent hover:text-sidebar-accent-foreground",
          agentSelected && "bg-sidebar-accent text-sidebar-accent-foreground",
        )}
      >
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
          <button
            {...dragProps}
            type="button"
            onClick={() => handlers.onSelectSession(session.id)}
            className={cn(
              "flex min-w-0 flex-1 touch-manipulation items-start gap-2.5 py-2 pl-2 text-left max-md:min-h-11",
              dimmed && "opacity-70",
            )}
          >
            <span
              aria-label={attention ? "Needs attention" : undefined}
              className={cn(
                "mt-0.5 inline-flex shrink-0",
                attention
                  ? "text-cyan-100 motion-safe:animate-attention-pulse motion-reduce:animate-none"
                  : "text-sidebar-accent-foreground",
              )}
            >
              <Bot
                className={cn(
                  "size-4.5 shrink-0 motion-safe:transition-transform motion-safe:duration-300",
                  shimmer && "motion-safe:animate-agent-working",
                )}
              />
            </span>
            <span className="flex min-w-0 flex-1 flex-col gap-0.5">
              {/* Line one: name + PR + time. */}
              <span className="flex items-center gap-2">
                <span
                  className={cn(
                    "min-w-0 flex-1 truncate text-sm agent-name-shimmer",
                    shimmer && "agent-name-shimmer--on",
                  )}
                >
                  <HighlightedText text={label} query={query} />
                </span>
                {/* Typing cue: the violet caret next to the name (working's bob +
                    shimmer are suppressed while typing, so this is the sole cue). */}
                {typing ? <TypingCaret /> : null}
                {session.pr ? (
                  <SimpleTooltip
                    content={`#${session.pr.number} · ${session.pr.title} (${prStateLabel(session.pr.state)})`}
                    side="right"
                  >
                    <a
                      href={session.pr.url}
                      target="_blank"
                      rel="noopener noreferrer"
                      aria-label={`PR #${session.pr.number} (${prStateLabel(session.pr.state)})`}
                      className={cn(
                        "inline-flex shrink-0 items-center gap-0.5 rounded px-1 py-0.5 transition-colors",
                        prIconClass(session.pr.state),
                        prIconHoverClass(session.pr.state),
                      )}
                      onClick={(event) => {
                        // `stopPropagation` keeps the click off the row's own
                        // select handler. `preventDefault` is what keeps this to
                        // ONE tab: the anchor already carries `target="_blank"`,
                        // so without it the browser follows the href as well and
                        // the explicit `window.open` below opens a second tab.
                        // The open stays explicit because this anchor is nested
                        // inside the row's button, where the native default is
                        // not dependable.
                        event.preventDefault()
                        event.stopPropagation()
                        window.open(session.pr!.url, "_blank", "noopener")
                      }}
                    >
                      <GitPullRequest className="size-3.5" />
                      <span className="text-xs font-medium tabular-nums">
                        #{session.pr.number}
                      </span>
                    </a>
                  </SimpleTooltip>
                ) : null}
              </span>
              {/* Line two: display-only project + state word + branch + tabs.
                  Sans throughout to match the app; only the branch is mono. */}
              <span className="flex items-center gap-1.5 text-xs text-muted-foreground">
                <ProjectTag name={projectName} query={query} />
                <Dot className="text-muted-foreground" />
                {/* Keyed on the label so a state change (Working ⇄ Idle ⇄ Detached
                    …) remounts the span and replays the one-shot fade+rise instead
                    of snapping the text. */}
                <span
                  key={word.label}
                  className={cn(
                    "shrink-0 font-medium motion-safe:animate-state-word",
                    word.className,
                  )}
                >
                  {word.label}
                </span>
                {branchDiverges ? (
                  <>
                    <Dot className="text-muted-foreground" />
                    <span className="min-w-0 truncate font-mono">
                      <HighlightedText text={session.branch_name} query={query} />
                    </span>
                  </>
                ) : null}
                {tabCount > 1 ? (
                  <>
                    <Dot className="text-muted-foreground" />
                    <span className="shrink-0">{tabCount} tabs</span>
                  </>
                ) : null}
              </span>
            </span>
          </button>
        </SimpleTooltip>

        <DropdownMenu>
          <div className="flex shrink-0 items-center overflow-hidden transition-[max-width,opacity] duration-200 ease-out motion-reduce:transition-none max-md:max-w-none md:max-w-0 md:opacity-0 md:group-hover/flat-row:max-w-8 md:group-hover/flat-row:opacity-100 md:group-focus-within/flat-row:max-w-8 md:group-focus-within/flat-row:opacity-100 md:has-[[data-popup-open]]:max-w-8 md:has-[[data-popup-open]]:opacity-100">
            <DropdownMenuTrigger
              render={
                <Button
                  variant="ghost"
                  size="icon"
                  className="size-7 shrink-0 max-md:size-10"
                  aria-label="Session actions"
                />
              }
            >
              <Ellipsis />
            </DropdownMenuTrigger>
          </div>
          <DropdownMenuContent side="right" align="start">
            <AgentActionsMenu session={session} />
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </div>
  )
}

// The two-line terminal row, mirroring the agent row's shape. Line one: the
// terminal icon + primary label (the foreground command when something is running,
// via `terminalTitle`, else the shell label), with the working shimmer / typing
// caret cues. Line two: `↳ {ownerLabel} · {stateWord}`, the owner being the agent
// name (session terminal) or the project name (project terminal), and the state
// word one of Typing / Working / Idle (terminals have no detached/exited/attention).
function TerminalFlatRow({
  terminal,
  siblings,
  owner,
  ownerLabel,
  active,
  onSelect,
  sortable,
  query,
}: {
  terminal: TerminalView
  siblings: readonly TerminalView[]
  owner: TerminalOwnerRef
  ownerLabel: string
  active: boolean
  onSelect: (terminalId: string, owner: TerminalOwnerRef) => void
  sortable: boolean
  // The live search query, for the match highlight ("" renders plain).
  query: string
}) {
  // In the sidebar row an idle terminal reads a plain "Terminal" (the owner on
  // line two and row order distinguish several), while a running one shows its
  // foreground app via `terminalTitle`. The identifying "Terminal N" label still
  // drives the tooltip and the other surfaces (breadcrumb, task manager) that
  // call `terminalTitle` directly. Mirrors the TUI terminal row.
  const title =
    terminalForeground(terminal) === null
      ? "Terminal"
      : terminalTitle(terminal, siblings)
  const word = terminalStateWord(terminal)
  // Same working cue as the agent row: the name shimmers while streaming, and only
  // while streaming and NOT typing (typing owns the caret) so the two read apart.
  const shimmer = terminal.working && !terminal.typing

  // Whole-row drag, exactly like AgentFlatRow: `useSortable` supplies the drag
  // listeners spread onto the select button (the PointerSensor's 6px activation
  // distance keeps a plain click as a select), and the wrapper carries the
  // Y-locked transform so a row never flies out of the column.
  const { attributes, listeners, setNodeRef, transform, transition, isDragging } =
    useSortable({ id: terminal.id })
  const style: CSSProperties = {
    transform: transform
      ? `translate3d(0, ${Math.round(transform.y)}px, 0)`
      : undefined,
    transition,
    opacity: isDragging ? 0.6 : undefined,
  }
  const dragProps = sortable ? { ...attributes, ...listeners } : {}

  return (
    <div
      ref={setNodeRef}
      style={style}
      className={cn(
        "group/flat-term relative flex items-stretch rounded-md pr-1 transition-colors",
        "hover:bg-sidebar-accent hover:text-sidebar-accent-foreground",
        active && "bg-sidebar-accent text-sidebar-accent-foreground",
      )}
    >
      <button
        {...dragProps}
        type="button"
        onClick={() => onSelect(terminal.id, owner)}
        className="flex min-w-0 flex-1 touch-manipulation items-start gap-2.5 py-2 pl-2 text-left max-md:min-h-10"
      >
        <SquareTerminal className="mt-0.5 size-4 shrink-0 text-muted-foreground" />
        <span className="flex min-w-0 flex-1 flex-col gap-0.5">
          <span className="flex items-center gap-2">
            <SimpleTooltip
              content={title !== terminal.label ? terminal.label : null}
              side="right"
            >
              <span
                className={cn(
                  "min-w-0 flex-1 truncate text-sm agent-name-shimmer",
                  shimmer && "agent-name-shimmer--on",
                )}
              >
                <HighlightedText text={title} query={query} />
              </span>
            </SimpleTooltip>
            {terminal.typing ? <TypingCaret /> : null}
          </span>
          <span className="flex items-center gap-1.5 text-xs text-muted-foreground">
            {/* The ↳ owner tag mirrors the agent row's project tag: which owner
                this terminal belongs to, then its colored state word. */}
            <span className="flex min-w-0 shrink items-center gap-1">
              <span aria-hidden>↳</span>
              <span className="min-w-0 truncate">
                <HighlightedText text={ownerLabel} query={query} />
              </span>
            </span>
            <Dot className="text-muted-foreground" />
            <span
              key={word.label}
              className={cn(
                "shrink-0 font-medium motion-safe:animate-state-word",
                word.className,
              )}
            >
              {word.label}
            </span>
          </span>
        </span>
      </button>
      <DropdownMenu>
        <div className="flex shrink-0 items-center overflow-hidden transition-[max-width,opacity] duration-200 ease-out motion-reduce:transition-none max-md:max-w-none md:max-w-0 md:opacity-0 md:group-hover/flat-term:max-w-8 md:group-hover/flat-term:opacity-100 md:group-focus-within/flat-term:max-w-8 md:group-focus-within/flat-term:opacity-100 md:has-[[data-popup-open]]:max-w-8 md:has-[[data-popup-open]]:opacity-100">
          <DropdownMenuTrigger
            render={
              <Button
                variant="ghost"
                size="icon"
                className="size-7 shrink-0 max-md:size-10"
                aria-label="Terminal actions"
              />
            }
          >
            <Ellipsis />
          </DropdownMenuTrigger>
        </div>
        {/* One real action only: closing. Opening the terminal is the row's own
            click, and a menu duplicate of it ("Stream") was removed as
            misleading; the menu stays (rather than an inline X) so the
            destructive action keeps its confirm flow and misclick-safe
            reveal-on-hover treatment. */}
        <DropdownMenuContent side="right" align="start">
          <DropdownMenuItem onClick={() => openDeleteTerminal(terminal.id)}>
            <X />
            Close…
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  )
}

// The flat Terminals section: every terminal (companion + project), each a
// two-line TerminalFlatRow, under a collapsible labeled divider that matches the
// Quiet tail's header and toggle exactly (a chevron, no leading icon). Renders
// directly below the main agent list, ABOVE the Inactive tail. Defaults OPEN,
// unlike the Quiet tail: a listed terminal is a live PTY worth surfacing, so it
// is shown by default but can be collapsed to reclaim space. Renders nothing
// when there are no terminals.
function TerminalsSection({
  terminals,
  selectedTarget,
  onSelect,
  sensors,
  onDragEnd,
  query,
}: {
  terminals: FlatTerminal[]
  selectedTarget: SelectedTarget | null
  onSelect: (terminalId: string, owner: TerminalOwnerRef) => void
  sensors: ReturnType<typeof useSensors>
  onDragEnd: (event: DragEndEvent) => void
  // The live search query, forwarded to each row's match highlight.
  query: string
}) {
  const [open, setOpen] = useState(true)
  if (terminals.length === 0) return null
  return (
    <div className="mt-2 border-t border-border/50 pt-2">
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        aria-expanded={open}
        className="flex w-full items-center gap-1.5 rounded-md px-2 py-1.5 text-xs font-medium text-muted-foreground transition-colors hover:bg-sidebar-accent hover:text-foreground max-md:min-h-10"
      >
        <ChevronRight
          className={cn("size-3 shrink-0 transition-transform", open && "rotate-90")}
        />
        <span>Terminals</span>
        <span className="ml-auto rounded-full bg-muted px-1.5 py-0.5 text-[10px] leading-none tabular-nums text-muted-foreground">
          {terminals.length}
        </span>
      </button>
      {open ? (
        // A SEPARATE DndContext + SortableContext from the agents one above: its
        // items are ONLY terminal ids, so dnd-kit can never pick an agent row as
        // a drop target for a terminal (or vice versa). This enforces the
        // within-group rule (a terminal reorders only among terminals) purely
        // through the two contexts holding disjoint id sets. A drag flips the
        // shared sort to manual, exactly like the agent drag.
        <DndContext
          sensors={sensors}
          collisionDetection={closestCenter}
          onDragEnd={onDragEnd}
        >
          <SortableContext
            items={terminals.map((ft) => ft.terminal.id)}
            strategy={verticalListSortingStrategy}
          >
            <div className="mt-1 flex flex-col gap-1">
              {terminals.map((ft) => (
                <TerminalFlatRow
                  key={ft.terminal.id}
                  terminal={ft.terminal}
                  siblings={ft.siblings}
                  owner={ft.owner}
                  ownerLabel={ft.ownerLabel}
                  active={
                    selectedTarget?.kind === "terminal" &&
                    selectedTarget.terminalId === ft.terminal.id
                  }
                  onSelect={onSelect}
                  sortable
                  query={query}
                />
              ))}
            </div>
          </SortableContext>
        </DndContext>
      ) : null}
    </div>
  )
}

// The collapsed Quiet tail: detached / exited agents, hidden by default so
// dormant work stops hogging the list. Its rows reuse the same AgentFlatRow (they
// render dimmed via agentRowVisual and carry the Detached/Exited state word).
//
// Search auto-expand is DERIVED state, never a mutation of the collapse
// preference: while `searchHit` (the live query matches something quiet) the
// section renders open so the results are visible, and it falls back to the
// manual `open` state the moment the query stops matching. The one override:
// a user who collapses the section WHILE a matching query is active has made
// an explicit call, so that dismissal wins, keyed to the NORMALIZED query (the
// core `quiet_tail` rule) and expiring the moment the normalized query changes
// (`prevQuery` tracks the transition via React's adjust-state-on-input-change
// pattern, no effect pass needed).
function QuietTail({
  sessions,
  projectName,
  selectedTarget,
  handlers,
  query,
  searchHit,
}: {
  sessions: SessionView[]
  projectName: (id: string) => string
  selectedTarget: SelectedTarget | null
  handlers: FlatSelectHandlers
  query: string
  searchHit: boolean
}) {
  const [open, setOpen] = useState(false)
  // The NORMALIZED query under which the user explicitly collapsed a
  // search-expanded tail; inert once the normalized query changes. Keying on the
  // normalized query (not the raw text) matches what the filter actually matches
  // on, so a whitespace/case variant of the same query does not resurrect a tail
  // the user just dismissed. Twin of the core `quiet_tail` rule.
  const normalizedQuery = normalizeQuery(query)
  const [dismissedQuery, setDismissedQuery] = useState<string | null>(null)
  const [prevQuery, setPrevQuery] = useState(normalizedQuery)
  if (normalizedQuery !== prevQuery) {
    setPrevQuery(normalizedQuery)
    if (dismissedQuery !== null && dismissedQuery !== normalizedQuery) {
      setDismissedQuery(null)
    }
  }
  const forcedOpen = quietTailForcedOpen(normalizedQuery, dismissedQuery, searchHit)
  const effectiveOpen = forcedOpen || open
  const toggle = () => {
    if (effectiveOpen) {
      // Collapsing: when the search is holding the section open, record the
      // dismissal for this normalized query; the base state collapses too, so
      // clearing the query lands on the state the user last chose.
      if (forcedOpen) setDismissedQuery(normalizedQuery)
      setOpen(false)
    } else {
      setDismissedQuery(null)
      setOpen(true)
    }
  }
  if (sessions.length === 0) return null
  return (
    <div className="mt-2 border-t border-border/50 pt-2">
      <button
        type="button"
        onClick={toggle}
        aria-expanded={effectiveOpen}
        className="flex w-full items-center gap-1.5 rounded-md px-2 py-1.5 text-xs font-medium text-muted-foreground transition-colors hover:bg-sidebar-accent hover:text-foreground max-md:min-h-10"
      >
        <ChevronRight
          className={cn(
            "size-3 shrink-0 transition-transform",
            effectiveOpen && "rotate-90",
          )}
        />
        <span>Inactive</span>
        <span className="ml-auto rounded-full bg-muted px-1.5 py-0.5 text-[10px] leading-none tabular-nums text-muted-foreground">
          {sessions.length}
        </span>
      </button>
      {effectiveOpen ? (
        <div className="mt-1 flex flex-col gap-1">
          {sessions.map((session) => (
            <AgentFlatRow
              key={session.id}
              session={session}
              projectName={projectName(session.project_id)}
              selectedTarget={selectedTarget}
              handlers={handlers}
              sortable={false}
              query={query}
            />
          ))}
        </div>
      ) : null}
    </div>
  )
}

// The sort control: a small dropdown listing the flat-list sort options. Default
// "active first"; "manual" is the only mode that enables drag-reorder.
const SORT_KEYS: FlatSortKey[] = ["active", "updated", "created", "name", "manual"]

function SortControl() {
  const agentSort = agentSortValue(useDux())
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        render={
          <button
            type="button"
            className="flex items-center gap-1.5 rounded-md border border-border/60 bg-input/30 px-2 py-1 text-xs text-muted-foreground transition-colors hover:border-border hover:bg-input/60 hover:text-foreground data-[popup-open]:border-border data-[popup-open]:bg-input/60 data-[popup-open]:text-foreground max-md:min-h-9"
            aria-label="Sort agents"
          />
        }
      >
        <ArrowUpDown className="size-3 shrink-0" />
        <span className="text-foreground/90">{FLAT_SORT_LABELS[agentSort]}</span>
        <ChevronDown className="size-3 shrink-0 opacity-60" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end">
        {SORT_KEYS.map((key) => (
          <DropdownMenuItem key={key} onClick={() => setAgentSort(key)}>
            {agentSort === key ? <Check /> : <span className="size-4" />}
            {FLAT_SORT_LABELS[key]}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

// The shared flat list: header controls (New agent, search, sort) over one
// active-first, cross-project list with a collapsible Quiet tail. Rendered inside
// the desktop sidebar's expanded content and the mobile hub alike.
export function FlatAgentList({ handlers }: { handlers: FlatSelectHandlers }) {
  const dux = useDux()
  const {
    spine,
    selectedTarget,
    agentSearch: rawAgentSearch,
    pendingAgentOrder,
    pendingTerminalOrder,
  } = dux
  const agentSort = agentSortValue(dux)
  const agentSearch = rawAgentSearch ?? ""

  const rawSessions = spine?.sessions ?? []
  const rawProjects = spine?.projects ?? []
  // Every terminal, of every owner, straight off the spine. It arrives flat and
  // owner-tagged, so there is nothing to stitch together here.
  const rawTerminals = spine?.terminals ?? []
  // partitionProjects still supplies the per-session project name, the project
  // terminals, and the project id sets. Ordering, however, is now a single GLOBAL
  // flat list (agents are independent of project grouping).
  const { withAgents, withoutAgents, projectName } = partitionProjects(
    spine?.sidebar,
    rawProjects,
    rawSessions,
  )

  // Flat model: agents are one global list ordered by the server's `sort_order`
  // (spine.sessions already arrives in that order). The optimistic drag overlay,
  // when present, names every session in the just-dropped order.
  const coreSessions: SessionView[] = pendingAgentOrder
    ? reorderById(rawSessions, pendingAgentOrder)
    : rawSessions

  // Split into main (active) and quiet (dormant), then order the main list by the
  // chosen sort. Search filters both, and the project terminals, by name/branch/
  // provider/owner.
  const { main, quiet } = partitionQuiet(coreSessions)
  const sortedMain = sortMainSessions(main, agentSort)
  // The quiet tail is recency-ordered in "active" mode (verbatim otherwise),
  // matching the TUI and the drag baseline in `displayedSessionOrder`.
  const sortedQuiet = sortQuietTail(quiet, agentSort)

  const query = agentSearch
  const visibleMain = sortedMain.filter((s) =>
    matchesSessionQuery(s, projectName(s.project_id), query),
  )
  const visibleQuiet = sortedQuiet.filter((s) =>
    matchesSessionQuery(s, projectName(s.project_id), query),
  )

  // The flat Terminals section: EVERY terminal, companion (session-owned) and
  // project-owned alike, in one list at the bottom, each carrying its owner label.
  // Ordering mirrors the agent list: a GLOBAL base order (every terminal sorted by
  // its `sort_order`, since a drag restamps `sort_order` across all owners), then
  // the optimistic drag overlay, then the shared active sort mode applied on top.
  const orderedProjects = [...withAgents, ...withoutAgents]
    .map((id) => rawProjects.find((p) => p.id === id))
    .filter((p): p is (typeof rawProjects)[number] => p !== undefined)
  // Assemble (owner-grouped, for the owner labels) then re-sort into the global
  // `sort_order` base (the terminal twin of `spine.sessions` already being in
  // global order for agents).
  const assembledTerminals = assembleFlatTerminals(
    rawTerminals,
    coreSessions,
    orderedProjects,
    projectName,
  )
  const baseTerminals = assembledTerminals
    .slice()
    .sort((a, b) => a.terminal.sort_order - b.terminal.sort_order)
  // The optimistic overlay reorders the base by terminal id (reusing `reorderById`,
  // which keys off `.id`, via a thin `{ id, ft }` wrapper). This is the terminal
  // twin of `coreSessions = reorderById(rawSessions, pendingAgentOrder)`.
  const overlaidTerminals: FlatTerminal[] = pendingTerminalOrder
    ? reorderById(
        baseTerminals.map((ft) => ({ id: ft.terminal.id, ft })),
        pendingTerminalOrder,
      ).map((w) => w.ft)
    : baseTerminals
  // Apply the shared sort mode (manual is verbatim, so the overlay survives it),
  // then the search filter (after the sort, matching the agent pipeline).
  const flatTerminals = sortFlatTerminals(overlaidTerminals, agentSort).filter((ft) =>
    matchesTerminalQuery(ft.terminal, ft.ownerLabel, ft.projectName, query),
  )

  const manual = agentSort === "manual"
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 6 } }),
  )

  // The terminal drag's twin of `handleDragEnd`: move the dragged terminal to the
  // drop target's slot in the COMPLETE terminal id order (any owner) as the
  // active sort mode DISPLAYS it (`displayedTerminalOrder`; in manual that is
  // the base order verbatim), and if that changed the order, flip the shared
  // sort to manual (so the dropped position sticks instead of being re-sorted
  // away) and persist via `reorderTerminals`. The displayed order matters
  // because drags start from every sort mode: computing the move against the
  // hidden base order would land the row relative to neighbors the user is not
  // seeing. The search filter is deliberately NOT applied: the persisted order
  // must be total.
  function handleTerminalDragEnd(event: DragEndEvent) {
    const { active, over } = event
    if (!over || active.id === over.id) return
    const fullOrder = displayedTerminalOrder(overlaidTerminals, agentSort)
    const next = moveItem(fullOrder, String(active.id), String(over.id))
    if (ordersMatch(fullOrder, next)) return
    if (!manual) setAgentSort("manual")
    reorderTerminals(next)
  }

  function handleDragEnd(event: DragEndEvent) {
    const { active, over } = event
    if (!over || active.id === over.id) return
    // Global flat reorder: move the dragged agent to the drop target's slot in
    // the COMPLETE session order as the active sort mode DISPLAYS it (sorted
    // main list, quiet tail appended; manual stays the base order verbatim, its
    // long-standing behavior). Drags start from every sort mode, so the move
    // must be computed against what the user is looking at, and the captured
    // baseline is total (all sessions, never the search-filtered subset).
    const fullOrder = displayedSessionOrder(coreSessions, agentSort)
    const next = moveItem(fullOrder, String(active.id), String(over.id))
    if (ordersMatch(fullOrder, next)) return
    // A drag is an explicit request for manual control. If the user reordered from
    // a computed sort (active-first, name, ...), flip the sort to manual so the
    // dropped position sticks instead of being immediately re-sorted away. The order
    // persists in SQLite; persisting `agentSort` too (see store) keeps the manual
    // view across restarts.
    if (!manual) setAgentSort("manual")
    reorderAgents(next)
  }

  const nothing =
    coreSessions.length === 0 &&
    flatTerminals.length === 0 &&
    quiet.length === 0
  const nothingMatches =
    query.trim() !== "" &&
    visibleMain.length === 0 &&
    visibleQuiet.length === 0 &&
    flatTerminals.length === 0

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {/* Header: title + sort on one row, then search. The New-agent action now
          lives in the bottom bar next to Add project (both surfaces). px-2 matches
          the sidebar header (the logo row's p-2) and the list below, so the search
          box and the agent rows share one inset and none of it hugs the edge. */}
      <div className="flex flex-col gap-2 px-2 pt-2 pb-3">
        <div className="flex items-center gap-2">
          <span className="text-sm font-semibold">Agents</span>
          <span className="text-xs text-muted-foreground">
            {coreSessions.length}
          </span>
          {coreSessions.length > 0 ? (
            <div className="ml-auto">
              <SortControl />
            </div>
          ) : null}
        </div>
        <div className="flex items-center gap-2 rounded-md border border-input bg-input/30 px-2.5 max-md:min-h-10">
          <Search className="size-4 shrink-0 text-muted-foreground" />
          <input
            value={agentSearch}
            onChange={(event) => setAgentSearch(event.target.value)}
            placeholder="Search agents and terminals"
            aria-label="Search agents and terminals"
            className="min-w-0 flex-1 bg-transparent py-1.5 text-sm outline-none placeholder:text-muted-foreground"
          />
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-2 no-scrollbar">
        {nothing ? (
          <Empty className="border-0 p-4">
            <EmptyHeader>
              <EmptyMedia variant="icon">
                <Bot />
              </EmptyMedia>
              <EmptyTitle>No agents yet</EmptyTitle>
              <EmptyDescription>
                Create one from the New agent picker.
              </EmptyDescription>
            </EmptyHeader>
          </Empty>
        ) : nothingMatches ? (
          <p className="px-3 py-4 text-sm text-muted-foreground">
            Nothing matches “{query}”.
          </p>
        ) : (
          <>
            <DndContext
              sensors={sensors}
              collisionDetection={closestCenter}
              onDragEnd={handleDragEnd}
            >
              <SortableContext
                items={visibleMain.map((s) => s.id)}
                strategy={verticalListSortingStrategy}
              >
                <div className="flex flex-col gap-1">
                  {visibleMain.map((session) => (
                    <AgentFlatRow
                      key={session.id}
                      session={session}
                      projectName={projectName(session.project_id)}
                      selectedTarget={selectedTarget}
                      handlers={handlers}
                      sortable
                      query={query}
                    />
                  ))}
                </div>
              </SortableContext>
            </DndContext>

            {/* Terminals sit ABOVE the Inactive tail: a live terminal is worth
                more prominence than dormant agents, and the default-open
                Terminals section would otherwise render below a section that is
                default-closed. Collapse defaults are unchanged (Terminals open,
                Inactive closed, per the CLAUDE.md tenet). */}
            <TerminalsSection
              terminals={flatTerminals}
              selectedTarget={selectedTarget}
              onSelect={handlers.onSelectTerminal}
              sensors={sensors}
              onDragEnd={handleTerminalDragEnd}
              query={query}
            />

            <QuietTail
              sessions={visibleQuiet}
              projectName={projectName}
              selectedTarget={selectedTarget}
              handlers={handlers}
              query={query}
              // A live query with a quiet hit derives the section open (see the
              // QuietTail doc); an empty query never forces anything.
              searchHit={query.trim() !== "" && visibleQuiet.length > 0}
            />
          </>
        )}
      </div>
    </div>
  )
}
