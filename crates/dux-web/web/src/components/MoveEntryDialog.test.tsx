// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import { MoveEntryDialog } from "./MoveEntryDialog"

// A tiny worktree: the root holds `lib/`, `src/`, `src-old/` and a README;
// `lib/` holds `util/`; `src/` is empty. Only the network is faked, so the
// real component drives the real fileApi against it.
const TREE: Record<
  string,
  { name: string; path: string; is_dir: boolean; is_symlink: boolean; expandable: boolean }[]
> = {
  "": [
    { name: "lib", path: "lib", is_dir: true, is_symlink: false, expandable: true },
    { name: "src", path: "src", is_dir: true, is_symlink: false, expandable: true },
    { name: "src-old", path: "src-old", is_dir: true, is_symlink: false, expandable: true },
    // A symlinked directory pointing OUT of the worktree. `list_dir` reports
    // it as `is_dir: false` (not merely as non-expandable), which is what
    // keeps it out of the destination list.
    { name: "escape", path: "escape", is_dir: false, is_symlink: true, expandable: false },
    { name: "README.md", path: "README.md", is_dir: false, is_symlink: false, expandable: false },
  ],
  lib: [
    { name: "util", path: "lib/util", is_dir: true, is_symlink: false, expandable: true },
  ],
  src: [],
}

let requestedDirs: string[] = []

function stubTreeFetch() {
  requestedDirs = []
  vi.stubGlobal(
    "fetch",
    vi.fn((_url: string, init: RequestInit) => {
      const dir = (JSON.parse(String(init.body)) as { dir: string }).dir
      requestedDirs.push(dir)
      return Promise.resolve({
        ok: true,
        status: 200,
        json: () => Promise.resolve({ dir, entries: TREE[dir] ?? [] }),
        text: () => Promise.resolve(""),
      } as unknown as Response)
    }),
  )
}

