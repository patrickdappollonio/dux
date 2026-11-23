import * as React from "react"

// Every declarative animation is gated in CSS through Tailwind's `motion-safe:`
// / `motion-reduce:` variants. A JS-driven animation has no such variant to hang
// off, so it asks the media query directly: the glyph spinner advances its frame
// on a timer, and a timer keeps ticking however the CSS is written.
/// The query `usePrefersReducedMotion` subscribes to, exported so tests can
/// stub the same string the hook asks for.
export const REDUCED_MOTION_QUERY = "(prefers-reduced-motion: reduce)"

function subscribe(callback: () => void) {
  // jsdom ships no matchMedia. The snapshot below degrades to "motion is fine"
  // without it, so a missing matchMedia costs only the subscription; return a
  // no-op unsubscribe rather than crashing tests that never stubbed it.
  if (typeof window.matchMedia !== "function") return () => {}
  const mql = window.matchMedia(REDUCED_MOTION_QUERY)
  mql.addEventListener("change", callback)
  return () => mql.removeEventListener("change", callback)
}

function snapshot() {
  if (typeof window.matchMedia !== "function") return false
  return window.matchMedia(REDUCED_MOTION_QUERY).matches
}

/**
 * True when the user prefers reduced motion, live-updating if they change the
 * system setting while the page is open. Read through `useSyncExternalStore`:
 * no initial `undefined` flash, no synchronous `setState` in an effect, and a
 * safe `false` server snapshot.
 */
export function usePrefersReducedMotion() {
  return React.useSyncExternalStore(subscribe, snapshot, () => false)
}
