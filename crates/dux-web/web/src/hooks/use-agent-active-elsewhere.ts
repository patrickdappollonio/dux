import { assertNever } from "@/lib/assertNever"
import type { EditorRoot } from "@/lib/editorRoot"
import { useOpenerVerdict } from "@/lib/openerPage"
import { sessionLabel, workspaceProjectId } from "@/lib/agentWorkspace"
import {
  agentRowKey,
  rowBlockFor,
  rowKeyOfPart,
  rowKeyOfTarget,
  useAgentRowWatch,
  useRowWatch,
  type RowBlock,
} from "@/lib/paneCover"
import {
  partChangeBlock,
  rowDriverLabel,
  sessionActiveElsewhere,
  terminalActiveElsewhere,
  useDux,
  useDuxSelector,
  type DuxState,
} from "@/lib/store"
import type { SessionView } from "@/lib/types"

/// Why this device may not change an agent right now, or null when it may. A
/// mounted pane of the row hears a hand-over or a take-over at once, so its
/// answer wins over the spine's, which can only say that another device drives.
export function agentChangeBlock(
  rowAnswer: RowBlock | null | undefined,
  spineAnswer: () => boolean,
): RowBlock | null {
  if (rowAnswer !== undefined) return rowAnswer
  return spineAnswer() ? "other" : null
}

export function useAgentChangeBlock(
  session: SessionView | null | undefined,
): RowBlock | null {
  const state = useDux()
  const rowAnswer = useAgentRowWatch(session?.id).block
  if (session == null) return null
  return agentChangeBlock(rowAnswer, () => sessionActiveElsewhere(state, session))
}

/// The same answer for an agent known by id, whose session the spine may not
/// list yet: its mounted panes answer, and the spine only once it lists it.
export function useAgentChangeBlockById(sessionId: string): RowBlock | null {
  const state = useDux()
  const rowAnswer = useAgentRowWatch(sessionId).block
  const session = state.spine?.sessions.find((s) => s.id === sessionId)
  return agentChangeBlock(
    rowAnswer,
    () => session !== undefined && sessionActiveElsewhere(state, session),
  )
}

/// The same answer by id for a surface that subscribes selectively, so it
/// re-renders when the answer changes rather than on every store change.
export function useAgentChangeBlockSelective(
  sessionId: string | null,
): RowBlock | null {
  const rowAnswer = useAgentRowWatch(sessionId).block
  const spineElsewhere = useDuxSelector((state) => agentActiveElsewhere(state, sessionId))
  if (sessionId === null) return null
  return agentChangeBlock(rowAnswer, () => spineElsewhere)
}

/// Why this page may not write the files under an editor root, or null when
/// it may: the agent's answer for an agent's root, the terminal row's own for a
/// terminal's. A standalone editor tab writes as the page that opened it, so
/// where the spine says another device drives, the server says whether that is its opener.
export function useEditorRootChangeBlock(root: EditorRoot): RowBlock | null {
  const state = useDux()
  const rowAnswer = useRowWatch(rowKeyOfTarget(root)).block
  const block = agentChangeBlock(rowAnswer, () => {
    if (root.kind === "terminal") return terminalActiveElsewhere(state, root.terminalId)
    return agentActiveElsewhere(state, root.sessionId)
  })
  const asOpener = state.standaloneEditor && rowAnswer === undefined && block === "other"
  const verdict = useOpenerVerdict(root, asOpener ? rowDriverInSpine(state, root) : null)
  switch (verdict) {
    case null:
      return block
    case "allowed":
      return null
    case "asking":
      return "pending"
    case "refused":
      return "other"
    default:
      return assertNever(verdict)
  }
}

/// The driver the spine names for a root's row, or null when it names none.
function rowDriverInSpine(state: DuxState, root: EditorRoot): string | null {
  const terminals = state.spine?.terminals ?? []
  const owners =
    root.kind === "terminal"
      ? [terminals.find((t) => t.id === root.terminalId)?.input_owner]
      : [
          ...(state.spine?.sessions.find((s) => s.id === root.sessionId)?.tabs ?? []).map(
            (tab) => tab.input_owner,
          ),
          ...terminals
            .filter((t) => t.owner.kind === "session" && t.owner.session_id === root.sessionId)
            .map((t) => t.input_owner),
        ]
  return owners.find((owner) => owner !== undefined && owner !== "") ?? null
}

