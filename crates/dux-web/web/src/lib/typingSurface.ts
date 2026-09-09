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

/** The latch behind the once-per-device "here is the way back" hint. */
export const DIRECT_INPUT_HINT_KEY = "dux:direct-input-hint"

/**
 * Should the "where the way back lives" hint fire on this device? Storage that
 * cannot be read cannot be written either, so a browser refusing it never hints
 * rather than hinting on every switch, as the theater pill's grip hint does.
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

/// Which flip took the last row away. Both leave the pane with nothing under
/// its terminal and the way back in the same top menu, so they share one latch;
/// only the row that went, and the item that brings it back, differ.
export type VirtualInputExit = "direct" | "keys"

/**
 * The hint's sentence, naming a control that exists on the shell it fires on:
 * on a phone the `⋯` over the terminal, on a computer the row menu in the
 * sidebar. The item named follows the flip that was made, since a message box
 * and a key row come back through different entries. Pure, so a test can pin
 * both without a DOM.
 */
export function directHintMessage(
  shell: TypingSurfaceShell,
  exit: VirtualInputExit = "direct",
): string {
  const where =
    shell === "phone"
      ? "The ⋯ button over the terminal"
      : "This pane's own ⋯ menu, on its row in the sidebar,"
  return exit === "direct"
    ? `Typing goes straight to the terminal now. ${where} has “Use virtual input” when you want the message box back.`
    : `The terminal keys are hidden now. ${where} has “Show terminal keys” when you want them back.`
}

/**
 * Raise the one-time "here is the way back" hint, if this is its moment. One
 * latch per device covers both flips: they teach the same lesson, so leaving by
 * the other door does not earn a second telling.
 */
function raiseVirtualInputHint(
  exit: VirtualInputExit,
  nothingLeftBelow: boolean,
): void {
  if (!nothingLeftBelow) return
  if (!directHintPending()) return
  // Marked BEFORE the raise, so a double-invoked caller cannot produce two.
  markDirectHintShown()
  notifyInfo(directHintMessage(isMobileViewport() ? "phone" : "computer", exit))
}

/**
 * The other way out of the virtual input: hiding the terminal keys. From direct
 * typing, that row is the bottom bar and the `⋯` on it is the visible way back,
 * so hiding it raises the same hint the surface switch does. The caller answers
 * whether anything is left below; this module cannot see the rows.
 */
export function hideTerminalKeysHint(nothingLeftBelow: boolean): void {
  raiseVirtualInputHint("keys", nothingLeftBelow)
}

/**
 * The one gesture that changes the typing surface: every surface that flips the
 * choice calls this rather than `setTypingSurface`, so none of them can raise a
 * different hint or none at all.
 *
 * `nothingLeftBelow` is what narrows the hint to the switch that leaves nothing
 * under the terminal at all, the `⋯` included; a key row that stays is a bottom
 * `⋯` that visibly carries the way back. The caller knows that, this module
 * cannot, and the shell is read from the width here for the same reason. The
 * hint is info and not sticky: nothing is lost if it goes unread.
 */
export function switchTypingSurface(
  next: TypingSurface,
  nothingLeftBelow: boolean,
): void {
  setTypingSurface(next)
  if (next !== "direct") return
  raiseVirtualInputHint("direct", nothingLeftBelow)
}

/** Subscribe to changes; returns the unsubscribe. */
export function subscribeTypingSurface(listener: () => void): () => void {
  listeners.add(listener)
  return () => listeners.delete(listener)
}
