import { readFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { fileURLToPath } from "node:url"

import { describe, expect, it } from "vitest"

import {
  entryIsRenderable,
  hasRenderableBody,
  INVISIBLE_CODE_POINTS,
  NO_NOTES_EXPLANATION,
  stripInvisibleMarkup,
} from "./releaseNotes"

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

  it("does not count the horizontal rule the release pipeline appends", () => {
    // `.github/workflows/release.yml` appends `---` before its `## Installation`
    // section, so a release with a one-line human note leaves the parser a
    // headline and this. Rendering it is the blank panel with one extra
    // character.
    expect(hasRenderableBody({ paragraphs: ["---"], sections: [] })).toBe(false)
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

describe("entryIsRenderable", () => {
  // The same table as `bodies_that_look_empty_to_a_human_are_treated_as_empty`
  // in `crates/dux-core/src/release_notes.rs`. Both surfaces must answer the
  // same way, or one shows a body and the other the no-notes explanation.
  it.each([
    ["a horizontal rule", "---"],
    ["a long horizontal rule", "-------"],
    ["an asterisk rule", "***"],
    ["an underscore rule", "___"],
    ["a spaced rule", "- - -"],
    ["an HTML line break", "<br>"],
    ["a self-closed HTML line break", "<br />"],
    ["an uppercase HTML line break", "<BR/>"],
    ["a zero-width space", "​"],
    ["a byte-order mark", "﻿"],
    ["a next-line character", ""],
    ["an HTML comment", "<!-- release notes go here -->"],
    ["an unterminated HTML comment", "<!-- oops"],
    ["nothing at all", ""],
  ])("treats %s as empty", (_what, entry) => {
    expect(entryIsRenderable(entry)).toBe(false)
  })

  it.each([
    ["prose containing a rule", "Before --- after."],
    ["prose containing a break", "Line one<br>line two."],
    ["prose containing a comment", "A note <!-- aside --> that still reads."],
    ["a single dash, which is not a thematic break", "-"],
    ["ordinary prose", "Fixes the thing."],
  ])("still renders %s", (_what, entry) => {
    expect(entryIsRenderable(entry)).toBe(true)
  })
})

describe("the invisible-character set", () => {
  // Regression for a real divergence: `String.prototype.trim` does not trim
  // U+0085, and Rust's `str::trim` does not trim U+FEFF, so relying on either
  // language's trim made the two surfaces disagree about the same release body.
  it("covers what each language's own trim would miss", () => {
    expect("".trim()).not.toBe("") // JS trim misses it...
    expect(stripInvisibleMarkup("")).toBe("") // ...this does not.
    expect(stripInvisibleMarkup("﻿")).toBe("") // Rust's trim misses this one.
    for (const c of ["​", "‌", "‍", "⁠"]) {
      expect(stripInvisibleMarkup(c)).toBe("")
    }
  })

  it("leaves visible text alone", () => {
    expect(stripInvisibleMarkup("a-#日\u{1F986}")).toBe("a-#日\u{1F986}")
  })

  it("is declared as hex ranges a Rust test can read back", () => {
    // `the_web_mirror_of_the_no_notes_surface_has_not_drifted` parses this exact
    // string and compares it to `is_invisible_char` over every code point, so the
    // shape has to stay machine-readable.
    expect(INVISIBLE_CODE_POINTS).toMatch(/^[0-9A-F]{4}(-[0-9A-F]{4})?(,[0-9A-F]{4}(-[0-9A-F]{4})?)*$/)
  })
})

describe("the shared cross-language fixture", () => {
  // The other half of the pin in `release_notes.rs`. The Rust test compares the
  // invisible SET; this compares the ANSWERS, because the two surfaces can hold
  // the same set and still disagree: each `<br>` matcher used to ask its own
  // language what whitespace is, and this file used to strip comments, breaks and
  // invisibles in three sequential passes where Rust scans once, left to right.
  // Both sides read this same file, so a case passes only when both agree.
  const cases = JSON.parse(
    readFileSync(
      join(
        dirname(fileURLToPath(import.meta.url)),
        "../../../../dux-core/tests/fixtures/release_notes_cross_language.json",
      ),
      "utf8",
    ),
  ) as { cases: { what: string; entry: string; renderable: boolean }[] }

  it("has not lost its cases", () => {
    expect(cases.cases.length).toBeGreaterThanOrEqual(10)
  })

  it.each(cases.cases.map((c) => [c.what, c.entry, c.renderable] as const))(
    "agrees with the Rust side about %s",
    (_what, entry, renderable) => {
      expect(entryIsRenderable(entry)).toBe(renderable)
    },
  )
})
