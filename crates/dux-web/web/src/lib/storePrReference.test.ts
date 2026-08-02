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
let resolveReply: unknown = {
  repository: null,
  number: null,
  projects: [],
  uninspected_count: 0,
  uninspected_summary: null,
}
// When set, `POST .../resolve` does NOT answer until the returned resolver is
// called. That is what makes a late reply testable: the browser gets to cancel
// and reopen while the request is genuinely still out.
let deferResolve: ((value: unknown) => void) | null = null
let resolveStatus = 200
let resolveMessage = ""
const posted: { url: string; body: unknown }[] = []

const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
  const u = String(url)
  if (init?.method === "POST") {
    posted.push({ url: u, body: JSON.parse(String(init.body ?? "null")) })
  }
  if (u.includes("/api/v1/pull-requests/resolve")) {
    if (deferResolve) {
      const gate = new Promise<unknown>((done) => {
        deferResolve = done
      })
      const body = await gate
      return {
        ok: true,
        status: 200,
        json: async () => body,
        text: async () => JSON.stringify(body),
        headers: { get: () => "application/json" },
      } as unknown as Response
    }
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
  resolveReply = {
    repository: null,
    number: null,
    projects: [],
    uninspected_count: 0,
    uninspected_summary: null,
  }
  deferResolve = null
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
      uninspected_count: 0,
      uninspected_summary: null,
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
      uninspected_count: 0,
      uninspected_summary: null,
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
      uninspected_count: 0,
      uninspected_summary: null,
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
      uninspected_count: 0,
      uninspected_summary: null,
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
    resolveReply = {
      repository: "acme/widget",
      number: 8,
      projects: [],
      uninspected_count: 0,
      uninspected_summary: null,
    }
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

  it("refuses a bare number in the field and sends nothing at all", async () => {
    const mod = await loadStore()
    // The server would refuse this too, and does (see the Rust end-to-end
    // test). The point of this one is that the trip is never made: a number
    // with no project names no repository, so there is nothing to ask about.
    for (const typed of ["123", "#123", "  42 "]) {
      mod.openCreateAgentFromPr(null)
      mod.setCreateAgentPrInput(typed)
      mod.submitNameDialog("")

      expect(
        posted.some((p) => p.url.includes("/pull-requests/resolve")),
        `${typed} must be refused before any request is sent`,
      ).toBe(false)
      expect(createBodies()).toEqual([])
      expect(toasts).toEqual([])
      const error = mod.getSnapshot().createAgentPrError
      expect(error).toContain("does not say which repository")
      // And it must point at the way out, which is the action right below it.
      expect(error).toContain("choose an existing project")
      // The dialog stays open so the reference can be corrected in place.
      expect(mod.getSnapshot().createAgentTarget).not.toBeNull()
      expect(mod.getSnapshot().createAgentPrResolving).toBe(false)
      mod.closeCreateAgent()
    }
  })

  it("clears the field refusal as soon as the reference is edited", async () => {
    const mod = await loadStore()
    mod.openCreateAgentFromPr(null)
    mod.setCreateAgentPrInput("123")
    mod.submitNameDialog("")
    expect(mod.getSnapshot().createAgentPrError).not.toBeNull()
    mod.setCreateAgentPrInput("acme/widget#123")
    expect(mod.getSnapshot().createAgentPrError).toBeNull()
  })

  it("does not claim there is no checkout when it could not check everything", async () => {
    const mod = await loadStore()
    resolveReply = {
      repository: "github.com/acme/widget",
      number: 3,
      projects: [],
      uninspected_count: 1,
      uninspected_summary: "1 has an address dux could not read",
    }
    mod.openCreateAgentFromPr(null)
    mod.setCreateAgentPrInput("https://github.com/acme/widget/pull/3")
    mod.submitNameDialog("")

    await vi.waitFor(() => {
      expect(toasts.length).toBeGreaterThan(0)
    })
    const said = toasts.map((t) => t.message).join(" ")
    expect(said).toContain("could not check every project")
    expect(said).toContain("address dux could not read")
    expect(said.toLowerCase()).not.toContain("clone")
  })

  it("a resolution the user walked away from acts on nothing", async () => {
    // Submit reference A, cancel while it is still out, open a new dialog for
    // reference B, and only then let A answer. Nothing can recall a reply
    // already in flight, so the only defence is the generation stamp.
    const mod = await loadStore()
    deferResolve = () => {}
    mod.openCreateAgentFromPr(null)
    mod.setCreateAgentPrInput("acme/widget#1")
    mod.submitNameDialog("")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().createAgentPrRequestId).not.toBeNull()
    })
    await vi.waitFor(() => {
      expect(typeof deferResolve).toBe("function")
    })
    const releaseA = deferResolve as (value: unknown) => void

    // Cancel, then open a fresh dialog asking about something else.
    mod.closeCreateAgent()
    expect(mod.getSnapshot().createAgentPrRequestId).toBeNull()
    mod.openCreateAgentFromPr(null)
    mod.setCreateAgentPrInput("acme/gadget#2")

    // Now A finally answers, naming a project it matched.
    releaseA({
      repository: "acme/widget",
      number: 1,
      projects: [{ id: "p1", name: "widget" }],
      uninspected_count: 0,
      uninspected_summary: null,
    })
    await new Promise((r) => setTimeout(r, 0))

    expect(createBodies(), "a superseded answer must create nothing").toEqual([])
    expect(
      mod.getSnapshot().createAgentTarget,
      "and must not close the dialog the user is now looking at",
    ).not.toBeNull()
    expect(
      mod.getSnapshot().createAgentPrInput,
      "nor replace what they typed",
    ).toBe("acme/gadget#2")
    expect(mod.getSnapshot().newAgentPickerOpen).toBe(false)
  })

  it("a stale rejection does not clear or overwrite a newer request", async () => {
    const mod = await loadStore()
    deferResolve = () => {}
    mod.openCreateAgentFromPr(null)
    mod.setCreateAgentPrInput("acme/widget#1")
    mod.submitNameDialog("")
    await vi.waitFor(() => {
      expect(typeof deferResolve).toBe("function")
    })
    const releaseA = deferResolve as (value: unknown) => void

    mod.closeCreateAgent()
    mod.openCreateAgentFromPr(null)
    mod.setCreateAgentPrInput("acme/gadget#2")
    mod.submitNameDialog("")
    await vi.waitFor(() => {
      expect(mod.getSnapshot().createAgentPrRequestId).not.toBeNull()
    })

    // A's reply arrives as a body the client cannot use, taking the catch path.
    releaseA(undefined)
    await new Promise((r) => setTimeout(r, 0))

    expect(toasts, "a stale failure must not be shown").toEqual([])
    expect(
      mod.getSnapshot().createAgentPrResolving,
      "nor clear the newer request's spinner",
    ).toBe(true)
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
