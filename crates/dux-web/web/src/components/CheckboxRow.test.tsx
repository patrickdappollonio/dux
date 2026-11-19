// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest"
import { cleanup, render, screen } from "@testing-library/react"

import { CheckboxRow } from "./CheckboxRow"

afterEach(cleanup)

describe("checkbox row touch height", () => {
  // jsdom cannot evaluate media queries, so the classes are what can be pinned.
  it("makes the row 40px at phone width and under a coarse pointer, with the box's tap area filling that height", () => {
    render(
      <CheckboxRow id="opt" checked={false} onCheckedChange={() => {}}>
        Option
      </CheckboxRow>,
    )
    const box = screen.getByRole("checkbox")
    const row = box.parentElement as HTMLElement
    expect(Array.from(row.classList)).toEqual(
      expect.arrayContaining(["max-md:min-h-10", "pointer-coarse:min-h-10"]),
    )
    expect(
      Array.from(box.classList),
      "the label stretches across the row's height; the box's own tap margin must cover the same 40px so no strip above or below it is dead",
    ).toEqual(
      expect.arrayContaining(["max-md:after:-inset-y-[13px]", "pointer-coarse:after:-inset-y-[13px]"]),
    )
  })
})
