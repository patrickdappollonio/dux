import { describe, expect, it } from "vitest"

import { baseName, standaloneAgentDefaultName } from "./paths"

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

// SHARED VECTORS with dux-core `git.rs`
// `the_default_standalone_name_collapses_whitespace_and_never_ends_up_empty`.
// The dialog promises this name in its placeholder, so the browser has to
// derive the same string the server will store. A case that changes on one side
// and not the other fails a test on the other side.
describe("standaloneAgentDefaultName", () => {
  it("names the folder", () => {
    expect(standaloneAgentDefaultName("/home/ada/notes")).toBe("notes")
    expect(standaloneAgentDefaultName("/home/ada/notes/")).toBe("notes")
  })

  it("collapses runs of whitespace and trims the ends", () => {
    expect(standaloneAgentDefaultName("/home/ada/My   Notes ")).toBe("My Notes")
  })

  it("falls back to a fixed word when nothing usable is left", () => {
    expect(standaloneAgentDefaultName("/")).toBe("Standalone agent")
    expect(standaloneAgentDefaultName("/home/ada/   ")).toBe("Standalone agent")
    expect(standaloneAgentDefaultName("")).toBe("Standalone agent")
  })
})
