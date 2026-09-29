// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react"

import type { ChangesSlice, DuxState } from "@/lib/store"
import type { ChangedFileView } from "@/lib/types"
import { stubMatchMedia, type MatchMediaStub } from "@/test/matchMedia"

// Expanding a folded folder in the Changes pane: the toggle, the lazy load of
// one level at a time, what shows while it loads or when it fails, and that
// every row inside is a full row, selectable and actionable like any other.

const openEditor = vi.fn()
let mockState: DuxState
const mockListeners = new Set<() => void>()
function publishMockState() {
  act(() => {
    for (const listener of mockListeners) listener()
  })
}
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  const { useSyncExternalStore } = await import("react")
  const subscribe = (listener: () => void) => {
    mockListeners.add(listener)
    return () => void mockListeners.delete(listener)
  }
  return {
    ...actual,
    useDux: () => mockState,
    useDuxSelector: <T,>(select: (state: DuxState) => T): T =>
      useSyncExternalStore(subscribe, () => select(mockState)),
    openEditor: (...args: unknown[]) => openEditor(...args),
  }
})

const stageOne = vi.fn(async (_id: string, _path: string) => ({
  left_out_repositories: 0,
  left_out_worktrees: 0,
}))
const stageMany = vi.fn(async (_id: string, paths: string[]) => ({
  done: paths,
  refused: [] as string[],
}))
vi.mock("@/lib/git", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/git")>()
  return {
    ...actual,
    git: {
      ...actual.git,
      stage: (...args: [string, string]) => stageOne(...args),
      stageMany: (...args: [string, string[]]) => stageMany(...args),
    },
  }
})

// Each call answers when the test says so.
type Answer = { resolve: (rows: ChangedFileView[]) => void; reject: (e: Error) => void }
const answers: Answer[] = []
const fetchFolderChildren = vi.fn(
  (_s: string, dir: string, side: string, signal?: AbortSignal) =>
    new Promise<{ dir: string; side: string; children: ChangedFileView[] }>(
      (resolve, reject) => {
        answers.push({
          resolve: (children) => resolve({ dir, side, children }),
          reject,
        })
        signal?.addEventListener("abort", () => reject(new ChangesFetchAborted()))
      },
    ),
)
vi.mock("@/lib/changesApi", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/changesApi")>()
  return {
    ...actual,
    fetchFolderChildren: (...args: [string, string, string, AbortSignal]) =>
      fetchFolderChildren(...args),
  }
})

let bootMedia: MatchMediaStub | null = null
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
  bootMedia?.restore()
  bootMedia = stubMatchMedia()
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  )
  if (!Element.prototype.getAnimations) {
    Element.prototype.getAnimations = () => []
  }
}

installBootStubs()
const { ChangedFiles } = await import("./ChangedFiles")
const { ChangesFetchAborted, ChangesFetchError } = await import("@/lib/changesApi")
const { resetExpansionsForTests } = await import("@/lib/changesExpansion")

function file(path: string, extra: Partial<ChangedFileView> = {}): ChangedFileView {
  return {
    path,
    status: "??",
    additions: 1,
    deletions: 0,
    binary: false,
    diff_excluded: false,
    ...extra,
  }
}
function folder(path: string, count: number, fingerprint = "f1"): ChangedFileView {
  return file(path, { kind: "directory", file_count: count, additions: 0, fingerprint })
}

function slice(unstaged: ChangedFileView[], rev = 1): ChangesSlice {
  return { sessionId: "s1", phase: "loaded", rev, staged: [], unstaged, error: null }
}

function toggle(name: string): HTMLButtonElement {
  return screen.getByRole("button", { name: new RegExp(`^${name}`) }) as HTMLButtonElement
}

async function answer(index: number, rows: ChangedFileView[]) {
  await act(async () => {
    answers[index]!.resolve(rows)
    await Promise.resolve()
  })
}

beforeEach(() => {
  installBootStubs()
  resetExpansionsForTests()
  answers.length = 0
  fetchFolderChildren.mockClear()
  openEditor.mockClear()
  stageOne.mockClear()
  stageMany.mockClear()
  mockState = {
    selectedSessionId: "s1",
    changes: slice([folder("node_modules", 3), file("notes.md")]),
  } as unknown as DuxState
})

