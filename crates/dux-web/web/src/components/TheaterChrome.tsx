import { useLayoutEffect, useRef, useState, type ReactNode } from "react"

import { usePrefersReducedMotion } from "@/hooks/use-reduced-motion"
import { theaterTransitionMs } from "@/lib/theater"
import { cn } from "@/lib/utils"

// THE CHROME THAT LEAVES. Wraps a stack of dux's own chrome (the pane header,
// the pull-request band and the tab strip on the desktop shell; the phone
// shell's header and strip) and collapses it out of the layout when theater is
// on, giving its height to the terminal underneath.
//
// The collapse animates from a MEASURED explicit height, never from
// `height: auto`, which is not an interpolable value: written the obvious way
// the chrome snaps and the mode change has no motion at all. So the natural
// height is read once, written to the element, and replaced with zero on the
// next frame; on the way back the pair runs in reverse and the inline height is
// dropped at the end so the chrome is free to grow again (a PR banner
// appearing, a tab strip gaining a row).
//
// The children are UNMOUNTED once the collapse finishes, rather than left in
// the DOM at zero height: invisible chrome that keyboard focus and a screen
// reader can still reach is not hidden, it is just hard to see.
//
// It runs NO refit of its own. The single PTY refit for the whole gesture is
// the layout gesture's (see `lib/layoutGesture.ts` and `useTheaterGesture`),
// which holds the pane's coordinator across every frame of this transition and
// releases it once at the end.
export function TheaterChrome({
  hidden,
  children,
}: {
  hidden: boolean
  children: ReactNode
}) {
  const reducedMotion = usePrefersReducedMotion()
  const ms = theaterTransitionMs(reducedMotion)
  const ref = useRef<HTMLDivElement | null>(null)
  const [mounted, setMounted] = useState(!hidden)

  // Adjusted DURING render, the way `useEverReady` in TerminalPane is: showing
  // has to put the children in the DOM before the same commit measures them,
  // and a cut (reduced motion) has nothing to wait for. Doing either in an
  // effect costs a second paint at the wrong height.
  if (!hidden && !mounted) setMounted(true)
  if (hidden && mounted && ms === 0) setMounted(false)

  useLayoutEffect(() => {
    if (!hidden || ms === 0) return
    const timer = setTimeout(() => setMounted(false), ms)
    return () => clearTimeout(timer)
  }, [hidden, ms])

  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    if (!mounted) {
      el.style.height = "0px"
      return
    }
    const natural = el.scrollHeight
    if (hidden) {
      el.style.height = `${natural}px`
      // Force the browser to take the explicit height before the zero lands,
      // or the two writes coalesce into one and there is nothing to animate.
      void el.offsetHeight
      el.style.height = "0px"
      return
    }
    el.style.height = `${natural}px`
    const timer = setTimeout(() => {
      if (ref.current) ref.current.style.height = ""
    }, ms)
    return () => clearTimeout(timer)
  }, [hidden, mounted, ms])

  return (
    <div
      ref={ref}
      data-testid="theater-chrome"
      data-hidden={hidden ? "true" : "false"}
      className={cn(
        "relative z-10 shrink-0 overflow-hidden",
        // `duration-300` is written out because Tailwind scans source text and
        // a class built from a variable produces no CSS at all. It has to be
        // the same number as `THEATER_TRANSITION_MS`, which is how long the
        // gesture holds the terminal's refit for; a test pins the pair, because
        // a drift here re-grids the terminal in the middle of the transition.
        "motion-safe:transition-[height,opacity,transform] motion-safe:duration-300 motion-safe:ease-[cubic-bezier(0.2,0,0,1)]",
        hidden && "-translate-y-3.5 opacity-0",
      )}
    >
      {mounted ? children : null}
    </div>
  )
}
