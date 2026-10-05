import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { DEFAULT_RECONNECT_ATTEMPT_TIMEOUT_SECONDS } from "./connectionTiming"

// The gate is module state, so every test imports a fresh copy.
type Gate = typeof import("./authGate")

function json(status: number, body: unknown): Response {
  return new Response(status === 204 ? null : JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  })
}

const SIGNED_OUT = {
  password_set: true,
  required_here: true,
  signed_in: false,
  client_class: "network",
  transport_encrypted: false,
}
const SIGNED_IN = { ...SIGNED_OUT, signed_in: true, transport_encrypted: true }
const NO_PASSWORD = {
  password_set: false,
  required_here: false,
  signed_in: false,
  no_auth_warning: true,
}

// A fetch that only ends when its signal aborts it.
function hang(_input: unknown, init?: RequestInit): Promise<Response> {
  return new Promise((_resolve, reject) => {
    init?.signal?.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError")))
  })
}

// A server double answering the auth routes from mutable state.
let statusBody: unknown = SIGNED_IN
let loginReply: () => Response = () => json(204, undefined)
let logoutReply: () => Response = () => json(204, undefined)

const fetchMock = vi.fn(async (input: string) => {
  const url = String(input)
  if (url.endsWith("/api/v1/auth/status")) return json(200, statusBody)
  if (url.endsWith("/api/v1/auth/login")) return loginReply()
  if (url.endsWith("/api/v1/auth/logout")) return logoutReply()
  return json(200, {})
})

async function load(): Promise<Gate> {
  return import("./authGate")
}

beforeEach(() => {
  statusBody = SIGNED_IN
  loginReply = () => json(204, undefined)
  logoutReply = () => json(204, undefined)
  fetchMock.mockClear()
  vi.stubGlobal("fetch", fetchMock)
  vi.resetModules()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe("phaseForStatus", () => {
  it("opens the app when signed in", async () => {
    const g = await load()
    expect(g.phaseForStatus(g.normalizeAuthStatus(SIGNED_IN)).kind).toBe("open")
  })

  it("opens the app when no password applies here", async () => {
    const g = await load()
    expect(g.phaseForStatus(g.normalizeAuthStatus(NO_PASSWORD)).kind).toBe("open")
  })

  it("asks for the password when one is required and this browser has no session", async () => {
    const g = await load()
    expect(g.phaseForStatus(g.normalizeAuthStatus(SIGNED_OUT)).kind).toBe("signed_out")
  })

  it("says auth is broken whatever else the answer holds", async () => {
    const g = await load()
    const s = g.normalizeAuthStatus({ ...SIGNED_IN, auth_broken: "bad hash" })
    expect(g.phaseForStatus(s)).toEqual({ kind: "broken", detail: "bad hash" })
  })
})

describe("initAuthGate", () => {
  it("starts in checking, then opens for a signed-in browser and tells its listeners", async () => {
    const g = await load()
    expect(g.getAuthPhase().kind).toBe("checking")
    const opened = vi.fn()
    g.onAuthOpen(opened)
    await g.initAuthGate()
    expect(g.getAuthPhase().kind).toBe("open")
    expect(opened).toHaveBeenCalledTimes(1)
  })

  it("holds the app back behind the login page when signed out", async () => {
    statusBody = SIGNED_OUT
    const g = await load()
    const opened = vi.fn()
    g.onAuthOpen(opened)
    await g.initAuthGate()
    expect(g.getAuthPhase()).toMatchObject({ kind: "signed_out", reason: "required" })
    expect(opened).not.toHaveBeenCalled()
    expect(g.authPaused()).toBe(true)
  })

  it("boots when the first look fails at once, so the offline overlay says the server is down", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new TypeError("network")
      }),
    )
    const g = await load()
    await g.initAuthGate()
    expect(g.getAuthPhase()).toEqual({ kind: "open", status: null })
  })

  it("says so after the deadline when the first look never answers, and Retry asks again", async () => {
    vi.useFakeTimers()
    vi.stubGlobal("fetch", vi.fn(hang))
    const g = await load()
    const first = g.initAuthGate()
    await vi.advanceTimersByTimeAsync(DEFAULT_RECONNECT_ATTEMPT_TIMEOUT_SECONDS * 1000)
    await first
    expect(g.getAuthPhase()).toEqual({ kind: "unreachable", timedOut: true })
    vi.stubGlobal("fetch", fetchMock)
    await g.retryAuthGate()
    expect(g.getAuthPhase().kind).toBe("open")
    vi.useRealTimers()
  })

  it("opens for a server that answers without the document, an older dux", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response("nope", { status: 404 })))
    const g = await load()
    await g.initAuthGate()
    expect(g.getAuthPhase()).toEqual({ kind: "open", status: null })
  })

  it("shows the blocked page for a blocked address", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => json(403, { error: "blocked" })),
    )
    const g = await load()
    await g.initAuthGate()
    expect(g.getAuthPhase()).toStrictEqual({ kind: "blocked" })
  })

  it("shows the broken page for an invalid auth config", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => json(503, { error: "auth_config_invalid", detail: "bad hash" })),
    )
    const g = await load()
    await g.initAuthGate()
    expect(g.getAuthPhase()).toEqual({ kind: "broken", detail: "bad hash" })
  })
})

