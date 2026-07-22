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
 * True when `payload` (the built send bytes) exceeds
 * [`MAX_COMPOSE_SEND_BYTES`]. Measured on the final bytes, so multi-byte text
 * (CJK, emoji) counts at its real wire size.
 */
export function composeSendTooLarge(payload: Uint8Array): boolean {
  return payload.byteLength > MAX_COMPOSE_SEND_BYTES
}

/**
 * Builds the single PTY write a compose-bar Send performs: the message as a
 * MACRO-style keystroke stream, submitted by a trailing Enter.
 *
 * The body is exactly [`macroPayloadBytes`] (the transform the macro
 * quick-picker already uses against these same PTYs): every newline, in any
 * of its `\r\n` / `\n` / `\r` spellings, becomes Alt+Enter (ESC CR), the
 * soft-newline keystroke agent CLIs treat as newline-without-submit. The one
 * byte compose adds is the trailing bare CR, the submitting Enter.
 *
 * Because the payload is a keystroke stream, "line break" and "Enter" are
 * DISTINCT keys on the wire and everything goes as one immediate write. This
 * deliberately does not use bracketed paste: wrapping the body as a paste made
 * Ink-based TUIs (Claude Code and friends) consume a same-chunk CR inside
 * their paste handling, so Send typed the message but never submitted it, and
 * working around that needed a delayed second write. The macro convention has
 * neither problem.
 *
 * An EMPTY buffer therefore sends a bare CR, a plain Enter, with no special
 * case: it is how the user confirms a TUI menu or permission prompt without
 * ever focusing xterm, so Send stays enabled on an empty buffer.
 * Whitespace-only text is real text, not empty.
 */
export function composeSendBytes(text: string): Uint8Array {
  const body = macroPayloadBytes(text)
  const out = new Uint8Array(body.byteLength + 1)
  out.set(body, 0)
  out[body.byteLength] = CR
  return out
}
