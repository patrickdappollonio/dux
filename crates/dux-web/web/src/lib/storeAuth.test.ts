import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { Spine } from "./workspaceApi"

// The sign-in journeys at the store level, against a mocked server that holds
// one session cookie's worth of state: whether this browser is signed in.
//
//   Situation: a dux with a password, reached from somewhere it applies.
//   Task: boot only when signed in, survive the session ending mid-use, and
//         land back on the same position with nothing lost.
//   Action: drive the auth routes, the protected routes and the app socket.
//   Result: what was fetched, what was opened, where the URL is.

function makeSpine(ids: string[]): Spine {
  return {
    projects: [{ id: "p1", name: "p1" }] as unknown as Spine["projects"],
    sessions: ids.map((id) => ({
      id,
      workspace: {
        kind: "managed",
        project_id: "p1",
        branch_name: "",
        initial_branch: "",
        branch_provenance: "created",
        source_branch: "",
        worktree_path: "",
      },
      status: "active",
      tabs: [{ id }],
    })) as unknown as Spine["sessions"],
    terminals: [],
    sidebar: { groups: [], agentless_start: null },
  }
}

// The server.
let signedIn = false
let passwordSet = true
let spineBody: Spine = makeSpine(["s1"])
let workspaceGets = 0
let bootstrapGets = 0
// When set, the next workspace GET waits for this to be called.
let holdWorkspace: ((release: () => void) => void) | null = null

function json(status: number, body?: unknown): Response {
  return new Response(body === undefined ? null : JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  })
}

function protectedAnswer(body: unknown): Response {
  return signedIn || !passwordSet ? json(200, body) : json(401, { error: "auth_required" })
}

const fetchMock = vi.fn(async (input: string, init?: RequestInit) => {
  const url = String(input)
  if (url.endsWith("/api/v1/auth/status")) {
    return json(200, {
      password_set: passwordSet,
      required_here: passwordSet,
      signed_in: signedIn,
      client_class: "network",
      transport_encrypted: false,
    })
  }
  if (url.endsWith("/api/v1/auth/login")) {
    const body = JSON.parse(String(init?.body ?? "{}")) as { password?: string }
    if (body.password !== "correct horse battery staple") {
      return json(401, { error: "auth_required" })
    }
    signedIn = true
    return json(204)
  }
  if (url.endsWith("/api/v1/auth/logout")) {
    signedIn = false
    return json(204)
  }
  if (url.includes("/api/v1/workspace")) {
    workspaceGets++
    const answer = protectedAnswer(spineBody)
    const hold = holdWorkspace
    if (hold) {
      holdWorkspace = null
      return new Promise<Response>((resolve) => hold(() => resolve(answer)))
    }
    return answer
  }
  if (url.includes("/api/v1/bootstrap")) {
    bootstrapGets++
    return protectedAnswer({})
  }
  if (url.includes("/changes")) return protectedAnswer({ rev: 1, staged: [], unstaged: [] })
  return protectedAnswer({})
})

class FakeWebSocket {
  static OPEN = 1
  static instances: FakeWebSocket[] = []
  onopen: (() => void) | null = null
  onclose: ((e: { code: number }) => void) | null = null
  onerror: (() => void) | null = null
  onmessage: (() => void) | null = null
  binaryType = ""
  readyState = 0
  url: string
  constructor(url: string) {
    this.url = url
    FakeWebSocket.instances.push(this)
  }
  close() {}
  send() {}
  open() {
    this.readyState = 1
    this.onopen?.()
  }
  drop(code: number) {
    this.readyState = 3
    this.onclose?.({ code })
  }
}

const hashRef = { value: "" }