describe("reportUnauthorized", () => {
  it("signs the page out, moves the epoch, and asks the server for a fresh status", async () => {
    const g = await load()
    await g.initAuthGate()
    const before = g.authEpoch()
    statusBody = SIGNED_OUT
    g.reportUnauthorized()
    expect(g.getAuthPhase()).toMatchObject({ kind: "signed_out", reason: "expired" })
    expect(g.authEpoch()).toBe(before + 1)
    await vi.waitFor(() => {
      expect(g.getAuthPhase()).toMatchObject({
        kind: "signed_out",
        status: { transport_encrypted: false },
      })
    })
  })

  it("moves the epoch once however many requests report it", async () => {
    const g = await load()
    await g.initAuthGate()
    const before = g.authEpoch()
    g.reportUnauthorized()
    g.reportUnauthorized()
    g.reportUnauthorized()
    expect(g.authEpoch()).toBe(before + 1)
  })
})

describe("socket close codes", () => {
  it("knows its two codes and nothing else", async () => {
    const g = await load()
    expect(g.isAuthCloseCode(4401)).toBe(true)
    expect(g.isAuthCloseCode(4403)).toBe(true)
    expect(g.isAuthCloseCode(4001)).toBe(false)
    expect(g.isAuthCloseCode(1006)).toBe(false)
  })

  it("4401 signs the page out", async () => {
    const g = await load()
    await g.initAuthGate()
    statusBody = SIGNED_OUT
    g.reportSocketAuthClose(4401)
    expect(g.getAuthPhase().kind).toBe("signed_out")
  })

  it("4403 shows the blocked page at once, with nothing left to ask the status route", async () => {
    const g = await load()
    await g.initAuthGate()
    const fetchMock = vi.fn(async () => json(403, { error: "blocked" }))
    vi.stubGlobal("fetch", fetchMock)
    g.reportSocketAuthClose(4403)
    expect(g.getAuthPhase()).toStrictEqual({ kind: "blocked" })
    await Promise.resolve()
    expect(fetchMock).not.toHaveBeenCalled()
  })
})

describe("signIn", () => {
  it("opens the app on success and tells the open listeners again", async () => {
    statusBody = SIGNED_OUT
    const g = await load()
    await g.initAuthGate()
    const opened = vi.fn()
    g.onAuthOpen(opened)
    statusBody = SIGNED_IN
    expect(await g.signIn("correct horse battery staple")).toEqual({ kind: "ok" })
    expect(g.getAuthPhase().kind).toBe("open")
    expect(opened).toHaveBeenCalledTimes(1)
  })

  it("stays signed out on a wrong password", async () => {
    statusBody = SIGNED_OUT
    loginReply = () => json(401, { error: "auth_required" })
    const g = await load()
    await g.initAuthGate()
    expect(await g.signIn("nope")).toEqual({ kind: "wrong" })
    expect(g.getAuthPhase().kind).toBe("signed_out")
  })

  it("moves to the blocked page when the login is refused as blocked", async () => {
    statusBody = SIGNED_OUT
    loginReply = () => json(403, { error: "blocked" })
    const g = await load()
    await g.initAuthGate()
    await g.signIn("x")
    expect(g.getAuthPhase()).toStrictEqual({ kind: "blocked" })
  })

  it("stays on the login page and says why when the browser did not keep the session", async () => {
    statusBody = SIGNED_OUT
    const g = await load()
    await g.initAuthGate()
    const answer = await g.signIn("right password")
    expect(answer).toEqual({ kind: "refused", message: g.COOKIE_NOT_KEPT })
    expect(g.getAuthPhase().kind).toBe("signed_out")
  })

  it("opens even when the follow-up status read fails, because the login said yes", async () => {
    statusBody = SIGNED_OUT
    const g = await load()
    await g.initAuthGate()
    vi.stubGlobal(
      "fetch",
      vi.fn(async (input: string) =>
        String(input).endsWith("/login")
          ? json(204, undefined)
          : Promise.reject(new TypeError("network")),
      ),
    )
    expect(await g.signIn("x")).toEqual({ kind: "ok" })
    expect(g.getAuthPhase().kind).toBe("open")
  })
})

