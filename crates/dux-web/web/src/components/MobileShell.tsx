import { ChevronLeft, Ellipsis, GitPullRequest, Settings } from "lucide-react"
import { Suspense, useState, type ReactElement } from "react"

import { AddProjectSplitButton } from "@/components/AddProjectSplitButton"
import { AgentNotFound } from "@/components/AgentNotFound"
import { NewAgentSplitButton } from "@/components/NewAgentSplitButton"
import { AppMenuSheet } from "@/components/AppMenuSheet"
import { ChangedFiles } from "@/components/ChangedFiles"
import { ChunkBoundary } from "@/components/ChunkBoundary"
import { LazyTerminalPane } from "@/components/LazyTerminalPane"
import { ConnDot } from "@/components/ConnDot"
import { AgentTabsStrip } from "@/components/AgentTabsStrip"
import { DormantTabCard } from "@/components/DormantTabCard"
import { AgentActionsMenu, FlatAgentList } from "@/components/FlatAgentList"
import { SimpleTooltip } from "@/components/SimpleTooltip"
import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { isExtraTabDormant, shouldShowTabStrip } from "@/lib/agentTabs"
import { resolveInstanceTitle } from "@/lib/instanceTitle"
import {
  navigateUp,
  openChangesScreen,
  selectSession,
  selectTerminal,
  useDux,
} from "@/lib/store"
import { prIconClass, prIconHoverClass, prStateLabel } from "@/lib/pr"
import type { TerminalOwnerRef } from "@/lib/store"
import { matchOwner } from "@/lib/terminalOwner"
import { terminalsForOwner, terminalTitle } from "@/lib/terminals"
import { cn } from "@/lib/utils"

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
// sidebar. The New-agent picker button + search + sort live in the list header;
// the Add-project split button sits below it.
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

      <div className="flex shrink-0 items-center gap-2 border-t p-3">
        <NewAgentSplitButton className="flex-1" />
        <AddProjectSplitButton className="flex-1" />
      </div>
    </div>
  )
}

// The spoke for a PROJECT-owned terminal: a project-name crumb over the shared
// terminal. It has no agent, so it borrows none of the agent screen's chrome
// (no changes chip, no agent actions menu).
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
  // This project's own terminals, selected out of the flat collection by owner,
  // so the crumb still disambiguates against its true siblings.
  const projectTerminals = terminalsForOwner(spine?.terminals ?? [], owner)
  const terminal = projectTerminals.find((t) => t.id === terminalId)
  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
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
          <span className="truncate font-semibold">{project.name}</span>
          <span className="truncate text-muted-foreground">
            {terminal ? terminalTitle(terminal, projectTerminals) : "Terminal"}
          </span>
        </div>
      </header>
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

// The focused-terminal spoke: a slim top bar over the full-screen shared terminal.
function TerminalScreen() {
  const {
    spine,
    bootstrap,
    selectedSessionId,
    selectedTarget,
    terminalEpoch,
    changes,
    startedDormantTabs,
  } = useDux()
  const session = spine?.sessions.find((s) => s.id === selectedSessionId)
  const changeCount =
    changes.sessionId === selectedSessionId && changes.phase === "loaded"
      ? changes.staged.length + changes.unstaged.length
      : 0

  // Which screen a focused TERMINAL gets is decided by an EXHAUSTIVE match on
  // its OWNER. The two-way `owner.kind === "project"` test this replaces kept
  // compiling for any other kind of owner and fell through to the agent screen
  // below, which requires a session and therefore renders the HUB: the user taps
  // a terminal and lands on the home screen, with no error anywhere.
  const ownerScreen =
    selectedTarget?.kind === "terminal"
      ? matchOwner<ReactElement | null>(selectedTarget.owner, {
          // A session-owned terminal has no screen of its own: it renders INSIDE
          // the agent screen below, sharing that agent's header, changes chip and
          // actions menu. `null` is that answer, not an unhandled case.
          session: () => null,
          project: (owner) => (
            <ProjectTerminalScreen
              owner={owner}
              terminalId={selectedTarget.terminalId}
            />
          ),
        })
      : null
  if (ownerScreen) return ownerScreen

  if (!selectedTarget || !session) {
    return <HomeScreen />
  }

  const targetId =
    selectedTarget.kind === "terminal"
      ? selectedTarget.terminalId
      : selectedTarget.tabId
  const paneKey =
    selectedTarget.kind === "agent" ? `${targetId}:${terminalEpoch}` : targetId

  const tabs = session.tabs ?? []
  const focusedTab =
    selectedTarget.kind === "agent"
      ? tabs.find((t) => t.id === selectedTarget.tabId)
      : undefined
  const isExtraDormant = isExtraTabDormant(
    selectedTarget,
    focusedTab,
    startedDormantTabs,
  )

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      <header className="flex h-11 shrink-0 items-center gap-2 border-b px-3">
        {/* Up to the hub, by name (see the project-terminal header above). */}
        <Button
          variant="ghost"
          size="icon"
          className="size-10 shrink-0"
          aria-label="Back"
          onClick={() => navigateUp()}
        >
          <ChevronLeft />
        </Button>
        <div className="min-w-0 flex-1 text-sm">
          <span className="truncate font-mono">{session.branch_name}</span>
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
        <Button
          variant="outline"
          size="sm"
          className="min-h-10 shrink-0"
          aria-label={`${changeCount} changed files`}
          onClick={() => openChangesScreen()}
        >
          ±{changeCount}
        </Button>
        <DropdownMenu>
          <DropdownMenuTrigger
            render={
              <Button
                variant="ghost"
                size="icon"
                className="size-10 shrink-0"
                aria-label="Session actions"
              />
            }
          >
            <Ellipsis />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <AgentActionsMenu session={session} />
          </DropdownMenuContent>
        </DropdownMenu>
      </header>

      {selectedTarget.kind === "agent" &&
      shouldShowTabStrip(tabs, bootstrap?.always_show_tab_strip ?? false) ? (
        <AgentTabsStrip
          session={session}
          activeTabId={selectedTarget.tabId}
          maxTabs={bootstrap?.agent_tabs_max}
        />
      ) : null}

      <div className="min-h-0 flex-1">
        {isExtraDormant && focusedTab && selectedTarget.kind === "agent" ? (
          <DormantTabCard
            sessionId={selectedTarget.sessionId}
            tabId={focusedTab.id}
            provider={focusedTab.provider}
          />
        ) : (
          <ChunkBoundary>
            <Suspense fallback={null}>
              {selectedTarget.kind === "agent" ? (
                <LazyTerminalPane
                  key={paneKey}
                  kind="agent"
                  id={targetId}
                  sessionId={selectedTarget.sessionId}
                />
              ) : (
                <LazyTerminalPane
                  key={paneKey}
                  kind="terminal"
                  id={targetId}
                  owner={selectedTarget.owner}
                />
              )}
            </Suspense>
          </ChunkBoundary>
        )}
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
