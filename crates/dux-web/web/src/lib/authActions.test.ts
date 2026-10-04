import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { DEFAULT_RECONNECT_ATTEMPT_TIMEOUT_SECONDS } from "./connectionTiming"

// The protected auth routes: logout, password change and the no-password
// warning's dismissal. They go through the one fetch door like every other
// protected request, answer in words rather than raw bodies, and give up after
// the browser's request deadline.

type Actions = typeof import("./authActions")
type Gate = typeof import("./authGate")

function json(status: number, body?: unknown, headers: Record<string, string> = {}): Response {
  return new Response(body === undefined ? null : JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", ...headers },
  })
}

let reply: (url: string, init?: RequestInit) => Promise<Response>
const fetchMock = vi.fn((input: string, init?: RequestInit) => {
  const url = String(input)
  if (url.endsWith("/api/v1/auth/status")) {
    return Promise.resolve(json(200, { password_set: true, required_here: true, signed_in: true }))
  }
  return reply(url, init)
})

async function load(): Promise<{ a: Actions; gate: Gate }> {
  const gate = await import("./authGate")
  const a = await import("./authActions")
  await gate.initAuthGate()
  return { a, gate }
}

beforeEach(() => {
  reply = async () => json(204)
  fetchMock.mockClear()
  vi.stubGlobal("fetch", fetchMock)
  vi.resetModules()
})

afterEach(() => {
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

function callsTo(path: string) {
  return fetchMock.mock.calls.filter(([u]) => String(u).endsWith(path))
}

describe("postLogout", () => {
  it("posts and reads 204 as done", async () => {
    const { a } = await load()
    expect(await a.postLogout()).toEqual({ kind: "ok" })
    expect((callsTo("/api/v1/auth/logout")[0][1] as RequestInit).method).toBe("POST")
  })

  it("reads an already-ended session as done", async () => {
    reply = async () => json(401, { error: "auth_required" })
    const { a } = await load()
    expect(await a.postLogout()).toEqual({ kind: "ok" })
  })

  it("answers an unknown refusal in words, never raw JSON", async () => {
    reply = async () => json(500, { error: "config_write_failed" })
    const { a } = await load()
    const answer = await a.postLogout()
    expect(answer).toEqual({ kind: "refused", message: "dux refused to sign out (HTTP 500)." })
  })

  it("gives up after the request deadline", async () => {
    vi.useFakeTimers()
    reply = (_u, init) =>
      new Promise((_resolve, reject) => {
        init?.signal?.addEventListener("abort", () =>
          reject(new DOMException("aborted", "AbortError")),
        )
      })
    const { a } = await load()
    const pending = a.postLogout()
    await vi.advanceTimersByTimeAsync(DEFAULT_RECONNECT_ATTEMPT_TIMEOUT_SECONDS * 1000)
    expect(await pending).toEqual({ kind: "unreachable", timedOut: true })
  })
})

describe("postPassword", () => {
  it("sends current and new, and reads 204 as changed", async () => {
    const { a } = await load()
    expect(await a.postPassword({ current: "old one", next: "new one" })).toEqual({ kind: "ok" })
    const init = callsTo("/api/v1/auth/password")[0][1] as RequestInit
    expect(JSON.parse(init.body as string)).toEqual({ current: "old one", new: "new one" })
  })

  it("leaves current out for a first password", async () => {
    const { a } = await load()
    await a.postPassword({ next: "first password here" })
    const init = callsTo("/api/v1/auth/password")[0][1] as RequestInit
    expect(JSON.parse(init.body as string)).toEqual({ new: "first password here" })
  })

  it("reads the contract's weak-password refusal with its score and feedback", async () => {
    reply = async () =>
      json(400, {
        error: "weak_password",
        score: 1,
        feedback: { warning: "This is a top-100 common password.", suggestions: [] },
      })
    const { a } = await load()
    expect(await a.postPassword({ next: "password1" })).toEqual({
      kind: "refused",
      message: "That password is too easy to guess, so dux refused it. This is a top-100 common password.",
      score: 1,
    })
  })

  it("reads a wrong current password, even as a 401, without signing out", async () => {
    reply = async () => json(401, { error: "wrong_current_password" })
    const { a, gate } = await load()
    expect(await a.postPassword({ current: "a", next: "b" })).toEqual({
      kind: "refused",
      message: "The current password is not right, so nothing was changed.",
      score: null,
    })
    expect(gate.getAuthPhase().kind).toBe("open")
  })

  it("reads auth_required as signed out, and the gate hears it", async () => {
    reply = async () => json(401, { error: "auth_required" })
    const { a, gate } = await load()
    expect(await a.postPassword({ current: "a", next: "b" })).toEqual({ kind: "signed_out" })
    expect(gate.getAuthPhase().kind).toBe("signed_out")
  })
})

describe("postDismissNoAuthWarning", () => {
  it("posts to the dismiss route", async () => {
    const { a } = await load()
    await a.postDismissNoAuthWarning()
    expect((callsTo("/api/v1/auth/dismiss-no-auth-warning")[0][1] as RequestInit).method).toBe(
      "POST",
    )
  })

  it("throws a sentence, never the raw body", async () => {
    reply = async () => json(500, { error: "config_write_failed" })
    const { a } = await load()
    await expect(a.postDismissNoAuthWarning()).rejects.toThrow(
      "dux refused to save that choice (HTTP 500).",
    )
  })
})
