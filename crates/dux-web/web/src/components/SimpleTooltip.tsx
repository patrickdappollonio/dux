import type * as React from "react"

import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip"

// The single shadcn-Tooltip wrapper used everywhere a browser-native `title=`
// hover hint used to live. Browser tooltips render irregularly across platforms,
// so this is the standing replacement: one consistent, themed popup.
//
// Pass the hovered element through `children` (it becomes the TooltipTrigger via
// base-ui's render prop) and the hint text/side via `content`/`side`. When
// `content` is empty/nullish the trigger renders bare — callers that derive the
// hint from possibly-absent data (a truncated path, an optional reason) get a
// no-op tooltip instead of an empty popup, matching the old `title={x ??
// undefined}` behavior.
export function SimpleTooltip({
  content,
  children,
  side = "top",
  delay = 300,
}: {
  content: React.ReactNode
  children: React.ReactElement
  side?: React.ComponentProps<typeof TooltipContent>["side"]
  delay?: number
}) {
  if (content === null || content === undefined || content === "") {
    return children
  }
  return (
    <TooltipProvider delay={delay}>
      <Tooltip>
        <TooltipTrigger render={children} />
        <TooltipContent side={side}>{content}</TooltipContent>
      </Tooltip>
    </TooltipProvider>
  )
}
