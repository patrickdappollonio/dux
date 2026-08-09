import {
  FilePlus,
  FolderInput,
  FolderPlus,
  Info,
  Pencil,
  Trash2,
} from "lucide-react"

import {
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
} from "@/components/ui/context-menu"

// The presentational content of the file tree's right-click menu, reused for
// every row plus the root/empty-area menu. New File… and New Folder… always
// appear; Rename…, Move…, Delete… and File/Folder info… only for an actual
// file/dir row (the root has nothing to rename, move, delete or describe).
// The info item names the KIND of row it sits on, because the panel it opens
// does too. Every item
// carries a leading lucide icon and a trailing "…" (it opens a dialog). Delete
// stays NEUTRAL colored per the menu tenet: the "…" plus the confirm dialog
// are the danger signal, not red text.
export function FileTreeContextMenu({
  variant,
  onNewFile,
  onNewFolder,
  onRename,
  onMove,
  onDelete,
  onInfo,
}: {
  variant: "file" | "dir" | "root"
  onNewFile: () => void
  onNewFolder: () => void
  onRename: () => void
  onMove: () => void
  onDelete: () => void
  onInfo: () => void
}) {
  return (
    <ContextMenuContent>
      <ContextMenuItem onClick={onNewFile}>
        <FilePlus />
        New File…
      </ContextMenuItem>
      <ContextMenuItem onClick={onNewFolder}>
        <FolderPlus />
        New Folder…
      </ContextMenuItem>
      {variant !== "root" && (
        <>
          <ContextMenuSeparator />
          <ContextMenuItem onClick={onRename}>
            <Pencil />
            Rename…
          </ContextMenuItem>
          <ContextMenuItem onClick={onMove}>
            <FolderInput />
            Move…
          </ContextMenuItem>
          <ContextMenuItem onClick={onDelete}>
            <Trash2 />
            Delete…
          </ContextMenuItem>
          <ContextMenuSeparator />
          {/* The panel this opens calls a folder a "Folder", so the item that
              opens it must not call the same row a file. */}
          <ContextMenuItem onClick={onInfo}>
            <Info />
            {variant === "dir" ? "Folder info…" : "File info…"}
          </ContextMenuItem>
        </>
      )}
    </ContextMenuContent>
  )
}
