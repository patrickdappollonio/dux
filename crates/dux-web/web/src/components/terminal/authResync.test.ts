import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// A terminal that stayed open across a sign-out asks its resize coordinator to
// re-assert its size once the page is signed in again: the window may have
// changed under the login page, and every resize sent meanwhile was refused.
// Through the coordinator, never around it, so the ownership rules (a watcher
// sends nothing; an unowned pty is not claimed by a page that merely came back)
// decide as they do for every other resize.

function json(body: unknown): Response {
  return new Response(JSON.stringify(body), { status: 200 })
}

let signedIn = true

beforeEach(() => {
  signedIn = true
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: string) =>
      String(input).endsWith("/auth/login")
        ? new Response(null, { status: 204 })
        : json({ password_set: true, required_here: true, signed_in: signedIn }),
    ),
  )
  vi.resetModules()
})

afterEach(() => {
  vi.unstubAllGlobals()
})

async function load() {
  const gate = await import("@/lib/authGate")
  const { resyncOnSignIn } = await import("./authResync")
  await gate.initAuthGate()
  return { gate, resyncOnSignIn }
}

describe("resyncOnSignIn", () => {
  it("asks the coordinator to re-assert the size when the page signs in again", async () => {
    const { gate, resyncOnSignIn } = await load()
    const resync = vi.fn()
    const stop = resyncOnSignIn({ isOpen: () => true, resync })
    signedIn = false
    gate.reportUnauthorized()
    expect(resync).not.toHaveBeenCalled()
    signedIn = true
    await gate.signIn("pw")
    expect(resync).toHaveBeenCalledTimes(1)
    stop()
  })

  it("leaves a closed socket to its own reopen, which asserts the size anyway", async () => {
    const { gate, resyncOnSignIn } = await load()
    const resync = vi.fn()
    const stop = resyncOnSignIn({ isOpen: () => false, resync })
    signedIn = false
    gate.reportUnauthorized()
    signedIn = true
    await gate.signIn("pw")
    expect(resync).not.toHaveBeenCalled()
    stop()
  })

  it("stops listening when the pane goes", async () => {
    const { gate, resyncOnSignIn } = await load()
    const resync = vi.fn()
    resyncOnSignIn({ isOpen: () => true, resync })()
    signedIn = false
    gate.reportUnauthorized()
    signedIn = true
    await gate.signIn("pw")
    expect(resync).not.toHaveBeenCalled()
  })
})
