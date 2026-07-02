import { describe, expect, it } from "vitest"

import {
  applyModifiers,
  arrowSeq,
  classifyClipboardKey,
  type ClipboardKeyEvent,
  ctrlByte,
  ESC,
  LF,
  pageKeySeq,
  sgrWheelSeq,
  softNewline,
  softNewlineAction,
  TAB,
} from "./termkeys"

// Default runtime context: the input owner with no latch armed.
const OWNER = { isOwner: true, ctrlLatched: false, altLatched: false }

// Builds the minimal shape `softNewline` reads, defaulting to a bare Enter
// keydown (13, not composing) so each test only states the fields it cares about.
function keyEvent(
  over: Partial<{
    type: string
    key: string
    ctrlKey: boolean
    shiftKey: boolean
    altKey: boolean
    metaKey: boolean
    isComposing: boolean
    keyCode: number
  }>,
): {
  type: string
  key: string
  ctrlKey: boolean
  shiftKey: boolean
  altKey: boolean
  metaKey: boolean
  isComposing: boolean
  keyCode: number
} {
  return {
    type: "keydown",
    key: "Enter",
    ctrlKey: false,
    shiftKey: false,
    altKey: false,
    metaKey: false,
    isComposing: false,
    keyCode: 13,
    ...over,
  }
}

describe("constants", () => {
  it("ESC is the ASCII escape byte", () => {
    expect(ESC).toBe("\x1b")
    expect(ESC.charCodeAt(0)).toBe(0x1b)
  })

  it("TAB is the horizontal-tab byte", () => {
    expect(TAB).toBe("\x09")
    expect(TAB.charCodeAt(0)).toBe(0x09)
  })

  it("LF is the line-feed (Ctrl-J) byte", () => {
    expect(LF).toBe("\x0a")
    expect(LF.charCodeAt(0)).toBe(0x0a)
  })
})

describe("softNewline", () => {
  it("maps bare Shift-Enter keydown to LF", () => {
    expect(softNewline(keyEvent({ shiftKey: true }))).toBe(LF)
  })

  it("ignores a plain Enter (no Shift) so it submits as CR", () => {
    expect(softNewline(keyEvent({ shiftKey: false }))).toBeNull()
  })

  it("ignores keyup so the newline is not emitted twice", () => {
    expect(softNewline(keyEvent({ type: "keyup", shiftKey: true }))).toBeNull()
  })

  it("ignores Shift-Enter when another modifier is also held", () => {
    expect(softNewline(keyEvent({ shiftKey: true, ctrlKey: true }))).toBeNull()
    expect(softNewline(keyEvent({ shiftKey: true, altKey: true }))).toBeNull()
    expect(softNewline(keyEvent({ shiftKey: true, metaKey: true }))).toBeNull()
  })

  it("ignores Shift held with a non-Enter key", () => {
    expect(softNewline(keyEvent({ key: "a", shiftKey: true }))).toBeNull()
    expect(softNewline(keyEvent({ key: "Tab", shiftKey: true }))).toBeNull()
  })

  it("ignores Shift-Enter while an IME composition is in flight", () => {
    // A confirming keystroke mid-CJK-composition must finalize composition via
    // xterm, not get rewritten to a stray LF. Both signals are honored.
    expect(
      softNewline(keyEvent({ shiftKey: true, isComposing: true })),
    ).toBeNull()
    expect(
      softNewline(keyEvent({ shiftKey: true, keyCode: 229 })),
    ).toBeNull()
  })
})

