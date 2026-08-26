// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, render } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { ConnState } from "@/lib/types"
import type { Terminal as XTerm } from "@xterm/xterm"

import { activateLinkAtPoint } from "@/lib/termlink"

// Unlike TerminalPane.test.tsx, this file mounts the pane against the REAL
// @xterm/xterm. The bug it guards lives in the seam between xterm's Linkifier
// and the pane's `linkHandler.activate`, and a stubbed Terminal has no Linkifier
// at all, so a stub could only ever re-assert the stub. jsdom lays nothing out
// and has no canvas, so the few measurements xterm needs are forced below; every
// other part of the path (the OSC 8 parser, the OSC link service, the link
// provider, the mouse-to-cell conversion, the mouseup handler) is the real one.

class FitStub {
  activate() {}
  dispose() {}
  fit() {}
}

class FakePtySocket {
  static instances: FakePtySocket[] = []
  url: string
  connect = vi.fn()
  close = vi.fn()
  // The pane's teardown DISPOSES rather than closes: a socket whose pane is
  // gone must lose its wake listeners too.
  dispose = vi.fn()
  // The real socket answers whether the frame actually went on the wire; a test
  // models a dropped frame (a socket mid-reconnect) by returning false.
  sendResize = vi.fn(() => true)
  sendInput = vi.fn()
  // THE ONE PERIODIC FRAME, and the three page-lifecycle entry points the base
  // class grew with it. Answering `true` means "it went on the wire", which is
  // what starts the heartbeat's answer deadline.
  sendBeat = vi.fn(() => true)
  onBeat: (n: number) => void = () => {}
  dropForRetry = vi.fn()
  resumeNow = vi.fn()
  park = vi.fn()
  isOpen = true
  onConnected: (id: string) => void = () => {}
  onOpen: () => void = () => {}
  onReconnecting: () => void = () => {}
  onConn: (state: ConnState) => void = () => {}
  bytesCb: ((b: Uint8Array) => void) | null = null
  onBytes = (cb: (b: Uint8Array) => void) => {
    this.bytesCb = cb
  }
  shouldRetry: () => boolean = () => true
  onGone: () => void = () => {}
  constructor(url: string) {
    this.url = url
    FakePtySocket.instances.push(this)
  }
}

vi.mock("@xterm/addon-fit", () => ({ FitAddon: FitStub }))
// The REAL xterm, with every constructed terminal recorded: the force-selection
// test has to ask xterm itself what it selected, and its selection model is not
// reachable from the DOM.
const terminals: XTerm[] = []
vi.mock("@xterm/xterm", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@xterm/xterm")>()
  class RecordingTerminal extends actual.Terminal {
    constructor(options?: ConstructorParameters<typeof actual.Terminal>[0]) {
      super(options)
      terminals.push(this)
    }
  }
  return { ...actual, Terminal: RecordingTerminal }
})
const notifyInfoMock = vi.fn()
vi.mock("@/lib/notify", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/notify")>()
  return { ...actual, notifyInfo: (...args: unknown[]) => notifyInfoMock(...args) }
})
vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn() }),
}))
vi.mock("@/components/MacroPopover", () => ({ MacroPopover: () => null }))
vi.mock("@/lib/ptySocket", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/ptySocket")>()
  return { ...actual, PtySocket: FakePtySocket }
})

let mockState: DuxState
// The compose drafts live in the real store, which is module state that outlives
// one test. Cleared per test through the real setter (the ids these suites use).
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  // The compose draft genuinely lives in the store now (keyed by target id, so
  // it survives the pane remount `reconnect()` performs), so the fake state must
  // carry the REAL slice or nothing typed into the box would ever come back.
  // Calling the real hook here also subscribes, so a draft write re-renders.
  return {
    ...actual,
    useDux: () => ({ ...mockState, composeDrafts: actual.useDux().composeDrafts }),
  }
})

