import {
  agentHeaderChips,
  directoryChip,
  focusedTerminalChip,
  type AgentChipsInput,
  type HeaderChip,
} from "@/lib/headerSubject"
import type { TerminalTarget } from "@/lib/editorRoot"
import type { DuxState, SelectedTarget } from "@/lib/store"
import { matchOwner } from "@/lib/terminalOwner"
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

// The facts describing one AGENT. Shared by an agent selection, where the
// agent's own name is the primary chip, and by a session-owned terminal, where
// the terminal is primary and the agent's chips sit in front of it.
function agentFacts(
  spine: Spine,
  agent: SessionView,
  provider: string | undefined,
  terminalCount: number,
  primary: "agent" | "none",
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
    primary,
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
      session: (owner) => {
        const agent = spine?.sessions.find((s) => s.id === owner.sessionId)
        // The agent is no longer the primary chip here, and its terminal COUNT
        // is suppressed: the focused terminal's own chip carries that count in
        // its hover clause, and two terminal glyphs in one row would read as
        // two different terminals.
        return {
          chips: agent
            ? agentHeaderChips(agentFacts(spine, agent, undefined, 0, "none"))
            : [],
          siblings: terminalsForOwner(allTerminals, owner),
        }
      },
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

export function insetHeaderChips(
  spine: Spine,
  session: SessionView | undefined,
  selectedTarget: SelectedTarget | null,
): HeaderChip[] {
  if (selectedTarget?.kind === "terminal") {
    return terminalChips(spine, selectedTarget)
  }
  if (!session) return []
  // When an agent tab is focused, the assistant chip reflects the FOCUSED TAB
  // (an extra tab can run a different provider than the session-slot tab), not
  // the session-slot tab's own provider.
  const focusedTabProvider =
    selectedTarget?.kind === "agent"
      ? session.tabs.find((t) => t.id === selectedTarget.tabId)?.provider
      : undefined
  const sessionTerminals = terminalsForOwner(spine?.terminals ?? [], {
    kind: "session",
    sessionId: session.id,
  })
  return agentHeaderChips(
    agentFacts(
      spine,
      session,
      focusedTabProvider,
      sessionTerminals.length,
      "agent",
    ),
  )
}
