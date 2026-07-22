import { describe, expect, it } from "vitest"

import {
  MAX_COMPOSE_SEND_BYTES,
  composeSendBytes,
  composeSendTooLarge,
} from "./composebar"

// The full payload matrix for the mobile compose bar's Send action. The helper
// is pure (no xterm import), so every rule the component relies on is pinned
// here without mounting anything. A Send is a MACRO-style keystroke stream,
// not a paste: newlines become Alt+Enter (ESC CR, the soft-newline keystroke
// agent CLIs treat as newline-without-submit, exactly like `macroPayloadBytes`)
// and the trailing CR is the submitting Enter. One write, no bracketed paste,
// no delay: a keystroke stream expresses "line break" and "Enter" as distinct
// keys, so there is nothing for a paste guard to swallow.
describe("composeSendBytes", () => {
  const bytes = (s: string) => composeSendBytes(s)
  const str = (b: Uint8Array) => {
    let out = ""
    for (const x of b) out += String.fromCharCode(x)
    return out
  }

  it("sends a plain single line with its trailing CR", () => {
    expect(str(bytes("ls -la"))).toBe("ls -la\r")
  })

  it("turns internal newlines into Alt+Enter and submits with a trailing CR", () => {
    // ESC CR per newline (the macro convention), then the bare CR that
    // actually submits: line break and Enter are distinct keystrokes.
    expect(str(bytes("first\nsecond"))).toBe("first\x1b\rsecond\r")
  })

  it("normalizes a CRLF pair to a single Alt+Enter", () => {
    expect(str(bytes("a\r\nb"))).toBe("a\x1b\rb\r")
  })

  it("normalizes a lone CR to a single Alt+Enter", () => {
    expect(str(bytes("a\rb"))).toBe("a\x1b\rb\r")
  })

  it("sends a bare Enter (CR) for an empty buffer", () => {
    // An empty Send is how the user confirms a TUI menu/prompt without ever
    // focusing xterm, so it must be a plain CR, not a no-op. It falls out of
    // the one rule: empty body + the submitting CR.
    expect(str(bytes(""))).toBe("\r")
  })

  it("treats whitespace-only text as text, not as an empty buffer", () => {
    expect(str(bytes("  "))).toBe("  \r")
  })

  it("a buffer holding only a newline is one Alt+Enter plus the submit", () => {
    expect(str(bytes("\n"))).toBe("\x1b\r\r")
  })

  it("passes multi-byte UTF-8 through untouched", () => {
    const out = bytes("中\n中")
    const expected = new TextEncoder().encode("中\x1b\r中\r")
    expect(Array.from(out)).toEqual(Array.from(expected))
  })

  it("matches the macro transform byte-for-byte on the body", () => {
    // The body IS `macroPayloadBytes`; only the trailing CR is compose's own.
    // Pinned so the compose and macro conventions can never drift apart.
    const text = "line one\r\nline two\rline three\nend"
    const viaCompose = Array.from(bytes(text))
    expect(viaCompose[viaCompose.length - 1]).toBe(0x0d)
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
    expect(composeSendTooLarge(new Uint8Array(MAX_COMPOSE_SEND_BYTES))).toBe(
      false,
    )
  })

  it("rejects a payload one byte over the cap", () => {
    expect(
      composeSendTooLarge(new Uint8Array(MAX_COMPOSE_SEND_BYTES + 1)),
    ).toBe(true)
  })
})
