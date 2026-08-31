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

import type * as React from "react"

import type { ChangesSlice, DuxState } from "@/lib/store"
import { stubMatchMedia, type MatchMediaStub } from "@/test/matchMedia"

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
    expect(box.parentElement!.className).toContain("size-5")
    expect(box.parentElement!.className).toContain("pointer-coarse:size-11")
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
    const view = render(<ChangedFiles />)
    check("a.ts")
    expect(bar().getByRole("button", { name: "Stage 1" })).toBeTruthy()

    mockState = withFiles([["staged.ts", "M"], ["a.ts", "M"]], [["b.ts", "??"]])
    view.rerender(<ChangedFiles />)

    expect(
      screen.queryByRole("toolbar", { name: "Actions for the selected files" }),
    ).toBeNull()
  })

  it("scopes checked paths to their session and restores them on return", () => {
    const view = render(<ChangedFiles />)
    check("a.ts")

    const second = withFiles([], [["other.ts", "M"]])
    mockState = {
      ...second,
      selectedSessionId: "s2",
      changes: { ...second.changes, sessionId: "s2" },
    }
    view.rerender(<ChangedFiles />)

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
    view.rerender(<ChangedFiles />)

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
    expect(notifyWarning).toHaveBeenCalledWith(
      "1 file staged. 1 file had already left the list, starting with b.ts.",
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

  // The 44px floor on BOTH axes, and the checkbox halo grown to fill it rather
  // than suppressed the way the desktop one is. Class pins: the geometry they
  // stand for is measured in the preview container.
  it("gives coarse pointers a larger slot and halo without a React media subscription", () => {
    render(<ChangedFiles />)
    const box = screen.getByLabelText("Select a.ts")
    expect(box.parentElement!.className).toContain("pointer-coarse:size-11")
    expect(box.className).toContain("pointer-coarse:after:-inset-[15px]")
    expect(box.className).toContain("pointer-coarse:after:block")
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

    expect(discardMany).toHaveBeenCalledWith("s1", ["a.ts", "gone.ts"])
    expect(notifySuccess).toHaveBeenCalledTimes(1)
  })

  // The dialog's copy and its target both come from the live unstaged list, so
  // a file that leaves the list while the dialog is open is not discarded.
  it("acts on the survivors when a checked path leaves the list", async () => {
    const view = render(<ChangedFiles />)
    check("a.ts")
    check("gone.ts")
    fireEvent.click(bar().getByRole("button", { name: "Discard 2…" }))

    mockState = withFiles([], [["a.ts", "M"]])
    view.rerender(<ChangedFiles />)

    const dialog = within(screen.getByRole("dialog"))
    expect(dialog.getByText(/1 file/)).toBeTruthy()
    fireEvent.click(dialog.getByRole("button", { name: "Discard" }))
    await act(() => discardMany.mock.results[0]!.value as Promise<unknown>)

    expect(discardMany).toHaveBeenCalledWith("s1", ["a.ts"])
  })

  it("closes itself once every checked path has left the list", async () => {
    const view = render(<ChangedFiles />)
    check("a.ts")
    check("gone.ts")
    fireEvent.click(bar().getByRole("button", { name: "Discard 2…" }))
    expect(screen.getByRole("dialog")).toBeTruthy()

    mockState = withFiles([["a.ts", "M"], ["gone.ts", "??"]], [])
    await act(async () => {
      view.rerender(<ChangedFiles />)
    })

    expect(screen.queryByRole("dialog")).toBeNull()
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
    expect(String(notifyError.mock.calls[0]![0])).toContain("a.ts")
    expect(notifySuccess).not.toHaveBeenCalled()
    expect(notifyWarning).not.toHaveBeenCalled()
  })
})
