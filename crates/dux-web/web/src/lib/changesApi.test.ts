import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import {
  CHANGES_FETCH_TIMEOUT_MS,
  ChangesFetchAborted,
  ChangesFetchError,
  fetchChanges,
} from "./changesApi"

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
  vi.useRealTimers()
  vi.unstubAllGlobals()
})

describe("fetchChanges", () => {
  it("gives up on a request that never answers and says so", async () => {
    vi.stubGlobal("fetch", hangingFetch())
    const outcome = fetchChanges("s1").catch((error: unknown) => error)

    await vi.advanceTimersByTimeAsync(CHANGES_FETCH_TIMEOUT_MS)

    const error = await outcome
    expect(error).toBeInstanceOf(ChangesFetchError)
    expect((error as ChangesFetchError).status).toBe(0)
    expect((error as Error).message).toMatch(
      new RegExp(`within ${CHANGES_FETCH_TIMEOUT_MS / 1000} seconds`),
    )
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
    await vi.advanceTimersByTimeAsync(CHANGES_FETCH_TIMEOUT_MS * 2)
    expect(response.rev).toBe(1)
  })
})
