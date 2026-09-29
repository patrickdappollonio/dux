import type * as React from "react"

import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip"

// The single Tooltip wrapper used everywhere a browser-native `title=` hover
// hint would otherwise go, since browser tooltips render irregularly across
// platforms. `children` becomes the TooltipTrigger through base-ui's render
// prop. An empty or nullish `content` renders the trigger bare, so a hint
// derived from possibly-absent data is a no-op rather than an empty popup.
export function SimpleTooltip({
  content,
  children,
  side = "top",
  delay = 300,
  open,
  onOpenChange,
}: {
  content: React.ReactNode
  children: React.ReactElement
  side?: React.ComponentProps<typeof TooltipContent>["side"]
  delay?: number
  /** Controlled mode, for a caller that must veto or force the popup (a hint
   * shown only while its text is clipped, or while an ancestor has focus). */
  open?: boolean
  onOpenChange?: (open: boolean) => void
}) {
  if (content === null || content === undefined || content === "") {
    return children
  }
  return (
    <TooltipProvider delay={delay}>
      <Tooltip open={open} onOpenChange={onOpenChange}>
        <TooltipTrigger render={children} />
        <TooltipContent side={side}>{content}</TooltipContent>
      </Tooltip>
    </TooltipProvider>
  )
}
