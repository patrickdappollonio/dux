// Pure helpers for the editor's LAZY file tree. The tree is a partially-loaded
// view of the worktree: each directory's children are fetched from
// `/files/tree` the first time the directory is expanded, and cached in a
// Map<dirPath, DirState> owned by the component. Kept free of React so it's
// trivially unit-testable.

// One directory entry as returned by the server's `/files/tree` route
// (mirrors the Rust `DirEntryInfo`). Entries arrive pre-sorted dirs-first,
// case-insensitive.
export interface DirEntry {
  // The child's own name (final path segment).
  name: string
  // The child's worktree-relative path.
  path: string
  // True for a directory (including an in-worktree symlinked dir).
  is_dir: boolean
  // True for a symlink of any kind. A symlinked dir that escapes the worktree
  // is reported with is_dir=false and expandable=false.
  is_symlink: boolean
  // True when this entry's children may be requested via `/files/tree`.
  expandable: boolean
}

// The loaded-directory cache: dirPath ("" = root) → its children, or a
// sentinel while loading / on error.
export type DirState =
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "loaded"; entries: DirEntry[] }

/// The ancestor directory paths of a file path (e.g. "a/b/c.ts" → ["a", "a/b"]),
/// used to auto-expand the tree down to a file opened from elsewhere.
export function ancestorDirs(filePath: string): string[] {
  const segments = filePath.split("/").filter(Boolean)
  const dirs: string[] = []
  let acc = ""
  for (let i = 0; i < segments.length - 1; i++) {
    acc = acc ? `${acc}/${segments[i]}` : segments[i]
    dirs.push(acc)
  }
  return dirs
}

/// Given a target file path and the set of already-loaded dirs, return the
/// ancestor dirs (root included) that still need fetching, top-down, so a
/// deep link can expand the chain to reveal the file.
export function dirsToLoadFor(filePath: string, loaded: Set<string>): string[] {
  return ["", ...ancestorDirs(filePath)].filter((d) => !loaded.has(d))
}

// One render row of the flattened lazy tree.
export interface TreeRow {
  path: string
  name: string
  depth: number
  isDir: boolean
  expandable: boolean
  isSymlink: boolean
  // For dir rows: "loading" while the dir's children fetch is in flight (or an
  // expanded dir has no cache entry yet), "error" when the fetch failed.
  // Placeholder child rows carry the same state at depth+1 so the component
  // can render a spinner/retry row.
  state: "idle" | "loading" | "error"
  // Explicit discriminant for placeholder rows, checked by the component
  // INSTEAD of the row's path. flattenLazy synthesizes a `<dir>/__loading__`
  // or `<dir>/__error__` path for placeholder rows purely so each has a
  // unique React `key`; a real worktree file named `__loading__` or
  // `__error__` still gets `kind: "entry"` and renders as a normal file row.
  kind: "entry" | "loading" | "error"
  // True for a dir row whose cache entry is `loaded` with zero children.
  // Independent of `expanded`: the file-tree icon shows a distinct empty
  // glyph whether the empty dir is expanded or collapsed, as soon as it's
  // been fetched once. Always false for file rows and for dirs never fetched.
  empty: boolean
}

/// The cached dir paths strictly nested under `path` (not `path` itself),
/// e.g. descendantDirPaths(dirs, "a") with dirs keyed "a", "a/b", "a/b/c",
/// "ax" returns ["a/b", "a/b/c"]. Used on collapse to evict a subtree's
/// cached listings (and any still-loading/errored entries) instead of
/// leaking them in memory forever.
export function descendantDirPaths(
  dirs: Map<string, DirState>,
  path: string,
): string[] {
  const prefix = `${path}/`
  return [...dirs.keys()].filter((k) => k.startsWith(prefix))
}

function entryState(
  entry: DirEntry,
  expanded: boolean,
  childState: DirState | undefined,
): TreeRow["state"] {
  if (!entry.is_dir) return "idle"
  if (childState?.status === "error") return "error"
  if (expanded && childState?.status !== "loaded") return "loading"
  return "idle"
}

function entryRow(
  entry: DirEntry,
  depth: number,
  expanded: boolean,
  childState: DirState | undefined,
): TreeRow {
  return {
    path: entry.path,
    name: entry.name,
    depth,
    isDir: entry.is_dir,
    expandable: entry.expandable,
    isSymlink: entry.is_symlink,
    state: entryState(entry, expanded, childState),
    kind: "entry",
    empty:
      entry.is_dir &&
      childState?.status === "loaded" &&
      childState.entries.length === 0,
  }
}

function placeholderRow(
  path: string,
  depth: number,
  state: "loading" | "error",
): TreeRow {
  return {
    path: `${path}/__${state}__`,
    name: "",
    depth,
    isDir: false,
    expandable: false,
    isSymlink: false,
    state,
    kind: state,
    empty: false,
  }
}

function expandedChildRows(
  dirs: Map<string, DirState>,
  expanded: Set<string>,
  entry: DirEntry,
  childState: DirState | undefined,
  depth: number,
): TreeRow[] {
  if (!entry.is_dir || !expanded.has(entry.path)) return []
  if (childState?.status === "loaded") {
    return flattenLazy(dirs, expanded, entry.path, depth + 1)
  }
  const state = childState?.status === "error" ? "error" : "loading"
  return [placeholderRow(entry.path, depth + 1, state)]
}

/// Flatten the loaded tree into render rows honoring the `expanded` set. Only
/// descends into dirs that are BOTH expanded AND loaded; an expanded-but-not-
/// yet-loaded dir contributes a single synthetic "loading" placeholder row, an
/// errored one a single "error" row. Returns [] when the root isn't loaded
/// (the component shows a top-level spinner instead).
export function flattenLazy(
  dirs: Map<string, DirState>,
  expanded: Set<string>,
  rootDir = "",
  depth = 0,
): TreeRow[] {
  const state = dirs.get(rootDir)
  if (!state || state.status !== "loaded") return []
  const rows: TreeRow[] = []
  for (const entry of state.entries) {
    const isExpanded = entry.is_dir && expanded.has(entry.path)
    const childState = dirs.get(entry.path)
    rows.push(entryRow(entry, depth, isExpanded, childState))
    rows.push(
      ...expandedChildRows(dirs, expanded, entry, childState, depth),
    )
  }
  return rows
}
