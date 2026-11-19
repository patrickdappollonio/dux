import { describe, expect, it } from "vitest"

import { HOVER_REVEAL, REVEALED_ON_ANY_FOCUS } from "./hoverReveal"
import { ALWAYS_REVEALED_ON_TOUCH } from "./touchReveal"

// jsdom evaluates no Tailwind, so the classes are what can be pinned. A browser
// check must confirm the same states: Tab onto a row (or straight onto its ⋯)
// with the mouse elsewhere, and the ⋯ is painted with its whole ring.
describe("HOVER_REVEAL", () => {
  const classes = HOVER_REVEAL.split(" ")

  it("takes no width and no paint at rest, clipping for the width animation", () => {
    expect(classes).toEqual(expect.arrayContaining(["max-w-0", "opacity-0", "overflow-hidden"]))
  })

  it("reveals on hover and while its menu is open", () => {
    expect(classes).toEqual(
      expect.arrayContaining([
        "group-hover/reveal:max-w-(--reveal-cap)",
        "group-hover/reveal:opacity-100",
        "has-[[data-popup-open]]:max-w-(--reveal-cap)",
        "has-[[data-popup-open]]:opacity-100",
      ]),
    )
  })

  // The changes row revealed its ⋯ on hover only, so a keyboard user Tabbing
  // onto "Actions for <file>" held focus on a button with nothing drawn. The
  // row itself takes roving focus, so both the row and a descendant count.
  it("reveals whenever the row or its ⋯ holds keyboard focus", () => {
    expect(classes).toEqual(
      expect.arrayContaining([
        "group-focus-visible/reveal:max-w-(--reveal-cap)",
        "group-focus-visible/reveal:opacity-100",
        "group-has-[:focus-visible]/reveal:max-w-(--reveal-cap)",
        "group-has-[:focus-visible]/reveal:opacity-100",
      ]),
    )
  })

  // The tab strips' ⋯ kept the clip under keyboard focus and lost 3px of its
  // ring on every side.
  it("lets the ring out while the ⋯ holds keyboard focus", () => {
    expect(classes).toContain("has-[:focus-visible]:overflow-visible")
  })

  it("is always revealed and unclipped under a coarse pointer", () => {
    expect(HOVER_REVEAL).toContain(ALWAYS_REVEALED_ON_TOUCH)
    expect(classes).toContain("pointer-coarse:overflow-visible")
  })

  // A mouse click focuses a row without :focus-visible; only the sites that
  // always revealed on that keep doing so, through this opt-in.
  it("leaves mouse focus to the opt-in", () => {
    expect(HOVER_REVEAL).not.toMatch(/focus-within/)
    expect(REVEALED_ON_ANY_FOCUS).toContain("group-focus-within/reveal:opacity-100")
  })
})
