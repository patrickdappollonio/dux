import * as React from "react"

import { usePrefersReducedMotion } from "@/hooks/use-reduced-motion"

import {
  type SettlingAnimationHandlers,
  useSettlingAnimation,
} from "@/hooks/use-settling-animation"

// The glyph's working cue, shared by every surface that bounces a working
// glyph (the sidebar list's agent and terminal rows, the collapsed rail and the
// agent tab strip) through `WorkingGlyph`: the pulse on the glyph plus four
// bounces per pulse on a frame around it, on the shared clock. The bounce is a
// user-locked decision (CLAUDE.md, "Locked by the user").
//
// The two ride separate elements on purpose. Every stop, a plain one or a
// higher state (typing, needs-you) taking over, is the same stop: the pulse
// yields at once, because it must never run inside the attention blink, and
// the bounce in flight finishes and rests exactly at translateY(0). With both
// in one `animation` list, dropping the pulse would be a restyle of the very
// element the bounce runs on; on a frame of its own, the bounce is never
// touched by it. Splitting them costs nothing in phase: both classes land in
// the same commit, so both animations start on the same style change, and in
// Chromium they were measured to share one startTime.
export const WORKING_BOUNCE_NAME = "working-bounce"
export const WORKING_PULSE_NAME = "working-pulse"
export const WORKING_BOUNCE_ANIMATION = "motion-safe:animate-working-bounce"
export const WORKING_PULSE_ANIMATION = "motion-safe:animate-working-pulse"
// How long the glyph takes to come back to full opacity once the pulse
// yields on a plain stop. Short enough to read as immediate, long enough not
// to flash.
export const WORKING_PULSE_FADE_MS = 300

function pageHidden(): boolean {
  return typeof document !== "undefined" && document.hidden
}

function findAnimation(el: Element | null, name: string): Animation | undefined {
  if (!el || typeof el.getAnimations !== "function") return undefined
  return el.getAnimations().find((a) => (a as CSSAnimation).animationName === name)
}

// Work resuming while the last bounce is still in flight restarts every pulse
// but not the bounce, so a restarted pulse is anchored to the bounce's own
// start and every pulse boundary stays a bounce boundary. A fresh start needs
// nothing: everything begins on the same frame with the same startTime.
function anchorPulse(frame: Element | null, target: Element | null) {
  const pulse = findAnimation(target, WORKING_PULSE_NAME)
  const bounce = findAnimation(frame, WORKING_BOUNCE_NAME)
  if (!pulse || !bounce || bounce.startTime === null) return
  if (pulse.startTime !== bounce.startTime) pulse.startTime = bounce.startTime
}

/**
 * Keeps another element's working pulse (the row's state word) in step with a
 * `WorkingGlyph`'s bounce when work resumes mid-settle. It belongs in the
 * component that renders both, because a parent's layout effect runs after its
 * children's refs are attached and their classes applied.
 */
export function useWorkingPulseAnchor(
  working: boolean,
  frameRef: React.RefObject<Element | null>,
  targetRef: React.RefObject<Element | null>,
) {
  React.useLayoutEffect(() => {
    if (working) anchorPulse(frameRef.current, targetRef.current)
  }, [working, frameRef, targetRef])
}

/**
 * Drives one working glyph: `bouncing` keys the bounce class on the frame that
 * `attachBounce` and `handlers` go on, and `pulsing` keys the pulse class on the
 * glyph that `attachPulse` goes on. `landing` is true from a stop until the
 * bounce has finished and the glyph has eased back to full opacity.
 *
 * `working` is the cue's one flag, already resolved through the state word's
 * ladder by the caller, so a higher state taking over is just `working` turning
 * false. The pulse follows `working` exactly; the bounce settles through
 * `useSettlingAnimation` on every stop, which is also what makes a cancelled
 * animation, a hidden tab and reduced motion stop at once.
 *
 * Removing a CSS animation starts no CSS transition from its animated value
 * (the opacity snaps straight to full in Chromium), so on every stop, a
 * hand-over included, the pulse is held for one layout pass, long enough to
 * read the opacity it had reached and ease the glyph from there back to full
 * with a script animation, and then dropped before anything is painted. A
 * higher state's own look waits for `landing` to clear (see `WorkingGlyph`), so
 * the ease never runs under an attention blink. Reduced motion has no landing:
 * it never starts the ease, cancels one already running, and releases the look
 * at once.
 */
