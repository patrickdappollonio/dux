import { describe, expect, it } from "vitest"
import { dirIconKind, fileIconKind } from "./fileIcons"

describe("fileIconKind", () => {
  it("code extensions map to code", () => {
    for (const p of ["main.ts", "lib.rs", "server.go", "app.py"]) {
      expect(fileIconKind(p)).toBe("code")
    }
  })

  it("images map to image", () => {
    expect(fileIconKind("logo.png")).toBe("image")
    expect(fileIconKind("icon.svg")).toBe("image")
  })

  it("config/data map to config, including bare filenames", () => {
    expect(fileIconKind("config.json")).toBe("config")
    expect(fileIconKind("config.toml")).toBe("config")
    expect(fileIconKind("values.yaml")).toBe("config")
    expect(fileIconKind("Dockerfile")).toBe("config")
  })

  it("markdown maps to markdown", () => {
    expect(fileIconKind("notes.md")).toBe("markdown")
    expect(fileIconKind("README.md")).toBe("markdown")
  })

  it("lockfiles map to lock", () => {
    expect(fileIconKind("Cargo.lock")).toBe("lock")
    expect(fileIconKind("package-lock.json")).toBe("lock")
    expect(fileIconKind("go.sum")).toBe("lock")
  })

  it("binary extensions map to binary", () => {
    expect(fileIconKind("module.wasm")).toBe("binary")
    expect(fileIconKind("report.pdf")).toBe("binary")
  })

  it("dotfiles/unknown fall back to file", () => {
    expect(fileIconKind(".bashrc")).toBe("file")
    expect(fileIconKind("noext")).toBe("file")
  })
})

describe("dirIconKind", () => {
  it("returns folder-open, folder, or folder-empty per open/empty state", () => {
    expect(dirIconKind({ open: true, empty: false })).toBe("folder-open")
    expect(dirIconKind({ open: false, empty: false })).toBe("folder")
    expect(dirIconKind({ open: false, empty: true })).toBe("folder-empty")
    // An open dir known to have zero children still reads as visibly distinct
    // ("empty" outranks "open" — nothing to show open or closed).
    expect(dirIconKind({ open: true, empty: true })).toBe("folder-empty")
  })
})
