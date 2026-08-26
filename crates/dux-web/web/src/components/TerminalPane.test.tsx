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
import { VIEWER_MIN_FONT_SIZE } from "@/lib/viewerFit"
import { replayWaitMs } from "@/lib/connectionTiming"
import { REPLAY_WAIT_POLL_MS } from "@/components/terminal/constants"
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
  // Counted, because "take-over is a fresh attach" means precisely that the
  // bounce runs the reset-then-replay path: the reset is what discards this
  // viewer's polluted scrollback, and nothing else in the pane clears it.
  resets = 0
  reset() {
    this.resets++
  }
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
  // Mirrors the real socket's `isOpen` getter; a test flips it to false to
  // model a disconnected socket (the compose bar's Send checks it).
  isOpen = true
  // The handshake now carries the pty's current OWNER as well as this socket's
  // id, and the pane seeds its ownership verdict from it. Tests that pass no
  // owner exercise the older-server fallback (the key absent, so the foreground
  // guess still decides), which is what most of this file wants.
  onConnected: (
    id: string,
    owner?: string | null,
    ownerEpoch?: number,
    ownerDevice?: string,
  ) => void = () => {}
  // The PTY's authoritative grid, reported by the `connected` handshake at
  // attach (`fromHandshake` true) and by a `size` event on every applied resize
  // after it. A test drives it directly, which is exactly what the server does.
  onPtyGrid: (
    grid: { rows: number; cols: number } | null,
    fromHandshake: boolean,
  ) => void = () => {}
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
  // The generation the server stamps on each open's scrollback replay. A test
  // that drives a reconnect bumps it, exactly as a real server would, so the
  // pane's already-applied guard does not drop the second replay as a
  // duplicate.
  replayGeneration: number | null = null
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
// Only the parser-handler installer is stubbed out (it needs a real xterm
// parser); `isFocusReport` is a pure helper the pane's onData gate calls, so it
// comes from the real module.
vi.mock("@/lib/suppressViewerReports", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("@/lib/suppressViewerReports")>()
  return { ...actual, suppressViewerReports: () => {} }
})
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

// Stub browser globals BEFORE the store module evaluates (it touches
// localStorage at import time, pulled in transitively by TerminalPane).
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

function makeState(offline = false, conn: ConnState = "open"): DuxState {
  return {
    conn,
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
  resetComposeDrafts()
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

// THE PHANTOM OWNER, and the frame that prevents it.
//
// A plain claim is refused SILENTLY server-side now, so a foregrounded pane
// arriving at a pty another device drives gets no `pty.owner`, no error, and no
// correction of any kind. Left to its foreground guess it would render typing
// surfaces over a pty whose every keystroke the server drops, with no card ever
// explaining why. The `connected` handshake's `owner` field is the correction.
describe("TerminalPane seeds its ownership verdict from the connected frame", () => {
  it("shows the take-over card to a FOREGROUNDED pane that joined a driven pty", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    // The pre-handshake guess: foregrounded, so optimistically the owner.
    expect(screen.queryByText("Active on another device")).toBeNull()
    act(() => last().onConnected("conn-self", "conn-other"))
    expect(screen.getByText("Active on another device")).toBeTruthy()
  })

  it("leaves a foregrounded pane driving an UNOWNED pty", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    act(() => last().onConnected("conn-self", null))
    expect(screen.queryByText("Active on another device")).toBeNull()
  })

  // The owner's browser tab closed. Nothing else would ever say so, because
  // ownership stopped following focus, and the card would name a device that is
  // gone. This pane is BACKGROUNDED, so it watches rather than auto-claiming.
  it("offers control once the owner disconnects, to a backgrounded viewer", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    act(() => last().onConnected("conn-self", "conn-other"))
    expect(screen.getByText("Active on another device")).toBeTruthy()
    Object.defineProperty(document, "visibilityState", {
      value: "hidden",
      configurable: true,
    })
    try {
      act(() => notifyPtyOwner("s1", undefined, 7))
      expect(screen.getByText("Take control")).toBeTruthy()
      expect(screen.queryByText("Active on another device")).toBeNull()
    } finally {
      Object.defineProperty(document, "visibilityState", {
        value: "visible",
        configurable: true,
      })
    }
  })

  // LOSING OWNERSHIP IS STICKY. Foregrounded and mounted changes nothing: the
  // same broadcast only re-titles the card, because sitting on an open card is
  // not a gesture. The passive claim that used to live here is exactly what let
  // an idle desktop beat the blipped owner back to its own pty.
  it("does not claim the freed pty, even foregrounded and mounted", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    act(() => pty.onConnected("conn-self", "conn-other"))
    expect(screen.getByText("Active on another device")).toBeTruthy()
    pty.sendResize.mockClear()
    act(() => notifyPtyOwner("s1", undefined, 7))
    expect(screen.queryByText("Active on another device")).toBeNull()
    expect(screen.getByText("Take control")).toBeTruthy()
    expect(pty.sendResize).not.toHaveBeenCalled()
  })

  // THE HALF-OPEN ORDERING, end to end through the pane. The owner's socket
  // half-opens, the client is back in about a second with a fresh id, and the
  // server has not reaped the old one, so the handshake names this pane's own
  // ghost. It takes its pty back with a FLAGGED claim on the first resize of
  // the new connection.
  it("self-succeeds when the reconnect's handshake names its own dead connection", () => {
    vi.useFakeTimers()
    try {
      render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
      const pty = last()
      act(() => pty.onOpen())
      act(() => pty.onConnected("conn-a", null))
      act(() => {
        vi.advanceTimersByTime(400)
      })
      expect(screen.queryByText("Active on another device")).toBeNull()

      // The blip: the socket drops and comes back with a new id while the
      // server still records conn-a as the driver.
      act(() => pty.onReconnecting())
      pty.sendResize.mockClear()
      act(() => {
        pty.onOpen()
        pty.onConnected("conn-b", "conn-a")
      })
      // No card: the returning driver is the driver again.
      expect(screen.queryByText("Active on another device")).toBeNull()
      act(() => pty.bytesCb?.(new Uint8Array([0x61])))
      act(() => {
        vi.advanceTimersByTime(400)
      })
      // And the claim rode the first resize of the new connection, FLAGGED and
      // NAMING THE GHOST it expects to displace: the server refuses the transfer
      // if anybody else holds the pty by then.
      expect(pty.sendResize).toHaveBeenCalledWith(24, 80, true, "conn-a")
    } finally {
      vi.useRealTimers()
    }
  })
})

