// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import { AttentionDot } from "./AttentionDot"

afterEach(() => {
  cleanup()
})

describe("AttentionDot", () => {
  it("renders the cyan-frost fill and keeps the pulse animation", () => {
    render(<AttentionDot withTooltip={false} />)
    const dot = screen.getByLabelText("Needs attention")
    expect(dot.className).toContain("bg-cyan-100")
    expect(dot.className).toContain("motion-safe:animate-attention-pulse")
  })
})
