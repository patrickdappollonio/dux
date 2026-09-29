// Pure planner for keeping the editor's file tree fresh off the changed-files
// broadcast, the same signal the open buffers are questioned by. A path that
// entered or left the slice may have changed a directory LISTING; a path that
// merely changed content did not.

import type { ChangesSliceView } from "@/lib/editorBuffers"
import type { DirEntry } from "@/lib/fileTree"

// Separates a folded folder's path from its file count in its key. A NUL can
// never be part of a path git reports.
const FOLDER_COUNT_MARK = "\u0000"

// Every path git reports for the worktree, staged and unstaged alike, sorted so
// two reads of the same slice compare equal as strings. A rename contributes
// both of its ends: the listing it left is as stale as the one it arrived in.
//
// A folded folder is one row however many files are inside it, so a file added
// inside an already-untracked folder moves no path. Its key therefore carries
// the folder's file count too, and a moved count reads as the key leaving and
// arriving, which `dirsToRefetch` charges to the listings inside the folder.
export function changedPathsFrom(slice: ChangesSliceView | null): string[] {
  if (!slice) return []
  const paths = new Set<string>()
  for (const f of [...slice.unstaged, ...slice.staged]) {
    if (f.kind) {
      paths.add(`${f.path}/${FOLDER_COUNT_MARK}${f.file_count ?? 0}`)
    } else {
      paths.add(f.path)
    }
    if (f.renamed_from) paths.add(f.renamed_from)
  }
  return [...paths].sort()
}

// The folder a key stands for when it is a folded folder's, else null.
function folderOfKey(key: string): string | null {
  const mark = key.indexOf(FOLDER_COUNT_MARK)
  if (mark === -1) return null
  return key.slice(0, mark).replace(/\/$/, "")
}

// The directory a changed path lives in, "" for the worktree root. A trailing
// slash is stripped, which is how a folded folder's key names the folder.
function parentDirOf(path: string): string {
  const trimmed = path.endsWith("/") ? path.slice(0, -1) : path
  const cut = trimmed.lastIndexOf("/")
  return cut === -1 ? "" : trimmed.slice(0, cut)
}

// The deepest loaded directory at or above `dir`, or null when not even the
// root is loaded. A file in a folder git has only just seen has no loaded
// parent of its own, and the listing that gained an entry is its nearest
// loaded ancestor's.
function nearestLoadedDir(
  dir: string,
  loadedDirs: ReadonlySet<string>,
): string | null {
  let current = dir
  for (;;) {
    if (loadedDirs.has(current)) return current
    if (current === "") return null
    const cut = current.lastIndexOf("/")
    current = cut === -1 ? "" : current.slice(0, cut)
  }
}

// Which loaded directories may list something new or missing, given the changed
// paths before and after the slice moved. Only paths that ENTERED or LEFT count:
// a modified file's content change leaves every listing exactly as it was.
// Each such path is charged to the nearest loaded directory at or above it.
//
// `previousPaths` is null before any slice has been seen, which is a baseline
// rather than a change: the tree was just fetched.
export function dirsToRefetch(
  previousPaths: readonly string[] | null,
  nextPaths: readonly string[],
  loadedDirs: ReadonlySet<string>,
): string[] {
  if (previousPaths === null) return []
  const before = new Set(previousPaths)
  const after = new Set(nextPaths)
  const dirs = new Set<string>()
  for (const path of [...previousPaths, ...nextPaths]) {
    if (before.has(path) && after.has(path)) continue
    const folder = folderOfKey(path)
    const dir = nearestLoadedDir(parentDirOf(folder ?? path), loadedDirs)
    if (dir !== null) dirs.add(dir)
    // Anything may have changed inside a folded folder, at any depth, so every
    // listing loaded inside it is stale.
    if (folder !== null) {
      for (const loaded of loadedDirs) {
        if (loaded === folder || loaded.startsWith(`${folder}/`)) dirs.add(loaded)
      }
    }
  }
  return [...dirs].sort()
}

// The subdirectories a refetched listing no longer has. Their cached listings
// are dropped with them, so a directory deleted and later recreated under the
// same name does not come back holding the old one's children.
export function vanishedDirPaths(
  before: readonly DirEntry[],
  after: readonly DirEntry[],
): string[] {
  const kept = new Set(after.filter((e) => e.is_dir).map((e) => e.path))
  return before.filter((e) => e.is_dir && !kept.has(e.path)).map((e) => e.path)
}
