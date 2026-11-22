// Whether the phone's action flap is folded away, on this device.
//
// Per-device view state, one answer for every pane: never configuration and
// never the server. Kept in `localStorage` so a reload keeps the terminal's top
// lines clear, and read through `useSyncExternalStore` (see
// `hooks/use-flap-hidden.ts`) so every screen agrees the moment one flips it.

/** The `localStorage` key. */
export const FLAP_HIDDEN_KEY = "dux:phone-flap-hidden"

// Storage can be missing or throw (Safari private mode); both degrade to the
// in-memory answer, so the button keeps working for the life of the page.
function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage
  } catch {
    return null
  }
}

let fallback = false

const listeners = new Set<() => void>()

/** Read the choice. Anything this module did not write reads as "shown". */
export function readFlapHidden(): boolean {
  let raw: string | null
  try {
    raw = storage()?.getItem(FLAP_HIDDEN_KEY) ?? null
  } catch {
    return fallback
  }
  if (raw === "1") return true
  if (raw === "0") return false
  return fallback
}

/** Fold the flap away (`true`) or bring it back (`false`). */
export function setFlapHidden(next: boolean): void {
  fallback = next
  try {
    storage()?.setItem(FLAP_HIDDEN_KEY, next ? "1" : "0")
  } catch {
    // Storage refused; the in-memory fallback above still carries the choice.
  } finally {
    for (const listener of listeners) listener()
  }
}

/** Subscribe to changes; returns the unsubscribe. */
export function subscribeFlapHidden(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}
