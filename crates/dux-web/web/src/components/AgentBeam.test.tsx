// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { act, cleanup, render } from "@testing-library/react"

import { AgentBeam } from "./AgentBeam"

afterEach(() => {
  cleanup()
  vi.useRealTimers()
})

function bar(container: HTMLElement) {
  return container.querySelector(".agent-beam-bar")
}

describe("AgentBeam", () => {
  it("renders nothing while idle", () => {
    const { container } = render(<AgentBeam working={false} />)
    expect(bar(container)).toBeNull()
  })

  it("shows the sweeping bar while the agent is working", () => {
    const { container } = render(<AgentBeam working={true} />)
    expect(bar(container)).not.toBeNull()
  })

  it("lingers when work stops, then unmounts after the fallback timeout", () => {
    vi.useFakeTimers()
    const { container, rerender } = render(<AgentBeam working={true} />)
    expect(bar(container)).not.toBeNull()

    // Work stops: the beam stays mounted so the current sweep can finish.
    rerender(<AgentBeam working={false} />)
    expect(bar(container)).not.toBeNull()

    // The fallback timer eventually unmounts it even without an
    // animationiteration event (jsdom never fires one on its own).
    act(() => {
      vi.advanceTimersByTime(1600)
    })
    expect(bar(container)).toBeNull()
  })
})
