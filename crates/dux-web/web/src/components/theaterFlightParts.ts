// The parts of the phone's flying cluster that animate on clocks of their own:
// the grip and the flap's fillets, plus the theater toggle's glyph swap, which
// belongs to the press rather than to the flight. The box itself (its travel,
// corners and shadow) is the choreography's inline transitions; these are short
// animations inside it.
//
// They run through the Web Animations API rather than as keyframes named by a
// class. A flight turned around in mid-air has to take each part back from the
// frame on screen, and an animation object can be reversed where it stands,
// while a keyframe animation swapped by a class change restarts from its first
// frame: a glyph half faded snaps back to full, a grip half gone reappears.

import type { FlightPhase } from "@/lib/theaterFlight"

/// The stages that animate parts. The resting ones settle them.
export type PartStage = Extract<FlightPhase, "detaching" | "returning" | "attaching">

type Part = "grip" | "fillets"

/// The animations a pill is running, by part, so a turn can find them.
export type FlightParts = Map<Part, Animation[]>

const GLYPH_OUT: Keyframe[] = [
  { opacity: 1, transform: "none" },
  { opacity: 0, transform: "rotate(90deg) scale(0.6)" },
]
const GLYPH_IN: Keyframe[] = [
  { opacity: 0, transform: "rotate(-90deg) scale(0.6)" },
  { opacity: 1, transform: "none" },
]
/// No fill: each glyph ends on the value its resting rule gives it (the hidden
/// one's rotation is invisible at zero opacity), so a finished swap hands the
/// glyphs back to the stylesheet without moving them.
const GLYPH_TIMING: KeyframeAnimationOptions = {
  duration: 180,
  easing: "ease",
}
const FILLETS_OUT: Keyframe[] = [
  { opacity: 1, transform: "none" },
  { opacity: 0, transform: "scale(0.25)" },
]
const FILLETS_IN: Keyframe[] = [
  { opacity: 0, transform: "scale(0.25)" },
  { opacity: 1, transform: "none" },
]
/// How long a fillet takes to fade out or grow in.
export const FILLET_FADE_MS = 180
const FILLET_TIMING: KeyframeAnimationOptions = {
  duration: FILLET_FADE_MS,
  easing: "ease",
  fill: "both",
}

interface PartAnimation {
  part: Part
  selector: string
  keyframes: Keyframe[]
  timing: KeyframeAnimationOptions
}

const MAX = '.dux-ic-max'
const MIN = '.dux-ic-min'
const GRIP = '[data-testid="theater-pill-grip"]'
const FILLETS = ".dux-pill-fillet"

/// What each stage animates when it starts from a resting end.
function stageParts(stage: PartStage): PartAnimation[] {
  switch (stage) {
    case "detaching":
      return [
        // The grip arrives with the slot it sits in, late in the travel.
        {
          part: "grip",
          selector: GRIP,
          keyframes: [{ opacity: 0 }, { opacity: 1 }],
          timing: { duration: 380, delay: 100, easing: "ease", fill: "both" },
        },
        { part: "fillets", selector: FILLETS, keyframes: FILLETS_OUT, timing: FILLET_TIMING },
      ]
    case "returning":
      return [
        {
          part: "grip",
          selector: GRIP,
          keyframes: [{ opacity: 1 }, { opacity: 0 }],
          timing: { duration: 200, easing: "ease", fill: "both" },
        },
      ]
    case "attaching":
      return [
        { part: "fillets", selector: FILLETS, keyframes: FILLETS_IN, timing: FILLET_TIMING },
      ]
  }
}

function start(box: HTMLElement, parts: FlightParts, anim: PartAnimation): void {
  const running: Animation[] = []
  for (const el of box.querySelectorAll(anim.selector)) {
    // No Web Animations (a test environment): that part simply does not move.
    if (typeof el.animate !== "function") continue
    running.push(el.animate(anim.keyframes, anim.timing))
  }
  if (running.length > 0) parts.set(anim.part, running)
}

/// Let every part go back to what the stylesheet says. Each animation ends on
/// the value its resting state has, so cancelling one moves nothing.
export function settleParts(parts: FlightParts): void {
  for (const running of parts.values()) running.forEach((a) => a.cancel())
  parts.clear()
}

/// A stage started from a resting end, or off the band: its parts run from
/// their first frame. The snap only grows the fillets: the return's grip stays
/// held on its last frame, so nothing about it moves when the snap starts, and
/// a snap turned back into a departure finds it to reverse.
export function startParts(
  box: HTMLElement,
  parts: FlightParts,
  stage: PartStage,
): void {
  if (stage === "attaching") {
    // Any fade has finished by now (the return outlasts it), so nothing shows.
    parts.get("fillets")?.forEach((a) => a.cancel())
    parts.delete("fillets")
  } else {
    settleParts(parts)
  }
  for (const anim of stageParts(stage)) start(box, parts, anim)
}

/// Send one animation back the way it came from the frame it is on. Not
/// `reverse()`: an animation that has not moved yet (still pending its first
/// frame, at time zero) is rewound by it to its far end, which paints the
/// other end in one frame. One already at the end the new direction heads for
/// is held there instead, because playing it would rewind it the same way.
function turnAround(anim: Animation): void {
  const end = Number(anim.effect?.getComputedTiming().endTime ?? 0)
  const rate = -anim.playbackRate
  const now = anim.currentTime === null ? null : Number(anim.currentTime)
  const at = now ?? (rate < 0 ? 0 : end)
  anim.pause()
  anim.playbackRate = rate
  anim.currentTime = at
  if ((rate < 0 && at <= 0) || (rate > 0 && at >= end)) return
  anim.play()
}

/// A flight turned around. Every part already moving heads back the way it
/// came from where it is, finished ones included, since their last frame is
/// what is on screen. A part this stage needs that nothing was running starts
/// afresh.
///
/// The fillets are the exception, because a tab shape never flies: they only
/// ever grow against the band, in the snap. A turn out of the snap shrinks
/// them back from where they are; a turn in mid-air leaves whatever fade they
/// are in to run on toward gone, and starts none.
export function turnParts(
  box: HTMLElement,
  parts: FlightParts,
  stage: PartStage,
  inAir: boolean,
): void {
  for (const [part, running] of parts) {
    if (part === "fillets" && inAir) continue
    running.forEach(turnAround)
  }
  for (const anim of stageParts(stage)) {
    if (parts.has(anim.part)) continue
    if (inAir && anim.part === "fillets") continue
    start(box, parts, anim)
  }
}

/// The theater toggle's glyph swap, on the toggle whose mode just flipped to
/// `theater`. It is the press's, not the flight's: it plays at once on the
/// cluster under the finger, which stays on screen through the chrome stage, so
/// it has finished before a travel starts and the travel has nothing to swap.
///
/// A swap still playing when the mode flips back is turned around from the
/// frame it shows rather than started over.
export function swapTheaterGlyphs(
  root: HTMLElement,
  swaps: Animation[],
  theater: boolean,
): Animation[] {
  const running = swaps.filter((a) => a.playState !== "finished" && a.playState !== "idle")
  if (running.length > 0) {
    running.forEach(turnAround)
    return running
  }
  const max = root.querySelector(MAX)
  const min = root.querySelector(MIN)
  if (!max || !min || typeof max.animate !== "function") return []
  const [leaving, arriving] = theater ? [max, min] : [min, max]
  return [
    leaving.animate(GLYPH_OUT, GLYPH_TIMING),
    arriving.animate(GLYPH_IN, GLYPH_TIMING),
  ]
}
