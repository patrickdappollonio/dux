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

/// A stand-in for xterm that does what xterm's `paste()` REALLY does.
///
/// Recording the argument and stopping there let a test report a successful
/// paste with no socket write at all, which is the one thing that matters: the
/// path only reaches the agent if it goes out over the PTY socket. So this
/// mirrors the installed `@xterm/xterm` implementation of `paste()` exactly
/// (`src/browser/Clipboard.ts`): normalize `\r?\n` to `\r`, bracket the text
/// when the running program asked for bracketed paste, and fire the data event
/// that the pane's `onData` handler turns into a socket write.
///
/// The newline rewriting is the part worth mirroring. A path carrying a line
/// feed does not arrive as a line feed; it arrives as a CARRIAGE RETURN, which
/// SUBMITS. A stub that only records the argument cannot see that at all.
class TermStub {
  static instances: TermStub[] = []
  /// Every `paste()` call's ARGUMENT, in order, before xterm's preparation.
  static pastes: string[] = []
  private dataHandler: ((s: string) => void) | null = null
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
  // The pane reports every local re-grid to the PTY through xterm's own resize
  // event; this suite never re-grids, so the stub only has to be subscribable.
  onResize() {
    return { dispose() {} }
  }
  onData(cb: (s: string) => void) {
    this.dataHandler = cb
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
    let out = text.replace(/\r?\n/g, "\r")
    if (this.modes.bracketedPasteMode) out = `\x1b[200~${out}\x1b[201~`
    this.dataHandler?.(out)
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
  // The real socket answers whether the frame actually went on the wire; a test
  // models a dropped frame (a socket mid-reconnect) by returning false.
  sendResize = vi.fn(() => true)
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
      // A real server always sends this, and the pane treats its ABSENCE as
      // "not known yet, so not offered". Stated here rather than left to a
      // fallback, so every other test in this file describes a dux that has
      // finished loading and has the feature switched on.
      file_drop_max_bytes: 1024,
    },
    offline: false,
    terminalEpoch: 0,
  } as unknown as DuxState
}

/// A workspace whose one agent has `sessionProvider` as its own provider and is
/// FOCUSED on a tab running `tabProvider`, with the server publishing the forms
/// it publishes in a real deployment.
///
/// The two are kept DIFFERENT by every caller, and that is the point. An earlier
/// version set both to the same value, so it would have passed unchanged if
/// resolution had regressed from the focused tab's provider to the session's. A
/// session and its focused tab genuinely differ whenever a tab is retargeted or
/// added with another provider, which is the ordinary case this has to get right.
function stateRunning(sessionProvider: string, tabProvider: string): DuxState {
  const s = makeState()
  const session = s.spine!.sessions[0]
  session.provider = sessionProvider
  session.tabs[0].provider = tabProvider
  s.bootstrap!.provider_drop_paste = {
    claude: { form: "bare", command_name: "claude" },
    codex: { form: "single_quoted", command_name: "codex" },
    opencode: { form: "bare", command_name: "opencode" },
    copilot: { form: "bare", command_name: "copilot" },
  }
  return s
}

