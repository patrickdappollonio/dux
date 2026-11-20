import { useAgentRowWatch } from "@/lib/paneCover"
import { sessionActiveElsewhere, useDux } from "@/lib/store"
import type { SessionView } from "@/lib/types"

/// Does another device drive this agent's row? A mounted pane of the row hears
/// a hand-over or a take-over at once, so its answer wins over the spine's.
export function agentDrivenElsewhere(
  rowAnswer: boolean | undefined,
  spineAnswer: () => boolean,
): boolean {
  return rowAnswer ?? spineAnswer()
}

export function useAgentActiveElsewhere(session: SessionView | null | undefined): boolean {
  const state = useDux()
  const rowAnswer = useAgentRowWatch(session?.id).elsewhere
  if (session == null) return false
  return agentDrivenElsewhere(rowAnswer, () => sessionActiveElsewhere(state, session))
}

/// Why a menu's entries that change the agent are disabled right now, said once
/// at the head of the menu.
export const AGENT_ELSEWHERE_REASON =
  "This agent is active on another device, so actions that modify it are disabled. Take over in its terminal to use them here."

/// Why this device cannot add a tab or a terminal to the agent right now.
export const ADD_TO_AGENT_ELSEWHERE_HINT = "Take over to add a tab or a terminal"
