import { describe, expect, it } from "vitest"

import { createSgrColonNormalizer } from "./sgrColonForm"

const enc = new TextEncoder()
const dec = new TextDecoder()

/// Feed one whole string through a fresh normaliser and read back what came out.
function once(input: string): string {
  const n = createSgrColonNormalizer()
  return dec.decode(n.push(enc.encode(input)))
}

/// Feed the same string in two pieces, split at `at`, and concatenate the output.
function split(input: string, at: number): string {
  const n = createSgrColonNormalizer()
  const bytes = enc.encode(input)
  const a = n.push(bytes.subarray(0, at))
  const b = n.push(bytes.subarray(at))
  const out = new Uint8Array(a.length + b.length)
  out.set(a, 0)
  out.set(b, a.length)
  return dec.decode(out)
}

describe("the three-sub-parameter colon form gains its empty colour-space slot", () => {
  it("rewrites a background teal, the exact bytes the bug was measured with", () => {
    expect(once("\x1b[48:2:0:128:128m")).toBe("\x1b[48:2::0:128:128m")
  })

  it("rewrites a foreground colour", () => {
    expect(once("\x1b[38:2:255:255:255m")).toBe("\x1b[38:2::255:255:255m")
  })

  it("rewrites an underline colour, which has the same shape", () => {
    expect(once("\x1b[58:2:10:20:30m")).toBe("\x1b[58:2::10:20:30m")
  })

  it("rewrites the affected parameter only, leaving its neighbours alone", () => {
    expect(once("\x1b[0;1;38:2:1:2:3;4m")).toBe("\x1b[0;1;38:2::1:2:3;4m")
  })

  it("rewrites every affected parameter in one sequence", () => {
    expect(once("\x1b[38:2:1:2:3;48:2:4:5:6m")).toBe(
      "\x1b[38:2::1:2:3;48:2::4:5:6m",
    )
  })

  it("keeps the text around the sequence byte for byte", () => {
    expect(once("hi \x1b[48:2:0:128:128mthere")).toBe(
      "hi \x1b[48:2::0:128:128mthere",
    )
  })
})

describe("everything else passes through untouched", () => {
  const untouched = [
    ["the semicolon form", "\x1b[48;2;0;128;128m"],
    ["the already-correct empty colour-space form", "\x1b[48:2::0:128:128m"],
    ["a real colour-space id", "\x1b[48:2:1:0:128:128m"],
    ["the indexed colon form", "\x1b[38:5:30m"],
    ["the indexed colon form with an empty slot", "\x1b[38:5::30m"],
    ["a bare reset", "\x1b[m"],
    ["an ordinary SGR", "\x1b[1;31m"],
    ["a non-SGR CSI", "\x1b[38:2:1:2:3H"],
    ["a private CSI", "\x1b[?1049h"],
    ["a cursor position report", "\x1b[2J"],
    ["an OSC title", "\x1b]0;38:2:1:2:3\x07"],
    ["an OSC 8 hyperlink", "\x1b]8;;https://example.com/38:2:1:2:3\x1b\\"],
    ["a DCS string", "\x1bP38:2:1:2:3\x1b\\"],
    ["plain text that looks like a parameter", "38:2:1:2:3m"],
    ["an escape that is not a CSI", "\x1b(B"],
    ["a lone escape followed by text", "\x1bZhello"],
    ["a CSI with an intermediate byte", "\x1b[38:2:1:2:3 q"],
    ["a non-numeric sub-parameter", "\x1b[38:2:a:2:3m"],
    ["an empty sub-parameter where a channel belongs", "\x1b[38:2::2:3m"],
    ["too few sub-parameters", "\x1b[38:2:1:2m"],
    ["a colon form that is not 38, 48 or 58", "\x1b[4:2:1:2:3m"],
    ["a colon form whose second slot is not 2", "\x1b[38:3:1:2:3m"],
    ["UTF-8 text", "héllo ✷ 日本語"],
    ["an empty stream", ""],
  ] as const

  for (const [name, input] of untouched) {
    it(`leaves ${name} alone`, () => {
      expect(once(input)).toBe(input)
    })
  }

  it("does not delay a byte that cannot be part of an SGR", () => {
    const n = createSgrColonNormalizer()
    expect(dec.decode(n.push(enc.encode("plain output")))).toBe("plain output")
  })
})

