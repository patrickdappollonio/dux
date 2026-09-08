// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

const tips = ["Try the `cog` menu", "Drop a file on an `agent`"]
vi.mock("@/lib/store", () => ({
  useDux: () => ({ bootstrap: { welcome_tips: tips } }),
}))

const { Welcome } = await import("./Welcome")

afterEach(cleanup)

describe("Welcome", () => {
  it("renders the logo and one tip with no action slot", () => {
    const { container } = render(<Welcome />)
    expect(screen.getByLabelText("dux")).toBeTruthy()
    const body = document.body.textContent ?? ""
    expect(tips.some((tip) => body.includes(tip.replace(/`/g, "")))).toBe(true)
    // The idle screen alone is decoration: it keeps its clipping look.
    expect(container.firstElementChild?.className).toContain("overflow-hidden")
    expect(container.firstElementChild?.className).not.toContain("overflow-y-auto")
  })

  it("renders the action slot after the tip", () => {
    render(<Welcome action={<button>Start session</button>} />)
    const action = screen.getByText("Start session")
    const tip = document.querySelector("p")
    expect(tip).toBeTruthy()
    // Node.DOCUMENT_POSITION_FOLLOWING: the action comes after the tip.
    expect(tip!.compareDocumentPosition(action) & 4).toBeTruthy()
  })

  // An action is something the user must REACH, so a viewport too short for the
  // logo, wordmark, tip and button together has to scroll rather than clip.
  it("scrolls rather than clipping once it carries an action", () => {
    const { container } = render(<Welcome action={<button>Start session</button>} />)
    const outer = container.firstElementChild
    expect(outer?.className).toContain("overflow-y-auto")
    expect(outer?.className).toContain("min-h-0")
    expect(outer?.className).not.toContain("overflow-hidden")
    // `my-auto` on the inner stack is what centres content that fits while
    // letting taller content scroll from the top.
    expect(outer?.firstElementChild?.className).toContain("my-auto")
  })

  // The tip is chosen once per mount from a stored fraction. Passing an action
  // must not disturb that: a button under the tip is not a reason to re-roll it.
  it("picks its tip the same way with and without an action", () => {
    const random = vi.spyOn(Math, "random").mockReturnValue(0.9)
    const plain = render(<Welcome />)
    const plainTip = plain.container.querySelector("p")?.textContent
    cleanup()
    const withAction = render(<Welcome action={<button>Start session</button>} />)
    const actionTip = withAction.container.querySelector("p")?.textContent
    expect(actionTip).toBe(plainTip)
    random.mockRestore()
  })
})
