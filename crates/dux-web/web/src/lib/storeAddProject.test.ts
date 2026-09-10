import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// Exercises the store's add-project slice: the `runBrowse` staleness guard (a
// late browse reply must not repopulate a closed picker), the `initProject`
// wire contract (POST body carries `init_repo: true`), the `addProjectIntent`
// set/clear lifecycle, that inspections fire only on explicit selection (never
// as a side effect of browsing), and which refusals the add toasts.

const { notifyErrorMock } = vi.hoisted(() => ({ notifyErrorMock: vi.fn() }))

vi.mock("./notify", async (importOriginal) => ({
  ...(await importOriginal<typeof import("./notify")>()),
  notifyError: notifyErrorMock,
}))

interface Deferred {
  resolve: (value: unknown) => void
  reject: (reason: unknown) => void
  promise: Promise<unknown>
}

function defer(): Deferred {
  let resolve!: (value: unknown) => void
  let reject!: (reason: unknown) => void
  const promise = new Promise<unknown>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { resolve, reject, promise }
}

function jsonResponse(body: unknown) {
  return {
    ok: true,
    status: 200,
    json: async () => body,
    text: async () => JSON.stringify(body),
    headers: { get: () => null },
  }
}

// In-flight browse GETs, in call order; tests resolve them explicitly.
let pendingBrowse: Deferred[] = []
// Paths inspected via GET /api/v1/projects/inspect.
let inspectedPaths: string[] = []
// Bodies POSTed to /api/v1/projects.
let createBodies: unknown[] = []
// What the next POST /api/v1/projects answers, when it is not the 200 default.
let createRefusal: { status: number; text: string } | null = null

function refusalResponse(status: number, text: string) {
  return {
    ok: false,
    status,
    text: async () => text,
    headers: { get: () => null },
  }
}

const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
  const u = String(url)
  if (u.includes("/api/v1/browse")) {
    const d = defer()
    pendingBrowse.push(d)
    return d.promise as unknown as Response
  }
  if (u.includes("/api/v1/projects/inspect")) {
    const m = u.match(/[?&]path=([^&]+)/)
    inspectedPaths.push(m ? decodeURIComponent(m[1]) : "")
    return jsonResponse({
      kind: "plain",
      current_branch: null,
      warning: null,
      has_commits: false,
      gitignore_candidates: ["node_modules"],
    }) as unknown as Response
  }
  if (u.endsWith("/api/v1/projects") && init?.method === "POST") {
    createBodies.push(JSON.parse(String(init.body)))
    if (createRefusal) {
      return refusalResponse(
        createRefusal.status,
        createRefusal.text,
      ) as unknown as Response
    }
    return jsonResponse({ id: "p-new" }) as unknown as Response
  }
  throw new Error(`unexpected fetch: ${u}`)
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
  pendingBrowse = []
  inspectedPaths = []
  createBodies = []
  createRefusal = null
  notifyErrorMock.mockClear()
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
    expect(mod.getSnapshot().booted).toBe(true)
  })
  return mod
}

// Flush the fetch promise chain.
const tick = () => new Promise((r) => setTimeout(r, 0))

describe("add-project slice", () => {
  it("drops a late browse reply once the picker has closed (staleness guard)", async () => {
    const mod = await loadStore()
    mod.openAddProject()
    expect(pendingBrowse.length).toBe(1)
    mod.closeAddProject()
    pendingBrowse[0].resolve(
      jsonResponse({
        path: "/home/u",
        entries: [{ path: "/home/u/x", label: "x/", is_git_repo: false }],
      }),
    )
    // The reply resolves through browseApi's json() before the store guard.
    await tick()
    await tick()
    const snap = mod.getSnapshot()
    expect(snap.addProjectOpen).toBe(false)
    expect(snap.browseEntries).toEqual([])
  })

  it("initProject posts init_repo: true (the wire contract)", async () => {
    const mod = await loadStore()
    mod.initProject("/home/u/plain", "My Folder")
    await tick()
    expect(createBodies).toEqual([
      { path: "/home/u/plain", name: "My Folder", init_repo: true },
    ])
  })

  it("does not toast a 422, whose message the status stream already carries", async () => {
    createRefusal = {
      status: 422,
      text: "Initialized the repository, but couldn't create the initial commit.",
    }
    const mod = await loadStore()
    mod.initProject("/home/u/plain", "My Folder")
    await tick()
    await tick()
    expect(notifyErrorMock).not.toHaveBeenCalled()
  })

  it("toasts every other add refusal", async () => {
    createRefusal = { status: 400, text: "that path is inside a repository" }
    const mod = await loadStore()
    mod.initProject("/home/u/plain", "My Folder")
    await tick()
    await tick()
    expect(notifyErrorMock).toHaveBeenCalledWith(
      "that path is inside a repository",
    )
  })

  it("openAddProjectForInit sets the intent and close clears it", async () => {
    const mod = await loadStore()
    mod.openAddProjectForInit()
    expect(mod.getSnapshot().addProjectIntent).toBe("init")
    expect(mod.getSnapshot().addProjectOpen).toBe(true)
    mod.closeAddProject()
    expect(mod.getSnapshot().addProjectIntent).toBe("add")
    // A plain open never inherits the stale intent.
    mod.openAddProject()
    expect(mod.getSnapshot().addProjectIntent).toBe("add")
  })

  it("inspects only on explicit selection, never as a side effect of browsing", async () => {
    const mod = await loadStore()
    mod.openAddProject()
    pendingBrowse[0].resolve(
      jsonResponse({
        path: "/home/u",
        entries: [{ path: "/home/u/x", label: "x/", is_git_repo: false }],
      }),
    )
    await tick()
    await tick()
    mod.browseDir("/home/u/x")
    expect(inspectedPaths).toEqual([])

    mod.inspectProjectPath("/home/u/x")
    await tick()
    expect(inspectedPaths).toEqual(["/home/u/x"])
    const inspection = mod.getSnapshot().projectPathInspection
    expect(inspection).toMatchObject({
      path: "/home/u/x",
      kind: "plain",
      gitignoreCandidates: ["node_modules"],
      hasCommits: false,
      loading: false,
    })
  })
})
