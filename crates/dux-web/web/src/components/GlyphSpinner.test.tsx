// @vitest-environment jsdom
import { readFileSync } from "node:fs"
import { join } from "node:path"
import { act, cleanup, render } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { GlyphSpinner } from "./GlyphSpinner"
import {
  GLYPH_SPINNER_CLASS,
  SPINNER_FRAMES,
  SPINNER_FRAME_MS,
} from "@/lib/spinnerFrames"
import { DUX_MONO_FILL_FAMILY } from "@/lib/terminalFont"
import { REDUCED_MOTION_QUERY } from "@/hooks/use-reduced-motion"
import { stubMatchMedia, type MatchMediaStub } from "@/test/matchMedia"

afterEach(() => {
  cleanup()
})

// The `.glyph-spinner` declaration block, comments stripped first. Asserting
// against the raw file would let the whole rule be commented out with every
// assertion below still green.
function slotRule(): string {
  // `import.meta.url` is an http URL under the jsdom environment, so the
  // stylesheet is reached from the vitest root instead.
  const css = readFileSync(join(process.cwd(), "src/index.css"), "utf-8").replace(
    /\/\*[\s\S]*?\*\//g,
    "",
  )
  const match = /\.glyph-spinner\s*\{([^}]*)\}/.exec(css)
  expect(match, "index.css declares a live .glyph-spinner rule").toBeTruthy()
  return (match as RegExpExecArray)[1]
}

describe("GlyphSpinner", () => {
  let media: MatchMediaStub

  beforeEach(() => {
    media = stubMatchMedia()
  })

  afterEach(() => {
    media.restore()
    vi.useRealTimers()
  })

  it("renders one arc frame inside the shared fixed-width slot", () => {
    const { container } = render(<GlyphSpinner />)
    const span = container.querySelector("span") as HTMLSpanElement
    expect(span.className.split(/\s+/)).toContain(GLYPH_SPINNER_CLASS)
    expect(SPINNER_FRAMES).toContain(span.textContent)
    // Decorative: the label beside it carries the meaning.
    expect(span.getAttribute("aria-hidden")).toBe("true")
  })

  it("keeps a caller's classes without losing the slot class", () => {
    const { container } = render(<GlyphSpinner className="text-primary" />)
    const span = container.querySelector("span") as HTMLSpanElement
    const classes = span.className.split(/\s+/)
    expect(classes).toContain(GLYPH_SPINNER_CLASS)
    expect(classes).toContain("text-primary")
  })

  it("pins the slot to a fixed width so the label cannot shift as frames cycle", () => {
    const rule = slotRule()
    // A fixed width plus centering is what makes the advance of the glyph
    // itself irrelevant. `flex: none` keeps it fixed inside the flex rows
    // every caller drops it into.
    expect(rule).toContain("width: 1em;")
    expect(rule).toContain("text-align: center;")
    expect(rule).toContain("flex: none;")
    expect(rule).toContain("display: inline-block;")
  })

  it("names the one bundled face that carries the arc glyphs, first", () => {
    const families = /font-family:\s*([^;]+);/.exec(slotRule())
    expect(families, ".glyph-spinner sets a font-family").toBeTruthy()
    const list = (families as RegExpExecArray)[1]
      .split(",")
      .map((part) => part.trim().replace(/^"|"$/g, ""))
    // First, so the arcs never reach a fallback face; a monospace tail behind
    // it for the case where the bundled face fails to load at all.
    expect(list[0]).toBe(DUX_MONO_FILL_FAMILY)
    expect(list).toContain("monospace")
  })

  it("advances a frame per tick of the TUI's cadence", () => {
    vi.useFakeTimers()
    const { container } = render(<GlyphSpinner />)
    const span = container.querySelector("span") as HTMLSpanElement
    const first = span.textContent
    act(() => {
      vi.advanceTimersByTime(SPINNER_FRAME_MS)
    })
    expect(span.textContent).not.toBe(first)
    act(() => {
      vi.advanceTimersByTime(SPINNER_FRAME_MS * (SPINNER_FRAMES.length - 1))
    })
    expect(span.textContent).toBe(first)
  })

  it("holds a single frame under prefers-reduced-motion", () => {
    vi.useFakeTimers()
    media.set(REDUCED_MOTION_QUERY, true)
    const { container } = render(<GlyphSpinner />)
    const span = container.querySelector("span") as HTMLSpanElement
    expect(span.textContent).toBe(SPINNER_FRAMES[0])
    act(() => {
      vi.advanceTimersByTime(SPINNER_FRAME_MS * 10)
    })
    // The glyph stays on screen; only the cycling stops.
    expect(span.textContent).toBe(SPINNER_FRAMES[0])
  })

  it("stops cycling when the preference flips on mid-spin", () => {
    vi.useFakeTimers()
    const { container } = render(<GlyphSpinner />)
    const span = container.querySelector("span") as HTMLSpanElement
    act(() => {
      vi.advanceTimersByTime(SPINNER_FRAME_MS)
    })
    act(() => {
      media.set(REDUCED_MOTION_QUERY, true)
    })
    expect(span.textContent).toBe(SPINNER_FRAMES[0])
    act(() => {
      vi.advanceTimersByTime(SPINNER_FRAME_MS * 10)
    })
    expect(span.textContent).toBe(SPINNER_FRAMES[0])
  })
})
