// Where the user last left the typing-surface toggle, on this device.
//
// Transient per-device UI state, and it must stay that way: `ui.compose_bar`
// (auto/always/never) is the one configuration surface for the compose bar and
// this toggle never writes it. Kept in `localStorage` only so a reload does not
// snap the surface back mid-session.
//
// A module-level listener set, read through `useSyncExternalStore`, so every
// open pane in the tab agrees the moment one flips it. The snapshot reads
// storage on every call: it returns a string or null, compared by value, so a
// second tab's write shows on the next render with no cache to invalidate.

/** The `localStorage` key. */
export const TYPING_SURFACE_KEY = "dux:typing-surface"

/**
 * Which typing surface the user chose on this device, or `null` while they
 * have not chosen and the pointer capability answers for them.
 *
 * `compose` is the buffered message box, `direct` is typing straight into the
 * terminal.
 */
export type TypingSurface = "compose" | "direct"

// Storage can be missing (SSR, a test that never stubbed it) or throw outright
// (Safari private mode), and neither is a reason for a terminal pane to fail to
// render. Both degrade to "nothing chosen", which lands on the capability
// answer, exactly where the feature started.
function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage
  } catch {
    return null
  }
}

// The chosen value while storage refuses to keep it, so a private-mode browser
// still gets a working toggle for the life of the page.
let fallback: TypingSurface | null = null

const listeners = new Set<() => void>()

/**
 * Read the device-local choice. Only a value this module wrote counts; missing
 * or garbage falls through to the in-memory answer, which is what a storage
 * that allows reads and refuses writes needs so the toggle keeps working.
 */
export function readTypingSurface(): TypingSurface | null {
  let raw: string | null
  try {
    raw = storage()?.getItem(TYPING_SURFACE_KEY) ?? null
  } catch {
    return fallback
  }
  if (raw === "compose" || raw === "direct") return raw
  return fallback
}

/** Write the choice (or `null` to hand the decision back to the pointer). */
export function setTypingSurface(next: TypingSurface | null): void {
  fallback = next
  try {
    const store = storage()
    if (!store) return
    if (next === null) store.removeItem(TYPING_SURFACE_KEY)
    else store.setItem(TYPING_SURFACE_KEY, next)
  } catch {
    // Storage refused; the in-memory fallback above still carries the choice.
  } finally {
    for (const listener of listeners) listener()
  }
}

/**
 * The one gesture that changes the typing surface: every surface that flips the
 * choice calls this rather than `setTypingSurface`, so they cannot drift.
 */
export function switchTypingSurface(next: TypingSurface): void {
  setTypingSurface(next)
}

/** Subscribe to changes; returns the unsubscribe. */
export function subscribeTypingSurface(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}
