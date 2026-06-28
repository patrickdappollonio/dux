import { describe, expect, it } from "vitest"

import {
  MACRO_SURFACE_OPTIONS,
  commitMacro,
  isMacroSurface,
  macroMatchesSurface,
  macroPayloadBytes,
  macroTextPreview,
  macrosForTarget,
  validateMacros,
} from "./macros"
import type { SelectedTarget } from "./store"
import type { MacroView } from "./types"

const agentTarget: SelectedTarget = { kind: "agent", sessionId: "s1" }
const terminalTarget: SelectedTarget = {
  kind: "terminal",
  terminalId: "t1",
  sessionId: "s1",
}

function macro(name: string, surface: MacroView["surface"], text = "x"): MacroView {
  return { name, text, surface }
}

describe("macroMatchesSurface", () => {
  // All six (surface × target-kind) combos, mirroring the Rust
  // `macro_matches_surface` cases exactly.
  it("both is available on every target kind", () => {
    expect(macroMatchesSurface("both", "agent")).toBe(true)
    expect(macroMatchesSurface("both", "terminal")).toBe(true)
  })

  it("agent only matches an agent target", () => {
    expect(macroMatchesSurface("agent", "agent")).toBe(true)
    expect(macroMatchesSurface("agent", "terminal")).toBe(false)
  })

  it("terminal only matches a terminal target", () => {
    expect(macroMatchesSurface("terminal", "terminal")).toBe(true)
    expect(macroMatchesSurface("terminal", "agent")).toBe(false)
  })
})

describe("macrosForTarget", () => {
  const macros = [
    macro("a", "agent"),
    macro("t", "terminal"),
    macro("b", "both"),
  ]

  it("returns agent + both for an agent target, in config order", () => {
    expect(macrosForTarget(macros, agentTarget).map((m) => m.name)).toEqual([
      "a",
      "b",
    ])
  })

  it("returns terminal + both for a terminal target, in config order", () => {
    expect(macrosForTarget(macros, terminalTarget).map((m) => m.name)).toEqual([
      "t",
      "b",
    ])
  })

  it("returns [] when there are no macros at all", () => {
    expect(macrosForTarget([], agentTarget)).toEqual([])
  })

  it("returns [] when none match the target surface", () => {
    expect(macrosForTarget([macro("t", "terminal")], agentTarget)).toEqual([])
  })
})

describe("validateMacros", () => {
  it("accepts a valid set", () => {
    expect(
      validateMacros([macro("a", "agent"), macro("b", "both")]),
    ).toBeNull()
  })

  it("rejects an empty name", () => {
    expect(validateMacros([macro("", "agent")])).toMatch(/needs a name/)
    expect(validateMacros([macro("   ", "agent")])).toMatch(/needs a name/)
  })

  it("rejects duplicate names (after trim)", () => {
    expect(
      validateMacros([macro("dup", "agent"), macro(" dup ", "both")]),
    ).toMatch(/Duplicate macro name/)
  })

  it("rejects empty text", () => {
    expect(validateMacros([macro("a", "agent", "")])).toMatch(/needs some text/)
  })

  it("rejects an unknown surface", () => {
    const bad = { name: "a", text: "x", surface: "bogus" } as unknown as MacroView
    expect(validateMacros([bad])).toMatch(/unknown surface/)
  })
})

describe("isMacroSurface", () => {
  it("narrows the known values", () => {
    expect(isMacroSurface("agent")).toBe(true)
    expect(isMacroSurface("terminal")).toBe(true)
    expect(isMacroSurface("both")).toBe(true)
    expect(isMacroSurface("nope")).toBe(false)
  })
})

describe("MACRO_SURFACE_OPTIONS", () => {
  it("covers exactly the three surfaces in config-comment order", () => {
    expect(MACRO_SURFACE_OPTIONS.map((o) => o.value)).toEqual([
      "agent",
      "terminal",
      "both",
    ])
  })
})

