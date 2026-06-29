import { describe, expect, it } from "vitest"

import { faviconHref, outlineFaviconDataUri, resolveFavicon } from "./favicon"

describe("resolveFavicon", () => {
  it("treats empty/blank/missing as the bundled default", () => {
    expect(resolveFavicon("")).toEqual({ kind: "default" })
    expect(resolveFavicon("   ")).toEqual({ kind: "default" })
    expect(resolveFavicon(undefined)).toEqual({ kind: "default" })
    expect(resolveFavicon(null)).toEqual({ kind: "default" })
  })

  it("reads a hex colour as an outline in that colour (normalized lowercase)", () => {
    expect(resolveFavicon("#863bff")).toEqual({ kind: "outline", color: "#863bff" })
    expect(resolveFavicon("#ABC")).toEqual({ kind: "outline", color: "#abc" })
  })

  it("maps a known colour name to a safe hex outline (case/space-insensitive)", () => {
    expect(resolveFavicon("violet")).toEqual({ kind: "outline", color: "#863bff" })
    expect(resolveFavicon("  Blue ")).toEqual({
      kind: "outline",
      color: expect.stringMatching(/^#[0-9a-f]{6}$/),
    })
  })

  it("accepts http(s) and absolute-path URLs as custom favicons", () => {
    expect(resolveFavicon("https://x.test/a.png")).toEqual({
      kind: "url",
      href: "https://x.test/a.png",
    })
    expect(resolveFavicon("http://x.test/a")).toEqual({
      kind: "url",
      href: "http://x.test/a",
    })
    expect(resolveFavicon("/icons/me.svg")).toEqual({
      kind: "url",
      href: "/icons/me.svg",
    })
  })

  it("rejects unsafe or unknown values, falling back to default", () => {
    expect(resolveFavicon("javascript:alert(1)")).toEqual({ kind: "default" })
    expect(resolveFavicon("data:image/svg+xml,<svg/>")).toEqual({ kind: "default" })
    // protocol-relative URL must not be treated as a custom favicon source
    expect(resolveFavicon("//evil.test/x.png")).toEqual({ kind: "default" })
    // backslash variants browsers normalize to a cross-origin host are rejected
    expect(resolveFavicon("/\\evil.test/x.png")).toEqual({ kind: "default" })
    expect(resolveFavicon("/\\\\evil.test")).toEqual({ kind: "default" })
    expect(resolveFavicon("notacolor")).toEqual({ kind: "default" })
    expect(resolveFavicon("#xyz")).toEqual({ kind: "default" })
    // an SVG-attribute breakout attempt is not a known colour → rejected
    expect(resolveFavicon('blue"/><script>')).toEqual({ kind: "default" })
  })
})

describe("faviconHref", () => {
  it("returns the bundled svg for the default", () => {
    expect(faviconHref("")).toEqual({ href: "/favicon.svg", svg: true })
  })

  it("returns a custom url untouched, not flagged as svg", () => {
    expect(faviconHref("https://x.test/a.png")).toEqual({
      href: "https://x.test/a.png",
      svg: false,
    })
  })

  it("returns an inline svg data uri for an outline colour", () => {
    const { href, svg } = faviconHref("#863bff")
    expect(svg).toBe(true)
    expect(href.startsWith("data:image/svg+xml,")).toBe(true)
  })
})

describe("outlineFaviconDataUri", () => {
  it("embeds the dux path stroked in the given colour with no fill", () => {
    const decoded = decodeURIComponent(
      outlineFaviconDataUri("#863bff").replace("data:image/svg+xml,", ""),
    )
    expect(decoded).toContain('stroke="#863bff"')
    expect(decoded).toContain('fill="none"')
    // the start of the extracted dux-logo path
    expect(decoded).toContain("M25.946")
  })

  it("clamps a non-hex colour to a safe value (defense in depth)", () => {
    // resolveFavicon never yields this, but a mistaken direct caller must not be
    // able to inject an attribute breakout into the generated SVG.
    const decoded = decodeURIComponent(
      outlineFaviconDataUri('#fff" onload="alert(1)').replace(
        "data:image/svg+xml,",
        "",
      ),
    )
    expect(decoded).not.toContain("onload")
    expect(decoded).toContain('stroke="#863bff"')
  })
})
