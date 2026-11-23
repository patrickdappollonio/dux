// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"

import { installBrowserHistory } from "@/test/browserHistory"

// The New agent form while the server has not yet answered its create, on the
// real store and a history with the Navigation API.
installBrowserHistory()

// The Changes pane's own wait, set short to show the create does not follow it.
vi.mock("@/lib/connectionTiming", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/connectionTiming")>()),
  changesRequestTimeoutMs: () => 60,
}))

const warnings: string[] = []
const warningIds: Array<string | undefined> = []
const errors: string[] = []
vi.mock("@/lib/notify", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/notify")>()
  return {
    ...actual,
    notifyWarning: (...args: Parameters<typeof actual.notifyWarning>) => {
      warnings.push(actual.noticeText(args[0]))
      warningIds.push(args[1]?.id)
      actual.notifyWarning(...args)
    },
    notifyError: (...args: Parameters<typeof actual.notifyError>) => {
      errors.push(actual.noticeText(args[0]))
      actual.notifyError(...args)
    },
  }
})

// Each create is held until the test answers it.
const held: Array<(response: Response) => void> = []
const fetchMock = vi.fn((url: string, init?: RequestInit) => {
  if (String(url).includes("/api/v1/sessions") && init?.method === "POST") {
    return new Promise<Response>((resolve) => held.push(resolve))
  }
  return Promise.reject(new Error("offline test"))
})
vi.stubGlobal("fetch", fetchMock)
vi.spyOn(console, "warn").mockImplementation(() => {})

const store = await import("@/lib/store")
const { CreateAgentDialog } = await import("./CreateAgentDialog")
const { ConfirmUseExistingBranchDialog } = await import("./ConfirmUseExistingBranchDialog")

function Both() {
  return (
    <>
      <CreateAgentDialog />
      <ConfirmUseExistingBranchDialog />
    </>
  )
}

const form = () => screen.queryByRole("dialog", { name: /^New agent/, hidden: true })
const question = () => screen.queryByRole("dialog", { name: /Attach to existing branch/ })
const nameField = () => screen.getByPlaceholderText("Branch name (optional)") as HTMLInputElement
const createButton = () => within(form()!).getByRole("button", { name: /Create agent/ })
const conflict = () =>
  new Response(JSON.stringify({ existing_branch: { name: "feature-old", location: "local" } }), {
    status: 409,
    headers: { "content-type": "application/json" },
  })
const refused = () =>
  new Response("The project is gone.", {
    status: 400,
    headers: { "content-type": "text/plain" },
  })
const started = () =>
  new Response(JSON.stringify({ id: "op-1" }), {
    status: 202,
    headers: { "content-type": "application/json" },
  })

async function openForm() {
  render(<Both />)
  act(() => store.openCreateAgent("p1"))
  await waitFor(() => expect(form()).not.toBeNull())
  fireEvent.change(nameField(), { target: { value: "feature-old" } })
}

// The create's deadline is passed on a fake clock rather than sat out.
const DEADLINE_MS = 30_000
const passDeadline = () => act(() => vi.advanceTimersByTime(DEADLINE_MS))

beforeEach(() => {
  vi.useFakeTimers({ shouldAdvanceTime: true })
  held.length = 0
  warnings.length = 0
  warningIds.length = 0
  errors.length = 0
  fetchMock.mockClear()
  Object.defineProperty(window, "innerWidth", { configurable: true, value: 1280 })
})
afterEach(async () => {
  cleanup()
  act(() => {
    store.closeExistingBranch()
    store.closeCreateAgent()
  })
  // A create left unanswered says so at its deadline, which must not land in
  // the next test.
  passDeadline()
  vi.useRealTimers()
})

const posts = () => fetchMock.mock.calls.filter(([, init]) => init?.method === "POST").length

