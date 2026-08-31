import {
  ChevronLeft,
  Ellipsis,
  ExternalLink,
  GitPullRequest,
  Settings,
  X,
} from "lucide-react"
import { Suspense, useState, type ReactElement } from "react"

import { AgentNotFound } from "@/components/AgentNotFound"
import { LauncherCorner } from "@/components/LauncherCorner"
import { AppMenuSheet } from "@/components/AppMenuSheet"
import { ChangedFiles } from "@/components/ChangedFiles"
import { ChunkBoundary } from "@/components/ChunkBoundary"
import { LazyTerminalPane } from "@/components/LazyTerminalPane"
import { ConnDot } from "@/components/ConnDot"
import { AgentTabsStrip } from "@/components/AgentTabsStrip"
import { DormantTabCard } from "@/components/DormantTabCard"
import { AgentActionsMenu, FlatAgentList } from "@/components/FlatAgentList"
import { CHIP_GLYPHS } from "@/components/headerChipGlyphs"
import { MacroPopover } from "@/components/MacroPopover"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { InputMenuItems } from "@/components/InputMenuItems"
import { useIsMobile } from "@/hooks/use-mobile"
import {
  dormantTabNeedsCard,
  shouldShowTabStrip,
  slotTabIdOf,
} from "@/lib/agentTabs"
import { changesSummary, type ChangesSummary } from "@/lib/changesSummary"
import { mobileHeaderLanes } from "@/lib/headerSubject"
import { resolveInstanceTitle } from "@/lib/instanceTitle"
import {
  mobileTopBarVisible,
  navigateUp,
  openChangesScreen,
  openDeleteTerminal,
  selectSession,
  selectTerminal,
  standaloneEditorHash,
  useDux,
} from "@/lib/store"
import { prIconClass, prIconHoverClass, prStateLabel } from "@/lib/pr"
import type { SelectedTarget, TerminalOwnerRef } from "@/lib/store"
import type { AgentTabView, SessionView } from "@/lib/types"
import { editorRootForTarget } from "@/lib/editorRoot"
import { matchOwner } from "@/lib/terminalOwner"
import { terminalsForOwner, terminalTitle } from "@/lib/terminals"
import { cn } from "@/lib/utils"
import {
  folderWorkspace,
  sessionLabel,
  workspaceBranchName,
  workspaceProjectId,
} from "@/lib/agentWorkspace"

// Tapping a session on the hub focuses it, and focusing something IS the
// terminal screen: the screen is derived from the URL the selection writes, so
// there is no second navigation call to make here.
function selectAndOpen(sessionId: string): void {
  selectSession(sessionId)
}

function selectTerminalAndOpen(
  terminalId: string,
  owner: TerminalOwnerRef,
): void {
  selectTerminal(terminalId, owner)
}

// The hub: the shared flat agent list at touch size, mirroring the desktop
// sidebar. Search, sort and the new-agent + live in the list header; the
// launcher corner (one filled verb plus its ⋯) sits in the bottom bar, the same
// component and the same size tokens as the desktop sidebar's footer.
function HomeScreen() {
  const { bootstrap } = useDux()
  const [menuOpen, setMenuOpen] = useState(false)
  const instanceTitle = resolveInstanceTitle(bootstrap?.title)

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      <header className="flex shrink-0 items-center gap-2 border-b px-3 py-3">
        <span className="relative shrink-0">
          <img src="/dux-logo.png" alt="dux" className="size-8 rounded-lg" />
          <ConnDot className="absolute -right-0.5 -bottom-0.5 ring-2 ring-background" />
        </span>
        <div className="flex min-w-0 flex-1 flex-col gap-0.5 leading-none">
          <span className="truncate font-semibold">{instanceTitle}</span>
          <span className="text-sm text-muted-foreground">agent sessions</span>
        </div>
        <Button
          variant="outline"
          size="icon"
          className="size-11 shrink-0"
          aria-label="Menu"
          onClick={() => setMenuOpen(true)}
        >
          <Settings />
        </Button>
      </header>
      <AppMenuSheet open={menuOpen} onOpenChange={setMenuOpen} />

      <FlatAgentList
        handlers={{
          onSelectSession: selectAndOpen,
          onSelectTerminal: selectTerminalAndOpen,
        }}
      />

      <div className="flex shrink-0 items-center border-t p-3">
        <LauncherCorner className="flex-1" />
      </div>
    </div>
  )
}

