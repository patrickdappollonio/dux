import {
  File,
  FileArchive,
  FileCode,
  FileImage,
  FileJson,
  FileLock,
  FileText,
  Folder,
  FolderOpen,
  FolderX,
  type LucideIcon,
} from "lucide-react"

import type { FileIconKind } from "@/lib/fileIcons"
import { cn } from "@/lib/utils"

// Maps a pure `FileIconKind` (see lib/fileIcons.ts) to its lucide glyph. A
// `Record` over the full union means adding a kind without adding it here is a
// compile error, mirrors `FileStatusIcon`'s `ICONS`/`COLORS` Record pattern.
// This is the LEFT-side, always-present file-TYPE icon; it is additive to (not
// a replacement for) the git-status marker `FileStatusIcon`, which stays on the
// right of a row and is untouched by this module.
const ICONS: Record<FileIconKind, LucideIcon> = {
  folder: Folder,
  "folder-open": FolderOpen,
  // Empty folder: FolderX reads as visibly distinct from both a populated
  // folder (Folder/FolderOpen) and a generic file (File) at a glance, without
  // relying on a subtle opacity difference that's easy to miss at 14px.
  "folder-empty": FolderX,
  code: FileCode,
  image: FileImage,
  config: FileJson,
  markdown: FileText,
  text: FileText,
  lock: FileLock,
  binary: FileArchive,
  file: File,
}

export function FileTreeIcon({
  kind,
  className,
}: {
  kind: FileIconKind
  className?: string
}) {
  const Icon = ICONS[kind]
  return (
    <Icon
      className={cn("size-3.5 shrink-0 text-muted-foreground", className)}
    />
  )
}