describe("an escape-free chunk costs nothing", () => {
  // Most of what a pty sends contains no escape at all, and the whole chunk is
  // then already the answer. Returning it by IDENTITY is the observable proof
  // that no copy and no per-byte work happened: scanning for the escape is
  // roughly forty times cheaper than walking the bytes.
  it("returns the caller's own array when there is no escape and no carry", () => {
    const n = createSgrColonNormalizer()
    const chunk = enc.encode("no escapes here at all")
    expect(n.push(chunk)).toBe(chunk)
  })

  it("takes the fast path again on the chunk after a completed sequence", () => {
    const n = createSgrColonNormalizer()
    n.push(enc.encode("\x1b[38:2:1:2:3m"))
    const chunk = enc.encode("plain again")
    expect(n.push(chunk)).toBe(chunk)
  })

  it("does NOT take it while a sequence is still being carried", () => {
    const n = createSgrColonNormalizer()
    n.push(enc.encode("\x1b[38:2:1"))
    const chunk = enc.encode(":2:3m tail")
    expect(n.push(chunk)).not.toBe(chunk)
    expect(dec.decode(n.push(enc.encode("")))).toBe("")
  })

  it("does NOT take it for a chunk that contains an escape", () => {
    const n = createSgrColonNormalizer()
    const chunk = enc.encode("text \x1b[0m more")
    expect(n.push(chunk)).not.toBe(chunk)
    expect(dec.decode(n.push(new Uint8Array(0)))).toBe("")
  })
})

describe("the stream may break at any byte", () => {
  const samples = [
    "\x1b[48:2:0:128:128m",
    "before\x1b[38:2:1:2:3mafter",
    "\x1b[0;38:2:1:2:3;48;2;9;9;9mtail",
    "\x1b]8;;https://example.com\x1b\\link",
    "\x1b[38:2:1:2:3H",
  ] as const

  for (const sample of samples) {
    it(`survives every split of ${JSON.stringify(sample)}`, () => {
      const whole = once(sample)
      for (let at = 0; at <= sample.length; at++) {
        expect(split(sample, at)).toBe(whole)
      }
    })
  }

  it("survives a sequence delivered one byte at a time", () => {
    const n = createSgrColonNormalizer()
    const input = "\x1b[48:2:0:128:128m"
    let out = ""
    for (const ch of input) out += dec.decode(n.push(enc.encode(ch)))
    expect(out).toBe("\x1b[48:2::0:128:128m")
  })

  it("holds nothing back once the sequence has ended", () => {
    const n = createSgrColonNormalizer()
    n.push(enc.encode("\x1b[38:2:1:2:3m"))
    expect(dec.decode(n.push(enc.encode("x")))).toBe("x")
  })
})

describe("a runaway parameter string is released rather than hoarded", () => {
  it("flushes an unterminated CSI once it grows past anything real", () => {
    const n = createSgrColonNormalizer()
    const long = "\x1b[" + "1;".repeat(600)
    const out = dec.decode(n.push(enc.encode(long)))
    expect(out.length).toBeGreaterThan(0)
    expect(long.startsWith(out)).toBe(true)
  })

  it("still passes the whole runaway through across the flush", () => {
    const n = createSgrColonNormalizer()
    const long = "\x1b[" + "1;".repeat(600) + "m"
    let out = ""
    for (const ch of long) out += dec.decode(n.push(enc.encode(ch)))
    expect(out).toBe(long)
  })
})

describe("reset drops a half-read sequence", () => {
  it("starts the next stream clean", () => {
    const n = createSgrColonNormalizer()
    n.push(enc.encode("\x1b[48:2:0"))
    n.reset()
    expect(dec.decode(n.push(enc.encode("\x1b[48:2:0:128:128m")))).toBe(
      "\x1b[48:2::0:128:128m",
    )
  })
})