// The spoke for a terminal that is NOT session-owned: one identity crumb over
// the shared terminal. Such a terminal has no agent, so it borrows none of the
// agent screen's AGENT chrome (no changes chip, no agent actions menu) — but it
// carries a ⋯ menu of its own, with the shared mobile-bar quick toggles and the
// terminal's one real action, Close….
//
// Shared by the project-owned and standalone screens, which differ only in what
// the identity crumb says and in what has to exist for the screen to be valid;
// the two wrappers below own that difference and nothing else, so the two spokes
// cannot drift apart in layout, header height or touch targets.
function AgentlessTerminalScreen({
  owner,
  terminalId,
  primary,
}: {
  owner: TerminalOwnerRef
  terminalId: string
  primary: string
}) {
  const duxState = useDux()
  const { spine } = duxState
  // This owner's own terminals, selected out of the flat collection by owner, so
  // the crumb still disambiguates against its true siblings.
  const ownedTerminals = terminalsForOwner(spine?.terminals ?? [], owner)
  const terminal = ownedTerminals.find((t) => t.id === terminalId)
  // The same `ui.mobile_top_bar` gate as the agent terminal screen: both
  // preferences deliberately cover every mobile terminal surface. Hiding
  // happens from this screen's own ⋯ menu (the shared items below), exactly as
  // on the agent screen; restoring from the input ⋯ menu below the terminal,
  // which is on screen in every bar state, or from Preferences.
  const topBarVisible = mobileTopBarVisible(duxState)
  const isMobile = useIsMobile()
  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      {topBarVisible ? (
        <header className="flex h-11 shrink-0 items-center gap-2 border-b px-3">
          {/* Up to the hub, by name. A relative history step would walk out
              of the app whenever this screen is the entry the browser opened
              on, which a deep link makes routine. */}
          <Button
            variant="ghost"
            size="icon"
            className="size-10 shrink-0"
            aria-label="Back"
            onClick={() => navigateUp()}
          >
            <ChevronLeft />
          </Button>
          <div className="flex min-w-0 flex-1 items-baseline gap-1.5 text-sm">
            <span className="truncate font-semibold">{primary}</span>
            <span className="truncate text-muted-foreground">
              {terminal ? terminalTitle(terminal, ownedTerminals) : "Terminal"}
            </span>
          </div>
          {/* The macro quick-picker's mobile entry point, a header icon like
              the agent screen's (a floating trigger over the PTY covered the
              text under it). Hiding the top bar hides it with the header —
              the same more-space trade the tab strip makes; restore is the
              input ⋯ menu below the terminal, or Preferences. */}
          <MacroPopover
            variant="icon"
            target={{ kind: "terminal", terminalId, owner }}
          />
          <DropdownMenu>
            <DropdownMenuTrigger
              render={
                <Button
                  variant="ghost"
                  size="icon"
                  className="size-10 shrink-0"
                  aria-label="Terminal actions"
                />
              }
            >
              <Ellipsis />
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end">
              {/* The shared input-menu items, identical to the agent screen's
                  menu (and to the input `⋯` below the terminal), then the
                  terminal's one real action from its sidebar row menu: Close…,
                  neutral color per the destructive convention (the … plus
                  ConfirmDeleteTerminalDialog are the danger signal), routed
                  through the same confirm target.

                  Both toggles ride `isMobile`: this header is the phone
                  shell's own chrome, which is the gate this menu has always
                  had. Visibility is the caller's, so the input `⋯` can widen
                  the keys item to a coarse-pointer tablet without widening it
                  here. */}
              <InputMenuItems
                gates={{
                  attach: false,
                  surfaceSwitch: false,
                  keysToggle: isMobile,
                  topBarToggle: isMobile,
                }}
                trailingSeparator
              />
              {/* The new-tab editor entry only, matching the agent screen's
                  menu: the in-app overlay is desktop-only, so its item would be
                  a dead no-op here. A real anchor, so a long-press keeps its
                  native open-in-new-tab. */}
              <DropdownMenuItem
                render={
                  <a
                    href={standaloneEditorHash(
                      editorRootForTarget({
                        kind: "terminal",
                        terminalId,
                        owner,
                      }),
                    )}
                    target="_blank"
                    rel="noopener"
                  />
                }
              >
                <ExternalLink />
                Open editor in new tab
              </DropdownMenuItem>
              <DropdownMenuSeparator />
              <DropdownMenuItem onClick={() => openDeleteTerminal(terminalId)}>
                <X />
                Close…
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
        </header>
      ) : null}
      <div className="min-h-0 flex-1">
        <ChunkBoundary>
          <Suspense fallback={null}>
            <LazyTerminalPane
              key={terminalId}
              kind="terminal"
              id={terminalId}
              owner={owner}
            />
          </Suspense>
        </ChunkBoundary>
      </div>
    </div>
  )
}

