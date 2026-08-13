// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

import { COMPOSE_SUBMIT_DELAY_MS } from "@/lib/composebar"
import {
  getComposeInsertSink,
  setComposeInsertSink,
} from "@/lib/composeInsert"
import type { DuxState } from "@/lib/store"
import type { ConnState } from "@/lib/types"
import { notifyPtyOwner, resetPtyOwnerEpochs } from "@/lib/ptyOwnership"
import { installXtermMouseModel } from "@/lib/xtermMouseModel"
import { stubCoarsePointer, type MatchMediaStub } from "@/test/matchMedia"

// TerminalPane embeds xterm.js, whose canvas rendering jsdom cannot back (see the
// note in TerminalArea.test.tsx). So we mount the REAL TerminalPane — exercising
// its actual JSX and the Reconnect button's real `onClick` — against a minimal
// xterm stub and a fake PtySocket. The fake captures the pane's own `onConn`
// handler (TerminalPane assigns `pty.onConn = …` in its wiring effect) so a test
// can drive a `failed`/`open` transition, and exposes a `connect` spy so we can
// assert the Reconnect button reconnects THIS pane's socket directly rather than
// falling back to the epoch-only store no-op that never remounts a companion
// terminal. That no-op is the regression this file guards, for BOTH kinds.

class TermStub {
  static instances: TermStub[] = []
  // The options object TerminalPane constructs the Terminal with, captured so
  // tests can pin construction-time options (e.g. scrollSensitivity).
  options: Record<string, unknown>
  constructor(options?: Record<string, unknown>) {
    this.options = options ?? {}
    TermStub.instances.push(this)
  }
  rows = 24
  cols = 80
  // `value` and `focused` are both real. xterm's own `contextmenu` handler
  // stuffs the current selection into this hidden textarea AND focuses it
  // (`moveTextAreaUnderMouseCursor`), so the pane's guard has to wipe the value
  // or it leaks back into the PTY as a paste, and hand focus back on touch or
  // the soft keyboard rises over the selection.
  textarea = {
    setAttribute() {},
    focused: false,
    blur() {
      this.focused = false
    },
    focus() {
      this.focused = true
    },
    value: "",
  }
  // The scrollback the selection tests read words out of. `lines` is the whole
  // buffer and `viewportY` is the first line on screen, so a test can scroll
  // and see the pane resolve a DIFFERENT absolute row from the same finger
  // position. `getLine`/`getCell` mirror the shape `rowCells` consumes; only
  // single-width cells are modelled here, because the wide-glyph rules are
  // pinned against a REAL xterm buffer in `lib/termselect.xterm.test.ts`.
  lines: string[] = ["git status --porcelain", "second line here"]
  // Whether each line CONTINUES the one above it, for the wrapped-word rules.
  wrapped: boolean[] = []
  // A line as COLUMNS, not characters. A wide glyph really does take two of
  // them (the glyph, then a zero-width continuation cell), because column and
  // character index part company there and that is the whole point of the
  // wide-glyph handling under test. The range is the one xterm's own default
  // Unicode provider calls wide; an emoji is deliberately NOT in it (measured
  // in `lib/termselect.xterm.test.ts`).
  static WIDE =
    /[\u1100-\u115F\u2E80-\uA4CF\uAC00-\uD7A3\uF900-\uFAFF\uFE30-\uFE6F\uFF00-\uFF60\uFFE0-\uFFE6]/
  lineCells(y: number): { chars: string; width: number }[] | undefined {
    const text = this.lines[y]
    if (text === undefined) return undefined
    const cells: { chars: string; width: number }[] = []
    for (const ch of text) {
      if (TermStub.WIDE.test(ch)) {
        cells.push({ chars: ch, width: 2 }, { chars: "", width: 0 })
      } else {
        cells.push({ chars: ch, width: 1 })
      }
    }
    while (cells.length < this.cols) cells.push({ chars: "", width: 1 })
    return cells.slice(0, this.cols)
  }
  buffer = {
    active: {
      type: "normal",
      viewportY: 0,
      getLine: (y: number) => {
        const cells = this.lineCells(y)
        if (!cells) return undefined
        return {
          length: this.cols,
          isWrapped: this.wrapped[y] ?? false,
          getCell: (x: number) => ({
            getChars: () => cells[x]?.chars ?? "",
            getWidth: () => cells[x]?.width ?? 1,
          }),
        }
      },
    },
  }
  // xterm's selection model, faithful to the one thing the pane depends on:
  // `select(col, row, length)` is a forward start-plus-length whose length
  // WRAPS across rows at `cols` (`SelectionModel.finalSelectionEnd`).
  selection: { col: number; row: number; length: number } | null = null
  select(col: number, row: number, length: number) {
    this.selection = { col, row, length }
  }
  hasSelection() {
    return this.selection !== null && this.selection.length > 0
  }
  modes = {
    mouseTrackingMode: "none",
    applicationCursorKeysMode: false,
    bracketedPasteMode: false,
  }
  // The pane registers a parser-level OSC 8 gate directly on the terminal (the
  // hyperlink on/off gate), so the stub must expose registerOscHandler.
  parser = {
    registerOscHandler() {
      return { dispose() {} }
    },
  }
  // Real xterm hands itself to an addon through `activate`; mirroring that lets
  // FitStub re-grid the terminal the way the real FitAddon does.
  loadAddon(addon: { activate?: (term: TermStub) => void }) {
    addon.activate?.(this)
  }
  // xterm's own DOM: `Terminal.element` is the `.xterm` div it creates inside
  // the host, and `.xterm-screen` is the child its coordinate math measures.
  // Both are real nodes here so the mouse-replay path can dispatch at them, and
  // `installXtermMouseModel` stands in for the pipeline jsdom cannot run (see
  // `lib/xtermMouseModel.ts`). Rects are stubbed because jsdom reports zeros.
  element: HTMLElement | null = null
  mouse: ReturnType<typeof installXtermMouseModel> | null = null
  focusCalls = 0
  static mouseGeometry = {
    left: 100,
    top: 50,
    cellWidth: 10,
    cellHeight: 20,
    paddingLeft: 0,
    paddingTop: 0,
  }
  open(container: HTMLElement) {
    const g = TermStub.mouseGeometry
    const element = container.ownerDocument.createElement("div")
    element.className = "xterm"
    const screen = container.ownerDocument.createElement("div")
    screen.className = "xterm-screen"
    element.appendChild(screen)
    container.appendChild(element)
    const rect = (el: HTMLElement, w: number, h: number) => {
      el.getBoundingClientRect = () =>
        ({
          left: g.left,
          top: g.top,
          right: g.left + w,
          bottom: g.top + h,
          width: w,
          height: h,
          x: g.left,
          y: g.top,
          toJSON() {},
        }) as DOMRect
    }
    rect(element, this.cols * g.cellWidth, this.rows * g.cellHeight)
    rect(screen, this.cols * g.cellWidth, this.rows * g.cellHeight)
    this.element = element
    this.mouse = installXtermMouseModel({
      element,
      screen,
      cols: this.cols,
      rows: this.rows,
      cellWidth: g.cellWidth,
      cellHeight: g.cellHeight,
      paddingLeft: g.paddingLeft,
      paddingTop: g.paddingTop,
      onData: (d) => this.dataHandler?.(d),
      onBinary: (d) => this.binaryHandler?.(d),
      onFocus: () => {
        this.focusCalls++
      },
    })
  }
  dataHandler: ((s: string) => void) | null = null
  binaryHandler: ((s: string) => void) | null = null
  onData(cb: (s: string) => void) {
    this.dataHandler = cb
    return {
      dispose: () => {
        this.dataHandler = null
      },
    }
  }
  onBinary(cb: (s: string) => void) {
    this.binaryHandler = cb
    return {
      dispose: () => {
        this.binaryHandler = null
      },
    }
  }
  // xterm's own resize event. It fires only when the grid actually CHANGES, and
  // it is the pane's single choke point for reporting geometry to the PTY, so
  // the stub has to be faithful about both halves.
  resizeListeners: ((size: { cols: number; rows: number }) => void)[] = []
  onResize(cb: (size: { cols: number; rows: number }) => void) {
    this.resizeListeners.push(cb)
    return {
      dispose: () => {
        this.resizeListeners = this.resizeListeners.filter((l) => l !== cb)
      },
    }
  }
  // xterm's signature is resize(columns, rows).
  resize(cols: number, rows: number) {
    if (cols === this.cols && rows === this.rows) return
    this.cols = cols
    this.rows = rows
    for (const cb of [...this.resizeListeners]) cb({ cols, rows })
  }
  attachCustomKeyEventHandler() {}
  focus() {}
  getSelection() {
    const sel = this.selection
    if (!sel || sel.length <= 0) return ""
    let out = ""
    let remaining = sel.length
    let row = sel.row
    let col = sel.col
    while (remaining > 0) {
      const take = Math.min(remaining, this.cols - col)
      const cells = this.lineCells(row) ?? []
      // Join the CHARS of the covered columns; a continuation cell carries
      // none, so a wide glyph contributes itself once across its two columns.
      for (let x = col; x < col + take; x++) out += cells[x]?.chars ?? ""
      remaining -= take
      row++
      col = 0
      if (remaining > 0) out += "\n"
    }
    return out
  }
  selectAll() {}
  // Counted and applied: the auto-scroll test asserts BOTH that the pane
  // scrolled and that the row the selection then resolves to moved with it.
  scrollLineCalls: number[] = []
  scrollLines(amount: number) {
    this.scrollLineCalls.push(amount)
    this.buffer.active.viewportY = Math.max(
      0,
      this.buffer.active.viewportY + amount,
    )
  }
  scrollToBottom() {}
  clearSelection() {
    this.selection = null
  }
  reset() {}
  paste() {}
  write(_data: unknown, cb?: () => void) {
    cb?.()
  }
  dispose() {}
}

class FitStub {
  // Counted so the live font-preference suite can assert a settings change
  // refits the open terminal (reset in beforeEach).
  static fits = 0
  // Arm the NEXT fit to re-grid the terminal, the way a real fit does once the
  // cell metrics move under it (a font landing, a container change). One-shot,
  // so mounting's own fits cannot consume a value a test armed for a later one.
  static nextDims: { rows: number; cols: number } | null = null
  term: TermStub | null = null
  activate(term: TermStub) {
    this.term = term
  }
  fit() {
    FitStub.fits++
    const next = FitStub.nextDims
    if (next && this.term) {
      FitStub.nextDims = null
      this.term.resize(next.cols, next.rows)
    }
  }
}

// The fake PtySocket. TerminalPane overwrites `onConn`/`onOpen`/`onReconnecting`/
// `onConnected` with its own handlers and calls `onBytes`, `connect`, `close`,
// `sendResize`, `sendInput`. `connect` is a spy so the Reconnect assertion can
// count calls. `emit` is a helper for tests to drive the pane's captured `onConn`.
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
  // Mirrors the real socket's `isOpen` getter; a test flips it to false to
  // model a disconnected socket (the compose bar's Send checks it).
  isOpen = true
  onConnected: (id: string) => void = () => {}
  onOpen: () => void = () => {}
  onReconnecting: () => void = () => {}
  onConn: (state: ConnState) => void = () => {}
  // Captured so a test can deliver the server's first (repaint) frame, which is
  // what the pane's deferred initial/reconnect resize hangs off.
  bytesCb: ((b: Uint8Array) => void) | null = null
  onBytes = (cb: (b: Uint8Array) => void): void => {
    this.bytesCb = cb
  }
  shouldRetry: () => boolean = () => true
  onGone: () => void = () => {}
  constructor(url: string) {
    this.url = url
    FakePtySocket.instances.push(this)
  }
  emit(state: ConnState): void {
    act(() => this.onConn(state))
  }
}

vi.mock("@xterm/xterm", () => ({ Terminal: TermStub }))
vi.mock("@xterm/addon-fit", () => ({ FitAddon: FitStub }))
// Spy on sonner so the compose bar's refused-send tests can assert the user
// was told WHY the message stayed in the buffer (owner/offline/oversized).
const toastError = vi.fn()
// Plain `toast(...)` bodies, so the long-press suite can assert that selecting
// over a mouse-tracking app does NOT produce the mouse-only "hold Shift and
// drag" hint.
const toastCalls: unknown[] = []
vi.mock("sonner", () => ({
  toast: Object.assign((...args: unknown[]) => void toastCalls.push(args[0]), {
    success: vi.fn(),
    error: (...args: unknown[]) => toastError(...args),
  }),
}))
// What reached the clipboard. The real helper needs `navigator.clipboard` or
// `document.execCommand`, neither of which jsdom provides, so a stub is the
// only way to tell "copied" from "tried and failed".
const copied: string[] = []
vi.mock("@/lib/clipboard", () => ({
  copyToClipboard: async (text: string) => {
    copied.push(text)
    return true
  },
}))
vi.mock("@/lib/suppressViewerReports", () => ({ suppressViewerReports: () => {} }))
const notifyRegistrations: { title: () => string }[] = []
vi.mock("@/lib/agentNotifications", () => ({
  registerAgentNotifications: (_term: unknown, opts: { title: () => string }) => {
    notifyRegistrations.push(opts)
    return () => {}
  },
}))
// A marker stand-in rather than null so the floating-trigger suite below can
// assert WHERE the pane composes it (desktop only) without pulling in the
// real popover's Command/Popover machinery.
vi.mock("@/components/MacroPopover", () => ({
  MacroPopover: () => <div data-testid="macro-popover" />,
}))
vi.mock("@/lib/ptySocket", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/ptySocket")>()
  return { ...actual, PtySocket: FakePtySocket }
})

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState }
})

// Stub browser globals BEFORE the store module evaluates (it touches
// localStorage at import time, pulled in transitively by TerminalPane).
installStubs()
const { TerminalPane } = await import("./TerminalPane")

