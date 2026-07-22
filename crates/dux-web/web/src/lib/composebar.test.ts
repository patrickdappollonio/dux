import { describe, expect, it } from "vitest"

import {
  MAX_COMPOSE_SEND_BYTES,
  composeSendPayload,
  composeSendTooLarge,
} from "./composebar"

// The full payload matrix for the mobile compose bar's Send action. The helper
// is pure (no xterm import), so every rule the component relies on is pinned
// here without mounting anything: normalization, the empty-buffer bare Enter,
// and the bracketed-paste wrap with the submitting CR OUTSIDE the wrap.
describe("composeSendPayload", () => {
  it("appends a trailing CR to a plain single line", () => {
    expect(composeSendPayload("ls -la", { bracketedPaste: false })).toBe(
      "ls -la\r",
    )
  })

  it("keeps internal newlines as LF and submits with a trailing CR", () => {
    // The LF bytes are the soft-newline byte (Ctrl-j) agent CLIs already treat
    // as newline-without-submit, so a multiline message lands as typed.
    expect(
      composeSendPayload("first line\nsecond line", { bracketedPaste: false }),
    ).toBe("first line\nsecond line\r")
  })

  it("normalizes CRLF pairs to LF before sending", () => {
    expect(composeSendPayload("a\r\nb", { bracketedPaste: false })).toBe(
      "a\nb\r",
    )
  })

  it("normalizes a lone CR to LF before sending", () => {
    expect(composeSendPayload("a\rb", { bracketedPaste: false })).toBe("a\nb\r")
  })

  it("sends a bare Enter (CR) for an empty buffer", () => {
    // An empty Send is how the user confirms a TUI menu/prompt without ever
    // focusing xterm, so it must be a plain CR, not a no-op.
    expect(composeSendPayload("", { bracketedPaste: false })).toBe("\r")
  })

  it("wraps the body in bracketed-paste markers with the CR outside the wrap", () => {
    expect(composeSendPayload("hello\nworld", { bracketedPaste: true })).toBe(
      "\x1b[200~hello\nworld\x1b[201~\r",
    )
  })

  it("still sends a bare CR for an empty buffer under bracketed paste", () => {
    // There is no body to protect, and a wrapped empty paste would make some
    // CLIs treat the following CR as paste-adjacent instead of a submit.
    expect(composeSendPayload("", { bracketedPaste: true })).toBe("\r")
  })

  it("treats whitespace-only text as text, not as an empty buffer", () => {
    expect(composeSendPayload("  ", { bracketedPaste: false })).toBe("  \r")
    expect(composeSendPayload(" ", { bracketedPaste: true })).toBe(
      "\x1b[200~ \x1b[201~\r",
    )
  })

  it("normalizes before the empty check, so a lone CR is a newline, not empty", () => {
    // A buffer holding only "\r" (a mobile keyboard artifact) normalizes to
    // "\n": one soft newline plus the submit, not the bare-Enter path.
    expect(composeSendPayload("\r", { bracketedPaste: false })).toBe("\n\r")
  })
})

// The client-side size cap on a compose Send. The server aborts the whole PTY
// socket on an oversized frame (16 MiB `MAX_WS_MESSAGE_SIZE`), so an unchecked
// giant paste would kill the connection instead of failing one send; the cap
// rejects it client-side with the buffer kept.
describe("composeSendTooLarge", () => {
  it("pins the cap at 2 MiB, well under the server's 16 MiB frame limit", () => {
    expect(MAX_COMPOSE_SEND_BYTES).toBe(2 * 1024 * 1024)
  })

  it("accepts a payload exactly at the cap", () => {
    expect(composeSendTooLarge("a".repeat(MAX_COMPOSE_SEND_BYTES))).toBe(false)
  })

  it("rejects a payload one byte over the cap", () => {
    expect(composeSendTooLarge("a".repeat(MAX_COMPOSE_SEND_BYTES + 1))).toBe(
      true,
    )
  })

  it("measures encoded BYTES, not characters (multi-byte UTF-8 counts fully)", () => {
    // U+4E2D is three UTF-8 bytes: a third of the cap in characters already
    // exceeds it in bytes.
    const cjk = "中".repeat(Math.ceil(MAX_COMPOSE_SEND_BYTES / 3) + 1)
    expect(composeSendTooLarge(cjk)).toBe(true)
  })
})
