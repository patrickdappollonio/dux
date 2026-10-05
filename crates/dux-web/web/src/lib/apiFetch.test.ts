import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

type Fetch = typeof import("./apiFetch")
type Gate = typeof import("./authGate")

function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  })
}

let routes: (url: string) => Promise<Response>
const fetchMock = vi.fn((input: string) => routes(String(input)))

async function load(): Promise<{ api: Fetch; gate: Gate }> {
  const gate = await import("./authGate")
  const api = await import("./apiFetch")
  return { api, gate }
}

beforeEach(() => {
  routes = async (url) =>
    url.endsWith("/auth/status")
      ? json(200, { required_here: true, signed_in: true, password_set: true })
      : json(200, { ok: true })
  fetchMock.mockClear()
  vi.stubGlobal("fetch", fetchMock)
  vi.resetModules()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe("apiFetch", () => {
  it("passes an ordinary answer through untouched", async () => {
    const { api, gate } = await load()
    await gate.initAuthGate()
    const resp = await api.apiFetch("/api/v1/workspace", { credentials: "same-origin" })
    expect(await resp.json()).toEqual({ ok: true })
    expect(fetchMock).toHaveBeenLastCalledWith("/api/v1/workspace", {
      credentials: "same-origin",
    })
  })

  it("turns a 401 into a sign-out and an interruption the caller can recognise", async () => {
    const { api, gate } = await load()
    await gate.initAuthGate()
    routes = async (url) =>
      url.endsWith("/auth/status")
        ? json(200, { required_here: true, signed_in: false, password_set: true })
        : json(401, { error: "auth_required" })
    const err = await api.apiFetch("/api/v1/workspace").catch((e: unknown) => e)
    expect(api.isAuthInterruption(err)).toBe(true)
    expect(gate.getAuthPhase().kind).toBe("signed_out")
  })

  it("leaves a 401 that is about something else to its caller", async () => {
    const { api, gate } = await load()
    await gate.initAuthGate()
    routes = async () => json(401, { error: "wrong_current_password" })
    const resp = await api.apiFetch("/api/v1/auth/password")
    expect(resp.status).toBe(401)
    expect(gate.getAuthPhase().kind).toBe("open")
  })

  it("turns a blocked 403 into the blocked page", async () => {
    const { api, gate } = await load()
    await gate.initAuthGate()
    routes = async () => json(403, { error: "blocked" })
    const err = await api.apiFetch("/api/v1/workspace").catch((e: unknown) => e)
    expect(api.isAuthInterruption(err)).toBe(true)
    expect(gate.getAuthPhase()).toStrictEqual({ kind: "blocked" })
  })

  it("leaves any other 403 to its caller, its body still readable", async () => {
    const { api, gate } = await load()
    await gate.initAuthGate()
    routes = async () => new Response("cross-origin request rejected", { status: 403 })
    const resp = await api.apiFetch("/api/v1/projects")
    expect(await resp.text()).toBe("cross-origin request rejected")
    expect(gate.getAuthPhase().kind).toBe("open")
  })

  it("turns a broken-auth 503 into the broken page", async () => {
    const { api, gate } = await load()
    await gate.initAuthGate()
    routes = async () => json(503, { error: "auth_config_invalid", detail: "bad hash" })
    const err = await api.apiFetch("/api/v1/workspace").catch((e: unknown) => e)
    expect(api.isAuthInterruption(err)).toBe(true)
    expect(gate.getAuthPhase()).toEqual({ kind: "broken", detail: "bad hash" })
  })

  it("sends nothing while signed out", async () => {
    const { api, gate } = await load()
    routes = async (url) =>
      url.endsWith("/auth/status")
        ? json(200, { required_here: true, signed_in: false, password_set: true })
        : json(200, {})
    await gate.initAuthGate()
    fetchMock.mockClear()
    const err = await api.apiFetch("/api/v1/workspace").catch((e: unknown) => e)
    expect(api.isAuthInterruption(err)).toBe(true)
    expect(fetchMock).not.toHaveBeenCalled()
  })

  // A request sent in one session whose answer lands after that session ended
  // and a new one began.
  async function straddle(answer: Response) {
    const { api, gate } = await load()
    await gate.initAuthGate()
    let release!: () => void
    routes = (url) =>
      url.endsWith("/auth/status")
        ? Promise.resolve(json(200, { required_here: true, signed_in: true }))
        : url.endsWith("/auth/login")
          ? Promise.resolve(new Response(null, { status: 204 }))
          : new Promise((resolve) => {
              release = () => resolve(answer)
            })
    const pending = api.apiFetch("/api/v1/sessions/s1/kill", { method: "POST" }).then(
      (r) => r,
      (e: unknown) => e,
    )
    await vi.waitFor(() => expect(release).toBeTypeOf("function"))
    routes = async (url) =>
      url.endsWith("/auth/status")
        ? json(200, { required_here: true, signed_in: false })
        : url.endsWith("/auth/login")
          ? new Response(null, { status: 204 })
          : json(200, {})
    gate.reportUnauthorized()
    routes = async (url) =>
      url.endsWith("/auth/login")
        ? new Response(null, { status: 204 })
        : json(200, { required_here: true, signed_in: true })
    await gate.signIn("again")
    expect(gate.getAuthPhase().kind).toBe("open")
    release()
    return { api, gate, result: await pending }
  }

  it("delivers a 2xx that lands after the session it was sent in ended, because the work was done", async () => {
    const { result } = await straddle(json(200, { done: true }))
    expect(result).toBeInstanceOf(Response)
    expect(await (result as Response).json()).toEqual({ done: true })
  })

  it("does not let an ended session's 401 sign the new session out", async () => {
    const { api, gate, result } = await straddle(json(401, { error: "auth_required" }))
    expect(api.isAuthInterruption(result)).toBe(true)
    expect(gate.getAuthPhase().kind).toBe("open")
  })

  it("says a request refused before sending was not sent", async () => {
    const { api, gate } = await load()
    routes = async (url) =>
      url.endsWith("/auth/status")
        ? json(200, { required_here: true, signed_in: false, password_set: true })
        : json(200, {})
    await gate.initAuthGate()
    const err = await api.apiFetch("/api/v1/workspace").catch((e: unknown) => e)
    expect(String((err as Error).message)).toContain("not sent")
  })

  it("lets a network failure through as the caller's own error", async () => {
    const { api, gate } = await load()
    await gate.initAuthGate()
    routes = async () => {
      throw new TypeError("network")
    }
    await expect(api.apiFetch("/api/v1/workspace")).rejects.toBeInstanceOf(TypeError)
  })

  it("does not pause before the first status answer, so a page that never asks still works", async () => {
    const { api } = await load()
    const resp = await api.apiFetch("/api/v1/workspace")
    expect(resp.ok).toBe(true)
  })
})
