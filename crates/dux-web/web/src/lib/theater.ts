// THEATER MODE: the one-pane, no-chrome layout for a terminal surface.
//
// The pure decisions live here, away from the components and the store, because
// every one of them is a rule rather than a rendering: which key a pane's
// memory is filed under, whether an Escape belongs to dux or to the child, and
// how the mode rides the address. The store owns the live flag and the URL
// write; the components own the boxes that move.
//
// Theater is deliberately NOT the browser's Fullscreen API. The tenet is
// explicit about why (Android hard-clips a fullscreen page against the
// keyboard-shrunk viewport), and the pixels being reclaimed are dux's own
// chrome, which is not something a system mode could give back anyway.

import type { SelectedTarget } from "./store"

/** The `localStorage` key prefix every pane's theater memory is filed under. */
export const THEATER_STORAGE_PREFIX = "dux:theater:"

/**
 * The key a pane's theater memory lives at, or `null` when nothing is focused.
 *
 * Keyed on the STABLE id of the thing on screen: an agent's tab id (never its
 * session id, since the mode is remembered per tab and two tabs of one agent
 * must be able to disagree) and a terminal's own id (whatever owns it, since
 * ownership never changes after spawn and the id alone names the pane).
 */
export function theaterMemoryKey(target: SelectedTarget | null): string | null {
  if (!target) return null
  return target.kind === "agent"
    ? theaterMemoryKeyForPty("agent", target.tabId)
    : theaterMemoryKeyForPty("terminal", target.terminalId)
}

/**
 * The same key from the two things a mounted pane holds: which kind it is and
 * the id of the PTY it drives. The pane learns it lost input ownership without
 * ever holding a `SelectedTarget`, and that is the one caller that must be able
 * to name a pane's memory without one.
 */
export function theaterMemoryKeyForPty(
  kind: "agent" | "terminal",
  id: string,
): string {
  return `${THEATER_STORAGE_PREFIX}${kind}:${id}`
}

// Storage can be missing (a test that never stubbed it) or throw outright
// (Safari private mode, a browser set to block site data), and neither is a
// reason for a pane to fail to render. Both degrade to "not in theater", which
// is the state every pane starts in anyway. Same shape as `typingSurface.ts`.
function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage
  } catch {
    return null
  }
}

/** Was this pane last left in theater? Anything unreadable reads as "no". */
export function readTheaterMemory(key: string | null): boolean {
  if (!key) return false
  try {
    return storage()?.getItem(key) === "on"
  } catch {
    return false
  }
}

/** Remember (or forget) that this pane is in theater. Best-effort. */
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
 * Forget a pane's theater memory outright.
 *
 * The one non-user caller is losing input ownership of the pane: after another
 * device takes over, the full-chrome state is where the user comes back to and
 * re-entering theater is a fresh choice.
 */
export function clearTheaterMemory(key: string | null): void {
  writeTheaterMemory(key, false)
}

/**
 * How theater rides the address: a modifier on the position, appended after the
 * position's own grammar rather than woven into it, so every existing shape
 * (agent, tab, companion terminal, project terminal, standalone terminal) gains
 * it without a second parser.
 */
export const THEATER_QUERY = "?view=theater"

/** Split the modifier off a hash, returning the bare position and the flag. */
export function splitTheaterHash(hash: string): {
  hash: string
  theater: boolean
} {
  return hash.endsWith(THEATER_QUERY)
    ? { hash: hash.slice(0, -THEATER_QUERY.length), theater: true }
    : { hash, theater: false }
}

/**
 * Put the modifier back on. Home (the empty hash) never carries it: there is no
 * pane there to be in theater, and a bare `#?view=theater` would be a position
 * that names nothing.
 */
export function withTheaterHash(hash: string, theater: boolean): string {
  return theater && hash !== "" ? hash + THEATER_QUERY : hash
}

/**
 * Whether a route may carry the modifier at all. Theater is a modifier on a
 * FOCUSED PANE, so it is dropped for home, for the changes screen and for every
 * editor address: those surfaces have no PTY to give the height to, and an
 * address that claimed otherwise would restore a mode nothing can honour.
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
 * It is the exit ONLY when focus is nowhere typeable. A hardware Escape over a
 * focused compose box is already forwarded to the PTY
 * (`composeHardwareKeyForwards`), and the accessory bar's Esc key is its
 * physical twin; over a focused xterm it is the child's Escape outright.
 * Stealing either would make Escape mean two different things depending on a
 * mode the user cannot see, and interrupting a running agent is the more
 * expensive of the two mistakes. So Escape is the exit only where nothing else
 * wants it, and the pill and the header button are the exits that always work.
 *
 * SOMETHING ELSE ALREADY WANTS IT. An open menu, popover or dialog answers
 * Escape by closing, and Base UI's dismiss hook marks that answer by calling
 * `preventDefault` on the very keydown this rule reads; abstaining on a
 * prevented event is what keeps one press from closing a menu AND leaving the
 * mode.
 *
 * The modifier and IME guards mirror the ones in `termkeys.ts` for the same
 * reasons: a modified Escape is somebody else's chord, and mid-composition
 * Escape means cancel the composition.
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
 *
 * `isOwner` starts as a FOREGROUND GUESS and is only honest once the server has
 * answered, so the first real verdict is not a transition however it differs
 * from the guess. Without that gate a watcher opening a shared theater link
 * entered the mode, immediately "lost" ownership it never had, and cleared the
 * pane's memory on its way out.
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
 * Is this element something a keystroke is being typed into?
 *
 * Takes the minimal shape rather than an `Element` so the rule is testable
 * without a DOM. `select` is in the list because an open select consumes
 * Escape to close itself.
 */
export function isTypingSurfaceElement(
  el: { tagName?: string; isContentEditable?: boolean } | null | undefined,
): boolean {
  if (!el) return false
  if (el.isContentEditable === true) return true
  const tag = el.tagName
  return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT"
}

// THE PILL CARRIES NO TAB STATUS, deliberately.
//
// It used to grow a status half that bobbed while a hidden tab worked, wore an
// attention dot, and folded out a mini strip of the same tab pills to switch
// between them. All three came out: the agents list and the tab strip are where
// tab status lives, and a second, smaller copy of it floating over the terminal
// was a place for the two to disagree. What is left is four controls that ACT,
// which is what a floating cluster is for.
//
// THE COST, WRITTEN DOWN RATHER THAN WISHED AWAY: in theater on a phone both of
// those surfaces are off screen, so a hidden tab that needs attention has no
// on-screen signal until the mode is left. Nothing raises a notification for it
// either (the browser ones are the agent's own escape sequences, delivered by a
// mounted terminal to a page that is not being looked at). That is accepted for
// a mode whose whole purpose is one terminal and nothing else.

/**
 * How long the chrome takes to leave, in milliseconds.
 *
 * The single PTY refit lands when this elapses, so it is also how long the
 * gesture is. Under reduced motion the whole thing is a cut and the refit is
 * immediate: there is no transition to wait for, and waiting anyway would leave
 * the terminal at the wrong grid for a third of a second for no reason.
 */
export const THEATER_TRANSITION_MS = 300

export function theaterTransitionMs(reducedMotion: boolean): number {
  return reducedMotion ? 0 : THEATER_TRANSITION_MS
}
