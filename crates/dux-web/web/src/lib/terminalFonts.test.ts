// Pins the wiring between the bundled web terminal font stack (`terminalFont.ts`)
// and its real assets: the `@font-face` declarations in index.css and the
// woff2 files themselves. Mirrors `components/blockArtFont.test.ts`, which does
// the same for the "Dux Blocks" wordmark font.
import { readFileSync } from "node:fs"
import { fileURLToPath } from "node:url"
import { brotliDecompressSync } from "node:zlib"
import { describe, expect, it } from "vitest"

import {
  DUX_MONO_FAMILY,
  DUX_MONO_FILL_FAMILY,
  DUX_MONO_SYMBOLS_FAMILY,
  DUX_TERMINAL_FONT_STACK,
  FILL_UNICODE_RANGES,
  TERMINAL_FONT_PRELOADS,
  UNICODE_RANGES,
} from "./terminalFont"

const read = (rel: string) =>
  readFileSync(fileURLToPath(new URL(rel, import.meta.url)), "utf-8")

const readBytes = (rel: string) =>
  readFileSync(fileURLToPath(new URL(rel, import.meta.url)))

// Removes `/* ... */` comments. Asserting with a bare `toContain` against the
// raw file text was a real hole: commenting the whole "Dux Mono Fill"
// `@font-face` block out left every assertion here satisfied, so the feature
// could be deleted with the suite green. Everything below asserts against the
// stripped text.
function stripCssComments(css: string): string {
  return css.replace(/\/\*[\s\S]*?\*\//g, "")
}

// The body of every `@font-face` block, comments already gone. `@font-face`
// bodies are flat declaration lists with no nested braces, so a non-greedy
// `[^}]*` is an exact reading of the block rather than an approximation.
function fontFaceBlocks(css: string): string[] {
  return Array.from(
    stripCssComments(css).matchAll(/@font-face\s*\{([^}]*)\}/g),
    (m) => m[1],
  )
}

// The first `@font-face` block declaring `family`. Matching the trailing `";`
// keeps "Dux Mono" from selecting the "Dux Mono Symbols" or "Dux Mono Fill"
// block.
function fontFaceBlockFor(css: string, family: string): string | undefined {
  return fontFaceBlocks(css).find((body) =>
    body.includes(`font-family: "${family}";`),
  )
}

