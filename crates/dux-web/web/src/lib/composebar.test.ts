import { describe, expect, it } from "vitest"

import {
  COMPOSE_SUBMIT_DELAY_MS,
  MAX_COMPOSE_SEND_BYTES,
  composeSendTooLarge,
  composeSendWrites,
} from "./composebar"

// The full write-plan matrix for the mobile compose bar's Send action. The
// helper is pure (no xterm import), so every rule the component relies on is
// pinned here without mounting anything: normalization, the empty-buffer bare
// Enter, and the bracketed-paste wrap whose submitting CR is a SEPARATE,
// delayed second write (see composeSendWrites' doc for why one write swallowed
// the Enter in Ink-based TUIs).
describe("composeSendWrites", () => {
  it("sends a plain single line with its trailing CR in one write", () => {
    expect(composeSendWrites("ls -la", { bracketedPaste: false })).toEqual([
      "ls -la\r",
    ])
  })

  it("keeps internal newlines as LF and submits with a trailing CR, one write", () => {
    // The LF bytes are the soft-newline byte (Ctrl-j) agent CLIs already treat
    // as newline-without-submit, so a multiline message lands as typed.
    expect(
      composeSendWrites("first line\nsecond line", { bracketedPaste: false }),
    ).toEqual(["first line\nsecond line\r"])
  })

  it("normalizes CRLF pairs to LF before sending", () => {
    expect(composeSendWrites("a\r\nb", { bracketedPaste: false })).toEqual([
      "a\nb\r",
    ])
  })

  it("normalizes a lone CR to LF before sending", () => {
    expect(composeSendWrites("a\rb", { bracketedPaste: false })).toEqual([
      "a\nb\r",
    ])
  })

  it("sends a bare Enter (CR) as a single write for an empty buffer", () => {
    // An empty Send is how the user confirms a TUI menu/prompt without ever
    // focusing xterm, so it must be a plain CR, not a no-op. It is a
    // KEYSTROKE, not a paste, so it never splits or delays.
    expect(composeSendWrites("", { bracketedPaste: false })).toEqual(["\r"])
  })

  it("splits a bracketed-paste send into the wrap and a separate CR write", () => {
    // The wrapped body is write one; the submitting CR is write two, which
    // the caller delivers after COMPOSE_SUBMIT_DELAY_MS. A CR inside the same
    // stdin chunk as the paste is consumed by Ink's paste handling instead of
    // acting as Enter, which is how Send "typed but did not submit" on device.
    expect(composeSendWrites("hello\nworld", { bracketedPaste: true })).toEqual([
      "\x1b[200~hello\nworld\x1b[201~",
      "\r",
    ])
  })

  it("still sends a single bare CR for an empty buffer under bracketed paste", () => {
    // There is no body to protect: no wrap, no split, no delay.
    expect(composeSendWrites("", { bracketedPaste: true })).toEqual(["\r"])
  })

  it("treats whitespace-only text as text, not as an empty buffer", () => {
    expect(composeSendWrites("  ", { bracketedPaste: false })).toEqual(["  \r"])
    expect(composeSendWrites(" ", { bracketedPaste: true })).toEqual([
      "\x1b[200~ \x1b[201~",
      "\r",
    ])
  })

  it("normalizes before the empty check, so a lone CR is a newline, not empty", () => {
    // A buffer holding only "\r" (a mobile keyboard artifact) normalizes to
    // "\n": one soft newline plus the submit, not the bare-Enter path.
    expect(composeSendWrites("\r", { bracketedPaste: false })).toEqual(["\n\r"])
  })

  it("pins the submit delay in the one-frame-ish range the split relies on", () => {
    // Long enough that Ink processes the paste chunk in an earlier input event
    // than the CR; short enough to be imperceptible next to a tap.
    expect(COMPOSE_SUBMIT_DELAY_MS).toBe(40)
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
