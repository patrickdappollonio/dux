// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest"
import { act, cleanup, render, screen } from "@testing-library/react"

import { EditorStatusBar, type StatusBarEditor } from "./EditorStatusBar"
import type { StatusBarPlan } from "@/lib/editorStatus"

if (!Element.prototype.getAnimations) {
  Element.prototype.getAnimations = () => []
}

// A Monaco stand-in that fires the four events the bar listens to.
function fakeEditor() {
  const listeners = new Set<() => void>()
  const subscribe = (cb: () => void) => {
    listeners.add(cb)
    return { dispose: () => listeners.delete(cb) }
  }
  const state = {
    line: 1,
    column: 1,
    selected: 0,
    lines: 3,
    eol: "\n",
    insertSpaces: true,
    tabSize: 2,
  }
  const editor: StatusBarEditor = {
    getModel: () => ({
      getLineCount: () => state.lines,
      getEOL: () => state.eol,
      getOptions: () => ({
        insertSpaces: state.insertSpaces,
        tabSize: state.tabSize,
        indentSize: state.tabSize,
      }),
      getValueLengthInRange: () => state.selected,
    }),
    getPosition: () => ({ lineNumber: state.line, column: state.column }),
    getSelection: () => ({ isEmpty: () => state.selected === 0 }),
    onDidChangeCursorSelection: subscribe,
    onDidChangeModelContent: subscribe,
    onDidChangeModel: subscribe,
    onDidChangeModelOptions: subscribe,
  }
  return {
    editor,
    state,
    fire: () => act(() => listeners.forEach((cb) => cb())),
    listenerCount: () => listeners.size,
  }
}

const FULL: StatusBarPlan = {
  path: "src/main.rs",
  dirty: false,
  readOnly: false,
  size: 1536,
  dimensions: null,
  editing: true,
  language: true,
}

function renderBar(plan: StatusBarPlan, editor: StatusBarEditor | null) {
  return render(
    <EditorStatusBar
      plan={plan}
      editor={editor}
      languageLabel="Rust"
      languageItems={null}
    />,
  )
}

describe("the editor status bar", () => {
  afterEach(cleanup)

  it("shows the path, the live cursor, the line count, indentation, line endings, size and language", () => {
    const fake = fakeEditor()
    renderBar(FULL, fake.editor)
    expect(screen.getByText("src/main.rs")).toBeTruthy()
    expect(screen.getByText("Ln 1, Col 1")).toBeTruthy()
    expect(screen.getByText("3 lines")).toBeTruthy()
    expect(screen.getByText("Spaces: 2")).toBeTruthy()
    expect(screen.getByText("LF")).toBeTruthy()
    expect(screen.getByText("1.5 KiB")).toBeTruthy()
    expect(
      screen.getByRole("button", { name: "Syntax language: Rust" }),
    ).toBeTruthy()
    // No encoding item: every file the editor opens is UTF-8.
    expect(screen.queryByText(/utf-?8/i)).toBeNull()
  })

  it("follows the cursor, the content and the model options as they change", () => {
    const fake = fakeEditor()
    renderBar(FULL, fake.editor)
    fake.state.line = 7
    fake.state.column = 12
    fake.state.selected = 5
    fake.state.lines = 40
    fake.state.eol = "\r\n"
    fake.state.insertSpaces = false
    fake.state.tabSize = 4
    fake.fire()
    expect(screen.getByText("Ln 7, Col 12 (5 selected)")).toBeTruthy()
    expect(screen.getByText("40 lines")).toBeTruthy()
    expect(screen.getByText("Tabs: 4")).toBeTruthy()
    expect(screen.getByText("CRLF")).toBeTruthy()
  })

  it("lets go of the editor's events when it unmounts", () => {
    const fake = fakeEditor()
    const { unmount } = renderBar(FULL, fake.editor)
    expect(fake.listenerCount()).toBeGreaterThan(0)
    unmount()
    expect(fake.listenerCount()).toBe(0)
  })

  it("keeps the dirty dot's tooltip text for a screen reader, and says read-only", () => {
    renderBar({ ...FULL, dirty: true, readOnly: true }, fakeEditor().editor)
    expect(screen.getByText("unsaved changes").className).toContain("sr-only")
    expect(screen.getByText("read-only")).toBeTruthy()
  })

  it("carries nothing with no file open, at the same height", () => {
    const { container } = renderBar(
      {
        path: null,
        dirty: false,
        readOnly: false,
        size: null,
        dimensions: null,
        editing: false,
        language: false,
      },
      null,
    )
    const bar = container.firstElementChild as HTMLElement
    expect(bar.textContent).toBe("")
    expect(bar.className).toContain("h-6.5")
  })

  it("carries only the path and the dimensions for an image", () => {
    const { container } = renderBar(
      {
        ...FULL,
        path: "logo.png",
        size: null,
        dimensions: { width: 640, height: 480 },
        editing: false,
        language: false,
      },
      null,
    )
    expect(screen.getByText("640 × 480")).toBeTruthy()
    expect(container.textContent).toBe("logo.png640 × 480")
  })

  it("leaves out the editor's items when Monaco is not what is on screen", () => {
    const fake = fakeEditor()
    renderBar({ ...FULL, editing: false, language: false }, fake.editor)
    expect(screen.queryByText(/^Ln /)).toBeNull()
    expect(screen.queryByText(/lines$/)).toBeNull()
    expect(screen.queryByRole("button", { name: /syntax language/i })).toBeNull()
    expect(screen.getByText("1.5 KiB")).toBeTruthy()
  })

  it("keeps only the path, the dirty dot and the cursor on a phone", () => {
    const fake = fakeEditor()
    renderBar({ ...FULL, dirty: true, readOnly: true }, fake.editor)
    const phoneHidden = (text: string | RegExp) =>
      screen.getByText(text).closest(".max-md\\:hidden") !== null
    expect(phoneHidden("src/main.rs")).toBe(false)
    expect(phoneHidden("●")).toBe(false)
    expect(phoneHidden("Ln 1, Col 1")).toBe(false)
    expect(phoneHidden("read-only")).toBe(true)
    expect(phoneHidden("3 lines")).toBe(true)
    expect(phoneHidden("Spaces: 2")).toBe(true)
    expect(phoneHidden("LF")).toBe(true)
    expect(phoneHidden("1.5 KiB")).toBe(true)
    expect(
      screen
        .getByRole("button", { name: /syntax language/i })
        .closest(".max-md\\:hidden"),
    ).not.toBeNull()
  })
})
