// @vitest-environment jsdom
import { render } from "@testing-library/react"
import { describe, expect, it } from "vitest"

import { IconTile } from "@/components/IconTile"

describe("IconTile", () => {
  it("is hidden from a screen reader whichever way it is used", () => {
    const { container } = render(
      <>
        <IconTile size="md" use="action">
          a
        </IconTile>
        <IconTile size="sm" use="display">
          1
        </IconTile>
      </>,
    )
    for (const tile of container.querySelectorAll("[data-use]")) {
      expect(tile.getAttribute("aria-hidden")).toBe("true")
    }
  })

  it("tells a tile on a pressable row from a read-only one by its border", () => {
    const { container } = render(
      <>
        <IconTile size="md" use="action">
          a
        </IconTile>
        <IconTile size="md" use="display">
          b
        </IconTile>
      </>,
    )
    const [action, display] = container.querySelectorAll("[data-use]")
    expect(action.className).toContain("border")
    expect(display.className).not.toContain("border")
  })
})
