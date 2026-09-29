import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import {
  ChangesFetchAborted,
  ChangesFetchError,
  fetchChanges,
} from "./changesApi"
import {
  DEFAULT_CHANGES_REQUEST_TIMEOUT_SECONDS,
  publishConnectionTiming,
} from "./connectionTiming"

const DEFAULT_MS = DEFAULT_CHANGES_REQUEST_TIMEOUT_SECONDS * 1000

// A fetch that never answers, the way a half-open connection behaves, and that
// rejects the way a real fetch does once its signal aborts.
function hangingFetch() {
  return vi.fn(
    (_url: string, init?: RequestInit) =>
      new Promise<Response>((_resolve, reject) => {
        init?.signal?.addEventListener("abort", () =>
          reject(new DOMException("aborted", "AbortError")),
        )
      }),
  )
}

beforeEach(() => {
  vi.useFakeTimers()
})

afterEach(() => {
  publishConnectionTiming(undefined)
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

describe("fetchChanges", () => {
  it("gives up on a request that never answers and says so", async () => {
    vi.stubGlobal("fetch", hangingFetch())
    const outcome = fetchChanges("s1").catch((error: unknown) => error)

    await vi.advanceTimersByTimeAsync(DEFAULT_MS)

    const error = await outcome
    expect(error).toBeInstanceOf(ChangesFetchError)
    expect((error as ChangesFetchError).status).toBe(0)
    expect((error as Error).message).toMatch(
      new RegExp(`within ${DEFAULT_CHANGES_REQUEST_TIMEOUT_SECONDS} seconds`),
    )
  })

  it("gives up on the configured deadline, and names it", async () => {
    publishConnectionTiming({ changes_request_timeout_seconds: 5 })
    vi.stubGlobal("fetch", hangingFetch())
    const outcome = fetchChanges("s1").catch((error: unknown) => error)

    await vi.advanceTimersByTimeAsync(4_999)
    let settled = false
    void outcome.then(() => (settled = true))
    await Promise.resolve()
    expect(settled).toBe(false)
    await vi.advanceTimersByTimeAsync(1)

    const error = await outcome
    expect(error).toBeInstanceOf(ChangesFetchError)
    expect((error as Error).message).toMatch(/within 5 seconds/)
  })

  it("reports a caller's abort as an abort, not as a failure to show", async () => {
    vi.stubGlobal("fetch", hangingFetch())
    const controller = new AbortController()
    const outcome = fetchChanges("s1", controller.signal).catch(
      (error: unknown) => error,
    )

    controller.abort()

    expect(await outcome).toBeInstanceOf(ChangesFetchAborted)
  })

  it("does not time out a request that answered", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({
        ok: true,
        status: 200,
        json: async () => ({ rev: 1, staged: [], unstaged: [] }),
      })),
    )
    const response = await fetchChanges("s1")
    await vi.advanceTimersByTimeAsync(DEFAULT_MS * 2)
    expect(response.rev).toBe(1)
  })
})
