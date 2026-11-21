import { assertNever } from "@/lib/assertNever"
import type { EditorRoot } from "@/lib/editorRoot"
import { useOpenerVerdict } from "@/lib/openerPage"
import {
  agentRowKey,
  rowBlockFor,
  rowKeyOfTarget,
  useAgentRowWatch,
  useRowWatch,
  type RowBlock,
} from "@/lib/paneCover"
import {
  sessionActiveElsewhere,
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

/// The spine's word on whether another device drives a terminal: a mounted
/// pane's verdict first, then the terminal's owner against this page's own ids.
function terminalActiveElsewhere(state: DuxState, terminalId: string): boolean {
  const local = state.ptyOwnership?.[terminalId]
  if (local !== undefined) return local === "elsewhere"
  const owner = state.spine?.terminals.find((t) => t.id === terminalId)?.input_owner
  return owner !== undefined && owner !== "" && !state.ownPtyConnIds?.[owner]
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

/// Why a menu's entries that change the agent are disabled right now, said once
/// at the head of the menu.
export function agentChangeBlockReason(block: RowBlock): string {
  switch (block) {
    case "other":
      return "This agent is active on another device, so actions that modify it are disabled. Take over the agent to use them here."
    case "nobody":
      return "Nobody is driving this agent, this device included, so actions that modify it are disabled. Take over the agent to use them here."
    case "reconnecting":
      return "This device is reconnecting to this agent, so actions that modify it are disabled until it hears again who is driving."
    case "pending":
      return "This device is still connecting to this agent, so actions that modify it are disabled until it knows who is driving."
  }
}

/// Why an editor is read-only right now, said once at its head, for an agent's
/// working copy or a terminal's folder.
export function editorReadOnlyReason(
  block: RowBlock,
  subject: "agent" | "terminal",
): string {
  switch (block) {
    case "other":
      return `Read-only: this ${subject} is active on another device, so its files cannot be changed here. Take over the ${subject} to edit them.`
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

/// Why this device cannot add a tab or a terminal to the agent right now.
export function addToAgentBlockHint(block: RowBlock): string {
  switch (block) {
    case "reconnecting":
      return "Reconnecting to this agent"
    case "pending":
      return "Still connecting to this agent"
    case "other":
    case "nobody":
      return "Take over to add a tab or a terminal"
  }
}
