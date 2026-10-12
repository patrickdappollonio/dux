import { describe, expect, it } from "vitest"

import {
  cursorLabel,
  eolLabel,
  formatFileSize,
  indentationLabel,
  lineCountLabel,
  planStatusBar,
  readEditorStatus,
  type EditorStatusSource,
  type StatusBarInput,
} from "./editorStatus"

// A stand-in for the narrow slice of Monaco's editor the status bar reads.
function fakeEditor(opts: {
  line?: number
  column?: number
  selected?: number
  lines?: number
  eol?: string
  insertSpaces?: boolean
  tabSize?: number
  indentSize?: number
  model?: boolean
}): EditorStatusSource {
  const selected = opts.selected ?? 0
  return {
    getModel: () =>
      opts.model === false
        ? null
        : {
            getLineCount: () => opts.lines ?? 1,
            getEOL: () => opts.eol ?? "\n",
            getOptions: () => ({
              insertSpaces: opts.insertSpaces ?? true,
              tabSize: opts.tabSize ?? 4,
              indentSize: opts.indentSize ?? opts.tabSize ?? 4,
            }),
            getValueLengthInRange: () => selected,
          },
    getPosition: () => ({
      lineNumber: opts.line ?? 1,
      column: opts.column ?? 1,
    }),
    getSelection: () => ({ isEmpty: () => selected === 0 }),
  }
}

describe("reading the editor's live status", () => {
  it("reads the cursor, the line count, the indentation and the line endings off the model", () => {
    expect(
      readEditorStatus(
        fakeEditor({
          line: 12,
          column: 5,
          lines: 240,
          eol: "\r\n",
          insertSpaces: true,
          tabSize: 4,
          indentSize: 2,
        }),
      ),
    ).toEqual({
      line: 12,
      column: 5,
      selected: 0,
      lineCount: 240,
      indentation: { kind: "spaces", size: 2 },
      eol: "CRLF",
    })
  })

  it("reports tabs by their width", () => {
    expect(
      readEditorStatus(fakeEditor({ insertSpaces: false, tabSize: 8 }))
        ?.indentation,
    ).toEqual({ kind: "tabs", size: 8 })
  })

  it("counts the characters a selection covers", () => {
    expect(readEditorStatus(fakeEditor({ selected: 17 }))?.selected).toBe(17)
  })

  it("has nothing to say with no model attached", () => {
    expect(readEditorStatus(fakeEditor({ model: false }))).toBeNull()
  })
})

describe("the status bar's words", () => {
  it("names the cursor the way VS Code does, with a selection when there is one", () => {
    const base = {
      line: 3,
      column: 9,
      selected: 0,
      lineCount: 10,
      indentation: { kind: "spaces" as const, size: 2 },
      eol: "LF" as const,
    }
    expect(cursorLabel(base)).toBe("Ln 3, Col 9")
    expect(cursorLabel({ ...base, selected: 42 })).toBe("Ln 3, Col 9 (42 selected)")
  })

  it("pluralizes the line count and groups its digits", () => {
    expect(lineCountLabel(1)).toBe("1 line")
    expect(lineCountLabel(2)).toBe("2 lines")
    expect(lineCountLabel(12345)).toBe("12,345 lines")
  })

  it("names spaces and tabs", () => {
    expect(indentationLabel({ kind: "spaces", size: 2 })).toBe("Spaces: 2")
    expect(indentationLabel({ kind: "tabs", size: 4 })).toBe("Tabs: 4")
  })

  it("names the two line endings", () => {
    expect(eolLabel("\n")).toBe("LF")
    expect(eolLabel("\r\n")).toBe("CRLF")
  })

  it("formats a size compactly, in binary units", () => {
    expect(formatFileSize(0)).toBe("0 B")
    expect(formatFileSize(1023)).toBe("1023 B")
    expect(formatFileSize(1024)).toBe("1.0 KiB")
    expect(formatFileSize(1536)).toBe("1.5 KiB")
    expect(formatFileSize(5 * 1024 * 1024)).toBe("5.0 MiB")
    expect(formatFileSize(3 * 1024 * 1024 * 1024)).toBe("3.0 GiB")
  })
})

function input(over: Partial<StatusBarInput>): StatusBarInput {
  return {
    tab: { path: "src/a.ts", mode: "file" },
    dirty: false,
    readOnly: false,
    image: false,
    imageDims: null,
    fileReady: true,
    binary: false,
    preview: false,
    diffEditorShown: false,
    size: 2048,
    languageAvailable: true,
    encodingLabel: null,
    mixedEol: false,
    readOnlyReason: null,
    diffHeadEncodingLabel: null,
    ...over,
  }
}

