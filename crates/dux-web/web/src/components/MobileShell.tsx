import { ChevronLeft, GitPullRequest, Settings } from "lucide-react"
import { Suspense, useState, type ReactElement, type ReactNode } from "react"

import { AgentNotFound } from "@/components/AgentNotFound"
import { LauncherCorner } from "@/components/LauncherCorner"
import { AppMenuSheet } from "@/components/AppMenuSheet"
import { ChangedFiles } from "@/components/ChangedFiles"
import { ChunkBoundary } from "@/components/ChunkBoundary"
import { LazyTerminalPane } from "@/components/LazyTerminalPane"
import { ConnDot } from "@/components/ConnDot"
import { AgentTabsStrip } from "@/components/AgentTabsStrip"
import { DormantTabSurface } from "@/components/DormantTabSurface"
import { FlatAgentList } from "@/components/FlatAgentList"
import { CHIP_GLYPHS } from "@/components/headerChipGlyphs"
import { MobileActionFlap } from "@/components/MobileActionFlap"
import type { PaneMenuSubject } from "@/components/PaneMenu"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { TheaterChrome } from "@/components/TheaterChrome"
import { TheaterPill } from "@/components/TheaterPill"
import { Button } from "@/components/ui/button"
import { useTheaterFlight } from "@/hooks/use-theater-flight"
import { flapMounted, flapVisible, pillMounted } from "@/lib/theaterFlight"
import {
  dormantTabNeedsCard,
  shouldShowTabStrip,
  slotTabIdOf,
} from "@/lib/agentTabs"
import { mobileHeaderLanes } from "@/lib/headerSubject"
import { resolveInstanceTitle } from "@/lib/instanceTitle"
import {
  navigateUp,
  selectSession,
  selectTerminal,
  useDux,
} from "@/lib/store"
import {
  prAriaLabel,
  prIconClass,
  prIconHoverClass,
  prStateLabel,
} from "@/lib/pr"
import type { SelectedTarget, TerminalOwnerRef } from "@/lib/store"
import type { AgentTabView, SessionView } from "@/lib/types"
import { matchOwner } from "@/lib/terminalOwner"
import { terminalsForOwner, terminalTitle } from "@/lib/terminals"
import { cn } from "@/lib/utils"
import {
  folderWorkspace,
  sessionLabel,
  workspaceBranchName,
  workspaceProjectId,
} from "@/lib/agentWorkspace"

// Tapping a session focuses it, and the screen is derived from the URL that
// selection writes, so there is no second navigation call to make.
function selectAndOpen(sessionId: string): void {
  selectSession(sessionId)
}

function selectTerminalAndOpen(
  terminalId: string,
  owner: TerminalOwnerRef,
): void {
  selectTerminal(terminalId, owner)
}

// The hub: the shared flat agent list at touch size. Search, sort and the
// new-agent + live in the list header; the launcher corner sits in the bottom
// bar, the same component and size tokens as the desktop sidebar's footer.
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

