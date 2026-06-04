import {
  Bot,
  ChevronLeft,
  Download,
  Ellipsis,
  FolderOpen,
  GitCommitHorizontal,
  GitPullRequest,
  Plus,
  RefreshCw,
  Search,
  Send,
  Settings,
  SquareTerminal,
  Trash2,
  X,
} from "lucide-react"

import { ChangedFiles } from "@/components/ChangedFiles"
import { StatusBadge } from "@/components/StatusBadge"
import { TerminalPane } from "@/components/TerminalPane"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import {
  Empty,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty"
import { ScrollArea } from "@/components/ui/scroll-area"
import { CONN_BADGE } from "@/lib/conn"
import { partitionProjects } from "@/lib/projects"
import {
  createTerminal,
  deleteTerminal,
  mobileNavigate,
  openAddProject,
  openCommit,
  openCreateAgent,
  openDelete,
  openProjectSettings,
  openRemoveProject,
  selectSession,
  selectTerminal,
  setPaletteOpen,
  socket,
  useDux,
} from "@/lib/store"
import type { SelectedTarget } from "@/lib/store"
import type { PrView, SessionView } from "@/lib/types"

// Mirror the sidebar's PR badge coloring (the one intentional semantic-color
// exception: GitHub PR states carry real-world green/purple/red meaning).
function prBadgeClass(state: PrView["state"]): string {
  if (state === "open") return "border-transparent bg-green-600/15 text-green-500"
  if (state === "merged") return "border-transparent bg-purple-600/15 text-purple-400"
  return "border-transparent bg-red-600/15 text-red-400"
}

// The set of actions the sidebar's ⋯ menu offers for a session, reused verbatim
// here so the mobile menu and the desktop menu never drift.
function SessionActions({ session }: { session: SessionView }) {
  function handleToggleAutoReopen() {
    socket.sendCommand("toggle_agent_auto_reopen", {
      session_id: session.id,
      enabled: !session.auto_reopen_enabled,
    })
  }

  function handlePush() {
    socket.sendCommand("push", { session_id: session.id })
  }

  function handlePull() {
    socket.sendCommand("pull", { session_id: session.id })
  }

  return (
    <DropdownMenuGroup>
      <DropdownMenuItem onClick={() => selectAndOpen(session.id)}>
        <SquareTerminal />
        Stream
      </DropdownMenuItem>
      <DropdownMenuSeparator />
      <DropdownMenuItem onClick={handleToggleAutoReopen}>
        <RefreshCw />
        {session.auto_reopen_enabled
          ? "Disable auto-reopen"
          : "Enable auto-reopen"}
      </DropdownMenuItem>
      <DropdownMenuItem onClick={handlePush}>
        <Send />
        Push
      </DropdownMenuItem>
      <DropdownMenuItem onClick={handlePull}>
        <Download />
        Pull
      </DropdownMenuItem>
      <DropdownMenuItem onClick={() => openCommit(session.id)}>
        <GitCommitHorizontal />
        Commit…
      </DropdownMenuItem>
      <DropdownMenuSeparator />
      <DropdownMenuItem onClick={() => createTerminal(session.id)}>
        <SquareTerminal />
        New terminal
      </DropdownMenuItem>
      <DropdownMenuSeparator />
      <DropdownMenuItem
        className="text-destructive"
        onClick={() => openDelete(session.id)}
      >
        <Trash2 />
        Delete…
      </DropdownMenuItem>
    </DropdownMenuGroup>
  )
}

// Tapping a session on the hub focuses it AND drives the spoke navigation, so
// the user lands on the full-screen terminal in one tap.
function selectAndOpen(sessionId: string): void {
  selectSession(sessionId)
  mobileNavigate("terminal")
}

function selectTerminalAndOpen(terminalId: string, sessionId: string): void {
  selectTerminal(terminalId, sessionId)
  mobileNavigate("terminal")
}

// One companion terminal nested under its session — touch-sized, with a close
// action spaced away from the row tap target.
function TerminalRow({
  terminal,
  sessionId,
  active,
}: {
  terminal: SessionView["terminals"][number]
  sessionId: string
  active: boolean
}) {
  return (
    <div className="flex items-center gap-1 pl-6">
      <Button
        variant={active ? "secondary" : "ghost"}
        className="min-h-11 flex-1 justify-start gap-2 px-2"
        onClick={() => selectTerminalAndOpen(terminal.id, sessionId)}
      >
        <SquareTerminal />
        <span className="flex-1 truncate text-left">{terminal.label}</span>
      </Button>
      <Button
        variant="ghost"
        size="icon"
        className="size-11 shrink-0"
        aria-label="Close terminal"
        onClick={() => deleteTerminal(terminal.id)}
      >
        <X />
      </Button>
    </div>
  )
}

// A session row on the hub: tap to stream, with the same ⋯ actions the sidebar
// offers and the companion terminals nested beneath.
function SessionRow({
  session,
  selectedTarget,
}: {
  session: SessionView
  selectedTarget: SelectedTarget | null
}) {
  const label = session.title || session.branch_name
  const agentSelected =
    selectedTarget?.kind === "agent" && selectedTarget.sessionId === session.id

  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center gap-1">
        <Button
          variant={agentSelected ? "secondary" : "ghost"}
          className="min-h-11 flex-1 justify-start gap-2 px-2"
          onClick={() => selectAndOpen(session.id)}
        >
          <Bot />
          <span className="flex-1 truncate text-left">{label}</span>
          <span className="flex shrink-0 items-center gap-1">
            {session.pr ? (
              <Badge
                className={prBadgeClass(session.pr.state)}
                title={session.pr.title}
                render={
                  <a
                    href={session.pr.url}
                    target="_blank"
                    rel="noopener noreferrer"
                    onClick={(event) => {
                      event.stopPropagation()
                      window.open(session.pr!.url, "_blank", "noopener")
                    }}
                  >
                    <GitPullRequest data-icon="inline-start" />#
                    {session.pr.number}
                  </a>
                }
              />
            ) : null}
            <StatusBadge status={session.status} iconOnly />
          </span>
        </Button>
        <DropdownMenu>
          <DropdownMenuTrigger
            render={
              <Button
                variant="ghost"
                size="icon"
                className="size-11 shrink-0"
                aria-label="Session actions"
              />
            }
          >
            <Ellipsis />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <SessionActions session={session} />
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      {session.terminals.map((terminal) => (
        <TerminalRow
          key={terminal.id}
          terminal={terminal}
          sessionId={session.id}
          active={
            selectedTarget?.kind === "terminal" &&
            selectedTarget.terminalId === terminal.id
          }
        />
      ))}
    </div>
  )
}

