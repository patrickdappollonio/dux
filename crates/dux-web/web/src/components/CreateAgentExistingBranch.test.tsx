// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, waitFor, within } from "@testing-library/react"

import { installBrowserHistory } from "@/test/browserHistory"

// The New agent form and the question the server's refusal raises over it, on
// the real store and a history with the Navigation API.
const browser = installBrowserHistory()

const created: unknown[] = []
const fetchMock = vi.fn(async (url: string, init?: RequestInit) => {
  const u = String(url)
  if (u.includes("/api/v1/sessions") && init?.method === "POST") {
    const body = JSON.parse(String(init.body)) as { use_existing_branch?: boolean }
    created.push(body)
    if (!body.use_existing_branch) {
      return new Response(
        JSON.stringify({ existing_branch: { name: "feature-old", location: "local" } }),
        { status: 409, headers: { "content-type": "application/json" } },
      )
    }
    return new Response(JSON.stringify({ id: "op-1" }), {
      status: 202,
      headers: { "content-type": "application/json" },
    })
  }
  throw new Error("offline test")
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

async function submitTakenName() {
  render(<Both />)
  act(() => store.openCreateAgent("p1"))
  await waitFor(() => expect(form()).not.toBeNull())
  fireEvent.change(nameField(), { target: { value: "feature-old" } })
  fireEvent.click(within(form()!).getByRole("button", { name: "Create agent" }))
  await waitFor(() => expect(question()).not.toBeNull())
}

beforeEach(() => {
  created.length = 0
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

describe("Attach to existing branch, raised over the New agent form", () => {
  it.each([
    ["Cancel", () => fireEvent.click(within(question()!).getByRole("button", { name: "Cancel" }))],
    ["Escape", () => fireEvent.keyDown(document.activeElement ?? document.body, { key: "Escape" })],
    ["Back", () => act(() => browser.back())],
  ] as const)("returns to the form with what was typed on %s", async (_way, leave) => {
    await submitTakenName()
    expect(form()).not.toBeNull()
    const entries = browser.length
    leave()
    await waitFor(() => expect(question()).toBeNull())
    await new Promise((resolve) => setTimeout(resolve, 20))
    expect(form()).not.toBeNull()
    expect(nameField().value).toBe("feature-old")
    expect(browser.length).toBe(entries)
    expect(store.getSnapshot().createAgentTarget).not.toBeNull()
  })

  it("closes the form and creates on the branch when attaching is confirmed", async () => {
    await submitTakenName()
    fireEvent.click(within(question()!).getByRole("button", { name: "Attach to branch" }))
    await waitFor(() => expect(form()).toBeNull())
    await waitFor(() => expect(created).toHaveLength(2))
    expect(created[1]).toMatchObject({ name: "feature-old", use_existing_branch: true })
  })
})
