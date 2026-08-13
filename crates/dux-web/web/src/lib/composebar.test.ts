import { describe, expect, it } from "vitest"

import {
  COMPOSE_SUBMIT_DELAY_MS,
  MAX_COMPOSE_SEND_BYTES,
  composeSendTooLarge,
  composeSendWrites,
  composeBarMode,
  composeBarVisible,
  composeBarShown,
  insertIntoComposeDraft,
  inactiveCursorStyle,
  touchSurfacesApply,
  typingSurfaceToggleOffered,
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

// The splice a picked macro performs on the compose draft: the macro's text
// lands IN the draft at the caret (the compose bar is the phone's typing
// surface, so a macro is an editable draft there, not an immediate wire
// write). Pure and DOM-free: the caller reads the textarea's selection and
// passes it in, so the whole matrix is pinned without mounting anything.
describe("insertIntoComposeDraft", () => {
  it("inserts at the caret, preserving the draft around it", () => {
    expect(insertIntoComposeDraft("hello world", 5, 5, " brave")).toEqual({
      next: "hello brave world",
      caret: 11,
    })
  })

  it("replaces a selection range with the inserted text", () => {
    expect(insertIntoComposeDraft("hello world", 6, 11, "there")).toEqual({
      next: "hello there",
      caret: 11,
    })
  })

  it("appends to the end when the selection is unavailable (null)", () => {
    expect(insertIntoComposeDraft("draft", null, null, "!")).toEqual({
      next: "draft!",
      caret: 6,
    })
  })

  it("appends to the end when either half of the selection is missing", () => {
    expect(insertIntoComposeDraft("draft", 2, null, "!")).toEqual({
      next: "draft!",
      caret: 6,
    })
    expect(insertIntoComposeDraft("draft", null, 2, "!")).toEqual({
      next: "draft!",
      caret: 6,
    })
  })

  it("clamps an out-of-range selection to the draft's bounds", () => {
    // A stale selection (the DOM value briefly lagging the controlled state)
    // must never slice past the end or below zero.
    expect(insertIntoComposeDraft("ab", 5, 9, "X")).toEqual({
      next: "abX",
      caret: 3,
    })
    expect(insertIntoComposeDraft("ab", -3, -1, "X")).toEqual({
      next: "Xab",
      caret: 1,
    })
  })

  it("orders a reversed selection before splicing", () => {
    expect(insertIntoComposeDraft("hello world", 11, 6, "there")).toEqual({
      next: "hello there",
      caret: 11,
    })
  })

  it("inserts multi-line text verbatim, newlines included", () => {
    // The Send path already converts newlines to newline-without-submit
    // keystrokes on the wire; the DRAFT keeps them as real newlines.
    expect(insertIntoComposeDraft("", 0, 0, "a\nb\nc")).toEqual({
      next: "a\nb\nc",
      caret: 5,
    })
  })

  it("lands in an empty draft as the whole draft", () => {
    expect(insertIntoComposeDraft("", null, null, "run tests")).toEqual({
      next: "run tests",
      caret: 9,
    })
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

// The three-way `ui.compose_bar` gate. Pure, so the whole matrix is pinned
// without mounting a terminal or a media query.
describe("composeBarMode", () => {
  it("passes the three known modes through", () => {
    expect(composeBarMode("auto")).toBe("auto")
    expect(composeBarMode("always")).toBe("always")
    expect(composeBarMode("never")).toBe("never")
  })

  // An older server sends no field at all, and a config typo reaches the
  // browser only if it slipped past both the load warning and the projection.
  // Both land on the documented default rather than throwing or disabling.
  it("reads an absent or unrecognized value as auto", () => {
    expect(composeBarMode(undefined)).toBe("auto")
    expect(composeBarMode("")).toBe("auto")
    expect(composeBarMode("sometimes")).toBe("auto")
    expect(composeBarMode("Auto")).toBe("auto")
  })

  // It was a BOOLEAN before the mode existed. The server migrates a config
  // file, but a stale client/server pair could still put one on the wire, and
  // "auto" is the safe landing for either.
  it("reads a legacy boolean off the wire as auto", () => {
    expect(composeBarMode(true as unknown as string)).toBe("auto")
    expect(composeBarMode(false as unknown as string)).toBe("auto")
  })
})

describe("composeBarVisible", () => {
  it("auto follows the pointer capability", () => {
    expect(composeBarVisible("auto", true)).toBe(true)
    expect(composeBarVisible("auto", false)).toBe(false)
  })

  // The whole reason the setting is three-way: a tablet with a keyboard case
  // and one without report identical media queries, so the user must be able
  // to override the capability answer in BOTH directions.
  it("always and never override the capability in both directions", () => {
    expect(composeBarVisible("always", false)).toBe(true)
    expect(composeBarVisible("always", true)).toBe(true)
    expect(composeBarVisible("never", true)).toBe(false)
    expect(composeBarVisible("never", false)).toBe(false)
  })
})

// THE TWO ORTHOGONAL QUESTIONS. Width decides the LAYOUT (how much room is
// there, so which shell you get). The POINTER decides the TYPING SURFACE (is a
// finger doing the typing, so does the text need a buffer autocorrect and swipe
// can work in). These helpers answer only the second one, and nothing here has
// a width in it.
describe("touchSurfacesApply", () => {
  it("puts the touch surfaces wherever the pointer is coarse, layout regardless", () => {
    expect(touchSurfacesApply("auto", true)).toBe(true)
    expect(touchSurfacesApply("never", true)).toBe(true)
  })

  it("keeps them away from a fine pointer unless the user asked for them", () => {
    expect(touchSurfacesApply("auto", false)).toBe(false)
    expect(touchSurfacesApply("never", false)).toBe(false)
    expect(touchSurfacesApply("always", false)).toBe(true)
  })

  // `never` is about the compose BOX. A phone still cannot produce Esc, Tab or
  // a Ctrl chord, so the accessory keys stay.
  it("keeps the accessory keys on a phone whose compose box is switched off", () => {
    expect(touchSurfacesApply("never", true)).toBe(true)
    expect(composeBarShown("never", true, null)).toBe(false)
  })
})

describe("composeBarShown", () => {
  it("follows the pointer while nothing has been chosen on this device", () => {
    expect(composeBarShown("auto", true, null)).toBe(true)
    expect(composeBarShown("auto", false, null)).toBe(false)
  })

  it("lets the device-local choice flip the surface in both directions", () => {
    expect(composeBarShown("auto", true, "direct")).toBe(false)
    expect(composeBarShown("auto", false, "compose")).toBe(true)
  })

  // The SETTING is the configuration surface and it wins. A transient toggle
  // must never quietly defeat something the operator wrote in config.
  it("lets always and never win over the device-local choice", () => {
    expect(composeBarShown("always", true, "direct")).toBe(true)
    expect(composeBarShown("always", false, "direct")).toBe(true)
    expect(composeBarShown("never", true, "compose")).toBe(false)
    expect(composeBarShown("never", false, "compose")).toBe(false)
  })
})

describe("typingSurfaceToggleOffered", () => {
  // The toggle exists only where it can do something: in `auto`, on a device
  // that has the touch surfaces at all. Under always/never the setting decides
  // and a control that changed nothing would be a lie.
  it("is offered in auto on a coarse pointer and nowhere else", () => {
    expect(typingSurfaceToggleOffered("auto", true)).toBe(true)
    expect(typingSurfaceToggleOffered("auto", false)).toBe(false)
    expect(typingSurfaceToggleOffered("always", true)).toBe(false)
    expect(typingSurfaceToggleOffered("never", true)).toBe(false)
  })
})

describe("inactiveCursorStyle", () => {
  // With the compose bar up, xterm NEVER holds focus (the textarea does), so
  // its unfocused cursor is what the user looks at for the whole session. The
  // hollow outline reads as "this terminal is asleep" when it is in fact the
  // live prompt, so that mode gets a solid block.
  it("is a solid block while the compose bar is the typing surface", () => {
    expect(inactiveCursorStyle(true)).toBe("block")
  })

  // Direct typing focuses xterm, so its unfocused cursor means what it means
  // in any real terminal: focus is elsewhere. Leave the convention alone.
  it("keeps the conventional outline when typing goes straight to xterm", () => {
    expect(inactiveCursorStyle(false)).toBe("outline")
  })
})
