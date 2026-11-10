// Theater mode: the one-pane, no-chrome layout for a terminal surface. The pure
// rules live here (the memory key, the Escape verdict, how the mode rides the
// address); the store owns the live flag and the URL write.
//
// Theater is deliberately not the browser's Fullscreen API: Android hard-clips a
// fullscreen page against the keyboard-shrunk viewport, and what is reclaimed
// here is dux's own chrome, which no system mode could give back.

import type { SelectedTarget } from "./store"
import { matchOwner } from "./terminalOwner"

/** The `localStorage` key prefix every row's theater memory is filed under. */
export const THEATER_STORAGE_PREFIX = "dux:theater:"

/**
 * A row of the agents list, which is what theater is remembered for: an agent
 * with all its tabs and terminals, or a project or standalone terminal.
 */
export type TheaterRow =
  | { kind: "agent"; sessionId: string }
  | { kind: "terminal"; terminalId: string }

/** The key a row's theater memory lives at. */
export function theaterRowKey(row: TheaterRow): string {
  return row.kind === "agent"
    ? `${THEATER_STORAGE_PREFIX}agent:${row.sessionId}`
    : `${THEATER_STORAGE_PREFIX}terminal:${row.terminalId}`
}

/**
 * The row a focused part belongs to, or `null` when nothing is focused. Every
 * tab of an agent and every terminal of its dock answer with the agent, so
 * switching between them never enters or leaves theater.
 */
export function theaterRow(target: SelectedTarget | null): TheaterRow | null {
  if (!target) return null
  if (target.kind === "agent") return { kind: "agent", sessionId: target.sessionId }
  const terminalId = target.terminalId
  return matchOwner<TheaterRow>(target.owner, {
    session: ({ sessionId }) => ({ kind: "agent", sessionId }),
    project: () => ({ kind: "terminal", terminalId }),
    standalone: () => ({ kind: "terminal", terminalId }),
  })
}

/** The memory key of the row a focused part belongs to. */
export function theaterMemoryKey(target: SelectedTarget | null): string | null {
  const row = theaterRow(target)
  return row === null ? null : theaterRowKey(row)
}

// Storage can be missing or throw outright (private mode, a browser set to block
// site data). Both degrade to "not in theater", the state a pane starts in.
function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage
  } catch {
    return null
  }
}

/** Was this row last left in theater? Anything unreadable reads as "no". */
export function readTheaterMemory(key: string | null): boolean {
  if (!key) return false
  try {
    return storage()?.getItem(key) === "on"
  } catch {
    return false
  }
}

/** Remember (or forget) that this row is in theater. Best-effort. */
export function writeTheaterMemory(key: string | null, on: boolean): void {
  if (!key) return
  try {
    const store = storage()
    if (!store) return
    if (on) store.setItem(key, "on")
    else store.removeItem(key)
  } catch {
    // Storage refused. The live mode still works for the life of the page; only
    // the memory is lost, which is the cheap half.
  }
}

/**
 * Forget a row's theater memory outright. Losing input ownership calls this:
 * after another device takes over, re-entering theater is a fresh choice.
 */
export function clearTheaterMemory(key: string | null): void {
  writeTheaterMemory(key, false)
}

// How theater rides the address (`?view=theater` after the position) is the
// query tail's business, written and read in `lib/routeQuery.ts` together with
// the editor's view. What may carry it is decided here.

/**
 * Whether a route may carry the modifier at all. Theater modifies a focused
 * pane, so it is dropped for every address with no PTY to give the height to,
 * which would otherwise restore a mode nothing can honour.
 */
export function theaterSerializable(route: {
  target: unknown | null
  changes: boolean
  editor: unknown | null
  standalone: boolean
}): boolean {
  return (
    route.target !== null &&
    !route.changes &&
    route.editor === null &&
    !route.standalone
  )
}

/** The slice of a keyboard event the Escape rule reads, plus its context. */
export interface TheaterEscEvent {
  type: string
  key: string
  ctrlKey: boolean
  shiftKey: boolean
  altKey: boolean
  metaKey: boolean
  isComposing: boolean
  keyCode: number
  /// Whether the keystroke landed in something that can be typed into: the
  /// compose textarea, xterm's own helper textarea, a dialog's field.
  inTypingSurface: boolean
  /// Whether an overlay has already answered this Escape.
  defaultPrevented: boolean
  /// Whether the pane is in theater at all.
  theater: boolean
}

