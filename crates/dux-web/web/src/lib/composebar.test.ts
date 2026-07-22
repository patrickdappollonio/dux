import { describe, expect, it } from "vitest"

import {
  COMPOSE_SUBMIT_DELAY_MS,
  MAX_COMPOSE_SEND_BYTES,
  composeSendTooLarge,
  composeSendWrites,
} from "./composebar"

// The full write-plan matrix for the mobile compose bar's Send action. The
// helper is pure (no xterm import), so every rule the component relies on is
// pinned here without mounting anything. A Send is a MACRO-style keystroke
// stream, not a paste: newlines become Alt+Enter (ESC CR, the soft-newline
// keystroke agent CLIs treat as newline-without-submit, exactly like
// `macroPayloadBytes`) and the submitting Enter is a SEPARATE second write the
// caller delivers after COMPOSE_SUBMIT_DELAY_MS. The split exists because
// Claude Code merges stdin chunks into one paste through a 50ms debounce
// (measured in the installed 2.1.217 bundle), so an Enter riding with or right
// after the body is swallowed into the paste as a newline instead of
// submitting, intermittently, depending on chunk timing and length.
describe("composeSendWrites", () => {
  const str = (b: Uint8Array) => {
    let out = ""
    for (const x of b) out += String.fromCharCode(x)
    return out
  }
  const plan = (text: string) => composeSendWrites(text).map(str)

  it("a plain single line is the body write, then the CR write", () => {
    expect(plan("ls -la")).toEqual(["ls -la", "\r"])
  })

  it("turns internal newlines into Alt+Enter keystrokes in the body write", () => {
    // ESC CR per newline (the macro convention): line break and Enter are
    // distinct keystrokes on the wire.
    expect(plan("first\nsecond")).toEqual(["first\x1b\rsecond", "\r"])
  })

  it("normalizes a CRLF pair to a single Alt+Enter", () => {
    expect(plan("a\r\nb")).toEqual(["a\x1b\rb", "\r"])
  })

  it("normalizes a lone CR to a single Alt+Enter", () => {
    expect(plan("a\rb")).toEqual(["a\x1b\rb", "\r"])
  })

  it("an empty buffer is ONE immediate bare-CR write, never split", () => {
    // An empty Send is how the user confirms a TUI menu/prompt without ever
    // focusing xterm. It is a lone Enter keystroke: there is no body for a
    // paste heuristic to merge it into, so no delay applies.
    expect(plan("")).toEqual(["\r"])
  })

  it("treats whitespace-only text as text, not as an empty buffer", () => {
    expect(plan("  ")).toEqual(["  ", "\r"])
  })

  it("a buffer holding only a newline is an Alt+Enter body plus the submit", () => {
    expect(plan("\n")).toEqual(["\x1b\r", "\r"])
  })

  it("passes multi-byte UTF-8 through untouched", () => {
    const writes = composeSendWrites("中\n中")
    const expected = new TextEncoder().encode("中\x1b\r中")
    expect(Array.from(writes[0])).toEqual(Array.from(expected))
    expect(Array.from(writes[1])).toEqual([0x0d])
  })

  it("pins the submit delay comfortably above Claude Code's 50ms paste debounce", () => {
    // Measured against the installed Claude Code 2.1.217 bundle: stdin chunks
    // are merged into one paste through a 50ms debounce, and a CR arriving
    // inside that window is swallowed into the paste as a newline instead of
    // submitting. 150ms is 3x that window with margin, still imperceptible
    // next to the tap that triggered the send.
    expect(COMPOSE_SUBMIT_DELAY_MS).toBe(150)
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
    expect(composeSendTooLarge(MAX_COMPOSE_SEND_BYTES)).toBe(false)
  })

  it("rejects a payload one byte over the cap", () => {
    expect(composeSendTooLarge(MAX_COMPOSE_SEND_BYTES + 1)).toBe(true)
  })
})
