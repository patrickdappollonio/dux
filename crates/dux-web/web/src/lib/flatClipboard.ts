import { toast } from "sonner"

import { copyToClipboard } from "@/lib/clipboard"

// Copy a worktree path to the clipboard with the same toast feedback the sidebar
// used inline, extracted so the agent ⋯ menu's "Copy local path" reads cleanly.
export function clipboardWorktree(path: string): void {
  void copyToClipboard(path).then((ok) =>
    ok
      ? toast.success("Copied local path to clipboard")
      : toast.error("Couldn't copy the path"),
  )
}
