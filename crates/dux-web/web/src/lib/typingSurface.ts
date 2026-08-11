// WHERE THE USER LAST LEFT THE TYPING-SURFACE TOGGLE, on this device.
//
// This is transient UI state and it must stay that way. `ui.compose_bar`
// (auto/always/never) is the one CONFIGURATION surface for the compose bar, and
// the toggle deliberately does not write it: the same tablet wants the buffered
// box with no keyboard case attached and direct typing with one, which is a
// question about the next ten minutes rather than about how dux is set up. The
// browser genuinely cannot see the difference (measured: with and without a
// physical keyboard, every interaction media query is identical), so the person
// swaps it themselves.
//
// It is remembered in `localStorage` for one reason: a reload that snapped the
// surface back under someone mid-session would make the toggle feel broken.
// Nothing else about it is persistent, and it is per DEVICE by construction,
// which is what a device-shaped question deserves.
//
// A module-level listener set, read through `useSyncExternalStore`, so every
// open pane in the tab agrees the moment one of them flips it. The snapshot
// reads storage on every call rather than caching: it returns a string or null,
// so `useSyncExternalStore` compares it by value and never loops, and a fresh
// read is what makes a second tab's write visible on the next render instead of
// needing a cache to invalidate.

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

/** Read the device-local choice. An unrecognized stored value reads as unchosen. */
export function readTypingSurface(): TypingSurface | null {
  let raw: string | null
  try {
    raw = storage()?.getItem(TYPING_SURFACE_KEY) ?? null
  } catch {
    return fallback
  }
  if (raw === "compose" || raw === "direct") return raw
  return raw === null ? fallback : null
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

/** Subscribe to changes; returns the unsubscribe. */
export function subscribeTypingSurface(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}
