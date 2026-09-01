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
  ClipboardCopy,
  Cpu,
  Ellipsis,
  ExternalLink,
  FileCode2,
  Folder,
  GitFork,
  GitPullRequest,
  Info,
  Paperclip,
  Pencil,
  Play,
  Plus,
  Radar,
  RotateCcw,
  RefreshCw,
  ScrollText,
  Search,
  SquareChevronRight,
  SquarePlus,
  SquareTerminal,
  Trash2,
  Unlink,
  Variable,
  X,
} from "lucide-react"
import type { CSSProperties, ReactNode } from "react"
import { useState } from "react"

import { AgentVitalsTooltip } from "@/components/AgentVitalsTooltip"
import { InputMenuItems } from "@/components/InputMenuItems"
import { ProjectMenuItems } from "@/components/ProjectMenuItems"
import {
  quietTailManualChoice,
  setQuietTailManualChoice,
} from "@/lib/quietTailChoice"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
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
import { useIsMobile } from "@/hooks/use-mobile"
import { useTouchSurfaces } from "@/hooks/use-typing-surface"
import { agentRowVisual } from "@/lib/agentRow"
import { defaultProviderForSession } from "@/lib/agentTabs"
import {
  agentSearchLocation,
  matchCharRange,
  matchesSessionQuery,
  matchesTerminalQuery,
  normalizeQuery,
} from "@/lib/agentSearch"
import { changesCountFor } from "@/lib/agentVitals"
import { DEFAULT_AGENT_TABS_MAX } from "@/lib/bootstrapApi"
import { clipboardWorktree } from "@/lib/flatClipboard"
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
  type StateWord,
} from "@/lib/flatList"
import {
  assembleFlatTerminals,
  displayedTerminalOrder,
  sortFlatTerminals,
  terminalStateWord,
  type FlatTerminal,
} from "@/lib/flatTerminals"
import { prIconClass, prIconHoverClass, prStateLabel } from "@/lib/pr"
import { launcherVerb } from "@/lib/launcherVerb"
import { partitionProjects } from "@/lib/projects"
import { moveItem, ordersMatch, reorderById } from "@/lib/reorder"
import { agentRoot, editorRootForTarget } from "@/lib/editorRoot"
import {
  sessionLabel,
  supportsBranchGit,
  workspaceDirectory,
  workspaceLocation,
  workspaceProjectId, folderDisplayName } from "@/lib/agentWorkspace"
