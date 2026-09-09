import * as React from "react"

export const MOBILE_BREAKPOINT = 768

/**
 * The same width question, for a caller that is not a component: it reads the
 * width the hook's snapshot reads, from the one breakpoint constant, so the two
 * can never answer differently. Outside a browser there is no shell at all,
 * which reads as the desktop one.
 */
export function isMobileViewport(): boolean {
  if (typeof window === "undefined") return false
  return window.innerWidth < MOBILE_BREAKPOINT
}

function subscribe(callback: () => void) {
  // jsdom ships no matchMedia. A missing one costs only the resize subscription,
  // since the snapshot below reads window.innerWidth directly, so degrade to a
  // no-op unsubscribe instead of crashing tests that never stubbed it.
  if (typeof window.matchMedia !== "function") return () => {}
  const mql = window.matchMedia(`(max-width: ${MOBILE_BREAKPOINT - 1}px)`)
  mql.addEventListener("change", callback)
  return () => mql.removeEventListener("change", callback)
}

// `useSyncExternalStore` rather than state mirrored in an effect: the live value
// is read during render (no initial `undefined` flash, no synchronous
// `setState`) and the `false` server snapshot keeps it SSR-safe.
export function useIsMobile() {
  return React.useSyncExternalStore(
    subscribe,
    () => window.innerWidth < MOBILE_BREAKPOINT,
    () => false
  )
}
