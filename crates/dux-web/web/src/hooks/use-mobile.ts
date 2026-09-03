import * as React from "react"

export const MOBILE_BREAKPOINT = 768

/**
 * The same width question, for a caller that is not a component.
 *
 * WHICH SHELL IS ON SCREEN is a fact some non-React code has to state (the
 * typing-surface hint names a control, and the phone and the computer keep it
 * in different places). It reads the width the hook's snapshot reads, from the
 * one breakpoint constant, so the two can never answer differently. Outside a
 * browser there is no shell at all, which reads as the desktop one.
 */
export function isMobileViewport(): boolean {
  if (typeof window === "undefined") return false
  return window.innerWidth < MOBILE_BREAKPOINT
}

function subscribe(callback: () => void) {
  // jsdom ships no matchMedia, and the shared dropdown-menu primitive now calls
  // useIsMobile in every menu-rendering test. A missing matchMedia only costs
  // the resize SUBSCRIPTION (the snapshot below reads window.innerWidth
  // directly), so degrade to a no-op unsubscribe instead of crashing tests
  // that never stubbed it.
  if (typeof window.matchMedia !== "function") return () => {}
  const mql = window.matchMedia(`(max-width: ${MOBILE_BREAKPOINT - 1}px)`)
  mql.addEventListener("change", callback)
  return () => mql.removeEventListener("change", callback)
}

// Subscribe to the viewport-width media query via `useSyncExternalStore` rather
// than mirroring it into state inside an effect. This reads the live value during
// render (no initial `undefined` flash, no synchronous `setState` in an effect)
// and stays SSR-safe through the `false` server snapshot.
export function useIsMobile() {
  return React.useSyncExternalStore(
    subscribe,
    () => window.innerWidth < MOBILE_BREAKPOINT,
    () => false
  )
}
