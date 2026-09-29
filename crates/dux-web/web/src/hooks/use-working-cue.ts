import * as React from "react"

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
 * glyph that `attachPulse` goes on.
 *
 * `working` is the cue's one flag, already resolved through the state word's
 * ladder by the caller, so a higher state taking over is `working` turning
 * false with `handover` true. The pulse follows `working` exactly; the bounce
 * settles through `useSettlingAnimation` on every stop, which is also what
 * makes a cancelled animation, a hidden tab and reduced motion stop at once.
 *
 * Removing a CSS animation starts no CSS transition from its animated value
 * (the opacity snaps straight to full in Chromium), so on a plain stop the
 * pulse is held for one layout pass, long enough to read the opacity it had
 * reached and ease the glyph from there back to full with a script animation,
 * and then dropped before anything is painted. A hand-over snaps instead, and
 * cuts short an ease already running: the higher state's own blink is running
 * around the glyph, and an ease under it would be two opacity animations
 * multiplying, which is the very thing the pulse yields to avoid.
 */
export function useWorkingCue<B extends Element, G extends Element>(
  working: boolean,
  handover = false,
): {
  bouncing: boolean
  pulsing: boolean
  attachBounce: (el: B | null) => void
  attachPulse: (el: G | null) => void
  handlers: SettlingAnimationHandlers
} {
  const bounce = useSettlingAnimation<B>(working, WORKING_BOUNCE_NAME)
  const [frame, setFrame] = React.useState<B | null>(null)
  const [glyph, setGlyph] = React.useState<G | null>(null)
  const fade = React.useRef<Animation | null>(null)

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
      !handover &&
      !pageHidden() &&
      typeof glyph.animate === "function"
    ) {
      const from = window.getComputedStyle(glyph).opacity
      fade.current = glyph.animate([{ opacity: from }, { opacity: "1" }], {
        duration: WORKING_PULSE_FADE_MS,
        easing: "ease-out",
      })
    }
    // Deliberate: the pulse's value can only be read while its class is still
    // on, and the class has to leave before the first paint, so the update
    // belongs in this layout effect rather than after it.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    setLeaving(false)
  }, [leaving, glyph, handover])

  // Resuming, or a higher state arriving, ends an ease still running.
  React.useLayoutEffect(() => {
    if (!working && !handover) return
    fade.current?.cancel()
    fade.current = null
  }, [working, handover])

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
    attachBounce,
    attachPulse: setGlyph,
    handlers: bounce.handlers,
  }
}
