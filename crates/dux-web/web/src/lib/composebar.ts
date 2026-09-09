// Pure send rules for the compose bar's Send action, and the rules deciding
// which typing surface a pane gets. This module owns what a send looks like on
// the wire; `ComposeBar.tsx` owns the input surface and `TerminalPane.tsx` owns
// the PTY write.

import { macroPayloadBytes } from "./macros"

// The carriage-return byte (CR, 0x0D), what a plain Enter sends. Interactive
// CLIs treat it as submit, which is exactly what Send means.
const CR = 0x0d

// The largest payload a compose Send will put on the wire, in bytes. Must stay
// under the server's per-frame cap (`MAX_WS_MESSAGE_SIZE` in
// `crates/dux-web/src/server.rs`): an oversized frame aborts the whole PTY
// socket, so a giant accidental paste would kill the connection.
export const MAX_COMPOSE_SEND_BYTES = 2 * 1024 * 1024

/**
 * True when a send totalling `payloadBytes` (the summed byte length of its
 * writes) exceeds [`MAX_COMPOSE_SEND_BYTES`]. Callers measure the built
 * bytes, so multi-byte text (CJK, emoji) counts at its real wire size.
 */
export function composeSendTooLarge(payloadBytes: number): boolean {
  return payloadBytes > MAX_COMPOSE_SEND_BYTES
}

// How long the caller waits between the body write and the submitting CR write
// of a non-empty send. Must clear a receiving CLI's paste debounce, measured at
// 50ms for Claude Code, or the Enter is merged into the paste as a newline and
// never submits.
export const COMPOSE_SUBMIT_DELAY_MS = 150

/**
 * The ordered PTY writes a compose-bar Send performs: the message as a
 * macro-style keystroke stream through [`macroPayloadBytes`], submitted by an
 * Enter that travels alone. Deliberately not bracketed paste, so the wire keeps
 * "line break" and "Enter" distinct without negotiating a paste protocol.
 *
 * Two elements mean the caller writes the body immediately and the bare-CR
 * submit after [`COMPOSE_SUBMIT_DELAY_MS`]. One element is a single immediate
 * write: an empty buffer sends a bare CR, which confirms a TUI menu without
 * focusing xterm and needs no delay. Whitespace-only text is real text.
 */
export function composeSendWrites(text: string): Uint8Array[] {
  const body = macroPayloadBytes(text)
  const submit = new Uint8Array([CR])
  if (body.byteLength === 0) return [submit]
  return [body, submit]
}

/**
 * Splices `text` into the compose draft, returning the new draft and the caret
 * position after the inserted text. A macro picked while the compose bar is up
 * becomes an editable draft rather than an immediate wire write.
 *
 * `selectionStart`/`selectionEnd` are the textarea's selection in UTF-16 code
 * units. A `null` half appends to the end, out-of-range values are clamped and
 * a reversed selection is reordered. Multi-line text is inserted verbatim: only
 * the Send path converts newlines to keystrokes.
 */
export function insertIntoComposeDraft(
  draft: string,
  selectionStart: number | null,
  selectionEnd: number | null,
  text: string,
): { next: string; caret: number } {
  let start: number
  let end: number
  if (selectionStart === null || selectionEnd === null) {
    start = draft.length
    end = draft.length
  } else {
    start = Math.min(Math.max(selectionStart, 0), draft.length)
    end = Math.min(Math.max(selectionEnd, 0), draft.length)
    if (start > end) [start, end] = [end, start]
  }
  const next = draft.slice(0, start) + text + draft.slice(end)
  return { next, caret: start + text.length }
}

/**
 * The three values `ui.compose_bar` can take, mirroring
 * `dux_core::config::ComposeBarMode`.
 */
export type ComposeBarMode = "auto" | "always" | "never"

/** Narrow whatever the bootstrap document carried into a mode we have a case
 * for. An absent field (an older server) and an unrecognized one both read as
 * `"auto"`, matching the server-side warn-and-degrade fallback. */
export function composeBarMode(raw: string | undefined): ComposeBarMode {
  return raw === "always" || raw === "never" || raw === "auto" ? raw : "auto"
}