function makeState(offline = false, conn: ConnState = "open"): DuxState {
  return {
    conn,
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
      terminals: [
        {
          id: "t1",
          owner: { kind: "session", session_id: "s1" },
          has_output: false,
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
    },
    offline,
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

function last(): FakePtySocket {
  const inst = FakePtySocket.instances.at(-1)
  if (!inst) throw new Error("no PtySocket constructed")
  return inst
}

beforeEach(() => {
  FakePtySocket.instances = []
  TermStub.instances = []
  FitStub.fits = 0
  FitStub.nextDims = null
  notifyRegistrations.length = 0
  toastError.mockClear()
  toastCalls.length = 0
  copied.length = 0
  mockState = makeState()
  installStubs()
  // The `pty.owner` epoch high-water marks are module-global; reset so a handover
  // in one test is never dropped as "stale" by a prior test's epoch.
  resetPtyOwnerEpochs()
  // The compose-insert sink is module-global too; a pane left registered by a
  // prior test must never satisfy this test's assertions.
  setComposeInsertSink(null)
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

// Run the same behavioral contract for both surfaces the pane serves. The
// companion-terminal ("terminal") case is the load-bearing one: an epoch-only
// reconnect is a NO-OP for it, so this parametrization is the regression guard.
// Build the pane's (union-typed) props for either surface: an agent carries
// its owning session id, a terminal its owner ref.
function paneProps(kind: "agent" | "terminal", id: string) {
  return kind === "agent"
    ? ({ kind: "agent", id, sessionId: "s1" } as const)
    : ({ kind: "terminal", id, owner: { kind: "session", sessionId: "s1" } } as const)
}

describe.each([
  { kind: "agent" as const, id: "s1" },
  { kind: "terminal" as const, id: "t1" },
])("TerminalPane connectionLost affordance ($kind)", ({ kind, id }) => {
  it("shows the Reconnect affordance on 'failed' without doubling the spinner", () => {
    render(<TerminalPane {...paneProps(kind, id)} />)
    last().emit("failed")
    expect(screen.getByText("Connection lost.")).toBeTruthy()
    expect(screen.getByText("Reconnect")).toBeTruthy()
    // The connection-lost block replaces (does not stack with) the reconnecting
    // spinner — no double overlay.
    expect(screen.queryByText("Reconnecting…")).toBeNull()
  })

  it("Reconnect calls the pane's OWN socket.connect() (not an epoch no-op)", () => {
    render(<TerminalPane {...paneProps(kind, id)} />)
    const pty = last()
    pty.emit("failed")
    // Ignore the connect() the wiring effect already fired on mount; the button
    // must fire a fresh one on THIS socket. For a companion terminal an
    // epoch-only reconnect would never reach here — that is the regression.
    pty.connect.mockClear()
    fireEvent.click(screen.getByText("Reconnect"))
    expect(pty.connect).toHaveBeenCalledTimes(1)
  })

  it("clears the affordance once the socket reopens ('open')", () => {
    render(<TerminalPane {...paneProps(kind, id)} />)
    const pty = last()
    pty.emit("failed")
    expect(screen.getByText("Connection lost.")).toBeTruthy()
    pty.emit("open")
    expect(screen.queryByText("Connection lost.")).toBeNull()
  })

  it("suppresses its own connectionLost overlay while globally offline", () => {
    mockState = makeState(true)
    render(<TerminalPane {...paneProps(kind, id)} />)
    last().emit("failed")
    // The app-wide OfflineOverlay owns the offline case; the `&& !offline` gate
    // hides the per-pane affordance so the two never double up.
    expect(screen.queryByText("Connection lost.")).toBeNull()
    expect(screen.queryByText("Reconnect")).toBeNull()
  })
})

// The take-over placeholder's device naming: a `pty.owner` handover carrying the
// other device's raw User-Agent must render "Open on {parsed label}", our own claim
// echo must restore the owner view, and a non-open events socket must drop the
// specific name back to the generic copy (the stale-name-on-reconnect fix).
describe("TerminalPane take-over device naming", () => {
  const chromeMac =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"

  it("names the other device in the modal when a handover carries a device UA", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    // A foreign device claims the PTY (owner id is not ours) with its UA attached.
    act(() => notifyPtyOwner("s1", "conn-other", undefined, chromeMac))
    expect(screen.getByText("Open on Chrome on macOS")).toBeTruthy()
  })

  it("clears the label and restores the owner view on our own claim echo", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    // Learn our own connection id so an echo with that id reads as ours.
    act(() => last().onConnected("conn-self"))
    act(() => notifyPtyOwner("s1", "conn-other", undefined, chromeMac))
    expect(screen.getByText("Open on Chrome on macOS")).toBeTruthy()
    // Our own claim echoes back -> we are the owner again; the placeholder (and its
    // device name) disappears entirely.
    act(() => notifyPtyOwner("s1", "conn-self", undefined, chromeMac))
    expect(screen.queryByText(/Open on/)).toBeNull()
    expect(screen.queryByText("Active on another device")).toBeNull()
  })

  it("drops the specific name to the generic copy when events socket is not open", () => {
    const { rerender } = render(
      <TerminalPane kind="agent" id="s1" sessionId="s1" />,
    )
    act(() => notifyPtyOwner("s1", "conn-other", undefined, chromeMac))
    expect(screen.getByText("Open on Chrome on macOS")).toBeTruthy()
    // The events socket drops (conn !== "open"): the specific-but-now-possibly-stale
    // device name is cleared, falling back to the always-correct generic copy.
    mockState = makeState(false, "closed")
    act(() => rerender(<TerminalPane kind="agent" id="s1" sessionId="s1" />))
    expect(screen.queryByText("Open on Chrome on macOS")).toBeNull()
    expect(screen.getByText("Active on another device")).toBeTruthy()
  })
})

// The pane is the only surface that KNOWS a PTY's input ownership (from
// `pty.owner` handovers on its own socket), so it publishes the verdict into
// the store ledger the agent ⋯ menu gates its mutating entries on.
describe("TerminalPane ownership reporting into the store", () => {
  it("reports the live verdict on takeover, reclaim, and retirement on unmount", async () => {
    const store = await import("@/lib/store")
    const { unmount } = render(
      <TerminalPane kind="agent" id="s1" sessionId="s1" />,
    )
    // A foregrounded pane assumes ownership until told otherwise, and says so:
    // the "mine" verdict is what overrides a stale spine `input_owner` right
    // after a take-over.
    expect(store.getSnapshot().ptyOwnership["s1"]).toBe("mine")
    act(() => notifyPtyOwner("s1", "conn-other"))
    expect(store.getSnapshot().ptyOwnership["s1"]).toBe("elsewhere")
    // Take over from here: the placeholder's button reclaims and the verdict
    // flips back with it.
    act(() => {
      fireEvent.click(screen.getByText("Take over"))
    })
    expect(store.getSnapshot().ptyOwnership["s1"]).toBe("mine")
    // Owned elsewhere again, then unmount: with the pane gone this client has
    // no live verdict, so the entry must retire entirely (the server-published
    // spine field answers alone from here).
    act(() => notifyPtyOwner("s1", "conn-other-2"))
    expect(store.getSnapshot().ptyOwnership["s1"]).toBe("elsewhere")
    unmount()
    expect(store.getSnapshot().ptyOwnership["s1"]).toBeUndefined()
  })

  it("does not report companion-terminal PTYs (a terminal is not the agent)", async () => {
    const store = await import("@/lib/store")
    render(
      <TerminalPane
        kind="terminal"
        id="t1"
        owner={{ kind: "session", sessionId: "s1" }}
      />,
    )
    act(() => notifyPtyOwner("t1", "conn-other"))
    expect(store.getSnapshot().ptyOwnership["t1"]).toBeUndefined()
  })

  it("registers its socket id as this client's own and retires it on reconnect and unmount", async () => {
    // The own-connection set is the identity half of the server-published
    // ownership comparison: a spine `input_owner` naming an id in this set is
    // OUR ownership, anything else is another connection's. A dropped call
    // site here would make a client read its OWN agent as active elsewhere.
    const store = await import("@/lib/store")
    const { unmount } = render(
      <TerminalPane kind="agent" id="s1" sessionId="s1" />,
    )
    const pty = FakePtySocket.instances.at(-1)!
    act(() => pty.onConnected("c7"))
    expect(store.getSnapshot().ownPtyConnIds["c7"]).toBe(true)
    // The socket drops: the server has released anything c7 owned, so the id
    // must retire immediately, not linger until the reopen.
    act(() => pty.onReconnecting())
    expect(store.getSnapshot().ownPtyConnIds["c7"]).toBeUndefined()
    // The reopen mints a fresh id.
    act(() => {
      pty.onOpen()
      pty.onConnected("c8")
    })
    expect(store.getSnapshot().ownPtyConnIds["c8"]).toBe(true)
    unmount()
    expect(store.getSnapshot().ownPtyConnIds["c8"]).toBeUndefined()
  })

  it("retires its ownership verdict while the socket has failed for good", async () => {
    // A pane whose socket never (re)connects knows nothing about the live
    // PTY; a stale "mine" would override the server-published owner forever
    // on a surface that cannot even type.
    const store = await import("@/lib/store")
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(store.getSnapshot().ptyOwnership["s1"]).toBe("mine")
    const pty = FakePtySocket.instances.at(-1)!
    pty.emit("failed")
    expect(store.getSnapshot().ptyOwnership["s1"]).toBeUndefined()
    // A successful reopen restores the live verdict.
    pty.emit("open")
    expect(store.getSnapshot().ptyOwnership["s1"]).toBe("mine")
  })
})

// T9: a project terminal must resolve its OWNER (a project), not scan sessions.
// With the old session-only resolution, its desktop notifications were titled
// "Agent", its readiness never latched (hasOutput stayed false), and the pane
// opened the session-nested PTY route (a silent 404).
describe("TerminalPane project-terminal owner resolution", () => {
  function projectState(hasOutput: boolean): DuxState {
    const base = makeState()
    return {
      ...base,
      spine: {
        projects: [{ id: "p1", name: "Repo" }],
        sessions: [],
        terminals: [
          {
            id: "pt-1",
            owner: { kind: "project", project_id: "p1" },
            label: "Terminal 2",
            has_output: hasOutput,
            foreground_cmd: null,
          },
        ],
        sidebar: { groups: [], agentless_start: null },
      },
    } as unknown as DuxState
  }

  it("opens the PROJECT-nested PTY socket URL", () => {
    mockState = projectState(true)
    render(
      <TerminalPane
        kind="terminal"
        id="pt-1"
        owner={{ kind: "project", projectId: "p1" }}
      />,
    )
    expect(last().url).toMatch(/\/ws\/projects\/p1\/terminals\/pt-1\/pty$/)
  })

  it("titles desktop notifications with the project name, not 'Agent'", () => {
    mockState = projectState(true)
    render(
      <TerminalPane
        kind="terminal"
        id="pt-1"
        owner={{ kind: "project", projectId: "p1" }}
      />,
    )
    expect(notifyRegistrations).toHaveLength(1)
    expect(notifyRegistrations[0].title()).toBe("Repo")
  })

  it("reads readiness from the project's own TerminalView", () => {
    // has_output true on the PROJECT's terminal: the readiness spinner is
    // skipped. The old session scan saw `undefined ?? false` forever, so the
    // "Launching terminal…" card never cleared.
    mockState = projectState(true)
    render(
      <TerminalPane
        kind="terminal"
        id="pt-1"
        owner={{ kind: "project", projectId: "p1" }}
      />,
    )
    expect(screen.queryByText("Launching terminal…")).toBeNull()
  })

  it("shows the launch spinner while the project terminal has no output yet", () => {
    mockState = projectState(false)
    render(
      <TerminalPane
        kind="terminal"
        id="pt-1"
        owner={{ kind: "project", projectId: "p1" }}
      />,
    )
    expect(screen.getByText("Launching terminal…")).toBeTruthy()
  })
})

// The macro trigger no longer floats over the PTY at ANY width. On desktop it
// moved into the center pane's top bar (`InsetHeader`), parked on this pane's
// right edge; on a phone it has always lived in the terminal screen's header
// (MobileShell), because the overlay sat on top of the output and made the text
// under it unreadable. What the pane keeps is the FOCUS hand-off: the header's
// picker is outside this component and cannot reach xterm, so the pane
// registers its typing surface for it.
describe("TerminalPane macro trigger", () => {
  const desktopWidth = window.innerWidth
  afterEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: desktopWidth,
      configurable: true,
    })
  })

  it("renders no macro trigger over the terminal on desktop", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(screen.queryByTestId("macro-popover")).toBeNull()
  })

  it("renders no macro trigger over the terminal on mobile", () => {
    Object.defineProperty(window, "innerWidth", {
      value: 500,
      configurable: true,
    })
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(screen.queryByTestId("macro-popover")).toBeNull()
  })

  it("registers its typing surface so the header's picker can return focus to it", async () => {
    // Without this, closing the header picker hands focus back to the trigger
    // and the Enter that was meant to submit the macro re-opens the menu, and the
    // exact bug the pane's old `finalFocus` prop existed to prevent.
    const { getTerminalFocusElement, peekTerminalFocusTarget } = await import(
      "@/lib/terminalFocus"
    )
    // Deliberately no assertion that the slot starts empty: it is module-global
    // state shared by every test in this file, so a pane a neighbouring test
    // left mounted would make that assertion order-dependent. What matters is
    // that THIS pane claims the slot and gives it back.
    const view = render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(peekTerminalFocusTarget()).not.toBeNull()
    expect(getTerminalFocusElement()).toBe(TermStub.instances.at(-1)!.textarea)
    view.unmount()
    expect(peekTerminalFocusTarget()).toBeNull()
  })
})

