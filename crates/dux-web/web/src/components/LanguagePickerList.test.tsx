// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import { LanguagePickerList } from "@/components/LanguagePickerList"
import { languagePickerEntries } from "@/lib/editorLanguage"

const CHOICES = languagePickerEntries([
  { id: "plaintext", aliases: ["Plain Text", "text"], extensions: [".txt"] },
  { id: "typescript", aliases: ["TypeScript"], extensions: [".ts"] },
  { id: "toml", aliases: ["TOML"], extensions: [".toml"] },
  { id: "yaml", aliases: ["YAML", "yaml"], extensions: [".yaml", ".yml"] },
])

afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  delete (Element.prototype as Partial<Element>).scrollIntoView
})

// A ResizeObserver whose callbacks a test fires by hand, standing in for the
// popup being placed and height-capped, or a soft keyboard shrinking it.
function stubResize(): { resize: () => void } {
  const callbacks: Array<() => void> = []
  vi.stubGlobal(
    "ResizeObserver",
    class {
      constructor(callback: () => void) {
        callbacks.push(callback)
      }
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  )
  return { resize: () => callbacks.forEach((callback) => callback()) }
}

// Every scroll a row asks for, by the row's name.
function spyScrolls(): string[] {
  const scrolled: string[] = []
  Element.prototype.scrollIntoView = function (this: Element) {
    scrolled.push(this.textContent ?? "")
  }
  return scrolled
}

function mount(current?: string) {
  const onPick = vi.fn()
  render(
    <LanguagePickerList
      choices={CHOICES}
      current={current}
      onPick={onPick}
    />,
  )
  const input = screen.getByRole("combobox", { name: "Search languages" })
  return {
    onPick,
    type: (value: string) => fireEvent.change(input, { target: { value } }),
    key: (key: string, init: Record<string, unknown> = {}) =>
      fireEvent.keyDown(input, { key, ...init }),
  }
}

// The highlighted row's name, after checking that the field points a screen
// reader at that same row, and at nothing when no row is highlighted.
function highlighted(): string | null {
  const rows = screen
    .getAllByRole("option")
    .filter((o) => o.getAttribute("aria-selected") === "true")
  expect(rows.length).toBeLessThanOrEqual(1)
  const field = screen.getByRole("combobox", { name: "Search languages" })
  expect(field.getAttribute("aria-activedescendant")).toBe(rows[0]?.id ?? null)
  return rows[0]?.textContent ?? null
}

describe("LanguagePickerList", () => {
  it("keeps the best match highlighted through every keystroke", () => {
    const { type, key, onPick } = mount()
    for (const value of ["y", "ya", "yam", "yaml"]) {
      type(value)
      expect(highlighted()).toBe("YAML")
    }
    key("Enter")
    expect(onPick).toHaveBeenCalledWith("yaml")
  })

  it("moves off a language already set to the best match for the search", () => {
    const { type, key, onPick } = mount("toml")
    expect(highlighted()).toBe("TOML")
    type("yml")
    expect(highlighted()).toBe("YAML")
    key("Enter")
    expect(onPick).toHaveBeenCalledWith("yaml")
  })

  it("lets the arrow keys reach the pinned Auto row and back", () => {
    const { type, key, onPick } = mount()
    type("yml")
    key("ArrowUp")
    expect(highlighted()).toBe("Auto")
    key("ArrowDown")
    expect(highlighted()).toBe("YAML")
    key("ArrowUp")
    key("Enter")
    expect(onPick).toHaveBeenCalledWith(null)
  })

  it("highlights the best match on every search change, never an earlier arrow move", () => {
    const { type, key, onPick } = mount()
    type("y")
    key("ArrowUp")
    expect(highlighted()).toBe("Auto")
    type("ya")
    expect(highlighted()).toBe("YAML")
    type("y")
    expect(highlighted()).toBe("YAML")
    key("Enter")
    expect(onPick).toHaveBeenCalledWith("yaml")
  })

  it("goes back to the current choice when the search is cleared", () => {
    const { type, key } = mount("toml")
    type("yml")
    key("ArrowUp")
    expect(highlighted()).toBe("Auto")
    type("")
    expect(highlighted()).toBe("TOML")
  })

  it("highlights nothing when nothing matches, and Enter then does nothing", () => {
    const { type, key, onPick } = mount("toml")
    type("cobol")
    expect(highlighted()).toBeNull()
    key("Enter")
    expect(onPick).not.toHaveBeenCalled()
    // Auto is still there for the arrow keys.
    key("ArrowDown")
    expect(highlighted()).toBe("Auto")
  })

  it("highlights Auto for an override the registry does not know, so Enter still picks", () => {
    const { key, onPick } = mount("cobol")
    expect(highlighted()).toBe("Auto")
    key("Enter")
    expect(onPick).toHaveBeenCalledWith(null)
  })

  it("leaves keys the IME or a modifier holds alone, and moves on the menus' Ctrl chords", () => {
    const { type, key, onPick } = mount()
    type("t")
    expect(highlighted()).toBe("Plain text")
    key("Enter", { keyCode: 229 })
    key("Enter", { shiftKey: true })
    key("ArrowDown", { altKey: true })
    expect(onPick).not.toHaveBeenCalled()
    expect(highlighted()).toBe("Plain text")
    key("n", { ctrlKey: true })
    expect(highlighted()).toBe("TOML")
    key("p", { ctrlKey: true })
    expect(highlighted()).toBe("Plain text")
  })

  it("keeps focus in the field when the list's blank space is pressed", () => {
    const { type } = mount()
    type("cobol")
    // fireEvent returns false when the press's default (a focus change) was
    // prevented.
    expect(fireEvent.pointerDown(screen.getByRole("listbox"))).toBe(false)
    expect(fireEvent.mouseDown(screen.getByText("No language matches that search."))).toBe(false)
  })

  it("marks the current choice checked and says out loud when nothing matches", () => {
    const { type } = mount("toml")
    expect(screen.getByRole("option", { name: "TOML" }).getAttribute("aria-checked")).toBe("true")
    expect(screen.getByRole("option", { name: "Auto" }).getAttribute("aria-checked")).toBe("false")
    expect(screen.getByRole("status").textContent).toBe("")
    type("cobol")
    expect(screen.getByRole("status").textContent).toBe("No language matches that search.")
  })

  it("brings the highlighted row back into view once the list is laid out", () => {
    const { resize } = stubResize()
    mount("toml")
    const scrolled = spyScrolls()
    resize()
    expect(scrolled).toEqual(["TOML"])
  })

  it("stops bringing the row back once the user has scrolled the list", () => {
    const { resize } = stubResize()
    mount("toml")
    const scrolled = spyScrolls()
    fireEvent.wheel(screen.getByRole("listbox"))
    resize()
    expect(scrolled).toEqual([])
  })

  it("does not scroll to a row the pointer highlights", () => {
    const { resize } = stubResize()
    mount()
    const scrolled = spyScrolls()
    fireEvent.pointerMove(screen.getByRole("option", { name: "YAML" }))
    expect(highlighted()).toBe("YAML")
    resize()
    expect(scrolled).toEqual([])
  })

  it("still scrolls to a row the arrow keys reach", () => {
    mount()
    const scrolled = spyScrolls()
    fireEvent.keyDown(screen.getByRole("combobox", { name: "Search languages" }), {
      key: "ArrowDown",
    })
    expect(scrolled).toEqual(["Plain text"])
  })
})
