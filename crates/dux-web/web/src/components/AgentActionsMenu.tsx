import {
  Bot,
  Check,
  ClipboardCopy,
  Cpu,
  ExternalLink,
  FileCode2,
  Folder,
  FolderSync,
  GitFork,
  GitPullRequest,
  Info,
  Pencil,
  Play,
  Plus,
  RefreshCw,
  RotateCcw,
  ScrollText,
  SquareChevronRight,
  SquareTerminal,
  Trash2,
  Unplug,
  Variable,
} from "lucide-react"

import { MenuItemReason, MenuReasonsHead } from "@/components/MenuReason"
import { useMenuReason, type MenuReasonHandle } from "@/hooks/use-menu-reason"
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
import { ADDING_TAB_HINT, defaultProviderForSession, tabLimitHint } from "@/lib/agentTabs"
import { InlineCode } from "@/components/ui/inline-code"
import {
  newTerminalLabel,
  supportsBranchGit,
  workspaceDirectory,
  workspaceProjectId,
} from "@/lib/agentWorkspace"
import { DEFAULT_AGENT_TABS_MAX } from "@/lib/bootstrapApi"
import { agentIsDetachable } from "@/lib/detachAgent"
import {
  MENU_GROUP_LABELS,
  locationGroupLabel,
  tabsGroupLabel,
} from "@/lib/menuGroupLabels"
import { canRecreateWorkingCopy } from "@/lib/recreateWorkingCopy"
import { agentRoot } from "@/lib/editorRoot"
import { clipboardWorktree } from "@/lib/flatClipboard"
import { EDITOR_TAB_REL } from "@/lib/openerPage"
import {
  addTab,
  createTerminal,
  openAgentEnv,
  openAgentInfo,
  openAgentStartupCommand,
  openPullRequests,
  openChangeProvider,
  openDelete,
  openEditor,
  openForceReconnect,
  openForkAgent,
  openRecreateWorkingCopy,
  openRename,
  openStartupLogs,
  openStopAgent,
  rerunStartupCommand,
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
export function AgentActionsMenu({
  session,
  changeReason,
}: {
  session: SessionView
  /** What `useAgentChangeReason` made for this agent, which the menu says at its head. */
  changeReason: MenuReasonHandle
}) {
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
  // Whether the branch-identity features exist for this agent at all: fork,
  // pull requests, startup commands. They are about a branch dux manages, and
  // a standalone agent has none whatever its folder contains.
  const branchGit = supportsBranchGit(session.workspace)
  // While this device does not drive the agent's row, the mutating entries
  // disable, each naming the one reason said at the head of the menu.
  const changesBlocked = changeReason.text !== null
  const describedBy = changeReason.describedBy
  // Whether there is a process to ask to shut down at all. The SERVER's answer,
  // from the same oracle the engine's teardown and the terminal UI's palette
  // gate ask, rather than a scan of the tab rows here.
  const liveProcess = agentIsDetachable(session)
  const copyMissing = canRecreateWorkingCopy(session.workspace)

  return (
    <>
      <MenuReasonsHead reasons={[changeReason]} />
      {/* The changed-file row and the shared input-menu items belong to
        * `PaneMenu`, the one menu every surface opens, which renders this body
        * as its agent group with the pane's input group above it. */}
      <DropdownMenuGroup>
        <DropdownMenuLabel>{tabsGroupLabel(session.workspace)}</DropdownMenuLabel>
        <AgentTabSubmenu
          sessionId={session.id}
          providers={providers}
          defaultProvider={defaultProvider}
          tabCap={tabCap}
          atTabCap={atTabCap}
          addingTab={addingTab}
          changeReason={changeReason}
        />
        <AgentProjectSubmenu projectId={projectId} projectName={projectName} />
      </DropdownMenuGroup>
      <DropdownMenuSeparator />
      <DropdownMenuGroup>
        <DropdownMenuLabel>{MENU_GROUP_LABELS.running}</DropdownMenuLabel>
        <DropdownMenuItem
          disabled={changesBlocked}
          aria-describedby={describedBy}
          onClick={() => openForceReconnect(session.id)}
        >
          <RotateCcw />
          Force recreate agent…
        </DropdownMenuItem>
        {/* Absent unless the working copy is actually gone: there is nothing to
          * put back otherwise, and this is the one control that exists only in
          * that state, because it IS the way out of it. A standalone agent never
          * gets it: dux does not create, move or remove the user's own folder. */}
        {copyMissing ? (
          <DropdownMenuItem
            disabled={changesBlocked}
            aria-describedby={describedBy}
            onClick={() => openRecreateWorkingCopy(session.id)}
          >
            <FolderSync />
            Recreate working copy…
          </DropdownMenuItem>
        ) : null}
        {/* Absent, not disabled, with nothing running: a detach asks a process to
          * go, so with none there is nothing to ask, and a disabled row would
          * promise an action that is not waiting on the user. Neutral colour like
          * every other destructive item here; the dialog is the danger signal. */}
        {liveProcess ? (
          <DropdownMenuItem
            disabled={changesBlocked}
            aria-describedby={describedBy}
            onClick={() => openStopAgent(session.id)}
          >
            <Unplug />
            Detach agent…
          </DropdownMenuItem>
        ) : null}
        <DropdownMenuItem
          disabled={changesBlocked}
          aria-describedby={describedBy}
          onClick={() => toggleSessionAutoReopen(session.id, !session.auto_reopen_enabled)}
        >
          <RefreshCw />
          {session.auto_reopen_enabled
            ? "Disable agent auto-reopen"
            : "Enable agent auto-reopen"}
        </DropdownMenuItem>
      </DropdownMenuGroup>
      <DropdownMenuSeparator />
      <DropdownMenuGroup>
        <DropdownMenuLabel>{MENU_GROUP_LABELS.agent}</DropdownMenuLabel>
        <DropdownMenuItem
          disabled={changesBlocked}
          aria-describedby={describedBy}
          onClick={() => openRename(session.id)}
        >
          <Pencil />
          Rename agent…
        </DropdownMenuItem>
        <AgentIdentityItems
          sessionId={session.id}
          branchGit={branchGit}
          changeReason={changeReason}
        />
      </DropdownMenuGroup>
      {branchGit ? (
        <>
          <DropdownMenuSeparator />
          <AgentStartupItems sessionId={session.id} changeReason={changeReason} />
        </>
      ) : null}
      <DropdownMenuSeparator />
      <DropdownMenuGroup>
        <DropdownMenuLabel>{locationGroupLabel(session.workspace)}</DropdownMenuLabel>
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
              rel={EDITOR_TAB_REL}
            />
          }
        >
          <ExternalLink />
          Open editor in new tab
        </DropdownMenuItem>
        {/* The label names where the shell opens, and is workspace-derived rather
          * than fixed: this menu also serves a standalone agent, which has a
          * folder and no worktree. */}
        <DropdownMenuItem
          disabled={changesBlocked}
          aria-describedby={describedBy}
          onClick={() => createTerminal(session.id)}
        >
          <SquareTerminal />
          {newTerminalLabel(session.workspace)}
        </DropdownMenuItem>
        <DropdownMenuItem
          onClick={() => clipboardWorktree(workspaceDirectory(session.workspace))}
        >
          <ClipboardCopy />
          Copy local path
        </DropdownMenuItem>
      </DropdownMenuGroup>
      <DropdownMenuSeparator />
      <DropdownMenuGroup>
        <DropdownMenuLabel>{MENU_GROUP_LABELS.remove}</DropdownMenuLabel>
        {/* The one deliberate red-tinted destructive menu item (dim at rest, bright
            on hover), per the menu rules in `web/AGENTS.md`; the confirm dialog gates it. */}
        <DropdownMenuItem
          variant="destructive"
          className="not-focus:text-destructive/70! not-focus:*:[svg]:text-destructive/70!"
          disabled={changesBlocked}
          aria-describedby={describedBy}
          onClick={() => openDelete(session.id)}
        >
          <Trash2 />
          Delete agent…
        </DropdownMenuItem>
      </DropdownMenuGroup>
    </>
  )
}

