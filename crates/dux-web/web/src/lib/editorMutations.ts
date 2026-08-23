// What the editor SAYS when a file mutation lands.
//
// Every mutation confirms out loud: a silent success reads as "did the click
// register?" for the harmless operations and worse for delete, which closes its
// dialog BEFORE the request settles (deliberately; see EditorBody's handler)
// and so has no other on-screen trace of the outcome. These four match save,
// open-in-editor and the drop path.
//
// Kept pure and here rather than inline at the call sites so the wording is
// one place, testable without mounting the editor, and hard to drift between
// the two shells. Nothing in this file imports the notification raiser: the
// caller decides WHEN to raise, this decides WHAT it says.

import { basename } from "@/lib/fileTreeOps"

/// The two things the editor can create. Mirrors `NewEntryTarget.kind`.
export type EntryKind = "file" | "folder"

/// The noun for an entry, chosen by whether it is a directory.
function noun(isDir: boolean): EntryKind {
  return isDir ? "folder" : "file"
}

/// "Created file src/new.ts" / "Created folder src/vendor".
///
/// The FULL path, not the bare name: the entry was created in whichever
/// directory the tree row (or the header button, which targets the root)
/// pointed at, and naming only "new.ts" leaves the one interesting fact out.
export function createdMessage(kind: EntryKind, path: string): string {
  return `Created ${kind} ${path}`
}

/// "Renamed src/config.toml to config.bak".
///
/// The source in full and the destination as its NEW NAME alone. A rename
/// only ever changes the last segment, so repeating the shared directory on
/// both sides is noise that makes the one changed word harder to find. The
/// split is `fileTreeOps.basename`, the same one the move target is built
/// with, so this file cannot disagree with the rest of the editor about what
/// a path separator is.
export function renamedMessage(from: string, to: string): string {
  return `Renamed ${from} to ${basename(to)}`
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
