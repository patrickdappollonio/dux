// Whether a fetched release actually has notes worth rendering, and what to say
// when it does not.
//
// The what's-new screen is driven by a GitHub release body parsed server-side by
// `dux_core::release_notes`, which is a two-level heading reader rather than a
// Markdown parser. A body shaped differently degrades: `## ` becomes the headline
// (which this dialog renders as its TITLE) and `### ` becomes a feature title, and
// anything else lands in the intro prose. So a release whose body is only a
// headline -- entirely reachable, because GitHub prepends `## What's Changed` and
// the release workflow appends `## Installation` -- parses to a headline and
// nothing else, and the dialog used to render a title above a blank body with no
// explanation at all.
//
// Mirrors `ReleaseNotes::has_renderable_body` and `NO_NOTES_EXPLANATION` in
// `crates/dux-core/src/release_notes.rs`. A TS surface cannot import a Rust
// const, so these are plain duplicated definitions: if you reword or re-scope one
// side, do the other in the same change. The required release-body format is
// written down in CONTRIBUTING.md.

import type { ReleaseNotesView } from "./bootstrapApi"

/** Shown in place of the body when `hasRenderableBody` is false. Mirrors
 *  `dux_core::release_notes::NO_NOTES_EXPLANATION`. */
export const NO_NOTES_EXPLANATION =
  "This release published no notes we could read. Open the full notes to see what changed."

/** Whether there is anything to render UNDER the dialog title.
 *
 * The headline is deliberately excluded: it IS the title, so a release carrying
 * only a headline has an empty body. Whitespace-only entries do not count either
 * -- a `### **__**` heading collapses to `""` once inline markup is stripped, and
 * rendering that is a lone blank bullet, which is the same empty screen with
 * extra steps. */
export function hasRenderableBody(
  notes: Pick<ReleaseNotesView, "paragraphs" | "sections"> | null | undefined,
): boolean {
  if (!notes) return false
  return hasContent(notes.paragraphs) || hasContent(notes.sections)
}

function hasContent(entries: string[] | null | undefined): boolean {
  return (entries ?? []).some((entry) => entry.trim().length > 0)
}
