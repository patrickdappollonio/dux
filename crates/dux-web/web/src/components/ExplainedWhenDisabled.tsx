import { useState, type ReactElement } from "react"

import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip"
import { FOCUS_RING } from "@/lib/focusRing"
import { cn } from "@/lib/utils"

// A control's hover hint that stays readable while the control is disabled. A
// disabled button takes no pointer events and no focus, so the hint hangs off a
// wrapper around it, which takes the focus, and the ring, the button cannot.
// A tap on the wrapper opens it too, because a finger has no hover. The wrapper
// is there whether or not the control is disabled, so flipping `disabled`
// never remounts the control and the focus stays on it.
export function ExplainedWhenDisabled({
  disabled,
  hint,
  className,
  disabledClassName,
  children,
}: {
  disabled: boolean
  /** Null shows no hint at all. */
  hint: string | null
  /** The wrapper's own classes, such as a rounding that matches the control. */
  className?: string
  /** Added while disabled, such as the tap area the control's own cannot give. */
  disabledClassName?: string
  children: ReactElement
}) {
  const [hovered, setHovered] = useState(false)
  const [tapped, setTapped] = useState(false)
  // A tap's hint belongs to the state that was tapped: once the control changes
  // state, nobody asked about the new one.
  const [tappedWhile, setTappedWhile] = useState(disabled)
  if (tappedWhile !== disabled) {
    setTappedWhile(disabled)
    setTapped(false)
  }
  return (
    <TooltipProvider delay={300}>
      <Tooltip
        disabled={hint === null}
        open={hovered || tapped}
        onOpenChange={(next) => {
          setHovered(next)
          if (!next) setTapped(false)
        }}
      >
        <TooltipTrigger
          render={
            <span
              tabIndex={disabled ? 0 : undefined}
              onClick={disabled ? () => setTapped(true) : undefined}
              className={cn(
                "inline-flex shrink-0 rounded-md",
                FOCUS_RING,
                className,
                disabled && disabledClassName,
              )}
            />
          }
        >
          {children}
        </TooltipTrigger>
        <TooltipContent side="top">{hint}</TooltipContent>
      </Tooltip>
    </TooltipProvider>
  )
}
