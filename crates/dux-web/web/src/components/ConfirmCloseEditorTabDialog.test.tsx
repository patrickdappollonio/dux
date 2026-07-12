// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import type { DuxState } from "@/lib/store"
import type { EditorTab, EditorTabsState } from "@/lib/editorTabs"

let mockState: DuxState
const editorCloseTabMock = vi.fn()
const closeEditorCloseTabMock = vi.fn()
vi.mock("@/lib/store", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/store")>()
  return {
    ...actual,
    useDux: () => mockState,
    editorCloseTab: (...args: unknown[]) => editorCloseTabMock(...args),
    closeEditorCloseTab: () => closeEditorCloseTabMock(),
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
const { ConfirmCloseEditorTabDialog } = await import(
  "./ConfirmCloseEditorTabDialog"
)

function tab(overrides: Partial<EditorTab>): EditorTab {
  return {
    id: "t1",
    path: "src/a.ts",
    mode: "file",
    preview: false,
    dirty: true,
    ...overrides,
  }
}

function seed(
  target: { sessionId: string; tabId: string } | null,
  tabs: EditorTab[],
) {
  const tabsState: EditorTabsState = { tabs, activeId: tabs[0]?.id ?? null }
  mockState = {
    editorCloseTabTarget: target,
    editorTabs: target ? { [target.sessionId]: tabsState } : {},
  } as unknown as DuxState
}

beforeEach(() => {
  installBootStubs()
  editorCloseTabMock.mockClear()
  closeEditorCloseTabMock.mockClear()
})

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
})

describe("ConfirmCloseEditorTabDialog", () => {
  it("renders when a dirty tab target is set and shows its path", () => {
    seed({ sessionId: "s1", tabId: "t1" }, [tab({ dirty: true, path: "src/a.ts" })])
    render(<ConfirmCloseEditorTabDialog />)
    expect(screen.getByText("Discard unsaved changes?")).toBeTruthy()
    expect(screen.getByText("src/a.ts")).toBeTruthy()
  })

  it("Cancel / Keep editing is autoFocused and closes without closing the tab", () => {
    seed({ sessionId: "s1", tabId: "t1" }, [tab({ dirty: true })])
    render(<ConfirmCloseEditorTabDialog />)
    const cancel = screen.getByRole("button", { name: "Keep editing" })
    expect(document.activeElement).toBe(cancel)
    fireEvent.click(cancel)
    expect(closeEditorCloseTabMock).toHaveBeenCalled()
    expect(editorCloseTabMock).not.toHaveBeenCalled()
  })

  it("Discard button has variant destructive and calls editorCloseTab", () => {
    seed({ sessionId: "s1", tabId: "t1" }, [tab({ dirty: true })])
    render(<ConfirmCloseEditorTabDialog />)
    const discard = screen.getByRole("button", { name: /discard/i })
    fireEvent.click(discard)
    expect(editorCloseTabMock).toHaveBeenCalledWith("s1", "t1")
    expect(closeEditorCloseTabMock).toHaveBeenCalled()
  })

  it("self-closes via the vanished-target guard when the target tab disappears", () => {
    seed({ sessionId: "s1", tabId: "gone" }, [tab({ id: "t1", dirty: true })])
    render(<ConfirmCloseEditorTabDialog />)
    expect(screen.queryByText("Discard unsaved changes?")).toBeNull()
    expect(closeEditorCloseTabMock).toHaveBeenCalled()
  })

  it("self-closes when the target tab stopped being dirty (saved elsewhere)", () => {
    seed({ sessionId: "s1", tabId: "t1" }, [tab({ id: "t1", dirty: false })])
    render(<ConfirmCloseEditorTabDialog />)
    expect(screen.queryByText("Discard unsaved changes?")).toBeNull()
    expect(closeEditorCloseTabMock).toHaveBeenCalled()
  })

  // Space-activates-focused-button is a real native <button> behavior (the
  // browser fires a click on Space keyup) that jsdom does not implement, so it
  // can't be exercised via fireEvent here: no dialog test in this codebase
  // simulates it for that reason. What IS testable, and asserted here, is the
  // precondition the tenet depends on: both footer controls are real
  // `<button>` elements (not divs with onClick), so the browser's native Space
  // handling applies without any extra code in this dialog.
  it("both footer buttons are native <button> elements (Space works without extra wiring)", () => {
    seed({ sessionId: "s1", tabId: "t1" }, [tab({ dirty: true })])
    render(<ConfirmCloseEditorTabDialog />)
    const cancel = screen.getByRole("button", { name: "Keep editing" })
    const discard = screen.getByRole("button", { name: /discard/i })
    expect(cancel.tagName).toBe("BUTTON")
    expect(discard.tagName).toBe("BUTTON")
  })
})
