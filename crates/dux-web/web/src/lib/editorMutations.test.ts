import { describe, expect, it } from "vitest"

import { deletedMessage, movedMessage } from "./editorMutations"

// The sentences the editor says after a file mutation lands. Every one of
// them names the ENTRY, because the confirmation is worthless if the user
// cannot tell which file it is about, and the delete dialog in particular is
// already gone from the screen by the time the request settles.

// A create and an in-place rename say nothing at all: the tree row and the
// open tab carry the result. What is left here is the pair that does speak.

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
