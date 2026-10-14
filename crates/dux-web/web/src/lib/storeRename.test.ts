import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { toastText } from "@/test/toastText"

// The rename dialog's save, driven through the store: a second press while the
// first request is still out must send nothing, rather than racing it to the
// server and toasting the loser's refusal.

const spineBody = {
  projects: [{ id: "p1", name: "widget" }],
  sessions: [
    {
      id: "s1",
      title: null,
      workspace: {
        kind: "managed",
        project_id: "p1",
        branch_name: "feat/login",
        initial_branch: "feat/login",
        branch_provenance: "created",
        source_branch: "main",
        worktree_path: "/tmp/feat-login",
      },
    },
  ],
  sidebar: { groups: [], agentless_start: null },
}

const patches: string[] = []
let answerPatch: (() => void) | null = null

function reply(body: unknown): Response {
  return {
    ok: true,
    status: 200,
    json: async () => body,
    text: async () => JSON.stringify(body),
    headers: { get: () => "application/json" },
  } as unknown as Response
}

const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
  const u = String(url)
  if (init?.method === "PATCH") {
    patches.push(u)
    await new Promise<void>((done) => {
      answerPatch = done
    })
    return reply({})
  }
  if (u.includes("/api/v1/workspace")) return reply(spineBody)
  return reply({})
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

const toasts: { tone: string; message: string }[] = []
vi.mock("sonner", () => {
  const record = (tone: string) => (message: unknown) => {
    toasts.push({ tone, message: toastText(message) })
    return tone
  }
  return {
    toast: Object.assign(record("info"), {
      info: record("info"),
      error: record("error"),
      success: record("success"),
      warning: record("warning"),
      loading: record("loading"),
      dismiss: () => {},
      custom: () => {},
    }),
  }
})

beforeEach(() => {
  patches.length = 0
  answerPatch = null
  toasts.length = 0
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
    expect(mod.getSnapshot().spine).not.toBeNull()
  })
  return mod
}

describe("saving a rename", () => {
  it("sends one request however often Save is pressed while it is out", async () => {
    const mod = await loadStore()
    mod.openRename("s1")
    mod.setRenameDraft("feat/signin")

    const first = mod.submitRename()
    const second = mod.submitRename()
    await vi.waitFor(() => expect(answerPatch).not.toBeNull())
    expect(mod.getSnapshot().renamePending).toBe(true)
    await second
    expect(patches).toHaveLength(1)

    answerPatch!()
    await first
    expect(patches).toHaveLength(1)
    expect(mod.getSnapshot().renamePending).toBe(false)
    expect(mod.getSnapshot().renameTarget).toBeNull()
    expect(toasts.filter((t) => t.tone === "error")).toEqual([])
  })
})