// The mobile compose bar (the `ui.compose_bar` preference, default on): the
// third row of the mobile shell, whose Send delivers the buffered message plus
// a submitting Enter to the PTY through the pure `composeSendBytes` rules.
// `useIsMobile` reads `window.innerWidth`, so shrinking it below the 768px
// breakpoint is how these tests mount the mobile shell.
describe("TerminalPane mobile compose bar", () => {
  const desktopWidth = window.innerWidth
  // `goMobile` now means "the phone SHELL is up AND touch is the primary
  // pointer". Those are two different signals since the compose bar moved off
  // the width breakpoint: width still drives the mobile layout, but the
  // compose bar itself is gated on `pointer: coarse` (see
  // `hooks/use-coarse-pointer.ts`). Real phones report both, so these suites
  // set both; the tests that pull them APART live in the compose-bar gate
  // suite at the end of this file.
  let pointerStub: MatchMediaStub | null = null
  const goMobile = () => {
    Object.defineProperty(window, "innerWidth", {
      value: 500,
      configurable: true,
    })
    pointerStub = stubCoarsePointer()
  }
  afterEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: desktopWidth,
      configurable: true,
    })
    pointerStub?.restore()
    pointerStub = null
  })

  const composeTextarea = () =>
    screen.getByRole("textbox", { name: "Message" }) as HTMLTextAreaElement
  const sendButton = () => screen.getByRole("button", { name: "Send" })
  const bytesOf = (call: unknown[]) =>
    new TextDecoder().decode(call[0] as Uint8Array)

  it("renders on mobile by default (preference absent falls back to on)", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(composeTextarea()).toBeTruthy()
    expect(sendButton()).toBeTruthy()
  })

  it("does not render when the ui.compose_bar preference is off", () => {
    goMobile()
    const state = makeState()
    ;(state.bootstrap as unknown as { compose_bar?: string }).compose_bar =
      "never"
    mockState = state
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(screen.queryByRole("textbox", { name: "Message" })).toBeNull()
  })

  it("does not render on desktop", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(screen.queryByRole("textbox", { name: "Message" })).toBeNull()
  })

  it("Send writes the body first and the submitting CR as a DELAYED second write", () => {
    // Claude Code merges stdin chunks into one paste through a measured 50ms
    // debounce, swallowing a same-window CR into the paste as a newline. The
    // Enter must travel alone, a beat later, like a human's (see
    // COMPOSE_SUBMIT_DELAY_MS). Fake timers pin the two-write timing.
    vi.useFakeTimers()
    try {
      goMobile()
      render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
      const pty = last()
      fireEvent.change(composeTextarea(), { target: { value: "ls -la" } })
      fireEvent.pointerDown(sendButton())
      expect(pty.sendInput).toHaveBeenCalledTimes(1)
      expect(bytesOf(pty.sendInput.mock.calls[0])).toBe("ls -la")
      vi.advanceTimersByTime(COMPOSE_SUBMIT_DELAY_MS)
      expect(pty.sendInput).toHaveBeenCalledTimes(2)
      expect(bytesOf(pty.sendInput.mock.calls[1])).toBe("\r")
    } finally {
      vi.useRealTimers()
    }
  })

  it("a multiline body uses Alt+Enter newlines (the macro convention)", () => {
    vi.useFakeTimers()
    try {
      goMobile()
      render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
      const pty = last()
      fireEvent.change(composeTextarea(), { target: { value: "a\nb" } })
      fireEvent.pointerDown(sendButton())
      expect(bytesOf(pty.sendInput.mock.calls[0])).toBe("a\x1b\rb")
      vi.advanceTimersByTime(COMPOSE_SUBMIT_DELAY_MS)
      expect(bytesOf(pty.sendInput.mock.calls[1])).toBe("\r")
    } finally {
      vi.useRealTimers()
    }
  })

  it("an empty Send is ONE immediate bare CR (a lone keystroke, never delayed)", () => {
    vi.useFakeTimers()
    try {
      goMobile()
      render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
      const pty = last()
      fireEvent.pointerDown(sendButton())
      expect(pty.sendInput).toHaveBeenCalledTimes(1)
      expect(bytesOf(pty.sendInput.mock.calls[0])).toBe("\r")
      vi.advanceTimersByTime(COMPOSE_SUBMIT_DELAY_MS)
      expect(pty.sendInput).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it("skips the delayed CR when the pane unmounted before it fired", () => {
    vi.useFakeTimers()
    try {
      goMobile()
      const { unmount } = render(
        <TerminalPane kind="agent" id="s1" sessionId="s1" />,
      )
      const pty = last()
      fireEvent.change(composeTextarea(), { target: { value: "gone" } })
      fireEvent.pointerDown(sendButton())
      expect(pty.sendInput).toHaveBeenCalledTimes(1)
      // The pane goes away before the delay elapses: the orphaned CR must not
      // land on a socket the pane no longer owns.
      unmount()
      vi.advanceTimersByTime(COMPOSE_SUBMIT_DELAY_MS)
      expect(pty.sendInput).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it("skips the delayed CR when the socket dropped in between", () => {
    vi.useFakeTimers()
    try {
      goMobile()
      render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
      const pty = last()
      fireEvent.change(composeTextarea(), { target: { value: "dropped" } })
      fireEvent.pointerDown(sendButton())
      expect(pty.sendInput).toHaveBeenCalledTimes(1)
      pty.isOpen = false
      vi.advanceTimersByTime(COMPOSE_SUBMIT_DELAY_MS)
      expect(pty.sendInput).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })

  it("hides the compose bar AND the accessory bar for a non-owner viewer", () => {
    // If the session is being driven from another machine, this client's
    // typing surfaces disappear entirely: the take-over card is the only
    // interaction left, so there is no way to even stage input at a PTY this
    // device does not drive. (The Send owner-gate below stays as defense in
    // depth behind the hidden UI.)
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(composeTextarea()).toBeTruthy()
    expect(screen.getByRole("button", { name: "Esc" })).toBeTruthy()
    // A foreign device takes over: this view is demoted to a read-only viewer.
    act(() => notifyPtyOwner("s1", "conn-other", undefined, undefined))
    expect(screen.queryByRole("textbox", { name: "Message" })).toBeNull()
    expect(screen.queryByRole("button", { name: "Send" })).toBeNull()
    expect(screen.queryByRole("button", { name: "Esc" })).toBeNull()
  })

  it("a Send while the socket is down keeps the buffer and toasts", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    // Model a dropped connection: the real socket's isOpen getter reads the
    // WebSocket readyState; the fake exposes it as a writable flag.
    pty.isOpen = false
    fireEvent.change(composeTextarea(), { target: { value: "while offline" } })
    fireEvent.pointerDown(sendButton())
    expect(pty.sendInput).not.toHaveBeenCalled()
    expect(composeTextarea().value).toBe("while offline")
    expect(toastError).toHaveBeenCalledWith(
      "Not connected right now. Your message was kept.",
      expect.anything(),
    )
  })

  it("an oversized Send keeps the buffer and toasts instead of writing", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    // One byte over the 2 MiB client-side cap; the server would abort the
    // whole PTY socket on a genuinely oversized frame, so the client refuses.
    const huge = "a".repeat(2 * 1024 * 1024 + 1)
    fireEvent.change(composeTextarea(), { target: { value: huge } })
    fireEvent.pointerDown(sendButton())
    expect(pty.sendInput).not.toHaveBeenCalled()
    expect(composeTextarea().value).toBe(huge)
    expect(toastError).toHaveBeenCalled()
  })

  it("keeps in-progress text across a compose-bar unmount (pref flip off and on)", () => {
    goMobile()
    const { rerender } = render(
      <TerminalPane kind="agent" id="s1" sessionId="s1" />,
    )
    fireEvent.change(composeTextarea(), { target: { value: "draft survives" } })
    // The preference flips off: the bar unmounts. The buffer lives in
    // TerminalPane state, not the bar, so it must survive the round trip.
    const off = makeState()
    ;(off.bootstrap as unknown as { compose_bar?: string }).compose_bar = "never"
    mockState = off
    rerender(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(screen.queryByRole("textbox", { name: "Message" })).toBeNull()
    mockState = makeState()
    rerender(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(composeTextarea().value).toBe("draft survives")
  })
})

// The accessory keys must PRESERVE the soft-keyboard state, never change it:
// a user paging through output with the keyboard closed must not have it pop
// open because a key handler unconditionally refocused the typing surface,
// and a user mid-typing must not lose the keyboard to a key tap. The bar's
// buttons already preventDefault their pointerdown (they never take focus);
// what these tests pin is the HANDLER side — the refocus is conditional on
// the typing surface having had focus when the tap landed.
describe("TerminalPane accessory keys preserve the keyboard state", () => {
  const desktopWidth = window.innerWidth
  // `goMobile` now means "the phone SHELL is up AND touch is the primary
  // pointer". Those are two different signals since the compose bar moved off
  // the width breakpoint: width still drives the mobile layout, but the
  // compose bar itself is gated on `pointer: coarse` (see
  // `hooks/use-coarse-pointer.ts`). Real phones report both, so these suites
  // set both; the tests that pull them APART live in the compose-bar gate
  // suite at the end of this file.
  let pointerStub: MatchMediaStub | null = null
  const goMobile = () => {
    Object.defineProperty(window, "innerWidth", {
      value: 500,
      configurable: true,
    })
    pointerStub = stubCoarsePointer()
  }
  afterEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: desktopWidth,
      configurable: true,
    })
    pointerStub?.restore()
    pointerStub = null
  })

  const composeTextarea = () =>
    screen.getByRole("textbox", { name: "Message" }) as HTMLTextAreaElement
  const bytesOf = (call: unknown[]) =>
    new TextDecoder().decode(call[0] as Uint8Array)

  it("a key tap with the keyboard closed does NOT focus the compose textarea", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    // The keyboard is closed: nothing has focus (the user blurred to read).
    composeTextarea().blur()
    expect(document.activeElement).not.toBe(composeTextarea())
    fireEvent.pointerDown(screen.getByRole("button", { name: "Esc" }))
    // The key still reached the PTY…
    expect(pty.sendInput).toHaveBeenCalledTimes(1)
    expect(bytesOf(pty.sendInput.mock.calls[0])).toBe("\x1b")
    // …but the typing surface was NOT summoned (no soft keyboard pop).
    expect(document.activeElement).not.toBe(composeTextarea())
  })

  it("a key tap while typing keeps the compose textarea focused", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    composeTextarea().focus()
    fireEvent.pointerDown(screen.getByRole("button", { name: "Left" }))
    expect(pty.sendInput).toHaveBeenCalledTimes(1)
    expect(document.activeElement).toBe(composeTextarea())
  })

  it("an arrow tap with the keyboard closed leaves it closed too", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    composeTextarea().blur()
    fireEvent.pointerDown(screen.getByRole("button", { name: "Down" }))
    expect(pty.sendInput).toHaveBeenCalledTimes(1)
    expect(document.activeElement).not.toBe(composeTextarea())
  })

  it("a modifier latch tap preserves the keyboard state in both directions", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const ctrlKey = () => screen.getByRole("button", { name: "Ctrl" })
    // Closed stays closed…
    composeTextarea().blur()
    fireEvent.pointerDown(ctrlKey())
    expect(ctrlKey().getAttribute("aria-pressed")).toBe("true")
    expect(document.activeElement).not.toBe(composeTextarea())
    // …and open stays open.
    composeTextarea().focus()
    fireEvent.pointerDown(ctrlKey())
    expect(ctrlKey().getAttribute("aria-pressed")).toBe("false")
    expect(document.activeElement).toBe(composeTextarea())
  })

  it("a keyboard/AT activation (click with detail 0) still sends the key", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    fireEvent.click(screen.getByRole("button", { name: "Tab" }), { detail: 0 })
    expect(pty.sendInput).toHaveBeenCalledTimes(1)
    expect(bytesOf(pty.sendInput.mock.calls[0])).toBe("\t")
  })
})

