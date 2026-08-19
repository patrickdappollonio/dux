// @vitest-environment jsdom
import { Terminal } from "@xterm/xterm"
import { afterEach, describe, expect, it } from "vitest"

// A DOCUMENTED-FACT test, in the same spirit as `termselect.xterm.test.ts`: it
// mounts the REAL `@xterm/xterm` to pin one property of the library that a
// design decision in `TerminalPane` rests on, so the decision stops being a
// belief and starts being a measurement.
//
// THE FACT: a local resize of xterm resets the buffer's scrolling region
// (DECSTBM) to full screen, on the alternate buffer as well as the normal one.
// In xterm 6.0.0 that is `Buffer.resize`, which assigns `scrollBottom = newRows
// - 1` unconditionally and `scrollTop = 0` for any non-empty buffer, run over
// both buffers by `BufferSet.resize`.
//
// WHY dux CARES: `fit.fit()` is a local resize. A refit that lands while a
// touch gesture is forwarding wheel reports therefore silently widens the
// region under a mouse-tracking, region-relative pager that is still painting
// for the old geometry, and its repaint stamps one line per notch. That is the
// repeated-line bug on phones, and it is why the pane defers the FIT along with
// the SIGWINCH for the length of a gesture: the two are one atomic pair.
//
// VERSION DEPENDENCE: this is a property of xterm's implementation, not of any
// spec (a real terminal is entitled to clamp or preserve the margins instead),
// so it is pinned to the version in `package.json` and nothing else. If an
// upgrade turns these assertions red, the fix is to re-measure and re-write the
// reasoning in `TerminalPane`, not to loosen the test.

// `scrollTop`/`scrollBottom` are not on the public `Terminal` surface (nothing
// in `src/lib` or `src/components` reads them), so the test reaches through
// `_core` exactly as the selection suite reaches through `_renderService` for
// the geometry jsdom cannot produce. Reading the private buffer is the only way
// to ask this question directly rather than inferring it from a repaint.
interface CoreBuffer {
  scrollTop: number
  scrollBottom: number
}
function region(term: Terminal): CoreBuffer {
  return (
    term as unknown as {
      _core: { _bufferService: { buffers: { active: CoreBuffer } } }
    }
  )._core._bufferService.buffers.active
}

// xterm's CoreBrowserService tracks the device pixel ratio through
// `matchMedia` and calls the LEGACY `addListener`, which the shared
// `@/test/matchMedia` stub does not implement (nothing in dux's own code uses
// it), so this file carries the same small stand-in `termselect.xterm.test.ts`
// does.
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

async function open(rows: number): Promise<Terminal> {
  restoreMedia = stubMatchMedia()
  const host = document.createElement("div")
  document.body.appendChild(host)
  const term = new Terminal({ cols: 40, rows })
  term.open(host)
  opened.push(term)
  await new Promise<void>((resolve) => term.write("hello\r\n", resolve))
  return term
}

const write = (term: Terminal, data: string) =>
  new Promise<void>((resolve) => term.write(data, resolve))

describe("xterm resets the scrolling region on a local resize", () => {
  it("drops a DECSTBM region on the normal buffer", async () => {
    const term = await open(10)
    // DECSTBM: rows 3 through 7, one-based on the wire, zero-based inside.
    await write(term, "\x1b[3;7r")
    expect(region(term)).toMatchObject({ scrollTop: 2, scrollBottom: 6 })
    term.resize(40, 12)
    expect(region(term)).toMatchObject({ scrollTop: 0, scrollBottom: 11 })
  })

  it("drops it on the ALTERNATE buffer too, which has no scrollback to recover from", async () => {
    const term = await open(10)
    // ?1049h: the alt screen every full-screen TUI lives on.
    await write(term, "\x1b[?1049h\x1b[3;7r")
    expect(region(term)).toMatchObject({ scrollTop: 2, scrollBottom: 6 })
    term.resize(40, 12)
    expect(region(term)).toMatchObject({ scrollTop: 0, scrollBottom: 11 })
  })

  it("resets the region even when only the COLUMN count changes", async () => {
    // The width-only case matters because the pane's own first-frame jiggle and
    // a phone's keyboard collapse both move width, not just height.
    const term = await open(10)
    await write(term, "\x1b[3;7r")
    term.resize(50, 10)
    expect(region(term)).toMatchObject({ scrollTop: 0, scrollBottom: 9 })
  })
})
