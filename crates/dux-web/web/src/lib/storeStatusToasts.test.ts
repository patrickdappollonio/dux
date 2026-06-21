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

  it("unkeyed (anonymous) status has no sonner id (transient)", async () => {
    const mod = await loadStore()
    const { toast } = await import("sonner")

    mod.socket.onStatus(null, "info", "All good.")
    expect(toast.success).toHaveBeenCalledWith("All good.", {
      id: undefined,
      duration: 6000,
    })
  })

  it("null key clear is a no-op for toasts", async () => {
    const mod = await loadStore()
    const { toast } = await import("sonner")

    mod.socket.onStatusCleared(null)
    expect(toast.dismiss).not.toHaveBeenCalled()
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

  it("commit-success status routes through engine (not a local CommitDialog toast)", async () => {
    // The engine emits a keyed "commit" status on success; that reaches the
    // client as an onStatus event and surfaces as a toast.success here.
    // CommitDialog.tsx must NOT fire its own toast.success — this test confirms
    // the engine path works so there is nothing for the component to duplicate.
    const mod = await loadStore()
    const { toast } = await import("sonner")

    mod.socket.onStatus("commit", "info", "Changes committed successfully.")
    expect(toast.success).toHaveBeenCalledWith("Changes committed successfully.", {
      id: "commit",
      duration: 6000,
    })
  })
})
