import { afterEach, describe, expect, it, vi } from "vitest"

import { DEFAULT_RECONNECT_ATTEMPT_TIMEOUT_SECONDS } from "./connectionTiming"

import {
  fetchAuthStatus,
  normalizeAuthStatus,
  postLogin,
  retryAfterSeconds,
} from "./authApi"

afterEach(() => {
  vi.unstubAllGlobals()
})

function reply(
  status: number,
  body: unknown,
  headers: Record<string, string> = {},
): Response {
  const text = body === undefined ? "" : JSON.stringify(body)
  return new Response(status === 204 ? null : text, {
    status,
    headers: { "content-type": "application/json", ...headers },
  })
}

function stubFetch(res: Response | (() => Promise<Response>)) {
  const fn = vi.fn(async () => (typeof res === "function" ? res() : res))
  vi.stubGlobal("fetch", fn)
  return fn
}

// A fetch that only ends when its signal aborts it.
function hang(_input: unknown, init?: RequestInit): Promise<Response> {
  return new Promise((_resolve, reject) => {
    init?.signal?.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError")))
  })
}

const FULL = {
  password_set: true,
  required_here: true,
  signed_in: false,
  client_class: "network",
  transport_encrypted: false,
  no_auth_warning: false,
  weak_password: false,
  can_set_first_password: false,
  auth_broken: false,
}

describe("normalizeAuthStatus", () => {
  it("reads every field of the contract", () => {
    expect(normalizeAuthStatus(FULL)).toEqual({
      ...FULL,
      auth_broken: null,
      minimum_password_length: null,
      minimum_password_score: null,
      required_reason: null,
    })
  })

  it("reads why this device is treated as the network, and nothing else as a reason", () => {
    expect(
      normalizeAuthStatus({ required_reason: "`[server] tailscale` is `no`." }).required_reason,
    ).toBe("`[server] tailscale` is `no`.")
    expect(normalizeAuthStatus({ required_reason: "" }).required_reason).toBeNull()
    expect(normalizeAuthStatus({ required_reason: 7 }).required_reason).toBeNull()
  })

  it("reads a missing flag as false, so an older or partial answer never invents a login", () => {
    expect(normalizeAuthStatus({})).toMatchObject({
      password_set: false,
      required_here: false,
      signed_in: false,
      no_auth_warning: false,
      weak_password: false,
      can_set_first_password: false,
      auth_broken: null,
    })
  })

  it("keeps the transport unknown rather than calling it unencrypted", () => {
    expect(normalizeAuthStatus({}).transport_encrypted).toBeNull()
  })

  it("carries a broken-auth string as the detail, and true as an unnamed problem", () => {
    expect(normalizeAuthStatus({ auth_broken: "bad PHC string" }).auth_broken).toBe(
      "bad PHC string",
    )
    expect(normalizeAuthStatus({ auth_broken: true }).auth_broken).toBe("")
  })

  it("reads the minimums when the server reports them", () => {
    const s = normalizeAuthStatus({
      minimum_password_length: 14,
      minimum_password_score: 3,
    })
    expect(s.minimum_password_length).toBe(14)
    expect(s.minimum_password_score).toBe(3)
  })

  it("treats a non-object as an empty answer", () => {
    expect(normalizeAuthStatus(null).required_here).toBe(false)
    expect(normalizeAuthStatus("nope").signed_in).toBe(false)
  })
})

describe("fetchAuthStatus", () => {
  it("asks the public status route without caching", async () => {
    const fn = stubFetch(reply(200, FULL))
    const answer = await fetchAuthStatus()
    expect(answer).toMatchObject({ kind: "status", status: { required_here: true } })
    const [path, init] = fn.mock.calls[0] as unknown as [string, RequestInit]
    expect(path).toBe("/api/v1/auth/status")
    expect(init.cache).toBe("no-store")
    expect(init.credentials).toBe("same-origin")
  })

  it("answers blocked with where the block lives", async () => {
    stubFetch(reply(403, { error: "blocked", where: "/home/u/.config/dux/config.toml" }))
    expect(await fetchAuthStatus()).toEqual({
      kind: "blocked",
      where: "/home/u/.config/dux/config.toml",
    })
  })

  it("answers broken with the server's detail", async () => {
    stubFetch(reply(503, { error: "auth_config_invalid", detail: "password_hash is not a PHC string" }))
    expect(await fetchAuthStatus()).toEqual({
      kind: "broken",
      detail: "password_hash is not a PHC string",
    })
  })

  it("answers unreachable when the server cannot be reached", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new TypeError("network")
      }),
    )
    expect(await fetchAuthStatus()).toEqual({ kind: "unreachable", timedOut: false })
  })

  it("gives up on a status read that never answers, after the request deadline", async () => {
    vi.useFakeTimers()
    vi.stubGlobal("fetch", vi.fn(hang))
    const pending = fetchAuthStatus()
    await vi.advanceTimersByTimeAsync(DEFAULT_RECONNECT_ATTEMPT_TIMEOUT_SECONDS * 1000)
    expect(await pending).toEqual({ kind: "unreachable", timedOut: true })
    vi.useRealTimers()
  })

  it("answers unknown for a route this server does not have", async () => {
    stubFetch(new Response("not found", { status: 404 }))
    expect(await fetchAuthStatus()).toEqual({ kind: "unknown" })
  })
})

