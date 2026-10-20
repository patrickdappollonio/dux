// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, render } from "@testing-library/react"

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
    <ChunkBoundary>
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
  })
})
