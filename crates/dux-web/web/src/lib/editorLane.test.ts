import { afterEach, describe, expect, it, vi } from "vitest"

import {
  acquireLane,
  laneBusy,
  laneKey,
  resetLanes,
  subscribeLanes,
} from "./editorLane"

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
