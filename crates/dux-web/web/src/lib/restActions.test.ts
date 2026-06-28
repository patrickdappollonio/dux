import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { setConnectionId } from "./connection"
import { ProjectsApiError, projectsApi } from "./projectsApi"
import { SessionsApiError, sessionsApi } from "./sessionsApi"

// Wire-level coverage for the Phase 4 REST action clients: each verb hits the
// exact endpoint with the right method/body, stamps the per-connection id as
// `X-Connection-Id` (every endpoint reads it to scope its `/ws` toasts back to
// this client), and surfaces a non-2xx as a typed error carrying the HTTP status
// + parsed message. The store-level toast-on-error and optimistic-overlay
// behaviour is exercised in `restActionsStore.test.ts` / `storeSpine.test.ts`.

// A fetch double that records the most recent call and serves a canned response.
function stubOkFetch(status = 200, jsonBody: unknown = {}) {
  const fetchMock = vi.fn(async () => ({
    ok: status >= 200 && status < 300,
    status,
    json: async () => jsonBody,
    text: async () => (jsonBody ? JSON.stringify(jsonBody) : ""),
    headers: { get: () => null },
  })) as unknown as typeof fetch
  vi.stubGlobal("fetch", fetchMock)
  return fetchMock as unknown as ReturnType<typeof vi.fn>
}

// Pull apart the [url, init] of the last fetch call for assertions.
function lastCall(fetchMock: ReturnType<typeof vi.fn>) {
  const [url, init] = fetchMock.mock.calls.at(-1) as [string, RequestInit]
  return {
    url,
    method: init.method,
    headers: init.headers as Record<string, string>,
    body: init.body ? JSON.parse(init.body as string) : undefined,
  }
}

beforeEach(() => {
  setConnectionId("conn-123")
})

afterEach(() => {
  vi.unstubAllGlobals()
  setConnectionId(null)
})

describe("sessionsApi", () => {
  it("create (new) POSTs /api/v1/sessions with the connection id and body", async () => {
    const fetchMock = stubOkFetch(201, { id: "s1" })
    await sessionsApi.create({ kind: "new", project_id: "p1", name: "feat" })
    const c = lastCall(fetchMock)
    expect(c.url).toBe("/api/v1/sessions")
    expect(c.method).toBe("POST")
    expect(c.headers["x-connection-id"]).toBe("conn-123")
    expect(c.headers["content-type"]).toBe("application/json")
    expect(c.body).toEqual({ kind: "new", project_id: "p1", name: "feat" })
  })

  it("create (fork/from_worktree/from_pr) send the matching discriminated body", async () => {
    const fetchMock = stubOkFetch(201, { id: "s2" })
    await sessionsApi.create({ kind: "fork", session_id: "s1", name: "f" })
    expect(lastCall(fetchMock).body).toEqual({
      kind: "fork",
      session_id: "s1",
      name: "f",
    })
    await sessionsApi.create({
      kind: "from_worktree",
      project_id: "p1",
      worktree_path: "/wt",
      name: "w",
    })
    expect(lastCall(fetchMock).body).toEqual({
      kind: "from_worktree",
      project_id: "p1",
      worktree_path: "/wt",
      name: "w",
    })
    await sessionsApi.create({ kind: "from_pr", project_id: "p1", pr: "#7", name: "p" })
    expect(lastCall(fetchMock).body).toEqual({
      kind: "from_pr",
      project_id: "p1",
      pr: "#7",
      name: "p",
    })
  })

  it("remove DELETEs with the delete_worktree query flag", async () => {
    const fetchMock = stubOkFetch(204, null)
    await sessionsApi.remove("s 1", true)
    const c = lastCall(fetchMock)
    expect(c.url).toBe("/api/v1/sessions/s%201?delete_worktree=true")
    expect(c.method).toBe("DELETE")
    expect(c.headers["x-connection-id"]).toBe("conn-123")
  })

  it("patch PATCHes the session with the title/provider/auto_reopen body", async () => {
    const fetchMock = stubOkFetch(200, { provider_change: "pending_reconnect" })
    await sessionsApi.patch("s1", { provider: "codex" })
    const c = lastCall(fetchMock)
    expect(c.url).toBe("/api/v1/sessions/s1")
    expect(c.method).toBe("PATCH")
    expect(c.body).toEqual({ provider: "codex" })
  })

  it("reconnect POSTs /reconnect with the force flag", async () => {
    const fetchMock = stubOkFetch(200)
    await sessionsApi.reconnect("s1", true)
    const c = lastCall(fetchMock)
    expect(c.url).toBe("/api/v1/sessions/s1/reconnect")
    expect(c.method).toBe("POST")
    expect(c.body).toEqual({ force: true })
  })

  it("reorder POSTs /sessions/reorder with project + ordered ids", async () => {
    const fetchMock = stubOkFetch(200)
    await sessionsApi.reorder("p1", ["s2", "s1"])
    const c = lastCall(fetchMock)
    expect(c.url).toBe("/api/v1/sessions/reorder")
    expect(c.body).toEqual({ project_id: "p1", session_ids: ["s2", "s1"] })
  })

  it("generateCommitMessage POSTs the commit-message trigger (no body)", async () => {
    const fetchMock = stubOkFetch(202)
    await sessionsApi.generateCommitMessage("s1")
    const c = lastCall(fetchMock)
    expect(c.url).toBe("/api/v1/sessions/s1/commit-message")
    expect(c.method).toBe("POST")
    expect(c.body).toBeUndefined()
    // Still scoped even without a JSON body.
    expect(c.headers["x-connection-id"]).toBe("conn-123")
  })

  it("throws a typed SessionsApiError carrying status + message on non-2xx", async () => {
    stubOkFetch(409, null)
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({
        ok: false,
        status: 409,
        text: async () => "create already in flight",
        headers: { get: () => null },
      })) as unknown as typeof fetch,
    )
    const err = await sessionsApi
      .create({ kind: "new", project_id: "p1" })
      .catch((e) => e)
    expect(err).toBeInstanceOf(SessionsApiError)
    expect(err.status).toBe(409)
    expect(err.message).toBe("create already in flight")
  })

  it("omits the header until the connection id is known", async () => {
    setConnectionId(null)
    const fetchMock = stubOkFetch(200)
    await sessionsApi.reconnect("s1", false)
    expect(lastCall(fetchMock).headers["x-connection-id"]).toBeUndefined()
  })
})

