// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "./select"

afterEach(cleanup)

// jsdom cannot evaluate media queries, so the classes are what can be pinned.
describe("select touch heights", () => {
  it("lifts the trigger to 40px at phone width and under a coarse pointer at any width", () => {
    render(
      <Select>
        <SelectTrigger>
          <SelectValue />
        </SelectTrigger>
      </Select>,
    )
    expect(
      Array.from(document.querySelector('[data-slot="select-trigger"]')?.classList ?? []),
      "the 32px trigger is under the 40px touch floor for a finger on a phone and on a touch tablet",
    ).toEqual(expect.arrayContaining(["max-md:min-h-10", "pointer-coarse:min-h-10"]))
  })

  it("gives option rows 44px at phone width and 40px under a coarse pointer at desktop width", async () => {
    render(
      <Select open value="a">
        <SelectTrigger>
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="a">Alpha</SelectItem>
        </SelectContent>
      </Select>,
    )
    await screen.findByRole("option")
    const classes = Array.from(
      document.querySelector('[data-slot="select-item"]')?.classList ?? [],
    )
    expect(
      classes,
      "44px below md, 40px for a finger at md and up, compact only for a fine pointer at md and up",
    ).toEqual(
      expect.arrayContaining([
        "min-h-11",
        "md:pointer-coarse:min-h-10",
        "md:not-pointer-coarse:min-h-0",
      ]),
    )
  })
})
