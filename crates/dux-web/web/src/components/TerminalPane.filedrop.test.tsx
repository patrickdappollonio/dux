// @vitest-environment jsdom
//
// The file-drop journeys, driven through the real pane against a stubbed xterm
// and a stubbed upload. What is mocked is only what genuinely cannot run here:
// the terminal emulator (it needs a canvas) and the network. The ordering, the
// gating and the reporting are all the real code.
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import { notifyPtyOwner, resetPtyOwnerEpochs } from "@/lib/ptyOwnership"

class TermStub {
  static instances: TermStub[] = []
  /// Every `paste()` call, in order. This is the whole point of the stub: it is
  /// what proves the paste ORDER and the payload SHAPE.
  static pastes: string[] = []
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
  paste(text: string) {
    TermStub.pastes.push(text)
  }
  write(_data: unknown, cb?: () => void) {
    cb?.()
  }
  dispose() {}
}

class FitStub {
  fit() {}
  proposeDimensions() {
    return { rows: 24, cols: 80 }
  }
}

class FakePtySocket {
  static instances: FakePtySocket[] = []
  url: string
  connect = vi.fn()
  close = vi.fn()
  sendResize = vi.fn()
  sendInput = vi.fn()
  sendViewed = vi.fn()
  isOpen = true
  onConnected: (id: string) => void = () => {}
  onOpen: () => void = () => {}
  onReconnecting: () => void = () => {}
  onConn: () => void = () => {}
  onBytes: (cb: (b: Uint8Array) => void) => void = () => {}
  shouldRetry: () => boolean = () => true
  onGone: () => void = () => {}
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
    error: vi.fn(),
    warning: vi.fn(),
    loading: vi.fn(),
    dismiss: vi.fn(),
  }),
}))
vi.mock("@/lib/suppressViewerReports", () => ({
  suppressViewerReports: () => {},
}))
vi.mock("@/components/MacroPopover", () => ({ MacroPopover: () => null }))
vi.mock("@/lib/ptySocket", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/ptySocket")>()
  return { ...actual, PtySocket: FakePtySocket }
})

/// The upload, stubbed at the API boundary. Each call resolves with a saved
/// file at the requested name unless a test overrides it.
const uploadDroppedFile = vi.fn()
vi.mock("@/lib/fileDropApi", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/fileDropApi")>()
  return { ...actual, uploadDroppedFile: (...a: unknown[]) => uploadDroppedFile(...a) }
})

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState }
})

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
    vi.fn(() => Promise.reject(new Error("no network in tests"))),
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

installStubs()
const { TerminalPane } = await import("./TerminalPane")
const { toast } = await import("sonner")

function makeState(): DuxState {
  return {
    conn: "open",
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
        { id: "t1", owner: { kind: "session", session_id: "s1" }, has_output: false },
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
      status_clear_seconds: 6,
    },
    offline: false,
    terminalEpoch: 0,
  } as unknown as DuxState
}

/// A `DataTransfer` stand-in. jsdom does not construct one with files, and the
/// pane only ever reads `types` and `files`.
function fileTransfer(files: File[]) {
  return { types: ["Files"], files, dropEffect: "none" }
}

function saved(name: string, savedName = name) {
  return {
    path: `/tmp/p1/${savedName}`,
    saved_name: savedName,
    requested_name: name,
    folder: "/tmp/p1",
    folder_label: "~/p1",
    renamed: name !== savedName,
  }
}

async function drop(files: File[]) {
  const pane = screen.getByTestId("terminal-container").closest(".group")!
  await act(async () => {
    fireEvent.drop(pane, { dataTransfer: fileTransfer(files) })
    // Let the sequential upload loop run to completion.
    await Promise.resolve()
    await Promise.resolve()
    await Promise.resolve()
    await Promise.resolve()
  })
}

function file(name: string) {
  return new File(["x"], name, { type: "image/png" })
}

