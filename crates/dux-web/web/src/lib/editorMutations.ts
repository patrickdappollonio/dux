// What the editor says when a file mutation lands.
//
// Only the mutations whose outcome is not already on screen: a move, which
// takes the entry out of the folder in view, and a delete, whose dialog closes
// before the request settles. A create and an in-place rename are read off the
// tree row and the open tab, so they say nothing. Nothing here imports the
// notification raiser: the caller decides when to raise, this decides what it
// says.

/// The two things the editor can create. Mirrors `NewEntryTarget.kind`.
export type EntryKind = "file" | "folder"

/// The noun for an entry, chosen by whether it is a directory.
function noun(isDir: boolean): EntryKind {
  return isDir ? "folder" : "file"
}

/// "Moved notes.md to docs/", or "Moved notes.md to the worktree root". The
/// destination is a directory, unlike the rename case: the name is unchanged.
/// An empty destination is the worktree root and gets a word rather than "/".
export function movedMessage(from: string, destDir: string): string {
  if (destDir === "") return `Moved ${from} to the worktree root`
  return `Moved ${from} to ${destDir}/`
}

/// "Deleted file notes.md" / "Deleted folder tools/old".
export function deletedMessage(path: string, isDir: boolean): string {
  return `Deleted ${noun(isDir)} ${path}`
}
