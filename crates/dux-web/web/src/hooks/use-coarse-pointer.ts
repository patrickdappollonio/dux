import * as React from "react"

// Is touch the primary way this person points at the screen? Deliberately not
// `useIsMobile`, a viewport width query: rotating a tablet crosses the width breakpoint
// and would swap the typing surface mid-session, while `pointer: coarse` does not change
// with orientation.
//
// Measured, and each kills a narrower-looking alternative: `any-pointer: fine` is true on
// a plain Android phone, and a stylus reports hover, so both "coarse and no fine pointer"
// and "coarse and nothing hovers" hide the bar on the devices it exists for. An Android
// tablet with and without a physical keyboard is identical under every interaction query,
// which is why `ui.compose_bar` is three-way: `auto` reads this hook, and `always`/`never`
// resolve a case the browser cannot see.
const COARSE_POINTER_QUERY = "(pointer: coarse)"

function subscribe(callback: () => void) {
  // jsdom ships no matchMedia; the snapshot already degrades to `false`, so a missing
  // matchMedia costs only the subscription. Same shape as `use-mobile.ts`.
  if (typeof window.matchMedia !== "function") return () => {}
  const mql = window.matchMedia(COARSE_POINTER_QUERY)
  mql.addEventListener("change", callback)
  return () => mql.removeEventListener("change", callback)
}

function snapshot() {
  // There is no non-matchMedia way to ask this, so a browser without it reads as
  // not-coarse, which lands on typing straight into the terminal.
  if (typeof window.matchMedia !== "function") return false
  return window.matchMedia(COARSE_POINTER_QUERY).matches
}

/**
 * True when the primary pointing device is coarse (a finger), live-updating as the browser
 * re-evaluates the query. Read during render via `useSyncExternalStore`, so there is no
 * initial `undefined` flash and the `false` server snapshot keeps it SSR-safe.
 */
export function useIsCoarsePointer() {
  return React.useSyncExternalStore(subscribe, snapshot, () => false)
}
