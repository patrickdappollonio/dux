import { describe, expect, it } from "vitest"

import { baseName } from "./paths"

describe("baseName", () => {
  it("reads the trailing segment, with or without a trailing slash", () => {
    expect(baseName("/home/ada/notes")).toBe("notes")
    expect(baseName("/home/ada/notes/")).toBe("notes")
    expect(baseName("notes")).toBe("notes")
  })

  it("answers the path itself for the filesystem root", () => {
    expect(baseName("/")).toBe("/")
  })
})