afterEach(() => {
  cleanup()
  resetExpansionsForTests()
})

describe("expanding a folded folder", () => {
  it("is a real button that says whether it is expanded and which container it opens", async () => {
    render(<ChangedFiles />)
    const button = toggle("node_modules/")
    expect(button.tagName).toBe("BUTTON")
    expect(button.getAttribute("aria-expanded")).toBe("false")

    fireEvent.click(button)

    expect(button.getAttribute("aria-expanded")).toBe("true")
    const controls = button.getAttribute("aria-controls")!
    expect(document.getElementById(controls)?.getAttribute("role")).toBe("group")
    expect(fetchFolderChildren).toHaveBeenCalledWith(
      "s1",
      "node_modules",
      "unstaged",
      expect.any(AbortSignal),
    )
    expect(screen.getByText("Loading…")).toBeTruthy()

    await answer(0, [folder("node_modules/pkg0", 2), file("node_modules/top.js")])

    expect(screen.queryByText("Loading…")).toBeNull()
    const group = within(document.getElementById(controls)!)
    expect(group.getByText("pkg0/")).toBeTruthy()
    expect(group.getByText("top.js")).toBeTruthy()

    fireEvent.click(button)
    expect(button.getAttribute("aria-expanded")).toBe("false")
    expect(screen.queryByText("top.js")).toBeNull()
  })

  it("expands a folder inside an expanded one, a level deeper", async () => {
    render(<ChangedFiles />)
    fireEvent.click(toggle("node_modules/"))
    await answer(0, [folder("node_modules/pkg0", 2), file("node_modules/top.js")])

    fireEvent.click(toggle("pkg0/"))
    expect(fetchFolderChildren).toHaveBeenLastCalledWith(
      "s1",
      "node_modules/pkg0",
      "unstaged",
      expect.any(AbortSignal),
    )
    await answer(1, [file("node_modules/pkg0/a.js"), file("node_modules/pkg0/b.js")])

    const aRow = screen.getByText("a.js").closest('[role="row"]')!
    expect(aRow.getAttribute("aria-level")).toBe("3")
  })

  it("says why a folder could not be listed, and Retry asks again", async () => {
    render(<ChangedFiles />)
    fireEvent.click(toggle("node_modules/"))
    await act(async () => {
      answers[0]!.reject(new ChangesFetchError("the repository is busy", 409))
      await Promise.resolve()
    })

    expect(screen.getByText(/Could not list this folder: the repository is busy/)).toBeTruthy()
    fireEvent.click(screen.getByRole("button", { name: "Retry" }))
    expect(fetchFolderChildren).toHaveBeenCalledTimes(2)
    expect(screen.getByText("Loading…")).toBeTruthy()
    await answer(1, [file("node_modules/top.js")])
    expect(screen.getByText("top.js")).toBeTruthy()
  })

  it("makes every row inside a full row: diff, menu actions and selection", async () => {
    render(<ChangedFiles />)
    fireEvent.click(toggle("node_modules/"))
    await answer(0, [file("node_modules/top.js")])

    fireEvent.click(screen.getByText("top.js"))
    expect(openEditor).toHaveBeenCalledWith(
      expect.anything(),
      "node_modules/top.js",
      "diff",
    )

    fireEvent.click(screen.getByLabelText("Actions for node_modules/top.js"))
    fireEvent.click(within(await screen.findByRole("menu")).getByText("Stage"))
    await vi.waitFor(() =>
      expect(stageOne).toHaveBeenCalledWith("s1", "node_modules/top.js"),
    )

    fireEvent.click(screen.getByLabelText("Select node_modules/top.js"))
    const bar = within(
      screen.getByRole("toolbar", { name: "Actions for the selected files" }),
    )
    fireEvent.click(bar.getByRole("button", { name: /Stage 1/ }))
    await vi.waitFor(() =>
      expect(stageMany).toHaveBeenCalledWith("s1", ["node_modules/top.js"]),
    )
  })

  it("counts a checked row inside a checked folder once, in the folder", async () => {
    render(<ChangedFiles />)
    fireEvent.click(toggle("node_modules/"))
    await answer(0, [file("node_modules/top.js")])
    fireEvent.click(screen.getByLabelText("Select node_modules"))
    fireEvent.click(screen.getByLabelText("Select node_modules/top.js"))

    const bar = within(
      screen.getByRole("toolbar", { name: "Actions for the selected files" }),
    )
    fireEvent.click(bar.getByRole("button", { name: /Stage 3/ }))
    await vi.waitFor(() => expect(stageMany).toHaveBeenCalledWith("s1", ["node_modules"]))
  })

  it("refreshes an expanded folder quietly when its row moves, and forgets it when it goes", async () => {
    render(<ChangedFiles />)
    fireEvent.click(toggle("node_modules/"))
    await answer(0, [file("node_modules/top.js")])

    mockState = {
      ...mockState,
      changes: slice([folder("node_modules", 4, "f2"), file("notes.md")], 2),
    } as unknown as DuxState
    publishMockState()
    expect(fetchFolderChildren).toHaveBeenCalledTimes(2)
    // What is on screen stays until the answer lands.
    expect(screen.getByText("top.js")).toBeTruthy()
    expect(screen.queryByText("Loading…")).toBeNull()
    await answer(1, [file("node_modules/top.js"), file("node_modules/new.js")])
    expect(screen.getByText("new.js")).toBeTruthy()

    mockState = {
      ...mockState,
      changes: slice([file("notes.md")], 3),
    } as unknown as DuxState
    publishMockState()
    expect(screen.queryByText("top.js")).toBeNull()
  })

  it("remembers what was expanded when the pane comes back for the same agent", async () => {
    const { unmount } = render(<ChangedFiles />)
    fireEvent.click(toggle("node_modules/"))
    await answer(0, [file("node_modules/top.js")])
    unmount()

    render(<ChangedFiles />)
    expect(toggle("node_modules/").getAttribute("aria-expanded")).toBe("true")
    expect(screen.getByText("top.js")).toBeTruthy()
    expect(fetchFolderChildren).toHaveBeenCalledTimes(1)
  })

  it("offers no toggle on a repository of its own", () => {
    mockState = {
      ...mockState,
      changes: slice([file("vendor/lib", { kind: "nested_repository", additions: 0 })]),
    } as unknown as DuxState
    render(<ChangedFiles />)
    expect(screen.queryByRole("button", { name: /^vendor\/lib/ })).toBeNull()
  })
})