export function useWorkingCue<B extends Element, G extends Element>(
  working: boolean,
  { bounce: bounces = true }: { bounce?: boolean } = {},
): {
  bouncing: boolean
  pulsing: boolean
  landing: boolean
  attachBounce: (el: B | null) => void
  attachPulse: (el: G | null) => void
  handlers: SettlingAnimationHandlers
} {
  // A glyph that never bounces has no bounce to finish: its landing is the
  // opacity ease alone.
  const bounce = useSettlingAnimation<B>(working && bounces, WORKING_BOUNCE_NAME)
  const [frame, setFrame] = React.useState<B | null>(null)
  const [glyph, setGlyph] = React.useState<G | null>(null)
  const fade = React.useRef<Animation | null>(null)
  const [fading, setFading] = React.useState(false)
  const reduced = usePrefersReducedMotion()

  const [prevWorking, setPrevWorking] = React.useState(working)
  const [leaving, setLeaving] = React.useState(false)
  // A state update made during render lands on the NEXT render, so this render
  // reads the value it is about to store.
  let leavingNow = leaving
  if (working !== prevWorking) {
    setPrevWorking(working)
    leavingNow = !working
    setLeaving(leavingNow)
  }

  // The pulse's last layout pass: read where it is, start the ease back from
  // there, and let it go. Layout effects run before paint, and so does the
  // re-render the state update here causes, so no frame is ever painted with
  // the pulse still running after a stop.
  React.useLayoutEffect(() => {
    if (!leaving) return
    const pulse = findAnimation(glyph, WORKING_PULSE_NAME)
    if (
      glyph &&
      pulse &&
      !reduced &&
      !pageHidden() &&
      typeof glyph.animate === "function"
    ) {
      const from = window.getComputedStyle(glyph).opacity
      const animation = glyph.animate([{ opacity: from }, { opacity: "1" }], {
        duration: WORKING_PULSE_FADE_MS,
        easing: "ease-out",
      })
      fade.current = animation
      // A cancel rejects the promise, and a cancelled ease is just as done.
      const done = () => {
        if (fade.current !== animation) return
        fade.current = null
        setFading(false)
      }
      animation.finished.then(done, done)
    }
    // Deliberate: the pulse's value can only be read while its class is still
    // on, and the class has to leave before the first paint, so the updates
    // belong in this layout effect rather than after it.
    setFading(fade.current !== null)
    setLeaving(false)
  }, [leaving, glyph, reduced])

  // Resuming ends an ease still running, and so do reduced motion and the tab
  // going hidden, where there is no motion to finish.
  React.useLayoutEffect(() => {
    if (!working && !reduced) return
    fade.current?.cancel()
    fade.current = null
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setFading(false)
  }, [working, reduced])
  React.useEffect(() => {
    if (!fading) return
    const onVisibility = () => {
      if (pageHidden()) fade.current?.cancel()
    }
    document.addEventListener("visibilitychange", onVisibility)
    return () => document.removeEventListener("visibilitychange", onVisibility)
  }, [fading])

  React.useLayoutEffect(() => {
    if (working) anchorPulse(frame, glyph)
  }, [working, glyph, frame])

  const { attach } = bounce
  const attachBounce = React.useCallback(
    (el: B | null) => {
      attach(el)
      setFrame(el)
    },
    [attach],
  )

  return {
    bouncing: bounce.running,
    pulsing: working || leavingNow,
    // True from the stop's own render, before any effect or animation event
    // has run, so a look waiting on it can never slip in ahead of the ease.
    landing: !working && !reduced && (bounce.running || fading || leavingNow),
    attachBounce,
    attachPulse: setGlyph,
    handlers: bounce.handlers,
  }
}

// When each keyed cue's animations began their active time, by key and
// animation name, so a glyph remounted elsewhere picks the cue up where its
// other instance had it rather than restarting it. A clock belongs to one
// episode of the cue: it lives only while a glyph with its key is mounted (and
// through the commit that swaps one instance for another), so the map holds no
// more keys than there are glyphs on screen, and a cue that starts again later
// starts afresh.
const cueClocks = new Map<string, number>()
const cueHolders = new Map<string, number>()

function forgetKey(key: string): void {
  for (const slot of [...cueClocks.keys()]) {
    if (slot.startsWith(`${key}|`)) cueClocks.delete(slot)
  }
}

/**
 * Registers a mounted glyph showing `key`'s cue, and returns its release. When
 * the last one goes, the key's clocks are forgotten once the commit is over,
 * so an instance mounted in the same commit (the flap swapped for the pill)
 * still finds them.
 */
export function holdCueClock(key: string): () => void {
  cueHolders.set(key, (cueHolders.get(key) ?? 0) + 1)
  return () => {
    const left = (cueHolders.get(key) ?? 1) - 1
    if (left > 0) {
      cueHolders.set(key, left)
      return
    }
    cueHolders.delete(key)
    queueMicrotask(() => {
      if (!cueHolders.has(key)) forgetKey(key)
    })
  }
}

/**
 * Carries a keyed cue's clock across remounts, the way the tab strips keep
 * their pills' clocks across a reorder. A cue that starts under a mounted glyph
 * starts as the list's does, at once, and its clock is recorded; a glyph that
 * mounts with the cue already running in another instance (the phone's flap
 * flying into the floating pill) takes the recorded clock, so the two show the
 * same frame at the hand-over. Times are kept as the start of the active
 * phase, because the look a glyph lands into carries a delay and the same look
 * mounted afresh does not. Only CSS animations are carried: a colour
 * transition has no clock to keep.
 */
export function carryCueClock(el: Element | null, key: string, carry: boolean): void {
  if (!el || typeof el.getAnimations !== "function") return
  for (const animation of el.getAnimations()) {
    const name = (animation as CSSAnimation).animationName
    if (typeof name !== "string" || name === WORKING_BOUNCE_NAME) continue
    const slot = `${key}|${name}`
    const delay = Number(animation.effect?.getComputedTiming().delay ?? 0)
    const recorded = cueClocks.get(slot)
    if (carry && recorded !== undefined) {
      const start = recorded - delay
      if (animation.startTime !== start) animation.startTime = start
    }
    const record = () => {
      if (animation.startTime !== null && cueHolders.has(key)) {
        cueClocks.set(slot, Number(animation.startTime) + delay)
      }
    }
    if (animation.startTime !== null) record()
    else animation.ready?.then(record, () => {})
  }
}

/** Forgets a keyed cue's clocks once that cue has stopped or changed, so a
 * later start is new: the one named, or with no name every one but the
 * pulse's (the look's, which change together). */
export function forgetCueClock(key: string, name?: string): void {
  if (name !== undefined) {
    cueClocks.delete(`${key}|${name}`)
    return
  }
  for (const slot of [...cueClocks.keys()]) {
    if (slot.startsWith(`${key}|`) && slot !== `${key}|${WORKING_PULSE_NAME}`) cueClocks.delete(slot)
  }
}
