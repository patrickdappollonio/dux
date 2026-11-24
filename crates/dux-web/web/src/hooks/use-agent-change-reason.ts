import { useMenuReason, type MenuReasonHandle } from "@/hooks/use-menu-reason"
import {
  agentChangeBlockReason,
  useAgentChangeBlock,
  useRowDriverLabel,
} from "@/hooks/use-agent-active-elsewhere"
import type { SessionView } from "@/lib/types"

/**
 * Why nothing in an agent's menu may change the agent right now, as a reason
 * with no text when it may. Made by whoever opens the menu, so a terminal's
 * group under the agent's actions can name the same line the agent's menu says.
 */
export function useAgentChangeReason(session: SessionView | null): MenuReasonHandle {
  const block = useAgentChangeBlock(session)
  const device = useRowDriverLabel(session === null ? null : { sessionId: session.id })
  return useMenuReason(
    block === null || session === null ? null : agentChangeBlockReason(block, device),
  )
}
