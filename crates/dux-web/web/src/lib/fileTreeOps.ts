// Pure helpers for the file tree's file-management flows. The server remains the authority on
// containment; validation here is UX only, rejecting a bad name before a round trip.

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

// The final worktree-relative target of a move: the destination directory plus the source's
// own basename. A move is a rename that keeps the name, so it takes the same server route.
export function moveTarget(from: string, destDir: string): string {
  return joinName(destDir, basename(from))
}

// "a/b/c.ts" -> "c.ts"; "x" -> "x".
export function basename(path: string): string {
  const idx = path.lastIndexOf("/")
  return idx === -1 ? path : path.slice(idx + 1)
}

// Whether a chosen destination directory is a legal target for moving `from`. UX only: the
// server is still the authority on containment and on refusing an occupied destination.
export function validateMove(
  from: string,
  destDir: string,
): { ok: true } | { ok: false; error: string } {
  if (destDir === parentDir(from)) {
    return { ok: false, error: "This is already the folder it is in." }
  }
  // A folder cannot contain itself. Compare on a segment boundary so a sibling that shares a
  // name prefix ("src-old" next to "src") is not read as a descendant.
  if (destDir === from || destDir.startsWith(`${from}/`)) {
    return { ok: false, error: "A folder cannot be moved inside itself." }
  }
  return { ok: true }
}

// eslint-disable-next-line no-control-regex
const CONTROL_CHAR_RE = /[\x00-\x1f\x7f]/

// Validate a single path segment typed into New File/New Folder/Rename; a full path is never
// valid here. Rejects empty, slashes, "." and "..", control chars, and a case-insensitive ".git".
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
