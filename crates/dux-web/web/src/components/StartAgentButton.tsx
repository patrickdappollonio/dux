import type { ReactNode } from "react"

import { Button } from "@/components/ui/button"
import {
  agentChangeBlockReason,
  useRowDriverLabel,
  startAgentBlockHint,
  useAgentChangeBlockById,
} from "@/hooks/use-agent-active-elsewhere"

/**
 * A Start for a stopped agent or one of its tabs. Starting changes the agent's
 * row, so while this device may not change it the button is disabled and says
 * why beside it: the short hint in a band's one line, the whole reason on a card.
 */
export function StartAgentButton({
  sessionId,
  onStart,
  reason = "full",
  size,
  className,
  label,
  children,
}: {
  sessionId: string
  onStart: () => void
  reason?: "full" | "hint"
  size?: "sm"
  className?: string
  /** The accessible name, when the visible words alone would match another
   * Start on screen that starts something else. */
  label?: string
  children: ReactNode
}) {
  const block = useAgentChangeBlockById(sessionId)
  const device = useRowDriverLabel({ sessionId })
  const why =
    block === null
      ? null
      : reason === "hint"
        ? startAgentBlockHint(block)
        : agentChangeBlockReason(block, device)
  return (
    <>
      {why !== null && reason === "hint" ? (
        <span className="shrink-0 text-xs text-muted-foreground">{why}</span>
      ) : null}
      <Button
        size={size}
        className={className}
        aria-label={label}
        disabled={why !== null}
        onClick={onStart}
      >
        {children}
      </Button>
      {why !== null && reason === "full" ? (
        <p className="max-w-md text-xs text-muted-foreground">{why}</p>
      ) : null}
    </>
  )
}
