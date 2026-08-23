// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DirEntry } from "@/lib/fileTree"
import type { DroppedItems } from "@/lib/editorDrop"
import { agentRoot } from "@/lib/editorRoot"

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
        root={agentRoot("s1")}
        openPath="src/app/main.ts"
        changed={new Map()}
        initialPath="src/app/main.ts"
        onOpen={() => {}}
      />,
    )

    expect(await screen.findByText("main.ts")).toBeTruthy()
    expect(screen.getByText("src")).toBeTruthy()
    expect(screen.getByText("app")).toBeTruthy()
    expect(treeMock).toHaveBeenCalledWith(agentRoot("s1"), "")
    expect(treeMock).toHaveBeenCalledWith(agentRoot("s1"), "src")
    expect(treeMock).toHaveBeenCalledWith(agentRoot("s1"), "src/app")
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
        root={agentRoot("s1")}
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
        root={agentRoot("s1")}
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
        root={agentRoot("s1")}
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

  it("renders a real file named __error__ as a normal, openable entry row", async () => {
    treeMock.mockImplementation((_sid, d) =>
      Promise.resolve({ dir: d, entries: d === "" ? [file("__error__")] : [] }),
    )
    const onOpen = vi.fn()
    render(
      <FileTree
        root={agentRoot("s1")}
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
        root={agentRoot("s1")}
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
        root={agentRoot("s1")}
        openPath={null}
        changed={new Map()}
        initialPath={null}
        onOpen={() => {}}
      />,
    )
    const dirRow = await screen.findByText("empty-dir")
    const button = dirRow.closest("button")
    fireEvent.click(button!) // expand, triggers the fetch that reveals "empty"
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
        root={agentRoot("s1")}
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
        root={agentRoot("s1")}
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

  it("evicts a collapsed directory's cache so re-expanding refetches", async () => {
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
        root={agentRoot("s1")}
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

  it("fetches a failing ancestor exactly once automatically, and once more per explicit Retry click", async () => {
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
        root={agentRoot("s1")}
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

  it("keeps a manually collapsed ancestor collapsed when an unrelated dir loads", async () => {
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
        root={agentRoot("s1")}
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
  // the SAME element the user scrolls. An unbounded inner div inside the
  // sidebar's outer ScrollArea would scroll on an ancestor the scroll handler
  // never sees, rendering everything past the first screenful as blank spacer
  // (huge dirs like target/debug/deps show a handful of rows, then nothing).
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
        root={agentRoot("s1")}
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

  describe("context menu", () => {
    it("right-clicking a file row opens a menu with all four actions, each with an icon", async () => {
      treeMock.mockImplementation((_sid, d) =>
        Promise.resolve({ dir: d, entries: d === "" ? [file("a.ts")] : [] }),
      )
      render(
        <FileTree
          root={agentRoot("s1")}
          openPath={null}
          changed={new Map()}
          initialPath={null}
          onOpen={() => {}}
          onNewFile={() => {}}
          onNewFolder={() => {}}
          onRename={() => {}}
          onDelete={() => {}}
        />,
      )
      const row = await screen.findByText("a.ts")
      fireEvent.contextMenu(row)

      const newFile = await screen.findByText("New File…")
      const newFolder = await screen.findByText("New Folder…")
      const rename = await screen.findByText("Rename…")
      const del = await screen.findByText("Delete…")
      for (const item of [newFile, newFolder, rename, del]) {
        const menuItem = item.closest('[data-slot="context-menu-item"]')
        expect(menuItem, `${item.textContent} has no menu-item wrapper`).toBeTruthy()
        expect(
          menuItem?.querySelector("svg"),
          `${item.textContent} has no leading icon`,
        ).toBeTruthy()
      }
    })

    it("right-clicking a folder row targets New File/New Folder at that folder's own path", async () => {
      const onNewFile = vi.fn()
      treeMock.mockImplementation((_sid, d) =>
        Promise.resolve({ dir: d, entries: d === "" ? [dir("src")] : [] }),
      )
      render(
        <FileTree
          root={agentRoot("s1")}
          openPath={null}
          changed={new Map()}
          initialPath={null}
          onOpen={() => {}}
          onNewFile={onNewFile}
          onNewFolder={() => {}}
          onRename={() => {}}
          onDelete={() => {}}
        />,
      )
      const row = await screen.findByText("src")
      fireEvent.contextMenu(row)
      const newFile = await screen.findByText("New File…")
      fireEvent.click(newFile)
      expect(onNewFile).toHaveBeenCalledWith("src")
    })

    it("right-clicking a file row targets New File/New Folder at the file's parent dir", async () => {
      const onNewFolder = vi.fn()
      treeMock.mockImplementation((_sid, d) =>
        Promise.resolve({
          dir: d,
          entries: d === "" ? [dir("src")] : d === "src" ? [file("src/a.ts")] : [],
        }),
      )
      render(
        <FileTree
          root={agentRoot("s1")}
          openPath={null}
          changed={new Map()}
          initialPath="src/a.ts"
          onOpen={() => {}}
          onNewFile={() => {}}
          onNewFolder={onNewFolder}
          onRename={() => {}}
          onDelete={() => {}}
        />,
      )
      const row = await screen.findByText("a.ts")
      fireEvent.contextMenu(row)
      const newFolder = await screen.findByText("New Folder…")
      fireEvent.click(newFolder)
      expect(onNewFolder).toHaveBeenCalledWith("src")
    })

    it("right-clicking the empty area below the rows opens the root menu with only New File/New Folder", async () => {
      treeMock.mockImplementation((_sid, d) =>
        Promise.resolve({ dir: d, entries: d === "" ? [file("a.ts")] : [] }),
      )
      const { container } = render(
        <FileTree
          root={agentRoot("s1")}
          openPath={null}
          changed={new Map()}
          initialPath={null}
          onOpen={() => {}}
          onNewFile={() => {}}
          onNewFolder={() => {}}
          onRename={() => {}}
          onDelete={() => {}}
        />,
      )
      await screen.findByText("a.ts")
      // Right-click the root filler div directly (not a row): this is the
      // "empty area below the rows" trigger.
      const filler = container.querySelector(
        '[data-slot="context-menu-trigger"]',
      )
      if (!filler) throw new Error("root context menu trigger not found")
      fireEvent.contextMenu(filler)

      expect(await screen.findByText("New File…")).toBeTruthy()
      expect(screen.getByText("New Folder…")).toBeTruthy()
      expect(screen.queryByText("Rename…")).toBeNull()
      expect(screen.queryByText("Delete…")).toBeNull()
    })

    it("clicking New File… in the root menu targets the worktree root", async () => {
      const onNewFile = vi.fn()
      treeMock.mockImplementation((_sid, d) =>
        Promise.resolve({ dir: d, entries: d === "" ? [file("a.ts")] : [] }),
      )
      const { container } = render(
        <FileTree
          root={agentRoot("s1")}
          openPath={null}
          changed={new Map()}
          initialPath={null}
          onOpen={() => {}}
          onNewFile={onNewFile}
          onNewFolder={() => {}}
          onRename={() => {}}
          onDelete={() => {}}
        />,
      )
      await screen.findByText("a.ts")
      const filler = container.querySelector(
        '[data-slot="context-menu-trigger"]',
      )
      if (!filler) throw new Error("root context menu trigger not found")
      fireEvent.contextMenu(filler)
      fireEvent.click(await screen.findByText("New File…"))
      expect(onNewFile).toHaveBeenCalledWith("")
    })

    it("clicking New Folder… in the root menu targets the worktree root", async () => {
      const onNewFolder = vi.fn()
      treeMock.mockImplementation((_sid, d) =>
        Promise.resolve({ dir: d, entries: d === "" ? [file("a.ts")] : [] }),
      )
      const { container } = render(
        <FileTree
          root={agentRoot("s1")}
          openPath={null}
          changed={new Map()}
          initialPath={null}
          onOpen={() => {}}
          onNewFile={() => {}}
          onNewFolder={onNewFolder}
          onRename={() => {}}
          onDelete={() => {}}
        />,
      )
      await screen.findByText("a.ts")
      const filler = container.querySelector(
        '[data-slot="context-menu-trigger"]',
      )
      if (!filler) throw new Error("root context menu trigger not found")
      fireEvent.contextMenu(filler)
      fireEvent.click(await screen.findByText("New Folder…"))
      expect(onNewFolder).toHaveBeenCalledWith("")
    })

    it("clicking Rename/Delete in a row's menu fires the callback with the row's path and isDir", async () => {
      const onRename = vi.fn()
      const onDelete = vi.fn()
      treeMock.mockImplementation((_sid, d) =>
        Promise.resolve({ dir: d, entries: d === "" ? [file("a.ts")] : [] }),
      )
      render(
        <FileTree
          root={agentRoot("s1")}
          openPath={null}
          changed={new Map()}
          initialPath={null}
          onOpen={() => {}}
          onNewFile={() => {}}
          onNewFolder={() => {}}
          onRename={onRename}
          onDelete={onDelete}
        />,
      )
      const row = await screen.findByText("a.ts")
      fireEvent.contextMenu(row)
      fireEvent.click(await screen.findByText("Rename…"))
      expect(onRename).toHaveBeenCalledWith("a.ts", false)

      fireEvent.contextMenu(row)
      fireEvent.click(await screen.findByText("Delete…"))
      expect(onDelete).toHaveBeenCalledWith("a.ts", false)
    })
  })

  // Dropping files onto the tree is the DURABLE half of dux's two drop
  // intents: "add this file to my project", saved where the user pointed. The
  // rule these tests pin is where "where the user pointed" resolves to, since
  // getting it wrong writes a file into the wrong folder silently.
  describe("dropping files onto the tree", () => {
    // jsdom builds no DataTransfer, so a drag is described by the minimum the
    // handlers actually read: what kinds it carries, the files, and the items
    // (which is the only thing that can tell a folder from a file).
    function fileDrag(files: File[], folders: string[] = []) {
      return {
        dataTransfer: {
          types: ["Files"],
          files,
          items: [
            ...files.map((f) => ({
              kind: "file",
              type: f.type,
              webkitGetAsEntry: () => ({ isDirectory: false, name: f.name }),
            })),
            ...folders.map((name) => ({
              kind: "file",
              type: "",
              webkitGetAsEntry: () => ({ isDirectory: true, name }),
            })),
          ],
          dropEffect: "none",
        },
      }
    }

    // What the highlight actually IS: the classes the row paints while it is
    // the drop target. Asserted rather than a marker attribute, because a
    // marker no CSS reads keeps its test green while the tree lights up
    // nothing at all, which is exactly what the previous version of these
    // tests did.
    const DROP_HIGHLIGHT = ["bg-primary/10", "ring-1", "ring-primary"]
    const isHighlighted = (el: Element | null) =>
      el !== null && DROP_HIGHLIGHT.every((c) => el.classList.contains(c))
    // Every row's drop target is the row's own button; empty space is the
    // filler surface.
    const rowTarget = (label: string) => screen.getByText(label).closest("button")

    async function tree(
      onFilesDropped: (dir: string, dropped: DroppedItems) => void,
    ) {
      treeMock.mockImplementation((_sid, d) =>
        Promise.resolve({
          dir: d,
          entries:
            d === "" ? [dir("src"), file("README.md")] : [file("src/a.ts")],
        }),
      )
      render(
        <FileTree
          root={agentRoot("s1")}
          openPath={null}
          changed={new Map()}
          initialPath={null}
          onOpen={() => {}}
          fileDropEnabled
          onFilesDropped={onFilesDropped}
        />,
      )
      expect(await screen.findByText("README.md")).toBeTruthy()
    }

    const dropped = [new File(["x"], "logo.png")]
    const asFiles = (files: File[]) => ({ files, folders: [] })

    it("targets the folder itself when dropped on a folder row", async () => {
      const onFilesDropped = vi.fn()
      await tree(onFilesDropped)
      fireEvent.drop(screen.getByText("src"), fileDrag(dropped))
      expect(onFilesDropped).toHaveBeenCalledWith("src", asFiles(dropped))
    })

    it("targets the PARENT folder when dropped on a file row", async () => {
      // A file is not a place to put a file. Every other tree action that
      // needs a destination folder resolves a file row the same way.
      const onFilesDropped = vi.fn()
      await tree(onFilesDropped)
      fireEvent.drop(screen.getByText("README.md"), fileDrag(dropped))
      expect(onFilesDropped).toHaveBeenCalledWith("", asFiles(dropped))
    })

    it("targets the worktree root when dropped on empty tree space", async () => {
      const onFilesDropped = vi.fn()
      await tree(onFilesDropped)
      fireEvent.drop(
        screen.getByTestId("file-tree-drop-surface"),
        fileDrag(dropped),
      )
      expect(onFilesDropped).toHaveBeenCalledWith("", asFiles(dropped))
    })

    it("passes every file of a multi-file drop through in one call", async () => {
      const onFilesDropped = vi.fn()
      await tree(onFilesDropped)
      const many = [new File(["a"], "a.png"), new File(["b"], "b.png")]
      fireEvent.drop(screen.getByText("src"), fileDrag(many))
      expect(onFilesDropped).toHaveBeenCalledTimes(1)
      expect(onFilesDropped).toHaveBeenCalledWith("src", asFiles(many))
    })

    it("reports a dropped FOLDER as a folder rather than as a file", async () => {
      // Dropping a folder on a file tree is an entirely natural gesture. What
      // arrives for one is browser-dependent: in one shape it rides in `files`
      // as an entry whose read fails, which uploaded as a file produces a
      // transport-shaped error blaming the network. The tree sorts it out here,
      // because this is the only place the DataTransfer is reachable.
      const onFilesDropped = vi.fn()
      await tree(onFilesDropped)
      const folderAsFile = new File([], "icons")
      fireEvent.drop(
        screen.getByText("src"),
        fileDrag([...dropped, folderAsFile], ["icons"]),
      )
      expect(onFilesDropped).toHaveBeenCalledWith("src", {
        files: dropped,
        folders: ["icons"],
      })
    })

    it("still reports a drop that delivered nothing identifiable", async () => {
      // The other shape: the drag said it carried files and neither a file nor
      // an item arrived. Reporting nothing here is how letting go of a folder
      // came to look exactly like letting go of nothing.
      const onFilesDropped = vi.fn()
      await tree(onFilesDropped)
      fireEvent.drop(screen.getByText("src"), {
        dataTransfer: { types: ["Files"], files: [], items: [] },
      })
      expect(onFilesDropped).toHaveBeenCalledWith("src", {
        files: [],
        folders: [],
      })
    })

    it("highlights the row that would receive the drop, and only that one", async () => {
      // The target has to be obvious BEFORE the drop lands, or the user finds
      // out where the file went by reading a toast afterwards. The assertion is
      // on the CLASSES the row paints, so deleting the highlight fails here.
      const onFilesDropped = vi.fn()
      await tree(onFilesDropped)
      expect(isHighlighted(rowTarget("src"))).toBe(false)

      fireEvent.dragEnter(screen.getByText("src"), fileDrag(dropped))
      fireEvent.dragOver(screen.getByText("src"), fileDrag(dropped))
      expect(isHighlighted(rowTarget("src"))).toBe(true)
      expect(isHighlighted(rowTarget("README.md"))).toBe(false)

      fireEvent.drop(screen.getByText("src"), fileDrag(dropped))
      expect(isHighlighted(rowTarget("src"))).toBe(false)
    })

    it("highlights the empty-space surface when the drag is over the root", async () => {
      const onFilesDropped = vi.fn()
      await tree(onFilesDropped)
      const surface = screen.getByTestId("file-tree-drop-surface")
      expect(isHighlighted(surface)).toBe(false)
      fireEvent.dragEnter(surface, fileDrag(dropped))
      expect(isHighlighted(surface)).toBe(true)
      expect(isHighlighted(rowTarget("src"))).toBe(false)
    })

    it("clears the highlight when the drag leaves without dropping", async () => {
      const onFilesDropped = vi.fn()
      await tree(onFilesDropped)
      fireEvent.dragEnter(screen.getByText("src"), fileDrag(dropped))
      expect(isHighlighted(rowTarget("src"))).toBe(true)
      fireEvent.dragLeave(screen.getByText("src"), fileDrag(dropped))
      expect(isHighlighted(rowTarget("src"))).toBe(false)
      expect(onFilesDropped).not.toHaveBeenCalled()
    })

    it("ignores a drag that carries no files, so an in-app drag never uploads", async () => {
      const onFilesDropped = vi.fn()
      await tree(onFilesDropped)
      const textDrag = {
        dataTransfer: { types: ["text/plain"], files: [], items: [] },
      }
      fireEvent.dragEnter(screen.getByText("src"), textDrag)
      expect(isHighlighted(rowTarget("src"))).toBe(false)
      fireEvent.drop(screen.getByText("src"), textDrag)
      expect(onFilesDropped).not.toHaveBeenCalled()
    })

    it("does nothing at all when file drop is switched off on the server", async () => {
      const onFilesDropped = vi.fn()
      treeMock.mockImplementation((_sid, d) =>
        Promise.resolve({ dir: d, entries: d === "" ? [dir("src")] : [] }),
      )
      render(
        <FileTree
          root={agentRoot("s1")}
          openPath={null}
          changed={new Map()}
          initialPath={null}
          onOpen={() => {}}
          fileDropEnabled={false}
          onFilesDropped={onFilesDropped}
        />,
      )
      const row = await screen.findByText("src")
      fireEvent.dragEnter(row, fileDrag(dropped))
      expect(isHighlighted(row.closest("button"))).toBe(false)
      fireEvent.drop(row, fileDrag(dropped))
      expect(onFilesDropped).not.toHaveBeenCalled()
    })
  })

  // "UPLOAD HERE…": the picker gesture into the tree's own drop intent. It
  // reuses the drop's destination resolution and its reporter, so the two
  // gestures cannot land in two different places.
  describe("Upload here… in the context menu", () => {
    async function tree(
      onFilesDropped: (dir: string, dropped: DroppedItems) => void,
      fileDropEnabled = true,
    ) {
      treeMock.mockImplementation((_sid, d) =>
        Promise.resolve({
          dir: d,
          entries:
            d === "" ? [dir("src"), file("README.md")] : [file("src/a.ts")],
        }),
      )
      render(
        <FileTree
          root={agentRoot("s1")}
          openPath={null}
          changed={new Map()}
          initialPath={null}
          onOpen={() => {}}
          fileDropEnabled={fileDropEnabled}
          onFilesDropped={onFilesDropped}
        />,
      )
      expect(await screen.findByText("README.md")).toBeTruthy()
    }

    // The native dialog cannot open headlessly, so the hidden input is driven
    // the way the browser drives it.
    async function pick(files: File[]) {
      const input = screen.getByTestId("file-picker-input") as HTMLInputElement
      Object.defineProperty(input, "files", { value: files, configurable: true })
      await act(async () => {
        fireEvent.change(input)
        await Promise.resolve()
      })
    }

    const picked = [new File(["x"], "logo.png")]

    async function chooseUploadOn(label: string) {
      fireEvent.contextMenu(screen.getByText(label))
      fireEvent.click(await screen.findByText("Upload here…"))
    }

    it("targets the folder itself from a folder row", async () => {
      const onFilesDropped = vi.fn()
      await tree(onFilesDropped)
      await chooseUploadOn("src")
      await pick(picked)
      // `folders: []` always: a picker cannot produce a directory, so the
      // folder-refusal rung is unreachable from this gesture.
      expect(onFilesDropped).toHaveBeenCalledWith("src", {
        files: picked,
        folders: [],
      })
    })

    it("targets the PARENT folder from a file row", async () => {
      const onFilesDropped = vi.fn()
      await tree(onFilesDropped)
      await chooseUploadOn("README.md")
      await pick(picked)
      expect(onFilesDropped).toHaveBeenCalledWith("", {
        files: picked,
        folders: [],
      })
    })

    it("targets the worktree root from the empty-space menu", async () => {
      const onFilesDropped = vi.fn()
      await tree(onFilesDropped)
      fireEvent.contextMenu(screen.getByTestId("file-tree-drop-surface"))
      fireEvent.click(await screen.findByText("Upload here…"))
      await pick(picked)
      expect(onFilesDropped).toHaveBeenCalledWith("", {
        files: picked,
        folders: [],
      })
    })

    it("reports nothing when the picker is cancelled", async () => {
      const onFilesDropped = vi.fn()
      await tree(onFilesDropped)
      await chooseUploadOn("src")
      await act(async () => {
        screen
          .getByTestId("file-picker-input")
          .dispatchEvent(new Event("cancel"))
        await Promise.resolve()
      })
      expect(onFilesDropped).not.toHaveBeenCalled()
    })

    it("is absent when file drop is switched off on the server", async () => {
      const onFilesDropped = vi.fn()
      await tree(onFilesDropped, false)
      fireEvent.contextMenu(screen.getByText("src"))
      expect(await screen.findByText("New Folder…")).toBeTruthy()
      expect(screen.queryByText("Upload here…")).toBeNull()
    })
  })
})
