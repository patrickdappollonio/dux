import { useCallback, useLayoutEffect, useRef, useState } from "react"

import {
  WORKING_BOUNCE_NAME,
  WORKING_PULSE_NAME,
} from "@/hooks/use-working-cue"

// How long the floating sidebar takes to unfold out of the rail (and fold back
// into it), and how long its contents take to fade in over that.
const UNFOLD_MS = 180
const CONTENT_FADE_MS = 120

// "open" and "closing" both lay the sidebar out expanded; only "closed" is the
// rail.
type Phase = "closed" | "open" | "closing"

// The working cue's animations on one row: the glyph's bounce and pulse, the
// state word's pulse and its ellipsis all start together, so they share a
// start time.
const CUE_ANIMATIONS = new Set([
  WORKING_BOUNCE_NAME,
  WORKING_PULSE_NAME,
  "working-dots",
])

function cueAnimations(row: Element): CSSAnimation[] {
  if (typeof row.getAnimations !== "function") return []
  return row
    .getAnimations({ subtree: true })
    .filter(
      (a): a is CSSAnimation =>
        "animationName" in a &&
        CUE_ANIMATIONS.has((a as CSSAnimation).animationName),
    )
}

// The rail and the expanded list are the same rows laid out twice, and the
// one not on screen is `display: none`, which ends its animations. Swapping
// between them would restart every working cue from its first frame, so the
// clock each row's cue is on is read off the rows on screen before the swap
// and handed to the same rows in the other layout after it.
function readCueClocks(root: Element | null): Map<string, number> {
  const clocks = new Map<string, number>()
  if (!root) return clocks
  for (const row of root.querySelectorAll<HTMLElement>("[data-sidebar-row]")) {
    const key = row.dataset.sidebarRow
    if (!key) continue
    const bounce = cueAnimations(row).find(
      (a) => a.animationName === WORKING_BOUNCE_NAME,
    )
    if (bounce?.startTime != null) clocks.set(key, Number(bounce.startTime))
  }
  return clocks
}

// Chromium replaces the animations a row gets once it is shown, more than once
// over its first frames, and a start time set on a replaced one is lost. So the
// clocks are handed over on every frame of the contents' fade-in, which starts
// at opacity 0 and so hides the first frame or two of any restarted cue.
//
// Only while the row is still in the working cycle the clock was read from: a
// bounce that has begun settling is sized to end against its own start time,
// so moving that would end it at once, and a cue that went away and came back
// is a new cycle on a clock of its own. Either way the row is let go for good.
function applyCueClocks(root: Element | null, clocks: Map<string, number>) {
  if (!root || clocks.size === 0) return
  const pending = new Map(clocks)
  const seen = new Set<string>()
  // Frames as well as time: a page that is not painting steadily (a busy main
  // thread) can spend the whole fade in one frame.
  const until = performance.now() + CONTENT_FADE_MS
  let frames = 0
  const apply = () => {
    frames += 1
    for (const row of root.querySelectorAll<HTMLElement>("[data-sidebar-row]")) {
      const key = row.dataset.sidebarRow ?? ""
      const start = pending.get(key)
      if (start === undefined) continue
      const animations = cueAnimations(row)
      const bounce = animations.find(
        (a) => a.animationName === WORKING_BOUNCE_NAME,
      )
      if (!bounce) {
        if (seen.has(key)) pending.delete(key)
        continue
      }
      seen.add(key)
      if (bounce.effect?.getTiming().iterations !== Infinity) {
        pending.delete(key)
        continue
      }
      for (const animation of animations) {
        if (animation.effect?.getTiming().iterations !== Infinity) continue
        if (animation.startTime !== start) animation.startTime = start
      }
    }
    if (pending.size === 0) return
    if (frames < 8 || performance.now() < until) requestAnimationFrame(apply)
  }
  requestAnimationFrame(apply)
}

function reducedMotion(): boolean {
  return (
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  )
}

/**
 * The collapsed rail's floating sidebar, unfolding out of the rail rather than
 * appearing whole: the expanded sidebar is revealed from the rail's own edge
 * out to its full width while its contents fade in, and folds back the same
 * way before the rail returns. Clip and opacity only, so nothing is laid out
 * twice and the page underneath never moves. Reduced motion gets a plain fade.
 *
 * `enabled` says there is a rail to float from; `wantOpen` is the float's own
 * open state. `floating` is what lays the sidebar out expanded, and it outlasts
 * `wantOpen` by the length of the fold. `attachContainer` and `attachInner` go
 * on the fixed container and the element holding the sidebar's sections.
 */