// The compose-insert sink: while the compose bar is the rendered typing
// surface (mobile, preference on, input owner), the pane registers a sink the
// store's `runMacro` routes a picked macro through, so the macro text lands in
// the compose DRAFT (editable, sent later by Send) instead of going straight
// to the PTY. The sink exists exactly while the bar is rendered: desktop, a
// disabled preference, and a non-owner viewer all leave it unregistered, which
// is what keeps today's direct-to-PTY path for those cases.
describe("TerminalPane compose macro insert sink", () => {
  const desktopWidth = window.innerWidth
  // `goMobile` now means "the phone SHELL is up AND touch is the primary
  // pointer". Those are two different signals since the compose bar moved off
  // the width breakpoint: width still drives the mobile layout, but the
  // compose bar itself is gated on `pointer: coarse` (see
  // `hooks/use-coarse-pointer.ts`). Real phones report both, so these suites
  // set both; the tests that pull them APART live in the compose-bar gate
  // suite at the end of this file.
  let pointerStub: MatchMediaStub | null = null
  const goMobile = () => {
    Object.defineProperty(window, "innerWidth", {
      value: 500,
      configurable: true,
    })
    pointerStub = stubCoarsePointer()
  }
  afterEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: desktopWidth,
      configurable: true,
    })
    pointerStub?.restore()
    pointerStub = null
  })

  const composeTextarea = () =>
    screen.getByRole("textbox", { name: "Message" }) as HTMLTextAreaElement

  it("registers the sink while the compose bar is rendered (mobile owner)", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const sink = getComposeInsertSink()
    expect(sink).not.toBeNull()
    // The sink's focus target is the compose textarea itself (the macro
    // popover hands it to Base UI as the close-focus target).
    expect(sink?.target()).toBe(composeTextarea())
  })

  it("does not register the sink on desktop", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(getComposeInsertSink()).toBeNull()
  })

  it("does not register the sink when the ui.compose_bar preference is off", () => {
    goMobile()
    const state = makeState()
    ;(state.bootstrap as unknown as { compose_bar?: string }).compose_bar =
      "never"
    mockState = state
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(getComposeInsertSink()).toBeNull()
  })

  it("retires the sink when a foreign device takes over, restores it on reclaim", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    act(() => pty.onConnected("conn-me"))
    expect(getComposeInsertSink()).not.toBeNull()
    // Demoted to a read-only viewer: the compose bar unmounts, so a picked
    // macro must fall back to the (owner-gated) PTY path, not a hidden draft.
    act(() => notifyPtyOwner("s1", "conn-other", undefined, undefined))
    expect(getComposeInsertSink()).toBeNull()
    // Our own claim echo restores ownership, the bar, and the sink.
    act(() => notifyPtyOwner("s1", "conn-me", undefined, undefined))
    expect(getComposeInsertSink()).not.toBeNull()
  })

  it("retires the sink on unmount", () => {
    goMobile()
    const { unmount } = render(
      <TerminalPane kind="agent" id="s1" sessionId="s1" />,
    )
    expect(getComposeInsertSink()).not.toBeNull()
    unmount()
    expect(getComposeInsertSink()).toBeNull()
  })

  it("insert lands the text in the draft at the caret and writes NOTHING to the PTY", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    fireEvent.change(composeTextarea(), { target: { value: "hello world" } })
    composeTextarea().setSelectionRange(5, 5)
    act(() => getComposeInsertSink()?.insert(" brave"))
    expect(composeTextarea().value).toBe("hello brave world")
    // The caret sits after the inserted text, ready to keep editing.
    expect(composeTextarea().selectionStart).toBe(11)
    expect(composeTextarea().selectionEnd).toBe(11)
    expect(pty.sendInput).not.toHaveBeenCalled()
  })

  it("insert keeps multi-line macro text verbatim in the draft", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    act(() => getComposeInsertSink()?.insert("first\nsecond\nthird"))
    // Real newlines in the DRAFT: the Send path owns the wire transform
    // (newline-without-submit keystrokes plus the lone submitting CR).
    expect(composeTextarea().value).toBe("first\nsecond\nthird")
    expect(pty.sendInput).not.toHaveBeenCalled()
  })

  it("insert moves focus to the compose textarea", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    composeTextarea().blur()
    expect(document.activeElement).not.toBe(composeTextarea())
    act(() => getComposeInsertSink()?.insert("run the tests"))
    expect(document.activeElement).toBe(composeTextarea())
  })
})

// Desktop wheel scrolling: one wheel notch moves 3 lines of local scrollback,
// via xterm's scrollSensitivity option (the installed xterm 6 Viewport feeds it
// to its scrollable element as mouseWheelScrollSensitivity for LOCAL scrolling
// only; the wheel-report path to a mouse-tracking app sends one report per
// wheel event regardless, so forwarding stays 1:1 per tick).
describe("TerminalPane wheel scroll speed", () => {
  it("constructs the terminal with scrollSensitivity 3", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const term = TermStub.instances.at(-1)
    if (!term) throw new Error("no terminal constructed")
    expect(term.options.scrollSensitivity).toBe(3)
  })
})

// The tap-to-focus redirect, driven through real touch events on the terminal
// container (jsdom accepts plain {clientX, clientY} objects in the touch
// lists). touchend must be read via `changedTouches` (the `touches` list is
// empty once the finger lifts).
describe("TerminalPane tap-to-focus redirect", () => {
  const desktopWidth = window.innerWidth
  // `goMobile` now means "the phone SHELL is up AND touch is the primary
  // pointer". Those are two different signals since the compose bar moved off
  // the width breakpoint: width still drives the mobile layout, but the
  // compose bar itself is gated on `pointer: coarse` (see
  // `hooks/use-coarse-pointer.ts`). Real phones report both, so these suites
  // set both; the tests that pull them APART live in the compose-bar gate
  // suite at the end of this file.
  let pointerStub: MatchMediaStub | null = null
  const goMobile = () => {
    Object.defineProperty(window, "innerWidth", {
      value: 500,
      configurable: true,
    })
    pointerStub = stubCoarsePointer()
  }
  afterEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: desktopWidth,
      configurable: true,
    })
    pointerStub?.restore()
    pointerStub = null
  })

  const container = () => screen.getByTestId("terminal-container")
  const composeTextarea = () =>
    screen.getByRole("textbox", { name: "Message" }) as HTMLTextAreaElement
  const tap = (el: HTMLElement) => {
    fireEvent.touchStart(el, {
      touches: [{ clientX: 10, clientY: 10 }],
    })
    // fireEvent returns false iff preventDefault was called on the touchend,
    // which is the mechanism that stops xterm's synthetic-mousedown focus.
    return !fireEvent.touchEnd(el, {
      touches: [],
      changedTouches: [{ clientX: 10, clientY: 10 }],
    })
  }

  it("a tap preventDefaults and focuses the compose textarea (owner, pref on)", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const prevented = tap(container())
    expect(prevented).toBe(true)
    expect(document.activeElement).toBe(composeTextarea())
  })

  it("does not intercept the tap when the preference is off", () => {
    goMobile()
    const state = makeState()
    ;(state.bootstrap as unknown as { compose_bar?: string }).compose_bar =
      "never"
    mockState = state
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    // No preventDefault: the synthetic mouse events flow and xterm focuses its
    // hidden textarea exactly as before the compose bar existed.
    expect(tap(container())).toBe(false)
  })

  // The forwarded tap is a REPLAY, not an encoding: dux dispatches the mouse
  // events the swallowed synthetic ones would have been at `Terminal.element`
  // and xterm resolves the cell and picks the wire format. So these assert what
  // the APP received, through the transcribed pipeline in `lib/xtermMouseModel.ts`
  // (geometry: origin 100,50; 10x20 cells; 80x24 grid, so the canvas is 800x480).
  const sent = () => {
    const pty = FakePtySocket.instances.at(-1)
    if (!pty) throw new Error("no pty constructed")
    const dec = new TextDecoder("latin1")
    return pty.sendInput.mock.calls.map((c) =>
      dec.decode(c[0] as Uint8Array),
    )
  }
  const armed = (
    protocol: Parameters<
      NonNullable<InstanceType<typeof TermStub>["mouse"]>["setProtocol"]
    >[0],
    encoding: Parameters<
      NonNullable<InstanceType<typeof TermStub>["mouse"]>["setEncoding"]
    >[0],
  ) => {
    const term = TermStub.instances.at(-1)
    if (!term?.mouse) throw new Error("no term constructed")
    // `mouseTrackingMode` is what dux gates the forward on; the model carries
    // the protocol and the encoding, which xterm publishes neither of.
    term.modes.mouseTrackingMode = protocol === "X10" ? "x10" : "vt200"
    term.mouse.setProtocol(protocol)
    term.mouse.setEncoding(encoding)
    return term
  }
  // A cell, as a client point: the CENTRE of the 1-based cell (col, row).
  const at = (col: number, row: number) => ({
    clientX: 100 + (col - 1) * 10 + 5,
    clientY: 50 + (row - 1) * 20 + 10,
  })
  const tapAt = (point: { clientX: number; clientY: number }) => {
    const el = container()
    fireEvent.touchStart(el, { touches: [point] })
    return !fireEvent.touchEnd(el, { touches: [], changedTouches: [point] })
  }

  it("forwards a tap to a mouse-tracking app AND focuses compose", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    // The app in the PTY has grabbed the mouse: the swallowed tap must still
    // reach it as a left click (press + release) at the tapped cell, or
    // tap-driven TUIs go dead with the compose bar up.
    const term = armed("VT200", "SGR")
    expect(tapAt(at(4, 3))).toBe(true)
    expect(document.activeElement).toBe(composeTextarea())
    expect(sent()).toEqual(["\x1b[<0;4;3M", "\x1b[<0;4;3m"])
    // xterm's own mousedown handler grabbed focus; the redirect took it back.
    expect(term.focusCalls).toBe(1)
  })

  // One case per encoding xterm can actually be in. `?1005` (UTF-8) and `?1015`
  // (urxvt) are absent on purpose: the installed xterm parses both DECSETs and
  // ignores them ("DECSET 1005 not supported (see #2507)"), so it has no such
  // state to be in and dux can never owe an app those bytes.
  it("sends the DEFAULT (X10 byte) encoding on onBinary when the app never asked for SGR", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    armed("VT200", "DEFAULT")
    expect(tapAt(at(4, 3))).toBe(true)
    // `ESC [ M Cb Cx Cy`, each coordinate offset by 32. Press: button 0 -> 32
    // (space), col 4 -> 36 ($), row 3 -> 35 (#). Release: X10 has no button on
    // a release, so Cb is 3 + 32 = 35 (#).
    expect(sent()).toEqual(["\x1b[M \x24\x23", "\x1b[M\x23\x24\x23"])
  })

  it("sends SGR_PIXELS in pixels, not cells, when the app asked for ?1016", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    armed("VT200", "SGR_PIXELS")
    expect(tapAt(at(4, 3))).toBe(true)
    // The point is 35px, 50px into the canvas, and stays in pixels.
    expect(sent()).toEqual(["\x1b[<0;35;50M", "\x1b[<0;35;50m"])
  })

  it("sends NO release under the X10 protocol, which reports presses only", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    armed("X10", "DEFAULT")
    expect(tapAt(at(4, 3))).toBe(true)
    expect(sent()).toEqual(["\x1b[M \x24\x23"])
  })

  // Boundary cells. xterm clamps the point into the canvas and then rejects an
  // out-of-grid cell, so a tap in the padding resolves to the edge cell exactly
  // as a desktop click there does, and nothing lands outside 1..cols / 1..rows.
  it("resolves the first cell, the last cell, and each far edge", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    armed("VT200", "SGR")
    const press = () => sent()[0]
    const pty = () => FakePtySocket.instances.at(-1)!
    tapAt(at(1, 1))
    expect(press()).toBe("\x1b[<0;1;1M")
    pty().sendInput.mockClear()
    tapAt(at(80, 1))
    expect(press()).toBe("\x1b[<0;80;1M")
    pty().sendInput.mockClear()
    tapAt(at(1, 24))
    expect(press()).toBe("\x1b[<0;1;24M")
    pty().sendInput.mockClear()
    tapAt(at(80, 24))
    expect(press()).toBe("\x1b[<0;80;24M")
    pty().sendInput.mockClear()
    // Beyond every edge, in both directions: clamped onto the edge cell, never
    // dropped and never off-grid.
    tapAt({ clientX: -500, clientY: -500 })
    expect(press()).toBe("\x1b[<0;1;1M")
    pty().sendInput.mockClear()
    tapAt({ clientX: 5000, clientY: 5000 })
    expect(press()).toBe("\x1b[<0;80;24M")
  })

  it("forwards nothing when the app has no mouse tracking on", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(tapAt(at(4, 3))).toBe(true)
    expect(sent()).toEqual([])
    expect(document.activeElement).toBe(composeTextarea())
  })

})

// The accessory-bar render gate (the `ui.mobile_accessory_bar` preference,
// default on) sits beside the owner gate: hiding the key rows returns them to
// the terminal, while the compose bar (its own preference) stays.
// A touch drag on the ALT SCREEN of a mouse-tracking app is forwarded as wheel
// notches. Same replay, same reason: the app, not dux, picks the wire format.
describe("TerminalPane forwards a touch drag as wheel reports", () => {
  const container = () => screen.getByTestId("terminal-container")
  const sent = () => {
    const pty = FakePtySocket.instances.at(-1)
    if (!pty) throw new Error("no pty constructed")
    const dec = new TextDecoder("latin1")
    return pty.sendInput.mock.calls.map((c) => dec.decode(c[0] as Uint8Array))
  }
  // Alt screen (no xterm scrollback) plus a mouse-tracking app is the only
  // combination that forwards; anything else scrolls xterm locally.
  const armAltScreen = (encoding: "SGR" | "DEFAULT") => {
    const term = TermStub.instances.at(-1)
    if (!term?.mouse) throw new Error("no term constructed")
    term.buffer = { active: { type: "alternate" } }
    term.modes.mouseTrackingMode = "drag"
    term.mouse.setProtocol("DRAG")
    term.mouse.setEncoding(encoding)
    return term
  }
  // 20px past the threshold, upward: one notch of wheel-DOWN (newer output),
  // matching `dragScrollLines`' sign convention. jsdom reports a zero container
  // height, so the drag math falls back to its 16px row guard.
  const dragUp = () => {
    fireEvent.touchStart(container(), { touches: [{ clientX: 10, clientY: 300 }] })
    fireEvent.touchMove(container(), { touches: [{ clientX: 10, clientY: 280 }] })
  }

  it("sends exactly one SGR wheel report per move, at the finger's cell", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    armAltScreen("SGR")
    dragUp()
    // Button 64|action: action 1 (deltaY > 0) is wheel down. The point is left
    // of the canvas, so the column clamps to 1; y = 280 - 50 = 230px = row 12.
    expect(sent()).toEqual(["\x1b[<65;1;12M"])
  })

  it("sends the X10 byte form on onBinary when the app never asked for SGR", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    armAltScreen("DEFAULT")
    dragUp()
    // Cb 65 + 32 = 97 (a), col 1 + 32 = 33 (!), row 12 + 32 = 44 (,).
    expect(sent()).toEqual(["\x1b[Ma!,"])
  })

  it("forwards nothing on the alt screen when the app has no mouse tracking", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const term = TermStub.instances.at(-1)!
    term.buffer = { active: { type: "alternate" } }
    dragUp()
    expect(sent()).toEqual([])
  })
})

