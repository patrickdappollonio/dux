// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import { installBrowserHistory } from "@/test/browserHistory"

// Override `useDux` with a seeded state the mocked setters write back into, and
// spy the store actions the dialog fires; every other store export stays real.
let mockState: DuxState
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    closeRename: vi.fn(),
    submitRename: vi.fn(),
    setRenameDraft: vi.fn(),
    setRenameBranch: vi.fn(),
  }
})

vi.mock("@/lib/sessionsApi", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/sessionsApi")>()
  return {
    ...actual,
    sessionsApi: {
      ...actual.sessionsApi,
      branchPushed: vi.fn(),
      patch: vi.fn(),
    },
  }
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
}
installBootStubs()
installBrowserHistory()
const { RenameSessionDialog } = await import("./RenameSessionDialog")
const store = await import("@/lib/store")
const api = await import("@/lib/sessionsApi")
const { renameDraftValue, renamePayload } = await import("@/lib/renameAgent")
const submitRename = vi.mocked(store.submitRename)
const setRenameDraft = vi.mocked(store.setRenameDraft)
const setRenameBranch = vi.mocked(store.setRenameBranch)
const branchPushed = vi.mocked(api.sessionsApi.branchPushed)
const patch = vi.mocked(api.sessionsApi.patch)

const managedSession = {
  id: "s1",
  title: null,
  workspace: {
    kind: "managed" as const,
    project_id: "p1",
    branch_name: "feat/login",
    initial_branch: "feat/login",
    branch_provenance: "created" as const,
    source_branch: "main",
    worktree_path: "/tmp/feat-login",
  },
}

const standaloneSession = {
  id: "sa1",
  title: "My Notes",
  workspace: {
    kind: "folder" as const,
    folder_path: "/srv/someone/My Notes",
    folder_label: "/srv/someone/My Notes",
    repo_status: "no_repo" as const,
    quiet_reason: "",
  },
}

function seed(target: string, session: object, draft = "") {
  mockState = {
    renameTarget: target,
    renameDraft: draft,
    renameBranch: false,
    spine: { sessions: [session] },
  } as unknown as DuxState
}

function renderDialog() {
  const view = render(<RenameSessionDialog />)
  const sync = () => view.rerender(<RenameSessionDialog />)
  setRenameDraft.mockImplementation((raw: string) => {
    const session = (mockState.spine?.sessions ?? [])[0]
    const branchNamed = session?.workspace.kind === "managed"
    mockState = { ...mockState, renameDraft: renameDraftValue(raw, branchNamed) }
    sync()
  })
  setRenameBranch.mockImplementation((renameBranch: boolean) => {
    mockState = { ...mockState, renameBranch }
    sync()
  })
  return view
}

function saveButton(): HTMLButtonElement {
  return screen.getByRole("button", { name: "Save" }) as HTMLButtonElement
}

function note(): HTMLElement {
  return screen.getByTestId("rename-branch-note")
}

beforeEach(() => {
  installBootStubs()
  submitRename.mockClear()
  setRenameDraft.mockReset()
  setRenameBranch.mockReset()
  patch.mockReset()
  patch.mockResolvedValue({})
  branchPushed.mockReset()
  branchPushed.mockResolvedValue({ pushed: false })
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("RenameSessionDialog", () => {
  it("offers the branch box unticked, with the branch name as the field's hint", () => {
    seed("s1", managedSession)
    renderDialog()
    const box = screen.getByRole("checkbox", { name: "Also rename the git branch" })
    expect(box.getAttribute("aria-checked")).toBe("false")
    expect(screen.getByRole("textbox", { name: "Agent name" }).getAttribute("placeholder"))
      .toBe("feat/login")
    expect(saveButton().disabled).toBe(false)
  })

  it("refuses to save an empty name while the branch box is ticked, and says why", () => {
    seed("s1", managedSession)
    renderDialog()
    fireEvent.click(screen.getByRole("checkbox", { name: "Also rename the git branch" }))
    expect(setRenameBranch).toHaveBeenCalledWith(true)
    expect(saveButton().disabled).toBe(true)
    expect(screen.getByRole("alert").textContent).toBe(
      "Type a name to rename the branch to. An empty name only resets the agent's name.",
    )
    fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" })
    expect(submitRename).not.toHaveBeenCalled()

    fireEvent.change(screen.getByRole("textbox"), { target: { value: "feat/signin" } })
    expect(saveButton().disabled).toBe(false)
    fireEvent.click(saveButton())
    expect(submitRename).toHaveBeenCalledOnce()
  })

  it("saves an empty name as a reset", () => {
    seed("s1", managedSession)
    renderDialog()
    fireEvent.click(saveButton())
    expect(submitRename).toHaveBeenCalledOnce()
    expect(renamePayload("", false, true)).toEqual({ title: "", rename_branch: false })
    expect(renamePayload(" next ", true, true)).toEqual({ title: "next", rename_branch: true })
  })

  it("sends the branch flag with the title", async () => {
    await store.renameSession("s1", "next", true)
    expect(patch).toHaveBeenCalledWith("s1", { title: "next", rename_branch: true })
    await store.renameSession("s1", "")
    expect(patch).toHaveBeenLastCalledWith("s1", { title: "", rename_branch: false })
  })

  it("gives a standalone agent no box, its folder's name as the hint, and keeps spaces", () => {
    seed("sa1", standaloneSession, "My Notes")
    renderDialog()
    expect(screen.queryByRole("checkbox")).toBeNull()
    expect(screen.queryByText(/Letters, digits/)).toBeNull()
    expect(screen.getByText(/show its folder's name again/)).toBeTruthy()
    const field = screen.getByRole("textbox", { name: "Agent name" })
    expect(field.getAttribute("placeholder")).toBe("My Notes")
    fireEvent.change(field, { target: { value: "My Notes (2026)" } })
    expect((field as HTMLInputElement).value).toBe("My Notes (2026)")
    expect(saveButton().disabled).toBe(false)
    fireEvent.click(saveButton())
    expect(submitRename).toHaveBeenCalledOnce()
    expect(renamePayload("My Notes (2026)", true, false)).toEqual({
      title: "My Notes (2026)",
      rename_branch: false,
    })
    expect(branchPushed).not.toHaveBeenCalled()
  })

  it("says only what the box does when nothing is pushed", async () => {
    seed("s1", managedSession)
    renderDialog()
    await waitFor(() => expect(branchPushed).toHaveBeenCalledWith("s1"))
    expect(note().textContent).toBe("Renames feat/login to the new name.")
    expect([...note().querySelectorAll("code")].map((c) => c.textContent)).toEqual([
      "feat/login",
    ])
  })

  it("warns that the open pull request and the pushed branch keep the old name", async () => {
    branchPushed.mockResolvedValue({ pushed: true })
    seed("s1", {
      ...managedSession,
      pr: { number: 42, state: "open", title: "Login", url: "u", overridden: false },
    })
    renderDialog()
    await waitFor(() =>
      expect(note().textContent).toBe(
        "Renames feat/login on this machine only: pull request #42 and the pushed branch stay on the old name.",
      ),
    )
    expect(note().className).toContain("text-dux-warning")
  })

  it("warns about a pushed branch with no pull request", async () => {
    branchPushed.mockResolvedValue({ pushed: true })
    seed("s1", {
      ...managedSession,
      pr: { number: 7, state: "merged", title: "Old", url: "u", overridden: false },
    })
    renderDialog()
    await waitFor(() =>
      expect(note().textContent).toBe(
        "Renames feat/login on this machine only: the pushed branch on the remote keeps the old name.",
      ),
    )
  })
})