import {
  addTab,
  agentSortValue,
  createStandaloneTerminal,
  createTerminal,
  detachPullRequest,
  resumePullRequestAutodetection,
  openAgentEnv,
  openAgentInfo,
  openAgentStartupCommand,
  openAddProject,
  openAttachPullRequest,
  openChangeProvider,
  openDelete,
  openDeleteTerminal,
  openEditor,
  standaloneEditorHash,
  openForceReconnect,
  openForkAgent,
  openNewAgentPicker,
  openRename,
  openStartupLogs,
  reorderAgents,
  reorderTerminals,
  rerunStartupCommand,
  sessionActiveElsewhere,
  setAgentSearch,
  setAgentSort,
  toggleSessionAutoReopen,
  useDux,
} from "@/lib/store"
import { useAttachCapability } from "@/lib/attachRegistry"
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
//
// "Attach a file…" is not context-scoped: it is offered from any of this
// agent's menus, and answers for itself by asking whether a pane of this agent
// is mounted and owns its input (see `useAttachCapability`).
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
  // The two submenu labels name their subject so the menu reads unambiguously
  // even when opened far from the row (the mobile terminal header). The agent
  // name is the row's own display idiom (title ?? branch); the Project
  // submenu names the PROJECT, because its actions affect the whole project,
  // not just this agent.
  const agentName = sessionLabel(session)
  // `null` for a standalone agent, which belongs to no project. Read once so
  // the submenu's presence and its contents cannot disagree.
  const projectId = workspaceProjectId(session.workspace)
  const projectName = spine?.projects.find((p) => p.id === projectId)?.name
  const ghAvailable = bootstrap?.gh_available ?? false
  // Whether the branch-identity features exist for this agent at all: fork,
  // pull requests, startup commands. They are about a branch dux manages, and
  // a standalone agent has none whatever its folder contains.
  const branchGit = supportsBranchGit(session.workspace)
  const prOverridden = session.pr?.overridden ?? false
  // Detach answers "this agent has no PR", so it is offered on ANY association,
  // pinned or autodetected: an autodetected badge the user does not want is the
  // case it exists for, and gating on the pin hid it from exactly those people.
  const prAssociated = session.pr != null
  // The way back, offered only where it means something. Both are gh-free: the
  // suppression is dux's own state, so it must be removable even if gh went
  // away after the detach.
  const prSuppressed = session.pr_autodetect_suppressed ?? false
  // While another connection input-owns one of this agent's PTYs, the entries
  // that MUTATE the agent disable: deleting, renaming or relaunching an agent
  // someone else is actively driving is a surprise for them. Two sources feed
  // the answer (see `sessionActiveElsewhere`): a mounted TerminalPane's live
  // verdict, and the server-published `input_owner` field on the spine's
  // tabs — the latter is what lets a hub or sidebar row gate an agent NO pane
  // on this device is attached to. Read-only entries (info, the
  // project submenu, editor/terminal/copy entries) and this device's own view
  // preferences (the bar toggles) stay usable. The reason renders as an
  // inline label rather than a tooltip: disabled menu items are
  // pointer-events-none, so a hover tooltip could never fire, and touch has
  // no hover at all.
  const activeElsewhere = sessionActiveElsewhere(duxState, session)
  const isMobile = useIsMobile()
  const touchSurfaces = useTouchSurfaces()
  // Every PTY this agent can have a mounted pane for: the session-slot tab's id
  // IS the session id, and each extra tab has its own. Whichever pane is
  // mounted and owns its input answers.
  const attachToPane = useAttachCapability([
    session.id,
    ...session.tabs.map((t) => t.id),
  ])

  return (
    <DropdownMenuGroup>
      {activeElsewhere ? (
        <>
          <DropdownMenuLabel className="max-w-60 whitespace-normal">
            This agent is active on another device, so actions that modify it
            are disabled. Take over in its terminal to use them here.
          </DropdownMenuLabel>
          <DropdownMenuSeparator />
        </>
      ) : null}
      {/* The shared input-menu items (labels, icons and store writes live in
          InputMenuItems, shared with the input `⋯` below the terminal and with
          the agentless terminal screens' menu, so the three can never drift).
          Visibility is computed HERE rather than inside the component: this
          header menu is phone-shell chrome, so both toggles ride `isMobile`,
          which is the behavior this menu has always had. The keys toggle rides
          the touch surfaces too, so it is present exactly where pressing it
          puts a key row on screen: in a narrow window on a laptop the width
          alone said yes and the press did nothing.

          Deliberately NOT disabled while the agent is active elsewhere: hiding
          this device's own bars is this device's view preference, not a
          mutation of the agent. Attach is gated off in the shared items only
          because this menu renders its own Attach entry just below: this menu
          is the phone terminal screen's header menu and a keyboard-reachable
          path, so it carries Attach itself. On a phone agent screen Attach
          therefore appears both here and in the input `⋯` menu; both call the
          one registered capability, so they cannot drift. */}
      <InputMenuItems
        gates={{
          attach: false,
          surfaceSwitch: false,
          keysToggle: context === "terminal" && isMobile && touchSurfaces,
          topBarToggle: context === "terminal" && isMobile,
          // This menu hangs off the phone header, which is one of the things
          // theater takes away, so it is never on screen while theater is on
          // and has nothing to offer a way out of.
          theaterExit: false,
        }}
        trailingSeparator
      />
      {attachToPane ? (
        <>
          {/* THE DESKTOP AND KEYBOARD PATH INTO THE UPLOAD JOURNEY. A drag
              needs a desktop pointer and a paste needs the file already on the
              clipboard; this needs neither. It is offered only while a pane
              for this agent is MOUNTED AND OWNS its input, because the upload
              still travels through that pane's own gated connection and sink
              (never a side channel), and a file attached from a viewer would
              strand as saved-but-not-sent. Hidden rather than disabled when no
              such pane exists, per the row-menu convention. */}
          <DropdownMenuItem onClick={attachToPane}>
            <Paperclip />
            Attach a file…
          </DropdownMenuItem>
          <DropdownMenuSeparator />
        </>
      ) : null}
      <AgentTabSubmenu
        sessionId={session.id}
        agentName={agentName}
        providers={providers}
        defaultProvider={defaultProvider}
        atTabCap={atTabCap}
        addingTab={addingTab}
        activeElsewhere={activeElsewhere}
      />
      <AgentProjectSubmenu projectId={projectId} projectName={projectName} />
      <DropdownMenuSeparator />
      <DropdownMenuItem
        disabled={activeElsewhere}
        onClick={() => openForceReconnect(session.id)}
      >
        <RotateCcw />
        Force recreate agent…
      </DropdownMenuItem>
      <DropdownMenuItem
        disabled={activeElsewhere}
        onClick={() => toggleSessionAutoReopen(session.id, !session.auto_reopen_enabled)}
      >
        <RefreshCw />
        {session.auto_reopen_enabled
          ? "Disable agent auto-reopen"
          : "Enable agent auto-reopen"}
      </DropdownMenuItem>
      <DropdownMenuSeparator />
      <DropdownMenuItem
        disabled={activeElsewhere}
        onClick={() => openRename(session.id)}
      >
        <Pencil />
        Rename agent…
      </DropdownMenuItem>
      <AgentIdentityAndSetupItems
        sessionId={session.id}
        branchGit={branchGit}
        ghAvailable={ghAvailable}
        prOverridden={prOverridden}
        prAssociated={prAssociated}
        prSuppressed={prSuppressed}
        activeElsewhere={activeElsewhere}
      />
      <DropdownMenuSeparator />
      {/* Two editor entries, named to distinguish their surfaces. The in-app
          overlay cannot open on a phone (EditorOverlay is desktop-only), so
          its item is CSS-hidden there rather than left as a dead no-op; the
          new-tab item, which opens the standalone surface, is the only
          editor entry on phones. Final copy was left to PR review. */}
      <DropdownMenuItem
        className="max-md:hidden"
        onClick={() => openEditor(agentRoot(session.id))}
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
            href={standaloneEditorHash(agentRoot(session.id))}
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
      <DropdownMenuItem
        onClick={() => clipboardWorktree(workspaceDirectory(session.workspace))}
      >
        <ClipboardCopy />
        Copy local path
      </DropdownMenuItem>
      <DropdownMenuSeparator />
      {/* The one deliberate red-tinted destructive menu item (dim at rest, bright
          on hover), per the CLAUDE.md web-UI menu tenet; the confirm dialog gates it. */}
      <DropdownMenuItem
        variant="destructive"
        className="not-focus:text-destructive/70! not-focus:*:[svg]:text-destructive/70!"
        disabled={activeElsewhere}
        onClick={() => openDelete(session.id)}
      >
        <Trash2 />
        Delete agent…
      </DropdownMenuItem>
    </DropdownMenuGroup>
  )
}

