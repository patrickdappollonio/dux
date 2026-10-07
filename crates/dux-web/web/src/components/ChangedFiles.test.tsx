// @vitest-environment jsdom
import { chip, proseText, type Prose } from "@/lib/prose"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react"

import type * as React from "react"

import type { ChangesSlice, DuxState } from "@/lib/store"
import { stubMatchMedia, type MatchMediaStub } from "@/test/matchMedia"
import { toastText } from "@/test/toastText"

// The Changes pane's "Refresh changes" item has exactly one job that a reader
// cannot see by looking at it: it must take the FORCING path. The store's
// `refreshChanges` only re-GETs, and the server answers that from the very cache
// this action exists to bypass, so an item wired to it would look like it worked
// and change nothing. Two comments in the source warn about that, and a comment
// cannot fail a build, so this mounts the component and clicks the real item.

const forceRefreshChanges = vi.fn(() => Promise.resolve())
const refreshChanges = vi.fn()
const openEditor = vi.fn()
const toggleChangesPane = vi.fn()
const openCommit = vi.fn()

let mockState: DuxState
// The pane subscribes selectively and is memoized, so a new state reaches it
// the way the real store delivers one: through its subscribers, not through a
// parent re-render.
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
    forceRefreshChanges: () => forceRefreshChanges(),
    refreshChanges: () => refreshChanges(),
    openEditor: (...args: unknown[]) => openEditor(...args),
    toggleChangesPane: (...args: unknown[]) => toggleChangesPane(...args),
    openCommit: (...args: unknown[]) => openCommit(...args),
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
const stageOne = vi.fn(async (_id: string, _path: string) => ({
  left_out_repositories: 0,
  left_out_worktrees: 0,
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
      stage: (...args: [string, string]) => stageOne(...args),
      stageMany: (...args: [string, string[]]) => stageMany(...args),
      unstageMany: (...args: [string, string[]]) => unstageMany(...args),
      discardMany: (...args: [string, string[]]) => discardMany(...args),
    },
  }
})

// The real tooltip only mounts its popup into a portal on hover and needs a
// ResizeObserver, which jsdom lacks. Render its `content` inline instead so a
// test can assert what a row's status slot is wired to reveal, mirroring the
// pattern used in Sidebar.test.tsx and PrBanner.test.tsx.
vi.mock("@/components/SimpleTooltip", () => ({
  SimpleTooltip: ({
    children,
    content,
  }: {
    children: React.ReactNode
    content: React.ReactNode
  }) => (
    <>
      {children}
      <span data-testid="tooltip-content">{content}</span>
    </>
  ),
}))

const notifySuccess = vi.fn()
const notifyInfo = vi.fn()
const notifyWarning = vi.fn()
const notifyError = vi.fn()
vi.mock("@/lib/notify", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/notify")>()
  return {
    ...actual,
    notifySuccess: (...args: unknown[]) => notifySuccess(...args),
    notifyInfo: (...args: unknown[]) => notifyInfo(...args),
    notifyWarning: (...args: unknown[]) => notifyWarning(...args),
    notifyError: (...args: unknown[]) => notifyError(...args),
  }
})

// The real store boots at import time and touches localStorage and fetch, and
// the pane renders a base-ui ScrollArea whose viewport probes APIs jsdom does
// not implement. `matches: false` plus jsdom's 1024px width put this on the
// desktop layout.
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
// Imported after the boot stubs, like the component itself: the real store
// module boots at import time. `standaloneEditorHash` is the actual one (the
// mock above spreads the original), so the phone button's address is checked
// against the router's own grammar rather than a hand-written string.
const { standaloneEditorHash } = await import("@/lib/store")
const { agentRoot } = await import("@/lib/editorRoot")

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
  toggleChangesPane.mockClear()
  openCommit.mockClear()
  stageMany.mockClear()
  unstageMany.mockClear()
  discardMany.mockClear()
  notifySuccess.mockClear()
  notifyInfo.mockClear()
  notifyWarning.mockClear()
  notifyError.mockClear()
  mockState = {
    selectedSessionId: "s1",
    changes: loadedChanges(),
  } as unknown as DuxState
})

