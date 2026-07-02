import { describe, expect, it } from "vitest"

import {
  applyModifiers,
  arrowSeq,
  classifyClipboardKey,
  type ClipboardKeyEvent,
  ctrlByte,
  ESC,
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
