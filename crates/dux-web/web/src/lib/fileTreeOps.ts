// Pure helpers for the file tree's context-menu file management flows (New
// File, New Folder, Rename, Delete). Kept free of React so they're trivially
// unit-testable; the server remains the source of truth for containment
// (resolve_worktree_path / is_under / resolves_into_git_dir); validation here
// is UX only, to reject an obviously-bad name before a round trip.

// The directory a create (New File / New Folder) should target, given the
// right-clicked context.
export type CreateContext =
  | { kind: "file"; path: string }
  | { kind: "dir"; path: string }
  | { kind: "root" }

export function targetDirForCreate(ctx: CreateContext): string {
  switch (ctx.kind) {
    case "file":
      return parentDir(ctx.path)
    case "dir":
      return ctx.path
    case "root":
      return ""
  }
}

// "a/b/c.ts" -> "a/b"; "x" -> ""; "" -> "".
export function parentDir(path: string): string {
  const idx = path.lastIndexOf("/")
  return idx === -1 ? "" : path.slice(0, idx)
}

// dir === "" -> name (root); otherwise "dir/name".
export function joinName(dir: string, name: string): string {
  return dir === "" ? name : `${dir}/${name}`
}

// The final worktree-relative target of a rename: parentDir(from) + "/" + newName.
export function renameTarget(from: string, newName: string): string {
  return joinName(parentDir(from), newName)
}

// eslint-disable-next-line no-control-regex
const CONTROL_CHAR_RE = /[\x00-\x1f\x7f]/

// Validate a single path SEGMENT typed into New File/New Folder/Rename (never
// a full path: no "/" is ever valid here). Rejects: empty/whitespace-only,
// a "/" or "\" (would try to create a sub-path or escape), "." or "..", any
// NUL/control char, and a case-insensitive ".git".
export function validateEntryName(
  name: string,
): { ok: true } | { ok: false; error: string } {
  if (name.trim().length === 0) {
    return { ok: false, error: "Name cannot be empty." }
  }
  if (name.includes("/") || name.includes("\\")) {
    return { ok: false, error: "Name cannot contain a slash." }
  }
  if (name === "." || name === "..") {
    return { ok: false, error: `"${name}" is not a valid name.` }
  }
  if (CONTROL_CHAR_RE.test(name)) {
    return { ok: false, error: "Name cannot contain control characters." }
  }
  if (name.toLowerCase() === ".git") {
    return { ok: false, error: '"' + name + '" is reserved.' }
  }
  return { ok: true }
}
