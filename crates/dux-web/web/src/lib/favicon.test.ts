import { describe, expect, it, vi } from "vitest"

vi.mock("sonner", () => ({ toast: { info: vi.fn() } }))

import { toast } from "sonner"

import {
  FAVICON_COLORS,
  applyFavicon,
  duckFaviconDataUri,
  faviconHref,
  faviconIsLegacy,
  resolveFavicon,
} from "./favicon"

// The curated set MUST equal the Rust `CURATED_FAVICON_COLORS` in
// `crates/dux-core/src/wire.rs` (a cross-language pin keeps them in sync). NO
// yellow — the default/unset favicon is the full-colour yellow duck.
const CURATED_NAMES = [
  "violet",
  "blue",
  "sky",
  "cyan",
  "teal",
  "green",
  "amber",
  "orange",
  "red",
  "pink",
  "rose",
]

describe("FAVICON_COLORS", () => {
  it("has exactly the 11 curated colour names (must match Rust CURATED_FAVICON_COLORS)", () => {
    expect(Object.keys(FAVICON_COLORS).sort()).toEqual([...CURATED_NAMES].sort())
  })

  it("maps every name to a plain #rrggbb hex", () => {
    for (const hex of Object.values(FAVICON_COLORS)) {
      expect(hex).toMatch(/^#[0-9a-f]{6}$/)
    }
  })

  it("does not include yellow", () => {
    expect(FAVICON_COLORS.yellow).toBeUndefined()
  })
})

describe("resolveFavicon", () => {
  it("treats empty/blank/missing as the bundled default", () => {
    expect(resolveFavicon("")).toEqual({ kind: "default" })
    expect(resolveFavicon("   ")).toEqual({ kind: "default" })
    expect(resolveFavicon(undefined)).toEqual({ kind: "default" })
    expect(resolveFavicon(null)).toEqual({ kind: "default" })
  })

  it("maps a curated colour name to a tinted duck (case/space-insensitive)", () => {
    expect(resolveFavicon("violet")).toEqual({ kind: "tinted", color: "#863bff" })
    expect(resolveFavicon("  Blue ")).toEqual({
      kind: "tinted",
      color: expect.stringMatching(/^#[0-9a-f]{6}$/),
    })
  })

  it("degrades a legacy hex value to the default duck", () => {
    // The hex/URL branches were removed; a pre-existing hex config now falls back
    // gracefully rather than being interpolated into the SVG.
    expect(resolveFavicon("#863bff")).toEqual({ kind: "default" })
    expect(resolveFavicon("#ABC")).toEqual({ kind: "default" })
  })

  it("degrades a legacy URL value to the default duck", () => {
    expect(resolveFavicon("https://x.test/a.png")).toEqual({ kind: "default" })
    expect(resolveFavicon("http://x.test/a")).toEqual({ kind: "default" })
    expect(resolveFavicon("/icons/me.svg")).toEqual({ kind: "default" })
    // protocol-relative and backslash-host breakout attempts are just non-curated
    // values now → default, no special-casing needed.
    expect(resolveFavicon("//evil.test/x.png")).toEqual({ kind: "default" })
    expect(resolveFavicon("/\\evil.test/x.png")).toEqual({ kind: "default" })
  })

  it("degrades a dropped colour name / unknown value to the default duck", () => {
    // purple/lime/slate/gray/white/black were dropped from the curated set.
    expect(resolveFavicon("purple")).toEqual({ kind: "default" })
    expect(resolveFavicon("yellow")).toEqual({ kind: "default" })
    expect(resolveFavicon("notacolor")).toEqual({ kind: "default" })
    // an SVG-attribute breakout attempt is not a curated name → default
    expect(resolveFavicon('blue"/><script>')).toEqual({ kind: "default" })
  })
})

describe("faviconIsLegacy", () => {
  it("flags a non-empty, non-curated value as legacy", () => {
    expect(faviconIsLegacy("#863bff")).toBe(true)
    expect(faviconIsLegacy("https://x.test/a.png")).toBe(true)
    expect(faviconIsLegacy("purple")).toBe(true)
  })

  it("does not flag empty or a curated name", () => {
    expect(faviconIsLegacy("")).toBe(false)
    expect(faviconIsLegacy("   ")).toBe(false)
    expect(faviconIsLegacy(null)).toBe(false)
    expect(faviconIsLegacy("violet")).toBe(false)
    expect(faviconIsLegacy("  Blue ")).toBe(false)
  })
})

describe("faviconHref", () => {
  it("returns the bundled png for the default (not flagged as svg)", () => {
    expect(faviconHref("")).toEqual({ href: "/favicon.png", svg: false })
  })

  it("returns the bundled png for a legacy value", () => {
    expect(faviconHref("https://x.test/a.png")).toEqual({
      href: "/favicon.png",
      svg: false,
    })
  })

  it("returns an inline svg data uri for a curated colour", () => {
    const { href, svg } = faviconHref("violet")
    expect(svg).toBe(true)
    expect(href.startsWith("data:image/svg+xml,")).toBe(true)
  })
})

describe("duckFaviconDataUri", () => {
  it("fills the duck path with the given colour", () => {
    const decoded = decodeURIComponent(
      duckFaviconDataUri("#863bff").replace("data:image/svg+xml,", ""),
    )
    expect(decoded).toContain('fill="#863bff"')
    expect(decoded).toContain('fill-rule="evenodd"')
    // the start of the traced duck path
    expect(decoded).toContain("M 276 7.166")
  })

  it("clamps a non-curated colour to a safe value (defense in depth)", () => {
    // resolveFavicon never yields this, but a mistaken direct caller must not be
    // able to inject an attribute breakout into the generated SVG.
    const decoded = decodeURIComponent(
      duckFaviconDataUri('#fff" onload="alert(1)').replace(
        "data:image/svg+xml,",
        "",
      ),
    )
    expect(decoded).not.toContain("onload")
    // falls back to a curated hex fill
    expect(decoded).toMatch(/fill="#[0-9a-f]{6}"/)
  })

  it("clamps a non-curated but well-formed hex to a safe value", () => {
    // A hex that isn't in the curated set is still rejected (only curated fills
    // may reach the SVG), so it can never smuggle an arbitrary colour in.
    const decoded = decodeURIComponent(
      duckFaviconDataUri("#123456").replace("data:image/svg+xml,", ""),
    )
    expect(decoded).not.toContain("#123456")
  })
})

describe("applyFavicon without a DOM", () => {
  it("is a no-op that does not throw or toast when document is absent", () => {
    // This file runs in the default node environment, where `document` is absent
    // (mirroring the store's Node test environment). applyFavicon must self-guard
    // and return before touching the DOM or the migration toast.
    expect(typeof document).toBe("undefined")
    expect(() => applyFavicon("#863bff")).not.toThrow()
    expect(vi.mocked(toast.info)).not.toHaveBeenCalled()
  })
})