/// The spine's word on whether another device drives an agent, false while the
/// spine does not list it.
function agentActiveElsewhere(state: DuxState, sessionId: string | null): boolean {
  const session = state.spine?.sessions.find((s) => s.id === sessionId)
  return session !== undefined && sessionActiveElsewhere(state, session)
}

/// The same answer for every agent of a list, by session id.
export function useAgentChangeBlocks(
  sessions: readonly SessionView[],
): Map<string, RowBlock | null> {
  const state = useDux()
  // Re-renders on any pane's change, so every row's answer below stays current.
  useRowWatch(null)
  return new Map(
    sessions.map((session) => [
      session.id,
      agentChangeBlock(rowBlockFor(agentRowKey(session.id)), () =>
        sessionActiveElsewhere(state, session),
      ),
    ]),
  )
}

/// An agent or a terminal of a project that another device drives, by name,
/// with that device's label when a mounted pane of it heard which.
export interface DrivenElsewhere {
  kind: "agent" | "terminal"
  name: string
  device: string | null
}

/// Every agent and project terminal of `projectId` this page can tell another
/// device drives: a change to the project would change their rows, which the
/// server refuses.
export function useProjectDrivenElsewhere(projectId: string | null): DrivenElsewhere[] {
  const state = useDux()
  useRowWatch(null)
  if (projectId === null) return []
  const agents = (state.spine?.sessions ?? [])
    .filter((session) => workspaceProjectId(session.workspace) === projectId)
    .filter(
      (session) =>
        agentChangeBlock(rowBlockFor(agentRowKey(session.id)), () =>
          sessionActiveElsewhere(state, session),
        ) === "other",
    )
    .map((session) => ({
      kind: "agent" as const,
      name: sessionLabel(session),
      device: rowDriverLabel(state, { sessionId: session.id }),
    }))
  const terminals = (state.spine?.terminals ?? [])
    .filter((t) => t.owner.kind === "project" && t.owner.project_id === projectId)
    .map((t) => ({ t, row: rowKeyOfPart({ kind: "terminal", id: t.id, owner: { kind: "project" } }) }))
    .filter(
      ({ t, row }) =>
        agentChangeBlock(rowBlockFor(row), () => terminalActiveElsewhere(state, t.id)) ===
        "other",
    )
    .map(({ t }) => ({
      kind: "terminal" as const,
      name: t.label,
      device: rowDriverLabel(state, { terminalId: t.id }),
    }))
  return [...agents, ...terminals]
}

/** A project dialog's disabled confirm's hint while another device drives an
 * agent or a terminal of the project. */
export function drivenElsewhereHint(driven: readonly DrivenElsewhere[]): string | null {
  if (driven.length === 0) return null
  if (driven.length > 1) {
    return "Agents or terminals of this project are being driven from another device. Take them over on this device first."
  }
  const [part] = driven
  const who = part.device ?? "Another device"
  return `${who} is driving ${part.kind === "agent" ? "an agent" : "a terminal"} of this project. Take it over on this device first.`
}

/// Why a menu's entries that change the agent are disabled right now, said once
/// at the head of the menu.
export function agentChangeBlockReason(block: RowBlock, device: string | null = null): string {
  return changeBlockReason(block, "agent", device)
}

/// The label of the device driving a part's row, as `rowDriverLabel` says it,
/// re-read when a pane of any row hears a new driver. Selective, so the
/// memoized Changes pane re-renders when the label moves and not on every
/// store change.
export function useRowDriverLabel(
  part: { sessionId: string } | { terminalId: string } | null,
): string | null {
  useRowWatch(null)
  const sessionId = part !== null && "sessionId" in part ? part.sessionId : null
  const terminalId = part !== null && "terminalId" in part ? part.terminalId : null
  return useDuxSelector((state) => {
    if (sessionId !== null) return rowDriverLabel(state, { sessionId })
    if (terminalId !== null) return rowDriverLabel(state, { terminalId })
    return null
  })
}

