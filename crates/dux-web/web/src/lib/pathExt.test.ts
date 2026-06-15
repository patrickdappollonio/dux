import { describe, expect, it } from "vitest"

import { extensionForPath, fileNameForPath } from "@/lib/pathExt"

describe("extensionForPath", () => {
  it("returns the lowercased extension with its leading dot", () => {
    expect(extensionForPath("src/main.rs")).toBe(".rs")
    expect(extensionForPath("App.TSX")).toBe(".tsx")
    expect(extensionForPath("a/b/config.JSON")).toBe(".json")
    expect(extensionForPath("archive.tar.gz")).toBe(".gz")
  })

  it("returns '' for a filename with no extension", () => {
    expect(extensionForPath("Makefile")).toBe("")
    expect(extensionForPath("src/Dockerfile")).toBe("")
  })

  it("treats a leading-dot dotfile as having no extension (dot > 0 guard)", () => {
    expect(extensionForPath(".bashrc")).toBe("")
    expect(extensionForPath("config/.gitignore")).toBe("")
  })

  it("considers only the last path segment", () => {
    // A dot in a directory name must not be mistaken for the file's extension.
    expect(extensionForPath("a.dir/file")).toBe("")
    expect(extensionForPath("weird.path/x.ts")).toBe(".ts")
  })
})

describe("fileNameForPath", () => {
  it("returns the lowercased last segment", () => {
    expect(fileNameForPath("src/Makefile")).toBe("makefile")
    expect(fileNameForPath("Dockerfile")).toBe("dockerfile")
    expect(fileNameForPath("a/b/c.ts")).toBe("c.ts")
  })
})
