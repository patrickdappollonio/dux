import { describe, expect, it } from "vitest"

import { hasRenderableBody, NO_NOTES_EXPLANATION } from "./releaseNotes"

describe("hasRenderableBody", () => {
  it("is true when there is intro prose", () => {
    expect(
      hasRenderableBody({ paragraphs: ["Fixes the thing."], sections: [] }),
    ).toBe(true)
  })

  it("is true when there are feature titles", () => {
    expect(
      hasRenderableBody({ paragraphs: [], sections: ["A feature"] }),
    ).toBe(true)
  })

  it("is false for an empty body, which is also what a headline-only release parses to", () => {
    // The headline is rendered as the dialog TITLE, so it is not part of the
    // body and is not an input here at all. That is the shape that used to
    // render a title above an empty pane.
    expect(hasRenderableBody({ paragraphs: [], sections: [] })).toBe(false)
  })

  it("does not count whitespace-only entries as content", () => {
    // A `### **__**` heading collapses to "" once the server strips inline
    // markup, and a lone blank bullet is not an explanation.
    expect(hasRenderableBody({ paragraphs: ["  "], sections: [""] })).toBe(
      false,
    )
  })

  it("treats a missing or nullish payload as having no body", () => {
    expect(hasRenderableBody(null)).toBe(false)
    expect(hasRenderableBody(undefined)).toBe(false)
    // A server older than these fields, or a hand-rolled payload.
    expect(
      hasRenderableBody({} as unknown as { paragraphs: string[]; sections: string[] }),
    ).toBe(false)
  })

  it("has an explanation that names the escape hatch the dialog offers", () => {
    // The dialog's primary button is "Open full notes", so the copy has to point
    // at it rather than leaving the reader with a dead end.
    expect(NO_NOTES_EXPLANATION).toMatch(/full notes/i)
  })
})