describe("Dux Mono / Dux Mono Symbols / Dux Mono Fill font wiring", () => {
  it("index.css declares a live @font-face block for all three bundled families", () => {
    const css = read("../index.css")
    for (const family of [
      DUX_MONO_FAMILY,
      DUX_MONO_SYMBOLS_FAMILY,
      DUX_MONO_FILL_FAMILY,
    ]) {
      expect(fontFaceBlockFor(css, family), family).toBeDefined()
    }
  })

  it("each unicode-range sits inside its own family's @font-face block", () => {
    const css = read("../index.css")
    // Block-scoped on purpose: asserted against the whole file, swapping the
    // symbols and fill range strings between the two blocks passes both
    // assertions while every restricted glyph resolves to the wrong face.
    const symbols = fontFaceBlockFor(css, DUX_MONO_SYMBOLS_FAMILY)
    expect(symbols).toContain(`unicode-range: ${UNICODE_RANGES};`)
    const fill = fontFaceBlockFor(css, DUX_MONO_FILL_FAMILY)
    expect(fill).toContain(`unicode-range: ${FILL_UNICODE_RANGES};`)
  })

  it("each @font-face block points at its own woff2 asset", () => {
    const css = read("../index.css")
    expect(fontFaceBlockFor(css, DUX_MONO_SYMBOLS_FAMILY)).toContain(
      "dux-mono-symbols.woff2",
    )
    expect(fontFaceBlockFor(css, DUX_MONO_FILL_FAMILY)).toContain(
      "dux-mono-fill.woff2",
    )
    expect(fontFaceBlockFor(css, DUX_MONO_FAMILY)).toContain(
      "dux-mono-regular.woff2",
    )
  })

  it("DUX_TERMINAL_FONT_STACK references all three bundled family constants", () => {
    expect(DUX_TERMINAL_FONT_STACK).toContain(`"${DUX_MONO_SYMBOLS_FAMILY}"`)
    expect(DUX_TERMINAL_FONT_STACK).toContain(`"${DUX_MONO_FAMILY}"`)
    expect(DUX_TERMINAL_FONT_STACK).toContain(`"${DUX_MONO_FILL_FAMILY}"`)
    // The symbols face comes first, so structural glyphs never fall through
    // to a system font before dux's own verified single-cell-advance face.
    expect(DUX_TERMINAL_FONT_STACK.indexOf(DUX_MONO_SYMBOLS_FAMILY)).toBeLessThan(
      DUX_TERMINAL_FONT_STACK.indexOf(`"${DUX_MONO_FAMILY}"`),
    )
  })

  it("orders the stack symbols, then text, then fill, then the system tail", () => {
    const symbols = DUX_TERMINAL_FONT_STACK.indexOf(`"${DUX_MONO_SYMBOLS_FAMILY}"`)
    const text = DUX_TERMINAL_FONT_STACK.indexOf(`"${DUX_MONO_FAMILY}",`)
    const fill = DUX_TERMINAL_FONT_STACK.indexOf(`"${DUX_MONO_FILL_FAMILY}"`)
    const tail = DUX_TERMINAL_FONT_STACK.indexOf("ui-monospace")
    // Symbols BEFORE text: structural glyphs come from the verified
    // single-cell-advance face. Text BEFORE fill: the fill face's declared
    // range takes in General Punctuation the text face also covers, so the
    // text face must win there and ordinary punctuation must not switch
    // typeface mid-sentence. Fill BEFORE the tail: it is dux's own backstop
    // and only what it also lacks may reach the system fonts.
    expect(symbols).toBeGreaterThanOrEqual(0)
    expect(symbols).toBeLessThan(text)
    expect(text).toBeLessThan(fill)
    expect(fill).toBeLessThan(tail)
  })

  it("the four bundled woff2 assets are real woff2 files", () => {
    for (const name of [
      "dux-mono-regular.woff2",
      "dux-mono-bold.woff2",
      "dux-mono-symbols.woff2",
      "dux-mono-fill.woff2",
    ]) {
      const bytes = readBytes(`../assets/fonts/${name}`)
      expect(bytes.subarray(0, 4).toString("latin1"), name).toBe("wOF2")
    }
  })
})

// Parses a CSS `unicode-range` value ("U+2190-21FF, U+2800-28FF, ...") into
// inclusive [start, end] pairs. Only the two forms dux writes are accepted; a
// wildcard form (`U+27??`) throws rather than being skipped, so a range style
// this reader cannot see through can never pass the membership test below by
// covering nothing.
function parseUnicodeRange(value: string): [number, number][] {
  return value.split(",").map((part) => {
    const token = part.trim()
    const match = /^U\+([0-9A-Fa-f]+)(?:-([0-9A-Fa-f]+))?$/.exec(token)
    if (!match) throw new Error(`unhandled unicode-range token: ${token}`)
    const start = parseInt(match[1], 16)
    return [start, match[2] ? parseInt(match[2], 16) : start]
  })
}