/// A workspace focused on a tab running `tabProvider`, with the AGENT's own
/// provider set to one that wants a different form, so reading the wrong one
/// cannot produce the right bytes by luck.
function stateForFocusedTabProvider(tabProvider: string): DuxState {
  return stateRunning(tabProvider === "codex" ? "claude" : "codex", tabProvider)
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

/// Everything the pane actually WROTE TO THE SOCKET, decoded, in order.
///
/// This is the layer the assertions live at now. `term.paste()` returning
/// without throwing proves nothing: the path reaches the agent only if it goes
/// out here, and between the two sit xterm's own preparation and the pane's
/// ownership gate, both of which can swallow it.
function sentToSocket(): string[] {
  const socket = FakePtySocket.instances.at(-1)
  if (!socket) return []
  const decoder = new TextDecoder()
  return socket.sendInput.mock.calls.map(([bytes]) =>
    decoder.decode(bytes as Uint8Array),
  )
}

beforeEach(() => {
  FakePtySocket.instances = []
  TermStub.instances = []
  TermStub.pastes = []
  uploadDroppedFile.mockReset()
  vi.mocked(toast.success).mockClear()
  vi.mocked(toast.warning).mockClear()
  vi.mocked(toast.error).mockClear()
  vi.mocked(toast.loading).mockClear()
  vi.mocked(toast.dismiss).mockClear()
  mockState = makeState()
  installStubs()
  resetPtyOwnerEpochs()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("dropping a file onto an agent", () => {
  it("uploads it and the bare path reaches the socket with nothing that submits", async () => {
    uploadDroppedFile.mockResolvedValue(saved("shot.png"))
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    await drop([file("shot.png")])

    expect(uploadDroppedFile).toHaveBeenCalledTimes(1)
    expect(TermStub.pastes).toEqual(["/tmp/p1/shot.png "])
    // Asserted after xterm's real preparation and after the pane's own gate, so
    // this is the byte stream the agent would receive rather than a call that
    // was merely made. A newline SUBMITS in these tools, and xterm turns one
    // into a CARRIAGE RETURN on the way through, so both are checked here where
    // the rewriting has actually happened.
    expect(sentToSocket()).toEqual(["/tmp/p1/shot.png "])
    expect(sentToSocket()[0]).not.toContain("\n")
    expect(sentToSocket()[0]).not.toContain("\r")
    expect(vi.mocked(toast.success)).toHaveBeenCalled()
  })

  it("sends an awkward path byte for byte as it is on disk", async () => {
    // This agent runs claude, whose configured form is `bare`, so nothing is
    // added to any of these. The receiving CLI does not tokenise a pasted path
    // the way a shell does: it trims the whole string, strips ONE surrounding
    // pair of matching quotes, and unescapes backslash sequences. So quoting
    // buys nothing on an ordinary path and actively CORRUPTS one holding an
    // apostrophe, because POSIX single-quoting writes an embedded quote as
    // close-escape-reopen and that unescape step collapses it. The per-provider
    // forms themselves are exercised below.
    //
    // Asserted at the SOCKET rather than on the payload helper, so what is
    // pinned is the byte stream the agent would actually receive, after
    // xterm's own preparation and the pane's ownership gate.
    for (const path of [
      "/tmp/p1/Web App/shot.png",
      "/tmp/p1/Bob's app/shot.png",
      "/tmp/p1/$(rm -rf ~)/shot.png",
      "/tmp/p1/`whoami`/shot.png",
      '/tmp/p1/it"s a dir/shot.png',
    ]) {
      cleanup()
      FakePtySocket.instances = []
      TermStub.instances = []
      TermStub.pastes = []
      uploadDroppedFile.mockReset()
      uploadDroppedFile.mockResolvedValue({
        path,
        saved_name: "shot.png",
        requested_name: "shot.png",
        folder: "/tmp/p1",
        folder_label: "~/p1",
        renamed: false,
      })
      render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
      await drop([file("shot.png")])
      expect(sentToSocket()).toEqual([`${path} `])
    }
  })

  it("brackets the path when the running program asked for bracketed paste", async () => {
    // The pane deliberately leaves this to xterm rather than building the
    // markers itself, so the proof has to be that the markers are on the wire.
    uploadDroppedFile.mockResolvedValue(saved("shot.png"))
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    TermStub.instances.at(-1)!.modes.bracketedPasteMode = true
    await drop([file("shot.png")])

    expect(sentToSocket()).toEqual([
      "\x1b[200~/tmp/p1/shot.png \x1b[201~",
    ])
  })

  it("tells the user the new name when the file was renamed", async () => {
    uploadDroppedFile.mockResolvedValue(saved("shot.png", "shot-S-1.png"))
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    await drop([file("shot.png")])

    const message = vi.mocked(toast.success).mock.calls[0][0] as string
    expect(message).toContain("shot.png")
    expect(message).toContain("shot-S-1.png")
    expect(sentToSocket()).toEqual(["/tmp/p1/shot-S-1.png "])
  })

  it("carries the terminal socket's own connection id, not the events one", async () => {
    uploadDroppedFile.mockResolvedValue(saved("shot.png"))
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    act(() => FakePtySocket.instances.at(-1)!.onConnected("42"))
    await drop([file("shot.png")])
    expect(uploadDroppedFile.mock.calls[0][1]).toEqual({ pty: "s1", conn: "42" })
  })
})

describe("the paste form follows the provider running in the pane", () => {
  // The whole point of making this a setting: the same dropped file, on two
  // panes running two different CLIs, has to leave the browser in two different
  // shapes. Both halves are asserted at the SOCKET, because a call to
  // `term.paste` proves nothing about what the agent receives.

  /// Drops onto a pane whose FOCUSED TAB runs `provider`, inside an agent whose
  /// own provider is deliberately something else.
  async function dropOnAgentRunning(provider: string, path: string) {
    cleanup()
    FakePtySocket.instances = []
    TermStub.instances = []
    TermStub.pastes = []
    uploadDroppedFile.mockReset()
    uploadDroppedFile.mockResolvedValue({
      path,
      saved_name: "shot.png",
      requested_name: "shot.png",
      folder: "/tmp/p1",
      folder_label: "~/p1",
      renamed: false,
    })
    mockState = stateForFocusedTabProvider(provider)
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    await drop([file("shot.png")])
    return sentToSocket()
  }

  it("sends the BARE path to a pane running claude", async () => {
    // Measured: Claude Code takes the whole pasted string and never splits on a
    // space, and its own unescape step corrupts a quoted apostrophe. So nothing
    // is added, not even for a path that a shell would need protected.
    expect(await dropOnAgentRunning("claude", "/tmp/p1/Web App/shot.png")).toEqual([
      "/tmp/p1/Web App/shot.png ",
    ])
    expect(await dropOnAgentRunning("claude", "/tmp/p1/Bob's app/shot.png")).toEqual([
      "/tmp/p1/Bob's app/shot.png ",
    ])
  })

  it("sends the SINGLE-QUOTED path to a pane running codex", async () => {
    // Measured: Codex lexes the pasted text with POSIX shell rules and accepts
    // it only if it comes out as exactly one token, so the bare form above is
    // silently ignored for a path with a space in it.
    expect(await dropOnAgentRunning("codex", "/tmp/p1/Web App/shot.png")).toEqual([
      "'/tmp/p1/Web App/shot.png' ",
    ])
    expect(await dropOnAgentRunning("codex", "/tmp/p1/Bob's app/shot.png")).toEqual([
      "'/tmp/p1/Bob'\\''s app/shot.png' ",
    ])
  })

  it("sends the bare path for a provider the server published no form for", async () => {
    // A provider the user added themselves. Bare is the do-nothing option.
    expect(
      await dropOnAgentRunning("myagent", "/tmp/p1/Web App/shot.png"),
    ).toEqual(["/tmp/p1/Web App/shot.png "])
  })

  /// Drops onto a companion terminal owned by an agent whose provider is
  /// `ownerProvider`.
  async function dropOnTerminalOwnedBy(ownerProvider: string, path: string) {
    cleanup()
    FakePtySocket.instances = []
    TermStub.instances = []
    TermStub.pastes = []
    uploadDroppedFile.mockReset()
    uploadDroppedFile.mockResolvedValue({
      path,
      saved_name: "shot.png",
      requested_name: "shot.png",
      folder: "/tmp/p1",
      folder_label: "~/p1",
      renamed: false,
    })
    mockState = stateRunning(ownerProvider, ownerProvider)
    render(
      <TerminalPane
        kind="terminal"
        id="t1"
        owner={{ kind: "session", session_id: "s1" }}
      />,
    )
    await drop([file("shot.png")])
    return sentToSocket()
  }

  it("gives a TERMINAL the shell-safe path, whatever its owning agent runs", async () => {
    // This test used to pin the opposite, and pinning it was the whole problem:
    // a terminal got the path BARE, on the reasoning that it runs a shell rather
    // than that CLI. That reasoning is inverted. Running a shell is exactly why
    // the path must be quoted, because the shell is what would split a space,
    // expand a `$` and RUN a command substitution the moment the user presses
    // Enter on the line the path was pasted into.
    //
    // So the answer is `single_quoted` for both owners below, and it does not
    // move when the owning agent's provider does, because the terminal branch
    // never reads that setting at all.
    for (const owner of ["claude", "codex"]) {
      expect(
        await dropOnTerminalOwnedBy(owner, "/tmp/p1/Web App/shot.png"),
      ).toEqual(["'/tmp/p1/Web App/shot.png' "])
    }
  })

  it("makes a hostile path inert in a TERMINAL: a dollar, a backtick and a space", async () => {
    // The case the bare form shipped wrong. Bare, this line reads as three
    // words, one of them a command substitution that DELETES A DIRECTORY and
    // one a substitution that runs `whoami`, and the shell would act on all of
    // it. Quoted, the whole thing is one literal argument.
    const path = "/tmp/p1/$(rm -rf ~) `whoami`/shot.png"
    expect(await dropOnTerminalOwnedBy("claude", path)).toEqual([
      "'/tmp/p1/$(rm -rf ~) `whoami`/shot.png' ",
    ])
  })

  it("keeps a terminal path with an apostrophe one word", async () => {
    // The one character single-quoting has to work for, closed and reopened
    // around, so the payload still lexes as a single argument.
    expect(
      await dropOnTerminalOwnedBy("claude", "/tmp/p1/Bob's app/shot.png"),
    ).toEqual(["'/tmp/p1/Bob'\\''s app/shot.png' "])
  })

  it("sends a very long path to a TERMINAL, because a shell has no such limit", async () => {
    // The paste-length limit used to be keyed by FORM, and a terminal always
    // uses the shell-safe form, so a terminal inherited codex's composer limit.
    // dux withheld a perfectly good path from a shell and told the user it was
    // too long for "this agent", which is not even what it was talking to.
    //
    // 2000 characters, comfortably past codex's threshold, and it goes out.
    const longPath = `/tmp/p1/${"a".repeat(1_992)}`
    expect(longPath.length).toBe(2000)
    vi.mocked(toast.warning).mockClear()
    expect(await dropOnTerminalOwnedBy("codex", longPath)).toEqual([
      `'${longPath}' `,
    ])
    expect(vi.mocked(toast.warning)).not.toHaveBeenCalled()
  })

  it("holds a long path back from codex on EVERY form it can be configured with", async () => {
    // The other direction of the same mistake. `bare`, `double_quoted` and
    // `backslash_escaped` are all valid choices for codex, and with the limit
    // keyed by form they escaped it entirely: dux sent an over-limit payload
    // codex silently ignores while the toast claimed the file was attached.
    for (const form of ["bare", "double_quoted", "backslash_escaped"]) {
      cleanup()
      FakePtySocket.instances = []
      TermStub.instances = []
      TermStub.pastes = []
      uploadDroppedFile.mockReset()
      vi.mocked(toast.warning).mockClear()
      const longPath = `/tmp/p1/${"a".repeat(1_992)}`
      uploadDroppedFile.mockResolvedValue({
        path: longPath,
        saved_name: "shot.png",
        requested_name: "shot.png",
        folder: "/tmp/p1",
        folder_label: "~/p1",
        renamed: false,
      })
      mockState = stateForFocusedTabProvider("codex")
      mockState.bootstrap!.provider_drop_paste = {
        codex: { form, command_name: "codex" },
      }
      render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
      await drop([file("shot.png")])
      expect(sentToSocket(), `codex configured as ${form}`).toEqual([])
      const message = vi.mocked(toast.warning).mock.calls[0][0] as string
      expect(message).toContain("1000 characters")
    }
  })
})

describe("which CLI a pane is actually talking to", () => {
  // A provider's BLOCK NAME is free text the user picks; the `command` is what
  // says which CLI runs. The measured paste-length limit used to be keyed by the
  // name, so it answered for the wrong tool in both directions: a real Codex
  // under any other name got no limit and was handed oversized paths it silently
  // ignores, and an unrelated CLI merely NAMED codex had valid long paths
  // withheld from it. Both directions are asserted on the bytes.

  const longPath = `/tmp/p1/${"a".repeat(1_992)}`

  async function dropLongPathOn(provider: string, command_name: string) {
    cleanup()
    FakePtySocket.instances = []
    TermStub.instances = []
    TermStub.pastes = []
    uploadDroppedFile.mockReset()
    vi.mocked(toast.warning).mockClear()
    uploadDroppedFile.mockResolvedValue({
      path: longPath,
      saved_name: "shot.png",
      requested_name: "shot.png",
      folder: "/tmp/p1",
      folder_label: "~/p1",
      renamed: false,
    })
    mockState = stateRunning("claude", provider)
    mockState.bootstrap!.provider_drop_paste = {
      [provider]: { form: "bare", command_name },
    }
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    await drop([file("shot.png")])
    return sentToSocket()
  }

  it("holds a long path back from a real codex running under another name", async () => {
    // `[providers.myagent] command = "codex"` IS codex, and codex files any
    // paste over 1000 characters away as generic large content before it looks
    // for a path at all. Sending this would put a placeholder in the prompt,
    // attach nothing, and report success.
    expect(longPath.length).toBe(2000)
    expect(await dropLongPathOn("myagent", "codex")).toEqual([])
    const message = vi.mocked(toast.warning).mock.calls[0][0] as string
    expect(message).toContain("1000 characters")
  })

  it("sends a long path to a different CLI that merely happens to be named codex", async () => {
    // `[providers.codex] command = "something-else"` is NOT codex, and nothing
    // has been measured about it, so withholding the file would be dux refusing
    // on a guess.
    expect(await dropLongPathOn("codex", "something-else")).toEqual([
      `${longPath} `,
    ])
    expect(vi.mocked(toast.warning)).not.toHaveBeenCalled()
  })
})

describe("two live tabs of one provider that launched with different forms", () => {
  // The case a map keyed by provider NAME cannot answer, and the reason the
  // server publishes the launched forms keyed by TAB ID.
  //
  // Launch a codex tab, edit `[providers.codex] web_dragdrop_paste`, launch a
  // second codex tab. Both processes are live, both report the provider name
  // `codex`, and each needs the form it started with. Folded onto one provider
  // key there is one slot for two answers, so one of the two panes got the
  // other's form, and which one depended on server-side map iteration order.

  /// Two live codex tabs whose LAUNCHED forms differ, over a current config
  /// value that matches NEITHER, so a pane reading the provider map instead of
  /// its own tab shows up in the bytes rather than passing by luck.
  function stateWithTwoLaunchedTabs(): DuxState {
    const s = stateRunning("codex", "codex")
    const session = s.spine!.sessions[0]
    session.tabs = [
      { ...session.tabs[0], id: "s1", provider: "codex" },
      { ...session.tabs[0], id: "tab-b", provider: "codex" },
    ]
    s.bootstrap!.provider_drop_paste = {
      codex: { form: "bare", command_name: "codex" },
    }
    session.tabs[0].drop_paste = { form: "single_quoted", command_name: "codex" }
    session.tabs[1].drop_paste = {
      form: "backslash_escaped",
      command_name: "codex",
    }
    return s
  }

  async function dropOnTab(tabId: string, path: string) {
    cleanup()
    FakePtySocket.instances = []
    TermStub.instances = []
    TermStub.pastes = []
    uploadDroppedFile.mockReset()
    uploadDroppedFile.mockResolvedValue({
      path,
      saved_name: "shot.png",
      requested_name: "shot.png",
      folder: "/tmp/p1",
      folder_label: "~/p1",
      renamed: false,
    })
    mockState = stateWithTwoLaunchedTabs()
    render(<TerminalPane kind="agent" id={tabId} sessionId="s1" />)
    await drop([file("shot.png")])
    return sentToSocket()
  }

  it("gives each pane the form its OWN tab launched with", async () => {
    const path = "/tmp/p1/Web App/shot.png"
    expect(await dropOnTab("s1", path)).toEqual(["'/tmp/p1/Web App/shot.png' "])
    expect(await dropOnTab("tab-b", path)).toEqual([
      "/tmp/p1/Web\\ App/shot.png ",
    ])
    // And neither pane got the CURRENT config value, which is the single answer
    // a provider-keyed map would have handed both of them.
    expect(await dropOnTab("s1", path)).not.toEqual([`${path} `])
  })
})

describe("a tab whose live process is replaced under the pane", () => {
  // THE STALENESS DEFECT, end to end, in the bytes.
  //
  // What a tab launched with used to ride the BOOTSTRAP document, which the
  // browser refetches only on `config.changed`. A launch and a termination
  // refresh the SPINE (`sessions.changed`) instead, so a client that had
  // refetched config before a relaunch kept resolving the OLD entry for the
  // whole life of the new process: nothing but a reconnect or a restart
  // corrected it. Both sequences below therefore START from a stale copy and
  // are corrected by the relaunch, rather than being handed a correct one.

  /// One drop against whatever `mockState` currently says, returning the bytes.
  async function dropAgain(path: string) {
    cleanup()
    FakePtySocket.instances = []
    TermStub.instances = []
    TermStub.pastes = []
    uploadDroppedFile.mockReset()
    vi.mocked(toast.warning).mockClear()
    uploadDroppedFile.mockResolvedValue({
      path,
      saved_name: "shot.png",
      requested_name: "shot.png",
      folder: "/tmp/p1",
      folder_label: "~/p1",
      renamed: false,
    })
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    await drop([file("shot.png")])
    return sentToSocket()
  }

  const path = "/tmp/p1/Web App/shot.png"

  it("keeps the running form after a config edit, then takes the new one on relaunch", async () => {
    // SEQUENCE ONE. A live codex tab launched single-quoted. The user edits
    // `[providers.codex] web_dragdrop_paste` to bare and a config refetch lands,
    // so the pane's configured map now disagrees with the running process.
    mockState = stateRunning("codex", "codex")
    mockState.bootstrap!.provider_drop_paste = {
      codex: { form: "bare", command_name: "codex" },
    }
    mockState.spine!.sessions[0].tabs[0].drop_paste = {
      form: "single_quoted",
      command_name: "codex",
    }
    // The running process still wants what it started with, and gets it.
    expect(await dropAgain(path)).toEqual(["'/tmp/p1/Web App/shot.png' "])

    // The relaunch. It arrives on the spine, on the tab, so the pane sees it.
    mockState = stateRunning("codex", "codex")
    mockState.bootstrap!.provider_drop_paste = {
      codex: { form: "bare", command_name: "codex" },
    }
    mockState.spine!.sessions[0].tabs[0].drop_paste = {
      form: "bare",
      command_name: "codex",
    }
    expect(await dropAgain(path)).toEqual(["/tmp/p1/Web App/shot.png "])
  })

  it("drops the dead process's form when the tab goes dormant and relaunches elsewhere", async () => {
    // SEQUENCE TWO. The codex process exits, the user retargets the tab to
    // claude, and it relaunches. The stale codex entry must survive neither
    // step: it must not outlive the process, and it must not win over the
    // claude one that replaces it.
    //
    // At every step the CONFIGURED map is set to disagree with the answer being
    // asserted, so a pane that ignored the tab and read config would produce
    // different bytes rather than passing by luck.
    const config = {
      codex: { form: "bare", command_name: "codex" },
      claude: { form: "backslash_escaped", command_name: "claude" },
    }
    mockState = stateRunning("codex", "codex")
    mockState.bootstrap!.provider_drop_paste = config
    mockState.spine!.sessions[0].tabs[0].drop_paste = {
      form: "single_quoted",
      command_name: "codex",
    }
    expect(await dropAgain(path)).toEqual(["'/tmp/p1/Web App/shot.png' "])

    // Dormant, and retargeted. No live process, so the pane falls back to what
    // config says this tab will launch with, which is now claude's form. The
    // dead codex process's single-quoting must not survive here.
    mockState = stateRunning("codex", "claude")
    mockState.bootstrap!.provider_drop_paste = config
    mockState.spine!.sessions[0].tabs[0].has_live_process = false
    expect(await dropAgain(path)).toEqual(["/tmp/p1/Web\\ App/shot.png "])

    // Relaunched as claude, which launched BARE even though config now says
    // otherwise. And the codex length limit goes with the codex process: a
    // 2000-character path codex would have had withheld goes out, because
    // claude is what is reading it.
    const longPath = `/tmp/p1/${"a".repeat(1_992)}`
    expect(longPath.length).toBe(2000)
    mockState = stateRunning("codex", "claude")
    mockState.bootstrap!.provider_drop_paste = config
    mockState.spine!.sessions[0].tabs[0].drop_paste = {
      form: "bare",
      command_name: "claude",
    }
    expect(await dropAgain(longPath)).toEqual([`${longPath} `])
    expect(vi.mocked(toast.warning)).not.toHaveBeenCalled()
  })
})

describe("dropping several files", () => {
  it("finishes each upload and sends its path before the next one starts", async () => {
    // What the code actually does, stated as the thing it is. The earlier
    // version handed the uploads different timings and asserted the pastes came
    // out in order, which cannot fail: the loop awaits each upload before
    // starting the next, so no two are ever in flight and the timings never
    // interleave anything. It would have passed against a broken implementation
    // that merely happened to be fast.
    //
    // The real guarantee is SEQUENCING, so that is what is pinned: upload N+1
    // is not started until upload N has resolved AND its path has gone out. The
    // uploads are held open one at a time and released by hand, so nothing is
    // raced for and the ordering is observed rather than hoped for.
    const gates: ((v: unknown) => void)[] = []
    const started: string[] = []
    uploadDroppedFile.mockImplementation(
      (f: File) =>
        new Promise((resolve) => {
          started.push(f.name)
          gates.push(() => resolve(saved(f.name)))
        }),
    )

    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pane = screen.getByTestId("terminal-container").closest(".group")!
    await act(async () => {
      fireEvent.drop(pane, {
        dataTransfer: fileTransfer(["a.png", "b.png", "c.png"].map(file)),
      })
      await Promise.resolve()
    })

    // Only the first has even been ASKED for. A parallel implementation would
    // have all three here.
    expect(started).toEqual(["a.png"])
    expect(sentToSocket()).toEqual([])

    for (const [i, name] of ["a.png", "b.png", "c.png"].entries()) {
      await act(async () => {
        gates[i]()
        await Promise.resolve()
        await Promise.resolve()
      })
      // Each release sends exactly that file's path, and only then is the next
      // upload started.
      expect(sentToSocket()).toEqual(
        ["a.png", "b.png", "c.png"]
          .slice(0, i + 1)
          .map((n) => `/tmp/p1/${n} `),
      )
      expect(started).toEqual(
        ["a.png", "b.png", "c.png"].slice(0, Math.min(i + 2, 3)),
      )
      expect(name).toBe(started[i])
    }

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
    expect(sentToSocket()).toEqual([])
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
    expect(sentToSocket()).toEqual([])
    const message = vi.mocked(toast.warning).mock.calls[0][0] as string
    expect(message).toContain("/tmp/p1/shot.png")
    expect(message).toContain("the connection dropped")
  })

  it("holds back a path too long for the CLI to read as one, and says so", async () => {
    // Codex classifies any paste over 1000 characters as generic large content
    // BEFORE it looks for a path, so a long enough path is never attached
    // however well it is quoted. Sending it anyway would put a placeholder in
    // the prompt and let the toast claim a success that did not happen.
    //
    // The path here is 998 characters, comfortably under the limit; the quotes
    // and the trailing space take the PAYLOAD to 1001, which is over. That is
    // the boundary the check has to measure, and it is why counting the file's
    // own path would miss this.
    const longPath = `/tmp/p1/${"a".repeat(990)}`
    expect(longPath.length).toBe(998)
    uploadDroppedFile.mockResolvedValue({
      path: longPath,
      saved_name: "shot.png",
      requested_name: "shot.png",
      folder: "/tmp/p1",
      folder_label: "~/p1",
      renamed: false,
    })
    mockState = stateForFocusedTabProvider("codex")
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    await drop([file("shot.png")])

    expect(TermStub.pastes).toEqual([])
    expect(sentToSocket()).toEqual([])
    expect(vi.mocked(toast.success)).not.toHaveBeenCalled()
    const message = vi.mocked(toast.warning).mock.calls[0][0] as string
    expect(message).toContain("not sent")
    expect(message).toContain("1000 characters")
    expect(message).toContain(longPath)
  })

  it("still sends a path that only just fits", async () => {
    // One character shorter, so the payload is exactly 1000 and the CLI still
    // looks at it. The refusal must not creep inward: a file dux could have
    // attached and did not is a regression in the other direction.
    const longPath = `/tmp/p1/${"a".repeat(989)}`
    uploadDroppedFile.mockResolvedValue({
      path: longPath,
      saved_name: "shot.png",
      requested_name: "shot.png",
      folder: "/tmp/p1",
      folder_label: "~/p1",
      renamed: false,
    })
    mockState = stateForFocusedTabProvider("codex")
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    await drop([file("shot.png")])

    expect(sentToSocket()).toEqual([`'${longPath}' `])
    expect(sentToSocket()[0]).toHaveLength(1000)
    expect(vi.mocked(toast.success)).toHaveBeenCalled()
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
    expect(sentToSocket()).toEqual([])
    const message = vi.mocked(toast.error).mock.calls[0][0] as string
    expect(message).toContain("big.png")
    expect(message).toContain("over the 1024 byte limit")
  })

  it("says a busy server is temporary, which a 413 does not", async () => {
    // The status was carried and never read, so this refusal used to be worded
    // exactly like the permanent one above.
    const { FileDropApiError } = await import("@/lib/fileDropApi")
    uploadDroppedFile.mockRejectedValue(
      new FileDropApiError(
        "The server is already handling as many dropped files as it allows at once. Try the drop again shortly.",
        503,
      ),
    )
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    await drop([file("shot.png")])

    const message = vi.mocked(toast.error).mock.calls[0][0] as string
    expect(message).toContain("shot.png")
    // The server's own words carry the advice, so dux does not add a second
    // copy of it. The whole sentence is asserted, because the bug this replaces
    // was a wording defect that every looser assertion passed: dux used to weld
    // ", so it was not saved; try the drop again in a moment" onto the end of
    // the server's already-finished sentence.
    expect(message).toBe(
      "Could not save shot.png: The server is already handling as many dropped " +
        "files as it allows at once. Try the drop again shortly.",
    )
    expect(message.toLowerCase().match(/\btry\b[^.!?]*\bagain\b/g)).toHaveLength(
      1,
    )
    expect(message).not.toContain("..")
  })
})

// The drop overlay is cleared the instant the browser hands the files over, so
// between then and the report there was nothing on screen at all. The permit
// wait is bounded but real, uploads are sequential, and a multi-file drop
// multiplies it, so a drop could look like nothing had happened for a long
// time. CLAUDE.md: prefer explicit failure over silent waiting.
describe("what the user sees while a drop is uploading", () => {
  /// An upload the test releases by hand, so the in-flight window is a state
  /// the assertions can stand in rather than a race to catch.
  function heldUpload() {
    let release: (v: unknown) => void = () => {}
    const held = new Promise((resolve) => {
      release = resolve
    })
    return { held, release: () => release(undefined) }
  }

  it("raises a spinner naming the file before the request is even answered", async () => {
    const { held, release } = heldUpload()
    uploadDroppedFile.mockImplementation(async () => {
      await held
      return saved("shot.png")
    })
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)

    const pane = screen.getByTestId("terminal-container").closest(".group")!
    await act(async () => {
      fireEvent.drop(pane, { dataTransfer: fileTransfer([file("shot.png")]) })
      await Promise.resolve()
    })

    // In flight: the spinner is up and nothing final has been said yet.
    expect(vi.mocked(toast.loading)).toHaveBeenCalled()
    const busy = vi.mocked(toast.loading).mock.calls[0]
    expect(busy[0] as string).toContain("shot.png")
    expect(vi.mocked(toast.success)).not.toHaveBeenCalled()

    await act(async () => {
      release()
      await Promise.resolve()
      await Promise.resolve()
      await Promise.resolve()
      await Promise.resolve()
    })
    expect(vi.mocked(toast.success)).toHaveBeenCalled()
  })

  it("puts the spinner and the report on ONE id, so the final replaces it", async () => {
    // Otherwise the spinner sits under the report claiming the upload is still
    // running, since sonner never retires a loading toast on its own.
    uploadDroppedFile.mockResolvedValue(saved("shot.png"))
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    await drop([file("shot.png")])

    const busyId = (
      vi.mocked(toast.loading).mock.calls[0][1] as { id: string }
    ).id
    const finalId = (
      vi.mocked(toast.success).mock.calls[0][1] as { id: string }
    ).id
    expect(busyId).toBe(finalId)
  })

  it("counts through a multi-file drop rather than sitting on one message", async () => {
    // Uploads are sequential, so the wait multiplies. A single unchanging
    // message cannot be told apart from a stuck one.
    uploadDroppedFile.mockImplementation(async (f: File) => saved(f.name))
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    await drop([file("a.png"), file("b.png"), file("c.png")])

    const messages = vi
      .mocked(toast.loading)
      .mock.calls.map((c) => c[0] as string)
    expect(messages).toHaveLength(3)
    expect(messages[0]).toContain("a.png")
    expect(messages[0]).toContain("1 of 3")
    expect(messages[2]).toContain("c.png")
    expect(messages[2]).toContain("3 of 3")
  })

  it("still ends in a final toast when something throws unexpectedly", async () => {
    // handleDroppedFiles is called with `void`, so an unexpected throw would
    // otherwise be an unhandled rejection with the spinner left on screen
    // claiming the upload is still running.
    uploadDroppedFile.mockResolvedValue(saved("shot.png"))
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const term = TermStub.instances.at(-1)!
    term.paste = () => {
      throw new Error("xterm blew up")
    }
    await drop([file("shot.png")])

    expect(vi.mocked(toast.loading)).toHaveBeenCalled()
    const message = vi.mocked(toast.error).mock.calls[0][0] as string
    expect(message).toContain("xterm blew up")
    const finalId = (
      vi.mocked(toast.error).mock.calls[0][1] as { id: string }
    ).id
    expect(finalId).toBe(
      (vi.mocked(toast.loading).mock.calls[0][1] as { id: string }).id,
    )
  })
})

describe("the drop overlay", () => {
  it("appears while a file is over the pane and names where it will land", () => {
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pane = screen.getByTestId("terminal-container").closest(".group")!
    expect(screen.queryByTestId("file-drop-overlay")).toBeNull()
    fireEvent.dragEnter(pane, { dataTransfer: fileTransfer([]) })
    expect(screen.getByTestId("file-drop-overlay")).toBeTruthy()
    // The upload folder, and the two things about it the user needs to know:
    // git does not see it, and it goes when the agent goes. "Worktree root" was
    // the OLD destination and its "where git can see it" tail is now the exact
    // opposite of what happens.
    expect(screen.getByText(/upload folder/)).toBeTruthy()
    expect(screen.queryByText(/worktree root/)).toBeNull()
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

  it("offers nothing at all when file drop is switched off", async () => {
    // `[server] file_drop_max_bytes = 0` is documented as switching the feature
    // off, and the server refuses every upload when it is. The browser's gate
    // checked ownership, whether this is a phone, and what was being dragged,
    // but never whether the feature existed, so a disabled dux still advertised
    // a drop target, still accepted the drop, and only then produced a toast
    // full of the server's refusals. Recorded before the fix: the overlay
    // rendered and uploadDroppedFile was called once per file.
    mockState = makeState()
    mockState.bootstrap!.file_drop_max_bytes = 0
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pane = screen.getByTestId("terminal-container").closest(".group")!
    fireEvent.dragEnter(pane, { dataTransfer: fileTransfer([]) })
    expect(screen.queryByTestId("file-drop-overlay")).toBeNull()
    await drop([file("shot.png")])
    expect(uploadDroppedFile).not.toHaveBeenCalled()
    // And nothing is said about it: a feature that is off has no failure to
    // report, because nothing was attempted.
    expect(vi.mocked(toast.error)).not.toHaveBeenCalled()
    expect(vi.mocked(toast.warning)).not.toHaveBeenCalled()
  })

  it("offers nothing while the setting is not known yet", async () => {
    // Bootstrap and the workspace load in parallel, so there is a real window
    // in which the pane is on screen and dux does not yet know whether file
    // drop exists. The gate defaulted that window to ENABLED, so a drag landing
    // in it advertised a drop target and uploaded, even for a dux with the
    // feature switched off. The previous test could not see this because it
    // pre-seeded the setting. Recorded before the fix: the overlay rendered and
    // uploadDroppedFile was called once.
    uploadDroppedFile.mockResolvedValue(saved("shot.png"))
    mockState = makeState()
    mockState.bootstrap = null
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pane = screen.getByTestId("terminal-container").closest(".group")!
    fireEvent.dragEnter(pane, { dataTransfer: fileTransfer([]) })
    expect(screen.queryByTestId("file-drop-overlay")).toBeNull()
    await drop([file("shot.png")])
    expect(uploadDroppedFile).not.toHaveBeenCalled()
  })

  it("takes the overlay back if the setting arrives disabled mid-drag", () => {
    // The other half of the same window: the drag starts while the answer is
    // unknown or the feature is on, and the bootstrap document lands saying it
    // is off. The gate then refuses the matching dragleave and drop, so nothing
    // is left that could clear the overlay and it would sit on the pane until
    // it unmounted. Recorded before the fix: the overlay was still in the
    // document after the setting arrived.
    mockState = makeState()
    mockState.bootstrap!.file_drop_max_bytes = 1024
    const { rerender } = render(
      <TerminalPane kind="agent" id="s1" sessionId="s1" />,
    )
    const pane = screen.getByTestId("terminal-container").closest(".group")!
    fireEvent.dragEnter(pane, { dataTransfer: fileTransfer([]) })
    expect(screen.getByTestId("file-drop-overlay")).toBeTruthy()

    mockState = makeState()
    mockState.bootstrap!.file_drop_max_bytes = 0
    act(() => {
      rerender(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    })
    expect(screen.queryByTestId("file-drop-overlay")).toBeNull()

    // And switching the feature back on must not revive the retired drag. The
    // drag that was in flight ended while the feature was off, so no
    // dragleave/drop is ever coming for it, and a revived overlay would sit on
    // the pane until the user started an unrelated drag over it.
    mockState = makeState()
    mockState.bootstrap!.file_drop_max_bytes = 1024
    act(() => {
      rerender(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    })
    expect(screen.queryByTestId("file-drop-overlay")).toBeNull()

    // The pane is still usable: a fresh drag opens and closes the overlay with
    // a single enter/leave pair, proving the depth counter went back to zero
    // rather than keeping the count from the drag that was retired.
    fireEvent.dragEnter(pane, { dataTransfer: fileTransfer([]) })
    expect(screen.getByTestId("file-drop-overlay")).toBeTruthy()
    fireEvent.dragLeave(pane, { dataTransfer: fileTransfer([]) })
    expect(screen.queryByTestId("file-drop-overlay")).toBeNull()
  })

  it("offers both the overlay and the upload when file drop is on", async () => {
    // The other half, so the gate cannot be satisfied by refusing everything.
    uploadDroppedFile.mockResolvedValue(saved("shot.png"))
    mockState = makeState()
    mockState.bootstrap!.file_drop_max_bytes = 1024
    render(<TerminalPane kind="agent" id="s1" sessionId="s1" />)
    const pane = screen.getByTestId("terminal-container").closest(".group")!
    fireEvent.dragEnter(pane, { dataTransfer: fileTransfer([]) })
    expect(screen.getByTestId("file-drop-overlay")).toBeTruthy()
    fireEvent.dragLeave(pane, { dataTransfer: fileTransfer([]) })
    await drop([file("shot.png")])
    expect(uploadDroppedFile).toHaveBeenCalledTimes(1)
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
