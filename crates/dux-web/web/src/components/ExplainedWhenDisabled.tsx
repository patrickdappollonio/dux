import { useLayoutEffect, useRef, useState, type ReactElement } from "react"

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
// so flipping `disabled` never remounts the control.
//
// A browser takes focus off a button the moment it is disabled and leaves it
// on the page body, so focus held anywhere in here moves to the wrapper when
// the control is disabled, and back to the control when it is enabled again.
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

  const wrapperRef = useRef<HTMLSpanElement | null>(null)
  // Whether focus is in here, kept through the blur the browser gives a button
  // as it is disabled, which no person asked for.
  const focusInside = useRef(false)
  // Set while focus is carried across a change of state, which asks nothing,
  // so it opens no hint.
  const carrying = useRef(false)
  useLayoutEffect(() => {
    const wrapper = wrapperRef.current
    if (wrapper === null || !focusInside.current) return
    const active = document.activeElement
    if (active !== document.body && !wrapper.contains(active)) return
    const target = disabled ? wrapper : wrapper.firstElementChild
    if (!(target instanceof HTMLElement) || active === target) return
    carrying.current = true
    target.focus({ preventScroll: true })
    carrying.current = false
  }, [disabled])
  return (
    <TooltipProvider delay={300}>
      <Tooltip
        disabled={hint === null}
        open={open}
        onOpenChange={(next) => {
          if (next && carrying.current) return
          setHovered(next)
          if (!next) setTapped(false)
        }}
      >
        <TooltipTrigger
          render={
            <span
              ref={wrapperRef}
              tabIndex={disabled ? 0 : undefined}
              onFocus={() => {
                focusInside.current = true
              }}
              onBlur={(event) => {
                const lost = event.relatedTarget === null && isDisabled(event.target)
                if (!lost && !event.currentTarget.contains(event.relatedTarget)) {
                  focusInside.current = false
                }
              }}
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

function isDisabled(el: EventTarget): boolean {
  return el instanceof HTMLButtonElement && el.disabled
}