beforeEach(() => {
  stubTreeFetch()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

function renderDialog(
  overrides: Partial<Parameters<typeof MoveEntryDialog>[0]> = {},
) {
  const props = {
    sessionId: "s1",
    target: { path: "src/moveme.txt", isDir: false },
    isDirty: false,
    onClose: vi.fn(),
    onSubmit: vi.fn(() => Promise.resolve()),
    ...overrides,
  } as Parameters<typeof MoveEntryDialog>[0]
  render(<MoveEntryDialog {...props} />)
  return props
}

const upOneLevel = () => screen.getByRole("button", { name: /Up one level/i })
const moveHere = () => screen.getByRole("button", { name: /Move here/i })

describe("MoveEntryDialog", () => {
  // Opening on the entry's own folder is the point: the common move is a short
  // hop, and starting at the root would make every move begin by re-walking
  // back down.
  it("opens browsing the folder the entry is already in", async () => {
    renderDialog()
    await waitFor(() => expect(requestedDirs).toContain("src"))
    expect(screen.getByText(/No folders here/i)).toBeTruthy()
  })

  it("lists only the destination FOLDERS of the browsed directory", async () => {
    renderDialog()
    await waitFor(() => upOneLevel())
    fireEvent.click(upOneLevel())
    await waitFor(() => screen.getByRole("button", { name: /^lib$/ }))
    expect(screen.getByRole("button", { name: /^src$/ })).toBeTruthy()
    expect(screen.getByRole("button", { name: /^src-old$/ })).toBeTruthy()
    expect(screen.queryByRole("button", { name: /README\.md/ })).toBeNull()
  })

  // A symlinked directory whose target is outside the worktree is reported as
  // not a directory at all; offering it would invite a move the server
  // refuses.
  it("does not offer a symlinked directory that escapes the worktree", async () => {
    renderDialog()
    await waitFor(() => upOneLevel())
    fireEvent.click(upOneLevel())
    await waitFor(() => screen.getByRole("button", { name: /^lib$/ }))
    expect(screen.queryByRole("button", { name: /^escape$/ })).toBeNull()
  })

  it("descending into a folder fetches it and names the landing path", async () => {
    renderDialog()
    await waitFor(() => upOneLevel())
    fireEvent.click(upOneLevel())
    await waitFor(() => screen.getByRole("button", { name: /^lib$/ }))
    fireEvent.click(screen.getByRole("button", { name: /^lib$/ }))
    await waitFor(() => expect(requestedDirs).toContain("lib"))
    await waitFor(() => screen.getByRole("button", { name: /^util$/ }))
    expect(screen.getByText("lib/moveme.txt")).toBeTruthy()
  })

  it("submits the browsed destination directory", async () => {
    const props = renderDialog()
    await waitFor(() => upOneLevel())
    fireEvent.click(upOneLevel())
    await waitFor(() => screen.getByRole("button", { name: /^lib$/ }))
    fireEvent.click(screen.getByRole("button", { name: /^lib$/ }))
    await waitFor(() => screen.getByRole("button", { name: /^util$/ }))
    fireEvent.click(moveHere())
    expect(props.onSubmit).toHaveBeenCalledWith("lib")
  })

  it("submits the worktree root as an empty destination directory", async () => {
    const props = renderDialog()
    await waitFor(() => upOneLevel())
    fireEvent.click(upOneLevel())
    await waitFor(() => screen.getByRole("button", { name: /^lib$/ }))
    fireEvent.click(moveHere())
    expect(props.onSubmit).toHaveBeenCalledWith("")
  })

  it("refuses the folder the entry already lives in", async () => {
    renderDialog()
    await waitFor(() => screen.getByText(/already the folder it is in/i))
    expect(moveHere().hasAttribute("disabled")).toBe(true)
  })

  it("refuses moving a folder inside itself", async () => {
    // `lib`'s own parent is the root, so the browse opens there.
    renderDialog({ target: { path: "lib", isDir: true } })
    await waitFor(() => screen.getByRole("button", { name: /^lib$/ }))
    fireEvent.click(screen.getByRole("button", { name: /^lib$/ }))
    await waitFor(() => screen.getByText(/cannot be moved inside itself/i))
    expect(moveHere().hasAttribute("disabled")).toBe(true)
  })

  // A sibling whose name merely shares a prefix is a legitimate destination.
  it("accepts a sibling folder whose name shares a prefix with the source", async () => {
    const props = renderDialog({ target: { path: "src", isDir: true } })
    await waitFor(() => screen.getByRole("button", { name: /^src-old$/ }))
    fireEvent.click(screen.getByRole("button", { name: /^src-old$/ }))
    await waitFor(() => expect(requestedDirs).toContain("src-old"))
    fireEvent.click(moveHere())
    expect(props.onSubmit).toHaveBeenCalledWith("src-old")
  })

  // Same gate as Rename: moving a file with an unsaved draft would retarget
  // its tab and reload it, dropping the draft.
  it("blocks a target with unsaved changes", async () => {
    renderDialog({ isDirty: true })
    await waitFor(() => screen.getByText(/before moving it/i))
    expect(moveHere().hasAttribute("disabled")).toBe(true)
  })

  // Stepping into a folder unmounts the button that was clicked, and the
  // browser drops focus onto the dialog container, so a keyboard user would
  // restart their Tab walk at every level. Focus lands on the first control of
  // the newly listed folder instead.
  it("keeps keyboard focus in the list after stepping into a folder", async () => {
    renderDialog()
    await waitFor(() => upOneLevel())
    fireEvent.click(upOneLevel())
    await waitFor(() => screen.getByRole("button", { name: /^lib$/ }))
    fireEvent.click(screen.getByRole("button", { name: /^lib$/ }))
    await waitFor(() => screen.getByRole("button", { name: /^util$/ }))
    // Focus lands from an effect that runs after the listing resolves, and
    // under a loaded full suite that can take longer than waitFor's default
    // 1s window: this assertion flaked twice in two days and passed in
    // isolation every time. The wait is what is generous, not the behaviour.
    await waitFor(() => expect(document.activeElement).toBe(upOneLevel()), {
      timeout: 5000,
    })
  })

  it("can climb back up towards the worktree root", async () => {
    renderDialog()
    await waitFor(() => upOneLevel())
    fireEvent.click(upOneLevel())
    await waitFor(() => screen.getByRole("button", { name: /^src-old$/ }))
    // At the root there is nothing above, so the climb control is gone.
    expect(screen.queryByRole("button", { name: /Up one level/i })).toBeNull()
  })
})
