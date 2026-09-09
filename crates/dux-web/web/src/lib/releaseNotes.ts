// Whether a fetched release has notes worth rendering, and what to say when it
// does not. The server parses a GitHub release body as a two-level heading
// reader, so a body whose human-written part is a single `## ` line parses to a
// headline and nothing else, and the dialog would draw its title over a blank
// panel. That shape is ordinary: GitHub appends its own `## What's Changed` and
// the release workflow appends a rule and `## Installation`.
//
// This file mirrors `body_is_renderable`, `is_invisible_char` and
// `NO_NOTES_EXPLANATION` in `crates/dux-core/src/release_notes.rs`. TypeScript
// cannot import a Rust const, so a Rust test reads this file to catch drift
// (`the_web_mirror_of_the_no_notes_surface_has_not_drifted`): reword one side
// and reword the other in the same change. The required release-body format is
// in CONTRIBUTING.md.
//
// Sharing the code-point set is not enough, because each language's own trim and
// whitespace predicate cover different sets. The answers themselves are pinned
// by `crates/dux-core/tests/fixtures/release_notes_cross_language.json`, which
// both suites read; add a case there rather than to one suite.

import type { ReleaseNotesView } from "./bootstrapApi"

/** Shown in place of the body when `hasRenderableBody` is false. Mirrors
 *  `dux_core::release_notes::NO_NOTES_EXPLANATION`. */
export const NO_NOTES_EXPLANATION =
  "This release published no notes we could read. Open the full notes to see what changed."

/** The code points both surfaces treat as invisible, as comma-separated hex
 *  ranges. Mirrors `is_invisible_char`, which a Rust test reads back from this
 *  declaration.
 *
 *  Neither language's own `trim` can be the definition: Rust trims U+0085 and
 *  JavaScript does not, JavaScript trims U+FEFF and Rust does not. The set is
 *  Unicode `White_Space` plus the zero-width characters, which render as nothing
 *  at all and so are worse than whitespace here. */
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
/** `<br>`, `<br/>`, `<br />`, in any case. The gaps are the shared invisible set
 *  rather than `\s`, which covers U+FEFF and misses U+0085 while Rust's
 *  `char::is_whitespace` does the opposite. Both are in the shared fixture. */
const HTML_BREAK_SOURCE = `<br[${INVISIBLE_CLASS}]*\\/?[${INVISIBLE_CLASS}]*>`

/** One left-to-right pass over comments, breaks and invisibles, in that
 *  priority, which is the scan the Rust side does. Sequential passes answer
 *  differently: removing every comment first manufactures a break out of
 *  `<br<!--x-->>`, which holds no `<br>` at any single position. */
const STRIP_RE = new RegExp(
  `${HTML_COMMENT_SOURCE}|${HTML_BREAK_SOURCE}|[${INVISIBLE_CLASS}]`,
  "giu",
)
/** Three or more of the same `-`, `*` or `_`: a Markdown thematic break. The
 *  spaced forms (`- - -`) reach this already closed up, because the spaces are
 *  invisible and have been dropped. */
const THEMATIC_BREAK_RE = /^(-{3,}|\*{3,}|_{3,})$/

/** Whether there is anything to render under the dialog title. The headline is
 * excluded, because it is the title. "Not the empty string" is not enough
 * either: `### **__**` collapses to `""`, zero-width characters render as
 * nothing, and the appended horizontal rule renders as a lone `---`. */
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
