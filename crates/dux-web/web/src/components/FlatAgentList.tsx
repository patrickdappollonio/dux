import {
  DndContext,
  MouseSensor,
  TouchSensor,
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
  ArrowDownWideNarrow,
  Bot,
  Check,
  ChevronDown,
  ChevronRight,
  Ellipsis,
  Folder,
  GitPullRequest,
  Plus,
  Search,
  SquarePlus,
  SquareTerminal,
  TriangleAlert,
} from "lucide-react"
import type { ComponentProps, CSSProperties, ReactNode } from "react"
import { useRef, useState } from "react"

import { AgentVitalsTooltip } from "@/components/AgentVitalsTooltip"
import { MatchHighlight } from "@/components/MatchHighlight"
import { RowStateWord } from "@/components/RowStateWord"
import { WorkingGlyph } from "@/components/WorkingGlyph"
import { useWorkingPulseAnchor } from "@/hooks/use-working-cue"
import {
  agentRowKey,
  selectedRowKey,
  terminalRowKey,
  useRevealSelectedRow,
} from "@/hooks/use-reveal-selected-row"
import { useSearchPick } from "@/hooks/use-search-pick"
import { ACTIVE_ROW_RING } from "@/lib/focusRing"
import { PaneMenuBody } from "@/components/PaneMenu"
import {
  quietTailManualChoice,
  setQuietTailManualChoice,
} from "@/lib/quietTailChoice"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty"
import { agentRowVisual } from "@/lib/agentRow"
import {
  agentSearchLocation,
  matchCharRange,
  matchesSessionQuery,
  matchesTerminalQuery,
  normalizeQuery,
} from "@/lib/agentSearch"
import { changesCountFor } from "@/lib/agentVitals"
import {
  MOUSE_DRAG_ACTIVATION,
  TOUCH_DRAG_ACTIVATION,
} from "@/lib/dragActivation"
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
  displayedTerminalOrder,
  overlaidFlatTerminals,
  sortFlatTerminals,
  terminalStateWord,
  type FlatTerminal,
} from "@/lib/flatTerminals"
import {
  prAriaLabel,
  prIconClass,
  prIconHoverClass,
  prStateLabel,
} from "@/lib/pr"
import { ALWAYS_REVEALED_ON_TOUCH } from "@/lib/touchReveal"
import { launcherVerb } from "@/lib/launcherVerb"
import { partitionProjects } from "@/lib/projects"
import { moveItem, ordersMatch, reorderById } from "@/lib/reorder"
import {
  sessionLabel,
  workingCopyMissing,
  workspaceLocation,
  workspaceProjectId, folderDisplayName } from "@/lib/agentWorkspace"
import {
  agentSortValue,
  openAddProject,
  openNewAgentPicker,
  reorderAgents,
  reorderTerminals,
  setAgentSearch,
  setAgentSort,
  useDux,
} from "@/lib/store"
import { terminalForeground, terminalTitle } from "@/lib/terminals"
import type { DuxState, SelectedTarget, TerminalOwnerRef } from "@/lib/store"
import { matchOwner } from "@/lib/terminalOwner"
import type { SessionView, TerminalView } from "@/lib/types"
import { cn } from "@/lib/utils"

// How a flat row's tap resolves. Desktop just selects; mobile also drives the
// hub-to-terminal navigation. Passed in so the one shared list serves both.
export interface FlatSelectHandlers {
  onSelectSession: (sessionId: string) => void
  onSelectTerminal: (terminalId: string, owner: TerminalOwnerRef) => void
}

// Display-only project label: the project's actions live in the agent's ⋯ menu,
// so the row stays one click target. Searched, so a hit gets the match emphasis.
function ProjectTag({ name, query }: { name: string; query: string }) {
  return (
    // Baseline-aligned like every other tag on line two (see RowLineTwo). The
    // glyph is the one thing centered: an icon has no baseline of its own.
    <span className="flex min-w-0 shrink items-baseline gap-1 text-muted-foreground">
      <Folder className="size-3 shrink-0 self-center" />
      <span className="min-w-0 truncate">
        <HighlightedText text={name} query={query} />
      </span>
    </span>
  )
}

// A managed agent whose working copy is gone from disk. It REPLACES the project
// tag rather than joining it: the project is healthy and the agent is not, so a
// row naming the project would say nothing about why the agent cannot run.
function WorkingCopyMissingTag() {
  return (
    <span className="flex min-w-0 shrink items-baseline gap-1 text-destructive">
      <TriangleAlert className="size-3 shrink-0 self-center" />
      <span className="min-w-0 truncate">working copy missing</span>
    </span>
  )
}

// The standalone identity tag, on an agent's row and a terminal's row alike: an
// aria-hidden ✷ plus an sr-only word, in the identity-only dux-standalone token.
function StandaloneTag({ label, query }: { label: string; query: string }) {
  return (
    <span className="flex min-w-0 shrink items-baseline gap-1 text-dux-standalone">
      <span aria-hidden>✷</span>
      <span className="sr-only">standalone</span>
      <span className="min-w-0 truncate">
        <HighlightedText text={folderDisplayName(label)} query={query} />
      </span>
    </span>
  )
}