describe("the New agent form with its create unanswered", () => {
  it("sends one create however often Enter is pressed in the name field", async () => {
    await openForm()
    for (let i = 0; i < 3; i++) fireEvent.keyDown(nameField(), { key: "Enter" })
    await new Promise((resolve) => setTimeout(resolve, 10))
    expect(posts()).toBe(1)
  })

  it("sends one create however often the store is asked", async () => {
    await openForm()
    act(() => {
      store.submitNameDialog("feature-old")
      store.submitNameDialog("feature-old")
    })
    await new Promise((resolve) => setTimeout(resolve, 10))
    expect(posts()).toBe(1)
  })

  it("is freed with the name kept and says so once 30 seconds have passed", async () => {
    await openForm()
    fireEvent.click(createButton())
    await waitFor(() => expect(createButton().getAttribute("aria-busy")).toBe("true"))
    act(() => vi.advanceTimersByTime(DEADLINE_MS - 1000))
    expect(createButton().getAttribute("aria-busy")).toBe("true")
    expect(warnings).toHaveLength(0)
    act(() => vi.advanceTimersByTime(1000))
    expect(createButton().getAttribute("aria-busy")).toBeNull()
    expect(nameField().value).toBe("feature-old")
    expect(warnings).toHaveLength(1)
    expect(warnings[0]).toMatch(/has not answered/)
    expect(warnings[0]).toMatch(/may still appear/)
  })

  it("replaces its warning when a second create also goes unanswered", async () => {
    await openForm()
    fireEvent.click(createButton())
    await waitFor(() => expect(posts()).toBe(1))
    passDeadline()
    fireEvent.click(createButton())
    await waitFor(() => expect(posts()).toBe(2))
    passDeadline()
    expect(warnings).toHaveLength(2)
    expect(warningIds[0]).toBeDefined()
    expect(warningIds[1]).toBe(warningIds[0])
  })

  it.each([
    ["a refusal over an existing branch", conflict],
    ["the create starting", started],
  ] as const)("opens nothing and closes nothing on %s after the wait", async (_answer, answer) => {
    await openForm()
    fireEvent.click(createButton())
    await waitFor(() => expect(posts()).toBe(1))
    passDeadline()
    expect(warnings).toHaveLength(1)
    act(() => held[0](answer()))
    await new Promise((resolve) => setTimeout(resolve, 20))
    expect(question()).toBeNull()
    expect(form()).not.toBeNull()
    expect(nameField().value).toBe("feature-old")
  })

  it("says a refusal over an existing branch that comes after the wait", async () => {
    await openForm()
    fireEvent.click(createButton())
    await waitFor(() => expect(posts()).toBe(1))
    passDeadline()
    expect(warnings).toHaveLength(1)
    act(() => held[0](conflict()))
    await waitFor(() => expect(errors).toHaveLength(1))
    expect(errors[0]).toMatch(/feature-old was not created/)
    expect(errors[0]).toMatch(/already exists/)
    expect(errors[0]).toMatch(/offer to attach/)
    expect(question()).toBeNull()
  })

  it.each([
    ["over an existing branch", conflict, /already exists/],
    ["for another reason", refused, /The project is gone/],
  ] as const)(
    "says a refusal %s of a create the form has stopped waiting for",
    async (_why, answer, reason) => {
      await openForm()
      fireEvent.click(createButton())
      await waitFor(() => expect(posts()).toBe(1))
      act(() => store.closeCreateAgent())
      act(() => store.openCreateAgent("p1"))
      await waitFor(() => expect(form()).not.toBeNull())
      fireEvent.change(nameField(), { target: { value: "feature-new" } })
      fireEvent.click(createButton())
      await waitFor(() => expect(posts()).toBe(2))
      act(() => held[0](answer()))
      await waitFor(() => expect(errors).toHaveLength(1))
      expect(errors[0]).toMatch(/feature-old was not created/)
      expect(errors[0]).toMatch(reason)
      expect(question()).toBeNull()
      expect(nameField().value).toBe("feature-new")
      expect(createButton().getAttribute("aria-busy")).toBe("true")
    },
  )
})
