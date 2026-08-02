import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { Spine } from "./spineApi"

// The reference-first from-PR flow, driven through the store the way the dialog
// drives it. The server's resolution is stubbed at `fetch` (it is the thing
// under test on the OTHER side of the wire, where it has its own end-to-end
// tests against real repositories); what is checked here is what the browser
// does with each of the three answers.

function makeSpine(projects: { id: string; name: string }[]): Spine {
  return {
    projects: projects.map((p) => ({ ...p })) as Spine["projects"],
    sessions: [],
    sidebar: { groups: [], agentless_start: null },
  }
}

let spineBody: Spine = makeSpine([])
// What `POST /api/v1/pull-requests/resolve` answers, and whether it refuses.
let resolveReply: unknown = { repository: null, number: null, projects: [] }
let resolveStatus = 200
let resolveMessage = ""
const posted: { url: string; body: unknown }[] = []

const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
  const u = String(url)
  if (init?.method === "POST") {
    posted.push({ url: u, body: JSON.parse(String(init.body ?? "null")) })
  }
  if (u.includes("/api/v1/pull-requests/resolve")) {
    return {
      ok: resolveStatus === 200,
      status: resolveStatus,
      // The client reads the body as TEXT and parses it, so the stub has to
      // answer in text as the server does.
      json: async () => resolveReply,
      text: async () =>
        resolveStatus === 200 ? JSON.stringify(resolveReply) : resolveMessage,
      headers: { get: () => "application/json" },
    } as unknown as Response
  }
  if (u.includes("/api/v1/spine")) {
    return {
      ok: true,
      status: 200,
      json: async () => spineBody,
      text: async () => "",
      headers: { get: () => null },
    } as unknown as Response
  }
  return {
    ok: true,
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

const toasts: { tone: string; message: string }[] = []
vi.mock("sonner", () => {
  const record = (tone: string) => (message: string) => {
    toasts.push({ tone, message })
    return tone
  }
  return {
    toast: Object.assign(record("info"), {
      info: record("info"),
      error: record("error"),
      success: record("success"),
      warning: record("warning"),
      loading: record("loading"),
      dismiss: () => {},
      custom: () => {},
    }),
  }
})

beforeEach(() => {
  spineBody = makeSpine([
    { id: "p1", name: "widget" },
    { id: "p2", name: "widget-review" },
  ])
  resolveReply = { repository: null, number: null, projects: [] }
  resolveStatus = 200
  resolveMessage = ""
  posted.length = 0
  toasts.length = 0
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
    expect(mod.getSnapshot().spine).not.toBeNull()
  })
  return mod
}

function createBodies() {
  return posted
    .filter((p) => p.url.endsWith("/api/v1/sessions"))
    .map((p) => p.body as Record<string, unknown>)
}

