import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { Bootstrap } from "./bootstrapApi"

// Store-level coverage for the agent startup-command-log viewer actions
// (openStartupLogs / selectStartupLog) and the rerun action. Mirrors the store
// test harness in `storeChangesPane.test.ts`: stub bootstrap so the store
// settles, then drive the REST endpoints through `fetchMock`.

function makeBootstrap(): Bootstrap {
  return {
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
    status_clear_seconds: 6,
  }
}

const LIST_BODY = {
  entries: [
    { name: "b.log", modified_at: "2026-01-02T00:00:00Z" },
    { name: "a.log", modified_at: "2026-01-01T00:00:00Z" },
  ],
  selected: { name: "b.log", content: "content of b.log" },
}

const fetchMock = vi.fn(async (url: string, _init?: RequestInit) => {
  const u = String(url)
  const ok = (json: unknown, status = 200) =>
    ({
      ok: status >= 200 && status < 300,
      status,
      json: async () => json,
      text: async () => (json ? JSON.stringify(json) : ""),
      headers: { get: () => null },
    }) as unknown as Response

  if (u.includes("/api/v1/bootstrap")) return ok(makeBootstrap())
  if (u.includes("/startup-logs/content")) {
    const name = new URL(u, "http://x").searchParams.get("name") ?? "b.log"
    return ok({ name, content: `content of ${name}` })
  }
  if (u.includes("/startup-logs")) return ok(LIST_BODY)
  if (u.includes("/rerun-startup-command")) return ok(null, 200)
  return ok({ auth: "disabled" })
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
  vi.stubGlobal("history", { go: () => {} })
  vi.stubGlobal("WebSocket", FakeWebSocket)
  vi.stubGlobal("fetch", fetchMock)
  vi.resetModules()
})

afterEach(() => {
  vi.unstubAllGlobals()
  fetchMock.mockClear()
})

async function loadStore() {
  const mod = await import("./store")
  await vi.waitFor(() => {
    expect(mod.getSnapshot().bootstrap).not.toBeNull()
  })
  return mod
}

describe("startup-command log viewer store actions", () => {
  it("openStartupLogs loads the listing newest-first with the newest preselected", async () => {
    const mod = await loadStore()
    mod.openStartupLogs("s1")
    // Loading is set synchronously while the request is in flight.
    expect(mod.getSnapshot().startupLogsTarget).toBe("s1")
    expect(mod.getSnapshot().startupLogsLoading).toBe(true)

    await vi.waitFor(() =>
      expect(mod.getSnapshot().startupLogsLoading).toBe(false),
    )
    const s = mod.getSnapshot()
    expect(s.startupLogsEntries.map((e) => e.name)).toEqual(["b.log", "a.log"])
    expect(s.startupLogsSelected?.name).toBe("b.log")
    expect(s.startupLogsSelected?.content).toBe("content of b.log")
    expect(s.startupLogsError).toBe(null)
  })

  it("selectStartupLog fetches the chosen file's contents", async () => {
    const mod = await loadStore()
    mod.openStartupLogs("s1")
    await vi.waitFor(() =>
      expect(mod.getSnapshot().startupLogsLoading).toBe(false),
    )

    mod.selectStartupLog("a.log")
    await vi.waitFor(() =>
      expect(mod.getSnapshot().startupLogsSelected?.name).toBe("a.log"),
    )
    expect(mod.getSnapshot().startupLogsSelected?.content).toBe(
      "content of a.log",
    )
    expect(fetchMock).toHaveBeenCalledWith(
      "/api/v1/sessions/s1/startup-logs/content?name=a.log",
      expect.objectContaining({ method: "GET" }),
    )
  })

  it("rerunStartupCommand POSTs the rerun endpoint", async () => {
    const mod = await loadStore()
    mod.rerunStartupCommand("s1")
    await vi.waitFor(() =>
      expect(fetchMock).toHaveBeenCalledWith(
        "/api/v1/sessions/s1/rerun-startup-command",
        expect.objectContaining({ method: "POST" }),
      ),
    )
  })

  it("ignores a late listing reply once the viewer has closed", async () => {
    const mod = await loadStore()
    mod.openStartupLogs("s1") // fires the in-flight list fetch
    mod.closeStartupLogs() // target → null before the reply resolves
    // Let the in-flight reply resolve; the stale-reply guard must drop it.
    await new Promise((r) => setTimeout(r, 0))
    const s = mod.getSnapshot()
    expect(s.startupLogsTarget).toBe(null)
    expect(s.startupLogsEntries).toEqual([])
    expect(s.startupLogsSelected).toBe(null)
    expect(s.startupLogsLoading).toBe(false)
  })
})
