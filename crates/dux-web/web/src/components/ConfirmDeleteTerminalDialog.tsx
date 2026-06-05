import { useEffect } from "react"

import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import {
  closeDeleteTerminal,
  deleteTerminal,
  useDux,
} from "@/lib/store"
import { terminalForeground } from "@/lib/terminals"
import type { TerminalView } from "@/lib/types"

// Confirmation before closing a companion terminal. The TUI ALWAYS confirms
// terminal deletion (its running process is killed), with Cancel as the default
// focus — the web mirrors that exactly. The ✕ on the sidebar/mobile rows opens
// this dialog instead of deleting on a single click.
export function ConfirmDeleteTerminalDialog() {
  const { deleteTerminalTarget, viewModel } = useDux()

  // Derive the terminal from the ViewModel so a process that exits while the
  // dialog is open (the terminal vanishes from the model) closes it gracefully
  // via the effect below, mirroring the TUI's exit handling.
  let terminal: TerminalView | undefined
  if (deleteTerminalTarget && viewModel) {
    for (const session of viewModel.sessions) {
      const found = session.terminals.find((t) => t.id === deleteTerminalTarget)
      if (found) {
        terminal = found
        break
      }
    }
  }

  // If the target was set but no longer exists in the ViewModel, the terminal
  // already closed (its process exited). Drop the pending confirmation so the
  // dialog doesn't linger pointing at a dead terminal.
  useEffect(() => {
    if (deleteTerminalTarget && !terminal) {
      closeDeleteTerminal()
    }
  }, [deleteTerminalTarget, terminal])

  const isOpen = deleteTerminalTarget !== null && terminal !== undefined
  // The title names the STATIC label like the TUI's prompt does ("delete
  // Terminal 1?"); the running command appears in the warning body instead —
  // avoiding the redundant "Close vim?" + "vim is running…" phrasing.
  const title = terminal?.label ?? ""
  const foreground = terminal ? terminalForeground(terminal) : null

  function handleConfirm() {
    if (!deleteTerminalTarget) return
    deleteTerminal(deleteTerminalTarget)
    closeDeleteTerminal()
  }

  function handleOpenChange(open: boolean) {
    if (!open) closeDeleteTerminal()
  }

  return (
    <Dialog open={isOpen} onOpenChange={handleOpenChange}>
      <DialogContent showCloseButton={false}>
        <DialogHeader>
          <DialogTitle>Close {title}?</DialogTitle>
        </DialogHeader>
        <p className="text-sm text-destructive">
          {foreground ? (
            <>
              <span className="font-mono">{foreground}</span> is running in this
              terminal and will be killed.
            </>
          ) : (
            "The running process will be killed."
          )}
        </p>
        {/* Misclick-safe spacing between the warning and the buttons. */}
        <div className="h-2" />
        <DialogFooter>
          {/* Cancel is the default focus, matching the TUI (Cancel highlighted).
              shadcn/radix buttons already activate on Space/Enter natively. */}
          <Button variant="outline" autoFocus onClick={closeDeleteTerminal}>
            Cancel
          </Button>
          <Button variant="destructive" onClick={handleConfirm}>
            Close terminal
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
