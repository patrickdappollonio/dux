import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { MacroView, ViewModel } from "./types"

// `store` reads `location`/`localStorage`, registers a `popstate` listener, and
// at module load fires a boot `/api/me` fetch + constructs a `DuxSocket`. Stub
// the minimum so the import succeeds; steer the boot probe to auth-off so the
// store settles cleanly (mirrors storeAuth.test.ts's setup).

const fetchMock = vi.fn(async () => {
  return {
    status: 200,
    json: async () => ({ auth: "disabled" }),
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

// A minimal ViewModel carrying just the `macros` field the dialog seeds from.
function vmWithMacros(macros: MacroView[]): ViewModel {
  return {
    projects: [],
    sessions: [],
    changed_files: { staged: [], unstaged: [], watched_session_id: null },
    global_env: {},
    available_providers: [],
    welcome_tips: [],
    randomize_agent_names_by_default: false,
    gh_available: false,
    pr_banner_position: "top",
    palette_commands: [],
    macros,
  }
}

async function loadStore() {
  const mod = await import("./store")
  await vi.waitFor(() => {
    expect(mod.getSnapshot().auth.phase).not.toBe("checking")
  })
  return mod
}

describe("store ViewModel macros normalization", () => {
  it("defaults a missing macros key to [] at the boundary", async () => {
    const mod = await loadStore()
    // Simulate an older server snapshot that predates the `macros` field: the
    // key is absent on the wire. `onViewModel` must normalize it to a real
    // array so every consumer (and the typed-required field) holds true.
    const legacy = vmWithMacros([]) as Partial<ViewModel>
    delete legacy.macros
    mod.socket.onViewModel(legacy as ViewModel)
    expect(mod.getSnapshot().viewModel?.macros).toEqual([])
  })

  it("passes a present macros array through unchanged", async () => {
    const mod = await loadStore()
    const macros: MacroView[] = [
      { name: "Review", text: "review this", surface: "agent" },
    ]
    mod.socket.onViewModel(vmWithMacros(macros))
    expect(mod.getSnapshot().viewModel?.macros).toEqual(macros)
  })
})

describe("store macros dialog", () => {
  const seed: MacroView[] = [
    { name: "Review", text: "review this", surface: "agent" },
    { name: "Build", text: "cargo build", surface: "terminal" },
  ]

  it("openMacrosDialog seeds the draft from the ViewModel macros (copied)", async () => {
    const mod = await loadStore()
    mod.socket.onViewModel(vmWithMacros(seed))

    mod.openMacrosDialog()
    const snap = mod.getSnapshot()
    expect(snap.macrosDialogOpen).toBe(true)
    expect(snap.macrosDraft).toEqual(seed)
    // The draft is a copy, not the same array/objects as the ViewModel.
    expect(snap.macrosDraft).not.toBe(snap.viewModel?.macros)
    expect(snap.macrosDraft[0]).not.toBe(seed[0])
  })

  it("openMacrosDialog seeds an empty draft when there are no macros", async () => {
    const mod = await loadStore()
    mod.socket.onViewModel(vmWithMacros([]))
    mod.openMacrosDialog()
    expect(mod.getSnapshot().macrosDraft).toEqual([])
  })

  it("closeMacrosDialog clears the draft", async () => {
    const mod = await loadStore()
    mod.socket.onViewModel(vmWithMacros(seed))
    mod.openMacrosDialog()
    mod.closeMacrosDialog()
    const snap = mod.getSnapshot()
    expect(snap.macrosDialogOpen).toBe(false)
    expect(snap.macrosDraft).toEqual([])
  })
})

describe("store macros commands", () => {
  it("runMacro sends run_macro with the target id and name", async () => {
    const mod = await loadStore()
    const spy = vi.spyOn(mod.socket, "sendCommand")
    mod.runMacro("term-9", "Greet")
    expect(spy).toHaveBeenCalledWith("run_macro", {
      target_id: "term-9",
      name: "Greet",
    })
  })

  it("saveMacros sends update_macros with the FULL ordered entries and closes", async () => {
    const mod = await loadStore()
    const spy = vi.spyOn(mod.socket, "sendCommand")
    mod.openMacrosDialog()

    // The wholesale list preserves order and edits made in the dialog.
    const edited: MacroView[] = [
      { name: "Build", text: "cargo build --release", surface: "terminal" },
      { name: "Review", text: "review this", surface: "agent" },
      { name: "New", text: "added", surface: "both" },
    ]
    mod.saveMacros(edited)

    expect(spy).toHaveBeenCalledWith("update_macros", { entries: edited })
    // Order is preserved exactly (wholesale replace, not a delta).
    const call = spy.mock.calls.find((c) => c[0] === "update_macros")
    const sent = (call?.[1] as { entries: MacroView[] }).entries
    expect(sent.map((m) => m.name)).toEqual(["Build", "Review", "New"])
    // The dialog closes after dispatching the save.
    expect(mod.getSnapshot().macrosDialogOpen).toBe(false)
  })
})
