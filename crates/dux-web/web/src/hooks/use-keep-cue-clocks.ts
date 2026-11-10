import { useCallback, useLayoutEffect, useRef } from "react"

import {
  WORKING_BOUNCE_NAME,
  WORKING_PULSE_NAME,
} from "@/hooks/use-working-cue"

// How long after a reorder the moved pills' clocks are corrected: Chromium
// replaces the animations a moved element gets more than once over its first
// frames, and a start time set on a replaced one is lost. Frames as well as
// time, so a page that is not painting steadily still gets its corrections.
const CORRECT_MS = 120
const CORRECT_FRAMES = 8

function cueAnimations(el: Element): CSSAnimation[] {
  if (typeof el.getAnimations !== "function") return []
  return el
    .getAnimations({ subtree: true })
    .filter(
      (a): a is CSSAnimation =>
        "animationName" in a &&
        ((a as CSSAnimation).animationName === WORKING_BOUNCE_NAME ||
          (a as CSSAnimation).animationName === WORKING_PULSE_NAME),
    )
}

/**
 * Keeps a working pill on the shared working-cue clock when its strip is
 * reordered. Moving an element in the DOM ends and restarts its CSS animations
 * (measured in Chromium: the moved pill's bounce and pulse start over while its
 * unmoved neighbours run on), which would put the bounce, a user-locked cue, off
 * the shared clock. So the start time each pill's cue was on is remembered at
 * every commit, and when the order changes it is handed back to the pill that
 * moved, the way the collapsed sidebar hands the clocks across its own swap.
 *
 * Only while the pill is still in the working cycle the clock was read from: a
 * bounce that has begun settling is sized to end against its own start, so
 * moving that would end it at once, and a cue that went away and came back is a
 * new cycle on a clock of its own. Either way the pill is let go.
 *
 * `orderKey` changes when the pills' order does. Put the returned ref on the
 * strip's element and `data-cue-key` on each pill.
 */
export function useKeepCueClocks(orderKey: string) {
  const root = useRef<HTMLElement | null>(null)
  const clocks = useRef<Map<string, number>>(new Map())
  const lastOrder = useRef(orderKey)
  const run = useRef(0)

  const attach = useCallback((el: HTMLElement | null) => {
    root.current = el
  }, [])

  useLayoutEffect(() => {
    const strip = root.current
    if (!strip) return
    const read = () => {
      const next = new Map<string, number>()
      for (const pill of strip.querySelectorAll<HTMLElement>("[data-cue-key]")) {
        const bounce = cueAnimations(pill).find(
          (a) => a.animationName === WORKING_BOUNCE_NAME,
        )
        if (bounce?.startTime != null && pill.dataset.cueKey) {
          next.set(pill.dataset.cueKey, Number(bounce.startTime))
        }
      }
      return next
    }
    const moved = lastOrder.current !== orderKey
    lastOrder.current = orderKey
    if (!moved) {
      clocks.current = read()
      return
    }
    const held = clocks.current
    if (held.size === 0) return
    const mine = ++run.current
    const released = new WeakSet<Element>()
    const seen = new WeakSet<Element>()
    const until = performance.now() + CORRECT_MS
    let frames = 0
    // Frames do not run while the page is hidden, so a window opened before the
    // page was hidden would resume after it and force a clock held from before
    // onto a cue that has since stopped and started again. Hiding ends it.
    const onVisibility = () => {
      if (!document.hidden) return
      document.removeEventListener("visibilitychange", onVisibility)
      if (run.current === mine) run.current += 1
    }
    document.addEventListener("visibilitychange", onVisibility)
    const correct = () => {
      if (mine !== run.current) return
      frames += 1
      for (const pill of strip.querySelectorAll<HTMLElement>("[data-cue-key]")) {
        const start = held.get(pill.dataset.cueKey ?? "")
        if (start === undefined || released.has(pill)) continue
        const animations = cueAnimations(pill)
        const bounce = animations.find((a) => a.animationName === WORKING_BOUNCE_NAME)
        if (!bounce) {
          // Its cue went away: the clock it was on belongs to a cycle that is
          // over, and a cue that comes back is a new one on a clock of its own.
          if (seen.has(pill)) released.add(pill)
          continue
        }
        seen.add(pill)
        if (bounce.effect?.getTiming().iterations !== Infinity) {
          released.add(pill)
          continue
        }
        for (const animation of animations) {
          if (animation.effect?.getTiming().iterations !== Infinity) continue
          if (animation.startTime !== start) animation.startTime = start
        }
      }
      if (frames < CORRECT_FRAMES || performance.now() < until) {
        requestAnimationFrame(correct)
      } else {
        document.removeEventListener("visibilitychange", onVisibility)
        clocks.current = read()
      }
    }
    requestAnimationFrame(correct)
  })

  return attach
}
