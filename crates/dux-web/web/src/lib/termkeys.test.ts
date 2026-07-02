import { describe, expect, it } from "vitest"

import {
  applyModifiers,
  arrowSeq,
  ctrlByte,
  ESC,
  pageKeySeq,
  sgrWheelSeq,
  TAB,
} from "./termkeys"

describe("constants", () => {
  it("ESC is the ASCII escape byte", () => {
    expect(ESC).toBe("\x1b")
    expect(ESC.charCodeAt(0)).toBe(0x1b)
  })

  it("TAB is the horizontal-tab byte", () => {
    expect(TAB).toBe("\x09")
    expect(TAB.charCodeAt(0)).toBe(0x09)
  })
})

describe("ctrlByte", () => {
  it("maps lowercase letters a-z to 0x01-0x1A", () => {
    expect(ctrlByte("a")).toBe("\x01")
    expect(ctrlByte("z")).toBe("\x1a")
    expect(ctrlByte("c")).toBe("\x03")
  })

  it("case-folds uppercase letters to the same control byte", () => {
    expect(ctrlByte("A")).toBe(ctrlByte("a"))
    expect(ctrlByte("Z")).toBe(ctrlByte("z"))
    expect(ctrlByte("C")).toBe("\x03")
  })

  it("maps the full control-punctuation table", () => {
    expect(ctrlByte("@")).toBe("\x00")
    expect(ctrlByte("[")).toBe("\x1b")
    expect(ctrlByte("\\")).toBe("\x1c")
    expect(ctrlByte("]")).toBe("\x1d")
    expect(ctrlByte("^")).toBe("\x1e")
    expect(ctrlByte("_")).toBe("\x1f")
    expect(ctrlByte(" ")).toBe("\x00")
  })

  it("maps Ctrl-<digit> aliases for 2-8", () => {
    expect(ctrlByte("2")).toBe("\x00")
    expect(ctrlByte("3")).toBe("\x1b")
    expect(ctrlByte("4")).toBe("\x1c")
    expect(ctrlByte("5")).toBe("\x1d")
    expect(ctrlByte("6")).toBe("\x1e")
    expect(ctrlByte("7")).toBe("\x1f")
    expect(ctrlByte("8")).toBe("\x7f")
  })

  it("returns null for digits without a control mapping", () => {
    expect(ctrlByte("0")).toBeNull()
    expect(ctrlByte("1")).toBeNull()
    expect(ctrlByte("9")).toBeNull()
  })

  it("returns null for unmapped, multi-char, and empty input", () => {
    expect(ctrlByte("!")).toBeNull()
    expect(ctrlByte("ab")).toBeNull()
    expect(ctrlByte("")).toBeNull()
  })
})

describe("arrowSeq", () => {
  it("emits CSI form in normal cursor-key mode", () => {
    expect(arrowSeq("up", false)).toBe(`${ESC}[A`)
    expect(arrowSeq("down", false)).toBe(`${ESC}[B`)
    expect(arrowSeq("right", false)).toBe(`${ESC}[C`)
    expect(arrowSeq("left", false)).toBe(`${ESC}[D`)
  })

  it("emits SS3 form in application cursor-key mode", () => {
    expect(arrowSeq("up", true)).toBe(`${ESC}OA`)
    expect(arrowSeq("down", true)).toBe(`${ESC}OB`)
    expect(arrowSeq("right", true)).toBe(`${ESC}OC`)
    expect(arrowSeq("left", true)).toBe(`${ESC}OD`)
  })
})

describe("sgrWheelSeq", () => {
  // SGR press form: ESC [ < Cb ; Col ; Row M. Button 64 = wheel up (older
  // output), 65 = wheel down (newer). `lines` is signed like xterm's
  // scrollLines(): NEGATIVE reveals older output (wheel up), POSITIVE newer.
  it("emits a wheel-UP (button 64) event for negative lines (reveal older)", () => {
    expect(sgrWheelSeq(-1, 3, 7)).toBe(`${ESC}[<64;3;7M`)
  })

  it("emits a wheel-DOWN (button 65) event for positive lines (reveal newer)", () => {
    expect(sgrWheelSeq(1, 3, 7)).toBe(`${ESC}[<65;3;7M`)
  })

  it("stacks one wheel event per line, preserving direction", () => {
    expect(sgrWheelSeq(-3, 1, 1)).toBe(
      `${ESC}[<64;1;1M${ESC}[<64;1;1M${ESC}[<64;1;1M`,
    )
    expect(sgrWheelSeq(2, 1, 1)).toBe(`${ESC}[<65;1;1M${ESC}[<65;1;1M`)
  })

  it("returns an empty string for a zero scroll", () => {
    expect(sgrWheelSeq(0, 5, 5)).toBe("")
  })

  it("clamps the cell to a 1-based minimum so an out-of-bounds touch is valid", () => {
    expect(sgrWheelSeq(-1, 0, -4)).toBe(`${ESC}[<64;1;1M`)
  })

  it("truncates fractional line counts and coordinates", () => {
    expect(sgrWheelSeq(-1.9, 2.8, 4.2)).toBe(`${ESC}[<64;2;4M`)
  })
})

describe("pageKeySeq", () => {
  it("emits the PgUp escape for up", () => {
    expect(pageKeySeq("up")).toBe(`${ESC}[5~`)
  })

  it("emits the PgDn escape for down", () => {
    expect(pageKeySeq("down")).toBe(`${ESC}[6~`)
  })
})

describe("applyModifiers", () => {
  it("passes a single char through with no modifiers", () => {
    expect(applyModifiers("x", { ctrl: false, alt: false })).toBe("x")
  })

  it("applies ctrl to a mappable single char", () => {
    expect(applyModifiers("c", { ctrl: true, alt: false })).toBe("\x03")
  })

  it("falls back to the raw char when ctrl has no mapping", () => {
    expect(applyModifiers("1", { ctrl: true, alt: false })).toBe("1")
    expect(applyModifiers("!", { ctrl: true, alt: false })).toBe("!")
  })

  it("prefixes alt (Meta) with ESC", () => {
    expect(applyModifiers("a", { ctrl: false, alt: true })).toBe(`${ESC}a`)
  })

  it("combines alt+ctrl as ESC then the control byte, in that order", () => {
    expect(applyModifiers("c", { ctrl: true, alt: true })).toBe(`${ESC}\x03`)
  })

  it("passes multi-char chunks through untransformed under every modifier", () => {
    const chunk = "paste"
    expect(applyModifiers(chunk, { ctrl: false, alt: false })).toBe(chunk)
    expect(applyModifiers(chunk, { ctrl: true, alt: false })).toBe(chunk)
    expect(applyModifiers(chunk, { ctrl: false, alt: true })).toBe(chunk)
    expect(applyModifiers(chunk, { ctrl: true, alt: true })).toBe(chunk)
  })
})
