// THE PHONE'S THEATER CHOREOGRAPHY, as a state machine and some arithmetic.
//
// Entering theater on a phone does not swap one control for another. The docked
// flap TEARS OFF the band and flies to the floating pill's dock as one object,
// and leaving theater flies it home and re-attaches it. That reads as one
// control moving rather than two controls appearing and disappearing, which is
// the whole point: the four buttons under the user's thumb are the same four
// buttons in both modes, and an animation that says so is cheaper to learn than
// a legend.
//
// Only the TOP chrome collapses. On a coarse pointer the compose bar and the
// terminal-key row are the typing surface, not decoration, and taking them away
// on the way into a mode about looking at the terminal would take away the way
// to answer it. The desktop's theater is unaffected and keeps its own shape.
//
// Everything here is a rule or a number rather than a rendering, which is why
// it is here: the phases and their clocks, the FLIP translation, and the one
// piece of colour arithmetic the flight needs (a shadow with its alpha taken
// out, so the real one can fade IN across the travel instead of popping).

/**
 * Where a flight is.
 *
 * `docked` and `floating` are the two RESTING states, one per mode; the other
 * five are the stages between them. A phase is what both surfaces are rendered
 * from, so "which of these exists right now" has exactly one answer.
 */
export type FlightPhase =
  /// Theater off, settled: the flap is on the band and there is no pill.
  | "docked"
  /// Theater just turned on: the top chrome is collapsing and the flap rides
  /// up with it, still visible, still where the user last saw it.
  | "collapsing"
  /// The cluster is in the air, on its way to the pill's dock.
  | "detaching"
  /// Theater on, settled: the pill floats and the flap is gone.
  | "floating"
  /// Theater just turned off: the top chrome is expanding, so the dock the
  /// pill is about to fly to is a real place again by the time it leaves.
  | "expanding"
  /// The capsule is in the air, on its way home.
  | "returning"
  /// Arrived: the capsule squares into the tab shape against the band before
  /// the real flap takes over, pixel for pixel.
  | "attaching"

/// The travel itself. Long enough to read as one object moving across the
/// screen, short enough that a control the user reached for is not withheld.
export const FLIGHT_TRAVEL_MS = 320

/// How long the travel stage is HELD, which is the travel plus a frame of
/// slack so the transition has landed before the next stage starts writing.
export const FLIGHT_TRAVEL_HOLD_MS = 340

/// The arrival snap: corners squaring, background becoming the band's, the top
/// hairline fading out, the fillets growing.
export const FLIGHT_ATTACH_MS = 200

/// The snap plus its own frame of slack.
export const FLIGHT_ATTACH_HOLD_MS = 220

/// A frame of slack past the CHROME's own clock, for the same reason the travel
/// holds one past the travel. The timer is armed in the commit that flips the
/// mode, and the chrome's transition does not start until the next paint, so a
/// stage handed over exactly on the chrome's duration measures a dock that is
/// still short of where it settles. That measurement IS the flight's start
/// point, so the difference lands in the translation.
export const FLIGHT_CHROME_SLACK_MS = 20

/// The corner and colour morph at pull-off, which finishes inside the flight's
/// first stretch so the rest of the travel is already a finished capsule.
export const FLIGHT_SHAPE_MS = 200

/// The one easing the whole gesture is on, the same curve the chrome collapse
/// uses, so nothing in the choreography moves on a different clock.
export const FLIGHT_EASE = "cubic-bezier(0.2, 0, 0, 1)"

/// The flap's bottom corner radius, which is the radius the capsule squares
/// INTO on arrival and out of at pull-off.
export const FLIGHT_TAB_RADIUS_PX = 10

/**
 * What a change of mode starts.
 *
 * A viewer who asked for less motion is handed the resting state outright:
 * every stage of this is a transition, and a transition nobody sees is a delay
 * before a control they asked for appears.
 */
export function flightForMode(theater: boolean, chromeMs: number): FlightPhase {
  if (chromeMs <= 0) return theater ? "floating" : "docked"
  return theater ? "collapsing" : "expanding"
}

