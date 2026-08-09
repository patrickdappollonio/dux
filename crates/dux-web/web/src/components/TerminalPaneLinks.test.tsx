// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, render } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { ConnState } from "@/lib/types"

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
  // The real socket answers whether the frame actually went on the wire; a test
  // models a dropped frame (a socket mid-reconnect) by returning false.
  sendResize = vi.fn(() => true)
  sendInput = vi.fn()
  sendViewed = vi.fn()
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
vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), { success: vi.fn(), error: vi.fn() }),
}))
vi.mock("@/components/MacroPopover", () => ({ MacroPopover: () => null }))
vi.mock("@/lib/ptySocket", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/ptySocket")>()
  return { ...actual, PtySocket: FakePtySocket }
})

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState }
})

installStubs()
const { TerminalPane } = await import("./TerminalPane")

function makeState(hyperlinks: boolean): DuxState {
  return {
    conn: "open" as ConnState,
    spine: {
      projects: [],
      sessions: [
        {
          id: "s1",
          project_id: "p1",
          title: null,
          provider: "claude",
          branch_name: "main",
          worktree_path: "/tmp/p1",
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
const OSC8_LINK = `\x1b]8;;${LINK_URL}\x07link\x1b]8;;\x07`

let openSpy: ReturnType<typeof vi.fn>

beforeEach(() => {
  FakePtySocket.instances = []
  mockState = makeState(true)
  installStubs()
  forceLayout()
  openSpy = vi.fn()
  vi.stubGlobal("open", openSpy)
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

// Mount the pane, push an OSC 8 hyperlink through the PTY socket, and hand back
// the xterm screen element the Linkifier listens on.
async function mountWithLink(): Promise<HTMLElement> {
  const { container } = render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
  const sock = FakePtySocket.instances.at(-1)
  if (!sock) throw new Error("no PtySocket constructed")
  await act(async () => {
    sock.bytesCb?.(new TextEncoder().encode(OSC8_LINK))
    await new Promise((r) => setTimeout(r, 20))
  })
  const screenEl = container.querySelector(".xterm-screen")
  if (!screenEl) throw new Error("xterm did not render a screen element")
  return screenEl as HTMLElement
}

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

  // The regression. Double-clicking is how you select a word in a terminal, and
  // xterm's Linkifier activates the link on EVERY mouseup, so the second click
  // of that gesture opened a second tab.
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
})
