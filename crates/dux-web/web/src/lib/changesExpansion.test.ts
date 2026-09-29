import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { ChangedFileView } from "./types"

const fetchFolderChildren = vi.hoisted(() =>
  vi.fn<
    (
      sessionId: string,
      dir: string,
      side: "staged" | "unstaged",
      signal?: AbortSignal,
    ) => Promise<{ dir: string; side: string; children: ChangedFileView[] }>
  >(),
)
vi.mock("./changesApi", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./changesApi")>()
  return { ...actual, fetchFolderChildren }
})

const {
  expansionsFor,
  reconcileFolders,
  resetExpansionsForTests,
  retryFolderChildren,
  toggleFolder,
} = await import("./changesExpansion")
const { ChangesFetchAborted, ChangesFetchError } = await import("./changesApi")
const { folderKey } = await import("./changesTree")

function file(path: string): ChangedFileView {
  return { path, status: "??", additions: 0, deletions: 0, binary: false, diff_excluded: false }
}
function folder(path: string, count: number, fingerprint = "f"): ChangedFileView {
  return { ...file(path), kind: "directory", file_count: count, fingerprint }
}

// A request the test answers when it wants to, and that rejects as a real one
// does when its signal aborts.
function pending() {
  let answer!: (children: ChangedFileView[]) => void
  let refuse!: (error: Error) => void
  let signal: AbortSignal | undefined
  fetchFolderChildren.mockImplementationOnce((_s, dir, side, sig) => {
    signal = sig
    return new Promise((resolve, reject) => {
      answer = (children) => resolve({ dir, side, children })
      refuse = reject
      sig?.addEventListener("abort", () => reject(new ChangesFetchAborted()))
    })
  })
  return {
    answer: (children: ChangedFileView[]) => answer(children),
    refuse: (error: Error) => refuse(error),
    aborted: () => signal?.aborted ?? false,
  }
}

const node = (sessionId: string, path: string, section: "staged" | "unstaged" = "unstaged") =>
  expansionsFor(sessionId).get(folderKey(section, path))

beforeEach(() => {
  resetExpansionsForTests()
  fetchFolderChildren.mockReset()
})

afterEach(() => resetExpansionsForTests())

describe("the expansion store", () => {
  it("expands, loads, and remembers per agent", async () => {
    const request = pending()
    toggleFolder("s1", "unstaged", folder("dist", 1))
    expect(node("s1", "dist")).toMatchObject({ loading: true, children: null })
    expect(fetchFolderChildren).toHaveBeenCalledWith(
      "s1",
      "dist",
      "unstaged",
      expect.any(AbortSignal),
    )
    request.answer([file("dist/a.js")])
    await vi.waitFor(() => expect(node("s1", "dist")?.children).toEqual([file("dist/a.js")]))
    expect(expansionsFor("s2").size).toBe(0)
  })

  it("collapsing aborts the request and forgets the folder", async () => {
    const request = pending()
    toggleFolder("s1", "unstaged", folder("dist", 1))
    toggleFolder("s1", "unstaged", folder("dist", 1))
    expect(request.aborted()).toBe(true)
    expect(expansionsFor("s1").size).toBe(0)
  })

  it("shows a first failure with Retry, and retrying asks again", async () => {
    const first = pending()
    toggleFolder("s1", "unstaged", folder("dist", 1))
    first.refuse(new ChangesFetchError("the server went away", 0))
    await vi.waitFor(() => expect(node("s1", "dist")?.error).toBe("the server went away"))

    const second = pending()
    retryFolderChildren("s1", "unstaged", "dist")
    expect(node("s1", "dist")).toMatchObject({ loading: true, error: null })
    second.answer([file("dist/a.js")])
    await vi.waitFor(() => expect(node("s1", "dist")?.children).toHaveLength(1))
  })

  it("refetches quietly when the row moves, superseding the older request", async () => {
    const first = pending()
    toggleFolder("s1", "unstaged", folder("dist", 1, "f1"))
    first.answer([file("dist/a.js")])
    await vi.waitFor(() => expect(node("s1", "dist")?.children).toHaveLength(1))

    const quiet = pending()
    reconcileFolders("s1", [], [folder("dist", 2, "f2")])
    expect(node("s1", "dist")).toMatchObject({
      loading: true,
      children: [file("dist/a.js")],
    })
    const newer = pending()
    reconcileFolders("s1", [], [folder("dist", 3, "f3")])
    expect(quiet.aborted()).toBe(true)

    newer.answer([file("dist/a.js"), file("dist/b.js"), file("dist/c.js")])
    await vi.waitFor(() => expect(node("s1", "dist")?.children).toHaveLength(3))
  })

  it("keeps what is on screen when a quiet refetch fails", async () => {
    const first = pending()
    toggleFolder("s1", "unstaged", folder("dist", 1, "f1"))
    first.answer([file("dist/a.js")])
    await vi.waitFor(() => expect(node("s1", "dist")?.children).toHaveLength(1))

    const quiet = pending()
    reconcileFolders("s1", [], [folder("dist", 2, "f2")])
    quiet.refuse(new ChangesFetchError("busy", 409))
    await vi.waitFor(() => expect(node("s1", "dist")?.loading).toBe(false))
    expect(node("s1", "dist")).toMatchObject({ children: [file("dist/a.js")], error: null })
  })

  it("drops a folder that left the listing and aborts what it was waiting for", () => {
    const request = pending()
    toggleFolder("s1", "unstaged", folder("dist", 1))
    reconcileFolders("s1", [], [file("other")])
    expect(request.aborted()).toBe(true)
    expect(expansionsFor("s1").size).toBe(0)
  })
})