describe("signOut", () => {
  it("ends the session on the server and shows the login page", async () => {
    const g = await load()
    await g.initAuthGate()
    const before = g.authEpoch()
    statusBody = SIGNED_OUT
    expect(await g.signOut()).toEqual({ kind: "ok" })
    expect(g.getAuthPhase()).toMatchObject({ kind: "signed_out", reason: "signed_out" })
    expect(g.authEpoch()).toBe(before + 1)
    expect(fetchMock.mock.calls.some(([u]) => String(u).endsWith("/logout"))).toBe(true)
  })

  it("stays signed in and says so when the server cannot be reached", async () => {
    const g = await load()
    await g.initAuthGate()
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => {
        throw new TypeError("network")
      }),
    )
    expect(await g.signOut()).toEqual({ kind: "unreachable", timedOut: false })
    expect(g.getAuthPhase().kind).toBe("open")
  })
})

describe("probeAfterDrop", () => {
  it("finds a session that ended while the socket was refused at the upgrade", async () => {
    const g = await load()
    await g.initAuthGate()
    statusBody = SIGNED_OUT
    await g.probeAfterDrop()
    expect(g.getAuthPhase()).toMatchObject({ kind: "signed_out", reason: "expired" })
  })

  it("leaves an open page open when the status still says signed in", async () => {
    const g = await load()
    await g.initAuthGate()
    await g.probeAfterDrop()
    expect(g.getAuthPhase().kind).toBe("open")
  })

  it("keeps one probe in flight", async () => {
    const g = await load()
    await g.initAuthGate()
    fetchMock.mockClear()
    await Promise.all([g.probeAfterDrop(), g.probeAfterDrop(), g.probeAfterDrop()])
    expect(fetchMock).toHaveBeenCalledTimes(1)
  })
})

describe("authPaused", () => {
  it("does not pause while the first answer is still coming", async () => {
    const g = await load()
    expect(g.authPaused()).toBe(false)
  })
})

describe("hasSessionToEnd", () => {
  it("is true only for an open page signed in with a password", async () => {
    const g = await load()
    const st = (o: object) => g.normalizeAuthStatus({ password_set: true, signed_in: true, ...o })
    expect(g.hasSessionToEnd({ kind: "open", status: st({}) })).toBe(true)
    expect(g.hasSessionToEnd({ kind: "open", status: st({ signed_in: false }) })).toBe(false)
    expect(g.hasSessionToEnd({ kind: "open", status: st({ password_set: false }) })).toBe(false)
    expect(g.hasSessionToEnd({ kind: "open", status: null })).toBe(false)
    expect(g.hasSessionToEnd({ kind: "checking" })).toBe(false)
  })
})

describe("stale probe answers", () => {
  it("an answer asked before a sign-in does not undo it", async () => {
    const g = await load()
    await g.initAuthGate()
    let release!: () => void
    vi.stubGlobal(
      "fetch",
      vi.fn((input: string) => {
        if (String(input).endsWith("/auth/status")) {
          return new Promise<Response>((resolve) => {
            release = () => resolve(json(200, SIGNED_OUT))
          })
        }
        return fetchMock(input)
      }),
    )
    g.reportUnauthorized()
    await vi.waitFor(() => expect(release).toBeTypeOf("function"))
    const stale = release
    // The sign-in reads its own, fresh status.
    vi.stubGlobal("fetch", fetchMock)
    await g.signIn("pw")
    expect(g.getAuthPhase().kind).toBe("open")
    stale()
    await new Promise((r) => setTimeout(r, 0))
    expect(g.getAuthPhase().kind).toBe("open")
  })

  it("a hung probe does not hold up a fresh one", async () => {
    const g = await load()
    await g.initAuthGate()
    vi.stubGlobal("fetch", vi.fn(hang))
    void g.probeAuth("expired")
    statusBody = SIGNED_OUT
    vi.stubGlobal("fetch", fetchMock)
    await g.probeAuth("expired", { fresh: true })
    expect(g.getAuthPhase().kind).toBe("signed_out")
  })
})

