// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  within,
} from "@testing-library/react"

import type { ChangesSlice, DuxState } from "@/lib/store"

// The Changes pane's "Refresh changes" item has exactly one job that a reader
// cannot see by looking at it: it must take the FORCING path. The store's
// `refreshChanges` only re-GETs, and the server answers that from the very cache
// this action exists to bypass, so an item wired to it would look like it worked
// and change nothing. Two comments in the source warn about that, and a comment
// cannot fail a build, so this mounts the component and clicks the real item.

const forceRefreshChanges = vi.fn(() => Promise.resolve())
const refreshChanges = vi.fn()
const openEditor = vi.fn()

let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    forceRefreshChanges: () => forceRefreshChanges(),
    refreshChanges: () => refreshChanges(),
    openEditor: (...args: unknown[]) => openEditor(...args),
  }
})

const stageMany = vi.fn(async (_id: string, paths: string[]) => ({
  done: paths,
  refused: [] as string[],
}))
const unstageMany = vi.fn(async (_id: string, paths: string[]) => ({
  done: paths,
  refused: [] as string[],
}))
const discardMany = vi.fn(async (_id: string, paths: string[]) => ({
  done: paths,
  failed: [] as { path: string; message: string }[],
}))
vi.mock("@/lib/git", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/git")>()
  return {
    ...actual,
    git: {
      ...actual.git,
      stageMany: (...args: [string, string[]]) => stageMany(...args),
      unstageMany: (...args: [string, string[]]) => unstageMany(...args),
      discardMany: (...args: [string, string[]]) => discardMany(...args),
    },
  }
})

const notifySuccess = vi.fn()
const notifyWarning = vi.fn()
const notifyError = vi.fn()
vi.mock("@/lib/notify", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/notify")>()
  return {
    ...actual,
    notifySuccess: (...args: unknown[]) => notifySuccess(...args),
    notifyWarning: (...args: unknown[]) => notifyWarning(...args),
    notifyError: (...args: unknown[]) => notifyError(...args),
  }
})

// The real store boots at import time and touches localStorage and fetch, and
// the pane renders a base-ui ScrollArea whose viewport probes APIs jsdom does
// not implement. `matches: false` plus jsdom's 1024px width put this on the
// desktop layout.
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

function loadedChanges(): ChangesSlice {
  return {
    sessionId: "s1",
    phase: "loaded",
    rev: 1,
    staged: [],
    unstaged: [],
    error: null,
  }
}

async function openActionsMenu() {
  render(<ChangedFiles />)
  fireEvent.click(screen.getByLabelText("Changes actions"))
  return within(await screen.findByRole("menu"))
}

