import { describe, expect, it } from "vitest"

import {
  DUX_TERMINAL_FONT_STACK,
  TERMINAL_FONT_PRELOADS,
  clampTerminalFontSize,
  terminalFontFamily,
} from "./terminalFont"

describe("DUX_TERMINAL_FONT_STACK", () => {
  it("lists the symbols face, the text face, the fill face, then the system tail", () => {
    expect(DUX_TERMINAL_FONT_STACK).toBe(
      '"Dux Mono Symbols", "Dux Mono", "Dux Mono Fill", ui-monospace, SFMono-Regular, Menlo, monospace',
    )
  })
})

describe("TERMINAL_FONT_PRELOADS", () => {
  // The list exists so the eager load names one family per entry. A face
  // missing from it is not fetched before xterm measures its cell advances,
  // and xterm caches the fallback advance it measured instead, which is the
  // row-drift bug the capture harness hit.
  it("names all four bundled faces, the bold weight included", () => {
    const entries = TERMINAL_FONT_PRELOADS.map(
      (preload) => `${preload.weight ? `${preload.weight} ` : ""}${preload.family}`,
    )
    expect(entries).toEqual([
      "Dux Mono",
      "bold Dux Mono",
      "Dux Mono Symbols",
      "Dux Mono Fill",
    ])
  })

  it("gives every entry a non-empty sample", () => {
    for (const preload of TERMINAL_FONT_PRELOADS) {
      expect(preload.sample.length, preload.family).toBeGreaterThan(0)
    }
  })
})

describe("terminalFontFamily", () => {
  it("returns the bundled default stack for null", () => {
    expect(terminalFontFamily(null)).toBe(DUX_TERMINAL_FONT_STACK)
  })

  it("returns the bundled default stack for undefined", () => {
    expect(terminalFontFamily(undefined)).toBe(DUX_TERMINAL_FONT_STACK)
  })

  it("returns the bundled default stack for an empty string", () => {
    expect(terminalFontFamily("")).toBe(DUX_TERMINAL_FONT_STACK)
  })

  it("returns the bundled default stack for a whitespace-only string", () => {
    expect(terminalFontFamily("   ")).toBe(DUX_TERMINAL_FONT_STACK)
  })

  it("prepends a trimmed custom family ahead of the default stack", () => {
    expect(terminalFontFamily("Fira Code")).toBe(
      `Fira Code, ${DUX_TERMINAL_FONT_STACK}`,
    )
  })

  it("trims surrounding whitespace from the custom family", () => {
    expect(terminalFontFamily("  Fira Code  ")).toBe(
      `Fira Code, ${DUX_TERMINAL_FONT_STACK}`,
    )
  })

  it("preserves a quoted custom family value as given", () => {
    expect(terminalFontFamily('"Cascadia Code"')).toBe(
      `"Cascadia Code", ${DUX_TERMINAL_FONT_STACK}`,
    )
  })

  it("allows a comma-separated font list", () => {
    expect(terminalFontFamily('"Fira Code", Consolas')).toBe(
      `"Fira Code", Consolas, ${DUX_TERMINAL_FONT_STACK}`,
    )
  })

  describe("sanitizes a value that would otherwise break out of the CSS declaration", () => {
    it("strips a semicolon that could terminate font-family and start a new declaration", () => {
      const value = terminalFontFamily('Fira Code"; color: red; --x: "')
      expect(value).not.toContain(";")
      expect(value.startsWith("Fira Code")).toBe(true)
    })

    it("strips curly braces that could close and reopen a CSS rule", () => {
      const value = terminalFontFamily("Fira Code} body { display: none")
      expect(value).not.toContain("{")
      expect(value).not.toContain("}")
    })

    it("strips an unbalanced quote so it cannot leave the declaration open-ended", () => {
      // A lone unescaped quote is stripped rather than preserved unbalanced;
      // the character itself is in the allowlist, but this pins that an
      // unbalanced one does not, on its own, corrupt the surrounding CSS
      // (there is nothing after it in the allowlisted output that could
      // reopen a new rule).
      const value = terminalFontFamily('Fira Code"')
      expect(value).toBe(`Fira Code", ${DUX_TERMINAL_FONT_STACK}`)
    })

    it("strips a newline so the value cannot span multiple CSS lines", () => {
      const value = terminalFontFamily("Fira Code\n} body { display: none")
      expect(value).not.toContain("\n")
      expect(value).not.toContain("{")
    })

    it("strips angle brackets and backslashes", () => {
      const value = terminalFontFamily("Fira Code</style><script>alert(1)</script>")
      expect(value).not.toMatch(/[<>\\]/)
    })

    it("caps a 10k-character value to 200 characters", () => {
      const huge = "a".repeat(10_000)
      const value = terminalFontFamily(huge)
      // 200 sanitized characters plus ", " plus the bundled stack.
      expect(value).toBe(`${"a".repeat(200)}, ${DUX_TERMINAL_FONT_STACK}`)
    })

    it("caps a 10k-character value full of characters that would break out of the declaration", () => {
      const huge = ";{}<>\\\n".repeat(2000)
      const value = terminalFontFamily(huge)
      // Every character in the input is disallowed, so the sanitized (and
      // capped) result is empty and the family degrades to the bundled stack.
      expect(value).toBe(DUX_TERMINAL_FONT_STACK)
    })

    it("falls back to the bundled stack when sanitizing removes everything", () => {
      expect(terminalFontFamily(";{}<>\\")).toBe(DUX_TERMINAL_FONT_STACK)
    })
  })
})

describe("clampTerminalFontSize", () => {
  it("defaults to 14 for null", () => {
    expect(clampTerminalFontSize(null)).toBe(14)
  })

  it("defaults to 14 for undefined", () => {
    expect(clampTerminalFontSize(undefined)).toBe(14)
  })

  it("defaults to 14 for NaN", () => {
    expect(clampTerminalFontSize(NaN)).toBe(14)
  })

  it("defaults to 14 for a non-numeric string", () => {
    expect(clampTerminalFontSize("large")).toBe(14)
  })

  it("defaults to 14 for Infinity", () => {
    expect(clampTerminalFontSize(Infinity)).toBe(14)
  })

  it("rounds a fractional value to the nearest integer", () => {
    expect(clampTerminalFontSize(15.6)).toBe(16)
  })

  it("passes through an in-range integer", () => {
    expect(clampTerminalFontSize(18)).toBe(18)
  })

  it("degrades a value below the floor of 8 to the default of 14", () => {
    expect(clampTerminalFontSize(1)).toBe(14)
  })

  it("degrades a value of 2 to the default of 14", () => {
    expect(clampTerminalFontSize(2)).toBe(14)
  })

  it("passes through the floor of 8", () => {
    expect(clampTerminalFontSize(8)).toBe(8)
  })

  it("degrades a value above the ceiling of 32 to the default of 14", () => {
    expect(clampTerminalFontSize(500)).toBe(14)
  })

  it("degrades 99 to the default of 14", () => {
    expect(clampTerminalFontSize(99)).toBe(14)
  })

  it("passes through the ceiling of 32", () => {
    expect(clampTerminalFontSize(32)).toBe(32)
  })

  it("accepts a numeric string", () => {
    expect(clampTerminalFontSize("20")).toBe(20)
  })
})