afterEach(() => {
  cleanup()
  bootMedia?.restore()
  bootMedia = null
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
            slot_tab_id: "sa1",
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

// A quiet pane is still a pane: its title and its ⋯ stay, so it can be hidden
// and the editor reached, and the git items stay on screen greyed out with the
// reason, because they come back once the folder has a repository.
describe("the Changes pane header in a quiet state", () => {
  function sessionWith(workspace: Record<string, unknown>): DuxState {
    return {
      selectedSessionId: "q1",
      changes: { ...loadedChanges(), sessionId: "q1" },
      spine: {
        projects: [],
        terminals: [],
        sessions: [
          {
            id: "q1",
            slot_tab_id: "q1",
            title: "quiet",
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
            workspace,
          },
        ],
      },
    } as unknown as DuxState
  }
  const folderAt = (repo_status: string, quiet_reason: string) =>
    sessionWith({
      kind: "folder",
      folder_path: "/home/someone/src",
      folder_label: "~/src",
      repo_status,
      quiet_reason,
    })
  const missingWorkingCopy = () =>
    sessionWith({
      kind: "managed",
      project_id: "p1",
      branch_name: "feature/x",
      initial_branch: "feature/x",
      branch_provenance: "created",
      source_branch: "main",
      worktree_path: "/managed/wt",
      worktree_missing: true,
      quiet_reason: "The working copy at /managed/wt no longer exists on disk.",
    })

  const folderStates = [
    {
      status: "no_repo",
      sentence: "This folder has no git repository, so there are no changes to show.",
      reason: "This folder has no git repository.",
    },
    {
      status: "inside_repo_rooted_elsewhere",
      sentence: "This folder sits inside a repository rooted elsewhere, so dux shows no changes for it.",
      reason: "This folder sits inside a repository rooted elsewhere.",
    },
    {
      status: "indeterminate",
      sentence: "dux could not consult git about this folder, so it cannot say whether it has changes.",
      reason: "dux could not consult git about this folder.",
    },
    {
      status: "unprobed",
      sentence: "dux is still looking at this folder to see whether it is a git repository.",
      reason: "dux is still looking at this folder.",
    },
    {
      status: "missing",
      sentence: "The folder ~/src no longer exists on disk.",
      reason: "This folder no longer exists on disk.",
    },
  ]

  async function openMenu() {
    fireEvent.click(screen.getByLabelText("Changes actions"))
    return within(await screen.findByRole("menu"))
  }
  const itemOf = (menu: ReturnType<typeof within>, text: string) =>
    menu.getByText(text).closest('[role="menuitem"]') as HTMLElement
  // A disabled item's reason is the element its aria-describedby names, so a
  // screen reader hears why on the item itself.
  const reasonOf = (item: HTMLElement) => {
    const id = item.getAttribute("aria-describedby")
    return id ? document.getElementById(id)?.textContent ?? null : null
  }

  for (const { status, sentence, reason } of folderStates) {
    it(`keeps the title and the ⋯ above the quiet sentence (${status})`, () => {
      mockState = folderAt(status, sentence)
      render(<ChangedFiles />)
      expect(screen.getByText("Changes")).toBeTruthy()
      expect(screen.getByLabelText("Changes actions")).toBeTruthy()
      expect(screen.getByText(sentence)).toBeTruthy()
      expect(screen.getByText("~/src")).toBeTruthy()
    })

    it(`greys out the git items with the reason, and keeps Hide (${status})`, async () => {
      mockState = folderAt(status, sentence)
      render(<ChangedFiles />)
      const menu = await openMenu()
      for (const text of ["Commit…", "Refresh changes"]) {
        const item = itemOf(menu, text)
        expect(item.getAttribute("aria-disabled"), text).toBe("true")
        expect(reasonOf(item), text).toBe(reason)
      }
      expect(menu.getByText(reason)).toBeTruthy()
      // Branch identity stays absent for a standalone agent, quiet or not.
      expect(menu.queryByText("Push")).toBeNull()
      expect(menu.queryByText("Pull")).toBeNull()
      const hide = itemOf(menu, "Hide Changes pane")
      expect(hide.getAttribute("aria-disabled")).not.toBe("true")
      fireEvent.click(hide)
      expect(toggleChangesPane).toHaveBeenCalledTimes(1)
    })
  }

  // The reason line is the menu primitive's own label, inside a real group.
  it("puts the reason in the menu's label part", async () => {
    mockState = folderAt("no_repo", folderStates[0]!.sentence)
    render(<ChangedFiles />)
    const menu = await openMenu()
    const reason = menu.getByText("This folder has no git repository.")
    expect(reason.getAttribute("data-slot")).toBe("dropdown-menu-label")
    expect(itemOf(menu, "Commit…").getAttribute("aria-describedby")).toBe(reason.id)
  })

  // One verdict for the body and the menu: an empty sentence is not quiet in
  // either, so the list renders and the menu stays live together.
  it("treats an empty quiet sentence as not quiet in the body and the menu alike", async () => {
    mockState = folderAt("no_repo", "")
    render(<ChangedFiles />)
    expect(screen.queryByText("No changes to show")).toBeNull()
    expect(screen.getByText("No changes")).toBeTruthy()
    const menu = await openMenu()
    expect(itemOf(menu, "Refresh changes").getAttribute("aria-disabled")).not.toBe("true")
  })

  it("keeps the editor open to a folder that is there but has no repository", () => {
    mockState = folderAt("no_repo", folderStates[0]!.sentence)
    render(<ChangedFiles />)
    const button = screen.getByLabelText("Open editor")
    expect(button.hasAttribute("disabled")).toBe(false)
    fireEvent.click(button)
    expect(openEditor).toHaveBeenCalledWith({ kind: "agent", sessionId: "q1" })
  })

  it("greys out the editor, with the reason, once the folder is gone", () => {
    mockState = folderAt("missing", folderStates[4]!.sentence)
    render(<ChangedFiles />)
    const button = screen.getByLabelText("Open editor")
    expect(button.hasAttribute("disabled")).toBe(true)
    const cell = button.closest("[data-slot=card-action]") as HTMLElement
    expect(
      within(cell)
        .getAllByTestId("tooltip-content")
        .some((node) => node.textContent === "This folder no longer exists on disk."),
    ).toBe(true)
  })

  // A managed agent keeps its branch features, so its Push and Pull are on
  // screen; with the working copy gone they are greyed out like the rest.
  it("greys out Push and Pull too for a managed working copy that is gone", async () => {
    mockState = missingWorkingCopy()
    render(<ChangedFiles />)
    expect(
      screen.getByText("The working copy at /managed/wt no longer exists on disk."),
    ).toBeTruthy()
    const menu = await openMenu()
    for (const text of ["Commit…", "Push", "Pull", "Refresh changes"]) {
      const item = itemOf(menu, text)
      expect(item.getAttribute("aria-disabled"), text).toBe("true")
      expect(reasonOf(item), text).toBe("This working copy no longer exists on disk.")
    }
    expect(screen.getByLabelText("Open editor").hasAttribute("disabled")).toBe(true)
  })

  it("keeps the header while the changes load and when they fail to", () => {
    mockState = {
      selectedSessionId: "s1",
      changes: { ...loadedChanges(), phase: "loading" },
    } as unknown as DuxState
    render(<ChangedFiles />)
    expect(screen.getByText("Loading changes…")).toBeTruthy()
    expect(screen.getByLabelText("Changes actions")).toBeTruthy()
    cleanup()

    mockState = {
      selectedSessionId: "s1",
      changes: { ...loadedChanges(), phase: "error", error: "git exploded" },
    } as unknown as DuxState
    render(<ChangedFiles />)
    expect(screen.getByText("git exploded")).toBeTruthy()
    expect(screen.getByLabelText("Changes actions")).toBeTruthy()
  })

  it("keeps the header over the ordinary no-changes state, with every item live", async () => {
    render(<ChangedFiles />)
    expect(screen.getByText("No changes")).toBeTruthy()
    const menu = await openMenu()
    expect(itemOf(menu, "Refresh changes").getAttribute("aria-disabled")).not.toBe("true")
    expect(itemOf(menu, "Refresh changes").getAttribute("aria-describedby")).toBeNull()
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
  //
  // And it says nothing: the rows cross from Unstaged to Staged in the pane
  // the click happened in, so a toast restating it is noise.
  it("stages every checked path in one request and says nothing", async () => {
    render(<ChangedFiles />)
    check("a.ts")
    check("b.ts")
    fireEvent.click(bar().getByRole("button", { name: "Stage 2" }))
    await act(() => stageMany.mock.results[0]!.value as Promise<unknown>)

    expect(stageMany).toHaveBeenCalledTimes(1)
    expect(stageMany).toHaveBeenCalledWith("s1", ["a.ts", "b.ts"])
    expect(notifySuccess).not.toHaveBeenCalled()
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

  // A tick that lands while a verb is in flight survives the response: the
  // selection is written from the state at that moment, not from the render
  // that started the request.
  it("keeps a box ticked while a request was already in flight", async () => {
    let land: (result: { done: string[]; refused: string[] }) => void = () => {}
    stageMany.mockImplementationOnce(
      () =>
        new Promise<{ done: string[]; refused: string[] }>((resolve) => {
          land = resolve
        }),
    )
    render(<ChangedFiles />)
    check("a.ts")
    fireEvent.click(bar().getByRole("button", { name: "Stage 1" }))

    check("b.ts")
    expect(bar().getByRole("button", { name: "Stage 2" })).toBeTruthy()

    await act(async () => {
      land({ done: ["a.ts"], refused: [] })
    })

    expect(bar().getByRole("button", { name: "Stage 1" })).toBeTruthy()
    expect(
      screen.getByLabelText("Select b.ts").getAttribute("aria-checked"),
    ).toBe("true")
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

  // While a verb is in flight the bar says so and refuses a second start: the
  // buttons are disabled, the acting one is aria-busy, and it wears the row's
  // spinner idiom.
  it("marks the bar busy while a verb is in flight", async () => {
    let land: (result: { done: string[]; refused: string[] }) => void = () => {}
    stageMany.mockImplementationOnce(
      () =>
        new Promise<{ done: string[]; refused: string[] }>((resolve) => {
          land = resolve
        }),
    )
    render(<ChangedFiles />)
    check("a.ts")
    check("staged.ts")
    fireEvent.click(bar().getByRole("button", { name: "Stage 1" }))

    const staging = bar().getByRole("button", { name: "Stage 1" })
    expect(staging.getAttribute("aria-busy")).toBe("true")
    expect(staging.hasAttribute("disabled")).toBe(true)
    expect(staging.querySelector('svg[class*="animate-spin"]')).toBeTruthy()
    // The other verb is disabled too, so nothing else can start behind it.
    const unstaging = bar().getByRole("button", { name: "Unstage 1" })
    expect(unstaging.hasAttribute("disabled")).toBe(true)
    expect(unstaging.getAttribute("aria-busy")).toBe("false")

    await act(async () => {
      land({ done: ["a.ts"], refused: [] })
    })

    expect(
      bar().getByRole("button", { name: "Unstage 1" }).hasAttribute("disabled"),
    ).toBe(false)
  })

  // A request that never reached an answer is one error toast, not a success
  // and not one per file.
  it("raises a single error toast when the request itself fails", async () => {
    stageMany.mockRejectedValueOnce(new Error("git is busy"))
    render(<ChangedFiles />)
    check("a.ts")
    check("b.ts")
    fireEvent.click(bar().getByRole("button", { name: "Stage 2" }))
    await act(async () => {
      await Promise.resolve()
    })

    expect(notifyError).toHaveBeenCalledTimes(1)
    expect(notifyError).toHaveBeenCalledWith("git is busy")
    expect(notifySuccess).not.toHaveBeenCalled()
    expect(notifyWarning).not.toHaveBeenCalled()
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

  // The leading slot belongs to the status marker again. The checkbox lives IN
  // that slot rather than in a column of its own, so nothing on the row moved
  // to make room for multi-select.
  it("keeps the status marker in the row's leading slot, before the path", () => {
    render(<ChangedFiles />)
    const path = screen.getByText("a.ts")
    const row = path.closest('[role="row"]')!
    const marker = within(row as HTMLElement).getByRole("img", { name: "Modified" })
    expect(
      path.compareDocumentPosition(marker) & Node.DOCUMENT_POSITION_PRECEDING,
    ).toBeTruthy()
  })

  // The checkbox is ALWAYS in the DOM, never display-swapped: a keyboard user
  // has to be able to reach it on a row nobody is hovering, and jsdom cannot
  // hover at all.
  it("renders a focusable checkbox on an unhovered, unchecked row", () => {
    render(<ChangedFiles />)
    const box = screen.getByLabelText("Select a.ts")
    expect(box.getAttribute("aria-checked")).toBe("false")
    box.focus()
    expect(document.activeElement).toBe(box)
  })

  it("puts the checkbox in the leading slot, sharing it with the marker", () => {
    render(<ChangedFiles />)
    const path = screen.getByText("a.ts")
    const box = screen.getByLabelText("Select a.ts")
    const marker = within(
      path.closest('[role="row"]') as HTMLElement,
    ).getByRole("img", { name: "Modified" })
    expect(
      path.compareDocumentPosition(box) & Node.DOCUMENT_POSITION_PRECEDING,
    ).toBeTruthy()
    expect(box.parentElement!.contains(marker)).toBe(true)
  })

  it("ticks the row when the checkbox is clicked", () => {
    render(<ChangedFiles />)
    const box = screen.getByLabelText("Select a.ts")
    fireEvent.click(box)
    expect(box.getAttribute("aria-checked")).toBe("true")
  })

  // On a mouse the slot stays small and its click halo is suppressed, so a
  // near-miss lands on the row's open-diff click rather than on a checkbox the
  // user cannot see reaching that far. These are class pins: what the geometry
  // actually measures is proven in the preview container, not by these strings.
  it("keeps the desktop slot small with the checkbox halo suppressed", () => {
    render(<ChangedFiles />)
    const box = screen.getByLabelText("Select a.ts")
    expect(box.className).toContain("after:hidden")
    expect(box.parentElement!.className).toContain("w-5")
    expect(box.parentElement!.className).toContain("h-5")
  })

  // The reveal is keyed on KEYBOARD focus of the checkbox, never focus-within
  // on the row: focus-within also fires when the row's ellipsis menu closes
  // back onto its trigger, and when a mouse tick leaves the checkbox focused,
  // stranding that row showing a checkbox and no marker.
  it("reveals on keyboard focus of the checkbox, never on row focus-within", () => {
    render(<ChangedFiles />)
    const box = screen.getByLabelText("Select a.ts")
    // The fading wrapper around the marker, not the marker glyph itself.
    const markerWrap = box.parentElement!.querySelector('[role="img"]')!
      .parentElement!
    for (const el of [box, markerWrap]) {
      expect(el.className).toContain("group-has-[[data-slot=checkbox]:focus-visible]:")
      expect(el.className).not.toContain("group-focus-within:")
    }
  })

  // Both of the row's hover reveals, this slot and the trailing ellipsis, run
  // on the same duration and easing so they arrive together.
  it("matches the trailing ellipsis's reveal timing", () => {
    render(<ChangedFiles />)
    const box = screen.getByLabelText("Select a.ts")
    expect(box.className).toContain("duration-200")
    expect(box.className).toContain("ease-out")
  })

  // The marker is pointer-transparent and fades on the very hover that would
  // have opened its own tooltip, so the status word lives on the slot around
  // it instead.
  it("names the file's status in the slot's tooltip", () => {
    render(<ChangedFiles />)
    const row = screen.getByText("a.ts").closest('[role="row"]') as HTMLElement
    const tip = within(row).getByTestId("tooltip-content")
    expect(tip.textContent).toBe("Modified")
    // Its trigger is the whole SLOT, the box holding both the marker and the
    // checkbox, not the pointer-transparent marker inside it.
    expect(
      tip.previousElementSibling!.contains(screen.getByLabelText("Select a.ts")),
    ).toBe(true)
    // And the marker no longer carries a second tooltip of its own.
    expect(within(row).getAllByTestId("tooltip-content")).toHaveLength(1)
  })

  // One baseline: the path and the +N/-N counts sit in one items-baseline
  // container, so the digits stop reading as superscript beside the path.
  it("puts the path and its counts in one baseline container, in that order", () => {
    render(<ChangedFiles />)
    const path = screen.getByText("a.ts")
    const row = path.closest('[role="row"]') as HTMLElement
    const counts = within(row).getByText("+1")
    const box = path.closest(".items-baseline")
    expect(box).toBeTruthy()
    expect(box!.contains(counts)).toBe(true)
    expect(
      path.compareDocumentPosition(counts) & Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy()
  })

  it("keeps the header ellipsis the only surface-scoped one while the bar shows", () => {
    render(<ChangedFiles />)
    check("a.ts")
    expect(screen.getAllByLabelText("Changes actions")).toHaveLength(1)
    expect(bar().queryByLabelText(/actions/i)).toBeNull()
  })

  it("drops a checked path once a refresh moves it to the other section", () => {
    render(<ChangedFiles />)
    check("a.ts")
    expect(bar().getByRole("button", { name: "Stage 1" })).toBeTruthy()

    mockState = withFiles([["staged.ts", "M"], ["a.ts", "M"]], [["b.ts", "??"]])
    publishMockState()

    expect(
      screen.queryByRole("toolbar", { name: "Actions for the selected files" }),
    ).toBeNull()
  })

  it("scopes checked paths to their session and restores them on return", () => {
    render(<ChangedFiles />)
    check("a.ts")

    const second = withFiles([], [["other.ts", "M"]])
    mockState = {
      ...second,
      selectedSessionId: "s2",
      changes: { ...second.changes, sessionId: "s2" },
    }
    publishMockState()

    expect(
      screen.queryByRole("toolbar", { name: "Actions for the selected files" }),
    ).toBeNull()
    expect(screen.getByLabelText("Select other.ts").getAttribute("aria-checked")).toBe(
      "false",
    )

    mockState = withFiles(
      [["staged.ts", "M"]],
      [["a.ts", "M"], ["b.ts", "??"]],
    )
    publishMockState()

    expect(screen.getByLabelText("Select a.ts").getAttribute("aria-checked")).toBe(
      "true",
    )
    expect(bar().getByRole("button", { name: "Stage 1" })).toBeTruthy()
  })

  it("drops every attempted path after a partial bulk result", async () => {
    stageMany.mockResolvedValueOnce({ done: ["a.ts"], refused: ["b.ts"] })
    render(<ChangedFiles />)
    check("a.ts")
    check("b.ts")

    fireEvent.click(bar().getByRole("button", { name: "Stage 2" }))
    await act(() => stageMany.mock.results[0]!.value as Promise<unknown>)

    expect(
      screen.queryByRole("toolbar", { name: "Actions for the selected files" }),
    ).toBeNull()
    // The path is a chip, not text spliced into a plain string.
    const notice = notifyWarning.mock.calls[0]![0] as Prose
    expect(proseText(notice)).toBe(
      "1 file staged. 1 file had already left the list, starting with b.ts.",
    )
    expect(notice).toContainEqual(chip("b.ts"))
  })

  // A path the server refused for a reason of its own (a folder holding only
  // repositories) is explained with the server's sentence, not as a path that
  // left the list.
  it("says why the server refused a path in a bulk stage", async () => {
    const reason =
      'There is nothing in "b.ts/" to stage: it holds only repositories of their own.'
    stageMany.mockResolvedValueOnce({
      done: ["a.ts"],
      refused: ["b.ts"],
      reasons: { "b.ts": reason },
    })
    render(<ChangedFiles />)
    check("a.ts")
    check("b.ts")

    fireEvent.click(bar().getByRole("button", { name: "Stage 2" }))
    await act(() => stageMany.mock.results[0]!.value as Promise<unknown>)

    expect(proseText(notifyWarning.mock.calls[0]![0] as Prose)).toBe(
      `1 file staged. ${reason}`,
    )
  })

  it("keeps the selection and releases busy state after a bulk request error", async () => {
    stageMany.mockRejectedValueOnce("offline")
    render(<ChangedFiles />)
    check("a.ts")

    fireEvent.click(bar().getByRole("button", { name: "Stage 1" }))
    await act(async () => {
      await Promise.resolve()
    })

    expect(notifyError).toHaveBeenCalledWith("could not stage the files")
    expect(screen.getByLabelText("Select a.ts").getAttribute("aria-checked")).toBe(
      "true",
    )
    const button = bar().getByRole("button", { name: "Stage 1" })
    expect(button.getAttribute("aria-busy")).toBe("false")
    expect(button.hasAttribute("disabled")).toBe(false)
  })
})

describe("the changes pane's selection on a touch screen", () => {
  beforeEach(() => {
    mockState = withFiles([], [["a.ts", "M"], ["b.ts", "??"]])
  })

  // A finger cannot hover, so the slot itself is the tap target. jsdom has no
  // geometry, so this exercises the checkbox by its label; that the halo
  // actually fills the slot is a measurement, pinned by class below and proven
  // in the preview container.
  it("ticks the row when the slot's checkbox is activated", () => {
    render(<ChangedFiles />)
    const box = screen.getByLabelText("Select a.ts")
    expect(box.getAttribute("aria-checked")).toBe("false")

    fireEvent.click(box)

    expect(bar().getByRole("button", { name: "Stage 1" })).toBeTruthy()
  })

  // A tap anywhere in the slot lands on the checkbox halo and bubbles to the
  // slot wrapper, which is what has to stop it: this dispatches on the wrapper
  // itself so the stopPropagation is what is being exercised.
  it("never opens the diff when the slot itself is tapped", () => {
    render(<ChangedFiles />)
    const slot = screen.getByLabelText("Select a.ts").parentElement!
    fireEvent.click(slot)
    expect(openEditor).not.toHaveBeenCalled()
  })

  // The 44px floor on BOTH axes, carried by the checkbox halo rather than by
  // the slot's layout box: the slot keeps the desktop's 20px column, so the
  // path and the section heading line up with the marker as they do on a
  // mouse, and the halo reaches left across the list's empty gutter to the
  // pane edge and right to where the path starts. Class pins: the geometry
  // they stand for is measured in the preview container.
  it("gives coarse pointers a 44px halo without widening the slot or a React media subscription", () => {
    render(<ChangedFiles />)
    const box = screen.getByLabelText("Select a.ts")
    expect(box.parentElement!.className).toContain("w-5")
    expect(box.parentElement!.className).toContain("pointer-coarse:h-11")
    expect(box.parentElement!.className).not.toContain("pointer-coarse:size-11")
    expect(box.className).toContain("pointer-coarse:after:-inset-y-[15px]")
    expect(box.className).toContain("pointer-coarse:after:-left-[17px]")
    expect(box.className).toContain("pointer-coarse:after:-right-[11px]")
    expect(box.className).toContain("pointer-coarse:after:block")
    // The halo stops short of the pane divider's grab zone, which reaches
    // into the list's gutter and wins a tap there, so touch rows and the
    // section heading both sit 8px further in, keeping them lined up.
    const row = box.closest('[role="row"]') as HTMLElement
    expect(row.className).toContain("pointer-coarse:[--row-pad:--spacing(3)]")
    const heading = screen.getByText("Unstaged").closest("button") as HTMLElement
    expect(heading.className).toContain("pointer-coarse:pl-3")
  })

  // The row's ⋯ answers the same question the slot above answers, and used to
  // answer it with the viewport-width breakpoint instead of the pointer, so a
  // landscape tablet had no way to any row's actions at all. The marker/
  // checkbox swap is a separate mechanism with its own coarse rule (above) and
  // is deliberately untouched by this.
  it("always reveals the row's ⋯ where the pointer is coarse", () => {
    render(<ChangedFiles />)
    const wrapper = screen
      .getByLabelText("Actions for a.ts")
      .closest("div") as HTMLElement
    expect(wrapper.className).toContain("pointer-coarse:max-w-none")
    expect(wrapper.className).toContain("pointer-coarse:opacity-100")
    // A mouse still gets the collapsed slot and the hover reveal, plus the
    // two states that hold it open: the menu, and an action in flight.
    expect(wrapper.className).toContain("md:max-w-0")
    expect(wrapper.className).toContain("md:opacity-0")
    expect(wrapper.className).toContain("md:group-hover:max-w-10")
    expect(wrapper.className).toContain("md:has-[[data-popup-open]]:max-w-10")
    expect(wrapper.className).toContain("md:has-[[aria-busy=true]]:max-w-10")
  })
})

describe("the bulk bar's Select all toggle", () => {
  beforeEach(() => {
    mockState = withFiles(
      [["staged.ts", "M"]],
      [["a.ts", "M"], ["b.ts", "??"]],
    )
  })

  // The universe is every row the filter shows, across BOTH sections, never one
  // section at a time.
  it("reads Select all while a visible row is unchecked, and checks every section", () => {
    render(<ChangedFiles />)
    check("a.ts")

    fireEvent.click(bar().getByRole("button", { name: "Select all" }))

    expect(bar().getByRole("button", { name: "Stage 2" })).toBeTruthy()
    expect(bar().getByRole("button", { name: "Unstage 1" })).toBeTruthy()
  })

  it("flips to Select none once every visible row is checked, and unchecks them", () => {
    render(<ChangedFiles />)
    check("a.ts")
    fireEvent.click(bar().getByRole("button", { name: "Select all" }))

    fireEvent.click(bar().getByRole("button", { name: "Select none" }))

    expect(
      screen.queryByRole("toolbar", { name: "Actions for the selected files" }),
    ).toBeNull()
  })

  it("checks only the rows the filter shows", () => {
    render(<ChangedFiles />)
    check("staged.ts")
    fireEvent.change(screen.getByLabelText("Filter changed files"), {
      target: { value: "a.ts" },
    })

    fireEvent.click(bar().getByRole("button", { name: "Select all" }))

    expect(bar().getByRole("button", { name: "Stage 1" })).toBeTruthy()
  })

  it("changes visible rows across sections without touching hidden rows", () => {
    mockState = withFiles(
      [["visible-staged.ts", "M"], ["hidden-staged.ts", "M"]],
      [["visible-unstaged.ts", "M"], ["hidden-unstaged.ts", "M"]],
    )
    render(<ChangedFiles />)
    check("hidden-staged.ts")
    fireEvent.change(screen.getByLabelText("Filter changed files"), {
      target: { value: "visible" },
    })

    fireEvent.click(bar().getByRole("button", { name: "Select all" }))
    fireEvent.change(screen.getByLabelText("Filter changed files"), {
      target: { value: "" },
    })

    expect(
      screen.getByLabelText("Select visible-staged.ts").getAttribute("aria-checked"),
    ).toBe("true")
    expect(
      screen.getByLabelText("Select visible-unstaged.ts").getAttribute("aria-checked"),
    ).toBe("true")
    expect(
      screen.getByLabelText("Select hidden-staged.ts").getAttribute("aria-checked"),
    ).toBe("true")
    expect(
      screen.getByLabelText("Select hidden-unstaged.ts").getAttribute("aria-checked"),
    ).toBe("false")
  })

  // Select none acts on what is on screen, so a checked row the filter hides
  // stays checked: the bar stays up and the label flips back.
  it("leaves a hidden checked row alone and flips the label back", () => {
    render(<ChangedFiles />)
    check("a.ts")
    check("b.ts")
    fireEvent.change(screen.getByLabelText("Filter changed files"), {
      target: { value: "a.ts" },
    })

    fireEvent.click(bar().getByRole("button", { name: "Select none" }))

    expect(bar().getByRole("button", { name: "Stage 1" })).toBeTruthy()
    expect(bar().getByRole("button", { name: "Select all" })).toBeTruthy()
  })

  // Clear is not the same control: it empties the WHOLE selection, including
  // the rows the filter hides.
  it("keeps Clear emptying rows the filter hides, unlike Select none", () => {
    render(<ChangedFiles />)
    check("a.ts")
    check("b.ts")
    fireEvent.change(screen.getByLabelText("Filter changed files"), {
      target: { value: "a.ts" },
    })

    fireEvent.click(bar().getByRole("button", { name: "Clear" }))

    expect(
      screen.queryByRole("toolbar", { name: "Actions for the selected files" }),
    ).toBeNull()
  })

  // Nothing on screen to select: the toggle would be a lie about an empty
  // universe, so it is absent rather than disabled.
  it("renders no toggle when the filter hides every row", () => {
    render(<ChangedFiles />)
    check("a.ts")
    fireEvent.change(screen.getByLabelText("Filter changed files"), {
      target: { value: "no-such-file" },
    })

    expect(bar().queryByRole("button", { name: /^Select (all|none)$/ })).toBeNull()
    expect(bar().getByRole("button", { name: "Clear" })).toBeTruthy()
  })

  // It matches the checkboxes, not the verbs: ticking stays possible while a
  // verb is in flight, the same way the row checkboxes do.
  it("stays enabled while a verb is in flight", () => {
    stageMany.mockImplementationOnce(
      () => new Promise<{ done: string[]; refused: string[] }>(() => {}),
    )
    render(<ChangedFiles />)
    check("a.ts")
    fireEvent.click(bar().getByRole("button", { name: "Stage 1" }))

    expect(
      bar().getByRole("button", { name: "Select all" }).hasAttribute("disabled"),
    ).toBe(false)
  })

  // The section headings lost their checkboxes: the bar is the one place a
  // whole-list selection is made.
  it("leaves no checkbox on a section heading", () => {
    render(<ChangedFiles />)
    expect(screen.queryByLabelText("Select all staged files")).toBeNull()
    expect(screen.queryByLabelText("Select all unstaged files")).toBeNull()
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

    expect(discardMany).toHaveBeenCalledWith("s1", ["a.ts", "gone.ts"], {})
    expect(notifySuccess).toHaveBeenCalledTimes(1)
  })

  // The dialog's copy and its target both come from the live unstaged list, so
  // a file that leaves the list while the dialog is open is not discarded.
  it("acts on the survivors when a checked path leaves the list", async () => {
    render(<ChangedFiles />)
    check("a.ts")
    check("gone.ts")
    fireEvent.click(bar().getByRole("button", { name: "Discard 2…" }))

    mockState = withFiles([], [["a.ts", "M"]])
    publishMockState()

    const dialog = within(screen.getByRole("dialog"))
    expect(dialog.getByText(/1 file/)).toBeTruthy()
    fireEvent.click(dialog.getByRole("button", { name: "Discard" }))
    await act(() => discardMany.mock.results[0]!.value as Promise<unknown>)

    expect(discardMany).toHaveBeenCalledWith("s1", ["a.ts"], {})
  })

  it("closes itself once every checked path has left the list", async () => {
    render(<ChangedFiles />)
    check("a.ts")
    check("gone.ts")
    fireEvent.click(bar().getByRole("button", { name: "Discard 2…" }))
    expect(screen.getByRole("dialog")).toBeTruthy()

    mockState = withFiles([["a.ts", "M"], ["gone.ts", "??"]], [])
    await act(async () => {
      publishMockState()
    })

    expect(screen.queryByRole("dialog")).toBeNull()
  })

  // The list it was asked about is gone from under it: confirming would only
  // hit a refused write. It closes, and stays closed when the list returns.
  it("closes itself when the working copy goes away while it is open", async () => {
    render(<ChangedFiles />)
    check("a.ts")
    fireEvent.click(bar().getByRole("button", { name: "Discard 1…" }))
    expect(screen.getByRole("dialog")).toBeTruthy()

    const listed = withFiles([], [["a.ts", "M"], ["gone.ts", "??"]])
    mockState = {
      ...listed,
      spine: {
        projects: [],
        terminals: [],
        sessions: [
          {
            id: "s1",
            slot_tab_id: "s1",
            title: "s1",
            provider: "claude",
            status: "active",
            tabs: [],
            workspace: {
              kind: "managed",
              project_id: "p1",
              branch_name: "b",
              initial_branch: "b",
              branch_provenance: "created",
              source_branch: "main",
              worktree_path: "/wt",
              worktree_missing: true,
              quiet_reason: "The working copy at /wt no longer exists on disk.",
            },
          },
        ],
      },
    } as unknown as DuxState
    await act(async () => {
      publishMockState()
    })
    // Closed: the popup may linger for its exit transition, so wait it out.
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull())

    mockState = listed
    await act(async () => {
      publishMockState()
    })
    expect(screen.queryByRole("dialog")).toBeNull()
    expect(discardMany).not.toHaveBeenCalled()
  })

  it("closes itself when the listing goes back to loading", async () => {
    render(<ChangedFiles />)
    check("a.ts")
    fireEvent.click(bar().getByRole("button", { name: "Discard 1…" }))
    expect(screen.getByRole("dialog")).toBeTruthy()

    mockState = {
      ...mockState,
      changes: { ...mockState.changes, phase: "loading" },
    } as unknown as DuxState
    await act(async () => {
      publishMockState()
    })
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull())
  })

  // The ladder is one toast whose severity is the outcome: a partial run warns,
  // and a run that discarded nothing is an error.
  it("warns once when only some files were discarded", async () => {
    discardMany.mockResolvedValueOnce({
      done: ["a.ts"],
      failed: [{ path: "gone.ts", message: "unstage it first" }],
    })
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

    expect(notifyWarning).toHaveBeenCalledTimes(1)
    expect(String(notifyWarning.mock.calls[0]![0])).toContain("unstage it first")
    expect(notifySuccess).not.toHaveBeenCalled()
    expect(notifyError).not.toHaveBeenCalled()
  })

  it("errors once when nothing at all was discarded", async () => {
    discardMany.mockResolvedValueOnce({
      done: [],
      failed: [
        { path: "a.ts", message: "unstage it first" },
        { path: "gone.ts", message: "unstage it first" },
      ],
    })
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

    expect(notifyError).toHaveBeenCalledTimes(1)
    expect(toastText(notifyError.mock.calls[0]![0])).toContain("a.ts")
    expect(notifySuccess).not.toHaveBeenCalled()
    expect(notifyWarning).not.toHaveBeenCalled()
  })
})

// Each file group, and the pane header above them, carries an aggregate recap:
// the lines the visible rows add and remove between them, plus a quiet marker
// for the binaries, which carry no line counts at all.
describe("the changes pane's group recaps", () => {
  function counted(
    path: string,
    additions: number,
    deletions: number,
    binary = false,
    diffExcluded = false,
  ) {
    return {
      path,
      status: "M",
      additions,
      deletions,
      binary,
      diff_excluded: diffExcluded,
    }
  }

  function withCounted(
    staged: ReturnType<typeof counted>[],
    unstaged: ReturnType<typeof counted>[],
  ): DuxState {
    return {
      selectedSessionId: "s1",
      changes: { ...loadedChanges(), staged, unstaged },
    } as unknown as DuxState
  }

  function recap(scope: string) {
    return screen.getByLabelText(new RegExp(`^${scope}: `))
  }

  it("sums the lines of each group and of the pane as a whole", () => {
    mockState = withCounted(
      [counted("staged.ts", 12, 3)],
      [counted("a.ts", 7, 40), counted("b.ts", 0, 2)],
    )
    render(<ChangedFiles />)

    expect(recap("Staged").textContent).toBe("+12 −3")
    expect(recap("Unstaged").textContent).toBe("+7 −42")
    expect(recap("Changes").textContent).toBe("+19 −45")
  })

  // Big sums abbreviate so they cannot crowd the file count beside them, and
  // the spoken label keeps the exact figures the glyphs give up.
  it("abbreviates a big sum and keeps the full number in the label", () => {
    mockState = withCounted([], [counted("big.ts", 12345, 6789)])
    render(<ChangedFiles />)

    expect(recap("Unstaged").textContent).toBe("+12.3k −6.7k")
    expect(recap("Unstaged").getAttribute("aria-label")).toBe(
      "Unstaged: 12345 lines added, 6789 lines removed",
    )
  })

  // Under a thousand nothing changes, and no thousands separator appears: the
  // rows below carry none either.
  it("prints a sum under a thousand plainly", () => {
    mockState = withCounted([], [counted("big.ts", 999, 100)])
    render(<ChangedFiles />)

    expect(recap("Unstaged").textContent).toBe("+999 −100")
  })

  // The pane header is a two-cell grid with the ⋯ trigger in the second cell,
  // so its recap must give way rather than paint over the trigger at the widths
  // where the two meet. jsdom lays nothing out, so the degradation is pinned by
  // the classes that produce it.
  it("lets the pane's recap shrink away rather than reach the ⋯ trigger", () => {
    mockState = withCounted([], [counted("big.ts", 12345, 9999)])
    render(<ChangedFiles />)

    const paneRecap = recap("Changes")
    expect(paneRecap.className).toContain("truncate")
    expect(paneRecap.className).not.toContain("shrink-0")
    // A group heading keeps its figure whole: its badge shrinks with it.
    expect(recap("Unstaged").className).toContain("shrink-0")
  })

  // Only LINE counts abbreviate: the badge beside the sum counts files, and it
  // is printed exactly.
  it("leaves the group's file count exact beside an abbreviated sum", () => {
    mockState = withCounted(
      [],
      Array.from({ length: 12 }, (_, index) => counted(`f${index}.ts`, 1000, 0)),
    )
    render(<ChangedFiles />)

    expect(recap("Unstaged").textContent).toBe("+12k")
    expect(screen.getByText("12")).toBeTruthy()
  })

  // The recap describes exactly the rows visible beneath it, which is the
  // filtered set, matching the first number in the group badge's "1 of 2".
  it("follows the filter, describing only the rows still on screen", () => {
    mockState = withCounted(
      [],
      [counted("src/a.ts", 10, 1), counted("docs/b.md", 100, 5)],
    )
    render(<ChangedFiles />)
    expect(recap("Unstaged").textContent).toBe("+110 −6")

    fireEvent.change(screen.getByLabelText("Filter changed files"), {
      target: { value: "src/" },
    })

    expect(recap("Unstaged").textContent).toBe("+10 −1")
    expect(recap("Changes").textContent).toBe("+10 −1")
  })

  it("marks the binaries quietly beside the line counts", () => {
    mockState = withCounted(
      [],
      [counted("a.ts", 5, 1), counted("logo.png", 0, 0, true)],
    )
    render(<ChangedFiles />)

    expect(recap("Unstaged").textContent).toBe("+5 −1 · 1 bin")
    expect(recap("Unstaged").getAttribute("aria-label")).toBe(
      "Unstaged: 5 lines added, 1 line removed, 1 binary file",
    )
  })

  // A file the repository excludes from diffs has no counts either, and its own
  // marker and tally say so without calling it binary.
  it("marks a diff-excluded file with its own marker and tally", () => {
    mockState = withCounted(
      [],
      [counted("a.ts", 5, 1), counted("locked.txt", 0, 0, false, true)],
    )
    render(<ChangedFiles />)

    expect(recap("Unstaged").textContent).toBe("+5 −1 · 1 excl")
    expect(recap("Unstaged").getAttribute("aria-label")).toBe(
      "Unstaged: 5 lines added, 1 line removed, 1 excluded file",
    )
    expect(screen.getByText("Excl")).toBeTruthy()
  })

  // An all-binary group has no lines to report, so it says so rather than
  // claiming "+0 −0".
  it("says only the binary count for a group with no lines in it", () => {
    mockState = withCounted(
      [],
      [counted("logo.png", 0, 0, true), counted("clip.mp4", 0, 0, true)],
    )
    render(<ChangedFiles />)

    expect(recap("Unstaged").textContent).toBe("2 bin")
    expect(screen.queryByText("+0")).toBeNull()
  })

  // A group whose files changed no lines and are not binary (a mode change, an
  // empty new file) gets no recap at all.
  it("renders no recap for a lineless, binary-free group", () => {
    mockState = withCounted([], [counted("mode-only.sh", 0, 0)])
    render(<ChangedFiles />)

    expect(screen.queryByLabelText(/^Unstaged: /)).toBeNull()
    expect(screen.queryByLabelText(/^Changes: /)).toBeNull()
  })

  // An empty group is hidden entirely, recap and all.
  it("shows no staged recap while nothing is staged", () => {
    mockState = withCounted([], [counted("a.ts", 1, 0)])
    render(<ChangedFiles />)

    expect(screen.queryByLabelText(/^Staged: /)).toBeNull()
    expect(recap("Changes").textContent).toBe("+1")
  })
})

// The pane header's direct route to the editor for the agent whose changes are
// on screen. One button, one act: the in-page overlay on a computer, the
// standalone editor's address on a phone (where the overlay renders nothing at
// all), with the new-tab variant left to the row menus.
describe("the Changes pane header's Open editor button", () => {
  function setViewportWidth(width: number) {
    Object.defineProperty(window, "innerWidth", {
      value: width,
      configurable: true,
      writable: true,
    })
  }

  const editorButton = () => screen.getByLabelText("Open editor")
  const actionsTrigger = () => screen.getByLabelText("Changes actions")

  afterEach(() => {
    setViewportWidth(1024)
  })

  it("opens the in-page editor for the selected agent on a computer", () => {
    setViewportWidth(1024)
    render(<ChangedFiles />)

    fireEvent.click(editorButton())

    expect(openEditor).toHaveBeenCalledTimes(1)
    expect(openEditor).toHaveBeenCalledWith({ kind: "agent", sessionId: "s1" })
  })

  // The overlay is the act; the new-tab variant stays a menu item rather than
  // riding a modifier on this button, so on a computer it is not an anchor.
  it("is a plain button on a computer, with no address of its own", () => {
    setViewportWidth(1024)
    render(<ChangedFiles />)

    expect(editorButton().getAttribute("href")).toBeNull()
  })

  // On a phone the overlay renders null, so the button is the same anchor the
  // phone's menu entries are: the standalone editor's own address.
  it("navigates to the standalone editor's address on a phone", () => {
    setViewportWidth(400)
    render(<ChangedFiles />)

    const link = editorButton()
    expect(link.tagName).toBe("A")
    expect(link.getAttribute("href")).toBe(standaloneEditorHash(agentRoot("s1")))
    expect(link.getAttribute("target")).toBe("_blank")
    expect(link.getAttribute("rel")).toBe("noopener")

    fireEvent.click(link)
    expect(openEditor).not.toHaveBeenCalled()
  })

  it("sits beside the pane's ⋯, in the header's action cell", () => {
    render(<ChangedFiles />)
    const button = editorButton()
    const ellipsis = actionsTrigger()

    expect(button.parentElement).toBe(ellipsis.parentElement)
    expect(
      ellipsis.compareDocumentPosition(button) &
        Node.DOCUMENT_POSITION_PRECEDING,
    ).toBeTruthy()
    // The misclick spacing between two adjacent icon squares.
    expect(button.parentElement!.className).toContain("gap-2")
  })

  // One cluster, one look: the editor button wears the `⋯`'s outline variant at
  // the same height token (including the touch floor), so it reads as a button
  // rather than a bare glyph. jsdom cannot paint, so the shared Button's variant
  // and size classes are what is pinned.
  it("matches the ⋯ on variant and geometry", () => {
    render(<ChangedFiles />)
    const button = editorButton()
    const ellipsis = actionsTrigger()

    for (const control of [button, ellipsis]) {
      expect(control.className).toContain("size-8")
      expect(control.className).toContain("max-md:size-11")
      expect(control.className).toContain("border-border")
    }
  })

  it("names itself for a screen reader and hints the same words on hover", () => {
    render(<ChangedFiles />)
    const cell = editorButton().parentElement!
    expect(
      within(cell)
        .getAllByTestId("tooltip-content")
        .some((node) => node.textContent === "Open editor"),
    ).toBe(true)
  })

  // The pane with no editor target is not a pane with a disabled button: there
  // is no header at all, because there is no agent behind it.
  it("is absent when no session is selected", () => {
    mockState = {
      selectedSessionId: null,
      changes: loadedChanges(),
    } as unknown as DuxState
    render(<ChangedFiles />)

    expect(screen.queryByLabelText("Open editor")).toBeNull()
    expect(screen.getByText("No session selected")).toBeTruthy()
  })
})

// An agent whose worktree held an unignored node_modules (~29k untracked files)
// once froze the whole browser tab on selection. Two measured causes lived in
// this pane: every changed file mounted a full row (no windowing), and each
// row's base-ui Checkbox read its hidden input's `labels` after EVERY commit,
// which walks the whole document, so a mount or re-render of n rows cost
// O(n^2). In the preview repro that lookup alone took 1.2 s at 1000 rows, 4.8 s
// at 2000 and 13.2 s at 4000. These pin both fixes.
describe("huge-changes repro: the pane with thousands of changed files", () => {
  function manyUntracked(count: number): Array<[string, string]> {
    return Array.from({ length: count }, (_, index): [string, string] => [
      `web/node_modules/pkg${Math.floor(index / 50)}/lib/file${index % 50}.js`,
      "?",
    ])
  }

  it("mounts a bounded number of rows however long the list is", () => {
    mockState = withFiles([], manyUntracked(2000))
    render(<ChangedFiles />)
    const rows = screen.getAllByRole("row").length
    expect(rows, `2000 changed files mounted ${rows} rows`).toBeLessThanOrEqual(300)
  })

  it("does not look up a label per row on every commit", () => {
    const descriptor = Object.getOwnPropertyDescriptor(
      HTMLInputElement.prototype,
      "labels",
    )
    expect(descriptor?.get, "jsdom implements input.labels").toBeTruthy()
    let reads = 0
    Object.defineProperty(HTMLInputElement.prototype, "labels", {
      configurable: true,
      get(this: HTMLInputElement) {
        reads += 1
        return descriptor!.get!.call(this)
      },
    })
    try {
      mockState = withFiles([], manyUntracked(200))
      const view = render(<ChangedFiles />)
      const onMount = reads
      reads = 0
      // An unrelated store update hands the pane a new state object, as
      // `useDux` does on every setState anywhere in the app.
      mockState = { ...mockState }
      view.rerender(<ChangedFiles />)
      const onRerender = reads
      expect(
        { onMount: onMount <= 10, onRerender: onRerender <= 10 },
        `label lookups scale with rows: ${onMount} on mount, ${onRerender} on a re-render of 200 rows`,
      ).toEqual({ onMount: true, onRerender: true })
    } finally {
      Object.defineProperty(HTMLInputElement.prototype, "labels", descriptor!)
    }
  })
})

// The windowed list has to keep every behaviour the full list had: the section
// headings fold, the window follows the scroll, a focused row survives being
// scrolled away, and a row's menu still opens.
describe("the Changes pane's windowed list", () => {
  function numbered(count: number): Array<[string, string]> {
    return Array.from({ length: count }, (_, index): [string, string] => [
      `src/file${index}.ts`,
      "M",
    ])
  }

  function scrollTo(top: number) {
    const el = document.querySelector(
      '[data-slot="scroll-area-viewport"]',
    ) as HTMLElement
    Object.defineProperty(el, "scrollTop", { configurable: true, value: top })
    fireEvent.scroll(el)
  }

  // Default desktop heights: a 32px heading, then 42px rows.
  const ROW_2500 = 32 + 2500 * 42

  it("folds a section's rows away under its heading and brings them back", () => {
    mockState = withFiles([["staged.ts", "M"]], [["a.ts", "M"]])
    render(<ChangedFiles />)
    const heading = screen.getByRole("button", { name: /^Staged/ })
    expect(heading.getAttribute("aria-expanded")).toBe("true")

    fireEvent.click(heading)

    expect(heading.getAttribute("aria-expanded")).toBe("false")
    expect(screen.queryByText("staged.ts")).toBeNull()
    expect(screen.getByText("a.ts")).toBeTruthy()

    fireEvent.click(heading)
    expect(screen.getByText("staged.ts")).toBeTruthy()
  })

  it("points each heading at the container holding its own rows", () => {
    mockState = withFiles([["staged.ts", "M"]], [["a.ts", "M"]])
    render(<ChangedFiles />)
    for (const [name, mine, theirs] of [
      [/^Staged/, "staged.ts", "a.ts"],
      [/^Unstaged/, "a.ts", "staged.ts"],
    ] as const) {
      const id = screen.getByRole("button", { name }).getAttribute("aria-controls")
      expect(id).toBeTruthy()
      const rows = document.getElementById(id!)
      expect(rows).toBeTruthy()
      expect(within(rows!).getByText(mine)).toBeTruthy()
      expect(within(rows!).queryByText(theirs)).toBeNull()
    }
  })

  it("mounts the rows the scroll position shows rather than the first screenful", () => {
    mockState = withFiles([], numbered(5000))
    render(<ChangedFiles />)
    expect(screen.getByText("src/file0.ts")).toBeTruthy()
    expect(screen.queryByText("src/file2500.ts")).toBeNull()

    scrollTo(ROW_2500)

    expect(screen.getByText("src/file2500.ts")).toBeTruthy()
    expect(screen.queryByText("src/file0.ts")).toBeNull()
    expect(screen.getAllByRole("row").length).toBeLessThanOrEqual(60)
  })

  it("keeps a focused row mounted when it is scrolled out of the window", () => {
    mockState = withFiles([], numbered(5000))
    render(<ChangedFiles />)
    const box = screen.getByLabelText("Select src/file3.ts")
    act(() => box.focus())
    expect(document.activeElement).toBe(box)

    scrollTo(ROW_2500)

    expect(screen.getByText("src/file2500.ts")).toBeTruthy()
    expect(box.isConnected).toBe(true)
    expect(document.activeElement).toBe(box)
  })

  // The row's ⋯ menu is portaled out of the list, so focus moving into it
  // looks like focus leaving the list. It has not: the menu is the row's own,
  // and unmounting the row would take the open menu with it.
  it("keeps the row pinned while focus is in that row's own menu", async () => {
    mockState = withFiles([], numbered(5000))
    render(<ChangedFiles />)
    const trigger = screen.getByLabelText("Actions for src/file3.ts")
    act(() => trigger.focus())
    fireEvent.click(trigger)
    const menu = await screen.findByRole("menu")
    const item = within(menu).getByText("Stage").closest('[role="menuitem"]') as HTMLElement
    act(() => item.focus())
    expect(menu.contains(document.activeElement)).toBe(true)

    scrollTo(ROW_2500)

    expect(screen.getByText("src/file2500.ts")).toBeTruthy()
    expect(trigger.isConnected).toBe(true)
    expect(screen.getByRole("menu")).toBeTruthy()
  })

  it("releases the pin once focus leaves the list", () => {
    mockState = withFiles([], numbered(5000))
    render(<ChangedFiles />)
    act(() => screen.getByLabelText("Select src/file3.ts").focus())
    act(() => screen.getByLabelText("Filter changed files").focus())

    scrollTo(ROW_2500)

    expect(screen.queryByText("src/file3.ts")).toBeNull()
  })

  it("mounts a row's menu only once it is opened, and it still opens", async () => {
    mockState = withFiles([], [["a.ts", "M"]])
    render(<ChangedFiles />)
    expect(document.querySelector('[data-slot="dropdown-menu-content"]')).toBeNull()

    fireEvent.click(screen.getByLabelText("Actions for a.ts"))

    const menu = within(await screen.findByRole("menu"))
    expect(menu.getByText("Stage")).toBeTruthy()
    expect(menu.getByText("Discard…")).toBeTruthy()
  })
})

// Folding: a wholly untracked folder is ONE row on the wire. The browser's
// expand control is a later change; until then a folder row must still read as
// a folder (its path with a trailing slash and how many files it stands for)
// and must never open a diff, which would try to read a directory as a file.
describe("a folded folder row", () => {
  function withFolder(): DuxState {
    return {
      selectedSessionId: "s1",
      changes: {
        ...loadedChanges(),
        unstaged: [
          {
            path: "node_modules",
            status: "??",
            additions: 0,
            deletions: 0,
            binary: false,
            diff_excluded: false,
            kind: "directory",
            file_count: 28747,
          },
          {
            path: "vendor/lib",
            status: "??",
            additions: 0,
            deletions: 0,
            binary: false,
            diff_excluded: false,
            kind: "nested_repository",
          },
        ],
      },
    } as unknown as DuxState
  }

  it("shows the path with a trailing slash and the file count as text", () => {
    mockState = withFolder()
    render(<ChangedFiles />)
    expect(screen.getByText("node_modules/")).toBeTruthy()
    expect(screen.getByText("28,747 files")).toBeTruthy()
    expect(screen.getByText("vendor/lib/")).toBeTruthy()
    expect(screen.getByText("nested repository")).toBeTruthy()
  })

  it("counts the files inside the folder in the group's badge", () => {
    mockState = withFolder()
    render(<ChangedFiles />)
    const heading = screen.getByText("Unstaged").closest("button") as HTMLElement
    expect(within(heading).getByText("28,748")).toBeTruthy()
  })

  it("counts the files inside a checked folder on the bulk bar", () => {
    mockState = withFolder()
    render(<ChangedFiles />)
    fireEvent.click(screen.getByLabelText("Select node_modules"))
    const toolbar = within(
      screen.getByRole("toolbar", { name: "Actions for the selected files" }),
    )
    expect(toolbar.getByText("Stage 28,747")).toBeTruthy()
    expect(toolbar.getByText("Discard 28,747…")).toBeTruthy()
  })

  // The bulk discard names what the user confirmed for each folder, so the
  // server can refuse one that changed kind since the dialog opened.
  it("tells the server what each folder was when the bulk discard was confirmed", async () => {
    mockState = withFolder()
    render(<ChangedFiles />)
    fireEvent.click(screen.getByLabelText("Select node_modules"))
    fireEvent.click(bar().getByRole("button", { name: "Discard 28,747…" }))
    fireEvent.click(
      within(screen.getByRole("dialog")).getByRole("button", { name: "Discard" }),
    )
    await act(() => discardMany.mock.results[0]!.value as Promise<unknown>)
    expect(discardMany).toHaveBeenCalledWith("s1", ["node_modules"], {
      node_modules: { kind: "directory", files: 28747 },
    })
  })

  it("does not open a diff when a folder row is clicked", () => {
    mockState = withFolder()
    render(<ChangedFiles />)
    fireEvent.click(screen.getByText("node_modules/"))
    expect(openEditor).not.toHaveBeenCalled()
  })

  function withWorktree(): DuxState {
    const base = withFolder()
    return {
      ...base,
      changes: {
        ...base.changes,
        unstaged: [
          ...base.changes.unstaged,
          {
            path: "inner-wt",
            status: "??",
            additions: 0,
            deletions: 0,
            binary: false,
            diff_excluded: false,
            kind: "linked_worktree",
          },
        ],
      },
    } as unknown as DuxState
  }

  // A verb with nothing to act on is not offered as a click that silently
  // does nothing: it is disabled, and its tooltip says why.
  it("disables a bulk verb whose count is zero and says why", () => {
    mockState = withWorktree()
    render(<ChangedFiles />)
    fireEvent.click(screen.getByLabelText("Select inner-wt"))

    const stage = bar().getByRole("button", { name: /Stage 0/ })
    const discard = bar().getByRole("button", { name: /Discard 0/ })
    expect((stage as HTMLButtonElement).disabled).toBe(true)
    expect((discard as HTMLButtonElement).disabled).toBe(true)
    const hints = bar()
      .getAllByTestId("tooltip-content")
      .map((el) => el.textContent ?? "")
    expect(hints.some((text) => text.startsWith("Nothing selected can be staged"))).toBe(true)
    expect(hints.some((text) => text.startsWith("Nothing selected can be discarded"))).toBe(true)
  })

  // In a mixed selection the rows a bulk stage leaves out are named with why,
  // because they did not move and nothing on screen says so.
  it("names the rows a mixed bulk stage left out", async () => {
    mockState = withWorktree()
    render(<ChangedFiles />)
    fireEvent.click(screen.getByLabelText("Select node_modules"))
    fireEvent.click(screen.getByLabelText("Select inner-wt"))

    fireEvent.click(bar().getByRole("button", { name: /Stage 28,747/ }))
    await act(() => stageMany.mock.results[0]!.value as Promise<unknown>)

    expect(stageMany).toHaveBeenCalledWith("s1", ["node_modules"])
    const notice = notifyInfo.mock.calls.map((call) => proseText(call[0] as Prose))
    expect(notice).toContain(
      "1 selected row was left out: inner-wt/ is a worktree of this repository, which staging would record as a link.",
    )
  })

  it("names the rows a mixed bulk discard left out", async () => {
    mockState = withWorktree()
    render(<ChangedFiles />)
    fireEvent.click(screen.getByLabelText("Select node_modules"))
    fireEvent.click(screen.getByLabelText("Select inner-wt"))

    fireEvent.click(bar().getByRole("button", { name: /Discard 28,747/ }))
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Discard" }))
    await act(() => discardMany.mock.results[0]!.value as Promise<unknown>)

    const notice = notifyInfo.mock.calls.map((call) => proseText(call[0] as Prose))
    expect(notice).toContain(
      "1 selected row was left out: inner-wt/ is a worktree of this repository, which the worktree manager removes.",
    )
  })

  it("leaves a worktree of this repository out of the bar's counts", () => {
    const base = withFolder()
    mockState = {
      ...base,
      changes: {
        ...base.changes,
        unstaged: [
          ...base.changes.unstaged,
          {
            path: "inner-wt",
            status: "??",
            additions: 0,
            deletions: 0,
            binary: false,
            diff_excluded: false,
            kind: "linked_worktree",
          },
        ],
      },
    } as unknown as DuxState
    render(<ChangedFiles />)
    fireEvent.click(screen.getByLabelText("Select node_modules"))
    fireEvent.click(screen.getByLabelText("Select inner-wt"))
    const toolbar = within(
      screen.getByRole("toolbar", { name: "Actions for the selected files" }),
    )
    expect(toolbar.getByText("Stage 28,747")).toBeTruthy()
    expect(toolbar.getByText("Discard 28,747…")).toBeTruthy()
  })

  it("offers no menu for a row nothing acts on, and the row says what it is", async () => {
    mockState = {
      selectedSessionId: "s1",
      changes: {
        ...loadedChanges(),
        unstaged: [
          {
            path: "vendor",
            status: "??",
            additions: 0,
            deletions: 0,
            binary: false,
            diff_excluded: false,
            kind: "directory",
            file_count: 0,
            nested_repositories: 2,
          },
          {
            path: "inner-wt",
            status: "??",
            additions: 0,
            deletions: 0,
            binary: false,
            diff_excluded: false,
            kind: "linked_worktree",
          },
        ],
      },
    } as unknown as DuxState
    render(<ChangedFiles />)
    // Neither is staged nor discarded: the server refuses a worktree of this
    // repository, and a folder holding only repositories has nothing a stage
    // or a delete would act on. A menu with nothing in it is no menu, so the
    // rows carry no trigger; their labels say what they are.
    expect(screen.getByText("worktree of this repository")).toBeTruthy()
    expect(screen.getByText("2 nested repositories")).toBeTruthy()
    for (const path of ["vendor", "inner-wt"]) {
      expect(screen.queryByLabelText(`Actions for ${path}`)).toBeNull()
    }
  })

  // Staging a folder stages its files and leaves the repositories inside it
  // out; the rows moving says the stage happened, but only a message can say
  // what was left behind.
  it("says which repositories a folder stage left out", async () => {
    mockState = withFolder()
    stageOne.mockResolvedValueOnce({ left_out_repositories: 1, left_out_worktrees: 2 })
    render(<ChangedFiles />)
    fireEvent.click(screen.getByLabelText("Actions for node_modules"))
    const menu = within(await screen.findByRole("menu"))
    fireEvent.click(menu.getByText("Stage"))
    await vi.waitFor(() => expect(notifyInfo).toHaveBeenCalledTimes(1))
    expect(String(notifyInfo.mock.calls[0]![0])).toContain(
      "left out 1 nested repository and 2 worktrees of this repository",
    )
  })

  it("offers staging and discarding the folder but no editor", async () => {
    mockState = withFolder()
    render(<ChangedFiles />)
    fireEvent.click(screen.getByLabelText("Actions for node_modules"))
    const menu = within(await screen.findByRole("menu"))
    expect(menu.getByText("Stage")).toBeTruthy()
    expect(menu.getByText("Discard…")).toBeTruthy()
    expect(menu.queryByText("Edit")).toBeNull()
  })
})