// `self-center` because line two aligns its text by baseline, and a box with
// no text has no baseline to align: unpinned, the dot sinks to the bottom edge.
function Dot({ className }: { className: string }) {
  return <span className={cn("size-1 shrink-0 self-center rounded-full bg-current opacity-50", className)} />
}

// Line two, shared by the agent row and the terminal row: everything sits on ONE
// text baseline, since a mono label and a sans state word have different ascents.
function RowLineTwo({ children }: { children: ReactNode }) {
  return (
    <span className="flex items-baseline gap-1.5 text-xs text-muted-foreground">
      {children}
    </span>
  )
}

// The typing cue: a blinking caret in the typing token, shared by both row kinds.
// Motion-reduce drops the blink and rests the caret fully opaque.
function TypingCaret() {
  return (
    <span
      aria-hidden
      className="inline-block h-3.5 w-0.5 shrink-0 rounded-full bg-dux-typing align-middle motion-safe:animate-typing-caret motion-reduce:opacity-100"
    />
  )
}

// A row label with its first match lit, computed against the DISPLAYED string
// only, the same first range the terminal UI lights.
function HighlightedText({ text, query }: { text: string; query: string }) {
  const range = matchCharRange(text, query)
  return <MatchHighlight text={text} ranges={range ? [range] : []} />
}

// One home for the row's name. It never animates: the working cue is the glyph
// and the state word, so the name stays a single legible run of text.
function RowName({
  text,
  query,
  className,
  ...rest
}: {
  text: string
  query: string
} & ComponentProps<"span">) {
  return (
    <span {...rest} className={cn("min-w-0 flex-1 truncate text-sm", className)}>
      <HighlightedText text={text} query={query} />
    </span>
  )
}

