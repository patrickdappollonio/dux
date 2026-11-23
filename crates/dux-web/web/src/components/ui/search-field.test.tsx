// @vitest-environment jsdom
import { useState } from "react"
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import { SearchField, type SearchFieldProps } from "./search-field"

afterEach(cleanup)

function Harness(props: Partial<SearchFieldProps>) {
  const [value, setValue] = useState("abc")
  return <SearchField label="Search things" {...props} value={value} onChange={setValue} />
}

describe("SearchField", () => {
  it("is named by its label, which is also what the empty field says", () => {
    render(<Harness />)
    const field = screen.getByLabelText("Search things") as HTMLInputElement
    expect(field.placeholder).toBe("Search things")
    expect(screen.getByLabelText("Search things")).toBe(field)
  })

  it("is plain text, never type=search, so no browser clears or decorates it by itself", () => {
    render(<Harness />)
    expect((screen.getByLabelText("Search things") as HTMLInputElement).type).toBe("text")
  })

  it("is announced as a search field and asks a phone keyboard for its Search key", () => {
    render(<Harness />)
    expect(screen.getByRole("searchbox", { name: "Search things" }).getAttribute("enterkeyhint")).toBe("search")
  })

  it("lets a caller's own role replace the search role and keeps the Search key", () => {
    render(<Harness role="combobox" />)
    const field = screen.getByRole("combobox", { name: "Search things" })
    expect(field.getAttribute("enterkeyhint")).toBe("search")
  })

  it("lifts to 40px at phone width and under a coarse pointer", () => {
    render(<Harness />)
    const box = screen.getByLabelText("Search things").parentElement!
    expect(Array.from(box.classList)).toEqual(
      expect.arrayContaining(["min-h-8", "max-md:min-h-10", "pointer-coarse:min-h-10"]),
    )
  })

  it("leaves the typed text alone on Escape unless it was told to clear", () => {
    render(<Harness />)
    fireEvent.keyDown(screen.getByLabelText("Search things"), { key: "Escape" })
    expect((screen.getByLabelText("Search things") as HTMLInputElement).value).toBe("abc")
  })

  it("clears on Escape when told to, and lets the key go on", () => {
    const outer = vi.fn()
    render(
      <div onKeyDown={outer}>
        <Harness escape="clears" />
      </div>,
    )
    fireEvent.keyDown(screen.getByLabelText("Search things"), { key: "Escape" })
    expect((screen.getByLabelText("Search things") as HTMLInputElement).value).toBe("")
    expect(outer).toHaveBeenCalledTimes(1)
  })

  it("runs the caller's key handler first and respects a key it handled", () => {
    const onKeyDown = vi.fn((event: React.KeyboardEvent) => event.preventDefault())
    render(<Harness escape="clears" onKeyDown={onKeyDown} />)
    fireEvent.keyDown(screen.getByLabelText("Search things"), { key: "Escape" })
    expect(onKeyDown).toHaveBeenCalled()
    expect((screen.getByLabelText("Search things") as HTMLInputElement).value).toBe("abc")
  })
})
