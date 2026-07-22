// Pure payload rules for the mobile compose bar's Send action.
//
// The compose bar lets a phone user type into a real textarea (native
// autocorrect, swipe input, local editing) and deliver the finished message to
// the PTY in one write. This module owns what that write LOOKS like; the
// component (`ComposeBar.tsx`) owns the buffer, and `TerminalPane.tsx` owns the
// PTY write and its side effects. Like `termkeys.ts`, it is deliberately free of
// any xterm/React/DOM import so the whole payload matrix is unit-testable in
// isolation (see `composebar.test.ts`).

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

// The largest payload a compose Send will put on the wire, in encoded UTF-8
// bytes. The server enforces a 16 MiB per-frame cap (`MAX_WS_MESSAGE_SIZE` in
// `crates/dux-web/src/server.rs`) and an oversized frame ABORTS the whole PTY
// socket, so without a client-side check a giant accidental paste would kill
// the connection instead of failing one send. 2 MiB is far more than any real
// composed message while staying well under the server's limit.
export const MAX_COMPOSE_SEND_BYTES = 2 * 1024 * 1024

/**
 * True when `payload` (the already-built send payload) exceeds
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
 * Builds the byte string a compose-bar Send writes to the PTY.
 *
 * - Line endings are normalized first: mobile keyboards can emit `\r\n` or a
 *   lone `\r`, and both must become the LF soft-newline byte before any other
 *   rule looks at the text.
 * - An EMPTY buffer sends a bare CR, a plain Enter. This is deliberate: it is
 *   how the user confirms a TUI menu or permission prompt without ever focusing
 *   xterm's hidden textarea, so Send stays enabled on an empty buffer.
 *   Whitespace-only text is real text, not empty.
 * - With bracketed paste active, the body is wrapped in the paste markers and
 *   the submitting CR goes OUTSIDE the wrap (a CR inside would be pasted as
 *   text, not treated as Enter).
 * - Without it, the body goes as-is (internal LFs are the soft-newline byte
 *   agent CLIs already treat as Ctrl-j) followed by the submitting CR.
 */
export function composeSendPayload(
  text: string,
  opts: ComposeSendOptions,
): string {
  const body = text.replace(/\r\n/g, LF).replace(/\r/g, LF)
  if (body === "") return CR
  if (opts.bracketedPaste) {
    return `${BRACKETED_PASTE_START}${body}${BRACKETED_PASTE_END}${CR}`
  }
  return body + CR
}
