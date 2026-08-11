import * as React from "react"

// Is TOUCH the primary way this person points at the screen?
//
// This is deliberately NOT `useIsMobile` (which is a viewport WIDTH query, and
// stays the right signal for layout, menus and touch-target sizing). It exists
// because the compose bar is a decision about the INPUT METHOD, not about how
// much room there is: rotating a tablet crosses the width breakpoint and the
// typing surface changed underneath the user mid-session, which is the bug this
// replaced. `pointer: coarse` does not change with orientation.
//
// MEASURED, and recorded here because each of these kills an alternative that
// looks more precise and will otherwise be tried again:
//
//   - `any-pointer: fine` is TRUE on a plain Android phone with no mouse
//     attached. So "coarse AND no fine pointer anywhere" hides the bar on the
//     exact device it exists for.
//   - A stylus reports HOVER. So "coarse AND nothing can hover" hides the bar
//     on a pen tablet that has no keyboard.
//   - On an Android tablet, WITH and WITHOUT a physical keyboard attached,
//     every interaction media query is identical (`pointer: coarse` true,
//     `any-pointer: fine` true, `any-hover: hover` false). The two situations
//     are indistinguishable to the browser.
//
// That last measurement is why the `ui.compose_bar` setting is three-way rather
// than a pure capability gate: `auto` uses this hook, and `always`/`never` exist
// because only the user can resolve a case the browser genuinely cannot see.
const COARSE_POINTER_QUERY = "(pointer: coarse)"

function subscribe(callback: () => void) {
  // jsdom ships no matchMedia. The snapshot below already degrades to `false`
  // without it, so a missing matchMedia only costs the SUBSCRIPTION; return a
  // no-op unsubscribe rather than crashing tests that never stubbed it. Same
  // shape as `use-mobile.ts`.
  if (typeof window.matchMedia !== "function") return () => {}
  const mql = window.matchMedia(COARSE_POINTER_QUERY)
  mql.addEventListener("change", callback)
  return () => mql.removeEventListener("change", callback)
}

function snapshot() {
  // Unlike `use-mobile.ts`, whose snapshot can read `window.innerWidth`
  // directly, there is no non-matchMedia way to ask this question. A browser
  // without matchMedia reads as not-coarse, which lands on the same
  // type-into-the-terminal behavior as before the compose bar existed.
  if (typeof window.matchMedia !== "function") return false
  return window.matchMedia(COARSE_POINTER_QUERY).matches
}

/**
 * True when the primary pointing device is coarse (a finger), live-updating if
 * the browser re-evaluates the query.
 *
 * Read the live value during render via `useSyncExternalStore` rather than
 * mirroring it into state in an effect: no initial `undefined` flash, no
 * synchronous `setState` in an effect, and SSR-safe through the `false` server
 * snapshot. Same construction as `useIsMobile`.
 */
export function useIsCoarsePointer() {
  return React.useSyncExternalStore(subscribe, snapshot, () => false)
}