describe("what is checked under an expanded folder", () => {
  function bar() {
    return within(screen.getByRole("toolbar", { name: "Actions for the selected files" }))
  }

  // "Select all" is every row shown, the rows inside expanded folders too.
  it("Select all checks the rows inside expanded folders, and Select none clears them", async () => {
    render(<ChangedFiles />)
    fireEvent.click(toggle("node_modules/"))
    await answer(0, [file("node_modules/top.js")])
    fireEvent.click(screen.getByLabelText("Select notes.md"))

    fireEvent.click(bar().getByRole("button", { name: /Select all/ }))
    const child = screen.getByLabelText("Select node_modules/top.js")
    expect(child.getAttribute("aria-checked")).toBe("true")

    fireEvent.click(bar().getByRole("button", { name: /Select none/ }))
    expect(child.getAttribute("aria-checked")).toBe("false")
  })

  // A collapsed folder's rows are not shown, so they are not checked either.
  it("collapsing a folder unchecks the rows under it", async () => {
    render(<ChangedFiles />)
    fireEvent.click(toggle("node_modules/"))
    await answer(0, [file("node_modules/top.js")])
    fireEvent.click(screen.getByLabelText("Select node_modules/top.js"))
    fireEvent.click(screen.getByLabelText("Select notes.md"))
    expect(bar().getByRole("button", { name: /Stage 2/ })).toBeTruthy()

    fireEvent.click(toggle("node_modules/"))
    expect(bar().getByRole("button", { name: /Stage 1/ })).toBeTruthy()

    fireEvent.click(toggle("node_modules/"))
    await answer(1, [file("node_modules/top.js")])
    expect(
      screen.getByLabelText("Select node_modules/top.js").getAttribute("aria-checked"),
    ).toBe("false")
  })

  // A sub-folder that left its parent takes its checked rows with it: nothing
  // hidden is counted or sent.
  it("drops a checked row inside a sub-folder that left its parent", async () => {
    render(<ChangedFiles />)
    fireEvent.click(toggle("node_modules/"))
    await answer(0, [folder("node_modules/pkg0", 1), file("node_modules/top.js")])
    fireEvent.click(toggle("pkg0/"))
    await answer(1, [file("node_modules/pkg0/a.js")])
    fireEvent.click(screen.getByLabelText("Select node_modules/pkg0/a.js"))
    fireEvent.click(screen.getByLabelText("Select notes.md"))

    // The listing moves; the folder's new answer no longer holds pkg0.
    mockState = {
      ...mockState,
      changes: slice([folder("node_modules", 1, "f2"), file("notes.md")], 2),
    } as unknown as DuxState
    publishMockState()
    await answer(2, [file("node_modules/top.js")])

    expect(screen.queryByText("a.js")).toBeNull()
    fireEvent.click(bar().getByRole("button", { name: /Stage 1/ }))
    await vi.waitFor(() => expect(stageMany).toHaveBeenCalledWith("s1", ["notes.md"]))
  })

  // A sub-folder that grew is asked for again once its parent's answer says so.
  it("asks again for an expanded sub-folder that grew", async () => {
    render(<ChangedFiles />)
    fireEvent.click(toggle("node_modules/"))
    await answer(0, [folder("node_modules/pkg0", 1, "p1")])
    fireEvent.click(toggle("pkg0/"))
    await answer(1, [file("node_modules/pkg0/a.js")])

    mockState = {
      ...mockState,
      changes: slice([folder("node_modules", 2, "f2"), file("notes.md")], 2),
    } as unknown as DuxState
    publishMockState()
    await answer(2, [folder("node_modules/pkg0", 2, "p2")])
    expect(fetchFolderChildren).toHaveBeenLastCalledWith(
      "s1",
      "node_modules/pkg0",
      "unstaged",
      expect.any(AbortSignal),
    )
    await answer(3, [file("node_modules/pkg0/a.js"), file("node_modules/pkg0/b.js")])
    expect(screen.getByText("b.js")).toBeTruthy()
  })
})

