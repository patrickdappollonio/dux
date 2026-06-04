import { describe, expect, it } from "vitest"

import {
  getHighlighterReady,
  highlightLine,
  languageForPath,
  subscribeHighlighter,
} from "./highlight"

describe("languageForPath", () => {
  it("maps known extensions to highlight.js languages", () => {
    expect(languageForPath("src/main.rs")).toBe("rust")
    expect(languageForPath("App.tsx")).toBe("typescript")
    expect(languageForPath("config.toml")).toBe("ini")
  })

  it("lowercases the extension before lookup", () => {
    expect(languageForPath("README.MD")).toBe("markdown")
  })

  it("returns null for unknown or extension-less paths", () => {
    expect(languageForPath("Makefile")).toBeNull()
    expect(languageForPath("data.unknownext")).toBeNull()
  })

  it("does not require the lazy highlighter to be loaded", () => {
    // Resolution is a pure map lookup now — it must not throw before the
    // highlight.js chunk has loaded.
    expect(() => languageForPath("a.go")).not.toThrow()
  })
})

describe("highlightLine before the highlighter loads", () => {
  it("returns escaped plain text when not ready", () => {
    const out = highlightLine("const x = a < b && c > d", "typescript", false)
    expect(out).toBe("const x = a &lt; b &amp;&amp; c &gt; d")
  })

  it("returns an empty string for empty content", () => {
    expect(highlightLine("", "typescript", true)).toBe("")
  })

  it("escapes plain text when no language is resolved", () => {
    expect(highlightLine("<script>", null, true)).toBe("&lt;script&gt;")
  })
})

describe("highlighter lazy loading", () => {
  it("loads on subscription and highlights once ready", async () => {
    expect(getHighlighterReady()).toBe(false)

    await new Promise<void>((resolve) => {
      const unsubscribe = subscribeHighlighter(() => {
        unsubscribe()
        resolve()
      })
    })

    expect(getHighlighterReady()).toBe(true)
    // With the module loaded and a valid language, output is real (non-empty)
    // highlighted HTML — and source angle brackets stay escaped.
    const html = highlightLine("let x = 1 < 2", "rust", true)
    expect(html).toContain("&lt;")
    expect(html).not.toContain("<2")
  })
})