/**
 * The same question, asked by a machine that is already in the middle of one.
 *
 * A MODE FLIPPED BACK DURING ITS OWN CHROME STAGE UNDOES THAT STAGE rather than
 * starting the opposite gesture. The chrome stages are the two where nothing has
 * moved yet: a collapse that is abandoned before the cluster ever left the band
 * has a flap still on the band to go back to, and running the leaving gesture
 * from there hides the flap and floats a pill over a screen the cluster never
 * flew off. So an abandoned stage rests at the state it started from, with no
 * flight at all, and the chrome animates back on its own flag.
 *
 * Every other interruption is a real gesture in the opposite direction, because
 * by then the cluster really is somewhere else.
 */
export function flightForModeFrom(
  previous: FlightPhase,
  theater: boolean,
  chromeMs: number,
): FlightPhase {
  const abandoned = theater ? "expanding" : "collapsing"
  if (previous === abandoned) return theater ? "floating" : "docked"
  return flightForMode(theater, chromeMs)
}

/**
 * How long a stage lasts, or `null` for a resting state.
 *
 * The two chrome stages are the CHROME's clock rather than one of their own:
 * the flap may not leave the band until the band has finished leaving, and the
 * pill may not fly home until the dock it is aiming at is back on screen. Plus
 * the frame of slack the transition starts a paint late by, so the stage that
 * measures the dock reads it settled rather than nearly there.
 */
export function flightHoldMs(
  phase: FlightPhase,
  chromeMs: number,
): number | null {
  switch (phase) {
    case "collapsing":
    case "expanding":
      return chromeMs + FLIGHT_CHROME_SLACK_MS
    case "detaching":
    case "returning":
      return FLIGHT_TRAVEL_HOLD_MS
    case "attaching":
      return FLIGHT_ATTACH_HOLD_MS
    case "docked":
    case "floating":
      return null
  }
}

/** The stage after this one. A resting state stays put. */
export function flightNext(phase: FlightPhase): FlightPhase {
  switch (phase) {
    case "collapsing":
      return "detaching"
    case "detaching":
      return "floating"
    case "expanding":
      return "returning"
    case "returning":
      return "attaching"
    case "attaching":
      return "docked"
    case "docked":
    case "floating":
      return phase
  }
}

/**
 * Is the flap in the DOM?
 *
 * Everywhere but the one phase where theater is settled. It is mounted through
 * the whole return flight (hidden) because it IS the dock: the choreography
 * measures the real element rather than reconstructing where it would have
 * been, so the capsule lands on the flap's own pixels and the final swap moves
 * nothing.
 */
export function flapMounted(phase: FlightPhase): boolean {
  return phase !== "floating"
}

/** Is the flap actually painted, or only there to be measured? */
export function flapVisible(phase: FlightPhase): boolean {
  return phase === "docked" || phase === "collapsing"
}

/** Is the pill in the DOM? */
export function pillMounted(phase: FlightPhase): boolean {
  return phase !== "docked" && phase !== "collapsing"
}

/**
 * Is the flight, rather than the pill's own drag state, saying where the pill
 * sits?
 *
 * Only on the way home. The detach flies FROM the flap TO the dock the pill
 * already knows about, so the pill keeps its own coordinates and the flight
 * adds a transform; the return has to pin the box at the coordinates it is
 * leaving and then park it on the flap's, neither of which is a position the
 * pill's drag state has any business holding.
 */
export function flightOwnsPosition(phase: FlightPhase): boolean {
  return phase === "returning" || phase === "attaching"
}

/** A rectangle, reduced to the two numbers a translation needs. */
export interface FlightPoint {
  left: number
  top: number
}

/**
 * The FLIP translation from one painted box to another.
 *
 * A PURE translation, deliberately: the pill starts the detach gripless, at
 * which width its box is the flap's box exactly, so there is no scale to apply
 * and applying one anyway would smear a 1px border and a row of glyphs for the
 * length of the flight.
 */
export function flightTranslation(
  from: FlightPoint,
  to: FlightPoint,
): { x: number; y: number } {
  return { x: from.left - to.left, y: from.top - to.top }
}

