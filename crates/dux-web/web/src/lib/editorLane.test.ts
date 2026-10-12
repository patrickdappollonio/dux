import { afterEach, describe, expect, it, vi } from "vitest"

import {
  acquireLane,
  LaneDeadlineError,
  laneBusy,
  laneKey,
  resetLanes,
  subscribeLanes,
  withLaneDeadline,
} from "./editorLane"

describe("an operation the server never answers", () => {
  afterEach(() => {
    vi.useRealTimers()
    resetLanes()
  })

  /// A request that hangs (a path swapped for a FIFO, a wedged server) would
  /// hold the file's lane for the life of the page. Past the deadline the
  /// operation fails, its lane is given back, and its answer, should it ever
  /// come, reaches nobody.
  it("fails a request past the deadline and gives the lane back", async () => {
    vi.useFakeTimers()
    const key = laneKey("agent:s1", "a.txt")
    const release = acquireLane(key, "reload")!
    let answer!: (v: string) => void
    const request = new Promise<string>((resolve) => {
      answer = resolve
    })
    const seen = vi.fn()
    const outcome = withLaneDeadline(request, "reload", 1000)
      .then(seen)
      .catch((e: unknown) => e)
      .finally(release)
    vi.advanceTimersByTime(1000)
    const error = await outcome
    expect(error).toBeInstanceOf(LaneDeadlineError)
    expect(String(error)).toContain("applied nothing")
    expect(laneBusy(key)).toBeUndefined()
    answer("late")
    await Promise.resolve()
    expect(seen).not.toHaveBeenCalled()
  })

  it("says a save that timed out may or may not have been written", () => {
    expect(new LaneDeadlineError("save", 60_000).message).toContain(
      "may or may not have been written",
    )
  })

  it("passes an answer that comes in time straight through", async () => {
    await expect(withLaneDeadline(Promise.resolve(7), "save", 1000)).resolves.toBe(7)
    await expect(
      withLaneDeadline(Promise.reject(new Error("no")), "save", 1000),
    ).rejects.toThrow("no")
  })
})

describe("one operation per file at a time", () => {
  afterEach(resetLanes)

  it("lets one operation hold a file and turns the next away until it is released", () => {
    const key = laneKey("agent:1", "a.txt")
    const release = acquireLane(key, "save")
    expect(release).not.toBeNull()
    expect(laneBusy(key)).toBe("save")
    expect(acquireLane(key, "reopen")).toBeNull()
    release!()
    expect(laneBusy(key)).toBeUndefined()
    expect(acquireLane(key, "reopen")).not.toBeNull()
  })

  it("keeps files and roots apart", () => {
    acquireLane(laneKey("agent:1", "a.txt"), "save")
    expect(acquireLane(laneKey("agent:1", "b.txt"), "save")).not.toBeNull()
    expect(acquireLane(laneKey("agent:2", "a.txt"), "save")).not.toBeNull()
  })

  it("releases once, so a late second release cannot free a later holder", () => {
    const key = laneKey("agent:1", "a.txt")
    const first = acquireLane(key, "save")!
    first()
    const second = acquireLane(key, "reopen")!
    first()
    expect(laneBusy(key)).toBe("reopen")
    second()
  })

  it("tells subscribers when a file's lane changes", () => {
    const listener = vi.fn()
    const stop = subscribeLanes(listener)
    const release = acquireLane(laneKey("agent:1", "a.txt"), "save")!
    release()
    expect(listener).toHaveBeenCalledTimes(2)
    stop()
  })
})
