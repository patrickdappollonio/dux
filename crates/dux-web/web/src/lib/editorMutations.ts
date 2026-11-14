// What the editor says when a file mutation lands.
//
// Only the mutations whose outcome is not already on screen: a move, which
// takes the entry out of the folder in view, a delete, whose dialog closes
// before the request settles, and a rename, whose row is re-sorted under its new
// name and can land outside the part of the tree in view. A create is read off
// the new row and, for a file, the tab it opens, so it says nothing. Nothing
// here imports the notification raiser: the caller decides when to raise, this
// decides what it says.

import { chip, type Prose, prose } from "./prose"

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

/// "Renamed docs/notes.md to todo.md": the path it had, and the name it has
/// now, in the folder it was already in.
export function renamedMessage(from: string, newName: string): Prose {
  return prose`Renamed ${chip(from)} to ${chip(newName)}.`
}

/// "Deleted file notes.md" / "Deleted folder tools/old".
export function deletedMessage(path: string, isDir: boolean): string {
  return `Deleted ${noun(isDir)} ${path}`
}
