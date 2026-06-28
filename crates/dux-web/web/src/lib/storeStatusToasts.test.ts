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

// The info-toast auto-clear window is config-driven (`status_clear_seconds` in
// the bootstrap document). Tests flip this before loading the store with a
// bootstrap to assert the computed duration.
let statusClearSeconds = 6

const fetchMock = vi.fn(async (url: string) => {
  const u = String(url)
  if (u.includes("/api/v1/bootstrap")) {
    return {
      status: 200,
      ok: true,
      json: async () => ({
        available_providers: [],
        macros: [],
        palette_commands: [],
        welcome_tips: [],
        dux_version: "development",
        randomize_agent_names_by_default: false,
        gh_available: false,
        pr_banner_position: "top",
        agent_scrollback_lines: 10000,
        show_changes_pane: true,
        global_env: {},
        status_clear_seconds: statusClearSeconds,
      }),
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  return {
    status: 200,
    ok: true,
    json: async () => ({ auth: "disabled" }),
    text: async () => "{}",
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
  statusClearSeconds = 6
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

// Load and wait for the bootstrap document to land too, so `status_clear_seconds`
// is available to the duration computation.
async function loadStoreWithBootstrap() {
  const mod = await import("./store")
  await vi.waitFor(() => {
    expect(mod.getSnapshot().auth.phase).not.toBe("checking")
    expect(mod.getSnapshot().bootstrap).not.toBeNull()
  })
  return mod
}

// Drive a `status` / `status_cleared` frame through the events socket exactly as
// the server pushes it on `/ws/events` (Phase 6: status moved off the retired
// `/ws`). `key` is omitted from the frame when null/undefined (the anonymous
// slot), mirroring the server's `skip_serializing_if = None`.
type StoreModule = typeof import("./store")
function status(
  mod: StoreModule,
  key: string | null | undefined,
  tone: string,
  message: string,
) {
  mod.eventsSocket.onEvent(
    key == null
      ? { event: "status", tone, message }
      : { event: "status", key, tone, message },
  )
}
function statusCleared(mod: StoreModule, key: string | null | undefined) {
  mod.eventsSocket.onEvent(
    key == null ? { event: "status_cleared" } : { event: "status_cleared", key },
  )
}

describe("engine status → sonner toast routing", () => {
  it("busy then success reuses the same toast id and dismisses on clear", async () => {
    const mod = await loadStore()
    const { toast } = await import("sonner")

    // Busy arrives first — should fire toast.loading with Infinity duration.
    status(mod, "pull", "busy", "Pulling…")
    expect(toast.loading).toHaveBeenCalledWith("Pulling…", {
      id: "pull",
      duration: Infinity,
    })

    // Success replaces it on the same id — should fire toast.success with 6s.
    status(mod, "pull", "info", "Pulled.")
    expect(toast.success).toHaveBeenCalledWith("Pulled.", {
      id: "pull",
      duration: 6000,
    })

    // Clear dismisses the toast by key.
    statusCleared(mod, "pull")
    expect(toast.dismiss).toHaveBeenCalledWith("pull")
  })

  it("error status fires toast.error with Infinity duration", async () => {
    const mod = await loadStore()
    const { toast } = await import("sonner")

    status(mod, "push", "error", "Push failed.")
    expect(toast.error).toHaveBeenCalledWith("Push failed.", {
      id: "push",
      duration: Infinity,
    })
  })

  it("warning status fires toast.warning with Infinity duration", async () => {
    const mod = await loadStore()
    const { toast } = await import("sonner")

    status(mod, "warn-key", "warning", "Careful!")
    expect(toast.warning).toHaveBeenCalledWith("Careful!", {
      id: "warn-key",
      duration: Infinity,
    })
  })

  it("unkeyed (anonymous) status uses the stable anonymous-slot id", async () => {
    const mod = await loadStore()
    const { toast } = await import("sonner")

    status(mod, null, "info", "All good.")
    expect(toast.success).toHaveBeenCalledWith("All good.", {
      id: "dux-anon-status",
      duration: 6000,
    })
  })

  it("anonymous clear dismisses the anonymous slot toast", async () => {
    const mod = await loadStore()
    const { toast } = await import("sonner")

    // Set an anonymous busy first.
    status(mod, null, "busy", "Uploading…")
    expect(toast.loading).toHaveBeenCalledWith("Uploading…", {
      id: "dux-anon-status",
      duration: Infinity,
    })

    // Clear dismisses the anonymous slot.
    statusCleared(mod, null)
    expect(toast.dismiss).toHaveBeenCalledWith("dux-anon-status")
  })

  it("empty message is dropped — no toast fired", async () => {
    const mod = await loadStore()
    const { toast } = await import("sonner")

    status(mod, "k", "info", "")
    expect(toast.success).not.toHaveBeenCalled()
    expect(toast.loading).not.toHaveBeenCalled()
  })

  it("status does NOT update a statusLine field — toasts are the sole web surface", async () => {
    const mod = await loadStore()

    status(mod, "sl-key", "info", "Status bar message.")
    // The statusLine field was removed in T14; the store no longer carries it.
    expect(mod.getSnapshot()).not.toHaveProperty("statusLine")
  })

  it("an anonymous (no-key) status uses the stable anonymous-slot id", async () => {
    // The engine's CommitChanges emits an anonymous Info status (no key); it now
    // arrives as a `status` event over `/ws/events` and lands on the stable
    // anonymous-slot id.
    const mod = await loadStore()
    const { toast } = await import("sonner")

    status(mod, undefined, "info", "Changes committed successfully.")
    expect(toast.success).toHaveBeenCalledWith("Changes committed successfully.", {
      id: "dux-anon-status",
      duration: 6000,
    })
  })

  it("info-toast duration honors a custom status_clear_seconds", async () => {
    statusClearSeconds = 10
    const mod = await loadStoreWithBootstrap()
    const { toast } = await import("sonner")

    status(mod, "k", "info", "Done.")
    expect(toast.success).toHaveBeenCalledWith("Done.", {
      id: "k",
      duration: 10000,
    })
  })

  it("status_clear_seconds of 0 makes info toasts sticky (Infinity)", async () => {
    statusClearSeconds = 0
    const mod = await loadStoreWithBootstrap()
    const { toast } = await import("sonner")

    status(mod, "k", "info", "Sticky info.")
    expect(toast.success).toHaveBeenCalledWith("Sticky info.", {
      id: "k",
      duration: Infinity,
    })
  })

  it("uses the 6s default window for info toasts when status_clear_seconds is the default", async () => {
    // The `?? 6` fallback covers both the pre-load (null bootstrap) window and a
    // config whose status_clear_seconds is the default 6 — either way, 6000ms.
    const mod = await loadStore()
    const { toast } = await import("sonner")

    status(mod, "k", "info", "Default window.")
    expect(toast.success).toHaveBeenCalledWith("Default window.", {
      id: "k",
      duration: 6000,
    })
  })

  it("a keyed busy is dismissed by its matching-key async final", async () => {
    // The async worktree-removal delete emits a `delete:{id}` busy whose final
    // arrives later keyed identically. Both ride `status` events; the busy adopts
    // the key as its sonner id so the final replaces it in place (otherwise the
    // spinner strands on the anonymous slot — the reported worktree-delete bug).
    const mod = await loadStore()
    const { toast } = await import("sonner")

    status(mod, "delete:s1", "busy", 'Removing worktree for agent "x"…')
    expect(toast.loading).toHaveBeenCalledWith('Removing worktree for agent "x"…', {
      id: "delete:s1",
      duration: Infinity,
    })

    // The async success final reuses the same id, swapping spinner → check.
    status(mod, "delete:s1", "info", "Agent and worktree removed.")
    expect(toast.success).toHaveBeenCalledWith("Agent and worktree removed.", {
      id: "delete:s1",
      duration: 6000,
    })
  })
})