describe("terminal font preload samples", () => {
  // The real drift guard. A restricted face is only fetched by
  // `document.fonts.load` when the sample text contains a code point its
  // `unicode-range` actually covers; a sample outside the range loads nothing,
  // xterm measures that face's glyphs against a fallback, and every row
  // carrying one shifts. Recutting a face's range without re-picking its
  // sample is exactly how that regresses, so the two are pinned together here.
  const rangesByFamily: Record<string, string> = {
    [DUX_MONO_SYMBOLS_FAMILY]: UNICODE_RANGES,
    [DUX_MONO_FILL_FAMILY]: FILL_UNICODE_RANGES,
  }

  it("keeps every restricted face's sample inside that face's own unicode-range", () => {
    for (const preload of TERMINAL_FONT_PRELOADS) {
      const declared = rangesByFamily[preload.family]
      if (!declared) {
        // "Dux Mono" carries no `unicode-range` at all, so any sample fetches
        // it and there is nothing to pin. Every OTHER family is unknown to
        // this test, and skipping an unknown one would let a fifth face join
        // the preload list with a sample nothing ever checks. Naming the one
        // legitimate exception makes that a failure instead.
        expect(preload.family, "unrestricted face missing its range").toBe(
          DUX_MONO_FAMILY,
        )
        continue
      }
      const ranges = parseUnicodeRange(declared)
      for (const character of [...preload.sample]) {
        const cp = character.codePointAt(0) as number
        const inside = ranges.some(([start, end]) => cp >= start && cp <= end)
        expect(
          inside,
          `${preload.family} sample U+${cp.toString(16).toUpperCase()}`,
        ).toBe(true)
      }
    }
  })

  it("covers both restricted faces with at least one sample each", () => {
    for (const family of [DUX_MONO_SYMBOLS_FAMILY, DUX_MONO_FILL_FAMILY]) {
      expect(
        TERMINAL_FONT_PRELOADS.some((preload) => preload.family === family),
        family,
      ).toBe(true)
    }
  })
})

// The woff2 table tags that fit in the 6-bit index of a table directory entry,
// in the order the spec assigns them. Index 63 means an arbitrary 4-byte tag
// follows instead.
const WOFF2_KNOWN_TAGS = [
  "cmap", "head", "hhea", "hmtx", "maxp", "name", "OS/2", "post",
  "cvt ", "fpgm", "glyf", "loca", "prep", "CFF ", "VORG", "EBDT",
  "EBLC", "gasp", "hdmx", "kern", "LTSH", "PCLT", "VDMX", "vhea",
  "vmtx", "BASE", "GDEF", "GPOS", "GSUB", "EBSC", "JSTF", "MATH",
  "CBDT", "CBLC", "COLR", "CPAL", "SVG ", "sbix", "acnt", "avar",
  "bdat", "bloc", "bsln", "cvar", "fdsc", "feat", "fmtx", "fvar",
  "gvar", "hsty", "just", "lcar", "mort", "morx", "opbd", "prop",
  "trak", "Zapf", "Silf", "Glat", "Gloc", "Feat", "Sill",
]

// woff2's variable-length big-endian integer: seven bits per byte, high bit
// set on every byte but the last.
function readUIntBase128(buf: Buffer, pos: number): [number, number] {
  let value = 0
  for (let i = 0; i < 5; i++) {
    const byte = buf[pos++]
    value = (((value << 7) >>> 0) | (byte & 0x7f)) >>> 0
    if ((byte & 0x80) === 0) return [value, pos]
  }
  throw new Error("malformed UIntBase128")
}

// Pulls one uncompressed sfnt table out of a woff2 file. The header is fixed
// at 48 bytes, then one directory entry per table (a flag byte, an optional
// explicit tag, the original length, and a transformed length only when the
// entry is transformed), then a single brotli stream holding every table
// concatenated in directory order with no padding. Only `cmap` is read here,
// and `cmap` is never a transformed table, so no transform has to be undone.
function woff2Table(buf: Buffer, wanted: string): Buffer {
  if (buf.subarray(0, 4).toString("latin1") !== "wOF2") {
    throw new Error("not a woff2 file")
  }
  const numTables = buf.readUInt16BE(12)
  let pos = 48
  const entries: { tag: string; length: number }[] = []
  for (let i = 0; i < numTables; i++) {
    const flags = buf[pos++]
    const index = flags & 0x3f
    let tag: string
    if (index === 0x3f) {
      tag = buf.subarray(pos, pos + 4).toString("latin1")
      pos += 4
    } else {
      tag = WOFF2_KNOWN_TAGS[index]
    }
    const transformVersion = (flags >> 6) & 0x03
    let length: number
    ;[length, pos] = readUIntBase128(buf, pos)
    // The null transform is version 3 for glyf/loca and version 0 for every
    // other table; anything else means a transformed length follows and
    // replaces the original one as the stored size.
    const transformed =
      tag === "glyf" || tag === "loca"
        ? transformVersion !== 3
        : transformVersion !== 0
    if (transformed) [length, pos] = readUIntBase128(buf, pos)
    entries.push({ tag, length })
  }
  const data = brotliDecompressSync(buf.subarray(pos))
  let offset = 0
  for (const entry of entries) {
    if (entry.tag === wanted) {
      return data.subarray(offset, offset + entry.length)
    }
    offset += entry.length
  }
  throw new Error(`woff2 has no ${wanted} table`)
}