// ONE PTY HAS ONE AUTHORITATIVE GRID, the owner's, and every other attached
// browser renders the same byte stream into its own, differently sized xterm.
// A viewer in that state is looking at wrapped and clamped output, and every
// child repaint scrolls mangled rows into its LOCAL scrollback, which nothing
// but a fresh attach ever cleans up. Two answers, pinned here: the pane ADOPTS
// the PTY's grid so the divergence never happens, and it HEALS the scrollback
// it already recorded BY RE-ATTACHING (never by resizing the PTY, which is the
// silent steal this whole arc exists to kill).
//
// The stub terminal's grid is 24x80, so a reported grid of anything else is a
// diverged viewer.
describe("TerminalPane viewer grid divergence", () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  /// Mount, settle the deferred first-frame resize, and make this pane a
  /// WATCHER by answering the handshake with somebody else's connection id.
  const mountWatcher = () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    act(() => pty.onOpen())
    act(() => {
      vi.advanceTimersByTime(400)
    })
    act(() => pty.onConnected("conn-self", "conn-other"))
    pty.connect.mockClear()
    return pty
  }

  it("adopts the PTY's grid rather than diverging from it", () => {
    // The structural fix, pinned from the outside: the handshake's grid is
    // what this watcher re-grids to, so there is no divergence to manage.
    const pty = mountWatcher()
    const term = TermStub.instances.at(-1)!
    act(() => pty.onPtyGrid({ rows: 40, cols: 120 }, true))
    expect({ rows: term.rows, cols: term.cols }).toEqual({ rows: 40, cols: 120 })
  })

  it("adopts a grid CHANGE too, not just the handshake's", () => {
    const pty = mountWatcher()
    const term = TermStub.instances.at(-1)!
    act(() => pty.onPtyGrid({ rows: 40, cols: 120 }, true))
    act(() => pty.onPtyGrid({ rows: 50, cols: 132 }, false))
    expect({ rows: term.rows, cols: term.cols }).toEqual({ rows: 50, cols: 132 })
  })

  it("never adopts anything for the OWNER, whose container defines the grid", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    act(() => pty.onOpen())
    act(() => {
      vi.advanceTimersByTime(400)
    })
    // An UNOWNED pty, so this foregrounded pane is the driver.
    act(() => pty.onConnected("conn-self", null))
    const term = TermStub.instances.at(-1)!
    act(() => pty.onPtyGrid({ rows: 40, cols: 120 }, false))
    expect({ rows: term.rows, cols: term.cols }).toEqual({
      rows: 24,
      cols: 80,
    })
  })

  it("adopts on DEMOTION, without waiting for the next size event", () => {
    // The owner-defined grid is recorded in both modes precisely so a handover
    // has something to adopt at once.
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    act(() => pty.onOpen())
    act(() => {
      vi.advanceTimersByTime(400)
    })
    act(() => pty.onConnected("conn-self", null))
    const term = TermStub.instances.at(-1)!
    act(() => pty.onPtyGrid({ rows: 40, cols: 120 }, false))
    expect(term.cols).toBe(80)
    act(() => notifyPtyOwner("s1", "conn-other"))
    expect({ rows: term.rows, cols: term.cols }).toEqual({ rows: 40, cols: 120 })
  })

  it("bounces the socket ONCE after a burst of grid changes settles", () => {
    const pty = mountWatcher()
    act(() => pty.onPtyGrid({ rows: 40, cols: 120 }, true))
    // A divider drag on the owner's desktop: several applied grids in quick
    // succession. Each re-arms the debounce; none of them may reconnect a
    // watching phone on its own.
    act(() => {
      pty.onPtyGrid({ rows: 41, cols: 121 }, false)
      vi.advanceTimersByTime(100)
      pty.onPtyGrid({ rows: 42, cols: 122 }, false)
      vi.advanceTimersByTime(100)
      pty.onPtyGrid({ rows: 43, cols: 123 }, false)
    })
    expect(pty.connect).not.toHaveBeenCalled()
    act(() => {
      vi.advanceTimersByTime(600)
    })
    expect(pty.connect).toHaveBeenCalledTimes(1)
    // The heal is a reconnect, and the watcher's pane stays covered throughout
    // by the take-over card, which is the cover a non-owner gets in every
    // ordinary state (it is solid and full-pane, so it always painted over the
    // reconnect spinner anyway).
    expect(screen.getByText("Take over")).toBeTruthy()
    // Each announcement was ADOPTED before it armed the heal, so by the time
    // the bounce fires this pane is already rendering at the child's geometry.
    // The bounce is still worth taking, because adopting the grid does not
    // clean the scrollback the pre-adoption view recorded; only a fresh attach
    // does.
  })

  it("never bounces on the handshake's OWN grid", () => {
    // A fresh attach has just rebuilt its buffer from the server's repaint.
    // Bouncing on the grid that attach reported would loop forever.
    const pty = mountWatcher()
    act(() => pty.onPtyGrid({ rows: 40, cols: 120 }, true))
    act(() => {
      vi.advanceTimersByTime(1000)
    })
    expect(pty.connect).not.toHaveBeenCalled()
  })

  it("never bounces the OWNER, whose own resize is echoed back to it", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    act(() => pty.onOpen())
    act(() => {
      vi.advanceTimersByTime(400)
    })
    act(() => pty.onConnected("conn-self", null))
    pty.connect.mockClear()
    act(() => pty.onPtyGrid({ rows: 40, cols: 120 }, false))
    act(() => {
      vi.advanceTimersByTime(1000)
    })
    expect(pty.connect).not.toHaveBeenCalled()
  })

  it("stands down while a take-over is armed, which is already a bounce", () => {
    const pty = mountWatcher()
    fireEvent.click(screen.getByText("Take over"))
    // The take-over's own bounce, and the only one this test permits.
    expect(pty.connect).toHaveBeenCalledTimes(1)
    act(() => pty.onPtyGrid({ rows: 40, cols: 120 }, false))
    act(() => {
      vi.advanceTimersByTime(1000)
    })
    expect(pty.connect).toHaveBeenCalledTimes(1)
  })
})

// THE TAKE-OVER CARD, full-pane and solid on purpose. It is not a rendering
// shield (the faithful view keeps the picture underneath clean); it says that
// a device with a different viewport size is driving this PTY and that taking
// over retargets the PTY's size to this device. The xterm stays mounted
// underneath, still receiving output, so reclaiming is instant.
describe("TerminalPane take-over card", () => {
  const card = () => screen.getByText("Take over").closest("div")

  it("paints solid over the whole pane, with the terminal still mounted underneath", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    act(() => notifyPtyOwner("s1", "conn-other"))
    // A full-pane, solid backdrop: it reads as "instead of" the terminal
    // rather than a banner over it.
    const backdrop = screen
      .getByText("Active on another device")
      .closest(".absolute")
    expect(backdrop).toBeTruthy()
    expect(backdrop!.className).toContain("inset-0")
    expect(backdrop!.className).toContain("bg-background")
    // And the terminal stays mounted underneath it, so reclaiming is instant.
    expect(screen.getByTestId("terminal-container")).toBeTruthy()
  })

  it("keeps the three titles, the second sentence, and the one action", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    act(() => notifyPtyOwner("s1", "conn-other"))
    expect(screen.getByText("Active on another device")).toBeTruthy()
    // The placeholder explains what the covered pane is for.
    expect(
      screen.getByText(/Take over to drive this agent from here/),
    ).toBeTruthy()
    expect(screen.getByText("Take over")).toBeTruthy()
    expect(card()).toBeTruthy()
  })

  it("is not rendered for the owner at all", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    act(() => last().onConnected("conn-self", null))
    expect(screen.queryByText("Take over")).toBeNull()
    expect(screen.queryByText("Active on another device")).toBeNull()
  })
})

// The take-over placeholder's device naming: a `pty.owner` handover carrying the
// other device's raw User-Agent must render "Open on {parsed label}", our own claim
// echo must restore the owner view, and a non-open events socket must drop the
// specific name back to the generic copy (the stale-name-on-reconnect fix).
describe("TerminalPane take-over device naming", () => {
  const chromeMac =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36"

  // THE MERE-ATTACH CASE, the user-reported regression. Under attach-never-steals
  // a watcher's only word on who drives arrives on the `connected` handshake; no
  // `pty.owner` ever follows a plain attach, so the handshake must carry the
  // owner's device label or the card can only say "Active on another device".
  it("names the owning device from the connected handshake alone, no pty.owner event", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    act(() => last().onConnected("conn-self", "conn-other", 3, chromeMac))
    expect(screen.getByText("Open on Chrome on macOS")).toBeTruthy()
    expect(screen.queryByText("Active on another device")).toBeNull()
  })

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

  // FLIPPED. Losing the events socket used to WIPE the name while
  // `ownerPresent` stayed true, so a flapping spine downgraded a perfectly good
  // "Open on Chrome on macOS" to the generic copy and back again. The wipe was
  // defending against a name going stale with no correction coming; the
  // correction now exists (the spine's own `input_owner`, checked on the next
  // open), so the name is kept and only ever replaced by a newer fact.
  it("KEEPS the specific name while the events socket is down", () => {
    const { rerender } = render(
      <TerminalPane kind="agent" id="s1" sessionId="s1" />,
    )
    act(() => notifyPtyOwner("s1", "conn-other", undefined, chromeMac))
    expect(screen.getByText("Open on Chrome on macOS")).toBeTruthy()
    mockState = makeState(false, "closed")
    act(() => rerender(<TerminalPane kind="agent" id="s1" sessionId="s1" />))
    expect(screen.getByText("Open on Chrome on macOS")).toBeTruthy()
    expect(screen.queryByText("Active on another device")).toBeNull()
  })
})