function AgentTabSubmenu({
  sessionId,
  agentName,
  providers,
  defaultProvider,
  atTabCap,
  addingTab,
  activeElsewhere,
}: {
  sessionId: string
  agentName: string
  providers: string[]
  defaultProvider: string
  atTabCap: boolean
  addingTab: boolean
  activeElsewhere: boolean
}) {
  return (
    <DropdownMenuSub>
      <DropdownMenuSubTrigger
        disabled={atTabCap || addingTab || activeElsewhere}
      >
        <Plus />
        <span className="min-w-0 truncate">
          New agent tab for &quot;{agentName}&quot;…
        </span>
      </DropdownMenuSubTrigger>
      <DropdownMenuSubContent>
        {providers.map((provider) => {
          const isDefault = provider === defaultProvider
          return (
            <DropdownMenuItem
              key={provider}
              onClick={() => addTab(sessionId, provider)}
            >
              {isDefault ? <Check /> : <Bot />}
              {provider}
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
  )
}

function AgentProjectSubmenu({
  projectId,
  projectName,
}: {
  projectId: string | null
  projectName?: string
}) {
  if (projectId === null) return null
  return (
    <DropdownMenuSub>
      <DropdownMenuSubTrigger>
        <Folder />
        <span className="min-w-0 truncate">
          {projectName ? <>Project &quot;{projectName}&quot;…</> : <>Project…</>}
        </span>
      </DropdownMenuSubTrigger>
      <DropdownMenuSubContent>
        <ProjectMenuItems id={projectId} />
      </DropdownMenuSubContent>
    </DropdownMenuSub>
  )
}

function AgentIdentityAndSetupItems({
  sessionId,
  branchGit,
  ghAvailable,
  prOverridden,
  prAssociated,
  prSuppressed,
  activeElsewhere,
}: {
  sessionId: string
  branchGit: boolean
  ghAvailable: boolean
  prOverridden: boolean
  prAssociated: boolean
  prSuppressed: boolean
  activeElsewhere: boolean
}) {
  return (
    <>
      {branchGit ? (
        <DropdownMenuItem onClick={() => openForkAgent(sessionId)}>
          <GitFork />
          Fork agent…
        </DropdownMenuItem>
      ) : null}
      <DropdownMenuItem
        disabled={activeElsewhere}
        onClick={() => openChangeProvider(sessionId)}
      >
        <Cpu />
        Change agent provider…
      </DropdownMenuItem>
      {branchGit && ghAvailable ? (
        <DropdownMenuItem
          disabled={activeElsewhere}
          onClick={() => openAttachPullRequest(sessionId)}
        >
          <GitPullRequest />
          {prOverridden
            ? "Change attached pull request…"
            : "Attach pull request…"}
        </DropdownMenuItem>
      ) : null}
      {branchGit && prAssociated ? (
        <DropdownMenuItem
          disabled={activeElsewhere}
          onClick={() => detachPullRequest(sessionId)}
        >
          <Unlink />
          Detach pull request
        </DropdownMenuItem>
      ) : null}
      {branchGit && prSuppressed ? (
        <DropdownMenuItem
          disabled={activeElsewhere}
          onClick={() => resumePullRequestAutodetection(sessionId)}
        >
          <Radar />
          Resume PR autodetection
        </DropdownMenuItem>
      ) : null}
      <DropdownMenuItem onClick={() => openAgentInfo(sessionId)}>
        <Info />
        Agent info…
      </DropdownMenuItem>
      <DropdownMenuSeparator />
      {branchGit ? (
        <>
          <DropdownMenuItem onClick={() => openAgentStartupCommand(sessionId)}>
            <SquareChevronRight />
            Configure startup command…
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => openAgentEnv(sessionId)}>
            <Variable />
            Configure environment variables…
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => rerunStartupCommand(sessionId)}>
            <Play />
            Rerun startup command
          </DropdownMenuItem>
          <DropdownMenuItem onClick={() => openStartupLogs(sessionId)}>
            <ScrollText />
            Startup command logs…
          </DropdownMenuItem>
        </>
      ) : null}
    </>
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
    // Baseline-aligned like every other tag on line two (see RowLineTwo), so
    // the project name sits on the same line as the state word whatever the
    // line's tallest item turns out to be. `self-center` on the tag opted the
    // whole thing out of that shared baseline and only happened to look right
    // while every item was the same height. The GLYPH is the one thing centered
    // here: an icon has no baseline of its own, and resting its box on the text
    // baseline would hang it into the descender space.
    <span className="flex min-w-0 shrink items-baseline gap-1 text-muted-foreground">
      <Folder className="size-3 shrink-0 self-center" />
      <span className="min-w-0 truncate">
        <HighlightedText text={name} query={query} />
      </span>
    </span>
  )
}

// The STANDALONE identity tag: the folder something standalone lives in, in
// the slot an owner would occupy. Worn by a standalone agent's row (where a
// project tag would sit) and a standalone terminal's row (where the ↳ owner
// tag would sit): one indicator, learned once, meaning "this one lives in your
// folder, not in a dux-managed working copy". The glyph is the literal ✷ star
// drawn as text, following the ↳ arrow's own idiom, and glyph plus label wear
// the dux-standalone token, the web twin of the TUI's standalone-location
// theme color: an IDENTITY tone, never a state color, quiet enough that it
// cannot shout over the row's state cues, and scoped to this tag so the rest
// of line two stays muted. The star is aria-hidden with an sr-only word beside
// it, so a screen reader speaks the meaning rather than the Unicode name. The
// label arrives home-collapsed from the server (the browser is not necessarily
// on its machine) and is cut to its last component here. Searched like the project name, so a query that
// matched a path explains itself.
//
// The path is set in the row's own face, not monospace: line two is one
// sentence (folder, dot, state word) and a second typeface inside it read as
// a different element. Baseline-aligned all the same, because that is the
// only alignment that survives a font change.
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

// Line two of a row, shared by the agent row and the terminal row because a
// terminal row IS an agent row: the location tag, the separator dot, the state
// word and whatever else the row appends, all sitting on ONE text baseline.
// Baseline, not center: a mono folder label and a sans state word carry
// different ascents, so centering their boxes leaves one visibly higher than
// the other, and baseline alignment is the only one that survives a font
// change. Every child either has a real text baseline or pins itself with
// `self-center` (the dot, the project tag's glyph) and says why.
function RowLineTwo({ children }: { children: ReactNode }) {
  return (
    <span className="flex items-baseline gap-1.5 text-xs text-muted-foreground">
      {children}
    </span>
  )
}

// The row's state word (Working / Typing / Idle / Detached / …), shared by both
// row kinds so the two can never drift in tone or in motion. Keyed on the label
// by the caller so a state change remounts the span and replays the one-shot
// swap instead of snapping the text. The swap is a fade and ONLY a fade: any
// vertical motion drops the word below line two's baseline for a frame, and a
// busy agent changes state often enough that a screenshot will catch it.
function RowStateWord({ word }: { word: StateWord }) {
  return (
    <span
      className={cn(
        "shrink-0 font-medium motion-safe:animate-state-word",
        word.className,
      )}
    >
      {word.label}
    </span>
  )
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
// is the clickable project tag, a colored state word, and a tab count. The
// branch is deliberately absent (see line two below). Uses ONLY fields that
// exist today.
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
  const label = sessionLabel(session)
  const agentSelected =
    selectedTarget?.kind === "agent" && selectedTarget.sessionId === session.id
  const { shimmer, dimmed, attention, typing } = agentRowVisual(
    session.status,
    session.working,
    session.needs_attention,
    session.typing,
  )
  const word = stateWord(session)
  // Which thing this agent is IN: its project, or (for a standalone agent) the
  // folder it runs in. Tagged rather than a bare string, so the row picks the
  // glyph without re-deriving which kind of agent it is.
  const location = workspaceLocation(session.workspace)
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
    // While dragging, the row visibly LIFTS (shadow + stacking) so a touch
    // hold that armed the drag reads as "grabbed" before the finger moves.
    <div
      ref={setNodeRef}
      style={style}
      className={cn("flex flex-col", isDragging && "z-10 rounded-md shadow-lg")}
    >
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
                {/* Typing cue: the violet caret, kept as the RIGHTMOST indicator so
                    its position is stable whether or not a PR badge is shown (the PR
                    sits to its left). Working's bob + shimmer are suppressed while
                    typing, so this is the sole cue. */}
                {typing ? <TypingCaret /> : null}
              </span>
              {/* Line two: display-only project + state word + tabs, through the
                  shared RowLineTwo the terminal row uses too. */}
              <RowLineTwo>
                {location.kind === "folder" ? (
                  <StandaloneTag label={location.label} query={query} />
                ) : (
                  <ProjectTag name={projectName} query={query} />
                )}
                <Dot className="text-muted-foreground" />
                {/* Keyed on the label so a state change (Working ⇄ Idle ⇄ Detached
                    …) remounts the span and replays the one-shot fade instead of
                    snapping the text. */}
                <RowStateWord key={word.label} word={word} />
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
// A standalone terminal swaps the arrow for the shared standalone star over the
// directory it opened in; see `StandaloneTag`.
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

  // Whether this row wears the standalone star instead of the owned-by arrow,
  // decided by the exhaustive owner matcher so a new owner kind must answer
  // for its marker before this compiles.
  const isStandalone = matchOwner(owner, {
    session: () => false,
    project: () => false,
    standalone: () => true,
  })

  // Whole-row drag, exactly like AgentFlatRow: `useSortable` supplies the drag
  // listeners spread onto the select button (the mouse sensor's 6px activation
  // distance keeps a plain click as a select; touch arms on a hold, see
  // lib/dragActivation.ts), and the wrapper carries the Y-locked transform so
  // a row never flies out of the column.
  // A terminal is one PTY, so one id answers whether a mounted owning pane is
  // there to attach through.
  const attachToPane = useAttachCapability([terminal.id])

  // Where this row's two editor entries point. A session-owned terminal
  // resolves to its AGENT's root, which is what keeps one worktree from
  // sprouting a second, git-blind editor beside the agent's.
  const editorRoot = editorRootForTarget({
    kind: "terminal",
    terminalId: terminal.id,
    owner,
  })

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
        // The same drag-lift cue as the agent row wrapper.
        isDragging && "z-10 shadow-lg",
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
            <RowStateWord key={word.label} word={word} />
          </RowLineTwo>
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
        {/* Streaming the terminal is the row's own click, so it is deliberately
            not repeated here (a menu duplicate, "Stream", was removed as
            misleading). What the menu carries is everything else: the two
            editor entries, matching the agent row's pair exactly, "Attach a
            file…", which is an action on the terminal's live PANE rather than
            on the terminal (hence only while that pane is mounted and owns its
            input), and Close, which stays in the menu rather than becoming an
            inline X so the destructive action keeps its confirm flow and its
            misclick-safe reveal-on-hover treatment. */}
        <DropdownMenuContent side="right" align="start">
          {/* The editor's root is the directory this terminal was SPAWNED in,
              and a terminal owned by an agent is sent to that agent's editor
              instead: same worktree, and the agent's editor is the one with the
              git surface. `editorRootForTarget` is what decides that. */}
          <DropdownMenuItem
            className="max-md:hidden"
            onClick={() => openEditor(editorRoot)}
          >
            <FileCode2 />
            Open editor here
          </DropdownMenuItem>
          <DropdownMenuItem
            render={
              <a
                href={standaloneEditorHash(editorRoot)}
                target="_blank"
                rel="noopener"
              />
            }
          >
            <ExternalLink />
            Open editor in new tab
          </DropdownMenuItem>
          <DropdownMenuSeparator />
          {attachToPane ? (
            <DropdownMenuItem onClick={attachToPane}>
              <Paperclip />
              Attach a file…
            </DropdownMenuItem>
          ) : null}
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
//
// That last sentence is load-bearing for the divider's + : the section is
// absent at zero terminals, so the + can never create the FIRST standalone
// terminal. Its zero-state home is the launcher corner's ⋯, which is always on
// screen. Deliberate, not an oversight, and written here so nobody "fixes" it
// by rendering an empty section. There is also no search-forced-open here (only
// the Quiet tail carries that machinery), so a search cannot conjure the
// divider either.
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
      {/* The divider is a ROW of two siblings, not one button with another
          nested inside it (nested interactive elements are invalid HTML and
          the click routing is a coin toss). Same shape as AgentFlatRow and
          TerminalFlatRow: a full-width primary button plus its own control in
          a flex wrapper. The word "Terminals" stays INSIDE the toggle, so the
          whole label is still what expands the section. */}
      <div className="flex items-center gap-2">
        <button
          type="button"
          onClick={() => setOpen((v) => !v)}
          aria-expanded={open}
          className="flex min-w-0 flex-1 items-center gap-1.5 rounded-md px-2 py-1.5 text-xs font-medium text-muted-foreground transition-colors hover:bg-sidebar-accent hover:text-foreground max-md:min-h-10"
        >
          <ChevronRight
            className={cn("size-3 shrink-0 transition-transform", open && "rotate-90")}
          />
          <span>Terminals</span>
          <span className={SECTION_COUNT_PILL}>{terminals.length}</span>
        </button>
        {/* One tap, no dialog: a standalone terminal has nothing to confirm
            (that is why its shared menu entry carries no trailing "…").

            Variant: ghost, quieter than even the header's outline +, because
            it lives inside a section divider whose whole row is muted chrome;
            an outlined block here would outweigh the divider it decorates.

            Sizing: 28px square on desktop, the per-axis exemption from the
            40px floor. Its only neighbour on either axis is the collapse
            toggle 8px to its left, which expands a section and executes
            nothing; on touch it takes the floor on both axes anyway. */}
        <SimpleTooltip content="New standalone terminal">
          <Button
            variant="ghost"
            size="icon-sm"
            aria-label="New standalone terminal"
            onClick={() => createStandaloneTerminal()}
            className="shrink-0 text-muted-foreground max-md:min-h-10 max-md:min-w-10"
          >
            <Plus />
          </Button>
        </SimpleTooltip>
      </div>
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

// The Quiet tail: detached / exited agents, collapsed while anything active is
// on screen so dormant work stops hogging the list, but OPEN while the whole
// workspace is dormant (see the auto-manage note on `manual` below; the rule is
// the TUI's, kept in step by hand). Its rows reuse the same AgentFlatRow (they
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
  anyActive,
}: {
  sessions: SessionView[]
  projectName: (id: string) => string
  selectedTarget: SelectedTarget | null
  handlers: FlatSelectHandlers
  query: string
  searchHit: boolean
  // Whether ANY agent in the workspace is active (pre-search, whole list).
  anyActive: boolean
}) {
  // Auto-managed until the user toggles the section by hand, mirroring the
  // TUI's rule: a wholly-dormant workspace renders its Inactive tail OPEN
  // (hiding every agent behind a collapsed toggle is the worst possible
  // landing screen after a restart, which brings agents back dormant), and
  // the tail collapses once any agent is active. The first manual toggle
  // takes over from the automation; `null` means "still automatic". The
  // choice is mirrored into page-load-scoped module state (see
  // lib/quietTailChoice.ts for why component state is not enough here).
  const [manual, setManual] = useState<boolean | null>(quietTailManualChoice())
  const setManualChoice = (next: boolean) => {
    setQuietTailManualChoice(next)
    setManual(next)
  }
  const open = manual ?? !anyActive
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
      setManualChoice(false)
    } else {
      setDismissedQuery(null)
      setManualChoice(true)
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
        {/* Right after the word, like every other section count. The Inactive
            divider deliberately gains NO button beside it: there is no such
            thing as creating a dormant agent. */}
        <span className={SECTION_COUNT_PILL}>{sessions.length}</span>
      </button>
      {effectiveOpen ? (
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

// One height token for every control in the Agents header (the new-agent + and
// the sort trigger): they sit side by side, so a difference of a pixel reads as
// a mistake. Written as an explicit height rather than inherited from padding,
// per the CLAUDE.md control-height tenet, and lifted to the 40px floor where a
// finger is the pointer (the hub renders this header too).
const HEADER_CONTROL_SIZING = "h-7 max-md:min-h-10"

// One counter pill for every section of the list: the count sits immediately
// after the section word everywhere, and right edges carry controls only.
const SECTION_COUNT_PILL =
  "rounded-full bg-muted px-1.5 py-0.5 text-[10px] leading-none tabular-nums text-muted-foreground"

function SortControl() {
  const agentSort = agentSortValue(useDux())
  // The menu, not the trigger, is where the active mode is legible: the trigger
  // is static ("Sort"), so the checkmark below is the touch-visible truth and
  // the tooltip is a desktop nicety on top of it.
  //
  // name_desc is the one mode the web never OFFERS (only the TUI cycles into
  // it), so its row is appended only while it is the active mode. Without that
  // row a TUI-set name_desc would be a checkmark-less menu: five rows, none of
  // them ticked, and no way to see what the list is actually sorted by.
  const keys: FlatSortKey[] =
    agentSort === "name_desc" ? [...SORT_KEYS, "name_desc"] : SORT_KEYS
  return (
    <DropdownMenu>
      <SimpleTooltip content={`Sorted by ${FLAT_SORT_LABELS[agentSort]}`}>
        {/* The trigger reads "Sort" and never the mode name: the full label
            plus the neighbouring + overflows a narrow sidebar and wraps, and a
            control that changes width when you use it is its own small
            annoyance. The mode lives in the tooltip and the menu instead.

            The old 36px phone exemption is retired: this trigger now HAS a
            horizontal neighbour (the new-agent + immediately to its left), so
            the "no interactive neighbour on that axis" basis for the
            relaxation is gone. It takes the 40px floor through the shared
            header sizing token, same as the +. */}
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
  const { withAgents, withoutAgents, projectName } = partitionProjects(
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
  const visibleMain = sortedMain.filter((session) =>
    matchesSessionQuery(
      session,
      agentSearchLocation(session, projectName),
      query,
    ),
  )
  const visibleQuiet = sortedQuiet.filter((session) =>
    matchesSessionQuery(
      session,
      agentSearchLocation(session, projectName),
      query,
    ),
  )
  const orderedProjects = [...withAgents, ...withoutAgents]
    .map((id) => rawProjects.find((project) => project.id === id))
    .filter((project): project is (typeof rawProjects)[number] =>
      project !== undefined
    )
  const assembledTerminals = assembleFlatTerminals(
    rawTerminals,
    coreSessions,
    orderedProjects,
    projectName,
  )
  const baseTerminals = assembledTerminals
    .slice()
    .sort((left, right) =>
      left.terminal.sort_order - right.terminal.sort_order
    )
  const overlaidTerminals: FlatTerminal[] = pendingTerminalOrder
    ? reorderById(
        baseTerminals.map((terminal) => ({
          id: terminal.terminal.id,
          terminal,
        })),
        pendingTerminalOrder,
      ).map((wrapped) => wrapped.terminal)
    : baseTerminals
  const flatTerminals = sortFlatTerminals(overlaidTerminals, agentSort).filter(
    (terminal) =>
      matchesTerminalQuery(
        terminal.terminal,
        terminal.ownerLabel,
        terminal.projectName,
        query,
      ),
  )
  const emptyVerbIsAddProject =
    launcherVerb(spine ? rawProjects.length : null) === "add-project"
  const nothing =
    coreSessions.length === 0 &&
    flatTerminals.length === 0 &&
    quiet.length === 0
  const nothingMatches =
    query.trim() !== "" &&
    visibleMain.length === 0 &&
    visibleQuiet.length === 0 &&
    flatTerminals.length === 0

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
  // Mouse drags on a 6px pull (a click stays a select); touch drags on a
  // HOLD, or it fights the list's scroll gesture on phones. Why this is two
  // sensors rather than one PointerSensor, and the values themselves, live
  // in lib/dragActivation.ts.
  const sensors = useSensors(
    useSensor(MouseSensor, { activationConstraint: MOUSE_DRAG_ACTIVATION }),
    useSensor(TouchSensor, { activationConstraint: TOUCH_DRAG_ACTIVATION }),
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
          {coreSessions.length > 0 ? (
            <div className="ml-auto flex items-center gap-2">
              {/* The section's own + : one tap to the same picker the launcher
                  corner's verb opens. A deliberate duplicate of that verb, and
                  worth it, because this is where the eye already is when the
                  thought "another one" arrives.

                  Variant: outline, while the sort trigger beside it keeps its
                  quieter borderless-until-hover styling. They are not two peers
                  in one cluster: this one acts (it opens a creation dialog) and
                  that one only reveals a menu.

                  It rides the same `coreSessions.length > 0` gate as sort,
                  because the empty state below carries its own hero button and
                  two buttons offering the same click on one screen is one too
                  many.

                  Sizing: 28px square on desktop is the per-axis exemption from
                  the 40px floor; its one horizontal neighbour is the Sort
                  trigger 8px to the right, which only reveals a menu, so a
                  misclick opens a list rather than executing anything. On
                  touch both controls take the floor through the shared header
                  token. */}
              <SimpleTooltip content="New agent">
                <Button
                  variant="outline"
                  size="sm"
                  aria-label="New agent"
                  onClick={() => openNewAgentPicker("new")}
                  className={cn(
                    "w-7 px-0 max-md:min-w-10",
                    HEADER_CONTROL_SIZING,
                  )}
                >
                  <Plus />
                </Button>
              </SimpleTooltip>
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
              {/* Copy unchanged by the launcher overhaul: the button flips, the
                  sentence does not (it is what was signed off, and it reads
                  true either way). */}
              <EmptyDescription>
                Pick a project and dux gives the agent its own worktree.
              </EmptyDescription>
            </EmptyHeader>
            {/* A real button, not a sentence pointing at one elsewhere: an
                empty workspace is exactly where the next click should be on
                screen. It doubles with the launcher corner's verb, which is
                accepted: the duplicate costs a tap nobody needs, while the
                missing one costs a hunt.

                It flips through the SAME pure helper the corner's verb reads,
                so the two buttons on screen can never offer different next
                steps. max-md:min-h-11 is the touch floor, matching the
                corner. */}
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
              // Keyed on the FULL list, not the filtered one: a search that
              // hides every active agent must not resurrect the auto-open.
              anyActive={main.length > 0}
            />
          </>
        )}
      </div>
    </div>
  )
}
