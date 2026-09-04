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
import { DormantTabCard } from "@/components/DormantTabCard"
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
// agent screen's AGENT chrome: its flap carries no changed-file count and its
// header no pull-request chip, because a terminal has neither.
//
// EVERYTHING ELSE IS THE AGENT SCREEN'S. The actions used to sit in this header
// as three icon buttons, which is the idiom the agent screen left behind: they
// hang off the band as the flap now, they fly into the floating pill on the way
// into theater and back out of it on the way home, and the `⋯` opens the
// terminal's own merged menu. The header keeps Back and the identity, which is
// what the header tenet says a phone header is for.
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
  // On the phone shell the app header IS the chrome stack theater takes away.
  // It is the only way to hide this header, deliberately: a preference that
  // hid it too was a second flow for the same intent with no way back of its
  // own, and the two could disagree about what was on screen.
  const theater = duxState.theater
  // The one phase both clusters are rendered from, exactly as the agent screen
  // does it: the flap and the pill are rendered FROM it rather than each
  // deciding for itself, so the handoff cannot land in the gap between them.
  const flight = useTheaterFlight()
  const target: SelectedTarget = { kind: "terminal", terminalId, owner }
  const subject: PaneMenuSubject = { kind: "terminal", terminalId, owner }
  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      <TheaterChrome hidden={theater}>
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
        </header>
      </TheaterChrome>
      <div className="relative min-h-0 flex-1">
        {/* The flap is a SIBLING of the pane, not part of its overlay: the
            overlay is withheld while a full-pane cover owns the terminal, and
            these are the only controls the phone has left. The band is always
            "plain" here: only an agent can have a tab strip, so there is never
            a strip for this flap to hang from. */}
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
              // THE ONLY CHROME LEFT IN THEATER, and the reason this screen has
              // one at all: everything else lives in the header and the flap's
              // dock, which the mode takes away. It is the flap's own cluster
              // in the air, carrying the terminal's menu rather than the
              // agent's, and it FLIES now that there is a dock to leave from
              // and land back on.
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
  focusedTab: AgentTabView | undefined
  projectName: string | undefined
}

// THE AGENT SCREEN'S HEADER: Back, the identity, and the pull request.
//
// It carries no actions at all. Those live in the flap hanging off the band
// below it, which is what buys the identity the whole remaining width: an agent
// name, its assistant, its branch and its project are what tell you which of
// half a dozen near-identical terminals you are looking at, and four icon
// buttons were ellipsizing all four of them down to nothing.
//
// The pull request stays, as the compact chip: it is the phone's whole PR
// surface (the desktop's wide band has no room here), it is one tap to the
// review, and it opens the same URL every other PR control in the app opens.
// It carries `#N` beside the glyph, the way the sidebar row and the desktop
// banner already do: the slimmed header has the room, and the number is what
// lets you match the chip against the tab you have open in the review.
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
            {/* The number is DATA, so it stays on the phone where this surface
                otherwise prefers icon-only, and it is why the chip is
                content-sized rather than square. `shrink-0` on the whole chip
                is what makes the IDENTITY beside it give up width first: a
                truncated agent name is still readable, half a PR number is
                not. */}
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
  // Same rule as the desktop shell's: over a live terminal the overlay belongs
  // inside the pane's own positioned box, because the compose row and the
  // terminal keys sit under the terminal in this column. A dormant card has no
  // input rows, so there it rides the column.
  if (dormant && focusedTab && target.kind === "agent") {
    return (
      <>
        <DormantTabCard
          sessionId={target.sessionId}
          tabId={focusedTab.id}
          provider={focusedTab.provider}
          lastRunFailed={focusedTab.last_run_failed === true}
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

// WHICH PANE SCREEN IS ON, and nothing else.
//
// It is deliberately a router with no state of its own: the flight machine is
// the screen's, and a screen that hands over to another must not be running one
// too. This used to hold the hook above its own early returns, so an agentless
// terminal screen mounted a second machine that stepped its own timers and
// re-rendered this tree on every stage of a flight it was not showing.
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
      {/* The phone shell's chrome stack: its header AND the tab strip, which is
          what "the app header goes with them" means here. Both leave on the one
          flag; the actions they used to sit beside are in the flap below, which
          detaches into the floating pill rather than leaving with them. Theater
          is the only thing that hides them: a preference that hid the top bar
          too was a second flow for the same intent with no way back of its
          own. */}
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
        {/* The flap is a SIBLING of the pane, not part of its overlay: the
            overlay is withheld while a full-pane cover owns the terminal, and
            these are the only controls the phone has left. */}
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