/// Why controls that change an agent's or a terminal's row are disabled,
/// naming the driving device when it is known.
export function changeBlockReason(
  block: RowBlock,
  subject: "agent" | "terminal",
  device: string | null = null,
): string {
  switch (block) {
    case "other":
      return `This ${subject} is active on ${device ?? "another device"}, so actions that modify it are disabled. Take over the ${subject} to use them here.`
    case "nobody":
      return `Nobody is driving this ${subject}, this device included, so actions that modify it are disabled. Take over the ${subject} to use them here.`
    case "reconnecting":
      return `This device is reconnecting to this ${subject}, so actions that modify it are disabled until it hears again who is driving.`
    case "pending":
      return `This device is still connecting to this ${subject}, so actions that modify it are disabled until it knows who is driving.`
  }
}

/// Why this page may not change one row right now, in words, or null when it
/// may. Selective, so it re-renders when the answer moves and not on every
/// store change.
export function useRowChangeReason(
  part: { sessionId: string } | { terminalId: string } | null,
): string | null {
  useRowWatch(null)
  const sessionId = part !== null && "sessionId" in part ? part.sessionId : null
  const terminalId = part !== null && "terminalId" in part ? part.terminalId : null
  return useDuxSelector((state) => {
    const row = sessionId !== null ? { sessionId } : terminalId !== null ? { terminalId } : null
    if (row === null) return null
    const block = partChangeBlock(state, row)
    if (block === null) return null
    return changeBlockReason(block, "sessionId" in row ? "agent" : "terminal", rowDriverLabel(state, row))
  })
}

/// Why this page may not change the row a part belongs to, in words, or null
/// when it may.
export function usePartChangeReason(): (
  part: { sessionId: string } | { terminalId: string },
) => string | null {
  const state = useDux()
  // Re-renders on any pane's change, so every answer below stays current.
  useRowWatch(null)
  return (part) => {
    const block = partChangeBlock(state, part)
    if (block === null) return null
    return changeBlockReason(
      block,
      "sessionId" in part ? "agent" : "terminal",
      rowDriverLabel(state, part),
    )
  }
}

/// Why an editor is read-only right now, said once at its head, for an agent's
/// working copy or a terminal's folder.
export function editorReadOnlyReason(
  block: RowBlock,
  subject: "agent" | "terminal",
  device: string | null = null,
): string {
  switch (block) {
    case "other":
      return `Read-only: this ${subject} is active on ${device ?? "another device"}, so its files cannot be changed here. Take over the ${subject} to edit them.`
    case "nobody":
      return `Read-only: nobody is driving this ${subject}, this device included, so its files cannot be changed here. Take over the ${subject} to edit them.`
    case "reconnecting":
      return `Read-only while this device reconnects to this ${subject}: its files can be changed again once it hears who is driving.`
    case "pending":
      return `Read-only while this device connects to this ${subject}: its files can be changed once it knows who is driving.`
  }
}

/// Why this device cannot start the agent right now, short enough for a band.
export function startAgentBlockHint(block: RowBlock): string {
  switch (block) {
    case "reconnecting":
      return "Reconnecting to this agent"
    case "pending":
      return "Still connecting to this agent"
    case "other":
    case "nobody":
      return "Take over to start it"
  }
}

/// Why this device cannot add a tab or a terminal to the agent right now,
/// naming the driving device when it is known.
export function addToAgentBlockHint(block: RowBlock, device: string | null): string {
  switch (block) {
    case "reconnecting":
      return "Reconnecting to this agent"
    case "pending":
      return "Still connecting to this agent"
    case "other":
      return `This agent is active on ${device ?? "another device"}. Take over to add a tab or a terminal`
    case "nobody":
      return "Take over to add a tab or a terminal"
  }
}
