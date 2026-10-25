import { Plus } from "lucide-react"
import { useEffect, useState, type FocusEvent } from "react"

import { Button } from "@/components/ui/button"
import { usePrefersReducedMotion } from "@/hooks/use-reduced-motion"
import { LAUNCHER_LABELS, launcherLabelAt } from "@/lib/launcherLabel"
import { openNewLauncher } from "@/lib/store"
import { cn } from "@/lib/utils"

// The launcher corner on every surface: the desktop sidebar's footer and the
// mobile hub's bottom bar render this one component. One filled button that
// opens the New dialog, where every way to create something lives.
//
// Its label rests on "New…" and now and then names, one word at a time, what
// is behind it. Hover or keyboard focus holds it on "New…", reduced motion never
// cycles it, and a screen reader always hears "New…" (the words are hidden).
export function LauncherCorner({ className }: { className?: string }) {
  const [hovered, setHovered] = useState(false)
  const [focused, setFocused] = useState(false)
  const { index, previous } = useLauncherLabel(hovered || focused)
  return (
    <div className={cn("flex items-center", className)}>
      {/* size="sm" is the height token, with max-md:min-h-11 lifting it over
        * the touch floor. The desktop height is the per-axis exemption: the
        * corner has no other control beside it. */}
      <Button
        size="sm"
        aria-label={LAUNCHER_LABELS[0]}
        className="min-w-0 flex-1 max-md:min-h-11"
        onClick={openNewLauncher}
        onPointerEnter={() => setHovered(true)}
        onPointerLeave={() => setHovered(false)}
        onFocus={(event) => setFocused(isKeyboardFocus(event))}
        onBlur={() => setFocused(false)}
      >
        <Plus />
        {/* Only the shown word takes up room, so the plus and that word stay
          * centred together and re-centre when it changes. The others lie over
          * it out of flow, centred on it, waiting or leaving. */}
        <span aria-hidden="true" className="relative min-w-0 overflow-hidden">
          {LAUNCHER_LABELS.map((label, i) => (
            <span
              key={label}
              data-shown={i === index ? "" : undefined}
              className={cn(
                "flex justify-center whitespace-nowrap transition-[transform,opacity] duration-[125ms] ease-out motion-reduce:transition-none",
                i === index
                  ? "translate-y-0 opacity-100"
                  : i === previous
                    ? "absolute inset-x-0 top-0 -translate-y-full opacity-0"
                    : "absolute inset-x-0 top-0 translate-y-full opacity-0",
              )}
            >
              {label}
            </span>
          ))}
        </span>
      </Button>
    </div>
  )
}

// Keyboard focus holds the label still, the way hover does. A click focuses the
// button too, but that is not somebody reading it; where the browser cannot
// answer the question, any focus counts.
function isKeyboardFocus(event: FocusEvent<HTMLElement>): boolean {
  try {
    return event.currentTarget.matches(":focus-visible")
  } catch {
    return true
  }
}

// Which word shows, and which one just left (it slides up and out while the
// new one slides in from below). Wall-clock timed, so the words keep their
// pace whatever else the page is doing; a hidden tab stops the cycle and a
// returning one starts it again from the rest, as does the end of a hold.
function useLauncherLabel(held: boolean): { index: number; previous: number } {
  const reduced = usePrefersReducedMotion()
  const still = held || reduced
  const [shown, setShown] = useState({ index: 0, previous: 0 })
  useEffect(() => {
    if (still) return
    let timer: number | undefined
    let startedAt = 0
    const show = (index: number) =>
      setShown((current) =>
        current.index === index ? current : { index, previous: current.index },
      )
    const tick = () => {
      const { index, nextInMs } = launcherLabelAt(Date.now() - startedAt)
      show(index)
      timer = window.setTimeout(tick, nextInMs)
    }
    const begin = () => {
      startedAt = Date.now()
      timer = window.setTimeout(tick, launcherLabelAt(0).nextInMs)
    }
    const stop = () => {
      window.clearTimeout(timer)
      timer = undefined
      show(0)
    }
    const onVisibility = () => {
      stop()
      if (!document.hidden) begin()
    }
    if (!document.hidden) begin()
    document.addEventListener("visibilitychange", onVisibility)
    return () => {
      document.removeEventListener("visibilitychange", onVisibility)
      stop()
    }
  }, [still])
  // Held, the word on screen leaves the way any word does, upward, before the
  // stop above has caught up with it.
  if (still && shown.index !== 0) return { index: 0, previous: shown.index }
  return still ? { index: 0, previous: shown.previous } : shown
}
