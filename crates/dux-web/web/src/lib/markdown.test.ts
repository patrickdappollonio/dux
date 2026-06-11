import { describe, expect, it } from "vitest"

import { isMarkdownPath } from "./markdown"

describe("isMarkdownPath", () => {
  it("matches common markdown extensions", () => {
    expect(isMarkdownPath("README.md")).toBe(true)
    expect(isMarkdownPath("docs/guide.markdown")).toBe(true)
    expect(isMarkdownPath("notes.mdown")).toBe(true)
    expect(isMarkdownPath("a.mkd")).toBe(true)
    expect(isMarkdownPath("component.mdx")).toBe(true)
  })

  it("is case-insensitive", () => {
    expect(isMarkdownPath("README.MD")).toBe(true)
    expect(isMarkdownPath("Guide.Markdown")).toBe(true)
  })

  it("rejects non-markdown files", () => {
    expect(isMarkdownPath("main.rs")).toBe(false)
    expect(isMarkdownPath("index.ts")).toBe(false)
    expect(isMarkdownPath("style.css")).toBe(false)
    expect(isMarkdownPath("noextension")).toBe(false)
  })

  it("does not match a bare extension-like name without the dot", () => {
    expect(isMarkdownPath("md")).toBe(false)
    expect(isMarkdownPath("readme-md")).toBe(false)
  })
})
