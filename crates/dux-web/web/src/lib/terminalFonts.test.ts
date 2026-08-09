// Pins the wiring between the bundled web terminal font stack (`terminalFont.ts`)
// and its two real assets: the `@font-face` declarations in index.css and the
// woff2 files themselves. Mirrors `components/blockArtFont.test.ts`, which does
// the same for the "Dux Blocks" wordmark font.
import { readFileSync } from "node:fs"
import { fileURLToPath } from "node:url"
import { describe, expect, it } from "vitest"

import {
  DUX_MONO_FAMILY,
  DUX_MONO_SYMBOLS_FAMILY,
  DUX_TERMINAL_FONT_STACK,
  UNICODE_RANGES,
} from "./terminalFont"

const read = (rel: string) =>
  readFileSync(fileURLToPath(new URL(rel, import.meta.url)), "utf-8")

describe("Dux Mono / Dux Mono Symbols font wiring", () => {
  it("index.css declares both bundled font-families", () => {
    const css = read("../index.css")
    expect(css).toContain(`font-family: "${DUX_MONO_FAMILY}";`)
    expect(css).toContain(`font-family: "${DUX_MONO_SYMBOLS_FAMILY}";`)
  })

  it("index.css's unicode-range on the symbols face matches the exported constant", () => {
    const css = read("../index.css")
    expect(css).toContain(`unicode-range: ${UNICODE_RANGES};`)
  })

  it("DUX_TERMINAL_FONT_STACK references both bundled family constants", () => {
    expect(DUX_TERMINAL_FONT_STACK).toContain(`"${DUX_MONO_SYMBOLS_FAMILY}"`)
    expect(DUX_TERMINAL_FONT_STACK).toContain(`"${DUX_MONO_FAMILY}"`)
    // The symbols face comes first, so structural glyphs never fall through
    // to a system font before dux's own verified single-cell-advance face.
    expect(DUX_TERMINAL_FONT_STACK.indexOf(DUX_MONO_SYMBOLS_FAMILY)).toBeLessThan(
      DUX_TERMINAL_FONT_STACK.indexOf(`"${DUX_MONO_FAMILY}"`),
    )
  })

  it("the three bundled woff2 assets are real woff2 files", () => {
    for (const name of [
      "dux-mono-regular.woff2",
      "dux-mono-bold.woff2",
      "dux-mono-symbols.woff2",
    ]) {
      const bytes = readFileSync(
        fileURLToPath(new URL(`../assets/fonts/${name}`, import.meta.url)),
      )
      expect(bytes.subarray(0, 4).toString("latin1"), name).toBe("wOF2")
    }
  })
})
