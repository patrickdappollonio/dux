import { useAgentRowWatch, type RowWatch } from "@/lib/paneCover"
import { sessionActiveElsewhere, useDux } from "@/lib/store"
import type { SessionView } from "@/lib/types"

/// Does another device drive this agent's row, for the agent's mutating
/// actions? The store answers from the agent's tabs and terminals, and a
/// mounted pane of the row answers as well, which is what covers a stopped
/// agent whose only pane is one of its terminals.
export function useAgentActiveElsewhere(session: SessionView | null | undefined): boolean {
  const state = useDux()
  const row = useAgentRowWatch(session?.id)
  if (session == null) return false
  return row.elsewhere || sessionActiveElsewhere(state, session)
}

/// May this device add a tab or a terminal to the agent? Not while another
/// device drives it. A row nobody drives (the card's "Running in the
/// background") does not block it: a mounted pane of the row tells the two
/// apart, which the store's answer cannot, and only without one is the store
/// asked.
export function addToAgentBlocked(row: RowWatch, storeElsewhere: boolean): boolean {
  return row.watched ? row.elsewhere : storeElsewhere
}

/// `addToAgentBlocked` for a component that already reads the whole store.
export function useAddToAgentBlocked(session: SessionView | null | undefined): boolean {
  const state = useDux()
  const row = useAgentRowWatch(session?.id)
  if (session == null) return false
  return addToAgentBlocked(row, sessionActiveElsewhere(state, session))
}

/// Why this device cannot add a tab or a terminal to the agent right now.
export const ADD_TO_AGENT_ELSEWHERE_HINT = "Take over to add a tab or a terminal"