// The whole journey a user takes: open a folder, open one inside it, check a
// file there, stage it, and see the listing's answer fold everything back up.
describe("a user expanding and staging inside a folder", () => {
  it("goes from a folded row to a staged file inside it", async () => {
    render(<ChangedFiles />)
    fireEvent.click(toggle("node_modules/"))
    await answer(0, [folder("node_modules/pkg0", 2), file("node_modules/top.js")])
    fireEvent.click(toggle("pkg0/"))
    await answer(1, [file("node_modules/pkg0/a.js"), file("node_modules/pkg0/b.js")])

    fireEvent.click(screen.getByLabelText("Select node_modules/pkg0/a.js"))
    const bar = within(
      screen.getByRole("toolbar", { name: "Actions for the selected files" }),
    )
    fireEvent.click(bar.getByRole("button", { name: /Stage 1/ }))
    await vi.waitFor(() =>
      expect(stageMany).toHaveBeenCalledWith("s1", ["node_modules/pkg0/a.js"]),
    )

    // The server's next listing: the folder now holds two files, one staged.
    mockState = {
      ...mockState,
      changes: {
        ...slice([folder("node_modules", 2, "f2"), file("notes.md")], 2),
        staged: [file("node_modules/pkg0/a.js", { status: "A" })],
      },
    } as unknown as DuxState
    publishMockState()
    expect(screen.getByText("node_modules/pkg0/a.js")).toBeTruthy()
    expect(fetchFolderChildren).toHaveBeenCalledTimes(3)
  })
})
