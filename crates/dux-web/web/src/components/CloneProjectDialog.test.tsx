// @vitest-environment jsdom
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest"
import {
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
} from "@testing-library/react"

// The dialog runs against the real store: what it promises is how typing, the
// destination and the submit answer each other, and the store is where that
// lives. Only the network is faked.

// Replies the fake server gives, in order; a held one is a promise the test
// settles when it wants the reply to land. Empty queues answer at once.
let projectReplies: (Response | Promise<Response>)[] = []
let nameReplies: (Response | Promise<Response>)[] = []
let browseReplies: (Response | Promise<Response>)[] = []
let posted: unknown[] = []
// What the folder check answers, in order, and what it was asked.
let destinationReplies: (Response | Promise<Response>)[] = []
let checked: unknown[] = []

function held() {
  let release: (r: Response) => void = () => {}
  const promise = new Promise<Response>((resolve) => {
    release = resolve
  })
  return { promise, release }
}

function reply(status: number, body: string) {
  return {
    ok: status >= 200 && status < 300,
    status,
    json: async () => JSON.parse(body),
    text: async () => body,
    headers: { get: () => null },
  } as unknown as Response
}

const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
  const u = String(url)
  if (u.endsWith("/api/v1/browse")) {
    return browseReplies.shift() ?? reply(200, JSON.stringify({ path: "/home/u", entries: [] }))
  }
  if (u.endsWith("/api/v1/agent-name")) {
    return nameReplies.shift() ?? reply(200, JSON.stringify({ name: "quacky-mallard" }))
  }
  // The record a started clone is followed through; these tests are about the
  // dialog, so it is one the server no longer knows.
  if (u.includes("/api/v1/operations/")) {
    return reply(404, JSON.stringify({ error: "unknown_operation" }))
  }
  if (u.endsWith("/api/v1/pull-requests/clone-destination")) {
    checked.push(JSON.parse(String(init?.body)))
    return (
      destinationReplies.shift() ??
      reply(200, JSON.stringify({ state: "free", path: "/home/u/widget" }))
    )
  }
  if (u.endsWith("/api/v1/projects") && init?.method === "POST") {
    posted.push(JSON.parse(String(init.body)))
    return projectReplies.shift() ?? reply(202, JSON.stringify({ op_id: "op-1" }))
  }
  return reply(200, "{}")
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

let store: typeof import("@/lib/store")
let CloneProjectDialog: typeof import("./CloneProjectDialog").CloneProjectDialog

beforeAll(async () => {
  vi.stubGlobal("location", { host: "localhost:0", pathname: "/", search: "", hash: "" })
  vi.stubGlobal("localStorage", {
    getItem: () => null,
    setItem: () => {},
    removeItem: () => {},
  })
  vi.stubGlobal("WebSocket", FakeWebSocket)
  vi.stubGlobal("fetch", fetchMock)
  store = await import("@/lib/store")
  ;({ CloneProjectDialog } = await import("./CloneProjectDialog"))
})

beforeEach(() => {
  posted = []
  projectReplies = []
  nameReplies = []
  browseReplies = []
  destinationReplies = []
  checked = []
})

afterEach(() => {
  store.closeCloneProject()
  cleanup()
})

const address = () => screen.getByLabelText("Repository address") as HTMLInputElement
const destination = () => screen.getByLabelText("Destination folder") as HTMLInputElement
const agentName = () => screen.getByLabelText("Agent name") as HTMLInputElement
const type = (field: HTMLElement, value: string) =>
  fireEvent.change(field, { target: { value } })

async function open() {
  store.openCloneProject()
  render(<CloneProjectDialog />)
  // The start folder arrives from the server; typing before it lands is fine,
  // and the destination catches up when it does.
  await waitFor(() => expect(store.getSnapshot().cloneProject?.startFolder).toBe("/home/u"))
}

