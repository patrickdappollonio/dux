import {
  FilePlus,
  FolderInput,
  FolderPlus,
  Info,
  Pencil,
  Trash2,
  Upload,
} from "lucide-react"

import {
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
} from "@/components/ui/context-menu"

// The presentational content of the file tree's right-click menu, reused for every
// row and for the root, which has nothing to rename, move, delete or describe.
// Every item carries a leading icon and a trailing "…", and Delete stays NEUTRAL:
// the ellipsis and the confirm dialog are the danger signal, not red text.
export function FileTreeContextMenu({
  variant,
  onNewFile,
  onNewFolder,
  onUpload,
  canUpload = false,
  onRename,
  onMove,
  onDelete,
  onInfo,
}: {
  variant: "file" | "dir" | "root"
  onNewFile: () => void
  onNewFolder: () => void
  /// Opens the browser's file picker and uploads into this row's directory.
  /// Called straight from the click, so the user activation still covers it.
  onUpload?: () => void
  /// Whether the server accepts uploads at all (`file_drop_max_bytes > 0`). With
  /// it off the tree refuses a drop, so this must not be offered either.
  canUpload?: boolean
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
      {/* The picker gesture into the tree's own drop intent, "add this to my
          project": an ordinary visible file in the directory this row means,
          never the agent's upload folder and never a paste into a terminal.
          `Upload`, not the pane menu's paperclip, because that is the intent
          this one carries. The trailing "…" is the operating system's picker
          dialog, like every other item here. */}
      {canUpload && onUpload ? (
        <ContextMenuItem onClick={onUpload}>
          <Upload />
          Upload here…
        </ContextMenuItem>
      ) : null}
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
