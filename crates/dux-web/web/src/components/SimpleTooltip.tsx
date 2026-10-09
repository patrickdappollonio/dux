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
  openOnHover = true,
}: {
  content: React.ReactNode
  children: React.ReactElement
  side?: React.ComponentProps<typeof TooltipContent>["side"]
  delay?: number
  /** Controlled mode, for a caller that must veto or force the popup (a hint
   * shown only while its text is clipped, or while an ancestor has focus). */
  open?: boolean
  onOpenChange?: (open: boolean) => void
  /** False keeps the card for keyboard focus alone, for a trigger whose mouse
   * hover already reveals something bigger. */
  openOnHover?: boolean
}) {
  if (content === null || content === undefined || content === "") {
    return children
  }
  return (
    <TooltipProvider delay={delay}>
      <Tooltip
        open={open}
        onOpenChange={(next, details) => {
          if (next && !openOnHover && details.reason === "trigger-hover") {
            details.cancel()
            return
          }
          onOpenChange?.(next)
        }}
      >
        <TooltipTrigger render={children} />
        <TooltipContent side={side}>{content}</TooltipContent>
      </Tooltip>
    </TooltipProvider>
  )
}