describe("TerminalPane mobile accessory-bar preference", () => {
  const desktopWidth = window.innerWidth
  // `goMobile` now means "the phone SHELL is up AND touch is the primary
  // pointer". Those are two different signals since the compose bar moved off
  // the width breakpoint: width still drives the mobile layout, but the
  // compose bar itself is gated on `pointer: coarse` (see
  // `hooks/use-coarse-pointer.ts`). Real phones report both, so these suites
  // set both; the tests that pull them APART live in the compose-bar gate
  // suite at the end of this file.
  let pointerStub: MatchMediaStub | null = null
  const goMobile = () => {
    Object.defineProperty(window, "innerWidth", {
      value: 500,
      configurable: true,
    })
    pointerStub = stubCoarsePointer()
  }
  afterEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: desktopWidth,
      configurable: true,
    })
    pointerStub?.restore()
    pointerStub = null
  })

  it("renders the accessory bar on mobile by default (preference absent falls back to on)", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(screen.getByRole("button", { name: "Esc" })).toBeTruthy()
  })

  it("hides the accessory bar when the preference is off, keeping the compose bar", () => {
    goMobile()
    const state = makeState()
    ;(
      state.bootstrap as unknown as { mobile_accessory_bar?: boolean }
    ).mobile_accessory_bar = false
    mockState = state
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(screen.queryByRole("button", { name: "Esc" })).toBeNull()
    expect(screen.getByRole("textbox", { name: "Message" })).toBeTruthy()
  })

  it("an optimistic override hides the accessory bar before the bootstrap confirms", () => {
    goMobile()
    const state = makeState()
    ;(
      state as unknown as { mobileAccessoryBarOverride: boolean | null }
    ).mobileAccessoryBarOverride = false
    mockState = state
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(screen.queryByRole("button", { name: "Esc" })).toBeNull()
  })
})

// The compose bar's restore button: the escape hatch back from hidden bars.
// It renders only while at least one bar is hidden, and one tap restores BOTH
// preferences through the same settings PATCH the quick toggles use.
describe("TerminalPane compose-bar restore button", () => {
  const desktopWidth = window.innerWidth
  // `goMobile` now means "the phone SHELL is up AND touch is the primary
  // pointer". Those are two different signals since the compose bar moved off
  // the width breakpoint: width still drives the mobile layout, but the
  // compose bar itself is gated on `pointer: coarse` (see
  // `hooks/use-coarse-pointer.ts`). Real phones report both, so these suites
  // set both; the tests that pull them APART live in the compose-bar gate
  // suite at the end of this file.
  let pointerStub: MatchMediaStub | null = null
  const goMobile = () => {
    Object.defineProperty(window, "innerWidth", {
      value: 500,
      configurable: true,
    })
    pointerStub = stubCoarsePointer()
  }
  afterEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: desktopWidth,
      configurable: true,
    })
    pointerStub?.restore()
    pointerStub = null
  })

  it("does not render while both bars are visible", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(
      screen.queryByRole("button", { name: "Show hidden bars" }),
    ).toBeNull()
  })

  it("renders when the accessory bar is hidden", () => {
    goMobile()
    const state = makeState()
    ;(
      state.bootstrap as unknown as { mobile_accessory_bar?: boolean }
    ).mobile_accessory_bar = false
    mockState = state
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(
      screen.getByRole("button", { name: "Show hidden bars" }),
    ).toBeTruthy()
  })

  it("renders when the top bar is hidden", () => {
    goMobile()
    const state = makeState()
    ;(
      state.bootstrap as unknown as { mobile_top_bar?: boolean }
    ).mobile_top_bar = false
    mockState = state
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(
      screen.getByRole("button", { name: "Show hidden bars" }),
    ).toBeTruthy()
  })

  it("one tap restores BOTH preferences through the settings PATCH", () => {
    goMobile()
    const state = makeState()
    ;(
      state.bootstrap as unknown as { mobile_top_bar?: boolean }
    ).mobile_top_bar = false
    mockState = state
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    fireEvent.click(screen.getByRole("button", { name: "Show hidden bars" }))
    const fetchSpy = globalThis.fetch as unknown as ReturnType<typeof vi.fn>
    expect(fetchSpy).toHaveBeenCalledWith(
      "/api/v1/config/settings",
      expect.objectContaining({
        method: "PATCH",
        // `quiet: true` asks the server to skip its "Settings updated."
        // status: the bars visibly returning is the feedback.
        body: JSON.stringify({
          ui: { mobile_top_bar: true, mobile_accessory_bar: true },
          quiet: true,
        }),
      }),
    )
  })
})

// The escape hatch when the compose bar itself is off: the terminal screen
// must never be chrome-free (the PWA has no browser Back button), so with a
// bar hidden and `ui.compose_bar` false the pane renders a minimal bottom row
// carrying ONLY the same restore button.
describe("TerminalPane restore row when the compose bar is off", () => {
  const desktopWidth = window.innerWidth
  // `goMobile` now means "the phone SHELL is up AND touch is the primary
  // pointer". Those are two different signals since the compose bar moved off
  // the width breakpoint: width still drives the mobile layout, but the
  // compose bar itself is gated on `pointer: coarse` (see
  // `hooks/use-coarse-pointer.ts`). Real phones report both, so these suites
  // set both; the tests that pull them APART live in the compose-bar gate
  // suite at the end of this file.
  let pointerStub: MatchMediaStub | null = null
  const goMobile = () => {
    Object.defineProperty(window, "innerWidth", {
      value: 500,
      configurable: true,
    })
    pointerStub = stubCoarsePointer()
  }
  afterEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: desktopWidth,
      configurable: true,
    })
    pointerStub?.restore()
    pointerStub = null
  })

  it("renders the restore button in its own bottom row when a bar is hidden", () => {
    goMobile()
    const state = makeState()
    ;(
      state.bootstrap as unknown as {
        compose_bar?: string
        mobile_top_bar?: boolean
      }
    ).compose_bar = "never"
    ;(
      state.bootstrap as unknown as { mobile_top_bar?: boolean }
    ).mobile_top_bar = false
    mockState = state
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    // No compose bar, but the restore escape hatch is still on screen.
    expect(screen.queryByRole("textbox", { name: "Message" })).toBeNull()
    expect(
      screen.getByRole("button", { name: "Show hidden bars" }),
    ).toBeTruthy()
  })

  it("renders nothing extra while both bars are visible", () => {
    goMobile()
    const state = makeState()
    ;(state.bootstrap as unknown as { compose_bar?: string }).compose_bar =
      "never"
    mockState = state
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(screen.queryByRole("textbox", { name: "Message" })).toBeNull()
    expect(
      screen.queryByRole("button", { name: "Show hidden bars" }),
    ).toBeNull()
  })
})

// A PTY resize that lands while a touch-scroll gesture is still in flight puts a
// SIGWINCH (a full child repaint) in the middle of the forwarded wheel-report
// stream, and a mouse-tracking alt-screen pager's repaint corrupts under that
// interleaving (persistently: an alt-screen has no client scrollback and nothing
// reconnects). The scroll-start blur makes this routine on phones: it collapses
// the soft keyboard, the viewport grows, and the debounced resize would fire
// under the finger. So the debounced send is HELD while a touch-scroll gesture
// is active and flushed once the finger lifts. These tests drive the real
// ResizeObserver debounce with a capturing stub plus fake timers.
describe("TerminalPane holds the PTY resize while a touch-scroll gesture is active", () => {
  // The capturing ResizeObserver: remembers each observer's callback so the
  // test can model the container growing (the keyboard collapse) on demand.
  let roCallbacks: (() => void)[]
  const installCapturingRO = () => {
    roCallbacks = []
    const cbs = roCallbacks
    vi.stubGlobal(
      "ResizeObserver",
      class {
        constructor(cb: () => void) {
          cbs.push(cb)
        }
        observe() {}
        unobserve() {}
        disconnect() {}
      },
    )
  }

  const container = () => screen.getByTestId("terminal-container")

  // Mount the pane, let the initial first-frame resize (the 250ms fallback plus
  // the 60ms jiggle tail) run to completion, and clear the spy so the tests
  // below observe ONLY the ResizeObserver-debounced sends.
  const mountSettled = () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    act(() => {
      vi.advanceTimersByTime(400)
    })
    const pty = last()
    pty.sendResize.mockClear()
    return pty
  }

  // Engage a scroll gesture: past the 8px threshold so touchScrolling arms.
  // jsdom rects are 0, so the row-height fallback (16px) applies; the local
  // scroll path consumes rows while the buffer stays "normal".
  const startScroll = () => {
    fireEvent.touchStart(container(), {
      touches: [{ clientX: 10, clientY: 300 }],
    })
    fireEvent.touchMove(container(), {
      touches: [{ clientX: 10, clientY: 280 }],
    })
  }

  beforeEach(() => {
    vi.useFakeTimers()
    installCapturingRO()
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  it("defers the debounced resize until touchend, then sends exactly once", () => {
    const pty = mountSettled()
    const term = TermStub.instances.at(-1)
    if (!term) throw new Error("no terminal constructed")
    startScroll()
    // The keyboard collapse: the container grows, the observer fires, and the
    // refit yields a new row count while the finger is still down.
    term.rows = 40
    act(() => {
      roCallbacks.forEach((cb) => cb())
      vi.advanceTimersByTime(250)
    })
    // Mid-gesture: nothing goes on the wire (no SIGWINCH inside the stream).
    expect(pty.sendResize).not.toHaveBeenCalled()
    // The finger lifts: the held resize flushes once, at the final size.
    fireEvent.touchEnd(container(), { touches: [], changedTouches: [] })
    act(() => {
      vi.advanceTimersByTime(250)
    })
    expect(pty.sendResize).toHaveBeenCalledTimes(1)
    expect(pty.sendResize).toHaveBeenCalledWith(40, 80)
  })

  it("touchcancel flushes a held resize too", () => {
    const pty = mountSettled()
    const term = TermStub.instances.at(-1)
    if (!term) throw new Error("no terminal constructed")
    startScroll()
    term.rows = 40
    act(() => {
      roCallbacks.forEach((cb) => cb())
      vi.advanceTimersByTime(250)
    })
    expect(pty.sendResize).not.toHaveBeenCalled()
    fireEvent.touchCancel(container(), { touches: [] })
    act(() => {
      vi.advanceTimersByTime(250)
    })
    expect(pty.sendResize).toHaveBeenCalledTimes(1)
    expect(pty.sendResize).toHaveBeenCalledWith(40, 80)
  })

  it("a gesture without any held resize sends nothing on touchend", () => {
    const pty = mountSettled()
    startScroll()
    fireEvent.touchEnd(container(), { touches: [], changedTouches: [] })
    act(() => {
      vi.advanceTimersByTime(500)
    })
    expect(pty.sendResize).not.toHaveBeenCalled()
  })

  it("a resize outside any gesture still sends through the normal debounce", () => {
    const pty = mountSettled()
    const term = TermStub.instances.at(-1)
    if (!term) throw new Error("no terminal constructed")
    term.rows = 40
    act(() => {
      roCallbacks.forEach((cb) => cb())
      vi.advanceTimersByTime(250)
    })
    expect(pty.sendResize).toHaveBeenCalledTimes(1)
    expect(pty.sendResize).toHaveBeenCalledWith(40, 80)
  })
})

describe("TerminalPane live font preferences", () => {
  it("applies a Preferences font change to the OPEN terminal and refits", async () => {
    const { rerender } = render(<TerminalPane {...paneProps("agent", "s1")} />)
    await act(async () => {})
    const term = TermStub.instances.at(-1)
    if (!term) throw new Error("no terminal constructed")
    // Construction-time defaults: the bundled stack (symbols face first) and
    // the default size.
    expect(String(term.options.fontFamily)).toMatch(
      /^"Dux Mono Symbols", "Dux Mono", "Dux Mono Fill", /,
    )
    expect(term.options.fontSize).toBe(14)
    const fitsBefore = FitStub.fits
    mockState = {
      ...mockState,
      bootstrap: {
        ...(mockState.bootstrap as unknown as Record<string, unknown>),
        terminal_font_family: "Iosevka",
        terminal_font_size: 20,
      },
    } as unknown as DuxState
    rerender(<TerminalPane {...paneProps("agent", "s1")} />)
    // The change lands on the SAME terminal instance (no remount) with the
    // user family prepended to the bundled stack, and a refit follows so
    // rows/cols track the new cell metrics.
    expect(TermStub.instances.at(-1)).toBe(term)
    expect(String(term.options.fontFamily)).toMatch(/^Iosevka, "Dux Mono Symbols", /)
    expect(term.options.fontSize).toBe(20)
    expect(FitStub.fits).toBeGreaterThan(fitsBefore)
  })

  it("degrades an out-of-range live size to the default instead of applying it raw", async () => {
    const { rerender } = render(<TerminalPane {...paneProps("terminal", "t1")} />)
    await act(async () => {})
    const term = TermStub.instances.at(-1)
    if (!term) throw new Error("no terminal constructed")
    mockState = {
      ...mockState,
      bootstrap: {
        ...(mockState.bootstrap as unknown as Record<string, unknown>),
        terminal_font_size: 500,
      },
    } as unknown as DuxState
    rerender(<TerminalPane {...paneProps("terminal", "t1")} />)
    expect(term.options.fontSize).toBe(14)
  })

  it("an unchanged rerender does not churn the options or refit", async () => {
    const { rerender } = render(<TerminalPane {...paneProps("agent", "s1")} />)
    await act(async () => {})
    const fitsBefore = FitStub.fits
    rerender(<TerminalPane {...paneProps("agent", "s1")} />)
    expect(FitStub.fits).toBe(fitsBefore)
  })
})

describe("TerminalPane bundled font load on mount", () => {
  afterEach(() => {
    // jsdom has no `document.fonts` by default; tests here add it, so remove
    // it again rather than leaking it into a later test's guard check.
    Reflect.deleteProperty(document, "fonts")
  })

  it("opens synchronously against fallback metrics, then loads the bundled fonts and refits", async () => {
    const load = vi.fn().mockResolvedValue([])
    Object.defineProperty(document, "fonts", {
      value: { load, check: () => false },
      configurable: true,
    })
    render(<TerminalPane {...paneProps("agent", "s1")} />)
    // The terminal is open (and its options set) even before the font-load
    // promise below has been given a chance to settle: opening never awaits
    // fonts.
    expect(TermStub.instances).toHaveLength(1)
    const fitsAfterMount = FitStub.fits
    await act(async () => {})
    expect(load).toHaveBeenCalledWith(
      expect.stringContaining('"Dux Mono"'),
      expect.any(String),
    )
    // The fill face is deliberately absent from the eager load, and this
    // assertion exists so re-adding it has to argue with a test. It is a
    // rarely hit backstop of ~79 KB; its `unicode-range` already makes the
    // browser fetch it lazily on first use of a code point the earlier faces
    // lack, and it cannot affect the cell grid, which xterm measures from a
    // `"W".repeat(32)` span whose code points fall outside every restricted
    // face's range. Forcing it here would cost every terminal mount, phones
    // included, a download nothing was waiting on.
    // Matched on the fill family leading the shorthand, which is what asks
    // for that face by name. The user-family call passes the whole stack, so
    // it mentions the fill family too, but its sample is the symbols sample:
    // CSS font matching hands U+2588 and U+28FF to "Dux Mono Symbols", which
    // leads the stack and really carries them, so the fill face is never
    // selected and never fetched.
    const fillCall = load.mock.calls.find(([shorthand]: [string]) =>
      /^\d+px "Dux Mono Fill"/.test(String(shorthand)),
    )
    expect(fillCall).toBeUndefined()
    // The font-load promise resolving triggers a refit on top of whatever
    // synchronous fits mounting itself already performed.
    expect(FitStub.fits).toBeGreaterThan(fitsAfterMount)
  })

  it("still opens the terminal, and warns once, when the font load rejects", async () => {
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {})
    const load = vi.fn().mockRejectedValue(new Error("offline"))
    Object.defineProperty(document, "fonts", {
      value: { load, check: () => false },
      configurable: true,
    })
    render(<TerminalPane {...paneProps("agent", "s1")} />)
    expect(TermStub.instances).toHaveLength(1)
    await act(async () => {})
    expect(warn).toHaveBeenCalledTimes(1)
    // The warning must not name a font: several faces load together and a
    // rejection does not say which one failed, so naming the configured
    // family would accuse it of a bundled face's failure.
    expect(warn.mock.calls[0][0]).toContain("a terminal font failed to load")
    expect(warn.mock.calls[0][0]).not.toContain("Dux Mono")
    warn.mockRestore()
  })
})

