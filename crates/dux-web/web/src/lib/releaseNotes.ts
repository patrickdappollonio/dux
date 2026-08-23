// Whether a fetched release actually has notes worth rendering, and what to say
// when it does not.
//
// The what's-new screen is driven by a GitHub release body parsed server-side by
// `dux_core::release_notes`, which is a two-level heading reader rather than a
// Markdown parser. A body shaped differently degrades: `## ` becomes the headline
// (which this dialog renders as its TITLE) and `### ` becomes a feature title, and
// anything else lands in the intro prose. So a release whose body is only a
// headline parses to a headline and nothing else, so without this check the
// dialog renders a title above a blank body with no explanation.
//
// That shape is not exotic. GitHub APPENDS its generated `## What's Changed`
// section (it lands after every human-written section, not before them), the
// release workflow APPENDS a horizontal rule and then `## Installation`, and the
// parser stops at the second top-level heading. A release whose human-written part
// is a single `## ` line is left with a headline and, at most, that rule.
//
// Mirrors `body_is_renderable`, `is_invisible_char` and `NO_NOTES_EXPLANATION` in
// `crates/dux-core/src/release_notes.rs`. A TS surface cannot import a Rust const,
// so these are plain duplicated definitions, and a Rust test READS this file to
// catch drift (`the_web_mirror_of_the_no_notes_surface_has_not_drifted`). If you
// reword or re-scope one side, do the other in the same change. The required
// release-body format is written down in CONTRIBUTING.md.
//
// Sharing the code-point set is not enough on its own: the two surfaces held the
// same set and still disagreed, because the `<br>` matcher here asked `\s` and the
// Rust one asked `char::is_whitespace` (different sets), and because this file ran
// three sequential passes where Rust scans once. So the answers themselves are
// pinned by a fixture BOTH test suites read,
// `crates/dux-core/tests/fixtures/release_notes_cross_language.json`. Add a case
// there rather than to one suite.

import type { ReleaseNotesView } from "./bootstrapApi"

/** Shown in place of the body when `hasRenderableBody` is false. Mirrors
 *  `dux_core::release_notes::NO_NOTES_EXPLANATION`. */
export const NO_NOTES_EXPLANATION =
  "This release published no notes we could read. Open the full notes to see what changed."

/** The code points BOTH surfaces treat as invisible, as comma-separated hex
 *  ranges. Mirrors `dux_core::release_notes::is_invisible_char`, which a Rust test
 *  reads back from this very declaration.
 *
 *  Neither language's own `trim` can be the definition, because they trim
 *  DIFFERENT sets: Rust trims U+0085 (next line) and JavaScript does not;
 *  JavaScript trims U+FEFF (byte-order mark) and Rust does not. Left to their own
 *  trims, the same release body showed a body here and the no-notes explanation in
 *  the terminal.
 *
 *  The set is Unicode `White_Space` plus the zero-width characters, which are
 *  worse than whitespace: they render as literally nothing, so a body made of them
 *  is the original blank-panel bug with an extra step. */
export const INVISIBLE_CODE_POINTS =
  "0009-000D,0020,0085,00A0,1680,2000-200A,2028,2029,202F,205F,3000,200B-200D,2060,FEFF"

/** The body of a character class matching exactly `INVISIBLE_CODE_POINTS`. */
const INVISIBLE_CLASS = INVISIBLE_CODE_POINTS.split(",")
  .map((range) =>
    range
      .split("-")
      .map((hex) => `\\u{${hex}}`)
      .join("-"),
  )
  .join("")

/** `<!-- ... -->`, including an unterminated one, which swallows the rest. */
const HTML_COMMENT_SOURCE = String.raw`<!--[\s\S]*?(?:-->|$)`
/** `<br>`, `<br/>`, `<br />`, in any case.
 *
 *  The gaps are the SHARED invisible set, not `\s`. `\s` covers U+FEFF and misses
 *  U+0085; Rust's `char::is_whitespace` does the opposite, so `<br` U+FEFF `/>` was
 *  empty here and renderable in the terminal, and `<br` U+0085 `/>` was the reverse
 *  (both pinned in the shared fixture the tests on both sides read). */
const HTML_BREAK_SOURCE = `<br[${INVISIBLE_CLASS}]*\\/?[${INVISIBLE_CLASS}]*>`

/** ONE left-to-right pass over comments, breaks and invisibles, in that priority.
 *
 *  Three sequential passes are not the same thing and did not answer the same way:
 *  removing every comment first MANUFACTURES a break out of `<br<!--x-->>`, text
 *  that holds no `<br>` at any single position, so the browser called it empty while
 *  the terminal (which has always scanned once, left to right, trying comment then
 *  break then invisible at each index) kept it. Alternation in this order reproduces
 *  that scan exactly. */
const STRIP_RE = new RegExp(
  `${HTML_COMMENT_SOURCE}|${HTML_BREAK_SOURCE}|[${INVISIBLE_CLASS}]`,
  "giu",
)
/** Three or more of the same `-`, `*` or `_`: a Markdown thematic break. The
 *  spaced forms (`- - -`) reach this already closed up, because the spaces are
 *  invisible and have been dropped. */
const THEMATIC_BREAK_RE = /^(-{3,}|\*{3,}|_{3,})$/

/** Whether there is anything to render UNDER the dialog title.
 *
 * The headline is deliberately excluded: it IS the title, so a release carrying
 * only a headline has an empty body. Nor is "not the empty string" enough: a
 * `### **__**` heading collapses to `""` once inline markup is stripped, a body of
 * only zero-width characters renders as nothing at all, and the horizontal rule
 * the release pipeline appends renders as a lone `---`. Each of those is the same
 * empty screen with an extra step. */
export function hasRenderableBody(
  notes: Pick<ReleaseNotesView, "paragraphs" | "sections"> | null | undefined,
): boolean {
  if (!notes) return false
  return hasContent(notes.paragraphs) || hasContent(notes.sections)
}

function hasContent(entries: string[] | null | undefined): boolean {
  return (entries ?? []).some(entryIsRenderable)
}

/** Whether one parsed entry (an intro paragraph or a feature title) is worth
 *  rendering. Mirrors `entry_is_renderable` in `release_notes.rs`. */
export function entryIsRenderable(entry: string): boolean {
  const visible = stripInvisibleMarkup(entry)
  return visible.length > 0 && !THEMATIC_BREAK_RE.test(visible)
}

/** Drops HTML comments, HTML line breaks, and every invisible code point, leaving
 *  what a reader would actually see. Mirrors `strip_invisible_markup` in
 *  `release_notes.rs`, one left-to-right pass (see `STRIP_RE`). */
export function stripInvisibleMarkup(entry: string): string {
  return entry.replace(STRIP_RE, "")
}
