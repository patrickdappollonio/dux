import { describe, expect, it } from "vitest"

import { EDITOR_ICON_PATHS } from "@/lib/editorIcons"
import { OPEN_IN_EDITORS } from "@/lib/editors"

describe("OPEN_IN_EDITORS", () => {
  it("has a unique key and a non-empty label per entry", () => {
    const keys = OPEN_IN_EDITORS.map((e) => e.key)
    expect(new Set(keys).size).toBe(keys.length)
    for (const editor of OPEN_IN_EDITORS) {
      expect(editor.label.length).toBeGreaterThan(0)
    }
  })

  it("ships a bundled brand icon for every listed editor", () => {
    // A key with no bundled path silently renders the neutral fallback glyph, so
    // pin every menu entry to a real simple-icons path — adding an editor without
    // its icon fails here.
    for (const editor of OPEN_IN_EDITORS) {
      expect(
        EDITOR_ICON_PATHS[editor.key],
        `no icon path bundled for "${editor.key}"`,
      ).toBeTruthy()
    }
  })
})
