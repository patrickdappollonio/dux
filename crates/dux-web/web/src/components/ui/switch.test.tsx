// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import { Switch } from "./switch"

afterEach(cleanup)

describe("switch tap area", () => {
  // jsdom cannot evaluate media queries, so the classes are what can be pinned.
  it("extends the tap area with an invisible margin at phone width and under a coarse pointer without growing the switch", () => {
    render(<Switch aria-label="Toggle" />)
    const classes = Array.from(screen.getByRole("switch").classList)
    expect(classes).toEqual(
      expect.arrayContaining([
        "max-md:after:absolute",
        "max-md:after:-inset-2.5",
        "pointer-coarse:after:absolute",
        "pointer-coarse:after:-inset-2.5",
      ]),
    )
    expect(classes, "the visible switch keeps its size on every device").toEqual(
      expect.arrayContaining(["h-5", "w-9"]),
    )
  })
})
