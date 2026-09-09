// Pure helpers for the editor's lazy file tree: a partially-loaded view of the
// worktree, where each directory's children are fetched from `/files/tree` the
// first time it is expanded and cached in a Map<dirPath, DirState> the
// component owns.

// One directory entry from the server's `/files/tree` route (mirrors the Rust
// `DirEntryInfo`), pre-sorted dirs-first, case-insensitive.
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

/// The ancestor dirs of `filePath` (root included) that still need fetching,
/// top-down, so a deep link can expand the chain to reveal the file.
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
  // For dir rows: "loading" while the children fetch is in flight or an expanded
  // dir has no cache entry yet, "error" when the fetch failed. Placeholder child
  // rows carry the same state at depth+1.
  state: "idle" | "loading" | "error"
  // Explicit discriminant for placeholder rows, checked instead of the row's
  // path: `flattenLazy` synthesizes `<dir>/__loading__` and `<dir>/__error__`
  // paths purely for React keys, so a real file named `__loading__` still gets
  // `kind: "entry"`.
  kind: "entry" | "loading" | "error"
  // True for a dir row whose cache entry is `loaded` with zero children,
  // independent of `expanded`, so the icon reads empty either way. Always false
  // for file rows and for dirs never fetched.
  empty: boolean
}

/// The cached dir paths strictly nested under `path`, not `path` itself, so a
/// collapse can evict a subtree's cached listings (still-loading and errored
/// entries included) instead of leaking them in memory.
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

/// Flatten the loaded tree into render rows honoring `expanded`. Descends only
/// into dirs that are both expanded and loaded; one not yet loaded contributes a
/// single "loading" placeholder row, an errored one an "error" row. Returns []
/// when the root is not loaded.
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
