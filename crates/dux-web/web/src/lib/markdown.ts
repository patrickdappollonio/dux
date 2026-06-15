// Recognize markdown files by extension so the editor can offer a rendered
// preview toggle only where it makes sense. Case-insensitive.
const MARKDOWN_EXTENSIONS = [".md", ".markdown", ".mdown", ".mkd", ".mdx"]

export function isMarkdownPath(path: string): boolean {
  const lower = path.toLowerCase()
  return MARKDOWN_EXTENSIONS.some((ext) => lower.endsWith(ext))
}

// A URL is "external" (leave it alone in the preview) when it carries a scheme
// (http:, data:, mailto:, …), is protocol-relative (//host), or is already
// root-absolute (/foo) — anything but a worktree-relative path.
function isExternalUrl(url: string): boolean {
  return /^[a-z][a-z0-9+.-]*:/i.test(url) || url.startsWith("//") || url.startsWith("/")
}

// Resolve a relative asset reference (a markdown image `src`) against the markdown
// FILE's directory into a normalized, worktree-relative path. Returns null when
// the reference is external (see isExternalUrl) or escapes the worktree root via
// `..` — the caller then leaves the URL untouched. Query/hash suffixes are dropped.
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

// The same-origin proxy URL that serves a worktree asset for the markdown
// preview, or null when `src` isn't a worktree-relative reference. The route is
// auth-gated; the path is re-validated server-side for worktree containment.
export function markdownAssetUrl(
  sessionId: string,
  filePath: string,
  src: string,
): string | null {
  const rel = resolveWorktreeRelative(filePath, src)
  if (rel === null) return null
  return `/api/file/raw?session_id=${encodeURIComponent(sessionId)}&path=${encodeURIComponent(rel)}`
}
