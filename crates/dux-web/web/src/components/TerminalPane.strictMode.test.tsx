// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import React from "react"
import { cleanup, render } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { ConnState } from "@/lib/types"

// How many PTY sockets ONE pane mount opens. `main.tsx` wraps the app in
// `<StrictMode>`, which double-invokes effects in development (setup, cleanup,
// setup), and the pane's attach effect constructs its socket in that setup. The
// numbers below are measured against this build, not predicted: they are the
// contract that a second construction is always accompanied by the disposal of
// the first, so a development mount can never leave two sockets on one PTY.

// The pane's xterm, cut down to what MOUNTING needs: no buffer, no selection
// model, no mouse geometry. Nothing here is read by these assertions; it exists
// so the attach effect can run to completion and be torn down again.
class TermStub {
  static instances: TermStub[] = []
  rows = 24
  cols = 80
  element: HTMLElement | null = null
  textarea = {
    setAttribute: () => {},
    tabIndex: 0,
    blur: () => {},
    focus: () => {},
    value: "",
  }
  modes = {
    mouseTrackingMode: "none",
    applicationCursorKeysMode: false,
    bracketedPasteMode: false,
  }
  buffer = {
    active: {
      type: "normal",
      viewportY: 0,
      getLine: () => undefined,
    },
  }
  parser = {
    registerOscHandler: () => ({ dispose: () => {} }),
  }
  constructor() {
    TermStub.instances.push(this)
  }
  loadAddon(addon: { activate?: (term: TermStub) => void }) {
    addon.activate?.(this)
  }
  open(container: HTMLElement) {
    const element = container.ownerDocument.createElement("div")
    element.className = "xterm"
    const screen = container.ownerDocument.createElement("div")
    screen.className = "xterm-screen"
    element.appendChild(screen)
    container.appendChild(element)
    this.element = element
  }
  onData() {
    return { dispose: () => {} }
  }
  onBinary() {
    return { dispose: () => {} }
  }
  resizeListeners: ((size: { cols: number; rows: number }) => void)[] = []
  onResize(cb: (size: { cols: number; rows: number }) => void) {
    this.resizeListeners.push(cb)
    return {
      dispose: () => {
        this.resizeListeners = this.resizeListeners.filter((l) => l !== cb)
      },
    }
  }
  resize(cols: number, rows: number) {
    if (cols === this.cols && rows === this.rows) return
    this.cols = cols
    this.rows = rows
    for (const cb of [...this.resizeListeners]) cb({ cols, rows })
  }
  attachCustomKeyEventHandler() {}
  focus() {}
  hasSelection() {
    return false
  }
  getSelection() {
    return ""
  }
  select() {}
  selectAll() {}
  clearSelection() {}
  scrollLines() {}
  scrollToBottom() {}
  reset() {}
  paste() {}
  write(_data: unknown, cb?: () => void) {
    cb?.()
  }
  dispose = vi.fn()
}

class FitStub {
  activate() {}
  dispose() {}
  fit() {}
}

// The socket double. `connect` and `dispose` are spies because they are exactly
// what this file counts: a construction that never connects costs nothing, and
// a construction never disposed is the leak.
class FakePtySocket {
  static instances: FakePtySocket[] = []
  url: string
  connect = vi.fn()
  close = vi.fn()
  dispose = vi.fn()
  sendResize = vi.fn(() => true)
  sendInput = vi.fn()
  sendBeat = vi.fn(() => true)
  onBeat: (n: number) => void = () => {}
  dropForRetry = vi.fn()
  resumeNow = vi.fn()
  park = vi.fn()
  isOpen = true
  onConnected: (id: string) => void = () => {}
  onPtyGrid: (
    grid: { rows: number; cols: number } | null,
    fromHandshake: boolean,
  ) => void = () => {}
  onOpen: () => void = () => {}
  onReconnecting: () => void = () => {}
  onConn: (state: ConnState) => void = () => {}
  bytesCb: ((b: Uint8Array) => void) | null = null
  onBytes = (cb: (b: Uint8Array) => void): void => {
    this.bytesCb = cb
  }
  shouldRetry: () => boolean = () => true
  onGone: () => void = () => {}
  replayGeneration: number | null = null
  constructor(url: string) {
    this.url = url
    FakePtySocket.instances.push(this)
  }
}

