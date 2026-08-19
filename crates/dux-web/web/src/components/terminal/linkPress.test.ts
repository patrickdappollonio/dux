// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { Terminal } from "@xterm/xterm"

import { markDuxReplay } from "@/lib/termreplay"

import { createLinkPress } from "./linkPress"

// The pane's own links suite mounts the REAL xterm, because the bug it guards
// lives in the seam between xterm's Linkifier and the pane. These tests are the
// other half: the MACHINE's own bookkeeping (press/release pairing, the
// outside-release watcher, the counter, the abstention) against a stubbed
// Linkifier, where each rule can be provoked directly.
//
// `primeLinkHover` is the one thing stubbed: it is the synchronous hover replay
// whose own contract is pinned in `lib/termlink.test.ts`. Here it just reports
// which point was primed, and the test decides what link is "there".
let primedPoints: { x: number; y: number }[] = []
let linkAt: (x: number, y: number) => string | null = () => null
let hoverSink: ((uri: string | null) => void) | null = null
vi.mock("@/lib/termlink", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/termlink")>()
  return {
    ...actual,
    linkifierElement: (el: unknown) => el,
    primeLinkHover: (_el: unknown, x: number, y: number) => {
      primedPoints.push({ x, y })
      hoverSink?.(linkAt(x, y))
    },
  }
})

class TermFake {
  element = document.createElement("div")
  modes = { mouseTrackingMode: "any" as string }
  focusCalls = 0
  focus() {
    this.focusCalls++
  }
}

function setup(opts: { hyperlinks?: boolean; isMac?: boolean } = {}) {
  const term = new TermFake()
  const machine = createLinkPress({
    hyperlinks: () => opts.hyperlinks ?? true,
    isMac: opts.isMac ?? false,
  })
  machine.setTerminal(term as unknown as Terminal)
  // The hover half of the handler is what `primeLinkHover` feeds.
  hoverSink = (uri) => {
    if (uri === null) machine.linkHandler.leave()
    else machine.linkHandler.hover(null, uri)
  }
  const container = document.createElement("div")
  document.body.appendChild(container)
  // xterm's own listeners sit on DESCENDANTS of the container, which is the
  // whole reason the intercept is capture-phase, so events are dispatched at a
  // child here too and `reached` stands in for xterm's handler.
  const child = document.createElement("div")
  container.appendChild(child)
  const reached: string[] = []
  child.addEventListener("mousedown", () => reached.push("mousedown"))
  child.addEventListener("mouseup", () => reached.push("mouseup"))
  machine.attach(container)
  return { term, machine, container: child, reached }
}

function press(
  container: Element,
  init: Partial<MouseEventInit> & { tagged?: boolean } = {},
) {
  const e = new MouseEvent("mousedown", {
    bubbles: true,
    cancelable: true,
    button: 0,
    clientX: 10,
    clientY: 10,
    ...init,
  })
  if (init.tagged) markDuxReplay(e)
  container.dispatchEvent(e)
  return e
}

function release(
  container: EventTarget,
  init: Partial<MouseEventInit> & { tagged?: boolean } = {},
) {
  const e = new MouseEvent("mouseup", {
    bubbles: true,
    cancelable: true,
    button: 0,
    clientX: 10,
    clientY: 10,
    ...init,
  })
  if (init.tagged) markDuxReplay(e)
  container.dispatchEvent(e)
  return e
}

let opened: string[] = []
beforeEach(() => {
  primedPoints = []
  opened = []
  linkAt = () => null
  hoverSink = null
  vi.stubGlobal("open", (url: string) => {
    opened.push(url)
    return null
  })
})
afterEach(() => {
  vi.unstubAllGlobals()
  document.body.innerHTML = ""
})

describe("a press on a link while the app tracks the mouse", () => {
  beforeEach(() => {
    linkAt = () => "https://example.test/a"
  })

  it("is swallowed, does xterm's own preventDefault and focus, and opens on the release", () => {
    const { container, term, machine } = setup()
    const down = press(container)
    expect(down.defaultPrevented).toBe(true)
    expect(term.focusCalls).toBe(1)
    expect(opened).toEqual([])
    release(container)
    expect(opened).toEqual(["https://example.test/a"])
    expect(machine.activations()).toBe(1)
  })

  it("resolves the link SYNCHRONOUSLY at press time rather than trusting hover", () => {
    const { container } = setup()
    press(container, { clientX: 42, clientY: 24 })
    expect(primedPoints).toEqual([{ x: 42, y: 24 }])
  })

  it("pairs the release with its press, always: neither reaches xterm", () => {
    const { container, reached } = setup()
    press(container)
    release(container)
    // A release forwarded on its own would be a report for a gesture the app
    // never saw begin, so the release is stopped too.
    expect(reached).toEqual([])
  })

  it("keeps a swallowed press paired when a RIGHT press chords into it", () => {
    const { container, reached } = setup()
    press(container)
    press(container, { button: 2 })
    release(container)
    // The right press reached xterm (it is somebody else's event); the left
    // release did NOT, because it still belongs to the swallowed press.
    expect(reached).toEqual(["mousedown"])
    expect(opened).toEqual(["https://example.test/a"])
  })

  it("skips dux's OWN tagged replays, rather than checking isTrusted", () => {
    const { container } = setup()
    const down = press(container, { tagged: true })
    expect(down.defaultPrevented).toBe(false)
  })
})