function AgentTabSubmenu({
  sessionId,
  providers,
  defaultProvider,
  tabCap,
  atTabCap,
  addingTab,
  changeReason,
}: {
  sessionId: string
  providers: string[]
  defaultProvider: string
  tabCap: number
  atTabCap: boolean
  addingTab: boolean
  changeReason: MenuReasonHandle
}) {
  // The words the tab strip's `+` says for the same two states. The row's own
  // reason, said at the head of the menu, outranks them, as it does there.
  const limitReason = useMenuReason(
    changeReason.text !== null
      ? null
      : atTabCap
        ? tabLimitHint(tabCap)
        : addingTab
          ? ADDING_TAB_HINT
          : null,
  )
  return (
    <>
      <DropdownMenuSub>
        <DropdownMenuSubTrigger
          disabled={atTabCap || addingTab || changeReason.text !== null}
          aria-describedby={changeReason.describedBy ?? limitReason.describedBy}
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
      <MenuItemReason reason={limitReason} />
    </>
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
          {projectName ? <>Project <InlineCode>{projectName}</InlineCode>…</> : <>Project…</>}
        </span>
      </DropdownMenuSubTrigger>
      <DropdownMenuSubContent>
        <ProjectMenuItems id={projectId} />
      </DropdownMenuSubContent>
    </DropdownMenuSub>
  )
}

