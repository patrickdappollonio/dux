import { describe, expect, it } from "vitest"

import {
  isImagePreviewPath,
  isSvgPath,
  previewKind,
} from "./editorPreview"

// The two predicates split the image world deliberately:
// - isImagePreviewPath: raster (and other non-SVG) images, which NEVER fetch
//   /read (the 5 MiB /read cap fires before the binary flag exists, so an
//   image tab would otherwise park on a spinner) and render from /raw.
// - previewKind: text formats with a draft-accurate preview TOGGLE (markdown
//   via react-markdown, SVG via a Blob URL); SVGs open in Monaco as text.

describe("isSvgPath", () => {
  it("matches .svg case-insensitively", () => {
    expect(isSvgPath("icons/logo.svg")).toBe(true)
    expect(isSvgPath("LOGO.SVG")).toBe(true)
  })

  it("rejects non-svg paths and svg-ish names", () => {
    expect(isSvgPath("logo.png")).toBe(false)
    expect(isSvgPath("logo.svg.bak")).toBe(false)
    expect(isSvgPath(".svg")).toBe(false) // dotfile, not an extension
  })
})

describe("isImagePreviewPath", () => {
  it("matches raster image extensions", () => {
    for (const p of [
      "logo.png",
      "photo.jpg",
      "photo.JPEG",
      "anim.gif",
      "pic.webp",
      "fav.ico",
      "old.bmp",
      "new.avif",
    ]) {
      expect(isImagePreviewPath(p), p).toBe(true)
    }
  })

  it("excludes SVG: it opens in Monaco as editable text instead", () => {
    expect(isImagePreviewPath("icons/logo.svg")).toBe(false)
  })

  it("rejects everything that is not an image", () => {
    for (const p of ["main.rs", "notes.md", "archive.zip", "Makefile"]) {
      expect(isImagePreviewPath(p), p).toBe(false)
    }
  })
})

describe("previewKind", () => {
  it("markdown paths preview as markdown", () => {
    expect(previewKind("README.md")).toBe("markdown")
    expect(previewKind("docs/page.mdx")).toBe("markdown")
  })

  it("svg paths preview as svg", () => {
    expect(previewKind("icons/logo.svg")).toBe("svg")
  })

  it("everything else has no preview", () => {
    expect(previewKind("main.rs")).toBe(null)
    expect(previewKind("logo.png")).toBe(null) // raster: image pane, not a toggle
  })
})
