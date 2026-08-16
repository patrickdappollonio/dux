/**
 * Telling dux's own replayed DOM events apart from the visitor's real ones.
 *
 * Two paths in the terminal pane dispatch mouse events at xterm on purpose: the
 * OSC 8 link probe (`lib/termlink.ts`) and the touch-gesture forwarder
 * (`lib/termmouse.ts`). Both are deliberate replays, and both must pass THROUGH
 * the pane's own capture-phase listeners rather than being judged by them: the
 * link intercept's whole job is to decide what a HUMAN press means, and a
 * replay dispatched at a descendant still travels the capture path from the
 * container down (a non-bubbling event skips the bubble phase, never the
 * capture one).
 *
 * `isTrusted` is NOT the discriminator, even though it reads like the obvious
 * one. jsdom dispatches are never trusted, so an `isTrusted` guard would make
 * every component test of the intercept exercise nothing at all, and a click
 * synthesized by an assistive technology is untrusted too: it is a real user
 * intent and deserves the real decision. So dux tags what dux dispatched, and
 * nothing else. A `WeakSet` keeps the tag off the event object and lets the
 * entry go with the event.
 */

const duxReplays = new WeakSet<Event>()

/** Marks an event as one dux dispatched itself. Returns it for chaining. */
export function markDuxReplay<T extends Event>(event: T): T {
  duxReplays.add(event)
  return event
}

/** Whether this event came out of one of dux's own replays. */
export function isDuxReplay(event: Event): boolean {
  return duxReplays.has(event)
}
