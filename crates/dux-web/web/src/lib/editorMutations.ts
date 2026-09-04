// What the editor SAYS when a file mutation lands.
//
// Only the mutations whose outcome is NOT already on screen: a move, which
// takes the entry out of the folder the user was looking at, and a delete,
// whose dialog closes BEFORE the request settles (deliberately; see
// EditorBody's handler) and so leaves no other trace of what happened. A
// create and an in-place rename are read straight off the tree row and the
// open tab, so they say nothing.
//
// Kept pure and here rather than inline at the call sites so the wording is
// one place, testable without mounting the editor, and hard to drift between
// the two shells. Nothing in this file imports the notification raiser: the
// caller decides WHEN to raise, this decides WHAT it says.

/// The two things the editor can create. Mirrors `NewEntryTarget.kind`.
export type EntryKind = "file" | "folder"

/// The noun for an entry, chosen by whether it is a directory.
function noun(isDir: boolean): EntryKind {
  return isDir ? "folder" : "file"
}

/// "Moved notes.md to docs/", or "Moved notes.md to the worktree root".
///
/// The destination is a DIRECTORY here, which is the opposite of the rename
/// case: what changed is where the entry lives, and its name is unchanged. An
/// empty destination is the worktree root, which is a real destination and
/// deserves a real word rather than an empty string or a bare "/".
export function movedMessage(from: string, destDir: string): string {
  if (destDir === "") return `Moved ${from} to the worktree root`
  return `Moved ${from} to ${destDir}/`
}

/// "Deleted file notes.md" / "Deleted folder tools/old".
export function deletedMessage(path: string, isDir: boolean): string {
  return `Deleted ${noun(isDir)} ${path}`
}
