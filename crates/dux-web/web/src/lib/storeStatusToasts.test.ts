import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// Mock sonner before importing the store so the store's top-level
// `import { toast } from "sonner"` picks up our spies.
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

// Mirror the store test harness (see storeCommitMessage.test.ts): the module
// reads location/localStorage, registers listeners, and fires a boot probe on
// import. Stub the minimum and steer the probe to auth-off so the store settles
// before each test.

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
  vi.clearAllMocks()
  vi.stubGlobal("location", { host: "localhost:0" })
  vi.stubGlobal("localStorage", {
    getItem: () => null,
    setItem: () => {},
    removeItem: () => {},
  })
  vi.stubGlobal("window", { addEventListener: () => {} })
  vi.stubGlobal("history", { go: () => {} })
  vi.stubGlobal("WebSocket", FakeWebSocket)
  vi.stubGlobal("fetch", fetchMock)
  vi.resetModules()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

async function loadStore() {
  const mod = await import("./store")
  await vi.waitFor(() => {
    expect(mod.getSnapshot().auth.phase).not.toBe("checking")
  })
  return mod
}

describe("engine status → sonner toast routing", () => {
  it("busy then success reuses the same toast id and dismisses on clear", async () => {
    const mod = await loadStore()
    const { toast } = await import("sonner")

    // Busy arrives first — should fire toast.loading with Infinity duration.
    mod.socket.onStatus("pull", "busy", "Pulling…")
    expect(toast.loading).toHaveBeenCalledWith("Pulling…", {
      id: "pull",
      duration: Infinity,
    })

    // Success replaces it on the same id — should fire toast.success with 6s.
    mod.socket.onStatus("pull", "info", "Pulled.")
    expect(toast.success).toHaveBeenCalledWith("Pulled.", {
      id: "pull",
      duration: 6000,
    })

    // Clear dismisses the toast by key.
    mod.socket.onStatusCleared("pull")
    expect(toast.dismiss).toHaveBeenCalledWith("pull")
  })

  it("error status fires toast.error with Infinity duration", async () => {
    const mod = await loadStore()
    const { toast } = await import("sonner")

    mod.socket.onStatus("push", "error", "Push failed.")
    expect(toast.error).toHaveBeenCalledWith("Push failed.", {
      id: "push",
      duration: Infinity,
    })
  })

  it("warning status fires toast.warning with Infinity duration", async () => {
    const mod = await loadStore()
    const { toast } = await import("sonner")

    mod.socket.onStatus("warn-key", "warning", "Careful!")
    expect(toast.warning).toHaveBeenCalledWith("Careful!", {
      id: "warn-key",
      duration: Infinity,
    })
  })

  it("unkeyed (anonymous) status uses the stable anonymous-slot id", async () => {
    const mod = await loadStore()
    const { toast } = await import("sonner")

    mod.socket.onStatus(null, "info", "All good.")
    expect(toast.success).toHaveBeenCalledWith("All good.", {
      id: "dux-anon-status",
      duration: 6000,
    })
  })

  it("anonymous clear dismisses the anonymous slot toast", async () => {
    const mod = await loadStore()
    const { toast } = await import("sonner")

    // Set an anonymous busy first.
    mod.socket.onStatus(null, "busy", "Uploading…")
    expect(toast.loading).toHaveBeenCalledWith("Uploading…", {
      id: "dux-anon-status",
      duration: Infinity,
    })

    // Clear dismisses the anonymous slot.
    mod.socket.onStatusCleared(null)
    expect(toast.dismiss).toHaveBeenCalledWith("dux-anon-status")
  })

  it("empty message is dropped — no toast fired", async () => {
    const mod = await loadStore()
    const { toast } = await import("sonner")

    mod.socket.onStatus("k", "info", "")
    expect(toast.success).not.toHaveBeenCalled()
    expect(toast.loading).not.toHaveBeenCalled()
  })

  it("status does NOT update a statusLine field — toasts are the sole web surface", async () => {
    const mod = await loadStore()

    mod.socket.onStatus("sl-key", "info", "Status bar message.")
    // The statusLine field was removed in T14; the store no longer carries it.
    expect(mod.getSnapshot()).not.toHaveProperty("statusLine")
  })

  it("commit-success status routes through onCommandResult with the anonymous slot id", async () => {
    // The engine's CommitChanges emits an anonymous Info status (no key), which
    // the wire layer surfaces as a commandResult. onCommandResult calls
    // showStatusToast(undefined, ...) → the stable anonymous-slot id.
    const mod = await loadStore()
    const { toast } = await import("sonner")

    mod.socket.onCommandResult(
      { tone: "info", message: "Changes committed successfully." },
      null,
    )
    expect(toast.success).toHaveBeenCalledWith("Changes committed successfully.", {
      id: "dux-anon-status",
      duration: 6000,
    })
  })

  it("keyed command-result busy is dismissed by its matching-key async final", async () => {
    // A synchronous command reply can carry a KEYED busy — e.g. the async
    // worktree-removal delete returns `delete:{id}` busy as a command_result.
    // Its final arrives later on the async status channel keyed identically.
    // The command-result busy MUST adopt the key as its sonner id so the async
    // final replaces it in place; otherwise the loading spinner lands on the
    // anonymous slot and strands forever (the reported worktree-delete bug).
    const mod = await loadStore()
    const { toast } = await import("sonner")

    mod.socket.onCommandResult(
      { key: "delete:s1", tone: "busy", message: 'Removing worktree for agent "x"…' },
      null,
    )
    expect(toast.loading).toHaveBeenCalledWith('Removing worktree for agent "x"…', {
      id: "delete:s1",
      duration: Infinity,
    })

    // The async success final reuses the same id, swapping spinner → check.
    mod.socket.onStatus("delete:s1", "info", "Agent and worktree removed.")
    expect(toast.success).toHaveBeenCalledWith("Agent and worktree removed.", {
      id: "delete:s1",
      duration: 6000,
    })
  })
})
