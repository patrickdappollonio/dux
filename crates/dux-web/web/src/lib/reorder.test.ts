import { describe, expect, it } from "vitest"

import { moveItem, ordersMatch, reorderById } from "./reorder"

describe("moveItem", () => {
  it("moves an item forward to the over slot", () => {
    expect(moveItem(["a", "b", "c", "d"], "a", "c")).toEqual([
      "b",
      "c",
      "a",
      "d",
    ])
  })

  it("moves an item backward to the over slot", () => {
    expect(moveItem(["a", "b", "c", "d"], "d", "b")).toEqual([
      "a",
      "d",
      "b",
      "c",
    ])
  })

  it("is a no-op when active equals over", () => {
    const ids = ["a", "b", "c"]
    expect(moveItem(ids, "b", "b")).toEqual(["a", "b", "c"])
  })

  it("returns the original order when an id is missing", () => {
    expect(moveItem(["a", "b"], "x", "a")).toEqual(["a", "b"])
    expect(moveItem(["a", "b"], "a", "x")).toEqual(["a", "b"])
  })

  it("does not mutate the input array", () => {
    const ids = ["a", "b", "c"]
    moveItem(ids, "a", "c")
    expect(ids).toEqual(["a", "b", "c"])
  })
})

describe("ordersMatch", () => {
  it("is true for identical orders", () => {
    expect(ordersMatch(["a", "b"], ["a", "b"])).toBe(true)
  })

  it("is false for different positions", () => {
    expect(ordersMatch(["a", "b"], ["b", "a"])).toBe(false)
  })

  it("is false for different lengths", () => {
    expect(ordersMatch(["a"], ["a", "b"])).toBe(false)
  })

  it("is true for two empty arrays", () => {
    expect(ordersMatch([], [])).toBe(true)
  })
})

describe("reorderById", () => {
  const items = (ids: string[]) => ids.map((id) => ({ id }))

  it("reorders items to match the named order", () => {
    expect(reorderById(items(["a", "b", "c"]), ["c", "a", "b"])).toEqual(
      items(["c", "a", "b"]),
    )
  })

  it("keeps unnamed items in their original slots", () => {
    // Only a and c are named; b is unnamed and keeps its middle slot.
    expect(reorderById(items(["a", "b", "c"]), ["c", "a"])).toEqual(
      items(["c", "b", "a"]),
    )
  })

  it("ignores named ids not present in the list", () => {
    expect(reorderById(items(["a", "b"]), ["b", "ghost", "a"])).toEqual(
      items(["b", "a"]),
    )
  })
})
