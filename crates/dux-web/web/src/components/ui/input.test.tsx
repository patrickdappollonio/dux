// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import { Input } from "./input"

afterEach(cleanup)

describe("text input touch height", () => {
  // jsdom cannot evaluate media queries, so the classes are what can be pinned.
  it("lifts the field to 40px at phone width and under a coarse pointer at any width", () => {
    render(<Input aria-label="Name" />)
    expect(
      Array.from(screen.getByRole("textbox").classList),
      "the 32px field is under the 40px touch floor for a finger on a phone and on a touch tablet",
    ).toEqual(expect.arrayContaining(["max-md:min-h-10", "pointer-coarse:min-h-10"]))
  })
})