// The project-owned spoke: the crumb is the project's name, and a project that
// is no longer in the workspace has no screen, so it lands home.
function ProjectTerminalScreen({
  owner,
  terminalId,
}: {
  owner: Extract<TerminalOwnerRef, { kind: "project" }>
  terminalId: string
}) {
  const { spine } = useDux()
  const project = spine?.projects.find((p) => p.id === owner.projectId)
  if (!project) return <HomeScreen />
  return (
    <AgentlessTerminalScreen
      owner={owner}
      terminalId={terminalId}
      primary={project.name}
    />
  )
}

// The standalone spoke: the crumb is the DIRECTORY the terminal opened in,
// `~`-shortened by the server, which is the same thing its sidebar row says.
// There is no owner that could have gone missing, so unlike the project screen
// there is nothing to fall home for: a terminal id the spine no longer carries
// is handled by the router, not here.
function StandaloneTerminalScreen({
  owner,
  terminalId,
}: {
  owner: Extract<TerminalOwnerRef, { kind: "standalone" }>
  terminalId: string
}) {
  const { spine } = useDux()
  const terminal = spine?.terminals.find((t) => t.id === terminalId)
  const cwd =
    terminal?.owner.kind === "standalone" ? terminal.owner.cwd_label : null
  return (
    <AgentlessTerminalScreen
      owner={owner}
      terminalId={terminalId}
      primary={cwd ?? "Standalone terminal"}
    />
  )
}

// The phone header's identity block: the two lanes described at the call site.
// It renders `mobileHeaderLanes`, which derives both lanes from the desktop
// chip model, and the shared `CHIP_GLYPHS`, so a chip kind is drawn as the same
// glyph on both surfaces.
function MobileHeaderLanes({
  session,
  provider,
  projectName,
}: {
  session: SessionView
  provider: string
  projectName?: string | null
}) {
  const { lead, rest } = mobileHeaderLanes({
    name: sessionLabel(session),
    provider,
    projectName,
    // A STANDALONE agent's answer to the project question. Without it the phone
    // header said only the agent's name and its assistant, so nothing on screen
    // said where it was working; the chip model has carried the directory chip
    // all along and this call site simply never handed it the label.
    folderLabel: folderWorkspace(session.workspace)?.folder_label,
    branchName: workspaceBranchName(session.workspace),
  })
  const LeadGlyph = CHIP_GLYPHS[lead.kind]
  return (
    <>
      <div className="flex min-w-0 items-center gap-1.5">
        <LeadGlyph className="size-3.5 shrink-0 text-muted-foreground" />
        <span className="truncate text-sm leading-tight font-medium">
          {lead.value}
        </span>
      </div>
      <div className="flex min-w-0 items-center gap-2.5 text-[11px] leading-tight text-muted-foreground">
        {rest.map((chip) => {
          const Glyph = CHIP_GLYPHS[chip.kind]
          return (
            <span key={chip.kind} className="flex min-w-0 items-center gap-1">
              <Glyph className="size-3 shrink-0" />
              <span className="truncate">{chip.value}</span>
            </span>
          )
        })}
      </div>
    </>
  )
}

