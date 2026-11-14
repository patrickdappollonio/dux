import type { worktreeFilesNote } from "@/lib/deleteAgent"
import { renderProse } from "@/lib/prose"
import { cn } from "@/lib/utils"

// One worktree's uncommitted-files line in a delete dialog: muted while git is
// still being asked, in the loss warning's colour once it has answered.
export function WorktreeFilesNote({
  note,
}: {
  note: ReturnType<typeof worktreeFilesNote>
}) {
  if (note === null) return null
  return (
    <p
      className={cn(
        "text-sm",
        note.tone === "quiet" ? "text-muted-foreground" : "text-destructive",
      )}
    >
      {renderProse(note.prose)}
    </p>
  )
}
