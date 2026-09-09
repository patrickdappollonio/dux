import type * as React from "react"

import { SimpleTooltip } from "@/components/SimpleTooltip"
import type { TooltipContent } from "@/components/ui/tooltip"

// The single "needs attention" marker for every surface, so the markup and the
// color live in one place; it holds still under reduced motion. The fill must stay
// in lockstep with `ATTENTION_DOT_FILL` in `lib/favicon.ts`, which draws the same
// dot onto a canvas where a Tailwind class is unreadable.
export function AttentionDot({
  withTooltip = true,
  side,
}: {
  /** Wrap the dot in the shared `SimpleTooltip`. On by default; `MobileShell`
   * passes `false` because a touch surface has no hover hint. */
  withTooltip?: boolean
  side?: React.ComponentProps<typeof TooltipContent>["side"]
}) {
  const dot = (
    <span
      aria-label="Needs attention"
      className="size-2 shrink-0 rounded-full bg-cyan-100 motion-safe:animate-attention-pulse motion-reduce:animate-none"
    />
  )
  if (!withTooltip) return dot
  return (
    <SimpleTooltip content="Needs attention" side={side}>
      {dot}
    </SimpleTooltip>
  )
}