// Every code point mapped by a cmap table, unioned across its subtables.
// Handles the two formats a modern subsetter emits: format 4 for the BMP and
// format 12 for anything above it. An unexpected format throws rather than
// being skipped, so a recut that changed the encoding cannot quietly shrink
// the set this test measures.
function cmapCodePoints(cmap: Buffer): Set<number> {
  const points = new Set<number>()
  const numTables = cmap.readUInt16BE(2)
  for (let i = 0; i < numTables; i++) {
    const sub = cmap.subarray(cmap.readUInt32BE(4 + i * 8 + 4))
    const format = sub.readUInt16BE(0)
    if (format === 4) {
      const segCountX2 = sub.readUInt16BE(6)
      const endBase = 14
      const startBase = endBase + segCountX2 + 2
      for (let s = 0; s < segCountX2 / 2; s++) {
        const end = sub.readUInt16BE(endBase + s * 2)
        const start = sub.readUInt16BE(startBase + s * 2)
        // The final segment is the required 0xFFFF terminator, not content.
        if (start === 0xffff) continue
        for (let c = start; c <= end; c++) points.add(c)
      }
    } else if (format === 12) {
      const numGroups = sub.readUInt32BE(12)
      for (let g = 0; g < numGroups; g++) {
        const base = 16 + g * 12
        const start = sub.readUInt32BE(base)
        const end = sub.readUInt32BE(base + 4)
        for (let c = start; c <= end; c++) points.add(c)
      }
    } else {
      throw new Error(`unhandled cmap subtable format ${format}`)
    }
  }
  return points
}

describe("Dux Mono Fill subset contents", () => {
  // Without this the only claim made about the 79 KB asset was that its first
  // four bytes read "wOF2", so a recut that dropped the glyphs the face exists
  // for, or let colour emoji back in, would ship green. The cut itself lives
  // in src/assets/fonts/dux-mono.LICENSE.
  const coverage = cmapCodePoints(
    woff2Table(readBytes("../assets/fonts/dux-mono-fill.woff2"), "cmap"),
  )

  it("carries the code points the face was cut for", () => {
    // The Claude Code permission-mode and status icons: accept-edits/bypass,
    // manual/plan, and the check marker. These rendering as tofu on Android
    // is why this face exists.
    for (const cp of [0x23f5, 0x23f8, 0x2714]) {
      expect(coverage.has(cp), `U+${cp.toString(16).toUpperCase()}`).toBe(true)
    }
  })

  it("excludes code points with Emoji_Presentation=Yes", () => {
    // A default-emoji code point is drawn double-width by the terminal but
    // single-width by this monospace face, so admitting one desynchronises the
    // grid. Sampled across the ranges the subset spans.
    for (const cp of [0x274c, 0x2b50, 0x23f0]) {
      expect(coverage.has(cp), `U+${cp.toString(16).toUpperCase()}`).toBe(false)
    }
  })

  it("maps nothing in the Private Use Area", () => {
    // The PUA is the symbols face's territory (Powerline, U+E0A0-E0D7). A
    // backstop claiming any of it would shadow glyphs cut to dux's own cell
    // metrics with Adwaita's unrelated ones.
    const pua = Array.from(coverage).filter((c) => c >= 0xe000 && c <= 0xf8ff)
    expect(pua).toEqual([])
  })

  it("maps exactly the expected number of code points", () => {
    // A total, so a recut cannot swap glyphs in and out under the sampled
    // assertions above without saying so here.
    expect(coverage.size).toBe(2267)
  })
})