// The two-line agent row: identity and cues on line one, the location tag, state
// word and tab count on line two. The branch is deliberately absent.
function AgentFlatRow({
  session,
  projectName,
  selectedTarget,
  handlers,
  sortable,
  query,
  pickId,
  picked = false,
}: {
  session: SessionView
  projectName: string
  selectedTarget: SelectedTarget | null
  handlers: FlatSelectHandlers
  sortable: boolean
  // The live search query, for the match highlight ("" renders plain).
  query: string
  // The row's id for the search field's active descendant, and whether it is
  // the row Enter there would pick, ringed as if focused.
  pickId?: string
  picked?: boolean
}) {
  const label = sessionLabel(session)
  const agentSelected =
    selectedTarget?.kind === "agent" && selectedTarget.sessionId === session.id
  const { working, dimmed, attention, typing } = agentRowVisual(
    session.status,
    session.working,
    session.needs_attention,
    session.typing,
  )
  const word = stateWord(session)
  // The glyph's frame and the state word, so the word's pulse stays in step
  // with the glyph's bounce when work resumes mid-settle.
  const frameRef = useRef<HTMLSpanElement>(null)
  const wordRef = useRef<HTMLSpanElement>(null)
  useWorkingPulseAnchor(working, frameRef, wordRef)
  // Which thing this agent is IN: its project, or a standalone agent's folder.
  // Tagged so the row picks the glyph without re-deriving the agent kind.
  const location = workspaceLocation(session.workspace)
  const copyMissing = workingCopyMissing(session.workspace)
  const tabCount = session.tabs.length

  const { changes } = useDux()
  const changesCount = changesCountFor(changes, session.id)

  const { attributes, listeners, setNodeRef, transform, transition, isDragging } =
    useSortable({ id: session.id })
  const style: CSSProperties = {
    // Lock the drag to Y: the scroll container is overflow-x visible, so a
    // sideways drag flies the row out over the center pane.
    transform: transform
      ? `translate3d(0, ${Math.round(transform.y)}px, 0)`
      : undefined,
    transition,
    opacity: isDragging ? 0.6 : undefined,
  }
  const dragProps = sortable ? { ...attributes, ...listeners } : {}

  return (
    // While dragging, the row visibly LIFTS (shadow + stacking) so a touch
    // hold that armed the drag reads as "grabbed" before the finger moves.
    <div
      ref={setNodeRef}
      style={style}
      data-sidebar-row={agentRowKey(session.id)}
      className={cn("flex flex-col", isDragging && "z-10 rounded-md shadow-lg")}
    >
      <div
        className={cn(
          // The wrapper owns the highlight so it spans both lines and the trailing
          // ⋯; the button below is transparent and fills the row.
          "group/flat-row relative flex items-stretch rounded-md pr-1 transition-colors",
          "hover:bg-sidebar-accent hover:text-sidebar-accent-foreground",
          agentSelected && "bg-sidebar-accent text-sidebar-accent-foreground",
          picked && ACTIVE_ROW_RING,
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
            id={pickId}
            onClick={() => handlers.onSelectSession(session.id)}
            className={cn(
              "flex min-w-0 flex-1 touch-manipulation items-start gap-2.5 py-2 pl-2 text-left max-md:min-h-11",
              dimmed && "opacity-70",
            )}
          >
            <span
              aria-label={attention ? "Needs attention" : undefined}
              className="mt-0.5 inline-flex shrink-0"
            >
              {/* `working` is already resolved through the ladder, so a higher
                  state taking over is a stop like any other: the pulse yields,
                  the glyph lands at rest, and only then does the attention
                  look ease in. The label above is there at once. */}
              <WorkingGlyph
                icon={Bot}
                working={working}
                look={
                  attention
                    ? "text-cyan-100 motion-safe:animate-attention-pulse motion-reduce:animate-none"
                    : "text-sidebar-accent-foreground"
                }
                frameRef={frameRef}
                className="size-4.5"
              />
            </span>
            <span className="flex min-w-0 flex-1 flex-col gap-0.5">
              {/* Line one: name + PR + time. */}
              <span className="flex items-center gap-2">
                <RowName text={label} query={query} />
                {session.pr ? (
                  <SimpleTooltip
                    content={`#${session.pr.number} · ${session.pr.title} (${prStateLabel(session.pr.state)})`}
                    side="right"
                  >
                    <a
                      href={session.pr.url}
                      target="_blank"
                      rel="noopener noreferrer"
                      aria-label={prAriaLabel(session.pr.number, session.pr.state)}
                      className={cn(
                        "inline-flex shrink-0 items-center gap-0.5 rounded px-1 py-0.5 transition-colors",
                        prIconClass(session.pr.state),
                        prIconHoverClass(session.pr.state),
                      )}
                      onClick={(event) => {
                        // `stopPropagation` keeps the click off the row's select
                        // handler; `preventDefault` keeps this to ONE tab, since
                        // the anchor's `target="_blank"` would open a second.
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
                {/* Typing cue: the violet caret, kept as the RIGHTMOST indicator so
                    its position is stable whether or not a PR badge is shown (the PR
                    sits to its left). The working pulse is suppressed while
                    typing, so this is the sole cue. */}
                {typing ? <TypingCaret /> : null}
              </span>
              {/* Line two: display-only project + state word + tabs, through the
                  shared RowLineTwo the terminal row uses too. */}
              <RowLineTwo>
                {copyMissing ? (
                  <WorkingCopyMissingTag />
                ) : location.kind === "folder" ? (
                  <StandaloneTag label={location.label} query={query} />
                ) : (
                  <ProjectTag name={projectName} query={query} />
                )}
                <Dot className="text-muted-foreground" />
                {/* Keyed on the label so a state change (Working ⇄ Idle ⇄ Detached
                    …) remounts the span and replays the one-shot fade instead of
                    snapping the text. */}
                <RowStateWord
                  key={word.label}
                  ref={wordRef}
                  word={word}
                  working={working}
                />
                {/* No branch here, by decision: a drifted agent would put a
                    long mono branch inline on every row, noise, and worst on a
                    tablet. The branch's one home is the top bar's
                    branch chip (InsetHeader), which shows the CURRENT branch and
                    carries the drift note on hover. The branch stays searchable
                    (lib/agentSearch.ts still matches on it); a branch-only query
                    simply highlights nothing visible. */}
                {tabCount > 1 ? (
                  <>
                    <Dot className="text-muted-foreground" />
                    <span className="shrink-0">{tabCount} tabs</span>
                  </>
                ) : null}
              </RowLineTwo>
            </span>
          </button>
        </SimpleTooltip>

        <DropdownMenu>
          {/* The row rule: revealed on hover or focus anywhere in the row, kept
              while its menu is open, always shown on a coarse pointer, and
              taking no width while idle. Every revealed cap is max-w-10, the
              trigger's 40px coarse-pointer size: these md: caps outrank the
              unprefixed pointer-coarse:max-w-none, so a narrower cap would clip
              a focused or open trigger on a tablet at desktop width. The
              overflow clip the width animation needs also shears the trigger's
              focus ring (a box-shadow) off its sides while the row's full
              height leaves the bands above and below, which is what drew a
              wide bar instead of a ring; the wrapper lets the ring out while
              the trigger has keyboard focus. */}
          <div
            className={cn(
              "flex shrink-0 items-center overflow-hidden transition-[max-width,opacity] duration-200 ease-out motion-reduce:transition-none max-md:max-w-none md:max-w-0 md:opacity-0 md:group-hover/flat-row:max-w-10 md:group-hover/flat-row:opacity-100 md:group-focus-within/flat-row:max-w-10 md:group-focus-within/flat-row:opacity-100 md:has-[[data-popup-open]]:max-w-10 md:has-[[data-popup-open]]:opacity-100 has-[:focus-visible]:overflow-visible",
              ALWAYS_REVEALED_ON_TOUCH,
            )}
          >
            <DropdownMenuTrigger
              render={
                <Button
                  variant="ghost"
                  size="icon"
                  // 28px under a fine pointer, where the only neighbour on
                  // either axis is this row's own button, whose click selects
                  // the agent this menu is about; the 40px floor returns on a
                  // phone and on any coarse pointer.
                  className="size-7 shrink-0 max-md:size-10 pointer-coarse:size-10"
                  aria-label="Session actions"
                />
              }
            >
              <Ellipsis />
            </DropdownMenuTrigger>
          </div>
          {/* THE PANE'S ONE MENU, at the row's anchor. The same body the
              desktop pane header's `⋯` and the phone's flap open, so an agent's
              menu is one menu a user learns once, in whichever place they
              reached it from. Its INPUT group is still one home at a time: what
              the group holds is the pane's own published answer, not something
              this anchor decides. */}
          <DropdownMenuContent side="right" align="start">
            <PaneMenuBody
              subject={{ kind: "agent", session }}
              // No Settings drill at a row: both shells that render this list
              // keep a cog on screen outside this menu.
              settingsDrill={false}
            />
          </DropdownMenuContent>
        </DropdownMenu>
      </div>
    </div>
  )
}

// The two-line terminal row, mirroring the agent row's shape: line two is
// `↳ {ownerLabel} · {stateWord}`, with the standalone star in place of the arrow.
function TerminalFlatRow({
  terminal,
  siblings,
  owner,
  ownerLabel,
  active,
  onSelect,
  sortable,
  query,
  pickId,
  picked = false,
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
  // The row's id for the search field's active descendant, and whether it is
  // the row Enter there would pick, ringed as if focused.
  pickId?: string
  picked?: boolean
}) {
  // An idle terminal reads a plain "Terminal" here; a running one shows its
  // foreground app. `terminalTitle` still names it for the other surfaces.
  const title =
    terminalForeground(terminal) === null
      ? "Terminal"
      : terminalTitle(terminal, siblings)
  // The same working cue as the agent row, and only while streaming and NOT
  // typing (typing owns the caret) so the two read apart.
  const working = terminal.working && !terminal.typing
  const word = terminalStateWord(terminal)
  const frameRef = useRef<HTMLSpanElement>(null)
  const wordRef = useRef<HTMLSpanElement>(null)
  useWorkingPulseAnchor(working, frameRef, wordRef)

  // Whether this row wears the standalone star instead of the owned-by arrow,
  // decided by the exhaustive owner matcher so a new owner kind must answer.
  const isStandalone = matchOwner(owner, {
    session: () => false,
    project: () => false,
    standalone: () => true,
  })

  // Whole-row drag: `useSortable`'s listeners go on the select button, whose 6px
  // activation keeps a plain click a select, and the wrapper is Y-locked.

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
      data-sidebar-row={terminalRowKey(terminal.id)}
      className={cn(
        "group/flat-term relative flex items-stretch rounded-md pr-1 transition-colors",
        "hover:bg-sidebar-accent hover:text-sidebar-accent-foreground",
        active && "bg-sidebar-accent text-sidebar-accent-foreground",
        picked && ACTIVE_ROW_RING,
        // The same drag-lift cue as the agent row wrapper.
        isDragging && "z-10 shadow-lg",
      )}
    >
      <button
        {...dragProps}
        type="button"
        id={pickId}
        onClick={() => onSelect(terminal.id, owner)}
        className="flex min-w-0 flex-1 touch-manipulation items-start gap-2.5 py-2 pl-2 text-left max-md:min-h-10"
      >
        <WorkingGlyph
          icon={SquareTerminal}
          working={working}
          frameRef={frameRef}
          frameClassName="mt-0.5"
          className="size-4 text-muted-foreground"
        />
        <span className="flex min-w-0 flex-1 flex-col gap-0.5">
          <span className="flex items-center gap-2">
            <SimpleTooltip
              content={title !== terminal.label ? terminal.label : null}
              side="right"
            >
              <RowName text={title} query={query} />
            </SimpleTooltip>
            {terminal.typing ? <TypingCaret /> : null}
          </span>
          <RowLineTwo>
            {/* The owner tag mirrors the agent row's project tag: which owner
                this terminal belongs to, then its colored state word. A
                STANDALONE terminal has no owner, so it wears the shared
                standalone star over its directory instead, the same tag a
                standalone agent's row wears; owned terminals keep the ↳
                arrow, where it means "owned by". */}
            {isStandalone ? (
              <StandaloneTag label={ownerLabel} query={query} />
            ) : (
              <span className="flex min-w-0 shrink items-baseline gap-1">
                <span aria-hidden>↳</span>
                <span className="min-w-0 truncate">
                  <HighlightedText text={ownerLabel} query={query} />
                </span>
              </span>
            )}
            <Dot className="text-muted-foreground" />
            <RowStateWord key={word.label} ref={wordRef} word={word} working={working} />
          </RowLineTwo>
        </span>
      </button>
      <DropdownMenu>
        {/* The agent row's reveal rule and ring clearance, for the same reasons. */}
        <div
          className={cn(
            "flex shrink-0 items-center overflow-hidden transition-[max-width,opacity] duration-200 ease-out motion-reduce:transition-none max-md:max-w-none md:max-w-0 md:opacity-0 md:group-hover/flat-term:max-w-10 md:group-hover/flat-term:opacity-100 md:group-focus-within/flat-term:max-w-10 md:group-focus-within/flat-term:opacity-100 md:has-[[data-popup-open]]:max-w-10 md:has-[[data-popup-open]]:opacity-100 has-[:focus-visible]:overflow-visible",
            ALWAYS_REVEALED_ON_TOUCH,
          )}
        >
          <DropdownMenuTrigger
            render={
              <Button
                variant="ghost"
                size="icon"
                // Sized as the agent row's trigger, for the same neighbour.
                className="size-7 shrink-0 max-md:size-10 pointer-coarse:size-10"
                aria-label="Terminal actions"
              />
            }
          >
            <Ellipsis />
          </DropdownMenuTrigger>
        </div>
        {/* THE PANE'S ONE MENU, at the row's anchor: the same body the desktop
            pane header's `⋯` and the phone flap's open for this terminal, so a
            terminal's menu is one menu learned once. Its INPUT group is still
            one home at a time, because what the group holds is the pane's own
            published answer rather than something this anchor decides. */}
        <DropdownMenuContent side="right" align="start">
          <PaneMenuBody
            subject={{ kind: "terminal", terminalId: terminal.id, owner }}
            // The agent row's answer, for the same reason: a row's surrounding
            // chrome carries the cog on both form factors.
            settingsDrill={false}
          />
        </DropdownMenuContent>
      </DropdownMenu>
    </div>
  )
}

// The flat Terminals section, under a divider defaulting OPEN because a listed
// terminal is a live PTY. Absent at zero terminals, so its + never creates the first.
function TerminalsSection({
  terminals,
  selectedTarget,
  onSelect,
  sensors,
  onDragEnd,
  query,
  open,
  onToggle,
  pickedKey,
  pickId,
}: {
  terminals: FlatTerminal[]
  selectedTarget: SelectedTarget | null
  onSelect: (terminalId: string, owner: TerminalOwnerRef) => void
  sensors: ReturnType<typeof useSensors>
  onDragEnd: (event: DragEndEvent) => void
  // The live search query, forwarded to each row's match highlight.
  query: string
  // Held by the list, which needs to know which rows the search can reach.
  open: boolean
  onToggle: () => void
  pickedKey: string | null
  pickId: (key: string) => string
}) {
  if (terminals.length === 0) return null
  return (
    <div className="mt-2 border-t border-border/50 pt-2">
      {/* The whole label is what expands the section. It carries no + of its
          own: creating lives behind the launcher corner. */}
      <div className="flex items-center gap-2">
        <button
          type="button"
          onClick={onToggle}
          aria-expanded={open}
          className="flex min-w-0 flex-1 items-center gap-1.5 rounded-md px-2 py-1.5 text-xs font-medium text-muted-foreground transition-colors hover:bg-sidebar-accent hover:text-foreground max-md:min-h-10"
        >
          <ChevronRight
            className={cn("size-3 shrink-0 transition-transform", open && "rotate-90")}
          />
          <span>Terminals</span>
          <span className={SECTION_COUNT_PILL}>{terminals.length}</span>
        </button>
      </div>
      {open ? (
        // A separate DndContext holding ONLY terminal ids, so dnd-kit can never
        // pick an agent row as a drop target for a terminal, or the reverse.
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
                  pickId={pickId(terminalRowKey(ft.terminal.id))}
                  picked={pickedKey === terminalRowKey(ft.terminal.id)}
                />
              ))}
            </div>
          </SortableContext>
        </DndContext>
      ) : null}
    </div>
  )
}

// The Quiet tail: detached and exited agents, open only while the whole workspace
// is dormant. Search auto-expand is derived, never a write to the collapse preference.
function QuietTail({
  sessions,
  projectName,
  selectedTarget,
  handlers,
  query,
  open,
  onToggle,
  pickedKey,
  pickId,
}: {
  sessions: SessionView[]
  projectName: (id: string) => string
  selectedTarget: SelectedTarget | null
  handlers: FlatSelectHandlers
  query: string
  // From `useQuietTailOpen`, held by the list, which needs to know which rows
  // the search can reach.
  open: boolean
  onToggle: () => void
  pickedKey: string | null
  pickId: (key: string) => string
}) {
  if (sessions.length === 0) return null
  return (
    <div className="mt-2 border-t border-border/50 pt-2">
      <button
        type="button"
        onClick={onToggle}
        aria-expanded={open}
        className="flex w-full items-center gap-1.5 rounded-md px-2 py-1.5 text-xs font-medium text-muted-foreground transition-colors hover:bg-sidebar-accent hover:text-foreground max-md:min-h-10"
      >
        <ChevronRight
          className={cn(
            "size-3 shrink-0 transition-transform",
            open && "rotate-90",
          )}
        />
        <span>Inactive</span>
        {/* Right after the word, like every other section count. The Inactive
            divider deliberately gains NO button beside it: there is no such
            thing as creating a dormant agent. */}
        <span className={SECTION_COUNT_PILL}>{sessions.length}</span>
      </button>
      {open ? (
        <div className="mt-1 flex flex-col gap-1">
          {sessions.map((session) => (
            <AgentFlatRow
              key={session.id}
              session={session}
              projectName={projectName(
                workspaceProjectId(session.workspace) ?? "",
              )}
              selectedTarget={selectedTarget}
              handlers={handlers}
              sortable={false}
              query={query}
              pickId={pickId(agentRowKey(session.id))}
              picked={pickedKey === agentRowKey(session.id)}
            />
          ))}
        </div>
      ) : null}
    </div>
  )
}

// Whether the Quiet tail is open, and its toggle.
function useQuietTailOpen({
  query,
  searchHit,
  anyActive,
}: {
  query: string
  searchHit: boolean
  // Whether ANY agent in the workspace is active (pre-search, whole list).
  anyActive: boolean
}) {
  // Auto-managed until the user toggles by hand: a wholly-dormant workspace opens
  // the tail. `null` is still automatic; the choice mirrors to lib/quietTailChoice.ts.
  const [manual, setManual] = useState<boolean | null>(quietTailManualChoice())
  const setManualChoice = (next: boolean) => {
    setQuietTailManualChoice(next)
    setManual(next)
  }
  const open = manual ?? !anyActive
  // The NORMALIZED query under which the user collapsed a search-expanded tail,
  // inert once that query changes, so a case variant cannot resurrect it.
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
      // Collapsing while the search holds the section open records the dismissal
      // for this normalized query; the base state collapses too.
      if (forcedOpen) setDismissedQuery(normalizedQuery)
      setManualChoice(false)
    } else {
      setDismissedQuery(null)
      setManualChoice(true)
    }
  }
  return { open: effectiveOpen, toggle }
}

