import { describe, expect, it } from "vitest"

import {
  COMPOSE_SUBMIT_DELAY_MS,
  MAX_COMPOSE_SEND_BYTES,
  composeSendTooLarge,
  composeSendWrites,
  composeBarMode,
  resolvedTypingSurface,
  composeBarShown,
  insertIntoComposeDraft,
  inactiveCursorStyle,
  bottomBarSurvivesDirect,
  terminalKeysApply,
  inputMenuSurfaceSwitchOffered,
} from "./composebar"
import type { ComposeBarMode, TypingSurfaceChoice } from "./composebar"

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

describe("resolvedTypingSurface", () => {
  // THE POINTER IS A DEFAULT, NOT A GATE. Nobody has chosen, so the capability
  // answers: a finger gets the buffered box, a mouse types straight in.
  it("falls back to the pointer while nothing has been chosen", () => {
    expect(resolvedTypingSurface("auto", true, null)).toBe("compose")
    expect(resolvedTypingSurface("auto", false, null)).toBe("direct")
  })

  // The whole point of the fix: an explicit choice wins on EVERY device, in
  // both directions. Turning the message box on where the browser thinks it is
  // not needed is allowed.
  it("lets an explicit choice win in both directions on either pointer", () => {
    expect(resolvedTypingSurface("auto", false, "compose")).toBe("compose")
    expect(resolvedTypingSurface("auto", true, "direct")).toBe("direct")
    expect(resolvedTypingSurface("auto", true, "compose")).toBe("compose")
    expect(resolvedTypingSurface("auto", false, "direct")).toBe("direct")
  })

  // The SETTING is the configuration surface and it wins. A per-device toggle
  // must never quietly defeat something the operator wrote in config.
  it("lets always and never win over the device-local choice", () => {
    expect(resolvedTypingSurface("always", false, "direct")).toBe("compose")
    expect(resolvedTypingSurface("never", true, "compose")).toBe("direct")
  })
})

// EVERY STATE OF THE RESOLVED RULE, as a table. Three modes times two pointer
// answers times three choices is small enough to write out in full, and written
// out in full a changed arm shows up as a row that disagrees rather than as a
// case nobody thought to add. The prose tests above say WHY each arm is what it
// is; this says WHAT, exhaustively.
describe("resolvedTypingSurface over every state", () => {
  const table: [
    ComposeBarMode,
    boolean,
    TypingSurfaceChoice | null,
    TypingSurfaceChoice,
  ][] = [
    // The setting has decided; neither the pointer nor the choice is consulted.
    ["always", false, null, "compose"],
    ["always", false, "compose", "compose"],
    ["always", false, "direct", "compose"],
    ["always", true, null, "compose"],
    ["always", true, "compose", "compose"],
    ["always", true, "direct", "compose"],
    ["never", false, null, "direct"],
    ["never", false, "compose", "direct"],
    ["never", false, "direct", "direct"],
    ["never", true, null, "direct"],
    ["never", true, "compose", "direct"],
    ["never", true, "direct", "direct"],
    // `auto` means work it out: the person first, the browser second.
    ["auto", false, null, "direct"],
    ["auto", false, "compose", "compose"],
    ["auto", false, "direct", "direct"],
    ["auto", true, null, "compose"],
    ["auto", true, "compose", "compose"],
    ["auto", true, "direct", "direct"],
  ]

  it.each(table)(
    "%s with coarse=%s and choice %s resolves to %s",
    (mode, coarsePointer, choice, expected) => {
      expect(resolvedTypingSurface(mode, coarsePointer, choice)).toBe(expected)
      // The two readers of the rule agree with it by construction, and this is
      // what keeps that true if either ever grows a condition of its own.
      expect(composeBarShown(mode, coarsePointer, choice)).toBe(
        expected === "compose",
      )
    },
  )
})

