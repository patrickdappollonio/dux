import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

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

  it("opens when the server cannot say, so a network failure is the offline overlay's to report", async () => {
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

  it("shows the blocked page for a blocked address", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => json(403, { error: "blocked", where: "config.toml" })),
    )
    const g = await load()
    await g.initAuthGate()
    expect(g.getAuthPhase()).toEqual({ kind: "blocked", where: "config.toml" })
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

  it("4403 shows the blocked page and learns where from the status route", async () => {
    const g = await load()
    await g.initAuthGate()
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => json(403, { error: "blocked", where: "config.toml" })),
    )
    g.reportSocketAuthClose(4403)
    expect(g.getAuthPhase().kind).toBe("blocked")
    await vi.waitFor(() => {
      expect(g.getAuthPhase()).toEqual({ kind: "blocked", where: "config.toml" })
    })
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
    loginReply = () => json(403, { error: "blocked", where: "config.toml" })
    const g = await load()
    await g.initAuthGate()
    await g.signIn("x")
    expect(g.getAuthPhase()).toEqual({ kind: "blocked", where: "config.toml" })
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
    expect(await g.signOut()).toEqual({ kind: "unreachable" })
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
