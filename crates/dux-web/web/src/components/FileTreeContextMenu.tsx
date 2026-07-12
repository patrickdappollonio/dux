import { FilePlus, FolderPlus, Pencil, Trash2 } from "lucide-react"

import {
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
} from "@/components/ui/context-menu"

// The presentational content of the file tree's right-click menu, reused for
// every row plus the root/empty-area menu. New File… and New Folder… always
// appear; Rename… and Delete… only for an actual file/dir row (the root has
// nothing to rename or delete). Every item carries a leading lucide icon and a
// trailing "…" (it opens a dialog). Delete stays NEUTRAL colored per the menu
// tenet: the "…" plus the confirm dialog are the danger signal, not red text.
export function FileTreeContextMenu({
  variant,
  onNewFile,
  onNewFolder,
  onRename,
  onDelete,
}: {
  variant: "file" | "dir" | "root"
  onNewFile: () => void
  onNewFolder: () => void
  onRename: () => void
  onDelete: () => void
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
          <ContextMenuItem onClick={onDelete}>
            <Trash2 />
            Delete…
          </ContextMenuItem>
        </>
      )}
    </ContextMenuContent>
  )
}
