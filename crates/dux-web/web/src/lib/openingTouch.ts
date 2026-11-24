// The touch that opens a context menu must not also choose from it.
//
// A long press opens the file tree's menu as a card over the bottom of the
// screen, and the finger that opened it is, for a row low on the screen, still
// resting over the card's items. Lifting it must choose nothing, on every
// platform, however the browser reports the lift: Android sends a `contextmenu`
// while the finger is down, iOS sends none and the menu opens on a timer, and
// either may follow the lift with a click on whatever is under the finger.
//
// So the card is armed when it opens with a touch still down. While armed, the
// touch's release is cancelled (which is how a browser is told to send no click
// after it) and any click or mouse release that arrives anyway is swallowed
// before it reaches an item. The first input that begins afterwards, a new
// touch, a pointer press or a key, disarms it, so a deliberate tap, a mouse and
// the keyboard are untouched; a card opened with no touch down (a mouse
// right-click, the keyboard) is never armed.
//
// A browser's compatibility mouse events follow a touch's release, so a mouse
// press is deliberately not one of the disarming inputs: a real mouse begins
// with a pointer press, which is.

let touchDown = false
let tracking: Document | null = null

/** Starts watching whether a finger is down. Safe to call again. */
export function trackTouches(doc: Document = document): void {
  if (tracking === doc) return
  tracking = doc
  const begin = (event: Event) => {
    touchDown = ((event as TouchEvent).touches?.length ?? 1) > 0
  }
  const end = (event: Event) => {
    touchDown = ((event as TouchEvent).touches?.length ?? 0) > 0
  }
  doc.addEventListener("touchstart", begin, { capture: true, passive: true })
  doc.addEventListener("touchend", end, { capture: true, passive: true })
  doc.addEventListener("touchcancel", end, { capture: true, passive: true })
}

/** Whether a finger is down right now. */
export function touchIsDown(): boolean {
  return touchDown
}

const SWALLOWED = ["click", "auxclick", "mouseup"] as const
const RELEASES = ["touchend", "touchcancel"] as const
const NEW_INPUT = ["touchstart", "pointerdown", "keydown"] as const

/**
 * Arms the guard for a card that has just opened, and returns the way to take
 * it down when the card closes. Does nothing, and returns a no-op, when no
 * finger is down.
 */
export function guardOpeningTouch(doc: Document = document): () => void {
  if (!touchDown) return () => {}
  let armed = true
  const cancel = (event: Event) => {
    if (armed && event.cancelable) event.preventDefault()
  }
  const swallow = (event: Event) => {
    if (!armed) return
    event.preventDefault()
    event.stopPropagation()
  }
  const disarm = () => {
    armed = false
  }
  // Registered first, so a new input disarms before anything else it reaches.
  for (const type of NEW_INPUT) doc.addEventListener(type, disarm, { capture: true })
  for (const type of RELEASES) doc.addEventListener(type, cancel, { capture: true, passive: false })
  for (const type of SWALLOWED) doc.addEventListener(type, swallow, { capture: true })
  return () => {
    for (const type of NEW_INPUT) doc.removeEventListener(type, disarm, { capture: true })
    for (const type of RELEASES) doc.removeEventListener(type, cancel, { capture: true })
    for (const type of SWALLOWED) doc.removeEventListener(type, swallow, { capture: true })
  }
}
