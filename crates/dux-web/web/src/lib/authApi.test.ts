import { afterEach, describe, expect, it, vi } from "vitest"

import {
  fetchAuthStatus,
  normalizeAuthStatus,
  postDismissNoAuthWarning,
  postLogin,
  postLogout,
  postPassword,
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
    })
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

  it("answers unknown when the server cannot be reached", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new TypeError("network")
      }),
    )
    expect(await fetchAuthStatus()).toEqual({ kind: "unknown" })
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
    expect(await postLogin("x")).toEqual({ kind: "unreachable" })
  })

  it("reads any other refusal with the server's words", async () => {
    stubFetch(new Response("cross-origin request rejected", { status: 403 }))
    expect(await postLogin("x")).toEqual({
      kind: "refused",
      message: "cross-origin request rejected",
    })
  })
})

describe("postLogout", () => {
  it("posts to the logout route and reads 204 as done", async () => {
    const fn = stubFetch(reply(204, undefined))
    expect(await postLogout()).toEqual({ kind: "ok" })
    const [path, init] = fn.mock.calls[0] as unknown as [string, RequestInit]
    expect(path).toBe("/api/v1/auth/logout")
    expect(init.method).toBe("POST")
  })

  it("reads a 401 as already signed out", async () => {
    stubFetch(reply(401, { error: "auth_required" }))
    expect(await postLogout()).toEqual({ kind: "ok" })
  })

  it("reads a network failure as unreachable", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new TypeError("network")
      }),
    )
    expect(await postLogout()).toEqual({ kind: "unreachable" })
  })
})

describe("postPassword", () => {
  it("sends current and new, and reads 204 as changed", async () => {
    const fn = stubFetch(reply(204, undefined))
    expect(await postPassword({ current: "old one", next: "new one" })).toEqual({
      kind: "ok",
    })
    const [path, init] = fn.mock.calls[0] as unknown as [string, RequestInit]
    expect(path).toBe("/api/v1/auth/password")
    expect(JSON.parse(init.body as string)).toEqual({ current: "old one", new: "new one" })
  })

  it("leaves current out for a first password", async () => {
    const fn = stubFetch(reply(204, undefined))
    await postPassword({ next: "first password here" })
    const [, init] = fn.mock.calls[0] as unknown as [string, RequestInit]
    expect(JSON.parse(init.body as string)).toEqual({ new: "first password here" })
  })

  it("reads a 400 with its message and strength result", async () => {
    stubFetch(
      reply(400, {
        error: "weak_password",
        message: "That password is too weak.",
        score: 1,
        feedback: { warning: "This is a top-100 common password.", suggestions: [] },
      }),
    )
    expect(await postPassword({ next: "password1" })).toEqual({
      kind: "refused",
      message: "That password is too weak.",
      score: 1,
    })
  })

  it("falls back to the strength warning, then the body text", async () => {
    stubFetch(reply(400, { feedback: { warning: "Too short." } }))
    expect(await postPassword({ next: "a" })).toMatchObject({ message: "Too short." })
    stubFetch(new Response("current password is wrong", { status: 403 }))
    expect(await postPassword({ current: "a", next: "b" })).toEqual({
      kind: "refused",
      message: "current password is wrong",
      score: null,
    })
  })

  it("reads auth_required as signed out", async () => {
    stubFetch(reply(401, { error: "auth_required" }))
    expect(await postPassword({ current: "a", next: "b" })).toEqual({
      kind: "signed_out",
    })
  })
})

describe("postDismissNoAuthWarning", () => {
  it("posts to the dismiss route", async () => {
    const fn = stubFetch(reply(204, undefined))
    await postDismissNoAuthWarning()
    const [path, init] = fn.mock.calls[0] as unknown as [string, RequestInit]
    expect(path).toBe("/api/v1/auth/dismiss-no-auth-warning")
    expect(init.method).toBe("POST")
  })

  it("throws the server's words on a refusal", async () => {
    stubFetch(new Response("could not write config.toml", { status: 500 }))
    await expect(postDismissNoAuthWarning()).rejects.toThrow(
      "could not write config.toml",
    )
  })
})
