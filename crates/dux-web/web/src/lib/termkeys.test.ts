import { describe, expect, it } from "vitest"

import {
  applyModifiers,
  arrowSeq,
  classifyClipboardKey,
  type ClipboardKeyEvent,
  copyOnSelectAction,
  type CopyOnSelectContext,
  ctrlByte,
  ESC,
  forcesTextPaste,
  LF,
  linkActivateAction,
  type LinkActivateContext,
  type LinkActivateEvent,
  linkPressAction,
  type LinkPressContext,
  linkReleaseOpens,
  pageKeySeq,
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

  it("LF is the line-feed (Ctrl-j) byte", () => {
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

  it("keeps Ctrl-c as passthrough so it still sends SIGINT (non-mac)", () => {
    expect(classifyClipboardKey(ev({ ctrlKey: true, code: "KeyC", keyCode: 67 }))).toBe(
      "passthrough",
    )
  })

  it("copies on Ctrl-Shift-c (non-mac)", () => {
    expect(
      classifyClipboardKey(ev({ ctrlKey: true, shiftKey: true, code: "KeyC", keyCode: 67 })),
    ).toBe("copy")
  })

  it("copies on Ctrl-Insert (the Chrome-safe chord)", () => {
    expect(classifyClipboardKey(ev({ ctrlKey: true, code: "Insert", keyCode: 45 }))).toBe(
      "copy",
    )
  })

  it("pastes on Ctrl-v (non-mac)", () => {
    expect(classifyClipboardKey(ev({ ctrlKey: true, code: "KeyV", keyCode: 86 }))).toBe(
      "paste",
    )
  })

  it("pastes on Ctrl-Shift-v (non-mac)", () => {
    expect(
      classifyClipboardKey(ev({ ctrlKey: true, shiftKey: true, code: "KeyV", keyCode: 86 })),
    ).toBe("paste")
  })

  it("passes Shift-Insert through (browser/OS default, not our clipboard)", () => {
    expect(classifyClipboardKey(ev({ shiftKey: true, code: "Insert", keyCode: 45 }))).toBe(
      "passthrough",
    )
  })

  it("passes Cmd-c and Cmd-v through so the browser does native copy/paste", () => {
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

    it("still copies on mac Ctrl-Shift-c", () => {
      expect(
        classifyClipboardKey(
          ev({ ctrlKey: true, shiftKey: true, code: "KeyC", keyCode: 67, isMac: true }),
        ),
      ).toBe("copy")
    })

    it("still pastes on mac Ctrl-Shift-v", () => {
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
    // Ctrl-Alt-v is excluded so AltGr/Meta chords reach the app.
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

describe("forcesTextPaste", () => {
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

  it("matches Ctrl-Shift-v", () => {
    expect(forcesTextPaste(ev({ ctrlKey: true, shiftKey: true, code: "KeyV" }))).toBe(
      true,
    )
  })

  it("matches Cmd-Shift-v, which the classifier never sees", () => {
    // `classifyClipboardKey` returns `passthrough` for every Cmd combo before
    // any other rule, so a mac user would have no hatch if this lived inside
    // it. Both platforms get the same chord.
    expect(classifyClipboardKey(ev({ metaKey: true, shiftKey: true, code: "KeyV" }))).toBe(
      "passthrough",
    )
    expect(forcesTextPaste(ev({ metaKey: true, shiftKey: true, code: "KeyV" }))).toBe(true)
  })

  it("does not match a plain paste chord, which stays image-wins", () => {
    expect(forcesTextPaste(ev({ ctrlKey: true, code: "KeyV" }))).toBe(false)
    expect(forcesTextPaste(ev({ metaKey: true, code: "KeyV" }))).toBe(false)
    expect(forcesTextPaste(ev({ shiftKey: true, code: "KeyV" }))).toBe(false)
  })

  it("does not match another key, or the chord with Alt in it", () => {
    expect(forcesTextPaste(ev({ ctrlKey: true, shiftKey: true, code: "KeyC" }))).toBe(
      false,
    )
    expect(
      forcesTextPaste(ev({ ctrlKey: true, shiftKey: true, altKey: true, code: "KeyV" })),
    ).toBe(false)
  })

  it("falls back to keyCode on a layout that reports no code", () => {
    expect(
      forcesTextPaste(ev({ ctrlKey: true, shiftKey: true, code: "", keyCode: 86 })),
    ).toBe(true)
    expect(
      forcesTextPaste(ev({ ctrlKey: true, shiftKey: true, code: "", keyCode: 0 })),
    ).toBe(false)
  })
})

describe("copyOnSelectAction", () => {
  const ctx = (o: Partial<CopyOnSelectContext> = {}): CopyOnSelectContext => ({
    copyOnSelect: true,
    selection: "",
    dragged: false,
    mouseTrackingMode: "none",
    hintShown: false,
    gesture: "mouse-drag",
    ...o,
  })

  it("copies a real multi-char selection", () => {
    expect(copyOnSelectAction(ctx({ selection: "hello" }))).toBe("copy")
  })

  it("ignores an empty or whitespace-only selection", () => {
    expect(copyOnSelectAction(ctx({ selection: "" }))).toBe("ignore")
    expect(copyOnSelectAction(ctx({ selection: "   " }))).toBe("ignore")
  })

  it("ignores a trivial one-char selection from a MOUSE drag (misclick guard)", () => {
    expect(copyOnSelectAction(ctx({ selection: "x" }))).toBe("ignore")
  })

  it("copies a one-char selection from a long press, which is deliberate", () => {
    // The floor exists to stop a stray click-drag clobbering the clipboard with
    // one character. A 400ms committed press is not a stray anything, and
    // single-token targets are ordinary in a terminal: a flag letter, a digit,
    // a "y". Refusing them highlighted the character and copied nothing, with
    // no toast to say why.
    expect(copyOnSelectAction(ctx({ selection: "y", gesture: "long-press" }))).toBe(
      "copy",
    )
  })

  it("still ignores an empty or blank selection from a long press", () => {
    expect(copyOnSelectAction(ctx({ selection: "", gesture: "long-press" }))).toBe(
      "ignore",
    )
    expect(copyOnSelectAction(ctx({ selection: " ", gesture: "long-press" }))).toBe(
      "ignore",
    )
  })

  it("does nothing when the preference is off, even with a selection", () => {
    expect(copyOnSelectAction(ctx({ copyOnSelect: false, selection: "hello" }))).toBe(
      "ignore",
    )
  })

  it("hints when a drag produced no local selection while the app holds the mouse", () => {
    expect(
      copyOnSelectAction(ctx({ dragged: true, mouseTrackingMode: "any" })),
    ).toBe("hint")
  })

  it("does not hint on a plain click (no drag) in a mouse-reporting app", () => {
    expect(
      copyOnSelectAction(ctx({ dragged: false, mouseTrackingMode: "any" })),
    ).toBe("ignore")
  })

  it("does not hint when the app has not captured the mouse", () => {
    expect(
      copyOnSelectAction(ctx({ dragged: true, mouseTrackingMode: "none" })),
    ).toBe("ignore")
  })

  it("hints only once per session", () => {
    expect(
      copyOnSelectAction(ctx({ dragged: true, mouseTrackingMode: "any", hintShown: true })),
    ).toBe("ignore")
  })

  it("prefers copying a real selection over hinting", () => {
    expect(
      copyOnSelectAction(
        ctx({ selection: "hello", dragged: true, mouseTrackingMode: "any" }),
      ),
    ).toBe("copy")
  })
})

describe("linkActivateAction", () => {
  const ev = (over: Partial<LinkActivateEvent> = {}): LinkActivateEvent => ({
    button: 0,
    detail: 1,
    ctrlKey: false,
    metaKey: false,
    ...over,
  })
  const ctx = (over: Partial<LinkActivateContext> = {}): LinkActivateContext => ({
    hyperlinks: true,
    uri: "https://example.com",
    mouseTracking: false,
    isMac: false,
    ...over,
  })

  it("opens a plain primary click on an http(s) link", () => {
    expect(linkActivateAction(ev(), ctx())).toBe("open")
    expect(linkActivateAction(ev(), ctx({ uri: "http://example.com" }))).toBe("open")
    expect(linkActivateAction(ev(), ctx({ uri: "HTTPS://EXAMPLE.COM" }))).toBe("open")
  })

  it("ignores the repeat clicks of a double- or triple-click", () => {
    // Selecting a word (double) or a line (triple) is the gesture; xterm fires
    // an activation on every mouseup of it, and only the first is a click.
    expect(linkActivateAction(ev({ detail: 2 }), ctx())).toBe("ignore")
    expect(linkActivateAction(ev({ detail: 3 }), ctx())).toBe("ignore")
  })

  it("treats a detail-less activation as a single click", () => {
    // Synthetic and assistive-technology events can carry detail 0.
    expect(linkActivateAction(ev({ detail: 0 }), ctx())).toBe("open")
  })

  it("ignores non-primary buttons", () => {
    // Right-click is dux's paste gesture and middle-click is the X11 primary
    // paste; neither means "follow this link".
    expect(linkActivateAction(ev({ button: 1 }), ctx())).toBe("ignore")
    expect(linkActivateAction(ev({ button: 2 }), ctx())).toBe("ignore")
  })

  it("ignores everything when the hyperlinks preference is off", () => {
    expect(linkActivateAction(ev(), ctx({ hyperlinks: false }))).toBe("ignore")
  })

  it("ignores schemes other than http and https", () => {
    for (const uri of [
      "javascript:alert(1)",
      "file:///etc/passwd",
      "data:text/html,<script>",
      "ftp://example.com",
      "vscode://file/tmp",
      "  https://example.com",
    ]) {
      expect(linkActivateAction(ev(), ctx({ uri }))).toBe("ignore")
    }
  })

  // The force-forward hatch hands the click to the app in the PTY, so dux must
  // not ALSO open a tab: without this the hatch is the double-open it exists to
  // avoid. It reaches here because the hatch click is deliberately not
  // swallowed, so xterm's own Linkifier still activates the link on mouseup.
  it("refuses the hatch chord while the app is tracking the mouse", () => {
    expect(
      linkActivateAction(ev({ ctrlKey: true }), ctx({ mouseTracking: true })),
    ).toBe("ignore")
    expect(
      linkActivateAction(
        ev({ metaKey: true }),
        ctx({ mouseTracking: true, isMac: true }),
      ),
    ).toBe("ignore")
  })

  // The chord is per platform, and the OTHER platform's chord is not a hatch.
  it("reads the hatch chord per platform", () => {
    expect(
      linkActivateAction(
        ev({ metaKey: true }),
        ctx({ mouseTracking: true, isMac: false }),
      ),
    ).toBe("open")
    expect(
      linkActivateAction(
        ev({ ctrlKey: true }),
        ctx({ mouseTracking: true, isMac: true }),
      ),
    ).toBe("open")
  })

  // With tracking OFF there is no app to forward to, so the chord keeps its
  // browser meaning (open in a new tab) rather than becoming a way to open
  // nothing at all.
  it("keeps chord-click opening when the app is not tracking the mouse", () => {
    expect(linkActivateAction(ev({ ctrlKey: true }), ctx())).toBe("open")
    expect(linkActivateAction(ev({ metaKey: true }), ctx({ isMac: true }))).toBe("open")
  })
})

describe("linkPressAction", () => {
  const ev = (over: Partial<LinkActivateEvent> = {}): LinkActivateEvent => ({
    button: 0,
    detail: 1,
    ctrlKey: false,
    metaKey: false,
    ...over,
  })
  const ctx = (over: Partial<LinkPressContext> = {}): LinkPressContext => ({
    hoveredUri: "https://example.com",
    mouseTracking: true,
    hyperlinks: true,
    isMac: false,
    ...over,
  })

  it("swallows a plain primary press on a link and opens it", () => {
    expect(linkPressAction(ev(), ctx())).toEqual({ suppress: true, open: true })
  })

  // The whole point: with tracking off there is no report to suppress, and
  // swallowing would cost the focus grab, the selection clear and the
  // copy-on-select listeners. Today's Linkifier path stays byte-identical.
  it("leaves every press alone when the app is not tracking the mouse", () => {
    expect(linkPressAction(ev(), ctx({ mouseTracking: false }))).toEqual({
      suppress: false,
      open: false,
    })
  })

  it("leaves a press that is not on a link alone", () => {
    expect(linkPressAction(ev(), ctx({ hoveredUri: null }))).toEqual({
      suppress: false,
      open: false,
    })
  })

  it("leaves non-primary buttons alone, so paste and menus are untouched", () => {
    for (const button of [1, 2]) {
      expect(linkPressAction(ev({ button }), ctx())).toEqual({
        suppress: false,
        open: false,
      })
    }
  })

  // The hatch is the escape valve: the app gets the click, dux opens nothing.
  it("forwards the press under the platform hatch chord", () => {
    expect(linkPressAction(ev({ ctrlKey: true }), ctx())).toEqual({
      suppress: false,
      open: false,
    })
    expect(
      linkPressAction(ev({ metaKey: true }), ctx({ isMac: true })),
    ).toEqual({ suppress: false, open: false })
    // ...and the other platform's chord is an ordinary click.
    expect(linkPressAction(ev({ metaKey: true }), ctx())).toEqual({
      suppress: true,
      open: true,
    })
    expect(
      linkPressAction(ev({ ctrlKey: true }), ctx({ isMac: true })),
    ).toEqual({ suppress: true, open: true })
  })

  // The split. The second press of a double-click must still be SWALLOWED (a
  // clean click reaching the app resurrects the server-side open) while opening
  // nothing, because a double-click is the select-a-word gesture.
  it("swallows the tail of a multi-click gesture without opening", () => {
    expect(linkPressAction(ev({ detail: 2 }), ctx())).toEqual({
      suppress: true,
      open: false,
    })
    expect(linkPressAction(ev({ detail: 3 }), ctx())).toEqual({
      suppress: true,
      open: false,
    })
  })

  it("treats a detail-less press as a single click", () => {
    expect(linkPressAction(ev({ detail: 0 }), ctx())).toEqual({
      suppress: true,
      open: true,
    })
  })

  // A press dux swallows but cannot open is still swallowed: forwarding it
  // would hand the app a lone press with no release.
  it("swallows without opening when the preference or the scheme refuses", () => {
    expect(linkPressAction(ev(), ctx({ hyperlinks: false }))).toEqual({
      suppress: true,
      open: false,
    })
    expect(linkPressAction(ev(), ctx({ hoveredUri: "ftp://example.com" }))).toEqual({
      suppress: true,
      open: false,
    })
  })
})

describe("linkReleaseOpens", () => {
  const ctx = (over: Partial<Parameters<typeof linkReleaseOpens>[0]> = {}) => ({
    open: true,
    withinDragThreshold: true,
    releaseUri: "https://example.com",
    pressedUri: "https://example.com",
    ...over,
  })

  it("opens a release that barely moved", () => {
    expect(linkReleaseOpens(ctx({ releaseUri: null }))).toBe(true)
  })

  it("opens a release that travelled but stayed on the pressed link", () => {
    expect(linkReleaseOpens(ctx({ withinDragThreshold: false }))).toBe(true)
  })

  // Press here, release elsewhere: a drag, not a click.
  it("refuses a release that left the pressed link", () => {
    expect(
      linkReleaseOpens(ctx({ withinDragThreshold: false, releaseUri: null })),
    ).toBe(false)
    expect(
      linkReleaseOpens(
        ctx({ withinDragThreshold: false, releaseUri: "https://other.example" }),
      ),
    ).toBe(false)
  })

  it("refuses when the press was never eligible to open", () => {
    expect(linkReleaseOpens(ctx({ open: false }))).toBe(false)
  })
})
