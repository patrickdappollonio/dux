import { describe, expect, it } from "vitest"

import { chip, proseText } from "./prose"
import { editorTitleProse, standaloneEditorName } from "./standaloneEditorName"
import type { TerminalView } from "./types"
import type { Spine } from "./workspaceApi"

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

describe("the editor's terminal name", () => {
  it("disambiguates only among the terminal's own owner's terminals", () => {
    // Terminal numbers are per owner, so another project's "Terminal 1" running the same
    // app is a different terminal and must not turn the name into "vim (#1)".
    const vim = (id: string, project_id: string) =>
      ({
        id,
        label: "Terminal 1",
        has_output: true,
        foreground_cmd: "vim",
        owner: { kind: "project", project_id },
      }) as TerminalView
    const spine = {
      sessions: [],
      projects: [],
      terminals: [vim("term-1", "p1"), vim("term-2", "p2")],
    } as unknown as Spine
    const named = standaloneEditorName(
      { kind: "terminal", terminalId: "term-1", owner: { kind: "project", projectId: "p1" } },
      spine,
    )
    expect(named.name).toBe("vim")
  })
})
