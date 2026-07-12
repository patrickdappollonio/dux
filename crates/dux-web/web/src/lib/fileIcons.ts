// Pure path -> icon-KIND mapper for the file tree, kept free of any React or
// lucide import so it is unit-testable in node (mirrors `pathExt.ts`'s reason
// for staying framework-free). A small component (`FileTreeIcon.tsx`) maps a
// kind to its lucide glyph. This module is NOT the git-status marker, that
// stays `FileStatusIcon`/`fileStatusMeta` (renders on the right of a row); this
// is the LEFT-side, always-present file-type icon.

import { extensionForPath, fileNameForPath } from "@/lib/pathExt"

export type FileIconKind =
  | "folder"
  | "folder-open"
  | "folder-empty"
  | "code"
  | "image"
  | "config"
  | "markdown"
  | "text"
  | "lock"
  | "binary"
  | "file" // generic fallback

// Directory icon kind. "empty" (a loaded dir with zero children) outranks
// "open": an empty dir is visibly distinct whether expanded or collapsed,
// since there is nothing to show open either way.
export function dirIconKind(opts: { open: boolean; empty: boolean }): FileIconKind {
  if (opts.empty) return "folder-empty"
  return opts.open ? "folder-open" : "folder"
}

const CODE_EXTENSIONS = new Set([
  ".ts", ".tsx", ".js", ".jsx", ".mjs", ".cjs",
  ".rs", ".go", ".py", ".rb", ".java", ".kt",
  ".c", ".h", ".cpp", ".cc", ".cs", ".php", ".swift",
  ".scala", ".sh", ".bash", ".zsh", ".lua", ".sql", ".vue", ".svelte",
])

const IMAGE_EXTENSIONS = new Set([
  ".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".ico", ".bmp", ".avif",
])

const CONFIG_EXTENSIONS = new Set([
  ".json", ".toml", ".yaml", ".yml", ".ini", ".env", ".conf", ".cfg", ".properties",
])
const CONFIG_FILENAMES = new Set([
  "dockerfile", "makefile", ".gitignore", ".editorconfig",
])

const MARKDOWN_EXTENSIONS = new Set([".md", ".mdx", ".markdown"])

const TEXT_EXTENSIONS = new Set([".txt", ".rst", ".log"])

const LOCK_FILENAMES = new Set([
  "package-lock.json", "yarn.lock", "pnpm-lock.yaml", "cargo.lock", "go.sum",
])

const BINARY_EXTENSIONS = new Set([
  ".bin", ".exe", ".dll", ".so", ".dylib", ".o", ".a", ".wasm",
  ".zip", ".tar", ".gz", ".pdf", ".woff", ".woff2", ".ttf",
])

// File icon kind by extension/filename. Checks are ordered most-specific
// first (lockfiles before generic extension matches, since e.g. "*.lock"
// files have no useful extension-based mapping otherwise).
export function fileIconKind(path: string): FileIconKind {
  const name = fileNameForPath(path)
  const ext = extensionForPath(path)

  if (name.endsWith(".lock") || LOCK_FILENAMES.has(name)) return "lock"
  if (CONFIG_FILENAMES.has(name)) return "config"
  if (CODE_EXTENSIONS.has(ext)) return "code"
  if (IMAGE_EXTENSIONS.has(ext)) return "image"
  if (CONFIG_EXTENSIONS.has(ext)) return "config"
  if (MARKDOWN_EXTENSIONS.has(ext)) return "markdown"
  if (TEXT_EXTENSIONS.has(ext)) return "text"
  if (BINARY_EXTENSIONS.has(ext)) return "binary"
  return "file"
}
