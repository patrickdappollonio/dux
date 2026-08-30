import { describe, expect, it } from "vitest"
import {
  formatFrontMatterValue,
  splitFrontMatter,
  type FrontMatterRow,
} from "./frontMatter"

function rowsOf(doc: string): FrontMatterRow[] {
  const split = splitFrontMatter(doc)
  expect(split).not.toBeNull()
  return split!.rows
}

function displayOf(doc: string): Array<[string, string]> {
  return rowsOf(doc).map((row) => [row.key, formatFrontMatterValue(row.value)])
}

describe("splitFrontMatter detection", () => {
  it("returns null when the document has no front matter", () => {
    expect(splitFrontMatter("# Title\n\nbody\n")).toBeNull()
  })

  it("returns null when the fence is not the first line", () => {
    expect(splitFrontMatter("\n---\ntitle: x\n---\n")).toBeNull()
    expect(splitFrontMatter("# H\n\n---\ntitle: x\n---\n")).toBeNull()
  })

  it("returns null when the block is never closed", () => {
    expect(splitFrontMatter("---\ntitle: x\n\nbody\n")).toBeNull()
  })

  it("accepts a `...` closing fence", () => {
    expect(displayOf("---\ntitle: x\n...\nbody\n")).toEqual([["title", "x"]])
  })

  it("reads an empty block as front matter with no rows", () => {
    const split = splitFrontMatter("---\n---\n\n# Body\n")
    expect(split?.rows).toEqual([])
    expect(split?.body.trim()).toBe("# Body")
  })

  it("does not treat a horizontal rule mid-document as a fence", () => {
    expect(splitFrontMatter("intro\n\n---\n\nmore\n")).toBeNull()
  })

  it("strips the block from the body", () => {
    const split = splitFrontMatter("---\ntitle: x\n---\n\n# Body\n\ntext\n")
    expect(split?.body).toBe("\n# Body\n\ntext\n")
  })

  it("handles Windows line endings", () => {
    const doc = "---\r\ntitle: Hello\r\ndraft: true\r\n---\r\n\r\n# Body\r\n"
    const split = splitFrontMatter(doc)
    expect(split?.rows.map((r) => [r.key, formatFrontMatterValue(r.value)])).toEqual([
      ["title", "Hello"],
      ["draft", "true"],
    ])
    expect(split?.body).toContain("# Body")
  })
})

describe("front matter scalars", () => {
  it("reads plain strings, numbers, booleans and null", () => {
    const doc = [
      "---",
      "title: Hello world",
      "count: 42",
      "ratio: -1.5",
      "draft: true",
      "published: false",
      "author: null",
      "editor: ~",
      "reviewer:",
      "---",
      "",
    ].join("\n")
    expect(rowsOf(doc).map((r) => r.value)).toEqual([
      "Hello world",
      42,
      -1.5,
      true,
      false,
      null,
      null,
      null,
    ])
  })

  it("keeps colons inside quoted strings", () => {
    const doc = '---\ntitle: "dux: a terminal UI"\nsub: \'time: 10:30\'\n---\n'
    expect(displayOf(doc)).toEqual([
      ["title", "dux: a terminal UI"],
      ["sub", "time: 10:30"],
    ])
  })

  it("unescapes a double-quoted string and unwraps doubled single quotes", () => {
    const doc = "---\na: \"say \\\"hi\\\"\"\nb: 'it''s here'\n---\n"
    expect(displayOf(doc)).toEqual([
      ["a", 'say "hi"'],
      ["b", "it's here"],
    ])
  })

  it("keeps a quoted number as a string", () => {
    expect(rowsOf('---\nversion: "1.0"\n---\n')[0].value).toBe("1.0")
  })

  it("drops a trailing comment but keeps a URL fragment", () => {
    expect(displayOf("---\ntitle: Hello # a note\nurl: http://h/p#frag\n---\n")).toEqual([
      ["title", "Hello"],
      ["url", "http://h/p#frag"],
    ])
  })

  it("skips comment and blank lines inside the block", () => {
    expect(displayOf("---\n# a comment\n\ntitle: x\n---\n")).toEqual([["title", "x"]])
  })
})

describe("front matter lists", () => {
  it("reads an inline list", () => {
    expect(rowsOf("---\ntags: [rust, web, tui]\n---\n")[0].value).toEqual([
      "rust",
      "web",
      "tui",
    ])
  })

  it("reads an inline list with a quoted comma", () => {
    expect(displayOf('---\ntags: ["a, b", c]\n---\n')).toEqual([["tags", "a, b, c"]])
  })

  it("reads an empty inline list", () => {
    expect(displayOf("---\ntags: []\n---\n")).toEqual([["tags", ""]])
  })

  it("reads a block list and joins it inline", () => {
    const doc = "---\ntags:\n  - rust\n  - web\n---\n"
    expect(rowsOf(doc)[0].value).toEqual(["rust", "web"])
    expect(displayOf(doc)).toEqual([["tags", "rust, web"]])
  })
})

describe("front matter nested maps", () => {
  it("flattens one level into parent.child rows", () => {
    const doc = "---\nauthor:\n  name: Ada\n  age: 36\ntitle: x\n---\n"
    expect(displayOf(doc)).toEqual([
      ["author.name", "Ada"],
      ["author.age", "36"],
      ["title", "x"],
    ])
  })

  it("reads a list nested under a map key", () => {
    const doc = "---\nauthor:\n  tags:\n    - a\n    - b\n---\n"
    expect(displayOf(doc)).toEqual([["author.tags", "a, b"]])
  })
})

describe("front matter fallbacks", () => {
  it("shows an unterminated inline list as raw text", () => {
    expect(displayOf("---\ntags: [a, b\n---\n")).toEqual([["tags", "[a, b"]])
  })

  it("shows a flow map as raw text", () => {
    expect(displayOf("---\nauthor: {name: Ada}\n---\n")).toEqual([
      ["author", "{name: Ada}"],
    ])
  })

  it("shows a deeply nested block as raw text rather than throwing", () => {
    const doc = "---\na:\n  b:\n    c: 1\n    d: 2\n---\n"
    expect(displayOf(doc)).toEqual([["a.b", "c: 1 d: 2"]])
  })

  it("folds a literal block scalar", () => {
    expect(displayOf("---\nnote: |\n  line one\n  line two\n---\n")).toEqual([
      ["note", "line one\nline two"],
    ])
  })

  it("never throws on junk", () => {
    const junk = "---\n\t\t\n: : :\n- lonely\n%%%\n---\n"
    expect(() => splitFrontMatter(junk)).not.toThrow()
  })
})
