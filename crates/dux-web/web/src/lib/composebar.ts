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
