// Whether the phone's action flap is folded away, on this device.
//
// Per-device view state, one answer for every pane: never configuration and
// never the server. The page holds its own answer, read from `localStorage`
// once and written back on every press, so a reload keeps the terminal's top
// lines clear; another tab's press reaches this one through the `storage` event
// as it happens, and at no other time.

/** The `localStorage` key. */
export const FLAP_HIDDEN_KEY = "dux:phone-flap-hidden"

// Storage can be missing or throw (Safari private mode); both leave the page's
// own answer in charge, so the button keeps working for the life of the page.
function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage
  } catch {
    return null
  }
}

/** Anything this module did not write reads as "shown". */
function parse(raw: string | null): boolean {
  return raw === "1"
}

let current: boolean | null = null
let changedAt = Number.NEGATIVE_INFINITY

const listeners = new Set<() => void>()

function change(next: boolean): void {
  current = next
  changedAt = performance.now()
  for (const listener of listeners) listener()
}

/** Read the choice. */
export function readFlapHidden(): boolean {
  if (current === null) {
    try {
      current = parse(storage()?.getItem(FLAP_HIDDEN_KEY) ?? null)
    } catch {
      current = false
    }
  }
  return current
}

/** When the choice last moved, on the `performance.now()` clock. */
export function flapHiddenChangedAt(): number {
  return changedAt
}

/** Forget the page's answer and when it moved, as a fresh page would. */
export function resetFlapHiddenForTests(): void {
  current = null
  changedAt = Number.NEGATIVE_INFINITY
}

/** Fold the flap away (`true`) or bring it back (`false`). */
export function setFlapHidden(next: boolean): void {
  try {
    storage()?.setItem(FLAP_HIDDEN_KEY, next ? "1" : "0")
  } catch {
    // Not kept across a reload; this page still follows the press.
  }
  if (next !== readFlapHidden()) change(next)
}

function onStorage(ev: StorageEvent): void {
  // A `clear()` in another tab arrives with no key.
  if (ev.key !== FLAP_HIDDEN_KEY && ev.key !== null) return
  const next = parse(ev.newValue)
  if (next !== readFlapHidden()) change(next)
}

// Listened to for the life of the page rather than per subscriber, so a pane
// mounted after another tab's press still opens on that press.
if (typeof window !== "undefined") window.addEventListener("storage", onStorage)

/** Subscribe to changes, this tab's and other tabs'; returns the unsubscribe. */
export function subscribeFlapHidden(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}
