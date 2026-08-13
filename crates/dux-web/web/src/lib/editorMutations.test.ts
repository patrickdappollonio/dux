import { describe, expect, it } from "vitest"

import {
  createdMessage,
  deletedMessage,
  movedMessage,
  renamedMessage,
} from "./editorMutations"

// The sentences the editor says after a file mutation lands. Every one of
// them names the ENTRY, because the confirmation is worthless if the user
// cannot tell which file it is about, and the delete dialog in particular is
// already gone from the screen by the time the request settles.

describe("createdMessage", () => {
  it("names the kind and the full path", () => {
    expect(createdMessage("file", "src/new.ts")).toBe("Created file src/new.ts")
    expect(createdMessage("folder", "src/vendor")).toBe(
      "Created folder src/vendor",
    )
  })

  it("keeps the full path for a root-level entry too", () => {
    expect(createdMessage("file", "README.md")).toBe("Created file README.md")
  })
})

describe("renamedMessage", () => {
  it("names the source in full and the destination by its new name", () => {
    // Repeating "src/" on both sides would bury the one word that changed.
    expect(renamedMessage("src/config.toml", "src/config.bak")).toBe(
      "Renamed src/config.toml to config.bak",
    )
  })

  it("handles a root-level rename, where there is no directory to strip", () => {
    expect(renamedMessage("config.toml", "config.bak")).toBe(
      "Renamed config.toml to config.bak",
    )
  })
})

describe("movedMessage", () => {
  it("names the entry and the destination directory", () => {
    expect(movedMessage("notes.md", "docs")).toBe("Moved notes.md to docs/")
    expect(movedMessage("src/notes.md", "docs/old")).toBe(
      "Moved src/notes.md to docs/old/",
    )
  })

  it("gives the worktree root a real word instead of an empty destination", () => {
    expect(movedMessage("docs/notes.md", "")).toBe(
      "Moved docs/notes.md to the worktree root",
    )
  })
})

describe("deletedMessage", () => {
  it("names the kind and the full path", () => {
    expect(deletedMessage("notes.md", false)).toBe("Deleted file notes.md")
    expect(deletedMessage("tools/old", true)).toBe("Deleted folder tools/old")
  })
})