describe("what the status bar carries in each state", () => {
  it("carries nothing with no file open", () => {
    expect(planStatusBar(input({ tab: null }))).toEqual({
      path: null,
      dirty: false,
      readOnly: false,
      size: null,
      dimensions: null,
      editing: false,
      language: false,
      encoding: null,
      mixedEol: false,
      readOnlyReason: null,
    })
  })

  it("carries everything for a text file in the editor", () => {
    expect(planStatusBar(input({ dirty: true }))).toEqual({
      path: "src/a.ts",
      dirty: true,
      readOnly: false,
      size: 2048,
      dimensions: null,
      editing: true,
      language: true,
      encoding: null,
      mixedEol: false,
      readOnlyReason: null,
    })
  })

  it("says read-only for a read-only file", () => {
    expect(planStatusBar(input({ readOnly: true })).readOnly).toBe(true)
  })

  it("carries only the path and the dimensions for an image", () => {
    expect(
      planStatusBar(
        input({
          tab: { path: "logo.png", mode: "file" },
          image: true,
          imageDims: { path: "logo.png", width: 64, height: 32 },
          fileReady: false,
          size: null,
        }),
      ),
    ).toEqual({
      path: "logo.png",
      dirty: false,
      readOnly: false,
      size: null,
      dimensions: { width: 64, height: 32 },
      editing: false,
      language: false,
      encoding: null,
      mixedEol: false,
      readOnlyReason: null,
    })
  })

  it("does not show another image's dimensions", () => {
    expect(
      planStatusBar(
        input({
          tab: { path: "logo.png", mode: "file" },
          image: true,
          imageDims: { path: "old.png", width: 64, height: 32 },
        }),
      ).dimensions,
    ).toBeNull()
  })

  it("carries the path and the size for a binary file, and nothing Monaco would report", () => {
    const plan = planStatusBar(input({ binary: true, size: 900 }))
    expect(plan.path).toBe("src/a.ts")
    expect(plan.size).toBe(900)
    expect(plan.editing).toBe(false)
    expect(plan.language).toBe(false)
  })

  it("waits for the read before it reports a size", () => {
    const plan = planStatusBar(input({ fileReady: false }))
    expect(plan.size).toBeNull()
    expect(plan.editing).toBe(false)
  })

  it("drops the editor's items under a preview, where Monaco is not on screen", () => {
    const plan = planStatusBar(input({ preview: true }))
    expect(plan.editing).toBe(false)
    expect(plan.language).toBe(false)
    expect(plan.size).toBe(2048)
  })

  it("keeps the language picker in diff mode, which highlights by it, and drops the rest", () => {
    const plan = planStatusBar(
      input({ tab: { path: "src/a.ts", mode: "diff" }, diffEditorShown: true }),
    )
    expect(plan).toEqual({
      path: "src/a.ts",
      dirty: false,
      readOnly: false,
      size: null,
      dimensions: null,
      editing: false,
      language: true,
      encoding: null,
      mixedEol: false,
      readOnlyReason: null,
    })
  })

  it("drops the picker in diff mode while the diff editor is not what is on screen", () => {
    expect(
      planStatusBar(
        input({ tab: { path: "src/a.ts", mode: "diff" }, diffEditorShown: false }),
      ).language,
    ).toBe(false)
  })

  it("offers no picker before the language registry has loaded", () => {
    expect(planStatusBar(input({ languageAvailable: false })).language).toBe(false)
  })

  it("names the encoding of a text file that is not plain UTF-8, preview included", () => {
    expect(planStatusBar(input({ encodingLabel: "Windows-1252" })).encoding).toBe(
      "Windows-1252",
    )
    expect(
      planStatusBar(input({ encodingLabel: "UTF-16 LE", preview: true })).encoding,
    ).toBe("UTF-16 LE")
    expect(planStatusBar(input({ encodingLabel: null })).encoding).toBeNull()
  })

  it("names no encoding before the read, for a binary file, an image or a diff", () => {
    const label = { encodingLabel: "Windows-1252" }
    expect(planStatusBar(input({ ...label, fileReady: false })).encoding).toBeNull()
    expect(planStatusBar(input({ ...label, binary: true })).encoding).toBeNull()
    expect(
      planStatusBar(
        input({ ...label, tab: { path: "a.png", mode: "file" }, image: true }),
      ).encoding,
    ).toBeNull()
    expect(
      planStatusBar(
        input({ ...label, tab: { path: "src/a.ts", mode: "diff" }, diffEditorShown: true }),
      ).encoding,
    ).toBeNull()
  })
})

describe("a large file's diff", () => {
  it("names the encoding the head of git's patch was read in", () => {
    const plan = planStatusBar(
      input({
        tab: { path: "big.txt", mode: "diff" },
        diffHeadEncodingLabel: "Windows-1252",
      }),
    )
    expect(plan.encoding).toBe("Windows-1252")
  })
})