vi.mock("@xterm/xterm", () => ({ Terminal: TermStub }))
vi.mock("@xterm/addon-fit", () => ({ FitAddon: FitStub }))
vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), {
    success: vi.fn(),
    info: vi.fn(),
    warning: vi.fn(),
    error: vi.fn(),
  }),
}))
vi.mock("@/lib/suppressViewerReports", async (importOriginal) => {
  const actual =
    await importOriginal<typeof import("@/lib/suppressViewerReports")>()
  return { ...actual, suppressViewerReports: () => {} }
})
vi.mock("@/lib/agentNotifications", () => ({
  registerAgentNotifications: () => () => {},
}))
vi.mock("@/components/MacroPopover", () => ({ MacroPopover: () => null }))
vi.mock("@/lib/ptySocket", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/ptySocket")>()
  return { ...actual, PtySocket: FakePtySocket }
})

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => ({ ...mockState, composeDrafts: actual.useDux().composeDrafts }),
  }
})

// The store reads `localStorage` at import time, transitively through the pane.
installStubs()
const { TerminalPane } = await import("./TerminalPane")
const { getActivePtySocket, setActivePtySocket } = await import(
  "@/lib/ptySocket"
)

function makeState(): DuxState {
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
      terminals: [],
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

const paneProps = { kind: "agent", id: "s1", sessionId: "s1" } as const

beforeEach(() => {
  FakePtySocket.instances = []
  TermStub.instances = []
  setActivePtySocket(null)
  mockState = makeState()
  installStubs()
})

afterEach(() => {
  cleanup()
  setActivePtySocket(null)
  vi.unstubAllGlobals()
})

describe("how many PTY sockets one pane mount opens", () => {
  it("constructs exactly one, and connects it once, outside StrictMode", () => {
    render(<TerminalPane {...paneProps} />)

    expect(FakePtySocket.instances).toHaveLength(1)
    expect(FakePtySocket.instances[0].connect).toHaveBeenCalledTimes(1)
    expect(FakePtySocket.instances[0].dispose).not.toHaveBeenCalled()
    expect(getActivePtySocket()).toBe(FakePtySocket.instances[0])
  })

  it("disposes that socket and clears the registration on unmount", () => {
    const view = render(<TerminalPane {...paneProps} />)
    view.unmount()

    expect(FakePtySocket.instances).toHaveLength(1)
    expect(FakePtySocket.instances[0].dispose).toHaveBeenCalledTimes(1)
    expect(getActivePtySocket()).toBeNull()
  })

  it("constructs two under StrictMode, one per effect invocation", () => {
    render(
      <React.StrictMode>
        <TerminalPane {...paneProps} />
      </React.StrictMode>,
    )

    // Setup, cleanup, setup: the second setup builds a whole second socket.
    // Production builds invoke the effect once and see the count above.
    expect(FakePtySocket.instances).toHaveLength(2)
    // Both are told to connect, so in development the discarded one really does
    // reach the wire before its cleanup disposes it.
    expect(
      FakePtySocket.instances.map((pty) => pty.connect.mock.calls.length),
    ).toEqual([1, 1])
  })

  it("disposes every StrictMode socket but the last", () => {
    render(
      <React.StrictMode>
        <TerminalPane {...paneProps} />
      </React.StrictMode>,
    )

    const disposals = FakePtySocket.instances.map(
      (pty) => pty.dispose.mock.calls.length,
    )
    expect(disposals).toEqual([1, 0])
  })

  it("leaves the surviving socket registered as the active one", () => {
    render(
      <React.StrictMode>
        <TerminalPane {...paneProps} />
      </React.StrictMode>,
    )

    // The discarded socket clears the registration on its way out and the
    // survivor claims it, so the macro picker writes to the live PTY rather
    // than to a socket the pane has already thrown away.
    expect(getActivePtySocket()).toBe(FakePtySocket.instances.at(-1))
    expect(getActivePtySocket()).not.toBe(FakePtySocket.instances[0])
  })

  it("disposes the StrictMode survivor too, and clears the registration", () => {
    const view = render(
      <React.StrictMode>
        <TerminalPane {...paneProps} />
      </React.StrictMode>,
    )
    view.unmount()

    // Both sockets end disposed: the discarded one by the double-invoke's own
    // cleanup, the survivor by the unmount. Nothing is left holding the PTY.
    const disposals = FakePtySocket.instances.map(
      (pty) => pty.dispose.mock.calls.length,
    )
    expect(disposals).toEqual([1, 1])
    expect(getActivePtySocket()).toBeNull()
  })
})

