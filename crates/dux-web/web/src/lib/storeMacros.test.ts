import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// Mock sonner so the store's `import { toast } from "sonner"` picks up spies; the
// saveMacros null-bootstrap guard surfaces an error toast we assert on.
vi.mock("sonner", () => {
  const toast = Object.assign(vi.fn(), {
    success: vi.fn(),
    error: vi.fn(),
    warning: vi.fn(),
    loading: vi.fn(),
    dismiss: vi.fn(),
  })
  return { toast }
})

import type { MacroView } from "./types"
import type { Bootstrap } from "./bootstrapApi"
import { macroPayloadBytes } from "./macros"
import type { PtySocket } from "./ptySocket"

// `store` reads `location`/`localStorage`, registers a `popstate` listener, and
// at module load fires a boot `/api/me` fetch + constructs an `EventsSocket`. Stub
// the minimum so the import succeeds; steer the boot probe to auth-off so the
// store settles cleanly (mirrors storeAuth.test.ts's setup).
//
// Macros moved off the broadcast ViewModel onto the `GET /api/v1/bootstrap`
// document (Phase 2), so the dialog seeds from `state.bootstrap.macros`. The
// boot path fetches it after auth resolves; tests control its body via the
// `bootstrapMacros` variable the fetch double reads at call time.

let bootstrapMacros: MacroView[] = []
// When set, the bootstrap fetch fails so `state.bootstrap` stays null — the
// window the saveMacros guard protects against.
let failBootstrap = false

function makeBootstrap(macros: MacroView[]): Bootstrap {
  return {
    available_providers: [],
    macros,
    palette_commands: [],
    welcome_tips: [],
    dux_version: "development",
    randomize_agent_names_by_default: false,
    gh_available: false,
    pr_banner_position: "top",
    agent_scrollback_lines: 10000,
    show_changes_pane: true,
    global_env: {},
    status_clear_seconds: 6,
  }
}