// The spoke for a terminal that is not session-owned: one identity crumb over
// the shared terminal. It has no agent, so its flap carries no changed-file
// count and its header no pull-request chip.
//
// Everything else is the agent screen's: the actions hang off the band as the
// flap, fly into the floating pill and back, and the `⋯` opens the terminal's
// own merged menu, leaving the header Back and the identity.
//
// Shared by the project-owned and standalone screens, which differ only in what
// the crumb says and what must exist for the screen to be valid; the wrappers
// below own that difference, so the spokes cannot drift in layout or targets.
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
  // On the phone shell the app header is the chrome stack theater takes away,
  // and theater is deliberately the only way to hide it: two flows for one
  // intent could disagree about what is on screen.
  const theater = duxState.theater
  // The one phase both clusters render from, so the handoff cannot land in the
  // gap between two controls each deciding for itself.
  const flight = useTheaterFlight()
  const target: SelectedTarget = { kind: "terminal", terminalId, owner }
  const subject: PaneMenuSubject = { kind: "terminal", terminalId, owner }
  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      <TheaterChrome hidden={theater}>
        <header className="flex h-11 shrink-0 items-center gap-2 border-b px-3">
          {/* Up to the hub, by name: a relative history step walks out of the
            * app whenever this screen is the entry the browser opened on. */}
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
        </header>
      </TheaterChrome>
      <div className="relative min-h-0 flex-1">
        {/* The flap is a sibling of the pane, not part of its overlay, which is
          * withheld while a full-pane cover owns the terminal; these are the
          * only controls the phone has left. The band is always plain here,
          * since only an agent can have a strip to hang from. */}
        {flapMounted(flight) ? (
          <MobileActionFlap
            target={target}
            subject={subject}
            band="plain"
            hidden={!flapVisible(flight)}
          />
        ) : null}
        <ChunkBoundary>
          <Suspense fallback={null}>
            <LazyTerminalPane
              key={terminalId}
              kind="terminal"
              id={terminalId}
              owner={owner}
              // The only chrome left in theater: everything else lives in the
              // header and the flap's dock, which the mode takes away. It is the
              // flap's own cluster in the air, carrying the terminal's menu.
              overlay={
                pillMounted(flight) ? (
                  <TheaterPill
                    target={target}
                    session={undefined}
                    flight={flight}
                  />
                ) : null
              }
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

// The standalone spoke: the crumb is the directory the terminal opened in,
// `~`-shortened by the server, as its sidebar row says. There is no owner that
// could go missing, so nothing falls home from here; a terminal id the spine no
// longer carries is the router's case.
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

// The phone header's identity block. It renders `mobileHeaderLanes`, which
// derives both lanes from the desktop chip model, and the shared `CHIP_GLYPHS`,
// so a chip kind is drawn as the same glyph on both surfaces.
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
    // A standalone agent's answer to the project question: without the label,
    // nothing in the header says where the agent is working.
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
  focusedTab: AgentTabView | undefined
  projectName: string | undefined
}

// The agent screen's header: Back, the identity, and the pull request.
//
// It carries no actions, which is what buys the identity the whole remaining
// width: the agent's name, assistant, branch and project are what tell you which
// of half a dozen near-identical terminals you are looking at. The actions live
// in the flap hanging off the band below.
//
// The pull request stays as the compact chip, the phone's whole PR surface: one
// tap to the review, opening the same URL every other PR control does. It
// carries `#N` beside the glyph, as the sidebar row and the desktop banner do,
// so the chip can be matched against the tab open in the review.
function TerminalHeader({ session, focusedTab, projectName }: TerminalHeaderProps) {
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
            aria-label={prAriaLabel(session.pr.number, session.pr.state)}
            className={cn(
              "inline-flex h-10 w-auto shrink-0 items-center justify-center gap-1 rounded-md px-2 transition-colors",
              prIconClass(session.pr.state),
              prIconHoverClass(session.pr.state),
            )}
          >
            <GitPullRequest className="size-4 shrink-0" />
            {/* The number is data, so it stays where this surface otherwise
              * prefers icon-only, and the chip is content-sized rather than
              * square. `shrink-0` makes the identity give up width first: a
              * truncated agent name still reads, half a PR number does not. */}
            <span className="text-xs font-medium tabular-nums">
              #{session.pr.number}
            </span>
          </a>
        </SimpleTooltip>
      ) : null}
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
  overlay: ReactNode
}

