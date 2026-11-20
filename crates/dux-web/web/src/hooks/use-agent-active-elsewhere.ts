import { useAgentRowWatch, type RowBlock } from "@/lib/paneCover"
import { sessionActiveElsewhere, useDux } from "@/lib/store"
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

/// Why a menu's entries that change the agent are disabled right now, said once
/// at the head of the menu.
export function agentChangeBlockReason(block: RowBlock): string {
  switch (block) {
    case "other":
      return "This agent is active on another device, so actions that modify it are disabled. Take over the agent to use them here."
    case "nobody":
      return "Nobody is driving this agent, this device included, so actions that modify it are disabled. Take over the agent to use them here."
    case "pending":
      return "This device is still connecting to this agent, so actions that modify it are disabled until it knows who is driving."
  }
}

/// Why this device cannot add a tab or a terminal to the agent right now.
export function addToAgentBlockHint(block: RowBlock): string {
  return block === "pending"
    ? "Still connecting to this agent"
    : "Take over to add a tab or a terminal"
}
