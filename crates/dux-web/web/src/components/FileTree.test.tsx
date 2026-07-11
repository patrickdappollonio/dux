// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DirEntry } from "@/lib/fileTree"

// FileTree talks to the server exclusively through `fileApi.tree`, so mocking
// that one function is enough to drive every scenario below without a real
// server. Follows TerminalArea.test.tsx's style of mocking only what's needed.
const treeMock = vi.fn<(sessionId: string, dir: string) => Promise<unknown>>()
vi.mock("@/lib/fileApi", () => ({
  fileApi: {
    tree: (sessionId: string, dir: string) => treeMock(sessionId, dir),
  },
}))

vi.stubGlobal(
  "ResizeObserver",
  class {
    observe() {}
    unobserve() {}
    disconnect() {}
  },
)

const { FileTree } = await import("./FileTree")

function file(path: string): DirEntry {
  const name = path.split("/").pop() ?? path
  return { name, path, is_dir: false, is_symlink: false, expandable: false }
}

function dir(path: string): DirEntry {
  const name = path.split("/").pop() ?? path
  return { name, path, is_dir: true, is_symlink: false, expandable: true }
}

beforeEach(() => {
  treeMock.mockReset()
})

afterEach(() => {
  cleanup()
})

describe("FileTree", () => {
  it("auto-expands ancestors and reveals a deep initialPath on mount", async () => {
    const listings: Record<string, DirEntry[]> = {
      "": [dir("src")],
      src: [dir("src/app")],
      "src/app": [file("src/app/main.ts")],
    }
    treeMock.mockImplementation((_sid, d) =>
      Promise.resolve({ dir: d, entries: listings[d] ?? [] }),
    )

    render(
      <FileTree
        sessionId="s1"
        openPath="src/app/main.ts"
        changed={new Map()}
        initialPath="src/app/main.ts"
        onOpen={() => {}}
      />,
    )

    expect(await screen.findByText("main.ts")).toBeTruthy()
    expect(screen.getByText("src")).toBeTruthy()
    expect(screen.getByText("app")).toBeTruthy()
    expect(treeMock).toHaveBeenCalledWith("s1", "")
    expect(treeMock).toHaveBeenCalledWith("s1", "src")
    expect(treeMock).toHaveBeenCalledWith("s1", "src/app")
  })

  it("refetches a loaded parent once when it doesn't yet list the newly opened file, with no infinite refetch", async () => {
    let srcCalls = 0
    treeMock.mockImplementation((_sid, d) => {
      if (d === "") return Promise.resolve({ dir: "", entries: [dir("src")] })
      if (d === "src") {
        srcCalls++
        const entries =
          srcCalls === 1
            ? [file("src/existing.ts")]
            : [file("src/existing.ts"), file("src/new.ts")]
        return Promise.resolve({ dir: "src", entries })
      }
      return Promise.resolve({ dir: d, entries: [] })
    })

    const { rerender } = render(
      <FileTree
        sessionId="s1"
        openPath="src/existing.ts"
        changed={new Map()}
        initialPath="src/existing.ts"
        onOpen={() => {}}
      />,
    )
    expect(await screen.findByText("existing.ts")).toBeTruthy()
    expect(srcCalls).toBe(1)

    rerender(
      <FileTree
        sessionId="s1"
        openPath="src/new.ts"
        changed={new Map()}
        initialPath="src/existing.ts"
        onOpen={() => {}}
      />,
    )

    expect(await screen.findByText("new.ts")).toBeTruthy()
    expect(srcCalls).toBe(2)

    // Settle further and confirm the parent is not refetched again.
    await act(async () => {
      await new Promise((r) => setTimeout(r, 20))
    })
    rerender(
      <FileTree
        sessionId="s1"
        openPath="src/new.ts"
        changed={new Map()}
        initialPath="src/existing.ts"
        onOpen={() => {}}
      />,
    )
    await act(async () => {
      await new Promise((r) => setTimeout(r, 20))
    })
    expect(srcCalls).toBe(2)
  })

  it("renders a real file named __error__ as a normal, openable entry row (F1)", async () => {
    treeMock.mockImplementation((_sid, d) =>
      Promise.resolve({ dir: d, entries: d === "" ? [file("__error__")] : [] }),
    )
    const onOpen = vi.fn()
    render(
      <FileTree
        sessionId="s1"
        openPath={null}
        changed={new Map()}
        initialPath={null}
        onOpen={onOpen}
      />,
    )
    const row = await screen.findByText("__error__")
    expect(screen.queryByText("Failed to load — retry")).toBeNull()
    fireEvent.click(row)
    expect(onOpen).toHaveBeenCalledWith("__error__")
  })

  it("evicts a collapsed directory's cache so re-expanding refetches (F3)", async () => {
    let srcCalls = 0
    treeMock.mockImplementation((_sid, d) => {
      if (d === "") return Promise.resolve({ dir: "", entries: [dir("src")] })
      if (d === "src") {
        srcCalls++
        return Promise.resolve({ dir: "src", entries: [file("src/a.ts")] })
      }
      return Promise.resolve({ dir: d, entries: [] })
    })
    render(
      <FileTree
        sessionId="s1"
        openPath={null}
        changed={new Map()}
        initialPath={null}
        onOpen={() => {}}
      />,
    )
    const srcRow = await screen.findByText("src")
    fireEvent.click(srcRow) // expand
    await screen.findByText("a.ts")
    expect(srcCalls).toBe(1)

    fireEvent.click(srcRow) // collapse: should evict the cache
    expect(screen.queryByText("a.ts")).toBeNull()

    fireEvent.click(srcRow) // re-expand: should refetch, not reuse the cache
    await screen.findByText("a.ts")
    expect(srcCalls).toBe(2)
  })

  it("fetches a failing ancestor exactly once automatically, and once more per explicit Retry click (F9)", async () => {
    let badCalls = 0
    treeMock.mockImplementation((_sid, d) => {
      if (d === "") return Promise.resolve({ dir: "", entries: [dir("bad")] })
      if (d === "bad") {
        badCalls++
        return Promise.reject(new Error("boom"))
      }
      return Promise.resolve({ dir: d, entries: [] })
    })
    render(
      <FileTree
        sessionId="s1"
        openPath="bad/file.ts"
        changed={new Map()}
        initialPath="bad/file.ts"
        onOpen={() => {}}
      />,
    )
    const retry = await screen.findByText("Failed to load — retry")
    expect(badCalls).toBe(1)

    // Give any (undesired) automatic retry loop a chance to fire.
    await act(async () => {
      await new Promise((r) => setTimeout(r, 30))
    })
    expect(badCalls).toBe(1)

    fireEvent.click(retry)
    await act(async () => {
      await new Promise((r) => setTimeout(r, 10))
    })
    expect(badCalls).toBe(2)
  })

  it("keeps a manually collapsed ancestor collapsed when an unrelated dir loads (F11)", async () => {
    const listings: Record<string, DirEntry[]> = {
      "": [dir("parent"), dir("other")],
      parent: [file("parent/target.ts")],
      other: [file("other/x.ts")],
    }
    treeMock.mockImplementation((_sid, d) =>
      Promise.resolve({ dir: d, entries: listings[d] ?? [] }),
    )
    render(
      <FileTree
        sessionId="s1"
        openPath="parent/target.ts"
        changed={new Map()}
        initialPath="parent/target.ts"
        onOpen={() => {}}
      />,
    )
    expect(await screen.findByText("target.ts")).toBeTruthy()

    const parentRow = screen.getByText("parent").closest("button")
    if (!parentRow) throw new Error("parent row button not found")
    fireEvent.click(parentRow) // collapse
    expect(screen.queryByText("target.ts")).toBeNull()
    expect(parentRow.getAttribute("aria-expanded")).toBe("false")

    const otherRow = screen.getByText("other").closest("button")
    if (!otherRow) throw new Error("other row button not found")
    fireEvent.click(otherRow) // expand an unrelated dir
    expect(await screen.findByText("x.ts")).toBeTruthy()

    expect(screen.queryByText("target.ts")).toBeNull()
    expect(parentRow.getAttribute("aria-expanded")).toBe("false")
  })
})