// Every LOCAL re-grid has to be reported to the PTY, whatever caused it. A fit()
// is not only ever the ResizeObserver's doing: the bundled fonts land after the
// terminal is already open, the cell metrics move, and the terminal re-grids with
// no container resize anywhere in sight. Nothing watched for that, so the PTY kept
// the size the fallback metrics produced and the child drew for one geometry while
// the browser rendered another, which on a phone left a copy of the agent's status
// line behind on every redraw. So the pane subscribes to xterm's own resize event
// and that is the single choke point; these tests pin it, and pin that the paths
// which deliberately BYPASS the dedupe still send.
describe("TerminalPane reports every local re-grid to the PTY", () => {
  let roCallbacks: (() => void)[]
  const installCapturingRO = () => {
    roCallbacks = []
    const cbs = roCallbacks
    vi.stubGlobal(
      "ResizeObserver",
      class {
        constructor(cb: () => void) {
          cbs.push(cb)
        }
        observe() {}
        unobserve() {}
        disconnect() {}
      },
    )
  }

  const term = () => {
    const t = TermStub.instances.at(-1)
    if (!t) throw new Error("no terminal constructed")
    return t
  }

  // Mount and let the deferred first-frame resize (the 250ms fallback plus the
  // 60ms jiggle tail) finish, so a test observes only what it triggers itself.
  const mountSettled = () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    act(() => {
      vi.advanceTimersByTime(400)
    })
    const pty = last()
    pty.sendResize.mockClear()
    return pty
  }

  beforeEach(() => {
    vi.useFakeTimers()
    installCapturingRO()
  })
  afterEach(() => {
    vi.useRealTimers()
    // jsdom has no `document.fonts`; the font test adds it, so take it back off.
    Reflect.deleteProperty(document, "fonts")
  })

  it("sends the resize when a FONT LOAD re-grids the terminal, with no container resize", async () => {
    // One shared pending promise for all four faces, so nothing resolves until
    // the test says so (Promise.all needs every one of them).
    let landFonts: () => void = () => {}
    const pending = new Promise<void>((resolve) => {
      landFonts = resolve
    })
    const load = vi.fn(() => pending)
    Object.defineProperty(document, "fonts", {
      value: { load, check: () => false },
      configurable: true,
    })
    const pty = mountSettled()
    // The bundled faces land: the cell metrics change, so the refit re-grids the
    // terminal. No ResizeObserver callback is fired anywhere in this test.
    FitStub.nextDims = { rows: 30, cols: 100 }
    await act(async () => {
      landFonts()
    })
    expect(term().rows).toBe(30)
    expect(term().cols).toBe(100)
    act(() => {
      vi.advanceTimersByTime(250)
    })
    expect(pty.sendResize).toHaveBeenCalledTimes(1)
    expect(pty.sendResize).toHaveBeenCalledWith(30, 100)
  })

  it("does not record a resize the owner gate dropped, so it is re-sent once ownership returns", () => {
    const pty = mountSettled()
    act(() => pty.onConnected("conn-self"))
    // Another device takes the PTY: this pane is a read-only viewer and its
    // resizes are dropped on the floor by the owner gate.
    act(() => notifyPtyOwner("s1", "conn-other"))
    term().resize(100, 30)
    act(() => {
      roCallbacks.forEach((cb) => cb())
      vi.advanceTimersByTime(250)
    })
    expect(pty.sendResize).not.toHaveBeenCalled()
    // Ownership comes back with the grid unchanged. The PTY never learned this
    // size, so the next size check must still send it: a send that never reached
    // the socket must not have been recorded as sent.
    act(() => notifyPtyOwner("s1", "conn-self"))
    act(() => {
      roCallbacks.forEach((cb) => cb())
      vi.advanceTimersByTime(250)
    })
    expect(pty.sendResize).toHaveBeenCalledTimes(1)
    expect(pty.sendResize).toHaveBeenCalledWith(30, 100)
  })

  it("does not record a resize the SOCKET dropped, so it is re-sent once it reopens", () => {
    // The owner gate is only one of the two ways a resize evaporates. The other
    // is the socket: `PtySocket.sendResize` silently discards the frame when the
    // WebSocket is not OPEN, which is exactly the state a reconnect passes
    // through. Recording it anyway books a size the PTY was never told, and the
    // dedupe then suppresses the re-assert forever, leaving the child drawing
    // for somebody else's viewport.
    const pty = mountSettled()
    pty.sendResize.mockReturnValue(false)
    term().resize(100, 30)
    act(() => {
      vi.advanceTimersByTime(250)
    })
    expect(pty.sendResize).toHaveBeenCalledWith(30, 100)
    // The socket comes back with the grid unchanged. The PTY never learned this
    // size, so the next size check must still send it.
    pty.sendResize.mockClear()
    pty.sendResize.mockReturnValue(true)
    act(() => {
      roCallbacks.forEach((cb) => cb())
      vi.advanceTimersByTime(250)
    })
    expect(pty.sendResize).toHaveBeenCalledTimes(1)
    expect(pty.sendResize).toHaveBeenCalledWith(30, 100)
  })

  it("still jiggles on the very first open (the deliberate first-frame bypass)", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    act(() => {
      vi.advanceTimersByTime(250)
    })
    expect(pty.sendResize).toHaveBeenNthCalledWith(1, 24, 79)
    act(() => {
      vi.advanceTimersByTime(60)
    })
    expect(pty.sendResize).toHaveBeenNthCalledWith(2, 24, 80)
  })

  it("reports the re-grid caused by the first-frame handler's OWN fit", () => {
    // `sendInitialResize` calls `fit()` itself, and that fit can re-grid for
    // exactly the reason the whole subscription exists: the bundled faces may
    // land between mount and the first PTY frame. Everything after it must read
    // the POST-fit grid, or the jiggle asserts a size the browser is no longer
    // rendering.
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    FitStub.nextDims = { rows: 30, cols: 100 }
    act(() => {
      vi.advanceTimersByTime(250)
    })
    expect(term().rows).toBe(30)
    expect(pty.sendResize).toHaveBeenNthCalledWith(1, 30, 99)
    act(() => {
      vi.advanceTimersByTime(60)
    })
    expect(pty.sendResize).toHaveBeenNthCalledWith(2, 30, 100)
    // The re-grid also scheduled a debounced size check. It must find the PTY
    // already at this size and say nothing, rather than a third frame.
    act(() => {
      vi.advanceTimersByTime(250)
    })
    expect(pty.sendResize).toHaveBeenCalledTimes(2)
  })

  it("carries a re-grid landing INSIDE the jiggle window through to the PTY", () => {
    // The nastiest window: the fonts land between the jiggle's `cols - 1` frame
    // and its `cols` tail 60ms later. The tail must send the size the terminal
    // has NOW, not the one it had when the jiggle was armed, and the debounced
    // check behind it must not then contradict the tail.
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    act(() => {
      vi.advanceTimersByTime(250)
    })
    expect(pty.sendResize).toHaveBeenNthCalledWith(1, 24, 79)
    act(() => {
      term().resize(100, 30)
    })
    act(() => {
      vi.advanceTimersByTime(60)
    })
    expect(pty.sendResize).toHaveBeenNthCalledWith(2, 30, 100)
    act(() => {
      vi.advanceTimersByTime(250)
    })
    expect(pty.sendResize).toHaveBeenCalledTimes(2)
  })

  it("still sends one plain resize on a RECONNECT's first frame, and does not jiggle", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    act(() => pty.onOpen())
    act(() => pty.bytesCb?.(new Uint8Array([1])))
    act(() => {
      vi.advanceTimersByTime(100)
    })
    pty.sendResize.mockClear()
    // The socket drops and reopens: the server replays a repaint as the first
    // frame and the pane re-asserts its size exactly once.
    act(() => pty.onOpen())
    act(() => pty.bytesCb?.(new Uint8Array([1])))
    act(() => {
      vi.advanceTimersByTime(100)
    })
    expect(pty.sendResize).toHaveBeenCalledTimes(1)
    expect(pty.sendResize).toHaveBeenCalledWith(24, 80)
  })

  it("still claims by sending this viewport's size on TAKE OVER", () => {
    const pty = mountSettled()
    act(() => pty.onConnected("conn-self"))
    act(() => notifyPtyOwner("s1", "conn-other"))
    pty.sendResize.mockClear()
    fireEvent.click(screen.getByText("Take over"))
    expect(pty.sendResize).toHaveBeenCalledTimes(1)
    expect(pty.sendResize).toHaveBeenCalledWith(24, 80)
  })

  it("still re-asserts an UNCHANGED size on a foreground return (the dedupe bypass)", () => {
    const pty = mountSettled()
    act(() => {
      window.dispatchEvent(new Event("focus"))
      vi.advanceTimersByTime(200)
    })
    expect(pty.sendResize).toHaveBeenCalledTimes(1)
    expect(pty.sendResize).toHaveBeenCalledWith(24, 80)
  })
})

// THE COMPOSE-BAR GATE. The bar used to carry its own `useIsMobile()` width
// check, so its visibility had a SECOND width opinion on top of the shell's. It
// is now gated on the three-way `ui.compose_bar` mode resolved against
// `pointer: coarse`.
//
// SCOPE: this decides the BAR, not the mobile SHELL. The pane still returns the
// desktop layout above the 768px breakpoint (that early return is layout, and
// is deliberately untouched), so these tests hold the width inside the mobile
// range and vary the pointer and the mode, which is exactly the axis the gate
// now owns. The one test that varies width does so within that range.
describe("TerminalPane compose bar gate", () => {
  const desktopWidth = window.innerWidth
  let pointerStub: MatchMediaStub | null = null

  const setWidth = (value: number) =>
    Object.defineProperty(window, "innerWidth", { value, configurable: true })

  beforeEach(() => setWidth(500))
  afterEach(() => {
    setWidth(desktopWidth)
    pointerStub?.restore()
    pointerStub = null
  })

  const withMode = (mode?: string) => {
    const state = makeState()
    ;(state.bootstrap as unknown as { compose_bar?: string }).compose_bar = mode
    mockState = state
  }

  const barIsUp = () =>
    screen.queryByRole("textbox", { name: "Message" }) !== null

  it("shows the bar on a coarse pointer and hides it on a fine one", () => {
    pointerStub = stubCoarsePointer(true)
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(barIsUp()).toBe(true)

    cleanup()
    pointerStub.restore()
    pointerStub = stubCoarsePointer(false)
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(barIsUp()).toBe(false)
  })

  // THE REGRESSION. Same device, same pointer, four widths inside the mobile
  // range. The answer must not move. Under the old gate the bar was tied to a
  // width comparison of its own; now the only thing that decides it is the
  // pointer.
  it("does NOT change when only the viewport width changes", () => {
    for (const coarse of [true, false]) {
      for (const width of [760, 640, 500, 320]) {
        pointerStub?.restore()
        pointerStub = stubCoarsePointer(coarse)
        setWidth(width)
        render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
        expect(
          barIsUp(),
          `coarse=${coarse} width=${width}: the bar must follow the pointer, not the width`
        ).toBe(coarse)
        cleanup()
      }
    }
  })

  // A touchscreen laptop at a narrow window used to get the bar purely because
  // the window was narrow. It is a FINE pointer, so it should not.
  it("does not show the bar to a fine pointer at a phone-sized window", () => {
    pointerStub = stubCoarsePointer(false)
    setWidth(400)
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(barIsUp()).toBe(false)
  })

  // A tablet with a keyboard case and one without are INDISTINGUISHABLE to the
  // browser (measured), so the user must be able to override the capability
  // answer in both directions. That is the entire reason this is not a bool.
  it("'always' shows the bar even on a fine pointer", () => {
    pointerStub = stubCoarsePointer(false)
    withMode("always")
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(barIsUp()).toBe(true)
  })

  it("'never' hides the bar even on a coarse pointer", () => {
    pointerStub = stubCoarsePointer(true)
    withMode("never")
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(barIsUp()).toBe(false)
  })

  it("'auto' defers to the pointer in both directions", () => {
    pointerStub = stubCoarsePointer(true)
    withMode("auto")
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(barIsUp()).toBe(true)

    cleanup()
    pointerStub.restore()
    pointerStub = stubCoarsePointer(false)
    withMode("auto")
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(barIsUp()).toBe(false)
  })

  // An older server omits the field entirely; it must behave as "auto" rather
  // than as "off".
  it("treats an absent mode as auto", () => {
    pointerStub = stubCoarsePointer(true)
    withMode(undefined)
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(barIsUp()).toBe(true)
  })
})

