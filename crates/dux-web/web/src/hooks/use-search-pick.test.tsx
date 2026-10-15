// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

import { useSearchPick } from "./use-search-pick"

function Picker({
  query,
  items,
  onPick,
}: {
  query: string
  items: string[]
  onPick: (item: string) => void
}) {
  const pick = useSearchPick({ query, items, keyOf: (item) => item, onPick })
  return <input aria-label="search" readOnly value={query} {...pick.fieldProps} />
}

afterEach(cleanup)

describe("useSearchPick", () => {
  it("keeps the row the user moved to when the list reorders under the same query", () => {
    const onPick = vi.fn()
    const view = render(
      <Picker query="fix" items={["fix-a", "fix-b", "fix-c"]} onPick={onPick} />,
    )
    fireEvent.keyDown(screen.getByLabelText("search"), { key: "ArrowDown" })
    view.rerender(
      <Picker query="fix" items={["fix-c", "fix-a", "fix-b"]} onPick={onPick} />,
    )
    fireEvent.keyDown(screen.getByLabelText("search"), { key: "Enter" })
    expect(onPick).toHaveBeenCalledWith("fix-b")
  })

  it("starts over at the first match when a query comes back", () => {
    const onPick = vi.fn()
    const items = ["front-door", "frontier"]
    const view = render(<Picker query="front" items={items} onPick={onPick} />)
    fireEvent.keyDown(screen.getByLabelText("search"), { key: "ArrowDown" })
    view.rerender(<Picker query="front-" items={["front-door"]} onPick={onPick} />)
    view.rerender(<Picker query="front" items={items} onPick={onPick} />)
    fireEvent.keyDown(screen.getByLabelText("search"), { key: "Enter" })
    expect(onPick).toHaveBeenCalledWith("front-door")
  })
})
