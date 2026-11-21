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

  it("names this page, and the page that opened it, on every request that is not a plain read", async () => {
    const { api, gate } = await load()
    const seat = await import("./pageSeat")
    await gate.initAuthGate()
    const sent = (): Headers =>
      new Headers((fetchMock.mock.calls.at(-1) as unknown as [string, RequestInit])[1].headers)
    const own = seat.pageSeatId()

    for (const method of ["POST", "PUT", "PATCH", "DELETE"]) {
      await api.apiFetch("/api/v1/x", { method, headers: { "content-type": "text/plain" } })
      expect(sent().get(seat.PAGE_HEADER)).toBe(own)
      expect(sent().get("content-type")).toBe("text/plain")
    }
    await api.apiFetch("/api/v1/x", { method: "POST", headers: new Headers({ a: "b" }) })
    expect(sent().get(seat.PAGE_HEADER)).toBe(own)
    expect(sent().get("a")).toBe("b")

    const opener = "b".repeat(32)
    seat.setEditorSurface(true)
    expect(seat.adoptOpenerPages([opener, own, "short", 7])).toBe(true)
    await api.apiFetch("/api/v1/x", { method: "post" })
    expect(sent().get(seat.PAGE_HEADER)).toBe(`${own},${opener}`)

    await api.apiFetch("/api/v1/x", { method: "GET" })
    expect(sent().has(seat.PAGE_HEADER)).toBe(false)
  })

  it("holds a change to a row this page is taking over until the claim lands, and never sends it when the claim does not", async () => {
    const { api, gate } = await load()
    const flight = await import("./takeoverFlight")
    await gate.initAuthGate()
    const stage = () =>
      api.apiFetch("/api/v1/sessions/s%201/git/stage", { method: "POST", body: "{}" })
    const sentStages = () =>
      fetchMock.mock.calls.filter(([url]) => String(url).includes("/git/stage")).length

    const landed = flight.beginTakeoverFlight("agent:s 1")
    const held = stage()
    // Another row's change and a read of this one go straight out.
    await api.apiFetch("/api/v1/sessions/other/git/stage", { method: "POST" })
    await api.apiFetch("/api/v1/sessions/s%201/changes")
    expect(sentStages()).toBe(1)
    landed(true)
    expect((await held).status).toBe(200)
    expect(sentStages()).toBe(2)

    const failed = flight.beginTakeoverFlight("agent:s 1")
    const refused = stage()
    failed(false)
    const answer = await refused
    expect(answer.status).toBe(423)
    expect(await answer.text()).toMatch(/take-over .* did not go through.*nothing was changed/i)
    expect(sentStages()).toBe(2)

    vi.useFakeTimers()
    try {
      flight.beginTakeoverFlight("terminal:t1")
      const late = api.apiFetch("/api/v1/projects/p1/terminals/t1/files/write", { method: "POST" })
      await vi.advanceTimersByTimeAsync(flight.TAKEOVER_WAIT_MS + 1)
      const answer = await late
      expect(answer.status).toBe(423)
      expect(await answer.text()).toMatch(/still taking .* over.*nothing was changed/i)
    } finally {
      vi.useRealTimers()
    }
    expect(fetchMock.mock.calls.some(([url]) => String(url).includes("/files/write"))).toBe(false)
  })

  it("names the terminal connection a drop goes to only once this page's take-over of it has landed", async () => {
    const { gate } = await load()
    const flight = await import("./takeoverFlight")
    const { uploadDroppedFile } = await import("./fileDropApi")
    routes = async (url) =>
      url.endsWith("/auth/status")
        ? json(200, { required_here: true, signed_in: true, password_set: true })
        : json(200, { requested_name: "a.txt", saved_name: "a.txt", path: "/x/a.txt" })
    await gate.initAuthGate()
    // A project terminal, an agent's extra tab and an agent's docked terminal:
    // each pane's pty id differs from its row's key.
    for (const [pty, row] of [
      ["term-4", "terminal:term-4"],
      ["tab-2", "agent:s1"],
      ["dock-1", "agent:s1"],
    ]) {
      fetchMock.mockClear()
      let conn = "9"
      const landed = flight.beginTakeoverFlight(row)
      const drop = uploadDroppedFile(new File(["x"], "a.txt"), { pty, row, conn: () => conn })
      // The take-over bounces the socket: the claim lands on its new connection.
      conn = "12"
      landed(true)
      await drop
      const url = String(fetchMock.mock.calls.find(([u]) => String(u).includes("file-drop"))![0])
      expect(new URL(url, "http://x").searchParams.get("conn")).toBe("12")
    }
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

  it("never reads a 401 without dux's code as this browser's session ending, and lets the status route decide", async () => {
    const { api, gate } = await load()
    await gate.initAuthGate()
    // An auth proxy in front of a dux that is still fine: the page stays.
    routes = async (url) =>
      url.endsWith("/auth/status")
        ? json(200, { required_here: true, signed_in: true, password_set: true })
        : new Response("Unauthorized", { status: 401 })
    const resp = await api.apiFetch("/api/v1/workspace")
    expect(resp.status).toBe(401)
    await new Promise((r) => setTimeout(r, 0))
    expect(gate.getAuthPhase().kind).toBe("open")
    // Something that answers everything that way is not dux.
    routes = async () => new Response("Unauthorized", { status: 401 })
    await api.apiFetch("/api/v1/workspace")
    await vi.waitFor(() => expect(gate.getAuthPhase()).toEqual({ kind: "not_dux" }))
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
        ? Promise.resolve(json(200, { required_here: true, signed_in: true, password_set: true }))
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
        ? json(200, { required_here: true, signed_in: false, password_set: true })
        : url.endsWith("/auth/login")
          ? new Response(null, { status: 204 })
          : json(200, {})
    gate.reportUnauthorized()
    routes = async (url) =>
      url.endsWith("/auth/login")
        ? new Response(null, { status: 204 })
        : json(200, { required_here: true, signed_in: true, password_set: true })
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
