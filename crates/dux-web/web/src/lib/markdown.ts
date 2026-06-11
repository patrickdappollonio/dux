// Recognize markdown files by extension so the editor can offer a rendered
// preview toggle only where it makes sense. Case-insensitive.
const MARKDOWN_EXTENSIONS = [".md", ".markdown", ".mdown", ".mkd", ".mdx"]

export function isMarkdownPath(path: string): boolean {
  const lower = path.toLowerCase()
  return MARKDOWN_EXTENSIONS.some((ext) => lower.endsWith(ext))
}
