// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

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
  encoding: null,
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
    // No encoding item: a plain UTF-8 file says nothing about it.
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

  it("never paints the previous editor's numbers when a new one arrives", () => {
    const first = fakeEditor()
    first.state.line = 9
    first.state.column = 3
    const { rerender } = renderBar(FULL, first.editor)
    expect(screen.getByText("Ln 9, Col 3")).toBeTruthy()
    const bar = (editor: StatusBarEditor | null) => (
      <EditorStatusBar
        plan={FULL}
        editor={editor}
        languageLabel="Rust"
        languageItems={null}
      />
    )
    rerender(bar(null))
    // Every text the bar paints on the way to the new editor, recorded as it
    // changes: the old file's position must not be one of them.
    const seen: string[] = []
    const observer = new MutationObserver((records) => {
      for (const r of records) {
        if (r.oldValue !== null) seen.push(r.oldValue)
      }
    })
    observer.observe(document.body, {
      subtree: true,
      characterData: true,
      characterDataOldValue: true,
      childList: true,
    })
    const second = fakeEditor()
    second.state.line = 2
    second.state.column = 8
    rerender(bar(second.editor))
    seen.push(...observer.takeRecords().flatMap((r) => (r.oldValue === null ? [] : [r.oldValue])))
    observer.disconnect()
    expect(screen.getByText("Ln 2, Col 8")).toBeTruthy()
    expect(seen).not.toContain("Ln 9, Col 3")
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
        encoding: null,
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

  it("shows the encoding as a menu on every screen, phones included", async () => {
    render(
      <EditorStatusBar
        plan={{ ...FULL, encoding: "Windows-1252" }}
        editor={fakeEditor().editor}
        languageLabel="Rust"
        languageItems={null}
        encodingItems={<div role="menuitem">Reopen with encoding</div>}
      />,
    )
    const trigger = screen.getByRole("button", {
      name: "File encoding: Windows-1252",
    })
    expect(trigger.textContent).toContain("Windows-1252")
    expect(trigger.closest(".max-md\\:hidden")).toBeNull()
    // The bar's one control on a touch screen: it and the bar grow to the
    // 40px floor under a coarse pointer, at any width.
    expect(trigger.className).toContain("pointer-coarse:h-10")
    fireEvent.click(trigger)
    expect(await screen.findByText("Reopen with encoding")).toBeTruthy()
  })

  it("keeps only the path, the dirty dot, read-only and the encoding on a phone", () => {
    const fake = fakeEditor()
    const { container } = renderBar(
      { ...FULL, dirty: true, readOnly: true, encoding: "UTF-8" },
      fake.editor,
    )
    const phoneHidden = (text: string | RegExp) =>
      screen.getByText(text).closest(".max-md\\:hidden") !== null
    expect(phoneHidden("src/main.rs")).toBe(false)
    expect(phoneHidden("●")).toBe(false)
    expect(phoneHidden("Ln 1, Col 1")).toBe(true)
    expect(
      screen
        .getByRole("button", { name: "File encoding: UTF-8" })
        .closest(".max-md\\:hidden"),
    ).toBeNull()
    const bar = container.firstElementChild as HTMLElement
    expect(bar.className).toMatch(/(^| )pointer-coarse:h-10( |$)/)
    // A phone can type into a read-only file too, so it must be told why
    // Save stays disabled.
    expect(phoneHidden("read-only")).toBe(false)
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