// THE TWO ORTHOGONAL QUESTIONS. Width decides the LAYOUT (how much room is
// there, so which shell you get). The POINTER decides the DEFAULT TYPING
// SURFACE (is a finger doing the typing, so does the text need a buffer
// autocorrect and swipe can work in). These helpers answer only the second one,
// and nothing here has a width in it.
describe("terminalKeysApply", () => {
  it("is up wherever the pointer is coarse and nothing has been chosen", () => {
    expect(terminalKeysApply("auto", true, null)).toBe(true)
  })

  it("is down for a fine pointer unless the user or the setting asked", () => {
    expect(terminalKeysApply("auto", false, null)).toBe(false)
    expect(terminalKeysApply("never", false, null)).toBe(false)
    expect(terminalKeysApply("always", false, null)).toBe(true)
  })

  // The inert-toggle bug: choosing the message box on a laptop used to leave
  // the keys behind, because the key row was gated on the pointer alone.
  it("brings the keys to a fine pointer that chose the message box", () => {
    expect(terminalKeysApply("auto", false, "compose")).toBe(true)
  })

  // THE KEY ROW AND THE MESSAGE BOX ARE INDEPENDENT. Direct typing is a
  // statement about where the TEXT is composed; Esc, Tab and a Ctrl chord are
  // keys a soft keyboard cannot produce at all, and holding the virtual Ctrl
  // while a physical keyboard types the letter is a real way to work.
  it("keeps the keys on a phone that chose direct typing", () => {
    expect(terminalKeysApply("auto", true, "direct")).toBe(true)
    expect(composeBarShown("auto", true, "direct")).toBe(false)
  })

  it("keeps the keys on a phone whose compose box is switched off", () => {
    expect(terminalKeysApply("never", true, null)).toBe(true)
    expect(composeBarShown("never", true, null)).toBe(false)
    expect(terminalKeysApply("never", true, "direct")).toBe(true)
  })

  // ALL FOUR COMBINATIONS ARE LEGAL, which is the whole point of keeping the
  // two apart: box and keys, box alone, keys alone, and neither.
  it("spans the four states the two surfaces can be in", () => {
    const state = (
      coarse: boolean,
      choice: "compose" | "direct" | null,
      keysVisible: boolean,
    ) => ({
      box: composeBarShown("auto", coarse, choice),
      keys: keysVisible && terminalKeysApply("auto", coarse, choice),
    })
    expect(state(true, "compose", true)).toEqual({ box: true, keys: true })
    expect(state(true, "compose", false)).toEqual({ box: true, keys: false })
    expect(state(true, "direct", true)).toEqual({ box: false, keys: true })
    expect(state(true, "direct", false)).toEqual({ box: false, keys: false })
  })

  // THE FINE-POINTER HALF, which is a different shape and has to be pinned as
  // one: the keys are on the keyboard already, so the row is offered only once
  // the box is up and it has no state of its own there. Whatever the setting
  // and whatever the choice, the two answers are the same answer.
  it("collapses onto the message box wherever the pointer is fine", () => {
    const modes: ComposeBarMode[] = ["auto", "always", "never"]
    const choices: TypingSurfaceChoice[] = [null, "compose", "direct"]
    for (const mode of modes) {
      for (const choice of choices) {
        expect(terminalKeysApply(mode, false, choice)).toBe(
          composeBarShown(mode, false, choice),
        )
      }
    }
  })

  // So a fine pointer reaches TWO of the four states, and keys-alone is not one
  // of them: that is the state a finger holds a virtual Ctrl in, and a laptop
  // has a real one.
  it("spans two states on a fine pointer, keys-alone not among them", () => {
    const state = (choice: TypingSurfaceChoice, keysVisible: boolean) => ({
      box: composeBarShown("auto", false, choice),
      keys: keysVisible && terminalKeysApply("auto", false, choice),
    })
    expect(state("compose", true)).toEqual({ box: true, keys: true })
    expect(state("compose", false)).toEqual({ box: true, keys: false })
    expect(state("direct", true)).toEqual({ box: false, keys: false })
    expect(state(null, true)).toEqual({ box: false, keys: false })
  })
})

describe("bottomBarSurvivesDirect", () => {
  // What decides whether the one-time way-back hint has anything to say: with a
  // key row still under the terminal, the bottom `⋯` is visibly carrying the
  // way back and a toast pointing at another menu is noise.
  it("survives on a phone whose keys are on", () => {
    expect(bottomBarSurvivesDirect("auto", true, true)).toBe(true)
  })

  it("does not survive once the keys are hidden by preference", () => {
    expect(bottomBarSurvivesDirect("auto", true, false)).toBe(false)
  })

  it("does not survive on a fine pointer, whose keys go with the box", () => {
    expect(bottomBarSurvivesDirect("auto", false, true)).toBe(false)
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

  it("lets always and never win over the device-local choice", () => {
    expect(composeBarShown("always", true, "direct")).toBe(true)
    expect(composeBarShown("always", false, "direct")).toBe(true)
    expect(composeBarShown("never", true, "compose")).toBe(false)
    expect(composeBarShown("never", false, "compose")).toBe(false)
  })
})

describe("inputMenuSurfaceSwitchOffered", () => {
  // NEVER INERT AND NEVER MISSING. The menu is the guaranteed way in and out of
  // the virtual input, so under `auto` it is offered on every device whatever
  // the pointer says and whatever has been chosen so far.
  it("is offered on every device under auto", () => {
    expect(inputMenuSurfaceSwitchOffered("auto")).toBe(true)
  })

  // The SETTING still wins: the switch cannot appear under always/never, where
  // pressing it would change nothing.
  it("is never offered outside auto", () => {
    expect(inputMenuSurfaceSwitchOffered("always")).toBe(false)
    expect(inputMenuSurfaceSwitchOffered("never")).toBe(false)
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