// A project heading plus its session rows (or "New agent" entry point when it
// has none), with the same ⋯ actions the sidebar's project menu offers.
function ProjectBlock({
  id,
  name,
  sessions,
  selectedTarget,
}: {
  id: string
  name: string
  sessions: SessionView[]
  selectedTarget: SelectedTarget | null
}) {
  return (
    <div className="flex flex-col gap-1">
      <div className="flex items-center gap-1">
        <div className="flex min-h-11 flex-1 items-center gap-2 px-2">
          <span className="truncate font-semibold">{name}</span>
          {sessions.length > 0 ? (
            <Badge variant="secondary" className="shrink-0">
              {sessions.length}
            </Badge>
          ) : null}
        </div>
        <DropdownMenu>
          <DropdownMenuTrigger
            render={
              <Button
                variant="ghost"
                size="icon"
                className="size-11 shrink-0"
                aria-label="Project actions"
              />
            }
          >
            <Ellipsis />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <DropdownMenuItem onClick={() => openCreateAgent(id)}>
              <Bot />
              New agent…
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem onClick={() => openProjectSettings(id)}>
              <Settings />
              Project settings…
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            <DropdownMenuItem
              className="text-destructive"
              onClick={() => openRemoveProject(id)}
            >
              <Trash2 />
              Remove project…
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
      </div>

      {sessions.map((session) => (
        <SessionRow
          key={session.id}
          session={session}
          selectedTarget={selectedTarget}
        />
      ))}
    </div>
  )
}

