// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"
import { agentRoot, type EditorRoot } from "@/lib/editorRoot"

const { default: MarkdownPreview } = await import("./MarkdownPreview")

afterEach(cleanup)

const root: EditorRoot = agentRoot("s1")

function preview(content: string) {
  return render(<MarkdownPreview content={content} root={root} path="a.md" />)
}

// Each row of the front-matter table as [key, value].
function tableRows(): Array<[string, string]> {
  const rows = Array.from(document.querySelectorAll("tbody tr"))
  return rows.map((row) => {
    const cells = Array.from(row.querySelectorAll("th, td"))
    return [cells[0]?.textContent ?? "", cells[1]?.textContent ?? ""]
  })
}

describe("MarkdownPreview front matter", () => {
  it("renders the leading YAML block as a key/value table above the body", () => {
    preview(
      [
        "---",
        "title: Hello world",
        "draft: true",
        "tags: [rust, web]",
        "---",
        "",
        "# Body",
        "",
        "text",
      ].join("\n"),
    )
    const table = screen.getByRole("table")
    expect(table).toBeTruthy()
    expect(tableRows()).toEqual([
      ["title", "Hello world"],
      ["draft", "true"],
      ["tags", "rust, web"],
    ])
    // The table precedes the prose it describes.
    const heading = screen.getByRole("heading", { name: "Body" })
    expect(table.compareDocumentPosition(heading)).toBe(
      Node.DOCUMENT_POSITION_FOLLOWING,
    )
  })

  it("renders the body without the front-matter block", () => {
    const { container } = preview("---\ntitle: x\n---\n\n# Body\n\ntext\n")
    expect(screen.getByRole("heading", { name: "Body" })).toBeTruthy()
    expect(container.textContent).not.toContain("---")
    expect(container.querySelector("hr")).toBeNull()
  })

  it("leaves a document without front matter unchanged", () => {
    const { container } = preview("# Body\n\ntext with a: colon\n")
    expect(container.querySelector("table")).toBeNull()
    expect(container.textContent).toContain("text with a: colon")
  })

  it("renders no table for an empty block but still drops it", () => {
    const { container } = preview("---\n---\n\n# Body\n")
    expect(container.querySelector("table")).toBeNull()
    expect(screen.getByRole("heading", { name: "Body" })).toBeTruthy()
  })

  it("escapes markup in a front-matter value instead of rendering it", () => {
    const { container } = preview(
      '---\ntitle: "<img src=x onerror=alert(1)> **bold**"\n---\n\nbody\n',
    )
    expect(container.querySelector("img")).toBeNull()
    expect(container.querySelector("strong")).toBeNull()
    expect(tableRows()).toEqual([
      ["title", "<img src=x onerror=alert(1)> **bold**"],
    ])
  })
})
