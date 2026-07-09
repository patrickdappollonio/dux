// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

// The card only calls `startDormantTab` from the store; stub it so we can render
// in isolation and assert the docs link.
vi.mock("@/lib/store", () => ({ startDormantTab: vi.fn() }))

const { DormantTabCard } = await import("./DormantTabCard")

afterEach(cleanup)

describe("DormantTabCard", () => {
  it("links to the resume docs, safely, in a new tab", () => {
    render(<DormantTabCard sessionId="s1" tabId="b2" provider="claude" />)
    const link = screen.getByRole("link", { name: /how resume works/i })
    expect(link.getAttribute("href")).toBe(
      "https://getdux.app/docs/agent-tabs#how-resume-works",
    )
    expect(link.getAttribute("target")).toBe("_blank")
    expect(link.getAttribute("rel")).toContain("noopener")
    expect(link.getAttribute("rel")).toContain("noreferrer")
  })
})