/** Where a viewport rectangle sits inside its offset parent. */
export function flightOffset(
  rect: FlightPoint,
  parent: FlightPoint,
): { left: number; top: number } {
  return { left: rect.left - parent.left, top: rect.top - parent.top }
}

// Every colour a computed box-shadow can be written in. Browsers normalize
// shadow colours to a function, so a hex or a keyword never reaches this.
const SHADOW_COLOR =
  /\b(?:rgba?|hsla?|hwb|oklch|oklab|lab|lch|color)\([^()]*\)/gi

/**
 * The same shadow with every colour taken down to fully transparent.
 *
 * The flap has NO shadow and the floating pill has one, so the shadow has to
 * fade in across the detach and back out across the return. Fading needs
 * something to interpolate FROM, and `none` is not that: a shadow going to or
 * from `none` snaps. Keeping the structure and zeroing the alpha is what makes
 * it a fade, and it is why the capsule arrives at the dock already shadowless,
 * where a swap to the shadowless flap wipes nothing visible.
 *
 * `null` when there is nothing to fade (no shadow, or a value with no colour in
 * it, which is what a test environment with no stylesheet reports). The caller
 * skips that half of the animation rather than guessing at a value.
 */
export function transparentShadow(shadow: string | null | undefined): string | null {
  if (!shadow) return null
  const trimmed = shadow.trim()
  if (trimmed === "" || trimmed === "none") return null
  if (!new RegExp(SHADOW_COLOR.source, "i").test(trimmed)) return null
  return trimmed.replace(SHADOW_COLOR, "rgba(0, 0, 0, 0)")
}

// THE DOCKED FLAP'S ELEMENT, published for the one thing that has to measure it.
//
// A module-level registration, the same idiom as `layoutGesture.ts` and
// `terminalFocus.ts`, and for the same reason: the flap is a sibling of the
// pane and the pill is inside it, so a prop chain joining them would have to
// cross the whole terminal component. What travels is a measurement, not
// control: the pill asks where the flap is and nothing else.
let flapElement: HTMLElement | null = null

/** Publish the mounted flap. Returns the unregister. */
export function registerFlapElement(el: HTMLElement | null): () => void {
  flapElement = el
  return () => {
    // Only retire our OWN registration: a successor flap may already have
    // replaced it, and React does not order an old cleanup before a new effect.
    if (flapElement === el) flapElement = null
  }
}

/** The flap's painted box, or `null` when no flap is mounted. */
export function peekFlapRect(): DOMRect | null {
  return flapElement?.getBoundingClientRect() ?? null
}

/// The custom property the flap's body colour is published on, and the pill
/// wears for the length of a flight.
export const FLAP_FILL_VAR = "--dux-flap-fill"

/// The colour the pill has to be at the dock end of a flight.
///
/// THE FLAP IS NOT ONE COLOUR. It takes the tone of whatever band it hangs
/// from: the tab strip's composited one, or the plain app background for a
/// single-tab agent or a hidden top bar, which is the common case rather than
/// the exotic one. A flight that assumed the strip's colour popped in the wrong
/// tone for a frame at both ends of the journey. The flap publishes its own
/// answer on the element the flight already measures, so the two cannot
/// disagree; the strip's tone is the fallback for a flight with no flap to ask.
/// THE LAST COLOUR A FLAP PUBLISHED, remembered past its unmount.
///
/// The flap is unmounted for the whole of the floating stage, and the pill's
/// SETTLED background is that same colour: without this the pill would fly out
/// of a plain-band flap and then, one commit later, repaint itself in the
/// strip's tone the moment the resting stage cleared the flight's writes. It is
/// page-global, like the registration it shadows, which is honest for a phone
/// showing one pane at a time.
let lastFlapFill = ""

export function peekFlapFill(): string {
  const published = flapElement?.style.getPropertyValue(FLAP_FILL_VAR).trim()
  if (published) lastFlapFill = published
  return lastFlapFill || "var(--dux-flap-bg)"
}
