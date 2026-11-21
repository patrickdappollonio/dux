import { useState, type ReactElement } from "react"

import { SimpleTooltip } from "@/components/SimpleTooltip"
import { FOCUS_RING } from "@/lib/focusRing"
import { cn } from "@/lib/utils"

// A control's hover hint that stays readable while the control is disabled.
// A disabled button takes no pointer events and no focus, so a tooltip hung on
// it never opens; the hint hangs off a focusable wrapper around it instead,
// which wears the focus ring the button cannot. A press on the wrapper opens it
// too, because a finger has no hover. Enabled, or with nothing to say, the
// control carries the ordinary tooltip.
export function ExplainedWhenDisabled({
  disabled,
  hint,
  children,
}: {
  disabled: boolean
  hint: string | null
  children: ReactElement
}) {
  const [open, setOpen] = useState(false)
  if (!disabled || hint === null) {
    return <SimpleTooltip content={hint}>{children}</SimpleTooltip>
  }
  return (
    <SimpleTooltip content={hint} open={open} onOpenChange={setOpen}>
      <span
        tabIndex={0}
        onClick={() => setOpen(true)}
        className={cn("inline-flex shrink-0 rounded-md", FOCUS_RING)}
      >
        {children}
      </span>
    </SimpleTooltip>
  )
}