beforeEach(() => {
  installBootStubs()
  forceRefreshChanges.mockClear()
  refreshChanges.mockClear()
  openEditor.mockClear()
  stageMany.mockClear()
  unstageMany.mockClear()
  discardMany.mockClear()
  notifySuccess.mockClear()
  notifyWarning.mockClear()
  notifyError.mockClear()
  mockState = {
    selectedSessionId: "s1",
    changes: loadedChanges(),
  } as unknown as DuxState
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("the Changes pane's Refresh changes action", () => {
  it("forces the server to ask git again rather than re-reading its cache", async () => {
    const menu = await openActionsMenu()

    fireEvent.click(menu.getByText("Refresh changes"))

    expect(forceRefreshChanges).toHaveBeenCalledTimes(1)
    expect(refreshChanges).not.toHaveBeenCalled()
  })

  it("keeps the leading icon the menu conventions require, and no ellipsis", async () => {
    const menu = await openActionsMenu()

    const item = menu.getByText("Refresh changes").closest('[role="menuitem"]')
    expect(item).toBeTruthy()
    expect(item!.querySelector("svg")).toBeTruthy()
    // A trailing "…" marks an item that opens a dialog or needs confirming.
    // This one does neither.
    expect(item!.textContent?.endsWith("…")).toBe(false)
  })
})

describe("ChangedFiles for a standalone agent", () => {
  function standaloneState(
    repo_status: "working_repo" | "no_repo" | "inside_repo_rooted_elsewhere",
    quiet_reason: string,
  ) {
    return {
      selectedSessionId: "sa1",
      changes: { ...loadedChanges(), sessionId: "sa1" },
      spine: {
        projects: [],
        terminals: [],
        sessions: [
          {
            id: "sa1",
            title: "notes",
            provider: "claude",
            status: "active",
            tabs: [],
            has_output: false,
            working: false,
            typing: false,
            needs_attention: false,
            created_at: "",
            updated_at: "",
            auto_reopen_enabled: false,
            workspace: {
              kind: "folder",
              folder_path: "/home/someone/notes",
              folder_label: "~/notes",
              repo_status,
              quiet_reason,
            },
          },
        ],
      },
    } as unknown as DuxState
  }

  // A folder with no repository is QUIET, and it says why in the folder's own
  // words. The old error path reported "the repository is busy" once per poll,
  // which is a lie about a folder that simply has no repository.
  it("says why the region is quiet, and never that a repository is busy", () => {
    mockState = standaloneState(
      "no_repo",
      "This folder has no git repository, so there are no changes to show.",
    )
    render(<ChangedFiles />)
    expect(screen.getByText(/no git repository/)).toBeTruthy()
    expect(screen.queryByText(/busy/i)).toBeNull()
    // And it names the folder, so the sentence is about something the user can
    // see rather than an abstraction.
    expect(screen.getByText(/~\/notes/)).toBeTruthy()
  })

  it("says which quiet this is for a folder inside somebody else's repository", () => {
    mockState = standaloneState(
      "inside_repo_rooted_elsewhere",
      "This folder sits inside a repository rooted elsewhere, so dux shows no changes for it.",
    )
    render(<ChangedFiles />)
    expect(screen.getByText(/rooted elsewhere/)).toBeTruthy()
  })

  // And when the folder IS a repository the panel is ordinary: no quiet copy,
  // the real changes view.
  it("renders the ordinary changes view when the folder is a repository", () => {
    mockState = standaloneState("working_repo", "")
    render(<ChangedFiles />)
    expect(screen.queryByText(/no git repository/)).toBeNull()
    expect(screen.getByLabelText("Changes actions")).toBeTruthy()
  })

  // Push and Pull publish a BRANCH, which this agent does not have even in a
  // real repository. Absent rather than on screen and refused on click by the
  // server. Committing stays: that is folder work.
  it("offers Commit but neither Push nor Pull, even in a repository folder", async () => {
    mockState = standaloneState("working_repo", "")
    render(<ChangedFiles />)
    fireEvent.click(screen.getByLabelText("Changes actions"))
    const menu = within(await screen.findByRole("menu"))
    expect(menu.getByText("Commit…")).toBeTruthy()
    expect(menu.queryByText("Push")).toBeNull()
    expect(menu.queryByText("Pull")).toBeNull()
  })
})

function withFiles(
  staged: Array<[string, string]>,
  unstaged: Array<[string, string]>,
): DuxState {
  const view = ([path, status]: [string, string]) => ({
    path,
    status,
    additions: 1,
    deletions: 0,
    binary: false,
  })
  return {
    selectedSessionId: "s1",
    changes: {
      ...loadedChanges(),
      staged: staged.map(view),
      unstaged: unstaged.map(view),
    },
  } as unknown as DuxState
}

function check(path: string) {
  fireEvent.click(screen.getByLabelText(`Select ${path}`))
}

function bar() {
  return within(screen.getByRole("toolbar", { name: "Actions for the selected files" }))
}

describe("the changes pane's multi-select", () => {
  beforeEach(() => {
    mockState = withFiles(
      [["staged.ts", "M"]],
      [["a.ts", "M"], ["b.ts", "??"]],
    )
  })

  it("shows the bulk bar with the verb and the count once files are checked", () => {
    render(<ChangedFiles />)
    expect(
      screen.queryByRole("toolbar", { name: "Actions for the selected files" }),
    ).toBeNull()

    check("a.ts")
    check("b.ts")

    expect(bar().getByRole("button", { name: "Stage 2" })).toBeTruthy()
    expect(bar().getByRole("button", { name: "Discard 2…" })).toBeTruthy()
  })

  // One request per verb: the batch route stages the lot in one git call and
  // broadcasts once. A per-file loop would churn the pane.
  it("stages every checked path in one request and says so once", async () => {
    render(<ChangedFiles />)
    check("a.ts")
    check("b.ts")
    fireEvent.click(bar().getByRole("button", { name: "Stage 2" }))
    await act(() => stageMany.mock.results[0]!.value as Promise<unknown>)

    expect(stageMany).toHaveBeenCalledTimes(1)
    expect(stageMany).toHaveBeenCalledWith("s1", ["a.ts", "b.ts"])
    expect(notifySuccess).toHaveBeenCalledTimes(1)
    expect(notifyError).not.toHaveBeenCalled()
  })

  // The acted paths leave the set the moment the server says yes, so the bar
  // cannot be clicked a second time on files that have already moved.
  it("sends nothing on a second click after a success", async () => {
    render(<ChangedFiles />)
    check("a.ts")
    fireEvent.click(bar().getByRole("button", { name: "Stage 1" }))
    await act(() => stageMany.mock.results[0]!.value as Promise<unknown>)

    expect(
      screen.queryByRole("toolbar", { name: "Actions for the selected files" }),
    ).toBeNull()
    expect(stageMany).toHaveBeenCalledTimes(1)
  })

  it("unstages from the staged section with its own verb", async () => {
    render(<ChangedFiles />)
    check("staged.ts")
    fireEvent.click(bar().getByRole("button", { name: "Unstage 1" }))
    await act(() => unstageMany.mock.results[0]!.value as Promise<unknown>)

    expect(unstageMany).toHaveBeenCalledWith("s1", ["staged.ts"])
  })

  it("warns once, not per file, when the server could not act on everything", async () => {
    stageMany.mockResolvedValueOnce({ done: ["a.ts"], refused: ["b.ts"] })
    render(<ChangedFiles />)
    check("a.ts")
    check("b.ts")
    fireEvent.click(bar().getByRole("button", { name: "Stage 2" }))
    await act(() => stageMany.mock.results[0]!.value as Promise<unknown>)

    expect(notifyWarning).toHaveBeenCalledTimes(1)
    expect(notifySuccess).not.toHaveBeenCalled()
  })

  it("empties both sections when Clear is pressed", () => {
    render(<ChangedFiles />)
    check("a.ts")
    check("staged.ts")
    expect(bar().getByRole("button", { name: "Stage 1" })).toBeTruthy()
    expect(bar().getByRole("button", { name: "Unstage 1" })).toBeTruthy()

    fireEvent.click(bar().getByRole("button", { name: "Clear" }))

    expect(
      screen.queryByRole("toolbar", { name: "Actions for the selected files" }),
    ).toBeNull()
  })

  // The checkbox and the row mean different things, and base-ui re-dispatches a
  // click on the root's hidden input, so both clicks have to stop at the
  // wrapper or every tick would open a diff.
  it("never opens the diff when the checkbox itself is clicked", () => {
    render(<ChangedFiles />)
    check("a.ts")
    expect(openEditor).not.toHaveBeenCalled()
  })

  it("still opens the diff when the row is clicked", () => {
    render(<ChangedFiles />)
    fireEvent.click(screen.getByText("a.ts"))
    expect(openEditor).toHaveBeenCalledTimes(1)
  })

  // The status marker moved out of the leading slot to make room for the
  // checkbox, and must still be on the row, after the path.
  it("keeps the status marker on the row, trailing the path", () => {
    render(<ChangedFiles />)
    const path = screen.getByText("a.ts")
    const row = path.closest('[role="row"]')!
    const marker = within(row as HTMLElement).getByRole("img", { name: "Modified" })
    expect(
      path.compareDocumentPosition(marker) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy()
  })

  it("keeps the header ellipsis the only surface-scoped one while the bar shows", () => {
    render(<ChangedFiles />)
    check("a.ts")
    expect(screen.getAllByLabelText("Changes actions")).toHaveLength(1)
    expect(bar().queryByLabelText(/actions/i)).toBeNull()
  })

  it("drops a checked path once a refresh moves it to the other section", () => {
    const view = render(<ChangedFiles />)
    check("a.ts")
    expect(bar().getByRole("button", { name: "Stage 1" })).toBeTruthy()

    mockState = withFiles([["staged.ts", "M"], ["a.ts", "M"]], [["b.ts", "??"]])
    view.rerender(<ChangedFiles />)

    expect(
      screen.queryByRole("toolbar", { name: "Actions for the selected files" }),
    ).toBeNull()
  })
})

describe("the section select-all", () => {
  beforeEach(() => {
    mockState = withFiles([], [["a.ts", "M"], ["b.ts", "??"]])
  })

  it("reads mixed while only some rows are checked", () => {
    render(<ChangedFiles />)
    check("a.ts")
    expect(
      screen.getByLabelText("Select all unstaged files").getAttribute("aria-checked"),
    ).toBe("mixed")
  })

  it("checks every row in its own section", () => {
    render(<ChangedFiles />)
    fireEvent.click(screen.getByLabelText("Select all unstaged files"))
    expect(bar().getByRole("button", { name: "Stage 2" })).toBeTruthy()
  })

  // The header counts the rows on screen; the bar counts and acts on the whole
  // selection, so a filter never silently shrinks what a verb will do.
  it("counts the filtered rows while the bar keeps the whole selection", () => {
    render(<ChangedFiles />)
    check("a.ts")
    check("b.ts")
    fireEvent.change(screen.getByLabelText("Filter changed files"), {
      target: { value: "a.ts" },
    })

    expect(
      screen.getByLabelText("Select all unstaged files").getAttribute("aria-checked"),
    ).toBe("true")
    expect(bar().getByRole("button", { name: "Stage 2" })).toBeTruthy()
  })

  it("checks only the filtered rows", () => {
    render(<ChangedFiles />)
    fireEvent.change(screen.getByLabelText("Filter changed files"), {
      target: { value: "a.ts" },
    })
    fireEvent.click(screen.getByLabelText("Select all unstaged files"))

    expect(bar().getByRole("button", { name: "Stage 1" })).toBeTruthy()
  })
})

describe("the multi-file discard confirm", () => {
  beforeEach(() => {
    mockState = withFiles([], [["a.ts", "M"], ["gone.ts", "??"]])
  })

  it("names the count and both outcomes, and defaults to Cancel", () => {
    render(<ChangedFiles />)
    check("a.ts")
    check("gone.ts")
    fireEvent.click(bar().getByRole("button", { name: "Discard 2…" }))

    const dialog = within(screen.getByRole("dialog"))
    expect(dialog.getByText(/2 files/)).toBeTruthy()
    // One untracked (deleted from disk) and one tracked (restored).
    expect(dialog.getByText(/1 untracked/)).toBeTruthy()
    expect(dialog.getByText(/1 .*restored/)).toBeTruthy()
    const cancel = dialog.getByRole("button", { name: "Cancel" })
    expect(document.activeElement).toBe(cancel)
  })

  it("discards the live intersection and reports it once", async () => {
    render(<ChangedFiles />)
    check("a.ts")
    check("gone.ts")
    fireEvent.click(bar().getByRole("button", { name: "Discard 2…" }))
    fireEvent.click(
      within(screen.getByRole("dialog")).getByRole("button", {
        name: "Discard",
      }),
    )
    await act(() => discardMany.mock.results[0]!.value as Promise<unknown>)

    expect(discardMany).toHaveBeenCalledWith("s1", ["a.ts", "gone.ts"])
    expect(notifySuccess).toHaveBeenCalledTimes(1)
  })
})
