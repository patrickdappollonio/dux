// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

const startDormantTab = vi.fn()
vi.mock("@/lib/store", () => ({
  startDormantTab,
  // `Welcome` reads the tips off the bootstrap document.
  useDux: () => ({ bootstrap: { welcome_tips: ["Try the `cog` menu"] } }),
}))

const { DormantTabSurface } = await import("./DormantTabSurface")

afterEach(() => {
  cleanup()
  startDormantTab.mockClear()
})

describe("DormantTabSurface", () => {
  // Parity with the terminal UI, which has always shown its idle screen for a
  // tab that is merely not running. A card headed "this tab isn't running" told
  // the user what the empty pane already said.
  it("shows the idle screen with a start action for a tab with no verdict", () => {
    render(
      <DormantTabSurface sessionId="s1" tabId="b2" provider="claude" />,
    )
    expect(screen.getByLabelText("dux")).toBeTruthy()
    expect(document.body.textContent ?? "").toMatch(/Try the cog menu/)
    expect(document.body.textContent ?? "").toMatch(/most recent conversation/)
    // Not the diagnosis card.
    expect(document.body.textContent ?? "").not.toMatch(/last run/i)
  })

  it("starts the tab from the idle screen's button", () => {
    render(<DormantTabSurface sessionId="s1" tabId="b2" provider="claude" />)
    fireEvent.click(screen.getByText("Start session"))
    expect(startDormantTab).toHaveBeenCalledWith("s1", "b2")
  })

  // A tab whose last run ended badly is not at rest: it gets the card, with the
  // ending and the output, because that is the whole reason it is waiting.
  it("shows the diagnosis card for a tab whose last run ended badly", () => {
    render(
      <DormantTabSurface
        sessionId="s1"
        tabId="b2"
        provider="codex"
        lastRunFailed
        lastRunVerdict={{
          ending: "exited",
          status: 1,
          ended_seconds_ago: 0,
          excerpt: ["already has an active writer"],
        }}
      />,
    )
    const body = document.body.textContent ?? ""
    expect(body).toMatch(/last run exited with status 1/i)
    expect(body).toMatch(/Last output/)
    expect(body).toMatch(/already has an active writer/)
    // The idle screen's wordmark is NOT painted behind the card.
    expect(screen.queryByLabelText("dux")).toBeNull()
  })

  // A healthy tab on the live wire carries an explicit null rather than an
  // absent key, and that must route to the idle screen like any other healthy
  // tab rather than tripping a truthiness check somewhere.
  it("treats an explicit null verdict as no verdict", () => {
    render(
      <DormantTabSurface
        sessionId="s1"
        tabId="b2"
        provider="claude"
        lastRunVerdict={null}
      />,
    )
    expect(screen.getByLabelText("dux")).toBeTruthy()
  })
})