describe("commitMacro", () => {
  it("renaming entry 0 keeps it at index 0", () => {
    const prev = [macro("A", "agent"), macro("B", "agent"), macro("C", "agent")]
    const next = commitMacro(prev, 0, macro("Renamed", "agent"))
    expect(next.map((m) => m.name)).toEqual(["Renamed", "B", "C"])
  })

  it("editing a middle entry replaces it in place", () => {
    const prev = [macro("A", "agent"), macro("B", "agent"), macro("C", "agent")]
    const next = commitMacro(prev, 1, macro("Renamed", "agent"))
    expect(next.map((m) => m.name)).toEqual(["A", "Renamed", "C"])
  })

  it("adding appends to the end, preserving order", () => {
    const prev = [macro("A", "agent"), macro("B", "agent")]
    const next = commitMacro(prev, "new", macro("C", "agent"))
    expect(next.map((m) => m.name)).toEqual(["A", "B", "C"])
  })

  it("adding to an empty list yields a single entry", () => {
    expect(commitMacro([], "new", macro("A", "agent")).map((m) => m.name)).toEqual([
      "A",
    ])
  })

  it("does not mutate the input array", () => {
    const prev = [macro("A", "agent"), macro("B", "agent")]
    const snapshot = prev.map((m) => m.name)
    commitMacro(prev, 0, macro("Renamed", "agent"))
    commitMacro(prev, "new", macro("C", "agent"))
    expect(prev.map((m) => m.name)).toEqual(snapshot)
  })
})

describe("macroTextPreview", () => {
  it("collapses newlines to a single-line preview", () => {
    expect(macroTextPreview("a\nb")).toBe("a ⏎ b")
    expect(macroTextPreview("a\r\nb")).toBe("a ⏎ b")
    expect(macroTextPreview("a\rb")).toBe("a ⏎ b")
  })

  it("truncates by character (not byte) and appends an ellipsis", () => {
    // Multi-byte glyphs: truncation must split on char boundaries, never bytes.
    const text = "🦆".repeat(10)
    const out = macroTextPreview(text, 4)
    expect([...out]).toHaveLength(5) // 4 ducks + ellipsis
    expect(out.endsWith("…")).toBe(true)
  })

  it("leaves short text untouched", () => {
    expect(macroTextPreview("short", 80)).toBe("short")
  })
})

describe("macroPayloadBytes", () => {
  // An exact port of `dux_core::macros::macro_payload_bytes`; these mirror the
  // Rust unit tests so the two surfaces stay byte-for-byte identical.
  const bytes = (s: string) => Array.from(macroPayloadBytes(s))
  const ALT_ENTER = [0x1b, 0x0d] // ESC, CR

  it("passes plain text through byte-for-byte", () => {
    expect(bytes("abc")).toEqual([0x61, 0x62, 0x63])
  })

  it("translates LF, CR, and CRLF each to one Alt+Enter", () => {
    expect(bytes("a\nb")).toEqual([0x61, ...ALT_ENTER, 0x62])
    expect(bytes("a\rb")).toEqual([0x61, ...ALT_ENTER, 0x62])
    expect(bytes("a\r\nb")).toEqual([0x61, ...ALT_ENTER, 0x62])
  })

  it("translates each newline in a multi-line macro and leading/trailing ones", () => {
    expect(bytes("a\nb\nc")).toEqual([
      0x61,
      ...ALT_ENTER,
      0x62,
      ...ALT_ENTER,
      0x63,
    ])
    expect(bytes("\na\n")).toEqual([...ALT_ENTER, 0x61, ...ALT_ENTER])
  })

  it("preserves multi-byte UTF-8 glyphs intact", () => {
    // The duck emoji is 4 UTF-8 bytes; none of them is a newline byte.
    expect(bytes("🦆")).toEqual(Array.from(new TextEncoder().encode("🦆")))
  })
})
