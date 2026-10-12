import { describe, expect, it } from "vitest"

import { chip, proseText } from "./prose"
import { editorTitleProse } from "./standaloneEditorName"

describe("the editor header's sentence", () => {
  it("names an agent as one chip", () => {
    const title = editorTitleProse({ glyph: "agent", name: "fix-login", detail: null })
    expect(title).toEqual(["Editor for ", chip("fix-login")])
  })

  it("names a terminal and where it lives, both as chips", () => {
    const project = editorTitleProse({ glyph: "terminal", name: "zsh", detail: "dux" })
    expect(project).toEqual(["Editor for ", chip("zsh"), " in ", chip("dux")])
    expect(proseText(project)).toBe("Editor for zsh in dux")
    const standalone = editorTitleProse({
      glyph: "terminal",
      name: "zsh",
      detail: "~/code",
    })
    expect(proseText(standalone)).toBe("Editor for zsh in ~/code")
  })
})
