import * as React from "react"

const MOBILE_BREAKPOINT = 768

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
