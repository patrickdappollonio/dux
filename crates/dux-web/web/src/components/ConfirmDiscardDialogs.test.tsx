// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { ChangedFileView } from "@/lib/types"

// A folded folder is one row standing for thousands of files. Both discard
// dialogs must say so, in the same words the terminal UI uses: calling a
// 30,000-file folder "1 file" understates exactly what a destructive act is
// about to take, and a repository of its own takes its history with it.

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState }
})

function installBootStubs() {
  const mem = new Map<string, string>()
  vi.stubGlobal("localStorage", {
    getItem: (k: string) => mem.get(k) ?? null,
    setItem: (k: string, v: string) => void mem.set(k, String(v)),
    removeItem: (k: string) => void mem.delete(k),
    clear: () => mem.clear(),
  })
  vi.stubGlobal(
    "fetch",
    vi.fn(() => Promise.reject(new Error("offline test"))),
  )
  vi.stubGlobal(
    "matchMedia",
    vi.fn((query: string) => ({
      matches: false,
      media: query,
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    })),
  )
}
installBootStubs()
const { ConfirmDiscardFileDialog } = await import("./ConfirmDiscardFileDialog")
const { ConfirmDiscardFilesDialog } = await import("./ConfirmDiscardFilesDialog")

afterEach(() => cleanup())

const folder: ChangedFileView = {
  path: "node_modules",
  status: "??",
  additions: 0,
  deletions: 0,
  binary: false,
  diff_excluded: false,
  kind: "directory",
  file_count: 28747,
  nested_repositories: 1,
}
const nested: ChangedFileView = {
  path: "vendor/lib",
  status: "??",
  additions: 0,
  deletions: 0,
  binary: false,
  diff_excluded: false,
  kind: "nested_repository",
}
const plain: ChangedFileView = {
  path: "notes.md",
  status: "??",
  additions: 1,
  deletions: 0,
  binary: false,
  diff_excluded: false,
}

function openOn(path: string) {
  mockState = {
    discardTarget: { sessionId: "s1", path, untracked: true },
    changes: {
      sessionId: "s1",
      phase: "loaded",
      rev: 1,
      staged: [],
      unstaged: [folder, nested, plain],
      error: null,
    },
  } as unknown as DuxState
  render(<ConfirmDiscardFileDialog />)
}

function dialogText(): string {
  return (screen.getByRole("dialog").textContent ?? "").replace(/\s+/g, " ")
}

describe("the single discard dialog", () => {
  it("names a folder with its trailing slash, its file count and what it keeps", () => {
    openOn("node_modules")
    const text = dialogText()
    expect(text).toContain("node_modules/")
    expect(text).toContain("28,747 files")
    expect(text).toContain("Files the repository ignores inside it are kept")
    expect(text).toContain("The 1 nested repository inside it is kept")
    expect(screen.getByRole("button", { name: "Delete" })).toBeTruthy()
    // The folder is a chip, never a quotation.
    expect(
      screen.getAllByText("node_modules/").some((el) => el.closest("code") !== null),
    ).toBe(true)
  })

  it("warns that a repository of its own goes with its history", () => {
    openOn("vendor/lib")
    expect(dialogText()).toContain(
      "including its history and any commits not pushed anywhere else",
    )
  })

  it("keeps the file wording for a file", () => {
    openOn("notes.md")
    expect(dialogText()).toContain("is untracked and will be permanently DELETED")
    expect(screen.getByRole("button", { name: "Discard" })).toBeTruthy()
  })
})

describe("the bulk discard dialog", () => {
  function openBulk(paths: string[]) {
    render(
      <ConfirmDiscardFilesDialog
        open
        paths={paths}
        unstaged={[folder, nested, plain]}
        onCancel={() => {}}
        onConfirm={() => {}}
      />,
    )
  }

  it("counts the files inside a folder rather than one for it", () => {
    openBulk(["node_modules", "notes.md"])
    const text = dialogText()
    expect(text).toContain("28,748 untracked files will be permanently DELETED")
    expect(text).toContain("Files the repository ignores inside the folders are kept")
    expect(text).toContain("The 1 nested repository inside them is kept")
    expect(text).not.toContain("2 untracked files")
  })

  it("warns about the history of a repository of its own", () => {
    openBulk(["vendor/lib"])
    expect(dialogText()).toContain(
      "including its history and any commits not pushed anywhere else",
    )
  })
})