// TAKE-OVER IS A FRESH ATTACH. The button writes nothing down the live socket;
// it arms an intent and BOUNCES the socket, and the claim rides the first
// resize frame of the new connection, flagged.
//
// The reason is the viewer's own buffer. While the owner drives a wider grid,
// every cursor-positioned repaint overflows this narrower viewport and scrolls
// mangled wrapped rows into the LOCAL scrollback. A live claim resized the PTY,
// the child repainted cleanly, and nothing cleared what was already recorded:
// scrolling up after a take-over read back the garbage. Reconnecting routes
// through the reset-then-repaint path every reconnect already uses, so a
// take-over structurally cannot inherit viewer-era history.
//
// It also collapses the old dead-socket special case: there is no healthy path
// and unhealthy path any more, only the bounce.
describe("TerminalPane take-over is a fresh attach", () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  // Mount and let the deferred first-frame resize settle, then clear both spies
  // so a test observes only what its own click causes.
  const mountSettled = () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    // The mount's OWN open, which the real socket fires from `connect()`. It
    // matters here and nowhere else in this file: it is what makes the bounce's
    // reopen a RECONNECT (one plain resize) rather than a first open (the
    // two-step jiggle), and the claim rides that single frame.
    act(() => pty.onOpen())
    act(() => {
      vi.advanceTimersByTime(400)
    })
    pty.sendResize.mockClear()
    pty.connect.mockClear()
    return pty
  }

  /// A reopen that never produces a screen: the socket comes back and the
  /// handshake lands, but no replay frame follows, so the deferred first-frame
  /// resize is fired by the fallback timer instead. That is the state the
  /// bounded wait exists for.
  const reopenWithNoScreen = (
    pty: FakePtySocket,
    id: string,
    owner: string | null,
  ) => {
    act(() => {
      pty.onOpen()
      pty.onConnected(id, owner)
    })
    act(() => {
      vi.advanceTimersByTime(400)
    })
  }

  /// Run the replay wait out. The wait is measured in VISIBLE time, which is a
  /// sum of `performance.now()` deltas that fake timers do not move, so the
  /// reading is pushed past the configured wait and the poll is then let fire.
  const expireReplayWait = () => {
    const past = performance.now() + replayWaitMs() + REPLAY_WAIT_POLL_MS
    vi.spyOn(performance, "now").mockReturnValue(past)
    act(() => {
      vi.advanceTimersByTime(2 * REPLAY_WAIT_POLL_MS)
    })
  }

  /// Drive the reconnect the bounce asked for, all the way to the first PTY
  /// frame that triggers the first-frame resize. This is the reattach sequence
  /// the pane's own wiring runs on every reopen.
  const completeBounce = (
    pty: FakePtySocket,
    id = "conn-2",
    owner: string | null = "conn-other",
  ) => {
    act(() => {
      pty.onOpen()
      pty.onConnected(id, owner)
    })
    act(() => pty.bytesCb?.(new Uint8Array([0x61])))
    act(() => {
      vi.advanceTimersByTime(400)
    })
  }

  it("bounces the socket instead of claiming over it, and says it is reconnecting", () => {
    const pty = mountSettled()
    act(() => pty.onConnected("conn-self", null))
    act(() => notifyPtyOwner("s1", "conn-other"))

    fireEvent.click(screen.getByText("Take over"))
    // A FRESH ATTACH, whatever the socket's health: nothing goes down the live
    // one, because the polluted scrollback is on THIS side and only a replay
    // clears it.
    expect(pty.connect).toHaveBeenCalledTimes(1)
    expect(pty.sendResize).not.toHaveBeenCalled()
    // A deliberate `connect()` fires no `onReconnecting` of its own, so the
    // window would otherwise read as a frozen terminal. The press flipped the
    // verdict optimistically, so this pane is the owner and gets a cue rather
    // than the card. The wording is the launch one because this pane has never
    // had a screen (no replay frame in this fixture), which is what "first
    // attach" means: "Reconnecting…" would claim something went away.
    expect(screen.getByText("Starting claude…")).toBeTruthy()
    expect(screen.queryByText("Take over")).toBeNull()
  })

  it("carries the claim on the reconnect's FIRST resize frame, and spends it once", () => {
    const pty = mountSettled()
    act(() => pty.onConnected("conn-self", null))
    act(() => notifyPtyOwner("s1", "conn-other"))
    fireEvent.click(screen.getByText("Take over"))

    completeBounce(pty)
    // Flagged: a plain resize would be refused, because the pty is still
    // recorded to the other device until this frame lands. A PRESS names no
    // expected owner, because a press may take from anyone.
    expect(pty.sendResize).toHaveBeenCalledWith(24, 80, true, undefined)
    // And the cue is gone: the reopen cleared it.
    expect(screen.queryByText("Reconnecting…")).toBeNull()

    // The intent is SPENT. Every later resize is an ordinary one; a second
    // flagged frame would re-take a pty this client already owns.
    pty.sendResize.mockClear()
    act(() => {
      TermStub.instances.at(-1)!.resize(100, 30)
      vi.advanceTimersByTime(400)
    })
    expect(pty.sendResize).toHaveBeenCalledTimes(1)
    expect(pty.sendResize).toHaveBeenCalledWith(30, 100)
  })

  // The intent is state, not a queued frame, exactly so it survives the socket
  // churn a bounce is made of. A frame the socket silently discarded must leave
  // it armed, or the take-over is spent on nothing and the button looks broken.
  it("keeps the intent armed when the claim frame is dropped, and re-sends it", () => {
    const pty = mountSettled()
    act(() => pty.onConnected("conn-self", null))
    act(() => notifyPtyOwner("s1", "conn-other"))
    fireEvent.click(screen.getByText("Take over"))

    // The reopen's first resize is written into a socket that has re-dropped.
    pty.sendResize.mockReturnValue(false)
    completeBounce(pty)
    expect(pty.sendResize).toHaveBeenCalledWith(24, 80, true, undefined)

    // The next frame that actually goes out still carries it.
    pty.sendResize.mockClear()
    pty.sendResize.mockReturnValue(true)
    act(() => {
      TermStub.instances.at(-1)!.resize(100, 30)
      vi.advanceTimersByTime(400)
    })
    expect(pty.sendResize).toHaveBeenCalledWith(30, 100, true, undefined)
  })

  // THE POINT OF THE WHOLE ARC. A take-over must run the reset-then-replay path,
  // because this viewer's own scrollback is the thing that is polluted: while
  // the owner drove a wider grid, every repaint overflowed this narrower
  // viewport and scrolled mangled wrapped rows into it. Resizing the PTY makes
  // the child repaint cleanly and clears NOTHING already recorded. Only a fresh
  // attach's `reset()` does, and this pins that it happens, against a genuinely
  // NEW replay generation (a repeated generation is dropped whole, so a test
  // that forgot to bump it would pass for the wrong reason).
  it("runs the reset-and-replay path, against a fresh replay generation", () => {
    const pty = mountSettled()
    pty.replayGeneration = 1
    act(() => pty.onConnected("conn-self", null))
    act(() => notifyPtyOwner("s1", "conn-other"))
    const term = TermStub.instances.at(-1)!
    const resetsBefore = term.resets

    fireEvent.click(screen.getByText("Take over"))
    // The bounce's reopen, with the generation the server would mint for it.
    pty.replayGeneration = 2
    act(() => {
      pty.onOpen()
      pty.onConnected("conn-2", "conn-other")
    })
    act(() => pty.bytesCb?.(new Uint8Array([0x61])))
    expect(term.resets).toBe(resetsBefore + 1)
  })

  // Losing the race mid-bounce puts the card straight back, and the second
  // press works: the demotion retired the armed intent rather than leaving it
  // stuck and the button inert. (Idempotence of the press itself is pinned at
  // the machine, in `terminal/ownership.test.ts`, where a second press is
  // reachable without the card.)
  it("restores the card and re-arms when another device wins the race mid-bounce", () => {
    const pty = mountSettled()
    act(() => pty.onConnected("conn-self", null))
    act(() => notifyPtyOwner("s1", "conn-other"))
    fireEvent.click(screen.getByText("Take over"))
    expect(screen.queryByText("Take over")).toBeNull()

    act(() => pty.onOpen())
    act(() => pty.onConnected("conn-2", "conn-other"))
    act(() => notifyPtyOwner("s1", "conn-third", 9))
    expect(screen.getByText("Take over")).toBeTruthy()

    fireEvent.click(screen.getByText("Take over"))
    expect(pty.connect).toHaveBeenCalledTimes(2)
  })

  // A PLAIN BOUNCE IS NEVER A CLAIM, and the Reconnect box is a plain bounce.
  // `connect()` detaches the orphan's handlers before closing it, so no `closed`
  // reaches the ownership machine and an intent that was armed but never
  // confirmed on the wire SURVIVES the press. The next first resize would then
  // carry the take-over flag with no expected owner, which the server grants
  // unconditionally: a button labelled Reconnect would steal the pty from
  // whoever is typing into it by then.
  it("never carries a surviving take-over on a Reconnect press", () => {
    const pty = mountSettled()
    pty.emit("open")
    act(() => pty.onConnected("conn-self", null))
    act(() => notifyPtyOwner("s1", "conn-other"))
    fireEvent.click(screen.getByText("Take over"))
    // The claim frame is discarded by a socket that re-dropped, so the intent
    // stays armed (which is the designed behavior; see the test above).
    pty.sendResize.mockReturnValue(false)
    reopenWithNoScreen(pty, "conn-2", "conn-other")
    // No screen ever lands, so the bounded wait turns the cover into the box.
    expireReplayWait()
    expect(screen.getByText("Still waiting for the terminal's screen.")).toBeTruthy()

    pty.sendResize.mockClear()
    pty.sendResize.mockReturnValue(true)
    fireEvent.click(screen.getByText("Reconnect"))
    // The pty is UNOWNED by the time the press lands (the other device's own
    // socket went away), so this pane keeps the verdict and its first resize
    // reaches the wire. A flag on it would be granted unconditionally, which is
    // how a surviving intent takes a pty a plain attach would have had to ask
    // for.
    completeBounce(pty, "conn-3", null)
    // Every frame the press produced is an ordinary resize: no flag, no
    // expected owner, nothing the server can read as a transfer.
    expect(pty.sendResize.mock.calls.filter((call) => call[2] === true)).toEqual([])
    expect(pty.sendResize).toHaveBeenCalledWith(24, 80)
  })

  // The self-succession variant of the same press. The intent it arms NAMES the
  // ghost it expects to displace, and a stale name is refused by the server, so
  // the pane would sit at a geometry the pty never applied while believing it
  // owns it.
  it("never carries a surviving self-succession on a Reconnect press", () => {
    const pty = mountSettled()
    pty.emit("open")
    act(() => pty.onConnected("conn-self", null))
    // The socket blips and comes back; the handshake names this pane's own dead
    // connection, so the returning driver self-succeeds.
    pty.sendResize.mockReturnValue(false)
    reopenWithNoScreen(pty, "conn-2", "conn-self")
    expect(pty.sendResize).toHaveBeenCalledWith(24, 80, true, "conn-self")

    expireReplayWait()
    pty.sendResize.mockClear()
    pty.sendResize.mockReturnValue(true)
    fireEvent.click(screen.getByText("Reconnect"))
    // Nobody holds the pty now, so the stale expected owner this intent still
    // names would be refused by the server, leaving the pane believing it owns
    // a pty at a geometry that was never applied.
    completeBounce(pty, "conn-3", null)
    expect(pty.sendResize.mock.calls.filter((call) => call[2] === true)).toEqual([])
    expect(pty.sendResize).toHaveBeenCalledWith(24, 80)
  })

  it("shows the Reconnect affordance, not the take-over card, on a dead socket", () => {
    const pty = mountSettled()
    act(() => pty.onConnected("conn-self"))
    act(() => notifyPtyOwner("s1", "conn-other"))
    expect(screen.getByText("Take over")).toBeTruthy()
    // The socket gives up for good. The take-over card is a solid, full-pane
    // overlay painted ABOVE the connection overlays, so leaving it up hides the
    // one control that can fix this: the non-owner would click Take over into a
    // socket that is not there.
    act(() => pty.onReconnecting())
    pty.emit("failed")
    expect(screen.getByText("Connection lost.")).toBeTruthy()
    expect(screen.queryByText("Take over")).toBeNull()
    pty.connect.mockClear()
    fireEvent.click(screen.getByText("Reconnect"))
    expect(pty.connect).toHaveBeenCalledTimes(1)
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
    fireEvent.change(composeTextarea(), {
      target: { value: "draft survives" },
    })
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

// THE INPUT ⋯ MENU AND ITS THREE ANCHORS. It replaced a button that existed
// only while something was hidden, which is why the hidden-bars dead end kept
// coming back: a way back is not the same as a way there. The menu renders in
// EVERY bar state, at the leading edge of the bottom-most input row that
// exists, and exactly one instance of it is ever on screen.
describe("TerminalPane input menu anchors", () => {
  const desktopWidth = window.innerWidth
  // `goMobile` means "the phone SHELL is up AND touch is the primary pointer".
  // Those are two different signals since the compose bar moved off the width
  // breakpoint: width still drives the mobile layout, but the typing surfaces
  // are gated on `pointer: coarse`. Real phones report both.
  let pointerStub: MatchMediaStub | null = null
  const goMobile = () => {
    Object.defineProperty(window, "innerWidth", {
      value: 500,
      configurable: true,
    })
    pointerStub = stubCoarsePointer()
  }
  afterEach(async () => {
    Object.defineProperty(window, "innerWidth", {
      value: desktopWidth,
      configurable: true,
    })
    pointerStub?.restore()
    pointerStub = null
    // The surface switch writes a DEVICE-local choice that outlives the render,
    // so hand the decision back to the pointer before the next case.
    const mod = await import("@/lib/typingSurface")
    mod.setTypingSurface(null)
  })

  const triggers = () => screen.queryAllByRole("button", { name: "Input options" })
  const openMenu = () => fireEvent.click(triggers()[0]!)

  it("renders in the compose row while both bars are visible", () => {
    // The old restore button was ABSENT in this state, which is the whole bug:
    // nothing on screen led to the hide/show controls from the terminal.
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(triggers()).toHaveLength(1)
    // In the compose row, opposite Send, not in the key rows above it.
    expect(
      triggers()[0]!.closest("div")!.querySelector('[aria-label="Send"]'),
    ).toBeTruthy()
  })

  it("renders in the accessory bar's key row when the message box is off", () => {
    goMobile()
    const state = makeState()
    ;(state.bootstrap as unknown as { compose_bar?: string }).compose_bar =
      "never"
    mockState = state
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(screen.queryByRole("textbox", { name: "Message" })).toBeNull()
    expect(triggers()).toHaveLength(1)
    // Beside the keys, in row one: Esc is its neighbour.
    const row = triggers()[0]!.closest("div")!.parentElement!
    expect(row.querySelector('[aria-label="Esc"]')).toBeTruthy()
  })

  // THE DUPLICATE STATE. The keys are up, the message box is off and the top
  // bar is hidden. The old fallback row's condition ("compose off AND something
  // hidden") was true here at the same time as the accessory anchor, so this
  // exact state would have shipped TWO menus.
  it("renders exactly one menu with the keys up, the box off and the top bar hidden", () => {
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
    expect(triggers()).toHaveLength(1)
    // And it is the accessory bar's, not a second minimal row's.
    const row = triggers()[0]!.closest("div")!.parentElement!
    expect(row.querySelector('[aria-label="Esc"]')).toBeTruthy()
  })

  it("renders its own row when neither bar is up", () => {
    goMobile()
    const state = makeState()
    ;(state.bootstrap as unknown as { compose_bar?: string }).compose_bar =
      "never"
    ;(
      state.bootstrap as unknown as { mobile_accessory_bar?: boolean }
    ).mobile_accessory_bar = false
    mockState = state
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(screen.queryByRole("textbox", { name: "Message" })).toBeNull()
    expect(screen.queryByRole("button", { name: "Esc" })).toBeNull()
    expect(triggers()).toHaveLength(1)
  })

  it("carries the per-bar Show items, each writing only its own field", () => {
    goMobile()
    const state = makeState()
    ;(
      state.bootstrap as unknown as { mobile_accessory_bar?: boolean }
    ).mobile_accessory_bar = false
    mockState = state
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    openMenu()
    // The individual toggles replaced the one-tap restore-both: the menu names
    // what each one does, and each is its own preference.
    expect(screen.getByText("Hide top bar")).toBeTruthy()
    fireEvent.click(screen.getByText("Show terminal keys"))
    const fetchSpy = globalThis.fetch as unknown as ReturnType<typeof vi.fn>
    expect(fetchSpy).toHaveBeenCalledWith(
      "/api/v1/config/settings",
      expect.objectContaining({
        method: "PATCH",
        // `quiet: true` asks the server to skip its "Settings updated."
        // status: the keys visibly returning is the feedback.
        body: JSON.stringify({
          ui: { mobile_accessory_bar: true },
          quiet: true,
        }),
      }),
    )
  })

  it("offers the typing-surface switch as the action it performs", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    openMenu()
    // The message box is up, so the switch is the way OUT of it. It writes
    // through the same helper as the key row's Box/Direct cap.
    fireEvent.click(screen.getByText("Type directly in the terminal"))
    expect(screen.queryByRole("textbox", { name: "Message" })).toBeNull()
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

  // THE OTHER HALF OF THE PAIR. Holding only the SIGWINCH still let the
  // ResizeObserver's local `fit()` run every frame under the finger, and a local
  // fit is not free: xterm's buffer resize resets the scrolling region to full
  // screen on both buffers, so the mouse-tracking pager on the other end keeps
  // painting region-relative into a viewer that no longer has the region.
  it("performs NO local refit while the gesture holds the pair", () => {
    mountSettled()
    const term = TermStub.instances.at(-1)
    if (!term) throw new Error("no terminal constructed")
    FitStub.fits = 0
    startScroll()
    term.rows = 40
    act(() => {
      // Several observer rounds, the way a keyboard collapse really arrives.
      roCallbacks.forEach((cb) => cb())
      roCallbacks.forEach((cb) => cb())
      vi.advanceTimersByTime(250)
    })
    expect(FitStub.fits).toBe(0)
  })

  it("refits exactly once at the lift, together with the one send", () => {
    const pty = mountSettled()
    const term = TermStub.instances.at(-1)
    if (!term) throw new Error("no terminal constructed")
    FitStub.fits = 0
    startScroll()
    term.rows = 40
    act(() => {
      roCallbacks.forEach((cb) => cb())
      vi.advanceTimersByTime(250)
    })
    expect(FitStub.fits).toBe(0)
    fireEvent.touchEnd(container(), { touches: [], changedTouches: [] })
    // The refit is the first thing the lift does, before the debounce runs out.
    expect(FitStub.fits).toBe(1)
    act(() => {
      vi.advanceTimersByTime(250)
    })
    expect(FitStub.fits).toBe(1)
    expect(pty.sendResize).toHaveBeenCalledTimes(1)
    expect(pty.sendResize).toHaveBeenCalledWith(40, 80)
  })

  // A BYPASS PATH. The foreground resync sends directly rather than through the
  // debounced `sendSize`, so it needs its own route into the hold or the pair
  // comes apart exactly where a phone reconnect meets a finger.
  it("defers the foreground-resync resize (a direct send) to the lift", () => {
    const pty = mountSettled()
    const term = TermStub.instances.at(-1)
    if (!term) throw new Error("no terminal constructed")
    FitStub.fits = 0
    startScroll()
    act(() => {
      // The resync's trigger moved to the socket reopening; the property under
      // test (a DIRECT send still defers to the gesture's lift) is unchanged.
      pty.onOpen()
      vi.advanceTimersByTime(200)
    })
    expect(FitStub.fits).toBe(0)
    expect(pty.sendResize).not.toHaveBeenCalled()
    // The container settles at a new size while the request is parked: the
    // deferred send reads the geometry when it runs, so the final size wins.
    term.rows = 40
    fireEvent.touchEnd(container(), { touches: [], changedTouches: [] })
    act(() => {
      vi.advanceTimersByTime(250)
    })
    expect(FitStub.fits).toBe(1)
    expect(pty.sendResize).toHaveBeenCalledTimes(1)
    expect(pty.sendResize).toHaveBeenCalledWith(40, 80)
  })
})

// MITIGATION B: the viewer must not volunteer focus state while a replay is
// being applied. Parsing the replay's `?1004h` mode-restore tail makes xterm
// emit a focus report of its own through `onData` (measured against xterm
// 6.0.0; see `lib/suppressViewerReports.ts` for the exact call chain), so every
// replay applied to a pane that does not hold DOM focus used to type a spurious
// focus-OUT at the child. The claude CLI acts on focus state, so that is a real
// input, not a cosmetic one.
describe("TerminalPane focus reports raised by a replay", () => {
  const FOCUS_OUT = "\x1b[O"
  const encoded = new TextEncoder().encode(FOCUS_OUT)

  const term = () => {
    const t = TermStub.instances.at(-1)
    if (!t) throw new Error("no terminal constructed")
    return t
  }

  beforeEach(() => {
    vi.useFakeTimers()
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  // Mount and take the pane past its own first-frame plumbing, then arm the
  // socket for the (re)open whose replay the test delivers.
  const mountAwaitingReplay = () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    act(() => {
      vi.advanceTimersByTime(400)
    })
    act(() => pty.onOpen())
    pty.sendInput.mockClear()
    return pty
  }

  it("drops a focus report the replay chunk provokes", () => {
    const pty = mountAwaitingReplay()
    const t = term()
    // Model xterm: the report comes out of the parser DURING the write, before
    // the write's completion callback.
    t.write = (data: unknown, cb?: () => void) => {
      if (data instanceof Uint8Array) t.dataHandler?.(FOCUS_OUT)
      cb?.()
    }
    act(() => pty.bytesCb?.(new Uint8Array([1])))
    expect(pty.sendInput).not.toHaveBeenCalled()
  })

  it("still forwards a genuine focus report once the replay has landed", () => {
    const pty = mountAwaitingReplay()
    const t = term()
    act(() => pty.bytesCb?.(new Uint8Array([1])))
    // The user really did click away from the pane.
    act(() => t.dataHandler?.(FOCUS_OUT))
    expect(pty.sendInput).toHaveBeenCalledTimes(1)
    expect(pty.sendInput).toHaveBeenCalledWith(encoded)
  })

  it("suppresses the focus report on the RECONNECT drain path too", () => {
    // The reconnect replay takes the other branch: reset + drain, then the
    // FIRST held chunk (the repaint) through the suppression window. Reverting
    // that branch to a plain write must fail here.
    const pty = mountAwaitingReplay()
    const t = term()
    // First open's replay lands normally.
    act(() => pty.bytesCb?.(new Uint8Array([1])))
    pty.sendInput.mockClear()
    // The socket drops and reopens: the next binary frame is a reconnect
    // replay, which resets and drains before writing.
    act(() => pty.onReconnecting())
    act(() => pty.onOpen())
    let drainCb: (() => void) | undefined
    t.write = (data: unknown, cb?: () => void) => {
      if (typeof data === "string" && data === "") {
        drainCb = cb
        return
      }
      // The replay chunk parses `?1004h` and xterm volunteers a focus report
      // mid-write, before the completion callback.
      if (data instanceof Uint8Array) t.dataHandler?.(FOCUS_OUT)
      cb?.()
    }
    act(() => pty.bytesCb?.(new Uint8Array([2])))
    expect(drainCb).toBeDefined()
    act(() => drainCb?.())
    expect(pty.sendInput).not.toHaveBeenCalled()
    // The window closed with the replay write: a real focus change forwards.
    act(() => t.dataHandler?.(FOCUS_OUT))
    expect(pty.sendInput).toHaveBeenCalledTimes(1)
  })

  it("closes the window on the write CALLBACK, not on a timer", () => {
    const pty = mountAwaitingReplay()
    const t = term()
    // A write whose completion the test controls, the way a real (async) xterm
    // write behaves: the window must stay open for as long as the parse does,
    // however many timers tick meanwhile.
    const pending: { finish?: () => void } = {}
    t.write = (data: unknown, cb?: () => void) => {
      if (data instanceof Uint8Array) pending.finish = cb
      else cb?.()
    }
    act(() => pty.bytesCb?.(new Uint8Array([1])))
    act(() => {
      vi.advanceTimersByTime(5000)
      t.dataHandler?.(FOCUS_OUT)
    })
    expect(pty.sendInput).not.toHaveBeenCalled()
    act(() => pending.finish?.())
    act(() => t.dataHandler?.(FOCUS_OUT))
    expect(pty.sendInput).toHaveBeenCalledTimes(1)
    expect(pty.sendInput).toHaveBeenCalledWith(encoded)
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

  // Re-scoped at the take-over arc. Take-over no longer sends anything itself:
  // it bounces the socket, and the REOPEN's ordinary first-frame resize carries
  // the claim, flagged. So the property this suite cares about (a take-over
  // still tells the PTY this viewport's size) now holds one reconnect later,
  // through the same path every reattach uses.
  it("still claims by sending this viewport's size on TAKE OVER, one reattach later", () => {
    const pty = mountSettled()
    // The mount's own open, so the bounce's reopen is a reconnect (one plain
    // resize) rather than a first open (the jiggle).
    act(() => pty.onOpen())
    pty.sendResize.mockClear()
    act(() => pty.onConnected("conn-self", null))
    act(() => notifyPtyOwner("s1", "conn-other"))
    pty.sendResize.mockClear()
    fireEvent.click(screen.getByText("Take over"))
    expect(pty.sendResize).not.toHaveBeenCalled()

    act(() => {
      pty.onOpen()
      pty.onConnected("conn-2", "conn-other")
    })
    act(() => pty.bytesCb?.(new Uint8Array([0x61])))
    act(() => {
      vi.advanceTimersByTime(400)
    })
    // The reopen also runs the foreground resync (it moved to `pty.onOpen`), so
    // count the FLAGGED frame rather than every frame the reattach produces.
    expect(
      pty.sendResize.mock.calls.filter((c) => c[2] === true),
    ).toHaveLength(1)
    // A PRESSED take-over names no expected owner: a press may take from anyone.
    expect(
      pty.sendResize.mock.calls.find((c) => c[2] === true),
    ).toEqual([24, 80, true, undefined])
  })

  it("still re-asserts an UNCHANGED size when the socket REOPENS (the dedupe bypass)", () => {
    // The foreground resync moved from the visibility listener to `pty.onOpen`:
    // it used to run on every visibility signal, which meant it could fire at a
    // DEAD socket and book a size as delivered that never went out.
    const pty = mountSettled()
    pty.sendResize.mockClear()
    act(() => {
      pty.onOpen()
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
// tablet too; if the way back stayed gated on the mobile LAYOUT, that tablet
// got the desktop shell with no keys and no way to ask for them back. The input
// ⋯ therefore follows the same predicate that mounts the bars.
describe("TerminalPane input menu follows the touch surfaces", () => {
  let pointerStub: MatchMediaStub | null = null

  const setWidth = (value: number) =>
    Object.defineProperty(window, "innerWidth", { value, configurable: true })

  beforeEach(async () => {
    setWidth(1200)
    // A stored choice would offer the surface switch on its own (that is the
    // point of the wider gate, and there is a case for it below); start every
    // case here with the decision back in the pointer's hands.
    const mod = await import("@/lib/typingSurface")
    mod.setTypingSurface(null)
  })
  afterEach(async () => {
    setWidth(1200)
    pointerStub?.restore()
    pointerStub = null
    const mod = await import("@/lib/typingSurface")
    mod.setTypingSurface(null)
  })

  const hideAccessoryBar = () => {
    const state = makeState()
    ;(
      state.bootstrap as unknown as { mobile_accessory_bar?: boolean }
    ).mobile_accessory_bar = false
    return state
  }
  const trigger = () => screen.queryByRole("button", { name: "Input options" })

  it("offers the menu in the DESKTOP shell on a coarse pointer", () => {
    pointerStub = stubCoarsePointer(true)
    mockState = hideAccessoryBar()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    // The dead end: keys gone, desktop layout, and before this no way back.
    expect(screen.queryByRole("button", { name: "Esc" })).toBeNull()
    expect(trigger()).toBeTruthy()
  })

  it("brings the keys back from the menu, writing only that preference", () => {
    pointerStub = stubCoarsePointer(true)
    mockState = hideAccessoryBar()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    fireEvent.click(trigger()!)
    fireEvent.click(screen.getByText("Show terminal keys"))
    const fetchSpy = globalThis.fetch as unknown as ReturnType<typeof vi.fn>
    expect(fetchSpy).toHaveBeenCalledWith(
      "/api/v1/config/settings",
      expect.objectContaining({
        method: "PATCH",
        body: JSON.stringify({
          ui: { mobile_accessory_bar: true },
          quiet: true,
        }),
      }),
    )
  })

  // With the message box off too there is no bar left to anchor the menu, so
  // it takes its own minimal row in the desktop shell as well.
  it("falls back to its own row when the message box is off too", () => {
    pointerStub = stubCoarsePointer(true)
    const state = hideAccessoryBar()
    ;(state.bootstrap as unknown as { compose_bar?: string }).compose_bar =
      "never"
    mockState = state
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(screen.queryByRole("textbox", { name: "Message" })).toBeNull()
    expect(trigger()).toBeTruthy()
  })

  // A fine-pointer desktop never had the keys, never has the box, and (with
  // uploads switched off in this state) has nothing else to put in the menu.
  // An ⋯ that opens an empty popup is worse than no ⋯, so it renders none, and
  // no new row appears under a desktop terminal.
  it("offers nothing on a fine pointer with an empty item list", () => {
    pointerStub = stubCoarsePointer(false)
    mockState = hideAccessoryBar()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(trigger()).toBeNull()
  })

  // THE TRAP THE WIDER GATE CLOSES. A stored `compose` choice puts the message
  // box up on a FINE pointer (the choice replaces the capability answer), while
  // the accessory bar, and with it the Box/Direct cap, never mounts. Before
  // this the only control that could switch back was in the bar that is not
  // there; now the compose row's own ⋯ carries it.
  it("offers the surface switch on a fine pointer with a stored choice", async () => {
    const mod = await import("@/lib/typingSurface")
    mod.setTypingSurface("compose")
    pointerStub = stubCoarsePointer(false)
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(screen.getByRole("textbox", { name: "Message" })).toBeTruthy()
    expect(screen.queryByRole("button", { name: "Esc" })).toBeNull()
    fireEvent.click(trigger()!)
    fireEvent.click(screen.getByText("Type directly in the terminal"))
    expect(screen.queryByRole("textbox", { name: "Message" })).toBeNull()
  })

  // The TOP bar is the mobile shell's own chrome. The desktop shell never
  // renders it, so its preference being off hides nothing here and must not
  // put an unexplained row under a desktop terminal.
  it("ignores the top-bar preference in the desktop shell", () => {
    pointerStub = stubCoarsePointer(true)
    const state = makeState()
    ;(
      state.bootstrap as unknown as { mobile_top_bar?: boolean }
    ).mobile_top_bar = false
    mockState = state
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    // The keys are up, so the menu is anchored in them; what must not happen
    // is a second, minimal row appearing because of a bar this shell has not
    // got. The top-bar item is likewise absent from the menu.
    expect(screen.getByRole("button", { name: "Esc" })).toBeTruthy()
    expect(
      screen.queryAllByRole("button", { name: "Input options" }),
    ).toHaveLength(1)
    fireEvent.click(trigger()!)
    expect(screen.queryByText("Show top bar")).toBeNull()
  })
})

// THE VIEWER'S WAY BACK. Everything below the terminal used to be owner-gated,
// so a non-owner on a phone who hid the top bar from the header menu had hidden
// the only menu that could bring it back. The input ⋯ renders for them too,
// carrying that one item: not the keys (a write with no visible effect on their
// screen that would re-hide the owner's), and nothing about input.
describe("TerminalPane input menu for a non-owner", () => {
  const desktopWidth = window.innerWidth
  let pointerStub: MatchMediaStub | null = null

  beforeEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: 500,
      configurable: true,
    })
    pointerStub = stubCoarsePointer()
  })
  afterEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: desktopWidth,
      configurable: true,
    })
    pointerStub?.restore()
    pointerStub = null
    Object.defineProperty(document, "visibilityState", {
      value: "visible",
      configurable: true,
    })
  })

  // A VIEWER, made one the way the server makes one: the `connected` handshake
  // names another connection as the pty's owner and the pane seeds its verdict
  // from that. It used to be made by backgrounding the document, which was the
  // pane's whole non-owner story back when a foregrounded attach really did
  // claim; now a plain claim is refused silently, so the handshake is the only
  // thing that can tell a foregrounded pane it is watching rather than driving.
  // (The document stays VISIBLE here on purpose: seeding from the server has to
  // beat the foreground guess, and hiding the document would let the old
  // mechanism pass this suite whatever the seeding did.)
  const renderViewer = (state: DuxState) => {
    mockState = state
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    act(() => {
      FakePtySocket.instances.at(-1)!.onConnected("conn-self", "conn-other")
    })
  }

  it("offers the top-bar item once the top bar is hidden", () => {
    const state = makeState()
    ;(
      state.bootstrap as unknown as { mobile_top_bar?: boolean }
    ).mobile_top_bar = false
    renderViewer(state)
    // No typing surfaces for a viewer, per the tenet.
    expect(screen.queryByRole("textbox", { name: "Message" })).toBeNull()
    expect(screen.queryByRole("button", { name: "Esc" })).toBeNull()
    fireEvent.click(screen.getByRole("button", { name: "Input options" }))
    expect(screen.getByText("Show top bar")).toBeTruthy()
    expect(screen.queryByText("Show terminal keys")).toBeNull()
    expect(screen.queryByText("Attach a file…")).toBeNull()
  })

  it("stays out of the way while the top bar is on screen", () => {
    renderViewer(makeState())
    expect(
      screen.queryByRole("button", { name: "Input options" }),
    ).toBeNull()
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

// THE FAITHFUL VIEW'S OVERFLOW STATE AND THE LIVE PREFERENCE FLIPS. Three
// regressions pinned from the outside:
//  1. The coordinator's ResizeObserver must watch the HOST, because the
//     overflow branch pins the CONTAINER to the grid's pixel size; observing
//     the pinned box left the below-floor state deaf to every window resize
//     and stuck in pan mode forever.
//  2. A PROMOTION must land whole: leaving the faithful branch runs one fit
//     even when neither the family nor the size moved, or the freshly promoted
//     owner stays at the grid it adopted as a watcher.
//  3. While the overflow can scroll vertically, a vertical touch drag is left
//     to the browser (the host is the scroller); everywhere else the drag
//     keeps moving xterm's scrollback.
describe("TerminalPane faithful-view overflow and live preference flips", () => {
  let roCallbacks: (() => void)[]
  let roObserved: Element[]
  const installCapturingRO = () => {
    roCallbacks = []
    roObserved = []
    const cbs = roCallbacks
    const seen = roObserved
    vi.stubGlobal(
      "ResizeObserver",
      class {
        constructor(cb: () => void) {
          cbs.push(cb)
        }
        observe(el: Element) {
          seen.push(el)
        }
        unobserve() {}
        disconnect() {}
      },
    )
  }

  beforeEach(() => {
    // Fake timers fake requestAnimationFrame too, so `advanceTimersByTime`
    // drives the coordinator's one-frame observer deferral.
    vi.useFakeTimers()
    installCapturingRO()
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  const container = () => screen.getByTestId("terminal-container")
  const host = () => container().parentElement as HTMLElement
  const term = () => {
    const t = TermStub.instances.at(-1)
    if (!t) throw new Error("no terminal constructed")
    return t
  }

  // jsdom lays nothing out, so the measured boxes are stubbed per element.
  const setBox = (el: HTMLElement, width: number, height: number) => {
    Object.defineProperty(el, "clientWidth", {
      value: width,
      configurable: true,
    })
    Object.defineProperty(el, "clientHeight", {
      value: height,
      configurable: true,
    })
  }

  /// Mount as a WATCHER in the (default) faithful view and settle the
  /// deferred first-frame plumbing.
  const mountWatcher = () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    act(() => pty.onOpen())
    act(() => {
      vi.advanceTimersByTime(400)
    })
    act(() => pty.onConnected("conn-self", "conn-other"))
    return pty
  }

  /// Drive the watcher below the floor: a small measurable host and a big
  /// remote grid, so even the 7px floor cannot fit it and the relayout pins
  /// the container and flips the pane pannable.
  const mountOverflowed = () => {
    const pty = mountWatcher()
    setBox(host(), 200, 200)
    act(() => pty.onPtyGrid({ rows: 40, cols: 120 }, true))
    expect(term().options.fontSize).toBe(VIEWER_MIN_FONT_SIZE)
    expect(host().className).toContain("overflow-auto")
    expect(container().style.width).not.toBe("")
    return pty
  }

  it("observes the HOST, so a window resize still reaches the overflow state", () => {
    mountOverflowed()
    // The mechanism itself: the observed element is the host, never the
    // container the overflow branch just pinned.
    expect(roObserved).toContain(host())
    expect(roObserved).not.toContain(container())
    // The window grows enough for the grid to fit at the user's own size. The
    // pinned container does not move (that is the point), the host does.
    setBox(host(), 2000, 2000)
    act(() => {
      roCallbacks.forEach((cb) => cb())
      vi.advanceTimersByTime(50)
    })
    // The relayout ran off the observer: the font leaves the floor, the pin
    // is cleared, and the pane is no longer pannable.
    expect(term().options.fontSize).toBe(14)
    expect(container().style.width).toBe("")
    expect(container().style.height).toBe("")
    expect(host().className).not.toContain("overflow-auto")
  })

  it("runs one fit immediately on a PROMOTION out of the faithful branch", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    act(() => pty.onOpen())
    act(() => {
      vi.advanceTimersByTime(400)
    })
    act(() => pty.onConnected("conn-self", "conn-other"))
    // Adopted grid, and (jsdom's unmeasurable host) a font equal to the
    // preference: neither the family nor the size changes when this pane is
    // promoted, so only LEAVING the branch can force the fit.
    act(() => pty.onPtyGrid({ rows: 40, cols: 120 }, true))
    expect({ rows: term().rows, cols: term().cols }).toEqual({
      rows: 40,
      cols: 120,
    })
    expect(term().options.fontSize).toBe(14)
    const fitsBefore = FitStub.fits
    FitStub.nextDims = { rows: 24, cols: 80 }
    // The take-over lands: this pane is the driver, so its own container
    // defines the grid again.
    act(() => notifyPtyOwner("s1", "conn-self"))
    expect(FitStub.fits).toBe(fitsBefore + 1)
    expect({ rows: term().rows, cols: term().cols }).toEqual({
      rows: 24,
      cols: 80,
    })
  })

  it("paces the touch scrollback off the rendered screen, not the host-sized container", () => {
    // The letterboxed watcher: the container stays host-sized while the grid
    // renders smaller inside it. A row is what the FINGER sees, so the cadence
    // divides the screen's height by the rows; dividing the container's
    // overestimated the row and scrolled fewer lines than the drag covered.
    mountWatcher()
    const screenEl = term().element?.querySelector(
      ".xterm-screen",
    ) as HTMLElement
    Object.defineProperty(screenEl, "clientHeight", {
      value: 240,
      configurable: true,
    })
    setBox(container(), 480, 480)
    // 24 rows over 240px: a 10px row. A 20px upward drag is two whole rows;
    // the container formula (480 / 24 = 20px rows) would have scrolled one.
    fireEvent.touchStart(container(), {
      touches: [{ clientX: 10, clientY: 300 }],
    })
    fireEvent.touchMove(container(), {
      touches: [{ clientX: 10, clientY: 280 }],
    })
    expect(term().scrollLineCalls).toEqual([2])
  })

  it("leaves a vertical drag to the browser while the overflow scrolls vertically, and keeps intercepting otherwise", () => {
    mountOverflowed()
    // The host really can scroll vertically: content taller than the box.
    Object.defineProperty(host(), "scrollHeight", {
      value: 400,
      configurable: true,
    })
    fireEvent.touchStart(container(), {
      touches: [{ clientX: 10, clientY: 300 }],
    })
    fireEvent.touchMove(container(), {
      touches: [{ clientX: 10, clientY: 280 }],
    })
    fireEvent.touchEnd(container(), { touches: [], changedTouches: [] })
    // Not intercepted: no local scrollback motion, nothing forwarded, the
    // browser pans the host natively.
    expect(term().scrollLineCalls).toEqual([])
    // The overflow retires (the window grows), and the same drag scrolls
    // xterm's scrollback again.
    setBox(host(), 2000, 2000)
    Object.defineProperty(host(), "scrollHeight", {
      value: 240,
      configurable: true,
    })
    act(() => {
      roCallbacks.forEach((cb) => cb())
      vi.advanceTimersByTime(50)
    })
    expect(host().className).not.toContain("overflow-auto")
    fireEvent.touchStart(container(), {
      touches: [{ clientX: 10, clientY: 300 }],
    })
    fireEvent.touchMove(container(), {
      touches: [{ clientX: 10, clientY: 280 }],
    })
    fireEvent.touchEnd(container(), { touches: [], changedTouches: [] })
    expect(term().scrollLineCalls.length).toBeGreaterThan(0)
  })
})

// THE DRAFT OUTLIVES THE PANE. It lives in the store keyed by target id
// precisely because `reconnect()` bumps `terminalEpoch` to REMOUNT the pane, and
// that Retry is the gesture a user reaches for after a bad network, which is
// exactly when they are most likely to have an unsent message typed.
describe("TerminalPane compose draft survives a remount", () => {
  const desktopWidth = window.innerWidth
  let pointerStub: MatchMediaStub | null = null
  const goMobile = () => {
    Object.defineProperty(window, "innerWidth", { value: 500, configurable: true })
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

  it("survives the pane REMOUNT that Retry performs", () => {
    goMobile()
    const { unmount } = render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    fireEvent.change(composeTextarea(), { target: { value: "half a thought" } })
    expect(composeTextarea().value).toBe("half a thought")
    // `reconnect()` bumps `terminalEpoch`, which changes the pane's React key and
    // throws this component away. The draft must not go with it.
    unmount()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(composeTextarea().value).toBe("half a thought")
  })

  it("survives a pagehide / pageshow round trip", () => {
    goMobile()
    const { unmount } = render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    fireEvent.change(composeTextarea(), { target: { value: "typed on a train" } })
    act(() => {
      window.dispatchEvent(new Event("pagehide"))
    })
    unmount()
    act(() => {
      window.dispatchEvent(new Event("pageshow"))
    })
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(composeTextarea().value).toBe("typed on a train")
  })

  it("keeps each target's draft apart", () => {
    goMobile()
    const { unmount } = render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    fireEvent.change(composeTextarea(), { target: { value: "for the agent" } })
    unmount()
    render(
      <TerminalPane
        kind="terminal"
        id="t1"
        owner={{ kind: "session", session_id: "s1" }}
      />,
    )
    expect(composeTextarea().value).toBe("")
  })
})

// NO AUTOFOCUS, AND NO SOFT KEYBOARD, until the handshake has confirmed
// ownership AND the replay for the current attach epoch is on screen.
describe("TerminalPane does not summon the keyboard before it has reconciled", () => {
  const desktopWidth = window.innerWidth
  let pointerStub: MatchMediaStub | null = null
  const goMobile = () => {
    Object.defineProperty(window, "innerWidth", { value: 500, configurable: true })
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

  it("does not focus the message box before the handshake and the replay", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    // The socket opened, but the server has answered neither question yet.
    act(() => pty.onOpen())
    expect(document.activeElement).not.toBe(composeTextarea())
    // The handshake alone is not enough: the screen is still missing.
    act(() => pty.onConnected("conn-self", null))
    expect(document.activeElement).not.toBe(composeTextarea())
    // The replay lands, and only now does the keyboard come up.
    act(() => pty.bytesCb?.(new Uint8Array([0x61])))
    expect(document.activeElement).toBe(composeTextarea())
  })

  it("never interrupts an IME composition to take focus", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = last()
    // Somebody is mid-composition somewhere in the pane.
    act(() => {
      document.dispatchEvent(new Event("compositionstart", { bubbles: true }))
    })
    act(() => {
      pty.onOpen()
      pty.onConnected("conn-self", null)
    })
    act(() => pty.bytesCb?.(new Uint8Array([0x61])))
    // Moving focus here would destroy the half-typed text and its candidate
    // popup, so it does not happen.
    expect(document.activeElement).not.toBe(composeTextarea())
  })
})