function terminalOwnerScreen(target: SelectedTarget | null): ReactElement | null {
  if (target?.kind !== "terminal") return null

  return matchOwner<ReactElement | null>(target.owner, {
    session: () => null,
    project: (owner) => (
      <ProjectTerminalScreen owner={owner} terminalId={target.terminalId} />
    ),
    standalone: (owner) => (
      <StandaloneTerminalScreen owner={owner} terminalId={target.terminalId} />
    ),
  })
}

interface TerminalHeaderProps {
  session: SessionView
  target: SelectedTarget
  focusedTab: AgentTabView | undefined
  projectName: string | undefined
  changes: ChangesSummary
}

function TerminalHeader({
  session,
  target,
  focusedTab,
  projectName,
  changes,
}: TerminalHeaderProps) {
  return (
    <header className="flex h-11 shrink-0 items-center gap-2 border-b px-3">
      <Button
        variant="ghost"
        size="lg"
        className="min-w-11 shrink-0"
        aria-label="Back"
        onClick={() => navigateUp()}
      >
        <ChevronLeft />
      </Button>
      <div className="min-w-0 flex-1">
        <MobileHeaderLanes
          session={session}
          provider={focusedTab?.provider ?? session.provider}
          projectName={projectName}
        />
      </div>
      {session.pr ? (
        <SimpleTooltip
          content={`#${session.pr.number} · ${prStateLabel(session.pr.state)} · ${session.pr.title}`}
        >
          <a
            href={session.pr.url}
            target="_blank"
            rel="noopener noreferrer"
            aria-label={`PR #${session.pr.number} (${prStateLabel(session.pr.state)})`}
            className={cn(
              "inline-flex size-10 shrink-0 items-center justify-center rounded-md transition-colors",
              prIconClass(session.pr.state),
              prIconHoverClass(session.pr.state),
            )}
          >
            <GitPullRequest className="size-4" />
          </a>
        </SimpleTooltip>
      ) : null}
      <MacroPopover variant="icon" target={target} />
      <Button
        variant="outline"
        size="lg"
        className="min-w-11 shrink-0"
        aria-label={changes.countLabel}
        onClick={() => openChangesScreen()}
      >
        {changes.label}
      </Button>
      <DropdownMenu>
        <DropdownMenuTrigger
          render={
            <Button
              variant="outline"
              size="lg"
              className="min-w-11 shrink-0"
              aria-label="Session actions"
            />
          }
        >
          <Ellipsis />
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end">
          <AgentActionsMenu session={session} context="terminal" />
        </DropdownMenuContent>
      </DropdownMenu>
    </header>
  )
}

interface TerminalViewportProps {
  target: SelectedTarget
  focusedTab: AgentTabView | undefined
  dormant: boolean
  paneKey: string
  targetId: string
  slotTabId: string | undefined
}

function TerminalViewport({
  target,
  focusedTab,
  dormant,
  paneKey,
  targetId,
  slotTabId,
}: TerminalViewportProps) {
  if (dormant && focusedTab && target.kind === "agent") {
    return (
      <DormantTabCard
        sessionId={target.sessionId}
        tabId={focusedTab.id}
        provider={focusedTab.provider}
        lastRunFailed={focusedTab.last_run_failed === true}
      />
    )
  }

  return (
    <ChunkBoundary>
      <Suspense fallback={null}>
        {target.kind === "agent" ? (
          <LazyTerminalPane
            key={paneKey}
            kind="agent"
            id={targetId}
            sessionId={target.sessionId}
            slotTabId={slotTabId}
          />
        ) : (
          <LazyTerminalPane
            key={paneKey}
            kind="terminal"
            id={targetId}
            owner={target.owner}
          />
        )}
      </Suspense>
    </ChunkBoundary>
  )
}

