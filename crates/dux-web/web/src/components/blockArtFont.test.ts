// The block-glyph "dux" wordmark must render every glyph from ONE bundled font.
//
// The art mixes U+2591 (light shade) and U+2588 (full block) with ASCII spaces.
// Many system monospace fonts lack the Block Elements range, so the browser
// substitutes those glyphs from a DIFFERENT fallback font with a different
// advance width and the lines shear device-dependently (seen on a real Android
// phone). The fix is the bundled "Dux Blocks" font (a DejaVu Sans Mono subset:
// Basic Latin + Block Elements, one advance width) resolved FIRST at all three
// art sites. These tests pin the art itself, the wiring, and the byte-identity
// of the two copies of the font (the vite asset and offline.html's data URI).
import { readFileSync } from "node:fs"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

const read = (rel: string) =>
  readFileSync(fileURLToPath(new URL(rel, import.meta.url)), "utf-8")

// The wordmark, trailing spaces stripped (the sites differ only in trailing
// padding: Welcome pads to a 33-column rectangle so the <pre> centers cleanly).
const ART_LINES = [
  "       ░██",
  "       ░██",
  " ░████████ ░██    ░██ ░██    ░██",
  "░██    ░██ ░██    ░██  ░██  ░██",
  "░██    ░██ ░██    ░██   ░█████",
  "░██   ░███ ░██   ░███  ░██  ░██",
  " ░█████░██  ░█████░██ ░██    ░██",
]

// Every codepoint the art may use. If this set grows, the bundled font subset
// (Basic Latin + Block Elements U+2580-259F) must be re-verified to cover it.
const ALLOWED = new Set([" ", "░", "█"])

function expectArt(raw: string) {
  const lines = raw.split("\n")
  expect(lines.map((l) => l.replace(/ +$/, ""))).toEqual(ART_LINES)
  for (const ch of raw.replace(/\n/g, "")) {
    expect(ALLOWED.has(ch), `unexpected art codepoint U+${ch.codePointAt(0)!.toString(16)}`).toBe(true)
  }
}

describe("block art constants are unchanged", () => {
  it("Welcome.tsx TEXT_LOGO", () => {
    const src = read("./Welcome.tsx")
    const m = src.match(/const TEXT_LOGO = \[([\s\S]*?)\]\.join\("\\n"\)/)
    expect(m).not.toBeNull()
    const lines = [...m![1].matchAll(/"([^"]*)"/g)].map((x) => x[1])
    expectArt(lines.join("\n"))
    // The padding contract Welcome documents: a clean 33-column rectangle.
    for (const line of lines) expect([...line].length).toBe(33)
  })

  it("OfflineOverlay.tsx DUX_ART", () => {
    const src = read("./OfflineOverlay.tsx")
    const m = src.match(/const DUX_ART = `([\s\S]*?)`/)
    expect(m).not.toBeNull()
    expectArt(m![1])
  })

  it("offline.html <pre>", () => {
    const html = read("../../public/offline.html")
    const m = html.match(/<pre aria-label="dux">([\s\S]*?)<\/pre>/)
    expect(m).not.toBeNull()
    expectArt(m![1])
  })
})

describe("Dux Blocks font wiring", () => {
  it("index.css declares the @font-face and the font-blocks token", () => {
    const css = read("../index.css")
    expect(css).toContain('font-family: "Dux Blocks";')
    expect(css).toContain('url("./assets/fonts/dux-blocks.woff2") format("woff2")')
    // Bundled font first, system mono only as fallback.
    expect(css).toMatch(/--font-blocks:\s*"Dux Blocks",\s*ui-monospace/)
  })

  it("both React art sites render through font-blocks, not bare font-mono", () => {
    for (const file of ["./Welcome.tsx", "./OfflineOverlay.tsx"]) {
      const src = read(file)
      expect(src, `${file} art <pre> must use font-blocks`).toContain(
        "font-blocks text-[11px] leading-[1.15]",
      )
      expect(src, `${file} art <pre> must not fall back to the system mono stack first`).not.toContain(
        "font-mono text-[11px]",
      )
    }
  })

  it("the woff2 asset is a real woff2 with its license alongside", () => {
    const woff2 = readFileSync(
      fileURLToPath(new URL("../assets/fonts/dux-blocks.woff2", import.meta.url)),
    )
    expect(woff2.subarray(0, 4).toString("latin1")).toBe("wOF2")
    const license = read("../assets/fonts/dux-blocks.LICENSE")
    expect(license).toContain("Bitstream Vera")
    expect(license).toContain("DejaVu")
  })
})

describe("offline.html works with the network gone", () => {
  it("embeds the font as a data URI byte-identical to the bundled asset", () => {
    const html = read("../../public/offline.html")
    // The offline page may not fetch anything: the service worker caches only
    // offline.html itself and only intercepts navigations, so a font URL would
    // 404 exactly when this page shows. The font must ride inside the HTML.
    const m = html.match(/url\(data:font\/woff2;base64,([A-Za-z0-9+/=]+)\)/)
    expect(m).not.toBeNull()
    const embedded = Buffer.from(m![1], "base64")
    const asset = readFileSync(
      fileURLToPath(new URL("../assets/fonts/dux-blocks.woff2", import.meta.url)),
    )
    expect(embedded.equals(asset)).toBe(true)
    expect(html).toContain('font-family: "Dux Blocks";')
    expect(html).toContain('font-family: "Dux Blocks", ui-monospace')
    // No other external URL may sneak into the page's CSS.
    expect(html).not.toMatch(/url\((?!data:)/)
  })
})
