import type * as React from "react"

import { SimpleTooltip } from "@/components/SimpleTooltip"
import type { TooltipContent } from "@/components/ui/tooltip"

// The single "needs attention" marker used across every surface (sidebar rows,
// the mobile agent rows, the tab-strip pills): an amber dot that blinks in a
// double-pulse-then-fade rhythm (`--animate-attention-pulse` in index.css) and
// holds still under reduced motion. Extracted so the markup and, crucially,
// the amber color live in exactly one place.
//
// COLOR PAIRING: the fill is Tailwind `bg-amber-400`. The favicon compositor in
// `lib/favicon.ts` draws the same dot onto a canvas, where a Tailwind class is
// unreadable, so it hardcodes the matching hex (`ATTENTION_DOT_FILL = #fbbf24`,
// amber-400). Keep the two in lockstep: if you change the dot color here, change
// `ATTENTION_DOT_FILL` there too.
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
      className="size-2 shrink-0 rounded-full bg-amber-400 motion-safe:animate-attention-pulse motion-reduce:animate-none"
    />
  )
  if (!withTooltip) return dot
  return (
    <SimpleTooltip content="Needs attention" side={side}>
      {dot}
    </SimpleTooltip>
  )
}
