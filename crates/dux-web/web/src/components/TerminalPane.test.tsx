// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { ConnState } from "@/lib/types"
import { notifyPtyOwner, resetPtyOwnerEpochs } from "@/lib/ptyOwnership"

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
  rows = 24
  cols = 80
  textarea = { setAttribute() {}, blur() {} }
  buffer = { active: { type: "normal" } }
  modes = { mouseTrackingMode: "none", applicationCursorKeysMode: false }
  loadAddon() {}
  open() {}
  onData() {
    return { dispose() {} }
  }
  attachCustomKeyEventHandler() {}
  focus() {}
  getSelection() {
    return ""
  }
  selectAll() {}
  scrollLines() {}
  scrollToBottom() {}
  clearSelection() {}
  reset() {}
  paste() {}
  write(_data: unknown, cb?: () => void) {
    cb?.()
  }
  dispose() {}
}

class FitStub {
  fit() {}
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
  sendResize = vi.fn()
  sendInput = vi.fn()
  onConnected: (id: string) => void = () => {}
  onOpen: () => void = () => {}
  onReconnecting: () => void = () => {}
  onConn: (state: ConnState) => void = () => {}
  onBytes: (cb: (b: Uint8Array) => void) => void = () => {}
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
vi.mock("@/lib/suppressViewerReports", () => ({ suppressViewerReports: () => {} }))
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
          terminals: [{ id: "t1", has_output: false }],
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
  mockState = makeState()
  installStubs()
  // The `pty.owner` epoch high-water marks are module-global; reset so a handover
  // in one test is never dropped as "stale" by a prior test's epoch.
  resetPtyOwnerEpochs()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

// Run the same behavioral contract for both surfaces the pane serves. The
// companion-terminal ("terminal") case is the load-bearing one: an epoch-only
// reconnect is a NO-OP for it, so this parametrization is the regression guard.
describe.each([
  { kind: "agent" as const, id: "s1" },
  { kind: "terminal" as const, id: "t1" },
])("TerminalPane connectionLost affordance ($kind)", ({ kind, id }) => {
  it("shows the Reconnect affordance on 'failed' without doubling the spinner", () => {
    render(<TerminalPane kind={kind} id={id} sessionId="s1" />)
    last().emit("failed")
    expect(screen.getByText("Connection lost.")).toBeTruthy()
    expect(screen.getByText("Reconnect")).toBeTruthy()
    // The connection-lost block replaces (does not stack with) the reconnecting
    // spinner — no double overlay.
    expect(screen.queryByText("Reconnecting…")).toBeNull()
  })

  it("Reconnect calls the pane's OWN socket.connect() (not an epoch no-op)", () => {
    render(<TerminalPane kind={kind} id={id} sessionId="s1" />)
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
    render(<TerminalPane kind={kind} id={id} sessionId="s1" />)
    const pty = last()
    pty.emit("failed")
    expect(screen.getByText("Connection lost.")).toBeTruthy()
    pty.emit("open")
    expect(screen.queryByText("Connection lost.")).toBeNull()
  })

  it("suppresses its own connectionLost overlay while globally offline", () => {
    mockState = makeState(true)
    render(<TerminalPane kind={kind} id={id} sessionId="s1" />)
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
