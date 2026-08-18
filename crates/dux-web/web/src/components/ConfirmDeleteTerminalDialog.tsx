import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { useVanishedTargetGuard } from "@/hooks/use-vanished-target"
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
  const { deleteTerminalTarget, spine } = useDux()

  // Derive the terminal from the ViewModel so a process that exits while the
  // dialog is open (the terminal vanishes from the model) closes it gracefully
  // via the effect below, mirroring the TUI's exit handling. One flat
  // collection, so this is a lookup by id and cannot miss an owner kind (a
  // session-only scan once resolved a project terminal to `undefined`, so its
  // dialog auto-closed the instant it opened).
  const terminal: TerminalView | undefined =
    deleteTerminalTarget && spine
      ? spine.terminals.find((t) => t.id === deleteTerminalTarget)
      : undefined

  // Closes the dialog when the terminal vanishes from the ViewModel (its
  // process exited); see the hook.
  const isOpen = useVanishedTargetGuard(
    deleteTerminalTarget !== null,
    terminal !== undefined,
    closeDeleteTerminal,
  )
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
      <DialogContent showCloseButton={false} destructive>
        <DialogHeader>
          <DialogTitle>Close {title}?</DialogTitle>
        </DialogHeader>
        {/* Only warn about killing a process when an actual app is running in
            the foreground. The bare shell (no foreground command) is not worth
            warning about, so an idle terminal confirms with just the title. */}
        {foreground ? (
          <p className="text-sm text-destructive">
            <span className="font-mono break-all">{foreground}</span> is running in this
            terminal and will be killed.
          </p>
        ) : null}
        {/* Misclick-safe spacing between the body and the buttons. */}
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