function TerminalViewport({
  target,
  focusedTab,
  dormant,
  paneKey,
  targetId,
  slotTabId,
  overlay,
}: TerminalViewportProps) {
  // Over a live terminal the overlay belongs inside the pane's own positioned
  // box, because the compose row and the terminal keys sit under the terminal in
  // this column. A dormant card has no input rows, so there it rides the column.
  if (dormant && focusedTab && target.kind === "agent") {
    return (
      <>
        <DormantTabSurface
          sessionId={target.sessionId}
          tabId={focusedTab.id}
          provider={focusedTab.provider}
          lastRunFailed={focusedTab.last_run_failed === true}
          lastRunVerdict={focusedTab.last_run_verdict}
        />
        {overlay}
      </>
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
            overlay={overlay}
          />
        ) : (
          <LazyTerminalPane
            key={paneKey}
            kind="terminal"
            id={targetId}
            owner={target.owner}
            overlay={overlay}
          />
        )}
      </Suspense>
    </ChunkBoundary>
  )
}

// Which pane screen is on, and nothing else: a router with no state of its own.
// The flight machine belongs to the screen, and a router holding one too would
// step timers and re-render this tree for a flight it is not showing.
function TerminalScreen() {
  const { spine, selectedSessionId, selectedTarget } = useDux()
  const ownerScreen = terminalOwnerScreen(selectedTarget)
  if (ownerScreen) return ownerScreen

  const session = spine?.sessions.find((item) => item.id === selectedSessionId)
  if (!selectedTarget || !session) return <HomeScreen />
  return <AgentTerminalScreen session={session} target={selectedTarget} />
}

// The agent spoke: the agent's chrome stack over its pane, with the flap and
// the pill rendered from the ONE flight phase this screen owns.
function AgentTerminalScreen({
  session,
  target: selectedTarget,
}: {
  session: SessionView
  target: SelectedTarget
}) {
  const duxState = useDux()
  const { spine, bootstrap, terminalEpoch, startedDormantTabs, pendingSlotTab } =
    duxState
  const flight = useTheaterFlight()

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
  const projectName = spine?.projects.find(
    (project) => project.id === workspaceProjectId(session.workspace),
  )?.name
  const stripShown =
    selectedTarget.kind === "agent" &&
    shouldShowTabStrip(tabs, bootstrap?.always_show_tab_strip ?? false)

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      {/* The phone shell's chrome stack: the header and the tab strip, which
        * leave together on the one flag. The actions beside them are in the flap
        * below, which detaches into the floating pill rather than leaving.
        * Theater is the only thing that hides them. */}
      <TheaterChrome hidden={duxState.theater}>
        <TerminalHeader
          session={session}
          focusedTab={focusedTab}
          projectName={projectName}
        />
        {stripShown ? (
          <AgentTabsStrip
            session={session}
            activeTabId={selectedTarget.tabId}
            maxTabs={bootstrap?.agent_tabs_max}
          />
        ) : null}
      </TheaterChrome>
      <div className="relative min-h-0 flex-1">
        {/* The flap is a sibling of the pane, not part of its overlay, which is
          * withheld while a full-pane cover owns the terminal. */}
        {flapMounted(flight) ? (
          <MobileActionFlap
            target={selectedTarget}
            subject={{ kind: "agent", session }}
            band={stripShown ? "strip" : "plain"}
            hidden={!flapVisible(flight)}
          />
        ) : null}
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
          overlay={
            pillMounted(flight) ? (
              <TheaterPill
                target={selectedTarget}
                session={session}
                flight={flight}
              />
            ) : null
          }
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
        {/* Up from changes is the agent it belongs to, not a history step: a
          * deep link straight to the changes screen pushed nothing. */}
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

// The screen the URL names, plus the one it cannot name: a route pointing at an
// agent that no longer exists. Not-found is checked first only to stay ahead of
// the hub, which is the fallthrough at the bottom; it cannot compete with the
// other branches, since `setRouteNotFound` commits the home screen in the same
// patch and any patch carrying a target clears the flag.
export function MobileShell() {
  const { mobileScreen, routeNotFound } = useDux()

  if (routeNotFound) return <AgentNotFound sessionId={routeNotFound.sessionId} />
  if (mobileScreen === "terminal") return <TerminalScreen />
  if (mobileScreen === "changes") return <ChangesScreen />
  return <HomeScreen />
}
