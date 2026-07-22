// Pure send rules for the mobile compose bar's Send action.
//
// The compose bar lets a phone user type into a real textarea (native
// autocorrect, swipe input, local editing) and deliver the finished message to
// the PTY. This module owns what the delivery LOOKS like on the wire; the
// component (`ComposeBar.tsx`) owns the input surface, and `TerminalPane.tsx`
// owns the PTY writes and their side effects. Like `termkeys.ts`, it is
// deliberately free of any xterm/React/DOM import so the whole matrix is
// unit-testable in isolation (see `composebar.test.ts`).

import { ESC, LF } from "./termkeys"

// The carriage-return byte (CR, 0x0D), what a plain Enter sends. Interactive
// CLIs treat it as submit, which is exactly what Send means; the message body's
// internal newlines stay LF (the Ctrl-j soft-newline byte, see `LF` in
// termkeys.ts) so they never submit early.
const CR = "\r"

// Bracketed-paste markers (DECSET 2004). When the app in the PTY has enabled
// bracketed paste, wrapping the body tells it "this is one pasted block": the
// app then treats internal newlines as literal text instead of interpreting
// each one as a keystroke. Agent CLIs and modern shells all negotiate this.
const BRACKETED_PASTE_START = `${ESC}[200~`
const BRACKETED_PASTE_END = `${ESC}[201~`

// How long the caller waits between the wrapped-body write and the submitting
// CR write of a split bracketed-paste send (see `composeSendWrites`). One
// frame-ish: long enough that the terminal app reads the paste chunk in an
// EARLIER stdin/input event than the CR (Ink batches everything that arrives
// in one chunk into one input event), short enough to be imperceptible next
// to the tap that triggered it.
export const COMPOSE_SUBMIT_DELAY_MS = 40

// The largest total payload a compose Send will put on the wire, in encoded
// UTF-8 bytes across all of its writes. The server enforces a 16 MiB per-frame
// cap (`MAX_WS_MESSAGE_SIZE` in `crates/dux-web/src/server.rs`) and an
// oversized frame ABORTS the whole PTY socket, so without a client-side check
// a giant accidental paste would kill the connection instead of failing one
// send. 2 MiB is far more than any real composed message while staying well
// under the server's limit.
export const MAX_COMPOSE_SEND_BYTES = 2 * 1024 * 1024

/**
 * True when `payload` (the concatenation of the send's writes) exceeds
 * [`MAX_COMPOSE_SEND_BYTES`] once UTF-8 encoded. Measured in BYTES, not
 * characters: multi-byte text (CJK, emoji) reaches the cap in fewer characters.
 */
export function composeSendTooLarge(payload: string): boolean {
  return new TextEncoder().encode(payload).byteLength > MAX_COMPOSE_SEND_BYTES
}

export interface ComposeSendOptions {
  /** The terminal's live `term.modes.bracketedPasteMode`, read at send time. */
  bracketedPaste: boolean
}

/**
 * Builds the ordered PTY writes a compose-bar Send performs. One element is a
 * single immediate write; two elements mean the caller writes the first
 * immediately and the second after [`COMPOSE_SUBMIT_DELAY_MS`].
 *
 * - Line endings are normalized first: mobile keyboards can emit `\r\n` or a
 *   lone `\r`, and both must become the LF soft-newline byte before any other
 *   rule looks at the text.
 * - An EMPTY buffer is one write of a bare CR, a plain Enter. This is
 *   deliberate: it is how the user confirms a TUI menu or permission prompt
 *   without ever focusing xterm's hidden textarea, so Send stays enabled on an
 *   empty buffer. It is a keystroke, not a paste: never wrapped, never split.
 *   Whitespace-only text is real text, not empty.
 * - With bracketed paste active, the wrapped body and the submitting CR are
 *   SEPARATE writes. Ink-based TUIs (Claude Code and friends) process an
 *   entire stdin chunk as ONE input event, and a CR riding in the same chunk
 *   as the paste is consumed by the paste handling instead of acting as
 *   Enter; on device the message was typed into the prompt but never
 *   submitted. A human pasting and then pressing Enter always produces two
 *   writes with a gap, so the split (plus the delay the caller applies)
 *   restores the shape those TUIs actually handle. (A CR inside the wrap is
 *   no alternative: it would be pasted as text, not treated as Enter.)
 * - Without bracketed paste, the body (internal LFs are the soft-newline byte
 *   agent CLIs treat as Ctrl-j) and the trailing CR go as one write; readline
 *   and friends process byte streams, not batched paste events, and show no
 *   evidence of the swallowed-Enter problem.
 */
export function composeSendWrites(
  text: string,
  opts: ComposeSendOptions,
): string[] {
  const body = text.replace(/\r\n/g, LF).replace(/\r/g, LF)
  if (body === "") return [CR]
  if (opts.bracketedPaste) {
    return [`${BRACKETED_PASTE_START}${body}${BRACKETED_PASTE_END}`, CR]
  }
  return [body + CR]
}