describe("CloneProjectDialog", () => {
  it("fills the destination from the address until the destination is edited", async () => {
    await open()
    type(address(), "https://github.com/owner/repo.git")
    expect(destination().value).toBe("/home/u/repo")

    type(address(), "git@github.com:owner/other.git")
    expect(destination().value).toBe("/home/u/other")

    type(destination(), "/srv/checkouts/mine")
    type(address(), "https://github.com/owner/third.git")
    expect(destination().value).toBe("/srv/checkouts/mine")
  })

  it("fills the destination when the start folder lands after the address was typed", async () => {
    store.openCloneProject()
    render(<CloneProjectDialog />)
    type(address(), "https://github.com/owner/repo.git")
    await waitFor(() => expect(destination().value).toBe("/home/u/repo"))
  })

  it("posts the address, destination, agent name and the random-name choice, then closes", async () => {
    await open()
    type(address(), "https://github.com/owner/repo.git")
    type(agentName(), "fixer")
    fireEvent.click(screen.getByRole("button", { name: "Clone" }))

    await waitFor(() => expect(store.getSnapshot().cloneProject).toBeNull())
    expect(posted).toEqual([
      {
        path: "/home/u/repo",
        clone_url: "https://github.com/owner/repo.git",
        agent_name: "fixer",
        random_name: false,
      },
    ])
  })

  it("leaves a reopened dialog alone when an earlier dialog's reply lands late", async () => {
    const first = held()
    const second = held()
    const third = held()
    projectReplies = [first.promise, second.promise, third.promise]
    const clone = screen.getByRole.bind(screen, "button", { name: "Clone" })
    for (const name of ["first", "second", "third"]) {
      if (name !== "first") {
        store.closeCloneProject()
        cleanup()
      }
      await open()
      type(address(), "https://github.com/owner/repo.git")
      type(agentName(), name)
      fireEvent.click(clone())
    }
    await waitFor(() => expect(posted).toHaveLength(3))

    // The first dialog's clone started: that closes the first dialog, which is
    // already gone, and not the one on screen now.
    first.release(reply(202, JSON.stringify({ op_id: "op-a" })))
    // The second dialog's refusal is about the second dialog's answers.
    second.release(reply(400, "The destination folder /home/u/repo is not empty."))
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(store.getSnapshot().cloneProject?.name).toBe("third")
    expect(screen.queryByRole("alert")).toBeNull()
    expect((clone() as HTMLButtonElement).disabled).toBe(true)

    third.release(reply(202, JSON.stringify({ op_id: "op-c" })))
    await waitFor(() => expect(store.getSnapshot().cloneProject).toBeNull())
  })

  it("fills the name from the pet-name box and sends the choice", async () => {
    await open()
    type(address(), "https://github.com/owner/repo.git")
    fireEvent.click(screen.getByRole("checkbox"))
    await waitFor(() => expect(agentName().value).toBe("quacky-mallard"))
    fireEvent.click(screen.getByRole("button", { name: "Clone" }))

    await waitFor(() => expect(store.getSnapshot().cloneProject).toBeNull())
    expect(posted).toMatchObject([{ agent_name: "quacky-mallard", random_name: true }])
  })

  it("shows only the pet name asked for last, never one an earlier dialog asked for", async () => {
    const earlier = held()
    const latest = held()
    nameReplies = [earlier.promise, latest.promise]
    await open()
    fireEvent.click(screen.getByRole("checkbox"))
    store.closeCloneProject()
    cleanup()
    await open()
    fireEvent.click(screen.getByRole("checkbox"))

    earlier.release(reply(200, JSON.stringify({ name: "stale-heron" })))
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(agentName().value).toBe("")
    latest.release(reply(200, JSON.stringify({ name: "fresh-otter" })))
    await waitFor(() => expect(agentName().value).toBe("fresh-otter"))
  })

  it("keeps the dialog open and shows the refusal's sentence, whatever lands after it", async () => {
    projectReplies = [
      reply(400, "The destination folder /home/u/repo is not empty."),
    ]
    const startFolder = held()
    const petName = held()
    browseReplies = [startFolder.promise]
    nameReplies = [petName.promise]
    store.openCloneProject()
    render(<CloneProjectDialog />)
    type(address(), "https://github.com/owner/repo.git")
    type(destination(), "/home/u/repo")
    fireEvent.click(screen.getByRole("checkbox"))
    type(agentName(), "fixer")
    fireEvent.click(screen.getByRole("button", { name: "Clone" }))

    const alert = await screen.findByRole("alert")
    expect(alert.textContent).toBe("The destination folder /home/u/repo is not empty.")
    expect(store.getSnapshot().cloneProject).not.toBeNull()
    expect((screen.getByRole("button", { name: "Clone" }) as HTMLButtonElement).disabled).toBe(false)
    expect(store.getSnapshot().pendingCreateFocus).toBeNull()

    // The start folder and the pet name arriving now are nobody's answer to it.
    startFolder.release(reply(200, JSON.stringify({ path: "/home/u", entries: [] })))
    petName.release(reply(200, JSON.stringify({ name: "quacky-mallard" })))
    await waitFor(() => expect(store.getSnapshot().cloneProject?.startFolder).toBe("/home/u"))
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(screen.getByRole("alert").textContent).toBe(
      "The destination folder /home/u/repo is not empty.",
    )

    // Answering the refusal retires it.
    type(destination(), "/home/u/elsewhere")
    expect(screen.queryByRole("alert")).toBeNull()
  })

  it("cannot be submitted without an address", async () => {
    await open()
    expect((screen.getByRole("button", { name: "Clone" }) as HTMLButtonElement).disabled).toBe(true)
  })
})