/**
 * Which typing surface a device-local toggle has been left on, or `null` while
 * nobody has touched it and the pointer capability answers. Persisted in
 * `localStorage` by `lib/typingSurface.ts`; deliberately not configuration.
 */
export type TypingSurfaceChoice = "compose" | "direct"

/**
 * What the pointer says the typing surface should be, and nothing more. A finger
 * wants a buffer where autocorrect, swipe and IMEs have something to work with.
 * Only ever a default: a tablet with and without a keyboard case report
 * identical interaction media queries, so the guess loses to the person.
 */
export function detectedTypingSurface(
  coarsePointer: boolean
): TypingSurfaceChoice {
  return coarsePointer ? "compose" : "direct"
}

/**
 * The one resolved answer to "where does typing go in this pane".
 *
 * `always`/`never` are configuration and win outright, so a per-device toggle
 * cannot defeat what the operator set up. Under `auto` the person comes first
 * and the browser second: an explicit choice, else the pointer's default. The
 * choice wins in both directions on every device and persists until the user
 * changes it; a change of pointer moves only the default.
 */
export function resolvedTypingSurface(
  mode: ComposeBarMode,
  coarsePointer: boolean,
  choice: TypingSurfaceChoice | null
): TypingSurfaceChoice {
  switch (mode) {
    case "always":
      return "compose"
    case "never":
      return "direct"
    case "auto":
      return choice ?? detectedTypingSurface(coarsePointer)
  }
}

/**
 * Is the buffered message box the typing surface right now? The capability
 * argument is "is touch the primary pointer" (`pointer: coarse`, see
 * `hooks/use-coarse-pointer.ts`) and never a viewport width: the bar is a
 * decision about the input method, so rotating a tablet must not change it.
 */
export function composeBarShown(
  mode: ComposeBarMode,
  coarsePointer: boolean,
  choice: TypingSurfaceChoice | null
): boolean {
  return resolvedTypingSurface(mode, coarsePointer, choice) === "compose"
}

/**
 * Do the terminal keys belong under this terminal at all?
 *
 * The message box and the key row are independent surfaces: the box is where
 * text is composed, the row is keys a soft keyboard cannot produce (Esc, Tab, a
 * Ctrl chord, a soft newline). So a coarse pointer always has the row on offer,
 * whatever the typing surface resolves to, while a fine pointer gets it only
 * once the box is up, since a laptop already has those keys.
 *
 * Eligibility, not the answer: the row is also subject to its own
 * Hide-terminal-keys preference and to owning the input.
 */
export function terminalKeysApply(
  mode: ComposeBarMode,
  coarsePointer: boolean,
  choice: TypingSurfaceChoice | null
): boolean {
  return coarsePointer || composeBarShown(mode, coarsePointer, choice)
}

/**
 * Would anything still be under the terminal after a switch to direct typing?
 * The one-time hint naming where the way back went fires only when nothing is,
 * since a surviving key row carries it visibly. Asked of the surface after the
 * flip, which is why the choice is pinned to `direct` rather than passed in.
 */
export function bottomBarSurvivesDirect(
  mode: ComposeBarMode,
  coarsePointer: boolean,
  keysVisible: boolean
): boolean {
  return keysVisible && terminalKeysApply(mode, coarsePointer, "direct")
}

/**
 * Should the input menu's typing-surface item render? Under `auto`, always, on
 * every device, because the menu is the guaranteed way in and out of the virtual
 * input and both the key row and the compose bar can be absent. Never under
 * `always`/`never`, where the setting has decided.
 */
export function inputMenuSurfaceSwitchOffered(mode: ComposeBarMode): boolean {
  return mode === "auto"
}

/**
 * The cursor style xterm paints while it does not have focus.
 *
 * The compose bar changes what "unfocused" means: xterm is never focused by
 * design while the textarea holds focus, so the conventional hollow caret would
 * state something false for the whole session. That mode gets the solid block;
 * direct typing keeps the convention.
 */
export function inactiveCursorStyle(
  composeBarActive: boolean
): "block" | "outline" {
  return composeBarActive ? "block" : "outline"
}
