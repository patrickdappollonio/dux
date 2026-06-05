import { describe, expect, it } from "vitest"

import { statusPresentation } from "./statusLine"

describe("statusPresentation", () => {
  it("maps info to plain muted text with no icon", () => {
    expect(statusPresentation("info")).toEqual({
      icon: "none",
      className: "text-muted-foreground",
    })
  })

  it("maps busy to the braille spinner with the TUI warning yellow", () => {
    expect(statusPresentation("busy")).toEqual({
      icon: "spinner",
      className: "text-amber-500",
    })
  })

  it("maps warning to the triangle icon with amber text", () => {
    expect(statusPresentation("warning")).toEqual({
      icon: "warning",
      className: "text-amber-500",
    })
  })

  it("maps error to the circle-x icon with destructive text", () => {
    expect(statusPresentation("error")).toEqual({
      icon: "error",
      className: "text-destructive",
    })
  })

  it("falls back to the info presentation for an unknown tone", () => {
    expect(statusPresentation("nonsense")).toEqual({
      icon: "none",
      className: "text-muted-foreground",
    })
  })
})