describe("the reference-first from-PR flow", () => {
  it("opens with no project and asks for none", async () => {
    const mod = await loadStore()
    mod.openCreateAgentFromPr(null)
    expect(mod.getSnapshot().createAgentTarget).toEqual({
      kind: "pr",
      projectId: null,
    })
  })

  it("creates the agent when the reference names one project dux has", async () => {
    const mod = await loadStore()
    resolveReply = {
      repository: "github.com/acme/widget",
      number: 17,
      projects: [{ id: "p1", name: "widget" }],
    }
    mod.openCreateAgentFromPr(null)
    mod.setCreateAgentPrInput("https://github.com/acme/widget/pull/17")
    mod.submitNameDialog("")

    await vi.waitFor(() => {
      expect(createBodies()).toEqual([
        {
          kind: "from_pr",
          project_id: "p1",
          pr: "https://github.com/acme/widget/pull/17",
          name: "",
        },
      ])
    })
    expect(mod.getSnapshot().createAgentTarget).toBeNull()
    expect(mod.getSnapshot().newAgentPickerOpen).toBe(false)
  })

  it("names the repository and offers the picker when dux has no project for it", async () => {
    const mod = await loadStore()
    resolveReply = {
      repository: "github.com/acme/unknown",
      number: 3,
      projects: [],
    }
    mod.openCreateAgentFromPr(null)
    mod.setCreateAgentPrInput("https://github.com/acme/unknown/pull/3")
    mod.submitNameDialog("")

    await vi.waitFor(() => {
      expect(mod.getSnapshot().newAgentPickerOpen).toBe(true)
    })
    expect(createBodies()).toEqual([])
    const said = toasts.map((t) => t.message).join(" ")
    expect(said).toContain("github.com/acme/unknown")
    // dux does not clone, and the wording must not imply it might.
    expect(said.toLowerCase()).not.toContain("clone")
    expect(said.toLowerCase()).not.toContain("download")
    expect(mod.getSnapshot().newAgentPickerIntent).toBe("from_pr")
    // Every project is listed, because the user is being asked to point at a
    // checkout they already have.
    expect(mod.getSnapshot().newAgentPickerOnlyIds).toBeNull()
    expect(mod.getSnapshot().pendingPrReference).toBe(
      "https://github.com/acme/unknown/pull/3",
    )
  })

  it("asks which one when the same repository is checked out twice", async () => {
    const mod = await loadStore()
    resolveReply = {
      repository: "acme/widget",
      number: 8,
      projects: [
        { id: "p1", name: "widget" },
        { id: "p2", name: "widget-review" },
      ],
    }
    mod.openCreateAgentFromPr(null)
    mod.setCreateAgentPrInput("acme/widget#8")
    mod.submitNameDialog("")

    await vi.waitFor(() => {
      expect(mod.getSnapshot().newAgentPickerOpen).toBe(true)
    })
    expect(createBodies()).toEqual([])
    // Only the two checkouts of that repository are worth showing.
    expect(mod.getSnapshot().newAgentPickerOnlyIds).toEqual(["p1", "p2"])
    expect(toasts.map((t) => t.message).join(" ")).toContain("acme/widget")
  })

  it("keeps the reference so the project the user picks completes it", async () => {
    const mod = await loadStore()
    resolveReply = {
      repository: "acme/widget",
      number: 8,
      projects: [
        { id: "p1", name: "widget" },
        { id: "p2", name: "widget-review" },
      ],
    }
    mod.openCreateAgentFromPr(null)
    mod.setCreateAgentPrInput("acme/widget#8")
    mod.submitNameDialog("")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().newAgentPickerOpen).toBe(true)
    })

    // The picker row opens the dialog project-first, and the field is already
    // filled in.
    mod.openCreateAgentFromPr("p2")
    expect(mod.getSnapshot().createAgentPrInput).toBe("acme/widget#8")
    expect(mod.getSnapshot().pendingPrReference).toBeNull()

    mod.submitNameDialog("")
    await vi.waitFor(() => {
      expect(createBodies()).toEqual([
        { kind: "from_pr", project_id: "p2", pr: "acme/widget#8", name: "" },
      ])
    })
  })

  it("drops the parked reference when the picker is dismissed without a pick", async () => {
    const mod = await loadStore()
    resolveReply = { repository: "acme/widget", number: 8, projects: [] }
    mod.openCreateAgentFromPr(null)
    mod.setCreateAgentPrInput("acme/widget#8")
    mod.submitNameDialog("")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().newAgentPickerOpen).toBe(true)
    })

    mod.dismissNewAgentPicker()
    expect(mod.getSnapshot().pendingPrReference).toBeNull()
    // So a later from-PR dialog opens empty rather than prefilled with text the
    // user walked away from.
    mod.openCreateAgentFromPr(null)
    expect(mod.getSnapshot().createAgentPrInput).toBe("")
  })

  it("surfaces the server's refusal of a bare number and creates nothing", async () => {
    const mod = await loadStore()
    resolveStatus = 400
    resolveMessage =
      "A pull request number on its own does not say which repository it is in. Paste a link, type owner/repo#123, or choose an existing project first."
    mod.openCreateAgentFromPr(null)
    mod.setCreateAgentPrInput("123")
    mod.submitNameDialog("")

    await vi.waitFor(() => {
      expect(toasts.length).toBeGreaterThan(0)
    })
    expect(toasts[0].tone).toBe("error")
    expect(toasts[0].message).toContain("does not say which repository")
    expect(createBodies()).toEqual([])
    // The dialog stays open so the reference can be corrected in place.
    expect(mod.getSnapshot().createAgentTarget).not.toBeNull()
    expect(mod.getSnapshot().createAgentPrResolving).toBe(false)
  })

  it("the project-first shape still creates directly, resolving nothing", async () => {
    const mod = await loadStore()
    mod.openCreateAgentFromPr("p1")
    mod.setCreateAgentPrInput("#123")
    mod.submitNameDialog("")

    await vi.waitFor(() => {
      expect(createBodies()).toEqual([
        { kind: "from_pr", project_id: "p1", pr: "#123", name: "" },
      ])
    })
    expect(
      posted.some((p) => p.url.includes("/pull-requests/resolve")),
      "a chosen project needs no resolution",
    ).toBe(false)
  })
})
