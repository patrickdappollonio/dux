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

// FileTree renders its own base-ui ScrollArea, whose viewport probes
// `getAnimations` on a timer; jsdom doesn't implement it.
if (!Element.prototype.getAnimations) {
  Element.prototype.getAnimations = () => []
}

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

  // classList.contains does exact-token matching (unlike a `[class*=]` CSS
  // selector, which would substring-match "lucide-folder" against
  // "lucide-folder-open" too), so this reliably distinguishes lucide's
  // sibling icon names.
  function hasIcon(el: Element | null, lucideClass: string): boolean {
    if (!el) return false
    return [...el.querySelectorAll("svg")].some((svg) =>
      svg.classList.contains(lucideClass),
    )
  }

  it("renders a type-aware icon for a code file, and folder/folder-open for a directory", async () => {
    treeMock.mockImplementation((_sid, d) =>
      Promise.resolve({
        dir: d,
        entries: d === "" ? [dir("src"), file("main.ts")] : [file("src/lib.rs")],
      }),
    )
    render(
      <FileTree
        sessionId="s1"
        openPath={null}
        changed={new Map()}
        initialPath={null}
        onOpen={() => {}}
      />,
    )
    await screen.findByText("main.ts")
    const fileRow = screen.getByText("main.ts").closest("button")
    expect(hasIcon(fileRow, "lucide-file-code")).toBe(true)

    const dirRow = screen.getByText("src").closest("button")
    expect(hasIcon(dirRow, "lucide-folder")).toBe(true)
    expect(hasIcon(dirRow, "lucide-folder-open")).toBe(false)

    fireEvent.click(dirRow!)
    await screen.findByText("lib.rs")
    expect(hasIcon(dirRow, "lucide-folder-open")).toBe(true)
  })

  it("renders the distinct empty-folder glyph for a loaded, zero-child directory", async () => {
    treeMock.mockImplementation((_sid, d) =>
      Promise.resolve({ dir: d, entries: d === "" ? [dir("empty-dir")] : [] }),
    )
    render(
      <FileTree
        sessionId="s1"
        openPath={null}
        changed={new Map()}
        initialPath={null}
        onOpen={() => {}}
      />,
    )
    const dirRow = await screen.findByText("empty-dir")
    const button = dirRow.closest("button")
    fireEvent.click(button!) // expand — triggers the fetch that reveals "empty"
    await act(async () => {
      await new Promise((r) => setTimeout(r, 10))
    })
    expect(hasIcon(button, "lucide-folder-x")).toBe(true)
  })

  it("double-click on a file row calls onOpen with { pin: true }", async () => {
    treeMock.mockImplementation((_sid, d) =>
      Promise.resolve({ dir: d, entries: d === "" ? [file("main.ts")] : [] }),
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
    const row = await screen.findByText("main.ts")
    fireEvent.doubleClick(row)
    expect(onOpen).toHaveBeenCalledWith("main.ts", { pin: true })
  })

  // A real browser double-click fires click, click, THEN dblclick, in that
  // order, not just a single doubleclick event. The comment above FileTree's
  // `onOpen` prop claims the two preceding `onClick`s are "harmless" because
  // `openFile` (lib/editorTabs.ts) is idempotent for an already-open path; this
  // exercises that actual three-event sequence rather than only the synthetic
  // `fireEvent.doubleClick` shortcut used above, so a regression that makes the
  // preceding clicks NOT harmless (e.g. clobbering the pin) would be caught.
  it("a real click, click, dblclick sequence calls onOpen with the preview opens first, then the pin call", async () => {
    treeMock.mockImplementation((_sid, d) =>
      Promise.resolve({ dir: d, entries: d === "" ? [file("main.ts")] : [] }),
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
    const row = await screen.findByText("main.ts")
    fireEvent.click(row)
    fireEvent.click(row)
    fireEvent.doubleClick(row)
    expect(onOpen.mock.calls).toEqual([
      ["main.ts"],
      ["main.ts"],
      ["main.ts", { pin: true }],
    ])
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

  // The tree must own its scroll surface: virtualization has to window against
  // the SAME element the user scrolls. Regression: the tree used to render an
  // unbounded inner div inside the sidebar's outer ScrollArea, so scrolling
  // happened on an ancestor its scroll handler never saw and everything past
  // the first screenful rendered as blank spacer (huge dirs like
  // target/debug/deps showed a handful of rows, then nothing).
  it("windows rows against its own scroll viewport and reveals rows on scroll", async () => {
    const ROW_HEIGHT = 28
    const many = Array.from({ length: 300 }, (_, i) =>
      file(`file-${String(i).padStart(3, "0")}.txt`),
    )
    treeMock.mockImplementation((_sid, d) =>
      Promise.resolve({ dir: d, entries: d === "" ? many : [] }),
    )

    const { container } = render(
      <FileTree
        sessionId="s1"
        openPath={null}
        changed={new Map()}
        initialPath={null}
        onOpen={() => {}}
      />,
    )
    expect(await screen.findByText("file-000.txt")).toBeTruthy()

    // The scroller must be the tree's own ScrollArea viewport.
    const viewport = container.querySelector<HTMLDivElement>(
      '[data-slot="scroll-area-viewport"]',
    )
    if (!viewport) throw new Error("FileTree does not own a scroll viewport")

    // jsdom has no layout: pin the viewport's geometry, then scroll it.
    Object.defineProperty(viewport, "clientHeight", {
      configurable: true,
      value: 10 * ROW_HEIGHT,
    })
    Object.defineProperty(viewport, "scrollTop", {
      configurable: true,
      writable: true,
      value: 150 * ROW_HEIGHT,
    })
    fireEvent.scroll(viewport)

    // The window follows the scroll: rows near index 150 exist, the top rows
    // are no longer in the DOM, and the total row count stays bounded by the
    // viewport window (10 visible + overscan), not the 300-entry listing.
    expect(screen.getByText("file-150.txt")).toBeTruthy()
    expect(screen.queryByText("file-000.txt")).toBeNull()
    const rendered = container.querySelectorAll("li").length
    expect(rendered).toBeLessThan(60)
  })
})