beforeEach(() => {
  FakePtySocket.instances = []
  TermStub.instances = []
  TermStub.pastes = []
  uploadDroppedFile.mockReset()
  vi.mocked(toast.success).mockClear()
  vi.mocked(toast.warning).mockClear()
  vi.mocked(toast.error).mockClear()
  mockState = makeState()
  installStubs()
  resetPtyOwnerEpochs()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("dropping a file onto an agent", () => {
  it("uploads it and pastes the quoted path with no newline", async () => {
    uploadDroppedFile.mockResolvedValue(saved("shot.png"))
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    await drop([file("shot.png")])

    expect(uploadDroppedFile).toHaveBeenCalledTimes(1)
    expect(TermStub.pastes).toEqual(["'/tmp/p1/shot.png' "])
    // A newline SUBMITS in these tools, so its absence is the load-bearing part.
    expect(TermStub.pastes[0]).not.toContain("\n")
    expect(vi.mocked(toast.success)).toHaveBeenCalled()
  })

  it("tells the user the new name when the file was renamed", async () => {
    uploadDroppedFile.mockResolvedValue(saved("shot.png", "shot-S-1.png"))
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    await drop([file("shot.png")])

    const message = vi.mocked(toast.success).mock.calls[0][0] as string
    expect(message).toContain("shot.png")
    expect(message).toContain("shot-S-1.png")
    expect(TermStub.pastes).toEqual(["'/tmp/p1/shot-S-1.png' "])
  })

  it("carries the terminal socket's own connection id, not the events one", async () => {
    uploadDroppedFile.mockResolvedValue(saved("shot.png"))
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    act(() => FakePtySocket.instances.at(-1)!.onConnected("42"))
    await drop([file("shot.png")])
    expect(uploadDroppedFile.mock.calls[0][1]).toEqual({ pty: "s1", conn: "42" })
  })
})

describe("dropping several files", () => {
  it("pastes the paths in the order they were dropped, not the order they finished", async () => {
    // The uploads resolve BACKWARDS, which is exactly the race the sequential
    // loop exists to prevent: a naive Promise.all would paste c, b, a.
    const order = ["a.png", "b.png", "c.png"]
    const delays: Record<string, number> = { "a.png": 30, "b.png": 20, "c.png": 0 }
    uploadDroppedFile.mockImplementation(
      (f: File) =>
        new Promise((resolve) =>
          setTimeout(() => resolve(saved(f.name)), delays[f.name]),
        ),
    )
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pane = screen.getByTestId("terminal-container").closest(".group")!
    await act(async () => {
      fireEvent.drop(pane, { dataTransfer: fileTransfer(order.map(file)) })
      await new Promise((r) => setTimeout(r, 200))
    })

    expect(TermStub.pastes).toEqual([
      "'/tmp/p1/a.png' ",
      "'/tmp/p1/b.png' ",
      "'/tmp/p1/c.png' ",
    ])
    const message = vi.mocked(toast.success).mock.calls[0][0] as string
    expect(message).toContain("3 files")
  })
})

describe("when a file cannot be pasted", () => {
  it("says it was saved but not sent, and gives its full path", async () => {
    // Ownership moves to another device between the save and the paste. The
    // file exists on the server, so the user has to be told where, by hand.
    uploadDroppedFile.mockImplementation(async (f: File) => {
      act(() => notifyPtyOwner("s1", "someone-else", undefined, undefined))
      return saved(f.name)
    })
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    act(() => FakePtySocket.instances.at(-1)!.onConnected("42"))
    await drop([file("shot.png")])

    expect(TermStub.pastes).toEqual([])
    expect(vi.mocked(toast.success)).not.toHaveBeenCalled()
    const message = vi.mocked(toast.warning).mock.calls[0][0] as string
    expect(message).toContain("not sent")
    expect(message).toContain("/tmp/p1/shot.png")
  })

  it("does not claim a paste when the socket has closed", async () => {
    // A write to a closed socket is dropped SILENTLY, so without this check the
    // file would be reported as pasted with nothing sent.
    uploadDroppedFile.mockImplementation(async (f: File) => {
      FakePtySocket.instances.at(-1)!.isOpen = false
      return saved(f.name)
    })
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    await drop([file("shot.png")])

    expect(TermStub.pastes).toEqual([])
    const message = vi.mocked(toast.warning).mock.calls[0][0] as string
    expect(message).toContain("/tmp/p1/shot.png")
    expect(message).toContain("the connection dropped")
  })

  it("reports a refusal with the server's own reason and writes nothing", async () => {
    const { FileDropApiError } = await import("@/lib/fileDropApi")
    uploadDroppedFile.mockRejectedValue(
      new FileDropApiError(
        "That file is over the 1024 byte limit for a dropped file.",
        413,
      ),
    )
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    await drop([file("big.png")])

    expect(TermStub.pastes).toEqual([])
    const message = vi.mocked(toast.error).mock.calls[0][0] as string
    expect(message).toContain("big.png")
    expect(message).toContain("over the 1024 byte limit")
  })
})

describe("the drop overlay", () => {
  it("appears while a file is over the pane and names where it will land", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pane = screen.getByTestId("terminal-container").closest(".group")!
    expect(screen.queryByTestId("file-drop-overlay")).toBeNull()
    fireEvent.dragEnter(pane, { dataTransfer: fileTransfer([]) })
    expect(screen.getByTestId("file-drop-overlay")).toBeTruthy()
    expect(screen.getByText(/worktree root/)).toBeTruthy()
    fireEvent.dragLeave(pane, { dataTransfer: fileTransfer([]) })
    expect(screen.queryByTestId("file-drop-overlay")).toBeNull()
  })

  it("says the terminal's CURRENT folder, because a shell moves", () => {
    render(
      <TerminalPane
        kind="terminal"
        id="t1"
        owner={{ kind: "session", sessionId: "s1" }}
      />,
    )
    const pane = screen.getByTestId("terminal-container").closest(".group")!
    fireEvent.dragEnter(pane, { dataTransfer: fileTransfer([]) })
    expect(screen.getByText(/currently in/)).toBeTruthy()
  })

  it("never appears for a viewer who does not hold input", async () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    act(() => FakePtySocket.instances.at(-1)!.onConnected("42"))
    act(() => notifyPtyOwner("s1", "someone-else", undefined, undefined))
    const pane = screen.getByTestId("terminal-container").closest(".group")!
    fireEvent.dragEnter(pane, { dataTransfer: fileTransfer([]) })
    expect(screen.queryByTestId("file-drop-overlay")).toBeNull()
    // And the drop itself is left entirely alone, so nothing is uploaded.
    await drop([file("shot.png")])
    expect(uploadDroppedFile).not.toHaveBeenCalled()
  })
})
