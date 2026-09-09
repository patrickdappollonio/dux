import { rootApiBase, type EditorRoot } from "@/lib/editorRoot"

// Recognize markdown files by extension so the editor can offer a rendered
// preview toggle only where it makes sense. Case-insensitive.
const MARKDOWN_EXTENSIONS = [".md", ".markdown", ".mdown", ".mkd", ".mdx"]

export function isMarkdownPath(path: string): boolean {
  const lower = path.toLowerCase()
  return MARKDOWN_EXTENSIONS.some((ext) => lower.endsWith(ext))
}

// A URL is external, and left alone in the preview, when it carries a scheme, is
// protocol-relative, or is root-absolute: anything but a worktree-relative path.
function isExternalUrl(url: string): boolean {
  return /^[a-z][a-z0-9+.-]*:/i.test(url) || url.startsWith("//") || url.startsWith("/")
}

// Resolve a relative asset reference against the markdown file's own directory into a
// normalized, worktree-relative path. Null when the reference is external or escapes the
// worktree root via `..`, and the caller then leaves the URL untouched. Query and hash dropped.
export function resolveWorktreeRelative(
  filePath: string,
  src: string,
): string | null {
  if (!src || isExternalUrl(src)) return null
  const bare = src.split(/[?#]/, 1)[0]
  if (!bare) return null
  const slash = filePath.lastIndexOf("/")
  const baseParts = slash === -1 ? [] : filePath.slice(0, slash).split("/")
  const stack: string[] = [...baseParts]
  for (const part of bare.split("/")) {
    if (part === "" || part === ".") continue
    if (part === "..") {
      if (stack.length === 0) return null // escapes the worktree root
      stack.pop()
    } else {
      stack.push(part)
    }
  }
  return stack.length > 0 ? stack.join("/") : null
}

// The same-origin proxy URL serving an asset under the editor's root, or null when `src` is
// not a root-relative reference. The server re-validates the path for containment in that root.
export function markdownAssetUrl(
  root: EditorRoot,
  filePath: string,
  src: string,
): string | null {
  const rel = resolveWorktreeRelative(filePath, src)
  if (rel === null) return null
  return `${rootApiBase(root)}/files/raw?path=${encodeURIComponent(rel)}`
}
