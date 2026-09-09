// Pure path predicates deciding how the editor presents a file.
//
// - `isImagePreviewPath`: non-SVG images, which never fetch the file's text.
//   The server refuses a read over the editable size cap before it sniffs
//   text or binary, so such a tab would park on a spinner.
// - `previewKind`: text formats with a draft-accurate preview toggle, rendered
//   over the current draft rather than the saved file.
//
// `.svg` is an image to `fileIcons`, which is right for the tree icon, so
// `isImagePreviewPath` subtracts it rather than trusting the icon kind.

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
