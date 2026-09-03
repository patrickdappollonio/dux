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

import { isMobileViewport } from "@/hooks/use-mobile"

import { notifyInfo } from "./notify"

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
 * Read the device-local choice.
 *
 * Only a value this module wrote counts. Anything else, missing or garbage, is
 * "nothing stored", and nothing stored falls through to the in-memory answer.
 * That last step is what a storage which allows reads and refuses writes needs:
 * the string sitting under the key is stale or nonsense, the choice the user
 * just made lives only in `fallback`, and reading the nonsense as a decision
 * would leave the toggle looking dead for the rest of the page.
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

/** The latch behind the once-per-device "here is the way back" hint. */
export const DIRECT_INPUT_HINT_KEY = "dux:direct-input-hint"

/**
 * Should the "where the way back lives" hint fire on this device?
 *
 * Storage that cannot be read cannot be written either, so a browser that
 * refuses it never hints rather than hinting on every switch. Same shape, and
 * same reason, as the theater pill's grip hint.
 */
export function directHintPending(): boolean {
  try {
    const store = storage()
    if (!store) return false
    return store.getItem(DIRECT_INPUT_HINT_KEY) === null
  } catch {
    return false
  }
}

/** Never hint on this device again. Best-effort. */
export function markDirectHintShown(): void {
  try {
    storage()?.setItem(DIRECT_INPUT_HINT_KEY, "shown")
  } catch {
    // See `directHintPending`: a storage that refuses writes also refuses
    // reads, so the hint is already suppressed.
  }
}

/// Which shell the hint is being read on. The two keep the way back in
/// different places, and a sentence that names the wrong one is worse than no
/// sentence: it sends the reader looking for a control that is not there.
export type TypingSurfaceShell = "phone" | "computer"

/**
 * The hint's sentence, naming a control that exists on the shell it fires on.
 *
 * On a phone the pane's own `⋯` is the cluster over the terminal (docked in the
 * band, or floating in theater); on a computer it is the agent's or terminal's
 * row menu in the sidebar. Pure, so a test can pin both without a DOM.
 */
export function directHintMessage(shell: TypingSurfaceShell): string {
  const where =
    shell === "phone"
      ? "The ⋯ button over the terminal"
      : "This pane's own ⋯ menu, on its row in the sidebar,"
  return `Typing goes straight to the terminal now. ${where} has “Use virtual input” when you want the message box back.`
}

/**
 * THE ONE GESTURE that changes the typing surface, and the reason the switch
 * cannot become a dead end.
 *
 * Choosing to type directly in the terminal takes the message box away, and on
 * a pane whose terminal keys are down too it takes the last row under the
 * terminal with it, the `⋯` that hung off it included. That is the moment a
 * user has no way of knowing where the way back went, so the first time it
 * happens on a device dux says so, once, through the one raiser. INFO and not
 * sticky: nothing is lost if it goes unread, and the menu it names is on screen
 * either way.
 *
 * `nothingLeftBelow` is what makes it that moment rather than every switch. A
 * key row that stays is a bottom `⋯` that stays, visibly carrying the way back,
 * and a toast sending the reader to a different menu would be noise pointing at
 * the wrong control. The caller knows which case it is; this module cannot.
 *
 * `setTypingSurface` stays the writer. Every surface that flips the choice
 * calls this instead, so none of them can raise a different hint or none at all.
 *
 * The sentence names a control that is actually on the shell reading it, which
 * is why the width is consulted here rather than a shell being threaded down
 * from each call site: none of them knows, and all of them are on screen on
 * both shells.
 */
export function switchTypingSurface(
  next: TypingSurface,
  nothingLeftBelow: boolean,
): void {
  setTypingSurface(next)
  if (next !== "direct") return
  if (!nothingLeftBelow) return
  if (!directHintPending()) return
  // Marked BEFORE the raise, so a double-invoked caller cannot produce two.
  markDirectHintShown()
  notifyInfo(directHintMessage(isMobileViewport() ? "phone" : "computer"))
}

/** Subscribe to changes; returns the unsubscribe. */
export function subscribeTypingSurface(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}