function AgentIdentityItems({
  sessionId,
  branchGit,
  changeReason,
}: {
  sessionId: string
  branchGit: boolean
  changeReason: MenuReasonHandle
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
        disabled={changeReason.text !== null}
        aria-describedby={changeReason.describedBy}
        onClick={() => openChangeProvider(sessionId)}
      >
        <Cpu />
        Change agent provider…
      </DropdownMenuItem>
      {/* Offered with or without gh: the dialog says when attaching needs a
        * signed-in gh, and hiding or detaching talks to nothing, so a pull
        * request never outlives the way to remove it. Left enabled while
        * another device drives the agent: the list ends nobody's terminal. */}
      {branchGit ? (
        <DropdownMenuItem onClick={() => openPullRequests(sessionId)}>
          <GitPullRequest />
          Pull requests…
        </DropdownMenuItem>
      ) : null}
      <DropdownMenuItem onClick={() => openAgentInfo(sessionId)}>
        <Info />
        Agent info…
      </DropdownMenuItem>
    </>
  )
}

// Startup is its own group so its heading can be dropped with it: a standalone
// agent has no startup command, and a heading over nothing says nothing.
function AgentStartupItems({
  sessionId,
  changeReason,
}: {
  sessionId: string
  changeReason: MenuReasonHandle
}) {
  return (
    <DropdownMenuGroup>
      <DropdownMenuLabel>{MENU_GROUP_LABELS.startup}</DropdownMenuLabel>
      <DropdownMenuItem onClick={() => openAgentStartupCommand(sessionId)}>
        <SquareChevronRight />
        Configure startup command…
      </DropdownMenuItem>
      <DropdownMenuItem onClick={() => openAgentEnv(sessionId)}>
        <Variable />
        Configure environment variables…
      </DropdownMenuItem>
      {/* Runs a command in the agent's working copy, so it is a change. */}
      <DropdownMenuItem
        disabled={changeReason.text !== null}
        aria-describedby={changeReason.describedBy}
        onClick={() => rerunStartupCommand(sessionId)}
      >
        <Play />
        Rerun startup command
      </DropdownMenuItem>
      <DropdownMenuItem onClick={() => openStartupLogs(sessionId)}>
        <ScrollText />
        Startup command logs…
      </DropdownMenuItem>
    </DropdownMenuGroup>
  )
}
