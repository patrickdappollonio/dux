import { ChevronLeft, Ellipsis, GitPullRequest, Settings } from "lucide-react"
import { Suspense, useState } from "react"

import { AddProjectSplitButton } from "@/components/AddProjectSplitButton"
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
  mobileNavigate,
  selectSession,
  selectTerminal,
  useDux,
} from "@/lib/store"
import { prIconClass, prIconHoverClass, prStateLabel } from "@/lib/pr"
import type { TerminalOwnerRef } from "@/lib/store"
import { terminalTitle } from "@/lib/terminals"
import { cn } from "@/lib/utils"

// Tapping a session on the hub focuses it AND drives the spoke navigation, so the
// user lands on the full-screen terminal in one tap.
function selectAndOpen(sessionId: string): void {
  selectSession(sessionId)
  mobileNavigate("terminal")
}

function selectTerminalAndOpen(
  terminalId: string,
  owner: TerminalOwnerRef,
): void {
  selectTerminal(terminalId, owner)
  mobileNavigate("terminal")
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

  if (
    selectedTarget?.kind === "terminal" &&
    selectedTarget.owner.kind === "project"
  ) {
    const owner = selectedTarget.owner
    const project = spine?.projects.find((p) => p.id === owner.projectId)
    if (!project) return <HomeScreen />
    const terminal = project.terminals.find(
      (t) => t.id === selectedTarget.terminalId,
    )
    return (
      <div className="flex h-full min-h-0 flex-col overflow-hidden">
        <header className="flex h-11 shrink-0 items-center gap-2 border-b px-3">
          <Button
            variant="ghost"
            size="icon"
            className="size-10 shrink-0"
            aria-label="Back"
            onClick={() => history.back()}
          >
            <ChevronLeft />
          </Button>
          <div className="flex min-w-0 flex-1 items-baseline gap-1.5 text-sm">
            <span className="truncate font-semibold">{project.name}</span>
            <span className="truncate text-muted-foreground">
              {terminal ? terminalTitle(terminal, project.terminals) : "Terminal"}
            </span>
          </div>
        </header>
        <div className="min-h-0 flex-1">
          <ChunkBoundary>
            <Suspense fallback={null}>
              <LazyTerminalPane
                key={selectedTarget.terminalId}
                kind="terminal"
                id={selectedTarget.terminalId}
                owner={owner}
              />
            </Suspense>
          </ChunkBoundary>
        </div>
      </div>
    )
  }

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
        <Button
          variant="ghost"
          size="icon"
          className="size-10 shrink-0"
          aria-label="Back"
          onClick={() => history.back()}
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
