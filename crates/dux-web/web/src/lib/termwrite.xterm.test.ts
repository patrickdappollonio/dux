// @vitest-environment jsdom
import { Terminal } from "@xterm/xterm"
import { afterEach, describe, expect, it } from "vitest"

// A DOCUMENTED-FACT test, the third in the `*.xterm.test.ts` family: it mounts
// the REAL `@xterm/xterm` to pin one property of the library that a design
// decision rests on, so the decision stops being a belief.
//
// THE FACT: `term.write(data, cb)` runs `cb` even when `data` is EMPTY, for a
// zero-length `Uint8Array` and for the empty string alike.
//
// WHY dux CARES: the attach-replay machine reports the cover-clearing "the
// replay is on screen" signal from the replay write's own completion callback,
// and the server sends a Binary repaint frame even when the pty is quiet. If an
// empty frame's callback never fired, the cover would hang over a perfectly
// healthy quiet terminal until the replay wait expired and offered a Reconnect
// button for a connection with nothing wrong with it.
//
// The empty-string case is measured alongside it because the drain gate writes
// exactly that (`term.write("", cb)`) to learn when the previous connection's
// queue has parsed.
//
// VERSION DEPENDENCE: a property of xterm's implementation, pinned to the
// version in `package.json`. If an upgrade turns it red, the fix is to
// re-measure and treat an empty frame as applied explicitly in
// `attachReplay.ts`, not to loosen this test.

// xterm's CoreBrowserService calls the LEGACY `matchMedia().addListener`, which
// the shared `@/test/matchMedia` stub does not implement; the same small
// stand-in the other two xterm suites carry.
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

function open(): Terminal {
  restoreMedia = stubMatchMedia()
  const host = document.createElement("div")
  document.body.appendChild(host)
  const term = new Terminal({ cols: 40, rows: 10 })
  term.open(host)
  opened.push(term)
  return term
}

const callbackFor = (term: Terminal, data: string | Uint8Array) =>
  new Promise<void>((resolve) => term.write(data, resolve))

describe("an empty write still completes", () => {
  it("fires the callback for a zero-length Uint8Array, which is what a quiet repaint frame is", async () => {
    const term = open()
    let fired = false
    await callbackFor(term, new Uint8Array(0)).then(() => {
      fired = true
    })
    expect(fired).toBe(true)
  })

  it("fires the callback for the empty string the drain gate writes", async () => {
    const term = open()
    let fired = false
    await callbackFor(term, "").then(() => {
      fired = true
    })
    expect(fired).toBe(true)
  })

  it("still orders an empty write behind the bytes queued before it", async () => {
    const term = open()
    const order: string[] = []
    term.write("hello", () => order.push("bytes"))
    await callbackFor(term, new Uint8Array(0)).then(() => order.push("empty"))
    expect(order).toEqual(["bytes", "empty"])
  })
})
