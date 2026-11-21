import { useState, type ReactElement } from "react"

import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip"
import { FOCUS_RING } from "@/lib/focusRing"
import { cn } from "@/lib/utils"

// A control's hint that stays readable while it is disabled: a disabled button
// takes no pointer events or focus, so the hint hangs off a wrapper that takes
// the focus, the ring and, for a finger, a tap. The wrapper is there either way,
// so flipping `disabled` never remounts the control or drops its focus.
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
  // A hint belongs to the state it was opened for: once the control changes
  // state, nobody asked about the new one, so the tooltip closes.
  const [tappedWhile, setTappedWhile] = useState(disabled)
  const stateChanged = tappedWhile !== disabled
  if (stateChanged) {
    setTappedWhile(disabled)
    setTapped(false)
    setHovered(false)
  }
  const open = !stateChanged && (hovered || tapped)
  // The text stops following the hint once the tooltip closes, so its fade
  // still says what it said while open.
  const [shown, setShown] = useState(hint)
  if (open && shown !== hint) setShown(hint)
  return (
    <TooltipProvider delay={300}>
      <Tooltip
        disabled={hint === null}
        open={open}
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
        <TooltipContent side="top">{open ? hint : shown}</TooltipContent>
      </Tooltip>
    </TooltipProvider>
  )
}
