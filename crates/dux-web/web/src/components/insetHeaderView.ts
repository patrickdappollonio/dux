import {
  agentHeaderChips,
  directoryChip,
  focusedTerminalChip,
  type AgentChipsInput,
  type HeaderChip,
} from "@/lib/headerSubject"
import type { PaneMenuSubject } from "@/components/PaneMenu"
import type { TerminalTarget } from "@/lib/editorRoot"
import type { DuxState, SelectedTarget } from "@/lib/store"
import { matchOwner, ownerSessionId } from "@/lib/terminalOwner"
import { terminalsForOwner, terminalTitle } from "@/lib/terminals"
import type { SessionView, TerminalView } from "@/lib/types"
import {
  managedWorkspace,
  sessionLabel,
  workspaceLocation,
} from "@/lib/agentWorkspace"

// What the desktop center-pane top bar names: the chips describing the pane's
// subject, in the order they are drawn. Which chips exist and what each says
// lives in `lib/headerSubject.ts`; this module decides which subject is asked.

type Spine = DuxState["spine"]

// The facts describing one AGENT, whose own name is the primary chip.
function agentFacts(
  spine: Spine,
  agent: SessionView,
  provider: string | undefined,
  terminalCount: number,
): AgentChipsInput {
  // A standalone agent has no project; it names its FOLDER in the same slot
  // instead, through the chip a standalone terminal already uses.
  const location = workspaceLocation(agent.workspace)
  const owningProject =
    location.kind === "project"
      ? spine?.projects.find((p) => p.id === location.projectId)
      : undefined
  const managed = managedWorkspace(agent.workspace)
  return {
    name: sessionLabel(agent),
    provider: provider ?? agent.provider,
    projectName: owningProject?.name,
    folderLabel: location.kind === "folder" ? location.label : undefined,
    branchName: managed?.branch_name ?? null,
    initialBranch: managed?.initial_branch ?? null,
    terminalCount,
  }
}

// The owner's own chips plus that owner's terminals: the set `terminalTitle`
// disambiguates against, and the set the sibling count counts. Chosen by an
// exhaustive match on the terminal's owner, so a new kind of owner is a compile
// error rather than a blank bar.
function terminalOwnerContext(
  spine: Spine,
  focused: TerminalTarget,
): { chips: HeaderChip[]; siblings: TerminalView[] } {
  const allTerminals = spine?.terminals ?? []
  return matchOwner<{ chips: HeaderChip[]; siblings: TerminalView[] }>(
    focused.owner,
    {
      // Never asked: an agent's own terminal is named by its agent's row (see
      // `insetHeaderChips`), so it has no chips of its own here.
      session: () => ({ chips: [], siblings: [] }),
      project: (owner) => {
        const owningProject = spine?.projects.find(
          (p) => p.id === owner.projectId,
        )
        return {
          chips: owningProject
            ? [
                {
                  kind: "project" as const,
                  label: "Project",
                  value: owningProject.name,
                },
              ]
            : [],
          siblings: terminalsForOwner(allTerminals, owner),
        }
      },
      // No owner to name, so the context names where the terminal is. The label
      // comes off this terminal's own wire owner: every standalone terminal
      // shares one client-side reference, which carries no id and no label, and
      // only the terminal knows its directory.
      standalone: (owner) => {
        const siblings = terminalsForOwner(allTerminals, owner)
        const self = siblings.find((t) => t.id === focused.terminalId)
        const cwd =
          self?.owner.kind === "standalone" ? self.owner.cwd_label : null
        return { chips: cwd ? [directoryChip(cwd)] : [], siblings }
      },
    },
  )
}

// The chips for a focused terminal: its owner's, with the terminal's own chip
// spliced in where a terminal chip always lands, after the branch and before the
// assistant. Its value is the terminal's own title rather than a count, because
// the terminal is the thing on screen.
function terminalChips(spine: Spine, focused: TerminalTarget): HeaderChip[] {
  const { chips, siblings } = terminalOwnerContext(spine, focused)
  const terminal = siblings.find((t) => t.id === focused.terminalId)
  if (!terminal) return []
  const self = focusedTerminalChip(
    terminalTitle(terminal, siblings),
    siblings.length,
  )
  return [
    ...chips.filter((c) => c.kind !== "assistant"),
    self,
    ...chips.filter((c) => c.kind === "assistant"),
  ]
}

// The header names the selected row of the agents list, never the part of it
// that has focus: an agent with its terminal count whether typing goes to one
// of its tabs or a terminal of its dock, and a project or standalone terminal,
// which is a row of its own, by itself. `shownTabId` is the tab on top of the
// agent's stack while its dock has focus. `terminalsHidden` drops the count,
// for a device that does not drive the agent and is not shown it has any.
export function insetHeaderChips(
  spine: Spine,
  session: SessionView | undefined,
  selectedTarget: SelectedTarget | null,
  shownTabId?: string,
  terminalsHidden = false,
): HeaderChip[] {
  if (selectedTarget?.kind === "terminal" && selectedTarget.owner.kind !== "session") {
    return terminalChips(spine, selectedTarget)
  }
  if (selectedTarget?.kind === "terminal") {
    const ownTerminal = spine?.terminals.some((t) => t.id === selectedTarget.terminalId)
    if (!ownTerminal || session?.id !== ownerSessionId(selectedTarget.owner)) return []
  }
  if (!session) return []
  // The assistant chip reflects the tab on top of the stack (an extra tab can
  // run a different provider than the session-slot tab), not the session-slot
  // tab's own provider.
  const tabOnTop = selectedTarget?.kind === "agent" ? selectedTarget.tabId : shownTabId
  const focusedTabProvider = session.tabs.find((t) => t.id === tabOnTop)?.provider
  const sessionTerminals = terminalsForOwner(spine?.terminals ?? [], {
    kind: "session",
    sessionId: session.id,
  })
  return agentHeaderChips(
    agentFacts(
      spine,
      session,
      focusedTabProvider,
      terminalsHidden ? 0 : sessionTerminals.length,
    ),
  )
}

// What the pane menu is about, decided the same way the chips are: the agent
// when one is behind the pane, the terminal itself when nothing is. A
// session-owned terminal takes the agent's menu, and its own Close and editor
// entries ride along as a labelled group because the menu is handed the pane as
// well as the subject.
//
// Read off the target, not the selected session, which can still name the agent
// a project terminal was reached from, and matched exhaustively on the owner so
// a fourth kind must answer for itself rather than falling into the terminal
// arm.
export function insetHeaderPaneSubject(
  spine: Spine,
  session: SessionView | undefined,
  selectedTarget: SelectedTarget | null,
): PaneMenuSubject | null {
  if (selectedTarget?.kind !== "terminal") {
    return session ? { kind: "agent", session } : null
  }
  const ownTerminal: PaneMenuSubject = {
    kind: "terminal",
    terminalId: selectedTarget.terminalId,
    owner: selectedTarget.owner,
  }
  return matchOwner<PaneMenuSubject>(selectedTarget.owner, {
    session: (owner) => {
      const agent = spine?.sessions.find((s) => s.id === owner.sessionId)
      return agent ? { kind: "agent", session: agent } : ownTerminal
    },
    project: () => ownTerminal,
    standalone: () => ownTerminal,
  })
}