describe("what is left alone", () => {
  it("leaves an ordinary press off a link entirely alone", () => {
    linkAt = () => null
    const { container, term } = setup()
    const down = press(container)
    expect(down.defaultPrevented).toBe(false)
    expect(term.focusCalls).toBe(0)
    release(container)
    expect(opened).toEqual([])
  })

  it("leaves every press alone when nothing is tracking the mouse", () => {
    linkAt = () => "https://example.test/a"
    const { container, term } = setup()
    term.modes.mouseTrackingMode = "none"
    const down = press(container)
    expect(down.defaultPrevented).toBe(false)
    expect(primedPoints).toEqual([])
  })

  it("leaves a press alone under the force-local-selection modifier", () => {
    linkAt = () => "https://example.test/a"
    const { container } = setup()
    const down = press(container, { shiftKey: true })
    expect(down.defaultPrevented).toBe(false)
    release(container, { shiftKey: true })
    expect(opened).toEqual([])
  })

  it("forwards a hatch-chord click to the app and opens nothing", () => {
    linkAt = () => "https://example.test/a"
    const { container } = setup()
    const down = press(container, { ctrlKey: true })
    expect(down.defaultPrevented).toBe(false)
    release(container, { ctrlKey: true })
    expect(opened).toEqual([])
  })

  it("swallows WITHOUT opening when the hyperlinks preference is off", () => {
    linkAt = () => "https://example.test/a"
    const { container } = setup({ hyperlinks: false })
    press(container)
    release(container)
    expect(opened).toEqual([])
  })
})

describe("the in-flight record", () => {
  it("clears on a release OFF-WINDOW, and never swallows an unrelated mouseup", () => {
    linkAt = () => "https://example.test/a"
    const { container, reached } = setup()
    press(container)
    // Released over another pane entirely.
    const elsewhere = document.createElement("div")
    document.body.appendChild(elsewhere)
    const away = release(elsewhere)
    expect(away.defaultPrevented).toBe(false)
    expect(opened).toEqual([])
    // And the record is gone, so a later unrelated release inside the pane is
    // not consumed as this gesture's.
    release(container)
    expect(reached).toEqual(["mouseup"])
  })

  it("is reset by a new primary press, so a lost release cannot wedge the next click", () => {
    linkAt = () => "https://example.test/a"
    const { container } = setup()
    press(container)
    press(container)
    release(container)
    expect(opened).toEqual(["https://example.test/a"])
  })

  it("opens nothing for a gesture that slid OFF the link", () => {
    let at = "https://example.test/a"
    linkAt = () => at
    const { container, reached } = setup()
    press(container, { clientX: 10, clientY: 10 })
    at = ""
    linkAt = () => null
    release(container, { clientX: 400, clientY: 10 })
    // Still paired (the press was swallowed), but nothing opens.
    expect(reached).toEqual([])
    expect(opened).toEqual([])
  })

  it("opens for a travelled gesture that stayed on the SAME link", () => {
    linkAt = () => "https://example.test/a"
    const { container } = setup()
    press(container, { clientX: 10, clientY: 10 })
    release(container, { clientX: 400, clientY: 10 })
    expect(opened).toEqual(["https://example.test/a"])
  })
})

describe("the Linkifier's own path", () => {
  it("opens through the SAME function, counting the same activations", () => {
    const { container, machine } = setup()
    machine.linkHandler.activate(
      new MouseEvent("mouseup", { button: 0, detail: 1 }),
      "https://example.test/b",
    )
    expect(opened).toEqual(["https://example.test/b"])
    expect(machine.activations()).toBe(1)
    void container
  })

  it("refuses a double-click's second activation", () => {
    const { machine } = setup()
    machine.linkHandler.activate(
      new MouseEvent("mouseup", { button: 0, detail: 2 }),
      "https://example.test/b",
    )
    expect(opened).toEqual([])
  })
})

describe("teardown", () => {
  it("stops intercepting once disposed", () => {
    linkAt = () => "https://example.test/a"
    const { container, machine } = setup()
    machine.dispose()
    const down = press(container)
    expect(down.defaultPrevented).toBe(false)
  })
})