/** What one Escape does in theater. */
export type TheaterEscapeAction = "none" | "exit"

/**
 * Does this Escape belong to theater, and what does it do?
 *
 * It is the exit only where nothing else wants the key. Over a focused compose
 * box or xterm the Escape is the child's (`composeHardwareKeyForwards`), and an
 * overlay that answered by closing marks the keydown `defaultPrevented`, so one
 * press never both closes a menu and leaves the mode. The pill and the header
 * button are the exits that always work.
 *
 * The modifier and IME guards mirror `termkeys.ts`: a modified Escape is
 * somebody else's chord, and mid-composition Escape cancels the composition.
 */
export function theaterEscapeAction(ev: TheaterEscEvent): TheaterEscapeAction {
  if (!ev.theater) return "none"
  if (ev.type !== "keydown") return "none"
  if (ev.key !== "Escape") return "none"
  if (ev.defaultPrevented) return "none"
  if (ev.ctrlKey || ev.shiftKey || ev.altKey || ev.metaKey) return "none"
  if (ev.isComposing || ev.keyCode === 229) return "none"
  if (ev.inTypingSurface) return "none"
  return "exit"
}

/**
 * Watching a pane's input ownership for the one transition theater cares about.
 * `isOwner` is a foreground guess until the server answers, so the first real
 * verdict is never a transition however far it differs from that guess.
 */
export interface TheaterOwnershipWatch {
  verdictSeen: boolean
  wasOwner: boolean
}

export const theaterOwnershipWatchStart: TheaterOwnershipWatch = {
  verdictSeen: false,
  wasOwner: false,
}

export function theaterOwnershipStep(
  prev: TheaterOwnershipWatch,
  next: { handshakeSeen: boolean; isOwner: boolean },
): { state: TheaterOwnershipWatch; lost: boolean } {
  if (!next.handshakeSeen) return { state: prev, lost: false }
  const state = { verdictSeen: true, wasOwner: next.isOwner }
  if (!prev.verdictSeen) return { state, lost: false }
  return { state, lost: prev.wasOwner && !next.isOwner }
}

/**
 * Is this element something a keystroke is being typed into? Takes a minimal
 * shape rather than an `Element` so the rule is testable without a DOM.
 * `select` counts, because an open select consumes Escape to close itself.
 */
export function isTypingSurfaceElement(
  el: { tagName?: string; isContentEditable?: boolean } | null | undefined,
): boolean {
  if (!el) return false
  if (el.isContentEditable === true) return true
  const tag = el.tagName
  return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT"
}

// The pill carries controls that act and no tab status: the agents list and the
// tab strip are where tab status lives, and a second copy of it could disagree.
// The accepted cost is that in theater on a phone, where both are off screen, a
// hidden tab needing attention has no on-screen signal until the mode is left.

/// What a shell's top chrome reads to decide whether it is on screen.
export interface TopChromeInputs {
  /// The mode itself, as the address and the store hold it.
  theater: boolean
  /// A full-pane cover (the take-over card, the Reconnect box) speaks for the
  /// pane. The transparent spinner is not one of them.
  coverOwnsPane: boolean
}

/**
 * Is a shell's top chrome hidden? Theater hides it, except while a full-pane
 * cover owns the terminal: a covered terminal is not one anybody is looking at,
 * and the pill that would carry the way out is withheld under such a cover, so
 * the top bar is the only route left.
 *
 * Both surfaces ask it. A computer is reached by width alone, so a tablet with
 * no keyboard lands there with no Escape either; only its TOP bar comes back,
 * since its side panels are not a way out of anything.
 *
 * Nothing here writes the mode: the address stays in theater throughout, and
 * the chrome leaves again by itself once the cover goes.
 */
export function topChromeHidden(input: TopChromeInputs): boolean {
  return input.theater && !input.coverOwnsPane
}

/**
 * How long the chrome takes to leave, in milliseconds. The PTY refit lands when
 * this elapses, so under reduced motion it must be zero: there is no transition
 * to wait for, and waiting would hold the terminal at the wrong grid.
 */
export const THEATER_TRANSITION_MS = 300

export function theaterTransitionMs(reducedMotion: boolean): number {
  return reducedMotion ? 0 : THEATER_TRANSITION_MS
}
