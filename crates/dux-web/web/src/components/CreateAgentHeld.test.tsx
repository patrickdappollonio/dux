// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import { installBrowserHistory } from "@/test/browserHistory"

// The New agent form while the server has not yet answered its create, on the
// real store and a history with the Navigation API.
installBrowserHistory()

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
const nameField = () => screen.getByPlaceholderText("Branch name (optional)") as HTMLInputElement

async function openForm() {
  render(<Both />)
  act(() => store.openCreateAgent("p1"))
  await waitFor(() => expect(form()).not.toBeNull())
  fireEvent.change(nameField(), { target: { value: "feature-old" } })
}

beforeEach(() => {
  held.length = 0
  fetchMock.mockClear()
  Object.defineProperty(window, "innerWidth", { configurable: true, value: 1280 })
})
afterEach(async () => {
  cleanup()
  act(() => {
    store.closeExistingBranch()
    store.closeCreateAgent()
  })
  await new Promise((resolve) => setTimeout(resolve, 20))
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
})