// The hub: a grouped, tappable list of every project and its sessions, mirroring
// the sidebar's partition (projects with agents first, agent-less ones under
// their own heading).
function HomeScreen() {
  const { viewModel, selectedTarget, conn } = useDux()
  const sessions = viewModel?.sessions ?? []
  const projects = viewModel?.projects ?? []
  const { grouped, withAgents, withoutAgents, projectName } = partitionProjects(
    projects,
    sessions,
  )
  const badge = CONN_BADGE[conn]
  const hasProjects = projects.length > 0 || sessions.length > 0

  const renderProject = (projectId: string) => (
    <ProjectBlock
      key={projectId}
      id={projectId}
      name={projectName(projectId)}
      sessions={grouped.get(projectId) ?? []}
      selectedTarget={selectedTarget}
    />
  )

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      <header className="flex h-12 shrink-0 items-center gap-2 border-b px-3">
        <span className="font-semibold">dux</span>
        <div className="ml-auto flex items-center gap-2">
          <Button
            variant="outline"
            size="icon"
            aria-label="Search"
            onClick={() => setPaletteOpen(true)}
          >
            <Search />
          </Button>
          <Badge variant={badge.variant}>{badge.label}</Badge>
        </div>
      </header>

      {hasProjects ? (
        <ScrollArea className="min-h-0 flex-1">
          <div className="flex flex-col gap-4 p-3">
            <div className="flex flex-col gap-2">
              {withAgents.length > 0 ? (
                <p className="px-2 text-xs font-medium text-muted-foreground">
                  Projects
                </p>
              ) : null}
              {withAgents.map(renderProject)}
            </div>

            {withoutAgents.length > 0 ? (
              <div className="flex flex-col gap-2">
                <p className="px-2 text-xs font-medium text-muted-foreground">
                  Projects with no agents
                </p>
                {withoutAgents.map(renderProject)}
              </div>
            ) : null}

            <Button
              variant="outline"
              className="min-h-11 w-full"
              onClick={openAddProject}
            >
              <Plus />
              Add project
            </Button>
          </div>
        </ScrollArea>
      ) : (
        <div className="flex min-h-0 flex-1 flex-col items-center justify-center gap-4 p-6">
          <Empty className="border-0">
            <EmptyHeader>
              <EmptyMedia variant="icon">
                <FolderOpen />
              </EmptyMedia>
              <EmptyTitle>No projects</EmptyTitle>
              <EmptyDescription>Add a project to get started.</EmptyDescription>
            </EmptyHeader>
          </Empty>
          <Button
            variant="outline"
            className="min-h-11"
            onClick={openAddProject}
          >
            <Plus />
            Add project
          </Button>
        </div>
      )}
    </div>
  )
}

// The focused-terminal spoke: a slim top bar (back · project·branch · changes
// count · ⋯ actions) over the full-screen shared terminal.
function TerminalScreen() {
  const { viewModel, selectedSessionId, selectedTarget } = useDux()
  const session = viewModel?.sessions.find((s) => s.id === selectedSessionId)
  const project = session
    ? viewModel?.projects.find((p) => p.id === session.project_id)
    : undefined
  const changed = viewModel?.changed_files ?? { staged: [], unstaged: [] }
  const changeCount = changed.staged.length + changed.unstaged.length

  // Defensive fallback: the agent exited (TerminalPane reset the selection) or
  // the target was pruned while we sat here. Show the hub content rather than a
  // dead terminal. No navigation side effects in render — the next Back press
  // still unwinds cleanly via the popstate listener.
  if (!selectedTarget || !session) {
    return <HomeScreen />
  }

  const targetId =
    selectedTarget.kind === "terminal"
      ? selectedTarget.terminalId
      : selectedTarget.sessionId

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      <header className="flex h-12 shrink-0 items-center gap-2 border-b px-3">
        <Button
          variant="ghost"
          size="icon"
          aria-label="Back"
          onClick={() => history.back()}
        >
          <ChevronLeft />
        </Button>
        <div className="flex min-w-0 flex-1 items-center gap-1 text-sm">
          <span className="truncate">{project?.name ?? "dux"}</span>
          <span className="text-muted-foreground">·</span>
          <span className="truncate font-mono">{session.branch_name}</span>
        </div>
        <Button
          variant="outline"
          size="sm"
          className="shrink-0"
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
                className="shrink-0"
                aria-label="Session actions"
              />
            }
          >
            <Ellipsis />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end">
            <SessionActions session={session} />
          </DropdownMenuContent>
        </DropdownMenu>
      </header>

      <div className="min-h-0 flex-1">
        <TerminalPane
          key={targetId}
          kind={selectedTarget.kind}
          id={targetId}
        />
      </div>
    </div>
  )
}

// The changes spoke: a slim back bar over the full-screen shared changed-files
// pane (its diff Sheet is already 90vw-wide, i.e. near-full on phones).
function ChangesScreen() {
  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      <header className="flex h-12 shrink-0 items-center gap-2 border-b px-3">
        <Button
          variant="ghost"
          size="icon"
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
