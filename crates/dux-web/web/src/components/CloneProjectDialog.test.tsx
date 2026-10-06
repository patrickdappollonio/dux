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
