import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// `store` reads `location`/`localStorage`, registers a `popstate` listener, and
// at module load fires a boot `/api/me` fetch + constructs an `EventsSocket`. Stub
// the minimum so the import succeeds; steer the boot probe to auth-off so the
// store settles cleanly.
//
// Since Phase 6 a generated commit message arrives as a `session.commit_message`
// event over `/ws/events`; the store then GETs `/api/v1/sessions/:id/commit-message`.
// The mock serves that GET from `commitMessages` (404 when a session has none).

let commitMessages: Record<string, string> = {}

const fetchMock = vi.fn(async (url: string) => {
  const u = String(url)
  const m = u.match(/\/api\/v1\/sessions\/([^/]+)\/commit-message$/)
  if (m) {
    const id = decodeURIComponent(m[1])
    if (id in commitMessages) {
      const body = { session_id: id, message: commitMessages[id] }
      return {
        status: 200,
        ok: true,
        json: async () => body,
        text: async () => JSON.stringify(body),
        headers: { get: () => null },
      } as unknown as Response
    }
    return {
      status: 404,
      ok: false,
      json: async () => null,
      text: async () => "no generated commit message for this session",
      headers: { get: () => null },
    } as unknown as Response
  }
  // /api/me (boot probe) + bootstrap/spine: auth off / empty.
  return {
    status: 200,
    ok: true,
    json: async () => ({ auth: "disabled" }),
    text: async () => "{}",
    headers: { get: () => null },
  } as unknown as Response
})

class FakeWebSocket {
  onopen: (() => void) | null = null
  onclose: (() => void) | null = null
  onerror: (() => void) | null = null
  onmessage: (() => void) | null = null
  binaryType = ""
  readyState = 1
  close() {}
  send() {}
}

beforeEach(() => {
  commitMessages = {}
  vi.stubGlobal("location", { host: "localhost:0" })
  vi.stubGlobal("localStorage", {
    getItem: () => null,
    setItem: () => {},
    removeItem: () => {},
  })
  vi.stubGlobal("window", { addEventListener: () => {} })
  vi.stubGlobal("history", { go: () => {} })
  vi.stubGlobal("WebSocket", FakeWebSocket)
  vi.stubGlobal("fetch", fetchMock)
  vi.resetModules()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

async function loadStore() {
  const mod = await import("./store")
  await vi.waitFor(() => {
    expect(mod.getSnapshot().auth.phase).not.toBe("checking")
  })
  return mod
}

// Fire a `session.commit_message` event for `id` exactly as the server pushes it
// on `/ws/events` (the on-connect snapshot rides the same event per session).
type StoreModule = typeof import("./store")
function commitMessageEvent(mod: StoreModule, id: string) {
  mod.eventsSocket.onEvent({ event: "session.commit_message", id })
}

describe("commit-message routing", () => {
  it("fills the draft when the message matches the open dialog's session", async () => {
    const mod = await loadStore()
    commitMessages.s1 = "Generated for s1"
    mod.openCommit("s1")
    commitMessageEvent(mod, "s1")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().commitDraft).toBe("Generated for s1")
    })
  })

  it("ignores a message for a different session (anti-misroute)", async () => {
    const mod = await loadStore()
    // The dialog is open for s1, but a message for s2 arrives (a second tab, or
    // the user switched). It must NOT clobber the s1 draft — the event's id !==
    // the open target, so the store never even fetches.
    mod.openCommit("s1")
    mod.setCommitDraft("hand-typed for s1")
    commitMessages.s2 = "Generated for s2"
    commitMessageEvent(mod, "s2")
    await Promise.resolve()
    expect(mod.getSnapshot().commitDraft).toBe("hand-typed for s1")
  })

  it("drops a message when no dialog is open", async () => {
    const mod = await loadStore()
    // commitTarget is null by default. A late event (dialog already closed) must
    // be dropped, leaving the empty draft untouched.
    commitMessages.s1 = "Generated for s1"
    commitMessageEvent(mod, "s1")
    await Promise.resolve()
    expect(mod.getSnapshot().commitTarget).toBeNull()
    expect(mod.getSnapshot().commitDraft).toBe("")
  })
})

describe("commit-message subscription lifecycle", () => {
  it("openCommit subscribes to the session's commit-message topic", async () => {
    const mod = await loadStore()
    mod.openCommit("s1")
    // Live `session.commit_message` delivery is gated server-side on exactly this
    // fine topic; without the subscription the generated message is dropped.
    expect(mod.eventsSocket.topics).toContain("session:s1:commit-message")
  })

  it("closeCommit unsubscribes the previous commit target's topic", async () => {
    const mod = await loadStore()
    mod.openCommit("s1")
    mod.closeCommit()
    expect(mod.eventsSocket.topics).not.toContain("session:s1:commit-message")
  })

  it("reopening for a different session drops the stale subscription", async () => {
    const mod = await loadStore()
    mod.openCommit("s1")
    mod.openCommit("s2")
    expect(mod.eventsSocket.topics).not.toContain("session:s1:commit-message")
    expect(mod.eventsSocket.topics).toContain("session:s2:commit-message")
  })
})

describe("commit-message connect snapshot (same event path)", () => {
  it("fills an empty draft for the open dialog (reconnect during generation)", async () => {
    const mod = await loadStore()
    // The dialog is open for s1 with an empty draft; the snapshot re-delivers the
    // result as a `session.commit_message` event, which GETs and fills the draft.
    mod.openCommit("s1")
    commitMessages.s1 = "Generated for s1"
    commitMessageEvent(mod, "s1")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().commitDraft).toBe("Generated for s1")
    })
  })

  it("never clobbers an in-progress edit (non-empty draft)", async () => {
    const mod = await loadStore()
    // A stale snapshot must not overwrite a draft the user has started editing —
    // the empty-draft guard short-circuits before the GET.
    mod.openCommit("s1")
    mod.setCommitDraft("hand-typed for s1")
    commitMessages.s1 = "Stale generated message"
    commitMessageEvent(mod, "s1")
    await Promise.resolve()
    expect(mod.getSnapshot().commitDraft).toBe("hand-typed for s1")
  })

  it("ignores a snapshot for a different session", async () => {
    const mod = await loadStore()
    mod.openCommit("s1")
    commitMessages.s2 = "Generated for s2"
    commitMessageEvent(mod, "s2")
    await Promise.resolve()
    expect(mod.getSnapshot().commitDraft).toBe("")
  })

  it("drops a snapshot when no dialog is open", async () => {
    const mod = await loadStore()
    commitMessages.s1 = "Generated for s1"
    commitMessageEvent(mod, "s1")
    await Promise.resolve()
    expect(mod.getSnapshot().commitTarget).toBeNull()
    expect(mod.getSnapshot().commitDraft).toBe("")
  })
})