// WIDTH DECIDES THE LAYOUT; THE POINTER DECIDES THE TYPING SURFACE. The two
// are orthogonal, and treating them as one question was the bug: a tablet in
// landscape gets the DESKTOP shell because it has the room, and still needs the
// buffered input because a finger is still doing the typing. So both touch
// surfaces (the compose bar and the accessory keys) travel with the pointer,
// including inside the desktop shell, which was impossible before.
describe("TerminalPane typing surfaces follow the pointer, not the layout", () => {
  const desktopWidth = 1200
  let pointerStub: MatchMediaStub | null = null

  const setWidth = (value: number) =>
    Object.defineProperty(window, "innerWidth", { value, configurable: true })

  beforeEach(() => setWidth(desktopWidth))
  afterEach(() => {
    setWidth(desktopWidth)
    pointerStub?.restore()
    pointerStub = null
  })

  const composeUp = () =>
    screen.queryByRole("textbox", { name: "Message" }) !== null
  const accessoryUp = () =>
    screen.queryByRole("button", { name: "Esc" }) !== null

  it("renders BOTH bars in the desktop shell on a coarse pointer", () => {
    pointerStub = stubCoarsePointer(true)
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(composeUp()).toBe(true)
    expect(accessoryUp()).toBe(true)
  })

  it("renders neither in the desktop shell on a fine pointer", () => {
    pointerStub = stubCoarsePointer(false)
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(composeUp()).toBe(false)
    expect(accessoryUp()).toBe(false)
  })

  // The input-owner gate is unchanged: a viewer gets the take-over card and no
  // surface that could even stage input.
  it("renders neither for a viewer, coarse pointer or not", () => {
    pointerStub = stubCoarsePointer(true)
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    act(() => notifyPtyOwner("s1", "conn-other"))
    expect(composeUp()).toBe(false)
    expect(accessoryUp()).toBe(false)
  })

  it("still honours 'always' and 'never' in the desktop shell", () => {
    const withMode = (mode: string) => {
      const state = makeState()
      ;(state.bootstrap as unknown as { compose_bar?: string }).compose_bar =
        mode
      mockState = state
    }

    pointerStub = stubCoarsePointer(false)
    withMode("always")
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(composeUp()).toBe(true)

    cleanup()
    pointerStub.restore()
    pointerStub = stubCoarsePointer(true)
    withMode("never")
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(composeUp()).toBe(false)
    // The accessory keys stay: a phone still cannot produce Esc or a Ctrl
    // chord, whatever the compose box is doing.
    expect(accessoryUp()).toBe(true)
  })
})

// THE CARET STAYS SOLID WHILE THE COMPOSE BAR HOLDS FOCUS. xterm is never
// focused in that mode by design, so its unfocused caret is the only one the
// user ever sees, and the conventional hollow outline says "asleep" about a
// live prompt. The option follows the state, because the Box/Direct toggle
// flips it mid-session.
describe("TerminalPane inactive cursor style", () => {
  let pointerStub: MatchMediaStub | null = null

  beforeEach(async () => {
    const mod = await import("@/lib/typingSurface")
    mod.setTypingSurface(null)
  })
  afterEach(() => {
    pointerStub?.restore()
    pointerStub = null
  })

  it("opens solid when the compose bar is the typing surface", async () => {
    pointerStub = stubCoarsePointer(true)
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    await act(async () => {})
    expect(TermStub.instances.at(-1)!.options.cursorInactiveStyle).toBe("block")
  })

  it("opens with the conventional outline when it is not", async () => {
    pointerStub = stubCoarsePointer(false)
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    await act(async () => {})
    expect(TermStub.instances.at(-1)!.options.cursorInactiveStyle).toBe(
      "outline",
    )
  })

  it("follows the typing-surface toggle on the SAME terminal", async () => {
    pointerStub = stubCoarsePointer(true)
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    await act(async () => {})
    const term = TermStub.instances.at(-1)!
    expect(term.options.cursorInactiveStyle).toBe("block")

    // Direct typing: xterm gets focus again, so the convention comes back.
    fireEvent.pointerDown(
      screen.getByRole("button", { name: /^Typing surface:/ }),
    )
    expect(TermStub.instances.at(-1)).toBe(term)
    expect(term.options.cursorInactiveStyle).toBe("outline")

    fireEvent.pointerDown(
      screen.getByRole("button", { name: /^Typing surface:/ }),
    )
    expect(term.options.cursorInactiveStyle).toBe("block")
  })
})

// THE COMPOSE BOX SAYS WHAT IT IS FOR. An agent pane is a conversation, not a
// shell: prompting for a command there described the wrong activity. Every
// non-agent PTY surface (companion, project and standalone terminals alike) is
// a shell, so it keeps the command wording.
describe("TerminalPane compose placeholder follows the surface", () => {
  let pointerStub: MatchMediaStub | null = null

  beforeEach(() => {
    pointerStub = stubCoarsePointer(true)
  })
  afterEach(() => {
    pointerStub?.restore()
    pointerStub = null
  })

  const placeholder = () =>
    (
      screen.getByRole("textbox", { name: "Message" }) as HTMLTextAreaElement
    ).getAttribute("placeholder")

  it("asks an agent pane for a message", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(placeholder()).toBe("Write a message to the agent…")
  })

  it("asks a terminal pane for a command", () => {
    render(
      <TerminalPane
        kind="terminal"
        id="t1"
        owner={{ kind: "session", sessionId: "s1" }}
      />,
    )
    expect(placeholder()).toBe("Type a command…")
  })

  it("asks a standalone terminal for a command too", () => {
    render(
      <TerminalPane kind="terminal" id="t2" owner={{ kind: "standalone" }} />,
    )
    expect(placeholder()).toBe("Type a command…")
  })
})

// THE WAY BACK TRAVELS WITH THE KEYS. `ui.mobile_accessory_bar` is a
// SERVER-SIDE preference, so hiding it from a phone hides the keys on the
// tablet too; if the restore affordance stayed gated on the mobile LAYOUT, that
// tablet got the desktop shell with no keys and no way to ask for them back.
// The affordance therefore follows the same predicate that mounts the bars.
describe("TerminalPane restore affordance follows the touch surfaces", () => {
  let pointerStub: MatchMediaStub | null = null

  const setWidth = (value: number) =>
    Object.defineProperty(window, "innerWidth", { value, configurable: true })

  beforeEach(() => setWidth(1200))
  afterEach(() => {
    setWidth(1200)
    pointerStub?.restore()
    pointerStub = null
  })

  const hideAccessoryBar = () => {
    const state = makeState()
    ;(
      state.bootstrap as unknown as { mobile_accessory_bar?: boolean }
    ).mobile_accessory_bar = false
    return state
  }
  const restoreButton = () =>
    screen.queryByRole("button", { name: "Show hidden bars" })

  it("offers the restore button in the DESKTOP shell on a coarse pointer", () => {
    pointerStub = stubCoarsePointer(true)
    mockState = hideAccessoryBar()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    // The dead end: keys gone, desktop layout, and before this the button too.
    expect(screen.queryByRole("button", { name: "Esc" })).toBeNull()
    expect(restoreButton()).toBeTruthy()
  })

  it("one tap brings the keys back", () => {
    pointerStub = stubCoarsePointer(true)
    mockState = hideAccessoryBar()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    fireEvent.click(restoreButton()!)
    const fetchSpy = globalThis.fetch as unknown as ReturnType<typeof vi.fn>
    expect(fetchSpy).toHaveBeenCalledWith(
      "/api/v1/config/settings",
      expect.objectContaining({
        method: "PATCH",
        body: JSON.stringify({
          ui: { mobile_top_bar: true, mobile_accessory_bar: true },
          quiet: true,
        }),
      }),
    )
  })

  // With the compose bar off too there is no bar left to carry the button, so
  // the pane's own minimal row must carry it in the desktop shell as well.
  it("falls back to the minimal row when the compose bar is off too", () => {
    pointerStub = stubCoarsePointer(true)
    const state = hideAccessoryBar()
    ;(state.bootstrap as unknown as { compose_bar?: string }).compose_bar =
      "never"
    mockState = state
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(screen.queryByRole("textbox", { name: "Message" })).toBeNull()
    expect(restoreButton()).toBeTruthy()
  })

  // A fine-pointer desktop never had the keys in the first place, so there is
  // nothing hidden from it and nothing to restore.
  it("offers nothing on a fine pointer", () => {
    pointerStub = stubCoarsePointer(false)
    mockState = hideAccessoryBar()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(restoreButton()).toBeNull()
  })

  // The TOP bar is the mobile shell's own chrome. The desktop shell never
  // renders it, so its preference being off hides nothing here and must not
  // put an unexplained button under a desktop terminal.
  it("ignores the top-bar preference in the desktop shell", () => {
    pointerStub = stubCoarsePointer(true)
    const state = makeState()
    ;(
      state.bootstrap as unknown as { mobile_top_bar?: boolean }
    ).mobile_top_bar = false
    mockState = state
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(screen.getByRole("button", { name: "Esc" })).toBeTruthy()
    expect(restoreButton()).toBeNull()
  })
})

// THE TOGGLE. A tablet with a keyboard case attached wants direct typing; the
// same tablet without it wants the buffered box, and the browser cannot tell
// the two apart (measured). So the user swaps it, transiently, per device.
describe("TerminalPane typing-surface toggle", () => {
  let pointerStub: MatchMediaStub | null = null

  beforeEach(async () => {
    pointerStub = stubCoarsePointer(true)
    const mod = await import("@/lib/typingSurface")
    mod.setTypingSurface(null)
  })
  afterEach(() => {
    pointerStub?.restore()
    pointerStub = null
  })

  const toggle = () => screen.getByRole("button", { name: /^Typing surface:/ })
  const composeUp = () =>
    screen.queryByRole("textbox", { name: "Message" }) !== null

  it("flips the surface and says which state it is in", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(composeUp()).toBe(true)
    // It names its state rather than being an unlabelled icon.
    expect(toggle().textContent).toBeTruthy()
    const before = toggle().textContent

    fireEvent.pointerDown(toggle())
    expect(composeUp()).toBe(false)
    expect(toggle().textContent).not.toBe(before)

    fireEvent.pointerDown(toggle())
    expect(composeUp()).toBe(true)
  })

  // It lives in the ACCESSORY bar because that bar is present in BOTH states.
  // Inside the compose bar it would vanish with the surface it turns off.
  it("is reachable in both states", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(toggle()).toBeTruthy()
    fireEvent.pointerDown(toggle())
    expect(composeUp()).toBe(false)
    expect(toggle()).toBeTruthy()
  })

  it("writes the choice to localStorage, so a reload does not snap back", async () => {
    const mod = await import("@/lib/typingSurface")
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    fireEvent.pointerDown(toggle())
    expect(composeUp()).toBe(false)
    // THE STORAGE, not the module's own memory: a live module variable would
    // satisfy a remount in this file and still lose the choice on a real
    // reload. (What a genuinely fresh page does with that key is pinned in
    // `lib/typingSurface.test.ts`, which re-evaluates the module.)
    expect(localStorage.getItem(mod.TYPING_SURFACE_KEY)).toBe("direct")
    cleanup()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(composeUp()).toBe(false)
  })

  it("does not write the ui.compose_bar setting", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const calls = () =>
      (fetch as unknown as { mock: { calls: unknown[][] } }).mock.calls
    const before = calls().length
    fireEvent.pointerDown(toggle())
    for (const call of calls().slice(before)) {
      expect(String(call[0])).not.toContain("/settings")
    }
  })

  // Under always/never the SETTING decides, so a control that changed nothing
  // must not be there at all.
  it("is absent when the setting has already decided", () => {
    const state = makeState()
    ;(state.bootstrap as unknown as { compose_bar?: string }).compose_bar =
      "always"
    mockState = state
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(screen.queryByRole("button", { name: /^Typing surface:/ })).toBeNull()
  })
})

