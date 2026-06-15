import { describe, expect, it } from "vitest"

import {
  isMarkdownPath,
  markdownAssetUrl,
  resolveWorktreeRelative,
} from "./markdown"

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

describe("resolveWorktreeRelative", () => {
  it("resolves a sibling reference against the file's directory", () => {
    expect(resolveWorktreeRelative("README.md", "assets/logo.png")).toBe(
      "assets/logo.png",
    )
    expect(resolveWorktreeRelative("docs/guide.md", "img/x.png")).toBe(
      "docs/img/x.png",
    )
  })

  it("normalizes . and .. within the worktree", () => {
    expect(resolveWorktreeRelative("docs/guide.md", "./x.png")).toBe("docs/x.png")
    expect(resolveWorktreeRelative("docs/guide.md", "../assets/x.png")).toBe(
      "assets/x.png",
    )
    expect(resolveWorktreeRelative("a/b/c.md", "../../d/e.png")).toBe("d/e.png")
  })

  it("returns null when the reference escapes the worktree root", () => {
    expect(resolveWorktreeRelative("README.md", "../secret.png")).toBeNull()
    expect(
      resolveWorktreeRelative("docs/guide.md", "../../../etc/passwd"),
    ).toBeNull()
  })

  it("leaves external/absolute references alone (null)", () => {
    expect(resolveWorktreeRelative("README.md", "https://x/y.png")).toBeNull()
    expect(resolveWorktreeRelative("README.md", "//cdn/y.png")).toBeNull()
    expect(resolveWorktreeRelative("README.md", "/abs/y.png")).toBeNull()
    expect(
      resolveWorktreeRelative("README.md", "data:image/png;base64,AAAA"),
    ).toBeNull()
  })

  it("drops a query/hash suffix", () => {
    expect(resolveWorktreeRelative("README.md", "assets/x.png?v=2")).toBe(
      "assets/x.png",
    )
    expect(resolveWorktreeRelative("README.md", "assets/x.png#frag")).toBe(
      "assets/x.png",
    )
  })
})

describe("markdownAssetUrl", () => {
  it("builds an auth-gated proxy URL with encoded params for a relative asset", () => {
    expect(markdownAssetUrl("s 1", "docs/guide.md", "img/a b.png")).toBe(
      "/api/file/raw?session_id=s%201&path=docs%2Fimg%2Fa%20b.png",
    )
  })

  it("returns null for external references (no rewrite)", () => {
    expect(markdownAssetUrl("s1", "README.md", "https://x/y.png")).toBeNull()
  })
})
