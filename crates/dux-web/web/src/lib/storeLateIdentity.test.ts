// @vitest-environment jsdom
import { expect, it, vi } from "vitest"

// A reconnect's build read that answers only after the page was covered must not
// reload it: the cover was the decision, and a reload would load whatever answers
// at the address into the tab.
const reloadPage = vi.fn()
vi.mock("./reloadPage", () => ({ reloadPage }))

let holdBuild: ((answer: unknown) => void) | null = null
let holding = false

const ok = (body: unknown) =>
  new Response(JSON.stringify(body), { status: 200, headers: { "content-type": "application/json" } })

vi.stubGlobal(
  "fetch",
  vi.fn(async (url: string) => {
    const u = String(url)
    if (u.includes("/api/v1/auth/status")) {
      return ok({ password_set: false, required_here: false, signed_in: false, instance: "dux-a" })
    }
    if (u.includes("/api/v1/build")) {
      if (holding) {
        return new Promise<Response>((resolve) => {
          holdBuild = (body) => resolve(ok(body))
        })
      }
      return ok({ version: "development", process: "run-1", instance: "dux-a" })
    }
    if (u.includes("/api/v1/workspace")) {
      return ok({ projects: [], sessions: [], terminals: [], sidebar: { groups: [], agentless_start: null } })
    }
    return ok({})
  }),
)
const mem = new Map<string, string>()
vi.stubGlobal("localStorage", {
  getItem: (k: string) => mem.get(k) ?? null,
  setItem: (k: string, v: string) => void mem.set(k, String(v)),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})

const store = await import("./store")
const gate = await import("./authGate")
await vi.waitFor(() => expect(store.getSnapshot().booted).toBe(true))
// The boot's own open.
store.eventsSocket.onOpen()

it("does not reload on a build answer that lands after the page was covered", async () => {
  holding = true
  store.eventsSocket.onOpen()
  await vi.waitFor(() => expect(holdBuild).toBeTypeOf("function"))
  gate.reportOtherDux()
  holdBuild?.({ version: "development", process: "run-2", instance: "dux-a" })
  await new Promise((r) => setTimeout(r, 20))
  expect(reloadPage).not.toHaveBeenCalled()
})