// Long-press text selection. A browser synthesizes mouse events for a TAP and
// for nothing else, so xterm's own selection service never sees a touch drag;
// the pane drives xterm's selection model itself through the pure helpers in
// `lib/termselect.ts`. These tests are about the WIRING: that the long press
// picks a word, that the drag re-selects the normalized span, that the lift
// copies, and that none of the other touch branches got broken doing it.
//
// Geometry comes from `TermStub.mouseGeometry`: the `.xterm-screen` rect starts
// at (100, 50) with 10x20 cells and an 80x24 grid, and the stub buffer's first
// line is "git status --porcelain".
describe("TerminalPane long-press text selection", () => {
  const LONG_PRESS_MS = 400
  const container = () => screen.getByTestId("terminal-container")
  const term = () => {
    const t = TermStub.instances.at(-1)
    if (!t) throw new Error("no term constructed")
    return t
  }
  // The centre of the ZERO-based cell (col, row).
  const at = (col: number, row: number) => ({
    clientX: 100 + col * 10 + 5,
    clientY: 50 + row * 20 + 10,
  })
  const press = (point: { clientX: number; clientY: number }) => {
    fireEvent.touchStart(container(), { touches: [point] })
    act(() => {
      vi.advanceTimersByTime(LONG_PRESS_MS)
    })
  }
  const move = (point: { clientX: number; clientY: number }) =>
    fireEvent.touchMove(container(), { touches: [point] })
  const lift = (point: { clientX: number; clientY: number }) =>
    fireEvent.touchEnd(container(), { touches: [], changedTouches: [point] })

  beforeEach(() => {
    vi.useFakeTimers()
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  it("cancels the lift so no compatibility mouse events follow it", () => {
    // A bare press-and-lift, with no drag to have cancelled anything for us.
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    press(at(6, 0))
    expect(lift(at(6, 0))).toBe(false)
  })

  it("selects the word under the finger on a long press", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    press(at(6, 0))
    expect(term().getSelection()).toBe("status")
  })

  it("selects nothing where there is nothing, rather than the nearest word", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    // Column 60 of the first line is past the end of the text: blank cells.
    press(at(60, 0))
    expect(term().getSelection().trim()).toBe("")
  })

  it("extends the selection as the finger drags forward", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    press(at(6, 0))
    move(at(13, 0))
    expect(term().getSelection()).toBe("status --p")
  })

  it("normalizes a backwards drag instead of collapsing the selection", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    press(at(6, 0))
    move(at(0, 0))
    expect(term().getSelection()).toBe("git status")
  })

  it("carries the selection onto the next row", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    press(at(4, 0))
    move(at(5, 1))
    expect(term().getSelection()).toBe("status --porcelain\nsecond")
  })

  // Auto-scroll is TIMER driven, not event driven. A finger parked at the edge
  // produces no further touchmove, so an event-driven version simply stopped
  // and the user had to jiggle to keep extending. xterm's own mouse drag scroll
  // is a 50ms interval for the same reason.
  const SCROLL_TICK_MS = 50
  const tick = (times: number) =>
    act(() => {
      vi.advanceTimersByTime(SCROLL_TICK_MS * times)
    })

  it("keeps auto-scrolling while the finger is parked past the bottom edge", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    press(at(6, 0))
    // ONE move to the edge, then no further events at all.
    move({ clientX: at(6, 0).clientX, clientY: 50 + 480 + 30 })
    tick(3)
    expect(term().scrollLineCalls).toEqual([1, 1, 1])
    expect(term().buffer.active.viewportY).toBe(3)
  })

  it("auto-scrolls the other way above the top edge", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    // Scrolled back five rows, so the press lands on a real line and there is
    // somewhere above the viewport to scroll to.
    term().lines = ["one two", "b", "c", "d", "e", "git status here", "g"]
    term().buffer.active.viewportY = 5
    press(at(6, 0))
    move({ clientX: at(6, 0).clientX, clientY: 50 - 30 })
    tick(2)
    expect(term().scrollLineCalls).toEqual([-1, -1])
  })

  it("stops auto-scrolling when the finger comes back inside", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    press(at(6, 0))
    move({ clientX: at(6, 0).clientX, clientY: 50 + 480 + 30 })
    tick(2)
    move(at(6, 3))
    tick(5)
    expect(term().scrollLineCalls).toEqual([1, 1])
  })

  it("stops auto-scrolling the moment the finger lifts", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    press(at(6, 0))
    move({ clientX: at(6, 0).clientX, clientY: 50 + 480 + 30 })
    tick(1)
    lift({ clientX: at(6, 0).clientX, clientY: 50 + 480 + 30 })
    tick(10)
    expect(term().scrollLineCalls).toEqual([1])
  })

  // Nothing else pins the viewport-to-absolute-row conversion: the pure helpers
  // never see `viewportY`, so dropping it left every other test green. A
  // SCROLLED-BACK viewport is what makes the two answers differ in the text.
  it("reads the word from the SCROLLED-BACK row, not from the top of the buffer", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    term().lines = ["alpha", "bravo", "charlie", "delta"]
    // The user has scrolled back, so viewport row 0 is buffer row 2.
    term().buffer.active.viewportY = 2
    press(at(0, 0))
    expect(term().getSelection()).toBe("charlie")
  })

  it("follows the viewport as the auto-scroll moves it under the finger", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    term().lines = ["alpha", "bravo", "charlie", "delta"]
    term().buffer.active.viewportY = 0
    press(at(0, 0))
    expect(term().getSelection()).toBe("alpha")
    move({ clientX: at(2, 0).clientX, clientY: 50 + 480 + 30 })
    tick(2)
    expect(term().buffer.active.viewportY).toBe(2)
    expect(term().getSelection()).toContain("bravo")
  })

  it("does not scroll the scrollback while a selection drag is in flight", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    press(at(6, 0))
    // A long vertical drag that stays inside the screen: it must extend the
    // selection and move the viewport not at all.
    move(at(6, 10))
    expect(term().scrollLineCalls).toEqual([])
    expect(term().getSelection()).not.toBe("")
  })

  it("leaves no selection behind when the gesture was a scroll", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    fireEvent.touchStart(container(), { touches: [at(6, 0)] })
    // Move past the 8px threshold BEFORE the long press could fire.
    fireEvent.touchMove(container(), { touches: [at(6, 5)] })
    act(() => {
      vi.advanceTimersByTime(LONG_PRESS_MS * 2)
    })
    expect(term().getSelection()).toBe("")
    expect(term().scrollLineCalls.length).toBeGreaterThan(0)
  })

  it("a second finger cancels the pending long press", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    fireEvent.touchStart(container(), { touches: [at(6, 0)] })
    fireEvent.touchStart(container(), { touches: [at(6, 0), at(10, 0)] })
    act(() => {
      vi.advanceTimersByTime(LONG_PRESS_MS * 2)
    })
    expect(term().getSelection()).toBe("")
  })

  it("a second finger during an ACTIVE selection cancels the gesture", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    press(at(6, 0))
    expect(term().getSelection()).toBe("status")
    // A pinch begins. Lifting one finger out of it must not be read as the end
    // of a selection gesture: no copy, and no further extension.
    fireEvent.touchStart(container(), { touches: [at(6, 0), at(20, 0)] })
    move(at(20, 0))
    lift(at(20, 0))
    expect(copied).toEqual([])
    expect(term().getSelection()).toBe("status")
  })

  it("clears the selection on the NEXT tap", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    press(at(6, 0))
    lift(at(6, 0))
    expect(term().getSelection()).toBe("status")
    fireEvent.touchStart(container(), { touches: [at(20, 0)] })
    lift(at(20, 0))
    expect(term().getSelection()).toBe("")
  })

  it("copies on lift and leaves the selection painted", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    press(at(6, 0))
    lift(at(6, 0))
    expect(copied).toEqual(["status"])
    expect(term().getSelection()).toBe("status")
  })

  it("copies a ONE-character word, because a long press is deliberate", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    term().lines = ["a b c"]
    press(at(2, 0))
    lift(at(2, 0))
    expect(term().getSelection()).toBe("b")
    expect(copied).toEqual(["b"])
  })

  it("copies nothing on lift when copy-on-select is off", () => {
    const state = makeState()
    ;(state.bootstrap as unknown as { copy_on_select?: boolean }).copy_on_select =
      false
    mockState = state
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    press(at(6, 0))
    lift(at(6, 0))
    expect(copied).toEqual([])
    expect(term().getSelection()).toBe("status")
  })

  // The touch equivalent of the desktop force-local-selection modifier (Shift
  // on Linux/Windows, Option on macOS). Claude Code and opencode both enable
  // mouse tracking, so a long press that forwarded instead of selecting would
  // leave every real agent pane unselectable by finger.
  it("selects locally over a mouse-tracking app and forwards nothing", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const t = term()
    t.modes.mouseTrackingMode = "vt200"
    t.mouse?.setProtocol("VT200")
    t.mouse?.setEncoding("SGR")
    const pty = FakePtySocket.instances.at(-1)
    if (!pty) throw new Error("no pty constructed")
    press(at(6, 0))
    move(at(13, 0))
    lift(at(13, 0))
    expect(t.getSelection()).toBe("status --p")
    expect(pty.sendInput.mock.calls).toEqual([])
  })

  // The mouse-capture hint says "hold Shift and drag to select". It is advice
  // for a MOUSE, and it is nonsense here: the long press already selected
  // locally with no modifier at all. `copyOnSelectAction` still answers "hint"
  // on this input (a blank selection, the app holding the mouse, a drag), so
  // the pane has to be the thing that ignores it. Pressing on BLANK space is
  // what reaches that answer; with real text the copy branch wins first, which
  // is why asserting it there proved nothing.
  it("never shows the mouse-capture hint, whatever the long press lands on", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const t = term()
    // Real SPACES, not the unwritten cells past the end of a line: the hint
    // branch needs a selection that is non-empty and yet blank, and unwritten
    // cells carry no characters at all so they never reach it. (The pane's
    // `mouseCaptureHintShown` latch is module-global and never reset; nothing
    // else in this file trips it, so it is still false here.)
    t.lines = ["a    b"]
    t.modes.mouseTrackingMode = "vt200"
    press(at(2, 0))
    move(at(3, 0))
    lift(at(3, 0))
    expect(t.getSelection()).toBe("    ")
    expect(t.getSelection().trim()).toBe("")
    expect(toastCalls).toEqual([])
    expect(copied).toEqual([])
  })

  it("hands focus back when xterm's contextmenu handler grabs the textarea", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    // xterm's handler focuses its hidden textarea to prepare a native Copy,
    // which on a phone raises the soft keyboard over the selection.
    term().textarea.focus()
    fireEvent.pointerDown(container(), { pointerType: "touch" })
    fireEvent.contextMenu(container())
    expect(term().textarea.focused).toBe(false)
  })

  it("leaves the textarea focused for a MOUSE right-click, which pastes", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    term().textarea.focus()
    fireEvent.pointerDown(container(), { pointerType: "mouse" })
    fireEvent.contextMenu(container())
    expect(term().textarea.value).toBe("")
    expect(term().textarea.focused).toBe(true)
  })

  // The focus cell must be resolved to the GLYPH that owns it before any
  // arithmetic. On the right half of a wide glyph the raw column is the
  // continuation cell, so a BACKWARDS drag ending there started the span inside
  // the glyph: the glyph was dropped and a blank appeared at the front.
  it("starts a backwards drag at the wide glyph, not inside it", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    term().lines = ["ok 日本語 x"]
    // Columns: o k _ 日 日 本 本 語 語 _ x
    press(at(10, 0))
    expect(term().getSelection()).toBe("x")
    move(at(4, 0))
    expect(term().getSelection()).toBe("日本語 x")
  })

  // A long press is not a tap, so the compose-bar focus redirect must not run:
  // raising the soft keyboard over the text the user is selecting takes half
  // the screen away mid-gesture.
  it("does not raise the soft keyboard", () => {
    Object.defineProperty(window, "innerWidth", { value: 500, configurable: true })
    const pointer = stubCoarsePointer()
    try {
      render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
      const compose = screen.getByRole("textbox", { name: "Message" })
      // The pane focuses the compose box on mount (so the keyboard types into
      // the buffer from the first moment). Let it go first, or this asserts
      // nothing: the question is whether the LONG PRESS pulls focus back.
      ;(compose as HTMLTextAreaElement).blur()
      expect(document.activeElement).not.toBe(compose)
      press(at(6, 0))
      const prevented = !lift(at(6, 0))
      expect(term().getSelection()).toBe("status")
      expect(document.activeElement).not.toBe(compose)
      // And it CANCELS the touchend. That is not incidental: the browser's
      // compatibility mouse events are dispatched after an uncancelled
      // touchend, xterm focuses its hidden textarea from that mousedown (so the
      // keyboard rises over the text just selected) and xterm's own
      // `_handleSingleClick` then wipes the highlight the copy was for. Over a
      // mouse-tracking app the same mousedown is forwarded into the TUI as a
      // stray click.
      expect(prevented).toBe(true)
    } finally {
      pointer.restore()
    }
  })

  it("abandons the gesture when the app flips buffers mid-drag", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    press(at(6, 0))
    // The app entered its alt screen. The anchor names a NORMAL-buffer row;
    // applying it to the alt buffer would select unrelated content.
    term().buffer.active.type = "alternate"
    move(at(20, 0))
    expect(term().getSelection()).toBe("status")
  })

  it("wipes the selection xterm stuffed into its hidden textarea on a touch long press", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    // Android fires `contextmenu` on a long press. xterm's OWN listener sits on
    // `term.element`, inside this container, so it runs first and writes the
    // selection into the hidden textarea, where it would later be delivered to
    // the PTY as a paste.
    term().textarea.value = "status"
    fireEvent.pointerDown(container(), { pointerType: "touch" })
    fireEvent.contextMenu(container())
    expect(term().textarea.value).toBe("")
  })

  it("suppresses the platform callout and context menu on the terminal", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    // iOS raises its own magnifier and share menu over a long press unless the
    // element opts out; Android fires `contextmenu`.
    expect(container().className).toContain("[-webkit-touch-callout:none]")
    fireEvent.pointerDown(container(), { pointerType: "touch" })
    const menu = fireEvent.contextMenu(container())
    expect(menu).toBe(false)
  })
})
