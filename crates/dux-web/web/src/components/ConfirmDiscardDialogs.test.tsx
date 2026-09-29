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
const closeDiscard = vi.hoisted(() => vi.fn())
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return { ...actual, useDux: () => mockState, closeDiscard }
})

const discard = vi.hoisted(() => vi.fn(() => Promise.resolve()))
vi.mock("@/lib/git", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/git")>()
  return { ...actual, git: { ...actual.git, discard } }
})

const notifySuccess = vi.hoisted(() => vi.fn())
const notifyWarning = vi.hoisted(() => vi.fn())
vi.mock("@/lib/notify", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/notify")>()
  return { ...actual, notifySuccess, notifyWarning }
})

import { proseText } from "@/lib/prose"

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

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

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
  linked_worktrees: 1,
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

function stateOn(path: string, unstaged: ChangedFileView[]): DuxState {
  const row = [folder, nested, plain].find((f) => f.path === path)!
  return {
    discardTarget: { sessionId: "s1", path, untracked: true, row },
    changes: {
      sessionId: "s1",
      phase: "loaded",
      rev: 1,
      staged: [],
      unstaged,
      error: null,
    },
  } as unknown as DuxState
}

function openOn(path: string) {
  mockState = stateOn(path, [folder, nested, plain])
  return render(<ConfirmDiscardFileDialog />)
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
    expect(text).toContain("The 1 worktree of this repository inside it is kept")
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

  // A destructive act is always confirmed once it lands, in words that say
  // what went: here, a whole folder's files.
  it("confirms the delete with a toast once the server answers", async () => {
    openOn("node_modules")
    screen.getByRole("button", { name: "Delete" }).click()
    await vi.waitFor(() => expect(notifySuccess).toHaveBeenCalledTimes(1))
    expect(proseText(notifySuccess.mock.calls[0]![0])).toContain(
      "Deleted the untracked files in node_modules/ (28,747 files)",
    )
    // What the user confirmed travels with the request, so the server can
    // refuse a folder that became something else meanwhile.
    expect(discard).toHaveBeenCalledWith("s1", "node_modules", "directory")
  })

  // The dialog says what it opened on, and that is what a click confirms: a
  // folder that becomes a repository while the dialog is open closes it and
  // says so, rather than quietly turning into the history warning one click
  // away from deleting that history.
  it("closes and says so when the folder changes kind while it is open", () => {
    const { rerender } = openOn("node_modules")
    expect(dialogText()).toContain("28,747 files")

    mockState = stateOn("node_modules", [
      { ...folder, kind: "nested_repository" },
      nested,
      plain,
    ])
    rerender(<ConfirmDiscardFileDialog />)

    expect(screen.queryByRole("dialog")).toBeNull()
    expect(closeDiscard).toHaveBeenCalled()
    expect(discard).not.toHaveBeenCalled()
    expect(notifyWarning).toHaveBeenCalledTimes(1)
    expect(proseText(notifyWarning.mock.calls[0]![0])).toBe(
      "node_modules/ changed while the dialog was open: it is now a repository of its own, " +
        "with a history. Nothing was deleted; look at it again before deleting it.",
    )
  })

  it("keeps the file wording for a file", () => {
    openOn("notes.md")
    expect(dialogText()).toContain("is untracked and will be permanently DELETED")
    expect(screen.getByRole("button", { name: "Discard" })).toBeTruthy()
  })

  // A file row is confirmed as a file, so the server refuses it once a
  // folder has taken the name rather than cleaning that folder.
  it("tells the server a file row is a file", async () => {
    openOn("notes.md")
    screen.getByRole("button", { name: "Discard" }).click()
    await vi.waitFor(() => expect(discard).toHaveBeenCalledTimes(1))
    expect(discard).toHaveBeenCalledWith("s1", "notes.md", "file")
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

  // Rows a delete would not act on are left out of the count and the
  // request, and the dialog says which and why rather than letting the
  // server refuse them afterwards.
  it("leaves out the rows it would not delete, and says why", () => {
    const onConfirm = vi.fn()
    const linked: ChangedFileView = {
      path: "inner-wt",
      status: "??",
      additions: 0,
      deletions: 0,
      binary: false,
      diff_excluded: false,
      kind: "linked_worktree",
    }
    const nestedOnly: ChangedFileView = {
      path: "vendor-only",
      status: "??",
      additions: 0,
      deletions: 0,
      binary: false,
      diff_excluded: false,
      kind: "directory",
      file_count: 0,
      nested_repositories: 1,
    }
    render(
      <ConfirmDiscardFilesDialog
        open
        paths={["node_modules", "inner-wt", "vendor-only"]}
        unstaged={[folder, linked, nestedOnly]}
        onCancel={() => {}}
        onConfirm={onConfirm}
      />,
    )
    const text = dialogText()
    expect(text).toContain("28,747 untracked files will be permanently DELETED")
    expect(text).toContain("2 selected rows are left out")
    expect(text).toContain("inner-wt/ is a worktree of this repository")
    expect(text).toContain("vendor-only/ holds only repositories of their own")
    // Each left-out folder is a chip, not a path in a plain sentence.
    for (const name of ["inner-wt/", "vendor-only/"]) {
      expect(
        screen.getAllByText(name).some((el) => el.closest("code") !== null),
        name,
      ).toBe(true)
    }
    screen.getByRole("button", { name: "Discard" }).click()
    // What each folder was when the dialog opened travels with it.
    expect(onConfirm).toHaveBeenCalledWith(["node_modules"], {
      node_modules: "directory",
    })
  })

  it("closes and says so when a selected folder changes kind while it is open", () => {
    const onCancel = vi.fn()
    const onConfirm = vi.fn()
    const dialog = (unstaged: ChangedFileView[]) => (
      <ConfirmDiscardFilesDialog
        open
        paths={["node_modules", "notes.md"]}
        unstaged={unstaged}
        onCancel={onCancel}
        onConfirm={onConfirm}
      />
    )
    const { rerender } = render(dialog([folder, plain]))
    expect(dialogText()).toContain("28,748 untracked files")

    rerender(dialog([{ ...folder, kind: "nested_repository" }, plain]))

    expect(screen.queryByRole("dialog")).toBeNull()
    expect(onCancel).toHaveBeenCalled()
    expect(onConfirm).not.toHaveBeenCalled()
    expect(proseText(notifyWarning.mock.calls[0]![0])).toContain(
      "node_modules/ changed while the dialog was open: it is now a repository of its own",
    )
  })

  // The copy is what the rows were when the dialog opened, not whatever the
  // live list says a moment later.
  it("words the dialog from the rows it opened on", () => {
    const dialog = (unstaged: ChangedFileView[]) => (
      <ConfirmDiscardFilesDialog
        open
        paths={["node_modules"]}
        unstaged={unstaged}
        onCancel={() => {}}
        onConfirm={() => {}}
      />
    )
    const { rerender } = render(dialog([folder]))
    rerender(dialog([{ ...folder, file_count: 5 }]))
    expect(dialogText()).toContain("28,747 untracked files")
  })

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
