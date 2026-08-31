import * as React from "react"

// Has this person asked their system to cut down on animation?
//
// CSS answers this on its own through Tailwind's `motion-safe:` /
// `motion-reduce:` variants, which is how every declarative animation in the
// app is gated (the working-agent bob, the attention dot, the name shimmer).
// A JS-driven animation has no such variant to hang off, so it has to ask the
// same media query directly: the glyph spinner advances its frame on a timer,
// and a timer keeps ticking however the CSS is written.
/// The query `usePrefersReducedMotion` subscribes to, exported so tests can
/// stub the same string the hook asks for.
export const REDUCED_MOTION_QUERY = "(prefers-reduced-motion: reduce)"

function subscribe(callback: () => void) {
  // jsdom ships no matchMedia. The snapshot below already degrades to "motion
  // is fine" without it, so a missing matchMedia costs only the SUBSCRIPTION;
  // return a no-op unsubscribe rather than crashing tests that never stubbed
  // it. Same shape as `use-coarse-pointer.ts`.
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
 * system setting while the page is open.
 *
 * Read through `useSyncExternalStore` for the same reasons as
 * `useIsCoarsePointer`: no initial `undefined` flash, no synchronous
 * `setState` in an effect, and a safe `false` server snapshot.
 */
export function usePrefersReducedMotion() {
  return React.useSyncExternalStore(subscribe, snapshot, () => false)
}