describe("an explicit sign-out", () => {
  it("wins over the socket close and drop probe it causes", async () => {
    const g = await load()
    await g.initAuthGate()
    let finish!: () => void
    vi.stubGlobal(
      "fetch",
      vi.fn((input: string) => {
        if (String(input).endsWith("/auth/logout")) {
          return new Promise<Response>((resolve) => {
            finish = () => resolve(json(204, undefined))
          })
        }
        return Promise.resolve(json(200, SIGNED_OUT))
      }),
    )
    const out = g.signOut()
    await vi.waitFor(() => expect(finish).toBeTypeOf("function"))
    // The server closes this page's sockets while the logout is still out.
    void g.probeAfterDrop()
    g.reportSocketAuthClose(4401)
    finish()
    await out
    await new Promise((r) => setTimeout(r, 0))
    expect(g.getAuthPhase()).toMatchObject({ kind: "signed_out", reason: "signed_out" })
  })
})

describe("the circuit breaker", () => {
  it("backs off when a refusal and the status disagree, then stops on an explicit page", async () => {
    vi.useFakeTimers()
    const g = await load()
    await g.initAuthGate()
    // Status keeps saying signed in; protected routes keep refusing.
    g.reportUnauthorized()
    await vi.advanceTimersByTimeAsync(0)
    expect(g.getAuthPhase().kind).toBe("signed_out")
    await vi.advanceTimersByTimeAsync(1000)
    expect(g.getAuthPhase().kind).toBe("open")

    g.reportUnauthorized()
    await vi.advanceTimersByTimeAsync(1000)
    expect(g.getAuthPhase().kind).toBe("signed_out")
    await vi.advanceTimersByTimeAsync(1000)
    expect(g.getAuthPhase().kind).toBe("open")

    g.reportUnauthorized()
    await vi.advanceTimersByTimeAsync(10_000)
    expect(g.getAuthPhase().kind).toBe("stuck")
    vi.useRealTimers()
  })

  it("trips even when other requests succeed between the refusals", async () => {
    // The loop the breaker exists for: one route refuses, the reopen's own
    // refetches succeed, the route refuses again.
    vi.useFakeTimers()
    const g = await load()
    await g.initAuthGate()
    for (let i = 0; i < g.MAX_AUTH_DISAGREEMENTS; i++) {
      g.reportUnauthorized()
      await vi.advanceTimersByTimeAsync(5000)
      // A refetch that worked: says nothing about the route that refuses.
      if (g.getAuthPhase().kind === "open") {
        const { apiFetch } = await import("./apiFetch")
        expect((await apiFetch("/api/v1/workspace")).ok).toBe(true)
      }
    }
    expect(g.getAuthPhase().kind).toBe("stuck")
    vi.useRealTimers()
  })

  it("forgets the disagreements after a quiet period", async () => {
    vi.useFakeTimers()
    const g = await load()
    await g.initAuthGate()
    for (let i = 0; i < 6; i++) {
      g.reportUnauthorized()
      await vi.advanceTimersByTimeAsync(1000)
      expect(g.getAuthPhase().kind).toBe("open")
      await vi.advanceTimersByTimeAsync(g.AUTH_DISAGREEMENT_QUIET_MS)
    }
    vi.useRealTimers()
  })

  it("Try again from the stuck page asks afresh", async () => {
    vi.useFakeTimers()
    const g = await load()
    await g.initAuthGate()
    for (let i = 0; i < 3; i++) {
      g.reportUnauthorized()
      await vi.advanceTimersByTimeAsync(5000)
    }
    expect(g.getAuthPhase().kind).toBe("stuck")
    await g.retryAuthGate()
    expect(g.getAuthPhase().kind).toBe("open")
    vi.useRealTimers()
  })
})

