// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

// A lazy chunk that will not load sends the boundary to reload the tab. That
// reload must land on this dux again, never on whatever else answers at the
// address now, so it waits for the status route to say so.

let statusReply: () => Response
const reload = vi.fn()

beforeEach(() => {
  vi.resetModules()
  reload.mockClear()
  sessionStorage.clear()
  vi.stubGlobal(
    "fetch",
    vi.fn(async () => statusReply()),
  )
  Object.defineProperty(window, "location", {
    configurable: true,
    value: { ...window.location, reload },
  })
  vi.spyOn(console, "error").mockImplementation(() => {})
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})

function Broken(): never {
  throw new Error("Failed to fetch dynamically imported module")
}

async function failAChunk(): Promise<typeof import("@/lib/authGate")> {
  const gate = await import("@/lib/authGate")
  const { ChunkBoundary } = await import("./ChunkBoundary")
  render(
    <ChunkBoundary area="pane">
      <Broken />
    </ChunkBoundary>,
  )
  await vi.waitFor(() => expect(fetch).toHaveBeenCalled())
  for (let i = 0; i < 10; i++) await Promise.resolve()
  return gate
}

const json = (body: unknown) =>
  new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } })

describe("ChunkBoundary", () => {
  it("reloads once the status route confirms this is dux", async () => {
    statusReply = () => json({ password_set: false, required_here: false, signed_in: false })
    await failAChunk()
    await vi.waitFor(() => expect(reload).toHaveBeenCalledTimes(1))
  })

  it("does not reload into something that is not dux, and covers the page instead", async () => {
    statusReply = () =>
      new Response("<html><body>someone else</body></html>", {
        status: 200,
        headers: { "content-type": "text/html" },
      })
    const gate = await failAChunk()
    expect(reload).not.toHaveBeenCalled()
    expect(gate.getAuthPhase()).toEqual({ kind: "not_dux" })
  })

  it("does not reload when the status route cannot be reached", async () => {
    statusReply = () => {
      throw new TypeError("network")
    }
    await failAChunk()
    expect(reload).not.toHaveBeenCalled()
    expect(screen.getByText("dux needs a reload")).toBeTruthy()
  })

  it("says any other error is this pane's, shows it, and leaves reloading to the button", async () => {
    statusReply = () => json({ password_set: false, required_here: false, signed_in: false })
    const { ChunkBoundary } = await import("./ChunkBoundary")
    function Throws(): never {
      throw new Error("Invalid 1 panel layout: 100%, 0%")
    }
    render(
      <ChunkBoundary area="pane">
        <Throws />
      </ChunkBoundary>,
    )
    expect(screen.getByText("This pane hit an error")).toBeTruthy()
    expect(screen.getByText("Invalid 1 panel layout: 100%, 0%")).toBeTruthy()
    expect(screen.queryByText("dux needs a reload")).toBeNull()
    for (let i = 0; i < 10; i++) await Promise.resolve()
    expect(fetch).not.toHaveBeenCalled()
    screen.getByRole("button", { name: "Reload" }).click()
    expect(reload).toHaveBeenCalledTimes(1)
  })

  it.each([
    "Failed to fetch dynamically imported module: https://x/assets/a.js",
    "error loading dynamically imported module: https://x/assets/a.js",
    "Importing a module script failed.",
    "Unable to preload CSS for /assets/a.css",
  ])("treats %j as an out-of-date tab, not as the area's own error", async (message) => {
    statusReply = () => json({ password_set: false, required_here: false, signed_in: false })
    const { ChunkBoundary } = await import("./ChunkBoundary")
    function Fails(): never {
      throw new Error(message)
    }
    render(
      <ChunkBoundary area="pane">
        <Fails />
      </ChunkBoundary>,
    )
    expect(screen.getByText("dux needs a reload")).toBeTruthy()
    expect(screen.queryByRole("button", { name: /Copy details/ })).toBeNull()
  })

  it.each([
    ["pane", "This pane hit an error", true],
    ["editor", "This part of the editor hit an error", false],
    ["dialog", "This dialog hit an error", false],
  ] as const)("names the %s it covers and copies the error", async (area, title, mentionsAgents) => {
    const { ChunkBoundary } = await import("./ChunkBoundary")
    const writeText = vi.fn(async () => {})
    Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } })
    function Throws(): never {
      throw new Error("kaput")
    }
    render(
      <ChunkBoundary area={area}>
        <Throws />
      </ChunkBoundary>,
    )
    expect(screen.getByText(title)).toBeTruthy()
    expect(screen.getByText(title).parentElement!.textContent!.includes("agents and terminals")).toBe(
      mentionsAgents,
    )
    screen.getByRole("button", { name: /Copy details/ }).click()
    await vi.waitFor(() => expect(writeText).toHaveBeenCalled())
    expect((writeText.mock.calls[0] as unknown as [string])[0]).toContain("kaput")
  })
})
