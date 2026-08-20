// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { EditorTab } from "@/lib/editorTabs"
import { agentRoot, rootKey } from "@/lib/editorRoot"

let mockState: DuxState
const editorActivateTabMock = vi.fn()
const editorPinTabMock = vi.fn()
const editorCloseTabMock = vi.fn()
const openEditorCloseTabMock = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    editorActivateTab: (...args: unknown[]) => editorActivateTabMock(...args),
    editorPinTab: (...args: unknown[]) => editorPinTabMock(...args),
    editorCloseTab: (...args: unknown[]) => editorCloseTabMock(...args),
    openEditorCloseTab: (...args: unknown[]) => openEditorCloseTabMock(...args),
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
const { EditorTabsStrip } = await import("./EditorTabsStrip")

function tab(overrides: Partial<EditorTab>): EditorTab {
  return {
    id: "t1",
    path: "src/a.ts",
    mode: "file",
    preview: false,
    dirty: false,
    ...overrides,
  }
}

function seed(sessionId: string, tabs: EditorTab[], activeId: string | null) {
  mockState = {
    editorTabs: { [rootKey(agentRoot(sessionId))]: { tabs, activeId } },
  } as unknown as DuxState
}

beforeEach(() => {
  installBootStubs()
  editorActivateTabMock.mockClear()
  editorPinTabMock.mockClear()
  editorCloseTabMock.mockClear()
  openEditorCloseTabMock.mockClear()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("EditorTabsStrip", () => {
  it("renders one pill per tab with the basename label", () => {
    seed(
      "s1",
      [tab({ id: "t1", path: "src/a.ts" }), tab({ id: "t2", path: "b.ts" })],
      "t1",
    )
    render(<EditorTabsStrip root={agentRoot("s1")} />)
    expect(screen.getByText("a.ts")).toBeTruthy()
    expect(screen.getByText("b.ts")).toBeTruthy()
  })

  it("preview tabs render italic; permanent tabs do not", () => {
    seed(
      "s1",
      [
        tab({ id: "t1", path: "a.ts", preview: true }),
        tab({ id: "t2", path: "b.ts", preview: false }),
      ],
      "t1",
    )
    render(<EditorTabsStrip root={agentRoot("s1")} />)
    expect(screen.getByText("a.ts").className).toContain("italic")
    expect(screen.getByText("b.ts").className).not.toContain("italic")
  })

  it("the truncating label span keeps a right padding so italics don't clip", () => {
    // The clip happens inside the span's own overflow-hidden box: an italic
    // final ascender leans past the content edge and `truncate` cuts it. The
    // padding is unconditional (not preview-only) so a tab flipping between
    // preview and permanent never shifts its label by a padding's width.
    seed(
      "s1",
      [
        tab({ id: "t1", path: "a.ts", preview: true }),
        tab({ id: "t2", path: "b.ts", preview: false }),
      ],
      "t1",
    )
    render(<EditorTabsStrip root={agentRoot("s1")} />)
    expect(screen.getByText("a.ts").className).toContain("pr-0.5")
    expect(screen.getByText("b.ts").className).toContain("pr-0.5")
  })

  it("clicking a pill activates that tab", () => {
    seed("s1", [tab({ id: "t1" }), tab({ id: "t2", path: "b.ts" })], "t1")
    render(<EditorTabsStrip root={agentRoot("s1")} />)
    fireEvent.click(screen.getByText("b.ts"))
    expect(editorActivateTabMock).toHaveBeenCalledWith(agentRoot("s1"), "t2")
  })

  it("double-clicking a pill pins it (clears preview)", () => {
    seed("s1", [tab({ id: "t1", preview: true })], "t1")
    render(<EditorTabsStrip root={agentRoot("s1")} />)
    fireEvent.doubleClick(screen.getByText("a.ts"))
    expect(editorPinTabMock).toHaveBeenCalledWith(agentRoot("s1"), "t1")
  })

  it("a dirty tab shows the dirty dot", () => {
    seed("s1", [tab({ id: "t1", dirty: true })], "t1")
    render(<EditorTabsStrip root={agentRoot("s1")} />)
    expect(screen.getByText(/unsaved changes/i)).toBeTruthy()
  })

  it("closing a clean tab calls editorCloseTab directly (no dialog target set)", () => {
    seed("s1", [tab({ id: "t1", dirty: false })], "t1")
    render(<EditorTabsStrip root={agentRoot("s1")} />)
    fireEvent.click(screen.getByLabelText("Close a.ts"))
    expect(editorCloseTabMock).toHaveBeenCalledWith(agentRoot("s1"), "t1")
    expect(openEditorCloseTabMock).not.toHaveBeenCalled()
  })

  it("closing a dirty tab sets editorCloseTabTarget (opens the confirm dialog)", () => {
    seed("s1", [tab({ id: "t1", dirty: true })], "t1")
    render(<EditorTabsStrip root={agentRoot("s1")} />)
    fireEvent.click(screen.getByLabelText("Close a.ts"))
    expect(openEditorCloseTabMock).toHaveBeenCalledWith(agentRoot("s1"), "t1")
    expect(editorCloseTabMock).not.toHaveBeenCalled()
  })

  it("middle-click on a pill triggers the same close path", () => {
    seed("s1", [tab({ id: "t1", dirty: false })], "t1")
    render(<EditorTabsStrip root={agentRoot("s1")} />)
    const pillEl = screen.getByText("a.ts").closest('[role="tab"]')!
    fireEvent(
      pillEl,
      new MouseEvent("auxclick", { button: 1, bubbles: true }),
    )
    expect(editorCloseTabMock).toHaveBeenCalledWith(agentRoot("s1"), "t1")
  })

  it("strip container is horizontally scrollable", () => {
    seed("s1", [tab({ id: "t1" })], "t1")
    const { container } = render(<EditorTabsStrip root={agentRoot("s1")} />)
    const strip = container.firstElementChild as HTMLElement
    expect(strip.className).toContain("overflow-x-auto")
  })

  it("phone pills match the mode toggle's height, a deliberate touch-floor deviation", () => {
    // The pill's visible phone height is pinned to the File/Diff mode
    // toggle's rendered height (h-7 button + p-0.5 + border = 34px =
    // min-h-8.5), NOT the 40px touch floor — a settled product decision for
    // this surface. The close button keeps a larger-than-visual hit area
    // (max-md:size-8 fits inside the 34px pill without adding height).
    seed("s1", [tab({ id: "t1" })], "t1")
    render(<EditorTabsStrip root={agentRoot("s1")} />)
    const pill = screen.getByText("a.ts").closest('[role="tab"]') as HTMLElement
    expect(pill.className).toContain("max-md:min-h-8.5")
    // py-0 on the phone: the 32px close-button hit area already fills the
    // 34px pill; keeping py-1 would push the rendered height to 42px.
    expect(pill.className).toContain("max-md:py-0")
    expect(pill.className).not.toContain("max-md:min-h-10")
    const closeBtn = screen.getByLabelText("Close a.ts")
    expect(closeBtn.className).toContain("max-md:size-8")
    expect(closeBtn.className).not.toContain("max-md:size-10")
  })

  it("renders nothing when the session has no tabs", () => {
    seed("s1", [], null)
    const { container } = render(<EditorTabsStrip root={agentRoot("s1")} />)
    expect(container.firstChild).toBeNull()
  })
})
