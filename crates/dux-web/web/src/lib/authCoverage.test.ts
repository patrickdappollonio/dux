import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// Every fetch helper the app has, answered 401 by the server: each must sign the
// page out and hand its caller the auth interruption rather than a "could not
// reach the server" of its own. `apiBoundary.test.ts` proves no helper can call
// `fetch` behind the door's back; this proves each one's own error handling
// lets the door's answer through.

function json(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  })
}

let answer: () => Response = () => json(401, { error: "auth_required" })

beforeEach(() => {
  answer = () => json(401, { error: "auth_required" })
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string) =>
      String(input).endsWith("/api/v1/auth/status")
        ? json(200, { password_set: true, required_here: true, signed_in: false })
        : answer(),
    ),
  )
  vi.resetModules()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

type Call = (m: Record<string, Record<string, unknown>>) => Promise<unknown>

// [label, module, call]. The module is imported fresh per test so the gate it
// reports to is the one the test reads.
const HELPERS: [string, string, Call][] = [
  ["bootstrap", "./bootstrapApi", (m) => (m.fetchBootstrap as unknown as () => Promise<unknown>)()],
  ["workspace", "./workspaceApi", (m) => (m.fetchWorkspace as unknown as () => Promise<unknown>)()],
  [
    "changes",
    "./changesApi",
    (m) => (m.fetchChanges as unknown as (id: string) => Promise<unknown>)("s1"),
  ],
  ["browse", "./browseApi", (m) => (m.browseApi.agentName as () => Promise<unknown>)()],
  ["config send", "./configApi", (m) => (m.configApi.reload as () => Promise<unknown>)()],
  [
    "config raw read",
    "./configApi",
    (m) => (m.configApi.readRawConfig as () => Promise<unknown>)(),
  ],
  [
    "tailscale mode",
    "./configApi",
    (m) => (m.configApi.setTailscaleMode as (x: string) => Promise<unknown>)("auto"),
  ],
  [
    "editor files",
    "./fileApi",
    (m) =>
      (m.fileApi.list as (r: unknown) => Promise<unknown>)({ kind: "agent", sessionId: "s1" }),
  ],
  [
    "file drop",
    "./fileDropApi",
    (m) =>
      (
        m.uploadDroppedFile as unknown as (
          f: File,
          o: { pty: string; conn: null },
        ) => Promise<unknown>
      )(new File(["x"], "x.txt"), { pty: "p", conn: null }),
  ],
  [
    "first load",
    "./firstLoadApi",
    (m) => (m.firstLoadApi.dismiss as () => Promise<unknown>)(),
  ],
  [
    "release notes",
    "./firstLoadApi",
    (m) => (m.firstLoadApi.fetchReleaseNotes as () => Promise<unknown>)(),
  ],
  ["git", "./git", (m) => (m.git.stage as (a: string, b: string) => Promise<unknown>)("s1", "a")],
  ["resources", "./resourcesApi", (m) => (m.resourcesApi.get as () => Promise<unknown>)()],
  [
    "sessions (createJsonRequest)",
    "./sessionsApi",
    (m) => (m.sessionsApi.reconnect as (id: string, f: boolean) => Promise<unknown>)("s1", false),
  ],
  [
    "projects (createJsonRequest)",
    "./projectsApi",
    (m) => (m.projectsApi.worktreeCounts as () => Promise<unknown>)(),
  ],
  [
    "tabs (createJsonRequest)",
    "./tabsApi",
    (m) =>
      (m.tabsApi.create as (id: string, p?: string) => Promise<unknown>)("s1", "claude"),
  ],
  [
    "terminals (createJsonRequest)",
    "./terminalsApi",
    (m) => (m.terminalsApi.createStandalone as () => Promise<unknown>)(),
  ],
]

describe("every fetch helper reports a 401 to the gate", () => {
  it.each(HELPERS)("%s", async (_label, path, call) => {
    const gate = await import("./authGate")
    const { isAuthInterruption } = await import("./apiFetch")
    const mod = (await import(/* @vite-ignore */ path)) as Record<
      string,
      Record<string, unknown>
    >
    const err = await call(mod).catch((e: unknown) => e)
    expect(isAuthInterruption(err)).toBe(true)
    expect(gate.getAuthPhase().kind).toBe("signed_out")
  })
})

describe("the server-identity read", () => {
  it("throws the interruption on a 401 rather than answering unknown, and the gate has heard it", async () => {
    const gate = await import("./authGate")
    const { fetchServerIdentity } = await import("./buildApi")
    const { isAuthInterruption } = await import("./apiFetch")
    const err = await fetchServerIdentity().then(
      () => null,
      (e: unknown) => e,
    )
    // Unknown would open the terminals' attach gate; nothing was checked.
    expect(isAuthInterruption(err)).toBe(true)
    expect(gate.getAuthPhase().kind).toBe("signed_out")
  })
})

describe("a helper while signed out", () => {
  it("sends nothing and reports the interruption, not a network failure", async () => {
    const gate = await import("./authGate")
    await gate.initAuthGate()
    expect(gate.getAuthPhase().kind).toBe("signed_out")
    const fetchMock = vi.mocked(fetch)
    fetchMock.mockClear()
    const { fetchWorkspace } = await import("./workspaceApi")
    const { isAuthInterruption } = await import("./apiFetch")
    const err = await fetchWorkspace().catch((e: unknown) => e)
    expect(isAuthInterruption(err)).toBe(true)
    expect(fetchMock).not.toHaveBeenCalled()
  })
})

describe("the protected auth routes report a 401 to the gate too", () => {
  it("logout", async () => {
    const gate = await import("./authGate")
    const { postLogout } = await import("./authActions")
    expect(await postLogout()).toEqual({ kind: "ok" })
    expect(gate.getAuthPhase().kind).toBe("signed_out")
  })

  it("password", async () => {
    const gate = await import("./authGate")
    const { postPassword } = await import("./authActions")
    expect(await postPassword({ current: "a", next: "b" })).toEqual({ kind: "signed_out" })
    expect(gate.getAuthPhase().kind).toBe("signed_out")
  })

  it("dismissing the no-password warning", async () => {
    const gate = await import("./authGate")
    const { postDismissNoAuthWarning } = await import("./authActions")
    const { isAuthInterruption } = await import("./apiFetch")
    const err = await postDismissNoAuthWarning().catch((e: unknown) => e)
    expect(isAuthInterruption(err)).toBe(true)
    expect(gate.getAuthPhase().kind).toBe("signed_out")
  })
})