describe("projectsApi", () => {
  it("create POSTs /api/v1/projects (with optional checkout_default)", async () => {
    const fetchMock = stubOkFetch(201, { id: "p1" })
    await projectsApi.create({ path: "/repo", name: "Repo", checkout_default: true })
    const c = lastCall(fetchMock)
    expect(c.url).toBe("/api/v1/projects")
    expect(c.method).toBe("POST")
    expect(c.body).toEqual({ path: "/repo", name: "Repo", checkout_default: true })
  })

  it("remove DELETEs /api/v1/projects/{id}", async () => {
    const fetchMock = stubOkFetch(204, null)
    await projectsApi.remove("p1")
    const c = lastCall(fetchMock)
    expect(c.url).toBe("/api/v1/projects/p1")
    expect(c.method).toBe("DELETE")
  })

  it("patch sends the tri-state body verbatim (null clears, omit untouched)", async () => {
    const fetchMock = stubOkFetch(200)
    await projectsApi.patch("p1", {
      provider: "codex",
      auto_reopen_agents: null,
      env: { A: "1" },
    })
    const c = lastCall(fetchMock)
    expect(c.url).toBe("/api/v1/projects/p1")
    expect(c.method).toBe("PATCH")
    // `startup_command` is omitted (untouched); `auto_reopen_agents` is null (clear).
    expect(c.body).toEqual({
      provider: "codex",
      auto_reopen_agents: null,
      env: { A: "1" },
    })
    expect("startup_command" in c.body).toBe(false)
  })

  it("reorder/pull/checkoutDefault hit the new project endpoints", async () => {
    const fetchMock = stubOkFetch(200)
    await projectsApi.reorder(["p2", "p1"])
    expect(lastCall(fetchMock).url).toBe("/api/v1/projects/reorder")
    expect(lastCall(fetchMock).body).toEqual({ project_ids: ["p2", "p1"] })
    await projectsApi.pull("p1")
    expect(lastCall(fetchMock).url).toBe("/api/v1/projects/p1/pull")
    await projectsApi.checkoutDefault("p1")
    expect(lastCall(fetchMock).url).toBe("/api/v1/projects/p1/checkout-default")
  })

  it("throws a typed ProjectsApiError on non-2xx", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({
        ok: false,
        status: 400,
        text: async () => "path is not a git repo",
        headers: { get: () => null },
      })) as unknown as typeof fetch,
    )
    const err = await projectsApi.create({ path: "/x" }).catch((e) => e)
    expect(err).toBeInstanceOf(ProjectsApiError)
    expect(err.status).toBe(400)
    expect(err.message).toBe("path is not a git repo")
  })
})
