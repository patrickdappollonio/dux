import { useSyncExternalStore } from "react"

// The visual viewport height, which on mobile shrinks when the soft keyboard
// opens while the layout viewport and innerHeight do not; constraining the
// terminal screen to it keeps the top bar, terminal and accessory bar above the
// keyboard. Null where the API is absent, so callers fall back to their default
// sizing. Read through useSyncExternalStore, so there is no setState-in-effect.

function subscribe(onChange: () => void): () => void {
  const vv = window.visualViewport
  if (!vv) return () => {}
  // Resize fires when the keyboard opens or closes; scroll fires when the visual
  // viewport pans, which also changes the usable height, so both re-snapshot.
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
