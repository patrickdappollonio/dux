// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen } from "@testing-library/react"

vi.mock("@/lib/store", () => ({
  // Nothing drives the agent here, so its Start is a live button.
  useDux: () => ({ spine: null }),
  useDuxSelector: () => null,
}))

const { NotRunningCard } = await import("./NotRunningCard")

afterEach(cleanup)

describe("NotRunningCard", () => {
  it("ends its details with the resume docs link and starts on a press", () => {
    const onStart = vi.fn()
    render(
      <NotRunningCard
        icon="small"
        title="claude isn’t running."
        details="Start picks up its last conversation."
        start={{ sessionId: "s1", onStart, label: "Start" }}
      />,
    )
    const link = screen.getByRole("link", { name: /how resume works/i })
    expect(link.closest("p")?.textContent).toMatch(
      /^Start picks up its last conversation\. How resume works/,
    )
    fireEvent.click(screen.getByRole("button", { name: "Start" }))
    expect(onStart).toHaveBeenCalledOnce()
  })

  // Output a run never left is no section at all, not an empty box under a label.
  it("draws the output section only when there are lines to show", () => {
    const props = {
      icon: "none",
      title: "This claude tab isn’t running.",
      details: "Starting it picks up this provider’s most recent conversation.",
      start: { sessionId: "s1", onStart: () => {}, label: "Start session" },
    } as const
    const { rerender, container } = render(<NotRunningCard {...props} output={[]} />)
    expect(screen.queryByText("Last output")).toBeNull()
    expect(container.querySelector("img")).toBeNull()
    rerender(<NotRunningCard {...props} output={["boom", "bang"]} />)
    expect(screen.getByText("Last output")).toBeTruthy()
    expect(container.querySelector("pre")?.textContent).toBe("boom\nbang")
  })
})