describe("a password change", () => {
  it("says the password changed even when the server's socket close lands first", async () => {
    const g = await load()
    await g.initAuthGate()
    let finish!: () => void
    vi.stubGlobal(
      "fetch",
      vi.fn((input: string) => {
        if (String(input).endsWith("/auth/password")) {
          return new Promise<Response>((resolve) => {
            finish = () => resolve(json(204, undefined))
          })
        }
        return Promise.resolve(json(200, SIGNED_OUT))
      }),
    )
    const change = g.changePassword({ current: "old", next: "a much longer new one" })
    await vi.waitFor(() => expect(finish).toBeTypeOf("function"))
    // Every browser is signed out, this one's sockets first.
    g.reportSocketAuthClose(4401)
    void g.probeAfterDrop()
    finish()
    expect(await change).toEqual({ kind: "ok" })
    await new Promise((r) => setTimeout(r, 0))
    expect(g.getAuthPhase()).toMatchObject({ kind: "signed_out", reason: "password_changed" })
  })

  it("leaves a connection the password does not apply to open", async () => {
    const g = await load()
    await g.initAuthGate()
    statusBody = NO_PASSWORD
    expect(await g.changePassword({ next: "a first password here" })).toEqual({ kind: "ok" })
    expect(g.getAuthPhase().kind).toBe("open")
  })
})

describe("signing out where no password is needed", () => {
  it("opens again and says so", async () => {
    const g = await load()
    await g.initAuthGate()
    statusBody = NO_PASSWORD
    expect(await g.signOut()).toEqual({ kind: "ok", reopened: true })
    expect(g.getAuthPhase().kind).toBe("open")
  })

  it("goes to the blocked page when the logout is refused as blocked", async () => {
    const g = await load()
    await g.initAuthGate()
    logoutReply = () => json(403, { error: "blocked" })
    expect(await g.signOut()).toEqual({ kind: "gate" })
    expect(g.getAuthPhase()).toStrictEqual({ kind: "blocked" })
  })
})

describe("refreshAuthStatus", () => {
  it("re-reads the status of an open page", async () => {
    const g = await load()
    await g.initAuthGate()
    statusBody = { ...SIGNED_IN, weak_password: true }
    await g.refreshAuthStatus()
    expect(g.getAuthPhase()).toMatchObject({ kind: "open", status: { weak_password: true } })
  })

  it("does nothing on a page that is not open", async () => {
    statusBody = SIGNED_OUT
    const g = await load()
    await g.initAuthGate()
    fetchMock.mockClear()
    await g.refreshAuthStatus()
    expect(fetchMock).not.toHaveBeenCalled()
  })
})

describe("an act's own status read landing after a fresh sign-in", () => {
  // A status read that hangs until released; everything else answers at once.
  function holdNextStatus(body: unknown) {
    let release!: () => void
    let held = false
    vi.stubGlobal(
      "fetch",
      vi.fn((input: string) => {
        const url = String(input)
        if (url.endsWith("/auth/status") && !held) {
          held = true
          return new Promise<Response>((resolve) => {
            release = () => resolve(json(200, body))
          })
        }
        if (url.endsWith("/auth/status")) return Promise.resolve(json(200, SIGNED_IN))
        return Promise.resolve(json(204, undefined))
      }),
    )
    return () => release
  }

  it("does not sign out a session that started after the sign-out's read was asked", async () => {
    const g = await load()
    await g.initAuthGate()
    const releaseOf = holdNextStatus(SIGNED_OUT)
    const out = g.signOut()
    await vi.waitFor(() => expect(releaseOf()).toBeTypeOf("function"))
    // The server's close lands, the page shows the login, and the user signs
    // straight back in before the sign-out's own read has answered.
    g.reportSocketAuthClose(4401)
    await g.signIn("pw")
    expect(g.getAuthPhase().kind).toBe("open")
    releaseOf()()
    await out
    expect(g.getAuthPhase().kind).toBe("open")
  })

  it("does not sign out a session that started after the password change's read was asked", async () => {
    const g = await load()
    await g.initAuthGate()
    const releaseOf = holdNextStatus(SIGNED_OUT)
    const change = g.changePassword({ current: "old", next: "a much longer new one" })
    await vi.waitFor(() => expect(releaseOf()).toBeTypeOf("function"))
    g.reportSocketAuthClose(4401)
    await g.signIn("the new one")
    expect(g.getAuthPhase().kind).toBe("open")
    releaseOf()()
    await change
    expect(g.getAuthPhase().kind).toBe("open")
  })
})

describe("setting the first password from somewhere it then applies", () => {
  it("says the password was set, not that it changed", async () => {
    const g = await load()
    statusBody = NO_PASSWORD
    await g.initAuthGate()
    // From here on the password applies to this connection.
    statusBody = SIGNED_OUT
    expect(await g.changePassword({ next: "a first password here" })).toEqual({ kind: "ok" })
    expect(g.getAuthPhase()).toMatchObject({ kind: "signed_out", reason: "password_set" })
  })
})
