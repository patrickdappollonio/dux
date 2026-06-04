import { useSyncExternalStore } from "react"

// Tracks the visual viewport height (window.visualViewport.height), which on
// mobile shrinks when the soft keyboard opens — unlike the layout viewport /
// innerHeight, which does not. Constraining the terminal screen to this height
// keeps the top bar, terminal, and accessory bar above the keyboard.
//
// Implemented with useSyncExternalStore (the repo-sanctioned reactive-browser-
// API pattern) so there is no setState-in-effect. Returns null when the API is
// absent (older browsers, or non-mobile desktops where there's no keyboard to
// account for), letting callers fall back to their default sizing.

function subscribe(onChange: () => void): () => void {
  const vv = window.visualViewport
  if (!vv) return () => {}
  // Resize fires when the keyboard opens/closes; scroll fires when the visual
  // viewport pans (e.g. the keyboard nudges the page), which also changes the
  // usable height envelope, so we re-snapshot on both.
  vv.addEventListener("resize", onChange)
  vv.addEventListener("scroll", onChange)
  return () => {
    vv.removeEventListener("resize", onChange)
    vv.removeEventListener("scroll", onChange)
  }
}

function getSnapshot(): number | null {
  const vv = window.visualViewport
  return vv ? Math.round(vv.height) : null
}

// Server snapshot: there is no visual viewport without a browser. Returning
// null keeps SSR/non-DOM renders on the default sizing path.
function getServerSnapshot(): number | null {
  return null
}

export function useVisualViewportHeight(): number | null {
  return useSyncExternalStore(subscribe, getSnapshot, getServerSnapshot)
}
