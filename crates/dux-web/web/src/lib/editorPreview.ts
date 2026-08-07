// Pure path predicates deciding HOW the editor presents a file, kept free of
// React so they are unit-testable in node (mirrors fileIcons.ts).
//
// Two different mechanisms, split on purpose:
//
// - `isImagePreviewPath`: raster (and other non-SVG) images. These NEVER
//   fetch `/read`: the server stats the size and refuses anything over the
//   5 MiB editable cap BEFORE the text/binary sniff runs, so an image tab
//   waiting on a buffer would park on a spinner (or read megabytes only to
//   discard them). EditorBody skips `loadFileBuffer` for them entirely and
//   renders a read-only pane from `fileApi.rawUrl` instead.
//
// - `previewKind`: text formats with a draft-accurate preview TOGGLE:
//   markdown renders through react-markdown, SVG through a Blob object URL
//   over the CURRENT DRAFT. SVGs stay editable in Monaco; the preview is a
//   view of the unsaved text, exactly like markdown's.
//
// `.svg` is in fileIcons' IMAGE_EXTENSIONS (right for the tree icon), which is
// why `isImagePreviewPath` must subtract it here rather than trust the icon
// kind alone.

import { fileIconKind } from "@/lib/fileIcons"
import { isMarkdownPath } from "@/lib/markdown"
import { extensionForPath } from "@/lib/pathExt"

export function isSvgPath(path: string): boolean {
  return extensionForPath(path) === ".svg"
}

// True for image files that render as a read-only preview pane from /raw
// (never fetching /read). SVG is excluded: it opens in Monaco as text.
export function isImagePreviewPath(path: string): boolean {
  return fileIconKind(path) === "image" && !isSvgPath(path)
}

// The kind of draft-accurate preview a text file offers, or null when the
// Preview toggle should not render at all.
export type PreviewKind = "markdown" | "svg"

export function previewKind(path: string): PreviewKind | null {
  if (isMarkdownPath(path)) return "markdown"
  if (isSvgPath(path)) return "svg"
  return null
}
