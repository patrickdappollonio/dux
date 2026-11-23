// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it } from "vitest"
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react"

import { quotedChip } from "@/lib/prose"

import { ExplainedWhenDisabled } from "./ExplainedWhenDisabled"

// An exit animation that never ends, so a closing tooltip stays on screen the
// way it does for the length of its fade in a browser.
beforeEach(() => {
  Element.prototype.getAnimations = () =>
    [{ finished: new Promise(() => {}) }] as unknown as Animation[]
})
afterEach(() => {
  cleanup()
  delete (Element.prototype as Partial<Element>).getAnimations
})

function Control({ disabled }: { disabled: boolean }) {
  return (
    <ExplainedWhenDisabled disabled={disabled} hint={disabled ? "Take over" : "New tab"}>
      <button type="button" disabled={disabled} aria-label="Add" />
    </ExplainedWhenDisabled>
  )
}

describe("ExplainedWhenDisabled", () => {
  it("keeps the text it opened with while it closes", async () => {
    const { rerender } = render(<Control disabled />)
    fireEvent.click(screen.getByLabelText("Add").parentElement!)
    expect((await screen.findAllByText("Take over")).length).toBeGreaterThan(0)
    rerender(<Control disabled={false} />)
    expect(screen.queryByText("New tab")).toBeNull()
    expect(screen.getAllByText("Take over").length).toBeGreaterThan(0)
  })

  it("keeps the text it opened with on focus while it closes", async () => {
    const { rerender } = render(<Control disabled />)
    act(() => screen.getByLabelText("Add").parentElement!.focus())
    expect((await screen.findAllByText("Take over")).length).toBeGreaterThan(0)
    rerender(<Control disabled={false} />)
    expect(screen.queryByText("New tab")).toBeNull()
  })

  it("draws a name in its hint as the chip, without the plain text's quotes", async () => {
    render(
      <ExplainedWhenDisabled disabled hint={["Macro ", quotedChip("Build"), " needs some text."]}>
        <button type="button" disabled aria-label="Save" />
      </ExplainedWhenDisabled>,
    )
    fireEvent.click(screen.getByLabelText("Save").parentElement!)
    const chips = await screen.findAllByText("Build")
    expect(chips[0].getAttribute("data-slot")).toBe("inline-code")
    expect(chips[0].parentElement?.textContent).toBe("Macro Build needs some text.")
  })
})