function TerminalScreen() {
  const duxState = useDux()
  const {
    spine,
    bootstrap,
    selectedSessionId,
    selectedTarget,
    terminalEpoch,
    changes,
    startedDormantTabs,
    pendingSlotTab,
  } = duxState
  const ownerScreen = terminalOwnerScreen(selectedTarget)
  if (ownerScreen) return ownerScreen

  const session = spine?.sessions.find((item) => item.id === selectedSessionId)
  if (!selectedTarget || !session) return <HomeScreen />

  const targetId =
    selectedTarget.kind === "terminal"
      ? selectedTarget.terminalId
      : selectedTarget.tabId
  const paneKey =
    selectedTarget.kind === "agent" ? `${targetId}:${terminalEpoch}` : targetId
  const tabs = session.tabs ?? []
  const focusedTab =
    selectedTarget.kind === "agent"
      ? tabs.find((tab) => tab.id === selectedTarget.tabId)
      : undefined
  const slotTabId = slotTabIdOf(session.id, session, pendingSlotTab)
  // The ±N summary, from the helper the desktop header's reopen control reads
  // too. Non-null here because this screen has a session.
  const changesControl = changesSummary(changes, session.id)
  const projectName = spine?.projects.find(
    (project) => project.id === workspaceProjectId(session.workspace),
  )?.name

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      {mobileTopBarVisible(duxState) ? (
        <>
          <TerminalHeader
            session={session}
            target={selectedTarget}
            focusedTab={focusedTab}
            projectName={projectName}
            changes={changesControl}
          />
          {selectedTarget.kind === "agent" &&
          shouldShowTabStrip(tabs, bootstrap?.always_show_tab_strip ?? false) ? (
            <AgentTabsStrip
              session={session}
              activeTabId={selectedTarget.tabId}
              maxTabs={bootstrap?.agent_tabs_max}
            />
          ) : null}
        </>
      ) : null}
      <div className="min-h-0 flex-1">
        <TerminalViewport
          target={selectedTarget}
          focusedTab={focusedTab}
          dormant={dormantTabNeedsCard(
            selectedTarget,
            session,
            focusedTab,
            startedDormantTabs,
            slotTabId,
          )}
          paneKey={paneKey}
          targetId={targetId}
          slotTabId={slotTabId}
        />
      </div>
    </div>
  )
}

// The changes spoke: a slim back bar over the full-screen shared changed-files
// pane (diffs open in the full-screen Monaco editor, not a sheet).
function ChangesScreen() {
  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      <header className="flex h-12 shrink-0 items-center gap-2 border-b px-3">
        {/* Up from changes is the agent it belongs to, not a history step:
            a deep link straight to `#/agent/<sid>/changes` pushed nothing, so
            stepping from here leaves the app. */}
        <Button
          variant="ghost"
          size="icon"
          className="size-11 shrink-0"
          aria-label="Back"
          onClick={() => navigateUp()}
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

// The screen the URL names, plus the one screen the URL cannot name: a route
// pointing at an agent that no longer exists. Not-found is checked first for
// readability, not because the branches compete: `setRouteNotFound` commits
// `mobileScreen: "home"` in the same patch, and `setState` clears the flag on
// any patch that carries a target, so "not-found AND terminal/changes" is not a
// state that occurs. What the check does have to stay ahead of is the HUB,
// which is the fallthrough at the bottom rather than a test of its own.
export function MobileShell() {
  const { mobileScreen, routeNotFound } = useDux()

  if (routeNotFound) return <AgentNotFound sessionId={routeNotFound.sessionId} />
  if (mobileScreen === "terminal") return <TerminalScreen />
  if (mobileScreen === "changes") return <ChangesScreen />
  return <HomeScreen />
}
