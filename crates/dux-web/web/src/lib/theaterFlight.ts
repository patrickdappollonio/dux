// The phone's theater choreography, as a state machine and some arithmetic:
// the phases and their clocks, the FLIP translation, and the colour arithmetic
// the flight needs. Entering theater tears the docked flap off the band and
// flies it to the floating pill's dock as one object, so the same controls read
// as moving rather than as two sets appearing and disappearing.

/**
 * Where a flight is. `docked` and `floating` rest, one per mode; the rest are
 * stages between them. Both surfaces render from the phase, so what exists at
 * any moment has one answer.
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

/// A frame of slack past the chrome's own clock: the chrome's transition starts
/// a paint after the commit that arms the timer, so a stage handed over exactly
/// on the chrome's duration measures a dock short of where it settles, and that
/// measurement is the flight's start point.
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
 * The same question from a machine already mid-flight. A mode flipped back
 * during its own chrome stage undoes that stage and rests, with no flight: the
 * cluster has not moved yet, so the opposite gesture would hide a flap that is
 * still on the band. Any later interruption is a real gesture the other way.
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
 * How long a stage lasts, or `null` for a resting state. The chrome stages run
 * on the chrome's clock plus slack: the flap may not leave until the band has,
 * and the pill may not fly home until its dock is back on screen.
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
 * Is the flap in the DOM? Everywhere but settled theater. It stays mounted and
 * hidden through the return flight because it is the dock the choreography
 * measures, so the capsule lands on its real pixels and the swap moves nothing.
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
 * Is the flight, rather than the pill's drag state, saying where the pill sits?
 * Only on the way home: the detach flies to coordinates the pill already holds
 * and only adds a transform, while the return pins the box where it is leaving
 * and parks it on the flap's, neither of which belongs to the drag state.
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
 * The FLIP translation from one painted box to another. Deliberately pure: the
 * pill starts the detach gripless, where its box is the flap's box exactly, and
 * a scale would smear the border and the glyphs for the length of the flight.
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
 * The same shadow with every colour taken to fully transparent, which is what
 * the pill's shadow interpolates from: a shadow going to or from `none` snaps
 * rather than fading, and the flap has none.
 *
 * `null` when there is nothing to fade (no shadow, or a value with no colour in
 * it, as a test environment with no stylesheet reports). The caller then skips
 * that half of the animation rather than guessing at a value.
 */
export function transparentShadow(shadow: string | null | undefined): string | null {
  if (!shadow) return null
  const trimmed = shadow.trim()
  if (trimmed === "" || trimmed === "none") return null
  if (!new RegExp(SHADOW_COLOR.source, "i").test(trimmed)) return null
  return trimmed.replace(SHADOW_COLOR, "rgba(0, 0, 0, 0)")
}

// The docked flap's element, registered at module level as `layoutGesture.ts`
// and `terminalFocus.ts` do: the flap is a sibling of the pane and the pill is
// inside it, so a prop chain would cross the whole terminal component. What
// travels is a measurement, not control.
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

/// The last colour a flap published, remembered past its unmount.
///
/// A flap is not one colour: it takes the tone of the band it hangs from, the
/// tab strip's or the plain app background, and publishes its own answer on the
/// element the flight already measures. The flap is unmounted for the whole
/// floating stage and the pill's settled background is that same colour, so
/// without the memory the pill would repaint in the strip's tone once the
/// resting stage cleared the flight's writes. Page-global, like the
/// registration it shadows, which suits a phone showing one pane at a time.
let lastFlapFill = ""

export function peekFlapFill(): string {
  const published = flapElement?.style.getPropertyValue(FLAP_FILL_VAR).trim()
  if (published) lastFlapFill = published
  return lastFlapFill || "var(--dux-flap-bg)"
}