// The sort control: a small dropdown listing the flat-list sort options. Default
// "active first"; "manual" is the only mode that enables drag-reorder.
const SORT_KEYS: FlatSortKey[] = ["active", "updated", "created", "name", "manual"]

// One height token for every control in the Agents header, set explicitly rather
// than inherited, and lifted to the 40px floor where a finger is the pointer.
const HEADER_CONTROL_SIZING = "h-7 max-md:min-h-10"

// One counter pill for every section of the list: the count sits immediately
// after the section word everywhere, and right edges carry controls only.
const SECTION_COUNT_PILL =
  "rounded-full bg-muted px-1.5 py-0.5 text-[10px] leading-none tabular-nums text-muted-foreground"

function SortControl() {
  const agentSort = agentSortValue(useDux())
  // The trigger is static, so the checkmark in the menu is where the active mode
  // is legible. `name_desc` is never offered here, so its row shows only while active.
  const keys: FlatSortKey[] =
    agentSort === "name_desc" ? [...SORT_KEYS, "name_desc"] : SORT_KEYS
  return (
    <DropdownMenu>
      <SimpleTooltip content={`Sorted by ${FLAT_SORT_LABELS[agentSort]}`}>
        {/* The trigger reads "Sort" and never the mode name: the full label
            overflows a narrow sidebar and wraps, and a control that changes
            width when you use it is its own small annoyance. The mode lives in
            the tooltip and the menu instead. It takes the 40px floor on a
            phone through the shared header sizing token. */}
        <DropdownMenuTrigger
          render={
            <button
              type="button"
              className={cn(
                "flex items-center gap-1.5 rounded-md border border-border/60 bg-input/30 px-2 text-xs text-muted-foreground transition-colors hover:border-border hover:bg-input/60 hover:text-foreground data-[popup-open]:border-border data-[popup-open]:bg-input/60 data-[popup-open]:text-foreground",
                HEADER_CONTROL_SIZING,
              )}
              aria-label="Sort agents"
            />
          }
        >
          <ArrowDownWideNarrow className="size-3 shrink-0" />
          <span className="text-foreground/90">Sort</span>
          <ChevronDown className="size-3 shrink-0 opacity-60" />
        </DropdownMenuTrigger>
      </SimpleTooltip>
      <DropdownMenuContent align="end">
        {keys.map((key) => (
          <DropdownMenuItem key={key} onClick={() => setAgentSort(key)}>
            {agentSort === key ? <Check /> : <span className="size-4" />}
            {FLAT_SORT_LABELS[key]}
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

// The sessions a search query leaves on screen. Each is matched against its own
// searchable location, so typing part of a project name or a standalone agent's
// folder finds it.
function matchingSessions(
  sessions: SessionView[],
  projectName: (id: string) => string,
  query: string,
): SessionView[] {
  return sessions.filter((session) =>
    matchesSessionQuery(
      session,
      agentSearchLocation(session, projectName),
      query,
    ),
  )
}

// Which of the list's two blank screens is on, and what the onboarding one's
// hero button offers. A workspace with nothing in it beats an empty search
// result, so both flags are reported and the list decides in that order.
interface FlatListEmptyState {
  // The empty workspace: nothing to show whatever the search box says.
  nothing: boolean
  // Rows exist, but the query hides every one of them.
  nothingMatches: boolean
  emptyVerbIsAddProject: boolean
}

function flatListEmptyState(input: {
  // Null while the spine is unloaded, which is not yet a zero project count.
  projectCount: number | null
  coreSessions: SessionView[]
  quiet: SessionView[]
  visibleMain: SessionView[]
  visibleQuiet: SessionView[]
  flatTerminals: FlatTerminal[]
  query: string
}): FlatListEmptyState {
  const noTerminals = input.flatTerminals.length === 0
  return {
    nothing:
      input.coreSessions.length === 0 && noTerminals && input.quiet.length === 0,
    nothingMatches:
      input.query.trim() !== "" &&
      input.visibleMain.length === 0 &&
      input.visibleQuiet.length === 0 &&
      noTerminals,
    emptyVerbIsAddProject: launcherVerb(input.projectCount) === "add-project",
  }
}

function flatAgentListModel(dux: DuxState) {
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
  const rawTerminals = spine?.terminals ?? []
  const { projectName } = partitionProjects(
    spine?.sidebar,
    rawProjects,
    rawSessions,
  )
  const coreSessions: SessionView[] = pendingAgentOrder
    ? reorderById(rawSessions, pendingAgentOrder)
    : rawSessions
  const { main, quiet } = partitionQuiet(coreSessions)
  const sortedMain = sortMainSessions(main, agentSort)
  const sortedQuiet = sortQuietTail(quiet, agentSort)
  const query = agentSearch
  const visibleMain = matchingSessions(sortedMain, projectName, query)
  const visibleQuiet = matchingSessions(sortedQuiet, projectName, query)
  const overlaidTerminals = overlaidFlatTerminals({
    terminals: rawTerminals,
    sessions: coreSessions,
    projects: rawProjects,
    projectName,
    pendingOrder: pendingTerminalOrder,
  })
  const flatTerminals = sortFlatTerminals(overlaidTerminals, agentSort).filter(
    (terminal) =>
      matchesTerminalQuery(
        terminal.terminal,
        terminal.ownerLabel,
        terminal.projectName,
        query,
      ),
  )
  const { nothing, nothingMatches, emptyVerbIsAddProject } =
    flatListEmptyState({
      projectCount: spine ? rawProjects.length : null,
      coreSessions,
      quiet,
      visibleMain,
      visibleQuiet,
      flatTerminals,
      query,
    })

  return {
    selectedTarget,
    agentSort,
    agentSearch,
    coreSessions,
    projectName,
    main,
    visibleMain,
    visibleQuiet,
    overlaidTerminals,
    flatTerminals,
    query,
    manual: agentSort === "manual",
    emptyVerbIsAddProject,
    nothing,
    nothingMatches,
  }
}

// A row Enter in the search field can pick.
type SearchPickRow =
  | { kind: "agent"; session: SessionView }
  | { kind: "terminal"; terminal: FlatTerminal }

export function FlatAgentList({ handlers }: { handlers: FlatSelectHandlers }) {
  const dux = useDux()
  const {
    selectedTarget,
    agentSort,
    agentSearch,
    coreSessions,
    projectName,
    main,
    visibleMain,
    visibleQuiet,
    overlaidTerminals,
    flatTerminals,
    query,
    manual,
    emptyVerbIsAddProject,
    nothing,
    nothingMatches,
  } = flatAgentListModel(dux)
  // A newly selected row is scrolled into view, a just-created one as soon as
  // its row arrives, so a launch never happens out of sight below the fold.
  const listRef = useRevealSelectedRow(selectedRowKey(selectedTarget))
  // Mouse drags on a 6px pull; touch drags on a HOLD, or it fights the list's
  // scroll gesture. The values live in lib/dragActivation.ts.
  const sensors = useSensors(
    useSensor(MouseSensor, { activationConstraint: MOUSE_DRAG_ACTIVATION }),
    useSensor(TouchSensor, { activationConstraint: TOUCH_DRAG_ACTIVATION }),
  )

  const [terminalsOpen, setTerminalsOpen] = useState(true)
  const quietTail = useQuietTailOpen({
    query,
    // A live query with a quiet hit derives the section open (see the
    // QuietTail doc); an empty query never forces anything.
    searchHit: query.trim() !== "" && visibleQuiet.length > 0,
    // Keyed on the FULL list, not the filtered one: a search that hides every
    // active agent must not resurrect the auto-open.
    anyActive: main.length > 0,
  })
  // Enter in the search field selects a row the way a click would, so it walks
  // only the rows on screen, in the order they are shown.
  const searchPick = useSearchPick<SearchPickRow>({
    query,
    items: [
      ...visibleMain.map((session) => ({ kind: "agent" as const, session })),
      ...(terminalsOpen
        ? flatTerminals.map((terminal) => ({ kind: "terminal" as const, terminal }))
        : []),
      ...(quietTail.open
        ? visibleQuiet.map((session) => ({ kind: "agent" as const, session }))
        : []),
    ],
    keyOf: (row) =>
      row.kind === "agent"
        ? agentRowKey(row.session.id)
        : terminalRowKey(row.terminal.terminal.id),
    onPick: (row) =>
      row.kind === "agent"
        ? handlers.onSelectSession(row.session.id)
        : handlers.onSelectTerminal(row.terminal.terminal.id, row.terminal.owner),
  })

  // Move the dragged terminal to the slot the active sort DISPLAYS, over the
  // complete order and never the filtered subset, then flip the sort to manual.
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
    // Move the dragged agent to the slot the active sort DISPLAYS, over the
    // complete session order and never the search-filtered subset.
    const fullOrder = displayedSessionOrder(coreSessions, agentSort)
    const next = moveItem(fullOrder, String(active.id), String(over.id))
    if (ordersMatch(fullOrder, next)) return
    // A drag is an explicit request for manual control, so a reorder out of a
    // computed sort flips the sort to manual and the dropped position sticks.
    if (!manual) setAgentSort("manual")
    reorderAgents(next)
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {/* Header: the section word and its count on the left, the section's own
          controls (new agent, sort) at the right edge, then search. px-2 matches
          the sidebar header (the logo row's p-2) and the list below, so the search
          box and the agent rows share one inset and none of it hugs the edge. */}
      <div className="flex flex-col gap-2 px-2 pt-2 pb-3">
        <div className="flex items-center gap-2">
          <span className="text-sm font-semibold">Agents</span>
          <span className={SECTION_COUNT_PILL}>{coreSessions.length}</span>
          {/* Sort rides the same gate as the list it sorts: with no agent
              there is nothing to order, and the empty state below carries its
              own hero button. Creating lives behind the launcher corner, so
              the header carries no + of its own. */}
          {coreSessions.length > 0 ? (
            <div className="ml-auto flex items-center gap-2">
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
            {...searchPick.fieldProps}
            className="min-w-0 flex-1 bg-transparent py-1.5 text-sm outline-none placeholder:text-muted-foreground"
          />
        </div>
      </div>

      <div
        ref={listRef}
        className="min-h-0 flex-1 overflow-y-auto px-2 pb-2 no-scrollbar"
      >
        {nothing ? (
          <Empty className="border-0 p-4">
            <EmptyHeader>
              <EmptyMedia variant="icon">
                <Bot />
              </EmptyMedia>
              <EmptyTitle>No agents yet</EmptyTitle>
              {/* The button below flips on an empty workspace and this
                  sentence does not: it reads true either way. */}
              <EmptyDescription>
                Pick a project and dux gives the agent its own worktree.
              </EmptyDescription>
            </EmptyHeader>
            {/* A real button, not a sentence pointing at one elsewhere: an
                empty workspace is exactly where the next click should be on
                screen. It doubles with the launcher corner's New agent…, which is accepted:
                the duplicate costs a tap nobody needs, while the missing one
                costs a hunt.

                On a confirmed-empty workspace it flips to Add project through
                the pure helper in lib/launcherVerb.ts. max-md:min-h-11 is the
                touch floor, matching the launcher. */}
            <EmptyContent>
              <Button
                variant="outline"
                size="sm"
                className="max-md:min-h-11"
                onClick={
                  emptyVerbIsAddProject
                    ? openAddProject
                    : () => openNewAgentPicker("new")
                }
              >
                {emptyVerbIsAddProject ? <SquarePlus /> : <Plus />}
                {emptyVerbIsAddProject ? "Add project" : "New agent"}
              </Button>
            </EmptyContent>
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
                      projectName={projectName(
                        workspaceProjectId(session.workspace) ?? "",
                      )}
                      selectedTarget={selectedTarget}
                      handlers={handlers}
                      sortable
                      query={query}
                      pickId={searchPick.rowId(agentRowKey(session.id))}
                      picked={searchPick.activeKey === agentRowKey(session.id)}
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
              open={terminalsOpen}
              onToggle={() => setTerminalsOpen((v) => !v)}
              pickedKey={searchPick.activeKey}
              pickId={searchPick.rowId}
            />

            <QuietTail
              sessions={visibleQuiet}
              projectName={projectName}
              selectedTarget={selectedTarget}
              handlers={handlers}
              query={query}
              open={quietTail.open}
              onToggle={quietTail.toggle}
              pickedKey={searchPick.activeKey}
              pickId={searchPick.rowId}
            />
          </>
        )}
      </div>
    </div>
  )
}