installStubs()
const { TerminalPane } = await import("./TerminalPane")
// The compose drafts live in the real store, which is module state outliving one
// test, so each test starts from a clean map. Imported here rather than at the
// top of the file because the store touches `localStorage` at import time and
// `installStubs()` has to run first.
const { setComposeDraft: setComposeDraftReal } = await import("@/lib/store")
const resetComposeDrafts = () => {
  for (const id of ["s1", "t1", "tab-2", "term-1"]) setComposeDraftReal(id, "")
}

function makeState(hyperlinks: boolean): DuxState {
  return {
    conn: "open" as ConnState,
    spine: {
      projects: [],
      sessions: [
        {
          id: "s1",
          workspace: {
            kind: "managed",
            project_id: "p1",
            branch_name: "main",
            initial_branch: "",
            branch_provenance: "created",
            source_branch: "",
            worktree_path: "/tmp/p1",
          },
          title: null,
          provider: "claude",
          status: "active",
          auto_reopen_enabled: false,
          terminals: [],
          tabs: [
            {
              id: "s1",
              provider: "claude",
              order: 0,
              working: false,
              has_output: false,
              has_live_process: true,
            },
          ],
          has_output: false,
          working: false,
        },
      ],
      sidebar: { groups: [], agentless_start: null },
    },
    bootstrap: {
      title: "dux",
      dux_version: "v1",
      show_changes_pane: false,
      always_show_tab_strip: false,
      available_providers: ["claude"],
      agent_tabs_max: 20,
      hyperlinks,
    },
    offline: false,
    terminalEpoch: 0,
  } as unknown as DuxState
}

function installStubs() {
  const mem = new Map<string, string>()
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => mem.get(k) ?? null,
    setItem: (k: string, v: string) => void mem.set(k, String(v)),
    removeItem: (k: string) => void mem.delete(k),
    clear: () => mem.clear(),
  })
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.reject(new Error("offline test"))),
  )
  vi.stubGlobal(
    "matchMedia",
    vi.fn((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    })),
  )
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  )
}

// jsdom reports every box as 0x0, which makes xterm's char measurement invalid
// and its mouse-to-cell conversion bail out (so no link would ever resolve).
// Force a plausible 8x17 glyph and an 800x408 viewport so a click at (5, 5)
// lands on row 1, column 1 — the first cell of the link written below.
function forceLayout() {
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", {
    configurable: true,
    get(this: HTMLElement) {
      return this.classList?.contains("xterm-char-measure-element") ? 32 * 8 : 800
    },
  })
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", {
    configurable: true,
    get(this: HTMLElement) {
      return this.classList?.contains("xterm-char-measure-element") ? 17 : 408
    },
  })
  // The overview-ruler decoration canvas is created during `term.open`, and
  // xterm throws if its 2d context is null (jsdom ships no canvas backend).
  HTMLCanvasElement.prototype.getContext = (() =>
    ({
      canvas: {},
      clearRect() {},
      fillRect() {},
      save() {},
      restore() {},
      scale() {},
      measureText: () => ({ width: 8 }),
      set fillStyle(_v: unknown) {},
    }) as unknown as CanvasRenderingContext2D) as typeof HTMLCanvasElement.prototype.getContext
  Element.prototype.getBoundingClientRect = function () {
    return {
      x: 0,
      y: 0,
      top: 0,
      left: 0,
      right: 800,
      bottom: 408,
      width: 800,
      height: 408,
    } as DOMRect
  }
}

const LINK_URL = "https://example.com"
const osc8 = (url: string) => `\x1b]8;;${url}\x07link\x1b]8;;\x07`
// VT200 mouse tracking plus SGR encoding: what every measured agent CLI asks
// for (see the table in `lib/termmouse.ts`).
const MOUSE_TRACKING_ON = "\x1b[?1000h\x1b[?1006h"

let openSpy: ReturnType<typeof vi.fn>

