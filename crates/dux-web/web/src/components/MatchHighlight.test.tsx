// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest"
import { cleanup, render } from "@testing-library/react"

import { MatchHighlight } from "@/components/MatchHighlight"
import { matchCharRanges } from "@/lib/searchMatch"

afterEach(cleanup)

function lit(text: string, needle: string): string[] {
  const { container } = render(
    <MatchHighlight text={text} ranges={matchCharRanges(text, [needle])} />,
  )
  expect(container.textContent).toBe(text)
  return [...container.querySelectorAll("mark")].map((m) => m.textContent ?? "")
}

describe("MatchHighlight", () => {
  it("lights several ranges, keeping the text's own case", () => {
    expect(lit("Tailscale on your tailnet", "tail")).toEqual(["Tail", "tail"])
  })

  it("lights nothing for an empty query", () => {
    expect(lit("Tailscale", "")).toEqual([])
  })
})