describe("CloneProjectDialog for a pull request", () => {
  const offer = {
    address: "https://github.com/acme/widget.git",
    host: "github.com",
    owner_repo: "acme/widget",
    number: 3,
    title: "Fix the thing",
    state: "OPEN",
    head_branch: "fix-thing",
    head_oid: "abc123",
    review_copy: { state: "not_needed" as const },
  }
  const needed = {
    state: "needed" as const,
    reason: "busy",
    branch: "fix-thing",
    copy_branch: "fix-thing-review",
    warning: ["Pushes would land on ", { name: "fix-thing-review", quoted: true }, "."],
  }
  const noFreeName = {
    state: "no_free_name" as const,
    reason: "busy",
    branch: "fix-thing",
    message: ["Every review name for ", { name: "fix-thing", quoted: true }, " is taken."],
  }

  async function openPr(
    extra: Partial<Parameters<typeof store.openCloneProject>[0] & object> = {},
    reviewCopy: unknown = offer.review_copy,
  ) {
    store.openCloneProject({
      reference: "acme/widget#3",
      repository: "github.com/acme/widget",
      incompleteSummary: null,
      hasProjects: false,
      name: "fix-thing",
      offer: { ...offer, review_copy: reviewCopy } as never,
      ...extra,
    })
    render(<CloneProjectDialog />)
    await waitFor(() => expect(checked).toHaveLength(1))
  }

  const primary = (name: string) => screen.getByRole("button", { name }) as HTMLButtonElement

  it("shows the pull request, the address and the folder, with no random-name box", async () => {
    await openPr()
    expect(screen.getByText("New agent from PR")).toBeTruthy()
    const chips = [...document.querySelectorAll("code")].map((c) => c.textContent)
    expect(chips).toEqual(["github.com/acme/widget", "#3", "Fix the thing", "fix-thing"])
    expect(screen.getByText(/Fix the thing/)).toBeTruthy()
    expect((screen.getByLabelText("Repository address") as HTMLInputElement).value).toBe(
      "https://github.com/acme/widget.git",
    )
    expect(destination().value).toBe("/home/u/widget")
    expect(screen.queryByRole("checkbox")).toBeNull()
    expect(screen.queryByText("Choose an existing project")).toBeNull()
  })

  it("says which projects it could not check, and offers the existing projects", async () => {
    await openPr({ incompleteSummary: "1 has an address dux could not read", hasProjects: true })
    expect(document.body.textContent).toContain(
      "dux could not check every project (1 has an address dux could not read)",
    )
    expect(screen.getByText("Choose an existing project")).toBeTruthy()
  })

  it("posts the folder, the name and the pull request, with no address", async () => {
    await openPr()
    fireEvent.click(primary("Clone and create"))
    await waitFor(() => expect(store.getSnapshot().cloneProject).toBeNull())
    expect(checked).toEqual([
      {
        path: "/home/u/widget",
        host: "github.com",
        owner_repo: "acme/widget",
        number: 3,
        head_branch: "fix-thing",
        head_oid: "abc123",
      },
      expect.anything(),
    ])
    expect(posted).toEqual([
      {
        path: "/home/u/widget",
        agent_name: "fix-thing",
        pull_request: { host: "github.com", owner_repo: "acme/widget", number: 3 },
        add_existing: false,
      },
    ])
  })

  it("switches to the add when the folder turns out to be a checkout, and posts nothing until pressed again", async () => {
    await openPr()
    const checkout = () =>
      reply(200, JSON.stringify({ state: "checkout", path: "/home/u/widget" }))
    destinationReplies = [checkout(), checkout()]
    fireEvent.click(primary("Clone and create"))
    await screen.findByRole("button", { name: "Add this folder as a project" })
    expect(posted).toEqual([])
    expect(document.body.textContent).toContain("is already a checkout of")

    fireEvent.click(primary("Add this folder as a project"))
    await waitFor(() => expect(store.getSnapshot().cloneProject).toBeNull())
    expect(posted).toMatchObject([{ add_existing: true }])
  })

  it("shows a refused folder in the dialog and posts nothing", async () => {
    await openPr()
    destinationReplies = [reply(400, "/home/u/widget already holds something that isn't a checkout of github.com/acme/widget. Pick another folder.")]
    fireEvent.click(primary("Clone and create"))
    const alert = await screen.findByRole("alert")
    expect(alert.textContent).toContain("Pick another folder.")
    expect(posted).toEqual([])
    expect(primary("Clone and create").disabled).toBe(false)
  })

  it("cannot be submitted without a name", async () => {
    await openPr({ name: "" })
    expect(primary("Clone and create").disabled).toBe(true)
  })

  it("keeps the reference when choosing an existing project", async () => {
    await openPr({ hasProjects: true })
    fireEvent.click(screen.getByText("Choose an existing project"))
    expect(store.getSnapshot().cloneProject).toBeNull()
    expect(store.getSnapshot().pendingPrReference).toBe("acme/widget#3")
    expect(store.getSnapshot().newAgentPickerOpen).toBe(true)
  })

  describe("when the agent would need a review copy", () => {
    it("blocks the clone until the box is ticked, then posts the agreement it showed", async () => {
      await openPr({}, needed)
      expect(document.body.textContent).toContain('Pushes would land on')
      const box = screen.getByRole("checkbox", { name: "Create a review copy" })
      expect(box.getAttribute("aria-checked")).toBe("false")
      expect(primary("Clone and create").disabled).toBe(true)

      fireEvent.click(box)
      expect(primary("Clone and create").disabled).toBe(false)
      fireEvent.click(primary("Clone and create"))
      await waitFor(() => expect(store.getSnapshot().cloneProject).toBeNull())
      expect(posted).toMatchObject([
        { review_copy: true, review_copy_reason: "busy", review_copy_branch: "fix-thing-review" },
      ])
    })

    it("asks nothing once the name is not the pull request's branch", async () => {
      await openPr({}, needed)
      type(agentName(), "my-own-name")
      expect(screen.queryByRole("checkbox")).toBeNull()
      expect(primary("Clone and create").disabled).toBe(false)
      fireEvent.click(primary("Clone and create"))
      await waitFor(() => expect(store.getSnapshot().cloneProject).toBeNull())
      expect(posted[0]).not.toHaveProperty("review_copy")
    })

    it("takes back the tick and waits when the folder check answers differently", async () => {
      await openPr({}, needed)
      fireEvent.click(screen.getByRole("checkbox", { name: "Create a review copy" }))
      destinationReplies = [
        reply(200, JSON.stringify({ state: "checkout", path: "/home/u/widget", review_copy: { state: "not_needed" } })),
      ]
      fireEvent.click(primary("Clone and create"))
      await screen.findByRole("button", { name: "Add this folder as a project" })
      expect(posted).toEqual([])
      expect(screen.queryByRole("checkbox")).toBeNull()
    })

    it("blocks the clone for good while no review name is free and the name is the branch", async () => {
      await openPr({}, noFreeName)
      expect(document.body.textContent).toContain("Every review name for")
      expect(primary("Clone and create").disabled).toBe(true)
      type(agentName(), "my-own-name")
      expect(primary("Clone and create").disabled).toBe(false)
    })

    it("lets go of a folder's no-free-name answer when the folder changes", async () => {
      destinationReplies = [
        reply(
          200,
          JSON.stringify({ state: "checkout", path: "/home/u/widget", review_copy: noFreeName }),
        ),
      ]
      await openPr()
      await waitFor(() => expect(primary("Add this folder as a project").disabled).toBe(true))
      type(destination(), "/home/u/elsewhere")
      expect(primary("Add this folder as a project").disabled).toBe(false)
    })

    it("asks about a name spelled like a review copy, from the answer for that name", async () => {
      await openPr()
      destinationReplies = [
        reply(
          200,
          JSON.stringify({
            state: "free",
            path: "/home/u/widget",
            review_copy: {
              state: "needed",
              reason: "typed",
              branch: "fix-thing",
              copy_branch: "fix-thing-review",
              warning: ["The name ", { name: "fix-thing-review", quoted: true }, " is a copy."],
            },
          }),
        ),
      ]
      type(agentName(), "fix-thing-review")
      await screen.findByRole("checkbox", { name: "Create a review copy" })
      expect(checked[1]).toMatchObject({ agent_name: "fix-thing-review" })
      expect(primary("Clone and create").disabled).toBe(true)

      fireEvent.click(screen.getByRole("checkbox", { name: "Create a review copy" }))
      destinationReplies = [
        reply(
          200,
          JSON.stringify({
            state: "free",
            path: "/home/u/widget",
            review_copy: {
              state: "needed",
              reason: "typed",
              branch: "fix-thing",
              copy_branch: "fix-thing-review",
              warning: ["The name ", { name: "fix-thing-review", quoted: true }, " is a copy."],
            },
          }),
        ),
      ]
      fireEvent.click(primary("Clone and create"))
      await waitFor(() => expect(store.getSnapshot().cloneProject).toBeNull())
      expect(posted).toMatchObject([
        {
          agent_name: "fix-thing-review",
          review_copy: true,
          review_copy_reason: "typed",
          review_copy_branch: "fix-thing-review",
        },
      ])
    })
  })

  it("drops a press when the name is edited while the folder is being checked", async () => {
    await openPr()
    const check = held()
    destinationReplies = [check.promise]
    fireEvent.click(primary("Clone and create"))
    await waitFor(() => expect(checked).toHaveLength(2))
    type(agentName(), "other-name")
    expect(primary("Clone and create").disabled).toBe(false)
    check.release(reply(200, JSON.stringify({ state: "free", path: "/home/u/widget" })))
    await new Promise((resolve) => setTimeout(resolve, 0))
    expect(posted).toEqual([])
    expect(store.getSnapshot().cloneProject?.submitting).toBe(false)
  })

  it("keeps the button disabled while the clone is being sent, whatever is edited", async () => {
    await openPr()
    const sent = held()
    projectReplies = [sent.promise]
    fireEvent.click(primary("Clone and create"))
    await waitFor(() => expect(posted).toHaveLength(1))
    type(destination(), "/home/u/elsewhere")
    expect(primary("Clone and create").disabled).toBe(true)
    sent.release(reply(202, JSON.stringify({ op_id: "op-1" })))
    await waitFor(() => expect(store.getSnapshot().cloneProject).toBeNull())
    expect(posted).toHaveLength(1)
  })

  it("is not stranded by a start folder that arrives after the press", async () => {
    const start = held()
    browseReplies = [start.promise]
    store.openCloneProject({
      reference: "acme/widget#3",
      repository: "github.com/acme/widget",
      incompleteSummary: null,
      hasProjects: false,
      name: "fix-thing",
      offer: offer as never,
    })
    render(<CloneProjectDialog />)
    type(destination(), "/srv/mine")
    const check = held()
    destinationReplies = [check.promise]
    fireEvent.click(primary("Clone and create"))
    await waitFor(() => expect(checked).toHaveLength(1))
    start.release(reply(200, JSON.stringify({ path: "/home/u", entries: [] })))
    await waitFor(() => expect(store.getSnapshot().cloneProject?.startFolder).toBe("/home/u"))
    check.release(reply(200, JSON.stringify({ state: "free", path: "/srv/mine" })))
    await waitFor(() => expect(store.getSnapshot().cloneProject).toBeNull())
    expect(posted).toMatchObject([{ path: "/srv/mine" }])
    expect(checked).toHaveLength(1)
  })

  it("does not call an empty name an error before anything is typed", async () => {
    await openPr({ name: "" })
    expect(agentName().getAttribute("aria-invalid")).toBe("false")
    expect(primary("Clone and create").disabled).toBe(true)
  })
})