beforeEach(() => {
  signedIn = false
  passwordSet = true
  spineBody = makeSpine(["s1"])
  workspaceGets = 0
  bootstrapGets = 0
  holdWorkspace = null
  FakeWebSocket.instances = []
  hashRef.value = ""
  vi.stubGlobal("localStorage", {
    getItem: () => null,
    setItem: () => {},
    removeItem: () => {},
  })
  vi.stubGlobal("window", { addEventListener: () => {} })
  vi.stubGlobal("history", {
    pushState: (_s: unknown, _t: string, url: string) => {
      hashRef.value = url.startsWith("#") ? url : ""
    },
    replaceState: (_s: unknown, _t: string, url: string) => {
      hashRef.value = url.startsWith("#") ? url : ""
    },
    state: null,
  })
  vi.stubGlobal("location", {
    protocol: "http:",
    host: "localhost:0",
    get hash() {
      return hashRef.value
    },
    pathname: "/",
    search: "",
  })
  vi.stubGlobal("WebSocket", FakeWebSocket)
  vi.stubGlobal("fetch", fetchMock)
  fetchMock.mockClear()
  vi.resetModules()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

async function settle(): Promise<void> {
  for (let i = 0; i < 5; i++) await new Promise((r) => setTimeout(r, 0))
}

async function loadSignedOut(hash: string) {
  hashRef.value = hash
  const store = await import("./store")
  const gate = await import("./authGate")
  await vi.waitFor(() => expect(gate.getAuthPhase().kind).toBe("signed_out"))
  return { store, gate }
}

async function loadSignedIn(hash: string) {
  signedIn = true
  hashRef.value = hash
  const store = await import("./store")
  const gate = await import("./authGate")
  await vi.waitFor(() => expect(store.getSnapshot().spine).not.toBeNull())
  return { store, gate }
}

describe("booting signed out", () => {
  it("fetches nothing protected and opens no socket until the password is given", async () => {
    const { store } = await loadSignedOut("#/agent/s1")
    await settle()
    expect(workspaceGets).toBe(0)
    expect(bootstrapGets).toBe(0)
    expect(FakeWebSocket.instances).toHaveLength(0)
    expect(store.getSnapshot().booted).toBe(false)
  })

  it("boots after signing in and lands on the position the URL named", async () => {
    const { store, gate } = await loadSignedOut("#/agent/s1")
    expect(await gate.signIn("correct horse battery staple")).toEqual({ kind: "ok" })
    await vi.waitFor(() => expect(store.getSnapshot().spine).not.toBeNull())
    expect(FakeWebSocket.instances).toHaveLength(1)
    expect(store.getSnapshot().selectedTarget).toMatchObject({
      kind: "agent",
      sessionId: "s1",
    })
    expect(hashRef.value).toBe("#/agent/s1")
  })

  it("a wrong password boots nothing", async () => {
    const { gate } = await loadSignedOut("")
    expect(await gate.signIn("nope")).toEqual({ kind: "wrong" })
    await settle()
    expect(workspaceGets).toBe(0)
    expect(gate.getAuthPhase().kind).toBe("signed_out")
  })
})

describe("booting with no password in the way", () => {
  it("boots exactly as before", async () => {
    passwordSet = false
    const store = await import("./store")
    await vi.waitFor(() => expect(store.getSnapshot().spine).not.toBeNull())
    expect(store.getSnapshot().booted).toBe(true)
    expect(FakeWebSocket.instances).toHaveLength(1)
  })
})

describe("the session ending mid-use", () => {
  it("a 401 on a refetch shows the login, and signing in refetches and keeps the position", async () => {
    const { store, gate } = await loadSignedIn("#/agent/s1")
    FakeWebSocket.instances[0].open()
    signedIn = false
    store.eventsSocket.onEvent({ event: "sessions.changed" })
    await vi.waitFor(() => expect(gate.getAuthPhase().kind).toBe("signed_out"))
    expect(hashRef.value).toBe("#/agent/s1")

    const before = workspaceGets
    spineBody = makeSpine(["s1", "s2"])
    await gate.signIn("correct horse battery staple")
    await vi.waitFor(() => expect(store.getSnapshot().spine?.sessions).toHaveLength(2))
    expect(workspaceGets).toBeGreaterThan(before)
    expect(hashRef.value).toBe("#/agent/s1")
    expect(store.getSnapshot().selectedTarget).toMatchObject({ sessionId: "s1" })
  })

  it("a 4401 close holds the app socket, and signing in reopens it and refetches", async () => {
    const { store, gate } = await loadSignedIn("")
    FakeWebSocket.instances[0].open()
    signedIn = false
    FakeWebSocket.instances[0].drop(4401)
    expect(gate.getAuthPhase().kind).toBe("signed_out")
    await settle()
    expect(FakeWebSocket.instances).toHaveLength(1)

    const before = workspaceGets
    await gate.signIn("correct horse battery staple")
    expect(FakeWebSocket.instances).toHaveLength(2)
    FakeWebSocket.instances[1].open()
    await vi.waitFor(() => expect(workspaceGets).toBeGreaterThan(before))
  })

  it("a refused upgrade, which looks like a network drop, is found by asking the status route", async () => {
    const { gate } = await loadSignedIn("")
    FakeWebSocket.instances[0].open()
    signedIn = false
    FakeWebSocket.instances[0].drop(1006)
    await vi.waitFor(() => expect(gate.getAuthPhase().kind).toBe("signed_out"))
  })

  it("an answer from the ended session is not applied after signing back in", async () => {
    const { store, gate } = await loadSignedIn("")
    FakeWebSocket.instances[0].open()
    let release!: () => void
    holdWorkspace = (r) => {
      release = r
    }
    spineBody = makeSpine(["stale"])
    store.eventsSocket.onEvent({ event: "sessions.changed" })
    await vi.waitFor(() => expect(release).toBeTypeOf("function"))
    // The session ends and a new one starts while that GET is still out.
    gate.reportUnauthorized()
    spineBody = makeSpine(["fresh"])
    await gate.signIn("correct horse battery staple")
    await vi.waitFor(() =>
      expect(store.getSnapshot().spine?.sessions.map((s) => s.id)).toEqual(["fresh"]),
    )
    release()
    await settle()
    expect(store.getSnapshot().spine?.sessions.map((s) => s.id)).toEqual(["fresh"])
  })

  it("unsaved editor drafts outlive the sign-out", async () => {
    const { gate } = await loadSignedIn("")
    const drafts = await import("./editorDrafts")
    drafts.storeRootDrafts(
      "agent:s1",
      new Map([["t1", { content: "unsaved", loading: false } as never]]),
    )
    gate.reportUnauthorized()
    await gate.signIn("correct horse battery staple")
    expect(drafts.loadRootDrafts("agent:s1").get("t1")).toMatchObject({
      content: "unsaved",
    })
  })
})

describe("signing out from the menu", () => {
  it("ends the session, keeps the URL, and the next sign-in returns to it", async () => {
    const { store, gate } = await loadSignedIn("#/agent/s1")
    FakeWebSocket.instances[0].open()
    await gate.signOut()
    expect(gate.getAuthPhase()).toMatchObject({ kind: "signed_out", reason: "signed_out" })
    expect(signedIn).toBe(false)
    expect(hashRef.value).toBe("#/agent/s1")
    await gate.signIn("correct horse battery staple")
    expect(gate.getAuthPhase().kind).toBe("open")
    expect(store.getSnapshot().selectedTarget).toMatchObject({ sessionId: "s1" })
  })
})