beforeEach(() => {
  resetComposeDrafts()
  FakePtySocket.instances = []
  terminals.length = 0
  mockState = makeState(true)
  installStubs()
  forceLayout()
  openSpy = vi.fn()
  notifyInfoMock.mockReset()
  vi.stubGlobal("open", openSpy)
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

// Mount the pane, push an OSC 8 hyperlink through the PTY socket, and hand back
// the xterm screen element the Linkifier listens on.
async function mountWithLink(): Promise<HTMLElement> {
  return (await mountLinkPane({ mouseTracking: false })).screen
}

/**
 * The same mount, plus the two things the suppression tests need: the option to
 * turn the child's mouse tracking on, and the socket whose `sendInput` IS the
 * byte stream the app would have received (the pane is the input owner in these
 * tests, so xterm's `onData`/`onBinary` both land there).
 */
async function mountLinkPane({
  mouseTracking,
  url = LINK_URL,
}: {
  mouseTracking: boolean
  url?: string
}): Promise<{
  screen: HTMLElement
  sock: FakePtySocket
  term: XTerm
  rerender: () => Promise<void>
}> {
  const { container, rerender } = render(
    <TerminalPane kind="agent" id="s1" sessionId="s1" />,
  )
  const sock = FakePtySocket.instances.at(-1)
  if (!sock) throw new Error("no PtySocket constructed")
  await act(async () => {
    sock.bytesCb?.(
      new TextEncoder().encode((mouseTracking ? MOUSE_TRACKING_ON : "") + osc8(url)),
    )
    await new Promise((r) => setTimeout(r, 20))
  })
  const screenEl = container.querySelector(".xterm-screen")
  if (!screenEl) throw new Error("xterm did not render a screen element")
  // Everything written before this point is setup, not a mouse report.
  sock.sendInput.mockClear()
  const term = terminals.at(-1)
  if (!term) throw new Error("no Terminal constructed")
  return {
    screen: screenEl as HTMLElement,
    sock,
    term,
    // Re-renders against whatever `mockState` now says, which is how a test
    // flips a preference on a pane that is already mounted.
    rerender: async () => {
      await act(async () => {
        rerender(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
      })
    },
  }
}

/** Everything the pane sent to the PTY, as one string. */
function sentBytes(sock: FakePtySocket): string {
  return sock.sendInput.mock.calls
    .map(([b]) => String.fromCharCode(...(b as Uint8Array)))
    .join("")
}

/** A press and release with no mousemove in front of them. */
async function pressReleaseNoMove(
  el: HTMLElement,
  clientX: number,
  clientY: number,
): Promise<void> {
  const opts = { bubbles: true, clientX, clientY, detail: 1, button: 0 }
  await act(async () => {
    el.dispatchEvent(new MouseEvent("mousedown", opts))
    el.dispatchEvent(new MouseEvent("mouseup", opts))
  })
}

/** A press/release pair at an arbitrary point, with optional modifiers. */
async function clickAt(
  el: HTMLElement,
  clientX: number,
  clientY: number,
  over: { detail?: number; button?: number; ctrlKey?: boolean; metaKey?: boolean } = {},
): Promise<void> {
  const opts = { bubbles: true, clientX, clientY, detail: 1, button: 0, ...over }
  await act(async () => {
    el.dispatchEvent(new MouseEvent("mousemove", { ...opts, detail: 0, button: 0 }))
    el.dispatchEvent(new MouseEvent("mousedown", opts))
    el.dispatchEvent(new MouseEvent("mouseup", opts))
  })
}

// The first cell of the link written on row 1, and a point far away from it on
// row 6 (an 8x17 glyph in an 800x408 viewport; see `forceLayout`).
// SGR mouse reports for the left button: press ends in `M`, release in `m`.
// `no-control-regex` is disabled deliberately: an ESC is precisely the byte
// under test here, and matching it is the point of the assertion.
/* eslint-disable no-control-regex */
const SGR_PRESS = new RegExp("\\x1b\\[<0;\\d+;\\d+M")
const SGR_RELEASE = new RegExp("\\x1b\\[<0;\\d+;\\d+m")
const SGR_ANY = new RegExp("\\x1b\\[<\\d+;\\d+;\\d+M")
/* eslint-enable no-control-regex */

const ON_LINK: [number, number] = [5, 5]
const OFF_LINK: [number, number] = [300, 100]

// One press/release pair over the link's first cell. `detail` is the browser's
// click counter (2 on the second click of a double-click); `button` is 0 for the
// primary button, 1 middle, 2 right.
async function clickLink(
  el: HTMLElement,
  { detail = 1, button = 0 }: { detail?: number; button?: number } = {},
): Promise<void> {
  const opts = { bubbles: true, clientX: 5, clientY: 5, detail, button }
  await act(async () => {
    el.dispatchEvent(new MouseEvent("mousemove", { ...opts, detail: 0, button: 0 }))
    el.dispatchEvent(new MouseEvent("mousedown", opts))
    el.dispatchEvent(new MouseEvent("mouseup", opts))
  })
}

describe("TerminalPane OSC 8 hyperlinks", () => {
  it("opens exactly one tab for a single primary click", async () => {
    const el = await mountWithLink()
    await clickLink(el)
    expect(openSpy.mock.calls).toEqual([[LINK_URL, "_blank", "noopener,noreferrer"]])
  })

  // Double-clicking is how you select a word in a terminal, and xterm's
  // Linkifier activates the link on EVERY mouseup, so the second click of that
  // gesture would open a second tab.
  it("opens only one tab for a double-click, which selects a word", async () => {
    const el = await mountWithLink()
    await clickLink(el, { detail: 1 })
    await clickLink(el, { detail: 2 })
    expect(openSpy).toHaveBeenCalledTimes(1)
  })

  it("opens only one tab for a triple-click, which selects a line", async () => {
    const el = await mountWithLink()
    await clickLink(el, { detail: 1 })
    await clickLink(el, { detail: 2 })
    await clickLink(el, { detail: 3 })
    expect(openSpy).toHaveBeenCalledTimes(1)
  })

  // xterm's Linkifier does not look at `button`, so a right-click over a link
  // activated it as well — and in dux a right-click is the PASTE gesture, so the
  // user got a paste AND a new tab they never asked for.
  it("does not open on a right-click, which is the paste gesture", async () => {
    const el = await mountWithLink()
    await clickLink(el, { button: 2 })
    expect(openSpy).not.toHaveBeenCalled()
  })

  it("does not open on a middle-click", async () => {
    const el = await mountWithLink()
    await clickLink(el, { button: 1 })
    expect(openSpy).not.toHaveBeenCalled()
  })

  it("does not open when the hyperlinks preference is off", async () => {
    mockState = makeState(false)
    const el = await mountWithLink()
    await clickLink(el)
    expect(openSpy).not.toHaveBeenCalled()
  })

  // With mouse tracking OFF nothing is suppressed, so the click must reach
  // xterm's own machinery exactly as it always did.
  it("sends no mouse report for a link click when the app is not tracking", async () => {
    const { screen, sock } = await mountLinkPane({ mouseTracking: false })
    await clickAt(screen, ...ON_LINK)
    expect(openSpy).toHaveBeenCalledTimes(1)
    expect(sentBytes(sock)).toBe("")
  })
})

/**
 * The click that dispatches a link never reaches a mouse-tracking app.
 *
 * The bug: with tracking on, dux's browser-side `window.open` AND the forwarded
 * mouse report both fired, and the agent CLI answers the report by opening the
 * URL on the SERVER's machine. Two tabs, one of them on the wrong computer.
 *
 * ORDER MATTERS in this file. The positive control comes first: without it a
 * "zero bytes were sent" assertion would pass just as happily against a broken
 * harness that never sends bytes at all.
 */
describe("a link click under a mouse-tracking app", () => {
  it("POSITIVE CONTROL: an ordinary click off the link is still reported", async () => {
    const { screen, sock } = await mountLinkPane({ mouseTracking: true })
    await clickAt(screen, ...OFF_LINK)
    // SGR press and release at the clicked cell.
    expect(sentBytes(sock)).toMatch(SGR_PRESS)
    expect(sentBytes(sock)).toMatch(SGR_RELEASE)
    expect(openSpy).not.toHaveBeenCalled()
  })

  // The hint says "dux opened that link in your browser", so it must not fire
  // for a press dux swallowed and then opened NOTHING from. This test has to
  // come before the hint test below: the latch is module scope, so the first
  // raise in this file is the only one.
  it("stays quiet when a suppressed click opens nothing", async () => {
    // The preference turned off under a link ALREADY on screen: xterm keeps the
    // link (the parser gate only affects cells written after the flip), so the
    // press is still swallowed, and dux opens nothing.
    const { screen, sock, rerender } = await mountLinkPane({ mouseTracking: true })
    mockState = makeState(false)
    await rerender()
    await clickAt(screen, ...ON_LINK)
    // Still swallowed: forwarding it would hand the app the click and bring the
    // server-side open back.
    expect(sentBytes(sock)).toBe("")
    expect(openSpy).not.toHaveBeenCalled()
    expect(notifyInfoMock).not.toHaveBeenCalled()
  })

  // The hint tells the visitor the hatch exists, the first time dux takes a
  // click away from the app, and never again in this page session. The latch is
  // module scope, so this has to be the FIRST suppressing test in the file and
  // every test after it asserts the silence (see the one below it).
  it("names the hatch chord on the first suppressed click", async () => {
    const { screen } = await mountLinkPane({ mouseTracking: true })
    await clickAt(screen, ...ON_LINK)
    await clickAt(screen, ...ON_LINK)
    expect(notifyInfoMock).toHaveBeenCalledTimes(1)
    expect(String(notifyInfoMock.mock.calls[0][0])).toMatch(/Ctrl/)
  })

  it("opens exactly one tab and reports nothing to the app", async () => {
    const { screen, sock } = await mountLinkPane({ mouseTracking: true })
    await clickAt(screen, ...ON_LINK)
    expect(openSpy.mock.calls).toEqual([[LINK_URL, "_blank", "noopener,noreferrer"]])
    expect(sentBytes(sock)).toBe("")
    // ...and the hint stays retired for the rest of the session.
    expect(notifyInfoMock).not.toHaveBeenCalled()
  })

  // The second press of a double-click is still swallowed even though it opens
  // nothing: forwarding it would hand the app a clean click and resurrect the
  // server-side open, once per extra click of a select-a-word gesture.
  it("leaks neither a second tab nor a report on a double-click", async () => {
    const { screen, sock } = await mountLinkPane({ mouseTracking: true })
    await clickAt(screen, ...ON_LINK, { detail: 1 })
    await clickAt(screen, ...ON_LINK, { detail: 2 })
    expect(openSpy).toHaveBeenCalledTimes(1)
    expect(sentBytes(sock)).toBe("")
  })

  // The escape hatch: Ctrl (Cmd on a Mac) hands the click to the app instead,
  // and dux opens nothing, or the hatch would be a double-open of its own.
  it("forwards a hatch-chord click to the app and opens nothing", async () => {
    const { screen, sock } = await mountLinkPane({ mouseTracking: true })
    await clickAt(screen, ...ON_LINK, { ctrlKey: true })
    expect(sentBytes(sock)).toMatch(SGR_ANY)
    expect(openSpy).not.toHaveBeenCalled()
  })

  // A press swallowed on a link and released somewhere else must emit NOTHING:
  // a release-without-press report is a gesture the app never saw begin. And
  // the next click must still work, so the in-flight state cannot wedge.
  it("emits no report for a press that slides off the link, and does not wedge", async () => {
    const { screen, sock } = await mountLinkPane({ mouseTracking: true })
    await act(async () => {
      screen.dispatchEvent(
        new MouseEvent("mousemove", { bubbles: true, clientX: 5, clientY: 5 }),
      )
      screen.dispatchEvent(
        new MouseEvent("mousedown", {
          bubbles: true,
          clientX: 5,
          clientY: 5,
          button: 0,
          detail: 1,
        }),
      )
      screen.dispatchEvent(
        new MouseEvent("mouseup", {
          bubbles: true,
          clientX: 300,
          clientY: 100,
          button: 0,
          detail: 1,
        }),
      )
    })
    expect(sentBytes(sock)).toBe("")
    expect(openSpy).not.toHaveBeenCalled()
    // Not wedged: the next ordinary click reports as usual.
    await clickAt(screen, ...OFF_LINK)
    expect(sentBytes(sock)).toMatch(SGR_PRESS)
  })

  // The one-shot document listener that clears an outside release OBSERVES; it
  // must never stop propagation, or a stale arming (a release that happened
  // off-window, so no mouseup ever arrived) would eat an unrelated one later.
  it("never swallows an unrelated mouseup after a release off-window", async () => {
    const { screen, sock } = await mountLinkPane({ mouseTracking: true })
    await act(async () => {
      screen.dispatchEvent(
        new MouseEvent("mousemove", { bubbles: true, clientX: 5, clientY: 5 }),
      )
      screen.dispatchEvent(
        new MouseEvent("mousedown", {
          bubbles: true,
          clientX: 5,
          clientY: 5,
          button: 0,
          detail: 1,
        }),
      )
    })
    // ...the visitor releases outside the browser window, so no mouseup is
    // delivered for that press at all. Some later, unrelated mouseup elsewhere
    // on the page must reach its own listeners untouched.
    let seen = 0
    const spy = () => {
      seen++
    }
    document.addEventListener("mouseup", spy)
    await act(async () => {
      document.body.dispatchEvent(new MouseEvent("mouseup", { bubbles: true }))
    })
    document.removeEventListener("mouseup", spy)
    expect(seen).toBe(1)
    expect(openSpy).not.toHaveBeenCalled()
    expect(sentBytes(sock)).toBe("")
  })

  // The two halves of "the hover record must be TRUE at the moment of the
  // press", which is what the press-time prime buys. Both use a press with NO
  // mousemove of its own, the case passive hover tracking cannot answer: the
  // pointer can end up somewhere new without a move event dux saw (the buffer
  // scrolls under a still pointer, a resize clears the current link, or the
  // click is simply the first thing that happens on the page).
  //
  // A stale TRUE is the dangerous direction: it swallows an ordinary click,
  // which in a TUI is a button press.
  it("does not swallow an unhovered click off the link after hovering it", async () => {
    const { screen, sock } = await mountLinkPane({ mouseTracking: true })
    await act(async () => {
      screen.dispatchEvent(
        new MouseEvent("mousemove", { bubbles: true, clientX: 5, clientY: 5 }),
      )
    })
    await pressReleaseNoMove(screen, ...OFF_LINK)
    expect(sentBytes(sock)).toMatch(SGR_PRESS)
    expect(sentBytes(sock)).toMatch(SGR_RELEASE)
    expect(openSpy).not.toHaveBeenCalled()
  })

  // ...and a stale FALSE leaks the server-side open the whole change is about.
  it("suppresses a press on a link that no mousemove ever hovered", async () => {
    const { screen, sock } = await mountLinkPane({ mouseTracking: true })
    await pressReleaseNoMove(screen, ...ON_LINK)
    expect(openSpy).toHaveBeenCalledTimes(1)
    expect(sentBytes(sock)).toBe("")
  })

  // THE FORCE-LOCAL-SELECTION GESTURE STAYS A SELECTION. Shift (Option on a
  // Mac) is the documented way to select and copy text out of a mouse-tracking
  // app, and a URL is exactly the text people select. xterm's own `mousedown`
  // returns before it sends anything under that modifier, so letting the press
  // through forwards nothing AND lets xterm start the selection; dux must open
  // no tab out of it either.
  it("selects the link locally under the force-selection modifier", async () => {
    const { screen, sock, term } = await mountLinkPane({ mouseTracking: true })
    const at = (type: string, x: number, over: Record<string, unknown> = {}) =>
      new MouseEvent(type, {
        bubbles: true,
        clientX: x,
        clientY: 5,
        button: 0,
        detail: 1,
        shiftKey: true,
        ...over,
      })
    await act(async () => {
      screen.dispatchEvent(at("mousemove", 1, { detail: 0 }))
      screen.dispatchEvent(at("mousedown", 1))
      screen.dispatchEvent(at("mousemove", 45, { detail: 0, buttons: 1 }))
      screen.dispatchEvent(at("mouseup", 45))
    })
    expect(term.getSelection()).toBe("link")
    expect(openSpy).not.toHaveBeenCalled()
    expect(sentBytes(sock)).toBe("")
    // CONTROL, so none of the three above can pass for want of a link under the
    // press: the SAME point without the modifier is the swallowed link click.
    await clickAt(screen, 1, 5)
    expect(openSpy.mock.calls).toEqual([[LINK_URL, "_blank", "noopener,noreferrer"]])
    expect(sentBytes(sock)).toBe("")
  })

  // A right press MID-GESTURE (the visitor chords a paste while the left button
  // is still down on a swallowed link press) must not wipe the in-flight record:
  // the left release would then be forwarded on its own, and a release with no
  // press is a report for a gesture the app never saw begin.
  it("keeps a swallowed press paired when a right press chords into it", async () => {
    const { screen, sock } = await mountLinkPane({ mouseTracking: true })
    const at = (type: string, over: Record<string, unknown> = {}) =>
      new MouseEvent(type, {
        bubbles: true,
        clientX: 5,
        clientY: 5,
        button: 0,
        detail: 1,
        ...over,
      })
    await act(async () => {
      screen.dispatchEvent(at("mousemove", { detail: 0 }))
      screen.dispatchEvent(at("mousedown"))
      screen.dispatchEvent(at("mousedown", { button: 2, buttons: 3 }))
      screen.dispatchEvent(at("mouseup", { button: 2, buttons: 1 }))
      screen.dispatchEvent(at("mouseup"))
    })
    // The right button is untouched and reaches the app (the positive control
    // for this test: the bytes below are missing, not absent for want of a
    // working harness).
    /* eslint-disable-next-line no-control-regex */
    expect(sentBytes(sock)).toMatch(new RegExp("\\x1b\\[<2;\\d+;\\d+M"))
    // ...and the left pair stayed swallowed, both halves.
    expect(sentBytes(sock)).not.toMatch(SGR_PRESS)
    expect(sentBytes(sock)).not.toMatch(SGR_RELEASE)
    // Not wedged either: the next ordinary click reports as usual.
    sock.sendInput.mockClear()
    await clickAt(screen, ...OFF_LINK)
    expect(sentBytes(sock)).toMatch(SGR_PRESS)
  })

  // dux's own replays travel the capture phase too (a `bubbles: false` event
  // still runs capture listeners on its ancestors), so the intercept has to let
  // them past or the touch link probe would probe nothing.
  it("lets dux's own tagged link probe through", async () => {
    const { screen, sock } = await mountLinkPane({ mouseTracking: true })
    let activations = 0
    const opened = () => {
      activations = openSpy.mock.calls.length
      return activations
    }
    const hit = activateLinkAtPoint(screen, 5, 5, opened)
    expect(hit).toBe(true)
    expect(openSpy).toHaveBeenCalledTimes(1)
    // ...and the probe itself reports nothing to the app either, because it is
    // dispatched at the screen element and does not bubble.
    expect(sentBytes(sock)).toBe("")
  })
})