const fetchMock = vi.fn(async (url: string) => {
  const u = String(url)
  if (u.includes("/api/v1/bootstrap")) {
    if (failBootstrap) {
      return {
        ok: false,
        status: 500,
        json: async () => null,
        text: async () => "bootstrap unavailable",
        headers: { get: () => null },
      } as unknown as Response
    }
    return {
      ok: true,
      status: 200,
      json: async () => makeBootstrap(bootstrapMacros),
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  // The macro editor now persists via a REST PUT (Phase 6, was a `/ws` command).
  if (u.includes("/api/v1/macros")) {
    return {
      ok: true,
      status: 204,
      json: async () => null,
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  // /api/me (and anything else): auth off.
  return {
    ok: true,
    status: 200,
    json: async () => ({ auth: "disabled" }),
    text: async () => "",
    headers: { get: () => null },
  } as unknown as Response
})

class FakeWebSocket {
  onopen: (() => void) | null = null
  onclose: (() => void) | null = null
  onerror: (() => void) | null = null
  onmessage: (() => void) | null = null
  binaryType = ""
  readyState = 1
  close() {}
  send() {}
}

beforeEach(() => {
  bootstrapMacros = []
  failBootstrap = false
  vi.stubGlobal("location", { host: "localhost:0" })
  vi.stubGlobal("localStorage", {
    getItem: () => null,
    setItem: () => {},
    removeItem: () => {},
  })
  vi.stubGlobal("window", { addEventListener: () => {} })
  vi.stubGlobal("WebSocket", FakeWebSocket)
  vi.stubGlobal("fetch", fetchMock)
  vi.resetModules()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

// Load the store and wait for the boot probe AND the bootstrap fetch to settle,
// so `state.bootstrap.macros` is the `bootstrapMacros` set by the test.
async function loadStore() {
  const mod = await import("./store")
  await vi.waitFor(() => {
    expect(mod.getSnapshot().auth.phase).not.toBe("checking")
    expect(mod.getSnapshot().bootstrap).not.toBeNull()
  })
  return mod
}

// Load the store with the bootstrap fetch failing, so `state.bootstrap` stays
// null (consumers fall back to defaults). Used to exercise the saveMacros guard.
async function loadStoreNoBootstrap() {
  const mod = await import("./store")
  await vi.waitFor(() => {
    expect(mod.getSnapshot().auth.phase).not.toBe("checking")
  })
  expect(mod.getSnapshot().bootstrap).toBeNull()
  return mod
}

describe("store macros dialog", () => {
  const seed: MacroView[] = [
    { name: "Review", text: "review this", surface: "agent" },
    { name: "Build", text: "cargo build", surface: "terminal" },
  ]

  it("openMacrosDialog seeds the draft from the bootstrap macros (copied)", async () => {
    bootstrapMacros = seed
    const mod = await loadStore()

    mod.openMacrosDialog()
    const snap = mod.getSnapshot()
    expect(snap.macrosDialogOpen).toBe(true)
    expect(snap.macrosDraft).toEqual(seed)
    // The draft is a copy, not the same array/objects as the bootstrap slice.
    expect(snap.macrosDraft).not.toBe(snap.bootstrap?.macros)
    expect(snap.macrosDraft[0]).not.toBe(seed[0])
  })

  it("openMacrosDialog seeds an empty draft when there are no macros", async () => {
    bootstrapMacros = []
    const mod = await loadStore()
    mod.openMacrosDialog()
    expect(mod.getSnapshot().macrosDraft).toEqual([])
  })

  it("closeMacrosDialog clears the draft", async () => {
    bootstrapMacros = seed
    const mod = await loadStore()
    mod.openMacrosDialog()
    mod.closeMacrosDialog()
    const snap = mod.getSnapshot()
    expect(snap.macrosDialogOpen).toBe(false)
    expect(snap.macrosDraft).toEqual([])
  })
})

describe("store macros commands", () => {
  // A minimal PtySocket double that records the stdin written to it. Phase 5
  // routes a macro straight to the focused PTY socket as stdin (no server-side
  // `run_macro` command), so the store resolves the macro text from bootstrap,
  // applies the newline transform, and calls `sendInput` on the active socket.
  // `setActivePtySocket` is pulled from the SAME module graph the dynamically
  // imported store uses (vi.resetModules() in beforeEach forks a fresh graph), so
  // a static import here would write to a different, unobserved module instance.
  async function fakeActivePty(): Promise<{ sent: Uint8Array[] }> {
    const sent: Uint8Array[] = []
    const { setActivePtySocket } = await import("./ptySocket")
    setActivePtySocket({
      sendInput: (b: Uint8Array) => sent.push(b),
    } as unknown as PtySocket)
    return { sent }
  }

  it("runMacro writes the macro payload to the active PTY socket", async () => {
    bootstrapMacros = [{ name: "Greet", text: "hello\nworld", surface: "both" }]
    const mod = await loadStore()
    const pty = await fakeActivePty()
    // A macro only runs against a FOCUSED terminal: select a session so
    // `selectedTarget` is set, mirroring the real precondition the guard enforces
    // (without a target the active socket is treated as stale and skipped).
    mod.selectSession("s1")
    mod.runMacro("Greet")
    expect(pty.sent).toHaveLength(1)
    // The bytes match the newline→Alt+Enter transform exactly.
    expect(Array.from(pty.sent[0])).toEqual(
      Array.from(macroPayloadBytes("hello\nworld")),
    )
  })

  it("runMacro is a no-op when no target is focused even if a socket lingers", async () => {
    // A stale active socket can outlive its pane during a focus switch; with no
    // selected target the macro must NOT be injected into it.
    bootstrapMacros = [{ name: "Greet", text: "hi", surface: "both" }]
    const mod = await loadStore()
    const pty = await fakeActivePty()
    // No selectSession → selectedTarget stays null.
    mod.runMacro("Greet")
    expect(pty.sent).toHaveLength(0)
  })

  it("runMacro is a no-op when no terminal is focused (no active socket)", async () => {
    bootstrapMacros = [{ name: "Greet", text: "hi", surface: "both" }]
    const mod = await loadStore()
    const { setActivePtySocket } = await import("./ptySocket")
    setActivePtySocket(null)
    expect(() => mod.runMacro("Greet")).not.toThrow()
  })

  it("runMacro ignores an unknown macro name", async () => {
    bootstrapMacros = [{ name: "Greet", text: "hi", surface: "both" }]
    const mod = await loadStore()
    const pty = await fakeActivePty()
    mod.runMacro("Nope")
    expect(pty.sent).toHaveLength(0)
  })

  it("saveMacros PUTs the FULL ordered entries to /api/v1/macros and closes", async () => {
    const mod = await loadStore()
    mod.openMacrosDialog()

    // The wholesale list preserves order and edits made in the dialog.
    const edited: MacroView[] = [
      { name: "Build", text: "cargo build --release", surface: "terminal" },
      { name: "Review", text: "review this", surface: "agent" },
      { name: "New", text: "added", surface: "both" },
    ]
    mod.saveMacros(edited)

    expect(fetchMock).toHaveBeenLastCalledWith(
      "/api/v1/macros",
      expect.objectContaining({
        method: "PUT",
        body: JSON.stringify({ entries: edited }),
      }),
    )
    // Order is preserved exactly (wholesale replace, not a delta).
    const call = fetchMock.mock.calls.find(
      (c) => String(c[0]) === "/api/v1/macros",
    )
    const sent = (JSON.parse((call?.[1] as RequestInit).body as string) as {
      entries: MacroView[]
    }).entries
    expect(sent.map((m) => m.name)).toEqual(["Build", "Review", "New"])
    // The dialog closes after dispatching the save.
    expect(mod.getSnapshot().macrosDialogOpen).toBe(false)
  })

  it("saveMacros refuses (no PUT, error toast, dialog stays open) when bootstrap is null", async () => {
    failBootstrap = true
    const mod = await loadStoreNoBootstrap()
    const { toast } = await import("sonner")
    mod.openMacrosDialog()

    // The shared module-level fetch double accumulates calls across tests; clear
    // it so a PUT here can only come from THIS save.
    fetchMock.mockClear()

    // A wholesale update_macros from an empty/unknown base would destroy the
    // server's macros, so the save must be refused while bootstrap is unloaded.
    mod.saveMacros([{ name: "X", text: "y", surface: "both" }])

    const putCall = fetchMock.mock.calls.find(
      (c) => String(c[0]) === "/api/v1/macros",
    )
    expect(putCall).toBeUndefined()
    expect(toast.error).toHaveBeenCalled()
    // The dialog stays open so the user keeps their edits and can retry.
    expect(mod.getSnapshot().macrosDialogOpen).toBe(true)
  })
})