describe("retryAfterSeconds", () => {
  it("reads delta seconds from the header", () => {
    expect(retryAfterSeconds("17", null, 0)).toBe(17)
  })

  it("reads an HTTP date from the header", () => {
    const now = Date.parse("2026-10-03T12:00:00Z")
    expect(retryAfterSeconds("Sat, 03 Oct 2026 12:00:30 GMT", null, now)).toBe(30)
  })

  it("falls back to a retry_after field in the body", () => {
    expect(retryAfterSeconds(null, { retry_after: 9 }, 0)).toBe(9)
    expect(retryAfterSeconds(null, { retry_after_seconds: 4 }, 0)).toBe(4)
  })

  it("answers null when neither says", () => {
    expect(retryAfterSeconds(null, {}, 0)).toBeNull()
    expect(retryAfterSeconds("soon", null, 0)).toBeNull()
  })

  it("never counts down from a negative wait", () => {
    expect(retryAfterSeconds("-5", null, 0)).toBe(0)
  })
})

describe("postLogin", () => {
  it("posts the password as JSON and reads 204 as signed in", async () => {
    const fn = stubFetch(reply(204, undefined))
    expect(await postLogin("correct horse battery staple")).toEqual({ kind: "ok" })
    const [path, init] = fn.mock.calls[0] as unknown as [string, RequestInit]
    expect(path).toBe("/api/v1/auth/login")
    expect(init.method).toBe("POST")
    expect(JSON.parse(init.body as string)).toEqual({
      password: "correct horse battery staple",
    })
  })

  it("reads 401 as a wrong password", async () => {
    stubFetch(reply(401, { error: "auth_required" }))
    expect(await postLogin("x")).toEqual({ kind: "wrong" })
  })

  it("reads 429 with the wait the server asks for", async () => {
    stubFetch(reply(429, { error: "rate_limited" }, { "retry-after": "12" }))
    expect(await postLogin("x")).toEqual({ kind: "rate_limited", retryAfterSeconds: 12 })
  })

  it("reads 403 as blocked, keeping where", async () => {
    stubFetch(reply(403, { error: "blocked", where: "config.toml" }))
    expect(await postLogin("x")).toEqual({ kind: "blocked", where: "config.toml" })
  })

  it("reads 503 as a broken auth config", async () => {
    stubFetch(reply(503, { error: "auth_config_invalid", detail: "bad" }))
    expect(await postLogin("x")).toEqual({ kind: "broken", detail: "bad" })
  })

  it("reads a network failure as unreachable", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new TypeError("network")
      }),
    )
    expect(await postLogin("x")).toEqual({ kind: "unreachable", timedOut: false })
  })

  it("gives up on a sign-in that never answers, after the request deadline", async () => {
    vi.useFakeTimers()
    vi.stubGlobal("fetch", vi.fn(hang))
    const pending = postLogin("x")
    await vi.advanceTimersByTimeAsync(DEFAULT_RECONNECT_ATTEMPT_TIMEOUT_SECONDS * 1000)
    expect(await pending).toEqual({ kind: "unreachable", timedOut: true })
    vi.useRealTimers()
  })

  it("never shows a JSON refusal raw", async () => {
    stubFetch(reply(500, { error: "something_else" }))
    expect(await postLogin("x")).toEqual({
      kind: "refused",
      message: "dux refused to sign in (HTTP 500).",
    })
  })

  it("reads any other refusal with the server's words", async () => {
    stubFetch(new Response("cross-origin request rejected", { status: 403 }))
    expect(await postLogin("x")).toEqual({
      kind: "refused",
      message: "cross-origin request rejected",
    })
  })
})
