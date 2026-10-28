import { describe, expect, it } from "vitest"

import {
  canonicalEditorView,
  editorViewFromQuery,
  queryTheater,
  splitRouteQuery,
  withRouteQuery,
  type EditorViewState,
} from "./routeQuery"

const NONE: EditorViewState = {
  preview: false,
  line: null,
  top: null,
  language: null,
  encoding: null,
}

describe("splitting the query tail off a hash", () => {
  it("takes a key=value tail off the position", () => {
    const { hash, params } = splitRouteQuery("#/agent/s1/editor/file/a.md?view=preview&line=3")
    expect(hash).toBe("#/agent/s1/editor/file/a.md")
    expect(params.get("view")).toBe("preview")
    expect(params.get("line")).toBe("3")
  })

  it("leaves a literal ? in a hand-written path alone when the tail is not a query", () => {
    expect(splitRouteQuery("#/agent/s1/editor/file/a?b.md")).toEqual({
      hash: "#/agent/s1/editor/file/a?b.md",
      params: new Map(),
    })
    // One part out of shape is enough to keep the whole string as the path.
    expect(splitRouteQuery("#/agent/s1/editor/file/a?line=3&b").hash).toBe(
      "#/agent/s1/editor/file/a?line=3&b",
    )
    expect(splitRouteQuery("#/agent/s1/editor/file/a?").hash).toBe(
      "#/agent/s1/editor/file/a?",
    )
    // A key=value tail is still the path when a key is not one dux writes.
    expect(splitRouteQuery("#/agent/s1/editor/file/a?b=c.md").hash).toBe(
      "#/agent/s1/editor/file/a?b=c.md",
    )
  })

  it("still takes a known key with a value it cannot use as a query", () => {
    expect(splitRouteQuery("#/agent/s1?view=nonsense").hash).toBe("#/agent/s1")
  })

  it("keeps the first value of a repeated key, undecoded", () => {
    const { params } = splitRouteQuery("#/agent/s1?view=theater&view=preview&lang=%E0")
    expect(params.get("view")).toBe("theater")
    expect(params.get("lang")).toBe("%E0")
  })
})

describe("reading theater from the tail", () => {
  it("is on only when the first view value says theater", () => {
    expect(queryTheater(splitRouteQuery("#/agent/s1?view=theater").params)).toBe(true)
    expect(queryTheater(splitRouteQuery("#/agent/s1?view=preview&view=theater").params)).toBe(
      false,
    )
    expect(queryTheater(splitRouteQuery("#/agent/s1?view=nonsense").params)).toBe(false)
  })
})

describe("reading the editor view from the tail", () => {
  const read = (tail: string, mode: "file" | "diff" = "file", path = "notes.md") =>
    editorViewFromQuery(splitRouteQuery(`#/x?${tail}`).params, mode, path)

  it("reads every field", () => {
    expect(read("view=preview&line=12&top=4&lang=python&enc=Shift_JIS")).toEqual({
      preview: true,
      line: 12,
      top: 4,
      language: "python",
      encoding: "Shift_JIS",
    })
  })

  it("drops a key whose value is invalid and keeps the rest", () => {
    expect(read("line=0&top=x&lang=a%20b&enc=klingon&view=preview")).toEqual({
      ...NONE,
      preview: true,
    })
  })

  it("drops a key whose value has a malformed escape", () => {
    expect(read("lang=%E0%A4&line=7")).toEqual({ ...NONE, line: 7 })
  })

  it("drops a key whose FIRST value is invalid, even with a valid one behind it", () => {
    expect(read("line=abc&line=5")).toBeUndefined()
  })

  it("drops what the file cannot carry", () => {
    // No preview kind.
    expect(read("view=preview", "file", "main.ts")).toBeUndefined()
    // Diff mode has no cursor to put anywhere.
    expect(read("line=3&top=2&lang=python", "diff")).toEqual({ ...NONE, language: "python" })
    // A picture is always its own preview and has no text.
    expect(read("view=preview&line=3&lang=python&enc=UTF-8", "file", "logo.png")).toBeUndefined()
  })
})

describe("writing the tail", () => {
  it("writes the editor view in a fixed order, values encoded", () => {
    expect(
      withRouteQuery("#/editor/agent/s1/file/a.md", {
        theater: false,
        view: { preview: true, line: 9, top: 2, language: "c#", encoding: "UTF-8" },
      }),
    ).toBe("#/editor/agent/s1/file/a.md?view=preview&line=9&top=2&lang=c%23&enc=UTF-8")
  })

  it("writes theater, and nothing on the home address", () => {
    expect(withRouteQuery("#/agent/s1", { theater: true })).toBe("#/agent/s1?view=theater")
    expect(withRouteQuery("", { theater: true })).toBe("")
    expect(withRouteQuery("#/agent/s1", { theater: false })).toBe("#/agent/s1")
  })
})

describe("the canonical view", () => {
  it("is omitted when every field is the default", () => {
    expect(canonicalEditorView("file", "a.md", NONE)).toBeUndefined()
    expect(canonicalEditorView("file", null, { ...NONE, line: 3 })).toBeUndefined()
  })

  it("round-trips through the tail over the canonical domain", () => {
    const cases: Array<["file" | "diff", string, EditorViewState]> = [
      ["file", "a.md", { ...NONE, preview: true }],
      ["file", "a.svg", { ...NONE, preview: true, line: 4, top: 1 }],
      ["file", "a.ts", { ...NONE, line: 120 }],
      ["file", "a.ts", { ...NONE, top: 7 }],
      ["file", "a.ts", { ...NONE, language: "python" }],
      ["file", "a.ts", { ...NONE, encoding: "windows-1252" }],
      ["diff", "a.md", { ...NONE, preview: true, language: "markdown", encoding: "UTF-16LE" }],
    ]
    for (const [mode, path, view] of cases) {
      const hash = withRouteQuery("#/p", { theater: false, view })
      expect(editorViewFromQuery(splitRouteQuery(hash).params, mode, path)).toEqual(view)
    }
  })
})