export function useSidebarUnfold(enabled: boolean, wantOpen: boolean) {
  const [phase, setPhase] = useState<Phase>("closed")
  if (!enabled && phase !== "closed") setPhase("closed")

  const container = useRef<HTMLElement | null>(null)
  const inner = useRef<HTMLElement | null>(null)
  const animations = useRef<Animation[]>([])
  const clocks = useRef<Map<string, number> | null>(null)
  const prevPhase = useRef<Phase>(phase)
  // Whether what is running is the reduced-motion fade rather than the fold.
  const fadedIn = useRef(false)

  const cancelAll = useCallback(() => {
    for (const animation of animations.current) animation.cancel()
    animations.current = []
  }, [])

  // Drives the phase from the wanted state. Leaving "closed" swaps the layout,
  // so the clocks are read here, while the rail is still what is rendered.
  useLayoutEffect(() => {
    if (!enabled) return
    // The phase is what the layout follows, so it has to move in this layout
    // pass for the swap and its animations to land before the next paint.
    /* eslint-disable react-hooks/set-state-in-effect */
    if (wantOpen && phase === "closed") {
      clocks.current = readCueClocks(container.current)
      setPhase("open")
    } else if (wantOpen && phase === "closing") {
      // Reopened mid-fold: the same animations run forward again from where
      // they are, so nothing jumps.
      for (const animation of animations.current) {
        if (animation.playbackRate < 0) animation.reverse()
      }
      setPhase("open")
    } else if (!wantOpen && phase === "open") {
      let master = animations.current[0]
      if (!master || typeof master.reverse !== "function") {
        clocks.current = readCueClocks(container.current)
        setPhase("closed")
        return
      }
      const el = container.current
      if (el && reducedMotion() && !fadedIn.current) {
        // Reduced motion came on while it was open: no fold, just the fade.
        cancelAll()
        const fade = el.animate([{ opacity: 0 }, { opacity: 1 }], {
          duration: CONTENT_FADE_MS,
          easing: "ease-out",
          fill: "both",
        })
        animations.current = [fade]
        fadedIn.current = true
        master = fade
      }
      for (const animation of animations.current) {
        if (animation.playbackRate > 0) animation.reverse()
      }
      master.onfinish = () => {
        if (master.playbackRate > 0) return
        clocks.current = readCueClocks(container.current)
        setPhase("closed")
      }
      setPhase("closing")
    }
    /* eslint-enable react-hooks/set-state-in-effect */
  }, [enabled, wantOpen, phase, cancelAll])

  // Runs on the commit that swapped the layout, before it is painted.
  useLayoutEffect(() => {
    const from = prevPhase.current
    prevPhase.current = phase
    const el = container.current
    const sections = inner.current
    if (phase === from || !el) return
    if (phase === "open" && from === "closed") {
      cancelAll()
      if (clocks.current) applyCueClocks(el, clocks.current)
      clocks.current = null
      if (typeof el.animate !== "function") return
      const timing = {
        duration: UNFOLD_MS,
        easing: "ease-out",
        fill: "both" as const,
      }
      fadedIn.current = reducedMotion()
      if (fadedIn.current) {
        animations.current = [
          el.animate([{ opacity: 0 }, { opacity: 1 }], {
            ...timing,
            duration: CONTENT_FADE_MS,
          }),
        ]
        return
      }
      // From the rail's width out past the full one, far enough to take the
      // sidebar's shadow along once the edge has passed.
      const beyondRail = el.offsetWidth - railWidth(el)
      animations.current = [
        el.animate(
          [
            { clipPath: `inset(0 ${beyondRail}px 0 0)` },
            { clipPath: "inset(0 -100px -100px 0)" },
          ],
          timing,
        ),
        ...Array.from(sections?.children ?? []).map((child) =>
          child.animate([{ opacity: 0 }, { opacity: 1 }], {
            ...timing,
            duration: CONTENT_FADE_MS,
          }),
        ),
      ]
    } else if (phase === "closed") {
      cancelAll()
      // The rail's width comes back at once: the fold already showed it
      // narrowing, so the container's own width tween must not run again.
      const transition = el.style.transition
      el.style.transition = "none"
      void el.offsetWidth
      el.style.transition = transition
      if (clocks.current) applyCueClocks(el, clocks.current)
      clocks.current = null
      if (from !== "closing" || typeof el.animate !== "function") return
      animations.current = Array.from(sections?.children ?? []).map((child) =>
        child.animate([{ opacity: 0 }, { opacity: 1 }], {
          duration: CONTENT_FADE_MS,
          easing: "ease-out",
        }),
      )
    }
  }, [phase, cancelAll])

  useLayoutEffect(() => cancelAll, [cancelAll])

  return {
    floating: enabled && phase !== "closed",
    attachContainer: useCallback((el: HTMLElement | null) => {
      container.current = el
    }, []),
    attachInner: useCallback((el: HTMLElement | null) => {
      inner.current = el
    }, []),
  }
}

// The rail's width is the gap's: the gap keeps the collapsed width while the
// container floats.
function railWidth(container: HTMLElement): number {
  const gap = container.parentElement?.querySelector<HTMLElement>(
    '[data-slot="sidebar-gap"]',
  )
  return gap?.offsetWidth ?? 48
}