describe("softNewlineAction", () => {
  const shiftEnter = keyEvent({ shiftKey: true })

  it("leaves a non-matching key entirely to xterm", () => {
    const a = softNewlineAction(keyEvent({ shiftKey: false }), OWNER)
    expect(a).toEqual({ handled: false, send: null, clearLatch: false })
  })

  it("an owner sends the LF for a bare Shift-Enter", () => {
    const a = softNewlineAction(shiftEnter, OWNER)
    expect(a).toEqual({ handled: true, send: LF, clearLatch: false })
  })

  it("a read-only viewer consumes the key but sends nothing", () => {
    const a = softNewlineAction(shiftEnter, {
      isOwner: false,
      ctrlLatched: false,
      altLatched: false,
    })
    expect(a).toEqual({ handled: true, send: null, clearLatch: false })
  })

  it("clears an armed Ctrl or Alt latch when the owner sends the newline", () => {
    expect(
      softNewlineAction(shiftEnter, { ...OWNER, ctrlLatched: true }),
    ).toEqual({ handled: true, send: LF, clearLatch: true })
    expect(
      softNewlineAction(shiftEnter, { ...OWNER, altLatched: true }),
    ).toEqual({ handled: true, send: LF, clearLatch: true })
  })

  it("does not clear a latch for a non-owner (nothing is consumed to send)", () => {
    const a = softNewlineAction(shiftEnter, {
      isOwner: false,
      ctrlLatched: true,
      altLatched: false,
    })
    expect(a.clearLatch).toBe(false)
  })

  it("never sends or clears mid-IME-composition", () => {
    const a = softNewlineAction(
      keyEvent({ shiftKey: true, isComposing: true }),
      { ...OWNER, ctrlLatched: true },
    )
    expect(a).toEqual({ handled: false, send: null, clearLatch: false })
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

describe("classifyClipboardKey", () => {
  // Build a key event with every field defaulted; tests override only what
  // matters. `code` is the physical-key signal we classify on (NOT `key`).
  const ev = (over: Partial<ClipboardKeyEvent>): ClipboardKeyEvent => ({
    ctrlKey: false,
    shiftKey: false,
    altKey: false,
    metaKey: false,
    code: "",
    keyCode: 0,
    isMac: false,
    ...over,
  })

  it("keeps Ctrl-C as passthrough so it still sends SIGINT (non-mac)", () => {
    expect(classifyClipboardKey(ev({ ctrlKey: true, code: "KeyC", keyCode: 67 }))).toBe(
      "passthrough",
    )
  })

  it("copies on Ctrl-Shift-C (non-mac)", () => {
    expect(
      classifyClipboardKey(ev({ ctrlKey: true, shiftKey: true, code: "KeyC", keyCode: 67 })),
    ).toBe("copy")
  })

  it("copies on Ctrl-Insert (the Chrome-safe chord)", () => {
    expect(classifyClipboardKey(ev({ ctrlKey: true, code: "Insert", keyCode: 45 }))).toBe(
      "copy",
    )
  })

  it("pastes on Ctrl-V (non-mac)", () => {
    expect(classifyClipboardKey(ev({ ctrlKey: true, code: "KeyV", keyCode: 86 }))).toBe(
      "paste",
    )
  })

  it("pastes on Ctrl-Shift-V (non-mac)", () => {
    expect(
      classifyClipboardKey(ev({ ctrlKey: true, shiftKey: true, code: "KeyV", keyCode: 86 })),
    ).toBe("paste")
  })

  it("passes Shift-Insert through (browser/OS default, not our clipboard)", () => {
    expect(classifyClipboardKey(ev({ shiftKey: true, code: "Insert", keyCode: 45 }))).toBe(
      "passthrough",
    )
  })

  it("passes Cmd-C and Cmd-V through so the browser does native copy/paste", () => {
    expect(classifyClipboardKey(ev({ metaKey: true, code: "KeyC", keyCode: 67 }))).toBe(
      "passthrough",
    )
    expect(classifyClipboardKey(ev({ metaKey: true, code: "KeyV", keyCode: 86 }))).toBe(
      "passthrough",
    )
  })

  describe("mac: Control passes through to the app, Ctrl-Shift aliases still work", () => {
    it("passes mac Control-V through (vim visual-block / verbatim survive)", () => {
      expect(
        classifyClipboardKey(ev({ ctrlKey: true, code: "KeyV", keyCode: 86, isMac: true })),
      ).toBe("passthrough")
    })

    it("passes mac Control-C through (SIGINT)", () => {
      expect(
        classifyClipboardKey(ev({ ctrlKey: true, code: "KeyC", keyCode: 67, isMac: true })),
      ).toBe("passthrough")
    })

    it("still copies on mac Ctrl-Shift-C", () => {
      expect(
        classifyClipboardKey(
          ev({ ctrlKey: true, shiftKey: true, code: "KeyC", keyCode: 67, isMac: true }),
        ),
      ).toBe("copy")
    })

    it("still pastes on mac Ctrl-Shift-V", () => {
      expect(
        classifyClipboardKey(
          ev({ ctrlKey: true, shiftKey: true, code: "KeyV", keyCode: 86, isMac: true }),
        ),
      ).toBe("paste")
    })
  })

  it("passes plain keys and non-clipboard chords through", () => {
    expect(classifyClipboardKey(ev({ code: "KeyV", keyCode: 86 }))).toBe("passthrough")
    expect(classifyClipboardKey(ev({ code: "KeyC", keyCode: 67 }))).toBe("passthrough")
    // Ctrl-Alt-V is excluded so AltGr/Meta chords reach the app.
    expect(
      classifyClipboardKey(ev({ ctrlKey: true, altKey: true, code: "KeyV", keyCode: 86 })),
    ).toBe("passthrough")
    // Ctrl-1 is not a clipboard chord.
    expect(classifyClipboardKey(ev({ ctrlKey: true, code: "Digit1", keyCode: 49 }))).toBe(
      "passthrough",
    )
  })

  it("classifies by physical key, not ev.key — so non-Latin layouts still work", () => {
    // A Cyrillic layout types 'м' on the physical V key, but `code` is still
    // 'KeyV'. We must intercept it (xterm would otherwise emit \x16 by keyCode).
    expect(
      classifyClipboardKey(ev({ ctrlKey: true, code: "KeyV", keyCode: 86 })),
    ).toBe("paste")
  })

  it("falls back to keyCode when code is empty", () => {
    expect(classifyClipboardKey(ev({ ctrlKey: true, code: "", keyCode: 86 }))).toBe("paste")
    expect(
      classifyClipboardKey(ev({ ctrlKey: true, shiftKey: true, code: "", keyCode: 67 })),
    ).toBe("copy")
  })

  it("is safe when both code and keyCode are unset (synthetic/IME)", () => {
    expect(classifyClipboardKey(ev({ ctrlKey: true, code: "", keyCode: 0 }))).toBe(
      "passthrough",
    )
  })
})
