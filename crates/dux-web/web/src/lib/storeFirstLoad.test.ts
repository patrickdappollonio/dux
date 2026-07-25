import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { Bootstrap, PendingFirstLoad } from "./bootstrapApi"

// The first-load slice end to end through the real store: the server's automatic
// offer arriving on the bootstrap document, the guards that stop it re-opening
// while the user works, the two on-demand entry points, and the dismissal write.
//
// Driven through a controllable fetch double (the `storeBootstrap.test.ts`
// pattern) so the store's actual boot path runs.

function makeBootstrap(overrides: Partial<Bootstrap> = {}): Bootstrap {
  return {
    available_providers: ["claude"],
    macros: [],
    welcome_tips: [],
    dux_version: "v1.2.3",
    randomize_agent_names_by_default: false,
    gh_available: false,
    pr_banner_position: "top",
    agent_scrollback_lines: 10000,
    show_changes_pane: true,
    always_show_tab_strip: false,
    global_env: {},
    status_clear_seconds: 6,
    website_url: "https://getdux.app",
    welcome_screen: {
      tagline: "One git worktree per coding agent.",
      paragraphs: ["Start by adding a project."],
      steps: [
        { number: 1, title: "Add a project", detail: "Point dux at a repo." },
        { number: 2, title: "Create an agent", detail: "It gets a worktree." },
        { number: 3, title: "Launch", detail: "A real terminal." },
      ],
    },
    ...overrides,
  }
}

const SAMPLE_NOTES = {
  version: "v1.2.3",
  headline: "Quieter plumbing",
  paragraphs: ["A tune-up release."],
  sections: ["Env config", "A website"],
  html_url: "https://github.com/patrickdappollonio/dux/releases/tag/v1.2.3",
}

let bootstrapBody: Bootstrap = makeBootstrap()
let dismissCalls = 0
let dismissShouldFail = false
let notesCalls = 0
let notesShouldFail = false

