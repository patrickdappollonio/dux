// Pure send rules for the mobile compose bar's Send action.
//
// The compose bar lets a phone user type into a real textarea (native
// autocorrect, swipe input, local editing) and deliver the finished message to
// the PTY. This module owns what the delivery LOOKS like on the wire; the
// component (`ComposeBar.tsx`) owns the input surface, and `TerminalPane.tsx`
// owns the PTY write and its side effects. Like `termkeys.ts`, it is
// deliberately free of any xterm/React/DOM import so the whole matrix is
// unit-testable in isolation (see `composebar.test.ts`).

import { macroPayloadBytes } from "./macros"

// The carriage-return byte (CR, 0x0D), what a plain Enter sends. Interactive
// CLIs treat it as submit, which is exactly what Send means.
const CR = 0x0d

// The largest payload a compose Send will put on the wire, in bytes. The
// server enforces a 16 MiB per-frame cap (`MAX_WS_MESSAGE_SIZE` in
// `crates/dux-web/src/server.rs`) and an oversized frame ABORTS the whole PTY
// socket, so without a client-side check a giant accidental paste would kill
// the connection instead of failing one send. 2 MiB is far more than any real
// composed message while staying well under the server's limit.
export const MAX_COMPOSE_SEND_BYTES = 2 * 1024 * 1024

/**
 * True when a send totalling `payloadBytes` (the summed byte length of its
 * writes) exceeds [`MAX_COMPOSE_SEND_BYTES`]. Callers measure the built
 * bytes, so multi-byte text (CJK, emoji) counts at its real wire size.
 */
export function composeSendTooLarge(payloadBytes: number): boolean {
  return payloadBytes > MAX_COMPOSE_SEND_BYTES
}

// How long the caller waits between the body write and the submitting CR
// write of a non-empty send (see `composeSendWrites`). MEASURED, not guessed:
// the installed Claude Code 2.1.217 bundle merges stdin chunks into one paste
// through a 50ms debounce (and force-classifies any single key event over 800
// chars as a paste), so an Enter arriving with or within 50ms of the body is
// swallowed into the paste as a newline instead of submitting, intermittently,
// depending on chunk timing and length. 150ms is 3x that window with margin,
// still imperceptible next to the tap that triggered the send. This is also
// how a human behaves: paste, a beat, then Enter, as separate key events.
export const COMPOSE_SUBMIT_DELAY_MS = 150

/**
 * Builds the ordered PTY writes a compose-bar Send performs: the message as a
 * MACRO-style keystroke stream, submitted by an Enter that travels alone.
 *
 * The body is exactly [`macroPayloadBytes`] (the transform the macro
 * quick-picker already uses against these same PTYs): every newline, in any
 * of its `\r\n` / `\n` / `\r` spellings, becomes Alt+Enter (ESC CR), the
 * soft-newline keystroke agent CLIs treat as newline-without-submit. This
 * deliberately does not use bracketed paste: the keystroke stream keeps "line
 * break" and "Enter" distinct on the wire, so nothing here depends on a paste
 * protocol the app may or may not negotiate.
 *
 * Two elements mean the caller writes the body immediately and the bare-CR
 * submit after [`COMPOSE_SUBMIT_DELAY_MS`] (see that constant for the measured
 * paste-debounce reason). One element is a single immediate write: an EMPTY
 * buffer sends just the bare CR, a plain Enter, which is how the user confirms
 * a TUI menu or permission prompt without ever focusing xterm; it is a lone
 * keystroke with no body for a paste heuristic to merge it into, so it is
 * never delayed. Whitespace-only text is real text, not empty.
 */
export function composeSendWrites(text: string): Uint8Array[] {
  const body = macroPayloadBytes(text)
  const submit = new Uint8Array([CR])
  if (body.byteLength === 0) return [submit]
  return [body, submit]
}

/**
 * Splices `text` into the compose draft, returning the new draft and where the
 * caret lands (immediately after the inserted text). This is what a picked
 * macro does while the compose bar is the typing surface: the macro becomes an
 * editable draft the user reviews and Sends, never an immediate wire write.
 *
 * `selectionStart`/`selectionEnd` are the textarea's reported selection, in
 * UTF-16 code units (the same units `String.slice` uses, so no conversion). A
 * missing half (`null`) means the caret state is unavailable and the insert
 * APPENDS to the end; out-of-range values (a DOM value briefly lagging the
 * controlled state) are clamped to the draft's bounds, and a reversed
 * selection is reordered before splicing. Multi-line text is inserted
 * verbatim: the draft keeps real newlines, and only the Send path converts
 * them to newline-without-submit keystrokes on the wire.
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
 * Should the compose bar be up?
 *
 * Pure so the whole matrix is testable without mounting a terminal. The
 * capability argument is "is touch the primary pointer" (`pointer: coarse`,
 * see `hooks/use-coarse-pointer.ts`), NOT a viewport width: the bar is a
 * decision about the INPUT METHOD, and keying it to width meant rotating a
 * tablet changed the typing surface underneath the user mid-session.
 *
 * `always`/`never` exist because that capability check cannot finish the job.
 * MEASURED: an Android tablet with a physical keyboard attached and the same
 * tablet without one report identical interaction media queries, so the two
 * cases that want opposite answers are indistinguishable to the browser and
 * only the user can resolve them.
 */
