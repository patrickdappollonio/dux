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
  textarea = { setAttribute() {}, blur() {} }
  buffer = { active: { type: "normal" } }
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
  sendViewed = vi.fn()
  // Mirrors the real socket's `isOpen` getter; a test flips it to false to
  // model a disconnected socket (the compose bar's Send checks it).
  isOpen = true
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
// Spy on sonner so the compose bar's refused-send tests can assert the user
// was told WHY the message stayed in the buffer (owner/offline/oversized).
const toastError = vi.fn()
vi.mock("sonner", () => ({
  toast: Object.assign(vi.fn(), {
    success: vi.fn(),
    error: (...args: unknown[]) => toastError(...args),
  }),
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
  notifyRegistrations.length = 0
  toastError.mockClear()
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

// The floating "Macros…" trigger is desktop chrome only: on a phone it sat
// over the PTY text and made the text under it unreadable, so the mobile
// entry point is the terminal screen's header (MobileShell), never this
// overlay.
describe("TerminalPane floating macro trigger", () => {
  const desktopWidth = window.innerWidth
  afterEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: desktopWidth,
      configurable: true,
    })
  })

  it("renders the floating trigger on desktop", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(screen.getByTestId("macro-popover")).toBeTruthy()
  })

  it("does not render the floating trigger on mobile", () => {
    Object.defineProperty(window, "innerWidth", {
      value: 500,
      configurable: true,
    })
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    expect(screen.queryByTestId("macro-popover")).toBeNull()
  })
})

// The mobile compose bar (the `ui.compose_bar` preference, default on): the
// third row of the mobile shell, whose Send delivers the buffered message plus
// a submitting Enter to the PTY through the pure `composeSendBytes` rules.
// `useIsMobile` reads `window.innerWidth`, so shrinking it below the 768px
// breakpoint is how these tests mount the mobile shell.
describe("TerminalPane mobile compose bar", () => {
  const desktopWidth = window.innerWidth
  const goMobile = () => {
    Object.defineProperty(window, "innerWidth", {
      value: 500,
      configurable: true,
    })
  }
  afterEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: desktopWidth,
      configurable: true,
    })
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
    ;(state.bootstrap as unknown as { compose_bar?: boolean }).compose_bar =
      false
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
    ;(off.bootstrap as unknown as { compose_bar?: boolean }).compose_bar = false
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
  const goMobile = () => {
    Object.defineProperty(window, "innerWidth", {
      value: 500,
      configurable: true,
    })
  }
  afterEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: desktopWidth,
      configurable: true,
    })
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
  const goMobile = () => {
    Object.defineProperty(window, "innerWidth", {
      value: 500,
      configurable: true,
    })
  }
  afterEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: desktopWidth,
      configurable: true,
    })
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
    ;(state.bootstrap as unknown as { compose_bar?: boolean }).compose_bar =
      false
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
  const goMobile = () => {
    Object.defineProperty(window, "innerWidth", {
      value: 500,
      configurable: true,
    })
  }
  afterEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: desktopWidth,
      configurable: true,
    })
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
    ;(state.bootstrap as unknown as { compose_bar?: boolean }).compose_bar =
      false
    mockState = state
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    // No preventDefault: the synthetic mouse events flow and xterm focuses its
    // hidden textarea exactly as before the compose bar existed.
    expect(tap(container())).toBe(false)
  })

  it("forwards a synthetic SGR click to a mouse-tracking app AND focuses compose", () => {
    goMobile()
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pty = FakePtySocket.instances.at(-1)
    const term = TermStub.instances.at(-1)
    if (!pty || !term) throw new Error("no pty/term constructed")
    // The app in the PTY has grabbed the mouse: the swallowed tap must still
    // reach it as a left click (press + release) at the tapped cell, or
    // tap-driven TUIs go dead with the compose bar up.
    term.modes.mouseTrackingMode = "sgr"
    const prevented = tap(container())
    expect(prevented).toBe(true)
    expect(document.activeElement).toBe(composeTextarea())
    expect(pty.sendInput).toHaveBeenCalledTimes(1)
    const bytes = new TextDecoder().decode(
      pty.sendInput.mock.calls[0][0] as Uint8Array,
    )
    // jsdom rects/sizes are all 0, so the drag-path math degrades to the
    // 1px-per-cell guard: cell = floor(10 / 1) + 1 = 11 on both axes.
    expect(bytes).toBe("\x1b[<0;11;11M\x1b[<0;11;11m")
  })
})

// The accessory-bar render gate (the `ui.mobile_accessory_bar` preference,
// default on) sits beside the owner gate: hiding the key rows returns them to
// the terminal, while the compose bar (its own preference) stays.
describe("TerminalPane mobile accessory-bar preference", () => {
  const desktopWidth = window.innerWidth
  const goMobile = () => {
    Object.defineProperty(window, "innerWidth", {
      value: 500,
      configurable: true,
    })
  }
  afterEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: desktopWidth,
      configurable: true,
    })
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
  const goMobile = () => {
    Object.defineProperty(window, "innerWidth", {
      value: 500,
      configurable: true,
    })
  }
  afterEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: desktopWidth,
      configurable: true,
    })
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
  const goMobile = () => {
    Object.defineProperty(window, "innerWidth", {
      value: 500,
      configurable: true,
    })
  }
  afterEach(() => {
    Object.defineProperty(window, "innerWidth", {
      value: desktopWidth,
      configurable: true,
    })
  })

  it("renders the restore button in its own bottom row when a bar is hidden", () => {
    goMobile()
    const state = makeState()
    ;(
      state.bootstrap as unknown as {
        compose_bar?: boolean
        mobile_top_bar?: boolean
      }
    ).compose_bar = false
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
    ;(state.bootstrap as unknown as { compose_bar?: boolean }).compose_bar =
      false
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
