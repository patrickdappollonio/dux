// @vitest-environment jsdom
import { Terminal } from "@xterm/xterm"
import { afterEach, describe, expect, it } from "vitest"

import { createSgrColonNormalizer } from "./sgrColonForm"

// A DOCUMENTED-FACT test, in the `*.xterm.test.ts` family: it mounts the REAL
// `@xterm/xterm` to pin the library behaviour `lib/sgrColonForm.ts` exists to
// work around, so the workaround stops being a belief about a parser and starts
// being a measurement of the one the app ships.
//
// THE FACT: given `48:2:R:G:B`, xterm.js reads the first sub-parameter after
// the `2` as a colour-space id, so the channels shift left and the blue channel
// is lost. Teal (0,128,128) paints as olive (128,128,0).
//
// AND: the same colour spelled `48:2::R:G:B` parses correctly, which is what
// makes respelling the stream a fix rather than a workaround with a cost.
//
// VERSION DEPENDENCE: pinned to the xterm version in `package.json`. If an
// upgrade turns the first case red, upstream has accepted the short form and
// `lib/sgrColonForm.ts` can be deleted whole; do not loosen this test.

// xterm's CoreBrowserService calls the LEGACY `matchMedia().addListener`, which
// the shared `@/test/matchMedia` stub does not implement; the same small
// stand-in the other xterm suites carry.
function stubMatchMedia(): () => void {
  const previous = Object.getOwnPropertyDescriptor(window, "matchMedia")
  Object.defineProperty(window, "matchMedia", {
    configurable: true,
    writable: true,
    value: (query: string) =>
      ({
        matches: false,
        media: query,
        onchange: null,
        addListener() {},
        removeListener() {},
        addEventListener() {},
        removeEventListener() {},
        dispatchEvent: () => false,
      }) as unknown as MediaQueryList,
  })
  return () => {
    if (previous) Object.defineProperty(window, "matchMedia", previous)
    else delete (window as { matchMedia?: unknown }).matchMedia
  }
}

let restoreMedia: (() => void) | null = null
const opened: Terminal[] = []

afterEach(() => {
  for (const term of opened.splice(0)) term.dispose()
  restoreMedia?.()
  restoreMedia = null
  document.body.innerHTML = ""
})

/// Write `data` into a real terminal and read back the background colour of the
/// first cell, as `[r, g, b]`, or null if xterm did not record an RGB colour.
async function backgroundOfFirstCell(
  data: string,
): Promise<[number, number, number] | null> {
  restoreMedia = stubMatchMedia()
  const host = document.createElement("div")
  document.body.appendChild(host)
  const term = new Terminal({ cols: 40, rows: 10 })
  term.open(host)
  opened.push(term)

  await new Promise<void>((resolve) => term.write(data, resolve))

  const cell = term.buffer.active.getLine(0)?.getCell(0)
  if (!cell || !cell.isBgRGB()) return null
  const packed = cell.getBgColor()
  return [(packed >> 16) & 0xff, (packed >> 8) & 0xff, packed & 0xff]
}

const enc = new TextEncoder()
const dec = new TextDecoder()

function normalize(input: string): string {
  return dec.decode(createSgrColonNormalizer().push(enc.encode(input)))
}

describe("what xterm.js does with the colon form", () => {
  it("shifts the channels left for the three-sub-parameter form, which is the bug", async () => {
    expect(await backgroundOfFirstCell("\x1b[48:2:0:128:128mX")).toEqual([
      128, 128, 0,
    ])
  })

  it("reads the empty-colour-space form correctly", async () => {
    expect(await backgroundOfFirstCell("\x1b[48:2::0:128:128mX")).toEqual([
      0, 128, 128,
    ])
  })

  it("reads the semicolon form correctly", async () => {
    expect(await backgroundOfFirstCell("\x1b[48;2;0;128;128mX")).toEqual([
      0, 128, 128,
    ])
  })
})

describe("the respelled stream paints the colour the child asked for", () => {
  it("turns the olive teal back into teal", async () => {
    const painted = await backgroundOfFirstCell(
      normalize("\x1b[48:2:0:128:128mX"),
    )
    expect(painted).toEqual([0, 128, 128])
  })

  it("turns the yellow white back into white", async () => {
    const painted = await backgroundOfFirstCell(
      normalize("\x1b[48:2:255:255:255mX"),
    )
    expect(painted).toEqual([255, 255, 255])
  })
})