const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
  const u = String(url)
  if (u.includes("/api/v1/bootstrap")) {
    return {
      ok: true,
      status: 200,
      json: async () => bootstrapBody,
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u.includes("/api/v1/first-load/dismiss")) {
    dismissCalls++
    expect(init?.method).toBe("POST")
    if (dismissShouldFail) {
      return {
        ok: false,
        status: 500,
        text: async () => "could not record the version as seen",
        headers: { get: () => null },
      } as unknown as Response
    }
    return {
      ok: true,
      status: 200,
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  if (u.includes("/api/v1/release-notes")) {
    notesCalls++
    if (notesShouldFail) {
      return {
        ok: false,
        status: 502,
        text: async () => "GitHub is unreachable",
        headers: { get: () => null },
      } as unknown as Response
    }
    return {
      ok: true,
      status: 200,
      json: async () => SAMPLE_NOTES,
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  return {
    status: 200,
    json: async () => ({}),
    text: async () => "",
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

const toastError = vi.fn()
vi.mock("sonner", () => ({
  toast: {
    error: (m: string) => toastError(m),
    success: vi.fn(),
    info: vi.fn(),
    warning: vi.fn(),
    message: vi.fn(),
    dismiss: vi.fn(),
    custom: vi.fn(),
  },
}))

beforeEach(() => {
  bootstrapBody = makeBootstrap()
  dismissCalls = 0
  dismissShouldFail = false
  notesCalls = 0
  notesShouldFail = false
  toastError.mockClear()
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
    expect(mod.getSnapshot().bootstrap).not.toBeNull()
  })
  return mod
}

const welcomePending: PendingFirstLoad = { screen: "welcome", notes: null }
const whatsNewPending: PendingFirstLoad = {
  screen: "whats_new",
  notes: SAMPLE_NOTES,
}

describe("the automatic first-load offer", () => {
  it("opens the welcome screen when the server offers it", async () => {
    bootstrapBody = makeBootstrap({ pending_first_load: welcomePending })
    const mod = await loadStore()

    await vi.waitFor(() => {
      expect(mod.getSnapshot().firstLoad).not.toBeNull()
    })
    const open = mod.getSnapshot().firstLoad
    expect(open?.screen).toBe("welcome")
    // `automatic` is what makes closing it DISMISS it.
    expect(open?.automatic).toBe(true)
    expect(open?.loading).toBe(false)
    expect(open?.notes).toBeNull()
    // Nothing was fetched: the copy rides the bootstrap document.
    expect(notesCalls).toBe(0)
  })

  it("opens the what's-new screen with the notes the server already fetched", async () => {
    bootstrapBody = makeBootstrap({ pending_first_load: whatsNewPending })
    const mod = await loadStore()

    await vi.waitFor(() => {
      expect(mod.getSnapshot().firstLoad).not.toBeNull()
    })
    const open = mod.getSnapshot().firstLoad
    expect(open?.screen).toBe("whats_new")
    expect(open?.automatic).toBe(true)
    expect(open?.notes?.headline).toBe("Quieter plumbing")
    // The automatic path NEVER loads: the server had the notes before it offered
    // the screen, which is why a client can render immediately.
    expect(open?.loading).toBe(false)
    expect(notesCalls).toBe(0)
  })

  it("offers nothing when the server offers nothing", async () => {
    const mod = await loadStore()
    expect(mod.getSnapshot().bootstrap?.pending_first_load ?? null).toBeNull()
    expect(mod.getSnapshot().firstLoad).toBeNull()
  })

  it("dismisses on close, and a later refetch cannot pop it back up", async () => {
    bootstrapBody = makeBootstrap({ pending_first_load: welcomePending })
    const mod = await loadStore()
    await vi.waitFor(() => expect(mod.getSnapshot().firstLoad).not.toBeNull())

    mod.closeFirstLoad()
    expect(mod.getSnapshot().firstLoad).toBeNull()
    await vi.waitFor(() => expect(dismissCalls).toBe(1))
    await vi.waitFor(() =>
      expect(mod.getSnapshot().firstLoadDismissed).toBe(true),
    )

    // A `config.changed` refetch races the server's clear and still carries the
    // pending screen. It must NOT reappear over the user's work.
    mod.eventsSocket.onEvent({ event: "config.changed" })
    await vi.waitFor(() => expect(mod.getSnapshot().bootstrap).not.toBeNull())
    expect(mod.getSnapshot().firstLoad).toBeNull()
    expect(dismissCalls).toBe(1)
  })

  it("does not re-offer over an already-open dialog, so a refetch cannot reset the user's scroll", async () => {
    bootstrapBody = makeBootstrap({ pending_first_load: welcomePending })
    const mod = await loadStore()
    await vi.waitFor(() => expect(mod.getSnapshot().firstLoad).not.toBeNull())

    // The user is reading. Simulate them having opened the OTHER screen on
    // demand, then a refetch arriving.
    mod.openReleaseNotes()
    await vi.waitFor(() => expect(notesCalls).toBe(1))
    const before = mod.getSnapshot().firstLoad
    expect(before?.screen).toBe("whats_new")
    expect(before?.automatic).toBe(false)

    mod.eventsSocket.onEvent({ event: "config.changed" })
    await new Promise((r) => setTimeout(r, 10))
    // Still the on-demand screen, not clobbered back to the welcome offer.
    expect(mod.getSnapshot().firstLoad?.screen).toBe("whats_new")
    expect(mod.getSnapshot().firstLoad?.automatic).toBe(false)
  })

  it("keeps the screen genuinely pending when the dismissal write fails", async () => {
    dismissShouldFail = true
    bootstrapBody = makeBootstrap({ pending_first_load: welcomePending })
    const mod = await loadStore()
    await vi.waitFor(() => expect(mod.getSnapshot().firstLoad).not.toBeNull())

    // The close is optimistic: a failed write must never trap the user behind a
    // modal.
    mod.closeFirstLoad()
    expect(mod.getSnapshot().firstLoad).toBeNull()
    await vi.waitFor(() => expect(toastError).toHaveBeenCalled())
    // ...but the dismissed guard stays OFF, so the screen is still pending for
    // the next load rather than being silently swallowed.
    expect(mod.getSnapshot().firstLoadDismissed).toBe(false)
  })
})

describe("opening a first-load screen on demand", () => {
  it("opens the welcome screen with no fetch and dismisses nothing on close", async () => {
    const mod = await loadStore()

    mod.openWelcomeScreen()
    const open = mod.getSnapshot().firstLoad
    expect(open?.screen).toBe("welcome")
    // NOT automatic: looking something up is not acknowledging this launch's
    // screen, so closing must not stamp the version.
    expect(open?.automatic).toBe(false)
    expect(notesCalls).toBe(0)

    mod.closeFirstLoad()
    expect(mod.getSnapshot().firstLoad).toBeNull()
    await new Promise((r) => setTimeout(r, 10))
    expect(dismissCalls).toBe(0)
  })

  it("opens the what's-new screen in a real loading state, then fills in the notes", async () => {
    const mod = await loadStore()

    mod.openReleaseNotes()
    // Loading is visible IMMEDIATELY: the server may have to reach GitHub.
    const loading = mod.getSnapshot().firstLoad
    expect(loading?.screen).toBe("whats_new")
    expect(loading?.loading).toBe(true)
    expect(loading?.notes).toBeNull()
    expect(loading?.error).toBeNull()

    await vi.waitFor(() => {
      expect(mod.getSnapshot().firstLoad?.loading).toBe(false)
    })
    expect(notesCalls).toBe(1)
    expect(mod.getSnapshot().firstLoad?.notes?.version).toBe("v1.2.3")
    expect(mod.getSnapshot().firstLoad?.error).toBeNull()
    // On-demand, so still not a dismissal.
    mod.closeFirstLoad()
    await new Promise((r) => setTimeout(r, 10))
    expect(dismissCalls).toBe(0)
  })

  it("shows a failed fetch in the dialog AND toasts it, never silently", async () => {
    notesShouldFail = true
    const mod = await loadStore()

    mod.openReleaseNotes()
    await vi.waitFor(() => {
      expect(mod.getSnapshot().firstLoad?.loading).toBe(false)
    })
    const open = mod.getSnapshot().firstLoad
    expect(open?.error).toContain("GitHub is unreachable")
    expect(open?.notes).toBeNull()
    // Both surfaces: the body explains it even after a toast auto-clears.
    expect(toastError).toHaveBeenCalledWith("GitHub is unreachable")
  })

  it("drops a late notes reply after the dialog is closed", async () => {
    const mod = await loadStore()

    mod.openReleaseNotes()
    mod.closeFirstLoad()
    expect(mod.getSnapshot().firstLoad).toBeNull()

    await vi.waitFor(() => expect(notesCalls).toBe(1))
    await new Promise((r) => setTimeout(r, 10))
    // The reply must not resurrect a dialog the user already closed.
    expect(mod.getSnapshot().firstLoad).toBeNull()
  })
})
