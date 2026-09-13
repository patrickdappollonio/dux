import {
  Bot,
  Check,
  ClipboardCopy,
  Cpu,
  ExternalLink,
  FileCode2,
  Folder,
  GitFork,
  GitPullRequest,
  Info,
  Pencil,
  Play,
  Plus,
  Radar,
  RefreshCw,
  RotateCcw,
  ScrollText,
  SquareChevronRight,
  SquareTerminal,
  Trash2,
  Unlink,
  Unplug,
  Variable,
} from "lucide-react"

import { ProjectMenuItems } from "@/components/ProjectMenuItems"
import {
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
} from "@/components/ui/dropdown-menu"
import { defaultProviderForSession } from "@/lib/agentTabs"
import {
  newTerminalLabel,
  supportsBranchGit,
  workspaceDirectory,
  workspaceProjectId,
} from "@/lib/agentWorkspace"
import { DEFAULT_AGENT_TABS_MAX } from "@/lib/bootstrapApi"
import { agentIsDetachable } from "@/lib/detachAgent"
import { agentRoot } from "@/lib/editorRoot"
import { clipboardWorktree } from "@/lib/flatClipboard"
import {
  addTab,
  createTerminal,
  detachPullRequest,
  openAgentEnv,
  openAgentInfo,
  openAgentStartupCommand,
  openAttachPullRequest,
  openChangeProvider,
  openDelete,
  openEditor,
  openForceReconnect,
  openForkAgent,
  openRename,
  openStartupLogs,
  openStopAgent,
  rerunStartupCommand,
  resumePullRequestAutodetection,
  sessionActiveElsewhere,
  standaloneEditorHash,
  toggleSessionAutoReopen,
  useDux,
} from "@/lib/store"
import type { SessionView } from "@/lib/types"

// Every per-agent action, in one place so no surface can drift from another.
//
// It is never rendered on its own: `PaneMenuBody` is the menu and this is the
// group inside it about the agent, which is why the pane's input group is the
// wrapper's rather than this one's, and why there is no context to pass.
export function AgentActionsMenu({ session }: { session: SessionView }) {
  const duxState = useDux()
  const { bootstrap, spine, createTabInFlight } = duxState
  const tabCap = bootstrap?.agent_tabs_max ?? DEFAULT_AGENT_TABS_MAX
  const atTabCap = session.tabs.length >= tabCap
  const addingTab = createTabInFlight.includes(session.id)
  const providers = bootstrap?.available_providers ?? []
  const defaultProvider = defaultProviderForSession(spine, session)
  // The Project submenu names the project, whose actions affect more than this
  // agent; the tab submenu names nothing, the agent's own name being beside the
  // menu in every placement. `null` for a standalone agent, which belongs to no
  // project, read once so the submenu's presence and contents cannot disagree.
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
  // While another connection input-owns one of this agent's PTYs, the mutating
  // entries disable: deleting, renaming or relaunching an agent someone else is
  // driving surprises them. `sessionActiveElsewhere` answers from a mounted
  // pane's live verdict and from the spine's `input_owner`, which is what lets a
  // row gate an agent no pane here is attached to. Read-only entries and this
  // device's own view preferences stay usable. The reason is an inline label,
  // because a disabled item is pointer-events-none and touch has no hover.
  const activeElsewhere = sessionActiveElsewhere(duxState, session)
  // Whether there is a process to ask to shut down at all. The SERVER's answer,
  // from the same oracle the engine's teardown and the terminal UI's palette
  // gate ask, rather than a scan of the tab rows here.
  const liveProcess = agentIsDetachable(session)

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
      {/* The changed-file row and the shared input-menu items belong to
        * `PaneMenu`, the one menu every surface opens, which renders this body
        * as its agent group with the pane's input group above it. */}
      <AgentTabSubmenu
        sessionId={session.id}
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
      {/* Absent, not disabled, with nothing running: a detach asks a process to
        * go, so with none there is nothing to ask, and a disabled row would
        * promise an action that is not waiting on the user. Neutral colour like
        * every other destructive item here; the dialog is the danger signal. */}
      {liveProcess ? (
        <DropdownMenuItem
          disabled={activeElsewhere}
          onClick={() => openStopAgent(session.id)}
        >
          <Unplug />
          Detach agent…
        </DropdownMenuItem>
      ) : null}
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
      {/* Two editor entries, named for their surfaces. The in-app overlay
        * cannot open on a phone, so its item is CSS-hidden there rather than
        * left a dead no-op, and the new-tab item is the only one on phones. */}
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
      {/* The label names where the shell opens, and is workspace-derived rather
        * than fixed: this menu also serves a standalone agent, which has a
        * folder and no worktree. */}
      <DropdownMenuItem onClick={() => createTerminal(session.id)}>
        <SquareTerminal />
        {newTerminalLabel(session.workspace)}
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
  providers,
  defaultProvider,
  atTabCap,
  addingTab,
  activeElsewhere,
}: {
  sessionId: string
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
        <span className="min-w-0 truncate">New agent tab…</span>
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