export function composeBarVisible(
  mode: ComposeBarMode,
  coarsePointer: boolean
): boolean {
  switch (mode) {
    case "always":
      return true
    case "never":
      return false
    case "auto":
      return coarsePointer
  }
}

/**
 * Do the TOUCH TYPING SURFACES belong on this device at all?
 *
 * TWO ORTHOGONAL QUESTIONS, and treating them as one was the bug. WIDTH decides
 * the LAYOUT: how much room is there, so which shell you get. The POINTER
 * decides the TYPING SURFACE: is a finger doing the typing, so does the text
 * need a buffer where autocorrect and swipe have something to work with. A
 * tablet in landscape wants the desktop layout AND the buffered input, so the
 * bars travel with the pointer and render inside the desktop shell too.
 *
 * This gates the pair: the accessory keys (Esc/Tab/Ctrl/Alt and the rest) and
 * the compose bar. `never` still keeps the accessory keys on a coarse pointer,
 * because it is a preference about the compose BOX and a soft keyboard still
 * cannot produce a Ctrl chord; `always` brings the pair to a fine pointer,
 * because that is the user saying the capability answer is wrong here.
 */
export function touchSurfacesApply(
  mode: ComposeBarMode,
  coarsePointer: boolean
): boolean {
  return coarsePointer || mode === "always"
}

/**
 * Which typing surface a device-local toggle has been left on, or `null` while
 * nobody has touched it and the pointer capability answers. Persisted in
 * `localStorage` by `lib/typingSurface.ts`; deliberately NOT configuration.
 */
export type TypingSurfaceChoice = "compose" | "direct"

/**
 * Is the compose bar up, once the device-local toggle is folded in?
 *
 * The SETTING wins: `always` and `never` are what the operator wrote in config
 * and a transient toggle must never quietly defeat them. Only `auto` (which
 * MEANS "work it out") consults the choice, and there the choice replaces the
 * capability answer for exactly the case the browser cannot see: the same
 * tablet with and without a keyboard case attached.
 */
export function composeBarShown(
  mode: ComposeBarMode,
  coarsePointer: boolean,
  choice: TypingSurfaceChoice | null
): boolean {
  if (mode !== "auto") return composeBarVisible(mode, coarsePointer)
  if (choice !== null) return choice === "compose"
  return coarsePointer
}

/**
 * Should the toggle render?
 *
 * Only where it can do something: in `auto` (under always/never the setting has
 * already decided, and a control that changed nothing would be a lie) and on a
 * device that has the touch surfaces at all, since the toggle lives in the
 * accessory bar. It lives THERE rather than in the compose bar because the
 * accessory bar is present in BOTH states: a toggle inside the compose bar
 * would disappear the moment it turned the compose bar off, stranding the user
 * in direct typing with no way back.
 */
export function typingSurfaceToggleOffered(
  mode: ComposeBarMode,
  coarsePointer: boolean
): boolean {
  return mode === "auto" && touchSurfacesApply(mode, coarsePointer)
}

/**
 * The cursor style xterm should paint while it does NOT have focus.
 *
 * Verified against the installed @xterm/xterm 6.0.0: `cursorInactiveStyle`
 * accepts 'outline' | 'block' | 'bar' | 'underline' | 'none', defaults to
 * 'outline', and is not in the read-only option list (only `cols` and `rows`
 * are), so it can be reassigned on a live terminal.
 *
 * The compose bar changes what "unfocused" MEANS. Normally an unfocused
 * terminal is one you are not typing at, and every real emulator hollows the
 * caret to say so. With the compose bar up, xterm is never focused by design
 * (the textarea holds focus for the whole session) while the prompt on screen
 * is the live one, so the outline states something false all the time. That
 * mode gets the solid block; direct typing keeps the convention, because there
 * the unfocused caret means exactly what it says.
 */
export function inactiveCursorStyle(
  composeBarActive: boolean
): "block" | "outline" {
  return composeBarActive ? "block" : "outline"
}
