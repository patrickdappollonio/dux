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
  supportsBranchGit,
  workspaceDirectory,
  workspaceProjectId,
} from "@/lib/agentWorkspace"
import { DEFAULT_AGENT_TABS_MAX } from "@/lib/bootstrapApi"
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
  rerunStartupCommand,
  resumePullRequestAutodetection,
  sessionActiveElsewhere,
  standaloneEditorHash,
  toggleSessionAutoReopen,
  useDux,
} from "@/lib/store"
import type { SessionView } from "@/lib/types"

// The AGENT'S OWN ACTIONS, every per-agent entry from the parity inventory, in
// one place so no surface can drift from another.
//
// It is never rendered on its own: `AgentPaneMenuBody` is the menu, and this is
// the group inside it that is about the agent. That is why the pane's INPUT
// group is not here — it is the wrapper's, once per menu, above these rows —
// and why there is no context parameter left to pass: the sidebar row, the
// desktop pane header, the phone's flap and the floating pill all open the one
// merged body.
export function AgentActionsMenu({ session }: { session: SessionView }) {
  const duxState = useDux()
  const { bootstrap, spine, createTabInFlight } = duxState
  const tabCap = bootstrap?.agent_tabs_max ?? DEFAULT_AGENT_TABS_MAX
  const atTabCap = session.tabs.length >= tabCap
  const addingTab = createTabInFlight.includes(session.id)
  const providers = bootstrap?.available_providers ?? []
  const defaultProvider = defaultProviderForSession(spine, session)
  // The Project submenu names the PROJECT, because its actions affect the whole
  // project, not just this agent. The tab submenu names nothing: the agent's own
  // name sits beside the menu in every placement (the row, the mobile terminal
  // header), so repeating it in the label is noise.
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
      {/* The changed-file row and the shared input-menu items used to be here.
          They live in `PaneMenu` now, the ONE menu every surface opens (the
          phone's docked flap, the floating pill, the desktop pane header's `⋯`
          and this row's), and which renders this body as its agent group with
          the pane's INPUT group above it. */}
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
