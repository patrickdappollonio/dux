// @vitest-environment jsdom
import type { ReactNode } from "react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react"

import { RootErrorBoundary } from "@/components/RootErrorBoundary"

function Thrower(): never {
  throw new Error("sidebar row has no agent")
}

// A parent that has already returned when Thrower throws, so its name reaches
// only the component stack, never the error's own JavaScript stack.
function AgentSidebar({ children }: { children: ReactNode }) {
  return children
}

describe("RootErrorBoundary", () => {
  let reload: ReturnType<typeof vi.fn>
  let writeText: ReturnType<typeof vi.fn>

  beforeEach(() => {
    // React reports a caught render error through console.error as well; keep
    // the run quiet and let the tests read what was logged.
    vi.spyOn(console, "error").mockImplementation(() => {})
    reload = vi.fn()
    vi.stubGlobal("location", { ...window.location, reload })
    writeText = vi.fn().mockResolvedValue(undefined)
    Object.defineProperty(navigator, "clipboard", {
      configurable: true,
      value: { writeText },
    })
  })

  afterEach(() => {
    cleanup()
    vi.restoreAllMocks()
    vi.unstubAllGlobals()
  })

  it("renders its children when nothing throws", () => {
    render(
      <RootErrorBoundary>
        <p>workspace</p>
      </RootErrorBoundary>,
    )
    expect(screen.getByText("workspace")).toBeTruthy()
    expect(screen.queryByRole("button", { name: "Reload page" })).toBeNull()
  })

  it("replaces a tree that threw while rendering with a screen naming the error", () => {
    render(
      <RootErrorBoundary>
        <Thrower />
      </RootErrorBoundary>,
    )
    expect(screen.getByText(/stopped drawing/)).toBeTruthy()
    expect(screen.getByText(/keep running on the server/)).toBeTruthy()
    expect(screen.getByText("sidebar row has no agent")).toBeTruthy()
    expect(screen.getByRole("button", { name: "Reload page" })).toBeTruthy()
    expect(screen.getByRole("button", { name: "Copy details" })).toBeTruthy()
    expect(reload).not.toHaveBeenCalled()
  })

  it("logs the caught error with its component stack to the console", () => {
    render(
      <RootErrorBoundary>
        <AgentSidebar>
          <Thrower />
        </AgentSidebar>
      </RootErrorBoundary>,
    )
    // React logs a caught error on its own, without the component stack, so
    // the stack is what tells the boundary's own line apart.
    const logged = vi
      .mocked(console.error)
      .mock.calls.some(
        (args) =>
          args.some((a) => a instanceof Error && a.message === "sidebar row has no agent") &&
          args.some((a) => typeof a === "string" && a.includes("AgentSidebar")),
      )
    expect(logged).toBe(true)
  })

  it("reloads the page only when Reload page is pressed", () => {
    render(
      <RootErrorBoundary>
        <Thrower />
      </RootErrorBoundary>,
    )
    fireEvent.click(screen.getByRole("button", { name: "Reload page" }))
    expect(reload).toHaveBeenCalledTimes(1)
  })

  it("copies the message and the component stack", async () => {
    render(
      <RootErrorBoundary>
        <AgentSidebar>
          <Thrower />
        </AgentSidebar>
      </RootErrorBoundary>,
    )
    fireEvent.click(screen.getByRole("button", { name: "Copy details" }))
    await waitFor(() => expect(writeText).toHaveBeenCalledTimes(1))
    const copied = writeText.mock.calls[0][0] as string
    expect(copied).toContain("sidebar row has no agent")
    expect(copied).toContain("AgentSidebar")
  })
})
